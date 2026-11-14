// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The certificates Spark copies for the services that name them (ADR
//! 0512).
//!
//! certd keeps each certificate in `/system/state/certd/certificates`:
//! `NAME` is a symbolic link to the folder of its current generation,
//! `.NAME.G`, which holds `fullchain.pem` and `key.pem`. A service's file
//! names the certificates it uses, and Spark copies each into the
//! service's own folder, `/system/live/certificates/SERVICE`, laid
//! out the same way and given to the service's account, read-only: one
//! file cannot be two accounts', and a key is no other service's to read.
//! The service finds the folder as `CERTIFICATES_DIRECTORY` says. When
//! certd changes one, Spark copies it again for the services running
//! that name it, and they read it again on SIGHUP.
//!
//! Spark holds the whole tree, and certd, or anything that holds certd's
//! folder, can put anything in it, so Spark trusts nothing there: it
//! follows no link but a certificate's own, and only to one of that
//! certificate's generations; it reads regular files alone, no larger
//! than [`FILE_MAX`]; and it opens nothing that would wait. It does not
//! read what is in the files: certd and `cert` check them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::capability::Rights;
use libradix::abi::directory::{FileKind, OpenFlags};
use libradix::abi::errors::File as FileError;
use manifest::service::{self, CERTD_STATE, CERTIFICATE_FILES};
use radix_rt::fs::Dir;

use crate::services::{self, Spec};
use crate::views::Account;

/// The most bytes of a certificate's file Spark copies: a chain of ten
/// certificates fits many times over.
pub const FILE_MAX: u64 = 64 * 1024;

/// The read and write rights.
const READ_WRITE: Rights = Rights::READ.union(Rights::WRITE);

/// Makes service `spec`'s folder of copies hold the certificates its file
/// names, each copied anew from certd's, and nothing else; when it names
/// none, the folder goes. The copies are its account's, or root's for a
/// service without one.
pub fn copy_all(root: &Dir, spec: &Spec, account: Option<&Account>) -> Result<(), String> {
    let path = service::certificates_dir(&spec.name);
    if spec.certificates.is_empty() {
        remove_tree(root, &path[1..], 2);
        return Ok(());
    }
    let copies = folder(root, &spec.name, account)?;
    // What it names no more, or a link a copy left half made, goes.
    let entries = copies.entries().map_err(|err| format!("{path}: {err}"))?;
    for entry in &entries {
        let name = service::generation_of(&entry.name).map_or(entry.name.as_str(), |(n, _)| n);
        if !spec.certificates.iter().any(|named| named == name) {
            remove_tree(&copies, &entry.name, 1);
        }
    }
    for name in &spec.certificates {
        copy_into(root, &copies, name, account)?;
    }
    Ok(())
}

/// Copies certificate `name` anew for service `service`, which runs as
/// `account`, if it has one: what certd's change does for each running
/// service that names it.
pub fn copy(
    root: &Dir,
    service: &str,
    name: &str,
    account: Option<&Account>,
) -> Result<(), String> {
    let copies = folder(root, service, account)?;
    copy_into(root, &copies, name, account)
}

/// The folder of service `service`'s copies, made if it is not there, and
/// its account's alone.
fn folder(root: &Dir, service: &str, account: Option<&Account>) -> Result<Dir, String> {
    let path = service::certificates_dir(service);
    services::make_dirs(root, &path[1..])?;
    let (user, group) = owners(account);
    root.set_access(&path[1..], Some(user), Some(group), Some(0o500))
        .map_err(|err| format!("cannot give {path} to its account: {err}"))?;
    root.open_dir(&path[1..], READ_WRITE).map_err(|err| format!("{path}: {err}"))
}

/// The user and group a service's copies are given to.
fn owners(account: Option<&Account>) -> (u32, u32) {
    account.map_or((0, 0), |account| (account.user, account.group))
}

/// Copies certificate `name` from certd's folder into `copies` as a new
/// generation, points its link there, and removes the generations
/// before: a service reading it sees the old pair whole or the new one,
/// never a new chain with an old key.
fn copy_into(
    root: &Dir,
    copies: &Dir,
    name: &str,
    account: Option<&Account>,
) -> Result<(), String> {
    let files = read(root, name)?;
    let failed = |what: &str, err| format!("cannot copy its certificate {name}: {what}: {err}");
    let entries = copies.entries().map_err(|err| failed("its folder", err))?;
    let generations: Vec<u64> = entries
        .iter()
        .filter_map(|entry| service::generation_of(&entry.name))
        .filter(|&(of, _)| of == name)
        .map(|(_, generation)| generation)
        .collect();
    let generation = generations.iter().max().map_or(1, |newest| newest + 1);
    let folder = service::certificate_generation(name, generation);
    let (user, group) = owners(account);
    copies.make_dir(&folder).map_err(|err| failed(&folder, err))?;
    for (file, data) in CERTIFICATE_FILES.iter().zip(&files) {
        let path = format!("{folder}/{file}");
        copies.write_file(&path, data).map_err(|err| failed(&path, err))?;
        copies
            .set_access(&path, Some(user), Some(group), Some(0o400))
            .map_err(|err| failed(&path, err))?;
    }
    copies
        .set_access(&folder, Some(user), Some(group), Some(0o500))
        .map_err(|err| failed(&folder, err))?;
    let new = format!(".{name}.new");
    let _ = copies.remove(&new);
    copies.make_link(&new, &folder).map_err(|err| failed(&new, err))?;
    copies.rename(&new, copies, name).map_err(|err| failed(name, err))?;
    // A service that opened a file of an older one keeps what it opened.
    for older in generations.iter().filter(|&&older| older != generation) {
        remove_tree(copies, &service::certificate_generation(name, *older), 1);
    }
    Ok(())
}

/// The chain and key of certificate `name`, read from certd's folder
/// trusting nothing in it.
fn read(root: &Dir, name: &str) -> Result<[Vec<u8>; 2], String> {
    // Beneath certd's folder, a link with an absolute target leads
    // nowhere, as `..` above it does; the folder itself is no link.
    let flags = OpenFlags::NO_FOLLOW.union(OpenFlags::ABSOLUTE_LINKS_LEAVE);
    let store = root
        .open_dir_with(&CERTD_STATE[1..], flags, Rights::READ)
        .map_err(|err| format!("{CERTD_STATE}: {err}"))?;
    let target = match store.read_link(name) {
        Ok(target) => target,
        Err(err) if err.is(FileError::NotFound) => {
            return Err(format!(
                "its certificate {name} does not exist: certd makes this-machine; add others \
                 with `cert add`"
            ));
        }
        Err(_) => Vec::new(),
    };
    // The link leads to one of its own generations, by the name certd
    // gives it exactly, in the same folder.
    let folder = String::from_utf8(target)
        .ok()
        .filter(|target| {
            service::generation_of(target).is_some_and(|(of, generation)| {
                of == name && service::certificate_generation(name, generation) == *target
            })
        })
        .ok_or_else(|| format!("certd's {name} is not a link to one of its generations"))?;
    let generation = store
        .open_dir_with(&folder, OpenFlags::NO_FOLLOW, Rights::READ)
        .map_err(|err| format!("certd's {name}: {folder}: {err}"))?;
    let mut files = [Vec::new(), Vec::new()];
    for (file, data) in CERTIFICATE_FILES.iter().zip(&mut files) {
        *data = read_file(&generation, name, file)?;
    }
    Ok(files)
}

/// The file `file` of certificate `name`'s generation `folder`: a regular
/// file of [`FILE_MAX`] bytes or less, which is no link. A named pipe or
/// a device node opens what it stands for, so the file is looked at
/// before it is opened, and once more after, in case it was swapped
/// between.
fn read_file(folder: &Dir, name: &str, file: &str) -> Result<Vec<u8>, String> {
    let refused = || {
        format!("certd's {name}/{file} is not a regular file of {} KiB or less", FILE_MAX / 1024)
    };
    let regular = |kind, size| kind == FileKind::File && size <= FILE_MAX;
    let before = match folder.stat_link(file) {
        Ok(stat) => stat,
        Err(err) if err.is(FileError::NotFound) => {
            return Err(format!("certd's {name} has no {file}"));
        }
        Err(err) => return Err(format!("certd's {name}/{file}: {err}")),
    };
    if !regular(before.kind, before.size) {
        return Err(refused());
    }
    let flags = OpenFlags::NO_FOLLOW.union(OpenFlags::NON_BLOCKING).union(OpenFlags::NO_TERMINAL);
    let opened = folder
        .open(file, flags, Rights::READ)
        .map_err(|err| format!("certd's {name}/{file}: {err}"))?;
    let stat = opened.stat().map_err(|err| format!("certd's {name}/{file}: {err}"))?;
    if !regular(stat.kind, stat.size) || stat.id != before.id {
        return Err(refused());
    }
    let mut data = alloc::vec![0u8; stat.size as usize];
    let mut at = 0;
    while at < data.len() {
        match opened.read_at(at as u64, &mut data[at..]) {
            Ok(0) => break,
            Ok(read) => at += read,
            Err(err) => return Err(format!("certd's {name}/{file}: {err}")),
        }
    }
    data.truncate(at);
    Ok(data)
}

/// Removes `path` beneath `dir`, and what is in it, `depth` folders
/// deep: Spark's own folders, whose layout it knows. What cannot be
/// removed stays.
fn remove_tree(dir: &Dir, path: &str, depth: u32) {
    if dir.remove(path).is_ok() || depth == 0 {
        return;
    }
    let Ok(inner) = dir.open_dir_with(path, OpenFlags::NO_FOLLOW, READ_WRITE) else { return };
    if let Ok(entries) = inner.entries() {
        for entry in entries {
            remove_tree(&inner, &entry.name, depth - 1);
        }
    }
    let _ = dir.remove(path);
}
