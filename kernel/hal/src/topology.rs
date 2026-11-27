// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where an x86-64 CPU sits, from its APIC ID (ADR 0797).
//!
//! An APIC ID is a number made of fields, lowest first: the hardware
//! thread within its core, the core within its package (with Intel's
//! modules, tiles and dies, and AMD's core complexes, between them), and
//! the package. CPUID gives the fields' widths. Intel's extended topology
//! leaf 0x1F names every level and gives each one's shift: the number of
//! the ID's low bits that number the CPUs within one of that level; leaf
//! 0xB, which came first and which AMD's processors from Zen 2 also have,
//! names the hardware threads and the cores only, and the cores' shift is
//! then the package's, except on Intel's processors that have dies or
//! tiles, where only 0x1F reaches the package. Older AMD processors give
//! the package's width in leaf 0x8000_0008, and older processors of either
//! vendor only the count of logical processors per package, in leaf 1.
//!
//! The CPUs that share a cache are numbered by the ID's bits above that
//! cache's shift: the deterministic cache parameters, Intel's leaf 4 and
//! AMD's 0x8000_001D, give for each cache how many IDs share it. The
//! last-level cache is the one of the highest level. Where it is shared by
//! fewer CPUs than a package has (AMD's core complexes of four or eight
//! cores, or the dies of Intel's processors that have several), its number
//! within the package tells the scheduler which cores keep each other's
//! data close (`radix_sched::Level::Cache`).

use crate::CpuPlace;

/// The four registers a CPUID leaf returns: EAX, EBX, ECX and EDX.
pub type Regs = [u32; 4];

/// Whose processor it is, which says which leaves describe its caches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    /// Intel's.
    Intel,
    /// AMD's.
    Amd,
    /// Anyone else's, or a hypervisor's that names no vendor.
    Other,
}

/// How an APIC ID splits: the number of its low bits that number the
/// hardware threads of a core, the CPUs that share a last-level cache,
/// and the CPUs of a package, in that order and never decreasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApicLayout {
    /// Bits for the hardware thread within its core.
    pub thread: u32,
    /// Bits for the CPU among those sharing its last-level cache.
    pub cache: u32,
    /// Bits for the CPU within its package.
    pub package: u32,
}

/// Intel's leaf 0x1F, which names dies and tiles.
const LEAF_V2_TOPOLOGY: u32 = 0x1f;
/// The extended topology leaf, threads and cores only.
const LEAF_TOPOLOGY: u32 = 0xb;
/// Intel's deterministic cache parameters.
const LEAF_CACHES: u32 = 4;
/// AMD's deterministic cache parameters.
const LEAF_AMD_CACHES: u32 = 0x8000_001d;
/// AMD's address sizes and the package's APIC ID width.
const LEAF_AMD_SIZES: u32 = 0x8000_0008;
/// AMD's extended features, whose topology extensions bit says that
/// [`LEAF_AMD_CACHES`] is there.
const LEAF_AMD_FEATURES: u32 = 0x8000_0001;
/// CPUID 0x8000_0001 ECX: topology extensions.
const AMD_TOPOLOGY_EXTENSIONS: u32 = 1 << 22;
/// AMD's compute unit and core identifiers.
const LEAF_AMD_CORE: u32 = 0x8000_001e;
/// A topology level's type in leaf 0xB or 0x1F: the hardware threads.
const LEVEL_THREAD: u32 = 1;
/// A cache's type in the cache parameters: none, which ends the list.
const CACHE_NONE: u32 = 0;
/// A cache's type in the cache parameters: instructions only.
const CACHE_INSTRUCTIONS: u32 = 2;

impl ApicLayout {
    /// Reads the layout from CPUID, through `cpuid(leaf, subleaf)`, on a
    /// processor of `vendor`. Only leaves the processor says it has are
    /// asked for: Intel's return the highest leaf's registers for any leaf
    /// beyond it.
    pub fn read(vendor: Vendor, cpuid: impl Fn(u32, u32) -> Regs) -> ApicLayout {
        let max = cpuid(0, 0)[0];
        let max_extended = cpuid(0x8000_0000, 0)[0];
        let has = |leaf: u32| {
            if leaf >= 0x8000_0000 { max_extended >= leaf } else { max >= leaf }
        };
        let levels = |leaf: u32| has(leaf).then(|| extended_levels(leaf, &cpuid)).flatten();
        let (thread, package) = levels(LEAF_V2_TOPOLOGY)
            .or_else(|| levels(LEAF_TOPOLOGY))
            .or_else(|| (vendor == Vendor::Amd).then(|| amd_levels(&has, &cpuid)).flatten())
            .unwrap_or_else(|| legacy_levels(&cpuid));
        let caches = match vendor {
            Vendor::Intel if has(LEAF_CACHES) => Some(LEAF_CACHES),
            Vendor::Amd
                if has(LEAF_AMD_CACHES)
                    && cpuid(LEAF_AMD_FEATURES, 0)[2] & AMD_TOPOLOGY_EXTENSIONS != 0 =>
            {
                Some(LEAF_AMD_CACHES)
            }
            _ => None,
        };
        // A cache shared by fewer IDs than a core has, or more than a
        // package has, is taken to be the core's or the package's.
        let cache = caches
            .and_then(|leaf| last_level_shift(leaf, &cpuid))
            .map_or(package, |shift| shift.clamp(thread, package));
        ApicLayout { thread, cache, package }
    }

    /// Where the CPU with APIC ID `apic_id`, on NUMA node `node`, sits.
    pub fn place(&self, apic_id: u32, node: u8) -> CpuPlace {
        let above = |bits: u32| apic_id.checked_shr(bits).unwrap_or(0);
        let low = |bits: u32| if bits >= 32 { u32::MAX } else { (1u32 << bits) - 1 };
        CpuPlace {
            package: above(self.package),
            cache: above(self.cache) & low(self.package - self.cache),
            core: above(self.thread) & low(self.package - self.thread),
            thread: apic_id & low(self.thread),
            node,
        }
    }
}

/// The hardware threads' shift and the package's from leaf 0xB or 0x1F,
/// if the processor fills it in: its levels, lowest first, end at one of
/// type 0, and the last one's shift is the package's.
fn extended_levels(leaf: u32, cpuid: &impl Fn(u32, u32) -> Regs) -> Option<(u32, u32)> {
    // A leaf whose first level holds no processors is not filled in.
    if cpuid(leaf, 0)[1] & 0xffff == 0 {
        return None;
    }
    let (mut thread, mut package) = (0, None);
    for subleaf in 0..8 {
        let [eax, _, ecx, _] = cpuid(leaf, subleaf);
        let kind = (ecx >> 8) & 0xff;
        if kind == 0 {
            break;
        }
        let shift = eax & 0x1f;
        if kind == LEVEL_THREAD {
            thread = shift;
        } else {
            package = Some(shift);
        }
    }
    package.map(|package| (thread, package.max(thread)))
}

/// The shifts on AMD's processors without leaf 0xB: the package's width
/// from 0x8000_0008, and the threads of a core from 0x8000_001E.
fn amd_levels(has: &impl Fn(u32) -> bool, cpuid: &impl Fn(u32, u32) -> Regs) -> Option<(u32, u32)> {
    if !has(LEAF_AMD_SIZES) {
        return None;
    }
    let package = (cpuid(LEAF_AMD_SIZES, 0)[2] >> 12) & 0xf;
    if package == 0 {
        return None;
    }
    let extensions = cpuid(LEAF_AMD_FEATURES, 0)[2] & AMD_TOPOLOGY_EXTENSIONS != 0;
    let thread = if has(LEAF_AMD_CORE) && extensions {
        bits_for(((cpuid(LEAF_AMD_CORE, 0)[1] >> 8) & 0xff) + 1)
    } else {
        0
    };
    Some((thread.min(package), package))
}

/// The shifts from leaf 1 alone: its count of logical processors per
/// package, where it says it has one, makes up the package's width.
fn legacy_levels(cpuid: &impl Fn(u32, u32) -> Regs) -> (u32, u32) {
    let [_, ebx, _, edx] = cpuid(1, 0);
    if edx & (1 << 28) == 0 {
        return (0, 0);
    }
    (0, bits_for(((ebx >> 16) & 0xff).max(1)))
}

/// The shift of the last-level cache in the cache parameters at `leaf`:
/// of the caches it lists, the one of the highest level holding data.
fn last_level_shift(leaf: u32, cpuid: &impl Fn(u32, u32) -> Regs) -> Option<u32> {
    let mut last: Option<(u32, u32)> = None;
    for subleaf in 0..16 {
        let eax = cpuid(leaf, subleaf)[0];
        let kind = eax & 0x1f;
        if kind == CACHE_NONE {
            break;
        }
        if kind == CACHE_INSTRUCTIONS {
            continue;
        }
        let level = (eax >> 5) & 0x7;
        let sharing = ((eax >> 14) & 0xfff) + 1;
        if last.is_none_or(|(highest, _)| level > highest) {
            last = Some((level, sharing));
        }
    }
    last.map(|(_, sharing)| bits_for(sharing))
}

/// The bits it takes to number `count` things.
fn bits_for(count: u32) -> u32 {
    count.max(1).next_power_of_two().trailing_zeros()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A processor's CPUID, leaf by leaf: zeros for anything not given.
    struct Cpuid(HashMap<(u32, u32), Regs>);

    impl Cpuid {
        fn new(max: u32, max_extended: u32) -> Cpuid {
            let mut leaves = HashMap::new();
            leaves.insert((0, 0), [max, 0, 0, 0]);
            leaves.insert((0x8000_0000, 0), [max_extended, 0, 0, 0]);
            Cpuid(leaves)
        }

        fn with(mut self, leaf: u32, subleaf: u32, regs: Regs) -> Cpuid {
            self.0.insert((leaf, subleaf), regs);
            self
        }

        /// A topology level of leaf 0xB or 0x1F: its shift, its count of
        /// processors and its type.
        fn level(self, leaf: u32, subleaf: u32, shift: u32, count: u32, kind: u32) -> Cpuid {
            self.with(leaf, subleaf, [shift, count, (kind << 8) | subleaf, 0])
        }

        /// A cache of the cache parameters: its type, level and how many
        /// IDs share it.
        fn cache(self, leaf: u32, subleaf: u32, kind: u32, level: u32, sharing: u32) -> Cpuid {
            self.with(leaf, subleaf, [kind | (level << 5) | ((sharing - 1) << 14), 0, 0, 0])
        }

        /// An L1 data and instruction cache per core and an L2 per core
        /// of `threads` hardware threads, then an L3 shared by `sharing`.
        fn caches(self, leaf: u32, threads: u32, sharing: u32) -> Cpuid {
            self.cache(leaf, 0, 1, 1, threads)
                .cache(leaf, 1, 2, 1, threads)
                .cache(leaf, 2, 3, 2, threads)
                .cache(leaf, 3, 3, 3, sharing)
        }

        fn read(&self, vendor: Vendor) -> ApicLayout {
            ApicLayout::read(vendor, |leaf, subleaf| {
                self.0.get(&(leaf, subleaf)).copied().unwrap_or_default()
            })
        }
    }

    #[test]
    fn a_xeon_scalable_package_has_one_cache() {
        // Two Xeon Gold 6152s: 22 cores of two threads each, one L3.
        let cpuid = Cpuid::new(0x16, 0x8000_0008)
            .level(0xb, 0, 1, 2, 1)
            .level(0xb, 1, 6, 44, 2)
            .caches(4, 2, 64);
        let layout = cpuid.read(Vendor::Intel);
        assert_eq!(layout, ApicLayout { thread: 1, cache: 6, package: 6 });
        let place = layout.place(0x45, 1);
        assert_eq!(place, CpuPlace { package: 1, cache: 0, core: 2, thread: 1, node: 1 });
    }

    #[test]
    fn dies_reach_the_package_only_through_leaf_0x1f() {
        // One package of two dies of four cores, one L3 per die. Leaf 0xB
        // stops at the cores, whose shift is the die's: on its own it
        // would make each die a package.
        let cpuid = Cpuid::new(0x1f, 0x8000_0008)
            .level(0xb, 0, 0, 1, 1)
            .level(0xb, 1, 2, 4, 2)
            .level(0x1f, 0, 0, 1, 1)
            .level(0x1f, 1, 2, 4, 2)
            .level(0x1f, 2, 3, 8, 5)
            .caches(4, 1, 4);
        let layout = cpuid.read(Vendor::Intel);
        assert_eq!(layout, ApicLayout { thread: 0, cache: 2, package: 3 });
        assert_eq!(
            layout.place(5, 0),
            CpuPlace { package: 0, cache: 1, core: 5, thread: 0, node: 0 }
        );
        assert_eq!(layout.place(9, 0).package, 1);
        // Without 0x1F's first level filled in, leaf 0xB is read instead.
        let unfilled = cpuid.with(0x1f, 0, [0, 0, 0, 0]);
        assert_eq!(unfilled.read(Vendor::Intel).package, 2);
    }

    #[test]
    fn zen_core_complexes_share_an_l3_each() {
        // Zen 1, without leaf 0xB: 32 cores of two threads in a package,
        // four cores to a complex and its L3.
        let zen1 = Cpuid::new(0xd, 0x8000_001f)
            .with(0x8000_0001, 0, [0, 0, AMD_TOPOLOGY_EXTENSIONS, 0])
            .with(0x8000_0008, 0, [0, 0, 6 << 12, 0])
            .with(0x8000_001e, 0, [0, 1 << 8, 0, 0])
            .caches(0x8000_001d, 2, 8);
        let layout = zen1.read(Vendor::Amd);
        assert_eq!(layout, ApicLayout { thread: 1, cache: 3, package: 6 });
        assert_eq!(
            layout.place(11, 0),
            CpuPlace { package: 0, cache: 1, core: 5, thread: 1, node: 0 }
        );
        // Zen 3, with it: 64 cores, eight to a complex.
        let zen3 = Cpuid::new(0x10, 0x8000_0023)
            .with(0x8000_0001, 0, [0, 0, AMD_TOPOLOGY_EXTENSIONS, 0])
            .level(0xb, 0, 1, 2, 1)
            .level(0xb, 1, 7, 128, 2)
            .caches(0x8000_001d, 2, 16);
        let layout = zen3.read(Vendor::Amd);
        assert_eq!(layout, ApicLayout { thread: 1, cache: 4, package: 7 });
        assert_eq!(
            layout.place(0x95, 0),
            CpuPlace { package: 1, cache: 1, core: 10, thread: 1, node: 0 }
        );
        // Without the topology extensions there are no cache parameters,
        // and the package is taken as one cache.
        let bare = zen3.with(0x8000_0001, 0, [0, 0, 0, 0]);
        assert_eq!(bare.read(Vendor::Amd).cache, 7);
    }

    #[test]
    fn odd_answers_stay_within_the_core_and_the_package() {
        // An L3 said to be shared by one thread of a two-thread core, or
        // by more than the package holds, as a hypervisor may say.
        let base = Cpuid::new(0xd, 0x8000_0008).level(0xb, 0, 1, 2, 1).level(0xb, 1, 3, 8, 2);
        let small = base.0.clone();
        assert_eq!(Cpuid(small).caches(4, 1, 1).read(Vendor::Intel).cache, 1);
        assert_eq!(base.caches(4, 2, 256).read(Vendor::Intel).cache, 3);
        // Another vendor's: no cache leaves are trusted.
        let other = Cpuid::new(0xd, 0x8000_0008).level(0xb, 0, 1, 2, 1).level(0xb, 1, 3, 8, 2);
        assert_eq!(other.caches(4, 2, 4).read(Vendor::Other).cache, 3);
    }

    #[test]
    fn leaf_1_alone_gives_the_package_only() {
        let cpuid = Cpuid::new(4, 0x8000_0004).with(1, 0, [0, 4 << 16, 0, 1 << 28]);
        let layout = cpuid.read(Vendor::Other);
        assert_eq!(layout, ApicLayout { thread: 0, cache: 2, package: 2 });
        assert_eq!(
            layout.place(6, 0),
            CpuPlace { package: 1, cache: 0, core: 2, thread: 0, node: 0 }
        );
        // Leaves beyond the processor's are not asked for: here a leaf 0xB
        // that would say otherwise, as Intel's give the highest leaf's
        // registers for those.
        let beyond = cpuid.level(0xb, 0, 1, 2, 1).level(0xb, 1, 5, 8, 2);
        assert_eq!(beyond.read(Vendor::Intel).package, 2);
        let none = Cpuid::new(1, 0x8000_0000);
        assert_eq!(none.read(Vendor::Intel), ApicLayout { thread: 0, cache: 0, package: 0 });
        assert_eq!(none.read(Vendor::Intel).place(3, 0).package, 3);
    }
}
