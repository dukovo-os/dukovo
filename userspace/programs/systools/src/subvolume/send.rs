// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `subvolume send` (ADR 0462): a read-only subvolume as a send stream,
//! whole or as its changes from a parent, as Linux's `btrfs send` writes
//! one. The kernel compares the two trees (radix-directory(2)'s
//! `compare`); the Btrfs crate turns what differs into the stream's
//! commands, and reads the files' data through the snapshot's own path.

use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use radix_abi::directory::{self, TreeItem as T, TreeRange as R};
use radix_abi::endpoint::NO_SLOT;
use radix_btrfs::format::Key;
use radix_btrfs::send::sender::{self, Item, Source, Subvolume, Trees};
use radix_btrfs::send::stream::Writer;

use super::{Info, info, is_top};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// How much one `compare` gathers: the most the kernel gives at once.
const BATCH: usize = 256 << 10;

/// `range` as the kernel takes it: a TreeRange.
fn range_bytes(from: &Key, to: &Key) -> [u8; R::SIZE] {
    let mut bytes = [0u8; R::SIZE];
    bytes[R::OFFSET_START_OBJECTID..][..8].copy_from_slice(&from.objectid.to_le_bytes());
    bytes[R::OFFSET_START_OFFSET..][..8].copy_from_slice(&from.offset.to_le_bytes());
    bytes[R::OFFSET_END_OBJECTID..][..8].copy_from_slice(&to.objectid.to_le_bytes());
    bytes[R::OFFSET_END_OFFSET..][..8].copy_from_slice(&to.offset.to_le_bytes());
    bytes[R::OFFSET_START_KIND] = from.kind;
    bytes[R::OFFSET_END_KIND] = to.kind;
    bytes
}

/// The `count` TreeItem records `compare` wrote at the start of `bytes`,
/// checked against the bytes they take, each as old and new data, or the
/// data of one tree alone as old when `old_only`.
pub(super) fn records(bytes: &[u8], count: u32, old_only: bool) -> io::Result<Vec<Item>> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "the kernel's comparison is cut short");
    let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or([0; 8]));
    let half = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]));
    let mut items = Vec::with_capacity(count as usize);
    let mut at = 0usize;
    for _ in 0..count {
        if bytes.len() < at + T::SIZE {
            return Err(bad());
        }
        let key = Key::new(
            word(at + T::OFFSET_OBJECTID),
            bytes[at + T::OFFSET_KIND],
            word(at + T::OFFSET_OFFSET),
        );
        let old_len = half(at + T::OFFSET_OLD_LENGTH) as usize;
        let new_len = half(at + T::OFFSET_NEW_LENGTH) as usize;
        let size = (T::SIZE + old_len + new_len).next_multiple_of(8);
        if bytes.len() < at + size {
            return Err(bad());
        }
        let data = at + T::SIZE;
        let (old, new) = match bytes[at + T::OFFSET_CHANGE] {
            directory::TREE_ITEM_ADDED => (None, Some(bytes[data..data + new_len].to_vec())),
            directory::TREE_ITEM_REMOVED => (Some(bytes[data..data + old_len].to_vec()), None),
            directory::TREE_ITEM_CHANGED => (
                Some(bytes[data..data + old_len].to_vec()),
                Some(bytes[data + old_len..data + old_len + new_len].to_vec()),
            ),
            _ => return Err(bad()),
        };
        items.push(if old_only {
            Item { key, old: new, new: None }
        } else {
            Item { key, old, new }
        });
        at += size;
    }
    Ok(items)
}

/// What a send reads on Dukovo: the trees through the kernel, the files
/// through the snapshot's path.
struct SysSource {
    snapshot: File,
    parent: Option<File>,
    path: PathBuf,
    buffer: Vec<u8>,
    /// The file last read, open.
    open: Option<(Vec<u8>, File)>,
}

impl Source for SysSource {
    fn compare(
        &mut self,
        trees: Trees,
        from: &Key,
        to: &Key,
        out: &mut Vec<Item>,
    ) -> io::Result<bool> {
        let slot = |file: &File| sys::slot_of(file.as_raw_fd());
        let no_parent = || io::Error::other("no parent to compare with");
        let (of, with) = match trees {
            Trees::Both => (slot(&self.snapshot), self.parent.as_ref().map_or(NO_SLOT, slot)),
            Trees::Old => (self.parent.as_ref().map(slot).ok_or_else(no_parent)?, NO_SLOT),
            Trees::New => (slot(&self.snapshot), NO_SLOT),
        };
        let range = range_bytes(from, to);
        let results = directory::compare(&SYS, of, b".", with, &range, &mut self.buffer)
            .map_err(sys::io_error)?;
        let used = (results.used as usize).min(self.buffer.len());
        out.extend(records(&self.buffer[..used], results.count, trees == Trees::Old)?);
        Ok(results.done)
    }

    fn read(&mut self, path: &[u8], offset: u64, out: &mut [u8]) -> io::Result<usize> {
        if self.open.as_ref().is_none_or(|(at, _)| at != path) {
            let file = File::options()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(self.path.join(std::ffi::OsStr::from_bytes(path)))?;
            self.open = Some((path.to_vec(), file));
        }
        let Some((_, file)) = &self.open else { return Ok(0) };
        let mut done = 0;
        while done < out.len() {
            match file.read_at(&mut out[done..], offset + done as u64)? {
                0 => break,
                n => done += n,
            }
        }
        Ok(done)
    }

    fn read_link(&mut self, path: &[u8]) -> io::Result<Vec<u8>> {
        Ok(std::fs::read_link(self.path.join(std::ffi::OsStr::from_bytes(path)))?
            .into_os_string()
            .into_vec())
    }
}

/// What a send says of the subvolume `info` describes.
fn sent(info: &Info) -> Subvolume {
    Subvolume {
        uuid: info.uuid,
        received_uuid: info.received_uuid,
        changed_transaction: info.changed_transaction,
    }
}

/// The top of a read-only subvolume, open, and what Btrfs keeps of it.
fn read_only_top(path: &Path) -> Result<(File, Info), Fail> {
    let Some(info) = info(path)? else {
        return Err(util::fail(format!("not a btrfs filesystem: {}", path.display())));
    };
    if !is_top(path) {
        return Err(util::fail(format!("not a subvolume: {}", path.display())));
    }
    if !info.read_only {
        return Err(util::fail(format!(
            "subvolume {} is not read-only; send a read-only snapshot of it",
            path.display()
        )));
    }
    let file = File::open(path).map_err(|e| util::failed(&path.display(), e))?;
    Ok((file, info))
}

/// Sends each of `subvolumes` to `out`, or to standard output, each a
/// stream of its own, whole or as its changes from `parent`.
pub(super) fn run(
    subvolumes: &[PathBuf],
    parent: Option<&Path>,
    out: Option<&Path>,
    quiet: bool,
) -> Result<(), Fail> {
    let output: Box<dyn Write> = match out {
        Some(path) => Box::new(
            File::options()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .map_err(|e| util::failed(&path.display(), e))?,
        ),
        None if io::stdout().is_terminal() => {
            return Err(util::fail(
                "not writing a send stream to a terminal: redirect it, or name a file with -f"
                    .into(),
            ));
        }
        None => Box::new(io::stdout().lock()),
    };
    let mut output = BufWriter::with_capacity(1 << 20, output);
    let parent = parent.map(read_only_top).transpose()?;
    for path in subvolumes {
        if !quiet {
            let _ = writeln!(io::stderr(), "At subvol {}", path.display());
        }
        send_one(path, parent.as_ref(), &mut output)?;
    }
    output.flush().map_err(|e| util::failed(&"the stream", e))?;
    Ok(())
}

/// Sends the read-only snapshot `path` to `out` as one stream, whole or
/// as its changes from the read-only snapshot `parent`, as backup(8)
/// does (ADR 0463).
pub(crate) fn stream(path: &Path, parent: Option<&Path>, out: &mut dyn Write) -> Result<(), Fail> {
    let parent = parent.map(read_only_top).transpose()?;
    send_one(path, parent.as_ref(), out)
}

/// Sends `path` to `output`, with `parent` opened and read.
fn send_one(
    path: &Path,
    parent: Option<&(File, Info)>,
    output: &mut (impl Write + ?Sized),
) -> Result<(), Fail> {
    let (snapshot, info) = read_only_top(path)?;
    let name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.as_bytes().to_vec()))
        .unwrap_or_else(|| info.name.clone().into_bytes());
    let mut source = SysSource {
        snapshot,
        parent: parent
            .map(|(file, _)| file.try_clone())
            .transpose()
            .map_err(|e| util::failed(&path.display(), e))?,
        path: path.to_path_buf(),
        buffer: vec![0; BATCH],
        open: None,
    };
    let failed = |e: io::Error| util::failed(&path.display(), e);
    let mut writer = Writer::new(output).map_err(failed)?;
    let from = parent.map(|(_, info)| sent(info));
    sender::send(&mut source, &name, &sent(&info), from.as_ref(), &mut writer).map_err(failed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: (u64, u8, u64), change: u8, old: &[u8], new: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; T::SIZE];
        bytes[T::OFFSET_OBJECTID..][..8].copy_from_slice(&key.0.to_le_bytes());
        bytes[T::OFFSET_KIND] = key.1;
        bytes[T::OFFSET_OFFSET..][..8].copy_from_slice(&key.2.to_le_bytes());
        bytes[T::OFFSET_CHANGE] = change;
        bytes[T::OFFSET_OLD_LENGTH..][..4].copy_from_slice(&(old.len() as u32).to_le_bytes());
        bytes[T::OFFSET_NEW_LENGTH..][..4].copy_from_slice(&(new.len() as u32).to_le_bytes());
        bytes.extend_from_slice(old);
        bytes.extend_from_slice(new);
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes
    }

    #[test]
    fn the_kernels_records_read_back_as_items() {
        let mut bytes = record((256, 1, 0), directory::TREE_ITEM_ADDED, b"", b"abc");
        bytes.extend(record((257, 12, 256), directory::TREE_ITEM_CHANGED, b"old", b"newer"));
        bytes.extend(record((258, 1, 0), directory::TREE_ITEM_REMOVED, b"gone!", b""));
        let items = records(&bytes, 3, false).unwrap();
        assert_eq!(
            items[0],
            Item { key: Key::new(256, 1, 0), old: None, new: Some(b"abc".to_vec()) }
        );
        assert_eq!(
            items[1],
            Item {
                key: Key::new(257, 12, 256),
                old: Some(b"old".to_vec()),
                new: Some(b"newer".to_vec())
            }
        );
        assert_eq!(
            items[2],
            Item { key: Key::new(258, 1, 0), old: Some(b"gone!".to_vec()), new: None }
        );
        // One tree walked alone is the parent's, as old.
        let old = records(&bytes[..T::SIZE + 8], 1, true).unwrap();
        assert_eq!(
            old[0],
            Item { key: Key::new(256, 1, 0), old: Some(b"abc".to_vec()), new: None }
        );
    }

    #[test]
    fn records_cut_short_or_unknown_are_refused() {
        let bytes = record((256, 1, 0), directory::TREE_ITEM_ADDED, b"", b"abcdefgh");
        assert!(records(&bytes[..bytes.len() - 8], 1, false).is_err());
        assert!(records(&bytes, 2, false).is_err());
        let mut odd = bytes.clone();
        odd[T::OFFSET_CHANGE] = 9;
        assert!(records(&odd, 1, false).is_err());
    }

    #[test]
    fn a_range_is_a_tree_range() {
        let bytes = range_bytes(&Key::new(256, 1, 2), &Key::new(300, 108, u64::MAX));
        assert_eq!(&bytes[..8], &256u64.to_le_bytes());
        assert_eq!(&bytes[8..16], &2u64.to_le_bytes());
        assert_eq!(&bytes[16..24], &300u64.to_le_bytes());
        assert_eq!(&bytes[24..32], &u64::MAX.to_le_bytes());
        assert_eq!((bytes[32], bytes[33]), (1, 108));
        assert_eq!(&bytes[34..], &[0; 6]);
    }
}
