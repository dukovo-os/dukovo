// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A machine installed from an old release updates to this build, keeps
//! all it held, goes back, and updates again (ADR 0536). One case for each
//! release in *FIXTURE_RELEASES* (tools/xtask/src/test/upgrade.rs), under
//! UEFI as *update-from-VERSION* and under a BIOS as
//! *update-from-VERSION-bios*, so that GRUB's *core.img* in the BIOS boot
//! partition is the old release's too. The machine boots a copy-on-write
//! overlay of the release's fixture (*ob fixture release VERSION*), the
//! disk that release's own tree made, which is never written; the keys
//! the fixture trusts are this checkout's, adopted before anything is
//! signed when it has none.
//!
//! The old release runs first, and alice, its administrator, fills the
//! machine with commands that release has: a second account, bob
//! (*user*), files in her home and a checksum of one, a hostname and a
//! time zone (*hostname*, *timedatectl*), a setting of her shell, a
//! service that is off until it is turned on, *sshd* (*service*), the
//! health check's *stable* wait made 10 seconds, and an app she makes,
//! packs and installs (*bundle*) from nothing the network gives. Each
//! thing is checked on the old release's own console.
//!
//! A second disk holds a full repository of this build, signed by the
//! fixture's keys and updating from the oldest fixture release, with no
//! file left out, since the slot it is staged from is the old release's.
//! *sysupdate stage* builds it into slot B, the restart tries it, and it
//! proves itself; each thing is checked again, the app's data among them
//! as its run counts on. *sysupdate verify* reads the slot back against
//! the signed list. *sysupdate rollback --keep-settings* makes the old
//! release the default; it starts, and each thing is checked once more.
//! The update is staged and proved again, over the slot it left, every
//! thing is checked a last time, and the machine powers off.

use std::sync::OnceLock;

use super::cases::crash_dump::answer_within;
use super::cases::update_stage::{UPDATES, started};
use super::*;
use crate::fixture::release;

/// The releases the machine can be installed from, with a fixture each
/// (`./ob fixture release VERSION`): a case of each is made under both
/// firmware types. The list grows as releases are tagged.
pub const FIXTURE_RELEASES: [&str; 1] = ["0.15.0"];

/// What a case's name starts with, before the release.
const PREFIX: &str = "update-from-";

/// What a BIOS case's name ends with.
const BIOS_SUFFIX: &str = "-bios";

/// The size of the disk that holds the full repository of this build, in
/// KiB: the slot of a system is a few hundred MiB at most, since the whole
/// root is [`image::ROOT_BYTES`] and holds two slots.
const FULL_KIB: u64 = 512 * 1024;

/// The disk of the full repository, which Spark mounts at
/// [`UPDATES`].
const UPGRADE_DISKS: &[Disk] = &[update_full_disk(FULL_KIB, "UPDATES")];

/// The folder of the repository on the disk.
const FULL: &str = "full";

/// The test update the repository is made as: this build under the
/// version of `good`.
const UPDATE: &str = "good";

/// How long a prompt may take after a start, in seconds: the machine is
/// the old release's, under emulation too.
const SIGN_IN_SECONDS: u64 = 240;

/// How long a command typed at the console may take, in seconds: a stage
/// reads a whole slot and writes another.
const COMMAND_SECONDS: u64 = 600;

/// The name of the case that boots the fixture of `release`, under a
/// BIOS if `bios`.
pub fn case_name(release: &str, bios: bool) -> String {
    format!("{PREFIX}{release}{}", if bios { BIOS_SUFFIX } else { "" })
}

/// The release a case's name says it updates from: `None` for a case that
/// is none of these.
pub fn release_of(case: &Case) -> Option<&'static str> {
    if case.image != TestImage::Fixture {
        return None;
    }
    let rest = case.name.strip_prefix(PREFIX)?;
    let release = rest.strip_suffix(BIOS_SUFFIX).unwrap_or(rest);
    FIXTURE_RELEASES.iter().copied().find(|&known| known == release)
}

/// The cases, a pair for each of [`FIXTURE_RELEASES`]: UEFI, then BIOS.
pub fn cases() -> &'static [Case] {
    static CASES: OnceLock<Vec<Case>> = OnceLock::new();
    CASES.get_or_init(|| {
        FIXTURE_RELEASES
            .iter()
            .flat_map(|release| [false, true].map(|bios| case(release, bios)))
            .collect()
    })
}

/// The case of `release` under UEFI, or under a BIOS. Its name and its
/// description are made once and kept for as long as the program runs.
fn case(release: &str, bios: bool) -> Case {
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    let firmware = if bios { "a BIOS" } else { "UEFI" };
    Case {
        name: leak(case_name(release, bios)),
        about: leak(format!(
            "a machine installed from Dukovo {release}, under {firmware}, filled with accounts, files, settings, a service and an app, updates to this build, keeps all of it, rolls back to {release} keeping all of it, and updates again (ADR 0536)"
        )),
        firmware: if bios { Firmware::Bios } else { Firmware::Uefi },
        media: Media::Disk,
        // The fixture's disk keeps the command line the release made.
        cmdline: "",
        cpus: 2,
        memory: "1G",
        reboot: true,
        image: TestImage::Fixture,
        cpu_model: None,
        qemu: &[],
        script: update_from_release,
        disks: UPGRADE_DISKS,
        second: None,
    }
}

/// The cases with the description `dukovo-boot-tests(7)` gives them: this
/// file's, and for the BIOS ones what differs.
pub fn described() -> Vec<(&'static Case, String)> {
    let doc = super::file_doc(include_str!("upgrade.rs"));
    cases()
        .iter()
        .map(|case| {
            let text = match release_of(case) {
                Some(release) if case.firmware == Firmware::Bios => format!(
                    "As *{}*, under a BIOS: GRUB's *core.img* in the BIOS boot partition is \
                     Dukovo {release}'s, which the update keeps and the rollback starts again.",
                    case_name(release, false)
                ),
                _ => doc.clone(),
            };
            (case, text)
        })
        .collect()
}

/// Whether `case` boots a fixture, which `ob fixture release` makes
/// beforehand: a run that is not asked for it by name leaves it out.
pub fn needs_fixture(case: &Case) -> bool {
    case.image == TestImage::Fixture
}

/// `cases` without those that need a fixture, for a run that did not name
/// them (`--shard`, `--affected`, no `--case`): they run from the update
/// matrix, where the fixture is restored first.
pub fn without_fixture_cases(cases: Vec<&Case>) -> Vec<&Case> {
    let before = cases.len();
    let kept: Vec<&Case> = cases.into_iter().filter(|case| !needs_fixture(case)).collect();
    if kept.len() != before {
        eprintln!(
            "{} boot tests that update from an old release's fixture are left out; name them \
             with --case, after `./ob fixture release VERSION`",
            before - kept.len()
        );
    }
    kept
}

/// The releases whose fixtures `cases` boot, each once, in the order they
/// come.
pub fn releases_of(cases: &[&Case]) -> Vec<&'static str> {
    let mut releases = Vec::new();
    for release in cases.iter().filter_map(|case| release_of(case)) {
        if !releases.contains(&release) {
            releases.push(release);
        }
    }
    releases
}

/// Makes the release keys of this checkout those the fixtures of `cases`
/// trust, before anything is signed or built: see
/// [`release::adopt_keys`].
pub fn adopt_keys(ctx: &Ctx, cases: &[&Case]) -> Result<()> {
    let local = ctx.root.join(crate::release::KEYS_DIR);
    for version in releases_of(cases) {
        if release::adopt_keys(&ctx.target, version, &local)? == release::Adopted::Copied {
            step(&format!(
                "Using the release keys the fixture of {version} trusts, in {}",
                crate::release::KEYS_DIR
            ));
        }
    }
    Ok(())
}

/// The disk `case` boots, the fixture of its release, which is attached
/// as a copy-on-write overlay (QEMU's `snapshot=on`) and never written.
pub fn fixture_image(ctx: &Ctx, case: &Case) -> Result<std::path::PathBuf, String> {
    let version = release_of(case).ok_or_else(|| format!("{} boots no fixture", case.name))?;
    let image = release::image(&ctx.target, version);
    if !image.is_file() {
        return Err(format!(
            "{} is not made: run {}",
            image.display(),
            release::make_command(version)
        ));
    }
    Ok(image)
}

/// The FAT32 disk of `kib` KiB, labelled `label`, that holds the full
/// repository of this build ([`UPDATE_FULL`]).
pub const fn update_full_disk(kib: u64, label: &'static str) -> Disk {
    Disk { fat: UPDATE_FULL, ..fat_disk(kib, 32, label, Table::None, Bus::Virtio) }
}

/// Where the full repository is made, in the boot tests' folder `dir`.
pub fn full_dir(dir: &Path) -> std::path::PathBuf {
    dir.join("updates-full")
}

/// Makes the full repository, in [`full_dir`] of `dir`, if a case of
/// `cases` has the disk that holds it: the slot as
/// [`image::stage_system`] stages it, under the version of the test
/// update `good`, updating from the oldest fixture release, signed with
/// this machine's release keys, with every file in its store. A stage from
/// an old release's slot needs the files that differ from that slot's,
/// which are not known here.
pub fn make_full_repository(ctx: &Ctx, built: &Built, dir: &Path, cases: &[&Case]) -> Result<()> {
    if !cases.iter().any(|case| case.disks.iter().any(|disk| disk.fat == UPDATE_FULL)) {
        return Ok(());
    }
    let out = full_dir(dir);
    let staging = dir.join("updates-full-staging");
    for path in [&out, &staging] {
        if path.exists() {
            std::fs::remove_dir_all(path).context(|| format!("removing {}", path.display()))?;
        }
    }
    let tree = staging.join("slot");
    image::stage_system(ctx, &built.kernel, &built.programs, &tree)?;
    let version = test_update_version(UPDATE);
    let about = slots::about::About { version: version.clone(), flags: String::new() };
    let path = tree.join(image::SLOT_STARTUP).join(image::SLOT_ABOUT);
    std::fs::write(&path, about.encode()).context(|| format!("writing {}", path.display()))?;
    crate::release::make_as(ctx, &tree, &out, &version, FIXTURE_RELEASES[0])?;
    std::fs::remove_dir_all(&staging).context(|| format!("removing {}", staging.display()))?;
    let needed = repository_bytes(&out)?;
    if needed > FULL_KIB * 1024 {
        crate::bail!(
            "the full repository needs about {} MiB on its FAT disk, which holds {} MiB: raise \
             FULL_KIB in tools/xtask/src/test/upgrade.rs",
            needed >> 20,
            FULL_KIB >> 10
        );
    }
    Ok(())
}

/// The bytes a FAT32 disk needs for the files under `dir`.
fn repository_bytes(dir: &Path) -> Result<u64> {
    fn walk(dir: &Path, sizes: &mut Vec<u64>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), sizes)?;
            } else {
                sizes.push(entry.metadata()?.len());
            }
        }
        Ok(())
    }
    let mut sizes = Vec::new();
    walk(dir, &mut sizes).context(|| format!("reading {}", dir.display()))?;
    Ok(fat_bytes(&sizes))
}

/// The bytes a FAT32 disk needs for files of `sizes`: each whole
/// clusters, which `mkfs.fat` makes 4 KiB for a disk this size, a twentieth
/// more for the tables and long names, and 4 MiB for the rest.
pub fn fat_bytes(sizes: &[u64]) -> u64 {
    const CLUSTER: u64 = 4096;
    let data: u64 = sizes.iter().map(|size| size.div_ceil(CLUSTER).max(1) * CLUSTER).sum();
    data + data / 20 + (4 << 20)
}

/// Makes the [`UPDATE_FULL`] disk: FAT32 holding the repository
/// [`make_full_repository`] made, in the folder [`FULL`].
pub fn make_full_disk(disk: &Disk, path: &Path, repository: &Path) -> Result<(), String> {
    let mut mkfs = std::process::Command::new(fat_tool("mkfs.fat"));
    mkfs.args(["-F", "32", "-n", disk.label, "-i", "4f525448", "-C"])
        .arg(path)
        .arg(disk.kib.to_string());
    host_tool(&mut mkfs)?;
    let image = path.display().to_string();
    host_tool(
        std::process::Command::new("mcopy")
            .args(["-s", "-i", &image])
            .arg(repository)
            .arg(format!("::/{FULL}")),
    )?;
    Ok(())
}

/// What the machine is filled with, by the commands of the old release.
///
/// Each is a command typed at alice's prompt, and the lines the screen
/// then shows. Only commands the manual pages of Dukovo 0.15.0 describe:
/// *user*(8), *hostname*(5), *timedatectl*(1), *service*(8),
/// *health.toml*(5), *bundle*(1) and *settings.toml*(5).
fn fill_steps() -> Vec<(String, Vec<&'static str>)> {
    vec![
        // Root's: an account, the machine's name and zone, the health
        // check's wait made short, and a service that is off until it is
        // turned on.
        (
            as_root(
                "sh -c 'echo \"white rabbit\" | user add --full-name \"Bob Builder\" bob; \
                 hostname studio; timedatectl set-timezone Europe/Berlin; printf \"stable = \
                 10\\n\" > /system/settings/sysupdate/health.toml; service sshd enable; echo \
                 \"filled: $((6 * 7))\"'",
            ),
            vec!["filled: 42", "sshd is running"],
        ),
        // Alice's: files, a checksum of one, and a setting of her shell.
        (
            "cd; mkdir -p documents .config/nukesh; echo off with their heads > \
             documents/note.txt; cp /system/commands/bundle documents/big; sha256sum \
             $HOME/documents/big > documents/big.sha256; printf '[prompt]\\ngit-dirty = \
             false\\n' > .config/nukesh/settings.toml; echo \"made: $((6 * 7))\""
                .into(),
            vec!["made: 42"],
        ),
        // Her app: made, packed, installed and sealed, which counts its
        // runs in the data the system keeps for it.
        (
            "cd; bundle new org.example.keep keep > /dev/null; printf '#!/bin/sh\\nn=$(cat \
             $HOME/runs 2>/dev/null || echo 0)\\nn=$((n + 1))\\necho $n > $HOME/runs\\necho \
             \"keep runs: $n\"\\n' > keep/bin/keep; chmod 755 keep/bin/keep; bundle pack keep -o \
             keep.dkv > /dev/null; bundle install --yes keep.dkv | tail -1; rm -r keep keep.dkv"
                .into(),
            vec!["installed org.example.keep 0.1.0, for you"],
        ),
    ]
}

/// `command` as root: alice, the owner, is an administrator, and elevate
/// reads her password from its standard input with `-S`.
fn as_root(command: &str) -> String {
    format!("echo '{}' | elevate -S {command}", release::PASSWORD)
}

/// Types `command` at alice's prompt, waits for it to end, and checks that
/// the screen then shows each of `shown`.
fn run(vm: &mut Vm, command: &str, shown: &[&str]) -> Result<(), String> {
    answer_within(vm, "$", &format!("clear; {command}"), COMMAND_SECONDS)?;
    answer_within(vm, "$", "", COMMAND_SECONDS)?;
    vm.expect_screen(shown).map_err(|err| format!("`{command}`: {err}"))
}

/// Signs alice in at tty1, once the start shows its console.
fn sign_in(vm: &mut Vm) -> Result<(), String> {
    answer_within(vm, "Name:", "alice", SIGN_IN_SECONDS)?;
    answer_within(vm, "Password:", release::PASSWORD, SIGN_IN_SECONDS)
}

/// What the fill leaves, as each stage of the case checks it: the same
/// lines whatever release runs, but for the app's count of its runs,
/// `runs`, which each check adds one to.
fn check_steps(runs: u32) -> Vec<(String, Vec<String>)> {
    vec![
        (
            "cd; echo \"alice: $(user list | grep -c alice) bob: $(user list | grep -c bob)\"; \
             echo \"note: $(cat documents/note.txt)\"; sha256sum -c documents/big.sha256; echo \
             \"prompt: $(grep -c 'git-dirty = false' .config/nukesh/settings.toml)\""
                .into(),
            vec![
                "alice: 1 bob: 1".into(),
                "note: off with their heads".into(),
                "documents/big: OK".into(),
                "prompt: 1".into(),
            ],
        ),
        // Spark's control socket is root's on 0.15.0, so the service's
        // state is asked for as root.
        (
            format!(
                "echo \"host: $(hostname)\"; echo \"zone: $(readlink /system/settings/localtime \
                 | grep -c Europe/Berlin)\"; echo \"health: $(cat \
                 /system/settings/sysupdate/health.toml)\"; grep -q 'enabled = true' \
                 /system/settings/spark/services/sshd.toml && echo \"sshd file: on\"; {} > \
                 /dev/null && echo \"sshd: running\"",
                as_root("service sshd status")
            ),
            vec![
                "host: studio".into(),
                "zone: 1".into(),
                "health: stable = 10".into(),
                "sshd file: on".into(),
                "sshd: running".into(),
            ],
        ),
        (
            "echo \"app: $(bundle list | grep -c org.example.keep)\"; bundle run org.example.keep"
                .into(),
            vec!["app: 1".into(), format!("keep runs: {runs}")],
        ),
    ]
}

/// Checks each thing the fill made, as the `runs`th time the app runs.
fn check(vm: &mut Vm, runs: u32) -> Result<(), String> {
    for (command, shown) in check_steps(runs) {
        let shown: Vec<&str> = shown.iter().map(String::as_str).collect();
        run(vm, &command, &shown).map_err(|err| format!("check {runs}: {err}"))?;
    }
    Ok(())
}

/// Starts the machine's slot `slot`, and signs alice in.
fn start(vm: &mut Vm, slot: char) -> Result<(), String> {
    started(vm, slot)?;
    sign_in(vm)
}

/// Restarts the machine from alice's console.
fn restart(vm: &mut Vm) -> Result<(), String> {
    answer_within(vm, "$", &as_root("reboot"), COMMAND_SECONDS)?;
    vm.expect("spark: asked to restart the machine")
}

/// Stages this build from the disk of the full repository; on a failure,
/// says what root finds of that disk, and what the serial log says of it.
fn stage(vm: &mut Vm, version: &str) -> Result<(), String> {
    run(
        vm,
        &as_root(&format!("sysupdate stage {UPDATES}/{FULL}")),
        &[&format!("sysupdate: staged Dukovo {version} in slot B: it starts at the next restart")],
    )
    .map_err(|err| format!("{err}\n{}", what_is_there(vm)))
}

/// What root sees of the disk of the full repository, one line, and the
/// lines of the serial log that name its device or volumed.
fn what_is_there(vm: &mut Vm) -> String {
    let look = format!(
        "ls {UPDATES} {UPDATES}/{FULL} {UPDATES}/{FULL}/metadata; ls {UPDATES}/{FULL}/files | wc \
         -l; volumed --list; df {UPDATES}"
    );
    let command = as_root(&format!("sh -c '{look}'"));
    let screen = answer_within(vm, "$", &format!("clear; {command}"), COMMAND_SECONDS)
        .and_then(|()| answer_within(vm, "$", "", COMMAND_SECONDS))
        .and_then(|()| vm.screen());
    let seen = match screen {
        Ok(lines) => {
            let lines: Vec<&str> = lines.iter().map(|line| line.trim_end()).collect();
            format!("root sees: {}", lines.join(" | "))
        }
        Err(err) => format!("root: {err}"),
    };
    let log = vm.log();
    let said: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("vda") || line.contains("volumed") || line.contains("FAT"))
        .collect();
    format!("{seen}\nthe serial log of vda: {}", said.join(" | "))
}

/// Restarts into the staged update, and waits for it to prove itself.
fn try_update(vm: &mut Vm, version: &str, old: &str) -> Result<(), String> {
    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!("sysupdate: slot B (Dukovo {version}) is an update being tried"))?;
    vm.expect(&format!(
        "sysupdate: slot B (Dukovo {version}) proved itself: it is now the default, and slot A \
         (Dukovo {old}) the previous version"
    ))?;
    sign_in(vm)
}

/// Fills the machine, updates it, goes back, and updates again; on a
/// failure, says what the serial log tells of each start ([`story`]).
fn update_from_release(vm: &mut Vm, case: &Case) -> Result<(), String> {
    update_and_back(vm, case).map_err(|err| format!("{err}\n{}", story(&vm.log())))
}

/// The lines of the serial log `log` that tell the case's story: the slot
/// each start started and what its `/system` is, what sysupdate said,
/// each command run as root, sshd's lines, and each service that could
/// not start; the last [`STORY_LINES`] of them.
fn story(log: &str) -> String {
    const MARKS: [&str; 7] = [
        "started slot",
        "/system is",
        "sysupdate:",
        "could not start",
        "elevated on",
        "slot A",
        "sshd",
    ];
    let lines: Vec<&str> =
        log.lines().filter(|line| MARKS.iter().any(|mark| line.contains(mark))).collect();
    let told = &lines[lines.len().saturating_sub(STORY_LINES)..];
    let mut out = String::from("the serial log's starts, updates and elevated commands:");
    for line in told {
        out.push_str("\n  | ");
        out.push_str(line);
    }
    out
}

/// How many lines of the serial log [`story`] tells at most.
const STORY_LINES: usize = 150;

/// [`update_from_release`], but for what a failure says.
fn update_and_back(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let old = release_of(case).ok_or_else(|| format!("{} is no update-from case", case.name))?;
    let version = test_update_version(UPDATE);

    // The old release, filled.
    start(vm, 'a')?;
    for (command, shown) in fill_steps() {
        run(vm, &command, &shown)?;
    }
    check(vm, 1)?;

    // The update, tried and proved, with all of it kept.
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {UPDATES}"))?;
    stage(vm, &version)?;
    try_update(vm, &version, old)?;
    run(
        vm,
        &as_root("sysupdate status"),
        &[
            &format!("Running:  slot B, Dukovo {version}"),
            &format!("Slot A:   Dukovo {old}, the previous version"),
            "Next start: slot B",
        ],
    )?;
    check(vm, 2)?;
    run(
        vm,
        &as_root(&format!("sysupdate verify {UPDATES}/{FULL}")),
        &[&format!("sysupdate: slot B (Dukovo {version}) is the release its signed list names")],
    )?;

    // Back to the old release, keeping the settings as they are.
    run(
        vm,
        &as_root("sysupdate rollback --keep-settings"),
        &[&format!(
            "sysupdate: slot A (Dukovo {old}) starts from the next restart on; restart to go \
             back to it"
        )],
    )?;
    restart(vm)?;
    start(vm, 'a')?;
    vm.expect(&format!("sysupdate: this start of slot A (Dukovo {old}) is seen through"))?;
    run(
        vm,
        &as_root("sysupdate status"),
        &[&format!("Running:  slot A, Dukovo {old}"), "Next start: slot A"],
    )?;
    check(vm, 3)?;

    // The update again, over the slot the first one left.
    stage(vm, &version)?;
    try_update(vm, &version, old)?;
    check(vm, 4)?;
    answer_within(vm, "$", &as_root("poweroff"), COMMAND_SECONDS)?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_tells_the_starts_and_updates_of_the_serial_log() {
        let log = "[    1.2] spark: started slot B, @system.b\n\
                   [    1.3] timed: no server gave the time\n\
                   [    2.0] spark: could not start reportd: /system/commands/reportd: not_found\n\
                   [    9.1] authd: alice elevated on pts/0 (unlocked): process 9 runs sysupdate status\n";
        let told = story(log);
        assert!(told.contains("started slot B"), "{told}");
        assert!(told.contains("could not start reportd"), "{told}");
        assert!(told.contains("runs sysupdate status"), "{told}");
        assert!(!told.contains("timed:"), "{told}");
        let long = (0..STORY_LINES + 5)
            .map(|n| format!("sysupdate: line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let told = story(&long);
        assert_eq!(told.lines().count(), 1 + STORY_LINES);
        assert!(told.ends_with(&format!("sysupdate: line {}", STORY_LINES + 4)), "{told}");
    }

    fn named(name: &str) -> &'static Case {
        cases().iter().find(|case| case.name == name).unwrap()
    }

    #[test]
    fn a_pair_of_cases_is_made_for_each_release_under_both_firmware_types() {
        assert_eq!(cases().len(), 2 * FIXTURE_RELEASES.len());
        let uefi = named("update-from-0.15.0");
        let bios = named("update-from-0.15.0-bios");
        assert_eq!(uefi.firmware, Firmware::Uefi);
        assert_eq!(bios.firmware, Firmware::Bios);
        for case in [uefi, bios] {
            assert!(case.image == TestImage::Fixture);
            assert_eq!(case.media, Media::Disk);
            assert!(case.reboot);
            assert_eq!(case.disks.len(), 1);
            assert_eq!(case.disks[0].fat, UPDATE_FULL);
            assert!(case.about.contains("0.15.0"));
            assert!(needs_fixture(case));
        }
        assert_eq!(case_name("0.15.0", false), uefi.name);
        assert_eq!(case_name("0.15.0", true), bios.name);
    }

    #[test]
    fn a_case_says_which_release_it_boots() {
        assert_eq!(release_of(named("update-from-0.15.0")), Some("0.15.0"));
        assert_eq!(release_of(named("update-from-0.15.0-bios")), Some("0.15.0"));
        let stage = all_cases().find(|case| case.name == "update-stage").unwrap();
        assert_eq!(release_of(stage), None);
        assert!(!needs_fixture(stage));
    }

    #[test]
    fn the_cases_are_among_all_of_them_and_described_for_the_manual() {
        for case in cases() {
            assert!(all_cases().any(|other| std::ptr::eq(other, case)), "{}", case.name);
        }
        let described = described();
        assert_eq!(described.len(), cases().len());
        for (case, doc) in &described {
            assert!(!doc.is_empty() && !doc.starts_with('`'), "{}", case.name);
        }
        assert!(described[1].1.starts_with("As *update-from-0.15.0*, under a BIOS"));
        assert!(described[0].1.contains("FIXTURE_RELEASES"));
    }

    #[test]
    fn a_run_not_asking_for_them_leaves_the_fixture_cases_out() {
        let all: Vec<&Case> = all_cases().collect();
        assert!(all.iter().any(|case| needs_fixture(case)));
        let kept = without_fixture_cases(all.clone());
        assert_eq!(kept.len(), all.len() - cases().len());
        assert!(kept.iter().all(|case| !needs_fixture(case)));
    }

    #[test]
    fn each_release_is_named_once_for_the_keys() {
        let all: Vec<&Case> = all_cases().collect();
        assert_eq!(releases_of(&all), FIXTURE_RELEASES);
        let both: Vec<&Case> = cases().iter().collect();
        assert_eq!(releases_of(&both), ["0.15.0"]);
        let stage = all.iter().copied().find(|case| case.name == "update-stage").unwrap();
        assert!(releases_of(&[stage]).is_empty());
    }

    #[test]
    fn the_fixture_releases_are_releases_with_tags() {
        for release in FIXTURE_RELEASES {
            assert!(release::valid_version(release), "{release}");
        }
    }

    #[test]
    fn a_repository_needs_whole_clusters_and_room_for_the_tables() {
        assert_eq!(fat_bytes(&[]), 4 << 20);
        // Each file takes whole clusters, an empty one a cluster.
        let one = fat_bytes(&[1]);
        assert_eq!(one, (4 << 20) + 4096 + 4096 / 20);
        assert_eq!(fat_bytes(&[0]), one);
        assert!(fat_bytes(&[4097]) > fat_bytes(&[4096]));
        let many = vec![1; 1000];
        assert_eq!(fat_bytes(&many), (4 << 20) + 1000 * 4096 + 1000 * 4096 / 20);
    }

    #[test]
    fn the_repository_is_measured_by_its_files_in_every_folder() {
        let dir = std::env::temp_dir().join(format!("ob-upgrade-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("root.json"), vec![0u8; 5000]).unwrap();
        std::fs::write(dir.join("files/a"), vec![0u8; 10]).unwrap();
        assert_eq!(repository_bytes(&dir).unwrap(), fat_bytes(&[5000, 10]));
        assert!(repository_bytes(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_disk_is_a_fat32_virtio_disk_that_holds_the_repository() {
        let disk = &UPGRADE_DISKS[0];
        assert_eq!(disk.fat, UPDATE_FULL);
        assert_eq!(disk.kib, FULL_KIB);
        assert_eq!(disk.label, "UPDATES");
        assert_ne!(UPDATE_FULL, UPDATE_RELEASES);
        // 512 MiB is room for the root of one slot, with the tables.
        const { assert!(FULL_KIB * 1024 >= image::ROOT_BYTES) };
    }

    #[test]
    fn the_fill_uses_commands_the_old_release_has() {
        let fill: String = fill_steps().into_iter().map(|(command, _)| command).collect();
        for command in [
            "user add",
            "hostname studio",
            "timedatectl set-timezone",
            "service sshd enable",
            "bundle new",
            "bundle pack",
            "bundle install --yes",
            "elevate -S",
            "health.toml",
        ] {
            assert!(fill.contains(command), "{command}");
        }
        // Nothing of a later release: no web console, no catalog.
        assert!(!fill.contains("admin ") && !fill.contains("catalog"));
    }

    #[test]
    fn each_check_runs_the_app_once_more() {
        for runs in 1..=4 {
            let checks = check_steps(runs);
            let shown: Vec<&String> = checks.iter().flat_map(|(_, shown)| shown).collect();
            assert!(shown.contains(&&format!("keep runs: {runs}")));
        }
        // What is checked is what the fill made.
        let fill: String = fill_steps().into_iter().map(|(command, _)| command).collect();
        for needle in
            ["off with their heads", "big.sha256", "git-dirty = false", "org.example.keep"]
        {
            assert!(fill.contains(needle), "{needle}");
            let checked: String = check_steps(1).into_iter().map(|(command, _)| command).collect();
            assert!(checked.contains(needle) || needle.starts_with("off"), "{needle}");
        }
    }

    #[test]
    fn root_is_asked_through_elevate_with_alices_password() {
        assert_eq!(as_root("reboot"), format!("echo '{}' | elevate -S reboot", release::PASSWORD));
    }

    #[test]
    fn the_cases_are_in_covers_and_times() {
        let covers = include_str!("covers.txt");
        let times = include_str!("times.txt");
        for case in cases() {
            assert!(
                covers.lines().any(|line| line.split(' ').next() == Some(case.name)),
                "covers.txt names {}",
                case.name
            );
            assert!(
                times.lines().any(|line| line.split(' ').next() == Some(case.name)),
                "times.txt names {}",
                case.name
            );
        }
    }

    #[test]
    fn the_update_matrix_restores_each_fixture_before_ob_runs() {
        let workflow = include_str!("../../../../.github/workflows/update-matrix.yml");
        for release in FIXTURE_RELEASES {
            let key = format!("fixture-{release}-v1");
            assert!(workflow.contains(&key), "update-matrix.yml has no cache {key}");
            for path in [
                format!("target/fixtures/{release}/installed.img"),
                format!("target/fixtures/{release}/release-keys"),
            ] {
                assert!(workflow.contains(&path), "update-matrix.yml caches no {path}");
            }
            assert!(workflow.contains(&format!("./ob fixture release {release}")));
        }
    }
}
