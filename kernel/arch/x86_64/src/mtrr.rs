// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firmware's memory type range registers (MTRRs), read once at boot
//! to say how the processor caches RAM.
//!
//! The memory type the processor uses for a page is the page attribute
//! table's type combined with the MTRRs' type for its physical address,
//! and an uncached MTRR range stays uncached whatever the page tables
//! ask for. Firmware programs the MTRRs to cover RAM as write-back; one
//! that leaves part of RAM out (too few variable ranges for the memory
//! installed, or a range set wrongly) makes every page there run
//! uncached, many times slower. Linux warns of it ("CPU MTRRs don't
//! cover all of memory") and leaves such RAM out. The boot log's
//! `Memory types:` lines show what the firmware set and name any RAM
//! that is not write-back. Virtual machines show nothing useful here:
//! the hypervisor decides how the host caches a guest's RAM.

use radix_hal::MemoryKind;
use radix_klog::{info, warn};

use crate::cpu::{self, cpuid, rdmsr};
use crate::memory;

/// IA32_MTRRCAP: how many variable ranges there are, in bits 0 to 7.
const MTRRCAP: u32 = 0xfe;
/// IA32_MTRR_DEF_TYPE: the type of what no range covers, and whether the
/// MTRRs are on at all (bit 11).
const DEF_TYPE: u32 = 0x2ff;
/// IA32_MTRR_PHYSBASE0; each range's base and mask follow in pairs.
const PHYSBASE0: u32 = 0x200;

const DEF_TYPE_ENABLED: u64 = 1 << 11;
const MASK_VALID: u64 = 1 << 11;

/// The variable ranges the boot log lists, at most; Intel's processors
/// have 8 or 10.
const RANGES_MAX: usize = 16;

/// A memory type, as the MTRRs and the page attribute table encode it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Kind(u8);

impl Kind {
    const UNCACHED: Kind = Kind(0);
    const WRITE_THROUGH: Kind = Kind(4);
    const WRITE_BACK: Kind = Kind(6);

    fn name(self) -> &'static str {
        match self.0 {
            0 => "uncached",
            1 => "write-combining",
            4 => "write-through",
            5 => "write-protected",
            6 => "write-back",
            _ => "reserved",
        }
    }
}

/// A variable range: the addresses whose bits under `mask` equal `base`'s.
#[derive(Clone, Copy)]
struct Range {
    base: u64,
    mask: u64,
    kind: Kind,
}

impl Range {
    fn contains(&self, address: u64) -> bool {
        address & self.mask == self.base & self.mask
    }

    /// The range's end, for a range of contiguous addresses (every mask
    /// firmware writes in practice): its lowest address plus its size.
    fn end(&self, address_mask: u64) -> u64 {
        (self.base & self.mask).saturating_add((!self.mask & address_mask) + 1)
    }
}

/// The MTRRs as the firmware left them.
struct Mtrrs {
    enabled: bool,
    default: Kind,
    ranges: [Option<Range>; RANGES_MAX],
    address_mask: u64,
}

impl Mtrrs {
    /// The type of `address`, by Intel's rules for overlapping ranges
    /// (SDM volume 3, 12.11.4.1): uncached wins, write-through wins over
    /// write-back, and any other mix is undefined, taken here as
    /// uncached.
    fn kind_at(&self, address: u64) -> Kind {
        if !self.enabled {
            return Kind::UNCACHED;
        }
        let mut found: Option<Kind> = None;
        for range in self.ranges.iter().flatten().filter(|range| range.contains(address)) {
            found = Some(match found {
                None => range.kind,
                Some(kind) => overlap(kind, range.kind),
            });
        }
        found.unwrap_or(self.default)
    }

    /// Calls `each` with the pieces of `start..end` and their types: the
    /// range split wherever a variable range begins or ends.
    fn pieces(&self, start: u64, end: u64, each: &mut dyn FnMut(u64, u64, Kind)) {
        let mut at = start;
        while at < end {
            let mut next = end;
            for range in self.ranges.iter().flatten() {
                for edge in [range.base & range.mask, range.end(self.address_mask)] {
                    if edge > at && edge < next {
                        next = edge;
                    }
                }
            }
            each(at, next, self.kind_at(at));
            at = next;
        }
    }
}

/// The type of an address two ranges of `a` and `b` both cover.
fn overlap(a: Kind, b: Kind) -> Kind {
    let write_through_and_back =
        [a, b].contains(&Kind::WRITE_THROUGH) && [a, b].contains(&Kind::WRITE_BACK);
    if a == b {
        a
    } else if write_through_and_back {
        Kind::WRITE_THROUGH
    } else {
        Kind::UNCACHED
    }
}

/// Reads the MTRRs, if the processor has them and runs on its own.
fn read() -> Option<Mtrrs> {
    if cpu::feature::hypervisor() || cpuid(1, 0).edx & (1 << 12) == 0 {
        return None;
    }
    let bits =
        if cpu::max_extended_leaf() >= 0x8000_0008 { cpuid(0x8000_0008, 0).eax & 0xff } else { 36 };
    let address_mask = (1u64 << bits) - 1;
    // SAFETY: CPUID says the processor has MTRRs, so MTRRCAP, DEF_TYPE
    // and as many base and mask pairs as MTRRCAP counts exist; reading
    // them changes nothing.
    let (cap, def_type) = unsafe { (rdmsr(MTRRCAP), rdmsr(DEF_TYPE)) };
    let count = ((cap & 0xff) as usize).min(RANGES_MAX);
    let mut ranges = [None; RANGES_MAX];
    for (index, slot) in ranges.iter_mut().enumerate().take(count) {
        let msr = PHYSBASE0 + 2 * index as u32;
        // SAFETY: as above, for a pair MTRRCAP counts.
        let (base, mask) = unsafe { (rdmsr(msr), rdmsr(msr + 1)) };
        if mask & MASK_VALID != 0 {
            *slot = Some(Range {
                base: base & address_mask & !0xfff,
                mask: mask & address_mask & !0xfff,
                kind: Kind((base & 0xff) as u8),
            });
        }
    }
    Some(Mtrrs {
        enabled: def_type & DEF_TYPE_ENABLED != 0,
        default: Kind((def_type & 0xff) as u8),
        ranges,
        address_mask,
    })
}

/// Logs the MTRRs and warns of any RAM above the first MiB that they do
/// not make write-back.
pub fn report() {
    let Some(mtrrs) = read() else { return };
    let used = mtrrs.ranges.iter().flatten().count();
    info!(
        "Memory types: MTRRs {}, default {}, {used} variable range{}",
        if mtrrs.enabled { "on" } else { "off" },
        mtrrs.default.name(),
        if used == 1 { "" } else { "s" }
    );
    for range in mtrrs.ranges.iter().flatten() {
        info!(
            "  {:#014x}-{:#014x} {}",
            range.base & range.mask,
            range.end(mtrrs.address_mask) - 1,
            range.kind.name()
        );
    }
    for region in memory::memory_map() {
        if region.kind != MemoryKind::Usable || region.end <= 0x10_0000 {
            continue;
        }
        mtrrs.pieces(region.start.max(0x10_0000), region.end, &mut |start, end, kind| {
            if kind != Kind::WRITE_BACK {
                warn!(
                    "Memory types: RAM {start:#x}-{:#x} ({} MiB) is {} by the firmware's MTRRs",
                    end - 1,
                    (end - start) >> 20,
                    kind.name()
                );
            }
        });
    }
}
