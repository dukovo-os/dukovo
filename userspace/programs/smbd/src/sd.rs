// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Security descriptors (MS-DTYP 2.4.6) made from POSIX access lists, and
//! access lists made from them (ADR 0229): what a PC's Security tab, or
//! `icacls` or `smbcacls`, shows and changes.
//!
//! A file's owner and group are given as the SIDs Samba gives Unix's
//! users and groups, S-1-22-1-uid and S-1-22-2-gid, which PCs and Linux's
//! cifs know. Its access list is a DACL of allowing ACEs: the owner's
//! entry for the owner's SID, each named user's and group's for theirs,
//! the group's for the group's and the others' for Everyone, each limited
//! by the mask. A directory's default list follows as the same ACEs again,
//! inherit-only, with CREATOR OWNER and CREATOR GROUP for its owner's and
//! group's entries.
//!
//! A DACL a PC sets goes the other way. Its allowing ACEs give the
//! permissions their rights include (reading, writing, running), the ones
//! that inherit make a directory's default list, and the mask is the
//! group class's permissions together. Denying ACEs, and SIDs that name no
//! one here, are refused, so that what a PC is told was set is what was
//! set; the kernel then decides whether the person may set it.

use std::fmt;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use acl::{Acl, Entry, NO_ID, Tag};

use crate::fs::access;
use crate::wire::{self, Out, Status, status, status_of};

/// The parts of a descriptor QUERY_INFO asks for and SET_INFO sets.
pub mod part {
    pub const OWNER: u32 = 0x1;
    pub const GROUP: u32 = 0x2;
    pub const DACL: u32 = 0x4;
}

/// A descriptor's control bits.
mod control {
    pub const DACL_PRESENT: u16 = 0x0004;
    pub const SELF_RELATIVE: u16 = 0x8000;
}

/// The ACE types: the two an access list has.
pub mod ace {
    pub const ALLOWED: u8 = 0;
    pub const DENIED: u8 = 1;
}

/// An ACE's inheritance flags.
pub mod inherit {
    pub const OBJECT: u8 = 0x01;
    pub const CONTAINER: u8 = 0x02;
    pub const ONLY: u8 = 0x08;
}

/// The statuses a descriptor that cannot be set is refused with.
mod refused {
    pub const INVALID_OWNER: u32 = 0xc000_005a;
    pub const INVALID_PRIMARY_GROUP: u32 = 0xc000_005b;
    pub const NONE_MAPPED: u32 = 0xc000_0073;
    pub const INVALID_ACL: u32 = 0xc000_0077;
    pub const INVALID_SID: u32 = 0xc000_0078;
    pub const INVALID_SECURITY_DESCR: u32 = 0xc000_0079;
}
pub use refused::NONE_MAPPED;

/// A SID (MS-DTYP 2.4.2).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sid {
    /// Its identifier authority, 48 bits.
    pub authority: u64,
    /// Its subauthorities, at most 15.
    pub subs: Vec<u32>,
}

impl Sid {
    /// The SID of `authority` and `subs`.
    pub fn new(authority: u64, subs: &[u32]) -> Sid {
        Sid { authority, subs: subs.to_vec() }
    }

    /// The SID at the start of `bytes`, and how long it is.
    pub fn parse(bytes: &[u8]) -> Option<(Sid, usize)> {
        let (&revision, rest) = bytes.split_first()?;
        let count = usize::from(*rest.first()?);
        if revision != 1 || count > 15 {
            return None;
        }
        let length = 8 + 4 * count;
        let raw = bytes.get(..length)?;
        let authority = raw[2..8].iter().fold(0, |sum, &b| sum << 8 | u64::from(b));
        let (subs, _) = raw[8..].as_chunks::<4>();
        let subs = subs.iter().map(|sub| u32::from_le_bytes(*sub)).collect();
        Some((Sid { authority, subs }, length))
    }

    /// Its bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Out::new();
        out.u8(1).u8(self.subs.len() as u8).bytes(&self.authority.to_be_bytes()[2..]);
        for &sub in &self.subs {
            out.u32(sub);
        }
        out.0
    }

    /// The SID of the domain it is in, and its number there: all but its
    /// last subauthority, and that one.
    pub fn split(&self) -> Option<(Sid, u32)> {
        let (&rid, domain) = self.subs.split_last()?;
        Some((Sid::new(self.authority, domain), rid))
    }
}

/// Its text: S-1-22-1-1000.
impl fmt::Display for Sid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "S-1-{}", self.authority)?;
        for sub in &self.subs {
            write!(f, "-{sub}")?;
        }
        Ok(())
    }
}

/// Whom a SID names here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// A user, by number: S-1-22-1-uid.
    User(u32),
    /// A group, by number: S-1-22-2-gid.
    Group(u32),
    /// Everyone: S-1-1-0.
    Everyone,
    /// Whoever makes a file, in an ACE it inherits: S-1-3-0.
    CreatorOwner,
    /// The group of whoever makes a file: S-1-3-1.
    CreatorGroup,
}

impl Who {
    /// Whom `sid` names, if anyone here.
    pub fn of(sid: &Sid) -> Option<Who> {
        Some(match (sid.authority, sid.subs.as_slice()) {
            (22, &[1, uid]) if uid != NO_ID => Who::User(uid),
            (22, &[2, gid]) if gid != NO_ID => Who::Group(gid),
            (1, [0]) => Who::Everyone,
            (3, [0]) => Who::CreatorOwner,
            (3, [1]) => Who::CreatorGroup,
            _ => return None,
        })
    }

    /// Its SID.
    pub fn sid(self) -> Sid {
        match self {
            Who::User(uid) => Sid::new(22, &[1, uid]),
            Who::Group(gid) => Sid::new(22, &[2, gid]),
            Who::Everyone => Sid::new(1, &[0]),
            Who::CreatorOwner => Sid::new(3, &[0]),
            Who::CreatorGroup => Sid::new(3, &[1]),
        }
    }
}

/// The rights of an entry's permissions, `rwx`, as an access mask: all of
/// them, FILE_ALL_ACCESS, for all three, which a PC shows as full
/// control. Anyone may read a file's attributes and its descriptor, and
/// the owner changes its modes and its access list.
pub fn rights(perm: u8, owner: bool) -> u32 {
    if perm & 7 == 7 {
        return access::ALL;
    }
    let mut mask = access::READ_ATTRIBUTES | access::READ_CONTROL | access::SYNCHRONIZE;
    if perm & 4 != 0 {
        mask |= access::READ;
    }
    if perm & 2 != 0 {
        mask |= access::WRITE | access::DELETE | access::DELETE_CHILD;
    }
    if perm & 1 != 0 {
        mask |= access::RUN;
    }
    if owner {
        mask |= access::WRITE_DAC | access::WRITE_ATTRIBUTES;
    }
    mask
}

/// The permissions an access mask gives: read for reading the data (or
/// listing a folder), write for writing or adding to it, execute for
/// running it (or going through a folder).
pub fn perm_of(mask: u32) -> u8 {
    let all = mask & access::GENERIC_ALL != 0;
    let mut perm = 0;
    if all || mask & (access::READ_DATA | access::GENERIC_READ) != 0 {
        perm |= 4;
    }
    if all || mask & (access::WRITE_DATA | access::APPEND_DATA | access::GENERIC_WRITE) != 0 {
        perm |= 2;
    }
    if all || mask & (access::EXECUTE | access::GENERIC_EXECUTE) != 0 {
        perm |= 1;
    }
    perm
}

/// What a file's descriptor is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Security {
    /// Its owner.
    pub uid: u32,
    /// Its group.
    pub gid: u32,
    /// Its access list, from its mode if it has none of its own.
    pub access: Acl,
    /// A directory's default list, if it has one.
    pub default: Option<Acl>,
}

impl Security {
    /// The file's at `path`, of `meta`.
    ///
    /// # Errors
    ///
    /// Reading its access lists'.
    pub fn of(path: &Path, meta: &Metadata) -> Result<Security, Status> {
        let access = acl::access_list(path, meta.mode(), false).map_err(|err| status_of(&err))?;
        let default = if meta.is_dir() {
            acl::default_list(path, false).map_err(|err| status_of(&err))?
        } else {
            None
        };
        Ok(Security { uid: meta.uid(), gid: meta.gid(), access, default })
    }
}

/// Sets the `parts` of `given` on the file at `path`: its owner and
/// group with chown(2), then its access lists. Everything is checked
/// before anything is changed; the kernel decides whether the person may
/// change it.
///
/// # Errors
///
/// What [`lists`], [`owner_of`] and [`group_of`] refuse, `NOT_SUPPORTED`
/// for a DACL asked for that the descriptor does not have, and the
/// filesystem's.
pub fn set(path: &Path, given: &Given, parts: u32) -> Result<(), Status> {
    let meta = fs::metadata(path).map_err(|err| status_of(&err))?;
    let uid = if parts & part::OWNER != 0 { owner_of(given)? } else { meta.uid() };
    let gid = if parts & part::GROUP != 0 { group_of(given)? } else { meta.gid() };
    let lists = if parts & part::DACL == 0 {
        None
    } else {
        // No DACL is a null one, which would let anyone do anything.
        let aces = given.dacl.as_ref().ok_or(status::NOT_SUPPORTED)?;
        Some(lists(aces, uid, gid, meta.is_dir())?)
    };
    if uid != meta.uid() || gid != meta.gid() {
        let changed = |old: u32, new: u32| (new != old).then_some(new);
        std::os::unix::fs::chown(path, changed(meta.uid(), uid), changed(meta.gid(), gid))
            .map_err(|err| status_of(&err))?;
    }
    if let Some((access, default)) = lists {
        // chown(2) may have taken set-user-ID away: the mode as it is now.
        let mode = fs::metadata(path).map_err(|err| status_of(&err))?.mode();
        acl::write_access(path, &access, mode).map_err(|err| status_of(&err))?;
        if meta.is_dir() {
            acl::write_default(path, default.as_ref()).map_err(|err| status_of(&err))?;
        }
    }
    Ok(())
}

/// An ACE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ace {
    /// Allowing, denying, or another type.
    pub kind: u8,
    /// Its inheritance flags.
    pub flags: u8,
    /// Its access mask.
    pub mask: u32,
    /// Whom it is for.
    pub sid: Sid,
}

/// The ACEs of a list's entries: `owner` and `group` for its owner's and
/// group's, each named one's SID for theirs, Everyone for the others'.
fn aces_of(list: &Acl, owner: Who, group: Who, flags: u8) -> Vec<Ace> {
    let mask = list.mask().unwrap_or(7);
    let mut aces = Vec::new();
    for entry in &list.entries {
        let perm = if entry.tag.masked() { entry.perm & mask } else { entry.perm };
        let (who, is_owner) = match entry.tag {
            Tag::UserObj => (owner, true),
            Tag::User => (Who::User(entry.id), false),
            Tag::GroupObj => (group, false),
            Tag::Group => (Who::Group(entry.id), false),
            Tag::Mask => continue,
            Tag::Other => (Who::Everyone, false),
        };
        aces.push(Ace { kind: ace::ALLOWED, flags, mask: rights(perm, is_owner), sid: who.sid() });
    }
    aces
}

/// The DACL's ACEs of a file.
pub fn aces(security: &Security) -> Vec<Ace> {
    let mut aces = aces_of(&security.access, Who::User(security.uid), Who::Group(security.gid), 0);
    if let Some(default) = &security.default {
        let flags = inherit::OBJECT | inherit::CONTAINER | inherit::ONLY;
        aces.extend(aces_of(default, Who::CreatorOwner, Who::CreatorGroup, flags));
    }
    aces
}

/// A self-relative security descriptor of a file, with the `parts` asked
/// for: owner, group and DACL.
pub fn descriptor(security: &Security, parts: u32) -> Vec<u8> {
    let mut out = Out::new();
    let mut flags = control::SELF_RELATIVE;
    if parts & part::DACL != 0 {
        flags |= control::DACL_PRESENT;
    }
    out.u8(1).u8(0).u16(flags).u32(0).u32(0).u32(0).u32(0);
    let put = |out: &mut Out, field: usize, bytes: &[u8]| {
        let at = out.len() as u32;
        out.set_u32(field, at);
        out.bytes(bytes);
    };
    if parts & part::OWNER != 0 {
        put(&mut out, 4, &Who::User(security.uid).sid().bytes());
    }
    if parts & part::GROUP != 0 {
        put(&mut out, 8, &Who::Group(security.gid).sid().bytes());
    }
    if parts & part::DACL != 0 {
        let aces = aces(security);
        let mut dacl = Out::new();
        dacl.u8(2).u8(0).u16(0).u16(aces.len() as u16).u16(0);
        for ace in &aces {
            let sid = ace.sid.bytes();
            dacl.u8(ace.kind).u8(ace.flags).u16((8 + sid.len()) as u16).u32(ace.mask).bytes(&sid);
        }
        let size = dacl.len() as u16;
        dacl.set_u16(2, size);
        put(&mut out, 16, &dacl.0);
    }
    out.0
}

/// What a descriptor a PC sets has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Given {
    /// Its owner, if it has one.
    pub owner: Option<Sid>,
    /// Its group, if it has one.
    pub group: Option<Sid>,
    /// Its DACL's ACEs, if it has a DACL.
    pub dacl: Option<Vec<Ace>>,
}

/// The descriptor in `bytes`, self-relative as SET_INFO has it. A DACL
/// that is present but null, which would let anyone do anything, is
/// refused.
///
/// # Errors
///
/// `STATUS_INVALID_SECURITY_DESCR`, `STATUS_INVALID_SID` or
/// `STATUS_INVALID_ACL` for one that does not read, and
/// `STATUS_NOT_SUPPORTED` for a null DACL.
pub fn parse(bytes: &[u8]) -> Result<Given, Status> {
    let bad = refused::INVALID_SECURITY_DESCR;
    let flags = wire::u16_at(bytes, 2).map_err(|_| bad)?;
    if bytes.first() != Some(&1) || flags & control::SELF_RELATIVE == 0 {
        return Err(bad);
    }
    let offset = |field: usize| -> Result<usize, Status> {
        Ok(wire::u32_at(bytes, field).map_err(|_| bad)? as usize)
    };
    let sid = |at: usize| -> Result<Option<Sid>, Status> {
        if at == 0 {
            return Ok(None);
        }
        let (sid, _) = bytes.get(at..).and_then(Sid::parse).ok_or(refused::INVALID_SID)?;
        Ok(Some(sid))
    };
    let owner = sid(offset(4)?)?;
    let group = sid(offset(8)?)?;
    let dacl = if flags & control::DACL_PRESENT == 0 {
        None
    } else {
        match offset(16)? {
            0 => return Err(status::NOT_SUPPORTED),
            at => Some(parse_acl(bytes.get(at..).ok_or(refused::INVALID_ACL)?)?),
        }
    };
    Ok(Given { owner, group, dacl })
}

/// The ACEs of the ACL at the start of `bytes`.
fn parse_acl(bytes: &[u8]) -> Result<Vec<Ace>, Status> {
    let bad = refused::INVALID_ACL;
    let size = usize::from(wire::u16_at(bytes, 2).map_err(|_| bad)?);
    let count = usize::from(wire::u16_at(bytes, 4).map_err(|_| bad)?);
    let acl = bytes.get(..size).ok_or(bad)?;
    if !matches!(bytes.first(), Some(2 | 4)) || size < 8 || count > (size - 8) / 8 {
        return Err(bad);
    }
    let mut aces = Vec::with_capacity(count);
    let mut at = 8;
    for _ in 0..count {
        let kind = wire::u8_at(acl, at).map_err(|_| bad)?;
        let flags = wire::u8_at(acl, at + 1).map_err(|_| bad)?;
        let length = usize::from(wire::u16_at(acl, at + 2).map_err(|_| bad)?);
        let raw = acl.get(at..at + length).ok_or(bad)?;
        if length < 8 {
            return Err(bad);
        }
        let mask = wire::u32_at(raw, 4).map_err(|_| bad)?;
        // An object ACE, or one with conditions, has more before or after
        // its SID: read as a SID it is of no one here, and refused.
        let sid = match Sid::parse(&raw[8..]) {
            Some((sid, _)) => sid,
            None if kind > ace::DENIED => Sid::new(0, &[]),
            None => return Err(refused::INVALID_SID),
        };
        aces.push(Ace { kind, flags, mask, sid });
        at += length;
    }
    Ok(aces)
}

/// The entries `aces` give one list, each entry's permissions those of
/// all its ACEs together.
fn gather(list: &mut Acl, tag: Tag, id: u32, perm: u8) {
    let had = list.get(tag, id).map_or(0, |e| e.perm);
    list.set(Entry { tag, id, perm: had | perm });
}

/// Gives `list` the owner's, group's and others' entries it lacks, from
/// `from` where it has them and with no permissions where not, then the
/// mask its named entries need.
fn complete(list: &mut Acl, from: Option<&Acl>) {
    for tag in [Tag::UserObj, Tag::GroupObj, Tag::Other] {
        if list.get(tag, NO_ID).is_none() {
            let perm = from.and_then(|from| from.get(tag, NO_ID)).map_or(0, |e| e.perm);
            list.set(Entry { tag, id: NO_ID, perm });
        }
    }
    if list.has_named() {
        let mask = list.calc_mask();
        list.set(Entry { tag: Tag::Mask, id: NO_ID, perm: mask });
    } else {
        list.remove(Tag::Mask, NO_ID);
    }
}

/// The access list a DACL gives a file owned by `uid` and `gid`, and the
/// default list it gives a directory (none if none of its ACEs inherit).
///
/// An ACE for the owner's SID gives the owner's entry, one for the
/// group's the group's, one for Everyone the others'; other users and
/// groups get named entries. ACEs that inherit give a directory's default
/// list in the same way, with CREATOR OWNER and CREATOR GROUP for its
/// owner's and group's entries. A default list without its owner's or
/// group's entry takes the directory's own, as setfacl(1) fills one in,
/// and one without Everyone gives the others nothing, as on a PC.
///
/// # Errors
///
/// `STATUS_NOT_SUPPORTED` for a denying ACE or one of another type, and
/// `STATUS_NONE_MAPPED` for one whose SID is no one here.
pub fn lists(
    aces: &[Ace],
    uid: u32,
    gid: u32,
    directory: bool,
) -> Result<(Acl, Option<Acl>), Status> {
    let mut access = Acl::default();
    let mut default: Option<Acl> = None;
    for ace in aces {
        if ace.kind != ace::ALLOWED {
            return Err(status::NOT_SUPPORTED);
        }
        let who = Who::of(&ace.sid).ok_or(NONE_MAPPED)?;
        let perm = perm_of(ace.mask);
        if ace.flags & inherit::ONLY == 0 {
            let entry = match who {
                Who::User(id) if id == uid => Some((Tag::UserObj, NO_ID)),
                Who::User(id) => Some((Tag::User, id)),
                Who::Group(id) if id == gid => Some((Tag::GroupObj, NO_ID)),
                Who::Group(id) => Some((Tag::Group, id)),
                Who::Everyone => Some((Tag::Other, NO_ID)),
                // Only what a file is made with has a creator.
                Who::CreatorOwner | Who::CreatorGroup => None,
            };
            if let Some((tag, id)) = entry {
                gather(&mut access, tag, id, perm);
            }
        }
        if directory && ace.flags & (inherit::OBJECT | inherit::CONTAINER) != 0 {
            let (tag, id) = match who {
                Who::CreatorOwner => (Tag::UserObj, NO_ID),
                Who::CreatorGroup => (Tag::GroupObj, NO_ID),
                Who::User(id) => (Tag::User, id),
                Who::Group(id) => (Tag::Group, id),
                Who::Everyone => (Tag::Other, NO_ID),
            };
            gather(default.get_or_insert_default(), tag, id, perm);
        }
    }
    complete(&mut access, None);
    if let Some(default) = &mut default {
        let mut base = access.clone();
        base.remove(Tag::Other, NO_ID);
        complete(default, Some(&base));
    }
    Ok((access, default))
}

/// The owner a descriptor names, a user here.
///
/// # Errors
///
/// `STATUS_INVALID_OWNER` for none, or a SID that is not a user here.
pub fn owner_of(given: &Given) -> Result<u32, Status> {
    match given.owner.as_ref().and_then(Who::of) {
        Some(Who::User(uid)) => Ok(uid),
        _ => Err(refused::INVALID_OWNER),
    }
}

/// The group a descriptor names, a group here.
///
/// # Errors
///
/// `STATUS_INVALID_PRIMARY_GROUP` for none, or a SID that is not a
/// group here.
pub fn group_of(given: &Given) -> Result<u32, Status> {
    match given.group.as_ref().and_then(Who::of) {
        Some(Who::Group(gid)) => Ok(gid),
        _ => Err(refused::INVALID_PRIMARY_GROUP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[(Tag, u32, u8)]) -> Acl {
        Acl { entries: entries.iter().map(|&(tag, id, perm)| Entry { tag, id, perm }).collect() }
    }

    fn security(access: Acl, default: Option<Acl>) -> Security {
        Security { uid: 1000, gid: 100, access, default }
    }

    const ALL: u32 = part::OWNER | part::GROUP | part::DACL;

    #[test]
    fn sids_read_write_and_name() {
        let sid = Who::User(1000).sid();
        let bytes = sid.bytes();
        assert_eq!(bytes, [1, 2, 0, 0, 0, 0, 0, 22, 1, 0, 0, 0, 0xe8, 3, 0, 0]);
        assert_eq!(Sid::parse(&bytes), Some((sid.clone(), 16)));
        assert_eq!(sid.to_string(), "S-1-22-1-1000");
        assert_eq!(Who::of(&sid), Some(Who::User(1000)));
        assert_eq!(sid.split(), Some((Sid::new(22, &[1]), 1000)));
        assert_eq!(Who::Everyone.sid().to_string(), "S-1-1-0");
        // Administrators, a number with no user, and short bytes.
        assert_eq!(Who::of(&Sid::new(5, &[32, 544])), None);
        assert_eq!(Who::of(&Sid::new(22, &[1, NO_ID])), None);
        assert_eq!(Sid::parse(&bytes[..15]), None);
        assert_eq!(Sid::parse(&[2, 0, 0, 0, 0, 0, 0, 1]), None);
    }

    #[test]
    fn rights_and_permissions_go_both_ways() {
        for perm in 0..8 {
            for owner in [false, true] {
                assert_eq!(perm_of(rights(perm, owner)), perm, "{perm} {owner}");
            }
        }
        assert_eq!(rights(7, false), access::ALL);
        assert_eq!(rights(0, false), 0x0012_0080);
        assert_eq!(perm_of(access::GENERIC_ALL), 7);
        assert_eq!(perm_of(access::GENERIC_READ | access::GENERIC_EXECUTE), 5);
        assert_eq!(perm_of(access::APPEND_DATA), 2);
    }

    #[test]
    fn a_mode_is_three_aces() {
        let sec = security(Acl::from_mode(0o640), None);
        let all = descriptor(&sec, ALL);
        // Revision 1, self-relative with a DACL.
        assert_eq!(&all[..4], &[1, 0, 0x04, 0x80]);
        let owner = wire::u32_at(&all, 4).unwrap() as usize;
        assert_eq!(&all[owner..owner + 8], &[1, 2, 0, 0, 0, 0, 0, 22]);
        let given = parse(&all).unwrap();
        assert_eq!(owner_of(&given), Ok(1000));
        assert_eq!(group_of(&given), Ok(100));
        let aces = given.dacl.unwrap();
        let masks: Vec<_> = aces.iter().map(|a| (a.sid.to_string(), perm_of(a.mask))).collect();
        assert_eq!(
            masks,
            [("S-1-22-1-1000".into(), 6), ("S-1-22-2-100".into(), 4), ("S-1-1-0".into(), 0)]
        );
        let only_owner = descriptor(&sec, part::OWNER);
        assert_eq!(wire::u32_at(&only_owner, 16), Ok(0));
        assert_eq!(parse(&only_owner).unwrap().dacl, None);
    }

    #[test]
    fn named_entries_and_the_mask_show_what_they_allow() {
        let access = list(&[
            (Tag::UserObj, NO_ID, 7),
            (Tag::User, 1001, 7),
            (Tag::GroupObj, NO_ID, 5),
            (Tag::Group, 200, 6),
            (Tag::Mask, NO_ID, 5),
            (Tag::Other, NO_ID, 0),
        ]);
        let shown = aces(&security(access, None));
        let perms: Vec<_> =
            shown.iter().map(|a| (Who::of(&a.sid).unwrap(), perm_of(a.mask))).collect();
        assert_eq!(
            perms,
            [
                (Who::User(1000), 7),
                (Who::User(1001), 5),
                (Who::Group(100), 5),
                (Who::Group(200), 4),
                (Who::Everyone, 0),
            ]
        );
        // Set back, the mask is what they have together.
        let (back, default) = lists(&shown, 1000, 100, false).unwrap();
        assert_eq!(default, None);
        assert_eq!(
            back,
            list(&[
                (Tag::UserObj, NO_ID, 7),
                (Tag::User, 1001, 5),
                (Tag::GroupObj, NO_ID, 5),
                (Tag::Group, 200, 4),
                (Tag::Mask, NO_ID, 5),
                (Tag::Other, NO_ID, 0),
            ])
        );
    }

    #[test]
    fn a_default_list_inherits() {
        let access = Acl::from_mode(0o750);
        let default = list(&[
            (Tag::UserObj, NO_ID, 7),
            (Tag::User, 1001, 5),
            (Tag::GroupObj, NO_ID, 5),
            (Tag::Mask, NO_ID, 5),
            (Tag::Other, NO_ID, 0),
        ]);
        let sec = security(access.clone(), Some(default.clone()));
        let shown = aces(&sec);
        assert_eq!(shown.len(), 3 + 4);
        assert_eq!(shown[3].sid, Who::CreatorOwner.sid());
        assert_eq!(shown[3].flags, 0x0b);
        assert_eq!(shown[5].sid, Who::CreatorGroup.sid());
        let given = parse(&descriptor(&sec, ALL)).unwrap();
        let (a, d) = lists(&given.dacl.unwrap(), 1000, 100, true).unwrap();
        assert_eq!((a, d), (access, Some(default)));
        // A file has no default list, whatever its ACEs say.
        assert_eq!(lists(&shown, 1000, 100, false).unwrap().1, None);
    }

    #[test]
    fn a_pcs_dacl_becomes_lists() {
        let ace = |who: Who, flags: u8, mask: u32| Ace {
            kind: ace::ALLOWED,
            flags,
            mask,
            sid: who.sid(),
        };
        let both = inherit::OBJECT | inherit::CONTAINER;
        // Alice's folder, Bob may read it and what is made in it, and
        // Everyone may go through it.
        let aces = [
            ace(Who::User(1000), both, access::ALL),
            ace(Who::User(1001), both, access::GENERIC_READ | access::GENERIC_EXECUTE),
            ace(Who::Everyone, 0, access::EXECUTE),
        ];
        let (a, d) = lists(&aces, 1000, 100, true).unwrap();
        assert_eq!(
            a,
            list(&[
                (Tag::UserObj, NO_ID, 7),
                (Tag::User, 1001, 5),
                (Tag::GroupObj, NO_ID, 0),
                (Tag::Mask, NO_ID, 5),
                (Tag::Other, NO_ID, 1),
            ])
        );
        // The owner's entry and the group's come from the folder's own;
        // Everyone's did not inherit.
        assert_eq!(
            d,
            Some(list(&[
                (Tag::UserObj, NO_ID, 7),
                (Tag::User, 1000, 7),
                (Tag::User, 1001, 5),
                (Tag::GroupObj, NO_ID, 0),
                (Tag::Mask, NO_ID, 7),
                (Tag::Other, NO_ID, 0),
            ]))
        );
        // Two ACEs for one SID add up; an empty DACL allows no one.
        let twice =
            [ace(Who::Everyone, 0, access::READ_DATA), ace(Who::Everyone, 0, access::WRITE_DATA)];
        assert_eq!(lists(&twice, 1, 1, false).unwrap().0.get(Tag::Other, NO_ID).unwrap().perm, 6);
        assert_eq!(lists(&[], 1, 1, false).unwrap().0, Acl::from_mode(0));
    }

    #[test]
    fn what_cannot_be_kept_is_refused() {
        let deny = Ace { kind: ace::DENIED, flags: 0, mask: access::ALL, sid: Who::Everyone.sid() };
        assert_eq!(lists(&[deny], 1, 1, false), Err(status::NOT_SUPPORTED));
        let administrators =
            Ace { kind: ace::ALLOWED, flags: 0, mask: access::ALL, sid: Sid::new(5, &[32, 544]) };
        assert_eq!(lists(&[administrators], 1, 1, false), Err(NONE_MAPPED));
        let mut sd = descriptor(&security(Acl::from_mode(0o644), None), ALL);
        assert_eq!(owner_of(&Given::default()), Err(refused::INVALID_OWNER));
        let wrong = Given {
            owner: Some(Who::Group(5).sid()),
            group: Some(Who::User(5).sid()),
            ..Given::default()
        };
        assert_eq!(owner_of(&wrong), Err(refused::INVALID_OWNER));
        assert_eq!(group_of(&wrong), Err(refused::INVALID_PRIMARY_GROUP));
        // A null DACL, a descriptor that is not self-relative, and an
        // ACE that runs past its ACL.
        let mut null = sd.clone();
        null[16..20].copy_from_slice(&[0; 4]);
        assert_eq!(parse(&null), Err(status::NOT_SUPPORTED));
        let mut absolute = sd.clone();
        absolute[3] = 0;
        assert_eq!(parse(&absolute), Err(refused::INVALID_SECURITY_DESCR));
        let dacl = wire::u32_at(&sd, 16).unwrap() as usize;
        sd[dacl + 8 + 2] = 200;
        assert_eq!(parse(&sd), Err(refused::INVALID_ACL));
        // Every cut of a descriptor is refused or read, never a panic.
        let whole = descriptor(&security(Acl::from_mode(0o644), Some(Acl::from_mode(0o700))), ALL);
        for end in 0..whole.len() {
            let _ = parse(&whole[..end]);
        }
    }
}
