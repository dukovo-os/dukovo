// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukofetch(1): the system at a glance, beside Duke, as neofetch shows
//! Linux's (ADR 0363). Every line comes from Radix's own sources: the
//! kernel's version, the uptime and memory in `/processes`, CPUID and
//! `/kernel/cpus`, the PCI functions and the PCI ID database, the mounts,
//! the network interfaces, and bundled(8)'s list. A line whose source is
//! missing is left out, so nothing shows blank or made up.
//!
//! Duke is drawn in half blocks, two pixels to a character cell, in true
//! colour where COLORTERM says the terminal has it and in the nearest of
//! xterm's 256 colours elsewhere: 16 rows high on a terminal 80 columns
//! wide or more, and 10 rows high, beside fewer lines, below that. With
//! NO_COLOR set, TERM dumb, or the output not a terminal, the lines are
//! plain text without him; `--no-logo` leaves him out on a terminal too,
//! and `--json` gives the facts to scripts.

mod duke;

use std::ffi::{CStr, OsStr};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::args::{Arg, Args};
use crate::ip::{self, Cidr};
use crate::lspci::{self, Names};
use crate::sensors::json_text;
use crate::util::{self, Fail, usage};
use crate::{bundles, df, lscpu, proc};

const USAGE: &str = "Usage: dukofetch [--no-logo] [--json]\n  \
                     --no-logo  the lines without Duke\n  \
                     --json     the facts as JSON";

/// Where the kernel gives its name and version, and the release's.
const VERSION: &str = "/kernel/version";

/// The brand's colours dukofetch writes in: the labels in tan, the
/// separators in steel, and the palette row.
const TAN: u32 = 0xc8873a;
const STEEL: u32 = 0x44596b;
const PALETTE: [u32; 8] =
    [0x1a1d21, 0x44596b, 0x8fa6ba, 0xc8873a, 0xe9c48b, 0xede4d3, 0xf6f1e8, 0x15181c];

/// The columns between Duke and the lines.
const GAP: usize = 3;

/// The narrowest terminal the larger Duke is drawn on.
const WIDE: usize = 80;

/// The lines a narrow terminal shows beside the smaller Duke.
const NARROW_LINES: &[&str] =
    &["OS", "Kernel", "Uptime", "Apps", "Shell", "CPU", "Memory", "Disk (/)"];

/// What dukofetch shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Facts {
    user: String,
    host: String,
    /// The release's name and version, `Dukovo Server` and `0.12.0`.
    os: Option<(String, String)>,
    /// uname(2)'s machine, `x86_64`.
    machine: String,
    /// The kernel's name and version, `Radix` and `0.12.0`.
    kernel: Option<(String, String)>,
    /// Seconds since the kernel started.
    uptime: Option<u64>,
    bundles: Option<usize>,
    shell: Option<Shell>,
    terminal: Option<Terminal>,
    cpu: Option<Cpu>,
    gpus: Vec<String>,
    /// Bytes in use, besides the page cache, and in all.
    memory: Option<(u64, u64)>,
    disk: Option<Disk>,
    /// Each interface's IPv4 addresses, but loopback's.
    addresses: Vec<(String, Cidr)>,
}

/// The login shell, as SHELL names it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shell {
    path: String,
    name: String,
    version: Option<String>,
}

/// The terminal the output goes to: its device beneath `/devices`, and
/// the program on its other side, where that can be told.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Terminal {
    program: Option<String>,
    device: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cpu {
    name: String,
    /// The CPUs the kernel runs.
    count: Option<u32>,
    /// Its highest clock, or the one its name gives.
    mhz: Option<u32>,
}

/// The root filesystem's room, as df(1) counts it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Disk {
    used: u64,
    total: u64,
    /// What of the room a person can use is in use, rounded up.
    percent: u64,
    kind: Option<String>,
}

/// dukofetch(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut logo, mut as_json) = (true, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(ref l) if l == "no-logo" => logo = false,
            Arg::Long(ref l) if l == "json" => as_json = true,
            Arg::Long(ref l) if l == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Short('h') => return Err(usage(USAGE.into())),
            other => return Err(args.unknown(&other)),
        }
    }
    let facts = gather();
    let text = if as_json {
        json(&facts)
    } else if plain() {
        plain_text(&facts, None)
    } else {
        let mode = if truecolor() { Mode::True } else { Mode::Indexed };
        let columns = util::terminal_size(1).map_or(WIDE, |(columns, _)| columns);
        if logo { beside_duke(&facts, mode, columns) } else { plain_text(&facts, Some(mode)) }
    };
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(0)
}

/// Whether the output is plain text: NO_COLOR set and not empty
/// (no-color.org), TERM dumb, or the output not a terminal.
fn plain() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    set("NO_COLOR")
        || std::env::var_os("TERM").is_some_and(|t| t == "dumb")
        || !util::is_terminal(1)
}

/// Whether COLORTERM says the terminal has 24-bit colour.
fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|value| value == "truecolor" || value == "24bit")
}

// The facts, from the system.

fn gather() -> Facts {
    let (_, _, machine) = util::uname();
    let version = fs::read_to_string(VERSION).ok();
    let (kernel, os) = version.as_deref().and_then(parse_version).unzip();
    Facts {
        user: util::user_name(util::own_user()),
        host: util::hostname(),
        os,
        machine,
        kernel,
        uptime: Some(proc::uptime_ns()).filter(|&ns| ns > 0).map(|ns| ns / 1_000_000_000),
        bundles: bundles::count(),
        shell: std::env::var("SHELL").ok().filter(|s| !s.is_empty()).map(|path| shell(&path)),
        terminal: terminal(),
        cpu: cpu(),
        gpus: gpus(),
        memory: proc::memory()
            .ok()
            .filter(|m| m.total > 0)
            .map(|m| (m.used() * 1024, m.total * 1024)),
        disk: disk(),
        addresses: addresses(),
    }
}

/// The kernel's name and version, then the release's, from
/// `/kernel/version`'s `Radix 0.12.0 (Dukovo Server 0.12.0)`.
type Versions = ((String, String), (String, String));

fn parse_version(text: &str) -> Option<Versions> {
    let (kernel, release) = text.trim().split_once(" (")?;
    let release = release.strip_suffix(')')?;
    let (kernel_name, kernel_version) = kernel.split_once(' ')?;
    let (name, version) = release.rsplit_once(' ')?;
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    Some((pair(kernel_name, kernel_version), pair(name, version)))
}

/// The shell at `path`: Nukesh, which is built with this program and so
/// has its version, as `sh` and `bash` too; another by its file's name.
fn shell(path: &str) -> Shell {
    let real = fs::canonicalize(path).ok();
    let file = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let name = real.as_deref().and_then(file).or_else(|| file(Path::new(path)));
    let name = name.unwrap_or_else(|| path.to_string());
    let (name, version) = if name == "nukesh" {
        ("Nukesh".to_string(), Some(env!("CARGO_PKG_VERSION").to_string()))
    } else {
        (name, None)
    };
    Shell { path: path.to_string(), name, version }
}

/// The terminal on the output, or else the input, and what it is: an SSH
/// session's (sshd sets SSH_CONNECTION), one whose emulator says its name
/// in TERM_PROGRAM, or a console's (consoled sets TERM linux).
fn terminal() -> Option<Terminal> {
    let device = [1, 0].into_iter().find_map(terminal_device)?;
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let program = if var("SSH_CONNECTION").is_some() {
        Some("sshd".to_string())
    } else if let Some(program) = var("TERM_PROGRAM") {
        Some(program)
    } else {
        var("TERM").filter(|t| t == "linux").map(|_| "consoled".to_string())
    };
    Some(Terminal { program, device })
}

/// The terminal on `fd`, as `pts/0`.
fn terminal_device(fd: i32) -> Option<String> {
    // SAFETY: ttyname returns a C string the C library keeps, or null.
    let name = unsafe { libc::ttyname(fd) };
    if name.is_null() {
        return None;
    }
    // SAFETY: a non-null result is a C string.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    Some(util::terminal_name(&name).to_string())
}

fn cpu() -> Option<Cpu> {
    let brand = lscpu::model_name(&lscpu::cpuid)?;
    let [max, ..] = lscpu::cpuid(0, 0);
    // Leaf 0x16, Intel's since Skylake, gives the highest clock in EBX.
    let highest = (max >= 0x16).then(|| lscpu::cpuid(0x16, 0)[1]).filter(|&mhz| mhz > 0);
    Some(Cpu {
        name: cpu_name(&brand),
        count: lscpu::cpus().ok(),
        mhz: highest.or_else(|| named_mhz(&brand)),
    })
}

/// The processor's brand string without its marks, its clock, `CPU`,
/// `Processor` and a count of cores: `Intel Core i7-8550U` for `Intel(R)
/// Core(TM) i7-8550U CPU @ 1.80GHz`, as neofetch shortens it.
fn cpu_name(brand: &str) -> String {
    let mut name = brand.to_string();
    for mark in ["(R)", "(r)", "(TM)", "(tm)"] {
        name = name.replace(mark, "");
    }
    if let Some(at) = name.find(" @ ") {
        name.truncate(at);
    }
    let mut words: Vec<&str> = name.split_whitespace().collect();
    if words.last() == Some(&"Processor") {
        words.pop();
    }
    if words.last().is_some_and(|w| w.ends_with("-Core")) {
        words.pop();
    }
    if words.last() == Some(&"CPU") && words.len() > 1 {
        words.pop();
    }
    words.join(" ")
}

/// The clock a brand string ends with, as `@ 1.80GHz`, in MHz.
fn named_mhz(brand: &str) -> Option<u32> {
    let (_, clock) = brand.rsplit_once(" @ ")?;
    let ghz: f64 = clock.trim().strip_suffix("GHz")?.parse().ok()?;
    Some((ghz * 1000.0).round() as u32).filter(|&mhz| mhz > 0)
}

/// The display controllers, PCI class 03h, named as neofetch names them.
fn gpus() -> Vec<String> {
    let Ok(functions) = lspci::functions(Path::new(lspci::DEVICES)) else { return Vec::new() };
    let shown: Vec<_> = functions.into_iter().filter(|f| f.base_class() == 0x03).collect();
    if shown.is_empty() {
        return Vec::new();
    }
    let names = fs::read_to_string(lspci::IDS)
        .map_or_else(|_| Names::default(), |text| Names::read(&text, &shown));
    shown
        .iter()
        .map(|f| {
            let vendor = names.vendors.get(&f.vendor).map(String::as_str);
            let device = names.devices.get(&(f.vendor, f.device)).map(String::as_str);
            gpu_name(vendor, device, (f.vendor, f.device))
        })
        .collect()
}

/// A display controller's name: its vendor's short name, then its
/// model, the part of the database's name in brackets where it has one
/// (`NVIDIA GeForce RTX 3060` for `NVIDIA Corporation` and `GA106
/// [GeForce RTX 3060]`); the IDs for what the database lacks.
fn gpu_name(vendor: Option<&str>, device: Option<&str>, (v, d): (u16, u16)) -> String {
    let vendor = vendor.map_or_else(|| format!("{v:04x}"), short_vendor);
    let device = device.map_or_else(|| format!("{d:04x}"), |name| bracketed(name).to_string());
    format!("{vendor} {device}")
}

/// What is in the last brackets of `name`, or all of it.
fn bracketed(name: &str) -> &str {
    let inside = name.rsplit_once('[').and_then(|(_, rest)| rest.split_once(']'));
    inside.map_or(name, |(inside, _)| inside)
}

/// A vendor's name without its company's form: `Intel` for `Intel
/// Corporation`, and `AMD` for `Advanced Micro Devices, Inc. [AMD/ATI]`.
fn short_vendor(name: &str) -> String {
    if name.contains('[') {
        return bracketed(name).split('/').next().unwrap_or(name).to_string();
    }
    let forms = [" Corporation", " Corp.", ", Inc.", " Inc.", " Co., Ltd.", " Ltd."];
    let short = forms.iter().find_map(|form| name.strip_suffix(form)).unwrap_or(name);
    short.to_string()
}

fn disk() -> Option<Disk> {
    let space = df::statvfs(OsStr::new("/")).ok().filter(|s| s.size > 0)?;
    let used = space.size.saturating_sub(space.free);
    let usable = used + space.available;
    let percent = if usable == 0 { 0 } else { (used * 100).div_ceil(usable) };
    let mounts = proc::mounts().unwrap_or_default();
    let root = mounts.iter().rev().find(|m| m.path == "/");
    Some(Disk { used, total: space.size, percent, kind: root.map(|m| m.kind.clone()) })
}

fn addresses() -> Vec<(String, Cidr)> {
    let interfaces =
        ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES)).unwrap_or_default();
    interfaces
        .into_iter()
        .filter(|i| !i.loopback)
        .flat_map(|i| i.addresses.into_iter().map(move |a| (i.name.clone(), a)))
        .collect()
}

// The lines.

/// The labelled lines, in order: every one, or a narrow terminal's.
fn lines(facts: &Facts, narrow: bool) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some((name, version)) = &facts.os {
        lines.push(("OS", format!("{name} {version} ({})", facts.machine.replace('_', "-"))));
    }
    if let Some((name, version)) = &facts.kernel {
        lines.push(("Kernel", format!("{name} {version}")));
    }
    if let Some(seconds) = facts.uptime {
        lines.push(("Uptime", uptime(seconds)));
    }
    if let Some(count) = facts.bundles {
        lines.push(("Apps", format!("{count} bundle{}", if count == 1 { "" } else { "s" })));
    }
    if let Some(shell) = &facts.shell {
        let text = match &shell.version {
            Some(version) => format!("{} {version}", shell.name),
            None => shell.name.clone(),
        };
        lines.push(("Shell", text));
    }
    if let Some(terminal) = &facts.terminal {
        let text = match &terminal.program {
            Some(program) => format!("{program} ({})", terminal.device),
            None => terminal.device.clone(),
        };
        lines.push(("Terminal", text));
    }
    if let Some(cpu) = &facts.cpu {
        let mut text = cpu.name.clone();
        if let Some(count) = cpu.count {
            text += &format!(" ({count})");
        }
        if let Some(mhz) = cpu.mhz {
            let hundredths = (mhz + 5) / 10;
            text += &format!(" @ {}.{:02} GHz", hundredths / 100, hundredths % 100);
        }
        lines.push(("CPU", text));
    }
    for gpu in &facts.gpus {
        lines.push(("GPU", gpu.clone()));
    }
    if let Some((used, total)) = facts.memory {
        let percent = (used * 100 + total / 2) / total;
        lines.push(("Memory", format!("{} / {} ({percent}%)", memory(used), memory(total))));
    }
    if let Some(disk) = &facts.disk {
        let mut text = format!("{} / {} ({}%)", room(disk.used), room(disk.total), disk.percent);
        if let Some(kind) = &disk.kind {
            text += &format!(" {kind}");
        }
        lines.push(("Disk (/)", text));
    }
    for (name, address) in &facts.addresses {
        lines.push(("Address", format!("{}/{} ({name})", address.ip, address.prefix)));
    }
    if narrow {
        lines.retain(|(label, _)| NARROW_LINES.contains(label));
    }
    lines
}

/// `3 days, 4 hours, 12 mins`, as neofetch says it.
fn uptime(seconds: u64) -> String {
    let unit = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(unit(days, "day"));
    }
    if hours > 0 {
        parts.push(unit(hours, "hour"));
    }
    if minutes > 0 || parts.is_empty() {
        parts.push(unit(minutes, "min"));
    }
    parts.join(", ")
}

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

/// Memory to a tenth of a GiB, or in MiB below one.
fn memory(bytes: u64) -> String {
    if bytes < GIB { format!("{} MiB", bytes / MIB) } else { tenths(bytes, GIB, "GiB") }
}

/// A disk's room: in MiB below a GiB, to a tenth of a GiB below 10, in
/// whole GiB below a TiB, and to a tenth of a TiB above.
fn room(bytes: u64) -> String {
    match bytes {
        b if b < GIB => format!("{} MiB", b / MIB),
        b if b < 10 * GIB => tenths(b, GIB, "GiB"),
        b if b < TIB => format!("{} GiB", (b + GIB / 2) / GIB),
        b => tenths(b, TIB, "TiB"),
    }
}

/// `bytes` in `unit`s, rounded to a tenth.
fn tenths(bytes: u64, unit: u64, name: &str) -> String {
    let tenths = (bytes * 10 + unit / 2) / unit;
    format!("{}.{} {name}", tenths / 10, tenths % 10)
}

// Writing them.

/// How colours are written: 24-bit, or the nearest of xterm's 256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    True,
    Indexed,
}

/// A colour as `mode` writes it: `2;r;g;b` or `5;n`, after 38 or 48.
fn colour(rgb: u32, mode: Mode) -> String {
    let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
    match mode {
        Mode::True => format!("2;{r};{g};{b}"),
        Mode::Indexed => format!("5;{}", nearest_256(rgb)),
    }
}

/// The nearest of xterm's 256 colours to `rgb`, among the 6x6x6 cube and
/// the 24 greys, whose shades every xterm-like terminal and consoled
/// share; the first 16 are each terminal's own.
fn nearest_256(rgb: u32) -> u8 {
    let channels = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff].map(|c| c as i32);
    let levels = [0, 95, 135, 175, 215, 255];
    let step = |c: i32| (0..6).min_by_key(|&i| (levels[i] - c).abs()).unwrap_or(0);
    let cube = channels.map(step);
    let distance =
        |shade: [i32; 3]| shade.iter().zip(channels).map(|(s, c)| (s - c) * (s - c)).sum::<i32>();
    let cube_index = 16 + 36 * cube[0] + 6 * cube[1] + cube[2];
    let cube_distance = distance(cube.map(|i| levels[i]));
    let average = channels.iter().sum::<i32>() / 3;
    let grey = ((average - 8 + 5) / 10).clamp(0, 23);
    let grey_level = 8 + 10 * grey;
    if distance([grey_level; 3]) < cube_distance { (232 + grey) as u8 } else { cube_index as u8 }
}

const RESET: &str = "\x1b[0m";

/// The user and host, a label and its value, and a separator, each in
/// colour or not.
struct Paint(Option<Mode>);

impl Paint {
    fn fg(&self, rgb: u32, bold: bool, text: &str) -> String {
        match self.0 {
            Some(mode) => {
                let bold = if bold { "1;" } else { "" };
                format!("\x1b[{bold}38;{}m{text}{RESET}", colour(rgb, mode))
            }
            None => text.to_string(),
        }
    }

    fn title(&self, facts: &Facts) -> String {
        let at = self.fg(STEEL, false, "@");
        format!("{}{at}{}", self.fg(TAN, true, &facts.user), self.fg(TAN, true, &facts.host))
    }

    fn line(&self, label: &str, value: &str) -> String {
        format!("{}{} {value}", self.fg(TAN, true, label), self.fg(STEEL, false, ":"))
    }
}

/// The lines without Duke: `user@host`, then `Label: value`, coloured in
/// `mode` or plain.
fn plain_text(facts: &Facts, mode: Option<Mode>) -> String {
    let paint = Paint(mode);
    let mut text = paint.title(facts) + "\n";
    for (label, value) in lines(facts, false) {
        text += &paint.line(label, &value);
        text.push('\n');
    }
    text
}

/// Duke, and beside him `user@host`, a rule, the lines, cut to fit, and
/// the palette where it fits, on a terminal `columns` wide.
fn beside_duke(facts: &Facts, mode: Mode, columns: usize) -> String {
    let narrow = columns < WIDE;
    let picture: &[&str] = if narrow { &duke::DUKE_10 } else { &duke::DUKE_16 };
    let duke = draw(picture, mode);
    let width = picture[0].len();
    let room = columns.saturating_sub(width + GAP);
    let paint = Paint(Some(mode));
    let title_width = util::width(&facts.user) + 1 + util::width(&facts.host);
    let mut info = vec![paint.title(facts), paint.fg(STEEL, false, &"─".repeat(title_width))];
    for (label, value) in lines(facts, narrow) {
        // Cut what would wrap: the label and `: ` take their width.
        let value = cut(&value, room.saturating_sub(util::width(label) + 2));
        info.push(paint.line(label, &value));
    }
    if room >= 3 * PALETTE.len() {
        info.push(String::new());
        let swatch = |rgb: u32| format!("\x1b[48;{}m   ", colour(rgb, mode));
        info.push(PALETTE.map(swatch).concat() + RESET);
    }
    let rows = duke.len().max(info.len());
    let mut text = String::new();
    for row in 0..rows {
        match duke.get(row) {
            Some(cells) => text += cells,
            None => text += &" ".repeat(width),
        }
        if let Some(line) = info.get(row).filter(|line| !line.is_empty()) {
            text += &" ".repeat(GAP);
            text += line;
        }
        text.push('\n');
    }
    text
}

/// `text`, cut to `columns` with `…` if it is wider.
fn cut(text: &str, columns: usize) -> String {
    if util::width(text) <= columns {
        return text.to_string();
    }
    let mut kept = String::new();
    for c in text.chars() {
        if util::width(&kept) + util::width(&c.to_string()) + 1 > columns {
            break;
        }
        kept.push(c);
    }
    kept + "…"
}

/// The colour of a pixel's letter in [`duke::COLOURS`]; none for a space.
fn pixel(letter: u8) -> Option<u32> {
    duke::COLOURS.iter().find(|&&(l, _)| l == letter).map(|&(_, rgb)| rgb)
}

/// `picture`'s rows of pixels as lines of half blocks: each cell the
/// upper half block in the top pixel's colour on the bottom one's, or the
/// full block where the two are one colour as `mode` writes them, each
/// line ending in the terminal's own colours.
fn draw(picture: &[&str], mode: Mode) -> Vec<String> {
    picture
        .chunks(2)
        .map(|pair| {
            let (top, bottom) = (pair[0].as_bytes(), pair.get(1).map_or(&[][..], |b| b.as_bytes()));
            let mut line = String::new();
            // The colours set, foreground and background, written only on
            // a change.
            let mut pen: (Option<String>, Option<String>) = (None, None);
            for (x, &t) in top.iter().enumerate() {
                let upper = pixel(t).map(|rgb| colour(rgb, mode));
                let lower = bottom.get(x).and_then(|&b| pixel(b)).map(|rgb| colour(rgb, mode));
                let (fg, bg, glyph) = match (upper, lower) {
                    (None, None) => (None, None, ' '),
                    (Some(u), Some(l)) if u == l => (Some(u), None, '█'),
                    (Some(u), Some(l)) => (Some(u), Some(l), '▀'),
                    (Some(u), None) => (Some(u), None, '▀'),
                    (None, Some(l)) => (Some(l), None, '▄'),
                };
                // Only a reset takes the background away.
                if pen.1.is_some() && bg.is_none() {
                    line += RESET;
                    pen = (None, None);
                }
                if fg.is_some() && fg != pen.0 {
                    line += &format!("\x1b[38;{}m", fg.as_deref().unwrap_or_default());
                }
                if bg.is_some() && bg != pen.1 {
                    line += &format!("\x1b[48;{}m", bg.as_deref().unwrap_or_default());
                }
                pen = (fg.or(pen.0), bg);
                line.push(glyph);
            }
            if pen != (None, None) {
                line += RESET;
            }
            line
        })
        .collect()
}

// JSON.

/// The facts as a JSON object, a key a line; a fact whose source is
/// missing has no key.
fn json(facts: &Facts) -> String {
    let text = |value: &str| format!("\"{}\"", json_text(value));
    let pair = |name: &str, version: &str| {
        format!("{{\"name\": {}, \"version\": {}}}", text(name), text(version))
    };
    let mut keys = vec![("user", text(&facts.user)), ("host", text(&facts.host))];
    if let Some((name, version)) = &facts.os {
        let machine = text(&facts.machine);
        let os = format!(
            "{{\"name\": {}, \"version\": {}, \"machine\": {machine}}}",
            text(name),
            text(version)
        );
        keys.push(("os", os));
    }
    if let Some((name, version)) = &facts.kernel {
        keys.push(("kernel", pair(name, version)));
    }
    if let Some(seconds) = facts.uptime {
        keys.push(("uptime_seconds", seconds.to_string()));
    }
    if let Some(count) = facts.bundles {
        keys.push(("bundles", count.to_string()));
    }
    if let Some(shell) = &facts.shell {
        let mut object = format!("{{\"name\": {}", text(&shell.name));
        if let Some(version) = &shell.version {
            object += &format!(", \"version\": {}", text(version));
        }
        object += &format!(", \"path\": {}}}", text(&shell.path));
        keys.push(("shell", object));
    }
    if let Some(terminal) = &facts.terminal {
        let mut object = String::from("{");
        if let Some(program) = &terminal.program {
            object += &format!("\"program\": {}, ", text(program));
        }
        object += &format!("\"device\": {}}}", text(&terminal.device));
        keys.push(("terminal", object));
    }
    if let Some(cpu) = &facts.cpu {
        let mut object = format!("{{\"name\": {}", text(&cpu.name));
        if let Some(count) = cpu.count {
            object += &format!(", \"cpus\": {count}");
        }
        if let Some(mhz) = cpu.mhz {
            object += &format!(", \"mhz\": {mhz}");
        }
        keys.push(("cpu", object + "}"));
    }
    let gpus: Vec<String> = facts.gpus.iter().map(|g| text(g)).collect();
    keys.push(("gpus", format!("[{}]", gpus.join(", "))));
    if let Some((used, total)) = facts.memory {
        keys.push(("memory", format!("{{\"used\": {used}, \"total\": {total}}}")));
    }
    if let Some(disk) = &facts.disk {
        let mut object =
            format!("{{\"mount\": \"/\", \"used\": {}, \"total\": {}", disk.used, disk.total);
        if let Some(kind) = &disk.kind {
            object += &format!(", \"type\": {}", text(kind));
        }
        keys.push(("disk", object + "}"));
    }
    let addresses: Vec<String> = facts
        .addresses
        .iter()
        .map(|(name, a)| {
            format!(
                "{{\"interface\": {}, \"address\": \"{}\", \"prefix\": {}}}",
                text(name),
                a.ip,
                a.prefix
            )
        })
        .collect();
    keys.push(("addresses", format!("[{}]", addresses.join(", "))));
    let body: Vec<String> =
        keys.iter().map(|(key, value)| format!("  \"{key}\": {value}")).collect();
    format!("{{\n{}\n}}\n", body.join(",\n"))
}

#[cfg(test)]
mod tests;
