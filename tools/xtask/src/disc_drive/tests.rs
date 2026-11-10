// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use super::disc::{self, DATA, Disc, Error, Mode, RAW, Sector};
use super::mmc::{Check, Drive, sense};
use super::redir::{Packet, Wire, kind};
use super::usb::{self, Device};

/// Three seconds of audio, sector by sector the byte of its number, and
/// a data file of two sectors.
fn files(name: &str) -> Result<Vec<u8>, Error> {
    match name {
        "audio.bin" => Ok((0..225u32).flat_map(|n| [n as u8; RAW]).collect()),
        "data.iso" => Ok(vec![0x5a; 2 * DATA]),
        _ => Err(Error(format!("no {name}"))),
    }
}

/// A disc of two audio tracks in one file, the second with a pregap of
/// one second in the file, and a data track after a PREGAP of two
/// seconds; CD-Text for the disc and each audio track.
const SHEET: &str = r#"
REM made for the test
TITLE "Lab Sessions"
PERFORMER "The Lab"
FILE "audio.bin" BINARY
  TRACK 01 AUDIO
    TITLE "First Light"
    PERFORMER "The Lab"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Static"
    INDEX 00 00:01:00
    INDEX 01 00:02:00
FILE "data.iso" BINARY
  TRACK 03 MODE1/2048
    PREGAP 00:02:00
    INDEX 01 00:00:00
"#;

fn sheet_disc() -> Disc {
    Disc::cue(SHEET, &mut files).unwrap()
}

#[test]
fn a_cue_sheet_gives_each_track_its_start_and_sectors() {
    let disc = sheet_disc();
    let starts: Vec<(u8, Mode, u32, u32, u32)> =
        disc.tracks.iter().map(|t| (t.number, t.mode, t.start, t.sectors, t.pregap)).collect();
    assert_eq!(
        starts,
        [(1, Mode::Audio, 0, 75, 0), (2, Mode::Audio, 150, 75, 75), (3, Mode::Data, 375, 2, 0)]
    );
    assert_eq!(disc.lead_out(), 377);
    assert_eq!(disc.title.as_deref(), Some("Lab Sessions"));
    assert_eq!(disc.tracks[1].title.as_deref(), Some("Static"));
    // Track 2's pregap is in the file: sectors 75 to 149.
    assert_eq!(disc.sector(80), Some(Sector::Audio(&[80; RAW][..])));
    assert_eq!(disc.sector(150), Some(Sector::Audio(&[150; RAW][..])));
    // A PREGAP's silence holds nothing of any file.
    assert_eq!(disc.sector(300), Some(Sector::Empty(Mode::Data)));
    assert_eq!(disc.sector(375), Some(Sector::Data(&[0x5a; DATA][..])));
    assert_eq!(disc.sector(377), None);
}

#[test]
fn cue_sheets_that_do_not_hold_together_are_refused() {
    let refused = |sheet: &str| Disc::cue(sheet, &mut files).unwrap_err().0;
    assert!(refused("TRACK 01 AUDIO\n").contains("before any FILE"));
    assert!(
        refused("FILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n")
            .contains("from 1")
    );
    assert!(
        refused("FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:75\n")
            .contains("mm:ss:ff")
    );
    assert!(refused("FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\n").contains("no INDEX 01"));
    assert!(refused("FILE \"none.bin\" BINARY\n").contains("no none.bin"));
    assert!(
        refused("FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nPREGAP 00:01:00\nINDEX 01 00:00:00\n")
            .contains("two seconds")
    );
}

#[test]
fn big_endian_and_wave_files_become_little_endian_samples() {
    let mut wave = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    wave.extend(16u32.to_le_bytes());
    wave.extend([1, 0, 2, 0]);
    wave.extend(44_100u32.to_le_bytes());
    wave.extend(176_400u32.to_le_bytes());
    wave.extend([4, 0, 16, 0]);
    wave.extend(b"data");
    wave.extend((RAW as u32).to_le_bytes());
    wave.extend((0..RAW / 2).flat_map(|n| (n as u16).to_le_bytes()));
    let mut open = |name: &str| match name {
        "a.wav" => Ok(wave.clone()),
        "a.raw" => Ok((0..RAW / 2).flat_map(|n| (n as u16).to_be_bytes()).collect()),
        _ => Err(Error(name.into())),
    };
    for kind in ["WAVE", "MOTOROLA"] {
        let file = if kind == "WAVE" { "a.wav" } else { "a.raw" };
        let sheet = format!("FILE \"{file}\" {kind}\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n");
        let disc = Disc::cue(&sheet, &mut open).unwrap();
        let Some(Sector::Audio(raw)) = disc.sector(0) else { panic!("no audio") };
        assert_eq!(&raw[2..4], &1u16.to_le_bytes(), "{kind}");
    }
}

#[test]
fn cd_text_packs_carry_each_text_and_a_crc() {
    // CRC-16/XMODEM's check value; CD-Text stores it inverted.
    assert_eq!(disc::crc16(b"123456789"), 0x31c3);
    let packs = disc::cd_text(&sheet_disc());
    // Titles: "Lab Sessions\0First Light\0Static\0\0" (the data track has
    // none): 33 bytes, three packs; performers "The Lab\0The Lab\0\0\0",
    // two; and three of size information.
    let kinds: Vec<u8> = packs.iter().map(|p| p[0]).collect();
    assert_eq!(kinds, [0x80, 0x80, 0x80, 0x81, 0x81, 0x8f, 0x8f, 0x8f]);
    assert_eq!(&packs[0][4..16], b"Lab Sessions");
    // The second pack starts with the album title's end, track 0's 13th
    // character on: position 12.
    assert_eq!((packs[1][1], packs[1][3]), (0, 12));
    assert_eq!(&packs[1][4..16], b"\0First Light");
    // The third starts with the end of track 1's, its 12th character.
    assert_eq!((packs[2][1], packs[2][3]), (1, 11));
    for (n, pack) in packs.iter().enumerate() {
        assert_eq!(pack[2], n as u8, "sequence");
        assert_eq!(!disc::crc16(&pack[..16]), u16::from_be_bytes([pack[16], pack[17]]));
    }
    // Size information: first and last track, three title packs, two
    // performer packs, three of its own, and the last sequence number.
    let info: Vec<u8> = packs[5..].iter().flat_map(|p| p[4..16].to_vec()).collect();
    assert_eq!((info[1], info[2], info[4], info[5], info[19], info[20]), (1, 3, 3, 2, 3, 7));
}

/// Carries out `cdb` on `drive`.
fn run(drive: &mut Drive, cdb: &[u8]) -> Result<Vec<u8>, Check> {
    drive.command(cdb, &[])
}

#[test]
fn the_toc_lists_audio_and_data_tracks_and_the_lead_out() {
    let mut drive = Drive::new(Some(sheet_disc()));
    let toc = run(&mut drive, &[0x43, 0, 0, 0, 0, 0, 1, 0x03, 0x2c, 0]).unwrap();
    assert_eq!(&toc[..4], &[0, 34, 1, 3]);
    assert_eq!(&toc[4..12], &[0, 0x10, 1, 0, 0, 0, 0, 0]);
    assert_eq!(&toc[12..20], &[0, 0x10, 2, 0, 0, 0, 0, 150]);
    assert_eq!(&toc[20..28], &[0, 0x14, 3, 0, 0, 0, 1, 119]);
    assert_eq!(&toc[28..36], &[0, 0x14, 0xaa, 0, 0, 0, 1, 121]);
    // In MSF: block 150 is 00:04:00, two seconds after the start of the
    // program area; the format in the control byte, as older drives
    // take it.
    let msf = run(&mut drive, &[0x43, 2, 0, 0, 0, 0, 2, 0, 12, 0]).unwrap();
    assert_eq!(&msf[4..12], &[0, 0x10, 2, 0, 0, 0, 4, 0]);
    let session = run(&mut drive, &[0x43, 0, 0, 0, 0, 0, 0, 0, 12, 1 << 6]).unwrap();
    assert_eq!(session, [0, 10, 1, 1, 0, 0x10, 1, 0, 0, 0, 0, 0]);
    let full = run(&mut drive, &[0x43, 0, 2, 0, 0, 0, 0, 0x10, 0, 0]).unwrap();
    assert_eq!(&full[4..15], &[1, 0x10, 0, 0xa0, 0, 0, 0, 0, 1, 0, 0]);
    assert_eq!(&full[26..37], &[1, 0x14, 0, 0xa2, 0, 0, 0, 0, 0, 7, 2]);
    let text = run(&mut drive, &[0x43, 0, 5, 0, 0, 0, 0, 0x10, 0, 0]).unwrap();
    assert_eq!(&text[..4], &[0, 2 + 8 * 18, 0, 0]);
    assert_eq!(text[4], 0x80);
}

#[test]
fn read_cd_gives_audio_whole_and_read_10_refuses_it() {
    let mut drive = Drive::new(Some(sheet_disc()));
    // CD-DA, two sectors from block 74: the last of track 1, the first of
    // track 2's pregap.
    let audio = run(&mut drive, &[0xbe, 1 << 2, 0, 0, 0, 74, 0, 0, 2, 0x10, 0, 0]).unwrap();
    assert_eq!(audio.len(), 2 * RAW);
    assert!(audio[..RAW].iter().all(|&b| b == 74) && audio[RAW..].iter().all(|&b| b == 75));
    let mode = |result: Result<Vec<u8>, Check>| result.unwrap_err().code;
    assert_eq!(mode(run(&mut drive, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0])), sense::ILLEGAL_MODE);
    // A data sector read as CD-DA, and past the lead-out.
    assert_eq!(
        mode(run(&mut drive, &[0xbe, 1 << 2, 0, 0, 1, 119, 0, 0, 1, 0x10, 0, 0])),
        sense::ILLEGAL_MODE
    );
    assert_eq!(
        mode(run(&mut drive, &[0xbe, 0, 0, 0, 1, 121, 0, 0, 1, 0x10, 0, 0])),
        sense::OUT_OF_RANGE
    );
    // Data: its user data, or its raw sector with sync and header.
    assert_eq!(run(&mut drive, &[0x28, 0, 0, 0, 1, 119, 0, 0, 1, 0]).unwrap(), [0x5a; DATA]);
    let raw = run(&mut drive, &[0xbe, 0, 0, 0, 1, 119, 0, 0, 1, 0xf8, 0, 0]).unwrap();
    assert_eq!(&raw[..12], &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    // Block 375 is 00:07:00 from the program area's start, in BCD.
    assert_eq!(&raw[12..16], &[0x00, 0x07, 0x00, 1]);
}

#[test]
fn a_disc_put_in_is_said_once_and_a_locked_one_stays_in() {
    let mut drive = Drive::new(None);
    assert_eq!(run(&mut drive, &[0, 0, 0, 0, 0, 0]).unwrap_err().code, sense::NO_MEDIUM_CLOSED);
    let configuration = run(&mut drive, &[0x46, 0, 0, 0, 0, 0, 0, 1, 0, 0]).unwrap();
    assert_eq!(&configuration[6..8], &[0, 0]);
    drive.insert(sheet_disc());
    let changed = run(&mut drive, &[0, 0, 0, 0, 0, 0]).unwrap_err();
    assert_eq!((changed.key, changed.code), (sense::UNIT_ATTENTION, sense::MEDIUM_CHANGED));
    let sense_data = run(&mut drive, &[0x03, 0, 0, 0, 18, 0]).unwrap();
    assert_eq!((sense_data[2], sense_data[12]), (sense::UNIT_ATTENTION, 0x28));
    assert!(run(&mut drive, &[0, 0, 0, 0, 0, 0]).is_ok());
    let event = |drive: &mut Drive| run(drive, &[0x4a, 1, 0, 0, 0x10, 0, 0, 0, 8, 0]).unwrap();
    assert_eq!(event(&mut drive)[4..6], [2, 2]);
    assert_eq!(event(&mut drive)[4..6], [0, 2]);
    // Locked in: START STOP UNIT's eject is refused, and the button asks.
    run(&mut drive, &[0x1e, 0, 0, 0, 1, 0]).unwrap();
    assert_eq!(run(&mut drive, &[0x1b, 0, 0, 0, 2, 0]).unwrap_err().code, sense::REMOVAL_PREVENTED);
    assert!(drive.press_button().is_none());
    assert_eq!(event(&mut drive)[4..6], [1, 2]);
    run(&mut drive, &[0x1e, 0, 0, 0, 0, 0]).unwrap();
    run(&mut drive, &[0x1b, 0, 0, 0, 2, 0]).unwrap();
    assert_eq!(event(&mut drive)[4..6], [3, 1]);
    assert_eq!(run(&mut drive, &[0, 0, 0, 0, 0, 0]).unwrap_err().code, sense::NO_MEDIUM_OPEN);
}

/// A command block wrapper of tag 7 for `cdb`, `expected` bytes in.
fn cbw(cdb: &[u8], expected: u32) -> Vec<u8> {
    let mut wrapper = b"USBC".to_vec();
    wrapper.extend(7u32.to_le_bytes());
    wrapper.extend(expected.to_le_bytes());
    wrapper.extend([0x80, 0, cdb.len() as u8]);
    let mut block = [0u8; 16];
    block[..cdb.len()].copy_from_slice(cdb);
    wrapper.extend(block);
    wrapper
}

#[test]
fn bulk_only_carries_a_command_its_data_and_its_status() {
    let mut device = Device::new(Drive::new(Some(sheet_disc())));
    // Nothing to give before a command.
    assert_eq!(device.bulk_in(512), None);
    device.bulk_out(&cbw(&[0x12, 0, 0, 0, 36, 0], 36)).unwrap();
    let inquiry = device.bulk_in(512).unwrap();
    assert_eq!((inquiry.len(), &inquiry[8..16]), (36, &b"DUKOVO  "[..]));
    let csw = device.bulk_in(13).unwrap();
    assert_eq!(&csw[..4], b"USBS");
    assert_eq!((&csw[4..8], &csw[8..12], csw[12]), (&7u32.to_le_bytes()[..], &[0u8; 4][..], 0));
    // A failed command: no data, all of it the residue, and a failed
    // status; then REQUEST SENSE says why.
    device.bulk_out(&cbw(&[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0], 2048)).unwrap();
    assert_eq!(device.bulk_in(2048).unwrap(), Vec::<u8>::new());
    let csw = device.bulk_in(13).unwrap();
    assert_eq!((&csw[8..12], csw[12]), (&2048u32.to_le_bytes()[..], 1));
    // Data in two transfers: the second short.
    device.bulk_out(&cbw(&[0xbe, 4, 0, 0, 0, 0, 0, 0, 1, 0x10, 0, 0], RAW as u32)).unwrap();
    assert_eq!(device.bulk_in(2048).unwrap().len(), 2048);
    assert_eq!(device.bulk_in(2048).unwrap().len(), RAW - 2048);
    assert_eq!(device.bulk_in(13).unwrap()[12], 0);
    assert_eq!(device.bulk_out(b"not a wrapper"), Err(usb::Stall));
}

#[test]
fn the_device_describes_itself_as_a_bulk_only_drive() {
    let mut device = Device::new(Drive::new(None));
    let descriptor = device.control(0x80, 6, 0x0100, 0, 64).unwrap();
    assert_eq!(&descriptor[..4], &[18, 1, 0x00, 0x02]);
    assert_eq!(&descriptor[8..12], &[0x09, 0x12, 0x01, 0x00]);
    let configuration = device.control(0x80, 6, 0x0200, 0, 255).unwrap();
    assert_eq!(configuration.len(), 32);
    assert_eq!(&configuration[14..17], &[8, 6, 0x50]);
    // A host asks for the first nine bytes, then for the whole.
    assert_eq!(device.control(0x80, 6, 0x0200, 0, 9).unwrap().len(), 9);
    let name = device.control(0x80, 6, 0x0302, 0x0409, 255).unwrap();
    let name: Vec<u16> = name[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16(&name).unwrap(), "Test Disc Drive");
    assert_eq!(device.control(0xa1, 0xfe, 0, 0, 1).unwrap(), [0]);
    assert_eq!(device.control(0x80, 6, 0x0400, 0, 9), Err(usb::Stall));
}

#[test]
fn the_wire_says_hello_and_reads_qemus_packets() {
    let (ours, mut qemu) = UnixStream::pair().unwrap();
    // QEMU's hello: every capability up to buffered bulk input.
    let mut hello = Vec::new();
    hello.extend(kind::HELLO.to_le_bytes());
    hello.extend(68u32.to_le_bytes());
    hello.extend(0u32.to_le_bytes());
    hello.extend([0u8; 64]);
    hello.extend(0xffu32.to_le_bytes());
    qemu.write_all(&hello).unwrap();
    let mut wire = Wire::hello(ours, "test").unwrap();
    let mut theirs = [0u8; 12 + 68];
    qemu.read_exact(&mut theirs).unwrap();
    assert_eq!(&theirs[..12], &[0, 0, 0, 0, 68, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&theirs[12..16], b"test");
    assert_eq!(&theirs[76..80], &0x72u32.to_le_bytes());
    // A bulk transfer in of 70,000 bytes, its length in two halves, and a
    // 64-bit id.
    let mut bulk = Vec::new();
    bulk.extend(kind::BULK_PACKET.to_le_bytes());
    bulk.extend(10u32.to_le_bytes());
    bulk.extend((1u64 << 40).to_le_bytes());
    bulk.extend([0x81, 0]);
    bulk.extend((70_000u32 as u16).to_le_bytes());
    bulk.extend(0u32.to_le_bytes());
    bulk.extend(((70_000u32 >> 16) as u16).to_le_bytes());
    qemu.write_all(&bulk).unwrap();
    assert_eq!(
        wire.read().unwrap(),
        Packet::Bulk { id: 1 << 40, endpoint: 0x81, length: 70_000, data: vec![] }
    );
    wire.bulk_reply(1 << 40, 0x81, 0, 3, &[1, 2, 3]).unwrap();
    let mut reply = [0u8; 16 + 10 + 3];
    qemu.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[..8], &[101, 0, 0, 0, 13, 0, 0, 0]);
    assert_eq!(&reply[16..20], &[0x81, 0, 3, 0]);
    assert_eq!(&reply[26..], &[1, 2, 3]);
}
