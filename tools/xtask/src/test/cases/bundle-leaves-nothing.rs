// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Removing a bundle leaves only its data (ADR 0591). Alice installs
//! ripgrep for herself and runs it once, which she allows always, and
//! installs beacon, a test bundle with a service, for everyone, which
//! starts and writes its data. Before the installs and after each removal
//! root lists every path of the system's folders and of /shared, with its
//! type, mode, owner, group and a file's size, and Alice's own app-data and
//! applications folders. Removed without --purge, both leave only beacon's
//! data in /shared/services, what she allowed ripgrep, which runs with her
//! access and so has no data folder of its own, and the records that
//! outlive a bundle; beacon's account is gone. Installed again, beacon's
//! account has the number it had and starts on its data, and ripgrep runs
//! without asking. Removed with --purge, nothing differs from before the
//! installs but the records that outlive a bundle.

use super::super::*;

/// The disk of beacon's bundle: its label.
const SERVICES: &str = "SERVICES";

/// The disk of the catalog's bundles: its label.
const BUNDLES: &str = "BUNDLES";

/// Where Spark mounts the second and third virtio disks.
const SERVICES_VOLUME: &str = "/volumes/vdb";
const BUNDLES_VOLUME: &str = "/volumes/vdc";

/// The bundles' files.
const BEACON_FILE: &str = "org.example.beacon-1.0.0.dkv";
const RIPGREP_FILE: &str = "io.github.burntsushi.ripgrep-15.2.0.dkv";

/// beacon's data folder, and the number of its account.
const DATA: &str = "/shared/services/org.example.beacon/beacon";
const BEACON_NUMBER: &str = "100";

/// Dukovo's root on a copy of the test image, beacon's bundle and the
/// catalog's.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    service_files_disk(8 * 1024, SERVICES),
    bundle_files_disk(128 * 1024, BUNDLES),
];

/// The test.
pub const CASE: Case = Case {
    name: "bundle-leaves-nothing",
    about: "removing a bundle leaves only its data (ADR 0591): a path-by-path record of the system before ripgrep and beacon are installed and after each is removed, with and without --purge, differs only by the data that is kept and the records that outlive a bundle, and beacon's account, removed with it, has its number again when it is installed again",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: leaves_nothing,
    disks: DISKS,
    second: None,
};

/// The script root runs to record the system's paths into the file it is
/// given, in Alice's home, which the record does not list: the paths of
/// the system's settings and state, of /applications and of /shared, each
/// a subvolume of its own, and what runs in /system/live, two levels down,
/// where each service has a folder of its own. A folder's size is left
/// out: it counts the names in it, which the record lists itself.
const RECORD: &[&str] = &[
    "find /applications /shared /system/settings /system/state \\( -type d -printf \
     \"%p %y %m %U %G\\n\" \\) -o -printf \"%p %y %m %U %G %s\\n\" | sort > \"/users/alice/$1\"",
    "find /system/live -maxdepth 2 -printf \"%p %y %U %G\\n\" | sort >> \"/users/alice/$1\"",
    "find /users/alice/app-data /users/alice/applications \\( -type d -printf \
     \"%p %y %m %U %G\\n\" \\) -o -printf \"%p %y %m %U %G %s\\n\" 2> /dev/null | sort >> \
     \"/users/alice/$1\"",
];

/// What outlives every bundle, as the patterns of a path in the record,
/// each with why. A difference that none of them names is a leftover.
const OUTLIVES: &[&str] = &[
    // bundled writes each of these whole at every change and makes them at
    // its first one. The bindings of an ID to its publisher's key, for
    // everyone and for each person, and to its catalog, outlive the bundle
    // (ADR 0383), so that a later bundle of the ID from another key is
    // refused; the history says what bundled did. The rest are empty once
    // nothing is installed, which `bundle list` shows.
    "/system/state/bundled/(installed|bound|developer-keys|catalog-bound|previous|held|automatic|defaults|history)",
    // One file for each service's file bundled wrote, in a folder that
    // stays.
    "/system/state/bundled/services",
    // What bundled read of the catalogs, which an install from a file
    // reads too: the catalogs', not a bundle's.
    "/system/state/bundled/catalogs(/.*)?",
    // ADR 0220: a number once used is never given to another account.
    "/system/state/authd/retired",
    // The folder every service bundle's data is in, made by the first
    // install of one.
    "/shared/services",
    // What the system's services record whatever is installed: the log,
    // which keeps what was said while the bundle was there, the audit log
    // in it, and the others' state, which changes as the system runs.
    "/system/state/(log|sign-ins|starts|metrics|hardware|reports|crash|timed|timerd|netd|sshd|consoled)(/.*)?",
    // Alice's own folders, which her first install and first run make in
    // her home: no one bundle's.
    "/users/alice/(app-data|app-data/.snapshots|applications|applications/commands)",
];

/// What a removal without --purge keeps besides [`OUTLIVES`]: beacon's
/// data and what Alice allowed ripgrep, which stays as its data would.
/// ripgrep runs with her access, so it has no folder in her app-data.
/// --purge takes both.
const DATA_KEPT: &[&str] =
    &["/shared/services/org.example.beacon(/.*)?", "/system/state/grants/1000"];

/// The extended pattern for the paths of `lists`, as grep takes it after
/// the status of a line of diff.
fn paths(lists: &[&[&str]]) -> String {
    let each: Vec<String> =
        lists.iter().flat_map(|list| list.iter()).map(|path| format!("({path})")).collect();
    format!("^[<>] ({})( |$)", each.join("|"))
}

/// Records the system in Alice's file `name`, as root.
fn record(vm: &mut Vm, name: &str, first: bool) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!("clear; elevate sh /users/alice/record.sh {name}; echo \"recorded {name} $?\""),
    )?;
    if first {
        answer_prompt(vm, &format!("{name}:"), "queen of hearts")?;
    }
    answer_prompt_within(vm, "$", "", 120)?;
    vm.expect_screen(&[&format!("recorded {name} 0")])
}

/// Compares two records, which may differ by the paths `allowed` names
/// only, and fails with the lines that do not.
fn compare(vm: &mut Vm, before: &str, after: &str, allowed: &[&[&str]]) -> Result<(), String> {
    let pattern = paths(allowed);
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd; diff {before} {after} | grep '^[<>]' | grep -v -E '{pattern}' > left; cat \
             left; echo \"{after} leftovers: $(wc -l < left)\""
        ),
    )?;
    vm.expect_screen(&[&format!("{after} leftovers: 0")])
}

/// That the record `name` holds a path of each place a bundle leaves
/// something in: one that missed a place would find nothing left there.
fn sees_each_place(vm: &mut Vm, name: &str) -> Result<(), String> {
    let places = "/system/state/bundled/installed|/system/settings/spark/services|\
                  /shared/services/org.example.beacon|/system/state/grants/1000";
    answer_prompt(
        vm,
        "$",
        &format!("clear; cd; echo \"sees $(grep -c -E '^({places}) ' {name})\""),
    )?;
    vm.expect_screen(&["sees 4"])
}

/// That no bundle is listed and Spark knows no service of beacon's.
fn nothing_installed(vm: &mut Vm) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        "clear; echo \"listed $(bundle list --json | grep -c '\"id\"') $(elevate service list | grep \
         -c beacon)\"",
    )?;
    vm.expect_screen(&["listed 0 0"])
}

fn leaves_nothing(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {SERVICES_VOLUME}"))?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdc at {BUNDLES_VOLUME}"))?;
    let script = RECORD.iter().map(|line| format!("'{line}'")).collect::<Vec<_>>().join(" ");
    answer_prompt(
        vm,
        "$",
        &format!("clear; printf '%s\\n' {script} > record.sh; echo \"written $?\""),
    )?;
    vm.expect_screen(&["written 0"])?;
    record(vm, "before", true)?;

    // ripgrep for Alice, run once and allowed always; beacon for everyone,
    // which writes its data.
    answer_prompt(
        vm,
        "$",
        &format!("clear; bundle install --yes {BUNDLES_VOLUME}/{RIPGREP_FILE} | tail -1"),
    )?;
    vm.expect_screen(&["installed io.github.burntsushi.ripgrep 15.2.0, for you"])?;
    answer_prompt(
        vm,
        "$",
        "clear; printf 'alpha\\nneedle 42\\nomega\\n' > notes.txt; rg -n needle notes.txt; echo \
         \"rg $?\"",
    )?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_screen(&["2:needle 42", "rg 0"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate bundle install --for-everyone --yes {SERVICES_VOLUME}/{BEACON_FILE} \
             | tail -3"
        ),
    )?;
    vm.expect_screen(&[
        "installed org.example.beacon 1.0.0, for everyone; its service beacon runs",
    ])?;
    vm.expect_anywhere("beacon: beacon 1.0.0 runs as beacon, start 2")?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; elevate ls -ldn {DATA}; echo \"number $(id -u beacon)\""),
    )?;
    vm.expect_screen(&[
        &format!("{BEACON_NUMBER} {BEACON_NUMBER}"),
        &format!("number {BEACON_NUMBER}"),
    ])?;

    // Removed without --purge: the data stays, the account does not.
    answer_prompt(
        vm,
        "$",
        "clear; bundle remove --keep-data io.github.burntsushi.ripgrep | head -1; elevate bundle \
         remove --for-everyone --keep-data org.example.beacon | tr ';' '\\n'; id beacon; echo \
         \"removed $?\"",
    )?;
    vm.expect_screen(&[
        "removed io.github.burntsushi.ripgrep 15.2.0",
        "removed org.example.beacon 1.0.0",
        "its services' data stays in /shared/services/org.example.beacon",
        "id: 'beacon': no such user",
    ])?;
    nothing_installed(vm)?;
    record(vm, "kept", false)?;
    sees_each_place(vm, "kept")?;
    compare(vm, "before", "kept", &[OUTLIVES, DATA_KEPT])?;

    // Installed again: the account has its number, so the data is its own,
    // and ripgrep remembers what Alice allowed.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate bundle install --for-everyone --yes {SERVICES_VOLUME}/{BEACON_FILE} \
             | tail -3; bundle install --yes {BUNDLES_VOLUME}/{RIPGREP_FILE} | tail -1"
        ),
    )?;
    vm.expect_screen(&[
        "installed org.example.beacon 1.0.0, for everyone; its service beacon runs",
        "installed io.github.burntsushi.ripgrep 15.2.0, for you",
    ])?;
    vm.expect_anywhere("beacon: beacon 1.0.0 runs as beacon, start 3")?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate ls -ldn {DATA}; echo \"number $(id -u beacon)\"; rg -c a notes.txt; \
             echo \"again $?\""
        ),
    )?;
    vm.expect_screen(&[
        &format!("{BEACON_NUMBER} {BEACON_NUMBER}"),
        &format!("number {BEACON_NUMBER}"),
        "again 0",
    ])?;
    vm.expect_screen_without("[warning]")?;

    // Purged: nothing is left but the records that outlive a bundle.
    answer_prompt(
        vm,
        "$",
        "clear; bundle remove --purge io.github.burntsushi.ripgrep | tail -1; elevate bundle \
         remove --for-everyone --purge org.example.beacon; echo \"purged $?\"",
    )?;
    vm.expect_screen(&[
        "removed org.example.beacon 1.0.0, with its services' accounts and data",
        "purged 0",
    ])?;
    nothing_installed(vm)?;
    record(vm, "purged", false)?;
    compare(vm, "before", "purged", &[OUTLIVES])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
