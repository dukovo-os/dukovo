// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What burn(1) makes of a drive's answers, and what it sends one: the
//! write parameters, a CD's cue sheet and its sectors of audio.

use super::*;

#[test]
fn each_kind_of_disc_is_written_its_own_way() {
    assert_eq!(Way::of(0x09), Some(Way::Cd { rewritable: false }));
    assert_eq!(Way::of(0x0a), Some(Way::Cd { rewritable: true }));
    assert_eq!(Way::of(0x11), Some(Way::Incremental { rewritable: false }));
    assert_eq!(Way::of(0x14), Some(Way::Incremental { rewritable: true }));
    for profile in [0x1b, 0x2b, 0x41] {
        assert_eq!(Way::of(profile), Some(Way::Sequential), "{profile:#x}");
    }
    for profile in [0x12, 0x13, 0x1a, 0x43] {
        assert_eq!(Way::of(profile), Some(Way::Overwrite), "{profile:#x}");
    }
    // Pressed discs, and no disc.
    for profile in [0x00, 0x08, 0x10, 0x40] {
        assert_eq!(Way::of(profile), None, "{profile:#x}");
    }
    assert_eq!(profile_name(0x1b), "DVD+R");
    assert_eq!(profile_name(0x43), "BD-RE");
}

/// MODE SENSE(10)'s answer for the write parameters page: a header with
/// `descriptors` bytes of block descriptors, then the page, PS set.
fn sensed(descriptors: u16) -> Vec<u8> {
    let mut answer = vec![0u8; 8];
    answer[6..8].copy_from_slice(&descriptors.to_be_bytes());
    answer.extend(core::iter::repeat_n(0xee, usize::from(descriptors)));
    let mut page = vec![0u8; 0x34];
    page[..9].copy_from_slice(&[0x85, 0x32, 0x01, 0xc4, 0x08, 0x10, 0, 0, 0x20]);
    page[14..16].copy_from_slice(&150u16.to_be_bytes());
    answer.extend(page);
    answer
}

#[test]
fn the_write_parameters_are_set_for_each_way_with_underrun_protection() {
    let data =
        write_parameters(&sensed(0), Way::Cd { rewritable: false }, Content::Data, false).unwrap();
    assert_eq!(data.len(), 8 + 0x34);
    assert_eq!(&data[..8], &[0; 8]);
    // PS cleared; BUFE and track at once; no next session, data; mode 1;
    // no link size; CD-ROM's session format; the rest as the drive had it.
    assert_eq!(&data[8..17], &[0x05, 0x32, 0x41, 0x04, 0x08, 0, 0, 0, 0]);
    assert_eq!(&data[22..24], &150u16.to_be_bytes());
    let audio =
        write_parameters(&sensed(8), Way::Cd { rewritable: true }, Content::Audio, true).unwrap();
    // The block descriptors are not sent back.
    assert_eq!(&audio[8..13], &[0x05, 0x32, 0x52, 0x00, 0x00]);
    let dvd =
        write_parameters(&sensed(0), Way::Incremental { rewritable: false }, Content::Data, false)
            .unwrap();
    assert_eq!(&dvd[8..13], &[0x05, 0x32, 0x40, 0x05, 0x08]);
    // Another page, or one cut short, is not the write parameters page.
    let mut other = sensed(0);
    other[8] = 0x2a;
    assert_eq!(write_parameters(&other, Way::Cd { rewritable: false }, Content::Data, false), None);
    let short = &sensed(0)[..20];
    assert_eq!(write_parameters(short, Way::Cd { rewritable: false }, Content::Data, false), None);
}

#[test]
fn a_cue_sheet_has_the_lead_in_the_pregap_each_track_and_the_lead_out() {
    let sheet = cue_sheet(&[300, 450]);
    let (entries, rest) = sheet.as_chunks::<8>();
    assert!(rest.is_empty());
    assert_eq!(
        entries,
        [
            [0x01, 0, 0, 0x01, 0, 0, 0, 0],
            [0x01, 1, 0, 0x00, 0, 0, 0, 0],
            [0x01, 1, 1, 0x00, 0, 0, 2, 0],
            // Block 300 is 00:06:00.
            [0x01, 2, 1, 0x00, 0, 0, 6, 0],
            // Block 750 is 00:12:00.
            [0x01, 0xaa, 1, 0x01, 0, 0, 12, 0],
        ]
    );
    assert_eq!(msf(0), [0, 2, 0]);
    assert_eq!(msf(PREGAP.wrapping_neg()), [0, 0, 0]);
    assert_eq!(msf(4500 - 150 + 74), [1, 0, 74]);
}

#[test]
fn audio_is_padded_to_whole_sectors_and_four_seconds() {
    assert_eq!(cd_sectors(&[1; 10]).len(), SHORTEST_TRACK * AUDIO_SECTOR);
    let long = vec![2u8; 400 * AUDIO_SECTOR + 4];
    let sectors = cd_sectors(&long);
    assert_eq!(sectors.len(), 401 * AUDIO_SECTOR);
    assert_eq!(&sectors[..long.len()], &long[..]);
    assert!(sectors[long.len()..].iter().all(|&b| b == 0));
}

#[test]
fn the_drives_answers_are_read() {
    let mut info = vec![0u8; 34];
    info[2] = 0x1e;
    assert_eq!(DiscInformation::parse(&info), Some(DiscInformation { status: 2, erasable: true }));
    assert_eq!(DiscInformation::parse(&info[..2]), None);
    let mut track = vec![0u8; 36];
    track[2] = 1;
    track[7] = 1;
    track[12..16].copy_from_slice(&40u32.to_be_bytes());
    track[16..20].copy_from_slice(&960u32.to_be_bytes());
    let found = TrackInformation::parse(&track).unwrap();
    assert_eq!(found, TrackInformation { number: 1, next: Some(40), free: 960 });
    track[7] = 0;
    assert_eq!(TrackInformation::parse(&track).unwrap().next, None);
    // A DVD+RW, unformatted, which takes format 26h.
    let mut capacities = vec![0, 0, 0, 16];
    capacities.extend([0, 0x23, 0x05, 0x40, 0x01, 0, 0x08, 0]);
    capacities.extend([0, 0x23, 0x05, 0x40, 0x26 << 2, 0, 0, 0]);
    let found = Capacities::parse(&capacities, 0x1a).unwrap();
    assert_eq!(
        found,
        Capacities { formatted: false, blocks: 0x0023_0540, format: Some((0x0023_0540, 0x26)) }
    );
    // A BD-RE takes the format with spare areas first.
    let mut capacities = vec![0, 0, 0, 24];
    capacities.extend([0, 0, 1, 0, 0x02, 0, 0x08, 0]);
    capacities.extend([0, 0, 1, 0, 0x00, 0, 0, 0]);
    capacities.extend([0, 0, 0, 0xf0, 0x30 << 2, 0, 0, 0]);
    let found = Capacities::parse(&capacities, 0x43).unwrap();
    assert_eq!((found.formatted, found.format), (true, Some((0xf0, 0x30))));
}

#[test]
fn a_busy_drive_is_waited_for_and_a_failure_is_said() {
    assert!(Sense(0x02, 0x04, 0x08).busy());
    assert!(Sense(0x02, 0x04, 0x07).busy());
    assert!(!Sense(0x02, 0x3a, 0x00).busy());
    assert!(!Sense(0x05, 0x21, 0x02).busy());
    assert_eq!(
        Sense(0x05, 0x21, 0x02).words(),
        "not where the disc is written next (sense 5/21/02)"
    );
}
