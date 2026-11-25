// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The beat of a pipe (ADR 0703), on the firmware's card and on a virtio
//! one, neither of which has a beat of its own: the kernel's clock gives
//! each its mode's rate. display-test (userspace/programs/display-test)
//! asks for forty beats in a row of each pipe: they come a whole number
//! of beats apart to the nanosecond, each with its number, and mostly
//! none is missed. A change that does not wait shows at a later beat
//! than the one it was asked after; one that waits comes back at the
//! next beat, and one that asks to show at once comes back at once.
//! While a change waits, another is refused as busy, and one that says
//! the newest wins takes its place: the first ends as replaced. With
//! nothing to show and nobody asking, no card's thread wakes for two
//! seconds. Then a pointer on each screen: the firmware's card draws it
//! into its frame and puts back what was under it when it moves, which
//! the host sees in its pictures of the screen; the virtio card's is the
//! host's to draw.

use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "display-beat",
    about: "the beat of the firmware's card and of a virtio one: beats a whole number apart to the nanosecond, a change shows at a later beat, waits for it unless asked to show at once, is busy while one waits unless the newest wins; a still screen wakes nobody; the firmware card's pointer is in its frame (ADR 0703)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "-- --posix /system/tests/posix/display-test beat",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-device", "virtio-gpu-pci,id=gpu0,xres=1024,yres=768"],
    script: display_beat,
    disks: &[],
    second: None,
};

fn display_beat(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let mut watch = screens::Watch::default();
    watch.steps(vm, &["shown", "beats", "pointer", "moved"])?;
    screens::passed(vm)?;
    let log = vm.log();
    for screen in ["firmware0/0", "virtio0/0"] {
        if !log.contains(&format!("display-test: the beat of {screen} is steady")) {
            return Err(format!("display-test did not check the beat of {screen}"));
        }
    }
    Ok(())
}
