// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel command line.
//!
//! GRUB passes everything after the kernel path on its `multiboot2` line.
//! Flags are separated by spaces and are either a bare name (`kdb`) or
//! `name=value` (`loglevel=debug`). Unknown flags are reported and
//! otherwise ignored. A lone `--` ends the flags: the words after it are
//! the first program's arguments ([`CommandLine::init_args`]), as on
//! Linux. Every flag in [`FLAGS`] is documented in dukovo-boot(7); `ob
//! docs --check` enforces that.

/// One flag the kernel understands.
#[derive(Clone, Copy, Debug)]
pub struct Flag {
    /// Name before any `=`.
    pub name: &'static str,
    /// Placeholder for the value, or `None` for a bare flag.
    pub value: Option<&'static str>,
    /// One-line description.
    pub summary: &'static str,
}

/// Every flag the kernel understands.
pub const FLAGS: &[Flag] = &[
    Flag {
        name: "loglevel",
        value: Some("level"),
        summary: "lowest message level shown on the consoles: error, warn, info or debug",
    },
    Flag { name: "kdb", value: None, summary: "start the kernel debugger on the debug console" },
    Flag {
        name: "dbc",
        value: None,
        summary: "put the console on the first USB controller's Debug Capability, for a debug host",
    },
    Flag { name: "trace", value: None, summary: "record trace points from the start of boot" },
    Flag {
        name: "panic",
        value: Some("seconds"),
        summary: "restart this many seconds after a panic instead of waiting for a key",
    },
    Flag {
        name: "quiet",
        value: None,
        summary: "show the startup screen in place of the log until the console starts; Esc shows the log",
    },
    Flag { name: "nosmp", value: None, summary: "start only the boot CPU" },
    Flag {
        name: "mitigations",
        value: Some("mode"),
        summary: "how the kernel closes the processor's speculative-execution flaws: auto, off (only what another flag asks for with on, ibrs or ibpb) or auto,nosmt",
    },
    Flag {
        name: "nosmt",
        value: None,
        summary: "keep the second hardware thread of each core off",
    },
    Flag {
        name: "nospectre_v1",
        value: None,
        summary: "leave out the interrupt entry's fence after its SWAPGS choice",
    },
    Flag {
        name: "spectre_v2",
        value: Some("mode"),
        summary: "how programs are kept from steering the kernel's indirect branches: auto, off, on, ibrs or retpoline",
    },
    Flag { name: "nospectre_v2", value: None, summary: "same as spectre_v2=off" },
    Flag {
        name: "spectre_v2_user",
        value: Some("mode"),
        summary: "whether programs are kept from steering each other's indirect branches: auto, off or on",
    },
    Flag {
        name: "spec_store_bypass_disable",
        value: Some("mode"),
        summary: "whether every load waits for older stores' addresses (SSBD): auto (off), off or on",
    },
    Flag {
        name: "nospec_store_bypass_disable",
        value: None,
        summary: "same as spec_store_bypass_disable=off",
    },
    Flag {
        name: "retbleed",
        value: Some("mode"),
        summary: "how returns are kept from being predicted from the branch target buffer: auto, off, ibpb or unret",
    },
    Flag {
        name: "spec_rstack_overflow",
        value: Some("mode"),
        summary: "how AMD's return stack overflow is closed: auto, off, ibpb or safe-ret",
    },
    Flag {
        name: "indirect_target_selection",
        value: Some("mode"),
        summary: "keep Intel's indirect branches and returns out of the lower half of a cache line: on or off",
    },
    Flag {
        name: "pti",
        value: Some("mode"),
        summary: "unmap the kernel from programs' page tables while they run, against Meltdown: auto, off or on",
    },
    Flag { name: "nopti", value: None, summary: "same as pti=off" },
    Flag {
        name: "tsx",
        value: Some("mode"),
        summary: "whether Intel's transactional memory is on: auto (off where the CPU can turn it off), off or on",
    },
    Flag {
        name: "mds",
        value: Some("mode"),
        summary: "clear the CPU's buffers on return to a program, against microarchitectural data sampling: auto or off",
    },
    Flag {
        name: "tsx_async_abort",
        value: Some("mode"),
        summary: "clear the CPU's buffers against TSX asynchronous abort: auto or off",
    },
    Flag {
        name: "mmio_stale_data",
        value: Some("mode"),
        summary: "clear the CPU's buffers against stale data from device memory: auto or off",
    },
    Flag {
        name: "reg_file_data_sampling",
        value: Some("mode"),
        summary: "clear the CPU's register file against register file data sampling: auto or off",
    },
    Flag {
        name: "tsa",
        value: Some("mode"),
        summary: "clear the CPU's buffers against AMD's transient scheduler attacks: auto or off",
    },
    Flag {
        name: "srbds",
        value: Some("mode"),
        summary: "protect the random number instructions against special register buffer data sampling: auto or off",
    },
    Flag {
        name: "gather_data_sampling",
        value: Some("mode"),
        summary: "protect vector gathers against gather data sampling: auto or off",
    },
    Flag {
        name: "nokaslr",
        value: None,
        summary: "keep the kernel, the direct map and the mapping window at fixed addresses",
    },
    Flag {
        name: "norandmaps",
        value: None,
        summary: "put programs' stacks, images and anonymous memory in the same places every boot",
    },
    Flag {
        name: "nopcid",
        value: None,
        summary: "do not tag TLB entries with process-context identifiers (PCIDs)",
    },
    Flag {
        name: "noinvpcid",
        value: None,
        summary: "do not flush a PCID's TLB entries with INVPCID, even if the CPU has it",
    },
    Flag {
        name: "nox2apic",
        value: None,
        summary: "drive the local APIC in xAPIC mode even if x2APIC is available",
    },
    Flag { name: "mce", value: Some("off"), summary: "do not turn machine-check reporting on" },
    Flag {
        name: "usb",
        value: Some("off"),
        summary: "leave the USB controllers as the firmware left them",
    },
    Flag {
        name: "ups",
        value: Some("show"),
        summary: "show the UPSes plugged in by USB, but do not shut the machine down when one's \
                  battery is low",
    },
    Flag {
        name: "aml",
        value: Some("off"),
        summary: "run none of the firmware's AML: no uACPI, and the kernel hears the ACPI power \
                  button itself",
    },
    Flag {
        name: "idle",
        value: Some("mode"),
        summary: "how idle CPUs wait: halt keeps them in C1, with HLT; nomwait uses no MWAIT, \
                  so C1 is HLT and deeper states are the firmware's I/O-port ones",
    },
    Flag {
        name: "max_cstate",
        value: Some("count"),
        summary: "the most idle states a CPU uses, C1 first: 1 keeps it in C1",
    },
    Flag {
        name: "pstate",
        value: Some("off"),
        summary: "leave the CPUs' speed to the firmware, though they have Intel's HWP or AMD's CPPC",
    },
    Flag {
        name: "hybrid",
        value: Some("off"),
        summary: "place threads on performance and efficient cores alike, as on a processor whose \
                  cores are all of one kind",
    },
    Flag {
        name: "epp",
        value: Some("preference"),
        summary: "the energy preference the CPUs are asked for: performance, balance_performance, \
                  balance_power or power",
    },
    Flag {
        name: "iommu",
        value: Some("off"),
        summary: "leave the IOMMU's DMA remapping off, so that every device reaches all memory",
    },
    Flag {
        name: "mce-poll",
        value: Some("seconds"),
        summary: "read the CPUs' machine-check banks this often (300 by default)",
    },
    Flag {
        name: "ip",
        value: Some("address/prefix"),
        summary: "give the first network card this IPv4 address, such as 10.0.2.15/24",
    },
    Flag {
        name: "gateway",
        value: Some("address"),
        summary: "send IPv4 packets for other networks through this router",
    },
    Flag {
        name: "e1000",
        value: Some("driver"),
        summary: "program leaves the Intel e1000 and e1000e network cards that an IOMMU \
                  isolates to a driver program, e1000d; kernel, the default, drives them in \
                  the kernel",
    },
    Flag {
        name: "font",
        value: Some("size"),
        summary: "framebuffer console font: 8x16, 12x24 or 16x32",
    },
    Flag {
        name: "memtest",
        value: Some("once|fault"),
        summary: "test the free memory once before the first program, and take the pages \
                  with errors out of use; fault plants one error, for the boot tests",
    },
    Flag {
        name: "crashlog",
        value: Some("off"),
        summary: "do not keep the log in memory for the next boot",
    },
    Flag {
        name: "crashdump",
        value: Some("off"),
        summary: "write no crash dump when the kernel panics (ADR 0483)",
    },
    Flag {
        name: "selftest",
        value: Some("name"),
        summary: "run a self-test at the end of boot: page-fault, crash-reboot, breakpoint, \
                  mm-stress, stack-overflow, smep, smap, tlb, msr, copy, caps, fpu, sched-fair, \
                  sched-rt, sched-pi, sched-balance, lockdep, rcu, oom, user, ipc, fuzz, bench, \
                  iommu, net, gpio or i2c",
    },
    Flag {
        name: "init-after-selftest",
        value: None,
        summary: "start the first program after the self-test of selftest=, as after a boot without one",
    },
    Flag {
        name: "fuzz-seed",
        value: Some("number"),
        summary: "the first seed selftest=fuzz gives its programs, to repeat a run; random by \
                  default",
    },
    Flag {
        name: "lockdep",
        value: Some("on|off"),
        summary: "turn the lock-order checker on or off; it is on by default in debug builds only",
    },
    Flag {
        name: "init",
        value: Some("path"),
        summary: "the first program, from the boot archive (/system/commands/spark if not given); \
                  the words after a lone -- are its arguments",
    },
    Flag {
        name: "qemu-exit",
        value: None,
        summary: "leave QEMU through isa-debug-exit when boot finishes or the kernel panics",
    },
    Flag {
        name: "console-sign-in",
        value: Some("off"),
        summary: "for consoled, not the kernel: root's shell on each console with no sign-in, \
                  as the boot tests run it",
    },
    Flag {
        name: "console",
        value: Some("ttySn,options"),
        summary: "run serial port ttySn at Linux's console options, such as 57600n8 (115200n8 when \
                  none are given), from boot, and give it a sign-in through serial-console; the \
                  last is the primary console. May be given for several ports",
    },
    Flag {
        name: "serial-log",
        value: Some("ports"),
        summary: "the serial ports that carry the kernel log, kdb and a panic report: all (the \
                  default), none, or names such as ttyS0,ttyS4",
    },
    Flag {
        name: "spcr",
        value: Some("off"),
        summary: "do not make the serial port the firmware's console redirection names (ACPI \
                  SPCR) a console, as the kernel does when no console= is given",
    },
    Flag {
        name: "serial-console",
        value: Some("any"),
        summary: "for serial-console, not the kernel: every serial port offers the installer, \
                  which runs on the first where Enter is pressed, as the install stick's serial \
                  console entry starts it",
    },
    Flag {
        name: "netlog",
        value: Some("host:port"),
        summary: "for netlog, not the kernel: send the log over UDP to this host as it is \
                  written, to port 6666 if no port is given",
    },
    Flag {
        name: "startup-trouble",
        value: None,
        summary: "for login and authd, not the kernel: the consoles' sign-in offers the \
                  startup-trouble menu, which resets an administrator's password; the boot \
                  menu's startup-trouble entry sets it",
    },
    Flag {
        name: "essentials",
        value: None,
        summary: "for Spark, not the kernel: only the system's own services start, none an app \
                  or an administrator added; the boot menu's Troubleshooting sets it",
    },
    Flag {
        name: "firmware-from",
        value: Some("slot"),
        summary: "for the health check, not the kernel: the boot menu's Troubleshooting started \
                  this slot with the CPU microcode of the other slot's firmware set",
    },
    Flag {
        name: "root",
        value: Some("UUID=uuid"),
        summary: "for Spark, not the kernel: the Btrfs UUID of the Dukovo root to run the system \
                  from, which an installed system's boot menu gives; Spark mounts no other \
                  disk's root",
    },
    Flag {
        name: "slot",
        value: Some("a|b"),
        summary: "for Spark, not the kernel: the system slot GRUB started, which Spark mounts on \
                  /system; slot A without it",
    },
    Flag {
        name: "fallback",
        value: Some("slot"),
        summary: "for Spark, not the kernel: the update in this slot failed, so the version \
                  before started; Spark puts the settings from before the update back",
    },
    Flag {
        name: "trial",
        value: Some("number"),
        summary: "for Spark and the health check, not the kernel: the boot menu started this try \
                  of changes bisect, once, in place of the usual start; Spark leaves out the \
                  services the try lists",
    },
    Flag {
        name: "test-hang",
        value: None,
        summary: "for Spark, not the kernel: Spark stops responding at every start, as an update \
                  that hangs does, for the update-hang boot test",
    },
    Flag {
        name: "live",
        value: None,
        summary: "for Spark, not the kernel: the live system, as the install stick's boot menu \
                  starts it: /system stays the boot archive, no disk is the root, and no disk is \
                  mounted but the stick's DUKOVO_LIVE, read-only",
    },
    Flag {
        name: "install",
        value: None,
        summary: "for consoled, not the kernel: tty1 runs the installer's screens, as the install \
                  stick's Install Dukovo entry starts them, and root's shell once they end",
    },
    Flag {
        name: "rescue",
        value: Some("uuid"),
        summary: "for Spark, consoled and dukovo-rescue, not the kernel: with live, this is \
                  Rescue, for the Dukovo root of this Btrfs UUID: only the consoles, the log and \
                  the reader start, and the consoles run dukovo-rescue",
    },
    Flag {
        name: "speech",
        value: None,
        summary: "for readerd, not the kernel: the screen reader speaks from the first screen, \
                  whatever reader.toml says, as the install stick's Install with speech entry \
                  starts it",
    },
    Flag {
        name: "reader-log",
        value: None,
        summary: "for readerd, not the kernel: log what the screen reader says, for the boot tests",
    },
];

/// The self-tests `selftest=name` runs; radix-debug(7) describes them.
pub const SELFTESTS: &[&str] = &[
    "page-fault",
    "crash-reboot",
    "breakpoint",
    "mm-stress",
    "stack-overflow",
    "smep",
    "smap",
    "tlb",
    "msr",
    "copy",
    "caps",
    "fpu",
    "sched-fair",
    "sched-rt",
    "sched-pi",
    "sched-balance",
    "lockdep",
    "rcu",
    "oom",
    "user",
    "ipc",
    "fuzz",
    "bench",
    "iommu",
    "net",
    "gpio",
    "i2c",
];

/// A parsed view of the command line.
#[derive(Clone, Copy, Debug)]
pub struct CommandLine<'a> {
    text: &'a str,
}

impl<'a> CommandLine<'a> {
    /// Wraps the raw command line.
    pub fn new(text: &'a str) -> Self {
        Self { text }
    }

    /// The raw text.
    pub fn as_str(&self) -> &'a str {
        self.text
    }

    /// Every flag as (name, value): the words up to a lone `--`.
    pub fn flags(&self) -> impl Iterator<Item = (&'a str, Option<&'a str>)> + 'a {
        self.text.split_ascii_whitespace().take_while(|&word| word != "--").map(|flag| {
            match flag.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (flag, None),
            }
        })
    }

    /// The words after a lone `--`: the first program's arguments.
    pub fn init_args(&self) -> impl Iterator<Item = &'a str> + 'a {
        self.text.split_ascii_whitespace().skip_while(|&word| word != "--").skip(1)
    }

    /// Whether the bare flag `name` is present.
    pub fn has(&self, name: &str) -> bool {
        self.flags().any(|(flag, value)| flag == name && value.is_none())
    }

    /// The value of the last `name=value` flag.
    pub fn value(&self, name: &str) -> Option<&'a str> {
        self.flags().filter(|&(flag, _)| flag == name).filter_map(|(_, value)| value).last()
    }

    /// What the last `console=` flag for serial port `name` gives as its
    /// options: `Some(None)` for `console=ttyS1`, `Some(Some("57600n8"))`
    /// for `console=ttyS1,57600n8`, and `None` when no `console=` names
    /// the port. Names compare whole: `ttyS1` is not `ttyS10`.
    pub fn console_options(&self, name: &str) -> Option<Option<&'a str>> {
        self.flags()
            .filter(|&(flag, _)| flag == "console")
            .filter_map(|(_, value)| value)
            .filter_map(|value| match value.split_once(',') {
                Some((port, options)) => (port == name).then_some(Some(options)),
                None => (value == name).then_some(None),
            })
            .last()
    }

    /// The port the last `console=` flag names, without its options: the
    /// primary console, as Linux's `/dev/console` is.
    pub fn primary_console(&self) -> Option<&'a str> {
        let value =
            self.flags().filter(|&(flag, _)| flag == "console").filter_map(|(_, v)| v).last()?;
        Some(value.split_once(',').map_or(value, |(port, _)| port))
    }

    /// Whether serial port `name` carries the kernel log, `kdb` and a
    /// panic report, as `serial-log=` says: every port without it or with
    /// `all`, none with `none`, otherwise those its comma-separated list
    /// names.
    pub fn carries_log(&self, name: &str) -> bool {
        match self.value("serial-log") {
            None | Some("all") => true,
            Some("none") => false,
            Some(ports) => ports.split(',').any(|port| port == name),
        }
    }

    /// Whether the serial port the firmware sends its console to (ACPI's
    /// SPCR on a PC) is a console, run at the firmware's speed from boot
    /// and given a sign-in (ADR 0481): when no `console=` flag is given,
    /// as Linux takes it on Arm servers, and no `spcr=off`.
    pub fn firmware_console(&self) -> bool {
        self.value("spcr") != Some("off") && !self.flags().any(|(flag, _)| flag == "console")
    }

    /// Flags the kernel does not know, for a warning at boot.
    pub fn unknown(&self) -> impl Iterator<Item = &'a str> + 'a {
        self.flags()
            .map(|(name, _)| name)
            .filter(|name| !FLAGS.iter().any(|flag| flag.name == *name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flaw_flags_match_the_ones_the_plan_reads() {
        for &(name, values) in radix_cpuflaws::FLAGS {
            let flag = FLAGS.iter().find(|flag| flag.name == name);
            let flag = flag.unwrap_or_else(|| panic!("{name} is not in FLAGS"));
            assert_eq!(flag.value.is_some(), !values.is_empty(), "{name}");
        }
        let linux = [
            "mitigations",
            "nosmt",
            "nospectre_v1",
            "spectre_v2",
            "nospectre_v2",
            "spectre_v2_user",
            "spec_store_bypass_disable",
            "nospec_store_bypass_disable",
            "retbleed",
            "spec_rstack_overflow",
            "tsx",
            "mds",
            "tsx_async_abort",
            "mmio_stale_data",
            "reg_file_data_sampling",
            "tsa",
            "srbds",
            "gather_data_sampling",
            "indirect_target_selection",
            "pti",
            "nopti",
            "l1tf",
            "nospectre_bhb",
            "spectre_bhi",
            "ssbd",
        ];
        for flag in FLAGS.iter().filter(|flag| linux.contains(&flag.name)) {
            let known = radix_cpuflaws::FLAGS.iter().any(|&(name, _)| name == flag.name);
            assert!(known, "{} is not in the cpuflaws list", flag.name);
        }
    }

    #[test]
    fn parses_flags_and_values() {
        let line = CommandLine::new("  qemu-exit loglevel=debug kdb  loglevel=warn x=1 ");
        assert!(line.has("qemu-exit"));
        assert!(line.has("kdb"));
        assert!(!line.has("loglevel"));
        assert_eq!(line.value("loglevel"), Some("warn"));
        assert_eq!(line.value("kdb"), None);
        assert_eq!(line.unknown().collect::<Vec<_>>(), ["x"]);
        assert_eq!(line.init_args().count(), 0);
    }

    #[test]
    fn words_after_a_lone_dash_dash_are_arguments() {
        let line = CommandLine::new("kdb init=/a -- /system/tests/hello -- kdb x=1");
        assert!(line.has("kdb"));
        assert_eq!(line.value("init"), Some("/a"));
        assert_eq!(line.unknown().count(), 0);
        let args: Vec<_> = line.init_args().collect();
        assert_eq!(args, ["/system/tests/hello", "--", "kdb", "x=1"]);
    }

    #[test]
    fn console_names_a_port_whole() {
        let line =
            CommandLine::new("console=tty0 console=ttyS1,9600 console=ttyS10 -- console=ttyS2");
        assert_eq!(line.console_options("ttyS1"), Some(Some("9600")));
        assert_eq!(line.console_options("ttyS10"), Some(None));
        assert_eq!(line.console_options("ttyS0"), None);
        assert_eq!(line.console_options("ttyS2"), None);
        // The last for a port counts, as for any flag.
        let line = CommandLine::new("console=ttyS1,9600 console=ttyS1,57600n8 console=ttyS1");
        assert_eq!(line.console_options("ttyS1"), Some(None));
    }

    #[test]
    fn the_last_console_is_the_primary() {
        assert_eq!(
            CommandLine::new("console=ttyS4 console=ttyS1,57600").primary_console(),
            Some("ttyS1")
        );
        assert_eq!(CommandLine::new("console=ttyS0 console=tty0").primary_console(), Some("tty0"));
        assert_eq!(CommandLine::new("quiet -- console=ttyS0").primary_console(), None);
        assert_eq!(CommandLine::new("console").primary_console(), None);
    }

    #[test]
    fn serial_log_chooses_ports() {
        assert!(CommandLine::new("quiet").carries_log("ttyS4"));
        assert!(CommandLine::new("serial-log=all").carries_log("ttyS4"));
        assert!(!CommandLine::new("serial-log=none").carries_log("ttyS0"));
        let line = CommandLine::new("serial-log=ttyS0,ttyS4");
        assert!(line.carries_log("ttyS0") && line.carries_log("ttyS4"));
        assert!(!line.carries_log("ttyS1") && !line.carries_log("ttyS40"));
        // The last counts.
        assert!(!CommandLine::new("serial-log=all serial-log=ttyS1").carries_log("ttyS0"));
        assert!(CommandLine::new("serial-log=all -- serial-log=none").carries_log("ttyS0"));
    }

    #[test]
    fn the_firmwares_console_counts_when_no_console_is_given() {
        assert!(CommandLine::new("quiet live install serial-console=any").firmware_console());
        assert!(!CommandLine::new("console=ttyS1,57600").firmware_console());
        // Any console, as on Linux: the screen's too.
        assert!(!CommandLine::new("console=tty0").firmware_console());
        assert!(!CommandLine::new("spcr=off").firmware_console());
        assert!(CommandLine::new("spcr=on -- console=ttyS0 spcr=off").firmware_console());
    }

    #[test]
    fn flag_names_are_unique_and_simple() {
        for (i, flag) in FLAGS.iter().enumerate() {
            assert!(
                flag.name.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
                })
            );
            assert!(FLAGS[i + 1..].iter().all(|other| other.name != flag.name));
        }
    }

    #[test]
    fn the_flags_ob_test_gives_are_known() {
        let line = CommandLine::new("selftest=crash-reboot init-after-selftest panic=0 qemu-exit");
        assert_eq!(line.unknown().count(), 0, "{:?}", line.unknown().collect::<Vec<_>>());
    }

    #[test]
    fn selftest_summary_names_every_test() {
        let flag = FLAGS.iter().find(|flag| flag.name == "selftest").unwrap();
        for name in SELFTESTS {
            assert!(flag.summary.contains(name), "the selftest flag does not mention {name}");
        }
    }
}
