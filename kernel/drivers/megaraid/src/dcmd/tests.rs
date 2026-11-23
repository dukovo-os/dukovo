// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the DCMD builders and parsers, on buffers laid out by hand.

use super::*;

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

fn ptext(b: &mut [u8], off: usize, len: usize, s: &str) {
    b[off..off + len].fill(b' ');
    b[off..off + s.len()].copy_from_slice(s.as_bytes());
}

fn shown(t: Text<'_>) -> String {
    format!("{t}")
}

/// A pseudo-random byte stream, for garbage buffers.
fn noise(len: usize, mut seed: u32) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        })
        .collect()
}

/// Buffers of the sizes a parser might be handed: every length up to
/// `max`, filled with zeros, ones and noise.
fn garbage(max: usize) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for len in 0..=max + 8 {
        v.push(vec![0; len]);
        v.push(vec![0xff; len]);
        v.push(noise(len, len as u32 + 1));
    }
    v
}

// --- layout -----------------------------------------------------------

#[test]
fn sizes_match_the_c_structs() {
    assert_eq!(CtrlInfo::LEN, 2048);
    assert_eq!(CtrlInfo::MIN_LEN, 0x6a0);
    assert_eq!(LdList::LEN, 1032);
    assert_eq!(LdConfig::LEN, 256);
    assert_eq!(LdInfo::LEN, 384);
    assert_eq!(PdList::LEN, 5768);
    assert_eq!(PdInfo::LEN, 512);
    assert_eq!(EventLogState::LEN, 20);
    assert_eq!(EventDetail::LEN, 256);
    assert_eq!(Array::LEN, 288);
    assert_eq!(Spare::LEN, 40);
    assert_eq!(Config::HEADER, 32);
    assert_eq!(BbuStatus::LEN, 64);
    assert_eq!(BbuCapacity::LEN, 48);
    assert_eq!(BbuDesign::LEN, 67);
    assert_eq!(CtrlInfo::COMPONENT_LEN, 72);
    // The array's drives end where the C struct does.
    assert_eq!(Array::HEADER + Array::MAX_MEMBERS * 8, Array::LEN);
    assert_eq!(Spare::HEADER + Spare::MAX_ARRAYS * 2, Spare::LEN);
    // The logical drive's spans end where its C struct does.
    assert_eq!(64 + LdConfig::MAX_SPANS * 24, LdConfig::LEN);
    // The logical drive info is its config, then its own fields.
    assert_eq!(LdConfig::LEN + 8 + 36 + 4 + 64 + 16, LdInfo::LEN);
    // The controller info's fixed part: three interface blocks, the
    // components, the pending ones, the limits, the strings.
    assert_eq!(32 + 72 + 72 + 8 + 8 * 72 + 4 + 8 * 72 + 4, 1344);
    assert_eq!(1344 + 80 + 32, 1456);
    assert_eq!(1536 + 64, 1600);
    assert_eq!(1600 + 96, CtrlInfo::MIN_LEN);
}

#[test]
fn opcodes() {
    assert_eq!(opcode::CTRL_GET_INFO, 0x0101_0000);
    assert_eq!(opcode::CTRL_GET_PROPERTIES, 0x0102_0100);
    assert_eq!(opcode::CTRL_EVENT_GET_INFO, 0x0104_0100);
    assert_eq!(opcode::CTRL_EVENT_GET, 0x0104_0300);
    assert_eq!(opcode::CTRL_EVENT_WAIT, 0x0104_0500);
    assert_eq!(opcode::CTRL_SHUTDOWN, 0x0105_0000);
    assert_eq!(opcode::CTRL_CACHE_FLUSH, 0x0110_1000);
    assert_eq!(opcode::PD_GET_LIST, 0x0201_0000);
    assert_eq!(opcode::PD_LIST_QUERY, 0x0201_0100);
    assert_eq!(opcode::PD_GET_INFO, 0x0202_0000);
    assert_eq!(opcode::LD_GET_LIST, 0x0301_0000);
    assert_eq!(opcode::LD_GET_INFO, 0x0302_0000);
    assert_eq!(opcode::CFG_READ, 0x0401_0000);
    assert_eq!(opcode::BBU_GET_STATUS, 0x0501_0000);
    assert_eq!(opcode::BBU_GET_CAPACITY_INFO, 0x0502_0000);
    assert_eq!(opcode::BBU_GET_DESIGN_INFO, 0x0503_0000);
}

// --- mailboxes --------------------------------------------------------

#[test]
fn mailboxes() {
    assert_eq!(mbox::ld_info(0x0102), [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(mbox::pd_info(0xabcd), [0xcd, 0xab, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(mbox::pd_list_query_all(), [0; 12]);
    assert_eq!(
        mbox::event(0x0403_0201, locale::ALL, Class::Info.as_i8()),
        [1, 2, 3, 4, 0xff, 0xff, 0, 0, 0, 0, 0, 0]
    );
    // The class is the top byte of the word that follows the sequence.
    assert_eq!(mbox::event(7, locale::LD | locale::PD, -2)[4..8], [3, 0, 0, 0xfe]);
    assert_eq!(mbox::cache_flush()[0], 3);
    assert_eq!(mbox::shutdown(false), [0; 12]);
    assert_eq!(mbox::shutdown(true)[0], 1);
}

// --- text -------------------------------------------------------------

#[test]
fn text_trims_and_filters() {
    assert_eq!(shown(Text::new(b"  PERC H730  ")), "PERC H730");
    assert_eq!(shown(Text::new(b"\0\0ab\0\0")), "ab");
    assert_eq!(shown(Text::new(b"ab\0cd")), "ab");
    assert_eq!(shown(Text::new(b"a\x01b\xffc")), "abc");
    assert_eq!(shown(Text::new(b"  \0 x y \0 ")), "x y");
    assert!(Text::new(b"").is_empty());
    assert!(Text::new(b"   \0  ").is_empty());
    assert!(Text::new(&[1, 2, 3]).is_empty());
    assert!(!Text::new(b" a").is_empty());
    assert!(Text::new(b"  ab  ").is(" ab".trim()));
    assert!(!Text::new(b"ab").is("abc"));
    assert_eq!(Text::new(b"  ab  ").as_bytes(), b"ab");
    assert_eq!(format!("{:?}", Text::new(b"x")), "Text(\"x\")");
}

#[test]
fn parse_error_words() {
    assert!(ParseError::Short.to_string().contains("shorter"));
    assert!(ParseError::BadCount.to_string().contains("more entries"));
    assert!(ParseError::BadSize.to_string().contains("size"));
}

// --- controller info --------------------------------------------------

fn ctrl_info() -> Vec<u8> {
    let mut b = vec![0; CtrlInfo::LEN];
    p16(&mut b, 0, 0x1000);
    p16(&mut b, 2, 0x005d);
    p16(&mut b, 4, 0x1028);
    p16(&mut b, 6, 0x1f49);
    p32(&mut b, 180, 3);
    for (i, (name, ver)) in
        [("BOOT", "1.0"), ("APP", "4.300.00-8366"), ("PSOC", "7")].iter().enumerate()
    {
        let at = 184 + i * 72;
        ptext(&mut b, at, 8, name);
        ptext(&mut b, at + 8, 32, ver);
        ptext(&mut b, at + 40, 16, "Jan 01 2020");
        ptext(&mut b, at + 56, 16, "12:00:00");
    }
    p8(&mut b, 1340, 32);
    p8(&mut b, 1341, 8);
    p8(&mut b, 1342, 128);
    p8(&mut b, 1343, 64);
    ptext(&mut b, 1344, 80, "PERC H730 Mini");
    ptext(&mut b, 1424, 32, "SN12345");
    p32(&mut b, 1456, 0b0101);
    p32(&mut b, 1460, 1_700_000_000);
    p16(&mut b, 1464, 1008);
    p16(&mut b, 1466, 64);
    p32(&mut b, 1468, 0x1000);
    p16(&mut b, 1472, 4);
    p16(&mut b, 1474, 1);
    p16(&mut b, 1476, 2);
    p16(&mut b, 1478, 10);
    p16(&mut b, 1480, 9);
    p16(&mut b, 1482, 3);
    p16(&mut b, 1484, 5);
    p16(&mut b, 1486, 32);
    p16(&mut b, 1488, 1024);
    p16(&mut b, 1490, 16);
    p16(&mut b, 1492, 6);
    p16(&mut b, 1494, 7);
    p16(&mut b, 1498, 129);
    p32(&mut b, 1500, 0b10111);
    p32(&mut b, 1504, 1 << 7 | 1 << 14 | 1 << 8);
    p8(&mut b, 1512, 3);
    p8(&mut b, 1513, 11);
    p32(&mut b, 1536 + 32, 1 << 13);
    ptext(&mut b, 1600, 96, "24.5.0-0020");
    p16(&mut b, 0x7a2, 2048);
    p32(&mut b, 0x7a4, 1 << 11);
    p8(&mut b, 0x7c9, 61);
    p8(&mut b, 0x7ca, 38);
    b
}

#[test]
fn ctrl_info_fields() {
    let b = ctrl_info();
    let c = CtrlInfo::parse(&b).unwrap();
    assert_eq!(c.pci_ids(), [0x1000, 0x005d, 0x1028, 0x1f49]);
    assert_eq!(c.components().count(), 3);
    let first = c.components().next().unwrap();
    assert!(first.name().is("BOOT"));
    assert!(first.build_date().is("Jan 01 2020"));
    assert!(first.build_time().is("12:00:00"));
    assert_eq!(shown(c.firmware_version().unwrap()), "4.300.00-8366");
    assert_eq!((c.max_arms(), c.max_spans(), c.max_arrays(), c.max_lds()), (32, 8, 128, 64));
    assert_eq!(shown(c.product_name()), "PERC H730 Mini");
    assert_eq!(shown(c.serial_number()), "SN12345");
    let hw = c.hw_present();
    assert!(hw.bbu() && !hw.alarm() && hw.nvram() && !hw.uart());
    assert_eq!(hw.raw(), 5);
    assert_eq!(c.firmware_time(), 1_700_000_000);
    assert_eq!(c.max_commands(), 1008);
    assert_eq!(c.max_sge_count(), 64);
    assert_eq!(c.max_request_size(), 0x1000);
    assert_eq!((c.lds_present(), c.lds_degraded(), c.lds_offline()), (4, 1, 2));
    assert_eq!((c.pds_present(), c.pd_disks_present()), (10, 9));
    assert_eq!((c.pd_disks_critical(), c.pd_disks_failed()), (3, 5));
    assert_eq!((c.nvram_kb(), c.memory_mb(), c.flash_kb()), (32, 1024, 16));
    assert_eq!((c.memory_correctable_errors(), c.memory_uncorrectable_errors()), (6, 7));
    assert_eq!(c.max_strips_per_io(), 129);
    let r = c.raid_levels();
    assert!(r.raid0() && r.raid1() && r.raid5() && !r.raid1e() && r.raid6());
    let o = c.adapter_ops();
    assert!(o.bbu() && o.global_spares() && o.spanning_allowed());
    assert!(!o.rebuild_rate() && !o.foreign_import());
    assert_eq!(c.stripe_size_range(), (3, 11));
    assert!(c.jbod_enabled());
    assert_eq!(shown(c.package_version()), "24.5.0-0020");
    assert_eq!(c.cache_memory_mb(), Some(2048));
    assert_eq!(c.jbod_supported(), Some(true));
    assert_eq!(c.roc_temperature(), Some(61));
    assert_eq!(c.controller_temperature(), Some(38));
}

#[test]
fn ctrl_info_short_forms() {
    let b = ctrl_info();
    let c = CtrlInfo::parse(&b[..CtrlInfo::MIN_LEN]).unwrap();
    assert_eq!(shown(c.package_version()), "24.5.0-0020");
    assert_eq!(c.cache_memory_mb(), None);
    assert_eq!(c.jbod_supported(), None);
    assert_eq!(c.roc_temperature(), None);
    assert_eq!(c.controller_temperature(), None);
    assert_eq!(CtrlInfo::parse(&b[..CtrlInfo::MIN_LEN - 1]).unwrap_err(), ParseError::Short);
    // A sensor that reads zero is not reporting.
    let mut z = ctrl_info();
    p8(&mut z, 0x7c9, 0);
    p16(&mut z, 0x7a2, 0);
    let c = CtrlInfo::parse(&z).unwrap();
    assert_eq!(c.roc_temperature(), None);
    assert_eq!(c.cache_memory_mb(), None);
}

#[test]
fn ctrl_info_component_count_is_bounded() {
    let mut b = ctrl_info();
    p32(&mut b, 180, u32::MAX);
    let c = CtrlInfo::parse(&b).unwrap();
    assert_eq!(c.components().count(), CtrlInfo::MAX_COMPONENTS);
    // Without an APP component, the first version is the firmware's.
    let mut n = ctrl_info();
    ptext(&mut n, 184 + 72, 8, "FOO");
    let c = CtrlInfo::parse(&n).unwrap();
    assert_eq!(shown(c.firmware_version().unwrap()), "1.0");
    p32(&mut n, 180, 0);
    assert!(CtrlInfo::parse(&n).unwrap().firmware_version().is_none());
}

// --- logical drive list ----------------------------------------------

fn ld_list(entries: &[(u8, u16, u8, u64)]) -> Vec<u8> {
    let mut b = vec![0; LdList::LEN];
    p32(&mut b, 0, entries.len() as u32);
    for (i, &(t, seq, st, size)) in entries.iter().enumerate() {
        let at = 8 + i * 16;
        p8(&mut b, at, t);
        p16(&mut b, at + 2, seq);
        p8(&mut b, at + 4, st);
        p64(&mut b, at + 8, size);
    }
    b
}

#[test]
fn ld_list_entries() {
    let b = ld_list(&[(0, 5, 3, 1 << 30), (7, 9, 2, 123), (9, 0, 77, 0)]);
    let l = LdList::parse(&b).unwrap();
    assert_eq!(l.len(), 3);
    assert!(!l.is_empty());
    let v: Vec<_> = l.iter().collect();
    assert_eq!(
        v[0],
        LdListEntry { target_id: 0, sequence: 5, state: LdState::Optimal, size_blocks: 1 << 30 }
    );
    assert_eq!(v[1].state, LdState::Degraded);
    assert_eq!(v[2].state, LdState::Other(77));
    assert_eq!(l.get(1), Some(v[1]));
    assert_eq!(l.get(3), None);
    assert_eq!(LdState::Optimal.name(), "optimal");
}

#[test]
fn ld_list_counts_are_checked() {
    let empty = ld_list(&[]);
    assert!(LdList::parse(&empty).unwrap().is_empty());
    assert_eq!(LdList::parse(&empty[..7]).unwrap_err(), ParseError::Short);
    assert_eq!(LdList::parse(&[]).unwrap_err(), ParseError::Short);
    let mut b = ld_list(&[(1, 1, 3, 1), (2, 1, 3, 1)]);
    // Two entries fit in 8 + 32 bytes, three do not.
    assert!(LdList::parse(&b[..40]).is_ok());
    assert_eq!(LdList::parse(&b[..39]).unwrap_err(), ParseError::BadCount);
    p32(&mut b, 0, 65);
    assert_eq!(LdList::parse(&b).unwrap_err(), ParseError::BadCount);
    p32(&mut b, 0, u32::MAX);
    assert_eq!(LdList::parse(&b).unwrap_err(), ParseError::BadCount);
}

// --- logical drive info ----------------------------------------------

fn ld_info() -> Vec<u8> {
    let mut b = vec![0; LdInfo::LEN];
    p8(&mut b, 0, 2);
    p16(&mut b, 2, 77);
    ptext(&mut b, 4, 16, "data");
    p8(&mut b, 20, 0x01 | 0x04 | 0x20 | 0x40);
    p8(&mut b, 21, 2);
    p8(&mut b, 22, 2);
    p8(&mut b, 23, 0x01 | 0x02 | 0x08 | 0x10);
    p8(&mut b, 32, 5);
    p8(&mut b, 33, 3);
    p8(&mut b, 34, 1);
    p8(&mut b, 35, 7);
    p8(&mut b, 36, 4);
    p8(&mut b, 37, 1);
    p8(&mut b, 38, 3);
    p8(&mut b, 39, 2);
    p8(&mut b, 40, 1);
    p64(&mut b, 64, 0);
    p64(&mut b, 72, 1000);
    p16(&mut b, 80, 3);
    p64(&mut b, 256, 3000);
    p32(&mut b, 264, 0b1011);
    p16(&mut b, 268, 0x8000);
    p16(&mut b, 270, 12);
    p16(&mut b, 272, 0xffff);
    p16(&mut b, 274, 99);
    p16(&mut b, 276, 5);
    p16(&mut b, 278, 6);
    p16(&mut b, 280, 0);
    p16(&mut b, 282, 1);
    p8(&mut b, 302, 1);
    b
}

#[test]
fn ld_info_fields() {
    let b = ld_info();
    let i = LdInfo::parse(&b).unwrap();
    let c = i.config();
    assert_eq!((c.target_id(), c.sequence()), (2, 77));
    assert_eq!(shown(c.name()), "data");
    let d = c.default_cache_policy();
    assert!(d.write_back() && d.read_ahead() && d.allow_write_cache() && d.allow_read_cache());
    assert!(!d.write_adaptive() && !d.read_adaptive() && !d.write_cache_bad_bbu());
    assert_eq!(c.access(), LdAccess::ReadOnly);
    assert_eq!(c.disk_cache(), DiskCache::Disabled);
    let n = c.current_cache_policy();
    assert!(n.write_back() && n.write_adaptive() && n.read_adaptive() && n.write_cache_bad_bbu());
    assert_eq!(n.raw(), 0x1b);
    assert_eq!((c.primary_level(), c.level_qualifier(), c.secondary_level()), (5, 3, 1));
    assert_eq!(c.stripe_shift(), 7);
    assert_eq!(c.stripe_bytes(), Some(64 * 1024));
    assert_eq!((c.num_drives(), c.span_depth()), (4, 1));
    assert_eq!(c.state(), LdState::Optimal);
    assert_eq!(c.init_state(), 2);
    assert!(c.is_consistent());
    assert_eq!(c.raid_level(), RaidLevel::Raid5);
    assert_eq!(c.raid_level().name(), "RAID 5");
    assert_eq!(i.size_blocks(), 3000);
    assert_eq!(
        c.spans().collect::<Vec<_>>(),
        [Span { start_block: 0, num_blocks: 1000, array_ref: 3 }]
    );
    assert_eq!(c.span(1), None);
    let cc = i.consistency_check().unwrap();
    assert_eq!((cc.fraction, cc.elapsed_secs, cc.percent()), (0x8000, 12, 50));
    let bgi = i.background_init().unwrap();
    assert_eq!((bgi.percent(), bgi.elapsed_secs), (100, 99));
    assert_eq!(i.foreground_init(), None);
    let rc = i.reconstruction().unwrap();
    assert_eq!((rc.percent(), rc.elapsed_secs), (0, 1));
    assert!(i.reconstruct_active());
}

#[test]
fn ld_info_short_and_odd() {
    let b = ld_info();
    assert_eq!(LdInfo::parse(&b[..383]).unwrap_err(), ParseError::Short);
    let mut o = ld_info();
    p8(&mut o, 35, 200);
    p8(&mut o, 21, 1);
    p8(&mut o, 22, 9);
    let c = LdInfo::parse(&o).unwrap().config();
    assert_eq!(c.stripe_bytes(), None);
    assert_eq!(c.access(), LdAccess::Other(1));
    assert_eq!(c.disk_cache(), DiskCache::Other(9));
    assert_eq!(c.access(), LdAccess::Other(1));
    p8(&mut o, 21, 0);
    p8(&mut o, 21, 3);
    assert_eq!(LdInfo::parse(&o).unwrap().config().access(), LdAccess::Blocked);
    // More spans than the structure holds read no further than 8.
    p8(&mut o, 37, 255);
    assert_eq!(LdInfo::parse(&o).unwrap().config().spans().count(), 8);
}

#[test]
fn raid_levels_by_level_and_spans() {
    let level = |primary: u8, qual: u8, spans: u8| {
        let mut b = ld_info();
        p8(&mut b, 32, primary);
        p8(&mut b, 33, qual);
        p8(&mut b, 37, spans);
        LdInfo::parse(&b).unwrap().config().raid_level()
    };
    assert_eq!(level(0, 0, 1), RaidLevel::Raid0);
    assert_eq!(level(1, 0, 1), RaidLevel::Raid1);
    assert_eq!(level(5, 0, 1), RaidLevel::Raid5);
    assert_eq!(level(6, 0, 1), RaidLevel::Raid6);
    assert_eq!(level(1, 3, 1), RaidLevel::Raid1E);
    assert_eq!(level(0, 0, 2), RaidLevel::Raid00);
    assert_eq!(level(1, 0, 2), RaidLevel::Raid10);
    assert_eq!(level(5, 0, 3), RaidLevel::Raid50);
    assert_eq!(level(6, 0, 2), RaidLevel::Raid60);
    assert_eq!(level(0, 0, 0), RaidLevel::Raid0);
    assert_eq!(level(9, 0, 1), RaidLevel::Unknown);
    assert_eq!(level(9, 0, 2), RaidLevel::Unknown);
    assert_eq!(RaidLevel::Raid1E.name(), "RAID 1E");
    assert_eq!(RaidLevel::Raid00.name(), "RAID 00");
    assert_eq!(RaidLevel::Raid10.name(), "RAID 10");
    assert_eq!(RaidLevel::Raid50.name(), "RAID 50");
    assert_eq!(RaidLevel::Raid60.name(), "RAID 60");
}

// --- physical disk list ----------------------------------------------

fn pd_list(n: usize) -> Vec<u8> {
    let mut b = vec![0; 8 + n * 24];
    p32(&mut b, 0, (8 + n * 24) as u32);
    p32(&mut b, 4, n as u32);
    for i in 0..n {
        let at = 8 + i * 24;
        p16(&mut b, at, 10 + i as u16);
        p16(&mut b, at + 2, 252);
        p8(&mut b, at + 4, 1);
        p8(&mut b, at + 5, i as u8);
        p8(&mut b, at + 6, 0);
        p8(&mut b, at + 7, 3);
        p64(&mut b, at + 8, 0x5000_0000_0000_0000 + i as u64);
        p64(&mut b, at + 16, 0);
    }
    b
}

#[test]
fn pd_list_entries() {
    let b = pd_list(3);
    let l = PdList::parse(&b).unwrap();
    assert_eq!((l.len(), l.size()), (3, 80));
    let v: Vec<_> = l.iter().collect();
    assert_eq!(v.len(), 3);
    assert_eq!(
        v[2],
        PdAddress {
            device_id: 12,
            enclosure_device_id: 252,
            enclosure_index: 1,
            slot: 2,
            scsi_device_type: 0,
            connected_ports: 3,
            sas_address: [0x5000_0000_0000_0002, 0],
        }
    );
    assert_eq!(l.get(0), Some(v[0]));
    assert_eq!(l.get(3), None);
    assert!(PdList::parse(&pd_list(0)).unwrap().is_empty());
}

#[test]
fn pd_list_counts_are_checked() {
    let b = pd_list(2);
    assert_eq!(PdList::parse(&b[..7]).unwrap_err(), ParseError::Short);
    assert_eq!(PdList::parse(&b[..b.len() - 1]).unwrap_err(), ParseError::BadCount);
    let mut h = pd_list(2);
    p32(&mut h, 4, u32::MAX);
    assert_eq!(PdList::parse(&h).unwrap_err(), ParseError::BadCount);
}

// --- physical disk info ----------------------------------------------

fn pd_info() -> Vec<u8> {
    let mut b = vec![0; PdInfo::LEN];
    p16(&mut b, 0, 14);
    p16(&mut b, 2, 3);
    p8(&mut b, 4, 0);
    ptext(&mut b, 4 + 8, 8, "SEAGATE");
    ptext(&mut b, 4 + 16, 16, "ST900MM0006");
    ptext(&mut b, 4 + 32, 4, "0003");
    p8(&mut b, 100 + 1, 0x80);
    p8(&mut b, 100 + 3, 8);
    b[104..112].copy_from_slice(b"S0N1A2B3");
    p8(&mut b, 165, 0);
    p8(&mut b, 166, 1);
    p8(&mut b, 167, 4);
    p32(&mut b, 168, 11);
    p32(&mut b, 172, 12);
    p32(&mut b, 176, 2);
    p32(&mut b, 180, 4444);
    p16(&mut b, 184, 0x18);
    p8(&mut b, 186, 1);
    p8(&mut b, 187, 3);
    p32(&mut b, 188, (2 << 12) | (1 << 1) | (1 << 4));
    p8(&mut b, 192, 2);
    p64(&mut b, 200, 0x5000_c500_0000_0001);
    p64(&mut b, 208, 0x5000_c500_0000_0002);
    p64(&mut b, 232, 1_758_000_000);
    p64(&mut b, 240, 1_757_000_000);
    p64(&mut b, 248, 1_756_000_000);
    p16(&mut b, 256, 252);
    p8(&mut b, 258, 1);
    p8(&mut b, 259, 6);
    p32(&mut b, 260, 0b101);
    p16(&mut b, 264, 0x4000);
    p16(&mut b, 266, 30);
    p16(&mut b, 272, 0xffff);
    p16(&mut b, 274, 600);
    p8(&mut b, 370, 2);
    p8(&mut b, 401, 3);
    p8(&mut b, 402, 41);
    b
}

#[test]
fn pd_info_fields() {
    let b = pd_info();
    let p = PdInfo::parse(&b).unwrap();
    assert_eq!((p.device_id(), p.sequence()), (14, 3));
    let q = p.inquiry();
    assert_eq!(q.device_type(), 0);
    assert_eq!(shown(q.vendor()), "SEAGATE");
    assert_eq!(shown(q.product()), "ST900MM0006");
    assert_eq!(shown(q.revision()), "0003");
    assert_eq!(q.raw().len(), 96);
    assert_eq!(shown(p.serial().unwrap()), "S0N1A2B3");
    assert_eq!((p.scsi_device_type(), p.connected_ports(), p.device_speed()), (0, 1, 4));
    assert_eq!((p.media_errors(), p.other_errors()), (11, 12));
    assert_eq!((p.predictive_failures(), p.last_predictive_failure_seq()), (2, 4444));
    assert!(p.smart_alert());
    assert_eq!(p.state(), PdState::Online);
    assert!(p.disabled_for_removal());
    assert_eq!(p.link_speed(), 3);
    assert_eq!(p.interface(), PdInterface::Sas);
    assert!(p.in_vd() && p.is_foreign() && !p.is_spare() && !p.is_global_spare());
    assert_eq!(p.path_count(), 2);
    assert_eq!(p.sas_address(0), Some(0x5000_c500_0000_0001));
    assert_eq!(p.sas_address(1), Some(0x5000_c500_0000_0002));
    assert_eq!(p.sas_address(2), None);
    assert_eq!(
        (p.raw_size(), p.non_coerced_size(), p.coerced_size()),
        (1_758_000_000, 1_757_000_000, 1_756_000_000)
    );
    assert_eq!((p.enclosure_device_id(), p.enclosure_index(), p.slot()), (252, 1, 6));
    let r = p.rebuild().unwrap();
    assert_eq!((r.percent(), r.elapsed_secs), (25, 30));
    assert_eq!(p.patrol(), None);
    let c = p.clear().unwrap();
    assert_eq!((c.percent(), c.elapsed_secs), (100, 600));
    assert_eq!(p.media_type(), Some(MediaType::Ssd));
    assert_eq!(p.interface_ext(), Some(PdInterface::Sata));
    assert_eq!(p.temperature(), Some(41));
}

#[test]
fn pd_info_short_forms() {
    let b = pd_info();
    let p = PdInfo::parse(&b[..PdInfo::MIN_LEN]).unwrap();
    assert_eq!(p.media_type(), None);
    assert_eq!(p.interface_ext(), None);
    assert_eq!(p.temperature(), None);
    assert_eq!(PdInfo::parse(&b[..PdInfo::MIN_LEN - 1]).unwrap_err(), ParseError::Short);
    let mut o = pd_info();
    p8(&mut o, 402, 0xff);
    assert_eq!(PdInfo::parse(&o).unwrap().temperature(), None);
    p8(&mut o, 402, 0);
    assert_eq!(PdInfo::parse(&o).unwrap().temperature(), None);
    p8(&mut o, 192, 200);
    assert_eq!(PdInfo::parse(&o).unwrap().sas_address(4), None);
    assert!(PdInfo::parse(&o).unwrap().sas_address(3).is_some());
    p16(&mut o, 184, 0x7777);
    assert_eq!(PdInfo::parse(&o).unwrap().state(), PdState::Other(0x7777));
    p8(&mut o, 370, 9);
    assert_eq!(PdInfo::parse(&o).unwrap().media_type(), Some(MediaType::Other(9)));
}

#[test]
fn pd_states() {
    let table = [
        (0x00, PdState::UnconfiguredGood),
        (0x01, PdState::UnconfiguredBad),
        (0x02, PdState::HotSpare),
        (0x10, PdState::Offline),
        (0x11, PdState::Failed),
        (0x14, PdState::Rebuild),
        (0x18, PdState::Online),
        (0x20, PdState::Copyback),
        (0x40, PdState::System),
        (0x41, PdState::Other(0x41)),
    ];
    for (raw, state) in table {
        assert_eq!(PdState::from_raw(raw), state);
    }
    assert!(PdState::System.is_jbod());
    assert!(!PdState::Online.is_jbod());
    assert_eq!(PdState::Failed.name(), "failed");
    assert_eq!(PdState::Other(5).name(), "unknown");
}

#[test]
fn pd_interfaces() {
    for (raw, want) in [
        (0u8, PdInterface::Unknown),
        (1, PdInterface::ParallelScsi),
        (2, PdInterface::Sas),
        (3, PdInterface::Sata),
        (4, PdInterface::FibreChannel),
        (5, PdInterface::Nvme),
        (6, PdInterface::Other(6)),
    ] {
        let mut b = pd_info();
        p32(&mut b, 188, u32::from(raw) << 12);
        assert_eq!(PdInfo::parse(&b).unwrap().interface(), want);
    }
}

#[test]
fn pd_serial_from_page_83() {
    let mut b = pd_info();
    // A page 0x83 with a binary NAA designator, then an ASCII one.
    b[100..164].fill(0);
    p8(&mut b, 101, 0x83);
    p8(&mut b, 103, 8 + 4 + 6);
    b[104..108].copy_from_slice(&[0x01, 0x03, 0, 4]);
    b[108..112].copy_from_slice(&[0x50, 0, 0, 1]);
    b[112..116].copy_from_slice(&[0x02, 0x01, 0, 6]);
    b[116..122].copy_from_slice(b"ABC123");
    assert_eq!(shown(PdInfo::parse(&b).unwrap().serial().unwrap()), "ABC123");
    // The same page without the ASCII designator has no serial.
    p8(&mut b, 112, 0x01);
    assert!(PdInfo::parse(&b).unwrap().serial().is_none());
    // A designator that claims to run past the page ends the search.
    p8(&mut b, 112, 0x02);
    p8(&mut b, 115, 200);
    assert!(PdInfo::parse(&b).unwrap().serial().is_none());
    // A page 0x80 whose length runs past the field.
    p8(&mut b, 101, 0x80);
    p8(&mut b, 103, 250);
    assert!(PdInfo::parse(&b).unwrap().serial().is_none());
    // An unknown page.
    p8(&mut b, 101, 0x00);
    assert!(PdInfo::parse(&b).unwrap().serial().is_none());
}

// --- events -----------------------------------------------------------

#[test]
fn event_log_state() {
    let mut b = vec![0; 20];
    for (i, v) in [100u32, 90, 80, 70, 60].iter().enumerate() {
        p32(&mut b, i * 4, *v);
    }
    let s = EventLogState::parse(&b).unwrap();
    assert_eq!(s, EventLogState { newest: 100, oldest: 90, clear: 80, shutdown: 70, boot: 60 });
    assert_eq!(EventLogState::parse(&b[..19]).unwrap_err(), ParseError::Short);
}

fn event(arg_type: u8) -> Vec<u8> {
    let mut b = vec![0; EventDetail::LEN];
    p32(&mut b, 0, 1234);
    p32(&mut b, 4, 999);
    p32(&mut b, 8, event::LD_STATE_CHANGE);
    p16(&mut b, 12, locale::LD);
    p8(&mut b, 15, 0xff);
    p8(&mut b, 16, arg_type);
    ptext(&mut b, 128, 128, "State change on VD 02");
    b[255] = 0;
    b
}

#[test]
fn event_header() {
    let b = event(0);
    let e = EventDetail::parse(&b).unwrap();
    assert_eq!((e.sequence(), e.time(), e.code()), (1234, 999, 0x51));
    assert_eq!(e.locale(), 1);
    assert_eq!(e.class(), Class::Progress);
    assert_eq!(e.arg_type(), 0);
    assert_eq!(shown(e.description()), "State change on VD 02");
    assert_eq!(e.args(), EventArgs::None);
    assert_eq!(EventDetail::parse(&b[..255]).unwrap_err(), ParseError::Short);
}

#[test]
fn event_classes() {
    for (n, c) in [
        (-2, Class::Debug),
        (-1, Class::Progress),
        (0, Class::Info),
        (1, Class::Warning),
        (2, Class::Critical),
        (3, Class::Fatal),
        (4, Class::Dead),
        (5, Class::Other(5)),
        (-3, Class::Other(-3)),
    ] {
        assert_eq!(Class::from_i8(n), c);
        assert_eq!(c.as_i8(), n);
    }
}

fn args(arg_type: u8, f: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut b = event(arg_type);
    f(&mut b[32..128]);
    b
}

fn decode(b: &[u8]) -> EventArgs<'_> {
    EventDetail::parse(b).unwrap().args()
}

const LD: LdRef = LdRef { target_id: 3, index: 9 };
const PD: PdRef = PdRef { device_id: 0x0102, enclosure_index: 4, slot: 5 };

fn put_ld(a: &mut [u8], off: usize) {
    p16(a, off, 3);
    p8(a, off + 2, 9);
}

fn put_pd(a: &mut [u8], off: usize) {
    p16(a, off, 0x0102);
    p8(a, off + 2, 4);
    p8(a, off + 3, 5);
}

#[test]
fn event_arguments() {
    let b = args(1, |a| {
        put_pd(a, 0);
        p8(a, 4, 6);
        p8(a, 5, 3);
        a[8..14].copy_from_slice(&[0x28, 0, 0, 0, 0, 1]);
        a[24..27].copy_from_slice(&[0x70, 0, 3]);
    });
    assert_eq!(
        decode(&b),
        EventArgs::CdbSense { pd: PD, cdb: &[0x28, 0, 0, 0, 0, 1], sense: &[0x70, 0, 3] }
    );
    assert_eq!(decode(&args(2, |a| put_ld(a, 0))), EventArgs::Ld(LD));
    let b = args(3, |a| {
        put_ld(a, 0);
        p64(a, 4, 77);
    });
    assert_eq!(decode(&b), EventArgs::LdCount { ld: LD, count: 77 });
    let b = args(4, |a| {
        p64(a, 0, 0x1_0000_0000);
        put_ld(a, 8);
    });
    assert_eq!(decode(&b), EventArgs::LdLba { ld: LD, lba: 0x1_0000_0000 });
    let b = args(5, |a| {
        put_ld(a, 0);
        p32(a, 4, 1);
        p32(a, 8, 2);
    });
    assert_eq!(decode(&b), EventArgs::LdOwner { ld: LD, prev_owner: 1, new_owner: 2 });
    let b = args(6, |a| {
        p64(a, 0, 11);
        p64(a, 8, 22);
        put_ld(a, 16);
        put_pd(a, 20);
    });
    assert_eq!(decode(&b), EventArgs::LdLbaPdLba { ld: LD, pd: PD, ld_lba: 11, pd_lba: 22 });
    let b = args(7, |a| {
        put_ld(a, 0);
        p16(a, 4, 0xffff);
        p16(a, 6, 8);
    });
    assert_eq!(
        decode(&b),
        EventArgs::LdProgress { ld: LD, progress: Progress { fraction: 0xffff, elapsed_secs: 8 } }
    );
    let b = args(8, |a| {
        put_ld(a, 0);
        p32(a, 4, 2);
        p32(a, 8, 3);
    });
    assert_eq!(
        decode(&b),
        EventArgs::LdState { ld: LD, prev: LdState::Degraded, new: LdState::Optimal }
    );
    let b = args(9, |a| {
        p64(a, 0, 5);
        put_ld(a, 8);
    });
    assert_eq!(decode(&b), EventArgs::LdStrip { ld: LD, strip: 5 });
    assert_eq!(decode(&args(10, |a| put_pd(a, 0))), EventArgs::Pd(PD));
    let b = args(11, |a| {
        put_pd(a, 0);
        p32(a, 4, 0xe1);
    });
    assert_eq!(decode(&b), EventArgs::PdError { pd: PD, error: 0xe1 });
    let b = args(12, |a| {
        p64(a, 0, 99);
        put_pd(a, 8);
    });
    assert_eq!(decode(&b), EventArgs::PdLba { pd: PD, lba: 99 });
    let b = args(13, |a| {
        p64(a, 0, 98);
        put_pd(a, 8);
        put_ld(a, 12);
    });
    assert_eq!(decode(&b), EventArgs::PdLbaLd { pd: PD, ld: LD, lba: 98 });
    let b = args(14, |a| {
        put_pd(a, 0);
        p16(a, 4, 0x8000);
        p16(a, 6, 4);
    });
    assert_eq!(
        decode(&b),
        EventArgs::PdProgress { pd: PD, progress: Progress { fraction: 0x8000, elapsed_secs: 4 } }
    );
    let b = args(15, |a| {
        put_pd(a, 0);
        p32(a, 4, 0x18);
        p32(a, 8, 0x11);
    });
    assert_eq!(
        decode(&b),
        EventArgs::PdState { pd: PD, prev: PdState::Online, new: PdState::Failed }
    );
    let b = args(16, |a| {
        p16(a, 0, 0x1000);
        p16(a, 2, 0x5d);
        p16(a, 4, 0x1028);
        p16(a, 6, 0x1f49);
    });
    assert_eq!(
        decode(&b),
        EventArgs::Pci { vendor: 0x1000, device: 0x5d, subvendor: 0x1028, subdevice: 0x1f49 }
    );
    assert_eq!(decode(&args(17, |a| p32(a, 0, 6000))), EventArgs::Rate(6000));
    let b = args(18, |a| a[..5].copy_from_slice(b"hello"));
    match decode(&b) {
        EventArgs::Str(t) => assert!(t.is("hello")),
        other => panic!("{other:?}"),
    }
    let b = args(19, |a| {
        p32(a, 0, 5000);
        p16(a, 4, 7);
    });
    assert_eq!(decode(&b), EventArgs::Time { rtc: 5000, elapsed_secs: 7 });
    let b = args(20, |a| {
        p32(a, 0, 1);
        p32(a, 4, 2);
        a[8..11].copy_from_slice(b"ecc");
    });
    match decode(&b) {
        EventArgs::Ecc { ecar: 1, elog: 2, text } => assert!(text.is("ecc")),
        other => panic!("{other:?}"),
    }
    assert_eq!(decode(&args(33, |_| {})), EventArgs::Other(33));
    assert_eq!(decode(&args(255, |_| {})), EventArgs::Other(255));
}

#[test]
fn event_cdb_lengths_are_bounded() {
    let b = args(1, |a| {
        p8(a, 4, 255);
        p8(a, 5, 255);
    });
    match decode(&b) {
        EventArgs::CdbSense { cdb, sense, .. } => assert_eq!((cdb.len(), sense.len()), (16, 64)),
        other => panic!("{other:?}"),
    }
}

// --- configuration ----------------------------------------------------

/// A configuration: two arrays of 288 bytes (the first with 3 drives, the
/// second with 2), two logical drives of 256 bytes, and two spares of 40.
fn config() -> Vec<u8> {
    let (a, l, s) = (288, 256, 40);
    let mut b = vec![0; 32 + 2 * a + 2 * l + 2 * s];
    let total = b.len() as u32;
    p32(&mut b, 0, total);
    p16(&mut b, 4, 2);
    p16(&mut b, 6, a as u16);
    p16(&mut b, 8, 2);
    p16(&mut b, 10, l as u16);
    p16(&mut b, 12, 2);
    p16(&mut b, 14, s as u16);
    for (i, drives) in [3usize, 2].into_iter().enumerate() {
        let at = 32 + i * a;
        p64(&mut b, at, 1000 * (i as u64 + 1));
        p8(&mut b, at + 8, drives as u8);
        p16(&mut b, at + 10, i as u16 + 1);
        for m in 0..drives {
            let mat = at + 32 + m * 8;
            p16(&mut b, mat, if m == 2 { 0xffff } else { (i * 10 + m) as u16 });
            p16(&mut b, mat + 2, 4);
            p16(&mut b, mat + 4, 0x18);
            p8(&mut b, mat + 6, 1);
            p8(&mut b, mat + 7, (i * 10 + m) as u8);
        }
    }
    for i in 0..2 {
        let at = 32 + 2 * a + i * l;
        p8(&mut b, at, i as u8);
        p8(&mut b, at + 32, 1);
        p8(&mut b, at + 37, 1 + i as u8);
        p8(&mut b, at + 38, 3);
        for sp in 0..=i {
            let sat = at + 64 + sp * 24;
            p64(&mut b, sat, 0);
            p64(&mut b, sat + 8, 500);
            p16(&mut b, sat + 16, sp as u16 + 1);
        }
    }
    for i in 0..2 {
        let at = 32 + 2 * a + 2 * l + i * s;
        p16(&mut b, at, 30 + i as u16);
        p16(&mut b, at + 2, 6);
        p8(&mut b, at + 4, if i == 0 { 1 } else { 0 });
        p8(&mut b, at + 7, if i == 0 { 2 } else { 0 });
        p16(&mut b, at + 8, 1);
        p16(&mut b, at + 10, 2);
    }
    b
}

#[test]
fn config_views() {
    let b = config();
    let c = Config::parse(&b).unwrap();
    assert_eq!(c.size() as usize, b.len());
    assert_eq!((c.array_count(), c.array_size()), (2, 288));
    assert_eq!((c.ld_count(), c.ld_size()), (2, 256));
    assert_eq!((c.spare_count(), c.spare_size()), (2, 40));
    let arrays: Vec<_> = c.arrays().collect();
    assert_eq!(arrays.len(), 2);
    assert_eq!(
        (arrays[0].size_blocks(), arrays[0].array_ref(), arrays[0].num_drives()),
        (1000, 1, 3)
    );
    let m: Vec<_> = arrays[0].members().collect();
    assert_eq!(m.len(), 3);
    assert_eq!(
        m[1],
        ArrayMember { device_id: 1, sequence: 4, state: PdState::Online, enclosure: 1, slot: 1 }
    );
    assert!(!m[1].is_missing() && m[2].is_missing());
    assert_eq!(arrays[1].members().count(), 2);
    let lds: Vec<_> = c.lds().collect();
    assert_eq!(lds.len(), 2);
    assert_eq!((lds[0].target_id(), lds[1].target_id()), (0, 1));
    assert_eq!(lds[0].spans().map(|s| s.array_ref).collect::<Vec<_>>(), [1]);
    assert_eq!(lds[1].spans().map(|s| s.array_ref).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(lds[1].raid_level(), RaidLevel::Raid10);
    assert_eq!(lds[0].state(), LdState::Optimal);
    let spares: Vec<_> = c.spares().collect();
    assert_eq!(spares.len(), 2);
    assert_eq!((spares[0].device_id(), spares[0].sequence()), (30, 6));
    assert!(spares[0].is_dedicated() && !spares[0].is_global());
    assert_eq!(spares[0].arrays().collect::<Vec<_>>(), [1, 2]);
    assert!(spares[1].is_global());
    assert!(!spares[1].is_revertible() && !spares[1].enclosure_affinity());
    assert_eq!(spares[1].arrays().count(), 0);
}

#[test]
fn config_counts_and_strides_are_checked() {
    let b = config();
    assert_eq!(Config::parse(&b[..31]).unwrap_err(), ParseError::Short);
    // Cut in the middle of the last spare.
    assert_eq!(Config::parse(&b[..b.len() - 1]).unwrap_err(), ParseError::BadCount);
    let mut h = config();
    p16(&mut h, 4, 0xffff);
    assert_eq!(Config::parse(&h).unwrap_err(), ParseError::BadCount);
    let mut h = config();
    p16(&mut h, 6, 0xffff);
    p16(&mut h, 10, 0xffff);
    p16(&mut h, 14, 0xffff);
    assert_eq!(Config::parse(&h).unwrap_err(), ParseError::BadCount);
    let mut h = config();
    p16(&mut h, 6, 31);
    assert_eq!(Config::parse(&h).unwrap_err(), ParseError::BadSize);
    let mut h = config();
    p16(&mut h, 10, 63);
    assert_eq!(Config::parse(&h).unwrap_err(), ParseError::BadSize);
    let mut h = config();
    p16(&mut h, 14, 7);
    assert_eq!(Config::parse(&h).unwrap_err(), ParseError::BadSize);
    // Sizes of entries that are not there are not checked.
    let mut e = vec![0; 32];
    p16(&mut e, 6, 1);
    let c = Config::parse(&e).unwrap();
    assert_eq!(c.arrays().count(), 0);
    assert_eq!(c.lds().count() + c.spares().count(), 0);
}

#[test]
fn config_entries_with_small_strides() {
    // Arrays that have room for two drives, not 32; drives that count
    // more than they have room for are read only as far as the entry goes.
    let mut b = vec![0; 32 + 48 + 64 + 12];
    p16(&mut b, 4, 1);
    p16(&mut b, 6, 48);
    p16(&mut b, 8, 1);
    p16(&mut b, 10, 64);
    p16(&mut b, 12, 1);
    p16(&mut b, 14, 12);
    p8(&mut b, 32 + 8, 32);
    p8(&mut b, 32 + 48 + 37, 8);
    p8(&mut b, 32 + 48 + 64 + 7, 16);
    let c = Config::parse(&b).unwrap();
    assert_eq!(c.arrays().next().unwrap().members().count(), 2);
    assert_eq!(c.lds().next().unwrap().spans().count(), 0);
    assert_eq!(c.spares().next().unwrap().arrays().count(), 2);
}

// --- battery ----------------------------------------------------------

fn bbu_status(kind: u8) -> Vec<u8> {
    let mut b = vec![0; BbuStatus::LEN];
    p8(&mut b, 0, kind);
    p16(&mut b, 2, 4050);
    b[4..6].copy_from_slice(&(-120i16).to_le_bytes());
    p16(&mut b, 6, 31);
    p32(&mut b, 8, 1 << 3 | 1 << 6 | 1 << 9);
    p16(&mut b, 32, 0x0abc);
    p16(&mut b, 34, 88);
    p16(&mut b, 38, 500);
    p16(&mut b, 40, 600);
    p8(&mut b, 42, 1);
    b
}

#[test]
fn bbu_status_fields() {
    let b = bbu_status(2);
    let s = BbuStatus::parse(&b).unwrap();
    assert_eq!(s.battery_type(), BbuType::Bbu);
    assert_eq!((s.voltage_mv(), s.current_ma(), s.temperature_c()), (4050, -120, 31));
    let f = s.flags();
    assert!(f.charge_active() && f.learn_cycle_active() && f.i2c_error());
    assert!(!f.pack_missing() && !f.voltage_low() && !f.temperature_high());
    assert!(!f.discharge_active() && !f.learn_cycle_requested());
    assert!(!f.learn_cycle_failed() && !f.learn_cycle_timeout());
    assert_eq!(f.raw(), 0x248);
    assert_eq!(s.gas_gauge_status(), 0xabc);
    assert_eq!(s.relative_charge(), Some(88));
    assert_eq!(s.absolute_charge(), None);
    assert_eq!((s.remaining_capacity(), s.full_charge_capacity()), (Some(500), Some(600)));
    assert_eq!(s.state_of_health_good(), Some(true));
}

#[test]
fn bbu_status_by_type() {
    let b = bbu_status(1);
    let s = BbuStatus::parse(&b).unwrap();
    assert_eq!(s.battery_type(), BbuType::Ibbu);
    assert_eq!(s.relative_charge(), Some(88));
    assert_eq!(s.absolute_charge(), Some(1));
    assert_eq!(s.remaining_capacity(), None);
    assert_eq!(s.state_of_health_good(), None);
    let b = bbu_status(0);
    let s = BbuStatus::parse(&b).unwrap();
    assert_eq!(s.battery_type(), BbuType::None);
    assert_eq!(s.relative_charge(), None);
    let b = bbu_status(9);
    assert_eq!(BbuStatus::parse(&b).unwrap().battery_type(), BbuType::Other(9));
    assert_eq!(BbuStatus::parse(&b[..63]).unwrap_err(), ParseError::Short);
}

#[test]
fn bbu_capacity() {
    let mut b = vec![0; BbuCapacity::LEN];
    for i in 0..11 {
        p16(&mut b, i * 2, 100 + i as u16);
    }
    let c = BbuCapacity::parse(&b).unwrap();
    assert_eq!(
        c,
        BbuCapacity {
            relative_charge: 100,
            absolute_charge: 101,
            remaining_capacity: 102,
            full_charge_capacity: 103,
            run_time_to_empty: 104,
            average_time_to_empty: 105,
            average_time_to_full: 106,
            cycle_count: 107,
            max_error: 108,
            remaining_capacity_alarm: 109,
            remaining_time_alarm: 110,
        }
    );
    assert!(BbuCapacity::parse(&b[..BbuCapacity::MIN_LEN]).is_ok());
    assert_eq!(BbuCapacity::parse(&b[..BbuCapacity::MIN_LEN - 1]).unwrap_err(), ParseError::Short);
}

#[test]
fn bbu_design() {
    let mut b = vec![0; BbuDesign::LEN];
    // 2012-03-17 in the Smart Battery packing.
    p32(&mut b, 0, ((2012 - 1980) << 9 | 0b11 << 5 | 0b1_0001) as u32);
    p16(&mut b, 4, 1700);
    p16(&mut b, 6, 3700);
    p16(&mut b, 8, 0x31);
    p16(&mut b, 10, 4321);
    p16(&mut b, 12, 0x77);
    ptext(&mut b, 14, 12, "LSI");
    ptext(&mut b, 26, 8, "iBBU09");
    ptext(&mut b, 34, 8, "LION");
    b[42..50].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let d = BbuDesign::parse(&b).unwrap();
    assert_eq!(d.manufacture_date(), Some((2012, 3, 17)));
    assert_eq!(d.manufacture_date_raw(), 0x4071);
    assert_eq!((d.design_capacity(), d.design_voltage()), (1700, 3700));
    assert_eq!((d.spec_info(), d.serial_number(), d.pack_stat_config()), (0x31, 4321, 0x77));
    assert_eq!(shown(d.manufacturer()), "LSI");
    assert_eq!(shown(d.device_name()), "iBBU09");
    assert_eq!(shown(d.chemistry()), "LION");
    assert_eq!(d.manufacturer_data(), &[1, 2, 3, 4, 5, 6, 7, 8]);
    p32(&mut b, 0, 0);
    assert_eq!(BbuDesign::parse(&b).unwrap().manufacture_date(), None);
    assert!(BbuDesign::parse(&b[..BbuDesign::MIN_LEN]).is_ok());
    assert_eq!(BbuDesign::parse(&b[..BbuDesign::MIN_LEN - 1]).unwrap_err(), ParseError::Short);
}

// --- no parser panics on any buffer -----------------------------------

fn touch_ld_config(c: &LdConfig<'_>) {
    let _ = (c.target_id(), c.sequence(), c.name().to_string(), c.default_cache_policy());
    let _ = (c.access(), c.disk_cache(), c.current_cache_policy(), c.stripe_bytes());
    let _ = (c.state(), c.raid_level(), c.num_drives(), c.is_consistent(), c.init_state());
    let _ = (c.primary_level(), c.level_qualifier(), c.secondary_level());
    let _ = c.spans().count();
    let _ = c.span(usize::MAX);
}

#[test]
fn nothing_panics_on_garbage() {
    for b in garbage(CtrlInfo::LEN) {
        if let Ok(c) = CtrlInfo::parse(&b) {
            let _ = (c.product_name().to_string(), c.serial_number().to_string());
            let _ = (c.package_version().to_string(), c.firmware_version());
            for k in c.components() {
                let _ = (k.name().to_string(), k.version().to_string());
                let _ = (k.build_date().to_string(), k.build_time().to_string());
            }
            let _ = (c.cache_memory_mb(), c.jbod_supported(), c.roc_temperature());
            let _ = (c.controller_temperature(), c.hw_present(), c.adapter_ops());
            let _ = (c.raid_levels(), c.pci_ids(), c.stripe_size_range(), c.jbod_enabled());
        }
        if let Ok(l) = LdList::parse(&b) {
            assert_eq!(l.iter().count(), l.len());
            assert!(l.get(l.len()).is_none());
        }
        if let Ok(i) = LdInfo::parse(&b) {
            touch_ld_config(&i.config());
            let _ = (i.consistency_check(), i.background_init(), i.foreground_init());
            let _ = (i.reconstruction(), i.size_blocks(), i.reconstruct_active());
        }
        if let Ok(l) = PdList::parse(&b) {
            assert_eq!(l.iter().count(), l.len());
            assert!(l.get(l.len()).is_none());
        }
        if let Ok(p) = PdInfo::parse(&b) {
            let q = p.inquiry();
            let _ = (q.vendor().to_string(), q.product().to_string(), q.revision().to_string());
            let _ = (p.serial(), p.state(), p.interface(), p.sas_address(0), p.sas_address(9));
            let _ = (p.rebuild(), p.patrol(), p.clear(), p.media_type(), p.temperature());
            let _ = (p.interface_ext(), p.smart_alert(), p.ddf_type());
        }
        let _ = EventLogState::parse(&b);
        if let Ok(e) = EventDetail::parse(&b) {
            let _ = (e.class(), e.description().to_string(), e.args());
        }
        if let Ok(c) = Config::parse(&b) {
            for a in c.arrays() {
                let _ = a.members().count();
            }
            for l in c.lds() {
                touch_ld_config(&l);
            }
            for s in c.spares() {
                let _ = s.arrays().count();
            }
        }
        if let Ok(s) = BbuStatus::parse(&b) {
            let _ = (s.battery_type(), s.flags(), s.relative_charge(), s.absolute_charge());
            let _ = (s.remaining_capacity(), s.full_charge_capacity(), s.state_of_health_good());
        }
        let _ = BbuCapacity::parse(&b);
        if let Ok(d) = BbuDesign::parse(&b) {
            let _ = (d.manufacture_date(), d.manufacturer().to_string(), d.chemistry().to_string());
        }
    }
}

#[test]
fn every_argument_type_decodes_on_noise() {
    for t in 0..=255u8 {
        let mut b = noise(EventDetail::LEN, u32::from(t) + 9);
        b[16] = t;
        let e = EventDetail::parse(&b).unwrap();
        let _ = (e.args(), e.class(), e.description().to_string());
    }
}

#[test]
fn config_with_wild_headers_does_not_panic() {
    for seed in 0..200u32 {
        let mut b = noise(32 + (seed as usize * 37) % 900, seed);
        // Make the counts and strides plausible half the time.
        if seed % 2 == 0 {
            for off in [4, 8, 12] {
                p16(&mut b, off, (seed % 4) as u16);
            }
            for off in [6, 10, 14] {
                p16(&mut b, off, 32 + (seed % 300) as u16);
            }
        }
        if let Ok(c) = Config::parse(&b) {
            for a in c.arrays() {
                let _ = a.members().count();
            }
            for l in c.lds() {
                touch_ld_config(&l);
            }
            for s in c.spares() {
                let _ = s.arrays().count();
            }
        }
    }
}
