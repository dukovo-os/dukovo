// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A recordable disc in the emulated drive, and the MMC-6 commands that
//! write it (ADR 0449), as burn(1) sends them: the write parameters page
//! (MODE SENSE and MODE SELECT), READ TRACK INFORMATION, READ FORMAT
//! CAPACITIES, FORMAT UNIT, BLANK, SEND CUE SHEET, WRITE(10), SYNCHRONIZE
//! CACHE and CLOSE TRACK/SESSION; RESERVE TRACK and SET STREAMING are
//! taken and change nothing.
//!
//! A disc is one of [`Kind`]'s: a CD-R or CD-RW written a track at a
//! time (TAO) with data or at once (SAO) with audio from a cue sheet; a
//! DVD-R written incrementally; a DVD+R or BD-R written in sequence; a
//! DVD+RW or BD-RE formatted and then written anywhere. Written and
//! closed, it holds the tracks the drive reads back
//! ([`Recordable::tracks`]). A test write records nothing.
//!
//! Start at [`Recordable::blank`].

use super::disc::{DATA, LEAD_IN, Mode, RAW, Source, Track};
use super::mmc::{Check, sense};

/// The kinds of recordable disc, by their profile (MMC-6, 5.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    CdR,
    CdRw,
    DvdR,
    DvdPlusRw,
    DvdPlusR,
    BdR,
    BdRe,
}

impl Kind {
    /// Its profile number.
    pub fn profile(self) -> u16 {
        match self {
            Kind::CdR => 0x09,
            Kind::CdRw => 0x0a,
            Kind::DvdR => 0x11,
            Kind::DvdPlusRw => 0x1a,
            Kind::DvdPlusR => 0x1b,
            Kind::BdR => 0x41,
            Kind::BdRe => 0x43,
        }
    }

    /// The kind `name` names: `cd-r`, `cd-rw`, `dvd-r`, `dvd+rw`,
    /// `dvd+r`, `bd-r`, `bd-re`.
    pub fn named(name: &str) -> Option<Kind> {
        Some(match name {
            "cd-r" => Kind::CdR,
            "cd-rw" => Kind::CdRw,
            "dvd-r" => Kind::DvdR,
            "dvd+rw" => Kind::DvdPlusRw,
            "dvd+r" => Kind::DvdPlusR,
            "bd-r" => Kind::BdR,
            "bd-re" => Kind::BdRe,
            _ => return None,
        })
    }

    /// Whether it is a CD, written with the write parameters page.
    pub fn cd(self) -> bool {
        matches!(self, Kind::CdR | Kind::CdRw)
    }

    /// Whether it is formatted and written anywhere, not in sequence.
    pub fn random(self) -> bool {
        matches!(self, Kind::DvdPlusRw | Kind::BdRe)
    }

    /// Whether it is erased with BLANK.
    pub fn blankable(self) -> bool {
        self == Kind::CdRw
    }

    /// Whether it is written again, by BLANK or in place.
    pub fn erasable(self) -> bool {
        matches!(self, Kind::CdRw | Kind::DvdPlusRw | Kind::BdRe)
    }

    /// The sectors of a whole disc of the kind: a 700 MB CD, a 4.7 GB
    /// DVD, a 25 GB Blu-ray disc.
    pub fn sectors(self) -> u32 {
        match self {
            Kind::CdR | Kind::CdRw => 359_847,
            Kind::DvdR | Kind::DvdPlusR | Kind::DvdPlusRw => 2_295_104,
            Kind::BdR | Kind::BdRe => 12_219_392,
        }
    }
}

/// Where a disc written in sequence is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing written.
    Blank,
    /// A track written, and not closed yet.
    Open,
    /// Its track closed, its session not.
    TrackClosed,
    /// Closed: finished, nothing more is written.
    Closed,
}

/// The write parameters page's fields this drive keeps (MMC-6, 7.5.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Its byte 2: BUFE, test write and the write type (0 incremental, 1
    /// track at once, 2 session at once).
    pub flags: u8,
    /// Its byte 3: multi-session, and the track mode.
    pub track: u8,
    /// Its byte 4: the data block type (8, mode 1; 0, raw audio).
    pub block: u8,
}

impl Params {
    /// Whether writes are a test, which records nothing.
    pub fn test(self) -> bool {
        self.flags & 0x10 != 0
    }

    /// The write type.
    pub fn write_type(self) -> u8 {
        self.flags & 0x0f
    }
}

/// The write parameters page's length, after its first two bytes.
const PAGE_LENGTH: u8 = 0x32;

/// More sense codes, MMC-6 annex F's.
mod code {
    /// INVALID ADDRESS FOR WRITE.
    pub const INVALID_ADDRESS: (u8, u8) = (0x21, 0x02);
    /// CANNOT WRITE MEDIUM, INCOMPATIBLE FORMAT.
    pub const INCOMPATIBLE_FORMAT: (u8, u8) = (0x30, 0x05);
    /// INCOMPATIBLE MEDIUM INSTALLED.
    pub const INCOMPATIBLE_MEDIUM: (u8, u8) = (0x30, 0x00);
    /// PARAMETER LIST LENGTH ERROR.
    pub const LIST_LENGTH: (u8, u8) = (0x1a, 0x00);
    /// INVALID FIELD IN PARAMETER LIST.
    pub const INVALID_PARAMETER: (u8, u8) = (0x26, 0x00);
    /// SESSION FIXATION ERROR: no track, or no cue sheet, to close.
    pub const FIXATION: (u8, u8) = (0x71, 0x00);
}

fn illegal(code: (u8, u8)) -> Check {
    Check { key: sense::ILLEGAL_REQUEST, code }
}

/// A recordable disc: what kind it is, and what is written on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recordable {
    pub kind: Kind,
    /// The sectors it holds.
    pub capacity: u32,
    /// For a DVD+RW or BD-RE, whether it is formatted.
    pub formatted: bool,
    /// The write parameters, as MODE SELECT set them.
    pub params: Params,
    state: State,
    /// The cue sheet of a disc written at once, 8 bytes an entry.
    cue: Vec<[u8; 8]>,
    /// What was written: data sectors from block 0 of 2,048 bytes, or
    /// audio sectors from block -150 of 2,352 bytes, written at once.
    written: Vec<u8>,
    /// Whether `written` is audio.
    audio: bool,
    /// The tracks written and closed, which the drive reads.
    closed: Vec<Track>,
}

impl Recordable {
    /// A blank disc of `kind`, of `capacity` sectors (a test's disc is
    /// smaller than [`Kind::sectors`]); a DVD+RW or BD-RE unformatted.
    pub fn blank(kind: Kind, capacity: u32) -> Recordable {
        Recordable {
            kind,
            capacity,
            formatted: false,
            params: Params { flags: 0x41, track: 0x04, block: 0x08 },
            state: State::Blank,
            cue: Vec::new(),
            written: Vec::new(),
            audio: false,
            closed: Vec::new(),
        }
    }

    /// The tracks the drive reads: none while it is blank or not closed,
    /// but on a formatted DVD+RW or BD-RE, one over the whole disc.
    pub fn tracks(&self) -> &[Track] {
        &self.closed
    }

    /// The bytes of its data written, from block 0.
    pub fn data(&self) -> &[u8] {
        if self.audio { &[] } else { &self.written }
    }

    /// Sectors written in sequence, and so the next writable address.
    fn next(&self) -> u32 {
        let sector = if self.audio { RAW } else { DATA };
        (self.written.len() / sector) as u32
    }

    /// READ DISC INFORMATION's answer, of `length` bytes at most.
    pub fn disc_information(&self, length: usize) -> Vec<u8> {
        let mut data = vec![0u8; 34];
        data[1] = 32;
        let (status, last) = match (self.kind.random(), self.state) {
            (true, _) if self.formatted => (2, 3),
            (true, _) | (false, State::Blank) => (0, 0),
            (false, State::Open | State::TrackClosed) => (1, 1),
            (false, State::Closed) => (2, 3),
        };
        data[2] = (u8::from(self.kind.erasable()) << 4) | (last << 2) | status;
        let tracks = self.closed.len().max(1) as u8;
        data[3] = 1;
        data[4] = 1;
        data[5] = 1;
        data[6] = tracks;
        // A formatted DVD+RW's background format is complete.
        if self.kind == Kind::DvdPlusRw && self.formatted {
            data[7] = 0x03;
        }
        // Where the last session's lead-in starts, and where the last
        // lead-out could: the disc's end.
        data[16..20].copy_from_slice(&0u32.to_be_bytes());
        data[20..24].copy_from_slice(&self.capacity.to_be_bytes());
        data.truncate(length);
        data
    }

    /// READ TRACK INFORMATION of the track `number` (0xff, the one being
    /// written, the invisible track): of `length` bytes at most.
    pub fn track_information(&self, number: u32, length: usize) -> Result<Vec<u8>, Check> {
        let mut data = vec![0u8; 36];
        data[1] = 34;
        let written = self.next();
        // Each disc here has one track, from block 0.
        let (size, nwa) = match (number, self.state) {
            (_, _) if self.kind.random() => (self.capacity, None),
            (0xff | 1, State::Blank | State::Open) => (self.capacity, Some(written)),
            // Closed: what was written, and nothing writable.
            (0xff | 1, State::TrackClosed | State::Closed) => (written, None),
            _ => return Err(illegal(sense::INVALID_FIELD)),
        };
        data[2] = 1;
        data[3] = 1;
        data[5] = if self.audio { 0x00 } else { 0x04 };
        // Blank while nothing is written; data mode 1.
        data[6] = (u8::from(written == 0 && nwa.is_some()) << 6) | 0x01;
        data[7] = u8::from(nwa.is_some());
        data[8..12].copy_from_slice(&0u32.to_be_bytes());
        data[12..16].copy_from_slice(&nwa.unwrap_or(0).to_be_bytes());
        let free = if nwa.is_some() { self.capacity.saturating_sub(written) } else { 0 };
        data[16..20].copy_from_slice(&free.to_be_bytes());
        data[24..28].copy_from_slice(&size.to_be_bytes());
        data[28..32].copy_from_slice(&written.saturating_sub(1).to_be_bytes());
        data.truncate(length);
        Ok(data)
    }

    /// READ FORMAT CAPACITIES: the disc's capacity, formatted or not,
    /// and for a DVD+RW or BD-RE, the format it takes.
    pub fn format_capacities(&self, length: usize) -> Vec<u8> {
        let kind = if self.kind.random() && !self.formatted { 1 } else { 2 };
        let mut descriptors = vec![descriptor(self.capacity, kind)];
        match self.kind {
            Kind::DvdPlusRw => descriptors.push(descriptor(self.capacity, 0x26 << 2)),
            Kind::BdRe => {
                descriptors.push(descriptor(self.capacity, 0x00));
                descriptors.push(descriptor(self.capacity, 0x30 << 2));
            }
            _ => {}
        }
        let mut data = vec![0, 0, 0, (descriptors.len() * 8) as u8];
        data.extend(descriptors.concat());
        data.truncate(length);
        data
    }

    /// The write parameters page, as MODE SENSE gives it.
    pub fn page(&self) -> Vec<u8> {
        let mut page = vec![0u8; 2 + usize::from(PAGE_LENGTH)];
        page[0] = 0x05;
        page[1] = PAGE_LENGTH;
        page[2] = self.params.flags;
        page[3] = self.params.track;
        page[4] = self.params.block;
        // Two seconds of pause between audio tracks.
        page[14..16].copy_from_slice(&150u16.to_be_bytes());
        page
    }

    /// MODE SELECT(10) of the parameter list `list`: the write
    /// parameters page alone is taken.
    pub fn mode_select(&mut self, list: &[u8]) -> Result<(), Check> {
        let descriptors = list.get(6..8).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])));
        let page = descriptors.and_then(|d| list.get(8 + d..));
        let Some(page @ [code, length, ..]) = page else {
            return Err(illegal(code::LIST_LENGTH));
        };
        if code & 0x3f != 0x05 || page.len() < 2 + usize::from(*length) || *length < 3 {
            return Err(illegal(code::INVALID_PARAMETER));
        }
        let params = Params { flags: page[2], track: page[3], block: page[4] };
        let fits = match params.write_type() {
            // Incremental: a DVD-R.
            0 => self.kind == Kind::DvdR && params.block & 0x0f == 8,
            // Track at once: a CD of data.
            1 => self.kind.cd() && params.block & 0x0f == 8 && params.track & 0x0f == 4,
            // Session at once: a CD of audio.
            2 => self.kind.cd() && params.block & 0x0f == 0 && params.track & 0x0f == 0,
            _ => false,
        };
        if !fits {
            return Err(illegal(code::INVALID_PARAMETER));
        }
        self.params = params;
        Ok(())
    }

    /// SEND CUE SHEET: the layout of a CD written at once.
    pub fn cue_sheet(&mut self, sheet: &[u8]) -> Result<(), Check> {
        let (entries, rest) = sheet.as_chunks::<8>();
        if !self.kind.cd() || self.params.write_type() != 2 || self.state != State::Blank {
            return Err(illegal(code::INCOMPATIBLE_FORMAT));
        }
        if entries.len() < 3 || !rest.is_empty() {
            return Err(illegal(code::INVALID_PARAMETER));
        }
        self.cue = entries.to_vec();
        Ok(())
    }

    /// WRITE(10) of `data` at `lba`, `count` sectors.
    pub fn write(&mut self, lba: u32, count: u32, data: &[u8]) -> Result<(), Check> {
        if self.kind.random() {
            if !self.formatted {
                return Err(illegal(code::INCOMPATIBLE_FORMAT));
            }
            let end = u64::from(lba) + u64::from(count);
            if end > u64::from(self.capacity) || data.len() != count as usize * DATA {
                return Err(illegal(sense::OUT_OF_RANGE));
            }
            let (from, to) = (lba as usize * DATA, end as usize * DATA);
            if self.written.len() < to {
                self.written.resize(to, 0);
            }
            self.written[from..to].copy_from_slice(data);
            return Ok(());
        }
        if matches!(self.state, State::TrackClosed | State::Closed) {
            return Err(illegal(code::INCOMPATIBLE_FORMAT));
        }
        // A CD written at once starts at its first track's pregap, block
        // -150, and holds whole raw sectors.
        let audio = self.kind.cd() && self.params.write_type() == 2;
        if audio && self.cue.is_empty() {
            return Err(illegal(code::FIXATION));
        }
        let (sector, first) = if audio { (RAW, LEAD_IN.wrapping_neg()) } else { (DATA, 0) };
        let next = first.wrapping_add(self.written.len() as u32 / sector as u32);
        if lba != next {
            return Err(illegal(code::INVALID_ADDRESS));
        }
        if data.len() != count as usize * sector {
            return Err(illegal(sense::INVALID_FIELD));
        }
        if u64::from(self.next()) + u64::from(count) > u64::from(self.capacity) {
            return Err(illegal(sense::OUT_OF_RANGE));
        }
        self.audio = audio;
        self.written.extend_from_slice(data);
        self.state = State::Open;
        Ok(())
    }

    /// SYNCHRONIZE CACHE: what a DVD+RW or BD-RE was given is on it, and
    /// a CD written at once is finished, its lead-out written as its cue
    /// sheet says.
    pub fn synchronize(&mut self) {
        if self.kind.random() && self.formatted {
            self.closed = vec![data_track(self.capacity, self.written.clone())];
        }
        if self.audio && self.state == State::Open {
            self.finish_audio();
        }
    }

    /// CLOSE TRACK/SESSION, of close function `function`.
    pub fn close(&mut self, function: u8) -> Result<(), Check> {
        if self.kind.random() {
            return Ok(());
        }
        match (function, self.state) {
            // Close the track.
            (1, State::Open) => {
                self.state = State::TrackClosed;
                Ok(())
            }
            // Close the session, or finalize the disc: it is finished.
            (2 | 5 | 6, State::Open | State::TrackClosed) => {
                self.finish_data();
                Ok(())
            }
            (_, State::Closed) => Ok(()),
            _ => Err(illegal(code::FIXATION)),
        }
    }

    /// BLANK: a CD-RW erased.
    pub fn blank_disc(&mut self) -> Result<(), Check> {
        if !self.kind.blankable() {
            return Err(Check { key: sense::ILLEGAL_REQUEST, code: code::INCOMPATIBLE_MEDIUM });
        }
        *self = Recordable { params: self.params, ..Recordable::blank(self.kind, self.capacity) };
        Ok(())
    }

    /// FORMAT UNIT: a DVD+RW or BD-RE formatted, what it held gone.
    pub fn format(&mut self) -> Result<(), Check> {
        if !self.kind.random() {
            return Err(Check { key: sense::ILLEGAL_REQUEST, code: code::INCOMPATIBLE_MEDIUM });
        }
        self.formatted = true;
        self.written.clear();
        self.closed = vec![data_track(self.capacity, Vec::new())];
        Ok(())
    }

    /// The data written, closed into a track; a test write's left blank.
    fn finish_data(&mut self) {
        if self.params.test() {
            *self =
                Recordable { params: self.params, ..Recordable::blank(self.kind, self.capacity) };
            return;
        }
        self.closed = vec![data_track(self.next(), self.written.clone())];
        self.state = State::Closed;
    }

    /// The audio written at once, closed into the tracks its cue sheet
    /// starts with index 1; a test write's left blank.
    fn finish_audio(&mut self) {
        if self.params.test() {
            *self =
                Recordable { params: self.params, ..Recordable::blank(self.kind, self.capacity) };
            return;
        }
        let lead_out = self.next().saturating_sub(LEAD_IN);
        let mut starts: Vec<(u8, u32)> = Vec::new();
        for entry in &self.cue {
            let (number, index) = (entry[1], entry[2]);
            if (1..=99).contains(&number) && index == 1 {
                let msf = u32::from(entry[5]) * 60 * 75 + u32::from(entry[6]) * 75;
                starts.push((number, (msf + u32::from(entry[7])).saturating_sub(LEAD_IN)));
            }
        }
        let mut tracks = Vec::new();
        for (at, &(number, start)) in starts.iter().enumerate() {
            let end = starts.get(at + 1).map_or(lead_out, |&(_, next)| next);
            let from = (LEAD_IN + start) as usize * RAW;
            let to = ((LEAD_IN + end) as usize * RAW).min(self.written.len());
            tracks.push(Track {
                number,
                mode: Mode::Audio,
                start,
                sectors: end.saturating_sub(start),
                source: Source::Bytes {
                    bytes: self.written[from.min(to)..to].to_vec(),
                    sector: RAW,
                },
                pregap: 0,
                title: None,
                performer: None,
            });
        }
        self.closed = tracks;
        self.state = State::Closed;
    }
}

/// A track of data from block 0, of `sectors`, holding `bytes`.
fn data_track(sectors: u32, bytes: Vec<u8>) -> Track {
    Track {
        number: 1,
        mode: Mode::Data,
        start: 0,
        sectors,
        source: Source::Bytes { bytes, sector: DATA },
        pregap: 0,
        title: None,
        performer: None,
    }
}

/// A capacity descriptor of READ FORMAT CAPACITIES: `blocks` of 2,048
/// bytes, and its byte 4 (a type and format).
fn descriptor(blocks: u32, kind: u8) -> Vec<u8> {
    let mut out = blocks.to_be_bytes().to_vec();
    out.push(kind);
    out.extend([0, 0x08, 0x00]);
    out
}
