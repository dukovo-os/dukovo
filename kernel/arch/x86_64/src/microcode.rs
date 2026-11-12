// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Early microcode loading on Intel processors.
//!
//! When the image includes a microcode file (`ob image --microcode`), GRUB
//! loads it as a module whose command line says `microcode`. Every CPU
//! looks for the newest update for its signature and platform before it
//! does anything else of note, and writes it to the processor if it is
//! newer than what is running. Hyper-threads share microcode, so the second
//! thread of a core finds the update already applied. The module comes
//! from the firmware set of the slot GRUB started (ADR 0468), or from the
//! image's EFI system partition (ADR 0014); what became of it is in
//! `/kernel/firmware/microcode`, which the health check reads.

use core::sync::atomic::{AtomicU32, Ordering};

use radix_microcode::{CpuId, Update};
use radix_sync::Once;

use crate::cpu::{self, msr};

/// The microcode file from the boot loader.
static FILE: Once<&'static [u8]> = Once::new();
/// APs that loaded an update.
static AP_UPDATES: AtomicU32 = AtomicU32::new(0);

/// Records the microcode module. Called once, before [`load`].
pub fn set_file(bytes: &'static [u8]) {
    let _ = FILE.set(bytes);
}

/// The running microcode revision.
pub fn revision() -> u32 {
    // SAFETY: IA32_BIOS_SIGN_ID exists on Intel CPUs, the only ones this
    // is called on. Writing 0 and running CPUID 1 makes the processor
    // report its revision in the upper half, as the SDM prescribes.
    unsafe {
        cpu::wrmsr(msr::BIOS_SIGN_ID, 0);
        let _ = cpu::cpuid(1, 0);
        (cpu::rdmsr(msr::BIOS_SIGN_ID) >> 32) as u32
    }
}

/// This CPU's identity for matching updates.
pub fn cpu_id() -> CpuId {
    let signature = cpu::cpuid(1, 0).eax;
    let (family, model, _) = cpu::family_model_stepping();
    let platform = if family > 6 || (family == 6 && model >= 5) {
        // SAFETY: IA32_PLATFORM_ID exists on Intel family 6 model 5 and
        // later; reading it has no side effects.
        1 << ((unsafe { cpu::rdmsr(msr::PLATFORM_ID) } >> 50) & 7)
    } else {
        0
    };
    CpuId { signature, platform }
}

fn apply(update: &Update<'_>) -> Result<u32, u32> {
    let data = update.data();
    if !(data.as_ptr() as usize).is_multiple_of(16) {
        return Err(revision());
    }
    // SAFETY: the update passed its checksums and matches this CPU; the
    // processor reads `data` (16-byte aligned, mapped, and never freed)
    // when IA32_BIOS_UPDT_TRIG is written with its linear address, and
    // verifies Intel's signature before applying it.
    unsafe { cpu::wrmsr(msr::BIOS_UPDT_TRIG, data.as_ptr() as u64) };
    let now = revision();
    if now == update.revision() { Ok(now) } else { Err(now) }
}

/// What became of the microcode module on the boot CPU, which
/// `/kernel/firmware/microcode` shows and the health check reads (ADR
/// 0468).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The processor runs the module's update, of this revision.
    Applied(u32),
    /// The processor runs this revision, the module's newest for it or
    /// newer.
    Current(u32),
    /// The module has no update for this processor.
    Unmatched,
    /// Not an Intel processor: the module is not read.
    Unsupported,
    /// The update did not take effect under a hypervisor, which keeps
    /// the processor's own microcode; still running this revision.
    Hypervisor(u32),
    /// The module has data that failed its checks: none of it is used.
    Damaged,
    /// The processor refused the update `wanted`, and still runs
    /// `running`.
    Refused { wanted: u32, running: u32 },
}

/// The boot CPU's [`Outcome`], once it has looked at the module.
static OUTCOME: Once<Outcome> = Once::new();

/// What became of the module on the boot CPU, if there is one.
pub fn outcome() -> Option<Outcome> {
    OUTCOME.get().copied()
}

/// Writes `/kernel/firmware/microcode`: the module's size and updates,
/// and what became of it, a `key value` line each.
pub fn report(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
    let (Some(file), Some(outcome)) = (FILE.get(), outcome()) else {
        return Err(core::fmt::Error);
    };
    let updates = radix_microcode::updates(file).flatten().count();
    writeln!(out, "size {}", file.len())?;
    writeln!(out, "updates {updates}")?;
    let (result, revision) = match outcome {
        Outcome::Applied(revision) => ("applied", Some(revision)),
        Outcome::Current(revision) => ("current", Some(revision)),
        Outcome::Unmatched => ("unmatched", None),
        Outcome::Unsupported => ("unsupported", None),
        Outcome::Hypervisor(revision) => ("hypervisor", Some(revision)),
        Outcome::Damaged => ("damaged", None),
        Outcome::Refused { running, .. } => ("refused", Some(running)),
    };
    writeln!(out, "result {result}")?;
    if let Some(revision) = revision {
        writeln!(out, "revision {revision:#x}")?;
    }
    if let Outcome::Refused { wanted, .. } = outcome {
        writeln!(out, "refused {wanted:#x}")?;
    }
    Ok(())
}

/// Loads the newest matching update on this CPU. `boot_cpu` selects
/// logging: the boot CPU checks the module whole, reports what it did
/// and keeps the [`Outcome`]; the others only load and count. A module
/// with any data that fails its checks is not used at all, and one is
/// read only on Intel processors.
pub fn load(boot_cpu: bool) {
    let Some(file) = FILE.get() else { return };
    if !boot_cpu {
        // Each CPU whose module the boot CPU found usable for it.
        let usable = matches!(
            outcome(),
            Some(
                Outcome::Applied(_)
                    | Outcome::Current(_)
                    | Outcome::Hypervisor(_)
                    | Outcome::Refused { .. }
            )
        );
        if usable {
            load_on(file, false);
        }
        return;
    }
    let (count, bad) = radix_microcode::updates(file)
        .fold((0, 0), |(ok, bad), u| if u.is_ok() { (ok + 1, bad) } else { (ok, bad + 1) });
    if bad > 0 {
        radix_klog::warn!(
            "microcode: module of {} bytes with {count} updates and data that failed its \
             checks; none of it is used",
            file.len()
        );
        let _ = OUTCOME.set(Outcome::Damaged);
        return;
    }
    if !cpu::is_intel() {
        radix_klog::warn!("microcode: only Intel processors are supported; module ignored");
        let _ = OUTCOME.set(Outcome::Unsupported);
        return;
    }
    // The platform is read only for a processor the module has updates
    // for: a hypervisor need not have its register.
    let signature = cpu::cpuid(1, 0).eax;
    let current = revision();
    let named = radix_microcode::updates(file)
        .filter_map(Result::ok)
        .any(|u| u.targets().any(|(s, _)| s == signature));
    if !named {
        radix_klog::info!(
            "microcode: module of {} bytes with {count} updates; CPU signature {signature:#x}, \
             revision {current:#x}: no update for this CPU in the module",
            file.len()
        );
        let _ = OUTCOME.set(Outcome::Unmatched);
        return;
    }
    let id = cpu_id();
    radix_klog::info!(
        "microcode: module of {} bytes with {count} updates; CPU signature {:#x}, platform \
         {:#x}, revision {current:#x}",
        file.len(),
        id.signature,
        id.platform
    );
    let _ = OUTCOME.set(load_on(file, true));
}

/// Loads the newest update of `file` for this CPU if it is newer than
/// what runs, and gives what became of it; the boot CPU (`boot_cpu`)
/// says so and keeps it, the others count what they loaded, and what
/// they are given back is not kept.
fn load_on(file: &[u8], boot_cpu: bool) -> Outcome {
    let id = cpu_id();
    let current = revision();
    let Some(update) = radix_microcode::find(file, id) else {
        if boot_cpu {
            radix_klog::info!("microcode: no update for this CPU in the module");
        }
        return Outcome::Unmatched;
    };
    if update.revision() <= current {
        if boot_cpu {
            radix_klog::info!(
                "microcode: running revision {current:#x} is current (module has {:#x})",
                update.revision()
            );
        }
        return Outcome::Current(current);
    }
    let result = apply(&update);
    if !boot_cpu {
        if result.is_ok() {
            AP_UPDATES.fetch_add(1, Ordering::Relaxed);
        }
        return Outcome::Current(current);
    }
    let (year, month, day) = update.date();
    match result {
        Ok(now) => {
            radix_klog::info!(
                "microcode: updated from revision {current:#x} to {now:#x} \
                 (released {year:04}-{month:02}-{day:02})"
            );
            Outcome::Applied(now)
        }
        Err(now) if cpu::feature::hypervisor() => {
            radix_klog::info!(
                "microcode: revision {:#x} did not take effect; still running {now:#x}, as the \
                 hypervisor keeps the processor's own",
                update.revision()
            );
            Outcome::Hypervisor(now)
        }
        Err(now) => {
            radix_klog::warn!(
                "microcode: revision {:#x} did not take effect; still running {now:#x}",
                update.revision()
            );
            Outcome::Refused { wanted: update.revision(), running: now }
        }
    }
}

/// How many other CPUs loaded the update.
pub fn ap_updates() -> u32 {
    AP_UPDATES.load(Ordering::Relaxed)
}
