// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rescue turns SSH on when the installed system had it on, and signs in
//! the system's administrators with their keys, over the host key the
//! system had (ADR 0540). The install stick installs Dukovo as *rescue*
//! does. From the installed disk alone, alice, the administrator, puts a
//! key of the test's in her authorized_keys, turns sshd on, and adds a
//! health check that never ends. The host's ssh-keyscan reads the host
//! key sshd made, through a port QEMU forwards, and the test keeps its
//! fingerprint. Three starts cut short by resets bring Rescue.
//!
//! Rescue's screen says that SSH is on, as it was on this computer. The
//! host's ssh signs alice in with her key, and takes only the host key the
//! installed system showed: Rescue's menu comes, told who signed in and
//! from where, with no password asked. A password is refused, since the
//! installed system's sshd_config keeps them off, and so is a command,
//! since an SSH session in Rescue is its menu alone. Over SSH alice
//! chooses to copy the files off, which waits at its question for the
//! disk. At the screen, where alice signs in too, the choice to check the
//! disk says that alice is copying the files off, over SSH from the
//! host's address, and to wait and choose again. Once the SSH side has
//! answered, the same choice at the screen takes the lock and checks the
//! disk. The power-off choice ends it.

use std::path::Path;

use super::super::*;
use super::help_session::{
    Session, as_root, fingerprint, free_port, keys, scan_host_key, ssh, with_alice_key,
    with_password,
};
use super::install_uefi::{INSTALL_DISKS, PASSWORD, install, nothing, wait_for_screen};
use super::rescue::{
    NEVER_HEALTHY, ROOT, alice_signs_in, choose, rescue_starts, sign_in_at_screen,
};

/// The keys' folder, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/rescue-ssh";

/// What the host's address is to the guest: QEMU's user network forwards
/// from 10.0.2.2.
const FROM: &str = "10.0.2.2";

/// How long a check of the root's disk may take, in seconds.
const DISK_CHECK_SECONDS: u64 = 120;

pub const CASE: Case = Case {
    name: "rescue-ssh",
    about: "Rescue with SSH on because the installed system had it on (ADR 0540): after an install from the stick, with alice's key in her authorized_keys and sshd on, three starts cut short start Rescue, whose screen says SSH is on; the host's ssh signs alice in with her key and sees the host key the installed system had, gets Rescue's menu told she signed in over SSH, and is refused a password and a command; her copying the files off over SSH holds the lock, so that the same choice at the screen, where she signs in too, says who holds it and from where, and takes it once she has answered",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: install,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: nothing, script: rescued }),
};

/// alice's key in her authorized_keys, sshd turned on, and a health check
/// that never ends, so that no start is seen through. `port` of the host
/// reaches sshd. The fingerprint of the host key sshd made.
fn prepared(vm: &mut Vm, dir: &Path, alice_key: &str, port: u16) -> Result<String, String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; mkdir -p ~/.ssh; echo '{alice_key}' > ~/.ssh/authorized_keys; echo \"key $((6 \
             * 7))\""
        ),
    )?;
    vm.expect_screen(&["key 42"])?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; {}; echo \"sshd $((6 * 7))\"", as_root("service sshd enable")),
    )?;
    vm.expect_screen(&["sshd 42"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; printf '{NEVER_HEALTHY}' > health.toml; {}; sync; echo \"kept: $((6 * 7))\"",
            as_root("cp /users/alice/health.toml /system/settings/sysupdate/health.toml")
        ),
    )?;
    vm.expect_screen(&["kept: 42"])?;
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-:22"))?;
    let line = scan_host_key(port)?;
    let known_hosts = dir.join("known_hosts");
    std::fs::write(&known_hosts, format!("{line}\n"))
        .map_err(|err| format!("writing {}: {err}", known_hosts.display()))?;
    fingerprint(dir, &line)
}

/// alice signs in over SSH with her key, as soon as Rescue's sshd takes
/// her: its session, which shows Rescue's menu. Tried for a minute and a
/// half, since the screen says SSH is on once the service started, not
/// once the card has its address.
fn menu_over_ssh(dir: &Path, port: u16) -> Result<Session, String> {
    let mut last = String::new();
    for _ in 0..30 {
        let mut command = ssh(dir, port);
        with_alice_key(&mut command, dir).args(["-tt", "alice@127.0.0.1"]);
        let mut menu = Session::start(command, "ssh as alice")?;
        match menu.wait_for("Choose 0 to 13:", 20) {
            Ok(()) => return Ok(menu),
            Err(err) => last = err,
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    Err(format!("alice could not sign in to Rescue over SSH: {last}"))
}

/// What Rescue's SSH refuses: a password, which the installed system's
/// sshd_config keeps off, and a command, since a session is the menu
/// alone.
fn refusals(dir: &Path, port: u16) -> Result<(), String> {
    let mut command = ssh(dir, port);
    with_password(&mut command, dir, PASSWORD).args(["alice@127.0.0.1", "true"]);
    let mut by_password = Session::start(command, "ssh as alice with a password")?;
    by_password.close_input();
    let status = by_password.finished(60)?;
    if status != Some(255) || !by_password.said().contains("Permission denied") {
        return Err(format!(
            "alice's password was not refused: ssh ended with {status:?} and said: {}",
            by_password.said().trim()
        ));
    }
    let mut command = ssh(dir, port);
    with_alice_key(&mut command, dir).args(["alice@127.0.0.1", "echo ran-$((6 * 7))"]);
    let mut by_command = Session::start(command, "ssh as alice with a command")?;
    by_command.close_input();
    let status = by_command.finished(60)?;
    if status == Some(0) || by_command.said().contains("ran-42") {
        return Err(format!(
            "a command was not refused: ssh ended with {status:?} and said: {}",
            by_command.said().trim()
        ));
    }
    Ok(())
}

/// The second boot, from the installed disk alone.
fn rescued(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|err| err.to_string())?.join(DIR);
    let alice_key = keys(&dir)?;
    vm.expect(&format!("spark: /system is Dukovo's root on {ROOT}"))?;
    vm.expect("sysupdate: this start of slot A (Dukovo ")?;
    alice_signs_in(vm)?;
    let port = free_port()?;
    let installed_key = prepared(vm, &dir, &alice_key, port)?;

    // Three starts in a row cut short before the health check sees them
    // through: Rescue starts by itself at the fourth.
    vm.monitor("system_reset")?;
    for _ in 0..3 {
        vm.expect("spark: started slot a, @system.a")?;
        vm.expect("spark: started logd (")?;
        vm.monitor("system_reset")?;
    }
    rescue_starts(vm, "SSH is on, as it was on this computer")?;

    // alice signs in over SSH, to the host key the system had.
    let mut menu = menu_over_ssh(&dir, port)?;
    menu.wait_for(&format!("alice signed in over SSH from {FROM}."), 30)?;
    menu.wait_for("What would you like to do?", 30)?;
    let line = scan_host_key(port)?;
    let shown = fingerprint(&dir, &line)?;
    if shown != installed_key {
        return Err(format!(
            "Rescue's SSH shows the host key {shown}, not {installed_key}, which the installed \
             system showed"
        ));
    }
    refusals(&dir, port)?;

    // Copying the files off waits at its question, and holds the lock.
    menu.send("6\r")?;
    menu.wait_for("Copy to which disk or partition", 60)?;
    sign_in_at_screen(vm)?;
    choose(vm, "5")?;
    vm.expect_screen(&[&format!(
        "alice is copying the users' and shared files off, over SSH from {FROM}: wait for it to \
         finish, then choose again"
    )])?;

    // Once the SSH side has answered, the lock is free.
    menu.send("\r")?;
    menu.wait_for("name a disk other than the root's", 60)?;
    choose(vm, "5")?;
    wait_for_screen(vm, &format!("The Btrfs on {ROOT} is sound."), DISK_CHECK_SECONDS)?;
    drop(menu);
    choose(vm, "13")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
