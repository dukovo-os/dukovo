// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NFS version 4's numbers: its operations and statuses (RFC 8881 for
//! 4.1, RFC 7862 for 4.2), its attributes, and the kinds of files.

/// The operations, by number.
#[allow(missing_docs, dead_code)]
pub mod op {
    pub const ACCESS: u32 = 3;
    pub const CLOSE: u32 = 4;
    pub const COMMIT: u32 = 5;
    pub const CREATE: u32 = 6;
    pub const DELEGPURGE: u32 = 7;
    pub const DELEGRETURN: u32 = 8;
    pub const GETATTR: u32 = 9;
    pub const GETFH: u32 = 10;
    pub const LINK: u32 = 11;
    pub const LOCK: u32 = 12;
    pub const LOCKT: u32 = 13;
    pub const LOCKU: u32 = 14;
    pub const LOOKUP: u32 = 15;
    pub const LOOKUPP: u32 = 16;
    pub const NVERIFY: u32 = 17;
    pub const OPEN: u32 = 18;
    pub const OPENATTR: u32 = 19;
    pub const OPEN_CONFIRM: u32 = 20;
    pub const OPEN_DOWNGRADE: u32 = 21;
    pub const PUTFH: u32 = 22;
    pub const PUTPUBFH: u32 = 23;
    pub const PUTROOTFH: u32 = 24;
    pub const READ: u32 = 25;
    pub const READDIR: u32 = 26;
    pub const READLINK: u32 = 27;
    pub const REMOVE: u32 = 28;
    pub const RENAME: u32 = 29;
    pub const RENEW: u32 = 30;
    pub const RESTOREFH: u32 = 31;
    pub const SAVEFH: u32 = 32;
    pub const SECINFO: u32 = 33;
    pub const SETATTR: u32 = 34;
    pub const SETCLIENTID: u32 = 35;
    pub const SETCLIENTID_CONFIRM: u32 = 36;
    pub const VERIFY: u32 = 37;
    pub const WRITE: u32 = 38;
    pub const RELEASE_LOCKOWNER: u32 = 39;
    pub const BACKCHANNEL_CTL: u32 = 40;
    pub const BIND_CONN_TO_SESSION: u32 = 41;
    pub const EXCHANGE_ID: u32 = 42;
    pub const CREATE_SESSION: u32 = 43;
    pub const DESTROY_SESSION: u32 = 44;
    pub const FREE_STATEID: u32 = 45;
    pub const GET_DIR_DELEGATION: u32 = 46;
    pub const GETDEVICEINFO: u32 = 47;
    pub const GETDEVICELIST: u32 = 48;
    pub const LAYOUTCOMMIT: u32 = 49;
    pub const LAYOUTGET: u32 = 50;
    pub const LAYOUTRETURN: u32 = 51;
    pub const SECINFO_NO_NAME: u32 = 52;
    pub const SEQUENCE: u32 = 53;
    pub const SET_SSV: u32 = 54;
    pub const TEST_STATEID: u32 = 55;
    pub const WANT_DELEGATION: u32 = 56;
    pub const DESTROY_CLIENTID: u32 = 57;
    pub const RECLAIM_COMPLETE: u32 = 58;
    pub const ALLOCATE: u32 = 59;
    pub const COPY: u32 = 60;
    pub const COPY_NOTIFY: u32 = 61;
    pub const DEALLOCATE: u32 = 62;
    pub const IO_ADVISE: u32 = 63;
    pub const LAYOUTERROR: u32 = 64;
    pub const LAYOUTSTATS: u32 = 65;
    pub const OFFLOAD_CANCEL: u32 = 66;
    pub const OFFLOAD_STATUS: u32 = 67;
    pub const READ_PLUS: u32 = 68;
    pub const SEEK: u32 = 69;
    pub const WRITE_SAME: u32 = 70;
    pub const CLONE: u32 = 71;
    pub const GETXATTR: u32 = 72;
    pub const SETXATTR: u32 = 73;
    pub const LISTXATTRS: u32 = 74;
    pub const REMOVEXATTR: u32 = 75;
    pub const ILLEGAL: u32 = 10_044;
}

/// The statuses, nfsstat4.
#[allow(missing_docs, dead_code)]
pub mod err {
    pub const OK: u32 = 0;
    pub const PERM: u32 = 1;
    pub const NOENT: u32 = 2;
    pub const IO: u32 = 5;
    pub const NXIO: u32 = 6;
    pub const ACCESS: u32 = 13;
    pub const EXIST: u32 = 17;
    pub const XDEV: u32 = 18;
    pub const NOTDIR: u32 = 20;
    pub const ISDIR: u32 = 21;
    pub const INVAL: u32 = 22;
    pub const FBIG: u32 = 27;
    pub const NOSPC: u32 = 28;
    pub const ROFS: u32 = 30;
    pub const MLINK: u32 = 31;
    pub const NAMETOOLONG: u32 = 63;
    pub const NOTEMPTY: u32 = 66;
    pub const DQUOT: u32 = 69;
    pub const STALE: u32 = 70;
    pub const BADHANDLE: u32 = 10_001;
    pub const BAD_COOKIE: u32 = 10_003;
    pub const NOTSUPP: u32 = 10_004;
    pub const TOOSMALL: u32 = 10_005;
    pub const SERVERFAULT: u32 = 10_006;
    pub const BADTYPE: u32 = 10_007;
    pub const DELAY: u32 = 10_008;
    pub const SAME: u32 = 10_009;
    pub const DENIED: u32 = 10_010;
    pub const EXPIRED: u32 = 10_011;
    pub const LOCKED: u32 = 10_012;
    pub const SHARE_DENIED: u32 = 10_015;
    pub const WRONGSEC: u32 = 10_016;
    pub const CLID_INUSE: u32 = 10_017;
    pub const NOFILEHANDLE: u32 = 10_020;
    pub const MINOR_VERS_MISMATCH: u32 = 10_021;
    pub const STALE_CLIENTID: u32 = 10_022;
    pub const STALE_STATEID: u32 = 10_023;
    pub const OLD_STATEID: u32 = 10_024;
    pub const BAD_STATEID: u32 = 10_025;
    pub const BAD_SEQID: u32 = 10_026;
    pub const NOT_SAME: u32 = 10_027;
    pub const LOCK_RANGE: u32 = 10_028;
    pub const SYMLINK: u32 = 10_029;
    pub const RESTOREFH: u32 = 10_030;
    pub const ATTRNOTSUPP: u32 = 10_032;
    pub const NO_GRACE: u32 = 10_033;
    pub const BADXDR: u32 = 10_036;
    pub const LOCKS_HELD: u32 = 10_037;
    pub const OPENMODE: u32 = 10_038;
    pub const BADOWNER: u32 = 10_039;
    pub const BADNAME: u32 = 10_041;
    pub const BAD_RANGE: u32 = 10_042;
    pub const OP_ILLEGAL: u32 = 10_044;
    pub const DEADLOCK: u32 = 10_045;
    pub const FILE_OPEN: u32 = 10_046;
    pub const BADSESSION: u32 = 10_052;
    pub const BADSLOT: u32 = 10_053;
    pub const COMPLETE_ALREADY: u32 = 10_054;
    pub const CONN_NOT_BOUND_TO_SESSION: u32 = 10_055;
    pub const SEQ_MISORDERED: u32 = 10_063;
    pub const SEQUENCE_POS: u32 = 10_064;
    pub const REQ_TOO_BIG: u32 = 10_065;
    pub const REP_TOO_BIG: u32 = 10_066;
    pub const REP_TOO_BIG_TO_CACHE: u32 = 10_067;
    pub const RETRY_UNCACHED_REP: u32 = 10_068;
    pub const TOO_MANY_OPS: u32 = 10_070;
    pub const OP_NOT_IN_SESSION: u32 = 10_071;
    pub const CLIENTID_BUSY: u32 = 10_074;
    pub const BAD_HIGH_SLOT: u32 = 10_077;
    pub const DEADSESSION: u32 = 10_078;
    pub const NOT_ONLY_OP: u32 = 10_081;
    pub const WRONG_TYPE: u32 = 10_083;
}

/// The kinds of files, nfs_ftype4.
#[allow(missing_docs, dead_code)]
pub mod kind {
    pub const REG: u32 = 1;
    pub const DIR: u32 = 2;
    pub const BLK: u32 = 3;
    pub const CHR: u32 = 4;
    pub const LNK: u32 = 5;
    pub const SOCK: u32 = 6;
    pub const FIFO: u32 = 7;
}

/// The attributes, by their bits.
#[allow(missing_docs, dead_code)]
pub mod attr {
    pub const SUPPORTED_ATTRS: u32 = 0;
    pub const TYPE: u32 = 1;
    pub const FH_EXPIRE_TYPE: u32 = 2;
    pub const CHANGE: u32 = 3;
    pub const SIZE: u32 = 4;
    pub const LINK_SUPPORT: u32 = 5;
    pub const SYMLINK_SUPPORT: u32 = 6;
    pub const NAMED_ATTR: u32 = 7;
    pub const FSID: u32 = 8;
    pub const UNIQUE_HANDLES: u32 = 9;
    pub const LEASE_TIME: u32 = 10;
    pub const RDATTR_ERROR: u32 = 11;
    pub const ACLSUPPORT: u32 = 13;
    pub const CANSETTIME: u32 = 15;
    pub const CASE_INSENSITIVE: u32 = 16;
    pub const CASE_PRESERVING: u32 = 17;
    pub const CHOWN_RESTRICTED: u32 = 18;
    pub const FILEHANDLE: u32 = 19;
    pub const FILEID: u32 = 20;
    pub const FILES_AVAIL: u32 = 21;
    pub const FILES_FREE: u32 = 22;
    pub const FILES_TOTAL: u32 = 23;
    pub const HOMOGENEOUS: u32 = 26;
    pub const MAXFILESIZE: u32 = 27;
    pub const MAXLINK: u32 = 28;
    pub const MAXNAME: u32 = 29;
    pub const MAXREAD: u32 = 30;
    pub const MAXWRITE: u32 = 31;
    pub const MODE: u32 = 33;
    pub const NO_TRUNC: u32 = 34;
    pub const NUMLINKS: u32 = 35;
    pub const OWNER: u32 = 36;
    pub const OWNER_GROUP: u32 = 37;
    pub const RAWDEV: u32 = 41;
    pub const SPACE_AVAIL: u32 = 42;
    pub const SPACE_FREE: u32 = 43;
    pub const SPACE_TOTAL: u32 = 44;
    pub const SPACE_USED: u32 = 45;
    pub const TIME_ACCESS: u32 = 47;
    pub const TIME_ACCESS_SET: u32 = 48;
    pub const TIME_DELTA: u32 = 51;
    pub const TIME_METADATA: u32 = 52;
    pub const TIME_MODIFY: u32 = 53;
    pub const TIME_MODIFY_SET: u32 = 54;
    pub const MOUNTED_ON_FILEID: u32 = 55;
    pub const FS_LAYOUT_TYPES: u32 = 62;
    pub const MODE_SET_MASKED: u32 = 74;
    pub const SUPPATTR_EXCLCREAT: u32 = 75;
    pub const CHANGE_ATTR_TYPE: u32 = 79;
}

/// A set of attributes, a bitmap4 of three words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs(pub [u32; 3]);

impl Attrs {
    /// The set of `bits`.
    pub const fn of(bits: &[u32]) -> Attrs {
        let mut words = [0; 3];
        let mut i = 0;
        while i < bits.len() {
            words[(bits[i] / 32) as usize] |= 1 << (bits[i] % 32);
            i += 1;
        }
        Attrs(words)
    }

    /// The set a bitmap4's `words` name, those past the third left out.
    pub fn from_words(words: &[u32]) -> Attrs {
        let mut out = [0; 3];
        for (o, w) in out.iter_mut().zip(words) {
            *o = *w;
        }
        Attrs(out)
    }

    /// Whether `bit` is in it.
    pub fn has(self, bit: u32) -> bool {
        bit < 96 && self.0[(bit / 32) as usize] & (1 << (bit % 32)) != 0
    }

    /// Those in both.
    pub fn and(self, other: Attrs) -> Attrs {
        Attrs([self.0[0] & other.0[0], self.0[1] & other.0[1], self.0[2] & other.0[2]])
    }

    /// Those in it and not in `other`.
    pub fn without(self, other: Attrs) -> Attrs {
        Attrs([self.0[0] & !other.0[0], self.0[1] & !other.0[1], self.0[2] & !other.0[2]])
    }

    /// Whether it is empty.
    pub fn is_empty(self) -> bool {
        self.0 == [0; 3]
    }

    /// Its bits, lowest first.
    pub fn bits(self) -> impl Iterator<Item = u32> {
        (0..96).filter(move |&bit| self.has(bit))
    }

    /// Adds `bit`.
    pub fn add(&mut self, bit: u32) {
        self.0[(bit / 32) as usize] |= 1 << (bit % 32);
    }
}

/// The attributes nfsd gives.
pub const SUPPORTED: Attrs = Attrs::of(&[
    attr::SUPPORTED_ATTRS,
    attr::TYPE,
    attr::FH_EXPIRE_TYPE,
    attr::CHANGE,
    attr::SIZE,
    attr::LINK_SUPPORT,
    attr::SYMLINK_SUPPORT,
    attr::NAMED_ATTR,
    attr::FSID,
    attr::UNIQUE_HANDLES,
    attr::LEASE_TIME,
    attr::RDATTR_ERROR,
    attr::ACLSUPPORT,
    attr::CANSETTIME,
    attr::CASE_INSENSITIVE,
    attr::CASE_PRESERVING,
    attr::CHOWN_RESTRICTED,
    attr::FILEHANDLE,
    attr::FILEID,
    attr::FILES_AVAIL,
    attr::FILES_FREE,
    attr::FILES_TOTAL,
    attr::HOMOGENEOUS,
    attr::MAXFILESIZE,
    attr::MAXLINK,
    attr::MAXNAME,
    attr::MAXREAD,
    attr::MAXWRITE,
    attr::MODE,
    attr::NO_TRUNC,
    attr::NUMLINKS,
    attr::OWNER,
    attr::OWNER_GROUP,
    attr::RAWDEV,
    attr::SPACE_AVAIL,
    attr::SPACE_FREE,
    attr::SPACE_TOTAL,
    attr::SPACE_USED,
    attr::TIME_ACCESS,
    attr::TIME_ACCESS_SET,
    attr::TIME_DELTA,
    attr::TIME_METADATA,
    attr::TIME_MODIFY,
    attr::TIME_MODIFY_SET,
    attr::MOUNTED_ON_FILEID,
    attr::FS_LAYOUT_TYPES,
    attr::MODE_SET_MASKED,
    attr::SUPPATTR_EXCLCREAT,
    attr::CHANGE_ATTR_TYPE,
]);

/// The attributes a client may set.
pub const SETTABLE: Attrs = Attrs::of(&[
    attr::SIZE,
    attr::MODE,
    attr::OWNER,
    attr::OWNER_GROUP,
    attr::TIME_ACCESS_SET,
    attr::TIME_MODIFY_SET,
    attr::MODE_SET_MASKED,
]);

/// Those an exclusive create (EXCLUSIVE4_1) sets with the file: all but
/// the times, which hold its verifier until the client sets them.
pub const EXCLUSIVE_SETTABLE: Attrs =
    Attrs::of(&[attr::SIZE, attr::MODE, attr::OWNER, attr::OWNER_GROUP, attr::MODE_SET_MASKED]);

/// How long a client's state lasts without a call from it: its lease.
pub const LEASE_SECONDS: u32 = 90;

/// The most bytes a READ answers and a WRITE takes.
pub const IO_MAX: u32 = 1 << 20;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_of_attributes() {
        let a = Attrs::of(&[attr::TYPE, attr::MODE, attr::MOUNTED_ON_FILEID]);
        assert_eq!(a.0, [1 << 1, (1 << 1) | (1 << 23), 0]);
        assert!(a.has(attr::MODE) && !a.has(attr::SIZE) && !a.has(200));
        assert_eq!(a.bits().collect::<Vec<_>>(), [1, 33, 55]);
        assert!(SUPPORTED.has(attr::CHANGE_ATTR_TYPE));
        assert!(SETTABLE.without(SUPPORTED).is_empty());
    }
}
