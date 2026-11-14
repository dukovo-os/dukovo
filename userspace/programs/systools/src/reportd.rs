// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! reportd(8): problem reports (ADR 0531).
//!
//! reportd keeps the crashes of the machine's programs, of its services'
//! panics and of the kernel, each as what a report of it needs and
//! nothing of the program's memory, and offers each new kind of crash
//! once: a line on the sign-in screens and a notice, which `report
//! crash` answers. It drafts a report as root, from what the person who
//! asks may read, redacts it, and sends exactly the bytes the person saw
//! and chose to send, to the reports server report.toml names. Nothing
//! else makes or sends a report: there is no automatic sending, and no
//! setting for it.
//!
//! - A thread reads the kernel's crash records (`/devices/crashes`, ADR
//!   0530), unwinds each from its stack and the program's files
//!   ([`unwind`]), and keeps it ([`store`]) by its signature
//!   ([`signature`]).
//! - A thread follows the log for the panics of services ([`panics`]).
//! - A thread, each half minute, takes in the kernel's saved crash dumps,
//!   tries the reports that wait ([`queue`]), and drops those 30 days
//!   old.
//! - The socket ([`socket`]) takes `report`'s requests
//!   ([`crate::reports::protocol`]).

mod intake;
mod panics;
mod parts;
mod queue;
mod record;
mod security;
mod server;
mod settings;
mod signature;
mod socket;
mod store;
mod unwind;

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::args::Args;
use crate::reports::protocol::{
    self, About, Answers, Crash, Drafted, SOCKET, Sent, Status, TEXT_MAX, Waiting,
};
use crate::reports::{draft, facts, redact};
use crate::util::{self, Fail};
use crate::{notifyd, sign_in_notices};
use parts::Asker;
use queue::{Entry, Queue};
use server::{Failure, Server};
use settings::Settings;
use store::{Kept, Store};

/// The kernel's crash records.
const RECORDS: &str = "/devices/crashes";
/// The kernel's saved crash dumps (ADR 0483), and reportd's list of those
/// it took in.
const DUMPS: &str = "/system/state/crash";
const DUMPS_TAKEN: &str = "dumps.json";
/// The name of reportd's sign-in notice.
const NOTICE: &str = "reportd";
/// The most crashes the sign-in notice names.
const NOTICE_LINES: usize = 3;
/// How often the queue and the dumps are looked at.
const EVERY: Duration = Duration::from_secs(30);
/// The most requests served at once.
const CONNECTIONS_MAX: usize = 8;

static STOP: AtomicBool = AtomicBool::new(false);

/// What reportd's threads share.
pub struct Reportd {
    store: Store,
    queue: Queue,
    settings: Mutex<Settings>,
    /// The program files read for unwinding, kept while unchanged.
    files: Mutex<unwind::Files>,
    /// Held while a report is sent, so that none is sent twice at once.
    sending: Mutex<()>,
    /// The requests being served.
    serving: AtomicUsize,
    /// The system's version, for its programs' crashes.
    version: String,
}

impl Reportd {
    fn new(dir: &Path, settings: Settings) -> Reportd {
        Reportd {
            store: Store::new(dir),
            queue: Queue::new(dir),
            settings: Mutex::new(settings),
            files: Mutex::new(unwind::Files::default()),
            sending: Mutex::new(()),
            serving: AtomicUsize::new(0),
            version: facts::gather().version,
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Takes in a crash record's `bytes`.
    fn take_record(&self, bytes: &[u8]) {
        let record = match record::Record::read(bytes) {
            Ok(record) => record,
            Err(why) => return log(4, &format!("a crash record that does not read: {}", why.0)),
        };
        if record.dropped > 0 {
            log(4, &format!("{} crashes came too fast to be kept", record.dropped));
        }
        let frames = {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            let frames = unwind::trace(&record, &mut files);
            for problem in files.problems() {
                log(6, &format!("cannot read {problem}, so its functions are not named"));
            }
            frames
        };
        let services = service_names();
        let service = services.contains(&record.domain).then_some(record.domain.as_str());
        let context = intake::Context {
            service,
            version: intake::version_of(record.path.as_deref(), &self.version),
            now: now(),
        };
        self.keep(intake::of_record(&record, &frames, &context));
    }

    /// Takes in a service's panic.
    fn take_panic(&self, panic: &panics::Panic) {
        let at = panic.at_ns / 1_000_000_000;
        let context = intake::Context {
            service: Some(&panic.service),
            version: None,
            now: if at > 0 { at } else { now() },
        };
        let mut kept = intake::of_panic(
            &panic.service,
            &panic.service,
            panic.user,
            &panic.location,
            &panic.message,
            &context,
        );
        kept.crash.id = store::crash_id(kept.crash.at, panic.process.unwrap_or(0));
        self.keep(kept);
    }

    /// Takes in the kernel's saved dumps not taken in before, of the last
    /// 30 days.
    fn take_dumps(&self) {
        let Ok(entries) = fs::read_dir(DUMPS) else { return };
        let taken_path = self.store.dir().join(DUMPS_TAKEN);
        let mut taken: BTreeSet<String> = fs::read_to_string(&taken_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let before = taken.len();
        let now = now();
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            let text = Path::new(&name).extension().is_some_and(|x| x.eq_ignore_ascii_case("txt"));
            if !text || taken.contains(&name) {
                continue;
            }
            taken.insert(name.clone());
            let Some(at) =
                entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| {
                    m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                })
            else {
                continue;
            };
            if now.saturating_sub(at) > store::DAYS_KEPT * 86_400 {
                continue;
            }
            let Some(report) = fs::read(entry.path()).ok().map(|bytes| panic_section(&bytes))
            else {
                continue;
            };
            if report.is_empty() {
                continue;
            }
            let context =
                intake::Context { service: None, version: Some(self.version.clone()), now };
            self.keep(intake::of_kernel(&report, at, &context));
        }
        if taken.len() != before {
            let text = serde_json::to_string(&taken).unwrap_or_default();
            if let Err(err) = store::write_whole(self.store.dir(), DUMPS_TAKEN, text.as_bytes()) {
                log(4, &format!("cannot note the dumps taken in: {err}"));
            }
        }
    }

    /// Keeps `new`, logs it, and offers it if its signature is new.
    fn keep(&self, new: Kept) {
        let now = now();
        let mut kept = match self.store.add(new, now) {
            Ok(kept) => kept,
            Err(err) => return log(3, &format!("cannot keep a crash: {err}")),
        };
        let crash = &kept.crash;
        log(4, &format!("{}: {}; report crash {} reports it", whose(crash), crash.what, crash.id));
        if kept.forgotten || crash.reported.is_some() {
            return;
        }
        match self.store.offer(&kept.crash.signature, now) {
            Ok(true) => {}
            Ok(false) => {
                kept.offered = true;
                if let Err(err) = self.store.put(&kept) {
                    log(3, &format!("cannot keep a crash: {err}"));
                }
                self.refresh_notice();
                let body = format!(
                    "{}: {}.\nreport crash reports it; report forget puts it away.",
                    whose(&kept.crash),
                    kept.crash.what
                );
                let _ =
                    notifyd::send("reportd", "A program crashed", &body, notifyd::Urgency::Normal);
            }
            Err(err) => log(3, &format!("cannot note a crash as offered: {err}")),
        }
    }

    /// Writes the sign-in notice of the crashes offered and not yet
    /// reported or put away.
    fn refresh_notice(&self) {
        let open: Vec<Kept> = self
            .store
            .all()
            .into_iter()
            .filter(|k| k.offered && !k.forgotten && k.crash.reported.is_none())
            .collect();
        let mut lines: Vec<String> = open
            .iter()
            .take(NOTICE_LINES)
            .map(|kept| {
                let crash = &kept.crash;
                let when = util::local_time(crash.at as i64, "%Y-%m-%d %H:%M");
                format!(
                    "{} stopped at {when}: {}. `report crash` reports it, `report forget` puts it away.",
                    whose(crash),
                    crash.what
                )
            })
            .collect();
        if open.len() > NOTICE_LINES {
            lines.push(format!(
                "{} more crashes: `report list` lists them.",
                open.len() - NOTICE_LINES
            ));
        }
        sign_in_notices::put(NOTICE, &lines);
    }

    /// The crashes `asker` may see and has not put away, newest first.
    fn crashes(&self, asker: Asker) -> Vec<Crash> {
        self.store
            .all()
            .into_iter()
            .filter(|k| !k.forgotten && (asker.admin || k.crash.user == asker.user))
            .map(|k| k.crash)
            .collect()
    }

    /// A draft about `about`, with `answers`, redacted.
    fn draft(
        &self,
        asker: Asker,
        about: &About,
        answers: &Answers,
        without_logs: bool,
    ) -> Result<Drafted, String> {
        let parts = parts::gather(about, asker, facts::gather(), &self.store)?;
        let text = draft::compose(&parts, answers, without_logs);
        let (text, cuts) = redact::Redactor::from_system().redact(&text);
        Ok(Drafted { text, cuts, has_logs: draft::has_logs(&parts) })
    }

    /// `text` redacted again, after the person edited it.
    fn redact(text: &str) -> Drafted {
        let (text, cuts) = redact::Redactor::from_system().redact(text);
        Drafted { text, cuts, has_logs: false }
    }

    /// Queues `text` from `asker` and tries it at once.
    fn send(
        &self,
        asker: Asker,
        text: &str,
        title: &str,
        security: bool,
        crash: Option<&str>,
    ) -> Result<Sent, String> {
        if text.trim().is_empty() {
            return Err("the report is empty".into());
        }
        if text.len() > TEXT_MAX {
            return Err(format!("the report is longer than {} KiB", TEXT_MAX / 1024));
        }
        if let Some(id) = crash {
            self.store
                .get(id)
                .filter(|k| asker.admin || k.crash.user == asker.user)
                .ok_or_else(|| format!("no crash {id} is kept that you may report"))?;
        }
        let bytes = if security {
            let keys = security::recipients_in(&self.settings().recipients)?;
            security::seal(text.as_bytes(), &keys)?
        } else {
            text.as_bytes().to_vec()
        };
        let title = notifyd::clean(&protocol::title_of(title), protocol::TITLE_MAX, false);
        let entry = self
            .queue
            .add(&bytes, asker.user, &title, security, crash.map(str::to_string), now())
            .map_err(|err| format!("cannot queue the report: {}", util::strip_os_error(&err)))?;
        log(6, &format!("report {} queued", entry.id));
        Ok(self.try_send(entry))
    }

    /// Tries to send `entry` now: sent, or why it waits.
    fn try_send(&self, mut entry: Entry) -> Sent {
        let _sending = self.sending.lock().unwrap_or_else(PoisonError::into_inner);
        // Another thread may have sent it while this one waited.
        if !self.queue.waiting().iter().any(|e| e.id == entry.id) {
            return Sent { id: entry.id, number: None, waiting: Some("sent already".into()) };
        }
        let sent = self
            .queue
            .bytes(&entry)
            .map_err(|err| Failure::Unreachable(format!("its file: {err}")))
            .and_then(|bytes| {
                let key = unhex(&entry.key).unwrap_or_default();
                Server::new(&self.settings()).send(&bytes, entry.security, &server::key_hash(&key))
            });
        let now = now();
        match sent {
            Ok(number) => {
                if let Err(err) = self.queue.sent(&entry, number, now) {
                    log(3, &format!("report {number} was sent but its key was not kept: {err}"));
                }
                if let Some(mut kept) = entry.crash.as_deref().and_then(|id| self.store.get(id)) {
                    kept.crash.reported = Some(number);
                    let _ = self.store.put(&kept);
                    self.refresh_notice();
                }
                log(6, &format!("report {} sent as number {number}", entry.id));
                Sent { id: entry.id, number: Some(number), waiting: None }
            }
            Err(failure) => {
                let why = failure.to_string();
                if !failure.passing() {
                    log(4, &format!("report {}: {why}", entry.id));
                }
                if let Err(err) = self.queue.failed(&mut entry, &why, now) {
                    log(3, &format!("report {}: {err}", entry.id));
                }
                Sent { id: entry.id, number: None, waiting: Some(why) }
            }
        }
    }

    /// Tries the reports due, and drops those tried for 30 days.
    fn send_due(&self) {
        let now = now();
        for entry in self.queue.waiting().into_iter().filter(|e| e.next <= now) {
            self.try_send(entry);
        }
        let dropped = self.queue.drop_old(now);
        for entry in &dropped {
            log(
                4,
                &format!(
                    "report {} ({}) was not sent in 30 days and is dropped",
                    entry.id, entry.title
                ),
            );
        }
    }

    /// The reports of `asker` that wait.
    fn waiting(&self, asker: Asker) -> Vec<Waiting> {
        self.queue
            .waiting()
            .into_iter()
            .filter(|e| asker.admin || e.user == asker.user)
            .map(|e| Waiting {
                id: e.id,
                title: e.title,
                since: e.since,
                tries: e.tries,
                error: e.error,
                next: e.next,
                security: e.security,
            })
            .collect()
    }

    /// Tries `asker`'s waiting reports now.
    fn retry(&self, asker: Asker) -> Vec<Sent> {
        self.queue
            .waiting()
            .into_iter()
            .filter(|e| asker.admin || e.user == asker.user)
            .map(|e| self.try_send(e))
            .collect()
    }

    /// Where `asker`'s sent reports stand, or report `number`, as the
    /// server says.
    fn status(&self, asker: Asker, number: Option<u64>) -> Result<Vec<Status>, String> {
        let sent: Vec<queue::Sent> = match number {
            Some(number) => vec![self.own_sent(asker, number)?],
            None => self
                .queue
                .all_sent()
                .into_iter()
                .filter(|s| asker.admin || s.user == asker.user)
                .collect(),
        };
        let server = Server::new(&self.settings());
        Ok(sent
            .into_iter()
            .map(|s| {
                let mut status = Status {
                    number: s.number,
                    title: s.title.clone(),
                    sent: s.sent,
                    status: None,
                    duplicate_of: None,
                    fixed_in: None,
                    reason: None,
                    error: None,
                    security: s.security,
                };
                match server.status(s.number, &s.key) {
                    Ok(standing) => {
                        status.status = Some(notifyd::clean(&standing.status, 20, false));
                        status.duplicate_of = standing.duplicate_of;
                        status.fixed_in = standing.fixed_in.map(|v| notifyd::clean(&v, 40, false));
                        status.reason = standing.reason.map(|r| notifyd::clean(&r, 400, false));
                    }
                    Err(failure) => status.error = Some(failure.to_string()),
                }
                status
            })
            .collect())
    }

    /// Deletes report `number` from the server, and its key here.
    fn delete(&self, asker: Asker, number: u64) -> Result<(), String> {
        let sent = self.own_sent(asker, number)?;
        Server::new(&self.settings()).delete(number, &sent.key).map_err(|f| f.to_string())?;
        self.queue.forget_sent(number);
        log(6, &format!("report {number} deleted from the server"));
        Ok(())
    }

    /// Report `number`, if `asker` sent it, or is an administrator.
    fn own_sent(&self, asker: Asker, number: u64) -> Result<queue::Sent, String> {
        self.queue
            .get_sent(number)
            .filter(|s| asker.admin || s.user == asker.user)
            .ok_or_else(|| format!("no report {number} sent from here is yours"))
    }

    /// Puts crash `id` away: listed, never offered again.
    fn forget(&self, asker: Asker, id: &str) -> Result<(), String> {
        let mut kept = self
            .store
            .get(id)
            .filter(|k| asker.admin || k.crash.user == asker.user)
            .ok_or_else(|| format!("no crash {id} is kept that is yours"))?;
        kept.forgotten = true;
        self.store.put(&kept).map_err(|err| format!("cannot put it away: {err}"))?;
        self.refresh_notice();
        Ok(())
    }
}

/// `nginx (the service web)`, `crash-test`.
fn whose(crash: &Crash) -> String {
    match &crash.service {
        Some(service) if *service != crash.program => {
            format!("{} (the service {service})", crash.program)
        }
        Some(service) => format!("the service {service}"),
        None if crash.program == "kernel" => "the kernel".into(),
        None => crash.program.clone(),
    }
}

/// The `== Panic` section of a kernel dump: its lines to the next section.
fn panic_section(dump: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(dump)
        .lines()
        .skip_while(|line| *line != "== Panic")
        .skip(1)
        .take_while(|line| !line.starts_with("== "))
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .take(64)
        .collect()
}

/// The names of Spark's services; none when Spark does not answer.
fn service_names() -> Vec<String> {
    crate::power::request("show")
        .ok()
        .and_then(|answer| spark_api::show::parse(&answer).ok())
        .map_or_else(Vec::new, |show| show.services.into_iter().map(|s| s.name).collect())
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

fn now() -> u64 {
    util::now_ns() / 1_000_000_000
}

/// A line of reportd's log at `level`.
fn log(level: u8, text: &str) {
    eprintln!("<{level}>reportd: {text}");
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    install_signals();
    let (settings, problems) = Settings::load();
    for problem in &problems {
        log(4, problem);
    }
    let dir = Path::new(store::DIR);
    let reportd = Arc::new(Reportd::new(dir, settings));
    reportd.store.make().map_err(|err| util::failed(&dir.display(), err))?;
    let listener = socket::listen()?;
    start_threads(&reportd);
    reportd.refresh_notice();
    log(
        6,
        &format!("keeping crashes in {}; reports go to {}", store::DIR, reportd.settings().server),
    );
    let served = socket::serve(&reportd, &listener, &STOP);
    let _ = fs::remove_file(SOCKET);
    served.map_err(|err| util::failed(&"reportd", err))?;
    Ok(0)
}

fn install_signals() {
    // SAFETY: setting a signal's disposition touches no memory. A client
    // that goes away must not end reportd: the write to it fails instead.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    // SAFETY: all zeros is a valid sigaction.
    let mut action: libc::sigaction = unsafe { util::zeroed() };
    action.sa_sigaction = on_term as extern "C" fn(libc::c_int) as *const () as usize;
    // SAFETY: the handler only stores to an atomic.
    unsafe { libc::sigaction(libc::SIGTERM, &raw const action, std::ptr::null_mut()) };
}

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// The threads that read crash records, follow the log for panics, and
/// look at the queue and the dumps.
fn start_threads(reportd: &Arc<Reportd>) {
    let records = Arc::clone(reportd);
    thread::spawn(move || read_records(&records));
    let panics = Arc::clone(reportd);
    thread::spawn(move || follow_panics(&panics));
    let timer = Arc::clone(reportd);
    thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            timer.take_dumps();
            timer.send_due();
            thread::sleep(EVERY);
        }
    });
}

/// Reads the kernel's crash records as they come.
fn read_records(reportd: &Reportd) {
    let mut buffer = vec![0u8; radix_abi::crash::RECORD_MAX as usize];
    while !STOP.load(Ordering::Relaxed) {
        let mut file = match fs::File::open(RECORDS) {
            Ok(file) => file,
            Err(err) => {
                log(
                    3,
                    &format!(
                        "cannot read crash records: {RECORDS}: {}",
                        util::strip_os_error(&err)
                    ),
                );
                thread::sleep(Duration::from_secs(60));
                continue;
            }
        };
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => reportd.take_record(&buffer[..n]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    log(3, &format!("cannot read crash records: {}", util::strip_os_error(&err)));
                    thread::sleep(Duration::from_secs(10));
                    break;
                }
            }
        }
    }
}

/// Follows the log for services' panics, from where it was when logd
/// restarts.
fn follow_panics(reportd: &Reportd) {
    let mut since: Option<u64> = None;
    let mut last = util::now_ns();
    while !STOP.load(Ordering::Relaxed) {
        let followed = panics::follow(since, |panic| {
            last = last.max(panic.at_ns + 1);
            reportd.take_panic(&panic);
        });
        if let Err(why) = followed {
            log(4, &format!("cannot follow the log for panics: {why}"));
        }
        since = Some(last);
        thread::sleep(Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dumps_panic_section_is_its_report_alone() {
        let dump = b"Radix crash dump\nRadix 0.16.0 on x86_64, boot 3\nPanic at x\n\n== Panic\n\
                     Radix panic on CPU 0 at 1.000000 s: boom\nBacktrace:\n  #0  0x1  radix::a+0x1\n\
                     \n== CPUs\n2 CPUs\n";
        assert_eq!(
            panic_section(dump),
            ["Radix panic on CPU 0 at 1.000000 s: boom", "Backtrace:", "  #0  0x1  radix::a+0x1"]
        );
    }

    #[test]
    fn a_crash_is_named_by_its_program_and_service() {
        let crash = |program: &str, service: Option<&str>| Crash {
            id: String::new(),
            kind: protocol::Kind::Signal,
            program: program.into(),
            version: None,
            service: service.map(str::to_string),
            user: 0,
            at: 0,
            what: String::new(),
            signature: String::new(),
            seen: 1,
            reported: None,
        };
        assert_eq!(whose(&crash("nginx", Some("web"))), "nginx (the service web)");
        assert_eq!(whose(&crash("crash-test", Some("crash-test"))), "the service crash-test");
        assert_eq!(whose(&crash("ls", None)), "ls");
        assert_eq!(unhex("00ff10").unwrap(), [0, 255, 16]);
        assert!(unhex("0g").is_none());
    }
}
