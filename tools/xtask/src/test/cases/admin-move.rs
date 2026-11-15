// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Move to new hardware page of the web console, Dukovo Admin: the four
//! actions move.find, move.plan, move.stage and move.cancel (ADR 0464, ADR
//! 0561), on a Dukovo root of Btrfs with an exFAT disk to back up to at
//! /volumes/vdb. alice, the owner, is an administrator; lab-reader may use
//! the console and is none. The design has two machines here, A that is
//! backed up and B that the backups are moved to. This test has one that
//! starts twice: the first start is A, which has two network cards, the
//! second is B, which has the second card of A alone and has lost the first.
//! The machine's own disks stay as they are, so B has A's files before the
//! restore too; what the restore must bring back is what the first start
//! changes after the backup.
//!
//! The machine A. It is named lab-machine, and its card with the MAC address
//! 52:54:00:aa:00:01 has the fixed address 192.168.7.50/24, which network(8)
//! writes to network(5)'s file. The place lab is added to the exFAT disk
//! with backups.add, backed up by backups.run, and its key file is copied
//! to the disk as lab-machine-recovery.key, the way a person keeps it. A
//! second place, other, gives a key that is valid and is not the backups'.
//!
//! Finding and planning. move.find lists the backups of lab-machine on the
//! disk with the time of the newest, and the key file, and is what backup
//! find --json prints as root. move.plan with the key typed, which goes in
//! the body and is read from the command's input, answers the plan: the
//! machine, its record's format and time, its accounts, its places, the two
//! cards it had, and the card with an address of its own with its address.
//! The same plan comes with the key as a file, with the time given, and as
//! backup restore --machine --plan --json prints it; the bytes free on the
//! root are the one thing that changes between two asks. A card choice
//! gives the card of this machine its address, or asks DHCP. The other
//! place's key, typed or as a file, is BAK-103. move.plan is a POST: a GET
//! of its address is ADM-117. A key given as a file and typed, neither, a
//! source that is no place or folder, a time that is not one and a card
//! that is none are ADM-125. A reader is ADM-122 and an administrator with
//! the session locked ADM-106 for each of the four.
//!
//! Staging. move.stage with the key typed and cards that map the card of
//! A to the card that B keeps is a task that shows its steps, and leaves
//! the restore staged: the plan says so, and the place the restore waits
//! in is there. move.cancel takes it away. Staged again with the key as a
//! file, a third move.stage ends as a task that failed with BAK-104. No
//! byte of either key is then in admind's log, authd's audit lines, logd's
//! records, the journal's rows or the tasks.
//!
//! Then the machine is changed past knowing: it is named lab-other, its
//! card is asked to use DHCP again and lab-reader is removed. The power
//! button ends the first start.
//!
//! The second start. It has only the card with the MAC address
//! 52:54:00:bb:00:01. Spark puts the restore in place before any service
//! starts, and the machine is lab-machine again: lab-reader signs in with
//! the password it had, and network show says the card has 192.168.7.50/24
//! as a fixed address from a section that names its own MAC address, with
//! no address from DHCP. Nothing is staged any more. The console is not
//! asked at the new address: QEMU forwards its port to the card on its own
//! network, which has no way to the card's new one, so that what the test
//! holds to is that admind started, and that the card has the address.
//! The power button ends the second start.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{Browser, Console, Pane, elevated};
use super::super::*;
use super::admin_backups::{
    DISKS, READER, READER_PASSWORD, Test, Who, a_key_is_kept_nowhere, key_in,
};

/// The card that B keeps, which every start has: on QEMU's own network, so
/// that the host reaches the console through it.
const NEW_CARD: &str = "52:54:00:bb:00:01";

/// The card that A has and B has lost: a network of its own, on which the
/// fixed address of A is.
const OLD_CARD: &str = "52:54:00:aa:00:01";

/// The fixed address of A's card.
const FIXED: &str = "192.168.7.50/24";

/// The machine's name when it is backed up.
const MACHINE: &str = "lab-machine";

/// The folder of the machine's backups on the second disk, which move.find
/// gives as the source.
const SOURCE: &str = "/volumes/vdb/dukovo-backup/lab-machine";

/// The key file the place's key is copied to, on the same disk.
const KEY_FILE: &str = "/volumes/vdb/lab-machine-recovery.key";

/// The test.
pub const CASE: Case = Case {
    name: "admin-move",
    about: "The web console's Move to new hardware, as one machine that starts twice: a machine lab-machine with two network cards, one with a fixed address, backed up with backups.add and backups.run to an exFAT disk, whose key file is copied there; move.find lists the backups and the key file as backup find --json does, move.plan with the key typed gives the plan of accounts, places, cards and hardware, equal with the key as a file and to backup restore --machine --plan --json, and a key that is not the backups' is BAK-103; move.stage with a card choice is a task with steps and leaves the restore staged, move.cancel takes it away, a stage again and a third one is BAK-104; no byte of any key is in admind's log, authd's audit, logd, the journal or the tasks; a reader is ADM-122 and a locked administrator ADM-106 for all four; the machine is changed and restarted with only the second card, and Spark puts it back: its name, its account that signs in, and the fixed address on the card that is left (ADR 0464, ADR 0561)",
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
    // The card on QEMU's own network is the machine's first and its console's
    // way in, at both starts. The card of A is a second one, on a network of
    // its own, and the second start does without it.
    qemu: &[
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0,mac=52:54:00:bb:00:01",
        FIRST_BOOT_ONLY,
        "-netdev",
        "user,id=old,net=192.168.7.0/24",
        "-device",
        "virtio-net-pci,netdev=old,mac=52:54:00:aa:00:01",
    ],
    script: before_the_move,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: "lockdep", before: nothing_between, script: moved }),
};

/// The first start: the machine A, its backups, and what the console says
/// of them, until the restore is staged and the machine changed.
fn before_the_move(vm: &mut Vm, case: &Case) -> Result<(), String> {
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

    let (key, other) = the_machine_backed_up(vm, &mut test)?;
    the_backups_found(vm, &mut test)?;
    the_backups_planned(vm, &mut test, &key, &other)?;
    what_needs_an_administrator(&mut test)?;
    the_move_staged_and_cancelled(vm, &mut test, &key)?;
    a_key_is_kept_nowhere(vm, &mut test, &key)?;
    the_machine_changed(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second start: the machine B, which has the second card alone. Spark
/// puts the staged restore in place before any service starts.
fn moved(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("spark: restoring the machine lab-machine from its backup of ")?;
    vm.expect_anywhere("spark: the machine is as it was at ")?;
    vm.expect_anywhere("admind: answers at ")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    // The account lab-reader was removed after the backup, and has come back
    // with the password it had.
    answer_prompt(vm, "Name:", READER)?;
    answer_prompt(vm, "Password:", READER_PASSWORD)?;
    vm.expect_anywhere(&format!("authd: {READER} signed in on pts/"))?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; hostname; sleep 3; network show {NEW_CARD}; sh -c 'echo \"staged $(ls \
             /snapshots/machine-restore | grep -c staged)\"'; true"
        ),
    )?;
    // The name the machine had; the card that is left has the address of
    // the card that was lost, from a section that names its own MAC address,
    // and asks no DHCP.
    vm.expect_screen(&[
        MACHINE,
        &format!("{NEW_CARD}  link up"),
        &format!("{FIXED} (fixed)"),
        &format!("[{NEW_CARD}] fixed"),
        "staged 0",
    ])?;
    vm.expect_screen_without("(dhcp)")?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The arguments of move.plan and move.stage, an object, with the source
/// and the key as a file unless it names them.
fn by_file(mut args: Value) -> Value {
    for (name, value) in [("source", SOURCE), ("key_file", KEY_FILE)] {
        if args.get(name).is_none() {
            args[name] = json!(value);
        }
    }
    args
}

/// A plan without the bytes free on the root, which change between two asks
/// as the machine writes.
fn steady(mut plan: Value) -> Value {
    if let Some(plan) = plan.as_object_mut() {
        plan.remove("free");
    }
    plan
}

/// The MAC addresses of the cards in `cards`, as the plan lists them.
fn macs(cards: &Value) -> Vec<String> {
    cards
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|card| card["mac"].as_str())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether `plan`, from move.plan, says what the backups of the machine
/// hold, which is the same whatever the key and the card choice are.
fn the_plan_says_the_machine(plan: &Value) -> Result<(), String> {
    let wrong = |what: &str| Err(format!("the plan of the backups {what}: {plan}"));
    if plan["machine"] != json!(MACHINE) {
        return wrong("is not of lab-machine");
    }
    if plan["record_format"] != json!(2) || plan["staged"] != json!(false) {
        return wrong("is not of a record of format 2 with nothing staged");
    }
    // The record's time is a backup's time, to the second.
    if plan["at"].as_str().is_none_or(|at| at.len() != 17) || plan["at_ms"].as_u64().is_none() {
        return wrong("has no time to the second");
    }
    let accounts: Vec<&str> =
        plan["accounts"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !accounts.contains(&"alice") || !accounts.contains(&READER) {
        return wrong("does not list alice and lab-reader as accounts");
    }
    let users = |places: &Vec<Value>| places.iter().any(|place| place["path"] == "/users");
    if !plan["places"].as_array().is_some_and(users) {
        return wrong("has no place /users");
    }
    if plan["fits"] != json!(true) || plan["needed"].as_u64().is_none() {
        return wrong("does not say it fits the root");
    }
    // The hardware then had both cards. This one still has both at the first
    // start, the second being the card that B loses.
    for side in ["then", "now"] {
        let cards = macs(&plan["hardware"][side]["cards"]);
        if ![OLD_CARD, NEW_CARD].iter().all(|mac| cards.iter().any(|have| have == mac)) {
            return wrong(&format!("lists the cards {cards:?} of the machine {side}"));
        }
    }
    // The one card with addresses of its own is A's second one.
    let cards = plan["cards"].as_array().map_or(0, Vec::len);
    let old = &plan["cards"][0]["old"];
    if cards != 1
        || old["mac"].as_str().map(str::to_ascii_lowercase).as_deref() != Some(OLD_CARD)
        || old["addresses"] != json!([FIXED])
    {
        return wrong("does not have the one card with the fixed address");
    }
    Ok(())
}

/// Whether the old card of `plan` goes to the card `new`, when that is
/// given, and is `choice`.
fn the_card_goes(plan: &Value, new: Option<&str>, choice: &str) -> Result<(), String> {
    let pair = &plan["cards"][0];
    let to = pair["new"]["mac"].as_str().map(str::to_ascii_lowercase);
    if pair["choice"] != json!(choice) || new.is_some_and(|mac| to.as_deref() != Some(mac)) {
        let went = &pair["choice"];
        return Err(format!("the card of A goes to {to:?} as {went}, not {new:?} as {choice}"));
    }
    Ok(())
}

/// A: its name, its card with a fixed address, its place on the second disk
/// backed up, the key file on that disk, and a second place whose key is
/// not the backups'. The key of lab, and the key of the other place.
fn the_machine_backed_up(vm: &mut Vm, test: &mut Test) -> Result<(String, String), String> {
    elevated(vm)?;
    test.ends_with(vm, &format!("elevate hostname {MACHINE}"), 0)?;
    test.ends_with(vm, &format!("elevate network set {OLD_CARD} fixed {FIXED}"), 0)?;
    // The settings are read from the file, which network set has written.
    let shown = test.pane.output(vm, &format!("network show {OLD_CARD}"))?;
    if !shown.contains(&format!("[{OLD_CARD}] fixed, addresses {FIXED}")) {
        return Err(format!("network show says of the card of A: {shown:?}"));
    }
    test.unlocked()?;
    let add = json!({"name": "lab", "to": "/volumes/vdb", "every": "hand", "verify": "never"});
    let answer = test.change("/api/1/backups/add", &add)?;
    let key = key_in(&answer["output"])
        .ok_or_else(|| format!("the answer of backups.add holds no recovery key: {answer}"))?;
    let (_, steps) = test.task("/api/1/backups/lab/run", &json!({}), 600)?;
    if steps < 1 {
        return Err("the round of lab shows no step".into());
    }
    // The key file as a person keeps it, on the disk the backups are on.
    test.ends_with(vm, &format!("elevate cp /system/settings/backup/lab.key {KEY_FILE}"), 0)?;
    // Another place, whose key opens its own backups and none of lab's.
    test.ends_with(vm, "elevate mkdir /tmp/other", 0)?;
    let add = json!({"name": "other", "to": "/tmp/other", "every": "hand", "verify": "never"});
    let answer = test.change("/api/1/backups/add", &add)?;
    let other = key_in(&answer["output"])
        .ok_or_else(|| format!("the answer of backups.add holds no key for other: {answer}"))?;
    if other == key {
        return Err("the two places have the same recovery key".into());
    }
    Ok((key, other))
}

/// move.find lists the backups of the machine and its key file, as backup
/// find --json does as root.
fn the_backups_found(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    test.unlocked()?;
    let found = test.read(Who::Alice, "/api/1/move/sources")?;
    let source = found["sources"]
        .as_array()
        .and_then(|all| all.iter().find(|source| source["source"] == json!(SOURCE)))
        .ok_or_else(|| format!("move.find lists no backups at {SOURCE}: {found}"))?;
    if source["machine"] != json!(MACHINE)
        || source["newest"].as_str().is_none_or(|time| time.len() != 17)
        || source["newest_ms"].as_u64().is_none()
    {
        return Err(format!("move.find says of the backups of {MACHINE}: {source}"));
    }
    let key = found["keys"]
        .as_array()
        .and_then(|all| all.iter().find(|key| key["path"] == json!(KEY_FILE)))
        .ok_or_else(|| format!("move.find lists no key file at {KEY_FILE}: {found}"))?;
    let public = key["public"].as_str().unwrap_or_default();
    if key["for"] != json!("lab") || !public.starts_with("age1") {
        return Err(format!("move.find says of the key file: {key}"));
    }
    let printed = test.printed(vm, "elevate backup find --json")?;
    if found != printed {
        return Err(format!("move.find is {found}, and the command printed {printed}"));
    }
    // A key is never read out of the file: nothing of it is in the answer.
    if found.to_string().contains("AGE-SECRET-KEY-1") {
        return Err("move.find holds a recovery key".into());
    }
    Ok(())
}

/// The plan of what the backups bring across, by each way the key and the
/// cards are given, and what it refuses.
fn the_backups_planned(
    vm: &mut Vm,
    test: &mut Test,
    key: &str,
    other: &str,
) -> Result<(), String> {
    elevated(vm)?;
    let ask = |test: &mut Test, args: &Value| -> Result<Value, String> {
        test.unlocked()?;
        test.post(Who::Alice, "/api/1/move/plan", args)?.must("move.plan", 200)?.json()
    };
    // The key typed goes in the body, and the command reads it on its input.
    let typed = ask(test, &json!({"source": SOURCE, "key": key}))?;
    the_plan_says_the_machine(&typed)?;
    // The key as a file is the same plan, and what the command prints.
    let from_file = ask(test, &by_file(json!({})))?;
    if steady(typed.clone()) != steady(from_file.clone()) {
        return Err(format!("the plan with the key typed is {typed}, as a file {from_file}"));
    }
    let plan = "backup restore --machine --plan --json --key";
    let printed = test.printed(vm, &format!("elevate {plan} {KEY_FILE} {SOURCE}"))?;
    if steady(from_file.clone()) != steady(printed.clone()) {
        return Err(format!("move.plan is {from_file}, and the command printed {printed}"));
    }
    // The time of the record is one the backups can be asked for.
    let at = ask(test, &by_file(json!({"at": typed["at"]})))?;
    if steady(at.clone()) != steady(typed.clone()) {
        return Err(format!("the plan at its own time is {at}, not {typed}"));
    }
    // The card of A goes to the card that B keeps, or asks DHCP.
    let chosen = ask(test, &by_file(json!({"cards": format!("{OLD_CARD}={NEW_CARD}")})))?;
    the_plan_says_the_machine(&chosen)?;
    the_card_goes(&chosen, Some(NEW_CARD), "fixed")?;
    let automatic = ask(test, &by_file(json!({"cards": format!("{OLD_CARD}=automatic")})))?;
    the_card_goes(&automatic, None, "automatic")?;

    // A key that opens other backups is BAK-103, typed and as a file, and
    // the refusal says nothing of it.
    test.unlocked()?;
    let wrong = [
        json!({"source": SOURCE, "key": other}),
        json!({"source": SOURCE, "key_file": "/system/settings/backup/other.key"}),
    ];
    for args in &wrong {
        let refused = test.post(Who::Alice, "/api/1/move/plan", args)?.refused(
            "a plan with another place's key",
            422,
            "BAK-103",
        )?;
        if refused.body.contains("AGE-SECRET-KEY-1") {
            return Err(format!("the refusal of a wrong key holds a key: {}", refused.body));
        }
    }
    // The plan reads a key, so it is a POST: its address is no read.
    test.get(Who::Alice, "/api/1/move/plan")?.refused("the plan as a GET", 405, "ADM-117")?;
    // What the check of the arguments refuses, before anything runs.
    let refused = [
        ("a key as a file and typed", by_file(json!({"key": key}))),
        ("no key", json!({"source": SOURCE})),
        ("a source that is no place or folder", by_file(json!({"source": "x y"}))),
        ("a relative folder", by_file(json!({"source": "backups/lab"}))),
        ("a time that is not one", by_file(json!({"at": "2026-11-14"}))),
        ("a card choice that is none", by_file(json!({"cards": "nonsense"}))),
        ("a card whose new one is no MAC", by_file(json!({"cards": format!("{OLD_CARD}=net0")}))),
    ];
    for (what, args) in &refused {
        let reply = test.post(Who::Alice, "/api/1/move/plan", args)?;
        reply.refused(what, 400, "ADM-125")?;
    }
    Ok(())
}

/// Every action that needs an administrator, each with arguments that would
/// work if the asker were one, is ADM-122 for the reader and ADM-106 for
/// alice with her session locked, before anything runs.
fn what_needs_an_administrator(test: &mut Test) -> Result<(), String> {
    // Whether it is a POST, its address, and its arguments.
    let asks = [
        (false, "/api/1/move/sources", json!({})),
        (true, "/api/1/move/plan", by_file(json!({}))),
        (true, "/api/1/move/stage", by_file(json!({}))),
        (true, "/api/1/move/cancel", json!({})),
    ];
    test.locked()?;
    for (post, path, body) in &asks {
        for (who, id) in [(Who::Reader, "ADM-122"), (Who::Alice, "ADM-106")] {
            let reply = if *post { test.post(who, path, body)? } else { test.get(who, path)? };
            reply.refused(path, 403, id)?;
        }
    }
    Ok(())
}

/// How many times the place the restore waits in is listed in the machine's
/// folder of restores: one while a restore is staged, none otherwise.
fn staged_places(test: &Test, vm: &mut Vm) -> Result<String, String> {
    let said = test
        .pane
        .output(vm, "elevate ls /snapshots/machine-restore 2> /dev/null | grep -c staged")?;
    Ok(said.trim().to_string())
}

/// Takes a task of `path` with `body` to its end, which must be that it
/// failed, within `seconds`: the task.
fn a_task_that_fails(
    test: &mut Test,
    path: &str,
    body: &Value,
    seconds: u64,
) -> Result<Value, String> {
    test.unlocked()?;
    let reply = test.post(Who::Alice, path, body)?.must(path, 202)?;
    let id = reply.json()?["task"].as_i64().ok_or("no task number")?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let task = test.read(Who::Alice, &format!("/api/1/tasks/{id}"))?;
        if !matches!(task["state"].as_str(), Some("waiting" | "running")) {
            if task["state"] != "failed" {
                return Err(format!("the task of {path} ended as {task}, not failed"));
            }
            return Ok(task);
        }
        if Instant::now() >= deadline {
            return Err(format!("the task of {path} is {task} after {seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// move.stage as a task, move.cancel, move.stage again, and a third that
/// is refused.
fn the_move_staged_and_cancelled(vm: &mut Vm, test: &mut Test, key: &str) -> Result<(), String> {
    elevated(vm)?;
    let staged = |test: &mut Test| -> Result<Value, String> {
        test.unlocked()?;
        let args = by_file(json!({}));
        test.post(Who::Alice, "/api/1/move/plan", &args)?.must("move.plan", 200)?.json()
    };
    if staged_places(test, vm)? != "0" || staged(test)?["staged"] != json!(false) {
        return Err("a restore is staged before the test staged one".into());
    }
    // A card of A goes to the card that B keeps; the key is typed.
    let cards = format!("{OLD_CARD}={NEW_CARD}");
    let typed = json!({"source": SOURCE, "key": key, "cards": cards});
    let (task, most) = test.task("/api/1/move/stage", &typed, 900)?;
    if task["steps"].as_i64().unwrap_or(0) < 1 || most < 1 || task["action"] != json!("move.stage")
    {
        return Err(format!("the stage shows no `step N of M`: {task}, most step {most}"));
    }
    // The task keeps its arguments with the key as nothing, and its command
    // reads the key on its input.
    let argv: Vec<&str> =
        task["argv"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if task["args"]["key"] != json!("(secret)")
        || !argv.windows(2).any(|pair| pair == ["--key", "-"])
        || !argv.contains(&cards.as_str())
        || argv.last() != Some(&SOURCE)
    {
        return Err(format!("the task of the stage keeps {} and {}", task["args"], task["argv"]));
    }
    if staged(test)?["staged"] != json!(true) || staged_places(test, vm)? != "1" {
        return Err("the restore is not staged after move.stage".into());
    }
    // Cancelled, nothing is staged.
    test.change("/api/1/move/cancel", &json!({}))?;
    if staged(test)?["staged"] != json!(false) || staged_places(test, vm)? != "0" {
        return Err("the restore is staged after move.cancel".into());
    }
    // Staged again, with the key as a file this time.
    let (_, again) = test.task("/api/1/move/stage", &by_file(json!({"cards": cards})), 900)?;
    if again < 1 {
        return Err("the second stage shows no step".into());
    }
    // A third is refused by the command: the first is as it was.
    let refused = a_task_that_fails(test, "/api/1/move/stage", &typed, 300)?;
    let said = refused["note"].as_str().unwrap_or_default();
    if !said.contains("BAK-104") {
        return Err(format!("a third stage failed with {said:?}, not BAK-104: {refused}"));
    }
    if staged(test)?["staged"] != json!(true) || staged_places(test, vm)? != "1" {
        return Err("the refused stage left the restore not staged".into());
    }
    Ok(())
}

/// The machine changed past knowing, as the restore must put back: its name,
/// the card of A back to DHCP, and the reader removed.
fn the_machine_changed(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    test.ends_with(vm, "elevate hostname lab-other", 0)?;
    test.ends_with(vm, &format!("elevate network set {OLD_CARD} dhcp"), 0)?;
    test.ends_with(vm, &format!("elevate user remove --delete-home --yes {READER}"), 0)?;
    let name = test.pane.output(vm, "hostname")?;
    let people = test.pane.output(vm, "elevate user list")?;
    if name.trim() != "lab-other" || people.contains(READER) {
        return Err(format!("the machine is {name:?} with the accounts {people:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two cards of A, as the plan lists a machine's.
    fn both_cards() -> Value {
        json!([{"name": "net0", "mac": NEW_CARD}, {"name": "net1", "mac": OLD_CARD}])
    }

    /// A plan as move.plan answers it for the machine of this test.
    fn a_plan() -> Value {
        json!({
            "machine": MACHINE,
            "at": "2026-11-14-013000",
            "at_ms": 1_763_083_800_000u64,
            "record_format": 2,
            "staged": false,
            "accounts": ["alice", READER],
            "apps": [],
            "places": [{"path": "/users", "folder": "users", "bytes": 4096}],
            "needed": 4096,
            "free": 1_000_000,
            "fits": true,
            "hardware": {
                "then": {"cards": both_cards()},
                "now": {"cards": both_cards()},
            },
            "cards": [{
                "old": {"name": "net1", "mac": OLD_CARD, "addresses": [FIXED], "router": null},
                "new": {"name": "net0", "mac": NEW_CARD},
                "choice": "fixed",
            }],
            "firewall_on_missing": [],
        })
    }

    #[test]
    fn a_plan_of_the_machine_is_told_by_its_name_accounts_places_and_cards() {
        assert_eq!(the_plan_says_the_machine(&a_plan()), Ok(()));
        for (what, mend) in [
            ("machine", json!("other")),
            ("record_format", json!(1)),
            ("staged", json!(true)),
            ("at", json!("2026-11-14-0130")),
            ("accounts", json!(["alice"])),
            ("places", json!([])),
            ("fits", json!(false)),
            ("cards", json!([])),
        ] {
            let mut plan = a_plan();
            plan[what] = mend;
            assert!(the_plan_says_the_machine(&plan).is_err(), "{what}");
        }
        let mut plan = a_plan();
        plan["hardware"]["now"]["cards"] = json!([{"name": "net0", "mac": NEW_CARD}]);
        assert!(the_plan_says_the_machine(&plan).is_err());
        let mut plan = a_plan();
        plan["cards"][0]["old"]["addresses"] = json!(["10.0.2.15/24"]);
        assert!(the_plan_says_the_machine(&plan).is_err());
    }

    #[test]
    fn the_card_of_a_goes_where_the_plan_says_and_as_it_says() {
        assert_eq!(the_card_goes(&a_plan(), Some(NEW_CARD), "fixed"), Ok(()));
        assert_eq!(the_card_goes(&a_plan(), None, "fixed"), Ok(()));
        assert!(the_card_goes(&a_plan(), Some(OLD_CARD), "fixed").is_err());
        assert!(the_card_goes(&a_plan(), Some(NEW_CARD), "automatic").is_err());
        let mut none = a_plan();
        none["cards"][0]["new"] = json!(null);
        assert!(the_card_goes(&none, Some(NEW_CARD), "fixed").is_err());
    }

    #[test]
    fn the_bytes_free_are_the_one_thing_two_plans_may_differ_in() {
        let mut later = a_plan();
        later["free"] = json!(999_000);
        assert_ne!(a_plan(), later);
        assert_eq!(steady(a_plan()), steady(later));
        let mut other = a_plan();
        other["needed"] = json!(1);
        assert_ne!(steady(a_plan()), steady(other));
    }

    #[test]
    fn the_arguments_are_the_source_and_the_key_file_unless_they_are_named() {
        assert_eq!(by_file(json!({})), json!({"source": SOURCE, "key_file": KEY_FILE}));
        assert_eq!(
            by_file(json!({"cards": "a=b", "source": "x y"})),
            json!({"source": "x y", "key_file": KEY_FILE, "cards": "a=b"})
        );
        assert_eq!(
            by_file(json!({"key_file": "k"})),
            json!({"source": SOURCE, "key_file": "k"})
        );
    }

    #[test]
    fn the_cards_of_a_list_are_their_addresses_in_lower_case() {
        let cards = json!([{"name": "net0", "mac": "52:54:00:AA:00:01"}, {"name": "x"}]);
        assert_eq!(macs(&cards), ["52:54:00:aa:00:01"]);
        assert!(macs(&json!(null)).is_empty());
    }
}
