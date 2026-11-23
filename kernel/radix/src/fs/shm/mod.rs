// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `shm`: the memory filesystem on `/devices/shm` (ADR 0490), where
//! POSIX's `shm_open` and `sem_open` keep their files, and where System V
//! IPC's segments and semaphore sets live.
//!
//! A tmpfs file's pages are a page cache's, which no program maps: a
//! shared mapping of one cannot be made. A memory file's pages are a
//! [`SharedPages`] instead, the pages of a shared anonymous mapping, so
//! `mmap(MAP_SHARED)` of it maps the very pages every other process maps,
//! and `read` and `write` reach them by their frames. A page is made the
//! first time a program touches it, or `write` writes it, and reads of
//! one not made yet are zeros. The file's size is where its pages end: a
//! fault past it is SIGBUS, and cutting it short lets go of the pages
//! past the end and unmaps them from every space that maps them.
//!
//! One directory holds the files, flat, as `shm_open`'s names are: no
//! directory, link or device is made in it. Its root has mode 1777, as
//! Linux's `/dev/shm`, so that anyone makes files and only their owners
//! remove them. Its files' and segments' pages together may fill half of
//! memory, as a tmpfs may ([`Room`]), and it may hold a file for every
//! two pages of memory.
//!
//! Memory files (`memfd`, radix-process(2)'s `memory_file`) are files of a
//! second filesystem of this kind, which no directory of any tree holds
//! ([`memory_file`]): a program with no `/devices/shm` in its sandbox has
//! them all the same. A memory file has no name in a directory, goes when
//! its last description and mapping go, and may be sealed ([`seal`]): its
//! [`SharedPages`] keep the seals, so that the mapping paths see them.
//!
//! The filesystem keeps System V IPC's objects too ([`ipc`]): a C library
//! names them through the directory capability of `/devices/shm`, so a
//! sandbox that has no `/devices/shm` has no System V IPC, and the
//! permissions of each object are checked against the name of the
//! capability it comes through (ADR 0214).

pub mod ipc;

use core::any::Any;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use radix_abi::Status;
use radix_abi::address_space::PAGE_SIZE;
use radix_abi::errors::Kernel;
use radix_abi::file::Seals;
use radix_abi::process::MemoryFileFlags;
use radix_cap::{ObjectKind, Rights, SlotRef};
use radix_hal::UserMode;
use radix_mm::FRAMES;
use radix_sync::{Mutex, Once};
use radix_vfs::mount::{Location, MountRef};
use radix_vfs::path::check_name;
use radix_vfs::{
    APPEND, Access, AccessChange, Error, FileSystem, FsInfo, FsKind, In, KArc, KVec, Kind, ListFn,
    Node, NodeRef, Out, Result, Stat, mem, now,
};

pub use ipc::{Attachment, SegmentRef};

use super::{FileObject, Opened};
use crate::Arch;
use crate::user::memory::{
    Lack, Room, RoomRef, SharedPages, SharedRef, has_writable_mapping, unmap_past, unshare_page,
};

/// The mode of the root: everyone makes files, and only a file's owner
/// removes it.
const ROOT_MODE: u16 = 0o1777;

/// The largest a file may be, as Linux's MAX_LFS_FILESIZE.
const MAX_SIZE: u64 = i64::MAX as u64;

/// The bytes a read or write copies at a time, through the stack.
const STEP: usize = 256;

/// The longest name `memory_file` takes, as Linux's: its NAME_MAX of 255
/// less the `memfd:` its path starts with.
const NAME_MAX: usize = 249;

/// The seals that forbid writing a file.
const WRITE_SEALS: Seals = Seals::WRITE.union(Seals::FUTURE_WRITE);

/// What every node of one memory filesystem shares.
struct Shared {
    /// The pages its files and segments may hold together.
    room: RoomRef,
    /// The most files it may hold, and the names of files past their
    /// first.
    max_files: u64,
    files: AtomicU64,
    next_id: AtomicU64,
    /// Its System V objects.
    ipc: ipc::IpcRef,
}

impl Shared {
    /// Counts one more file, or name, against the limit.
    fn count_file(&self) -> Result<()> {
        self.files
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |files| {
                (files < self.max_files).then_some(files + 1)
            })
            .map(drop)
            .map_err(|_| Error::NoSpace)
    }
}

/// A memory filesystem.
pub struct ShmFs {
    root: KArc<ShmDir>,
}

impl ShmFs {
    /// An empty memory filesystem whose files and segments may hold
    /// `max_pages` pages together, and which may hold `max_files` files.
    pub fn new(max_pages: u64, max_files: u64) -> Result<KArc<ShmFs>> {
        let room = Room::new(max_pages).map_err(|_| Error::NoMemory)?;
        let ipc = ipc::Ipc::new(room.clone()).map_err(|_| Error::NoMemory)?;
        let shared = mem::arc(Shared {
            room,
            max_files,
            files: AtomicU64::new(0),
            next_id: AtomicU64::new(2),
            ipc,
        })?;
        let created = now();
        let root = mem::arc(ShmDir {
            id: 1,
            created,
            accessed: AtomicU64::new(created),
            changed: AtomicU64::new(created),
            access: Mutex::new(Access { owner: 0, group: 0, mode: ROOT_MODE }),
            entries: Mutex::new(Entries::new(created)),
            shared,
        })?;
        Ok(mem::arc(ShmFs { root })?)
    }
}

impl FileSystem for ShmFs {
    fn kind(&self) -> FsKind {
        FsKind::Shm
    }

    fn root(&self) -> NodeRef {
        self.root.clone()
    }

    fn info(&self) -> FsInfo {
        let shared = &self.root.shared;
        FsInfo {
            block_size: PAGE_SIZE as u32,
            blocks: shared.room.max(),
            free: shared.room.max().saturating_sub(shared.room.used()),
            files: shared.max_files,
            free_files: shared.max_files.saturating_sub(shared.files.load(Ordering::Relaxed)),
        }
    }

    // A file's last reference frees its pages, which takes only the frame
    // allocator's lock.
    fn light_drops(&self) -> bool {
        true
    }
}

/// The System V objects of the memory filesystem whose root is `node`;
/// `None` if `node` is no memory filesystem's root.
pub fn ipc_of(node: &NodeRef) -> Option<ipc::IpcRef> {
    radix_vfs::downcast::<ShmDir>(&**node).map(|dir| dir.shared.ipc.clone())
}

/// The shared pages of `node`, if it is a memory file: what a shared
/// mapping of it maps.
pub fn pages_of(node: &NodeRef) -> Option<SharedRef> {
    radix_vfs::downcast::<ShmFile>(&**node).map(|file| file.pages.clone())
}

/// A name in the directory, and its file.
struct Entry {
    /// When it was made, in the directory: a listing goes in this order.
    seq: u64,
    name: KVec<u8>,
    file: KArc<ShmFile>,
}

/// The directory's names: in the order they were made, which listings
/// follow (so that a name removed while a program lists the directory
/// makes it skip no other), with an index sorted by name.
struct Entries {
    list: KVec<Entry>,
    /// Indexes into `list`, sorted by the entries' names.
    by_name: KVec<u32>,
    next_seq: u64,
    modified: u64,
}

impl Entries {
    fn new(modified: u64) -> Entries {
        Entries { list: mem::vec(), by_name: mem::vec(), next_seq: 1, modified }
    }

    /// Where `name` is in `by_name`, or would go.
    fn find(&self, name: &str) -> core::result::Result<usize, usize> {
        self.by_name
            .binary_search_by(|&i| self.list[i as usize].name.as_slice().cmp(name.as_bytes()))
    }

    fn at(&self, at: usize) -> &Entry {
        &self.list[self.by_name[at] as usize]
    }

    /// Adds `file` as `name`, which [`Entries::find`] said goes at `at`.
    fn insert(&mut self, at: usize, name: &str, file: KArc<ShmFile>) -> Result<()> {
        let index = u32::try_from(self.list.len()).map_err(|_| Error::NoSpace)?;
        let name = mem::copy(name.as_bytes())?;
        // Room in both first, so that a failure changes nothing.
        self.by_name.try_reserve(1).map_err(|_| Error::NoMemory)?;
        mem::push(&mut self.list, Entry { seq: self.next_seq, name, file })?;
        self.next_seq += 1;
        mem::insert(&mut self.by_name, at, index)?;
        Ok(())
    }

    /// Takes out the entry at `at` in `by_name`.
    fn remove(&mut self, at: usize) -> Entry {
        let index = self.by_name.remove(at);
        for i in &mut self.by_name {
            if *i > index {
                *i -= 1;
            }
        }
        self.list.remove(index as usize)
    }
}

/// The directory of a memory filesystem: its root, and its only one.
pub struct ShmDir {
    id: u64,
    created: u64,
    accessed: AtomicU64,
    changed: AtomicU64,
    access: Mutex<Access>,
    entries: Mutex<Entries>,
    shared: KArc<Shared>,
}

impl ShmDir {
    /// Adds the name `name` for `file`: a new file, or with `again` one
    /// more name of one, which counts against the limit as a file does.
    fn add(&self, name: &str, file: KArc<ShmFile>, again: bool) -> Result<NodeRef> {
        check_name(name)?;
        let mut entries = self.entries.lock();
        let Err(at) = entries.find(name) else {
            return Err(Error::Exists);
        };
        if again {
            // A file whose names are all gone stays gone, as on Linux.
            if file.links.load(Ordering::Relaxed) == 0 {
                return Err(Error::NotFound);
            }
            self.shared.count_file()?;
        }
        if let Err(err) = entries.insert(at, name, file.clone()) {
            if again {
                self.shared.files.fetch_sub(1, Ordering::Relaxed);
            }
            return Err(err);
        }
        let time = now();
        entries.modified = time;
        file.links.fetch_add(1, Ordering::Relaxed);
        file.changed.store(time, Ordering::Relaxed);
        Ok(file)
    }
}

impl Node for ShmDir {
    fn stat(&self) -> Stat {
        let modified = self.entries.lock().modified;
        Stat {
            kind: Kind::Directory,
            size: 0,
            allocated: 0,
            id: self.id,
            created: self.created,
            modified,
            accessed: self.accessed.load(Ordering::Relaxed),
            changed: self.changed.load(Ordering::Relaxed).max(modified),
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::Directory
    }

    fn lookup(&self, name: &str) -> Result<NodeRef> {
        let entries = self.entries.lock();
        let at = entries.find(name).map_err(|_| Error::NotFound)?;
        Ok(entries.at(at).file.clone())
    }

    fn access(&self) -> Access {
        *self.access.lock()
    }

    fn set_access(&self, change: &AccessChange) -> Result<()> {
        let mut access = self.access.lock();
        *access = change.apply(*access);
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    // Only files: `shm_open`'s names have no directories.
    fn create(&self, name: &str, kind: Kind, access: Access) -> Result<NodeRef> {
        if kind != Kind::File {
            return Err(Error::NotSupported);
        }
        check_name(name)?;
        self.shared.count_file()?;
        let file = match ShmFile::new(&self.shared, access, false) {
            Ok(file) => file,
            Err(err) => {
                self.shared.files.fetch_sub(1, Ordering::Relaxed);
                return Err(err);
            }
        };
        // A file that could not be named goes, uncounting itself.
        self.add(name, file, false)
    }

    // A second name: `sem_open` publishes a semaphore's file with `link`.
    fn link(&self, name: &str, node: &NodeRef) -> Result<()> {
        let file = radix_vfs::downcast::<ShmFile>(&**node).ok_or(Error::CrossMount)?;
        if !KArc::ptr_eq(&file.shared, &self.shared) {
            return Err(Error::CrossMount);
        }
        let file = file.me.get().and_then(alloc::sync::Weak::upgrade).ok_or(Error::NotFound)?;
        self.add(name, file, true).map(drop)
    }

    fn remove(&self, name: &str) -> Result<()> {
        check_name(name)?;
        let mut entries = self.entries.lock();
        let at = entries.find(name).map_err(|_| Error::NotFound)?;
        let entry = entries.remove(at);
        let time = now();
        entries.modified = time;
        drop(entries);
        entry.file.unnamed(time);
        Ok(())
    }

    fn rename(&self, from: &str, to_dir: &dyn Node, to: &str) -> Result<()> {
        check_name(from)?;
        check_name(to)?;
        let target = radix_vfs::downcast::<ShmDir>(to_dir).ok_or(Error::CrossMount)?;
        if !core::ptr::eq(self, target) {
            return Err(Error::CrossMount);
        }
        let mut entries = self.entries.lock();
        let at = entries.find(from).map_err(|_| Error::NotFound)?;
        if from == to {
            return Ok(());
        }
        let moving = entries.at(at).file.clone();
        let time = now();
        let replaced = match entries.find(to) {
            Ok(old) => {
                // Two names of one file: nothing happens, as POSIX has it.
                if KArc::ptr_eq(&entries.at(old).file, &moving) {
                    return Ok(());
                }
                moving.changed.store(time, Ordering::Relaxed);
                let index = entries.by_name[old] as usize;
                let old_file = core::mem::replace(&mut entries.list[index].file, moving);
                let at = entries.find(from).map_err(|_| Error::NotFound)?;
                entries.remove(at);
                Some(old_file)
            }
            Err(to_at) => {
                // The entry keeps its place in listings; its name and its
                // place in the index change. The index loses one entry and
                // gains one, so it has room.
                let mut name = mem::copy(to.as_bytes())?;
                let index = entries.by_name.remove(at);
                let to_at = if to_at > at { to_at - 1 } else { to_at };
                mem::insert(&mut entries.by_name, to_at, index)?;
                core::mem::swap(&mut entries.list[index as usize].name, &mut name);
                moving.changed.store(time, Ordering::Relaxed);
                None
            }
        };
        entries.modified = time;
        drop(entries);
        if let Some(old) = replaced {
            old.unnamed(time);
        }
        Ok(())
    }

    fn list(&self, cursor: u64, f: ListFn<'_>) -> Result<()> {
        let entries = self.entries.lock();
        let from = entries.list.partition_point(|entry| entry.seq < cursor);
        for entry in &entries.list[from..] {
            let name = core::str::from_utf8(&entry.name).unwrap_or("");
            if !f(entry.seq + 1, name, Kind::File) {
                break;
            }
        }
        Ok(())
    }

    fn set_times(&self, accessed: Option<u64>, modified: Option<u64>) -> Result<()> {
        if let Some(accessed) = accessed {
            self.accessed.store(accessed, Ordering::Relaxed);
        }
        if let Some(modified) = modified {
            self.entries.lock().modified = modified;
        }
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A memory file.
pub struct ShmFile {
    id: u64,
    created: u64,
    accessed: AtomicU64,
    modified: AtomicU64,
    changed: AtomicU64,
    /// How many names it has.
    links: AtomicU32,
    access: Mutex<Access>,
    /// Its size in bytes, which its pages end at.
    size: AtomicU64,
    /// Held while its size changes, and while it is written, so that a
    /// write past the end and a cut do not cross.
    resize: Mutex<()>,
    pages: SharedRef,
    shared: KArc<Shared>,
    /// Whether it is a memory file, the only kind that takes seals.
    memfd: bool,
    /// Itself, so that `link` can name it again.
    me: radix_sync::Once<alloc::sync::Weak<ShmFile, radix_vfs::mem::Alloc>>,
}

impl ShmFile {
    fn new(shared: &KArc<Shared>, access: Access, memfd: bool) -> Result<KArc<ShmFile>> {
        let pages = SharedPages::in_room(shared.room.clone(), 0).map_err(|_| Error::NoMemory)?;
        let created = now();
        let file = mem::arc(ShmFile {
            id: shared.next_id.fetch_add(1, Ordering::Relaxed),
            created,
            accessed: AtomicU64::new(created),
            modified: AtomicU64::new(created),
            changed: AtomicU64::new(created),
            links: AtomicU32::new(0),
            access: Mutex::new(access),
            size: AtomicU64::new(0),
            resize: Mutex::new(()),
            pages,
            shared: shared.clone(),
            memfd,
            me: radix_sync::Once::new(),
        })?;
        let _ = file.me.set(KArc::downgrade(&file));
        Ok(file)
    }

    /// It has lost a name at `time`. A name past its first gives back what
    /// it counted.
    fn unnamed(&self, time: u64) {
        if self.links.fetch_sub(1, Ordering::Relaxed) > 1 {
            self.shared.files.fetch_sub(1, Ordering::Relaxed);
        }
        self.changed.store(time, Ordering::Relaxed);
    }

    /// Its contents changed at `time`.
    fn touched(&self, time: u64) {
        self.modified.store(time, Ordering::Relaxed);
        self.changed.store(time, Ordering::Relaxed);
    }

    /// Where a write from `offset` to `end` of a file of `size` bytes may
    /// end, its seals taken into account. Linux checks each page it
    /// writes: a file sealed with write or future_write takes no write,
    /// and one sealed with grow takes the pages that end inside it, then
    /// fails the one that would pass its end.
    ///
    /// # Errors
    ///
    /// [`Error::NotPermitted`] if the seals let it write nothing.
    fn writable_end(&self, offset: u64, end: u64, size: u64) -> Result<u64> {
        let seals = self.pages.seals();
        if seals.intersection(WRITE_SEALS) != Seals::NONE {
            return Err(Error::NotPermitted);
        }
        if !seals.contains(Seals::GROW) || end <= size {
            return Ok(end);
        }
        let whole_pages = size / PAGE_SIZE * PAGE_SIZE;
        if whole_pages <= offset { Err(Error::NotPermitted) } else { Ok(whole_pages) }
    }

    /// Adds the seals `add` and gives the file's seals; the contract of
    /// [`seal`].
    fn add_seals(&self, add: Seals) -> core::result::Result<Seals, Status> {
        // Held so that no write, resize or other seal runs meanwhile. The
        // lock order is this one, then the spaces' list and each space's
        // regions (in `has_writable_mapping`), as in `set_size`.
        let _resize = self.resize.lock();
        let seals = self.pages.seals();
        if add == Seals::NONE {
            return Ok(seals);
        }
        if seals.contains(Seals::SEAL) {
            return Err(Kernel::NotPermitted.status(0));
        }
        if add.contains(Seals::WRITE) && !seals.contains(Seals::WRITE) {
            // The seal is set before the look for mappings, and taken
            // back if one is found. A mapping made meanwhile reads the
            // seal with its space's regions locked and is refused or made
            // read-only, so none is made between the look and the seal; one
            // made before is in its space's regions when the look locks
            // them. A mapping refused for a seal that is taken back is
            // refused all the same, as when two programs race for it.
            self.pages.set_seals(seals.union(Seals::WRITE));
            if has_writable_mapping(&self.pages) {
                self.pages.set_seals(seals);
                return Err(Kernel::Busy.status(0));
            }
        }
        let seals = seals.union(add);
        self.pages.set_seals(seals);
        Ok(seals)
    }
}

impl Drop for ShmFile {
    fn drop(&mut self) {
        // Its pages go with the last of its mappings, uncharged then.
        self.shared.files.fetch_sub(1, Ordering::Relaxed);
    }
}

/// How many pages hold `size` bytes.
fn pages_for(size: u64) -> u64 {
    size.div_ceil(PAGE_SIZE)
}

impl Node for ShmFile {
    fn stat(&self) -> Stat {
        let modified = self.modified.load(Ordering::Relaxed);
        Stat {
            kind: Kind::File,
            size: self.size.load(Ordering::Acquire),
            allocated: self.pages.made() * PAGE_SIZE,
            id: self.id,
            created: self.created,
            modified,
            accessed: self.accessed.load(Ordering::Relaxed),
            changed: self.changed.load(Ordering::Relaxed).max(modified),
            links: self.links.load(Ordering::Relaxed),
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::File
    }

    fn access(&self) -> Access {
        *self.access.lock()
    }

    // As Linux's shmem_setattr: a file sealed with exec keeps its execute
    // bits.
    fn set_access(&self, change: &AccessChange) -> Result<()> {
        let mut access = self.access.lock();
        let changed = change.apply(*access);
        if self.pages.seals().contains(Seals::EXEC) && (changed.mode ^ access.mode) & 0o111 != 0 {
            return Err(Error::NotPermitted);
        }
        *access = changed;
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn read(&self, offset: u64, out: &mut dyn Out) -> Result<usize> {
        let size = self.size.load(Ordering::Acquire);
        if offset >= size {
            return Ok(0);
        }
        let len = (size - offset).min(out.len() as u64) as usize;
        let mut done = 0;
        while done < len {
            let at = offset + done as u64;
            let within = (at % PAGE_SIZE) as usize;
            let n = (PAGE_SIZE as usize - within).min(len - done);
            // A page not made reads as zeros, and is not made by reading.
            match self.pages.peek(at / PAGE_SIZE) {
                None => out.zero(done, n)?,
                Some(phys) => {
                    // The share taken keeps the page while it is copied;
                    // no lock is held, so the copy may fault.
                    let copied = copy_out(phys + within as u64, out, done, n);
                    unshare_page(phys);
                    copied?;
                }
            }
            done += n;
        }
        Ok(len)
    }

    fn write(&self, offset: u64, data: &dyn In) -> Result<usize> {
        let _resize = self.resize.lock();
        let size = self.size.load(Ordering::Acquire);
        let offset = if offset == APPEND { size } else { offset };
        let len = data.len() as u64;
        let end = offset.checked_add(len).filter(|&end| end <= MAX_SIZE).ok_or(Error::TooBig)?;
        if len == 0 {
            return Ok(0);
        }
        let end = self.writable_end(offset, end, size)?;
        let len = end - offset;
        self.pages.grow(pages_for(end));
        let mut done = 0u64;
        let mut failed = None;
        while done < len {
            let at = offset + done;
            let within = at % PAGE_SIZE;
            let n = (PAGE_SIZE - within).min(len - done);
            let phys = match self.pages.make(at / PAGE_SIZE) {
                Ok(phys) => phys,
                Err(Lack::NoMemory) => {
                    failed = Some(Error::NoMemory);
                    break;
                }
                Err(Lack::NoRoom | Lack::Past) => {
                    failed = Some(Error::NoSpace);
                    break;
                }
            };
            let copied = copy_in(data, done as usize, phys + within, n as usize);
            unshare_page(phys);
            if let Err(err) = copied {
                failed = Some(err);
                break;
            }
            done += n;
        }
        let written = offset + done;
        if written > size {
            self.size.store(written, Ordering::Release);
        }
        // Pages past what was written, which the growth above allowed,
        // are not the file's.
        let kept = size.max(written);
        if pages_for(kept) < pages_for(end) {
            self.pages.cut(pages_for(kept), kept);
        }
        match failed {
            Some(err) if done == 0 => Err(err),
            _ => {
                self.touched(now());
                Ok(done as usize)
            }
        }
    }

    fn set_size(&self, size: u64) -> Result<()> {
        if size > MAX_SIZE {
            return Err(Error::TooBig);
        }
        let _resize = self.resize.lock();
        let old = self.size.load(Ordering::Acquire);
        let seals = self.pages.seals();
        if (size < old && seals.contains(Seals::SHRINK))
            || (size > old && seals.contains(Seals::GROW))
        {
            return Err(Error::NotPermitted);
        }
        if size >= old {
            self.pages.grow(pages_for(size));
            self.size.store(size, Ordering::Release);
        } else {
            self.size.store(size, Ordering::Release);
            self.pages.cut(pages_for(size), size);
            unmap_past(&self.pages, pages_for(size));
        }
        self.touched(now());
        Ok(())
    }

    fn set_times(&self, accessed: Option<u64>, modified: Option<u64>) -> Result<()> {
        if let Some(accessed) = accessed {
            self.accessed.store(accessed, Ordering::Relaxed);
        }
        if let Some(modified) = modified {
            self.modified.store(modified, Ordering::Relaxed);
        }
        self.changed.store(now(), Ordering::Relaxed);
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Copies `len` bytes of RAM from `phys` to `out`, from its byte `at`.
fn copy_out(phys: u64, out: &mut dyn Out, at: usize, len: usize) -> Result<()> {
    let mut buffer = [0u8; STEP];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(STEP);
        Arch::read_frames(phys + done as u64, &mut buffer[..n]);
        out.put(at + done, &buffer[..n])?;
        done += n;
    }
    Ok(())
}

/// Copies `len` bytes of `data`, from its byte `at`, to RAM at `phys`.
fn copy_in(data: &dyn In, at: usize, phys: u64, len: usize) -> Result<()> {
    let mut buffer = [0u8; STEP];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(STEP);
        data.get(at + done, &mut buffer[..n])?;
        Arch::write_frames(phys + done as u64, &buffer[..n]);
        done += n;
    }
    Ok(())
}

/// What the memory files live in: a memory filesystem of its own that no
/// tree mounts, sized as `/devices/shm`, and the mount their locations
/// name.
struct MemoryFiles {
    fs: KArc<ShmFs>,
    mount: MountRef,
}

/// The memory filesystem of memory files, made when first needed.
fn memory_files() -> Result<&'static MemoryFiles> {
    static MEMORY_FILES: Once<MemoryFiles> = Once::new();
    if let Some(memory) = MEMORY_FILES.get() {
        return Ok(memory);
    }
    // As a tmpfs, and as `/devices/shm`: half of memory, and a file for
    // every two pages.
    let pages = FRAMES.managed_pages() / 2;
    let fs = ShmFs::new(pages, pages)?;
    let mount = radix_vfs::mount::Mount::detached(fs.clone(), "memfd")?;
    match MEMORY_FILES.set(MemoryFiles { fs, mount }) {
        Ok(memory) => {
            // Never unmounted, and every memory file holds it: counted on
            // each CPU for good, as the pipes' mount is.
            MountRef::count_on_cpus(&memory.mount);
            Ok(memory)
        }
        // Another thread made it first; this one's is dropped.
        Err(_) => MEMORY_FILES.get().ok_or(Error::NoMemory),
    }
}

/// Makes a memory file for `user`'s process in slot `to` (radix-process(2)'s
/// `memory_file`): empty, open for reading and writing, in no directory,
/// with the path `/memfd:NAME (deleted)`. It has the mode 0777 and the
/// owner and group of the process, or with `no_exec` the mode 0666 and the
/// seal exec, as Linux's MFD_NOEXEC_SEAL; without `allow_sealing` it
/// begins sealed with seal.
///
/// # Errors
///
/// EINVAL's status for a name of more than [`NAME_MAX`] bytes or with a
/// NUL in it, EFAULT's if it cannot be read, ENOSPC's if the filesystem
/// holds as many files as it may, and the slot's if the file cannot go
/// there.
#[inline(never)]
pub fn memory_file(
    user: &crate::user::UserRef,
    name: radix_abi::UserBuffer,
    to: u32,
    flags: MemoryFileFlags,
) -> core::result::Result<(), Status> {
    let Some(len) = usize::try_from(name.len).ok().filter(|&len| len <= NAME_MAX) else {
        return Err(Kernel::InvalidArgument.status(0));
    };
    if !radix_ipc::range::user_bytes(name.address, name.len) {
        return Err(Kernel::BadAddress.status(0));
    }
    let mut bytes = [0u8; NAME_MAX];
    if !Arch::copy_from_user(&mut bytes[..len], name.address) {
        return Err(Kernel::BadAddress.status(0));
    }
    let name = &bytes[..len];
    if name.contains(&0) {
        return Err(Kernel::InvalidArgument.status(0));
    }
    let memory = memory_files().map_err(super::status)?;
    let no_exec = flags.contains(MemoryFileFlags::NO_EXEC);
    let seals = if no_exec {
        Seals::EXEC
    } else if flags.contains(MemoryFileFlags::ALLOW_SEALING) {
        Seals::NONE
    } else {
        Seals::SEAL
    };
    let identity = user.process.identity();
    let mode = if no_exec { 0o666 } else { 0o777 };
    let access = Access { owner: identity.user, group: identity.group, mode };
    let shared = &memory.fs.root.shared;
    shared.count_file().map_err(super::status)?;
    let file = match ShmFile::new(shared, access, true) {
        Ok(file) => file,
        Err(err) => {
            shared.files.fetch_sub(1, Ordering::Relaxed);
            return Err(super::status(err));
        }
    };
    file.pages.set_seals(seals);
    // From here, the file uncounts itself when it goes, made or not.
    let node: NodeRef = file;
    let mut object = FileObject::new(Location::new(memory.mount.clone(), node), identity)
        .ok_or(Kernel::NoMemory.status(0))?;
    // Nothing else refers to the object yet.
    let Some(made) = alloc::sync::Arc::get_mut(&mut object) else {
        return Err(Kernel::NoMemory.status(0));
    };
    made.opened = Opened::verbatim(&[b"/memfd:", name, b" (deleted)"]);
    let rights = Rights::COPY.union(Rights::TRANSFER).union(Rights::READ).union(Rights::WRITE);
    crate::user::files::install_file(
        user,
        SlotRef::new(user.process.cspace(), to),
        ObjectKind::File,
        rights,
        object,
        flags.contains(MemoryFileFlags::CLOSE_ON_EXEC),
    )
}

/// Adds the seals `add` to the memory file `node` is, and gives its seals
/// (radix-file(2)'s `seal`); no `add` only reads them.
///
/// Write is added only if no space maps the file shared in a way that
/// may write (EBUSY's status if one does), and nothing is added once the
/// file is sealed with seal (EPERM's).
///
/// # Errors
///
/// EINVAL's status if `node` is no memory file, and those above.
pub fn seal(node: &NodeRef, add: Seals) -> core::result::Result<Seals, Status> {
    let file = radix_vfs::downcast::<ShmFile>(&**node)
        .filter(|file| file.memfd)
        .ok_or(Kernel::InvalidArgument.status(0))?;
    file.add_seals(add)
}
