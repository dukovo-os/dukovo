// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! kqueue's rules for an event queue's watches (Phase 17, ADR 0353): what
//! a change asks of a queue, what it does to a watch, and what a look at a
//! watch reports and leaves, as FreeBSD's and OpenBSD's kqueue(2) have
//! them.
//!
//! The kernel's event queue (`radix::user::queue`) keeps each watch's
//! [`State`] under the queue's lock, finds what each descriptor is ready
//! for, and carries out what this module decides: [`read`] takes a change
//! from the 32 bytes of the C library's `struct kevent`, [`asks`] says
//! what it asks for, [`State::added`] and [`State::change`] what it does
//! to a watch, and [`State::report`] what a look at the watch reports and
//! what becomes of it after. A change that fails comes back as
//! [`error_event`] says. abi/event_queue.ridl is the contract.
//!
//! A user event's own flags and value are those of FreeBSD's EVFILT_USER,
//! with one difference: a user event with `clear` forgets its flags and
//! value when it is reported, as it forgets that it was triggered, so that
//! each report carries only what came since the one before.

use radix_abi::event_queue::{
    QueueEvent, QueueFilter, QueueFlags, USER_AND, USER_CONTROL, USER_COPY, USER_FLAGS, USER_OR,
    USER_TRIGGER,
};
use radix_abi::process::PollEvents;

/// The flags a change may carry. The others, among them `error` and
/// `eof`, are cleared, as FreeBSD clears its EV_SYSFLAGS.
const CHANGE_FLAGS: QueueFlags = QueueFlags(0x00ff);

/// The flags a watch keeps from when it was added, and reports in each of
/// its events.
const OPTIONS: QueueFlags =
    QueueFlags::ONESHOT.union(QueueFlags::CLEAR).union(QueueFlags::DISPATCH);

/// The events with which a descriptor reads or writes no more: `eof`.
const ENDED: PollEvents = PollEvents::HANG_UP.union(PollEvents::ERROR);

/// A change `wait` cannot make, whatever the queue holds: the kernel's
/// invalid_argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;

/// What a change asks of the watch with its `ident` and `filter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asks {
    /// To add it, or to change the one the queue holds (`add`).
    Add,
    /// To delete it (`delete`).
    Delete,
    /// To change the one the queue holds (neither).
    Change,
}

/// The change or event in `bytes`, as the C library's `struct kevent`
/// lays it out.
pub fn read(bytes: &[u8; QueueEvent::SIZE]) -> QueueEvent {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let u64_at = |at: usize| {
        let mut word = [0; 8];
        word.copy_from_slice(&bytes[at..at + 8]);
        u64::from_le_bytes(word)
    };
    QueueEvent {
        ident: u64_at(QueueEvent::OFFSET_IDENT),
        filter: u16_at(QueueEvent::OFFSET_FILTER),
        flags: u16_at(QueueEvent::OFFSET_FLAGS),
        fflags: u32_at(QueueEvent::OFFSET_FFLAGS),
        data: u64_at(QueueEvent::OFFSET_DATA) as i64,
        udata: u64_at(QueueEvent::OFFSET_UDATA),
    }
}

/// The bytes of `event`, as [`read`] takes them.
pub fn write(event: &QueueEvent) -> [u8; QueueEvent::SIZE] {
    let mut bytes = [0; QueueEvent::SIZE];
    let mut put = |at: usize, field: &[u8]| bytes[at..at + field.len()].copy_from_slice(field);
    put(QueueEvent::OFFSET_IDENT, &event.ident.to_le_bytes());
    put(QueueEvent::OFFSET_FILTER, &event.filter.to_le_bytes());
    put(QueueEvent::OFFSET_FLAGS, &event.flags.to_le_bytes());
    put(QueueEvent::OFFSET_FFLAGS, &event.fflags.to_le_bytes());
    put(QueueEvent::OFFSET_DATA, &event.data.to_le_bytes());
    put(QueueEvent::OFFSET_UDATA, &event.udata.to_le_bytes());
    bytes
}

/// What `change` asks for, of the watch of which filter: `None` for one of
/// filter 0, which `wait` passes over, as FreeBSD's `kevent` does.
///
/// # Errors
///
/// [`Invalid`] for a filter the queue does not know, or `add` together
/// with `delete`.
pub fn asks(change: &QueueEvent) -> Result<Option<(QueueFilter, Asks)>, Invalid> {
    if change.filter == 0 {
        return Ok(None);
    }
    let filter = QueueFilter::from_raw(change.filter).ok_or(Invalid)?;
    let flags = flags_of(change);
    let asks = match (flags.contains(QueueFlags::ADD), flags.contains(QueueFlags::DELETE)) {
        (true, true) => return Err(Invalid),
        (true, false) => Asks::Add,
        (false, true) => Asks::Delete,
        (false, false) => Asks::Change,
    };
    Ok(Some((filter, asks)))
}

/// The event that reports `change` itself, with `status`, the kernel's
/// status of the change (0 when it worked, for a receipt), in `data`: the
/// change as it was passed, with only the `error` flag, as FreeBSD's
/// `kevent` writes it.
pub fn error_event(change: &QueueEvent, status: u64) -> QueueEvent {
    QueueEvent {
        flags: QueueFlags::ERROR.bits() as u16,
        // The status is a bit pattern, which the C library makes an errno.
        data: status as i64,
        ..*change
    }
}

/// The flags of a change that it may carry.
fn flags_of(change: &QueueEvent) -> QueueFlags {
    QueueFlags(u32::from(change.flags)).intersection(CHANGE_FLAGS)
}

/// A user event's own flags after a change with `fflags`: kept, kept where
/// the change's are also set, joined with the change's, or replaced by
/// them, as the change's USER_CONTROL bits say.
fn touched(own: u32, fflags: u32) -> u32 {
    let flags = fflags & USER_FLAGS;
    match fflags & USER_CONTROL {
        USER_AND => own & flags,
        USER_OR => own | flags,
        USER_COPY => flags,
        _ => own,
    }
}

/// What becomes of a watch once it has been reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// It is looked at again by the next `wait`: a level-triggered watch,
    /// which is reported for as long as it stays ready.
    LookAgain,
    /// It is looked at again once what it watches changes, or a change
    /// enables it: one with `clear` or `dispatch`.
    AwaitChange,
    /// It is deleted: a oneshot watch.
    Delete,
}

/// What a watch keeps between looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// Its `oneshot`, `clear` and `dispatch`, as it was added.
    options: QueueFlags,
    /// Whether it may be reported.
    enabled: bool,
    /// The program's own value, which its events carry.
    udata: u64,
    /// A user event's: whether a change has triggered it, its own flags,
    /// and its value.
    triggered: bool,
    fflags: u32,
    value: i64,
}

impl State {
    /// The watch `change` adds, of `filter`: enabled unless the change has
    /// `disable` without `enable`, as [`State::change`] has it.
    pub fn added(change: &QueueEvent, filter: QueueFilter) -> State {
        let flags = flags_of(change);
        let mut state = State {
            options: flags.intersection(OPTIONS),
            enabled: flags.contains(QueueFlags::ENABLE) || !flags.contains(QueueFlags::DISABLE),
            udata: change.udata,
            triggered: false,
            fflags: 0,
            value: 0,
        };
        if filter == QueueFilter::User {
            state.touch(change);
        }
        state
    }

    /// Makes `change` to the watch, of `filter`, which an `add` (`adding`)
    /// or a plain change names: sets its value, triggers a user event and
    /// sets its flags and value, and enables or disables it. `enable`
    /// wins over `disable`, as on FreeBSD; an `add` enables it unless it
    /// has `disable`. Its options stay as they were added.
    pub fn change(&mut self, change: &QueueEvent, filter: QueueFilter, adding: bool) {
        let flags = flags_of(change);
        self.udata = change.udata;
        if filter == QueueFilter::User {
            self.touch(change);
        }
        if flags.contains(QueueFlags::ENABLE) || (adding && !flags.contains(QueueFlags::DISABLE)) {
            self.enabled = true;
        } else if flags.contains(QueueFlags::DISABLE) {
            self.enabled = false;
        }
    }

    /// A user event's part of a change: USER_TRIGGER makes it ready, the
    /// change's flags apply to its own, and the change's `data` is its
    /// value.
    fn touch(&mut self, change: &QueueEvent) {
        if change.fflags & USER_TRIGGER != 0 {
            self.triggered = true;
        }
        self.fflags = touched(self.fflags, change.fflags);
        self.value = change.data;
    }

    /// Whether it may be reported, so that a look at it is worth making.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// What a look at the watch of `ident` and `filter` reports, if it is
    /// enabled and ready, and what becomes of it then: a `read` or `write`
    /// watch is ready for what its descriptor was found ready for,
    /// `events`, and a user event once triggered (`events` aside). A
    /// reported watch with `dispatch` is disabled, and a user event with
    /// `clear` forgets what triggered it.
    pub fn report(
        &mut self,
        ident: u64,
        filter: QueueFilter,
        events: PollEvents,
    ) -> Option<(QueueEvent, Then)> {
        if !self.enabled {
            return None;
        }
        let user = filter == QueueFilter::User;
        let ready = match filter {
            QueueFilter::User => self.triggered,
            QueueFilter::Read => !events.intersection(PollEvents::READABLE.union(ENDED)).is_empty(),
            QueueFilter::Write => {
                !events.intersection(PollEvents::WRITABLE.union(ENDED)).is_empty()
            }
        };
        if !ready {
            return None;
        }
        let mut flags = self.options;
        if !user && !events.intersection(ENDED).is_empty() {
            flags = flags.union(QueueFlags::EOF);
        }
        let event = QueueEvent {
            ident,
            filter: filter.raw(),
            // Every flag of an event is in the low 16 bits.
            flags: flags.bits() as u16,
            fflags: if user { self.fflags } else { 0 },
            data: if user { self.value } else { 0 },
            udata: self.udata,
        };
        Some((event, self.reported()))
    }

    /// What a report leaves of the watch, and what becomes of it.
    fn reported(&mut self) -> Then {
        if self.options.contains(QueueFlags::ONESHOT) {
            return Then::Delete;
        }
        if self.options.contains(QueueFlags::DISPATCH) {
            self.enabled = false;
        }
        if self.options.contains(QueueFlags::CLEAR) {
            self.triggered = false;
            self.fflags = 0;
            self.value = 0;
            return Then::AwaitChange;
        }
        if self.options.contains(QueueFlags::DISPATCH) {
            Then::AwaitChange
        } else {
            Then::LookAgain
        }
    }
}
