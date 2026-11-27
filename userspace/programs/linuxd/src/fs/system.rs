// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine-wide files of `/proc` and `/sys`, in Linux's text:
//! `meminfo`, `stat`, `loadavg`, `uptime`, `version`, `filesystems`, the
//! names `uname(2)` reports, the identifiers under `sys/kernel/random`
//! and the figures the cgroup root shows.
//!
//! They are made from `/processes/meminfo`, `/processes/stat`,
//! `/processes/loadavg` and `/processes/uptime` (radix-proc(5)), whose
//! keys the functions read by name. A figure the kernel does not keep is
//! 0, as Linux shows it for a machine without the thing.
//!
//! When `free(1)` or `uptime(1)` in a sandbox disagrees with Dukovo's,
//! compare this module's text with the same file under `/processes`.

use super::text::{Page, decimal, number, value};

/// What `uname(2)` and `/proc/sys/kernel/ostype` report as the system's
/// name, the release and the version: the kernel's Linux table's own.
pub use radix_linux::records::{RELEASE, SYSNAME as OSTYPE, VERSION};

/// What a machine is called until it is given a name, as the Linux
/// table's `uname(2)` has it.
const DEFAULT_HOSTNAME: &str = "dukovo";

/// The longest name of a machine, as `uname(2)`'s `nodename` holds.
const HOSTNAME_MAX: usize = 64;

/// A line of Linux's `/proc/meminfo`: the label with its colon, padded
/// to 16 columns, then the figure in 8.
fn kb(page: &mut Page, label: &str, kib: u64) {
    page.line(format_args!("{:<16}{kib:>8} kB", format!("{label}:")));
}

/// `/proc/meminfo`, from the text of `/processes/meminfo`.
pub fn meminfo(source: &[u8]) -> String {
    let get = |key: &str| number(source, key).unwrap_or(0);
    let (total, free) = (get("total"), get("free"));
    let page_cache = get("page_cache");
    let slabs = get("heap_slabs") + get("large_heap_blocks");
    let anonymous = get("memory_objects");
    let swap = (get("swap_total"), get("swap_free"));
    let mut page = Page::new();
    kb(&mut page, "MemTotal", total);
    kb(&mut page, "MemFree", free);
    // What can be had without swapping: the free memory and the page
    // cache, which the kernel gives back when memory runs low.
    kb(&mut page, "MemAvailable", free + page_cache);
    kb(&mut page, "Buffers", 0);
    kb(&mut page, "Cached", page_cache);
    kb(&mut page, "SwapCached", 0);
    for label in ["Active", "Inactive", "Active(anon)", "Inactive(anon)"] {
        kb(&mut page, label, 0);
    }
    for label in ["Active(file)", "Inactive(file)", "Unevictable", "Mlocked"] {
        kb(&mut page, label, 0);
    }
    kb(&mut page, "SwapTotal", swap.0);
    kb(&mut page, "SwapFree", swap.1);
    for label in ["Zswap", "Zswapped", "Dirty", "Writeback"] {
        kb(&mut page, label, 0);
    }
    kb(&mut page, "AnonPages", anonymous);
    kb(&mut page, "Mapped", 0);
    kb(&mut page, "Shmem", 0);
    kb(&mut page, "KReclaimable", 0);
    kb(&mut page, "Slab", slabs);
    kb(&mut page, "SReclaimable", 0);
    kb(&mut page, "SUnreclaim", slabs);
    kb(&mut page, "KernelStack", get("mapped_areas"));
    kb(&mut page, "PageTables", get("page_tables"));
    for label in ["SecPageTables", "NFS_Unstable", "Bounce", "WritebackTmp"] {
        kb(&mut page, label, 0);
    }
    // Linux's default: half the memory, and the swap.
    kb(&mut page, "CommitLimit", total / 2 + swap.0);
    kb(&mut page, "Committed_AS", anonymous);
    kb(&mut page, "VmallocTotal", 34_359_738_367);
    kb(&mut page, "VmallocUsed", 0);
    kb(&mut page, "VmallocChunk", 0);
    kb(&mut page, "Percpu", get("per_cpu_caches"));
    // The one line of Linux's that does not pad its label to 16 columns.
    page.line(format_args!("HardwareCorrupted: {:>5} kB", get("hardware_corrupted")));
    for label in ["AnonHugePages", "ShmemHugePages", "ShmemPmdMapped", "FileHugePages"] {
        kb(&mut page, label, 0);
    }
    for label in ["FilePmdMapped", "CmaTotal", "CmaFree"] {
        kb(&mut page, label, 0);
    }
    for label in ["HugePages_Total", "HugePages_Free", "HugePages_Rsvd", "HugePages_Surp"] {
        page.line(format_args!("{:<19}{:>5}", format!("{label}:"), 0));
    }
    kb(&mut page, "Hugepagesize", 2048);
    kb(&mut page, "Hugetlb", 0);
    kb(&mut page, "DirectMap4k", total);
    kb(&mut page, "DirectMap2M", 0);
    kb(&mut page, "DirectMap1G", 0);
    page.finish()
}

/// Nanoseconds in a tick of Linux's `USER_HZ`, 100 a second.
const NS_PER_TICK: u64 = 10_000_000;

/// The times of one CPU, as `/processes/stat` has them, in ticks.
struct CpuTimes {
    user: u64,
    system: u64,
    idle: u64,
}

/// The `cpuN:` lines of `/processes/stat`: each CPU's number and times.
fn cpu_times(source: &[u8]) -> Vec<(u32, CpuTimes)> {
    let text = String::from_utf8_lossy(source);
    let mut cpus = Vec::new();
    for line in text.lines() {
        let Some((name, fields)) = line.strip_prefix("cpu").and_then(|l| l.split_once(": ")) else {
            continue;
        };
        let Some(number) = decimal(name).and_then(|number| u32::try_from(number).ok()) else {
            continue;
        };
        let ticks = |key: &str| {
            let (_, rest) = fields.split_once(&format!("{key}="))?;
            let ns: u64 = rest.split(' ').next()?.parse().ok()?;
            Some(ns / NS_PER_TICK)
        };
        let times = CpuTimes {
            user: ticks("user_ns").unwrap_or(0),
            system: ticks("system_ns").unwrap_or(0),
            idle: ticks("idle_ns").unwrap_or(0),
        };
        cpus.push((number, times));
    }
    cpus
}

/// `/proc/stat`, from the text of `/processes/stat`, with `processes`,
/// the number of processes the kernel lists, as the count of forks,
/// which it does not keep.
pub fn stat(source: &[u8], processes: usize) -> String {
    let cpus = cpu_times(source);
    let get = |key: &str| number(source, key).unwrap_or(0);
    let mut page = Page::new();
    let sum = |pick: fn(&CpuTimes) -> u64| cpus.iter().map(|(_, times)| pick(times)).sum::<u64>();
    page.line(format_args!(
        "cpu  {} 0 {} {} 0 0 0 0 0 0",
        sum(|t| t.user),
        sum(|t| t.system),
        sum(|t| t.idle)
    ));
    for (number, times) in &cpus {
        page.line(format_args!(
            "cpu{number} {} 0 {} {} 0 0 0 0 0 0",
            times.user, times.system, times.idle
        ));
    }
    page.line(format_args!("intr 0"));
    page.line(format_args!("ctxt {}", get("switches")));
    page.line(format_args!("btime {}", get("boot_time_ns") / 1_000_000_000));
    page.line(format_args!("processes {processes}"));
    page.line(format_args!("procs_running {}", get("running")));
    page.line(format_args!("procs_blocked {}", get("disk_waiting")));
    page.line(format_args!("softirq 0 0 0 0 0 0 0 0 0 0 0"));
    page.finish()
}

/// `/proc/uptime`: the seconds since boot, and the seconds every CPU has
/// idled added up, each with two decimals.
pub fn uptime(uptime: &[u8], stat: &[u8]) -> String {
    let hundredths = |ns: u64| format!("{}.{:02}", ns / 1_000_000_000, ns / 10_000_000 % 100);
    let idle_ticks: u64 = cpu_times(stat).iter().map(|(_, times)| times.idle).sum();
    let up = number(uptime, "uptime_ns").unwrap_or(0);
    format!("{} {}\n", hundredths(up), hundredths(idle_ticks * NS_PER_TICK))
}

/// How many threads the processes have, and the highest process number,
/// for the last two fields of `/proc/loadavg`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Population {
    /// The threads of every process listed.
    pub threads: u64,
    /// The highest process number listed.
    pub newest: u64,
}

/// `/proc/loadavg`: the three averages as `/processes/loadavg` has them,
/// the threads ready to run over all threads, and the newest process.
pub fn loadavg(load: &[u8], stat: &[u8], population: Population) -> String {
    let average = |key: &str| {
        let text = value(load, key).and_then(|v| std::str::from_utf8(v).ok());
        text.unwrap_or("0.00").to_string()
    };
    format!(
        "{} {} {} {}/{} {}\n",
        average("load_1"),
        average("load_5"),
        average("load_15"),
        number(stat, "running").unwrap_or(0),
        population.threads,
        population.newest,
    )
}

/// `/proc/version`.
pub fn version() -> String {
    format!("{OSTYPE} version {RELEASE} {VERSION}\n")
}

/// `/proc/filesystems`: the kinds of filesystem a program may mount or
/// find mounted, by the names Linux gives them, `nodev` for those that
/// are not on a block device.
pub fn filesystems() -> String {
    let mut page = Page::new();
    for name in ["sysfs", "tmpfs", "proc", "devtmpfs", "cgroup2", "fuse"] {
        page.line(format_args!("nodev\t{name}"));
    }
    for name in ["ext4", "btrfs", "vfat", "exfat", "ntfs", "iso9660", "udf"] {
        page.line(format_args!("\t{name}"));
    }
    page.finish()
}

/// `/proc/swaps`: the headings only, as Linux shows a machine with no
/// swap area on. The kernel's swap areas are its own.
pub const SWAPS: &str = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";

/// `/proc/sys/kernel/hostname`: the machine's name, the first line of
/// `/system/settings/hostname`, or the default where it holds no name
/// or one `uname(2)` could not carry.
pub fn hostname(file: Option<&[u8]>) -> String {
    let line = file.and_then(|text| text.split(|&byte| byte == b'\n').next()).unwrap_or_default();
    let end = line.iter().rposition(|&b| !matches!(b, b' ' | b'\t' | b'\r')).map_or(0, |at| at + 1);
    let name = &line[..end];
    match std::str::from_utf8(name) {
        Ok(name) if !name.is_empty() && name.len() <= HOSTNAME_MAX && !name.contains('\0') => {
            format!("{name}\n")
        }
        _ => format!("{DEFAULT_HOSTNAME}\n"),
    }
}

/// `/proc/sys/kernel/threads-max`: as Linux sizes it at boot, a thread
/// for every 32 pages of memory, within its limits.
pub fn threads_max(meminfo: &[u8]) -> String {
    let pages = number(meminfo, "total").unwrap_or(0) / 4;
    format!("{}\n", (pages / 32).clamp(20, 0x3fff_ffff))
}

/// A version 4 UUID made of `bytes`, as Linux writes them.
pub fn uuid(mut bytes: [u8; 16]) -> String {
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}\n",
        hex[..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..].concat()
    )
}

/// `/proc/sys/kernel/random/boot_id`: the same for a whole boot, so it
/// is made from the time the kernel started, which `/processes/stat`
/// gives, and stays as it was when linuxd starts again.
pub fn boot_id(stat: &[u8]) -> String {
    let mut seed = number(stat, "boot_time_ns").unwrap_or(0);
    let mut bytes = [0u8; 16];
    for half in bytes.chunks_mut(8) {
        half.copy_from_slice(&splitmix64(&mut seed).to_le_bytes());
    }
    uuid(bytes)
}

/// The next number of SplitMix64's sequence from `state`.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `memory.current` of the cgroup root: the bytes in use, which are the
/// memory the kernel manages less what is free.
pub fn memory_current(meminfo: &[u8]) -> String {
    let get = |key: &str| number(meminfo, key).unwrap_or(0);
    format!("{}\n", get("total").saturating_sub(get("free")) * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &[u8] = b"total: 4194304 KiB\nfree: 2097152 KiB\nkernel: 100 KiB\n\
        page_tables: 2000 KiB\nheap_slabs: 3000 KiB\nlarge_heap_blocks: 500 KiB\n\
        mapped_areas: 700 KiB\nper_cpu_caches: 40 KiB\nmemory_objects: 600000 KiB\n\
        page_cache: 400000 KiB\nswap_total: 1024 KiB\nswap_free: 1000 KiB\n\
        hardware_corrupted: 0 KiB\n";

    #[test]
    fn meminfo_is_linuxs_with_the_kernels_figures() {
        let text = meminfo(MEMINFO);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "MemTotal:        4194304 kB");
        assert_eq!(lines[1], "MemFree:         2097152 kB");
        assert_eq!(lines[2], "MemAvailable:    2497152 kB");
        assert_eq!(lines[4], "Cached:           400000 kB");
        assert_eq!(lines[5], "SwapCached:            0 kB");
        assert!(text.contains("\nSwapTotal:          1024 kB\nSwapFree:           1000 kB\n"));
        assert!(text.contains("\nSlab:               3500 kB\n"));
        assert!(text.contains("\nPageTables:         2000 kB\n"));
        assert!(text.contains("\nCommitLimit:     2098176 kB\n"));
        assert!(text.contains("\nHugePages_Total:       0\nHugePages_Free:        0\n"));
        assert!(text.contains("\nHugepagesize:       2048 kB\n"));
        assert!(text.ends_with("DirectMap1G:           0 kB\n"));
        assert_eq!(lines.len(), 56);
    }

    #[test]
    fn meminfo_labels_fill_sixteen_columns() {
        let text = meminfo(MEMINFO);
        assert!(text.contains("\nHardwareCorrupted:     0 kB\n"));
        for line in text.lines().filter(|line| !line.starts_with("HardwareCorrupted")) {
            assert!(line.len() >= 24, "{line}");
            assert_eq!(line.as_bytes()[..16].iter().filter(|b| **b == b':').count(), 1, "{line}");
        }
    }

    const STAT: &[u8] =
        b"cpu0: user_ns=1000000000 system_ns=500000000 idle_ns=9000000000 switches=7\n\
        cpu1: user_ns=2000000000 system_ns=100000000 idle_ns=8000000000 switches=9\n\
        cpus: 2\nuser_ns: 3000000000\nsystem_ns: 600000000\nidle_ns: 17000000000\n\
        switches: 16\nrunning: 3\ndisk_waiting: 1\nboot_time_ns: 1700000000123456789\n";

    #[test]
    fn stat_has_a_line_a_cpu_in_ticks() {
        let text = stat(STAT, 42);
        let golden = "cpu  300 0 60 1700 0 0 0 0 0 0
cpu0 100 0 50 900 0 0 0 0 0 0
cpu1 200 0 10 800 0 0 0 0 0 0
intr 0
ctxt 16
btime 1700000000
processes 42
procs_running 3
procs_blocked 1
softirq 0 0 0 0 0 0 0 0 0 0 0
";
        assert_eq!(text, golden);
    }

    #[test]
    fn uptime_adds_the_idle_time_of_every_cpu() {
        assert_eq!(uptime(b"uptime_ns: 123456789012\n", STAT), "123.45 17.00\n");
        assert_eq!(uptime(b"", b""), "0.00 0.00\n");
    }

    #[test]
    fn loadavg_has_linuxs_six_fields() {
        let load = b"load_1: 0.05\nload_5: 0.10\nload_15: 1.25\n";
        let population = Population { threads: 120, newest: 4321 };
        assert_eq!(loadavg(load, STAT, population), "0.05 0.10 1.25 3/120 4321\n");
    }

    #[test]
    fn the_version_names_linux() {
        let text = version();
        let release = text.strip_prefix("Linux version 6.1.0-dukovo #1 SMP Dukovo ");
        assert!(release.is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())), "{text}");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn filesystems_mark_the_ones_with_no_device() {
        let text = filesystems();
        assert!(text.starts_with("nodev\tsysfs\nnodev\ttmpfs\nnodev\tproc\n"));
        assert!(text.contains("\n\text4\n"));
        assert!(text.contains("nodev\tfuse\n"));
    }

    #[test]
    fn a_hostname_is_the_first_line_or_the_default() {
        assert_eq!(hostname(Some(b"workstation \t\nsecond\n")), "workstation\n");
        assert_eq!(hostname(Some(b"\n")), "dukovo\n");
        assert_eq!(hostname(Some(&[b'a'; 65])), "dukovo\n");
        assert_eq!(hostname(Some(&[b'a'; 64])), format!("{}\n", "a".repeat(64)));
        assert_eq!(hostname(Some(b"a\0b")), "dukovo\n");
        assert_eq!(hostname(None), "dukovo\n");
    }

    #[test]
    fn threads_max_follows_the_memory() {
        assert_eq!(threads_max(MEMINFO), "32768\n");
        assert_eq!(threads_max(b"total: 16 KiB\n"), "20\n");
        assert_eq!(threads_max(b""), "20\n");
    }

    #[test]
    fn a_uuid_has_the_version_and_variant_bits() {
        assert_eq!(uuid([0x5a; 16]), "5a5a5a5a-5a5a-4a5a-9a5a-5a5a5a5a5a5a\n");
        let text = uuid([0xff; 16]);
        assert_eq!(&text[14..15], "4");
        assert!(["8", "9", "a", "b"].contains(&&text[19..20]));
    }

    #[test]
    fn the_boot_id_is_the_same_each_time_for_a_boot() {
        let first = boot_id(STAT);
        assert_eq!(first, boot_id(STAT));
        assert_ne!(first, boot_id(b"boot_time_ns: 1700000001000000000\n"));
        assert_eq!(first.len(), 37);
    }

    #[test]
    fn the_cgroup_root_counts_the_memory_in_use() {
        assert_eq!(memory_current(MEMINFO), format!("{}\n", 2097152u64 * 1024));
    }
}
