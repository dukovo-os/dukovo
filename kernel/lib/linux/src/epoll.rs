// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! epoll (ADR 0612): the flags of `epoll_create1` and `epoll_ctl`,
//! `struct epoll_event`, and the change of the event queue's `poll`
//! filter that each `epoll_ctl` makes.
//!
//! An epoll instance is an event queue, and a registration its `poll`
//! watch of the descriptor: EPOLL_CTL_ADD adds one (failing if the queue
//! holds one for the same description, `create`), EPOLL_CTL_MOD changes the
//! events and options of the one held, and EPOLL_CTL_DEL deletes it.
//! EPOLLET is the watch's `clear`, EPOLLONESHOT its `dispatch` (a watch
//! disabled once reported, which the next EPOLL_CTL_MOD enables again),
//! and EPOLLEXCLUSIVE its `exclusive`. The events a watch asks for are
//! Linux's bits as they stand, which the queue reports with error and
//! hang-up always, as epoll does.

use radix_abi::event_queue::{QueueEvent, QueueFilter, QueueFlags};
use radix_abi::process::PipeFlags;

use crate::errno::EINVAL;
use crate::flags::O_CLOEXEC;

/// `epoll_create1`'s one flag.
pub const EPOLL_CLOEXEC: u32 = O_CLOEXEC;

/// `epoll_ctl`'s operations.
pub const EPOLL_CTL_ADD: u32 = 1;
/// See [`EPOLL_CTL_ADD`].
pub const EPOLL_CTL_DEL: u32 = 2;
/// See [`EPOLL_CTL_ADD`].
pub const EPOLL_CTL_MOD: u32 = 3;

/// The events of `struct epoll_event`, which are `poll`'s.
pub const EPOLLIN: u32 = 0x001;
/// See [`EPOLLIN`].
pub const EPOLLPRI: u32 = 0x002;
/// See [`EPOLLIN`].
pub const EPOLLOUT: u32 = 0x004;
/// See [`EPOLLIN`].
pub const EPOLLERR: u32 = 0x008;
/// See [`EPOLLIN`].
pub const EPOLLHUP: u32 = 0x010;
/// See [`EPOLLIN`].
pub const EPOLLRDHUP: u32 = 0x2000;
/// A registration one of whose kind wakes one waiter for a change.
pub const EPOLLEXCLUSIVE: u32 = 1 << 28;
/// Keeps the system awake while an event waits: only for a program with
/// CAP_BLOCK_SUSPEND, so dropped for every program, as Linux drops it.
pub const EPOLLWAKEUP: u32 = 1 << 29;
/// Disables a registration once it is reported, until EPOLL_CTL_MOD.
pub const EPOLLONESHOT: u32 = 1 << 30;
/// Edge-triggered: reported once for each change.
pub const EPOLLET: u32 = 1 << 31;

/// The input flags, which are not events.
const INPUT_FLAGS: u32 = EPOLLEXCLUSIVE | EPOLLWAKEUP | EPOLLONESHOT | EPOLLET;

/// What a registration with EPOLLEXCLUSIVE may ask for besides:
/// Linux's EPOLLEXCLUSIVE_OK_BITS.
const EXCLUSIVE_OK: u32 = EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLWAKEUP | EPOLLET;

/// The size of `struct epoll_event` on x86-64, where it is packed: a u32
/// of events, then the program's 8 bytes.
pub const EPOLL_EVENT_SIZE: usize = 12;

/// The most events one `epoll_wait` takes: Linux's EP_MAX_EVENTS.
pub const MAX_EVENTS: u32 = i32::MAX as u32 / EPOLL_EVENT_SIZE as u32;

/// The native flags of the queue `epoll_create1` makes with `flags`.
///
/// # Errors
///
/// EINVAL for any flag but EPOLL_CLOEXEC.
pub fn create_flags(flags: u32) -> Result<PipeFlags, i32> {
    if flags & !EPOLL_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    Ok(if flags & EPOLL_CLOEXEC != 0 { PipeFlags::CLOSE_ON_EXEC } else { PipeFlags::NONE })
}

/// One `struct epoll_event`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EpollEvent {
    /// The events asked for, or found, with the input flags in a change.
    pub events: u32,
    /// The program's own 8 bytes, `epoll_data_t`.
    pub data: u64,
}

impl EpollEvent {
    /// The record in `bytes`.
    pub fn read(bytes: &[u8; EPOLL_EVENT_SIZE]) -> EpollEvent {
        let mut events = [0; 4];
        events.copy_from_slice(&bytes[..4]);
        let mut data = [0; 8];
        data.copy_from_slice(&bytes[4..]);
        EpollEvent { events: u32::from_le_bytes(events), data: u64::from_le_bytes(data) }
    }

    /// Its bytes.
    pub fn bytes(&self) -> [u8; EPOLL_EVENT_SIZE] {
        let mut bytes = [0; EPOLL_EVENT_SIZE];
        bytes[..4].copy_from_slice(&self.events.to_le_bytes());
        bytes[4..].copy_from_slice(&self.data.to_le_bytes());
        bytes
    }

    /// The record an event of a `poll` watch reports: what was found, and
    /// the program's bytes.
    pub fn of(event: &QueueEvent) -> EpollEvent {
        EpollEvent { events: event.fflags, data: event.udata }
    }
}

/// The change of the event queue that `epoll_ctl` makes with `op` on
/// descriptor `fd`, with `event` for EPOLL_CTL_ADD and EPOLL_CTL_MOD.
/// EPOLLWAKEUP is dropped, as for a program without CAP_BLOCK_SUSPEND.
///
/// # Errors
///
/// EINVAL for an unknown `op`, EPOLLEXCLUSIVE with EPOLL_CTL_MOD, and
/// EPOLLEXCLUSIVE with an event or flag outside Linux's
/// EPOLLEXCLUSIVE_OK_BITS (among them EPOLLONESHOT and EPOLLRDHUP).
pub fn change(op: u32, fd: u32, event: EpollEvent) -> Result<QueueEvent, i32> {
    let mut flags = match op {
        EPOLL_CTL_ADD => QueueFlags::ADD.union(QueueFlags::CREATE).union(QueueFlags::ENABLE),
        EPOLL_CTL_MOD => QueueFlags::ENABLE,
        EPOLL_CTL_DEL => QueueFlags::DELETE,
        _ => return Err(EINVAL),
    };
    let mut fflags = 0;
    if op != EPOLL_CTL_DEL {
        let events = event.events;
        if events & EPOLLEXCLUSIVE != 0
            && (op == EPOLL_CTL_MOD || events & !(EXCLUSIVE_OK | EPOLLEXCLUSIVE) != 0)
        {
            return Err(EINVAL);
        }
        if events & EPOLLET != 0 {
            flags = flags.union(QueueFlags::CLEAR);
        }
        if events & EPOLLONESHOT != 0 {
            flags = flags.union(QueueFlags::DISPATCH);
        }
        if events & EPOLLEXCLUSIVE != 0 {
            flags = flags.union(QueueFlags::EXCLUSIVE);
        }
        fflags = events & !INPUT_FLAGS;
    }
    Ok(QueueEvent {
        ident: u64::from(fd),
        filter: QueueFilter::Poll.raw(),
        // Every flag of a change is in the low 16 bits.
        flags: flags.bits() as u16,
        fflags,
        data: 0,
        udata: if op == EPOLL_CTL_DEL { 0 } else { event.data },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(flags: QueueFlags) -> u16 {
        u16::try_from(flags.bits()).unwrap()
    }

    #[test]
    fn epoll_event_is_packed_in_twelve_bytes() {
        let event = EpollEvent { events: EPOLLIN | EPOLLRDHUP, data: 0x1122_3344_5566_7788 };
        let bytes = event.bytes();
        assert_eq!(bytes, [0x01, 0x20, 0, 0, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
        assert_eq!(EpollEvent::read(&bytes), event);
    }

    #[test]
    fn linuxs_numbers() {
        assert_eq!([EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD], [1, 2, 3]);
        assert_eq!(EPOLLEXCLUSIVE, 0x1000_0000);
        assert_eq!(EPOLLWAKEUP, 0x2000_0000);
        assert_eq!(EPOLLONESHOT, 0x4000_0000);
        assert_eq!(EPOLLET, 0x8000_0000);
        assert_eq!(EPOLL_CLOEXEC, 0x80000);
        assert_eq!(MAX_EVENTS, 178_956_970);
    }

    #[test]
    fn epoll_events_are_poll_events() {
        use radix_abi::process::PollEvents;
        assert_eq!(EPOLLIN, PollEvents::READABLE.bits());
        assert_eq!(EPOLLPRI, PollEvents::PRIORITY.bits());
        assert_eq!(EPOLLOUT, PollEvents::WRITABLE.bits());
        assert_eq!(EPOLLERR, PollEvents::ERROR.bits());
        assert_eq!(EPOLLHUP, PollEvents::HANG_UP.bits());
        assert_eq!(EPOLLRDHUP, PollEvents::READ_HANG_UP.bits());
    }

    #[test]
    fn create_takes_only_close_on_exec() {
        assert_eq!(create_flags(0), Ok(PipeFlags::NONE));
        assert_eq!(create_flags(EPOLL_CLOEXEC), Ok(PipeFlags::CLOSE_ON_EXEC));
        assert_eq!(create_flags(1), Err(EINVAL));
        assert_eq!(create_flags(EPOLL_CLOEXEC | 0o4000), Err(EINVAL));
    }

    #[test]
    fn add_creates_and_enables_a_poll_watch() {
        let event = EpollEvent { events: EPOLLIN | EPOLLRDHUP, data: 7 };
        let change = change(EPOLL_CTL_ADD, 5, event).unwrap();
        assert_eq!(change.ident, 5);
        assert_eq!(change.filter, QueueFilter::Poll.raw());
        let flags = QueueFlags::ADD.union(QueueFlags::CREATE).union(QueueFlags::ENABLE);
        assert_eq!(change.flags, bits(flags));
        assert_eq!(change.fflags, EPOLLIN | EPOLLRDHUP);
        assert_eq!(change.udata, 7);
    }

    #[test]
    fn edge_and_oneshot_are_clear_and_dispatch() {
        let event = EpollEvent { events: EPOLLIN | EPOLLET | EPOLLONESHOT | EPOLLWAKEUP, data: 1 };
        let change = change(EPOLL_CTL_MOD, 3, event).unwrap();
        let flags = QueueFlags::ENABLE.union(QueueFlags::CLEAR).union(QueueFlags::DISPATCH);
        assert_eq!(change.flags, bits(flags));
        // The input flags are not events, and EPOLLWAKEUP is dropped.
        assert_eq!(change.fflags, EPOLLIN);
    }

    #[test]
    fn delete_carries_no_event() {
        let event = EpollEvent { events: EPOLLIN, data: 9 };
        let change = change(EPOLL_CTL_DEL, 4, event).unwrap();
        assert_eq!(change.flags, bits(QueueFlags::DELETE));
        assert_eq!((change.fflags, change.udata), (0, 0));
    }

    #[test]
    fn exclusive_is_only_for_an_add_of_plain_events() {
        let add = |events| change(EPOLL_CTL_ADD, 1, EpollEvent { events, data: 0 });
        let made = add(EPOLLIN | EPOLLOUT | EPOLLET | EPOLLEXCLUSIVE).unwrap();
        assert_ne!(u32::from(made.flags) & QueueFlags::EXCLUSIVE.bits(), 0);
        for bad in [EPOLLONESHOT, EPOLLRDHUP, EPOLLPRI] {
            assert_eq!(add(EPOLLIN | EPOLLEXCLUSIVE | bad), Err(EINVAL));
        }
        let modify = change(EPOLL_CTL_MOD, 1, EpollEvent { events: EPOLLEXCLUSIVE, data: 0 });
        assert_eq!(modify, Err(EINVAL));
    }

    #[test]
    fn an_unknown_operation_is_invalid() {
        for op in [0, 4, 0x8000_0001] {
            assert_eq!(change(op, 1, EpollEvent::default()), Err(EINVAL));
        }
    }

    #[test]
    fn an_event_reports_what_was_found() {
        let found = QueueEvent { fflags: EPOLLIN | EPOLLHUP, udata: 42, ..QueueEvent::default() };
        assert_eq!(EpollEvent::of(&found), EpollEvent { events: EPOLLIN | EPOLLHUP, data: 42 });
    }
}
