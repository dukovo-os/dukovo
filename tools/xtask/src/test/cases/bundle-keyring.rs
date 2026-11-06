// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An app in a sandbox reaches its own keyring entries alone (ADR 0396).
//! Alice signs in, as the owner, keeps a secret of her own in her
//! keyring, and installs into her applications a bundle whose command
//! keeps a secret, reads it back, lists the keyring, reads hers and
//! locks the keyring, all from its sandbox. Its first run asks about
//! keyring.shared with a warning on the console's bottom line, and D
//! leaves it out: the app reads its own secret and lists only its own
//! entry, while hers is not found and the lock is refused. Outside, its
//! entry is `app ID NAME`. bundle grant gives it keyring.shared after the
//! prompt's Y, and run again it reads her secret. The power button ends
//! it.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "bundle-keyring",
    about: "an app in a sandbox reaches its own keyring entries alone (ADR 0396): it keeps and reads a secret and lists only its own, alice's is not found and a lock is refused, its entry is `app ID NAME` outside; given keyring.shared on the trusted prompt's warning, it reads hers",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled, authd and
    // consoled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: bundle_keyring,
    disks: &[],
    second: None,
};

/// The bundle's command, a script run in its sandbox: keeps a secret,
/// reads it back and lists the keyring, each in capitals, then reads
/// alice's own secret and locks the keyring, each with its status.
const TOKENS: &str = "#!/bin/sh\\necho eat me | keyring set cake; echo \"set $?\"\\nkeyring get \
                      cake | tr a-z A-Z; echo\\necho $(keyring list | tr a-z A-Z) \"<- \
                      listed\"\\nkeyring get bottle; echo \"bottle $?\"\\nkeyring lock; echo \
                      \"lock $?\"\\n";

/// Its permissions, as its manifest writes them.
const PERMISSIONS: &str = r#"{"keyring.shared": {"need": "optional"}}"#;

fn bundle_keyring(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere("asks for passwords on the consoles")?;
    vm.expect_anywhere("authd: keyring of alice: open")?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; echo drink me | keyring set bottle; bundle new org.example.tokens --name \
             Tokens > /dev/null; printf '{TOKENS}' > tokens/bin/tokens; chmod 755 \
             tokens/bin/tokens; sed -i 's/\"permissions\": {{}}/\"permissions\": {PERMISSIONS}/' \
             tokens/manifest.json; bundle pack tokens -o tokens.dkv > /dev/null; bundle install \
             --yes tokens.dkv | grep -e keyring -e installed; rm -r tokens tokens.dkv"
        ),
    )?;
    vm.expect_screen(&[
        "Use every secret in your keyring, other apps' too (with a warning, optional)",
        "installed org.example.tokens 0.1.0, for you",
    ])?;
    // Its first run asks, with a warning; D leaves keyring.shared out.
    answer_prompt(vm, "$", "clear; tokens; echo \"status $?\"")?;
    vm.expect_screen(&["[warning] Tokens (org.example", "D: don't allow"])?;
    vm.type_text("d")?;
    vm.expect_anywhere(
        "bundled: alice did not allow org.example.tokens keyring.shared, from now on, on pts/",
    )?;
    // Its own secret and its own entry alone; not hers, and no lock.
    vm.expect_screen(&[
        "set 0",
        "EAT ME",
        "CAKE <- listed",
        "your keyring has no entry bottle",
        "bottle 1",
        "reaches its keyring alone",
        "lock 1",
        "status 0",
    ])?;
    // Outside its sandbox, its entry has its ID, beside hers.
    answer_prompt(vm, "$", "clear; keyring list | tr a-z A-Z")?;
    vm.expect_screen(&["APP ORG.EXAMPLE.TOKENS CAKE", "BOTTLE"])?;
    // Given keyring.shared, after the prompt's Y, it reads hers.
    answer_prompt(
        vm,
        "$",
        "clear; bundle grant org.example.tokens keyring.shared; echo \"granted $?\"",
    )?;
    vm.expect_screen(&["Give Tokens (org.example", "Y: yes"])?;
    vm.type_text("y")?;
    vm.expect_screen(&["gave org.example.tokens keyring.shared", "granted 0"])?;
    answer_prompt(vm, "$", "clear; tokens; echo \"status $?\"")?;
    vm.expect_screen(&["drink me", "bottle 0", "lock 1", "status 0"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
