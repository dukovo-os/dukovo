// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The disc in the emulated drive: its tracks, where each starts, what
//! its sectors hold, and its CD-Text, read from a cue sheet and the files
//! it names, from an ISO image, or made by a test.
//!
//! Addresses are logical block addresses, as MMC gives them: block 0 is
//! the first track's start, two seconds (150 sectors) after the start of
//! the disc's program area, which is MSF 00:02:00.

use std::fmt;
use std::fs;
use std::path::Path;

use super::record::{Kind, Recordable};

/// The bytes of a CD sector, raw: an audio sector's 588 stereo frames of
/// 16-bit samples, or a data sector with its sync, header and error
/// codes.
pub const RAW: usize = 2352;
/// The bytes of a data sector's user data.
pub const DATA: usize = 2048;
/// Sectors a second, at the speed a CD plays.
pub const SECTORS_A_SECOND: u32 = 75;
/// The sectors before block 0: the first track's two-second pregap.
pub const LEAD_IN: u32 = 150;
/// The profile of a pressed CD (MMC-6, 5.3.1).
pub const CD_ROM: u16 = 0x0008;

/// What a track holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Audio: 2,352 bytes of samples a sector.
    Audio,
    /// Mode 1 data: 2,048 bytes of user data a sector.
    Data,
}

/// Where a track's sectors come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Bytes held: whole sectors, [`RAW`] bytes each for an audio track,
    /// and for a data track either [`DATA`] bytes of user data each or
    /// [`RAW`] bytes with the user data at byte 16.
    Bytes { bytes: Vec<u8>, sector: usize },
}

/// A track of the disc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Its number, from 1.
    pub number: u8,
    pub mode: Mode,
    /// Its first block, where INDEX 01 puts it.
    pub start: u32,
    /// How many sectors it has, to the next track's start or the
    /// lead-out.
    pub sectors: u32,
    /// Where its sectors come from, from its pregap's first if it has
    /// one in its file: the first `pregap` sectors of `source` come
    /// before `start`.
    pub source: Source,
    pub pregap: u32,
    /// CD-Text: its title and performer.
    pub title: Option<String>,
    pub performer: Option<String>,
}

/// A disc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disc {
    pub tracks: Vec<Track>,
    /// CD-Text of the disc as a whole: the album's title and performer.
    pub title: Option<String>,
    pub performer: Option<String>,
    /// A recordable disc's kind and what is written on it (ADR 0449);
    /// `None` for a pressed CD.
    pub record: Option<Recordable>,
}

/// Why a disc could not be read.
#[derive(Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a sector read gives.
#[derive(Debug, PartialEq, Eq)]
pub enum Sector<'a> {
    /// An audio sector's [`RAW`] bytes.
    Audio(&'a [u8]),
    /// A data sector's user data, [`DATA`] bytes.
    Data(&'a [u8]),
    /// A sector of no track's file: silence, or zeros.
    Empty(Mode),
}

impl Disc {
    /// A disc of one data track, the bytes of an ISO image: whole
    /// sectors of 2,048 bytes.
    pub fn iso(bytes: Vec<u8>) -> Result<Disc, Error> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(DATA) {
            return Err(Error("an ISO image is whole sectors of 2,048 bytes".into()));
        }
        let sectors = u32::try_from(bytes.len() / DATA).map_err(|_| Error("too big".into()))?;
        Ok(Disc {
            tracks: vec![Track {
                number: 1,
                mode: Mode::Data,
                start: 0,
                sectors,
                source: Source::Bytes { bytes, sector: DATA },
                pregap: 0,
                title: None,
                performer: None,
            }],
            title: None,
            performer: None,
            record: None,
        })
    }

    /// A blank disc of `kind`, of `sectors` (ADR 0449).
    pub fn blank(kind: Kind, sectors: u32) -> Disc {
        let record = Some(Recordable::blank(kind, sectors));
        Disc { tracks: Vec::new(), title: None, performer: None, record }
    }

    /// Its profile (MMC-6, 5.3.1): a pressed CD's, or a recordable
    /// disc's.
    pub fn profile(&self) -> u16 {
        self.record.as_ref().map_or(CD_ROM, |record| record.kind.profile())
    }

    /// The disc a file holds: a cue sheet (`.cue`) and the files it
    /// names, beside it, or an ISO image.
    pub fn open(path: &Path) -> Result<Disc, Error> {
        let read = |path: &Path| {
            fs::read(path).map_err(|err| Error(format!("cannot read {}: {err}", path.display())))
        };
        let is_cue = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cue"));
        if !is_cue {
            return Disc::iso(read(path)?);
        }
        let sheet = String::from_utf8(read(path)?)
            .map_err(|_| Error(format!("{} is not UTF-8", path.display())))?;
        let folder = path.parent().unwrap_or(Path::new("."));
        Disc::cue(&sheet, &mut |name| read(&folder.join(name)))
    }

    /// The disc a cue sheet describes, with `file` giving the bytes of
    /// each file it names. Takes what CDRWIN's sheets have that a disc of
    /// audio and data needs: FILE (BINARY, little-endian samples;
    /// MOTOROLA, big-endian; WAVE), TRACK (AUDIO, MODE1/2048,
    /// MODE1/2352), INDEX 00 and 01, PREGAP, and TITLE and PERFORMER for
    /// CD-Text. A track's sectors run to the next track's INDEX 00 (or
    /// 01) in the same file, or to the file's end.
    pub fn cue(
        sheet: &str,
        file: &mut dyn FnMut(&str) -> Result<Vec<u8>, Error>,
    ) -> Result<Disc, Error> {
        let mut disc = Disc { tracks: Vec::new(), title: None, performer: None, record: None };
        // Each track as the sheet gives it: its file's bytes (shared by
        // index), where its INDEX 00 and 01 are in the file, in sectors,
        // and a PREGAP's silence.
        struct Pending {
            number: u8,
            mode: Mode,
            sector: usize,
            file: usize,
            index0: Option<u32>,
            index1: Option<u32>,
            silence: u32,
            title: Option<String>,
            performer: Option<String>,
        }
        let mut files: Vec<Vec<u8>> = Vec::new();
        let mut pending: Vec<Pending> = Vec::new();
        for (number, line) in sheet.lines().enumerate() {
            let at = |what: &str| Error(format!("line {}: {what}", number + 1));
            let words = split(line.trim()).map_err(at)?;
            let Some(command) = words.first() else { continue };
            match command.to_ascii_uppercase().as_str() {
                "FILE" => {
                    let [_, name, kind] = words.as_slice() else {
                        return Err(at("FILE takes a name and a kind"));
                    };
                    let mut bytes = file(name)?;
                    match kind.to_ascii_uppercase().as_str() {
                        "BINARY" => {}
                        "MOTOROLA" => {
                            for pair in bytes.as_chunks_mut::<2>().0 {
                                pair.swap(0, 1);
                            }
                        }
                        "WAVE" => bytes = wave_samples(&bytes).map_err(at)?,
                        _ => return Err(at("a FILE is BINARY, MOTOROLA or WAVE")),
                    }
                    files.push(bytes);
                }
                "TRACK" => {
                    let [_, number, kind] = words.as_slice() else {
                        return Err(at("TRACK takes a number and a mode"));
                    };
                    let number: u8 = number.parse().map_err(|_| at("a track's number"))?;
                    let (mode, sector) = match kind.to_ascii_uppercase().as_str() {
                        "AUDIO" => (Mode::Audio, RAW),
                        "MODE1/2048" => (Mode::Data, DATA),
                        "MODE1/2352" => (Mode::Data, RAW),
                        _ => return Err(at("a track is AUDIO, MODE1/2048 or MODE1/2352")),
                    };
                    let expected = pending.last().map_or(1, |track| track.number + 1);
                    if number != expected || number > 99 {
                        return Err(at("tracks are numbered from 1, in order, up to 99"));
                    }
                    let file =
                        files.len().checked_sub(1).ok_or_else(|| at("a TRACK before any FILE"))?;
                    pending.push(Pending {
                        number,
                        mode,
                        sector,
                        file,
                        index0: None,
                        index1: None,
                        silence: 0,
                        title: None,
                        performer: None,
                    });
                }
                "INDEX" => {
                    let [_, index, time] = words.as_slice() else {
                        return Err(at("INDEX takes a number and a time"));
                    };
                    let track =
                        pending.last_mut().ok_or_else(|| at("an INDEX before any TRACK"))?;
                    let sectors = msf(time).ok_or_else(|| at("a time is mm:ss:ff"))?;
                    match index.parse::<u8>() {
                        Ok(0) => track.index0 = Some(sectors),
                        Ok(1) => track.index1 = Some(sectors),
                        // Later indexes mark places within the track.
                        Ok(_) => {}
                        Err(_) => return Err(at("an index's number")),
                    }
                }
                "PREGAP" => {
                    let [_, time] = words.as_slice() else { return Err(at("PREGAP takes a time")) };
                    let track =
                        pending.last_mut().ok_or_else(|| at("a PREGAP before any TRACK"))?;
                    track.silence = msf(time).ok_or_else(|| at("a time is mm:ss:ff"))?;
                }
                "TITLE" | "PERFORMER" => {
                    let [command, text] = words.as_slice() else {
                        return Err(at("TITLE and PERFORMER take one text"));
                    };
                    let text = Some(text.clone());
                    let title = command.eq_ignore_ascii_case("TITLE");
                    match (pending.last_mut(), title) {
                        (Some(track), true) => track.title = text,
                        (Some(track), false) => track.performer = text,
                        (None, true) => disc.title = text,
                        (None, false) => disc.performer = text,
                    }
                }
                // What a disc read back needs nothing of.
                "REM" | "CATALOG" | "ISRC" | "FLAGS" | "SONGWRITER" | "POSTGAP" | "CDTEXTFILE" => {}
                _ => return Err(at("not a command of a cue sheet")),
            }
        }
        if pending.is_empty() {
            return Err(Error("the cue sheet has no tracks".into()));
        }
        // Where each track's sectors are in its file, and where it ends.
        let mut start = 0u32;
        for (at, track) in pending.iter().enumerate() {
            let index1 = track
                .index1
                .ok_or_else(|| Error(format!("track {} has no INDEX 01", track.number)))?;
            let first = track.index0.unwrap_or(index1);
            if first > index1 {
                return Err(Error(format!(
                    "track {}'s INDEX 00 is after its INDEX 01",
                    track.number
                )));
            }
            let bytes = &files[track.file];
            let file_sectors = u32::try_from(bytes.len() / track.sector).unwrap_or(u32::MAX);
            // The next track in the same file starts where this one ends.
            let end = match pending.get(at + 1) {
                Some(next) if next.file == track.file => {
                    let next1 = next.index1.unwrap_or(file_sectors);
                    next.index0.unwrap_or(next1)
                }
                _ => file_sectors,
            };
            if end <= index1 {
                return Err(Error(format!("track {} has no sectors", track.number)));
            }
            let from = first as usize * track.sector;
            let to = (end as usize * track.sector).min(bytes.len());
            let pregap = index1 - first;
            // A PREGAP's silence and a pregap in the file both come before
            // the track's start; the first track's two seconds are the
            // disc's, before block 0.
            let before = track.silence + pregap;
            if at > 0 {
                start += before;
            } else if before > 0 {
                return Err(Error("the first track's pregap is the disc's own two seconds".into()));
            }
            let sectors = end - index1;
            disc.tracks.push(Track {
                number: track.number,
                mode: track.mode,
                start,
                sectors,
                source: Source::Bytes { bytes: bytes[from..to].to_vec(), sector: track.sector },
                pregap,
                title: track.title.clone(),
                performer: track.performer.clone(),
            });
            start += sectors;
        }
        Ok(disc)
    }

    /// The block after the disc's last: where its lead-out starts.
    pub fn lead_out(&self) -> u32 {
        self.tracks.last().map_or(0, |track| track.start + track.sectors)
    }

    /// The sector at block `lba`: `None` past the lead-out.
    pub fn sector(&self, lba: u32) -> Option<Sector<'_>> {
        let track = self.tracks.iter().find(|track| lba < track.start + track.sectors)?;
        // A block before the track's start is in its pregap: in its file,
        // or of a PREGAP's silence.
        let within = i64::from(lba) - i64::from(track.start) + i64::from(track.pregap);
        let Source::Bytes { bytes, sector } = &track.source;
        let Ok(within) = usize::try_from(within) else { return Some(Sector::Empty(track.mode)) };
        let Some(raw) = bytes.get(within * sector..(within + 1) * sector) else {
            return Some(Sector::Empty(track.mode));
        };
        Some(match (track.mode, *sector) {
            (Mode::Audio, _) => Sector::Audio(raw),
            (Mode::Data, DATA) => Sector::Data(raw),
            (Mode::Data, _) => Sector::Data(&raw[16..16 + DATA]),
        })
    }
}

/// The words of a line of a cue sheet: those in double quotes whole.
fn split(line: &str) -> Result<Vec<String>, &'static str> {
    let mut words = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find(|c: char| !c.is_whitespace()) {
        rest = &rest[start..];
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"').ok_or("a quote is not closed")?;
            words.push(quoted[..end].to_string());
            rest = &quoted[end + 1..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            words.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    Ok(words)
}

/// A cue sheet's time, `mm:ss:ff` (minutes, seconds and sectors), in
/// sectors.
fn msf(time: &str) -> Option<u32> {
    let mut parts = time.split(':').map(str::parse::<u32>);
    let (m, s, f) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    (parts.next().is_none() && s < 60 && f < SECTORS_A_SECOND)
        .then(|| (m * 60 + s) * SECTORS_A_SECOND + f)
}

/// The samples of a WAVE file of CD audio: 16-bit stereo PCM at 44.1 kHz.
fn wave_samples(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("a WAVE file starts RIFF ... WAVE");
    }
    let mut at = 12;
    let mut format_ok = false;
    while let Some(header) = bytes.get(at..at + 8) {
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let body = bytes.get(at + 8..at + 8 + len).ok_or("a WAVE chunk runs past the file")?;
        match &header[..4] {
            b"fmt " => {
                let word = |at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                format_ok = body.len() >= 16
                    && word(0) == 1
                    && word(2) == 2
                    && rate == 44_100
                    && word(14) == 16;
            }
            b"data" if format_ok => return Ok(body.to_vec()),
            b"data" => return Err("a WAVE file of a CD is 16-bit stereo PCM at 44,100 Hz"),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    Err("a WAVE file has no data")
}

/// Block `lba` as a time from the start of the program area, MSF:
/// minutes, seconds and sectors; the first track's pregap, from block
/// -150, is from 00:00:00.
pub fn to_msf(lba: u32) -> (u8, u8, u8) {
    let at = lba.wrapping_add(LEAD_IN);
    let (m, s, f) =
        (at / (60 * SECTORS_A_SECOND), (at / SECTORS_A_SECOND) % 60, at % SECTORS_A_SECOND);
    (m as u8, s as u8, f as u8)
}

/// CD-Text's packs (Red Book's annex of CD-Text, as MMC's READ TOC format
/// 5 gives them): 18 bytes each, a header of four, twelve of text and a
/// CRC. The disc's text and each track's, in ISO 8859-1, one block, in
/// English: the titles (pack type 0x80), the performers (0x81), and the
/// size information (0x8f) that says how many packs of each there are.
pub fn cd_text(disc: &Disc) -> Vec<[u8; 18]> {
    let mut packs = Vec::new();
    let mut counts = [0u8; 16];
    let first = disc.tracks.first().map_or(1, |track| track.number);
    let last = disc.tracks.last().map_or(1, |track| track.number);
    let titles: Vec<Option<&str>> = std::iter::once(disc.title.as_deref())
        .chain(disc.tracks.iter().map(|track| track.title.as_deref()))
        .collect();
    let performers: Vec<Option<&str>> = std::iter::once(disc.performer.as_deref())
        .chain(disc.tracks.iter().map(|track| track.performer.as_deref()))
        .collect();
    for (kind, texts) in [(0x80u8, &titles), (0x81, &performers)] {
        if texts.iter().all(Option::is_none) {
            continue;
        }
        let before = packs.len();
        text_packs(kind, first, texts, &mut packs);
        counts[usize::from(kind - 0x80)] = (packs.len() - before) as u8;
    }
    if packs.is_empty() {
        return packs;
    }
    // Three packs of size information close the block: the character
    // code (0, ISO 8859-1), the first and last track, copyright (0), the
    // count of packs of each type (0x80 to 0x8f), the last sequence number
    // of each of the eight blocks, and each block's language (0x09,
    // English).
    counts[15] = 3;
    let sequence = packs.len() as u8 + 2;
    let mut info = [0u8; 36];
    info[1] = first;
    info[2] = last;
    info[4..20].copy_from_slice(&counts);
    info[20] = sequence;
    info[28] = 0x09;
    for (n, chunk) in info.chunks(12).enumerate() {
        let mut pack = [0u8; 18];
        pack[0] = 0x8f;
        pack[1] = n as u8;
        pack[2] = packs.len() as u8;
        pack[4..16].copy_from_slice(chunk);
        packs.push(pack);
    }
    for pack in &mut packs {
        let crc = !crc16(&pack[..16]);
        pack[16..].copy_from_slice(&crc.to_be_bytes());
    }
    packs
}

/// The packs of `kind` holding `texts`: the disc's (track 0) then each
/// track's from `first`, each ended by a zero, run together twelve bytes a
/// pack. Each pack names the track of its first byte, and how many of
/// that track's characters came before it (at most 15).
fn text_packs(kind: u8, first: u8, texts: &[Option<&str>], packs: &mut Vec<[u8; 18]>) {
    let mut bytes: Vec<(u8, u8)> = Vec::new();
    for (n, text) in texts.iter().enumerate() {
        let track = if n == 0 { 0 } else { first + n as u8 - 1 };
        let latin: Vec<u8> =
            text.unwrap_or("").chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect();
        bytes.extend(latin.iter().chain(&[0]).map(|&byte| (track, byte)));
    }
    // Where each byte's track's text started.
    let mut position = Vec::with_capacity(bytes.len());
    let mut run = 0u32;
    for (n, &(track, _)) in bytes.iter().enumerate() {
        if n > 0 && bytes[n - 1].0 != track {
            run = 0;
        }
        position.push(run);
        run += 1;
    }
    for (chunk, start) in bytes.chunks(12).zip((0..bytes.len()).step_by(12)) {
        let mut pack = [0u8; 18];
        pack[0] = kind;
        pack[1] = chunk[0].0;
        pack[2] = packs.len() as u8;
        pack[3] = position[start].min(15) as u8;
        for (to, &(_, byte)) in pack[4..16].iter_mut().zip(chunk) {
            *to = byte;
        }
        packs.push(pack);
    }
}

/// CRC-16 with the polynomial x^16 + x^12 + x^5 + 1, from 0, as CD-Text's
/// packs and the Q subchannel carry it (inverted).
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}
