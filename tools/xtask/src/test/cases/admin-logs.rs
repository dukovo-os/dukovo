// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Logs of the web console, Dukovo Admin: reads of the log by each
//! person's rights and the follow of it, which arrives as server-sent
//! events (ADR 0518). alice, the owner, is an administrator; bob may use
//! the console and is no administrator.
//!
//! What a person reads. bob's `GET /api/1/logs?service=diskhealth` is
//! empty: he reads the records of his own programs and no service's.
//! After `logger lab-bob-line` in his Terminal he reads that line, and
//! every record he is given is a program's. alice, with her session locked,
//! is refused the whole log, `GET /api/1/logs/all`, with ADM-106; unlocked,
//! the same read for diskhealth's records is what her pane prints for the
//! command `elevate log --all --output json --lines 50 --service
//! diskhealth`, record by record, by boot and sequence number, allowing for
//! what the log gained between the two reads.
//!
//! What is refused. A filter that cannot work is ADM-125 with its field:
//! 1001 lines, a pattern that is no pattern, and a service with the
//! kernel's records. A time that is no number is ADM-108, as every number
//! of an address is.
//!
//! The follow. `GET /api/1/logs/follow?all=true&grep=lab-follow`, read by
//! curl as the page's event source is, begins with a `command` event, the
//! command the follow runs as the page shows it, and tells each line that
//! root's steps write with `logger` as a `record` event while the steps
//! run, not all at the end. A line is matched by `grep` and not by the
//! service filter, since that is for a service's records and logger's are
//! a program's. The follow's own process, `log`, runs in the guest; the
//! route `POST /api/1/lock` ends the follow with an `ended` event, and its
//! reason is `locked`. The process is gone, and authd's audit has the
//! start of the follow for alice and its end, "the console locked". A
//! second follow of the same session ends the first with `replaced`.
//! Where the build is the lab's, which offers the action `lab.long`, a
//! follow lasts a minute and ends with the event `time`; a build for
//! people has an hour, which no test waits for.
//!
//! bob's follow has no `all`, and with it is ADM-122. It shows the line
//! that bob writes in his Terminal and not the one that root wrote before
//! it, and no record that is not a program's.
//!
//! What the pane's commands print reaches the host through nc, to a port on
//! the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::fs::File;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{
    ALICE_PASSWORD, Browser, Console, ORIGIN, PORT, Pane, Reply, SESSION_COOKIE, Socket, elevated,
    tail,
};
use super::super::*;

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// The size of a pane as the page opens one.
const SIZE: (u32, u32) = (80, 24);

/// The follows started in this run, to give each a file for what it hears.
static FOLLOWS: AtomicUsize = AtomicUsize::new(0);

/// How long a follow lasts in the lab's build, in seconds, and the seconds
/// the test allows for its end.
const LAB_HOUR: u64 = 60;
const LAB_HOUR_ALLOWED: u64 = 150;

/// The test.
pub const CASE: Case = Case {
    name: "admin-logs",
    about: "The web console's Logs: bob reads the records of his own programs and no service's, and the line he writes with logger; alice's read of the whole log is ADM-106 while locked, and unlocked the same as elevate log --all --output json in her pane, record by record by boot and sequence number; 1001 lines, a pattern that is none and a service with the kernel are ADM-125 with their fields; the follow of the whole log begins with its command and tells the lines root writes as they come, ends with locked when the session locks, with its log gone from the guest and authd's audit saying so, and with replaced when the session follows again, and with time after the lab build's minute; bob's follow shows his line alone, and the whole log is ADM-122 for him (ADR 0518)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_logs,
    disks: &[],
    second: None,
};

fn admin_logs(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        bob: Browser::default(),
    };

    // alice's password once for this pane, then bob, who is in group
    // console and no administrator.
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {BOB_PASSWORD} | user add bob; user set --yes --console \
             bob'"
        ),
    )?;
    vm.expect("authd: changed the account bob")?;
    vm.expect_screen(&["Made bob"])?;
    test.sign_in()?;

    bob_reads_his_own(vm, &mut test)?;
    alice_reads_the_whole_log(vm, &mut test)?;
    filters_that_cannot_work(&mut test)?;
    follows_and_is_locked(vm, &mut test)?;
    a_second_follow(&mut test)?;
    bobs_follow(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Who asks the console.
#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

/// What the test keeps from one step to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
    bob: Browser,
}

impl Test {
    /// Signs alice and bob in, as the console's page does, each with a
    /// session of its own.
    fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        self.bob = Browser::default();
        self.console.sign_in(&mut self.bob, "bob", BOB_PASSWORD)?;
        self.console.read_session(&mut self.bob)?;
        Ok(())
    }

    /// The browser of `who`.
    fn browser(&mut self, who: Who) -> &mut Browser {
        match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        }
    }

    /// A read of `path` for `who`, whatever the answer.
    fn get(&mut self, who: Who, path: &str) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.get(browser, path)
    }

    /// What `path` says to `who`, which must be 200.
    fn read(&mut self, who: Who, path: &str) -> Result<Value, String> {
        self.get(who, path)?.must(path, 200)?.json()
    }

    /// Unlocks alice's session with her password, if it is locked.
    fn unlocked(&mut self) -> Result<(), String> {
        let session =
            self.console.get(&mut self.alice, "/api/1/session")?.must("her session", 200)?;
        if session.json()?["unlock_seconds"].as_u64().is_some_and(|left| left > 120) {
            return Ok(());
        }
        let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
        self.console.post(&mut self.alice, "/api/1/unlock", &unlock)?.must("her unlocking", 200)?;
        Ok(())
    }

    /// Opens the follow `query` for `who`, once its answer has begun and is
    /// the stream.
    fn follow(&mut self, who: Who, query: &str) -> Result<Follow, String> {
        let cookie = self.browser(who).cookie.clone().ok_or("a follow needs a session's cookie")?;
        let number = FOLLOWS.fetch_add(1, Ordering::Relaxed);
        let path = self.console.pem.with_file_name(format!("follow-{number}.txt"));
        let file = File::create(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let errors = file.try_clone().map_err(|err| format!("{}: {err}", path.display()))?;
        let child = Command::new("curl")
            .args(["--disable", "--silent", "--show-error", "--http1.1", "--no-buffer"])
            .args(["--max-time", "300", "--noproxy", "*", "--connect-to"])
            .arg(format!("localhost:{PORT}:127.0.0.1:{}", self.console.port))
            .args(["--include", "--cacert"])
            .arg(&self.console.pem)
            .arg("--header")
            .arg(format!("Cookie: {SESSION_COOKIE}={cookie}"))
            .arg(format!("{ORIGIN}/api/1/logs/follow?{query}"))
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(errors)
            .spawn()
            .map_err(|err| format!("the console tests need the host's curl: {err}"))?;
        let follow = Follow { child, path };
        follow.begins()?;
        Ok(follow)
    }

    /// Makes a Terminal of bob's, types `command` in its shell and waits
    /// for it to end with status 0, and ends the terminal.
    fn bob_types(&mut self, vm: &Vm, command: &str) -> Result<(), String> {
        let body = json!({"cols": SIZE.0, "rows": SIZE.1, "admin": false});
        let made = self
            .console
            .post(&mut self.bob, "/api/1/terminals", &body)?
            .must("bob's terminal", 200)?
            .json()?;
        let terminal =
            made["terminal"]["id"].as_u64().ok_or_else(|| format!("no terminal in {made}"))?;
        let pane = made["terminal"]["panes"][0]["id"]
            .as_u64()
            .ok_or_else(|| format!("no pane in {made}"))?;
        let (mut socket, answer) = Socket::open(vm, &self.console, &self.bob.socket_headers())?;
        if answer["status"] != json!(101) || answer["accept_ok"] != json!(true) {
            return Err(format!("the answer to bob's socket is {answer}"));
        }
        socket.attach(pane, SIZE, None)?;
        // His shell draws its prompt, and the command's own echo is no
        // answer: an empty `echo` puts the status on a line of its own.
        socket.wait_output(pane, "$", 90)?;
        socket.type_in(pane, &format!("{command}; echo; echo ended-$?\r"))?;
        socket.wait_output(pane, "\r\nended-0\r\n", 60)?;
        drop(socket);
        let path = format!("/api/1/terminals/{terminal}/end");
        self.console
            .post(&mut self.bob, &path, &json!({}))?
            .must("the end of bob's terminal", 200)?;
        Ok(())
    }
}

/// A follow of the log, as the page's event source opens it: curl keeps
/// running with what it receives in a file, which the test reads as the
/// events come. It is stopped, and the file taken away, when this is
/// dropped.
struct Follow {
    child: Child,
    path: std::path::PathBuf,
}

impl Follow {
    /// What the follow has heard so far, the answer's head and its events,
    /// as text.
    fn text(&self) -> String {
        std::fs::read(&self.path)
            .map_or_default(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The events heard so far, oldest first.
    fn events(&self) -> Vec<(String, Value)> {
        sse_events(&self.text())
    }

    /// Waits up to 30 seconds for the answer's head, which must be a 200:
    /// a refusal is a JSON answer, with no stream.
    fn begins(&self) -> Result<(), String> {
        wait_for("the follow's answer", 30, || {
            let text = self.text();
            Ok(match text.lines().next() {
                Some(line) if line.starts_with("HTTP/") && text.contains("\r\n\r\n") => {
                    if line.split(' ').nth(1) == Some("200") {
                        Ok(())
                    } else {
                        return Err(format!("the follow was refused: {}", tail(&text, 400)));
                    }
                }
                _ => Err(format!("it has heard {:?}", tail(&text, 200))),
            })
        })
    }

    /// Waits up to `seconds` for an event called `name` whose data
    /// `wanted` holds of, and gives its data.
    fn wait_event(
        &self,
        name: &str,
        seconds: u64,
        wanted: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        wait_for(&format!("an event {name}"), seconds, || {
            let events = self.events();
            let found = events.iter().find(|(said, data)| said == name && wanted(data));
            Ok(match found {
                Some((_, data)) => Ok(data.clone()),
                None => Err(format!("it has heard {:?}", tail(&self.text(), 300))),
            })
        })
    }

    /// Waits up to `seconds` until a record with each of `texts` has come,
    /// and gives when each did, as the test saw it.
    fn arrivals(&self, texts: &[&str], seconds: u64) -> Result<Vec<Instant>, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut seen: Vec<Option<Instant>> = vec![None; texts.len()];
        loop {
            let said = self.events();
            for (at, text) in seen.iter_mut().zip(texts) {
                if at.is_none() && said.iter().any(|(name, data)| is_record_of(name, data, text)) {
                    *at = Some(Instant::now());
                }
            }
            if seen.iter().all(Option::is_some) {
                return Ok(seen.into_iter().flatten().collect());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the follow did not tell {texts:?} in {seconds} s: {:?}",
                    tail(&self.text(), 400)
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Calls `look` every second until it gives `Ok(Ok(found))`, up to
/// `seconds`: `what` is then said not to have happened, with the last
/// state `look` gave as `Ok(Err(state))`. An `Err` ends it at once.
fn wait_for<T>(
    what: &str,
    seconds: u64,
    mut look: impl FnMut() -> Result<Result<T, String>, String>,
) -> Result<T, String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match look()? {
            Ok(found) => return Ok(found),
            Err(state) if Instant::now() >= deadline => {
                return Err(format!("{what} did not happen in {seconds} s: {state}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// The events a stream of server-sent events said: `event: NAME` and `data:
/// JSON` lines, with a blank line after each event and a comment now and
/// then, after the answer's head. An event whose data has not come, or is not
/// JSON, is left out.
fn sse_events(text: &str) -> Vec<(String, Value)> {
    let mut named = None;
    let mut found = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("event: ") {
            named = Some(name);
        } else if let Some(data) = line.strip_prefix("data: ") {
            if let (Some(name), Ok(value)) = (named, serde_json::from_str(data)) {
                found.push((name.to_string(), value));
            }
            named = None;
        }
    }
    found
}

/// Whether the event `name` with `data` is a record whose text is `text`.
fn is_record_of(name: &str, data: &Value, text: &str) -> bool {
    name == "record" && data["text"] == text
}

/// The key that names a record of the log: its boot and its sequence number.
fn key_of(record: &Value) -> Option<(String, u64)> {
    Some((record["boot"].as_str()?.to_string(), record["seq"].as_u64()?))
}

/// Checks that `newer`, the records of a read made after the one that gave
/// `older`, tells the same log: each record of both is the same by its
/// boot and sequence number and in the same order, with what the log gained
/// between the reads at the end of `newer`, and what a window of the last
/// records then dropped from the start of `older`.
fn same_records(older: &[Value], newer: &[Value]) -> Result<(), String> {
    let keys = |records: &[Value]| -> Result<Vec<(String, u64)>, String> {
        records
            .iter()
            .map(|record| key_of(record).ok_or_else(|| format!("{record} has no boot and seq")))
            .collect()
    };
    let (older, newer) = (keys(older)?, keys(newer)?);
    if older.is_empty() && newer.is_empty() {
        return Ok(());
    }
    let common_in = |from: &[(String, u64)], other: &[(String, u64)]| {
        from.iter().filter(|key| other.contains(key)).cloned().collect::<Vec<_>>()
    };
    let (kept_older, kept_newer) = (common_in(&older, &newer), common_in(&newer, &older));
    if kept_older != kept_newer {
        return Err(format!("the reads have {older:?} and {newer:?}, in another order"));
    }
    let Some(first) = kept_older.first() else {
        // Nothing in common: all of `older` was dropped, which a window
        // does not do to the last record, or `older` had none.
        return if older.is_empty() {
            Ok(())
        } else {
            Err(format!("the second read has none of the first's records: {older:?}, {newer:?}"))
        };
    };
    let last = kept_older[kept_older.len() - 1].clone();
    // What only `older` has is before the first they share, and what only
    // `newer` has is after the last.
    let before_in_older = older.iter().position(|key| key == first).unwrap_or(0);
    let after_in_newer = newer.iter().position(|key| *key == last).map_or(0, |at| at + 1);
    let dropped = older.iter().filter(|key| !newer.contains(key)).count();
    let gained = newer.iter().filter(|key| !older.contains(key)).count();
    if dropped > before_in_older || gained > newer.len() - after_in_newer {
        return Err(format!("the reads differ inside: {older:?} and {newer:?}"));
    }
    Ok(())
}

/// Step 1: bob's read of a service's records is empty, and after `logger`
/// in his Terminal he reads his line, with only programs' records in
/// what he is given.
fn bob_reads_his_own(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let none = test.read(Who::Bob, "/api/1/logs?service=diskhealth")?;
    if none != json!([]) {
        return Err(format!("bob reads diskhealth's records: {none}"));
    }
    test.bob_types(vm, "logger lab-bob-line")?;
    let line = wait_for("bob's line in the log", 60, || {
        let records = test.read(Who::Bob, "/api/1/logs?grep=lab-bob-line&lines=20")?;
        let found = records.as_array().and_then(|all| {
            all.iter()
                .find(|record| record["text"] == "lab-bob-line" && record["from"] == "program")
        });
        Ok(found.cloned().ok_or_else(|| format!("he reads {records}")))
    })?;
    if line["from"] != "program" {
        return Err(format!("bob's line is not a program's record: {line}"));
    }
    let all = test.read(Who::Bob, "/api/1/logs?lines=1000")?;
    let records = all.as_array().ok_or_else(|| format!("bob reads {all}"))?;
    if let Some(other) = records.iter().find(|record| record["from"] != "program") {
        return Err(format!("bob is given a record that is no program's: {other}"));
    }
    let again = test.read(Who::Bob, "/api/1/logs?service=diskhealth")?;
    if again != json!([]) {
        return Err(format!("bob reads diskhealth's records after his line: {again}"));
    }
    Ok(())
}

/// Step 2: alice's read of the whole log is ADM-106 while she is locked,
/// and unlocked it is what `elevate log --all --output json` prints.
fn alice_reads_the_whole_log(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.get(Who::Alice, "/api/1/logs/all?lines=50&service=diskhealth")?.refused(
        "the whole log while locked",
        403,
        "ADM-106",
    )?;
    test.unlocked()?;
    let read = test.read(Who::Alice, "/api/1/logs/all?lines=50&service=diskhealth")?;
    let typed =
        test.pane.output(vm, "elevate log --all --output json --lines 50 --service diskhealth")?;
    let typed: Vec<Value> = typed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|err| format!("{line:?}: {err}")))
        .collect::<Result<_, _>>()?;
    let read = read.as_array().ok_or_else(|| format!("the whole log is {read}"))?;
    if read.is_empty() {
        return Err("diskhealth has no record in the whole log".into());
    }
    same_records(read, &typed).map_err(|why| format!("the route and the command differ: {why}"))
}

/// Step 3: a filter that cannot work is ADM-125 with its field, and a time
/// that is no number is ADM-108.
fn filters_that_cannot_work(test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let wrong = [
        ("lines=1001", "lines"),
        ("grep=%28", "grep"),
        ("service=diskhealth&kernel=true", "kernel"),
    ];
    for (query, field) in wrong {
        let reply = test.get(Who::Alice, &format!("/api/1/logs/all?{query}"))?;
        let said = reply.json().unwrap_or_default();
        if reply.status != 400
            || said["error"]["id"] != "ADM-125"
            || said["error"]["field"] != field
        {
            return Err(format!(
                "{query} was answered {} {}, not ADM-125 for {field}",
                reply.status, reply.body
            ));
        }
    }
    test.get(Who::Alice, "/api/1/logs/all?since=soon")?.refused("since=soon", 400, "ADM-108")?;
    Ok(())
}

/// Step 4: the follow of the whole log begins with its command, tells the
/// lines root writes as they come, and ends with `locked` when the session
/// locks, with its `log` gone from the guest and authd's audit saying so.
fn follows_and_is_locked(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let follow = test.follow(Who::Alice, "all=true&grep=lab-follow")?;
    let command = follow.wait_event("command", 30, |_| true)?;
    let shown = command["shown"].as_str().unwrap_or_default();
    if !shown.starts_with("elevate log ")
        || !shown.contains("--follow")
        || !shown.contains("lab-follow")
    {
        return Err(format!("the follow's command is {command}"));
    }
    // Three lines a second apart: a follow that waits for the end of the
    // steps would tell them together.
    answer_prompt(
        vm,
        "$",
        "clear; for i in 1 2 3; do elevate logger lab-follow-$i; sleep 1; done",
    )?;
    let came = follow.arrivals(&["lab-follow-1", "lab-follow-2", "lab-follow-3"], 90)?;
    let spread = came[2].duration_since(came[0]);
    if spread < Duration::from_secs(1) {
        return Err(format!("the follow told three lines a second apart within {spread:?}"));
    }
    if test.pane.status(vm, FOLLOW_RUNS)? != 0 {
        return Err("no log of alice's follow runs in the guest".into());
    }

    test.console.post(&mut test.alice, "/api/1/lock", &json!({}))?.must("alice's lock", 200)?;
    let ended = follow.wait_event("ended", 20, |_| true)?;
    if ended["why"] != json!("locked") {
        return Err(format!("the follow ended as {ended}, not locked"));
    }
    wait_for("the follow's log to end in the guest", 60, || {
        let status = test.pane.status(vm, FOLLOW_RUNS)?;
        Ok(if status == 1 { Ok(()) } else { Err(format!("pgrep says {status}")) })
    })?;
    follow_audited(vm, test)
}

/// What `pgrep` looks for, typed in the pane: the command authd starts for
/// the follow of `lab-follow`, whatever its place among the words. The
/// command line of root's process is root's to read, so pgrep runs as an
/// administrator; the pattern's `[f]` keeps it from finding elevate's own
/// command line, which has the pattern in it.
const FOLLOW_RUNS: &str = "elevate pgrep -f 'quiet --grep lab-[f]ollow' > /dev/null";

/// Waits up to 90 seconds for authd's audit to have the start of alice's
/// follow of the whole log, and its end by the lock: logd takes a while to
/// give authd's lines to the log.
fn follow_audited(vm: &mut Vm, test: &Test) -> Result<(), String> {
    elevated(vm)?;
    wait_for("authd's audit of the follow", 90, || {
        let audit = test.pane.output(
            vm,
            "elevate log --audit | grep -e 'follows the whole log' -e 'follow of the whole log'",
        )?;
        let began = follows_began(&audit);
        let Some(process) = began.first() else {
            return Ok(Err(format!("no start in {audit:?}")));
        };
        Ok(match follow_ended(&audit, *process) {
            Some(why) if why == "the console locked" => Ok(()),
            other => Err(format!("process {process} ended as {other:?}")),
        })
    })
}

/// The process of each follow of the whole log that authd's audit says
/// alice began in the console, oldest first.
fn follows_began(audit: &str) -> Vec<u64> {
    let start = "alice follows the whole log in the web console session ";
    audit
        .lines()
        .filter_map(|line| {
            let rest = &line[line.find(start)? + start.len()..];
            let after = &rest[rest.find(": process ")? + ": process ".len()..];
            after.split(' ').next()?.parse().ok()
        })
        .collect()
}

/// Why authd's audit says alice's follow of the whole log, process
/// `process`, ended.
fn follow_ended(audit: &str, process: u64) -> Option<String> {
    let start = format!("alice's follow of the whole log, process {process}, ended: ");
    audit.lines().find_map(|line| {
        let why = &line[line.find(&start)? + start.len()..];
        Some(why.trim().to_string())
    })
}

/// Step 5: a second follow of the same session ends the first with
/// `replaced`. In the lab's build the second ends after its minute with
/// `time`.
fn a_second_follow(test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let first = test.follow(Who::Alice, "all=true&grep=lab-follow")?;
    first.wait_event("command", 30, |_| true)?;
    let second = test.follow(Who::Alice, "all=true&grep=lab-follow")?;
    second.wait_event("command", 30, |_| true)?;
    let ended = first.wait_event("ended", 20, |_| true)?;
    if ended["why"] != json!("replaced") {
        return Err(format!("the first follow ended as {ended}, not replaced"));
    }
    if second.events().iter().any(|(name, _)| name == "ended") {
        return Err("the second follow ended with the first".into());
    }
    let actions = test.read(Who::Alice, "/api/1/actions")?;
    let lab = actions.as_array().is_some_and(|all| all.iter().any(|one| one["id"] == "lab.long"));
    if !lab {
        println!("admin-logs: this image is no lab build: its hour, not a minute, was not waited");
        return Ok(());
    }
    let ended = second.wait_event("ended", LAB_HOUR_ALLOWED, |_| true)?;
    if ended["why"] != json!("time") {
        return Err(format!(
            "the follow of a lab build, which lasts {LAB_HOUR} seconds, ended as {ended}"
        ));
    }
    Ok(())
}

/// Step 6: bob's follow has no `all`, and with it is ADM-122. It shows his
/// line and not the one root wrote before it, and no record that is not a
/// program's.
fn bobs_follow(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.get(Who::Bob, "/api/1/logs/follow?all=true")?.refused(
        "bob's follow of the whole log",
        403,
        "ADM-122",
    )?;
    let follow = test.follow(Who::Bob, "grep=lab-line")?;
    let command = follow.wait_event("command", 30, |_| true)?;
    let shown = command["shown"].as_str().unwrap_or_default();
    if shown.starts_with("elevate") || !shown.contains("--follow") {
        return Err(format!("bob's follow runs {command}"));
    }
    // Root's line is written first, so a stream that let it through would
    // have told it before bob's.
    if test.pane.status(vm, "elevate logger lab-line-root")? != 0 {
        return Err("root's line was not written".into());
    }
    test.bob_types(vm, "logger lab-line-bob")?;
    follow.wait_event("record", 60, |data| data["text"] == "lab-line-bob")?;
    let others: Vec<Value> = follow
        .events()
        .into_iter()
        .filter(|(name, data)| name == "record" && data["text"] != "lab-line-bob")
        .map(|(_, data)| data)
        .collect();
    if let Some(other) = others.first() {
        return Err(format!("bob's follow told a record that is not his line: {other}"));
    }
    if follow.events().iter().any(|(name, data)| name == "record" && data["from"] != "program") {
        return Err("bob's follow told a record that is no program's".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(boot: &str, seq: u64) -> Value {
        json!({"boot": boot, "seq": seq, "from": "service", "name": "diskhealth", "text": "x"})
    }

    fn records(boot: &str, seqs: &[u64]) -> Vec<Value> {
        seqs.iter().map(|seq| record(boot, *seq)).collect()
    }

    #[test]
    fn the_events_of_a_follow_are_read_after_the_head_by_name_and_data() {
        let text = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                    event: command\ndata: {\"shown\":\"elevate log\"}\n\n\
                    : keep\n\n\
                    event: record\ndata: {\"text\":\"a\",\"from\":\"program\"}\n\n\
                    event: skipped\ndata: {\"count\":7}\n\n\
                    event: ended\ndata: {\"why\":\"locked\"}\n\n\
                    event: record\ndata: {\"text\":";
        let events = sse_events(text);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["command", "record", "skipped", "ended"]);
        assert_eq!(events[0].1["shown"], "elevate log");
        assert_eq!(events[3].1["why"], "locked");
        assert!(sse_events("").is_empty());
        assert!(sse_events("event: ended\ndata: not json\n\n").is_empty());
    }

    #[test]
    fn a_record_event_is_the_line_by_its_text_alone() {
        let data = json!({"text": "lab-follow-1", "from": "program"});
        assert!(is_record_of("record", &data, "lab-follow-1"));
        assert!(!is_record_of("record", &data, "lab-follow-2"));
        assert!(!is_record_of("skipped", &data, "lab-follow-1"));
        // A line that only holds the words is another record.
        let audit = json!({"text": "alice ran logger lab-follow-1"});
        assert!(!is_record_of("record", &audit, "lab-follow-1"));
    }

    #[test]
    fn two_reads_of_a_log_are_the_same_by_boot_and_sequence_number() {
        let same = records("this", &[3, 4, 5]);
        assert!(same_records(&same, &same).is_ok());
        assert!(same_records(&[], &[]).is_ok());
        // What came between the reads is at the end of the second.
        assert!(same_records(&same, &records("this", &[3, 4, 5, 6])).is_ok());
        assert!(same_records(&[], &records("this", &[6])).is_ok());
        // A window of the last records drops the first of the older.
        assert!(same_records(&same, &records("this", &[4, 5, 6])).is_ok());
        // Another boot's record is another record.
        assert!(same_records(&same, &records("other", &[3, 4, 5])).is_err());
        // A record that went, in the middle, or in the other order.
        assert!(same_records(&same, &records("this", &[3, 5])).is_err());
        assert!(same_records(&same, &records("this", &[5, 4, 3])).is_err());
        assert!(same_records(&same, &records("this", &[3, 4])).is_err());
        assert!(same_records(&same, &[]).is_err());
        // One that came in the middle.
        assert!(same_records(&records("this", &[3, 5]), &records("this", &[3, 4, 5])).is_err());
        // A record with no key is no record of the log.
        assert!(same_records(&[json!({"text": "x"})], &same).is_err());
    }

    #[test]
    fn authds_audit_names_the_follows_alice_began_and_why_they_ended() {
        let audit = "Nov 14 10:06:01 host authd: alice follows the whole log in the web console \
                     session 5d3a9c1e: process 91 runs elevate log --output json\n\
                     Nov 14 10:07:30 host authd: alice's follow of the whole log, process 91, \
                     ended: the console locked\n\
                     Nov 14 10:08:00 host authd: alice follows the whole log in the web console \
                     session 5d3a9c1e: process 120 runs elevate log --output json\n";
        assert_eq!(follows_began(audit), [91, 120]);
        assert_eq!(follow_ended(audit, 91).as_deref(), Some("the console locked"));
        assert_eq!(follow_ended(audit, 120), None);
        assert_eq!(follow_ended(audit, 9), None);
        assert!(follows_began("").is_empty());
        assert!(
            follows_began(
                "authd: bob follows the whole log in the web console session x: process 3"
            )
            .is_empty()
        );
    }

    #[test]
    fn the_record_s_name_is_its_boot_and_sequence_number() {
        assert_eq!(key_of(&record("this", 7)), Some(("this".to_string(), 7)));
        assert_eq!(key_of(&json!({"boot": "this"})), None);
        assert_eq!(key_of(&json!({"seq": 1})), None);
    }

    #[test]
    fn a_wait_gives_what_it_found_or_the_last_state_it_saw() {
        let mut calls = 0;
        let found = wait_for("a number", 5, || {
            calls += 1;
            Ok(if calls == 3 { Ok(calls) } else { Err(format!("call {calls}")) })
        });
        assert_eq!(found, Ok(3));
        let never: Result<(), String> = wait_for("nothing", 0, || Ok(Err("still no".into())));
        assert_eq!(never, Err("nothing did not happen in 0 s: still no".into()));
        let failed: Result<(), String> = wait_for("it", 5, || Err("gone".into()));
        assert_eq!(failed, Err("gone".into()));
    }
}
