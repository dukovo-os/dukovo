// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! smbd(8) and share(1) against Linux's client (ADR 0228):
//! _/system/tests/posix/smbdtest_ makes alice's account, sets her sharing
//! password as root, and she changes it given her login password; it
//! shares a folder of her home to read and write, and again only to read,
//! and share(1) refuses names that are taken or cannot be a share's.
//! Then smbd starts at `share on`, and QEMU's user network reaches it
//! through a port of the host's loopback forwarded to its port 445. The
//! host's smbclient, Samba's, signs in over SMB 3 with encryption
//! required, lists the shares, copies files both ways and makes a folder;
//! it is refused a wrong password, a write to the read-only share, and
//! SMB 1. The guest finds what the host wrote, as alice's, and the
//! sign-ins and the wrong password in last(1) and lastb(1); `share off`
//! stops smbd and closes the port.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::super::*;

/// The host's files, from the top of the tree.
const DIR: &str = "out/test/smbd";

/// alice's sharing password, which smbdtest sets.
const PASSWORD: &str = "through the looking-glass";

pub const CASE: Case = Case {
    name: "smbd",
    about: "smbd and share against the host's smbclient through a port QEMU forwards: alice's sharing password set, by root and by her; a folder shared to read and write and only to read; shares listed, files copied both ways and a folder made over encrypted SMB 3; a wrong password, a write to the read-only share and SMB 1 refused; the sign-ins in last and lastb; share off closing the port (ADR 0228)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // Every service, as on a machine, netd setting the card up by DHCP.
    cmdline: "lockdep -- --services --posix --names --network /system/tests/posix/smbdtest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: smbd,
    disks: &[],
    second: None,
};

/// The parts smbdtest prints before the host's turn, and after it.
const BEFORE: &[&str] = &["accounts", "sharing", "on", "host"];
const AFTER: &[&str] = &["off"];

/// What smbclient printed, and whether it succeeded.
struct Ran {
    ok: bool,
    text: String,
}

/// Runs the host's smbclient in `dir` with `args`, for at most two
/// minutes.
fn smbclient(dir: &Path, args: &[&str]) -> Result<Ran, String> {
    let child = Command::new("smbclient")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("the smbd test needs the host's smbclient: install samba ({err})"))?;
    let id = child.id();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait_with_output());
    });
    match receive.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(output)) => Ok(Ran {
            ok: output.status.success(),
            text: format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        }),
        Ok(Err(err)) => Err(format!("running smbclient: {err}")),
        Err(_) => {
            let _ = Command::new("kill").args(["-KILL", &id.to_string()]).status();
            Err(format!("smbclient {} took more than two minutes", args.join(" ")))
        }
    }
}

/// Checks that `ran` succeeded, or failed, as `ok` says if it says, and
/// printed each of `needles`.
fn check(what: &str, ran: &Ran, ok: Option<bool>, needles: &[&str]) -> Result<(), String> {
    if ok.is_some_and(|ok| ran.ok != ok) || !needles.iter().all(|needle| ran.text.contains(needle)) {
        let how = if ran.ok { "succeeded" } else { "failed" };
        return Err(format!("smbclient {how} to {what}, or did not print {needles:?}: {}", ran.text.trim()));
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|err| format!("writing {}: {err}", path.display()))
}

/// The host's turn: smbclient against the guest's smbd, through `port` of
/// the host's loopback, in `dir`.
fn host(dir: &Path, port: u16) -> Result<(), String> {
    let port = port.to_string();
    let alice = format!("alice%{PASSWORD}");
    // smbclient `head`, signed in as `who` over SMB 3 with encryption
    // required, then `tail`.
    let run = |head: &[&str], who: &str, tail: &[&str]| {
        let mut args = head.to_vec();
        args.extend(["-p", &port, "-U", who, "-m", "SMB3", "--client-protection=encrypt"]);
        args.extend(tail);
        smbclient(dir, &args)
    };

    // The shares, once netd has the guest's address: QEMU forwards to
    // the address its DHCP server gives.
    let started = Instant::now();
    let listed = loop {
        let listed = run(&["-L", "127.0.0.1"], &alice, &[])?;
        if listed.ok || started.elapsed() > Duration::from_secs(60) {
            break listed;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    check("list the shares", &listed, Some(true), &["Public", "Alice's things", "Readonly", "IPC$"])?;

    // Both ways, and a folder, in the share to read and write.
    let commands = "get from-dukovo.txt; put from-host.txt; mkdir made-by-host; ls";
    let copied = run(&["//127.0.0.1/Public"], &alice, &["-c", commands])?;
    check("copy files to and from Public", &copied, Some(true), &["made-by-host", "from-host.txt"])?;
    let got = std::fs::read_to_string(dir.join("from-dukovo.txt")).unwrap_or_default();
    if got != "written on Dukovo\n" {
        return Err(format!("smbclient got \"{got}\" from the guest's from-dukovo.txt"));
    }

    // Read, but not written, in the read-only share.
    let read = run(&["//127.0.0.1/Readonly"], &alice, &["-c", "get from-dukovo.txt from-readonly.txt"])?;
    check("read from Readonly", &read, Some(true), &[])?;
    let got = std::fs::read_to_string(dir.join("from-readonly.txt")).unwrap_or_default();
    if got != "written on Dukovo\n" {
        return Err(format!("smbclient got \"{got}\" from Readonly"));
    }
    let written = run(&["//127.0.0.1/Readonly"], &alice, &["-c", "put refused.txt"])?;
    check("be refused a write to Readonly", &written, None, &["NT_STATUS_ACCESS_DENIED"])?;

    // A wrong password, and SMB 1, are refused.
    let wrong = run(&["-L", "127.0.0.1"], "alice%not her password", &[])?;
    check("be refused a wrong password", &wrong, Some(false), &["NT_STATUS_LOGON_FAILURE"])?;
    let old = run(&["-L", "127.0.0.1"], &alice, &["-m", "NT1", "--option=client min protocol=NT1"])?;
    check("be refused SMB 1", &old, Some(false), &[])?;

    // The last file, which tells the guest the host is done.
    let done = run(&["//127.0.0.1/Public"], &alice, &["-c", "put done.txt"])?;
    check("put done.txt", &done, Some(true), &[])
}

fn smbd(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|err| err.to_string())?.join(DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    write(&dir.join("from-host.txt"), "written on the host\n")?;
    write(&dir.join("refused.txt"), "never written\n")?;
    write(&dir.join("done.txt"), "done\n")?;

    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_ahead("spark: started /system/tests/posix/smbdtest as process ")?;
    for part in BEFORE {
        vm.expect(&format!("smbdtest: {part}"))?;
    }
    vm.expect("smbdtest: ready for the host")?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map_err(|err| format!("no free port: {err}"))?
        .port();
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:445"))?;
    host(&dir, port)?;
    for part in AFTER {
        vm.expect_ahead(&format!("smbdtest: {part}"))?;
    }
    let code = program_line(vm, "(/system/tests/posix/smbdtest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("smbdtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("smbdtest: every check passed") || code != "0x0" {
        return Err(format!("the smbd test failed ({code}): {}", failed.join("; ")));
    }
    Ok(())
}
