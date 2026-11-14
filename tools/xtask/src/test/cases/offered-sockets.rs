// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sockets a bundle offers to others (ADRs 0385 and 0477). A second disk
//! holds seven test bundles: three providers, whose service has listenfd
//! (userspace/posix/listenfd.c) answer on a socket `echo` that Spark
//! makes in the service's live folder, offered to `approved`, `publisher`
//! and `anyone`; a consumer of each, whose service asks the
//! socket for its line and logs what it got, or `no answer`; and a
//! stranger, which names no use of the approved provider's socket and
//! asks it all the same. Alice installs them for everyone, with `--yes`,
//! which approves the socket the approved provider offers to the
//! programs she approves. Each consumer is answered, and the stranger
//! is not: its view does not hold the socket. Alice's own `nc -U`
//! is answered by the socket offered to anyone and refused by the other
//! two, which are the bundles' alone (a person's own programs reaching
//! them waits for `bundle allow`, docs/deferred.md).

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "SERVICES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the bundles' files.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    service_files_disk(8 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "offered-sockets",
    about: "sockets a bundle offers to others (ADRs 0385 and 0477): a consumer is answered by the socket of a provider that offers it to anyone, to the publisher's bundles, or to the approved, which the install's yes approves; a bundle that does not use the approved one's socket cannot reach it, and Alice reaches only the one offered to anyone",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: offered_sockets,
    disks: DISKS,
    second: None,
};

/// Where a provider's socket is, as the system sees it: its service's
/// account's live folder.
fn socket_path(provider: &str) -> String {
    format!("/system/live/services/{provider}/echo")
}

/// The seven bundles, the providers first. The disk holds the other
/// tests' bundles too, which this test leaves alone.
const BUNDLES: &str = "anyprov echoprov pubprov anyuser echouser pubuser stranger";

/// Installs the seven bundles for everyone, as an administrator: the first
/// elevate asks for her password.
fn install_all(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; for b in {BUNDLES}; do f=org.example.$b-1.0.0.dkv; elevate \
             bundle install --for-everyone --yes $f > /dev/null 2>&1; echo \"installed $f $?\"; \
             done; cd"
        ),
    )?;
    answer_prompt(vm, "org.example.anyprov-1.0.0.dkv:", "queen of hearts")?;
    // Seven installs take longer than a screen check waits.
    answer_prompt_within(vm, "$", "", 120)?;
    vm.expect_screen(&["installed org.example.stranger-1.0.0.dkv 0"])
}

/// Alice asks `provider`'s socket for its line: what she gets, in the
/// `reply` line.
fn alice_asks(vm: &mut Vm, provider: &str) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; echo | nc -w 5 -U {} > /tmp/said 2>&1; echo \"reply $? $(head -c 60 \
             /tmp/said)\"",
            socket_path(provider)
        ),
    )
}

fn offered_sockets(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    install_all(vm)?;
    // The providers listen, on the sockets Spark made.
    for provider in ["echoprov", "pubprov", "anyprov"] {
        vm.expect_anywhere(&format!("{provider}: listenfd: serving 1 sockets"))?;
    }

    // Offered to anyone: the consumer and Alice both are answered.
    vm.expect_anywhere("anyuser: hello from echo")?;
    alice_asks(vm, "anyprov")?;
    vm.expect_screen(&["reply 0 hello from echo"])?;

    // Offered to the publisher's bundles: the consumer is the same
    // publisher's and is answered; Alice is refused.
    vm.expect_anywhere("pubuser: hello from echo")?;
    alice_asks(vm, "pubprov")?;
    vm.expect_screen(&["reply 1"])?;

    // Offered to the approved: the consumer was approved by the install's
    // yes and is answered; a bundle that names no use of it is not, and
    // neither is Alice.
    vm.expect_anywhere("echouser: hello from echo")?;
    vm.expect_anywhere("stranger: no answer")?;
    alice_asks(vm, "echoprov")?;
    vm.expect_screen(&["reply 1"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
