// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! alertd(8): keeps the alerts (ADR 0485).
//!
//! A service or a root script raises an alert on alertd's socket
//! ([`crate::alerts::protocol`]); alertd keeps which alerts are active
//! and what happened to them in `/system/state/alerts`, and tells the
//! owner of each change once: at once as a notification on the screen and
//! in the notice at the next sign-in ([`notify`]), and by email and
//! webhooks as alerts.toml(5) says, gathered into one message every 30
//! seconds and tried again until it gets through ([`deliver`]). alertd
//! also watches Spark, and raises `service:<name>` for a service that has
//! not run as asked for a minute ([`services`]).
//!
//! The files:
//!
//! - [`state`] the rules for the changes, and the files that keep them;
//! - [`settings`] alerts.toml, which alertd reads only when root's alone
//!   can read it, as it holds passwords and tokens;
//! - [`deliver`] batching, retries and the outbox, [`text`] the words
//!   of the messages, [`email`] SMTP submission with TLS, [`webhook`]
//!   ntfy, Matrix, Slack, Discord and JSON;
//! - [`socket`] one thread for each connection, at most 16, and the check
//!   of who asks.
//!
//! One lock holds the state and the outbox. A socket thread applies a
//! change under it, takes copies of the files to write, and writes them
//! after it lets go; no lock is held across the network, a disk write
//! that was already taken, or notifyd. Delivery runs on threads of its
//! own, one for each message being sent, never on a socket thread: a
//! slow mail server holds up nothing a producer waits for.
//!
//! SIGHUP (`service alertd reload`) reads alerts.toml again. After a
//! restart the active alerts and the outbox are as they were, and
//! nothing that went is sent again.

mod deliver;
mod email;
mod notify;
mod services;
mod settings;
mod socket;
mod state;
pub(crate) mod text;
mod trouble;
mod webhook;

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::alerts::protocol::{Active, Alert, Change, Channel, Delivery, SOCKET, Severity};
use crate::args::Args;
use crate::util::{self, Fail};
use deliver::{Message, Outbox};
use notify::Tell;
use settings::Settings;
use state::{Snapshot, State, Store};

/// The most connections served at once; another is refused as busy.
const CONNECTIONS_MAX: usize = 16;

/// Set by SIGTERM: alertd ends. Set by SIGHUP: alertd reads alerts.toml
/// again.
static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_hangup(_: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
}

/// What the lock holds: the alerts, the messages waiting for the
/// channels, and the channels.
struct Core {
    state: State,
    outbox: Outbox,
    settings: Settings,
}

/// Everything alertd holds, shared by its threads.
struct Alertd {
    core: Mutex<Core>,
    store: Store,
    /// The machine's name, as the messages say it.
    host: String,
    /// What the thread that tells the screen is to tell.
    tells: SyncSender<Tell>,
    /// How many connections are being served.
    serving: AtomicUsize,
    /// How many connections of those who may not ask are being read, to
    /// be refused.
    refusing: AtomicUsize,
}

/// Who hears of a change besides the history: the channels, from the
/// queue, and the consoles, in a notification.
#[derive(Clone, Copy, Debug)]
struct Reach {
    channels: bool,
    screen: bool,
}

impl Reach {
    const ALL: Reach = Reach { channels: true, screen: true };
    /// A test, which is sent to the channels at once and not queued.
    const SCREEN: Reach = Reach { channels: false, screen: true };
}

/// The time now, in seconds since 1970.
fn now() -> u64 {
    util::now_ns() / 1_000_000_000
}

/// Says `text` in the log at syslog level `level`.
fn log(level: u8, text: &str) {
    eprintln!("<{level}>alertd: {text}");
}

impl Alertd {
    fn new(dir: &Path, core: Core, host: String, tells: SyncSender<Tell>) -> Alertd {
        Alertd {
            core: Mutex::new(core),
            store: Store::new(dir),
            host,
            tells,
            serving: AtomicUsize::new(0),
            refusing: AtomicUsize::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Core> {
        // A thread that panicked left the state whole: every rule runs
        // to its end before it can fail.
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Applies what `op` does to the state at the time now, queues the
    /// changes for the channels and tells the screen of them as `reach`
    /// says, and saves what changed. The changes, or why `op` refused.
    fn apply(
        &self,
        op: impl FnOnce(&mut State, u64) -> Result<Vec<Change>, String>,
        reach: Reach,
    ) -> Result<Vec<Change>, String> {
        let now = now();
        let mut guard = self.lock();
        let core = &mut *guard;
        let changes = op(&mut core.state, now)?;
        let mut dropped = Vec::new();
        if reach.channels {
            for change in &changes {
                dropped.extend(core.outbox.add(&core.settings.channels, change, now));
            }
        }
        let (active, history) = core.state.take_text();
        let snapshot = Snapshot { active, history, outbox: core.outbox.take_text() };
        let writer = self.store.begin();
        let alerts = (!changes.is_empty()).then(|| core.state.active());
        drop(guard);
        writer.write(snapshot);
        for line in dropped {
            log(4, &line);
        }
        if let Some(active) = alerts {
            // A full queue loses nothing that matters: the sign-in notice
            // is written from the next telling.
            let told = if reach.screen { changes.clone() } else { Vec::new() };
            let _ = self.tells.try_send(Tell { changes: told, active });
        }
        Ok(changes)
    }

    /// Raises, or updates, `alert` for `source`.
    fn raise(&self, source: &str, alert: Alert) -> Result<(), String> {
        let apply = |state: &mut State, now| {
            state.raise(source, alert, now).map(|c| c.into_iter().collect())
        };
        self.apply(apply, Reach::ALL).map(drop)
    }

    /// Records `alert` as an event of `source`, which has shown it on the
    /// consoles itself when `shown`.
    fn event(&self, source: &str, alert: Alert, shown: bool) -> Result<(), String> {
        let reach = Reach { channels: true, screen: !shown };
        self.apply(|state, now| Ok(vec![state.event(source, alert, now)]), reach).map(drop)
    }

    /// Clears `source`'s alert `key`.
    fn clear(&self, source: &str, key: &str) -> Result<(), String> {
        let apply =
            |state: &mut State, now| Ok(state.clear(source, key, now).into_iter().collect());
        self.apply(apply, Reach::ALL).map(drop)
    }

    /// Makes `alerts` the whole of `source`'s alerts.
    fn set(&self, source: &str, alerts: Vec<Alert>) -> Result<(), String> {
        self.apply(|state, now| state.set(source, alerts, now), Reach::ALL).map(drop)
    }

    fn active(&self) -> Vec<Active> {
        self.lock().state.active()
    }

    fn history(&self, count: usize) -> Vec<Change> {
        self.lock().state.newest(count).to_vec()
    }

    fn status(&self) -> Vec<Channel> {
        let core = self.lock();
        core.outbox.status(&core.settings.channels)
    }

    /// Sends a test alert from `source` to every channel at once, and
    /// says how each went. The test is in the history and on the screen
    /// too, and is not queued: it is sent here, and not again.
    fn test(&self, source: &str) -> Vec<Delivery> {
        let alert = Alert {
            key: "test".into(),
            severity: Severity::Warning,
            title: "Test alert from alertd".into(),
            detail: "If you can read this, alerts reach this channel.".into(),
        };
        let Ok(changes) =
            self.apply(|state, now| Ok(vec![state.test(source, alert, now)]), Reach::SCREEN)
        else {
            return Vec::new();
        };
        let channels = self.lock().settings.channels.clone();
        match changes.first() {
            Some(change) => deliver::test_all(&channels, change, &self.host),
            None => Vec::new(),
        }
    }

    /// Reads alerts.toml again, and drops the waiting messages of
    /// channels that are not in it any more.
    fn reload(&self) {
        let (settings, problems) = settings::load(&self.host);
        for problem in &problems {
            log(4, problem);
        }
        let names: Vec<String> = settings.channels.iter().map(|c| c.name.clone()).collect();
        let mut guard = self.lock();
        let lines = guard.outbox.keep_only(&names);
        guard.settings = settings;
        let snapshot = Snapshot { outbox: guard.outbox.take_text(), ..Snapshot::default() };
        let writer = self.store.begin();
        drop(guard);
        writer.write(snapshot);
        for line in lines {
            log(4, &line);
        }
        log(6, &format!("read alerts.toml: {} channels", names.len()));
    }

    /// Starts a thread for each message that is due at `now`, and gives
    /// their handles. The outbox decides which: one message for a
    /// channel at a time.
    fn send_due(self: &Arc<Self>, now: u64) -> Vec<JoinHandle<()>> {
        let mut guard = self.lock();
        let core = &mut *guard;
        let due = core.outbox.due(now);
        let jobs: Vec<_> = due
            .send
            .into_iter()
            .map(|message| {
                let channel = core.settings.channels.iter().find(|c| c.name == message.channel);
                (message, channel.cloned())
            })
            .collect();
        let snapshot = Snapshot { outbox: core.outbox.take_text(), ..Snapshot::default() };
        let writer = self.store.begin();
        drop(guard);
        writer.write(snapshot);
        for line in due.dropped {
            log(4, &line);
        }
        jobs.into_iter()
            .map(|(message, channel)| {
                let this = Arc::clone(self);
                thread::spawn(move || this.attempt(&message, channel.as_ref()))
            })
            .collect()
    }

    /// Sends `message` to `channel` and says how it went to the outbox.
    fn attempt(&self, message: &Message, channel: Option<&settings::Channel>) {
        let result = match channel {
            Some(channel) => deliver::send(channel, &message.id, &message.changes, &self.host),
            None => Err("the channel is not in alerts.toml any more".into()),
        };
        let mut guard = self.lock();
        let lines = guard.outbox.finish(&message.id, result, now());
        let snapshot = Snapshot { outbox: guard.outbox.take_text(), ..Snapshot::default() };
        let writer = self.store.begin();
        drop(guard);
        writer.write(snapshot);
        for line in lines {
            log(4, &line);
        }
    }
}

/// alertd(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    install_signals();
    let host = util::hostname();
    let (settings, problems) = settings::load(&host);
    for problem in &problems {
        log(4, problem);
    }
    let dir = Path::new(state::DIR);
    let (state, problems) = State::load(dir);
    for problem in &problems {
        log(3, problem);
    }
    let (mut outbox, problem) = load_outbox(dir);
    if let Some(problem) = problem {
        log(3, &problem);
    }
    let names: Vec<String> = settings.channels.iter().map(|c| c.name.clone()).collect();
    for line in outbox.keep_only(&names) {
        log(4, &line);
    }
    let listener = listen()?;
    let tells = notify::start(&state.active());
    let alertd = Arc::new(Alertd::new(dir, Core { state, outbox, settings }, host, tells));
    start_threads(&alertd);
    log(6, &format!("keeping alerts at {SOCKET}; {} channels", names.len()));
    let served = serve(&alertd, &listener);
    let _ = fs::remove_file(SOCKET);
    served.map_err(|err| util::failed(&"alertd", err))?;
    Ok(0)
}

/// The outbox the file holds, and what is wrong with it; a file that is
/// not there is an empty outbox.
fn load_outbox(dir: &Path) -> (Outbox, Option<String>) {
    match fs::read_to_string(dir.join(state::OUTBOX_FILE)) {
        Ok(text) => Outbox::from_text(&text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => (Outbox::default(), None),
        Err(err) => (
            Outbox::default(),
            Some(format!(
                "{}: {}",
                dir.join(state::OUTBOX_FILE).display(),
                util::strip_os_error(&err)
            )),
        ),
    }
}

fn install_signals() {
    // SAFETY: setting a signal's disposition touches no memory. A client
    // that goes away must not end alertd: the write to it fails instead.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    for (signal, handler) in [
        (libc::SIGTERM, on_term as extern "C" fn(libc::c_int)),
        (libc::SIGHUP, on_hangup as extern "C" fn(libc::c_int)),
    ] {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers only store to atomics.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
}

/// The socket, open to every program: alertd checks who asks, and the
/// 16-connection limit and the 5-second read keep one that is not root
/// from holding it.
fn listen() -> Result<UnixListener, Fail> {
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(SOCKET).parent().unwrap_or(Path::new("/"));
    fs::create_dir_all(dir).map_err(|err| util::failed(&dir.display(), err))?;
    let _ = fs::remove_file(SOCKET);
    let listener = UnixListener::bind(SOCKET).map_err(|err| util::failed(&SOCKET, err))?;
    let open = |path: &Path, mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|err| util::failed(&path.display(), err))
    };
    open(dir, 0o755)?;
    open(Path::new(SOCKET), 0o666)?;
    listener.set_nonblocking(true).map_err(|err| util::failed(&SOCKET, err))?;
    Ok(listener)
}

/// The thread that sends what is due, once a second, and the thread that
/// watches Spark.
fn start_threads(alertd: &Arc<Alertd>) {
    let sender = Arc::clone(alertd);
    thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(1));
            drop(sender.send_due(now()));
        }
    });
    let watcher = Arc::clone(alertd);
    thread::spawn(move || watch_services(&watcher));
}

/// Asks Spark what its services do every few seconds and sets the
/// `spark` source's alerts to those that are wrong.
fn watch_services(alertd: &Alertd) {
    let mut watch = services::Watch::default();
    let mut failing = false;
    while !STOP.load(Ordering::Relaxed) {
        let shown = crate::power::request("show")
            .map_err(|why| why.message)
            .and_then(|answer| spark_api::show::parse(&answer));
        match shown {
            Ok(show) => {
                failing = false;
                let alerts = watch.observe(&show);
                if let Err(why) = alertd.set(services::SOURCE, alerts) {
                    log(4, &format!("cannot raise the alerts for services: {why}"));
                }
            }
            // Said once for as long as it keeps failing; what is raised
            // stays.
            Err(why) if !failing => {
                failing = true;
                log(4, &format!("cannot read Spark's services: {why}"));
            }
            Err(_) => {}
        }
        thread::sleep(Duration::from_secs(services::EVERY_SECONDS));
    }
}

/// Accepts connections until SIGTERM, and reads alerts.toml again on
/// SIGHUP.
fn serve(alertd: &Arc<Alertd>, listener: &UnixListener) -> io::Result<()> {
    while !STOP.load(Ordering::Relaxed) {
        if RELOAD.swap(false, Ordering::Relaxed) {
            alertd.reload();
        }
        let mut fds = [libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        // SAFETY: `fds` holds one valid pollfd.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, 500) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        while let Ok((stream, _)) = listener.accept() {
            socket::accept(alertd, stream);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, sync_channel};

    use crate::alerts::protocol::State as Kind;
    use settings::{Channel as Configured, Filter, Kind as HookKind, Target, Webhook};

    /// A folder of a test's own, which is removed when the test is done.
    pub(super) struct Dir(PathBuf);

    impl std::ops::Deref for Dir {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("alertd-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Dir(dir)
    }

    /// An alertd with the channels `channels`, saving in a folder of its
    /// own, and the receiver of what it tells the screen.
    pub(super) fn fixture(
        name: &str,
        channels: Vec<Configured>,
    ) -> (Arc<Alertd>, Receiver<Tell>, Dir) {
        let dir = scratch(name);
        let (tx, rx) = sync_channel(16);
        let core = Core {
            state: State::default(),
            outbox: Outbox::default(),
            settings: Settings { channels },
        };
        (Arc::new(Alertd::new(&dir, core, "server".into(), tx)), rx, dir)
    }

    pub(super) fn warning(key: &str) -> Alert {
        Alert {
            key: key.into(),
            severity: Severity::Warning,
            title: format!("{key} title"),
            detail: String::new(),
        }
    }

    pub(super) fn json_channel(port: u16) -> Configured {
        Configured {
            name: "json (127.0.0.1)".into(),
            filter: Filter { severity: Severity::Warning, cleared: true },
            target: Target::Webhook(Webhook {
                kind: HookKind::Json,
                url: format!("http://127.0.0.1:{port}/alerts"),
                token: None,
                room: None,
            }),
        }
    }

    #[test]
    fn a_change_is_saved_queued_for_the_channels_and_told_to_the_screen() {
        let (alertd, screen, dir) = fixture("change", vec![json_channel(1)]);
        alertd.raise("backup", warning("backup-late")).unwrap();
        let tell = screen.try_recv().unwrap();
        assert_eq!(tell.changes.len(), 1);
        assert_eq!(tell.active.len(), 1);
        assert_eq!(alertd.status()[0].waiting, 1);
        let (loaded, problems) = State::load(&dir);
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(loaded.active().len(), 1);
        assert!(dir.join(state::OUTBOX_FILE).exists());
    }

    #[test]
    fn a_second_raise_of_the_same_alert_tells_and_queues_nothing() {
        let (alertd, screen, _dir) = fixture("again", vec![json_channel(1)]);
        alertd.raise("backup", warning("backup-late")).unwrap();
        screen.try_recv().unwrap();
        alertd.raise("backup", warning("backup-late")).unwrap();
        assert!(screen.try_recv().is_err());
        assert_eq!(alertd.status()[0].waiting, 1);
        assert_eq!(alertd.history(10).len(), 1);
    }

    #[test]
    fn an_event_its_sender_showed_goes_to_the_channels_without_a_notification() {
        let (alertd, screen, _dir) = fixture("shown", vec![json_channel(1)]);
        alertd.event("sysupdate", warning("update-fell-back"), true).unwrap();
        assert!(screen.try_recv().unwrap().changes.is_empty());
        assert_eq!(alertd.status()[0].waiting, 1);
        assert_eq!(alertd.history(10).len(), 1);
        // One its sender did not show is told as any change is.
        alertd.event("dumps", warning("kernel-panic"), false).unwrap();
        assert_eq!(screen.try_recv().unwrap().changes.len(), 1);
        assert_eq!(alertd.history(10).len(), 2);
    }

    #[test]
    fn a_clear_is_told_and_the_alert_is_no_longer_active() {
        let (alertd, screen, _dir) = fixture("clear", Vec::new());
        alertd.raise("backup", warning("k")).unwrap();
        alertd.clear("backup", "k").unwrap();
        screen.try_recv().unwrap();
        let cleared = screen.try_recv().unwrap();
        assert_eq!((cleared.changes[0].state, cleared.active.len()), (Kind::Cleared, 0));
        assert!(alertd.active().is_empty());
        // Clearing what is not active is told to no one.
        alertd.clear("backup", "k").unwrap();
        assert!(screen.try_recv().is_err());
    }

    #[test]
    fn a_refused_change_saves_and_tells_nothing() {
        let (alertd, screen, _dir) = fixture("refused", Vec::new());
        for n in 0..state::SOURCE_MAX {
            alertd.raise("loop", warning(&format!("k{n}"))).unwrap();
            screen.try_recv().unwrap();
        }
        assert!(alertd.raise("loop", warning("one-too-many")).is_err());
        assert!(screen.try_recv().is_err());
        assert!(alertd.raise("other", warning("fine")).is_ok());
    }

    #[test]
    fn a_message_is_sent_when_it_is_due_and_not_sent_again_once_it_went() {
        let (port, server) = crate::alertd::webhook::tests::server("200 OK", "");
        let (alertd, _screen, dir) = fixture("send", vec![json_channel(port)]);
        alertd.raise("backup", warning("backup-late")).unwrap();
        assert!(alertd.send_due(now()).is_empty(), "not before 30 seconds");
        let sending = alertd.send_due(now() + deliver::BATCH_WAIT);
        assert_eq!(sending.len(), 1);
        for handle in sending {
            handle.join().unwrap();
        }
        let got = server.join().unwrap();
        assert!(got.body.contains(r#""key":"backup-late""#), "{}", got.body);
        assert_eq!(alertd.status()[0].waiting, 0);
        assert!(alertd.status()[0].last_ok.is_some());
        assert!(alertd.send_due(now() + 3600).is_empty());
        // What is left on the disk is an empty outbox.
        let text = fs::read_to_string(dir.join(state::OUTBOX_FILE)).unwrap();
        assert_eq!(text, "[]");
    }

    #[test]
    fn a_message_that_fails_stays_in_the_outbox_on_disk_for_the_next_start() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = listener.local_addr().unwrap().port();
        drop(listener);
        let (alertd, _screen, dir) = fixture("fail", vec![json_channel(dead)]);
        alertd.raise("backup", warning("backup-late")).unwrap();
        for handle in alertd.send_due(now() + deliver::BATCH_WAIT) {
            handle.join().unwrap();
        }
        let status = alertd.status();
        assert_eq!(status[0].waiting, 1);
        assert_eq!(
            status[0].last_error.as_ref().unwrap().1,
            "cannot reach 127.0.0.1: Connection refused"
        );
        let (restarted, problem) = load_outbox(&dir);
        assert_eq!(problem, None);
        assert_eq!(restarted.status(&[json_channel(dead)])[0].waiting, 1);
    }

    #[test]
    fn a_test_goes_to_the_channels_at_once_and_is_not_queued() {
        let (port, server) = crate::alertd::webhook::tests::server("200 OK", "");
        let (alertd, screen, _dir) = fixture("test", vec![json_channel(port)]);
        let answers = alertd.test("alerts");
        assert_eq!(answers.len(), 1);
        assert!(answers[0].ok, "{answers:?}");
        let got = server.join().unwrap();
        assert!(got.body.contains(r#""state":"test""#), "{}", got.body);
        assert_eq!(alertd.status()[0].waiting, 0);
        assert_eq!(screen.try_recv().unwrap().changes[0].state, Kind::Test);
        assert!(alertd.active().is_empty());
        assert_eq!(alertd.history(5)[0].state, Kind::Test);
    }

    #[test]
    fn a_restart_keeps_the_active_alerts_and_a_producers_next_set_clears_what_went() {
        let (alertd, screen, dir) = fixture("restart", Vec::new());
        alertd.set("diskhealth", vec![warning("smart:a"), warning("smart:b")]).unwrap();
        screen.try_recv().unwrap();
        let (state, problems) = State::load(&dir);
        assert_eq!(problems, Vec::<String>::new());
        let (tx, rx) = sync_channel(16);
        let core = Core { state, outbox: Outbox::default(), settings: Settings::default() };
        let restarted = Alertd::new(&dir, core, "server".into(), tx);
        assert_eq!(restarted.active().len(), 2);
        restarted.set("diskhealth", vec![warning("smart:a"), warning("smart:b")]).unwrap();
        assert!(rx.try_recv().is_err(), "nothing is told twice");
        restarted.set("diskhealth", vec![warning("smart:a")]).unwrap();
        assert_eq!(rx.try_recv().unwrap().changes[0].state, Kind::Cleared);
    }
}
