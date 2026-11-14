// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Spark's `show` answer (spark(8), ADR 0470): every service it knows,
//! what it is doing and why, as one JSON document on one line. `show
//! NAME` answers the same document with that service alone.
//!
//! The document is what service(8), sysupdate's health check, bundled(8)
//! and the web console know of the services, so every field a person
//! may need to see is here. A client ignores fields it does not know,
//! and a field a document leaves out takes its default, so a later Spark
//! can add one without a new [`VERSION`]. Times are milliseconds since
//! the machine started, as `now_ms` is.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// The version of the document: a client refuses a later one.
pub const VERSION: u32 = 1;

/// The whole answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Show {
    /// [`VERSION`], when Spark wrote it.
    pub show_version: u32,
    /// The time the answer was written.
    pub now_ms: u64,
    /// Each service, in the order Spark starts them at boot.
    pub services: Vec<Service>,
    /// The shutdown that is set for later, if one is.
    pub shutdown: Option<Shutdown>,
    /// The settings being tried, each undone unless it is kept in time
    /// (ADR 0472).
    pub trials: Vec<Trial>,
    /// The trials undone since the machine started, the newest last, at
    /// most [`UNDONE_KEPT`], so that a change undone while no one watched
    /// is still told.
    pub undone: Vec<Undone>,
}

/// The undone trials a document holds at most.
pub const UNDONE_KEPT: usize = 8;

/// A service.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Service {
    /// Its name, its file's without `.toml`.
    pub name: String,
    /// What it does, from its file.
    pub description: String,
    /// Whether it starts at boot.
    pub enabled: bool,
    /// Its `order`.
    pub order: u32,
    /// When it is started again after it ends: always, on-failure or
    /// never.
    pub restart: String,
    /// The failed starts in a row after which Spark stops trying; 0 for
    /// never.
    pub give_up_after: u32,
    /// The services it requires.
    pub requires: Vec<String>,
    /// The services it wants.
    pub wants: Vec<String>,
    /// The services it starts after.
    pub after: Vec<String>,
    /// When it is ready: started, listening, socket, healthy or ended.
    pub ready_when: String,
    /// What it is doing.
    pub state: State,
    /// Its process, while it is starting or running.
    pub pid: Option<u64>,
    /// When it came to its state.
    pub since_ms: u64,
    /// When it last started.
    pub started_ms: Option<u64>,
    /// When it was last ready.
    pub ready_ms: Option<u64>,
    /// While it is starting: the time by which it must be ready, or its
    /// start has failed (its `ready.timeout` after it started).
    pub ready_by_ms: Option<u64>,
    /// While it is waiting to start again: when Spark starts it.
    pub retry_ms: Option<u64>,
    /// How many times it has been started again since boot.
    pub restarts: u32,
    /// Its failed starts in a row.
    pub failures: u32,
    /// How its process last ended, in words: "exited with code 1".
    pub last_end: Option<String>,
    /// Its last wait status, as waitpid(2) gives it.
    pub last_status: Option<u32>,
    /// Why it is not running as asked, as a clause that follows its
    /// name: "it requires db, which is stopped"; or, while it is
    /// `off_as_meant`, why it is off.
    pub why: Option<String>,
    /// Whether it is stopped as it is meant to be, which is no failure:
    /// the kernel left it none of the cards it drives, or Rescue or
    /// essentials leave it off until it is started.
    pub off_as_meant: bool,
    /// While it is pending: the services it waits for.
    pub waiting_for: Vec<String>,
    /// What started it: "boot", "a request", or "web, which requires it".
    pub started_by: Option<String>,
    /// Its health check, if its file has one.
    pub health: Option<Health>,
    /// What is wrong with its file, while it runs its last good copy.
    pub complaint: Option<String>,
    /// The service its `why` names, when it names one: the requirement
    /// that stopped, failed or is missing, which an explanation follows.
    pub cause: Option<String>,
    /// The last line its output wrote, kept from its end until its next
    /// start: most often what a program says as it fails.
    pub last_line: Option<String>,
    /// The last request that started, stopped or restarted it.
    pub last_request: Option<Request>,
    /// What its file gives it to reach and do.
    pub holds: Holds,
    /// When it starts once enabled: "at-boot", or "on-connection", at the
    /// first connection to one of its sockets (ADR 0477).
    pub start: String,
    /// Whether timerd starts it, at the times its file names (ADR 0478).
    pub timer: bool,
    /// The signal that asks it to stop, as "SIGTERM" (ADR 0477).
    pub stop_signal: String,
    /// The seconds Spark waits for it to end after its stop signal before
    /// it ends its domain.
    pub stop_timeout_s: u32,
    /// What its domain has used and been held to, in the answer to `show
    /// NAME` alone, once Spark has made the domain.
    pub usage: Option<Usage>,
}

/// A request a client made of Spark.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    /// `start`, `stop` or `restart`.
    pub verb: String,
    /// The user the asking process acts for.
    pub user: u32,
    /// The asking process, by number.
    pub process: u64,
    /// When it asked.
    pub ms: u64,
}

/// What a service's file gives it (service.toml(5), ADR 0217), beyond
/// what every service's view holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Holds {
    /// It runs with Spark's root, the whole tree, in place of a view:
    /// `runs-with-your-access`.
    pub whole_tree: bool,
    /// The account it acts for, a bundle's service's; root's when none.
    pub account: Option<String>,
    /// The places it may read, by absolute path.
    pub reads: Vec<String>,
    /// The places it may change.
    pub changes: Vec<String>,
    /// The places it mounts filesystems beneath.
    pub mounts: Vec<String>,
    /// Its `/devices` of one class (ADR 0473), "disks" or "efi-system",
    /// in place of the devices every service has.
    pub devices: Option<String>,
    /// Whether it may write the disks of `devices`, or only read them.
    pub devices_write: bool,
    /// The names at the top of the root its view holds, at
    /// `/volumes/<disk>`.
    pub root_subvolumes: Vec<String>,
    /// Whether it sees every process, as its file names `/processes`,
    /// or only those of its own domain.
    pub every_process: bool,
    /// The names of its secrets.
    pub secrets: Vec<String>,
    /// The ports it may listen on: "80", or a range as "8000-8010".
    pub listen: Vec<String>,
    /// Whether it may connect out.
    pub outbound: bool,
    /// The sockets Spark holds for it, in its file's order, in words: "port
    /// 80", "UDP port 53 on ::", "/system/live/web/db" (ADR 0477).
    pub sockets: Vec<String>,
    /// More groups its processes are in, by name: those of the sockets
    /// other bundles offer it (ADR 0477).
    pub groups: Vec<String>,
    /// The machine's settings it may change: "network", "hardware",
    /// "time", "firewall" and "kernel settings".
    pub admin: Vec<String>,
    /// Whether it may give processes and files another user's name.
    pub names: bool,
    /// The PCI cards it was given, by address, as "0000:00:03.0".
    pub cards: Vec<String>,
    /// Its domain's memory limit, if it has one.
    pub memory_bytes: Option<u64>,
    /// Its domain's CPU weight.
    pub cpu_weight: u32,
    /// Its domain's disk I/O weight (ADR 0476).
    pub io_weight: u32,
    /// The bytes it may read from disks each second, if it has a limit.
    pub io_read: Option<u64>,
    /// The bytes it may write to disks each second, if it has a limit.
    pub io_write: Option<u64>,
    /// The read requests it may make each second, if it has a limit.
    pub io_read_ops: Option<u64>,
    /// The write requests it may make each second, if it has a limit.
    pub io_write_ops: Option<u64>,
    /// The most threads its domain may have.
    pub threads: u32,
    /// The files each of its processes may have open, if its file says.
    pub open_files: Option<u32>,
}

/// What a service's domain has used and been held to since Spark made it
/// (ADR 0476): counted for the domain, so they hold what its processes
/// before this one did too, and stay after the service ends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// The memory charged to it now, in bytes.
    pub memory_bytes: u64,
    /// The most memory charged to it at once, in bytes.
    pub memory_peak_bytes: u64,
    /// The charges its memory limit has refused.
    pub memory_refused: u64,
    /// Whether its last end was a stop at its memory limit.
    pub stopped_at_limit: bool,
    /// Its threads now.
    pub threads: u32,
    /// The bytes its processes have read from disks.
    pub read_bytes: u64,
    /// The bytes its processes have written to disks, those their writes
    /// left dirty in the page cache among them.
    pub written_bytes: u64,
    /// The time its requests have waited for a disk's room, in
    /// milliseconds.
    pub waited_ms: u64,
    /// The time its rates have held its requests and writers, in
    /// milliseconds.
    pub held_ms: u64,
}

/// What a service is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Not running, and not to be started: stopped by a request, ended
    /// for good, or refused (see `why`).
    #[default]
    Stopped,
    /// Not running, and not started at boot.
    Disabled,
    /// To start once what it waits for is ready (see `waiting_for`).
    Pending,
    /// Running, and not yet ready.
    Starting,
    /// Running, and ready.
    Running,
    /// Ended as it should, as a setup step does: ready.
    Done,
    /// To start again after a wait, as its restart setting says.
    Waiting,
    /// Ended, or asked to end, while what it left in its domain ends.
    Stopping,
    /// Given up on after failed starts in a row.
    Failed,
    /// Not running, and waiting for a connection at its sockets, which
    /// starts it (ADR 0477).
    Listening,
}

impl State {
    /// The word for it, as the document writes it.
    pub fn word(self) -> &'static str {
        match self {
            State::Stopped => "stopped",
            State::Disabled => "disabled",
            State::Pending => "pending",
            State::Starting => "starting",
            State::Running => "running",
            State::Done => "done",
            State::Waiting => "waiting",
            State::Stopping => "stopping",
            State::Failed => "failed",
            State::Listening => "listening",
        }
    }
}

/// A service's health check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    /// Whether its last check passed.
    pub state: HealthState,
    /// Its failed checks in a row.
    pub failures: u32,
    /// The failed checks in a row after which Spark restarts it, its
    /// file's `health.failures`.
    pub restart_after: u32,
    /// The last line a failed check wrote, or how it ended.
    pub last_failure: Option<String>,
    /// When it was last checked.
    pub checked_ms: Option<u64>,
}

/// How a service's last health check went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    /// Not checked since it last started.
    #[default]
    Unknown,
    /// The last check passed.
    Passing,
    /// The last check failed.
    Failing,
}

/// A shutdown set for later.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shutdown {
    /// What it ends in: "power off", "restart" or "halt".
    pub end: String,
    /// How long until it begins.
    pub in_seconds: u64,
}

/// Settings being tried (ADR 0472).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trial {
    /// Its name, which `keep` and `undo` give: `service.NAME` for one
    /// that `service try NAME` began.
    pub id: String,
    /// The files whose copies Spark keeps, to put back.
    pub files: Vec<String>,
    /// The services an undo restarts, stops or reloads.
    pub services: Vec<String>,
    /// The user it was begun for.
    pub user: u32,
    /// How long until it is undone unless it is kept.
    pub left_ms: u64,
}

/// A trial that was undone (ADR 0472).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Undone {
    /// Its name.
    pub id: String,
    /// Why, as Spark's line says it: "it was not kept in time".
    pub why: String,
    /// When.
    pub at_ms: u64,
}

/// The document for `show`, as Spark writes it.
///
/// # Errors
///
/// What serde_json says, which these types never give it cause to.
pub fn to_json(show: &Show) -> Result<String, String> {
    serde_json::to_string(show).map_err(|err| err.to_string())
}

/// Reads Spark's answer to `show`: the document, or what Spark said was
/// wrong, as "no service is called web".
///
/// # Errors
///
/// Spark's error, a document of a later version, or one that does not
/// read.
pub fn parse(answer: &str) -> Result<Show, String> {
    let answer = answer.trim_end();
    if let Some(error) = answer.strip_prefix("error: ") {
        return Err(error.to_string());
    }
    let show: Show = serde_json::from_str(answer)
        .map_err(|err| format!("Spark's answer does not read: {err}"))?;
    if show.show_version > VERSION {
        return Err(format!(
            "Spark answers with version {} of its document, and this program reads version \
             {VERSION}: it is older than Spark",
            show.show_version
        ));
    }
    Ok(show)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web() -> Service {
        Service {
            name: "web".into(),
            description: "A web server".into(),
            enabled: true,
            order: 60,
            restart: "on-failure".into(),
            requires: ["db".into()].into(),
            ready_when: "listening".into(),
            state: State::Pending,
            waiting_for: ["db".into()].into(),
            started_by: Some("boot".into()),
            health: Some(Health {
                state: HealthState::Failing,
                failures: 2,
                restart_after: 3,
                ..Health::default()
            }),
            cause: Some("db".into()),
            last_line: Some("web: cannot reach db".into()),
            last_request: Some(Request { verb: "start".into(), user: 0, process: 81, ms: 50_000 }),
            holds: Holds {
                account: Some("org.example.web".into()),
                changes: ["/system/state/org.example.web".into()].into(),
                secrets: ["tls-key".into()].into(),
                listen: ["80".into(), "8000-8010".into()].into(),
                cpu_weight: 100,
                io_weight: 400,
                io_read: Some(50 << 20),
                threads: 64,
                open_files: Some(10_000),
                ..Holds::default()
            },
            usage: Some(Usage {
                memory_bytes: 12 << 20,
                memory_peak_bytes: 64 << 20,
                memory_refused: 3,
                stopped_at_limit: true,
                threads: 4,
                read_bytes: 1 << 20,
                held_ms: 250,
                ..Usage::default()
            }),
            ..Service::default()
        }
    }

    #[test]
    fn a_document_reads_back_as_it_was_written() {
        let e1000d = Service {
            name: "e1000d".into(),
            state: State::Stopped,
            why: Some("the kernel left it none of the cards it drives".into()),
            off_as_meant: true,
            ..Service::default()
        };
        let show = Show {
            show_version: VERSION,
            now_ms: 51_234,
            services: [
                web(),
                e1000d,
                Service {
                    name: "db".into(),
                    state: State::Waiting,
                    retry_ms: Some(52_000),
                    ..Service::default()
                },
                Service {
                    name: "cache".into(),
                    state: State::Starting,
                    pid: Some(90),
                    ready_by_ms: Some(61_234),
                    ..Service::default()
                },
            ]
            .into(),
            shutdown: Some(Shutdown { end: "power off".into(), in_seconds: 60 }),
            trials: [Trial {
                id: "service.web".into(),
                files: ["/system/settings/spark/services/web.toml".into()].into(),
                services: ["web".into()].into(),
                user: 0,
                left_ms: 42_000,
            }]
            .into(),
            undone: [Undone {
                id: "network".into(),
                why: "it was not kept in time".into(),
                at_ms: 40_000,
            }]
            .into(),
        };
        let json = to_json(&show).unwrap();
        assert!(json.contains(r#""state":"pending""#), "{json}");
        assert!(json.contains(r#""off_as_meant":true"#), "{json}");
        assert!(
            json.contains(r#""health":{"state":"failing","failures":2,"restart_after":3"#),
            "{json}"
        );
        assert!(json.contains(r#""retry_ms":52000"#), "{json}");
        assert!(json.contains(r#""ready_by_ms":61234"#), "{json}");
        assert!(json.contains(r#""listen":["80","8000-8010"]"#), "{json}");
        assert!(json.contains(r#""io_read":52428800"#), "{json}");
        assert!(json.contains(r#""stopped_at_limit":true"#), "{json}");
        assert!(json.contains(r#""trials":[{"id":"service.web","#), "{json}");
        assert!(json.contains(r#""undone":[{"id":"network","why":"it was not kept"#), "{json}");
        assert_eq!(parse(&format!("{json}\n")), Ok(show));
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_take_their_defaults() {
        let show = parse(
            r#"{"show_version":1,"services":[{"name":"logd","state":"running","pid":7,"mood":"good"}],"later":true}"#,
        )
        .unwrap();
        let logd = &show.services[0];
        assert_eq!((logd.name.as_str(), logd.state, logd.pid), ("logd", State::Running, Some(7)));
        assert_eq!((logd.health.as_ref(), logd.requires.len()), (None, 0));
        assert_eq!((logd.usage.as_ref(), logd.holds.open_files), (None, None));
    }

    #[test]
    fn spark_s_error_and_a_later_version_are_refused() {
        assert_eq!(
            parse("error: no service is called web\n"),
            Err("no service is called web".into())
        );
        let later = parse(r#"{"show_version":2,"services":[]}"#).unwrap_err();
        assert!(later.contains("version 2"), "{later}");
        assert!(parse("web\trunning").is_err());
    }

    #[test]
    fn each_state_has_the_word_the_document_writes() {
        for state in [
            State::Stopped,
            State::Disabled,
            State::Pending,
            State::Starting,
            State::Running,
            State::Done,
            State::Waiting,
            State::Stopping,
            State::Failed,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, format!("\"{}\"", state.word()));
        }
    }
}
