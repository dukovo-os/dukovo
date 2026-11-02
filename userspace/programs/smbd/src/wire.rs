// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SMB 2 and 3 on the wire (MS-SMB2 2.1 and 2.2): the frames of Direct
//! TCP, the 64-byte header every message starts with, the codes of the
//! commands and of NT's statuses, and reading and writing the
//! little-endian fields of a message's body.
//!
//! Every offset a message gives is checked against its length before it
//! is read: a field past the end is `STATUS_INVALID_PARAMETER`, never a
//! read past it.

use std::io::{self, Read, Write};

/// The SMB 2 header's first four bytes.
pub const PROTOCOL: [u8; 4] = [0xfe, b'S', b'M', b'B'];
/// SMB 1's, which a client may open with to ask for SMB 2.
pub const PROTOCOL_1: [u8; 4] = [0xff, b'S', b'M', b'B'];
/// An encrypted message's TRANSFORM_HEADER's.
pub const PROTOCOL_ENCRYPTED: [u8; 4] = [0xfd, b'S', b'M', b'B'];

/// The header's length.
pub const HEADER: usize = 64;

/// The longest frame read, in bytes: a write of [`MAX_IO`] and its
/// headers, encrypted.
pub const FRAME_MAX: usize = MAX_IO as usize + 4096;

/// The most a read or a write moves at once, and the most a transaction
/// carries: what the NEGOTIATE response offers.
pub const MAX_IO: u32 = 4 << 20;

/// The commands (MS-SMB2 2.2.1).
pub mod command {
    pub const NEGOTIATE: u16 = 0x00;
    pub const SESSION_SETUP: u16 = 0x01;
    pub const LOGOFF: u16 = 0x02;
    pub const TREE_CONNECT: u16 = 0x03;
    pub const TREE_DISCONNECT: u16 = 0x04;
    pub const CREATE: u16 = 0x05;
    pub const CLOSE: u16 = 0x06;
    pub const FLUSH: u16 = 0x07;
    pub const READ: u16 = 0x08;
    pub const WRITE: u16 = 0x09;
    pub const LOCK: u16 = 0x0a;
    pub const IOCTL: u16 = 0x0b;
    pub const CANCEL: u16 = 0x0c;
    pub const ECHO: u16 = 0x0d;
    pub const QUERY_DIRECTORY: u16 = 0x0e;
    pub const CHANGE_NOTIFY: u16 = 0x0f;
    pub const QUERY_INFO: u16 = 0x10;
    pub const SET_INFO: u16 = 0x11;
    pub const OPLOCK_BREAK: u16 = 0x12;
}

/// The header's flags.
pub mod flags {
    pub const SERVER_TO_REDIR: u32 = 0x0000_0001;
    pub const ASYNC_COMMAND: u32 = 0x0000_0002;
    pub const RELATED_OPERATIONS: u32 = 0x0000_0004;
    pub const SIGNED: u32 = 0x0000_0008;
}

/// NT's statuses (MS-ERREF 2.3.1), those smbd answers with.
#[allow(missing_docs)]
pub mod status {
    pub const SUCCESS: u32 = 0x0000_0000;
    pub const PENDING: u32 = 0x0000_0103;
    pub const NOTIFY_CLEANUP: u32 = 0x0000_010b;
    pub const BUFFER_OVERFLOW: u32 = 0x8000_0005;
    pub const NO_MORE_FILES: u32 = 0x8000_0006;
    pub const UNSUCCESSFUL: u32 = 0xc000_0001;
    pub const INVALID_INFO_CLASS: u32 = 0xc000_0003;
    pub const INFO_LENGTH_MISMATCH: u32 = 0xc000_0004;
    pub const INVALID_PARAMETER: u32 = 0xc000_000d;
    pub const NO_SUCH_FILE: u32 = 0xc000_000f;
    pub const INVALID_DEVICE_REQUEST: u32 = 0xc000_0010;
    pub const END_OF_FILE: u32 = 0xc000_0011;
    pub const MORE_PROCESSING_REQUIRED: u32 = 0xc000_0016;
    pub const BUFFER_TOO_SMALL: u32 = 0xc000_0023;
    pub const ACCESS_DENIED: u32 = 0xc000_0022;
    pub const OBJECT_NAME_INVALID: u32 = 0xc000_0033;
    pub const OBJECT_NAME_NOT_FOUND: u32 = 0xc000_0034;
    pub const OBJECT_NAME_COLLISION: u32 = 0xc000_0035;
    pub const OBJECT_PATH_NOT_FOUND: u32 = 0xc000_003a;
    pub const QUOTA_EXCEEDED: u32 = 0xc000_0044;
    pub const EAS_NOT_SUPPORTED: u32 = 0xc000_004f;
    pub const FILE_LOCK_CONFLICT: u32 = 0xc000_0054;
    pub const LOCK_NOT_GRANTED: u32 = 0xc000_0055;
    pub const LOGON_FAILURE: u32 = 0xc000_006d;
    pub const RANGE_NOT_LOCKED: u32 = 0xc000_007e;
    pub const DISK_FULL: u32 = 0xc000_007f;
    pub const INSUFFICIENT_RESOURCES: u32 = 0xc000_009a;
    pub const MEDIA_WRITE_PROTECTED: u32 = 0xc000_00a2;
    pub const FILE_IS_A_DIRECTORY: u32 = 0xc000_00ba;
    pub const NOT_SUPPORTED: u32 = 0xc000_00bb;
    pub const NETWORK_NAME_DELETED: u32 = 0xc000_00c9;
    pub const BAD_NETWORK_NAME: u32 = 0xc000_00cc;
    pub const REQUEST_NOT_ACCEPTED: u32 = 0xc000_00d0;
    pub const PIPE_EMPTY: u32 = 0xc000_00d9;
    pub const NOT_SAME_DEVICE: u32 = 0xc000_00d4;
    pub const DIRECTORY_NOT_EMPTY: u32 = 0xc000_0101;
    pub const NOT_A_DIRECTORY: u32 = 0xc000_0103;
    pub const NAME_TOO_LONG: u32 = 0xc000_0106;
    pub const TOO_MANY_OPENED_FILES: u32 = 0xc000_011f;
    pub const CANCELLED: u32 = 0xc000_0120;
    pub const CANNOT_DELETE: u32 = 0xc000_0121;
    pub const FILE_CLOSED: u32 = 0xc000_0128;
    pub const USER_SESSION_DELETED: u32 = 0xc000_0203;
    pub const NOT_A_REPARSE_POINT: u32 = 0xc000_0275;
    pub const FS_DRIVER_REQUIRED: u32 = 0xc000_019c;
    pub const INVALID_LOCK_RANGE: u32 = 0xc000_01a1;
}

/// The status for an error of the file system.
pub fn status_of(err: &io::Error) -> u32 {
    match err.raw_os_error() {
        Some(libc::ENOENT) => status::OBJECT_NAME_NOT_FOUND,
        Some(libc::ENOTDIR) => status::OBJECT_PATH_NOT_FOUND,
        Some(libc::EACCES | libc::EPERM) => status::ACCESS_DENIED,
        Some(libc::EEXIST) => status::OBJECT_NAME_COLLISION,
        Some(libc::ENOTEMPTY) => status::DIRECTORY_NOT_EMPTY,
        Some(libc::EISDIR) => status::FILE_IS_A_DIRECTORY,
        Some(libc::ENOSPC) => status::DISK_FULL,
        Some(libc::EDQUOT) => status::QUOTA_EXCEEDED,
        Some(libc::EROFS) => status::MEDIA_WRITE_PROTECTED,
        Some(libc::ENAMETOOLONG) => status::NAME_TOO_LONG,
        Some(libc::EXDEV) => status::NOT_SAME_DEVICE,
        Some(libc::EMFILE | libc::ENFILE) => status::TOO_MANY_OPENED_FILES,
        Some(libc::EINVAL) => status::INVALID_PARAMETER,
        Some(libc::EOPNOTSUPP) => status::NOT_SUPPORTED,
        _ => match err.kind() {
            io::ErrorKind::NotFound => status::OBJECT_NAME_NOT_FOUND,
            io::ErrorKind::PermissionDenied => status::ACCESS_DENIED,
            io::ErrorKind::AlreadyExists => status::OBJECT_NAME_COLLISION,
            _ => status::UNSUCCESSFUL,
        },
    }
}

/// A status as an error: what a handler fails with.
pub type Status = u32;

/// The little-endian `u16` at `at`, or `STATUS_INVALID_PARAMETER`.
pub fn u16_at(bytes: &[u8], at: usize) -> Result<u16, Status> {
    let b = bytes.get(at..at + 2).ok_or(status::INVALID_PARAMETER)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

/// The little-endian `u32` at `at`.
pub fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Status> {
    let b = bytes.get(at..at + 4).ok_or(status::INVALID_PARAMETER)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The little-endian `u64` at `at`.
pub fn u64_at(bytes: &[u8], at: usize) -> Result<u64, Status> {
    let b = bytes.get(at..at + 8).ok_or(status::INVALID_PARAMETER)?;
    Ok(u64::from_le_bytes(b.try_into().map_err(|_| status::INVALID_PARAMETER)?))
}

/// The byte at `at`.
pub fn u8_at(bytes: &[u8], at: usize) -> Result<u8, Status> {
    bytes.get(at).copied().ok_or(status::INVALID_PARAMETER)
}

/// `length` bytes from `offset`, both as a message gives them.
pub fn slice_at(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8], Status> {
    if length == 0 {
        return Ok(&[]);
    }
    let end = offset.checked_add(length).ok_or(status::INVALID_PARAMETER)?;
    bytes.get(offset..end).ok_or(status::INVALID_PARAMETER)
}

/// UTF-16LE `bytes` as text: lone surrogates are refused.
pub fn utf16(bytes: &[u8]) -> Result<String, Status> {
    if !bytes.len().is_multiple_of(2) {
        return Err(status::OBJECT_NAME_INVALID);
    }
    let units: Vec<u16> =
        bytes.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
    String::from_utf16(&units).map_err(|_| status::OBJECT_NAME_INVALID)
}

/// `text` in UTF-16LE.
pub fn to_utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// A message's body being written: little-endian fields, one after
/// another.
#[derive(Default)]
pub struct Out(pub Vec<u8>);

impl Out {
    /// An empty one.
    pub fn new() -> Out {
        Out(Vec::new())
    }

    /// Appends a byte.
    pub fn u8(&mut self, value: u8) -> &mut Out {
        self.0.push(value);
        self
    }

    /// Appends a `u16`.
    pub fn u16(&mut self, value: u16) -> &mut Out {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Appends a `u32`.
    pub fn u32(&mut self, value: u32) -> &mut Out {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Appends a `u64`.
    pub fn u64(&mut self, value: u64) -> &mut Out {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Appends bytes.
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Out {
        self.0.extend_from_slice(bytes);
        self
    }

    /// Appends `count` zeros.
    pub fn zeros(&mut self, count: usize) -> &mut Out {
        self.0.resize(self.0.len() + count, 0);
        self
    }

    /// Pads with zeros to a multiple of `to`.
    pub fn align(&mut self, to: usize) -> &mut Out {
        let pad = (to - self.0.len() % to) % to;
        self.zeros(pad)
    }

    /// Its length.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Writes `value` over the `u16` at `at`.
    pub fn set_u16(&mut self, at: usize, value: u16) {
        self.0[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes `value` over the `u32` at `at`.
    pub fn set_u32(&mut self, at: usize, value: u32) {
        self.0[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

/// A message's header (MS-SMB2 2.2.1), as read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    /// How many credits the request costs.
    pub credit_charge: u16,
    /// A response's status; in a request, the channel sequence.
    pub status: u32,
    /// The command.
    pub command: u16,
    /// The credits asked for, or granted.
    pub credits: u16,
    /// [`flags`].
    pub flags: u32,
    /// Where the next message of a compound starts, from this one's
    /// start; 0 for the last.
    pub next_command: u32,
    /// The message's number.
    pub message_id: u64,
    /// An async message's number, or a sync one's tree in its low half.
    pub async_id: u64,
    /// The session.
    pub session_id: u64,
    /// The signature.
    pub signature: [u8; 16],
}

impl Header {
    /// Reads a message's header.
    pub fn parse(message: &[u8]) -> Result<Header, Status> {
        if message.len() < HEADER || message[..4] != PROTOCOL || u16_at(message, 4)? != 64 {
            return Err(status::INVALID_PARAMETER);
        }
        let mut signature = [0u8; 16];
        signature.copy_from_slice(&message[48..64]);
        Ok(Header {
            credit_charge: u16_at(message, 6)?,
            status: u32_at(message, 8)?,
            command: u16_at(message, 12)?,
            credits: u16_at(message, 14)?,
            flags: u32_at(message, 16)?,
            next_command: u32_at(message, 20)?,
            message_id: u64_at(message, 24)?,
            async_id: u64_at(message, 32)?,
            session_id: u64_at(message, 40)?,
            signature,
        })
    }

    /// A sync message's tree.
    pub fn tree_id(&self) -> u32 {
        (self.async_id >> 32) as u32
    }

    /// Whether it is async.
    pub fn is_async(&self) -> bool {
        self.flags & flags::ASYNC_COMMAND != 0
    }

    /// Whether it is related to the message before it in a compound.
    pub fn is_related(&self) -> bool {
        self.flags & flags::RELATED_OPERATIONS != 0
    }

    /// The header of the response to this request, with `status`: the
    /// same command, number, tree and session, credits to be filled in.
    pub fn response(&self, status: u32) -> Header {
        Header {
            credit_charge: self.credit_charge,
            status,
            command: self.command,
            credits: 0,
            flags: flags::SERVER_TO_REDIR | (self.flags & flags::RELATED_OPERATIONS),
            next_command: 0,
            message_id: self.message_id,
            async_id: if self.is_async() { self.async_id } else { self.async_id & !0xffff_ffff },
            session_id: self.session_id,
            signature: [0; 16],
        }
    }

    /// Its 64 bytes.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&PROTOCOL);
        out.extend_from_slice(&64u16.to_le_bytes());
        out.extend_from_slice(&self.credit_charge.to_le_bytes());
        out.extend_from_slice(&self.status.to_le_bytes());
        out.extend_from_slice(&self.command.to_le_bytes());
        out.extend_from_slice(&self.credits.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.next_command.to_le_bytes());
        out.extend_from_slice(&self.message_id.to_le_bytes());
        out.extend_from_slice(&self.async_id.to_le_bytes());
        out.extend_from_slice(&self.session_id.to_le_bytes());
        out.extend_from_slice(&self.signature);
    }
}

/// A whole message: `header`, then `body`.
pub fn message(header: &Header, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + body.len());
    header.write(&mut out);
    out.extend_from_slice(body);
    out
}

/// An error response's body (MS-SMB2 2.2.2): no error data, and the one
/// byte its structure size counts.
pub fn error_body() -> Vec<u8> {
    let mut out = Out::new();
    out.u16(9).u8(0).u8(0).u32(0).u8(0);
    out.0
}

/// An error response's body with `data`: STATUS_BUFFER_TOO_SMALL's, the
/// size the answer needs.
pub fn error_body_with(data: &[u8]) -> Vec<u8> {
    let mut out = Out::new();
    out.u16(9).u8(0).u8(0).u32(data.len() as u32).bytes(data);
    out.0
}

/// Reads one frame of Direct TCP (MS-SMB2 2.1): a zero byte, a 24-bit
/// big-endian length, then that many bytes. `None` when the connection
/// ended between frames.
pub fn read_frame(stream: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    read_frame_max(stream, FRAME_MAX)
}

/// Reads one frame, as [`read_frame`] does, of at most `max` bytes.
pub fn read_frame_max(stream: &mut impl Read, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match stream.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    let length = u32::from_be_bytes(head) as usize;
    if head[0] != 0 || length > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a frame too long for SMB"));
    }
    let mut frame = vec![0u8; length];
    stream.read_exact(&mut frame)?;
    Ok(Some(frame))
}

/// Writes `message` as one frame.
pub fn write_frame(stream: &mut impl Write, message: &[u8]) -> io::Result<()> {
    let length = message.len() as u32;
    let mut frame = Vec::with_capacity(4 + message.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(message);
    stream.write_all(&frame)
}

/// Windows' time, in 100-nanosecond ticks since 1601, of a Unix time.
pub fn filetime(seconds: i64, nanoseconds: i64) -> u64 {
    const FROM_1601: i64 = 11_644_473_600;
    let ticks = (seconds + FROM_1601).saturating_mul(10_000_000) + nanoseconds / 100;
    ticks.max(0) as u64
}

/// The Unix time of Windows' `ticks`: seconds and nanoseconds.
pub fn unix_time(ticks: u64) -> (i64, i64) {
    const FROM_1601: i64 = 11_644_473_600;
    let ticks = ticks as i64;
    (ticks.div_euclid(10_000_000) - FROM_1601, ticks.rem_euclid(10_000_000) * 100)
}

/// Windows' time now.
pub fn now() -> u64 {
    let since =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    filetime(since.as_secs() as i64, i64::from(since.subsec_nanos()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_goes_out_and_back() {
        let header = Header {
            credit_charge: 1,
            status: status::PENDING,
            command: command::CREATE,
            credits: 7,
            flags: flags::SERVER_TO_REDIR | flags::ASYNC_COMMAND,
            next_command: 0,
            message_id: 42,
            async_id: 9,
            session_id: 0x1234,
            signature: [3; 16],
        };
        let bytes = message(&header, &[1, 2]);
        assert_eq!(bytes.len(), 66);
        assert_eq!(Header::parse(&bytes), Ok(header));
        assert!(Header::parse(&bytes[..63]).is_err());
    }

    #[test]
    fn fields_past_the_end_are_refused() {
        assert_eq!(u32_at(&[1, 2, 3], 0), Err(status::INVALID_PARAMETER));
        assert_eq!(slice_at(&[1, 2, 3], 2, 2), Err(status::INVALID_PARAMETER));
        assert_eq!(slice_at(&[1, 2, 3], usize::MAX, 2), Err(status::INVALID_PARAMETER));
        assert_eq!(slice_at(&[1, 2, 3], 1, 2), Ok(&[2, 3][..]));
    }

    #[test]
    fn frames_carry_their_length() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"hello").unwrap();
        assert_eq!(&wire[..4], &[0, 0, 0, 5]);
        let mut reader = &wire[..];
        assert_eq!(read_frame(&mut reader).unwrap(), Some(b"hello".to_vec()));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
        let mut long = &[0u8, 0xff, 0xff, 0xff][..];
        assert!(read_frame(&mut long).is_err());
    }

    #[test]
    fn times_go_both_ways() {
        assert_eq!(filetime(0, 0), 116_444_736_000_000_000);
        assert_eq!(unix_time(filetime(1_700_000_000, 123_456_700)), (1_700_000_000, 123_456_700));
        assert_eq!(filetime(-20_000_000_000, 0), 0);
    }
}
