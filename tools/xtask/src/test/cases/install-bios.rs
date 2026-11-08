// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *install-uefi*, under a BIOS: the live system installs Dukovo onto
//! a blank disk from the test's answers file (ADR 0309), and the second
//! boot, from that disk alone, starts from the GRUB *rgrub-setup* put in
//! its first sector and BIOS boot partition. alice signs in and finds
//! the install's settings. The answers file comes from the web: a server
//! on the live system serves it over HTTPS, and *dukovo-install --answers
//! https://… --sha256* downloads it and checks it (ADR 0319), as
//! *install-web* tells.

use super::super::*;
use super::install_uefi::{install_by, live_shell};
use super::install_web::{ADDRESS, serve};

pub const CASE: Case = Case {
    name: "install-bios",
    about: "As install-uefi, under a BIOS: the live system installs Dukovo onto a blank disk from an answers file it downloads over HTTPS and checks against its SHA-256, the host finds the disk sound, and the BIOS then boots it alone with the GRUB the installer put on it",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: install,
    disks: super::install_uefi::INSTALL_DISKS,
    second: Some(SecondBoot {
        cmdline: "",
        before: super::install_uefi::nothing,
        script: super::install_uefi::installed,
    }),
};

/// The first boot, from the stick: install-uefi's, with the answers file
/// downloaded from the live system's web server and checked against the
/// SHA-256 the server printed.
fn install(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    live_shell(vm)?;
    let sha256 = serve(vm)?;
    install_by(vm, &format!("{ADDRESS}/answers.json"), Some(&sha256))
}
