// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Each person's keyring, which authd keeps (ADR 0393). Alice's account
//! is made at the first boot, and her sign-in makes her keyring. She keeps
//! a secret in it, reads it back and lists it, and its file, which root
//! reads, holds neither the secret nor its name in the clear. Signed out
//! and in again, it opens again. passwd seals it under her new password, which then
//! opens it at a sign-in. Locked, it refuses, and `keyring unlock` opens
//! it with her password. Locked again, an administrator's new password
//! for her, set through elevate without her old one, empties it, and
//! passwd says so. The power button ends it.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "keyring",
    about: "each person's keyring (ADR 0393): a sign-in with a password makes and opens it, keyring set, get and list keep and read a secret, it opens again at the next sign-in, passwd seals it under the new password, keyring lock and unlock close and open it, and a password set without the old one empties a locked keyring and says so",
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
    script: keyring,
    disks: &[],
    second: None,
};

/// What authd says as Alice's keyring opens and closes.
const OPEN: &str = "authd: keyring of alice: open";
const CLOSED: &str = "authd: keyring of alice: closed";

/// Alice signs out, and in again with `password`; her keyring, if it is
/// open, closes, and opens again. Each Argon2id under emulation takes
/// seconds, so each step waits for authd's line before it reads the
/// screen.
fn sign_in_again(vm: &mut Vm, password: &str, open: bool) -> Result<(), String> {
    answer_prompt(vm, "$", "exit")?;
    if open {
        vm.expect(CLOSED)?;
    }
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", password)?;
    vm.expect(OPEN)
}

fn keyring(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect(OPEN)?;
    // A secret kept, read back as it was kept, and listed. Each command
    // prints what the screen is checked for in other words than it was
    // typed in.
    answer_prompt(
        vm,
        "$",
        "clear; printf 'drink %s' me | keyring set bottle; keyring get bottle; echo \" status \
         $?\"; keyring list | tr a-z A-Z",
    )?;
    vm.expect_screen(&["drink me", "status 0", "BOTTLE"])?;
    // Its file, which root reads through elevate, holds neither the
    // secret, in text or in hexadecimal, nor the entry's name.
    answer_prompt(
        vm,
        "$",
        "clear; elevate -v && elevate head -1 /system/state/authd/keyrings/1000 | tr a-z A-Z && \
         echo \"lines in the clear: $(elevate cat /system/state/authd/keyrings/1000 | grep -c -e \
         drink -e bottle -e 6472696e6b)\"; elevate -k",
    )?;
    answer_prompt(vm, "unlock elevate in this pane:", "queen of hearts")?;
    vm.expect_screen(&["DUKOVO-KEYRING 1", "lines in the clear: 0"])?;
    // Closed when her session ends, and opened at her next sign-in.
    sign_in_again(vm, "queen of hearts", true)?;
    answer_prompt(vm, "$", "clear; keyring get bottle | tr a-z A-Z")?;
    vm.expect_screen(&["DRINK ME"])?;
    // A new password seals it again, and opens it at a sign-in.
    answer_prompt(
        vm,
        "$",
        "clear; printf 'queen of hearts\\nqueen of spades\\n' | passwd 2>&1 | tr a-z A-Z",
    )?;
    vm.expect("authd: the password of alice was changed, by process ")?;
    vm.expect_screen(&["THE PASSWORD OF ALICE IS CHANGED."])?;
    sign_in_again(vm, "queen of spades", true)?;
    answer_prompt(vm, "$", "clear; keyring get bottle | tr a-z A-Z")?;
    vm.expect_screen(&["DRINK ME"])?;
    // Locked, it refuses; her password opens it.
    answer_prompt(vm, "$", "clear; keyring lock; keyring get bottle; echo \"status $?\"")?;
    vm.expect("authd: keyring of alice: locked, as they asked")?;
    vm.expect_screen(&[
        "keyring: your keyring is locked: it opens when you sign in with your password",
        "status 1",
    ])?;
    answer_prompt(vm, "$", "clear; keyring unlock && keyring get bottle | tr a-z A-Z")?;
    answer_prompt(vm, "Password:", "queen of spades")?;
    vm.expect(OPEN)?;
    vm.expect_screen(&["DRINK ME"])?;
    // Locked, an administrator's new password for her, without her old
    // one, cannot open it: it is emptied, and said.
    answer_prompt(
        vm,
        "$",
        "clear; keyring lock; printf 'march hare\\n' | elevate passwd alice 2>&1 | tr a-z A-Z",
    )?;
    vm.expect("authd: keyring of alice: locked, as they asked")?;
    answer_prompt(vm, "passwd alice:", "queen of spades")?;
    vm.expect("authd: emptied the keyring of alice: only the old password opened it")?;
    vm.expect_screen(&[
        "THE PASSWORD OF ALICE IS CHANGED.",
        "THE KEYRING OF ALICE WAS EMPTIED: ONLY THE OLD PASSWORD OPENED IT.",
    ])?;
    // The new password makes her a new keyring, with nothing in it.
    sign_in_again(vm, "march hare", false)?;
    answer_prompt(vm, "$", "clear; keyring list | wc -l; keyring get bottle; echo \"status $?\"")?;
    vm.expect_screen(&["your keyring has no entry bottle", "status 1"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
