// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What admind counts so that it stays usable for those signed in while
//! anyone on the network may knock (ADR 0514): connections at once, in all
//! and from one address; sign-in tries from one address in a minute; the
//! sign-ins it gave a second step to, and to which address; why sessions
//! ended, for the tabs that ask; and how often a refused address is
//! mentioned in the log. Each is plain data with the time given in, so
//! that each rule is a host unit test. Addresses are the ones
//! [`reach::counted`](super::reach::counted) gives: an IPv4 address, or an
//! IPv6 address's /64.

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The most connections at once, and the most from one address.
pub const CONNECTIONS_MAX: usize = 512;
pub const CONNECTIONS_PER_ADDRESS_MAX: usize = 32;

/// The most sign-in requests one address may make in [`TRIES_WINDOW`].
pub const TRIES_MAX: u32 = 10;

/// How long a sign-in try counts.
pub const TRIES_WINDOW: Duration = Duration::from_secs(60);

/// The most addresses whose tries are kept. Past it, an address not kept
/// already waits for the next tidying, which is no worse than its own
/// wait.
const TRIES_ADDRESSES_MAX: usize = 10_000;

/// How long a sign-in's ticket is kept, and how many are: as authd keeps
/// them.
pub const TICKET_TIME: Duration = Duration::from_mins(5);
pub const TICKETS_MAX: usize = 128;

/// How many endings are remembered, for tabs that ask why their session
/// ended.
const ENDINGS_MAX: usize = 128;

/// How often one address's refusals are said in the log.
const QUIET_TIME: Duration = Duration::from_mins(1);

/// The most addresses whose refusals are counted for the log.
const QUIET_ADDRESSES_MAX: usize = 4096;

/// The connections held now.
#[derive(Default)]
pub struct Connections {
    total: usize,
    by_address: HashMap<IpAddr, usize>,
}

/// A connection's place among the connections held: it is given back when
/// the connection ends, however it ends.
pub struct Slot {
    connections: Rc<RefCell<Connections>>,
    address: IpAddr,
}

impl Slot {
    /// A place for a connection from `address`, or `None` when there are
    /// [`CONNECTIONS_MAX`] connections or [`CONNECTIONS_PER_ADDRESS_MAX`]
    /// from that address.
    pub fn take(connections: &Rc<RefCell<Connections>>, address: IpAddr) -> Option<Slot> {
        let mut held = connections.borrow_mut();
        let from_here = held.by_address.get(&address).copied().unwrap_or(0);
        if held.total >= CONNECTIONS_MAX || from_here >= CONNECTIONS_PER_ADDRESS_MAX {
            return None;
        }
        held.total += 1;
        held.by_address.insert(address, from_here + 1);
        Some(Slot { connections: Rc::clone(connections), address })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut held = self.connections.borrow_mut();
        held.total -= 1;
        if let Some(from_here) = held.by_address.get_mut(&self.address) {
            *from_here -= 1;
            if *from_here == 0 {
                held.by_address.remove(&self.address);
            }
        }
    }
}

/// The sign-in requests each address made in the last minute.
#[derive(Default)]
pub struct Tries {
    windows: HashMap<IpAddr, (Instant, u32)>,
}

impl Tries {
    /// Counts a sign-in request from `address` at `now`.
    ///
    /// # Errors
    ///
    /// With the seconds after which it may try again, when it has made
    /// [`TRIES_MAX`] in the minute since its first.
    pub fn take(&mut self, address: IpAddr, now: Instant) -> Result<(), u64> {
        if !self.windows.contains_key(&address) && self.windows.len() >= TRIES_ADDRESSES_MAX {
            self.tidy(now);
            if self.windows.len() >= TRIES_ADDRESSES_MAX {
                return Err(TRIES_WINDOW.as_secs());
            }
        }
        let (start, count) = self.windows.entry(address).or_insert((now, 0));
        if now.saturating_duration_since(*start) >= TRIES_WINDOW {
            (*start, *count) = (now, 0);
        }
        if *count >= TRIES_MAX {
            let left = TRIES_WINDOW.saturating_sub(now.saturating_duration_since(*start));
            return Err(left.as_secs() + 1);
        }
        *count += 1;
        Ok(())
    }

    /// Forgets the addresses whose minute is over.
    pub fn tidy(&mut self, now: Instant) {
        self.windows.retain(|_, (start, _)| now.saturating_duration_since(*start) < TRIES_WINDOW);
    }
}

/// A sign-in that was given a second step to take.
struct Given {
    name: String,
    address: IpAddr,
    at: Instant,
}

/// The sign-ins that were given a second step to take, each with the
/// account and the address it was given to: a ticket is of no use to
/// another address.
#[derive(Default)]
pub struct Tickets {
    given: HashMap<String, Given>,
}

impl Tickets {
    /// Keeps `ticket`, given to `address` for `name`'s sign-in at `now`;
    /// the oldest is let go when [`TICKETS_MAX`] are kept.
    pub fn give(&mut self, ticket: &str, name: &str, address: IpAddr, now: Instant) {
        self.tidy(now);
        while self.given.len() >= TICKETS_MAX {
            let Some(oldest) =
                self.given.iter().min_by_key(|(_, given)| given.at).map(|(ticket, _)| ticket.clone())
            else {
                break;
            };
            self.given.remove(&oldest);
        }
        self.given.insert(ticket.into(), Given { name: name.into(), address, at: now });
    }

    /// The account `ticket` was given for, if it was given to `address`
    /// not too long ago.
    pub fn name_for(&self, ticket: &str, address: IpAddr, now: Instant) -> Option<&str> {
        let given = self.given.get(ticket)?;
        let fresh = now.saturating_duration_since(given.at) < TICKET_TIME;
        (given.address == address && fresh).then_some(given.name.as_str())
    }

    /// Lets `ticket` go: its sign-in is done.
    pub fn forget(&mut self, ticket: &str) {
        self.given.remove(ticket);
    }

    /// Forgets the tickets that are too old.
    pub fn tidy(&mut self, now: Instant) {
        self.given.retain(|_, given| now.saturating_duration_since(given.at) < TICKET_TIME);
    }
}

/// Why sessions ended lately, by authd's name for them, so that a tab
/// whose session went can say why.
#[derive(Default)]
pub struct Endings {
    reasons: VecDeque<(String, &'static str)>,
}

impl Endings {
    /// Notes that session `authd` ended for `reason`.
    pub fn note(&mut self, authd: &str, reason: &'static str) {
        if self.reasons.len() >= ENDINGS_MAX {
            self.reasons.pop_front();
        }
        self.reasons.push_back((authd.into(), reason));
    }

    /// Why session `authd` ended; `expired` when it is not remembered.
    pub fn reason(&self, authd: &str) -> &'static str {
        self.reasons.iter().rev().find(|(id, _)| id == authd).map_or("expired", |(_, why)| why)
    }
}

/// The log's rule for a refusal from an address: it is said when it first
/// happens, and then once a minute, with how many there were, however
/// many tries come.
#[derive(Default)]
pub struct Quiet {
    said: HashMap<IpAddr, (Instant, u32)>,
}

impl Quiet {
    /// Notes a refusal of `address` at `now`: how many refusals the line
    /// to write now stands for, or `None` when one was said less than a
    /// minute ago.
    pub fn note(&mut self, address: IpAddr, now: Instant) -> Option<u32> {
        if !self.said.contains_key(&address) && self.said.len() >= QUIET_ADDRESSES_MAX {
            self.tidy(now);
            if self.said.len() >= QUIET_ADDRESSES_MAX {
                return None;
            }
        }
        match self.said.entry(address) {
            Entry::Vacant(first) => {
                first.insert((now, 0));
                Some(1)
            }
            Entry::Occupied(mut said) => {
                let (said_at, unsaid) = said.get_mut();
                *unsaid += 1;
                if now.saturating_duration_since(*said_at) < QUIET_TIME {
                    return None;
                }
                *said_at = now;
                Some(std::mem::take(unsaid))
            }
        }
    }

    /// Forgets the addresses that have been quiet for a minute.
    pub fn tidy(&mut self, now: Instant) {
        self.said.retain(|_, (said_at, _)| now.saturating_duration_since(*said_at) < QUIET_TIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_connection_holds_a_place_until_it_ends() {
        let held = Rc::new(RefCell::new(Connections::default()));
        let one = ip("192.0.2.1");
        let places: Vec<Slot> =
            (0..CONNECTIONS_PER_ADDRESS_MAX).map(|_| Slot::take(&held, one).unwrap()).collect();
        assert_eq!(held.borrow().total, CONNECTIONS_PER_ADDRESS_MAX);
        assert!(Slot::take(&held, one).is_none());
        // Another address is not held back by it.
        let other = Slot::take(&held, ip("192.0.2.2")).unwrap();
        drop(places);
        assert_eq!(held.borrow().total, 1);
        assert!(Slot::take(&held, one).is_some());
        drop(other);
    }

    #[test]
    fn there_are_512_connections_at_most_in_all() {
        let held = Rc::new(RefCell::new(Connections::default()));
        let mut places = Vec::new();
        for n in 0..CONNECTIONS_MAX {
            let address = ip(&format!("10.{}.{}.1", n / 256, n % 256));
            places.push(Slot::take(&held, address).unwrap());
        }
        assert!(Slot::take(&held, ip("10.9.9.9")).is_none());
        places.pop();
        assert!(Slot::take(&held, ip("10.9.9.9")).is_some());
        places.clear();
        assert_eq!(held.borrow().total, 1);
    }

    #[test]
    fn an_address_may_try_to_sign_in_ten_times_a_minute() {
        let mut tries = Tries::default();
        let start = Instant::now();
        let one = ip("192.0.2.1");
        for _ in 0..TRIES_MAX {
            assert_eq!(tries.take(one, start), Ok(()));
        }
        assert_eq!(tries.take(one, start + Duration::from_secs(20)), Err(41));
        assert_eq!(tries.take(one, start + Duration::from_secs(59)), Err(2));
        assert_eq!(tries.take(ip("192.0.2.2"), start), Ok(()));
        // The minute is over; the count starts again.
        let later = start + TRIES_WINDOW;
        for _ in 0..TRIES_MAX {
            assert_eq!(tries.take(one, later), Ok(()));
        }
        assert!(tries.take(one, later).is_err());
    }

    #[test]
    fn the_tries_of_so_many_addresses_do_not_fill_the_memory() {
        let mut tries = Tries::default();
        let start = Instant::now();
        for n in 0..TRIES_ADDRESSES_MAX {
            let (high, low) = (u8::try_from(n / 256).unwrap(), u8::try_from(n % 256).unwrap());
            tries.take(IpAddr::from([10, 0, high, low]), start).unwrap();
        }
        assert_eq!(tries.windows.len(), TRIES_ADDRESSES_MAX);
        assert_eq!(tries.take(ip("203.0.113.5"), start), Err(60));
        assert!(tries.take(ip("203.0.113.5"), start + TRIES_WINDOW).is_ok());
        assert_eq!(tries.windows.len(), 1);
    }

    #[test]
    fn a_ticket_is_good_for_the_address_it_was_given_to_and_for_five_minutes() {
        let mut tickets = Tickets::default();
        let start = Instant::now();
        let (mine, other) = (ip("192.0.2.1"), ip("192.0.2.2"));
        tickets.give("aa", "chase", mine, start);
        assert_eq!(tickets.name_for("aa", mine, start), Some("chase"));
        assert_eq!(tickets.name_for("aa", other, start), None);
        assert_eq!(tickets.name_for("bb", mine, start), None);
        assert!(tickets.name_for("aa", mine, start + Duration::from_secs(299)).is_some());
        assert_eq!(tickets.name_for("aa", mine, start + TICKET_TIME), None);
        tickets.forget("aa");
        assert_eq!(tickets.name_for("aa", mine, start), None);
    }

    #[test]
    fn at_most_128_tickets_are_kept_and_the_oldest_goes_first() {
        let mut tickets = Tickets::default();
        let start = Instant::now();
        let address = ip("192.0.2.1");
        for n in 0..=TICKETS_MAX {
            tickets.give(&format!("t{n}"), "chase", address, start + Duration::from_millis(n as u64));
        }
        let now = start + Duration::from_secs(1);
        assert_eq!(tickets.name_for("t0", address, now), None);
        assert!(tickets.name_for("t1", address, now).is_some());
        assert!(tickets.name_for(&format!("t{TICKETS_MAX}"), address, now).is_some());
        assert_eq!(tickets.given.len(), TICKETS_MAX);
    }

    #[test]
    fn an_ending_is_remembered_by_the_session_and_the_latest_wins() {
        let mut endings = Endings::default();
        assert_eq!(endings.reason("3f9a"), "expired");
        endings.note("3f9a", "idle");
        endings.note("5b2c", "signed-out");
        assert_eq!(endings.reason("3f9a"), "idle");
        assert_eq!(endings.reason("5b2c"), "signed-out");
        for n in 0..ENDINGS_MAX {
            endings.note(&format!("s{n}"), "worker");
        }
        assert_eq!(endings.reason("3f9a"), "expired");
        assert_eq!(endings.reason("s5"), "worker");
    }

    #[test]
    fn a_refused_address_is_said_once_a_minute_with_how_many_there_were() {
        let mut quiet = Quiet::default();
        let start = Instant::now();
        let address = ip("198.51.100.4");
        assert_eq!(quiet.note(address, start), Some(1));
        for second in 1..60 {
            assert_eq!(quiet.note(address, start + Duration::from_secs(second)), None);
        }
        // 59 more came; this one is the 60th and the minute is up.
        assert_eq!(quiet.note(address, start + Duration::from_secs(60)), Some(60));
        assert_eq!(quiet.note(address, start + Duration::from_secs(61)), None);
        // Another address has its own line.
        assert_eq!(quiet.note(ip("198.51.100.5"), start + Duration::from_secs(61)), Some(1));
        quiet.tidy(start + Duration::from_secs(200));
        assert!(quiet.said.is_empty());
    }
}
