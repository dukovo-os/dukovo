// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sound (Phase 14b.1, ADR 0320): `/devices/audio`, the outputs programs
//! play through.
//!
//! Each output a driver finds is a file in `/devices/audio`: a virtio
//! sound card's `virtio0` and on (ADR 0321), an Intel HD Audio
//! controller's `hda0` and on (ADR 0322), a USB speaker's or headset's
//! `usb0` and on (ADR 0323). `default` is a link to the one in use, and
//! `speech` plays on it for the screen reader. The files
//! take OSS's requests, with Linux's numbers (the ABI's `audio` module
//! and radix-audio(2)); the arithmetic behind them, which is tested on
//! the host, is `radix-audio`'s.
//!
//! An output keeps, under its lock, a [`Playback`]: the samples written
//! and not yet played, in a ring of [`MAX_BUFFER`] bytes cut into
//! fragments. A program that plays writes into it, and waits for room. A
//! thread for each output (the *worker*) does everything the device
//! needs: it sets the device up for the program's format and fragments,
//! moves whole fragments out of the ring to it (scaled by the volume,
//! when the device has none of its own), starts it once two fragments
//! wait or the program asks for what it wrote to be played, counts the
//! fragments it hands back as played, and stops it when it runs out.
//! The device's interrupt only wakes the worker; and the worker looks
//! every 20 ms while the device plays all the same, so that a lost
//! interrupt costs a stutter, not the sound.
//!
//! One description plays on an output at a time: its *player*, which the
//! first write or request that sets the format, the rate, the channels
//! or the fragments makes it, and which others then find busy. `speech`
//! takes the output from any player but another of its own, dropping
//! what the old one queued. What a player wrote and has not been played
//! when it closes plays out. A change of format drops what was queued,
//! as OSS does: a program sets its format before it writes.
//!
//! Every change the worker or a request makes that a writer or a poll
//! may wait for moves the output's *progress* count and wakes them.
//!
//! A USB output comes and goes with its device ([`attach`], [`detach`]).
//! One that goes keeps its index and its name, and its worker waits: the
//! next device of its kind plugged in takes them, so that `usb0` is the
//! headset plugged in whatever was plugged before. Its file goes from
//! `/devices/audio` meanwhile, `default` leads to another output, and
//! what was written to it is dropped; its player's writes and requests
//! fail with ENXIO.

pub mod hda;
pub mod usb;
pub mod virtio;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use radix_abi::Status;
use radix_abi::audio::{self as abi, OutputInfo};
use radix_abi::errors::{File as FileError, Kernel};
use radix_audio::{
    Format, Fragments, MAX_BUFFER, Playback, Volume, apply_gain, nearest_channels, nearest_rate,
};
use radix_klog::{info, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, Once};
use radix_vfs::synthetic::{FixedDir, Link};
use radix_vfs::{Access, In, KArc, Kind, Node, NodeRef, Out, Stat, mem, next_id};

use crate::fs::{Description, FileObject};
use crate::sched::{self, Options, Policy, wait};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// The most outputs.
pub const MAX_OUTPUTS: usize = 8;

/// The volume an output starts at, before audiod(8) sets the one the
/// settings keep.
const START_VOLUME: u8 = 70;

/// Bytes of a program's write copied in at a time.
const STEP: usize = 16 * 1024;

/// How long the worker sleeps between looks while the device plays, and
/// while it does not.
const PLAYING_LOOK_NS: u64 = 20_000_000;
const IDLE_LOOK_NS: u64 = 1_000_000_000;

/// How long a driver waits for its device to answer a request, in steps
/// of a millisecond.
const ANSWER_MS: u32 = 1000;

/// What the kernel asks of a sound device: one output stream of signed
/// 16-bit samples, played a period (a fragment) at a time. The worker
/// alone calls it.
pub trait Driver: Send {
    /// The most periods the device can hold.
    fn max_periods(&self) -> u32;
    /// Sets the stream up for `format`, in `fragments`, stopping and
    /// letting go of whatever it played before.
    fn configure(&mut self, format: Format, fragments: Fragments) -> Result<(), &'static str>;
    /// Whether a period can be handed over now.
    fn can_submit(&self) -> bool;
    /// Hands the device a period of samples; false if it would not take
    /// it.
    fn submit(&mut self, samples: &[u8]) -> bool;
    /// Tells the device of the periods handed over.
    fn notify(&mut self);
    /// The next period the device handed back: Some(true) if it played.
    fn collect(&mut self) -> Option<bool>;
    /// Periods handed over and not yet handed back.
    fn in_flight(&self) -> u32;
    /// Starts playing.
    fn start(&mut self) -> bool;
    /// Stops playing; the periods held stay the device's.
    fn stop(&mut self) -> bool;
    /// Lets go of the stream and every period the device held, uncounted.
    fn release(&mut self);
    /// Whether the device has failed and plays nothing more.
    fn failed(&self) -> bool;
    /// Sets the device's own volume; false if it has none, when the
    /// kernel scales the samples.
    fn set_volume(&mut self, _volume: Volume) -> bool {
        false
    }
    /// Does what the device needs now and then, at each of the worker's
    /// looks (an HDA codec reads its jacks); `name` is the output's, for
    /// the log.
    fn look(&mut self, _name: &str) {}
}

/// What an output can play, found when it was set up.
#[derive(Clone, Copy)]
pub struct Caps {
    /// The rates it plays: a mask of `radix_audio::RATES` bits.
    pub rates: u32,
    /// The fewest channels it plays.
    pub channels_min: u8,
    /// The most.
    pub channels_max: u8,
    /// What it is: an `abi::KIND_*`.
    pub kind: u8,
    /// What it is, in words.
    pub model: &'static str,
    /// Whether it has a volume of its own.
    pub hardware_volume: bool,
}

/// What the output's lock keeps.
struct State {
    playback: Playback<Vec<u8, Kmalloc>>,
    /// What the player asked of SET_FRAGMENT, if it did: the fragments
    /// follow it when the format changes.
    asked: Option<u32>,
    /// The player's description, by its address, or 0.
    holder: usize,
    /// Whether the player plays through `speech`.
    speech: bool,
    volume: Volume,
    /// Whether the volume changed since the worker last gave it to the
    /// device.
    volume_changed: bool,
    /// Whether what was written plays even if it is less than the
    /// worker would wait for: after SYNC, POST, or a player closing.
    flush: bool,
    /// Moves on when what was queued is dropped (HALT, a new format, a
    /// player pushed aside by `speech`): the worker lets go of the
    /// device's stream, whose periods are the old samples.
    epoch: u64,
    underruns: u32,
    /// Bytes played since the output was found.
    played: u64,
    /// Whether the device plays.
    running: bool,
    /// Whether the device failed, which the worker says once.
    failed: bool,
    /// Whether the device was unplugged.
    gone: bool,
}

/// An output: a file of `/devices/audio` and the device behind it.
pub struct Output {
    index: usize,
    name: [u8; 16],
    name_len: usize,
    /// What the device plays: another device's when one of the kind
    /// takes a gone output's place.
    caps: Mutex<Caps>,
    /// Whether a device is behind the output: false once it is unplugged.
    present: AtomicBool,
    /// Moves on each time another device takes the output's place.
    generation: AtomicU64,
    state: Mutex<State>,
    /// The device's driver, until the worker takes it.
    driver: Mutex<Option<Box<dyn Driver, Kmalloc>>>,
    /// Set to wake the worker.
    kick: AtomicBool,
    /// Moves on with each change writers and polls may wait for.
    progress: AtomicU64,
}

/// The outputs, by index, set once each.
static OUTPUTS: [Once<Arc<Output, Kmalloc>>; MAX_OUTPUTS] = [const { Once::new() }; MAX_OUTPUTS];
/// How many outputs there are, set up or being set up.
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// The index of the output `default` leads to.
static DEFAULT: AtomicUsize = AtomicUsize::new(0);
/// Whether `default` was made: by the first output added. False again
/// while no output is there.
static DIR_DEFAULT: AtomicBool = AtomicBool::new(false);
/// Held while an output comes or goes.
static PLUGGING: Mutex<()> = Mutex::new(());
/// `/devices/audio`.
static DIR: Once<KArc<FixedDir>> = Once::new();
/// What polls of the outputs' files wait on: one for all of them.
pub static POLLED: Polled = Polled::new();

fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// The node of an output's file, or of `speech` (no index).
pub struct AudioNode {
    id: u64,
    index: Option<usize>,
    /// The output's generation the file was made for: a file of a device
    /// unplugged is not the next one's.
    generation: u64,
}

impl Node for AudioNode {
    // Root's alone: a service is given the outputs it needs by its
    // service file, and only the screen reader's is given `speech`.
    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o600 }
    }

    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::CharDevice,
            size: 0,
            allocated: 0,
            id: self.id,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::CharDevice
    }

    // The descriptions' reads and writes are `write` below; the node's
    // own find and take nothing.
    fn read(&self, _offset: u64, _out: &mut dyn Out) -> radix_vfs::Result<usize> {
        Ok(0)
    }

    fn write(&self, _offset: u64, _data: &dyn In) -> radix_vfs::Result<usize> {
        Err(radix_vfs::Error::NotSupported)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Whether `node` is an output's file or `speech`.
pub fn is_audio(node: &NodeRef) -> bool {
    radix_vfs::downcast::<AudioNode>(&**node).is_some()
}

/// `/devices/audio`, made the first time it is asked for, with `speech`
/// in it.
pub fn dir() -> radix_vfs::Result<KArc<FixedDir>> {
    if let Some(dir) = DIR.get() {
        return Ok(dir.clone());
    }
    let dir = FixedDir::new(next_id())?;
    let speech: NodeRef = mem::arc(AudioNode { id: next_id(), index: None, generation: 0 })?;
    dir.add("speech", speech)?;
    Ok(match DIR.set(dir) {
        Ok(set) => set.clone(),
        Err(_) => DIR.get().ok_or(radix_vfs::Error::NoMemory)?.clone(),
    })
}

impl Output {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("audio")
    }

    /// What the device behind it plays.
    fn caps(&self) -> Caps {
        *self.caps.lock()
    }

    /// Whether a device is behind it.
    fn present(&self) -> bool {
        self.present.load(Ordering::Acquire)
    }

    /// Wakes the worker.
    fn kick(&self) {
        self.kick.store(true, Ordering::Release);
        wait::wake_all(self.kick_key());
    }

    fn kick_key(&self) -> usize {
        core::ptr::from_ref(&self.kick).addr()
    }

    fn progress_key(&self) -> usize {
        core::ptr::from_ref(&self.progress).addr()
    }

    /// Tells writers and polls that something changed.
    fn moved(&self) {
        self.progress.fetch_add(1, Ordering::AcqRel);
        wait::wake_all(self.progress_key());
        POLLED.changed();
    }

    /// Makes the description at `me` the player, as a write or a request
    /// that sets the format does. A new player waits for what the last
    /// one left to play out, then starts in the default format. `speech`
    /// takes the output at once from another player (not another of its
    /// own), whose queued samples go and whose next write or request is
    /// refused.
    fn claim(&self, me: usize, speech: bool) -> Result<(), Status> {
        loop {
            let seen = self.progress.load(Ordering::Acquire);
            let mut state = self.state.lock();
            if state.holder == me {
                return Ok(());
            }
            // Another plays: only speech takes the output, and only from a
            // player that is not speech itself.
            let speech_takes_it = speech && !state.speech;
            if state.holder != 0 && !speech_takes_it {
                return Err(Kernel::Busy.status(0));
            }
            let leftover = !state.playback.is_drained() && !state.failed;
            if leftover && !speech {
                drop(state);
                wait::wait_until(self.progress_key(), "audio", || {
                    self.progress.load(Ordering::Acquire) != seen || sched::interrupted()
                });
                if sched::interrupted() && self.progress.load(Ordering::Acquire) == seen {
                    return Err(interrupted(Restart::Always));
                }
                continue;
            }
            state.holder = me;
            state.speech = speech;
            state.asked = None;
            let format = self.default_format();
            let fragments = Fragments::default_for(format);
            if leftover
                || format != state.playback.format()
                || fragments != state.playback.fragments()
            {
                state.playback.configure(format, fragments);
                state.epoch += 1;
            }
            state.flush = false;
            drop(state);
            self.kick();
            self.moved();
            return Ok(());
        }
    }

    /// The format a player starts in: 48 kHz stereo, or what the output
    /// plays nearest.
    fn default_format(&self) -> Format {
        let caps = self.caps();
        Format {
            rate: nearest_rate(caps.rates, Format::DEFAULT.rate).unwrap_or(Format::DEFAULT.rate),
            channels: nearest_channels(caps.channels_min, caps.channels_max, 2),
        }
    }

    /// Sets the format the player at `me` plays in, and the fragments it
    /// asks for with SET_FRAGMENT's `asked`, if it does: what was queued
    /// is dropped if either changes, as OSS drops it.
    fn set_format(&self, me: usize, format: Format, asked: Option<u32>) {
        let mut state = self.state.lock();
        if state.holder != me {
            return;
        }
        if asked.is_some() {
            state.asked = asked;
        }
        // The fragments asked for follow the format, and so do the
        // default ones.
        let fragments = match state.asked {
            Some(asked) => Fragments::requested(asked, format),
            None => Fragments::default_for(format),
        };
        if format == state.playback.format() && fragments == state.playback.fragments() {
            return;
        }
        state.playback.configure(format, fragments);
        state.epoch += 1;
        state.flush = false;
        drop(state);
        self.kick();
        self.moved();
    }

    /// The flags GET_INFO gives.
    fn flags(&self, state: &State) -> u8 {
        let mut flags = 0;
        if DEFAULT.load(Ordering::Acquire) == self.index {
            flags |= abi::FLAG_DEFAULT;
        }
        if state.holder != 0 {
            flags |= abi::FLAG_PLAYING;
        }
        if state.holder != 0 && state.speech {
            flags |= abi::FLAG_SPEECH;
        }
        if self.caps().hardware_volume {
            flags |= abi::FLAG_HARDWARE_VOLUME;
        }
        flags
    }
}

/// Takes the index of an output about to be set up, which its
/// interrupt is given before [`add`] adds it; `None` if there are as many
/// as there can be. An index taken and not added stays unused.
pub fn reserve() -> Option<usize> {
    COUNT
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_OUTPUTS).then_some(count + 1)
        })
        .ok()
}

/// Adds output `index` (from [`reserve`]), which `driver` drives, called
/// `prefix` and a number (`virtio0`), and starts its worker. Returns its
/// name.
pub fn add(
    index: usize,
    prefix: &str,
    caps: Caps,
    driver: Box<dyn Driver, Kmalloc>,
) -> Result<&'static str, &'static str> {
    if index >= MAX_OUTPUTS {
        return Err("too many sound outputs");
    }
    // The number after the prefix: how many outputs of the kind there are.
    let number = OUTPUTS[..index]
        .iter()
        .filter_map(Once::get)
        .filter(|output| output.caps().kind == caps.kind)
        .count();
    let mut name = [0u8; 16];
    let mut text = radix_klog::TextBuf::<16>::new();
    let _ = core::fmt::Write::write_fmt(&mut text, format_args!("{prefix}{number}"));
    let name_len = text.as_str().len();
    name[..name_len].copy_from_slice(text.as_str().as_bytes());
    let storage =
        radix_mm::bytes::zeroed(MAX_BUFFER as usize, kmalloc()).map_err(|_| "out of memory")?;
    if caps.rates == 0 {
        return Err("it plays no rate");
    }
    let state = State {
        playback: Playback::new(storage, Format::DEFAULT),
        asked: None,
        holder: 0,
        speech: false,
        volume: Volume::both(START_VOLUME),
        volume_changed: true,
        flush: false,
        epoch: 0,
        underruns: 0,
        played: 0,
        running: false,
        failed: false,
        gone: false,
    };
    let output = Output {
        index,
        name,
        name_len,
        caps: Mutex::new(caps),
        present: AtomicBool::new(true),
        generation: AtomicU64::new(0),
        state: Mutex::new(state),
        driver: Mutex::new(Some(driver)),
        kick: AtomicBool::new(false),
        progress: AtomicU64::new(0),
    };
    let output = Arc::try_new_in(output, kmalloc()).map_err(|_| "out of memory")?;
    {
        let format = output.default_format();
        output.state.lock().playback.configure(format, Fragments::default_for(format));
    }
    if OUTPUTS[index].set(output.clone()).is_err() {
        return Err("its index was taken");
    }
    let output: &'static Output = OUTPUTS[index].get().ok_or("its index was taken")?;
    show(output)?;
    if DIR_DEFAULT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        make_default(output);
    }
    let policy = Policy::Fair { nice: -10 };
    sched::spawn("audio", worker, index, Options::new().policy(policy))
        .map_err(|_| "could not start its thread")?;
    Ok(output.name())
}

/// Puts `output`'s file in `/devices/audio`, for its generation.
fn show(output: &Output) -> Result<(), &'static str> {
    let generation = output.generation.load(Ordering::Acquire);
    let node = AudioNode { id: next_id(), index: Some(output.index), generation };
    let node: NodeRef = mem::arc(node).map_err(|_| "out of memory")?;
    let dir = dir().map_err(|_| "no /devices/audio")?;
    dir.add(output.name(), node).map_err(|_| "out of memory")
}

/// Adds an output that comes and goes with its device, as a USB one
/// does: called `prefix` and a number, driven by `driver`. It takes the
/// place of an output of its kind whose device went, with its index and
/// name, or else is added as [`add`] adds one. Returns its index and its
/// name.
pub fn attach(
    prefix: &str,
    caps: Caps,
    driver: Box<dyn Driver, Kmalloc>,
) -> Result<(usize, &'static str), &'static str> {
    let _plugging = PLUGGING.lock();
    let gone = (0..MAX_OUTPUTS)
        .filter_map(output)
        .find(|output| !output.present() && output.caps().kind == caps.kind);
    let Some(output) = gone else {
        let index = reserve().ok_or("too many sound outputs")?;
        return Ok((index, add(index, prefix, caps, driver)?));
    };
    if caps.rates == 0 {
        return Err("it plays no rate");
    }
    *output.caps.lock() = caps;
    output.generation.fetch_add(1, Ordering::AcqRel);
    {
        let mut state = output.state.lock();
        let format = output.default_format();
        state.playback.configure(format, Fragments::default_for(format));
        state.asked = None;
        state.holder = 0;
        state.speech = false;
        state.volume_changed = true;
        state.flush = false;
        state.epoch += 1;
        state.underruns = 0;
        state.running = false;
        state.failed = false;
        state.gone = false;
    }
    *output.driver.lock() = Some(driver);
    output.present.store(true, Ordering::Release);
    show(output)?;
    if DIR_DEFAULT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        make_default(output);
    }
    output.kick();
    output.moved();
    Ok((output.index, output.name()))
}

/// The device behind output `index` was unplugged: its file goes, what
/// was written to it is dropped, its player's next write or request
/// fails, `default` leads to another output, and the worker lets go of
/// the driver. The output waits for [`attach`] to give it another.
pub fn detach(index: usize) {
    let _plugging = PLUGGING.lock();
    let Some(gone) = output(index) else { return };
    if !gone.present.swap(false, Ordering::AcqRel) {
        return;
    }
    if let Some(dir) = DIR.get() {
        dir.remove_entry(gone.name());
    }
    {
        let mut state = gone.state.lock();
        state.playback.clear();
        state.holder = 0;
        state.speech = false;
        state.flush = false;
        state.epoch += 1;
        state.failed = true;
        state.gone = true;
    }
    if DEFAULT.load(Ordering::Acquire) == index {
        match (0..MAX_OUTPUTS).filter_map(output).find(|o| o.present()) {
            Some(other) => {
                make_default(other);
                info!("Audio: {} is the default output", other.name());
            }
            None => {
                if let Some(dir) = DIR.get() {
                    dir.remove_entry("default");
                }
                DIR_DEFAULT.store(false, Ordering::Release);
            }
        }
    }
    gone.kick();
    gone.moved();
}

/// Makes `output` the one `default` leads to.
fn make_default(output: &Output) {
    DEFAULT.store(output.index, Ordering::Release);
    let made = dir().and_then(|dir| {
        let link = Link::make(next_id(), output.name())?;
        dir.add("default", link)
    });
    if made.is_err() {
        warn!("Audio: could not point /devices/audio/default at {}", output.name());
    }
}

/// The output with index `index`.
fn output(index: usize) -> Option<&'static Arc<Output, Kmalloc>> {
    OUTPUTS.get(index).and_then(Once::get)
}

/// Wakes `index`'s worker: its device's interrupt.
pub fn interrupt(index: usize) {
    if let Some(output) = output(index) {
        output.kick();
    }
}

/// The output a description plays on, and whether it plays through
/// `speech`, which takes the default output the first time it is used
/// (kept in the description's offset, plus one).
fn output_of(file: &FileObject) -> Result<(&'static Output, bool), Status> {
    let node =
        radix_vfs::downcast::<AudioNode>(&*file.at.node).ok_or(Kernel::NotTerminal.status(0))?;
    let none = || Kernel::NotSupported.status(0);
    match node.index {
        Some(index) => {
            let output = output(index).ok_or_else(none)?;
            let generation = output.generation.load(Ordering::Acquire);
            if !output.present() || generation != node.generation {
                return Err(gone_status());
            }
            Ok((output, false))
        }
        None => {
            let mut bound = file.offset.lock();
            if *bound == 0 {
                if !DIR_DEFAULT.load(Ordering::Acquire) {
                    return Err(none());
                }
                *bound = DEFAULT.load(Ordering::Acquire) as u64 + 1;
            }
            let index = (*bound - 1) as usize;
            let output = output(index).ok_or_else(none)?;
            if !output.present() {
                // The screen reader opens `speech` again, for `default`.
                return Err(gone_status());
            }
            Ok((output, true))
        }
    }
}

/// Writes `data` to the output `file` plays on, making it the player,
/// and waiting for room unless `non_blocking`. Returns how many bytes it
/// took.
pub fn write(file: &FileObject, data: &dyn In, non_blocking: bool) -> Result<usize, Status> {
    let (output, speech) = output_of(file)?;
    let me = file.address();
    output.claim(me, speech)?;
    let len = data.len();
    if len == 0 {
        return Ok(0);
    }
    let mut buffer = radix_mm::bytes::zeroed(len.min(STEP), kmalloc())
        .map_err(|_| Kernel::NoMemory.status(0))?;
    let mut done = 0;
    while done < len {
        let step = (len - done).min(STEP);
        // Copied in first, with no lock held: the copy may wait for a page.
        data.get(done, &mut buffer[..step]).map_err(crate::fs::status)?;
        let mut put = 0;
        while put < step {
            let seen = output.progress.load(Ordering::Acquire);
            let taken = {
                let mut state = output.state.lock();
                if state.gone && done + put == 0 {
                    return Err(gone_status());
                }
                if state.holder != me {
                    // Pushed aside by `speech`, or its device went.
                    drop(state);
                    return if done + put > 0 {
                        Ok(done + put)
                    } else {
                        Err(Kernel::Busy.status(0))
                    };
                }
                state.playback.write(&buffer[put..step])
            };
            if taken > 0 {
                put += taken;
                output.kick();
                continue;
            }
            if non_blocking {
                return if done + put > 0 {
                    Ok(done + put)
                } else {
                    Err(Kernel::WouldBlock.status(0))
                };
            }
            wait::wait_until(output.progress_key(), "audio", || {
                output.progress.load(Ordering::Acquire) != seen || sched::interrupted()
            });
            if sched::interrupted() && output.progress.load(Ordering::Acquire) == seen {
                return if done + put > 0 {
                    Ok(done + put)
                } else {
                    Err(interrupted(Restart::Always))
                };
            }
        }
        done += step;
    }
    Ok(done)
}

/// The error a player of an output whose device went is given: ENXIO.
fn gone_status() -> Status {
    FileError::NoDevice.status(0)
}

/// What `poll` finds on `file`: writable while a fragment's worth of
/// room is free, or while another plays (when a write fails at once).
pub fn events(file: &FileObject) -> u16 {
    let Ok((output, _)) = output_of(file) else { return poll::WRITABLE };
    let state = output.state.lock();
    let mine = state.holder == 0 || state.holder == file.address();
    if !mine || state.playback.room() >= state.playback.fragments().bytes {
        poll::WRITABLE
    } else {
        0
    }
}

/// The description at `file` has gone: what its player wrote plays out.
pub fn closed(file: &FileObject) {
    let Some(node) = radix_vfs::downcast::<AudioNode>(&*file.at.node) else { return };
    let index = match node.index {
        Some(index) => index,
        None => match *file.offset.lock() {
            0 => return,
            bound => (bound - 1) as usize,
        },
    };
    let Some(output) = output(index) else { return };
    let mut state = output.state.lock();
    if state.holder != file.address() {
        return;
    }
    state.holder = 0;
    state.speech = false;
    state.playback.fill_out();
    state.flush = true;
    drop(state);
    output.kick();
    output.moved();
}

/// The int at `buffer`, for a request that reads one.
fn int_in(copy_in: &mut dyn FnMut(&mut [u8]) -> Result<(), Status>) -> Result<u32, Status> {
    let mut bytes = [0u8; 4];
    copy_in(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// A file request: OSS's, and GET_INFO and MAKE_DEFAULT. `may_write`
/// says whether the description's capability has the write right, which
/// every request that changes something needs. `copy_in` reads the
/// caller's buffer and `copy_out` writes it.
pub fn control(
    file: &FileObject,
    request: u32,
    may_write: bool,
    copy_in: &mut dyn FnMut(&mut [u8]) -> Result<(), Status>,
    copy_out: &mut dyn FnMut(&[u8]) -> Result<(), Status>,
) -> Result<(), Status> {
    let (output, speech) = output_of(file)?;
    let me = file.address();
    let rights = || Kernel::MissingRights.status(0);
    let int_out = |copy_out: &mut dyn FnMut(&[u8]) -> Result<(), Status>, value: u32| {
        copy_out(&value.to_le_bytes())
    };
    match request {
        abi::SPEED | abi::CHANNELS | abi::SET_FORMAT | abi::SET_FRAGMENT => {
            let asked = int_in(copy_in)?;
            if request == abi::SET_FORMAT && asked == abi::FORMAT_QUERY {
                return int_out(copy_out, abi::FORMAT_S16_LE);
            }
            if !may_write {
                return Err(rights());
            }
            output.claim(me, speech)?;
            let format = output.state.lock().playback.format();
            let caps = output.caps();
            let (format, fragments, answer) = match request {
                abi::SPEED => {
                    let rate = nearest_rate(caps.rates, asked).unwrap_or(format.rate);
                    (Format { rate, ..format }, None, rate)
                }
                abi::CHANNELS => {
                    let channels = nearest_channels(caps.channels_min, caps.channels_max, asked);
                    (Format { channels, ..format }, None, u32::from(channels))
                }
                abi::SET_FORMAT => (format, None, abi::FORMAT_S16_LE),
                _ => (format, Some(asked), asked),
            };
            output.set_format(me, format, fragments);
            if request == abi::SET_FRAGMENT {
                // OSS writes nothing back to SETFRAGMENT's argument.
                return Ok(());
            }
            int_out(copy_out, answer)
        }
        abi::GET_BLOCK_SIZE => {
            let bytes = output.state.lock().playback.fragments().bytes;
            int_out(copy_out, bytes)
        }
        abi::GET_FORMATS => int_out(copy_out, abi::FORMAT_S16_LE),
        abi::GET_CAPABILITIES => {
            int_out(copy_out, abi::CAPABILITY_OUTPUT | abi::CAPABILITY_REVISION)
        }
        abi::GET_OUTPUT_SPACE => {
            let space = output.state.lock().playback.space();
            let mut bytes = [0u8; abi::OutputSpace::SIZE];
            for (at, value) in [
                (abi::OutputSpace::OFFSET_FRAGMENTS, space.fragments),
                (abi::OutputSpace::OFFSET_TOTAL, space.total),
                (abi::OutputSpace::OFFSET_FRAGMENT_BYTES, space.fragment_bytes),
                (abi::OutputSpace::OFFSET_BYTES, space.bytes),
            ] {
                bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
            copy_out(&bytes)
        }
        abi::GET_OUTPUT_DELAY => {
            let delay = output.state.lock().playback.delay();
            int_out(copy_out, delay)
        }
        abi::GET_PLAY_VOLUME => {
            let volume = output.state.lock().volume;
            int_out(copy_out, volume.encode())
        }
        abi::SET_PLAY_VOLUME => {
            if !may_write {
                return Err(rights());
            }
            let volume = Volume::decode(int_in(copy_in)?);
            {
                let mut state = output.state.lock();
                state.volume = volume;
                state.volume_changed = true;
            }
            output.kick();
            output.moved();
            int_out(copy_out, volume.encode())
        }
        abi::NON_BLOCK => {
            file.set_description(Description { non_blocking: true, ..file.description() });
            Ok(())
        }
        abi::HALT | abi::HALT_OUTPUT | abi::POST | abi::SYNC => {
            if !may_write {
                return Err(rights());
            }
            {
                let mut state = output.state.lock();
                if state.holder != me {
                    // Nothing of this description's plays: nothing to do,
                    // unless another took the output from it.
                    return if state.holder == 0 { Ok(()) } else { Err(Kernel::Busy.status(0)) };
                }
                if matches!(request, abi::HALT | abi::HALT_OUTPUT) {
                    state.playback.clear();
                    state.epoch += 1;
                    state.flush = false;
                } else {
                    state.playback.fill_out();
                    state.flush = true;
                }
            }
            output.kick();
            output.moved();
            if request == abi::SYNC {
                sync(output, me)?;
            }
            Ok(())
        }
        abi::GET_INFO => {
            let caps = output.caps();
            let state = output.state.lock();
            let mut bytes = [0u8; OutputInfo::SIZE];
            let put32 = |bytes: &mut [u8], at: usize, value: u32| {
                bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            };
            put32(&mut bytes, OutputInfo::OFFSET_RATES, caps.rates);
            put32(&mut bytes, OutputInfo::OFFSET_VOLUME, state.volume.encode());
            bytes[OutputInfo::OFFSET_PLAYED..][..8].copy_from_slice(&state.played.to_le_bytes());
            put32(&mut bytes, OutputInfo::OFFSET_UNDERRUNS, state.underruns);
            bytes[OutputInfo::OFFSET_CHANNELS_MIN] = caps.channels_min;
            bytes[OutputInfo::OFFSET_CHANNELS_MAX] = caps.channels_max;
            bytes[OutputInfo::OFFSET_KIND] = caps.kind;
            bytes[OutputInfo::OFFSET_FLAGS] = output.flags(&state);
            drop(state);
            bytes[OutputInfo::OFFSET_NAME..][..output.name_len]
                .copy_from_slice(&output.name[..output.name_len]);
            let model = caps.model.as_bytes();
            let model = &model[..model.len().min(31)];
            bytes[OutputInfo::OFFSET_MODEL..][..model.len()].copy_from_slice(model);
            copy_out(&bytes)
        }
        abi::MAKE_DEFAULT => {
            if !may_write {
                return Err(rights());
            }
            make_default(output);
            info!("Audio: {} is the default output", output.name());
            output.moved();
            Ok(())
        }
        _ => Err(Kernel::NotTerminal.status(0)),
    }
}

/// SYNC: waits until everything the player at `me` wrote has played.
fn sync(output: &Output, me: usize) -> Result<(), Status> {
    loop {
        let seen = output.progress.load(Ordering::Acquire);
        {
            let state = output.state.lock();
            if state.holder != me {
                return Err(Kernel::Busy.status(0));
            }
            if state.playback.is_drained() || state.failed {
                return Ok(());
            }
        }
        wait::wait_until(output.progress_key(), "audio sync", || {
            output.progress.load(Ordering::Acquire) != seen || sched::interrupted()
        });
        if sched::interrupted() && output.progress.load(Ordering::Acquire) == seen {
            return Err(interrupted(Restart::Always));
        }
    }
}

/// What the worker knows of the device.
struct Device {
    /// The format and fragments the stream is set up for, if it is.
    prepared: Option<(Format, Fragments)>,
    /// The queue's epoch the stream plays.
    epoch: u64,
    running: bool,
}

/// An output's worker: moves the samples to the device and back.
fn worker(index: usize) {
    let Some(output) = output(index) else { return };
    let Ok(mut staging) = radix_mm::bytes::zeroed(radix_audio::MAX_FRAGMENT as usize, kmalloc())
    else {
        warn!("Audio: no memory for {}'s worker", output.name());
        return;
    };
    loop {
        // The driver of the device behind the output, until it goes.
        let driver = output.driver.lock().take();
        if let Some(mut driver) = driver {
            let mut device = Device { prepared: None, epoch: 0, running: false };
            while output.present() {
                output.kick.store(false, Ordering::Release);
                step(output, &mut *driver, &mut staging, &mut device);
                let wait = if device.running { PLAYING_LOOK_NS } else { IDLE_LOOK_NS };
                let deadline = sched::now_ns() + wait;
                wait::wait_until_or_timeout(output.kick_key(), "audio", deadline, || {
                    output.kick.load(Ordering::Acquire)
                });
            }
            // Unplugged: the stream goes with the driver.
            if device.running {
                driver.stop();
            }
            if device.prepared.is_some() {
                driver.release();
            }
            drop(driver);
            output.state.lock().running = false;
            output.moved();
        }
        // Until another device takes the output's place.
        wait::wait_until(output.kick_key(), "audio", || output.kick.load(Ordering::Acquire));
        output.kick.store(false, Ordering::Release);
    }
}

/// One look of the worker's.
fn step(output: &Output, driver: &mut dyn Driver, staging: &mut [u8], device: &mut Device) {
    let mut changed = false;
    if driver.failed() {
        let mut state = output.state.lock();
        // An unplugged device's driver fails too, which is no news.
        if !state.failed && !state.gone {
            state.failed = true;
            state.playback.clear();
            drop(state);
            warn!("Audio: {} stopped answering; it plays nothing more", output.name());
            output.moved();
        }
        return;
    }
    driver.look(output.name());
    // What was queued was dropped: so are the device's periods.
    let (epoch, volume_changed, volume) = {
        let mut state = output.state.lock();
        (state.epoch, core::mem::take(&mut state.volume_changed), state.volume)
    };
    if epoch != device.epoch {
        if device.running {
            driver.stop();
            device.running = false;
        }
        if device.prepared.take().is_some() {
            driver.release();
        }
        device.epoch = epoch;
        changed = true;
    }
    let hardware_volume = output.caps().hardware_volume;
    if volume_changed && hardware_volume {
        driver.set_volume(volume);
    }
    // The periods played.
    while let Some(_played) = driver.collect() {
        let mut state = output.state.lock();
        if state.epoch == device.epoch && state.playback.played_one() {
            state.played += u64::from(state.playback.fragments().bytes);
        }
        changed = true;
    }
    // Set up for what waits to be played.
    let (format, fragments, queued) = {
        let state = output.state.lock();
        (state.playback.format(), state.playback.fragments(), state.playback.queued())
    };
    if queued > 0 && device.prepared != Some((format, fragments)) {
        if device.running {
            driver.stop();
            device.running = false;
        }
        let fitted = Fragments { count: fragments.count.min(driver.max_periods()), ..fragments };
        match driver.configure(format, fitted) {
            Ok(()) => device.prepared = Some((format, fragments)),
            Err(why) => {
                warn!("Audio: {} could not be set up to play: {why}", output.name());
                let mut state = output.state.lock();
                state.playback.clear();
                state.epoch += 1;
                drop(state);
                output.moved();
                return;
            }
        }
    }
    // Whole fragments to the device, scaled by the volume.
    let mut submitted = false;
    while device.prepared.is_some() && driver.can_submit() {
        let (taken, volume) = {
            let mut state = output.state.lock();
            if state.epoch != device.epoch {
                break;
            }
            (state.playback.take(staging), state.volume)
        };
        let Some(bytes) = taken else { break };
        if !hardware_volume {
            apply_gain(&mut staging[..bytes], format.channels, volume);
        }
        if !driver.submit(&staging[..bytes]) {
            break;
        }
        submitted = true;
    }
    if submitted {
        driver.notify();
    }
    // Start once two fragments wait, or what was written is to play;
    // stop when the device runs out.
    let in_flight = driver.in_flight();
    let mut state = output.state.lock();
    let flush = state.flush;
    if !device.running && in_flight > 0 && (in_flight >= 2.min(fragments.count) || flush) {
        drop(state);
        device.running = driver.start();
        state = output.state.lock();
        changed = true;
    } else if device.running && in_flight == 0 {
        drop(state);
        driver.stop();
        device.running = false;
        state = output.state.lock();
        if !flush && state.holder != 0 {
            state.underruns = state.underruns.saturating_add(1);
        }
        changed = true;
    }
    if flush && state.playback.is_drained() && !device.running {
        state.flush = false;
        if state.holder == 0 && device.prepared.take().is_some() {
            // Nobody plays: the device's stream goes until somebody does.
            drop(state);
            driver.release();
            state = output.state.lock();
        }
        changed = true;
    }
    state.running = device.running;
    drop(state);
    if changed {
        output.moved();
    }
}
