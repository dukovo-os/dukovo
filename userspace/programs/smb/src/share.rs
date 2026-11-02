// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A share: an SMB session, signed or encrypted as the server asks, and
//! the share's tree, with the calls the filesystem makes of it.
//!
//! smb2 has a call for each whole-file and path operation (a listing, a
//! stat, a rename); a filesystem also reads and writes at an offset
//! through a handle it keeps open, and cuts a file short, which smb2
//! keeps to itself. Those go out here as SMB2's own CREATE, READ, WRITE,
//! FLUSH, SET_INFO and CLOSE, through the tree's connection, which signs
//! or encrypts them as it does smb2's. Each carries at most 64 KiB, which
//! one credit pays for (the filesystem asks for no more at a time).
//!
//! Everything runs on a tokio runtime of one thread, which the
//! filesystem's single thread waits on for each call.

use std::time::{Duration, SystemTime};

use libc::c_int;
use smb2::msg::close::CloseRequest;
use smb2::msg::create::{
    CreateDisposition, CreateRequest, CreateResponse, ImpersonationLevel, ShareAccess,
};
use smb2::msg::flush::FlushRequest;
use smb2::msg::query_info::InfoType;
use smb2::msg::read::{ReadRequest, ReadResponse, SMB2_CHANNEL_NONE};
use smb2::msg::set_info::SetInfoRequest;
use smb2::msg::write::{WriteRequest, WriteResponse};
use smb2::pack::{FileTime, ReadCursor, Unpack};
use smb2::types::flags::{FileAccessMask, FileAttributes};
use smb2::types::status::NtStatus;
use smb2::types::{Command, FileId, OplockLevel};
use smb2::{ClientConfig, DirectoryEntry, ErrorKind, FileInfo, FileTimes, SmbClient, Tree};
use tokio::runtime::Runtime;

/// The most bytes one READ or WRITE carries: what one credit pays for.
pub const MAX_IO: u32 = 64 * 1024;

/// SET_INFO's `FileEndOfFileInformation` (MS-FSCC 2.4.14).
const FILE_END_OF_FILE_INFORMATION: u8 = 20;

/// STATUS_NOT_SAME_DEVICE, which smb2 does not name: a rename to
/// another volume of the server's.
const NOT_SAME_DEVICE: NtStatus = NtStatus(0xC000_00D4);

/// STATUS_NAME_TOO_LONG.
const NAME_TOO_LONG: NtStatus = NtStatus(0xC000_0106);

/// CREATE's option for anything but a directory.
const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;

/// Where and as whom to connect.
pub struct Account {
    /// The server, as `host:port`.
    pub addr: String,
    pub user: String,
    pub password: String,
    pub domain: String,
}

/// A share, connected.
pub struct Share {
    rt: Runtime,
    client: SmbClient,
    tree: Tree,
}

/// What an error of smb2 is as an errno.
pub fn errno(err: &smb2::Error) -> c_int {
    match err.status() {
        Some(NtStatus::DIRECTORY_NOT_EMPTY) => return libc::ENOTEMPTY,
        Some(NtStatus::OBJECT_NAME_INVALID) => return libc::EINVAL,
        Some(NOT_SAME_DEVICE) => return libc::EXDEV,
        Some(NAME_TOO_LONG) => return libc::ENAMETOOLONG,
        Some(NtStatus::DELETE_PENDING) => return libc::ENOENT,
        Some(NtStatus::CANNOT_DELETE) => return libc::EACCES,
        _ => {}
    }
    match err.kind() {
        ErrorKind::NotFound => libc::ENOENT,
        ErrorKind::AlreadyExists => libc::EEXIST,
        ErrorKind::AccessDenied | ErrorKind::AuthRequired => libc::EACCES,
        ErrorKind::SharingViolation => libc::EBUSY,
        ErrorKind::IsADirectory => libc::EISDIR,
        ErrorKind::NotADirectory => libc::ENOTDIR,
        ErrorKind::DiskFull => libc::ENOSPC,
        ErrorKind::ConnectionLost | ErrorKind::SessionExpired => libc::ENOTCONN,
        ErrorKind::TimedOut => libc::ETIMEDOUT,
        ErrorKind::InvalidName => libc::EINVAL,
        ErrorKind::TooLarge => libc::EFBIG,
        _ => libc::EIO,
    }
}

/// A runtime of one thread, with sockets and timers.
pub fn runtime() -> std::io::Result<Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()
}

/// Signs in to the server `account` names.
pub fn sign_in(rt: &Runtime, account: &Account) -> smb2::Result<SmbClient> {
    let config = ClientConfig {
        addr: account.addr.clone(),
        timeout: Duration::from_secs(10),
        username: account.user.clone(),
        password: account.password.clone(),
        domain: account.domain.clone(),
        // A share that comes back after a server restarts or the network
        // drops is used again, as Linux's cifs does.
        auto_reconnect: true,
        // A path is the share's own: a DFS link is not followed off it.
        dfs_enabled: false,
        ..ClientConfig::default()
    };
    rt.block_on(SmbClient::connect(config))
}

impl Share {
    /// Signs in and connects to the share `name`.
    pub fn connect(account: &Account, name: &str) -> Result<Share, String> {
        let rt = runtime().map_err(|e| format!("no runtime: {e}"))?;
        let mut client = sign_in(&rt, account)
            .map_err(|e| format!("cannot sign in to {}: {e}", account.addr))?;
        let tree = rt
            .block_on(client.connect_share(name))
            .map_err(|e| format!("cannot connect to the share {name}: {e}"))?;
        if tree.is_dfs {
            return Err(format!("{name} is a DFS namespace, which smb does not mount yet"));
        }
        Ok(Share { rt, client, tree })
    }

    /// A path of the share as SMB2 names it: `\`s, and the characters it
    /// forbids in names mapped as macOS maps them (smb2's `encode_path`).
    fn wire(path: &str) -> String {
        smb2::encode_path(path)
    }

    pub fn stat(&mut self, path: &str) -> smb2::Result<FileInfo> {
        self.rt.block_on(self.client.stat(&mut self.tree, path))
    }

    pub fn list(&mut self, path: &str) -> smb2::Result<Vec<DirectoryEntry>> {
        self.rt.block_on(self.client.list_directory(&mut self.tree, path))
    }

    pub fn make_directory(&mut self, path: &str) -> smb2::Result<()> {
        self.rt.block_on(self.client.create_directory(&self.tree, path))
    }

    pub fn remove_file(&mut self, path: &str) -> smb2::Result<()> {
        self.rt.block_on(self.client.delete_file(&self.tree, path))
    }

    pub fn remove_directory(&mut self, path: &str) -> smb2::Result<()> {
        self.rt.block_on(self.client.delete_directory(&self.tree, path))
    }

    /// Renames `from` to `to`, replacing a file there, as rename(2) does.
    pub fn rename(&mut self, from: &str, to: &str) -> smb2::Result<()> {
        let options = smb2::RenameOptions::new().replace_if_exists(true);
        self.rt.block_on(self.client.rename_with(&self.tree, from, to, options))
    }

    /// Sets the times and attributes `times` names.
    pub fn set_times(&mut self, path: &str, times: FileTimes) -> smb2::Result<()> {
        self.rt.block_on(self.client.set_times(&self.tree, path, times))
    }

    /// The share's size and free space, in bytes, and its block size.
    pub fn space(&mut self) -> smb2::Result<(u64, u64, u32)> {
        let info = self.rt.block_on(self.client.fs_info(&mut self.tree))?;
        let block = info.bytes_per_sector.saturating_mul(info.sectors_per_unit).max(512);
        Ok((info.total_bytes, info.free_bytes, block))
    }

    /// Opens the file `path`, to read and to write if `write`, made if
    /// `disposition` says, and marked read-only if made and `read_only`,
    /// as Linux's cifs makes a file whose mode has no write bits: this
    /// handle writes it still. Returns its handle, size and attributes.
    pub fn open(
        &mut self,
        path: &str,
        write: bool,
        disposition: CreateDisposition,
        read_only: bool,
    ) -> smb2::Result<(FileId, u64, u32)> {
        let mut access = FileAccessMask::FILE_READ_DATA
            | FileAccessMask::FILE_READ_ATTRIBUTES
            | FileAccessMask::SYNCHRONIZE;
        if write {
            access |= FileAccessMask::FILE_WRITE_DATA | FileAccessMask::FILE_WRITE_ATTRIBUTES;
        }
        let req = CreateRequest {
            requested_oplock_level: OplockLevel::None,
            impersonation_level: ImpersonationLevel::Impersonation,
            desired_access: FileAccessMask::new(access),
            file_attributes: if read_only {
                FileAttributes::READONLY
            } else {
                FileAttributes::NORMAL
            },
            share_access: ShareAccess(
                ShareAccess::FILE_SHARE_READ
                    | ShareAccess::FILE_SHARE_WRITE
                    | ShareAccess::FILE_SHARE_DELETE,
            ),
            create_disposition: disposition,
            create_options: FILE_NON_DIRECTORY_FILE,
            name: Share::wire(path),
            create_contexts: vec![],
        };
        let frame = self.call(Command::Create, &req)?;
        let resp = CreateResponse::unpack(&mut ReadCursor::new(&frame))?;
        Ok((resp.file_id, resp.end_of_file, resp.file_attributes))
    }

    /// Reads at most [`MAX_IO`] bytes of `file` at `offset`: fewer at its
    /// end, and none past it.
    pub fn read(&mut self, file: FileId, offset: u64, len: u32) -> smb2::Result<Vec<u8>> {
        let req = ReadRequest {
            padding: 0x50,
            flags: 0,
            length: len.min(MAX_IO),
            offset,
            file_id: file,
            minimum_count: 0,
            channel: SMB2_CHANNEL_NONE,
            remaining_bytes: 0,
            read_channel_info: vec![],
        };
        match self.call(Command::Read, &req) {
            Ok(body) => Ok(ReadResponse::unpack(&mut ReadCursor::new(&body))?.data),
            Err(err) if err.status() == Some(NtStatus::END_OF_FILE) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    /// Writes `data`, [`MAX_IO`] bytes at most, to `file` at `offset`:
    /// returns how many were written.
    pub fn write(&mut self, file: FileId, offset: u64, data: &[u8]) -> smb2::Result<u32> {
        let req = WriteRequest {
            data_offset: 0x70,
            offset,
            file_id: file,
            channel: 0,
            remaining_bytes: 0,
            write_channel_info_offset: 0,
            write_channel_info_length: 0,
            flags: 0,
            data: data[..data.len().min(MAX_IO as usize)].to_vec(),
        };
        let body = self.call(Command::Write, &req)?;
        Ok(WriteResponse::unpack(&mut ReadCursor::new(&body))?.count)
    }

    /// Makes `file` `size` bytes long, cutting it short or adding zeros.
    pub fn set_size(&mut self, file: FileId, size: u64) -> smb2::Result<()> {
        let req = SetInfoRequest {
            info_type: InfoType::File,
            file_info_class: FILE_END_OF_FILE_INFORMATION,
            additional_information: 0,
            file_id: file,
            buffer: size.to_le_bytes().to_vec(),
        };
        self.call(Command::SetInfo, &req).map(drop)
    }

    /// Asks the server to write what it holds of `file` to its disk.
    pub fn flush(&mut self, file: FileId) -> smb2::Result<()> {
        self.call(Command::Flush, &FlushRequest { file_id: file }).map(drop)
    }

    pub fn close(&mut self, file: FileId) -> smb2::Result<()> {
        self.call(Command::Close, &CloseRequest { flags: 0, file_id: file }).map(drop)
    }

    /// Sends a request on the share's tree, through the connection the
    /// client has for it now (a new one after a reconnect): the answer's
    /// body, or the error its status makes.
    fn call(&mut self, command: Command, body: &dyn smb2::pack::Pack) -> smb2::Result<Vec<u8>> {
        let tree = self.tree.tree_id;
        let conn = self.client.connection_for(&self.tree)?;
        let frame = self.rt.block_on(conn.execute(command, body, Some(tree)))?;
        if frame.header.status != NtStatus::SUCCESS {
            return Err(smb2::Error::Protocol { status: frame.header.status, command });
        }
        Ok(frame.body)
    }

    /// Leaves the share; the session ends when the share is dropped,
    /// with its connection.
    pub fn disconnect(&mut self) {
        let _ = self.rt.block_on(self.client.disconnect_share(&self.tree));
    }
}

/// A time of the share's as a [`SystemTime`].
pub fn system_time(time: FileTime) -> SystemTime {
    time.to_system_time().unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Times to set: the access and modification times given.
pub fn times(accessed: Option<SystemTime>, modified: Option<SystemTime>) -> FileTimes {
    let mut times = FileTimes::new();
    times.accessed = accessed.map(FileTime::from_system_time);
    times.modified = modified.map(FileTime::from_system_time);
    times
}
