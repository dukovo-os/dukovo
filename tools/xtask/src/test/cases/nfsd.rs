// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nfsd(8) and share(1) against Linux's own NFS client (ADR 0343):
//! _/system/tests/posix/nfsdtest_ makes alice's account and shares a
//! folder of her home to QEMU's network, to read and write, and to the
//! host's address alone, only to read, and once more to no network;
//! share(1) refuses a network that is not one, and alice can neither
//! turn NFS on nor have authd start a worker. `share nfs on` starts
//! nfsd, and QEMU's user network reaches it through a port of the host's
//! loopback forwarded to its port 2049. The host's kernel mounts the
//! shares over NFS 4.2, as root, with sudo where the test does not run as
//! root. The root of the mounts lists the two shares the host's address
//! may reach, and not the third; root, served as no one, is refused a
//! write. As alice's number, the host reads her file, copies one in, sets
//! its mode, renames it, links to it, locks it, makes a folder and
//! copies a large file into it, which it reads back whole after mounting
//! again; the read-only share is read and refuses a write. The guest
//! finds what the host wrote, as alice's; `share nfs off` stops nfsd and
//! closes the port. Where the test is not root and sudo would ask for a
//! password, as in `ob --container`, the host's turn is left out.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::super::*;

/// The host's files and the mounts, where alice's number on the host
/// reaches them, as the tree may be in a home it cannot enter.
fn dir() -> PathBuf {
    std::env::temp_dir().join("dukovo-nfsd-test")
}

/// alice's number, in the guest and on the host's client.
const ALICE: &str = "1000";

/// The large file's size, as nfsdtest checks it.
const BIG_SIZE: usize = 4 * 1024 * 1024;

pub const CASE: Case = Case {
    name: "nfsd",
    about: "nfsd and share against the host's Linux NFS client through a port QEMU forwards: folders shared to a network and to one address, read-write and read-only; the shares the host may reach listed at the root; root served as no one and refused a write; as alice's number, files read, written, renamed, linked, locked and copied back after a new mount over NFS 4.2; a write to the read-only share refused; share nfs off closing the port (ADR 0343)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // Every service, as on a machine, netd setting the card up by DHCP.
    cmdline: "lockdep -- --services --posix --names --network /system/tests/posix/nfsdtest",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: nfsd,
    disks: &[],
    second: None,
};

/// The parts nfsdtest prints before the host's turn, and after it.
const BEFORE: &[&str] = &["accounts", "sharing", "on", "host"];
const AFTER: &[&str] = &["off"];

/// What alice does on the host, in the mounts of Public and Readonly
/// given as `$1` and `$2`, with the host's files in `$3`.
const ALICE_WRITES: &str = r#"set -eu
P=$1 R=$2 D=$3
test "$(cat "$P/from-dukovo.txt")" = "written on Dukovo"
test "$(stat -c %u:%g "$P/from-dukovo.txt")" = 1000:1000
cp "$D/from-host.txt" "$P/from-host.txt"
chmod 600 "$P/from-host.txt"
mv "$P/from-host.txt" "$P/renamed.txt"
test "$(stat -c %a:%u "$P/renamed.txt")" = 600:1000
ln -s renamed.txt "$P/link"
test "$(cat "$P/link")" = "written on the host"
flock "$P/renamed.txt" true
mkdir "$P/made-by-host"
cp "$D/big.bin" "$P/made-by-host/big.bin"
test "$(cat "$R/from-dukovo.txt")" = "written on Dukovo"
if (echo no >"$R/refused.txt") 2>"$D/refused.err"; then
    echo "wrote to Readonly" >&2
    exit 1
fi
grep -q "Read-only file system" "$D/refused.err"
"#;

/// What alice checks after Public is mounted again: what the guest
/// holds, not what the host's client kept.
const ALICE_READS: &str = r#"set -eu
P=$1 D=$2
cmp "$D/big.bin" "$P/made-by-host/big.bin"
test "$(cat "$P/renamed.txt")" = "written on the host"
ls -ln "$P"
echo done > "$P/done.txt"
"#;

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

/// Why the test cannot act as root on the host, if it cannot: it is not
/// root, and sudo asks for a password, or there is none, as in `ob
/// --container`.
fn not_root() -> Option<String> {
    let ok = as_root(&["true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    (!ok).then(|| "the test is not root, and sudo would ask for a password".to_string())
}

/// Runs `command`, for at most two minutes: whether it succeeded, and
/// what it printed.
fn run(mut command: Command, what: &str) -> Result<(bool, String), String> {
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("the nfsd test cannot {what}: {err}"))?;
    let id = child.id();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait_with_output());
    });
    match receive.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(output)) => Ok((
            output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )),
        Ok(Err(err)) => Err(format!("the nfsd test cannot {what}: {err}")),
        Err(_) => {
            let _ = as_root(&["kill", "-KILL", &id.to_string()]).status();
            Err(format!("the nfsd test took more than two minutes to {what}"))
        }
    }
}

/// Runs `command`, which must succeed.
fn must(command: Command, what: &str) -> Result<String, String> {
    match run(command, what)? {
        (true, text) => Ok(text),
        (false, text) => Err(format!("the host failed to {what}: {}", text.trim())),
    }
}

/// A share of the guest mounted on the host, unmounted when dropped, so
/// that no test leaves the host a mount whose server is gone.
struct Mount {
    at: PathBuf,
}

impl Mount {
    /// `share` of the guest's nfsd, through `port` of the host's
    /// loopback, mounted at `at` by the host's kernel over NFS 4.2.
    fn new(share: &str, port: u16, at: &Path) -> Result<Mount, String> {
        std::fs::create_dir_all(at).map_err(|err| format!("making {}: {err}", at.display()))?;
        // `soft`: a guest that ends leaves no process waiting for ever.
        let options = format!(
            "vers=4.2,proto=tcp,port={port},addr=127.0.0.1,soft,timeo=100,retrans=3"
        );
        let source = format!("127.0.0.1:/{share}");
        let target = at.to_string_lossy();
        let args = ["mount", "-t", "nfs4", "-o", &options, &source, &target];
        match run(as_root(&args), &format!("mount {source}"))? {
            (true, _) => Ok(Mount { at: at.to_path_buf() }),
            (false, text) => Err(format!(
                "the host's kernel did not mount {source} (the nfsd test mounts as root, or \
                 through sudo without a password, with Linux's NFS 4.2 client): {}",
                text.trim()
            )),
        }
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let target = self.at.to_string_lossy().into_owned();
        // `-f -l` and a timeout of its own: force the unmount even if the
        // server is gone, detach it lazily, and never let `drop` block for
        // ever on a share whose nfsd has died.
        let _ = run(as_root(&["umount", "-f", "-l", &target]), &format!("unmount {target}"));
    }
}

/// As alice's number on the host: `script`, given `args`.
fn as_alice(script: &str, args: &[&str]) -> Command {
    let uid = format!("--reuid={ALICE}");
    let gid = format!("--regid={ALICE}");
    let mut all = vec!["setpriv", &uid, &gid, "--clear-groups", "--", "sh", "-c", script, "sh"];
    all.extend_from_slice(args);
    // In a folder alice's number may enter, as the tree's may not be.
    let mut command = as_root(&all);
    command.current_dir(dir());
    command
}

/// The host's turn: Linux's client against the guest's nfsd, through
/// `port` of the host's loopback, with the host's files in `dir`.
fn host(dir: &Path, port: u16) -> Result<(), String> {
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    // The root, once netd has the guest's address: QEMU forwards to the
    // address its DHCP server gives.
    let started = Instant::now();
    let root = loop {
        match Mount::new("", port, &dir.join("root")) {
            Ok(mount) => break mount,
            Err(err) if started.elapsed() > Duration::from_secs(60) => return Err(err),
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    };
    let listed = must(as_root(&["ls", "-a", &path("root")]), "list the root")?;
    let names: Vec<&str> = listed.lines().collect();
    if !names.contains(&"Public") || !names.contains(&"Readonly") || names.contains(&"Private") {
        return Err(format!("the root listed {names:?}, not Public and Readonly alone"));
    }
    drop(root);

    let public = Mount::new("Public", port, &dir.join("public"))?;
    let readonly = Mount::new("Readonly", port, &dir.join("readonly"))?;
    // Root is served as no one, who may not write in alice's folder.
    let by_root = format!("{}/by-root.txt", path("public"));
    let wrote = run(as_root(&["sh", "-c", "echo root > \"$1\"", "sh", &by_root]), "write as root")?;
    if wrote.0 || !wrote.1.contains("Permission denied") {
        return Err(format!("root was not refused a write to Public: {}", wrote.1.trim()));
    }
    must(
        as_alice(ALICE_WRITES, &[&path("public"), &path("readonly"), &path("")]),
        "work in the shares as alice",
    )?;
    drop(readonly);
    drop(public);

    // Mounted again, the files are what the guest holds.
    let _public = Mount::new("Public", port, &dir.join("public"))?;
    must(as_alice(ALICE_READS, &[&path("public"), &path("")]), "read the shares back as alice")?;
    Ok(())
}

fn nfsd(vm: &mut Vm, case: &Case) -> Result<(), String> {
    // Only root mounts: on a contributor's machine without it, the host's
    // turn is left out, and the guest's part before it is checked.
    let not_root = not_root();
    if let Some(why) = &not_root
        && std::env::var_os("GITHUB_ACTIONS").is_some()
    {
        return Err(format!("the nfsd test mounts as root, and {why}"));
    }
    let dir = dir();
    // A mount an earlier run left behind goes first.
    if not_root.is_none() {
        for name in ["root", "public", "readonly"] {
            let at = dir.join(name).to_string_lossy().into_owned();
            let _ = run(as_root(&["umount", "-f", "-l", &at]), &format!("unmount {at}"));
        }
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    write(&dir.join("from-host.txt"), b"written on the host\n")?;
    // Bytes that repeat only every 251, so that a block out of place
    // shows.
    let big: Vec<u8> = (0..BIG_SIZE).map(|i| (i % 251) as u8).collect();
    write(&dir.join("big.bin"), &big)?;
    // alice's number on the host reads them, and writes there.
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o777))
        .map_err(|err| format!("opening {} to all: {err}", dir.display()))?;

    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_ahead("spark: started /system/tests/posix/nfsdtest as process ")?;
    for part in BEFORE {
        vm.expect(&format!("nfsdtest: {part}"))?;
    }
    vm.expect("nfsdtest: ready for the host")?;
    if let Some(why) = not_root {
        eprintln!("          nfsd: the host's mounts left out, as {why}");
        let log = vm.log();
        if log.contains("nfsdtest: FAILED") {
            return Err("nfsdtest failed before the host's turn".into());
        }
        return Ok(());
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map_err(|err| format!("no free port: {err}"))?
        .port();
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:2049"))?;
    host(&dir, port)?;
    for part in AFTER {
        vm.expect_ahead(&format!("nfsdtest: {part}"))?;
    }
    let code = program_line(vm, "(/system/tests/posix/nfsdtest) exited with code ")?;
    let log = vm.log();
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains("nfsdtest: FAILED")).map(str::trim).collect();
    if !failed.is_empty() || !log.contains("nfsdtest: every check passed") || code != "0x0" {
        return Err(format!("the nfsd test failed ({code}): {}", failed.join("; ")));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
