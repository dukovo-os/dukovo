// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! smb(1), mount(8) and umount(8) against a real server (ADR 0227): the
//! host's Samba, run as the host's user with a share and an account the
//! test makes in out/test/smb, which the guest reaches through QEMU's
//! user network at 10.0.2.2; the host tells the port and the user's name
//! on 10.0.2.100 port 79. _/system/tests/posix/smbtest_ lists the
//! server's shares and a folder, and is refused a wrong password and a
//! missing share; mounts the share with `mount -t cifs` and reads,
//! writes, appends to, cuts short, renames, removes and copies files
//! there, 3 MiB of them, and folders; marks a file read-only with chmod
//! and takes it back, and makes one read-only that is made without write
//! bits; mounts a folder of the share only to read; unmounts
//! both; and mounts the case's FAT disk with mount(8), to write and only
//! to read. The host finds the file the guest wrote in the share.

use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use super::super::*;

/// Where the server's files are, from the top of the tree, which QEMU
/// runs in.
const DIR: &str = "out/test/smb";

/// The password of the user's account in smbd, which smbtest gives too.
pub(super) const PASSWORD: &str = "dukovo-smb-test";

/// The test's network: QEMU's user network, with the host's smbd at
/// 10.0.2.2 and what smbtest needs to know at 10.0.2.100 port 79.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:79-cmd:out/test/smb/about";

pub const CASE: Case = Case {
    name: "smb",
    about: "smb, mount and umount against the host's Samba through QEMU's network: shares and folders listed, a share mounted with mount -t cifs and its files read, written, renamed and copied, chmod marking one read-only, a folder mounted read-only, both unmounted, and a FAT disk mounted with mount (ADR 0227)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep ip=10.0.2.15/24 gateway=10.0.2.2 -- --posix --network /system/tests/posix/smbtest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: smb,
    disks: &[fat_disk(32 * 1024, 32, "SMBDISK", Table::None, Bus::Virtio)],
    second: None,
};

/// The parts smbtest prints, in order.
const PARTS: &[&str] = &["listing", "mounted", "files", "folders", "modes", "unmounting", "disk"];

fn write(path: &Path, text: &str, executable: bool) -> Result<(), String> {
    std::fs::write(path, text).map_err(|err| format!("writing {}: {err}", path.display()))?;
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("making {} a program: {err}", path.display()))?;
    }
    Ok(())
}

/// The host's smbd, ended when the test is.
pub(super) struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        // SIGTERM to its process group, smbd and those it started for
        // each connection; SIGKILL to smbd if it has not ended in 5
        // seconds.
        let group = format!("-{}", self.0.id());
        let _ = Command::new("kill").args(["-TERM", "--", &group]).status();
        for _ in 0..50 {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The host's Samba program `name`, from /usr/sbin or the PATH.
fn samba(name: &str) -> Result<PathBuf, String> {
    for dir in ["/usr/sbin", "/usr/bin", "/sbin"] {
        let path = Path::new(dir).join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(format!("the smb test needs the host's {name}: install samba"))
}

/// A port of the host's loopback that nothing listens on.
fn free_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|err| format!("no free port: {err}"))?;
    listener.local_addr().map(|addr| addr.port()).map_err(|err| err.to_string())
}

/// The server's files: its settings, a share with a file and a folder,
/// the user's account, and what port 79 tells; then smbd, started, with
/// the port it listens on and the user's name.
pub(super) fn serve(dir: &Path) -> Result<(Server, u16, String), String> {
    let (smbd, pdbedit) = (samba("smbd")?, samba("pdbedit")?);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    for sub in ["share/folder", "private", "state", "lock", "cache", "pid", "log"] {
        std::fs::create_dir_all(dir.join(sub))
            .map_err(|err| format!("making {}: {err}", dir.join(sub).display()))?;
    }
    let user = String::from_utf8_lossy(
        &Command::new("id").arg("-un").output().map_err(|err| format!("running id: {err}"))?.stdout,
    )
    .trim()
    .to_string();
    let port = free_port()?;
    let at = dir.display();
    // Everything in the test's folder, smbd as the host's user: its
    // account is the one the share is written as, and no root is needed.
    write(
        &dir.join("smb.conf"),
        &format!(
            "[global]\n  server role = standalone server\n  smb ports = {port}\n\
             \x20 interfaces = 127.0.0.1\n  bind interfaces only = yes\n  disable netbios = yes\n\
             \x20 pid directory = {at}/pid\n  lock directory = {at}/lock\n\
             \x20 state directory = {at}/state\n  cache directory = {at}/cache\n\
             \x20 private dir = {at}/private\n  ncalrpc dir = {at}/state/ncalrpc\n\
             \x20 log file = {at}/log/log.%m\n  passdb backend = tdbsam:{at}/private/passdb.tdb\n\
             \x20 load printers = no\n  printing = bsd\n  printcap name = /dev/null\n\
             \x20 disable spoolss = yes\n  server min protocol = SMB2_10\n\
             \x20 server signing = mandatory\n\
             [share]\n  path = {at}/share\n  comment = Dukovo's test share\n  read only = no\n"
        ),
        false,
    )?;
    write(&dir.join("share/from-host.txt"), "from the host\n", false)?;
    write(&dir.join("share/folder/inner.txt"), "inside\n", false)?;
    write(&dir.join("about"), &format!("#!/bin/sh\necho {port} {user}\n"), true)?;
    let conf = format!("--configfile={at}/smb.conf");
    let mut add = Command::new(&pdbedit)
        .args([conf.as_str(), "-a", "-u", &user, "-t"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("running pdbedit: {err}"))?;
    if let Some(mut input) = add.stdin.take() {
        use std::io::Write;
        input
            .write_all(format!("{PASSWORD}\n{PASSWORD}\n").as_bytes())
            .map_err(|err| format!("giving pdbedit the password: {err}"))?;
    }
    let added = add.wait_with_output().map_err(|err| format!("running pdbedit: {err}"))?;
    if !added.status.success() {
        return Err(format!(
            "pdbedit did not add {user}: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        ));
    }
    let log = std::fs::File::create(dir.join("log/smbd.out"))
        .map_err(|err| format!("making smbd's log: {err}"))?;
    // In a process group of its own: smbd ends its group when it ends,
    // which would otherwise be ob's and QEMU's.
    let child = Command::new(&smbd)
        .args(["--foreground", "--no-process-group", conf.as_str()])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|err| err.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|err| format!("starting smbd: {err}"))?;
    let server = Server(child);
    // Ready once it listens.
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok((server, port, user));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!("smbd did not listen on port {port}: see {at}/log"))
}

fn smb(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let root: PathBuf = std::env::current_dir().map_err(|err| err.to_string())?;
    let dir = root.join(DIR);
    let (_server, _port, _user) = serve(&dir)?;
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_ahead("spark: started /system/tests/posix/smbtest as process ")?;
    for part in PARTS {
        vm.expect_ahead(&format!("smbtest: {part}"))?;
    }
    let code = program_line(vm, "(/system/tests/posix/smbtest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("smbtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("smbtest: every check passed") || code != "0x0" {
        return Err(format!("the smb test failed ({code}): {}", failed.join("; ")));
    }
    // The file the guest wrote, in the host's share; and none it removed.
    let share = dir.join("share");
    let written = std::fs::read_to_string(share.join("from-guest.txt")).unwrap_or_default();
    if written != "from dukovo\n" {
        return Err(format!("the guest wrote \"{written}\" in the host's share"));
    }
    for gone in ["scratch.txt", "big", "moved", "made", "made-ro.txt"] {
        if share.join(gone).exists() {
            return Err(format!("{gone} is still in the host's share"));
        }
    }
    Ok(())
}
