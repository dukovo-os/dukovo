// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A cloud image boots with a config disk and applies it (ADR 0328), as
//! Proxmox, libvirt and QEMU's users give one. The disk is FAT, labelled
//! CIDATA, with NoCloud's three files: *meta-data*, naming the instance
//! and the machine and holding the owner's SSH key, which the test makes;
//! *user-data*, `#cloud-config` with the default account, an
//! administrator whose sudo rule has NOPASSWD and a key of her own, a
//! person with a password, two files to write, one in base64, two
//! commands, and a key cloud-setup leaves out; and *network-config*,
//! netplan's version 2, giving the card by its MAC address an address of
//! its own. The image's disk is larger than its root, as a cloud's is.
//!
//! cloud-setup grows the root's partition and Btrfs over the disk, names
//! the machine, sets the card up, makes the three accounts through authd,
//! writes the files with their owners and modes, and runs the commands,
//! saying in the log what it left out. The host's OpenSSH client signs in
//! as the default account with the owner's key and as the administrator
//! with hers: each elevates without a password, and she reads the file
//! written for her. Run again, cloud-setup finds the instance set up.

use std::path::Path;
use std::process::Command;

use super::super::*;

/// The keys' and files' folder, from the top of the tree.
const DIR: &str = "out/test/cloud-config";

/// The card's MAC address, which the network configuration names.
const MAC: &str = "52:54:00:c1:0d:01";

/// The address the network configuration gives the card.
const ADDRESS: &str = "10.0.2.77";

/// The instance's ID.
const INSTANCE: &str = "iid-dukovo-test";

/// The test's network: QEMU's user network, on a card of [`MAC`].
const DEVICE: &str = "virtio-net-pci,netdev=n0,mac=52:54:00:c1:0d:01";

pub const CASE: Case = Case {
    name: "cloud-config",
    about: "A cloud image boots with a NoCloud config disk and applies it: the root grown over a larger disk, the machine named, a card given its address by MAC, the default account and two more made through authd, files written with owners, modes and base64, commands run, what is left out logged; the host's ssh signs in with the cloud's keys and elevates without a password; run again, the instance is set up already (ADR 0328)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Cloud,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", DEVICE],
    script: applied,
    disks: &[cloud_config_disk(2048)],
    second: None,
};

/// The base64 file's text.
const DECODED: &str = "decoded: 42\n";

/// Makes the config disk at `path`: FAT, labelled as `disk` says, with
/// the files of [`files`], after making the owner's and ann's keys.
pub fn make(disk: &Disk, path: &Path) -> Result<(), String> {
    let dir = Path::new(DIR);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|err| format!("removing {DIR}: {err}"))?;
    }
    std::fs::create_dir_all(dir).map_err(|err| format!("making {DIR}: {err}"))?;
    for name in ["owner", "ann"] {
        let made = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("cloud-{name}"), "-f", name])
            .current_dir(dir)
            .status()
            .map_err(|err| format!("the test needs the host's ssh-keygen: install openssh-client ({err})"))?;
        if !made.success() {
            return Err(format!("ssh-keygen could not make the {name} key"));
        }
    }
    let key = |name: &str| {
        std::fs::read_to_string(dir.join(format!("{name}.pub")))
            .map(|key| key.trim().to_string())
            .map_err(|err| format!("reading the {name} key: {err}"))
    };
    let mut mkfs = Command::new(fat_tool("mkfs.fat"));
    mkfs.args(["-F", "12", "-n", disk.label, "-i", "4f525448", "-C"]).arg(path).arg(disk.kib.to_string());
    host_tool(&mut mkfs)?;
    for (name, text) in files(&key("owner")?, &key("ann")?) {
        let file = dir.join(name);
        std::fs::write(&file, text).map_err(|err| format!("writing {}: {err}", file.display()))?;
        host_tool(Command::new("mcopy").arg("-i").arg(path).arg(&file).arg(format!("::/{name}")))?;
    }
    Ok(())
}

/// NoCloud's files, with the owner's key and ann's.
fn files(owner: &str, ann: &str) -> [(&'static str, String); 3] {
    let meta = format!(
        "instance-id: {INSTANCE}\nlocal-hostname: cloudy.example.org\npublic-keys:\n  - {owner}\n"
    );
    let user = format!(
        "#cloud-config
users:
  - default
  - name: ann
    gecos: Ann Admin
    groups: users, sudo
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - {ann}
  - name: bob
    lock_passwd: false
    plain_text_passwd: bob-password-1
write_files:
  - path: /users/ann/hello.txt
    content: |
      Hello from the cloud.
    owner: ann:ann
    permissions: '0640'
  - path: /system/state/cloud-test/decoded
    encoding: b64
    content: ZGVjb2RlZDogNDIK
runcmd:
  - echo \"runcmd $((40 + 2))\" > /system/state/cloud-test/ran
  - [touch, /system/state/cloud-test/touched]
packages: [nginx]
"
    );
    let network = format!(
        "version: 2
ethernets:
  eth0:
    match:
      macaddress: '{MAC}'
    addresses: [{ADDRESS}/24]
    gateway4: 10.0.2.2
    nameservers:
      addresses: [10.0.2.3]
"
    );
    [("meta-data", meta), ("user-data", user), ("network-config", network)]
}

/// The host's ssh as `user` with the key `key`, through `port`, running
/// `command`: its output, or why it failed.
fn ssh(port: u16, key: &str, user: &str, command: &str) -> Result<String, String> {
    let ran = Command::new("ssh")
        .args(["-F", "/dev/null", "-p", &port.to_string(), "-i"])
        .arg(Path::new(DIR).join(key))
        .args(["-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null"])
        .args(["-o", "LogLevel=ERROR", "-o", "ConnectTimeout=30"])
        .arg(format!("{user}@127.0.0.1"))
        .arg(command)
        .output()
        .map_err(|err| format!("the test needs the host's ssh: install openssh-client ({err})"))?;
    if !ran.status.success() {
        return Err(format!(
            "ssh as {user} running {command:?} ended with {}: {}",
            ran.status,
            String::from_utf8_lossy(&ran.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&ran.stdout).into_owned())
}

/// The machine set up as the config disk says, then checked from the
/// console and over SSH.
fn applied(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: /system is Dukovo's root on ")?;
    vm.expect(&format!("cloud-setup: instance {INSTANCE} from /volumes/"))?;
    vm.expect("cloud-setup: left out packages: Dukovo installs apps with bundle(1)")?;
    vm.expect("cloud-setup: naming the machine cloudy")?;
    vm.expect("cloud-setup: setting the network cards up as the network configuration says")?;
    vm.expect("cloud-setup: making an administrator's account, dukovo, with 1 SSH key(s)")?;
    vm.expect("cloud-setup: making an administrator's account, ann, with 1 SSH key(s)")?;
    vm.expect("cloud-setup: making an account, bob, with 0 SSH key(s)")?;
    vm.expect("cloud-setup: writing /users/ann/hello.txt (22 bytes, mode 640)")?;
    vm.expect(&format!("cloud-setup: instance {INSTANCE} set up\n"))?;
    let log = vm.log();
    for grown in ["cloud-setup: /devices/sda3 grows from 512 MiB to 768 MiB.", "Btrfs: disk 1 resized; it is 768 MiB now"] {
        if !log.contains(grown) {
            return Err(format!("the root did not grow over the disk: no \"{grown}\" in the log"));
        }
    }
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text(
        "clear; hostname; cat /system/state/cloud-test/ran /system/state/cloud-test/decoded; \
         ls /system/state/cloud-test\n",
    )?;
    vm.expect_screen(&["cloudy", "runcmd 42", DECODED.trim(), "touched"])?;
    vm.type_text(
        "clear; ip address | grep -q 10.0.2.77/24 && echo \"address: $((70 + 7))\"; \
         stat -c '%U %G %a' /users/ann/hello.txt\n",
    )?;
    vm.expect_screen(&["address: 77", "ann ann 640"])?;
    vm.type_text("clear; id ann; id bob\n")?;
    vm.expect_screen(&["(admins)", "(elevate-without-password)", "uid=1002(bob)"])?;
    vm.type_text("clear; cloud-setup\n")?;
    vm.expect_screen(&[&format!("cloud-setup: instance {INSTANCE} was set up at an earlier boot")])?;

    // The host signs in with the keys the cloud gave.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map_err(|err| format!("no free port: {err}"))?
        .port();
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{port}-{ADDRESS}:22"))?;
    let owner = ssh(port, "owner", "dukovo", "elevate id -u")?;
    if owner.trim() != "0" {
        return Err(format!("dukovo did not elevate without a password: {owner:?}"));
    }
    let ann = ssh(port, "ann", "ann", "cat hello.txt; elevate id -u")?;
    if ann != "Hello from the cloud.\n0\n" {
        return Err(format!("ann did not read her file and elevate: {ann:?}"));
    }
    // The owner's key is the default account's alone.
    if ssh(port, "owner", "ann", "true").is_ok() {
        return Err("the owner's key signed in as ann".into());
    }
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
