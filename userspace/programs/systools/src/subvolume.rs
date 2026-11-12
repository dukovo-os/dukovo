// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! subvolume(8): makes, snapshots, removes and shows Btrfs subvolumes, as
//! Linux's `btrfs subvolume` does with `create`, `snapshot`, `delete` and
//! `show` (ADRs 0138 and 0461), through the directory interface
//! (radix-directory(2)); and sends and receives them as Linux's `btrfs
//! send` and `btrfs receive` do (ADR 0462).

pub(crate) mod receive;
pub(crate) mod send;

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use radix_abi::directory::{self, SubvolumeInfo as S};
use radix_abi::errors::Kernel;

use crate::args::{Arg, Args};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// The directory that holds `path`, open, and the last name of `path`.
fn place(path: &Path) -> Result<(File, String), Fail> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(util::fail(format!("{}: not a name for a subvolume", path.display())));
    };
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let dir = File::open(parent).map_err(|e| util::failed(&parent.display(), e))?;
    Ok((dir, name.to_string()))
}

/// The bits of `st_ino` that are the inode's own number on Radix (the
/// subvolume's number is above them), and a subvolume's top folder's.
pub(crate) const INODE_BITS: u64 = (1 << 40) - 1;
pub(crate) const TOP_INODE: u64 = 256;

/// Whether the folder `path` is the top of a Btrfs subvolume.
pub(crate) fn is_top(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir() && m.ino() & INODE_BITS == TOP_INODE)
}

/// The slot of the directory `dir`, opened.
fn slot(dir: &File) -> u32 {
    sys::slot_of(dir.as_raw_fd())
}

/// Makes a new, empty subvolume at `path`, quietly, for the tools that
/// make one as a step of their own.
pub(crate) fn make(path: &Path) -> Result<(), Fail> {
    let (dir, name) = place(path)?;
    directory::make_subvolume(&SYS, slot(&dir), name.as_bytes())
        .map_err(|s| util::failed(&path.display(), sys::io_error(s)))
}

/// Makes `dest`, a path not there yet, a snapshot of the subvolume that
/// holds `source`, read-only if `read_only`, quietly.
pub(crate) fn snapshot_to(source: &Path, dest: &Path, read_only: bool) -> Result<(), Fail> {
    let from = File::open(source).map_err(|e| util::failed(&source.display(), e))?;
    let (dir, name) = place(dest)?;
    directory::snapshot(&SYS, slot(&from), b".", slot(&dir), name.as_bytes(), read_only)
        .map_err(|s| util::failed(&dest.display(), sys::io_error(s)))
}

/// Removes the subvolume at `path`, with everything in it, quietly.
pub(crate) fn remove(path: &Path) -> Result<(), Fail> {
    let (dir, name) = place(path)?;
    directory::remove_subvolume(&SYS, slot(&dir), name.as_bytes())
        .map_err(|s| util::failed(&path.display(), sys::io_error(s)))
}

/// What Btrfs keeps of a subvolume (ADR 0461). Times are nanoseconds
/// since 1970, 0 for never.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Info {
    /// Its ID: 5 for the top of the filesystem.
    pub id: u64,
    /// The ID of the subvolume that holds it; 0 for the top.
    pub parent: u64,
    pub generation: u64,
    /// Whether nothing in it may change.
    pub read_only: bool,
    /// The transaction that last changed a file in it.
    pub changed_transaction: u64,
    /// The transaction that made it.
    pub made_transaction: u64,
    /// The transaction it was sent in, for one received.
    pub sent_transaction: u64,
    /// When it was made.
    pub made: u64,
    pub uuid: [u8; 16],
    /// The UUID of the subvolume it is a snapshot of; zeros if none.
    pub parent_uuid: [u8; 16],
    pub received_uuid: [u8; 16],
    /// Its name in the folder that holds it; empty for the top.
    pub name: String,
}

/// What Btrfs keeps of the subvolume that holds `path`; `None` if `path`
/// is on another filesystem.
pub(crate) fn info(path: &Path) -> Result<Option<Info>, Fail> {
    let file = File::open(path).map_err(|e| util::failed(&path.display(), e))?;
    let mut buffer = [0u8; S::SIZE + 255];
    match directory::subvolume(&SYS, slot(&file), b".", &mut buffer) {
        Ok(()) => {}
        Err(status)
            if matches!(
                Kernel::from_status(status),
                Some(Kernel::NotSupported | Kernel::WrongKind)
            ) =>
        {
            return Ok(None);
        }
        Err(status) => return Err(util::failed(&path.display(), sys::io_error(status))),
    }
    Ok(Some(decode(&buffer)))
}

/// The Info in `buffer`, as `subvolume` wrote it.
fn decode(buffer: &[u8]) -> Info {
    let word = |at: usize| u64::from_le_bytes(buffer[at..at + 8].try_into().unwrap_or([0; 8]));
    let uuid = |at: usize| buffer[at..at + 16].try_into().unwrap_or([0; 16]);
    let len = (word(S::OFFSET_NAME_LENGTH) as usize).min(buffer.len() - S::SIZE);
    Info {
        id: word(S::OFFSET_ID),
        parent: word(S::OFFSET_PARENT),
        generation: word(S::OFFSET_GENERATION),
        read_only: word(S::OFFSET_FLAGS) & directory::SUBVOLUME_READ_ONLY != 0,
        changed_transaction: word(S::OFFSET_CHANGED_TRANSACTION),
        made_transaction: word(S::OFFSET_MADE_TRANSACTION),
        sent_transaction: word(S::OFFSET_SENT_TRANSACTION),
        made: word(S::OFFSET_MADE),
        uuid: uuid(S::OFFSET_UUID),
        parent_uuid: uuid(S::OFFSET_PARENT_UUID),
        received_uuid: uuid(S::OFFSET_RECEIVED_UUID),
        name: String::from_utf8_lossy(&buffer[S::SIZE..S::SIZE + len]).into_owned(),
    }
}

/// A UUID as `btrfs subvolume show` writes it: `-` for none.
fn uuid_or_dash(uuid: &[u8; 16]) -> String {
    if *uuid == [0; 16] { "-".into() } else { crate::probe::uuid(uuid) }
}

/// Shows what Btrfs keeps of the subvolume that holds `path`, as `btrfs
/// subvolume show` does.
fn show(path: &Path) -> Result<(), Fail> {
    let Some(info) = info(path)? else {
        return Err(util::fail(format!("not a btrfs filesystem: {}", path.display())));
    };
    let made = if info.made == 0 {
        "-".into()
    } else {
        util::local_time((info.made / 1_000_000_000) as i64, "%Y-%m-%d %H:%M:%S %z")
    };
    let name = if info.name.is_empty() { "<FS_TREE>" } else { &info.name };
    let mut out = io::stdout();
    let _ = writeln!(out, "{}", path.display());
    for (field, value) in [
        ("Name", name.to_string()),
        ("UUID", uuid_or_dash(&info.uuid)),
        ("Parent UUID", uuid_or_dash(&info.parent_uuid)),
        ("Received UUID", uuid_or_dash(&info.received_uuid)),
        ("Creation time", made),
        ("Subvolume ID", info.id.to_string()),
        ("Generation", info.generation.to_string()),
        ("Gen at creation", info.made_transaction.to_string()),
        ("Parent ID", info.parent.to_string()),
        ("Flags", if info.read_only { "readonly" } else { "-" }.into()),
    ] {
        let _ = writeln!(out, "\t{:<17}{value}", format!("{field}:"));
    }
    Ok(())
}

/// Makes a new, empty subvolume at `path`.
fn create(path: &Path) -> Result<(), Fail> {
    make(path)?;
    let _ = writeln!(io::stdout(), "Create subvolume '{}'", path.display());
    Ok(())
}

/// Makes a snapshot of the subvolume that holds `source` at `dest`, or
/// in `dest` with `source`'s name if it is a directory, read-only if
/// `read_only`.
fn snapshot(source: &Path, dest: &Path, read_only: bool) -> Result<(), Fail> {
    let from = File::open(source).map_err(|e| util::failed(&source.display(), e))?;
    let (dir, name, at) = if dest.is_dir() {
        let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
            return Err(util::fail(format!("{}: name the snapshot", source.display())));
        };
        let dir = File::open(dest).map_err(|e| util::failed(&dest.display(), e))?;
        (dir, name.to_string(), dest.join(name))
    } else {
        let (dir, name) = place(dest)?;
        (dir, name, dest.to_path_buf())
    };
    directory::snapshot(&SYS, slot(&from), b".", slot(&dir), name.as_bytes(), read_only)
        .map_err(|s| util::failed(&at.display(), sys::io_error(s)))?;
    let kind = if read_only { "a readonly snapshot" } else { "a snapshot" };
    let _ = writeln!(io::stdout(), "Create {kind} of '{}' in '{}'", source.display(), at.display());
    Ok(())
}

/// Removes the subvolume at `path`, with everything in it.
fn delete(path: &Path) -> Result<(), Fail> {
    remove(path)?;
    let _ = writeln!(io::stdout(), "Delete subvolume '{}'", path.display());
    Ok(())
}

/// subvolume(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let usage =
        || util::usage("say create, snapshot, delete, show, send or receive, and the paths".into());
    let Some(action) = args.subcommand() else { return Err(usage()) };
    let mut read_only = false;
    let (mut parent, mut file) = (None, None);
    let (mut quiet, mut stop_at_end, mut dump) = (false, false, false);
    let mut paths = Vec::new();
    let streams = action == "send" || action == "receive";
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('r') if action == "snapshot" => read_only = true,
            Arg::Short('p') if action == "send" => parent = Some(PathBuf::from(args.value()?)),
            Arg::Short('f') if streams => file = Some(PathBuf::from(args.value()?)),
            Arg::Short('q') if streams => quiet = true,
            Arg::Long(long) if streams && long == "quiet" => quiet = true,
            Arg::Short('e') if action == "receive" => stop_at_end = true,
            Arg::Long(long) if action == "receive" && long == "dump" => dump = true,
            Arg::Operand(path) => paths.push(PathBuf::from(path)),
            other => return Err(args.unknown(&other)),
        }
    }
    match (action.to_str(), paths.as_slice()) {
        (Some("send"), paths) if !paths.is_empty() => {
            send::run(paths, parent.as_deref(), file.as_deref(), quiet)?;
        }
        (Some("receive"), []) if dump => receive::dump(file.as_deref())?,
        (Some("receive"), [dest]) if !dump => {
            receive::run(dest, file.as_deref(), stop_at_end, quiet)?;
        }
        (Some("create"), paths) if !paths.is_empty() => {
            for path in paths {
                create(path)?;
            }
        }
        (Some("snapshot"), [source, dest]) => snapshot(source, dest, read_only)?,
        (Some("delete"), paths) if !paths.is_empty() => {
            for path in paths {
                delete(path)?;
            }
        }
        (Some("show"), [path]) => show(path)?,
        _ => return Err(usage()),
    }
    Ok(0)
}
