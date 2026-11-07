// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the memory manager needs from the machine.

/// Most CPUs the memory manager keeps per-CPU caches for. Equal to
/// `radix_hal::MAX_CPUS`, which checks it.
pub const MAX_CPUS: usize = 256;

// A held CPU's index is its number modulo `radix_sync::cpu::CPUS`, which
// leaves the number of every CPU there can be as it is.
const _: () = assert!(radix_sync::cpu::CPUS >= MAX_CPUS);

/// The machine, as the memory manager sees it. The architecture implements
/// it and registers it with [`crate::Frames::set_platform`] and
/// [`crate::Heap::set_platform`] before the first allocation.
///
/// # Safety
///
/// The heap builds pointers from [`Platform::direct_map`], so implementors
/// guarantee that `direct_map() + phys` is a readable and writable mapping
/// of every frame the frame allocator manages, for the kernel's whole life,
/// and that [`Platform::current_cpu`] is below [`MAX_CPUS`] and does not
/// change while interrupts are disabled.
#[allow(unsafe_code)]
pub unsafe trait Platform: Sync {
    /// Index of the CPU running the caller.
    fn current_cpu(&self) -> usize;

    /// NUMA node of `cpu` (0 when the machine has one node).
    fn cpu_node(&self, cpu: usize) -> usize;

    /// Disables interrupts on this CPU and returns whether they were on.
    fn disable_interrupts(&self) -> bool;

    /// Enables interrupts again if `were_enabled`.
    fn restore_interrupts(&self, were_enabled: bool);

    /// Disables interrupts on this CPU and returns its index and whether
    /// they were on: [`Platform::disable_interrupts`] and
    /// [`Platform::current_cpu`] in one call, which the heap makes on
    /// every allocation and free. The kernel's build holds the CPU through
    /// `radix_sync::cpu::HeldCpu` instead, which must do the same.
    fn enter(&self) -> (usize, bool) {
        let were_enabled = self.disable_interrupts();
        (self.current_cpu(), were_enabled)
    }

    /// Virtual address of physical address 0 in the direct map.
    fn direct_map(&self) -> usize;
}

/// Interrupts disabled until dropped (no-op without a platform, which
/// only happens before the machine is set up, with interrupts off).
///
/// In the kernel's build, [`NoInterrupts::enter`], which the heap makes on
/// every allocation and free, holds the CPU through
/// `radix_sync::cpu::HeldCpu` instead of calling the platform's `enter`
/// and `restore_interrupts` through its table (ADR 0406): the kernel
/// defines the hold with the same code as its platform's, and fat LTO
/// inlines it.
pub(crate) struct NoInterrupts<'a> {
    platform: Option<&'a dyn Platform>,
    were_enabled: bool,
    /// [`NoInterrupts::enter`]'s hold of the CPU, in the kernel's build,
    /// which lets go as it drops.
    #[cfg(target_os = "none")]
    held: Option<radix_sync::cpu::HeldCpu>,
}

impl<'a> NoInterrupts<'a> {
    pub(crate) fn new(platform: Option<&'a dyn Platform>) -> NoInterrupts<'a> {
        let were_enabled = platform.is_some_and(Platform::disable_interrupts);
        NoInterrupts {
            platform,
            were_enabled,
            #[cfg(target_os = "none")]
            held: None,
        }
    }

    /// Interrupts disabled on this CPU, and its index ([`Platform::enter`]).
    #[inline(always)]
    pub(crate) fn enter(platform: &'a dyn Platform) -> (NoInterrupts<'a>, usize) {
        #[cfg(target_os = "none")]
        {
            let held = radix_sync::cpu::HeldCpu::here();
            let cpu = held.index();
            (NoInterrupts { platform: Some(platform), were_enabled: false, held: Some(held) }, cpu)
        }
        #[cfg(not(target_os = "none"))]
        {
            let (cpu, were_enabled) = platform.enter();
            (NoInterrupts { platform: Some(platform), were_enabled }, cpu)
        }
    }
}

impl Drop for NoInterrupts<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        // A hold lets go as it drops, after this.
        #[cfg(target_os = "none")]
        if self.held.is_some() {
            return;
        }
        if let Some(platform) = self.platform {
            platform.restore_interrupts(self.were_enabled);
        }
    }
}
