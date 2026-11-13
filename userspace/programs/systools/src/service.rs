// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! service(8): the services Spark supervises, through its control socket
//! (spark(8)): `service list`, and a service's `status`, `start`, `stop`,
//! `restart`, `reload` and `logs`. `check`, `new`, `edit`, `secret`,
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
//! the ports it listens on now, and its last log lines from logd. The
//! `service-graph` boot test and the lab's `services/command` test cover
//! this file.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};

use spark_api::show::{self, HealthState, Holds, State};

use crate::args::{Arg, Args};
use crate::log;
use crate::power::{self, CONTROL};
use crate::proc::{self, Process};
use crate::servicefiles;
use crate::util::{self, Fail, fail, fail_with, usage};

const USAGE: &str = "Usage: service list [--failed] [--json]
       service NAME status [-n LINES]
       service NAME start | stop | restart [--no-wait] [-q]
       service NAME reload | --full-restart
       service NAME logs [log(1) options]
       service NAME enable | disable
       service check [NAME | FILE]... | new NAME | edit NAME
       service secret set NAME SECRET | list NAME | remove NAME SECRET
       service --status-all";

/// The log lines `status` shows unless `-n` says otherwise.
const STATUS_LINES: usize = 5;

/// The most process numbers `status` lists.
const PROCESSES_SHOWN: usize = 5;

/// The most names `list` gives of the services not running as asked.
const NAMES_SHOWN: usize = 3;

/// The kernel's tables of TCP sockets (radix-proc(5)).
const TCP_TABLES: [&str; 2] = ["/processes/net/tcp", "/processes/net/tcp6"];

/// Whether SIGINT came while a start, stop or restart was waiting.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Sends `request`, and fails with what Spark said if it refused.
fn ask(request: &str) -> Result<String, Fail> {
    let answer = power::request(request)?;
    match answer.strip_prefix("error: ") {
        Some(why) => Err(fail(why.trim_end().to_string())),
        None => Ok(answer),
    }
}

/// Spark's `show` document, of every service or of `name` alone; a name
/// Spark does not know is LSB's unknown service, 4.
fn shown(name: Option<&str>) -> Result<show::Show, Fail> {
    let request = name.map_or_else(|| String::from("show"), |name| format!("show {name}"));
    match ask(&request) {
        Ok(answer) => show::parse(&answer).map_err(fail),
        Err(failed) if failed.message.starts_with("no service is called") => {
            Err(fail_with(4, format!("{}: unrecognized service", name.unwrap_or(""))))
        }
        Err(failed) => Err(failed),
    }
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
        "logs" => {
            let mut items = vec!["-s".into(), name.into()];
            items.extend(options.iter().map(Into::into));
            log::main(&mut Args::new("log", items))
        }
        other => Err(usage(format!(
            "{name}: unknown command `{other}' (start, stop, restart, reload, status, logs, enable, \
             disable)"
        ))),
    }
}

/// Spark's answer without its `ok: ` and line end.
fn said(answer: &str) -> &str {
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
fn change(verb: &str, name: &str, options: &[String]) -> Result<i32, Fail> {
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
/// in seconds since the epoch.
struct Clock {
    now_ms: u64,
    wall: i64,
}

impl Clock {
    /// When `ms` (since boot) was, in local time: the time of day within
    /// the last day, and the date and time before.
    fn when(&self, ms: u64) -> String {
        let ago = self.now_ms.saturating_sub(ms) / 1000;
        let at = self.wall - i64::try_from(ago).unwrap_or(i64::MAX);
        let format = if ago < 86_400 { "%H:%M:%S" } else { "%b %d %H:%M" };
        util::local_time(at, format)
    }

    /// When `ms` was, and how long ago: "10:42:03, 2 min ago".
    fn since(&self, ms: u64) -> String {
        format!("{}, {} ago", self.when(ms), span(self.now_ms.saturating_sub(ms)))
    }
}

/// A length of time in words, rounded down: "4 s", "2 min", "3 h 5 min",
/// "2 days".
fn span(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        3600..86_400 if seconds % 3600 >= 60 => {
            format!("{} h {} min", seconds / 3600, seconds % 3600 / 60)
        }
        3600..86_400 => format!("{} h", seconds / 3600),
        _ if seconds < 2 * 86_400 => String::from("1 day"),
        _ => format!("{} days", seconds / 86_400),
    }
}

/// A duration in milliseconds as Spark's lines write one: "10.4 s".
fn tenths(ms: u64) -> String {
    format!("{}.{} s", ms / 1000, ms % 1000 / 100)
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

/// `service NAME status [-n LINES]`: what Spark says of the service, as
/// systemctl status lays it out, its processes and its last log lines;
/// 0 when it runs or is done, 3 otherwise (LSB).
fn status(name: &str, options: &[String]) -> Result<i32, Fail> {
    let lines = match options {
        [] => STATUS_LINES,
        [flag, count] if flag == "-n" || flag == "--lines" => {
            count.parse().map_err(|_| usage(format!("-n takes a number, not {count}")))?
        }
        _ => return Err(usage(format!("Usage: service {name} status [-n LINES]"))),
    };
    let document = shown(Some(name))?;
    let Some(service) = document.services.first() else {
        return Err(fail_with(4, format!("{name}: unrecognized service")));
    };
    let clock = Clock { now_ms: document.now_ms, wall: (util::now_ns() / 1_000_000_000) as i64 };
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
    print!("{}", status_text(service, &clock, &seen));
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
    if let Some(processes) = processes_line(seen.processes) {
        field(&mut text, "Processes", &processes);
    }
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
            ready_words(&service.ready_when)
        ),
        State::Pending => format!("activating (waiting for {})", service.waiting_for.join(", ")),
        State::Waiting => String::from("activating (waiting to start again)"),
        State::Stopping if service.pid.is_some() => format!("deactivating (stopping{pid})"),
        State::Stopping => String::from("deactivating (stopping what it left running)"),
        State::Failed => format!("failed (Spark gave up on it) since {since}"),
        State::Disabled => String::from("inactive (disabled)"),
        State::Stopped => format!("inactive (dead) since {since}"),
    }
}

/// What a service waits for to be ready, after "ready once".
fn ready_words(when: &str) -> &str {
    match when {
        "listening" => "it listens",
        "socket" => "its socket is made",
        "healthy" => "its health check passes",
        "ended" => "it has ended",
        other => other,
    }
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
    let memory = holds.memory_bytes.map_or_else(
        || String::from("no memory limit"),
        |bytes| format!("memory up to {}", util::human(bytes, false)),
    );
    lines.push(format!("{memory}; CPU weight {}", holds.cpu_weight));
    lines
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

/// `names` in words: "a", "a and b", "a, b and c".
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
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

/// `service list [--failed] [--json]`: each service, its state, whether
/// it starts at boot, since when and what it is; with `--failed`, those
/// not running as asked alone, and 1 if there are some.
fn list(options: &[String]) -> Result<i32, Fail> {
    let (mut failed_only, mut json) = (false, false);
    for option in options {
        match option.as_str() {
            "--failed" => failed_only = true,
            "--json" => json = true,
            other => return Err(usage(format!("service list: unknown option `{other}'"))),
        }
    }
    if json {
        print!("{}", ask("show")?);
        return Ok(0);
    }
    let mut document = shown(None)?;
    document.services.sort_by(|a, b| a.name.cmp(&b.name));
    let clock = Clock { now_ms: document.now_ms, wall: (util::now_ns() / 1_000_000_000) as i64 };
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

/// Whether `service` is not running as asked: given up on, waiting to
/// start again after an end, stopped for a reason, or failing its health
/// check.
fn not_as_asked(service: &show::Service) -> bool {
    match service.state {
        State::Failed | State::Waiting => true,
        State::Stopped => service.why.is_some(),
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
            if service.health.as_ref().is_some_and(|h| h.state == HealthState::Failing) {
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
    format!("Not running as asked: {}. `service {} status` says why.", and_list(&shown), names[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_api::show::{Health, Request};

    const CLOCK: Clock = Clock { now_ms: 600_000, wall: 1_800_000_000 };

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
                ..Holds::default()
            },
            ..show::Service::default()
        }
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
            "           memory up to 256M; CPU weight 100",
            "Processes: 2 (57 nginx, 58 nginx), 6.0M of memory, 1.2 s of CPU",
        ];
        for line in expected {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
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
            "           no memory limit; CPU weight 0",
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
    fn spans_read_as_words() {
        assert_eq!(span(4_999), "4 s");
        assert_eq!(span(120_000), "2 min");
        assert_eq!(span(3_600_000), "1 h");
        assert_eq!(span(11_100_000), "3 h 5 min");
        assert_eq!(span(90_000_000), "1 day");
        assert_eq!(span(200_000_000), "2 days");
        assert_eq!(tenths(10_449), "10.4 s");
    }

    #[test]
    fn ports_read_as_ranges() {
        assert_eq!(port_range("80"), Some((80, 1)));
        assert_eq!(port_range("8000-8010"), Some((8000, 11)));
        assert_eq!(port_range("9-8"), None);
        assert_eq!(port_range("http"), None);
    }

    #[test]
    fn the_list_lines_up_and_names_what_is_wrong() {
        let mut web = nginx();
        web.name = "lab-web".into();
        web.description = "Lab web".into();
        web.enabled = false;
        web.state = State::Failed;
        let mut sick = nginx();
        sick.name = "lab-sick".into();
        sick.health = Some(Health { state: HealthState::Failing, ..Health::default() });
        let fine = nginx();
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
        assert!([&web, &sick].iter().all(|s| not_as_asked(s)) && !not_as_asked(&fine));
        assert_eq!(
            not_as_asked_line(&["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]),
            "Not running as asked: a, b, c and 2 more. `service a status` says why."
        );
    }
}
