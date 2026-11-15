// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Building the user programs in userspace/, and the boot archive.
//!
//! The test and benchmark programs are linked into one image that the
//! boot loader loads as a module (`usertests`) and the kernel's `user`,
//! `ipc`, `fuzz` and `bench` self-tests start (ADR 0036). Spark and the
//! programs it starts go into the boot archive (`bootfs`), a ustar file
//! the boot loader loads as another module and the kernel starts Spark
//! from (spark(8)), with the C test programs of userspace/posix, the
//! files the ports put there (ADR 0069) and the programs on Rust's
//! standard library of userspace/programs (ADR 0070).

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Profile;
use crate::ports::{self, Ports};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result, doctor};

/// The user programs' target specification, relative to the repository
/// root.
pub const TARGET_SPEC: &str = "userspace/targets/x86_64-radix-user.json";

/// Name Cargo gives the target's output directory.
pub const TARGET_NAME: &str = "x86_64-radix-user";

/// The target of programs on Rust's standard library, relative to the
/// repository root (ADR 0070), and the name of its output directory.
pub const STD_TARGET_SPEC: &str = "userspace/targets/x86_64-unknown-radix.json";
pub const STD_TARGET_NAME: &str = "x86_64-unknown-radix";

/// Where the test image goes in a boot image, and the word on its module
/// line that the kernel finds it by.
pub const USERTESTS_PATH: &str = "/boot/usertests";
pub const USERTESTS_WORD: &str = "usertests";

/// Where the boot archive goes in a boot image, and the word on its module
/// line.
pub const BOOTFS_PATH: &str = "/boot/bootfs";
pub const BOOTFS_WORD: &str = "bootfs";

/// Where `ob` writes the startup archive, which an installed system
/// starts from instead of the boot archive (ADR 0307).
pub const STARTUP_PATH: &str = "/boot/startup";

/// The program the startup archive holds: Spark, which mounts the
/// installed system's root and runs everything else from it.
pub const STARTUP_PROGRAM: &str = "system/commands/spark";

/// The Cargo profile a release builds Spark with (userspace/Cargo.toml):
/// for size, so that the startup archive, Spark alone, keeps inside its
/// budget (ADR 0427).
const SPARK_PROFILE: &str = "spark";

/// What the boot archive holds: each program's path in it, and its
/// package.
pub const ARCHIVE: &[(&str, &str)] = &[
    ("system/commands/spark", "spark"),
    ("system/tests/hello", "hello"),
    ("system/tests/sockets", "sockets"),
    ("system/tests/storage", "storage"),
];

/// Empty directories of the boot archive: mount points under `/system`
/// for what Spark mounts there (spark(8)).
pub const ARCHIVE_DIRS: &[&str] =
    &["system/boot", "system/live", "system/settings", "system/state"];

/// Symbolic links of the boot archive: Unix's names for folders of
/// `/system`, each leading to the folder that holds what it stands for
/// (ADR 0114), so that `/usr/bin` and `/usr/lib` still lead somewhere.
pub const ARCHIVE_LINKS: &[(&str, &str)] =
    &[("system/bin", "commands"), ("system/lib", "libraries"), ("system/share", "resources")];

/// Symbolic links among the default settings, which Spark copies as
/// links, each until an administrator makes it a file of their own:
/// `/etc/resolv.conf` leads to the one resolved(8) writes, which names
/// it as the server (ADR 0193), as Linux's leads to systemd-resolved's;
/// `/etc/localtime`, the time zone, to UTC's in the time zone database,
/// which timedatectl(1) points elsewhere (ADR 0194); Debian's and
/// Alpine's names for the root certificates lead to the system's (ADR
/// 0192); and `/etc/os-release` to the file each release carries
/// ([`OS_RELEASE`]), as Linux's does.
pub const DEFAULT_LINKS: &[(&str, &str)] = &[
    ("system/resources/default-settings/localtime", "/system/resources/zoneinfo/UTC"),
    ("system/resources/default-settings/os-release", "/usr/lib/os-release"),
    ("system/resources/default-settings/resolv.conf", "/system/live/resolved/resolv.conf"),
    ("system/resources/default-settings/ssl/cert.pem", CERTIFICATES),
    ("system/resources/default-settings/ssl/certs/ca-certificates.crt", CERTIFICATES),
];

/// Where each release keeps its os-release(5): Linux's `/usr/lib/os-release`,
/// since `/usr/lib` leads to `libraries`. It is part of the release, so it
/// names the version that runs; `/etc/os-release` leads to it.
pub const OS_RELEASE: &str = "system/libraries/os-release";

/// The text of os-release(5) for Dukovo `version`, which scripts read to
/// tell which system they run on: Ansible's `distribution` is its `NAME`.
pub fn os_release_text(version: &str) -> String {
    format!(
        "NAME=\"Dukovo\"\n\
         ID=dukovo\n\
         VERSION_ID=\"{version}\"\n\
         VERSION=\"{version}\"\n\
         PRETTY_NAME=\"Dukovo {version}\"\n\
         HOME_URL=\"https://dukovo.org/\"\n\
         SUPPORT_URL=\"https://dukovo.org/support/\"\n\
         BUG_REPORT_URL=\"https://github.com/dukovo-os/dukovo/issues\"\n"
    )
}

/// Writes this build's os-release(5) in `out/os-release`, for the release's
/// `libraries` folder: its path in the boot archive, and the file.
fn os_release(ctx: &Ctx) -> Result<(String, PathBuf)> {
    let dir = ctx.out.join("os-release");
    std::fs::create_dir_all(&dir).context(|| format!("making {}", dir.display()))?;
    let file = dir.join("os-release");
    std::fs::write(&file, os_release_text(crate::VERSION))
        .context(|| format!("writing {}", file.display()))?;
    std::fs::set_permissions(&file, Permissions::from_mode(0o644))
        .context(|| format!("writing {}", file.display()))?;
    Ok((OS_RELEASE.to_string(), file))
}

/// The root certificates the system trusts, which ports/ca-certificates
/// writes (dukovo-certificates(7)).
pub const CERTIFICATES: &str = "/system/resources/certificates/ca-certificates.crt";

/// The `.hidden` file of `/system`: the names of its links, which
/// listings leave out (ADR 0113).
pub const ARCHIVE_HIDDEN: (&str, &str) = ("system/.hidden", "bin\nlib\nshare\n");

/// The programs on Rust's standard library that the boot archive holds:
/// each one's path in it, and its package in userspace/programs.
pub const STD_ARCHIVE: &[(&str, &str)] = &[
    ("system/commands/consoled", "consoled"),
    ("system/commands/dukovo-install", "dukovo-install"),
    ("system/commands/nfs", "nfs"),
    ("system/commands/nfsd", "nfsd"),
    ("system/commands/nukesh", "nukesh"),
    ("system/commands/smb", "smb"),
    ("system/commands/smbd", "smbd"),
    ("system/commands/sshd", "sshd"),
    ("system/commands/systools", "systools"),
    ("system/tests/posix/fuse-test", "fuse-test"),
    ("system/tests/posix/git-https-test", "git-https-test"),
    ("system/tests/posix/monitoring-test", "monitoring-test"),
    ("system/tests/posix/netservices-test", "netservices-test"),
    ("system/tests/posix/sqlite-test", "sqlite-test"),
    ("system/tests/posix/std-test", "std-test"),
    ("system/tests/posix/tls-test", "tls-test"),
    ("system/tests/report/crash-test", "report-test"),
];

/// Symbolic links to those programs in the boot archive: each link's path,
/// and its target. Nukesh is the POSIX shell when it is started as `sh`,
/// a restricted shell as `rnukesh`, and runs bash's scripts as `bash`
/// (nukesh(1), ADR 0103); the native tools are one program, which each
/// tool's name runs (ADR 0096).
pub const STD_LINKS: &[(&str, &str)] = &[
    ("system/commands/bash", "nukesh"),
    ("system/commands/sh", "nukesh"),
    ("system/commands/rnukesh", "nukesh"),
    ("system/commands/admin", "systools"),
    ("system/commands/admind", "systools"),
    ("system/commands/alertd", "systools"),
    ("system/commands/alerts", "systools"),
    ("system/commands/at", "systools"),
    ("system/commands/atq", "systools"),
    ("system/commands/atrm", "systools"),
    ("system/commands/audio", "systools"),
    ("system/commands/audiocd", "systools"),
    ("system/commands/audiod", "systools"),
    ("system/commands/authd", "systools"),
    ("system/commands/backup", "systools"),
    ("system/commands/balance", "systools"),
    ("system/commands/batch", "systools"),
    ("system/commands/blkid", "systools"),
    ("system/commands/braille", "systools"),
    ("system/commands/brailled", "systools"),
    ("system/commands/btrfsck", "systools"),
    ("system/commands/bundle", "systools"),
    ("system/commands/bundled", "systools"),
    ("system/commands/burn", "systools"),
    ("system/commands/cert", "systools"),
    ("system/commands/certd", "systools"),
    ("system/commands/changes", "systools"),
    ("system/commands/chgrp", "systools"),
    ("system/commands/chmod", "systools"),
    ("system/commands/chown", "systools"),
    ("system/commands/cloud-setup", "systools"),
    ("system/commands/column", "systools"),
    ("system/commands/crontab", "systools"),
    ("system/commands/df", "systools"),
    ("system/commands/device", "systools"),
    ("system/commands/dig", "systools"),
    ("system/commands/discover", "systools"),
    ("system/commands/discoverd", "systools"),
    ("system/commands/diskhealth", "systools"),
    ("system/commands/dmesg", "systools"),
    ("system/commands/dukofetch", "systools"),
    ("system/commands/dukovo-check", "systools"),
    ("system/commands/dukovo-rescue", "systools"),
    ("system/commands/dumps", "systools"),
    ("system/commands/e1000d", "systools"),
    ("system/commands/ectool", "systools"),
    ("system/commands/efiboot", "systools"),
    ("system/commands/efibootmgr", "systools"),
    ("system/commands/eject", "systools"),
    ("system/commands/elevate", "systools"),
    ("system/commands/feature", "systools"),
    ("system/commands/findmnt", "systools"),
    ("system/commands/firewall", "systools"),
    ("system/commands/floppy", "systools"),
    ("system/commands/fsck-btrfs", "systools"),
    ("system/commands/fsck-fat", "systools"),
    ("system/commands/fsck.fat", "systools"),
    ("system/commands/fsck.vfat", "systools"),
    ("system/commands/fstrim", "systools"),
    ("system/commands/free", "systools"),
    ("system/commands/getfacl", "systools"),
    ("system/commands/git-credential-keyring", "systools"),
    ("system/commands/guest-agent", "systools"),
    ("system/commands/hd", "systools"),
    ("system/commands/halt", "systools"),
    ("system/commands/hardware", "systools"),
    ("system/commands/help-session", "systools"),
    ("system/commands/hexdump", "systools"),
    ("system/commands/host", "systools"),
    ("system/commands/hostnamectl", "systools"),
    ("system/commands/hwinfo", "systools"),
    ("system/commands/ionice", "systools"),
    ("system/commands/iostat", "systools"),
    ("system/commands/ip", "systools"),
    ("system/commands/journalctl", "systools"),
    ("system/commands/kdestroy", "systools"),
    ("system/commands/keymap", "systools"),
    ("system/commands/keyring", "systools"),
    ("system/commands/kill", "systools"),
    ("system/commands/kinit", "systools"),
    ("system/commands/klist", "systools"),
    ("system/commands/kvno", "systools"),
    ("system/commands/last", "systools"),
    ("system/commands/lastb", "systools"),
    ("system/commands/lastlog", "systools"),
    ("system/commands/log", "systools"),
    ("system/commands/logd", "systools"),
    ("system/commands/logforward", "systools"),
    ("system/commands/logger", "systools"),
    ("system/commands/login", "systools"),
    ("system/commands/logtrim", "systools"),
    ("system/commands/logturn", "systools"),
    ("system/commands/lsblk", "systools"),
    ("system/commands/lscpu", "systools"),
    ("system/commands/lsmem", "systools"),
    ("system/commands/lsof", "systools"),
    ("system/commands/lspci", "systools"),
    ("system/commands/lsusb", "systools"),
    ("system/commands/metrics", "systools"),
    ("system/commands/metricsd", "systools"),
    ("system/commands/mkfs", "systools"),
    ("system/commands/mkfs-btrfs", "systools"),
    ("system/commands/mkfs-exfat", "systools"),
    ("system/commands/mkfs-fat", "systools"),
    ("system/commands/mkfs.btrfs", "systools"),
    ("system/commands/mkfs.exfat", "systools"),
    ("system/commands/mkfs.fat", "systools"),
    ("system/commands/mkfs.vfat", "systools"),
    ("system/commands/mkiso", "systools"),
    ("system/commands/mkswap", "systools"),
    ("system/commands/mount", "systools"),
    ("system/commands/netcheck", "systools"),
    ("system/commands/netd", "systools"),
    ("system/commands/netlog", "systools"),
    ("system/commands/network", "systools"),
    ("system/commands/nic", "systools"),
    ("system/commands/notifications", "systools"),
    ("system/commands/notify-send", "systools"),
    ("system/commands/notifyd", "systools"),
    ("system/commands/ntfsresize", "systools"),
    ("system/commands/nvme", "systools"),
    ("system/commands/passwd", "systools"),
    ("system/commands/pgrep", "systools"),
    ("system/commands/pidstat", "systools"),
    ("system/commands/ping", "systools"),
    ("system/commands/pkill", "systools"),
    ("system/commands/poweroff", "systools"),
    ("system/commands/ps", "systools"),
    ("system/commands/quota", "systools"),
    ("system/commands/rdisk", "systools"),
    ("system/commands/reach", "systools"),
    ("system/commands/reader", "systools"),
    ("system/commands/readerd", "systools"),
    ("system/commands/reboot", "systools"),
    ("system/commands/remount", "systools"),
    ("system/commands/renice", "systools"),
    ("system/commands/replace", "systools"),
    ("system/commands/report", "systools"),
    ("system/commands/reportd", "systools"),
    ("system/commands/resize-ntfs", "systools"),
    ("system/commands/resolvectl", "systools"),
    ("system/commands/resolved", "systools"),
    ("system/commands/rgrub-setup", "systools"),
    ("system/commands/rtrace", "systools"),
    ("system/commands/script", "systools"),
    ("system/commands/scrub", "systools"),
    ("system/commands/sensors", "systools"),
    ("system/commands/sendmail", "systools"),
    ("system/commands/serial", "systools"),
    ("system/commands/serial-console", "systools"),
    ("system/commands/service", "systools"),
    ("system/commands/setfacl", "systools"),
    ("system/commands/share", "systools"),
    ("system/commands/shutdown", "systools"),
    ("system/commands/snap", "systools"),
    ("system/commands/ss", "systools"),
    ("system/commands/ssh-askpass-keyring", "systools"),
    ("system/commands/startup", "systools"),
    ("system/commands/storage", "systools"),
    ("system/commands/subvolume", "systools"),
    ("system/commands/sudo", "systools"),
    ("system/commands/swapoff", "systools"),
    ("system/commands/swapon", "systools"),
    ("system/commands/system-release", "systools"),
    ("system/commands/systemctl", "systools"),
    ("system/commands/sysupdate", "systools"),
    ("system/commands/taskset", "systools"),
    ("system/commands/timed", "systools"),
    ("system/commands/timedatectl", "systools"),
    ("system/commands/timerd", "systools"),
    ("system/commands/top", "systools"),
    ("system/commands/traceroute", "systools"),
    ("system/commands/traceroute6", "systools"),
    ("system/commands/tree", "systools"),
    ("system/commands/troubleshoot", "systools"),
    ("system/commands/tune", "systools"),
    ("system/commands/umount", "systools"),
    ("system/commands/unlock", "systools"),
    ("system/commands/update", "systools"),
    ("system/commands/updated", "systools"),
    ("system/commands/uptime", "systools"),
    ("system/commands/user", "systools"),
    ("system/commands/users", "systools"),
    ("system/commands/vmstat", "systools"),
    ("system/commands/volumed", "systools"),
    ("system/commands/w", "systools"),
    ("system/commands/watch", "systools"),
    ("system/commands/watchdog", "systools"),
    ("system/commands/wg", "systools"),
    ("system/commands/wgd", "systools"),
    ("system/commands/which", "systools"),
    ("system/commands/who", "systools"),
    ("system/commands/whois", "systools"),
    ("system/commands/xxd", "systools"),
];

/// The shell's test scripts, with their expected output, and where in
/// the boot archive they go, for the `shell` POSIX test.
pub const SHELL_CASES: &str = "userspace/programs/nukesh/tests/cases";
pub const SHELL_CASES_DIR: &str = "system/tests/shell";

/// The system's own default settings (ADR 0090), and where in the boot
/// archive they go: among the ports' defaults, which Spark copies into
/// `/system/settings` (spark(8)).
pub const DEFAULTS: &str = "userspace/defaults";
pub const DEFAULTS_DIR: &str = "system/resources/default-settings";

/// The system's features (feature.toml(5), ADR 0316), and where in the
/// boot archive they go, beside the rest of the signed system.
pub const FEATURES: &str = "userspace/features";
pub const FEATURES_DIR: &str = "system/resources/features";

/// The signatures troubleshoot(8) matches (ADR 0474), and where in the
/// boot archive they go, beside the rest of the signed system.
pub const TROUBLESHOOT: &str = "userspace/troubleshoot";
pub const TROUBLESHOOT_DIR: &str = "system/resources/troubleshoot";
/// The performance profiles tune(8) applies (ADR 0476), and where in the
/// boot archive they go, beside the rest of the signed system.
pub const TUNE: &str = "userspace/tune";
pub const TUNE_DIR: &str = "system/resources/tune";

/// The security team's key that `report --security` seals reports to
/// (reportd(8), ADR 0531), and where in the boot archive it goes: where
/// reportd looks for it when report.toml(5) names no other file.
pub const REPORT: &str = "userspace/report";
pub const REPORT_DIR: &str = "system/resources/report";

/// The console's pages (admind(8), ADR 0514), and where in the boot
/// archive they go: the folder admind serves. Its `manifest.txt` lists
/// the SHA-256 of each file, and the folder `tests` beside the pages,
/// with the script tests, is left out.
pub const ADMIN: &str = "userspace/admin";
pub const ADMIN_DIR: &str = "system/resources/admin";
const ADMIN_TESTS: &str = "tests";

/// The error pages, one for each ID, and the folder of the console's
/// pages that the build copies them into, with `index.json`, a list of
/// each page's ID, title and area, for the help pages (`/help/errors`).
const ERROR_PAGES: &str = "docs/errors";
const ERROR_PAGES_DIR: &str = "system/resources/admin/help/errors";

/// The service files of the boot tests that Spark is pointed at with
/// `--service-dir` (spark(8)), and where in the boot archive they go.
pub const TEST_SERVICES: &str = "userspace/spark/tests/services";
pub const TEST_SERVICES_DIR: &str = "system/tests/services";

/// The answers files the install tests install by (ADR 0309), and where
/// they are in the boot archive.
pub const TEST_INSTALL: &str = "userspace/programs/installer/tests/install";
pub const TEST_INSTALL_DIR: &str = "system/tests/install";

/// The C and C++ test programs' sources, and where in the boot archive
/// they go.
pub const POSIX_SOURCES: &str = "userspace/posix";
pub const POSIX_DIR: &str = "system/tests/posix";

/// The user programs, built: the test image, the boot archive and the
/// startup archive.
pub struct Programs {
    /// The test image.
    pub usertests: PathBuf,
    /// The boot archive: the whole system, which a USB stick or a disc
    /// runs from memory.
    pub bootfs: PathBuf,
    /// The startup archive: Spark alone, which an installed system starts
    /// from (ADR 0307).
    pub startup: PathBuf,
    /// The ports, and the toolchain they and the C programs were built
    /// with: what the license manifest lists.
    pub ports: Ports,
}

/// Cargo arguments that build `core` for the user target.
pub fn target_args(ctx: &Ctx) -> Vec<String> {
    vec![
        "--target".into(),
        ctx.root.join(TARGET_SPEC).display().to_string(),
        "-Zjson-target-spec".into(),
        "-Zbuild-std=core,compiler_builtins,alloc".into(),
        "-Zbuild-std-features=compiler-builtins-mem".into(),
    ]
}

/// A Cargo command for the userspace workspace, writing to
/// `target/userspace`.
pub fn cargo(ctx: &Ctx, subcommand: &str) -> std::process::Command {
    let mut cmd = process::cargo(ctx);
    cmd.args([subcommand, "--manifest-path", "userspace/Cargo.toml"])
        .arg("--target-dir")
        .arg(ctx.target.join("userspace"))
        .env("CARGO_FUTURE_INCOMPAT_REPORT_FREQUENCY", "never");
    cmd
}

/// A Cargo command for the workspace of programs on Rust's standard
/// library, writing to `target/programs`: the toolchain's Cargo for
/// Radix, which builds for their own target against the sysroot the
/// rust-std port builds, with the crates the ports patch for Radix in
/// place of crates.io's, linked by the Radix C compiler (ADR 0070). It
/// points libsqlite3-sys at the sqlite port's library ([`sqlite_env`]).
pub fn std_cargo(ctx: &Ctx, subcommand: &str, toolchain: &ports::Toolchain) -> Command {
    let mut cmd = Command::new(toolchain.cargo());
    cmd.current_dir(&ctx.root)
        .args([subcommand, "--manifest-path", STD_MANIFEST])
        .env("CARGO_TARGET_DIR", ctx.target.join("programs"))
        .envs(sqlite_env(&ctx.out.join("sysroot")));
    cmd
}

/// The variables that make libsqlite3-sys, without its `bundled` feature,
/// link the sqlite port's `libsqlite3.a` from the sysroot `sysroot` and
/// read its `sqlite3.h` there, so that a program and the system's other
/// SQLite users have one library to update (ADR 0511). `SQLITE3_STATIC`
/// makes the link static, which every program on Radix is. The crate
/// asks pkg-config first, which a build for another target does not take
/// an answer from, and then links `sqlite3` from `SQLITE3_LIB_DIR`.
pub fn sqlite_env(sysroot: &Path) -> [(&'static str, PathBuf); 3] {
    [
        ("SQLITE3_LIB_DIR", sysroot.join("system/libraries")),
        ("SQLITE3_INCLUDE_DIR", sysroot.join("system/include")),
        ("SQLITE3_STATIC", PathBuf::from("1")),
    ]
}

/// Forgets what `target/programs` built for Radix against an older build
/// of the rust-std port, before Cargo builds there. Cargo does not see
/// Rust's standard library change under it, so it would keep crates
/// built against the old one, which rustc cannot load (E0463, E0460): a
/// change to the C library builds the port again, and CI restores a
/// target directory main built against main's.
///
/// # Errors
///
/// If the old builds cannot be taken away, or the port's stamp kept.
pub fn forget_stale_std(ctx: &Ctx) -> Result<()> {
    let stamp = std::fs::read_to_string(ctx.out.join("ports").join("rust-std.stamp"))
        .context(|| "reading the rust-std port's stamp".to_string())?;
    let dir = ctx.target.join("programs");
    let seen = dir.join("rust-std.stamp");
    if std::fs::read_to_string(&seen).ok().as_deref() == Some(stamp.as_str()) {
        return Ok(());
    }
    let built = dir.join(STD_TARGET_NAME);
    if built.exists() {
        step("Rust's standard library was built again: building its programs afresh");
        std::fs::remove_dir_all(&built).context(|| format!("removing {}", built.display()))?;
    }
    process::create_dir(&dir)?;
    ports::write_file(&seen, &stamp)
}

/// The workspace of programs on Rust's standard library.
pub const STD_MANIFEST: &str = "userspace/programs/Cargo.toml";

/// The programs on Rust's standard library whose unit tests run on the
/// host: what they test does not need Radix.
pub const HOST_TESTED: &[&str] = &[
    "accounts",
    "audio",
    "catalog-repo",
    "consoled",
    "disk",
    "dkv",
    "dukovo-install",
    "git-https-test",
    "kerberos",
    "keymap",
    "manifest",
    "netproto",
    "nfs",
    "nfsd",
    "nukesh",
    "slots",
    "smb",
    "smbd",
    "spark-api",
    "system-release",
    "systools",
    "tuf",
];

/// The Cargo.lock of the host builds of the programs on Rust's standard
/// library. They take the crates the ports patch for Radix from
/// crates.io, so they keep a lock of their own: with one lock for both,
/// each build locked the other's crates afresh, to crates.io's newest.
pub const HOST_LOCK: &str = "userspace/programs/host/Cargo.lock";

/// A Cargo command for the workspace of programs on Rust's standard
/// library, building for the host, writing to `target/programs-host`:
/// for their unit tests.
pub fn host_cargo(ctx: &Ctx, subcommand: &str) -> Command {
    host_cargo_of(ctx, subcommand, HOST_TESTED)
}

/// [`host_cargo`] for `packages` alone.
pub fn host_cargo_of(ctx: &Ctx, subcommand: &str, packages: &[&str]) -> Command {
    let mut cmd = process::cargo(ctx);
    cmd.args([subcommand, "--manifest-path", STD_MANIFEST])
        .arg("--config")
        .arg(format!(r#"resolver.lockfile-path="{}""#, ctx.root.join(HOST_LOCK).display()))
        .arg("--target-dir")
        .arg(ctx.target.join("programs-host"));
    for package in packages {
        cmd.args(["-p", package]);
    }
    cmd
}

/// Builds the program `package` of the workspace of programs on Rust's
/// standard library for Radix, optimised, as it is not one of the
/// image's, and gives its path: the update server's `catalog-repo`, which
/// a Dukovo server runs and builds nothing itself.
///
/// # Errors
///
/// If the ports' toolchain or the program does not build.
pub fn std_program(ctx: &Ctx, package: &str) -> Result<PathBuf> {
    install_headers(ctx)?;
    let ports = ports::build_all(ctx, &[])?;
    forget_stale_std(ctx)?;
    step(&format!("Building {package} for Radix"));
    let mut cmd = std_cargo(ctx, "build", &ports.toolchain);
    cmd.args(["-p", package, "--release"]);
    process::run(ctx, &mut cmd)?;
    check_patched(ctx, &ports.toolchain)?;
    Ok(ctx.target.join("programs").join(STD_TARGET_NAME).join("release").join(package))
}

/// Checks that the programs on Rust's standard library take each crate
/// the ports patch for Radix from the port, not from crates.io: Cargo
/// keeps a version Cargo.lock names that the port has no patch for.
fn check_patched(ctx: &Ctx, toolchain: &ports::Toolchain) -> Result<()> {
    let patches = toolchain.crate_patches();
    let mut patched: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&patches).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((name, version)) = name.strip_suffix(".toml").and_then(|n| n.rsplit_once('-')) {
            patched.push((name.to_string(), version.to_string()));
        }
    }
    let path = ctx.root.join("userspace/programs/Cargo.lock");
    let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let lock: toml::Value = toml::from_str(&text)
        .map_err(|err| crate::Error::new(format!("{}: {err}", path.display())))?;
    for package in lock.get("package").and_then(|p| p.as_array()).into_iter().flatten() {
        let field = |key: &str| package.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        let (name, version) = (field("name"), field("version"));
        if field("source").is_empty() {
            continue;
        }
        let versions: Vec<&str> =
            patched.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect();
        if !versions.is_empty() {
            return Err(crate::Error::new(format!(
                "userspace/programs uses {name} {version} from crates.io, which does not \
                 build for Radix"
            ))
            .hint(format!(
                "the ports patch {name} {}: run `cargo update --manifest-path {STD_MANIFEST} \
                 -p {name}@{version} --precise {}`",
                versions.join(" and "),
                versions[0]
            )));
        }
    }
    Ok(())
}

/// Where the test image is installed.
pub fn installed(ctx: &Ctx) -> PathBuf {
    ctx.out.join("sysroot").join(USERTESTS_PATH.trim_start_matches('/'))
}

/// The system call interface, compiled from `abi/*.ridl` by ridlc.
pub fn abi(ctx: &Ctx) -> Result<ridlc::Abi> {
    let dir = ctx.root.join("abi");
    ridlc::compile_dir(&dir).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|error| format!("    {error}")).collect();
        crate::Error::new(format!(
            "abi/*.ridl has {} problems:\n{}",
            errors.len(),
            lines.join("\n")
        ))
    })
}

/// Writes the C headers of the system call interface into
/// `out/sysroot/system/include/radix/`, where the system's headers are.
pub fn install_headers(ctx: &Ctx) -> Result<PathBuf> {
    let include = ctx.out.join("sysroot/system/include");
    for (name, text) in ridlc::c_headers(&abi(ctx)?) {
        let path = include.join(name);
        if let Some(parent) = path.parent() {
            process::create_dir(parent)?;
        }
        std::fs::write(&path, text).context(|| format!("writing {}", path.display()))?;
    }
    step(&format!("C headers of the system call interface in {}", ctx.display(&include)));
    Ok(include)
}

/// Dukovo's own catalog among the default settings, which bundled(8)
/// installs from (ADR 0383): its address, and the first root signed in
/// `catalog/`. None until that root is signed.
fn dukovo_catalog(ctx: &Ctx) -> Result<Vec<(String, PathBuf)>> {
    let first = ctx.root.join(crate::catalog::REPO).join("metadata/1.root.json");
    if !first.is_file() {
        return Ok(Vec::new());
    }
    let dir = ctx.out.join("catalog/default-settings");
    std::fs::create_dir_all(&dir).context(|| format!("making {}", dir.display()))?;
    let url = dir.join("url");
    std::fs::write(&url, format!("{}\n", crate::catalog::URL))
        .context(|| format!("writing {}", url.display()))?;
    std::fs::set_permissions(&url, Permissions::from_mode(0o644))
        .context(|| format!("writing {}", url.display()))?;
    let root = dir.join("root.json");
    std::fs::copy(&first, &root).context(|| format!("copying {}", first.display()))?;
    std::fs::set_permissions(&root, Permissions::from_mode(0o644))
        .context(|| format!("writing {}", root.display()))?;
    let at = format!("{DEFAULTS_DIR}/catalogs/dukovo");
    Ok(vec![(format!("{at}/url"), url), (format!("{at}/root.json"), root)])
}

/// Builds the ports, the C test programs and the user programs, installs
/// the test image as `out/sysroot/boot/usertests`, and writes the boot
/// archive to `out/sysroot/boot/bootfs`. The programs are the ones for
/// people, without the lab's routes: [`build_for`] with `lab` false.
pub fn build(ctx: &Ctx, profile: Profile) -> Result<Programs> {
    build_for(ctx, profile, false)
}

/// As [`build`], and with `lab` true builds systools with its `lab`
/// feature ([`LAB_FEATURE`]), which gives admind the route the lab's tests
/// run an elevated command by, `POST /api/1/test/elevate`, and the
/// console the long action `lab.long` that its task tests run (ADRs 0514
/// and 0516). Only the lab image is built so (`ob image --lab`,
/// [`crate::image::build_release`]): a lab test runs on that image alone
/// and needs those routes. Every other image, the boot tests' among them,
/// is a build for people, so nothing that ships has them. A build with
/// the feature and one without it share `target/programs`, and each puts
/// its own systools in place.
pub fn build_for(ctx: &Ctx, profile: Profile, lab: bool) -> Result<Programs> {
    install_headers(ctx)?;
    let ports = ports::build_all(ctx, &[])?;
    let mut files = build_posix(ctx, &ports.toolchain)?;
    install_tree(ctx, DEFAULTS, DEFAULTS_DIR, &[], &mut files)?;
    install_tree(ctx, FEATURES, FEATURES_DIR, &[], &mut files)?;
    install_tree(ctx, TROUBLESHOOT, TROUBLESHOOT_DIR, &[], &mut files)?;
    install_tree(ctx, TUNE, TUNE_DIR, &[], &mut files)?;
    install_tree(ctx, REPORT, REPORT_DIR, &[], &mut files)?;
    install_admin(ctx, &mut files)?;
    files.extend(dukovo_catalog(ctx)?);
    files.push(os_release(ctx)?);
    // The root this system trusts its releases by (ADR 0308).
    files.push(crate::release::trusted_root(ctx)?);
    install_tree(ctx, TEST_SERVICES, TEST_SERVICES_DIR, &[], &mut files)?;
    install_tree(ctx, TEST_INSTALL, TEST_INSTALL_DIR, &[], &mut files)?;
    for port in &ports.ports {
        files.extend(ports::image_files(ctx, &port.recipe, &ctx.out.join("sysroot"))?);
    }
    files.extend(build_std(ctx, profile, &ports.toolchain, lab)?);
    step(&format!("Building the user programs ({profile})"));
    let mut cmd = cargo(ctx, "build");
    cmd.args(["-p", "usertests"]);
    for (_, package) in ARCHIVE {
        if profile == Profile::Debug || *package != "spark" {
            cmd.args(["-p", package]);
        }
    }
    cmd.args(target_args(ctx));
    if profile == Profile::Release {
        cmd.arg("--release");
    }
    process::run(ctx, &mut cmd)?;
    let built = ctx.target.join("userspace").join(TARGET_NAME).join(profile.dir());
    let spark = match profile {
        Profile::Debug => built.join("spark"),
        Profile::Release => {
            let mut cmd = cargo(ctx, "build");
            cmd.args(["-p", "spark", "--profile", SPARK_PROFILE]);
            cmd.args(target_args(ctx));
            process::run(ctx, &mut cmd)?;
            ctx.target.join("userspace").join(TARGET_NAME).join(SPARK_PROFILE).join("spark")
        }
    };
    let installed = installed(ctx);
    if let Some(parent) = installed.parent() {
        process::create_dir(parent)?;
    }
    let usertests = built.join("usertests");
    std::fs::copy(&usertests, &installed)
        .context(|| format!("copying {} to {}", usertests.display(), installed.display()))?;
    let size = std::fs::metadata(&installed).map_or(0, |m| m.len());
    step(&format!(
        "User test programs installed at {} ({} KiB)",
        ctx.display(&installed),
        size / 1024
    ));
    let bootfs = ctx.out.join("sysroot").join(BOOTFS_PATH.trim_start_matches('/'));
    for (path, package) in ARCHIVE {
        let file = if *package == "spark" { spark.clone() } else { built.join(package) };
        files.push((path.to_string(), file));
    }
    crate::manuals::install(ctx, &mut files)?;
    // GRUB's images, which rgrub-setup(8) puts on disks (ADR 0306).
    let grub = crate::grub::build(ctx)?;
    let images = grub.disk_images(ctx, &ctx.out.join("sysroot").join(crate::grub::SYSTEM_DIR))?;
    for path in [images.boot, images.core, images.efi] {
        let name = path.file_name().expect("an image has a name").to_string_lossy().into_owned();
        files.push((format!("{}/{name}", crate::grub::SYSTEM_DIR), path));
    }
    // The menu dukovo-install(8) puts on the disks it installs (ADR 0309).
    let menu = crate::image::installer_menu(ctx, &ctx.out.join("release-menu"))?;
    files.push((format!("{}/{}", crate::grub::SYSTEM_DIR, crate::image::INSTALLER_MENU), menu));
    // Rescue's menu entry, which it puts beside that menu (ADR 0460).
    files.push((
        format!("{}/rescue.cfg", crate::grub::SYSTEM_DIR),
        ctx.root.join(crate::image::RESCUE_CFG),
    ));
    // The theme that menu draws itself with, from the slot (ADR 0505).
    let theme = ctx.out.join("release-theme");
    for name in crate::grub_theme::write(
        &theme,
        crate::VERSION,
        crate::grub_theme::INSTALLED_ENTRIES,
        crate::grub_theme::Place::Installed,
    )? {
        files.push((format!("system/{}/{name}", crate::image::SLOT_THEME), theme.join(name)));
    }
    let epoch = crate::image::source_date_epoch(ctx);
    let archive = write_archive(&files, epoch.clone())?;
    std::fs::write(&bootfs, &archive).context(|| format!("writing {}", bootfs.display()))?;
    step(&format!(
        "Boot archive of {} programs written to {} ({} KiB)",
        files.len(),
        ctx.display(&bootfs),
        archive.len() / 1024
    ));
    let startup = ctx.out.join("sysroot").join(STARTUP_PATH.trim_start_matches('/'));
    let archive = write_startup_archive(&spark, epoch)?;
    std::fs::write(&startup, &archive).context(|| format!("writing {}", startup.display()))?;
    step(&format!(
        "Startup archive written to {} ({} KiB)",
        ctx.display(&startup),
        archive.len() / 1024
    ));
    Ok(Programs { usertests: installed, bootfs, startup, ports })
}

/// Copies the files beneath `source`, a directory of the repository,
/// into `out/sysroot` at `dir`, in name order, and adds each one's place
/// in the boot archive and file to `files`. A directory whose name is in
/// `skip` is left out, wherever it is.
fn install_tree(
    ctx: &Ctx,
    source: &str,
    dir: &str,
    skip: &[&str],
    files: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    let from = ctx.root.join(source);
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(&from)
        .context(|| format!("reading {}", from.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let is_dir = entry.file_type().ok()?.is_dir();
            Some((entry.file_name().to_string_lossy().into_owned(), is_dir))
        })
        .collect();
    entries.sort();
    for (name, is_dir) in entries {
        if is_dir {
            if !skip.contains(&name.as_str()) {
                let (source, dir) = (format!("{source}/{name}"), format!("{dir}/{name}"));
                install_tree(ctx, &source, &dir, skip, files)?;
            }
            continue;
        }
        let installed = ctx.out.join("sysroot").join(dir).join(&name);
        process::copy(&from.join(&name), &installed)?;
        files.push((format!("{dir}/{name}"), installed));
    }
    Ok(())
}

/// Installs the console's pages like [`install_tree`], less their tests,
/// with the error pages and their `index.json` ([`install_error_pages`]),
/// and writes `manifest.txt` beside them, which admind checks the files
/// against when it starts (`systools/src/admind/files.rs`) and lists
/// every one of them in.
fn install_admin(ctx: &Ctx, files: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let first = files.len();
    install_tree(ctx, ADMIN, ADMIN_DIR, &[ADMIN_TESTS], files)?;
    install_error_pages(ctx, files)?;
    let manifest = admin_manifest(ctx, &files[first..])?;
    let path = ctx.out.join("sysroot").join(ADMIN_DIR).join("manifest.txt");
    std::fs::write(&path, manifest).context(|| format!("writing {}", path.display()))?;
    files.push((format!("{ADMIN_DIR}/manifest.txt"), path));
    Ok(())
}

/// One error page, as the help pages list it.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct ErrorPage {
    /// The ID, such as `ADM-123`, which names the page's file.
    id: String,
    /// The page's heading after the ID.
    title: String,
    /// The area of the ID's letters, in `docs/errors/README.md`'s table.
    area: String,
}

/// Whether `name` is an error ID: two to five capitals, a hyphen and three
/// digits.
fn is_error_id(name: &str) -> bool {
    let Some((letters, digits)) = name.split_once('-') else { return false };
    (2..=5).contains(&letters.len())
        && letters.bytes().all(|b| b.is_ascii_uppercase())
        && digits.len() == 3
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The area of each ID's letters in the table of `readme`, such as
/// `` | `UPD` | Updates of the system | 0.15.0 | ``: the area's name, which
/// is its words up to the first colon.
fn error_areas(readme: &str) -> std::collections::BTreeMap<String, String> {
    let mut areas = std::collections::BTreeMap::new();
    for line in readme.lines() {
        let Some(row) = line.strip_prefix('|') else { continue };
        let mut cells = row.split('|').map(str::trim);
        let (Some(letters), Some(area)) = (cells.next(), cells.next()) else { continue };
        let letters = letters.trim_matches('`');
        let named = area.split(':').next().unwrap_or(area).trim();
        let is_letters =
            (2..=5).contains(&letters.len()) && letters.bytes().all(|b| b.is_ascii_uppercase());
        if is_letters && !named.is_empty() {
            areas.insert(letters.to_string(), named.to_string());
        }
    }
    areas
}

/// The title of the page `text` of the error `id`: what follows the ID in
/// its first heading, `# ID: The title`.
fn error_title(id: &str, text: &str) -> Option<String> {
    let heading = text.lines().find_map(|line| line.strip_prefix("# "))?;
    let title = heading.strip_prefix(id)?.strip_prefix(':')?.trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// Copies each page of `docs/errors` (`UPD-101.md`, and not the README)
/// into [`ERROR_PAGES_DIR`] and writes `index.json` beside them: a list
/// of `{id, title, area}` in the order of the IDs, for the help pages.
/// Each file goes into `files` as [`install_tree`] puts its own, so the
/// manifest lists them. A page whose heading does not give its title, or
/// whose ID's letters are in no row of the README's table, stops the
/// build: the help pages cannot list it.
fn install_error_pages(ctx: &Ctx, files: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let from = ctx.root.join(ERROR_PAGES);
    let readme_path = from.join("README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .context(|| format!("reading {}", readme_path.display()))?;
    let areas = error_areas(&readme);
    let mut names: Vec<String> = std::fs::read_dir(&from)
        .context(|| format!("reading {}", from.display()))?
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.strip_suffix(".md").is_some_and(is_error_id))
        .collect();
    names.sort();
    let into = ctx.out.join("sysroot").join(ERROR_PAGES_DIR);
    let mut index = Vec::with_capacity(names.len());
    for name in names {
        let id = name.trim_end_matches(".md");
        let page = from.join(&name);
        let text =
            std::fs::read_to_string(&page).context(|| format!("reading {}", page.display()))?;
        let title = error_title(id, &text).ok_or_else(|| {
            Error::new(format!(
                "{}: the first heading is not `# {id}: The title`, which the console's help \
                 pages list the error by",
                page.display()
            ))
        })?;
        let letters = id.split_once('-').map_or(id, |(letters, _)| letters);
        let area = areas.get(letters).ok_or_else(|| {
            Error::new(format!(
                "{}: the letters {letters} are in no row of {}'s table, which gives the error \
                 pages' areas",
                page.display(),
                readme_path.display()
            ))
        })?;
        let installed = into.join(&name);
        process::copy(&page, &installed)?;
        files.push((format!("{ERROR_PAGES_DIR}/{name}"), installed));
        index.push(ErrorPage { id: id.to_string(), title, area: area.clone() });
    }
    let path = into.join("index.json");
    let mut json = serde_json::to_string(&index).expect("a list of strings is JSON");
    json.push('\n');
    std::fs::write(&path, json).context(|| format!("writing {}", path.display()))?;
    files.push((format!("{ERROR_PAGES_DIR}/index.json"), path));
    Ok(())
}

/// The text of the console's `manifest.txt` for `pages`, the files of
/// [`ADMIN_DIR`] as [`install_tree`] listed them: a line for each in the
/// order of their paths, the SHA-256 in hexadecimal, two spaces and the
/// path inside the folder, as `sha256sum` writes it.
fn admin_manifest(ctx: &Ctx, pages: &[(String, PathBuf)]) -> Result<String> {
    let mut lines = Vec::with_capacity(pages.len());
    for (place, file) in pages {
        let inside = place.strip_prefix(ADMIN_DIR).and_then(|rest| rest.strip_prefix('/'));
        let inside =
            inside.expect("install_tree lists each file beneath the folder it installs into");
        lines.push((inside.to_string(), process::sha256(ctx, file)?));
    }
    lines.sort();
    let lines: Vec<String> = lines.iter().map(|(path, sum)| format!("{sum}  {path}\n")).collect();
    Ok(lines.concat())
}

/// Where to get a Node new enough: some distributions' own package is
/// older (Ubuntu 24.04's is 18).
const NODE_HINT: &str = "install Node 20 or later, from the nodejs package where it is new enough or from \
     https://nodejs.org/, then run ./ob doctor";

/// Runs the script tests of the console's pages with Node's own test
/// runner. They are plain ES modules, so there is nothing to install.
pub fn test_admin_scripts(ctx: &Ctx) -> Result<()> {
    match doctor::node_installed() {
        None => {
            return Err(Error::new(
                "the console's script tests need Node 20 or later, and node is not installed",
            )
            .hint(NODE_HINT));
        }
        Some(version) if !doctor::node_is_new_enough(&version) => {
            return Err(Error::new(format!(
                "the console's script tests need Node 20 or later, and this node is {version}"
            ))
            .hint(NODE_HINT));
        }
        Some(_) => {}
    }
    let dir = ctx.root.join(ADMIN).join(ADMIN_TESTS);
    let mut tests = Vec::new();
    for entry in std::fs::read_dir(&dir).context(|| format!("reading {}", dir.display()))? {
        let path = entry.context(|| format!("reading {}", dir.display()))?.path();
        if path.to_string_lossy().ends_with(".test.mjs") {
            tests.push(path);
        }
    }
    tests.sort();
    // Node 22 does not take a folder, so each file is named.
    let mut cmd = Command::new("node");
    cmd.arg("--test").args(&tests).current_dir(&ctx.root);
    process::run(ctx, &mut cmd)
}

/// The feature of systools that [`build_for`] turns on for the lab image,
/// in the form Cargo takes in a workspace, `PACKAGE/FEATURE`
/// (userspace/programs/systools/Cargo.toml).
pub const LAB_FEATURE: &str = "systools/lab";

/// The arguments, after `build` and the manifest, of the Cargo command that
/// builds the programs of [`STD_ARCHIVE`]: each one's package, the lab
/// feature when `lab` is true ([`LAB_FEATURE`]), and `--release` for a
/// release.
fn std_build_args(profile: Profile, lab: bool) -> Vec<&'static str> {
    let mut args = Vec::new();
    for (_, package) in STD_ARCHIVE {
        args.extend(["-p", package]);
    }
    if lab {
        args.extend(["--features", LAB_FEATURE]);
    }
    if profile == Profile::Release {
        args.push("--release");
    }
    args
}

/// Builds the programs of [`STD_ARCHIVE`] into `out/sysroot`, with their
/// links and the shell's test scripts, and returns each one's place in the
/// boot archive and file. With `lab` true, systools has the lab's routes
/// ([`build_for`]).
fn build_std(
    ctx: &Ctx,
    profile: Profile,
    toolchain: &ports::Toolchain,
    lab: bool,
) -> Result<Vec<(String, PathBuf)>> {
    forget_stale_std(ctx)?;
    let routes = if lab { ", with the lab's routes" } else { "" };
    step(&format!("Building the programs on Rust's standard library ({profile}{routes})"));
    let mut cmd = std_cargo(ctx, "build", toolchain);
    cmd.args(std_build_args(profile, lab));
    process::run(ctx, &mut cmd)?;
    check_patched(ctx, toolchain)?;
    let built = ctx.target.join("programs").join(STD_TARGET_NAME).join(profile.dir());
    let mut files = Vec::new();
    for (path, package) in STD_ARCHIVE {
        let installed = ctx.out.join("sysroot").join(path);
        process::copy(&built.join(package), &installed)?;
        files.push((path.to_string(), installed));
    }
    for (path, target) in STD_LINKS {
        let link = ctx.out.join("sysroot").join(path);
        if link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link).context(|| format!("removing {}", link.display()))?;
        }
        std::os::unix::fs::symlink(target, &link)
            .context(|| format!("linking {} to {target}", link.display()))?;
        files.push((path.to_string(), link));
    }
    let cases = ctx.root.join(SHELL_CASES);
    let mut names: Vec<String> = std::fs::read_dir(&cases)
        .context(|| format!("reading {}", cases.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|name| matches!(Path::new(name).extension(), Some(e) if e == "sh" || e == "out"))
        .collect();
    names.sort();
    for name in names {
        let installed = ctx.out.join("sysroot").join(SHELL_CASES_DIR).join(&name);
        process::copy(&cases.join(&name), &installed)?;
        files.push((format!("{SHELL_CASES_DIR}/{name}"), installed));
    }
    Ok(files)
}

/// Builds each C test program of [`POSIX_SOURCES`] (one per `.c` file,
/// with the headers there), and each C++ one (one per `.cpp` file, on
/// libc++, ADR 0090), into `out/sysroot/system/tests/posix`, and
/// the dynamically linked one of its `dynamic` directory (ADR 0071), and
/// returns each one's place in the boot archive and file.
fn build_posix(ctx: &Ctx, toolchain: &ports::Toolchain) -> Result<Vec<(String, PathBuf)>> {
    let sources = ctx.root.join(POSIX_SOURCES);
    let out = ctx.out.join("sysroot").join(POSIX_DIR);
    process::create_dir(&out)?;
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&sources)
        .context(|| format!("reading {}", sources.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "c" || e == "cpp"))
        .collect();
    programs.sort();
    step(&format!("Building {} C and C++ test programs", programs.len()));
    let mut files = Vec::new();
    for source in programs {
        let name = source.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let program = out.join(&name);
        let (compiler, standard) = if source.extension().is_some_and(|e| e == "cpp") {
            (toolchain.cxx(), "-std=c++23")
        } else {
            (toolchain.cc(), "-std=c11")
        };
        let mut cmd = Command::new(compiler);
        cmd.args([standard, "-D_GNU_SOURCE"])
            .args(["-O2", "-g", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(&sources)
            .arg("-o")
            .arg(&program)
            .arg(&source);
        process::run(ctx, &mut cmd)?;
        files.push((format!("{POSIX_DIR}/{name}"), program));
    }
    files.extend(build_dynamic(ctx, toolchain)?);
    Ok(files)
}

/// Builds the `dynamic` test as a bundle's program is built (ADR 0071):
/// the program, on the shared C library, with its private library in
/// `lib` beside it, found through its run path, and a library it opens
/// with dlopen there too.
fn build_dynamic(ctx: &Ctx, toolchain: &ports::Toolchain) -> Result<Vec<(String, PathBuf)>> {
    let posix = ctx.root.join(POSIX_SOURCES);
    let sources = posix.join("dynamic");
    let out = ctx.out.join("sysroot").join(POSIX_DIR);
    let lib = out.join("lib");
    process::create_dir(&lib)?;
    step("Building the dynamically linked C test program and its libraries");
    // Each one's file, mode, source and further arguments.
    let parts: [(PathBuf, &str, &str, &[&str]); 3] = [
        (lib.join("libgreet.so"), "-shared", "greet.c", &["-fPIC", "-Wl,-soname,libgreet.so"]),
        (lib.join("plugin.so"), "-shared", "plugin.c", &["-fPIC", "-lgreet", "-Wl,-rpath,$ORIGIN"]),
        (out.join("dynamic"), "-dynamic", "main.c", &["-lgreet", "-Wl,-rpath,$ORIGIN/lib"]),
    ];
    let mut files = Vec::new();
    for (file, mode, source, args) in parts {
        let mut cmd = Command::new(toolchain.cc());
        cmd.arg(mode)
            .args(["-std=c11", "-D_GNU_SOURCE"])
            .args(["-O2", "-g", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(&posix)
            .arg("-I")
            .arg(&sources)
            .arg("-o")
            .arg(&file)
            .arg(sources.join(source))
            .arg("-L")
            .arg(&lib)
            .args(args);
        process::run(ctx, &mut cmd)?;
        let inside = file.strip_prefix(ctx.out.join("sysroot")).unwrap_or(&file);
        files.push((inside.to_string_lossy().into_owned(), file));
    }
    Ok(files)
}

/// What a path of the boot archive is.
enum Entry<'a> {
    Directory,
    /// A file, or a symbolic link, on the host.
    File(&'a Path),
    /// A symbolic link with this target.
    Link(&'a str),
    /// A file with this text.
    Text(&'a str),
}

/// The boot archive: `files`, each a path in the archive and the file
/// it holds (a symbolic link stays one, with its target), the
/// directories above them, those of [`ARCHIVE_DIRS`], the links of
/// [`ARCHIVE_LINKS`] and [`DEFAULT_LINKS`] and [`ARCHIVE_HIDDEN`], owned by root and dated
/// `epoch` (the commit's time), so the same programs make the same
/// bytes. A file is 0755 if its source may be run by anyone (a program
/// or a script), and 0644 otherwise (settings, manual pages, a test's
/// expected output), as Linux installs them. No file may be under one of
/// the links: it belongs in the folder the link leads to.
fn write_archive(files: &[(String, PathBuf)], epoch: Option<String>) -> Result<Vec<u8>> {
    let mtime = epoch.and_then(|epoch| epoch.parse().ok()).unwrap_or(0);
    let mut entries: Vec<(String, Entry<'_>)> = Vec::new();
    for dir in ARCHIVE_DIRS {
        entries.push((dir.to_string(), Entry::Directory));
    }
    let mut leaves: Vec<(String, Entry<'_>)> = Vec::new();
    for (link, target) in ARCHIVE_LINKS.iter().chain(DEFAULT_LINKS) {
        leaves.push((link.to_string(), Entry::Link(target)));
    }
    leaves.push((ARCHIVE_HIDDEN.0.to_string(), Entry::Text(ARCHIVE_HIDDEN.1)));
    for (path, file) in files {
        if let Some((link, _)) = ARCHIVE_LINKS
            .iter()
            .find(|(link, _)| path.strip_prefix(link).is_some_and(|rest| rest.starts_with('/')))
        {
            return Err(crate::Error::new(format!(
                "the boot archive: {path} is under the link /{link}"
            )));
        }
        leaves.push((path.clone(), Entry::File(file)));
    }
    for (path, entry) in leaves {
        // Each directory above it, once.
        for (end, _) in path.match_indices('/') {
            let dir = &path[..end];
            if !entries.iter().any(|(seen, _)| seen == dir) {
                entries.push((dir.to_string(), Entry::Directory));
            }
        }
        entries.push((path, entry));
    }
    // A directory before what is in it, as the archive's reader wants.
    entries.sort_by(|a, b| a.0.split('/').cmp(b.0.split('/')));
    let mut writer = radix_bootfs::write::Writer::new();
    for (path, entry) in &entries {
        let written = match entry {
            Entry::Directory => writer.directory(path, 0o755, mtime),
            Entry::Link(target) => writer.symlink(path, target, mtime),
            Entry::Text(text) => writer.file(path, 0o644, mtime, text.as_bytes()),
            Entry::File(file) if file.is_symlink() => {
                let target =
                    std::fs::read_link(file).context(|| format!("reading {}", file.display()))?;
                let target = target.to_str().ok_or_else(|| {
                    crate::Error::new(format!("the link {} is not UTF-8", file.display()))
                })?;
                writer.symlink(path, target, mtime)
            }
            Entry::File(file) => {
                let data = std::fs::read(file).context(|| format!("reading {}", file.display()))?;
                let mode = if is_executable(file)? { 0o755 } else { 0o644 };
                writer.file(path, mode, mtime, &data)
            }
        };
        written.map_err(|err| crate::Error::new(format!("the boot archive: {path}: {err}")))?;
    }
    let archive = writer.finish();
    // The kernel checks the archive before it uses any of it, and stops
    // the boot when it does not pass: check it as the kernel will.
    radix_bootfs::open(&archive).map_err(|err| {
        crate::Error::new(format!("the boot archive of {} entries: {err}", entries.len()))
    })?;
    Ok(archive)
}

/// The startup archive (ADR 0307): Spark at [`STARTUP_PROGRAM`] and the
/// directories of [`ARCHIVE_DIRS`], which Spark mounts memory on if no
/// disk holds the root it is told to mount, dated `epoch` as the boot
/// archive is. Everything else comes from the installed system's root, so
/// the archive the boot loader keeps in memory for the machine's whole
/// life is no bigger than Spark.
fn write_startup_archive(spark: &Path, epoch: Option<String>) -> Result<Vec<u8>> {
    let mtime = epoch.and_then(|epoch| epoch.parse().ok()).unwrap_or(0);
    let program = std::fs::read(spark).context(|| format!("reading {}", spark.display()))?;
    // Each path, and the program's bytes for the one file.
    let mut entries: Vec<(&str, Option<&[u8]>)> =
        ARCHIVE_DIRS.iter().map(|&dir| (dir, None)).collect();
    entries.extend([("system", None), ("system/commands", None)]);
    entries.push((STARTUP_PROGRAM, Some(&program)));
    // In path order, a directory before what is in it, as the archive's
    // reader wants.
    entries.sort_by(|a, b| a.0.split('/').cmp(b.0.split('/')));
    let mut writer = radix_bootfs::write::Writer::new();
    for (path, data) in entries {
        let written = match data {
            Some(data) => writer.file(path, 0o755, mtime, data),
            None => writer.directory(path, 0o755, mtime),
        };
        written.map_err(|err| crate::Error::new(format!("the startup archive: {path}: {err}")))?;
    }
    Ok(writer.finish())
}

/// Whether the file at `path` has an execute bit set.
fn is_executable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::metadata(path).context(|| format!("reading {}", path.display()))?;
    Ok(meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// The tools systools starts itself, from its own program, and never
    /// by a name in /system/commands: cert's responder of HTTP-01 (ADR
    /// 0513).
    const STARTED_BY_SYSTOOLS: [&str; 1] = ["acme-answer"];

    /// Every tool in systools' table has a link in /system/commands, so a
    /// new one is in the image; a tool left out is not found at boot.
    #[test]
    fn each_systools_tool_has_its_command() {
        let library = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../userspace/programs/systools/src/lib.rs");
        let text = std::fs::read_to_string(&library).unwrap();
        let start = text.find("const TOOLS: &[(&str, Tool)] = &[").expect("systools' table");
        let table = &text[start..start + text[start..].find("\n];").expect("the table's end")];
        let tools: Vec<&str> = table
            .lines()
            .filter_map(|line| line.trim().strip_prefix("(\"")?.split('"').next())
            .collect();
        assert!(tools.len() > 100, "read {} tools", tools.len());
        let missing: Vec<&str> = tools
            .into_iter()
            .filter(|tool| !STARTED_BY_SYSTOOLS.contains(tool))
            .filter(|tool| {
                !STD_LINKS.contains(&(format!("system/commands/{tool}").as_str(), "systools"))
            })
            .collect();
        assert!(missing.is_empty(), "add these to STD_LINKS: {missing:?}");
    }

    #[test]
    fn programs_on_std_link_the_sysroots_sqlite() {
        let env = sqlite_env(Path::new("/build/out/sysroot"));
        assert_eq!(
            env,
            [
                ("SQLITE3_LIB_DIR", PathBuf::from("/build/out/sysroot/system/libraries")),
                ("SQLITE3_INCLUDE_DIR", PathBuf::from("/build/out/sysroot/system/include")),
                ("SQLITE3_STATIC", PathBuf::from("1")),
            ]
        );
        assert!(STD_ARCHIVE.contains(&("system/tests/posix/sqlite-test", "sqlite-test")));
    }

    #[test]
    fn the_lab_feature_is_on_the_command_line_of_a_lab_build_alone() {
        let people = std_build_args(Profile::Debug, false);
        assert!(!people.contains(&"--features"), "{people:?}");
        assert!(!people.iter().any(|a| a.contains("lab")), "{people:?}");
        let lab = std_build_args(Profile::Debug, true);
        let at = lab.iter().position(|a| *a == "--features");
        assert_eq!(at.map(|i| lab[i + 1]), Some("systools/lab"), "{lab:?}");
        assert_eq!(lab.iter().filter(|a| a.contains("lab")).count(), 1, "{lab:?}");
        // The feature's package is one of those Cargo builds, or it refuses.
        let packages: Vec<&str> = lab.windows(2).filter(|w| w[0] == "-p").map(|w| w[1]).collect();
        assert_eq!(packages.len(), STD_ARCHIVE.len());
        assert!(packages.contains(&LAB_FEATURE.split('/').next().unwrap()));
        // Nothing else changes: the same packages, in the same order.
        let without: Vec<&str> =
            lab.iter().copied().filter(|a| *a != "--features" && *a != LAB_FEATURE).collect();
        assert_eq!(without, people);
    }

    #[test]
    fn a_release_adds_its_flag_to_either_build() {
        for lab in [false, true] {
            let debug = std_build_args(Profile::Debug, lab);
            let release = std_build_args(Profile::Release, lab);
            assert!(!debug.contains(&"--release"), "{debug:?}");
            assert_eq!(release.last(), Some(&"--release"), "{release:?}");
            assert_eq!(release[..release.len() - 1], debug[..], "{lab}");
        }
    }

    #[test]
    fn systools_has_the_feature_the_lab_build_turns_on() {
        let (package, feature) = LAB_FEATURE.split_once('/').unwrap();
        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../userspace/programs/systools/Cargo.toml");
        let manifest = std::fs::read_to_string(path).unwrap();
        assert_eq!(package, "systools");
        let features = manifest.split("\n[features]\n").nth(1).expect("a [features] table");
        let table = features.split("\n[").next().unwrap();
        assert!(
            table.lines().any(|line| line.trim_start().starts_with(&format!("{feature} = "))),
            "{table}"
        );
    }

    #[test]
    fn the_archive_holds_the_programs_under_their_directories() {
        let dir = std::env::temp_dir().join(format!("ob-archive-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut files = Vec::new();
        for (path, package) in ARCHIVE {
            std::fs::write(dir.join(package), format!("the {package} program")).unwrap();
            std::fs::set_permissions(dir.join(package), Permissions::from_mode(0o755)).unwrap();
            files.push((path.to_string(), dir.join(package)));
        }
        std::fs::write(dir.join("hosts"), "127.0.0.1 localhost\n").unwrap();
        std::fs::set_permissions(dir.join("hosts"), Permissions::from_mode(0o644)).unwrap();
        files.push(("system/settings/hosts".to_string(), dir.join("hosts")));
        std::os::unix::fs::symlink("spark", dir.join("init")).unwrap();
        files.push(("system/commands/init".to_string(), dir.join("init")));
        let bytes = write_archive(&files, Some("1700000000".into())).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let archive = radix_bootfs::open(&bytes).unwrap();
        let paths: Vec<String> = archive.entries().map(|e| e.path().to_string()).collect();
        assert_eq!(
            paths,
            [
                "system",
                "system/.hidden",
                "system/bin",
                "system/boot",
                "system/commands",
                "system/commands/init",
                "system/commands/spark",
                "system/lib",
                "system/live",
                "system/resources",
                "system/resources/default-settings",
                "system/resources/default-settings/localtime",
                "system/resources/default-settings/os-release",
                "system/resources/default-settings/resolv.conf",
                "system/resources/default-settings/ssl",
                "system/resources/default-settings/ssl/cert.pem",
                "system/resources/default-settings/ssl/certs",
                "system/resources/default-settings/ssl/certs/ca-certificates.crt",
                "system/settings",
                "system/settings/hosts",
                "system/share",
                "system/state",
                "system/tests",
                "system/tests/hello",
                "system/tests/sockets",
                "system/tests/storage"
            ]
        );
        let spark = archive.find("/system/commands/spark").unwrap();
        assert_eq!(spark.data, b"the spark program");
        assert_eq!((spark.mode, spark.mtime), (0o755, 1_700_000_000));
        assert_eq!(archive.find("/system/settings/hosts").unwrap().mode, 0o644);
        let init = archive.find("/system/commands/init").unwrap();
        assert_eq!((init.kind, init.data), (radix_bootfs::Kind::Symlink, &b"spark"[..]));
        let bin = archive.find("/system/bin").unwrap();
        assert_eq!((bin.kind, bin.data), (radix_bootfs::Kind::Symlink, &b"commands"[..]));
        let resolv = archive.find("/system/resources/default-settings/resolv.conf").unwrap();
        let stub = &b"/system/live/resolved/resolv.conf"[..];
        assert_eq!((resolv.kind, resolv.data), (radix_bootfs::Kind::Symlink, stub));
        let os_release = archive.find("/system/resources/default-settings/os-release").unwrap();
        let usr_lib = &b"/usr/lib/os-release"[..];
        assert_eq!((os_release.kind, os_release.data), (radix_bootfs::Kind::Symlink, usr_lib));
        let localtime = archive.find("/system/resources/default-settings/localtime").unwrap();
        let utc = &b"/system/resources/zoneinfo/UTC"[..];
        assert_eq!((localtime.kind, localtime.data), (radix_bootfs::Kind::Symlink, utc));
        for name in ["ssl/cert.pem", "ssl/certs/ca-certificates.crt"] {
            let link = archive.find(&format!("/system/resources/default-settings/{name}")).unwrap();
            let roots = CERTIFICATES.as_bytes();
            assert_eq!((link.kind, link.data), (radix_bootfs::Kind::Symlink, roots), "{name}");
        }
        let hidden = archive.find("/system/.hidden").unwrap();
        assert_eq!((hidden.mode, hidden.data), (0o644, &b"bin\nlib\nshare\n"[..]));
        assert_eq!(write_archive(&files, None).ok(), None, "the programs are gone");
        let under_a_link = [("system/bin/ls".to_string(), dir.join("ls"))];
        assert!(write_archive(&under_a_link, None).is_err(), "a file under /system/bin");
    }

    #[test]
    fn the_consoles_pages_are_installed_with_a_manifest_and_without_their_tests() {
        let root = std::env::temp_dir().join(format!("ob-admin-test-{}", std::process::id()));
        let pages = root.join(ADMIN);
        std::fs::create_dir_all(pages.join("css")).unwrap();
        std::fs::create_dir_all(pages.join("tests")).unwrap();
        std::fs::write(pages.join("index.html"), "<html>").unwrap();
        std::fs::write(pages.join("app.js"), "console.log(1)").unwrap();
        std::fs::write(pages.join("css/console.css"), "body{}").unwrap();
        std::fs::write(pages.join("tests/app.test.mjs"), "a test").unwrap();
        let errors = root.join(ERROR_PAGES);
        std::fs::create_dir_all(&errors).unwrap();
        for (name, text) in ERROR_FIXTURE {
            std::fs::write(errors.join(name), text).unwrap();
        }
        let ctx = Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root: root.clone(),
            config: crate::config::Config::default(),
            verbose: false,
        };
        let other = PathBuf::from("a file that is not a page");
        let mut files = vec![("system/commands/spark".to_string(), other.clone())];
        let installed = install_admin(&ctx, &mut files);
        let placed: Vec<&str> = files.iter().map(|(place, _)| place.as_str()).collect();
        let sysroot = root.join("out/sysroot").join(ADMIN_DIR);
        let manifest = std::fs::read_to_string(sysroot.join("manifest.txt"));
        let left_out = sysroot.join(ADMIN_TESTS).exists();
        let index = std::fs::read_to_string(sysroot.join("help/errors/index.json"));
        let copied = std::fs::read_to_string(sysroot.join("help/errors/ADM-101.md"));
        let readme_copied = sysroot.join("help/errors/README.md").exists();
        let notes_copied = sysroot.join("help/errors/notes.md").exists();
        std::fs::remove_dir_all(&root).unwrap();
        installed.unwrap();
        assert_eq!(
            placed,
            [
                "system/commands/spark",
                "system/resources/admin/app.js",
                "system/resources/admin/css/console.css",
                "system/resources/admin/index.html",
                "system/resources/admin/help/errors/ADM-101.md",
                "system/resources/admin/help/errors/UPD-101.md",
                "system/resources/admin/help/errors/index.json",
                "system/resources/admin/manifest.txt",
            ]
        );
        assert!(!left_out, "the tests folder is not installed");
        // The error pages are copied as they are, the README and a file
        // that is no page are not, and the index lists each page.
        assert_eq!(copied.unwrap(), ERROR_FIXTURE[1].1);
        assert!(!readme_copied && !notes_copied);
        assert_eq!(
            index.unwrap(),
            "[{\"id\":\"ADM-101\",\"title\":\"The name or the password is wrong\",\
             \"area\":\"The web console, Dukovo Admin\"},\
             {\"id\":\"UPD-101\",\"title\":\"The update does not fit: free space\",\
             \"area\":\"Updates of the system\"}]\n"
        );
        // The sums are `sha256sum`'s, for the files' contents; the line of
        // a file in a folder names its path inside the pages' folder, and the
        // manifest lists the error pages and their index, and does not list
        // itself or files that are not pages.
        assert_eq!(
            manifest.unwrap(),
            "0a286891c11c056e1ab5bfc25bf5d6b2f5b06d38eac10944f678fd8a2e70c393  app.js\n\
             7c98040a541657584690ae2a1cc3b42a8b53b159cc60c5d3abbfecbaeac6c94a  css/console.css\n\
             352ef3b530c670a4dd3483bd9589d50c82593ec34a66fa886984754df3d2ea43  help/errors/ADM-101.md\n\
             f73461180c58f3179e5c504a880f3b4540e7dc542c5c112d1fa487d96555def7  help/errors/UPD-101.md\n\
             80a9bb19e975d1c0922a597ecf9aeadbedb678e76ce8344aa1cee074c28718ef  help/errors/index.json\n\
             b7d082ee12e91b756ea22e8513b8594eebcf5d39fab813da3cb55794dc888ad7  index.html\n"
        );
        assert_eq!(files[0].1, other);
    }

    /// The files of a `docs/errors` for the tests: a README with the table
    /// of areas, two pages, and a file that is no page.
    const ERROR_FIXTURE: [(&str, &str); 4] = [
        (
            "README.md",
            "# Error pages\n\n| Letters | Area | Since |\n| --- | --- | --- |\n\
             | `ADM` | The web console, Dukovo Admin: its sign-in | 0.16.0 |\n\
             | `UPD` | Updates of the system | 0.15.0 |\n",
        ),
        ("ADM-101.md", "# ADM-101: The name or the password is wrong\n\nWhat it means.\n"),
        ("UPD-101.md", "# UPD-101: The update does not fit: free space\n\nWhat it means.\n"),
        ("notes.md", "# Notes\n"),
    ];

    /// A context whose root holds `docs/errors` of `files`, in a folder of
    /// the test's own.
    fn errors_ctx(what: &str, files: &[(&str, &str)]) -> Ctx {
        let root = std::env::temp_dir().join(format!("ob-errors-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let errors = root.join(ERROR_PAGES);
        std::fs::create_dir_all(&errors).unwrap();
        for (name, text) in files {
            std::fs::write(errors.join(name), text).unwrap();
        }
        Ctx {
            out: root.join("out"),
            target: root.join("target"),
            root,
            config: crate::config::Config::default(),
            verbose: false,
        }
    }

    #[test]
    fn an_error_is_named_by_its_letters_and_three_digits() {
        for id in ["ADM-101", "UPD-101", "FW-101", "ABCDE-999"] {
            assert!(is_error_id(id), "{id}");
        }
        for odd in [
            "README",
            "adm-101",
            "A-101",
            "ABCDEF-101",
            "ADM-10",
            "ADM-1011",
            "ADM-10a",
            "ADM",
            "-101",
            "",
        ] {
            assert!(!is_error_id(odd), "{odd}");
        }
    }

    #[test]
    fn the_areas_are_the_names_in_the_readmes_table_up_to_the_first_colon() {
        let areas = error_areas(ERROR_FIXTURE[0].1);
        assert_eq!(areas.len(), 2);
        assert_eq!(areas["ADM"], "The web console, Dukovo Admin");
        assert_eq!(areas["UPD"], "Updates of the system");
        // The header and the rule are no rows, and words that are not
        // capitals are not letters.
        let text = "| Letters | Area | Since |\n| --- | --- | --- |\n| `x` | no | 1 |\n\
                    | `NET` | Networking: the layers | 0.16.0 |\n| `HW` |\n";
        let areas = error_areas(text);
        assert_eq!(areas.keys().collect::<Vec<_>>(), ["NET"]);
        assert_eq!(areas["NET"], "Networking");
    }

    #[test]
    fn a_title_is_what_follows_the_id_in_the_first_heading() {
        let id = "ADM-105";
        let page = |text: &str| error_title(id, text);
        assert_eq!(
            page("# ADM-105: Too many tries: wait a moment\n\ntext"),
            Some("Too many tries: wait a moment".into())
        );
        assert_eq!(page("<!-- c -->\n\n# ADM-105:   Spaced  \n"), Some("Spaced".into()));
        assert_eq!(page("text\n# ADM-105: Later heading\n"), Some("Later heading".into()));
        for odd in [
            "",
            "# ADM-105\n",
            "# ADM-105:\n",
            "# ADM-105:  \n",
            "# ADM-106: Another\n",
            "## ADM-105: Second level\n",
            "ADM-105: no hash\n",
        ] {
            assert_eq!(page(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn a_page_the_help_pages_cannot_list_stops_the_build() {
        let readme = ERROR_FIXTURE[0].1;
        let adm = ERROR_FIXTURE[1];
        // A page with no title.
        let ctx = errors_ctx("no-title", &[("README.md", readme), ("ADM-101.md", "# ADM-101\n")]);
        let said = install_error_pages(&ctx, &mut Vec::new()).unwrap_err().to_string();
        std::fs::remove_dir_all(&ctx.root).unwrap();
        assert!(said.contains("ADM-101.md") && said.contains("# ADM-101: The title"), "{said}");
        // An area that the table does not have.
        let ctx = errors_ctx(
            "no-area",
            &[("README.md", readme), adm, ("XYZ-101.md", "# XYZ-101: A title\n")],
        );
        let said = install_error_pages(&ctx, &mut Vec::new()).unwrap_err().to_string();
        std::fs::remove_dir_all(&ctx.root).unwrap();
        assert!(said.contains("XYZ-101.md") && said.contains("the letters XYZ"), "{said}");
        // No README, and no folder.
        let ctx = errors_ctx("no-readme", &[adm]);
        let said = install_error_pages(&ctx, &mut Vec::new()).unwrap_err().to_string();
        std::fs::remove_dir_all(&ctx.root).unwrap();
        assert!(said.contains("README.md"), "{said}");
        let ctx = errors_ctx("no-folder", &[]);
        std::fs::remove_dir_all(ctx.root.join(ERROR_PAGES)).unwrap();
        let said = install_error_pages(&ctx, &mut Vec::new()).unwrap_err().to_string();
        std::fs::remove_dir_all(&ctx.root).unwrap();
        assert!(said.contains("reading "), "{said}");
    }

    /// The tree's own pages are what the build takes: each has a title and
    /// an area, so that `ob build` does not stop at one.
    #[test]
    fn every_error_page_of_the_tree_has_a_title_and_an_area_the_help_pages_can_list() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let ctx = errors_ctx("tree", &[]);
        let ctx = Ctx { root, ..ctx };
        let mut files = Vec::new();
        let done = install_error_pages(&ctx, &mut files);
        let index = std::fs::read_to_string(
            ctx.out.join("sysroot").join(ERROR_PAGES_DIR).join("index.json"),
        );
        let _ = std::fs::remove_dir_all(ctx.out.parent().unwrap());
        done.unwrap();
        let index: Vec<serde_json::Value> = serde_json::from_str(&index.unwrap()).unwrap();
        // One page for each ID, and the index with them.
        assert_eq!(files.len(), index.len() + 1);
        assert!(index.len() > 70, "{}", index.len());
        let ids: Vec<&str> = index.iter().map(|page| page["id"].as_str().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        for page in &index {
            let id = page["id"].as_str().unwrap();
            assert!(is_error_id(id), "{id}");
            for key in ["title", "area"] {
                assert!(!page[key].as_str().unwrap().is_empty(), "{id} {key}");
            }
        }
        let adm = index.iter().find(|page| page["id"] == "ADM-129").unwrap();
        assert_eq!(adm["title"], "This server does not have that command yet");
        assert_eq!(adm["area"], "The web console, Dukovo Admin");
    }

    #[test]
    fn os_release_names_dukovo_and_its_version_in_the_files_grammar() {
        let text = os_release_text("0.15.0");
        let pairs: Vec<(&str, &str)> =
            text.lines().map(|line| line.split_once('=').expect("NAME=value")).collect();
        let get = |name: &str| pairs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
        assert_eq!(get("ID"), Some("dukovo"));
        assert_eq!(get("NAME"), Some("\"Dukovo\""));
        assert_eq!(get("VERSION_ID"), Some("\"0.15.0\""));
        assert_eq!(get("VERSION"), Some("\"0.15.0\""));
        assert_eq!(get("PRETTY_NAME"), Some("\"Dukovo 0.15.0\""));
        for name in ["HOME_URL", "SUPPORT_URL", "BUG_REPORT_URL"] {
            assert!(get(name).is_some_and(|url| url.starts_with("\"https://")), "{name}");
        }
        for (name, value) in &pairs {
            let valid_name = name.starts_with(|c: char| c.is_ascii_uppercase())
                && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            assert!(valid_name, "{name} is not a variable name");
            let quoted = value.starts_with('"') && value.ends_with('"');
            assert!(quoted || !value.contains(' '), "{name}: a value with a space is quoted");
        }
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn the_release_carries_os_release_where_usr_lib_leads() {
        // /usr leads to /system and /system/lib to libraries.
        let lib = ARCHIVE_LINKS.iter().find(|(link, _)| *link == "system/lib").unwrap();
        assert_eq!(OS_RELEASE, format!("system/{}/os-release", lib.1));
        let link = DEFAULT_LINKS.iter().find(|(link, _)| link.ends_with("/os-release")).unwrap();
        assert_eq!(link.1, "/usr/lib/os-release");
    }

    #[test]
    fn the_startup_archive_holds_spark_and_the_mount_points_alone() {
        let dir = std::env::temp_dir().join(format!("ob-startup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spark = dir.join("spark");
        std::fs::write(&spark, "the spark program").unwrap();
        let bytes = write_startup_archive(&spark, Some("1700000000".into())).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let archive = radix_bootfs::open(&bytes).unwrap();
        let paths: Vec<String> = archive.entries().map(|e| e.path().to_string()).collect();
        assert_eq!(
            paths,
            [
                "system",
                "system/boot",
                "system/commands",
                "system/commands/spark",
                "system/live",
                "system/settings",
                "system/state"
            ]
        );
        let program = archive.find("/system/commands/spark").unwrap();
        assert_eq!(program.data, b"the spark program");
        assert_eq!((program.mode, program.mtime), (0o755, 1_700_000_000));
        assert!(write_startup_archive(&dir.join("gone"), None).is_err(), "no Spark to put in");
    }
}
