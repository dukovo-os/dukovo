// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How each flaw is closed ([`Plan::make`]) and what its report line says
//! ([`Plan::write_line`]).
//!
//! The defaults follow what Linux does by default, so that the two can be
//! compared like for like (ADR 0039's bench gate), but where the kernel
//! can do better at no cost worth counting: VERW also on processors that
//! only have MMIO stale data, IBRS around firmware calls, TSX off wherever
//! it can be turned off. A flaw that only a guest can use is reported as
//! closed while the kernel runs no guests ([`Built::guests`]).

use core::fmt::{self, Write};

use crate::{Cpu, Flaw, Found, Vendor, bits, caps, mcu_opt};

/// IA32_SPEC_CTRL (MSR 48h): indirect branches use no target trained at a
/// lower privilege (IBRS).
pub const SPEC_CTRL_IBRS: u64 = 1 << 0;
/// IA32_SPEC_CTRL: indirect branches use no target trained by the other
/// hardware thread (STIBP).
pub const SPEC_CTRL_STIBP: u64 = 1 << 1;
/// IA32_SPEC_CTRL: loads do not run ahead of older stores whose address is
/// not known yet (SSBD).
pub const SPEC_CTRL_SSBD: u64 = 1 << 2;
/// IA32_SPEC_CTRL: returns in the kernel are not predicted from the branch
/// target buffer when the return stack runs dry (RRSBA_DIS_S).
pub const SPEC_CTRL_RRSBA_DIS_S: u64 = 1 << 6;
/// IA32_SPEC_CTRL: indirect branches in the kernel ignore the branch
/// history (BHI_DIS_S).
pub const SPEC_CTRL_BHI_DIS_S: u64 = 1 << 10;

/// What a command-line flag asks of one protection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    /// What suits the processor: the default.
    #[default]
    Auto,
    /// No protection (for `tsx`, TSX turned off).
    Off,
    /// The protection, even where it costs more than it is worth by
    /// default (for `tsx`, TSX left on).
    On,
    /// IBRS for the kernel, written on every entry.
    Ibrs,
    /// IBPB on every entry to the kernel.
    Ibpb,
    /// Retpolines for the kernel's indirect branches, even where the
    /// processor's own protection would do.
    Retpoline,
    /// A return thunk for the kernel's returns, even where
    /// `mitigations=off` would leave them plain.
    Thunk,
}

/// Whether hardware threads are turned off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Nosmt {
    /// Never.
    #[default]
    No,
    /// Where a flaw reaches from one hardware thread to the other.
    IfNeeded,
    /// Always.
    Always,
}

/// The command-line flags [`Choice::set`] reads, by Linux's names, each
/// with the values it takes; a flag with none is bare.
pub const FLAGS: &[(&str, &[&str])] = &[
    ("mitigations", &["auto", "off", "auto,nosmt"]),
    ("nosmt", &[]),
    ("nospectre_v1", &[]),
    ("spectre_v2", &["auto", "off", "on", "ibrs", "retpoline", "retpoline,generic"]),
    ("nospectre_v2", &[]),
    ("spectre_v2_user", &["auto", "off", "on"]),
    ("spec_store_bypass_disable", &["auto", "off", "on"]),
    ("nospec_store_bypass_disable", &[]),
    ("retbleed", &["auto", "off", "ibpb", "unret"]),
    ("spec_rstack_overflow", &["auto", "off", "ibpb", "safe-ret"]),
    ("tsx", &["auto", "off", "on"]),
    ("mds", &["auto", "off"]),
    ("tsx_async_abort", &["auto", "off"]),
    ("mmio_stale_data", &["auto", "off"]),
    ("reg_file_data_sampling", &["auto", "off"]),
    ("tsa", &["auto", "off"]),
    ("srbds", &["auto", "off"]),
    ("gather_data_sampling", &["auto", "off"]),
    ("indirect_target_selection", &["on", "off"]),
];

/// A flag of [`FLAGS`] was given a value it does not take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadValue;

/// What the command line asks.
#[derive(Clone, Copy, Debug, Default)]
pub struct Choice {
    all_off: bool,
    nosmt: Nosmt,
    spectre_v1: Pick,
    spectre_v2: Pick,
    spectre_v2_user: Pick,
    spec_store_bypass: Pick,
    retbleed: Pick,
    spec_rstack_overflow: Pick,
    tsx: Pick,
    mds: Pick,
    tsx_async_abort: Pick,
    mmio_stale_data: Pick,
    reg_file_data_sampling: Pick,
    tsa: Pick,
    srbds: Pick,
    gather_data_sampling: Pick,
    indirect_target_selection: Pick,
}

impl Choice {
    /// Reads one command-line flag: `Ok(true)` if it is one of [`FLAGS`],
    /// `Ok(false)` if it is not. A later flag wins over an earlier one.
    /// `mitigations=off` turns off every protection that its own flag does
    /// not ask for by `on`, `ibrs`, `ibpb`, `retpoline`, `unret` or
    /// `safe-ret`.
    pub fn set(&mut self, name: &str, value: Option<&str>) -> Result<bool, BadValue> {
        let Some(&(_, values)) = FLAGS.iter().find(|&&(flag, _)| flag == name) else {
            return Ok(false);
        };
        let known = match value {
            None => values.is_empty(),
            Some(value) => values.contains(&value),
        };
        if !known {
            return Err(BadValue);
        }
        let pick = match value {
            Some("off") => Pick::Off,
            Some("on") => Pick::On,
            Some("ibrs") => Pick::Ibrs,
            Some("ibpb") => Pick::Ibpb,
            Some("retpoline" | "retpoline,generic") => Pick::Retpoline,
            Some("unret" | "safe-ret") => Pick::Thunk,
            _ => Pick::Auto,
        };
        match name {
            "mitigations" => {
                self.all_off = pick == Pick::Off;
                if value == Some("auto,nosmt") {
                    self.nosmt = self.nosmt.max(Nosmt::IfNeeded);
                }
            }
            "nosmt" => self.nosmt = Nosmt::Always,
            "nospectre_v1" => self.spectre_v1 = Pick::Off,
            "spectre_v2" => self.spectre_v2 = pick,
            "nospectre_v2" => self.spectre_v2 = Pick::Off,
            "spectre_v2_user" => self.spectre_v2_user = pick,
            "spec_store_bypass_disable" => self.spec_store_bypass = pick,
            "nospec_store_bypass_disable" => self.spec_store_bypass = Pick::Off,
            "retbleed" => self.retbleed = pick,
            "spec_rstack_overflow" => self.spec_rstack_overflow = pick,
            "tsx" => self.tsx = pick,
            "mds" => self.mds = pick,
            "tsx_async_abort" => self.tsx_async_abort = pick,
            "mmio_stale_data" => self.mmio_stale_data = pick,
            "reg_file_data_sampling" => self.reg_file_data_sampling = pick,
            "tsa" => self.tsa = pick,
            "srbds" => self.srbds = pick,
            "gather_data_sampling" => self.gather_data_sampling = pick,
            _ => self.indirect_target_selection = pick,
        }
        Ok(true)
    }

    /// What is asked of a protection, once `mitigations=off` is counted.
    fn get(&self, pick: Pick) -> Pick {
        match pick {
            Pick::Auto if self.all_off => Pick::Off,
            pick => pick,
        }
    }

    /// Whether a protection is wanted at all.
    fn wants(&self, pick: Pick) -> bool {
        self.get(pick) != Pick::Off
    }
}

/// What the kernel was built with, or does, that changes how flaws are
/// closed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Built {
    /// Its indirect calls and jumps go through retpoline thunks.
    pub retpoline: bool,
    /// Its returns go through a return thunk.
    pub return_thunks: bool,
    /// It unmaps itself from programs' page tables while they run (PTI).
    pub pti: bool,
    /// It runs guests.
    pub guests: bool,
    /// It lets programs set their own FS and GS bases (CR4.FSGSBASE), so
    /// a program can point GS at the kernel.
    pub fsgsbase: bool,
}

impl Built {
    /// None of it: a kernel without retpolines, return thunks or page-table
    /// isolation, that runs no guests.
    pub const DEFAULT: Built = Built {
        retpoline: false,
        return_thunks: false,
        pti: false,
        guests: false,
        fsgsbase: false,
    };
}

/// How the kernel keeps programs from steering its indirect branches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectreV2 {
    /// Not at all.
    Off,
    /// IBRS set on every entry and cleared on every exit.
    Ibrs,
    /// Enhanced IBRS (Intel) or automatic IBRS (AMD), set once.
    Eibrs,
    /// Retpoline thunks.
    Retpoline,
}

/// How Branch History Injection is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bhi {
    NotAffected,
    Off,
    /// IA32_SPEC_CTRL's BHI_DIS_S.
    Hardware,
    /// The history-clearing loop on every system call.
    Loop,
    /// Retpolines, which have no history to use, on a processor without
    /// RRSBA.
    Retpoline,
}

/// How a flaw of returns (Retbleed, SRSO) is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Returns {
    NotAffected,
    Off,
    /// IBRS on every entry.
    Ibrs,
    /// Enhanced IBRS.
    Eibrs,
    /// IBPB on every entry.
    Ibpb,
    /// IBPB on every entry wanted, but the microcode's IBPB does not flush
    /// return predictions.
    IbpbNoMicrocode,
    /// The return thunk.
    Thunk,
    /// Hardware threads off, where the flaw needs them.
    NoSmt,
    /// Only guests could use the flaw, and none run.
    NoGuests,
}

/// How STIBP is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stibp {
    /// Not at all, or not needed (enhanced IBRS covers it on Intel).
    Off,
    /// Always, as the processor prefers or another protection needs.
    AlwaysOn,
    /// Always, as the command line asks.
    Forced,
}

/// How the kernel's indirect calls and jumps run, each rewritten at boot
/// from the call or jump to the retpoline that it was compiled to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndirectBranches {
    /// As plain indirect branches.
    #[default]
    Plain,
    /// Through the retpoline.
    Retpoline,
    /// As plain indirect branches, each ending in the upper half of its
    /// cache line, which Indirect Target Selection does not reach, or
    /// through a thunk whose branch does.
    Aligned,
}

/// How the kernel's returns run, each rewritten at boot from the jump to
/// the return thunk that it was compiled to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReturnPath {
    /// As plain returns.
    #[default]
    Plain,
    /// As plain returns in the upper half of their cache lines, or through
    /// a thunk whose return is there (ITS).
    Aligned,
    /// Through one return whose prediction is untrained on every entry
    /// from a program (Retbleed on AMD's Zen 1 and 2).
    Untrained,
    /// Through a call to a return that drops the call's own return
    /// address, so that a return is only ever predicted to a trap, with
    /// that return untrained on every entry (SRSO on Zen 1 and 2; it
    /// closes Retbleed too).
    SafeRet,
    /// The same, with the untraining and the return at two addresses the
    /// branch predictor of Zen 3 and 4 takes for one.
    SafeRetAliased,
}

impl ReturnPath {
    /// Whether every entry from a program untrains the return.
    pub fn untrains(self) -> bool {
        matches!(self, ReturnPath::Untrained | ReturnPath::SafeRet | ReturnPath::SafeRetAliased)
    }
}

/// What the architecture sets on every CPU and does at every entry to and
/// exit from the kernel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    /// The processor has IA32_SPEC_CTRL: write it on every CPU.
    pub spec_ctrl: bool,
    /// IA32_SPEC_CTRL while the kernel runs.
    pub spec_ctrl_kernel: u64,
    /// IA32_SPEC_CTRL while a program runs; where it differs from
    /// [`Controls::spec_ctrl_kernel`], it is written on every entry and
    /// exit.
    pub spec_ctrl_user: u64,
    /// Set EFER.AIBRSE (AMD's automatic IBRS).
    pub auto_ibrs: bool,
    /// Set IBRS around calls into the firmware.
    pub ibrs_firmware: bool,
    /// IBPB around calls into the firmware.
    pub ibpb_firmware: bool,
    /// IBPB when a CPU switches to another program's address space.
    pub ibpb_on_switch: bool,
    /// IBPB on every entry to the kernel.
    pub ibpb_on_entry: bool,
    /// Fill the return stack on every switch between threads.
    pub rsb_fill_on_switch: bool,
    /// Run the branch-history clearing loop on every system call.
    pub bhb_clear: bool,
    /// LFENCE after deciding whether to SWAPGS on an interrupt, on both
    /// sides of the decision.
    pub swapgs_fence: bool,
    /// VERW, clearing the CPU's buffers, on every exit to a program.
    pub verw_on_exit: bool,
    /// VERW before a CPU idles while its other hardware thread runs.
    pub verw_on_idle: bool,
    /// Divide on every exit to a program, so that no quotient is left for
    /// it to read (AMD Zen 1).
    pub div0_clear: bool,
    /// Turn TSX off through IA32_TSX_CTRL.
    pub tsx_disable: bool,
    /// Set DE_CFG's Zenbleed chicken bit (AMD Zen 2).
    pub zenbleed_chicken: bool,
    /// IA32_MCU_OPT_CTRL bits to set on every CPU.
    pub mcu_opt_set: u64,
    /// IA32_MCU_OPT_CTRL bits to clear on every CPU.
    pub mcu_opt_clear: u64,
    /// Start no second hardware thread of any core.
    pub nosmt: bool,
    /// How the kernel's indirect branches run.
    pub indirect_branches: IndirectBranches,
    /// How the kernel's returns run.
    pub returns: ReturnPath,
}

/// The flaws a processor has and how each is closed.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    cpu: Cpu,
    found: Found,
    built: Built,
    controls: Controls,
    smt_active: bool,
    spectre_v1: bool,
    spectre_v2: SpectreV2,
    stibp: Stibp,
    bhi: Bhi,
    retbleed: Returns,
    srso: Returns,
    ssb: bool,
    rfds: bool,
    tsa: bool,
    srbds: bool,
    gds: bool,
    its: bool,
}

impl Plan {
    /// Decides how to close each flaw `cpu` has, with what the kernel was
    /// `built` with and what the command line asks (`choice`).
    pub fn make(cpu: &Cpu, choice: &Choice, built: Built) -> Plan {
        let found = crate::find(cpu);
        let amd = matches!(cpu.vendor, Vendor::Amd | Vendor::Hygon);
        let v2_wanted = found.has(Flaw::SpectreV2) && choice.wants(choice.spectre_v2);
        let spectre_v2 = spectre_v2(cpu, &found, choice, built);
        let user = match choice.get(choice.spectre_v2_user) {
            Pick::Auto if choice.get(choice.spectre_v2) == Pick::On => Pick::On,
            _ if !v2_wanted => Pick::Off,
            pick => pick,
        };

        // IBPB on entry closes both flaws of returns on AMD, whichever
        // asked for it.
        let ibpb_flushes_returns = cpu.ext21_eax & bits::EXT21_IBPB_BRTYPE != 0
            || (amd && cpu.family == 0x17)
            || cpu.vendor == Vendor::Hygon;
        let srso_user_kernel_no = cpu.ext21_eax & bits::EXT21_SRSO_USER_KERNEL_NO != 0;
        let retbleed_ibpb = amd
            && found.has(Flaw::Retbleed)
            && choice.get(choice.retbleed) == Pick::Ibpb
            && cpu.ibpb();
        let srso_wants_ibpb = found.has(Flaw::SpecRstackOverflow)
            && choice.get(choice.spec_rstack_overflow) == Pick::Ibpb
            && cpu.ibpb()
            && (built.guests || !srso_user_kernel_no);
        // An IBPB that leaves return predictions in place is no use
        // against SRSO, so it is not paid for.
        let srso_ibpb = srso_wants_ibpb && ibpb_flushes_returns;
        let ibpb_on_entry = retbleed_ibpb || srso_ibpb;

        let retbleed = if !found.has(Flaw::Retbleed) {
            Returns::NotAffected
        } else if amd && ibpb_on_entry {
            Returns::Ibpb
        } else if !choice.wants(choice.retbleed) {
            Returns::Off
        } else if amd {
            if built.return_thunks { Returns::Thunk } else { Returns::Off }
        } else {
            match spectre_v2 {
                SpectreV2::Eibrs => Returns::Eibrs,
                SpectreV2::Ibrs => Returns::Ibrs,
                SpectreV2::Off | SpectreV2::Retpoline => Returns::Off,
            }
        };

        // STIBP as it would be with both hardware threads running.
        let intel_eibrs = spectre_v2 == SpectreV2::Eibrs && cpu.caps(caps::IBRS_ALL);
        let stibp_with_smt = if !cpu.stibp() || intel_eibrs || user == Pick::Off {
            Stibp::Off
        } else if user == Pick::On {
            Stibp::Forced
        } else if cpu.ext8_ebx & bits::EXT8_STIBP_ALWAYS_ON != 0
            || (amd && matches!(retbleed, Returns::Ibpb | Returns::Thunk))
        {
            Stibp::AlwaysOn
        } else {
            Stibp::Off
        };

        let tsx_disable = cpu.caps(caps::TSX_CTRL) && choice.tsx != Pick::On;
        let nosmt = cpu.smt
            && match choice.nosmt {
                Nosmt::No => false,
                Nosmt::Always => true,
                Nosmt::IfNeeded => {
                    (found.has(Flaw::Mds) && !found.msbds_only)
                        || (found.has(Flaw::TsxAsyncAbort) && !tsx_disable)
                        || found.has(Flaw::MmioStaleData)
                        || found.has(Flaw::L1tf)
                        || found.has(Flaw::Tsa)
                        || (found.smt_rsb && stibp_with_smt == Stibp::Off)
                }
            };
        let smt_active = cpu.smt && !nosmt;
        let stibp = if smt_active { stibp_with_smt } else { Stibp::Off };

        let srso = if !found.has(Flaw::SpecRstackOverflow) {
            Returns::NotAffected
        } else if srso_user_kernel_no && !built.guests {
            Returns::NoGuests
        } else if ibpb_on_entry && ibpb_flushes_returns {
            Returns::Ibpb
        } else if srso_wants_ibpb {
            Returns::IbpbNoMicrocode
        } else if !choice.wants(choice.spec_rstack_overflow) {
            Returns::Off
        } else if cpu.family < 0x19 && !smt_active {
            Returns::NoSmt
        } else if built.return_thunks {
            Returns::Thunk
        } else {
            Returns::Off
        };

        let bhi = if !found.bhi {
            Bhi::NotAffected
        } else if !v2_wanted {
            Bhi::Off
        } else if spectre_v2 == SpectreV2::Retpoline && !cpu.caps(caps::RRSBA) {
            Bhi::Retpoline
        } else if cpu.leaf7_2_edx & bits::LEAF7_2_BHI_CTRL != 0 {
            Bhi::Hardware
        } else {
            Bhi::Loop
        };

        let ssb = found.has(Flaw::SpecStoreBypass)
            && choice.get(choice.spec_store_bypass) == Pick::On
            && cpu.ssbd();

        let mut base = 0;
        if intel_eibrs {
            base |= SPEC_CTRL_IBRS;
            if cpu.caps(caps::RRSBA) && cpu.leaf7_2_edx & bits::LEAF7_2_RRSBA_CTRL != 0 {
                base |= SPEC_CTRL_RRSBA_DIS_S;
            }
        }
        if stibp != Stibp::Off {
            base |= SPEC_CTRL_STIBP;
        }
        if ssb {
            base |= SPEC_CTRL_SSBD;
        }
        if bhi == Bhi::Hardware {
            base |= SPEC_CTRL_BHI_DIS_S;
        }
        let kernel = if spectre_v2 == SpectreV2::Ibrs { base | SPEC_CTRL_IBRS } else { base };

        let ibpb_firmware =
            v2_wanted && amd && found.has(Flaw::Retbleed) && cpu.ibpb() && !ibpb_on_entry;
        let ibrs_firmware = v2_wanted
            && !ibpb_firmware
            && cpu.ibrs()
            && matches!(spectre_v2, SpectreV2::Off | SpectreV2::Retpoline);

        // One VERW clears the buffers of every flaw below, so it runs if
        // any of them asks for it.
        let mds = found.has(Flaw::Mds) && choice.wants(choice.mds);
        let taa =
            found.has(Flaw::TsxAsyncAbort) && !tsx_disable && choice.wants(choice.tsx_async_abort);
        let mmio = found.has(Flaw::MmioStaleData) && choice.wants(choice.mmio_stale_data);
        let rfds = found.has(Flaw::RegFileDataSampling)
            && choice.wants(choice.reg_file_data_sampling)
            && cpu.caps(caps::RFDS_CLEAR);
        let tsa = found.has(Flaw::Tsa)
            && choice.wants(choice.tsa)
            && cpu.ext21_eax & bits::EXT21_VERW_CLEAR != 0;
        let verw_on_exit = mds || taa || mmio || rfds || tsa;
        let verw_on_idle =
            smt_active && ((mds && found.msbds_only) || (mmio && !found.has(Flaw::Mds)) || tsa);

        // An interrupt may speculatively take the wrong side of its SWAPGS
        // decision and run with the program's GS. Where programs can only
        // point GS at their own memory and SMAP stops speculative reads of
        // it (it does not on processors with Meltdown), that is harmless.
        let spectre_v1 = found.has(Flaw::SpectreV1) && choice.wants(choice.spectre_v1);
        let smap = cpu.leaf7_ebx & bits::LEAF7_SMAP != 0;
        let swapgs_fence = spectre_v1 && (built.fsgsbase || !smap || found.has(Flaw::Meltdown));

        // Indirect Target Selection reaches the indirect branches and
        // returns that end in the lower half of a cache line, where the
        // processor's own protection of indirect branches would be used:
        // with retpolines the kernel has no indirect branches left, but
        // its returns still count.
        let its = found.has(Flaw::IndirectTargetSelection)
            && choice.wants(choice.indirect_target_selection)
            && spectre_v2 != SpectreV2::Off
            && built.retpoline
            && built.return_thunks;
        let indirect_branches = match spectre_v2 {
            SpectreV2::Retpoline => IndirectBranches::Retpoline,
            _ if its => IndirectBranches::Aligned,
            _ => IndirectBranches::Plain,
        };
        // The safe return of SRSO's closing covers Retbleed too.
        let returns = if srso == Returns::Thunk {
            if cpu.family >= 0x19 { ReturnPath::SafeRetAliased } else { ReturnPath::SafeRet }
        } else if retbleed == Returns::Thunk {
            ReturnPath::Untrained
        } else if its {
            ReturnPath::Aligned
        } else {
            ReturnPath::Plain
        };

        let srbds = choice.wants(choice.srbds);
        let gds = choice.wants(choice.gather_data_sampling);
        let (mcu_opt_set, mcu_opt_clear) = mcu_opt_bits(cpu, &found, tsx_disable, srbds, gds);

        let controls = Controls {
            spec_ctrl: cpu.ibrs() || cpu.stibp() || cpu.ssbd(),
            spec_ctrl_kernel: kernel,
            spec_ctrl_user: base,
            auto_ibrs: spectre_v2 == SpectreV2::Eibrs && !intel_eibrs,
            ibrs_firmware,
            ibpb_firmware,
            ibpb_on_switch: user == Pick::On && cpu.ibpb(),
            ibpb_on_entry,
            rsb_fill_on_switch: v2_wanted,
            bhb_clear: bhi == Bhi::Loop,
            swapgs_fence,
            verw_on_exit,
            verw_on_idle,
            div0_clear: found.div0,
            tsx_disable,
            zenbleed_chicken: found.zenbleed,
            mcu_opt_set,
            mcu_opt_clear,
            nosmt,
            indirect_branches,
            returns,
        };
        Plan {
            cpu: *cpu,
            found,
            built,
            controls,
            smt_active,
            spectre_v1,
            spectre_v2,
            stibp,
            bhi,
            retbleed,
            srso,
            ssb,
            rfds,
            tsa,
            srbds,
            gds,
            its,
        }
    }

    /// What the architecture applies.
    pub fn controls(&self) -> &Controls {
        &self.controls
    }

    /// The flaws the processor has.
    pub fn found(&self) -> &Found {
        &self.found
    }

    /// The most pages a swap area may span. A page table's entry keeps a
    /// swapped-out page's slot inverted, above the lower half of the
    /// physical addresses, so that L1TF reads ahead from no memory through
    /// it; a page past the limit would name the lower half.
    pub fn swap_pages(&self) -> u64 {
        if self.found.has(Flaw::L1tf) { 1 << (self.cpu.cache_bits() - 1 - 12) } else { u64::MAX }
    }

    /// Whether the page tables' entries that are not present, which hold
    /// their frames inverted, name no memory for L1TF to read: whether all
    /// of RAM lies in the lower half of the physical addresses.
    fn memory_guarded(&self) -> bool {
        self.cpu.memory_end <= 1 << (self.cpu.cache_bits() - 1)
    }

    /// How the kernel's indirect branches are protected.
    pub fn spectre_v2(&self) -> SpectreV2 {
        self.spectre_v2
    }

    /// Whether the report says `flaw` is open.
    pub fn is_vulnerable(&self, flaw: Flaw) -> bool {
        let mut start = Start::default();
        // Writing into `Start` cannot fail.
        let _ = self.write_line(flaw, &mut start);
        start.is(b"Vulnerable")
    }

    /// Writes the line `/kernel/devices/system/cpu/vulnerabilities/<flaw>`
    /// shows, in Linux's words, without its newline.
    pub fn write_line(&self, flaw: Flaw, out: &mut dyn Write) -> fmt::Result {
        if !self.found.has(flaw) {
            return out.write_str("Not affected");
        }
        match flaw {
            Flaw::Meltdown => {
                out.write_str(if self.built.pti { "Mitigation: PTI" } else { "Vulnerable" })
            }
            Flaw::SpectreV1 => out.write_str(if self.spectre_v1 {
                "Mitigation: usercopy/swapgs barriers and __user pointer sanitization"
            } else {
                "Vulnerable: __user pointer sanitization and usercopy barriers only; no swapgs \
                 barriers"
            }),
            Flaw::SpectreV2 => self.write_spectre_v2(out),
            Flaw::SpecStoreBypass => out.write_str(if self.ssb {
                "Mitigation: Speculative Store Bypass disabled"
            } else {
                "Vulnerable"
            }),
            Flaw::L1tf => out.write_str(if self.memory_guarded() {
                "Mitigation: PTE Inversion"
            } else {
                "Vulnerable"
            }),
            Flaw::Mds => self.write_mds(out),
            Flaw::TsxAsyncAbort => self.write_taa(out),
            Flaw::ItlbMultihit => out.write_str(if self.built.guests {
                "Vulnerable"
            } else {
                "Mitigation: VMX disabled"
            }),
            Flaw::Srbds => out.write_str(self.srbds_line()),
            Flaw::MmioStaleData => self.write_mmio(out),
            Flaw::Retbleed => self.write_retbleed(out),
            Flaw::SpecRstackOverflow => out.write_str(match self.srso {
                Returns::Ibpb => "Mitigation: IBPB",
                Returns::IbpbNoMicrocode => "Vulnerable: No microcode",
                Returns::Thunk => "Mitigation: Safe RET",
                Returns::NoSmt => "Mitigation: SMT disabled",
                Returns::NoGuests => "Mitigation: no guests run",
                Returns::NotAffected => "Not affected",
                Returns::Off | Returns::Ibrs | Returns::Eibrs => "Vulnerable",
            }),
            Flaw::GatherDataSampling => out.write_str(self.gds_line()),
            Flaw::RegFileDataSampling => out.write_str(if self.rfds {
                "Mitigation: Clear Register File"
            } else if !self.cpu.caps(caps::RFDS_CLEAR) {
                "Vulnerable: No microcode"
            } else {
                "Vulnerable"
            }),
            Flaw::IndirectTargetSelection => out.write_str(if self.its {
                "Mitigation: Aligned branch/return thunks"
            } else {
                "Vulnerable"
            }),
            Flaw::Tsa => out.write_str(if self.tsa {
                "Mitigation: Clear CPU buffers: user/kernel boundary"
            } else if self.cpu.ext21_eax & bits::EXT21_VERW_CLEAR == 0 {
                "Vulnerable: No microcode"
            } else {
                "Vulnerable"
            }),
            Flaw::Vmscape => out.write_str(if self.built.guests {
                "Vulnerable"
            } else {
                "Mitigation: no guests run"
            }),
        }
    }

    fn write_spectre_v2(&self, out: &mut dyn Write) -> fmt::Result {
        out.write_str(match self.spectre_v2 {
            SpectreV2::Off => "Vulnerable",
            SpectreV2::Ibrs => "Mitigation: IBRS",
            SpectreV2::Eibrs => "Mitigation: Enhanced / Automatic IBRS",
            SpectreV2::Retpoline => "Mitigation: Retpolines",
        })?;
        if self.cpu.ibpb() {
            out.write_str(if self.controls.ibpb_on_switch {
                "; IBPB: always-on"
            } else {
                "; IBPB: disabled"
            })?;
        }
        if self.controls.ibrs_firmware {
            out.write_str("; IBRS_FW")?;
        }
        if !(self.spectre_v2 == SpectreV2::Eibrs && self.cpu.caps(caps::IBRS_ALL)) {
            out.write_str(match self.stibp {
                Stibp::Off => "; STIBP: disabled",
                Stibp::AlwaysOn => "; STIBP: always-on",
                Stibp::Forced => "; STIBP: forced",
            })?;
        }
        if self.controls.rsb_fill_on_switch {
            out.write_str("; RSB filling")?;
        }
        out.write_str(if !self.found.pbrsb {
            "; PBRSB-eIBRS: Not affected"
        } else if self.built.guests {
            "; PBRSB-eIBRS: Vulnerable"
        } else {
            "; PBRSB-eIBRS: no guests run"
        })?;
        out.write_str(match self.bhi {
            Bhi::NotAffected => "; BHI: Not affected",
            Bhi::Off => "; BHI: Vulnerable",
            Bhi::Hardware => "; BHI: BHI_DIS_S",
            Bhi::Loop => "; BHI: SW loop",
            Bhi::Retpoline => "; BHI: Retpoline",
        })
    }

    /// The `; SMT ...` ending of the buffer-clearing flaws' lines.
    fn write_smt(&self, out: &mut dyn Write, mitigated: bool) -> fmt::Result {
        if self.cpu.hypervisor() {
            out.write_str("; SMT Host state unknown")
        } else if !self.smt_active {
            out.write_str("; SMT disabled")
        } else if mitigated {
            out.write_str("; SMT mitigated")
        } else {
            out.write_str("; SMT vulnerable")
        }
    }

    fn write_mds(&self, out: &mut dyn Write) -> fmt::Result {
        let verw = self.controls.verw_on_exit;
        out.write_str(if !verw {
            "Vulnerable"
        } else if self.cpu.leaf7_edx & bits::LEAF7_MD_CLEAR != 0 {
            "Mitigation: Clear CPU buffers"
        } else {
            "Vulnerable: Clear CPU buffers attempted, no microcode"
        })?;
        // A store buffer that the hardware threads share only while one
        // idles is closed by VERW before idling.
        self.write_smt(out, verw && self.found.msbds_only)
    }

    fn write_taa(&self, out: &mut dyn Write) -> fmt::Result {
        if self.controls.tsx_disable {
            return out.write_str("Mitigation: TSX disabled");
        }
        if !self.controls.verw_on_exit {
            return out.write_str("Vulnerable");
        }
        // Without MDS, TAA's buffer clearing came with the microcode that
        // added IA32_TSX_CTRL.
        let microcode = self.cpu.leaf7_edx & bits::LEAF7_MD_CLEAR != 0
            && (!self.cpu.caps(caps::MDS_NO) || self.cpu.caps(caps::TSX_CTRL));
        out.write_str(if microcode {
            "Mitigation: Clear CPU buffers"
        } else {
            "Vulnerable: Clear CPU buffers attempted, no microcode"
        })?;
        self.write_smt(out, false)
    }

    fn write_mmio(&self, out: &mut dyn Write) -> fmt::Result {
        if !self.controls.verw_on_exit {
            return out.write_str("Vulnerable");
        }
        // The fill buffers' clearing is FB_CLEAR, or on processors with
        // MDS, MD_CLEAR with L1D_FLUSH.
        let microcode = self.cpu.caps(caps::FB_CLEAR)
            || (self.cpu.leaf7_edx & bits::LEAF7_MD_CLEAR != 0
                && self.cpu.leaf7_edx & bits::LEAF7_L1D_FLUSH != 0
                && !self.cpu.caps(caps::MDS_NO));
        out.write_str(if microcode {
            "Mitigation: Clear CPU buffers"
        } else {
            "Vulnerable: Clear CPU buffers attempted, no microcode"
        })?;
        self.write_smt(out, false)
    }

    fn write_retbleed(&self, out: &mut dyn Write) -> fmt::Result {
        out.write_str(match self.retbleed {
            Returns::Ibrs => "Mitigation: IBRS",
            Returns::Eibrs => "Mitigation: Enhanced IBRS",
            Returns::Ibpb => "Mitigation: IBPB",
            Returns::Thunk => "Mitigation: untrained return thunk",
            _ => "Vulnerable",
        })?;
        if !matches!(self.retbleed, Returns::Ibpb | Returns::Thunk) {
            return Ok(());
        }
        out.write_str(if !self.smt_active {
            "; SMT disabled"
        } else if self.stibp != Stibp::Off {
            "; SMT enabled with STIBP protection"
        } else {
            "; SMT vulnerable"
        })
    }

    fn srbds_line(&self) -> &'static str {
        let cpu = &self.cpu;
        if cpu.caps(caps::MDS_NO)
            && (!cpu.rtm() || self.controls.tsx_disable)
            && !self.found.has(Flaw::MmioStaleData)
        {
            "Mitigation: TSX disabled"
        } else if cpu.hypervisor() {
            "Unknown: Dependent on hypervisor status"
        } else if cpu.leaf7_edx & bits::LEAF7_SRBDS_CTRL == 0 {
            "Vulnerable: No microcode"
        } else if self.srbds {
            "Mitigation: Microcode"
        } else {
            "Vulnerable"
        }
    }

    fn gds_line(&self) -> &'static str {
        let cpu = &self.cpu;
        if cpu.hypervisor() {
            return "Unknown: Dependent on hypervisor status";
        }
        if !cpu.caps(caps::GDS_CTRL) {
            return "Vulnerable: No microcode";
        }
        let now = cpu.mcu_opt_ctrl.unwrap_or(0);
        if now & mcu_opt::GDS_MITG_LOCKED != 0 {
            return if now & mcu_opt::GDS_MITG_DIS != 0 {
                "Vulnerable"
            } else {
                "Mitigation: Microcode (locked)"
            };
        }
        if self.gds { "Mitigation: Microcode" } else { "Vulnerable" }
    }
}

/// Picks how the kernel protects its indirect branches.
fn spectre_v2(cpu: &Cpu, found: &Found, choice: &Choice, built: Built) -> SpectreV2 {
    if !found.has(Flaw::SpectreV2) {
        return SpectreV2::Off;
    }
    let pick = choice.get(choice.spectre_v2);
    if pick == Pick::Off {
        return SpectreV2::Off;
    }
    if pick == Pick::Ibrs && cpu.ibrs() {
        return SpectreV2::Ibrs;
    }
    if pick == Pick::Retpoline && built.retpoline {
        return SpectreV2::Retpoline;
    }
    if cpu.eibrs() {
        return SpectreV2::Eibrs;
    }
    // Retpolines leave Intel's returns open where they fall back to the
    // branch target buffer (Retbleed); IBRS closes both. AMD's IBRS costs
    // more than retpolines and does not close Retbleed.
    let amd = matches!(cpu.vendor, Vendor::Amd | Vendor::Hygon);
    if !amd && cpu.ibrs() && (found.has(Flaw::Retbleed) || !built.retpoline) {
        return SpectreV2::Ibrs;
    }
    if built.retpoline {
        return SpectreV2::Retpoline;
    }
    if pick == Pick::On && cpu.ibrs() {
        return SpectreV2::Ibrs;
    }
    SpectreV2::Off
}

/// The IA32_MCU_OPT_CTRL bits to set and to clear: the microcode's SRBDS
/// and GDS protections on where wanted and off where not (or, for SRBDS,
/// where TSX being off already closes it), but never a locked bit.
fn mcu_opt_bits(cpu: &Cpu, found: &Found, tsx_disable: bool, srbds: bool, gds: bool) -> (u64, u64) {
    let (mut set, mut clear) = (0, 0);
    if found.has(Flaw::Srbds) && cpu.leaf7_edx & bits::LEAF7_SRBDS_CTRL != 0 && !cpu.hypervisor() {
        let tsx_closes_it = cpu.caps(caps::MDS_NO)
            && (!cpu.rtm() || tsx_disable)
            && !found.has(Flaw::MmioStaleData);
        if srbds && !tsx_closes_it {
            clear |= mcu_opt::RNGDS_MITG_DIS;
        } else {
            set |= mcu_opt::RNGDS_MITG_DIS;
        }
    }
    let locked = cpu.mcu_opt_ctrl.unwrap_or(0) & mcu_opt::GDS_MITG_LOCKED != 0;
    if found.has(Flaw::GatherDataSampling)
        && cpu.caps(caps::GDS_CTRL)
        && !cpu.hypervisor()
        && !locked
    {
        if gds {
            clear |= mcu_opt::GDS_MITG_DIS;
        } else {
            set |= mcu_opt::GDS_MITG_DIS;
        }
    }
    (set, clear)
}

/// The first bytes of a line, enough to tell how it starts.
#[derive(Default)]
struct Start {
    bytes: [u8; 16],
    len: usize,
}

impl Start {
    fn is(&self, prefix: &[u8]) -> bool {
        self.bytes[..self.len].starts_with(prefix)
    }
}

impl Write for Start {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for &byte in text.as_bytes() {
            if self.len == self.bytes.len() {
                break;
            }
            self.bytes[self.len] = byte;
            self.len += 1;
        }
        Ok(())
    }
}
