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
        name: "aml",
        value: Some("off"),
        summary: "run none of the firmware's AML: no uACPI, and the kernel hears the ACPI power \
                  button itself",
    },
    Flag {
        name: "pstate",
        value: Some("off"),
        summary: "leave the CPUs' speed to the firmware, though they have Intel's HWP or AMD's CPPC",
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
        name: "crashlog",
        value: Some("off"),
        summary: "do not keep the log in memory for the next boot",
    },
    Flag {
        name: "selftest",
        value: Some("name"),
        summary: "run a self-test at the end of boot: page-fault, crash-reboot, breakpoint, \
                  mm-stress, stack-overflow, smep, smap, tlb, msr, caps, fpu, sched-fair, \
                  sched-rt, sched-pi, sched-balance, lockdep, rcu, oom, user, ipc, fuzz, bench, \
                  iommu, net, gpio or i2c",
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
    fn flag_names_are_unique_and_simple() {
        for (i, flag) in FLAGS.iter().enumerate() {
            assert!(
                flag.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            );
            assert!(FLAGS[i + 1..].iter().all(|other| other.name != flag.name));
        }
    }

    #[test]
    fn selftest_summary_names_every_test() {
        let flag = FLAGS.iter().find(|flag| flag.name == "selftest").unwrap();
        for name in SELFTESTS {
            assert!(flag.summary.contains(name), "the selftest flag does not mention {name}");
        }
    }
}
