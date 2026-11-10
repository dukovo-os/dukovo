// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! CD, DVD and Blu-ray drives (ADR 0431): what *SCSI Multi-Media
//! Commands* (MMC-6) add to SPC for an optical drive, which SATA and IDE
//! drives take as ATAPI packets and USB drives as SCSI commands.
//!
//! - [`Profile`]: what kind of disc is in the drive, and what kinds the
//!   drive reads (GET CONFIGURATION's profiles).
//! - [`Toc`]: the disc's tracks (READ TOC/PMA/ATIP, format 0), which hold
//!   data and which audio; [`last_session`] (format 1), where the last
//!   session of a multisession disc starts.
//! - [`DiscInformation`]: whether a recordable disc is blank, and how many
//!   sessions it has (READ DISC INFORMATION).
//! - [`MediaEvent`]: what the drive says happened to its disc or tray, of
//!   which the eject button's request (GET EVENT STATUS NOTIFICATION);
//!   [`events`] asks for them.
//! - [`disc`]: what the disc in a drive is, as Linux's `sr` and `cdrom`
//!   find out when one is put in: [`Disc`]; [`drive`], what the drive
//!   reads.
//!
//! The data a disc holds is read with READ(10) in blocks of [`BLOCK`]
//! bytes, as every disc presents it: a CD's mode 1 and mode 2 form 1
//! sectors, a DVD's and a Blu-ray's. Audio tracks hold no such blocks:
//! their sectors are read whole with READ CD ([`Cdb::read_cd_audio`]),
//! [`AUDIO_SECTOR`] bytes of samples each (ADR 0447).

use core::fmt;

use crate::probe::{Asked, TIMEOUT_MS, ask, ready};
use crate::{
    Absent, CAPACITY_10_LENGTH, Capacity, Cdb, Data, Reply, Sense, Transport, TransportError, key,
    op,
};

/// The length of a block of a disc's data, in bytes.
pub const BLOCK: u32 = 2048;
/// The length of a CD's sector of audio, in bytes: 588 frames of two
/// 16-bit samples, left then right, little-endian (1/75 of a second at
/// 44,100 frames a second).
pub const AUDIO_SECTOR: u32 = 2352;

/// The track number of the lead-out, in a TOC: where the disc's last
/// session ends.
const LEAD_OUT: u8 = 0xaa;
/// The bit of a track's control field that says it holds data, not audio
/// (MMC-6, table 495).
const DATA_TRACK: u8 = 0x04;

/// How many bytes of READ TOC data [`disc`] asks for: the header and 100
/// track descriptors, one for each track a CD can have and the lead-out.
pub const TOC_LENGTH: u16 = 4 + 8 * 101;
/// How many bytes of session information [`disc`] asks for.
pub const SESSION_LENGTH: u16 = 12;
/// How many bytes of disc information [`disc`] asks for: the fields MMC-6
/// defines, without the OPC tables that may follow.
pub const DISC_INFORMATION_LENGTH: u16 = 34;
/// How many bytes of configuration [`disc`] asks for: the header and the
/// profile list, as Linux's `cdrom` asks for (QEMU answers no more than
/// 512).
pub const CONFIGURATION_LENGTH: u16 = 256;
/// How many bytes of a media event GET EVENT STATUS NOTIFICATION asks for.
pub const EVENT_LENGTH: u16 = 8;

/// READ TOC/PMA/ATIP formats.
pub mod toc_format {
    /// The tracks of the disc.
    pub const TOC: u8 = 0;
    /// The first track of the last complete session.
    pub const SESSION: u8 = 1;
    /// The full TOC: the lead-in's Q subchannel, session by session.
    pub const FULL: u8 = 2;
    /// CD-Text: the packs of text the lead-in carries in its R-W
    /// subchannel.
    pub const CD_TEXT: u8 = 5;
}

/// READ CD's expected sector type of CD-DA, in bits 4:2 of byte 1: a
/// sector of another type fails the command (ILLEGAL MODE FOR THIS
/// TRACK).
const EXPECT_CD_DA: u8 = 1 << 2;
/// READ CD's field of user data, in byte 9: an audio sector's samples, all
/// 2,352 bytes.
const USER_DATA: u8 = 0x10;

impl Cdb {
    /// READ TOC/PMA/ATIP of `format` ([`toc_format`]) from track or
    /// session `track`, `length` bytes, with addresses as block numbers.
    /// The format goes in byte 2, where MMC has it, and in the top bits of
    /// the control byte, where drives older than MMC-3 look (Linux's `sr`
    /// sends it there, and QEMU's ATAPI drives read it there); those two
    /// bits hold formats 0 to 2 alone, so CD-Text's goes in byte 2 only.
    pub fn read_toc(format: u8, track: u8, length: u16) -> Cdb {
        let [h, l] = length.to_be_bytes();
        let format = format & 0xf;
        let old = if format <= toc_format::FULL { format << 6 } else { 0 };
        Cdb::new(&[op::READ_TOC, 0, format, 0, 0, 0, track, h, l, old])
    }

    /// READ CD of `count` sectors of audio from block `lba`: each sector's
    /// [`AUDIO_SECTOR`] bytes of samples, with no subchannel data and no
    /// C2 error flags (MMC-6, 6.19). A data sector among them fails it.
    pub fn read_cd_audio(lba: u32, count: u32) -> Cdb {
        let [a, b, c, d] = lba.to_be_bytes();
        let [_, h, m, l] = count.to_be_bytes();
        Cdb::new(&[op::READ_CD, EXPECT_CD_DA, a, b, c, d, h, m, l, USER_DATA, 0, 0])
    }

    /// GET CONFIGURATION of every feature from the profile list on,
    /// `length` bytes: the current profile and the profiles the drive
    /// has.
    pub fn get_configuration(length: u16) -> Cdb {
        let [h, l] = length.to_be_bytes();
        Cdb::new(&[op::GET_CONFIGURATION, 0, 0, 0, 0, 0, 0, h, l, 0])
    }

    /// READ DISC INFORMATION, standard disc information, `length` bytes.
    pub fn read_disc_information(length: u16) -> Cdb {
        let [h, l] = length.to_be_bytes();
        Cdb::new(&[op::READ_DISC_INFORMATION, 0, 0, 0, 0, 0, 0, h, l, 0])
    }

    /// GET EVENT STATUS NOTIFICATION of the media class, polled, `length`
    /// bytes.
    pub fn get_event_status_media(length: u16) -> Cdb {
        let [h, l] = length.to_be_bytes();
        Cdb::new(&[op::GET_EVENT_STATUS_NOTIFICATION, 1, 0, 0, MEDIA_CLASS_REQUEST, 0, 0, h, l, 0])
    }
}

/// GET EVENT STATUS NOTIFICATION's request bit of the media class.
const MEDIA_CLASS_REQUEST: u8 = 1 << 4;
/// The notification class of a media event, in its header.
const MEDIA_CLASS: u8 = 4;

/// A kind of disc, or of drive: an MMC profile (MMC-6, 5.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Profile(pub u16);

impl Profile {
    /// No disc, or a drive that does not say.
    pub const NONE: Profile = Profile(0);
    /// A pressed CD.
    pub const CD_ROM: Profile = Profile(0x08);
    /// A CD-R.
    pub const CD_R: Profile = Profile(0x09);
    /// A CD-RW.
    pub const CD_RW: Profile = Profile(0x0a);
    /// A pressed DVD.
    pub const DVD_ROM: Profile = Profile(0x10);
    /// A DVD-R, written sequentially.
    pub const DVD_R: Profile = Profile(0x11);
    /// A DVD-RAM.
    pub const DVD_RAM: Profile = Profile(0x12);
    /// A DVD-RW, overwritten in place.
    pub const DVD_RW_OVERWRITE: Profile = Profile(0x13);
    /// A DVD-RW, written sequentially.
    pub const DVD_RW: Profile = Profile(0x14);
    /// A dual-layer DVD-R, written sequentially.
    pub const DVD_R_DL: Profile = Profile(0x15);
    /// A dual-layer DVD-R, written by layer jumps.
    pub const DVD_R_DL_JUMP: Profile = Profile(0x16);
    /// A DVD+RW.
    pub const DVD_PLUS_RW: Profile = Profile(0x1a);
    /// A DVD+R.
    pub const DVD_PLUS_R: Profile = Profile(0x1b);
    /// A dual-layer DVD+RW.
    pub const DVD_PLUS_RW_DL: Profile = Profile(0x2a);
    /// A dual-layer DVD+R.
    pub const DVD_PLUS_R_DL: Profile = Profile(0x2b);
    /// A pressed Blu-ray disc.
    pub const BD_ROM: Profile = Profile(0x40);
    /// A BD-R, written sequentially.
    pub const BD_R: Profile = Profile(0x41);
    /// A BD-R, written at random.
    pub const BD_R_RANDOM: Profile = Profile(0x42);
    /// A BD-RE.
    pub const BD_RE: Profile = Profile(0x43);

    /// What family of disc it is.
    pub fn family(self) -> Family {
        match self.0 {
            0x08..=0x0a => Family::Cd,
            0x10..=0x2b => Family::Dvd,
            0x40..=0x43 => Family::BluRay,
            0x50..=0x5a => Family::HdDvd,
            _ => Family::Other,
        }
    }

    /// Its name, as the kernel shows a disc: `dvd+r-dl`, `bd-re`; `disc`
    /// for one MMC does not name, and `none` for no disc.
    pub fn name(self) -> &'static str {
        match self.0 {
            0x00 => "none",
            0x08 => "cd-rom",
            0x09 => "cd-r",
            0x0a => "cd-rw",
            0x10 => "dvd-rom",
            0x11 => "dvd-r",
            0x12 => "dvd-ram",
            0x13 | 0x14 => "dvd-rw",
            0x15 | 0x16 => "dvd-r-dl",
            0x17 => "dvd-rw-dl",
            0x1a => "dvd+rw",
            0x1b => "dvd+r",
            0x2a => "dvd+rw-dl",
            0x2b => "dvd+r-dl",
            0x40 => "bd-rom",
            0x41 | 0x42 => "bd-r",
            0x43 => "bd-re",
            0x50 => "hd-dvd-rom",
            0x51 | 0x58 => "hd-dvd-r",
            0x52 => "hd-dvd-ram",
            0x53 | 0x5a => "hd-dvd-rw",
            _ => "disc",
        }
    }

    /// Whether a disc of it can be written: a recordable or rewritable
    /// one, blank or not.
    pub fn recordable(self) -> bool {
        !matches!(self.0, 0x00 | 0x08 | 0x10 | 0x40 | 0x50) && self.family() != Family::Other
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A family of discs, in the order drives came to read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    /// Neither: a drive that does not say, or a profile of another kind.
    Other,
    /// Compact discs.
    Cd,
    /// DVDs.
    Dvd,
    /// HD DVDs.
    HdDvd,
    /// Blu-ray discs, BDXL among them.
    BluRay,
}

impl Family {
    /// What a drive that reads it is called: `a Blu-ray drive`.
    pub fn drive(self) -> &'static str {
        match self {
            Family::Other => "an optical drive",
            Family::Cd => "a CD drive",
            Family::Dvd => "a DVD drive",
            Family::HdDvd => "an HD DVD drive",
            Family::BluRay => "a Blu-ray drive",
        }
    }
}

/// What GET CONFIGURATION said: the profile of the disc in the drive, and
/// the profiles the drive has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// The disc's: [`Profile::NONE`] for none.
    pub current: Profile,
    /// The most a drive reads, of the profiles it lists.
    pub reads: Family,
}

impl Configuration {
    /// Reads GET CONFIGURATION data: its header, and the profile list
    /// feature if it comes first. `None` if the header is short.
    pub fn parse(bytes: &[u8]) -> Option<Configuration> {
        let current = Profile(u16::from_be_bytes(bytes.get(6..8)?.try_into().ok()?));
        // The data length counts the bytes after itself.
        let length = u32::from_be_bytes(bytes.get(0..4)?.try_into().ok()?);
        let end = bytes.len().min(4usize.saturating_add(length as usize));
        let mut reads = current.family();
        // The profile list is feature 0: its code, version, length, and a
        // descriptor of four bytes for each profile.
        if let Some(header) = bytes.get(8..12).filter(|h| h[0] == 0 && h[1] == 0) {
            let list = bytes.get(12..end.min(12 + usize::from(header[3]))).unwrap_or(&[]);
            for profile in list.as_chunks::<4>().0 {
                reads = reads.max(Profile(u16::from_be_bytes([profile[0], profile[1]])).family());
            }
        }
        Some(Configuration { current, reads })
    }
}

/// What a disc's tracks hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// Nothing: a blank recordable disc.
    Blank,
    /// Data tracks only.
    Data,
    /// Audio tracks only: a music CD, which holds no blocks to read.
    Audio,
    /// Both: a CD with music and a data track ("enhanced" or "mixed mode").
    Mixed,
}

impl Content {
    /// Whether it holds blocks of data to read.
    pub fn has_data(self) -> bool {
        matches!(self, Content::Data | Content::Mixed)
    }

    /// Its name, as the kernel shows a disc.
    pub fn name(self) -> &'static str {
        match self {
            Content::Blank => "blank",
            Content::Data => "data",
            Content::Audio => "audio",
            Content::Mixed => "mixed",
        }
    }
}

/// A disc's tracks, from READ TOC format 0 (MMC-6, 6.26.3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toc {
    /// How many tracks hold data.
    pub data: u8,
    /// How many hold audio.
    pub audio: u8,
    /// Where the lead-out starts: the block after the disc's last.
    pub lead_out: Option<u32>,
}

impl Toc {
    /// Reads READ TOC format 0 data, block addresses. `None` if the header
    /// is short.
    pub fn parse(bytes: &[u8]) -> Option<Toc> {
        let length = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
        bytes.get(2..4)?;
        let end = bytes.len().min(2 + usize::from(length));
        let mut toc = Toc { data: 0, audio: 0, lead_out: None };
        for track in bytes.get(4..end).unwrap_or(&[]).as_chunks::<8>().0 {
            let start = u32::from_be_bytes([track[4], track[5], track[6], track[7]]);
            match track[2] {
                LEAD_OUT => toc.lead_out = Some(start),
                _ if track[1] & DATA_TRACK != 0 => toc.data = toc.data.saturating_add(1),
                _ => toc.audio = toc.audio.saturating_add(1),
            }
        }
        Some(toc)
    }

    /// What its tracks hold: [`Content::Blank`] for none.
    pub fn content(&self) -> Content {
        match (self.data, self.audio) {
            (0, 0) => Content::Blank,
            (_, 0) => Content::Data,
            (0, _) => Content::Audio,
            _ => Content::Mixed,
        }
    }
}

/// Where the last complete session of a disc starts, from READ TOC format
/// 1 (MMC-6, 6.26.3.3): the block its first track starts at, as ISO 9660
/// finds the session's volume descriptors from (ECMA-119 and Linux's
/// `sr_cd_check`). 0 when that track is the disc's first or holds audio,
/// as Linux has it. `None` if the data is short.
pub fn last_session(bytes: &[u8]) -> Option<u32> {
    let length = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
    if usize::from(length) + 2 < usize::from(SESSION_LENGTH) {
        return None;
    }
    let descriptor = bytes.get(4..12)?;
    let start = u32::from_be_bytes([descriptor[4], descriptor[5], descriptor[6], descriptor[7]]);
    let data = descriptor[1] & DATA_TRACK != 0;
    Some(if descriptor[2] <= 1 || !data { 0 } else { start })
}

/// Where a disc's recording stands, from READ DISC INFORMATION.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing is written on it.
    Empty,
    /// Something is, and more can be.
    Incomplete,
    /// It is finished: nothing more can be written.
    Complete,
    /// Another state: a rewritable disc written at random, as a DVD-RAM.
    Other,
}

/// What READ DISC INFORMATION said of a disc (MMC-6, 6.22.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscInformation {
    /// Where its recording stands.
    pub status: Status,
    /// Whether it can be erased: a rewritable disc.
    pub erasable: bool,
    /// How many sessions it has, the one open for writing among them.
    pub sessions: u16,
}

impl DiscInformation {
    /// Reads standard disc information. `None` if it is short of the
    /// session count, or of another type.
    pub fn parse(bytes: &[u8]) -> Option<DiscInformation> {
        let state = *bytes.get(2)?;
        // The data type, standard disc information, is 0.
        if state >> 5 != 0 {
            return None;
        }
        let status = match state & 3 {
            0 => Status::Empty,
            1 => Status::Incomplete,
            2 => Status::Complete,
            _ => Status::Other,
        };
        let low = *bytes.get(4)?;
        let high = bytes.get(9).copied().unwrap_or(0);
        Some(DiscInformation {
            status,
            erasable: state & 0x10 != 0,
            sessions: u16::from_be_bytes([high, low]),
        })
    }
}

/// What a drive says happened to its disc or tray, from GET EVENT STATUS
/// NOTIFICATION's media class (MMC-6, 6.7.2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaEvent {
    /// Nothing since it was last asked.
    Nothing,
    /// Someone pressed its eject button: it asks for its disc to be let
    /// go.
    EjectRequest,
    /// A disc was put in.
    NewMedia,
    /// The disc was taken out.
    Removed,
    /// The disc was changed.
    Changed,
    /// Another event, by its code.
    Other(u8),
}

/// A media event and the state of the drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaStatus {
    /// What happened.
    pub event: MediaEvent,
    /// Whether a disc is in it.
    pub present: bool,
    /// Whether its tray or door is open.
    pub open: bool,
}

impl MediaStatus {
    /// Reads GET EVENT STATUS NOTIFICATION data. `None` if it is short or
    /// has no media event (No Event Available, or another class).
    pub fn parse(bytes: &[u8]) -> Option<MediaStatus> {
        let class = *bytes.get(2)?;
        if class & 0x80 != 0 || class & 7 != MEDIA_CLASS {
            return None;
        }
        let event = match bytes.get(4)? & 0xf {
            0 => MediaEvent::Nothing,
            1 => MediaEvent::EjectRequest,
            2 => MediaEvent::NewMedia,
            3 => MediaEvent::Removed,
            4 => MediaEvent::Changed,
            other => MediaEvent::Other(other),
        };
        let status = *bytes.get(5)?;
        Some(MediaStatus { event, present: status & 2 != 0, open: status & 1 != 0 })
    }
}

/// What an optical drive's media events said since it was last asked
/// ([`events`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events {
    /// Someone pressed its eject button while its disc was locked in: the
    /// drive asks for the disc to be let go, and keeps it in until then.
    pub eject_request: bool,
    /// It answered a UNIT ATTENTION that its disc may have changed, as it
    /// would have answered the look's TEST UNIT READY
    /// ([`Sense::medium_changed`]).
    pub changed: bool,
}

/// How many times [`events`] asks at most: a drive keeps an event of each
/// kind, and says the next at the next ask.
const EVENT_TRIES: u32 = 4;

/// Asks the drive at logical unit `lun` behind `transport` what happened
/// to its disc since it was last asked, as Linux's `sr` does before its
/// TEST UNIT READY: GET EVENT STATUS NOTIFICATION of the media class,
/// polled, until the drive has nothing more to say, up to
/// [`EVENT_TRIES`] times. `None` for a drive that does not take the
/// command, or has no media events (an old drive), which need not be
/// asked again. A disc put in or taken out is left to the look's TEST
/// UNIT READY, which says it too; MMC has the command pass a UNIT
/// ATTENTION by, and one a drive answers all the same is kept in
/// [`Events::changed`] for the look.
pub fn events<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
) -> Result<Option<Events>, TransportError> {
    let mut found = Events::default();
    let mut bytes = [0u8; EVENT_LENGTH as usize];
    let cdb = Cdb::get_event_status_media(EVENT_LENGTH);
    for _ in 0..EVENT_TRIES {
        bytes.fill(0);
        let status = match transport.command(lun, &cdb, Data::In(&mut bytes), TIMEOUT_MS) {
            Ok(Reply::Good { residue }) => {
                let came = bytes.len().saturating_sub(residue as usize);
                MediaStatus::parse(&bytes[..came])
            }
            Ok(Reply::Failed(Some(sense))) if sense.key == key::ILLEGAL_REQUEST => return Ok(None),
            Ok(Reply::Failed(Some(sense))) if sense.key == key::UNIT_ATTENTION => {
                found.changed |= sense.medium_changed();
                continue;
            }
            // Busy, or another trouble: asked again at the next look.
            Ok(Reply::Failed(_)) | Err(TransportError::Failed(_)) => break,
            Err(TransportError::Gone) => return Err(TransportError::Gone),
        };
        // No media class: the drive has no media events to give.
        let Some(status) = status else { return Ok(None) };
        match status.event {
            MediaEvent::Nothing => break,
            MediaEvent::EjectRequest => found.eject_request = true,
            _ => {}
        }
    }
    Ok(Some(found))
}

/// The disc in a drive, as [`disc`] found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Disc {
    /// What kind it is: [`Profile::NONE`] if the drive did not say.
    pub profile: Profile,
    /// The most the drive reads.
    pub drive: Family,
    /// What its tracks hold.
    pub content: Content,
    /// How big its data is, in blocks of [`BLOCK`] bytes (or of 512, a
    /// drive set so): `None` for a disc of no data.
    pub capacity: Option<Capacity>,
    /// Where its last session starts, in blocks: 0 for a disc of one
    /// session.
    pub last_session: u32,
    /// How many sessions it has: 0 if the drive did not say.
    pub sessions: u16,
}

impl fmt::Display for Disc {
    /// `dvd+r data`, `cd-rom audio`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.profile, self.content.name())
    }
}

/// Finds out what the disc in the drive at logical unit `lun` behind
/// `transport` is, as Linux's `sr` and `cdrom` do when a disc is put in:
/// that it is ready (waiting, as `wait` waits, while it spins up), its
/// profile (GET CONFIGURATION), whether it is blank (READ DISC
/// INFORMATION), what its tracks hold (READ TOC), where its last session
/// starts, and how big it is (READ CAPACITY(10), or the TOC's lead-out
/// for a drive that does not say). A drive that takes none of the MMC
/// commands, as some old CD drives, has a data disc of what READ
/// CAPACITY says. Fails with [`Absent::NoMedium`] if no disc is in it.
pub fn disc<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    wait: &mut dyn FnMut(u32),
) -> Result<Disc, Absent> {
    ready(transport, lun, wait)?;
    let mut bytes = [0u8; TOC_LENGTH as usize];

    let asked = &mut bytes[..usize::from(CONFIGURATION_LENGTH)];
    let configuration =
        match ask(transport, lun, &Cdb::get_configuration(CONFIGURATION_LENGTH), asked)? {
            Asked::Came(came) => Configuration::parse(&asked[..came]),
            Asked::Failed(_) => None,
        };
    let (profile, drive) =
        configuration.map_or((Profile::NONE, Family::Other), |c| (c.current, c.reads));

    let asked = &mut bytes[..usize::from(DISC_INFORMATION_LENGTH)];
    let cdb = Cdb::read_disc_information(DISC_INFORMATION_LENGTH);
    let information = match ask(transport, lun, &cdb, asked)? {
        Asked::Came(came) => DiscInformation::parse(&asked[..came]),
        Asked::Failed(_) => None,
    };
    let sessions = information.map_or(0, |i| i.sessions);
    let blank =
        Disc { profile, drive, content: Content::Blank, capacity: None, last_session: 0, sessions };
    if information.is_some_and(|i| i.status == Status::Empty) {
        return Ok(blank);
    }

    let cdb = Cdb::read_toc(toc_format::TOC, 1, TOC_LENGTH);
    let toc = match ask(transport, lun, &cdb, &mut bytes)? {
        Asked::Came(came) => Toc::parse(&bytes[..came]),
        Asked::Failed(_) => None,
    };
    // A drive that reads no TOC is taken to hold a data disc: READ
    // CAPACITY says whether it does.
    let content = toc.map_or(Content::Data, |toc| toc.content());
    if !content.has_data() {
        return Ok(Disc { content, ..blank });
    }

    let asked = &mut bytes[..usize::from(SESSION_LENGTH)];
    let cdb = Cdb::read_toc(toc_format::SESSION, 0, SESSION_LENGTH);
    let last_session = match ask(transport, lun, &cdb, asked)? {
        Asked::Came(came) => last_session(&asked[..came]).unwrap_or(0),
        Asked::Failed(_) => 0,
    };
    let capacity = capacity(transport, lun, toc.and_then(|toc| toc.lead_out))?;
    Ok(Disc { content, capacity: Some(capacity), last_session, ..blank })
}

/// The most the drive at logical unit `lun` behind `transport` reads, by
/// the profiles GET CONFIGURATION lists, with a disc in it or not:
/// [`Family::Other`] for a drive that does not say.
pub fn drive<T: Transport + ?Sized>(transport: &mut T, lun: u8) -> Result<Family, Absent> {
    let mut bytes = [0u8; CONFIGURATION_LENGTH as usize];
    match ask(transport, lun, &Cdb::get_configuration(CONFIGURATION_LENGTH), &mut bytes)? {
        Asked::Came(came) => {
            Ok(Configuration::parse(&bytes[..came]).map_or(Family::Other, |c| c.reads))
        }
        Asked::Failed(_) => Ok(Family::Other),
    }
}

/// How big the disc's data is: READ CAPACITY(10), whose block length is
/// taken as [`BLOCK`] whatever a drive says but 512 (as Linux's `sr`
/// does: some say 2,340 or 2,352, the length of a CD's raw sector), or
/// the TOC's `lead_out` when the drive does not say or says a single
/// block.
fn capacity<T: Transport + ?Sized>(
    transport: &mut T,
    lun: u8,
    lead_out: Option<u32>,
) -> Result<Capacity, Absent> {
    let mut bytes = [0u8; CAPACITY_10_LENGTH];
    let (said, sense) = match ask(transport, lun, &Cdb::read_capacity_10(), &mut bytes)? {
        Asked::Came(came) => (Capacity::parse_10(&bytes[..came]), None),
        Asked::Failed(sense) => (None, sense),
    };
    let capacity = match (said, lead_out) {
        (Some(said), _) if said.blocks > 1 || lead_out.is_none() => said,
        (_, Some(lead_out)) => {
            Capacity { blocks: u64::from(lead_out), block_length: BLOCK, physical_exponent: 0 }
        }
        _ => return Err(failed(sense)),
    };
    let block_length = if capacity.block_length == 512 { 512 } else { BLOCK };
    let capacity = Capacity { block_length, ..capacity };
    if !capacity.usable() {
        return Err(Absent::BadCapacity(capacity));
    }
    Ok(capacity)
}

/// Why READ CAPACITY failed.
fn failed(sense: Option<Sense>) -> Absent {
    match sense.map(|s| s.meaning()) {
        Some(crate::Meaning::NoMedium) => Absent::NoMedium,
        _ => Absent::Failed("READ CAPACITY(10)", sense),
    }
}
