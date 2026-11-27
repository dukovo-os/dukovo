// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A restored copy compared with the snapshot it was made from, file by
//! file (ADR 0463): names, kinds, owners, modes, the times their
//! contents last changed, links, devices, extended attributes and data.
//! The times a file was last read and changed in any way are not
//! compared, as a receive cannot set the second.

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use sysapi::fs::Link;

/// The extended attributes of `path`, itself and not what a link names,
/// by name.
fn attributes(path: &Path) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut found = Vec::new();
    for name in sysapi::fs::xattr_names(path, Link::Itself)? {
        let value = sysapi::fs::xattr(path, &name, Link::Itself)?;
        found.push((name.into_vec(), value));
    }
    found.sort();
    Ok(found)
}

/// What is compared of an entry's own record.
fn record(meta: &Metadata) -> (u32, u32, u32, u64, i64, i64, u64) {
    let file = meta.is_file();
    (
        meta.mode(),
        meta.uid(),
        meta.gid(),
        if file { meta.len() } else { 0 },
        meta.mtime(),
        meta.mtime_nsec(),
        if file {
            meta.nlink()
        } else if meta.file_type().is_dir() {
            0
        } else {
            meta.rdev()
        },
    )
}

/// Whether the files `a` and `b` hold the same bytes.
fn same_data(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (File::open(a)?, File::open(b)?);
    let (mut left, mut right) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let got = read_full(&mut a, &mut left)?;
        if read_full(&mut b, &mut right[..got.max(1)])? != got || left[..got] != right[..got] {
            return Ok(false);
        }
        if got == 0 {
            return Ok(true);
        }
    }
}

/// Reads until `buffer` is full or the file ends; how much was read.
fn read_full(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buffer.len() {
        match file.read(&mut buffer[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// The names in the folder `path`, sorted.
fn names(path: &Path) -> io::Result<Vec<std::ffi::OsString>> {
    let mut names: Vec<_> =
        fs::read_dir(path)?.map(|entry| entry.map(|e| e.file_name())).collect::<Result<_, _>>()?;
    names.sort();
    Ok(names)
}

/// Compares the tree at `copy` with the one at `original`; how many
/// entries were compared, or the first difference, by its path from the
/// top.
pub fn same(copy: &Path, original: &Path) -> Result<u64, String> {
    let mut count = 0;
    let mut pending: Vec<PathBuf> = vec![PathBuf::new()];
    let fail = |at: &Path, why: &str| format!("/{}: {why}", at.display());
    let io = |at: &Path, error: io::Error| format!("/{}: {error}", at.display());
    while let Some(at) = pending.pop() {
        let (a, b) = (copy.join(&at), original.join(&at));
        let (meta_a, meta_b) = (
            fs::symlink_metadata(&a).map_err(|e| io(&at, e))?,
            fs::symlink_metadata(&b).map_err(|e| io(&at, e))?,
        );
        count += 1;
        if record(&meta_a) != record(&meta_b) {
            return Err(fail(&at, "its owner, mode, size, time or links differ"));
        }
        if attributes(&a).map_err(|e| io(&at, e))? != attributes(&b).map_err(|e| io(&at, e))? {
            return Err(fail(&at, "its extended attributes differ"));
        }
        let kind = meta_a.file_type();
        if kind.is_symlink() {
            if fs::read_link(&a).map_err(|e| io(&at, e))?
                != fs::read_link(&b).map_err(|e| io(&at, e))?
            {
                return Err(fail(&at, "it links to another place"));
            }
        } else if kind.is_file() {
            if !same_data(&a, &b).map_err(|e| io(&at, e))? {
                return Err(fail(&at, "its data differ"));
            }
        } else if kind.is_dir() {
            let (in_a, in_b) =
                (names(&a).map_err(|e| io(&at, e))?, names(&b).map_err(|e| io(&at, e))?);
            if in_a != in_b {
                let only = |x: &[std::ffi::OsString], y: &[std::ffi::OsString]| {
                    x.iter().find(|n| !y.contains(n)).cloned()
                };
                let why = match (only(&in_a, &in_b), only(&in_b, &in_a)) {
                    (Some(extra), _) => format!("{} is in the copy alone", extra.to_string_lossy()),
                    (_, Some(missing)) => {
                        format!("{} is missing from the copy", missing.to_string_lossy())
                    }
                    _ => "its names differ".into(),
                };
                return Err(fail(&at, &why));
            }
            pending.extend(in_a.iter().rev().map(|name| at.join(name)));
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;
    use sysapi::fs::Time;

    fn tree(name: &str) -> PathBuf {
        let top =
            std::env::temp_dir().join(format!("backup-compare-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        fs::create_dir_all(top.join("a/b")).unwrap();
        fs::write(top.join("a/b/file"), vec![7u8; 3 << 20]).unwrap();
        fs::write(top.join("note"), b"hello").unwrap();
        symlink("a/b/file", top.join("link")).unwrap();
        top
    }

    /// Gives `copy`'s entries the times of `original`'s, as a receive
    /// does.
    fn same_times(copy: &Path, original: &Path) {
        let mut paths = vec![PathBuf::new()];
        while let Some(at) = paths.pop() {
            let meta = fs::symlink_metadata(original.join(&at)).unwrap();
            if meta.is_dir() {
                for name in names(&original.join(&at)).unwrap() {
                    paths.push(at.join(name));
                }
            }
            let modified =
                Time::At { seconds: meta.mtime(), nanoseconds: meta.mtime_nsec() as u32 };
            let _ = sysapi::fs::set_times(&copy.join(&at), Time::Keep, modified, Link::Itself);
        }
    }

    #[test]
    fn a_copy_is_the_same_until_one_thing_differs() {
        let (one, two) = (tree("one"), tree("two"));
        same_times(&two, &one);
        assert_eq!(same(&two, &one), Ok(6));
        fs::write(two.join("a/b/file"), vec![8u8; 3 << 20]).unwrap();
        same_times(&two, &one);
        assert_eq!(same(&two, &one), Err("/a/b/file: its data differ".into()));
        fs::write(two.join("a/b/file"), vec![7u8; 3 << 20]).unwrap();
        fs::set_permissions(two.join("note"), fs::Permissions::from_mode(0o600)).unwrap();
        same_times(&two, &one);
        assert!(same(&two, &one).unwrap_err().starts_with("/note: its owner, mode"));
        fs::remove_file(two.join("note")).unwrap();
        same_times(&two, &one);
        assert_eq!(same(&two, &one), Err("/: note is missing from the copy".into()));
        for top in [one, two] {
            fs::remove_dir_all(top).unwrap();
        }
    }
}
