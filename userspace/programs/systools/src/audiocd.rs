// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! audiocd(1): lists, plays and rips the music CD in an optical drive
//! (ADR 0447).
//!
//! A music CD's tracks are not files: the kernel reads them for a program
//! with radix-block(2)'s READ_TOC and READ_AUDIO. audiocd lists the
//! tracks, with the titles and performers of the disc's CD-Text
//! (`audio::cd`); plays them through an output of `/devices/audio`, as
//! audio(1) plays a WAV file; and rips them, a file a track, to FLAC
//! (`audio::flac`) tagged from the CD-Text, or to WAV. A file being
//! ripped is written under a name ending `.part`, and takes its own name
//! once it is whole.
//!
//! The drive is the one named with `--drive`, or else the first that
//! holds a music CD (or a CD of data and audio both), as
//! `/kernel/block/<drive>/disc` says. Start at
//! [`main`].

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use audio::cd::{self, CdText, Names, Track};
use audio::{convert, flac, resample, wav};
use radix_abi::Status;
use radix_abi::block;
use radix_abi::errors::{File as FileError, Kernel};

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::probe;
use crate::sound::{self, Player};
use crate::sys;
use crate::util::{self, Fail, fail, usage};

const USAGE: &str = "usage: audiocd [list] [--drive NAME]
       audiocd play [--drive NAME] [--output NAME] [TRACK...]
       audiocd rip [--drive NAME] [--wav] [--to DIRECTORY] [TRACK...]";

/// Where the kernel says what each disk is.
const BLOCKS: &str = "/kernel/block";
/// The most sectors one READ_AUDIO reads.
const MOST: u32 = block::READ_AUDIO_MAX;
/// The sectors read before they are played or written: a second.
const SECOND: u32 = cd::SECTORS_PER_SECOND;
/// How many times a read that failed is tried again: a scratch read
/// again often reads.
const RETRIES: u32 = 2;
/// The bytes READ_TOC's buffers take: the tracks' (99 and the lead-out,
/// eight bytes each), the sessions', and CD-Text's at most.
const TOC_BYTES: usize = 4 + 100 * 8;
const SESSIONS_BYTES: usize = 12;
const TEXT_BYTES: usize = block::READ_TOC_MAX as usize;

/// A music CD in a drive: the drive opened, the disc's tracks and its
/// CD-Text.
struct Disc {
    drive: String,
    device: File,
    tracks: Vec<Track>,
    text: CdText,
}

/// The first drive that holds a music CD, or else a CD of data and
/// audio both, as the kernel says.
fn music_drive() -> Result<String, Fail> {
    let mut names: Vec<String> = std::fs::read_dir(BLOCKS)
        .map_err(|e| util::failed(&BLOCKS, e))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let holding = |kind: &str| {
        names
            .iter()
            .find(|name| probe::block_text(name, "disc").is_some_and(|disc| disc.ends_with(kind)))
            .cloned()
    };
    holding(" audio")
        .or_else(|| holding(" mixed"))
        .ok_or_else(|| fail("no drive has a music CD in it".into()))
}

/// What a failed request on `drive` means, in words.
fn why(drive: &str, status: Status) -> String {
    match (FileError::from_status(status), Kernel::from_status(status)) {
        (Some(FileError::NoDevice), _) => format!("{drive} has no disc in it"),
        (_, Some(Kernel::NotSupported)) => format!("{drive} is not an optical drive"),
        _ => format!("{drive}: {}", util::strip_os_error(&sys::io_error(status))),
    }
}

/// READ_TOC on `device`, in `format`, from `track`, into a buffer of
/// `length` bytes: the drive's answer, as long as it says it is.
fn read_toc(device: &File, format: u32, track: u32, length: usize) -> Result<Vec<u8>, Status> {
    let mut answer = vec![0u8; length];
    let mut request = [0u8; block::READ_TOC_SIZE as usize];
    request[0..4].copy_from_slice(&format.to_le_bytes());
    request[4..8].copy_from_slice(&track.to_le_bytes());
    request[8..16].copy_from_slice(&(answer.as_mut_ptr() as u64).to_le_bytes());
    request[16..24].copy_from_slice(&(length as u64).to_le_bytes());
    blockdev::control(device, block::READ_TOC, &mut request)?;
    let said = usize::from(u16::from_be_bytes([answer[0], answer[1]])) + 2;
    answer.truncate(said.min(length));
    Ok(answer)
}

/// Opens the drive `name`, or the first that holds a music CD, and reads
/// its disc's tracks and CD-Text.
fn open(name: Option<&str>) -> Result<Disc, Fail> {
    let drive = match name {
        Some(name) => name.strip_prefix("/devices/").unwrap_or(name).to_string(),
        None => music_drive()?,
    };
    let path = format!("/devices/{drive}");
    let device = File::open(&path).map_err(|e| util::failed(&path, e))?;
    let toc =
        read_toc(&device, block::TOC_TRACKS, 1, TOC_BYTES).map_err(|s| fail(why(&drive, s)))?;
    // A drive that does not say its sessions has one, as far as this goes;
    // a disc without CD-Text has none to give.
    let sessions = read_toc(&device, block::TOC_SESSIONS, 0, SESSIONS_BYTES).ok();
    let text = read_toc(&device, block::TOC_CD_TEXT, 0, TEXT_BYTES)
        .map_or_else(|_| CdText::default(), |a| cd::cd_text(&a));
    let tracks =
        cd::tracks(&toc, sessions.as_deref()).map_err(|why| fail(format!("{drive}: {why}")))?;
    if !tracks.iter().any(|track| track.audio) {
        return Err(fail(format!("{drive}: the disc has no tracks of audio")));
    }
    Ok(Disc { drive, device, tracks, text })
}

impl Disc {
    /// Reads `count` sectors from `first` onto the end of `out`, a request
    /// of at most [`MOST`] at a time, each tried again [`RETRIES`] times.
    fn read(&self, first: u32, count: u32, out: &mut Vec<u8>) -> Result<(), Fail> {
        let mut at = first;
        while at < first + count {
            let n = (first + count - at).min(MOST);
            let start = out.len();
            out.resize(start + n as usize * cd::SECTOR, 0);
            let mut request = [0u8; block::READ_AUDIO_SIZE as usize];
            request[0..4].copy_from_slice(&at.to_le_bytes());
            request[4..8].copy_from_slice(&n.to_le_bytes());
            request[8..16].copy_from_slice(&(out[start..].as_mut_ptr() as u64).to_le_bytes());
            let mut tries = 0;
            while let Err(status) = blockdev::control(&self.device, block::READ_AUDIO, &mut request)
            {
                let gone = FileError::from_status(status) == Some(FileError::NoDevice);
                if gone || tries == RETRIES {
                    return Err(fail(format!("sector {at}: {}", why(&self.drive, status))));
                }
                tries += 1;
            }
            at += n;
        }
        Ok(())
    }

    /// The track numbered `number`, if it is one of audio.
    fn audio_track(&self, number: u8) -> Result<Track, Fail> {
        let track = self.tracks.iter().find(|track| track.number == number);
        let track =
            track.ok_or_else(|| fail(format!("{}: the disc has no track {number}", self.drive)))?;
        if !track.audio {
            return Err(fail(format!("{}: track {number} is data, not audio", self.drive)));
        }
        Ok(*track)
    }

    /// The tracks `operands` name, as numbers and ranges such as `2-5`, or
    /// else every track of audio.
    fn chosen(&self, operands: &[String]) -> Result<Vec<Track>, Fail> {
        if operands.is_empty() {
            return Ok(self.tracks.iter().filter(|track| track.audio).copied().collect());
        }
        let number = |text: &str| {
            text.trim().parse::<u8>().map_err(|_| usage(format!("'{text}' is not a track")))
        };
        let mut chosen = Vec::new();
        for operand in operands {
            let (from, to) = match operand.split_once('-') {
                Some((from, to)) => (number(from)?, number(to)?),
                None => (number(operand)?, number(operand)?),
            };
            for n in from..=to {
                chosen.push(self.audio_track(n)?);
            }
        }
        Ok(chosen)
    }

    /// The names of track `track`, as its CD-Text gives them, with the
    /// disc's performer if it has none of its own.
    fn names(&self, track: &Track) -> Names {
        let mut names = self.text.track(track.number);
        if names.performer.is_none() {
            names.performer.clone_from(&self.text.disc.performer);
        }
        names
    }
}

/// `seconds` as minutes and seconds.
fn duration(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// `audiocd list`: the disc and its tracks.
fn list(disc: &Disc) -> Result<i32, Fail> {
    let mut out = io::stdout().lock();
    let album = &disc.text.disc;
    match (&album.title, &album.performer) {
        (Some(title), Some(performer)) => writeln!(out, "{}: {title}, by {performer}", disc.drive)?,
        (Some(title), None) => writeln!(out, "{}: {title}", disc.drive)?,
        _ => writeln!(out, "{}: a music CD", disc.drive)?,
    }
    let mut seconds = 0;
    for track in &disc.tracks {
        if !track.audio {
            writeln!(out, "{:>2}  data", track.number)?;
            continue;
        }
        seconds += track.seconds();
        let own = disc.text.track(track.number);
        let title = own.title.unwrap_or_else(|| format!("Track {}", track.number));
        let by = match own.performer {
            Some(performer) if Some(&performer) != album.performer.as_ref() => {
                format!(", by {performer}")
            }
            _ => String::new(),
        };
        writeln!(out, "{:>2}  {:>5}  {title}{by}", track.number, duration(track.seconds()))?;
    }
    let count = disc.tracks.iter().filter(|track| track.audio).count();
    writeln!(out, "{count} track{}, {}", if count == 1 { "" } else { "s" }, duration(seconds))?;
    Ok(0)
}

/// `audiocd play`: the tracks chosen, through output `output`.
fn play(disc: &Disc, output: Option<&str>, tracks: &[Track]) -> Result<i32, Fail> {
    let name = sound::chosen(output)?;
    let mut player = Player::open(&name, cd::RATE, usize::from(cd::CHANNELS))?;
    let mut resampler = resample::Resampler::new(cd::RATE, player.rate, player.channels);
    for track in tracks {
        let title = disc.names(track).title.unwrap_or_else(|| format!("Track {}", track.number));
        println!("{:>2}  {:>5}  {title}", track.number, duration(track.seconds()));
        let mut at = track.start;
        let end = track.start + track.sectors;
        while at < end {
            let n = (end - at).min(SECOND);
            let mut bytes = Vec::with_capacity(n as usize * cd::SECTOR);
            disc.read(at, n, &mut bytes)?;
            let samples = convert::from_bytes(&bytes);
            let mapped = convert::channels(&samples, usize::from(cd::CHANNELS), player.channels);
            let mut out = Vec::new();
            resampler.process(&mapped, &mut out);
            player.play(&out)?;
            at += n;
        }
    }
    player.finish()?;
    Ok(0)
}

/// A file name of `title`: no slashes or control characters, and not
/// hidden.
fn file_name(number: u8, title: Option<&str>, extension: &str) -> String {
    let title: String = title
        .unwrap_or("")
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let title = title.trim().trim_start_matches('.');
    if title.is_empty() {
        format!("Track {number:02}.{extension}")
    } else {
        format!("{number:02} {title}.{extension}")
    }
}

/// The FLAC tags of `track` of `disc`, from its CD-Text.
fn tags(disc: &Disc, track: &Track) -> Vec<(&'static str, String)> {
    let names = disc.names(track);
    let mut tags = Vec::new();
    if let Some(title) = names.title {
        tags.push(("TITLE", title));
    }
    if let Some(performer) = names.performer {
        tags.push(("ARTIST", performer));
    }
    if let Some(album) = &disc.text.disc.title {
        tags.push(("ALBUM", album.clone()));
    }
    if let Some(performer) = &disc.text.disc.performer {
        tags.push(("ALBUMARTIST", performer.clone()));
    }
    let total = disc.tracks.iter().filter(|t| t.audio).count();
    tags.push(("TRACKNUMBER", track.number.to_string()));
    tags.push(("TRACKTOTAL", total.to_string()));
    tags
}

/// Rips `track` of `disc` into `file`, as FLAC or as WAV.
fn rip_track(disc: &Disc, track: &Track, file: &mut File, as_wav: bool) -> Result<(), Fail> {
    let failed = |e: io::Error| util::failed(&"write", e);
    let frames = track.frames();
    let mut encoder = (!as_wav).then(|| flac::Encoder::new(cd::RATE, cd::CHANNELS));
    if as_wav {
        let frames = u32::try_from(frames)
            .map_err(|_| fail("the track is too long for a WAV file".into()))?;
        file.write_all(&wav::header(cd::RATE, u16::from(cd::CHANNELS), frames)).map_err(failed)?;
    } else {
        let tags = tags(disc, track);
        let tags: Vec<(&str, &str)> =
            tags.iter().map(|(name, value)| (*name, value.as_str())).collect();
        let info = flac::StreamInfo::before(cd::RATE, cd::CHANNELS, frames);
        file.write_all(&flac::header(&info, &tags)).map_err(failed)?;
    }
    let mut at = track.start;
    let end = track.start + track.sectors;
    let mut out = Vec::new();
    while at < end {
        let n = (end - at).min(SECOND);
        let mut bytes = Vec::with_capacity(n as usize * cd::SECTOR);
        disc.read(at, n, &mut bytes)?;
        match &mut encoder {
            Some(encoder) => {
                out.clear();
                encoder.push(&convert::from_bytes(&bytes), &mut out);
                file.write_all(&out).map_err(failed)?;
            }
            None => file.write_all(&bytes).map_err(failed)?,
        }
        at += n;
    }
    if let Some(encoder) = encoder {
        out.clear();
        let info = encoder.finish(&mut out);
        file.write_all(&out).map_err(failed)?;
        file.seek(SeekFrom::Start(flac::STREAMINFO_AT)).map_err(failed)?;
        file.write_all(&info.bytes()).map_err(failed)?;
    }
    file.sync_all().map_err(failed)
}

/// `audiocd rip`: the tracks chosen, a file each in `directory`.
fn rip(disc: &Disc, directory: &Path, as_wav: bool, tracks: &[Track]) -> Result<i32, Fail> {
    let extension = if as_wav { "wav" } else { "flac" };
    for track in tracks {
        let names = disc.names(track);
        let name = file_name(track.number, names.title.as_deref(), extension);
        let path = directory.join(&name);
        let part = PathBuf::from(format!("{}.part", path.display()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part)
            .map_err(|e| util::failed(&part.display(), e))?;
        if let Err(err) = rip_track(disc, track, &mut file, as_wav) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err(err);
        }
        std::fs::rename(&part, &path).map_err(|e| util::failed(&path.display(), e))?;
        println!("{:>2}  {:>5}  {}", track.number, duration(track.seconds()), path.display());
    }
    Ok(0)
}

/// audiocd(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = args.subcommand().map(|c| c.to_string_lossy().into_owned());
    let (mut drive, mut output, mut directory, mut as_wav) = (None, None, None, false);
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
            Arg::Short('o') => output = Some(args.text()?),
            Arg::Long(long) if long == "output" => output = Some(args.text()?),
            Arg::Long(long) if long == "to" => directory = Some(PathBuf::from(args.value()?)),
            Arg::Long(long) if long == "wav" => as_wav = true,
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let command = command.as_deref().unwrap_or("list");
    let misplaced = match command {
        "list" => (output.is_some() || directory.is_some() || as_wav || !operands.is_empty())
            .then_some("list"),
        "play" => (directory.is_some() || as_wav).then_some("play"),
        "rip" => output.is_some().then_some("rip"),
        "help" => {
            println!("{USAGE}");
            return Ok(0);
        }
        other => return Err(usage(format!("unknown command '{other}'\n{USAGE}"))),
    };
    if let Some(command) = misplaced {
        return Err(usage(format!("an option or operand {command} does not take\n{USAGE}")));
    }
    let disc = open(drive.as_deref())?;
    match command {
        "play" => play(&disc, output.as_deref(), &disc.chosen(&operands)?),
        "rip" => rip(
            &disc,
            directory.as_deref().unwrap_or(Path::new(".")),
            as_wav,
            &disc.chosen(&operands)?,
        ),
        _ => list(&disc),
    }
}
