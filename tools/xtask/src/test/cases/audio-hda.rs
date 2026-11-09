// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's Intel HD Audio controller (ICH6) and its output codec are the
//! output */devices/audio/hda0* (ADR 0322): the driver finds the codec's
//! line out and the converter behind it, and *default* leads to it.
//! *audio test* plays 440 Hz on the left and then 660 Hz on the right, at
//! full volume and at half, while QEMU records what the codec plays; the
//! host finds each tone on its own channel, for as long as it was played,
//! the second time as loud as the codec's own amplifier makes half: 18 of
//! its 74 steps of 1 dB down, which QEMU's codec turns into 56/74 of the
//! amplitude. The volume is the codec's, not the kernel's scaling.

use super::super::*;

pub const CASE: Case = Case {
    name: "audio-hda",
    about: "an Intel HD Audio controller's codec is /devices/audio/hda0: its line out plays audio test's tones on the right channels, as long as asked, and its own amplifier sets the volume",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // No sound reaches the host: the `none` backend plays to nothing, and
    // QEMU's wavcapture records what the codec is given.
    qemu: &[
        "-audiodev",
        "none,id=snd0",
        "-device",
        "intel-hda",
        "-device",
        "hda-output,audiodev=snd0",
    ],
    script: audio_hda,
    disks: &[],
    second: None,
};

/// The `audio-hda` case.
fn audio_hda(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere("Audio: hda0 is an HD Audio codec at ")?;
    vm.expect_anywhere("Audio: hda0 plays through the line out")?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("audiod: playing through hda0 at 70%")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("echo link=$(readlink /devices/audio/default)\n")?;
    vm.expect_screen(&["link=hda0"])?;
    vm.type_text("audio list\n")?;
    vm.expect_screen(&["* hda0       QEMU HD Audio         70%"])?;
    // The codec's amplifier, not the kernel's scaling.
    vm.type_text("echo scaled=$(audio info | grep -c scaled)\n")?;
    vm.expect_screen(&["scaled=0"])?;
    play_tones(vm, "audio-hda.wav")?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Plays audio test's tones at full volume and at half while QEMU records
/// them in `file`, and checks them: the second pair as loud as QEMU's
/// codec plays the step for half, 56/74 of the first. The ICH9 test plays
/// them too.
pub(super) fn play_tones(vm: &mut Vm, file: &str) -> Result<(), String> {
    let recording = vm.dir().join(file);
    let _ = std::fs::remove_file(&recording);
    vm.monitor(&format!("wavcapture {} snd0 48000 16 2", recording.display()))?;
    vm.type_text(
        "audio volume 100 && audio test && audio volume 50 && audio test && echo tested-$((6 * 7))\n",
    )?;
    vm.expect_screen(&["tested-42"])?;
    vm.monitor("stopcapture 0")?;
    super::audio_virtio::check_tones(&recording, 0.70..=0.81, "56/74")
}
