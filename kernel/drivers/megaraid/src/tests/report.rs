// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the files [`gather`] writes and the event line, on answers laid
//! out by hand at the offsets the parsers read.

use std::string::String;
use std::vec::Vec;

use crate::Target;
use crate::dcmd::{event, locale, opcode};
use crate::report::{Failed, Files, Place, SCRATCH, Summary, gather, write_event};

fn p8(b: &mut [u8], off: usize, v: u8) {
    b[off] = v;
}

fn p16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

fn p32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn p64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Space-padded text in a field, as the firmware writes it.
fn ptext(b: &mut [u8], off: usize, len: usize, s: &[u8]) {
    b[off..off + len].fill(b' ');
    b[off..off + s.len()].copy_from_slice(s);
}

// --- answers ----------------------------------------------------------

/// What an H740P says of itself: a battery, 8 GiB of cache, 56 degrees.
fn ctrl_info(bbu: bool) -> Vec<u8> {
    let mut b = vec![0; 0x800];
    p32(&mut b, 180, 2);
    ptext(&mut b, 184, 8, b"BOOT");
    ptext(&mut b, 184 + 8, 32, b"7.0");
    ptext(&mut b, 184 + 72, 8, b"APP");
    ptext(&mut b, 184 + 72 + 8, 32, b"4.300.00-8366");
    ptext(&mut b, 1344, 80, b"PERC H740P Mini");
    ptext(&mut b, 1424, 32, b"SN12345");
    p32(&mut b, 1456, u32::from(bbu));
    p16(&mut b, 1482, 1);
    p16(&mut b, 1484, 2);
    ptext(&mut b, 1600, 96, b"51.13.0-3485");
    p16(&mut b, 0x7a2, 8192);
    p8(&mut b, 0x7c9, 56);
    b
}

/// The volume list: target, state (0 offline .. 3 optimal), blocks.
fn ld_list(entries: &[(u8, u8, u64)]) -> Vec<u8> {
    let mut b = vec![0; 8 + entries.len() * 16];
    p32(&mut b, 0, entries.len() as u32);
    for (i, &(target, state, blocks)) in entries.iter().enumerate() {
        let at = 8 + i * 16;
        p8(&mut b, at, target);
        p8(&mut b, at + 4, state);
        p64(&mut b, at + 8, blocks);
    }
    b
}

/// What `LD_GET_INFO` says of a volume.
struct Ld<'a> {
    target: u8,
    name: &'a [u8],
    level: u8,
    spans: &'a [u16],
    state: u8,
    default_policy: u8,
    current_policy: u8,
    task: Option<(u32, u16, u16)>,
}

impl Default for Ld<'_> {
    fn default() -> Self {
        Ld {
            target: 0,
            name: b"",
            level: 1,
            spans: &[1],
            state: 3,
            default_policy: 0x01 | 0x04,
            current_policy: 0x01 | 0x04,
            task: None,
        }
    }
}

fn ld_info(ld: &Ld<'_>) -> Vec<u8> {
    let mut b = vec![0; 384];
    p8(&mut b, 0, ld.target);
    ptext(&mut b, 4, 16, ld.name);
    p8(&mut b, 20, ld.default_policy);
    p8(&mut b, 23, ld.current_policy);
    p8(&mut b, 22, 0);
    p8(&mut b, 32, ld.level);
    p8(&mut b, 35, 7);
    p8(&mut b, 37, ld.spans.len() as u8);
    p8(&mut b, 38, ld.state);
    for (i, &r) in ld.spans.iter().enumerate() {
        p16(&mut b, 64 + i * 24 + 16, r);
    }
    if let Some((bit, fraction, secs)) = ld.task {
        // The bit of the progress flags, and the progress at its offset.
        p32(&mut b, 264, 1 << bit);
        p16(&mut b, 268 + bit as usize * 4, fraction);
        p16(&mut b, 270 + bit as usize * 4, secs);
    }
    b
}

/// The disk list: device ID, enclosure device ID, index, slot, SCSI type.
fn pd_list(entries: &[(u16, u16, u8, u8, u8)]) -> Vec<u8> {
    let mut b = vec![0; 8 + entries.len() * 24];
    p32(&mut b, 4, entries.len() as u32);
    for (i, &(id, enc, index, slot, kind)) in entries.iter().enumerate() {
        let at = 8 + i * 24;
        p16(&mut b, at, id);
        p16(&mut b, at + 2, enc);
        p8(&mut b, at + 4, index);
        p8(&mut b, at + 5, slot);
        p8(&mut b, at + 6, kind);
    }
    b
}

/// What `PD_GET_INFO` says of a disk.
struct Pd<'a> {
    id: u16,
    state: u16,
    interface: u32,
    media: u8,
    vendor: &'a [u8],
    model: &'a [u8],
    serial: &'a [u8],
    task: Option<(u32, u16, u16)>,
}

impl Default for Pd<'_> {
    fn default() -> Self {
        Pd {
            id: 0,
            state: 0x18,
            interface: 2,
            media: 1,
            vendor: b"SEAGATE",
            model: b"ST600MM0009",
            serial: b"S0N1A2B3",
            task: None,
        }
    }
}

fn pd_info(pd: &Pd<'_>) -> Vec<u8> {
    let mut b = vec![0; 512];
    p16(&mut b, 0, pd.id);
    ptext(&mut b, 4 + 8, 8, pd.vendor);
    ptext(&mut b, 4 + 16, 16, pd.model);
    ptext(&mut b, 4 + 32, 4, b"0003");
    p8(&mut b, 101, 0x80);
    p8(&mut b, 103, pd.serial.len() as u8);
    b[104..104 + pd.serial.len()].copy_from_slice(pd.serial);
    p32(&mut b, 168, 1);
    p32(&mut b, 172, 2);
    p32(&mut b, 176, 3);
    p16(&mut b, 184, pd.state);
    p32(&mut b, 188, pd.interface << 12);
    p64(&mut b, 232, 1_172_123_568);
    p64(&mut b, 248, 1_170_000_000);
    if let Some((bit, fraction, secs)) = pd.task {
        p32(&mut b, 260, 1 << bit);
        p16(&mut b, 264 + bit as usize * 4, fraction);
        p16(&mut b, 266 + bit as usize * 4, secs);
    }
    p8(&mut b, 370, pd.media);
    p8(&mut b, 402, 31);
    b
}

/// The configuration: arrays (reference, members), volumes (target,
/// array references) and spares (device ID, dedicated).
fn config(arrays: &[(u16, &[u16])], lds: &[(u8, &[u16])], spares: &[(u16, bool)]) -> Vec<u8> {
    let (a, l, s) = (288, 256, 40);
    let mut b = vec![0; 32 + arrays.len() * a + lds.len() * l + spares.len() * s];
    let total = b.len() as u32;
    p32(&mut b, 0, total);
    p16(&mut b, 4, arrays.len() as u16);
    p16(&mut b, 6, a as u16);
    p16(&mut b, 8, lds.len() as u16);
    p16(&mut b, 10, l as u16);
    p16(&mut b, 12, spares.len() as u16);
    p16(&mut b, 14, s as u16);
    for (i, &(array_ref, members)) in arrays.iter().enumerate() {
        let at = 32 + i * a;
        p8(&mut b, at + 8, members.len() as u8);
        p16(&mut b, at + 10, array_ref);
        for (m, &id) in members.iter().enumerate() {
            p16(&mut b, at + 32 + m * 8, id);
        }
    }
    for (i, &(target, refs)) in lds.iter().enumerate() {
        let at = 32 + arrays.len() * a + i * l;
        p8(&mut b, at, target);
        p8(&mut b, at + 37, refs.len() as u8);
        for (sp, &r) in refs.iter().enumerate() {
            p16(&mut b, at + 64 + sp * 24 + 16, r);
        }
    }
    for (i, &(id, dedicated)) in spares.iter().enumerate() {
        let at = 32 + arrays.len() * a + lds.len() * l + i * s;
        p16(&mut b, at, id);
        p8(&mut b, at + 4, u8::from(dedicated));
    }
    b
}

/// The battery's status: type, flags.
fn bbu_status(kind: u8, flags: u32) -> Vec<u8> {
    let mut b = vec![0; 64];
    p8(&mut b, 0, kind);
    p16(&mut b, 2, 9450);
    b[4..6].copy_from_slice(&(-120i16).to_le_bytes());
    p16(&mut b, 6, 28);
    p32(&mut b, 8, flags);
    p16(&mut b, 34, 100);
    p8(&mut b, 42, 1);
    b
}

// --- the controller ---------------------------------------------------

/// A controller's answers: what each command gets, by opcode and, where
/// it has one, mailbox. A command not listed is refused.
#[derive(Default)]
struct Fw {
    answers: Vec<(u32, Option<[u8; 12]>, Vec<u8>)>,
    asked: Vec<u32>,
    /// The disks the host has, as passed through.
    host: Vec<u16>,
}

impl Fw {
    fn new(ctrl: Vec<u8>, lds: Vec<u8>, pds: Vec<u8>) -> Fw {
        let mut fw = Fw::default();
        fw.any(opcode::CTRL_GET_INFO, ctrl);
        fw.any(opcode::LD_GET_LIST, lds);
        fw.any(opcode::PD_LIST_QUERY, pds);
        fw
    }

    fn any(&mut self, op: u32, answer: Vec<u8>) {
        self.answers.push((op, None, answer));
    }

    fn with(&mut self, op: u32, mailbox: [u8; 12], answer: Vec<u8>) {
        self.answers.push((op, Some(mailbox), answer));
    }

    fn refuse(&mut self, op: u32) {
        self.answers.retain(|a| a.0 != op);
    }

    fn volume(&mut self, ld: &Ld<'_>) {
        self.with(
            opcode::LD_GET_INFO,
            crate::dcmd::mbox::ld_info(u16::from(ld.target)),
            ld_info(ld),
        );
    }

    /// A disk, which the host has if the firmware passes it through.
    fn disk(&mut self, pd: &Pd<'_>) {
        self.with(opcode::PD_GET_INFO, crate::dcmd::mbox::pd_info(pd.id), pd_info(pd));
        if pd.state == 0x40 {
            self.host.push(pd.id);
        }
    }

    /// Runs the commands: the answer is copied as far as the room goes.
    fn run(&mut self, op: u32, mailbox: &[u8; 12], buf: &mut [u8]) -> Result<usize, Failed> {
        self.asked.push(op);
        let (_, _, answer) = self
            .answers
            .iter()
            .find(|(o, m, _)| *o == op && m.is_none_or(|m| m == *mailbox))
            .ok_or(Failed)?;
        let n = answer.len().min(buf.len());
        buf[..n].copy_from_slice(&answer[..n]);
        Ok(n)
    }
}

/// What a gathering wrote.
#[derive(Default)]
struct Out {
    controller: String,
    volumes: String,
    disks: String,
    battery: String,
}

const PLACE: Place<'static> = Place {
    interface: "fusion",
    chip: "SAS3508",
    pci: "0000:3b:00.0",
    numa: Some(0),
    commands: 256,
};

/// The names the host gives: `sd` and a letter by ID, volumes first, of
/// every volume and of the disks it has (`host`).
fn os_name(
    host: &[u16],
    target: Target,
    out: &mut dyn core::fmt::Write,
) -> Result<bool, core::fmt::Error> {
    match target {
        Target::Volume(t) => write!(out, "sd{}", char::from(b'a' + t as u8))?,
        Target::Disk(id) if host.contains(&id) => {
            write!(out, "sd{}", char::from(b'k' + id as u8))?;
        }
        Target::Disk(_) => return Ok(false),
    }
    Ok(true)
}

fn run_with(fw: &mut Fw, scratch: usize) -> (Result<Summary, Failed>, Out) {
    let mut out = Out::default();
    let mut scratch = vec![0u8; scratch];
    let host = fw.host.clone();
    let os = |target: Target, out: &mut dyn core::fmt::Write| os_name(&host, target, out);
    let mut fetch = |op: u32, m: &[u8; 12], buf: &mut [u8]| fw.run(op, m, buf);
    let result = gather(
        &mut fetch,
        &mut scratch,
        &PLACE,
        &os,
        &mut Files {
            controller: &mut out.controller,
            volumes: &mut out.volumes,
            disks: &mut out.disks,
            battery: &mut out.battery,
        },
    );
    (result, out)
}

fn run(fw: &mut Fw) -> (Result<Summary, Failed>, Out) {
    run_with(fw, SCRATCH)
}

/// The disk record of the R740's first disk, as every test of it reads.
fn disk_line(id: u16, slot: u8, volume: &str, os: &str) -> String {
    format!(
        "id={id}\tenclosure=32\tindex=0\tslot={slot}\tstate=online\tinterface=sas\tmedia=hdd\t\
         vendor=SEAGATE\tmodel=ST600MM0009\tserial=S0N1A2B3\trevision=0003\tsize=598560000000\t\
         raw-size=600127266816\tmedia-errors=1\tother-errors=2\tpredictive=3\tsmart-alert=yes\t\
         temperature=31\tvolume={volume}\tspare=-\tos={os}\ttask=none\tpercent=-\telapsed=-\n"
    )
}

/// A RAID 1 of two SAS disks in one enclosure and a battery.
fn r740() -> Fw {
    let mut fw = Fw::new(
        ctrl_info(true),
        ld_list(&[(0, 3, 1_171_000_000)]),
        pd_list(&[(0, 32, 0, 0, 0), (1, 32, 0, 1, 0), (32, 32, 0, 24, 13)]),
    );
    fw.volume(&Ld::default());
    fw.disk(&Pd { id: 0, ..Pd::default() });
    fw.disk(&Pd { id: 1, ..Pd::default() });
    fw.any(opcode::CFG_READ, config(&[(1, &[0, 1])], &[(0, &[1])], &[]));
    fw.any(opcode::BBU_GET_STATUS, bbu_status(2, 0));
    fw
}

#[test]
fn raid1_of_two_sas_disks_with_a_battery() {
    let mut fw = r740();
    let (result, out) = run(&mut fw);
    assert_eq!(result, Ok(Summary { task: false, battery: true, disks: 2 }));
    assert_eq!(
        out.controller,
        "model PERC H740P Mini\nserial SN12345\npackage 51.13.0-3485\nfirmware 4.300.00-8366\n\
         interface fusion\nchip SAS3508\npci 0000:3b:00.0\nnuma 0\ncache 8589934592\nmode raid\n\
         battery battery\ntemperature 56\nvolumes 1\ndegraded 0\noffline 0\ndisks 2\ncritical 1\n\
         failed 2\ncommands 256\n"
    );
    assert_eq!(
        out.volumes,
        "target=0\tname=-\tlevel=raid1\tsize=599552000000\tstripe=65536\tstate=optimal\t\
         write=back\tdefault-write=back\tread=ahead\tdisk-cache=default\tdisks=0,1\tos=sda\t\
         task=none\tpercent=-\telapsed=-\n"
    );
    assert_eq!(
        out.disks,
        disk_line(0, 0, "0", "-").replace("size=598560000000", "size=599040000000")
            + &disk_line(1, 1, "0", "-").replace("size=598560000000", "size=599040000000")
    );
    assert_eq!(
        out.battery,
        "type battery\nstate optimal\ncharge 100\ntemperature 28\nvoltage 9450\ncurrent -120\n\
         learn none\nwrite-through no\nflags -\n"
    );
}

#[test]
fn a_degraded_volume_with_a_rebuilding_disk() {
    let mut fw = Fw::new(
        ctrl_info(false),
        ld_list(&[(0, 2, 2_000_000)]),
        pd_list(&[(4, 32, 0, 4, 0), (5, 32, 0, 5, 0), (6, 32, 0, 6, 0)]),
    );
    fw.volume(&Ld { state: 2, ..Ld::default() });
    fw.disk(&Pd { id: 4, state: 0x11, ..Pd::default() });
    fw.disk(&Pd { id: 5, state: 0x14, task: Some((0, 0x8000, 90)), ..Pd::default() });
    fw.disk(&Pd { id: 6, state: 0x02, ..Pd::default() });
    fw.any(opcode::CFG_READ, config(&[(1, &[4, 5, 0xffff])], &[(0, &[1])], &[(6, true)]));
    let (result, out) = run(&mut fw);
    assert_eq!(result, Ok(Summary { task: true, battery: false, disks: 3 }));
    assert!(out.controller.contains("battery none\n"));
    assert!(out.controller.contains("degraded 1\noffline 0\ndisks 3\n"));
    assert!(out.volumes.contains("state=degraded\t"));
    assert!(out.volumes.contains("\tdisks=4,5\t"));
    let disks: Vec<&str> = out.disks.lines().collect();
    assert_eq!(disks.len(), 3);
    assert!(disks[0].contains("\tstate=failed\t") && disks[0].contains("\tvolume=0\t"));
    assert!(disks[1].contains("\tstate=rebuilding\t"));
    assert!(disks[1].ends_with("\ttask=rebuild\tpercent=50\telapsed=90"));
    assert!(disks[2].contains("\tstate=hot-spare\t"));
    assert!(disks[2].contains("\tvolume=-\tspare=dedicated\tos=-\ttask=none"));
    assert_eq!(out.battery, "");
}

#[test]
fn volume_tasks_by_precedence() {
    // A check at 25 percent after 30 seconds, then a reconstruction.
    let mut fw = Fw::new(ctrl_info(false), ld_list(&[(0, 3, 100)]), pd_list(&[]));
    fw.volume(&Ld { task: Some((0, 0x4000, 30)), ..Ld::default() });
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.task), Ok(true));
    assert!(out.volumes.ends_with("\ttask=check\tpercent=25\telapsed=30\n"));
    for (bit, name) in [(0, "check"), (1, "background-init"), (2, "init"), (3, "reconstruct")] {
        let mut fw = Fw::new(ctrl_info(false), ld_list(&[(0, 3, 100)]), pd_list(&[]));
        fw.volume(&Ld { task: Some((bit, 0xffff, 5)), ..Ld::default() });
        let (_, out) = run(&mut fw);
        assert!(out.volumes.ends_with(&format!("\ttask={name}\tpercent=100\telapsed=5\n")));
    }
}

#[test]
fn jbod_disks_only() {
    let mut fw = Fw::new(
        ctrl_info(false),
        ld_list(&[]),
        pd_list(&[(0, 0xffff, 0, 0, 0), (1, 0xffff, 0, 1, 0)]),
    );
    fw.disk(&Pd { id: 0, state: 0x40, interface: 5, media: 2, ..Pd::default() });
    fw.disk(&Pd { id: 1, state: 0x40, interface: 3, media: 1, ..Pd::default() });
    fw.any(opcode::CFG_READ, config(&[], &[], &[]));
    let (result, out) = run(&mut fw);
    assert_eq!(result, Ok(Summary { task: false, battery: false, disks: 2 }));
    assert!(out.controller.contains("mode hba\n"));
    assert!(out.controller.contains("volumes 0\n"));
    assert_eq!(out.volumes, "");
    let disks: Vec<&str> = out.disks.lines().collect();
    assert!(disks[0].starts_with(
        "id=0\tenclosure=-\tindex=-\tslot=0\tstate=jbod\tinterface=nvme\tmedia=ssd\t"
    ));
    assert!(disks[0].contains("\tvolume=-\tspare=-\tos=sdk\ttask=none"));
    assert!(disks[1].contains("\tinterface=sata\tmedia=hdd\t"));
    assert!(disks[1].contains("\tos=sdl\t"));
}

#[test]
fn a_disk_the_host_has_is_passed_through_whatever_its_state_says() {
    // QEMU's models say in PD_GET_INFO that every disk is offline, while
    // they hand the host the disk in JBOD mode.
    let mut fw = Fw::new(ctrl_info(false), ld_list(&[]), pd_list(&[(0, 0xffff, 0, 0, 0)]));
    fw.disk(&Pd { id: 0, state: 0x10, ..Pd::default() });
    fw.host.push(0);
    fw.any(opcode::CFG_READ, config(&[], &[], &[]));
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.disks), Ok(1));
    assert!(out.controller.contains(
        "mode hba
"
    ));
    assert!(out.disks.starts_with("id=0	enclosure=-	index=-	slot=0	state=offline	"));
    assert!(out.disks.contains("	volume=-	spare=-	os=sdk	"));
}

#[test]
fn volumes_and_jbod_disks_are_mixed() {
    let mut fw = Fw::new(
        ctrl_info(true),
        ld_list(&[(0, 3, 1000), (2, 0, 2000)]),
        pd_list(&[(0, 32, 0, 0, 0), (1, 32, 0, 1, 0), (2, 32, 0, 2, 0)]),
    );
    fw.volume(&Ld { name: b"data", level: 0, ..Ld::default() });
    fw.volume(&Ld { target: 2, state: 0, spans: &[2], ..Ld::default() });
    fw.disk(&Pd { id: 0, ..Pd::default() });
    fw.disk(&Pd { id: 1, state: 0x40, ..Pd::default() });
    fw.disk(&Pd { id: 2, ..Pd::default() });
    fw.any(opcode::CFG_READ, config(&[(1, &[0]), (2, &[2])], &[(0, &[1]), (2, &[2])], &[]));
    fw.any(opcode::BBU_GET_STATUS, bbu_status(1, 1 << 3));
    let (result, out) = run(&mut fw);
    assert_eq!(result, Ok(Summary { task: false, battery: true, disks: 3 }));
    assert!(out.controller.contains("mode mixed\nbattery battery\n"));
    assert!(out.controller.contains("volumes 2\ndegraded 0\noffline 1\n"));
    let volumes: Vec<&str> = out.volumes.lines().collect();
    assert!(volumes[0].starts_with("target=0\tname=data\tlevel=raid0\tsize=512000\t"));
    assert!(volumes[0].contains("\tdisks=0\tos=sda\t"));
    assert!(volumes[1].starts_with("target=2\tname=-\tlevel=raid1\t"));
    assert!(volumes[1].contains("\tstate=offline\t") && volumes[1].contains("\tdisks=2\tos=sdc\t"));
    let disks: Vec<&str> = out.disks.lines().collect();
    assert!(disks[1].contains("	state=jbod	"));
    assert!(disks[1].contains("	volume=-	spare=-	os=sdl	"));
    assert!(out.battery.contains("state charging\n"));
    assert!(out.battery.ends_with("flags charging\n"));
}

#[test]
fn a_battery_that_writes_through_and_its_states() {
    let mut fw = r740();
    fw.refuse(opcode::LD_GET_INFO);
    fw.volume(&Ld { current_policy: 0x04, ..Ld::default() });
    let (_, out) = run(&mut fw);
    assert!(out.battery.contains("write-through yes\n"));
    assert!(out.volumes.contains("\twrite=through\tdefault-write=back\t"));
    for (flags, state, learn, names) in [
        (1u32, "missing", "none", "pack-missing"),
        (1 << 6, "learning", "active", "learn-active"),
        (1 << 4, "discharging", "none", "discharging"),
        (1 << 1 | 1 << 9, "degraded", "none", "voltage-low,i2c-errors"),
        (1 << 7, "degraded", "failed", "learn-failed"),
        (1 << 8, "degraded", "timed-out", "learn-timeout"),
        (1 << 5, "optimal", "requested", "learn-requested"),
    ] {
        let mut fw = r740();
        fw.refuse(opcode::BBU_GET_STATUS);
        fw.any(opcode::BBU_GET_STATUS, bbu_status(2, flags));
        let (_, out) = run(&mut fw);
        assert!(out.battery.contains(&format!("state {state}\n")), "{flags:#x}");
        assert!(out.battery.contains(&format!("learn {learn}\n")), "{flags:#x}");
        assert!(out.battery.ends_with(&format!("flags {names}\n")), "{flags:#x}");
    }
    // A pack whose health is bad has failed.
    let mut status = bbu_status(2, 0);
    status[42] = 0;
    let mut fw = r740();
    fw.refuse(opcode::BBU_GET_STATUS);
    fw.any(opcode::BBU_GET_STATUS, status);
    let (_, out) = run(&mut fw);
    assert!(out.battery.contains("state failed\n"));
}

#[test]
fn no_battery() {
    let mut fw = r740();
    fw.answers.retain(|a| a.0 != opcode::CTRL_GET_INFO);
    fw.any(opcode::CTRL_GET_INFO, ctrl_info(false));
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.battery), Ok(false));
    assert!(out.controller.contains("battery none\n"));
    assert_eq!(out.battery, "");
    assert!(!fw.asked.contains(&opcode::BBU_GET_STATUS));
}

#[test]
fn a_battery_the_controller_will_not_describe() {
    let mut fw = r740();
    fw.refuse(opcode::BBU_GET_STATUS);
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.battery), Ok(false));
    assert!(out.controller.contains("battery other\n"));
    assert_eq!(out.battery, "");
}

#[test]
fn the_configuration_refused() {
    let mut fw = r740();
    fw.refuse(opcode::CFG_READ);
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.disks), Ok(2));
    assert!(out.volumes.contains("\tdisk-cache=default\tdisks=-\tos=sda\t"));
    let first = out.disks.lines().next().unwrap();
    assert!(first.contains("\tvolume=-\tspare=-\tos=-\t"));
    // Nor a scratch that cannot hold it makes the rest fail.
    let mut fw = r740();
    let (result, out) = run_with(&mut fw, 0x800 + 1032 + 5768 + 512 + 100);
    assert_eq!(result.map(|s| s.disks), Ok(2));
    assert!(out.volumes.contains("\tdisks=-\t"));
}

#[test]
fn one_disk_refused() {
    let mut fw = r740();
    fw.answers.retain(|a| !(a.0 == opcode::PD_GET_INFO && a.1.is_some_and(|m| m[0] == 1)));
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.disks), Ok(2));
    let disks: Vec<&str> = out.disks.lines().collect();
    assert_eq!(
        disks[1],
        "id=1\tenclosure=32\tindex=0\tslot=1\tstate=-\tinterface=-\tmedia=-\tvendor=-\tmodel=-\t\
         serial=-\trevision=-\tsize=-\traw-size=-\tmedia-errors=-\tother-errors=-\tpredictive=-\t\
         smart-alert=-\ttemperature=-\tvolume=0\tspare=-\tos=-\ttask=-\tpercent=-\telapsed=-"
    );
    assert!(disks[0].contains("state=online"));
}

#[test]
fn one_volume_refused() {
    let mut fw = r740();
    fw.refuse(opcode::LD_GET_INFO);
    let (result, out) = run(&mut fw);
    assert!(result.is_ok());
    assert_eq!(
        out.volumes,
        "target=0\tname=-\tlevel=-\tsize=599552000000\tstripe=-\tstate=optimal\twrite=-\t\
         default-write=-\tread=-\tdisk-cache=-\tdisks=0,1\tos=sda\ttask=-\tpercent=-\telapsed=-\n"
    );
}

#[test]
fn a_required_command_refused_fails() {
    for op in [opcode::CTRL_GET_INFO, opcode::LD_GET_LIST, opcode::PD_LIST_QUERY] {
        let mut fw = r740();
        fw.refuse(op);
        let (result, out) = run(&mut fw);
        assert_eq!(result, Err(Failed));
        assert_eq!(out.controller, "");
        assert_eq!(out.volumes, "");
        assert_eq!(out.disks, "");
    }
    // An answer that cannot be read is a failure too.
    let mut fw = r740();
    fw.refuse(opcode::LD_GET_LIST);
    fw.any(opcode::LD_GET_LIST, vec![0xff; 8]);
    assert_eq!(run(&mut fw).0, Err(Failed));
    let mut fw = r740();
    fw.refuse(opcode::CTRL_GET_INFO);
    fw.any(opcode::CTRL_GET_INFO, vec![0; 100]);
    assert_eq!(run(&mut fw).0, Err(Failed));
}

#[test]
fn control_bytes_in_a_name_are_dropped() {
    let mut fw = r740();
    fw.refuse(opcode::LD_GET_INFO);
    fw.volume(&Ld { name: b"a\tb\nc\x01d\xffe", ..Ld::default() });
    fw.refuse(opcode::PD_GET_INFO);
    fw.disk(&Pd { id: 0, model: b"ST\t6\r\n00", vendor: b"SEA\x00GATE", ..Pd::default() });
    let (_, out) = run(&mut fw);
    // The firmware's text stops at a NUL, and drops the rest of the bytes.
    assert!(out.volumes.starts_with("target=0\tname=abcde\t"));
    assert_eq!(out.volumes.matches('\n').count(), 1);
    let first = out.disks.lines().next().unwrap();
    assert!(first.contains("\tvendor=SEA\tmodel=ST600\t"), "{first}");
    assert_eq!(out.disks.matches('\n').count(), 2);
}

#[test]
fn a_host_name_with_control_bytes_is_cleaned() {
    let mut fw = r740();
    let mut out = Out::default();
    let mut scratch = vec![0u8; SCRATCH];
    let mut fetch = |op: u32, m: &[u8; 12], buf: &mut [u8]| fw.run(op, m, buf);
    let os = |_: Target, w: &mut dyn core::fmt::Write| -> Result<bool, core::fmt::Error> {
        w.write_str("s\td\na")?;
        Ok(true)
    };
    gather(
        &mut fetch,
        &mut scratch,
        &PLACE,
        &os,
        &mut Files {
            controller: &mut out.controller,
            volumes: &mut out.volumes,
            disks: &mut out.disks,
            battery: &mut out.battery,
        },
    )
    .unwrap();
    assert!(out.volumes.contains("\tos=sda\t"));
}

#[test]
fn disks_and_volumes_come_in_id_order_and_enclosures_are_skipped() {
    let mut fw = Fw::new(
        ctrl_info(false),
        ld_list(&[(3, 3, 8), (1, 3, 8)]),
        pd_list(&[(9, 32, 0, 3, 0), (32, 32, 0, 24, 13), (2, 32, 0, 1, 0)]),
    );
    fw.volume(&Ld { target: 1, ..Ld::default() });
    fw.volume(&Ld { target: 3, ..Ld::default() });
    fw.disk(&Pd { id: 2, ..Pd::default() });
    fw.disk(&Pd { id: 9, ..Pd::default() });
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.disks), Ok(2));
    let ids: Vec<&str> = out.disks.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(ids, ["id=2", "id=9"]);
    let targets: Vec<&str> = out.volumes.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(targets, ["target=1", "target=3"]);
    assert!(!fw.asked.is_empty());
}

#[test]
fn a_global_spare_and_a_copyback() {
    let mut fw =
        Fw::new(ctrl_info(false), ld_list(&[]), pd_list(&[(7, 32, 0, 7, 0), (8, 32, 0, 8, 0)]));
    fw.disk(&Pd { id: 7, state: 0x02, ..Pd::default() });
    fw.disk(&Pd { id: 8, state: 0x20, ..Pd::default() });
    fw.any(opcode::CFG_READ, config(&[], &[], &[(7, false)]));
    let (result, out) = run(&mut fw);
    assert_eq!(result.map(|s| s.task), Ok(true));
    let disks: Vec<&str> = out.disks.lines().collect();
    assert!(disks[0].contains("\tspare=global\t"));
    assert!(disks[1].contains("	state=copyback	"));
    assert!(disks[1].ends_with("\ttask=copyback\tpercent=-\telapsed=-"));
}

// --- events -----------------------------------------------------------

/// An event answer: sequence, time, code, locale, class, argument type,
/// arguments and description.
fn event_answer(
    seq: u32,
    time: u32,
    code: u32,
    locale: u16,
    class: i8,
    (arg_type, args): (u8, &[u8]),
    text: &[u8],
) -> Vec<u8> {
    let mut b = vec![0; 256];
    p32(&mut b, 0, seq);
    p32(&mut b, 4, time);
    p32(&mut b, 8, code);
    p16(&mut b, 12, locale);
    p8(&mut b, 15, class.to_le_bytes()[0]);
    p8(&mut b, 16, arg_type);
    b[32..32 + args.len()].copy_from_slice(args);
    b[128..128 + text.len()].copy_from_slice(text);
    b
}

fn line_of(detail: &[u8]) -> String {
    let mut s = String::new();
    write_event(detail, &mut s).unwrap();
    s
}

#[test]
fn an_event_that_names_a_disk() {
    // PD 4 in enclosure index 0, slot 4: removed (online to unconfigured).
    let args = [4, 0, 0, 4, 0, 0, 0, 0, 0x18, 0, 0, 0, 0x00, 0, 0, 0];
    let detail = event_answer(
        1234,
        800_000_000,
        event::PD_REMOVED,
        locale::PD,
        0,
        (15, &args),
        b"Removed: PD 04(e0x20/s4)",
    );
    assert_eq!(
        line_of(&detail),
        "seq=1234\ttime=800000000\tclass=info\tlocale=disk\tcode=0x0070\tvolume=-\tdisk=4\t\
         enclosure=0\tslot=4\tpercent=-\ttext=Removed: PD 04(e0x20/s4)\n"
    );
}

#[test]
fn an_event_that_carries_a_volume_progress() {
    // VD 2 at 0x8000 of 0xffff, after 61 seconds.
    let args = [2, 0, 0, 0, 0, 0x80, 61, 0];
    let detail = event_answer(
        77,
        100,
        0x0007,
        locale::LD | locale::CONTROLLER,
        -1,
        (7, &args),
        b"Rebuild progress on VD 02 is 50%=ok",
    );
    assert_eq!(
        line_of(&detail),
        "seq=77\ttime=100\tclass=progress\tlocale=volume,controller\tcode=0x0007\tvolume=2\t\
         disk=-\tenclosure=-\tslot=-\tpercent=50\ttext=Rebuild progress on VD 02 is 50%=ok\n"
    );
}

#[test]
fn an_event_stamped_from_the_start_of_the_controller() {
    let detail = event_answer(5, 0xff00_0e10, 0x0152, 0, 4, (0, &[]), b"Host bus scan requested");
    assert_eq!(
        line_of(&detail),
        "seq=5\ttime=boot+3600\tclass=dead\tlocale=-\tcode=0x0152\tvolume=-\tdisk=-\t\
         enclosure=-\tslot=-\tpercent=-\ttext=Host bus scan requested\n"
    );
}

#[test]
fn an_empty_answer_writes_no_event() {
    assert_eq!(line_of(&[0; 256]), "");
    assert_eq!(line_of(&[0; 100]), "");
    // A number alone is an event, and a description alone is one too.
    assert!(line_of(&event_answer(1, 0, 0, 0, 0, (0, &[]), b"")).starts_with("seq=1\t"));
    assert!(line_of(&event_answer(0, 0, 0, 0, 0, (0, &[]), b"x")).starts_with("seq=0\t"));
}
