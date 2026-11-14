// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What reportd gathers for a draft (ADR 0531), as root, and only what
//! the person who asks may read: the crashes of programs that ran as
//! them; a service's log, the failed start and every crash for an
//! administrator.

use super::store::Store;
use crate::logd::{self, Record, Request};
use crate::reports::draft::Parts;
use crate::reports::facts::Facts;
use crate::reports::protocol::{About, Kind};

/// The lines of a service's log a report holds.
const LOG_LINES: usize = 200;
/// The most warnings and errors of a failed start a report holds.
const START_LINES: usize = 200;
/// The most lines of a failure notice, and the most characters of each,
/// that a report holds.
const NOTICE_LINES: usize = 100;
const NOTICE_WIDTH: usize = 400;

/// Who asks for a draft.
#[derive(Clone, Copy, Debug)]
pub struct Asker {
    pub user: u32,
    pub admin: bool,
}

/// The parts of a report about `about` for `asker`, with the machine's
/// `facts`.
///
/// # Errors
///
/// What `asker` may not read, or a crash that is not kept.
pub fn gather(about: &About, asker: Asker, facts: Facts, store: &Store) -> Result<Parts, String> {
    let mut parts = Parts { facts, ..Parts::default() };
    match about {
        About::Crash { id } => {
            let kept = store
                .get(id)
                .filter(|kept| asker.admin || kept.crash.user == asker.user)
                .ok_or_else(|| format!("no crash {id} is kept that you may report"))?;
            let crash = &kept.crash;
            let program = match &crash.version {
                Some(version) if crash.kind != Kind::Kernel => {
                    format!("{} {version}", crash.program)
                }
                _ => crash.program.clone(),
            };
            parts.about = match (&crash.service, crash.kind) {
                (_, Kind::Kernel) => "a panic of the kernel".into(),
                (Some(service), _) => format!("a crash of {program} (the service {service})"),
                (None, _) => format!("a crash of {program}"),
            };
            parts.crash = Some(kept.lines.clone());
            if let Some(service) = crash.service.as_deref().filter(|_| asker.admin) {
                parts.log = service_log(service);
            }
        }
        About::Service { name } => {
            if !asker.admin {
                return Err("only an administrator may report a service, whose log goes with it; \
                     try elevate report"
                    .into());
            }
            if !crate::servicefiles::name_ok(name) {
                return Err(format!("{name} is not a service's name"));
            }
            parts.about = format!("the service {name}");
            parts.log = service_log(name);
        }
        About::Program { name } => {
            let name = crate::notifyd::clean(name, 64, false);
            parts.about = format!("the command or app {name}");
        }
        About::Hardware => parts.about = "the hardware".into(),
        About::Updates => {
            parts.about = "installing or updating Dukovo".into();
            if asker.admin {
                parts.log = service_log("sysupdate");
            }
        }
        About::Failure { notice } => {
            if !asker.admin {
                return Err("only an administrator may report a failed start".into());
            }
            if notice.iter().all(|line| line.trim().is_empty()) {
                return Err("no start has failed since this one began".into());
            }
            parts.about = "a failed start".into();
            parts.notice = Some(
                notice
                    .iter()
                    .take(NOTICE_LINES)
                    .map(|line| crate::notifyd::clean(line, NOTICE_WIDTH, false))
                    .collect(),
            );
            parts.start_log = Some(failed_start_log());
        }
        About::Other => parts.about = "something else".into(),
    }
    Ok(parts)
}

/// The last lines of this boot's log of service `name`, with its heading.
fn service_log(name: &str) -> Option<(String, Vec<String>)> {
    let lines = crate::log::recent(name, LOG_LINES);
    (!lines.is_empty())
        .then(|| (format!("Log of the service {name} (last {LOG_LINES} lines)"), lines))
}

/// The warnings and errors of the start before this one.
fn failed_start_log() -> Vec<String> {
    let request = Request {
        boot: "-1".into(),
        level: logd::level_number("warning"),
        lines: Some(START_LINES),
        ..Request::default()
    };
    let mut lines = Vec::new();
    let _ = logd::ask(&request, |line| {
        if let Ok(record) = serde_json::from_str::<Record>(line) {
            lines.push(line_of(&record));
        }
        Ok(true)
    });
    lines
}

/// A record as a line: `10:02:11 warning name: text`.
fn line_of(record: &Record) -> String {
    let time = record.time.get(11..19).unwrap_or(&record.time);
    format!("{time} {} {}: {}", record.level, record.name, record.text)
}

#[cfg(test)]
mod tests {
    use super::super::store::Kept;
    use super::*;
    use crate::reports::protocol::Crash;

    fn store(test: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("reportd-parts-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        store.make().unwrap();
        (store, dir)
    }

    fn kept(user: u32) -> Kept {
        Kept {
            crash: Crash {
                id: "20261113T100211-4012".into(),
                kind: Kind::Signal,
                program: "crash-test".into(),
                version: Some("0.16.0".into()),
                service: None,
                user,
                at: 1_763_028_131,
                what: "SIGSEGV".into(),
                signature: "aaaa".into(),
                seen: 1,
                reported: None,
            },
            signature: "crash-test SIGSEGV".into(),
            lines: vec!["crash-test stopped: SIGSEGV".into()],
            forgotten: false,
            offered: true,
        }
    }

    #[test]
    fn a_person_reports_their_own_crashes_and_an_administrator_any() {
        let (store, dir) = store("own");
        store.put(&kept(1000)).unwrap();
        let about = About::Crash { id: "20261113T100211-4012".into() };
        let alice = Asker { user: 1000, admin: false };
        let bob = Asker { user: 1001, admin: false };
        let admin = Asker { user: 1002, admin: true };
        let parts = gather(&about, alice, Facts::default(), &store).unwrap();
        assert_eq!(parts.about, "a crash of crash-test 0.16.0");
        assert_eq!(parts.crash.unwrap(), ["crash-test stopped: SIGSEGV"]);
        assert!(gather(&about, bob, Facts::default(), &store).is_err());
        assert!(gather(&about, admin, Facts::default(), &store).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_services_log_and_a_failed_start_are_an_administrators() {
        let (store, dir) = store("admin");
        let alice = Asker { user: 1000, admin: false };
        let service = About::Service { name: "web".into() };
        assert!(gather(&service, alice, Facts::default(), &store).unwrap_err().contains("elevate"));
        let failure = About::Failure { notice: vec!["Start 2 failed".into()] };
        assert!(gather(&failure, alice, Facts::default(), &store).is_err());
        let program = About::Program { name: "ls\nInjected: line".into() };
        let parts = gather(&program, alice, Facts::default(), &store).unwrap();
        assert!(!parts.about.contains('\n'));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
