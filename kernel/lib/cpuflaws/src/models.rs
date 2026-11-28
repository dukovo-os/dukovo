// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What each processor model is known to have and to lack, where the CPU
//! itself gives no way to ask.
//!
//! Newer CPUs say in IA32_ARCH_CAPABILITIES or CPUID that they are free of
//! a flaw ("MDS_NO"); for the rest, which flaws a model has comes from
//! Intel's list of affected processors ("Affected Processors: Transient
//! Execution Attacks and Related Security Issues by CPU") and AMD's
//! security bulletins. A model in neither table here has every flaw its
//! vendor's other models have, but those its CPUID or capability bits
//! rule out.

/// Not affected by any speculation flaw: in-order Atoms.
pub const NO_SPECULATION: u32 = 1 << 0;
/// Not affected by Speculative Store Bypass.
pub const NO_SSB: u32 = 1 << 1;
/// Not affected by L1 Terminal Fault.
pub const NO_L1TF: u32 = 1 << 2;
/// Not affected by Microarchitectural Data Sampling.
pub const NO_MDS: u32 = 1 << 3;
/// Affected by MDS only through the store buffer (MSBDS), which hardware
/// threads share only while one of them is idle.
pub const MSBDS_ONLY: u32 = 1 << 4;
/// Not affected by the SWAPGS variant of Spectre variant 1.
pub const NO_SWAPGS: u32 = 1 << 5;
/// Not affected by the instruction TLB multihit erratum.
pub const NO_ITLB_MULTIHIT: u32 = 1 << 6;
/// Not affected by Post-barrier Return Stack Buffer predictions on
/// enhanced IBRS.
pub const NO_EIBRS_PBRSB: u32 = 1 << 7;
/// Not affected by Branch History Injection.
pub const NO_BHI: u32 = 1 << 8;
/// Not affected by Meltdown (Rogue Data Cache Load).
pub const NO_MELTDOWN: u32 = 1 << 9;

/// Affected by Special Register Buffer Data Sampling.
pub const SRBDS: u32 = 1 << 16;
/// Affected by Processor MMIO Stale Data.
pub const MMIO: u32 = 1 << 17;
/// Affected by Shared Buffers Data Sampling, an MMIO stale data flaw
/// that is handled as SRBDS is.
pub const MMIO_SBDS: u32 = 1 << 18;
/// Affected by Retbleed (returns predicted from the branch target buffer).
pub const RETBLEED: u32 = 1 << 19;
/// Affected by Gather Data Sampling (Downfall).
pub const GDS: u32 = 1 << 20;
/// Affected by Register File Data Sampling (on Atom cores).
pub const RFDS: u32 = 1 << 21;
/// Affected by Indirect Target Selection.
pub const ITS: u32 = 1 << 22;
/// Affected by Indirect Target Selection only between privilege levels
/// on the CPU itself, not from a guest.
pub const ITS_NATIVE_ONLY: u32 = 1 << 23;
/// Affected by return-stack poisoning from the other hardware thread.
pub const SMT_RSB: u32 = 1 << 24;
/// Affected by Speculative Return Stack Overflow (Inception).
pub const SRSO: u32 = 1 << 25;
/// Affected by Transient Scheduler Attacks.
pub const TSA: u32 = 1 << 26;
/// Affected by VMSCAPE (a guest steering its host's user programs).
pub const VMSCAPE: u32 = 1 << 27;

/// Any stepping.
const ALL: u8 = 0xff;

/// Intel's family 6 models: the model, the last stepping the entry
/// covers, and what that model has and lacks. A model with two entries
/// differs by stepping; the first entry whose stepping range holds the
/// CPU's applies.
const INTEL: &[(u8, u8, u32)] = &[
    // In-order Atoms (Bonnell, Saltwell).
    (0x1c, ALL, NO_SPECULATION | NO_ITLB_MULTIHIT),
    (0x26, ALL, NO_SPECULATION | NO_ITLB_MULTIHIT),
    (0x27, ALL, NO_SPECULATION | NO_ITLB_MULTIHIT),
    (0x35, ALL, NO_SPECULATION | NO_ITLB_MULTIHIT),
    (0x36, ALL, NO_SPECULATION | NO_ITLB_MULTIHIT),
    // Silvermont, Airmont and Knights Landing and Mill.
    (0x37, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x4a, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x4c, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x4d, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x57, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x5a, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x75, ALL, NO_SSB | NO_L1TF | NO_SWAPGS | NO_ITLB_MULTIHIT),
    (0x85, ALL, NO_SSB | NO_L1TF | MSBDS_ONLY | NO_SWAPGS | NO_ITLB_MULTIHIT),
    // Goldmont and Goldmont Plus.
    (0x5c, ALL, NO_MDS | NO_L1TF | NO_SWAPGS | NO_ITLB_MULTIHIT | RFDS),
    (0x5f, ALL, NO_MDS | NO_L1TF | NO_SWAPGS | NO_ITLB_MULTIHIT | RFDS),
    (0x7a, ALL, NO_MDS | NO_L1TF | NO_SWAPGS | NO_ITLB_MULTIHIT | NO_EIBRS_PBRSB | RFDS),
    // Tremont.
    (0x86, ALL, NO_ITLB_MULTIHIT | NO_EIBRS_PBRSB | MMIO | RFDS),
    (0x96, ALL, NO_EIBRS_PBRSB | MMIO | MMIO_SBDS | RFDS),
    (0x9c, ALL, NO_EIBRS_PBRSB | MMIO | MMIO_SBDS | RFDS),
    // Gracemont (Alder Lake-N) and Crestmont (Sierra Forest).
    (0xbe, ALL, RFDS | VMSCAPE),
    (0xaf, ALL, VMSCAPE),
    // Sandy Bridge and Ivy Bridge.
    (0x2a, ALL, VMSCAPE),
    (0x2d, ALL, VMSCAPE),
    (0x3a, ALL, SRBDS | VMSCAPE),
    (0x3e, ALL, VMSCAPE),
    // Haswell and Broadwell.
    (0x3c, ALL, SRBDS | VMSCAPE),
    (0x45, ALL, SRBDS | VMSCAPE),
    (0x46, ALL, SRBDS | VMSCAPE),
    (0x3f, ALL, MMIO | VMSCAPE),
    (0x3d, ALL, SRBDS | VMSCAPE),
    (0x47, ALL, SRBDS | VMSCAPE),
    (0x4f, ALL, MMIO | VMSCAPE),
    (0x56, ALL, MMIO | VMSCAPE),
    // Skylake: Xeon Scalable first generation to stepping 5, Cascade Lake
    // and Cooper Lake after.
    (0x55, 0x05, MMIO | RETBLEED | GDS | VMSCAPE),
    (0x55, ALL, MMIO | RETBLEED | GDS | ITS | VMSCAPE),
    (0x4e, ALL, MMIO | RETBLEED | GDS | SRBDS | VMSCAPE),
    (0x5e, ALL, MMIO | RETBLEED | GDS | SRBDS | VMSCAPE),
    // Kaby Lake, Coffee Lake, Whiskey Lake and Amber Lake.
    (0x8e, 0x0b, MMIO | RETBLEED | GDS | SRBDS | VMSCAPE),
    (0x8e, ALL, MMIO | RETBLEED | GDS | SRBDS | ITS | VMSCAPE),
    (0x9e, 0x0c, MMIO | RETBLEED | GDS | SRBDS | VMSCAPE),
    (0x9e, ALL, MMIO | RETBLEED | GDS | SRBDS | ITS | VMSCAPE),
    // Cannon Lake.
    (0x66, ALL, RETBLEED | VMSCAPE),
    // Comet Lake.
    (0xa5, ALL, MMIO | MMIO_SBDS | RETBLEED | GDS | ITS | VMSCAPE),
    (0xa6, 0x00, MMIO | RETBLEED | ITS | VMSCAPE),
    (0xa6, ALL, MMIO | MMIO_SBDS | RETBLEED | GDS | ITS | VMSCAPE),
    // Ice Lake.
    (0x7e, ALL, MMIO | MMIO_SBDS | RETBLEED | GDS | ITS | ITS_NATIVE_ONLY),
    (0x6a, ALL, MMIO | GDS | ITS | ITS_NATIVE_ONLY),
    (0x6c, ALL, MMIO | GDS | ITS | ITS_NATIVE_ONLY),
    // Lakefield.
    (0x8a, ALL, MMIO | MMIO_SBDS | RETBLEED),
    // Tiger Lake and Rocket Lake.
    (0x8c, ALL, GDS | ITS | ITS_NATIVE_ONLY),
    (0x8d, ALL, GDS | ITS | ITS_NATIVE_ONLY),
    (0xa7, ALL, MMIO | RETBLEED | GDS | ITS | ITS_NATIVE_ONLY),
    // Alder Lake and Raptor Lake; RFDS only on their Atom cores
    // (`rfds_needs_atom_cores`).
    (0x97, ALL, RFDS | VMSCAPE),
    (0x9a, ALL, RFDS | VMSCAPE),
    (0xb7, ALL, RFDS | VMSCAPE),
    (0xba, ALL, RFDS | VMSCAPE),
    (0xbf, ALL, RFDS | VMSCAPE),
    // Meteor Lake, Arrow Lake, Lunar Lake, and Sapphire, Emerald and
    // Granite Rapids.
    (0xaa, ALL, VMSCAPE),
    (0xb5, ALL, VMSCAPE),
    (0xc5, ALL, VMSCAPE),
    (0xc6, ALL, VMSCAPE),
    (0xbd, ALL, VMSCAPE),
    (0x8f, ALL, VMSCAPE),
    (0xcf, ALL, VMSCAPE),
    (0xad, ALL, VMSCAPE),
];

/// The Intel family 6 models whose first-level data cache's tags hold 44
/// address bits, more than their addresses reach: Nehalem, Westmere, Sandy
/// Bridge, Ivy Bridge, Haswell, Broadwell, Skylake and Kaby Lake.
pub const CACHE_44_BITS: [u32; 13] =
    [0x1e, 0x25, 0x2a, 0x3a, 0x3c, 0x45, 0x46, 0x3d, 0x47, 0x4e, 0x5e, 0x8e, 0x9e];

/// Alder Lake and Raptor Lake desktop models, whose parts without Atom
/// cores are free of RFDS.
pub const RFDS_NEEDS_ATOM_CORES: [u8; 2] = [0x97, 0xb7];

/// What AMD's families have and lack. Families from 13h on (but those
/// listed) have only the flaws common to all of them.
const AMD: &[(u32, u32)] = &[
    (0x0f, NO_SSB),
    (0x10, NO_SSB),
    (0x11, NO_SSB),
    (0x12, NO_SSB),
    (0x15, RETBLEED),
    (0x16, RETBLEED),
    (0x17, RETBLEED | SMT_RSB | SRSO | VMSCAPE),
    (0x19, SRSO | TSA | VMSCAPE),
    (0x1a, SRSO | VMSCAPE),
];

/// What every AMD and Hygon processor lacks.
const AMD_ALL: u32 = NO_MELTDOWN | NO_L1TF | NO_MDS | NO_SWAPGS | NO_ITLB_MULTIHIT | NO_BHI;

/// What every AMD and Hygon processor after family 12h lacks, beyond
/// [`AMD_ALL`]. (Families to 12h have no enhanced IBRS for the question
/// to arise.)
const AMD_LATER: u32 = NO_EIBRS_PBRSB;

/// Hygon's one family, Dhyana, a Zen derivative.
const HYGON: &[(u32, u32)] = &[(0x18, RETBLEED | SMT_RSB | SRSO | VMSCAPE)];

/// What Intel's family 6 model `model` at `stepping` has and lacks.
pub fn intel(model: u32, stepping: u32) -> u32 {
    INTEL
        .iter()
        .find(|&&(m, last, _)| u32::from(m) == model && stepping <= u32::from(last))
        .map_or(0, |&(_, _, traits)| traits)
}

/// What AMD's family `family` has and lacks.
pub fn amd(family: u32) -> u32 {
    let listed = AMD.iter().find(|&&(f, _)| f == family).map_or(0, |&(_, traits)| traits);
    let later = if family > 0x12 { AMD_LATER } else { 0 };
    AMD_ALL | later | listed
}

/// What Hygon's family `family` has and lacks.
pub fn hygon(family: u32) -> u32 {
    let listed = HYGON.iter().find(|&&(f, _)| f == family).map_or(0, |&(_, traits)| traits);
    AMD_ALL | AMD_LATER | listed
}

/// Zen 1: family 17h models 00h to 2Fh and 50h to 5Fh.
pub fn is_zen1(family: u32, model: u32) -> bool {
    family == 0x17 && matches!(model, 0x00..=0x2f | 0x50..=0x5f)
}

/// Zen 2: family 17h models 30h to 4Fh, 60h to 7Fh, 90h, 91h and A0h to
/// AFh.
pub fn is_zen2(family: u32, model: u32) -> bool {
    family == 0x17 && matches!(model, 0x30..=0x4f | 0x60..=0x7f | 0x90..=0x91 | 0xa0..=0xaf)
}

/// The first microcode revision of each Zen 2 model and stepping that
/// fixes Zenbleed in microcode, from AMD's bulletin AMD-SB-7008: the
/// family 17h model, its stepping, and the revision.
const ZENBLEED_FIXED: &[(u32, u32, u32)] = &[
    (0x31, 0x0, 0x0830_107b),
    (0x60, 0x1, 0x0860_010c),
    (0x68, 0x1, 0x0860_8107),
    (0x71, 0x0, 0x0870_1033),
    (0xa0, 0x0, 0x08a0_0009),
];

/// Whether Zen 2 at `model` and `stepping`, running microcode `revision`,
/// still needs the decode unit's chicken bit against Zenbleed: it does
/// unless its model has a fixing revision and runs at least that one.
pub fn zenbleed_needs_chicken_bit(model: u32, stepping: u32, revision: u32) -> bool {
    !ZENBLEED_FIXED.iter().any(|&(m, s, fixed)| m == model && s == stepping && revision >= fixed)
}
