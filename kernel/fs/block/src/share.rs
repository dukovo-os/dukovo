// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How a disk is shared between resource domains (ADR 0476): the rates a
//! domain's requests keep to ([`Rate`]), and the order in which a disk
//! serves the domains whose requests wait for it, in proportion to their
//! weights ([`Shares`]).
//!
//! These are the arithmetic alone: the kernel's disk queue keeps them
//! under its locks and reads the clock for them, so the host tests run
//! them as they are.

use crate::mem::{self, KVec};

/// Nanoseconds in a second.
const SECOND: u64 = 1_000_000_000;

/// A rate of units each second, held as a token bucket that holds one
/// second's worth: a taker that finds too few tokens takes them all the
/// same, and waits until the bucket would have had them.
///
/// It is kept as the time the bucket's tokens are spent until (the
/// generic cell rate algorithm's theoretical arrival time): a full bucket
/// is one spent until a second ago or earlier, and a take of `units`
/// moves the time on by what they take to come back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    /// Units each second, at least 1.
    per_second: u64,
    /// Nanoseconds since boot until which the tokens are spent.
    spent_until: u64,
}

impl Rate {
    /// A rate of `per_second` units each second (at least 1), with its
    /// bucket full.
    #[must_use]
    pub const fn new(per_second: u64) -> Rate {
        Rate { per_second: if per_second == 0 { 1 } else { per_second }, spent_until: 0 }
    }

    /// Units each second.
    #[must_use]
    pub const fn per_second(&self) -> u64 {
        self.per_second
    }

    /// Changes the rate, keeping what was spent.
    pub fn set(&mut self, per_second: u64) {
        self.per_second = per_second.max(1);
    }

    /// Takes `units` at `now_ns`: returns how long the taker waits for
    /// them, in nanoseconds, 0 when the bucket held them. A take larger
    /// than the bucket waits for the rest to come back.
    pub fn take(&mut self, units: u64, now_ns: u64) -> u64 {
        let from = self.spent_until.max(now_ns.saturating_sub(SECOND));
        let cost = u128::from(units) * u128::from(SECOND) / u128::from(self.per_second);
        self.spent_until = from.saturating_add(u64::try_from(cost).unwrap_or(u64::MAX));
        self.spent_until.saturating_sub(now_ns)
    }
}

/// The most domains a disk keeps shares for at once, each at one rank of
/// urgency; requests beyond them go as if no weight were set.
pub const SHARES: usize = 32;

/// The weight a domain has by default.
pub const WEIGHT_DEFAULT: u64 = 100;

/// The virtual time a cost of 1 takes at a weight of 1, and at any other
/// in inverse proportion to it: 65,536 at the default weight, which is
/// fine enough that integer division keeps the shares within a hundredth
/// of the weights.
const TIME_PER_COST: u64 = WEIGHT_DEFAULT << 16;

/// Virtual times past this are all moved back by the same amount, so that
/// none reaches the end of a `u64`.
const REBASE: u64 = 1 << 62;

/// One domain's share of a disk, at one rank.
#[derive(Clone, Copy, Debug, Default)]
struct Share {
    /// Whether it is a domain's.
    used: bool,
    /// The domain's number.
    domain: u32,
    /// The rank of urgency of its requests: lower goes first.
    rank: u8,
    /// Its weight, flattened along the domain tree.
    weight: u64,
    /// Its virtual time: where its next request starts, as what it has
    /// had of the disk over its weight.
    vtime: u64,
    /// Its requests waiting to go.
    waiting: u32,
    /// Its requests gone and not yet done.
    in_flight: u32,
}

impl Share {
    /// Whether none of its requests waits or is on the disk.
    const fn idle(&self) -> bool {
        self.waiting == 0 && self.in_flight == 0
    }
}

/// A disk's shares between the domains whose requests wait for it or are
/// on it: start-time fair queuing over virtual time. While more requests
/// wait than the disk has room for (`depth`), the next to go is that of
/// the most urgent rank and, among those, of the domain whose virtual
/// time is least; each request moves its domain's virtual time on by its
/// cost over its weight. So domains that keep the disk busy share it in
/// proportion to their weights.
///
/// The disk's own virtual time is that of the last request to go. A
/// domain none of whose requests waited starts its next from there, or
/// from its own if that is later: a pause earns it no credit, and a
/// domain that has had more than its share is not let off. Shares stay
/// while the disk is busy, so that what a domain has had is remembered
/// between its requests; once the disk is idle, they are all forgotten,
/// as no domain then waits for any other.
#[derive(Debug)]
pub struct Shares {
    /// [`SHARES`] of them, on the heap: a disk's queue holds its shares,
    /// and kept in place they made every driver's frame that builds a
    /// queue a kilobyte larger (ADR 0058).
    shares: KVec<Share>,
    /// Requests gone and not yet done.
    in_flight: u32,
    /// The most requests on the disk at once while domains share it.
    depth: u32,
    /// The disk's virtual time.
    clock: u64,
}

/// A request's place in a disk's [`Shares`], from [`Shares::arrive`]: it
/// stays its domain's while the request waits or is on the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(usize);

impl Shares {
    /// Shares of a disk that holds `depth` requests at once (at least 1);
    /// `None` when memory runs out.
    #[must_use]
    pub fn new(depth: u32) -> Option<Shares> {
        let mut shares = KVec::new_in(mem::alloc());
        shares.try_reserve_exact(SHARES).ok()?;
        for _ in 0..SHARES {
            shares.push_within_capacity(Share::default()).ok()?;
        }
        Some(Shares { shares, in_flight: 0, depth: depth.max(1), clock: 0 })
    }

    /// The cost of a request of `pages` pages: its pages, and a charge for
    /// each request, as a disk that seeks pays for it with its head's time
    /// (16 pages' worth) and one that does not with its command's (2).
    #[must_use]
    pub const fn cost(pages: u64, seeks: bool) -> u64 {
        pages + if seeks { 16 } else { 2 }
    }

    /// A request of `domain` at `rank`, with its domain's
    /// flattened `weight`, comes to wait: its ticket, or `None` when
    /// [`SHARES`] domains' shares are in use, and it goes as if no weight
    /// were set.
    pub fn arrive(&mut self, domain: u32, rank: u8, weight: u64) -> Option<Ticket> {
        let index =
            match self.shares.iter().position(|s| s.used && s.domain == domain && s.rank == rank) {
                Some(index) => index,
                None => {
                    // A free share, or one idle whose domain has had no more
                    // than its share, which forgetting changes nothing for.
                    let clock = self.clock;
                    let index = self.shares.iter().position(|s| !s.used).or_else(|| {
                        self.shares.iter().position(|s| s.idle() && s.vtime <= clock)
                    })?;
                    self.shares[index] =
                        Share { used: true, domain, rank, vtime: clock, ..Share::default() };
                    index
                }
            };
        let share = &mut self.shares[index];
        if share.waiting == 0 {
            share.vtime = share.vtime.max(self.clock);
        }
        share.waiting += 1;
        share.weight = weight.max(1);
        Some(Ticket(index))
    }

    /// The share whose request goes next: of the most urgent rank that
    /// waits, the least virtual time, the first in the table on a tie.
    fn next(&self) -> Option<usize> {
        self.shares
            .iter()
            .enumerate()
            .filter(|(_, share)| share.waiting > 0)
            .min_by_key(|(index, share)| (share.rank, share.vtime, *index))
            .map(|(index, _)| index)
    }

    /// Whether the request of `ticket` goes now: the disk has room for it
    /// and it is the next.
    #[must_use]
    pub fn may_go(&self, ticket: Ticket) -> bool {
        self.in_flight < self.depth && self.next() == Some(ticket.0)
    }

    /// Whether the next request that waits, whichever it is, may go now.
    #[must_use]
    pub fn next_may_go(&self) -> bool {
        self.in_flight < self.depth && self.next().is_some()
    }

    /// The request of `ticket` goes, at `cost` ([`Shares::cost`]).
    pub fn go(&mut self, ticket: Ticket, cost: u64) {
        let share = &mut self.shares[ticket.0];
        debug_assert!(share.waiting > 0);
        share.waiting -= 1;
        share.in_flight += 1;
        self.clock = self.clock.max(share.vtime);
        share.vtime = share.vtime.saturating_add(cost.saturating_mul(TIME_PER_COST) / share.weight);
        self.in_flight += 1;
        if self.clock > REBASE {
            self.rebase();
        }
    }

    /// Moves every virtual time back by the same amount, the least of
    /// them, which keeps their order. A share none of whose requests wait
    /// starts its next from the disk's time at the earliest, so it is
    /// moved there first.
    fn rebase(&mut self) {
        let clock = self.clock;
        for share in self.shares.iter_mut().filter(|s| s.used && s.waiting == 0) {
            share.vtime = share.vtime.max(clock);
        }
        let least = self.shares.iter().filter(|s| s.used).map(|s| s.vtime).fold(clock, u64::min);
        for share in self.shares.iter_mut().filter(|s| s.used) {
            share.vtime -= least;
        }
        self.clock -= least;
    }

    /// The request of `ticket` gave up waiting before it went.
    pub fn leave(&mut self, ticket: Ticket) {
        let share = &mut self.shares[ticket.0];
        debug_assert!(share.waiting > 0);
        share.waiting -= 1;
        self.forget_if_idle();
    }

    /// The request of `ticket`, gone, is done.
    pub fn done(&mut self, ticket: Ticket) {
        let share = &mut self.shares[ticket.0];
        debug_assert!(share.in_flight > 0);
        share.in_flight -= 1;
        self.in_flight -= 1;
        self.forget_if_idle();
    }

    /// Forgets every share once the disk is idle.
    fn forget_if_idle(&mut self) {
        if self.in_flight == 0 && self.shares.iter().all(Share::idle) {
            self.shares.fill(Share::default());
            self.clock = 0;
        }
    }

    /// Whether requests wait to go.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.shares.iter().any(|share| share.waiting > 0)
    }

    /// Requests gone and not yet done.
    #[must_use]
    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn a_full_bucket_holds_a_second_and_then_keeps_to_the_rate() {
        let mut rate = Rate::new(1000);
        let now = 10 * SECOND;
        // A second's worth at once, then each unit waits a millisecond.
        assert_eq!(rate.take(1000, now), 0);
        assert_eq!(rate.take(1, now), MS);
        assert_eq!(rate.take(1, now), 2 * MS);
        // Time pays the debt back, and the bucket fills again, no fuller
        // than a second's worth.
        assert_eq!(rate.take(1, now + 2 * MS), MS);
        assert_eq!(rate.take(1000, now + 10 * SECOND), 0);
        assert_eq!(rate.take(1, now + 10 * SECOND), MS);
    }

    #[test]
    fn a_take_larger_than_the_bucket_waits_for_the_rest() {
        let mut rate = Rate::new(100);
        assert_eq!(rate.take(250, 5 * SECOND), 3 * SECOND / 2);
        // Over 10 seconds of takes, at most 10 seconds' worth and the
        // bucket's second go without waiting.
        let mut rate = Rate::new(1_000_000);
        let mut free = 0;
        for step in 0..10_000u64 {
            let now = SECOND + step * MS;
            if rate.take(1100, now) == 0 {
                free += 1100;
            }
        }
        assert!(free <= 11_000_000, "{free}");
        // A rate of 0 is taken as 1.
        assert_eq!(Rate::new(0).per_second(), 1);
    }

    #[test]
    fn rates_of_bytes_split_fairly_between_takers() {
        // Two takers each take 64 KiB as soon as their wait ends, against
        // a 10 MB/s rate: together they keep to it, each with half.
        let mut rate = Rate::new(10_000_000);
        let (mut next, mut taken) = ([SECOND; 2], [0u64; 2]);
        let end = 11 * SECOND;
        while next.iter().any(|&at| at < end) {
            let who = usize::from(next[1] < next[0]);
            let now = next[who];
            next[who] = now + rate.take(65_536, now).max(1);
            if now < end {
                taken[who] += 65_536;
            }
        }
        let total = taken[0] + taken[1];
        // Ten seconds at the rate, the bucket's second, and one take each.
        assert!(total <= 110_000_000 + 2 * 65_536, "{total}");
        assert!(total >= 100_000_000, "{total}");
        assert!(taken[0].abs_diff(taken[1]) <= 2 * 65_536, "{taken:?}");
    }

    /// Runs domains that each keep four requests waiting on a disk of
    /// `depth`, which finishes its oldest request whenever it is full,
    /// for `requests` requests in all: how many went for each.
    fn share_out(weights: &[u64], depth: u32, requests: usize) -> Vec<usize> {
        let mut shares = Shares::new(depth).unwrap();
        let domain = |who: usize| u32::try_from(who).unwrap() + 1;
        // A domain's requests share its ticket.
        let tickets: Vec<Ticket> = (0..weights.len())
            .map(|who| {
                let ticket = shares.arrive(domain(who), 4, weights[who]).unwrap();
                for _ in 1..4 {
                    assert_eq!(shares.arrive(domain(who), 4, weights[who]), Some(ticket));
                }
                ticket
            })
            .collect();
        let mut went = vec![0; weights.len()];
        let mut on_disk = std::collections::VecDeque::new();
        for _ in 0..requests {
            if on_disk.len() == depth as usize {
                let who: usize = on_disk.pop_front().unwrap();
                shares.done(tickets[who]);
                assert_eq!(shares.arrive(domain(who), 4, weights[who]), Some(tickets[who]));
            }
            let who = (0..weights.len())
                .find(|&who| shares.may_go(tickets[who]))
                .expect("one request may always go");
            shares.go(tickets[who], Shares::cost(16, false));
            on_disk.push_back(who);
            went[who] += 1;
        }
        went
    }

    #[test]
    fn busy_domains_share_the_disk_by_weight() {
        let went = share_out(&[100, 400], 4, 10_000);
        let ratio = went[1] as f64 / went[0] as f64;
        assert!((3.8..4.2).contains(&ratio), "{went:?}");
        let went = share_out(&[100, 100, 200], 8, 12_000);
        assert!(went[0].abs_diff(went[1]) < 100, "{went:?}");
        let ratio = went[2] as f64 / went[0] as f64;
        assert!((1.9..2.1).contains(&ratio), "{went:?}");
        // One domain alone has the whole disk.
        assert_eq!(share_out(&[1], 4, 1000), vec![1000]);
    }

    #[test]
    fn a_more_urgent_rank_goes_first_and_room_is_kept_to_the_depth() {
        let mut shares = Shares::new(2).unwrap();
        let low = shares.arrive(1, 5, 10_000).unwrap();
        let urgent = shares.arrive(2, 1, 1).unwrap();
        assert!(!shares.may_go(low) && shares.may_go(urgent));
        shares.go(urgent, Shares::cost(1, true));
        assert!(shares.may_go(low));
        shares.go(low, Shares::cost(1, true));
        let again = shares.arrive(1, 5, 10_000).unwrap();
        assert_eq!(again, low);
        assert!(!shares.may_go(again), "two on a disk of depth 2");
        assert!(!shares.next_may_go());
        assert_eq!(shares.in_flight(), 2);
        shares.done(urgent);
        assert!(shares.may_go(again) && shares.next_may_go());
    }

    #[test]
    fn a_domain_back_from_a_pause_has_no_credit() {
        let mut shares = Shares::new(1).unwrap();
        // Domains 1 and 2 take turns while both wait.
        let one = shares.arrive(1, 4, 100).unwrap();
        let two = shares.arrive(2, 4, 100).unwrap();
        let turn = |shares: &mut Shares, ticket: Ticket, domain: u32, again: bool| {
            assert!(shares.may_go(ticket), "domain {domain} goes");
            shares.go(ticket, 10);
            shares.done(ticket);
            if again {
                assert_eq!(shares.arrive(domain, 4, 100), Some(ticket));
            }
        };
        for _ in 0..3 {
            turn(&mut shares, one, 1, true);
            turn(&mut shares, two, 2, true);
        }
        // Domain 2 pauses while domain 1 has the disk 100 times.
        let _ = shares.arrive(1, 4, 100);
        shares.leave(two);
        for _ in 0..100 {
            turn(&mut shares, one, 1, true);
        }
        // Back, it takes turns again rather than going 100 times first.
        assert_eq!(shares.arrive(2, 4, 100), Some(two));
        let mut order = Vec::new();
        for _ in 0..6 {
            let (ticket, domain) = if shares.may_go(one) { (one, 1) } else { (two, 2) };
            turn(&mut shares, ticket, domain, true);
            order.push(domain);
        }
        assert_eq!(order.iter().filter(|&&d| d == 2).count(), 3, "{order:?}");
        // And once the disk is idle, all is forgotten.
        shares.leave(one);
        shares.leave(one);
        shares.leave(two);
        assert!(!shares.waiting());
        assert_eq!(shares.clock, 0);
    }

    #[test]
    fn a_full_table_turns_requests_away_and_idle_shares_come_free() {
        let mut shares = Shares::new(64).unwrap();
        let tickets: Vec<Ticket> =
            (1..=SHARES as u32).map(|domain| shares.arrive(domain, 4, 100).unwrap()).collect();
        assert_eq!(shares.arrive(SHARES as u32 + 1, 4, 100), None);
        // The same domain at another rank needs a share of its own too.
        assert_eq!(shares.arrive(1, 3, 100), None);
        shares.leave(tickets[0]);
        assert!(shares.arrive(SHARES as u32 + 1, 4, 100).is_some());
        assert!(shares.waiting());
    }

    #[test]
    fn virtual_times_move_back_before_they_run_out() {
        let mut shares = Shares::new(1).unwrap();
        let one = shares.arrive(1, 4, 100).unwrap();
        let two = shares.arrive(2, 4, 100).unwrap();
        let idle = shares.arrive(3, 4, 100).unwrap();
        shares.leave(idle);
        shares.clock = REBASE;
        shares.shares[one.0].vtime = REBASE + 1;
        shares.shares[two.0].vtime = REBASE + 7;
        shares.go(one, 1);
        // Moved back by the least, domain 1's start: the order and the gap
        // stay, and the idle share is where the disk's time was.
        assert_eq!(shares.clock, 0);
        assert_eq!(shares.shares[one.0].vtime, TIME_PER_COST / WEIGHT_DEFAULT);
        assert_eq!(shares.shares[two.0].vtime, 6);
        assert_eq!(shares.shares[idle.0].vtime, 0);
        assert!(!shares.may_go(two), "the disk is full");
        shares.done(one);
        assert!(shares.may_go(two));
    }
}
