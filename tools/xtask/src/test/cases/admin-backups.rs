// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Snapshots and backups of the web console, Dukovo Admin: the
//! snapshots' six actions and the backups' nine (ADR 0461, ADR 0463, ADR
//! 0561), on a Dukovo root of Btrfs with an exFAT disk to back up to at
//! /volumes/vdb. alice, the owner, is an administrator; lab-reader may use
//! the console and is none.
//!
//! A snapshot and what came after it. alice has /users/lab-reader hold
//! three files and a private folder of her own, and takes the snapshot
//! lab-before of /users with snapshots.take: it is in snap list, and a file
//! cannot be made in it. Then three files of lab-reader change: one is
//! changed, one deleted and one added.
//!
//! Browsing. lab-reader's snapshots.browse of his home says changed for the
//! changed file, deleted for the deleted one and same for the one left, and
//! has no entry for the one added after. alice's snapshots.browse-all of her
//! own folder is what the same command prints as root. lab-reader is refused
//! her private folder with the system's words, and browse-all with ADM-122;
//! a snapshot the place does not have is BAK-105.
//!
//! Restoring. lab-reader's snapshots.restore with the label lab puts the
//! copy of each file beside it, notes (lab).txt for notes.txt, and a cmp of
//! each copy with the snapshot's file finds no difference, the file beside
//! it as it was; a second restore of the same is BAK-106. alice's
//! snapshots.restore-all makes a copy in lab-reader's folder that he owns.
//!
//! A backup place. backups.add to a folder of the second disk answers with
//! the place's recovery key, and an S3 address is refused by the check, with
//! the words that say it is added at a terminal. Admind's log, authd's
//! audit lines, logd's records, the journal's rows and the tasks have no
//! byte of the key; backups.key gives the same key, as a secret answer is
//! given, to alice and no one else.
//!
//! A round, a check and a restore are tasks. backups.run shows a step of
//! the machine's places, and backups.list is what backup list --json
//! prints, with the place's last backup done. backups.verify reads the
//! backups through and restores the newest of each place to compare.
//! backups.restore of /users makes the snapshot restored from the place, and
//! snapshots.list has it, marked restored; the list of every place has the
//! settings, the update's snapshots and /users in the service, with the
//! time its newest was made.
//!
//! backups.remove takes the place away, with its two files, and
//! backups.show for it is BAK-101. A reader is ADM-122 and a locked
//! administrator ADM-106 for every action that needs an administrator.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;

/// The account of the reader, whom alice makes: he may use the console, and
/// is no administrator.
pub(super) const READER: &str = "lab-reader";

/// The reader's password.
pub(super) const READER_PASSWORD: &str = "tweedledum";

/// Dukovo's root on a copy of the test image, so that /users, /shared,
/// /applications and /system/settings are its subvolumes, and an exFAT
/// disk to back up to, which Spark mounts at /volumes/vdb.
pub(super) const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    exfat_disk(128 * 1024, 4096, "LAB", Bus::Virtio),
];

/// The test.
pub const CASE: Case = Case {
    name: "admin-backups",
    about: "The web console's Snapshots and backups: a snapshot lab-before of /users taken by snapshots.take, then lab-reader's files changed, deleted and added; his snapshots.browse says changed, deleted and same for them and nothing of the added one, browse-all of alice's folder is what the command prints as root, and her private folder and browse-all are refused to him with the system's words and ADM-122, a snapshot there is none of BAK-105; snapshots.restore with a label puts a copy beside each file, equal to the snapshot's by cmp, with the file beside it as it was, a second restore is BAK-106, and restore-all makes the copy for lab-reader's folder his own; backups.add to a folder of the second disk answers with the recovery key, an S3 address is refused with the words for a terminal, and no byte of the key is in admind's log, authd's audit, logd, the journal or the tasks, while backups.key gives the same key; backups.run shows its steps as a task, backups.list is backup list --json, backups.verify and backups.restore of /users finish as tasks and snapshots.list has the restored snapshot, marked restored, and for every place the settings, the update's snapshots and /users in the service's view with the time of its newest; backups.remove takes the place away and backups.show for it is BAK-101; a reader is ADM-122 and a locked administrator ADM-106 for what needs an administrator (ADR 0461, ADR 0463, ADR 0561)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark mounts the disk and starts the services,
    // admind among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_backups,
    disks: DISKS,
    second: None,
};

/// What an administrator does first, as root: the reader's files, and a
/// private folder of alice's.
const SETUP: &str = "elevate sh -c 'cd /users/lab-reader && echo one > keep.txt && echo two > \
                     change.txt && echo three > gone.txt && chown lab-reader keep.txt change.txt \
                     gone.txt && mkdir -p /users/alice/lab-private && echo hidden > \
                     /users/alice/lab-private/note.txt && chown -R alice /users/alice/lab-private \
                     && chmod 700 /users/alice/lab-private'";

/// What the reader's files go through after the snapshot, as root: one is
/// changed, one deleted, one added.
const CHANGES: &str = "elevate sh -c 'cd /users/lab-reader && echo two changed > change.txt && rm \
                       gone.txt && echo four > new.txt && chown lab-reader new.txt'";

fn admin_backups(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        reader: Browser::default(),
    };

    // alice's password once for this pane, then the reader, who is in group
    // console and no administrator.
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {READER_PASSWORD} | user add {READER}; user set --yes \
             --console {READER}'"
        ),
    )?;
    vm.expect(&format!("authd: changed the account {READER}"))?;
    vm.expect_screen(&[&format!("Made {READER}")])?;
    test.sign_in()?;

    a_snapshot_and_what_came_after(vm, &mut test)?;
    browsing(vm, &mut test)?;
    restoring(vm, &mut test)?;
    let key = a_place_added(vm, &mut test)?;
    a_round_a_check_and_a_restore(vm, &mut test)?;
    the_place_removed(vm, &mut test)?;
    what_needs_an_administrator(&mut test)?;
    a_key_is_kept_nowhere(vm, &mut test, &key)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Who asks the console.
#[derive(Clone, Copy)]
pub(super) enum Who {
    Alice,
    Reader,
}

/// What the test keeps from one step to the next.
pub(super) struct Test {
    pub(super) console: Console,
    pub(super) pane: Pane,
    pub(super) alice: Browser,
    pub(super) reader: Browser,
}

impl Test {
    /// Signs alice and the reader in, as the console's page does, each with
    /// a session of its own.
    pub(super) fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        self.reader = Browser::default();
        self.console.sign_in(&mut self.reader, READER, READER_PASSWORD)?;
        self.console.read_session(&mut self.reader)?;
        Ok(())
    }

    /// A read of `path` for `who`, whatever the answer.
    pub(super) fn get(&mut self, who: Who, path: &str) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Reader => &mut self.reader,
        };
        console.get(browser, path)
    }

    /// What `path` says to `who`, which must be 200.
    pub(super) fn read(&mut self, who: Who, path: &str) -> Result<Value, String> {
        self.get(who, path)?.must(path, 200)?.json()
    }

    /// A change, or a read that is a POST, of `path` with `body` for `who`,
    /// whatever the answer.
    pub(super) fn post(&mut self, who: Who, path: &str, body: &Value) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Reader => &mut self.reader,
        };
        console.post(browser, path, body)
    }

    /// Unlocks alice's session with her password, if it is locked.
    pub(super) fn unlocked(&mut self) -> Result<(), String> {
        let session =
            self.console.get(&mut self.alice, "/api/1/session")?.must("her session", 200)?;
        if session.json()?["unlock_seconds"].as_u64().is_some_and(|left| left > 120) {
            return Ok(());
        }
        let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
        self.console.post(&mut self.alice, "/api/1/unlock", &unlock)?.must("her unlocking", 200)?;
        Ok(())
    }

    /// Locks alice's session, as the page's lock button does.
    pub(super) fn locked(&mut self) -> Result<(), String> {
        self.console.post(&mut self.alice, "/api/1/lock", &json!({}))?.must("her locking", 200)?;
        Ok(())
    }

    /// A change as alice, unlocked: 200, and done.
    pub(super) fn change(&mut self, path: &str, body: &Value) -> Result<Value, String> {
        self.unlocked()?;
        let answer = self.post(Who::Alice, path, body)?.must(path, 200)?.json()?;
        if answer["done"] != json!(true) {
            return Err(format!("{path} was answered {answer}, which is not done"));
        }
        Ok(answer)
    }

    /// A task as alice, unlocked: 202 with its number, then the task once
    /// it has ended, which must be done, within `seconds`; and the most
    /// advanced step the test saw of it.
    pub(super) fn task(&mut self, path: &str, body: &Value, seconds: u64) -> Result<(Value, i64), String> {
        self.unlocked()?;
        let reply = self.post(Who::Alice, path, body)?.must(path, 202)?;
        let id = reply.json()?["task"].as_i64().ok_or("no task number")?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut most = 0;
        loop {
            let task = self.read(Who::Alice, &format!("/api/1/tasks/{id}"))?;
            most = most.max(task["step"].as_i64().unwrap_or(0));
            if !matches!(task["state"].as_str(), Some("waiting" | "running")) {
                if task["state"] != "done" {
                    return Err(format!("the task of {path} ended as {task}, not done"));
                }
                return Ok((task, most));
            }
            if Instant::now() >= deadline {
                return Err(format!("the task of {path} is {task} after {seconds} s"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// What `command` prints in the pane, as JSON.
    pub(super) fn printed(&self, vm: &mut Vm, command: &str) -> Result<Value, String> {
        let text = self.pane.output(vm, command)?;
        serde_json::from_str(text.trim())
            .map_err(|err| format!("`{command}` printed {text:?}, which is not JSON: {err}"))
    }

    /// The status `command` ends with in the pane, which must be `wanted`.
    pub(super) fn ends_with(&self, vm: &mut Vm, command: &str, wanted: i32) -> Result<(), String> {
        let status = self.pane.status(vm, command)?;
        if status != wanted {
            return Err(format!("`{command}` ended with status {status}, not {wanted}"));
        }
        Ok(())
    }
}

/// The entry of `shown`, the answer of a browse, called `name`.
fn entry<'a>(shown: &'a Value, name: &str) -> Option<&'a Value> {
    shown["entries"].as_array()?.iter().find(|entry| entry["name"] == name)
}

/// The recovery key in the lines `backup add` printed, if it printed one:
/// the word that begins as an age secret key does.
pub(super) fn key_in(lines: &Value) -> Option<String> {
    lines
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .flat_map(str::split_whitespace)
        .find(|word| word.starts_with("AGE-SECRET-KEY-1"))
        .map(String::from)
}

/// The query of a browse of `folder` of `place` in `snapshot`.
fn browse_path(root: &str, place: &str, snapshot: &str, folder: Option<&str>) -> String {
    let mut path = format!("{root}?place={place}&snapshot={snapshot}");
    if let Some(folder) = folder {
        path.push_str(&format!("&folder={folder}"));
    }
    path
}

/// The snapshot lab-before, taken as alice, and the changes after it.
fn a_snapshot_and_what_came_after(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.ends_with(vm, SETUP, 0)?;
    test.change("/api/1/snapshots/take", &json!({"place": "/users", "name": "lab-before"}))?;
    // It is in snap list, and read-only: nothing can be made in it.
    let listed = test.pane.output(vm, "snap list /users")?;
    if !listed.contains("lab-before") {
        return Err(format!("snap list /users has no lab-before: {listed:?}"));
    }
    if test.pane.status(vm, "elevate touch /snapshots/users/lab-before/lab-probe")? == 0 {
        return Err("a file was made in the snapshot lab-before, which is read-only".into());
    }
    // The route lists it with its place, as the command's own JSON does.
    let list = test.read(Who::Reader, "/api/1/snapshots?place=/users")?;
    let named = list["snapshots"]
        .as_array()
        .is_some_and(|all| all.iter().any(|s| s["name"] == "lab-before" && s["named"] == true));
    if list["place"] != json!("/users") || !named {
        return Err(format!("the list of the snapshots of /users is {list}"));
    }
    // Taking it again with the name used is the command's own refusal.
    let again = test.post(
        Who::Alice,
        "/api/1/snapshots/take",
        &json!({"place": "/users", "name": "lab-before"}),
    )?;
    if again.status == 200 {
        return Err("a second snapshot called lab-before was made".into());
    }
    // Three of the reader's files after it: changed, deleted and added.
    test.ends_with(vm, CHANGES, 0)
}

/// What the reader and alice see of the snapshot.
fn browsing(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    let home = "/users/lab-reader";
    let path = browse_path("/api/1/snapshots/browse", "/users", "lab-before", Some(home));
    let shown = test.read(Who::Reader, &path)?;
    if shown["place"] != json!("/users")
        || shown["snapshot"] != json!("lab-before")
        || shown["folder"] != json!(home)
        || shown["more"] != json!(false)
    {
        return Err(format!("the browse of {home} is {shown}"));
    }
    for (name, then) in [("keep.txt", "same"), ("change.txt", "changed"), ("gone.txt", "deleted")] {
        let found =
            entry(&shown, name).ok_or_else(|| format!("the browse has no {name}: {shown}"))?;
        if found["then"] != json!(then) || found["kind"] != json!("file") {
            return Err(format!("{name} is {found} in the browse, then is {then}"));
        }
    }
    if entry(&shown, "new.txt").is_some() {
        return Err(format!("the browse has new.txt, which came after the snapshot: {shown}"));
    }
    // The top of the place without a folder: the person's own folders, and
    // the folder given is the place's top.
    let top = test
        .read(Who::Reader, &browse_path("/api/1/snapshots/browse", "/users", "lab-before", None))?;
    if top["folder"] != json!("/users") {
        return Err(format!("the browse of the place's top says {top}"));
    }
    // alice's browse-all of her own folder is what root's command prints.
    test.unlocked()?;
    let owner = "/users/alice";
    let path = browse_path("/api/1/snapshots/browse/all", "/users", "lab-before", Some(owner));
    let all = test.read(Who::Alice, &path)?;
    let printed =
        test.printed(vm, &format!("elevate snap browse --json -- /users lab-before {owner}"))?;
    if all != printed {
        return Err(format!("browse-all is {all}, and the command printed {printed}"));
    }
    if entry(&all, "lab-private").is_none_or(|folder| folder["kind"] != json!("folder")) {
        return Err(format!("alice's folder in the snapshot has no lab-private: {all}"));
    }
    // What the reader may not: her private folder, with the system's words,
    // and the administrator's read.
    let private = browse_path(
        "/api/1/snapshots/browse",
        "/users",
        "lab-before",
        Some("/users/alice/lab-private"),
    );
    let refused = test.get(Who::Reader, &private)?.refused("her private folder", 422, "ADM-123")?;
    if !refused.detail().contains("Permission denied") {
        return Err(format!("her private folder is refused with {:?}", refused.detail()));
    }
    test.get(
        Who::Reader,
        &browse_path("/api/1/snapshots/browse/all", "/users", "lab-before", None),
    )?
    .refused("the reader's browse-all", 403, "ADM-122")?;
    // A snapshot the place has none of, and a folder outside the place.
    test.get(
        Who::Reader,
        &browse_path("/api/1/snapshots/browse", "/users", "lab-never", Some(home)),
    )?
    .refused("a snapshot that is not there", 422, "BAK-105")?;
    test.get(
        Who::Reader,
        &browse_path("/api/1/snapshots/browse", "/users", "lab-before", Some("/shared")),
    )?
    .refused("a folder outside the place", 400, "ADM-125")?;
    Ok(())
}

/// The restores, as the reader and as an administrator.
fn restoring(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    let snapshot = "/snapshots/users/lab-before/lab-reader";
    let body = json!({
        "snapshot": "lab-before",
        "label": "lab",
        "paths": "/users/lab-reader/change.txt\n/users/lab-reader/gone.txt"
    });
    let answer =
        test.post(Who::Reader, "/api/1/snapshots/restore", &body)?.must("his restore", 200)?;
    if answer.json()?["done"] != json!(true) {
        return Err(format!("his restore is {}", answer.body));
    }
    // A copy beside each, equal to the snapshot's, and the file beside it as
    // it was.
    for (copy, was) in [("change (lab).txt", "change.txt"), ("gone (lab).txt", "gone.txt")] {
        test.ends_with(vm, &format!("elevate cmp '/users/lab-reader/{copy}' {snapshot}/{was}"), 0)?;
    }
    let beside = test.pane.output(vm, "elevate cat /users/lab-reader/change.txt")?;
    if beside.trim() != "two changed" {
        return Err(format!("change.txt beside its copy is {beside:?}, not as it was"));
    }
    test.ends_with(vm, "elevate test -e /users/lab-reader/gone.txt", 1)?;
    // A name that is taken is BAK-106, and the copy as it was.
    test.post(Who::Reader, "/api/1/snapshots/restore", &body)?.refused(
        "his second restore",
        422,
        "BAK-106",
    )?;
    test.ends_with(
        vm,
        &format!("elevate cmp '/users/lab-reader/change (lab).txt' {snapshot}/change.txt"),
        0,
    )?;
    // A label that is a path, and no path at all, are refused at the check.
    let slash =
        json!({"snapshot": "lab-before", "label": "a/b", "paths": "/users/lab-reader/keep.txt"});
    test.post(Who::Reader, "/api/1/snapshots/restore", &slash)?.refused(
        "a label with a slash",
        400,
        "ADM-125",
    )?;
    let relative = json!({"snapshot": "lab-before", "label": "lab", "paths": "keep.txt"});
    test.post(Who::Reader, "/api/1/snapshots/restore", &relative)?.refused(
        "a relative path",
        400,
        "ADM-125",
    )?;
    // alice's restore-all copies as root into the reader's folder, and the
    // copy is his: the owner of the folder it is made in.
    let all = json!({
        "snapshot": "lab-before",
        "label": "all",
        "paths": "/users/lab-reader/keep.txt"
    });
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/snapshots/restore/all", &all)?.must("restore-all", 200)?;
    test.ends_with(
        vm,
        &format!("elevate cmp '/users/lab-reader/keep (all).txt' {snapshot}/keep.txt"),
        0,
    )?;
    let owner = test.pane.output(vm, "elevate stat -c %U '/users/lab-reader/keep (all).txt'")?;
    if owner.trim() != READER {
        return Err(format!("the copy restore-all made for {READER} is owned by {owner:?}"));
    }
    Ok(())
}

/// The backup place `lab` added to a folder of the second disk; its
/// recovery key.
fn a_place_added(vm: &mut Vm, test: &mut Test) -> Result<String, String> {
    elevated(vm)?;
    let add = json!({"name": "lab", "to": "/volumes/vdb", "every": "hand", "verify": "never"});
    // An S3 place is for a terminal for now, and a name that is the address
    // of the machine's key is none.
    let s3 = json!({"name": "lab", "to": "s3://bucket/x", "every": "hand", "verify": "never"});
    test.unlocked()?;
    let refused =
        test.post(Who::Alice, "/api/1/backups/add", &s3)?.refused("an S3 place", 400, "ADM-125")?;
    if !refused.detail().contains("S3 places are added at a terminal for now") {
        return Err(format!("an S3 place is refused with {:?}", refused.detail()));
    }
    let reserved =
        json!({"name": "ssh-key", "to": "/volumes/vdb", "every": "hand", "verify": "never"});
    test.post(Who::Alice, "/api/1/backups/add", &reserved)?.refused(
        "the name ssh-key",
        400,
        "ADM-125",
    )?;
    test.post(Who::Reader, "/api/1/backups/add", &add)?.refused(
        "the reader's add",
        403,
        "ADM-122",
    )?;
    // Nothing was made by those: the place is added now.
    let answer = test.change("/api/1/backups/add", &add)?;
    let key = key_in(&answer["output"])
        .ok_or_else(|| format!("the answer of backups.add holds no recovery key: {answer}"))?;
    if answer["changed"] != json!(true) {
        return Err(format!("backups.add says it changed nothing: {answer}"));
    }
    // Both of its files are there, the key's alone to root.
    test.ends_with(vm, "elevate test -e /system/settings/backup/lab.toml", 0)?;
    test.ends_with(vm, "elevate test -e /system/settings/backup/lab.key", 0)?;
    // A name in use is the command's own sentence, ADM-123.
    test.post(Who::Alice, "/api/1/backups/add", &add)?.refused("a name in use", 422, "ADM-123")?;
    // backups.key gives the same key, and is a POST: its address is no read.
    test.get(Who::Alice, "/api/1/backups/lab/key")?.refused("the key as a GET", 405, "ADM-117")?;
    let given = test
        .post(Who::Alice, "/api/1/backups/lab/key", &json!({}))?
        .must("backups.key", 200)?
        .json()?;
    if given["key"] != json!(key) || given["name"] != json!("lab") {
        return Err("backups.key does not give the key backups.add made".into());
    }
    Ok(key)
}

/// A round, a check and a restore of the place, each a task.
fn a_round_a_check_and_a_restore(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let (task, most) = test.task("/api/1/backups/lab/run", &json!({}), 600)?;
    let steps = task["steps"].as_i64().unwrap_or(0);
    if steps < 1 || most < 1 || task["action"] != json!("backups.run") {
        return Err(format!("the round shows no `step N of M`: {task}, most step {most}"));
    }
    // The list is what backup list --json prints, with the round done.
    let listed = test.read(Who::Alice, "/api/1/backups")?;
    let printed = test.printed(vm, "backup list --json")?;
    if listed != printed {
        return Err(format!("backups.list is {listed}, and the command printed {printed}"));
    }
    let place = listed["places"]
        .as_array()
        .and_then(|all| all.iter().find(|place| place["name"] == "lab"))
        .ok_or_else(|| format!("the list has no place lab: {listed}"))?;
    if place["state"]["done_ms"].as_u64().is_none() || place["state"]["failed"] != json!(null) {
        return Err(format!("the round of lab is not done in the list: {place}"));
    }
    // What the place holds, by place, as an administrator reads it.
    let held = test.read(Who::Alice, "/api/1/backups/lab")?;
    if held["name"] != json!("lab") || held["places"].as_array().is_none_or(Vec::is_empty) {
        return Err(format!("backups.show of lab holds {held}"));
    }
    let (_, checked) = test.task("/api/1/backups/lab/verify", &json!({}), 900)?;
    if checked < 1 {
        return Err("the check of lab shows no step of its restore".into());
    }
    let after = test.read(Who::Alice, "/api/1/backups")?;
    let verified = after["places"]
        .as_array()
        .and_then(|all| all.iter().find(|place| place["name"] == "lab"))
        .is_some_and(|place| place["state"]["verified_ms"].as_u64().is_some());
    if !verified {
        return Err(format!("the list does not say lab was checked: {after}"));
    }
    // The newest backup of /users, received beside the place's snapshots.
    test.task("/api/1/backups/lab/restore", &json!({"place": "/users"}), 600)?;
    let list = test.read(Who::Alice, "/api/1/snapshots?place=/users")?;
    let restored = list["snapshots"].as_array().is_some_and(|all| {
        all.iter().any(|s| {
            s["name"].as_str().is_some_and(|name| name.starts_with("restored-"))
                && s["restored"] == true
        })
    });
    if !restored {
        return Err(format!("snapshots.list has no restored- snapshot of /users: {list}"));
    }
    // The list of every place: the settings, the update's snapshots, and
    // /users in the service's view with the time its newest was made.
    let all = test.read(Who::Alice, "/api/1/snapshots")?;
    let users = all["places"]
        .as_array()
        .and_then(|places| places.iter().find(|place| place["place"] == "/users"));
    let users_ok = users.is_some_and(|place| {
        place["in_service"] == true
            && place["newest_made"].as_str().is_some_and(|made| made.ends_with('Z'))
    });
    if !users_ok || !all["settings"]["daily"].is_u64() || !all["update"].is_array() {
        return Err(format!("snapshots.list of every place is {all}"));
    }
    // It is a snapshot to restore files from, and the copies of its files
    // are as the files were when the place was backed up.
    let name = list["snapshots"]
        .as_array()
        .and_then(|all| {
            all.iter().filter_map(|s| s["name"].as_str()).find(|n| n.starts_with("restored-"))
        })
        .ok_or("no restored snapshot")?
        .to_string();
    let path = browse_path("/api/1/snapshots/browse", "/users", &name, Some("/users/lab-reader"));
    test.read(Who::Reader, &path)?;
    Ok(())
}

/// The place taken away, with its files; what is asked of it then.
fn the_place_removed(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    test.change("/api/1/backups/lab/remove", &json!({}))?;
    test.ends_with(vm, "elevate test -e /system/settings/backup/lab.toml", 1)?;
    test.ends_with(vm, "elevate test -e /system/settings/backup/lab.key", 1)?;
    // The backups stay where they were put.
    test.ends_with(vm, "elevate ls /volumes/vdb/dukovo-backup", 0)?;
    test.unlocked()?;
    test.get(Who::Alice, "/api/1/backups/lab")?.refused(
        "a place that was removed",
        422,
        "BAK-101",
    )?;
    let list = test.read(Who::Alice, "/api/1/backups")?;
    if list["places"].as_array().is_some_and(|all| all.iter().any(|place| place["name"] == "lab")) {
        return Err(format!("the list has lab after backups.remove: {list}"));
    }
    Ok(())
}

/// Every action that needs an administrator, each with arguments that
/// would work if the asker were one, is ADM-122 for the reader and ADM-106
/// for alice with her session locked, before anything runs. What a person
/// reads of their own is open to both.
fn what_needs_an_administrator(test: &mut Test) -> Result<(), String> {
    // Whether it is a POST, its address, and its arguments.
    let asks = [
        (false, "/api/1/snapshots/browse/all?place=/users&snapshot=lab-before", json!({})),
        (true, "/api/1/snapshots/take", json!({"place": "/users", "name": "lab-refused"})),
        (
            true,
            "/api/1/snapshots/restore/all",
            json!({"snapshot": "lab-before", "label": "x", "paths": "/users/a"}),
        ),
        (false, "/api/1/backups/ssh-key", json!({})),
        (false, "/api/1/backups/lab", json!({})),
        (true, "/api/1/backups/lab/key", json!({})),
        (true, "/api/1/backups/lab/run", json!({})),
        (true, "/api/1/backups/lab/verify", json!({})),
        (true, "/api/1/backups/lab/restore", json!({"place": "/users"})),
        (true, "/api/1/backups/lab/remove", json!({})),
        (
            true,
            "/api/1/backups/add",
            json!({"name": "lab2", "to": "/volumes/vdb", "every": "hand", "verify": "never"}),
        ),
    ];
    test.locked()?;
    for (post, path, body) in &asks {
        for (who, id) in [(Who::Reader, "ADM-122"), (Who::Alice, "ADM-106")] {
            let reply = if *post { test.post(who, path, body)? } else { test.get(who, path)? };
            reply.refused(path, 403, id)?;
        }
    }
    test.read(Who::Reader, "/api/1/backups")?;
    test.read(Who::Alice, "/api/1/backups")?;
    test.read(Who::Reader, "/api/1/snapshots?place=/users")?;
    Ok(())
}

/// What the machine keeps that a secret must not be in: admind's log and
/// authd's audit lines, which the serial console has, logd's records, the
/// journal's rows and the tasks, each with its name. A secret that was never
/// typed in the guest cannot be in any of them from the test.
pub(super) fn places_of_the_machine(
    vm: &mut Vm,
    test: &mut Test,
) -> Result<Vec<(&'static str, String)>, String> {
    elevated(vm)?;
    test.unlocked()?;
    let mut places = vec![("the serial console's log, admind's and authd's lines", vm.log())];
    let logd = test.pane.output(vm, "elevate log --all --output json --lines 5000")?;
    places.push(("logd's records", logd));
    let changes = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let mut journal = changes.to_string();
    for row in changes["changes"].as_array().into_iter().flatten() {
        if let Some(id) = row["id"].as_i64() {
            journal.push_str(&test.read(Who::Alice, &format!("/api/1/changes/{id}"))?.to_string());
        }
    }
    places.push(("the journal's rows", journal));
    places.push(("the tasks", test.read(Who::Alice, "/api/1/tasks")?.to_string()));
    Ok(places)
}

/// No byte of the recovery key `key` is in what the machine keeps
/// ([`places_of_the_machine`]).
pub(super) fn a_key_is_kept_nowhere(vm: &mut Vm, test: &mut Test, key: &str) -> Result<(), String> {
    for (what, text) in places_of_the_machine(vm, test)? {
        if text.contains(key) {
            return Err(format!("the recovery key is in {what}"));
        }
        if text.contains("AGE-SECRET-KEY-1") {
            return Err(format!("a secret key is in {what}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_the_word_that_begins_as_a_secret_key() {
        let lines = json!([
            "Added lab: /volumes/vdb/dukovo-backup/atlas, backed up by hand.",
            "",
            "Its recovery key, which reads its backups on any machine:",
            "",
            "    AGE-SECRET-KEY-1QQQQ",
            "",
            "Keep it somewhere other than this machine."
        ]);
        assert_eq!(key_in(&lines).as_deref(), Some("AGE-SECRET-KEY-1QQQQ"));
        assert_eq!(key_in(&json!(["no key here"])), None);
        assert_eq!(key_in(&json!({})), None);
    }

    #[test]
    fn a_browse_is_asked_with_its_place_snapshot_and_folder() {
        assert_eq!(
            browse_path("/api/1/snapshots/browse", "/users", "lab-before", None),
            "/api/1/snapshots/browse?place=/users&snapshot=lab-before"
        );
        assert_eq!(
            browse_path("/api/1/snapshots/browse/all", "/users", "s", Some("/users/a")),
            "/api/1/snapshots/browse/all?place=/users&snapshot=s&folder=/users/a"
        );
    }

    #[test]
    fn an_entry_is_found_by_its_name() {
        let shown =
            json!({"entries": [{"name": "a", "then": "same"}, {"name": "b", "then": "deleted"}]});
        assert_eq!(entry(&shown, "b").map(|e| &e["then"]), Some(&json!("deleted")));
        assert!(entry(&shown, "c").is_none());
        assert!(entry(&json!({}), "a").is_none());
    }
}
