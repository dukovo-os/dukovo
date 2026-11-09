// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's ICH9 HD Audio controller, a PCI Express function, with a codec
//! that plays and records (hda-duplex), is */devices/audio/hda0* too
//! (ADR 0322): the driver passes over the codec's input, plays through
//! its line out, and audio test's tones come out as on the ICH6 (the
//! *audio-hda* test). After them, *speech* takes the output from a
//! program that plays, as on any output.

use super::super::*;

pub const CASE: Case = Case {
    name: "audio-hda-ich9",
    about: "an ICH9 HD Audio controller with a duplex codec is /devices/audio/hda0: its line out plays audio test's tones, and speech takes the output",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-audiodev",
        "none,id=snd0",
        "-device",
        "ich9-intel-hda",
        "-device",
        "hda-duplex,audiodev=snd0",
    ],
    script: audio_hda_ich9,
    disks: &[],
    second: None,
};

/// Waits until the output plays, as `audio list` says.
const PLAYING: &str = "until audio list | grep -q playing; do sleep 0.05; done";

/// The `audio-hda-ich9` case.
fn audio_hda_ich9(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere("Audio: hda0 is an HD Audio codec at ")?;
    vm.expect_anywhere("Audio: hda0 plays through the line out")?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("audiod: playing through hda0 at 70%")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("audio info | grep -e Channels -e Rates\n")?;
    vm.expect_screen(&["Channels:   2 to 2"])?;
    super::audio_hda::play_tones(vm, "audio-hda-ich9.wav")?;
    // A second program waits for the first's sound to play out, then
    // plays; speech takes the output from one that plays.
    vm.type_text(&format!(
        "clear; audio test >/dev/null 2>&1 & p=$!; {PLAYING}; head -c 96000 /devices/zero > /devices/audio/speech; echo speech=$?; wait $p; echo pushed=$?\n"
    ))?;
    vm.expect_screen(&["speech=0", "pushed=1"])?;
    vm.type_text("audio info | grep -e Underruns -e Played\n")?;
    vm.expect_screen(&["Underruns:  "])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
