// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's Security, Dukovo Admin: the sign-in rules, SSH, who may
//! reach the console, private access and the checks, through the routes of
//! their actions (ADR 0562). alice, the owner, makes the changes; the reader
//! lab-reader, in group console and no administrator, makes the contrasts.
//! The three changes that can cut the browser off, a change of SSH, of the
//! console's reach and of the sign-in rules, are tried by Spark (ADR 0472,
//! ADR 0518), as the firewall's and the network's are in admin-trials.
//!
//! The sign-in rules. security.sign-in is what admin settings --json prints.
//! security.sign-in-set with session_hours 1 is in the file and in what the
//! command reads, and the undo puts the value back. A session that ends by
//! the hour of its sign-in cannot be made to age in a boot test, which has no
//! clock to move: the guest's own clock is what admind's upkeep counts, and
//! the lowest session-hours is 1. So the same upkeep is shown by the idle
//! limit, the lowest of which is 5 minutes: with idle_minutes 5 a session of
//! the reader that signs in and is not used, with no page open, is ended by
//! the next round of upkeep after the 5 minutes, and its next call is ADM-107
//! with the words that no page was open for a while. The session signs in
//! with the rule, the case goes on with the other steps, and the session is
//! asked at the end, when the rule is put back.
//!
//! SSH. ssh.on starts sshd, and ssh.show is what ssh-server show --json
//! prints. ssh.set takes passwords, keys and the lists of who may sign in,
//! refusing the two flags off together, as the command does. With try, for 15
//! seconds, Spark has the trial ssh with sshd_config, sshd reads it again,
//! and a second change, on, off or set, is NET-182 while it runs; when the
//! time is up Spark undoes it, with its entry in its document and a row in the
//! journal whose who is Spark and how is undone, and sshd_config reads as it
//! did before. ssh.keep and ssh.undo with no trial are NET-179. A trial of 30
//! seconds that ssh.keep keeps is still in use after 35; ssh.undo puts the
//! file back at once. ssh.off with try stops sshd, the trial holds its
//! service file, and Spark's undo starts it again.
//!
//! The console's reach. console.reach with try, for 20 seconds, is undone by
//! Spark, with the journal's row for admin.toml. For 60 seconds, a run of curl
//! that keeps one connection for the session, console.reach and
//! console/fresh/console gets ADM-133 from the third: the connection was
//! opened before the change took effect, and the console ends it, by
//! Connection: close on HTTP/1.1 and by a GOAWAY on HTTP/2. A new connection
//! gets 200 with where it came from and to. console.keep keeps it, and the
//! reach is put back without a trial. While a trial of the console runs,
//! security.sign-in-set and a second console.reach are ADM-138; console.undo
//! puts the reach back and ends the trial.
//!
//! Private access. A device before private access is NET-184. private.serve
//! sets up wg0 with the file wg0.conf, mode 600 and root's, and private.show
//! is what wireguard show --json prints. firewall.add of udp 51820 lets
//! devices in. private.add of a name says it changed something and answers
//! the device's file once, with its private key and the next address of the
//! tunnel; no byte of the key is in admind's log, authd's audit, logd, the
//! journal or the tasks; private.show lists the device, a second add of its
//! name is NET-185, and private.remove takes it away. A removal that cuts
//! off the browser, ADM-137, needs a request that came through wg0, which a
//! boot test on QEMU's user network cannot make: lab.cut covers the guard's
//! route.
//!
//! The checks and the blocks. checks.run keeps the result that checks.last
//! and dukovo-check --last --json print; security.blocked is what firewall
//! blocked --json prints as root; security.never-block adds a network to its
//! never list and security.never-block-remove takes it away. What needs an
//! administrator is ADM-122 for the reader and ADM-106 for alice locked.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{
    Browser, Call, Console, Http, PORT, Pane, elevated, goaway, is_refusal, is_undone_row,
    trial_in, undone_in, wait_for,
};
use super::super::*;
use super::admin_backups::{READER, READER_PASSWORD, Test, Who, places_of_the_machine};

/// sshd's settings.
const SSHD_CONFIG: &str = "/system/settings/ssh/sshd_config";

/// sshd's service file, which a tried off holds.
const SSHD_SERVICE: &str = "/system/settings/spark/services/sshd.toml";

/// The console's settings.
const ADMIN_TOML: &str = "/system/settings/admin.toml";

/// Spark's words for a trial that was not kept in time.
const TIME_UP: &str = "it was not kept in time";

/// The idle limit the sign-in step sets, the lowest there is, in minutes.
const IDLE_MINUTES: i64 = 5;

/// How long after its sign-in the idle session is asked: the limit and the
/// round of upkeep, which is every 30 seconds, and a few seconds more.
const IDLE_WAIT: Duration = Duration::from_secs(IDLE_MINUTES as u64 * 60 + 40);

/// How often alice's session is used while the idle session is waited for.
const IDLE_KEEP: Duration = Duration::from_secs(60);

/// How long a tried change lasts, in seconds, when the test lets it end by
/// itself: long enough to be asked about three times, and not much longer.
const WINDOW: i64 = 15;

/// How long Spark may take to undo a trial, in seconds, beyond its window.
const UNDO_SECONDS: u64 = 60;

/// The test.
pub const CASE: Case = Case {
    name: "admin-security",
    about: "The web console's Security: security.sign-in is admin settings --json, security.sign-in-set puts session_hours in the file and the undo puts it back, and a session of lab-reader that is not used for the idle limit of 5 minutes is ended by the upkeep, its next call ADM-107 with the words for it (the lowest session-hours, 1, cannot be reached by a clock the test can move); ssh.on and ssh.set change sshd's settings, the two flags off together being refused, and with try Spark has the trial ssh, a change while it runs is NET-182, an undone trial puts sshd_config back as it was with Spark's entry and journal row, keep and undo with no trial are NET-179, a kept one is still in use after its window, and a tried off stops sshd and Spark's undo starts it; console.reach with try is undone by Spark with its journal row, fresh/console is ADM-133 on the connection opened before the change and 200 on a new one over HTTP/1.1 and HTTP/2, console.keep and console.undo end the trial, and security.sign-in-set and console.reach are ADM-138 while it runs; a device before private access is NET-184, private.serve makes wg0.conf, root's with mode 600, private.add says it changed something and answers the device's file with its private key, which no log, audit, record, journal row or task has, private.show lists the device, its name again is NET-185 and private.remove takes it away; checks.run keeps what checks.last and dukovo-check --last --json print, security.blocked is firewall blocked --json, and never-block and its removal change the never list; what needs an administrator is ADM-122 for the reader and ADM-106 for alice locked (ADR 0472, ADR 0518, ADR 0561, ADR 0562)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind among them, and
    // keeps the trials.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_security,
    disks: &[],
    second: None,
};

fn admin_security(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        reader: Browser::default(),
    };

    // alice's password once for this pane, then the reader.
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

    let idle = sign_in_rules(vm, &mut test)?;
    ssh_changed_and_tried(vm, &mut test)?;
    ssh_kept_undone_and_off(vm, &mut test)?;
    console_reach_tried(vm, &mut test)?;
    private_access(vm, &mut test)?;
    checks_and_blocks(vm, &mut test)?;
    an_idle_session_ended(vm, &mut test, idle)?;
    what_needs_an_administrator(&mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// A session that signed in with the idle limit set and is not used.
struct Idle {
    browser: Browser,
    /// When it signed in.
    since: Instant,
    /// The limit before the step set it, in minutes.
    before: i64,
}

/// The text of the settings file `path`, read in the pane as an
/// administrator: empty when there is none.
fn file(vm: &mut Vm, test: &Test, path: &str) -> Result<String, String> {
    test.pane.output(vm, &format!("elevate cat {path} 2>/dev/null"))
}

/// What `admin settings --json` prints.
fn settings(vm: &mut Vm, test: &Test) -> Result<Value, String> {
    test.printed(vm, "admin settings --json")
}

/// Spark's document, which the list of services is.
fn document(test: &mut Test) -> Result<Value, String> {
    test.read(Who::Alice, "/api/1/services")
}

/// The number of the journal's newest row, 0 when it has none.
fn newest(test: &mut Test) -> Result<i64, String> {
    let list = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let rows = list["changes"].as_array().ok_or_else(|| format!("no changes in {list}"))?;
    Ok(rows.first().and_then(|row| row["id"].as_i64()).unwrap_or(0))
}

/// The rows of the journal made after row `after`, oldest first.
fn rows_after(test: &mut Test, after: i64) -> Result<Vec<Value>, String> {
    let list = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let mut rows = list["changes"].as_array().cloned().ok_or_else(|| format!("no changes in {list}"))?;
    rows.retain(|row| row["id"].as_i64().is_some_and(|id| id > after));
    rows.reverse();
    Ok(rows)
}

/// Checks that the journal has, after row `after`, a row of the change
/// `action` as alice made it through the console, with status 0, which
/// names `file`.
fn console_row(test: &mut Test, after: i64, action: &str, file: &str) -> Result<(), String> {
    let rows = rows_after(test, after)?;
    let row = rows
        .iter()
        .rev()
        .find(|row| row["action"] == action)
        .ok_or_else(|| format!("the journal has no row for {action} after row {after}"))?;
    let fields = (row["who"].as_str(), row["how"].as_str(), row["status"].as_i64());
    let names_it = row["files"].as_array().is_some_and(|all| all.iter().any(|one| one["path"] == file));
    if fields != (Some("alice"), Some("console"), Some(0)) || !names_it {
        return Err(format!(
            "the row of {action} is {row}, not alice's, by the console, with status 0, naming {file}"
        ));
    }
    Ok(())
}

/// Waits for Spark to undo the trial `id`, which it had `before` entries
/// of undone trials for: its document has no such trial and one more
/// entry, and the journal has Spark's row after row `after` for `file`.
fn undone_by_spark(
    test: &mut Test,
    id: &str,
    before: usize,
    after: i64,
    file: &str,
) -> Result<(), String> {
    wait_for(&format!("Spark's undo of {id}"), UNDO_SECONDS, || {
        let document = document(test)?;
        let undone = undone_in(&document, id);
        if trial_in(&document, id).is_none() && undone.len() > before {
            return Ok(Ok(()));
        }
        Ok(Err(format!("trial {:?}, {} undone", trial_in(&document, id), undone.len())))
    })?;
    wait_for(&format!("the journal's undone row for {file}"), UNDO_SECONDS, || {
        let rows = rows_after(test, after)?;
        let found = rows.iter().any(|row| is_undone_row(row, file, TIME_UP));
        Ok(if found { Ok(()) } else { Err("no such row yet".to_string()) })
    })
}

/// How many entries Spark's document has for the undone trials `id`.
fn undone_count(test: &mut Test, id: &str) -> Result<usize, String> {
    let document = document(test)?;
    Ok(undone_in(&document, id).len())
}

/// Whether the trial `id` of Spark's document holds `file`.
fn trial_holds(document: &Value, id: &str, file: &str) -> bool {
    let files = trial_in(document, id).and_then(|trial| trial["files"].as_array());
    files.is_some_and(|all| all.iter().any(|one| one == file))
}

/// Step 1: security.sign-in is what the command prints, and security.
/// sign-in-set puts session_hours 1 in what the command reads, and the
/// undo puts it back, each with its row in the journal. Then the idle
/// limit of 5 minutes is set, and a session of the reader signs in and is
/// left alone, to be asked at the end ([`an_idle_session_ended`]).
fn sign_in_rules(vm: &mut Vm, test: &mut Test) -> Result<Idle, String> {
    let hers = test.read(Who::Alice, "/api/1/security/sign-in")?;
    let printed = settings(vm, test)?;
    if hers != printed {
        return Err(format!("security.sign-in is {hers}, and admin settings --json printed {printed}"));
    }
    let his = test.read(Who::Reader, "/api/1/security/sign-in")?;
    if his != hers {
        return Err(format!("the reader's security.sign-in is {his}, alice's is {hers}"));
    }
    let rules = printed["sign_in"].clone();
    let (hours, before) = (rules["session_hours"].as_i64(), rules["idle_minutes"].as_i64());
    let (Some(hours), Some(before)) = (hours, before) else {
        return Err(format!("the sign-in rules are {rules}, which has no hours and minutes"));
    };

    // One hour is in the file, and what the command reads says so.
    let first = newest(test)?;
    test.change("/api/1/security/sign-in", &json!({"session_hours": 1}))?;
    console_row(test, first, "security.sign-in-set", ADMIN_TOML)?;
    let now = settings(vm, test)?;
    if now["sign_in"]["session_hours"] != json!(1) || now["sign_in"]["idle_minutes"] != json!(before) {
        return Err(format!("the sign-in rules are {} after session_hours 1", now["sign_in"]));
    }
    if test.read(Who::Alice, "/api/1/security/sign-in")? != now {
        return Err("security.sign-in does not say what was set".into());
    }
    // A value outside what a session may last is refused before it runs.
    for body in [json!({"session_hours": 0}), json!({"session_hours": 169}), json!({"idle_minutes": 4})] {
        test.post(Who::Alice, "/api/1/security/sign-in", &body)?.refused(
            &format!("the rules {body}"),
            400,
            "ADM-125",
        )?;
    }
    // The undo is the value it had.
    test.change("/api/1/security/sign-in", &json!({"session_hours": hours}))?;
    if settings(vm, test)?["sign_in"] != rules {
        return Err(format!("the rules are not {rules} after the undo"));
    }

    // The idle limit, and a session that has no page open and is not used.
    test.change("/api/1/security/sign-in", &json!({"idle_minutes": IDLE_MINUTES}))?;
    let mut browser = Browser::default();
    test.console.sign_in(&mut browser, READER, READER_PASSWORD)?;
    Ok(Idle { browser, since: Instant::now(), before })
}

/// The last of step 1: once the idle limit and a round of upkeep have gone
/// by, the session that was not used is ended, as the hour of a session
/// would end it: its next call is ADM-107 with the words for an idle
/// session. The limit is put back.
fn an_idle_session_ended(vm: &mut Vm, test: &mut Test, mut idle: Idle) -> Result<(), String> {
    // alice's session is read now and then, as a page's would be: with the
    // idle limit of 5 minutes, hers would end as well if she waited as long.
    let due = idle.since + IDLE_WAIT;
    while Instant::now() < due {
        std::thread::sleep(due.saturating_duration_since(Instant::now()).min(IDLE_KEEP));
        test.read(Who::Alice, "/api/1/session")?;
    }
    let reply = test.console.get(&mut idle.browser, "/api/1/session")?;
    let reply = reply.refused("the session that was not used", 401, "ADM-107")?;
    if !reply.detail().contains("no console tab was open for a while") {
        return Err(format!("the idle session was refused for {:?}", reply.detail()));
    }
    // alice's own session was used all the while, and is still hers.
    test.read(Who::Alice, "/api/1/session")?;
    test.change("/api/1/security/sign-in", &json!({"idle_minutes": idle.before}))?;
    if settings(vm, test)?["sign_in"]["idle_minutes"] != json!(idle.before) {
        return Err("the idle limit is not what it was".into());
    }
    Ok(())
}

/// What `ssh.show` says, which is `ssh-server show --json`.
fn ssh_shown(test: &mut Test) -> Result<Value, String> {
    test.read(Who::Alice, "/api/1/ssh")
}

/// Waits up to `seconds` for `ssh.show` to say `running` for sshd and `on`
/// for whether it starts at boot.
fn ssh_is(test: &mut Test, running: bool, on: bool, seconds: u64) -> Result<(), String> {
    wait_for(&format!("sshd running {running} and on {on}"), seconds, || {
        let shown = ssh_shown(test)?;
        let is = (shown["running"] == json!(running)) && (shown["on"] == json!(on));
        Ok(if is { Ok(()) } else { Err(shown.to_string()) })
    })
}

/// Step 2: ssh.on starts sshd and ssh.show is the command's. ssh.set of
/// both flags off is refused. A change of passwords tried for a short
/// window is in sshd_config and in the trial of Spark's document, is NET-182
/// to change again while it runs, and is undone by Spark when the window
/// ends: the file reads as before, and the journal has Spark's row.
/// ssh.keep and ssh.undo with no trial are NET-179.
fn ssh_changed_and_tried(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let printed = test.printed(vm, "ssh-server show --json")?;
    if ssh_shown(test)? != printed {
        return Err(format!("ssh.show is not what ssh-server show --json printed: {printed}"));
    }
    test.change("/api/1/ssh/on", &json!({}))?;
    ssh_is(test, true, true, 60)?;
    let shown = ssh_shown(test)?;
    if shown["mistakes"] != json!([]) || shown["trial"] != json!(null) {
        return Err(format!("sshd has mistakes or a trial before the step begins: {shown}"));
    }
    let passwords = shown["passwords"].as_bool().ok_or_else(|| format!("no passwords in {shown}"))?;

    // Both ways in off together would shut everyone out: the command
    // refuses it, before anything runs.
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/ssh", &json!({"passwords": "no", "keys": "no"}))?.refused(
        "both flags off",
        400,
        "ADM-125",
    )?;
    test.post(Who::Alice, "/api/1/ssh/trial/keep", &json!({}))?.refused(
        "keeping nothing",
        422,
        "NET-179",
    )?;
    test.post(Who::Alice, "/api/1/ssh/trial/undo", &json!({}))?.refused(
        "undoing nothing",
        422,
        "NET-179",
    )?;

    // A tried change.
    let (text, first) = (file(vm, test, SSHD_CONFIG)?, newest(test)?);
    let undone = undone_count(test, "ssh")?;
    test.change("/api/1/ssh", &json!({"passwords": word(!passwords), "try": WINDOW}))?;
    let document = document(test)?;
    if !trial_holds(&document, "ssh", SSHD_CONFIG) {
        return Err(format!("Spark's document has no trial ssh with {SSHD_CONFIG}: {document}"));
    }
    let shown = ssh_shown(test)?;
    if shown["passwords"] != json!(!passwords) || shown["running"] != json!(true) {
        return Err(format!("ssh.show is {shown} while passwords {} is tried", word(!passwords)));
    }
    if shown["trial"]["left_ms"].as_i64().is_none_or(|left| left <= 0) {
        return Err(format!("ssh.show has no time left for the trial: {shown}"));
    }
    if file(vm, test, SSHD_CONFIG)? == text {
        return Err("sshd_config is as before while a change of it is tried".into());
    }
    // A second change while it runs would be lost to the undo.
    test.unlocked()?;
    for (path, body) in [
        ("/api/1/ssh", json!({"passwords": word(passwords)})),
        ("/api/1/ssh/on", json!({})),
        ("/api/1/ssh/off", json!({})),
    ] {
        test.post(Who::Alice, path, &body)?.refused(&format!("{path} while tried"), 422, "NET-182")?;
    }

    // The window ends, and Spark puts the file back.
    undone_by_spark(test, "ssh", undone, first, SSHD_CONFIG)?;
    if file(vm, test, SSHD_CONFIG)? != text {
        return Err("sshd_config is not as it was after Spark's undo".into());
    }
    let shown = ssh_shown(test)?;
    if shown["passwords"] != json!(passwords) || shown["trial"] != json!(null) || shown["running"] != json!(true) {
        return Err(format!("ssh.show is {shown} after Spark's undo"));
    }
    Ok(())
}

/// The word the actions of SSH take for a flag.
fn word(yes: bool) -> &'static str {
    if yes { "yes" } else { "no" }
}

/// Step 3: a change tried for 30 seconds and kept is still in use after 35;
/// one undone by ssh.undo is put back at once; ssh.off with try stops sshd,
/// the trial holds its service file, and when the window ends Spark's undo
/// starts sshd again, with its row in the journal.
fn ssh_kept_undone_and_off(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let shown = ssh_shown(test)?;
    let passwords = shown["passwords"].as_bool().ok_or_else(|| format!("no passwords in {shown}"))?;

    // Kept: the change is still there after the window.
    let began = Instant::now();
    test.change("/api/1/ssh", &json!({"passwords": word(!passwords), "try": 30}))?;
    test.change("/api/1/ssh/trial/keep", &json!({}))?;
    std::thread::sleep((began + Duration::from_secs(35)).saturating_duration_since(Instant::now()));
    let shown = ssh_shown(test)?;
    if shown["passwords"] != json!(!passwords) || shown["trial"] != json!(null) {
        return Err(format!("ssh.show is {shown} after a kept change and its window"));
    }
    if trial_in(&document(test)?, "ssh").is_some() {
        return Err("Spark still has a trial ssh after ssh.keep".into());
    }
    // What was kept is changed back, with no trial.
    test.change("/api/1/ssh", &json!({"passwords": word(passwords)}))?;
    if ssh_shown(test)?["passwords"] != json!(passwords) {
        return Err("ssh.show does not have the passwords back".into());
    }

    // Undone by hand: the file is as it was at once.
    let text_before = file(vm, test, SSHD_CONFIG)?;
    test.change("/api/1/ssh", &json!({"passwords": word(!passwords), "try": 60}))?;
    test.change("/api/1/ssh/trial/undo", &json!({}))?;
    if file(vm, test, SSHD_CONFIG)? != text_before || trial_in(&document(test)?, "ssh").is_some() {
        return Err("ssh.undo did not put the file back and end the trial".into());
    }

    // A tried off: sshd stops, and Spark's undo starts it.
    let (service, first) = (file(vm, test, SSHD_SERVICE)?, newest(test)?);
    let undone = undone_count(test, "ssh")?;
    test.change("/api/1/ssh/off", &json!({"try": WINDOW * 2}))?;
    let document = document(test)?;
    if !trial_holds(&document, "ssh", SSHD_SERVICE) {
        return Err(format!("Spark's document has no trial ssh with {SSHD_SERVICE}: {document}"));
    }
    ssh_is(test, false, false, 30)?;
    undone_by_spark(test, "ssh", undone, first, SSHD_SERVICE)?;
    if file(vm, test, SSHD_SERVICE)? != service {
        return Err("sshd's service file is not as it was after Spark's undo".into());
    }
    ssh_is(test, true, true, 60)
}

/// The other word of the console's reach: `private` or `any`.
fn other_reach(reach: &str) -> &'static str {
    if reach == "private" { "any" } else { "private" }
}

/// Step 4: console.reach with try, for 20 seconds, is undone by Spark with
/// its row in the journal and the file as before. With a window of a minute,
/// console/fresh/console is ADM-133 on the connection that carried the change
/// and 200 on a new one, over HTTP/1.1 and over HTTP/2, and console.keep
/// keeps it. While a trial of the console runs, a change of the sign-in
/// rules and a second console.reach are ADM-138, and console.undo ends it.
fn console_reach_tried(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let reach = settings(vm, test)?["console"]["reach"].as_str().map(String::from);
    let reach = reach.ok_or("the settings have no reach for the console")?;
    let other = other_reach(&reach);

    // Left to Spark.
    let (text, first) = (file(vm, test, ADMIN_TOML)?, newest(test)?);
    let undone = undone_count(test, "console")?;
    test.change("/api/1/console/reach", &json!({"reach": other, "try": WINDOW + 5}))?;
    if !trial_holds(&document(test)?, "console", ADMIN_TOML) {
        return Err(format!("Spark has no trial console with {ADMIN_TOML}"));
    }
    undone_by_spark(test, "console", undone, first, ADMIN_TOML)?;
    if file(vm, test, ADMIN_TOML)? != text || settings(vm, test)?["console"]["reach"] != json!(reach) {
        return Err("the console's reach is not as it was after Spark's undo".into());
    }

    // The connection that carried the change is the old one.
    for http in [Http::One, Http::Two] {
        is_fresh(test, http, other)?;
        test.change("/api/1/console/reach", &json!({"reach": reach}))?;
        if settings(vm, test)?["console"]["reach"] != json!(reach) {
            return Err(format!("the reach is not {reach} after it was put back by hand"));
        }
    }

    // Nothing else of admin.toml is changed while a trial of it runs.
    let (text, hours) = (file(vm, test, ADMIN_TOML)?, settings(vm, test)?["sign_in"]["session_hours"].clone());
    test.change("/api/1/console/reach", &json!({"reach": other, "try": 60}))?;
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/security/sign-in", &json!({"session_hours": hours}))?.refused(
        "the sign-in rules while the console is tried",
        422,
        "ADM-138",
    )?;
    test.post(Who::Alice, "/api/1/console/reach", &json!({"reach": reach}))?.refused(
        "a second reach while the console is tried",
        422,
        "ADM-138",
    )?;
    test.change("/api/1/console/trial/undo", &json!({}))?;
    if trial_in(&document(test)?, "console").is_some() || file(vm, test, ADMIN_TOML)? != text {
        return Err("console.undo did not put admin.toml back and end the trial".into());
    }
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/console/trial/keep", &json!({}))?.refused(
        "keeping a trial that is over",
        422,
        "NET-179",
    )?;
    Ok(())
}

/// A change of the reach to `other`, tried for a minute, then asked about
/// by the three requests of one connection over `http`, and kept.
fn is_fresh(test: &mut Test, http: Http, other: &str) -> Result<(), String> {
    test.unlocked()?;
    let calls = [
        Call::get("/api/1/session"),
        Call::post("/api/1/console/reach", &json!({"reach": other, "try": 60})),
        Call::get("/api/1/console/fresh/console"),
    ];
    let run = test.console.carry(&test.alice, http, &calls)?;
    let [session, change, old] = &run.carried[..] else {
        return Err("a run of three requests carried out another number".into());
    };
    if session.reply.status != 200 || change.reply.status != 200 {
        return Err(format!(
            "the session was answered {}, and the change {} {}",
            session.reply.status, change.reply.status, change.reply.body
        ));
    }
    for (what, request) in [("the session", session), ("the change", change), ("fresh", old)] {
        if request.version != http.word() {
            return Err(format!("{what} went by HTTP {}, not {}", request.version, http.word()));
        }
    }
    if (session.connects, change.connects, old.connects) != (1, 0, 0) {
        return Err(format!(
            "curl opened {}, {} and {} connections for three requests that share one",
            session.connects, change.connects, old.connects
        ));
    }
    if !is_refusal(&old.reply, 409, "ADM-133") {
        return Err(format!(
            "fresh on the connection opened before the change was answered {} {}, not ADM-133",
            old.reply.status, old.reply.body
        ));
    }
    let ended = match http {
        Http::One => old.reply.header("connection") == Some("close"),
        Http::Two => goaway(&run.said),
    };
    if !ended || !run.ok {
        return Err(format!("the console did not end the old connection: {}", run.said));
    }

    // A new connection is one opened after the change.
    let new = test.console.carry(&test.alice, http, &[Call::get("/api/1/console/fresh/console")])?;
    let [one] = &new.carried[..] else {
        return Err("one request carried out another number".into());
    };
    let answer = one.reply.json()?;
    if one.reply.status != 200 || one.connects != 1 || answer["fresh"] != json!(true) {
        return Err(format!("fresh on a new connection was answered {} {answer}", one.reply.status));
    }
    if answer["via"]["port"] != json!(PORT) || answer["from"].as_str().is_none_or(str::is_empty) {
        return Err(format!("fresh does not say where the request came from and to: {answer}"));
    }
    test.change("/api/1/console/trial/keep", &json!({}))?;
    if trial_in(&document(test)?, "console").is_some() {
        return Err("Spark still has a trial console after console.keep".into());
    }
    Ok(())
}

/// The device's file in the answer of private.add, which `wireguard add
/// --json` prints as one line: its `config`.
fn config_in(answer: &Value) -> Option<String> {
    let lines = answer["output"].as_array()?;
    lines
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|device| device["config"].as_str().map(String::from))
}

/// The private key in a WireGuard file: the value of its `PrivateKey` line.
fn private_key_in(config: &str) -> Option<String> {
    let line = config.lines().find(|line| line.trim_start().starts_with("PrivateKey"))?;
    let (_, key) = line.split_once('=')?;
    Some(key.trim().to_string()).filter(|key| !key.is_empty())
}

/// Step 5: a device before private access is NET-184. private.serve makes
/// wg0.conf, which only root reads, and private.show is what wireguard
/// show --json prints; firewall.add lets udp 51820 in. private.add answers
/// the device's file with its private key once, and no log, audit, record,
/// row or task has the key. private.show lists the device, its name again is
/// NET-185, and private.remove takes it away.
fn private_access(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    for (path, what) in [
        ("/api/1/private/devices/add", "a device before private access"),
        ("/api/1/private/devices/remove", "a removal before private access"),
    ] {
        test.post(Who::Alice, path, &json!({"name": "lab-phone"}))?.refused(what, 422, "NET-184")?;
    }

    test.change("/api/1/private/serve", &json!({"endpoint": "lab.example.org"}))?;
    let modes = test.pane.output(vm, "elevate stat -c '%U %a' /system/state/wireguard/wg0.conf")?;
    if modes.trim() != "root 600" {
        return Err(format!("wg0.conf is {modes:?}, not root's with mode 600"));
    }
    let shown = private_shown(vm, test)?;
    let interface = &shown["interfaces"][0];
    let served = interface["name"] == json!("wg0")
        && interface["address"] == json!("10.8.0.1/24")
        && interface["port"] == json!(51820)
        && interface["endpoint"] == json!("lab.example.org");
    if !served || interface["devices"] != json!([]) {
        return Err(format!("private.show after private.serve is {shown}"));
    }
    let rule = json!({"action": "allow", "protocol": "udp", "ports": "51820"});
    test.change("/api/1/firewall/add", &rule)?;
    let rules = test.read(Who::Alice, "/api/1/firewall")?;
    let has_rule = rules["rules"].as_array().is_some_and(|all| all.iter().any(|one| one["text"] == "allow udp 51820"));
    if !has_rule {
        return Err(format!("the firewall has no rule for udp 51820: {rules}"));
    }

    // A device: its file, with its key, once.
    let answer = test.change("/api/1/private/devices/add", &json!({"name": "lab-phone"}))?;
    // wg0.conf is not in admind's view, so nothing compares it: the
    // answer says that the change changed something.
    if answer["changed"] != json!(true) {
        return Err(format!("private.add says it changed nothing: {}", answer["changed"]));
    }
    let config = config_in(&answer).ok_or_else(|| "private.add answered no device file".to_string())?;
    let key = private_key_in(&config).ok_or("the device's file has no PrivateKey")?;
    if !config.contains("[Peer]") || !config.contains("10.8.0.2/32") {
        return Err("the device's file has no [Peer] and no address of 10.8.0.2/32".into());
    }
    for (what, text) in places_of_the_machine(vm, test)? {
        if text.contains(&key) {
            return Err(format!("the device's private key is in {what}"));
        }
    }
    let shown = private_shown(vm, test)?;
    let device = &shown["interfaces"][0]["devices"][0];
    if device["name"] != json!("lab-phone") || device["addresses"] != json!(["10.8.0.2/32"]) {
        return Err(format!("private.show does not list lab-phone with its address: {shown}"));
    }
    if device["public_key"].as_str().is_none_or(str::is_empty) {
        return Err(format!("private.show has no public key for lab-phone: {shown}"));
    }
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/private/devices/add", &json!({"name": "lab-phone"}))?.refused(
        "a name in use",
        422,
        "NET-185",
    )?;
    test.post(Who::Alice, "/api/1/private/devices/add", &json!({"name": "lab-tablet", "reach": "console"}))?
        .refused("a device with a reach", 400, "ADM-108")?;

    // The device goes, and its key with it.
    test.change("/api/1/private/devices/remove", &json!({"name": "lab-phone"}))?;
    let shown = private_shown(vm, test)?;
    if shown["interfaces"][0]["devices"] != json!([]) {
        return Err(format!("private.show lists a device after private.remove: {shown}"));
    }
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/private/devices/remove", &json!({"name": "lab-phone"}))?.refused(
        "a device that is gone",
        422,
        "NET-184",
    )?;
    // The rule that let devices in is taken away.
    test.change("/api/1/firewall/remove", &rule)?;
    Ok(())
}

/// What private.show answers, which is `wireguard show --json` as root.
fn private_shown(vm: &mut Vm, test: &mut Test) -> Result<Value, String> {
    // wgd brings the interface up a moment after it is made: the two reads
    // are asked again until they agree.
    wait_for("private.show to be what wireguard show --json prints", 30, || {
        test.unlocked()?;
        let shown = test.read(Who::Alice, "/api/1/private")?;
        let printed = test.printed(vm, "elevate wireguard show --json")?;
        if shown == printed {
            return Ok(Ok(shown));
        }
        Ok(Err(format!("private.show is {shown}, and the command printed {printed}")))
    })
}

/// The result that checks.run answered, from the lines the command
/// printed: they are its one line of JSON.
fn kept_in(answer: &Value) -> Option<Value> {
    let lines: Vec<&str> = answer["output"].as_array()?.iter().filter_map(Value::as_str).collect();
    serde_json::from_str(&lines.join("\n")).ok()
}

/// Step 6: checks.run keeps the result that checks.last and dukovo-check
/// --last --json print. security.blocked is what firewall blocked --json
/// prints as root; a network never blocked is in its list, and is gone with
/// security.never-block-remove.
fn checks_and_blocks(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let ran = test.change("/api/1/checks/run", &json!({}))?;
    let kept = kept_in(&ran).ok_or_else(|| format!("checks.run did not print a result: {ran}"))?;
    let last = test.read(Who::Alice, "/api/1/checks/last")?;
    let printed = test.printed(vm, "dukovo-check --last --json")?;
    if last != printed || last != kept {
        return Err(format!(
            "checks.run kept {kept}, checks.last is {last}, and the command printed {printed}"
        ));
    }

    test.unlocked()?;
    let blocked = test.read(Who::Alice, "/api/1/security/blocked")?;
    let printed = test.printed(vm, "elevate firewall blocked --json")?;
    if blocked != printed {
        return Err(format!("security.blocked is {blocked}, and firewall blocked --json printed {printed}"));
    }
    let network = "192.0.2.0/24";
    let never = |blocked: &Value| {
        blocked["never"].as_array().is_some_and(|all| all.iter().any(|one| one == network))
    };
    if never(&blocked) {
        return Err(format!("{network} is never blocked before the test adds it: {blocked}"));
    }
    test.change("/api/1/security/never-block", &json!({"network": network}))?;
    let blocked = test.read(Who::Alice, "/api/1/security/blocked")?;
    if !never(&blocked) {
        return Err(format!("security.blocked does not list {network} as never blocked: {blocked}"));
    }
    test.change("/api/1/security/never-block/remove", &json!({"network": network}))?;
    let blocked = test.read(Who::Alice, "/api/1/security/blocked")?;
    if never(&blocked) {
        return Err(format!("security.blocked lists {network} after its removal: {blocked}"));
    }
    // A host's address is not a network the list takes.
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/security/never-block", &json!({"network": "192.0.2.5/24"}))?
        .refused("a network with host bits", 400, "ADM-125")?;
    Ok(())
}

/// Every action of the Security page that needs an administrator, each with
/// arguments that would work if the asker were one, is ADM-122 for the
/// reader and ADM-106 for alice with her session locked, before anything
/// runs. What a person reads of the rules and of SSH is open to both.
fn what_needs_an_administrator(test: &mut Test) -> Result<(), String> {
    // The reader's session of the start was not used for as long as the idle
    // limit: both sign in again.
    test.sign_in()?;
    // Whether it is a POST, its address, and its arguments.
    let asks = [
        (false, "/api/1/security/blocked", json!({})),
        (true, "/api/1/security/sign-in", json!({"session_hours": 8})),
        (true, "/api/1/security/unblock", json!({"address": "203.0.113.9"})),
        (true, "/api/1/security/never-block", json!({"network": "192.0.2.0/24"})),
        (true, "/api/1/security/never-block/remove", json!({"network": "192.0.2.0/24"})),
        (true, "/api/1/ssh", json!({"passwords": "no"})),
        (true, "/api/1/ssh/on", json!({})),
        (true, "/api/1/ssh/off", json!({})),
        (true, "/api/1/ssh/trial/keep", json!({})),
        (true, "/api/1/ssh/trial/undo", json!({})),
        (true, "/api/1/console/reach", json!({"reach": "private"})),
        (true, "/api/1/console/trial/keep", json!({})),
        (true, "/api/1/console/trial/undo", json!({})),
        (false, "/api/1/private", json!({})),
        (true, "/api/1/private/serve", json!({})),
        (true, "/api/1/private/devices/add", json!({"name": "lab-refused"})),
        (true, "/api/1/private/devices/remove", json!({"name": "lab-refused"})),
        (true, "/api/1/checks/run", json!({})),
    ];
    test.locked()?;
    for (post, path, body) in &asks {
        for (who, id) in [(Who::Reader, "ADM-122"), (Who::Alice, "ADM-106")] {
            let reply = if *post { test.post(who, path, body)? } else { test.get(who, path)? };
            reply.refused(path, 403, id)?;
        }
    }
    for path in ["/api/1/security/sign-in", "/api/1/ssh"] {
        test.read(Who::Reader, path)?;
        test.read(Who::Alice, path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_other_reach_is_the_one_the_console_does_not_have() {
        assert_eq!(other_reach("private"), "any");
        assert_eq!(other_reach("any"), "private");
    }

    #[test]
    fn the_idle_session_is_asked_after_the_limit_and_a_round_of_upkeep() {
        assert_eq!(IDLE_WAIT, Duration::from_secs(340));
    }

    #[test]
    fn a_trial_holds_the_files_spark_says() {
        let document = json!({"trials": [
            {"id": "ssh", "files": [SSHD_CONFIG], "left_ms": 9000},
            {"id": "console", "files": [ADMIN_TOML], "left_ms": 5000},
        ]});
        assert!(trial_holds(&document, "ssh", SSHD_CONFIG));
        assert!(!trial_holds(&document, "ssh", SSHD_SERVICE));
        assert!(!trial_holds(&document, "firewall", SSHD_CONFIG));
        assert!(!trial_holds(&json!({}), "ssh", SSHD_CONFIG));
    }

    #[test]
    fn a_flag_is_the_word_yes_or_no() {
        assert_eq!((word(true), word(false)), ("yes", "no"));
    }

    #[test]
    fn the_devices_file_is_in_the_line_the_command_printed() {
        let line = json!({"name": "lab-phone", "address": "10.8.0.2/32", "config": "[Interface]\nPrivateKey = AAAA\n", "live": true});
        let answer = json!({"done": true, "changed": true, "output": ["", line.to_string()]});
        assert_eq!(config_in(&answer).as_deref(), Some("[Interface]\nPrivateKey = AAAA\n"));
        assert_eq!(config_in(&json!({"output": ["not json"]})), None);
        assert_eq!(config_in(&json!({})), None);
    }

    #[test]
    fn the_private_key_is_the_value_of_its_line() {
        let config = "[Interface]\nPrivateKey = oK56DE9Ue9zK76rAc8pBl6opph+1v36lm7cXXsQKrQM=\nAddress = 10.8.0.2/32\n";
        assert_eq!(
            private_key_in(config).as_deref(),
            Some("oK56DE9Ue9zK76rAc8pBl6opph+1v36lm7cXXsQKrQM=")
        );
        assert_eq!(private_key_in("[Interface]\nAddress = 10.8.0.2/32\n"), None);
        assert_eq!(private_key_in("PrivateKey =\n"), None);
    }

    #[test]
    fn the_checks_result_is_the_json_the_lines_make() {
        let answer = json!({"done": true, "output": ["{\"format\":1,", "\"checks\":[]}"]});
        assert_eq!(kept_in(&answer), Some(json!({"format": 1, "checks": []})));
        assert_eq!(kept_in(&json!({"output": ["no"]})), None);
        assert_eq!(kept_in(&json!({})), None);
    }
}
