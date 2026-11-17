// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Certificates of the web console, Dukovo Admin: what certd and cert
//! did to the certificates, the checks before an order, a certificate a
//! person made and adds, an order that cannot work, the machine's name, and
//! the certificate the console serves (ADR 0515, ADR 0567). alice, the
//! owner, is an administrator; bob may use the console and is no
//! administrator. No authority is reached: the lab test of the page brings
//! one.
//!
//! The beginning. The route `GET /api/1/certificates/history/all` gives
//! what the command `cert history --json` prints in alice's pane, for alice
//! and for bob, and it holds nothing but certd making this-machine. The
//! route `GET /api/1/machine/name` gives what `hostnamectl status --json`
//! prints, for both: the name dukovo, its first word, no domain, and
//! dukovo.local.
//!
//! The checks. The route `GET /api/1/certificates/check/NAME` gives what
//! the command `cert get NAME --name NAME --check --json` prints, except
//! for the address of the authority's terms. For a name with no record it
//! says CRT-101 and has a check of port 80, and the command ends with
//! status 1; for localhost, which leads to a private address, it says
//! CRT-102; by DNS every check is skipped and the two TXT records are
//! named. A name that is none is ADM-125 with its field.
//!
//! A certificate a person made. Pane commands make a root, an intermediate
//! and two certificates the intermediate signed. `certificate.add` is
//! ADM-122 for bob and ADM-106 for alice while locked. For a chain with no
//! intermediate it is CRT-111 with the issuer's name, for another key
//! CRT-112, and neither adds anything. With a document of 60 KiB, which is
//! more than a pipe holds at once, it adds the certificate, and the history
//! has the line, as the pane prints it. `certificate.remove` takes the
//! certificate away and refuses this-machine.
//!
//! An order that cannot work. `certificate.get` without the agreement to
//! the terms is refused before it runs; with it, it is a task that fails
//! because the name leads nowhere, in the words of cert, and the history
//! has the failure.
//!
//! The machine's name. `machine.rename` is ADM-122 for bob and ADM-106 for
//! alice while locked, and a name that is none is ADM-125. To
//! lab-renamed.example.org it changes what `hostnamectl status --json`
//! prints and makes this-machine for lab-renamed.example.org, lab-renamed,
//! lab-renamed.local and localhost; the console answers a request for
//! lab-renamed.local and refuses the old name with ADM-114. Then the name
//! is put back.
//!
//! The console's certificate. `console.certificate` for a certificate that
//! does not name the address the browser uses, 10.0.2.15, is ADM-141 and
//! changes nothing. With the machine named lab-renamed.example.org and a
//! certificate for that name added, asked for by a browser that opened the
//! console by that name and tried for 30 seconds, the trial begins: on the
//! connection that carried the change `fresh` is ADM-133 and the console
//! ends the connection; on a new one it is 200, and the console serves the
//! certificate that was added. Left alone, Spark puts admin.toml and
//! admind's service file back, and the console serves this-machine again.
//!
//! What the pane's commands print reaches the host through nc, to a port on
//! the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{
    ALICE_PASSWORD, Browser, Console, ORIGIN, PORT, Pane, Reply, elevated, is_refusal, parse_reply,
    trial_in, undone_in, wait_for,
};
use super::super::*;

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// The folder of the test's files in the image: the script that makes the
/// certificates.
const FILES: &str = "/system/tests/services/admin-certificates";

/// The name the machine starts with, and is given back at the end.
const START: &str = "dukovo";

/// The name the test gives the machine.
const RENAMED: &str = "lab-renamed.example.org";

/// The address of the machine the browser of the test is given in step 6,
/// which no certificate of the test names.
const ADDRESS: &str = "10.0.2.15";

/// A name with no record anywhere: `.invalid` is never given out.
const NO_RECORD: &str = "no-record.test.invalid";

/// The console's settings, and the service file of admind that holds the
/// certificate Spark copies for it.
const ADMIN_TOML: &str = "/system/settings/admin.toml";
const ADMIND_SERVICE: &str = "/system/settings/spark/services/admind.toml";

/// The routes the test asks for besides the certificates'.
const HISTORY: &str = "/api/1/certificates/history/all";
const MACHINE: &str = "/api/1/machine/name";
const SERVE: &str = "/api/1/console/certificate";
const FRESH: &str = "/api/1/console/fresh/console";

/// How long the console's certificate is tried for, in seconds, which the
/// test lets run out.
const TRIAL_SECONDS: i64 = 30;

/// How long after its window Spark may take to undo the trial, in seconds.
const UNDO_SECONDS: u64 = 120;

/// How long an order that cannot work may take to fail, in seconds: the
/// looks for the name's addresses are slow where no resolver answers.
const ORDER_SECONDS: u64 = 180;

/// The size of the document `certificate.add` is given in step 3, in
/// bytes: 60 KiB, below the 64 KiB a document is at most.
const BIG_DOCUMENT: usize = 60 * 1024;

/// The test.
pub const CASE: Case = Case {
    name: "admin-certificates",
    about: "The web console's Certificates: certificate.history is what cert history --json prints, for alice and for bob, and holds nothing but certd making this-machine; machine.name is what hostnamectl status --json prints; certificate.check is cert get --check --json, CRT-101 and a check of port 80 for a name with no record, CRT-102 for localhost, every check skipped and the two TXT records named by DNS, and a name that is none ADM-125; certificate.add is ADM-122 for bob and ADM-106 for alice locked, CRT-111 for a chain with no intermediate and CRT-112 for another key, neither adding anything, and with a document of 60 KiB adds the certificate, which the history has as cert history --json prints it, and certificate.remove takes it away and refuses this-machine; certificate.get without the agreement to the terms is refused before it runs, and with it is a task that fails in the words of cert because the name leads nowhere, with the failure in the history; machine.rename is ADM-122 for bob and ADM-106 for alice locked, a name that is none is ADM-125, and to lab-renamed.example.org it changes what hostnamectl status --json prints, makes this-machine for the four names of the new name, answers a request for lab-renamed.local and refuses the old name with ADM-114, and is put back; console.certificate for a certificate that does not name 10.0.2.15 is ADM-141 and changes nothing, and for a certificate of the new name, asked for as that name and tried for 30 seconds, the trial begins, fresh is ADM-133 on the connection that carried the change and 200 on a new one, the console serves the added certificate, and left alone Spark puts admin.toml and admind's service file back and the console serves this-machine again (ADR 0515, ADR 0567)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, certd and admind among
    // them, and keeps the trials.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_certificates,
    disks: &[],
    second: None,
};

/// Who asks the console.
#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

/// What one run of curl for several requests gives: each answer with how
/// many connections curl opened for it, what curl said, and whether it
/// ended well.
type OnOneConnection = (Vec<(Reply, u32)>, String, bool);

/// What the test keeps from one step to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
    bob: Browser,
    /// How many runs of curl that keep one connection were made, to name
    /// their files.
    runs: usize,
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
            runs: 0,
        })
    }

    /// Signs alice and bob in, as the console's page does, each with a
    /// session of its own, locked.
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

    /// A change of alice's whose body is too long to give curl as a word of
    /// its own: curl reads it from a file.
    fn post_file(&mut self, path: &str, body: &Value) -> Result<Reply, String> {
        let file = self.console.pem.with_file_name("body.json");
        std::fs::write(&file, body.to_string())
            .map_err(|err| format!("writing {}: {err}", file.display()))?;
        let pem = self.console.pem.display().to_string();
        let url = format!("{ORIGIN}{path}");
        let from = format!("@{}", file.display());
        let lines: Vec<String> = self
            .alice
            .page_headers()
            .into_iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect();
        let mut args = vec!["--include", "--cacert", pem.as_str(), "--request", "POST"];
        for line in &lines {
            args.extend(["--header", line.as_str()]);
        }
        args.extend(["--data-binary", from.as_str(), url.as_str()]);
        let out = self.console.curl(&args)?;
        // The body holds a private key.
        let _ = std::fs::remove_file(&file);
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(format!("curl POST {path} ended with {}: {}", out.status, why.trim()));
        }
        let reply = parse_reply(&out.stdout)?;
        self.alice.keep(&reply);
        Ok(reply)
    }

    /// The headers alice's page sends with a request of `method` when the
    /// browser opened the console at `host`, a name and the port: the Host
    /// it asked for and, for a change, the origin that makes. The console
    /// answers the host as it is given, and the test's curl, which asked
    /// for localhost, takes the certificate for it.
    fn headers_at(&self, method: &str, host: &str) -> Vec<(String, String)> {
        let page = self.alice.page_headers();
        let mut headers: Vec<(String, String)> = if method == "GET" {
            page.into_iter().filter(|(name, _)| name == "Cookie").collect()
        } else {
            page.into_iter().filter(|(name, _)| name != "Origin").collect()
        };
        if method != "GET" {
            headers.push(("Origin".into(), format!("https://{host}")));
        }
        headers.push(("Host".into(), host.to_string()));
        headers
    }

    /// A read for alice by a browser that opened the console at `host`.
    fn get_at(&self, host: &str, path: &str) -> Result<Reply, String> {
        self.console.send("GET", path, &self.headers_at("GET", host), None)
    }

    /// A change of alice's by a browser that opened the console at `host`.
    fn post_at(&self, host: &str, path: &str, body: &Value) -> Result<Reply, String> {
        let headers = self.headers_at("POST", host);
        self.console.send("POST", path, &headers, Some(&body.to_string()))
    }

    /// `calls`, each a method, an address and a body, in one run of curl
    /// that keeps its connection for all of them, as a page's requests do,
    /// for a browser that opened the console at `host`: each answer with how
    /// many connections curl opened for it, and what curl said, which ends
    /// with its account of the last connection.
    fn on_one_connection(
        &mut self,
        host: &str,
        calls: &[(&str, &str, Option<String>)],
    ) -> Result<OnOneConnection, String> {
        self.runs += 1;
        let place = self.console.pem.with_file_name(format!("named-{}", self.runs));
        let mut command = Command::new("curl");
        // The first word of the command line, as curl wants it.
        command.arg("--disable");
        let mut files = Vec::new();
        for (number, call) in calls.iter().enumerate() {
            let (method, path, body) = (call.0, call.1, &call.2);
            if number > 0 {
                command.arg("--next");
            }
            let file = place.with_extension(number.to_string());
            command.args(["--silent", "--show-error", "--http1.1", "--max-time", "90"]);
            command.args(["--noproxy", "*", "--connect-to"]);
            command.arg(format!("localhost:{PORT}:127.0.0.1:{}", self.console.port));
            command.args(["--include", "--cacert"]).arg(&self.console.pem);
            command.args(["--request", method]);
            for (name, value) in self.headers_at(method, host) {
                command.arg("--header").arg(format!("{name}: {value}"));
            }
            if let Some(body) = body {
                command.arg("--data-binary").arg(body);
            }
            command.arg("--output").arg(&file);
            command.args(["--write-out", "%{http_code} %{num_connects}\\n"]);
            if number + 1 == calls.len() {
                command.arg("--verbose");
            }
            command.arg(format!("{ORIGIN}{path}"));
            files.push(file);
        }
        let out = command
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("the console tests need the host's curl: {err}"))?;
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        let summaries = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut carried = Vec::new();
        for (file, line) in files.iter().zip(summaries.lines()) {
            let raw = std::fs::read(file).map_err(|err| format!("{}: {err}", file.display()))?;
            let _ = std::fs::remove_file(file);
            let connects = line
                .split_whitespace()
                .nth(1)
                .and_then(|word| word.parse::<u32>().ok())
                .ok_or_else(|| format!("curl's summary of a request is {line:?}, not as asked"))?;
            carried.push((parse_reply(&raw)?, connects));
        }
        if carried.len() != calls.len() {
            return Err(format!(
                "curl carried out {} of {} requests and said {said:?}",
                carried.len(),
                calls.len()
            ));
        }
        Ok((carried, said, out.status.success()))
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

    /// Locks alice's session, as the page's Lock does.
    fn locked(&mut self) -> Result<(), String> {
        self.post(Who::Alice, "/api/1/lock", &json!({}))?.must("alice's lock", 200).map(drop)
    }

    /// A change that bob may not ask for, ADM-122, and alice may not while
    /// her session is locked, ADM-106: she is locked first, and the caller
    /// unlocks her again. `what` says which in the failure.
    fn refused_for_both(&mut self, what: &str, path: &str, body: &Value) -> Result<(), String> {
        self.locked()?;
        self.post(Who::Bob, path, body)?.refused(
            &format!("{what} asked by bob"),
            403,
            "ADM-122",
        )?;
        let locked = self.post(Who::Alice, path, body)?;
        locked.refused(&format!("{what} asked by alice, locked"), 403, "ADM-106")?;
        Ok(())
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
            let task = self.read(Who::Alice, &format!("/api/1/tasks/{id}"))?;
            if ready(&task) {
                return Ok(task);
            }
            if Instant::now() >= deadline {
                return Err(format!("task {id} is not as wanted after {seconds} s: {task}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// What `command`, which prints JSON, prints in the pane.
    fn typed(&self, vm: &mut Vm, command: &str) -> Result<Value, String> {
        let text = self.pane.output(vm, command)?;
        serde_json::from_str(text.trim())
            .map_err(|err| format!("{command} printed {text:?}: {err}"))
    }

    /// The text of the file `path`, read in the pane as an administrator:
    /// empty when there is none.
    fn file(&self, vm: &mut Vm, path: &str) -> Result<String, String> {
        self.pane.output(vm, &format!("elevate cat {path} 2>/dev/null"))
    }

    /// The history, as its route says it.
    fn history(&mut self) -> Result<Vec<Value>, String> {
        let history = self.read(Who::Alice, HISTORY)?;
        history.as_array().cloned().ok_or_else(|| format!("the history is {history}, not a list"))
    }

    /// The certificate `name` as `cert show --json` prints it.
    fn shown(&mut self, name: &str) -> Result<Value, String> {
        self.read(Who::Alice, &format!("/api/1/certificates/{name}"))
    }
}

/// The history's lines after the first `before` that are of `name` and say
/// `event`.
fn lines_of<'a>(history: &'a [Value], before: usize, name: &str, event: &str) -> Vec<&'a Value> {
    history
        .iter()
        .skip(before)
        .filter(|line| line["name"] == name && line["event"] == event)
        .collect()
}

/// The answer of a change that worked: done, whatever it changed. Returns
/// the answer.
fn done(reply: Reply, what: &str) -> Result<Value, String> {
    let answer = reply.must(what, 200)?.json()?;
    if answer["done"] != json!(true) || !answer["changed"].is_boolean() {
        return Err(format!("{what} answered {answer}, which is not done with a changed"));
    }
    Ok(answer)
}

/// `report`, what `cert get --check --json` prints, without the address of
/// the authority's terms, which the authority gives or does not.
fn without_terms(report: &Value) -> Value {
    let mut report = report.clone();
    report["terms_url"] = Value::Null;
    report
}

/// The check of `report` that has the error ID `id`.
fn check_with<'a>(report: &'a Value, id: &str) -> Option<&'a Value> {
    report["checks"].as_array()?.iter().find(|row| row["id"] == id)
}

/// The check of `report` that is of port 80, which no name belongs to.
fn check_of_port_80(report: &Value) -> Option<&Value> {
    report["checks"].as_array()?.iter().find(|row| {
        let words = row["words"].as_str().unwrap_or_default();
        row["name"].is_null() && words.contains("port 80")
    })
}

/// The id of the error a reply is, or none.
fn error_id(reply: &Reply) -> Option<String> {
    reply.json().ok()?["error"]["id"].as_str().map(str::to_string)
}

/// A key and a chain as one document, which is what `certificate.add` is
/// given.
fn document(key: &str, chain: &[&str]) -> String {
    format!("{key}{}", chain.concat())
}

/// The same document made `BIG_DOCUMENT` bytes long, by blank lines between
/// the key and the chain, which a reader of PEM passes over.
fn padded(key: &str, chain: &[&str]) -> String {
    let used = key.len() + chain.iter().map(|part| part.len()).sum::<usize>();
    format!("{key}{}{}", "\n".repeat(BIG_DOCUMENT.saturating_sub(used)), chain.concat())
}

/// The certificates the pane made, as PEM.
struct Chain {
    /// The key and the certificate for lab.example.org.
    lab_key: String,
    lab: String,
    /// The key and the certificate for lab-renamed.example.org, and
    /// localhost, which the test's curl asks the console for.
    renamed_key: String,
    renamed: String,
    /// The intermediate both are signed by.
    middle: String,
    /// A key that is neither's.
    other_key: String,
}

/// Makes bob, in group console and no administrator, once the owner alice
/// is signed in at the console, and starts the console's sessions for
/// alice and bob; alice's pane is elevated.
fn people(vm: &mut Vm) -> Result<Test, String> {
    let mut test = Test::open(vm)?;
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
    Ok(test)
}

/// The test: the history and the name at the start, the checks, a
/// certificate added and removed, an order that cannot work, the machine
/// renamed, and the console's certificate tried.
fn admin_certificates(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    let mut test = people(vm)?;

    let start = at_the_start(vm, &mut test)?;
    the_checks(vm, &mut test)?;
    let chain = made_chain(vm, &test)?;
    added_and_removed(vm, &mut test, &chain, start)?;
    an_order_that_cannot_work(&mut test)?;
    renamed_and_back(vm, &mut test)?;
    the_console_certificate(vm, &mut test, &chain)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Step 1: the history holds only certd making this-machine, as the route
/// and the command say it for alice and for bob, and the machine is called
/// dukovo, as `machine.name` and `hostnamectl status --json` say. Returns
/// how many lines the history has.
fn at_the_start(vm: &mut Vm, test: &mut Test) -> Result<usize, String> {
    let bob = test.read(Who::Bob, HISTORY)?;
    let alice = test.read(Who::Alice, HISTORY)?;
    if bob != alice {
        return Err(format!("bob is shown the history {bob}, and alice {alice}"));
    }
    let typed = test.typed(vm, "cert history --json")?;
    if typed != alice {
        return Err(format!("the pane has {typed}, and the route {alice}"));
    }
    let lines = alice.as_array().ok_or_else(|| format!("the history is {alice}, not a list"))?;
    for line in lines {
        let made = line["name"] == "this-machine" && line["event"] == "made";
        if !made || line["by"] != "certd" || !line["at_ms"].is_u64() {
            return Err(format!("the history has {line}, which is not certd making this-machine"));
        }
    }

    let bob = test.read(Who::Bob, MACHINE)?;
    let alice = test.read(Who::Alice, MACHINE)?;
    if bob != alice {
        return Err(format!("bob is shown the name {bob}, and alice {alice}"));
    }
    let typed = test.typed(vm, "hostnamectl status --json")?;
    if typed != alice {
        return Err(format!("the pane has {typed}, and the route {alice}"));
    }
    let wanted = json!({"name": START, "first": START, "domain": null, "local": "dukovo.local"});
    for member in ["name", "first", "domain", "local"] {
        if alice[member] != wanted[member] {
            return Err(format!("the machine's name is {alice}, not as {wanted}"));
        }
    }
    Ok(lines.len())
}

/// Step 2: `certificate.check` is what `cert get --check --json` prints.
/// A name with no record is CRT-101 and has a check of port 80, which the
/// command ends with status 1 for; localhost leads to a private address,
/// which an authority cannot reach, CRT-102; by DNS no check is made and the
/// two TXT records are named; and a name that is none is ADM-125.
fn the_checks(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let path = format!("/api/1/certificates/check/{NO_RECORD}");
    let report = test.read(Who::Alice, &path)?;
    if report["format"] != json!(1) {
        return Err(format!("the check of {NO_RECORD} is {report}, not of the format 1"));
    }
    let Some(row) = check_with(&report, "CRT-101") else {
        return Err(format!("the check of {NO_RECORD} has no CRT-101: {report}"));
    };
    let words = row["words"].as_str().unwrap_or_default();
    if row["result"] != "failed" || row["name"] != NO_RECORD || !words.contains("does not resolve")
    {
        return Err(format!("the check of {NO_RECORD} found {row}, not that it leads nowhere"));
    }
    if check_of_port_80(&report).is_none() {
        return Err(format!("the check of {NO_RECORD} has no check of port 80: {report}"));
    }
    let command = format!("cert get {NO_RECORD} --name {NO_RECORD} --check --json");
    let typed = test.typed(vm, &command)?;
    if without_terms(&typed) != without_terms(&report) {
        return Err(format!("the pane has {typed}, and the route {report}"));
    }
    // A check that found a failure ends with 1, and the route answers it.
    let status = test.pane.status(vm, &command)?;
    if status != 1 {
        return Err(format!("{command} ended with status {status}, not 1"));
    }

    let report = test.read(Who::Alice, "/api/1/certificates/check/localhost")?;
    let Some(row) = check_with(&report, "CRT-102") else {
        return Err(format!("the check of localhost has no CRT-102: {report}"));
    };
    if row["result"] != "failed" || row["name"] != "localhost" {
        return Err(format!("the check of localhost found {row}, not a private address"));
    }

    // By DNS the authority never connects to this machine.
    let path = "/api/1/certificates/check/shop.example.org,*.shop.example.org?as=shop&dns=true";
    let report = test.read(Who::Alice, path)?;
    let checks = report["checks"].as_array().ok_or_else(|| format!("no checks in {report}"))?;
    if checks.is_empty() || checks.iter().any(|row| row["result"] != "skipped") {
        return Err(format!("the check by DNS made a check: {report}"));
    }
    let record = json!({"name": "_acme-challenge.shop.example.org", "type": "TXT"});
    if report["dns_records"] != json!([record, record]) || !report["firewall_rule"].is_null() {
        return Err(format!("the check by DNS does not name the two records: {report}"));
    }

    for name in ["Shop.example.org", "shop..example.org"] {
        let refused = test.get(Who::Alice, &format!("/api/1/certificates/check/{name}"))?;
        let refused = refused.refused(&format!("the check of {name}"), 400, "ADM-125")?;
        if refused.json()?["error"]["field"] != json!("names") {
            return Err(format!("the refusal of {name} does not name the field names"));
        }
    }
    Ok(())
}

/// Makes the certificates in the pane, as the people who use them make
/// theirs, and reads them.
fn made_chain(vm: &mut Vm, test: &Test) -> Result<Chain, String> {
    answer_prompt(vm, "$", &format!("clear; sh {FILES}/chain.sh ~/chain"))?;
    vm.expect_screen(&["chain made"])?;
    let mut read = |name: &str| -> Result<String, String> {
        let text = test.pane.output(vm, &format!("cat ~/chain/{name}"))?;
        if !text.contains("-----BEGIN ") || !text.contains("-----END ") {
            return Err(format!("{name} is {text:?}, which is no PEM"));
        }
        Ok(text)
    };
    Ok(Chain {
        lab_key: read("lab.key")?,
        lab: read("lab.pem")?,
        renamed_key: read("renamed.key")?,
        renamed: read("renamed.pem")?,
        middle: read("middle.pem")?,
        other_key: read("other.key")?,
    })
}

/// Step 3: a certificate the person made is added, and taken away.
/// `certificate.add` is ADM-122 for bob and ADM-106 for alice locked; a
/// chain with no intermediate is CRT-111, naming the issuer it stops at; a
/// key that is not the certificate's is CRT-112; and neither adds anything.
/// With a document of 60 KiB, which no pipe holds at once, the certificate
/// is added: the list has it, and the history has the line, as the pane
/// prints it. `certificate.remove` is ADM-122 and ADM-106 for those, takes
/// it away, and refuses this-machine.
fn added_and_removed(
    vm: &mut Vm,
    test: &mut Test,
    chain: &Chain,
    start: usize,
) -> Result<(), String> {
    let path = "/api/1/certificates/lab/add";
    let small = document(&chain.lab_key, &[&chain.lab, &chain.middle]);
    test.refused_for_both("certificate.add", path, &json!({"pem": small, "untrusted": true}))?;
    test.unlocked()?;

    let alone = document(&chain.lab_key, &[&chain.lab]);
    let refused = test.post(Who::Alice, path, &json!({"pem": alone}))?;
    let refused = refused.refused("a chain with no intermediate", 422, "CRT-111")?;
    if !refused.detail().contains("the chain stops at CN=Test-Intermediate") {
        return Err(format!("the refusal of the chain does not name the issuer: {}", refused.body));
    }
    let wrong = document(&chain.other_key, &[&chain.lab, &chain.middle]);
    let body = json!({"pem": wrong, "untrusted": true});
    test.post(Who::Alice, path, &body)?.refused("another key", 422, "CRT-112")?;
    if test.history()?.len() != start {
        return Err("a certificate that was refused is in the history".into());
    }
    let listed = test.read(Who::Alice, "/api/1/certificates")?;
    if listed.as_array().is_none_or(|all| all.iter().any(|one| one["name"] == "lab")) {
        return Err(format!("the list has lab, which was refused: {listed}"));
    }

    // The input is 60 KiB, more than a pipe holds at once.
    let big = padded(&chain.lab_key, &[&chain.lab, &chain.middle]);
    if big.len() != BIG_DOCUMENT {
        return Err(format!("the document is {} bytes, not {BIG_DOCUMENT}", big.len()));
    }
    let reply = test.post_file(path, &json!({"pem": big, "untrusted": true}))?;
    let answer = done(reply, "the document of 60 KiB")?;
    let said = answer["output"][0].as_str().unwrap_or_default();
    let added_words = "lab added: lab.example.org, runs out ";
    if answer["changed"] != json!(true) || !said.starts_with(added_words) {
        return Err(format!("adding lab answered {answer}"));
    }
    let listed = test.read(Who::Alice, "/api/1/certificates")?;
    let row = listed.as_array().and_then(|all| all.iter().find(|one| one["name"] == "lab"));
    let Some(row) = row else {
        return Err(format!("the list has no lab after it was added: {listed}"));
    };
    if row["names"] != json!(["lab.example.org"]) || row["kind"] != "added" {
        return Err(format!("the list has lab as {row}"));
    }
    let history = test.history()?;
    let added = lines_of(&history, start, "lab", "added");
    if added.len() != 1 || added[0]["by"] != "cert" {
        return Err(format!("the history has no line of cert adding lab: {history:?}"));
    }
    let typed = test.typed(vm, "cert history --json")?;
    if typed != json!(history) {
        return Err(format!("the pane has {typed}, and the route {history:?}"));
    }

    let path = "/api/1/certificates/lab/remove";
    test.refused_for_both("certificate.remove", path, &json!({}))?;
    test.unlocked()?;
    let refused = test.post(Who::Alice, "/api/1/certificates/this-machine/remove", &json!({}))?;
    let refused = refused.refused("removing this-machine", 422, "ADM-123")?;
    if !refused.detail().contains("the machine's own certificate") {
        return Err(format!("the refusal to remove this-machine says {}", refused.body));
    }
    let reply = test.post(Who::Alice, path, &json!({}))?;
    let answer = done(reply, "removing lab")?;
    if answer["output"][0] != "lab removed" {
        return Err(format!("removing lab answered {answer}"));
    }
    let history = test.history()?;
    if lines_of(&history, start, "lab", "removed").len() != 1 {
        return Err(format!("the history has no line of lab removed: {history:?}"));
    }
    let listed = test.read(Who::Alice, "/api/1/certificates")?;
    let names: Vec<&str> =
        listed.as_array().into_iter().flatten().filter_map(|one| one["name"].as_str()).collect();
    if names.contains(&"lab") || !names.contains(&"this-machine") {
        return Err(format!("the list after the removal is {names:?}"));
    }
    Ok(())
}

/// Step 4: an order for a name that leads nowhere. Without the agreement to
/// the terms, or with it said to be false, it is refused before it runs;
/// with it, it is a task that fails in cert's words, with no authority
/// asked, and the history has the failure with the ID of the check.
fn an_order_that_cannot_work(test: &mut Test) -> Result<(), String> {
    let path = "/api/1/certificates/lab-order/get";
    let names = "lab-order.test.invalid";
    let agreed = json!({"names": names, "agree_terms": true});
    test.refused_for_both("certificate.get", path, &agreed)?;
    test.unlocked()?;
    let before = test.history()?.len();

    let without = test.post(Who::Alice, path, &json!({"names": names}))?;
    let id = error_id(&without);
    if without.status != 400 || !matches!(id.as_deref(), Some("ADM-108" | "ADM-125")) {
        return Err(format!(
            "an order with no agreement was answered {} {}",
            without.status, without.body
        ));
    }
    let said_no = test.post(Who::Alice, path, &json!({"names": names, "agree_terms": false}))?;
    let said_no = said_no.refused("an order with no agreement", 400, "ADM-125")?;
    if said_no.json()?["error"]["field"] != json!("agree_terms") {
        return Err(format!("the refusal does not name agree_terms: {}", said_no.body));
    }
    if test.history()?.len() != before {
        return Err("an order that was refused is in the history".into());
    }

    let reply = test.post(Who::Alice, path, &agreed)?;
    let id = reply.must("the order", 202)?.json()?["task"].as_i64().ok_or("the order has no task")?;
    let task = test.task_where(id, ORDER_SECONDS, |task| {
        task["state"] != "running" && task["state"] != "waiting"
    })?;
    let words = [
        "cert",
        "get",
        "lab-order",
        "--name",
        names,
        "--agree-terms",
        "--yes",
        "--progress",
    ];
    if task["action"] != "certificate.get" || task["argv"] != json!(words) {
        return Err(format!("the order's task is {task}"));
    }
    if task["state"] != "failed" || task["status"] != json!(1) {
        return Err(format!("the order of a name that leads nowhere ended as {task}"));
    }
    let log: Vec<&str> =
        task["log"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !log.iter().any(|line| line.contains("does not resolve to an address")) {
        return Err(format!("the order's log does not say the name leads nowhere: {log:?}"));
    }
    let history = test.history()?;
    let failed = lines_of(&history, before, "lab-order", "failed");
    let detail = failed.first().and_then(|line| line["detail"].as_str()).unwrap_or_default();
    if failed.len() != 1 || !detail.contains("CRT-101") {
        return Err(format!("the history has no failure of lab-order with CRT-101: {history:?}"));
    }
    Ok(())
}

/// Waits up to a minute for the console to serve a certificate other than
/// the one with the fingerprint `old`, and trusts that one: returns its
/// fingerprint.
fn served_anew(test: &mut Test, old: &str) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        // The console may drop a connection while it takes the new one.
        let learned = test.console.trust_what_is_served(Duration::from_secs(5));
        if learned.is_ok() && test.console.fingerprint != old {
            return Ok(test.console.fingerprint.clone());
        }
        if Instant::now() >= deadline {
            return Err(format!("the console still serves the certificate {old} a minute on"));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Renames the machine `name` as alice, who is unlocked, and waits for the
/// console to serve the certificate that was made for the name, which the
/// host trusts from then on.
fn renamed_to(test: &mut Test, name: &str) -> Result<(), String> {
    test.unlocked()?;
    let old = test.console.fingerprint.clone();
    let reply = test.post(Who::Alice, MACHINE, &json!({"name": name}))?;
    done(reply, &format!("the rename to {name}"))?;
    served_anew(test, &old)?;
    Ok(())
}

/// The names of `this-machine` as `cert show` says them.
fn names_of_this_machine(test: &mut Test) -> Result<Value, String> {
    Ok(test.shown("this-machine")?["names"].clone())
}

/// Waits for a request that names `host` to be answered `status`, with the
/// error `id` when it is a refusal. A name the console takes for its own is
/// known from the file's name and the certificate's, which it reads again
/// when the name changes.
fn answered_as(test: &Test, host: &str, status: u16, id: Option<&str>) -> Result<(), String> {
    let at = format!("{host}:{PORT}");
    wait_for(&format!("{host} to be answered {status}"), 40, || {
        let reply = test.get_at(&at, "/api/1/health")?;
        let right = match id {
            Some(id) => reply.status == status && error_id(&reply).as_deref() == Some(id),
            None => reply.status == status,
        };
        Ok(if right { Ok(()) } else { Err(format!("{} {}", reply.status, reply.body)) })
    })
}

/// Step 5: `machine.rename` is ADM-122 for bob and ADM-106 for alice locked,
/// and a name that is none is ADM-125 with its field, none of which changes
/// the name. To lab-renamed.example.org it changes what `hostnamectl status
/// --json` prints, which `machine.name` says for bob, makes this-machine for
/// the four names of the name, and the console answers a request for
/// lab-renamed.local and not for dukovo, ADM-114. Then it is put back.
fn renamed_and_back(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let body = json!({"name": RENAMED});
    test.refused_for_both("machine.rename", MACHINE, &body)?;
    test.unlocked()?;
    for bad in ["-x", "a b", "x-", "lab..example.org", ""] {
        let refused = test.post(Who::Alice, MACHINE, &json!({"name": bad}))?;
        let refused = refused.refused(&format!("the name {bad:?}"), 400, "ADM-125")?;
        if refused.json()?["error"]["field"] != json!("name") {
            return Err(format!("the refusal of {bad:?} does not name the field name"));
        }
    }
    let name = test.read(Who::Bob, MACHINE)?;
    if name["name"] != START {
        return Err(format!("a name that was refused changed the machine's: {name}"));
    }
    let before = test.history()?.len();

    renamed_to(test, RENAMED)?;
    let said = test.read(Who::Bob, MACHINE)?;
    let typed = test.typed(vm, "hostnamectl status --json")?;
    if typed != said {
        return Err(format!("the pane has {typed}, and the route {said}"));
    }
    let wanted = json!({
        "name": RENAMED, "first": "lab-renamed", "domain": "example.org",
        "local": "lab-renamed.local"
    });
    for member in ["name", "first", "domain", "local"] {
        if said[member] != wanted[member] {
            return Err(format!("the machine's name is {said}, not as {wanted}"));
        }
    }
    let names = names_of_this_machine(test)?;
    if names != json!([RENAMED, "lab-renamed", "lab-renamed.local", "localhost"]) {
        return Err(format!("this-machine names {names} after the rename"));
    }
    let history = test.history()?;
    if lines_of(&history, before, "this-machine", "made").is_empty() {
        return Err(format!("the history has no line of this-machine made anew: {history:?}"));
    }
    // The session of alice and of bob go on.
    test.read(Who::Bob, "/api/1/session")?;
    answered_as(test, "lab-renamed.local", 200, None)?;
    answered_as(test, START, 421, Some("ADM-114"))?;

    renamed_to(test, START)?;
    let back = test.read(Who::Bob, MACHINE)?;
    if back["name"] != START || back["domain"] != Value::Null {
        return Err(format!("the machine is {back} after it was given its name back"));
    }
    let names = names_of_this_machine(test)?;
    if names != json!([START, "dukovo.local", "localhost"]) {
        return Err(format!("this-machine names {names} after the name was put back"));
    }
    answered_as(test, "dukovo.local", 200, None)?;
    answered_as(test, "lab-renamed.local", 421, Some("ADM-114"))?;
    Ok(())
}

/// Adds the certificate `name` with `key` and `chain`, as alice, who is
/// unlocked, and asks for nothing of its answer.
fn added(test: &mut Test, name: &str, key: &str, chain: &[&str]) -> Result<(), String> {
    let path = format!("/api/1/certificates/{name}/add");
    let body = json!({"pem": document(key, chain), "untrusted": true});
    let reply = test.post(Who::Alice, &path, &body)?;
    done(reply, &format!("adding {name}")).map(drop)
}

/// Removes the certificate `name` as alice, who is unlocked.
fn removed(test: &mut Test, name: &str) -> Result<(), String> {
    let path = format!("/api/1/certificates/{name}/remove");
    let reply = test.post(Who::Alice, &path, &json!({}))?;
    done(reply, &format!("removing {name}")).map(drop)
}

/// Step 6: `console.certificate`. For a certificate that does not name the
/// address the browser uses, 10.0.2.15, it is ADM-141 and the console's
/// settings are as they were. With the machine named lab-renamed.example.org
/// and a certificate for that name added, asked for by a browser that
/// opened the console by that name and tried for 30 seconds, the trial
/// begins: `fresh` is ADM-133 on the connection that carried the change,
/// which the console ends, and 200 on a new one, whose certificate is the
/// one that was added. Left alone, Spark puts the two files back and the
/// console serves this-machine again.
fn the_console_certificate(vm: &mut Vm, test: &mut Test, chain: &Chain) -> Result<(), String> {
    test.unlocked()?;
    added(test, "lab", &chain.lab_key, &[&chain.lab, &chain.middle])?;
    let (admin, service) = (test.file(vm, ADMIN_TOML)?, test.file(vm, ADMIND_SERVICE)?);
    let serving = test.pane.output(vm, "admin certificate")?;
    if serving.trim() != "this-machine" {
        return Err(format!("the console serves {:?}, not this-machine", serving.trim()));
    }

    // The address is the machine's, and lab's certificate does not name it.
    let at_address = format!("{ADDRESS}:{PORT}");
    let body = json!({"name": "lab", "try": TRIAL_SECONDS});
    let refused = test.post_at(&at_address, SERVE, &body)?;
    let refused = refused.refused("a certificate that does not name the address", 409, "ADM-141")?;
    if !refused.detail().contains(ADDRESS) {
        return Err(format!("the refusal does not name the address {ADDRESS}: {}", refused.body));
    }
    let serving = test.pane.output(vm, "admin certificate")?;
    let trial = trial_in(&test.read(Who::Alice, "/api/1/services")?, "console").cloned();
    if serving.trim() != "this-machine"
        || test.file(vm, ADMIN_TOML)? != admin
        || test.file(vm, ADMIND_SERVICE)? != service
        || trial.is_some()
    {
        return Err(format!("the refusal changed something: {:?}, {trial:?}", serving.trim()));
    }

    // The machine is lab-renamed.example.org, and a certificate names it.
    renamed_to(test, RENAMED)?;
    test.unlocked()?;
    added(test, "lab-renamed", &chain.renamed_key, &[&chain.renamed, &chain.middle])?;
    let this_machine = test.console.fingerprint.clone();
    let undone = undone_in(&test.read(Who::Alice, "/api/1/services")?, "console").len();

    let host = format!("{RENAMED}:{PORT}");
    let change = json!({"name": "lab-renamed", "try": TRIAL_SECONDS}).to_string();
    let calls = [
        ("GET", "/api/1/session", None),
        ("POST", SERVE, Some(change)),
        ("GET", FRESH, None),
    ];
    let (carried, said, ended) = test.on_one_connection(&host, &calls)?;
    let [(session, first), (change, second), (old, third)] = &carried[..] else {
        return Err("a run of three requests carried out another number".into());
    };
    if session.status != 200 || change.status != 200 {
        return Err(format!(
            "the session was answered {}, and the change {} {}",
            session.status, change.status, change.body
        ));
    }
    if (*first, *second, *third) != (1, 0, 0) {
        return Err(format!(
            "curl opened {first}, {second} and {third} connections for three requests that \
             share one"
        ));
    }
    if !is_refusal(old, 409, "ADM-133") {
        return Err(format!(
            "fresh on the connection opened before the change was answered {} {}, not ADM-133",
            old.status, old.body
        ));
    }
    if old.header("connection") != Some("close") || !ended {
        return Err(format!("the console did not end the old connection: {said}"));
    }

    // A new connection is one opened after the change, and is given the
    // certificate that was added, which the host trusts from here on.
    let served = served_anew(test, &this_machine)?;
    let new = test.get_at(&host, FRESH)?;
    let answer = new.must("fresh on a new connection", 200)?.json()?;
    if answer["fresh"] != json!(true) || answer["via"]["port"] != json!(PORT) {
        return Err(format!("fresh on a new connection answered {answer}"));
    }
    let fingerprint = test.console.send("GET", "/api/1/fingerprint", &[], None)?.json()?;
    if fingerprint["fingerprint"] != json!(served) {
        return Err(format!("the console gives {fingerprint}, not the fingerprint {served}"));
    }
    let shown = test.shown("lab-renamed")?;
    let plain = served.replace(':', "").to_lowercase();
    if shown["fingerprint_sha256"] != json!(plain) {
        return Err(format!("the console serves {served}, and lab-renamed is {shown}"));
    }

    // Left alone, Spark undoes the trial: both files as they were, and the
    // console serves what it served before.
    wait_for("Spark's undo of the console's certificate", UNDO_SECONDS, || {
        let serving = test.pane.output(vm, "admin certificate")?;
        let back = serving.trim() == "this-machine";
        Ok(if back { Ok(()) } else { Err(format!("the console serves {:?}", serving.trim())) })
    })?;
    if test.file(vm, ADMIN_TOML)? != admin || test.file(vm, ADMIND_SERVICE)? != service {
        return Err("admin.toml or admind's service file is not as it was after the undo".into());
    }
    let after = served_anew(test, &served)?;
    let document = test.read(Who::Alice, "/api/1/services")?;
    if trial_in(&document, "console").is_some() || undone_in(&document, "console").len() <= undone {
        return Err(format!("Spark has no undone trial of the console afterwards: {document}"));
    }
    let this = test.shown("this-machine")?;
    if this["fingerprint_sha256"] != json!(after.replace(':', "").to_lowercase()) {
        return Err(format!("the console serves {after}, and this-machine is {this}"));
    }

    // What the test added is taken away, and the machine has its name back.
    // lab was removed once before, in step 3: only these removals count.
    test.unlocked()?;
    let before = test.history()?.len();
    removed(test, "lab-renamed")?;
    removed(test, "lab")?;
    let history = test.history()?;
    for name in ["lab", "lab-renamed"] {
        if lines_of(&history, before, name, "removed").len() != 1 {
            return Err(format!("the history has no line of {name} removed: {history:?}"));
        }
    }
    renamed_to(test, START)?;
    let back = test.read(Who::Bob, MACHINE)?;
    if back["name"] != START {
        return Err(format!("the machine is {back}, not called {START}"));
    }
    Ok(())
}
