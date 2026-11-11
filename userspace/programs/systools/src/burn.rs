// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! burn(1): writes CDs, DVDs and Blu-ray discs (ADR 0449).
//!
//! burn sends the drive the MMC-6 commands that write a disc, through
//! radix-block(2)'s MMC_COMMAND, which passes those and no others: it asks
//! what disc is in the drive and how it is written ([`Disc::inspect`]),
//! blanks or formats a rewritable one, sets the write parameters, writes,
//! and closes the track and the session. Each kind of disc is written its
//! own way ([`Way`]): a CD of data a track at a time (TAO), a CD of audio
//! all at once from a cue sheet (SAO), a DVD-R incrementally, a DVD+R or
//! BD-R in sequence and finalized, and a DVD+RW, DVD-RAM or BD-RE
//! formatted and written in place. Buffer underrun protection is asked
//! for wherever the write parameters page has it.
//!
//! What is burned is an image, a folder made into one as it is written
//! (`disk::disc`: ISO 9660 with Rock Ridge and Joliet, and a UDF bridge
//! with `--udf`), or, with `--audio`, WAV and FLAC files, a track each.
//! mkiso(1) makes an image of a folder without burning it ([`mkiso`]).
//!
//! The kernel holds the drive while burn writes: its disc is not looked
//! at, and its tray stays shut. Once the disc is written, burn has the
//! kernel look at it afresh (REREAD_PARTITIONS), and volumed(8) mounts
//! it, or ejects it (EJECT). Start at [`main`].

#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use audio::{convert, flac, resample, wav};
use disk::disc::{Image, Node, Options};
use radix_abi::Status;
use radix_abi::block;
use radix_abi::errors::{File as FileError, Kernel};

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::probe;
use crate::sys;
use crate::util::{self, Fail, fail, usage};

const USAGE: &str =
    "usage: burn [--drive NAME] [--blank] [--test] [--eject] [--label LABEL] [--udf] IMAGE|FOLDER
       burn --audio [--drive NAME] [--blank] [--test] [--eject] FILE...
       burn blank [--drive NAME] [--full]
       burn info [--drive NAME]";

/// Where the kernel says what each disk is.
const BLOCKS: &str = "/kernel/block";
/// The bytes of a disc's block of data.
const BLOCK: usize = 2048;
/// The bytes of a CD's sector of audio.
const AUDIO_SECTOR: usize = 2352;
/// The sectors before a CD's block 0: its first track's pregap.
const PREGAP: u32 = 150;
/// The fewest sectors of a track of audio: four seconds, as the Red Book
/// wants.
const SHORTEST_TRACK: usize = 4 * 75;
/// The longest a write waits for room in the drive's buffer.
const BUFFER_WAIT: Duration = Duration::from_secs(60);
/// The longest a blank, a format or a close is waited for.
const LONG_WAIT: Duration = Duration::from_mins(90);

/// The commands burn sends, by opcode (MMC-6).
mod op {
    pub const TEST_UNIT_READY: u8 = 0x00;
    pub const REQUEST_SENSE: u8 = 0x03;
    pub const FORMAT_UNIT: u8 = 0x04;
    pub const READ_FORMAT_CAPACITIES: u8 = 0x23;
    pub const WRITE_10: u8 = 0x2a;
    pub const SYNCHRONIZE_CACHE: u8 = 0x35;
    pub const GET_CONFIGURATION: u8 = 0x46;
    pub const READ_DISC_INFORMATION: u8 = 0x51;
    pub const READ_TRACK_INFORMATION: u8 = 0x52;
    pub const MODE_SELECT_10: u8 = 0x55;
    pub const MODE_SENSE_10: u8 = 0x5a;
    pub const CLOSE_TRACK_SESSION: u8 = 0x5b;
    pub const SEND_CUE_SHEET: u8 = 0x5d;
    pub const BLANK: u8 = 0xa1;
}

/// Why a command failed: the drive's sense key, code and qualifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sense(pub u8, pub u8, pub u8);

impl Sense {
    /// Whether the drive is busy with what it was given before: a long
    /// write, a blank, a format or a close in progress (NOT READY, 04h).
    fn busy(self) -> bool {
        self.0 == 0x02 && self.1 == 0x04 && matches!(self.2, 0x01 | 0x04 | 0x07 | 0x08)
    }

    /// What it says, in words, for the codes a burn meets most.
    fn words(self) -> String {
        let words = match (self.1, self.2) {
            (0x04, 0x08) => "a long write is in progress",
            (0x0c, _) => "write error",
            (0x21, 0x02) => "not where the disc is written next",
            (0x24, 0x00) => "the drive refused a field of the command",
            (0x26, 0x00) => "the drive refused the write parameters",
            (0x27, _) => "the disc is write-protected",
            (0x30, _) => "the drive cannot write this disc that way",
            (0x3a, _) => "no disc is in the drive",
            (0x64, _) => "the drive does not write this kind of track",
            (0x72, _) => "the disc is closed",
            (0x73, _) => "the disc's power calibration area is full",
            _ => "the drive failed the command",
        };
        format!("{words} (sense {:x}/{:02x}/{:02x})", self.0, self.1, self.2)
    }
}

/// An optical drive, opened.
struct Drive {
    name: String,
    device: File,
}

/// What a command's data is.
enum Data<'a> {
    None,
    /// From the drive, into the buffer.
    In(&'a mut [u8]),
    /// To the drive, from the buffer.
    Out(&'a [u8]),
}

impl Drive {
    /// Runs `cdb` with `data`, giving the drive `seconds` (0 for 30):
    /// the bytes that moved, or the drive's sense.
    fn command(
        &self,
        cdb: &[u8],
        data: Data<'_>,
        seconds: u32,
    ) -> Result<Result<u32, Sense>, Fail> {
        let mut request = [0u8; block::MMC_COMMAND_SIZE as usize];
        request[..cdb.len()].copy_from_slice(cdb);
        request[16..20].copy_from_slice(&seconds.to_le_bytes());
        // The kernel moves the data while the request runs, through the
        // buffer's address, as read(2) does.
        let (address, length) = match data {
            Data::None => (0, 0),
            Data::In(buffer) => (buffer.as_mut_ptr() as u64, buffer.len()),
            Data::Out(buffer) => (buffer.as_ptr() as u64, buffer.len()),
        };
        request[24..32].copy_from_slice(&address.to_le_bytes());
        request[32..40].copy_from_slice(&(length as u64).to_le_bytes());
        blockdev::control(&self.device, block::MMC_COMMAND, &mut request)
            .map_err(|status| fail(why(&self.name, status)))?;
        let moved = u32::from_le_bytes([request[40], request[41], request[42], request[43]]);
        let sense = Sense(request[44], request[45], request[46]);
        Ok(if sense.0 == 0 && sense.1 == 0 { Ok(moved) } else { Err(sense) })
    }

    /// Runs `cdb`, a command of `what`, failing with the drive's sense.
    fn run(&self, what: &str, cdb: &[u8], data: Data<'_>, seconds: u32) -> Result<u32, Fail> {
        self.command(cdb, data, seconds)?
            .map_err(|sense| fail(format!("{}: {what}: {}", self.name, sense.words())))
    }

    /// Asks with `cdb` for an answer of up to `length` bytes.
    fn ask(&self, what: &str, cdb: &[u8], length: usize) -> Result<Vec<u8>, Fail> {
        let mut answer = vec![0u8; length];
        let came = self.run(what, cdb, Data::In(&mut answer), 0)?;
        answer.truncate(came as usize);
        Ok(answer)
    }

    /// Waits while the drive is busy with a blank, a format or a close
    /// asked to answer at once (Immed), for up to `limit`.
    fn wait_ready(&self, what: &str, limit: Duration) -> Result<(), Fail> {
        let start = Instant::now();
        loop {
            match self.command(&[op::TEST_UNIT_READY, 0, 0, 0, 0, 0], Data::None, 0)? {
                Ok(_) => return Ok(()),
                Err(sense) if sense.busy() && start.elapsed() < limit => {
                    // REQUEST SENSE says how far it has come, which the
                    // drive keeps for those who ask.
                    let mut sensed = [0u8; 18];
                    let cdb = [op::REQUEST_SENSE, 0, 0, 0, 18, 0];
                    let _ = self.command(&cdb, Data::In(&mut sensed), 0)?;
                    std::thread::sleep(Duration::from_secs(1));
                }
                // A unit attention after a blank or a format is the disc
                // the drive says changed: it is ready.
                Err(Sense(0x06, _, _)) => {}
                Err(sense) => {
                    return Err(fail(format!("{}: {what}: {}", self.name, sense.words())));
                }
            }
        }
    }
}

/// What a failed request on `drive` means, in words.
fn why(drive: &str, status: Status) -> String {
    match (FileError::from_status(status), Kernel::from_status(status)) {
        (_, Some(Kernel::NotSupported)) => format!("{drive} is not an optical drive"),
        (_, Some(Kernel::MissingRights)) => format!("{drive}: burning needs the write right"),
        (Some(FileError::InUse), _) => format!("{drive} is in use"),
        _ => format!("{drive}: {}", util::strip_os_error(&sys::io_error(status))),
    }
}

/// How a kind of disc is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// A CD: a track at a time with data, at once with audio; a CD-RW is
    /// blanked first.
    Cd { rewritable: bool },
    /// A DVD-R, or a DVD-RW in sequential recording: incrementally; a
    /// DVD-RW is blanked first.
    Incremental { rewritable: bool },
    /// A DVD+R, DVD+R DL or BD-R: in sequence, then finalized.
    Sequential,
    /// A DVD+RW, a DVD-RW in restricted overwrite, a DVD-RAM or a BD-RE:
    /// formatted if it is not, and written in place.
    Overwrite,
}

impl Way {
    /// How a disc of profile `profile` (MMC-6, 5.3.1) is written; `None`
    /// for one that is not written.
    pub fn of(profile: u16) -> Option<Way> {
        Some(match profile {
            0x09 => Way::Cd { rewritable: false },
            0x0a => Way::Cd { rewritable: true },
            0x11 | 0x15 | 0x16 => Way::Incremental { rewritable: false },
            0x14 => Way::Incremental { rewritable: true },
            0x1b | 0x2b | 0x41 => Way::Sequential,
            0x12 | 0x13 | 0x1a | 0x2a | 0x43 => Way::Overwrite,
            _ => return None,
        })
    }

    /// Whether the write parameters page is set for it, and so whether it
    /// has a test write.
    fn has_page(self) -> bool {
        matches!(self, Way::Cd { .. } | Way::Incremental { .. })
    }
}

/// The name of a disc of profile `profile`, as people say it.
pub fn profile_name(profile: u16) -> &'static str {
    match profile {
        0x09 => "CD-R",
        0x0a => "CD-RW",
        0x11 => "DVD-R",
        0x12 => "DVD-RAM",
        0x13 | 0x14 => "DVD-RW",
        0x15 | 0x16 => "DVD-R DL",
        0x1a => "DVD+RW",
        0x1b => "DVD+R",
        0x2a => "DVD+RW DL",
        0x2b => "DVD+R DL",
        0x41 | 0x42 => "BD-R",
        0x43 => "BD-RE",
        0x08 => "CD-ROM",
        0x10 => "DVD-ROM",
        0x40 => "BD-ROM",
        _ => "disc",
    }
}

/// What READ DISC INFORMATION says of a disc (MMC-6, 6.22.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscInformation {
    /// 0 blank, 1 appendable, 2 complete, 3 other.
    pub status: u8,
    /// Whether it is erased with BLANK or written again.
    pub erasable: bool,
}

impl DiscInformation {
    /// Reads `bytes`; `None` if they are too short.
    pub fn parse(bytes: &[u8]) -> Option<DiscInformation> {
        let byte = *bytes.get(2)?;
        Some(DiscInformation { status: byte & 3, erasable: byte & 0x10 != 0 })
    }
}

/// What READ TRACK INFORMATION says of the track being written (MMC-6,
/// 6.27.3): where it is written next, and the blocks free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackInformation {
    pub number: u16,
    pub next: Option<u32>,
    pub free: u32,
}

impl TrackInformation {
    /// Reads `bytes`; `None` if they are too short.
    pub fn parse(bytes: &[u8]) -> Option<TrackInformation> {
        let long = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        let number = u16::from_be_bytes([bytes.get(32).copied().unwrap_or(0), *bytes.get(2)?]);
        let valid = bytes.get(7)? & 1 != 0;
        Some(TrackInformation { number, next: valid.then_some(long(12)?), free: long(16)? })
    }
}

/// What READ FORMAT CAPACITIES says of a DVD+RW or BD-RE: whether it is
/// formatted, its blocks, and the format to give it (a descriptor's
/// blocks and type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacities {
    pub formatted: bool,
    pub blocks: u32,
    pub format: Option<(u32, u8)>,
}

impl Capacities {
    /// Reads `bytes`, for a disc of profile `profile`: its current
    /// capacity, and of the formats it lists, a DVD+RW's (26h), or a BD-RE's
    /// with spare areas (30h) or else without (00h); `None` if too short.
    pub fn parse(bytes: &[u8], profile: u16) -> Option<Capacities> {
        let length = usize::from(*bytes.get(3)?);
        let list = bytes.get(4..4 + length.min(bytes.len().saturating_sub(4)))?;
        let (descriptors, _) = list.as_chunks::<8>();
        let current = descriptors.first()?;
        let blocks = u32::from_be_bytes(current[..4].try_into().ok()?);
        let formattable: Vec<(u32, u8)> = descriptors[1..]
            .iter()
            .map(|d| (u32::from_be_bytes([d[0], d[1], d[2], d[3]]), d[4] >> 2))
            .collect();
        let wanted: &[u8] = match profile {
            0x1a | 0x2a => &[0x26],
            0x43 => &[0x30, 0x00],
            0x12 => &[0x00],
            0x13 => &[0x15, 0x13, 0x10],
            _ => &[],
        };
        let format = wanted.iter().find_map(|w| formattable.iter().find(|f| f.1 == *w)).copied();
        Some(Capacities { formatted: current[4] & 3 == 2, blocks, format })
    }
}

/// The disc in a drive, as burn found it.
struct Disc {
    profile: u16,
    way: Way,
    information: DiscInformation,
}

impl Disc {
    /// What disc is in `drive`, and how it is written.
    fn inspect(drive: &Drive) -> Result<Disc, Fail> {
        let configuration = drive.ask(
            "GET CONFIGURATION",
            &[op::GET_CONFIGURATION, 0, 0, 0, 0, 0, 0, 0, 8, 0],
            8,
        )?;
        let profile = configuration.get(6..8).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]));
        if profile == 0 {
            return Err(fail(format!("{}: no disc is in the drive", drive.name)));
        }
        let way = Way::of(profile).ok_or_else(|| {
            fail(format!("{}: a {} is not written", drive.name, profile_name(profile)))
        })?;
        let cdb = [op::READ_DISC_INFORMATION, 0, 0, 0, 0, 0, 0, 0, 34, 0];
        let bytes = drive.ask("READ DISC INFORMATION", &cdb, 34)?;
        let information = DiscInformation::parse(&bytes).ok_or_else(|| {
            fail(format!("{}: the drive did not say what disc it holds", drive.name))
        })?;
        Ok(Disc { profile, way, information })
    }

    fn name(&self) -> &'static str {
        profile_name(self.profile)
    }

    /// Whether it is blank, or for one written in place, formatted.
    fn blank(&self) -> bool {
        self.information.status == 0
    }
}

/// The track being written on `drive`'s disc.
fn invisible_track(drive: &Drive) -> Result<TrackInformation, Fail> {
    let cdb = [op::READ_TRACK_INFORMATION, 1, 0, 0, 0, 0xff, 0, 0, 36, 0];
    let bytes = drive.ask("READ TRACK INFORMATION", &cdb, 36)?;
    TrackInformation::parse(&bytes)
        .ok_or_else(|| fail(format!("{}: the drive did not say where to write", drive.name)))
}

/// The capacities of `drive`'s disc of `profile`.
fn capacities(drive: &Drive, profile: u16) -> Result<Capacities, Fail> {
    let cdb = [op::READ_FORMAT_CAPACITIES, 0, 0, 0, 0, 0, 0, 0, 252, 0];
    let bytes = drive.ask("READ FORMAT CAPACITIES", &cdb, 252)?;
    Capacities::parse(&bytes, profile)
        .ok_or_else(|| fail(format!("{}: the drive did not say the disc's capacity", drive.name)))
}

/// What is written: data, or audio at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Data,
    Audio,
}

/// The MODE SELECT(10) parameter list that sets the write parameters
/// page, made of `sensed`, MODE SENSE(10)'s answer for it: for `content`
/// written the `way` it is, with buffer underrun protection, as a test
/// write if `test`, closing the disc when the session is. `None` if the
/// answer holds no such page.
pub fn write_parameters(sensed: &[u8], way: Way, content: Content, test: bool) -> Option<Vec<u8>> {
    let descriptors = usize::from(u16::from_be_bytes([*sensed.get(6)?, *sensed.get(7)?]));
    let page = sensed.get(8 + descriptors..)?;
    let length = usize::from(*page.get(1)?);
    if page[0] & 0x3f != 0x05 || length < 0x0e || page.len() < 2 + length {
        return None;
    }
    let mut page = page[..2 + length].to_vec();
    // The PS bit is MODE SENSE's to say, and MODE SELECT's to clear.
    page[0] = 0x05;
    let (write_type, track_mode, block_type) = match (way, content) {
        // Track at once, data uninterrupted, mode 1.
        (Way::Cd { .. }, Content::Data) => (1, 4, 8),
        // Session at once, two channels of audio, raw sectors.
        (Way::Cd { .. }, Content::Audio) => (2, 0, 0),
        // Incremental, data, mode 1.
        _ => (0, 5, 8),
    };
    // BUFE, link size valid (none), test write, and the write type.
    page[2] = 0x40 | (u8::from(test) << 4) | write_type;
    // No next session: the disc is closed with this one; fixed packets
    // off; the track mode.
    page[3] = track_mode;
    page[4] = block_type;
    page[5] = 0;
    // CD-DA or CD-ROM, as the session format.
    page[8] = 0;
    let mut list = vec![0u8; 8];
    list.extend(page);
    Some(list)
}

/// Block `lba` as minutes, seconds and frames from the start of a CD's
/// program area, two seconds before block 0.
pub fn msf(lba: u32) -> [u8; 3] {
    let at = lba.wrapping_add(PREGAP);
    [(at / (60 * 75)) as u8, ((at / 75) % 60) as u8, (at % 75) as u8]
}

/// The cue sheet of a CD of audio of tracks of `sectors` each, written at
/// once (MMC-6, 6.33): the lead-in, the first track's two seconds of
/// pregap from block -150, each track's start, and the lead-out, the
/// drive's own.
pub fn cue_sheet(sectors: &[u32]) -> Vec<u8> {
    // Control 0 (two channels of audio, no copy), address 1 (the Q
    // channel's position); data form 0 is the host's audio, 1 the
    // drive's silence.
    let entry = |track: u8, index: u8, form: u8, lba: u32| {
        let [m, s, f] = msf(lba);
        [0x01, track, index, form, 0, m, s, f]
    };
    let mut sheet = Vec::new();
    sheet.extend(entry(0, 0, 0x01, PREGAP.wrapping_neg()));
    sheet.extend(entry(1, 0, 0x00, PREGAP.wrapping_neg()));
    let mut at = 0u32;
    for (number, &length) in sectors.iter().enumerate() {
        sheet.extend(entry(number as u8 + 1, 1, 0x00, at));
        at += length;
    }
    sheet.extend(entry(0xaa, 1, 0x01, at));
    sheet
}

/// The samples of the WAV or FLAC file `path`, as a CD holds them: two
/// channels of 16 bits at 44,100 frames a second, little-endian, padded
/// with silence to whole sectors and to four seconds at least.
fn audio_track(path: &Path) -> Result<Vec<u8>, Fail> {
    let bytes = std::fs::read(path).map_err(|e| util::failed(&path.display(), e))?;
    let named = |why: String| fail(format!("{}: {why}", path.display()));
    let (samples, rate, channels) = if bytes.starts_with(b"fLaC") {
        let decoded = flac::decode(&bytes).map_err(|e| named(e.to_string()))?;
        let channels = usize::from(decoded.info.channels);
        (decoded.samples, decoded.info.rate, channels)
    } else {
        let parsed = wav::parse(&bytes).map_err(|e| named(e.to_string()))?;
        let mut samples = Vec::new();
        convert::to_s16(parsed.data, parsed.encoding, &mut samples);
        (samples, parsed.rate, usize::from(parsed.channels))
    };
    let stereo = convert::channels(&samples, channels, 2);
    let at_cd_rate =
        if rate == 44_100 { stereo } else { resample::resample(&stereo, rate, 44_100, 2) };
    Ok(cd_sectors(&convert::to_bytes(&at_cd_rate)))
}

/// `bytes` of samples padded with silence to whole sectors of audio and
/// to four seconds at least.
pub fn cd_sectors(bytes: &[u8]) -> Vec<u8> {
    let sectors = bytes.len().div_ceil(AUDIO_SECTOR).max(SHORTEST_TRACK);
    let mut out = bytes.to_vec();
    out.resize(sectors * AUDIO_SECTOR, 0);
    out
}

/// Writes to the disc in `drive` from block `next` on, in commands of as
/// many whole sectors of `sector` bytes as MMC_COMMAND moves, waiting
/// while the drive's buffer is full; the last sector is padded with
/// zeros.
struct Writer<'a> {
    drive: &'a Drive,
    sector: usize,
    next: u32,
    pending: Vec<u8>,
    written: u64,
    /// Sectors to write in all, for the progress shown.
    total: u64,
    shown: u64,
    progress: bool,
}

impl<'a> Writer<'a> {
    fn new(drive: &'a Drive, next: u32, sector: usize, total: u64, progress: bool) -> Writer<'a> {
        let pending = Vec::with_capacity(block::MMC_COMMAND_MAX as usize);
        Writer { drive, sector, next, pending, written: 0, total, shown: 0, progress }
    }

    /// The most bytes one WRITE moves: whole sectors.
    fn chunk(&self) -> usize {
        block::MMC_COMMAND_MAX as usize / self.sector * self.sector
    }

    /// Sends `bytes`, whole sectors, in one WRITE(10).
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        let count = (bytes.len() / self.sector) as u16;
        let lba = self.next.to_be_bytes();
        let count_bytes = count.to_be_bytes();
        let cdb =
            [op::WRITE_10, 0, lba[0], lba[1], lba[2], lba[3], 0, count_bytes[0], count_bytes[1], 0];
        let start = Instant::now();
        loop {
            let answer = self.drive.command(&cdb, Data::Out(bytes), 60).map_err(io_error)?;
            match answer {
                Ok(_) => break,
                // The drive's buffer is full, or it is still writing the
                // last: again, in a moment.
                Err(sense) if sense.busy() && start.elapsed() < BUFFER_WAIT => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(sense) => {
                    let at = self.next;
                    return Err(io::Error::other(format!("writing block {at}: {}", sense.words())));
                }
            }
        }
        self.next = self.next.wrapping_add(u32::from(count));
        self.written += u64::from(count);
        self.show();
        Ok(())
    }

    /// Says how far it has come, every tenth.
    fn show(&mut self) {
        if !self.progress || self.total == 0 {
            return;
        }
        let tenth = self.written * 10 / self.total;
        if tenth > self.shown && tenth < 10 {
            self.shown = tenth;
            eprintln!("burn: {}: {}% written", self.drive.name, tenth * 10);
        }
    }

    /// Writes what is left, the last sector padded with zeros.
    fn finish(mut self) -> io::Result<u64> {
        if !self.pending.is_empty() {
            let whole = self.pending.len().div_ceil(self.sector) * self.sector;
            let mut last = std::mem::take(&mut self.pending);
            last.resize(whole, 0);
            self.send(&last)?;
        }
        Ok(self.written)
    }
}

impl Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let chunk = self.chunk();
        let room = chunk - self.pending.len();
        let taken = bytes.len().min(room);
        self.pending.extend_from_slice(&bytes[..taken]);
        if self.pending.len() == chunk {
            let full = std::mem::take(&mut self.pending);
            self.send(&full)?;
            self.pending = full;
            self.pending.clear();
        }
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `bytes` as people read a disc's size: in powers of 1000, as discs are
/// sold.
fn size(bytes: u64) -> String {
    format!("{}B", util::human(bytes, true))
}

/// A failure, as the writer's I/O error.
fn io_error(failed: Fail) -> io::Error {
    io::Error::other(failed.message)
}

/// What a burn writes.
enum Source {
    /// An image file, of its sectors.
    Image(PathBuf, u64),
    /// A folder, made into an image as it is written.
    Folder(Box<Image>),
    /// Tracks of audio, their sectors.
    Audio(Vec<Vec<u8>>),
}

impl Source {
    /// The sectors it fills on the disc.
    fn sectors(&self) -> u64 {
        match self {
            Source::Image(_, sectors) => *sectors,
            Source::Folder(image) => image.sectors(),
            Source::Audio(tracks) => {
                u64::from(PREGAP)
                    + tracks.iter().map(|t| (t.len() / AUDIO_SECTOR) as u64).sum::<u64>()
            }
        }
    }

    fn content(&self) -> Content {
        match self {
            Source::Audio(_) => Content::Audio,
            _ => Content::Data,
        }
    }
}

/// The options of a burn.
struct Burn {
    blank: bool,
    test: bool,
    eject: bool,
    progress: bool,
}

/// The first optical drive with a disc it writes in it, by what
/// `/kernel/block/<drive>/disc` says.
fn writing_drive() -> Result<String, Fail> {
    let mut names: Vec<String> = std::fs::read_dir(BLOCKS)
        .map_err(|e| util::failed(&BLOCKS, e))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let writable = |disc: &str| {
        let kind = disc.split_whitespace().next().unwrap_or("");
        !matches!(kind, "" | "none" | "unreadable" | "cd-rom" | "dvd-rom" | "bd-rom" | "disc")
    };
    names
        .into_iter()
        .find(|name| probe::block_text(name, "disc").is_some_and(|disc| writable(&disc)))
        .ok_or_else(|| fail("no drive has a disc it writes in it".into()))
}

/// Opens the drive `name`, or the first with a disc it writes.
fn open(name: Option<&str>) -> Result<Drive, Fail> {
    let name = match name {
        Some(name) => name.strip_prefix("/devices/").unwrap_or(name).to_string(),
        None => writing_drive()?,
    };
    let path = format!("/devices/{name}");
    let device =
        File::options().read(true).write(true).open(&path).map_err(|e| util::failed(&path, e))?;
    Ok(Drive { name, device })
}

/// Blanks a CD-RW or DVD-RW, all of it if `full` or else its table of
/// contents, or formats a DVD+RW, DVD-RAM or BD-RE, and waits.
fn erase(drive: &Drive, disc: &Disc, full: bool) -> Result<(), Fail> {
    match disc.way {
        Way::Cd { rewritable: true } | Way::Incremental { rewritable: true } => {
            eprintln!("burn: {}: blanking the {}", drive.name, disc.name());
            // Immed, and the blanking type: 0 the whole disc, 1 minimal.
            let cdb = [op::BLANK, 0x10 | u8::from(!full), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            drive.run("BLANK", &cdb, Data::None, 0)?;
            drive.wait_ready("BLANK", LONG_WAIT)
        }
        Way::Overwrite => format(drive, disc.profile),
        _ => Err(fail(format!("{}: a {} is not erased", drive.name, disc.name()))),
    }
}

/// Formats the DVD+RW, DVD-RW, DVD-RAM or BD-RE in `drive`, and waits.
fn format(drive: &Drive, profile: u16) -> Result<(), Fail> {
    let found = capacities(drive, profile)?;
    let (blocks, kind) = found.format.ok_or_else(|| {
        fail(format!(
            "{}: the drive offers no format for the {}",
            drive.name,
            profile_name(profile)
        ))
    })?;
    eprintln!("burn: {}: formatting the {}", drive.name, profile_name(profile));
    // The header: FOV, Immed; the descriptor: its blocks and its type.
    let mut list = vec![0, 0x82, 0, 8];
    list.extend(blocks.to_be_bytes());
    list.extend([kind << 2, 0, 0, 0]);
    // FmtData, format code 1.
    let cdb = [op::FORMAT_UNIT, 0x11, 0, 0, 0, 0];
    drive.run("FORMAT UNIT", &cdb, Data::Out(&list), 0)?;
    drive.wait_ready("FORMAT UNIT", LONG_WAIT)
}

/// Sets the write parameters of `disc` for `content`.
fn set_parameters(drive: &Drive, disc: &Disc, content: Content, test: bool) -> Result<(), Fail> {
    let cdb = [op::MODE_SENSE_10, 0, 0x05, 0, 0, 0, 0, 0, 255, 0];
    let sensed = drive.ask("MODE SENSE", &cdb, 255)?;
    let list = write_parameters(&sensed, disc.way, content, test)
        .ok_or_else(|| fail(format!("{}: the drive has no write parameters page", drive.name)))?;
    let length = (list.len() as u16).to_be_bytes();
    let cdb = [op::MODE_SELECT_10, 0x10, 0, 0, 0, 0, 0, length[0], length[1], 0];
    drive.run("MODE SELECT", &cdb, Data::Out(&list), 0).map(drop)
}

/// Closes the track and the session, or finalizes the disc, as its way
/// wants, and waits.
fn close(drive: &Drive, disc: &Disc, track: u16) -> Result<(), Fail> {
    let functions: &[u8] = match disc.way {
        // The track, then the session: with no next session in the write
        // parameters, the disc is closed with it.
        Way::Cd { .. } | Way::Incremental { .. } => &[1, 2],
        // The track, then the disc finalized.
        Way::Sequential => &[1, 6],
        // A DVD+RW's background format stopped, its lead-out written.
        Way::Overwrite if matches!(disc.profile, 0x1a | 0x2a) => &[2],
        Way::Overwrite => &[],
    };
    for &function in functions {
        let number = if function == 1 { track.to_be_bytes() } else { [0, 0] };
        // Immed.
        let cdb = [op::CLOSE_TRACK_SESSION, 1, function, 0, number[0], number[1], 0, 0, 0, 0];
        drive.run("CLOSE TRACK/SESSION", &cdb, Data::None, 0)?;
        drive.wait_ready("CLOSE TRACK/SESSION", LONG_WAIT)?;
    }
    Ok(())
}

/// Burns `source` onto the disc in `drive`.
fn burn(drive: &Drive, source: Source, options: &Burn) -> Result<(), Fail> {
    let mut disc = Disc::inspect(drive)?;
    let content = source.content();
    if content == Content::Audio && !matches!(disc.way, Way::Cd { .. }) {
        return Err(fail(format!("{}: audio is burned on CDs, not a {}", drive.name, disc.name())));
    }
    if options.test && !disc.way.has_page() {
        return Err(fail(format!("{}: a {} has no test write", drive.name, disc.name())));
    }
    let rewritable =
        matches!(disc.way, Way::Cd { rewritable: true } | Way::Incremental { rewritable: true });
    match disc.way {
        _ if !disc.blank() && rewritable && options.blank => {
            erase(drive, &disc, false)?;
            disc = Disc::inspect(drive)?;
        }
        _ if !disc.blank() && rewritable => {
            return Err(fail(format!(
                "{}: the {} is not blank: burn --blank erases it first",
                drive.name,
                disc.name()
            )));
        }
        Way::Overwrite => {
            if !capacities(drive, disc.profile)?.formatted {
                format(drive, disc.profile)?;
            }
        }
        _ if !disc.blank() => {
            return Err(fail(format!(
                "{}: the {} is written already, and is not erased",
                drive.name,
                disc.name()
            )));
        }
        _ => {}
    }
    let sectors = source.sectors();
    let (start, free, track) = if disc.way == Way::Overwrite {
        (0, u64::from(capacities(drive, disc.profile)?.blocks), 1)
    } else {
        if disc.way.has_page() {
            set_parameters(drive, &disc, content, options.test)?;
        }
        let found = invisible_track(drive)?;
        let next = found.next.ok_or_else(|| {
            fail(format!("{}: the drive says the {} has no room to write", drive.name, disc.name()))
        })?;
        (next, u64::from(found.free), found.number)
    };
    if sectors > free {
        return Err(fail(format!(
            "{}: {} needs {} sectors, and the {} has {free}",
            drive.name,
            describe(&source),
            sectors,
            disc.name()
        )));
    }
    let test = if options.test { " as a test" } else { "" };
    eprintln!(
        "burn: {}: writing {} to the {}{test}",
        drive.name,
        size(sectors * BLOCK as u64),
        disc.name()
    );
    let written = match source {
        Source::Image(path, _) => {
            let mut file = File::open(&path).map_err(|e| util::failed(&path.display(), e))?;
            let mut writer = Writer::new(drive, start, BLOCK, sectors, options.progress);
            copy(&mut file, &mut writer).map_err(|e| fail(format!("{}: {e}", drive.name)))?;
            writer.finish()
        }
        Source::Folder(image) => {
            let mut writer = Writer::new(drive, start, BLOCK, sectors, options.progress);
            image.write(&mut writer).map_err(|e| fail(format!("{}: {e}", drive.name)))?;
            writer.finish()
        }
        Source::Audio(tracks) => {
            let lengths: Vec<u32> =
                tracks.iter().map(|t| (t.len() / AUDIO_SECTOR) as u32).collect();
            let sheet = cue_sheet(&lengths);
            let size = (sheet.len() as u32).to_be_bytes();
            let cdb = [op::SEND_CUE_SHEET, 0, 0, 0, 0, 0, size[1], size[2], size[3], 0];
            drive.run("SEND CUE SHEET", &cdb, Data::Out(&sheet), 0)?;
            let mut writer =
                Writer::new(drive, PREGAP.wrapping_neg(), AUDIO_SECTOR, sectors, options.progress);
            let pregap = vec![0u8; PREGAP as usize * AUDIO_SECTOR];
            let wrote = writer
                .write_all(&pregap)
                .and_then(|()| tracks.iter().try_for_each(|track| writer.write_all(track)));
            wrote.map_err(|e| fail(format!("{}: {e}", drive.name)))?;
            writer.finish()
        }
    }
    .map_err(|e| fail(format!("{}: {e}", drive.name)))?;
    // Everything in the drive's buffer on the disc; a CD written at once
    // gets its lead-out.
    let cdb = [op::SYNCHRONIZE_CACHE, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    drive.run("SYNCHRONIZE CACHE", &cdb, Data::None, block::MMC_TIMEOUT_MAX)?;
    drive.wait_ready("SYNCHRONIZE CACHE", LONG_WAIT)?;
    if content == Content::Data {
        close(drive, &disc, track)?;
    }
    let done = if options.test { "a test write of" } else { "burned" };
    println!("{}: {done} {} sectors on the {}", drive.name, written, disc.name());
    Ok(())
}

/// Copies all of `from` to `to`.
fn copy(from: &mut dyn Read, to: &mut dyn Write) -> io::Result<()> {
    let mut buffer = vec![0u8; block::MMC_COMMAND_MAX as usize];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => to.write_all(&buffer[..n])?,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

/// What `source` is, in words.
fn describe(source: &Source) -> String {
    match source {
        Source::Image(path, _) => path.display().to_string(),
        Source::Folder(_) => "the folder".into(),
        Source::Audio(tracks) => format!("{} tracks", tracks.len()),
    }
}

/// Lets the drive go: the kernel looks at the disc in `drive` afresh, as
/// one burned, and has it in `/devices` when this returns; or, if
/// `eject`, ejects it, which the kernel lets the program burning a disc
/// do before volumed(8) mounts it.
fn done(drive: &Drive, eject: bool) -> Result<(), Fail> {
    let request = if eject { block::EJECT } else { block::REREAD_PARTITIONS };
    blockdev::control(&drive.device, request, &mut [])
        .map_err(|status| fail(why(&drive.name, status)))
}

/// The image `label` of the folder `path`, with Rock Ridge and Joliet,
/// and UDF if `udf`; the files left out are said.
pub fn folder_image(path: &Path, label: &str, udf: bool) -> Result<Image, Fail> {
    let (root, skipped) = Node::scan(path).map_err(|e| util::failed(&path.display(), e))?;
    for left in skipped {
        eprintln!("burn: {}: not a file, folder or link a disc holds; left out", left.display());
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let mut options = Options::new(label, time);
    options.udf = udf;
    Image::new(root, options).map_err(|err| fail(format!("{}: {err}", path.display())))
}

/// The label of a disc of the folder `path`: its name.
fn default_label(path: &Path) -> String {
    let name = std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    name.unwrap_or_else(|| "DISC".into())
}

/// `burn info`: what disc is in the drive, and how much room it has.
fn info(drive: &Drive) -> Result<i32, Fail> {
    let disc = Disc::inspect(drive)?;
    let state = match (disc.way, disc.information.status) {
        (Way::Overwrite, _) => {
            let found = capacities(drive, disc.profile)?;
            let formatted = if found.formatted { "formatted" } else { "not formatted" };
            let size = size(u64::from(found.blocks) * BLOCK as u64);
            format!("{formatted}, {size}")
        }
        (_, 0) => {
            let free = invisible_track(drive)?.free;
            format!("blank, {} free", size(u64::from(free) * BLOCK as u64))
        }
        (_, 1) => "written, not closed".into(),
        _ => "written and closed".into(),
    };
    let erasable = if disc.information.erasable { ", erasable" } else { "" };
    println!("{}: {}, {state}{erasable}", drive.name, disc.name());
    Ok(0)
}

/// burn(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = args.optional_operand(|a| matches!(a.to_str(), Some("blank" | "info")));
    let command = command.map(|c| c.to_string_lossy().into_owned());
    let (mut drive, mut label, mut udf, mut audio, mut full) = (None, None, false, false, false);
    let mut options = Burn { blank: false, test: false, eject: false, progress: true };
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('h') => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Long(long) if long == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Short('d') => drive = Some(args.text()?),
            Arg::Long(long) if long == "drive" => drive = Some(args.text()?),
            Arg::Long(long) if long == "label" => label = Some(args.text()?),
            Arg::Long(long) if long == "udf" => udf = true,
            Arg::Long(long) if long == "audio" => audio = true,
            Arg::Long(long) if long == "blank" => options.blank = true,
            Arg::Long(long) if long == "full" => full = true,
            Arg::Long(long) if long == "test" => options.test = true,
            Arg::Long(long) if long == "eject" => options.eject = true,
            Arg::Short('q') => options.progress = false,
            Arg::Long(long) if long == "quiet" => options.progress = false,
            Arg::Operand(operand) => operands.push(PathBuf::from(operand)),
            other => return Err(args.unknown(&other)),
        }
    }
    match command.as_deref() {
        Some("info") => {
            if !operands.is_empty() || audio || full || label.is_some() {
                return Err(usage(format!("burn info takes only --drive\n{USAGE}")));
            }
            return info(&open(drive.as_deref())?);
        }
        Some(_) => {
            if !operands.is_empty() || audio || label.is_some() || udf {
                return Err(usage(format!("burn blank takes --drive and --full\n{USAGE}")));
            }
            let drive = open(drive.as_deref())?;
            let disc = Disc::inspect(&drive)?;
            erase(&drive, &disc, full)?;
            done(&drive, false)?;
            println!("{}: the {} is blank", drive.name, disc.name());
            return Ok(0);
        }
        None => {}
    }
    if full {
        return Err(usage(format!("--full is burn blank's\n{USAGE}")));
    }
    let source = if audio {
        if operands.is_empty() || label.is_some() || udf {
            return Err(usage(format!("burn --audio takes WAV and FLAC files\n{USAGE}")));
        }
        if operands.len() > 99 {
            return Err(fail("a CD holds 99 tracks at most".into()));
        }
        Source::Audio(operands.iter().map(|path| audio_track(path)).collect::<Result<_, _>>()?)
    } else {
        let [path] = operands.as_slice() else {
            return Err(usage(format!("burn takes one image or folder\n{USAGE}")));
        };
        let meta = std::fs::metadata(path).map_err(|e| util::failed(&path.display(), e))?;
        if meta.is_dir() {
            let label = label.unwrap_or_else(|| default_label(path));
            Source::Folder(Box::new(folder_image(path, &label, udf)?))
        } else {
            if label.is_some() || udf {
                return Err(usage(format!("--label and --udf are for a folder\n{USAGE}")));
            }
            Source::Image(path.clone(), meta.len().div_ceil(BLOCK as u64))
        }
    };
    let drive = open(drive.as_deref())?;
    let burned = burn(&drive, source, &options);
    // The disc looked at afresh, burned or not: a burn that failed has
    // the kernel find what is on it.
    let looked = done(&drive, options.eject && burned.is_ok());
    burned?;
    looked?;
    Ok(0)
}

/// mkiso(1): an image of a folder, with Rock Ridge and Joliet, and UDF
/// if asked, written to a file.
pub fn mkiso(args: &mut Args) -> Result<i32, Fail> {
    const MKISO_USAGE: &str =
        "usage: mkiso [--label LABEL] [--udf] [--no-joliet] [--no-rock] -o FILE FOLDER";
    let (mut label, mut udf, mut joliet, mut rock, mut output) = (None, false, true, true, None);
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('h') => {
                println!("{MKISO_USAGE}");
                return Ok(0);
            }
            Arg::Long(long) if long == "help" => {
                println!("{MKISO_USAGE}");
                return Ok(0);
            }
            Arg::Short('V') => label = Some(args.text()?),
            Arg::Long(long) if long == "label" => label = Some(args.text()?),
            Arg::Long(long) if long == "udf" => udf = true,
            Arg::Long(long) if long == "no-joliet" => joliet = false,
            Arg::Long(long) if long == "no-rock" => rock = false,
            Arg::Short('o') => output = Some(PathBuf::from(args.value()?)),
            Arg::Long(long) if long == "output" => output = Some(PathBuf::from(args.value()?)),
            Arg::Operand(operand) => operands.push(PathBuf::from(operand)),
            other => return Err(args.unknown(&other)),
        }
    }
    let ([folder], Some(output)) = (operands.as_slice(), output) else {
        return Err(usage(format!("mkiso takes -o FILE and one folder\n{MKISO_USAGE}")));
    };
    let label = label.unwrap_or_else(|| default_label(folder));
    let (root, skipped) = Node::scan(folder).map_err(|e| util::failed(&folder.display(), e))?;
    for left in skipped {
        eprintln!("mkiso: {}: not a file, folder or link a disc holds; left out", left.display());
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let mut options = Options::new(&label, time);
    (options.udf, options.joliet, options.rock) = (udf, joliet, rock);
    let image =
        Image::new(root, options).map_err(|err| fail(format!("{}: {err}", folder.display())))?;
    let part = PathBuf::from(format!("{}.part", output.display()));
    let file = File::create(&part).map_err(|e| util::failed(&part.display(), e))?;
    let mut out = io::BufWriter::with_capacity(1 << 20, file);
    let written = image.write(&mut out).and_then(|()| out.flush());
    if let Err(err) = written {
        let _ = std::fs::remove_file(&part);
        return Err(util::failed(&output.display(), err));
    }
    drop(out);
    std::fs::rename(&part, &output).map_err(|e| util::failed(&output.display(), e))?;
    println!("{}: {} sectors, {}", output.display(), image.sectors(), size(image.bytes()));
    Ok(0)
}
