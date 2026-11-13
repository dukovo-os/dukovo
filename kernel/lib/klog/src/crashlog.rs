// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The crash log: console output kept in RAM for the next boot.
//!
//! A machine without a serial port, such as the reference laptop, loses a
//! kernel's last words when it crashes. So the console output also goes to a
//! region of RAM that the architecture sets aside at the same physical
//! address on every boot. A warm restart keeps RAM contents on most
//! machines, and the next boot finds the log there and shows its end.
//!
//! The region is split into two slots, each a 64-byte header followed by a
//! ring of text. Each boot writes the slot the previous boot did not use,
//! so the previous log survives until the boot after next. A slot's header
//! records the boot number and how that boot ended.
//!
//! A region with room for them ends in two dump slots of [`DUMP_AREA`]
//! between them (ADR 0483): when the kernel panics, it writes a crash dump,
//! a text report of its state, into the dump slot that goes with its log's
//! slot, and the next boot hands it to dumps(8). The log's slots keep the
//! layout they have in a region without dumps, so a log that a kernel
//! without dumps wrote still reads. The format is described in
//! radix-debug(7).

use core::fmt;

/// First eight bytes of a valid slot.
pub const MAGIC: [u8; 8] = *b"RADIXLOG";
/// Format version.
pub const VERSION: u32 = 1;
/// Bytes of header at the start of each slot.
pub const HEADER: usize = 64;
/// Smallest region [`CrashLog::open`] accepts.
pub const MIN_REGION: usize = 2 * (HEADER + 1024);

/// How the boot that wrote a slot ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Still running when the slot was last written: the machine hung, was
    /// reset, or lost power while running.
    Running,
    /// The kernel panicked.
    Panicked,
    /// The kernel restarted the machine on request.
    Restarted,
}

impl Ending {
    fn code(self) -> u32 {
        match self {
            Ending::Running => 1,
            Ending::Panicked => 2,
            Ending::Restarted => 3,
        }
    }

    fn from_code(code: u32) -> Option<Ending> {
        match code {
            1 => Some(Ending::Running),
            2 => Some(Ending::Panicked),
            3 => Some(Ending::Restarted),
            _ => None,
        }
    }
}

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Ending::Running => "stopped while running (hang, reset or power loss)",
            Ending::Panicked => "ended with a kernel panic",
            Ending::Restarted => "ended with a requested restart",
        })
    }
}

/// Parsed header of a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    boot: u32,
    ending: Ending,
    written: u64,
}

const OFF_VERSION: usize = 8;
const OFF_BOOT: usize = 12;
const OFF_ENDING: usize = 16;
const OFF_RING_SIZE: usize = 20;
const OFF_WRITTEN: usize = 24;
const OFF_CHECKSUM: usize = 32;

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("8 bytes"))
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// FNV-1a over the header bytes before the checksum field.
fn checksum(header: &[u8]) -> u32 {
    header[..OFF_CHECKSUM]
        .iter()
        .fold(0x811c_9dc5u32, |hash, &byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193))
}

fn parse_header(slot: &[u8]) -> Option<Header> {
    let header = &slot[..HEADER];
    if header[..8] != MAGIC
        || read_u32(header, OFF_VERSION) != VERSION
        || read_u32(header, OFF_RING_SIZE) as usize != slot.len() - HEADER
        || read_u32(header, OFF_CHECKSUM) != checksum(header)
    {
        return None;
    }
    Some(Header {
        boot: read_u32(header, OFF_BOOT),
        ending: Ending::from_code(read_u32(header, OFF_ENDING))?,
        written: read_u64(header, OFF_WRITTEN),
    })
}

fn write_header(slot: &mut [u8], header: Header) {
    let ring_size = (slot.len() - HEADER) as u32;
    let bytes = &mut slot[..HEADER];
    bytes.fill(0);
    bytes[..8].copy_from_slice(&MAGIC);
    bytes[OFF_VERSION..OFF_VERSION + 4].copy_from_slice(&VERSION.to_le_bytes());
    bytes[OFF_BOOT..OFF_BOOT + 4].copy_from_slice(&header.boot.to_le_bytes());
    bytes[OFF_ENDING..OFF_ENDING + 4].copy_from_slice(&header.ending.code().to_le_bytes());
    bytes[OFF_RING_SIZE..OFF_RING_SIZE + 4].copy_from_slice(&ring_size.to_le_bytes());
    bytes[OFF_WRITTEN..OFF_WRITTEN + 8].copy_from_slice(&header.written.to_le_bytes());
    let sum = checksum(bytes);
    bytes[OFF_CHECKSUM..OFF_CHECKSUM + 4].copy_from_slice(&sum.to_le_bytes());
}

/// The log of the boot before this one, read from its slot.
#[derive(Clone, Copy, Debug)]
pub struct Previous<'a> {
    /// That boot's number.
    pub boot: u32,
    /// How it ended.
    pub ending: Ending,
    /// Its text in order: `older` then `newer`.
    older: &'a [u8],
    newer: &'a [u8],
    /// Whether the ring wrapped, so that `older` starts in a line.
    wrapped: bool,
}

impl<'a> Previous<'a> {
    fn new(slot: &'a [u8], header: Header) -> Self {
        let ring = &slot[HEADER..];
        let size = ring.len() as u64;
        if header.written <= size {
            Previous {
                boot: header.boot,
                ending: header.ending,
                older: &ring[..header.written as usize],
                newer: &[],
                wrapped: false,
            }
        } else {
            // The ring wrapped: the oldest byte is at the write position.
            let start = (header.written % size) as usize;
            Previous {
                boot: header.boot,
                ending: header.ending,
                older: &ring[start..],
                newer: &ring[..start],
                wrapped: true,
            }
        }
    }

    /// The text kept, oldest first, as two parts of the ring. When the
    /// ring has wrapped, the line it cut is left out, so the text starts
    /// with a whole line.
    pub fn text(&self) -> (&'a [u8], &'a [u8]) {
        if !self.wrapped {
            return (self.older, self.newer);
        }
        match self.older.iter().position(|&byte| byte == b'\n') {
            Some(end) => (&self.older[end + 1..], self.newer),
            None => {
                let end = self.newer.iter().position(|&byte| byte == b'\n');
                (&[], end.map_or(&[][..], |end| &self.newer[end + 1..]))
            }
        }
    }

    /// Bytes of text kept.
    pub fn len(&self) -> usize {
        self.older.len() + self.newer.len()
    }

    /// Whether no text was kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The kept lines, oldest first. A line cut by the ring's wrap-around
    /// comes first, partial. Bytes that are not printable ASCII show as `?`.
    pub fn lines(&self) -> Lines<'a> {
        Lines { bytes: self.older.iter().chain(self.newer.iter()), done: false }
    }

    /// Number of lines kept.
    pub fn line_count(&self) -> usize {
        self.lines().count()
    }

    /// Whether some line contains `needle`.
    pub fn contains(&self, needle: &str) -> bool {
        self.lines().any(|line| line.as_str().contains(needle))
    }
}

/// One line of a [`Previous`] log.
#[derive(Clone)]
pub struct Line {
    bytes: [u8; Line::MAX],
    len: usize,
}

impl Line {
    /// Longest line kept; longer lines are split.
    pub const MAX: usize = 240;

    /// The line's text, without the newline.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Iterator over the lines of a [`Previous`] log.
pub struct Lines<'a> {
    bytes: core::iter::Chain<core::slice::Iter<'a, u8>, core::slice::Iter<'a, u8>>,
    done: bool,
}

impl Iterator for Lines<'_> {
    type Item = Line;

    fn next(&mut self) -> Option<Line> {
        if self.done {
            return None;
        }
        let mut line = Line { bytes: [0; Line::MAX], len: 0 };
        loop {
            match self.bytes.next() {
                None => {
                    self.done = true;
                    return (line.len > 0).then_some(line);
                }
                Some(b'\n') => return Some(line),
                Some(b'\r') => {}
                Some(&byte) => {
                    let printable = byte == b'\t' || (0x20..0x7f).contains(&byte);
                    line.bytes[line.len] = if printable { byte } else { b'?' };
                    line.len += 1;
                    if line.len == Line::MAX {
                        return Some(line);
                    }
                }
            }
        }
    }
}

/// First eight bytes of a dump slot that holds a dump.
pub const DUMP_MAGIC: [u8; 8] = *b"RADIXDMP";
/// The dump format's version.
pub const DUMP_VERSION: u32 = 1;
/// Bytes at the end of a region that its two dump slots take.
pub const DUMP_AREA: usize = 1 << 20;
/// Bytes of a dump slot, its header included.
const DUMP_SLOT: usize = DUMP_AREA / 2;
/// The last line of a dump that filled its slot.
pub const DUMP_FULL: &str = "(the dump is cut short here: it filled its 512 KiB)\n";
const _: () = assert!(DUMP_SLOT == 512 * 1024);
/// Room a dump keeps for [`DUMP_FULL`] and the line end before it.
const DUMP_RESERVE: usize = DUMP_FULL.len() + 1;

const OFF_DUMP_BOOT: usize = 12;
const OFF_DUMP_TIME: usize = 16;
const OFF_DUMP_LEN: usize = 24;
const OFF_DUMP_CRC: usize = 28;
const OFF_DUMP_STATE: usize = 32;

/// How much of a dump there is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DumpState {
    /// Every section was written.
    Whole,
    /// It ends after its last whole section: the slot filled, or the
    /// kernel failed again, or the machine stopped, while writing it.
    CutShort,
}

impl DumpState {
    fn code(self) -> u32 {
        match self {
            DumpState::Whole => 1,
            DumpState::CutShort => 2,
        }
    }

    fn from_code(code: u32) -> Option<DumpState> {
        match code {
            1 => Some(DumpState::Whole),
            2 => Some(DumpState::CutShort),
            _ => None,
        }
    }
}

/// CRC-32 as IEEE 802.3 and zlib's `crc32` have it, one byte at a time.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

/// The CRC-32 of the bytes whose CRC-32 is `crc` (0 for none) followed by
/// `bytes`.
fn crc32(crc: u32, bytes: &[u8]) -> u32 {
    !bytes
        .iter()
        .fold(!crc, |crc, &byte| CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8))
}

/// This boot's crash dump, which the panic path writes into its slot.
///
/// The text goes into the slot as it comes, and [`Dump::seal`] writes the
/// header that makes it a dump: how long it is and its CRC-32. The panic
/// path seals after each section, as cut short, and once at the end as
/// whole, so that a dump the kernel could not finish still holds every
/// section before the one it failed in, and says it is cut short.
pub struct Dump<'a> {
    slot: &'a mut [u8],
    boot: u32,
    wall_ns: u64,
    /// Bytes of text written, and their CRC-32.
    len: usize,
    crc: u32,
    /// Whether the slot filled, which ends the dump.
    full: bool,
}

impl Dump<'_> {
    /// Appends `text`. Text that does not fit is cut at the end of a
    /// character, the line [`DUMP_FULL`] follows it, and the dump takes
    /// nothing more: false from then on.
    pub fn write(&mut self, text: &str, flush: &mut dyn FnMut(&[u8])) -> bool {
        if self.full {
            return false;
        }
        let room = self.slot.len() - HEADER - DUMP_RESERVE - self.len;
        if text.len() <= room {
            self.append(text.as_bytes(), flush);
            return true;
        }
        let mut fits = room;
        while !text.is_char_boundary(fits) {
            fits -= 1;
        }
        self.append(&text.as_bytes()[..fits], flush);
        if self.len > 0 && self.slot[HEADER + self.len - 1] != b'\n' {
            self.append(b"\n", flush);
        }
        self.append(DUMP_FULL.as_bytes(), flush);
        self.full = true;
        false
    }

    fn append(&mut self, bytes: &[u8], flush: &mut dyn FnMut(&[u8])) {
        let at = HEADER + self.len..HEADER + self.len + bytes.len();
        self.slot[at.clone()].copy_from_slice(bytes);
        flush(&self.slot[at]);
        self.crc = crc32(self.crc, bytes);
        self.len += bytes.len();
    }

    /// Writes the header for the text so far, as `state`, or as cut short
    /// once the slot has filled. The text is in memory before its header
    /// says so: `flush` has had each part of it.
    pub fn seal(&mut self, state: DumpState, flush: &mut dyn FnMut(&[u8])) {
        let state = if self.full { DumpState::CutShort } else { state };
        let header = &mut self.slot[..HEADER];
        header[..8].copy_from_slice(&DUMP_MAGIC);
        put_u32(header, OFF_VERSION, DUMP_VERSION);
        put_u32(header, OFF_DUMP_BOOT, self.boot);
        put_u64(header, OFF_DUMP_TIME, self.wall_ns);
        put_u32(header, OFF_DUMP_LEN, self.len as u32);
        put_u32(header, OFF_DUMP_CRC, self.crc);
        put_u32(header, OFF_DUMP_STATE, state.code());
        flush(header);
    }
}

/// The crash dump of the boot before this one.
#[derive(Clone, Copy, Debug)]
pub struct PreviousDump<'a> {
    /// That boot's number.
    pub boot: u32,
    /// When it panicked, in nanoseconds since 1970 (UTC); 0 when the
    /// kernel did not know the time.
    pub wall_ns: u64,
    /// Whether every section is there.
    pub state: DumpState,
    /// The dump: UTF-8 text, a line ending each line.
    pub text: &'a [u8],
}

/// The dump in `slot`, if it holds one whose text matches its CRC-32.
fn parse_dump(slot: &[u8]) -> Option<PreviousDump<'_>> {
    let header = &slot[..HEADER];
    if header[..8] != DUMP_MAGIC || read_u32(header, OFF_VERSION) != DUMP_VERSION {
        return None;
    }
    let len = read_u32(header, OFF_DUMP_LEN) as usize;
    let text = slot.get(HEADER..HEADER.checked_add(len)?)?;
    if crc32(0, text) != read_u32(header, OFF_DUMP_CRC) {
        return None;
    }
    Some(PreviousDump {
        boot: read_u32(header, OFF_DUMP_BOOT),
        wall_ns: read_u64(header, OFF_DUMP_TIME),
        state: DumpState::from_code(read_u32(header, OFF_DUMP_STATE))?,
        text,
    })
}

/// The crash log of the running boot.
pub struct CrashLog {
    region: &'static mut [u8],
    /// Bytes at the start of the region that the log's two slots take;
    /// the dump slots, if there is room for them, follow.
    log_len: usize,
    current: usize,
    header: Header,
    previous: Option<(usize, Header)>,
}

impl CrashLog {
    /// Takes over `region`: finds the previous boot's slot, if any, and
    /// starts this boot's log in the other one. A region of at least
    /// [`DUMP_AREA`] more than [`MIN_REGION`] keeps its last [`DUMP_AREA`]
    /// for dumps, and this boot's dump slot is emptied.
    ///
    /// Returns `None` if the region is smaller than [`MIN_REGION`].
    pub fn open(region: &'static mut [u8]) -> Option<CrashLog> {
        if region.len() < MIN_REGION {
            return None;
        }
        let log_len = match region.len().checked_sub(DUMP_AREA) {
            Some(log_len) if log_len >= MIN_REGION => log_len,
            _ => region.len(),
        };
        let half = log_len / 2;
        let slots = [parse_header(&region[..half]), parse_header(&region[half..2 * half])];
        let previous = match slots {
            [Some(a), Some(b)] if b.boot > a.boot => Some((1, b)),
            [Some(a), _] => Some((0, a)),
            [None, Some(b)] => Some((1, b)),
            [None, None] => None,
        };
        let current = match previous {
            Some((0, _)) => 1,
            _ => 0,
        };
        let boot = previous.map_or(1, |(_, header)| header.boot.wrapping_add(1).max(1));
        let header = Header { boot, ending: Ending::Running, written: 0 };
        let mut log = CrashLog { region, log_len, current, header, previous };
        let slot = log.slot_mut(current);
        write_header(slot, header);
        // A dump left from the boot before last is not this boot's.
        if let Some(slot) = log.dump_slot_mut(current) {
            slot[..HEADER].fill(0);
        }
        Some(log)
    }

    fn half(&self) -> usize {
        self.log_len / 2
    }

    /// Dump slot `index`, which goes with log slot `index`, if the region
    /// has room for dumps.
    fn dump_slot(&self, index: usize) -> Option<&[u8]> {
        let start = self.log_len + index * DUMP_SLOT;
        (self.region.len() > self.log_len).then(|| &self.region[start..start + DUMP_SLOT])
    }

    fn dump_slot_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        let start = self.log_len + index * DUMP_SLOT;
        (self.region.len() > self.log_len).then(|| &mut self.region[start..start + DUMP_SLOT])
    }

    fn slot(&self, index: usize) -> &[u8] {
        let half = self.half();
        &self.region[index * half..(index + 1) * half]
    }

    fn slot_mut(&mut self, index: usize) -> &mut [u8] {
        let half = self.half();
        &mut self.region[index * half..(index + 1) * half]
    }

    /// This boot's number: one more than the previous boot's.
    pub fn boot(&self) -> u32 {
        self.header.boot
    }

    /// The previous boot's log, if the region held one.
    pub fn previous(&self) -> Option<Previous<'_>> {
        let (index, header) = self.previous?;
        Some(Previous::new(self.slot(index), header))
    }

    /// The previous boot's log as it was written, oldest first, as two
    /// parts of its ring ([`Previous::text`]).
    pub fn previous_text(&self) -> Option<(&[u8], &[u8])> {
        Some(self.previous()?.text())
    }

    /// The previous boot's crash dump: there is one when that boot ended
    /// with a panic and its dump slot holds a dump of that boot.
    pub fn previous_dump(&self) -> Option<PreviousDump<'_>> {
        let (index, header) = self.previous?;
        if header.ending != Ending::Panicked {
            return None;
        }
        parse_dump(self.dump_slot(index)?).filter(|dump| dump.boot == header.boot)
    }

    /// Starts this boot's crash dump, from the panic path: `None` if the
    /// region has no room for dumps. `wall_ns` is the time of the panic,
    /// in nanoseconds since 1970 (UTC), or 0 if the kernel does not know
    /// it. Until the first [`Dump::seal`], the slot holds no dump.
    pub fn begin_dump(&mut self, wall_ns: u64, flush: &mut dyn FnMut(&[u8])) -> Option<Dump<'_>> {
        let boot = self.header.boot;
        let slot = self.dump_slot_mut(self.current)?;
        slot[..HEADER].fill(0);
        flush(&slot[..HEADER]);
        Some(Dump { slot, boot, wall_ns, len: 0, crc: 0, full: false })
    }

    /// Appends `text` to this boot's log. `flush` is called with every
    /// range of the region that changed, so the caller can write it back
    /// from the CPU caches to RAM.
    pub fn write(&mut self, text: &str, flush: &mut dyn FnMut(&[u8])) {
        let current = self.current;
        let half = self.half();
        let size = half - HEADER;
        let mut bytes = text.as_bytes();
        if bytes.len() > size {
            bytes = &bytes[bytes.len() - size..];
        }
        let mut position = (self.header.written % size as u64) as usize;
        let base = current * half + HEADER;
        while !bytes.is_empty() {
            let chunk = bytes.len().min(size - position);
            let range = base + position..base + position + chunk;
            self.region[range.clone()].copy_from_slice(&bytes[..chunk]);
            flush(&self.region[range]);
            bytes = &bytes[chunk..];
            position = (position + chunk) % size;
        }
        self.header.written += text.len().min(size) as u64;
        self.commit_header(flush);
    }

    /// Gives the region back, as the next boot would find it.
    pub fn into_region(self) -> &'static mut [u8] {
        self.region
    }

    /// Records how this boot ends.
    pub fn set_ending(&mut self, ending: Ending, flush: &mut dyn FnMut(&[u8])) {
        self.header.ending = ending;
        self.commit_header(flush);
    }

    fn commit_header(&mut self, flush: &mut dyn FnMut(&[u8])) {
        let header = self.header;
        let current = self.current;
        write_header(self.slot_mut(current), header);
        flush(&self.slot(current)[..HEADER]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(len: usize) -> &'static mut [u8] {
        Box::leak(vec![0u8; len].into_boxed_slice())
    }

    fn no_flush() -> impl FnMut(&[u8]) {
        |_| {}
    }

    #[test]
    fn first_boot_has_no_previous_log() {
        let log = CrashLog::open(region(4096)).unwrap();
        assert_eq!(log.boot(), 1);
        assert!(log.previous().is_none());
        assert!(CrashLog::open(region(100)).is_none());
    }

    #[test]
    fn survives_into_the_next_boot() {
        let mut flushed = 0;
        let mut log = CrashLog::open(region(4096)).unwrap();
        log.write("[    0.1] hello\n[    0.2] world\n", &mut |bytes| flushed += bytes.len());
        log.set_ending(Ending::Panicked, &mut no_flush());
        assert!(flushed > 0);

        // The next boot sees the same memory.
        let log = CrashLog::open(log.into_region()).unwrap();
        assert_eq!(log.boot(), 2);
        let previous = log.previous().unwrap();
        assert_eq!(previous.boot, 1);
        assert_eq!(previous.ending, Ending::Panicked);
        let lines: Vec<String> = previous.lines().map(|l| l.to_string()).collect();
        assert_eq!(lines, ["[    0.1] hello", "[    0.2] world"]);
        assert!(previous.contains("world"));
    }

    #[test]
    fn alternates_slots_and_keeps_the_newest() {
        let mut log = CrashLog::open(region(4096)).unwrap();
        log.write("boot one\n", &mut no_flush());
        let mut log = CrashLog::open(log.into_region()).unwrap();
        assert_eq!(log.previous().unwrap().ending, Ending::Running);
        log.write("boot two\n", &mut no_flush());
        log.set_ending(Ending::Restarted, &mut no_flush());
        let log = CrashLog::open(log.into_region()).unwrap();
        assert_eq!(log.boot(), 3);
        let previous = log.previous().unwrap();
        assert_eq!(previous.boot, 2);
        assert_eq!(previous.ending, Ending::Restarted);
        assert!(previous.contains("boot two"));
        assert!(!previous.contains("boot one"));
    }

    #[test]
    fn wraps_around_and_sanitises() {
        let mut log = CrashLog::open(region(MIN_REGION)).unwrap();
        for i in 0..200 {
            log.write(&format!("line {i}\n"), &mut no_flush());
        }
        log.write("bad \u{7}byte\n", &mut no_flush());
        let log = CrashLog::open(log.into_region()).unwrap();
        let previous = log.previous().unwrap();
        assert_eq!(previous.len(), MIN_REGION / 2 - HEADER);
        let lines: Vec<String> = previous.lines().map(|l| l.to_string()).collect();
        assert_eq!(lines.last().unwrap(), "bad ?byte");
        assert_eq!(lines[lines.len() - 2], "line 199");
        assert!(!lines.iter().any(|line| line == "line 0"));
    }

    #[test]
    fn rejects_corrupt_headers() {
        let mut log = CrashLog::open(region(4096)).unwrap();
        log.write("x\n", &mut no_flush());
        let memory = log.into_region();
        memory[OFF_BOOT] ^= 0xff;
        let log = CrashLog::open(memory).unwrap();
        assert!(log.previous().is_none());
        assert_eq!(log.boot(), 1);
    }

    /// A region with a log of 4 KiB and room for dumps.
    fn with_dumps() -> &'static mut [u8] {
        region(4096 + DUMP_AREA)
    }

    /// The boot after `log`'s, which ended as `ending`.
    fn next_boot(mut log: CrashLog, ending: Ending) -> CrashLog {
        log.set_ending(ending, &mut no_flush());
        CrashLog::open(log.into_region()).unwrap()
    }

    /// Writes a dump of `sections`, sealing it after each, cut short, and
    /// at the end as `last`.
    fn dump(log: &mut CrashLog, sections: &[&str], last: DumpState) {
        let mut dump = log.begin_dump(1_763_028_131_000_000_000, &mut no_flush()).unwrap();
        for section in sections {
            dump.write(section, &mut no_flush());
            dump.seal(DumpState::CutShort, &mut no_flush());
        }
        dump.seal(last, &mut no_flush());
    }

    fn text(dump: PreviousDump<'_>) -> &str {
        core::str::from_utf8(dump.text).unwrap()
    }

    #[test]
    fn crc32_is_ieee_and_continues_across_parts() {
        assert_eq!(crc32(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), 0xcbf4_3926);
        assert_eq!(crc32(0, b""), 0);
    }

    #[test]
    fn a_dump_reads_back_at_the_boot_after_the_panic() {
        let mut log = CrashLog::open(with_dumps()).unwrap();
        log.write("[    0.1] hello\n", &mut no_flush());
        let mut flushed = 0;
        let mut dump = log.begin_dump(1_763_028_131_000_000_000, &mut |_| ()).unwrap();
        dump.write("Radix crash dump\n", &mut |bytes| flushed += bytes.len());
        dump.write("== Panic\noops\n", &mut |bytes| flushed += bytes.len());
        dump.seal(DumpState::Whole, &mut |bytes| flushed += bytes.len());
        assert_eq!(flushed, 31 + HEADER);
        assert!(log.previous_dump().is_none());

        let log = next_boot(log, Ending::Panicked);
        let previous = log.previous_dump().unwrap();
        assert_eq!((previous.boot, previous.state), (1, DumpState::Whole));
        assert_eq!(previous.wall_ns, 1_763_028_131_000_000_000);
        assert_eq!(text(previous), "Radix crash dump\n== Panic\noops\n");
        // The log is there as before.
        assert!(log.previous().unwrap().contains("hello"));
    }

    #[test]
    fn a_dump_ends_at_its_last_seal_and_says_it_is_cut_short() {
        let mut log = CrashLog::open(with_dumps()).unwrap();
        let mut dump = log.begin_dump(0, &mut no_flush()).unwrap();
        dump.write("== Panic\noops\n", &mut no_flush());
        dump.seal(DumpState::CutShort, &mut no_flush());
        // The kernel failed again in the next section.
        dump.write("== Threads\nhalf a li", &mut no_flush());
        let log = next_boot(log, Ending::Panicked);
        let previous = log.previous_dump().unwrap();
        assert_eq!((previous.state, previous.wall_ns), (DumpState::CutShort, 0));
        assert_eq!(text(previous), "== Panic\noops\n");
    }

    #[test]
    fn a_dump_that_fills_its_slot_ends_with_a_line_saying_so() {
        let mut log = CrashLog::open(with_dumps()).unwrap();
        let mut dump = log.begin_dump(0, &mut no_flush()).unwrap();
        // Two bytes a character, so the slot fills in the middle of one.
        let line = format!("{}\n", "é".repeat(100));
        let mut lines = 0;
        while dump.write(&line, &mut no_flush()) {
            lines += 1;
        }
        assert!(!dump.write("more\n", &mut no_flush()));
        dump.seal(DumpState::Whole, &mut no_flush());
        let log = next_boot(log, Ending::Panicked);
        let previous = log.previous_dump().unwrap();
        assert_eq!(previous.state, DumpState::CutShort);
        let text = text(previous);
        assert!(text.ends_with("é\n(the dump is cut short here: it filled its 512 KiB)\n"));
        assert!(text.starts_with(&line.repeat(lines)));
        assert!(previous.text.len() <= DUMP_SLOT - HEADER);
        assert!(previous.text.len() > DUMP_SLOT - HEADER - DUMP_RESERVE - line.len());
    }

    #[test]
    fn there_is_no_dump_unless_the_previous_boot_panicked_and_wrote_one() {
        // A boot that panicked without writing one: crashdump=off.
        let log = next_boot(CrashLog::open(with_dumps()).unwrap(), Ending::Panicked);
        assert!(log.previous_dump().is_none());

        // A boot that wrote one and then restarted after all.
        let mut log = CrashLog::open(with_dumps()).unwrap();
        dump(&mut log, &["== Panic\n"], DumpState::Whole);
        assert!(next_boot(log, Ending::Restarted).previous_dump().is_none());

        // A dump whose text changed.
        let mut log = CrashLog::open(with_dumps()).unwrap();
        dump(&mut log, &["== Panic\n"], DumpState::Whole);
        log.set_ending(Ending::Panicked, &mut no_flush());
        let memory = log.into_region();
        memory[4096 + HEADER + 3] ^= 1;
        assert!(CrashLog::open(memory).unwrap().previous_dump().is_none());

        // A dump of another boot in the previous boot's slot.
        let mut log = CrashLog::open(with_dumps()).unwrap();
        dump(&mut log, &["boot one\n"], DumpState::Whole);
        let log = next_boot(next_boot(log, Ending::Panicked), Ending::Panicked);
        assert_eq!(log.previous().unwrap().boot, 2);
        let memory = log.into_region();
        memory.copy_within(4096..4096 + DUMP_SLOT, 4096 + DUMP_SLOT);
        let log = CrashLog::open(memory).unwrap();
        assert_eq!(log.previous().unwrap().boot, 3);
        assert!(log.previous_dump().is_none());
    }

    #[test]
    fn dump_slots_alternate_with_the_logs_slots() {
        let mut log = CrashLog::open(with_dumps()).unwrap();
        dump(&mut log, &["boot one\n"], DumpState::Whole);
        let mut log = next_boot(log, Ending::Panicked);
        assert_eq!(text(log.previous_dump().unwrap()), "boot one\n");
        dump(&mut log, &["boot two\n"], DumpState::Whole);
        let log = next_boot(log, Ending::Panicked);
        let previous = log.previous_dump().unwrap();
        assert_eq!((previous.boot, text(previous)), (2, "boot two\n"));
        // Boot two's log was in the second slot, and so was its dump; boot
        // three emptied the first, its own.
        let memory = log.into_region();
        assert_eq!(memory[4096 + DUMP_SLOT..][..8], DUMP_MAGIC);
        assert_eq!(memory[4096..][..HEADER], [0; HEADER]);
    }

    #[test]
    fn a_region_without_room_for_dumps_keeps_its_log_as_before() {
        let mut log = CrashLog::open(region(1 << 20)).unwrap();
        assert!(log.begin_dump(0, &mut no_flush()).is_none());
        log.write("an older kernel's log\n", &mut no_flush());
        log.set_ending(Ending::Panicked, &mut no_flush());
        let old = log.into_region();
        // The region a kernel with dumps reserves starts with the old one.
        let memory = region((1 << 20) + DUMP_AREA);
        memory[..1 << 20].copy_from_slice(old);
        let log = CrashLog::open(memory).unwrap();
        let previous = log.previous().unwrap();
        assert_eq!((previous.boot, previous.ending), (1, Ending::Panicked));
        assert!(previous.contains("an older kernel's log"));
        assert!(log.previous_dump().is_none());
    }

    #[test]
    fn the_previous_text_starts_with_a_whole_line() {
        let mut log = CrashLog::open(region(MIN_REGION)).unwrap();
        log.write("one\ntwo\n", &mut no_flush());
        let mut log = next_boot(log, Ending::Running);
        assert_eq!(log.previous_text(), Some((&b"one\ntwo\n"[..], &b""[..])));
        for i in 0..200 {
            log.write(&format!("line {i}\n"), &mut no_flush());
        }
        let log = next_boot(log, Ending::Running);
        let (older, newer) = log.previous_text().unwrap();
        let text = [older, newer].concat();
        let text = core::str::from_utf8(&text).unwrap();
        assert!(text.starts_with("line "), "{text}");
        assert!(text.ends_with("line 199\n"));
        let numbers: Vec<u32> =
            text.lines().map(|line| line["line ".len()..].parse().unwrap()).collect();
        assert!(numbers.windows(2).all(|pair| pair[1] == pair[0] + 1));
    }
}
