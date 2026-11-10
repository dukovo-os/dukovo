// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The channel against a simulated one: its command block, control port
//! and bus-master registers, and up to two devices, disks whose sectors
//! are a vector and optical drives that answer as packet devices do.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, IoPorts, fake_memory, fake_memory_write};

use super::{Channel, Device, Error, Op, Transfer};

thread_local! {
    /// The high half of the test's memory's host addresses: the PRDs hold
    /// the low half, as below 4 GiB.
    static HIGH: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn full(phys: u32) -> u64 {
    HIGH.with(|h| h.get().expect("memory was allocated")) | u64::from(phys)
}

/// `pages` of the test's memory, in the same 4 GiB as the rest of it. The
/// host's allocator may put a block anywhere, so one that lands in other
/// 4 GiB, or across a 4 GiB line, is kept aside and another asked for.
fn alloc(pages: usize, below: bool) -> Option<Dma> {
    assert!(below, "the PRD table is below 4 GiB");
    let mut aside = Vec::new();
    for _ in 0..64 {
        let dma = Dma::fake(pages * 4096);
        let high = dma.phys() & !0xffff_ffff;
        let last = (dma.phys() + (pages * 4096) as u64 - 1) & !0xffff_ffff;
        let fits = HIGH.with(|h| last == high && *h.get().get_or_insert(high) == high);
        if fits {
            HIGH.with(|h| h.set(Some(high)));
            return Some(dma);
        }
        aside.push(dma);
    }
    panic!("the host gave no memory in the test's 4 GiB")
}

fn low(dma: &Dma) -> u32 {
    dma.phys() as u32
}

#[derive(Clone)]
struct SimDisk {
    /// A disk's 512-byte sectors, or an optical drive's disc of 2048-byte
    /// blocks.
    sectors: Vec<u8>,
    atapi: bool,
    /// An optical drive that does DMA.
    dma: bool,
    /// It says it is Serial ATA, behind a SATA controller in IDE mode.
    serial: bool,
    /// Commands it fails, as a bad sector does.
    fails: bool,
    flushes: u32,
    /// It takes 48-bit addresses; one that does not takes 28-bit ones.
    lba48: bool,
    /// It is a CompactFlash card in True IDE mode.
    cfa: bool,
}

impl SimDisk {
    fn new(sectors: usize) -> SimDisk {
        SimDisk {
            sectors: vec![0; sectors * 512],
            atapi: false,
            dma: false,
            serial: false,
            fails: false,
            flushes: 0,
            lba48: true,
            cfa: false,
        }
    }

    /// A CompactFlash card of `sectors`, of 28-bit addresses, as QEMU's
    /// `ide-cf` is.
    fn compact_flash(sectors: usize) -> SimDisk {
        SimDisk { lba48: false, cfa: true, ..SimDisk::new(sectors) }
    }

    fn optical() -> SimDisk {
        SimDisk { atapi: true, ..SimDisk::new(0) }
    }

    /// An optical drive that does DMA, holding a disc of `blocks` blocks
    /// of 2048 bytes, each filled with its number's low byte.
    fn drive(blocks: usize) -> SimDisk {
        let mut sectors = vec![0; blocks * 2048];
        for (block, bytes) in sectors.chunks_mut(2048).enumerate() {
            bytes.fill(block as u8);
        }
        SimDisk { sectors, atapi: true, dma: true, ..SimDisk::new(0) }
    }

    /// IDENTIFY PACKET DEVICE: a removable CD/DVD device of 12-byte
    /// packets.
    fn identify_packet(&self) -> [u16; 256] {
        let mut id = [0u16; 256];
        id[0] = 0b10 << 14 | 5 << 8 | 1 << 7;
        if self.dma {
            id[49] = 1 << 8;
        }
        let model = b"QEMU DVD-ROM                            ";
        for (i, pair) in model.chunks(2).enumerate() {
            id[27 + i] = u16::from_be_bytes([pair[0], pair[1]]);
        }
        for (i, pair) in b"2.5+    ".chunks(2).enumerate() {
            id[23 + i] = u16::from_be_bytes([pair[0], pair[1]]);
        }
        id
    }
    fn identify(&self) -> [u16; 256] {
        let mut id = [0u16; 256];
        let count = (self.sectors.len() / 512) as u64;
        id[49] = 1 << 9;
        if self.lba48 {
            id[82] = 1 << 5;
            id[83] = 0x4000 | 1 << 10 | 1 << 13;
            id[85] = 1 << 5;
            id[86] = 1 << 10 | 1 << 13;
            for (i, word) in id[100..104].iter_mut().enumerate() {
                *word = (count >> (16 * i)) as u16;
            }
        } else {
            // FLUSH CACHE, and no write cache, as QEMU's CompactFlash.
            id[83] = 0x4000 | 1 << 12;
            id[86] = 1 << 12;
            id[60] = count as u16;
            id[61] = (count >> 16) as u16;
        }
        id[87] = 0x4000;
        if self.cfa {
            id[0] = 0x848a;
        }
        if self.serial {
            // Serial ATA Gen 1 and 2.
            id[76] = 0b110;
        }
        let model = b"QEMU HARDDISK                           ";
        for (i, pair) in model.chunks(2).enumerate() {
            id[27 + i] = u16::from_be_bytes([pair[0], pair[1]]);
        }
        id
    }
}

/// A register that keeps what was written before, as 48-bit commands
/// read it: the high byte first, then the low.
#[derive(Clone, Copy, Default)]
struct Pair {
    now: u8,
    before: u8,
}

impl Pair {
    fn write(&mut self, value: u8) {
        self.before = self.now;
        self.now = value;
    }
}

#[derive(Default)]
struct State {
    disks: [Option<SimDisk>; 2],
    selected: usize,
    status: u8,
    error: u8,
    count: Pair,
    lba: [Pair; 3],
    data: VecDeque<u16>,
    control: u8,
    bm_command: u8,
    bm_status: u8,
    prd: u32,
    /// A DMA command waiting for the bus master: read, LBA, count.
    pending: Option<(bool, u64, u32)>,
    /// While set, a started transfer waits for the test.
    hold: bool,
    /// What each transfer's PRDs were.
    prds: Vec<Vec<(u32, u32)>>,
    /// The LBAs and counts of the commands.
    commands: Vec<(u8, u64, u32)>,
    /// The features register.
    features: u8,
    /// While a packet command waits for its packet, the bytes of it so
    /// far.
    packet: Option<Vec<u8>>,
    /// The packets taken, and whether each was to go by DMA.
    packets: Vec<(Vec<u8>, bool)>,
    /// A packet command's byte count limit, and the most the device
    /// sends in one phase.
    limit: u32,
    chunk: u32,
    /// What is left of a packet command's answer by PIO.
    pio: Option<VecDeque<u8>>,
    /// A packet command's answer waiting for the bus master.
    dma: Option<Vec<u8>>,
    /// The sense of the last packet command that failed.
    sense: (u8, u8),
    /// The device register, whose low four bits are a 28-bit address's
    /// top ones.
    device: u8,
    /// A disk's PIO command: whether it reads, the sector it is at, and
    /// how many are left.
    sectors_pio: Option<(bool, u64, u32)>,
    /// A PIO write's sector, as the host writes it.
    incoming: Vec<u8>,
    /// The device asks for data in a phase of no bytes.
    zero_phase: bool,
}

const READY: u8 = 1 << 6;
const BSY: u8 = 1 << 7;
const DRQ: u8 = 1 << 3;
const ERR: u8 = 1 << 0;
/// The interrupt reason: command or data, and towards the host.
const COD: u8 = 1 << 0;
const IO: u8 = 1 << 1;

impl State {
    fn status(&self) -> u8 {
        if self.disks.iter().all(Option::is_none) {
            return 0xff;
        }
        if self.disks[self.selected].is_none() { 0 } else { self.status }
    }

    fn lba48(&self) -> u64 {
        let mut lba = 0u64;
        for (i, pair) in self.lba.iter().enumerate() {
            lba |= u64::from(pair.now) << (8 * i) | u64::from(pair.before) << (8 * (i + 3));
        }
        lba
    }

    /// A 28-bit command's address and count: 0 for 256.
    fn lba28(&self) -> (u64, u32) {
        let lba = u64::from(self.lba[0].now)
            | u64::from(self.lba[1].now) << 8
            | u64::from(self.lba[2].now) << 16
            | u64::from(self.device & 0x0f) << 24;
        let count = u32::from(self.count.now);
        (lba, if count == 0 { 256 } else { count })
    }

    /// A 48-bit command's count: 0 for 65,536.
    fn count48(&self) -> u32 {
        let count = u32::from(u16::from_le_bytes([self.count.now, self.count.before]));
        if count == 0 { 65_536 } else { count }
    }

    /// Starts a disk's PIO command: `read`s, from `lba`, `count` sectors.
    fn start_sectors(&mut self, code: u8, read: bool, lba: u64, count: u32) {
        self.commands.push((code, lba, count));
        let disk = self.disks[self.selected].as_ref().unwrap();
        if disk.fails || (lba + u64::from(count)) * 512 > disk.sectors.len() as u64 {
            self.status = READY | ERR;
            self.error = if disk.fails { 1 << 6 } else { 1 << 4 };
            return;
        }
        self.sectors_pio = Some((read, lba, count));
        self.next_sector();
    }

    /// The next sector of a disk's PIO command, or its end.
    fn next_sector(&mut self) {
        let Some((read, lba, left)) = self.sectors_pio else { return };
        if left == 0 {
            self.sectors_pio = None;
            self.status = READY;
            return;
        }
        if read {
            let disk = self.disks[self.selected].as_ref().unwrap();
            let at = lba as usize * 512;
            let sector = &disk.sectors[at..at + 512];
            self.data = sector.chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        }
        self.status = READY | DRQ;
    }

    fn command(&mut self, code: u8) {
        let Some(disk) = self.disks[self.selected].clone() else { return };
        match code {
            0xec if disk.atapi => {
                self.status = READY | ERR;
                self.error = 1 << 2;
                self.lba[1].now = 0x14;
                self.lba[2].now = 0xeb;
            }
            0xec => {
                self.data = disk.identify().into_iter().collect();
                self.status = READY | DRQ;
            }
            0x25 | 0x35 if disk.lba48 => {
                let (lba, count) = (self.lba48(), self.count48());
                self.commands.push((code, lba, count));
                self.pending = Some((code == 0x25, lba, count));
                self.status = READY | BSY;
            }
            0xc8 | 0xca => {
                let (lba, count) = self.lba28();
                self.commands.push((code, lba, count));
                self.pending = Some((code == 0xc8, lba, count));
                self.status = READY | BSY;
            }
            0x24 | 0x34 if disk.lba48 => {
                let (lba, count) = (self.lba48(), self.count48());
                self.start_sectors(code, code == 0x24, lba, count);
            }
            0x20 | 0x30 => {
                let (lba, count) = self.lba28();
                self.start_sectors(code, code == 0x20, lba, count);
            }
            0xe7 => {
                self.commands.push((code, 0, 0));
                self.disks[self.selected].as_mut().unwrap().flushes += 1;
                self.status = READY;
            }
            0xea if disk.lba48 => {
                self.disks[self.selected].as_mut().unwrap().flushes += 1;
                self.status = READY;
            }
            0xa1 if disk.atapi => {
                self.data = disk.identify_packet().into_iter().collect();
                self.status = READY | DRQ;
            }
            0xa0 if disk.atapi => {
                self.packet = Some(Vec::new());
                self.limit = u32::from(self.lba[1].now) | u32::from(self.lba[2].now) << 8;
                self.count.now = COD;
                self.status = READY | DRQ;
            }
            _ => {
                self.status = READY | ERR;
                self.error = 1 << 2;
            }
        }
    }

    /// The packet `cdb` has come: runs it.
    fn run_packet(&mut self, cdb: &[u8]) {
        let dma = self.features & 1 != 0;
        self.packets.push((cdb.to_vec(), dma));
        let disk = self.disks[self.selected].clone().unwrap();
        let answer = match cdb[0] {
            // TEST UNIT READY.
            0x00 => Vec::new(),
            // REQUEST SENSE: fixed format.
            0x03 => {
                let mut sense = vec![0u8; 18];
                sense[0] = 0x70;
                sense[2] = self.sense.0;
                sense[7] = 10;
                sense[12] = self.sense.1;
                sense.truncate(usize::from(cdb[4]));
                sense
            }
            // INQUIRY.
            0x12 => {
                let mut inquiry = vec![5u8, 0x80, 5, 2, 31, 0, 0, 0];
                inquiry.extend_from_slice(b"QEMU    QEMU DVD-ROM    2.5+");
                inquiry.truncate(usize::from(cdb[4]));
                inquiry
            }
            // READ(10).
            0x28 => {
                let lba = u32::from_be_bytes(cdb[2..6].try_into().unwrap()) as usize;
                let count = usize::from(u16::from_be_bytes([cdb[7], cdb[8]]));
                let (start, end) = (lba * 2048, (lba + count) * 2048);
                if end > disk.sectors.len() {
                    // LOGICAL BLOCK ADDRESS OUT OF RANGE.
                    return self.fail(5, 0x21);
                }
                disk.sectors[start..end].to_vec()
            }
            // INVALID COMMAND OPERATION CODE.
            _ => return self.fail(5, 0x20),
        };
        if answer.is_empty() {
            self.done();
        } else if dma {
            self.dma = Some(answer);
            self.status = READY | BSY;
        } else {
            self.pio = Some(answer.into_iter().collect());
            self.next_phase();
        }
    }

    /// A packet command that ended well.
    fn done(&mut self) {
        self.pio = None;
        self.status = READY;
        self.count.now = COD | IO;
    }

    /// A packet command that failed with the sense key `key` and code
    /// `asc`: CHK, and the key in the error register's upper bits, with
    /// ABRT.
    fn fail(&mut self, key: u8, asc: u8) {
        self.sense = (key, asc);
        self.pio = None;
        self.status = READY | ERR;
        self.error = key << 4 | 1 << 2;
        self.count.now = COD | IO;
    }

    /// The next PIO data phase of a packet command's answer, or its end.
    fn next_phase(&mut self) {
        let Some(left) = self.pio.as_mut() else { return };
        if left.is_empty() {
            return self.done();
        }
        let most = if self.chunk == 0 { self.limit } else { self.limit.min(self.chunk) };
        let size = (left.len() as u32).min(most & !1).max(1);
        let bytes: Vec<u8> = left.drain(..size as usize).collect();
        self.data =
            bytes.chunks(2).map(|p| u16::from_le_bytes([p[0], *p.get(1).unwrap_or(&0)])).collect();
        let size = if self.zero_phase { 0 } else { size };
        self.lba[1].now = size as u8;
        self.lba[2].now = (size >> 8) as u8;
        self.count.now = IO;
        self.status = READY | DRQ;
    }

    /// Runs the pending transfer through the PRD table.
    fn transfer(&mut self) {
        if let Some(answer) = self.dma.take() {
            let prds = self.read_prds();
            let mut offset = 0;
            for (address, size) in prds {
                let end = (offset + size as usize).min(answer.len());
                fake_memory_write(full(address), &answer[offset..end]);
                offset = end;
            }
            self.bm_status &= !1;
            return self.done();
        }
        let Some((read, lba, count)) = self.pending.take() else { return };
        let prds = self.read_prds();
        let disk = self.disks[self.selected].as_mut().unwrap();
        let start = lba as usize * 512;
        let end = start + count as usize * 512;
        if disk.fails || end > disk.sectors.len() {
            self.status = READY | ERR;
            self.error = 1 << 6;
        } else {
            let mut offset = start;
            for (address, size) in prds {
                let size = size as usize;
                if read {
                    fake_memory_write(full(address), &disk.sectors[offset..offset + size]);
                } else {
                    fake_memory(full(address), &mut disk.sectors[offset..offset + size]);
                }
                offset += size;
            }
            self.status = READY;
        }
        self.bm_status &= !1;
    }

    /// The PRD table's entries, recorded.
    fn read_prds(&mut self) -> Vec<(u32, u32)> {
        let mut prds = Vec::new();
        let mut at = self.prd;
        loop {
            let mut entry = [0u8; 8];
            fake_memory(full(at), &mut entry);
            let address = u32::from_le_bytes(entry[0..4].try_into().unwrap());
            let flags = u32::from_le_bytes(entry[4..8].try_into().unwrap());
            let size = if flags & 0xffff == 0 { 0x1_0000 } else { flags & 0xffff };
            prds.push((address, size));
            if flags & 1 << 31 != 0 {
                break;
            }
            at += 8;
        }
        self.prds.push(prds.clone());
        prds
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Command,
    Control,
    BusMaster,
}

#[derive(Clone)]
struct Port {
    sim: Arc<Mutex<State>>,
    block: Block,
}

impl IoPorts for Port {
    fn len(&self) -> u16 {
        if self.block == Block::Control { 1 } else { 8 }
    }

    fn read8(&self, offset: u16) -> u8 {
        let s = self.sim.lock().unwrap();
        match (self.block, offset) {
            (Block::Command, 1) => s.error,
            (Block::Command, 2) => s.count.now,
            (Block::Command, o @ 3..=5) => s.lba[usize::from(o - 3)].now,
            (Block::Command, 7) | (Block::Control, 0) => s.status(),
            (Block::BusMaster, 0) => s.bm_command,
            (Block::BusMaster, 2) => s.bm_status,
            _ => 0,
        }
    }

    fn write8(&self, offset: u16, value: u8) {
        let mut s = self.sim.lock().unwrap();
        match (self.block, offset) {
            (Block::Command, 1) => s.features = value,
            (Block::Command, 2) => s.count.write(value),
            (Block::Command, o @ 3..=5) => s.lba[usize::from(o - 3)].write(value),
            (Block::Command, 6) => {
                s.selected = usize::from(value >> 4 & 1);
                s.device = value;
            }
            (Block::Command, 7) => s.command(value),
            (Block::Control, 0) => {
                if s.control & 4 != 0 && value & 4 == 0 {
                    // The end of a software reset: each device shows its
                    // signature, and the master is picked.
                    s.selected = 0;
                    s.status = READY;
                    s.pending = None;
                    let atapi = s.disks[0].as_ref().is_some_and(|d| d.atapi);
                    let signature = if atapi { (0x14, 0xeb) } else { (0, 0) };
                    s.lba[1].now = signature.0;
                    s.lba[2].now = signature.1;
                }
                s.control = value;
            }
            (Block::BusMaster, 0) => {
                s.bm_command = value;
                if value & 1 != 0 {
                    s.bm_status |= 1;
                    if !s.hold {
                        s.transfer();
                    }
                } else {
                    s.bm_status &= !1;
                }
            }
            (Block::BusMaster, 2) => {
                s.bm_status = s.bm_status & !(value & 6) & !0x60 | value & 0x60;
            }
            _ => {}
        }
    }

    fn read16(&self, offset: u16) -> u16 {
        let mut s = self.sim.lock().unwrap();
        if self.block != Block::Command || offset != 0 {
            return 0;
        }
        let word = s.data.pop_front().unwrap_or(0);
        if s.data.is_empty() {
            s.status &= !DRQ;
            if let Some((true, lba, left)) = s.sectors_pio {
                s.sectors_pio = Some((true, lba + 1, left - 1));
                s.next_sector();
            }
            s.next_phase();
        }
        word
    }

    fn write16(&self, offset: u16, value: u16) {
        let mut s = self.sim.lock().unwrap();
        if self.block != Block::Command || offset != 0 {
            return;
        }
        if let Some((false, lba, left)) = s.sectors_pio {
            s.incoming.extend_from_slice(&value.to_le_bytes());
            if s.incoming.len() == 512 {
                let sector = std::mem::take(&mut s.incoming);
                let selected = s.selected;
                let disk = s.disks[selected].as_mut().unwrap();
                disk.sectors[lba as usize * 512..][..512].copy_from_slice(&sector);
                s.sectors_pio = Some((false, lba + 1, left - 1));
                s.next_sector();
            }
            return;
        }
        let Some(packet) = s.packet.as_mut() else { return };
        packet.extend_from_slice(&value.to_le_bytes());
        if packet.len() == 12 {
            let cdb = s.packet.take().unwrap();
            s.status = READY | BSY;
            s.run_packet(&cdb);
        }
    }

    fn read32(&self, offset: u16) -> u32 {
        let s = self.sim.lock().unwrap();
        if self.block == Block::BusMaster && offset == 4 { s.prd } else { 0 }
    }

    fn write32(&self, offset: u16, value: u32) {
        if self.block == Block::BusMaster && offset == 4 {
            self.sim.lock().unwrap().prd = value;
        }
    }
}

struct Rig {
    sim: Arc<Mutex<State>>,
    channel: Channel<Port>,
}

fn rig(disks: [Option<SimDisk>; 2]) -> Rig {
    let sim = Arc::new(Mutex::new(State { disks, status: READY, ..State::default() }));
    let port = |block| Port { sim: sim.clone(), block };
    let channel = Channel::new(
        port(Block::Command),
        port(Block::Control),
        port(Block::BusMaster),
        &mut alloc,
    )
    .unwrap();
    Rig { sim, channel }
}

fn delay(_us: u32) {}

#[test]
fn finds_a_disk_and_leaves_an_optical_drive() {
    let mut rig = rig([Some(SimDisk::new(2048)), Some(SimDisk::optical())]);
    assert!(rig.channel.present());
    assert!(rig.channel.reset(&mut delay));
    let Device::Ata { identity: id, serial } = rig.channel.identify(0, &mut delay) else {
        panic!("no disk")
    };
    assert!(!serial, "a parallel ATA disk");
    assert_eq!(id.sectors, 2048);
    assert_eq!(radix_ahci::ata::trimmed(&id.model), "QEMU HARDDISK");
    assert!(id.write_cache && id.flush);
    let Device::Atapi(drive) = rig.channel.identify(1, &mut delay) else {
        panic!("no optical drive")
    };
    assert_eq!(radix_ahci::ata::trimmed(&drive.model), "QEMU DVD-ROM");
    assert_eq!((drive.device_type, drive.packet_length, drive.dma), (5, 12, false));
    assert!(drive.removable);
    assert_eq!(rig.sim.lock().unwrap().control & 2, 2, "its interrupt is off");
}

#[test]
fn an_optical_drive_as_master_shows_its_signature() {
    let mut rig = rig([Some(SimDisk::drive(4)), None]);
    assert!(rig.channel.reset(&mut delay));
    let Device::Atapi(drive) = rig.channel.identify(0, &mut delay) else {
        panic!("no optical drive")
    };
    assert!(drive.dma);
    // Asked IDENTIFY PACKET DEVICE at once, from its signature.
    assert!(rig.sim.lock().unwrap().packets.is_empty());
}

/// Runs the packet command `cdb` on `drive`, moving `transfer`, to its
/// end.
fn packet(rig: &mut Rig, drive: u8, cdb: &[u8], mut transfer: Transfer<'_>) -> Result<u32, Error> {
    rig.channel.start_packet(drive, cdb, &transfer, false)?;
    for _ in 0..1000 {
        if let Some(done) = rig.channel.poll_packet(&mut transfer) {
            return done;
        }
    }
    panic!("the packet command never ended")
}

const INQUIRY: [u8; 6] = [0x12, 0, 0, 0, 36, 0];

fn read10(lba: u32, blocks: u16) -> [u8; 10] {
    let [a, b, c, d] = lba.to_be_bytes();
    let [e, f] = blocks.to_be_bytes();
    [0x28, 0, a, b, c, d, 0, e, f, 0]
}

#[test]
fn a_packet_command_answers_by_pio() {
    let mut rig = rig([Some(SimDisk::new(64)), Some(SimDisk::drive(4))]);
    let mut answer = [0u8; 36];
    assert_eq!(packet(&mut rig, 1, &INQUIRY, Transfer::In(&mut answer)), Ok(36));
    assert_eq!(answer[0], 5, "a CD/DVD device");
    assert_eq!(&answer[16..28], b"QEMU DVD-ROM");
    let s = rig.sim.lock().unwrap();
    // Six bytes of command, padded to the twelve of its packet.
    assert_eq!(s.packets, [(vec![0x12, 0, 0, 0, 36, 0, 0, 0, 0, 0, 0, 0], false)]);
    assert_eq!(s.limit, 36, "the byte count limit is what the buffer holds");
    assert_eq!(s.selected, 1);
}

#[test]
fn pio_comes_in_phases_of_what_the_device_says() {
    let mut rig = rig([None, Some(SimDisk::drive(4))]);
    rig.sim.lock().unwrap().chunk = 1000;
    let mut blocks = vec![0u8; 2 * 2048];
    assert_eq!(packet(&mut rig, 1, &read10(1, 2), Transfer::In(&mut blocks)), Ok(4096));
    assert!(blocks[..2048].iter().all(|&b| b == 1) && blocks[2048..].iter().all(|&b| b == 2));
}

#[test]
fn more_than_the_buffer_holds_is_dropped() {
    let mut rig = rig([None, Some(SimDisk::drive(4))]);
    let mut first = [0u8; 100];
    assert_eq!(packet(&mut rig, 1, &read10(3, 1), Transfer::In(&mut first)), Ok(100));
    assert!(first.iter().all(|&b| b == 3));
    // The channel is free for the next.
    let mut answer = [0u8; 36];
    assert_eq!(packet(&mut rig, 1, &INQUIRY, Transfer::In(&mut answer)), Ok(36));
}

#[test]
fn a_read_goes_by_dma() {
    let mut rig = rig([Some(SimDisk::new(64)), Some(SimDisk::drive(8))]);
    let buffer = alloc(2, true).unwrap();
    let buffers = [(low(&buffer), 4096), (low(&buffer) + 4096, 2048)];
    let read =
        packet(&mut rig, 1, &read10(4, 3), Transfer::Dma { into_host: true, buffers: &buffers });
    assert_eq!(read, Ok(6144));
    let mut bytes = vec![0u8; 6144];
    buffer.read(0, &mut bytes);
    for (block, data) in bytes.chunks(2048).enumerate() {
        assert!(data.iter().all(|&b| b == 4 + block as u8), "block {block}");
    }
    let s = rig.sim.lock().unwrap();
    assert!(s.packets.last().unwrap().1, "by DMA");
    assert_eq!(s.prds.last().unwrap(), &buffers);
    assert_eq!(s.bm_command & 1, 0, "the bus master is stopped");
}

#[test]
fn a_failed_packet_command_gives_its_sense_key_and_sense() {
    let mut rig = rig([None, Some(SimDisk::drive(4))]);
    let mut nothing = [0u8; 8];
    let failed = packet(&mut rig, 1, &[0xff, 0, 0, 0, 0, 0], Transfer::In(&mut nothing));
    assert_eq!(failed, Err(Error::Device { status: READY | ERR, error: 5 << 4 | 1 << 2 }));
    let mut sense = [0u8; 18];
    assert_eq!(packet(&mut rig, 1, &[0x03, 0, 0, 0, 18, 0], Transfer::In(&mut sense)), Ok(18));
    assert_eq!((sense[2], sense[12]), (5, 0x20), "ILLEGAL REQUEST, INVALID COMMAND OPERATION CODE");
    // Past the disc's end.
    let failed = packet(&mut rig, 1, &read10(4, 1), Transfer::None);
    assert!(matches!(failed, Err(Error::Device { .. })));
}

#[test]
fn a_command_without_data_ends_at_once() {
    let mut rig = rig([None, Some(SimDisk::drive(4))]);
    assert_eq!(packet(&mut rig, 1, &[0; 6], Transfer::None), Ok(0));
}

#[test]
fn a_disk_and_a_drive_share_the_channel_one_command_at_a_time() {
    let mut rig = rig([Some(SimDisk::new(64)), Some(SimDisk::drive(4))]);
    let buffer = alloc(1, true).unwrap();
    rig.sim.lock().unwrap().hold = true;
    rig.channel.start(0, Op::Read, 0, 1, &[(low(&buffer), 512)]).unwrap();
    let mut answer = [0u8; 36];
    let transfer = Transfer::In(&mut answer);
    assert_eq!(rig.channel.start_packet(1, &INQUIRY, &transfer, false), Err(Error::Busy));
    assert!(!rig.channel.packet_in_flight());
    {
        let mut s = rig.sim.lock().unwrap();
        s.hold = false;
        s.transfer();
    }
    assert_eq!(rig.channel.poll(), Some(Ok(())));
    rig.channel.start_packet(1, &INQUIRY, &transfer, false).unwrap();
    assert!(rig.channel.packet_in_flight());
    assert_eq!(rig.channel.start(0, Op::Read, 0, 1, &[(low(&buffer), 512)]), Err(Error::Busy));
    assert_eq!(rig.channel.poll(), None, "a disk's poll leaves a packet command alone");
}

#[test]
fn a_device_that_breaks_the_protocol_ends_its_command() {
    let mut rig = rig([None, Some(SimDisk::drive(4))]);
    rig.sim.lock().unwrap().zero_phase = true;
    let mut blocks = vec![0u8; 2048];
    let failed = packet(&mut rig, 1, &read10(0, 1), Transfer::In(&mut blocks));
    assert_eq!(failed, Err(Error::Protocol));
    assert!(!rig.channel.busy());
}

#[test]
fn bad_packets_are_refused() {
    let mut rig = rig([None, Some(SimDisk::drive(4))]);
    assert_eq!(
        rig.channel.start_packet(2, &INQUIRY, &Transfer::None, false),
        Err(Error::BadRequest)
    );
    assert_eq!(rig.channel.start_packet(1, &[], &Transfer::None, false), Err(Error::BadRequest));
    assert_eq!(
        rig.channel.start_packet(1, &[0; 17], &Transfer::None, false),
        Err(Error::BadRequest)
    );
}

#[test]
fn a_serial_ata_disk_says_so() {
    let mut disk = SimDisk::new(64);
    disk.serial = true;
    let mut rig = rig([Some(disk), None]);
    assert!(matches!(rig.channel.identify(0, &mut delay), Device::Ata { serial: true, .. }));
}

#[test]
fn an_empty_end_or_channel_is_absent() {
    let mut rig = self::rig([Some(SimDisk::new(64)), None]);
    assert_eq!(rig.channel.identify(1, &mut delay), Device::Absent);
    let rig = self::rig([None, None]);
    assert!(!rig.channel.present(), "a floating bus reads all ones");
}

fn run(
    rig: &mut Rig,
    drive: u8,
    op: Op,
    lba: u64,
    count: u32,
    buffers: &[(u32, u32)],
) -> Result<(), Error> {
    rig.channel.start(drive, op, lba, count, buffers)?;
    rig.channel.poll().expect("done")
}

#[test]
fn what_is_written_reads_back() {
    let mut rig = rig([Some(SimDisk::new(64)), Some(SimDisk::new(64))]);
    let out = alloc(1, true).unwrap();
    let back = alloc(1, true).unwrap();
    let data: Vec<u8> = (0..4096).map(|i| (i * 7) as u8).collect();
    out.write(0, &data);
    assert_eq!(run(&mut rig, 1, Op::Write, 8, 8, &[(low(&out), 4096)]), Ok(()));
    assert_eq!(&rig.sim.lock().unwrap().disks[1].as_ref().unwrap().sectors[4096..8192], &data[..]);
    assert_eq!(
        run(&mut rig, 1, Op::Read, 8, 8, &[(low(&back), 2048), (low(&back) + 2048, 2048)]),
        Ok(())
    );
    let mut read = vec![0u8; 4096];
    back.read(0, &mut read);
    assert_eq!(read, data);
    assert!(rig.sim.lock().unwrap().disks[0].as_ref().unwrap().sectors.iter().all(|&b| b == 0));
    assert_eq!(rig.sim.lock().unwrap().bm_command & 1, 0, "the bus master is stopped");
}

#[test]
fn a_buffer_across_64_kib_takes_two_prds() {
    let mut rig = rig([Some(SimDisk::new(64)), None]);
    // 68 KiB always holds a 64 KiB line with 2 KiB either side, and stays
    // under the host allocator's threshold for blocks of their own, which
    // land anywhere.
    let buffer = alloc(17, true).unwrap();
    let start = low(&buffer);
    // From 2 KiB before the next 64 KiB line, 4 KiB.
    let at = (start | 0xffff).wrapping_add(1).wrapping_sub(2048);
    let at = if at >= start && u64::from(at) + 4096 <= u64::from(start) + 17 * 4096 {
        at
    } else {
        start
    };
    assert_eq!(run(&mut rig, 0, Op::Read, 0, 8, &[(at, 4096)]), Ok(()));
    let prds = rig.sim.lock().unwrap().prds.last().unwrap().clone();
    if at != start {
        assert_eq!(prds, [(at, 2048), (at + 2048, 2048)]);
    }
    assert_eq!(prds.iter().map(|p| p.1).sum::<u32>(), 4096);
}

#[test]
fn addresses_take_48_bits_high_byte_first() {
    let mut rig = rig([Some(SimDisk::new(64)), None]);
    let buffer = alloc(1, true).unwrap();
    let lba = 0x0000_1234_5678_9abc;
    // Past the disk's end: it fails, but the address it saw is whole.
    let result = run(&mut rig, 0, Op::Read, lba, 1, &[(low(&buffer), 512)]);
    assert_eq!(result, Err(Error::Device { status: READY | ERR, error: 1 << 6 }));
    assert_eq!(rig.sim.lock().unwrap().commands.last(), Some(&(0x25, lba, 1)));
    let result = run(&mut rig, 0, Op::Read, 0, 65_536, &[(low(&buffer), 512)]);
    assert_eq!(result, Err(Error::BadRequest), "the buffers hold one sector, not 65,536");
}

#[test]
fn a_flush_reaches_the_disk() {
    let mut rig = rig([Some(SimDisk::new(64)), None]);
    assert_eq!(run(&mut rig, 0, Op::Flush, 0, 0, &[]), Ok(()));
    assert_eq!(rig.sim.lock().unwrap().disks[0].as_ref().unwrap().flushes, 1);
}

#[test]
fn a_failing_disk_says_why() {
    let mut disk = SimDisk::new(64);
    disk.fails = true;
    let mut rig = rig([Some(disk), None]);
    let buffer = alloc(1, true).unwrap();
    let result = run(&mut rig, 0, Op::Read, 0, 1, &[(low(&buffer), 512)]);
    assert_eq!(result, Err(Error::Device { status: READY | ERR, error: 1 << 6 }));
    assert!(!rig.channel.busy());
}

#[test]
fn one_command_at_a_time() {
    let mut rig = rig([Some(SimDisk::new(64)), Some(SimDisk::new(64))]);
    let buffer = alloc(1, true).unwrap();
    rig.sim.lock().unwrap().hold = true;
    rig.channel.start(0, Op::Read, 0, 1, &[(low(&buffer), 512)]).unwrap();
    assert_eq!(rig.channel.poll(), None, "still moving");
    assert_eq!(rig.channel.in_flight(), Some(0));
    assert_eq!(rig.channel.start(1, Op::Read, 0, 1, &[(low(&buffer), 512)]), Err(Error::Busy));
    {
        let mut s = rig.sim.lock().unwrap();
        s.hold = false;
        s.transfer();
    }
    assert_eq!(rig.channel.poll(), Some(Ok(())));
    assert_eq!(rig.channel.poll(), None, "nothing in flight");
    assert_eq!(run(&mut rig, 1, Op::Read, 0, 1, &[(low(&buffer), 512)]), Ok(()));
}

#[test]
fn a_reset_drops_what_was_in_flight() {
    let mut rig = rig([Some(SimDisk::new(64)), None]);
    let buffer = alloc(1, true).unwrap();
    rig.sim.lock().unwrap().hold = true;
    rig.channel.start(0, Op::Write, 0, 1, &[(low(&buffer), 512)]).unwrap();
    assert!(rig.channel.reset(&mut delay));
    assert!(!rig.channel.busy());
    rig.sim.lock().unwrap().hold = false;
    assert_eq!(run(&mut rig, 0, Op::Write, 0, 1, &[(low(&buffer), 512)]), Ok(()));
}

#[test]
fn bad_requests_are_refused() {
    let mut rig = rig([Some(SimDisk::new(64)), None]);
    let buffer = alloc(1, true).unwrap();
    let at = low(&buffer);
    assert_eq!(rig.channel.start(2, Op::Read, 0, 1, &[(at, 512)]), Err(Error::BadRequest));
    assert_eq!(rig.channel.start(0, Op::Read, 0, 0, &[(at, 512)]), Err(Error::BadRequest));
    assert_eq!(rig.channel.start(0, Op::Read, 0, 1, &[(at, 100)]), Err(Error::BadRequest));
    assert_eq!(rig.channel.start(0, Op::Read, 0, 1, &[]), Err(Error::BadRequest));
}

/// Runs the PIO command in flight on `rig` through `buffer`, to its end.
fn pio(rig: &mut Rig, buffer: &Dma) -> Result<(), Error> {
    for _ in 0..1000 {
        if let Some(done) = rig.channel.poll_pio(buffer) {
            return done;
        }
    }
    panic!("the PIO command never ended")
}

#[test]
fn a_compactflash_card_moves_its_sectors_by_pio() {
    let mut card = SimDisk::compact_flash(1024);
    for (i, byte) in card.sectors.iter_mut().enumerate() {
        *byte = (i * 7 % 251) as u8;
    }
    let want = card.sectors.clone();
    let mut rig = rig([None, Some(card)]);
    let Device::Ata { identity, .. } = rig.channel.identify(1, &mut delay) else {
        panic!("no card")
    };
    assert!(identity.cfa && !identity.lba48);
    assert_eq!(identity.sectors, 1024);
    assert!(identity.flush && !identity.write_cache);
    assert!(rig.channel.pio(1) && !rig.channel.pio(0));
    // Its data do not go by DMA.
    let buffer = alloc(3, true).unwrap();
    let prds = [(low(&buffer), 1024)];
    assert_eq!(rig.channel.start(1, Op::Read, 0, 2, &prds), Err(Error::BadRequest));
    // Twenty sectors read, a sector to each DRQ block, eight at most in
    // each poll.
    rig.channel.start_pio(1, Op::Read, 5, 20).unwrap();
    assert_eq!(rig.channel.poll(), None, "a PIO command is polled with poll_pio");
    assert_eq!(rig.channel.poll_pio(&buffer), None);
    assert_eq!(rig.channel.poll_pio(&buffer), None);
    assert_eq!(rig.channel.poll_pio(&buffer), Some(Ok(())));
    let mut got = vec![0u8; 20 * 512];
    buffer.read(0, &mut got);
    assert_eq!(got, want[5 * 512..25 * 512]);
    // Two written, from the buffer's start.
    buffer.write(0, &[0xa5; 1024]);
    rig.channel.start_pio(1, Op::Write, 900, 2).unwrap();
    assert_eq!(pio(&mut rig, &buffer), Ok(()));
    {
        let s = rig.sim.lock().unwrap();
        let card = s.disks[1].as_ref().unwrap();
        assert!(card.sectors[900 * 512..902 * 512].iter().all(|&b| b == 0xa5));
        assert_eq!(card.sectors[902 * 512], want[902 * 512], "no more than asked");
        assert_eq!(s.commands, [(0x20, 5, 20), (0x30, 900, 2)]);
    }
    // FLUSH CACHE, its 28-bit flush.
    rig.channel.start(1, Op::Flush, 0, 0, &[]).unwrap();
    assert_eq!(rig.channel.poll(), Some(Ok(())));
    assert_eq!(rig.sim.lock().unwrap().disks[1].as_ref().unwrap().flushes, 1);
}

#[test]
fn twenty_eight_bit_addresses_carry_their_top_bits_in_the_device_register() {
    let mut rig = rig([Some(SimDisk::compact_flash(64)), None]);
    rig.channel.identify(0, &mut delay);
    let buffer = alloc(1, true).unwrap();
    // At most 256 sectors a command, below 2^28.
    assert_eq!(rig.channel.start_pio(0, Op::Read, 0, 257), Err(Error::BadRequest));
    assert_eq!(rig.channel.start_pio(0, Op::Read, (1 << 28) - 1, 2), Err(Error::BadRequest));
    assert_eq!(rig.channel.start_pio(0, Op::Read, 0, 0), Err(Error::BadRequest));
    // Beyond the card: it says the sector is not there (IDNF).
    rig.channel.start_pio(0, Op::Read, 0x0abc_def0, 256).unwrap();
    assert_eq!(pio(&mut rig, &buffer), Err(Error::Device { status: READY | ERR, error: 1 << 4 }));
    let s = rig.sim.lock().unwrap();
    assert_eq!(s.commands, [(0x20, 0x0abc_def0, 256)]);
    assert_eq!(s.device, 0xa0 | 1 << 6 | 0x0a, "the master, LBA, and the top bits");
}

#[test]
fn a_card_that_fails_a_read_says_why() {
    let card = SimDisk { fails: true, ..SimDisk::compact_flash(64) };
    let mut rig = rig([Some(card), None]);
    rig.channel.identify(0, &mut delay);
    let buffer = alloc(1, true).unwrap();
    rig.channel.start_pio(0, Op::Read, 1, 1).unwrap();
    assert_eq!(pio(&mut rig, &buffer), Err(Error::Device { status: READY | ERR, error: 1 << 6 }));
    // The channel is free again.
    assert!(!rig.channel.busy());
}

#[test]
fn a_disk_without_48_bit_addresses_takes_28_bit_dma_commands() {
    let mut old = SimDisk { lba48: false, ..SimDisk::new(1024) };
    old.sectors[300 * 512..301 * 512].fill(0x5a);
    let mut rig = rig([Some(old), None]);
    let Device::Ata { identity, .. } = rig.channel.identify(0, &mut delay) else {
        panic!("no disk")
    };
    assert!(!identity.lba48 && !identity.cfa && !rig.channel.pio(0));
    let buffer = alloc(32, true).unwrap();
    // 256 sectors, written as a count of 0.
    let prds = [(low(&buffer), 0x1_0000), (low(&buffer) + 0x1_0000, 0x1_0000)];
    rig.channel.start(0, Op::Read, 100, 256, &prds).unwrap();
    assert_eq!(rig.channel.poll(), Some(Ok(())));
    let mut sector = vec![0u8; 512];
    buffer.read(200 * 512, &mut sector);
    assert!(sector.iter().all(|&b| b == 0x5a));
    assert_eq!(rig.channel.start(0, Op::Read, 0, 257, &prds), Err(Error::BadRequest));
    rig.channel.start(0, Op::Write, 7, 1, &[(low(&buffer), 512)]).unwrap();
    assert_eq!(rig.channel.poll(), Some(Ok(())));
    assert_eq!(rig.sim.lock().unwrap().commands, [(0xc8, 100, 256), (0xca, 7, 1)]);
}
