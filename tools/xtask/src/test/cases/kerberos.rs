// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! kinit, klist, kvno and kdestroy against a real KDC (ADR 0397): the
//! host runs MIT's krb5kdc as itself, on a port of its own, for the
//! realm DUKOVO.TEST, where QEMU's user network reaches it at 10.0.2.2.
//! Its principals are alice, who must prove herself first
//! (pre-authentication), bob, who has only an aes128-cts key, and the
//! service nfs/server.dukovo.test, whose keytab the host serves at
//! 10.0.2.100 port 80 (and nfs/localhost, which the nfs-kerberos test
//! serves instead). Alice, signed in, writes krb5.conf through
//! elevate; kinit asks for alice's password and keeps the ticket in her
//! keyring, which klist and keyring list show; a wrong password is
//! refused and the ticket stays; kvno takes the service's ticket and
//! names its key's version. nc fetches the keytab, klist -k lists it, and
//! kinit -k signs the service in with it; kinit signs bob in with his
//! older key type; kdestroy throws every ticket away. The host's KDC log
//! then shows each exchange as MIT saw it. The power button ends it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::super::*;

/// The test's realm, which the nfs-kerberos test also makes.
pub(super) const REALM: &str = "DUKOVO.TEST";

/// The service whose keytab the guest fetches.
const SERVICE: &str = "nfs/server.dukovo.test";

/// The folder of the realm, its database and its log, from the top of
/// the tree, which QEMU runs in.
const DIR: &str = "out/test/kerberos";

/// The test's network: QEMU's user network, with the host at 10.0.2.2
/// and its keytab at 10.0.2.100 port 80.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:80-cmd:out/test/kerberos/keytab";

/// MIT's programs the test runs, as Debian and Fedora install them.
const KRB5KDC: &str = "/usr/sbin/krb5kdc";
const KDB5_UTIL: &str = "/usr/sbin/kdb5_util";
const KADMIN_LOCAL: &str = "/usr/sbin/kadmin.local";

/// The test.
pub const CASE: Case = Case {
    name: "kerberos",
    about: "kinit, klist, kvno and kdestroy against MIT's KDC on the host through QEMU's network (ADR 0397): a password with pre-authentication, the ticket in the keyring, a wrong password refused, a service ticket and its key's version, a keytab fetched, listed and signed in with, an aes128-cts principal, and every ticket thrown away",
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
    script: kerberos,
    disks: &[],
    second: None,
};

/// The KDC, stopped when dropped, so that no test leaves one running.
pub(super) struct Kdc(Child);

impl Drop for Kdc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A port free for both TCP and UDP on the host's loopback.
pub(super) fn free_port() -> Result<u16, String> {
    for _ in 0..20 {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
        let port = tcp.local_addr().map_err(|err| err.to_string())?.port();
        if std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err("the host has no port free for both TCP and UDP".into())
}

/// `program` with the realm's files in its environment.
fn mit(program: &str, dir: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env("KRB5_CONFIG", dir.join("krb5.conf"))
        .env("KRB5_KDC_PROFILE", dir.join("kdc.conf"))
        .stdin(Stdio::null());
    command
}

/// Runs `command`, which must succeed.
fn run(mut command: Command, what: &str) -> Result<(), String> {
    let out = command.output().map_err(|err| format!("the kerberos test cannot run {what}: {err}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("{what} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
}

/// The realm, made afresh in `dir` and served on `port`: its principals,
/// the keytabs of nfs/server.dukovo.test (`nfs.keytab`) and of
/// nfs/localhost (`localhost.keytab`), a program at `dir/keytab` that
/// prints the one named `served`, and the KDC running.
pub(super) fn realm(dir: &Path, port: u16, served: &str) -> Result<Kdc, String> {
    for program in [KRB5KDC, KDB5_UTIL, KADMIN_LOCAL] {
        if !Path::new(program).exists() {
            return Err(format!(
                "the kerberos test runs MIT's KDC, and {program} is missing: install krb5-kdc \
                 and krb5-admin-server (`ob doctor` says how)"
            ));
        }
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    let path = |name: &str| dir.join(name).display().to_string();
    let kdc_conf = format!(
        "[kdcdefaults]\n    kdc_ports = {port}\n    kdc_tcp_ports = {port}\n\
         [realms]\n    {REALM} = {{\n        database_name = {}\n        key_stash_file = {}\n\
                 acl_file = {}\n        max_life = 10h\n        max_renewable_life = 7d\n    }}\n\
         [logging]\n    kdc = FILE:{}\n",
        path("principal"),
        path("stash"),
        path("kadm5.acl"),
        path("kdc.log"),
    );
    // The host's programs name services as they are given, without DNS:
    // server.dukovo.test is no name the host knows.
    let krb5_conf = format!(
        "[libdefaults]\n    default_realm = {REALM}\n    dns_canonicalize_hostname = false\n    \
         rdns = false\n[realms]\n    {REALM} = {{\n        kdc = 127.0.0.1:{port}\n    }}\n"
    );
    for (name, text) in [("kdc.conf", kdc_conf), ("krb5.conf", krb5_conf), ("kadm5.acl", String::new())] {
        std::fs::write(dir.join(name), text).map_err(|err| format!("writing {name}: {err}"))?;
    }
    let mut create = mit(KDB5_UTIL, dir);
    create.args(["create", "-s", "-r", REALM, "-P", "the master key of a test realm"]);
    run(create, "kdb5_util create")?;
    for query in [
        "addprinc -pw drink-me +requires_preauth alice".to_string(),
        "addprinc -pw eat-me -e aes128-cts:normal bob".to_string(),
        format!("addprinc -randkey {SERVICE}"),
        format!("ktadd -k {} -e aes256-cts:normal {SERVICE}", path("nfs.keytab")),
        "addprinc -randkey nfs/localhost".to_string(),
        format!("ktadd -k {} -e aes256-cts:normal nfs/localhost", path("localhost.keytab")),
    ] {
        let mut kadmin = mit(KADMIN_LOCAL, dir);
        kadmin.args(["-r", REALM, "-q", &query]);
        run(kadmin, &format!("kadmin.local -q \"{query}\""))?;
    }
    let script = dir.join("keytab");
    std::fs::write(&script, format!("#!/bin/sh\nexec cat {}\n", path(served)))
        .map_err(|err| format!("writing {}: {err}", script.display()))?;
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .map_err(|err| format!("making {} a program: {err}", script.display()))?;
    let log = std::fs::File::create(dir.join("krb5kdc.out")).map_err(|err| err.to_string())?;
    let child = mit(KRB5KDC, dir)
        .args(["-n", "-r", REALM])
        .stdout(log.try_clone().map_err(|err| err.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|err| format!("starting krb5kdc: {err}"))?;
    let kdc = Kdc(child);
    // It listens once it has read its database.
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if Instant::now() > deadline {
            return Err(format!("krb5kdc did not listen on port {port}: see {DIR}/krb5kdc.out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(kdc)
}

/// The lines of the KDC's log the exchanges must have made.
fn check_log(dir: &Path) -> Result<(), String> {
    let mut log = String::new();
    std::fs::File::open(dir.join("kdc.log"))
        .and_then(|mut file| file.read_to_string(&mut log))
        .map_err(|err| format!("reading the KDC's log: {err}"))?;
    let found = |needles: &[&str]| log.lines().any(|line| needles.iter().all(|n| line.contains(n)));
    let wanted: &[(&[&str], &str)] = &[
        (&["NEEDED_PREAUTH: alice@DUKOVO.TEST"], "asked alice to prove herself"),
        (&["AS_REQ", "ISSUE", "alice@DUKOVO.TEST for krbtgt/DUKOVO.TEST@DUKOVO.TEST"], "gave alice her ticket"),
        (&["PREAUTH_FAILED: alice@DUKOVO.TEST"], "refused the wrong password"),
        (&["TGS_REQ", "ISSUE", "alice@DUKOVO.TEST for nfs/server.dukovo.test@DUKOVO.TEST"], "gave alice the service's ticket"),
        (&["AS_REQ", "ISSUE", "nfs/server.dukovo.test@DUKOVO.TEST for krbtgt/DUKOVO.TEST@DUKOVO.TEST"], "signed the service in with its keytab"),
        (&["AS_REQ", "ISSUE", "rep=aes128-cts-hmac-sha1-96(17)", "bob@DUKOVO.TEST"], "answered bob under his aes128-cts key"),
    ];
    for (needles, what) in wanted {
        if !found(needles) {
            return Err(format!("the KDC's log does not show that it {what}: see {DIR}/kdc.log"));
        }
    }
    Ok(())
}

fn kerberos(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let root: PathBuf = std::env::current_dir().map_err(|err| err.to_string())?;
    let dir = root.join(DIR);
    let port = free_port()?;
    let kdc = realm(&dir, port, "nfs.keytab")?;

    owner_signs_in(vm, case)?;
    vm.expect("authd: keyring of alice: open")?;
    // The realm's KDC, at the host, in the settings' krb5.conf.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate -v && elevate sh -c 'printf \"[libdefaults]\\n default_realm = {REALM}\\n[realms]\\n \
             {REALM} = {{\\n kdc = 10.0.2.2:{port}\\n }}\\n\" > /system/settings/krb5.conf' && \
             echo krb5.conf in place | tr a-z A-Z"
        ),
    )?;
    answer_prompt(vm, "unlock elevate in this pane:", "queen of hearts")?;
    vm.expect_screen(&["KRB5.CONF IN PLACE"])?;
    // Alice's password gets her ticket, which her keyring keeps.
    answer_prompt(vm, "$", "clear; kinit alice")?;
    answer_prompt(vm, "Password for alice@DUKOVO.TEST:", "drink-me")?;
    answer_prompt(vm, "$", "clear; klist | tr a-z A-Z; keyring list | tr a-z A-Z")?;
    vm.expect_screen(&[
        "DEFAULT PRINCIPAL: ALICE@DUKOVO.TEST",
        "  KRBTGT/DUKOVO.TEST@DUKOVO.TEST",
        "FLAGS: IA",
        "KRB5 ALICE@DUKOVO.TEST KRBTGT/DUKOVO.TEST@DUKOVO.TEST",
    ])?;
    // A wrong one is refused, and the ticket stays.
    answer_prompt(vm, "$", "clear; kinit alice; echo \"status $?\"; klist -s; echo \"kept $?\"")?;
    answer_prompt(vm, "Password for alice@DUKOVO.TEST:", "drink-you")?;
    vm.expect_screen(&["alice@DUKOVO.TEST: the password or key is wrong", "status 1", "kept 0"])?;
    // A service's ticket, taken with hers, and its key's version.
    answer_prompt(vm, "$", &format!("clear; kvno {SERVICE} | tr a-z A-Z; keyring list | tr a-z A-Z"))?;
    vm.expect_screen(&[
        "NFS/SERVER.DUKOVO.TEST@DUKOVO.TEST: KVNO = 2",
        "KRB5 ALICE@DUKOVO.TEST NFS/SERVER.DUKOVO.TEST@DUKOVO.TEST",
    ])?;
    // The service's keytab, fetched, listed, and signed in with.
    answer_prompt(vm, "$", "clear; nc -d 10.0.2.100 80 > nfs.keytab; klist -k -e nfs.keytab | tr a-z A-Z")?;
    vm.expect_screen(&["   2 NFS/SERVER.DUKOVO.TEST@DUKOVO.TEST (AES256-CTS-HMAC-SHA1-96)"])?;
    answer_prompt(vm, "$", &format!("clear; kinit -V -k -t nfs.keytab {SERVICE}"))?;
    vm.expect_screen(&["Authenticated to Kerberos v5 as nfs/server.dukovo.test@DUKOVO.TEST"])?;
    // Bob, with only an aes128-cts key, in place of the service.
    answer_prompt(vm, "$", "clear; kinit -V bob")?;
    answer_prompt(vm, "Password for bob@DUKOVO.TEST:", "eat-me")?;
    vm.expect_screen(&["Authenticated to Kerberos v5 as bob@DUKOVO.TEST"])?;
    answer_prompt(vm, "$", "clear; klist | tr a-z A-Z")?;
    vm.expect_screen(&["DEFAULT PRINCIPAL: BOB@DUKOVO.TEST"])?;
    // Every ticket thrown away; the keyring's other entries stay.
    answer_prompt(
        vm,
        "$",
        "clear; kdestroy; klist; echo \"status $?\"; klist -s; echo \"silent $?\"; echo \"left \
         $(keyring list | grep -c krb5)\"",
    )?;
    vm.expect_screen(&["your keyring holds no Kerberos tickets", "status 1", "silent 1", "left 0"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    drop(kdc);
    check_log(&dir)
}
