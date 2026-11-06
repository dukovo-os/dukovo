// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NFS with Kerberos both ways against Linux (ADR 0399): the host runs
//! MIT's KDC for DUKOVO.TEST, as the kerberos test does, and Linux's
//! NFS server exports a folder with `sec=krb5p` alone, rpc.svcgssd
//! accepting tickets for nfs/server.dukovo.test. Alice, signed in, puts
//! krb5.conf and a keytab for nfs/localhost in place through elevate and
//! takes her ticket with kinit; mount(8) mounts the host's folder
//! with `sec=krb5p`, under her ticket, and she reads the host's file and
//! writes one, which the host finds. She then shares a folder of hers to
//! QEMU's network with `--nfs-sec krb5p`, and the host's kernel mounts
//! it through a port QEMU forwards, with `sec=krb5p`, rpc.gssd setting
//! the context up from a ticket of alice's in root's cache: it reads her
//! file and writes one, which the guest finds as alice's, her principal
//! mapped to her account. The KDC's log shows both services' tickets
//! given to alice. Mounting and exporting need root, with sudo where the
//! test is not root, and Linux's NFS server and client (nfs-kernel-server,
//! nfs-common); where they are missing, as in `ob --container`, only the
//! guest's part is checked: its settings, alice's ticket and her share.
//! Where the host has no /etc/krb5.keytab, the test puts an empty one
//! there while it runs: mountd offers Kerberos on the folders on the way
//! to an export, NFS 4's pseudo-root, only when that file exists, and
//! without it the guest's mount, as Linux's own would, finds the root
//! refusing krb5p.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::super::*;
use super::kerberos::{REALM, free_port, realm};

/// The folder of the realm, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/nfs-kerberos";

/// The test's network: QEMU's user network, with the host at 10.0.2.2
/// and the guest's keytab at 10.0.2.100 port 80.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:80-cmd:out/test/nfs-kerberos/keytab";

/// The host's NFS service, which rpc.svcgssd accepts tickets for.
const HOST_SERVICE: &str = "nfs/server.dukovo.test@DUKOVO.TEST";

/// The host's own keytab, whose being there makes mountd offer Kerberos
/// on the pseudo-root (nfs-utils' v4root.c).
const HOST_KEYTAB: &str = "/etc/krb5.keytab";

/// A keytab with no keys: MIT's format version, 0x0502, alone.
const EMPTY_KEYTAB: [u8; 2] = [0x05, 0x02];

/// MIT's and Linux's programs the host runs, as Debian and Ubuntu
/// install them.
const SVCGSSD: &str = "/usr/sbin/rpc.svcgssd";
const GSSD: &str = "/usr/sbin/rpc.gssd";
const KINIT: &str = "/usr/bin/kinit";

pub const CASE: Case = Case {
    name: "nfs-kerberos",
    about: "NFS with sec=krb5p both ways against Linux and MIT's KDC on the host: mount(8) of a Linux export under alice's ticket, her file read and one written; a share of hers with --nfs-sec krb5p mounted by Linux's kernel through rpc.gssd, her file read and one written as her account (ADR 0399)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // A normal boot, whose netd takes an address from QEMU's network.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: nfs_kerberos,
    disks: &[],
    second: None,
};

/// The host's files, outside the tree, which may be in a home that the
/// NFS server's no one cannot enter.
fn host_dir() -> PathBuf {
    std::env::temp_dir().join("dukovo-nfs-kerberos-test")
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

/// What the host lacks for its part, if anything: root, Linux's NFS
/// server running, or its Kerberos daemons.
fn missing() -> Option<String> {
    let rooted = as_root(&["true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !rooted {
        return Some("the test is not root, and sudo would ask for a password".into());
    }
    let threads = as_root(&["cat", "/proc/fs/nfsd/threads"]).output();
    let running = threads.is_ok_and(|out| {
        String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().unwrap_or(0) > 0
    });
    if !running {
        return Some("Linux's NFS server is not running (install nfs-kernel-server)".into());
    }
    for program in [SVCGSSD, GSSD, KINIT] {
        if !Path::new(program).exists() {
            return Some(format!("{program} is missing (install nfs-common and krb5-user)"));
        }
    }
    None
}

/// Runs `command` for at most two minutes: whether it succeeded, and
/// what it printed.
fn run(mut command: Command, what: &str) -> Result<(bool, String), String> {
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("the nfs-kerberos test cannot {what}: {err}"))?;
    let id = child.id();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait_with_output());
    });
    match receive.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(out)) => Ok((
            out.status.success(),
            format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
        )),
        Ok(Err(err)) => Err(format!("the nfs-kerberos test cannot {what}: {err}")),
        Err(_) => {
            let _ = as_root(&["kill", "-KILL", &id.to_string()]).status();
            Err(format!("the nfs-kerberos test took more than two minutes to {what}"))
        }
    }
}

/// Runs `args` as root, which must succeed.
fn must(args: &[&str], what: &str) -> Result<String, String> {
    match run(as_root(args), what)? {
        (true, text) => Ok(text),
        (false, text) => Err(format!("the host failed to {what}: {}", text.trim())),
    }
}

/// A daemon run as root, stopped when dropped.
struct Daemon(Child);

impl Daemon {
    /// `args` as root, with the realm's krb5.conf in `dir` and the
    /// keytab `keytab`, its output in `dir/<log>`.
    fn start(dir: &Path, keytab: &str, log: &str, args: &[&str]) -> Result<Daemon, String> {
        let config = format!("KRB5_CONFIG={}", dir.join("krb5.conf").display());
        let keytab = format!("KRB5_KTNAME={}", dir.join(keytab).display());
        let mut all = vec!["env", &config, &keytab];
        all.extend_from_slice(args);
        let out = std::fs::File::create(dir.join(log)).map_err(|err| err.to_string())?;
        let child = as_root(&all)
            .stdout(out.try_clone().map_err(|err| err.to_string())?)
            .stderr(out)
            .spawn()
            .map_err(|err| format!("starting {}: {err}", args[0]))?;
        Ok(Daemon(child))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Through sudo, which passes the signal on.
        let _ = as_root(&["kill", "-TERM", &self.0.id().to_string()]).status();
        let _ = self.0.wait();
    }
}

/// An empty keytab at /etc/krb5.keytab where the host had none, taken
/// away when dropped. Empty, so that rpc.gssd finds no machine key in it
/// and nothing else on the host changes what it does.
struct HostKeytab {
    /// Whether the test put it there.
    ours: bool,
}

impl HostKeytab {
    /// Puts it in place, through a file in `dir`, unless the host has one.
    fn put(dir: &Path) -> Result<HostKeytab, String> {
        if Path::new(HOST_KEYTAB).exists() {
            return Ok(HostKeytab { ours: false });
        }
        let empty = dir.join("empty.keytab");
        std::fs::write(&empty, EMPTY_KEYTAB)
            .map_err(|err| format!("writing {}: {err}", empty.display()))?;
        let from = empty.to_string_lossy().into_owned();
        must(
            &["install", "-m", "600", &from, HOST_KEYTAB],
            "put an empty keytab at /etc/krb5.keytab",
        )?;
        Ok(HostKeytab { ours: true })
    }
}

impl Drop for HostKeytab {
    fn drop(&mut self) {
        if self.ours {
            let _ = as_root(&["rm", "-f", HOST_KEYTAB]).status();
        }
    }
}

/// The host's export, taken back when dropped.
struct Export(String);

impl Export {
    /// `folder`, exported to the loopback with `sec=krb5p` alone:
    /// `insecure`, as QEMU's connections come from ports past 1023, and a
    /// filesystem number of its own, as /tmp may be a tmpfs.
    fn new(folder: &Path) -> Result<Export, String> {
        let spec = format!("127.0.0.1:{}", folder.display());
        let options = "rw,sec=krb5p,insecure,no_subtree_check,root_squash,fsid=7301";
        must(&["exportfs", "-o", options, &spec], "export with sec=krb5p")?;
        Ok(Export(spec))
    }
}

impl Drop for Export {
    fn drop(&mut self) {
        let _ = as_root(&["exportfs", "-u", &self.0]).stdout(Stdio::null()).status();
    }
}

/// A mount of the guest's share on the host, taken away when dropped.
struct Mount(String);

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = as_root(&["umount", "-l", &self.0]).stdout(Stdio::null()).status();
    }
}

/// rpc_pipefs, where the kernel's client asks rpc.gssd for contexts:
/// where the host has it, or mounted at /run/rpc_pipefs.
fn pipefs() -> Result<String, String> {
    for dir in ["/run/rpc_pipefs", "/var/lib/nfs/rpc_pipefs"] {
        if Path::new(dir).join("nfs").exists() {
            return Ok(dir.into());
        }
    }
    must(&["mkdir", "-p", "/run/rpc_pipefs"], "make /run/rpc_pipefs")?;
    must(&["mount", "-t", "rpc_pipefs", "sunrpc", "/run/rpc_pipefs"], "mount rpc_pipefs")?;
    Ok("/run/rpc_pipefs".into())
}

/// Root's credential cache in `cache`, holding a ticket of alice's, as
/// rpc.gssd finds it for root's calls with `-n`.
fn root_takes_alices_ticket(dir: &Path, cache: &Path) -> Result<(), String> {
    let config = format!("KRB5_CONFIG={}", dir.join("krb5.conf").display());
    let name = format!("FILE:{}", cache.join("krb5cc_0").display());
    let mut kinit = as_root(&["env", &config, KINIT, "-c", &name, "alice"]);
    let mut child = kinit
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("running kinit: {err}"))?;
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(b"drink-me\n");
    }
    let out = child.wait_with_output().map_err(|err| format!("running kinit: {err}"))?;
    if !out.status.success() {
        return Err(format!("the host's kinit failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// Waits, for up to `seconds`, until the screen shows each of `needles`.
fn wait_screen(vm: &mut Vm, needles: &[&str], seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match vm.expect_screen(needles) {
            Ok(()) => return Ok(()),
            Err(err) if Instant::now() >= deadline => return Err(err),
            Err(_) => {}
        }
    }
}

/// The settings alice puts in place, krb5.conf and nfsd's keytab, and
/// her ticket.
fn settings_and_ticket(vm: &mut Vm, port: u16) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate -v && elevate sh -c 'printf \"[libdefaults]\\n default_realm = {REALM}\\n\
             [realms]\\n {REALM} = {{\\n kdc = 10.0.2.2:{port}\\n }}\\n\" > /system/settings/krb5.conf \
             && nc -d 10.0.2.100 80 > /system/settings/krb5.keytab && chmod 600 \
             /system/settings/krb5.keytab' && echo settings in place | tr a-z A-Z"
        ),
    )?;
    answer_prompt(vm, "unlock elevate in this pane:", "queen of hearts")?;
    vm.expect_screen(&["SETTINGS IN PLACE"])?;
    answer_prompt(vm, "$", "clear; kinit alice")?;
    answer_prompt(vm, "Password for alice@DUKOVO.TEST:", "drink-me")?;
    answer_prompt(vm, "$", "clear; klist | tr a-z A-Z")?;
    vm.expect_screen(&["DEFAULT PRINCIPAL: ALICE@DUKOVO.TEST"])
}

/// The host's export mounted with krb5p, read and written to.
fn dukovo_mounts_linux(vm: &mut Vm, host: &Path) -> Result<(), String> {
    // server.dukovo.test names the host's service; addr= is where it is.
    let export = host.join("export");
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; mkdir linux && elevate mount -t nfs -o sec=krb5p,addr=10.0.2.2,soft,timeo=100 \
             server.dukovo.test:{} /users/alice/linux && cat linux/from-linux.txt | tr a-z A-Z",
            export.display()
        ),
    )?;
    wait_screen(vm, &["WRITTEN ON LINUX"], 90)?;
    answer_prompt(
        vm,
        "$",
        "clear; echo written on Dukovo > linux/from-dukovo.txt && elevate umount /users/alice/linux && \
         echo unmounted | tr a-z A-Z",
    )?;
    wait_screen(vm, &["UNMOUNTED"], 60)?;
    let wrote = std::fs::read_to_string(export.join("from-dukovo.txt"))
        .map_err(|err| format!("the guest's file is not in the host's export: {err}"))?;
    if wrote != "written on Dukovo\n" {
        return Err(format!("the guest wrote {wrote:?} to the host's export"));
    }
    Ok(())
}

/// Alice's folder, shared to QEMU's network with krb5p alone, and NFS
/// turned on.
fn alice_shares(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        "clear; mkdir Secret && echo written on Dukovo > Secret/from-dukovo.txt && elevate share \
         add --nfs 10.0.2.0/24 --nfs-sec krb5p Secret /users/alice/Secret && elevate share nfs on \
         && share list | tr a-z A-Z",
    )?;
    wait_screen(vm, &["(SEC=KRB5P)"], 60)
}

/// The last `count` lines of `text`, each marked with `from`, for a
/// failure's message: CI keeps the test's folder only in an artifact,
/// which is slow to reach, so what the daemons said goes in the job's log.
fn last_lines(from: &str, text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    for line in &lines[lines.len().saturating_sub(count)..] {
        out.push_str(&format!("\n          | {from}: {line}"));
    }
    out
}

/// What rpc.gssd, the KDC and the host's kernel said about the host's
/// mount, for when it fails.
fn host_mount_logs(dir: &Path) -> String {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    let kernel = run(as_root(&["dmesg"]), "read the kernel's log").map(|(_, text)| text);
    let rpc: Vec<&str> = match &kernel {
        Ok(text) => {
            text.lines().filter(|line| line.contains("RPC") || line.contains("gss")).collect()
        }
        Err(_) => Vec::new(),
    };
    format!(
        "{}{}{}",
        last_lines("gssd", &read("gssd.log"), 30),
        last_lines("kdc", &read("kdc.log"), 8),
        last_lines("kernel", &rpc.join("\n"), 30)
    )
}

/// The share, which the host's kernel mounts with krb5p through
/// `forwarded`, reads and writes to; the guest finds the file as alice's.
fn linux_mounts_dukovo(vm: &mut Vm, host: &Path, forwarded: u16) -> Result<(), String> {
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{forwarded}-:2049"))?;
    // The kernel's RPC client says why a context or a call failed, in its
    // log, while this is on (where its kernel has the debugging built in).
    let _ = as_root(&["rpcdebug", "-m", "rpc", "-s", "auth"]).stdout(Stdio::null()).status();
    let at = host.join("mnt");
    let target = at.to_string_lossy().into_owned();
    // localhost names the guest's service, nfs/localhost, for rpc.gssd;
    // addr= keeps the kernel off ::1, which QEMU does not forward.
    let options = format!(
        "vers=4.2,proto=tcp,port={forwarded},addr=127.0.0.1,sec=krb5p,soft,timeo=100,retrans=3"
    );
    let started = Instant::now();
    let mount = loop {
        let args = ["mount", "-t", "nfs4", "-o", &options, "localhost:/Secret", &target];
        match run(as_root(&args), "mount the guest's share with krb5p")? {
            (true, _) => break Mount(target.clone()),
            (false, text) if started.elapsed() > Duration::from_secs(60) => {
                let logs = host_mount_logs(Path::new(DIR));
                let _ = as_root(&["rpcdebug", "-m", "rpc", "-c", "auth"])
                    .stdout(Stdio::null())
                    .status();
                return Err(format!(
                    "the host's kernel did not mount the guest's share with krb5p: {}; see \
                     {DIR}/gssd.log{logs}",
                    text.trim()
                ));
            }
            (false, _) => std::thread::sleep(Duration::from_secs(2)),
        }
    };
    let _ = as_root(&["rpcdebug", "-m", "rpc", "-c", "auth"]).stdout(Stdio::null()).status();
    let read = must(&["cat", &format!("{target}/from-dukovo.txt")], "read the guest's file")?;
    if read != "written on Dukovo\n" {
        return Err(format!("the host read {read:?} from the guest's share"));
    }
    let file = format!("{target}/from-linux.txt");
    must(&["sh", "-c", "echo written on Linux > \"$1\"", "sh", &file], "write to the share")?;
    drop(mount);
    answer_prompt(
        vm,
        "$",
        "clear; cat Secret/from-linux.txt | tr a-z A-Z; echo \"owner $(stat -c %u \
         Secret/from-linux.txt)\"",
    )?;
    vm.expect_screen(&["WRITTEN ON LINUX", "owner 1000"])
}

/// The lines of the KDC's log both mounts must have made.
fn check_log(dir: &Path) -> Result<(), String> {
    let log = std::fs::read_to_string(dir.join("kdc.log"))
        .map_err(|err| format!("reading the KDC's log: {err}"))?;
    for service in ["nfs/server.dukovo.test", "nfs/localhost"] {
        let needles = ["TGS_REQ", "ISSUE", "alice@DUKOVO.TEST for ", service];
        if !log.lines().any(|line| needles.iter().all(|needle| line.contains(needle))) {
            return Err(format!("the KDC's log shows no ticket of alice's for {service}: see {DIR}/kdc.log"));
        }
    }
    Ok(())
}

fn nfs_kerberos(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let missing = missing();
    if let Some(why) = &missing
        && std::env::var_os("GITHUB_ACTIONS").is_some()
    {
        return Err(format!("the nfs-kerberos test needs the host's NFS, and {why}"));
    }
    let root: PathBuf = std::env::current_dir().map_err(|err| err.to_string())?;
    let dir = root.join(DIR);
    let port = free_port()?;
    let kdc = realm(&dir, port, "localhost.keytab")?;
    owner_signs_in(vm, case)?;
    vm.expect("authd: keyring of alice: open")?;
    settings_and_ticket(vm, port)?;
    match &missing {
        None => with_host(vm, &dir)?,
        Some(why) => {
            eprintln!("          nfs-kerberos: the host's NFS left out, as {why}");
            alice_shares(vm)?;
        }
    }
    answer_prompt(vm, "$", "clear; kdestroy; elevate share nfs off; echo done | tr a-z A-Z")?;
    wait_screen(vm, &["DONE"], 30)?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    drop(kdc);
    if missing.is_none() {
        check_log(&dir)?;
        let _ = as_root(&["rm", "-rf", &host_dir().to_string_lossy()]).status();
    }
    Ok(())
}

/// The host's part: its server and the guest's mount of it, then the
/// guest's share and the host's kernel's mount of it.
fn with_host(vm: &mut Vm, dir: &Path) -> Result<(), String> {
    let host = host_dir();
    let _ = as_root(&["umount", "-l", &host.join("mnt").to_string_lossy()]).status();
    let _ = as_root(&["rm", "-rf", &host.to_string_lossy()]).status();
    for sub in ["export", "mnt", "cache"] {
        std::fs::create_dir_all(host.join(sub)).map_err(|err| format!("making {sub}: {err}"))?;
    }
    // Alice is no one on the host: her writes come as its no one's.
    let open = std::os::unix::fs::PermissionsExt::from_mode(0o777);
    std::fs::set_permissions(host.join("export"), open)
        .map_err(|err| format!("opening the export to all: {err}"))?;
    std::fs::write(host.join("export/from-linux.txt"), "written on Linux\n")
        .map_err(|err| format!("writing the host's file: {err}"))?;
    // The kernel's Kerberos for RPC, where it is a module.
    let _ = as_root(&["modprobe", "rpcsec_gss_krb5"]).status();

    // Before the export, as mountd looks for it as the export changes;
    // taken away last, after the export.
    let _keytab = HostKeytab::put(dir)?;
    let svcgssd = [SVCGSSD, "-f", "-v", "-p", HOST_SERVICE];
    let _svcgssd = Daemon::start(dir, "nfs.keytab", "svcgssd.log", &svcgssd)?;
    let _export = Export::new(&host.join("export"))?;
    dukovo_mounts_linux(vm, &host)?;

    alice_shares(vm)?;
    let pipefs = pipefs()?;
    let cache = host.join("cache");
    root_takes_alices_ticket(dir, &cache)?;
    let cache = cache.to_string_lossy().into_owned();
    let gssd = [GSSD, "-f", "-v", "-n", "-d", &cache, "-p", &pipefs];
    let _gssd = Daemon::start(dir, "localhost.keytab", "gssd.log", &gssd)?;
    linux_mounts_dukovo(vm, &host, free_port()?)
}
