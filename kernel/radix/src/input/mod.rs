// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Input devices (release 1.1, ADR 0706): `/devices/input`, and the
//! merged files `/devices/kbd` and `/devices/mouse`.
//!
//! A driver adds each device it finds ([`add`]) and from then on hands
//! over what each report of it said, with the time of the interrupt that
//! brought the report ([`frame`]). The rest is done for every device
//! alike by `radix_input::hub`, where all of it is tested on the host:
//! which reader reads what, input that follows the screen, the secure
//! key (ADR 0708), and the repeats of a held key. What is here is what
//! only the kernel has: the lock, the files and their requests
//! (`requests`, radix-input(2)), what wakes a reader, and the thread that
//! keeps a held key repeating (`keyboard`). The drivers are with their
//! buses (PS/2 in `keyboard`, `crate::usb`, `crate::i2c`,
//! `crate::hyperv`), but for virtio's devices, which are only input
//! devices and are in [`virtio`].
//!
//! Every description of an input file is a reader with a ring of its
//! own, made when the file is opened. The hub wakes a reader through its
//! [`Reader`]: a count that moves, the threads that wait in a read of
//! the description, and its pollers. A report of one device wakes only
//! the readers that read it.
//!
//! One lock keeps the hub. Drivers hand their reports over on a thread,
//! never in an interrupt handler: waking a reader takes the wait table's
//! lock, which interrupt handlers must not take. The interrupt notes the
//! time, and the report carries it. A read copies its records out after
//! it has let go of the lock, so the lock is never held across a fault.
//!
//! The display layer tells the hub who is in front and who has the
//! attention file ([`screen_changed`]), with its own lock held: what a
//! holder reads changes in the order the front passed. The hub's lock is
//! always the inner one of the two. The secure key begins its hold in the
//! hub, in the call that brought the key, and the display layer is asked
//! to bring the attention holder to the front once the hub's lock has
//! gone ([`frame`]).

mod keyboard;
mod requests;
pub mod virtio;

use alloc::sync::Arc;
use core::any::Any;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::input::{EV_KEY, KEY_ESC};
pub use radix_input::device::{About, Axis, Few};
pub use radix_input::event::{FRAME_MAX, Said};
use radix_input::hub::{Ready, Refused, Source, Wake};
use radix_klog::{TextBuf, warn};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Mutex, Once};
use radix_vfs::synthetic::FixedDir;
use radix_vfs::{Access, In, KArc, Kind, Node, NodeRef, Out, Stat, Text, mem, next_id};

pub use self::keyboard::{lights, set1_byte, start};
pub use self::requests::{Call, control};
use crate::fs::FileObject;
use crate::sched::{self, wait::WaitCount};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// The hub: its readers are woken through a [`Waker`], and its lists are
/// the kernel heap's.
type Hub = radix_input::hub::Hub<Waker, Kmalloc>;

/// The bytes of records a read copies out for each time it takes the
/// lock: whole records of every file's kind.
const READ_STEP: usize = 480;

/// Everything the hub keeps.
static HUB: Mutex<Hub> = Mutex::new(Hub::new_in(kmalloc()));
/// `/devices/input`.
static DIR: Once<KArc<FixedDir>> = Once::new();

const fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// What the kernel keeps for one description of an input file, shared
/// with the hub, which wakes the description through it.
pub struct Reader {
    /// Moved each time the hub gives the reader something to read or its
    /// file changes. A thread that waits reads it, looks under the lock,
    /// and sleeps until it moves.
    news: AtomicU32,
    /// The threads that wait in a read of the description.
    waiters: WaitCount,
    /// Its pollers.
    polled: Polled,
}

impl Reader {
    /// What its waiters wait on.
    fn key(&self) -> usize {
        core::ptr::from_ref(&self.news).addr()
    }
}

/// A reader as the hub holds it.
struct Waker(Arc<Reader, Kmalloc>);

/// Whether the hub is being handed a report by a thread that waits for
/// its device's next interrupt once it has ([`frame_then_waits`]).
/// Written and read with the hub's lock held.
static WAITS_NEXT: AtomicBool = AtomicBool::new(false);

impl Wake for Waker {
    // Called with the hub's lock held, which the woken thread wants
    // next: it is given this CPU only once the lock has gone
    // ([`with_hub`]).
    fn wake(&self) {
        let reader = &*self.0;
        reader.news.fetch_add(1, Ordering::SeqCst);
        let wake = || {
            // Takes the barrier the pollers' look needs too.
            reader.waiters.wake_all(reader.key());
            reader.polled.changed_by_rmw();
        };
        // What a thread that waits next wakes may run after it on its
        // CPU, which the device's interrupt has awake already.
        if WAITS_NEXT.load(Ordering::Relaxed) {
            sched::lazily_handing_on(wake);
        } else {
            sched::lazily(wake);
        }
    }
}

/// Runs `with` on the hub, its lock held, and then gives way to a reader
/// it woke.
fn with_hub<R>(with: impl FnOnce(&mut Hub) -> R) -> R {
    let result = with(&mut HUB.lock());
    sched::pay_owed();
    result
}

/// The status of what the hub refused.
fn refused(refused: Refused) -> Status {
    match refused {
        // A device that has gone, and records _/devices/kbd_ or
        // _/devices/mouse_ lost: each file has only one of the two.
        Refused::NoDevice | Refused::Lost => Kernel::Gone,
        Refused::NoRoom => Kernel::NoMemory,
        Refused::Tied => Kernel::Busy,
        Refused::NoReader | Refused::NotThisFile => Kernel::NotTerminal,
        Refused::TooSmall => Kernel::InvalidArgument,
    }
    .status(0)
}

/// The node of an input file: a device's own, `control`, or one of the
/// merged files.
pub struct InputNode {
    id: u64,
    source: Source,
}

impl Node for InputNode {
    // Root's alone, every one of them: what is typed is for the program
    // the person types to. inputd(8) hands a device's file out, to a
    // screen's holder only (ADR 0707), and a service is given a merged
    // file by its service file.
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

/// Whether `node` is an input file.
pub fn is_input(node: &NodeRef) -> bool {
    radix_vfs::downcast::<InputNode>(&**node).is_some()
}

/// The file `file` is a description of.
fn source_of(file: &FileObject) -> Option<Source> {
    radix_vfs::downcast::<InputNode>(&*file.at.node).map(|node| node.source)
}

/// `/devices/input`, made the first time it is asked for: a driver may
/// find its device before the tree of `/devices` is made.
pub fn dir() -> radix_vfs::Result<KArc<FixedDir>> {
    if let Some(dir) = DIR.get() {
        return Ok(dir.clone());
    }
    let dir = FixedDir::new(next_id())?;
    dir.add("control", node(Source::Control)?)?;
    Ok(match DIR.set(dir) {
        Ok(set) => set.clone(),
        Err(_) => DIR.get().ok_or(radix_vfs::Error::NoMemory)?.clone(),
    })
}

fn node(source: Source) -> radix_vfs::Result<NodeRef> {
    Ok(mem::arc(InputNode { id: next_id(), source })?)
}

/// The node of `/devices/kbd`.
pub fn keyboard_node() -> radix_vfs::Result<NodeRef> {
    node(Source::Keyboard)
}

/// The node of `/devices/mouse`.
pub fn mouse_node() -> radix_vfs::Result<NodeRef> {
    node(Source::Mouse)
}

/// The name of the device `number`'s file in `/devices/input`: its
/// number in digits, of which a `u32` has ten at most.
fn file_name(number: u32) -> TextBuf<10> {
    let mut name = TextBuf::new();
    let _ = write!(name, "{number}");
    name
}

/// Adds a device that says `about` of itself: from its driver's thread.
/// Returns its number, the N of `/devices/input/N`, which the driver
/// hands its reports over with; `None`, with a line in the log, if there
/// is no room for it.
pub fn add(about: &About) -> Option<u32> {
    let now = sched::now_ns();
    let added = with_hub(|hub| {
        let number = hub.add(about, now).map_err(|_| "there are too many, or no memory")?;
        // With the lock held: a reader of the list that is told of the
        // device finds its file.
        let filed = dir()
            .and_then(|dir| dir.add(file_name(number).as_str(), node(Source::Device(number))?));
        if filed.is_err() {
            hub.remove(number, now);
            return Err("no memory for its file");
        }
        Ok(number)
    });
    match added {
        Ok(number) => Some(number),
        Err(why) => {
            let name = radix_input::device::text(&about.name);
            warn!("Input: {name} is not read: {why}");
            None
        }
    }
}

/// The device `number` has gone: what it held down goes up, its file
/// goes, and its readers find it gone once they have read what waits.
pub fn remove(number: u32) {
    let now = sched::now_ns();
    with_hub(|hub| {
        if let Some(dir) = DIR.get() {
            dir.remove_entry(file_name(number).as_str());
        }
        hub.remove(number, now);
        keyboard::repeat_is(hub.repeat_at());
    });
}

/// What a report of the device `number` said, which came at `time_ns`,
/// the time of the interrupt that brought it: from the driver's thread.
/// Everything one report said is handed over in one call, so that a
/// reader reads it as one frame.
pub fn frame(number: u32, time_ns: u64, said: &[Said]) {
    hand_over(number, time_ns, said, false);
}

/// As [`frame`], from a thread that has nothing to do but wait for its
/// device's next interrupt once its frames are handed over. A reader the
/// frame wakes, whether it waits in a read or on an event queue, then
/// runs after that thread on its CPU if nothing else waits there, and no
/// idle CPU is interrupted and brought out of its sleep for it
/// (`sched::lazily_handing_on`).
pub fn frame_then_waits(number: u32, time_ns: u64, said: &[Said]) {
    hand_over(number, time_ns, said, true);
}

/// [`frame`], or with `waits` [`frame_then_waits`].
fn hand_over(number: u32, time_ns: u64, said: &[Said], waits: bool) {
    let (secure, sooner) = with_hub(|hub| {
        WAITS_NEXT.store(waits, Ordering::Relaxed);
        let secure = hub.frame(number, time_ns, said);
        WAITS_NEXT.store(false, Ordering::Relaxed);
        (secure, keyboard::repeat_is(hub.repeat_at()))
    });
    if secure {
        // The hold is on already: nobody else reads another key. The
        // attention holder comes to the front, or there is none after
        // all and the hold ends.
        crate::display::secure_key();
    }
    let escape = |said: &Said| said.kind == EV_KEY && said.code == KEY_ESC && said.value != 0;
    if sooner || said.iter().any(escape) {
        keyboard::wake(said.iter().any(escape));
    }
}

/// Repeats the held key if it is due at `now`: for the keyboards'
/// thread.
fn tick(now: u64) {
    with_hub(|hub| {
        let next = hub.tick(now);
        keyboard::repeat_is(next);
    });
}

/// The display layer's front, attention file or raised holder changed:
/// `front` is the holder in front and `attention` the one that has the
/// attention file, 0 for none, and `lowered` says the holder the secure
/// key raised is done. Called with the display layer's lock held.
pub fn screen_changed(front: u32, attention: u32, lowered: bool) {
    let now = sched::now_ns();
    with_hub(|hub| {
        hub.attention_is(attention, now);
        hub.front_is(front, now);
        if lowered {
            hub.release(now);
        }
    });
}

/// The secure key's hold ends with no holder raised: there was no
/// attention holder after all.
pub fn release() {
    let now = sched::now_ns();
    with_hub(|hub| hub.release(now));
}

/// The display holder `holder` has ended: the readers tied to it hear
/// nothing more. Called with the display layer's lock held.
pub fn holder_ended(holder: u32) {
    with_hub(|hub| hub.forget(holder));
}

/// Makes the reader that the description `file` is.
fn open(hub: &mut Hub, file: &FileObject) -> Result<(), Status> {
    let source = source_of(file).ok_or(Kernel::NotTerminal.status(0))?;
    let reader =
        Reader { news: AtomicU32::new(0), waiters: WaitCount::new(), polled: Polled::new() };
    let reader = Arc::try_new_in(reader, kmalloc()).map_err(|_| Kernel::NoMemory.status(0))?;
    hub.open(file.address(), source, Waker(reader)).map_err(refused)
}

/// A description of an input file was made: it is a reader from now. If
/// there is no room for one, its first use tries again and says so.
pub fn opened(file: &FileObject) {
    let _ = with_hub(|hub| open(hub, file));
}

/// The description at `file` has gone.
pub fn closed(file: &FileObject) {
    with_hub(|hub| hub.close(file.address()));
}

/// Runs `with` on the hub with the key of the reader that `file` is,
/// which is made first if its making failed when the file was opened.
///
/// # Errors
///
/// no_memory if the reader cannot be made; what `with` was refused is
/// the caller's to turn into a status.
fn with_reader<R>(
    file: &FileObject,
    mut with: impl FnMut(&mut Hub, usize) -> Result<R, Refused>,
) -> Result<Result<R, Refused>, Status> {
    let key = file.address();
    with_hub(|hub| match with(hub, key) {
        Err(Refused::NoReader) => {
            open(hub, file)?;
            Ok(with(hub, key))
        }
        other => Ok(other),
    })
}

/// Reads the records the reader `file` has not read into `out`, whole
/// ones, as many as fit, waiting for one unless `non_blocking`. Returns
/// the bytes read.
///
/// # Errors
///
/// invalid_argument if `out` cannot hold one record; gone, once, if
/// `/devices/kbd` or `/devices/mouse` lost records, and for a device's
/// file once the device has gone and nothing waits; would_block, and
/// interrupted.
pub fn read(file: &FileObject, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
    let mut done = 0;
    loop {
        let mut step = [0u8; READ_STEP];
        let room = (out.len() - done).min(READ_STEP);
        let read = with_reader(file, |hub, key| {
            // A loss that only an error tells of is this read's to tell
            // while it has nothing, and the next read's once it has.
            let out = &mut step[..room];
            let taken = if done == 0 { hub.read(key, out)? } else { hub.read_on(key, out)? };
            // Read with the lock held: a wake after this moves it.
            let seen = |waker: &Waker| (waker.0.clone(), waker.0.news.load(Ordering::SeqCst));
            Ok((taken, hub.waker(key).filter(|_| taken == 0).map(seen)))
        })?;
        let (taken, waits) = match read {
            // No whole record fits what is left of `out`.
            Err(Refused::TooSmall) if done > 0 => return Ok(done),
            read => read.map_err(refused)?,
        };
        out.put(done, &step[..taken]).map_err(crate::fs::status)?;
        done += taken;
        if taken > 0 {
            continue;
        }
        let Some((reader, seen)) = waits.filter(|_| done == 0) else { return Ok(done) };
        if non_blocking {
            return Err(Kernel::WouldBlock.status(0));
        }
        let news = || reader.news.load(Ordering::SeqCst) != seen;
        reader.waiters.wait_until(reader.key(), "input", || news() || sched::interrupted());
        if sched::interrupted() && !news() {
            return Err(interrupted(Restart::Always));
        }
    }
}

/// What `poll` finds on `file`: readable while a record waits, and hung
/// up once its device has gone and nothing waits.
pub fn events(file: &FileObject) -> u16 {
    match HUB.lock().ready(file.address()) {
        Ready::Readable => poll::READABLE,
        Ready::Gone => poll::HANG_UP,
        Ready::Nothing => 0,
    }
}

/// Calls `each` with what the reader `file` keeps for its pollers.
pub fn polled(file: &FileObject, each: &mut dyn FnMut(&Polled)) {
    let reader = HUB.lock().waker(file.address()).map(|waker| waker.0.clone());
    if let Some(reader) = reader {
        each(&reader.polled);
    }
}

/// `/kernel/class/input/state`: the hub in words
/// (`radix_input::report`), of one moment.
///
/// # Errors
///
/// `text`'s, when it is full.
pub fn report(text: &mut Text) -> fmt::Result {
    let hub = HUB.lock();
    let out: &mut dyn Write = text;
    hub.describe(out)
}
