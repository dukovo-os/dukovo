// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The processor, as `/proc/cpuinfo` and the topology files of
//! `/sys/devices/system/cpu` show it: what CPUID says of the CPU linuxd
//! runs on, and how many CPUs `/kernel/cpus` says the kernel runs.
//!
//! The decoding is the one `lscpu(1)` makes (systools' `lscpu.rs`): the
//! flags are the CPUID bits Linux's `/proc/cpuinfo` names, without the
//! flags Linux's kernel adds of its own (`constant_tsc`, `pti`). linuxd
//! cannot run CPUID on each CPU, so every CPU is the one it asked, and
//! the APIC ids and the topology are made from the numbers CPUID gives
//! for a package: sibling threads have neighbouring numbers.
//!
//! When `/proc/cpuinfo` misleads a program, run `lscpu` on the machine
//! and compare the family, model and flags.

use super::text::Page;

/// Reads CPUID leaf `leaf`, subleaf `sub`: EAX, EBX, ECX and EDX.
pub type Cpuid<'a> = &'a dyn Fn(u32, u32) -> [u32; 4];

/// Names by bit.
type Bits = &'static [(u32, &'static str)];

/// The Linux names of the bits of a CPUID register, in Linux's order of
/// feature words: leaf, subleaf, register (0 EAX to 3 EDX) and the names
/// by bit.
const FLAGS: &[(u32, u32, usize, Bits)] = &[
    (
        1,
        0,
        3,
        &[
            (0, "fpu"),
            (1, "vme"),
            (2, "de"),
            (3, "pse"),
            (4, "tsc"),
            (5, "msr"),
            (6, "pae"),
            (7, "mce"),
            (8, "cx8"),
            (9, "apic"),
            (11, "sep"),
            (12, "mtrr"),
            (13, "pge"),
            (14, "mca"),
            (15, "cmov"),
            (16, "pat"),
            (17, "pse36"),
            (18, "pn"),
            (19, "clflush"),
            (21, "dts"),
            (22, "acpi"),
            (23, "mmx"),
            (24, "fxsr"),
            (25, "sse"),
            (26, "sse2"),
            (27, "ss"),
            (28, "ht"),
            (29, "tm"),
            (30, "ia64"),
            (31, "pbe"),
        ],
    ),
    (
        0x8000_0001,
        0,
        3,
        &[
            (11, "syscall"),
            (19, "mp"),
            (20, "nx"),
            (22, "mmxext"),
            (25, "fxsr_opt"),
            (26, "pdpe1gb"),
            (27, "rdtscp"),
            (29, "lm"),
            (30, "3dnowext"),
            (31, "3dnow"),
        ],
    ),
    (
        1,
        0,
        2,
        &[
            (0, "pni"),
            (1, "pclmulqdq"),
            (2, "dtes64"),
            (3, "monitor"),
            (4, "ds_cpl"),
            (5, "vmx"),
            (6, "smx"),
            (7, "est"),
            (8, "tm2"),
            (9, "ssse3"),
            (10, "cid"),
            (11, "sdbg"),
            (12, "fma"),
            (13, "cx16"),
            (14, "xtpr"),
            (15, "pdcm"),
            (17, "pcid"),
            (18, "dca"),
            (19, "sse4_1"),
            (20, "sse4_2"),
            (21, "x2apic"),
            (22, "movbe"),
            (23, "popcnt"),
            (24, "tsc_deadline_timer"),
            (25, "aes"),
            (26, "xsave"),
            (28, "avx"),
            (29, "f16c"),
            (30, "rdrand"),
            (31, "hypervisor"),
        ],
    ),
    (
        0x8000_0001,
        0,
        2,
        &[
            (0, "lahf_lm"),
            (1, "cmp_legacy"),
            (2, "svm"),
            (3, "extapic"),
            (4, "cr8_legacy"),
            (5, "abm"),
            (6, "sse4a"),
            (7, "misalignsse"),
            (8, "3dnowprefetch"),
            (9, "osvw"),
            (10, "ibs"),
            (11, "xop"),
            (12, "skinit"),
            (13, "wdt"),
            (15, "lwp"),
            (16, "fma4"),
            (17, "tce"),
            (19, "nodeid_msr"),
            (21, "tbm"),
            (22, "topoext"),
            (23, "perfctr_core"),
            (24, "perfctr_nb"),
            (26, "bpext"),
            (27, "ptsc"),
            (28, "perfctr_llc"),
            (29, "mwaitx"),
        ],
    ),
    (
        7,
        0,
        1,
        &[
            (0, "fsgsbase"),
            (1, "tsc_adjust"),
            (2, "sgx"),
            (3, "bmi1"),
            (4, "hle"),
            (5, "avx2"),
            (7, "smep"),
            (8, "bmi2"),
            (9, "erms"),
            (10, "invpcid"),
            (11, "rtm"),
            (12, "cqm"),
            (14, "mpx"),
            (15, "rdt_a"),
            (16, "avx512f"),
            (17, "avx512dq"),
            (18, "rdseed"),
            (19, "adx"),
            (20, "smap"),
            (21, "avx512ifma"),
            (23, "clflushopt"),
            (24, "clwb"),
            (25, "intel_pt"),
            (26, "avx512pf"),
            (27, "avx512er"),
            (28, "avx512cd"),
            (29, "sha_ni"),
            (30, "avx512bw"),
            (31, "avx512vl"),
        ],
    ),
    (0xd, 1, 0, &[(0, "xsaveopt"), (1, "xsavec"), (2, "xgetbv1"), (3, "xsaves"), (4, "xfd")]),
    (7, 1, 0, &[(4, "avx_vnni"), (5, "avx512_bf16")]),
    (
        0x8000_0008,
        0,
        1,
        &[(0, "clzero"), (1, "irperf"), (2, "xsaveerptr"), (4, "rdpru"), (9, "wbnoinvd")],
    ),
    (
        6,
        0,
        0,
        &[
            (0, "dtherm"),
            (1, "ida"),
            (2, "arat"),
            (4, "pln"),
            (6, "pts"),
            (7, "hwp"),
            (8, "hwp_notify"),
            (9, "hwp_act_window"),
            (10, "hwp_epp"),
            (11, "hwp_pkg_req"),
        ],
    ),
    (
        7,
        0,
        2,
        &[
            (1, "avx512vbmi"),
            (2, "umip"),
            (3, "pku"),
            (4, "ospke"),
            (5, "waitpkg"),
            (6, "avx512_vbmi2"),
            (8, "gfni"),
            (9, "vaes"),
            (10, "vpclmulqdq"),
            (11, "avx512_vnni"),
            (12, "avx512_bitalg"),
            (13, "tme"),
            (14, "avx512_vpopcntdq"),
            (16, "la57"),
            (22, "rdpid"),
            (24, "bus_lock_detect"),
            (25, "cldemote"),
            (27, "movdiri"),
            (28, "movdir64b"),
            (29, "enqcmd"),
            (30, "sgx_lc"),
        ],
    ),
    (
        7,
        0,
        3,
        &[
            (2, "avx512_4vnniw"),
            (3, "avx512_4fmaps"),
            (4, "fsrm"),
            (8, "avx512_vp2intersect"),
            (10, "md_clear"),
            (14, "serialize"),
            (16, "tsxldtrk"),
            (18, "pconfig"),
            (19, "arch_lbr"),
            (20, "ibt"),
            (22, "amx_bf16"),
            (23, "avx512_fp16"),
            (24, "amx_tile"),
            (25, "amx_int8"),
            (28, "flush_l1d"),
            (29, "arch_capabilities"),
        ],
    ),
];

/// The 12 characters of three registers, as the vendor and hypervisor
/// names are kept.
fn chars(registers: [u32; 3]) -> String {
    let bytes: Vec<u8> = registers.iter().flat_map(|r| r.to_le_bytes()).collect();
    String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_string()
}

/// One cache: its level and kind, its size, and how many logical CPUs
/// share each one.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Cache {
    level: u32,
    bytes: u64,
    shared_by: u32,
}

/// What `/proc/cpuinfo` and the topology files show of the processor.
#[derive(Debug, PartialEq, Eq)]
pub struct Processor {
    vendor: String,
    model_name: Option<String>,
    family: u32,
    model: u32,
    stepping: u32,
    /// The highest CPUID leaf.
    level: u32,
    /// Logical CPUs a core runs.
    threads_per_core: u32,
    /// Logical CPUs a package holds.
    per_package: u32,
    /// The bytes a cache line holds (CLFLUSH's size).
    line: u32,
    address_sizes: Option<(u32, u32)>,
    flags: Vec<&'static str>,
    caches: Vec<Cache>,
    /// The clock the processor is rated at, in MHz, where CPUID or its
    /// name says.
    base_mhz: Option<u32>,
}

/// The caches leaf `leaf` (4 on Intel, 0x8000001d on AMD) describes,
/// one subleaf each. The instruction cache of a level is not counted
/// apart: Linux's `cache size` is the last level's.
fn deterministic_caches(cpuid: Cpuid<'_>, leaf: u32) -> Vec<Cache> {
    let mut caches = Vec::new();
    for sub in 0..16 {
        let [a, b, c, _] = cpuid(leaf, sub);
        let level = match (a & 0x1f, (a >> 5) & 7) {
            (0, _) => break,
            (1 | 2, 1) => 1,
            (_, level @ 2..=4) => level,
            _ => continue,
        };
        let ways = u64::from((b >> 22) + 1);
        let partitions = u64::from(((b >> 12) & 0x3ff) + 1);
        let line = u64::from((b & 0xfff) + 1);
        let sets = u64::from(c) + 1;
        let shared_by = ((a >> 14) & 0xfff) + 1;
        caches.push(Cache { level, bytes: ways * partitions * line * sets, shared_by });
    }
    caches
}

/// AMD's older cache leaves, which say nothing of sharing: each core has
/// its own L1 and L2, and the package one L3.
fn amd_legacy_caches(cpuid: Cpuid<'_>, threads: u32, package: u32) -> Vec<Cache> {
    let [_, _, l1d, l1i] = cpuid(0x8000_0005, 0);
    let [_, _, l2, l3] = cpuid(0x8000_0006, 0);
    let kib = |k: u32| u64::from(k) * 1024;
    let mut caches = Vec::new();
    for (level, bytes, shared_by) in [
        (1, kib(l1d >> 24), threads),
        (1, kib(l1i >> 24), threads),
        (2, kib(l2 >> 16), threads),
        (3, kib(l3 >> 18) * 512, package),
    ] {
        if bytes > 0 {
            caches.push(Cache { level, bytes, shared_by });
        }
    }
    caches
}

/// Logical CPUs a core runs, and a package holds: leaf 0xb's levels
/// where it has them, else AMD's extended leaves, else leaf 1's count.
fn topology(cpuid: Cpuid<'_>, misc: u32, features: u32, amd: bool, topoext: bool) -> (u32, u32) {
    let (mut threads, mut package) = (0, 0);
    for sub in 0..8 {
        let [_, b, c, _] = cpuid(0xb, sub);
        match (c >> 8) & 0xff {
            0 => break,
            1 => threads = b & 0xffff,
            2 => package = b & 0xffff,
            _ => {}
        }
    }
    if threads > 0 && package >= threads {
        return (threads, package);
    }
    if amd {
        let cores = (cpuid(0x8000_0008, 0)[2] & 0xff) + 1;
        let threads = if topoext { ((cpuid(0x8000_001e, 0)[1] >> 8) & 0xff) + 1 } else { 1 };
        return (threads, cores.max(threads));
    }
    let package = if features & (1 << 28) != 0 { ((misc >> 16) & 0xff).max(1) } else { 1 };
    (1, package)
}

/// The processor's brand string, from CPUID leaves 0x80000002 to
/// 0x80000004, with its runs of spaces made one, if it has one.
fn model_name(cpuid: Cpuid<'_>) -> Option<String> {
    let [max_ext, ..] = cpuid(0x8000_0000, 0);
    if max_ext < 0x8000_0004 {
        return None;
    }
    let mut bytes = Vec::new();
    for leaf in 0x8000_0002..=0x8000_0004 {
        bytes.extend(cpuid(leaf, 0).iter().flat_map(|r| r.to_le_bytes()));
    }
    let text = String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_string();
    Some(text.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|name| !name.is_empty())
}

/// The clock a brand string ends with, as `@ 1.80GHz`, in MHz: the base
/// clock of a processor that has no leaf 0x16.
fn named_mhz(brand: &str) -> Option<u32> {
    let (_, clock) = brand.rsplit_once(" @ ")?;
    let ghz: f64 = clock.trim().strip_suffix("GHz")?.parse().ok()?;
    Some((ghz * 1000.0).round() as u32).filter(|&mhz| mhz > 0)
}

impl Processor {
    /// What `cpuid` says of the processor.
    pub fn read(cpuid: Cpuid<'_>) -> Processor {
        let [max, b, c, d] = cpuid(0, 0);
        let vendor = chars([b, d, c]);
        let [max_ext, ..] = cpuid(0x8000_0000, 0);
        let leaf = |leaf: u32, sub: u32| -> [u32; 4] {
            let present = if leaf >= 0x8000_0000 { leaf <= max_ext } else { leaf <= max };
            if present { cpuid(leaf, sub) } else { [0; 4] }
        };
        let [signature, misc, features, _] = leaf(1, 0);
        let base_family = (signature >> 8) & 0xf;
        let family =
            if base_family == 0xf { base_family + ((signature >> 20) & 0xff) } else { base_family };
        let mut model = (signature >> 4) & 0xf;
        if base_family == 6 || base_family == 0xf {
            model |= ((signature >> 16) & 0xf) << 4;
        }
        let model_name = model_name(cpuid);
        let mut flags = Vec::new();
        for &(l, sub, register, names) in FLAGS {
            let value = leaf(l, sub)[register];
            flags.extend(names.iter().filter(|(bit, _)| value & (1 << bit) != 0).map(|(_, n)| *n));
        }
        let amd = vendor == "AuthenticAMD" || vendor == "HygonGenuine";
        let topoext = leaf(0x8000_0001, 0)[2] & (1 << 22) != 0;
        let (threads_per_core, per_package) = topology(&leaf, misc, features, amd, topoext);
        let caches = if amd && topoext {
            deterministic_caches(&leaf, 0x8000_001d)
        } else if !amd && max >= 4 {
            deterministic_caches(&leaf, 4)
        } else {
            amd_legacy_caches(&leaf, threads_per_core, per_package)
        };
        let [sizes, ..] = leaf(0x8000_0008, 0);
        // Only Intel's processors define leaf 0x16: the base clock in its
        // low 16 bits.
        let rated =
            if max >= 0x16 && vendor == "GenuineIntel" { leaf(0x16, 0)[0] & 0xffff } else { 0 };
        let base_mhz =
            Some(rated).filter(|&mhz| mhz > 0).or(model_name.as_deref().and_then(named_mhz));
        Processor {
            vendor,
            model_name,
            family,
            model,
            stepping: signature & 0xf,
            level: max,
            threads_per_core: threads_per_core.max(1),
            per_package: per_package.max(1),
            line: ((misc >> 8) & 0xff) * 8,
            address_sizes: (sizes != 0).then_some((sizes & 0xff, (sizes >> 8) & 0xff)),
            flags,
            caches,
            base_mhz,
        }
    }

    /// The cores of a package.
    fn cores(&self) -> u32 {
        (self.per_package / self.threads_per_core).max(1)
    }

    /// Where logical CPU `cpu` sits: its package, its core in the
    /// package, and the first CPU of its core.
    fn place(&self, cpu: u32) -> Place {
        let package = cpu / self.per_package;
        let in_package = cpu % self.per_package;
        let core = in_package / self.threads_per_core;
        let first = package * self.per_package + core * self.threads_per_core;
        Place { package, core, first }
    }

    /// The size of the last level of cache, in KiB, which Linux's
    /// `cache size` shows.
    fn cache_kib(&self) -> Option<u64> {
        self.caches.iter().rfind(|cache| cache.level > 1).map(|cache| cache.bytes / 1024)
    }
}

/// A logical CPU's place in the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// Its package, which Linux's `physical_package_id` numbers.
    pub package: u32,
    /// Its core, numbered within the package.
    pub core: u32,
    /// The first logical CPU of its core.
    pub first: u32,
}

/// The CPUs `first` to `last` as Linux lists them: `3`, or `0-7`.
fn range(first: u32, last: u32) -> String {
    if first == last { first.to_string() } else { format!("{first}-{last}") }
}

/// The topology files of CPU `cpu` of `cpus`.
#[derive(Debug, PartialEq, Eq)]
pub struct Topology {
    /// `core_id`.
    pub core_id: String,
    /// `physical_package_id`.
    pub physical_package_id: String,
    /// `thread_siblings_list`: the threads of its core.
    pub thread_siblings_list: String,
    /// `core_siblings_list`: the CPUs of its package.
    pub core_siblings_list: String,
}

impl Processor {
    /// The topology files of CPU `cpu` on a machine of `cpus` CPUs.
    pub fn topology_of(&self, cpu: u32, cpus: u32) -> Topology {
        let place = self.place(cpu);
        let last_thread = (place.first + self.threads_per_core - 1).min(cpus - 1);
        let package_first = place.package * self.per_package;
        let package_last = (package_first + self.per_package - 1).min(cpus - 1);
        Topology {
            core_id: format!("{}\n", place.core),
            physical_package_id: format!("{}\n", place.package),
            thread_siblings_list: format!("{}\n", range(place.first, last_thread)),
            core_siblings_list: format!("{}\n", range(package_first, package_last)),
        }
    }

    /// `/proc/cpuinfo` of a machine of `cpus` CPUs.
    pub fn cpuinfo(&self, cpus: u32) -> String {
        let mut page = Page::new();
        let name = self.model_name.as_deref().unwrap_or("unknown");
        let mut flags = String::new();
        for flag in &self.flags {
            flags.push(' ');
            flags.push_str(flag);
        }
        let khz = u64::from(self.base_mhz.unwrap_or(0)) * 1000;
        // Linux's bogomips is twice the clock in MHz; here in hundredths.
        let bogo = khz / 5;
        for cpu in 0..cpus {
            let place = self.place(cpu);
            page.line(format_args!("processor\t: {cpu}"));
            page.line(format_args!("vendor_id\t: {}", self.vendor));
            page.line(format_args!("cpu family\t: {}", self.family));
            page.line(format_args!("model\t\t: {}", self.model));
            page.line(format_args!("model name\t: {name}"));
            page.line(format_args!("stepping\t: {}", self.stepping));
            page.line(format_args!("cpu MHz\t\t: {}.{:03}", khz / 1000, khz % 1000));
            if let Some(kib) = self.cache_kib() {
                page.line(format_args!("cache size\t: {kib} KB"));
            }
            page.line(format_args!("physical id\t: {}", place.package));
            page.line(format_args!("siblings\t: {}", self.per_package));
            page.line(format_args!("core id\t\t: {}", place.core));
            page.line(format_args!("cpu cores\t: {}", self.cores()));
            page.line(format_args!("apicid\t\t: {cpu}"));
            page.line(format_args!("initial apicid\t: {cpu}"));
            page.line(format_args!("fpu\t\t: yes"));
            page.line(format_args!("fpu_exception\t: yes"));
            page.line(format_args!("cpuid level\t: {}", self.level));
            page.line(format_args!("wp\t\t: yes"));
            page.line(format_args!("flags\t\t:{flags}"));
            page.line(format_args!("bugs\t\t:"));
            page.line(format_args!("bogomips\t: {}.{:02}", bogo / 100, bogo % 100));
            page.line(format_args!("clflush size\t: {}", self.line));
            page.line(format_args!("cache_alignment\t: {}", self.line));
            if let Some((physical, virtual_)) = self.address_sizes {
                page.line(format_args!(
                    "address sizes\t: {physical} bits physical, {virtual_} bits virtual"
                ));
            }
            page.line(format_args!("power management:"));
            page.line(format_args!(""));
        }
        page.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registers of `text`'s first 12 bytes, as CPUID names a vendor.
    fn registers(text: &str) -> [u32; 3] {
        let bytes = text.as_bytes();
        let word = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        [word(0), word(1), word(2)]
    }

    /// The brand string's 48 bytes, as leaves 0x80000002 to 4 give them.
    fn brand(text: &str, leaf: u32) -> [u32; 4] {
        let mut bytes = text.as_bytes().to_vec();
        bytes.resize(48, 0);
        let at = (leaf - 0x8000_0002) as usize * 16;
        let word =
            |i: usize| u32::from_le_bytes(bytes[at + i * 4..at + i * 4 + 4].try_into().unwrap());
        [word(0), word(1), word(2), word(3)]
    }

    /// The Dell's Core i7-8550U: 4 cores of 2 threads, as Linux shows it.
    fn kaby_lake(leaf: u32, sub: u32) -> [u32; 4] {
        let [b, d, c] = registers("GenuineIntel");
        match (leaf, sub) {
            (0, _) => [0x16, b, c, d],
            // Family 6, model 0x8e, stepping 10; 16 logical, HTT.
            (1, _) => [0x0008_06ea, 0x0010_0800, 0x7ffa_fbbf, 0xbfeb_fbff],
            (4, 0) => [0x1c00_4121, 0x01c0_003f, 0x3f, 0],
            (4, 1) => [0x1c00_4122, 0x01c0_003f, 0x3f, 0],
            (4, 2) => [0x1c00_4143, 0x00c0_003f, 0x3ff, 0],
            (4, 3) => [0x1c03_c163, 0x03c0_003f, 0x1fff, 6],
            (4, _) => [0; 4],
            (7, 0) => [0, 0x029c_67af, 0, 0x9c00_0400],
            (0xb, 0) => [1, 2, 0x100, 0],
            (0xb, 1) => [4, 8, 0x201, 0],
            (0x8000_0000, _) => [0x8000_0008, 0, 0, 0],
            (0x8000_0001, _) => [0, 0, 0x121, 0x2c10_0800],
            (l @ 0x8000_0002..=0x8000_0004, _) => {
                brand("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz", l)
            }
            (0x8000_0008, _) => [0x3027, 0, 0, 0],
            (0x16, _) => [1800, 4000, 100, 0],
            _ => [0; 4],
        }
    }

    /// QEMU's default CPU under KVM on an AMD host: no brand string
    /// beyond QEMU's, leaf 0xb, AMD's cache leaves without topoext.
    fn qemu_amd(leaf: u32, sub: u32) -> [u32; 4] {
        let [b, d, c] = registers("AuthenticAMD");
        match (leaf, sub) {
            (0, _) => [0xd, b, c, d],
            (1, _) => [0x0006_0fb1, 0x0002_0800, 0x80b8_2203, 0x078b_fbfd],
            (0xb, _) => [0; 4],
            (0x4000_0000, _) => {
                let [b, c, d] = registers("KVMKVMKVM\0\0\0");
                [0x4000_0001, b, c, d]
            }
            (0x8000_0000, _) => [0x8000_000a, 0, 0, 0],
            (0x8000_0001, _) => [0, 0, 0x0000_0065, 0x2193_fbfd],
            (l @ 0x8000_0002..=0x8000_0004, _) => brand("QEMU Virtual CPU version 2.5+", l),
            (0x8000_0005, _) => [0, 0, 0x4002_0140, 0x4002_0140],
            (0x8000_0006, _) => [0, 0, 0x0200_4140, 0x0080_8140],
            (0x8000_0008, _) => [0x3028, 0, 1, 0],
            _ => [0; 4],
        }
    }

    #[test]
    fn a_laptop_is_read_from_cpuid() {
        let p = Processor::read(&kaby_lake);
        assert_eq!(p.vendor, "GenuineIntel");
        assert_eq!(p.model_name.as_deref(), Some("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz"));
        assert_eq!((p.family, p.model, p.stepping), (6, 142, 10));
        assert_eq!((p.threads_per_core, p.per_package), (2, 8));
        assert_eq!(p.address_sizes, Some((39, 48)));
    }

    #[test]
    fn cpuinfo_is_linuxs_for_each_cpu() {
        let text = Processor::read(&kaby_lake).cpuinfo(2);
        let blocks: Vec<&str> = text.split_inclusive("\n\n").collect();
        assert_eq!(blocks.len(), 2);
        let (flags, rest): (Vec<&str>, Vec<&str>) =
            blocks[1].lines().partition(|line| line.starts_with("flags"));
        let rest = rest.join("\n");
        let golden = "processor\t: 1
vendor_id\t: GenuineIntel
cpu family\t: 6
model\t\t: 142
model name\t: Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz
stepping\t: 10
cpu MHz\t\t: 1800.000
cache size\t: 8192 KB
physical id\t: 0
siblings\t: 8
core id\t\t: 0
cpu cores\t: 4
apicid\t\t: 1
initial apicid\t: 1
fpu\t\t: yes
fpu_exception\t: yes
cpuid level\t: 22
wp\t\t: yes
bugs\t\t:
bogomips\t: 3600.00
clflush size\t: 64
cache_alignment\t: 64
address sizes\t: 39 bits physical, 48 bits virtual
power management:
";
        assert_eq!(rest.trim_end(), golden.trim_end());
        assert!(
            flags[0].starts_with("flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge")
        );
        assert!(flags[0].contains(" syscall nx "));
    }

    #[test]
    fn a_cpu_is_in_the_core_its_threads_share() {
        let p = Processor::read(&kaby_lake);
        let topology = |cpu| p.topology_of(cpu, 8);
        assert_eq!(
            topology(3),
            Topology {
                core_id: "1\n".into(),
                physical_package_id: "0\n".into(),
                thread_siblings_list: "2-3\n".into(),
                core_siblings_list: "0-7\n".into(),
            }
        );
        assert_eq!(topology(0).thread_siblings_list, "0-1\n");
        assert_eq!(topology(7).core_id, "3\n");
    }

    #[test]
    fn a_machine_with_fewer_cpus_than_the_package_holds_ends_the_lists_there() {
        let p = Processor::read(&kaby_lake);
        let topology = p.topology_of(0, 3);
        assert_eq!(topology.core_siblings_list, "0-2\n");
        assert_eq!(p.topology_of(2, 3).thread_siblings_list, "2\n");
    }

    #[test]
    fn a_virtual_machine_has_a_thread_to_each_core() {
        let p = Processor::read(&qemu_amd);
        assert_eq!(p.vendor, "AuthenticAMD");
        assert_eq!((p.family, p.model, p.stepping), (15, 107, 1));
        assert_eq!((p.threads_per_core, p.per_package), (1, 2));
        assert_eq!(p.topology_of(1, 2).core_siblings_list, "0-1\n");
        assert_eq!(p.topology_of(1, 2).thread_siblings_list, "1\n");
        let text = p.cpuinfo(1);
        assert!(text.contains("model name\t: QEMU Virtual CPU version 2.5+\n"));
        assert!(text.contains("hypervisor"));
        // Nothing says the clock of this processor, which the line shows
        // as none.
        assert!(text.contains("cpu MHz\t\t: 0.000\n"));
    }
}
