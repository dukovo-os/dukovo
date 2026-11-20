// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! service(8): the services Spark supervises, through its control socket
//! (spark(8)): `service list`, and a service's `status`, `explain`, `start`,
//! `stop`, `restart`, `reload` and `logs`. `check`, `new`, `edit`, `secret`,
//! `enable` and `disable`, which work on the services' files, are in
//! `servicefiles`. The command line is Debian's, with more.
//!
//! A start, stop or restart waits until the service has settled (ADR
//! 0471): the request asks Spark to watch, and Spark writes each of its
//! log lines about the services the request touches as it says them, and
//! a last line, `ok:` or `error:`, once the service is ready, has failed
//! a start, or has stopped. This prints the lines without `spark: `, so
//! that a start shows each step along the service graph, and exits 1 if
//! the service did not settle as asked.
//!
//! `status` reads Spark's `show` document for a service and adds what
//! the system says of it elsewhere: its processes, from `/processes`,
//! the ports it listens on now, and its last log lines from logd.
//! `explain` reads the document of every service and gives its sentence
//! from `spark_api::explain`, the words the web console says too (ADR
//! 0471). The `service-graph` boot test and the lab's `services/command`
//! and `services/explain` tests cover this file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use manifest::service::Stop;
use serde::Serialize;
use spark_api::explain;
use spark_api::show::{self, HealthState, Holds, State, Usage};
use spark_api::words::{and_list, ready_once, span, tenths};

use crate::args::{Arg, Args};
use crate::log;
use crate::power::{self, CONTROL};
use crate::proc::{self, Process};
use crate::util::{self, Fail, fail, fail_with, usage};
use crate::{servicefiles, servicetry, timerd};

const USAGE: &str = "Usage: service list [--failed] [--json] [--timers]
       service NAME status [-n LINES | --json]
       service NAME explain [--json]
       service NAME start | stop | restart [--no-wait] [-q]
       service NAME reload | --full-restart
       service NAME logs [log(1) options]
       service NAME enable | disable
       service check [--brief] [NAME | FILE]... | new NAME | edit NAME
       service try NAME [--for SECONDS] | keep NAME | undo NAME
       service secret set NAME SECRET | list NAME | remove NAME SECRET
       service --status-all";

/// The log lines `status` shows unless `-n` says otherwise.
const STATUS_LINES: usize = 5;

/// The most process numbers `status` lists.
const PROCESSES_SHOWN: usize = 5;

/// The most processes `status --json` gives.
const PROCESSES_JSON: usize = 50;

/// The most ports of one range `status --json` looks at for a socket that
/// listens: a service that names every port above 1024 is not asked
/// about each.
const PORTS_LOOKED_AT: u32 = 1024;

/// The most names `list` gives of the services not running as asked.
const NAMES_SHOWN: usize = 3;

/// The kernel's tables of TCP sockets (radix-proc(5)).
const TCP_TABLES: [&str; 2] = ["/processes/net/tcp", "/processes/net/tcp6"];

/// Whether SIGINT came while a start, stop or restart was waiting.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Whether SIGINT came since a start, stop or restart began to wait.
pub(crate) fn sigint_came() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// Sends `request`, and fails with what Spark said if it refused.
pub(crate) fn ask(request: &str) -> Result<String, Fail> {
    let answer = power::request(request)?;
    match answer.strip_prefix("error: ") {
        Some(why) => Err(fail(why.trim_end().to_string())),
        None => Ok(answer),
    }
}

/// Asks Spark `request`, which only reads, on its read socket (spark(8)),
/// which answers anyone, and fails with what Spark said if it refused.
pub(crate) fn read(request: &str) -> Result<String, Fail> {
    let answer = power::read(request)?;
    match answer.strip_prefix("error: ") {
        Some(why) => Err(fail(why.trim_end().to_string())),
        None => Ok(answer),
    }
}

/// Spark's `show` document, of every service or of `name` alone; a name
/// Spark does not know is LSB's unknown service, 4. Anyone may read it,
/// but only root sees what the services' output said: the last line a
/// service wrote, and the last a failed health check wrote.
pub(crate) fn shown(name: Option<&str>) -> Result<show::Show, Fail> {
    let request = name.map_or_else(|| String::from("show"), |name| format!("show {name}"));
    match read(&request) {
        Ok(answer) => show::parse(&answer).map_err(fail),
        Err(failed) if failed.message.starts_with("no service is called") => {
            Err(fail_with(4, format!("{}: unrecognized service", name.unwrap_or(""))))
        }
        Err(failed) => Err(failed),
    }
}

/// How long until the trial `id` of `document` is undone unless it is
/// kept: none when it is not being tried.
pub(crate) fn left_of(document: &show::Show, id: &str) -> Option<u64> {
    document.trials.iter().find(|trial| trial.id == id).map(|trial| trial.left_ms)
}

/// How long until the trial `id` is undone unless it is kept, as Spark
/// says now. Spark that does not answer says nothing: the callers show
/// what they can without it, as a trial does not change what a machine has.
pub(crate) fn trial_left_ms(id: &str) -> Option<u64> {
    left_of(&shown(None).ok()?, id)
}

/// service(8).
pub fn service(args: &mut Args) -> Result<i32, Fail> {
    args.stop_at_operand();
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "status-all" => return status_all(),
            Arg::Long(l) if l == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Operand(value) => operands.push(value.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    operands.extend(args.rest().into_iter().map(|a| a.to_string_lossy().into_owned()));
    // The files' own commands come first, as `service check logd`, and
    // `list` when nothing but options follows it.
    match operands.first().map(String::as_str) {
        Some("check") => return servicefiles::check(&operands[1..]),
        Some("new") => return servicefiles::new(&operands[1..]),
        Some("edit") => return servicefiles::edit(&operands[1..]),
        Some("try") => return servicetry::try_change(&operands[1..]),
        Some(verb @ ("keep" | "undo")) => return servicetry::end(verb, &operands[1..]),
        Some("secret") => return servicefiles::secret(&operands[1..]),
        Some("list") if operands.get(1).is_none_or(|o| o.starts_with('-')) => {
            return list(&operands[1..]);
        }
        _ => {}
    }
    let (Some(name), Some(command)) = (operands.first(), operands.get(1)) else {
        return Err(usage(USAGE.into()));
    };
    let options = &operands[2..];
    match command.as_str() {
        "enable" => servicefiles::enable(name, true),
        "disable" => servicefiles::enable(name, false),
        "start" | "stop" | "restart" => change(command, name, options),
        "--full-restart" => change("restart", name, options),
        "reload" => {
            let answer = ask(&format!("reload {name}"))?;
            println!("{}", said(&answer));
            Ok(0)
        }
        "status" => status(name, options),
        "explain" => explain(name, options),
        "logs" => {
            let mut items = vec!["-s".into(), name.into()];
            items.extend(options.iter().map(Into::into));
            log::main(&mut Args::new("log", items))
        }
        other => Err(usage(format!(
            "{name}: unknown command `{other}' (start, stop, restart, reload, status, explain, logs, \
             enable, disable)"
        ))),
    }
}

/// Spark's answer without its `ok: ` and line end.
pub(crate) fn said(answer: &str) -> &str {
    let answer = answer.trim_end();
    answer.strip_prefix("ok: ").unwrap_or(answer)
}

/// `service --status-all`, as Debian's: each service, `+` when it runs or
/// is done, `?` while it is on its way, and `-` otherwise.
fn status_all() -> Result<i32, Fail> {
    for service in shown(None)?.services {
        let mark = match service.state {
            State::Running | State::Done => '+',
            State::Pending | State::Starting | State::Waiting => '?',
            _ => '-',
        };
        println!(" [ {mark} ]  {}", service.name);
    }
    Ok(0)
}

/// `service NAME start|stop|restart [--no-wait] [-q]`: asks Spark, and
/// unless `--no-wait`, prints its lines until the service has settled.
pub(crate) fn change(verb: &str, name: &str, options: &[String]) -> Result<i32, Fail> {
    let (mut wait, mut quiet) = (true, false);
    for option in options {
        match option.as_str() {
            "--no-wait" | "--no-block" => wait = false,
            "-q" | "--quiet" => quiet = true,
            other => return Err(usage(format!("service {verb}: unknown option `{other}'"))),
        }
    }
    if !wait {
        let answer = ask(&format!("{verb} {name}"))?;
        if !quiet {
            println!("{}", said(&answer));
        }
        return Ok(0);
    }
    // Before the request goes, so that a SIGINT from then on ends the wait
    // and leaves the request going.
    // SAFETY: all zeros is a valid sigaction: numbers, a signal set and a
    // function address.
    let mut action: libc::sigaction = unsafe { util::zeroed() };
    action.sa_sigaction = on_interrupt as extern "C" fn(libc::c_int) as *const () as usize;
    // SAFETY: the handler only stores to an atomic; without SA_RESTART the
    // read that waits for Spark's next line ends when SIGINT comes.
    unsafe { libc::sigaction(libc::SIGINT, &raw const action, std::ptr::null_mut()) };
    let mut stream = UnixStream::connect(CONTROL).map_err(|e| util::failed(&CONTROL, e))?;
    stream.write_all(format!("{verb} {name} --wait\n").as_bytes())?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut lines = Lines { stream, held: Vec::new() };
    let first = match lines.next_line() {
        Ok(Some(line)) => line,
        Ok(None) => return Err(fail("Spark ended the connection without answering".into())),
        Err(e) => return Err(interrupted(verb, name, e)),
    };
    if let Some(why) = first.strip_prefix("error: ") {
        return Err(fail(why.to_string()));
    }
    let mut printed = false;
    loop {
        let line = match lines.next_line() {
            Ok(Some(line)) => line,
            // Answered without a watch: the first line is all there is.
            Ok(None) => break,
            Err(e) => return Err(interrupted(verb, name, e)),
        };
        if let Some(line) = line.strip_prefix("spark: ") {
            if !quiet {
                println!("{line}");
                printed = true;
            }
        } else if let Some(why) = line.strip_prefix("error: ") {
            return Err(fail(why.to_string()));
        } else if let Some(done) = line.strip_prefix("ok: ") {
            if !printed && !quiet {
                println!("{done}");
            }
            return Ok(0);
        }
    }
    if !quiet {
        println!("{}", said(&first));
    }
    Ok(0)
}

/// What a wait cut short by `error` says: SIGINT leaves the request going.
fn interrupted(verb: &str, name: &str, error: io::Error) -> Fail {
    if error.kind() != io::ErrorKind::Interrupted {
        return util::failed(&CONTROL, error);
    }
    let going = match verb {
        "stop" => "goes on stopping",
        _ => "goes on starting",
    };
    fail_with(130, format!("{name} {going}; `service {name} status` says how it goes"))
}

/// The lines Spark writes on a connection, read as they come.
struct Lines {
    stream: UnixStream,
    held: Vec<u8>,
}

impl Lines {
    /// The next whole line, None at the end; a SIGINT ends the wait with
    /// an error of kind Interrupted.
    fn next_line(&mut self) -> io::Result<Option<String>> {
        let mut buffer = [0u8; 1024];
        loop {
            if let Some(at) = self.held.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.held.drain(..=at).collect();
                return Ok(Some(String::from_utf8_lossy(&line[..at]).into_owned()));
            }
            if INTERRUPTED.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            match self.stream.read(&mut buffer) {
                Ok(0) if self.held.is_empty() => return Ok(None),
                Ok(0) => {
                    let line = std::mem::take(&mut self.held);
                    return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
                }
                Ok(read) => self.held.extend_from_slice(&buffer[..read]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

/// The clocks a document's times are read against: Spark's, in
/// milliseconds since boot, as the document was written, and the wall's,
/// in milliseconds since the epoch. Both are kept to the millisecond, so
/// that a time reads as the same second each time it is shown: rounding
/// either to whole seconds first moves it by up to a second between two
/// readings a moment apart.
struct Clock {
    now_ms: u64,
    wall_ms: i64,
}

impl Clock {
    /// The clocks as `now_ms` was read.
    fn at(now_ms: u64) -> Clock {
        Clock { now_ms, wall_ms: (util::now_ns() / 1_000_000) as i64 }
    }

    /// When `ms` (since boot) was, in local time: the time of day within
    /// the last day, and the date and time before.
    fn when(&self, ms: u64) -> String {
        let ago_ms = self.now_ms.saturating_sub(ms);
        let at = (self.wall_ms - i64::try_from(ago_ms).unwrap_or(i64::MAX)).div_euclid(1000);
        let format = if ago_ms / 1000 < 86_400 { "%H:%M:%S" } else { "%b %d %H:%M" };
        util::local_time(at, format)
    }

    /// When `ms` was, and how long ago: "10:42:03, 2 min ago".
    fn since(&self, ms: u64) -> String {
        format!("{}, {} ago", self.when(ms), span(self.now_ms.saturating_sub(ms)))
    }
}

/// What the system says of a service besides Spark: its processes, the
/// ports listening now, and the names of users.
struct Seen<'a> {
    /// Its processes: those in its domain, by number.
    processes: &'a [Process],
    /// Whether something listens on these ports, as (first, count).
    listening: &'a dyn Fn(u32, u32) -> bool,
    user_name: &'a dyn Fn(u32) -> String,
}

/// What `service NAME status` was asked for.
#[derive(Debug, PartialEq, Eq)]
struct StatusOptions {
    /// The log lines to end with, 0 for none.
    lines: usize,
    /// The JSON of the web console, which has no log lines: they are read
    /// with `logs.read-all`.
    json: bool,
}

/// `[-n LINES | --json]`.
fn status_options(name: &str, options: &[String]) -> Result<StatusOptions, Fail> {
    let (mut lines, mut json) = (None, false);
    let mut items = options.iter();
    while let Some(option) = items.next() {
        match option.as_str() {
            "--json" => json = true,
            "-n" | "--lines" => {
                let count = items.next().ok_or_else(|| {
                    usage(format!("Usage: service {name} status [-n LINES | --json]"))
                })?;
                lines = Some(
                    count.parse().map_err(|_| usage(format!("-n takes a number, not {count}")))?,
                );
            }
            _ => return Err(usage(format!("Usage: service {name} status [-n LINES | --json]"))),
        }
    }
    if json && lines.is_some() {
        return Err(usage("-n does not go with --json".into()));
    }
    Ok(StatusOptions { lines: lines.unwrap_or(if json { 0 } else { STATUS_LINES }), json })
}

/// A process of the service, in `status --json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Running {
    pid: u64,
    /// The name of the user it acts for.
    user: String,
    /// The program it was started as, the first word of its command line;
    /// null when the kernel does not say.
    program: Option<String>,
}

/// A port with a socket listening on it, in `status --json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Open {
    port: u16,
    /// Always true: the TCP tables are what is read, as `status` reads
    /// them for `(open)`.
    tcp: bool,
}

/// The trial of a change to the service's file, in `status --json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Tried {
    /// Spark's name for it, `service.NAME`.
    id: String,
    /// How long until it is undone unless it is kept.
    left_ms: u64,
}

/// What `service NAME status --json` writes, in this order.
#[derive(Debug, Serialize)]
struct StatusJson<'a> {
    /// Spark's clock when it answered, in milliseconds since the machine
    /// started, as the service's own times are.
    now_ms: u64,
    /// The service as Spark's `show NAME` gives it: Spark leaves out the
    /// last line it wrote and the last failure of its health check for
    /// anyone but root.
    service: &'a show::Service,
    /// Its processes, at most [`PROCESSES_JSON`].
    processes: Vec<Running>,
    /// The ports of its `network.listen` that a socket listens on now.
    listening: Vec<Open>,
    trial: Option<Tried>,
}

/// The ports of `listen`, each a port or a range as `show` writes it, that
/// a socket listens on, by port.
fn open_ports(listen: &[String], seen: &Seen) -> Vec<Open> {
    let mut ports = BTreeSet::new();
    for item in listen {
        let Some((first, count)) = port_range(item) else { continue };
        for port in first..first.saturating_add(count.min(PORTS_LOOKED_AT)) {
            if let Ok(number) = u16::try_from(port)
                && (seen.listening)(port, 1)
            {
                ports.insert(number);
            }
        }
    }
    ports.into_iter().map(|port| Open { port, tcp: true }).collect()
}

/// The JSON of `status` for `service`: what `seen` says of its processes
/// and ports, `program_of` of a process's program, and `trial`, the
/// trial of its file.
fn status_json<'a>(
    now_ms: u64,
    service: &'a show::Service,
    seen: &Seen,
    program_of: &dyn Fn(u64) -> Option<String>,
    trial: Option<&show::Trial>,
) -> StatusJson<'a> {
    let processes = seen
        .processes
        .iter()
        .take(PROCESSES_JSON)
        .map(|process| Running {
            pid: process.number,
            user: (seen.user_name)(process.user),
            program: program_of(process.number),
        })
        .collect();
    StatusJson {
        now_ms,
        service,
        processes,
        listening: open_ports(&service.holds.listen, seen),
        trial: trial.map(|trial| Tried { id: trial.id.clone(), left_ms: trial.left_ms }),
    }
}

/// `service NAME status [-n LINES | --json]`: what Spark says of the
/// service, as systemctl status lays it out, its processes and its last
/// log lines; 0 when it runs or is done, 3 otherwise (LSB). With
/// `--json`, the document of the web console, and 0 for a service Spark
/// knows in any state, since the JSON says it; 4 for one it does not.
pub(crate) fn status(name: &str, options: &[String]) -> Result<i32, Fail> {
    let StatusOptions { lines, json } = status_options(name, options)?;
    let mut document = shown(Some(name))?;
    // Whether it starts at boot, and so `disabled`, as `list` says it.
    as_filed(&mut document.services, &servicefiles::at_boot());
    let Some(service) = document.services.first() else {
        return Err(fail_with(4, format!("{name}: unrecognized service")));
    };
    let clock = Clock::at(document.now_ms);
    let processes: Vec<Process> = proc::processes()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.domain == service.name && !p.zombie())
        .collect();
    let tables: Vec<String> =
        TCP_TABLES.iter().filter_map(|t| std::fs::read_to_string(t).ok()).collect();
    let listening = |first: u32, count: u32| {
        tables.iter().any(|table| spark_api::ready::listening(table, &[(first, count)]))
    };
    let seen = Seen { processes: &processes, listening: &listening, user_name: &util::user_name };
    let id = servicetry::trial_id(name);
    let trial = document.trials.iter().find(|t| t.id == id);
    if json {
        let program_of =
            |number| proc::cmdline(number).first().map(|word| word.to_string_lossy().into_owned());
        let document = status_json(document.now_ms, service, &seen, &program_of, trial);
        println!("{}", serde_json::to_string(&document).map_err(|e| fail(e.to_string()))?);
        return Ok(0);
    }
    print!("{}", status_text(service, &clock, &seen));
    print!("{}", timer_field(name));
    if let Some(trial) = trial {
        print!("{}", trying_line(trial, name));
    }
    if lines > 0 {
        let records = log::recent(name, lines);
        if !records.is_empty() {
            println!();
            for record in records {
                println!("{record}");
            }
        }
    }
    Ok(if matches!(service.state, State::Running | State::Done) { 0 } else { 3 })
}

/// The `Timer:` line of `service NAME status` when the service has a
/// timer and timerd answers (it answers root alone), else nothing.
fn timer_field(name: &str) -> String {
    let mut text = String::new();
    let shown_at = |seconds: i64| util::local_time(seconds, "%Y-%m-%d %H:%M:%S");
    if let Some(row) =
        timerd::show::timers().ok().and_then(|rows| rows.into_iter().find(|r| r.name == name))
    {
        field(&mut text, "Timer", &timerd::show::status_line(&row, &shown_at));
    }
    text
}

/// The lines of `service NAME status`, but the log's.
fn status_text(service: &show::Service, clock: &Clock, seen: &Seen) -> String {
    let mut text = format!("{} - {}\n", service.name, service.description);
    field(&mut text, "Active", &active(service, clock));
    if let Some(why) = &service.why {
        field(&mut text, "Why", why);
    }
    let links: Vec<String> = [("requires", &service.requires), ("wants", &service.wants)]
        .into_iter()
        .chain([("after", &service.after)])
        .filter(|(_, names)| !names.is_empty())
        .map(|(key, names)| format!("{key} {}", names.join(", ")))
        .collect();
    if !links.is_empty() {
        field(&mut text, "Needs", &links.join("; "));
    }
    if let Some(by) = &service.started_by {
        field(&mut text, "Started", &format!("by {by}"));
    }
    if let Some(request) = &service.last_request {
        let who = (seen.user_name)(request.user);
        let when = clock.since(request.ms);
        let asked = format!("to {} by {who} (process {}) at {when}", request.verb, request.process);
        field(&mut text, "Asked", &asked);
    }
    if let Some(health) = &service.health {
        let state = match health.state {
            HealthState::Unknown => String::from("not checked yet"),
            HealthState::Passing => String::from("passing"),
            HealthState::Failing => {
                let last = health.last_failure.as_deref().unwrap_or("no reason given");
                format!("failing, {} checks in a row ({last})", health.failures)
            }
        };
        field(&mut text, "Health", &state);
    }
    let give_up = match service.give_up_after {
        0 => String::new(),
        n => format!("; gives up after {n} failed starts in a row"),
    };
    let restart = format!("{}, {} restarts so far{give_up}", service.restart, service.restarts);
    field(&mut text, "Restart", &restart);
    if let Some(stop) = stop_line(service) {
        field(&mut text, "", &stop);
    }
    if let Some(sockets) = sockets_line(service) {
        field(&mut text, "Sockets", &sockets);
    }
    if service.timer {
        field(&mut text, "Timer", "timerd starts it at the times its file names");
    }
    if let Some(end) = &service.last_end {
        let running = matches!(service.state, State::Running | State::Starting);
        match &service.last_line {
            Some(said) if !running => field(&mut text, "Last", &format!("{end} (\"{said}\")")),
            _ => field(&mut text, "Last", end),
        }
    }
    if let Some(complaint) = &service.complaint {
        field(&mut text, "File", &format!("{complaint}; it runs its last good copy"));
    }
    for (at, held) in holds_lines(&service.holds, seen).iter().enumerate() {
        field(&mut text, if at == 0 { "Holds" } else { "" }, held);
    }
    for (at, limit) in limit_lines(&service.holds).iter().enumerate() {
        field(&mut text, if at == 0 { "Limits" } else { "" }, limit);
    }
    if let Some(usage) = &service.usage {
        for (at, used) in use_lines(usage, &service.holds).iter().enumerate() {
            field(&mut text, if at == 0 { "Use" } else { "" }, used);
        }
    }
    if let Some(processes) = processes_line(seen.processes) {
        field(&mut text, "Processes", &processes);
    }
    text
}

/// The `Trying:` line of `service NAME status` while a change to it is
/// tried (ADR 0472).
fn trying_line(trial: &show::Trial, name: &str) -> String {
    let mut text = String::new();
    let left = span(trial.left_ms);
    let words = format!("a change, undone in {left} unless it is kept: `service keep {name}`");
    field(&mut text, "Trying", &words);
    text
}

/// Adds a line of `status`: its label right-aligned before a colon, as
/// systemctl's, or none for a line that goes on the one before.
fn field(text: &mut String, label: &str, value: &str) {
    if label.is_empty() {
        text.push_str(&format!("{:11}{value}\n", ""));
    } else {
        text.push_str(&format!("{label:>9}: {value}\n"));
    }
}

/// The `Active:` line's value: what the service is doing, and since when.
fn active(service: &show::Service, clock: &Clock) -> String {
    let pid = service.pid.map_or_else(String::new, |pid| format!(" as process {pid}"));
    let since = clock.since(service.since_ms);
    match service.state {
        State::Running => {
            let ready = match (service.started_ms, service.ready_ms) {
                (Some(started), Some(ready)) if service.ready_when != "started" => {
                    format!("; ready in {}", tenths(ready.saturating_sub(started)))
                }
                _ => String::new(),
            };
            format!("active (running){pid} since {since}{ready}")
        }
        State::Done => format!("active (exited) since {since}"),
        State::Starting => format!(
            "activating (starting{pid}, ready once {}) since {since}",
            ready_once(&service.ready_when)
        ),
        State::Pending => format!("activating (waiting for {})", service.waiting_for.join(", ")),
        State::Waiting => String::from("activating (waiting to start again)"),
        State::Stopping if service.pid.is_some() => format!("deactivating (stopping{pid})"),
        State::Stopping => String::from("deactivating (stopping what it left running)"),
        State::Failed => format!("failed (Spark gave up on it) since {since}"),
        State::Disabled => String::from("inactive (disabled)"),
        State::Stopped => format!("inactive (dead) since {since}"),
        State::Listening => format!("listening ({}) since {since}", explain::listening(service)),
    }
}

/// The line that says how the service stops, when its file's `[stop]` is
/// not the default (ADR 0477): "stops with SIGINT, then ends it after
/// 300 s".
fn stop_line(service: &show::Service) -> Option<String> {
    let default = Stop::default();
    let signal = service.stop_signal.as_str();
    let timeout = service.stop_timeout_s;
    let plain =
        (signal.is_empty() || signal == default.signal.name()) && timeout == default.timeout;
    (!plain).then(|| {
        let signal = if signal.is_empty() { default.signal.name() } else { signal };
        format!("stops with {signal}, then ends it after {timeout} s")
    })
}

/// The `Sockets:` line: the sockets Spark holds for the service (ADR
/// 0477), and whether a connection starts it.
fn sockets_line(service: &show::Service) -> Option<String> {
    let sockets = &service.holds.sockets;
    if sockets.is_empty() {
        return None;
    }
    let held = format!("{}, which Spark holds", and_list(sockets));
    Some(if service.start == "on-connection" {
        format!("{held}; a connection starts it")
    } else {
        held
    })
}

/// The `Holds:` lines: what the service's file gives it.
fn holds_lines(holds: &Holds, seen: &Seen) -> Vec<String> {
    let mut lines = Vec::new();
    let account = holds.account.as_deref().unwrap_or("root");
    lines.push(if holds.whole_tree {
        String::from("the whole tree, with Spark's access")
    } else {
        format!("its own view, acting for {account}")
    });
    for (verb, places) in [("reads", &holds.reads), ("changes", &holds.changes)] {
        if !places.is_empty() {
            lines.push(format!("{verb} {}", places.join(", ")));
        }
    }
    if !holds.mounts.is_empty() {
        lines.push(format!("mounts beneath {}", holds.mounts.join(", ")));
    }
    if let Some(class) = &holds.devices {
        let verb = if holds.devices_write { "reads and writes" } else { "reads" };
        let what = if class == "efi-system" { "the EFI system partitions" } else { "the disks" };
        lines.push(format!("{verb} {what}, as /devices"));
    }
    if !holds.root_subvolumes.is_empty() {
        let names = and_list(&holds.root_subvolumes);
        lines.push(format!("changes {names} at the top of the root, and nothing else there"));
    }
    if holds.every_process && !holds.whole_tree {
        lines.push(String::from("sees every process"));
    }
    if !holds.secrets.is_empty() {
        lines.push(format!("secrets {}", holds.secrets.join(", ")));
    }
    let mut network = Vec::new();
    if !holds.listen.is_empty() {
        let ports: Vec<String> = holds
            .listen
            .iter()
            .map(|ports| match port_range(ports) {
                Some((first, count)) if (seen.listening)(first, count) => format!("{ports} (open)"),
                _ => ports.clone(),
            })
            .collect();
        network.push(format!("listens on {}", ports.join(", ")));
    }
    if holds.outbound {
        network.push(String::from("connects out"));
    }
    lines.push(if network.is_empty() { String::from("no network") } else { network.join("; ") });
    if !holds.admin.is_empty() {
        lines.push(format!("sets the machine's {}", and_list(&holds.admin)));
    }
    if holds.names {
        lines.push(String::from("names processes and files for other users"));
    }
    if !holds.cards.is_empty() {
        lines.push(format!("drives {}", holds.cards.join(", ")));
    }
    lines
}

/// The `Limits:` lines: what the service's file limits its domain to
/// (ADR 0476), on a line for the CPU, memory, threads and files, and one
/// for the disks.
fn limit_lines(holds: &Holds) -> Vec<String> {
    let memory = holds.memory_bytes.map_or_else(
        || String::from("no memory limit"),
        |bytes| format!("memory up to {}", util::human(bytes, false)),
    );
    let mut cpu_and_memory = vec![
        memory,
        format!("CPU weight {}", holds.cpu_weight),
        format!("at most {} threads", holds.threads),
    ];
    cpu_and_memory.extend(holds.open_files.map(|files| format!("{files} open files")));
    let mut disks = vec![format!("disk weight {}", holds.io_weight)];
    disks.extend(rate_words("reads", holds.io_read, holds.io_read_ops));
    disks.extend(rate_words("writes", holds.io_write, holds.io_write_ops));
    vec![cpu_and_memory.join("; "), disks.join("; ")]
}

/// What a service may `verb` to disks each second, in bytes and in
/// requests, if either is limited.
fn rate_words(verb: &str, bytes: Option<u64>, requests: Option<u64>) -> Option<String> {
    let bytes = bytes.map(|bytes| format!("{} a second", util::human(bytes, false)));
    let requests = requests.map(|requests| format!("{requests} requests a second"));
    let limits: Vec<String> = bytes.into_iter().chain(requests).collect();
    (!limits.is_empty()).then(|| format!("{verb} up to {}", limits.join(" and ")))
}

/// The `Use:` lines: what the service's domain has used and been held
/// to, on a line for its memory, its threads and its disks (ADR 0476).
fn use_lines(usage: &Usage, holds: &Holds) -> Vec<String> {
    let human = |bytes| util::human(bytes, false);
    let mut memory =
        format!("memory {}, peak {}", human(usage.memory_bytes), human(usage.memory_peak_bytes));
    match usage.memory_refused {
        0 => {}
        1 => memory.push_str("; 1 charge refused"),
        n => memory.push_str(&format!("; {n} charges refused")),
    }
    if usage.stopped_at_limit {
        memory.push_str("; stopped at its memory limit");
    }
    vec![
        memory,
        format!("threads {} of {}", usage.threads, holds.threads),
        format!(
            "disk read {}, written {}; waited {} for a disk, held {} by its rates",
            human(usage.read_bytes),
            human(usage.written_bytes),
            tenths(usage.waited_ms),
            tenths(usage.held_ms)
        ),
    ]
}

/// A port or range of ports as `show` writes it, "80" or "8000-8010",
/// as (first, count).
fn port_range(ports: &str) -> Option<(u32, u32)> {
    match ports.split_once('-') {
        Some((first, last)) => {
            let (first, last): (u32, u32) = (first.parse().ok()?, last.parse().ok()?);
            Some((first, last.checked_sub(first)? + 1))
        }
        None => Some((ports.parse().ok()?, 1)),
    }
}

/// The `Processes:` line: how many, their numbers and names, their memory
/// and CPU time; None when it has none.
fn processes_line(processes: &[Process]) -> Option<String> {
    if processes.is_empty() {
        return None;
    }
    let mut named: Vec<String> = processes
        .iter()
        .take(PROCESSES_SHOWN)
        .map(|p| format!("{} {}", p.number, p.name))
        .collect();
    if processes.len() > PROCESSES_SHOWN {
        named.push(format!("and {} more", processes.len() - PROCESSES_SHOWN));
    }
    let memory: u64 = processes.iter().map(|p| p.resident_kib * 1024).sum();
    let cpu_ms: u64 = processes.iter().map(|p| p.cpu_ns() / 1_000_000).sum();
    Some(format!(
        "{} ({}), {} of memory, {} of CPU",
        processes.len(),
        named.join(", "),
        util::human(memory, false),
        tenths(cpu_ms)
    ))
}

/// `service NAME explain [--json]`: why the service is in its state and
/// what to do, followed through the services it waits for or that
/// stopped it, from the document of every service; 0 when it runs with
/// its health check not failing, or is done, 3 otherwise.
fn explain(name: &str, options: &[String]) -> Result<i32, Fail> {
    let mut json = false;
    for option in options {
        match option.as_str() {
            "--json" => json = true,
            other => return Err(usage(format!("service explain: unknown option `{other}'"))),
        }
    }
    let document = shown(None)?;
    let clock = Clock::at(document.now_ms);
    let when = |ms| clock.since(ms);
    let context = spark_api::explain::Context { when: &when, user_name: &util::user_name };
    let Some(mut explanation) = spark_api::explain::explain(&document, name, &context) else {
        return Err(fail_with(4, format!("{name}: unrecognized service")));
    };
    let well = document.services.iter().any(|s| s.name == name && runs_well(s));
    // A fix the signatures know for the service at the root of the chain
    // replaces the generic one (ADR 0474).
    if !well && let Some(hit) = crate::troubleshoot::best_for_service(&explanation.root) {
        explanation.fix = Some(format!("{} ({}, `troubleshoot` says more)", hit.fix, hit.id));
    }
    if json {
        println!("{}", serde_json::to_string(&explanation).map_err(|e| fail(e.to_string()))?);
    } else {
        println!("{}", explanation.text());
    }
    Ok(if well { 0 } else { 3 })
}

/// Whether `service` is done, or runs with its health check not failing.
fn runs_well(service: &show::Service) -> bool {
    match service.state {
        State::Done => true,
        State::Running => !service.health.as_ref().is_some_and(|h| h.state == HealthState::Failing),
        _ => false,
    }
}

/// `service list [--failed] [--json] [--timers]`: each service, its
/// state, whether it starts at boot, since when and what it is; with
/// `--failed`, those not running as asked alone, and 1 if there are some;
/// with `--timers`, the services with a timer instead, as timerd says.
fn list(options: &[String]) -> Result<i32, Fail> {
    let (mut failed_only, mut json, mut timers) = (false, false, false);
    for option in options {
        match option.as_str() {
            "--failed" => failed_only = true,
            "--json" => json = true,
            "--timers" => timers = true,
            other => return Err(usage(format!("service list: unknown option `{other}'"))),
        }
    }
    if timers {
        if failed_only {
            return Err(usage("service list: --failed does not go with --timers".into()));
        }
        return list_timers(json);
    }
    let mut document = shown(None)?;
    without_removed(&mut document.services, |name| {
        let file = Path::new(servicefiles::DIR).join(format!("{name}.toml"));
        !matches!(fs::symlink_metadata(file), Err(e) if e.kind() == io::ErrorKind::NotFound)
    });
    as_filed(&mut document.services, &servicefiles::at_boot());
    if json {
        println!("{}", show::to_json(&document).map_err(fail)?);
        return Ok(0);
    }
    document.services.sort_by(|a, b| a.name.cmp(&b.name));
    let clock = Clock::at(document.now_ms);
    let wrong: Vec<&show::Service> = document.services.iter().filter(|s| not_as_asked(s)).collect();
    let listed: Vec<&show::Service> =
        if failed_only { wrong.clone() } else { document.services.iter().collect() };
    let terminal = util::is_terminal(1);
    let mut out = io::stdout().lock();
    write!(out, "{}", list_text(&listed, &clock, terminal))?;
    if terminal && !failed_only && !wrong.is_empty() {
        let names: Vec<String> = wrong.iter().map(|s| s.name.clone()).collect();
        writeln!(out, "\n{}", not_as_asked_line(&names))?;
    }
    Ok(i32::from(failed_only && !wrong.is_empty()))
}

/// Leaves out of `services` each one that is stopped and whose file,
/// as `has_file` says, is gone: the service of a bundle that was removed
/// (ADR 0591), or one an administrator took the file of. Spark keeps it,
/// stopped, until the next boot, which no longer knows it; the list is of
/// what the system has. One that still runs, or does anything else, is
/// listed, file or not.
fn without_removed(services: &mut Vec<show::Service>, has_file: impl Fn(&str) -> bool) {
    services.retain(|s| !matches!(s.state, State::Stopped | State::Disabled) || has_file(&s.name));
}

/// Sets whether each of `services` starts at boot to what its file says
/// (`at_boot`, by name), which is what the next boot does. Spark reads a
/// service's file again only as it starts the service, so its own word
/// does not follow `service NAME disable` of a service that is stopped, or
/// `enable` of one that runs. A stopped service that nothing started and
/// whose file says it does not start at boot is `disabled`, as Spark says
/// of one, and one whose file says it does is `stopped`. A service whose
/// file does not read keeps Spark's word.
fn as_filed(services: &mut [show::Service], at_boot: &BTreeMap<String, bool>) {
    for service in services {
        let Some(&enabled) = at_boot.get(&service.name) else { continue };
        service.enabled = enabled;
        service.state = match service.state {
            State::Stopped if !enabled && service.started_by.is_none() => State::Disabled,
            State::Disabled if enabled => State::Stopped,
            state => state,
        };
    }
}

/// `service list --timers [--json]`: each service with a timer, its next
/// run and its last start.
fn list_timers(json: bool) -> Result<i32, Fail> {
    let rows = timerd::show::timers().map_err(fail)?;
    if json {
        let files = servicefiles::read_all()
            .into_iter()
            .filter_map(|(name, _, service)| Some((name, service.timer?)))
            .collect();
        // Spark's word on how each timer's last run went; where it cannot
        // be had, no result is known.
        let results: BTreeMap<String, &'static str> =
            shown(None).ok().map_or_else(BTreeMap::new, |document| {
                document
                    .services
                    .iter()
                    .filter_map(|service| Some((service.name.clone(), last_result(service)?)))
                    .collect()
            });
        let timers = timers_json(&rows, &files, &results, machine_zone().as_deref());
        println!("{}", serde_json::to_string(&timers).map_err(|e| fail(e.to_string()))?);
        return Ok(0);
    }
    let shown_at = |seconds: i64| util::local_time(seconds, "%Y-%m-%d %H:%M:%S");
    print!("{}", timerd::show::table(&rows, &shown_at, util::is_terminal(1)));
    Ok(0)
}

/// A timer, in `service list --timers --json`: times are Unix
/// milliseconds, each `null` where nothing says it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct TimerJson {
    /// The service the timer starts.
    service: String,
    /// The schedule as the service's file writes it; `null` when the
    /// file no longer reads or has no `[timer]`.
    schedule: Option<String>,
    /// When it starts next; `null` while it runs and when no time comes
    /// within five years.
    next_ms: Option<i64>,
    /// When it last started the service.
    last_ms: Option<i64>,
    /// How that run went, `ok` or `failed`, as Spark has it since boot;
    /// `null` while it runs, after a start by hand, and before a start
    /// by the timer since boot.
    last_result: Option<&'static str>,
    /// The time zone its times of day are read in: the file's, or the
    /// system's.
    zone: Option<String>,
}

/// The timers of `rows`, as timerd keeps them, with each service's
/// `timers` (from its file), the `results` of the last runs by service,
/// and the machine's time zone, `machine`, for a timer whose file names
/// none: timerd's own answer has no schedule and no zone.
fn timers_json(
    rows: &[timerd::protocol::TimerRow],
    timers: &BTreeMap<String, manifest::service::Timer>,
    results: &BTreeMap<String, &'static str>,
    machine: Option<&str>,
) -> Vec<TimerJson> {
    rows.iter()
        .map(|row| {
            let timer = timers.get(&row.name);
            TimerJson {
                service: row.name.clone(),
                schedule: timer.map(|timer| timer.schedule.clone()),
                next_ms: row.next.map(|seconds| seconds.saturating_mul(1000)),
                last_ms: row.last.map(|seconds| seconds.saturating_mul(1000)),
                last_result: results.get(&row.name).copied(),
                zone: timer
                    .and_then(|timer| timer.time_zone.as_deref())
                    .or(machine)
                    .map(str::to_string),
            }
        })
        .collect()
}

/// How the last run of a service that its timer started went, from
/// Spark's document: `ok` when its process ended well, `failed` when it
/// ended any other way or Spark gave up on it, and none while it runs,
/// when it was last started by anything else, and where no end is known.
fn last_result(service: &show::Service) -> Option<&'static str> {
    if service.started_by.as_deref() != Some("its timer") {
        return None;
    }
    match service.state {
        State::Pending | State::Starting | State::Running | State::Stopping => None,
        State::Done => Some("ok"),
        State::Failed => Some("failed"),
        State::Stopped | State::Disabled | State::Waiting | State::Listening => {
            service.last_status.map(|status| if status == 0 { "ok" } else { "failed" })
        }
    }
}

/// The machine's time zone by name, as timedatectl(1) says it; none when
/// `/etc/localtime` is a file of its own, which names no zone.
fn machine_zone() -> Option<String> {
    Some(crate::timedatectl::timezone()).filter(|zone| !zone.contains(' '))
}

/// Whether `service` is not running as asked: given up on, waiting to
/// start again after an end, stopped for a reason that is not one it is
/// meant to be off for, or failing its health check. alertd(8) raises
/// its alerts by the same rule.
pub(crate) fn not_as_asked(service: &show::Service) -> bool {
    match service.state {
        State::Failed | State::Waiting => true,
        State::Stopped => service.why.is_some() && !service.off_as_meant,
        State::Running => service.health.as_ref().is_some_and(|h| h.state == HealthState::Failing),
        _ => false,
    }
}

/// `list`'s table: a header on a terminal, then a line a service.
fn list_text(services: &[&show::Service], clock: &Clock, header: bool) -> String {
    let rows: Vec<[String; 5]> = services
        .iter()
        .map(|service| {
            let mut state = String::from(service.state.word());
            if service.state == State::Running
                && service.health.as_ref().is_some_and(|h| h.state == HealthState::Failing)
            {
                state.push_str(", health failing");
            }
            [
                service.name.clone(),
                state,
                String::from(if service.enabled { "on" } else { "off" }),
                clock.when(service.since_ms),
                service.description.clone(),
            ]
        })
        .collect();
    let titles = ["SERVICE", "STATE", "AT BOOT", "SINCE", "DESCRIPTION"].map(String::from);
    let mut widths = [0; 4];
    for row in rows.iter().chain(header.then_some(&titles)) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut text = String::new();
    for row in header.then_some(&titles).into_iter().chain(&rows) {
        for (cell, width) in row.iter().zip(widths) {
            text.push_str(&format!("{cell:<width$}  "));
        }
        text.push_str(&row[4]);
        text.push('\n');
    }
    text
}

/// The line under `list`'s table that names the services not running as
/// asked, at most NAMES_SHOWN of them.
fn not_as_asked_line(names: &[String]) -> String {
    let mut shown: Vec<String> = names.iter().take(NAMES_SHOWN).cloned().collect();
    if names.len() > NAMES_SHOWN {
        shown.push(format!("{} more", names.len() - NAMES_SHOWN));
    }
    format!("Not running as asked: {}. `service {} explain` says why.", and_list(&shown), names[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_api::show::{Health, Request};

    const CLOCK: Clock = Clock { now_ms: 600_000, wall_ms: 1_800_000_000_000 };

    #[test]
    fn a_time_reads_as_the_same_second_from_clocks_read_a_moment_apart() {
        // A stop 4.5 s before CLOCK's reading, read again 700 ms later.
        let later = Clock { now_ms: CLOCK.now_ms + 700, wall_ms: CLOCK.wall_ms + 700 };
        let stop = CLOCK.now_ms - 4_500;
        assert_eq!(CLOCK.when(stop), later.when(stop));
        assert_eq!(CLOCK.when(stop), util::local_time(CLOCK.wall_ms / 1000 - 5, "%H:%M:%S"));
        // A day and more ago, the date and the minute.
        let old = CLOCK.now_ms + 86_400_000;
        let next_day = Clock { now_ms: old, wall_ms: CLOCK.wall_ms + 86_400_000 };
        assert_eq!(
            next_day.when(CLOCK.now_ms),
            util::local_time(CLOCK.wall_ms / 1000, "%b %d %H:%M")
        );
    }

    fn nginx() -> show::Service {
        show::Service {
            name: "nginx".into(),
            description: "nginx web server".into(),
            enabled: true,
            restart: "on-failure".into(),
            after: vec!["netd".into(), "resolved".into()],
            ready_when: "listening".into(),
            state: State::Running,
            pid: Some(57),
            since_ms: 480_000,
            started_ms: Some(469_600),
            ready_ms: Some(480_000),
            started_by: Some("boot".into()),
            holds: Holds {
                account: Some("org.nginx.nginx".into()),
                reads: vec!["/applications/org.nginx.nginx".into(), "/shared/www".into()],
                changes: vec!["/system/state/org.nginx.nginx".into()],
                secrets: vec!["tls-key".into()],
                listen: vec!["80".into(), "443".into()],
                outbound: true,
                memory_bytes: Some(256 << 20),
                cpu_weight: 100,
                io_weight: 100,
                threads: 4096,
                open_files: Some(65536),
                ..Holds::default()
            },
            ..show::Service::default()
        }
    }

    #[test]
    fn a_trial_has_the_time_left_of_the_one_with_its_name() {
        let trial =
            |id: &str, left_ms| show::Trial { id: id.into(), left_ms, ..show::Trial::default() };
        let document = show::Show {
            trials: vec![trial("service.nginx", 41_000), trial("network", 9_000)],
            ..show::Show::default()
        };
        assert_eq!(left_of(&document, "network"), Some(9_000));
        assert_eq!(left_of(&document, "service.nginx"), Some(41_000));
        assert_eq!(left_of(&document, "firewall"), None);
        assert_eq!(left_of(&show::Show::default(), "network"), None);
    }

    fn process(number: u64, name: &str, resident_kib: u64) -> Process {
        Process {
            number,
            name: name.into(),
            resident_kib,
            user_ns: 600_000_000,
            ..Process::default()
        }
    }

    fn seen_with<'a>(processes: &'a [Process], open: &'a dyn Fn(u32, u32) -> bool) -> Seen<'a> {
        Seen {
            processes,
            listening: open,
            user_name: &|uid| if uid == 0 { "root".into() } else { "chase".into() },
        }
    }

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn status_takes_a_count_of_log_lines_or_json_and_not_both() {
        let read = |items: &[&str]| status_options("nginx", &words(items));
        assert_eq!(read(&[]).unwrap(), StatusOptions { lines: STATUS_LINES, json: false });
        assert_eq!(read(&["-n", "20"]).unwrap(), StatusOptions { lines: 20, json: false });
        assert_eq!(read(&["--lines", "0"]).unwrap(), StatusOptions { lines: 0, json: false });
        assert_eq!(read(&["--json"]).unwrap(), StatusOptions { lines: 0, json: true });
        for items in [
            &["-n"][..],
            &["-n", "many"],
            &["-n", "5", "--json"],
            &["--json", "-n", "5"],
            &["--json", "--json", "-n", "0"],
            &["--text"],
            &["5"],
        ] {
            let error = read(items).unwrap_err();
            assert!(error.usage && error.code == 1, "{items:?}: {}", error.message);
        }
        assert_eq!(read(&["-n", "5", "--json"]).unwrap_err().message, "-n does not go with --json");
    }

    #[test]
    fn status_json_has_the_clock_the_service_its_processes_its_ports_and_its_trial_in_that_order() {
        let mut web = nginx();
        web.holds.listen = vec!["80".into(), "443".into(), "8000-8003".into(), "bad".into()];
        let processes = [
            Process { user: 1000, ..process(57, "nginx", 4096) },
            Process { user: 0, ..process(58, "nginx", 2048) },
        ];
        // Port 80, a port in the range, and port 22, which no file names.
        let open =
            |first: u32, count: u32| (first..first + count).any(|p| [80, 8001, 22].contains(&p));
        let seen = seen_with(&processes, &open);
        let program = |number: u64| (number == 57).then(|| "/system/commands/nginx".to_string());
        let trial =
            show::Trial { id: "service.nginx".into(), left_ms: 41_000, ..show::Trial::default() };
        let document = status_json(51_234, &web, &seen, &program, Some(&trial));
        let text = serde_json::to_string(&document).unwrap();
        let order: Vec<usize> =
            ["\"now_ms\":", "\"service\":", "\"processes\":", "\"listening\":", "\"trial\":"]
                .iter()
                .map(|key| text.find(key).unwrap())
                .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 5);
        assert_eq!(value["now_ms"], 51_234);
        assert_eq!(value["service"], serde_json::to_value(&web).unwrap());
        assert_eq!(
            value["processes"],
            serde_json::json!([
                {"pid": 57, "user": "chase", "program": "/system/commands/nginx"},
                {"pid": 58, "user": "root", "program": null},
            ])
        );
        assert_eq!(
            value["listening"],
            serde_json::json!([
                {"port": 80, "tcp": true},
                {"port": 8001, "tcp": true},
            ])
        );
        assert_eq!(value["trial"], serde_json::json!({"id": "service.nginx", "left_ms": 41_000}));
    }

    #[test]
    fn status_json_has_no_trial_none_and_at_most_fifty_processes() {
        let web = nginx();
        let processes: Vec<Process> = (1..=60).map(|n| process(n, "nginx", 1)).collect();
        let seen = seen_with(&processes, &|_, _| false);
        let document = status_json(0, &web, &seen, &|_| None, None);
        let value = serde_json::to_value(&document).unwrap();
        assert_eq!(value["trial"], serde_json::Value::Null);
        assert_eq!(value["listening"], serde_json::json!([]));
        let shown = value["processes"].as_array().unwrap();
        assert_eq!(shown.len(), PROCESSES_JSON);
        assert_eq!(
            (&shown[0]["pid"], &shown[49]["pid"]),
            (&serde_json::json!(1), &serde_json::json!(50))
        );
        let none = status_json(0, &web, &seen_with(&[], &|_, _| false), &|_| None, None);
        assert_eq!(serde_json::to_value(&none).unwrap()["processes"], serde_json::json!([]));
    }

    #[test]
    fn a_range_is_looked_at_for_its_first_ports_only() {
        let mut web = nginx();
        web.holds.listen = vec!["1-65535".into()];
        let looked = std::cell::Cell::new(0);
        let open = |first: u32, count: u32| {
            looked.set(looked.get() + 1);
            first <= 5000 && 5000 < first + count
        };
        let seen = seen_with(&[], &open);
        assert!(open_ports(&web.holds.listen, &seen).is_empty());
        assert_eq!(looked.get(), PORTS_LOOKED_AT);
        // The ports are the ones asked, not the range's.
        web.holds.listen = vec!["5000-5001".into()];
        let ports: Vec<u16> = open_ports(&web.holds.listen, &seen).iter().map(|o| o.port).collect();
        assert_eq!(ports, [5000]);
    }

    #[test]
    fn status_says_how_a_service_stops_and_its_sockets() {
        // The default stop says nothing.
        let mut service = nginx();
        service.stop_signal = "SIGTERM".into();
        service.stop_timeout_s = 5;
        assert_eq!(stop_line(&service), None);
        // An older Spark's document, without them, is the default too.
        service.stop_signal = String::new();
        assert_eq!(stop_line(&service), None);
        service.stop_signal = "SIGINT".into();
        service.stop_timeout_s = 300;
        assert_eq!(
            stop_line(&service).as_deref(),
            Some("stops with SIGINT, then ends it after 300 s")
        );
        service.stop_signal = "SIGTERM".into();
        service.stop_timeout_s = 30;
        assert_eq!(
            stop_line(&service).as_deref(),
            Some("stops with SIGTERM, then ends it after 30 s")
        );

        assert_eq!(sockets_line(&service), None);
        service.holds.sockets = vec!["port 80".into(), "port 80 on ::".into()];
        service.start = "at-boot".into();
        assert_eq!(
            sockets_line(&service).as_deref(),
            Some("port 80 and port 80 on ::, which Spark holds")
        );
        service.start = "on-connection".into();
        assert_eq!(
            sockets_line(&service).as_deref(),
            Some("port 80 and port 80 on ::, which Spark holds; a connection starts it")
        );
        service.state = State::Listening;
        service.pid = None;
        let line = active(&service, &CLOCK);
        assert!(
            line.starts_with(
                "listening (waits for a connection on port 80 and port 80 on ::, which starts \
                 it) since "
            ),
            "{line}"
        );
        let text = status_text(&service, &CLOCK, &seen_with(&[], &|_, _| false));
        assert!(
            text.contains("  Sockets: port 80 and port 80 on ::, which Spark holds; a"),
            "{text}"
        );
        assert!(text.contains("stops with SIGTERM, then ends it after 30 s\n"), "{text}");
    }

    #[test]
    fn status_says_what_a_service_holds_and_runs() {
        let processes = [process(57, "nginx", 4096), process(58, "nginx", 2048)];
        let open = |first: u32, _| first == 80;
        let text = status_text(&nginx(), &CLOCK, &seen_with(&processes, &open));
        let expected = [
            "nginx - nginx web server",
            "   Active: active (running) as process 57 since ",
            ", 2 min ago; ready in 10.4 s",
            "    Needs: after netd, resolved",
            "  Started: by boot",
            "    Holds: its own view, acting for org.nginx.nginx",
            "           reads /applications/org.nginx.nginx, /shared/www",
            "           changes /system/state/org.nginx.nginx",
            "           secrets tls-key",
            "           listens on 80 (open), 443; connects out",
            "   Limits: memory up to 256M; CPU weight 100; at most 4096 threads; 65536 open files",
            "           disk weight 100",
            "Processes: 2 (57 nginx, 58 nginx), 6.0M of memory, 1.2 s of CPU",
        ];
        for line in expected {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
    }

    /// The disks by class, the root's subvolumes by name and every
    /// process, each on its line (ADR 0473).
    #[test]
    fn status_says_which_disks_subvolumes_and_processes_a_service_holds() {
        let mut updater = nginx();
        updater.holds = Holds {
            devices: Some("efi-system".into()),
            devices_write: true,
            root_subvolumes: vec!["@snapshots".into(), "@state".into()],
            every_process: true,
            ..Holds::default()
        };
        let text = status_text(&updater, &CLOCK, &seen_with(&[], &|_, _| false));
        for line in [
            "           reads and writes the EFI system partitions, as /devices",
            "           changes @snapshots and @state at the top of the root, and nothing else \
             there",
            "           sees every process",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        updater.holds = Holds { devices: Some("disks".into()), ..Holds::default() };
        let text = status_text(&updater, &CLOCK, &seen_with(&[], &|_, _| false));
        assert!(text.contains("           reads the disks, as /devices"), "{text}");
        assert!(!text.contains("every process") && !text.contains("top of the root"), "{text}");
    }

    /// Each limit its file names, and what its domain used (ADR 0476).
    #[test]
    fn status_says_each_limit_and_what_the_domain_used() {
        let mut web = nginx();
        web.holds = Holds {
            memory_bytes: Some(64 << 20),
            cpu_weight: 100,
            io_weight: 400,
            io_read: Some(50 << 20),
            io_write_ops: Some(200),
            threads: 64,
            open_files: Some(10_000),
            ..Holds::default()
        };
        web.usage = Some(Usage {
            memory_bytes: 12 << 20,
            memory_peak_bytes: 64 << 20,
            memory_refused: 3,
            stopped_at_limit: true,
            threads: 4,
            read_bytes: 1 << 20,
            waited_ms: 1_234,
            held_ms: 300,
            ..Usage::default()
        });
        let text = status_text(&web, &CLOCK, &seen_with(&[], &|_, _| false));
        for line in [
            "   Limits: memory up to 64M; CPU weight 100; at most 64 threads; 10000 open files",
            "           disk weight 400; reads up to 50M a second; writes up to 200 requests a \
             second",
            "      Use: memory 12M, peak 64M; 3 charges refused; stopped at its memory limit",
            "           threads 4 of 64",
            "           disk read 1.0M, written 0; waited 1.2 s for a disk, held 0.3 s by its rates",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        // One refused charge is one charge, and a service that was not
        // stopped at its limit says nothing of it.
        if let Some(usage) = &mut web.usage {
            usage.memory_refused = 1;
            usage.stopped_at_limit = false;
        }
        let text = status_text(&web, &CLOCK, &seen_with(&[], &|_, _| false));
        assert!(text.contains("memory 12M, peak 64M; 1 charge refused\n"), "{text}");
        // Before its domain is made, nothing was used.
        web.usage = None;
        let text = status_text(&web, &CLOCK, &seen_with(&[], &|_, _| false));
        assert!(text.contains("   Limits: ") && !text.contains("Use:"), "{text}");
    }

    #[test]
    fn status_says_who_asked_and_what_a_stopped_one_said_last() {
        let mut web = nginx();
        web.state = State::Stopped;
        web.pid = None;
        web.last_end = Some("exited with code 0x1".into());
        web.last_line = Some("nginx: [emerg] bind() to 0.0.0.0:80 failed".into());
        web.last_request =
            Some(Request { verb: "stop".into(), user: 1000, process: 812, ms: 420_000 });
        web.holds = Holds { whole_tree: true, ..Holds::default() };
        let text = status_text(&web, &CLOCK, &seen_with(&[], &|_, _| false));
        for line in [
            "   Active: inactive (dead) since ",
            "    Asked: to stop by chase (process 812) at ",
            ", 3 min ago\n",
            "     Last: exited with code 0x1 (\"nginx: [emerg] bind() to 0.0.0.0:80 failed\")",
            "    Holds: the whole tree, with Spark's access",
            "           no network",
            "   Limits: no memory limit; CPU weight 0; at most 0 threads",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        assert!(!text.contains("Processes:"), "{text}");
    }

    #[test]
    fn a_starting_service_says_what_it_waits_for() {
        let mut web = nginx();
        web.state = State::Starting;
        web.since_ms = 596_000;
        let text = status_text(&web, &CLOCK, &seen_with(&[], &|_, _| false));
        assert!(
            text.contains("activating (starting as process 57, ready once it listens) since "),
            "{text}"
        );
        assert!(text.contains(", 4 s ago\n"), "{text}");
    }

    #[test]
    fn a_change_being_tried_says_when_it_is_undone() {
        let trial = show::Trial {
            id: "service.nginx".into(),
            files: vec!["/system/settings/spark/services/nginx.toml".into()],
            services: vec!["nginx".into()],
            user: 0,
            left_ms: 42_400,
        };
        assert_eq!(
            trying_line(&trial, "nginx"),
            "   Trying: a change, undone in 42 s unless it is kept: `service keep nginx`\n"
        );
    }

    #[test]
    fn ports_read_as_ranges() {
        assert_eq!(port_range("80"), Some((80, 1)));
        assert_eq!(port_range("8000-8010"), Some((8000, 11)));
        assert_eq!(port_range("9-8"), None);
        assert_eq!(port_range("http"), None);
    }

    #[test]
    fn a_service_runs_well_when_it_is_done_or_runs_with_its_check_not_failing() {
        let mut web = nginx();
        assert!(runs_well(&web));
        web.health = Some(Health { state: HealthState::Passing, ..Health::default() });
        assert!(runs_well(&web));
        web.health = Some(Health { state: HealthState::Failing, ..Health::default() });
        assert!(!runs_well(&web));
        web.state = State::Done;
        assert!(runs_well(&web));
        for state in [State::Stopped, State::Failed, State::Pending, State::Starting] {
            web.state = state;
            assert!(!runs_well(&web), "{state:?}");
        }
    }

    #[test]
    fn the_list_lines_up_and_names_what_is_wrong() {
        let mut web = nginx();
        web.name = "lab-web".into();
        web.description = "Lab web".into();
        web.enabled = false;
        web.state = State::Failed;
        // Its last checks failed, but a service that does not run has no
        // health to show.
        web.health = Some(Health { state: HealthState::Failing, ..Health::default() });
        let mut sick = nginx();
        sick.name = "lab-sick".into();
        sick.health = Some(Health { state: HealthState::Failing, ..Health::default() });
        let fine = nginx();
        let mut refused = nginx();
        refused.state = State::Stopped;
        refused.why = Some("it requires db, which Spark gave up on".into());
        // A driver the kernel left no cards, as on most virtual machines.
        let mut idle = refused.clone();
        idle.why = Some("the kernel left it none of the cards it drives".into());
        idle.off_as_meant = true;
        let text = list_text(&[&sick, &web, &fine], &CLOCK, true);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("SERVICE   STATE                    AT BOOT  SINCE     DESC"),
            "{text}"
        );
        assert!(lines[1].starts_with("lab-sick  running, health failing  on       "), "{text}");
        assert!(lines[2].starts_with("lab-web   failed                   off      "), "{text}");
        assert!(lines[2].ends_with("  Lab web"), "{text}");
        assert!(!list_text(&[&fine], &CLOCK, false).contains("SERVICE"));
        assert!([&web, &sick, &refused].iter().all(|s| not_as_asked(s)));
        assert!([&fine, &idle].iter().all(|s| !not_as_asked(s)));
        assert_eq!(
            not_as_asked_line(&["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]),
            "Not running as asked: a, b, c and 2 more. `service a explain` says why."
        );
    }

    #[test]
    fn whether_a_service_starts_at_boot_is_what_its_file_says() {
        let named = |name: &str, state, enabled, started_by: Option<&str>| show::Service {
            name: name.into(),
            state,
            enabled,
            started_by: started_by.map(String::from),
            ..nginx()
        };
        let mut services = vec![
            // Disabled while stopped: Spark's copy still says at boot.
            named("stopped-off", State::Stopped, true, None),
            // Enabled while it runs: Spark's copy still says not.
            named("running-on", State::Running, false, Some("boot")),
            // Enabled while Spark calls it disabled.
            named("disabled-on", State::Disabled, false, None),
            // Off, but another service started it, so it is not disabled.
            named("wanted", State::Stopped, true, Some("web")),
            // Its file does not read: Spark's word stands.
            named("unread", State::Stopped, true, None),
        ];
        let at_boot = BTreeMap::from([
            ("stopped-off".to_string(), false),
            ("running-on".to_string(), true),
            ("disabled-on".to_string(), true),
            ("wanted".to_string(), false),
        ]);
        as_filed(&mut services, &at_boot);
        let seen: Vec<(&str, State, bool)> =
            services.iter().map(|s| (s.name.as_str(), s.state, s.enabled)).collect();
        assert_eq!(
            seen,
            [
                ("stopped-off", State::Disabled, false),
                ("running-on", State::Running, true),
                ("disabled-on", State::Stopped, true),
                ("wanted", State::Stopped, false),
                ("unread", State::Stopped, true),
            ]
        );
    }

    #[test]
    fn a_stopped_service_whose_file_is_gone_is_not_listed() {
        let named = |name: &str, state| show::Service { name: name.into(), state, ..nginx() };
        let mut services = vec![
            named("beacon", State::Stopped),
            named("tidy", State::Disabled),
            named("lab-web", State::Running),
            named("lab-sick", State::Failed),
            named("logd", State::Stopped),
        ];
        without_removed(&mut services, |name| name == "logd");
        let left: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(left, ["lab-web", "lab-sick", "logd"]);
    }

    fn timer(text: &str) -> manifest::service::Timer {
        let file = format!("service_version = 1\nprogram = \"/system/commands/true\"\n{text}");
        manifest::service::from_toml(&file).unwrap().timer.unwrap()
    }

    fn row(name: &str, last: Option<i64>, next: Option<i64>) -> timerd::protocol::TimerRow {
        timerd::protocol::TimerRow { name: name.into(), last, next, running: false }
    }

    #[test]
    fn timers_json_has_a_timer_in_timerds_order_with_milliseconds_and_the_zone() {
        let rows = [
            row("backup", Some(1_763_172_000), Some(1_763_258_400)),
            row("logturn", None, Some(1_763_175_600)),
            row("scrub", Some(1_763_000_000), None),
        ];
        let timers = BTreeMap::from([
            (
                "backup".to_string(),
                timer("[timer]\nat = [\"0 3 * * *\"]\ntime_zone = \"Europe/Prague\"\n"),
            ),
            ("logturn".to_string(), timer("[timer]\nevery = \"1h\"\nafter_boot = \"5m\"\n")),
            ("scrub".to_string(), timer("[timer]\nat = [\"weekly\", \"30 2 * * 1\"]\n")),
        ]);
        let results =
            BTreeMap::from([("backup".to_string(), "ok"), ("scrub".to_string(), "failed")]);
        let timers = timers_json(&rows, &timers, &results, Some("UTC"));
        let text = serde_json::to_string(&timers).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            serde_json::json!([
                {
                    "service": "backup", "schedule": "0 3 * * *",
                    "next_ms": 1_763_258_400_000_i64, "last_ms": 1_763_172_000_000_i64,
                    "last_result": "ok", "zone": "Europe/Prague"
                },
                {
                    "service": "logturn", "schedule": "every 1h, first 5m after boot",
                    "next_ms": 1_763_175_600_000_i64, "last_ms": null,
                    "last_result": null, "zone": "UTC"
                },
                {
                    "service": "scrub", "schedule": "weekly, 30 2 * * 1",
                    "next_ms": null, "last_ms": 1_763_000_000_000_i64,
                    "last_result": "failed", "zone": "UTC"
                }
            ])
        );
        // The keys come in the order the design gives them.
        assert!(text.starts_with(
            r#"[{"service":"backup","schedule":"0 3 * * *","next_ms":1763258400000,"last_ms":1763172000000,"last_result":"ok","zone":"Europe/Prague"}"#
        ));
    }

    #[test]
    fn a_timer_with_no_file_or_no_zone_says_null_and_none_is_made_up() {
        let rows = [row("gone", Some(5), Some(9))];
        let timers = timers_json(&rows, &BTreeMap::new(), &BTreeMap::new(), None);
        assert_eq!(
            timers,
            [TimerJson {
                service: "gone".into(),
                schedule: None,
                next_ms: Some(9000),
                last_ms: Some(5000),
                last_result: None,
                zone: None,
            }]
        );
        assert!(timers_json(&[], &BTreeMap::new(), &BTreeMap::new(), None).is_empty());
        assert_eq!(
            serde_json::to_string(&timers_json(&[], &BTreeMap::new(), &BTreeMap::new(), None))
                .unwrap(),
            "[]"
        );
    }

    #[test]
    fn the_last_result_is_spark_s_word_on_a_run_the_timer_started() {
        let by = |by: Option<&str>, state: State, status: Option<u32>| {
            last_result(&show::Service {
                started_by: by.map(str::to_string),
                state,
                last_status: status,
                ..show::Service::default()
            })
        };
        let timer = Some("its timer");
        assert_eq!(by(timer, State::Done, Some(0)), Some("ok"));
        assert_eq!(by(timer, State::Stopped, Some(0)), Some("ok"));
        assert_eq!(by(timer, State::Stopped, Some(256)), Some("failed"));
        assert_eq!(by(timer, State::Waiting, Some(1)), Some("failed"));
        assert_eq!(by(timer, State::Failed, None), Some("failed"));
        // It runs, or no end is known: no result.
        assert_eq!(by(timer, State::Running, Some(0)), None);
        assert_eq!(by(timer, State::Starting, None), None);
        assert_eq!(by(timer, State::Stopped, None), None);
        // Another started it last: it says nothing of the timer's run.
        assert_eq!(by(Some("a request"), State::Done, Some(0)), None);
        assert_eq!(by(Some("boot"), State::Failed, Some(1)), None);
        assert_eq!(by(None, State::Stopped, Some(0)), None);
    }

    #[test]
    fn timers_do_not_go_with_failed() {
        let err = list(&["--timers".to_string(), "--failed".to_string()]).unwrap_err();
        assert_eq!(err.message, "service list: --failed does not go with --timers");
        assert!(err.usage);
        let err = list(&["--timers".to_string(), "--all".to_string()]).unwrap_err();
        assert_eq!(err.message, "service list: unknown option `--all'");
    }
}
