// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dukovo's native administration tools (ADR 0096): the commands an
//! administrator expects from Linux whose Linux versions read Linux's
//! `/proc`, utmp or system calls, written for Radix, with their names and
//! common options, as one program. A link by each tool's name runs it
//! (`/system/commands/ps` is a link to `systools`), and so does
//! `systools <tool> [argument...]`.
//!
//! The program is a library with a `main` of its own and a binary that
//! calls it, so that a host program can link the same code. Every module
//! is private: the library gives that one function, and, when built with
//! the `fuzz` feature, the module `fuzz` (ADR 0515).
//!
//! The process tools read the kernel's `/processes` (radix-proc(5)); `lsof`
//! lists the capabilities each process holds; `rtrace` reads a trace of
//! a process's system calls (radix-trace(2)), and `dmesg` the kernel log
//! through `/devices/kmsg`; `logd` keeps the system's log and its audit
//! log, which `log` and `journalctl` read and `logtrim` trims (ADRs 0210
//! and 0211), and `logger` writes a line to it; `notifyd` passes
//! notifications from `notify-send` and other programs to the console,
//! and `notifications` lists them (ADR 0212).
//! `e1000d` drives Intel's e1000 and e1000e cards as a program (ADR
//! 0198), `netd` sets the network cards up (ADR 0191), and `resolved` answers
//! every program's DNS queries, which `resolvectl` shows (ADR 0193);
//! `discoverd` finds `.local` names and what the local networks offer,
//! which `discover` lists, and `ping` asks a machine whether it is there
//! (ADRs 0336 to 0338);
//! `dig` and `host` ask name servers, and `whois` the registries, what
//! they hold, and `nic` shows what a network card says of itself, as
//! ethtool does (ADR 0341); `traceroute` finds the way to a machine, a
//! router at a time (ADR 0345); `netcheck` finds the layer where the
//! network stops working (ADR 0489), and `network` shows each card with
//! what it has and what network(5) says of it (ADR 0518);
//! `timed` keeps the clock right from the network over NTS, and
//! `timedatectl` shows it and sets the time zone (ADR 0194); `netlog`
//! sends the log to another computer as it is written (ADR 0196), and
//! `logforward` sends it to syslog servers over TLS, TCP or UDP, keeping
//! it until each has it (ADR 0482); `metricsd` keeps the history of the
//! machine's processors, memory, disks, network cards and temperatures,
//! which `metrics` shows (ADR 0484);
//! `firewall` gives the packet filter its rules and says what listens
//! (ADR 0334).
//! `authd` keeps the accounts, which `user` and `passwd` change, `login`
//! signs people in through it, and `elevate`, and `sudo` in front of it,
//! run a command as an administrator (ADRs 0218, 0250 and 0219); `last`,
//! `lastb` and `lastlog` show the sign-ins it records (ADR 0254);
//! `serial-console` runs the installer, a shell or the sign-in on the
//! PC's serial ports, those a `console=ttyS<n>` flag names or the one the
//! firmware sends its console to (ADRs 0318, 0480 and 0481); `serial` lists
//! the ports and sets which have a console and carry the kernel log at the
//! next start, and which the boot menu shows on.
//! `rdisk` partitions disks, `mkfs-fat` and `mkfs-exfat` make FAT and
//! exFAT filesystems, and `fsck-fat` checks and mends FAT ones, with the
//! disk engine (ADRs 0300 to 0302); `mkfs-btrfs` makes a Btrfs, and
//! `fsck-btrfs` checks one, with the kernel's Btrfs crate (ADRs 0304 and
//! 0305); `resize-ntfs` checks and shrinks an NTFS volume with the
//! kernel's NTFS crate (ADR 0312); `rgrub-setup` puts GRUB on a disk or
//! an EFI system partition (ADR 0306); `efiboot`, and `efibootmgr` by
//! its name, lists and changes the firmware's boot entries (ADR 0313).
//! `changes` says what changed between two starts, from the record the
//! health check keeps of each (ADR 0500).
//! `hardware` checks the memory, the disks, the processors, the sensors,
//! the battery and the network cards for faults, and keeps what it found
//! (ADR 0488).
//! `help-session` shares the owner's screen with a helper who signs in over
//! SSH as `help`, and ends at a key (ADR 0540).
//! `dukovo-check` looks at the computer's security and health, a line for
//! each check, and keeps what it found, every day by its service's timer
//! (ADR 0479).
//! `dumps` saves the kernel's crash dump at the boot after a panic, and
//! lists, shows and deletes the saved ones (ADR 0483).
//! `tune` chooses a performance profile, a few of the kernel's settings
//! with a reason each, and its service applies it at each start (ADR
//! 0476).
//! `audiocd` lists, plays and rips music CDs (ADR 0447); `burn` writes
//! CDs, DVDs and Blu-ray discs, of data or of audio, and `mkiso` makes a
//! disc image of a folder (ADR 0449).
//! `certd` keeps the machine's certificates, which `cert` lists, adds and
//! removes, and which Spark copies for the services that name them (ADR
//! 0512).
//! `website` adds, lists, checks and removes nginx's websites, and takes one
//! to HTTPS, with nginx's own check run on the new set before it is live
//! (ADR 0569).
//! `bundle` makes, signs, installs and runs bundles, which `bundled`
//! installs, seals and removes (ADRs 0221 to 0223 and 0225); called by a
//! name that is no tool's, through a link in a commands folder of
//! bundled's, it runs the bundle command of that name.
//! `linux` runs a Linux program in a sandbox of its own, with `linuxd`
//! beside it to answer what the kernel's Linux table does not (ADRs 0621
//! and 0622).
//! `sysupdate` stages a signed release in the spare slot, and as a
//! service checks that a new version proves itself, or falls back
//! (ADRs 0451 to 0454); `dukovo-rescue` is Rescue's screens, which
//! repair a system that will not start (ADR 0460).

mod account;
mod account_errors;
mod admin;
mod admin_about;
mod admin_certificate;
mod admin_errors;
mod admin_export;
#[cfg(feature = "lab")]
mod admin_lab;
mod admin_put_back;
mod admin_settings;
mod admind;
mod alertd;
mod alerts;
mod args;
mod at;
mod audiocd;
mod authd;
mod backup;
mod backup_errors;
mod balance;
mod blkid;
mod blockdev;
mod braille;
mod btrfs;
mod bundled;
mod bundles;
mod burn;
mod cert;
mod certd;
mod changes;
mod check;
mod cloud;
mod column;
mod crontab;
mod database;
mod deadline;
mod device;
mod df;
mod dhcp;
mod dhcp6;
mod dhcp_server;
mod dig;
mod discover;
mod discoverd;
mod disk_errors;
mod diskhealth;
mod dmesg;
mod dukofetch;
mod dumps;
mod e1000d;
mod ectool;
mod efiboot;
mod eject;
mod elevate;
mod facl;
mod feature;
mod findmnt;
mod firewall;
mod floppy;
mod fstrim;
#[cfg(feature = "fuzz")]
pub mod fuzz;
mod git_credential;
mod guest_agent;
mod hardware;
mod help_session;
mod hexdump;
mod hostnamectl;
mod hwinfo;
mod iostat;
mod ip;
mod jobs;
mod journalctl;
mod json;
mod kerberos;
mod keymaps;
mod keyring;
mod last;
mod links;
mod linux;
mod log;
mod logd;
mod logforward;
mod logger;
mod login;
mod logtrim;
mod logturn;
mod lsblk;
mod lscpu;
mod lsmem;
mod lsof;
mod lspci;
mod lsusb;
mod mail;
mod memory;
mod metricsd;
mod mkfs;
mod modes;
mod mount;
mod net_errors;
mod netcheck;
mod netd;
mod netlog;
mod network;
mod nic;
mod notifyd;
mod ntfs;
mod nvme;
mod pidstat;
mod ping;
mod power;
mod priority;
mod private_access;
mod probe;
mod proc;
mod prompt;
mod ps;
mod quota;
mod rdisk;
mod reach;
mod reader;
mod releases;
mod remount;
mod replace;
mod reportd;
mod reports;
mod rescue;
mod resolvectl;
mod resolved;
mod rgrub;
mod rtrace;
mod script;
mod scrub;
mod selftest;
mod sendmail;
mod sensors;
mod serial;
mod serial_console;
mod service;
mod servicefiles;
mod servicetry;
mod sessions;
mod settings_edit;
mod share;
mod sign_in_notices;
mod signals;
mod signin;
mod snap;
mod sound;
mod ss;
mod ssh_askpass;
mod ssh_server;
mod sshkeys;
mod startup;
mod storage;
mod subvolume;
mod swap;
mod sys;
mod systemctl;
mod sysupdate;
mod timed;
mod timedatectl;
mod timerd;
mod tls;
mod top;
mod traceroute;
mod tree;
mod trial;
mod troubleshoot;
mod tune;
mod unlock;
mod update;
mod util;
mod volumed;
mod watch;
mod watchdog;
mod website;
mod wg;
mod wgd;
mod which;
mod whois;
mod wireguard;
mod xxd;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use args::Args;
use util::Fail;

/// A tool: it takes its arguments and returns its exit status.
type Tool = fn(&mut Args) -> Result<i32, Fail>;

/// The tools, by name, in order.
const TOOLS: &[(&str, Tool)] = &[
    ("acme-answer", cert::acme::answer::main),
    ("admin", admin::main),
    ("admind", admind::main),
    ("alertd", alertd::main),
    ("alerts", alerts::command::main),
    ("at", at::main),
    ("atq", at::atq),
    ("atrm", at::atrm),
    ("audio", sound::main),
    ("audiocd", audiocd::main),
    ("audiod", sound::daemon),
    ("authd", authd::main),
    ("backup", backup::main),
    ("balance", balance::main),
    ("batch", at::batch),
    ("blkid", blkid::main),
    ("braille", braille::main),
    ("brailled", braille::daemon),
    ("btrfsck", btrfs::fsck_btrfs),
    ("bundle", bundles::main),
    ("bundled", bundled::main),
    ("burn", burn::main),
    ("cert", cert::main),
    ("certd", certd::main),
    ("changes", changes::changes),
    ("chgrp", modes::chgrp),
    ("chmod", modes::chmod),
    ("chown", modes::chown),
    ("cloud-setup", cloud::main),
    ("column", column::main),
    ("crontab", crontab::main),
    ("database", database::main),
    ("df", df::main),
    ("device", device::main),
    ("dig", dig::dig),
    ("discover", discover::main),
    ("discoverd", discoverd::main),
    ("diskhealth", diskhealth::main),
    ("dmesg", dmesg::main),
    ("dukofetch", dukofetch::main),
    ("dukovo-check", check::main),
    ("dukovo-rescue", rescue::main),
    ("dumps", dumps::main),
    ("e1000d", e1000d::main),
    ("ectool", ectool::main),
    ("efiboot", efiboot::main),
    ("efibootmgr", efiboot::main),
    ("eject", eject::main),
    ("elevate", elevate::main),
    ("feature", feature::main),
    ("findmnt", findmnt::main),
    ("firewall", firewall::main),
    ("floppy", floppy::main),
    ("fsck-btrfs", btrfs::fsck_btrfs),
    ("fsck-fat", mkfs::fsck_fat),
    ("fsck.fat", mkfs::fsck_fat),
    ("fsck.vfat", mkfs::fsck_fat),
    ("fstrim", fstrim::main),
    ("free", memory::free),
    ("getfacl", facl::getfacl),
    ("git-credential-keyring", git_credential::main),
    ("guest-agent", guest_agent::main),
    ("halt", power::halt),
    ("hardware", hardware::main),
    ("hd", hexdump::hd),
    ("help-session", help_session::main),
    ("hexdump", hexdump::main),
    ("host", dig::host),
    ("hostnamectl", hostnamectl::main),
    ("hwinfo", hwinfo::main),
    ("ionice", priority::ionice),
    ("iostat", iostat::main),
    ("ip", ip::main),
    ("journalctl", journalctl::main),
    ("kdestroy", kerberos::kdestroy),
    ("keymap", keymaps::main),
    ("keyring", keyring::main),
    ("kill", signals::kill),
    ("kinit", kerberos::kinit),
    ("klist", kerberos::klist),
    ("kvno", kerberos::kvno),
    ("last", last::last),
    ("lastb", last::lastb),
    ("lastlog", last::lastlog),
    ("linux", linux::main),
    ("log", log::main),
    ("logd", logd::main),
    ("logforward", logforward::main),
    ("logger", logger::main),
    ("login", login::main),
    ("logtrim", logtrim::main),
    ("logturn", logturn::main),
    ("lsblk", lsblk::main),
    ("lscpu", lscpu::main),
    ("lsmem", lsmem::main),
    ("lsof", lsof::main),
    ("lspci", lspci::main),
    ("lsusb", lsusb::main),
    ("metrics", metricsd::metrics),
    ("metricsd", metricsd::main),
    ("mkfs", mkfs::mkfs),
    ("mkfs-btrfs", btrfs::mkfs_btrfs),
    ("mkfs-exfat", mkfs::mkfs_exfat),
    ("mkfs-fat", mkfs::mkfs_fat),
    ("mkfs.btrfs", btrfs::mkfs_btrfs),
    ("mkfs.exfat", mkfs::mkfs_exfat),
    ("mkfs.fat", mkfs::mkfs_fat),
    ("mkfs.vfat", mkfs::mkfs_fat),
    ("mkiso", burn::mkiso),
    ("mkswap", swap::mkswap),
    ("mount", mount::mount),
    ("netcheck", netcheck::main),
    ("netd", netd::main),
    ("netlog", netlog::main),
    ("network", network::main),
    ("nic", nic::main),
    ("notifications", notifyd::notifications),
    ("notify-send", notifyd::notify_send),
    ("notifyd", notifyd::main),
    ("ntfsresize", ntfs::resize_ntfs),
    ("nvme", nvme::main),
    ("passwd", account::passwd),
    ("pgrep", signals::pgrep),
    ("pidstat", pidstat::main),
    ("ping", ping::main),
    ("pkill", signals::pkill),
    ("poweroff", power::poweroff),
    ("ps", ps::main),
    ("quota", quota::main),
    ("rdisk", rdisk::main),
    ("reach", reach::main),
    ("reader", reader::main),
    ("readerd", reader::daemon),
    ("reboot", power::reboot),
    ("remount", remount::main),
    ("renice", priority::renice),
    ("replace", replace::main),
    ("report", reports::command::main),
    ("reportd", reportd::main),
    ("resize-ntfs", ntfs::resize_ntfs),
    ("resolvectl", resolvectl::main),
    ("resolved", resolved::main),
    ("rgrub-setup", rgrub::main),
    ("rtrace", rtrace::main),
    ("script", script::main),
    ("scrub", scrub::main),
    ("sendmail", sendmail::main),
    ("sensors", sensors::main),
    ("serial", serial::main),
    ("serial-console", serial_console::main),
    ("service", service::service),
    ("setfacl", facl::setfacl),
    ("share", share::main),
    ("shutdown", power::shutdown),
    ("snap", snap::main),
    ("ss", ss::main),
    ("ssh-askpass-keyring", ssh_askpass::main),
    ("ssh-server", ssh_server::main),
    ("startup", startup::main),
    ("storage", storage::main),
    ("subvolume", subvolume::main),
    ("sudo", elevate::main),
    ("swapoff", swap::swapoff),
    ("swapon", swap::swapon),
    ("system-release", releases::main),
    ("systemctl", systemctl::main),
    ("sysupdate", sysupdate::main),
    ("taskset", priority::taskset),
    ("timed", timed::main),
    ("timedatectl", timedatectl::main),
    ("timerd", timerd::main),
    ("top", top::main),
    ("traceroute", traceroute::main),
    ("traceroute6", traceroute::main6),
    ("tree", tree::main),
    ("troubleshoot", troubleshoot::main),
    ("tune", tune::main),
    ("umount", mount::umount),
    ("unlock", unlock::main),
    ("update", update::update),
    ("updated", update::updated),
    ("uptime", sessions::uptime),
    ("user", account::user),
    ("users", sessions::users),
    ("vmstat", memory::vmstat),
    ("volumed", volumed::main),
    ("w", sessions::w),
    ("watch", watch::main),
    ("watchdog", watchdog::main),
    ("website", website::main),
    ("wg", wg::main),
    ("wgd", wgd::main),
    ("which", which::main),
    ("who", sessions::who),
    ("whois", whois::main),
    ("wireguard", private_access::main),
    ("xxd", xxd::main),
];

/// The tools whose Linux versions exit with 2, not 1, for a mistake in
/// the command line: procps-ng's pgrep and pkill, iputils' ping, and
/// which; update, whose exit status 1 says that something applies
/// (`update advisories`, ADR 0534); alerts, whose exit status 1 says
/// that the channel it was to change is not there, or that a test through
/// a channel failed; and database and website, whose exit status 1 says
/// that PostgreSQL or nginx, or what was asked of it, refused, with an ID.
const USAGE_TWO: &[&str] = &[
    "alerts", "database", "getfacl", "pgrep", "ping", "pkill", "setfacl", "update", "website",
    "which",
];

/// The tool named `name`.
fn tool(name: &str) -> Option<Tool> {
    TOOLS.iter().find(|(tool, _)| *tool == name).map(|(_, run)| *run)
}

/// Runs the tool the program was called by, or the one `systools TOOL`
/// names, or the command of a bundle when the name is no tool's, and gives
/// its exit status.
pub fn main() -> ExitCode {
    // SAFETY: setting a signal's disposition to the default touches no
    // memory; it is done before any thread starts.
    unsafe {
        // A closed pipe ends a tool quietly, as it does a C program,
        // where Rust would have every write after it fail.
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let mut argv: Vec<OsString> = std::env::args_os().collect();
    let called = argv.first().map(Path::new).and_then(Path::file_name);
    let mut name = called.map_or_default(|name| name.to_string_lossy().into_owned());
    if tool(&name).is_none() && name != "systools" && !name.is_empty() {
        // A bundle's command, through the link bundled made by its name
        // (ADR 0225): everything after it is the command's.
        let code = bundles::run::command(&name, argv.split_off(1));
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    if tool(&name).is_none() {
        // `systools <tool> ...`, or `systools --list`.
        match argv.get(1).map(|arg| arg.to_string_lossy().into_owned()) {
            Some(arg) if arg == "--list" => {
                let mut out = io::stdout().lock();
                for (tool, _) in TOOLS {
                    let _ = writeln!(out, "{tool}");
                }
                return ExitCode::SUCCESS;
            }
            Some(arg) if tool(&arg).is_some() => {
                argv.remove(0);
                name = arg;
            }
            _ => {
                eprintln!("usage: systools TOOL [ARGUMENT...]   (systools --list names them)");
                return ExitCode::from(2);
            }
        }
    }
    let Some(&(name, run)) = TOOLS.iter().find(|(tool, _)| *tool == name) else {
        // Each way here has found the tool.
        eprintln!("usage: systools TOOL [ARGUMENT...]   (systools --list names them)");
        return ExitCode::from(2);
    };
    let mut args = Args::new(name, argv.into_iter().skip(1).collect());
    let code = match run(&mut args) {
        Ok(code) => code,
        Err(fail) => {
            if !fail.message.is_empty() {
                eprintln!("{name}: {}", fail.message);
            }
            if fail.usage && USAGE_TWO.contains(&name) { 2 } else { fail.code }
        }
    };
    // A closed standard output (`ps | head`) is not an error to report.
    let _ = io::stdout().flush();
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
