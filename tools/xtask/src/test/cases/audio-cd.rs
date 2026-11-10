// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A music CD in a USB CD drive (ADR 0447): the emulated drive, which
//! QEMU's *usb-redir* plugs in, holds a disc of two audio tracks with
//! CD-Text. The kernel says a music CD went in and mounts nothing, and
//! */kernel/block/cd1/disc* says it holds audio. *audiocd* lists the
//! tracks by their CD-Text; rips them to FLAC files tagged from it, and
//! one to a WAV file, each the very bytes the host makes of the disc's
//! samples; and plays the first through the virtio sound card, which QEMU
//! records: 440 Hz on the left and 660 Hz on the right, for four seconds.
//! Ejected, the drive is empty and *audiocd* finds no music CD; the disc
//! put back is found again.

use std::time::{Duration, Instant};

use crate::disc_drive::DiscDrive;
use crate::disc_drive::disc::{Disc, Error, RAW};

use super::super::*;

pub const CASE: Case = Case {
    name: "audio-cd",
    about: "a music CD in a USB CD drive is said to be one and not mounted; audiocd lists its tracks by their CD-Text, rips them to FLAC and WAV bit for bit, and plays them; ejected, it is gone, and put back, found",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // No sound reaches the host: the `none` backend plays to nothing, and
    // QEMU's wavcapture records what the card is given.
    qemu: &[
        "-chardev",
        DISC_DRIVE_LINE,
        "-device",
        "usb-redir,chardev=discdrive,bus=xhci.0,port=2",
        "-audiodev",
        "none,id=snd0",
        "-device",
        "virtio-sound-pci,audiodev=snd0",
    ],
    script: audio_cd,
    disks: &[],
    second: None,
};

/// The disc: its cue sheet, of the file [`samples`] makes.
const SHEET: &str = r#"TITLE "Lab Sessions"
PERFORMER "Dukovo Lab"
FILE "lab.bin" BINARY
  TRACK 01 AUDIO
    TITLE "First Light"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Static"
    INDEX 00 00:04:00
    INDEX 01 00:05:00
"#;

/// The seconds of each part of the file: track 1's tones, track 2's
/// pregap of silence, and its noise.
const TONES_SECONDS: usize = 4;
const GAP_SECONDS: usize = 1;
const NOISE_SECONDS: usize = 2;

/// The samples of the disc's file: 440 Hz on the left and 660 Hz on the
/// right at half of full scale, a second of silence, then noise from a
/// fixed seed.
pub fn samples() -> Vec<u8> {
    let frames = |seconds: usize| seconds * 44_100;
    let mut bytes = Vec::with_capacity((TONES_SECONDS + GAP_SECONDS + NOISE_SECONDS) * 176_400);
    for n in 0..frames(TONES_SECONDS) {
        let t = n as f64 / 44_100.0;
        for hz in [440.0, 660.0] {
            let sample = (16_384.0 * (2.0 * std::f64::consts::PI * hz * t).sin()).round() as i16;
            bytes.extend(sample.to_le_bytes());
        }
    }
    bytes.resize(bytes.len() + frames(GAP_SECONDS) * 4, 0);
    let mut seed = 0x2545_f491_u32;
    for _ in 0..frames(NOISE_SECONDS) * 2 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bytes.extend(((seed >> 16) as i16 / 4).to_le_bytes());
    }
    debug_assert!(bytes.len().is_multiple_of(RAW));
    bytes
}

/// The test's disc.
pub fn lab_disc() -> Disc {
    let mut files = |name: &str| match name {
        "lab.bin" => Ok(samples()),
        _ => Err(Error(format!("no {name}"))),
    };
    Disc::cue(SHEET, &mut files).expect("the test's cue sheet")
}

/// The sectors of track 1, which ends where track 2's index 1 starts,
/// its pregap with it, and of track 2.
const TRACK_1: std::ops::Range<usize> = 0..375;
const TRACK_2: std::ops::Range<usize> = 375..525;

/// The tags audiocd gives track `number` of the disc, `title`.
fn tags(title: &'static str, number: &'static str) -> [(&'static str, &'static str); 6] {
    [
        ("TITLE", title),
        ("ARTIST", "Dukovo Lab"),
        ("ALBUM", "Lab Sessions"),
        ("ALBUMARTIST", "Dukovo Lab"),
        ("TRACKNUMBER", number),
        ("TRACKTOTAL", "2"),
    ]
}

/// The files a rip of the disc makes, by name: the tracks as FLAC, and
/// track 2 as WAV too. Each FLAC file decodes to its track's samples.
fn rips() -> Result<Vec<(&'static str, Vec<u8>)>, String> {
    let bytes = samples();
    let track = |sectors: std::ops::Range<usize>| &bytes[sectors.start * RAW..sectors.end * RAW];
    let mut files = Vec::new();
    for (name, sectors, title, number) in
        [("01 First Light.flac", TRACK_1, "First Light", "1"), ("02 Static.flac", TRACK_2, "Static", "2")]
    {
        let samples = audio::convert::from_bytes(track(sectors));
        let flac = audio::flac::encode(&samples, 44_100, 2, &tags(title, number));
        let decoded = audio::flac::decode(&flac).map_err(|err| format!("{name} on the host: {err}"))?;
        if decoded.samples != samples {
            return Err(format!("{name} on the host does not decode to its track"));
        }
        files.push((name, flac));
    }
    let wav_track = track(TRACK_2);
    let mut wav = audio::wav::header(44_100, 2, (wav_track.len() / 4) as u32).to_vec();
    wav.extend_from_slice(wav_track);
    files.push(("02 Static.wav", wav));
    Ok(files)
}

/// The first 20 hexadecimal digits of the SHA-256 of `bytes`, as the
/// host's sha256sum gives it, written to `dir` to be summed.
fn sum(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    let out = host_tool(std::process::Command::new("sha256sum").arg(&path))?;
    Ok(String::from_utf8_lossy(&out).chars().take(20).collect())
}

/// Waits for the screen to show `needles`, for up to `seconds`.
fn expect_screen_for(vm: &mut Vm, needles: &[&str], seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match vm.expect_screen(needles) {
            Err(_) if Instant::now() < deadline => {}
            done => return done,
        }
    }
}

/// Checks what QEMU recorded of `audiocd play 1`: 440 Hz on the left and
/// 660 Hz on the right, for track 1's four seconds of tones.
fn check_played(path: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wav = audio::wav::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut samples = Vec::new();
    audio::convert::to_s16(wav.data, wav.encoding, &mut samples);
    let rate = wav.rate;
    for (which, hz) in [(0, 440.0), (1, 660.0)] {
        let channel = audio::detect::channel(&samples, 2, which);
        let Some(sound) = audio::detect::sounding(&channel, rate, 0.005) else {
            return Err(format!("nothing was played on channel {which}"));
        };
        let seconds = sound.len() as f64 / f64::from(rate);
        if !(3.5..=4.6).contains(&seconds) {
            return Err(format!("channel {which} sounded for {seconds:.2} s, not about 4"));
        }
        // The middle two seconds, away from the ends.
        let middle = sound.start + sound.len() / 2;
        let span = middle - rate as usize..middle + rate as usize;
        let share = audio::detect::share(&channel[span], rate, hz);
        if share < 0.8 {
            return Err(format!("channel {which} is only {:.0}% {hz} Hz", share * 100.0));
        }
    }
    Ok(())
}

/// The `audio-cd` case.
fn audio_cd(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let drive = DiscDrive::plug_in(&disc_drive_line(vm.dir(), case.name), Some(lab_disc()))
        .map_err(|err| format!("the emulated drive: {err}"))?;
    let files = rips()?;
    boot_report(vm, case)?;
    vm.expect_anywhere("Disk: cd1 is a USB optical drive on 1-6: DUKOVO TEST DISC DRIVE 0001")?;
    vm.expect_anywhere("Disk: cd1: a music CD went in; its tracks are not files, and it is not mounted")?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("audiod: playing through virtio0 at 70%")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("echo disc=$(cat /kernel/block/cd1/disc)\n")?;
    vm.expect_screen(&["disc=cd-rom audio"])?;

    // The tracks, by their CD-Text.
    vm.type_text("clear; audiocd\n")?;
    vm.expect_screen(&[
        "cd1: Lab Sessions, by Dukovo Lab",
        " 1   0:05  First Light",
        " 2   0:02  Static",
        "2 tracks, 0:07",
    ])?;

    // Ripped, to FLAC and to WAV: the very bytes the host makes.
    vm.type_text(
        "clear; mkdir /tmp/rip && audiocd rip --to /tmp/rip && audiocd rip --wav --to /tmp/rip 2 && echo ripped-$((6 * 7))\n",
    )?;
    expect_screen_for(vm, &["ripped-42"], 120)?;
    vm.type_text("clear; cd /tmp/rip; for f in *; do echo \"$(sha256sum < \"$f\" | cut -c1-20) $f\"; done; cd\n")?;
    let mut sums = Vec::new();
    for (name, bytes) in &files {
        sums.push(format!("{} {name}", sum(vm.dir(), name, bytes)?));
    }
    let sums: Vec<&str> = sums.iter().map(String::as_str).collect();
    vm.expect_screen(&sums)?;

    // Played: the tones of track 1, as the card is given them.
    let recording = vm.dir().join("audio-cd.wav");
    let _ = std::fs::remove_file(&recording);
    vm.monitor(&format!("wavcapture {} snd0 48000 16 2", recording.display()))?;
    vm.type_text("clear; audiocd play 1 && echo played-$((7 * 7))\n")?;
    expect_screen_for(vm, &["played-49"], 60)?;
    vm.monitor("stopcapture 0")?;
    check_played(&recording)?;

    // Ejected: the drive is empty, and no drive has a music CD.
    vm.type_text(
        "clear; eject cd1; until [ \"$(cat /kernel/block/cd1/disc)\" = none ]; do sleep 0.2; done; audiocd; echo listed=$?\n",
    )?;
    vm.expect_screen(&["audiocd: no drive has a music CD in it", "listed=1"])?;
    // Put back, it is found again.
    drive.insert(lab_disc());
    vm.expect("Disk: cd1: a music CD went in; its tracks are not files, and it is not mounted")?;
    vm.type_text("clear; audiocd | tail -1\n")?;
    vm.expect_screen(&["2 tracks, 0:07"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    drive.unplug();
    Ok(())
}
