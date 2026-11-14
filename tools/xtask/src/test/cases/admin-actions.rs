// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Every action of the web console, Dukovo Admin, is called through its
//! route and run as its shown command, and the two are held to the same
//! (ADR 0514). The test takes the list from GET /api/1/actions, which must
//! be the list that admin actions --json prints in alice's pane, and from
//! it each action's good example, so that an action with a good example is
//! tested as soon as it is registered. What the pane's commands print
//! reaches the host through nc, to a port on the host's loopback that
//! QEMU's user network gives the guest at 10.0.2.2.
//!
//! Who may: a read answers 200 for bob, who is in group console and no
//! administrator, and for alice. A change is ADM-122 for bob and ADM-106
//! for alice while her session is locked. A name that is no name is ADM-125
//! with the field name, whoever asks, and says so before anyone is asked;
//! a name with a slash is no address at all, ADM-116. A token made with
//! admin token add, its password on standard input, shows in admin token
//! list. A token for services:read reads services, and is ADM-124 for
//! certificates and for a change in services; a token for every scope
//! changes; a token sent with a session's cookie is ADM-108; and once
//! admin token remove has taken a token away it is ADM-115.
//!
//! Same as the command: POST /api/1/show gives each action's command, and
//! admin show prints that line. A read's command, typed in alice's pane,
//! prints what the route answers. A change's command, typed in the pane
//! that one elevate unlocked with her password, leaves the example service
//! as the route does; the route's undo, which examples.undo names, puts
//! back what the change did, and the log has a line for each change asked
//! for and none for a read that worked. certificate.renew comes last: a
//! renewal makes the machine's certificate anew, so the console serves
//! another, the issue file and cert show name its fingerprint, and the
//! host, told the new one, is answered.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Reply, SESSION_COOKIE, elevated};
use super::super::*;

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "white-rabbit";

/// The host, as the guest's network sees it.
const HOST: &str = "10.0.2.2";

/// Where admind's log says the host's requests come from.
const PEER: &str = "10.0.2.2";

/// The test.
pub const CASE: Case = Case {
    name: "admin-actions",
    about: "Each action of the web console is called through its route and typed as its shown command, and both do the same: the list of GET /api/1/actions is the one admin actions --json prints; a read is 200 for bob and alice, a change ADM-122 for bob and ADM-106 for alice locked, a name that is none ADM-125 with its field; a token made with admin token add reads and changes within its scopes, is ADM-124 outside them, ADM-108 with a cookie and ADM-115 once removed; POST /api/1/show and admin show give the command that, typed in alice's elevated pane, prints what a read's route answers and leaves the example service as the route's change did; the undo of examples.undo puts a change back; admind's log has a line for each change and none for a read that worked; certificate.renew makes the machine's certificate anew, which the console then serves, with its fingerprint in the issue file (ADR 0514)",
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
    script: admin_actions,
    disks: &[],
    second: None,
};

fn admin_actions(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    vm.expect_anywhere("admind: answers at ")?;
    let mut test = Test {
        console: Console::open(vm)?,
        pane: Pane::open()?,
        alice: Browser::default(),
        bob: Browser::default(),
        actions: Vec::new(),
        commands: BTreeMap::new(),
        said: Vec::new(),
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
    vm.expect_screen(&["Made bob"])?;
    test.sign_in()?;

    the_list(vm, &mut test)?;
    who_may(&mut test)?;
    tokens(vm, &mut test)?;
    commands_are_shown(vm, &mut test)?;
    reads_as_commands(vm, &mut test)?;
    // The commands that change something need the pane unlocked again
    // and the console's session too.
    elevated(vm)?;
    service_changes(vm, &mut test)?;
    renewing(vm, &mut test)?;
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
    bob: Browser,
    /// The actions, as the console lists them.
    actions: Vec<Action>,
    /// What POST /api/1/show gave as each action's command.
    commands: BTreeMap<String, String>,
    /// The lines the log must have, each without its `admind: `.
    said: Vec<String>,
}

/// Who asks the console.
#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

impl Who {
    fn name(self) -> &'static str {
        match self {
            Who::Alice => "alice",
            Who::Bob => "bob",
        }
    }
}

impl Test {
    /// Signs alice and bob in, as the console's page does.
    fn sign_in(&mut self) -> Result<(), String> {
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        self.console.sign_in(&mut self.bob, "bob", BOB_PASSWORD)?;
        self.console.read_session(&mut self.bob)?;
        Ok(())
    }

    /// The action `id`.
    fn action(&self, id: &str) -> Result<Action, String> {
        self.actions
            .iter()
            .find(|action| action.id == id)
            .cloned()
            .ok_or_else(|| format!("the action list has no {id}"))
    }

    /// Calls `action`'s route with `args`, for `who`'s session, whatever
    /// the answer is.
    fn run(&mut self, who: Who, action: &Action, args: &Value) -> Result<Reply, String> {
        let (path, body) = address(&action.path, args)?;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        if action.changes {
            self.console.post(browser, &path, &Value::Object(body))
        } else if body.is_empty() {
            self.console.get(browser, &path)
        } else {
            Err(format!("{}'s example has arguments that its address does not take", action.id))
        }
    }

    /// Calls `action`'s route with `args` for the API token `token`, and
    /// the session cookie `cookie` too if there is one.
    fn run_with_token(
        &self,
        token: &str,
        cookie: Option<&str>,
        action: &Action,
        args: &Value,
    ) -> Result<Reply, String> {
        let (path, body) = address(&action.path, args)?;
        let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
        if let Some(cookie) = cookie {
            headers.push(("Cookie".into(), format!("{SESSION_COOKIE}={cookie}")));
        }
        if action.changes {
            headers.push(("Content-Type".into(), "application/json".into()));
            let body = Value::Object(body).to_string();
            self.console.send("POST", &path, &headers, Some(&body))
        } else {
            self.console.send("GET", &path, &headers, None)
        }
    }

    /// Keeps alice's session unlocked: at least two minutes are left, or
    /// she gives her password again.
    fn unlocked(&mut self) -> Result<(), String> {
        let session = self.console.get(&mut self.alice, "/api/1/session")?.must("her session", 200)?;
        if session.json()?["unlock_seconds"].as_u64().is_some_and(|left| left > 120) {
            return Ok(());
        }
        let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
        self.console.post(&mut self.alice, "/api/1/unlock", &unlock)?.must("her unlocking", 200)?;
        Ok(())
    }

    /// Calls the change `id` as alice, unlocked: it is done, and the log
    /// will say so.
    fn change(&mut self, id: &str, args: &Value) -> Result<(), String> {
        let action = self.action(id)?;
        self.unlocked()?;
        let reply = self.run(Who::Alice, &action, args)?;
        let reply = reply.must(&format!("{id} by the route, alice unlocked"), 200)?;
        if reply.json()?["done"] != json!(true) {
            return Err(format!("{id} was answered {}, which does not say it is done", reply.body));
        }
        self.said("alice", id, "ended with status 0");
        Ok(())
    }

    /// Notes a line admind's log must have: `who` asked for `id`, and it
    /// `ended`.
    fn said(&mut self, who: &str, id: &str, ended: &str) {
        self.said.push(format!("{who} from {PEER}: {id} {ended}"));
    }

    /// The service's state once it has settled, which is not a state it
    /// passes through, and is running when `running` is Some(true) and not
    /// when it is Some(false). It is read by the route, as the page does.
    fn settled(&mut self, name: &str, running: Option<bool>) -> Result<Seen, String> {
        let list = self.action("service.list")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let reply = self.run(Who::Alice, &list, &list.args)?.must("the list of services", 200)?;
            let now = seen(&reply.json()?, name)?;
            let passing = matches!(now.state.as_str(), "starting" | "stopping" | "pending" | "waiting");
            if !passing && running.is_none_or(|want| now.running() == want) {
                return Ok(now);
            }
            if Instant::now() >= deadline {
                let wanted = match running {
                    Some(true) => " running",
                    Some(false) => " not running",
                    None => " settled",
                };
                return Err(format!("{name} is {now:?} 30 s after the change, not{wanted}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// An action, as the list gives it, and the arguments of its first good
/// example.
#[derive(Clone, Debug)]
struct Action {
    id: String,
    area: String,
    changes: bool,
    /// The route's address, with `{name}` parts.
    path: String,
    args: Value,
    /// The action and arguments that put back what `args` did.
    undo: Option<(String, Value)>,
}

/// The actions of the list `list` as GET /api/1/actions answers.
fn actions_of(list: &Value) -> Result<Vec<Action>, String> {
    let rows = list.as_array().filter(|rows| !rows.is_empty()).ok_or("the action list is empty")?;
    let mut actions: Vec<Action> = Vec::new();
    for row in rows {
        let id = row["id"].as_str().ok_or("an action in the list has no id")?.to_string();
        let text = |key: &str| {
            row[key].as_str().map(str::to_string).ok_or_else(|| format!("{id} has no {key}"))
        };
        let args = row["examples"]["good"]
            .get(0)
            .cloned()
            .ok_or_else(|| format!("{id} has no good example in examples.good"))?;
        let undo = match &row["examples"]["undo"] {
            Value::Null => None,
            undo => {
                let action = undo["action"].as_str().ok_or_else(|| {
                    format!("{id}'s examples.undo does not name an action: {undo}")
                })?;
                Some((action.to_string(), undo["args"].clone()))
            }
        };
        let changes = row["changes"].as_bool().ok_or_else(|| format!("{id} has no changes"))?;
        if actions.iter().any(|have| have.id == id) {
            return Err(format!("{id} is in the list twice"));
        }
        actions.push(Action { area: text("area")?, path: text("path")?, changes, args, undo, id });
    }
    for action in &actions {
        if let Some((undo, _)) = &action.undo
            && !actions.iter().any(|have| &have.id == undo && have.changes)
        {
            return Err(format!("{}'s undo, {undo}, is not a change in the list", action.id));
        }
    }
    Ok(actions)
}

/// The address of `template` with each `{name}` part filled in from `args`
/// and percent-encoded, and the arguments the address does not hold, which
/// a body carries.
fn address(template: &str, args: &Value) -> Result<(String, Map<String, Value>), String> {
    let mut rest = args.as_object().cloned().unwrap_or_default();
    let mut parts = Vec::new();
    for part in template.split('/') {
        match part.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')) {
            Some(name) => match rest.remove(name) {
                Some(Value::String(text)) => parts.push(percent_encode(&text)),
                Some(other) => parts.push(percent_encode(&other.to_string())),
                None => return Err(format!("the arguments {args} have no {name} for {template}")),
            },
            None => parts.push(part.to_string()),
        }
    }
    Ok((parts.join("/"), rest))
}

/// `text` with every byte outside `A-Za-z0-9-._~` written as `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `args` as the words `admin show` takes after the action: `name=value`
/// for a string, `name:=JSON` for any other value, quoted for sh.
fn show_words(args: &Value) -> Vec<String> {
    let Some(object) = args.as_object() else { return Vec::new() };
    object
        .iter()
        .map(|(name, value)| match value {
            Value::String(text) => quoted(&format!("{name}={text}")),
            other => quoted(&format!("{name}:={other}")),
        })
        .collect()
}

/// `word` as one word of a line for sh: as it is when it holds nothing but
/// `A-Za-z0-9@%+=:,./_-`, in single quotes otherwise.
fn quoted(word: &str) -> String {
    let plain = !word.is_empty()
        && word.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"@%+=:,./_-".contains(&byte));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

/// What the guest's pane says to the host: nc, at the end of a typed line,
/// sends what the line's command printed to a port on the host's loopback,
/// where QEMU's user network lets the guest reach it at 10.0.2.2. The
/// screen would have to hold it, and cannot hold a list of services.
struct Pane {
    listener: TcpListener,
    port: u16,
}

impl Pane {
    fn open() -> Result<Pane, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|err| format!("no port for what the pane says: {err}"))?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        listener.set_nonblocking(true).map_err(|err| err.to_string())?;
        Ok(Pane { listener, port })
    }

    /// What follows a pipe to send the output to the host: nc half-closes
    /// the connection at the end of its input, and ends once the host has
    /// closed its side.
    fn sink(&self) -> String {
        format!("nc -N {HOST} {}", self.port)
    }

    /// Takes what the guest sends over one connection, waiting up to
    /// `seconds` for the connection and up to 30 for its end.
    fn take(&self, seconds: u64) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).map_err(|err| err.to_string())?;
                    stream
                        .set_read_timeout(Some(Duration::from_secs(30)))
                        .map_err(|err| err.to_string())?;
                    let mut bytes = Vec::new();
                    stream
                        .read_to_end(&mut bytes)
                        .map_err(|err| format!("what the pane sent: {err}"))?;
                    return Ok(String::from_utf8_lossy(&bytes).into_owned());
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(format!("the pane sent nothing to port {} in {seconds} s", self.port));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(format!("taking what the pane sends: {err}")),
            }
        }
    }

    /// Types `command` in the pane, and returns what it printed.
    fn output(&self, vm: &mut Vm, command: &str) -> Result<String, String> {
        answer_prompt(vm, "$", &format!("{command} | {}", self.sink()))?;
        self.take(120)
    }

    /// Types `command` in the pane, and returns the status it ended with.
    fn status(&self, vm: &mut Vm, command: &str) -> Result<i32, String> {
        answer_prompt(vm, "$", &format!("{command}; echo \"ran $?\" | {}", self.sink()))?;
        let said = self.take(120)?;
        ran_status(&said).ok_or_else(|| format!("`{command}` was followed by {said:?}"))
    }
}

/// The status in `ran N`, as `Pane::status` has the pane say it.
fn ran_status(text: &str) -> Option<i32> {
    text.lines().find_map(|line| line.trim().strip_prefix("ran ")?.trim().parse().ok())
}

/// The first part of what the shown form of an action prints: the line
/// under `Command`.
fn command_of_show(text: &str) -> Option<&str> {
    let mut lines = text.lines();
    lines.find(|line| line.trim() == "Command")?;
    lines.next().map(str::trim)
}

/// What the test sees of a service in Spark's document.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    state: String,
    enabled: bool,
    pid: Option<u64>,
}

impl Seen {
    fn running(&self) -> bool {
        self.state == "running"
    }

    /// Whether `other` is the same state and the same setting for the next
    /// boot: its process is another one after a restart or a start.
    fn as_before(&self, other: &Seen) -> bool {
        self.state == other.state && self.enabled == other.enabled
    }
}

/// The service `name` in the document of `service list --json`.
fn seen(document: &Value, name: &str) -> Result<Seen, String> {
    let service = document["services"]
        .as_array()
        .and_then(|services| services.iter().find(|service| service["name"] == name))
        .ok_or_else(|| format!("the list of services has no {name}"))?;
    Ok(Seen {
        state: service["state"].as_str().ok_or("a service has no state")?.to_string(),
        enabled: service["enabled"].as_bool().ok_or("a service has no enabled")?,
        pid: service["pid"].as_u64(),
    })
}

/// What a read's answer holds that does not change from one moment to the
/// next: for the services, each one's name and state (their times and
/// process numbers do), for an explanation, what it follows and says to
/// do; anything else whole.
fn projection(id: &str, answer: &Value) -> Value {
    match id {
        "service.list" => {
            let states: Map<String, Value> = answer["services"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|service| {
                    (service["name"].as_str().unwrap_or_default().to_string(), service["state"].clone())
                })
                .collect();
            Value::Object(states)
        }
        "service.explain" => {
            json!({"chain": answer["chain"], "root": answer["root"], "fix": answer["fix"]})
        }
        _ => answer.clone(),
    }
}

/// The token in what admin token add printed: the line that begins with
/// the prefix every token has.
fn token_in(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| line.starts_with("dkv_")).map(str::to_string)
}

/// The line of admin token list that is about the token `name`.
fn token_line<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find(|line| line.split_whitespace().next() == Some(name))
}

/// Problems found, kept until a section has looked at everything it was to,
/// so that one run says all that is wrong with the actions.
#[derive(Default)]
struct Problems(Vec<String>);

impl Problems {
    fn note(&mut self, what: &str, result: Result<(), String>) {
        if let Err(why) = result {
            self.0.push(format!("{what}: {why}"));
        }
    }

    fn end(self, section: &str) -> Result<(), String> {
        if self.0.is_empty() { Ok(()) } else { Err(format!("{section}: {}", self.0.join("; "))) }
    }
}

/// The list the route answers is the list the command prints, and every
/// action has a good example.
fn the_list(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let route = test.console.get(&mut test.alice, "/api/1/actions")?.must("the action list", 200)?;
    let route = route.json()?;
    let typed = test.pane.output(vm, "admin actions --json")?;
    let typed: Value = serde_json::from_str(typed.trim())
        .map_err(|err| format!("admin actions --json printed {typed:?}, which is not JSON: {err}"))?;
    if route != typed {
        let differs = route
            .as_array()
            .into_iter()
            .flatten()
            .zip(typed.as_array().into_iter().flatten())
            .find(|(one, other)| one != other)
            .map_or_else(
                || "the lists are of different lengths".to_string(),
                |(one, _)| format!("they differ at {}", one["id"]),
            );
        return Err(format!("GET /api/1/actions is not what admin actions --json prints: {differs}"));
    }
    test.actions = actions_of(&route)?;
    Ok(())
}

/// Who may: reads for bob and alice, changes refused for bob and for alice
/// locked, a name that is none refused before anyone is asked.
fn who_may(test: &mut Test) -> Result<(), String> {
    // Signing in leaves a session locked, but the test says so.
    test.console.post(&mut test.alice, "/api/1/lock", &json!({}))?.must("lock", 200)?;
    let mut problems = Problems::default();
    for action in test.actions.clone() {
        let who = who_may_ask(test, &action);
        problems.note(&format!("{} for bob and alice", action.id), who);
        if action.args.get("name").is_some() {
            let names = bad_names(test, &action);
            problems.note(&format!("{} with a bad name", action.id), names);
        }
    }
    problems.end("who may")
}

/// One action, asked by bob and by alice, locked.
fn who_may_ask(test: &mut Test, action: &Action) -> Result<(), String> {
    let id = &action.id;
    if !action.changes {
        for who in [Who::Bob, Who::Alice] {
            let reply = test.run(who, action, &action.args)?;
            reply.must(&format!("{id} read by {}", who.name()), 200)?.json()?;
        }
        return Ok(());
    }
    let reply = test.run(Who::Bob, action, &action.args)?;
    reply.refused(&format!("{id} asked by bob"), 403, "ADM-122")?;
    test.said("bob", id, "was not run: ADM-122");
    let reply = test.run(Who::Alice, action, &action.args)?;
    reply.refused(&format!("{id} asked by alice, locked"), 403, "ADM-106")?;
    test.said("alice", id, "was not run: ADM-106");
    Ok(())
}

/// A name with a space is refused, with its field, and a name with a
/// slash is no address of the route.
fn bad_names(test: &mut Test, action: &Action) -> Result<(), String> {
    let id = &action.id;
    let mut args = action.args.clone();
    args["name"] = json!("a b");
    let refused = test.run(Who::Alice, action, &args)?.refused(&format!("{id} for `a b`"), 400, "ADM-125")?;
    let field = refused.json()?["error"]["field"].as_str().map(str::to_string);
    if field.as_deref() != Some("name") {
        return Err(format!("the refusal of `a b` names the field {field:?}, not name"));
    }
    args["name"] = json!("../x");
    test.run(Who::Alice, action, &args)?.refused(&format!("{id} for `../x`"), 404, "ADM-116")?;
    // The console's own services are not stopped from the console: it
    // would not answer. Alice is locked, so that nothing would be done
    // even if the name were taken.
    if matches!(id.as_str(), "service.stop" | "service.restart" | "service.disable") {
        args["name"] = json!("admind");
        let reply = test.run(Who::Alice, action, &args)?;
        let reply = reply.refused(&format!("{id} for admind"), 400, "ADM-125")?;
        if reply.json()?["error"]["field"] != json!("name") {
            return Err("the refusal of admind does not name the field name".into());
        }
    }
    Ok(())
}

/// Tokens: made in alice's pane with her password on standard input, listed,
/// held to their scopes, refused with a cookie, and refused once removed.
fn tokens(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let add = |name: &str, scope: &str| {
        format!("echo '{ALICE_PASSWORD}' | admin token add {name} --scope {scope}")
    };
    let read_token = test.pane.output(vm, &add("ci-read", "services:read"))?;
    let read_token = token_in(&read_token).ok_or("admin token add ci-read printed no token")?;
    let all_token = test.pane.output(vm, &add("ci-all", "'*'"))?;
    let all_token = token_in(&all_token).ok_or("admin token add ci-all printed no token")?;
    let listed = test.pane.output(vm, "admin token list")?;
    let line = |name: &str, scope: &str| match token_line(&listed, name) {
        Some(line) if line.contains(scope) => Ok(()),
        _ => Err(format!("admin token list has no line for {name} with {scope}: {listed:?}")),
    };
    line("ci-read", "services:read")?;
    line("ci-all", "*")?;

    // The first read and the first change of the services, and a read of
    // another area.
    let actions = test.actions.clone();
    let read = actions.iter().find(|action| action.area == "services" && !action.changes);
    let change = actions.iter().find(|action| action.area == "services" && action.changes);
    let other = actions.iter().find(|action| action.area != "services" && !action.changes);
    let (Some(read), Some(change), Some(other)) = (read, change, other) else {
        return Err("the list has no read and no change in services, or no read in another area".into());
    };
    let mut problems = Problems::default();
    let outcome = |result: Result<Reply, String>, what: &str, status: u16, id: Option<&str>| match id {
        Some(id) => result.and_then(|reply| reply.refused(what, status, id)).map(|_| ()),
        None => result.and_then(|reply| reply.must(what, status)).map(|_| ()),
    };

    let result = test.run_with_token(&read_token, None, read, &read.args);
    problems.note("ci-read reads the services", outcome(result, "ci-read reading services", 200, None));
    let result = test.run_with_token(&read_token, None, other, &other.args);
    problems.note("ci-read reads another area", outcome(result, "ci-read reading certificates", 403, Some("ADM-124")));
    test.said("alice (API token ci-read)", &other.id, "was not run: ADM-124");
    let result = test.run_with_token(&read_token, None, change, &change.args);
    problems.note("ci-read changes the services", outcome(result, "ci-read changing services", 403, Some("ADM-124")));
    test.said("alice (API token ci-read)", &change.id, "was not run: ADM-124");

    // What a token that reads may be shown: its own area's actions.
    let showing = |test: &Test, token: &str, action: &Action| {
        let body = json!({"action": action.id, "args": action.args}).to_string();
        let headers = [
            ("Authorization".to_string(), format!("Bearer {token}")),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        test.console.send("POST", "/api/1/show", &headers, Some(&body))
    };
    let result = showing(test, &read_token, read);
    problems.note("ci-read shows a read of its area", outcome(result, "ci-read showing a read", 200, None));
    let result = showing(test, &read_token, other);
    problems.note("ci-read shows an action of another area", outcome(result, "ci-read showing another area", 403, Some("ADM-124")));

    // A token for every scope changes, and reads any area.
    let result = test.run_with_token(&all_token, None, other, &other.args);
    problems.note("ci-all reads another area", outcome(result, "ci-all reading certificates", 200, None));
    let result = test.run_with_token(&all_token, None, change, &change.args);
    problems.note("ci-all changes the services", outcome(result, "ci-all changing services", 200, None));
    test.said("alice (API token ci-all)", &change.id, "ended with status 0");

    // A token is for a caller with no session: a cookie beside it is not
    // what the route takes.
    let cookie = test.alice.cookie.clone().ok_or("alice has no cookie")?;
    let result = test.run_with_token(&all_token, Some(&cookie), read, &read.args);
    problems.note("a token with a cookie", outcome(result, "a token and a cookie", 400, Some("ADM-108")));

    let removed = test.pane.status(vm, "admin token remove ci-read")?;
    if removed != 0 {
        return Err(format!("admin token remove ci-read ended with status {removed}"));
    }
    let result = test.run_with_token(&read_token, None, read, &read.args);
    problems.note("a token that was removed", outcome(result, "ci-read once removed", 401, Some("ADM-115")));
    problems.end("tokens")
}

/// POST /api/1/show gives each action's command, and admin show prints it.
fn commands_are_shown(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let mut problems = Problems::default();
    for action in test.actions.clone() {
        let id = &action.id;
        let body = json!({"action": id, "args": action.args});
        let result = test.console.post(&mut test.alice, "/api/1/show", &body);
        let command = match result.and_then(|reply| reply.must(&format!("showing {id}"), 200)?.json()) {
            Ok(shown) => shown["command"].as_str().unwrap_or_default().to_string(),
            Err(why) => {
                problems.note(&format!("{id} shown by the route"), Err(why));
                continue;
            }
        };
        if command.is_empty() {
            problems.note(&format!("{id} shown by the route"), Err("it gives no command".into()));
            continue;
        }
        let mut line = format!("admin show {id}");
        for word in show_words(&action.args) {
            line.push(' ');
            line.push_str(&word);
        }
        let typed = test.pane.output(vm, &line)?;
        let printed = command_of_show(&typed);
        if printed != Some(command.as_str()) {
            problems.note(
                &format!("{id} shown by admin show"),
                Err(format!("it prints {printed:?} under Command, and the route gives {command:?}")),
            );
        }
        test.commands.insert(id.clone(), command);
    }
    problems.end("the shown commands")
}

/// A read's shown command, typed in alice's pane, prints what the route
/// answers.
fn reads_as_commands(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let mut problems = Problems::default();
    for action in test.actions.clone().into_iter().filter(|action| !action.changes) {
        let Some(command) = test.commands.get(&action.id).cloned() else { continue };
        let result = read_as_command(vm, test, &action, &command);
        problems.note(&format!("{}: {command}", action.id), result);
    }
    problems.end("reads as commands")
}

fn read_as_command(vm: &mut Vm, test: &mut Test, action: &Action, command: &str) -> Result<(), String> {
    let answer = test.run(Who::Alice, action, &action.args)?.must("the route's read", 200)?.json()?;
    let printed = test.pane.output(vm, command)?;
    let printed: Value = serde_json::from_str(printed.trim())
        .map_err(|err| format!("it printed {printed:?}, which is not JSON: {err}"))?;
    let (route, typed) = (projection(&action.id, &answer), projection(&action.id, &printed));
    if route != typed {
        return Err(format!("it prints {typed}, and the route answers {route}"));
    }
    Ok(())
}

/// The changes of the services: the route, its undo, and the command.
fn service_changes(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let changes: Vec<Action> = test.actions.iter().filter(|a| a.changes && a.area == "services").cloned().collect();
    let Some(first) = changes.first() else { return Err("the list has no change in services".into()) };
    let name = first.args["name"].as_str().ok_or("a change in services has no name")?.to_string();
    let original = test.settled(&name, None)?;
    if !original.running() {
        return Err(format!("{name} is {original:?} before any change, not running"));
    }
    let mut problems = Problems::default();
    for action in &changes {
        let Some(command) = test.commands.get(&action.id).cloned() else { continue };
        let result = service_change(vm, test, action, &command, &name);
        problems.note(&format!("{}: {command}", action.id), result);
    }
    problems.end("changes in services")?;

    // Put back as it was, as the routes that start and enable it do, and
    // the service says so itself.
    test.change("service.start", &json!({"name": name}))?;
    test.change("service.enable", &json!({"name": name}))?;
    let now = test.settled(&name, Some(true))?;
    if !now.as_before(&original) {
        return Err(format!("{name} was {original:?} and is {now:?} after the changes put back"));
    }
    let said = test.pane.output(vm, &format!("elevate service {name} status"))?;
    if !said.contains("active (running)") {
        return Err(format!("service {name} status says {said:?}, not active (running)"));
    }
    Ok(())
}

/// One change of the services. A change that has an undo is tested from
/// the state the undo leaves: the undo, the change, the undo again, which
/// puts the service back as the first left it, and the command, which
/// leaves it as the change did.
fn service_change(
    vm: &mut Vm,
    test: &mut Test,
    action: &Action,
    command: &str,
    name: &str,
) -> Result<(), String> {
    let id = action.id.as_str();
    let Some((undo, undo_args)) = action.undo.clone() else {
        return service_restart(vm, test, action, command, name);
    };
    let runs = match id {
        "service.start" | "service.enable" => true,
        "service.stop" | "service.disable" => false,
        _ => return Err(format!("the test does not know what {id} leaves the service as")),
    };
    test.change(&undo, &undo_args)?;
    let before = test.settled(name, Some(!runs))?;
    test.change(id, &action.args)?;
    let after = test.settled(name, Some(runs))?;
    test.change(&undo, &undo_args)?;
    let again = test.settled(name, Some(!runs))?;
    if !again.as_before(&before) {
        return Err(format!("{undo} left {again:?}, and it was {before:?} before {id}"));
    }
    let status = test.pane.status(vm, command)?;
    if status != 0 {
        return Err(format!("the command ended with status {status}"));
    }
    let typed = test.settled(name, Some(runs))?;
    if !typed.as_before(&after) {
        return Err(format!("the command left {typed:?}, and the route left {after:?}"));
    }
    Ok(())
}

/// A restart has nothing that puts it back: from a running service, the
/// route and then the command each leave it running as another process.
fn service_restart(
    vm: &mut Vm,
    test: &mut Test,
    action: &Action,
    command: &str,
    name: &str,
) -> Result<(), String> {
    test.change("service.start", &json!({"name": name}))?;
    let before = test.settled(name, Some(true))?;
    test.change(&action.id, &action.args)?;
    let after = test.settled(name, Some(true))?;
    if after.pid == before.pid || !after.as_before(&before) {
        return Err(format!("the route left {after:?}, and it was {before:?} before"));
    }
    let status = test.pane.status(vm, command)?;
    if status != 0 {
        return Err(format!("the command ended with status {status}"));
    }
    let typed = test.settled(name, Some(true))?;
    if typed.pid == after.pid || !typed.as_before(&before) {
        return Err(format!("the command left {typed:?}, and the route left {after:?}"));
    }
    Ok(())
}

/// A renewal makes the machine's certificate anew, which the console
/// serves from then on: the route's, and the command's after it. The issue
/// file and cert show say the new fingerprint, and the host, told it, is
/// answered.
fn renewing(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    let renew = test.action("certificate.renew")?;
    let command = test
        .commands
        .get(&renew.id)
        .cloned()
        .ok_or("the command of certificate.renew was not shown")?;
    let name = renew.args["name"].as_str().ok_or("certificate.renew has no name")?.to_string();

    let old = test.console.fingerprint.clone();
    test.change(&renew.id, &renew.args)?;
    let new = served_anew(test, &old)?;
    agree(vm, test, &name, &new, &old)?;

    let status = test.pane.status(vm, &command)?;
    if status != 0 {
        return Err(format!("the command `{command}` ended with status {status}"));
    }
    let newer = served_anew(test, &new)?;
    agree(vm, test, &name, &newer, &new)
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
            return Err(format!("the console still serves the certificate {old} a minute after the renewal"));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The issue file and cert show name `new` and not `old`, and the console,
/// asked with the certificate the host now trusts, says it is `new`.
fn agree(vm: &mut Vm, test: &mut Test, name: &str, new: &str, old: &str) -> Result<(), String> {
    let issue = test.pane.output(vm, "cat /system/live/admind/issue")?;
    let shown = test.pane.output(vm, &format!("cert show {name}"))?;
    for (what, text) in [("the issue file", &issue), (&format!("cert show {name}"), &shown)] {
        if !text.contains(new) || text.contains(old) {
            return Err(format!("{what} does not name the new fingerprint {new} alone: {text:?}"));
        }
    }
    let said = test.console.send("GET", "/api/1/fingerprint", &[], None)?.must("the fingerprint", 200)?;
    if said.json()?["fingerprint"].as_str() != Some(new) {
        return Err(format!("the console gives {}, not the fingerprint {new}", said.body));
    }
    test.console.get(&mut test.alice, "/api/1/session")?.must("alice's session after the renewal", 200)?;
    Ok(())
}

/// The log has a line for each change asked for, however it ended, and for
/// no read that worked, and none for arguments that do not read.
fn the_log(vm: &mut Vm, test: &Test) -> Result<(), String> {
    let mut wanted: BTreeMap<String, usize> = BTreeMap::new();
    for line in &test.said {
        *wanted.entry(format!("admind: {line}")).or_default() += 1;
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while wanted.iter().any(|(line, &times)| vm.log().matches(line.as_str()).count() < times)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(500));
    }
    let log = vm.log();
    let mut problems = Problems::default();
    for (line, times) in &wanted {
        let have = log.matches(line.as_str()).count();
        if have != *times {
            problems.note(line, Err(format!("the log has it {have} times, not {times}")));
        }
    }
    for action in test.actions.iter().filter(|action| !action.changes) {
        let line = format!(": {} ended with status 0", action.id);
        if log.lines().any(|have| have.contains("admind: ") && have.contains(&line)) {
            problems.note(&action.id, Err("the log has a line for a read that worked".into()));
        }
    }
    if log.lines().any(|line| line.contains("admind: ") && line.contains("ADM-125")) {
        problems.note("arguments that do not read", Err("the log has a line for them".into()));
    }
    problems.end("the log")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Value {
        json!([
            {"id": "service.list", "area": "services", "changes": false, "path": "/api/1/services",
             "examples": {"good": [{}], "undo": null}},
            {"id": "service.start", "area": "services", "changes": true,
             "path": "/api/1/services/{name}/start",
             "examples": {"good": [{"name": "diskhealth"}],
                          "undo": {"action": "service.stop", "args": {"name": "diskhealth"}}}},
            {"id": "service.stop", "area": "services", "changes": true,
             "path": "/api/1/services/{name}/stop",
             "examples": {"good": [{"name": "diskhealth"}], "undo": null}},
        ])
    }

    #[test]
    fn an_address_has_each_name_part_filled_in_and_percent_encoded() {
        let (path, rest) = address("/api/1/services/{name}/start", &json!({"name": "a b"})).unwrap();
        assert_eq!(path, "/api/1/services/a%20b/start");
        assert!(rest.is_empty());
        let (path, _) = address("/api/1/services/{name}/start", &json!({"name": "../x"})).unwrap();
        assert_eq!(path, "/api/1/services/..%2Fx/start");
        let (path, rest) = address("/api/1/services", &json!({"force": true})).unwrap();
        assert_eq!(path, "/api/1/services");
        assert_eq!(rest.get("force"), Some(&json!(true)));
        assert!(address("/api/1/x/{name}", &json!({})).is_err());
        let (path, _) = address("/api/1/x/{port}", &json!({"port": 8080})).unwrap();
        assert_eq!(path, "/api/1/x/8080");
    }

    #[test]
    fn the_words_admin_show_takes_are_name_equals_value_or_name_colon_equals_json() {
        assert_eq!(show_words(&json!({"name": "diskhealth"})), ["name=diskhealth"]);
        assert_eq!(show_words(&json!({"wait": 5, "name": "a b"})), ["'name=a b'", "wait:=5"]);
        assert_eq!(show_words(&json!({})), Vec::<String>::new());
        assert_eq!(quoted("it's"), "'it'\\''s'");
    }

    #[test]
    fn the_list_gives_each_action_its_first_example_and_what_puts_it_back() {
        let actions = actions_of(&list()).unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[1].args, json!({"name": "diskhealth"}));
        let (undo, args) = actions[1].undo.clone().unwrap();
        assert_eq!((undo.as_str(), args), ("service.stop", json!({"name": "diskhealth"})));
        assert!(actions[0].undo.is_none() && !actions[0].changes && actions[2].changes);
    }

    #[test]
    fn a_list_with_no_example_or_an_undo_that_is_no_change_is_refused() {
        let mut no_example = list();
        no_example[0]["examples"]["good"] = json!([]);
        assert!(actions_of(&no_example).unwrap_err().contains("service.list has no good example"));
        let mut not_a_change = list();
        not_a_change[1]["examples"]["undo"]["action"] = json!("service.list");
        assert!(actions_of(&not_a_change).unwrap_err().contains("is not a change in the list"));
        let mut missing = list();
        missing[1]["examples"]["undo"]["action"] = json!("service.none");
        assert!(actions_of(&missing).is_err());
        let mut twice = list();
        twice[2]["id"] = json!("service.start");
        assert!(actions_of(&twice).unwrap_err().contains("twice"));
        assert!(actions_of(&json!([])).is_err());
    }

    #[test]
    fn what_the_pane_says_is_read_back() {
        assert_eq!(ran_status("ran 0\n"), Some(0));
        assert_eq!(ran_status("ran 3"), Some(3));
        assert_eq!(ran_status("noise\nran 1\n"), Some(1));
        assert_eq!(ran_status(""), None);
        let shown = "Command\nelevate service diskhealth start\n\nAPI\ncurl -sS x\n";
        assert_eq!(command_of_show(shown), Some("elevate service diskhealth start"));
        assert_eq!(command_of_show("API\nx\n"), None);
        let token = "dkv_abc123\n";
        assert_eq!(token_in(token), Some("dkv_abc123".into()));
        assert_eq!(token_in("no token here\n"), None);
        let table = "NAME     SCOPES         ENDS\nci-read  services:read  2027-01-01\nci-all   *  2027-01-01\n";
        assert!(token_line(table, "ci-read").unwrap().contains("services:read"));
        assert!(token_line(table, "ci-all").unwrap().contains('*'));
        assert!(token_line(table, "ci").is_none());
    }

    #[test]
    fn a_service_is_read_from_sparks_document_and_compared_by_state_and_setting() {
        let document = json!({"services": [
            {"name": "logd", "state": "running", "enabled": true, "pid": 4},
            {"name": "diskhealth", "state": "stopped", "enabled": false, "pid": null},
        ]});
        let seen_one = seen(&document, "diskhealth").unwrap();
        assert_eq!(seen_one, Seen { state: "stopped".into(), enabled: false, pid: None });
        assert!(!seen_one.running());
        assert!(seen(&document, "logd").unwrap().running());
        assert!(seen(&document, "none").is_err());
        let other = Seen { pid: Some(9), ..seen_one.clone() };
        assert!(other.as_before(&seen_one));
        assert!(!Seen { enabled: true, ..seen_one.clone() }.as_before(&seen_one));
    }

    #[test]
    fn a_read_is_compared_by_what_does_not_change_from_one_moment_to_the_next() {
        let at = |pid: u64, ms: u64| {
            json!({"now_ms": ms, "services": [
                {"name": "logd", "state": "running", "pid": pid, "since_ms": ms},
            ]})
        };
        assert_eq!(projection("service.list", &at(4, 1)), projection("service.list", &at(5, 9)));
        assert_eq!(projection("service.list", &at(4, 1)), json!({"logd": "running"}));
        let explained = |sentence: &str| {
            json!({"sentence": sentence, "chain": ["a"], "root": "a", "fix": null})
        };
        assert_eq!(
            projection("service.explain", &explained("a ran for 3 s.")),
            projection("service.explain", &explained("a ran for 9 s."))
        );
        assert_eq!(projection("certificate.list", &json!([1])), json!([1]));
    }

    #[test]
    fn problems_are_kept_and_told_together() {
        let mut problems = Problems::default();
        problems.note("one", Ok(()));
        assert!(Problems::default().end("x").is_ok());
        problems.note("two", Err("went wrong".into()));
        problems.note("three", Err("did too".into()));
        assert_eq!(problems.end("section").unwrap_err(), "section: two: went wrong; three: did too");
    }
}
