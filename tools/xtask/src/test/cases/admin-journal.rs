// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What changed in the web console, Dukovo Admin: the journal, Keep it,
//! Put back and tasks (ADR 0516). Alice, the owner, changes the service
//! file of the example service, diskhealth's, three ways, and the
//! console's "What changed" tells each as it is.
//!
//! A console change: the route that enables the service writes its service
//! file, and the journal has a row for it at once: `console`, alice, status
//! 0, the file's text before and after, and a diff that adds the line
//! enabled = true. Put back, POST /api/1/changes/N/put-back, runs `admin
//! put-back N` as an administrator for her unlocked session: the answer
//! shows that command, the file is as it was, the running service is asked
//! to read it again, and the put back has a row of its own and a line in
//! admind's log. Keep it is for hand edits, so a console change is ADM-128,
//! and a change the journal does not have is ADM-127.
//!
//! A hand edit that no command names: `elevate service diskhealth disable`
//! in alice's pane writes the same file, but its command names the service
//! and not the file. The round that looks every 30 seconds adds a `by hand`
//! row for it, "outside the console", with the diff. Keep it marks it seen
//! by alice, and marking it again changes nothing. Put back writes the file
//! as it was, and says the stopped service reads it when it starts.
//!
//! A hand edit that names the file: alice copies a changed copy over it
//! with `elevate cp`, which authd records with the file's path. admind
//! asks logd for authd's audit lines, which only root reads, and the row
//! says alice did it. Typed in the pane as the page shows it, `elevate
//! admin put-back N` puts it back as root, reading the store that admind
//! has open, and the file is as it was. That write is a hand edit too and
//! is the third `by hand` row; the console's own writes, the enable and the
//! two put backs, are none, so the file has three and no more.
//!
//! The command admin export /tmp/dukovo-export, typed by alice without
//! elevate, makes the settings a Git repository of one commit, "Settings of
//! NAME at TIME", by the machine's name and an address that is no person's,
//! with the service file as it is on the machine and a SECRETS.md.
//!
//! Tasks need the lab's action, `lab.long`, and a build for people does not
//! offer it. The image this test boots is such a build, as the case `admin`
//! says of the lab's route, so there the test asks only that GET
//! /api/1/tasks is a list, that stopping a task there is not is ADM-127,
//! and that the lab's route is not there. Where the action is offered, in a
//! lab build, it runs as a task for 60 seconds: the console answers 202
//! with the task's number; while it runs the task shows its step and words
//! and a log, in the list and at its own address, and its command runs in
//! the guest; Stop answers 200, ends the command at once as `stopped by
//! alice`, and the journal has the action's row with the task's number. A
//! second task is running when `elevate service admind restart` restarts
//! admind: the new admind has it `stopped`, "the console restarted", says
//! so in its log, and its command is gone from the guest.
//!
//! What the pane's commands print reaches the host through nc, to a port on
//! the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;

/// The service the test changes: every image runs it and nothing needs it.
const SERVICE: &str = "diskhealth";

/// Its service file, which the console's enable and disable write.
const FILE: &str = "/system/settings/spark/services/diskhealth.toml";

/// Where admind's log says the host's requests come from.
const PEER: &str = "10.0.2.2";

/// The test.
pub const CASE: Case = Case {
    name: "admin-journal",
    about: "The web console's journal of what changed: enabling the example service through its route is a console row with the file's diff, and Put back writes the file as it was, with admin put-back N and a row of its own; the same file changed by hand is a by hand row, outside the console when no elevated command named the file and alice when one did, which Keep it marks seen and Put back undoes, through the route and typed as the page shows it, and the console's own writes are never taken for hand edits; admin export makes the settings a repository of one commit by the machine; where the build offers the lab's long action, a task shows its steps and log and is stopped by alice, and one running when admind restarts ends as stopped, the console restarted (ADR 0516)",
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
    script: admin_journal,
    disks: &[],
    second: None,
};

fn admin_journal(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        said: Vec::new(),
    };
    elevated(vm)?;
    test.sign_in()?;

    let original = test.file(vm)?;
    if original.contains("enabled = ") || original.contains("# edited by hand") {
        return Err(format!("{FILE} is not as the image has it before any change: {original:?}"));
    }
    let latest = test.newest()?;
    test.read("/api/1/changes/999999")
        .err()
        .filter(|why| why.contains("404") && why.contains("ADM-127"))
        .ok_or("a change that is not in the journal is not ADM-127")?;
    console_change(vm, &mut test, &original)?;
    hand_edits(vm, &mut test, &original, latest)?;
    export(vm, &test, &original)?;
    tasks(vm, &mut test)?;
    the_log(vm, &test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// What the test keeps from one section to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
    /// The lines admind's log must have, each without its `admind: `.
    said: Vec<String>,
}

impl Test {
    /// Signs alice in, as the console's page does, with a session of its
    /// own: the one before it, if there was one, is forgotten.
    fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        Ok(())
    }

    /// Keeps alice's session unlocked: at least two minutes are left, or
    /// she gives her password again.
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

    /// What the console answers to a read of `path`, which must be 200.
    fn read(&mut self, path: &str) -> Result<Value, String> {
        let reply = self.console.get(&mut self.alice, path)?;
        if reply.status != 200 {
            let id =
                reply.json().ok().and_then(|body| body["error"]["id"].as_str().map(str::to_string));
            return Err(format!("{path} was answered {} {}", reply.status, id.unwrap_or_default()));
        }
        reply.json()
    }

    /// A change of `path`, asked as the page does with no arguments, whatever
    /// the answer is.
    fn post(&mut self, path: &str) -> Result<Reply, String> {
        self.console.post(&mut self.alice, path, &json!({}))
    }

    /// Notes a line admind's log must have: alice asked for `what`, and it
    /// ended with status 0.
    fn said(&mut self, what: &str) {
        self.said.push(format!("alice from {PEER}: {what} ended with status 0"));
    }

    /// The journal's newest rows, newest first, at most 200.
    fn changes(&mut self) -> Result<Vec<Value>, String> {
        let list = self.read("/api/1/changes?limit=200")?;
        list["changes"].as_array().cloned().ok_or_else(|| format!("no changes in {list}"))
    }

    /// The number of the journal's newest row, 0 when it has none.
    fn newest(&mut self) -> Result<i64, String> {
        let rows = self.changes()?;
        Ok(rows.first().and_then(|row| row["id"].as_i64()).unwrap_or(0))
    }

    /// The rows made after row `after`, oldest first.
    fn rows_after(&mut self, after: i64) -> Result<Vec<Value>, String> {
        let mut rows = self.changes()?;
        rows.retain(|row| row["id"].as_i64().is_some_and(|id| id > after));
        rows.reverse();
        Ok(rows)
    }

    /// The change `id`, with its diffs.
    fn detail(&mut self, id: i64) -> Result<Value, String> {
        self.read(&format!("/api/1/changes/{id}"))
    }

    /// The text of the service file, read in the pane: it is for everyone
    /// to read.
    fn file(&self, vm: &mut Vm) -> Result<String, String> {
        self.pane.output(vm, &format!("cat {FILE}"))
    }

    /// Waits up to `seconds` for a `by hand` row after row `after` that
    /// names the service file: the round that finds hand edits runs every
    /// 30 seconds.
    fn by_hand(&mut self, after: i64, seconds: u64) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let rows = self.rows_after(after)?;
            if let Some(row) = rows.into_iter().find(|row| row["how"] == "by hand" && names(row)) {
                return Ok(row);
            }
            if Instant::now() >= deadline {
                return Err(format!("no by hand row for {FILE} came in {seconds} s"));
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    }

    /// The newest row after row `after` that is the console's change
    /// `action`: it is made before the change's answer comes.
    fn console_row(&mut self, after: i64, action: &str) -> Result<Value, String> {
        let rows = self.rows_after(after)?;
        rows.into_iter()
            .rev()
            .find(|row| row["action"] == action)
            .ok_or_else(|| format!("the journal has no row for {action} after row {after}"))
    }

    /// Waits up to 30 seconds for the service to be running or not.
    fn running(&mut self, wanted: bool) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let list = self.read("/api/1/services")?;
            let state = list["services"]
                .as_array()
                .and_then(|all| all.iter().find(|service| service["name"] == SERVICE))
                .and_then(|service| service["state"].as_str())
                .ok_or_else(|| format!("the list of services has no {SERVICE}"))?
                .to_string();
            if (state == "running") == wanted && !matches!(state.as_str(), "starting" | "stopping")
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{SERVICE} is {state} 30 s after the change, wanted running {wanted}"
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// The task `id` once `ready` is true of it, within `seconds`.
    fn task_where(
        &mut self,
        id: i64,
        seconds: u64,
        ready: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let task = self.read(&format!("/api/1/tasks/{id}"))?;
            if ready(&task) {
                return Ok(task);
            }
            if Instant::now() >= deadline {
                return Err(format!("task {id} is not as wanted after {seconds} s: {task}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Puts change `id` back through the console, unlocked, and returns
    /// the answer, which shows the command that ran.
    fn put_back(&mut self, id: i64) -> Result<Value, String> {
        self.unlocked()?;
        let reply = self.post(&format!("/api/1/changes/{id}/put-back"))?;
        let answer = reply.must(&format!("the put back of change {id}"), 200)?.json()?;
        let command = format!("elevate admin put-back {id}");
        if answer["done"] != json!(true) || answer["command"] != json!(command) {
            return Err(format!("the put back of change {id} was answered {answer}"));
        }
        let put = answer["change"].as_i64().ok_or("the put back has no row of its own")?;
        let row = self.detail(put)?;
        if row["action"] != "changes.put-back" || row["how"] != "console" || row["who"] != "alice" {
            return Err(format!("the put back's own row is {row}"));
        }
        self.said(&format!("changes.put-back {id}"));
        Ok(answer)
    }
}

/// Whether the row names the service file among its files.
fn names(row: &Value) -> bool {
    row["files"].as_array().is_some_and(|files| files.iter().any(|file| file["path"] == FILE))
}

/// The diff of the service file in `detail`, a change as one's address
/// gives it, or what is said of it if it has none.
fn diff_of(detail: &Value) -> String {
    let file =
        detail["files"].as_array().and_then(|files| files.iter().find(|f| f["path"] == FILE));
    match file {
        Some(file) => file["diff"].as_str().unwrap_or(&file.to_string()).to_string(),
        None => format!("no {FILE} in {detail}"),
    }
}

/// What a put back's answer says, whole: its lines of output together.
fn output_of(answer: &Value) -> String {
    let lines = answer["output"].as_array().into_iter().flatten();
    lines.filter_map(Value::as_str).collect::<Vec<_>>().join("\n")
}

/// Checks that the service file reads `original` again.
fn is_back(test: &Test, vm: &mut Vm, original: &str, what: &str) -> Result<(), String> {
    let now = test.file(vm)?;
    if now != original {
        return Err(format!("{FILE} is not as it was after {what}: {now:?}, not {original:?}"));
    }
    Ok(())
}

/// A change through the console: the route that enables the service
/// writes its file, and the journal has the row with the diff. Put back
/// writes the file as it was, and asks the running service to read it.
fn console_change(vm: &mut Vm, test: &mut Test, original: &str) -> Result<(), String> {
    test.unlocked()?;
    let before = test.newest()?;
    let reply = test.post(&format!("/api/1/services/{SERVICE}/enable"))?;
    if reply.must("the enable", 200)?.json()?["done"] != json!(true) {
        return Err("the enable does not say it is done".into());
    }
    test.said("service.enable");
    let row = test.console_row(before, "service.enable")?;
    let id = row["id"].as_i64().ok_or("a row has no number")?;
    let fields = (row["who"].as_str(), row["how"].as_str(), row["status"].as_i64());
    if fields != (Some("alice"), Some("console"), Some(0)) || !names(&row) {
        return Err(format!("the row of the enable is {row}"));
    }
    let detail = test.detail(id)?;
    let diff = diff_of(&detail);
    if !diff.contains("\n+enabled = true") || diff.contains("\n-enabled") {
        return Err(format!("the diff of the enable is {diff:?}, which should add enabled = true"));
    }
    let command = format!("elevate admin put-back {id}");
    if detail["put_back"]["possible"] != json!(true)
        || detail["put_back"]["command"] != json!(command)
    {
        return Err(format!("the enable cannot be put back as {command}: {}", detail["put_back"]));
    }
    // Keep it is for hand edits: this one is in the journal already.
    test.post(&format!("/api/1/changes/{id}/keep"))?.refused(
        "keeping a console change",
        409,
        "ADM-128",
    )?;
    test.post("/api/1/changes/999999/keep")?.refused("keeping no change", 404, "ADM-127")?;

    let answer = test.put_back(id)?;
    let said = output_of(&answer);
    if !said.contains(&format!("put back {FILE}")) || !said.contains("asked diskhealth to reload") {
        return Err(format!(
            "the put back says {said:?}, not that it wrote the file and asked the service"
        ));
    }
    is_back(test, vm, original, "the put back of the enable")
}

/// Hand edits of the same file: one no command names, one a command does,
/// each put back, one through the console and one as the page shows it.
fn hand_edits(vm: &mut Vm, test: &mut Test, original: &str, first: i64) -> Result<(), String> {
    // 1. `service disable` names the service and not its file.
    let before = test.newest()?;
    let status = test.pane.status(vm, &format!("elevate service {SERVICE} disable"))?;
    if status != 0 {
        return Err(format!("service {SERVICE} disable ended with status {status}"));
    }
    let row = test.by_hand(before, 100)?;
    let id = row["id"].as_i64().ok_or("a row has no number")?;
    if row["who"] != "outside the console"
        || row["status"] != Value::Null
        || row["action"] != Value::Null
    {
        return Err(format!("a hand edit no command names is told as {row}"));
    }
    let diff = diff_of(&test.detail(id)?);
    if !diff.contains("\n+enabled = false") {
        return Err(format!(
            "the diff of the hand edit is {diff:?}, which should add enabled = false"
        ));
    }
    keep_it(test, id)?;
    // The service is stopped by the edit, so it reads its file when it
    // starts.
    let answer = test.put_back(id)?;
    if !output_of(&answer).contains("diskhealth is not running") {
        return Err(format!("the put back of a stopped service says {}", output_of(&answer)));
    }
    is_back(test, vm, original, "the put back of the hand edit")?;
    test.unlocked()?;
    test.post(&format!("/api/1/services/{SERVICE}/start"))?.must("the start", 200)?;
    test.said("service.start");
    test.running(true)?;

    // 2. A copy changed and copied over the file, `elevate cp`, whose
    // command names the file: authd's audit line has it.
    let before = test.newest()?;
    let copy = format!(
        "cp {FILE} /tmp/hand.toml && echo \"# edited by hand\" >> /tmp/hand.toml && elevate cp \
         /tmp/hand.toml {FILE}"
    );
    let status = test.pane.status(vm, &copy)?;
    if status != 0 {
        return Err(format!("the copy over {FILE} ended with status {status}"));
    }
    let row = test.by_hand(before, 100)?;
    let id = row["id"].as_i64().ok_or("a row has no number")?;
    if row["who"] != "alice" {
        return Err(format!(
            "a hand edit that an elevated command of alice's names is told as {row}: admind \
             could not name her from authd's audit lines"
        ));
    }
    let detail = test.detail(id)?;
    if !diff_of(&detail).contains("\n+# edited by hand") {
        return Err(format!("the diff of the copy is {:?}", diff_of(&detail)));
    }
    // 3. The command the page shows for the put back, typed in the pane.
    let command = detail["put_back"]["command"].as_str().ok_or("no command to put it back")?;
    if command != format!("elevate admin put-back {id}") {
        return Err(format!("the page shows {command:?} for change {id}"));
    }
    let status = test.pane.status(vm, command)?;
    if status != 0 {
        return Err(format!("`{command}` ended with status {status}"));
    }
    is_back(test, vm, original, "the shown command")?;

    // The pane's put back is a hand edit as any other, and the third row;
    // the console's two, and the enable, are none.
    let third = test.by_hand(id, 100)?;
    let rows: Vec<Value> = test
        .rows_after(first)?
        .into_iter()
        .filter(|row| row["how"] == "by hand" && names(row))
        .collect();
    if rows.len() != 3 || rows[2]["id"] != third["id"] {
        return Err(format!(
            "the file has {} rows by hand, not the three the test made",
            rows.len()
        ));
    }
    Ok(())
}

/// `admin export` in alice's pane, with her own rights: the settings are a
/// Git repository of one commit, which the machine made and no person,
/// with the service file as it is on the machine, and SECRETS.md.
fn export(vm: &mut Vm, test: &Test, original: &str) -> Result<(), String> {
    let said = test.pane.output(vm, "admin export /tmp/dukovo-export")?;
    if !said.contains("files copied to /tmp/dukovo-export") || !said.contains("committed ") {
        return Err(format!("admin export said {said:?}"));
    }
    let log = test.pane.output(vm, "git -C /tmp/dukovo-export log --format='%an <%ae>|%s'")?;
    let lines: Vec<&str> = log.lines().collect();
    let made = lines.first().is_some_and(|line| {
        line.starts_with("Dukovo on ")
            && line.contains("@")
            && line.contains(".invalid>|Settings of ")
            && line.contains(" UTC")
    });
    if lines.len() != 1 || !made {
        return Err(format!("the export is not one commit of the machine's: {log:?}"));
    }
    let copy = test.pane.output(vm, &format!("cat /tmp/dukovo-export{FILE}"))?;
    if copy != original {
        return Err(format!("the exported {FILE} is {copy:?}, not {original:?}"));
    }
    let left_out = test.pane.output(vm, "cat /tmp/dukovo-export/SECRETS.md")?;
    if !left_out.starts_with("# Left out of the export of ") {
        return Err(format!("SECRETS.md of the export is {left_out:?}"));
    }
    Ok(())
}

/// Marks the hand edit `id` seen, as alice, and again, which changes
/// nothing.
fn keep_it(test: &mut Test, id: i64) -> Result<(), String> {
    test.unlocked()?;
    let path = format!("/api/1/changes/{id}/keep");
    let kept = test.post(&path)?.must("keeping a hand edit", 200)?.json()?;
    if kept["seen"]["by"] != "alice" || !kept["seen"]["at"].is_i64() {
        return Err(format!("the hand edit kept is {kept}"));
    }
    let again = test.post(&path)?.must("keeping it again", 200)?.json()?;
    if again["seen"] != kept["seen"] {
        return Err(format!("keeping it again made it {again}"));
    }
    let listed = test.detail(id)?;
    if listed["seen"] != kept["seen"] {
        return Err(format!("the journal has it as {listed} after Keep it"));
    }
    let line = format!("alice from {PEER} kept change {id}");
    test.said.push(line);
    Ok(())
}

/// The lab's action, as a task: or, where this build does not offer it,
/// what the console says of tasks that there are none of.
fn tasks(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let list = test.read("/api/1/tasks")?;
    if list["tasks"].as_array().is_none() {
        return Err(format!("GET /api/1/tasks gave {list}"));
    }
    let actions = test.read("/api/1/actions")?;
    let offered =
        actions.as_array().is_some_and(|all| all.iter().any(|action| action["id"] == "lab.long"));
    if !offered {
        test.post("/api/1/tasks/1/stop")?.refused(
            "stopping a task that is not there",
            404,
            "ADM-127",
        )?;
        test.post("/api/1/lab/long")?.refused("the lab's route", 404, "ADM-116")?;
        println!("admin-journal: this image does not offer lab.long: no task was run");
        return Ok(());
    }
    if !list["tasks"].as_array().is_some_and(Vec::is_empty) {
        return Err(format!("the console has tasks before any was run: {list}"));
    }
    a_task_is_stopped(vm, test)?;
    a_task_when_admind_restarts(vm, test)
}

/// Runs the long action for 60 seconds, watches it from the list and its
/// own address, and stops it.
fn a_task_is_stopped(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let before = test.newest()?;
    let reply = test.console.post(&mut test.alice, "/api/1/lab/long", &json!({"seconds": 60}))?;
    let id =
        reply.must("the long action", 202)?.json()?["task"].as_i64().ok_or("no task number")?;
    let task = test
        .task_where(id, 30, |task| task["state"] == "running" && task["step"].as_i64() > Some(0))?;
    if task["steps"] != json!(4) || task["who"] != "alice" || task["how"] != "console" {
        return Err(format!("the task is {task}"));
    }
    if task["words"].as_str().is_none_or(str::is_empty) || task["action"] != "lab.long" {
        return Err(format!("the task has no words for its step: {task}"));
    }
    let listed = test.read("/api/1/tasks")?;
    let in_list =
        listed["tasks"].as_array().is_some_and(|all| all.iter().any(|t| t["id"] == json!(id)));
    if !in_list {
        return Err(format!("the task list does not have task {id}: {listed}"));
    }
    let own = test.read(&format!("/api/1/tasks/{id}"))?;
    if own["log"].as_array().is_none_or(Vec::is_empty) {
        return Err(format!("the running task has no log: {own}"));
    }
    if test.pane.status(vm, "pgrep -f lab-steps > /dev/null")? != 0 {
        return Err("no lab-steps command runs in the guest for the running task".into());
    }

    let stopping = test.post(&format!("/api/1/tasks/{id}/stop"))?.must("the stop", 200)?.json()?;
    if stopping["stopping"] != json!(true) {
        return Err(format!("the stop was answered {stopping}"));
    }
    let task = test.task_where(id, 40, |task| task["state"] != "running")?;
    if task["state"] != "stopped" || task["note"] != "stopped by alice" || task["ended"].is_null() {
        return Err(format!("the stopped task is {task}"));
    }
    test.post(&format!("/api/1/tasks/{id}/stop"))?.refused(
        "stopping an ended task",
        409,
        "ADM-128",
    )?;
    if test.pane.status(vm, "pgrep -f lab-steps > /dev/null")? != 1 {
        return Err("the command of the stopped task still runs in the guest".into());
    }
    // The row of the action is made when the task ends, with its number.
    let row = test.console_row(before, "lab.long")?;
    if row["task"] != json!(id) || row["who"] != "alice" || row["how"] != "console" {
        return Err(format!("the row of the task is {row}"));
    }
    test.said.push(format!("alice from {PEER}: asked task {id} to stop"));
    Ok(())
}

/// Runs the long action again and restarts admind while it runs: the
/// task is `stopped`, "the console restarted", and its command is gone.
fn a_task_when_admind_restarts(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let reply = test.console.post(&mut test.alice, "/api/1/lab/long", &json!({"seconds": 120}))?;
    let id = reply.must("the second long action", 202)?.json()?["task"]
        .as_i64()
        .ok_or("no task number")?;
    test.task_where(id, 30, |task| task["state"] == "running")?;
    elevated(vm)?;
    let status = test.pane.status(vm, "elevate service admind restart")?;
    if status != 0 {
        return Err(format!("service admind restart ended with status {status}"));
    }
    // The sessions are gone with the old admind; the console is up again
    // within a minute and signs alice in once more.
    let deadline = Instant::now() + Duration::from_secs(60);
    while test.console.get(&mut Browser::default(), "/api/1/health").map(|reply| reply.status)
        != Ok(200)
    {
        if Instant::now() >= deadline {
            return Err("the console does not answer a minute after admind's restart".into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    test.sign_in()?;
    let task =
        test.task_where(id, 30, |task| task["state"] != "running" && task["state"] != "waiting")?;
    if task["state"] != "stopped"
        || task["note"] != "the console restarted"
        || task["ended"].is_null()
    {
        return Err(format!("the task the restart met is {task}"));
    }
    let log = vm.log();
    if !log.contains("admind: 1 task left by the last admind ended as stopped") {
        return Err("admind's log does not say the task left by the last admind ended".into());
    }
    // The old admind's relay closed, which authd takes as the terminal
    // going: the command is hung up, within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(30);
    while test.pane.status(vm, "pgrep -f lab-steps > /dev/null")? != 1 {
        if Instant::now() >= deadline {
            return Err("the command of the task the restart met still runs 30 s after".into());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(())
}

/// admind's log has a line for each change asked for, and for each Keep it
/// and stop.
fn the_log(vm: &mut Vm, test: &Test) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let log = vm.log();
        let missing: Vec<&String> =
            test.said.iter().filter(|line| !log.contains(&format!("admind: {line}"))).collect();
        if missing.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("admind's log lacks the lines {missing:?}"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_names_the_file_it_changed() {
        let row = json!({"files": [{"path": "/etc/other"}, {"path": FILE}]});
        assert!(names(&row));
        assert!(!names(&json!({"files": [{"path": "/etc/other"}]})));
        assert!(!names(&json!({"files": []})));
        assert!(!names(&json!({})));
    }

    #[test]
    fn the_diff_and_the_output_are_read_from_the_answers() {
        let detail = json!({"files": [{"path": FILE, "diff": "--- a\n+++ b\n+enabled = true\n"}]});
        assert!(diff_of(&detail).contains("\n+enabled = true"));
        assert!(diff_of(&json!({"files": []})).contains("no /system"));
        let answer = json!({"output": ["put back x", "asked y to reload"]});
        assert_eq!(output_of(&answer), "put back x\nasked y to reload");
        assert_eq!(output_of(&json!({})), "");
    }
}
