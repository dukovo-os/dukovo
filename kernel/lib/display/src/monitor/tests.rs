// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the reader on blocks built here as the standards lay them
//! out (EDID 1.4 section 3, CTA-861 section 7.5), field by field, so
//! that what is read back is what was meant and not what happened to be
//! in a sample.

use super::*;

/// A detailed timing, in the numbers a monitor's maker starts from.
struct Timing {
    clock_khz: u32,
    /// Across: the picture, then from its end to the sync pulse, the
    /// pulse, and the whole blanking.
    h: [u32; 4],
    /// Down, likewise.
    v: [u32; 4],
    /// The picture's size in millimetres.
    mm: (u32, u32),
    interlaced: bool,
}

/// 1920 by 1080 at 60 Hz, as CTA-861 times it, on a 27 inch panel.
const FULL_HD: Timing = Timing {
    clock_khz: 148_500,
    h: [1920, 88, 44, 280],
    v: [1080, 4, 5, 45],
    mm: (597, 336),
    interlaced: false,
};

/// 3840 by 2160 at 60 Hz with reduced blanking.
const ULTRA_HD: Timing = Timing {
    clock_khz: 533_250,
    h: [3840, 48, 32, 160],
    v: [2160, 3, 5, 62],
    mm: (597, 336),
    interlaced: false,
};

impl Timing {
    /// The descriptor's 18 bytes (EDID 1.4, table 3.21).
    fn bytes(&self) -> [u8; DESCRIPTOR] {
        let clock = (self.clock_khz / 10) as u16;
        let [width, h_front, h_sync, h_blank] = self.h;
        let [height, v_front, v_sync, v_blank] = self.v;
        let (wide, tall) = self.mm;
        [
            clock as u8,
            (clock >> 8) as u8,
            width as u8,
            h_blank as u8,
            ((width >> 8) << 4 | h_blank >> 8) as u8,
            height as u8,
            v_blank as u8,
            ((height >> 8) << 4 | v_blank >> 8) as u8,
            h_front as u8,
            h_sync as u8,
            ((v_front & 0xf) << 4 | (v_sync & 0xf)) as u8,
            ((h_front >> 8) << 6 | (h_sync >> 8) << 4 | (v_front >> 4) << 2 | v_sync >> 4) as u8,
            wide as u8,
            tall as u8,
            ((wide >> 8) << 4 | tall >> 8) as u8,
            0,
            0,
            // Digital separate sync, both pulses positive.
            0x1e | u8::from(self.interlaced) << 7,
        ]
    }
}

/// A descriptor that is no timing, with tag `tag` and the bytes `body`
/// after its five of header.
fn tagged(tag: u8, body: &[u8]) -> [u8; DESCRIPTOR] {
    let mut descriptor = [0; DESCRIPTOR];
    descriptor[3] = tag;
    descriptor[5..5 + body.len()].copy_from_slice(body);
    descriptor
}

/// A descriptor of text: the text, a line feed if there is room, and
/// spaces.
fn text(tag: u8, text: &str) -> [u8; DESCRIPTOR] {
    let mut body = [b' '; 13];
    body[..text.len()].copy_from_slice(text.as_bytes());
    if let Some(end) = body.get_mut(text.len()) {
        *end = b'\n';
    }
    tagged(tag, &body)
}

/// A range descriptor of `min` to `max` Hz with the offsets `flags`.
fn rates(flags: u8, min: u8, max: u8) -> [u8; DESCRIPTOR] {
    let mut descriptor = tagged(TAG_RANGE, &[min, max, 30, 160, 60, 0, b'\n']);
    descriptor[4] = flags;
    descriptor[12..].fill(b' ');
    descriptor
}

/// A descriptor that says nothing.
const UNUSED: [u8; DESCRIPTOR] = [0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Makes a block's last byte bring its sum to zero.
fn seal(block: &mut [u8; BLOCK]) {
    block[BLOCK - 1] = 0;
    block[BLOCK - 1] = block.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)).wrapping_neg();
}

/// A base block of a Dell monitor on DisplayPort with these four
/// descriptors, not yet sealed.
fn base_of(descriptors: [[u8; DESCRIPTOR]; 4]) -> [u8; BLOCK] {
    let mut block = [0; BLOCK];
    block[..8].copy_from_slice(&HEADER);
    // "DEL": D is 4, E is 5 and L is 12, five bits each.
    block[8..10].copy_from_slice(&[0x10, 0xac]);
    block[10..12].copy_from_slice(&0x4147u16.to_le_bytes());
    block[12..16].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    // Made in week 30 of 2020; EDID 1.4.
    block[16..20].copy_from_slice(&[30, 30, 1, 4]);
    // A digital input of 8 bits a colour on DisplayPort; 60 by 34 cm.
    block[20..23].copy_from_slice(&[0xa5, 60, 34]);
    block[38..54].fill(1);
    for (at, descriptor) in DESCRIPTORS.into_iter().zip(descriptors) {
        block[at..at + DESCRIPTOR].copy_from_slice(&descriptor);
    }
    block
}

/// The usual monitor: full HD, a name and a range.
fn usual() -> [u8; BLOCK] {
    let mut block =
        base_of([FULL_HD.bytes(), text(TAG_NAME, "DELL U2720Q"), rates(0, 56, 76), UNUSED]);
    seal(&mut block);
    block
}

/// A CTA block of extension with these data blocks and timings.
fn extension(data_blocks: &[&[u8]], timings: &[[u8; DESCRIPTOR]]) -> [u8; BLOCK] {
    let mut block = [0; BLOCK];
    block[..2].copy_from_slice(&[EXTENSION_CTA, 3]);
    let mut at = 4;
    for data in data_blocks {
        block[at..at + data.len()].copy_from_slice(data);
        at += data.len();
    }
    block[2] = at as u8;
    for timing in timings {
        block[at..at + DESCRIPTOR].copy_from_slice(timing);
        at += DESCRIPTOR;
    }
    seal(&mut block);
    block
}

/// `base` with `extensions` after it, counted and sealed.
fn with(mut base: [u8; BLOCK], extensions: &[[u8; BLOCK]]) -> Vec<u8> {
    base[126] = extensions.len() as u8;
    seal(&mut base);
    let mut data = base.to_vec();
    for extension in extensions {
        data.extend_from_slice(extension);
    }
    data
}

fn listed(data: &[u8]) -> Vec<Mode> {
    let mut list = Vec::new();
    modes(data, &mut |mode| list.push(mode));
    list
}

fn sizes(data: &[u8]) -> Vec<(u32, u32, u32)> {
    listed(data).iter().map(|mode| (mode.width, mode.height, mode.refresh_mhz)).collect()
}

/// The high dynamic range data block: the extended tag, the transfer
/// functions `functions`, static metadata of type 1, and `rest`.
fn hdr_block(functions: u8, rest: &[u8]) -> Vec<u8> {
    let mut block = vec![CTA_EXTENDED << 5 | (3 + rest.len()) as u8, CTA_HDR, functions, 1];
    block.extend_from_slice(rest);
    block
}

#[test]
fn the_builder_writes_the_timing_full_hd_monitors_send() {
    // The 18 bytes found in the data of a great many 27 inch monitors.
    assert_eq!(
        FULL_HD.bytes(),
        [
            0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0x00, 0x55, 0x50,
            0x21, 0x00, 0x00, 0x1e
        ]
    );
}

#[test]
fn a_monitor_is_read_from_its_base_block() {
    let monitor = Monitor::read(&usual()).unwrap();
    assert_eq!(&monitor.maker, b"DEL\0");
    assert_eq!(monitor.product, 0x4147);
    assert_eq!(monitor.serial, 0x1234_5678);
    assert_eq!(&monitor.name, b"DELL U2720Q\0\0\0\0\0");
    // The first timing's millimetres, not the block's centimetres.
    assert_eq!((monitor.width_mm, monitor.height_mm), (597, 336));
    assert_eq!(monitor.depth, 8);
    assert_eq!((monitor.refresh_min_mhz, monitor.refresh_max_mhz), (56_000, 76_000));
    assert!(!monitor.hdr);
    assert_eq!(monitor.luminance_max, 0);
}

#[test]
fn a_timing_is_read_into_a_mode_with_all_its_numbers() {
    let modes = listed(&usual());
    assert_eq!(
        modes,
        [Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
            flags: MODE_PREFERRED,
            clock_khz: 148_500,
            h_sync_start: 2008,
            h_sync_end: 2052,
            h_total: 2200,
            v_sync_start: 1084,
            v_sync_end: 1089,
            v_total: 1125,
        }]
    );
}

#[test]
fn a_rate_is_worked_out_to_the_thousandth() {
    let film = Timing { clock_khz: 148_350, ..FULL_HD };
    assert_eq!(timing(&film.bytes()).unwrap().refresh_mhz, 59_939);
    assert_eq!(timing(&ULTRA_HD.bytes()).unwrap().refresh_mhz, 59_997);
}

#[test]
fn modes_come_preferred_first_then_extensions_then_the_plain_ones() {
    let mut base = base_of([ULTRA_HD.bytes(), FULL_HD.bytes(), UNUSED, UNUSED]);
    // Standard timings: 1280 by 1024, 1920 by 1080 and 1600 by 1200 at
    // 60 Hz, 1280 by 800 at 75 Hz, and four unused.
    base[38..46].copy_from_slice(&[0x81, 0x80, 0xd1, 0xc0, 0xa9, 0x40, 0x81, 0x0f]);
    // Established: 640 by 480 and 800 by 600 at 60 Hz, 1024 by 768 at
    // 60 Hz, and the interlaced one that is left out.
    base[35..38].copy_from_slice(&[0x21, 0x18, 0x80]);
    let half = Timing { clock_khz: 74_250, h: [1280, 110, 40, 370], v: [720, 5, 5, 30], ..FULL_HD };
    let data = with(base, &[extension(&[], &[half.bytes()])]);
    let modes = listed(&data);
    assert_eq!(modes[0].flags, MODE_PREFERRED);
    assert!(modes[1..].iter().all(|mode| mode.flags == 0));
    assert!(modes[3..].iter().all(|mode| mode.clock_khz == 0 && mode.h_total == 0));
    assert_eq!(
        sizes(&data),
        [
            (3840, 2160, 59_997),
            (1920, 1080, 60_000),
            (1280, 720, 60_000),
            (1280, 1024, 60_000),
            (1920, 1080, 60_000),
            (1600, 1200, 60_000),
            (1280, 800, 75_000),
            (640, 480, 60_000),
            (800, 600, 60_000),
            (1024, 768, 60_000),
            (1152, 870, 75_000),
        ]
    );
}

#[test]
fn a_timing_that_contradicts_itself_is_left_out() {
    let no_width = Timing { h: [0, 88, 44, 280], ..FULL_HD };
    let late_sync = Timing { h: [1920, 250, 44, 280], ..FULL_HD };
    let tall_sync = Timing { v: [1080, 40, 6, 45], ..FULL_HD };
    let interlaced = Timing { interlaced: true, ..FULL_HD };
    let stopped = Timing { clock_khz: 10, h: [4095, 0, 0, 4095], v: [4095, 0, 0, 4095], ..FULL_HD };
    for bad in [no_width, late_sync, tall_sync, interlaced, stopped] {
        assert_eq!(timing(&bad.bytes()), None);
    }
    // A sync pulse that ends where the blanking does is in order.
    let tight = Timing { h: [1920, 236, 44, 280], ..FULL_HD };
    assert_eq!(timing(&tight.bytes()).unwrap().h_sync_end, 2200);
    // And the largest numbers a descriptor holds fit a mode.
    let huge = Timing {
        clock_khz: 655_350,
        h: [4095, 1023, 1023, 4095],
        v: [4095, 63, 63, 4095],
        ..FULL_HD
    };
    let mode = timing(&huge.bytes()).unwrap();
    assert_eq!((mode.h_sync_end, mode.h_total), (6141, 8190));
    assert_eq!((mode.v_sync_end, mode.v_total), (4221, 8190));
    assert_eq!(mode.refresh_mhz, 9_770);
}

#[test]
fn a_range_past_255_hertz_uses_the_offsets() {
    let range_of = |flags, min, max| {
        let mut base = base_of([FULL_HD.bytes(), rates(flags, min, max), UNUSED, UNUSED]);
        seal(&mut base);
        let monitor = Monitor::read(&base).unwrap();
        (monitor.refresh_min_mhz, monitor.refresh_max_mhz)
    };
    assert_eq!(range_of(0, 48, 144), (48_000, 144_000));
    // 48 to 360 Hz: the fastest is written less 255.
    assert_eq!(range_of(2, 48, 105), (48_000, 360_000));
    // 300 to 500 Hz: both are.
    assert_eq!(range_of(3, 45, 245), (300_000, 500_000));
    // Not a range: nothing, or the ends the wrong way round.
    assert_eq!(range_of(0, 0, 60), (0, 0));
    assert_eq!(range_of(0, 75, 60), (0, 0));
}

#[test]
fn the_depth_is_read_only_from_a_digital_input_of_edid_1_4() {
    let depth_of = |revision: u8, input: u8| {
        let mut base = base_of([FULL_HD.bytes(), UNUSED, UNUSED, UNUSED]);
        (base[19], base[20]) = (revision, input);
        seal(&mut base);
        Monitor::read(&base).unwrap().depth
    };
    assert_eq!(depth_of(4, 0x95), 6);
    assert_eq!(depth_of(4, 0xa5), 8);
    assert_eq!(depth_of(4, 0xb5), 10);
    assert_eq!(depth_of(4, 0xc5), 12);
    assert_eq!(depth_of(4, 0xe5), 16);
    // Not said, a code the standard keeps, an analogue input, and
    // EDID 1.3, where those bits mean something else.
    assert_eq!(depth_of(4, 0x85), 0);
    assert_eq!(depth_of(4, 0xf5), 0);
    assert_eq!(depth_of(4, 0x25), 0);
    assert_eq!(depth_of(3, 0xa5), 0);
}

#[test]
fn a_monitor_without_a_name_or_a_size_says_nothing_of_them() {
    let bare = Timing { mm: (0, 0), ..FULL_HD };
    let mut base = base_of([bare.bytes(), UNUSED, UNUSED, UNUSED]);
    // A projector gives no size; and a maker code that is no letter.
    (base[21], base[22]) = (0, 0);
    base[8..10].copy_from_slice(&[0x7f, 0xe0]);
    seal(&mut base);
    let monitor = Monitor::read(&base).unwrap();
    assert_eq!(monitor.name, [0; 16]);
    assert_eq!((monitor.width_mm, monitor.height_mm), (0, 0));
    assert_eq!(&monitor.maker, b"???\0");
    // With only centimetres, those are used.
    (base[21], base[22]) = (60, 34);
    seal(&mut base);
    let monitor = Monitor::read(&base).unwrap();
    assert_eq!((monitor.width_mm, monitor.height_mm), (600, 340));
}

#[test]
fn a_name_is_cut_at_its_line_feed_and_cleaned() {
    let name_of = |descriptor| {
        let mut base = base_of([FULL_HD.bytes(), descriptor, UNUSED, UNUSED]);
        seal(&mut base);
        Monitor::read(&base).unwrap().name
    };
    // Thirteen characters leave no room for the line feed.
    assert_eq!(&name_of(text(TAG_NAME, "THIRTEEN CHAR"))[..14], b"THIRTEEN CHAR\0");
    assert_eq!(&name_of(text(TAG_NAME, "QEMU Monitor"))[..13], b"QEMU Monitor\0");
    let odd = tagged(TAG_NAME, &[b'A', 0x01, 0xe9, b'B', b' ', b' ', b'\n', b'Z']);
    assert_eq!(&name_of(odd)[..5], b"A??B\0");
    // Another kind of text is not the name.
    assert_eq!(name_of(text(0xff, "SERIAL123")), [0; 16]);
}

#[test]
fn high_dynamic_range_is_read_from_the_extension() {
    let read = |blocks: &[&[u8]]| {
        let data = with(usual(), &[extension(blocks, &[])]);
        let monitor = Monitor::read(&data).unwrap();
        (monitor.hdr, monitor.luminance_max)
    };
    // ST 2084 with a brightest picture of code 138, about 993 candelas.
    assert_eq!(read(&[&hdr_block(0b0101, &[138, 96, 20])]), (true, 993));
    // Hybrid log-gamma alone, with no brightness given.
    assert_eq!(read(&[&hdr_block(0b1001, &[])]), (true, 0));
    // The block of a monitor that takes only standard range.
    assert_eq!(read(&[&hdr_block(0b0001, &[])]), (false, 0));
    // It is found after other blocks: here audio and a speaker block.
    let audio = [1 << 5 | 3, 0x09, 0x07, 0x07];
    let speakers = [4 << 5 | 3, 0x01, 0, 0];
    assert_eq!(read(&[&audio, &speakers, &hdr_block(0b0100, &[96])]), (true, 400));
    // Another extended block is not it.
    assert_eq!(read(&[&[CTA_EXTENDED << 5 | 3, 0x05, 0xff, 0xff]]), (false, 0));
    assert_eq!(read(&[]), (false, 0));
}

#[test]
fn a_data_block_longer_than_what_is_left_ends_the_list() {
    // A block that claims 31 bytes where 2 follow, then what would be
    // the high dynamic range block.
    let liar = [2 << 5 | 31, 0xaa, 0xbb];
    let data = with(usual(), &[extension(&[&liar, &hdr_block(0b0100, &[96])], &[])]);
    let monitor = Monitor::read(&data).unwrap();
    assert!(!monitor.hdr);
    // And a block of extension whose timings are said to start before
    // its data could gives neither.
    let mut early = extension(&[&hdr_block(0b0100, &[96])], &[FULL_HD.bytes()]);
    early[2] = 2;
    seal(&mut early);
    let data = with(usual(), &[early]);
    assert!(!Monitor::read(&data).unwrap().hdr);
    assert_eq!(listed(&data).len(), 1);
}

#[test]
fn bytes_that_are_no_monitor_data_are_refused() {
    let good = usual();
    assert_eq!(Monitor::read(&good[..127]), Err(Fault::Short));
    assert_eq!(Monitor::read(&[]), Err(Fault::Short));
    let mut headless = good;
    headless[1] = 0;
    seal(&mut headless);
    assert_eq!(Monitor::read(&headless), Err(Fault::Header));
    let mut spoiled = good;
    spoiled[60] ^= 1;
    assert_eq!(Monitor::read(&spoiled), Err(Fault::Checksum));
    for bad in [&good[..127], &headless[..], &spoiled[..]] {
        assert_eq!(listed(bad), []);
    }
}

#[test]
fn an_extension_that_is_missing_or_spoiled_is_left_out() {
    let more = extension(&[&hdr_block(0b0100, &[96])], &[ULTRA_HD.bytes()]);
    let whole = with(usual(), &[more]);
    assert_eq!(listed(&whole).len(), 2);
    assert!(Monitor::read(&whole).unwrap().hdr);
    // Counted, but cut off by a card that fetched one block.
    let cut = &whole[..BLOCK + 100];
    assert_eq!(listed(cut).len(), 1);
    assert!(!Monitor::read(cut).unwrap().hdr);
    // Spoiled on the way.
    let mut spoiled = whole.clone();
    spoiled[BLOCK + 9] ^= 0x40;
    assert_eq!(listed(&spoiled).len(), 1);
    assert!(!Monitor::read(&spoiled).unwrap().hdr);
    // Of a kind this reader does not know.
    let mut other = more;
    other[0] = 0x70;
    seal(&mut other);
    assert_eq!(listed(&with(usual(), &[other])).len(), 1);
    // And no more blocks are read than the layer keeps, whatever the
    // base block counts.
    let mut many = with(usual(), &[more; 5]);
    many[126] = 255;
    let mut first: [u8; BLOCK] = many[..BLOCK].try_into().unwrap();
    seal(&mut first);
    many[..BLOCK].copy_from_slice(&first);
    assert_eq!(listed(&many).len(), 1 + DATA_MAX / BLOCK - 1);
}

#[test]
fn brightness_codes_are_steps_of_a_32nd_of_a_doubling() {
    assert_eq!(luminance(0), 0);
    assert_eq!(luminance(1), 51);
    assert_eq!(luminance(32), 100);
    assert_eq!(luminance(64), 200);
    assert_eq!(luminance(138), 993);
    assert_eq!(luminance(255), 12_526);
    for coded in 2..=255 {
        assert!(luminance(coded) >= luminance(coded - 1), "{coded}");
    }
}

#[test]
fn no_bytes_make_the_reader_panic() {
    // A small generator of its own, so that a failure is the same on
    // every run.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..4_000 {
        let mut data = [0u8; DATA_MAX + BLOCK];
        data.fill_with(|| next() as u8);
        // Most rounds get past the checks that would stop the reader at
        // the door: a true header, CTA tags and sound sums.
        if round % 8 != 0 {
            data[..8].copy_from_slice(&HEADER);
            for block in data.as_chunks_mut::<BLOCK>().0 {
                if block[0] % 2 == 0 {
                    block[0] = EXTENSION_CTA;
                }
                seal(block);
            }
        }
        let len = next() as usize % (data.len() + 1);
        let data = if round % 4 == 0 { &data[..len] } else { &data[..] };
        let _ = Monitor::read(data);
        for mode in listed(data) {
            assert!(mode.width != 0 && mode.height != 0 && mode.refresh_mhz != 0);
        }
    }
}
