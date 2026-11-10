// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A music CD's tracks and their names, from what an optical drive's
//! READ TOC answers (radix-block(2)'s READ_TOC, ADR 0447), for audiocd(1).
//!
//! - [`tracks`] reads the table of contents: each track's number, whether
//!   it holds audio, and the sectors it spans, up to the next track or
//!   the lead-out. The last audio track before a data track that starts a
//!   later session (an Enhanced CD, or CD-Extra) ends 11,400 sectors
//!   before it, where the first session's lead-out and the next one's
//!   lead-in and pregap lie.
//! - [`cd_text`] reads the CD-Text: the disc's and each track's title and
//!   performer, in ISO 8859-1, from its first block.
//!
//! Sectors of audio are [`SECTOR`] bytes: 588 frames of two 16-bit
//! samples, left then right, little-endian, at 44,100 frames a second.

use std::collections::BTreeMap;

/// The bytes of a sector of audio.
pub const SECTOR: usize = 2352;
/// Sectors a second.
pub const SECTORS_PER_SECOND: u32 = 75;
/// Frames of samples a second.
pub const RATE: u32 = 44_100;
/// Channels: a CD is stereo.
pub const CHANNELS: u8 = 2;

/// The sectors between the last audio track of a disc's first session and
/// a data track that starts the next: the first session's lead-out, and
/// the next one's lead-in and pregap.
const SESSION_GAP: u32 = 11_400;
/// The track number of the lead-out in a table of contents.
const LEAD_OUT: u8 = 0xaa;
/// A track's control bit that says it holds data.
const DATA: u8 = 0x04;

/// A track of a disc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Track {
    /// Its number, 1 to 99.
    pub number: u8,
    /// Whether it holds audio, not data.
    pub audio: bool,
    /// Its first sector's block number.
    pub start: u32,
    /// How many sectors it spans.
    pub sectors: u32,
}

impl Track {
    /// How long it plays, in whole seconds, rounded.
    pub fn seconds(&self) -> u32 {
        (self.sectors + SECTORS_PER_SECOND / 2) / SECTORS_PER_SECOND
    }

    /// How many frames of samples it holds.
    pub fn frames(&self) -> u64 {
        u64::from(self.sectors) * (SECTOR / 4) as u64
    }
}

/// The tracks a READ TOC of the tracks (format 0) answered, `toc`, given
/// the answer to one of the sessions (format 1), `sessions`, if the drive
/// gave one.
pub fn tracks(toc: &[u8], sessions: Option<&[u8]>) -> Result<Vec<Track>, &'static str> {
    let length = answer_length(toc).ok_or("the drive's table of contents is too short")?;
    let entries: Vec<(u8, u8, u32)> = toc[4..length]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|d| (d[2], d[1] & 0x0f, u32::from_be_bytes([d[4], d[5], d[6], d[7]])))
        .collect();
    let lead_out =
        entries.iter().find(|(number, _, _)| *number == LEAD_OUT).map(|&(_, _, start)| start);
    let lead_out = lead_out.ok_or("the drive's table of contents has no lead-out")?;
    // The first track of the last session, if the disc has more than one.
    let later_session = sessions
        .and_then(|answer| Some((answer_length(answer)?, answer)))
        .filter(|(length, answer)| *length >= 12 && answer[3] > 1)
        .map(|(_, answer)| answer[6]);
    let listed: Vec<&(u8, u8, u32)> =
        entries.iter().filter(|(number, _, _)| *number != LEAD_OUT).collect();
    let mut tracks = Vec::with_capacity(listed.len());
    for (i, &&(number, control, start)) in listed.iter().enumerate() {
        let audio = control & DATA == 0;
        let end = match listed.get(i + 1) {
            Some(&&(next, next_control, next_start)) => {
                let new_session = later_session == Some(next) && next_control & DATA != 0;
                if audio && new_session {
                    next_start.saturating_sub(SESSION_GAP)
                } else {
                    next_start
                }
            }
            None => lead_out,
        };
        if end < start {
            return Err("the drive's table of contents has tracks out of order");
        }
        tracks.push(Track { number, audio, start, sectors: end - start });
    }
    Ok(tracks)
}

/// The bytes of a READ TOC answer that are its own: its first two bytes
/// count those after them, and a drive cuts the answer to the buffer.
fn answer_length(answer: &[u8]) -> Option<usize> {
    let said = usize::from(u16::from_be_bytes([*answer.first()?, *answer.get(1)?])) + 2;
    Some(said.min(answer.len())).filter(|&length| length >= 4)
}

/// A title and a performer, either of which a disc may not give.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    /// The title: the disc's, or the track's.
    pub title: Option<String>,
    /// Who performs it.
    pub performer: Option<String>,
}

/// What a disc's CD-Text says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CdText {
    /// The disc's own.
    pub disc: Names,
    /// Each track's, by number.
    pub tracks: BTreeMap<u8, Names>,
}

impl CdText {
    /// What it says of track `number`: nothing if it says nothing.
    pub fn track(&self, number: u8) -> Names {
        self.tracks.get(&number).cloned().unwrap_or_default()
    }
}

/// The pack types of the title and the performer, and of the size
/// information, whose first pack's first byte is the character code.
const TITLE: u8 = 0x80;
const PERFORMER: u8 = 0x81;
const SIZE_INFORMATION: u8 = 0x8f;
/// The character codes of ISO 8859-1, and of ASCII; the third, MS-JIS, of
/// two bytes, is not read.
const LATIN_1: u8 = 0x00;
const ASCII: u8 = 0x01;

/// What the READ TOC of CD-Text (format 5) answered, `answer`, says: the
/// titles and performers of its first block, in packs whose CRC holds (or
/// that a drive gave with none). A disc without CD-Text, or one of a
/// character code this does not read, says nothing.
pub fn cd_text(answer: &[u8]) -> CdText {
    let mut text = CdText::default();
    let Some(length) = answer_length(answer) else { return text };
    let packs: Vec<&[u8; 18]> = answer[4..length]
        .as_chunks::<18>()
        .0
        .iter()
        .filter(|pack| {
            let stored = u16::from_be_bytes([pack[16], pack[17]]);
            stored == 0 || stored == !crc16(&pack[..16])
        })
        // The first block, of characters of one byte.
        .filter(|pack| pack[3] & 0xf0 == 0)
        .collect();
    let code =
        packs.iter().find(|pack| pack[0] == SIZE_INFORMATION && pack[1] == 0).map(|pack| pack[4]);
    if !matches!(code, None | Some(LATIN_1 | ASCII)) {
        return text;
    }
    for kind in [TITLE, PERFORMER] {
        let mut of_kind: Vec<&&[u8; 18]> = packs.iter().filter(|pack| pack[0] == kind).collect();
        of_kind.sort_by_key(|pack| pack[2]);
        let Some(first) = of_kind.first() else { continue };
        // The text runs on from pack to pack, each string ended by a zero:
        // the first string is the first pack's track's, and each after it
        // the next track's. A tab alone is the one before it again.
        let mut track = first[1] & 0x7f;
        let mut string: Vec<u8> = Vec::new();
        let mut previous: Option<String> = None;
        for &byte in of_kind.iter().flat_map(|pack| &pack[4..16]) {
            if byte != 0 {
                string.push(byte);
                continue;
            }
            let value = match string.as_slice() {
                b"\t" => previous.clone(),
                b"" => None,
                bytes => Some(bytes.iter().map(|&b| char::from(b)).collect::<String>()),
            };
            if let Some(value) = &value {
                let names =
                    if track == 0 { &mut text.disc } else { text.tracks.entry(track).or_default() };
                let slot = if kind == TITLE { &mut names.title } else { &mut names.performer };
                *slot = Some(value.trim().to_string());
            }
            previous = value;
            string.clear();
            track = track.saturating_add(1);
        }
    }
    text
}

/// CRC-16 of polynomial x^16 + x^12 + x^5 + 1, from 0, as a CD-Text pack
/// holds it inverted.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}
