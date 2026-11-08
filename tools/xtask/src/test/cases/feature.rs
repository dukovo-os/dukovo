// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! feature(8) at the console (ADR 0316): *feature* lists File sharing,
//! the system's feature of smbd and nfsd, as not added, and *feature
//! info* says each service is off. *feature add file-sharing* turns both
//! on, so that Spark runs them, and the list says it is added; *share
//! off* turns smbd off behind its back, and the list says it is partly
//! added. *feature remove* turns nfsd off too, and an ID that names no
//! feature, or is not one, is refused with what to do.

use super::super::*;

pub const CASE: Case = Case {
    name: "feature",
    about: "feature at the console: File sharing listed as not added, its services off; feature add turns smbd and nfsd on, which Spark runs; share off leaves it partly added; feature remove turns the rest off; an unknown or malformed ID refused (ADR 0316)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: feature,
    disks: &[],
    second: None,
};

/// The `feature` test.
fn feature(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    vm.type_text("feature; feature info file-sharing | tail -2\n")?;
    vm.expect_screen(&[
        "file-sharing  not added  File sharing",
        "  service smbd: off",
        "  service nfsd: off",
    ])?;

    vm.type_text("clear; feature add file-sharing && feature list | tail -1\n")?;
    vm.expect_screen(&[
        "File sharing is added: smbd and nfsd run, and start at each boot.",
        "file-sharing  added      File sharing",
    ])?;
    vm.expect_anywhere("spark: started smbd (/system/commands/smbd) as process ")?;
    vm.expect_anywhere("spark: started nfsd (/system/commands/nfsd) as process ")?;
    vm.type_text("service --status-all | grep -e smbd -e nfsd\n")?;
    vm.expect_screen(&[" [ + ]  nfsd", " [ + ]  smbd"])?;

    vm.type_text("clear; share off; feature list | tail -1\n")?;
    vm.expect_screen(&["file-sharing  partly     File sharing"])?;
    vm.type_text("feature remove file-sharing; service --status-all | grep -c -e '- ]  smbd' -e '- ]  nfsd'\n")?;
    vm.expect_screen(&["File sharing is removed.", "2"])?;

    vm.type_text("clear; feature add nothing; echo \"status: $?\"; feature info File; echo \"status: $?\"\n")?;
    vm.expect_screen(&[
        "no feature is called nothing: `feature list` lists them",
        "status: 1",
        "`File` is not a feature's ID",
        "status: 1",
    ])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
