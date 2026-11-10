// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The controller against a simulated one: an 82077AA's command, execution
//! and result phases, channel 2 of the DMA controller with its page
//! register, and two drives whose disks are images in memory, with their
//! disk change lines.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::IoPorts;

use super::format::{self, DD_720, HD_1440};
use super::{Direction, DriveKind, Error, Fdc, Format, Found, SECTOR, dma};

/// Where the simulated memory that ISA DMA reaches is, and its size.
const MEMORY: u64 = 0x1_0000;
const MEMORY_LEN: usize = 0x1_0000;

/// A disk in a simulated drive.
struct SimDisk {
    format: Format,
    bytes: Vec<u8>,
    read_only: bool,
}

impl SimDisk {
    /// A disk of `format` whose every sector holds its own number in its
    /// first four bytes, little-endian.
    fn new(format: Format) -> SimDisk {
        let mut bytes = vec![0; format.bytes() as usize];
        for (number, sector) in bytes.chunks_mut(SECTOR).enumerate() {
            sector[..4].copy_from_slice(&(number as u32).to_le_bytes());
        }
        SimDisk { format, bytes, read_only: false }
    }

    /// A disk of `format` whose boot sector's parameter block says it has
    /// `sectors` a track and `heads`.
    fn with_boot(format: Format, sectors: u16, heads: u16) -> SimDisk {
        let mut disk = SimDisk::new(format);
        let boot = &mut disk.bytes[..SECTOR];
        boot[0] = 0xeb;
        boot[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
        boot[24..26].copy_from_slice(&sectors.to_le_bytes());
        boot[26..28].copy_from_slice(&heads.to_le_bytes());
        disk
    }
}

/// A simulated drive.
#[derive(Default)]
struct SimDrive {
    /// Whether a drive is on the cable at all.
    attached: bool,
    disk: Option<SimDisk>,
    cylinder: u8,
    /// The disk change line.
    changed: bool,
    steps: u32,
}

impl SimDrive {
    /// Steps its heads to `cylinder`: a step with a disk in brings the
    /// change line down, as a drive's does.
    fn step_to(&mut self, cylinder: u8) {
        if cylinder != self.cylinder {
            self.steps += cylinder.abs_diff(self.cylinder) as u32;
            self.cylinder = cylinder;
            if self.disk.is_some() {
                self.changed = false;
            }
        }
    }
}

/// Channel 2 of the DMA controller.
#[derive(Default)]
struct SimDma {
    address: u16,
    count: u16,
    page: u8,
    mode: u8,
    masked: bool,
    /// Which byte of a 16-bit register comes next: the high one.
    high: bool,
}

/// The simulated controller, DMA controller and drives.
struct Sim {
    dor: u8,
    rate: u8,
    enhanced: bool,
    /// The command's bytes so far.
    command: Vec<u8>,
    results: VecDeque<u8>,
    /// Status reads left before a transfer's results come.
    executing: u32,
    /// Interrupts waiting: ST0 and the cylinder.
    interrupts: VecDeque<(u8, u8)>,
    /// SENSE INTERRUPT STATUS answers left for a reset.
    reset_answers: u8,
    /// The controller never takes a byte.
    stuck: bool,
    drives: [SimDrive; 2],
    dma: SimDma,
    memory: Vec<u8>,
    /// Every command's first byte.
    commands: Vec<u8>,
}

impl Sim {
    fn new() -> Sim {
        Sim {
            dor: 0,
            rate: 2,
            enhanced: true,
            command: Vec::new(),
            results: VecDeque::new(),
            executing: 0,
            interrupts: VecDeque::new(),
            reset_answers: 0,
            stuck: false,
            drives: [
                SimDrive { attached: true, changed: true, ..SimDrive::default() },
                SimDrive::default(),
            ],
            dma: SimDma { masked: true, ..SimDma::default() },
            memory: vec![0; MEMORY_LEN],
            commands: Vec::new(),
        }
    }

    fn status(&mut self) -> u8 {
        if self.dor & 0x04 == 0 || self.stuck {
            return 0x10;
        }
        if self.executing > 0 {
            self.executing -= 1;
            return 0x10;
        }
        if self.results.is_empty() { 0x80 } else { 0xd0 }
    }

    fn read(&mut self, port: u16) -> u8 {
        match port {
            0x3f4 => self.status(),
            0x3f5 => self.results.pop_front().unwrap_or(0xff),
            0x3f7 => {
                let unit = usize::from(self.dor & 0x03);
                // A drive is selected only while its motor is on.
                let selected = self.dor & (0x10 << unit) != 0;
                let changed = !selected || self.drives.get(unit).is_none_or(|d| d.changed);
                u8::from(changed) << 7
            }
            _ => 0xff,
        }
    }

    fn write(&mut self, port: u16, value: u8) {
        match port {
            0x3f2 => {
                if self.dor & 0x04 == 0 && value & 0x04 != 0 {
                    self.reset_answers = 4;
                }
                if value & 0x04 == 0 {
                    self.command.clear();
                    self.results.clear();
                    self.interrupts.clear();
                }
                self.dor = value;
            }
            0x3f5 => self.take(value),
            0x3f7 => self.rate = value & 0x03,
            0x04 | 0x05 => {
                let register =
                    if port == 0x04 { &mut self.dma.address } else { &mut self.dma.count };
                if self.dma.high {
                    *register = (*register & 0x00ff) | u16::from(value) << 8;
                } else {
                    *register = (*register & 0xff00) | u16::from(value);
                }
                self.dma.high = !self.dma.high;
            }
            0x0a if value & 0x03 == 2 => self.dma.masked = value & 0x04 != 0,
            0x0b if value & 0x03 == 2 => self.dma.mode = value,
            0x0c => self.dma.high = false,
            0x81 => self.dma.page = value,
            _ => {}
        }
    }

    /// Takes a command byte; runs the command once it has them all.
    fn take(&mut self, byte: u8) {
        self.command.push(byte);
        let needed = match self.command[0] & 0x1f {
            0x03 | 0x0f => 3,
            0x04 | 0x07 | 0x0a => 2,
            0x05 | 0x06 => 9,
            0x08 | 0x10 => 1,
            0x13 => 4,
            _ => 1,
        };
        if self.command.len() < needed {
            return;
        }
        let command = std::mem::take(&mut self.command);
        self.commands.push(command[0]);
        self.run(&command);
    }

    fn run(&mut self, command: &[u8]) {
        let unit = command.get(1).map_or(0, |b| b & 0x03);
        let head = command.get(1).map_or(0, |b| b >> 2 & 1);
        match command[0] & 0x1f {
            0x03 => {}
            0x04 => {
                let drive = &self.drives[usize::from(unit)];
                let protected = drive.disk.as_ref().is_some_and(|d| d.read_only);
                let st3 = u8::from(protected) << 6 | u8::from(drive.cylinder == 0) << 4 | unit;
                self.results.push_back(st3);
            }
            0x05 | 0x06 => self.transfer(command),
            0x07 => {
                let drive = &mut self.drives[usize::from(unit)];
                if drive.attached {
                    drive.step_to(0);
                    self.interrupts.push_back((0x20 | unit, 0));
                } else {
                    self.interrupts.push_back((0x70 | unit, 79));
                }
            }
            0x08 => {
                if self.reset_answers > 0 {
                    self.results.push_back(0xc0 + 4 - self.reset_answers);
                    self.results.push_back(0);
                    self.reset_answers -= 1;
                } else if let Some((st0, cylinder)) = self.interrupts.pop_front() {
                    self.results.push_back(st0);
                    self.results.push_back(cylinder);
                } else {
                    self.results.push_back(0x80);
                }
            }
            0x0a => {
                let drive = &self.drives[usize::from(unit)];
                let found = drive.disk.as_ref().is_some_and(|d| d.format.rate.bits() == self.rate);
                let (st0, st1) = if found { (0, 0) } else { (0x40, 0x01) };
                self.executing = 3;
                self.results.extend([st0 | head << 2 | unit, st1, 0, drive.cylinder, head, 1, 2]);
            }
            0x0f => {
                self.drives[usize::from(unit)].step_to(command[2]);
                self.interrupts.push_back((0x20 | unit, command[2]));
            }
            0x10 => self.results.push_back(if self.enhanced { 0x90 } else { 0x80 }),
            0x13 if self.enhanced => {}
            _ => self.results.push_back(0x80),
        }
    }

    /// READ DATA or WRITE DATA, by DMA until the count runs out or the
    /// track (both, with MT) ends.
    fn transfer(&mut self, command: &[u8]) {
        let (unit, head) = (command[1] & 0x03, command[1] >> 2 & 1);
        let (cylinder, mut at_head, mut sector, eot) =
            (command[2], command[3], command[4], command[6]);
        let write = command[0] & 0x1f == 0x05;
        let multi = command[0] & 0x80 != 0;
        self.executing = 3;
        let id =
            |st0: u8, st1: u8, st2: u8| [st0 | head << 2 | unit, st1, st2, cylinder, head, 1, 2];
        let drive = &mut self.drives[usize::from(unit)];
        let Some(disk) = drive.disk.as_mut() else {
            self.results.extend(id(0x40, 0x01, 0));
            return;
        };
        if disk.format.rate.bits() != self.rate || head != at_head {
            self.results.extend(id(0x40, 0x01, 0));
            return;
        }
        if drive.cylinder != cylinder {
            self.results.extend(id(0x40, 0x04, 0x10));
            return;
        }
        if write && disk.read_only {
            self.results.extend(id(0x40, 0x02, 0));
            return;
        }
        let into_memory = self.dma.mode & 0x0c == 0x04;
        assert!(!self.dma.masked, "the DMA channel is unmasked for a transfer");
        assert_eq!(into_memory, !write, "the DMA channel moves the right way");
        let start = (u64::from(self.dma.page) << 16 | u64::from(self.dma.address)) - MEMORY;
        let mut left = usize::from(self.dma.count) + 1;
        let mut offset = start as usize;
        let spt = disk.format.sectors;
        let heads = disk.format.heads;
        let mut st = (0, 0);
        while left > 0 {
            if sector > spt || sector > eot {
                if multi && at_head + 1 < heads {
                    (at_head, sector) = (at_head + 1, 1);
                } else {
                    st = (0x40, 0x80);
                    break;
                }
            }
            let number = (usize::from(cylinder) * usize::from(heads) + usize::from(at_head))
                * usize::from(spt)
                + usize::from(sector - 1);
            let on_disk = &mut disk.bytes[number * SECTOR..][..SECTOR];
            let in_memory = &mut self.memory[offset..offset + SECTOR];
            if write {
                on_disk.copy_from_slice(in_memory);
            } else {
                in_memory.copy_from_slice(on_disk);
            }
            offset += SECTOR;
            left -= SECTOR;
            sector += 1;
        }
        self.results.extend(id(st.0, st.1, 0));
    }
}

/// One of the simulated windows of ports: the controller's, the DMA
/// controller's, or the page registers'.
struct Window {
    base: u16,
    sim: Arc<Mutex<Sim>>,
}

impl IoPorts for Window {
    fn len(&self) -> u16 {
        16
    }
    fn read8(&self, offset: u16) -> u8 {
        self.sim.lock().unwrap().read(self.base + offset)
    }
    fn write8(&self, offset: u16, value: u8) {
        self.sim.lock().unwrap().write(self.base + offset, value);
    }
    fn read16(&self, _offset: u16) -> u16 {
        unreachable!("the controller's registers are bytes")
    }
    fn write16(&self, _offset: u16, _value: u16) {
        unreachable!("the controller's registers are bytes")
    }
    fn read32(&self, _offset: u16) -> u32 {
        unreachable!("the controller's registers are bytes")
    }
    fn write32(&self, _offset: u16, _value: u32) {
        unreachable!("the controller's registers are bytes")
    }
}

fn no_wait(_: u32) {}

/// A controller over `sim`, reset.
fn controller(sim: &Arc<Mutex<Sim>>) -> Fdc<Window> {
    let window = |base| Window { base, sim: sim.clone() };
    let mut fdc = Fdc::new(window(super::BASE), window(dma::PORTS.0), window(dma::PAGE_PORTS.0));
    fdc.reset(&mut no_wait).expect("the controller resets");
    fdc
}

fn sim_with(disk: Option<SimDisk>) -> Arc<Mutex<Sim>> {
    let mut sim = Sim::new();
    sim.drives[0].disk = disk;
    Arc::new(Mutex::new(sim))
}

/// Reads simulated memory at `phys`.
fn reader(sim: &Arc<Mutex<Sim>>) -> impl FnMut(u64, &mut [u8; SECTOR]) + '_ {
    move |phys, out| {
        let at = (phys - MEMORY) as usize;
        out.copy_from_slice(&sim.lock().unwrap().memory[at..at + SECTOR]);
    }
}

/// The number in the first four bytes of the sector at `index` of the
/// simulated memory.
fn number_at(sim: &Arc<Mutex<Sim>>, index: usize) -> u32 {
    let memory = &sim.lock().unwrap().memory;
    u32::from_le_bytes(memory[index * SECTOR..][..4].try_into().unwrap())
}

#[test]
fn a_reset_configures_an_enhanced_controller_and_sets_the_rate() {
    let sim = sim_with(None);
    let fdc = controller(&sim);
    assert!(fdc.present());
    assert!(fdc.enhanced());
    let sim = sim.lock().unwrap();
    // VERSION, CONFIGURE, then SPECIFY for 500 kbit/s.
    assert_eq!(&sim.commands[sim.commands.len() - 3..], &[0x10, 0x13, 0x03]);
    assert_eq!(sim.rate, 0, "500 kbit/s");
    assert_eq!(sim.reset_answers, 0, "each drive's reset interrupt was answered");
}

#[test]
fn an_older_controller_is_not_configured() {
    let sim = sim_with(None);
    sim.lock().unwrap().enhanced = false;
    let fdc = controller(&sim);
    assert!(!fdc.enhanced());
    assert!(!sim.lock().unwrap().commands.contains(&0x13));
}

#[test]
fn nothing_at_the_ports_is_no_controller() {
    struct Nothing;
    impl IoPorts for Nothing {
        fn len(&self) -> u16 {
            8
        }
        fn read8(&self, _: u16) -> u8 {
            0xff
        }
        fn write8(&self, _: u16, _: u8) {}
        fn read16(&self, _: u16) -> u16 {
            0xffff
        }
        fn write16(&self, _: u16, _: u16) {}
        fn read32(&self, _: u16) -> u32 {
            u32::MAX
        }
        fn write32(&self, _: u16, _: u32) {}
    }
    let mut fdc = Fdc::new(Nothing, Nothing, Nothing);
    assert!(!fdc.present());
    assert_eq!(fdc.reset(&mut no_wait), Err(Error::Timeout));
}

#[test]
fn a_controller_that_takes_no_byte_times_out() {
    let sim = sim_with(Some(SimDisk::new(HD_1440)));
    let mut fdc = controller(&sim);
    sim.lock().unwrap().stuck = true;
    assert_eq!(fdc.recalibrate(0, &mut no_wait), Err(Error::Timeout));
}

#[test]
fn a_missing_drive_fails_its_recalibration() {
    let sim = sim_with(None);
    let mut fdc = controller(&sim);
    assert_eq!(fdc.recalibrate(1, &mut no_wait), Err(Error::Seek));
    assert_eq!(fdc.recalibrate(0, &mut no_wait), Ok(()));
}

#[test]
fn the_change_line_falls_only_when_the_heads_step_with_a_disk_in() {
    let sim = sim_with(None);
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    assert!(fdc.changed(0), "an empty drive's line is up");
    assert_eq!(fdc.disk_in(0, &mut no_wait), Ok(false));
    assert!(fdc.changed(0), "stepping with no disk leaves it up");

    sim.lock().unwrap().drives[0].disk = Some(SimDisk::new(HD_1440));
    assert!(fdc.changed(0), "a disk going in does not bring it down");
    assert_eq!(fdc.disk_in(0, &mut no_wait), Ok(true));
    assert!(!fdc.changed(0));
    let steps = sim.lock().unwrap().drives[0].steps;
    assert_eq!(fdc.disk_in(0, &mut no_wait), Ok(true));
    assert_eq!(sim.lock().unwrap().drives[0].steps, steps, "a line that is down needs no steps");

    sim.lock().unwrap().drives[0].disk = None;
    sim.lock().unwrap().drives[0].changed = true;
    assert!(fdc.changed(0), "taking the disk out raises it");
}

#[test]
fn the_change_line_is_read_with_the_motor_on() {
    let sim = sim_with(Some(SimDisk::new(HD_1440)));
    let mut fdc = controller(&sim);
    assert!(!fdc.motor(0, true), "its motor was off");
    assert_eq!(fdc.disk_in(0, &mut no_wait), Ok(true));
    assert!(fdc.motor(0, false), "its motor was on");
    // Deselected, the drive drives nothing: the simulation reads that as
    // a change, as a driver that forgot the motor would see one.
    assert!(sim.lock().unwrap().read(0x3f7) & 0x80 != 0);
}

#[test]
fn a_disk_is_detected_by_its_data_rate() {
    for (format, kind) in [(HD_1440, DriveKind::Inch35Hd), (DD_720, DriveKind::Inch35Hd)] {
        let sim = sim_with(Some(SimDisk::with_boot(format, format.sectors.into(), 2)));
        let mut fdc = controller(&sim);
        fdc.motor(0, true);
        let found = fdc.detect(0, kind, MEMORY, &mut reader(&sim), &mut no_wait);
        assert_eq!(found, Ok(Found::Disk(if format == HD_1440 { &HD_1440 } else { &DD_720 })));
    }
}

#[test]
fn a_disk_with_no_boot_sector_takes_the_format_of_its_rate() {
    let sim = sim_with(Some(SimDisk::new(DD_720)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    let found = fdc.detect(0, DriveKind::Inch35Dd, MEMORY, &mut reader(&sim), &mut no_wait);
    assert_eq!(found, Ok(Found::Disk(&DD_720)));
}

#[test]
fn a_boot_sector_of_another_geometry_is_another_format() {
    // A Microsoft DMF disk: 21 sectors a track at 500 kbit/s.
    let sim = sim_with(Some(SimDisk::with_boot(HD_1440, 21, 2)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    let found = fdc.detect(0, DriveKind::Inch35Hd, MEMORY, &mut reader(&sim), &mut no_wait);
    assert_eq!(found, Ok(Found::Other { sectors: 21, heads: 2 }));
}

#[test]
fn a_boot_sector_of_a_hard_disk_s_geometry_takes_the_format_of_its_rate() {
    // Formatted by a tool that writes 63 sectors and 255 heads on every
    // volume: that says nothing of the disk's tracks.
    let sim = sim_with(Some(SimDisk::with_boot(HD_1440, 63, 255)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    let found = fdc.detect(0, DriveKind::Inch35Hd, MEMORY, &mut reader(&sim), &mut no_wait);
    assert_eq!(found, Ok(Found::Disk(&HD_1440)));
}

#[test]
fn a_disk_at_no_rate_the_drive_has_is_unformatted() {
    // A 1.44 MB disk in a 720 KB drive, which reads only 250 kbit/s.
    let sim = sim_with(Some(SimDisk::new(HD_1440)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    let found = fdc.detect(0, DriveKind::Inch35Dd, MEMORY, &mut reader(&sim), &mut no_wait);
    assert_eq!(found, Ok(Found::Unformatted));
    let found = fdc.detect(0, DriveKind::Inch525Hd, MEMORY, &mut reader(&sim), &mut no_wait);
    assert_eq!(found, Ok(Found::Unformatted), "5.25-inch disks are not read");
}

#[test]
fn a_cylinder_is_read_across_both_heads_in_one_command() {
    let sim = sim_with(Some(SimDisk::new(HD_1440)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    // Cylinder 3, from head 0's sector 10 to head 1's last.
    let first = 3 * 36 + 9;
    let count = HD_1440.left_in_cylinder(first);
    assert_eq!(count, 27);
    let read = fdc.transfer(0, Direction::Read, &HD_1440, first, count, MEMORY, &mut no_wait);
    assert_eq!(read, Ok(()));
    for index in 0..count as usize {
        assert_eq!(number_at(&sim, index), first + index as u32);
    }
    let commands = &sim.lock().unwrap().commands;
    assert_eq!(commands.iter().filter(|&&c| c & 0x1f == 0x06).count(), 1);
    assert!(commands.contains(&0xc6), "READ DATA with MT and MFM");
}

#[test]
fn a_write_reaches_the_disk() {
    let sim = sim_with(Some(SimDisk::new(DD_720)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    sim.lock().unwrap().memory[..2 * SECTOR].fill(0xa5);
    let wrote = fdc.transfer(0, Direction::Write, &DD_720, 100, 2, MEMORY, &mut no_wait);
    assert_eq!(wrote, Ok(()));
    let sim = sim.lock().unwrap();
    let disk = sim.drives[0].disk.as_ref().unwrap();
    assert!(disk.bytes[100 * SECTOR..102 * SECTOR].iter().all(|&b| b == 0xa5));
    assert_eq!(disk.bytes[102 * SECTOR], 102, "the next sector is untouched");
    assert_eq!(sim.drives[0].cylinder, 5, "sector 100 of a 720 KB disk is on cylinder 5");
}

#[test]
fn a_write_protected_disk_says_so() {
    let mut disk = SimDisk::new(HD_1440);
    disk.read_only = true;
    let sim = sim_with(Some(disk));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    assert_eq!(fdc.write_protected(0, &mut no_wait), Ok(true));
    let wrote = fdc.transfer(0, Direction::Write, &HD_1440, 0, 1, MEMORY, &mut no_wait);
    assert_eq!(wrote, Err(Error::WriteProtected));
    let read = fdc.transfer(0, Direction::Read, &HD_1440, 0, 1, MEMORY, &mut no_wait);
    assert_eq!(read, Ok(()));
}

#[test]
fn a_read_at_the_wrong_rate_finds_no_sector() {
    let sim = sim_with(Some(SimDisk::new(DD_720)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    let read = fdc.transfer(0, Direction::Read, &HD_1440, 0, 1, MEMORY, &mut no_wait);
    assert_eq!(read, Err(Error::NotFound));
    assert!(Error::NotFound.may_pass());
    assert!(!Error::WriteProtected.may_pass());
}

#[test]
fn transfers_that_do_not_fit_are_refused() {
    let sim = sim_with(Some(SimDisk::new(HD_1440)));
    let mut fdc = controller(&sim);
    fdc.motor(0, true);
    let mut transfer = |first, count, buffer| {
        fdc.transfer(0, Direction::Read, &HD_1440, first, count, buffer, &mut no_wait)
    };
    assert_eq!(transfer(0, 37, MEMORY), Err(Error::BadRequest), "past the cylinder");
    assert_eq!(transfer(2879, 2, MEMORY), Err(Error::BadRequest), "past the disk");
    assert_eq!(transfer(2880, 1, MEMORY), Err(Error::BadRequest), "past the disk");
    assert_eq!(transfer(0, 0, MEMORY), Err(Error::BadRequest), "nothing");
    assert_eq!(transfer(0, 2, 0x1_ff00), Err(Error::BadRequest), "across a 64 KiB line");
    assert_eq!(transfer(0, 1, 0x100_0000), Err(Error::BadRequest), "past 16 MiB");
    assert_eq!(transfer(0, 36, MEMORY), Ok(()));
}

#[test]
fn sectors_are_placed_a_track_then_the_other_head_then_the_next_cylinder() {
    let place = |n| HD_1440.place(n).map(|p| (p.cylinder, p.head, p.sector));
    assert_eq!(place(0), Some((0, 0, 1)));
    assert_eq!(place(17), Some((0, 0, 18)));
    assert_eq!(place(18), Some((0, 1, 1)));
    assert_eq!(place(36), Some((1, 0, 1)));
    assert_eq!(place(2879), Some((79, 1, 18)));
    assert_eq!(place(2880), None);
    assert_eq!(HD_1440.total(), 2880);
    assert_eq!(DD_720.bytes(), 737_280);
    assert_eq!(HD_1440.left_in_cylinder(18), 18);
    assert_eq!(HD_1440.left_in_cylinder(35), 1);
}

#[test]
fn drive_kinds_come_from_the_firmware_s_numbers() {
    assert_eq!(DriveKind::from_number(4), Some(DriveKind::Inch35Hd));
    assert_eq!(DriveKind::from_number(0), None);
    assert_eq!(DriveKind::from_number(6), None);
    assert_eq!(DriveKind::Inch35Ed.formats(), &[HD_1440, DD_720]);
    assert_eq!(DriveKind::Inch35Hd.to_string(), "a 3.5-inch 1.44 MB floppy drive");
}

#[test]
fn a_boot_sector_gives_its_disk_s_geometry() {
    let disk = SimDisk::with_boot(HD_1440, 18, 2);
    let boot: [u8; SECTOR] = disk.bytes[..SECTOR].try_into().unwrap();
    assert_eq!(format::boot_geometry(&boot), Some((18, 2)));
    assert_eq!(format::boot_geometry(&[0; SECTOR]), None);
}

#[test]
fn qemu_s_resources_give_the_first_controller() {
    // IO (Decode16, 0x3F2, 0x3F2, 0, 4), IO (Decode16, 0x3F7, 0x3F7, 0, 1),
    // IRQNoFlags () {6}, DMA (Compatibility, NotBusMaster, Transfer8) {2}.
    let template = [
        0x47, 0x01, 0xf2, 0x03, 0xf2, 0x03, 0x00, 0x04, //
        0x47, 0x01, 0xf7, 0x03, 0xf7, 0x03, 0x00, 0x01, //
        0x22, 0x40, 0x00, //
        0x2a, 0x04, 0x00, //
        0x79, 0x00,
    ];
    let found = format::resources(&template).expect("an I/O descriptor");
    assert_eq!(found.base, 0x3f0);
    assert_eq!(found.irq, Some(6));
    assert_eq!(found.dma, Some(2));
    assert_eq!(format::resources(&[0x79, 0x00]), None);
    assert_eq!(format::resources(&[0x47, 0x01]), None, "a cut descriptor");
}
