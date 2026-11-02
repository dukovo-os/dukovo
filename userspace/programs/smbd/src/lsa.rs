// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lsarpc pipe of IPC$: enough of the Local Security Authority's
//! remote protocol (MS-LSAT) for a PC to name the SIDs of a security
//! descriptor and to find the SID of a name, as the Security tab,
//! `icacls` and `smbcacls` do: LsarOpenPolicy and LsarOpenPolicy2,
//! LsarLookupSids and LsarLookupSids2, LsarLookupNames, LsarLookupNames2
//! and LsarLookupNames3, and LsarClose, in NDR.
//!
//! Users are named in the domain `Unix User` and groups in `Unix Group`,
//! as Samba names the SIDs S-1-22-1-uid and S-1-22-2-gid ([`crate::sd`],
//! ADR 0229), so that a user and a group of one name stay apart. Only the
//! person's own session asks, and it learns no more than the user and
//! group databases tell anyone on the machine.

use crate::rpc::{BAD_STUB_DATA, NCA_OP_RNG_ERROR, Ndr, Server, Writer};
use crate::sd::{Sid, Who};
use crate::sys;

/// The operations.
const CLOSE: u16 = 0;
const OPEN_POLICY: u16 = 6;
const LOOKUP_NAMES: u16 = 14;
const LOOKUP_SIDS: u16 = 15;
const OPEN_POLICY2: u16 = 44;
const LOOKUP_SIDS2: u16 = 57;
const LOOKUP_NAMES2: u16 = 58;
const LOOKUP_NAMES3: u16 = 68;

/// The statuses.
const SUCCESS: u32 = 0;
const SOME_NOT_MAPPED: u32 = 0x0000_0107;
const INVALID_HANDLE: u32 = 0xc000_0008;
const NONE_MAPPED: u32 = 0xc000_0073;
const INSUFFICIENT_RESOURCES: u32 = 0xc000_009a;

/// What a SID is (SID_NAME_USE).
mod kind {
    pub const USER: u16 = 1;
    pub const GROUP: u16 = 2;
    pub const WELL_KNOWN_GROUP: u16 = 5;
    pub const UNKNOWN: u16 = 8;
}

/// The most SIDs or names one lookup takes (MS-LSAT's ranges).
const SIDS_MAX: usize = 20_480;
const NAMES_MAX: usize = 1000;
/// The most policy handles a pipe has open at once.
const POLICIES_MAX: usize = 16;

/// The names of users and groups, both ways.
pub trait Names {
    /// User `uid`'s name.
    fn user(&self, uid: u32) -> Option<String>;
    /// Group `gid`'s name.
    fn group(&self, gid: u32) -> Option<String>;
    /// The number of the user called `name`.
    fn uid(&self, name: &str) -> Option<u32>;
    /// The number of the group called `name`.
    fn gid(&self, name: &str) -> Option<u32>;
}

/// The machine's names, from its user and group databases.
pub struct System;

impl Names for System {
    fn user(&self, uid: u32) -> Option<String> {
        sys::user_name(uid)
    }
    fn group(&self, gid: u32) -> Option<String> {
        sys::group_name(gid)
    }
    fn uid(&self, name: &str) -> Option<u32> {
        sys::user_id(name)
    }
    fn gid(&self, name: &str) -> Option<u32> {
        sys::group_id(name)
    }
}

/// The policy handles a pipe has given and not yet closed.
#[derive(Debug, Default)]
pub struct Policies {
    open: Vec<[u8; 20]>,
    made: u32,
}

impl Policies {
    /// A new handle, if the pipe may have another.
    fn open(&mut self) -> Option<[u8; 20]> {
        if self.open.len() >= POLICIES_MAX {
            return None;
        }
        self.made = self.made.wrapping_add(1);
        let mut handle = [0; 20];
        handle[4..8].copy_from_slice(&self.made.to_le_bytes());
        handle[8..20].copy_from_slice(b"dukovo-lsarp");
        self.open.push(handle);
        Some(handle)
    }

    fn is_open(&self, handle: &[u8; 20]) -> bool {
        self.open.contains(handle)
    }

    /// Closes `handle`: whether it was open.
    fn close(&mut self, handle: &[u8; 20]) -> bool {
        let before = self.open.len();
        self.open.retain(|open| open != handle);
        self.open.len() != before
    }
}

/// The domain of the users' SIDs and its name.
fn users() -> (&'static str, Sid) {
    ("Unix User", Sid::new(22, &[1]))
}

/// The domain of the groups' SIDs and its name.
fn groups() -> (&'static str, Sid) {
    ("Unix Group", Sid::new(22, &[2]))
}

/// The well-known SIDs a descriptor here has, and their names.
const WELL_KNOWN: [(Who, &str); 3] = [
    (Who::Everyone, "Everyone"),
    (Who::CreatorOwner, "CREATOR OWNER"),
    (Who::CreatorGroup, "CREATOR GROUP"),
];

/// What a SID or a name was found to be: what it is, its domain, and its
/// name and number there.
struct Found {
    kind: u16,
    domain: (&'static str, Sid),
    name: String,
    rid: u32,
}

impl Found {
    fn sid(&self) -> Sid {
        let mut sid = self.domain.1.clone();
        sid.subs.push(self.rid);
        sid
    }
}

/// The name of `sid`, if it names someone here.
fn by_sid(sid: &Sid, names: &dyn Names) -> Option<Found> {
    let who = Who::of(sid)?;
    let (domain, rid) = sid.split()?;
    let (kind, domain, name) = match who {
        Who::User(uid) => (kind::USER, users(), names.user(uid)?),
        Who::Group(gid) => (kind::GROUP, groups(), names.group(gid)?),
        _ => {
            let (_, name) = WELL_KNOWN.iter().find(|(known, _)| *known == who)?;
            (kind::WELL_KNOWN_GROUP, ("", domain), (*name).to_string())
        }
    };
    Some(Found { kind, domain, name, rid })
}

/// The SID of `name`: `alice`, `Unix User\alice`, `Unix Group\staff`,
/// `SERVER\alice`, or a well-known name. A name alone is a well-known
/// one's, then a user's, then a group's.
fn by_name(name: &str, server: &str, names: &dyn Names) -> Option<Found> {
    let (domain, account) = match name.split_once('\\') {
        Some((domain, account)) => (Some(domain), account),
        None => (None, name),
    };
    let user = || {
        let uid = names.uid(account)?;
        let name = names.user(uid).unwrap_or_else(|| account.to_string());
        Some(Found { kind: kind::USER, domain: users(), name, rid: uid })
    };
    let group = || {
        let gid = names.gid(account)?;
        let name = names.group(gid).unwrap_or_else(|| account.to_string());
        Some(Found { kind: kind::GROUP, domain: groups(), name, rid: gid })
    };
    let well_known = || {
        let (who, known) =
            WELL_KNOWN.iter().find(|(_, known)| known.eq_ignore_ascii_case(account))?;
        let (domain, rid) = who.sid().split()?;
        Some(Found {
            kind: kind::WELL_KNOWN_GROUP,
            domain: ("", domain),
            name: (*known).to_string(),
            rid,
        })
    };
    match domain {
        Some(d) if d.eq_ignore_ascii_case(users().0) => user(),
        Some(d) if d.eq_ignore_ascii_case(groups().0) => group(),
        Some(d) if d.eq_ignore_ascii_case(server) => user().or_else(group),
        Some("") | None => well_known().or_else(user).or_else(group),
        Some(_) => None,
    }
}

/// The domains a lookup's answers are in, each once.
#[derive(Default)]
struct Domains(Vec<(&'static str, Sid)>);

impl Domains {
    /// The index of `domain`, added if it is new.
    fn index(&mut self, domain: &(&'static str, Sid)) -> u32 {
        let at = self.0.iter().position(|known| known == domain).unwrap_or_else(|| {
            self.0.push(domain.clone());
            self.0.len() - 1
        });
        at as u32
    }

    /// The LSAPR_REFERENCED_DOMAIN_LIST, after its pointer.
    fn write(&self, w: &mut Writer) {
        w.pointer();
        w.u32(self.0.len() as u32);
        if self.0.is_empty() {
            w.u32(0);
        } else {
            w.pointer();
        }
        w.u32(self.0.len() as u32);
        if self.0.is_empty() {
            return;
        }
        w.u32(self.0.len() as u32);
        for (name, _) in &self.0 {
            w.unicode(name);
            w.pointer();
        }
        for (name, sid) in &self.0 {
            w.units(name);
            w.sid(sid);
        }
    }
}

/// The status of a lookup that found `mapped` of `count`.
fn mapping(mapped: usize, count: usize) -> u32 {
    if mapped == count {
        SUCCESS
    } else if mapped == 0 {
        NONE_MAPPED
    } else {
        SOME_NOT_MAPPED
    }
}

/// The answer to lsarpc's operation `operation` with `stub`, or the
/// fault of one it does not have or that does not read.
pub fn operate(
    operation: u16,
    stub: &[u8],
    server: &Server,
    policies: &mut Policies,
) -> Result<Vec<u8>, u32> {
    let mut ndr = Ndr::new(stub);
    let mut w = Writer::default();
    match operation {
        CLOSE => {
            let handle = ndr.handle().ok_or(BAD_STUB_DATA)?;
            if policies.close(&handle) {
                w.out.zeros(20);
                w.u32(SUCCESS);
            } else {
                w.out.bytes(&handle);
                w.u32(INVALID_HANDLE);
            }
        }
        // What they are asked for is all a person's session may have:
        // names, which anyone here may read.
        OPEN_POLICY | OPEN_POLICY2 => match policies.open() {
            Some(handle) => {
                w.out.bytes(&handle);
                w.u32(SUCCESS);
            }
            None => {
                w.out.zeros(20);
                w.u32(INSUFFICIENT_RESOURCES);
            }
        },
        LOOKUP_SIDS | LOOKUP_SIDS2 => {
            lookup_sids(&mut ndr, &mut w, server, policies, operation == LOOKUP_SIDS2)
                .ok_or(BAD_STUB_DATA)?;
        }
        LOOKUP_NAMES | LOOKUP_NAMES2 | LOOKUP_NAMES3 => {
            lookup_names(&mut ndr, &mut w, server, policies, operation).ok_or(BAD_STUB_DATA)?;
        }
        _ => return Err(NCA_OP_RNG_ERROR),
    }
    Ok(w.out.0)
}

/// LsarLookupSids, or LsarLookupSids2 if `ex`: the names of SIDs.
fn lookup_sids(
    ndr: &mut Ndr,
    w: &mut Writer,
    server: &Server,
    policies: &Policies,
    ex: bool,
) -> Option<()> {
    let handle = ndr.handle()?;
    // LSAPR_SID_ENUM_BUFFER: the count, the array's pointer, the array.
    let count = ndr.u32()? as usize;
    let mut sids = Vec::new();
    if ndr.u32()? != 0 {
        if ndr.u32()? as usize != count || count > SIDS_MAX {
            return None;
        }
        let pointers = (0..count).map(|_| ndr.u32()).collect::<Option<Vec<u32>>>()?;
        for pointer in pointers {
            sids.push(if pointer == 0 { None } else { Some(ndr.sid()?) });
        }
    } else if count != 0 {
        return None;
    }
    if !policies.is_open(&handle) {
        // No domains, no names, none mapped.
        w.u32(0);
        w.u32(0);
        w.u32(0);
        w.u32(0);
        w.u32(INVALID_HANDLE);
        return Some(());
    }
    let mut domains = Domains::default();
    let found: Vec<Option<(Found, u32)>> = sids
        .iter()
        .map(|sid| {
            let found = by_sid(sid.as_ref()?, server.names.as_ref())?;
            let index = domains.index(&found.domain);
            Some((found, index))
        })
        .collect();
    let mapped = found.iter().flatten().count();
    domains.write(w);
    // LSAPR_TRANSLATED_NAMES(_EX): the count, the array's pointer, the
    // array, the names.
    w.u32(sids.len() as u32);
    if sids.is_empty() {
        w.u32(0);
    } else {
        w.pointer();
        w.u32(sids.len() as u32);
        for one in &found {
            match one {
                Some((found, index)) => {
                    w.u16(found.kind);
                    w.unicode(&found.name);
                    w.u32(*index);
                }
                None => {
                    w.u16(kind::UNKNOWN);
                    w.unicode("");
                    w.u32(u32::MAX);
                }
            }
            if ex {
                w.u32(0);
            }
        }
        for (found, _) in found.iter().flatten() {
            w.units(&found.name);
        }
    }
    w.u32(mapped as u32);
    w.u32(mapping(mapped, sids.len()));
    Some(())
}

/// LsarLookupNames, LsarLookupNames2 or LsarLookupNames3: the SIDs of
/// names.
fn lookup_names(
    ndr: &mut Ndr,
    w: &mut Writer,
    server: &Server,
    policies: &Policies,
    operation: u16,
) -> Option<()> {
    let handle = ndr.handle()?;
    let count = ndr.u32()? as usize;
    // The array of RPC_UNICODE_STRINGs: its conformance, their own parts,
    // then their buffers.
    if ndr.u32()? as usize != count || count > NAMES_MAX {
        return None;
    }
    let mut heads = Vec::with_capacity(count);
    for _ in 0..count {
        let _length = ndr.u32()?;
        heads.push(ndr.u32()?);
    }
    let mut names = Vec::with_capacity(count);
    for pointer in heads {
        names.push(if pointer == 0 { String::new() } else { ndr.units()? });
    }
    if !policies.is_open(&handle) {
        w.u32(0);
        w.u32(0);
        w.u32(0);
        w.u32(0);
        w.u32(INVALID_HANDLE);
        return Some(());
    }
    let mut domains = Domains::default();
    let found: Vec<Option<(Found, u32)>> = names
        .iter()
        .map(|name| {
            let found = by_name(name, &server.name, server.names.as_ref())?;
            let index = domains.index(&found.domain);
            Some((found, index))
        })
        .collect();
    let mapped = found.iter().flatten().count();
    domains.write(w);
    // LSAPR_TRANSLATED_SIDS, _EX or _EX2: the count, the array's pointer,
    // the array, and _EX2's SIDs.
    w.u32(count as u32);
    if count == 0 {
        w.u32(0);
    } else {
        w.pointer();
        w.u32(count as u32);
        for one in &found {
            let (kind, index) =
                one.as_ref().map_or((kind::UNKNOWN, u32::MAX), |(f, i)| (f.kind, *i));
            w.u16(kind);
            match (operation, one) {
                (LOOKUP_NAMES3, Some(_)) => w.pointer(),
                (LOOKUP_NAMES3, None) => w.u32(0),
                (_, found) => w.u32(found.as_ref().map_or(0, |(f, _)| f.rid)),
            }
            w.u32(index);
            if operation != LOOKUP_NAMES {
                w.u32(0);
            }
        }
        if operation == LOOKUP_NAMES3 {
            for (found, _) in found.iter().flatten() {
                w.sid(&found.sid());
            }
        }
    }
    w.u32(mapped as u32);
    w.u32(mapping(mapped, count));
    Some(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::rpc::{Interface, Listed, Pipe};

    /// alice (1000), bob (1001), the group staff (100) and alice's own
    /// group, also called alice (1000).
    pub struct Fake;

    impl Names for Fake {
        fn user(&self, uid: u32) -> Option<String> {
            let name = match uid {
                1000 => "alice",
                1001 => "bob",
                _ => return None,
            };
            Some(name.into())
        }
        fn group(&self, gid: u32) -> Option<String> {
            let name = match gid {
                100 => "staff",
                1000 => "alice",
                _ => return None,
            };
            Some(name.into())
        }
        fn uid(&self, name: &str) -> Option<u32> {
            Some(match name {
                "alice" => 1000,
                "bob" => 1001,
                _ => return None,
            })
        }
        fn gid(&self, name: &str) -> Option<u32> {
            Some(match name {
                "staff" => 100,
                "alice" => 1000,
                _ => return None,
            })
        }
    }

    fn server() -> Server {
        Server { name: "BOX".into(), shares: Vec::<Listed>::new(), names: Box::new(Fake) }
    }

    #[test]
    fn sids_and_names_go_both_ways() {
        let names = Fake;
        let found = by_sid(&Who::User(1000).sid(), &names).unwrap();
        assert_eq!(
            (found.kind, found.domain.0, found.name.as_str()),
            (kind::USER, "Unix User", "alice")
        );
        let found = by_sid(&Who::Group(1000).sid(), &names).unwrap();
        assert_eq!(
            (found.kind, found.domain.0, found.name.as_str()),
            (kind::GROUP, "Unix Group", "alice")
        );
        let found = by_sid(&Who::CreatorOwner.sid(), &names).unwrap();
        assert_eq!(
            (found.domain.1.to_string(), found.name.as_str()),
            ("S-1-3".into(), "CREATOR OWNER")
        );
        assert!(by_sid(&Who::User(4242).sid(), &names).is_none());
        assert!(by_sid(&Sid::new(5, &[32, 544]), &names).is_none());

        let sid = |name: &str| by_name(name, "BOX", &names).map(|f| f.sid().to_string());
        assert_eq!(sid("alice").as_deref(), Some("S-1-22-1-1000"));
        assert_eq!(sid("Unix Group\\alice").as_deref(), Some("S-1-22-2-1000"));
        assert_eq!(sid("unix user\\bob").as_deref(), Some("S-1-22-1-1001"));
        assert_eq!(sid("box\\staff").as_deref(), Some("S-1-22-2-100"));
        assert_eq!(sid("everyone").as_deref(), Some("S-1-1-0"));
        assert_eq!(sid("\\CREATOR GROUP").as_deref(), Some("S-1-3-1"));
        assert_eq!(sid("ELSEWHERE\\alice"), None);
        assert_eq!(sid("nobody"), None);
    }

    /// An LsarOpenPolicy2 request's stub, and its handle from the answer.
    fn open(pipe_policies: &mut Policies) -> [u8; 20] {
        let mut stub = Writer::default();
        stub.u32(0);
        stub.u32(24);
        stub.out.zeros(20);
        stub.u32(0x0800);
        let answer = operate(OPEN_POLICY2, &stub.out.0, &server(), pipe_policies).unwrap();
        assert_eq!(&answer[20..24], &[0; 4]);
        answer[..20].try_into().unwrap()
    }

    #[test]
    fn handles_are_opened_and_closed() {
        let mut policies = Policies::default();
        let handle = open(&mut policies);
        assert!(policies.is_open(&handle));
        let closed = operate(CLOSE, &handle, &server(), &mut policies).unwrap();
        assert_eq!(closed, [0; 24]);
        let again = operate(CLOSE, &handle, &server(), &mut policies).unwrap();
        assert_eq!(&again[20..], &INVALID_HANDLE.to_le_bytes());
        for _ in 0..POLICIES_MAX {
            open(&mut policies);
        }
        let more = operate(OPEN_POLICY, &[0; 8], &server(), &mut policies).unwrap();
        assert_eq!(&more[20..], &INSUFFICIENT_RESOURCES.to_le_bytes());
        assert_eq!(operate(99, &[], &server(), &mut policies), Err(NCA_OP_RNG_ERROR));
    }

    /// A LookupSids request for `sids` with `handle`.
    fn lookup_sids_request(handle: &[u8; 20], sids: &[Sid]) -> Vec<u8> {
        let mut w = Writer::default();
        w.out.bytes(handle);
        w.u32(sids.len() as u32);
        w.pointer();
        w.u32(sids.len() as u32);
        for _ in sids {
            w.pointer();
        }
        for sid in sids {
            w.sid(sid);
        }
        // TranslatedNames, empty; the level; the count.
        w.u32(0);
        w.u32(0);
        w.u16(1);
        w.u32(0);
        w.out.0
    }

    #[test]
    fn a_lookup_names_what_it_can() {
        let mut policies = Policies::default();
        let handle = open(&mut policies);
        let sids = [Who::User(1000).sid(), Sid::new(5, &[18]), Who::Everyone.sid()];
        let answer =
            operate(LOOKUP_SIDS, &lookup_sids_request(&handle, &sids), &server(), &mut policies)
                .unwrap();
        let tail = &answer[answer.len() - 8..];
        assert_eq!(tail, [2, 0, 0, 0, 0x07, 0x01, 0, 0], "two mapped, some not");
        let text = String::from_utf16_lossy(
            &answer.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>(),
        );
        assert!(text.contains("Unix User") && text.contains("alice") && text.contains("Everyone"));
        // An unknown handle maps nothing.
        let answer =
            operate(LOOKUP_SIDS, &lookup_sids_request(&[9; 20], &sids), &server(), &mut policies)
                .unwrap();
        assert_eq!(&answer[answer.len() - 4..], &INVALID_HANDLE.to_le_bytes());
        // Every cut of a request is answered or faults, never a panic.
        let request = lookup_sids_request(&handle, &sids);
        for end in 0..request.len() {
            let _ = operate(LOOKUP_SIDS2, &request[..end], &server(), &mut policies);
            let _ = operate(LOOKUP_NAMES3, &request[..end], &server(), &mut policies);
        }
    }

    /// Reads every answer the pipe has, joined.
    fn drain(pipe: &mut Pipe) -> Vec<u8> {
        let mut all = Vec::new();
        while let Some((bytes, _)) = pipe.read(1 << 20) {
            all.extend(bytes);
        }
        all
    }

    #[test]
    fn the_pipe_binds_lsarpc_only() {
        let mut pipe = Pipe::new(Interface::Lsarpc);
        // srvsvc's BIND is refused on lsarpc's pipe.
        pipe.write(&smb2::rpc::build_srvsvc_bind(1), &server());
        let ack = drain(&mut pipe);
        assert_eq!(ack[2], 12);
        let results = ack.len() - 24;
        assert_eq!(&ack[results..results + 2], &[2, 0], "provider rejection");
    }
}
