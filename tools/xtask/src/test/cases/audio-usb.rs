// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's USB speaker (`usb-audio`, USB Audio Class 1 at full speed) on
//! its xHCI controller is the output */devices/audio/usb0* (ADR 0323):
//! the kernel plays through its isochronous endpoint a packet a
//! millisecond and sets its feature unit's volume. *audio test* plays
//! 440 Hz on the left and then 660 Hz on the right, at full volume and at
//! half, while QEMU records what the speaker plays; the host finds each
//! tone on its own channel, for as long as it was played, the second time
//! as loud as the speaker's volume makes half: 18 dB down, which QEMU's
//! speaker turns into 206/239 of the amplitude. Unplugged while it plays,
//! the speaker takes its file away and the player fails; plugged in
//! again, it is *usb0* and the default once more, and plays.

use super::super::*;

pub const CASE: Case = Case {
    name: "audio-usb",
    about: "a USB speaker is /devices/audio/usb0: it plays audio test's tones on the right channels, as long as asked, at its own volume, and unplugged and plugged in again it is usb0 again",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // No sound reaches the host: the `none` backend plays to nothing, and
    // QEMU's wavcapture records what the speaker is given.
    qemu: &[
        "-audiodev",
        "none,id=snd0",
        "-device",
        "usb-audio,bus=xhci.0,audiodev=snd0,id=speaker0",
    ],
    script: audio_usb,
    disks: &[],
    second: None,
};

/// What the kernel says of QEMU's speaker, after its place on the bus.
const FOUND: &str = ", USB Audio Class 1: 2 channels of 16 bits, 48000 to 48000 frames a second \
                     on the bus's clock, volume on the device";

/// Waits until the output plays, as `audio list` says.
const PLAYING: &str = "until audio list | grep -q playing; do sleep 0.05; done";

/// The `audio-usb` case.
fn audio_usb(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere("Audio: usb0 is a USB speaker at 1-")?;
    vm.expect_anywhere(FOUND)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("audiod: playing through usb0 at 70%")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("echo link=$(readlink /devices/audio/default)\n")?;
    vm.expect_screen(&["link=usb0"])?;
    vm.type_text("audio list\n")?;
    vm.expect_screen(&["* usb0       USB speaker           70%"])?;
    // The speaker's volume, not the kernel's scaling.
    vm.type_text("echo scaled=$(audio info | grep -c scaled)\n")?;
    vm.expect_screen(&["scaled=0"])?;
    vm.type_text("echo drivers=$(lsusb -t | grep -c Driver=snd-usb-audio)\n")?;
    vm.expect_screen(&["drivers=2"])?;

    let recording = vm.dir().join("audio-usb.wav");
    let _ = std::fs::remove_file(&recording);
    vm.monitor(&format!("wavcapture {} snd0 48000 16 2", recording.display()))?;
    vm.type_text(
        "clear; audio volume 100 && audio test && audio volume 50 && audio test && echo tested-$((6 * 7))\n",
    )?;
    vm.expect_screen(&["tested-42"])?;
    vm.monitor("stopcapture 0")?;
    super::audio_virtio::check_tones(&recording, 0.80..=0.92, "206/239")?;
    vm.type_text("audio volume 70 && audio info | grep -e Underruns -e Played\n")?;
    vm.expect_screen(&["Underruns:  "])?;

    // Unplugged while a test tone plays: the player fails, and the file
    // and the default link go.
    vm.type_text(&format!(
        "clear; audio test >/dev/null 2>&1 & p=$!; {PLAYING}; echo playing-$((7 * 7))\n"
    ))?;
    vm.expect_screen(&["playing-49"])?;
    vm.monitor("device_del speaker0")?;
    vm.expect("QEMU USB Audio was unplugged")?;
    vm.type_text("wait $p; echo test=$?; echo left: $(ls /devices/audio)\n")?;
    vm.expect_screen(&["test=1", "left: speech"])?;

    // Plugged in again: usb0 once more, which audiod plays through.
    vm.monitor("device_add usb-audio,bus=xhci.0,audiodev=snd0,id=speaker1")?;
    vm.expect("Audio: usb0 is a USB speaker at 1-")?;
    vm.expect("audiod: playing through usb0 at 70%")?;
    vm.type_text("clear; audio test && echo again-$((8 * 8)); audio list\n")?;
    vm.expect_screen(&["again-64", "* usb0       USB speaker           70%"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
