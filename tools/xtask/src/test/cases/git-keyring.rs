// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! git keeps its tokens in the keyring (ADR 0394), and pushes over
//! https after a reboot without asking again: the roadmap's 13g is done
//! when this passes. Dukovo's root is on a Btrfs disk, which the second
//! boot starts from again. Alice's account is made at the first boot. In
//! her session she starts git-https-test's server on lo, gives git the
//! token once with `git credential approve`, which git-credential-keyring,
//! the system's helper, keeps in her keyring, and clones, commits and
//! pushes with git asking nothing. No file of hers holds the token, and
//! the keyring's file, which root reads through elevate, holds it neither
//! in text nor in hexadecimal. At the second boot she signs in, starts
//! the server afresh, and clones, commits and pushes again, with git
//! still asking nothing; the server's repository has the commit.

use super::super::*;

/// Dukovo's root on a copy of the test image.
const DISKS: &[Disk] =
    &[Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) }];

/// The test.
pub const CASE: Case = Case {
    name: "git-keyring",
    about: "git keeps its tokens in the keyring (ADR 0394): given once, a token git-credential-keyring keeps lets git clone, commit and push over https without asking, in no file of the person's and not in the clear in the keyring's, and again after a reboot",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: before_the_reboot,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: "lockdep", before: nothing_between, script: after_the_reboot }),
};

/// Starts git-https-test's server in Alice's session, which makes its
/// repository and the test authority's certificate afresh in her /tmp,
/// and goes on serving once the command returns.
fn serve(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(vm, "$", "clear; /system/tests/posix/git-https-test --serve | tr a-z A-Z")?;
    vm.expect_screen(&["GIT-HTTPS-TEST: SERVING HTTPS://127.0.0.1:8443/REPO.GIT"])
}

/// Clones, commits `subject` and pushes, with git asking for nothing:
/// with no terminal prompts, a token it lacked would fail the push.
fn push(vm: &mut Vm, subject: &str) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; rm -rf /tmp/work; env GIT_TERMINAL_PROMPT=0 git clone -q \
             https://127.0.0.1:8443/repo.git /tmp/work && cd /tmp/work && echo more >> file && \
             git commit -qam '{subject}' && env GIT_TERMINAL_PROMPT=0 git push -q origin main; \
             echo \"pushed: $?\"; git -C /tmp/git-https/srv/repo.git log -1 --format=%s main | tr \
             a-z A-Z"
        ),
    )?;
    vm.expect_screen(&["pushed: 0", &subject.to_uppercase()])
}

fn before_the_reboot(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    // git's settings, kept in her home across the reboot: who commits,
    // and the test authority, whose certificate the server writes.
    answer_prompt(
        vm,
        "$",
        "clear; git config --global user.name Alice; git config --global user.email \
         alice@dukovo; git config --global http.sslCAInfo /tmp/git-https/ca.pem; git config \
         --system credential.helper | tr a-z A-Z",
    )?;
    vm.expect_screen(&["KEYRING"])?;
    serve(vm)?;
    // The token, given once, as a person's first push asks for it.
    answer_prompt(
        vm,
        "$",
        "clear; printf 'protocol=https\\nhost=127.0.0.1:8443\\nusername=test\\npassword=s3c%s\\n\\n' \
         ret | git credential approve; keyring list | tr a-z A-Z",
    )?;
    vm.expect_screen(&["GIT HTTPS://TEST@127.0.0.1:8443"])?;
    push(vm, "Before the reboot")?;
    // The token is in no file of hers, nor in the clear in the keyring's.
    answer_prompt(
        vm,
        "$",
        "clear; echo \"her files: $(grep -rl 's3[c]ret' ~ /tmp/work | wc -l)\"; elevate -v && \
         echo \"the keyring: $(elevate cat /system/state/authd/keyrings/1000 | grep -c -e \
         's3[c]ret' -e 733363726574)\"; elevate -k",
    )?;
    answer_prompt(vm, "unlock elevate in this pane:", "queen of hearts")?;
    vm.expect_screen(&["her files: 0", "the keyring: 0"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

fn after_the_reboot(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", "queen of hearts")?;
    vm.expect_anywhere("authd: keyring of alice: open")?;
    serve(vm)?;
    push(vm, "After the reboot")?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
