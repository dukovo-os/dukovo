// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The flaws of x86-64 processors that let one program read what it should
//! not, through speculative execution and the buffers it fills (ADR 0790),
//! and how the kernel closes each (ADR 0791).
//!
//! [`find`] works out which flaws the running processor has from what it
//! says of itself, its CPUID bits and IA32_ARCH_CAPABILITIES ([`Cpu`]),
//! and, where it says nothing, from the vendors' published lists of
//! affected models ([`models`]). [`Plan::make`] then picks how each flaw
//! is closed, from what the processor offers, what the kernel was built
//! with ([`Built`]) and what the command line asks ([`Choice`]): the
//! [`Controls`] the architecture applies on every CPU and at every entry
//! to and exit from the kernel, and the line `/kernel/devices/system/cpu/
//! vulnerabilities/<flaw>` shows for each ([`Plan::write_line`]), in
//! Linux's words, so that tools written for Linux read them.
//!
//! References: Intel's "Affected Processors: Transient Execution Attacks
//! and Related Security Issues by CPU" and its guidance for each flaw;
//! AMD's security bulletins (AMD-SB-1010, 1037, 1038, 7008, 7029) and its
//! "Software Techniques for Managing Speculation on AMD Processors"; Linux's
//! `arch/x86/kernel/cpu/bugs.c`, as the reference for the defaults and the
//! wording of the report (no code taken).
//!
//! This crate only decides; the architecture reads the registers and
//! applies the controls, so the decisions are tested on the host.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]

mod models;
mod plan;
#[cfg(test)]
mod tests;

pub use plan::{
    BadValue, Built, Choice, Controls, FLAGS, IndirectBranches, Pick, Plan, ReturnPath,
    SPEC_CTRL_BHI_DIS_S, SPEC_CTRL_IBRS, SPEC_CTRL_RRSBA_DIS_S, SPEC_CTRL_SSBD, SPEC_CTRL_STIBP,
    SpectreV2,
};

/// CPUID bits the decisions read, named as Intel's and AMD's manuals name
/// them.
pub mod bits {
    /// Leaf 1, ECX: AVX.
    pub const LEAF1_AVX: u32 = 1 << 28;
    /// Leaf 1, ECX: RDRAND.
    pub const LEAF1_RDRAND: u32 = 1 << 30;
    /// Leaf 1, ECX: the processor runs under a hypervisor.
    pub const LEAF1_HYPERVISOR: u32 = 1 << 31;
    /// Leaf 7, EBX: RDSEED.
    pub const LEAF7_RDSEED: u32 = 1 << 18;
    /// Leaf 7, EBX: supervisor-mode access prevention (SMAP).
    pub const LEAF7_SMAP: u32 = 1 << 20;
    /// Leaf 7, EBX: restricted transactional memory (TSX's RTM).
    pub const LEAF7_RTM: u32 = 1 << 11;
    /// Leaf 7, EDX: IA32_MCU_OPT_CTRL's SRBDS control.
    pub const LEAF7_SRBDS_CTRL: u32 = 1 << 9;
    /// Leaf 7, EDX: VERW clears the CPU's buffers (MD_CLEAR).
    pub const LEAF7_MD_CLEAR: u32 = 1 << 10;
    /// Leaf 7, EDX: the processor has cores of more than one kind.
    pub const LEAF7_HYBRID: u32 = 1 << 15;
    /// Leaf 7, EDX: IA32_SPEC_CTRL's IBRS and IA32_PRED_CMD's IBPB.
    pub const LEAF7_IBRS_IBPB: u32 = 1 << 26;
    /// Leaf 7, EDX: IA32_SPEC_CTRL's STIBP.
    pub const LEAF7_STIBP: u32 = 1 << 27;
    /// Leaf 7, EDX: IA32_FLUSH_CMD's L1D_FLUSH.
    pub const LEAF7_L1D_FLUSH: u32 = 1 << 28;
    /// Leaf 7, EDX: IA32_ARCH_CAPABILITIES.
    pub const LEAF7_ARCH_CAPABILITIES: u32 = 1 << 29;
    /// Leaf 7, EDX: IA32_SPEC_CTRL's SSBD.
    pub const LEAF7_SSBD: u32 = 1 << 31;
    /// Leaf 7 subleaf 2, EDX: IA32_SPEC_CTRL's RRSBA_DIS_U and _S.
    pub const LEAF7_2_RRSBA_CTRL: u32 = 1 << 2;
    /// Leaf 7 subleaf 2, EDX: IA32_SPEC_CTRL's BHI_DIS_S.
    pub const LEAF7_2_BHI_CTRL: u32 = 1 << 4;
    /// Leaf 8000_0008h, EBX: IBPB.
    pub const EXT8_IBPB: u32 = 1 << 12;
    /// Leaf 8000_0008h, EBX: IBRS.
    pub const EXT8_IBRS: u32 = 1 << 14;
    /// Leaf 8000_0008h, EBX: STIBP.
    pub const EXT8_STIBP: u32 = 1 << 15;
    /// Leaf 8000_0008h, EBX: STIBP is best left on at all times.
    pub const EXT8_STIBP_ALWAYS_ON: u32 = 1 << 17;
    /// Leaf 8000_0008h, EBX: SSBD through IA32_SPEC_CTRL.
    pub const EXT8_SSBD: u32 = 1 << 24;
    /// Leaf 8000_0008h, EBX: SSBD through VIRT_SPEC_CTRL, under a
    /// hypervisor.
    pub const EXT8_VIRT_SSBD: u32 = 1 << 25;
    /// Leaf 8000_0008h, EBX: not affected by Speculative Store Bypass.
    pub const EXT8_SSB_NO: u32 = 1 << 26;
    /// Leaf 8000_0008h, EBX: not affected by Branch Type Confusion
    /// (Retbleed).
    pub const EXT8_BTC_NO: u32 = 1 << 29;
    /// Leaf 8000_0021h, EAX: VERW clears the buffers TSA reads.
    pub const EXT21_VERW_CLEAR: u32 = 1 << 5;
    /// Leaf 8000_0021h, EAX: automatic IBRS (EFER.AIBRSE).
    pub const EXT21_AUTOIBRS: u32 = 1 << 8;
    /// Leaf 8000_0021h, EAX: IBPB also flushes return predictions.
    pub const EXT21_IBPB_BRTYPE: u32 = 1 << 28;
    /// Leaf 8000_0021h, EAX: not affected by SRSO.
    pub const EXT21_SRSO_NO: u32 = 1 << 29;
    /// Leaf 8000_0021h, EAX: not affected by SRSO between programs and the
    /// kernel (only between guests and their host).
    pub const EXT21_SRSO_USER_KERNEL_NO: u32 = 1 << 30;
    /// Leaf 8000_0021h, ECX: not affected by TSA through the store queue.
    pub const EXT21_TSA_SQ_NO: u32 = 1 << 1;
    /// Leaf 8000_0021h, ECX: not affected by TSA through the L1 cache.
    pub const EXT21_TSA_L1_NO: u32 = 1 << 2;
}

/// IA32_ARCH_CAPABILITIES (MSR 10Ah) bits, as Intel's manual names them.
pub mod caps {
    /// Not affected by Meltdown.
    pub const RDCL_NO: u64 = 1 << 0;
    /// Enhanced IBRS.
    pub const IBRS_ALL: u64 = 1 << 1;
    /// Returns may be predicted from the branch target buffer when the
    /// return stack is empty.
    pub const RSBA: u64 = 1 << 2;
    /// Not affected by Speculative Store Bypass.
    pub const SSB_NO: u64 = 1 << 4;
    /// Not affected by MDS.
    pub const MDS_NO: u64 = 1 << 5;
    /// Not affected by the instruction TLB multihit erratum.
    pub const PSCHANGE_MC_NO: u64 = 1 << 6;
    /// IA32_TSX_CTRL exists.
    pub const TSX_CTRL: u64 = 1 << 7;
    /// Not affected by TSX Asynchronous Abort.
    pub const TAA_NO: u64 = 1 << 8;
    /// Not affected by Shared Buffers Data Read.
    pub const SBDR_SSDP_NO: u64 = 1 << 13;
    /// Not affected by Fill Buffer Stale Data Propagator.
    pub const FBSDP_NO: u64 = 1 << 14;
    /// Not affected by Primary Stale Data Propagator.
    pub const PSDP_NO: u64 = 1 << 15;
    /// VERW clears the fill buffers (MMIO stale data's microcode).
    pub const FB_CLEAR: u64 = 1 << 17;
    /// Returns may be predicted from the branch target buffer when the
    /// return stack is empty, also with enhanced IBRS.
    pub const RRSBA: u64 = 1 << 19;
    /// Not affected by Branch History Injection.
    pub const BHI_NO: u64 = 1 << 20;
    /// Not affected by Post-barrier Return Stack Buffer predictions.
    pub const PBRSB_NO: u64 = 1 << 24;
    /// IA32_MCU_OPT_CTRL's GDS control.
    pub const GDS_CTRL: u64 = 1 << 25;
    /// Not affected by Gather Data Sampling.
    pub const GDS_NO: u64 = 1 << 26;
    /// Not affected by Register File Data Sampling.
    pub const RFDS_NO: u64 = 1 << 27;
    /// VERW clears the register file (RFDS's microcode).
    pub const RFDS_CLEAR: u64 = 1 << 28;
    /// Not affected by Indirect Target Selection.
    pub const ITS_NO: u64 = 1 << 62;
}

/// IA32_MCU_OPT_CTRL (MSR 123h) bits.
pub mod mcu_opt {
    /// The microcode's SRBDS protection is off.
    pub const RNGDS_MITG_DIS: u64 = 1 << 0;
    /// The microcode's GDS protection is off.
    pub const GDS_MITG_DIS: u64 = 1 << 4;
    /// GDS_MITG_DIS can no longer change until the next reset.
    pub const GDS_MITG_LOCKED: u64 = 1 << 5;
}

/// Who made the processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    /// GenuineIntel.
    Intel,
    /// AuthenticAMD.
    Amd,
    /// HygonGenuine.
    Hygon,
    /// Anyone else, assumed to have every flaw it does not rule out.
    Other,
}

/// What the running processor says of itself.
#[derive(Clone, Copy, Debug)]
pub struct Cpu {
    /// Its maker.
    pub vendor: Vendor,
    /// Its family, with the extended family added.
    pub family: u32,
    /// Its model, with the extended model added.
    pub model: u32,
    /// Its stepping.
    pub stepping: u32,
    /// The microcode revision it runs.
    pub microcode: u32,
    /// CPUID leaf 1, ECX.
    pub leaf1_ecx: u32,
    /// CPUID leaf 7, EBX.
    pub leaf7_ebx: u32,
    /// CPUID leaf 7, EDX.
    pub leaf7_edx: u32,
    /// CPUID leaf 7 subleaf 2, EDX.
    pub leaf7_2_edx: u32,
    /// CPUID leaf 8000_0008h, EBX.
    pub ext8_ebx: u32,
    /// The physical address bits it has: CPUID leaf 8000_0008h, EAX bits 0
    /// to 7.
    pub phys_bits: u32,
    /// The address past the machine's highest RAM, which the page tables'
    /// guard against L1TF must stay above.
    pub memory_end: u64,
    /// CPUID leaf 8000_0021h, EAX.
    pub ext21_eax: u32,
    /// CPUID leaf 8000_0021h, ECX.
    pub ext21_ecx: u32,
    /// IA32_ARCH_CAPABILITIES, or 0 where the processor has none.
    pub arch_capabilities: u64,
    /// IA32_MCU_OPT_CTRL, where the processor has it.
    pub mcu_opt_ctrl: Option<u64>,
    /// Its cores run more than one hardware thread.
    pub smt: bool,
}

impl Cpu {
    pub(crate) fn caps(&self, bits: u64) -> bool {
        self.arch_capabilities & bits == bits
    }

    /// The address bits the tags of its first-level data cache hold, which
    /// L1TF reads ahead with: its physical address bits (36 where it does
    /// not say), but at least 44 on the Intel cores whose caches hold more
    /// than their addresses reach.
    pub fn cache_bits(&self) -> u32 {
        let bits = if self.phys_bits == 0 { 36 } else { self.phys_bits.clamp(36, 52) };
        let wider = self.vendor == Vendor::Intel
            && self.family == 6
            && models::CACHE_44_BITS.contains(&self.model);
        if wider { bits.max(44) } else { bits }
    }

    /// Whether it runs under a hypervisor.
    pub fn hypervisor(&self) -> bool {
        self.leaf1_ecx & bits::LEAF1_HYPERVISOR != 0
    }

    /// Whether it is an AMD Zen or a Hygon, whose families from 17h on share
    /// Zen's design.
    fn zen(&self) -> bool {
        matches!(self.vendor, Vendor::Amd | Vendor::Hygon) && self.family >= 0x17
    }

    /// Whether it has IBPB.
    pub fn ibpb(&self) -> bool {
        self.leaf7_edx & bits::LEAF7_IBRS_IBPB != 0 || self.ext8_ebx & bits::EXT8_IBPB != 0
    }

    /// Whether it has IBRS in IA32_SPEC_CTRL.
    pub fn ibrs(&self) -> bool {
        self.leaf7_edx & bits::LEAF7_IBRS_IBPB != 0 || self.ext8_ebx & bits::EXT8_IBRS != 0
    }

    /// Whether it has STIBP in IA32_SPEC_CTRL.
    pub fn stibp(&self) -> bool {
        self.leaf7_edx & bits::LEAF7_STIBP != 0 || self.ext8_ebx & bits::EXT8_STIBP != 0
    }

    /// Whether it has SSBD in IA32_SPEC_CTRL.
    pub fn ssbd(&self) -> bool {
        self.leaf7_edx & bits::LEAF7_SSBD != 0 || self.ext8_ebx & bits::EXT8_SSBD != 0
    }

    /// Whether it has enhanced IBRS (Intel) or automatic IBRS (AMD): IBRS
    /// set once protects the kernel from then on.
    pub fn eibrs(&self) -> bool {
        self.caps(caps::IBRS_ALL) || self.ext21_eax & bits::EXT21_AUTOIBRS != 0
    }

    /// Whether it has TSX's restricted transactional memory.
    pub(crate) fn rtm(&self) -> bool {
        self.leaf7_ebx & bits::LEAF7_RTM != 0
    }
}

/// A flaw, by the name of its file in
/// `/kernel/devices/system/cpu/vulnerabilities`, as Linux names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flaw {
    /// Rogue Data Cache Load: a program reads kernel memory.
    Meltdown,
    /// Bounds check bypass.
    SpectreV1,
    /// Branch target injection.
    SpectreV2,
    /// Speculative Store Bypass (Spectre variant 4).
    SpecStoreBypass,
    /// L1 Terminal Fault: memory behind a page entry that is not present.
    L1tf,
    /// Microarchitectural Data Sampling.
    Mds,
    /// TSX Asynchronous Abort.
    TsxAsyncAbort,
    /// A large page's change machine-checks a CPU that runs a guest.
    ItlbMultihit,
    /// Special Register Buffer Data Sampling (RDRAND and RDSEED's output).
    Srbds,
    /// Processor MMIO Stale Data.
    MmioStaleData,
    /// Returns predicted from the branch target buffer.
    Retbleed,
    /// Speculative Return Stack Overflow (Inception).
    SpecRstackOverflow,
    /// Gather Data Sampling (Downfall).
    GatherDataSampling,
    /// Register File Data Sampling.
    RegFileDataSampling,
    /// Indirect Target Selection.
    IndirectTargetSelection,
    /// Transient Scheduler Attacks.
    Tsa,
    /// A guest steering its host's user programs' branches.
    Vmscape,
}

impl Flaw {
    /// Every flaw, in the order `ls` lists their files.
    pub const ALL: [Flaw; 17] = [
        Flaw::GatherDataSampling,
        Flaw::IndirectTargetSelection,
        Flaw::ItlbMultihit,
        Flaw::L1tf,
        Flaw::Mds,
        Flaw::Meltdown,
        Flaw::MmioStaleData,
        Flaw::RegFileDataSampling,
        Flaw::Retbleed,
        Flaw::SpecRstackOverflow,
        Flaw::SpecStoreBypass,
        Flaw::SpectreV1,
        Flaw::SpectreV2,
        Flaw::Srbds,
        Flaw::Tsa,
        Flaw::TsxAsyncAbort,
        Flaw::Vmscape,
    ];

    /// The name of its file.
    pub const fn name(self) -> &'static str {
        match self {
            Flaw::Meltdown => "meltdown",
            Flaw::SpectreV1 => "spectre_v1",
            Flaw::SpectreV2 => "spectre_v2",
            Flaw::SpecStoreBypass => "spec_store_bypass",
            Flaw::L1tf => "l1tf",
            Flaw::Mds => "mds",
            Flaw::TsxAsyncAbort => "tsx_async_abort",
            Flaw::ItlbMultihit => "itlb_multihit",
            Flaw::Srbds => "srbds",
            Flaw::MmioStaleData => "mmio_stale_data",
            Flaw::Retbleed => "retbleed",
            Flaw::SpecRstackOverflow => "spec_rstack_overflow",
            Flaw::GatherDataSampling => "gather_data_sampling",
            Flaw::RegFileDataSampling => "reg_file_data_sampling",
            Flaw::IndirectTargetSelection => "indirect_target_selection",
            Flaw::Tsa => "tsa",
            Flaw::Vmscape => "vmscape",
        }
    }

    /// The flaw whose file is `name`.
    pub fn named(name: &str) -> Option<Flaw> {
        Flaw::ALL.into_iter().find(|flaw| flaw.name() == name)
    }

    const fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// Which flaws a processor has ([`find`]), with the details of some that
/// decide how they are closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    flaws: u32,
    /// MDS reaches it only through the store buffer.
    pub msbds_only: bool,
    /// The SWAPGS variant of Spectre variant 1 applies.
    pub swapgs: bool,
    /// Branch History Injection applies (enhanced IBRS parts, and any
    /// under a hypervisor, which may hide it).
    pub bhi: bool,
    /// Post-barrier return predictions apply to its enhanced IBRS.
    pub pbrsb: bool,
    /// The other hardware thread can poison its return stack (Zen 1, 2).
    pub smt_rsb: bool,
    /// Indirect Target Selection reaches it only from its own programs.
    pub its_native_only: bool,
    /// A division by zero leaves its quotient for the next program to
    /// read (Zen 1).
    pub div0: bool,
    /// Zen 2's vector registers leak through a mispredicted VZEROUPPER
    /// until the chicken bit or a fixed microcode stops it.
    pub zenbleed: bool,
}

impl Found {
    /// Whether it has `flaw`.
    pub fn has(&self, flaw: Flaw) -> bool {
        self.flaws & flaw.bit() != 0
    }

    fn add(&mut self, flaw: Flaw) {
        self.flaws |= flaw.bit();
    }
}

/// Works out which flaws `cpu` has.
pub fn find(cpu: &Cpu) -> Found {
    let traits = match cpu.vendor {
        Vendor::Intel if cpu.family == 6 => models::intel(cpu.model, cpu.stepping),
        Vendor::Amd => models::amd(cpu.family),
        Vendor::Hygon => models::hygon(cpu.family),
        Vendor::Intel | Vendor::Other => 0,
    };
    let is = |bits: u32| traits & bits != 0;
    let mut found = Found::default();
    if !is(models::NO_ITLB_MULTIHIT) && !cpu.caps(caps::PSCHANGE_MC_NO) {
        found.add(Flaw::ItlbMultihit);
    }
    if is(models::NO_SPECULATION) {
        return found;
    }
    found.add(Flaw::SpectreV1);
    found.add(Flaw::SpectreV2);
    if !is(models::NO_SSB) && !cpu.caps(caps::SSB_NO) && cpu.ext8_ebx & bits::EXT8_SSB_NO == 0 {
        found.add(Flaw::SpecStoreBypass);
    }
    found.pbrsb = cpu.eibrs() && !is(models::NO_EIBRS_PBRSB) && !cpu.caps(caps::PBRSB_NO);
    if !is(models::NO_MDS) && !cpu.caps(caps::MDS_NO) {
        found.add(Flaw::Mds);
        found.msbds_only = is(models::MSBDS_ONLY);
    }
    found.swapgs = !is(models::NO_SWAPGS);
    if !cpu.caps(caps::TAA_NO) && (cpu.rtm() || cpu.caps(caps::TSX_CTRL)) {
        found.add(Flaw::TsxAsyncAbort);
    }
    let rng = cpu.leaf1_ecx & bits::LEAF1_RDRAND != 0 || cpu.leaf7_ebx & bits::LEAF7_RDSEED != 0;
    if rng && is(models::SRBDS | models::MMIO_SBDS) {
        found.add(Flaw::Srbds);
    }
    let mmio_immune = cpu.caps(caps::FBSDP_NO | caps::PSDP_NO | caps::SBDR_SSDP_NO);
    if !mmio_immune && is(models::MMIO) {
        found.add(Flaw::MmioStaleData);
    }
    if cpu.ext8_ebx & bits::EXT8_BTC_NO == 0 && (is(models::RETBLEED) || cpu.caps(caps::RSBA)) {
        found.add(Flaw::Retbleed);
    }
    found.smt_rsb = is(models::SMT_RSB);
    if cpu.ext21_eax & bits::EXT21_SRSO_NO == 0 && is(models::SRSO) {
        found.add(Flaw::SpecRstackOverflow);
    }
    if is(models::GDS) && !cpu.caps(caps::GDS_NO) && cpu.leaf1_ecx & bits::LEAF1_AVX != 0 {
        found.add(Flaw::GatherDataSampling);
    }
    if has_rfds(cpu, traits) {
        found.add(Flaw::RegFileDataSampling);
    }
    found.bhi = !is(models::NO_BHI) && (cpu.eibrs() || cpu.hypervisor());
    if has_its(cpu, traits) {
        found.add(Flaw::IndirectTargetSelection);
        found.its_native_only = is(models::ITS_NATIVE_ONLY);
    }
    if has_tsa(cpu, traits) {
        found.add(Flaw::Tsa);
    }
    if is(models::VMSCAPE) && !cpu.hypervisor() {
        found.add(Flaw::Vmscape);
    }
    if cpu.vendor == Vendor::Amd {
        found.div0 = models::is_zen1(cpu.family, cpu.model);
        found.zenbleed = models::is_zen2(cpu.family, cpu.model)
            && !cpu.hypervisor()
            && cpu.leaf1_ecx & bits::LEAF1_AVX != 0
            && models::zenbleed_needs_chicken_bit(cpu.model, cpu.stepping, cpu.microcode);
    }
    if is(models::NO_MELTDOWN) || cpu.caps(caps::RDCL_NO) {
        return found;
    }
    found.add(Flaw::Meltdown);
    if !is(models::NO_L1TF) {
        found.add(Flaw::L1tf);
    }
    found
}

/// Whether `cpu` has Register File Data Sampling. Its own "not affected"
/// bit wins; a hypervisor that sets RFDS_CLEAR says the guest may run on
/// an affected host; otherwise the model decides, and Alder Lake and
/// Raptor Lake parts are affected only where they have Atom cores.
fn has_rfds(cpu: &Cpu, traits: u32) -> bool {
    if cpu.caps(caps::RFDS_NO) {
        return false;
    }
    if cpu.caps(caps::RFDS_CLEAR) {
        return true;
    }
    if traits & models::RFDS == 0 {
        return false;
    }
    let atom_cores = cpu.leaf7_edx & bits::LEAF7_HYBRID != 0;
    let model = u8::try_from(cpu.model).unwrap_or(0);
    atom_cores || !models::RFDS_NEEDS_ATOM_CORES.contains(&model)
}

/// Whether `cpu` has Indirect Target Selection: only Intel's, never one
/// with BHI_DIS_S (no affected part has it), and under a hypervisor that
/// does not say ITS_NO, whatever the model, as the guest may run on or
/// move to an affected host.
fn has_its(cpu: &Cpu, traits: u32) -> bool {
    if cpu.caps(caps::ITS_NO) || cpu.vendor != Vendor::Intel {
        return false;
    }
    if cpu.leaf7_2_edx & bits::LEAF7_2_BHI_CTRL != 0 {
        return false;
    }
    cpu.hypervisor() || traits & models::ITS != 0
}

/// Whether `cpu` has Transient Scheduler Attacks: AMD's family 19h, and any
/// Zen guest (it may move to such a host), unless it says it is free of
/// both of TSA's ways, through the store queue and through the L1 cache.
fn has_tsa(cpu: &Cpu, traits: u32) -> bool {
    if cpu.vendor != Vendor::Amd {
        return false;
    }
    let free = bits::EXT21_TSA_SQ_NO | bits::EXT21_TSA_L1_NO;
    if cpu.ext21_ecx & free == free {
        return false;
    }
    traits & models::TSA != 0 || (cpu.hypervisor() && cpu.zen())
}
