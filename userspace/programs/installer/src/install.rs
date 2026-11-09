// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install itself, step by step, as the approved design lists the
//! steps (ADR 0309): check the release's signatures, partition the disk,
//! make its filesystems and subvolumes, copy the system into its slot,
//! checking every file, write the settings and accounts, put GRUB on the
//! disk with the menu that starts the slot (ADR 0307), and save the
//! answers and the install log in the new system. Next to Windows, its
//! volume is shrunk before the disk is partitioned, and a UEFI boot entry
//! starts Dukovo's GRUB, whose menu starts Windows too (ADR 0314). The
//! mirror layout gives two disks the same partitions and GRUB, and makes
//! their roots one Btrfs RAID1 (ADR 0315). Each step runs the system's
//! own tools (resize-ntfs, rdisk, mkfs-fat, mkswap, mkfs-btrfs, device,
//! balance, mount, subvolume, rgrub-setup and efiboot), so the log shows
//! each command as it ran.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use system_release::list::Release;

use crate::answers::{Answers, Change, Format, Layout, Role};
use crate::copy;
use crate::disks;
use crate::features;
use crate::log::{Log, Step};
use crate::machine::{self, DiskInfo};
use crate::plan;
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

/// Where the release was found and what it is.
pub struct Source {
    /// The repository: the `release` folder of the stick's DUKOVO_LIVE
    /// partition or of the install DVD.
    pub repo: PathBuf,
    /// The disk the live system started from, the stick or the DVD's
    /// drive, which is never installed onto.
    pub stick: Option<String>,
    /// Whether that is the install DVD's drive, which the messages call a
    /// disc.
    pub disc: bool,
    /// The release, its signatures checked.
    pub release: Release,
}

impl Source {
    /// What the messages call the medium the live system started from:
    /// `disc` for the install DVD, `stick` otherwise.
    #[must_use]
    pub fn medium(&self) -> &'static str {
        if self.disc { "disc" } else { "stick" }
    }
}

/// A disk the install changes: what it is now, the change, and the
/// table it has after.
struct Planned {
    disk: DiskInfo,
    change: Change,
    after: disk::Table,
}

/// The install's partitions, by their names in `/devices`: the disks
/// that start the system, the root, the mirror layout's second root, and,
/// next to Windows, the number of the EFI system partition the firmware's
/// boot entry names.
struct Parts {
    /// The target, then the mirror layout's second disk: each gets GRUB
    /// and the menu, so that either starts the system alone.
    boot: Vec<BootDisk>,
    root: String,
    /// The second disk's root partition, which the root's Btrfs takes as
    /// its mirror.
    mirror: Option<String>,
    boot_entry: Option<u32>,
}

/// A disk that starts the system: its EFI system partition, and whether
/// it has a BIOS boot partition for GRUB.
struct BootDisk {
    disk: String,
    efi: String,
    bios_boot: bool,
}

/// The description of Dukovo's UEFI boot entry, as the firmware's boot
/// menu shows it.
const BOOT_ENTRY: &str = "Dukovo";

/// GRUB's environment block on the EFI system partition, where the menu
/// keeps the entry chosen last: 1024 bytes, as `grub-editenv` makes it.
const GRUBENV: &str = "boot/grub/grubenv";

/// Finds the release, in `release` or on the stick or DVD, and checks it
/// against the root the system trusts: every signature and version, and
/// the list's hash. Its files are checked as they are copied.
///
/// # Errors
///
/// If there is no release, or it is not signed by the trusted root.
pub fn find_release(release: Option<&Path>, log: &mut Log) -> Result<Source, String> {
    log.step(Step::Release);
    let (repo, medium) = match release {
        Some(dir) => (dir.to_path_buf(), disks::live_medium().ok()),
        None => {
            let medium = disks::live_medium()?;
            (medium.mounted.join("release"), Some(medium))
        }
    };
    let disc = medium.as_ref().is_some_and(|medium| medium.disc);
    let stick = medium.map(|medium| medium.disk);
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
    Ok(Source { repo, stick, disc, release: opened.release })
}

/// Installs the release onto the disk the answers name.
///
/// # Errors
///
/// The step that failed and why. The disk is then not ready to start:
/// the installer must run again.
pub fn install(answers: &Answers, source: &Source, log: &mut Log) -> Result<(), String> {
    let disk = answers.target();
    log.installing(&answers.disks(), &source.release.version);
    log.step(Step::Disk);
    // The live system is the release copied: what it lacks, the new
    // system would too, and this is the last moment nothing is written.
    features::check(answers, Path::new("/system"))?;
    let planned = plan_disks(answers, source)?;
    fs::create_dir_all(WORK).map_err(|err| format!("making {WORK}: {err}"))?;
    shrink(&planned, log)?;
    let parts = partition(&planned, disk, log)?;
    format(&planned, disk, log)?;
    let root = Path::new(WORK).join("root");
    mount(&parts.root, &root, &["-t", "btrfs", "-o", "compress"], log)?;
    let done =
        mirror(&parts, &root, log).and_then(|()| fill_root(answers, source, &parts, &root, log));
    // The root is unmounted whatever happened, so that its writes reach
    // the disk and the installer can run again.
    let unmounted = run_when_free(log, "umount", &[&path(&root)]);
    done.and(unmounted)
}

/// Works out the change of each disk the answers' layout touches (ADR
/// 0311), and checks it against the disk as it is now: each a whole disk
/// that is not the stick, each change fitting, the root on a GPT and big
/// enough for the release and room to spare.
fn plan_disks(answers: &Answers, source: &Source) -> Result<Vec<Planned>, String> {
    let target =
        machine::read_disk(answers.target()).map_err(|why| format!("disk.target: {why}"))?;
    let root_bytes = source.release.bytes() + plan::ROOM;
    let swap_gib = answers.disk.swap_gib;
    let changes = match answers.disk.layout {
        Layout::Guided => vec![plan::guided(&target, swap_gib)?],
        Layout::Free => vec![plan::free_space(&target, swap_gib, root_bytes)?],
        Layout::Manual => answers.disk.changes.clone(),
        Layout::Beside => {
            let space = u64::from(answers.disk.space_gib.unwrap_or(0)) << 30;
            let entries = machine::boot_entries();
            vec![
                plan::beside(&target, entries, swap_gib, space, root_bytes)
                    .map_err(|why| format!("disk: {why}"))?,
            ]
        }
        Layout::Mirror => {
            let name = answers.mirror().ok_or("disk.mirror: no second disk")?;
            let second = machine::read_disk(name).map_err(|why| format!("disk.mirror: {why}"))?;
            plan::mirror(&target, &second, swap_gib, root_bytes)
                .map_err(|why| format!("disk: {why}"))?
                .into()
        }
    };
    let mut planned = Vec::new();
    for change in changes {
        let disk = if change.disk == target.name {
            target.clone()
        } else {
            machine::read_disk(&change.disk).map_err(|why| format!("disk.changes: {why}"))?
        };
        if source.stick.as_deref() == Some(disk.name.as_str()) {
            return Err(format!("disk: {} is the install stick this system runs from", disk.name));
        }
        let after = plan::after(&disk, &change).map_err(|why| format!("disk: {why}"))?;
        planned.push(Planned { disk, change, after });
    }
    let on_target = planned.iter().find(|p| p.disk.name == target.name);
    let root = on_target.and_then(|p| {
        let part = p.change.partitions.iter().find(|part| part.role == Role::Root)?;
        p.after.get(part.number).map(|root| (p, root.sectors() * u64::from(p.after.sector_size)))
    });
    let Some((p, bytes)) = root else {
        return Err(format!("disk: no root partition on the target, {}", target.name));
    };
    if p.after.scheme != disk::Scheme::Gpt {
        return Err(format!(
            "disk: {} has an MBR table, and Dukovo's disk needs a GPT",
            target.name
        ));
    }
    plan::check_bios_boot(&target.name, &p.after).map_err(|why| format!("disk: {why}"))?;
    if bytes < root_bytes {
        return Err(format!(
            "disk.target: the root on {} holds {} MiB, and this install needs {} MiB",
            target.name,
            bytes >> 20,
            root_bytes.div_ceil(1 << 20)
        ));
    }
    Ok(planned)
}

/// Shrinks Windows' volume, on a disk whose change shrinks its partition,
/// with resize-ntfs(8): checked before and after, its files kept. The
/// volume is left marked for Windows to check at its next start, as after
/// `ntfsresize`; the partition is shrunk to match when the table is
/// written.
fn shrink(planned: &[Planned], log: &mut Log) -> Result<(), String> {
    for p in planned {
        let Some(shrink) = p.change.shrink else { continue };
        log.step(Step::Shrink);
        let partition = disks::partition(&p.disk.name, shrink.number);
        let device = path(&disks::device(&partition));
        let bytes = shrink.sectors * u64::from(p.disk.sector_size);
        log.note(&format!(
            "shrinking Windows' volume on {partition} to {bytes} bytes ({} MiB)",
            bytes >> 20
        ));
        log.run("resize-ntfs", &["--check", &device])?;
        log.run("resize-ntfs", &["--no-progress-bar", "--size", &bytes.to_string(), &device])?;
        log.run("resize-ntfs", &["--check", &device])?;
    }
    Ok(())
}

/// Writes each disk's new table with rdisk(8), from a layout file that
/// gives every new partition's place, and waits for its partitions.
fn partition(planned: &[Planned], target: &str, log: &mut Log) -> Result<Parts, String> {
    log.step(Step::Partition);
    for p in planned {
        let name = &p.disk.name;
        let layout = Path::new(WORK).join(format!("layout-{name}.json"));
        let text = plan::layout_text(&p.disk, &p.change)?;
        fs::write(&layout, &text).map_err(|err| format!("writing {}: {err}", layout.display()))?;
        log.note(&format!("{}: {}", layout.display(), text.split_whitespace().collect::<String>()));
        log.run("rdisk", &["--script", &path(&layout), &path(&disks::device(name))])?;
        for part in &p.change.partitions {
            let partition = disks::partition(name, part.number);
            match p.after.get(part.number) {
                // In 512-byte sectors, as the kernel counts.
                Some(made) => {
                    let first = made.first * u64::from(p.after.sector_size) / 512;
                    disks::wait_for_start(&partition, first)?;
                }
                None => disks::wait_for(&partition)?,
            }
        }
    }
    let p = planned.iter().find(|p| p.disk.name == target).ok_or("the target is not changed")?;
    let root = numbered(p, Role::Root).ok_or("no root partition")?;
    let mut parts = Parts {
        boot: vec![boot_disk(p)?],
        root: disks::partition(target, root),
        mirror: None,
        boot_entry: p.change.shrink.and(numbered(p, Role::Efi)),
    };
    // The mirror layout's second disk has a root too, and starts the
    // system as the target does.
    for other in planned.iter().filter(|other| other.disk.name != target) {
        if let Some(number) = numbered(other, Role::Root) {
            parts.mirror = Some(disks::partition(&other.disk.name, number));
            parts.boot.push(boot_disk(other)?);
        }
    }
    Ok(parts)
}

/// The number of the partition for `role` that `p`'s change makes or
/// formats, if it has one.
fn numbered(p: &Planned, role: Role) -> Option<u32> {
    p.change.partitions.iter().find(|part| part.role == role).map(|part| part.number)
}

/// The disk `p` changes as one that starts the system.
fn boot_disk(p: &Planned) -> Result<BootDisk, String> {
    let name = &p.disk.name;
    let efi = numbered(p, Role::Efi).ok_or_else(|| format!("no EFI system partition on {name}"))?;
    let bios_boot = disk::types::parse("bios-boot", disk::Scheme::Gpt);
    Ok(BootDisk {
        disk: name.clone(),
        efi: disks::partition(name, efi),
        bios_boot: p.after.partitions.iter().any(|part| Some(part.kind) == bios_boot),
    })
}

/// Makes the filesystems each change asks for: FAT32 on the EFI system
/// partition, swap, the root's Btrfs on the `target`, and those of data
/// partitions. The mirror layout's second root is left for [`mirror`].
fn format(planned: &[Planned], target: &str, log: &mut Log) -> Result<(), String> {
    log.step(Step::Format);
    for p in planned {
        for part in &p.change.partitions {
            if part.role == Role::Root && p.disk.name != target {
                continue;
            }
            let device = path(&disks::device(&disks::partition(&p.disk.name, part.number)));
            let (program, args): (&str, &[&str]) = match (part.role, part.format) {
                (Role::Efi, _) => ("mkfs-fat", &["-F", "32", "-n", "DUKOVO_EFI"]),
                (Role::Swap, _) => ("mkswap", &["-L", "swap"]),
                (Role::Root, _) => ("mkfs-btrfs", &["-f", "-L", "dukovo-root"]),
                (Role::Data, Some(Format::Fat32)) => ("mkfs-fat", &["-F", "32"]),
                (Role::Data, Some(Format::Exfat)) => ("mkfs-exfat", &[]),
                (Role::Data, Some(Format::Btrfs)) => ("mkfs-btrfs", &["-f"]),
                (Role::BiosBoot, _) | (Role::Data, None) => continue,
            };
            let mut args = args.to_vec();
            args.push(&device);
            log.run(program, &args)?;
        }
    }
    Ok(())
}

/// Makes the root's Btrfs, mounted at `root`, a RAID1 of two disks, for
/// the mirror layout: device(8) adds the second disk's root partition,
/// and balance(8) converts the chunks the new filesystem has, of data,
/// metadata and the chunk tree, to RAID1, a copy on each disk. The root
/// is all but empty, so the balance moves a few MiB; every chunk written
/// after is RAID1 too. Either disk then holds every byte, and the system
/// starts from either alone (ADR 0153).
fn mirror(parts: &Parts, root: &Path, log: &mut Log) -> Result<(), String> {
    let Some(second) = &parts.mirror else { return Ok(()) };
    log.step(Step::Mirror);
    let root = path(root);
    // -f: the partition is new, but may hold an earlier install's root.
    log.run("device", &["add", "-f", &path(&disks::device(second)), &root])?;
    log.run("balance", &["start", "-dconvert=raid1", "-mconvert=raid1", &root])?;
    log.note(&format!("the root is a Btrfs RAID1 of {} and {second}", parts.root));
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
    features::turn_on(answers, &target, log)?;
    let console = answers.serial_console(machine::on_serial_console());
    install_grub(parts, &root.join("@system.a"), console, log)?;
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

/// Puts GRUB on each disk that starts the system ([`Parts::boot`]): for
/// a BIOS, if it has a BIOS boot partition, and on its EFI system
/// partition for UEFI, from the new slot's own GRUB, with the menu that
/// starts the slot, which names the root by its Btrfs UUID (ADR 0307).
/// The two disks of a mirror have one UUID, so either one's menu finds
/// the root on whichever disk is there. With `serial_console`, the menu's
/// Dukovo entries start the kernel with `console=ttyS0`, so that the
/// system's sign-in is on the serial port too (ADR 0318).
fn install_grub(
    parts: &Parts,
    slot: &Path,
    serial_console: bool,
    log: &mut Log,
) -> Result<(), String> {
    log.step(Step::Grub);
    let uuid = btrfs_uuid(&parts.root)?;
    let template = slot.join("boot/grub/installed.cfg");
    let menu =
        fs::read_to_string(&template).map_err(|err| format!("{}: {err}", template.display()))?;
    let console = serial_console.then(machine::serial_console_flag);
    let menu = fill_menu(&menu, &uuid, console.as_deref());
    if let Some(flag) = &console {
        log.note(&format!("the system keeps a console on the serial port: {flag}"));
    }
    for disk in &parts.boot {
        grub_on(disk, slot, &menu, log)?;
    }
    log.note(&format!("the menu starts the root {uuid}"));
    if let (Some(number), Some(first)) = (parts.boot_entry, parts.boot.first()) {
        boot_entry(&first.disk, number, log)?;
    }
    Ok(())
}

/// The installed system's menu, from the slot's template `template`: its
/// root `uuid`, and the serial console's kernel flag `console`, if it
/// keeps one, in their placeholders.
fn fill_menu(template: &str, uuid: &str, console: Option<&str>) -> String {
    let console = console.map_or_default(|flag| format!(" {flag}"));
    template.replace("@ROOT_UUID@", uuid).replace("@CONSOLE@", &console)
}

/// Puts GRUB from the slot on `disk`, and `menu` with GRUB's environment
/// block on its EFI system partition.
fn grub_on(disk: &BootDisk, slot: &Path, menu: &str, log: &mut Log) -> Result<(), String> {
    let images = path(&slot.join("boot/grub"));
    if disk.bios_boot {
        log.run("rgrub-setup", &["-d", &images, &path(&disks::device(&disk.disk))])?;
    } else {
        log.note(&format!(
            "{} has no BIOS boot partition: it starts by UEFI only, with no GRUB for a BIOS",
            disk.disk
        ));
    }
    log.run("rgrub-setup", &["--efi", "-d", &images, &path(&disks::device(&disk.efi))])?;
    let esp = Path::new(WORK).join("esp");
    mount(&disk.efi, &esp, &["-t", "fat"], log)?;
    let written = settings::make_dir(&esp.join("boot/grub"), 0o755)
        .and_then(|()| settings::write_file(&esp.join("boot/grub/grub.cfg"), menu, 0o644));
    // The menu remembers the entry chosen last in GRUB's environment
    // block, which must be there, whole, for GRUB to write it.
    let environment =
        written.and_then(|()| settings::write_file(&esp.join(GRUBENV), &grub_environment(), 0o644));
    let unmounted = run_when_free(log, "umount", &[&path(&esp)]);
    environment.and(unmounted)
}

/// GRUB's empty environment block: its first line, then `#` to 1024
/// bytes.
fn grub_environment() -> String {
    let mut block = String::from("# GRUB Environment Block\n");
    block.push_str(&"#".repeat(1024 - block.len()));
    block
}

/// Makes the firmware's boot entry for Dukovo first in its boot order,
/// starting GRUB on the EFI system partition `number` of `disk`, as
/// efiboot(8) makes one: Windows Boot Manager, which was first, then
/// starts from Dukovo's menu. An entry an earlier install made for Dukovo
/// is taken out first, as Linux's grub-install does.
fn boot_entry(disk: &str, number: u32, log: &mut Log) -> Result<(), String> {
    let listed = log.run("efiboot", &[])?;
    for old in earlier_entries(&listed) {
        log.run("efiboot", &["--bootnum", &old, "--delete-bootnum", "--quiet"])?;
    }
    let number = number.to_string();
    log.run(
        "efiboot",
        &["--create", "--disk", disk, "--part", &number, "--label", BOOT_ENTRY, "--quiet"],
    )?;
    log.run("efiboot", &["--verbose"]).map(drop)
}

/// The numbers of the boot entries efiboot lists in `listed` as Dukovo's,
/// by their description: `Boot0003* Dukovo`.
fn earlier_entries(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Boot")?;
            let (number, label) = (rest.get(..4)?, rest.get(4..)?);
            let label = label.strip_prefix('*').or_else(|| label.strip_prefix(' '))?;
            let hex = number.bytes().all(|b| b.is_ascii_hexdigit());
            (hex && label.trim() == BOOT_ENTRY).then(|| number.to_string())
        })
        .collect()
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
    fn an_earlier_installs_boot_entries_are_found_by_their_description() {
        let listed = "BootCurrent: 0001\nBootOrder: 0003,0000,0001\n\
                      Boot0000* Windows Boot Manager\nBoot0001* UEFI QEMU HARDDISK\n\
                      Boot0003* Dukovo\nBoot000A  Dukovo\nBoot0004* Dukovo lab\n";
        assert_eq!(earlier_entries(listed), ["0003", "000A"]);
        assert!(earlier_entries("BootOrder: 0003\n").is_empty());
    }

    #[test]
    fn the_menu_keeps_the_serial_console_only_when_asked() {
        let template = "radix root=UUID=@ROOT_UUID@ panic=10@CONSOLE@ quiet\n";
        let uuid = "90c9a303-3954-4174-8f8b-ee75a92a9eb0";
        assert_eq!(
            fill_menu(template, uuid, None),
            format!("radix root=UUID={uuid} panic=10 quiet\n")
        );
        assert_eq!(
            fill_menu(template, uuid, Some("console=ttyS0,9600")),
            format!("radix root=UUID={uuid} panic=10 console=ttyS0,9600 quiet\n")
        );
    }

    #[test]
    fn grubs_environment_block_is_whole() {
        let block = grub_environment();
        assert_eq!(block.len(), 1024, "GRUB writes it only when it is 1024 bytes");
        assert!(block.starts_with("# GRUB Environment Block\n#"));
    }
}
