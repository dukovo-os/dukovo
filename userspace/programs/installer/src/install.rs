// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install itself, step by step, as the approved design lists the
//! steps (ADR 0309): check the release's signatures, partition the disk,
//! make its filesystems and subvolumes, copy the system into its slot,
//! checking every file, write the settings and accounts, put GRUB on the
//! disk with the menu that starts the slot (ADR 0307), and save the
//! answers and the install log in the new system. Each step runs the
//! system's own tools (rdisk, mkfs-fat, mkswap, mkfs-btrfs, mount,
//! subvolume and rgrub-setup), so the log shows each command as it ran.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use system_release::list::Release;

use crate::answers::Answers;
use crate::copy;
use crate::disks;
use crate::log::{Log, Step};
use crate::settings::{self, Target};

/// The root a system trusts releases by (system-release(8)).
pub const TRUSTED_ROOT: &str = "/system/resources/updates/root.json";

/// Where the installer keeps its layout files and mounts the new disk.
const WORK: &str = "/tmp/install";

/// The Dukovo root's subvolumes, as `./ob image` makes them, but for the
/// slot, which is copied into `@system.new` and kept read-only as
/// `@system.a`.
const SUBVOLUMES: [&str; 6] =
    ["@settings", "@state", "@applications", "@users", "@shared", "@snapshots"];

/// What the root needs beyond the release itself: room for settings,
/// state, users' files and the next update's slot.
const ROOM: u64 = 1 << 30;

/// The guided layout's partitions before the root, in MiB: the BIOS boot
/// partition and the EFI system partition.
const BEFORE_ROOT_MIB: u64 = 1 + 512;

/// Where the release was found and what it is.
pub struct Source {
    /// The repository: the stick's `release` folder.
    pub repo: PathBuf,
    /// The disk it is on, which is never installed onto.
    pub stick: Option<String>,
    /// The release, its signatures checked.
    pub release: Release,
}

/// The install's partitions, by their names in `/devices`.
struct Parts {
    disk: String,
    efi: String,
    swap: Option<String>,
    root: String,
}

/// Finds the release, in `release` or on the stick, and checks it
/// against the root the system trusts: every signature and version, and
/// the list's hash. Its files are checked as they are copied.
///
/// # Errors
///
/// If there is no release, or it is not signed by the trusted root.
pub fn find_release(release: Option<&Path>, log: &mut Log) -> Result<Source, String> {
    log.step(Step::Release);
    let (repo, stick) = match release {
        Some(dir) => {
            (dir.to_path_buf(), disks::live_partition().ok().map(|(name, _)| disks::disk_of(&name)))
        }
        None => {
            let (name, mounted) = disks::live_partition()?;
            (mounted.join("release"), Some(disks::disk_of(&name)))
        }
    };
    let root =
        fs::read(TRUSTED_ROOT).map_err(|err| format!("the trusted root {TRUSTED_ROOT}: {err}"))?;
    let opened = system_release::repo::open(&repo, &root, None)?;
    log.note(&format!(
        "release {} ({}), built {} from commit {}, {} files of {} MiB, in {}",
        opened.release.version,
        &opened.name[..16],
        opened.release.built,
        opened.release.commit,
        opened.release.files.iter().filter(|entry| entry.file().is_some()).count(),
        opened.release.bytes() >> 20,
        repo.display()
    ));
    Ok(Source { repo, stick, release: opened.release })
}

/// Installs the release onto the disk the answers name.
///
/// # Errors
///
/// The step that failed and why. The disk is then not ready to start:
/// the installer must run again.
pub fn install(answers: &Answers, source: &Source, log: &mut Log) -> Result<(), String> {
    let disk = answers.target();
    log.installing(disk, &source.release.version);
    log.step(Step::Disk);
    check_target(disk, answers, source)?;
    fs::create_dir_all(WORK).map_err(|err| format!("making {WORK}: {err}"))?;
    let parts = partition(disk, answers, log)?;
    format(&parts, log)?;
    let root = Path::new(WORK).join("root");
    mount(&parts.root, &root, &["-t", "btrfs", "-o", "compress"], log)?;
    let done = fill_root(answers, source, &parts, &root, log);
    // The root is unmounted whatever happened, so that its writes reach
    // the disk and the installer can run again.
    let unmounted = run_when_free(log, "umount", &[&path(&root)]);
    done.and(unmounted)
}

/// Checks that `disk` is a whole disk, not the stick, and large enough.
fn check_target(disk: &str, answers: &Answers, source: &Source) -> Result<(), String> {
    let bytes = disks::bytes(disk)?;
    if disks::is_partition(disk) {
        return Err(format!(
            "disk.target: {disk} is a partition; name its disk, {}",
            disks::disk_of(disk)
        ));
    }
    if source.stick.as_deref() == Some(disk) {
        return Err(format!("disk.target: {disk} is the install stick this system runs from"));
    }
    let swap = u64::from(answers.disk.swap_gib) << 30;
    let needed = (BEFORE_ROOT_MIB << 20) + swap + source.release.bytes() + ROOM;
    if bytes < needed {
        return Err(format!(
            "disk.target: {disk} holds {} MiB, and this install needs {} MiB",
            bytes >> 20,
            needed.div_ceil(1 << 20)
        ));
    }
    Ok(())
}

/// Writes the guided layout's table on `disk` with rdisk(8), and waits
/// for its partitions.
fn partition(disk: &str, answers: &Answers, log: &mut Log) -> Result<Parts, String> {
    log.step(Step::Partition);
    let layout = Path::new(WORK).join(format!("layout-{disk}.json"));
    let swap_gib = answers.disk.swap_gib;
    fs::write(&layout, guided_layout(swap_gib))
        .map_err(|err| format!("writing {}: {err}", layout.display()))?;
    log.run("rdisk", &["--script", &path(&layout), &path(&disks::device(disk))])?;
    let root_number = if swap_gib > 0 { 4 } else { 3 };
    let parts = Parts {
        disk: disk.to_string(),
        efi: disks::partition(disk, 2),
        swap: (swap_gib > 0).then(|| disks::partition(disk, 3)),
        root: disks::partition(disk, root_number),
    };
    for name in [&parts.efi, &parts.root].into_iter().chain(&parts.swap) {
        disks::wait_for(name)?;
    }
    Ok(parts)
}

/// The guided layout (rdisk-layout(5)): a BIOS boot partition, the EFI
/// system partition second, where a BIOS's GRUB looks for its files
/// (rgrub-setup(8)), swap if asked for, and the Dukovo root in the rest.
#[must_use]
pub fn guided_layout(swap_gib: u32) -> String {
    let mut partitions = vec![
        r#"{ "number": 1, "type": "bios-boot", "size": "1MiB", "name": "BIOS boot" }"#.to_string(),
        r#"{ "number": 2, "type": "efi", "size": "512MiB", "name": "EFI system" }"#.to_string(),
    ];
    if swap_gib > 0 {
        partitions.push(format!(
            r#"{{ "number": 3, "type": "swap", "size": "{swap_gib}GiB", "name": "Swap" }}"#
        ));
    }
    let root = if swap_gib > 0 { 4 } else { 3 };
    partitions.push(format!(
        r#"{{ "number": {root}, "type": "dukovo-root", "size": "rest", "name": "Dukovo root" }}"#
    ));
    format!(
        "{{\n  \"table\": \"gpt\",\n  \"partitions\": [\n    {}\n  ]\n}}\n",
        partitions.join(",\n    ")
    )
}

/// Makes the filesystems: FAT32 on the EFI system partition, swap, and
/// the root's Btrfs.
fn format(parts: &Parts, log: &mut Log) -> Result<(), String> {
    log.step(Step::Format);
    log.run("mkfs-fat", &["-F", "32", "-n", "DUKOVO_EFI", &path(&disks::device(&parts.efi))])?;
    if let Some(swap) = &parts.swap {
        log.run("mkswap", &["-L", "swap", &path(&disks::device(swap))])?;
    }
    log.run("mkfs-btrfs", &["-f", "-L", "dukovo-root", &path(&disks::device(&parts.root))])?;
    Ok(())
}

/// Fills the mounted root: subvolumes, the system, settings, GRUB, and
/// the saved answers and log.
fn fill_root(
    answers: &Answers,
    source: &Source,
    parts: &Parts,
    root: &Path,
    log: &mut Log,
) -> Result<(), String> {
    log.step(Step::Subvolumes);
    for name in SUBVOLUMES.iter().chain(&["@system.new"]) {
        log.run("subvolume", &["create", &path(&root.join(name))])?;
    }
    copy_system(source, root, log)?;
    log.step(Step::Settings);
    let target = Target {
        slot: &root.join("@system.a"),
        settings: &root.join("@settings"),
        state: &root.join("@state"),
    };
    settings::write(answers, &target)?;
    install_grub(parts, &root.join("@system.a"), log)?;
    save(answers, &target, log)
}

/// Copies the release into `@system.new`, checking every file, then
/// keeps it as the read-only slot `@system.a`.
fn copy_system(source: &Source, root: &Path, log: &mut Log) -> Result<(), String> {
    log.step(Step::Copy);
    let new = root.join("@system.new");
    let total = source.release.bytes();
    let copied =
        copy::release(&source.repo, &source.release, &new, &mut |done| log.copied(done, total))?;
    log.note(&format!(
        "copied {} files of {} MiB, each matching the signed list",
        copied.files,
        copied.bytes >> 20
    ));
    log.run("subvolume", &["snapshot", "-r", &path(&new), &path(&root.join("@system.a"))])?;
    run_when_free(log, "subvolume", &["delete", &path(&new)])
}

/// Puts GRUB on the disk for a BIOS and on its EFI system partition for
/// UEFI, from the new slot's own GRUB, and the menu that starts the slot,
/// which names the root by its Btrfs UUID (ADR 0307).
fn install_grub(parts: &Parts, slot: &Path, log: &mut Log) -> Result<(), String> {
    log.step(Step::Grub);
    let images = path(&slot.join("boot/grub"));
    log.run("rgrub-setup", &["-d", &images, &path(&disks::device(&parts.disk))])?;
    log.run("rgrub-setup", &["--efi", "-d", &images, &path(&disks::device(&parts.efi))])?;
    let uuid = btrfs_uuid(&parts.root)?;
    let template = slot.join("boot/grub/installed.cfg");
    let menu =
        fs::read_to_string(&template).map_err(|err| format!("{}: {err}", template.display()))?;
    let esp = Path::new(WORK).join("esp");
    mount(&parts.efi, &esp, &["-t", "fat"], log)?;
    let written = settings::make_dir(&esp.join("boot/grub"), 0o755).and_then(|()| {
        settings::write_file(
            &esp.join("boot/grub/grub.cfg"),
            &menu.replace("@ROOT_UUID@", &uuid),
            0o644,
        )
    });
    log.note(&format!("the menu starts the root {uuid}"));
    let unmounted = run_when_free(log, "umount", &[&path(&esp)]);
    written.and(unmounted)
}

/// The UUID of the Btrfs on the partition `name`, from its superblock,
/// as GRUB's `search --fs-uuid` and the kernel's command line write it.
fn btrfs_uuid(name: &str) -> Result<String, String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    // The superblock is at 64 KiB; its filesystem UUID is 32 bytes in.
    const FSID: u64 = 0x1_0000 + 0x20;
    let device = disks::device(name);
    let mut file = fs::File::open(&device).map_err(|err| format!("{}: {err}", device.display()))?;
    let mut uuid = [0u8; 16];
    file.seek(SeekFrom::Start(FSID))
        .and_then(|_| file.read_exact(&mut uuid))
        .map_err(|err| format!("reading the Btrfs UUID of {}: {err}", device.display()))?;
    let hex = system_release::list::hex(&uuid);
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

/// Saves the answers, without passwords' hashes unless asked, and the
/// log so far, in the new system.
fn save(answers: &Answers, target: &Target, log: &mut Log) -> Result<(), String> {
    log.step(Step::Save);
    let install = target.settings.join("install");
    settings::make_dir(&install, 0o755)?;
    let mode = if answers.keep_password_hashes { 0o600 } else { 0o644 };
    let saved = String::from_utf8_lossy(&answers.to_saved()).into_owned();
    settings::write_file(&install.join("answers.json"), &saved, mode)?;
    let logs = target.state.join("log");
    settings::make_dir(&logs, 0o755)?;
    log.note("installed: the log ends here");
    let copy = logs.join("install.log");
    fs::copy(log.path(), &copy).map_err(|err| format!("writing {}: {err}", copy.display()))?;
    Ok(())
}

/// How long a step waits for what the command before it still holds.
const RELEASE_WAIT: Duration = Duration::from_secs(5);

/// Runs `program` as [`Log::run`] does, and again while it fails because
/// something is busy. Radix frees what a process held a moment after the
/// process has ended, not before its parent sees it end, so a subvolume
/// or a filesystem the last command used can still be in use: deleting
/// `@system.new` right after `subvolume snapshot` read it is refused
/// now and then (ADR 0309).
fn run_when_free(log: &mut Log, program: &str, args: &[&str]) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match log.run(program, args) {
            Err(why) if why.contains("busy") && started.elapsed() < RELEASE_WAIT => {
                log.note("still in use; trying again");
                std::thread::sleep(Duration::from_millis(250));
            }
            done => return done.map(drop),
        }
    }
}

/// Mounts the partition `name` at `dir`, made if need be.
fn mount(name: &str, dir: &Path, options: &[&str], log: &mut Log) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| format!("making {}: {err}", dir.display()))?;
    let mut args = options.to_vec();
    let device = path(&disks::device(name));
    let dir = path(dir);
    args.extend([device.as_str(), dir.as_str()]);
    log.run("mount", &args).map(|_| ())
}

/// A path as a command's argument.
fn path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guided_layout_is_a_layout_file() {
        let with_swap = guided_layout(8);
        assert!(
            with_swap.contains(r#""number": 3, "type": "swap", "size": "8GiB""#),
            "{with_swap}"
        );
        assert!(with_swap.contains(r#""number": 4, "type": "dukovo-root", "size": "rest""#));
        let value: serde_json::Value = serde_json::from_str(&guided_layout(0)).unwrap();
        let parts = value["partitions"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2]["type"], "dukovo-root");
        assert_eq!(parts[2]["number"], 3);
        assert_eq!(parts[1]["number"], 2, "a BIOS's GRUB finds its files on the second partition");
    }
}
