// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Screens (release 1.1, ADR 0700): `/devices/display`, the layer every
//! picture on a screen goes through.
//!
//! A card's driver gives the layer its screens, pipes and planes
//! ([`add_card`]) and from then on does what the layer asks through
//! [`Driver`]: show a frame, write rows into a picture it keeps, give
//! the kernel's log its screen. The layer does the rest for every card
//! alike. Each description of `/devices/display/control` is a holder,
//! with its own pictures and the state it last asked for. The holders
//! are in a stack with the kernel's log at the bottom, and the one on
//! top is in front: only its changes reach a card. The front is lent and
//! given back, never taken. A holder that ends when its description is
//! closed, as one that is killed does, leaves its last picture on the
//! screens: its lender draws over it when it chooses, and with no lender
//! the kernel's log waits three seconds for whatever is started in the
//! holder's place, unless the log has an error to show ([`beat_thread`]).
//!
//! The layer itself is `radix_display::layer`, with the rules it keeps,
//! where all of it is tested on the host. What is here is what only the
//! kernel has: the lock, the files and their requests (`requests`,
//! radix-display(2)), the memory a picture's pixels are in ([`Pages`]),
//! the drivers, and the threads that keep the beats.
//!
//! One lock keeps the layer. A request copies the program's bytes in
//! before it takes the lock and writes its answer back after, so the
//! lock is never held across a fault. A driver is called with the lock
//! held: what a card shows changes in the order the lock was taken, and
//! a frame never mixes two changes.
//!
//! A change shows at its pipes' next beat. A thread for each card sleeps
//! until the earliest beat anything waits for, makes the changes that
//! are due and tells the holders that asked ([`beat_thread`]). The cards
//! of release 1.1 have no beat of their own, so the kernel's clock keeps
//! it (`radix_display::beat`, ADR 0703): a thread that sleeps while no
//! change waits costs nothing on a still screen. The same thread is the
//! one a driver's own work is done on ([`Driver::look`]): a monitor that
//! was plugged in, a line of the kernel's log to send to a screen.
//!
//! A card can go while the system runs ([`remove_card`]): the firmware's
//! framebuffer does when a driver takes the device it is the memory of
//! (ADR 0705). Its thread ends the next time it wakes.
//!
//! Everything a holder may wait for (an event to read, a change to show)
//! moves one count, [`PROGRESS`], after the lock is let go: a thread
//! that is woken finds the lock free.
//!
//! Input follows the screen (ADR 0706): whenever the front passes or the
//! attention file changes hands, the input hub is told before the lock
//! is let go ([`tell_input`]), and the secure key raises the attention
//! holder through [`secure_key`] (ADR 0708). The hub's lock is taken
//! inside this one, never the other way round.

mod firmware;
mod report;
mod requests;
pub mod virtio;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use radix_abi::Status;
use radix_abi::address_space::PAGE_SIZE;
use radix_abi::display::DisplayEvent;
use radix_abi::errors::Kernel;
use radix_display::Rect;
use radix_display::frame::{self, Card};
pub use radix_display::frame::{CardSpec, Change, Failed, PlaneFault, PlaneSpec, ScreenSpec, name};
use radix_display::state::PictureShape;
use radix_display::wire;
use radix_hal::UserMode;
use radix_klog::{error, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, Once};
use radix_vfs::synthetic::FixedDir;
use radix_vfs::{Access, In, KArc, Kind, Node, NodeRef, Out, Stat, mem, next_id};

pub use self::report::report;
pub use self::requests::{Call, control};
use crate::Arch;
use crate::console;
use crate::fs::FileObject;
use crate::sched::{self, Options, Policy, wait};
use crate::user::memory::{Lack, SharedPages, unshare_page};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// The layer: its pictures are counted references to a [`Picture`],
/// its cards are [`CardRun`]s, and its lists are the kernel heap's.
type Layer = radix_display::layer::Layer<PictureRef, CardRef, Kmalloc>;

/// The bytes of a page.
const PAGE: usize = PAGE_SIZE as usize;

/// Events a read copies out for each time it takes the lock.
const READ_STEP: usize = 16;

/// Everything the layer keeps.
static LAYER: Mutex<Layer> = Mutex::new(Layer::new_in(kmalloc()));
/// Moved, once the lock has gone, after every change that a holder may
/// wait for. A thread that waits reads it, looks under the lock, and
/// sleeps until it moves.
static PROGRESS: AtomicU64 = AtomicU64::new(0);
/// How many [`Polled`]s the holders' files share.
const POLLED_SLOTS: usize = 16;
/// What polls of the holders' files wait on: each file one of these, by
/// its description's address ([`polled`]). An event for one holder then
/// wakes the polls of its own file, and of the few files that share its
/// slot, which look and sleep again: not every holder's, as when the
/// consoles' service was switched to for each frame a desktop showed
/// (ADR 0815).
static POLLED: [Polled; POLLED_SLOTS] = [const { Polled::new() }; POLLED_SLOTS];
/// `/devices/display`.
static DIR: Once<KArc<FixedDir>> = Once::new();
/// What the input hub was last told of the layer: the holder in front
/// and the one that has the attention file, 0 for none, and whether the
/// secure key has a holder raised. Read and written with the lock held.
static TOLD_FRONT: AtomicU32 = AtomicU32::new(0);
static TOLD_ATTENTION: AtomicU32 = AtomicU32::new(0);
static TOLD_RAISED: AtomicBool = AtomicBool::new(false);

const fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

fn no_memory() -> Status {
    Kernel::NoMemory.status(0)
}

fn invalid() -> Status {
    Kernel::InvalidArgument.status(0)
}

/// The pages a picture's pixels are in: those of a memory file, each
/// held as one of its sharers while the picture lives, so that cutting
/// the file short takes nothing from under a screen.
pub struct Pages {
    /// The frame of each page, the first being the one the picture
    /// starts in.
    frames: Vec<u64, Kmalloc>,
    /// Where the picture starts in the first page.
    start: usize,
}

impl Pages {
    /// Holds the pages of `shared` that `bytes` bytes from `offset` are
    /// in, making those the program has not touched.
    ///
    /// # Errors
    ///
    /// invalid_argument if the file ends before the bytes do, no_room if
    /// its filesystem is full, and no_memory.
    fn hold(shared: &SharedPages, offset: u64, bytes: u64) -> Result<Pages, Status> {
        let last = offset.checked_add(bytes.max(1) - 1).ok_or_else(invalid)?;
        let (first, last) = (offset / PAGE_SIZE, last / PAGE_SIZE);
        let count = usize::try_from(last - first + 1).map_err(|_| invalid())?;
        let mut pages =
            Pages { frames: Vec::new_in(kmalloc()), start: (offset % PAGE_SIZE) as usize };
        pages.frames.try_reserve_exact(count).map_err(|_| no_memory())?;
        for index in first..=last {
            // What was held so far is let go as `pages` drops.
            let frame = shared.make(index).map_err(|lack| match lack {
                Lack::Past => invalid(),
                Lack::NoRoom => Kernel::NoRoom.status(0),
                Lack::NoMemory => no_memory(),
            })?;
            // Room was reserved.
            let _ = pages.frames.push_within_capacity(frame);
        }
        Ok(pages)
    }

    /// Calls `each` with each run of memory the picture's first `bytes`
    /// bytes are in, in order, as its physical address and its length:
    /// what a device is told that reads the picture where it is.
    pub fn runs(&self, bytes: usize, each: &mut dyn FnMut(u64, u32)) {
        // A run's length is told in 32 bits.
        const LONGEST: usize = 1 << 30;
        let (mut left, mut run) = (bytes, None::<(u64, usize)>);
        for (index, &frame) in self.frames.iter().enumerate() {
            let skip = if index == 0 { self.start } else { 0 };
            let len = (PAGE - skip).min(left);
            if len == 0 {
                break;
            }
            let at = frame + skip as u64;
            run = match run {
                Some((from, so_far)) if from + so_far as u64 == at && so_far + len <= LONGEST => {
                    Some((from, so_far + len))
                }
                Some((from, so_far)) => {
                    each(from, so_far as u32);
                    Some((at, len))
                }
                None => Some((at, len)),
            };
            left -= len;
        }
        if let Some((from, len)) = run {
            each(from, len as u32);
        }
    }

    /// Copies `out.len()` bytes of the picture from byte `at` of it. The
    /// program may be drawing in them, as it may in any picture a screen
    /// shows: what is copied is some mix of old and new, never anything
    /// else.
    pub fn read(&self, at: usize, out: &mut [u8]) {
        self.read_by(Arch::read_frames, at, out);
    }

    /// [`Pages::read`] into `screen`, bytes of a framebuffer: memory
    /// mapped write-combining, which takes its bytes once and in order
    /// (ADR 0718).
    pub fn read_to_screen(&self, at: usize, screen: &mut [u8]) {
        self.read_by(Arch::read_frames_in_order, at, screen);
    }

    /// Copies `out.len()` bytes of the picture from byte `at` of it, a
    /// page's part at a time, by `copy`, which is given where the part
    /// is in physical memory.
    fn read_by(&self, copy: impl Fn(u64, &mut [u8]), at: usize, mut out: &mut [u8]) {
        let mut at = self.start + at;
        while !out.is_empty() {
            let within = at % PAGE;
            let step = out.len().min(PAGE - within);
            let (now, rest) = out.split_at_mut(step);
            match self.frames.get(at / PAGE) {
                Some(&frame) => copy(frame + within as u64, now),
                // Past the picture, which no caller asks for.
                None => now.fill(0),
            }
            out = rest;
            at += step;
        }
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        for &frame in &self.frames {
            unshare_page(frame);
        }
    }
}

/// A picture: pixels of a size and a format, for one card.
pub struct Picture {
    pub shape: PictureShape,
    /// The bytes from one row to the next.
    pub row_bytes: u32,
    /// Its pages, or `None` for one the card keeps and PUT writes.
    pub pages: Option<Pages>,
    /// What its card knows it by.
    pub handle: u64,
    card: CardRef,
}

impl frame::Picture for Picture {
    fn shape(&self) -> PictureShape {
        self.shape
    }

    fn row_bytes(&self) -> u32 {
        self.row_bytes
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        self.card.driver.drop_picture(self);
    }
}

/// A picture, shared by its holder and by the plane that shows it.
pub type PictureRef = Arc<Picture, Kmalloc>;

/// What one pipe is to show.
pub type Frame<'a> = frame::Frame<'a, Picture>;

/// A plane's picture in a frame.
pub type Shown<'a> = frame::Shown<'a, Picture>;

/// What the layer asks of a card's driver: what `radix_display`'s
/// [`Card`] says, and the making of pictures, which only the kernel
/// knows the memory of. Called with the layer's lock held, so one call
/// at a time for all cards; a driver may sleep in it, as one that waits
/// for its device to answer does, and bounds how long.
pub trait Driver: Send + Sync {
    /// Whether it can show the kernel's log while no holder is in front.
    fn shows_log(&self) -> bool;
    /// The kernel's log is in front: `frame`, of its first pipe, is the
    /// log's to draw in.
    fn log_takes(&self, frame: &Frame<'_>);
    /// A holder is in front: the log draws no more.
    fn log_gives(&self);
    /// Whether it can show `frame`, as far as only it knows: the layer
    /// has checked the rest.
    fn check(&self, frame: &Frame<'_>) -> Result<(), PlaneFault>;
    /// Makes what it needs for a picture, and returns what it knows the
    /// picture by. `pages` is `None` for a picture it is to keep.
    fn make_picture(
        &self,
        shape: &PictureShape,
        row_bytes: u32,
        pages: Option<&Pages>,
    ) -> Result<u64, Status>;
    /// Lets go of what it made for `picture`, whose pages are let go of
    /// when it returns: its device reads them no more.
    fn drop_picture(&self, picture: &Picture);
    /// Makes a pipe show `frame`, which passed `check`; `change` says
    /// what of it is new.
    fn show(&self, frame: &Frame<'_>, change: &Change<'_>) -> Result<(), Failed>;
    /// Writes `rows` into `rect` of `picture`, which it keeps: each row
    /// is the rectangle's width in pixels of FORMAT_XRGB8888. `on` is
    /// the frame that shows the picture on its main plane, if one does,
    /// whose screens are to show the rows at once.
    fn put(&self, picture: &Picture, rect: Rect, rows: &[u8], on: Option<&Frame<'_>>);
    /// How long before a beat it needs a change to have it ready.
    fn latch_ns(&self) -> u64 {
        0
    }
    /// Does the driver's own work, on the card's thread each time it
    /// wakes and once more as the machine halts ([`draw_log`]): tells
    /// `layer` of screens that came, went or changed
    /// ([`Layer::screen_found`]), and draws what waits of the kernel's
    /// log if the card is sent it. Returns when it is to be called
    /// again at the latest, on the kernel's clock, or `None` if only
    /// when the thread is woken ([`CardRun::wake`]).
    fn look(&self, _layer: &mut Layer) -> Option<u64> {
        None
    }
    /// The machine has stopped with its report on the kernel's console:
    /// a card that is sent the console's pixels sends them now, with no
    /// interrupt and no sleep. Called again when a line of the report
    /// changes.
    fn show_report(&self) {}
}

/// A card as its thread and the layer share it.
pub struct CardRun {
    /// The card's number.
    pub id: u32,
    pub driver: Box<dyn Driver, Kmalloc>,
    /// Set to wake its thread.
    kick: AtomicBool,
    /// When its thread sleeps until: 0 while it is awake, and
    /// [`u64::MAX`] while it waits for no beat.
    sleeping: AtomicU64,
    /// How many times its thread has woken. A still screen adds none.
    wakes: AtomicU64,
}

/// A card, shared.
pub type CardRef = Arc<CardRun, Kmalloc>;

impl CardRun {
    fn kick_key(&self) -> usize {
        core::ptr::from_ref(&self.kick).addr()
    }

    /// Wakes the card's thread, which has its driver look
    /// ([`Driver::look`]): for the driver's interrupt.
    pub fn wake(&self) {
        self.kick.store(true, Ordering::Release);
        wait::wake_all(self.kick_key());
    }
}

impl Card<Picture> for CardRun {
    fn shows_log(&self) -> bool {
        self.driver.shows_log()
    }

    fn log_takes(&self, frame: &Frame<'_>) {
        self.driver.log_takes(frame);
    }

    fn log_gives(&self) {
        self.driver.log_gives();
    }

    fn check(&self, frame: &Frame<'_>) -> Result<(), PlaneFault> {
        self.driver.check(frame)
    }

    fn show(&self, frame: &Frame<'_>, change: &Change<'_>) -> Result<(), Failed> {
        let made = self.driver.show(frame, change);
        if made.is_err() {
            warn!("Display: card {} did not confirm a change of its pipe {}", self.id, frame.pipe);
        }
        made
    }

    fn put(&self, picture: &Picture, rect: Rect, rows: &[u8], on: Option<&Frame<'_>>) {
        self.driver.put(picture, rect, rows, on);
    }

    fn latch_ns(&self) -> u64 {
        self.driver.latch_ns()
    }

    /// Wakes the card's thread if it sleeps past `at_ns`.
    fn wake_for(&self, at_ns: u64) {
        let sleeping = self.sleeping.load(Ordering::Acquire);
        if sleeping == 0 || at_ns < sleeping {
            self.wake();
        }
    }
}

/// Runs `with` on the layer, its lock held, and then wakes whoever waits
/// for what it changed.
fn with_layer<R>(with: impl FnOnce(&mut Layer) -> R) -> R {
    let (result, news) = locked(with);
    if let Some(posted) = news {
        wake_waiters(posted);
    }
    result
}

/// As [`with_layer`], for a card's thread at its beat. The thread waits
/// for its next beat once it has told of this one, so whoever it wakes
/// may run after it on its CPU and no idle one is woken for them
/// ([`sched::lazily_handing_on`]): the desktop, told that its frame
/// shows.
fn with_layer_at_beat<R>(with: impl FnOnce(&mut Layer) -> R) -> R {
    let (result, news) = locked(with);
    if let Some(posted) = news {
        sched::lazily_handing_on(|| wake_waiters(posted));
    }
    result
}

/// Runs `with` on the layer, its lock held. Returns what `with` did and,
/// if anything changed that a holder may wait for, the slots of
/// [`POLLED`] of the holders with a new event to read, a bit each: for
/// [`wake_waiters`], once the lock has gone.
fn locked<R>(with: impl FnOnce(&mut Layer) -> R) -> (R, Option<u32>) {
    let mut layer = LAYER.lock();
    layer.set_time(sched::now_ns());
    let result = with(&mut layer);
    let news = layer.take_news().then(|| {
        let mut posted = 0u32;
        layer.take_posted(|key| posted |= 1 << slot_of(key));
        posted
    });
    tell_input(&layer);
    (result, news)
}

/// Wakes whoever waits for a change of the layer, and the polls of the
/// files in the slots of `posted`.
fn wake_waiters(posted: u32) {
    PROGRESS.fetch_add(1, Ordering::AcqRel);
    wait::wake_all(progress_key());
    for (slot, polled) in POLLED.iter().enumerate() {
        if posted & (1 << slot) != 0 {
            polled.changed();
        }
    }
}

/// The slot of [`POLLED`] for the holder whose description is at `key`:
/// the top bits of the address times the golden ratio's odd number,
/// since descriptions' addresses differ in few bits, and not in the
/// lowest.
fn slot_of(key: usize) -> usize {
    const _: () = assert!(POLLED_SLOTS.is_power_of_two() && POLLED_SLOTS <= u32::BITS as usize);
    let hash = (key as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash >> (u64::BITS - POLLED_SLOTS.ilog2())) as usize
}

/// What polls of the holder's file `file` wait on.
pub fn polled(file: &FileObject) -> &'static Polled {
    &POLLED[slot_of(file.address())]
}

/// Tells the input hub who is in front, who has the attention file and
/// whether the holder the secure key raised is done, if any of it
/// changed (`crate::input`, ADR 0706): with the lock held, so that what
/// a holder's input files read changes in the order the front passed.
/// Three loads and no more while nothing changed.
fn tell_input(layer: &Layer) {
    let front = layer.front().unwrap_or(0);
    let attention = layer.attention();
    let raised = layer.raised().is_some();
    let was_raised = TOLD_RAISED.load(Ordering::Relaxed);
    if TOLD_FRONT.load(Ordering::Relaxed) == front
        && TOLD_ATTENTION.load(Ordering::Relaxed) == attention
        && was_raised == raised
    {
        return;
    }
    TOLD_FRONT.store(front, Ordering::Relaxed);
    TOLD_ATTENTION.store(attention, Ordering::Relaxed);
    TOLD_RAISED.store(raised, Ordering::Relaxed);
    crate::input::screen_changed(front, attention, was_raised && !raised);
}

/// The secure key was pressed, and the input hub lets nobody but the
/// attention holder's readers hear a key from now (ADR 0708): the holder
/// of the attention file is raised over whoever is in front and told. If
/// no holder has the file, the hub's hold ends at once.
pub fn secure_key() {
    if with_layer(Layer::secure_key).is_none() {
        crate::input::release();
    }
}

/// Runs `with` with the holder that the description `file` is, the lock
/// held: the holder does not end and the front does not pass meanwhile.
///
/// # Errors
///
/// invalid_argument if `file` is not a holder's file, and busy for a
/// second description of the attention file.
pub fn with_holder<R>(file: &FileObject, with: impl FnOnce(u32) -> R) -> Result<R, Status> {
    with_layer(|layer| {
        if !file.is_display() {
            return Err(invalid());
        }
        let holder = holder_of(layer, file)?;
        Ok(with(holder))
    })
}

fn progress_key() -> usize {
    core::ptr::from_ref(&PROGRESS).addr()
}

/// Adds a card to the layer, with the thread that keeps its beat.
/// `driver` is given the card's number and makes its driver.
///
/// # Errors
///
/// Why the card was not added, for the log.
pub fn add_card(
    spec: &CardSpec<'_>,
    driver: impl FnOnce(u32) -> Option<Box<dyn Driver, Kmalloc>>,
) -> Result<CardRef, &'static str> {
    let make = |id: u32| {
        let run = CardRun {
            id,
            driver: driver(id)?,
            kick: AtomicBool::new(false),
            sleeping: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
        };
        Arc::try_new_in(run, kmalloc()).ok()
    };
    let card = with_layer(|layer| layer.add_card(spec, make))?;
    let options = Options::new().policy(Policy::Fair { nice: -10 });
    if let Err(err) = sched::spawn("display", beat_thread, card.id as usize, options) {
        // The card shows the log, and changes that ask for now.
        error!("Display: could not start the thread that keeps a card's beat: {err}");
    }
    Ok(card)
}

/// Takes card `id` away from the layer, with its screens, and ends its
/// thread. The kernel's log passes to the next card that can show it.
/// Returns whether there was such a card.
pub fn remove_card(id: u32) -> bool {
    let Some(card) = with_layer(|layer| layer.remove_card(id)) else { return false };
    card.wake();
    true
}

/// How often the thread of the card that shows the kernel's log looks
/// for an error in the log, while the log waits behind the last picture
/// of a holder that ended: the log's sink wakes nobody.
const URGENT_NS: u64 = 20_000_000;

/// A card's thread: at each beat anything waits for, makes the changes
/// that are due and tells the holders that asked, after its driver has
/// looked at what is its own to do. While the kernel's log waits its
/// turn behind the last picture of a holder that ended, the thread of
/// the log's card brings the log to the front as soon as an error is
/// logged, and any card's does when the wait is over. It ends when its
/// card is taken away.
fn beat_thread(card: usize) {
    let card = card as u32;
    let Some(run) = with_layer(|layer| layer.card(card).cloned()) else { return };
    loop {
        run.wakes.fetch_add(1, Ordering::Relaxed);
        run.sleeping.store(0, Ordering::Release);
        run.kick.store(false, Ordering::Release);
        let woken = with_layer_at_beat(|layer| {
            layer.card(card)?;
            let look = run.driver.look(layer);
            layer.log_urgent(console::error_at());
            layer.beat(card);
            let beat = layer.next_beat(card);
            let shows_log = layer.log_card().is_some_and(|log| log.id == card);
            let urgent = (shows_log && layer.keeps_last())
                .then(|| sched::now_ns().saturating_add(URGENT_NS));
            Some(look.into_iter().chain(beat).chain(urgent).min())
        });
        let Some(next) = woken else { return };
        let kicked = || run.kick.load(Ordering::Acquire);
        run.sleeping.store(next.unwrap_or(u64::MAX), Ordering::Release);
        match next {
            Some(at) => {
                wait::wait_until_or_timeout(run.kick_key(), "display", at, kicked);
            }
            None => wait::wait_until(run.kick_key(), "display", kicked),
        }
    }
}

/// The node of `control` or of `attention`.
pub struct DisplayNode {
    id: u64,
    attention: bool,
}

impl Node for DisplayNode {
    // Root's alone: a service is given the file by its service file.
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

    // A description's reads are `read` below; the node's own find
    // nothing, and nothing is written to it.
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

/// Whether `node` is `control` or `attention`.
pub fn is_display(node: &NodeRef) -> bool {
    radix_vfs::downcast::<DisplayNode>(&**node).is_some()
}

/// `/devices/display`, made the first time it is asked for.
pub fn dir() -> radix_vfs::Result<KArc<FixedDir>> {
    if let Some(dir) = DIR.get() {
        return Ok(dir.clone());
    }
    let dir = FixedDir::new(next_id())?;
    let control: NodeRef = mem::arc(DisplayNode { id: next_id(), attention: false })?;
    dir.add("control", control)?;
    let attention: NodeRef = mem::arc(DisplayNode { id: next_id(), attention: true })?;
    dir.add("attention", attention)?;
    Ok(match DIR.set(dir) {
        Ok(set) => set.clone(),
        Err(_) => DIR.get().ok_or(radix_vfs::Error::NoMemory)?.clone(),
    })
}

/// The holder that the description `file` is, made at its first use.
/// `attention` has one holder at a time: a second description of it
/// finds it busy.
fn holder_of(layer: &mut Layer, file: &FileObject) -> Result<u32, Status> {
    let node =
        radix_vfs::downcast::<DisplayNode>(&*file.at.node).ok_or(Kernel::NotTerminal.status(0))?;
    layer.holder_of(file.address(), node.attention)
}

/// Reads the events the holder `file` has not read into `out`, whole
/// ones, as many as fit, waiting for one unless `non_blocking`. Returns
/// the bytes read.
///
/// # Errors
///
/// invalid_argument if `out` cannot hold one event, would_block, and
/// interrupted.
pub fn read(file: &FileObject, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
    let fit = out.len() / DisplayEvent::SIZE;
    if fit == 0 {
        return Err(invalid());
    }
    let mut done = 0;
    loop {
        let seen = PROGRESS.load(Ordering::Acquire);
        let mut events = [DisplayEvent::default(); READ_STEP];
        let step = (fit - done).min(READ_STEP);
        let taken = with_layer(|layer| {
            let me = holder_of(layer, file)?;
            let holder = layer.holder(me).ok_or(Kernel::Gone.status(0))?;
            let mut taken = 0;
            while taken < step
                && let Some(event) = holder.next_event()
            {
                events[taken] = event;
                taken += 1;
            }
            Ok::<usize, Status>(taken)
        })?;
        for event in &events[..taken] {
            let bytes = wire::event_bytes(event);
            out.put(done * DisplayEvent::SIZE, &bytes).map_err(crate::fs::status)?;
            done += 1;
        }
        if done == fit || (done > 0 && taken < step) {
            return Ok(done * DisplayEvent::SIZE);
        }
        if taken > 0 {
            continue;
        }
        if non_blocking {
            return Err(Kernel::WouldBlock.status(0));
        }
        wait::wait_until(progress_key(), "display", || {
            PROGRESS.load(Ordering::Acquire) != seen || sched::interrupted()
        });
        if sched::interrupted() && PROGRESS.load(Ordering::Acquire) == seen {
            return Err(interrupted(Restart::Always));
        }
    }
}

/// What `poll` finds on `file`: readable while an event waits.
pub fn events(file: &FileObject) -> u16 {
    let waits = with_layer(|layer| {
        let me = holder_of(layer, file).ok()?;
        Some(layer.holder(me)?.has_events())
    });
    if waits == Some(true) { poll::READABLE } else { 0 }
}

/// The description at `file` has gone: its holder ends.
pub fn closed(file: &FileObject) {
    with_layer(|layer| {
        if let Some(holder) = layer.holder_by_key(file.address()) {
            layer.end(holder);
            // Its number may be given to a new holder from now.
            crate::input::holder_ended(holder);
        }
    });
}

/// Waits until the change the layer numbered `seq` has shown, or been
/// replaced, or its holder is no longer in front: at most a second.
///
/// # Errors
///
/// timed_out if its card's thread did not come to it.
fn wait_shown(seq: u64) -> Result<(), Status> {
    let deadline = sched::now_ns().saturating_add(SHOWN_WAIT_NS);
    loop {
        let seen = PROGRESS.load(Ordering::Acquire);
        if !with_layer(|layer| layer.waits(seq)) {
            return Ok(());
        }
        let moved = wait::wait_until_or_timeout(progress_key(), "display", deadline, || {
            PROGRESS.load(Ordering::Acquire) != seen
        });
        if !moved {
            warn!("Display: a change waited a second for its beat");
            return Err(Kernel::TimedOut.status(0));
        }
    }
}

/// How long a change that waits to show waits at most.
const SHOWN_WAIT_NS: u64 = 1_000_000_000;

/// Starts the layer: the firmware's framebuffer is its first card, where
/// there is one. `virtio_gpu` is the flag of that name, kept for the
/// virtio cards the scan of the PCI buses finds.
pub fn init(virtio_gpu: Option<&str>) {
    virtio::set_options(virtio_gpu);
    firmware::add();
}

/// The machine has stopped and its report is on the kernel's console
/// (`crate::panic`): the card that shows the log sends it to the screen
/// if that is how its pixels get there. Called again when a line of the
/// report changes. The lock is taken by force if a stopped CPU held it.
pub fn show_report() {
    let layer = LAYER.lock();
    if let Some(card) = layer.log_card() {
        card.driver.show_report();
    }
}

/// Sends the screens what waits of the kernel's log, where a card is
/// sent the log's pixels and its thread sends them: for the last line
/// before the machine halts, after which no thread runs.
pub fn draw_log() {
    with_layer(|layer| {
        if let Some(card) = layer.log_card().cloned() {
            let _ = card.driver.look(layer);
        }
    });
}

/// The machine ends: the screens are the log's again, whatever holder
/// was in front, so the last lines show.
pub fn shutdown() {
    with_layer(Layer::close);
}
