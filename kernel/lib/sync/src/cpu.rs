// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! [`HeldCpu`]: the CPU the caller runs on, held for a moment, for state
//! kept for each CPU.
//!
//! State that each CPU keeps for itself, such as the capability store's
//! spare object indexes (ADR 0405), stays on that CPU's cache lines: no
//! other CPU takes them away. In the kernel, holding the CPU holds off
//! interrupts, so nothing else runs on it until the hold goes, and its
//! state can be changed with plain loads and stores, at the price of a few
//! instructions. The kernel defines the two functions that do it, and fat
//! LTO inlines them, as with `cpu_ref`'s count.
//!
//! In the host's builds (the unit tests and Kani's proofs) there is no CPU
//! to hold: the number is the one the hooks give, which other threads may
//! share, so state kept by it needs a lock of its own there
//! ([`HeldCpu::ALONE`] says whether it does).

use core::marker::PhantomData;

/// The most CPUs there are: a CPU's number, taken modulo this, picks its
/// state.
pub const CPUS: usize = 256;

#[cfg(target_os = "none")]
#[allow(unsafe_code)]
unsafe extern "Rust" {
    /// Holds off interrupts and gives the running CPU's number, shifted up
    /// one, with bit 0 set if interrupts were on.
    safe fn radix_hold_cpu() -> usize;
    /// Lets interrupts in again if `were` says they were on.
    safe fn radix_let_go_cpu(were: bool);
}

/// The CPU the caller runs on, held until this is dropped.
pub struct HeldCpu {
    cpu: usize,
    /// Whether interrupts were on when the hold began.
    #[cfg(target_os = "none")]
    were: bool,
    /// The hold belongs to the CPU it was taken on.
    _not_send: PhantomData<*const ()>,
}

impl HeldCpu {
    /// Whether holding the CPU keeps everything else off its state: in the
    /// kernel it does; in the host's builds threads share CPU numbers.
    pub const ALONE: bool = cfg!(target_os = "none");

    /// Holds the CPU the caller runs on.
    #[inline(always)]
    pub fn here() -> HeldCpu {
        #[cfg(target_os = "none")]
        {
            let held = radix_hold_cpu();
            HeldCpu { cpu: (held >> 1) % CPUS, were: held & 1 != 0, _not_send: PhantomData }
        }
        #[cfg(not(target_os = "none"))]
        {
            let cpu = crate::hooks::active().map_or(0, crate::hooks::Hooks::current_cpu);
            HeldCpu { cpu: cpu % CPUS, _not_send: PhantomData }
        }
    }

    /// The CPU's number, below [`CPUS`].
    #[inline(always)]
    pub fn index(&self) -> usize {
        self.cpu
    }
}

impl Drop for HeldCpu {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(target_os = "none")]
        radix_let_go_cpu(self.were);
    }
}
