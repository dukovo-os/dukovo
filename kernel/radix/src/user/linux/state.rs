// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the kernel keeps of a Linux program beyond what every process
//! has (ADR 0611): whether its process runs Linux's calls, where its
//! break is, and its threads' Linux words. The rest of what Linux keeps
//! for a program, the kernel keeps for every program already: the
//! working directory (`PosixState::directory`), the file mode mask, the
//! descriptors (slots from SLOT_FD_BASE) with their close-on-exec flags,
//! and each open directory's listing cursor (its description's offset).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_sync::Mutex;

/// A process's Linux side.
pub struct LinuxProcess {
    /// Whether its program is a Linux program: what each of its system
    /// calls looks at first. Set by `exec`, before the program's thread
    /// starts, and copied by `fork`.
    on: AtomicBool,
    /// Whether it has run `exec` since it was forked: what the parent of a
    /// `vfork` waits for, unless the child ends first.
    execed: AtomicBool,
    /// Its break, which `brk` moves: taken before any lock of its address
    /// space, and held while the break's memory is mapped.
    pub brk: Mutex<Break>,
}

/// Where a Linux program's break starts and where it is: the end of the
/// memory `brk` gives it, which starts empty past the end of its image.
#[derive(Clone, Copy, Debug, Default)]
pub struct Break {
    /// Where it starts: `brk` never moves it below this.
    pub start: u64,
    /// Where it is now.
    pub end: u64,
}

impl LinuxProcess {
    /// A process that runs no Linux program.
    pub const fn new() -> LinuxProcess {
        LinuxProcess {
            on: AtomicBool::new(false),
            execed: AtomicBool::new(false),
            brk: Mutex::new(Break { start: 0, end: 0 }),
        }
    }

    /// Whether its program is a Linux program.
    #[inline(always)]
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Whether it has run `exec` since it was forked.
    pub fn execed(&self) -> bool {
        self.execed.load(Ordering::Acquire)
    }

    /// Its new program is a Linux program if `on`, with its break at
    /// `brk`: set by `exec` while no thread of the process runs.
    pub fn start(&self, on: bool, brk: u64) {
        *self.brk.lock() = Break { start: brk, end: brk };
        self.on.store(on, Ordering::Relaxed);
        self.execed.store(true, Ordering::Release);
    }

    /// A forked child's side is a copy of its parent's: set before the
    /// child's thread starts.
    pub fn copy_from(&self, parent: &LinuxProcess) {
        let brk = *parent.brk.lock();
        *self.brk.lock() = brk;
        self.on.store(parent.on(), Ordering::Relaxed);
    }
}

/// A thread's Linux words: where it writes its thread number when it
/// starts, where the kernel clears that number when it exits, and the
/// head of its robust futex list.
pub struct LinuxThread {
    /// CLONE_CHILD_SETTID's word: 0 for none. The new thread writes its
    /// number there itself before its first instruction, as Linux's does,
    /// so that the write meets its own memory's protection and
    /// copy-on-write as any write of its program would (`starting`).
    pub set_child_tid: AtomicU64,
    /// `set_tid_address`'s and CLONE_CHILD_CLEARTID's word: 0 for none.
    /// When the thread exits, the kernel writes 0 there and wakes a
    /// thread waiting on it, which is how a C library's join waits.
    pub clear_child_tid: AtomicU64,
    /// `set_robust_list`'s head: 0 for none. The kernel walks the list
    /// when the thread ends or runs `exec`, and forgets it (`robust`).
    pub robust_list: AtomicU64,
    /// The length `set_robust_list` was given with the head.
    pub robust_len: AtomicU64,
    /// The timeout, plus 1, in milliseconds that the `poll` a signal
    /// interrupted runs again with, set by that `poll` and taken at once
    /// by the call's end; 0 for none.
    poll_left: AtomicU64,
}

impl LinuxThread {
    /// A thread with no Linux words.
    pub const fn new() -> LinuxThread {
        LinuxThread {
            set_child_tid: AtomicU64::new(0),
            clear_child_tid: AtomicU64::new(0),
            robust_list: AtomicU64::new(0),
            robust_len: AtomicU64::new(0),
            poll_left: AtomicU64::new(0),
        }
    }

    /// The `poll` that fails with an interrupt is to run again waiting
    /// `ms` milliseconds.
    pub fn set_poll_left(&self, ms: u64) {
        self.poll_left.store(ms.saturating_add(1), Ordering::Relaxed);
    }

    /// The milliseconds the interrupted `poll` runs again with, if it set
    /// them.
    pub fn take_poll_left(&self) -> Option<u64> {
        self.poll_left.swap(0, Ordering::Relaxed).checked_sub(1)
    }
}

/// Run by a program's thread before its first instruction: writes its
/// thread number at its CLONE_CHILD_SETTID word, if it has one. A word
/// it cannot write is left alone, as Linux leaves it.
pub fn starting(thread: &crate::sched::ThreadRef, user: &crate::user::UserThread) {
    let word = user.linux.set_child_tid.swap(0, Ordering::Relaxed);
    if word != 0 && word.is_multiple_of(4) && radix_ipc::range::user_bytes(word, 4) {
        let _ =
            <crate::Arch as radix_hal::UserMode>::copy_to_user(word, &tid(thread).to_le_bytes());
    }
}

/// A thread's number as Linux gives it: a `pid_t`.
pub fn tid(thread: &crate::sched::ThreadRef) -> u32 {
    // 32 bits, as Dukovo's own C library's `pid_t` takes the same
    // numbers (docs/deferred.md: numbers past 2^31).
    thread.id() as u32
}
