// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Early microcode loading on Intel and AMD processors.
//!
//! When the image includes a microcode file (`ob image --microcode`), GRUB
//! loads it as a module whose command line says `microcode`. The file may
//! hold Intel updates, AMD containers or both (ADR 0792), and each CPU
//! reads the ones for its own vendor. Every CPU looks for the newest update
//! for its signature (and, on Intel, its platform) before it does anything
//! else of note, and writes it to the processor if it is newer than what
//! is running. Hyper-threads share microcode, so the second thread of a
//! core finds the update already applied. The module comes from the
//! firmware set of the slot GRUB started (ADR 0468), or from the image's
//! EFI system partition (ADR 0014); what became of it is in
//! `/kernel/firmware/microcode`, which the health check reads. A boot log
//! line that starts `microcode:` says what the boot CPU found; `result
//! refused` there means the processor rejected the update. Boot test
//! `microcode` covers the matching and the checks; QEMU cannot apply an
//! update.

use core::sync::atomic::{AtomicU32, Ordering};

use radix_microcode::{CpuId, Piece, Update, amd};
use radix_sync::{Once, SpinLock};

use crate::cpu::{self, msr};

/// The microcode file from the boot loader.
static FILE: Once<&'static [u8]> = Once::new();
/// APs that loaded an update.
static AP_UPDATES: AtomicU32 = AtomicU32::new(0);

/// The patch being loaded, copied here because the module's own alignment
/// is not guaranteed and the processor wants its patch in memory the
/// kernel keeps mapped. The lock keeps two CPUs from loading together:
/// the other CPUs start one at a time (`smp::start_all` waits for each),
/// and the lock makes that a rule of this buffer rather than of the
/// caller.
static PATCH_BUFFER: SpinLock<PatchBuffer> = SpinLock::new(PatchBuffer([0; amd::PATCH_MAX]));

#[repr(align(16))]
struct PatchBuffer([u8; amd::PATCH_MAX]);

/// Family 17h's cores keep the translation of the patch buffer they last
/// used, so each page of a new patch is invalidated after the load.
const FIRST_ZEN_FAMILY: u32 = 0x17;
const PAGE: usize = 4096;

/// Whose microcode this CPU takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Vendor {
    Intel,
    Amd,
    Other,
}

fn vendor() -> Vendor {
    if cpu::is_intel() {
        Vendor::Intel
    } else if &cpu::vendor() == b"AuthenticAMD" {
        // Hygon's cores are AMD's design, but AMD's containers hold no
        // patches for their signatures.
        Vendor::Amd
    } else {
        Vendor::Other
    }
}

/// Whether this CPU's vendor loads microcode, so that [`revision`] reads
/// a register it has.
pub fn supported() -> bool {
    vendor() != Vendor::Other
}

/// Records the microcode module. Called once, before [`load`].
pub fn set_file(bytes: &'static [u8]) {
    let _ = FILE.set(bytes);
}

/// The running microcode revision, or 0 on a vendor that has none.
pub fn revision() -> u32 {
    match vendor() {
        Vendor::Intel => intel_revision(),
        Vendor::Amd => amd_revision(),
        Vendor::Other => 0,
    }
}

fn intel_revision() -> u32 {
    // SAFETY: IA32_BIOS_SIGN_ID exists on Intel CPUs, the only ones this
    // is called on. Writing 0 and running CPUID 1 makes the processor
    // report its revision in the upper half, as the SDM prescribes.
    unsafe {
        cpu::wrmsr(msr::BIOS_SIGN_ID, 0);
        let _ = cpu::cpuid(1, 0);
        (cpu::rdmsr(msr::BIOS_SIGN_ID) >> 32) as u32
    }
}

fn amd_revision() -> u32 {
    // SAFETY: reading MSR_AMD64_PATCH_LEVEL has no side effects. A
    // hypervisor that does not emulate it faults the read, which reads
    // as revision 0. The revision is the low half; AMD's needs no CPUID
    // first, which is Intel's way.
    unsafe { cpu::rdmsr_safe(msr::AMD_PATCH_LEVEL) }.map_or(0, |level| level as u32)
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

/// What a CPU found in the module for itself.
enum Candidate<'a> {
    Intel(Update<'a>),
    Amd(amd::Patch<'a>),
}

impl Candidate<'_> {
    fn revision(&self) -> u32 {
        match self {
            Candidate::Intel(update) => update.revision(),
            Candidate::Amd(patch) => patch.revision(),
        }
    }

    fn date(&self) -> (u16, u8, u8) {
        match self {
            Candidate::Intel(update) => update.date(),
            Candidate::Amd(patch) => patch.date(),
        }
    }

    /// Writes it to the processor: the revision now running if that is
    /// the update's, the one still running if not.
    fn apply(&self) -> Result<u32, u32> {
        match self {
            Candidate::Intel(update) => apply_intel(update),
            Candidate::Amd(patch) => apply_amd(patch),
        }
    }
}

/// The newest update or patch in `file` for this CPU.
fn newest(file: &[u8], vendor: Vendor) -> Option<Candidate<'_>> {
    match vendor {
        Vendor::Intel => radix_microcode::find(file, cpu_id()).map(Candidate::Intel),
        Vendor::Amd => amd::find(file, cpu::cpuid(1, 0).eax).map(Candidate::Amd),
        Vendor::Other => None,
    }
}

/// Whether the module has an update for a CPU of `signature`, whatever
/// its platform or revision.
fn names_cpu(file: &[u8], vendor: Vendor, signature: u32) -> bool {
    match vendor {
        Vendor::Intel => radix_microcode::updates(file)
            .filter_map(Result::ok)
            .any(|u| u.targets().any(|(s, _)| s == signature)),
        Vendor::Amd => amd::find(file, signature).is_some(),
        Vendor::Other => false,
    }
}

fn apply_intel(update: &Update<'_>) -> Result<u32, u32> {
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

fn apply_amd(patch: &amd::Patch<'_>) -> Result<u32, u32> {
    let bytes = patch.bytes();
    let mut buffer = PATCH_BUFFER.lock();
    // A container's patches are at most `PATCH_MAX` bytes, which its
    // parser checks.
    buffer.0[..bytes.len()].copy_from_slice(bytes);
    let start = buffer.0.as_ptr() as usize;
    // SAFETY: MSR_AMD64_PATCH_LOADER exists on AMD CPUs, the only ones
    // this is called on. The processor reads the patch at the linear
    // address written, here the header at the start of `PATCH_BUFFER`,
    // which is mapped for the life of the kernel and held by `buffer`,
    // and verifies AMD's signature before it applies the patch.
    unsafe { cpu::wrmsr(msr::AMD_PATCH_LOADER, start as u64) };
    if cpu::family_model_stepping().0 >= FIRST_ZEN_FAMILY {
        let end = start + bytes.len();
        for page in (start & !(PAGE - 1)..end).step_by(PAGE) {
            cpu::invlpg(page);
        }
    }
    let now = amd_revision();
    if now == patch.revision() { Ok(now) } else { Err(now) }
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
    /// The module has no update or patch for this processor.
    Unmatched,
    /// Neither an Intel nor an AMD processor: the module is not read.
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

/// What a module holds: the Intel updates and AMD patches that passed
/// their checks, and the pieces that did not.
struct Counts {
    intel: usize,
    amd: usize,
    damaged: usize,
}

fn count(file: &[u8]) -> Counts {
    let mut counts = Counts { intel: 0, amd: 0, damaged: 0 };
    for piece in radix_microcode::pieces(file) {
        match piece {
            Ok(Piece::Intel(_)) => counts.intel += 1,
            Ok(Piece::Amd(container)) => counts.amd += container.patches().count(),
            Err(_) => counts.damaged += 1,
        }
    }
    counts
}

/// Writes `/kernel/firmware/microcode`: the module's size, the vendor
/// whose microcode this CPU reads from it, how many of that vendor's
/// updates it holds, and what became of it, a `key value` line each.
pub fn report(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
    let (Some(file), Some(outcome)) = (FILE.get(), outcome()) else {
        return Err(core::fmt::Error);
    };
    let counts = count(file);
    let (vendor, updates) = match vendor() {
        Vendor::Intel => ("intel", counts.intel),
        Vendor::Amd => ("amd", counts.amd),
        Vendor::Other => ("other", 0),
    };
    writeln!(out, "size {}", file.len())?;
    writeln!(out, "vendor {vendor}")?;
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
/// read only on Intel and AMD processors.
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
    let counts = count(file);
    if counts.damaged > 0 {
        radix_klog::warn!(
            "microcode: module of {} bytes with {} Intel updates, {} AMD patches and data that \
             failed its checks; none of it is used",
            file.len(),
            counts.intel,
            counts.amd
        );
        let _ = OUTCOME.set(Outcome::Damaged);
        return;
    }
    let vendor = vendor();
    if vendor == Vendor::Other {
        radix_klog::warn!("microcode: only Intel and AMD processors are supported; module ignored");
        let _ = OUTCOME.set(Outcome::Unsupported);
        return;
    }
    // Intel's platform is read only for a processor the module has
    // updates for: a hypervisor need not have its register.
    let signature = cpu::cpuid(1, 0).eax;
    let current = revision();
    if !names_cpu(file, vendor, signature) {
        radix_klog::info!(
            "microcode: module of {} bytes with {} Intel updates and {} AMD patches; CPU \
             signature {signature:#x}, revision {current:#x}: no update for this CPU in the \
             module",
            file.len(),
            counts.intel,
            counts.amd
        );
        let _ = OUTCOME.set(Outcome::Unmatched);
        return;
    }
    if vendor == Vendor::Intel {
        radix_klog::info!(
            "microcode: module of {} bytes with {} Intel updates and {} AMD patches; CPU \
             signature {signature:#x}, platform {:#x}, revision {current:#x}",
            file.len(),
            counts.intel,
            counts.amd,
            cpu_id().platform
        );
    } else {
        radix_klog::info!(
            "microcode: module of {} bytes with {} Intel updates and {} AMD patches; CPU \
             signature {signature:#x}, revision {current:#x}",
            file.len(),
            counts.intel,
            counts.amd
        );
    }
    let _ = OUTCOME.set(load_on(file, true));
}

/// Loads the newest update of `file` for this CPU if it is newer than
/// what runs, and gives what became of it; the boot CPU (`boot_cpu`)
/// says so and keeps it, the others count what they loaded, and what
/// they are given back is not kept.
fn load_on(file: &[u8], boot_cpu: bool) -> Outcome {
    let current = revision();
    let Some(update) = newest(file, vendor()) else {
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
    let result = update.apply();
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
