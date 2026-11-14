// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Repair startup (ADR 0503): the install stick's menu entry for a
//! computer whose installed Dukovo no longer starts, as when its EFI
//! system partition lost GRUB or its menu, or the firmware lost its boot
//! entry. For each Dukovo root on the computer's disks it puts GRUB back
//! on the disk, for UEFI and, where the disk has a BIOS boot partition,
//! for a BIOS; writes the start menu again from the slot that starts,
//! with Rescue's entry and GRUB's environment, and the serial console's
//! settings unless the partition has them (ADR 0481); keeps the slot state when
//! it is whole and names a slot that is there, and otherwise has the
//! slot that is there start; sets the count of starts not seen through
//! back to none; and makes Dukovo's firmware boot entry first again
//! where the firmware has one. Nothing in the root changes.
//!
//! An EFI system partition whose FAT cannot be read is made again, as
//! FAT32, and filled as an install fills it, with Rescue's files from the
//! stick's release too; that erases it, so it is done only once the
//! person has typed the disk's name. It is the partition with the EFI
//! system partition's type on the disk that holds the Dukovo root, and
//! no other. Where Dukovo is found nowhere, or Secure Boot is on, it says
//! which firmware setting to change.
//!
//! When it misbehaves, read `/tmp/install/repair-startup.log`, which has
//! every command it ran; the `repair-startup` boot test breaks a
//! partition and mends it.

use std::fs;
use std::io::BufRead as _;
use std::path::Path;

use slots::state::{self, Slot, State};

use crate::disks;
use crate::install::{self, BootDisk};
use crate::log::Log;
use crate::machine::{self, SataMode};

/// Where the kernel lists disks and partitions.
const BLOCK: &str = "/kernel/block";

/// The GPT types it looks for, as the kernel writes them.
const EFI_SYSTEM: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const BIOS_BOOT: &str = "21686148-6449-6e6f-744e-656564454649";
const DUKOVO_ROOT: &str = "90c9a303-3954-4174-8f8b-ee75a92a9eb0";

/// Where it keeps what it ran, on the stick's memory.
const LOG: &str = "/tmp/install/repair-startup.log";

/// A Dukovo found on a disk: the disk with its EFI system partition, and
/// its root partition.
#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    /// The disk.
    pub disk: String,
    /// Its EFI system partition.
    pub efi: String,
    /// Whether it has a BIOS boot partition, for GRUB under a BIOS.
    pub bios_boot: bool,
    /// The root's partition.
    pub root: String,
}

/// The Dukovo roots among `names`, every disk and partition, each with
/// its disk's EFI system partition, by each partition's GPT type as
/// `kind` gives it; the disk `stick` is left out.
pub fn find(
    names: &[String],
    kind: impl Fn(&str) -> Option<String>,
    stick: Option<&str>,
) -> Vec<Found> {
    let typed = |name: &str, wanted: &str| {
        kind(name).is_some_and(|kind| kind.trim().eq_ignore_ascii_case(wanted))
    };
    let on = |disk: String| {
        names.iter().filter(move |name| disks::disk_of(name) == disk && **name != disk)
    };
    names
        .iter()
        .filter(|name| typed(name, DUKOVO_ROOT))
        .filter_map(|root| {
            let disk = disks::disk_of(root);
            if Some(disk.as_str()) == stick {
                return None;
            }
            let efi = on(disk.clone()).find(|name| typed(name, EFI_SYSTEM))?.clone();
            let bios_boot = on(disk.clone()).any(|name| typed(name, BIOS_BOOT));
            Some(Found { disk, efi, bios_boot, root: root.clone() })
        })
        .collect()
}

/// The partition number of `partition` on `disk`: `sda1` and `nvme0n1p1`
/// are 1.
#[must_use]
pub fn number(disk: &str, partition: &str) -> Option<u32> {
    let rest = partition.strip_prefix(disk)?;
    rest.strip_prefix('p').unwrap_or(rest).parse().ok()
}

/// The state to keep: `found`, the newest whole copy, when the slot it
/// starts by default is `present`; otherwise slot A's, or slot B's,
/// whichever is there, as the only one. `None` when neither is there.
#[must_use]
pub fn state_to_keep(found: Option<State>, present: impl Fn(Slot) -> bool) -> Option<State> {
    if let Some(found) = found.filter(|found| present(found.default)) {
        return Some(found);
    }
    [Slot::A, Slot::B]
        .into_iter()
        .find(|&slot| present(slot))
        .map(|slot| State { default: slot, ..State::installed() })
}

/// Runs Repair startup, from the stick's menu: repairs each Dukovo found,
/// then waits for the stick to be pulled and restarts.
#[must_use]
pub fn run() -> u8 {
    println!("Repair startup");
    println!("Looking for Dukovo on this computer's disks...");
    let stick = disks::live_medium().ok().map(|medium| medium.disk);
    let mut names: Vec<String> = fs::read_dir(BLOCK)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    let kind = |name: &str| fs::read_to_string(Path::new(BLOCK).join(name).join("parttype")).ok();
    let found = find(&names, kind, stick.as_deref());
    let status = if found.is_empty() {
        println!("{}", nothing_found(machine::sata_mode()));
        1
    } else {
        repair_all(&found)
    };
    if machine::secure_boot() {
        println!("{SECURE_BOOT_ON}");
    }
    println!("Remove the stick, then press Enter to restart.");
    let _ = std::io::stdin().lock().read_line(&mut String::new());
    let _ = std::process::Command::new("/system/commands/reboot").status();
    status
}

/// What it says when it finds no Dukovo: a SATA controller in RAID or IDE
/// mode, when the computer has one and no controller in AHCI mode, hides
/// the disks from every system but the one the vendor set up, so the
/// firmware's SATA mode is the setting to change.
fn nothing_found(mode: Option<SataMode>) -> String {
    let advice = match mode {
        Some(mode) => format!(
            "The computer's SATA controller is in {} mode, which can hide its disks from every \
             system but Windows. In the firmware's settings, set the SATA mode, also called SATA \
             Operation or Storage Configuration, to AHCI, and try again.",
            mode.name()
        ),
        None => "If one is installed, check that its disk is connected; and if the firmware has a \
                 SATA mode setting, set it to AHCI, not RAID, as a controller in RAID mode hides \
                 the disks from every system but Windows."
            .to_string(),
    };
    format!("No installed Dukovo was found on this computer's disks. {advice}")
}

/// What it says when Secure Boot is on: the firmware then refuses to
/// start what is not signed for it, and Dukovo's startup files are not,
/// before 1.0 (the handbook's chapter on supported hardware).
const SECURE_BOOT_ON: &str = "Secure Boot is on in the firmware, and Dukovo's startup files are \
    not signed for it before 1.0, so the firmware may refuse to start them. In the firmware's \
    settings, under Security or Boot, turn Secure Boot off.";

/// Whether `typed` is the name of `found`'s disk, which is what the person
/// must type before its EFI system partition is made again.
fn names_the_disk(found: &Found, typed: &str) -> bool {
    typed.trim() == found.disk
}

/// What it says before it makes `found`'s EFI system partition again, in
/// lines that fit an 80-column console.
fn question(found: &Found) -> String {
    format!(
        "The EFI system partition {efi} of {disk} cannot be read.\n\
         Dukovo can make it again, as FAT32, and put back GRUB, its start menu and\n\
         Rescue's files from this stick. That erases everything on {efi},\n\
         including the startup files of any other system that shares it, such as\n\
         Windows'. The rest of {disk} stays as it is, and so does Dukovo's root.\n\
         To make {efi} again, type the disk's name, {disk}, and press Enter.\n\
         To leave it as it is, press Enter alone: ",
        efi = found.efi,
        disk = found.disk
    )
}

/// Asks on the console whether to make `found`'s EFI system partition
/// again, and is answered by a line of its input.
fn confirmed(found: &Found) -> bool {
    use std::io::Write as _;
    print!("{}", question(found));
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    std::io::stdin().lock().read_line(&mut typed).is_ok() && names_the_disk(found, &typed)
}

/// Repairs each of `found`, saying what it did; 0 if each was repaired.
fn repair_all(found: &[Found]) -> u8 {
    let _ = fs::create_dir_all(install::WORK);
    let mut log = match Log::create(Path::new(LOG), false) {
        Ok(log) => log,
        Err(why) => {
            println!("Repair startup: {why}");
            return 1;
        }
    };
    let mut status = 0;
    for one in found {
        match repair(one, &mut log) {
            Ok(said) => said.iter().for_each(|line| println!("{line}")),
            Err(why) => {
                println!("Repair startup could not repair {}: {why}", one.disk);
                status = 1;
            }
        }
    }
    if status == 0 {
        println!("Startup is repaired.");
    } else {
        println!("What it ran is in {LOG}.");
    }
    status
}

/// Repairs the startup of `found`; what it did, in words.
fn repair(found: &Found, log: &mut Log) -> Result<Vec<String>, String> {
    let uuid = install::btrfs_uuid(&found.root)?;
    let root = Path::new(install::WORK).join("root");
    install::mount(&found.root, &root, &["-t", "btrfs", "-o", "ro"], log)?;
    let repaired = repair_mounted(found, &root, &uuid, log);
    let unmounted = install::run_when_free(log, "umount", &[&install::path(&root)]);
    let said = repaired?;
    unmounted?;
    Ok(said)
}

/// What a repair finds on an EFI system partition.
enum Esp {
    /// It reads: the slot state it holds, if a copy is whole, and whether
    /// its menu keeps a serial console.
    Reads { state: Option<State>, serial: bool },
    /// Its FAT cannot be read.
    Unreadable,
}

/// What the EFI system partition `efi` holds that a repair keeps, or that
/// it cannot be read.
fn on_partition(efi: &str, log: &mut Log) -> Esp {
    let esp = Path::new(install::WORK).join("esp");
    if install::mount(efi, &esp, &["-t", "fat"], log).is_err() {
        return Esp::Unreadable;
    }
    let copy = |name: &str| fs::read(esp.join(name)).ok().and_then(|block| State::decode(&block));
    let found = state::newest(copy(state::COPIES[0]), copy(state::COPIES[1]));
    let serial = fs::read_to_string(esp.join("boot/grub/grub.cfg"))
        .is_ok_and(|menu| menu.contains("console=ttyS"));
    let _ = install::run_when_free(log, "umount", &[&install::path(&esp)]);
    Esp::Reads { state: found.map(|(state, _)| state), serial }
}

/// Makes `found`'s EFI system partition again, as FAT32 labelled as an
/// install labels it, empty: [`install::grub_on`] and the Rescue step
/// fill it as an install does.
fn make_again(found: &Found, log: &mut Log) -> Result<(), String> {
    let device = install::path(&disks::device(&found.efi));
    log.run("mkfs-fat", &["-F", "32", "-n", install::ESP_LABEL, &device]).map(drop)
}

/// Puts Rescue's files of the stick's release on `found`'s EFI system
/// partition, which was made again, and says what came of it. The system
/// starts without Rescue if the stick's release cannot be read or does
/// not fit, as after an install whose partition is too small.
fn rescue_back(found: &Found, log: &mut Log) -> String {
    let put = install::find_release(None, log).and_then(|source| {
        log.installing(&found.disk, &source.release.version);
        let disk = BootDisk {
            disk: found.disk.clone(),
            efi: found.efi.clone(),
            bios_boot: found.bios_boot,
        };
        install::install_rescue(&[disk], &source, log).map(|()| source.release.version)
    });
    match put {
        Ok(version) => format!("Rescue of Dukovo {version} is on {} again.", found.efi),
        Err(why) => format!(
            "Rescue's files could not be put back on {}: {why}. The system starts without \
             Rescue until an update or an install puts it back.",
            found.efi
        ),
    }
}

/// Repairs the startup of `found`, its root mounted at `root`.
fn repair_mounted(
    found: &Found,
    root: &Path,
    uuid: &str,
    log: &mut Log,
) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    let present =
        |slot: Slot| root.join(slot.subvolume()).join("boot/grub/installed.cfg").is_file();
    let (found_state, serial, remade) = match on_partition(&found.efi, log) {
        Esp::Reads { state, serial } => (state, serial, false),
        Esp::Unreadable => {
            if !confirmed(found) {
                return Err(format!(
                    "its EFI system partition {} cannot be read, and was left as it is; run \
                     Repair startup again and type the disk's name to have it made again",
                    found.efi
                ));
            }
            make_again(found, log)?;
            said.push(format!("{} could not be read: it is made again, as FAT32.", found.efi));
            (None, false, true)
        }
    };
    let Some(kept) = state_to_keep(found_state, present) else {
        return Err(format!(
            "the root on {} holds no system in either slot: Rescue, or an install, puts one back",
            found.root
        ));
    };
    if found_state != Some(kept) {
        said.push(format!(
            "The slot state was lost, or named a slot that is gone: slot {} starts from now on.",
            kept.default.letter().to_uppercase()
        ));
    }
    let slot = root.join(kept.default.subvolume());
    let template = slot.join("boot/grub/installed.cfg");
    let menu =
        fs::read_to_string(&template).map_err(|err| format!("{}: {err}", template.display()))?;
    let console = serial.then(machine::serial_console_flag);
    let menu = install::fill_menu(&menu, uuid, console.as_deref());
    let disk =
        BootDisk { disk: found.disk.clone(), efi: found.efi.clone(), bios_boot: found.bios_boot };
    // A system that has serial console settings keeps them.
    install::grub_on(&disk, &slot, &menu, &install::serial_arguments(serial), true, log)?;
    // grub_on leaves an install's state, slot A's; the one kept goes back.
    if kept != State::installed() {
        let esp = Path::new(install::WORK).join("esp");
        install::mount(&found.efi, &esp, &["-t", "fat"], log)?;
        let block = kept.encode();
        let written = state::COPIES.iter().try_for_each(|copy| {
            fs::write(esp.join(copy), block).map_err(|err| format!("{copy}: {err}"))
        });
        let unmounted = install::run_when_free(log, "umount", &[&install::path(&esp)]);
        written.and(unmounted)?;
    }
    if remade {
        said.push(rescue_back(found, log));
    }
    let bios = if found.bios_boot { " and for a BIOS" } else { "" };
    said.push(format!(
        "{}: GRUB is back on the disk for UEFI{bios}, with its start menu for the root {uuid}, \
         slot {} by default, and no starts counted as not seen through.",
        found.disk,
        kept.default.letter().to_uppercase()
    ));
    if machine::boot_entries() {
        let listed = log.run("efiboot", &[])?;
        let numbered = number(&found.disk, &found.efi);
        match numbered {
            Some(at) if !install::earlier_entries(&listed).is_empty() => {
                install::boot_entry(&found.disk, at, log)?;
                said.push(
                    "Dukovo's boot entry is first in the firmware's boot order again.".into(),
                );
            }
            _ => said.push(
                "The firmware starts the disk by its usual path, with no entry of Dukovo's to \
                 put first."
                    .into(),
            ),
        }
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dukovo_is_found_on_the_disks_but_not_the_stick() {
        let names: Vec<String> =
            ["sda", "sda1", "sda2", "sda3", "sda4", "sdb", "sdb1", "sdb2", "nvme0n1", "nvme0n1p1"]
                .iter()
                .map(|name| (*name).to_string())
                .collect();
        let kind = |name: &str| {
            Some(match name {
                "sda1" => BIOS_BOOT.to_uppercase(),
                "sda2" | "sdb1" => EFI_SYSTEM.into(),
                "sda4" | "sdb2" => format!("{DUKOVO_ROOT}\n"),
                _ => "0fc63daf-8483-4772-8e79-3d69d8477de4".into(),
            })
        };
        let found = find(&names, kind, Some("sdb"));
        assert_eq!(
            found,
            vec![Found {
                disk: "sda".into(),
                efi: "sda2".into(),
                bios_boot: true,
                root: "sda4".into()
            }]
        );
        assert_eq!(number("sda", "sda2"), Some(2));
        assert_eq!(number("nvme0n1", "nvme0n1p1"), Some(1));
        assert_eq!(number("sda", "sdb1"), None);
    }

    #[test]
    fn a_partition_is_made_again_only_once_the_disks_name_is_typed() {
        let found =
            Found { disk: "sda".into(), efi: "sda2".into(), bios_boot: true, root: "sda4".into() };
        assert!(names_the_disk(&found, "sda\n"));
        assert!(names_the_disk(&found, "  sda \r\n"));
        // Nothing typed, the partition's name, another disk's and a yes
        // all leave it as it is.
        for typed in ["\n", "", "sda2\n", "sdb\n", "yes\n", "SDA\n", "y\n"] {
            assert!(!names_the_disk(&found, typed), "{typed:?}");
        }
    }

    #[test]
    fn the_question_names_the_partition_and_what_it_erases() {
        let found = Found {
            disk: "nvme0n1".into(),
            efi: "nvme0n1p1".into(),
            bios_boot: false,
            root: "nvme0n1p3".into(),
        };
        let asked = question(&found);
        assert!(asked.contains("The EFI system partition nvme0n1p1 of nvme0n1 cannot be read"));
        assert!(asked.contains("erases everything on nvme0n1p1"));
        assert!(asked.contains("type the disk's name, nvme0n1, and press Enter"));
        assert!(asked.ends_with("press Enter alone: "));
        // Each line fits a console of 80 columns, with the longest names.
        assert!(asked.lines().all(|line| line.len() <= 80), "{asked}");
    }

    #[test]
    fn a_disk_that_is_not_found_says_which_firmware_setting_hides_it() {
        for mode in [SataMode::Raid, SataMode::Ide] {
            let name = mode.name();
            let said = nothing_found(Some(mode));
            assert!(said.contains(&format!("SATA controller is in {name} mode")), "{said}");
            assert!(said.contains("set the SATA mode"), "{said}");
            assert!(said.contains("to AHCI"), "{said}");
        }
        let unknown = nothing_found(None);
        assert!(unknown.starts_with("No installed Dukovo was found"), "{unknown}");
        assert!(unknown.contains("set it to AHCI, not RAID"), "{unknown}");
        assert!(SECURE_BOOT_ON.contains("turn Secure Boot off"));
    }

    #[test]
    fn the_state_is_kept_while_its_slot_is_there() {
        let b = State { default: Slot::B, previous: Some(Slot::A), ..State::installed() };
        assert_eq!(state_to_keep(Some(b), |_| true), Some(b));
        let only_a = state_to_keep(Some(b), |slot| slot == Slot::A).unwrap();
        assert_eq!((only_a.default, only_a.previous), (Slot::A, None));
        let lost = state_to_keep(None, |slot| slot == Slot::B).unwrap();
        assert_eq!(lost.default, Slot::B);
        assert_eq!(state_to_keep(None, |_| false), None);
    }
}
