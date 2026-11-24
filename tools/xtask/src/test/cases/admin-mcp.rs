// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console over MCP, Dukovo Admin: the AI clients an administrator
//! lets in, what they read, and the changes they ask for, which wait for a
//! yes in the console (ADR 0690, ADR 0691, ADR 0692). alice, the owner, lets
//! in three: lab-ssh, which connects over SSH and reaches every area;
//! lab-https, which connects over HTTPS and reaches the services, their
//! reads and asking for their changes; and lab-read, over HTTPS, with the
//! reads of the services that alice has before she unlocks. The reader
//! lab-reader, in group console and no administrator, makes the contrasts.
//!
//! Before a client is added nothing answers: /mcp with no token is ADM-143,
//! console.clients lists none, and admin mcp lab-ssh --check ends badly.
//! admin mcp add, with alice's password on its input, says the command the
//! client's settings run, or prints the token once, and refuses a name in
//! use. The page's route adds one from an unlocked session with no password,
//! and refuses the reader, who then has none. console.clients is what
//! elevate admin mcp list --all --json prints and the account's own route
//! what admin mcp list --json prints, and neither holds a token. The reader
//! is ADM-122 for the list, the requests and what the clients did, and alice
//! locked ADM-106 for the list.
//!
//! admin mcp lab-ssh --check says what the client reaches: the services
//! with changes to ask for, and the console's settings with none. It ends
//! badly for a client that connects over HTTPS and for a name that is no
//! client's.
//!
//! Over HTTPS, lab-https is answered in the newest revision, which has no
//! handshake and names its method and its tool in headers, and in those that
//! begin with initialize. A notification is 202 with no body, GET is
//! ADM-117, and a request that names another site as its origin is ADM-109.
//! It reads service.list as alice and service.timers as an administrator,
//! which goes through authd and the one program that reads for a client, and
//! both hold what the routes answer alice. The tool check says of a start
//! and of a stop what /api/1/check says, and of a restart that it always
//! changes something. A change given to the tool read, and an area outside
//! the scopes, are ADM-144. lab-read lists and reads what alice sees before
//! she unlocks; an administrator's read, a check and an ask are ADM-144 for
//! it. A client's token at the API is ADM-144, and an API token at /mcp is
//! ADM-143.
//!
//! Over SSH the test is the client: nc carries its lines from the host to
//! admin mcp lab-ssh in alice's pane, and the answers back. It reads as
//! alice and as an administrator through the socket, and its ask for
//! console.client-remove is ADM-144 although it reaches every area. Its ask
//! to restart diskhealth waits: the page's list has it, with the command a
//! yes would run, which is the one /api/1/show gives, and the same ask again
//! is the same request. The reader may not answer (ADM-122), alice locked
//! may not approve (ADM-106), and alice's yes runs the restart: both asks
//! and the tool request are told done with what the change answered, the
//! journal's row has alice as who and client lab-ssh of alice as how, and a
//! second yes is ADM-145. A no from alice's locked session tells the client
//! refused. A third request waits when alice removes the client by
//! console.client-remove: the list then has none, the client is told lapsed,
//! its next message is answered that the console closed the connection, and
//! the one after that it cannot connect. The stream of alice's page was told
//! how many wait, 1 and then 0.
//!
//! lab-https, removed by its owner in the console, and lab-read, removed by
//! the command that console.client-remove shows, are ADM-143 at their next
//! message, and a second removal is the command's own refusal. What the
//! clients did has the three requests as done, refused and lapsed, and each
//! client's reads. admind's log has a line for each call, with the action
//! and a refusal's ID. The audit log has each client let in and removed, the
//! pass, the reads as an administrator, the token of a client that was
//! removed, the yes and the no. No token is in the log or in the audit.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{
    ALICE_PASSWORD, Browser, Console, ORIGIN, Pane, Reply, elevated, events_named, tail, wait_for,
};
use super::super::*;
use super::admin_backups::{READER, READER_PASSWORD, Test, Who};

/// The client that connects over SSH, and reaches every area.
const OVER_SSH: &str = "lab-ssh";

/// The client that connects over HTTPS, and reaches the services: every
/// read of them, and asking for their changes.
const OVER_HTTPS: &str = "lab-https";

/// The client that connects over HTTPS with the reads of the services
/// that alice has before she unlocks the console.
const READS: &str = "lab-read";

/// The service the changes are about: every image runs it, and nothing
/// needs it.
const SERVICE: &str = "diskhealth";

/// The host, as the guest's network sees it: where the guest's nc reaches
/// the test, and where admind's log says the test's requests come from.
const HOST: &str = "10.0.2.2";

/// The revision of MCP that has no handshake.
const NEWEST: &str = "2026-07-28";

/// Every revision the server speaks, the newest first.
const REVISIONS: [&str; 5] = [NEWEST, "2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The tools, in the order a client is told of them.
const TOOLS: [&str; 6] = ["areas", "actions", "read", "check", "ask", "request"];

/// The route of console.clients, every account's clients.
const CLIENTS: &str = "/api/1/console/clients";

/// The routes of a person's own clients.
const OWN: &str = "/api/1/account/clients";
const OWN_ADD: &str = "/api/1/account/clients/add";
const OWN_REMOVE: &str = "/api/1/account/clients/remove";

/// The routes of the page: the requests that wait, what the clients did,
/// and the areas a client can be given.
const REQUESTS: &str = "/api/1/clients/requests";
const HISTORY: &str = "/api/1/clients/history";
const AREAS: &str = "/api/1/clients/areas";

/// Why the client says it asks, which the page shows as the client's own
/// words.
const REASON: &str = "The disk checks have not run since the last update.";

/// How long the console has to answer a line the client sent over SSH: a
/// read as an administrator starts a process for it.
const ANSWER_TIME: Duration = Duration::from_secs(120);

/// How long the tool ask waits for an administrator before it answers that
/// the request waits, as the console has it.
const ASK_WAIT: Duration = Duration::from_secs(45);

/// How soon an ask that waits must be told, once its request is answered.
const TOLD_WITHIN: Duration = Duration::from_secs(15);

/// The test.
pub const CASE: Case = Case {
    name: "admin-mcp",
    about: "The web console over MCP: before a client is added /mcp is ADM-143, console.clients lists none and admin mcp NAME --check ends badly; admin mcp add with the password on its input says the command to run over SSH or prints the token once and refuses a name in use, the page's route adds one from an unlocked session and refuses lab-reader; console.clients is elevate admin mcp list --all --json and the account's route admin mcp list --json, with no token in either, ADM-122 for the reader and ADM-106 for alice locked; --check says what a client reaches, the services with changes to ask for and the console's settings with none; over HTTPS a client is answered in the revision without a handshake and in those with initialize, a notification is 202, GET ADM-117 and another site's origin ADM-109, it reads service.list as alice and service.timers as an administrator through authd, both as the routes answer, check says what /api/1/check says, and a change given to read and an area outside its scopes are ADM-144; a client with :read lists and reads what alice sees before unlocking, and an administrator's read, a check and an ask are ADM-144; a client's token at the API is ADM-144 and an API token at /mcp ADM-143; over SSH, with nc carrying the test's lines to admin mcp NAME in alice's pane, the client reads through the socket, console.client-remove is ADM-144 for a client that reaches every area, an ask to restart diskhealth waits in the page's list with the command /api/1/show gives, the same ask again is the same request, the reader's answer is ADM-122 and locked alice's yes ADM-106, her yes runs the restart, both asks are told done, the journal has alice as who and client lab-ssh of alice as how, a second yes is ADM-145, a no from a locked session tells refused, a request that waits when its client is removed by console.client-remove is told lapsed, and the client's next messages are answered that the console closed the connection and that it cannot connect; the page's stream was told 1 and 0 waiting; clients removed by their owner and by the shown command are ADM-143; what they did has the requests done, refused and lapsed and each client's reads; admind's log has each call and the audit log each client let in and removed, the pass, the reads as an administrator, a removed client's token, the yes and the no, with no token in either (ADR 0690, ADR 0691, ADR 0692)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind and authd among
    // them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: admin_mcp,
    disks: &[],
    second: None,
};

fn admin_mcp(vm: &mut Vm, case: &Case) -> Result<(), String> {
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

    no_client_comes_with_the_system(vm, &mut test)?;
    let tokens = three_clients(vm, &mut test)?;
    the_lists(vm, &mut test, &tokens)?;
    a_check_at_a_terminal(vm, &test)?;
    over_https(&mut test, &tokens.https)?;
    within_its_scopes(&mut test, &tokens)?;
    let asked = over_ssh(vm, &mut test)?;
    the_others_removed(vm, &mut test, &tokens)?;
    what_they_did(vm, &mut test, &tokens, &asked)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The tokens of the two clients that connect over HTTPS, each shown once
/// when its client was added.
struct Tokens {
    /// lab-https's.
    https: String,
    /// lab-read's.
    reads: String,
}

/// The numbers of the three requests lab-ssh filed, by what became of each.
struct Asked {
    done: u64,
    refused: u64,
    lapsed: u64,
}

/// A request, as a client of the revisions that begin with `initialize`
/// sends it.
fn asks(id: u64, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A call of `tool` with `arguments`.
fn calls(id: u64, tool: &str, arguments: &Value) -> Value {
    asks(id, "tools/call", &json!({"name": tool, "arguments": arguments}))
}

/// The result in `answer`, which must answer the request `id` with one.
fn result_of(answer: &Value, id: u64, what: &str) -> Result<Value, String> {
    if answer["id"] == json!(id) && answer["result"].is_object() {
        return Ok(answer["result"].clone());
    }
    Err(format!("{what} was answered {answer}, which is no result of request {id}"))
}

/// What a tool's `result` says as data, which must be no failure.
fn told(result: &Value, what: &str) -> Result<Value, String> {
    if result["isError"] == json!(false) && result["structuredContent"].is_object() {
        return Ok(result["structuredContent"].clone());
    }
    Err(format!("{what} was answered {result}, which is a failure or holds no data"))
}

/// Checks that a tool's `result` is the console's failure `id`. A failure
/// of a tool is a result that says so, which the client reads.
fn fails(result: &Value, what: &str, id: &str) -> Result<(), String> {
    if result["isError"] == json!(true) && result["structuredContent"]["error"]["id"] == json!(id) {
        return Ok(());
    }
    Err(format!("{what} was answered {result}, not the failure {id}"))
}

/// What `/mcp` answers `message` from the client with `token`, which names
/// no revision and no origin.
fn sent(test: &Test, token: &str, message: &Value) -> Result<Reply, String> {
    test.console.with_token(token, "POST", "/mcp", Some(message))
}

/// The result of the tool `tool` with `arguments`, called over HTTPS by the
/// client with `token`.
fn called(test: &Test, token: &str, tool: &str, arguments: &Value) -> Result<Value, String> {
    let what = format!("the tool {tool} with {arguments}");
    let answer = sent(test, token, &calls(1, tool, arguments))?.must(&what, 200)?.json()?;
    result_of(&answer, 1, &what)
}

/// The headers of a request of the newest revision from the client with
/// `token`: the revision, the method, and the tool a call names.
fn newest_headers(token: &str, method: &str, tool: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("MCP-Protocol-Version".to_string(), NEWEST.to_string()),
        ("Mcp-Method".to_string(), method.to_string()),
    ];
    if let Some(tool) = tool {
        headers.push(("Mcp-Name".to_string(), tool.to_string()));
    }
    headers
}

/// The value under `key` of each object in the list `list`, in its order.
fn each(list: &Value, key: &str) -> Result<Vec<Value>, String> {
    let entries = list.as_array().ok_or_else(|| format!("{list} is not a list"))?;
    Ok(entries.iter().map(|entry| entry[key].clone()).collect())
}

/// The entry of the list `list` whose `key` is `value`.
fn entry<'a>(list: &'a Value, key: &str, value: &str) -> Option<&'a Value> {
    list.as_array()?.iter().find(|entry| entry[key] == value)
}

/// The token in what `admin mcp add --https` printed: the line that begins
/// as a client's token does.
fn token_in(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| line.starts_with("dkc_")).map(str::to_string)
}

/// The secret of a client's token `dkc_OWNER_SECRET`, the part authd keeps
/// only the SHA-256 of.
fn secret_of(token: &str) -> &str {
    token.rsplit('_').next().unwrap_or(token)
}

/// The line of a table `text` that is about `name`, its first word.
fn line_about<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find(|line| line.split_whitespace().next() == Some(name))
}

/// The requests that wait, as the page lists them for alice.
fn waiting(test: &mut Test) -> Result<Vec<Value>, String> {
    let listed = test.read(Who::Alice, REQUESTS)?;
    listed["requests"].as_array().cloned().ok_or_else(|| format!("no requests in {listed}"))
}

/// Waits for exactly one request to wait, and gives it as the page shows
/// it, with its number.
fn the_one_that_waits(test: &mut Test) -> Result<(u64, Value), String> {
    wait_for("one request waiting for an answer", 60, || {
        let now = waiting(test)?;
        Ok(match now.as_slice() {
            [one] => match one["request"].as_u64() {
                Some(number) => Ok((number, one.clone())),
                None => Err(format!("{one} has no number")),
            },
            others => Err(format!("{} requests wait", others.len())),
        })
    })
}

/// The number of the journal's newest row, 0 when it has none.
fn newest_row(test: &mut Test) -> Result<i64, String> {
    let list = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let rows = list["changes"].as_array().ok_or_else(|| format!("no changes in {list}"))?;
    Ok(rows.first().and_then(|row| row["id"].as_i64()).unwrap_or(0))
}

/// The rows of the journal made after row `after` for the action `action`.
fn rows_of(test: &mut Test, after: i64, action: &str) -> Result<Vec<Value>, String> {
    let list = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let rows = list["changes"].as_array().ok_or_else(|| format!("no changes in {list}"))?;
    let made = |row: &&Value| row["id"].as_i64().is_some_and(|id| id > after);
    Ok(rows.iter().filter(made).filter(|row| row["action"] == action).cloned().collect())
}

/// A client that connects over SSH, as far as a boot test can be one. Its
/// settings would run `ssh alice@HOST admin mcp NAME`; here alice's pane
/// runs `admin mcp NAME` between two runs of nc, one that brings the lines
/// the test writes on the host and one that takes the answers back, so the
/// test is the client and the bridge, the pass and the socket are the
/// machine's own.
struct Session {
    /// Where the test writes the client's messages.
    to: TcpStream,
    /// Where it reads the console's answers.
    from: BufReader<TcpStream>,
    /// The answers that came while the test waited for another: an ask is
    /// answered when a person answers its request, or when its 45 seconds
    /// are up, whatever the test does meanwhile.
    early: Vec<Value>,
}

impl Session {
    /// Starts `admin mcp NAME` in alice's pane, which keeps the pane until
    /// [`Session::close`].
    fn open(vm: &mut Vm, name: &str) -> Result<Session, String> {
        let listen = || -> Result<(TcpListener, u16), String> {
            let listener = TcpListener::bind("127.0.0.1:0")
                .map_err(|err| format!("no port for the AI client's lines: {err}"))?;
            listener.set_nonblocking(true).map_err(|err| err.to_string())?;
            let port = listener.local_addr().map_err(|err| err.to_string())?.port();
            Ok((listener, port))
        };
        let (messages, messages_port) = listen()?;
        let (answers, answers_port) = listen()?;
        // -d: the first nc reads nothing from the pane, and ends when the
        // host closes its side. -N: the second half-closes when the bridge
        // has ended, so that the host sees the end.
        let command = format!(
            "nc -d {HOST} {messages_port} | admin mcp {name} | nc -N {HOST} {answers_port}"
        );
        answer_prompt(vm, "$", &command)?;
        let to = Session::accepted(&messages, "to bring the AI client's messages")?;
        let from = Session::accepted(&answers, "to take the console's answers")?;
        from.set_read_timeout(Some(ANSWER_TIME)).map_err(|err| err.to_string())?;
        Ok(Session { to, from: BufReader::new(from), early: Vec::new() })
    }

    /// The connection the guest's nc makes to `listener`, within a minute.
    fn accepted(listener: &TcpListener, what: &str) -> Result<TcpStream, String> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).map_err(|err| err.to_string())?;
                    return Ok(stream);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(format!("the pane's nc did not connect {what} in 60 s"));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(format!("the pane's nc, {what}: {err}")),
            }
        }
    }

    /// Sends `message`, a line.
    fn send(&mut self, message: &Value) -> Result<(), String> {
        writeln!(self.to, "{message}")
            .and_then(|()| self.to.flush())
            .map_err(|err| format!("writing the AI client's message {message}: {err}"))
    }

    /// The next answer the console gives, a line.
    fn answer(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let read = self.from.read_line(&mut line).map_err(|err| {
            format!("no answer came over SSH in {} s: {err}", ANSWER_TIME.as_secs())
        })?;
        if read == 0 {
            return Err("admin mcp ended before it answered".into());
        }
        serde_json::from_str(line.trim())
            .map_err(|err| format!("admin mcp wrote {line:?}, which is not JSON: {err}"))
    }

    /// The answer to the request `id`, which may have come already.
    fn answer_to(&mut self, id: u64) -> Result<Value, String> {
        let to_it = |answer: &Value| answer["id"] == json!(id);
        if let Some(at) = self.early.iter().position(to_it) {
            return Ok(self.early.remove(at));
        }
        loop {
            let answer = self.answer().map_err(|err| {
                format!("request {id} has no answer: {err}; these came meanwhile: {:?}", self.early)
            })?;
            if to_it(&answer) {
                return Ok(answer);
            }
            self.early.push(answer);
        }
    }

    /// Sends the request `id` for `method` and gives its answer.
    fn asked(&mut self, id: u64, method: &str, params: &Value) -> Result<Value, String> {
        self.send(&asks(id, method, params))?;
        self.answer_to(id)
    }

    /// The result of the tool `tool` with `arguments`, called as request
    /// `id`.
    fn called(&mut self, id: u64, tool: &str, arguments: &Value) -> Result<Value, String> {
        let params = json!({"name": tool, "arguments": arguments});
        let answer = self.asked(id, "tools/call", &params)?;
        result_of(&answer, id, &format!("the tool {tool} with {arguments} over SSH"))
    }

    /// What each of the asks `ids` is told once its request is settled,
    /// which it has just been: answered, or lapsed. An ask waits 45
    /// seconds for that, and is told at once when it happens. On a machine
    /// so slow that `sent`, when the first of them was sent, is longer ago
    /// than that, an ask may have been told where its request stood then.
    /// That one is left out here, and the caller has the tool request, or
    /// the history, say what came of the request.
    fn outcomes(&mut self, ids: &[u64], sent: Instant) -> Result<Vec<Value>, String> {
        let settled = Instant::now();
        let late = settled.duration_since(sent) >= ASK_WAIT;
        let mut all = Vec::new();
        for id in ids {
            let came = told(&result_of(&self.answer_to(*id)?, *id, "an ask")?, "an ask")?;
            if matches!(came["state"].as_str(), Some("waiting" | "approved")) {
                if !late {
                    return Err(format!("ask {id} was told {came} of a request that is settled"));
                }
                continue;
            }
            if settled.elapsed() > TOLD_WITHIN {
                return Err(format!(
                    "ask {id} was told {came} only {} s after its request was settled",
                    settled.elapsed().as_secs()
                ));
            }
            all.push(came);
        }
        Ok(all)
    }

    /// Ends the client: the host closes its side, the first nc ends, admin
    /// mcp ends with its input, and the second nc says so. The pane then
    /// has its prompt again.
    fn close(mut self, vm: &mut Vm) -> Result<(), String> {
        if !self.early.is_empty() {
            return Err(format!("answers that no request of the test's had: {:?}", self.early));
        }
        self.to.shutdown(Shutdown::Both).map_err(|err| format!("closing the AI client: {err}"))?;
        let mut rest = String::new();
        loop {
            rest.clear();
            match self.from.read_line(&mut rest) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) => return Err(format!("admin mcp did not end with its input: {err}")),
            }
        }
        answer_prompt(vm, "$", "clear")
    }
}

/// Before a client is added, nothing answers MCP and nothing is listed: no
/// client comes with the system.
fn no_client_comes_with_the_system(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let ping = asks(1, "ping", &json!({})).to_string();
    let headers = [("Content-Type".to_string(), "application/json".to_string())];
    let nobody = test.console.send("POST", "/mcp", &headers, Some(&ping))?;
    let nobody = nobody.refused("/mcp with no token", 401, "ADM-143")?;
    if !nobody.detail().contains("dkc_") {
        return Err(format!("the refusal does not say what /mcp takes: {:?}", nobody.detail()));
    }
    test.unlocked()?;
    let all = test.read(Who::Alice, CLIENTS)?;
    if all != json!({"clients": []}) {
        return Err(format!("a machine nobody let a client into lists {all}"));
    }
    if test.pane.status(vm, &format!("admin mcp {OVER_SSH} --check"))? == 0 {
        return Err(format!("admin mcp {OVER_SSH} --check ended well before the client was added"));
    }
    Ok(())
}

/// alice lets in the three clients: two at her terminal, with her password
/// on the command's input, and one in the console, from her unlocked
/// session. The reader, who is no administrator, has none.
fn three_clients(vm: &mut Vm, test: &mut Test) -> Result<Tokens, String> {
    let add =
        |name: &str, rest: &str| format!("echo '{ALICE_PASSWORD}' | admin mcp add {name} {rest}");
    // Over SSH there is no secret: the command says what the client runs.
    let said = test.pane.output(vm, &add(OVER_SSH, "--scope '*'"))?;
    if !said.contains("ssh alice@") || !said.contains(&format!(" admin mcp {OVER_SSH}")) {
        return Err(format!("admin mcp add {OVER_SSH} does not say the command to run: {said:?}"));
    }
    // Over HTTPS the token is printed, this once.
    let said = test.pane.output(vm, &add(READS, "--https --scope services:read --days 7"))?;
    let reads = token_in(&said)
        .filter(|token| token.starts_with("dkc_alice_"))
        .ok_or_else(|| format!("admin mcp add {READS} --https printed no token: {said:?}"))?;
    // A client is never changed: its name is taken until it is removed.
    if test.pane.status(vm, &add(READS, "--scope logs"))? == 0 {
        return Err(format!("a second client called {READS} was let in"));
    }

    test.unlocked()?;
    let new = json!({"name": OVER_HTTPS, "way": "https", "scopes": ["services"], "days": 30});
    let added =
        test.post(Who::Alice, OWN_ADD, &new)?.must("adding a client in the console", 200)?;
    let added = added.json()?;
    let https = added["token"]
        .as_str()
        .filter(|token| token.starts_with("dkc_alice_"))
        .ok_or_else(|| format!("the console added {OVER_HTTPS} and answered {added}"))?
        .to_string();

    // The reader gives the right password, and is refused as no
    // administrator.
    let mine = json!({
        "proof": READER_PASSWORD,
        "name": "lab-mine",
        "way": "https",
        "scopes": ["services:read"],
        "days": 30,
    });
    let refused = test.post(Who::Reader, OWN_ADD, &mine)?;
    if refused.status == 200 {
        return Err(format!(
            "{READER}, who is no administrator, let in a client: {}",
            refused.body
        ));
    }
    let own = test.read(Who::Reader, OWN)?;
    if own != json!({"clients": []}) {
        return Err(format!("{READER} has the clients {own}"));
    }
    Ok(Tokens { https, reads })
}

/// The lists: every account's by the action and by its command, alice's own
/// by her route and by her command, with what each client is and nothing of
/// its token; and who may see them.
fn the_lists(vm: &mut Vm, test: &mut Test, tokens: &Tokens) -> Result<(), String> {
    test.unlocked()?;
    elevated(vm)?;
    let all = test.read(Who::Alice, CLIENTS)?;
    let typed = test.printed(vm, "elevate admin mcp list --all --json")?;
    if all != typed {
        return Err(format!(
            "console.clients answers {all}, and elevate admin mcp list --all --json prints {typed}"
        ));
    }
    let own = test.read(Who::Alice, OWN)?;
    let typed = test.printed(vm, "admin mcp list --json")?;
    if own != typed {
        return Err(format!("{OWN} answers {own}, and admin mcp list --json prints {typed}"));
    }
    // alice is the only one who let a client in.
    if own != all {
        return Err(format!("alice's own clients are {own}, and every account's {all}"));
    }
    let added = [
        (OVER_SSH, "ssh", json!(["*"])),
        (OVER_HTTPS, "https", json!(["services"])),
        (READS, "https", json!(["services:read"])),
    ];
    for (name, way, scopes) in added {
        let client = entry(&all["clients"], "name", name)
            .ok_or_else(|| format!("the list has no client {name}: {all}"))?;
        let as_added = client["owner"] == "alice"
            && client["way"] == way
            && client["scopes"] == scopes
            && client["ended"] == false
            && client["used"] == 0;
        if !as_added {
            return Err(format!("{name} is listed as {client}"));
        }
    }
    // authd keeps a token's SHA-256 alone, so no list can hold one.
    let text = all.to_string();
    for token in [&tokens.https, &tokens.reads] {
        if text.contains(secret_of(token)) {
            return Err("the list of clients holds a token's secret".into());
        }
    }
    // For a person to read.
    let table = test.pane.output(vm, "admin mcp list")?;
    for (name, connects) in [(OVER_SSH, "over SSH"), (OVER_HTTPS, "over HTTPS")] {
        if !line_about(&table, name).is_some_and(|line| line.contains(connects)) {
            return Err(format!("admin mcp list has no line for {name} {connects}: {table:?}"));
        }
    }

    // Every account's clients, the requests and what the clients did are
    // an administrator's, and the list is a read that runs as one.
    test.locked()?;
    for path in [CLIENTS, REQUESTS, HISTORY, AREAS] {
        test.get(Who::Reader, path)?.refused(&format!("{path} for {READER}"), 403, "ADM-122")?;
    }
    test.get(Who::Alice, CLIENTS)?.refused("console.clients for alice, locked", 403, "ADM-106")?;
    // To look at what waits, a locked session is enough.
    for path in [REQUESTS, HISTORY, AREAS] {
        test.read(Who::Alice, path)?;
    }
    Ok(())
}

/// `admin mcp NAME --check` connects as the client would, and says what it
/// reaches.
fn a_check_at_a_terminal(vm: &mut Vm, test: &Test) -> Result<(), String> {
    let said = test.pane.output(vm, &format!("admin mcp {OVER_SSH} --check"))?;
    if !said.contains(&format!("The AI client {OVER_SSH} connects. It reaches:")) {
        return Err(format!("admin mcp {OVER_SSH} --check said {said:?}"));
    }
    if !line_about(&said, "services").is_some_and(|line| line.contains("read all and ask")) {
        return Err(format!(
            "the check does not say the services' changes can be asked for: {said:?}"
        ));
    }
    // The console's settings decide who gets in: every read, and no change
    // to ask for, whatever the scopes.
    let console = line_about(&said, "console");
    if !console.is_some_and(|line| line.contains("read all") && !line.contains("and ask")) {
        return Err(format!(
            "the check does not say the console's settings are read only: {said:?}"
        ));
    }
    // A pass is for a client that connects over SSH, and that there is.
    for name in [OVER_HTTPS, "lab-nobody"] {
        if test.pane.status(vm, &format!("admin mcp {name} --check"))? == 0 {
            return Err(format!("admin mcp {name} --check ended well"));
        }
    }
    Ok(())
}

/// lab-https over HTTPS: the revisions of the protocol, what `/mcp` takes,
/// a read as its owner and one as an administrator, a check, and what its
/// scopes do not reach.
fn over_https(test: &mut Test, token: &str) -> Result<(), String> {
    // The newest revision has no handshake: each request says its revision
    // and its client's capabilities, and its headers say the same.
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": NEWEST,
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let discover = asks(1, "server/discover", &json!({"_meta": meta})).to_string();
    let headers = newest_headers(token, "server/discover", None);
    let found = test.console.send("POST", "/mcp", &headers, Some(&discover))?;
    let found = result_of(&found.must("server/discover", 200)?.json()?, 1, "server/discover")?;
    let server = &found["_meta"]["io.modelcontextprotocol/serverInfo"]["name"];
    if found["supportedVersions"] != json!(REVISIONS)
        || found["resultType"] != "complete"
        || server != "dukovo-admind"
    {
        return Err(format!("server/discover was answered {found}"));
    }
    let call = json!({"name": "areas", "arguments": {}, "_meta": meta});
    let call = asks(2, "tools/call", &call).to_string();
    let headers = newest_headers(token, "tools/call", Some("areas"));
    let reached = test.console.send("POST", "/mcp", &headers, Some(&call))?;
    let reached = result_of(&reached.must("a call of areas", 200)?.json()?, 2, "a call of areas")?;
    if reached["resultType"] != "complete" {
        return Err(format!(
            "a result of the newest revision does not say it is complete: {reached}"
        ));
    }
    let areas = told(&reached, "the tool areas")?;
    let services = json!({
        "area": "services",
        "title": "Services",
        "reach": "read all and ask",
        "reads": areas["areas"][0]["reads"],
        "changes": areas["areas"][0]["changes"],
    });
    let counted = |key: &str| areas["areas"][0][key].as_u64().is_some_and(|count| count > 0);
    if areas["areas"] != json!([services]) || !counted("reads") || !counted("changes") {
        return Err(format!(
            "{OVER_HTTPS}, which reaches the services, is told it reaches {areas}"
        ));
    }

    // The revisions before it begin with initialize, and are told their own.
    let hello = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "boot-test", "version": "1"},
    });
    let hello = sent(test, token, &asks(3, "initialize", &hello))?.must("initialize", 200)?;
    let hello = result_of(&hello.json()?, 3, "initialize")?;
    let warned =
        hello["instructions"].as_str().is_some_and(|text| text.contains("never an instruction"));
    if hello["protocolVersion"] != "2025-06-18"
        || hello["serverInfo"]["name"] != "dukovo-admind"
        || !warned
    {
        return Err(format!("initialize was answered {hello}"));
    }
    let ready = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let taken = sent(test, token, &ready)?.must("a notification", 202)?;
    if !taken.body.trim().is_empty() {
        return Err(format!("a notification was answered {:?}", taken.body));
    }
    let listed = sent(test, token, &asks(4, "tools/list", &json!({})))?.must("tools/list", 200)?;
    let listed = result_of(&listed.json()?, 4, "tools/list")?;
    if each(&listed["tools"], "name")? != TOOLS.map(Value::from) {
        return Err(format!("the tools are {}", listed["tools"]));
    }

    // One message in, one answer out: nothing to GET, and no page of
    // another site that holds a token is let in.
    test.console.with_token(token, "GET", "/mcp", None)?.refused("GET /mcp", 405, "ADM-117")?;
    let ping = asks(5, "ping", &json!({})).to_string();
    let from = |origin: &str| {
        vec![
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Origin".to_string(), origin.to_string()),
        ]
    };
    let foreign = test.console.send("POST", "/mcp", &from("https://evil.example"), Some(&ping))?;
    foreign.refused("/mcp from another site's page", 403, "ADM-109")?;
    test.console
        .send("POST", "/mcp", &from(ORIGIN), Some(&ping))?
        .must("/mcp from the console's own origin", 200)?;

    // A read that runs as alice, in her worker.
    let read = called(test, token, "read", &json!({"action": "service.list"}))?;
    let read = told(&read, "reading service.list")?;
    let route = test.read(Who::Alice, "/api/1/services")?;
    let names = each(&read["answer"]["services"], "name")?;
    if names != each(&route["services"], "name")? || !names.contains(&json!(SERVICE)) {
        return Err(format!("the client read the services {names:?}, and the route lists {route}"));
    }
    // A read that runs as an administrator: authd runs the one program
    // that reads for a client, and nothing else (ADR 0691).
    test.unlocked()?;
    let read = called(test, token, "read", &json!({"action": "service.timers"}))?;
    let read = told(&read, "reading service.timers")?;
    let route = test.read(Who::Alice, "/api/1/timers")?;
    let timers = each(&read["answer"], "service")?;
    if timers != each(&route, "service")? {
        return Err(format!("the client read the timers {timers:?}, and the route lists {route}"));
    }

    // check: whether a change would change anything, as the page asks it.
    let name = json!({"name": SERVICE});
    for action in ["service.start", "service.stop"] {
        let checked = called(test, token, "check", &json!({"action": action, "arguments": name}))?;
        let checked = told(&checked, &format!("checking {action}"))?;
        let asked = json!({"action": action, "args": name});
        let page =
            test.post(Who::Alice, "/api/1/check", &asked)?.must("/api/1/check", 200)?.json()?;
        if checked["changes"] != page["changed"] || checked["now"] != page["now"] {
            return Err(format!("check says {checked} of {action}, and /api/1/check {page}"));
        }
    }
    let restart = json!({"action": "service.restart", "arguments": name});
    let checked = told(&called(test, token, "check", &restart)?, "checking service.restart")?;
    if checked["changes"] != true || !checked["now"].is_null() {
        return Err(format!("check says {checked} of a restart, which always does something"));
    }

    // A change is asked for, never read; and the scopes name the services.
    fails(&called(test, token, "read", &restart)?, "a change given to read", "ADM-144")?;
    let logs = json!({"action": "logs.read"});
    fails(&called(test, token, "read", &logs)?, "a read outside the scopes", "ADM-144")?;
    let logs = json!({"area": "logs"});
    fails(
        &called(test, token, "actions", &logs)?,
        "the actions of an area outside the scopes",
        "ADM-144",
    )
}

/// lab-read has the reads alice has before she unlocks, and nothing more;
/// and a token is taken where its kind belongs, and nowhere else.
fn within_its_scopes(test: &mut Test, tokens: &Tokens) -> Result<(), String> {
    let token = &tokens.reads;
    let areas = told(&called(test, token, "areas", &json!({}))?, "the tool areas")?;
    let one = &areas["areas"][0];
    let only_reads = areas["areas"].as_array().is_some_and(|areas| areas.len() == 1)
        && one["area"] == "services"
        && one["reach"] == "read"
        && one["changes"] == 0;
    if !only_reads {
        return Err(format!("{READS}, with services:read, is told it reaches {areas}"));
    }
    let services = json!({"area": "services"});
    let listed = told(&called(test, token, "actions", &services)?, "the actions of the services")?;
    let ids = each(&listed["actions"], "id")?;
    let as_owner = listed["actions"].as_array().is_some_and(|all| {
        all.iter().all(|action| action["kind"] == "read" && action["as"] == "owner")
    });
    let has = |id: &str| ids.contains(&json!(id));
    if !as_owner || !has("service.list") || has("service.timers") || has("service.restart") {
        return Err(format!("{READS} is offered {listed}"));
    }
    told(
        &called(test, token, "read", &json!({"action": "service.list"}))?,
        "reading service.list",
    )?;
    let timers = json!({"action": "service.timers"});
    fails(&called(test, token, "read", &timers)?, "an administrator's read", "ADM-144")?;
    let restart = json!({"action": "service.restart", "arguments": {"name": SERVICE}});
    fails(&called(test, token, "check", &restart)?, "a check with :read", "ADM-144")?;
    let mut ask = restart;
    ask["reason"] = json!(REASON);
    fails(&called(test, token, "ask", &ask)?, "an ask with :read", "ADM-144")?;
    if !waiting(test)?.is_empty() {
        return Err("a request waits that a client with :read asked for".into());
    }

    // A client's token is no API token, and an API token no client's.
    let ping = asks(1, "ping", &json!({}));
    let elsewhere = test.console.with_token(&tokens.https, "GET", "/api/1/services", None)?;
    elsewhere.refused("a client's token at the API", 403, "ADM-144")?;
    let proof =
        json!({"proof": ALICE_PASSWORD, "name": "lab-api", "scopes": ["services"], "days": 1});
    let made = test.post(Who::Alice, "/api/1/account/tokens/add", &proof)?;
    let made = made.must("alice's API token", 200)?.json()?;
    let api = made["token"].as_str().ok_or("no API token was made")?;
    test.console
        .with_token(api, "GET", "/api/1/services", None)?
        .must("an API token at the API", 200)?;
    sent(test, api, &ping)?.refused("an API token at /mcp", 401, "ADM-143")?;
    Ok(())
}

/// lab-ssh over SSH: what it reads through the socket, a change from the
/// asking to the yes, a no, and what becomes of a request and of the
/// connection when the client is removed.
fn over_ssh(vm: &mut Vm, test: &mut Test) -> Result<Asked, String> {
    let events = test.console.events(&test.alice)?;
    let mut client = Session::open(vm, OVER_SSH)?;
    let hello = json!({
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "boot-test", "version": "1"},
    });
    let hello = client.asked(1, "initialize", &hello)?;
    if hello["result"]["protocolVersion"] != "2025-11-25" {
        return Err(format!("initialize over SSH was answered {hello}"));
    }
    client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;

    // It reaches every area: the services with changes to ask for, and the
    // console's settings, which decide who gets in, with none.
    let areas = told(&client.called(2, "areas", &json!({}))?, "the tool areas over SSH")?;
    let reach = |area: &str| entry(&areas["areas"], "area", area).map(|area| area["reach"].clone());
    if reach("services") != Some(json!("read all and ask"))
        || reach("console") != Some(json!("read all"))
    {
        return Err(format!("{OVER_SSH}, which reaches every area, is told it reaches {areas}"));
    }
    let read = client.called(3, "read", &json!({"action": "service.list"}))?;
    let read = told(&read, "reading service.list over SSH")?;
    if !each(&read["answer"]["services"], "name")?.contains(&json!(SERVICE)) {
        return Err(format!("the services read over SSH have no {SERVICE}: {read}"));
    }
    let read = client.called(4, "read", &json!({"action": "service.timers"}))?;
    let read = told(&read, "reading service.timers over SSH")?;
    if !read["answer"].is_array() {
        return Err(format!("the timers read over SSH are {read}"));
    }
    // No client is offered a change to who gets in, whatever it reaches.
    let remove = json!({
        "action": "console.client-remove",
        "arguments": {"owner": "alice", "name": READS},
        "reason": "To see what no client may ask for.",
    });
    fails(&client.called(5, "ask", &remove)?, "an ask to remove a client", "ADM-144")?;

    // A change waits for a yes: the page shows who asks, why, and the
    // command that would run.
    let name = json!({"name": SERVICE});
    let restart = json!({"action": "service.restart", "arguments": name, "reason": REASON});
    let sent = Instant::now();
    client.send(&calls(10, "ask", &restart))?;
    let (done, card) = the_one_that_waits(test)?;
    let show = json!({"action": "service.restart", "args": name});
    let shown = test.post(Who::Alice, "/api/1/show", &show)?.must("/api/1/show", 200)?.json()?;
    let as_asked = card["owner"] == "alice"
        && card["client"] == OVER_SSH
        && card["action"] == "service.restart"
        && card["area"] == "services"
        && card["args"] == name
        && card["reason"] == REASON
        && card["command"].is_string()
        && card["command"] == shown["command"]
        && card["files"] == shown["files"];
    if !as_asked {
        return Err(format!("the request is shown as {card}, and /api/1/show gives {shown}"));
    }
    // The same ask while it waits is the same request. The call after it
    // is answered once the socket has taken both, so the second ask is
    // filed before anyone answers the request.
    client.send(&calls(11, "ask", &restart))?;
    let stands = told(&client.called(12, "request", &json!({"id": done}))?, "the tool request")?;
    if stands["state"] != "waiting" || stands["request"] != done {
        return Err(format!("the request that waits is told as {stands}"));
    }
    if waiting(test)?.len() != 1 {
        return Err("the same ask a second time filed a second request".into());
    }

    // Who may answer: an administrator, and for a yes one who has unlocked.
    let before = newest_row(test)?;
    test.locked()?;
    let approve = format!("{REQUESTS}/{done}/approve");
    let refuse = format!("{REQUESTS}/{done}/refuse");
    test.post(Who::Reader, &approve, &json!({}))?.refused("the reader's yes", 403, "ADM-122")?;
    test.post(Who::Reader, &refuse, &json!({}))?.refused("the reader's no", 403, "ADM-122")?;
    test.post(Who::Alice, &approve, &json!({}))?.refused("a yes while locked", 403, "ADM-106")?;
    test.unlocked()?;
    let answer = test.post(Who::Alice, &approve, &json!({}))?.must("alice's yes", 200)?.json()?;
    if answer["done"] != true {
        return Err(format!("the approved restart answered {answer}"));
    }
    // Both asks are told what came of it, and so is the tool request.
    let as_done = |came: &Value| {
        came["state"] == "done"
            && came["by"] == "alice"
            && came["request"] == done
            && came["answer"] == answer
    };
    for came in client.outcomes(&[10, 11], sent)? {
        if !as_done(&came) {
            return Err(format!("the approved ask was told {came}, and the yes answered {answer}"));
        }
    }
    let stands = told(&client.called(13, "request", &json!({"id": done}))?, "the tool request")?;
    if !as_done(&stands) {
        return Err(format!(
            "the approved request is told as {stands}, and the yes answered {answer}"
        ));
    }
    test.post(Who::Alice, &approve, &json!({}))?.refused("a second yes", 404, "ADM-145")?;
    // The journal says who made the change and whose client asked.
    let rows = rows_of(test, before, "service.restart")?;
    let how = format!("client {OVER_SSH} of alice");
    let as_approved =
        |row: &Value| row["who"] == "alice" && row["how"] == how.as_str() && row["status"] == 0;
    if rows.len() != 1 || !as_approved(&rows[0]) {
        return Err(format!("the journal's rows of the approved restart are {rows:?}"));
    }

    // A no, which a locked session may give. Nothing runs.
    let stop = json!({
        "action": "service.stop",
        "arguments": name,
        "reason": "To see what a no does.",
    });
    test.locked()?;
    let sent = Instant::now();
    client.send(&calls(20, "ask", &stop))?;
    let (refused, _) = the_one_that_waits(test)?;
    let refuse = format!("{REQUESTS}/{refused}/refuse");
    let no = test.post(Who::Alice, &refuse, &json!({}))?.must("alice's no", 200)?.json()?;
    if no != json!({"done": true}) {
        return Err(format!("the no was answered {no}"));
    }
    let as_refused = |came: &Value| {
        came["state"] == "refused" && came["by"] == "alice" && came["request"] == refused
    };
    for came in client.outcomes(&[20], sent)? {
        if !as_refused(&came) {
            return Err(format!("the refused ask was told {came}"));
        }
    }
    let stands = told(&client.called(21, "request", &json!({"id": refused}))?, "the tool request")?;
    if !as_refused(&stands) {
        return Err(format!("the refused request is told as {stands}"));
    }
    test.post(Who::Alice, &refuse, &json!({}))?.refused("a second no", 404, "ADM-145")?;

    // The client is removed while a request of its waits, by the action
    // an administrator has for any account's client. admind hears of it
    // when the page next lists the requests: the request has lapsed.
    test.unlocked()?;
    let sent = Instant::now();
    client.send(&calls(30, "ask", &stop))?;
    let (lapsed, _) = the_one_that_waits(test)?;
    if lapsed == refused || lapsed == done {
        return Err(format!("a new request has the number {lapsed}, which another had"));
    }
    test.change(&format!("{CLIENTS}/alice/{OVER_SSH}/remove"), &json!({}))?;
    let left = waiting(test)?;
    if !left.is_empty() {
        return Err(format!("requests of a client that is gone still wait: {left:?}"));
    }
    // The ask is told so. On a machine too slow for that, the history
    // still says so at the end.
    for came in client.outcomes(&[30], sent)? {
        if came["state"] != "lapsed" || came["request"] != lapsed {
            return Err(format!("the ask of a removed client was told {came}"));
        }
    }
    if !rows_of(test, before, "service.stop")?.is_empty() {
        return Err("the journal has a stop that nobody approved".into());
    }
    // Its pass is taken no longer. The console closes the connection at
    // the next message, which the bridge answers, and sends nothing twice;
    // the message after that finds no pass to connect with.
    let closed = client.asked(40, "ping", &json!({}))?;
    let words = closed["error"]["message"].as_str().unwrap_or_default();
    if closed["error"]["code"] != -32603 || !words.contains("ask again") {
        return Err(format!("the first message of a removed client was answered {closed}"));
    }
    let gone = client.asked(41, "ping", &json!({}))?;
    let words = gone["error"]["message"].as_str().unwrap_or_default();
    if gone["error"]["code"] != -32603 || !words.contains("cannot connect to the console") {
        return Err(format!("the next message of a removed client was answered {gone}"));
    }
    client.close(vm)?;

    // The page was told how many wait as it changed: one, then none.
    wait_for("the page's stream to tell how many requests wait", 30, || {
        let so_far = events_named(&events.text(), "clients");
        let said = |count: u64| so_far.iter().any(|event| event["waiting"] == count);
        Ok(if said(1) && said(0) { Ok(()) } else { Err(format!("it told {so_far:?}")) })
    })?;
    Ok(Asked { done, refused, lapsed })
}

/// The two clients that connect over HTTPS are removed, one by its owner in
/// the console and one by the command the console shows for the action:
/// their tokens let nothing in from the next message on.
fn the_others_removed(vm: &mut Vm, test: &mut Test, tokens: &Tokens) -> Result<(), String> {
    let ping = asks(1, "ping", &json!({}));
    sent(test, &tokens.https, &ping)?.must(&format!("{OVER_HTTPS} before it is removed"), 200)?;
    let removed = test.post(Who::Alice, OWN_REMOVE, &json!({"name": OVER_HTTPS}))?;
    if removed.must("removing one's own client", 200)?.json()? != json!({"done": true}) {
        return Err(format!("removing {OVER_HTTPS} did not answer that it is done"));
    }
    sent(test, &tokens.https, &ping)?.refused("the token of a removed client", 401, "ADM-143")?;

    let of = json!({"action": "console.client-remove", "args": {"owner": "alice", "name": READS}});
    let shown = test.post(Who::Alice, "/api/1/show", &of)?.must("/api/1/show", 200)?.json()?;
    let command = shown["command"].as_str().ok_or_else(|| format!("no command in {shown}"))?;
    sent(test, &tokens.reads, &ping)?.must(&format!("{READS} before it is removed"), 200)?;
    elevated(vm)?;
    test.ends_with(vm, command, 0)?;
    sent(test, &tokens.reads, &ping)?.refused("the token of a removed client", 401, "ADM-143")?;
    // There is nothing left to remove, which the command says itself.
    if test.pane.status(vm, command)? == 0 {
        return Err(format!("`{command}` ended well a second time"));
    }
    test.unlocked()?;
    let all = test.read(Who::Alice, CLIENTS)?;
    if all != json!({"clients": []}) {
        return Err(format!("with every client removed, the list is {all}"));
    }
    Ok(())
}

/// What is kept of it all: the page's history, admind's log, and the audit
/// log, none of which holds a token.
fn what_they_did(
    vm: &mut Vm,
    test: &mut Test,
    tokens: &Tokens,
    asked: &Asked,
) -> Result<(), String> {
    let history = test.read(Who::Alice, HISTORY)?;
    let entries =
        history["entries"].as_array().ok_or_else(|| format!("no entries in {history}"))?;
    for (request, state, by) in [
        (asked.done, "done", "alice"),
        (asked.refused, "refused", "alice"),
        (asked.lapsed, "lapsed", ""),
    ] {
        let kept =
            entries.iter().find(|entry| entry["kind"] == "ask" && entry["request"] == request);
        let as_it_ended = kept.is_some_and(|entry| {
            entry["state"] == state
                && entry["owner"] == "alice"
                && entry["client"] == OVER_SSH
                && entry["by"].as_str().unwrap_or_default() == by
        });
        if !as_it_ended {
            return Err(format!("request {request}, which was {state}, is kept as {kept:?}"));
        }
    }
    for client in [OVER_SSH, OVER_HTTPS, READS] {
        let read = entries.iter().any(|entry| {
            entry["kind"] == "read"
                && entry["client"] == client
                && entry["areas"].as_array().is_some_and(|areas| areas.contains(&json!("services")))
        });
        if !read {
            return Err(format!("the history has no reads of {client} in the services: {history}"));
        }
    }

    // admind's log: a line for each call, with the action and never its
    // arguments, and a refusal's ID.
    let lines = [
        format!("AI client {OVER_HTTPS} of alice listed its areas"),
        format!("AI client {OVER_HTTPS} of alice read service.list"),
        format!("AI client {OVER_HTTPS} of alice read service.timers"),
        format!("AI client {OVER_HTTPS} of alice checked service.start"),
        format!("AI client {OVER_HTTPS} of alice read logs.read, and was refused: ADM-144"),
        format!("AI client {READS} of alice listed the actions of services"),
        format!("AI client {READS} of alice read service.timers, and was refused: ADM-144"),
        format!("AI client {READS} of alice asked for service.restart, and was refused: ADM-144"),
        format!("AI client {OVER_SSH} of alice connected over SSH"),
        format!("AI client {OVER_SSH} of alice asked for service.restart (request {})", asked.done),
        format!("AI client {OVER_SSH} of alice looked at request {}", asked.done),
        format!(
            "alice, for AI client {OVER_SSH} of alice from {HOST}: service.restart ended with status 0"
        ),
        format!("AI client {OVER_SSH} of alice disconnected: its pass is not taken (ADM-143)"),
    ];
    wait_for("admind's log to have a line for each call", 30, || {
        let log = vm.log();
        Ok(match lines.iter().find(|line| !log.contains(&format!("admind: {line}"))) {
            None => Ok(()),
            Some(line) => Err(format!("it has no line {line:?}")),
        })
    })?;

    // The audit log, which logd gives a while after: authd's lines of who
    // was let in and what was given, and admind's of who answered.
    let wanted = [
        format!("alice let in the AI client \"{OVER_SSH}\", over ssh, for *, until "),
        format!("alice let in the AI client \"{OVER_HTTPS}\", over https, for services, until "),
        format!("gave alice a pass for the AI client \"{OVER_SSH}\""),
        format!(
            "the AI client \"{OVER_HTTPS}\" of alice's reads service.timers as an administrator"
        ),
        format!("the AI client \"{OVER_SSH}\" of alice's reads service.timers as an administrator"),
        format!("a wrong AI client token, for alice, from {HOST}"),
        format!(
            "alice approved request {} of the AI client \"{OVER_SSH}\" of alice: service.restart",
            asked.done
        ),
        format!(
            "alice refused request {} of the AI client \"{OVER_SSH}\" of alice: service.stop",
            asked.refused
        ),
        format!("removed the AI client \"{OVER_SSH}\" of alice, for process "),
        format!("alice removed the AI client \"{OVER_HTTPS}\""),
        format!("removed the AI client \"{READS}\" of alice, for process "),
    ];
    elevated(vm)?;
    let audit = wait_for("the audit log to have each line about the clients", 90, || {
        let audit = test.pane.output(vm, "elevate log --audit | grep -e 'AI client'")?;
        Ok(match wanted.iter().find(|line| !audit.contains(line.as_str())) {
            None => Ok(audit),
            Some(line) => Err(format!("no line with {line:?} in {:?}", tail(&audit, 1500))),
        })
    })?;

    // A token is shown once, to the person who made the client.
    let log = vm.log();
    for token in [&tokens.https, &tokens.reads] {
        let secret = secret_of(token);
        if log.contains(secret) || audit.contains(secret) {
            return Err("a client's token is in the machine's log or in the audit log".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_names_its_tool_and_a_result_answers_its_request() {
        let call = calls(7, "read", &json!({"action": "service.list"}));
        assert_eq!(call["method"], "tools/call");
        assert_eq!(
            call["params"],
            json!({"name": "read", "arguments": {"action": "service.list"}})
        );
        let answer = json!({"jsonrpc": "2.0", "id": 7, "result": {"isError": false}});
        assert_eq!(result_of(&answer, 7, "it"), Ok(json!({"isError": false})));
        assert!(result_of(&answer, 8, "it").is_err());
        let refused = json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32603}});
        assert!(result_of(&refused, 7, "it").is_err());
    }

    #[test]
    fn a_tool_s_result_is_its_data_or_the_failure_it_names() {
        let read = json!({"isError": false, "structuredContent": {"answer": []}});
        assert_eq!(told(&read, "it"), Ok(json!({"answer": []})));
        assert!(fails(&read, "it", "ADM-144").is_err());
        let failed = json!({"isError": true, "structuredContent": {"error": {"id": "ADM-144"}}});
        assert!(told(&failed, "it").is_err());
        assert_eq!(fails(&failed, "it", "ADM-144"), Ok(()));
        assert!(fails(&failed, "it", "ADM-143").is_err());
    }

    #[test]
    fn a_token_is_the_line_that_begins_as_one_and_its_secret_the_last_part() {
        let printed = "\n  dkc_alice_0a1b\n";
        assert_eq!(token_in(printed).as_deref(), Some("dkc_alice_0a1b"));
        assert_eq!(token_in("dkv_alice_0a1b\nThe AI client is let in."), None);
        assert_eq!(secret_of("dkc_alice_0a1b"), "0a1b");
        // An account's name may hold the mark that parts the token.
        assert_eq!(secret_of("dkc_lab_admin_0a1b"), "0a1b");
    }

    #[test]
    fn a_table_s_line_is_found_by_its_first_word_and_a_list_s_entry_by_a_value() {
        let table = "AREA      REACH\nservices  read all and ask\nconsole   read all\n";
        assert_eq!(line_about(table, "console"), Some("console   read all"));
        assert_eq!(line_about(table, "read"), None);
        let list = json!([{"name": "lab-ssh", "way": "ssh"}, {"name": "lab-read", "way": "https"}]);
        assert_eq!(
            entry(&list, "name", "lab-read"),
            Some(&json!({"name": "lab-read", "way": "https"}))
        );
        assert_eq!(entry(&list, "name", "lab"), None);
        assert_eq!(each(&list, "way"), Ok(vec![json!("ssh"), json!("https")]));
        assert!(each(&json!({}), "way").is_err());
    }

    #[test]
    fn a_request_of_the_newest_revision_names_its_method_and_a_call_its_tool() {
        let names = |headers: &[(String, String)]| {
            headers.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>()
        };
        let plain = newest_headers("dkc_a_b", "server/discover", None);
        assert_eq!(
            names(&plain),
            ["Authorization", "Content-Type", "MCP-Protocol-Version", "Mcp-Method"]
        );
        assert_eq!(plain[0].1, "Bearer dkc_a_b");
        assert_eq!(plain[2].1, NEWEST);
        let call = newest_headers("dkc_a_b", "tools/call", Some("areas"));
        assert_eq!(call.last(), Some(&("Mcp-Name".to_string(), "areas".to_string())));
    }
}
