// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The installer takes its setup file from the web, over HTTPS only and
//! checked against a SHA-256 when one is given (ADR 0319). On the live
//! system, the test's answers files are served by openssl's *s_server*
//! twice: on port 8443 with a certificate from a test authority the
//! system is told to trust, and on port 8444 with one nothing trusts.
//! *dukovo-install --answers* refuses an `http://` address, the server it
//! cannot trust, and a file that differs from the SHA-256 given. On the
//! screens, *From a setup file* offers the web, as the stick holds none:
//! a SHA-256 that differs is refused there too, and with the right one
//! the review shows the file's answers and where they came from. The
//! install log names both downloads, and the blank disk stays blank, as
//! nothing is written before the review's Install. *install-bios*
//! installs from such a file, end to end.

use std::io::Read as _;

use super::super::*;
use super::install_uefi::{live_shell, wait_for_screen};

pub const CASE: Case = Case {
    name: "install-web",
    about: "Setup files from the web (ADR 0319): dukovo-install --answers refuses an http:// address, a server it cannot trust and a file that differs from its SHA-256; the screens download one from an https:// address, refuse it when the SHA-256 differs and review it when it matches, and the blank disk stays blank",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // The live menu entry has its own flags, `live` and
    // `console-sign-in=off`, before these.
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: web,
    disks: BLANK_DISKS,
    second: None,
};

/// The blank disk the review offers to erase, `vda`, as install-uefi's,
/// which must stay blank.
const BLANK_DISKS: &[Disk] = &[Disk {
    kib: 4 * 1024 * 1024,
    fat: INSTALL_TARGET,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: still_blank,
}];

/// The web server the live system trusts, with the answers files of
/// userspace/programs/installer/tests/install.
pub const ADDRESS: &str = "https://127.0.0.1:8443";

/// The web server with a certificate nothing trusts.
const UNTRUSTED: &str = "https://127.0.0.1:8444";

/// The program that starts both servers.
const SERVER: &str = "/system/tests/install/https-server.sh";

/// A SHA-256 no file has.
const WRONG_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Starts the setup files' web servers from the live system's shell, and
/// gives the SHA-256 of answers.json, which the server printed.
pub fn serve(vm: &mut Vm) -> Result<String, String> {
    vm.type_text(&format!("clear; sh {SERVER}\n"))?;
    // Making the keys takes a moment without KVM.
    wait_for_screen(vm, "serving setup files", 90)?;
    let screen = vm.screen()?;
    screen
        .iter()
        .find_map(|line| {
            let sha256 = line.trim().strip_suffix("  answers.json")?;
            let hex = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
            hex.then(|| sha256.to_string())
        })
        .ok_or_else(|| format!("the server printed no SHA-256 of answers.json: {}", screen.join(" | ")))
}

fn web(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    live_shell(vm)?;
    let sha256 = serve(vm)?;

    // The command line refuses what it must not install from, each with
    // its reason and exit status 1. The label is split, so that the
    // command line does not hold it whole.
    let answers = format!("{ADDRESS}/answers.json");
    for (label, from, why) in [
        (
            "plain",
            "http://127.0.0.1:8443/answers.json".to_string(),
            "Only https:// addresses are used, so the file can't be changed on the way.",
        ),
        (
            "untrusted",
            format!("{UNTRUSTED}/answers.json"),
            "The setup file could not be downloaded: ",
        ),
        (
            "differs",
            format!("{answers} --sha256 {WRONG_SHA256}"),
            "The SHA-256 does not match the downloaded file. It was not used.",
        ),
    ] {
        let (first, rest) = label.split_at(2);
        vm.type_text(&format!(
            "clear; dukovo-install --answers {from}; echo \"{first}\"\"{rest}: $((100 + $?))\"\n"
        ))?;
        vm.expect_screen(&[why, &format!("{label}: 101")])?;
    }

    // The screens: From a setup file, where the web is chosen, as the
    // stick holds none.
    vm.type_text("clear; dukovo-install\n")?;
    vm.expect_screen(&["Step 1 of 6", "How do you want to install?", "From a setup file"])?;
    vm.press(&["down", "down", "down", "down", "spc", "ret"])?;
    vm.expect_screen(&[
        "Install from a setup file",
        "No setup file was found on this stick.",
        "Download it from a web address",
        "HTTPS only",
        "SHA-256 (optional)",
    ])?;

    // A SHA-256 that differs is refused, and the page stays.
    vm.press(&["down"])?;
    vm.type_text(&answers)?;
    vm.press(&["down"])?;
    vm.type_text(WRONG_SHA256)?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["The SHA-256 does not match the downloaded file. It was not used."])?;

    // The right one: the review shows the file's answers, and where they
    // came from.
    vm.press(&["backspace"; 64])?;
    vm.type_text(&sha256)?;
    vm.press(&["ret"])?;
    vm.expect_screen(&[
        "Step 6 of 6",
        &format!("Downloaded from {answers}, and its SHA-256 matches."),
        "alice (Alice Liddell), administrator",
        "America/Denver",
        "Type vda to confirm",
    ])?;

    // Back twice, to the welcome, and Live console, the button three
    // before its first field, leaves the screens for the shell, whose
    // prompt is all the screen shows once they have cleared it.
    vm.press(&["esc"])?;
    vm.expect_screen(&["Install from a setup file"])?;
    vm.press(&["esc"])?;
    vm.expect_screen(&["How do you want to install?"])?;
    vm.press(&["up", "up", "up", "ret"])?;
    vm.expect_screen(&["root@dukovo /"])?;

    // The log names both downloads from the screens: the one refused,
    // and the one used, with its SHA-256.
    vm.type_text(&format!(
        "clear; echo \"lo\"\"gged: $(grep -c -F 'setup file from {answers}' /tmp/install.log)\"; \
         echo \"che\"\"cked: $(grep -c -F 'SHA-256 {sha256}, checked)' /tmp/install.log)\"\n"
    ))?;
    vm.expect_screen(&["logged: 2", "checked: 1"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The disk the review offered to erase is still zeros at its start,
/// where a partition table would be: nothing was written to it.
fn still_blank(disk: &DiskImage, _log: &str) -> Result<(), String> {
    let shown = disk.path.display();
    let mut start = vec![0u8; 1 << 20];
    std::fs::File::open(&disk.path)
        .and_then(|mut file| file.read_exact(&mut start))
        .map_err(|err| format!("reading {shown}: {err}"))?;
    if start.iter().any(|&byte| byte != 0) {
        return Err(format!("{shown} was written to, though nothing was installed"));
    }
    Ok(())
}
