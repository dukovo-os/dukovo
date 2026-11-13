// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screen reader's voice (ADR 0324): readerd(8), which speaks what
//! consoled(8) gives it through eSpeak NG on _/devices/audio/speech_, and
//! reader(1), which turns it on and off and sets its voice, rate and
//! pitch in reader.toml(5).
//!
//! readerd listens on [`SOCKET`], which only root may open, for lines of
//! UTF-8:
//!
//! - `say TEXT`: speak TEXT after what is still to be said;
//! - `now TEXT`: stop speaking, forget what was still to be said, and
//!   speak TEXT;
//! - `trusted TEXT`: as `now`, after the trusted sound, two rising tones
//!   that nothing else plays: consoled sends its own prompts so, which no
//!   program draws, so that a prompt a program draws to look like one
//!   never sounds like one;
//! - `stop`: stop speaking and forget what was still to be said;
//! - `faster` and `slower`: change the rate by [`RATE_STEP`] words a
//!   minute until readerd starts again, and say the new rate;
//! - `reload`: read reader.toml again;
//! - `status`: answer one line, `voice V rate R pitch P output O`.
//!
//! What is said is spoken by espeak-ng(1), run for each line: the text
//! goes to its standard input and the speech comes back on its standard
//! output as a WAV stream (16-bit mono, 22,050 frames a second), which
//! readerd converts to the output's rate and channels and writes to the
//! speech file a few milliseconds at a time. Speech takes the output from
//! any other player (ADR 0320). `now`, `trusted` and `stop` end the
//! espeak-ng that speaks and drop what the output has queued (HALT), so
//! the speech stops at once. After a second with nothing to say readerd
//! closes the speech file, which gives the output back.
//!
//! readerd runs when reader.toml says `on = true` or the kernel has the
//! flag `speech` (the install stick's "Install with speech"), and ends
//! with 0 otherwise, which leaves its service stopped. With the kernel
//! flag `reader-log`, for the boot tests, it writes each thing it says to
//! its log; otherwise it never does, since the screen's text is nobody
//! else's business.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use audio::settings::{entry, string};
use audio::{convert, resample, tone, wav};
use radix_abi::audio as abi;

use crate::args::Args;
use crate::sound::{control, int_request};
use crate::util::{self, Fail, fail, usage};

/// Where readerd listens.
pub const SOCKET: &str = "/system/live/readerd/socket";

/// Where the settings are: state, as audio.toml(5) is, so that readerd
/// may keep what the keys change.
pub const PATH: &str = "/system/state/reader/reader.toml";

/// What a new settings file starts with.
const HEADER: &str = "# The screen reader (reader.toml(5)): whether it starts with the system,\n\
                      # eSpeak NG's voice (espeak-ng --voices lists them), the rate in words\n\
                      # a minute, 80 to 450, and the pitch, 0 to 99. reader(1) changes it;\n\
                      # readerd(8) reads it.\n";

/// The speech file, which takes the output from any other player.
const SPEECH: &str = "/devices/audio/speech";

/// The speech synthesizer.
const ESPEAK: &str = "/system/commands/espeak-ng";

/// The rate the outputs are asked for, and the channels: the output
/// gives the nearest it has.
const OUTPUT_RATE: u32 = 48_000;
const OUTPUT_CHANNELS: usize = 2;

/// How long readerd keeps the speech file with nothing to say.
const IDLE: Duration = Duration::from_secs(1);

/// The speech written to the output at a time, in milliseconds: short,
/// so that a stop is heard at once.
const CHUNK_MS: u32 = 20;

/// The most bytes of a line readerd reads; a longer one is dropped.
const LINE_MAX: usize = 4096;

/// The most utterances waiting: past it, the oldest plain ones go, so
/// that a flood of output cannot keep the reader talking for minutes,
/// and what was said last is what is heard.
const WAITING_MAX: usize = 200;

/// How much `faster` and `slower` change the rate, in words a minute.
pub const RATE_STEP: u16 = 25;

/// The rates and pitches eSpeak NG takes.
const RATES: std::ops::RangeInclusive<u16> = 80..=450;
const PITCHES: std::ops::RangeInclusive<u8> = 0..=99;

/// The trusted sound: two tones a fifth apart, rising.
const TRUSTED_TONES: [f64; 2] = [660.0, 990.0];
const TRUSTED_TONE_MS: u32 = 70;
const TRUSTED_AMPLITUDE: f64 = 0.3;

/// The settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Whether readerd starts with the system.
    pub on: bool,
    /// eSpeak NG's voice: a language, with a variant after `+`.
    pub voice: String,
    /// Words a minute.
    pub rate: u16,
    /// 0 to 99, 50 by default.
    pub pitch: u8,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { on: false, voice: "en".to_string(), rate: 175, pitch: 50 }
    }
}

/// Whether `voice` is a voice's name: letters, digits, `-`, `_` and `+`,
/// and not an option.
fn is_voice(voice: &str) -> bool {
    !voice.is_empty()
        && !voice.starts_with('-')
        && voice.len() <= 64
        && voice.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+'))
}

impl Settings {
    /// Reads the settings from `text`: values missing or wrong are the
    /// defaults.
    pub fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for (key, value) in text.lines().filter_map(entry) {
            match key {
                "on" => settings.on = value == "true",
                "voice" => {
                    if let Some(voice) = string(value).filter(|v| is_voice(v)) {
                        settings.voice = voice;
                    }
                }
                "rate" => {
                    if let Ok(rate) = value.parse::<u16>()
                        && RATES.contains(&rate)
                    {
                        settings.rate = rate;
                    }
                }
                "pitch" => {
                    if let Ok(pitch) = value.parse::<u8>()
                        && PITCHES.contains(&pitch)
                    {
                        settings.pitch = pitch;
                    }
                }
                _ => {}
            }
        }
        settings
    }

    /// `text` with these settings in it: each key's line changed where it
    /// is, those it lacks added at the end, and the rest kept.
    pub fn update(&self, text: &str) -> String {
        let values = [
            ("on", self.on.to_string()),
            ("voice", format!("\"{}\"", self.voice)),
            ("rate", self.rate.to_string()),
            ("pitch", self.pitch.to_string()),
        ];
        let mut seen = [false; 4];
        let mut out = String::new();
        for line in text.lines() {
            let replaced = entry(line).and_then(|(key, _)| {
                let i = values.iter().position(|(name, _)| *name == key)?;
                seen[i] = true;
                Some(format!("{key} = {}", values[i].1))
            });
            out.push_str(replaced.as_deref().unwrap_or(line));
            out.push('\n');
        }
        for ((key, value), seen) in values.iter().zip(seen) {
            if !seen {
                out.push_str(&format!("{key} = {value}\n"));
            }
        }
        out
    }
}

/// Reads the settings: the defaults if the file is missing.
fn load() -> Settings {
    std::fs::read_to_string(PATH)
        .map_or_else(|_| Settings::default(), |text| Settings::parse(&text))
}

/// Writes `settings` over the file, keeping its notes: a new file put in
/// the old one's place, so that readerd never reads half of one.
fn save(settings: &Settings) -> io::Result<()> {
    let old = std::fs::read_to_string(PATH).unwrap_or_else(|_| HEADER.to_string());
    let dir = std::path::Path::new(PATH).parent().unwrap_or(std::path::Path::new("/"));
    std::fs::create_dir_all(dir)?;
    let new = format!("{PATH}.new");
    std::fs::write(&new, settings.update(&old))?;
    std::fs::rename(&new, PATH)
}

/// What a line asks of readerd.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Say(String),
    Now(String),
    Trusted(String),
    Stop,
    Faster,
    Slower,
    Reload,
    Status,
}

/// `text` as readerd speaks it: control characters as spaces, and spaces
/// run together.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

impl Line {
    /// Reads one line, without its newline. None if it is not one of
    /// readerd's.
    pub fn parse(line: &str) -> Option<Line> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let text = || Some(clean(rest)).filter(|t| !t.is_empty());
        Some(match word {
            "say" => Line::Say(text()?),
            "now" => Line::Now(text()?),
            "trusted" => Line::Trusted(text()?),
            "stop" => Line::Stop,
            "faster" => Line::Faster,
            "slower" => Line::Slower,
            "reload" => Line::Reload,
            "status" => Line::Status,
            _ => return None,
        })
    }
}

/// Something to say, and whether the trusted sound goes before it.
#[derive(Clone, Debug)]
struct Utterance {
    text: String,
    trusted: bool,
}

/// What the socket's thread and the speaker share.
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// Counts the stops: the speaker stops what it says when it changes.
    stops: AtomicU64,
}

struct Queue {
    waiting: VecDeque<Utterance>,
    settings: Settings,
    /// The output the speech last played on, for `status`.
    output: String,
}

impl Shared {
    /// Stops what is being said and forgets what was to be said.
    fn stop(&self, queue: &mut Queue) {
        queue.waiting.clear();
        self.stops.fetch_add(1, Ordering::SeqCst);
    }

    /// Hands `line` on: what to say to the speaker, the rest done here.
    /// Returns the answer to `status`.
    fn take(&self, line: Line) -> Option<String> {
        let Ok(mut queue) = self.queue.lock() else { return None };
        match line {
            Line::Say(text) => {
                if queue.waiting.len() >= WAITING_MAX
                    && let Some(oldest) = queue.waiting.iter().position(|u| !u.trusted)
                {
                    queue.waiting.remove(oldest);
                }
                if queue.waiting.len() < WAITING_MAX {
                    queue.waiting.push_back(Utterance { text, trusted: false });
                }
            }
            Line::Now(text) => {
                self.stop(&mut queue);
                queue.waiting.push_back(Utterance { text, trusted: false });
            }
            Line::Trusted(text) => {
                self.stop(&mut queue);
                queue.waiting.push_back(Utterance { text, trusted: true });
            }
            Line::Stop => self.stop(&mut queue),
            Line::Faster | Line::Slower => {
                let rate = queue.settings.rate;
                let rate = if line == Line::Faster {
                    rate.saturating_add(RATE_STEP)
                } else {
                    rate.saturating_sub(RATE_STEP)
                };
                queue.settings.rate = rate.clamp(*RATES.start(), *RATES.end());
                // Kept for the next boot, with what else the file says;
                // on a stick whose state cannot be written, for this one.
                let rate = queue.settings.rate;
                let _ = save(&Settings { rate, ..load() });
                let said = format!("rate {rate}");
                self.stop(&mut queue);
                queue.waiting.push_back(Utterance { text: said, trusted: false });
            }
            Line::Reload => {
                let on = queue.settings.on;
                queue.settings = Settings { on, ..load() };
            }
            Line::Status => {
                let s = &queue.settings;
                return Some(format!(
                    "voice {} rate {} pitch {} output {}\n",
                    s.voice, s.rate, s.pitch, queue.output
                ));
            }
        }
        self.ready.notify_one();
        None
    }

    /// The next thing to say, waiting up to [`IDLE`] for one; with the
    /// settings to say it with and the stop count it belongs to.
    fn next(&self) -> Option<(Utterance, Settings, u64)> {
        let mut queue = self.queue.lock().ok()?;
        if queue.waiting.is_empty() {
            queue = self.ready.wait_timeout(queue, IDLE).ok()?.0;
        }
        let utterance = queue.waiting.pop_front()?;
        Some((utterance, queue.settings.clone(), self.stops.load(Ordering::SeqCst)))
    }
}

/// The speech file, set up for 16-bit samples at the rate and channels
/// the output took.
struct Speech {
    file: File,
    rate: u32,
    channels: usize,
}

impl Speech {
    fn open() -> io::Result<Speech> {
        let file = OpenOptions::new().read(true).write(true).open(SPEECH)?;
        int_request(&file, abi::SET_FORMAT, abi::FORMAT_S16_LE)?;
        let channels = int_request(&file, abi::CHANNELS, OUTPUT_CHANNELS as u32)? as usize;
        let rate = int_request(&file, abi::SPEED, OUTPUT_RATE)?;
        Ok(Speech { file, rate, channels: channels.max(1) })
    }

    /// Drops what the output has queued and not played.
    fn halt(&self) {
        let _ = control(&self.file, abi::HALT, &mut []);
    }

    fn play(&mut self, samples: &[i16]) -> io::Result<()> {
        self.file.write_all(&convert::to_bytes(samples))
    }
}

/// The output the speech file plays on, by name, as GET_INFO on it says.
fn output_name(speech: &Speech) -> String {
    crate::sound::info(&speech.file).map_or_else(|_| "none".to_string(), |info| info.name)
}

/// Runs espeak-ng to say `text` with `settings`, its speech on a pipe.
fn synthesize(text: &str, settings: &Settings) -> io::Result<Child> {
    let mut child = Command::new(ESPEAK)
        .args(["--stdout", "--stdin", "-v", &settings.voice])
        .args(["-s", &settings.rate.to_string(), "-p", &settings.pitch.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    // A line is short, so the pipe takes all of it before espeak-ng reads.
    if let Some(mut input) = child.stdin.take() {
        input.write_all(text.as_bytes())?;
    }
    Ok(child)
}

/// The speaker's thread: says each utterance in turn, and closes the
/// speech file when it has had nothing to say for [`IDLE`].
fn speaker(shared: &Shared, log: bool) {
    let mut speech: Option<Speech> = None;
    // Whether the last attempt to open the speech file failed, so that
    // the log says so once.
    let mut silent = false;
    loop {
        let Some((utterance, settings, stops)) = shared.next() else {
            speech = None;
            continue;
        };
        if speech.is_none() {
            match Speech::open() {
                Ok(opened) => {
                    let output = output_name(&opened);
                    if silent {
                        println!("speaking again, on {output}");
                    }
                    silent = false;
                    if let Ok(mut queue) = shared.queue.lock() {
                        queue.output = output;
                    }
                    speech = Some(opened);
                }
                Err(err) => {
                    if !silent {
                        println!(
                            "no sound output to speak on ({err}); trying again with the next thing to say"
                        );
                    }
                    silent = true;
                    continue;
                }
            }
        }
        let Some(out) = speech.as_mut() else { continue };
        if log {
            let trusted = if utterance.trusted { " (trusted)" } else { "" };
            println!("says{trusted}: {}", utterance.text);
        }
        if let Err(err) = say(out, shared, &utterance, &settings, stops) {
            println!("cannot speak: {err}");
            // The output went away, or the file was taken: open it again
            // for the next.
            speech = None;
        }
    }
}

/// Says `utterance` on `out`, until a stop comes after `stops`.
fn say(
    out: &mut Speech,
    shared: &Shared,
    utterance: &Utterance,
    settings: &Settings,
    stops: u64,
) -> io::Result<()> {
    let stopped = || shared.stops.load(Ordering::SeqCst) != stops;
    if utterance.trusted {
        for frequency in TRUSTED_TONES {
            let frames = (out.rate * TRUSTED_TONE_MS / 1000) as usize;
            out.play(&tone::sine(
                frequency,
                out.rate,
                out.channels,
                frames,
                TRUSTED_AMPLITUDE,
                &|_| true,
            ))?;
        }
    }
    let mut child = synthesize(&utterance.text, settings)?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(());
    };
    let (rate, channels) = (out.rate, out.channels);
    let result = stream(&mut stdout, rate, channels, &mut |samples| out.play(samples), &stopped);
    if result.is_err() || stopped() {
        let _ = child.kill();
        out.halt();
    }
    let _ = child.wait();
    result
}

/// Plays the WAV stream `input` as it comes, converted to `rate` frames a
/// second of `channels` channels and handed to `play` [`CHUNK_MS`] at a
/// time, until it ends or `stopped`.
fn stream(
    input: &mut impl Read,
    rate: u32,
    channels: usize,
    play: &mut dyn FnMut(&[i16]) -> io::Result<()>,
    stopped: &dyn Fn() -> bool,
) -> io::Result<()> {
    let mut buffer = vec![0u8; 4096];
    // The stream's start, until its header is whole.
    let mut head = Vec::new();
    // Its channels, the bytes of a chunk, and the converter, once known.
    let mut format: Option<(usize, usize, resample::Resampler)> = None;
    // Bytes come and not yet played.
    let mut pending = Vec::new();
    let mut converted = Vec::new();
    loop {
        if stopped() {
            return Ok(());
        }
        let n = match input.read(&mut buffer) {
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let ended = n == 0;
        if format.is_none() {
            if ended {
                return Ok(());
            }
            head.extend_from_slice(&buffer[..n]);
            let parsed = match wav::parse(&head) {
                Ok(parsed) => parsed,
                Err(_) if head.len() <= 1024 => continue,
                Err(_) => {
                    return Err(io::Error::new(ErrorKind::InvalidData, "espeak-ng wrote no WAV"));
                }
            };
            if parsed.encoding != wav::Encoding::S16 {
                let why = "espeak-ng wrote no 16-bit samples";
                return Err(io::Error::new(ErrorKind::InvalidData, why));
            }
            let from = usize::from(parsed.channels);
            let chunk = (parsed.rate * CHUNK_MS / 1000) as usize * from * 2;
            let resampler = resample::Resampler::new(parsed.rate, rate, channels);
            let start = parsed.data.as_ptr() as usize - head.as_ptr() as usize;
            pending = head.split_off(start);
            format = Some((from, chunk, resampler));
        } else {
            pending.extend_from_slice(&buffer[..n]);
        }
        let Some((from, chunk, resampler)) = format.as_mut() else { continue };
        let frame = *from * 2;
        // Whole chunks, and at the end the whole frames left.
        loop {
            let take = if pending.len() >= *chunk {
                *chunk
            } else if ended {
                pending.len() / frame * frame
            } else {
                0
            };
            if take == 0 {
                break;
            }
            let samples = convert::from_bytes(&pending[..take]);
            pending.drain(..take);
            let mapped = convert::channels(&samples, *from, channels);
            converted.clear();
            resampler.process(&mapped, &mut converted);
            play(&converted)?;
            if stopped() {
                return Ok(());
            }
        }
        if ended {
            return Ok(());
        }
    }
}

/// A connection to readerd, and what it has sent of a line.
struct Client {
    stream: UnixStream,
    line: Vec<u8>,
    /// The line is too long, and is dropped up to its end.
    dropping: bool,
    done: bool,
}

impl Client {
    /// Reads what came, handing each whole line to `shared`.
    fn read(&mut self, shared: &Shared) {
        let mut buffer = [0u8; 4096];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    self.done = true;
                    return;
                }
                Ok(n) => {
                    for &byte in &buffer[..n] {
                        self.byte(byte, shared);
                    }
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) if err.kind() == ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.done = true;
                    return;
                }
            }
        }
    }

    fn byte(&mut self, byte: u8, shared: &Shared) {
        if byte != b'\n' {
            if self.line.len() < LINE_MAX {
                self.line.push(byte);
            } else {
                self.dropping = true;
            }
            return;
        }
        let line = std::mem::take(&mut self.line);
        if std::mem::take(&mut self.dropping) {
            return;
        }
        let parsed = std::str::from_utf8(&line).ok().and_then(Line::parse);
        if let Some(answer) = parsed.and_then(|line| shared.take(line)) {
            let _ = self.stream.set_nonblocking(false);
            let _ = self.stream.write_all(answer.as_bytes());
            let _ = self.stream.set_nonblocking(true);
        }
    }
}

/// readerd(8).
pub fn daemon(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let settings = load();
    let speech_flag = util::kernel_flag("speech");
    if !settings.on && !speech_flag {
        println!("the screen reader is off (reader.toml(5)); `reader on` turns it on");
        return Ok(0);
    }
    let log = util::kernel_flag("reader-log");
    let dir = std::path::Path::new(SOCKET).parent().unwrap_or(std::path::Path::new("/"));
    std::fs::create_dir_all(dir).map_err(|e| util::failed(&dir.display(), e))?;
    let _ = std::fs::remove_file(SOCKET);
    let listener = UnixListener::bind(SOCKET).map_err(|e| util::failed(&SOCKET, e))?;
    std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| util::failed(&SOCKET, e))?;
    listener.set_nonblocking(true).map_err(|e| util::failed(&SOCKET, e))?;

    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            waiting: VecDeque::new(),
            settings: settings.clone(),
            // Named once the speech file is open, for the first thing
            // said.
            output: "none".to_string(),
        }),
        ready: Condvar::new(),
        stops: AtomicU64::new(0),
    });
    let for_speaker = shared.clone();
    std::thread::Builder::new()
        .name("speaker".to_string())
        .spawn(move || speaker(&for_speaker, log))
        .map_err(|e| util::failed(&"thread", e))?;
    println!(
        "speaking with eSpeak NG's {} voice at {} words a minute{}",
        settings.voice,
        settings.rate,
        if settings.on { "" } else { ", for the kernel flag speech" }
    );

    let mut clients: Vec<Client> = Vec::new();
    loop {
        let mut fds =
            vec![libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        fds.extend(clients.iter().map(|c| libc::pollfd {
            fd: c.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }));
        // SAFETY: `fds` holds valid pollfds.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                return Err(util::failed(&"poll", error));
            }
            continue;
        }
        for (client, fd) in clients.iter_mut().zip(&fds[1..]) {
            if fd.revents != 0 {
                client.read(&shared);
            }
        }
        clients.retain(|c| !c.done);
        if fds[0].revents != 0 {
            while let Ok((stream, _)) = listener.accept() {
                if stream.set_nonblocking(true).is_ok() {
                    clients.push(Client { stream, line: Vec::new(), dropping: false, done: false });
                }
            }
        }
    }
}

/// How long readerd has to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(2);

/// Sends `line` to readerd.
fn send(line: &str) -> Result<(), Fail> {
    let mut stream = UnixStream::connect(SOCKET)
        .map_err(|_| fail("readerd is not running: `reader on` starts it".to_string()))?;
    stream.write_all(format!("{line}\n").as_bytes()).map_err(|e| util::failed(&SOCKET, e))
}

/// Asks readerd how it speaks: its answer, without the line's end.
fn ask_status() -> io::Result<String> {
    let mut stream = UnixStream::connect(SOCKET)?;
    stream.write_all(b"status\n")?;
    // By poll: Radix's Unix sockets take no SO_RCVTIMEO.
    let until = std::time::Instant::now() + ANSWER_WAIT;
    let mut answer = Vec::new();
    let mut buffer = [0u8; 256];
    while !answer.ends_with(b"\n") {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() || !crate::wireguard::readable(&stream, left)? {
            return Err(ErrorKind::TimedOut.into());
        }
        match stream.read(&mut buffer)? {
            0 => return Err(ErrorKind::UnexpectedEof.into()),
            n => answer.extend_from_slice(&buffer[..n]),
        }
    }
    answer.pop();
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// Runs `service readerd COMMAND --quiet`, which returns once readerd
/// has started or stopped, and says only what went wrong.
fn service(command: &str) -> Result<(), Fail> {
    let status = Command::new("/system/commands/service")
        .args(["readerd", command, "--quiet"])
        .status()
        .map_err(|e| util::failed(&"service", e))?;
    if status.success() { Ok(()) } else { Err(fail(format!("`service readerd {command}` failed"))) }
}

const USAGE: &str = "\
Usage: reader [status]
       reader on | off
       reader voice [NAME]
       reader rate [WORDS]
       reader pitch [PITCH]
       reader say TEXT...
       reader stop

Turns the screen reader on and off, and sets its voice (as espeak-ng
--voices names them), its rate (80 to 450 words a minute) and its pitch
(0 to 99) in /system/state/reader/reader.toml (reader(1)).";

/// reader(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = args.subcommand().map(|c| c.to_string_lossy().into_owned());
    let rest: Vec<String> = args.rest().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if rest.iter().any(|a| a == "--help" || a == "-h") || command.as_deref() == Some("help") {
        println!("{USAGE}");
        return Ok(0);
    }
    let first = rest.first().map(String::as_str);
    if rest.len() > 1 && command.as_deref() != Some("say") {
        return Err(usage(format!("unexpected argument '{}'", rest[1])));
    }
    let mut settings = load();
    let saved = |settings: &Settings| -> Result<(), Fail> {
        save(settings).map_err(|e| util::failed(&PATH, e))?;
        // A running readerd reads it again; one that is not is told
        // nothing.
        let _ = send("reload");
        Ok(())
    };
    match command.as_deref() {
        None | Some("status") => {
            let running = ask_status();
            println!("Screen reader: {}", if settings.on { "on" } else { "off" });
            println!("Voice:         {}", settings.voice);
            println!("Rate:          {} words a minute", settings.rate);
            println!("Pitch:         {}", settings.pitch);
            match running {
                Ok(line) => println!("readerd:       running ({line})"),
                Err(e)
                    if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) =>
                {
                    println!("readerd:       not running");
                }
                Err(e) => println!("readerd:       not answering ({e})"),
            }
        }
        Some("on") => {
            settings.on = true;
            saved(&settings)?;
            service("start")?;
            println!("on");
        }
        Some("off") => {
            settings.on = false;
            saved(&settings)?;
            service("stop")?;
            println!("off");
        }
        Some("voice") => match first {
            None => println!("{}", settings.voice),
            Some(voice) if is_voice(voice) => {
                settings.voice = voice.to_string();
                saved(&settings)?;
                println!("{voice}");
            }
            Some(voice) => return Err(usage(format!("not a voice's name: {voice}"))),
        },
        Some("rate") => match first {
            None => println!("{}", settings.rate),
            Some(rate) => {
                let rate =
                    rate.parse::<u16>().ok().filter(|r| RATES.contains(r)).ok_or_else(|| {
                        usage(format!("the rate is 80 to 450 words a minute, not {rate}"))
                    })?;
                settings.rate = rate;
                saved(&settings)?;
                println!("{rate}");
            }
        },
        Some("pitch") => match first {
            None => println!("{}", settings.pitch),
            Some(pitch) => {
                let pitch = pitch
                    .parse::<u8>()
                    .ok()
                    .filter(|p| PITCHES.contains(p))
                    .ok_or_else(|| usage(format!("the pitch is 0 to 99, not {pitch}")))?;
                settings.pitch = pitch;
                saved(&settings)?;
                println!("{pitch}");
            }
        },
        Some("say") => {
            let text = clean(&rest.join(" "));
            if text.is_empty() {
                return Err(usage("say needs something to say".to_string()));
            }
            send(&format!("say {text}"))?;
        }
        Some("stop") => {
            send("stop")?;
        }
        Some(other) => return Err(usage(format!("unknown command '{other}'; try reader --help"))),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_keeps_what_was_said_last_and_every_trusted_line() {
        let shared = Shared {
            queue: Mutex::new(Queue {
                waiting: VecDeque::new(),
                settings: Settings::default(),
                output: String::new(),
            }),
            ready: Condvar::new(),
            stops: AtomicU64::new(0),
        };
        shared.take(Line::Trusted("Password for elevate".to_string()));
        for n in 0..WAITING_MAX * 2 {
            shared.take(Line::Say(format!("line {n}")));
        }
        let queue = shared.queue.lock().unwrap();
        assert_eq!(queue.waiting.len(), WAITING_MAX);
        assert!(queue.waiting[0].trusted);
        assert_eq!(queue.waiting.back().unwrap().text, format!("line {}", WAITING_MAX * 2 - 1));
    }

    #[test]
    fn settings_are_read_and_kept_with_their_notes() {
        let text = "# notes\non = true\nvoice = \"en-us+f3\"\nrate = 300 # quick\nother = 1\n";
        let settings = Settings::parse(text);
        assert_eq!(
            settings,
            Settings { on: true, voice: "en-us+f3".to_string(), rate: 300, pitch: 50 }
        );
        let changed = Settings { rate: 200, ..settings }.update(text);
        assert_eq!(
            changed,
            "# notes\non = true\nvoice = \"en-us+f3\"\nrate = 200\nother = 1\npitch = 50\n"
        );
    }

    #[test]
    fn wrong_values_are_the_defaults() {
        let settings = Settings::parse("voice = \"--help\"\nrate = 9000\npitch = 120\non = yes\n");
        assert_eq!(settings, Settings::default());
        assert!(!is_voice("en us"));
        assert!(!is_voice("-v"));
        assert!(is_voice("de"));
    }

    #[test]
    fn lines_are_read_and_their_text_cleaned() {
        assert_eq!(Line::parse("say  Hello,\tworld "), Some(Line::Say("Hello, world".to_string())));
        assert_eq!(
            Line::parse("trusted Password for elevate:"),
            Some(Line::Trusted("Password for elevate:".to_string()))
        );
        assert_eq!(Line::parse("now \u{1b}[1mBold"), Some(Line::Now("[1mBold".to_string())));
        assert_eq!(Line::parse("stop"), Some(Line::Stop));
        assert_eq!(Line::parse("faster"), Some(Line::Faster));
        // Nothing to say, or not a line of readerd's.
        assert_eq!(Line::parse("say    "), None);
        assert_eq!(Line::parse("shout hello"), None);
    }

    /// Hands out a few bytes a read, as a pipe may.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let n = self.0.len().min(buffer.len()).min(7);
            buffer[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_stream_is_played_in_the_output_format_until_stopped() {
        // A second at 22,050 frames a second, mono, as espeak-ng writes
        // it, with the length it gives while it does not know it.
        let mut wav = wav::header(22_050, 1, 0).to_vec();
        wav[40..44].copy_from_slice(&0x7fff_f000u32.to_le_bytes());
        let samples: Vec<i16> = (0..22_050).map(|i| ((i % 50) * 100) as i16).collect();
        wav.extend_from_slice(&convert::to_bytes(&samples));

        let mut played = Vec::new();
        let mut calls = 0;
        let mut play = |s: &[i16]| {
            played.extend_from_slice(s);
            calls += 1;
            Ok(())
        };
        stream(&mut Trickle(&wav), 48_000, 2, &mut play, &|| false).unwrap();
        // About a second at 48,000 on both channels, in 20 ms chunks.
        let frames = played.len() / 2;
        assert!((47_900..=48_100).contains(&frames), "{frames} frames");
        assert!(played.chunks(2).all(|f| f[0] == f[1]));
        assert!((50..=51).contains(&calls), "{calls} chunks");

        // Stopped: nothing is played.
        let mut none = 0;
        let mut count = |s: &[i16]| {
            none += s.len();
            Ok(())
        };
        stream(&mut &wav[..], 48_000, 2, &mut count, &|| true).unwrap();
        assert_eq!(none, 0);

        // Not a WAV stream.
        let noise = vec![1u8; 2000];
        let mut ignore = |_: &[i16]| Ok(());
        assert!(stream(&mut &noise[..], 48_000, 2, &mut ignore, &|| false).is_err());
    }
}
