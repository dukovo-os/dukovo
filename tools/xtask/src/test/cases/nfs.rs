// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfs(1), mount(8) and umount(8) against the host's own Linux NFS server
//! (ADR 0344): the host exports two folders of the test's to its
//! loopback, where QEMU's user network reaches it from 10.0.2.2, one to
//! read and write, owned by number 1000, and one only to read, and names
//! them on 10.0.2.100 port 79. _/system/tests/posix/nfstest_ refuses
//! command lines nfs cannot take, lists the folder, mounts it with the
//! command `mount -t nfs`, where root, served as no one, may not write; as
//! number 1000 writes, renames, links, changes modes and copies a 4 MiB file;
//! and mounts the read-only folder, which refuses writes. The host finds
//! what the guest wrote, as number 1000's. Exporting needs root, with
//! sudo where the test does not run as root, and Linux's server
//! (nfs-kernel-server); where either is missing, as in `ob --container`,
//! the guest checks only its command lines.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::super::*;

/// The test's folders, outside the tree, which may be in a home the
/// server's no one cannot enter.
fn dir() -> PathBuf {
    std::env::temp_dir().join("dukovo-nfs-test")
}

/// What QEMU runs for each connection to 10.0.2.100 port 79, from the
/// top of the tree: it names the folders, or says there are none.
const ABOUT: &str = "out/test/nfs/about";

/// The test's network: QEMU's user network, the host at 10.0.2.2, and
/// what nfstest needs to know at 10.0.2.100 port 79.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:79-cmd:out/test/nfs/about";

/// The large file's size, as nfstest writes it.
const BIG: usize = 4 * 1024 * 1024;

pub const CASE: Case = Case {
    name: "nfs",
    about: "nfs, mount and umount against the host's Linux NFS server through QEMU's network: command lines refused, a folder listed and mounted with mount -t nfs, root served as no one and refused a write, files written, renamed, linked and copied as number 1000, modes changed, a read-only folder mounted without -t and refusing writes, both unmounted (ADR 0344)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep ip=10.0.2.15/24 gateway=10.0.2.2 -- --posix --network /system/tests/posix/nfstest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: nfs,
    disks: &[],
    second: None,
};

/// The parts nfstest prints when the host gives a server.
const PARTS: &[&str] = &[
    "command_lines",
    "listing",
    "mounted",
    "files",
    "folders",
    "links_and_modes",
    "unmounting",
];

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|err| format!("writing {}: {err}", path.display()))
}

/// `args` as root: themselves where the test is root, else through sudo,
/// which must not ask for a password.
fn as_root(args: &[&str]) -> Command {
    let root = Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "0");
    let mut command = if root {
        Command::new(args[0])
    } else {
        let mut sudo = Command::new("sudo");
        sudo.args(["-n", args[0]]);
        sudo
    };
    command.args(&args[1..]).stdin(Stdio::null());
    command
}

/// Runs `args` as root, which must succeed.
fn must(args: &[&str]) -> Result<(), String> {
    let out = as_root(args)
        .output()
        .map_err(|err| format!("the nfs test cannot run {}: {err}", args[0]))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "the host failed to run {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Why the host cannot give an NFS server, if it cannot: the test is not
/// root and sudo would ask for a password, or Linux's server is not
/// running.
fn no_server() -> Option<String> {
    let rooted = as_root(&["true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !rooted {
        return Some("the test is not root, and sudo would ask for a password".into());
    }
    if !Path::new("/proc/fs/nfsd/threads").exists() {
        return Some("Linux's NFS server is not running (install nfs-kernel-server)".into());
    }
    let threads = as_root(&["cat", "/proc/fs/nfsd/threads"]).output();
    match threads {
        Ok(out) if String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().unwrap_or(0) > 0 => {
            None
        }
        _ => Some("Linux's NFS server has no threads running".into()),
    }
}

/// The exports, taken back when dropped, so that no test leaves the
/// host's server serving its folders.
struct Exports(Vec<String>);

impl Exports {
    /// `folder` exported to the loopback with `options` besides those
    /// every export here has: `insecure`, as QEMU's connections come
    /// from ports past 1023, and a filesystem number of its own, as /tmp
    /// may be a tmpfs, which has no other.
    fn add(&mut self, folder: &Path, options: &str, fsid: u32) -> Result<(), String> {
        let spec = format!("127.0.0.1:{}", folder.display());
        let options = format!("{options},insecure,no_subtree_check,root_squash,fsid={fsid}");
        must(&["exportfs", "-o", &options, &spec])?;
        self.0.push(spec);
        Ok(())
    }
}

impl Drop for Exports {
    fn drop(&mut self) {
        for spec in &self.0 {
            let _ = as_root(&["exportfs", "-u", spec]).stdout(Stdio::null()).status();
        }
    }
}

/// The words nfstest writes, little-endian, as it writes them.
fn big() -> Vec<u8> {
    (0..BIG as u32 / 4).flat_map(|i| i.wrapping_mul(2_654_435_761).to_le_bytes()).collect()
}

/// The host's folders, exported: what nfstest reads, owned by number
/// 1000 where it writes.
fn serve(dir: &Path) -> Result<Exports, String> {
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    // What an earlier run left: its exports, then its files, some of
    // them number 1000's.
    for name in ["export", "readonly"] {
        let _ = as_root(&["exportfs", "-u", &format!("127.0.0.1:{}", path(name))])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    if dir.exists() {
        must(&["rm", "-rf", &path("")])?;
    }
    for sub in ["export/folder", "readonly"] {
        std::fs::create_dir_all(dir.join(sub))
            .map_err(|err| format!("making {}: {err}", dir.join(sub).display()))?;
    }
    write(&dir.join("export/from-host.txt"), b"from the host\n")?;
    write(&dir.join("export/folder/inner.txt"), b"inside\n")?;
    write(&dir.join("readonly/ro.txt"), b"only to read\n")?;
    for (name, mode) in [("", "755"), ("export", "755"), ("export/folder", "755")] {
        must(&["chmod", mode, &path(name)])?;
    }
    must(&["chmod", "644", &path("export/from-host.txt"), &path("readonly/ro.txt")])?;
    must(&["chown", "-R", "1000:1000", &path("export")])?;
    let mut exports = Exports(Vec::new());
    exports.add(&dir.join("export"), "rw,sync", 0x4e46_5301)?;
    exports.add(&dir.join("readonly"), "ro", 0x4e46_5302)?;
    Ok(exports)
}

/// What the host finds of what the guest wrote: its file, as number
/// 1000's, the large file whole, and none it removed.
fn check(export: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let written = std::fs::read_to_string(export.join("from-guest.txt")).unwrap_or_default();
    if written != "from dukovo\n" {
        return Err(format!("the guest wrote \"{written}\" in the host's folder"));
    }
    let owner = std::fs::metadata(export.join("from-guest.txt"))
        .map(|meta| (meta.uid(), meta.gid()))
        .map_err(|err| format!("from-guest.txt: {err}"))?;
    if owner != (1000, 1000) {
        return Err(format!("the guest's file is owned by {owner:?}, not 1000's"));
    }
    if std::fs::read(export.join("big")).ok() != Some(big()) {
        return Err("the large file the guest wrote is not what it wrote".into());
    }
    let link = std::fs::read_link(export.join("link")).map_err(|err| format!("link: {err}"))?;
    if link != Path::new("from-guest.txt") {
        return Err(format!("the guest's link leads to {}", link.display()));
    }
    for gone in ["scratch.txt", "new.txt", "made", "moved", "hard", "made-ro.txt", "by-root.txt"] {
        if export.join(gone).exists() {
            return Err(format!("{gone} is in the host's folder"));
        }
    }
    Ok(())
}

fn nfs(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let root: PathBuf = std::env::current_dir().map_err(|err| err.to_string())?;
    let about = root.join(ABOUT);
    std::fs::create_dir_all(about.parent().unwrap_or(&root))
        .map_err(|err| format!("making {}: {err}", about.display()))?;
    let dir = dir();
    let no_server = no_server();
    if let Some(why) = &no_server
        && std::env::var_os("GITHUB_ACTIONS").is_some()
    {
        return Err(format!("the nfs test exports folders as root, and {why}"));
    }
    let exports = match &no_server {
        None => Some(serve(&dir)?),
        Some(_) => None,
    };
    let said = match exports {
        Some(_) => format!(
            "ready {} {}",
            dir.join("export").display(),
            dir.join("readonly").display()
        ),
        None => "none".to_string(),
    };
    write(&about, format!("#!/bin/sh\necho {said}\n").as_bytes())?;
    std::fs::set_permissions(&about, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .map_err(|err| format!("making {} a program: {err}", about.display()))?;

    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_ahead("spark: started /system/tests/posix/nfstest as process ")?;
    let parts = if exports.is_some() { PARTS } else { &PARTS[..1] };
    for part in parts {
        vm.expect_ahead(&format!("nfstest: {part}"))?;
    }
    let code = program_line(vm, "(/system/tests/posix/nfstest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("nfstest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("nfstest: every check passed") || code != "0x0" {
        return Err(format!("the nfs test failed ({code}): {}", failed.join("; ")));
    }
    match (&exports, no_server) {
        (Some(_), _) => check(&dir.join("export"))?,
        (None, Some(why)) => eprintln!("          nfs: the host's server left out, as {why}"),
        (None, None) => {}
    }
    drop(exports);
    let _ = as_root(&["rm", "-rf", &dir.to_string_lossy()]).status();
    Ok(())
}
