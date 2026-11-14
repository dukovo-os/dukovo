// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! timerd(8), which keeps time for Spark and for people (ADR 0478): it
//! starts the services whose files have a `[timer]` at their times, and
//! runs the lines of people's crontabs and their `at` jobs, which authd(8)
//! starts as the person ([`crate::jobs`]).
//!
//! It is a service of its own, not a part of Spark, because calendar
//! times need the time zone, which a program with the C library reads and
//! Spark does not; and because a mistake in the arithmetic stops one
//! timer and not the machine.
//!
//! - [`plan`] works out when each timer runs next, and watches the clock;
//!   [`next`] finds the next time a calendar matches.
//! - [`crontab`] reads a crontab, [`protocol`] is what crontab(1), at(1)
//!   and service(8) say on the socket, `/system/live/timerd/socket`, and
//!   [`serve`] answers.
//! - [`state`] is what timerd keeps in `/system/state/timerd`; [`run`]
//!   runs one job.
//!
//! The loop sleeps until the next time, a minute at most, and on each
//! wake compares the clock with the monotonic clock. A clock that moved
//! forward leaves the times it passed due, and each timer runs once for
//! them; a clock that moved back runs no time that already ran again.
//! Each move is logged, as `<level>timerd: ...` lines in the system's
//! log (`log timerd`). A time that comes while the timer's last run
//! still runs is skipped, and the log says so.
//!
//! Boot tests `timers` and `crontab` cover it.

pub(crate) mod crontab;
mod next;
mod plan;
pub(crate) mod protocol;
mod run;
mod serve;
pub(crate) mod show;
pub(crate) mod state;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use manifest::service::{self as service_file, Service};
use spark_api::show::State;

use self::crontab::{Crontab, When};
use self::plan::{Timing, Watch, spread_delay};
use self::state::{AtJob, Store};
use crate::args::Args;
use crate::jobs::Job;
use crate::util::{self, Fail};

/// The folder of the socket.
const SOCKET_DIR: &str = "/system/live/timerd";

/// The longest timerd sleeps, in seconds: the clock is checked at least
/// this often.
const SLEEP_MAX: i64 = 60;

/// How often the service files are read again, in seconds.
const RESCAN: i64 = 60;

/// The most connections served at once.
const CONNECTIONS_MAX: usize = 16;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn log(level: u8, text: &str) {
    eprintln!("<{level}>timerd: {text}");
}

/// What a timer runs.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    /// A service, by its name.
    Service(String),
    /// A line of a person's crontab.
    Line { user: String, line: usize },
    /// A job of `at`.
    At(u32),
}

impl Key {
    /// What the log calls it, and its record of runs is named by.
    fn label(&self) -> String {
        match self {
            Key::Service(name) => name.clone(),
            Key::Line { user, line } => format!("crontab {user} (line {line})"),
            Key::At(id) => format!("at {id}"),
        }
    }

    /// The name of its record of runs.
    fn record(&self) -> Option<String> {
        match self {
            Key::Service(_) => None,
            Key::Line { user, line } => Some(format!("crontab.{user}.{line}")),
            Key::At(id) => Some(format!("at.{id}")),
        }
    }
}

/// One timer: when it runs next, and whether a run is going on.
struct Entry {
    key: Key,
    /// When it runs; an `at` job has none, only its time.
    timing: Option<Timing>,
    last_run: Option<i64>,
    next: Option<i64>,
    running: bool,
}

/// What the lock holds.
struct Inner {
    entries: Vec<Entry>,
    crontabs: BTreeMap<String, Crontab>,
    at_jobs: BTreeMap<u32, AtJob>,
    last_runs: BTreeMap<String, i64>,
    watch: Watch,
    /// Service files already said to be wrong, so that the log says it
    /// once and not at each reading.
    complained: BTreeSet<String>,
    /// When the service files were last read, by the monotonic clock, so
    /// that a clock set back does not put the next reading off.
    scanned: i64,
}

/// timerd's state and its timers.
pub struct Daemon {
    store: Store,
    seed: String,
    /// When the machine started, seconds since 1970 by the clock as it
    /// reads now: a move of the clock moves it too, so that a time counted
    /// from the start (`every`, `after_boot`) stays where it was.
    boot: AtomicI64,
    host: String,
    inner: Mutex<Inner>,
    /// The two ends of a socket pair: a byte written to the first wakes
    /// the loop ([`Daemon::wake`]), which then works out its sleep again
    /// from the timers as a request left them. Without it, the loop wakes
    /// a minute at most after a request.
    waker: Option<(UnixStream, UnixStream)>,
}

/// The socket pair of [`Daemon::waker`], or none when it cannot be made.
fn waker() -> Option<(UnixStream, UnixStream)> {
    let made = UnixStream::pair().and_then(|(tell, hear)| {
        tell.set_nonblocking(true)?;
        hear.set_nonblocking(true)?;
        Ok((tell, hear))
    });
    made.inspect_err(|err| {
        log(4, &format!("requests change the timers within a minute, not at once: {err}"));
    })
    .ok()
}

/// The wall clock and the monotonic clock, in seconds.
fn clocks() -> (i64, i64) {
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let monotonic = i64::try_from(util::monotonic_ns() / 1_000_000_000).unwrap_or(i64::MAX);
    (wall, monotonic)
}

/// `timerd`: keeps time for Spark and for people.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    install_signals();
    let store = Store::open(Path::new(state::DIR)).map_err(|err| util::failed(&state::DIR, err))?;
    let daemon = Arc::new(Daemon::new(store));
    daemon.load();
    let listener = listen()?;
    log(6, &format!("keeping time; socket {}", protocol::SOCKET));
    let served = serve_loop(&daemon, &listener);
    let _ = fs::remove_file(protocol::SOCKET); // the next start makes it again
    served.map_err(|err| util::failed(&"timerd", err))?;
    Ok(0)
}

fn install_signals() {
    // SAFETY: setting a signal's disposition touches no memory. A client
    // that goes away must not end timerd: the write to it fails instead.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    // SAFETY: all zeros is a valid sigaction: numbers, a signal set and a
    // function address.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
    // SAFETY: `action` is valid for the call, and on_term only stores a flag.
    unsafe { libc::sigaction(libc::SIGTERM, &raw const action, std::ptr::null_mut()) };
}

fn listen() -> Result<UnixListener, Fail> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(SOCKET_DIR).map_err(|err| util::failed(&SOCKET_DIR, err))?;
    let _ = fs::remove_file(protocol::SOCKET); // a socket left by an earlier run
    let listener =
        UnixListener::bind(protocol::SOCKET).map_err(|err| util::failed(&protocol::SOCKET, err))?;
    // Anyone may connect: who asks is the kernel's account of the peer,
    // and each request is allowed or refused by it.
    for (path, mode) in [(SOCKET_DIR, 0o755), (protocol::SOCKET, 0o666)] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|err| util::failed(&path, err))?;
    }
    listener.set_nonblocking(true).map_err(|err| util::failed(&protocol::SOCKET, err))?;
    Ok(listener)
}

/// Waits for connections, for a request's end and for the next time, and
/// runs what is due.
fn serve_loop(daemon: &Arc<Daemon>, listener: &UnixListener) -> std::io::Result<()> {
    let serving = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // A negative descriptor is one poll does not wait on.
    let woken = daemon.waker.as_ref().map_or(-1, |(_, hear)| hear.as_raw_fd());
    while !STOP.load(Ordering::Relaxed) {
        let wait = daemon.tick();
        let mut fds = [
            libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: woken, events: libc::POLLIN, revents: 0 },
        ];
        let ms = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: `fds` holds two valid pollfds, and its length is 2.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
        if ready < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if fds[1].revents != 0
            && let Some((_, hear)) = &daemon.waker
        {
            let (mut hear, mut bytes): (&UnixStream, _) = (hear, [0; 64]);
            while hear.read(&mut bytes).is_ok_and(|read| read > 0) {}
        }
        while let Ok((stream, _)) = listener.accept() {
            serve::accept(daemon, &serving, CONNECTIONS_MAX, stream);
        }
    }
    Ok(())
}

impl Daemon {
    fn new(store: Store) -> Daemon {
        let seed = store.seed();
        let (wall, monotonic) = clocks();
        Daemon {
            store,
            seed,
            boot: AtomicI64::new(wall - monotonic),
            host: util::hostname(),
            inner: Mutex::new(Inner {
                entries: Vec::new(),
                crontabs: BTreeMap::new(),
                at_jobs: BTreeMap::new(),
                last_runs: BTreeMap::new(),
                watch: Watch::new(wall, monotonic),
                complained: BTreeSet::new(),
                scanned: 0,
            }),
            waker: waker(),
        }
    }

    /// When the machine started, by the clock as it reads now.
    fn boot(&self) -> i64 {
        self.boot.load(Ordering::Relaxed)
    }

    /// Wakes the loop, so that it works out its sleep again.
    fn wake(&self) {
        if let Some((tell, _)) = &self.waker {
            let mut tell: &UnixStream = tell;
            // A full socket already holds a wake that is not yet read.
            let _ = tell.write(&[1]);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A thread that panicked holding the lock leaves the tables as
        // they were: they are consistent between statements.
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads what the state folder holds, and the service files.
    fn load(self: &Arc<Self>) {
        let (now, monotonic) = clocks();
        {
            let mut inner = self.lock();
            inner.last_runs = self.store.last_runs();
            for user in self.store.crontab_users() {
                let Some(text) = self.store.crontab(&user) else { continue };
                match crontab::parse(&text) {
                    Ok(crontab) => self.put_crontab(&mut inner, &user, crontab, now, true),
                    Err(mistake) => log(3, &format!("crontab {user}: {mistake}")),
                }
            }
            inner.at_jobs = self.store.at_jobs();
            let jobs: Vec<AtJob> = inner.at_jobs.values().cloned().collect();
            for job in jobs {
                // A job whose time passed while the machine was off runs
                // as soon as timerd starts.
                inner.entries.push(Entry {
                    key: Key::At(job.id),
                    timing: None,
                    last_run: None,
                    next: Some(job.time),
                    running: false,
                });
            }
            self.scan_services(&mut inner, now, monotonic, true);
        }
        self.run_reboot_lines(now);
    }

    /// Runs the `@reboot` lines, once for each start of the machine: a
    /// timerd started again within one does not run them again.
    fn run_reboot_lines(self: &Arc<Self>, now: i64) {
        if self
            .store
            .reboot_run()
            .is_some_and(|boot| (boot - self.boot()).abs() <= plan::STEP_TOLERANCE * 2)
        {
            return;
        }
        if let Err(err) = self.store.save_reboot_run(self.boot()) {
            log(3, &format!("cannot keep that the lines of @reboot ran: {err}"));
        }
        let lines: Vec<(String, usize)> = {
            let inner = self.lock();
            inner
                .crontabs
                .iter()
                .flat_map(|(user, tab)| {
                    tab.entries
                        .iter()
                        .filter(|entry| entry.when == When::Reboot)
                        .map(move |entry| (user.clone(), entry.line))
                })
                .collect()
        };
        for (user, line) in lines {
            self.start_line(Key::Line { user, line }, now);
        }
    }

    /// Makes the entries of `user`'s crontab from `crontab`.
    fn put_crontab(
        &self,
        inner: &mut Inner,
        user: &str,
        crontab: Crontab,
        now: i64,
        loading: bool,
    ) {
        inner.entries.retain(|e| !matches!(&e.key, Key::Line { user: u, .. } if u == user));
        let zone = crontab.variable("CRON_TZ").filter(|z| !z.is_empty()).map(str::to_string);
        for entry in &crontab.entries {
            let When::At(calendar) = entry.when else { continue };
            let key = Key::Line { user: user.to_string(), line: entry.line };
            let timing = Timing {
                calendars: vec![calendar],
                every: None,
                after_boot: None,
                delay: 0,
                catch_up: false,
                zone: zone.clone(),
            };
            let last_run = key.record().and_then(|r| self.store.run(&r)).and_then(|run| run.start);
            let next = timing.next(last_run, now, self.boot(), loading);
            inner.entries.push(Entry { key, timing: Some(timing), last_run, next, running: false });
        }
        inner.crontabs.insert(user.to_string(), crontab);
    }

    /// Reads the service files again, at `now` by the clock and
    /// `monotonic` by the monotonic clock: a service with a timer has an
    /// entry, which keeps its last run; one without, or whose file does
    /// not read, has none.
    fn scan_services(&self, inner: &mut Inner, now: i64, monotonic: i64, loading: bool) {
        inner.scanned = monotonic;
        let (services, wrong) = read_services(Path::new(crate::servicefiles::DIR));
        for (name, why) in wrong {
            if inner.complained.insert(format!("{name}: {why}")) {
                log(4, &format!("{name}: its file does not read, so its timer is not kept: {why}"));
            }
        }
        let found: Vec<(String, service_file::Timer)> = services
            .into_iter()
            .filter(|(_, service)| service.enabled)
            .filter_map(|(name, service)| service.timer.map(|timer| (name, timer)))
            .collect();
        inner.entries.retain(|e| match &e.key {
            Key::Service(name) => found.iter().any(|(n, _)| n == name),
            _ => true,
        });
        for (name, timer) in found {
            let delay = spread_delay(&self.seed, &name, timer.spread);
            let timing = Timing::from_timer(&timer, delay);
            let key = Key::Service(name.clone());
            let last_run = inner.last_runs.get(&name).copied();
            if let Some(entry) = inner.entries.iter_mut().find(|e| e.key == key) {
                if entry.timing.as_ref() != Some(&timing) {
                    entry.next = timing.next(entry.last_run, now, self.boot(), false);
                    entry.timing = Some(timing);
                }
                continue;
            }
            let next = timing.next(last_run, now, self.boot(), loading);
            if next.is_none() {
                log(4, &format!("{name}: no time of its timer comes within five years"));
            }
            inner.entries.push(Entry { key, timing: Some(timing), last_run, next, running: false });
        }
    }

    /// Checks the clock, starts what is due, and says how long to wait
    /// for the next time.
    fn tick(self: &Arc<Self>) -> Duration {
        let (wall, monotonic) = clocks();
        let (due, wait) = self.due_at(wall, monotonic);
        for (key, time) in due {
            self.start(key, time);
        }
        wait
    }

    /// What is due at the clocks' readings `wall` and `monotonic`: each
    /// timer whose time has come and is not running, marked as running,
    /// with the time that came; and how long until the next time.
    fn due_at(&self, wall: i64, monotonic: i64) -> (Vec<(Key, i64)>, Duration) {
        let mut due = Vec::new();
        let mut inner = self.lock();
        self.check_clock(&mut inner, wall, monotonic);
        if monotonic - inner.scanned >= RESCAN {
            self.scan_services(&mut inner, wall, monotonic, false);
        }
        for entry in &mut inner.entries {
            let Some(time) = entry.next.filter(|next| *next <= wall) else { continue };
            entry.next = None;
            if entry.running {
                log(
                    5,
                    &format!(
                        "{}: its time came while its last run still runs: skipped",
                        entry.key.label()
                    ),
                );
                entry.next = entry
                    .timing
                    .as_ref()
                    .and_then(|t| t.next(Some(time), wall, self.boot(), false));
            } else {
                entry.running = true;
                due.push((entry.key.clone(), time));
            }
        }
        let soonest = inner.entries.iter().filter_map(|e| e.next).min();
        let wait = soonest.map_or(SLEEP_MAX, |next| (next - wall).clamp(1, SLEEP_MAX));
        (due, Duration::from_secs(u64::try_from(wait).unwrap_or(1)))
    }

    /// Compares the clock, `wall`, with the monotonic clock, and handles a
    /// move of it.
    fn check_clock(&self, inner: &mut Inner, wall: i64, monotonic: i64) {
        if let Some(moved) = inner.watch.check(wall, monotonic) {
            self.clock_moved(inner, moved, wall);
        }
    }

    /// What a move of the clock by `moved` seconds does to the timers: the
    /// machine's start moves with it; a move forward leaves the times it
    /// passed due, and each runs once; a move back works out each next time
    /// again, from the last run, so that no time already run runs again.
    fn clock_moved(&self, inner: &mut Inner, moved: i64, wall: i64) {
        self.boot.fetch_add(moved, Ordering::Relaxed);
        if moved > 0 {
            log(
                4,
                &format!(
                    "the clock moved forward by {}: each timer whose time it passed runs once",
                    util::elapsed(moved.unsigned_abs())
                ),
            );
            return;
        }
        log(
            4,
            &format!(
                "the clock moved back by {}: no time that ran runs again",
                util::elapsed(moved.unsigned_abs())
            ),
        );
        for entry in &mut inner.entries {
            if let (Some(timing), false) = (&entry.timing, entry.running) {
                entry.next = timing.next(entry.last_run, wall, self.boot(), false);
            }
        }
    }

    /// Starts what `key` runs, on a thread, for the time `time`.
    fn start(self: &Arc<Self>, key: Key, time: i64) {
        let daemon = Arc::clone(self);
        thread::spawn(move || match key.clone() {
            Key::Service(name) => daemon.run_service(&key, &name, time),
            Key::Line { .. } | Key::At(_) => daemon.run_job(&key, time),
        });
    }

    /// Starts a crontab line at once, as `@reboot` has it.
    fn start_line(self: &Arc<Self>, key: Key, now: i64) {
        self.start(key, now);
    }

    /// Asks Spark to start `name` for its timer, unless its last start is
    /// still running.
    fn run_service(&self, key: &Key, name: &str, time: i64) {
        let busy = crate::service::shown(Some(name)).is_ok_and(|show| {
            show.services.iter().any(|s| {
                s.name == name
                    && matches!(
                        s.state,
                        State::Pending
                            | State::Starting
                            | State::Running
                            | State::Waiting
                            | State::Stopping
                    )
            })
        });
        if busy {
            log(5, &format!("{name}: its time came while its last run still runs: skipped"));
            self.finished(key, None, time);
            return;
        }
        match crate::service::ask(&format!("start {name} --by timer")) {
            Ok(_) => {
                log(6, &format!("{name}: started by its timer"));
                self.finished(key, Some(clocks().0), time);
            }
            Err(failed) => {
                log(3, &format!("{name}: its timer could not start it: {}", failed.message));
                self.finished(key, None, time);
            }
        }
    }

    /// Records the end of what `key` ran: `ran`, the time it started, if
    /// it did; and works out its next time.
    fn finished(&self, key: &Key, ran: Option<i64>, time: i64) {
        let (wall, _) = clocks();
        let mut inner = self.lock();
        let Some(at) = inner.entries.iter().position(|e| e.key == *key) else { return };
        let entry = &mut inner.entries[at];
        entry.running = false;
        if let Some(ran) = ran {
            entry.last_run = Some(ran);
        }
        let fence = entry.last_run.map_or(time, |last| last.max(time.min(wall)));
        entry.next =
            entry.timing.as_ref().and_then(|t| t.next(Some(fence), wall, self.boot(), false));
        if let (Key::Service(name), Some(ran)) = (key, ran) {
            inner.last_runs.insert(name.clone(), ran);
            if let Err(err) = self.store.save_last_runs(&inner.last_runs) {
                log(3, &format!("cannot keep the last run of {name}: {err}"));
            }
        }
    }

    /// The job `key` names, for authd to run.
    fn job_of(&self, key: &Key) -> Option<(Job, Option<String>)> {
        let inner = self.lock();
        match key {
            Key::Line { user, line } => {
                let tab = inner.crontabs.get(user)?;
                let entry = tab.entries.iter().find(|e| e.line == *line)?;
                let mut environment: Vec<String> =
                    tab.variables.iter().map(|(n, v)| format!("{n}={v}")).collect();
                environment.push("SHELL=/bin/sh".into());
                let home = tab
                    .variable("HOME")
                    .map(str::to_string)
                    .or_else(|| util::user_id(user).and_then(util::user_home));
                let job = Job {
                    owner: user.clone(),
                    label: format!("crontab {user}"),
                    folder: home.unwrap_or_else(|| "/".into()),
                    environment,
                    command: run::with_input(&entry.command, entry.input.as_deref()),
                };
                Some((job, tab.variable("MAILTO").filter(|m| !m.is_empty()).map(str::to_string)))
            }
            Key::At(id) => {
                let at = inner.at_jobs.get(id)?;
                let job = Job {
                    owner: at.owner.clone(),
                    label: format!("at {id}"),
                    folder: at.folder.clone(),
                    environment: at.environment.clone(),
                    command: at.command.clone(),
                };
                Some((job, None))
            }
            Key::Service(_) => None,
        }
    }

    /// Runs a person's job through authd, keeps its record, and mails its
    /// output when `MAILTO` says to. authd counts a job that waited on its
    /// owner for their next sign-in.
    fn run_job(&self, key: &Key, time: i64) {
        let Some((job, mail_to)) = self.job_of(key) else {
            self.finished(key, None, time);
            return;
        };
        let label = key.label();
        log(6, &format!("{label}: runs for {}", job.owner));
        let (run, not_run) = run::execute(&job, || clocks().0);
        match (&not_run, run.status) {
            (Some(why), _) => {
                let words = run::not_run_words(why);
                log(4, &format!("{label}: did not run: {words}"));
            }
            (None, Some(status)) => log(6, &format!("{label}: ended with status {status}")),
            (None, None) => log(
                4,
                &format!(
                    "{label}: ended, and its status is not known: {}",
                    run.not_run.as_deref().unwrap_or("")
                ),
            ),
        }
        if let Some(record) = key.record()
            && let Err(err) = self.store.save_run(&record, &run)
        {
            log(3, &format!("cannot keep the record of {label}: {err}"));
        }
        if let Some(to) = mail_to
            && !run.output.is_empty()
        {
            self.mail(&job, &to, &run.output);
        }
        if let Key::At(id) = key {
            self.remove_at(*id);
            return;
        }
        let ran = run.start;
        self.finished(key, ran, time);
    }

    /// Mails `output`, a job's, to `to`.
    fn mail(&self, job: &Job, to: &str, output: &str) {
        let message = run::mail_message(&job.owner, &self.host, &job.command, to, output);
        match crate::mail::send(None, &[to.to_string()], &message) {
            Ok(()) => log(6, &format!("{}: its output was mailed to {to}", job.label)),
            Err(err) if err.kind == crate::mail::Kind::NoServer => {
                log(
                    5,
                    &format!(
                        "{}: MAILTO is set, and the mail settings name no server: {err}",
                        job.label
                    ),
                );
            }
            Err(err) => {
                log(4, &format!("{}: its output could not be mailed to {to}: {err}", job.label));
            }
        }
    }

    /// Drops `at` job `id` after it ran, or at its owner's request.
    fn remove_at(&self, id: u32) {
        let mut inner = self.lock();
        inner.at_jobs.remove(&id);
        inner.entries.retain(|e| e.key != Key::At(id));
        self.store.remove_at_job(id);
    }
}

/// What [`read_services`] finds: the files that read, and the names of
/// those that do not, with why.
pub(crate) type Services = (Vec<(String, Service)>, Vec<(String, String)>);

/// The service files in `dir` that read, by name, with their overrides
/// laid over them, and the names of those that do not, with why.
pub(crate) fn read_services(dir: &Path) -> Services {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".toml").map(str::to_string))
        .collect();
    names.sort();
    let (mut services, mut wrong) = (Vec::new(), Vec::new());
    for name in names {
        match read_service(dir, &name) {
            Ok(Some(service)) => services.push((name, service)),
            Ok(None) => {}
            Err(why) => wrong.push((name, why)),
        }
    }
    (services, wrong)
}

/// The service `name` of the files in `dir`, with its override, if the
/// file is there and reads.
fn read_service(dir: &Path, name: &str) -> Result<Option<Service>, String> {
    let text = match fs::read_to_string(dir.join(format!("{name}.toml"))) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(util::strip_os_error(&err)),
    };
    let over =
        match fs::read_to_string(dir.join(service_file::OVERRIDES).join(format!("{name}.toml"))) {
            Ok(text) => Some(service_file::Override::from_toml(&text).map_err(|e| e.to_string())?),
            Err(_) => None,
        };
    service_file::from_toml_over(&text, over.as_ref()).map(Some).map_err(|e| e.to_string())
}

impl Timing {
    /// The timing of a service file's `[timer]`, put off by `delay`.
    fn from_timer(timer: &service_file::Timer, delay: i64) -> Timing {
        Timing {
            calendars: timer.at.clone(),
            every: timer.every,
            after_boot: timer.after_boot,
            delay,
            catch_up: timer.catch_up,
            zone: timer.time_zone.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::state::tests::Temp;
    use super::*;

    /// A daemon on a folder of its own, whose machine started at 1000.
    fn daemon(dir: &Temp) -> Daemon {
        let mut daemon = Daemon::new(Store::open(&dir.0).unwrap());
        daemon.boot = AtomicI64::new(1000);
        let mut inner = daemon.lock();
        inner.scanned = i64::MAX / 2;
        inner.watch = Watch::new(2000, 1000);
        drop(inner);
        daemon
    }

    #[test]
    fn a_wake_is_heard_by_the_loop() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        let (_, hear) = daemon.waker.as_ref().unwrap();
        let mut hear: &UnixStream = hear;
        let mut bytes = [0; 8];
        assert!(hear.read(&mut bytes).is_err(), "nothing to hear before a wake");
        daemon.wake();
        daemon.wake();
        assert_eq!(hear.read(&mut bytes).unwrap(), 2);
    }

    fn every(seconds: u64) -> Timing {
        Timing {
            calendars: Vec::new(),
            every: Some(seconds),
            after_boot: None,
            delay: 0,
            catch_up: true,
            zone: None,
        }
    }

    fn service_entry(
        name: &str,
        timing: Timing,
        last_run: Option<i64>,
        next: Option<i64>,
    ) -> Entry {
        Entry {
            key: Key::Service(name.into()),
            timing: Some(timing),
            last_run,
            next,
            running: false,
        }
    }

    fn keys(due: &[(Key, i64)]) -> Vec<String> {
        due.iter().map(|(key, _)| key.label()).collect()
    }

    #[test]
    fn a_time_that_comes_is_due_once_and_the_wait_is_until_the_next() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        daemon.lock().entries.push(service_entry("a", every(60), Some(1990), Some(2050)));
        daemon.lock().entries.push(service_entry("b", every(60), None, Some(2030)));
        let (due, wait) = daemon.due_at(2000, 1000);
        assert!(due.is_empty());
        assert_eq!(wait, Duration::from_secs(30));
        let (due, wait) = daemon.due_at(2031, 1031);
        assert_eq!(keys(&due), ["b"]);
        assert_eq!(due[0].1, 2030);
        // It is running, and has no time of its own until it ends.
        assert_eq!(wait, Duration::from_secs(19));
        assert!(daemon.due_at(2032, 1032).0.is_empty());
        daemon.finished(&Key::Service("b".into()), Some(2031), 2030);
        let inner = daemon.lock();
        let b = inner.entries.iter().find(|e| e.key == Key::Service("b".into())).unwrap();
        assert!(!b.running);
        assert_eq!(b.last_run, Some(2031));
    }

    #[test]
    fn a_time_that_comes_while_the_last_run_runs_is_skipped_and_the_next_is_worked_out() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        daemon.lock().entries.push(service_entry("slow", every(20), Some(2000), Some(2020)));
        assert_eq!(keys(&daemon.due_at(2020, 1020).0), ["slow"]);
        // The run goes on; its next time comes at 2040 and passes.
        daemon.lock().entries[0].next = Some(2040);
        let (due, wait) = daemon.due_at(2041, 1041);
        assert!(due.is_empty());
        let inner = daemon.lock();
        assert_eq!(inner.entries[0].next, Some(2060));
        assert!(inner.entries[0].running);
        assert_eq!(wait, Duration::from_secs(19));
    }

    #[test]
    fn a_clock_moved_forward_runs_each_timer_it_passed_once() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        daemon.lock().entries.push(service_entry("hourly", every(3600), Some(2000), Some(5600)));
        daemon.lock().entries.push(service_entry(
            "never-due",
            every(86_400),
            Some(2000),
            Some(88_400),
        ));
        // Two hours on the wall clock, 30 seconds on the monotonic one.
        let (due, _) = daemon.due_at(2000 + 7200, 1030);
        assert_eq!(keys(&due), ["hourly"]);
        daemon.finished(&Key::Service("hourly".into()), Some(9200), 5600);
        let inner = daemon.lock();
        assert_eq!(inner.entries[0].next, Some(9200 + 3600));
    }

    #[test]
    fn a_clock_moved_back_does_not_run_a_start_again_and_waits_a_whole_every() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        daemon.lock().entries.push(service_entry("hourly", every(3600), Some(2000), Some(5600)));
        // An hour back on the wall clock.
        let (due, wait) = daemon.due_at(2000 - 3600 + 30, 1030);
        assert!(due.is_empty());
        // The last start counts as now, not as an hour ahead.
        let next = daemon.lock().entries[0].next;
        assert_eq!(next, Some(2000 - 3600 + 30 + 3600));
        assert_eq!(wait, Duration::from_secs(60));
    }

    #[test]
    fn a_clock_set_back_years_moves_the_start_and_an_every_counted_from_it() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        daemon.lock().entries.push(service_entry("tick", every(20), None, Some(1020)));
        // Years back, as a clock set by hand to 2020 goes: the machine
        // started 1000 seconds ago by the clock as it reads now, so a timer
        // of every 20 seconds that never ran is due.
        let back = 200_000_000;
        let (due, _) = daemon.due_at(2000 - back + 30, 1030);
        assert_eq!(daemon.boot(), 1000 - back);
        assert_eq!(keys(&due), ["tick"]);
    }

    #[test]
    fn an_at_job_whose_time_passed_runs_as_soon_as_timerd_starts() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        daemon.lock().entries.push(Entry {
            key: Key::At(3),
            timing: None,
            last_run: None,
            next: Some(500),
            running: false,
        });
        let (due, _) = daemon.due_at(2000, 1000);
        assert_eq!(due, vec![(Key::At(3), 500)]);
    }

    #[test]
    fn a_crontab_line_has_its_time_its_zone_and_its_job() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        let crontab = crontab::parse(
            "CRON_TZ=UTC\nMAILTO=a@x.org\n*/5 * * * * echo hi%there\n@reboot true\n",
        )
        .unwrap();
        {
            let mut inner = daemon.lock();
            daemon.put_crontab(&mut inner, "alice", crontab, 2000, true);
            // Only the timed line is an entry; @reboot has none.
            assert_eq!(inner.entries.len(), 1);
            assert_eq!(inner.entries[0].timing.as_ref().unwrap().zone.as_deref(), Some("UTC"));
        }
        let (job, mail) = daemon.job_of(&Key::Line { user: "alice".into(), line: 3 }).unwrap();
        assert_eq!(job.owner, "alice");
        assert_eq!(job.label, "crontab alice");
        assert_eq!(job.command, "{ echo hi\n} <<'DUKOVO_INPUT'\nthere\nDUKOVO_INPUT\n");
        assert!(job.environment.contains(&"MAILTO=a@x.org".to_string()));
        assert!(job.environment.contains(&"SHELL=/bin/sh".to_string()));
        assert_eq!(mail.as_deref(), Some("a@x.org"));
    }

    #[test]
    fn the_spread_of_a_service_timer_is_kept_across_scans() {
        let dir = Temp::new();
        let daemon = daemon(&dir);
        let first = spread_delay(&daemon.seed, "backup", 3600);
        assert_eq!(first, spread_delay(&daemon.seed, "backup", 3600));
        let timer = service_file::Timer {
            at: Vec::new(),
            every: Some(600),
            after_boot: None,
            spread: 3600,
            catch_up: true,
            time_zone: None,
        };
        assert_eq!(Timing::from_timer(&timer, first).delay, first);
    }
}
