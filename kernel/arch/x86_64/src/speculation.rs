// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Closing the processor's speculative-execution flaws (ADRs 0790, 0791).
//!
//! The boot CPU reads what the processor says of itself, after its early
//! microcode, and `radix_cpuflaws` decides how to close each flaw ([`init`]).
//! The code that closes them on the way into and out of the kernel sits at
//! *sites* in the entry and exit paths (`user.rs`, `interrupts.rs`,
//! `thread.rs`): each site is a block that a jump skips until the boot CPU
//! turns the jump into a no-op, before any other CPU starts and before any
//! program runs. A processor that needs none of a site's work pays one
//! taken jump for it, and no site hangs on a conditional branch that a
//! program could train the CPU to mispredict. Every CPU then sets its
//! control registers ([`init_cpu`]): IA32_SPEC_CTRL, AMD's automatic IBRS,
//! TSX, Zen 2's Zenbleed bit, the microcode's SRBDS and GDS controls.
//!
//! What the kernel does about each flaw is in
//! `/kernel/devices/system/cpu/vulnerabilities`, in Linux's words
//! ([`write_report`]).

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use radix_cpuflaws::{
    Built, Choice, Controls, Cpu, Flaw, IndirectBranches, Plan, ReturnPath, Vendor, bits, caps,
    mcu_opt,
};
use radix_hal::MAX_CPUS;
use radix_hal::cmdline::CommandLine;
use radix_sync::Once;

use crate::{cpu, microcode, percpu, smp, thunks};

/// IA32_SPEC_CTRL.
const SPEC_CTRL: u32 = 0x48;
/// IA32_PRED_CMD, whose bit 0 is IBPB.
const PRED_CMD: u32 = 0x49;
/// IA32_ARCH_CAPABILITIES.
const ARCH_CAPABILITIES: u32 = 0x10a;
/// IA32_TSX_CTRL: RTM_DISABLE (bit 0) and TSX_CPUID_CLEAR (bit 1).
const TSX_CTRL: u32 = 0x122;
/// IA32_MCU_OPT_CTRL.
const MCU_OPT_CTRL: u32 = 0x123;
/// AMD's DE_CFG, whose bit 9 keeps Zen 2 from leaking vector registers
/// (Zenbleed).
const DE_CFG: u32 = 0xc001_1029;
/// EFER's automatic IBRS enable (AMD).
const EFER_AUTOIBRS: u64 = 1 << 21;
/// CR0's write protect bit.
const CR0_WP: u64 = 1 << 16;

/// IA32_SPEC_CTRL while the kernel runs; the entry sites write it.
pub static SPEC_CTRL_KERNEL: AtomicU32 = AtomicU32::new(0);
/// IA32_SPEC_CTRL while a program runs; the exit sites write it.
pub static SPEC_CTRL_USER: AtomicU32 = AtomicU32::new(0);
/// The selector VERW is given: any valid one does, and the kernel's data
/// segment is always valid.
pub static VERW_SELECTOR: u16 = percpu::KERNEL_DS;

/// VERW before idling, while the other hardware thread runs.
static VERW_ON_IDLE: AtomicBool = AtomicBool::new(false);
/// What [`firmware_enter`] does.
static FIRMWARE_IBRS: AtomicBool = AtomicBool::new(false);
static FIRMWARE_IBPB: AtomicBool = AtomicBool::new(false);
/// IBPB when a CPU moves to another program's address space.
static IBPB_ON_SWITCH: AtomicBool = AtomicBool::new(false);
/// Whether the processor has IA32_SPEC_CTRL.
static HAS_SPEC_CTRL: AtomicBool = AtomicBool::new(false);
/// The last program address space each CPU ran, for [`switched_space`].
static LAST_SPACE: [AtomicUsize; MAX_CPUS] = [const { AtomicUsize::new(0) }; MAX_CPUS];
/// No address space is ever at this address.
const NO_SPACE: usize = usize::MAX;

/// The decisions, made once by the boot CPU.
static PLAN: Once<Plan> = Once::new();

/// What the kernel is built with: its indirect branches and returns go
/// through thunks that the boot CPU rewrites (`thunks.rs`).
const BUILT: Built = Built { retpoline: true, return_thunks: true, ..Built::DEFAULT };

/// What a site turns on. The numbers are the ones the sites' records
/// carry (`.spec_sites`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Site {
    /// IA32_SPEC_CTRL's kernel value, on entry from a program.
    SpecCtrlEntry = 1,
    /// IA32_SPEC_CTRL's program value, on exit to one.
    SpecCtrlExit = 2,
    /// IA32_SPEC_CTRL saved, set to the kernel's and restored around an
    /// NMI or machine check that arrives in the kernel, which may be while
    /// it holds a program's value.
    SpecCtrlParanoid = 3,
    /// IBPB, then a filled return stack, on entry from a program.
    IbpbEntry = 4,
    /// The branch history cleared, on a system call.
    BhbClear = 5,
    /// VERW, on exit to a program.
    VerwExit = 6,
    /// A division, on exit to a program (AMD Zen 1).
    Div0Exit = 7,
    /// LFENCE after an interrupt's SWAPGS decision.
    SwapgsFence = 8,
    /// A filled return stack, on every switch between threads.
    RsbFillSwitch = 9,
    /// The return's prediction untrained, on entry from a program
    /// (Retbleed and SRSO on AMD, `thunks.rs`).
    UntrainEntry = 10,
}

impl Site {
    fn from_number(number: u8) -> Option<Site> {
        Some(match number {
            1 => Site::SpecCtrlEntry,
            2 => Site::SpecCtrlExit,
            3 => Site::SpecCtrlParanoid,
            4 => Site::IbpbEntry,
            5 => Site::BhbClear,
            6 => Site::VerwExit,
            7 => Site::Div0Exit,
            8 => Site::SwapgsFence,
            9 => Site::RsbFillSwitch,
            10 => Site::UntrainEntry,
            _ => return None,
        })
    }

    /// Whether `controls` turn the site on.
    fn on(self, controls: &Controls) -> bool {
        let switch_spec_ctrl =
            controls.spec_ctrl && controls.spec_ctrl_kernel != controls.spec_ctrl_user;
        match self {
            Site::SpecCtrlEntry | Site::SpecCtrlExit | Site::SpecCtrlParanoid => switch_spec_ctrl,
            Site::IbpbEntry => controls.ibpb_on_entry,
            Site::BhbClear => controls.bhb_clear,
            Site::VerwExit => controls.verw_on_exit,
            Site::Div0Exit => controls.div0_clear,
            Site::SwapgsFence => controls.swapgs_fence,
            Site::RsbFillSwitch => controls.rsb_fill_on_switch,
            Site::UntrainEntry => controls.returns.untrains(),
        }
    }
}

/// A site's assembly: `code` (Intel syntax), behind a jump that skips it
/// until [`init`] turns the site on, and a record of it in `.spec_sites`:
/// where the jump is, how long it is, and the [`Site`] it is, named by
/// the asm operand `$site`. The labels 660 to 662 are the macro's.
macro_rules! site {
    ($site:literal, $($code:expr),+ $(,)?) => {
        concat!(
            "660:\n",
            "jmp 662f\n",
            "661:\n",
            $($code, "\n",)+
            "662:\n",
            ".pushsection .spec_sites, \"a\"\n",
            ".balign 4\n",
            ".long 660b - .\n",
            ".byte 661b - 660b\n",
            ".byte {", $site, "}\n",
            ".short 0\n",
            ".popsection\n",
        )
    };
}
pub(crate) use site;

/// Writes the IA32_SPEC_CTRL value at the asm operand `$value` (whose
/// bits are all in the low half). Uses EAX, ECX and EDX.
macro_rules! write_spec_ctrl {
    ($value:literal) => {
        concat!(
            "mov ecx, 0x48\n",
            "mov eax, dword ptr [rip + {",
            $value,
            "}]\n",
            "xor edx, edx\n",
            "wrmsr"
        )
    };
}
pub(crate) use write_spec_ctrl;

/// Overwrites the return stack with 32 entries that lead nowhere, so that
/// no return predicts a target trained elsewhere: 16 rounds of two calls,
/// each to the instruction after a speculation trap, dropping the return
/// addresses as it goes. Uses EAX. The labels 663 to 665 are its own.
macro_rules! fill_rsb {
    () => {
        concat!(
            "mov eax, 16\n",
            "663:\n",
            "call 664f\n",
            "int3\n",
            "664:\n",
            "call 665f\n",
            "int3\n",
            "665:\n",
            "add rsp, 16\n",
            "dec eax\n",
            "jnz 663b\n",
            // The loop's last branch must not be mispredicted into a
            // speculative return.
            "lfence"
        )
    };
}
pub(crate) use fill_rsb;

/// The sites on every entry from a program, after its registers are
/// saved and before the first return: IA32_SPEC_CTRL's kernel value, then
/// IBPB, then the untraining of the return thunk's prediction. Uses RAX,
/// ECX and EDX, and asm operands `site_spec_ctrl_entry`,
/// `site_ibpb_entry`, `site_untrain_entry` and `spec_ctrl_kernel`.
macro_rules! entry_sites {
    () => {
        concat!(
            $crate::speculation::site!(
                "site_spec_ctrl_entry",
                $crate::speculation::write_spec_ctrl!("spec_ctrl_kernel"),
            ),
            $crate::speculation::site!(
                "site_ibpb_entry",
                "mov ecx, 0x49",
                "mov eax, 1",
                "xor edx, edx",
                "wrmsr",
                // IBPB leaves the return stack's predictions on some
                // processors.
                $crate::speculation::fill_rsb!(),
            ),
            $crate::speculation::site!("site_untrain_entry", "call radix_return_untrain"),
        )
    };
}
pub(crate) use entry_sites;

/// The sites on every exit to a program, before its registers are
/// restored: IA32_SPEC_CTRL's program value, then the division that
/// leaves no quotient behind. Uses EAX, ECX and EDX, and asm operands
/// `site_spec_ctrl_exit`, `site_div0_exit` and `spec_ctrl_user`.
macro_rules! exit_sites {
    () => {
        concat!(
            $crate::speculation::site!(
                "site_spec_ctrl_exit",
                $crate::speculation::write_spec_ctrl!("spec_ctrl_user"),
            ),
            $crate::speculation::site!(
                "site_div0_exit",
                "xor eax, eax",
                "xor edx, edx",
                "mov ecx, 1",
                "div ecx",
            ),
        )
    };
}
pub(crate) use exit_sites;

/// The last site before the instruction that returns to a program: VERW,
/// clearing the CPU's buffers. Changes only the flags. Uses asm operands
/// `site_verw_exit` and `verw_selector`.
macro_rules! verw_site {
    () => {
        $crate::speculation::site!("site_verw_exit", "verw word ptr [rip + {verw_selector}]")
    };
}
pub(crate) use verw_site;

// The branch history clearing sequence for processors with enhanced IBRS
// but no BHI_DIS_S, from Intel's guidance on Branch History Injection:
// five rounds, each a call and five taken jumps, every branch at its own
// place in the predictor, then the calls returned from in turn. It uses
// EAX, ECX and the flags. Its branches start cache lines, as the guidance
// asks. Its own calls and returns are the sequence, so they stay as they
// are; only its return to its caller goes through the return thunk.
core::arch::global_asm!(
    crate::thunks::asm_begin!(),
    r#"
    .section .text
    .balign 64
    .global radix_clear_bhb
radix_clear_bhb:
    push rbp
    mov rbp, rsp
    mov ecx, 5
    call 1f
    jmp 5f
    .balign 64, 0xcc
1:
    call 2f
    ret
    .balign 64, 0xcc
2:
    mov eax, 5
3:
    jmp 4f
    nop
4:
    sub eax, 1
    jnz 3b
    sub ecx, 1
    jnz 1b
    ret
5:
    lfence
    pop rbp
    radix_ret
"#,
    crate::thunks::asm_end!(),
);

/// One site's record, as [`site!`] lays it out.
#[repr(C)]
struct Record {
    /// From the record to the site's jump.
    offset: i32,
    /// The jump's length: 2 or 5 bytes.
    jump: u8,
    /// The [`Site`] number.
    site: u8,
    _reserved: u16,
}

unsafe extern "C" {
    static __spec_sites_start: Record;
    static __spec_sites_end: Record;
}

/// The sites' records.
fn records() -> &'static [Record] {
    let start = &raw const __spec_sites_start;
    let end = &raw const __spec_sites_end;
    let count = (end as usize - start as usize) / size_of::<Record>();
    // SAFETY: the linker script puts every record, and nothing else,
    // between the two symbols, in read-only data that lives as long as
    // the kernel; a record is 8 bytes, 4-aligned, as `site!` lays it out.
    unsafe { core::slice::from_raw_parts(start, count) }
}

/// Rewrites the kernel's indirect branches and returns for `controls`
/// (`thunks.rs`), and turns on the sites they pick: each one's jump
/// becomes a no-op of the same length. Run by the boot CPU before any
/// other CPU starts and before any program runs, with interrupts masked,
/// so no CPU fetches a site as it changes; an NMI on this CPU sees each
/// site whole, as each is written by one store. Returns how many sites
/// there are and how many it turned on, and the thunk sites it found.
fn patch_sites(controls: &Controls) -> (usize, usize, thunks::Counts) {
    let were_enabled = cpu::disable_interrupts();
    let cr0 = cpu::cr0();
    // SAFETY: with CR0.WP clear the kernel may write its read-only text,
    // which it does only below, to the sites' jumps; interrupts are masked
    // and no other CPU runs, so nothing else runs meanwhile.
    unsafe { cpu::write_cr0(cr0 & !CR0_WP) };
    let thunk_sites = thunks::patch(controls);
    let (mut total, mut turned_on) = (0, 0);
    for record in records() {
        total += 1;
        let site = Site::from_number(record.site)
            .unwrap_or_else(|| panic!("speculation: unknown site {}", record.site));
        let at = (&raw const record.offset as usize).wrapping_add_signed(record.offset as isize);
        // Every record is checked, so that a broken one fails every boot,
        // not only on the processors that turn its site on.
        let (opcode, nop) = jump_and_nop(at, record.jump);
        // SAFETY: `at` is a site's jump in the kernel's text, which stays
        // mapped.
        let first = unsafe { core::ptr::read(at as *const u8) };
        assert_eq!(first, opcode, "speculation: no jump at the site at {at:#x}");
        if site.on(controls) {
            patch_jump(at, nop);
            turned_on += 1;
        }
    }
    // Writing CR0 serializes, so this CPU runs the changed code from here.
    // SAFETY: CR0 as it was.
    unsafe { cpu::write_cr0(cr0) };
    if were_enabled {
        cpu::enable_interrupts();
    }
    (total, turned_on, thunk_sites)
}

/// The first byte of a `length`-byte jump, and the no-op of that length.
fn jump_and_nop(at: usize, length: u8) -> (u8, &'static [u8]) {
    match length {
        // `jmp rel8`, and `xchg ax, ax`.
        2 => (0xeb, &[0x66, 0x90]),
        // `jmp rel32`, and `nop dword ptr [rax + rax]`.
        5 => (0xe9, &[0x0f, 0x1f, 0x44, 0x00, 0x00]),
        _ => panic!("speculation: a site at {at:#x} has a {length}-byte jump"),
    }
}

/// Writes `nop` over the jump at `at`.
fn patch_jump(at: usize, nop: &[u8]) {
    // SAFETY: `at` is a site's jump in the kernel's text, which stays
    // mapped; the eight bytes from it are text too, as a site's code
    // follows its jump.
    let old = unsafe { core::ptr::read_unaligned(at as *const u64) };
    let mut new = old.to_le_bytes();
    new[..nop.len()].copy_from_slice(nop);
    let new = u64::from_le_bytes(new);
    // SAFETY: as for the read; CR0.WP is clear (`patch_sites`), and one
    // eight-byte store changes the jump whole, the bytes after it as they
    // were.
    unsafe {
        core::arch::asm!(
            "mov qword ptr [{at}], {new}",
            at = in(reg) at,
            new = in(reg) new,
            options(nostack, preserves_flags),
        );
    };
}

/// What the boot CPU says of itself, with `smt` from the CPU table.
fn read_cpu(smt: bool) -> Cpu {
    let vendor = match &cpu::vendor() {
        b"GenuineIntel" => Vendor::Intel,
        b"AuthenticAMD" => Vendor::Amd,
        b"HygonGenuine" => Vendor::Hygon,
        _ => Vendor::Other,
    };
    let (family, model, stepping) = cpu::family_model_stepping();
    let leaf1 = cpu::cpuid(1, 0);
    let leaf7 = (cpu::max_leaf() >= 7).then(|| cpu::cpuid(7, 0));
    let leaf7_2 = leaf7.filter(|leaf| leaf.eax >= 2).map_or(0, |_| cpu::cpuid(7, 2).edx);
    let (leaf7_ebx, leaf7_edx) = leaf7.map_or((0, 0), |leaf| (leaf.ebx, leaf.edx));
    let extended = cpu::max_extended_leaf();
    let ext8 = (extended >= 0x8000_0008).then(|| cpu::cpuid(0x8000_0008, 0));
    let ext21 = (extended >= 0x8000_0021).then(|| cpu::cpuid(0x8000_0021, 0));
    let arch_capabilities = if leaf7_edx & bits::LEAF7_ARCH_CAPABILITIES != 0 {
        // SAFETY: CPUID says the register exists; reading it changes
        // nothing.
        unsafe { cpu::rdmsr_safe(ARCH_CAPABILITIES) }.unwrap_or(0)
    } else {
        0
    };
    let has_mcu_opt =
        leaf7_edx & bits::LEAF7_SRBDS_CTRL != 0 || arch_capabilities & caps::GDS_CTRL != 0;
    // SAFETY: CPUID or IA32_ARCH_CAPABILITIES says the register exists;
    // reading it changes nothing.
    let mcu_opt_ctrl = has_mcu_opt.then(|| unsafe { cpu::rdmsr_safe(MCU_OPT_CTRL) }).flatten();
    Cpu {
        vendor,
        family,
        model,
        stepping,
        microcode: if microcode::supported() { microcode::revision() } else { 0 },
        leaf1_ecx: leaf1.ecx,
        leaf7_ebx,
        leaf7_edx,
        leaf7_2_edx: leaf7_2,
        ext8_ebx: ext8.map_or(0, |leaf| leaf.ebx),
        phys_bits: ext8.map_or(0, |leaf| leaf.eax & 0xff),
        memory_end: crate::memory::ram_end(),
        ext21_eax: ext21.map_or(0, |leaf| leaf.eax),
        ext21_ecx: ext21.map_or(0, |leaf| leaf.ecx),
        arch_capabilities,
        mcu_opt_ctrl,
        smt,
    }
}

/// Whether two listed CPUs are hardware threads of one core.
fn siblings_listed() -> bool {
    let count = smp::count();
    let places = || (0..count).filter_map(smp::place);
    places().enumerate().any(|(index, place)| {
        places()
            .skip(index + 1)
            .any(|other| (other.package, other.core) == (place.package, place.core))
    })
}

/// Decides how to close the processor's flaws and turns on the sites
/// that do it, from what the boot CPU says of itself and the command line.
/// Called once by the boot CPU, after its early microcode and the CPU
/// table, before the other CPUs start and before any program runs; then
/// sets this CPU's controls ([`init_cpu`]).
pub fn init(flags: &CommandLine<'_>) {
    let smt = siblings_listed() && !flags.has("nosmp");
    let cpu = read_cpu(smt);
    let mut choice = Choice::default();
    for (name, value) in flags.flags() {
        if choice.set(name, value).is_err() {
            radix_klog::warn!(
                "CPU flaws: {name}={} is not a value the flag takes; ignored",
                value.unwrap_or("")
            );
        }
    }
    let mut plan = Plan::make(&cpu, &choice, BUILT);
    if plan.controls().spec_ctrl && !spec_ctrl_takes(plan.controls()) {
        // A hypervisor that names the controls and refuses their register
        // leaves the processor as one without them, which the report then
        // says too.
        radix_klog::warn!("CPU flaws: IA32_SPEC_CTRL refused the values CPUID names; not used");
        plan = Plan::make(&without_spec_ctrl(cpu), &choice, BUILT);
    }
    let plan = PLAN.call_once(|| plan);
    let controls = plan.controls();
    SPEC_CTRL_KERNEL.store(controls.spec_ctrl_kernel as u32, Ordering::Relaxed);
    SPEC_CTRL_USER.store(controls.spec_ctrl_user as u32, Ordering::Relaxed);
    HAS_SPEC_CTRL.store(controls.spec_ctrl, Ordering::Relaxed);
    VERW_ON_IDLE.store(controls.verw_on_idle, Ordering::Relaxed);
    FIRMWARE_IBRS.store(controls.ibrs_firmware, Ordering::Relaxed);
    FIRMWARE_IBPB.store(controls.ibpb_firmware, Ordering::Relaxed);
    IBPB_ON_SWITCH.store(controls.ibpb_on_switch, Ordering::Relaxed);
    let (sites, turned_on, thunk_sites) = patch_sites(controls);
    init_cpu();
    log(plan, sites, turned_on, thunk_sites);
}

/// Whether IA32_SPEC_CTRL takes both values the plan writes to it, the
/// program's and the kernel's, which is left in it.
fn spec_ctrl_takes(controls: &Controls) -> bool {
    // SAFETY: CPUID says the register exists, and each value is made of
    // the bits CPUID names; one that faults changes nothing.
    unsafe {
        cpu::wrmsr_safe(SPEC_CTRL, controls.spec_ctrl_user)
            && cpu::wrmsr_safe(SPEC_CTRL, controls.spec_ctrl_kernel)
    }
}

/// `cpu` as if it had none of the controls of IA32_SPEC_CTRL, enhanced
/// IBRS among them. Intel's one bit for IBRS and IBPB goes too, so IBPB
/// goes with them there.
fn without_spec_ctrl(cpu: Cpu) -> Cpu {
    Cpu {
        leaf7_edx: cpu.leaf7_edx & !(bits::LEAF7_IBRS_IBPB | bits::LEAF7_STIBP | bits::LEAF7_SSBD),
        ext8_ebx: cpu.ext8_ebx & !(bits::EXT8_IBRS | bits::EXT8_STIBP | bits::EXT8_SSBD),
        arch_capabilities: cpu.arch_capabilities & !caps::IBRS_ALL,
        ..cpu
    }
}

/// Sets this CPU's controls as the boot CPU decided. Every CPU runs it,
/// the boot CPU from [`init`] and the others as they start.
pub fn init_cpu() {
    let Some(plan) = PLAN.get() else { return };
    let controls = plan.controls();
    if controls.spec_ctrl {
        // SAFETY: CPUID says the processor has IA32_SPEC_CTRL, and every
        // value the plan gives is one of its bits CPUID names.
        unsafe { cpu::wrmsr(SPEC_CTRL, controls.spec_ctrl_kernel) };
    }
    if controls.auto_ibrs {
        // SAFETY: CPUID says the processor has automatic IBRS; it only
        // restricts indirect branch prediction in the kernel.
        unsafe {
            let efer = cpu::rdmsr(cpu::msr::EFER);
            cpu::wrmsr(cpu::msr::EFER, efer | EFER_AUTOIBRS);
        }
    }
    if controls.tsx_disable {
        // SAFETY: IA32_ARCH_CAPABILITIES says IA32_TSX_CTRL exists; turning
        // TSX off makes transactions abort, which the kernel never starts.
        // A hypervisor that refuses the register leaves TSX as it was.
        unsafe {
            if let Some(tsx) = cpu::rdmsr_safe(TSX_CTRL) {
                cpu::wrmsr_safe(TSX_CTRL, tsx | 0b11);
            }
        }
    }
    if controls.zenbleed_chicken {
        // SAFETY: Zen 2 has DE_CFG; bit 9 only stops a mispredicted
        // VZEROUPPER from undoing a register's state.
        unsafe {
            if let Some(de_cfg) = cpu::rdmsr_safe(DE_CFG) {
                cpu::wrmsr_safe(DE_CFG, de_cfg | 1 << 9);
            }
        }
    }
    if controls.mcu_opt_set | controls.mcu_opt_clear != 0 {
        set_mcu_opt(controls);
    }
}

/// Applies the plan's IA32_MCU_OPT_CTRL bits on this CPU, leaving GDS's
/// alone where this CPU's firmware locked them.
fn set_mcu_opt(controls: &Controls) {
    // SAFETY: the plan sets bits only where CPUID or IA32_ARCH_CAPABILITIES
    // says the register exists; reading it changes nothing.
    let Some(now) = (unsafe { cpu::rdmsr_safe(MCU_OPT_CTRL) }) else { return };
    let gds = mcu_opt::GDS_MITG_DIS;
    let keep = if now & mcu_opt::GDS_MITG_LOCKED != 0 { gds } else { 0 };
    let new = (((now | controls.mcu_opt_set) & !controls.mcu_opt_clear) & !keep) | (now & keep);
    if new != now {
        // SAFETY: the register exists (read above); its SRBDS and GDS bits
        // only turn the microcode's own protections on or off.
        unsafe { cpu::wrmsr(MCU_OPT_CTRL, new) };
    }
}

fn log(plan: &Plan, sites: usize, turned_on: usize, thunk_sites: thunks::Counts) {
    let mut open = radix_klog::TextBuf::<160>::new();
    for flaw in Flaw::ALL {
        if plan.is_vulnerable(flaw) {
            use fmt::Write;
            let _ =
                write!(open, "{}{}", if open.as_str().is_empty() { "" } else { ", " }, flaw.name());
        }
    }
    let found = Flaw::ALL.iter().filter(|&&flaw| plan.found().has(flaw)).count();
    radix_klog::info!(
        "CPU flaws: {found} of {} apply; {turned_on} of {sites} entry and exit sites on{}{}",
        Flaw::ALL.len(),
        if open.as_str().is_empty() { "; none left open" } else { "; open: " },
        open.as_str(),
    );
    let controls = plan.controls();
    let indirect = match controls.indirect_branches {
        IndirectBranches::Plain => "plain",
        IndirectBranches::Retpoline => "through the retpoline",
        IndirectBranches::Aligned => "aligned against ITS",
    };
    let returns = match controls.returns {
        ReturnPath::Plain => "plain",
        ReturnPath::Aligned => "aligned against ITS",
        ReturnPath::Untrained => "through the untrained return",
        ReturnPath::SafeRet => "through the safe return",
        ReturnPath::SafeRetAliased => "through the safe return at aliased addresses",
    };
    radix_klog::info!(
        "CPU flaws: {} indirect branches {indirect}, {} returns {returns}{}",
        thunk_sites.indirect,
        thunk_sites.returns,
        if controls.returns.untrains() { ", untrained on entry" } else { "" },
    );
    let mut what = radix_klog::TextBuf::<200>::new();
    describe(controls, &mut what);
    if !what.as_str().is_empty() {
        radix_klog::info!("CPU flaws: {}", what.as_str());
    }
}

/// Names what `controls` do, for the boot log.
fn describe(controls: &Controls, out: &mut dyn fmt::Write) {
    let spec_ctrl = |bit: u64| controls.spec_ctrl_user & bit != 0;
    let items: [(bool, &str); 16] = [
        (controls.spec_ctrl_kernel != controls.spec_ctrl_user, "IBRS on entry"),
        (spec_ctrl(radix_cpuflaws::SPEC_CTRL_IBRS), "enhanced IBRS"),
        (controls.auto_ibrs, "automatic IBRS"),
        (spec_ctrl(radix_cpuflaws::SPEC_CTRL_STIBP), "STIBP"),
        (spec_ctrl(radix_cpuflaws::SPEC_CTRL_SSBD), "SSBD"),
        (spec_ctrl(radix_cpuflaws::SPEC_CTRL_BHI_DIS_S), "BHI_DIS_S"),
        (controls.bhb_clear, "branch history cleared on system calls"),
        (controls.ibpb_on_entry, "IBPB on entry"),
        (controls.ibpb_on_switch, "IBPB between programs"),
        (controls.rsb_fill_on_switch, "return stack filled on switches"),
        (controls.verw_on_exit, "CPU buffers cleared on exit"),
        (controls.swapgs_fence, "SWAPGS fenced"),
        (controls.tsx_disable, "TSX off"),
        (controls.zenbleed_chicken, "Zenbleed bit set"),
        (controls.div0_clear, "divider cleared on exit"),
        (controls.nosmt, "second hardware threads left off"),
    ];
    let mut first = true;
    for (on, name) in items {
        if on {
            let _ = out.write_str(if first { "" } else { ", " });
            let _ = out.write_str(name);
            first = false;
        }
    }
}

/// VERW before this CPU idles, where the other hardware thread could read
/// what the kernel left in buffers they share.
pub fn before_idle() {
    if VERW_ON_IDLE.load(Ordering::Relaxed) {
        verw();
    }
}

fn verw() {
    // SAFETY: VERW only reads the selector and sets ZF; with MD_CLEAR's
    // microcode it also clears the CPU's buffers.
    unsafe {
        core::arch::asm!("verw word ptr [{sel}]", sel = in(reg) &raw const VERW_SELECTOR, options(nostack));
    };
}

/// Restricts indirect branch prediction before a call into the firmware,
/// whose code was not built with the kernel's protections: IBRS set for
/// the call, or IBPB before it.
pub fn firmware_enter() {
    if FIRMWARE_IBRS.load(Ordering::Relaxed) {
        let kernel = u64::from(SPEC_CTRL_KERNEL.load(Ordering::Relaxed));
        // SAFETY: the plan asks for this only on a processor with IBRS.
        unsafe { cpu::wrmsr(SPEC_CTRL, kernel | radix_cpuflaws::SPEC_CTRL_IBRS) };
    }
    if FIRMWARE_IBPB.load(Ordering::Relaxed) {
        ibpb();
    }
}

/// Undoes [`firmware_enter`] once the firmware returns.
pub fn firmware_exit() {
    if FIRMWARE_IBRS.load(Ordering::Relaxed) {
        // SAFETY: as in `firmware_enter`.
        unsafe { cpu::wrmsr(SPEC_CTRL, u64::from(SPEC_CTRL_KERNEL.load(Ordering::Relaxed))) };
    }
}

fn ibpb() {
    // SAFETY: the plan asks for IBPB only on a processor with it; it only
    // discards branch predictions.
    unsafe { cpu::wrmsr(PRED_CMD, 1) };
}

/// Records that CPU `index` switched to the program address space at
/// `space`, with IBPB first if it last ran another program's and the plan
/// asks for that (`spectre_v2_user=on`).
pub fn switched_space(index: usize, space: usize) {
    if !IBPB_ON_SWITCH.load(Ordering::Relaxed) {
        return;
    }
    let last = LAST_SPACE[index].swap(space, Ordering::Relaxed);
    if last != 0 && last != space {
        ibpb();
    }
}

/// Forgets the address space at `space`, which is going away, so that a
/// new one at its address still counts as another program's.
pub fn forget_space(space: usize) {
    if !IBPB_ON_SWITCH.load(Ordering::Relaxed) {
        return;
    }
    for last in &LAST_SPACE {
        let _ = last.compare_exchange(space, NO_SPACE, Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// The most pages a swap area may span ([`Plan::swap_pages`]).
pub fn swap_pages() -> u64 {
    PLAN.get().map_or(u64::MAX, Plan::swap_pages)
}

/// Whether CPU `index` stays off: a second hardware thread of its core,
/// where the plan turns those off (`nosmt`).
pub fn leave_off(index: usize) -> bool {
    PLAN.get().is_some_and(|plan| plan.controls().nosmt)
        && smp::place(index).is_some_and(|place| place.thread != 0)
}

/// The names of the flaws reported, in the order `ls` lists their files.
pub const NAMES: [&str; Flaw::ALL.len()] = {
    let mut names = [""; Flaw::ALL.len()];
    let mut index = 0;
    while index < names.len() {
        names[index] = Flaw::ALL[index].name();
        index += 1;
    }
    names
};

/// Writes how the kernel closes flaw `name`, as
/// `/kernel/devices/system/cpu/vulnerabilities/<name>` shows it, with its
/// newline; fails for a name not in [`NAMES`] or before [`init`].
pub fn write_report(name: &str, out: &mut dyn fmt::Write) -> fmt::Result {
    let (Some(plan), Some(flaw)) = (PLAN.get(), Flaw::named(name)) else {
        return Err(fmt::Error);
    };
    plan.write_line(flaw, out)?;
    out.write_char('\n')
}
