// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! fattr4: a file's attributes as NFS 4 says them, each in the order of
//! its number after a bitmap of those given (RFC 8881 section 5), and
//! those a client sets.

use crate::ipc::{FsStat, SetTime, Stat, Time};
use crate::nfs::{self, Attrs, IO_MAX, LEASE_SECONDS, attr, err};
use crate::xdr::{Reader, Writer};

/// Attributes a client sets but never reads.
pub const WRITE_ONLY: Attrs =
    Attrs::of(&[attr::TIME_ACCESS_SET, attr::TIME_MODIFY_SET, attr::MODE_SET_MASKED]);

/// Attributes of the filesystem a file is on, which take a statvfs.
pub const FS_ATTRS: Attrs = Attrs::of(&[
    attr::FILES_AVAIL,
    attr::FILES_FREE,
    attr::FILES_TOTAL,
    attr::SPACE_AVAIL,
    attr::SPACE_FREE,
    attr::SPACE_TOTAL,
]);

/// change_attr_type: NFS4_CHANGE_TYPE_IS_UNDEFINED, so a client takes a
/// change only for a difference, not an order: the change attribute is
/// the status change time, which two changes in one tick of the clock
/// may share.
const CHANGE_TYPE_UNDEFINED: u32 = 4;

/// What a file's attributes are made from.
pub struct Facts<'a> {
    /// Its stat.
    pub stat: &'a Stat,
    /// The filesystem it is on, as NFS names it: its share's key and its
    /// device.
    pub fsid: (u64, u64),
    /// Its filehandle.
    pub fh: &'a [u8],
    /// Its filesystem's counts, if they were asked for and read.
    pub fs: Option<FsStat>,
}

/// A file's change attribute: its status change time in nanoseconds,
/// which every change to it moves.
pub fn change_of(stat: &Stat) -> u64 {
    (stat.ctime.seconds.cast_unsigned())
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(stat.ctime.nanos))
}

/// The attributes of `wanted` that can be said of a file, given whether
/// its filesystem's counts are at hand.
pub fn given(wanted: Attrs, fs: bool) -> Attrs {
    let given = wanted.and(nfs::SUPPORTED).without(WRITE_ONLY);
    if fs { given } else { given.without(FS_ATTRS) }
}

fn time(w: &mut Writer, t: Time) {
    w.i64(t.seconds).u32(t.nanos);
}

/// Writes fattr4: the bitmap of the attributes of `wanted` that can be
/// said, then their values.
pub fn encode(w: &mut Writer, wanted: Attrs, facts: &Facts) {
    let given = given(wanted, facts.fs.is_some());
    w.bitmap(&given.0);
    let values = values(given, facts);
    w.opaque(&values.bytes);
}

/// The values of the attributes of `given`, in the order of their
/// numbers.
pub fn values(given: Attrs, facts: &Facts) -> Writer {
    let s = facts.stat;
    let fs = facts.fs.unwrap_or_default();
    let mut v = Writer::new();
    for bit in given.bits() {
        match bit {
            attr::SUPPORTED_ATTRS => {
                v.bitmap(&nfs::SUPPORTED.0);
            }
            attr::TYPE => {
                v.u32(s.kind);
            }
            // FH4_PERSISTENT: a handle lasts as long as its file.
            attr::FH_EXPIRE_TYPE | attr::ACLSUPPORT | attr::RDATTR_ERROR => {
                v.u32(0);
            }
            attr::CHANGE => {
                v.u64(change_of(s));
            }
            attr::SIZE => {
                v.u64(s.size);
            }
            attr::LINK_SUPPORT
            | attr::SYMLINK_SUPPORT
            | attr::UNIQUE_HANDLES
            | attr::CANSETTIME
            | attr::CASE_PRESERVING
            | attr::CHOWN_RESTRICTED
            | attr::HOMOGENEOUS
            | attr::NO_TRUNC => {
                v.bool(true);
            }
            attr::NAMED_ATTR | attr::CASE_INSENSITIVE => {
                v.bool(false);
            }
            attr::FSID => {
                v.u64(facts.fsid.0).u64(facts.fsid.1);
            }
            attr::LEASE_TIME => {
                v.u32(LEASE_SECONDS);
            }
            attr::FILEHANDLE => {
                v.opaque(facts.fh);
            }
            attr::FILEID | attr::MOUNTED_ON_FILEID => {
                v.u64(s.ino);
            }
            attr::FILES_AVAIL => {
                v.u64(fs.files_avail);
            }
            attr::FILES_FREE => {
                v.u64(fs.files_free);
            }
            attr::FILES_TOTAL => {
                v.u64(fs.files_total);
            }
            attr::MAXFILESIZE => {
                v.u64(i64::MAX.cast_unsigned());
            }
            attr::MAXLINK => {
                v.u32(65_000);
            }
            attr::MAXNAME => {
                v.u32(255);
            }
            attr::MAXREAD | attr::MAXWRITE => {
                v.u64(u64::from(IO_MAX));
            }
            attr::MODE => {
                v.u32(s.mode & 0o7777);
            }
            attr::NUMLINKS => {
                v.u32(s.nlink);
            }
            attr::OWNER => {
                v.opaque(s.uid.to_string().as_bytes());
            }
            attr::OWNER_GROUP => {
                v.opaque(s.gid.to_string().as_bytes());
            }
            attr::RAWDEV => {
                v.u32(s.rdev.0).u32(s.rdev.1);
            }
            attr::SPACE_AVAIL => {
                v.u64(fs.space_avail);
            }
            attr::SPACE_FREE => {
                v.u64(fs.space_free);
            }
            attr::SPACE_TOTAL => {
                v.u64(fs.space_total);
            }
            attr::SPACE_USED => {
                v.u64(s.used);
            }
            attr::TIME_ACCESS => time(&mut v, s.atime),
            attr::TIME_DELTA => time(&mut v, Time { seconds: 0, nanos: 1 }),
            attr::TIME_METADATA => time(&mut v, s.ctime),
            attr::TIME_MODIFY => time(&mut v, s.mtime),
            // No layouts: an empty list.
            attr::FS_LAYOUT_TYPES => {
                v.u32(0);
            }
            attr::SUPPATTR_EXCLCREAT => {
                v.bitmap(&nfs::EXCLUSIVE_SETTABLE.0);
            }
            attr::CHANGE_ATTR_TYPE => {
                v.u32(CHANGE_TYPE_UNDEFINED);
            }
            _ => {}
        }
    }
    v
}

/// Attributes a client sets, as SETATTR, CREATE and OPEN carry them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Those it gave.
    pub given: Attrs,
    /// The size.
    pub size: Option<u64>,
    /// The mode.
    pub mode: Option<u32>,
    /// A mode and a mask of the bits of the file's own to keep.
    pub masked: Option<(u32, u32)>,
    /// The owner, by number.
    pub uid: Option<u32>,
    /// The group, by number.
    pub gid: Option<u32>,
    /// The access time.
    pub atime: SetTime,
    /// The modification time.
    pub mtime: SetTime,
}

/// An owner or group: its number, as Linux's clients send it when they
/// do not map names (`nfs4_disable_idmapping`, the default for AUTH_SYS).
fn number(text: &[u8]) -> Result<u32, u32> {
    std::str::from_utf8(text)
        .ok()
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|t| t.parse().ok())
        .ok_or(err::BADOWNER)
}

/// A settime4.
fn set_time(r: &mut Reader) -> Result<SetTime, u32> {
    match r.u32().map_err(|_| err::BADXDR)? {
        0 => Ok(SetTime::Now),
        1 => {
            let seconds = r.i64().map_err(|_| err::BADXDR)?;
            let nanos = r.u32().map_err(|_| err::BADXDR)?;
            if nanos >= 1_000_000_000 {
                return Err(err::INVAL);
            }
            Ok(SetTime::At(Time { seconds, nanos }))
        }
        _ => Err(err::BADXDR),
    }
}

/// Reads fattr4 to set, of the attributes of `allowed`: those NFS does
/// not have are ATTRNOTSUPP, and the rest INVAL.
pub fn decode_settings(r: &mut Reader, allowed: Attrs) -> Result<Settings, u32> {
    let given = Attrs::from_words(&r.bitmap().map_err(|_| err::BADXDR)?);
    let values = r.opaque(64 * 1024).map_err(|_| err::BADXDR)?;
    let mut v = Reader::new(values);
    let mut out = Settings { given, ..Settings::default() };
    for bit in given.bits() {
        if !nfs::SUPPORTED.has(bit) {
            return Err(err::ATTRNOTSUPP);
        }
        if !allowed.has(bit) {
            return Err(err::INVAL);
        }
        let bad = |_| err::BADXDR;
        match bit {
            attr::SIZE => out.size = Some(v.u64().map_err(bad)?),
            attr::MODE => out.mode = Some(v.u32().map_err(bad)? & 0o7777),
            attr::OWNER => out.uid = Some(number(v.opaque(1024).map_err(bad)?)?),
            attr::OWNER_GROUP => out.gid = Some(number(v.opaque(1024).map_err(bad)?)?),
            attr::TIME_ACCESS_SET => out.atime = set_time(&mut v)?,
            attr::TIME_MODIFY_SET => out.mtime = set_time(&mut v)?,
            attr::MODE_SET_MASKED => {
                let mode = v.u32().map_err(bad)? & 0o7777;
                let mask = v.u32().map_err(bad)? & 0o7777;
                out.masked = Some((mode, mask));
            }
            _ => return Err(err::INVAL),
        }
    }
    if v.left() != 0 {
        return Err(err::BADXDR);
    }
    if out.mode.is_some() && out.masked.is_some() {
        return Err(err::INVAL);
    }
    Ok(out)
}

/// The mode `settings` give a file whose mode is `old`, if they give one.
pub fn new_mode(settings: &Settings, old: u32) -> Option<u32> {
    settings.mode.or_else(|| settings.masked.map(|(mode, mask)| (old & mask) | (mode & !mask)))
}

/// What VERIFY and NVERIFY compare: the attributes asked about, and their
/// values as the client has them.
pub struct Claim<'a> {
    /// The attributes.
    pub attrs: Attrs,
    /// Their values.
    pub values: &'a [u8],
}

/// Reads the fattr4 of VERIFY or NVERIFY: none may be write-only or
/// RDATTR_ERROR (INVAL), or one NFS does not have (ATTRNOTSUPP).
pub fn decode_claim<'a>(r: &mut Reader<'a>) -> Result<Claim<'a>, u32> {
    let attrs = Attrs::from_words(&r.bitmap().map_err(|_| err::BADXDR)?);
    let values = r.opaque(64 * 1024).map_err(|_| err::BADXDR)?;
    if !attrs.without(nfs::SUPPORTED).is_empty() {
        return Err(err::ATTRNOTSUPP);
    }
    if attrs.has(attr::RDATTR_ERROR) || !attrs.and(WRITE_ONLY).is_empty() {
        return Err(err::INVAL);
    }
    Ok(Claim { attrs, values })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat() -> Stat {
        Stat {
            kind: nfs::kind::REG,
            mode: 0o100_644,
            nlink: 1,
            uid: 1000,
            gid: 1000,
            size: 5,
            used: 4096,
            rdev: (0, 0),
            dev: 9,
            ino: 77,
            atime: Time { seconds: 1, nanos: 2 },
            mtime: Time { seconds: 3, nanos: 4 },
            ctime: Time { seconds: 5, nanos: 6 },
        }
    }

    #[test]
    fn attributes_come_in_the_order_of_their_numbers() {
        let s = stat();
        let facts = Facts { stat: &s, fsid: (1, 9), fh: &[1, 2, 3], fs: None };
        let wanted = Attrs::of(&[attr::OWNER, attr::SIZE, attr::TYPE, attr::SPACE_FREE]);
        let mut w = Writer::new();
        encode(&mut w, wanted, &facts);
        let mut r = Reader::new(&w.bytes);
        // The space free is not given without the filesystem's counts.
        assert_eq!(r.bitmap().unwrap(), Attrs::of(&[attr::TYPE, attr::SIZE, attr::OWNER]).0[..2]);
        let mut v = Reader::new(r.opaque(1000).unwrap());
        assert_eq!(v.u32(), Ok(nfs::kind::REG));
        assert_eq!(v.u64(), Ok(5));
        assert_eq!(v.opaque(10), Ok(&b"1000"[..]));
        assert_eq!(v.left(), 0);
    }

    #[test]
    fn settings_are_read_and_refused() {
        let mut values = Writer::new();
        values.u64(0).opaque(b"1001").u32(1).i64(7).u32(8);
        let mut w = Writer::new();
        w.bitmap(&Attrs::of(&[attr::SIZE, attr::OWNER, attr::TIME_MODIFY_SET]).0);
        w.opaque(&values.bytes);
        let s = decode_settings(&mut Reader::new(&w.bytes), nfs::SETTABLE).unwrap();
        assert_eq!(s.size, Some(0));
        assert_eq!(s.uid, Some(1001));
        assert_eq!(s.mtime, SetTime::At(Time { seconds: 7, nanos: 8 }));
        assert_eq!(s.atime, SetTime::Keep);
        // A name for an owner, a settable one where it is not, and one
        // NFS does not have.
        let mut w = Writer::new();
        w.bitmap(&Attrs::of(&[attr::OWNER]).0);
        w.opaque(&{
            let mut v = Writer::new();
            v.opaque(b"chase@home");
            v.bytes
        });
        assert_eq!(decode_settings(&mut Reader::new(&w.bytes), nfs::SETTABLE), Err(err::BADOWNER));
        let mut w = Writer::new();
        w.bitmap(&Attrs::of(&[attr::SIZE]).0).opaque(&[0; 8]);
        assert_eq!(decode_settings(&mut Reader::new(&w.bytes), Attrs::default()), Err(err::INVAL));
        let mut w = Writer::new();
        w.bitmap(&Attrs::of(&[12]).0).opaque(&[]);
        assert_eq!(
            decode_settings(&mut Reader::new(&w.bytes), nfs::SETTABLE),
            Err(err::ATTRNOTSUPP)
        );
    }

    #[test]
    fn a_masked_mode_keeps_the_masked_bits() {
        let s = Settings { masked: Some((0o700, 0o077)), ..Settings::default() };
        assert_eq!(new_mode(&s, 0o644), Some(0o744));
        assert_eq!(new_mode(&Settings::default(), 0o644), None);
    }
}
