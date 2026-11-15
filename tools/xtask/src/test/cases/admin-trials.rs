// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Changes the web console, Dukovo Admin, makes to the firewall and the
//! network are tried: Spark keeps a copy of the settings file, and puts it
//! back unless the change is kept in time (ADR 0472, ADR 0518). alice, the
//! owner, an administrator, asks for them through their routes; bob, who
//! may use the console and is no administrator, asks too.
//!
//! The reads. `GET /api/1/firewall` is the same for alice and for bob as
//! the output of `firewall list --json` typed in the pane.
//!
//! A tried change. `POST /api/1/firewall/add` with a window, `try`, in
//! seconds, is done: Spark's document, which `GET /api/1/services` gives,
//! has the trial `firewall` with the file it keeps a copy of; the file has
//! the rule; the kernel's rules have one more, which `elevate firewall
//! status` counts; and the journal has a row for the change. When the
//! window ends Spark undoes it: the trial is gone from its document and
//! there is an `undone` entry with its why, the file is as it was, the
//! kernel has its rules as before, and the journal has a row of its own:
//! who is Spark, how is `undone`, the files name the file it put back and
//! the note is Spark's words. No row is by hand for the file at any time.
//!
//! A kept change. The same with a window, then `POST
//! /api/1/firewall/trial/keep`: when the window is long past the rule is
//! still there, and `firewall.remove` takes it away.
//!
//! A second change while one is tried is NET-177, as the command says it,
//! with its own ID in place of ADM-123; for bob it is ADM-122. The trial
//! goes on until `POST /api/1/firewall/trial/undo` ends it.
//!
//! The way back to a new connection. The route `GET
//! /api/1/console/fresh/firewall` says whether the connection it came by
//! was opened after the change took effect. One run of curl keeps one
//! connection for three requests, the session, the add with a window and
//! that route: the third is 409, ADM-133, and the console ends the
//! connection after the answer: `Connection: close` on HTTP/1.1, a GOAWAY
//! on HTTP/2, which curl reports with no error of its own. A new run is
//! answered 200 with `fresh`, the address the request came from and the
//! address, port and card it came to, as the machine names its card. Both
//! are done twice, over HTTP/1.1 and over HTTP/2, and the trial is kept.
//!
//! A tried change of a card. `POST /api/1/network/cards/net0` gives it a
//! second address, 10.0.2.99, besides its own, which `ip addr` shows;
//! the window ends, the address is gone, the file is as before, and the
//! journal has Spark's row for it.
//!
//! A start cut short. A firewall change tried for ten minutes, then a
//! reset of the machine (QEMU's `system_reset`) with the disk synced: Spark
//! undoes the trial at the start, before any service starts, so its line
//! comes before the one that says the `firewall` service started. The
//! rules are as before, and once the console is up again the journal has
//! the `undone` row, with Spark's words that the machine started again
//! before the change was kept. That needs a disk that keeps what Spark
//! writes, so the test boots the installed system's.
//!
//! What the pane's commands print reaches the host through nc, to a port
//! on the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{
    ALICE_PASSWORD, Browser, Console, ORIGIN, PORT, Pane, Reply, elevated, parse_reply,
};
use super::super::*;

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "white-rabbit";

/// Where admind's log says the host's requests come from.
const PEER: &str = "10.0.2.2";

/// The rule the test adds and takes away: a port that nothing listens on.
const RULE: &str = "deny tcp 7";

/// The rule's arguments for its routes, without a window.
const RULE_ARGS: &str = r#"{"action": "deny", "protocol": "tcp", "ports": "7"}"#;

/// The rules of the firewall, and the card's settings: the files the
/// trials keep copies of.
const FIREWALL: &str = "/system/settings/firewall";
const NETWORK: &str = "/system/settings/network";

/// The card the test changes, and the address it gives it besides its own.
const CARD: &str = "net0";
const EXTRA_ADDRESS: &str = "10.0.2.99";

/// The window of a change that is left to run out, in seconds: long enough
/// for the reads that follow it, which a machine without KVM takes a while
/// over.
const WINDOW: i64 = 60;

/// The window of the change that is kept, in seconds, and the seconds the
/// test waits beyond it: the keep comes well within the window, and the
/// rule must outlast it.
const KEPT_WINDOW: i64 = 40;
const KEPT_BEYOND: u64 = 5;

/// The window of the change that the reset cuts short, in seconds: far
/// longer than the test takes to get to the reset.
const LONG_WINDOW: i64 = 600;

/// How long Spark may take to undo a change after its window, and the
/// journal to have its row: the round that looks for rows is every 30
/// seconds.
const UNDO_SECONDS: u64 = 120;
const ROW_SECONDS: u64 = 120;

/// What Spark says when it undoes a trial whose window ran out, and one the
/// start that followed it cut short.
const TIME_UP: &str = "it was not kept in time";
const RESTARTED: &str = "the machine started again before it was kept";

/// The line Spark prints at the start for a trial it undoes, before any
/// service starts, and the one for the service that loads the rules.
const UNDID_AT_START: &str =
    "spark: undid firewall (the machine started again before it was kept): put back ";
const FIREWALL_STARTED: &str = "spark: started firewall (/system/commands/firewall) as process ";

/// The test.
pub const CASE: Case = Case {
    name: "admin-trials",
    about: "The web console's tried changes: a firewall rule added with a window is in the file, in the kernel's rules and in Spark's document as the trial firewall, with a row in the journal, and when the window ends Spark undoes it, the file is as it was and the journal has a row by Spark, undone, with Spark's words and no row by hand; kept in time it stays, and firewall.remove takes it away; a second change while one is tried is NET-177 and ADM-122 for bob; the route fresh answers 409 ADM-133 on a connection opened before the change, which the console ends after the answer, as a close on HTTP/1.1 and a GOAWAY on HTTP/2 that curl reports with no error of its own, and 200 with where the request came from and by which port and card on a new one; a card given a second address for a minute loses it and has its file back; a firewall change tried for ten minutes and a reset of the machine are undone by Spark at the start, before the firewall service starts, and the journal tells it once the console is up (ADR 0518)",
    firmware: Firmware::Uefi,
    // The reset needs a disk that keeps Spark's copy of the file and the
    // record of the trial, and a machine that QEMU resets.
    media: Media::Disk,
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_trials,
    disks: &[],
    second: None,
};

fn admin_trials(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    // The installed image's start, which has no report of the user test
    // programs that the plain image's has.
    owner_made(vm)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        bob: Browser::default(),
        runs: 0,
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

    let machine = Machine::take(vm, &mut test)?;
    let first = test.newest()?;
    reads_agree(vm, &mut test)?;
    a_tried_add(vm, &mut test, &machine)?;
    is_undone(vm, &mut test, &machine, first)?;
    is_kept(vm, &mut test, &machine)?;
    a_second_change(vm, &mut test, &machine)?;
    is_fresh(&mut test, Http::One)?;
    is_fresh(&mut test, Http::Two)?;
    a_tried_card(vm, &mut test, &machine, first)?;
    cut_short(vm, &mut test, &machine, first)?;

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
    /// How many runs of curl wrote their answers to files so far, to name
    /// them.
    runs: usize,
}

impl Test {
    /// Signs alice and bob in, as the console's page does, each with a
    /// session of its own: the ones before are forgotten.
    fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        self.bob = Browser::default();
        self.console.sign_in(&mut self.bob, "bob", BOB_PASSWORD)?;
        self.console.read_session(&mut self.bob)?;
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

    /// What `path` says to `who`, which must be 200.
    fn read(&mut self, who: Who, path: &str) -> Result<Value, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        let reply = console.get(browser, path)?;
        if reply.status != 200 {
            return Err(format!("{path} was answered {} {}", reply.status, reply.body));
        }
        reply.json()
    }

    /// A change of `path` with `body` for `who`, whatever the answer is.
    fn post(&mut self, who: Who, path: &str, body: &Value) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.post(browser, path, body)
    }

    /// A change of `path` with `body` as alice, unlocked, which must be
    /// done.
    fn change(&mut self, path: &str, body: &Value) -> Result<Value, String> {
        self.unlocked()?;
        let answer = self.post(Who::Alice, path, body)?.must(path, 200)?.json()?;
        if answer["done"] != json!(true) {
            return Err(format!("{path} does not say it is done: {answer}"));
        }
        Ok(answer)
    }

    /// Spark's document, which the list of services is.
    fn document(&mut self) -> Result<Value, String> {
        self.read(Who::Alice, "/api/1/services")
    }

    /// The rules of the file, as the page reads them.
    fn rules(&mut self) -> Result<Vec<String>, String> {
        let list = self.read(Who::Alice, "/api/1/firewall")?;
        rule_texts(&list).ok_or_else(|| format!("no rules in {list}"))
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

    /// The rows made after row `after`, oldest first.
    fn rows_after(&mut self, after: i64) -> Result<Vec<Value>, String> {
        let mut rows = self.changes()?;
        rows.retain(|row| row["id"].as_i64().is_some_and(|id| id > after));
        rows.reverse();
        Ok(rows)
    }

    /// The row of the console's change `action` made after row `after`, as
    /// alice made it: the row is there before the change's answer comes.
    fn console_row(&mut self, after: i64, action: &str, file: &str) -> Result<Value, String> {
        let rows = self.rows_after(after)?;
        let row = rows
            .into_iter()
            .rev()
            .find(|row| row["action"] == action)
            .ok_or_else(|| format!("the journal has no row for {action} after row {after}"))?;
        let fields = (row["who"].as_str(), row["how"].as_str(), row["status"].as_i64());
        if fields != (Some("alice"), Some("console"), Some(0)) || !names(&row, file) {
            return Err(format!(
                "the row of {action} is {row}, not alice's, by the console, with status 0, naming {file}"
            ));
        }
        Ok(row)
    }

    /// Waits up to `seconds` for the journal's row of a trial Spark undid
    /// by itself after row `after`, which names `file` and has Spark's
    /// words `why` as its note.
    fn undone_row(
        &mut self,
        after: i64,
        file: &str,
        why: &str,
        seconds: u64,
    ) -> Result<Value, String> {
        wait_for(&format!("the journal's undone row for {file}"), seconds, || {
            let rows = self.rows_after(after)?;
            match rows.into_iter().find(|row| is_undone_row(row, file, why)) {
                Some(row) => Ok(Ok(row)),
                None => Ok(Err("no such row yet".to_string())),
            }
        })
    }

    /// Checks that the journal has no row by hand after row `after` that
    /// names `file`: what Spark and the console wrote are not hand edits.
    fn no_hand_edit(&mut self, after: i64, file: &str) -> Result<(), String> {
        let rows = self.rows_after(after)?;
        match rows.iter().find(|row| row["how"] == "by hand" && names(row, file)) {
            Some(row) => Err(format!("the journal took a change of {file} for a hand edit: {row}")),
            None => Ok(()),
        }
    }

    /// The text of the settings file `path`, read in the pane as an
    /// administrator: empty when there is none.
    fn file(&self, vm: &mut Vm, path: &str) -> Result<String, String> {
        self.pane.output(vm, &format!("elevate cat {path} 2>/dev/null"))
    }

    /// How many rules the kernel holds, as `firewall status` says.
    fn kernel_rules(&self, vm: &mut Vm) -> Result<u64, String> {
        let said = self.pane.output(vm, "elevate firewall status | grep '^Firewall:'")?;
        kernel_rules(&said).ok_or_else(|| format!("firewall status began with {said:?}"))
    }

    /// Waits up to `seconds` for the kernel to hold `wanted` rules: the
    /// service that loads them runs a moment after the change.
    fn kernel_rules_are(&self, vm: &mut Vm, wanted: u64, seconds: u64) -> Result<(), String> {
        wait_for(&format!("the kernel's {wanted} rules"), seconds, || {
            let held = self.kernel_rules(vm)?;
            Ok(if held == wanted { Ok(()) } else { Err(format!("it holds {held}")) })
        })
    }

    /// Waits up to `seconds` for the settings of `card` to hold `address`
    /// or not, as the list of cards says.
    fn card_has(&mut self, address: &str, has: bool, seconds: u64) -> Result<(), String> {
        let what = format!("{CARD} to {} {address}", if has { "have" } else { "lose" });
        wait_for(&what, seconds, || {
            // The card's addresses change as netd reloads: a read that
            // fails meanwhile is asked again.
            let network = match self.read(Who::Alice, "/api/1/network") {
                Ok(network) => network,
                Err(why) => return Ok(Err(why)),
            };
            let held = addresses_of(&network, CARD);
            Ok(if held.iter().any(|held| held == address) == has {
                Ok(())
            } else {
                Err(format!("it has {held:?}"))
            })
        })
    }

    /// Waits up to `seconds` for Spark's document to have no trial `id`
    /// and one more `undone` entry for it than `before`, and gives that
    /// entry.
    fn undone_entry(&mut self, id: &str, before: usize, seconds: u64) -> Result<Value, String> {
        wait_for(&format!("Spark's undo of {id}"), seconds, || {
            let document = self.document()?;
            let undone = undone_in(&document, id);
            if trial_in(&document, id).is_none() && undone.len() > before {
                return Ok(Ok(undone[undone.len() - 1].clone()));
            }
            Ok(Err(format!("trial {:?}, {} undone", trial_in(&document, id), undone.len())))
        })
    }

    /// Runs curl once for `calls`, with the session of alice's browser, as
    /// one run: with `--next`, curl takes the connection it has for the
    /// next request, so the requests share one connection as a page's do.
    fn carry(&mut self, http: Http, calls: &[Call]) -> Result<Run, String> {
        self.runs += 1;
        let place = self.console.pem.with_file_name(format!("carried-{}", self.runs));
        let mut command = Command::new("curl");
        // The first word of the command line, as curl wants it.
        command.arg("--disable");
        let mut files = Vec::new();
        for (number, call) in calls.iter().enumerate() {
            if number > 0 {
                command.arg("--next");
            }
            let file = place.with_extension(number.to_string());
            command.args(["--silent", "--show-error", http.flag(), "--max-time", "90"]);
            command.args(["--noproxy", "*", "--connect-to"]);
            command.arg(format!("localhost:{PORT}:127.0.0.1:{}", self.console.port));
            command.args(["--include", "--cacert"]).arg(&self.console.pem);
            command.args(["--request", call.method]);
            for (name, value) in call.headers(&self.alice) {
                command.arg("--header").arg(format!("{name}: {value}"));
            }
            if let Some(body) = &call.body {
                command.arg("--data-binary").arg(body);
            }
            command.arg("--output").arg(&file);
            command.args(["--write-out", WRITE_OUT]);
            // What curl says of the last connection, for how it ended.
            if number + 1 == calls.len() {
                command.arg("--verbose");
            }
            command.arg(format!("{ORIGIN}{}", call.path));
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
            let (_, version, connects) = summary(line)
                .ok_or_else(|| format!("curl's summary of a request is {line:?}, not as asked"))?;
            carried.push(Carried { reply: parse_reply(&raw)?, version, connects });
        }
        if carried.len() != calls.len() {
            return Err(format!(
                "curl carried out {} of {} requests and said {said:?}",
                carried.len(),
                calls.len()
            ));
        }
        Ok(Run { carried, said, ok: out.status.success() })
    }
}

/// What curl prints of each request, one line: the status, the version of
/// HTTP and how many connections it opened for the request.
const WRITE_OUT: &str = "%{http_code} %{http_version} %{num_connects}\\n";

/// The three words of a line that [`WRITE_OUT`] printed.
fn summary(line: &str) -> Option<(u16, String, u32)> {
    let mut words = line.split_whitespace();
    let status = words.next()?.parse().ok()?;
    let version = words.next()?.to_string();
    let connects = words.next()?.parse().ok()?;
    words.next().is_none().then_some((status, version, connects))
}

/// The version of HTTP a run of curl asks for. The last of curl's two
/// options for it wins, and each request of a run has its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Http {
    One,
    Two,
}

impl Http {
    /// curl's option for it.
    fn flag(self) -> &'static str {
        match self {
            Http::One => "--http1.1",
            Http::Two => "--http2",
        }
    }

    /// The version as `%{http_version}` says it.
    fn word(self) -> &'static str {
        match self {
            Http::One => "1.1",
            Http::Two => "2",
        }
    }
}

/// A request of a run of curl.
struct Call {
    method: &'static str,
    path: String,
    body: Option<String>,
}

impl Call {
    /// A read of `path`.
    fn get(path: &str) -> Call {
        Call { method: "GET", path: path.to_string(), body: None }
    }

    /// A change of `path` with `body`.
    fn post(path: &str, body: &Value) -> Call {
        Call { method: "POST", path: path.to_string(), body: Some(body.to_string()) }
    }

    /// The headers the page sends with it: for a read the session's cookie
    /// alone, as [`Console::get`] sends it, for a change those of a change.
    fn headers(&self, browser: &Browser) -> Vec<(String, String)> {
        let all = browser.page_headers();
        if self.method == "GET" {
            all.into_iter().filter(|(name, _)| name == "Cookie").collect()
        } else {
            all
        }
    }
}

/// What curl made of one request.
struct Carried {
    reply: Reply,
    /// The version of HTTP it went by.
    version: String,
    /// How many connections curl opened for it: none when it took the one
    /// an earlier request had.
    connects: u32,
}

/// What one run of curl did.
struct Run {
    carried: Vec<Carried>,
    /// What curl said on its error output, with the last request's
    /// `--verbose`.
    said: String,
    /// Whether curl ended without an error of its own.
    ok: bool,
}

/// How a run of curl was told that the console ends the connection after an
/// answer: whether `said`, curl's account of the last request, has the
/// GOAWAY frame that closes an HTTP/2 connection with no error.
fn goaway(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    said.contains("goaway") && said.contains("error=0")
}

/// The machine as it is before any change, which each undo must give
/// back.
struct Machine {
    /// The rules of the file, as the page reads them.
    rules: Vec<String>,
    /// The file as it reads, empty when there is none.
    firewall: String,
    /// The same of the card's settings.
    network: String,
    /// How many rules the kernel holds.
    kernel: u64,
    /// How many of each trial Spark undid already.
    undone_firewall: usize,
    undone_network: usize,
}

impl Machine {
    /// Reads what the machine is now.
    fn take(vm: &mut Vm, test: &mut Test) -> Result<Machine, String> {
        let rules = test.rules()?;
        if rules.iter().any(|rule| rule == RULE) {
            return Err(format!("the machine already has the rule {RULE}: {rules:?}"));
        }
        let document = test.document()?;
        if !trials_of(&document).is_empty() {
            return Err(format!("Spark is trying something before the test begins: {document}"));
        }
        Ok(Machine {
            rules,
            firewall: test.file(vm, FIREWALL)?,
            network: test.file(vm, NETWORK)?,
            kernel: test.kernel_rules(vm)?,
            undone_firewall: undone_in(&document, "firewall").len(),
            undone_network: undone_in(&document, "network").len(),
        })
    }
}

/// Step 1: the page's read of the firewall is `firewall list --json`, the
/// same for alice and for bob, with no trial.
fn reads_agree(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let hers = test.read(Who::Alice, "/api/1/firewall")?;
    let his = test.read(Who::Bob, "/api/1/firewall")?;
    let typed = test.pane.output(vm, "firewall list --json")?;
    let typed: Value = serde_json::from_str(typed.trim())
        .map_err(|err| format!("firewall list --json printed {typed:?}: {err}"))?;
    if hers != his || hers != typed {
        return Err(format!(
            "the firewall reads differently: alice {hers}, bob {his}, the command {typed}"
        ));
    }
    if !hers["trial"].is_null() {
        return Err(format!("the firewall is tried before any change: {hers}"));
    }
    Ok(())
}

/// The arguments of the rule, tried for `window` seconds if there is one.
fn rule(window: Option<i64>) -> Value {
    let mut rule: Value = serde_json::from_str(RULE_ARGS).unwrap_or_default();
    if let Some(window) = window {
        rule["try"] = json!(window);
    }
    rule
}

/// Step 2: the rule is added with a window. Spark has the trial and a copy
/// of the file, the file has the rule, the kernel one more, and the journal
/// a row.
fn a_tried_add(vm: &mut Vm, test: &mut Test, machine: &Machine) -> Result<(), String> {
    let after = test.newest()?;
    test.change("/api/1/firewall/add", &rule(Some(WINDOW)))?;
    let document = test.document()?;
    let trial = trial_in(&document, "firewall")
        .ok_or_else(|| format!("Spark has no trial firewall after the add: {document}"))?;
    if trial["files"] != json!([FIREWALL]) || trial["left_ms"].as_u64().is_none_or(|ms| ms == 0) {
        return Err(format!("the trial firewall is {trial}, not of {FIREWALL} with time left"));
    }
    let list = test.read(Who::Alice, "/api/1/firewall")?;
    let rules = rule_texts(&list).unwrap_or_default();
    if !rules.iter().any(|rule| rule == RULE) || list["trial"]["left_ms"].as_u64().is_none() {
        return Err(format!("the rules after the add are {list}, which should have {RULE} tried"));
    }
    test.kernel_rules_are(vm, machine.kernel + 1, 60)?;
    test.console_row(after, "firewall.add", FIREWALL)?;
    Ok(())
}

/// Step 3: the window ends. The trial is gone from Spark's document and an
/// undone entry says why; the file is as it was and the kernel's rules too;
/// the journal has Spark's row for it and no hand edit.
fn is_undone(vm: &mut Vm, test: &mut Test, machine: &Machine, first: i64) -> Result<(), String> {
    let undone = test.undone_entry("firewall", machine.undone_firewall, UNDO_SECONDS)?;
    if undone["why"] != json!(TIME_UP) || undone["files"] != json!([FIREWALL]) {
        return Err(format!("Spark undid the trial as {undone}, not {TIME_UP:?} of {FIREWALL}"));
    }
    let file = test.file(vm, FIREWALL)?;
    if file != machine.firewall {
        return Err(format!("{FIREWALL} is {file:?} after the undo, not {:?}", machine.firewall));
    }
    test.kernel_rules_are(vm, machine.kernel, 60)?;
    let row = test.undone_row(first, FIREWALL, TIME_UP, ROW_SECONDS)?;
    let told = row["files"].as_array().map_or(0, Vec::len);
    if told != 1 {
        return Err(format!("the undone row names {told} files, not one: {row}"));
    }
    test.no_hand_edit(first, FIREWALL)
}

/// Step 4: the same with a window, kept at once: after the window the rule
/// is still there, in the file and in the kernel, and Spark undid nothing;
/// `firewall.remove` takes it away.
fn is_kept(vm: &mut Vm, test: &mut Test, machine: &Machine) -> Result<(), String> {
    // The pane's password lasts 15 minutes, which the steps before may
    // have used most of.
    elevated(vm)?;
    let undone = undone_in(&test.document()?, "firewall").len();
    let begun = Instant::now();
    test.change("/api/1/firewall/add", &rule(Some(KEPT_WINDOW)))?;
    test.change("/api/1/firewall/trial/keep", &json!({}))?;
    let document = test.document()?;
    if let Some(trial) = trial_in(&document, "firewall") {
        return Err(format!("the trial is still Spark's after the keep: {trial}"));
    }
    // The one wait that is no poll: the rule must be there after the time
    // that would have undone it.
    let wait = Duration::from_secs(u64::try_from(KEPT_WINDOW).unwrap_or(0) + KEPT_BEYOND);
    std::thread::sleep(wait.saturating_sub(begun.elapsed()));
    let rules = test.rules()?;
    if !rules.iter().any(|rule| rule == RULE) {
        return Err(format!("the kept rule is gone after its window: {rules:?}"));
    }
    let document = test.document()?;
    if undone_in(&document, "firewall").len() != undone || trial_in(&document, "firewall").is_some()
    {
        return Err(format!("Spark undid or tries something after the keep: {document}"));
    }
    test.kernel_rules_are(vm, machine.kernel + 1, 30)?;
    test.change("/api/1/firewall/remove", &rule(None))?;
    let rules = test.rules()?;
    if rules != machine.rules {
        return Err(format!("the rules after the remove are {rules:?}, not {:?}", machine.rules));
    }
    test.kernel_rules_are(vm, machine.kernel, 60)
}

/// Step 5: while a trial runs, a second add is NET-177 and is not made; for
/// bob it is ADM-122. The trial ends when alice undoes it.
fn a_second_change(vm: &mut Vm, test: &mut Test, machine: &Machine) -> Result<(), String> {
    test.change("/api/1/firewall/add", &rule(Some(WINDOW)))?;
    let other = json!({"action": "allow", "protocol": "tcp", "ports": "8"});
    test.unlocked()?;
    test.post(Who::Alice, "/api/1/firewall/add", &other)?.refused(
        "a second add while one is tried",
        422,
        "NET-177",
    )?;
    test.post(Who::Bob, "/api/1/firewall/add", &other)?.refused("bob's add", 403, "ADM-122")?;
    let rules = test.rules()?;
    if rules.iter().any(|rule| rule.contains(" 8")) || !rules.iter().any(|rule| rule == RULE) {
        return Err(format!("the rules after the refused add are {rules:?}"));
    }
    test.change("/api/1/firewall/trial/undo", &json!({}))?;
    let rules = test.rules()?;
    let document = test.document()?;
    if rules != machine.rules || trial_in(&document, "firewall").is_some() {
        return Err(format!(
            "after the undo the rules are {rules:?} and Spark's document {document}"
        ));
    }
    test.kernel_rules_are(vm, machine.kernel, 60)
}

/// Step 6, over `http`: one run of curl keeps one connection for the
/// session, the add with a window and the route `fresh`. The third is 409,
/// ADM-133, and the console ends the connection after it. A new run is 200
/// with where the request came from and where to; the trial is kept and the
/// rule taken away.
fn is_fresh(test: &mut Test, http: Http) -> Result<(), String> {
    test.unlocked()?;
    let calls = [
        Call::get("/api/1/session"),
        Call::post("/api/1/firewall/add", &rule(Some(WINDOW * 2))),
        Call::get("/api/1/console/fresh/firewall"),
    ];
    let run = test.carry(http, &calls)?;
    let [session, add, old] = &run.carried[..] else {
        return Err("a run of three requests carried out another number".into());
    };
    for (what, request) in [("the session", session), ("the add", add), ("fresh", old)] {
        if request.version != http.word() {
            return Err(format!(
                "{what} went by HTTP {}, not {}, over {}",
                request.version,
                http.word(),
                http.flag()
            ));
        }
    }
    if session.reply.status != 200 || add.reply.status != 200 {
        return Err(format!(
            "the session was answered {}, and the add {} {}",
            session.reply.status, add.reply.status, add.reply.body
        ));
    }
    if session.connects != 1 || add.connects != 0 || old.connects != 0 {
        return Err(format!(
            "curl opened {}, {} and {} connections for three requests that share one",
            session.connects, add.connects, old.connects
        ));
    }
    if !is_refusal(&old.reply, 409, "ADM-133") {
        return Err(format!(
            "fresh by the connection opened before the add was answered {} {}, not ADM-133 with 409",
            old.reply.status, old.reply.body
        ));
    }
    match http {
        Http::One => {
            if old.reply.header("connection") != Some("close") {
                return Err(format!(
                    "the answer of fresh does not end the connection: {:?}",
                    old.reply.headers
                ));
            }
        }
        Http::Two => {
            if !goaway(&run.said) {
                return Err(format!(
                    "curl was not told that the server ended the connection by a GOAWAY: {}",
                    run.said
                ));
            }
        }
    }
    if !run.ok {
        return Err(format!("curl ended with an error of its own: {}", run.said));
    }

    // A new run opens a new connection, opened after the change.
    let new = test.carry(http, &[Call::get("/api/1/console/fresh/firewall")])?;
    let [one] = &new.carried[..] else {
        return Err("one request carried out another number".into());
    };
    if one.connects != 1 || one.version != http.word() {
        return Err(format!(
            "the new run opened {} connections by HTTP {}",
            one.connects, one.version
        ));
    }
    let answer = one.reply.json()?;
    if one.reply.status != 200 || answer["fresh"] != json!(true) {
        return Err(format!(
            "fresh on a new connection was answered {} {answer}",
            one.reply.status
        ));
    }
    let network = test.read(Who::Alice, "/api/1/network")?;
    comes_from_and_by(&answer, &network)?;

    test.change("/api/1/firewall/trial/keep", &json!({}))?;
    test.change("/api/1/firewall/remove", &rule(None))?;
    Ok(())
}

/// Whether `reply` is the console's error `id` with `status`, as
/// [`Reply::refused`] holds an answer to, for a reply that is only looked at.
fn is_refusal(reply: &Reply, status: u16, id: &str) -> bool {
    let said = reply.json().ok().and_then(|body| body["error"]["id"].as_str().map(str::to_string));
    reply.status == status && said.as_deref() == Some(id)
}

/// Checks that `answer`, of a route that tells where a request came from,
/// says the host's address as `from` and where it came to: the port of the
/// console and the card that holds the address, as `network` lists the
/// cards, or no card when none does.
fn comes_from_and_by(answer: &Value, network: &Value) -> Result<(), String> {
    let from = answer["from"].as_str().unwrap_or_default();
    if !from.starts_with(PEER) {
        return Err(format!("the request came from {from:?}, not {PEER}: {answer}"));
    }
    let via = &answer["via"];
    let address = via["address"].as_str().unwrap_or_default();
    if address.is_empty() || via["port"] != json!(PORT) {
        return Err(format!("the request came to {via}, not to port {PORT}"));
    }
    let card = card_holding(network, address);
    if via["card"] != json!(card) {
        return Err(format!("the request came by the card {}, not {card:?}", via["card"]));
    }
    Ok(())
}

/// Step 7: the card is set to a second address for a window. The address is
/// there, in the list of cards and in `ip addr`, and the window ends: it is
/// gone, the file is as before and the journal has Spark's row.
fn a_tried_card(vm: &mut Vm, test: &mut Test, machine: &Machine, first: i64) -> Result<(), String> {
    elevated(vm)?;
    let after = test.newest()?;
    let settings = json!({
        "mode": "fixed",
        "addresses": format!("10.0.2.15/24 {EXTRA_ADDRESS}/24"),
        "router": "10.0.2.2",
        "dns": "10.0.2.3",
        "try": WINDOW,
    });
    test.change(&format!("/api/1/network/cards/{CARD}"), &settings)?;
    let document = test.document()?;
    if trial_in(&document, "network").is_none_or(|trial| trial["files"] != json!([NETWORK])) {
        return Err(format!("Spark has no trial network of {NETWORK}: {document}"));
    }
    test.card_has(EXTRA_ADDRESS, true, 60)?;
    let said = test.pane.output(vm, "ip addr | grep 10.0.2")?;
    if !said.contains(EXTRA_ADDRESS) {
        return Err(format!("ip addr does not show {EXTRA_ADDRESS}: {said:?}"));
    }
    test.console_row(after, "network.set", NETWORK)?;

    let undone = test.undone_entry("network", machine.undone_network, UNDO_SECONDS)?;
    if undone["why"] != json!(TIME_UP) || undone["files"] != json!([NETWORK]) {
        return Err(format!("Spark undid the trial as {undone}, not {TIME_UP:?} of {NETWORK}"));
    }
    test.card_has(EXTRA_ADDRESS, false, 60)?;
    let said = test.pane.output(vm, "ip addr | grep 10.0.2")?;
    if said.contains(EXTRA_ADDRESS) {
        return Err(format!("ip addr shows {EXTRA_ADDRESS} after the undo: {said:?}"));
    }
    let file = test.file(vm, NETWORK)?;
    if file != machine.network {
        return Err(format!("{NETWORK} is {file:?} after the undo, not {:?}", machine.network));
    }
    test.undone_row(after, NETWORK, TIME_UP, ROW_SECONDS)?;
    test.no_hand_edit(first, NETWORK)
}

/// Step 8: a firewall change tried for ten minutes, the disk synced and the
/// machine reset. Spark's line that it undid `firewall` comes before the
/// start of the `firewall` service; once the console is up the rules are as
/// before and the journal has the row, by Spark, with its words.
fn cut_short(vm: &mut Vm, test: &mut Test, machine: &Machine, first: i64) -> Result<(), String> {
    let after = test.newest()?;
    test.change("/api/1/firewall/add", &rule(Some(LONG_WINDOW)))?;
    let document = test.document()?;
    if trial_in(&document, "firewall").is_none() {
        return Err(format!("Spark has no trial firewall before the reset: {document}"));
    }
    // What Spark and the settings hold must be on the disk when the power
    // goes.
    if test.pane.status(vm, "sync")? != 0 {
        return Err("sync failed before the reset".into());
    }
    vm.monitor("system_reset")?;

    vm.expect("spark: started slot a, @system.a")?;
    vm.expect(&format!("{UNDID_AT_START}{FIREWALL}"))?;
    vm.expect(FIREWALL_STARTED)?;
    vm.expect("netd: net0 has 10.0.2.15/24")?;
    vm.expect("admind: answers at ")?;
    test.console.trust_what_is_served(Duration::from_secs(90))?;
    test.sign_in()?;

    let rules = test.rules()?;
    if rules != machine.rules {
        return Err(format!("the rules after the reset are {rules:?}, not {:?}", machine.rules));
    }
    let document = test.document()?;
    let undone = undone_in(&document, "firewall");
    if trial_in(&document, "firewall").is_some()
        || undone.last().is_none_or(|entry| entry["why"] != json!(RESTARTED))
    {
        return Err(format!("Spark's document after the reset is {document}"));
    }
    test.undone_row(after, FIREWALL, RESTARTED, ROW_SECONDS)?;
    test.no_hand_edit(first, FIREWALL)?;
    test.no_hand_edit(first, NETWORK)
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
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

/// The texts of the rules in the answer of `GET /api/1/firewall`.
fn rule_texts(list: &Value) -> Option<Vec<String>> {
    list["rules"].as_array()?.iter().map(|rule| rule["text"].as_str().map(str::to_string)).collect()
}

/// The trials in Spark's document.
fn trials_of(document: &Value) -> Vec<&Value> {
    document["trials"].as_array().map_or_default(|trials| trials.iter().collect())
}

/// The trial `id` of Spark's document, if it is being tried.
fn trial_in<'a>(document: &'a Value, id: &str) -> Option<&'a Value> {
    trials_of(document).into_iter().find(|trial| trial["id"] == id)
}

/// The entries of Spark's document for the trials `id` that were undone,
/// oldest first.
fn undone_in<'a>(document: &'a Value, id: &str) -> Vec<&'a Value> {
    let undone = document["undone"].as_array();
    undone.map_or_default(|all| all.iter().filter(|entry| entry["id"] == id).collect())
}

/// Whether the journal's `row` names `file` among its files.
fn names(row: &Value, file: &str) -> bool {
    row["files"].as_array().is_some_and(|files| files.iter().any(|one| one["path"] == file))
}

/// Whether `row` is Spark's row for a trial it undid, with its words `why`,
/// that put `file` back.
fn is_undone_row(row: &Value, file: &str, why: &str) -> bool {
    row["who"] == "Spark" && row["how"] == "undone" && row["note"] == why && names(row, file)
}

/// How many rules the kernel holds, from the first line of `firewall
/// status`: `Firewall: N rules; M packets dropped since the machine
/// started`.
fn kernel_rules(first_line: &str) -> Option<u64> {
    let rest = first_line.lines().find_map(|line| line.strip_prefix("Firewall: "))?;
    rest.split(" rules").next()?.trim().parse().ok()
}

/// The addresses of the card `card` in the answer of `GET /api/1/network`,
/// as `ADDRESS` each, without the prefix.
fn addresses_of(network: &Value, card: &str) -> Vec<String> {
    let cards = network["cards"].as_array();
    let found = cards.and_then(|all| all.iter().find(|one| one["name"] == card));
    let addresses = found.and_then(|one| one["addresses"].as_array());
    addresses.map_or_default(|all| {
        all.iter().filter_map(|one| one["address"].as_str().map(str::to_string)).collect()
    })
}

/// The name of the card that holds `address`, in the answer of `GET
/// /api/1/network`.
fn card_holding(network: &Value, address: &str) -> Option<String> {
    let cards = network["cards"].as_array()?;
    let found = cards.iter().find(|card| {
        let held = card["addresses"].as_array();
        held.is_some_and(|all| all.iter().any(|one| one["address"] == address))
    });
    found.and_then(|card| card["name"].as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Value {
        json!({
            "now_ms": 51234,
            "services": [],
            "trials": [
                {"id": "firewall", "files": [FIREWALL], "services": ["firewall"], "user": 1000, "left_ms": 42000},
            ],
            "undone": [
                {"id": "network", "why": TIME_UP, "at_ms": 100, "files": [NETWORK]},
                {"id": "firewall", "why": TIME_UP, "at_ms": 200, "files": [FIREWALL]},
                {"id": "firewall", "why": RESTARTED, "at_ms": 300, "files": [FIREWALL]},
            ],
        })
    }

    #[test]
    fn a_trial_and_the_undone_ones_are_found_by_their_name_in_sparks_document() {
        let document = document();
        assert_eq!(
            trial_in(&document, "firewall").map(|trial| &trial["left_ms"]),
            Some(&json!(42000))
        );
        assert!(trial_in(&document, "network").is_none());
        let undone = undone_in(&document, "firewall");
        assert_eq!(undone.len(), 2);
        assert_eq!(undone[1]["why"], json!(RESTARTED));
        assert_eq!(undone_in(&document, "network").len(), 1);
        assert!(undone_in(&json!({}), "firewall").is_empty());
        assert!(trials_of(&json!({"trials": []})).is_empty());
        assert_eq!(trials_of(&document).len(), 1);
    }

    #[test]
    fn the_rules_are_read_from_the_pages_answer() {
        let list = json!({
            "rules": [{"number": 1, "text": "allow tcp 443"}, {"number": 2, "text": "deny tcp 7"}],
            "problems": [],
            "trial": null,
        });
        assert_eq!(rule_texts(&list), Some(vec!["allow tcp 443".into(), RULE.into()]));
        assert_eq!(rule_texts(&json!({"rules": []})), Some(Vec::new()));
        assert_eq!(rule_texts(&json!({"rules": [{"number": 1}]})), None);
        assert_eq!(rule_texts(&json!({})), None);
    }

    #[test]
    fn the_kernel_s_rules_are_counted_from_the_first_line_of_the_status() {
        let said = "Firewall: 3 rules; 12 packets dropped since the machine started\n";
        assert_eq!(kernel_rules(said), Some(3));
        assert_eq!(
            kernel_rules("Firewall: 0 rules; 0 packets dropped since the machine started"),
            Some(0)
        );
        assert_eq!(kernel_rules("noise\nFirewall: 10 rules; 1 packets dropped"), Some(10));
        assert_eq!(kernel_rules("Firewall: many rules"), None);
        assert_eq!(kernel_rules(""), None);
        assert_eq!(kernel_rules("Other machines reach what services listen on"), None);
    }

    #[test]
    fn a_row_is_sparks_undo_of_a_file_by_its_words() {
        let row = json!({
            "id": 9, "who": "Spark", "how": "undone", "action": null, "note": TIME_UP,
            "files": [{"path": FIREWALL, "before": {}, "after": {}}],
        });
        assert!(is_undone_row(&row, FIREWALL, TIME_UP));
        assert!(!is_undone_row(&row, NETWORK, TIME_UP));
        assert!(!is_undone_row(&row, FIREWALL, RESTARTED));
        let mut other = row.clone();
        other["how"] = json!("by hand");
        assert!(!is_undone_row(&other, FIREWALL, TIME_UP));
        other["how"] = json!("undone");
        other["who"] = json!("alice");
        assert!(!is_undone_row(&other, FIREWALL, TIME_UP));
        assert!(names(&row, FIREWALL) && !names(&row, NETWORK));
        assert!(!names(&json!({}), FIREWALL));
    }

    #[test]
    fn the_card_that_holds_an_address_is_the_one_the_list_of_cards_names() {
        let network = json!({
            "cards": [
                {"name": "lo", "addresses": [{"address": "127.0.0.1", "prefix": 8, "from": "kernel"}]},
                {"name": "net0", "mac": "52:54:00:12:34:56", "addresses": [
                    {"address": "10.0.2.15", "prefix": 24, "from": "dhcp"},
                    {"address": "10.0.2.99", "prefix": 24, "from": "file"},
                ]},
            ],
            "trial": null,
        });
        assert_eq!(card_holding(&network, "10.0.2.15").as_deref(), Some("net0"));
        assert_eq!(card_holding(&network, "127.0.0.1").as_deref(), Some("lo"));
        assert_eq!(card_holding(&network, "192.168.1.1"), None);
        assert_eq!(card_holding(&json!({}), "10.0.2.15"), None);
        assert_eq!(addresses_of(&network, "net0"), ["10.0.2.15", "10.0.2.99"]);
        assert!(addresses_of(&network, "net1").is_empty());
        assert!(addresses_of(&json!({}), "net0").is_empty());
    }

    #[test]
    fn where_a_request_came_from_and_to_is_held_to_the_cards_the_machine_names() {
        let network = json!({"cards": [{"name": "net0", "addresses": [{"address": "10.0.2.15"}]}]});
        let answer = json!({
            "fresh": true, "from": "10.0.2.2",
            "via": {"address": "10.0.2.15", "port": 9090, "card": "net0"},
        });
        assert!(comes_from_and_by(&answer, &network).is_ok());
        let mut other = answer.clone();
        other["via"]["card"] = json!(null);
        assert!(comes_from_and_by(&other, &network).is_err());
        other["via"]["card"] = json!("net0");
        other["via"]["port"] = json!(9091);
        assert!(comes_from_and_by(&other, &network).is_err());
        other["via"]["port"] = json!(9090);
        other["from"] = json!("192.168.1.9");
        assert!(comes_from_and_by(&other, &network).is_err());
        // An address that no card holds has no card.
        let unknown = json!({
            "from": "10.0.2.2:5000",
            "via": {"address": "10.0.9.9", "port": 9090, "card": null},
        });
        assert!(comes_from_and_by(&unknown, &network).is_ok());
    }

    #[test]
    fn curls_summary_of_a_request_has_three_words() {
        assert_eq!(summary("409 2 0"), Some((409, "2".to_string(), 0)));
        assert_eq!(summary("200 1.1 1"), Some((200, "1.1".to_string(), 1)));
        assert_eq!(summary("200 1.1"), None);
        assert_eq!(summary("200 1.1 1 extra"), None);
        assert_eq!(summary("ok 1.1 1"), None);
        assert_eq!(summary(""), None);
    }

    #[test]
    fn a_goaway_with_no_error_is_how_curl_says_the_server_ended_the_connection() {
        let said = "* Connection #0 to host localhost left intact\n\
                    < HTTP/2 409 \n* received GOAWAY, error=0, last_stream=5\n* Closing connection\n";
        assert!(goaway(said));
        assert!(goaway("* Received GOAWAY, error=0, last_stream=1"));
        assert!(!goaway("* received GOAWAY, error=2, last_stream=5"));
        assert!(!goaway("* Closing connection"));
        assert!(!goaway(""));
    }

    #[test]
    fn a_request_sends_the_cookie_alone_for_a_read_and_the_pages_headers_for_a_change() {
        let browser = Browser { cookie: Some("abc".into()), token: Some("tok".into()) };
        let read = Call::get("/api/1/session").headers(&browser);
        assert_eq!(read, [("Cookie".to_string(), "__Host-dukovo=abc".to_string())]);
        let change = Call::post("/api/1/firewall/add", &json!({})).headers(&browser);
        assert!(change.iter().any(|(name, _)| name == "Origin"));
        assert!(change.iter().any(|(name, value)| name == "X-Dukovo-CSRF" && value == "tok"));
        assert!(change.iter().any(|(name, _)| name == "Cookie"));
    }

    #[test]
    fn the_rule_is_asked_for_with_a_window_or_without() {
        assert_eq!(rule(None), json!({"action": "deny", "protocol": "tcp", "ports": "7"}));
        assert_eq!(rule(Some(60))["try"], json!(60));
        assert_eq!(rule(Some(60))["ports"], json!("7"));
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
