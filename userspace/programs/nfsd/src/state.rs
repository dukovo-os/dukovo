// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What nfsd keeps of its clients (RFC 8881): each client's record, its
//! sessions and their slots, and the stateids of its opens and its
//! byte-range locks. It lives in memory: after a restart clients start
//! again, and their reclaims get NO_GRACE, so they open their files
//! afresh.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::nfs::{LEASE_SECONDS, err};
use crate::xdr::{self, Reader, Writer};

/// A file, by its share's key, its device and inode number.
pub type FileKey = (u64, u64, u64);

/// A stateid4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stateid {
    /// Its sequence number, which each change to its state moves.
    pub seqid: u32,
    /// What it names.
    pub other: [u8; 12],
}

impl Stateid {
    /// The anonymous stateid: a READ or WRITE without an open.
    pub const ANONYMOUS: Stateid = Stateid { seqid: 0, other: [0; 12] };
    /// The READ bypass stateid, all ones.
    pub const BYPASS: Stateid = Stateid { seqid: u32::MAX, other: [0xff; 12] };
    /// The current stateid: the one the operation before set.
    pub const CURRENT: Stateid = Stateid { seqid: 1, other: [0; 12] };
    /// The invalid stateid, which CLOSE answers with.
    pub const INVALID: Stateid = Stateid { seqid: u32::MAX, other: [0; 12] };

    /// Reads one.
    pub fn read(r: &mut Reader) -> xdr::Result<Stateid> {
        Ok(Stateid { seqid: r.u32()?, other: r.array()? })
    }

    /// Writes it.
    pub fn write(&self, w: &mut Writer) {
        w.u32(self.seqid).fixed(&self.other);
    }

    /// Whether it is the anonymous or the bypass stateid.
    pub fn is_special(&self) -> bool {
        *self == Stateid::ANONYMOUS || *self == Stateid::BYPASS
    }
}

/// A client's record, from EXCHANGE_ID.
#[derive(Debug)]
pub struct Client {
    /// What the client calls itself.
    pub owner: Vec<u8>,
    /// Its verifier, which changes when it restarts.
    pub verifier: [u8; 8],
    /// Whether a CREATE_SESSION confirmed it.
    pub confirmed: bool,
    /// The sequence number the next CREATE_SESSION carries.
    pub sequence: u32,
    /// The answer to the last CREATE_SESSION, for when it comes again.
    pub last_session: Option<Vec<u8>>,
    /// When it last called: its lease runs from then.
    pub renewed: Instant,
    /// Whether it said RECLAIM_COMPLETE.
    pub reclaimed: bool,
}

/// One slot of a session: the last request in it, and its answer if
/// the client asked that it be kept.
#[derive(Debug, Default)]
pub struct Slot {
    /// The last request's sequence number.
    pub sequence: u32,
    /// Its whole answer, if kept.
    pub reply: Option<Vec<u8>>,
}

/// A session's fore channel, as CREATE_SESSION settled it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    /// The padding before a request's header.
    pub pad: u32,
    /// The largest request.
    pub max_request: u32,
    /// The largest answer.
    pub max_response: u32,
    /// The largest answer kept for a replay.
    pub max_cached: u32,
    /// The most operations in a COMPOUND.
    pub max_ops: u32,
    /// The most requests at once: its slots.
    pub max_requests: u32,
}

impl Channel {
    /// Reads channel_attrs4.
    pub fn read(r: &mut Reader) -> xdr::Result<Channel> {
        let c = Channel {
            pad: r.u32()?,
            max_request: r.u32()?,
            max_response: r.u32()?,
            max_cached: r.u32()?,
            max_ops: r.u32()?,
            max_requests: r.u32()?,
        };
        // ca_rdma_ird<1>, which TCP has no use for.
        let n = r.count(1)?;
        for _ in 0..n {
            r.u32()?;
        }
        Ok(c)
    }

    /// Writes it, without RDMA's.
    pub fn write(&self, w: &mut Writer) {
        w.u32(self.pad).u32(self.max_request).u32(self.max_response).u32(self.max_cached);
        w.u32(self.max_ops).u32(self.max_requests).u32(0);
    }
}

/// A session.
#[derive(Debug)]
pub struct Session {
    /// Its client.
    pub client: u64,
    /// Its slots.
    pub slots: Vec<Slot>,
    /// Its fore channel.
    pub fore: Channel,
}

/// An open: a client's open-owner's on a file.
#[derive(Clone, Debug)]
pub struct Open {
    /// The client.
    pub client: u64,
    /// The open-owner.
    pub owner: Vec<u8>,
    /// The file.
    pub file: FileKey,
    /// The person whose worker holds it open.
    pub uid: u32,
    /// The worker's handle, while the worker that opened it serves: none
    /// to read and write by the path.
    pub handle: Option<u64>,
    /// What it may do: read (1), write (2), both.
    pub access: u32,
    /// What it keeps others from doing.
    pub deny: u32,
    /// Its stateid's sequence number.
    pub seqid: u32,
}

/// A lock-owner's locks on a file, through an open.
#[derive(Clone, Debug)]
pub struct Locker {
    /// The client.
    pub client: u64,
    /// The lock-owner.
    pub owner: Vec<u8>,
    /// The open's stateid's `other`.
    pub open: [u8; 12],
    /// The file.
    pub file: FileKey,
    /// Its stateid's sequence number.
    pub seqid: u32,
}

/// What a stateid names.
#[derive(Clone, Debug)]
pub enum Kind {
    /// An open.
    Open(Open),
    /// A lock-owner's locks.
    Lock(Locker),
}

/// A byte-range lock: from `start` to before `end`, `u64::MAX` being
/// the end of any file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    /// Whose: the client and the lock-owner.
    pub owner: (u64, Vec<u8>),
    /// Where it starts.
    pub start: u64,
    /// Where it ends.
    pub end: u64,
    /// Whether it is a write lock, which no one else's overlaps.
    pub write: bool,
}

/// A worker's handle to close: the person, and the handle.
pub type Handle = (u32, u64);

/// Everything nfsd keeps of its clients.
pub struct State {
    /// This run of nfsd's number, in its client IDs and stateids: those
    /// of another run are stale.
    pub boot: u32,
    next: u64,
    /// The clients, by ID.
    pub clients: HashMap<u64, Client>,
    /// The sessions, by ID.
    pub sessions: HashMap<[u8; 16], Session>,
    /// The opens and lockers, by their stateids' `other`.
    pub states: HashMap<[u8; 12], Kind>,
    /// The locks on each file.
    pub locks: HashMap<FileKey, Vec<Range>>,
}

/// How long a client that does not call keeps its state: twice its
/// lease, as a courtesy.
pub fn lease_end() -> Duration {
    Duration::from_secs(2 * u64::from(LEASE_SECONDS))
}

impl State {
    /// No clients, for the run numbered `boot`.
    pub fn new(boot: u32) -> State {
        State {
            boot,
            next: 1,
            clients: HashMap::new(),
            sessions: HashMap::new(),
            states: HashMap::new(),
            locks: HashMap::new(),
        }
    }

    fn count(&mut self) -> u64 {
        let n = self.next;
        self.next += 1;
        n
    }

    /// A new client ID: the run's number, then a count.
    pub fn new_client_id(&mut self) -> u64 {
        (u64::from(self.boot) << 32) | (self.count() & 0xffff_ffff)
    }

    /// A new stateid's `other`.
    pub fn new_other(&mut self) -> [u8; 12] {
        let mut other = [0; 12];
        other[..4].copy_from_slice(&self.boot.to_be_bytes());
        other[4..].copy_from_slice(&self.count().to_be_bytes());
        other
    }

    /// A new session ID, of `client`'s.
    pub fn new_session_id(&mut self, client: u64) -> [u8; 16] {
        let mut id = [0; 16];
        id[..8].copy_from_slice(&client.to_be_bytes());
        id[8..].copy_from_slice(&self.count().to_be_bytes());
        id
    }

    /// What `stateid` names, checked: the status if it names nothing now
    /// or an older state.
    pub fn find(&self, stateid: &Stateid) -> Result<&Kind, u32> {
        let Some(kind) = self.states.get(&stateid.other) else {
            return Err(if stateid.other[..4] == self.boot.to_be_bytes() {
                err::BAD_STATEID
            } else {
                err::STALE_STATEID
            });
        };
        let seqid = match kind {
            Kind::Open(open) => open.seqid,
            Kind::Lock(locker) => locker.seqid,
        };
        // Zero asks for the state as it is now.
        match stateid.seqid {
            0 => Ok(kind),
            s if s == seqid => Ok(kind),
            s if s < seqid => Err(err::OLD_STATEID),
            _ => Err(err::BAD_STATEID),
        }
    }

    /// The open `stateid` names, or a locker's.
    pub fn open_of(&self, stateid: &Stateid) -> Result<&Open, u32> {
        match self.find(stateid)? {
            Kind::Open(open) => Ok(open),
            Kind::Lock(locker) => match self.states.get(&locker.open) {
                Some(Kind::Open(open)) => Ok(open),
                _ => Err(err::BAD_STATEID),
            },
        }
    }

    /// The open of `client`'s `owner` on `file`, by its `other`.
    pub fn open_by(&self, client: u64, owner: &[u8], file: FileKey) -> Option<[u8; 12]> {
        self.states.iter().find_map(|(other, kind)| match kind {
            Kind::Open(o) if o.client == client && o.owner == owner && o.file == file => {
                Some(*other)
            }
            _ => None,
        })
    }

    /// The locker of `client`'s lock-owner `owner` on `file`.
    pub fn locker_by(&self, client: u64, owner: &[u8], file: FileKey) -> Option<[u8; 12]> {
        self.states.iter().find_map(|(other, kind)| match kind {
            Kind::Lock(l) if l.client == client && l.owner == owner && l.file == file => {
                Some(*other)
            }
            _ => None,
        })
    }

    /// Whether an open of `access` and `deny` on `file` by someone but
    /// `other` is kept out by another's share reservation, or keeps
    /// another out.
    pub fn share_conflict(
        &self,
        file: FileKey,
        access: u32,
        deny: u32,
        other: Option<[u8; 12]>,
    ) -> bool {
        self.states.iter().any(|(id, kind)| match kind {
            Kind::Open(o) if o.file == file && Some(*id) != other => {
                access & o.deny != 0 || deny & o.access != 0
            }
            _ => false,
        })
    }

    /// The lock on `file` that keeps `owner` from locking from `start`
    /// to `end`, for writing if `write`.
    pub fn conflict(
        &self,
        file: FileKey,
        owner: &(u64, Vec<u8>),
        start: u64,
        end: u64,
        write: bool,
    ) -> Option<&Range> {
        self.locks
            .get(&file)?
            .iter()
            .find(|r| &r.owner != owner && r.start < end && start < r.end && (write || r.write))
    }

    /// Takes the lock away from `owner` on `file` from `start` to `end`,
    /// splitting what it held across it.
    pub fn unlock(&mut self, file: FileKey, owner: &(u64, Vec<u8>), start: u64, end: u64) {
        let Some(ranges) = self.locks.get_mut(&file) else { return };
        let mut kept = Vec::with_capacity(ranges.len() + 1);
        for r in ranges.drain(..) {
            if &r.owner != owner || r.end <= start || end <= r.start {
                kept.push(r);
                continue;
            }
            if r.start < start {
                kept.push(Range { end: start, ..r.clone() });
            }
            if end < r.end {
                kept.push(Range { start: end, ..r });
            }
        }
        if kept.is_empty() {
            self.locks.remove(&file);
        } else {
            *ranges = kept;
        }
    }

    /// Gives `owner` a lock on `file` from `start` to `end`, as POSIX
    /// does: what it held there before becomes this.
    pub fn lock(
        &mut self,
        file: FileKey,
        owner: (u64, Vec<u8>),
        start: u64,
        end: u64,
        write: bool,
    ) {
        self.unlock(file, &owner, start, end);
        self.locks.entry(file).or_default().push(Range { owner, start, end, write });
    }

    /// Whether `owner` holds any lock on `file`.
    pub fn holds(&self, file: FileKey, owner: &(u64, Vec<u8>)) -> bool {
        self.locks.get(&file).is_some_and(|ranges| ranges.iter().any(|r| &r.owner == owner))
    }

    /// Removes a state: an open with its lockers and their locks. Returns
    /// the worker's handle to close, if any.
    pub fn remove(&mut self, other: &[u8; 12]) -> Option<Handle> {
        match self.states.remove(other)? {
            Kind::Open(open) => {
                let lockers: Vec<[u8; 12]> = self
                    .states
                    .iter()
                    .filter_map(|(id, k)| {
                        matches!(k, Kind::Lock(l) if l.open == *other).then_some(*id)
                    })
                    .collect();
                for id in lockers {
                    self.remove(&id);
                }
                open.handle.map(|h| (open.uid, h))
            }
            Kind::Lock(locker) => {
                let owner = (locker.client, locker.owner);
                self.unlock(locker.file, &owner, 0, u64::MAX);
                None
            }
        }
    }

    /// Removes a client with its sessions and its states: the workers'
    /// handles to close.
    pub fn purge(&mut self, client: u64) -> Vec<Handle> {
        self.clients.remove(&client);
        self.sessions.retain(|_, s| s.client != client);
        let opens: Vec<[u8; 12]> = self
            .states
            .iter()
            .filter_map(|(id, k)| matches!(k, Kind::Open(o) if o.client == client).then_some(*id))
            .collect();
        let handles = opens.iter().filter_map(|id| self.remove(id)).collect();
        // Lockers whose open went first.
        let lockers: Vec<[u8; 12]> = self
            .states
            .iter()
            .filter_map(|(id, k)| matches!(k, Kind::Lock(l) if l.client == client).then_some(*id))
            .collect();
        for id in lockers {
            self.remove(&id);
        }
        handles
    }

    /// Removes the clients whose leases ran out by `now`: the workers'
    /// handles to close.
    pub fn expire(&mut self, now: Instant) -> Vec<Handle> {
        let ended: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| now.saturating_duration_since(c.renewed) > lease_end())
            .map(|(id, _)| *id)
            .collect();
        ended.into_iter().flat_map(|id| self.purge(id)).collect()
    }

    /// Forgets every worker's handle, when the workers start again: the
    /// opens read and write by their paths.
    pub fn forget_handles(&mut self) {
        for kind in self.states.values_mut() {
            if let Kind::Open(open) = kind {
                open.handle = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: FileKey = (1, 2, 3);

    fn owner(n: u8) -> (u64, Vec<u8>) {
        (7, vec![n])
    }

    #[test]
    fn locks_split_merge_and_conflict() {
        let mut s = State::new(1);
        s.lock(FILE, owner(1), 0, 100, true);
        assert!(s.conflict(FILE, &owner(2), 50, 60, false).is_some());
        assert!(s.conflict(FILE, &owner(2), 100, 200, true).is_none());
        assert!(s.conflict(FILE, &owner(1), 0, 10, true).is_none());
        // Unlocking the middle leaves both ends.
        s.unlock(FILE, &owner(1), 40, 60);
        assert!(s.conflict(FILE, &owner(2), 45, 55, true).is_none());
        assert!(s.conflict(FILE, &owner(2), 30, 41, true).is_some());
        assert!(s.conflict(FILE, &owner(2), 59, 61, true).is_some());
        // A read lock over it all becomes what owner 1 holds.
        s.lock(FILE, owner(1), 0, u64::MAX, false);
        assert!(s.conflict(FILE, &owner(2), 5, 6, false).is_none());
        assert!(s.conflict(FILE, &owner(2), 5, 6, true).is_some());
        assert_eq!(s.locks[&FILE].len(), 1);
        s.unlock(FILE, &owner(1), 0, u64::MAX);
        assert!(!s.holds(FILE, &owner(1)));
        assert!(!s.locks.contains_key(&FILE));
    }

    #[test]
    fn stateids_are_checked() {
        let mut s = State::new(5);
        let other = s.new_other();
        let open = Open {
            client: 1,
            owner: vec![1],
            file: FILE,
            uid: 1000,
            handle: Some(4),
            access: 1,
            deny: 0,
            seqid: 2,
        };
        s.states.insert(other, Kind::Open(open));
        assert!(s.find(&Stateid { seqid: 2, other }).is_ok());
        assert!(s.find(&Stateid { seqid: 0, other }).is_ok());
        assert_eq!(s.find(&Stateid { seqid: 1, other }).unwrap_err(), err::OLD_STATEID);
        assert_eq!(s.find(&Stateid { seqid: 3, other }).unwrap_err(), err::BAD_STATEID);
        let mut gone = other;
        gone[11] ^= 1;
        assert_eq!(s.find(&Stateid { seqid: 1, other: gone }).unwrap_err(), err::BAD_STATEID);
        gone[0] ^= 1;
        assert_eq!(s.find(&Stateid { seqid: 1, other: gone }).unwrap_err(), err::STALE_STATEID);
        // Another's deny-write keeps a writer out, not a reader.
        assert!(!s.share_conflict(FILE, 2, 0, None));
        if let Some(Kind::Open(o)) = s.states.get_mut(&other) {
            o.deny = 2;
        }
        assert!(s.share_conflict(FILE, 2, 0, None));
        assert!(!s.share_conflict(FILE, 1, 0, None));
        assert!(!s.share_conflict(FILE, 2, 0, Some(other)));
        assert_eq!(s.purge(1), vec![(1000, 4)]);
        assert!(s.states.is_empty());
    }
}
