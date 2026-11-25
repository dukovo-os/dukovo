// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A profile of user code: which instructions of which programs the CPUs
//! ran, sampled every [`PERIOD_NS`], for finding where a program's time
//! goes, as `/processes/profile` gives it (radix-proc(5)).
//!
//! While a profile is taken, the architecture's timer interrupt calls
//! [`sample`] on every CPU once a period (`Timer::sample_every`). A sample
//! that interrupted user code keeps the address, with the number of a slot
//! of its CPU that holds the thread's address space and process; the
//! others are only counted. Nothing is allocated or freed there: the
//! reader makes the buffer and room for the slots beforehand, a slot is
//! filled by counting two references up, and the CPU's lock is only tried,
//! so a sample that finds the reader there is counted as lost.
//!
//! Each open of the file swaps every CPU's buffer for an empty one and
//! writes what the old ones hold ([`write`]): the samples of the opener's
//! own processes (every process's, for root), counted by process name,
//! file and offset in the file, the most first. An address is looked up in
//! the parts of files its space maps to run
//! (`SpaceObject::each_running_file`), or, for a space emptied meanwhile
//! by its process's end or an `exec`, in the copy of them that
//! [`space_ends`] kept as it was emptied. The first open starts the
//! sampling, and it stops by itself [`STOP_AFTER_NS`] after the last.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

use radix_hal::{CpuLocal, MAX_CPUS, Timer};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, SpinLock};
use radix_vfs::Text;

use super::memory::{SpaceObject, SpaceRef};
use super::process::ProcessRef;
use crate::Arch;
use crate::fs::Opened;
use crate::fs::procfs::{LINE_SPECIAL, write_escaped};
use crate::sched::{self, Name};

/// How often each CPU takes a sample: 2,000 times a second.
pub const PERIOD_NS: u64 = 500_000;

/// How long sampling goes on after the file was last opened: ten minutes.
pub const STOP_AFTER_NS: u64 = 600_000_000_000;

/// The samples a CPU keeps between two opens: 16 s of a busy CPU.
const SAMPLES: usize = 32 * 1024;

/// The address spaces a CPU tells apart between two opens; a sample in
/// another is lost.
const SLOTS: usize = 64;

/// The emptied spaces whose files are kept between two opens; the oldest
/// goes first.
const ENDED_MAX: usize = 1024;

/// A sample holds its slot's number above these bits and the user
/// address in them, which is below 2^47 on every architecture.
const ADDRESS_BITS: u32 = 56;
const ADDRESS_MASK: u64 = (1 << ADDRESS_BITS) - 1;

/// The key a sample is counted under: the process name's index in the top
/// byte, the file's index in the next 12 bits, the offset in the file in
/// the rest. [`NO_FILE`] for an address in no file the profile names
/// (the vDSO, anonymous memory), with offset 0.
const NAMES_MAX: usize = 255;
const FILES_MAX: usize = 1024;
const NO_FILE: u64 = 0xfff;
const OFFSET_MASK: u64 = (1 << 44) - 1;

/// The longest line of a sample, with room to spare: the file stops
/// listing samples this far short of [`Text::LIMIT`].
const SAMPLE_LINE: usize = 64;

/// The most kinds of counts the architecture gives (`Timer::profile_counts`)
/// with the memory's (`take_file_page_counts`).
const COUNTS_MAX: usize = 32;

/// What a slot holds: the address space its samples were taken in and the
/// process that ran there, each counted up while the slot holds it.
struct Slot {
    space: SpaceRef,
    process: ProcessRef,
    /// How many times the space's mappings had changed by the slot's
    /// first sample (`SpaceObject::tlb_changes`).
    changes: u64,
}

/// A CPU's samples since the last open.
struct Buffer {
    samples: Vec<u64, Kmalloc>,
    slots: Vec<Slot, Kmalloc>,
    /// Samples that interrupted the kernel or an idle CPU.
    kernel: u64,
    /// Samples that found the samples or the slots full.
    lost: u64,
}

impl Buffer {
    /// An empty buffer with room for [`SAMPLES`] samples and [`SLOTS`]
    /// slots, or `None` without the memory.
    fn new(heap: Kmalloc) -> Option<Buffer> {
        let mut samples = Vec::new_in(heap);
        let mut slots = Vec::new_in(heap);
        samples.try_reserve_exact(SAMPLES).ok()?;
        slots.try_reserve_exact(SLOTS).ok()?;
        Some(Buffer { samples, slots, kernel: 0, lost: 0 })
    }
}

/// A part of a file mapped to run, as an emptied space had it.
struct Part {
    start: u64,
    end: u64,
    /// Where its start is in the file.
    offset: u64,
    /// The file's path, empty when it is not known.
    path: Vec<u8, Kmalloc>,
}

/// The parts of files an emptied space ran.
struct Ended {
    /// The space's address. A slot that holds the space keeps the address
    /// its own; among records of one address, the last is the latest.
    space: usize,
    /// The [`GENERATION`] of buffers it was emptied in: the only buffers
    /// that can hold its samples, but for the ones the reader is reading
    /// as it is emptied.
    generation: u64,
    parts: Vec<Part, Kmalloc>,
}

/// Each CPU's samples since the last open; `None` before the first, and
/// for a CPU started since.
static BUFFERS: [SpinLock<Option<Buffer>>; MAX_CPUS] = [const { SpinLock::new(None) }; MAX_CPUS];

/// Samples that found their CPU's buffer held by the reader.
static MISSED: AtomicU64 = AtomicU64::new(0);

/// When the file was last opened (`Timer::now_ns`), or 0 before.
static OPENED_NS: AtomicU64 = AtomicU64::new(0);

/// Held by the reader: one open at a time swaps the buffers.
static READER: Mutex<()> = Mutex::new(());

/// The spaces emptied while sampling, since the open before the last.
static ENDED: Mutex<Option<Vec<Ended, Kmalloc>>> = Mutex::new(None);

/// The buffers' generation: each open adds one before it swaps them.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn heap() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// Adds `value` to `values`, growing it if need be; false without the
/// memory, and `value` is dropped.
fn push<T>(values: &mut Vec<T, Kmalloc>, value: T) -> bool {
    values.try_reserve(1).is_ok() && values.push_within_capacity(value).is_ok()
}

/// A copy of `bytes`, or `None` without the memory.
fn copy(bytes: &[u8]) -> Option<Vec<u8, Kmalloc>> {
    let mut copy = Vec::new_in(heap());
    radix_mm::bytes::extend(&mut copy, bytes).ok()?;
    Some(copy)
}

/// Whether a profile is being taken.
fn sampling() -> bool {
    let opened = OPENED_NS.load(Ordering::Relaxed);
    opened != 0 && Arch::now_ns().saturating_sub(opened) <= STOP_AFTER_NS
}

/// Takes this CPU's sample: the timer interrupt calls it, with interrupts
/// masked, with the user address it interrupted, or 0.
fn sample(user_address: u64) {
    if !sampling() {
        let _ = Arch::sample_every(0, sample);
        return;
    }
    let Some(mut held) = BUFFERS[Arch::current_cpu() % MAX_CPUS].try_lock() else {
        MISSED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Some(buffer) = held.as_mut() else { return };
    let running = if user_address == 0 { None } else { sched::running() };
    let Some(user) = running.as_ref().and_then(|thread| thread.user()) else {
        buffer.kernel += 1;
        return;
    };
    let space = user.space_ref();
    let slot = match buffer.slots.iter().position(|slot| Arc::ptr_eq(&slot.space, space)) {
        Some(slot) => slot,
        None if buffer.slots.len() < SLOTS => {
            // The running thread holds both too, so a count dropped here
            // could never be the last.
            let slot = Slot {
                space: Arc::clone(space),
                process: Arc::clone(&user.process),
                changes: space.tlb_changes(),
            };
            if buffer.slots.push_within_capacity(slot).is_err() {
                buffer.lost += 1;
                return;
            }
            buffer.slots.len() - 1
        }
        None => {
            buffer.lost += 1;
            return;
        }
    };
    let entry = ((slot as u64) << ADDRESS_BITS) | (user_address & ADDRESS_MASK);
    if buffer.samples.push_within_capacity(entry).is_err() {
        buffer.lost += 1;
    }
}

/// Keeps the parts of files that `space`, about to be emptied, maps to
/// run, while a profile is taken: the samples taken in it are looked up
/// in them.
pub fn space_ends(space: &SpaceObject) {
    if !sampling() {
        return;
    }
    let mut parts = Vec::new_in(heap());
    space.each_running_file(&mut |start, end, offset, file| {
        let path = file.and_then(Opened::path).map_or(&[][..], |(bytes, _)| bytes);
        if let Some(path) = copy(path) {
            push(&mut parts, Part { start, end, offset, path });
        }
    });
    let generation = GENERATION.load(Ordering::Relaxed);
    let record = Ended { space: core::ptr::from_ref(space) as usize, generation, parts };
    let mut ended = ENDED.lock();
    let ended = ended.get_or_insert_with(|| Vec::new_in(heap()));
    if ended.len() == ENDED_MAX {
        ended.remove(0);
    }
    push(ended, record);
}

/// A part of a file mapped to run, with the file's index in the names
/// the profile lists.
struct Mapped {
    start: u64,
    end: u64,
    offset: u64,
    file: u64,
}

/// How a slot's samples are counted: under its process's name, by the
/// parts of files its space ran.
struct View {
    name: u64,
    parts: Vec<Mapped, Kmalloc>,
}

impl View {
    /// The key `address` is counted under.
    fn key(&self, address: u64) -> u64 {
        let found = self.parts.iter().find(|part| part.start <= address && address < part.end);
        let (file, offset) = match found {
            Some(part) => (part.file, (address - part.start + part.offset) & OFFSET_MASK),
            None => (NO_FILE, 0),
        };
        (self.name << 56) | (file << 44) | offset
    }
}

/// The names and paths the profile lists, each once.
struct Names {
    processes: Vec<Name, Kmalloc>,
    files: Vec<Vec<u8, Kmalloc>, Kmalloc>,
}

impl Names {
    /// `name`'s index, added if it is new; the last index is shared once
    /// [`NAMES_MAX`] are listed.
    fn process(&mut self, name: Name) -> u64 {
        if let Some(at) = self.processes.iter().position(|&known| known == name) {
            return at as u64;
        }
        if self.processes.len() < NAMES_MAX && push(&mut self.processes, name) {
            return self.processes.len() as u64 - 1;
        }
        NAMES_MAX as u64 - 1
    }

    /// `path`'s index, added if it is new, or `None` once [`FILES_MAX`]
    /// are listed.
    fn file(&mut self, path: &[u8]) -> Option<u64> {
        if let Some(at) = self.files.iter().position(|known| known.as_slice() == path) {
            return Some(at as u64);
        }
        if self.files.len() == FILES_MAX || !push(&mut self.files, copy(path)?) {
            return None;
        }
        Some(self.files.len() as u64 - 1)
    }
}

/// How `slot`'s samples are counted, or `None` if `reader` (the user the
/// opener acts for) may not see them: only root sees other users'. A
/// space emptied since the samples were taken, even while the reader
/// reads them, is looked up in [`ENDED`].
fn view(slot: &Slot, reader: Option<u32>, names: &mut Names) -> Option<View> {
    let reader = reader?;
    if reader != 0 && reader != slot.process.identity().user {
        return None;
    }
    let name = names.process(slot.process.name());
    let mut parts = Vec::new_in(heap());
    slot.space.each_running_file(&mut |start, end, offset, file| {
        let path = file.and_then(Opened::path).map_or(&[][..], |(bytes, _)| bytes);
        if let Some(file) = names.file(path) {
            push(&mut parts, Mapped { start, end, offset, file });
        }
    });
    if parts.is_empty() {
        let space = Arc::as_ptr(&slot.space) as usize;
        let ended = ENDED.lock();
        let ended = ended.as_ref().map_or(&[][..], |ended| ended.as_slice());
        let record = ended.iter().rev().find(|record| record.space == space);
        for part in record.map_or(&[][..], |record| record.parts.as_slice()) {
            if let Some(file) = names.file(&part.path) {
                push(
                    &mut parts,
                    Mapped { start: part.start, end: part.end, offset: part.offset, file },
                );
            }
        }
    }
    Some(View { name, parts })
}

/// Writes the samples taken since the last open and starts afresh, and
/// starts sampling if it had stopped: the text of `/processes/profile`
/// (radix-proc(5)).
pub fn write(text: &mut Text) -> fmt::Result {
    let _reader = READER.lock();
    OPENED_NS.store(Arch::now_ns().max(1), Ordering::Relaxed);
    // Spaces emptied from here on may have samples in the fresh buffers.
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let mut taken = Vec::new_in(heap());
    for cpu in sched::online().iter() {
        // Without the memory for a fresh buffer, the CPU keeps the one it
        // has until the next open.
        let Some(fresh) = Buffer::new(heap()) else { continue };
        if taken.try_reserve(1).is_err() {
            break;
        }
        let old = BUFFERS[cpu % MAX_CPUS].lock_irq().replace(fresh);
        if let Some(old) = old {
            // There is room: reserved above.
            let _ = taken.push_within_capacity(old);
        }
    }
    let period = if Arch::sample_every(PERIOD_NS, sample) { PERIOD_NS } else { 0 };
    let mut lost = MISSED.swap(0, Ordering::Relaxed);
    let mut counts = [("", 0u64); COUNTS_MAX];
    let mut counted_kinds = 0;
    let mut add = |name, count| {
        if let Some(slot) = counts.get_mut(counted_kinds) {
            *slot = (name, count);
            counted_kinds += 1;
        }
    };
    Arch::profile_counts(&mut add);
    super::memory::take_file_page_counts(&mut add);
    // How often each space with samples changed its mappings since its
    // first sample: (space, name, changes), each space once, by the slot
    // that saw it first, whatever CPU's.
    let mut spaces: Vec<(usize, u64, u64), Kmalloc> = Vec::new_in(heap());

    let reader = sched::current().and_then(|me| me.user().map(|user| user.process.identity().user));
    let mut names = Names { processes: Vec::new_in(heap()), files: Vec::new_in(heap()) };
    let (mut others, mut kernel) = (0u64, 0u64);
    let mut keys = Vec::new_in(heap());
    let all: usize = taken.iter().map(|buffer: &Buffer| buffer.samples.len()).sum();
    if keys.try_reserve_exact(all).is_err() {
        lost += all as u64;
    }
    for buffer in &taken {
        kernel += buffer.kernel;
        lost += buffer.lost;
        if keys.capacity() < all {
            continue;
        }
        let mut views = Vec::new_in(heap());
        if views.try_reserve_exact(buffer.slots.len()).is_err() {
            lost += buffer.samples.len() as u64;
            continue;
        }
        for slot in &buffer.slots {
            let view = view(slot, reader, &mut names);
            if let Some(view) = &view {
                let space = Arc::as_ptr(&slot.space) as usize;
                let changes = slot.space.tlb_changes().saturating_sub(slot.changes);
                match spaces.iter_mut().find(|known| known.0 == space) {
                    Some(known) => known.2 = known.2.max(changes),
                    None => {
                        push(&mut spaces, (space, view.name, changes));
                    }
                }
            }
            let _ = views.push_within_capacity(view);
        }
        for &entry in &buffer.samples {
            let slot = (entry >> ADDRESS_BITS) as usize;
            match views.get(slot) {
                Some(Some(view)) => {
                    let _ = keys.push_within_capacity(view.key(entry & ADDRESS_MASK));
                }
                _ => others += 1,
            }
        }
    }

    // Only the spaces emptied in the fresh buffers' generation can have
    // samples in them.
    if let Some(ended) = ENDED.lock().as_mut() {
        ended.retain(|record| record.generation == generation);
    }

    // Counted by key, the most first.
    keys.sort_unstable();
    let distinct = 1 + keys.windows(2).filter(|pair| pair[0] != pair[1]).count();
    let mut counted: Vec<(u64, u64), Kmalloc> = Vec::new_in(heap());
    if counted.try_reserve_exact(distinct).is_err() {
        lost += keys.len() as u64;
        keys.clear();
    }
    for &key in &keys {
        match counted.last_mut() {
            Some((count, last)) if *last == key => *count += 1,
            _ => {
                let _ = counted.push_within_capacity((1, key));
            }
        }
    }
    counted.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    writeln!(text, "period_ns: {period}")?;
    writeln!(text, "user: {}", keys.len())?;
    writeln!(text, "others: {others}")?;
    writeln!(text, "kernel: {kernel}")?;
    writeln!(text, "lost: {lost}")?;
    for &(name, count) in &counts[..counted_kinds] {
        writeln!(text, "counter {name} {count}")?;
    }
    for (at, name) in names.processes.iter().enumerate() {
        write!(text, "name {at} ")?;
        write_escaped(text, name.as_str().as_bytes(), LINE_SPECIAL)?;
        text.write_char('\n')?;
    }
    for (at, path) in names.files.iter().enumerate() {
        write!(text, "file {at} ")?;
        let path = if path.is_empty() { &b"?"[..] } else { path.as_slice() };
        write_escaped(text, path, LINE_SPECIAL)?;
        text.write_char('\n')?;
    }
    // Changes by process name, in the order the names are listed.
    for (at, _) in names.processes.iter().enumerate() {
        let changes: u64 =
            spaces.iter().filter(|space| space.1 == at as u64).map(|space| space.2).sum();
        if changes != 0 {
            writeln!(text, "changes {at} {changes}")?;
        }
    }
    let mut left_out = 0;
    for &(count, key) in &counted {
        if left_out != 0 || text.bytes().len() + SAMPLE_LINE > Text::LIMIT {
            left_out += count;
            continue;
        }
        let (name, file, offset) = (key >> 56, (key >> 44) & NO_FILE, key & OFFSET_MASK);
        if file == NO_FILE {
            writeln!(text, "sample {count} {name} - -")?;
        } else {
            writeln!(text, "sample {count} {name} {file} {offset:x}")?;
        }
    }
    writeln!(text, "left_out: {left_out}")
}
