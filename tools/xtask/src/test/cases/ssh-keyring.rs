// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! OpenSSH takes key passphrases from the keyring (ADR 0395). Alice's
//! account is made at the first boot, and her session names
//! ssh-askpass-keyring as OpenSSH's askpass. She makes an Ed25519 key
//! with a passphrase. ssh-add, giving it to an agent, asks for the
//! passphrase once, through ssh-askpass-keyring, which keeps it in her
//! keyring once it opens the key. A second agent then takes the key with
//! nothing asked, and ssh-keygen signs a file with it, as git signs a
//! commit, with nothing asked either. The passphrase changed with
//! `ssh-keygen -p`, the one kept no longer opens the key: ssh-add asks
//! again, and the new one is kept in its place.
//!
//! What is asked is written on the terminal and read from it, whatever
//! the commands' output: each command writes its output to a file, which
//! the next shows, so that a question asked when none should be would
//! leave the command waiting and the test failing.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "ssh-keyring",
    about: "OpenSSH takes key passphrases from the keyring (ADR 0395): ssh-add asks once, through ssh-askpass-keyring, which keeps the passphrase; another agent takes the key and ssh-keygen signs with it, asking nothing; a passphrase changed with ssh-keygen -p is asked for again and kept in place of the old",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // A normal boot, as a computer's first.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: ssh_keyring,
    disks: &[],
    second: None,
};

/// Where the key is, and how ssh-add and ssh-askpass-keyring end their
/// question for it.
const KEY: &str = "/users/alice/.ssh/id_ed25519";
const ASKED: &str = "id_ed25519:";

/// Gives the key to a new agent, with ssh-add's output and status in
/// /tmp/out.
const ADD: &str = "clear; ssh-agent /bin/sh -c 'ssh-add ~/.ssh/id_ed25519; echo \"added: $?\"' > \
                   /tmp/out 2>&1";

fn ssh_keyring(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect("authd: keyring of alice: open")?;
    // Her key, sealed with a passphrase, and her session's askpass.
    answer_prompt(
        vm,
        "$",
        "clear; mkdir -m 700 -p ~/.ssh && ssh-keygen -q -t ed25519 -C alice -N 'down the hole' \
         -f ~/.ssh/id_ed25519; echo \"askpass: $SSH_ASKPASS_REQUIRE $?\" | tr a-z A-Z",
    )?;
    vm.expect_screen(&["ASKPASS: FORCE 0"])?;
    // ssh-add asks once; what she types is kept, as it opens the key.
    answer_prompt(vm, "$", ADD)?;
    answer_prompt(vm, ASKED, "down the hole")?;
    answer_prompt(vm, "$", "clear; tr a-z A-Z < /tmp/out; keyring list | tr a-z A-Z")?;
    vm.expect_screen(&["IDENTITY ADDED", "ADDED: 0", &format!("SSH {}", KEY.to_uppercase())])?;
    // A second agent, and ssh-keygen signing, ask nothing: a question
    // would leave them waiting.
    answer_prompt(vm, "$", ADD)?;
    answer_prompt(
        vm,
        "$",
        "clear; echo 'eat me' > /tmp/note; ssh-keygen -Y sign -n file -f ~/.ssh/id_ed25519 \
         /tmp/note >> /tmp/out 2>&1; echo \"signed: $?\" >> /tmp/out",
    )?;
    answer_prompt(vm, "$", "clear; tr a-z A-Z < /tmp/out; head -1 /tmp/note.sig")?;
    vm.expect_screen(&["IDENTITY ADDED", "ADDED: 0", "SIGNED: 0", "-----BEGIN SSH SIGNATURE-----"])?;
    // The passphrase changed, the one kept opens the key no more: ssh-add
    // asks, and the new one takes its place.
    answer_prompt(
        vm,
        "$",
        "clear; ssh-keygen -q -p -P 'down the hole' -N 'through the glass' -f ~/.ssh/id_ed25519",
    )?;
    answer_prompt(vm, "$", ADD)?;
    answer_prompt(vm, ASKED, "through the glass")?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; tr a-z A-Z < /tmp/out; keyring get 'ssh {KEY}' | tr a-z A-Z"),
    )?;
    vm.expect_screen(&["ADDED: 0", "THROUGH THE GLASS"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
