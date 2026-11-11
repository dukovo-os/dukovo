// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Discs burned in a USB burner (ADR 0449): the emulated drive, which
//! QEMU's *usb-redir* plugs in, records the blank discs put in it as burn
//! writes them.
//!
//! The music CD of the audio-cd test is ripped to a FLAC file and a WAV
//! file, and burn writes the two onto a blank CD-R at once, from a cue
//! sheet: the disc holds the very samples ripped, the short second track
//! padded with silence to four seconds, and audiocd lists it. A folder of
//! files, a link and a long Unicode name is burned onto a CD-R a track at
//! a time, with Rock Ridge and Joliet; the kernel looks at the disc once
//! burned, volumed mounts it, and each file reads back as it was. mkiso
//! makes an image of the folder with UDF, and burn formats a blank DVD+RW
//! and writes the image onto it, which volumed mounts as UDF.

use std::time::{Duration, Instant};

use crate::disc_drive::DiscDrive;
use crate::disc_drive::disc::{Disc, Mode, RAW, Source};
use crate::disc_drive::record::Kind;

use super::super::*;
use super::audio_cd::{lab_disc, samples};

pub const CASE: Case = Case {
    name: "disc-burn",
    about: "burn writes a CD of audio at once from ripped FLAC and WAV files, a folder onto a CD-R a track at a time, and an image with UDF onto a DVD+RW it formats, each read back as it was written, and ejects a CD-R it burned",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-chardev", DISC_DRIVE_LINE, "-device", "usb-redir,chardev=discdrive,bus=xhci.0,port=2"],
    script: disc_burn,
    disks: &[],
    second: None,
};

/// The sectors of the blank discs: a few megabytes, enough for what is
/// burned, and quick to fill.
const CD_SECTORS: u32 = 3000;
const DVD_SECTORS: u32 = 4096;

/// The folder's name, typed as the bytes of its UTF-8, as the keyboard
/// types ASCII alone: "Música en vivo".
const FOLDER: &str = "d=$(printf 'M\\303\\272sica en vivo')";

/// The folder burned: a file of text, a file of noise, a link, and a
/// folder with a name too long and too wide for ISO 9660 alone.
fn make_folder() -> String {
    format!(
        "clear; {FOLDER}; mkdir -p \"/tmp/disc/$d\" && \
         i=0; while [ $i -lt 400 ]; do echo \"line $i of the lab's disc\"; i=$((i + 1)); done > \
         \"/tmp/disc/$d/a file with a long name of more than thirty-one characters.txt\" && \
         head -c 700000 /devices/urandom > /tmp/disc/noise.bin && \
         ln -s \"$d\" /tmp/disc/live && echo made-$((6 * 7))\n"
    )
}

/// Compares each file of the folder with the disc's at `volume`, and says
/// `same` with the name of each that is, and `link-same` when the link
/// points at the folder.
fn compare(volume: &str) -> String {
    format!(
        "clear; {FOLDER}; cd /tmp/disc; for f in noise.bin \"$d\"/*; do \
         [ \"$(sha256sum < \"$f\")\" = \"$(sha256sum < \"{volume}/$f\")\" ] && echo \"same ${{f##*/}}\"; done; \
         [ \"$(readlink \"{volume}/live\")\" = \"$d\" ] && echo link-same; cd\n"
    )
}

/// What the screen shows once the folder compares the same.
const SAME: [&str; 3] = [
    "same noise.bin",
    "same a file with a long name of more than thirty-one characters.txt",
    "link-same",
];

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

/// The sectors of a track the drive holds.
fn track_bytes(disc: &Disc, number: u8) -> Result<&[u8], String> {
    let track = disc.tracks.iter().find(|t| t.number == number && t.mode == Mode::Audio);
    let Some(track) = track else { return Err(format!("the burned CD has no track {number}")) };
    let Source::Bytes { bytes, .. } = &track.source;
    Ok(bytes)
}

/// Checks the CD of audio burned: track 1 is the ripped FLAC file's
/// samples, the disc's first five seconds; track 2 the WAV file's, its
/// two seconds of noise padded with silence to four seconds.
fn check_audio(drive: &DiscDrive) -> Result<(), String> {
    let disc = drive.disc().ok_or("the burner has no disc in it")?;
    let samples = samples();
    let one = track_bytes(&disc, 1)?;
    if one != &samples[..375 * RAW] {
        return Err(format!("track 1 is not the FLAC file's samples ({} bytes)", one.len()));
    }
    let two = track_bytes(&disc, 2)?;
    let mut padded = samples[375 * RAW..525 * RAW].to_vec();
    padded.resize(300 * RAW, 0);
    if two != padded {
        return Err(format!("track 2 is not the WAV file's samples, padded ({} bytes)", two.len()));
    }
    Ok(())
}

/// Checks the CD of data burned holds the folder's name as it is, in
/// UTF-8 for Rock Ridge and UTF-16 for Joliet: the keyboard typed it as
/// bytes, which are what the disc must hold.
fn check_names(drive: &DiscDrive) -> Result<(), String> {
    let disc = drive.disc().ok_or("the burner has no disc in it")?;
    let track = disc.tracks.iter().find(|t| t.mode == Mode::Data);
    let Some(track) = track else { return Err("the burned CD has no track of data".into()) };
    let Source::Bytes { bytes, .. } = &track.source;
    let name = "Música en vivo";
    let utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let holds = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    if !holds(name.as_bytes()) {
        return Err(format!("the burned CD has no Rock Ridge name \"{name}\""));
    }
    if !holds(&utf16) {
        return Err(format!("the burned CD has no Joliet name \"{name}\""));
    }
    Ok(())
}

/// The `disc-burn` case.
fn disc_burn(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let drive = DiscDrive::plug_in(&disc_drive_line(vm.dir(), case.name), Some(lab_disc()))
        .map_err(|err| format!("the emulated drive: {err}"))?;
    boot_report(vm, case)?;
    vm.expect_anywhere("Disk: cd1: a music CD went in; its tracks are not files, and it is not mounted")?;
    vm.expect("Boot complete.")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // Ripped: track 1 to FLAC, track 2 to WAV.
    vm.type_text(
        "clear; mkdir /tmp/rip && audiocd rip --to /tmp/rip 1 && audiocd rip --wav --to /tmp/rip 2 && echo ripped-$((6 * 7))\n",
    )?;
    expect_screen_for(vm, &["ripped-42"], 120)?;
    vm.type_text("clear; eject cd1; until [ \"$(cat /kernel/block/cd1/disc)\" = none ]; do sleep 0.2; done; echo out-$((6 * 7))\n")?;
    vm.expect_screen(&["out-42"])?;

    // A blank CD-R: burned with the two at once.
    drive.insert(Disc::blank(Kind::CdR, CD_SECTORS));
    vm.expect("Disk: cd1: a blank disc went in; it holds nothing to read")?;
    vm.type_text("clear; burn info\n")?;
    vm.expect_screen(&["cd1: CD-R, blank, 6.2MB free"])?;
    vm.type_text(
        "clear; burn --audio -q '/tmp/rip/01 First Light.flac' '/tmp/rip/02 Static.wav' && echo burned-$((6 * 7))\n",
    )?;
    expect_screen_for(vm, &["cd1: burned 825 sectors on the CD-R", "burned-42"], 120)?;
    vm.expect("Disk: cd1: a music CD went in; its tracks are not files, and it is not mounted")?;
    check_audio(&drive)?;
    vm.type_text("clear; audiocd\n")?;
    vm.expect_screen(&["cd1: a music CD", " 1   0:05  Track 1", " 2   0:04  Track 2", "2 tracks, 0:09"])?;

    // A folder, onto a CD-R a track at a time.
    vm.type_text(&make_folder())?;
    vm.expect_screen(&["made-42"])?;
    vm.type_text("clear; eject cd1; until [ \"$(cat /kernel/block/cd1/disc)\" = none ]; do sleep 0.2; done; echo out-$((6 * 8))\n")?;
    vm.expect_screen(&["out-48"])?;
    drive.insert(Disc::blank(Kind::CdR, CD_SECTORS));
    vm.expect("Disk: cd1: a blank disc went in; it holds nothing to read")?;
    vm.type_text("clear; burn -q --label 'Lab Disc' /tmp/disc && echo burned-$((6 * 8))\n")?;
    expect_screen_for(vm, &["burned-48"], 120)?;
    vm.expect("volumed: mounted the ISO 9660 filesystem of cd1 at /volumes/Lab Disc, read-only")?;
    check_names(&drive)?;
    vm.type_text(&compare("/volumes/Lab Disc"))?;
    vm.expect_screen(&SAME)?;

    // An image with UDF, onto a DVD+RW, formatted first.
    vm.type_text("clear; mkiso --udf --label LABDVD -o /tmp/lab.iso /tmp/disc && echo made-$((6 * 9))\n")?;
    vm.expect_screen(&["made-54"])?;
    vm.type_text("clear; eject cd1; until [ \"$(cat /kernel/block/cd1/disc)\" = none ]; do sleep 0.2; done; echo out-$((6 * 9))\n")?;
    vm.expect_screen(&["out-54"])?;
    drive.insert(Disc::blank(Kind::DvdPlusRw, DVD_SECTORS));
    vm.expect("Disk: cd1: a blank disc went in; it holds nothing to read")?;
    vm.type_text("clear; burn -q /tmp/lab.iso && burn info\n")?;
    expect_screen_for(vm, &["cd1: DVD+RW, formatted, 8.4MB, erasable"], 120)?;
    vm.expect("volumed: mounted the UDF filesystem of cd1 at /volumes/LABDVD, read-only")?;
    vm.type_text(&compare("/volumes/LABDVD"))?;
    vm.expect_screen(&SAME)?;

    // The image onto a CD-R, ejected once burned, before it is mounted.
    vm.type_text("clear; eject LABDVD; until [ \"$(cat /kernel/block/cd1/disc)\" = none ]; do sleep 0.2; done; echo out-$((6 * 10))\n")?;
    vm.expect_screen(&["out-60"])?;
    drive.insert(Disc::blank(Kind::CdR, CD_SECTORS));
    vm.expect("Disk: cd1: a blank disc went in; it holds nothing to read")?;
    vm.type_text("clear; burn -q --eject /tmp/lab.iso && until [ \"$(cat /kernel/block/cd1/disc)\" = none ]; do sleep 0.2; done; echo ejected-$((6 * 10))\n")?;
    expect_screen_for(vm, &["cd1: burned 774 sectors on the CD-R", "ejected-60"], 120)?;
    if drive.disc().is_some() {
        return Err("burn --eject left the disc in the drive".into());
    }

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;
    drive.unplug();
    Ok(())
}
