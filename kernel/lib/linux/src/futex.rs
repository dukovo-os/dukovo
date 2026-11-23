// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `op` argument of Linux's `futex` call.
//!
//! `futex(uaddr, op, val, timeout, uaddr2, val3)` packs a command and two
//! flags into `op`. This module splits them so that the table can answer
//! the commands the kernel's futex has (wait and wake, each with a bit set,
//! requeue, and wake with an operation) and refuse the rest, the ones of
//! priority inheritance, with ENOSYS.
//!
//! The timeout is relative for `FUTEX_WAIT` and absolute for
//! `FUTEX_WAIT_BITSET`, on the monotonic clock, or on the real-time clock
//! with `FUTEX_CLOCK_REALTIME`. Linux gives ENOSYS for that flag with any
//! command but the two waits, and the table does the same. For the
//! requeues and `FUTEX_WAKE_OP` the timeout argument is no time but a
//! second count, `val2`.
//!
//! [`WakeOp`] is `FUTEX_WAKE_OP`'s `val3`: an operation on the second word
//! and a comparison of its old value. [`death`] is what a thread's end
//! does to a word of its robust list (`set_robust_list`).

/// The futex is private to the process: waiters and wakers are found by
/// address in the caller's address space alone.
pub const FUTEX_PRIVATE_FLAG: u32 = 128;
/// The absolute timeout of `FUTEX_WAIT_BITSET` is on the real-time clock.
pub const FUTEX_CLOCK_REALTIME: u32 = 256;

/// Wait while the word equals a value, for a relative timeout.
pub const FUTEX_WAIT: u32 = 0;
/// Wake up to a number of waiters.
pub const FUTEX_WAKE: u32 = 1;
/// Wake up to a number of waiters, and move up to a second number of the
/// others to wait on a second word.
pub const FUTEX_REQUEUE: u32 = 3;
/// `FUTEX_REQUEUE`, once the first word is found to hold a value.
pub const FUTEX_CMP_REQUEUE: u32 = 4;
/// Change a second word by an operation, wake up to a number of waiters
/// on the first, and, if a comparison of the second's old value holds, up
/// to a second number of waiters on the second.
pub const FUTEX_WAKE_OP: u32 = 5;
/// Wait while the word equals a value, for an absolute timeout, and wake
/// only for a waker whose bit set shares a bit with the waiter's.
pub const FUTEX_WAIT_BITSET: u32 = 9;
/// Wake up to a number of waiters whose bit set shares a bit with the
/// waker's.
pub const FUTEX_WAKE_BITSET: u32 = 10;

/// The bit set that matches every other: what `FUTEX_WAIT` and `FUTEX_WAKE`
/// act as having.
pub const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

/// What a futex call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `FUTEX_WAIT`.
    Wait,
    /// `FUTEX_WAKE`.
    Wake,
    /// `FUTEX_WAIT_BITSET`.
    WaitBitset,
    /// `FUTEX_WAKE_BITSET`.
    WakeBitset,
    /// `FUTEX_REQUEUE`.
    Requeue,
    /// `FUTEX_CMP_REQUEUE`.
    CmpRequeue,
    /// `FUTEX_WAKE_OP`.
    WakeOp,
    /// Another command, by its number: those of priority inheritance, and
    /// numbers Linux does not know. The table answers ENOSYS.
    Other(u32),
}

/// An `op` argument, taken apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Op {
    /// The command, which is `op` without the two flags.
    pub command: Command,
    /// Whether `FUTEX_PRIVATE_FLAG` is set.
    pub private: bool,
    /// Whether `FUTEX_CLOCK_REALTIME` is set.
    pub realtime: bool,
}

/// The command and flags of a raw `op`.
pub fn op(raw: u32) -> Op {
    let number = raw & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    let command = match number {
        FUTEX_WAIT => Command::Wait,
        FUTEX_WAKE => Command::Wake,
        FUTEX_WAIT_BITSET => Command::WaitBitset,
        FUTEX_WAKE_BITSET => Command::WakeBitset,
        FUTEX_REQUEUE => Command::Requeue,
        FUTEX_CMP_REQUEUE => Command::CmpRequeue,
        FUTEX_WAKE_OP => Command::WakeOp,
        other => Command::Other(other),
    };
    Op {
        command,
        private: raw & FUTEX_PRIVATE_FLAG != 0,
        realtime: raw & FUTEX_CLOCK_REALTIME != 0,
    }
}

/// `FUTEX_WAKE_OP`'s operations on the second word: `FUTEX_OP_SET`,
/// `FUTEX_OP_ADD`, `FUTEX_OP_OR`, `FUTEX_OP_ANDN` and `FUTEX_OP_XOR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// The word becomes the argument.
    Set,
    /// The argument is added to the word, wrapping.
    Add,
    /// The argument's bits are set in the word.
    Or,
    /// The argument's bits are cleared in the word.
    AndNot,
    /// The argument's bits are flipped in the word.
    Xor,
}

/// `FUTEX_WAKE_OP`'s comparisons of the second word's old value with the
/// comparison's argument, both taken as signed: `FUTEX_OP_CMP_EQ`,
/// `_NE`, `_LT`, `_LE`, `_GT` and `_GE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// Equal.
    Equal,
    /// Not equal.
    NotEqual,
    /// Less.
    Less,
    /// Less or equal.
    LessOrEqual,
    /// Greater.
    Greater,
    /// Greater or equal.
    GreaterOrEqual,
}

/// The flag of `FUTEX_WAKE_OP`'s `val3` that makes the operation's
/// argument a shift: the argument becomes `1 << argument`.
pub const FUTEX_OP_OPARG_SHIFT: u32 = 8;

/// `FUTEX_WAKE_OP`'s `val3`, taken apart: bits 28 to 30 the operation,
/// bit 31 [`FUTEX_OP_OPARG_SHIFT`], bits 24 to 27 the comparison, bits 12
/// to 23 the operation's argument and bits 0 to 11 the comparison's, each
/// a signed 12-bit number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WakeOp {
    /// What is done to the word.
    pub operation: Operation,
    /// With what: a shift's argument already shifted.
    pub argument: u32,
    /// What the word's old value is compared by.
    pub comparison: Comparison,
    /// With what.
    pub compared_with: i32,
}

impl WakeOp {
    /// `val3` taken apart; `None` for an operation or a comparison Linux
    /// does not know, which it answers with ENOSYS. A shift by more than
    /// 31, or by less than 0, shifts by its low five bits, as Linux's does
    /// (it warns of the program in its log).
    pub fn decode(raw: u32) -> Option<WakeOp> {
        let operation = match (raw >> 28) & 7 {
            0 => Operation::Set,
            1 => Operation::Add,
            2 => Operation::Or,
            3 => Operation::AndNot,
            4 => Operation::Xor,
            _ => return None,
        };
        let comparison = match (raw >> 24) & 15 {
            0 => Comparison::Equal,
            1 => Comparison::NotEqual,
            2 => Comparison::Less,
            3 => Comparison::LessOrEqual,
            4 => Comparison::Greater,
            5 => Comparison::GreaterOrEqual,
            _ => return None,
        };
        let mut argument = signed_12(raw >> 12) as u32;
        if (raw >> 28) & FUTEX_OP_OPARG_SHIFT != 0 {
            argument = 1 << (argument & 31);
        }
        Some(WakeOp { operation, argument, comparison, compared_with: signed_12(raw) })
    }

    /// The word the operation makes of `old`.
    pub fn apply(&self, old: u32) -> u32 {
        match self.operation {
            Operation::Set => self.argument,
            Operation::Add => old.wrapping_add(self.argument),
            Operation::Or => old | self.argument,
            Operation::AndNot => old & !self.argument,
            Operation::Xor => old ^ self.argument,
        }
    }

    /// Whether the comparison holds of the word's old value `old`.
    pub fn holds(&self, old: u32) -> bool {
        let old = old as i32;
        let with = self.compared_with;
        match self.comparison {
            Comparison::Equal => old == with,
            Comparison::NotEqual => old != with,
            Comparison::Less => old < with,
            Comparison::LessOrEqual => old <= with,
            Comparison::Greater => old > with,
            Comparison::GreaterOrEqual => old >= with,
        }
    }
}

/// The low 12 bits of `raw` as a signed number.
fn signed_12(raw: u32) -> i32 {
    ((raw << 20) as i32) >> 20
}

/// A robust futex's word has waiters: its owner must wake one when it
/// lets the lock go.
pub const FUTEX_WAITERS: u32 = 0x8000_0000;
/// A robust futex's owner ended while it held the lock.
pub const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// The bits of a robust futex's word that hold its owner's thread number.
pub const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
/// The size of `struct robust_list_head`: the list's first entry, the
/// offset from an entry to its word, and the entry being added or taken
/// off (`list_op_pending`), 8 bytes each.
pub const ROBUST_LIST_HEAD_SIZE: u64 = 24;

/// What a thread's end does to a word of its robust list, as Linux's
/// `handle_futex_death`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Death {
    /// Nothing: the thread does not hold the lock.
    Leave,
    /// Wake one waiter, and leave the word as it is.
    Wake,
    /// Write this value in the word, if it still holds what was read, and
    /// then, if the word had [`FUTEX_WAITERS`] and is not a priority
    /// inheritance lock, wake one waiter.
    Mark(u32),
}

/// What the end of thread `tid` does to a word holding `word` of its
/// robust list: `pi` if the entry's low bit marks a priority inheritance
/// lock, `pending` if it is the list's `list_op_pending`. A word the
/// thread owns (its number in [`FUTEX_TID_MASK`]) gets
/// [`FUTEX_OWNER_DIED`], keeping [`FUTEX_WAITERS`]. A pending entry of an
/// ordinary lock whose word has no owner is one the thread let go of, or
/// was woken for, as it ended: a waiter that may have missed that is woken,
/// and the word is left alone, as Linux leaves it.
pub fn death(word: u32, tid: u32, pi: bool, pending: bool) -> Death {
    let owner = word & FUTEX_TID_MASK;
    if pending && !pi && owner == 0 {
        Death::Wake
    } else if owner != tid {
        Death::Leave
    } else {
        Death::Mark((word & FUTEX_WAITERS) | FUTEX_OWNER_DIED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FUTEX_OP(op, oparg, cmp, cmparg)` from Linux's headers.
    fn encode(op: u32, oparg: u32, cmp: u32, cmparg: u32) -> u32 {
        ((op & 0xf) << 28) | ((cmp & 0xf) << 24) | ((oparg & 0xfff) << 12) | (cmparg & 0xfff)
    }

    #[test]
    fn commands_are_split_from_their_flags() {
        assert_eq!(op(FUTEX_REQUEUE | FUTEX_PRIVATE_FLAG).command, Command::Requeue);
        assert_eq!(op(FUTEX_CMP_REQUEUE).command, Command::CmpRequeue);
        assert_eq!(op(FUTEX_WAKE_OP | FUTEX_PRIVATE_FLAG).command, Command::WakeOp);
        // FUTEX_LOCK_PI.
        assert_eq!(op(6).command, Command::Other(6));
    }

    #[test]
    fn wake_op_decodes_what_c_libraries_send() {
        // glibc's and musl's: FUTEX_OP(FUTEX_OP_SET, 0, FUTEX_OP_CMP_GT, 1).
        let op = WakeOp::decode(encode(0, 0, 4, 1)).unwrap();
        assert_eq!(op.operation, Operation::Set);
        assert_eq!(op.apply(7), 0);
        assert!(op.holds(2));
        assert!(!op.holds(1));
    }

    #[test]
    fn wake_op_arguments_are_signed() {
        let op = WakeOp::decode(encode(1, 0xfff, 2, 0x800)).unwrap();
        // Adding -1.
        assert_eq!(op.apply(5), 4);
        assert_eq!(op.apply(0), u32::MAX);
        // Less than -2048, as signed: no value of a 32-bit word but those
        // below it.
        assert_eq!(op.compared_with, -2048);
        assert!(op.holds((-2049i32) as u32));
        assert!(!op.holds(0));
        assert!(!op.holds(u32::MAX));
    }

    #[test]
    fn wake_op_operations() {
        let or = WakeOp::decode(encode(2, 0x0f0, 0, 0)).unwrap();
        assert_eq!(or.apply(0x101), 0x1f1);
        let andn = WakeOp::decode(encode(3, 0x0f0, 0, 0)).unwrap();
        assert_eq!(andn.apply(0xfff), 0xf0f);
        let xor = WakeOp::decode(encode(4, 0x0ff, 0, 0)).unwrap();
        assert_eq!(xor.apply(0xf0f), 0xff0);
    }

    #[test]
    fn wake_op_shifts() {
        // FUTEX_OP_OPARG_SHIFT with FUTEX_OP_OR: set bit 5.
        let op = WakeOp::decode(encode(FUTEX_OP_OPARG_SHIFT | 2, 5, 0, 0)).unwrap();
        assert_eq!(op.argument, 1 << 5);
        assert_eq!(op.apply(1), 0x21);
        // Out of range: the low five bits, as Linux's.
        let op = WakeOp::decode(encode(FUTEX_OP_OPARG_SHIFT, 33, 0, 0)).unwrap();
        assert_eq!(op.argument, 1 << 1);
        let op = WakeOp::decode(encode(FUTEX_OP_OPARG_SHIFT, 0xfff, 0, 0)).unwrap();
        assert_eq!(op.argument, 1 << 31);
    }

    #[test]
    fn wake_op_comparisons() {
        let holds = |cmp, old: i32| WakeOp::decode(encode(0, 0, cmp, 3)).unwrap().holds(old as u32);
        assert!(holds(0, 3) && !holds(0, 4));
        assert!(holds(1, 4) && !holds(1, 3));
        assert!(holds(2, 2) && !holds(2, 3) && holds(2, -1));
        assert!(holds(3, 3) && !holds(3, 4));
        assert!(holds(4, 4) && !holds(4, 3) && !holds(4, -1));
        assert!(holds(5, 3) && !holds(5, 2));
    }

    #[test]
    fn unknown_operations_and_comparisons_are_refused() {
        for op in 5..8 {
            assert_eq!(WakeOp::decode(encode(op, 0, 0, 0)), None);
        }
        for cmp in 6..16 {
            assert_eq!(WakeOp::decode(encode(0, 0, cmp, 0)), None);
        }
    }

    #[test]
    fn a_dying_owner_marks_its_word() {
        let tid = 1234;
        assert_eq!(death(tid, tid, false, false), Death::Mark(FUTEX_OWNER_DIED));
        assert_eq!(
            death(tid | FUTEX_WAITERS, tid, false, false),
            Death::Mark(FUTEX_WAITERS | FUTEX_OWNER_DIED)
        );
        // A second death of an owner already marked keeps the marks.
        assert_eq!(
            death(tid | FUTEX_WAITERS | FUTEX_OWNER_DIED, tid, false, false),
            Death::Mark(FUTEX_WAITERS | FUTEX_OWNER_DIED)
        );
    }

    #[test]
    fn a_word_of_another_owner_is_left() {
        let other = 77;
        assert_eq!(death(other, 1234, false, false), Death::Leave);
        assert_eq!(death(0, 1234, false, false), Death::Leave);
        assert_eq!(death(other | FUTEX_WAITERS, 1234, false, true), Death::Leave);
    }

    #[test]
    fn a_pending_unowned_word_wakes_a_waiter() {
        assert_eq!(death(0, 1234, false, true), Death::Wake);
        assert_eq!(death(FUTEX_WAITERS, 1234, false, true), Death::Wake);
        // Not for a priority inheritance lock.
        assert_eq!(death(0, 1234, true, true), Death::Leave);
    }
}
