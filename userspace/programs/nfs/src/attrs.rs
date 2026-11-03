// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NFS version 4's attributes (RFC 8881 section 5): those asked for, read
//! from a fattr4, and those set, written into one.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nfsd::nfs::{Attrs, attr};
use nfsd::xdr::{self, Reader, Writer};

/// What is asked of a file to show it: its kind, size, owners, modes and
/// times.
pub const FILE: Attrs = Attrs::of(&[
    attr::TYPE,
    attr::CHANGE,
    attr::SIZE,
    attr::FILEID,
    attr::MODE,
    attr::NUMLINKS,
    attr::OWNER,
    attr::OWNER_GROUP,
    attr::RAWDEV,
    attr::SPACE_USED,
    attr::TIME_ACCESS,
    attr::TIME_METADATA,
    attr::TIME_MODIFY,
]);

/// What a listing asks of each name: its kind and number.
pub const LISTED: Attrs = Attrs::of(&[attr::TYPE, attr::RDATTR_ERROR, attr::FILEID]);

/// What is asked of the mount's folder at first: how long a lease is,
/// and the most a READ and a WRITE carry.
pub const SERVER: Attrs =
    Attrs::of(&[attr::TYPE, attr::LEASE_TIME, attr::FILEID, attr::MAXREAD, attr::MAXWRITE]);

/// What statfs asks: the room and the names left.
pub const SPACE: Attrs = Attrs::of(&[
    attr::FILES_AVAIL,
    attr::FILES_FREE,
    attr::FILES_TOTAL,
    attr::MAXNAME,
    attr::SPACE_AVAIL,
    attr::SPACE_FREE,
    attr::SPACE_TOTAL,
]);

/// A file's attributes, those the server gave; the rest are zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attr {
    /// nfs_ftype4.
    pub kind: u32,
    pub change: u64,
    pub size: u64,
    pub lease: u32,
    /// Why the attributes of a listed name could not be read.
    pub error: u32,
    pub fileid: u64,
    pub files_avail: u64,
    pub files_free: u64,
    pub files_total: u64,
    pub max_name: u32,
    pub max_read: u64,
    pub max_write: u64,
    /// The mode's permission bits, set-ID bits and sticky bit.
    pub mode: u32,
    pub nlink: u32,
    /// The owner and group, as the server names them: numbers, or
    /// `name@domain`.
    pub owner: Vec<u8>,
    pub group: Vec<u8>,
    /// specdata4: a device's major and minor numbers.
    pub rdev: (u32, u32),
    pub space_avail: u64,
    pub space_free: u64,
    pub space_total: u64,
    pub space_used: u64,
    /// The times of the last read, change of attributes and write.
    pub atime: Option<SystemTime>,
    pub ctime: Option<SystemTime>,
    pub mtime: Option<SystemTime>,
}

/// Reads an nfstime4.
fn time(r: &mut Reader) -> xdr::Result<SystemTime> {
    let seconds = r.i64()?;
    let nanos = r.u32()?.min(999_999_999);
    let at = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(seconds.unsigned_abs(), nanos))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .and_then(|t| t.checked_add(Duration::from_nanos(u64::from(nanos))))
    };
    Ok(at.unwrap_or(UNIX_EPOCH))
}

/// Writes `at` as an nfstime4.
fn write_time(w: &mut Writer, at: SystemTime) {
    match at.duration_since(UNIX_EPOCH) {
        Ok(after) => {
            w.i64(i64::try_from(after.as_secs()).unwrap_or(i64::MAX)).u32(after.subsec_nanos());
        }
        Err(before) => {
            let before = before.duration();
            let mut seconds = -i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
            let mut nanos = before.subsec_nanos();
            if nanos != 0 {
                seconds -= 1;
                nanos = 1_000_000_000 - nanos;
            }
            w.i64(seconds).u32(nanos);
        }
    }
}

/// Reads a fattr4: its bitmap, then its values, of the bits this knows.
/// A bit it does not know ends what it reads, as the values after it
/// cannot be found.
pub fn decode(r: &mut Reader) -> xdr::Result<Attr> {
    let given = Attrs::from_words(&r.bitmap()?);
    let values = r.opaque(1 << 16)?;
    let mut v = Reader::new(values);
    let mut a = Attr::default();
    for bit in given.bits() {
        match bit {
            attr::TYPE => a.kind = v.u32()?,
            attr::CHANGE => a.change = v.u64()?,
            attr::SIZE => a.size = v.u64()?,
            attr::LEASE_TIME => a.lease = v.u32()?,
            attr::RDATTR_ERROR => a.error = v.u32()?,
            attr::FILEID => a.fileid = v.u64()?,
            attr::FILES_AVAIL => a.files_avail = v.u64()?,
            attr::FILES_FREE => a.files_free = v.u64()?,
            attr::FILES_TOTAL => a.files_total = v.u64()?,
            attr::MAXNAME => a.max_name = v.u32()?,
            attr::MAXREAD => a.max_read = v.u64()?,
            attr::MAXWRITE => a.max_write = v.u64()?,
            attr::MODE => a.mode = v.u32()? & 0o7777,
            attr::NUMLINKS => a.nlink = v.u32()?,
            attr::OWNER => a.owner = v.opaque(1024)?.to_vec(),
            attr::OWNER_GROUP => a.group = v.opaque(1024)?.to_vec(),
            attr::RAWDEV => a.rdev = (v.u32()?, v.u32()?),
            attr::SPACE_AVAIL => a.space_avail = v.u64()?,
            attr::SPACE_FREE => a.space_free = v.u64()?,
            attr::SPACE_TOTAL => a.space_total = v.u64()?,
            attr::SPACE_USED => a.space_used = v.u64()?,
            attr::TIME_ACCESS => a.atime = Some(time(&mut v)?),
            attr::TIME_METADATA => a.ctime = Some(time(&mut v)?),
            attr::TIME_MODIFY => a.mtime = Some(time(&mut v)?),
            _ => break,
        }
    }
    Ok(a)
}

/// A time to set: the server's clock's, or this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Now,
    At(SystemTime),
}

/// Attributes to set, those given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Set {
    pub size: Option<u64>,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub atime: Option<When>,
    pub mtime: Option<When>,
}

impl Set {
    /// The mode alone.
    pub fn mode(mode: u32) -> Set {
        Set { mode: Some(mode & 0o7777), ..Set::default() }
    }

    /// Writes them as a fattr4. Owners go as numbers, as Linux's server
    /// takes them from AUTH_SYS's clients.
    pub fn write(&self, w: &mut Writer) {
        let mut bits = Attrs::default();
        let mut v = Writer::new();
        if let Some(size) = self.size {
            bits.add(attr::SIZE);
            v.u64(size);
        }
        if let Some(mode) = self.mode {
            bits.add(attr::MODE);
            v.u32(mode & 0o7777);
        }
        if let Some(uid) = self.uid {
            bits.add(attr::OWNER);
            v.opaque(uid.to_string().as_bytes());
        }
        if let Some(gid) = self.gid {
            bits.add(attr::OWNER_GROUP);
            v.opaque(gid.to_string().as_bytes());
        }
        for (bit, when) in [(attr::TIME_ACCESS_SET, self.atime), (attr::TIME_MODIFY_SET, self.mtime)]
        {
            let Some(when) = when else { continue };
            bits.add(bit);
            match when {
                When::Now => {
                    v.u32(0);
                }
                When::At(at) => {
                    v.u32(1);
                    write_time(&mut v, at);
                }
            }
        }
        w.bitmap(&bits.0).opaque(&v.bytes);
    }
}

/// A request's bitmap of `attrs`.
pub fn ask(w: &mut Writer, attrs: Attrs) {
    w.bitmap(&attrs.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_go_in_the_order_of_their_bits() {
        let set = Set {
            size: Some(5),
            mode: Some(0o100_644),
            uid: Some(1000),
            atime: Some(When::Now),
            mtime: Some(When::At(UNIX_EPOCH + Duration::new(7, 9))),
            ..Set::default()
        };
        let mut w = Writer::new();
        set.write(&mut w);
        let mut r = Reader::new(&w.bytes);
        let bits = Attrs::from_words(&r.bitmap().unwrap());
        assert_eq!(
            bits.bits().collect::<Vec<_>>(),
            [attr::SIZE, attr::MODE, attr::OWNER, attr::TIME_ACCESS_SET, attr::TIME_MODIFY_SET]
        );
        let mut v = Reader::new(r.opaque(1024).unwrap());
        assert_eq!(v.u64(), Ok(5));
        assert_eq!(v.u32(), Ok(0o644));
        assert_eq!(v.opaque(16), Ok(&b"1000"[..]));
        assert_eq!(v.u32(), Ok(0));
        assert_eq!((v.u32(), v.i64(), v.u32()), (Ok(1), Ok(7), Ok(9)));
        assert_eq!(v.left(), 0);
    }

    #[test]
    fn times_before_1970_go_both_ways() {
        let at = UNIX_EPOCH - Duration::new(3, 250);
        let mut w = Writer::new();
        write_time(&mut w, at);
        assert_eq!(time(&mut Reader::new(&w.bytes)), Ok(at));
    }

    #[test]
    fn attributes_are_read_up_to_a_bit_not_known() {
        let mut v = Writer::new();
        v.u32(2).u64(10).opaque(b"1000").i64(5).u32(6).u64(99);
        let mut w = Writer::new();
        let bits = Attrs::of(&[attr::TYPE, attr::SIZE, attr::OWNER, attr::TIME_MODIFY, 90]);
        w.bitmap(&bits.0).opaque(&v.bytes);
        let a = decode(&mut Reader::new(&w.bytes)).unwrap();
        assert_eq!((a.kind, a.size, a.owner.as_slice()), (2, 10, &b"1000"[..]));
        assert_eq!(a.mtime, Some(UNIX_EPOCH + Duration::new(5, 6)));
    }
}
