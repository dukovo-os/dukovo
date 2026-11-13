// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! metricsd(8): the machine's history (ADR 0484).
//!
//! Once a step, 60 seconds unless `--step` says otherwise, metricsd reads
//! the machine: the processors, memory, disks, filesystems, network
//! cards, temperatures, each service's use of both and each Btrfs
//! volume's missing disks. It keeps one row of that in the history,
//! two weeks at the step and a year at one record an hour, and writes the
//! newest row as text for `metrics now`. Nothing leaves the machine
//! unless the owner names a Prometheus server in metrics.toml(5).
//!
//! - [`sample`] reads the machine and makes the row of two readings;
//! - [`store`] is the history's files, which `metrics` and the console
//!   read;
//! - [`settings`] reads `/system/settings/metrics.toml`.
//!
//! metricsd never ends because of what it reads or writes: a source that
//! is not there has no series, and a disk that takes no writes keeps the
//! rows in memory until it does. Each such trouble is said once in the
//! log (`log metricsd`) and again when it is over. SIGTERM writes the rows
//! waiting and ends, SIGHUP reads metrics.toml again.

mod command;
mod prometheus;
mod sample;
mod settings;
mod store;
mod warnings;

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use prometheus::{Endpoint, Machine, Scrape};
use sample::{Reader, Snapshot, Sources};
use settings::{Network, Settings, Thresholds};
use store::Store;
use warnings::{Sent, Warnings};

use crate::alerts::client;
use crate::alerts::protocol::Alert;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, usage};

/// Where the history is.
pub const STATE: &str = "/system/state/metrics";
/// Where the newest row is, and the file it is in.
pub const LIVE: &str = "/system/live/metrics";
pub const NOW: &str = "/system/live/metrics/now";

/// The seconds between readings unless `--step` says another, and the
/// most it may say: an hour is the history's finest summary.
const STEP_DEFAULT: u64 = 60;
const STEP_MAX: u64 = 3600;

/// The steps between the writes of the rows to the disk. A power cut loses
/// the rows since the last.
const FLUSH_STEPS: u32 = 10;

/// How often the service looks at whether a signal came.
const WATCH: Duration = Duration::from_millis(250);

static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_hangup(_: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
}

/// metrics(1).
pub use command::main as metrics;

/// metricsd(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let help = "Usage: metricsd run [--step SECONDS]";
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(l)) if l == "help" => {
                println!("{help}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => Err(usage(format!("which? run\n{help}"))),
        };
    };
    match subcommand.to_str() {
        Some("run") => run(args),
        _ => Err(usage(format!("no such command: {}\n{help}", subcommand.to_string_lossy()))),
    }
}

/// The step in seconds that the options after `run` ask for.
fn step_option(args: &mut Args) -> Result<u64, Fail> {
    let mut step = STEP_DEFAULT;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(long) if long == "step" => {
                step = args.number()?;
                if !(1..=STEP_MAX).contains(&step) {
                    return Err(usage(format!("the step is from 1 to {STEP_MAX} seconds")));
                }
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(step)
}

/// `metricsd run`: the service.
fn run(args: &mut Args) -> Result<i32, Fail> {
    let step = Duration::from_secs(step_option(args)?);
    for (signal, handler) in
        [(libc::SIGTERM, on_term as extern "C" fn(libc::c_int)), (libc::SIGHUP, on_hangup)]
    {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers only store to an atomic.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
    let mut daemon = Daemon::new(Places::system(), Sources::system(), Box::new(client::set));
    println!("metricsd: reading the machine every {} s into {STATE}", step.as_secs());
    let mut due = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        if RELOAD.swap(false, Ordering::Relaxed) {
            daemon.reload();
        }
        let now = Instant::now();
        if now < due {
            thread::sleep(WATCH.min(due - now));
            continue;
        }
        daemon.step();
        due = next_due(due, step, now);
    }
    daemon.finish();
    Ok(0)
}

/// When the step after the one due at `due` is, the one that ran at
/// `now`: a step later, so that the steps keep their rhythm. A step that
/// ran more than one late, as when the machine was stopped, is not made up
/// for: the next is a step after it.
fn next_due(due: Instant, step: Duration, now: Instant) -> Instant {
    let next = due + step;
    if next > now { next } else { now + step }
}

/// Where metricsd keeps what it writes, and reads its settings.
#[derive(Clone, Debug)]
struct Places {
    state: PathBuf,
    live: PathBuf,
    settings: PathBuf,
    /// The system's certificates, which the warnings check.
    ssl: PathBuf,
    /// alertd's socket, which a scrape asks how many alerts are active.
    alertd: PathBuf,
}

impl Places {
    fn system() -> Places {
        Places {
            state: PathBuf::from(STATE),
            live: PathBuf::from(LIVE),
            settings: PathBuf::from(settings::FILE),
            ssl: PathBuf::from(warnings::SSL),
            alertd: PathBuf::from(crate::alerts::protocol::SOCKET),
        }
    }
}

/// What has gone wrong and been said, so that it is said once, and again
/// when it is over.
#[derive(Default)]
struct Said {
    history: bool,
    write: bool,
    now: bool,
    alertd: bool,
}

impl Said {
    /// Notes that a trouble `said` is now, and gives whether it is new.
    fn begins(said: &mut bool) -> bool {
        !std::mem::replace(said, true)
    }

    /// Notes that it is over, and gives whether it had been said.
    fn ends(said: &mut bool) -> bool {
        std::mem::replace(said, false)
    }
}

/// metricsd while it runs.
struct Daemon {
    places: Places,
    /// Shared with the Prometheus endpoint, which reads the machine for
    /// each scrape and so keeps the services' CPU time in step.
    reader: Arc<Mutex<Reader>>,
    settings: Settings,
    endpoint: Option<Running>,
    /// The history, which is not there while its folder cannot be made.
    store: Option<Store>,
    /// The reading before this one, which rates are over.
    previous: Option<Snapshot>,
    /// The steps since the rows were last written.
    unwritten: u32,
    warnings: Warnings,
    /// How the alerts reach alertd.
    set_alerts: SetAlerts,
    said: Said,
}

/// The Prometheus endpoint, and what it was started with, so that a
/// reload that changes neither leaves it alone.
struct Running {
    listen: Vec<SocketAddr>,
    allow: Vec<Network>,
    endpoint: Endpoint,
}

/// A way to give alertd the service's alerts.
type SetAlerts = Box<dyn FnMut(&[Alert]) -> Result<(), String>>;

impl Daemon {
    fn new(places: Places, sources: Sources, set_alerts: SetAlerts) -> Daemon {
        let mut warnings = Warnings::new(Thresholds::default(), &places.ssl);
        // The line of a filesystem's filling goes on from the history.
        warnings.backfill(&places.state, unix_seconds());
        let mut daemon = Daemon {
            places,
            reader: Arc::new(Mutex::new(Reader::new(sources))),
            settings: Settings::default(),
            endpoint: None,
            store: None,
            previous: None,
            unwritten: 0,
            warnings,
            set_alerts,
            said: Said::default(),
        };
        daemon.reload();
        daemon
    }

    /// Reads metrics.toml, after saying what is wrong in it.
    fn reload(&mut self) {
        let text = match fs::read_to_string(&self.places.settings) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                eprintln!(
                    "<3>metricsd: {}: {}",
                    self.places.settings.display(),
                    util::strip_os_error(&error)
                );
                String::new()
            }
        };
        let (settings, wrong) = settings::read_settings(&text);
        for wrong in wrong {
            eprintln!("<3>metricsd: {wrong}");
        }
        self.warnings.set_thresholds(settings.warnings);
        self.settings = settings;
        self.serve_endpoint();
    }

    /// Starts the Prometheus endpoint as the settings say, or stops it
    /// if they say it is off. An endpoint that the settings did not change
    /// goes on, with its connections.
    fn serve_endpoint(&mut self) {
        let (listen, allow) = (&self.settings.listen, &self.settings.allow);
        let same = self.endpoint.as_ref().is_some_and(|r| r.listen == *listen && r.allow == *allow);
        if same {
            return;
        }
        if let Some(running) = self.endpoint.take() {
            running.endpoint.stop();
        }
        if listen.is_empty() {
            return;
        }
        let config = prometheus::Config::new(listen.clone(), allow.clone());
        let endpoint = Endpoint::start(config, self.scrape());
        self.endpoint = Some(Running { listen: listen.clone(), allow: allow.clone(), endpoint });
    }

    /// How a scrape is made: the machine is read afresh, and alertd is
    /// asked what is active.
    fn scrape(&self) -> Scrape {
        let (reader, alertd) = (self.reader.clone(), self.places.alertd.clone());
        Arc::new(move || {
            let snapshot = reader.lock().unwrap_or_else(PoisonError::into_inner).read();
            let active = prometheus::active_counts(&alertd, client::PATIENCE);
            let now = util::now_ns() as f64 / 1e9;
            prometheus::exposition(&snapshot, &Machine::read(), active, now)
        })
    }

    /// One step: reads the machine, keeps its row in the history and
    /// writes it as text.
    fn step(&mut self) {
        let snapshot = self.reader.lock().unwrap_or_else(PoisonError::into_inner).read();
        let row = sample::series(self.previous.as_ref(), &snapshot);
        self.write_now(&row);
        self.keep(snapshot.at, &row);
        self.warn(&snapshot);
        self.previous = Some(snapshot);
    }

    /// Gives alertd the early warnings the reading shows, if they are due.
    fn warn(&mut self, snapshot: &Snapshot) {
        let now = Instant::now();
        let alerts = self.warnings.check(snapshot, now);
        match self.warnings.report(&alerts, now, &mut self.set_alerts) {
            Sent::Not => {}
            Sent::Yes => {
                if Said::ends(&mut self.said.alertd) {
                    println!("metricsd: alertd takes the alerts again");
                }
            }
            Sent::Failed(why) => {
                if Said::begins(&mut self.said.alertd) {
                    eprintln!("<4>metricsd: cannot give alertd the alerts: {why}; trying again");
                }
            }
        }
    }

    /// Puts the row in the history, and writes the rows waiting when
    /// [`FLUSH_STEPS`] have passed.
    fn keep(&mut self, at: i64, row: &[(String, f32)]) {
        if self.store.is_none() {
            self.store = self.open_store(at);
        }
        let Some(store) = self.store.as_mut() else { return };
        store.push(at, row);
        if let Some(name) = store.take_left_out() {
            eprintln!(
                "<4>metricsd: the history keeps {} series; {name} and any new one after it are \
                 left out",
                store::MAX_SERIES
            );
        }
        self.unwritten += 1;
        if self.unwritten >= FLUSH_STEPS {
            self.write_rows();
        }
    }

    /// Opens the history, or says why it cannot be, once; metricsd tries
    /// again at each step.
    fn open_store(&mut self, at: i64) -> Option<Store> {
        match Store::open(&self.places.state, at) {
            Ok(store) => {
                if Said::ends(&mut self.said.history) {
                    println!(
                        "metricsd: the history in {} opens again",
                        self.places.state.display()
                    );
                }
                Some(store)
            }
            Err(error) => {
                if Said::begins(&mut self.said.history) {
                    eprintln!(
                        "<3>metricsd: cannot open the history in {}: {}; trying again at each step",
                        self.places.state.display(),
                        util::strip_os_error(&error)
                    );
                }
                None
            }
        }
    }

    /// Writes the rows waiting. A write that fails is said once: the rows
    /// stay in memory and the next write has them again.
    fn write_rows(&mut self) {
        let Some(store) = self.store.as_mut() else { return };
        self.unwritten = 0;
        match store.flush() {
            Ok(()) => {
                if Said::ends(&mut self.said.write) {
                    println!("metricsd: the history is being written again");
                }
            }
            Err(error) => {
                if Said::begins(&mut self.said.write) {
                    eprintln!(
                        "<3>metricsd: cannot write the history in {}: {}; the rows wait in \
                         memory",
                        self.places.state.display(),
                        util::strip_os_error(&error)
                    );
                }
            }
        }
    }

    /// Writes the newest row for `metrics now`, said once if it fails.
    fn write_now(&mut self, row: &[(String, f32)]) {
        match write_now(&self.places.live, &now_text(row)) {
            Ok(()) => {
                if Said::ends(&mut self.said.now) {
                    println!("metricsd: {NOW} is written again");
                }
            }
            Err(error) => {
                if Said::begins(&mut self.said.now) {
                    eprintln!(
                        "<3>metricsd: cannot write {}: {}",
                        self.places.live.join("now").display(),
                        util::strip_os_error(&error)
                    );
                }
            }
        }
    }

    /// Writes what waits, when metricsd is stopped.
    fn finish(&mut self) {
        self.write_rows();
    }
}

/// `bytes` as `1.2 TB`, in powers of 1000, as the warnings and `metrics`
/// say sizes.
fn size_text(bytes: u64) -> String {
    let text = util::human(bytes, true);
    match text.strip_suffix(|c: char| c.is_ascii_alphabetic()) {
        Some(number) => format!("{number} {}B", &text[number.len()..]),
        None => format!("{text} B"),
    }
}

/// The seconds since 1970, now.
fn unix_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The text of the newest row: a line `name value` for each series that
/// has a value. A name may hold spaces, a sensor's label, so the value is
/// what follows the last one.
fn now_text(row: &[(String, f32)]) -> String {
    let mut text = String::new();
    for (name, value) in row.iter().filter(|(_, value)| value.is_finite()) {
        // Writing to a string cannot fail.
        let _ = writeln!(text, "{name} {value}");
    }
    text
}

/// Replaces the file `now` of `live` with `text`: it is written beside it
/// and renamed, so that `metrics now` never reads half of it.
fn write_now(live: &Path, text: &str) -> io::Result<()> {
    fs::create_dir_all(live)?;
    let temporary = live.join("now.new");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    fs::rename(temporary, live.join("now"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::rc::Rc;

    /// The sets of alerts a daemon gave alertd, and whether alertd takes
    /// them.
    #[derive(Default)]
    struct Alertd {
        sets: Rc<RefCell<Vec<Vec<Alert>>>>,
        refuses: Rc<RefCell<bool>>,
    }

    impl Alertd {
        fn client(&self) -> SetAlerts {
            let (sets, refuses) = (self.sets.clone(), self.refuses.clone());
            Box::new(move |alerts| {
                if *refuses.borrow() {
                    return Err("alertd is not running".into());
                }
                sets.borrow_mut().push(alerts.to_vec());
                Ok(())
            })
        }
    }

    /// A folder of the test's own.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metricsd-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A machine of a few files, and where a daemon keeps its own.
    fn daemon(name: &str) -> (Daemon, PathBuf) {
        daemon_for(name, &Alertd::default())
    }

    fn daemon_for(name: &str, alertd: &Alertd) -> (Daemon, PathBuf) {
        let dir = folder(name);
        let put = |path: &str, text: &str| {
            let file = dir.join("machine").join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        };
        put("processes/stat", "user_ns: 10\nsystem_ns: 5\nidle_ns: 85\n");
        put("processes/meminfo", "total: 1000 KiB\nfree: 100 KiB\npage_cache: 300 KiB\n");
        put("processes/loadavg", "load_1: 0.50\nload_5: 0.40\nload_15: 0.30\n");
        put("kernel/class/hwmon/hwmon0/name", "cros_ec\n");
        put("kernel/class/hwmon/hwmon0/temp1_input", "36900\n");
        put("kernel/class/hwmon/hwmon0/temp1_label", "Battery sensor\n");
        let machine = dir.join("machine");
        let sources = Sources {
            processes: machine.join("processes"),
            kernel: machine.join("kernel"),
            services: machine.join("services"),
        };
        let places = Places {
            state: dir.join("state"),
            live: dir.join("live"),
            settings: dir.join("metrics.toml"),
            ssl: dir.join("ssl"),
            alertd: dir.join("alertd-socket"),
        };
        (Daemon::new(places, sources, alertd.client()), dir)
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path).unwrap().lines().map(str::to_string).collect()
    }

    fn history(daemon: &Daemon, metric: &str) -> usize {
        let select = [store::Selector::parse(metric)];
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let read =
            store::read(&daemon.places.state, now - 3600, now + 3600, &select, None).unwrap();
        read.iter().map(|series| series.points.len()).sum()
    }

    #[test]
    fn the_newest_row_is_written_as_lines_of_name_and_value() {
        let (mut daemon, dir) = daemon("now");
        daemon.step();
        let said = lines(&dir.join("live/now"));
        assert!(said.contains(&"load 0.5".to_string()), "{said:?}");
        assert!(said.contains(&"memory.used 614400".to_string()), "{said:?}");
        // A sensor's label may hold spaces: the value follows the last.
        assert!(
            said.contains(&"temp{cros_ec-isa-0000/Battery sensor} 36.9".to_string()),
            "{said:?}"
        );
        // The first reading has no rates, and a missing value has no line.
        assert!(!said.iter().any(|line| line.starts_with("cpu ")), "{said:?}");
        // It is written beside the file and renamed: nothing is left over.
        assert!(!dir.join("live/now.new").exists());
        // The second has them, over what the processors did since.
        fs::write(dir.join("machine/processes/stat"), "user_ns: 30\nsystem_ns: 10\nidle_ns: 160\n")
            .unwrap();
        daemon.step();
        let said = lines(&dir.join("live/now"));
        assert!(said.contains(&"cpu 25".to_string()), "{said:?}");
        assert!(said.contains(&"cpu.user 20".to_string()), "{said:?}");
        assert!(said.contains(&"cpu.system 5".to_string()), "{said:?}");
    }

    #[test]
    fn rows_are_written_after_each_ten_steps_and_when_it_stops() {
        let (mut daemon, _) = daemon("flush");
        for _ in 0..9 {
            daemon.step();
        }
        // Nine rows wait in memory.
        assert_eq!(history(&daemon, "load"), 0);
        daemon.step();
        assert_eq!(history(&daemon, "load"), 10);
        daemon.step();
        daemon.step();
        assert_eq!(history(&daemon, "load"), 10);
        daemon.finish();
        assert_eq!(history(&daemon, "load"), 12);
        // Stopping with nothing waiting writes nothing more.
        daemon.finish();
        assert_eq!(history(&daemon, "load"), 12);
    }

    #[test]
    fn a_history_that_cannot_be_opened_does_not_stop_the_rest() {
        let (mut daemon, dir) = daemon("nostate");
        // A file where the folder should be.
        fs::write(dir.join("state"), "in the way").unwrap();
        daemon.step();
        daemon.step();
        assert!(daemon.store.is_none());
        assert!(daemon.said.history);
        assert!(dir.join("live/now").exists());
        // Once it can be made, the next step opens it and carries on.
        fs::remove_file(dir.join("state")).unwrap();
        daemon.step();
        assert!(daemon.store.is_some());
        assert!(!daemon.said.history);
        daemon.finish();
        assert_eq!(history(&daemon, "load"), 1);
    }

    #[test]
    fn a_live_folder_that_cannot_be_made_is_said_once() {
        let (mut daemon, dir) = daemon("nolive");
        fs::write(dir.join("live"), "in the way").unwrap();
        daemon.step();
        daemon.step();
        assert!(daemon.said.now);
        // The history is kept all the same.
        daemon.finish();
        assert_eq!(history(&daemon, "load"), 2);
        fs::remove_file(dir.join("live")).unwrap();
        daemon.step();
        assert!(!daemon.said.now && dir.join("live/now").exists());
    }

    #[test]
    fn settings_are_read_at_start_and_at_a_reload() {
        let (mut daemon, dir) = daemon("settings");
        assert_eq!(daemon.settings, Settings::default());
        fs::write(dir.join("metrics.toml"), "[warnings]\nfilesystem = 80\n").unwrap();
        daemon.reload();
        assert_eq!(daemon.settings.warnings.filesystem, 80);
        // A file with a mistake does not stop it, and a file that is gone
        // is the defaults.
        fs::write(dir.join("metrics.toml"), "[warnings]\nfilesystem = = 80\n").unwrap();
        daemon.reload();
        assert_eq!(daemon.settings, Settings::default());
        fs::remove_file(dir.join("metrics.toml")).unwrap();
        daemon.reload();
        assert_eq!(daemon.settings, Settings::default());
    }

    /// Makes the sensor of the test machine read `millidegrees`, with a
    /// critical temperature of 100 degrees.
    fn heat(dir: &Path, millidegrees: u32) {
        let hwmon = dir.join("machine/kernel/class/hwmon/hwmon0");
        fs::write(hwmon.join("temp1_input"), format!("{millidegrees}\n")).unwrap();
        fs::write(hwmon.join("temp1_crit"), "100000\n").unwrap();
    }

    #[test]
    fn the_early_warnings_are_given_to_alertd_after_each_step() {
        let alertd = Alertd::default();
        let (mut daemon, dir) = daemon_for("warnings", &alertd);
        daemon.step();
        // The first step gives the empty set, which clears what alertd
        // had from before a restart.
        assert_eq!(alertd.sets.borrow().len(), 1);
        assert!(alertd.sets.borrow()[0].is_empty());
        heat(&dir, 99_000);
        for _ in 0..4 {
            daemon.step();
        }
        assert_eq!(alertd.sets.borrow().len(), 1, "four steps are not enough");
        daemon.step();
        let sets = alertd.sets.borrow();
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[1].len(), 1);
        assert_eq!(sets[1][0].key, "temp:cros_ec-isa-0000/battery-sensor");
        drop(sets);
        // The set is not given again while it is the same, and the
        // thresholds of metrics.toml are the daemon's.
        daemon.step();
        assert_eq!(alertd.sets.borrow().len(), 2);
        fs::write(dir.join("metrics.toml"), "[warnings]\ntemperature-margin = 30\n").unwrap();
        daemon.reload();
        assert_eq!(daemon.settings.warnings.temperature_margin, 30);
    }

    #[test]
    fn an_alertd_that_does_not_answer_is_said_once_and_does_not_stop_the_rest() {
        let alertd = Alertd::default();
        *alertd.refuses.borrow_mut() = true;
        let (mut daemon, dir) = daemon_for("alertd-down", &alertd);
        daemon.step();
        assert!(daemon.said.alertd);
        daemon.step();
        daemon.step();
        // It is tried again after ten seconds, not at each step.
        assert!(alertd.sets.borrow().is_empty());
        assert!(dir.join("live/now").exists());
        daemon.finish();
        assert_eq!(history(&daemon, "load"), 3);
    }

    /// What `GET /metrics` at `address` answers.
    fn scrape_at(address: SocketAddr) -> String {
        use std::io::Read as _;
        // The endpoint's thread binds as it starts.
        let mut stream = (0..100)
            .find_map(|_| {
                std::net::TcpStream::connect(address).ok().or_else(|| {
                    thread::sleep(Duration::from_millis(20));
                    None
                })
            })
            .expect("the endpoint listens");
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn the_endpoint_follows_metrics_toml() {
        let (mut daemon, dir) = daemon("endpoint");
        assert!(daemon.endpoint.is_none());
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let on = format!("[prometheus]\nlisten = [\"{address}\"]\nallow = [\"127.0.0.1\"]\n");
        fs::write(dir.join("metrics.toml"), &on).unwrap();
        daemon.reload();
        assert!(daemon.endpoint.is_some());
        // The machine is read for the scrape, not taken from the history,
        // and alertd's not answering leaves its metric out.
        let answer = scrape_at(address);
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.contains("\nnode_load1 0.5\n"), "{answer}");
        assert!(answer.contains("\nnode_memory_MemTotal_bytes 1024000\n"), "{answer}");
        assert!(!answer.contains("dukovo_alerts_active"));
        // The same settings go on with the same endpoint; others replace
        // it, and none stops it.
        daemon.reload();
        assert!(daemon.endpoint.is_some());
        fs::write(dir.join("metrics.toml"), "# off\n").unwrap();
        daemon.reload();
        assert!(daemon.endpoint.is_none());
        assert!(std::net::TcpStream::connect(address).is_err());
    }

    #[test]
    fn a_listen_without_allow_does_not_open_the_endpoint() {
        let (mut daemon, dir) = daemon("no-allow");
        fs::write(dir.join("metrics.toml"), "[prometheus]\nlisten = [\"127.0.0.1:9100\"]\n")
            .unwrap();
        daemon.reload();
        assert!(daemon.endpoint.is_none());
    }

    #[test]
    fn sizes_are_said_in_powers_of_1000() {
        assert_eq!(size_text(1_300_000_000_000), "1.3 TB");
        assert_eq!(size_text(93_000_000_000), "93 GB");
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(0), "0 B");
    }

    #[test]
    fn steps_keep_their_rhythm_and_a_stall_is_not_made_up_for() {
        let (start, step) = (Instant::now(), Duration::from_secs(60));
        let later = |seconds| start + Duration::from_secs(seconds);
        // A step that ran a little late does not move the next.
        assert_eq!(next_due(start, step, later(1)), later(60));
        assert_eq!(next_due(later(60), step, later(61)), later(120));
        // One that ran after the next was due does not bring two at once.
        assert_eq!(next_due(start, step, later(200)), later(260));
    }

    #[test]
    fn the_step_is_from_one_second_to_an_hour() {
        let step = |items: &[&str]| {
            step_option(&mut Args::new("metricsd", items.iter().map(OsString::from).collect()))
        };
        assert_eq!(step(&[]).unwrap(), 60);
        assert_eq!(step(&["--step", "1"]).unwrap(), 1);
        assert_eq!(step(&["--step=3600"]).unwrap(), 3600);
        for wrong in [&["--step", "0"][..], &["--step", "3601"], &["--step", "x"], &["--step"]] {
            assert!(step(wrong).is_err(), "{wrong:?}");
        }
        assert!(step(&["--steps", "5"]).is_err());
        assert!(step(&["5"]).is_err());
    }
}
