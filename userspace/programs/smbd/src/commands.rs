// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The commands on a share's files (MS-SMB2 3.3.5.9 to 3.3.5.21):
//! opening and closing them, reading and writing, listing folders, what
//! a file is and changing it, locks, and the few IOCTLs a PC needs.
//!
//! A file is opened for its data only when the PC asks to read or write
//! it; an open for its attributes alone, as Windows makes to look at a
//! file, opens nothing. Files that are neither regular files nor folders,
//! as devices and named pipes, are not served: opening one could block
//! the session. No oplocks or leases are granted, so a PC keeps nothing
//! cached that another may change.

use std::fs::{self, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::crypto::dialect;
use crate::fs::{self as share, Kind, Listing, Lock, Open, Tree, access};
use crate::info::{self, Opened, Stat, attribute, class, fs_class};
use crate::negotiate::SECURITY_MODE;
use crate::sys;
use crate::wire::{self, HEADER, Out, Status, status, status_of};
use crate::worker::{Chain, OPENS_MAX, Reply, Worker, maximal_access};

/// CREATE's dispositions (MS-SMB2 2.2.13).
mod disposition {
    pub const SUPERSEDE: u32 = 0;
    pub const OPEN: u32 = 1;
    pub const CREATE: u32 = 2;
    pub const OVERWRITE: u32 = 4;
    pub const OVERWRITE_IF: u32 = 5;
}

/// CREATE's options that smbd reads.
mod option {
    pub const DIRECTORY_FILE: u32 = 0x0000_0001;
    pub const NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    pub const DELETE_ON_CLOSE: u32 = 0x0000_1000;
    pub const OPEN_BY_FILE_ID: u32 = 0x0000_2000;
}

/// What CREATE did.
mod action {
    pub const SUPERSEDED: u32 = 0;
    pub const OPENED: u32 = 1;
    pub const CREATED: u32 = 2;
    pub const OVERWRITTEN: u32 = 3;
}

/// The IOCTLs smbd knows (MS-FSCC 2.3 and MS-SMB2 2.2.31).
mod fsctl {
    pub const VALIDATE_NEGOTIATE_INFO: u32 = 0x0014_0204;
    pub const PIPE_TRANSCEIVE: u32 = 0x0011_c017;
    pub const DFS_GET_REFERRALS: u32 = 0x0006_0194;
    pub const DFS_GET_REFERRALS_EX: u32 = 0x0006_01b0;
    pub const GET_REPARSE_POINT: u32 = 0x0009_00a8;
    /// SMB2_0_IOCTL_IS_FSCTL.
    pub const IS_FSCTL: u32 = 0x0000_0001;
}

/// LOCK's flags.
mod lock {
    pub const SHARED: u32 = 0x01;
    pub const EXCLUSIVE: u32 = 0x02;
    pub const UNLOCK: u32 = 0x04;
    pub const FAIL_IMMEDIATELY: u32 = 0x10;
}

/// QUERY_DIRECTORY's flags.
mod listing {
    pub const RESTART_SCANS: u8 = 0x01;
    pub const RETURN_SINGLE_ENTRY: u8 = 0x02;
    pub const REOPEN: u8 = 0x10;
}

/// QUERY_INFO's and SET_INFO's kinds.
mod kind {
    pub const FILE: u8 = 1;
    pub const FILESYSTEM: u8 = 2;
    pub const SECURITY: u8 = 3;
    pub const QUOTA: u8 = 4;
}

/// The most locks one LOCK takes or lets go of.
const LOCKS_MAX: usize = 64;
/// The most create contexts one CREATE reads.
const CONTEXTS_MAX: usize = 32;

/// What a CREATE asks.
struct Asked<'a> {
    /// The name, within the share.
    name: &'a str,
    /// The access asked for.
    desired: u32,
    /// The attributes of a file it makes.
    attributes: u32,
    /// What to do whether or not the name is there.
    disposition: u32,
    /// Its options.
    options: u32,
}

impl Worker {
    /// CREATE (MS-SMB2 3.3.5.9): opens or makes a file or folder, or
    /// opens the srvsvc pipe on IPC$.
    pub(crate) fn create(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let desired = wire::u32_at(message, HEADER + 24)?;
        let attributes = wire::u32_at(message, HEADER + 28)?;
        let disposition = wire::u32_at(message, HEADER + 36)?;
        let options = wire::u32_at(message, HEADER + 40)?;
        let name_offset = usize::from(wire::u16_at(message, HEADER + 44)?);
        let name_length = usize::from(wire::u16_at(message, HEADER + 46)?);
        let contexts_offset = wire::u32_at(message, HEADER + 48)? as usize;
        let contexts_length = wire::u32_at(message, HEADER + 52)? as usize;
        let name = if name_length == 0 {
            String::new()
        } else {
            wire::utf16(wire::slice_at(message, name_offset, name_length)?)?
        };
        let contexts = if contexts_length == 0 {
            Vec::new()
        } else {
            context_names(wire::slice_at(message, contexts_offset, contexts_length)?)?
        };
        if disposition > disposition::OVERWRITE_IF
            || options & option::DIRECTORY_FILE != 0 && options & option::NON_DIRECTORY_FILE != 0
        {
            return Err(status::INVALID_PARAMETER);
        }
        if options & option::OPEN_BY_FILE_ID != 0 {
            return Err(status::NOT_SUPPORTED);
        }
        // A previous version of a file: smbd keeps none.
        if contexts.iter().any(|name| name == b"TWrp") {
            return Err(status::OBJECT_NAME_NOT_FOUND);
        }
        if self.opens.all.len() >= OPENS_MAX {
            return Err(status::TOO_MANY_OPENED_FILES);
        }
        let tree = self.tree(chain)?.clone();
        let Some(root) = tree.root.clone() else {
            return self.open_pipe(&name, desired, chain);
        };
        let asked = Asked { name: &name, desired, attributes, disposition, options };
        let (id, made, meta, maximal) = self.open_file(&root, &tree, &asked, chain.tree)?;
        chain.file = Some(id);
        let path = &self.opens.all[&id].path;
        let stat = Stat::of(&meta, file_name(path));
        let mut answers: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();
        if contexts.iter().any(|name| name == b"MxAc") {
            let mut data = Out::new();
            data.u32(status::SUCCESS).u32(maximal);
            answers.push((b"MxAc", data.0));
        }
        if contexts.iter().any(|name| name == b"QFid") {
            let mut data = Out::new();
            data.u64(stat.inode).u64(stat.device).zeros(16);
            answers.push((b"QFid", data.0));
        }
        Ok(Reply::ok(create_body(made, &stat, id, &contexts_out(&answers))))
    }

    /// Opens or makes the file or folder `asked` names in the share at
    /// `root`: its open's number, what was done, what it is, and the
    /// most the person may do to it.
    fn open_file(
        &mut self,
        root: &Path,
        tree: &Tree,
        asked: &Asked,
        tree_id: u32,
    ) -> Result<(u64, u32, fs::Metadata, u32), Status> {
        let path = share::resolve(root, asked.name, false)?;
        let found = match fs::metadata(&path) {
            Ok(meta) => Some(meta),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(status_of(&err)),
        };
        let want_folder = asked.options & option::DIRECTORY_FILE != 0;
        let delete_on_close = asked.options & option::DELETE_ON_CLOSE != 0;
        let overwrite = matches!(
            asked.disposition,
            disposition::SUPERSEDE | disposition::OVERWRITE | disposition::OVERWRITE_IF
        );
        let made = match &found {
            Some(meta) => {
                if asked.disposition == disposition::CREATE {
                    return Err(status::OBJECT_NAME_COLLISION);
                }
                if meta.is_dir() {
                    if asked.options & option::NON_DIRECTORY_FILE != 0 {
                        return Err(status::FILE_IS_A_DIRECTORY);
                    }
                    if overwrite {
                        return Err(status::INVALID_PARAMETER);
                    }
                } else if want_folder {
                    return Err(status::NOT_A_DIRECTORY);
                } else if !meta.is_file() {
                    // A device, named pipe or socket: not served.
                    return Err(status::ACCESS_DENIED);
                }
                match asked.disposition {
                    disposition::SUPERSEDE => action::SUPERSEDED,
                    disposition::OVERWRITE | disposition::OVERWRITE_IF => action::OVERWRITTEN,
                    _ => action::OPENED,
                }
            }
            None if matches!(asked.disposition, disposition::OPEN | disposition::OVERWRITE) => {
                return Err(status::OBJECT_NAME_NOT_FOUND);
            }
            None => action::CREATED,
        };
        let is_root = path == root;
        let maximum = asked.desired & access::MAXIMUM_ALLOWED != 0;
        let mut granted = access::specific(asked.desired) & !access::MAXIMUM_ALLOWED;
        if tree.read_only
            && (made != action::OPENED || delete_on_close || granted & access::CHANGES != 0)
        {
            return Err(status::ACCESS_DENIED);
        }
        let parent_writable =
            !is_root && path.parent().is_some_and(|parent| sys::may(parent, libc::W_OK));
        if granted & access::DELETE != 0 && !parent_writable {
            // Asked for by name, it is refused; within GENERIC_ALL, it is
            // left out.
            if asked.desired & access::DELETE != 0 || delete_on_close {
                return Err(status::ACCESS_DENIED);
            }
            granted &= !access::DELETE;
        }
        match made {
            action::CREATED if want_folder => {
                fs::create_dir(&path).map_err(|err| status_of(&err))?;
            }
            action::CREATED => {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|err| status_of(&err))?;
            }
            action::SUPERSEDED | action::OVERWRITTEN => {
                OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&path)
                    .map_err(|err| status_of(&err))?;
            }
            _ => {}
        }
        let meta = fs::metadata(&path).map_err(|err| status_of(&err))?;
        let folder = meta.is_dir();
        let maximal = maximal_access(&path, tree.read_only, parent_writable);
        if maximum {
            granted |= maximal;
        }
        let read = granted & (access::READ_DATA | access::EXECUTE) != 0;
        let write = granted & (access::WRITE_DATA | access::APPEND_DATA) != 0;
        let file = if folder {
            if read && !sys::may(&path, libc::R_OK) || write && !sys::may(&path, libc::W_OK) {
                return Err(status::ACCESS_DENIED);
            }
            None
        } else if read || write {
            let file = OpenOptions::new()
                .read(read)
                .write(write)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
                .map_err(|err| status_of(&err))?;
            if !file.metadata().is_ok_and(|meta| meta.is_file()) {
                return Err(status::ACCESS_DENIED);
            }
            Some(file)
        } else {
            None
        };
        if delete_on_close {
            if is_root {
                return Err(status::ACCESS_DENIED);
            }
            if !folder && meta.mode() & 0o200 == 0 {
                return Err(status::CANNOT_DELETE);
            }
        }
        if made == action::CREATED && !folder && asked.attributes & attribute::READONLY != 0 {
            let mode = meta.mode() & 0o7777 & !0o222;
            fs::set_permissions(&path, Permissions::from_mode(mode))
                .map_err(|err| status_of(&err))?;
        }
        let open = Open {
            tree: tree_id,
            name: share::share_name(root, &path),
            path,
            kind: if folder { Kind::Directory } else { Kind::File },
            file,
            access: granted,
            delete_on_close,
            position: 0,
            listing: None,
        };
        let id = self.opens.add(open);
        Ok((id, made, meta, maximal))
    }

    /// Opens the srvsvc pipe, the one IPC$ has.
    fn open_pipe(&mut self, name: &str, desired: u32, chain: &mut Chain) -> Result<Reply, Status> {
        if !name.trim_start_matches('\\').eq_ignore_ascii_case("srvsvc") {
            return Err(status::OBJECT_NAME_NOT_FOUND);
        }
        let open = Open {
            tree: chain.tree,
            path: PathBuf::new(),
            name: "\\srvsvc".into(),
            kind: Kind::Pipe(Box::default()),
            file: None,
            access: access::specific(desired) & !access::MAXIMUM_ALLOWED,
            delete_on_close: false,
            position: 0,
            listing: None,
        };
        let id = self.opens.add(open);
        chain.file = Some(id);
        let stat = Stat { allocation: 4096, attributes: attribute::NORMAL, ..Stat::default() };
        Ok(Reply::ok(create_body(action::OPENED, &stat, id, &[])))
    }

    /// CLOSE (MS-SMB2 3.3.5.10).
    pub(crate) fn close(
        &mut self,
        message: &[u8],
        chain: &mut Chain,
        extra: &mut Vec<Vec<u8>>,
    ) -> Result<Reply, Status> {
        let flags = wire::u16_at(message, HEADER + 2)?;
        let id = self.file_id(message, HEADER + 8, chain)?;
        let open = self.close_open(id, extra).ok_or(status::FILE_CLOSED)?;
        // SMB2_CLOSE_FLAG_POSTQUERY_ATTRIB: what the file is now.
        let meta = if flags & 1 != 0 && !matches!(open.kind, Kind::Pipe(_)) {
            fs::metadata(&open.path).ok()
        } else {
            None
        };
        let mut out = Out::new();
        out.u16(60);
        match meta {
            Some(meta) => {
                let stat = Stat::of(&meta, file_name(&open.path));
                out.u16(1).u32(0);
                out.u64(stat.creation).u64(stat.access).u64(stat.write).u64(stat.change);
                out.u64(stat.allocation).u64(stat.size).u32(stat.attributes);
            }
            None => {
                out.u16(0).u32(0).zeros(52);
            }
        }
        Ok(Reply::ok(out.0))
    }

    /// FLUSH (MS-SMB2 3.3.5.11): the file's data, to its disk.
    pub(crate) fn flush(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let id = self.file_id(message, HEADER + 8, chain)?;
        let open = self.open_mut(id)?;
        if matches!(open.kind, Kind::File) {
            if open.access & (access::WRITE_DATA | access::APPEND_DATA) == 0 {
                return Err(status::ACCESS_DENIED);
            }
            if let Some(file) = &open.file {
                file.sync_all().map_err(|err| status_of(&err))?;
            }
        }
        Ok(Reply::empty())
    }

    /// READ (MS-SMB2 3.3.5.12).
    pub(crate) fn read(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let length = wire::u32_at(message, HEADER + 4)?;
        let offset = wire::u64_at(message, HEADER + 8)?;
        let id = self.file_id(message, HEADER + 16, chain)?;
        let minimum = wire::u32_at(message, HEADER + 32)?;
        if length > wire::MAX_IO {
            return Err(status::INVALID_PARAMETER);
        }
        let open = self.opens.all.get_mut(&id).ok_or(status::FILE_CLOSED)?;
        // The response's body, its data at 80 from the header's start.
        let mut body = Vec::with_capacity(16 + length as usize);
        let mut out = Out::new();
        out.u16(17).u8((HEADER + 16) as u8).u8(0).u32(0).u32(0).u32(0);
        body.extend_from_slice(&out.0);
        let mut code = status::SUCCESS;
        match &mut open.kind {
            Kind::Pipe(pipe) => {
                let (data, more) = pipe.read(length as usize).ok_or(status::PIPE_EMPTY)?;
                if more {
                    code = status::BUFFER_OVERFLOW;
                }
                body.extend_from_slice(&data);
            }
            Kind::Directory => return Err(status::INVALID_DEVICE_REQUEST),
            Kind::File => {
                if open.access & (access::READ_DATA | access::EXECUTE) == 0 {
                    return Err(status::ACCESS_DENIED);
                }
                let file = open.file.as_ref().ok_or(status::ACCESS_DENIED)?;
                if !self.locks.0.is_empty() {
                    self.locks.allows(key_of(file)?, id, offset, u64::from(length), false)?;
                }
                body.resize(16 + length as usize, 0);
                let mut got = 0;
                while got < length as usize {
                    let at = offset.checked_add(got as u64).ok_or(status::INVALID_PARAMETER)?;
                    match file.read_at(&mut body[16 + got..], at) {
                        Ok(0) => break,
                        Ok(n) => got += n,
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                        Err(err) => return Err(status_of(&err)),
                    }
                }
                body.truncate(16 + got);
                if got == 0 && length > 0 || got < minimum as usize {
                    return Err(status::END_OF_FILE);
                }
            }
        }
        let count = (body.len() - 16) as u32;
        body[4..8].copy_from_slice(&count.to_le_bytes());
        Ok(Reply { status: code, body })
    }

    /// WRITE (MS-SMB2 3.3.5.13).
    pub(crate) fn write(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let data_offset = usize::from(wire::u16_at(message, HEADER + 2)?);
        let length = wire::u32_at(message, HEADER + 4)?;
        let offset = wire::u64_at(message, HEADER + 8)?;
        let id = self.file_id(message, HEADER + 16, chain)?;
        if length > wire::MAX_IO {
            return Err(status::INVALID_PARAMETER);
        }
        let data = wire::slice_at(message, data_offset, length as usize)?;
        let read_only = self.tree(chain)?.read_only;
        let is_pipe = matches!(self.opens.all.get(&id).map(|open| &open.kind), Some(Kind::Pipe(_)));
        let server = is_pipe.then(|| self.catalog.server(&self.server_name));
        let open = self.opens.all.get_mut(&id).ok_or(status::FILE_CLOSED)?;
        match &mut open.kind {
            Kind::Pipe(pipe) => {
                if let Some(server) = &server {
                    pipe.write(data, server);
                }
            }
            Kind::Directory => return Err(status::INVALID_DEVICE_REQUEST),
            Kind::File => {
                if read_only || open.access & (access::WRITE_DATA | access::APPEND_DATA) == 0 {
                    return Err(status::ACCESS_DENIED);
                }
                let file = open.file.as_ref().ok_or(status::ACCESS_DENIED)?;
                // At the end for an open that may only append, and for
                // the offset of all ones.
                let at = if open.access & access::WRITE_DATA == 0 || offset == u64::MAX {
                    file.metadata().map_err(|err| status_of(&err))?.len()
                } else {
                    offset
                };
                if !self.locks.0.is_empty() {
                    self.locks.allows(key_of(file)?, id, at, u64::from(length), true)?;
                }
                file.write_all_at(data, at).map_err(|err| status_of(&err))?;
            }
        }
        let mut out = Out::new();
        out.u16(17).u16(0).u32(length).u32(0).u16(0).u16(0);
        Ok(Reply::ok(out.0))
    }

    /// LOCK (MS-SMB2 3.3.5.14): takes or lets go of locks on ranges of a
    /// file's bytes, all of a request's or none. A lock another open of
    /// the session holds is refused at once, as no other can let it go
    /// while the session waits.
    pub(crate) fn lock(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let count = usize::from(wire::u16_at(message, HEADER + 2)?);
        let id = self.file_id(message, HEADER + 8, chain)?;
        if count == 0 || count > LOCKS_MAX {
            return Err(status::INVALID_PARAMETER);
        }
        let mut ranges = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER + 24 + i * 24;
            ranges.push((
                wire::u64_at(message, at)?,
                wire::u64_at(message, at + 8)?,
                wire::u32_at(message, at + 16)?,
            ));
        }
        let open = self.opens.all.get(&id).ok_or(status::FILE_CLOSED)?;
        if !matches!(open.kind, Kind::File) {
            return Err(status::INVALID_PARAMETER);
        }
        let meta = match &open.file {
            Some(file) => file.metadata(),
            None => fs::metadata(&open.path),
        }
        .map_err(|err| status_of(&err))?;
        let file = (meta.dev(), meta.ino());
        if ranges[0].2 & lock::UNLOCK != 0 {
            for &(offset, length, flags) in &ranges {
                if flags != lock::UNLOCK {
                    return Err(status::INVALID_PARAMETER);
                }
                self.locks.release(file, id, offset, length)?;
            }
            return Ok(Reply::empty());
        }
        let mut taken = Vec::new();
        let mut result = Ok(());
        for &(offset, length, flags) in &ranges {
            let exclusive = match flags & !lock::FAIL_IMMEDIATELY {
                lock::SHARED => false,
                lock::EXCLUSIVE => true,
                _ => {
                    result = Err(status::INVALID_PARAMETER);
                    break;
                }
            };
            if offset.checked_add(length).is_none() {
                result = Err(status::INVALID_LOCK_RANGE);
                break;
            }
            if let Err(code) = self.locks.take(Lock { file, open: id, offset, length, exclusive }) {
                result = Err(code);
                break;
            }
            taken.push((offset, length));
        }
        if result.is_err() {
            for (offset, length) in taken {
                let _ = self.locks.release(file, id, offset, length);
            }
        }
        result.map(|()| Reply::empty())
    }

    /// IOCTL (MS-SMB2 3.3.5.15): the srvsvc pipe's transactions and
    /// VALIDATE_NEGOTIATE_INFO; no other.
    pub(crate) fn ioctl(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let code = wire::u32_at(message, HEADER + 4)?;
        let input_offset = wire::u32_at(message, HEADER + 24)? as usize;
        let input_count = wire::u32_at(message, HEADER + 28)? as usize;
        let max_output = wire::u32_at(message, HEADER + 44)? as usize;
        let ioctl_flags = wire::u32_at(message, HEADER + 48)?;
        let file_id = wire::slice_at(message, HEADER + 8, 16)?.to_vec();
        let input = if input_count == 0 {
            &[][..]
        } else {
            wire::slice_at(message, input_offset, input_count)?
        };
        let mut result = status::SUCCESS;
        let output = match code {
            fsctl::VALIDATE_NEGOTIATE_INFO => self.validate_negotiate(input)?,
            fsctl::PIPE_TRANSCEIVE => {
                let id = self.file_id(message, HEADER + 8, chain)?;
                let server = self.catalog.server(&self.server_name);
                let open = self.opens.all.get_mut(&id).ok_or(status::FILE_CLOSED)?;
                let Kind::Pipe(pipe) = &mut open.kind else {
                    return Err(status::INVALID_DEVICE_REQUEST);
                };
                pipe.write(input, &server);
                match pipe.read(max_output) {
                    Some((data, more)) => {
                        if more {
                            result = status::BUFFER_OVERFLOW;
                        }
                        data
                    }
                    None => Vec::new(),
                }
            }
            // No DFS: the shares are what they are.
            fsctl::DFS_GET_REFERRALS | fsctl::DFS_GET_REFERRALS_EX => {
                return Err(status::FS_DRIVER_REQUIRED);
            }
            fsctl::GET_REPARSE_POINT => {
                self.file_id(message, HEADER + 8, chain)?;
                return Err(status::NOT_A_REPARSE_POINT);
            }
            _ if ioctl_flags & fsctl::IS_FSCTL != 0 => return Err(status::NOT_SUPPORTED),
            _ => return Err(status::INVALID_DEVICE_REQUEST),
        };
        if output.len() > max_output {
            return Err(status::INVALID_PARAMETER);
        }
        let at = (HEADER + 48) as u32;
        let mut out = Out::new();
        out.u16(49).u16(0).u32(code).bytes(&file_id);
        out.u32(at).u32(0).u32(at).u32(output.len() as u32).u32(0).u32(0).bytes(&output);
        Ok(Reply { status: result, body: out.0 })
    }

    /// FSCTL_VALIDATE_NEGOTIATE_INFO (MS-SMB2 3.3.5.15.12): what the PC
    /// says it sent in NEGOTIATE, which must be what smbd read, or
    /// someone between them changed it, as to have the PC sign less.
    fn validate_negotiate(&mut self, input: &[u8]) -> Result<Vec<u8>, Status> {
        if self.negotiated.dialect == dialect::SMB_3_1_1 {
            // SMB 3.1.1's sign-in hash covers NEGOTIATE already.
            self.drop_connection = Some("VALIDATE_NEGOTIATE_INFO on SMB 3.1.1");
            return Err(status::ACCESS_DENIED);
        }
        let capabilities = wire::u32_at(input, 0)?;
        let guid = wire::slice_at(input, 4, 16)?;
        let mode = wire::u16_at(input, 20)?;
        let count = usize::from(wire::u16_at(input, 22)?);
        let dialects: Vec<u16> = wire::slice_at(input, 24, count * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        let n = &self.negotiated;
        if capabilities != n.client_capabilities
            || guid != n.client_guid
            || mode != n.client_security_mode
            || dialects != n.client_dialects
        {
            self.drop_connection = Some(
                "VALIDATE_NEGOTIATE_INFO does not match NEGOTIATE: something changed it on the way",
            );
            return Err(status::ACCESS_DENIED);
        }
        let mut out = Out::new();
        out.u32(n.server_capabilities).bytes(&n.server_guid).u16(SECURITY_MODE).u16(n.dialect);
        Ok(out.0)
    }

    /// QUERY_DIRECTORY (MS-SMB2 3.3.5.18): a folder's entries that match
    /// a pattern, as many as fit.
    pub(crate) fn query_directory(
        &mut self,
        message: &[u8],
        chain: &mut Chain,
    ) -> Result<Reply, Status> {
        let class = wire::u8_at(message, HEADER + 2)?;
        let flags = wire::u8_at(message, HEADER + 3)?;
        let id = self.file_id(message, HEADER + 8, chain)?;
        let name_offset = usize::from(wire::u16_at(message, HEADER + 24)?);
        let name_length = usize::from(wire::u16_at(message, HEADER + 26)?);
        let room = (wire::u32_at(message, HEADER + 28)? as usize).min(wire::MAX_IO as usize);
        let pattern = if name_length == 0 {
            None
        } else {
            Some(wire::utf16(wire::slice_at(message, name_offset, name_length)?)?)
        };
        let root = self.tree(chain)?.root.clone().ok_or(status::INVALID_PARAMETER)?;
        let open = self.opens.all.get_mut(&id).ok_or(status::FILE_CLOSED)?;
        if !matches!(open.kind, Kind::Directory) {
            return Err(status::INVALID_PARAMETER);
        }
        if open.access & access::READ_DATA == 0 {
            return Err(status::ACCESS_DENIED);
        }
        let restart =
            flags & (listing::RESTART_SCANS | listing::REOPEN) != 0 || open.listing.is_none();
        if restart {
            let pattern = pattern.unwrap_or_else(|| "*".into());
            let names = list(&root, &open.path, &pattern).map_err(|err| status_of(&err))?;
            open.listing = Some(Listing { names, at: 0 });
        }
        let folder = open.path.clone();
        let listing = open.listing.as_mut().ok_or(status::INVALID_PARAMETER)?;
        let mut out: Vec<u8> = Vec::new();
        let mut last: Option<usize> = None;
        while let Some(name) = listing.names.get(listing.at) {
            let path = match name.as_str() {
                "." => folder.clone(),
                ".." if folder == root => folder.clone(),
                ".." => folder.parent().map_or_else(|| folder.clone(), Path::to_path_buf),
                _ => folder.join(name),
            };
            let Ok(meta) = fs::metadata(&path) else {
                // Gone since the listing was made.
                listing.at += 1;
                continue;
            };
            let entry = info::entry(class, &Stat::of(&meta, name), name)?;
            let start = out.len().next_multiple_of(8);
            if start + entry.len() > room {
                break;
            }
            out.resize(start, 0);
            if let Some(previous) = last {
                let gap = (start - previous) as u32;
                out[previous..previous + 4].copy_from_slice(&gap.to_le_bytes());
            }
            out.extend_from_slice(&entry);
            last = Some(start);
            listing.at += 1;
            if flags & listing::RETURN_SINGLE_ENTRY != 0 {
                break;
            }
        }
        if out.is_empty() {
            if listing.at < listing.names.len() {
                // Not even one entry fits.
                return Err(status::INFO_LENGTH_MISMATCH);
            }
            return Err(if restart { status::NO_SUCH_FILE } else { status::NO_MORE_FILES });
        }
        Ok(answer(&out))
    }

    /// QUERY_INFO (MS-SMB2 3.3.5.20): what a file, its filesystem, or
    /// its security descriptor is.
    pub(crate) fn query_info(
        &mut self,
        message: &[u8],
        chain: &mut Chain,
    ) -> Result<Reply, Status> {
        let kind = wire::u8_at(message, HEADER + 2)?;
        let class = wire::u8_at(message, HEADER + 3)?;
        let room = wire::u32_at(message, HEADER + 4)? as usize;
        let additional = wire::u32_at(message, HEADER + 16)?;
        let id = self.file_id(message, HEADER + 24, chain)?;
        let label = self.tree(chain)?.name.clone();
        let open = self.opens.all.get(&id).ok_or(status::FILE_CLOSED)?;
        let pipe = matches!(open.kind, Kind::Pipe(_));
        let meta = || -> Result<fs::Metadata, Status> {
            match &open.file {
                Some(file) => file.metadata(),
                None => fs::metadata(&open.path),
            }
            .map_err(|err| status_of(&err))
        };
        match kind {
            kind::FILE => {
                let stat = if pipe {
                    Stat { allocation: 4096, attributes: attribute::NORMAL, ..Stat::default() }
                } else {
                    Stat::of(&meta()?, file_name(&open.path))
                };
                let opened = Opened {
                    name: &open.name,
                    access: open.access,
                    position: open.position,
                    delete_pending: open.delete_on_close,
                };
                fit(info::file(class, &stat, &opened)?, room, info::may_be_cut(class))
            }
            kind::FILESYSTEM if !pipe => {
                let volume = sys::volume(&open.path).map_err(|err| status_of(&err))?;
                let data = info::volume(class, &volume, &label)?;
                fit(data, room, matches!(class, fs_class::VOLUME | fs_class::ATTRIBUTE))
            }
            kind::SECURITY if !pipe => {
                if open.access & access::READ_CONTROL == 0 {
                    return Err(status::ACCESS_DENIED);
                }
                let meta = meta()?;
                let descriptor = info::descriptor(meta.uid(), meta.gid(), meta.mode(), additional);
                if descriptor.len() > room {
                    // What it takes, for the PC to ask again.
                    let needed = (descriptor.len() as u32).to_le_bytes();
                    return Ok(Reply {
                        status: status::BUFFER_TOO_SMALL,
                        body: wire::error_body_with(&needed),
                    });
                }
                Ok(answer(&descriptor))
            }
            kind::QUOTA => Err(status::NOT_SUPPORTED),
            _ => Err(status::INVALID_PARAMETER),
        }
    }

    /// SET_INFO (MS-SMB2 3.3.5.21): changes a file's times, attributes,
    /// size, name or deletion. Security descriptors are set with the
    /// Security tab's ACLs, which smbd does not yet take.
    pub(crate) fn set_info(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let kind = wire::u8_at(message, HEADER + 2)?;
        let class = wire::u8_at(message, HEADER + 3)?;
        let length = wire::u32_at(message, HEADER + 4)? as usize;
        let offset = usize::from(wire::u16_at(message, HEADER + 8)?);
        let id = self.file_id(message, HEADER + 16, chain)?;
        let buffer = wire::slice_at(message, offset, length)?;
        let tree = self.tree(chain)?.clone();
        match kind {
            kind::FILE => self.set_file(id, class, buffer, &tree)?,
            kind::SECURITY => return Err(status::ACCESS_DENIED),
            _ => return Err(status::NOT_SUPPORTED),
        }
        let mut out = Out::new();
        out.u16(2);
        Ok(Reply::ok(out.0))
    }

    /// Sets file information class `class` of open `id` from `buffer`.
    fn set_file(&mut self, id: u64, class: u8, buffer: &[u8], tree: &Tree) -> Result<(), Status> {
        let open = self.opens.all.get_mut(&id).ok_or(status::FILE_CLOSED)?;
        if matches!(open.kind, Kind::Pipe(_)) {
            // A pipe's own states: nothing for smbd's to keep.
            return Ok(());
        }
        match class {
            class::POSITION => {
                open.position = wire::u64_at(buffer, 0)?;
                return Ok(());
            }
            class::MODE => return Ok(()),
            class::FULL_EA => return Err(status::EAS_NOT_SUPPORTED),
            _ => {}
        }
        let root = tree.root.as_deref().ok_or(status::INVALID_PARAMETER)?;
        if tree.read_only {
            return Err(status::ACCESS_DENIED);
        }
        match class {
            class::BASIC => {
                if open.access & access::WRITE_ATTRIBUTES == 0 {
                    return Err(status::ACCESS_DENIED);
                }
                // 0 leaves a time as it is, and so do -1 and -2, which
                // stop and start its keeping by the handle.
                let time = |at: usize| -> Result<Option<(i64, i64)>, Status> {
                    let ticks = wire::u64_at(buffer, at)?;
                    Ok((ticks != 0 && ticks < u64::MAX - 1).then(|| wire::unix_time(ticks)))
                };
                let (accessed, written) = (time(8)?, time(16)?);
                let attributes = wire::u32_at(buffer, 32)?;
                if accessed.is_some() || written.is_some() {
                    sys::set_times(&open.path, accessed, written).map_err(|err| status_of(&err))?;
                }
                if attributes != 0 && matches!(open.kind, Kind::File) {
                    let mode =
                        fs::metadata(&open.path).map_err(|err| status_of(&err))?.mode() & 0o7777;
                    let new = if attributes & attribute::READONLY != 0 {
                        mode & !0o222
                    } else if mode & 0o200 == 0 {
                        mode | 0o200
                    } else {
                        mode
                    };
                    if new != mode {
                        fs::set_permissions(&open.path, Permissions::from_mode(new))
                            .map_err(|err| status_of(&err))?;
                    }
                }
                Ok(())
            }
            class::END_OF_FILE | class::ALLOCATION => {
                if open.access & access::WRITE_DATA == 0 {
                    return Err(status::ACCESS_DENIED);
                }
                let size = wire::u64_at(buffer, 0)?;
                let file = open.file.as_ref().ok_or(status::INVALID_PARAMETER)?;
                let now = file.metadata().map_err(|err| status_of(&err))?.len();
                // An allocation only cuts a file shorter than it.
                if class == class::END_OF_FILE || size < now {
                    file.set_len(size).map_err(|err| status_of(&err))?;
                }
                Ok(())
            }
            class::DISPOSITION | class::DISPOSITION_EX => {
                if open.access & access::DELETE == 0 {
                    return Err(status::ACCESS_DENIED);
                }
                let (delete, ignore_read_only) = if class == class::DISPOSITION {
                    (wire::u8_at(buffer, 0)? != 0, false)
                } else {
                    let flags = wire::u32_at(buffer, 0)?;
                    (flags & 0x1 != 0, flags & 0x10 != 0)
                };
                if delete {
                    if open.path == root {
                        return Err(status::ACCESS_DENIED);
                    }
                    match open.kind {
                        Kind::Directory => {
                            let mut entries =
                                fs::read_dir(&open.path).map_err(|err| status_of(&err))?;
                            if entries.next().is_some() {
                                return Err(status::DIRECTORY_NOT_EMPTY);
                            }
                        }
                        _ => {
                            let mode =
                                fs::metadata(&open.path).map_err(|err| status_of(&err))?.mode();
                            if !ignore_read_only && mode & 0o200 == 0 {
                                return Err(status::CANNOT_DELETE);
                            }
                        }
                    }
                }
                open.delete_on_close = delete;
                Ok(())
            }
            class::RENAME => self.rename(id, buffer, root, false),
            class::LINK => self.rename(id, buffer, root, true),
            _ => Err(status::INVALID_INFO_CLASS),
        }
    }

    /// Renames open `id` (or, with `link`, gives it another name) to the
    /// name in `buffer`, a FileRenameInformation (MS-FSCC 2.4.37.2).
    fn rename(&mut self, id: u64, buffer: &[u8], root: &Path, link: bool) -> Result<(), Status> {
        let replace = wire::u8_at(buffer, 0)? != 0;
        let length = wire::u32_at(buffer, 16)? as usize;
        let name = wire::utf16(wire::slice_at(buffer, 20, length)?)?;
        let open = self.opens.all.get(&id).ok_or(status::FILE_CLOSED)?;
        if !link && open.access & access::DELETE == 0 {
            return Err(status::ACCESS_DENIED);
        }
        if open.path == root {
            return Err(status::ACCESS_DENIED);
        }
        let from = open.path.clone();
        let found = share::resolve(root, &name, false)?;
        // A new case of the same name is taken as given.
        let target = if found == from { share::resolve(root, &name, true)? } else { found };
        if target == from {
            return Ok(());
        }
        if let Ok(meta) = fs::symlink_metadata(&target) {
            if !replace {
                return Err(status::OBJECT_NAME_COLLISION);
            }
            if meta.is_dir() || self.opens.all.values().any(|other| other.path == target) {
                return Err(status::ACCESS_DENIED);
            }
            if link {
                fs::remove_file(&target).map_err(|err| status_of(&err))?;
            }
        }
        if link {
            return fs::hard_link(&from, &target).map_err(|err| status_of(&err));
        }
        fs::rename(&from, &target).map_err(|err| status_of(&err))?;
        for open in self.opens.all.values_mut() {
            let moved = match open.path.strip_prefix(&from) {
                Ok(rest) if rest.as_os_str().is_empty() => target.clone(),
                Ok(rest) => target.join(rest),
                Err(_) => continue,
            };
            open.name = share::share_name(root, &moved);
            open.path = moved;
        }
        Ok(())
    }
}

/// CREATE's response body (MS-SMB2 2.2.14), with `contexts` after it.
fn create_body(made: u32, stat: &Stat, id: u64, contexts: &[u8]) -> Vec<u8> {
    let mut out = Out::new();
    out.u16(89).u8(0).u8(0).u32(made);
    out.u64(stat.creation).u64(stat.access).u64(stat.write).u64(stat.change);
    out.u64(stat.allocation).u64(stat.size).u32(stat.attributes).u32(0);
    out.u64(id).u64(id);
    if contexts.is_empty() {
        out.u32(0).u32(0);
    } else {
        out.u32((HEADER + 88) as u32).u32(contexts.len() as u32).bytes(contexts);
    }
    out.0
}

/// The names of the create contexts in `bytes` (MS-SMB2 2.2.13.2).
fn context_names(bytes: &[u8]) -> Result<Vec<Vec<u8>>, Status> {
    let mut names = Vec::new();
    let mut at = 0usize;
    loop {
        let context = bytes.get(at..).ok_or(status::INVALID_PARAMETER)?;
        let next = wire::u32_at(context, 0)? as usize;
        let name_offset = usize::from(wire::u16_at(context, 4)?);
        let name_length = usize::from(wire::u16_at(context, 6)?);
        names.push(wire::slice_at(context, name_offset, name_length)?.to_vec());
        if next == 0 {
            return Ok(names);
        }
        if next < 16 || !next.is_multiple_of(8) || names.len() >= CONTEXTS_MAX {
            return Err(status::INVALID_PARAMETER);
        }
        at += next;
    }
}

/// The create contexts of a response, each a name and its data.
fn contexts_out(answers: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = Out::new();
    let mut previous: Option<usize> = None;
    for (name, data) in answers {
        out.align(8);
        let start = out.len();
        if let Some(previous) = previous {
            out.set_u32(previous, (start - previous) as u32);
        }
        out.u32(0).u16(16).u16(4).u16(0).u16(24).u32(data.len() as u32);
        out.bytes(&name[..]).zeros(4).bytes(data);
        previous = Some(start);
    }
    out.0
}

/// QUERY_INFO's and QUERY_DIRECTORY's response body around `data`.
fn answer(data: &[u8]) -> Reply {
    let mut out = Out::new();
    out.u16(9).u16((HEADER + 8) as u16).u32(data.len() as u32).bytes(data);
    Reply::ok(out.0)
}

/// `data` as an answer for a buffer of `room` bytes: cut short with
/// STATUS_BUFFER_OVERFLOW if `may_cut`, else refused.
fn fit(mut data: Vec<u8>, room: usize, may_cut: bool) -> Result<Reply, Status> {
    if data.len() <= room {
        return Ok(answer(&data));
    }
    if !may_cut {
        return Err(status::INFO_LENGTH_MISMATCH);
    }
    data.truncate(room);
    let mut reply = answer(&data);
    reply.status = status::BUFFER_OVERFLOW;
    Ok(reply)
}

/// The last part of `path`, for the attributes its name gives.
fn file_name(path: &Path) -> &str {
    path.file_name().and_then(|name| name.to_str()).unwrap_or("")
}

/// The file `file` is, by its device and number, for its locks.
fn key_of(file: &fs::File) -> Result<(u64, u64), Status> {
    let meta = file.metadata().map_err(|err| status_of(&err))?;
    Ok((meta.dev(), meta.ino()))
}

/// Whether Windows can hold `name`: names with characters it keeps out
/// of names are left out of listings.
fn windows_name(name: &str) -> bool {
    !name.chars().any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
}

/// The names in `folder` that match `pattern`, `.` and `..` first, of
/// the share at `root`: those Windows can hold, and no link out of it.
fn list(root: &Path, folder: &Path, pattern: &str) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = [".", ".."]
        .into_iter()
        .filter(|dot| info::matches(pattern, dot))
        .map(String::from)
        .collect();
    for entry in fs::read_dir(folder)? {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else { continue };
        if !windows_name(&name) || !info::matches(pattern, &name) {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_symlink())
            && share::within(root, &entry.path()).is_err()
        {
            continue;
        }
        names.push(name);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_go_out_and_their_names_come_back() {
        let out = contexts_out(&[(b"MxAc", vec![0; 8]), (b"QFid", vec![1; 32])]);
        assert_eq!(out.len(), 32 + 24 + 32);
        assert_eq!(wire::u32_at(&out, 0), Ok(32));
        assert_eq!(context_names(&out), Ok(vec![b"MxAc".to_vec(), b"QFid".to_vec()]));
        let mut looped = out.clone();
        looped[0] = 8;
        assert_eq!(context_names(&looped), Err(status::INVALID_PARAMETER));
        assert_eq!(context_names(&out[..20]), Err(status::INVALID_PARAMETER));
    }

    #[test]
    fn answers_too_long_are_cut_or_refused() {
        let reply = fit(vec![1; 10], 4, true).unwrap();
        assert_eq!((reply.status, reply.body.len()), (status::BUFFER_OVERFLOW, 8 + 4));
        assert_eq!(fit(vec![1; 10], 4, false).unwrap_err(), status::INFO_LENGTH_MISMATCH);
        assert_eq!(fit(vec![1; 10], 10, false).unwrap().status, status::SUCCESS);
    }

    #[test]
    fn listings_leave_out_what_windows_cannot_hold() {
        let dir = std::env::temp_dir().join(format!("smbd-list-{}", std::process::id()));
        let root = dir.join("share");
        fs::create_dir_all(&root).unwrap();
        for name in ["a.txt", "B.TXT", "what?", "c.md"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        std::os::unix::fs::symlink("/", root.join("out")).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let mut names = list(&root, &root, "*").unwrap();
        names.sort();
        assert_eq!(names, [".", "..", "B.TXT", "a.txt", "c.md"]);
        let mut names = list(&root, &root, "*.txt").unwrap();
        names.sort();
        assert_eq!(names, ["B.TXT", "a.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
