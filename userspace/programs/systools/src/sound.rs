// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! audio(1) and audiod(8): the sound outputs (ADR 0320).
//!
//! Each output the kernel drives is a file of `/devices/audio`, which
//! takes OSS's requests (radix-audio(2)); `default` leads to the one in
//! use. audio(1) lists them, sets the volume, mutes, picks the output,
//! plays a test tone (440 Hz on the left, then 660 Hz on the right) and
//! plays WAV files, converted to what the output takes. audiod(8) keeps
//! the outputs as audio.toml(5) says: it gives each the volume (none
//! while muted), points `default` at the output named, or else at the
//! one plugged in last, or the first, looks again every two seconds for
//! outputs that came or went and for a changed file, and turns the
//! keyboard's volume keys into changes of the settings.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use audio::settings::{self, Settings};
use audio::{convert, resample, tone, wav};
use radix_abi::audio::{self as abi, OutputInfo};
use radix_abi::{Status, file, input};

use crate::args::Args;
use crate::sys::{self, SYS};
use crate::util::{self, Fail, fail, usage};

/// Where the outputs are.
const DIR: &str = "/devices/audio";
/// The link to the output in use.
const DEFAULT: &str = "/devices/audio/default";

/// How often audiod looks for outputs and a changed file.
const LOOK: Duration = Duration::from_secs(2);
/// How much a volume key changes the volume.
const VOLUME_STEP: u8 = 5;
/// How loud the test tone is, of full scale.
const TEST_AMPLITUDE: f64 = 0.5;
/// How long each side of the test tone plays, in milliseconds.
const TEST_MS: u32 = 600;

const USAGE: &str = "usage: audio [list]
       audio info [OUTPUT]
       audio volume [PERCENT | +N | -N]
       audio mute [on | off | toggle]
       audio output [NAME]
       audio test [OUTPUT]
       audio play [--output NAME] FILE...";

/// A request with `value` as its buffer, which the kernel reads and
/// writes.
fn control(output: &File, request: u32, value: &mut [u8]) -> io::Result<()> {
    let address = value.as_mut_ptr() as u64;
    let slot = sys::slot_of(output.as_raw_fd());
    file::control(&SYS, slot, request, address, address, value.len() as u64)
        .map(drop)
        .map_err(|status: Status| sys::io_error(status))
}

/// A request whose buffer is an int: returns the int the kernel wrote
/// back.
fn int_request(output: &File, request: u32, value: u32) -> io::Result<u32> {
    let mut bytes = value.to_le_bytes();
    control(output, request, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// What GET_INFO says of an output.
pub struct Info {
    pub rates: u32,
    pub left: u8,
    pub right: u8,
    pub played: u64,
    pub underruns: u32,
    pub channels: (u8, u8),
    pub flags: u8,
    pub name: String,
    pub model: String,
}

/// A NUL-padded field as text.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn info(output: &File) -> io::Result<Info> {
    let mut b = [0u8; OutputInfo::SIZE];
    control(output, abi::GET_INFO, &mut b)?;
    let u32_at = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let volume = u32_at(OutputInfo::OFFSET_VOLUME);
    Ok(Info {
        rates: u32_at(OutputInfo::OFFSET_RATES),
        left: (volume & 0xff) as u8,
        right: ((volume >> 8) & 0xff) as u8,
        played: u64::from_le_bytes(
            b[OutputInfo::OFFSET_PLAYED..][..8].try_into().unwrap_or([0; 8]),
        ),
        underruns: u32_at(OutputInfo::OFFSET_UNDERRUNS),
        channels: (b[OutputInfo::OFFSET_CHANNELS_MIN], b[OutputInfo::OFFSET_CHANNELS_MAX]),
        flags: b[OutputInfo::OFFSET_FLAGS],
        name: text(&b[OutputInfo::OFFSET_NAME..][..16]),
        model: text(&b[OutputInfo::OFFSET_MODEL..][..32]),
    })
}

/// Opens output `name` (a name in `/devices/audio`, or a path).
fn open(name: &str) -> Result<File, Fail> {
    let path = if name.contains('/') { name.to_string() } else { format!("{DIR}/{name}") };
    OpenOptions::new().read(true).write(true).open(&path).map_err(|e| util::failed(&path, e))
}

/// The outputs' names, in order: the files of `/devices/audio` but
/// `default` and `speech`.
pub fn outputs() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(DIR).map_or_else(
        |_| Vec::new(),
        |dir| {
            dir.filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != "default" && name != "speech")
                .collect()
        },
    );
    names.sort();
    names
}

/// The output `default` leads to.
fn default_output() -> Option<String> {
    let target = std::fs::read_link(DEFAULT).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

/// The output named, or the default one.
fn chosen(name: Option<&str>) -> Result<String, Fail> {
    match name {
        Some(name) => Ok(name.to_string()),
        None => default_output().ok_or_else(|| fail("no sound output".to_string())),
    }
}

/// Reads the settings: the defaults if the file is missing.
fn load() -> Settings {
    std::fs::read_to_string(settings::PATH)
        .map_or_else(|_| Settings::default(), |text| Settings::parse(&text))
}

/// Writes `settings` over the file, keeping its notes: a new file put in
/// the old one's place, so that audiod never reads half of one.
fn save(settings: &Settings) -> io::Result<()> {
    let old =
        std::fs::read_to_string(settings::PATH).unwrap_or_else(|_| settings::HEADER.to_string());
    if let Some(parent) = Path::new(settings::PATH).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let new = format!("{}.new", settings::PATH);
    std::fs::write(&new, settings.update(&old))?;
    std::fs::rename(&new, settings::PATH)
}

/// Gives every output the settings' volume; returns the outputs it could
/// not set.
fn apply_volume(settings: &Settings) -> Vec<String> {
    let volume = u32::from(settings.effective_volume());
    outputs()
        .into_iter()
        .filter(|name| {
            let set = open(name).ok().map(|output| {
                int_request(&output, abi::SET_PLAY_VOLUME, volume | volume << 8).is_ok()
            });
            set != Some(true)
        })
        .collect()
}

/// Makes `name` the default output.
fn make_default(name: &str) -> Result<(), Fail> {
    let output = open(name)?;
    control(&output, abi::MAKE_DEFAULT, &mut []).map_err(|e| util::failed(&name, e))
}

/// The rates of the RATE_* bits, in their order.
const RATES: [u32; 14] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    384000,
];

/// The rates of a mask, for people: "44.1, 48 kHz".
fn rates_text(rates: u32) -> String {
    let list: Vec<String> = RATES
        .iter()
        .enumerate()
        .filter(|&(bit, _)| rates & (1 << bit) != 0)
        .map(|(_, &rate)| {
            if rate.is_multiple_of(1000) {
                format!("{}", rate / 1000)
            } else {
                format!("{:.1}", f64::from(rate) / 1000.0)
            }
        })
        .collect();
    format!("{} kHz", list.join(", "))
}

/// `audio list`.
fn list() -> Result<i32, Fail> {
    let names = outputs();
    if names.is_empty() {
        println!("no sound output");
        return Ok(0);
    }
    let mut out = io::stdout().lock();
    for name in names {
        let output = open(&name)?;
        let info = info(&output).map_err(|e| util::failed(&name, e))?;
        let mark = if info.flags & abi::FLAG_DEFAULT != 0 { '*' } else { ' ' };
        let playing = if info.flags & abi::FLAG_SPEECH != 0 {
            "  speaking"
        } else if info.flags & abi::FLAG_PLAYING != 0 {
            "  playing"
        } else {
            ""
        };
        writeln!(
            out,
            "{mark} {:<10} {:<20} {:>3}%{playing}",
            info.name,
            info.model,
            info.left.max(info.right)
        )?;
    }
    Ok(0)
}

/// `audio info [OUTPUT]`.
fn show_info(name: Option<&str>) -> Result<i32, Fail> {
    let name = chosen(name)?;
    let output = open(&name)?;
    let info = info(&output).map_err(|e| util::failed(&name, e))?;
    let yes = |flag: u8| if info.flags & flag != 0 { "yes" } else { "no" };
    let player = if info.flags & abi::FLAG_SPEECH != 0 {
        "the screen reader"
    } else if info.flags & abi::FLAG_PLAYING != 0 {
        "a program"
    } else {
        "none"
    };
    let mut out = io::stdout().lock();
    writeln!(out, "Output:     {}", info.name)?;
    writeln!(out, "Model:      {}", info.model)?;
    writeln!(out, "Default:    {}", yes(abi::FLAG_DEFAULT))?;
    writeln!(
        out,
        "Volume:     {}% left, {}% right{}",
        info.left,
        info.right,
        if info.flags & abi::FLAG_HARDWARE_VOLUME != 0 { "" } else { " (scaled by the kernel)" }
    )?;
    writeln!(out, "Channels:   {} to {}", info.channels.0, info.channels.1)?;
    writeln!(out, "Rates:      {}", rates_text(info.rates))?;
    writeln!(out, "Player:     {player}")?;
    writeln!(out, "Played:     {}", util::human(info.played, false))?;
    writeln!(out, "Underruns:  {}", info.underruns)?;
    Ok(0)
}

/// `audio volume [PERCENT | +N | -N]`: the volume of every output, kept
/// in the settings.
fn volume(value: Option<&str>) -> Result<i32, Fail> {
    let mut settings = load();
    let Some(value) = value else {
        let muted = if settings.muted { " (muted)" } else { "" };
        println!("{}%{muted}", settings.volume);
        return Ok(0);
    };
    let bad = || usage(format!("not a volume: {value}"));
    let number = |text: &str| text.trim_end_matches('%').parse::<i32>().map_err(|_| bad());
    let new = if let Some(up) = value.strip_prefix('+') {
        i32::from(settings.volume) + number(up)?
    } else if let Some(down) = value.strip_prefix('-') {
        i32::from(settings.volume) - number(down)?
    } else {
        number(value)?
    };
    settings.volume = new.clamp(0, 100) as u8;
    settings.muted = false;
    change(&settings)?;
    println!("{}%", settings.volume);
    Ok(0)
}

/// Saves `settings` and gives the outputs their volume at once, without
/// waiting for audiod to see the file.
fn change(settings: &Settings) -> Result<(), Fail> {
    save(settings).map_err(|e| util::failed(&settings::PATH, e))?;
    for name in apply_volume(settings) {
        eprintln!("audio: could not set the volume of {name}");
    }
    Ok(())
}

/// `audio mute [on | off | toggle]`.
fn mute(how: Option<&str>) -> Result<i32, Fail> {
    let mut settings = load();
    settings.muted = match how.unwrap_or("toggle") {
        "on" => true,
        "off" => false,
        "toggle" => !settings.muted,
        other => return Err(usage(format!("mute takes on, off or toggle, not {other}"))),
    };
    change(&settings)?;
    println!("{}", if settings.muted { "muted" } else { "unmuted" });
    Ok(0)
}

/// `audio output [NAME]`.
fn output(name: Option<&str>) -> Result<i32, Fail> {
    let Some(name) = name else {
        println!("{}", default_output().unwrap_or_else(|| "none".to_string()));
        return Ok(0);
    };
    if !outputs().iter().any(|known| known == name) {
        return Err(fail(format!("no output {name}; `audio list` lists them")));
    }
    make_default(name)?;
    let mut settings = load();
    settings.output = Some(name.to_string());
    save(&settings).map_err(|e| util::failed(&settings::PATH, e))?;
    println!("{name}");
    Ok(0)
}

/// A player on an output: its format as the kernel took it.
struct Player {
    file: File,
    rate: u32,
    channels: usize,
}

impl Player {
    /// Opens `name` and asks for `rate` and `channels`, taking what the
    /// output gives nearest.
    fn open(name: &str, rate: u32, channels: usize) -> Result<Player, Fail> {
        let file = open(name)?;
        let failed = |e| util::failed(&name, e);
        int_request(&file, abi::SET_FORMAT, abi::FORMAT_S16_LE).map_err(failed)?;
        let channels = int_request(&file, abi::CHANNELS, channels as u32).map_err(failed)? as usize;
        let rate = int_request(&file, abi::SPEED, rate).map_err(failed)?;
        Ok(Player { file, rate, channels })
    }

    /// Plays `samples`, at the player's rate and channels.
    fn play(&mut self, samples: &[i16]) -> Result<(), Fail> {
        self.file.write_all(&convert::to_bytes(samples)).map_err(|e| util::failed(&"write", e))
    }

    /// Waits until everything written has been played.
    fn finish(&self) -> Result<(), Fail> {
        control(&self.file, abi::SYNC, &mut []).map_err(|e| util::failed(&"sync", e))
    }
}

/// `audio test [OUTPUT]`: 440 Hz on the left, then 660 Hz on the right.
fn test(name: Option<&str>) -> Result<i32, Fail> {
    let name = chosen(name)?;
    let mut player = Player::open(&name, 48000, 2)?;
    let frames = (player.rate * TEST_MS / 1000) as usize;
    let channels = player.channels;
    let sides: [(&str, f64, usize); 2] =
        [("left", tone::LEFT_FREQUENCY, 0), ("right", tone::RIGHT_FREQUENCY, 1)];
    for (side, frequency, channel) in sides {
        println!("{side}: {frequency} Hz on {name}");
        // Mono plays both on its one channel.
        let on = move |c: usize| c == channel || channels == 1;
        let samples = tone::sine(frequency, player.rate, channels, frames, TEST_AMPLITUDE, &on);
        player.play(&samples)?;
        // A tenth of a second of silence between the two.
        player.play(&vec![0; (player.rate as usize / 10) * channels])?;
    }
    player.finish()?;
    Ok(0)
}

/// `audio play [--output NAME] FILE...`.
fn play(operands: &[String]) -> Result<i32, Fail> {
    let mut name = None;
    let mut files = Vec::new();
    let mut items = operands.iter();
    while let Some(item) = items.next() {
        if item == "--output" || item == "-o" {
            name = Some(
                items.next().ok_or_else(|| usage("--output needs a name".to_string()))?.clone(),
            );
        } else if let Some(value) = item.strip_prefix("--output=") {
            name = Some(value.to_string());
        } else {
            files.push(item.clone());
        }
    }
    if files.is_empty() {
        return Err(usage("play needs a WAV file".to_string()));
    }
    let name = chosen(name.as_deref())?;
    for path in files {
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
            .map_err(|e| util::failed(&path, e))?;
        let wav = wav::parse(&bytes).map_err(|e| fail(format!("{path}: {e}")))?;
        let from = usize::from(wav.channels);
        let mut player = Player::open(&name, wav.rate, from)?;
        let mut resampler = resample::Resampler::new(wav.rate, player.rate, player.channels);
        // A second at a time.
        let frame = from * wav.encoding.bytes();
        for chunk in wav.data.chunks(frame * wav.rate as usize) {
            let mut samples = Vec::new();
            convert::to_s16(chunk, wav.encoding, &mut samples);
            let mapped = convert::channels(&samples, from, player.channels);
            let mut out = Vec::new();
            resampler.process(&mapped, &mut out);
            player.play(&out)?;
        }
        player.finish()?;
    }
    Ok(0)
}

/// audio(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = args.subcommand().map(|c| c.to_string_lossy().into_owned());
    let rest: Vec<String> = args.rest().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if rest.iter().any(|a| a == "--help" || a == "-h") || command.as_deref() == Some("help") {
        println!("{USAGE}");
        return Ok(0);
    }
    let first = rest.first().map(String::as_str);
    let at_most_one = || {
        if rest.len() > 1 {
            return Err(usage(format!("unexpected argument '{}'", rest[1])));
        }
        Ok(())
    };
    match command.as_deref() {
        None | Some("list") => {
            at_most_one()?;
            list()
        }
        Some("info") => {
            at_most_one()?;
            show_info(first)
        }
        Some("volume") => {
            at_most_one()?;
            volume(first)
        }
        Some("mute") => {
            at_most_one()?;
            mute(first)
        }
        Some("output") => {
            at_most_one()?;
            output(first)
        }
        Some("test") => {
            at_most_one()?;
            test(first)
        }
        Some("play") => play(&rest),
        Some(other) => Err(usage(format!("unknown command '{other}'\n{USAGE}"))),
    }
}

/// What audiod keeps between looks.
struct Daemon {
    settings: Settings,
    /// When the settings file last changed, as it was last read.
    modified: Option<SystemTime>,
    /// The outputs at the last look, and the order they came in.
    known: BTreeSet<String>,
    arrived: Vec<String>,
    /// The output `default` was last pointed at.
    current: Option<String>,
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(settings::PATH).and_then(|m| m.modified()).ok()
}

impl Daemon {
    /// The output to play through: the one the settings name if it is
    /// there, else the one that came last (a headset plugged in), else
    /// the first.
    fn choose(&self) -> Option<String> {
        if let Some(name) = &self.settings.output
            && self.known.contains(name)
        {
            return Some(name.clone());
        }
        self.arrived.last().cloned().or_else(|| self.known.iter().next().cloned())
    }

    /// One look: new or gone outputs, a changed file.
    fn look(&mut self) {
        let now: BTreeSet<String> = outputs().into_iter().collect();
        let came: Vec<String> = now.difference(&self.known).cloned().collect();
        let went = self.known.difference(&now).count();
        let first = self.known.is_empty() && self.current.is_none();
        self.arrived.retain(|name| now.contains(name));
        if !first {
            // Outputs there at the start are no newer than each other.
            self.arrived.extend(came.iter().cloned());
        }
        self.known = now;
        let changed = modified() != self.modified;
        if changed {
            self.modified = modified();
            self.settings = load();
        }
        if came.is_empty() && went == 0 && !changed {
            return;
        }
        for name in apply_volume(&self.settings) {
            println!("could not set the volume of {name}");
        }
        let chosen = self.choose();
        if chosen != self.current {
            if let Some(name) = &chosen {
                match make_default(name) {
                    Ok(()) => println!(
                        "playing through {name} at {}%{}",
                        self.settings.volume,
                        if self.settings.muted { ", muted" } else { "" }
                    ),
                    Err(err) => {
                        println!("could not make {name} the default output: {}", err.message);
                    }
                }
            }
            self.current = chosen;
        } else if changed {
            println!(
                "volume {}%{}",
                self.settings.volume,
                if self.settings.muted { ", muted" } else { "" }
            );
        }
    }
}

/// The volume keys: each changes the settings, which are saved and given
/// to the outputs at once.
fn keys(daemon: &Mutex<Daemon>) {
    let Ok(mut keyboard) = File::open("/devices/kbd") else {
        println!("cannot read /devices/kbd: the volume keys do nothing");
        return;
    };
    let mut events = [0u8; input::Event::SIZE * 16];
    loop {
        let Ok(read) = keyboard.read(&mut events) else {
            // Events lost to a reader too slow: read on from the oldest.
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        for event in events[..read].as_chunks::<{ input::Event::SIZE }>().0 {
            let code = u16::from_le_bytes([
                event[input::Event::OFFSET_CODE],
                event[input::Event::OFFSET_CODE + 1],
            ]);
            let value = u16::from_le_bytes([
                event[input::Event::OFFSET_VALUE],
                event[input::Event::OFFSET_VALUE + 1],
            ]);
            if value == input::RELEASED {
                continue;
            }
            let Ok(mut daemon) = daemon.lock() else { return };
            let s = &mut daemon.settings;
            match code {
                input::KEY_VOLUMEUP => {
                    s.volume = s.volume.saturating_add(VOLUME_STEP).min(100);
                    s.muted = false;
                }
                input::KEY_VOLUMEDOWN => {
                    s.volume = s.volume.saturating_sub(VOLUME_STEP);
                    s.muted = false;
                }
                input::KEY_MUTE if value == input::PRESSED => s.muted = !s.muted,
                _ => continue,
            }
            let settings = daemon.settings.clone();
            if let Err(err) = save(&settings) {
                println!("cannot save {}: {}", settings::PATH, util::strip_os_error(&err));
            }
            daemon.modified = modified();
            for name in apply_volume(&settings) {
                println!("could not set the volume of {name}");
            }
            println!("volume {}%{}", settings.volume, if settings.muted { ", muted" } else { "" });
        }
    }
}

/// audiod(8).
pub fn daemon(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let daemon = Daemon {
        settings: load(),
        modified: modified(),
        known: BTreeSet::new(),
        arrived: Vec::new(),
        current: None,
    };
    let daemon = Arc::new(Mutex::new(daemon));
    {
        let mut d = daemon.lock().map_err(|_| fail("poisoned".to_string()))?;
        // The first look applies everything.
        d.modified = None;
        d.look();
        if d.known.is_empty() {
            println!("no sound output yet; looking every {} seconds", LOOK.as_secs());
        }
    }
    let for_keys = daemon.clone();
    std::thread::Builder::new()
        .name("keys".to_string())
        .spawn(move || keys(&for_keys))
        .map_err(|e| util::failed(&"thread", e))?;
    loop {
        std::thread::sleep(LOOK);
        let Ok(mut d) = daemon.lock() else { return Err(fail("poisoned".to_string())) };
        d.look();
    }
}
