// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Report a problem of the web console, Dukovo Admin: a crash is
//! listed, a report is drafted, read and sent to a reports server, where
//! it is kept as it was read, a phone is given the text once, a report
//! waits for the server and is sent when it is back, and the security box
//! says when it has no key. alice, the owner, is an administrator; bob may
//! use the console and is no administrator. Every action here runs as the
//! person who asks, as the commands do. The reports server is the one of
//! the problem-report test, lab/tests/reports/report-server.py, on the
//! host, with a certificate from an authority made for the run; reportd
//! reads report.toml(5), which names it, once it is restarted.
//!
//! A crash. A service of the test's ends with SIGSEGV. `GET
//! /api/1/reports/crashes` lists it for alice, with its program, its kind
//! and the time, the same document as the command `report crashes --json`
//! prints in her pane; bob, whose own programs did not crash, is shown
//! none.
//!
//! A draft. `POST /api/1/reports/drafts` for a report about something else
//! answers the draft: its ID, the SHA-256 of its text, which is the hash
//! of the text it holds, the text, how many marks of each kind redaction
//! put in, and the size of each part. It is a read: the journal has no
//! row for it, no task is made, and the log does not hold the words that
//! were typed.
//!
//! A send. A draft about the crash, read as it is, is sent with its hash
//! and a title. With another hash it is RPT-102 and is kept; for bob, who
//! does not hold it, RPT-101. Sent, it is a change in the journal with
//! alice's name, and the server keeps the bytes of the draft's text, no
//! more and no less. The draft is gone: a second send of it is RPT-101.
//! The list, `GET /api/1/reports`, has the report as sent, and what the
//! command `report list --json` prints is the same. The status, `GET
//! /api/1/reports/status`, has what the server says, which is received
//! and, once the server has fixed it, in which version; the crash is
//! marked as reported. Then `POST /api/1/reports/1/delete` takes the
//! report off the server and out of the list.
//!
//! The phone. A draft's text is made a link that works once: `POST
//! /api/1/reports/drafts/DRAFT/phone` with the hash answers its address, when
//! it ends and the fingerprint of the certificate the console serves, a
//! hash that is not the draft's is RPT-102, and `GET /once/TOKEN/report.txt`
//! with no session gets the text as a file to keep, a second time is 410
//! and a token that was never made 404, both with ADM-140.
//!
//! With the server away. A send is queued, not lost: the answer says it
//! is queued and why, the list has it as queued with the words of the
//! last try, and when the server is back `POST /api/1/reports/retry`
//! sends it, and the server has its text.
//!
//! The security box. With report.toml(5) naming a file of the security
//! team's keys that is not there, a report sent for the box is RPT-103 and
//! nothing is queued.
//!
//! What the pane's commands print reaches the host through nc, to a port
//! on the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;
use super::problem_report::{CRASH_SECONDS, Host, SEND_SECONDS, run, set_up};

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// Where the test keeps its files on the host, from the top of the tree.
const DIR: &str = "out/test/admin-reports";

/// The words alice types in the draft that leaves no trace, which no log
/// may hold.
const TYPED: &str = "DRAFT-WORDS that nothing may keep";

/// The file of the security team's keys that the last step names, which
/// is not there.
const NO_KEYS: &str = "/system/settings/report-no-keys.txt";

/// The title of the report that is sent, and of the one that waits.
const TITLE: &str = "The crash-test service stopped";
const WAITING_TITLE: &str = "A report that waits for the server";

/// The test.
pub const CASE: Case = Case {
    name: "admin-reports",
    about: "The web console's Report a problem: reports.crashes lists a service's crash for alice as report crashes --json prints it, and none for bob; reports.draft answers the kept draft with a hash that is its text's, and leaves no row in the journal, no task and none of the typed words in the log; a draft about the crash is sent with its hash and a title, with another hash it is RPT-102 and for bob RPT-101, sent it is a change in the journal and the server keeps the draft's bytes, a second send is RPT-101, reports.list and reports.status say what report list and the server say, the crash is reported, and reports.delete takes the report off the server; the phone link gives the text once with no session, then 410 and for a token never made 404, both ADM-140; with the server away a send is queued with why, reports.list says queued, and reports.retry sends it when the server is back; with the security team's key file missing a report for the box is RPT-103",
    firmware: Firmware::Uefi,
    // As the problem-report test: an installed system's disk.
    media: Media::Disk,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_reports,
    disks: &[],
    second: None,
};

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

/// A draft reportd keeps, as `reports.draft` answers it.
struct Draft {
    id: String,
    sha256: String,
    text: String,
    /// The whole answer.
    answer: Value,
}

impl Test {
    /// Opens the console and the pane once the machine has its network and
    /// admind answers.
    fn open(vm: &mut Vm) -> Result<Test, String> {
        vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
        vm.expect_anywhere("admind: answers at ")?;
        Ok(Test {
            console: Console::open(vm)?,
            pane: Pane::open()?,
            alice: Browser::default(),
            bob: Browser::default(),
        })
    }

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

    /// A change at `path` with `body`, for `who`, whatever the answer.
    fn post(&mut self, who: Who, path: &str, body: &Value) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.post(browser, path, body)
    }

    /// What `command`, which prints JSON, prints in the pane.
    fn typed(&self, vm: &mut Vm, command: &str) -> Result<Value, String> {
        let text = self.pane.output(vm, command)?;
        serde_json::from_str(text.trim())
            .map_err(|err| format!("{command} printed {text:?}: {err}"))
    }

    /// The journal's newest rows, newest first, at most 200.
    fn changes(&mut self) -> Result<Vec<Value>, String> {
        let list = self.read(Who::Alice, "/api/1/changes?limit=200")?;
        list["changes"].as_array().cloned().ok_or_else(|| format!("no changes in {list}"))
    }

    /// The number of the journal's newest row, 0 when it has none.
    fn newest(&mut self) -> Result<i64, String> {
        let rows = self.changes()?;
        Ok(rows.first().and_then(|row| row["id"].as_i64()).unwrap_or(0))
    }

    /// The row of the console's change `action` made after row `after`, as
    /// alice made it.
    fn console_row(&mut self, after: i64, action: &str) -> Result<Value, String> {
        let mut rows = self.changes()?;
        rows.retain(|row| row["id"].as_i64().is_some_and(|id| id > after));
        let row = rows
            .into_iter()
            .find(|row| row["action"] == action)
            .ok_or_else(|| format!("the journal has no row for {action} after row {after}"))?;
        let fields = (row["who"].as_str(), row["how"].as_str(), row["status"].as_i64());
        if fields != (Some("alice"), Some("console"), Some(0)) {
            return Err(format!("the row of {action} is {row}, not alice's, by the console"));
        }
        Ok(row)
    }

    /// The draft `body` asks for, kept for `who`, as the answer says it:
    /// its ID is 32 hex digits, and its hash the SHA-256 of its text.
    fn draft(&mut self, who: Who, body: &Value) -> Result<Draft, String> {
        let reply = self.post(who, "/api/1/reports/drafts", body)?;
        let answer = reply.must("the draft", 200)?.json()?;
        let member = |name: &str| answer[name].as_str().map(str::to_string);
        let (Some(id), Some(sha256), Some(text)) =
            (member("draft"), member("sha256"), member("text"))
        else {
            return Err(format!("the draft has no ID, hash or text: {answer}"));
        };
        let hashed = format!("{:x}", Sha256::digest(&text)) == sha256;
        if !is_hex(&id, 32) || !is_hex(&sha256, 64) || !hashed {
            return Err(format!("the draft's ID or hash is not as it is said to be: {answer}"));
        }
        if answer["format"] != json!(1) || answer["bytes"] != json!(text.len()) {
            return Err(format!("the draft's format or size is not as it is said to be: {answer}"));
        }
        if !text.starts_with("Dukovo problem report\n") {
            return Err(format!("the draft does not start as a report does: {text}"));
        }
        Ok(Draft { id, sha256, text, answer })
    }

    /// What `reports.send` of `draft` with `hash` is answered, for `who`.
    fn send(&mut self, who: Who, draft: &Draft, hash: &str, more: &Value) -> Result<Reply, String> {
        let mut body = json!({"draft": draft.id, "sha256": hash, "title": TITLE});
        for (name, value) in more.as_object().into_iter().flatten() {
            body[name] = value.clone();
        }
        self.post(who, "/api/1/reports/send", &body)
    }
}

/// Whether `text` is `length` digits of lower-case hexadecimal.
fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// What a change that sent or retried says as the line of JSON its
/// command printed: the first of its `output` lines, as a document.
fn printed(answer: &Value) -> Result<Value, String> {
    let line = answer["output"][0]
        .as_str()
        .ok_or_else(|| format!("the answer has no line of output: {answer}"))?;
    serde_json::from_str(line).map_err(|err| format!("the output {line:?} is no JSON: {err}"))
}

/// The row of the list `reports` with `number`, or the one that is queued.
fn row_of(list: &Value, number: Option<u64>) -> Option<&Value> {
    list["reports"].as_array()?.iter().find(|row| match number {
        Some(number) => row["number"] == json!(number),
        None => row["state"] == "queued",
    })
}

/// The test: a crash, a draft, a send, the phone, a report that waits, and
/// the security box.
fn admin_reports(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let mut host = Host::start(DIR)?;
    owner_made(vm)?;
    vm.expect_anywhere("spark: started reportd (")?;
    let mut test = Test::open(vm)?;
    // reportd reads the server's address and the two services of the test,
    // which the machine fetches and runs once.
    set_up(vm, &host)?;
    people(vm, &mut test)?;

    let crash = a_crash_is_listed(vm, &mut test)?;
    a_draft_leaves_no_trace(vm, &mut test)?;
    a_report_is_sent(vm, &mut test, &host, &crash)?;
    a_phone_gets_the_text_once(&mut test)?;
    a_report_waits_for_the_server(vm, &mut test, &mut host)?;
    the_box_has_no_key(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Makes bob, in group console and no administrator, once alice's pane is
/// elevated, and signs both in.
fn people(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
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
    test.sign_in()
}

/// Step 1: the service of the test ends with SIGSEGV, and reportd keeps
/// it. alice, an administrator, is shown it, as the command prints it in
/// her pane; bob is not, as it was none of his. The crash's ID is returned.
fn a_crash_is_listed(vm: &mut Vm, test: &mut Test) -> Result<String, String> {
    run(vm, "crash crash-test signal 1", CRASH_SECONDS, &["kept signal 1"])?;
    let path = "/api/1/reports/crashes";
    let list = test.read(Who::Alice, path)?;
    let typed = test.typed(vm, "report crashes --json")?;
    if typed != list || list["format"] != json!(1) {
        return Err(format!("the pane has {typed}, and the route {list}"));
    }
    let crashes = list["crashes"].as_array().ok_or_else(|| format!("no crashes in {list}"))?;
    let Some(crash) = crashes.iter().find(|crash| crash["program"] == "crash-test") else {
        return Err(format!("the crash of crash-test is not listed: {list}"));
    };
    let id = crash["id"].as_str().unwrap_or_default();
    let shaped = id.contains('T') && id.contains('-') && crash["kind"] == "signal";
    let counted = crash["seen"].as_u64().is_some_and(|seen| seen >= 1);
    let timed = crash["at_ms"].as_u64().is_some_and(|at| at > 0);
    if !shaped || !counted || !timed || !crash["reported"].is_null() {
        return Err(format!("the crash is listed as {crash}"));
    }
    let bobs = test.read(Who::Bob, path)?;
    if bobs["crashes"].as_array().is_none_or(|all| all.iter().any(|one| one["id"] == id)) {
        return Err(format!("bob is shown the crash, or a list that is none: {bobs}"));
    }
    Ok(id.to_string())
}

/// Step 2: a draft about something else is read, which is a read: the
/// journal has no row for it, no task is made, and the log does not hold
/// what was typed. The marks redaction put are counted by kind, and the
/// parts have their sizes.
fn a_draft_leaves_no_trace(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let before = test.newest()?;
    let tasks = test.read(Who::Alice, "/api/1/tasks")?;
    let draft = test.draft(Who::Alice, &json!({"about": "other", "happened": TYPED}))?;
    let answer = &draft.answer;
    if !draft.text.contains(TYPED) {
        return Err(format!("the draft does not hold what was typed: {}", draft.text));
    }
    for kind in ["user", "host", "ipv4", "ipv6", "mac", "email", "wifi", "serial", "machine", "you"]
    {
        if answer["cut_by"][kind].as_u64().is_none() {
            return Err(format!("the draft does not count the marks of {kind}: {answer}"));
        }
    }
    let parts = answer["parts"].as_array().ok_or("the draft has no parts")?;
    if parts.is_empty() || parts.iter().any(|part| part["bytes"].as_u64().is_none()) {
        return Err(format!("the draft's parts are {}", answer["parts"]));
    }
    if test.newest()? != before {
        return Err("the journal has a row for a draft, which is a read".into());
    }
    if test.read(Who::Alice, "/api/1/tasks")? != tasks {
        return Err("a draft made a task".into());
    }
    run(vm, "echo \"audit $(elevate log -s admind | grep -c 'DRAFT-WORD[S]')\"", 30, &["audit 0"])
}

/// Step 3: a draft about the crash is sent as it was read. Another hash is
/// RPT-102 and the draft stays; bob does not hold it, RPT-101. Sent, the
/// server has its bytes, the journal has alice's row, a second send is
/// RPT-101, the list and the status say what the commands and the server
/// say, the crash is reported, and the report is deleted.
fn a_report_is_sent(vm: &mut Vm, test: &mut Test, host: &Host, crash: &str) -> Result<(), String> {
    let body = json!({
        "about": "crash", "crash": crash, "happened": "The service stopped",
        "expected": "It should keep running"
    });
    let draft = test.draft(Who::Alice, &body)?;
    for frame in ["fault_here", "middle", "outer"] {
        if !draft.text.contains(frame) {
            return Err(format!("the draft about the crash has no {frame}: {}", draft.text));
        }
    }
    let more = json!({ "crash": crash });
    let other = "0".repeat(64);
    let refused = test.send(Who::Alice, &draft, &other, &more)?;
    refused.refused("a send with another hash", 422, "RPT-102")?;
    let refused = test.send(Who::Bob, &draft, &draft.sha256, &more)?;
    refused.refused("a send by bob of alice's draft", 422, "RPT-101")?;

    let before = test.newest()?;
    let sent =
        test.send(Who::Alice, &draft, &draft.sha256, &more)?.must("the send", 200)?.json()?;
    if sent["done"] != json!(true) || sent["changed"] != json!(true) {
        return Err(format!("the send does not say it is done and changed something: {sent}"));
    }
    let said = printed(&sent)?;
    if said["state"] != "sent" || said["number"] != json!(1) {
        return Err(format!("the send says {said}, not that report 1 was sent"));
    }
    if host.report(1)? != draft.text.as_bytes() {
        return Err("the server's report is not the bytes of the draft's text".into());
    }
    test.console_row(before, "reports.send")?;
    let again = test.send(Who::Alice, &draft, &draft.sha256, &more)?;
    again.refused("a second send of the same draft", 422, "RPT-101")?;

    // The list: the report as sent, as the command prints it.
    let list = test.read(Who::Alice, "/api/1/reports")?;
    let typed = test.typed(vm, "report list --json")?;
    if typed != list {
        return Err(format!("the pane has {typed}, and the route {list}"));
    }
    let row = row_of(&list, Some(1)).ok_or_else(|| format!("report 1 is not listed: {list}"))?;
    if row["state"] != "sent" || row["title"] != TITLE || row["security"] != json!(false) {
        return Err(format!("report 1 is listed as {row}"));
    }
    // What the server says of it, which it changes.
    let standing = |test: &mut Test| -> Result<String, String> {
        let status = test.read(Who::Alice, "/api/1/reports/status")?;
        let row = row_of(&status, Some(1)).ok_or_else(|| format!("no status of 1: {status}"))?;
        if row["unreachable"] != json!(false) {
            return Err(format!("the server could not be asked: {row}"));
        }
        Ok(row["status"].as_str().unwrap_or_default().to_string())
    };
    if !standing(test)?.contains("Received") {
        return Err("the server does not say it received report 1".into());
    }
    host.fixed(1, "0.16.1")?;
    let fixed = standing(test)?;
    if !fixed.contains("Fixed in 0.16.1") {
        return Err(format!("the server says {fixed:?}, not that it is fixed in 0.16.1"));
    }
    let crashes = test.read(Who::Alice, "/api/1/reports/crashes")?;
    let listed = crashes["crashes"].as_array().into_iter().flatten();
    if !listed.into_iter().any(|one| one["id"] == crash && one["reported"] == json!(1)) {
        return Err(format!("the crash is not marked as reported as report 1: {crashes}"));
    }

    // Deleted, it is off the server and out of the list.
    let before = test.newest()?;
    let deleted = test.post(Who::Alice, "/api/1/reports/1/delete", &json!({}))?;
    deleted.must("the delete", 200)?;
    host.forgets(1)?;
    test.console_row(before, "reports.delete")?;
    let list = test.read(Who::Alice, "/api/1/reports")?;
    if row_of(&list, Some(1)).is_some() {
        return Err(format!("report 1 is listed after it was deleted: {list}"));
    }
    let said = test.pane.output(vm, "report list")?;
    if said.contains(TITLE) {
        return Err(format!("report list names the deleted report: {said:?}"));
    }
    Ok(())
}

/// Step 4: a draft's text is made a link that works once, for 10 minutes.
/// The phone has no session: the first request gets the text as a file,
/// the second 410, and a token never made 404, each of those with ADM-140.
fn a_phone_gets_the_text_once(test: &mut Test) -> Result<(), String> {
    let draft = test.draft(Who::Alice, &json!({"about": "other", "happened": "For the phone"}))?;
    let path = format!("/api/1/reports/drafts/{}/phone", draft.id);
    let other = test.post(Who::Alice, &path, &json!({"sha256": "0".repeat(64)}))?;
    other.refused("a link for another hash", 422, "RPT-102")?;
    let link = test.post(Who::Alice, &path, &json!({"sha256": draft.sha256}))?;
    let link = link.must("the phone link", 200)?.json()?;
    let url = link["url"].as_str().unwrap_or_default();
    let Some((_, once)) = url.split_once("/once/") else {
        return Err(format!("the link's address is {url:?}, with no /once/"));
    };
    if !url.starts_with("https://") || !once.ends_with("/report.txt") {
        return Err(format!("the link's address is {url:?}"));
    }
    if link["expires_ms"].as_u64().is_none_or(|at| at == 0) {
        return Err(format!("the link does not say when it ends: {link}"));
    }
    // The phone is shown the certificate the console serves, to compare.
    let plain = |text: &str| text.replace(':', "").to_lowercase();
    let shown = plain(link["fingerprint_sha256"].as_str().unwrap_or_default());
    if shown.is_empty() || shown != plain(&test.console.fingerprint) {
        return Err(format!("the link shows the fingerprint {shown:?}: {link}"));
    }

    // Nobody is signed in: the first request gets the text, a file to keep.
    let path = format!("/once/{once}");
    let mut phone = Browser::default();
    let first = test.console.get(&mut phone, &path)?.must("the phone's first request", 200)?;
    if first.body != draft.text {
        return Err(format!("the phone got {:?}, not the draft's text", first.body));
    }
    let kept = first.header("content-disposition").unwrap_or_default();
    let kind = first.header("content-type").unwrap_or_default();
    if !kept.contains("attachment")
        || !kept.contains("report.txt")
        || !kind.starts_with("text/plain")
    {
        return Err(format!("the text is sent as {kind:?} and {kept:?}"));
    }
    if first.header("cache-control") != Some("no-store") {
        return Err("the text is not sent with no-store".into());
    }
    for (what, path, status) in [
        ("the second request", path.clone(), 410),
        ("a token never made", format!("/once/{}/report.txt", "A".repeat(22)), 404),
    ] {
        let reply = test.console.get(&mut phone, &path)?;
        if reply.status != status || !reply.body.contains("ADM-140") {
            return Err(format!(
                "{what} is {} {:?}, not {status} with ADM-140",
                reply.status, reply.body
            ));
        }
    }
    Ok(())
}

/// Step 5: with the server stopped a send is queued with why, the list
/// says so, and `reports.retry` sends it once the server is back.
fn a_report_waits_for_the_server(
    vm: &mut Vm,
    test: &mut Test,
    host: &mut Host,
) -> Result<(), String> {
    let draft = test.draft(Who::Alice, &json!({"about": "other", "happened": "It waits"}))?;
    host.stop_server();
    let title = json!({ "title": WAITING_TITLE });
    let sent =
        test.send(Who::Alice, &draft, &draft.sha256, &title)?.must("the send", 200)?.json()?;
    let said = printed(&sent)?;
    if said["state"] != "queued" || said["number"] != Value::Null || said["why"].is_null() {
        return Err(format!("the send with the server away says {said}, not that it is queued"));
    }
    let list = test.read(Who::Alice, "/api/1/reports")?;
    let typed = test.typed(vm, "report list --json")?;
    if typed != list {
        return Err(format!("the pane has {typed}, and the route {list}"));
    }
    let Some(row) = row_of(&list, None) else {
        return Err(format!("no queued report is listed: {list}"));
    };
    if row["title"] != WAITING_TITLE || row["why"].is_null() || row["tries"].as_u64() < Some(1) {
        return Err(format!("the queued report is listed as {row}"));
    }

    host.start_server()?;
    let before = test.newest()?;
    let retried = test.post(Who::Alice, "/api/1/reports/retry", &json!({}))?;
    let retried = retried.must("the retry", 200)?.json()?;
    let tried = printed(&retried)?;
    let sent = tried["reports"][0]["state"] == "sent" && tried["reports"][0]["number"] == json!(2);
    if !sent || tried["reports"].as_array().map(Vec::len) != Some(1) {
        return Err(format!("the retry says {tried}, not that report 2 was sent"));
    }
    test.console_row(before, "reports.retry")?;
    let text = host.report(2)?;
    if text != draft.text.as_bytes() {
        return Err("the server's report 2 is not the bytes of the draft that waited".into());
    }
    let list = test.read(Who::Alice, "/api/1/reports")?;
    if row_of(&list, None).is_some() || row_of(&list, Some(2)).is_none() {
        return Err(format!("the list after the retry is {list}"));
    }
    Ok(())
}

/// Step 6: report.toml(5) names a file of the security team's keys that is
/// not there, and reportd is restarted: a report for the box is RPT-103,
/// and nothing is queued.
fn the_box_has_no_key(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    run(
        vm,
        &format!(
            "elevate sh -c 'grep -v security-recipients /system/settings/report.toml > /tmp/r.toml; \
             echo \"security-recipients = \\\"{NO_KEYS}\\\"\" >> /tmp/r.toml; cat /tmp/r.toml > \
             /system/settings/report.toml; service reportd restart'; echo \"keyless $(grep -c \
             {NO_KEYS} /system/settings/report.toml)\""
        ),
        SEND_SECONDS,
        &["keyless 1"],
    )?;
    let draft = test.draft(Who::Alice, &json!({"about": "other", "happened": "For the box"}))?;
    let boxed = json!({ "security": true });
    let refused = test.send(Who::Alice, &draft, &draft.sha256, &boxed)?;
    refused.refused("a send for the security box with no key", 422, "RPT-103")?;
    let list = test.read(Who::Alice, "/api/1/reports")?;
    if row_of(&list, None).is_some() {
        return Err(format!("a report for the box with no key was queued: {list}"));
    }
    Ok(())
}
