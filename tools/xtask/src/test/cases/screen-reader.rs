// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screen reader (ADR 0324), as the install stick's *Install with
//! speech* entry starts it: the kernel flag *speech* starts readerd
//! whatever reader.toml says, and consoled tells it what to say. With
//! *reader-log*, readerd logs each thing it says, which the case follows:
//! the console when readerd starts listening, a command's output, the
//! reader keys (Caps Lock held with Up, T and Page Up), `reader say`,
//! and consoled's Ctrl+Alt+Delete menu, said as trusted. QEMU records
//! what the virtio sound card plays meanwhile, and the host finds the
//! trusted sound's two rising tones, 660 Hz then 990 Hz, and speech
//! after them.

use super::super::*;

pub const CASE: Case = Case {
    name: "screen-reader",
    about: "the kernel flag speech starts readerd; consoled has it say the console, output, the reader keys' review and its own menu after the trusted tones, which the virtio sound card plays",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "speech reader-log console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // QEMU's wavcapture records what the card is given; the `none`
    // backend plays it to nothing.
    qemu: &["-audiodev", "none,id=snd0", "-device", "virtio-sound-pci,audiodev=snd0"],
    script: screen_reader,
    disks: &[],
    second: None,
};

/// The `screen-reader` case.
fn screen_reader(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere(
        "readerd: speaking with eSpeak NG's en voice at 175 words a minute, for the kernel flag speech",
    )?;
    vm.expect_anywhere("consoled: the screen reader is listening")?;
    // The whole screen, when readerd starts listening: the console first.
    vm.expect_anywhere("readerd: says: Console 1")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    // A command's output, a line at a time.
    vm.type_text("echo said-$((6 * 7))\n")?;
    vm.expect("readerd: says: said-42")?;
    // The reader keys: the line above the prompt, which console this is,
    // and faster, which readerd keeps. No program sees them: the shell
    // shows nothing for them.
    vm.press(&["caps_lock-up"])?;
    vm.expect("readerd: says: said-42")?;
    vm.press(&["caps_lock-t"])?;
    vm.expect("readerd: says: Console 1")?;
    vm.press(&["caps_lock-pgup"])?;
    vm.expect("readerd: says: rate 200")?;
    vm.type_text("clear; reader status\n")?;
    vm.expect_screen(&[
        "Screen reader: off",
        "Rate:          200 words a minute",
        "readerd:       running (voice en rate 200 pitch 50 output virtio0)",
    ])?;
    vm.type_text("reader rate 175 && reader say from-$((6 * 7))\n")?;
    vm.expect("readerd: says: from-42")?;
    // consoled's own menu, after the trusted sound, recorded as the card
    // plays it.
    let recording = vm.dir().join("screen-reader.wav");
    let _ = std::fs::remove_file(&recording);
    vm.monitor(&format!("wavcapture {} snd0 48000 16 2", recording.display()))?;
    vm.press(&["ctrl-alt-delete"])?;
    vm.expect(
        "readerd: says (trusted): Ctrl+Alt+Delete. R: restart. P: power off. Esc: back",
    )?;
    // Long enough for the tones and the menu's first words.
    std::thread::sleep(Duration::from_secs(3));
    vm.monitor("stopcapture 0")?;
    vm.press(&["esc"])?;
    check_trusted(&recording)?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Finds the trusted sound in the recording at `path`: a stretch of
/// 660 Hz, then one of 990 Hz, then speech, which is neither.
fn check_trusted(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wav = audio::wav::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if (wav.rate, wav.channels) != (48000, 2) {
        return Err(format!("the recording is {} Hz, {} channels", wav.rate, wav.channels));
    }
    let mut samples = Vec::new();
    audio::convert::to_s16(wav.data, wav.encoding, &mut samples);
    let left = audio::detect::channel(&samples, 2, 0);
    // 20 ms blocks: each tone lasts 70 ms, so at least two are its alone.
    let block = 960;
    let blocks: Vec<&[f64]> = left.chunks_exact(block).collect();
    let loud = |b: &[f64]| audio::detect::rms(b) > 0.005 * 32768.0;
    let tone = |b: &[f64], f: f64| loud(b) && audio::detect::share(b, 48000, f) > 0.6;
    let low = blocks.iter().position(|b| tone(b, 660.0)).ok_or_else(|| {
        format!("no 660 Hz tone in the {} ms recorded", blocks.len() * 20)
    })?;
    let high = blocks[low..]
        .iter()
        .position(|b| tone(b, 990.0))
        .map(|i| low + i)
        .ok_or("no 990 Hz tone after the 660 Hz one")?;
    if high - low > 10 {
        return Err(format!("the tones are {} ms apart", (high - low) * 20));
    }
    // Speech after the tones: a quarter of a second of loud blocks that
    // are no tone.
    let speech = blocks[high + 1..]
        .iter()
        .filter(|b| loud(b) && !tone(b, 660.0) && !tone(b, 990.0))
        .count();
    if speech < 12 {
        return Err(format!("only {} ms of speech after the tones", speech * 20));
    }
    Ok(())
}
