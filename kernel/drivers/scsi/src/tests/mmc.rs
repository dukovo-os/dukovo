// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! MMC's commands as optical drives take them, what they answer, and
//! [`disc`](crate::mmc::disc) against simulated drives holding data
//! discs, blank ones, music CDs, discs of several sessions, and against
//! drives too old to say.

use proptest::prelude::*;
use std::string::ToString;
use std::vec::Vec;

use super::Sim;
use crate::mmc::{
    self, BLOCK, Configuration, Content, DiscInformation, Events, Family, MediaEvent, MediaStatus,
    Profile, Status, Toc, last_session, toc_format,
};
use crate::*;

/// The disc in a simulated optical drive, and what the drive answers of
/// it, as QEMU's and real drives do.
pub(super) struct Disc {
    profile: u16,
    /// The profiles the drive lists.
    profiles: Vec<u16>,
    /// Its tracks: number, control, start.
    tracks: Vec<(u8, u8, u32)>,
    lead_out: u32,
    /// Its last session's first track and where it starts.
    session: (u8, u32),
    status: u8,
    erasable: bool,
    sessions: u16,
    /// Takes none of GET CONFIGURATION, READ DISC INFORMATION and READ
    /// TOC's session format: a drive older than MMC-2.
    old: bool,
    /// Refuses READ CAPACITY.
    pub(super) no_capacity: bool,
    /// Media events to report, code and status, in order; a code with
    /// its top bit set says no event is available.
    events: Vec<(u8, u8)>,
}

/// A data track's control field: ADR 1, data.
const DATA: u8 = 0x14;
/// An audio track's.
const AUDIO: u8 = 0x10;

impl Disc {
    /// A pressed data disc of `profile`, `blocks` blocks, in a drive that
    /// reads DVDs and CDs.
    fn data(profile: Profile, blocks: u32) -> Disc {
        Disc {
            profile: profile.0,
            profiles: std::vec![0x10, 0x08],
            tracks: std::vec![(1, DATA, 0)],
            lead_out: blocks,
            session: (1, 0),
            status: 2,
            erasable: false,
            sessions: 1,
            old: false,
            no_capacity: false,
            events: Vec::new(),
        }
    }

    pub(super) fn answer(&mut self, cdb: &[u8], no_medium: bool, out: &mut Vec<u8>) -> Reply {
        let illegal = |asc| Sim::sense(key::ILLEGAL_REQUEST, asc, 0);
        match cdb[0] {
            op::GET_EVENT_STATUS_NOTIFICATION if !self.old => {
                assert_eq!((cdb[1] & 1, cdb[4]), (1, 0x10), "polled, of the media class");
                let (code, status) = if self.events.is_empty() {
                    (0, if no_medium { 0 } else { 2 })
                } else {
                    self.events.remove(0)
                };
                if code & 0x80 != 0 {
                    // No event available: a drive without the media class.
                    out.extend([0, 2, 0x80, 0]);
                } else {
                    out.extend([0, 6, 4, 0x10, code, status, 0, 0]);
                }
                Reply::Good { residue: 0 }
            }
            op::GET_CONFIGURATION if !self.old => {
                assert_eq!(&cdb[1..4], [0, 0, 0], "every feature, from the profile list");
                let current = if no_medium { 0 } else { self.profile };
                out.extend([0, 0, 0, 0, 0, 0]);
                out.extend(current.to_be_bytes());
                out.extend([0, 0, 3, 4 * self.profiles.len() as u8]);
                for &profile in &self.profiles {
                    out.extend(profile.to_be_bytes());
                    out.extend([u8::from(profile == current), 0]);
                }
                // The core feature follows.
                out.extend([0, 1, 0x0b, 8, 0, 0, 0, 2, 1, 0, 0, 0]);
                let length = (out.len() - 4) as u32;
                out[0..4].copy_from_slice(&length.to_be_bytes());
                Reply::Good { residue: 0 }
            }
            _ if no_medium => Sim::sense(key::NOT_READY, 0x3a, 1),
            op::READ_DISC_INFORMATION if !self.old => {
                assert_eq!(cdb[1] & 7, 0, "standard disc information");
                let mut bytes = std::vec![0u8; 34];
                bytes[1] = 32;
                bytes[2] = self.status | u8::from(self.erasable) << 4;
                bytes[3] = 1;
                [bytes[9], bytes[4]] = self.sessions.to_be_bytes();
                bytes[5] = self.session.0;
                bytes[6] = self.tracks.last().map_or(1, |t| t.0);
                *out = bytes;
                Reply::Good { residue: 0 }
            }
            op::READ_TOC => {
                let format = cdb[2] & 0xf;
                assert_eq!(cdb[9] >> 6, format, "the format where old drives look too");
                assert_eq!(cdb[1] & 2, 0, "addresses as block numbers");
                match format {
                    toc_format::TOC if !self.tracks.is_empty() => {
                        let first = cdb[6];
                        out.extend([0, 0, self.tracks[0].0, self.tracks.last().unwrap().0]);
                        let lead_out = (0xaa, DATA, self.lead_out);
                        for &(track, control, start) in
                            self.tracks.iter().filter(|t| t.0 >= first).chain([&lead_out])
                        {
                            out.extend([0, control, track, 0]);
                            out.extend(start.to_be_bytes());
                        }
                        let length = (out.len() - 2) as u16;
                        out[0..2].copy_from_slice(&length.to_be_bytes());
                        Reply::Good { residue: 0 }
                    }
                    toc_format::SESSION if !self.old => {
                        let (track, start) = self.session;
                        let control = self.tracks.iter().find(|t| t.0 == track).map_or(0, |t| t.1);
                        out.extend([0, 10, 1, self.sessions as u8, 0, control, track, 0]);
                        out.extend(start.to_be_bytes());
                        Reply::Good { residue: 0 }
                    }
                    _ => illegal(0x24),
                }
            }
            _ => illegal(0x20),
        }
    }
}

impl Sim {
    /// An optical drive holding `disc`.
    fn drive(disc: Disc) -> Sim {
        Sim {
            device_type: device_type::CD,
            blocks: u64::from(disc.lead_out.max(1)),
            block_length: BLOCK,
            disc: Some(disc),
            ..Sim::stick()
        }
    }
}

fn disc_of(sim: &mut Sim) -> (Result<mmc::Disc, Absent>, u32) {
    let mut waited = 0;
    let found = mmc::disc(sim, 0, &mut |ms| waited += ms);
    (found, waited)
}

#[test]
fn mmc_commands_are_laid_out_as_mmc_has_them() {
    assert_eq!(
        Cdb::read_toc(toc_format::TOC, 1, 812).bytes(),
        [op::READ_TOC, 0, 0, 0, 0, 0, 1, 0x03, 0x2c, 0]
    );
    // The session format goes in byte 2 and in the control byte's top
    // bits, where Linux's sr sends it and QEMU's ATAPI drives read it.
    assert_eq!(
        Cdb::read_toc(toc_format::SESSION, 0, 12).bytes(),
        [op::READ_TOC, 0, 1, 0, 0, 0, 0, 0, 12, 0x40]
    );
    assert_eq!(
        Cdb::read_toc(toc_format::FULL, 1, 2048).bytes(),
        [op::READ_TOC, 0, 2, 0, 0, 0, 1, 0x08, 0, 0x80]
    );
    // CD-Text's format is in byte 2 alone: the control byte's two bits
    // cannot hold it.
    assert_eq!(
        Cdb::read_toc(toc_format::CD_TEXT, 0, 0xffff).bytes(),
        [op::READ_TOC, 0, 5, 0, 0, 0, 0, 0xff, 0xff, 0]
    );
    // CD-DA, user data alone: each sector's 2,352 bytes of samples.
    assert_eq!(
        Cdb::read_cd_audio(0x0001_0203, 27).bytes(),
        [op::READ_CD, 0x04, 0, 1, 2, 3, 0, 0, 27, 0x10, 0, 0]
    );
    assert_eq!(
        Cdb::get_configuration(256).bytes(),
        [op::GET_CONFIGURATION, 0, 0, 0, 0, 0, 0, 1, 0, 0]
    );
    assert_eq!(
        Cdb::read_disc_information(34).bytes(),
        [op::READ_DISC_INFORMATION, 0, 0, 0, 0, 0, 0, 0, 34, 0]
    );
    assert_eq!(
        Cdb::get_event_status_media(8).bytes(),
        [op::GET_EVENT_STATUS_NOTIFICATION, 1, 0, 0, 0x10, 0, 0, 0, 8, 0]
    );
}

#[test]
fn profiles_name_the_disc_and_its_family() {
    let names = [
        (Profile::NONE, "none", Family::Other, false),
        (Profile::CD_ROM, "cd-rom", Family::Cd, false),
        (Profile::CD_R, "cd-r", Family::Cd, true),
        (Profile::CD_RW, "cd-rw", Family::Cd, true),
        (Profile::DVD_ROM, "dvd-rom", Family::Dvd, false),
        (Profile::DVD_R, "dvd-r", Family::Dvd, true),
        (Profile::DVD_RAM, "dvd-ram", Family::Dvd, true),
        (Profile::DVD_RW_OVERWRITE, "dvd-rw", Family::Dvd, true),
        (Profile::DVD_RW, "dvd-rw", Family::Dvd, true),
        (Profile::DVD_R_DL, "dvd-r-dl", Family::Dvd, true),
        (Profile::DVD_R_DL_JUMP, "dvd-r-dl", Family::Dvd, true),
        (Profile::DVD_PLUS_RW, "dvd+rw", Family::Dvd, true),
        (Profile::DVD_PLUS_R, "dvd+r", Family::Dvd, true),
        (Profile::DVD_PLUS_RW_DL, "dvd+rw-dl", Family::Dvd, true),
        (Profile::DVD_PLUS_R_DL, "dvd+r-dl", Family::Dvd, true),
        (Profile::BD_ROM, "bd-rom", Family::BluRay, false),
        (Profile::BD_R, "bd-r", Family::BluRay, true),
        (Profile::BD_R_RANDOM, "bd-r", Family::BluRay, true),
        (Profile::BD_RE, "bd-re", Family::BluRay, true),
        (Profile(0x50), "hd-dvd-rom", Family::HdDvd, false),
        (Profile(0x02), "disc", Family::Other, false),
        (Profile(0xffff), "disc", Family::Other, false),
    ];
    for (profile, name, family, recordable) in names {
        assert_eq!(profile.name(), name, "{profile:?}");
        assert_eq!(profile.to_string(), name);
        assert_eq!(profile.family(), family, "{profile:?}");
        assert_eq!(profile.recordable(), recordable, "{profile:?}");
    }
    assert_eq!(Family::BluRay.drive(), "a Blu-ray drive");
    assert_eq!(Family::Other.drive(), "an optical drive");
    assert!(Family::BluRay > Family::Dvd && Family::Dvd > Family::Cd);
}

#[test]
fn the_configuration_says_the_disc_and_the_most_the_drive_reads() {
    let mut sim = Disc::data(Profile::DVD_ROM, 1);
    let mut bytes = Vec::new();
    sim.answer(Cdb::get_configuration(256).bytes(), false, &mut bytes);
    let configuration = Configuration::parse(&bytes).unwrap();
    assert_eq!(configuration, Configuration { current: Profile::DVD_ROM, reads: Family::Dvd });
    // A Blu-ray writer with a CD in it.
    sim.profiles = std::vec![0x43, 0x41, 0x40, 0x1b, 0x10, 0x09, 0x08];
    sim.profile = 0x08;
    bytes.clear();
    sim.answer(Cdb::get_configuration(256).bytes(), false, &mut bytes);
    let configuration = Configuration::parse(&bytes).unwrap();
    assert_eq!(configuration, Configuration { current: Profile::CD_ROM, reads: Family::BluRay });
    // Cut short in the list: as much of it as came.
    let configuration = Configuration::parse(&bytes[..16]).unwrap();
    assert_eq!(configuration.reads, Family::BluRay);
    let configuration = Configuration::parse(&bytes[..12]).unwrap();
    assert_eq!(configuration.reads, Family::Cd);
    // No disc: profile 0.
    bytes.clear();
    sim.answer(Cdb::get_configuration(256).bytes(), true, &mut bytes);
    assert_eq!(Configuration::parse(&bytes).unwrap().current, Profile::NONE);
    assert_eq!(Configuration::parse(&bytes[..7]), None);
}

/// READ TOC format 0 data of `tracks` (number, control, start) and a
/// lead-out at `lead_out`.
fn toc(tracks: &[(u8, u8, u32)], lead_out: u32) -> Vec<u8> {
    let mut out = std::vec![0, 0, 1, tracks.len() as u8];
    for &(track, control, start) in tracks.iter().chain([&(0xaa, DATA, lead_out)]) {
        out.extend([0, control, track, 0]);
        out.extend(start.to_be_bytes());
    }
    let length = (out.len() - 2) as u16;
    out[0..2].copy_from_slice(&length.to_be_bytes());
    out
}

#[test]
fn the_toc_says_which_tracks_hold_data_and_which_audio() {
    let data = Toc::parse(&toc(&[(1, DATA, 0)], 4000)).unwrap();
    assert_eq!(data, Toc { data: 1, audio: 0, lead_out: Some(4000) });
    assert_eq!(data.content(), Content::Data);
    let audio = Toc::parse(&toc(&[(1, AUDIO, 0), (2, AUDIO, 15_000)], 30_000)).unwrap();
    assert_eq!(audio.content(), Content::Audio);
    // An enhanced CD: music, then a data session.
    let mixed = toc(&[(1, AUDIO, 0), (2, AUDIO, 15_000), (3, DATA, 41_400)], 50_000);
    assert_eq!(Toc::parse(&mixed).unwrap().content(), Content::Mixed);
    // Only as many tracks as the length says, and as came.
    let mut short = mixed.clone();
    short[1] = 2 + 8;
    assert_eq!(Toc::parse(&short).unwrap(), Toc { data: 0, audio: 1, lead_out: None });
    let cut = Toc::parse(&mixed[..4 + 8 * 2 + 3]).unwrap();
    assert_eq!(cut, Toc { data: 0, audio: 2, lead_out: None });
    assert_eq!(Toc::parse(&[0, 2, 1]), None);
    assert_eq!(Toc::parse(&[0, 2, 1, 1]).unwrap().content(), Content::Blank);
    for content in [Content::Blank, Content::Data, Content::Audio, Content::Mixed] {
        assert_eq!(content.has_data(), matches!(content, Content::Data | Content::Mixed));
    }
}

#[test]
fn the_last_session_starts_where_its_first_track_does_unless_it_is_the_first() {
    let session = |track: u8, control: u8, start: u32| {
        let mut bytes = std::vec![0, 10, 1, 2, 0, control, track, 0];
        bytes.extend(start.to_be_bytes());
        bytes
    };
    assert_eq!(last_session(&session(2, DATA, 31_000)), Some(31_000));
    assert_eq!(last_session(&session(1, DATA, 0)), Some(0));
    // QEMU names no track: one session.
    assert_eq!(last_session(&session(0, 0, 0)), Some(0));
    // A session starting with music holds no volume to find.
    assert_eq!(last_session(&session(2, AUDIO, 31_000)), Some(0));
    assert_eq!(last_session(&session(2, DATA, 31_000)[..11]), None);
    let mut short = session(2, DATA, 31_000);
    short[1] = 6;
    assert_eq!(last_session(&short), None);
}

#[test]
fn disc_information_says_whether_a_disc_is_blank_and_its_sessions() {
    let info = |state: u8, sessions: u16| {
        let mut bytes = std::vec![0u8; 34];
        bytes[1] = 32;
        bytes[2] = state;
        [bytes[9], bytes[4]] = sessions.to_be_bytes();
        bytes
    };
    let parsed = DiscInformation::parse(&info(0x10, 1)).unwrap();
    assert_eq!(parsed, DiscInformation { status: Status::Empty, erasable: true, sessions: 1 });
    let parsed = DiscInformation::parse(&info(0x0d, 0x0102)).unwrap();
    assert_eq!(
        parsed,
        DiscInformation { status: Status::Incomplete, erasable: false, sessions: 258 }
    );
    assert_eq!(DiscInformation::parse(&info(0x0e, 1)).unwrap().status, Status::Complete);
    assert_eq!(DiscInformation::parse(&info(0x1f, 1)).unwrap().status, Status::Other);
    // Track resources information, not the disc's.
    assert_eq!(DiscInformation::parse(&info(0x20, 1)), None);
    // Short of the high byte of the session count.
    assert_eq!(DiscInformation::parse(&info(0x0e, 0x0102)[..6]).unwrap().sessions, 2);
    assert_eq!(DiscInformation::parse(&info(0x0e, 1)[..4]), None);
}

#[test]
fn media_events_say_the_eject_button_was_pressed_or_a_disc_came() {
    let event = |class: u8, code: u8, status: u8| std::vec![0, 6, class, 0x10, code, status, 0, 0];
    let parsed = MediaStatus::parse(&event(4, 1, 2)).unwrap();
    assert_eq!(parsed, MediaStatus { event: MediaEvent::EjectRequest, present: true, open: false });
    let parsed = MediaStatus::parse(&event(4, 3, 1)).unwrap();
    assert_eq!(parsed, MediaStatus { event: MediaEvent::Removed, present: false, open: true });
    assert_eq!(MediaStatus::parse(&event(4, 2, 2)).unwrap().event, MediaEvent::NewMedia);
    assert_eq!(MediaStatus::parse(&event(4, 4, 2)).unwrap().event, MediaEvent::Changed);
    assert_eq!(MediaStatus::parse(&event(4, 0, 2)).unwrap().event, MediaEvent::Nothing);
    assert_eq!(MediaStatus::parse(&event(4, 6, 2)).unwrap().event, MediaEvent::Other(6));
    // No event available, or of another class.
    assert_eq!(MediaStatus::parse(&event(0x84, 1, 2)), None);
    assert_eq!(MediaStatus::parse(&event(2, 1, 2)), None);
    assert_eq!(MediaStatus::parse(&event(4, 1, 2)[..5]), None);
}

#[test]
fn a_data_disc_is_found_with_its_profile_size_and_one_session() {
    let mut sim = Sim::drive(Disc::data(Profile::DVD_ROM, 2_295_104));
    let (disc, waited) = disc_of(&mut sim);
    let disc = disc.unwrap();
    assert_eq!(waited, 0);
    assert_eq!(
        sim.sent,
        [
            op::TEST_UNIT_READY,
            op::GET_CONFIGURATION,
            op::READ_DISC_INFORMATION,
            op::READ_TOC,
            op::READ_TOC,
            op::READ_CAPACITY_10,
        ]
    );
    assert_eq!(disc.profile, Profile::DVD_ROM);
    assert_eq!(disc.drive, Family::Dvd);
    assert_eq!(disc.content, Content::Data);
    assert_eq!(disc.last_session, 0);
    assert_eq!(disc.sessions, 1);
    let capacity = disc.capacity.unwrap();
    assert_eq!((capacity.blocks, capacity.block_length), (2_295_104, BLOCK));
    assert_eq!(disc.to_string(), "dvd-rom data");
}

#[test]
fn a_disc_of_several_sessions_is_read_from_its_last() {
    // A CD-R written twice: the second session's volume describes both.
    let mut disc = Disc::data(Profile::CD_R, 60_000);
    disc.tracks = std::vec![(1, DATA, 0), (2, DATA, 31_000)];
    disc.session = (2, 31_000);
    disc.sessions = 2;
    disc.status = 1;
    let mut sim = Sim::drive(disc);
    let disc = disc_of(&mut sim).0.unwrap();
    assert_eq!((disc.last_session, disc.sessions), (31_000, 2));
    assert_eq!(disc.to_string(), "cd-r data");
    // An enhanced CD: music, then data, read from the data session.
    let mut disc = Disc::data(Profile::CD_ROM, 50_000);
    disc.tracks = std::vec![(1, AUDIO, 0), (2, AUDIO, 15_000), (3, DATA, 41_400)];
    disc.session = (3, 41_400);
    disc.sessions = 2;
    let disc = disc_of(&mut Sim::drive(disc)).0.unwrap();
    assert_eq!((disc.content, disc.last_session), (Content::Mixed, 41_400));
}

#[test]
fn blank_and_music_discs_have_no_data_to_read() {
    let mut disc = Disc::data(Profile::DVD_R, 0);
    (disc.tracks, disc.status, disc.sessions) = (Vec::new(), 0, 1);
    let mut sim = Sim::drive(disc);
    let disc = disc_of(&mut sim).0.unwrap();
    assert_eq!((disc.content, disc.capacity), (Content::Blank, None));
    assert_eq!(disc.to_string(), "dvd-r blank");
    assert!(!sim.sent.contains(&op::READ_TOC), "a blank disc has no TOC to read");

    let mut disc = Disc::data(Profile::CD_ROM, 300_000);
    disc.tracks = std::vec![(1, AUDIO, 0), (2, AUDIO, 20_000)];
    let mut sim = Sim::drive(disc);
    let disc = disc_of(&mut sim).0.unwrap();
    assert_eq!((disc.content, disc.capacity), (Content::Audio, None));
    assert_eq!(disc.to_string(), "cd-rom audio");
    assert!(!sim.sent.contains(&op::READ_CAPACITY_10));

    // A blank disc in a drive that does not say it is: no TOC, and no
    // capacity.
    let mut disc = Disc::data(Profile::CD_R, 0);
    (disc.tracks, disc.old, disc.no_capacity) = (Vec::new(), true, true);
    let found = disc_of(&mut Sim::drive(disc)).0;
    assert!(matches!(found, Err(Absent::Failed("READ CAPACITY(10)", Some(_)))), "{found:?}");
}

#[test]
fn a_drive_too_old_to_say_holds_a_data_disc_of_what_read_capacity_says() {
    let mut disc = Disc::data(Profile::CD_ROM, 330_000);
    disc.old = true;
    let mut sim = Sim::drive(disc);
    let disc = disc_of(&mut sim).0.unwrap();
    assert_eq!((disc.profile, disc.drive), (Profile::NONE, Family::Other));
    assert_eq!((disc.content, disc.last_session, disc.sessions), (Content::Data, 0, 0));
    assert_eq!(disc.capacity.unwrap().blocks, 330_000);
    assert_eq!(disc.to_string(), "none data");
}

#[test]
fn raw_sector_lengths_are_taken_as_2048_and_the_lead_out_stands_in_for_capacity() {
    for (said, taken) in [(2352, 2048), (2340, 2048), (0, 2048), (2048, 2048), (512, 512)] {
        let mut sim = Sim::drive(Disc::data(Profile::CD_ROM, 1000));
        sim.block_length = said;
        let capacity = disc_of(&mut sim).0.unwrap().capacity.unwrap();
        assert_eq!((capacity.blocks, capacity.block_length), (1000, taken), "{said}");
    }
    let mut disc = Disc::data(Profile::DVD_ROM, 5000);
    disc.no_capacity = true;
    let capacity = disc_of(&mut Sim::drive(disc)).0.unwrap().capacity.unwrap();
    assert_eq!((capacity.blocks, capacity.block_length), (5000, BLOCK));
    // A drive that says one block, of a disc whose TOC says more.
    let mut sim = Sim::drive(Disc::data(Profile::DVD_ROM, 5000));
    sim.blocks = 1;
    assert_eq!(disc_of(&mut sim).0.unwrap().capacity.unwrap().blocks, 5000);
    // Neither: the disc cannot be read.
    let mut disc = Disc::data(Profile::DVD_ROM, 5000);
    (disc.no_capacity, disc.old, disc.tracks) = (true, true, Vec::new());
    let found = disc_of(&mut Sim::drive(disc)).0;
    assert!(matches!(found, Err(Absent::Failed("READ CAPACITY(10)", Some(_)))), "{found:?}");
}

#[test]
fn an_empty_drive_has_no_medium_and_a_spinning_up_one_is_waited_for() {
    let mut sim = Sim::drive(Disc::data(Profile::DVD_ROM, 5000));
    sim.no_medium = true;
    assert_eq!(disc_of(&mut sim).0, Err(Absent::NoMedium));
    let mut sim = Sim::drive(Disc::data(Profile::DVD_ROM, 5000));
    sim.becoming = 3;
    let (disc, waited) = disc_of(&mut sim);
    assert!(disc.is_ok());
    assert_eq!(waited, 3_000);
    // A new disc's attentions, then the disc.
    let mut sim = Sim::drive(Disc::data(Profile::BD_ROM, 12_219_392));
    sim.changes = std::vec![0x3a, 0x28];
    let disc = disc_of(&mut sim).0.unwrap();
    assert_eq!(disc.to_string(), "bd-rom data");
}

#[test]
fn a_drive_says_what_it_reads_with_a_disc_in_it_or_not() {
    let mut disc = Disc::data(Profile::BD_ROM, 1000);
    disc.profiles = std::vec![0x43, 0x40, 0x10, 0x08];
    let mut sim = Sim::drive(disc);
    assert_eq!(mmc::drive(&mut sim, 0), Ok(Family::BluRay));
    sim.no_medium = true;
    assert_eq!(mmc::drive(&mut sim, 0), Ok(Family::BluRay));
    let mut disc = Disc::data(Profile::CD_ROM, 1000);
    disc.old = true;
    assert_eq!(mmc::drive(&mut Sim::drive(disc), 0), Ok(Family::Other));
}

#[test]
fn a_bdxl_disc_is_read_with_ten_byte_commands() {
    // A 128 GB BDXL disc: about 62.5 million blocks, which READ(10)
    // reaches.
    let blocks = 62_500_864;
    let mut sim = Sim::drive(Disc::data(Profile::BD_R, blocks));
    sim.disc.as_mut().unwrap().profiles = std::vec![0x43, 0x41, 0x40];
    let disc = disc_of(&mut sim).0.unwrap();
    let capacity = disc.capacity.unwrap();
    assert_eq!(capacity.bytes(), u64::from(blocks) * 2048);
    assert!(!capacity.needs_16());
    assert_eq!(disc.drive, Family::BluRay);
    assert_eq!(Cdb::read(u64::from(blocks - 1), 32, false).unwrap().opcode(), op::READ_10);
}

#[test]
fn the_simulated_drive_reports_media_events() {
    let mut disc = Disc::data(Profile::DVD_ROM, 5000);
    disc.events = std::vec![(1, 2)];
    let mut sim = Sim::drive(disc);
    let mut bytes = [0u8; 8];
    let cdb = Cdb::get_event_status_media(8);
    sim.command(0, &cdb, Data::In(&mut bytes), 1000).unwrap();
    assert_eq!(MediaStatus::parse(&bytes).unwrap().event, MediaEvent::EjectRequest);
    sim.command(0, &cdb, Data::In(&mut bytes), 1000).unwrap();
    assert_eq!(MediaStatus::parse(&bytes).unwrap().event, MediaEvent::Nothing);
}

proptest! {
    #[test]
    fn any_mmc_answer_is_read_without_panicking(bytes in proptest::collection::vec(any::<u8>(), 0..96)) {
        let _ = Configuration::parse(&bytes);
        let _ = Toc::parse(&bytes).map(|toc| toc.content());
        let _ = last_session(&bytes);
        let _ = DiscInformation::parse(&bytes);
        let _ = MediaStatus::parse(&bytes);
    }
}

#[test]
fn the_eject_button_is_said_among_the_drives_events_until_it_has_none() {
    // Nothing to say: one question.
    let mut sim = Sim::drive(Disc::data(Profile::DVD_ROM, 5000));
    assert_eq!(mmc::events(&mut sim, 0), Ok(Some(Events::default())));
    assert_eq!(sim.sent, [op::GET_EVENT_STATUS_NOTIFICATION]);

    // A disc put in, then the button: both read, until nothing is left.
    let mut disc = Disc::data(Profile::DVD_ROM, 5000);
    disc.events = std::vec![(2, 2), (1, 2)];
    let mut sim = Sim::drive(disc);
    assert_eq!(mmc::events(&mut sim, 0), Ok(Some(Events { eject_request: true, changed: false })));
    assert_eq!(sim.sent, [op::GET_EVENT_STATUS_NOTIFICATION; 3]);
    assert_eq!(mmc::events(&mut sim, 0), Ok(Some(Events::default())));

    // A drive that keeps saying something is asked no more than four times.
    let mut disc = Disc::data(Profile::DVD_ROM, 5000);
    disc.events = std::vec![(4, 2); 9];
    let mut sim = Sim::drive(disc);
    assert_eq!(mmc::events(&mut sim, 0), Ok(Some(Events::default())));
    assert_eq!(sim.sent.len(), 4);
}

#[test]
fn a_drive_without_media_events_is_not_asked_for_them() {
    // Too old for the command.
    let mut disc = Disc::data(Profile::CD_ROM, 5000);
    disc.old = true;
    assert_eq!(mmc::events(&mut Sim::drive(disc), 0), Ok(None));
    // No event of the media class: the drive does not report them.
    let mut disc = Disc::data(Profile::CD_ROM, 5000);
    disc.events = std::vec![(0x80, 0)];
    assert_eq!(mmc::events(&mut Sim::drive(disc), 0), Ok(None));
    // A transport that fails says nothing of it.
    let mut sim = Sim::drive(Disc::data(Profile::CD_ROM, 5000));
    sim.gone = true;
    assert_eq!(mmc::events(&mut sim, 0), Err(TransportError::Gone));
}

#[test]
fn a_unit_attention_answered_to_the_events_is_kept_for_the_look() {
    // A drive that answers the attentions of a disc put in to every
    // command, as MMC has it not do.
    let mut sim = Sim::drive(Disc::data(Profile::DVD_ROM, 5000));
    sim.changes = std::vec![0x3a, 0x28];
    let events = mmc::events(&mut sim, 0).unwrap().unwrap();
    assert_eq!(events, Events { eject_request: false, changed: true });
    assert_eq!(sim.sent, [op::GET_EVENT_STATUS_NOTIFICATION; 3]);
}
