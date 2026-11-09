// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's virtio sound card (ADR 0321) is the output */devices/audio/virtio0*,
//! root's alone, and *default* leads to it (ADR 0320). audiod gives it the
//! volume audio.toml keeps; *audio test* plays 440 Hz on the left and then
//! 660 Hz on the right, at full volume and at half, while QEMU records
//! what the card plays, and the host finds each tone on its own channel,
//! for as long as it was played, the second time an eighth as loud (the
//! cube of a half). The keyboard's volume keys change the volume and mute
//! it; a second program is refused the output while one plays, and
//! *speech* takes it from the one that plays.

use super::super::*;

pub const CASE: Case = Case {
    name: "audio-virtio",
    about: "a virtio sound card is /devices/audio/virtio0: audio test's tones come out on the right channels, as long and as loud as asked; the volume keys work; one player at a time, and speech takes the output",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // No sound reaches the host: the `none` backend plays to nothing, and
    // QEMU's wavcapture records what the card is given.
    qemu: &["-audiodev", "none,id=snd0", "-device", "virtio-sound-pci,audiodev=snd0"],
    script: audio_virtio,
    disks: &[],
    second: None,
};

/// Waits until the output plays, as `audio list` says, so that what
/// comes next finds the player at it however slowly the emulator starts
/// the program.
const PLAYING: &str = "until audio list | grep -q playing; do sleep 0.05; done";

/// The `audio-virtio` case.
fn audio_virtio(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere("Audio: virtio0 is a virtio sound card at ")?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("audiod: playing through virtio0 at 70%")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    // Root's alone, and the link.
    vm.type_text(
        "echo mode=$(stat -c %a /devices/audio/virtio0) link=$(readlink /devices/audio/default)\n",
    )?;
    vm.expect_screen(&["mode=600 link=virtio0"])?;
    vm.type_text("audio list\n")?;
    vm.expect_screen(&["* virtio0    virtio sound card     70%"])?;
    // The tones, recorded as the card plays them, at full volume and at
    // half.
    let recording = vm.dir().join("audio-virtio.wav");
    let _ = std::fs::remove_file(&recording);
    vm.monitor(&format!("wavcapture {} snd0 48000 16 2", recording.display()))?;
    vm.type_text(
        "audio volume 100 && audio test && audio volume 50 && audio test && echo tested-$((6 * 7))\n",
    )?;
    vm.expect_screen(&["tested-42"])?;
    vm.monitor("stopcapture 0")?;
    // Half the volume, the cube of a half: an eighth of the amplitude.
    check_tones(&recording, 0.10..=0.15, "an eighth")?;
    // The volume keys, through audiod.
    vm.press(&["volumeup"])?;
    vm.expect_anywhere("audiod: volume 55%")?;
    vm.press(&["audiomute"])?;
    vm.expect_anywhere("audiod: volume 55%, muted")?;
    vm.type_text("clear; audio volume; audio info | grep Volume\n")?;
    vm.expect_screen(&["55% (muted)", "Volume:     0% left, 0% right (scaled by the kernel)"])?;
    vm.type_text("audio mute off\n")?;
    vm.expect_screen(&["unmuted"])?;
    // One player at a time: a second is refused while the first plays.
    vm.type_text(&format!(
        "clear; audio test >/dev/null 2>&1 & p=$!; {PLAYING}; audio test; echo second=$?; wait $p; echo first=$?\n"
    ))?;
    vm.expect_screen(&["second=1", "first=0"])?;
    // The screen reader's file takes the output from the one that plays,
    // which is refused from then on.
    vm.type_text(&format!(
        "clear; audio test >/dev/null 2>&1 & p=$!; {PLAYING}; head -c 96000 /devices/zero > /devices/audio/speech; echo speech=$?; wait $p; echo pushed=$?\n"
    ))?;
    vm.expect_screen(&["speech=0", "pushed=1"])?;
    vm.type_text("audio info | grep Underruns\n")?;
    vm.expect_screen(&["Underruns:  "])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// A stretch of sound on one channel, in 10 ms blocks of the recording.
#[derive(Debug)]
struct Run {
    start: usize,
    end: usize,
}

/// The runs of sound on channel `which`: blocks louder than half a
/// percent of full scale, with gaps under 50 ms (an underrun under TCG)
/// counted in.
fn runs(samples: &[i16], which: usize, rate: usize) -> Vec<Run> {
    let channel = audio::detect::channel(samples, 2, which);
    let block = rate / 100;
    let loud: Vec<bool> = channel
        .chunks(block)
        .map(|chunk| audio::detect::rms(chunk) > 0.005 * 32768.0)
        .collect();
    let mut runs: Vec<Run> = Vec::new();
    for (i, &is_loud) in loud.iter().enumerate() {
        if !is_loud {
            continue;
        }
        match runs.last_mut() {
            Some(run) if i - run.end < 5 => run.end = i + 1,
            _ => runs.push(Run { start: i, end: i + 1 }),
        }
    }
    // Clicks are not tones.
    runs.retain(|run| run.end - run.start >= 10);
    runs
}

/// Checks the recording: left, right, left, right, each 0.6 s of its tone
/// with the other channel silent, the second pair as loud as `ratio` of
/// the first (which `expected` says in words). The HD Audio tests use it
/// too.
pub(super) fn check_tones(
    path: &Path,
    ratio: std::ops::RangeInclusive<f64>,
    expected: &str,
) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wav = audio::wav::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if (wav.rate, wav.channels) != (48000, 2) {
        return Err(format!("the recording is {} Hz, {} channels", wav.rate, wav.channels));
    }
    let mut samples = Vec::new();
    audio::convert::to_s16(wav.data, wav.encoding, &mut samples);
    let rate = 48000;
    let left = runs(&samples, 0, rate);
    let right = runs(&samples, 1, rate);
    if left.len() != 2 || right.len() != 2 {
        return Err(format!(
            "expected two tones on each channel, found {} on the left ({left:?}) and {} on the \
             right ({right:?}), in {} s recorded",
            left.len(),
            right.len(),
            samples.len() / 2 / rate
        ));
    }
    let order = [&left[0], &right[0], &left[1], &right[1]];
    if order.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(format!("the tones overlap or are out of order: {order:?}"));
    }
    let block = rate / 100;
    let mut loudness = [0.0f64; 4];
    for (i, run) in order.iter().enumerate() {
        let (channel, other, frequency) = if i.is_multiple_of(2) {
            (0, 1, audio::tone::LEFT_FREQUENCY)
        } else {
            (1, 0, audio::tone::RIGHT_FREQUENCY)
        };
        let ms = (run.end - run.start) * 10;
        if !(450..=800).contains(&ms) {
            return Err(format!("tone {} lasted {ms} ms, not about 600", i + 1));
        }
        // Away from the fades at its ends.
        let span = (run.start + 5) * block..(run.end - 5) * block;
        let this = &audio::detect::channel(&samples, 2, channel)[span.clone()];
        let that = &audio::detect::channel(&samples, 2, other)[span];
        let share = audio::detect::share(this, rate as u32, frequency);
        if share < 0.8 {
            return Err(format!("tone {} is only {:.0}% {frequency} Hz", i + 1, share * 100.0));
        }
        if audio::detect::rms(that) > audio::detect::rms(this) * 0.01 {
            return Err(format!("tone {} is heard on the other channel too", i + 1));
        }
        loudness[i] = audio::detect::rms(this);
    }
    for (i, found) in [loudness[2] / loudness[0], loudness[3] / loudness[1]].iter().enumerate() {
        if !ratio.contains(found) {
            return Err(format!(
                "at half volume the {} tone is {found:.3} as loud, not {expected}",
                if i == 0 { "left" } else { "right" }
            ));
        }
    }
    Ok(())
}
