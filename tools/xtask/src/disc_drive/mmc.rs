// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The drive: the SCSI and MMC-6 commands a CD drive answers, over the
//! disc in it, as the transport hands them over ([`Drive::command`]).
//!
//! It answers what Radix and Linux ask of a CD drive: INQUIRY, TEST UNIT
//! READY and REQUEST SENSE; READ CAPACITY, READ(10) and READ CD; READ TOC
//! in formats 0 (the tracks), 1 (the sessions), 2 (the full TOC) and 5
//! (CD-Text); GET CONFIGURATION, READ DISC INFORMATION, MODE SENSE's
//! capabilities page, and GET EVENT STATUS NOTIFICATION's media events;
//! PREVENT ALLOW MEDIUM REMOVAL, START STOP UNIT, and SET CD SPEED. Any
//! other command fails with ILLEGAL REQUEST, INVALID COMMAND OPERATION
//! CODE, as a drive's does.

use std::collections::VecDeque;

use super::disc::{self, DATA, Disc, Mode, RAW, Sector};

/// Sense keys, and the additional sense codes and qualifiers this drive
/// gives (SPC-5, MMC-6 annex F).
pub mod sense {
    pub const NOT_READY: u8 = 0x02;
    pub const ILLEGAL_REQUEST: u8 = 0x05;
    pub const UNIT_ATTENTION: u8 = 0x06;
    /// MEDIUM NOT PRESENT, tray closed / tray open.
    pub const NO_MEDIUM_CLOSED: (u8, u8) = (0x3a, 0x01);
    pub const NO_MEDIUM_OPEN: (u8, u8) = (0x3a, 0x02);
    /// NOT READY TO READY CHANGE, MEDIUM MAY HAVE CHANGED.
    pub const MEDIUM_CHANGED: (u8, u8) = (0x28, 0x00);
    /// INVALID COMMAND OPERATION CODE.
    pub const INVALID_OPCODE: (u8, u8) = (0x20, 0x00);
    /// INVALID FIELD IN CDB.
    pub const INVALID_FIELD: (u8, u8) = (0x24, 0x00);
    /// LOGICAL BLOCK ADDRESS OUT OF RANGE.
    pub const OUT_OF_RANGE: (u8, u8) = (0x21, 0x00);
    /// ILLEGAL MODE FOR THIS TRACK: an audio sector read as data, or a
    /// data sector as audio.
    pub const ILLEGAL_MODE: (u8, u8) = (0x64, 0x00);
    /// MEDIUM REMOVAL PREVENTED.
    pub const REMOVAL_PREVENTED: (u8, u8) = (0x53, 0x02);
}

/// Media event codes, GET EVENT STATUS NOTIFICATION's (MMC-6, table 154).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaEvent {
    /// The eject button was pressed while the disc was locked in.
    EjectRequest = 1,
    NewMedia = 2,
    MediaRemoval = 3,
}

/// Why a command failed: its sense key and code, which REQUEST SENSE
/// gives afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub key: u8,
    pub code: (u8, u8),
}

impl Check {
    fn new(key: u8, code: (u8, u8)) -> Check {
        Check { key, code }
    }

    fn illegal(code: (u8, u8)) -> Check {
        Check::new(sense::ILLEGAL_REQUEST, code)
    }
}

/// What INQUIRY names the drive.
pub const VENDOR: &[u8; 8] = b"DUKOVO  ";
pub const PRODUCT: &[u8; 16] = b"TEST DISC DRIVE ";
pub const REVISION: &[u8; 4] = b"0001";

/// A CD drive and the disc in it.
#[derive(Default)]
pub struct Drive {
    disc: Option<Disc>,
    /// Its CD-Text packs, made when it went in.
    text: Vec<[u8; 18]>,
    tray_open: bool,
    /// PREVENT ALLOW MEDIUM REMOVAL keeps the disc in.
    locked: bool,
    /// The disc changed since the last command that would say so: the
    /// next fails with UNIT ATTENTION, as a drive's does once.
    attention: bool,
    /// The last command's failure, for REQUEST SENSE.
    last: Option<Check>,
    events: VecDeque<MediaEvent>,
}

/// Operation codes.
mod op {
    pub const TEST_UNIT_READY: u8 = 0x00;
    pub const REQUEST_SENSE: u8 = 0x03;
    pub const INQUIRY: u8 = 0x12;
    pub const MODE_SENSE_6: u8 = 0x1a;
    pub const START_STOP_UNIT: u8 = 0x1b;
    pub const PREVENT_ALLOW: u8 = 0x1e;
    pub const READ_CAPACITY: u8 = 0x25;
    pub const READ_10: u8 = 0x28;
    pub const READ_TOC: u8 = 0x43;
    pub const GET_CONFIGURATION: u8 = 0x46;
    pub const GET_EVENT_STATUS: u8 = 0x4a;
    pub const READ_DISC_INFORMATION: u8 = 0x51;
    pub const MODE_SENSE_10: u8 = 0x5a;
    pub const SET_CD_SPEED: u8 = 0xbb;
    pub const READ_CD: u8 = 0xbe;
}

/// The profile of a pressed CD, and of no disc (MMC-6, 5.3.1).
const CD_ROM: u16 = 0x0008;

impl Drive {
    /// A drive with `disc` in it, or empty.
    pub fn new(disc: Option<Disc>) -> Drive {
        let mut drive = Drive::default();
        if let Some(disc) = disc {
            drive.insert(disc);
            // In at power on: nothing changed.
            drive.attention = false;
            drive.events.clear();
        }
        drive
    }

    /// Whether PREVENT ALLOW MEDIUM REMOVAL keeps its disc in.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Puts `disc` in, with the tray shut: the drive says so once.
    pub fn insert(&mut self, disc: Disc) {
        self.text = disc::cd_text(&disc);
        self.disc = Some(disc);
        self.tray_open = false;
        self.attention = true;
        self.events.push_back(MediaEvent::NewMedia);
    }

    /// Takes the disc out, whether or not it is locked in: pulled out by
    /// force, or the tray opened by its emergency hole. Returns it.
    pub fn take_out(&mut self) -> Option<Disc> {
        let disc = self.disc.take()?;
        self.text.clear();
        self.tray_open = true;
        self.attention = true;
        self.locked = false;
        self.events.push_back(MediaEvent::MediaRemoval);
        Some(disc)
    }

    /// Presses the eject button: a locked disc stays in and the drive
    /// asks for it to be let go; another comes out. Returns the disc that
    /// came out.
    pub fn press_button(&mut self) -> Option<Disc> {
        if self.locked && self.disc.is_some() {
            self.events.push_back(MediaEvent::EjectRequest);
            return None;
        }
        self.take_out()
    }

    /// Carries out `cdb`, with `out` the data the host sent: the data for
    /// the host, or why it failed.
    pub fn command(&mut self, cdb: &[u8], out: &[u8]) -> Result<Vec<u8>, Check> {
        let _ = out;
        let Some(&code) = cdb.first() else { return Err(Check::illegal(sense::INVALID_OPCODE)) };
        // REQUEST SENSE and INQUIRY never fail; GET EVENT STATUS
        // NOTIFICATION and GET CONFIGURATION do not report a change; any
        // other command fails once with UNIT ATTENTION after one.
        let quiet = matches!(
            code,
            op::REQUEST_SENSE | op::INQUIRY | op::GET_EVENT_STATUS | op::GET_CONFIGURATION
        );
        let result = if !quiet && std::mem::take(&mut self.attention) {
            Err(Check::new(sense::UNIT_ATTENTION, sense::MEDIUM_CHANGED))
        } else {
            self.carry_out(code, cdb)
        };
        if code != op::REQUEST_SENSE {
            self.last = result.as_ref().err().copied();
        }
        result
    }

    fn carry_out(&mut self, code: u8, cdb: &[u8]) -> Result<Vec<u8>, Check> {
        let byte = |at: usize| cdb.get(at).copied().unwrap_or(0);
        let word = |at: usize| u16::from_be_bytes([byte(at), byte(at + 1)]);
        let long =
            |at: usize| u32::from_be_bytes([byte(at), byte(at + 1), byte(at + 2), byte(at + 3)]);
        match code {
            op::TEST_UNIT_READY => self.ready().map(|_| Vec::new()),
            op::REQUEST_SENSE => Ok(self.request_sense(usize::from(byte(4)))),
            op::INQUIRY => inquiry(byte(1), usize::from(word(3))),
            op::MODE_SENSE_6 => Ok(self.mode_sense(byte(2) & 0x3f, usize::from(byte(4)), false)),
            op::MODE_SENSE_10 => Ok(self.mode_sense(byte(2) & 0x3f, usize::from(word(7)), true)),
            op::START_STOP_UNIT => self.start_stop(byte(4)).map(|()| Vec::new()),
            op::PREVENT_ALLOW => {
                self.locked = byte(4) & 1 != 0;
                Ok(Vec::new())
            }
            op::READ_CAPACITY => {
                let disc = self.ready()?;
                let last = disc.lead_out().saturating_sub(1);
                Ok([last.to_be_bytes(), (DATA as u32).to_be_bytes()].concat())
            }
            op::READ_10 => self.read_10(long(2), u32::from(word(7))),
            op::READ_TOC => {
                // The format is in byte 2; drives older than MMC-3 had it
                // in the top bits of the control byte.
                let format = match byte(2) & 0xf {
                    0 => byte(9) >> 6,
                    format => format,
                };
                let msf = byte(1) & 2 != 0;
                self.read_toc(format, msf, byte(6), usize::from(word(7)))
            }
            op::GET_CONFIGURATION => Ok(self.configuration(usize::from(word(7)))),
            op::GET_EVENT_STATUS => self.event_status(byte(1), byte(4), usize::from(word(7))),
            op::READ_DISC_INFORMATION => self.disc_information(usize::from(word(7))),
            op::SET_CD_SPEED => Ok(Vec::new()),
            op::READ_CD => {
                let count = u32::from_be_bytes([0, byte(6), byte(7), byte(8)]);
                self.read_cd(long(2), count, (byte(1) >> 2) & 7, byte(9))
            }
            _ => Err(Check::illegal(sense::INVALID_OPCODE)),
        }
    }

    /// The disc, if it is in and readable.
    fn ready(&self) -> Result<&Disc, Check> {
        let code = if self.tray_open { sense::NO_MEDIUM_OPEN } else { sense::NO_MEDIUM_CLOSED };
        self.disc.as_ref().ok_or(Check::new(sense::NOT_READY, code))
    }

    /// Fixed-format sense data of the last command's failure.
    fn request_sense(&mut self, length: usize) -> Vec<u8> {
        let mut data = vec![0u8; 18];
        data[0] = 0x70;
        data[7] = 10;
        if let Some(check) = self.last.take() {
            data[2] = check.key;
            (data[12], data[13]) = check.code;
        }
        data.truncate(length);
        data
    }

    /// MODE SENSE: the CD capabilities and mechanical status page (2Ah)
    /// alone, for that page or all of them; no block descriptors.
    fn mode_sense(&self, page: u8, length: usize, ten: bool) -> Vec<u8> {
        let mut pages = Vec::new();
        if page == 0x2a || page == 0x3f {
            let mut capabilities = vec![0u8; 22];
            capabilities[0] = 0x2a;
            capabilities[1] = 20;
            // Reads CD-R and CD-RW; plays audio; reads CD-DA with READ CD,
            // accurately, and CD-Text (bit 7 of byte 5 is the R-W
            // subchannel; MMC's CD-Text bit is in the CD Read feature).
            capabilities[2] = 0x03;
            capabilities[4] = 0x01;
            capabilities[5] = 0x03;
            // Locks, says whether it is locked, ejects; a tray.
            capabilities[6] = 0x09 | (u8::from(self.locked) << 1) | (1 << 5);
            pages = capabilities;
        }
        let mut data = if ten {
            let mut header = vec![0u8; 8];
            header[..2].copy_from_slice(&((6 + pages.len()) as u16).to_be_bytes());
            header
        } else {
            vec![(3 + pages.len()) as u8, 0, 0, 0]
        };
        data.extend(pages);
        data.truncate(length);
        data
    }

    /// START STOP UNIT: with LoEj, a stop ejects (refused while locked)
    /// and a start loads, shutting the tray.
    fn start_stop(&mut self, flags: u8) -> Result<(), Check> {
        let (load_eject, start) = (flags & 2 != 0, flags & 1 != 0);
        if !load_eject {
            return Ok(());
        }
        if start {
            self.tray_open = false;
            return Ok(());
        }
        if self.locked {
            return Err(Check::illegal(sense::REMOVAL_PREVENTED));
        }
        self.take_out();
        // The drive itself opened its tray: no change to say to the one
        // who asked.
        self.attention = false;
        Ok(())
    }

    fn read_10(&self, lba: u32, count: u32) -> Result<Vec<u8>, Check> {
        let disc = self.ready()?;
        let mut data = Vec::with_capacity(count as usize * DATA);
        for lba in lba..lba.saturating_add(count) {
            match disc.sector(lba) {
                None => return Err(Check::illegal(sense::OUT_OF_RANGE)),
                Some(Sector::Data(user)) => data.extend_from_slice(user),
                Some(Sector::Empty(Mode::Data)) => data.resize(data.len() + DATA, 0),
                Some(Sector::Audio(_) | Sector::Empty(Mode::Audio)) => {
                    return Err(Check::illegal(sense::ILLEGAL_MODE));
                }
            }
        }
        Ok(data)
    }

    /// READ CD of `count` sectors from `lba`, of the expected sector type
    /// `expected` (0 any, 1 CD-DA, 2 mode 1), with the fields `fields`
    /// asks for: an audio sector's 2,352 bytes whenever user data is
    /// asked for (bit 4), a data sector's 2,048 bytes of user data, or its
    /// whole raw sector for all of its fields (F8h).
    fn read_cd(&self, lba: u32, count: u32, expected: u8, fields: u8) -> Result<Vec<u8>, Check> {
        let disc = self.ready()?;
        if !matches!(expected, 0..=2) {
            return Err(Check::illegal(sense::INVALID_FIELD));
        }
        let mut data = Vec::with_capacity(count as usize * RAW);
        for lba in lba..lba.saturating_add(count) {
            let sector = disc.sector(lba).ok_or(Check::illegal(sense::OUT_OF_RANGE))?;
            let audio = matches!(sector, Sector::Audio(_) | Sector::Empty(Mode::Audio));
            if (expected == 1 && !audio) || (expected == 2 && audio) {
                return Err(Check::illegal(sense::ILLEGAL_MODE));
            }
            match sector {
                Sector::Audio(raw) if fields & 0x10 != 0 => data.extend_from_slice(raw),
                Sector::Empty(Mode::Audio) if fields & 0x10 != 0 => {
                    data.resize(data.len() + RAW, 0);
                }
                Sector::Data(user) if fields == 0xf8 => data.extend(raw_sector(lba, user)),
                Sector::Empty(Mode::Data) if fields == 0xf8 => {
                    data.extend(raw_sector(lba, &[0; DATA]));
                }
                Sector::Data(user) if fields & 0x10 != 0 => data.extend_from_slice(user),
                Sector::Empty(Mode::Data) if fields & 0x10 != 0 => {
                    data.resize(data.len() + DATA, 0);
                }
                _ => {}
            }
        }
        Ok(data)
    }

    /// READ TOC/PMA/ATIP.
    fn read_toc(&self, format: u8, msf: bool, track: u8, length: usize) -> Result<Vec<u8>, Check> {
        let disc = self.ready()?;
        let address = |lba: u32| -> [u8; 4] {
            if msf {
                let (m, s, f) = disc::to_msf(lba);
                [0, m, s, f]
            } else {
                lba.to_be_bytes()
            }
        };
        let control = |mode: Mode| if mode == Mode::Data { 0x14 } else { 0x10 };
        let first = disc.tracks.first().map_or(1, |t| t.number);
        let last = disc.tracks.last().map_or(1, |t| t.number);
        let mut body = Vec::new();
        let (one, two) = match format {
            // The tracks from `track` on, then the lead-out.
            0 => {
                if track > last && track != 0xaa {
                    return Err(Check::illegal(sense::INVALID_FIELD));
                }
                for t in disc.tracks.iter().filter(|t| t.number >= track && track != 0xaa) {
                    body.extend([0, control(t.mode), t.number, 0]);
                    body.extend(address(t.start));
                }
                let mode = disc.tracks.last().map_or(Mode::Data, |t| t.mode);
                body.extend([0, control(mode), 0xaa, 0]);
                body.extend(address(disc.lead_out()));
                (first, last)
            }
            // One session: its first track.
            1 => {
                let t = &disc.tracks[0];
                body.extend([0, control(t.mode), t.number, 0]);
                body.extend(address(t.start));
                (1, 1)
            }
            // The full TOC, as the Q subchannel of the lead-in has it.
            2 => {
                let point = |body: &mut Vec<u8>, ctl: u8, point: u8, p: [u8; 3]| {
                    body.extend([1, ctl, 0, point, 0, 0, 0, 0, p[0], p[1], p[2]]);
                };
                let (fc, lc) = (
                    control(disc.tracks[0].mode),
                    control(disc.tracks[disc.tracks.len() - 1].mode),
                );
                point(&mut body, fc, 0xa0, [first, 0x00, 0]);
                point(&mut body, lc, 0xa1, [last, 0, 0]);
                let (m, s, f) = disc::to_msf(disc.lead_out());
                point(&mut body, lc, 0xa2, [m, s, f]);
                for t in &disc.tracks {
                    let (m, s, f) = disc::to_msf(t.start);
                    point(&mut body, control(t.mode), t.number, [m, s, f]);
                }
                (1, 1)
            }
            // CD-Text.
            5 => {
                for pack in &self.text {
                    body.extend_from_slice(pack);
                }
                (0, 0)
            }
            _ => return Err(Check::illegal(sense::INVALID_FIELD)),
        };
        let mut data = ((body.len() + 2) as u16).to_be_bytes().to_vec();
        data.extend([one, two]);
        data.extend(body);
        data.truncate(length);
        Ok(data)
    }

    /// GET CONFIGURATION: the header with the current profile, the
    /// profile list (CD-ROM), the core feature (USB), and the removable
    /// medium feature (a tray that locks and ejects).
    fn configuration(&self, length: usize) -> Vec<u8> {
        let current = if self.disc.is_some() { CD_ROM } else { 0 };
        let mut features = Vec::new();
        // Profile list: CD-ROM, current while a disc is in.
        features.extend([0, 0, 0x03, 4]);
        features.extend(CD_ROM.to_be_bytes());
        features.extend([u8::from(self.disc.is_some()), 0]);
        // Core: USB, and the drive's own GET EVENT STATUS NOTIFICATION.
        features.extend([0, 1, 0x0b, 8, 0, 0, 0, 8, 0x01, 0, 0, 0]);
        // Removable medium: a tray, which ejects and locks.
        features.extend([0, 3, 0x03, 4, 0x29 | (u8::from(self.locked) << 1), 0, 0, 0]);
        let mut data = ((features.len() + 4) as u32).to_be_bytes().to_vec();
        data.extend([0, 0]);
        data.extend(current.to_be_bytes());
        data.extend(features);
        data.truncate(length);
        data
    }

    /// GET EVENT STATUS NOTIFICATION, polled, of the media class: the
    /// oldest event not yet given, and whether a disc is in.
    fn event_status(&mut self, polled: u8, classes: u8, length: usize) -> Result<Vec<u8>, Check> {
        if polled & 1 == 0 {
            return Err(Check::illegal(sense::INVALID_FIELD));
        }
        const MEDIA: u8 = 1 << 4;
        if classes & MEDIA == 0 {
            // No event of a class asked for: NEA.
            let mut data = vec![0, 2, 0x80, MEDIA];
            data.truncate(length);
            return Ok(data);
        }
        let event = self.events.pop_front().map_or(0, |event| event as u8);
        let status = u8::from(self.tray_open) | (u8::from(self.disc.is_some()) << 1);
        let mut data = vec![0, 6, 4, MEDIA, event, status, 0, 0];
        data.truncate(length);
        Ok(data)
    }

    /// READ DISC INFORMATION: a finished disc of one session.
    fn disc_information(&self, length: usize) -> Result<Vec<u8>, Check> {
        let disc = self.ready()?;
        let last = disc.tracks.last().map_or(1, |t| t.number);
        let mut data = vec![0u8; 34];
        data[1] = 32;
        // Complete, its last session complete, not erasable.
        data[2] = 0x0e;
        data[3] = 1;
        data[4] = 1;
        data[5] = disc.tracks[0].number;
        data[6] = last;
        // CD-DA or CD-ROM.
        data[8] = 0;
        // No last session lead-in or possible lead-out: a pressed disc.
        data[16..24].fill(0xff);
        data.truncate(length);
        Ok(data)
    }
}

/// INQUIRY's standard data: a removable CD/DVD device of SPC-3.
fn inquiry(flags: u8, length: usize) -> Result<Vec<u8>, Check> {
    // No vital product data pages.
    if flags & 1 != 0 {
        return Err(Check::illegal(sense::INVALID_FIELD));
    }
    let mut data = vec![0u8; 36];
    // A CD/DVD device, removable, SPC-3, response format 2.
    data[0] = 0x05;
    data[1] = 0x80;
    data[2] = 0x05;
    data[3] = 0x02;
    data[4] = 31;
    data[8..16].copy_from_slice(VENDOR);
    data[16..32].copy_from_slice(PRODUCT);
    data[32..36].copy_from_slice(REVISION);
    data.truncate(length);
    Ok(data)
}

/// A mode 1 sector at `lba`, raw: its sync pattern, its header (its
/// address and mode), and its user data. Its error codes are left zero,
/// which nothing that reads it here checks.
fn raw_sector(lba: u32, user: &[u8]) -> Vec<u8> {
    let mut raw = vec![0u8; RAW];
    raw[1..11].fill(0xff);
    let (m, s, f) = disc::to_msf(lba);
    let bcd = |n: u8| ((n / 10) << 4) | (n % 10);
    raw[12..16].copy_from_slice(&[bcd(m), bcd(s), bcd(f), 1]);
    raw[16..16 + DATA].copy_from_slice(user);
    raw
}
