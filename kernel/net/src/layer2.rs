// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Frames between interfaces: VLAN tags and a bridge's forwarding
//! database (ADR 0333).
//!
//! A VLAN interface sends and takes its frames through its parent's
//! link with an IEEE 802.1Q tag, which names its VLAN: [`tag`] adds it
//! to a frame on its way down, [`untag`] takes it from one on its way up,
//! and [`vlan_of`] says which VLAN a frame received is tagged for.
//!
//! A bridge joins the links of its ports into one network, as a switch
//! does: a frame one port received goes out of the port its destination
//! was last heard from, or of every other port when that is not known or
//! it goes to a group of machines (broadcast or multicast), and to the
//! bridge's own stack when it is for it. [`Fdb`] is where it keeps where
//! each machine was heard from, for [`AGEING_NS`] after it was last
//! heard, as Linux's bridges do; [`forward`] says where a frame goes.
//!
//! These are the kernel's to call while it holds one stack's lock: none
//! takes a lock of its own.

use crate::FRAME;

/// The bytes of an 802.1Q tag: its type and the VLAN's number, with the
/// frame's priority.
pub const VLAN_TAG: usize = 4;

/// The longest frame a link takes or sends: one of [`FRAME`] bytes with
/// a VLAN tag, which a VLAN on it sends and takes.
pub const TAGGED_FRAME: usize = FRAME + VLAN_TAG;

/// The type that says a frame is tagged, 802.1Q's Tag Protocol
/// Identifier.
const TPID: [u8; 2] = [0x81, 0x00];

/// The bytes of the two MAC addresses at the start of every frame.
const ADDRESSES: usize = 12;

/// The first VLAN number an interface may have: 0 is a frame with a
/// priority and no VLAN.
pub const VLAN_FIRST: u16 = 1;
/// The last: 4095 is reserved.
pub const VLAN_LAST: u16 = 4094;

/// The VLAN `frame` is tagged for, if it is tagged for one.
pub fn vlan_of(frame: &[u8]) -> Option<u16> {
    if frame.len() < ADDRESSES + VLAN_TAG + 2 || frame[ADDRESSES..ADDRESSES + 2] != TPID {
        return None;
    }
    let id = u16::from_be_bytes([frame[14], frame[15]]) & 0x0fff;
    (VLAN_FIRST..=VLAN_LAST).contains(&id).then_some(id)
}

/// Copies `frame`, tagged, without its tag into `out`: its length, or
/// `None` if it is not tagged or `out` is too short.
pub fn untag(frame: &[u8], out: &mut [u8]) -> Option<usize> {
    vlan_of(frame)?;
    let len = frame.len() - VLAN_TAG;
    let out = out.get_mut(..len)?;
    out[..ADDRESSES].copy_from_slice(&frame[..ADDRESSES]);
    out[ADDRESSES..].copy_from_slice(&frame[ADDRESSES + VLAN_TAG..]);
    Some(len)
}

/// Copies `frame` into `out` tagged for VLAN `id`, with no priority:
/// its length, or `None` if it is shorter than its addresses and type or
/// `out` is too short.
pub fn tag(frame: &[u8], id: u16, out: &mut [u8]) -> Option<usize> {
    if frame.len() < ADDRESSES + 2 {
        return None;
    }
    let len = frame.len() + VLAN_TAG;
    let out = out.get_mut(..len)?;
    out[..ADDRESSES].copy_from_slice(&frame[..ADDRESSES]);
    out[ADDRESSES..ADDRESSES + 2].copy_from_slice(&TPID);
    out[ADDRESSES + 2..ADDRESSES + 4].copy_from_slice(&(id & 0x0fff).to_be_bytes());
    out[ADDRESSES + VLAN_TAG..].copy_from_slice(&frame[ADDRESSES..]);
    Some(len)
}

/// A frame's destination MAC address, and its source's.
pub fn addresses(frame: &[u8]) -> Option<([u8; 6], [u8; 6])> {
    let to = frame.get(..6)?.try_into().ok()?;
    let from = frame.get(6..12)?.try_into().ok()?;
    Some((to, from))
}

/// Whether `mac` names a group of machines, broadcast or multicast,
/// rather than one.
pub fn is_group(mac: [u8; 6]) -> bool {
    mac[0] & 1 != 0
}

/// How long a bridge remembers where it heard a machine, as Linux's
/// `ageing_time`: five minutes.
pub const AGEING_NS: u64 = 300_000_000_000;

/// The most machines a bridge remembers; the one heard longest ago makes
/// room for another.
pub const FDB_ENTRIES: usize = 256;

/// Where a bridge heard a machine, and when last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heard {
    /// The machine's MAC address.
    pub mac: [u8; 6],
    /// The port, by its interface's number.
    pub port: usize,
    /// When, by the kernel's clock.
    pub at_ns: u64,
}

/// A bridge's forwarding database: where it heard each machine, in
/// `entries`, as many as they hold ([`FDB_ENTRIES`] in the kernel's).
pub struct Fdb<E> {
    entries: E,
}

impl<E: AsRef<[Option<Heard>]> + AsMut<[Option<Heard>]>> Fdb<E> {
    /// One that keeps what it hears in `entries`, all empty.
    pub const fn new(entries: E) -> Fdb<E> {
        Fdb { entries }
    }

    /// Notes that `mac`, one machine's, was heard on `port` at `now_ns`.
    pub fn learn(&mut self, mac: [u8; 6], port: usize, now_ns: u64) {
        if is_group(mac) || mac == [0; 6] {
            return;
        }
        let heard = Heard { mac, port, at_ns: now_ns };
        let entries = self.entries.as_mut();
        if let Some(entry) = entries.iter_mut().flatten().find(|entry| entry.mac == mac) {
            *entry = heard;
            return;
        }
        let slot = match entries.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                let oldest = entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| entry.map_or(0, |entry| entry.at_ns));
                oldest.map_or(0, |(at, _)| at)
            }
        };
        if let Some(entry) = entries.get_mut(slot) {
            *entry = Some(heard);
        }
    }

    /// The port `mac` was last heard on, if within [`AGEING_NS`] of
    /// `now_ns`.
    pub fn find(&self, mac: [u8; 6], now_ns: u64) -> Option<usize> {
        self.entries
            .as_ref()
            .iter()
            .flatten()
            .find(|entry| entry.mac == mac && now_ns.saturating_sub(entry.at_ns) < AGEING_NS)
            .map(|entry| entry.port)
    }

    /// Forgets the machines heard on `port`, which left the bridge.
    pub fn forget(&mut self, port: usize) {
        for entry in self.entries.as_mut() {
            if entry.is_some_and(|entry| entry.port == port) {
                *entry = None;
            }
        }
    }

    /// The machines heard within [`AGEING_NS`] of `now_ns`.
    pub fn heard(&self, now_ns: u64) -> impl Iterator<Item = Heard> + '_ {
        self.entries
            .as_ref()
            .iter()
            .flatten()
            .copied()
            .filter(move |entry| now_ns.saturating_sub(entry.at_ns) < AGEING_NS)
    }
}

/// Where a frame goes on a bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forward {
    /// To the bridge's own stack.
    pub up: bool,
    /// Out of one port, by its interface's number.
    pub port: Option<usize>,
    /// Out of every port but the one it came in on, if any.
    pub flood: bool,
}

/// Where a frame to `to` goes on a bridge whose own address is `own`,
/// which heard what `fdb` says: from port `from`, or from the bridge's
/// own stack for `None`. A frame to a group goes up and to every other
/// port; one to the bridge, up; one to a machine heard on a port, out of
/// that port, unless it came in on it; one to a machine not heard,
/// everywhere but up.
pub fn forward<E: AsRef<[Option<Heard>]> + AsMut<[Option<Heard>]>>(
    fdb: &Fdb<E>,
    own: [u8; 6],
    to: [u8; 6],
    from: Option<usize>,
    now_ns: u64,
) -> Forward {
    let came_in = from.is_some();
    if is_group(to) {
        return Forward { up: came_in, port: None, flood: true };
    }
    if to == own {
        return Forward { up: came_in, port: None, flood: false };
    }
    match fdb.find(to, now_ns) {
        Some(port) if Some(port) == from => Forward { up: false, port: None, flood: false },
        Some(port) => Forward { up: false, port: Some(port), flood: false },
        None => Forward { up: false, port: None, flood: true },
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    const A: [u8; 6] = [2, 0, 0, 0, 0, 0xa];
    const B: [u8; 6] = [2, 0, 0, 0, 0, 0xb];
    const OWN: [u8; 6] = [2, 0, 0, 0, 0, 0xf];
    const ALL: [u8; 6] = [0xff; 6];

    /// A frame from `from` to `to` of IPv4, with `body`.
    fn frame(to: [u8; 6], from: [u8; 6], body: &[u8]) -> Vec<u8> {
        let mut out = to.to_vec();
        out.extend_from_slice(&from);
        out.extend_from_slice(&[0x08, 0x00]);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn tags_go_on_and_come_off() {
        let plain = frame(B, A, b"payload");
        let mut tagged = [0u8; 64];
        let len = tag(&plain, 7, &mut tagged).unwrap();
        assert_eq!(len, plain.len() + VLAN_TAG);
        assert_eq!(&tagged[12..18], [0x81, 0x00, 0x00, 0x07, 0x08, 0x00]);
        assert_eq!(vlan_of(&tagged[..len]), Some(7));
        assert_eq!(vlan_of(&plain), None);
        let mut back = [0u8; 64];
        let back_len = untag(&tagged[..len], &mut back).unwrap();
        assert_eq!(&back[..back_len], plain.as_slice());
        // The priority bits are not the VLAN; VLAN 0 and 4095 are none.
        tagged[14] = 0xe0;
        assert_eq!(vlan_of(&tagged[..len]), Some(7));
        tagged[14..16].copy_from_slice(&[0xe0, 0]);
        assert_eq!(vlan_of(&tagged[..len]), None);
        tagged[14..16].copy_from_slice(&[0x0f, 0xff]);
        assert_eq!(vlan_of(&tagged[..len]), None);
        // Too short to tag, or to tag into.
        assert_eq!(tag(&plain[..13], 7, &mut [0; 64]), None);
        assert_eq!(tag(&plain, 7, &mut [0; 8]), None);
        assert_eq!(untag(&plain, &mut back), None);
    }

    #[test]
    fn a_bridge_learns_where_machines_are_and_forgets_them() {
        let mut fdb = Fdb::new([None; FDB_ENTRIES]);
        // A group's frame goes up and everywhere; an unknown machine's
        // everywhere but up.
        assert_eq!(
            forward(&fdb, OWN, ALL, Some(1), 0),
            Forward { up: true, port: None, flood: true }
        );
        assert_eq!(
            forward(&fdb, OWN, B, Some(1), 0),
            Forward { up: false, port: None, flood: true }
        );
        fdb.learn(A, 1, 0);
        fdb.learn(B, 2, 0);
        fdb.learn(ALL, 3, 0);
        assert_eq!(fdb.heard(0).count(), 2);
        // To the bridge, up; to a known machine, out of its port, unless
        // it came in there.
        assert_eq!(
            forward(&fdb, OWN, OWN, Some(1), 0),
            Forward { up: true, port: None, flood: false }
        );
        assert_eq!(
            forward(&fdb, OWN, B, Some(1), 0),
            Forward { up: false, port: Some(2), flood: false }
        );
        assert_eq!(
            forward(&fdb, OWN, B, Some(2), 0),
            Forward { up: false, port: None, flood: false }
        );
        // What the bridge sends goes out, never up.
        assert_eq!(
            forward(&fdb, OWN, B, None, 0),
            Forward { up: false, port: Some(2), flood: false }
        );
        assert_eq!(
            forward(&fdb, OWN, ALL, None, 0),
            Forward { up: false, port: None, flood: true }
        );
        // A machine moves; one not heard for five minutes is forgotten.
        fdb.learn(B, 1, 10);
        assert_eq!(fdb.find(B, 10), Some(1));
        assert_eq!(fdb.find(A, AGEING_NS), None);
        assert_eq!(fdb.find(B, AGEING_NS), Some(1));
        fdb.forget(1);
        assert_eq!(fdb.find(B, 10), None);
    }

    #[test]
    fn the_machine_heard_longest_ago_makes_room() {
        let mut fdb = Fdb::new([None; FDB_ENTRIES]);
        for n in 0..FDB_ENTRIES {
            fdb.learn([6, 0, 0, 0, (n >> 8) as u8, n as u8], 1, n as u64 + 1);
        }
        fdb.learn(A, 2, 1_000);
        assert_eq!(fdb.find([6, 0, 0, 0, 0, 0], 1_000), None);
        assert_eq!(fdb.find([6, 0, 0, 0, 0, 1], 1_000), Some(1));
        assert_eq!(fdb.find(A, 1_000), Some(2));
    }
}
