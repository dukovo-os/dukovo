// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! troubleshoot(8): the likely cause of a start that went wrong (ADR
//! 0474). It gathers the facts of one start ([`facts`]): its record and
//! what changed since the last good start, its log, which services have
//! failed and how full each filesystem is. Each signature
//! ([`signatures`], troubleshoot-signatures(5)) whose every condition
//! holds is a match ([`matching`]), and the best are said, each with its
//! error ID, its fix and its support page. Nothing is sent anywhere.
//!
//! The failure notice, Rescue's menu and `service explain` ask for the
//! best match too ([`likely_cause`], [`best_for_service`],
//! [`for_rescue`]).

pub mod facts;
pub mod glob;
pub mod matching;
pub mod signatures;

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use spark_api::show::{self, State};

use crate::args::{Arg, Args};
use crate::power;
use crate::sysupdate::starts::{self, Outcome, Start};
use crate::util::{Fail, fail};
use facts::{Facts, Line};
use signatures::Source;

const USAGE: &str =
    "usage: troubleshoot [--start N] [--all] [--json] [--facts DIR] [--save-facts DIR] | --about";

/// How many matches are said without `--all`.
const BEST: usize = 3;

/// How long the failure notice and `service explain` wait for the facts.
const WAIT: Duration = Duration::from_secs(20);

/// The page of an error ID.
#[must_use]
pub fn help_url(id: &str) -> String {
    format!("https://dukovo.org/support/{}/errors/{id}.html", env!("CARGO_PKG_VERSION"))
}

/// One match: what is probably wrong and what to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hit {
    pub id: String,
    pub cause: String,
    pub fix: String,
}

impl Hit {
    fn of(found: &matching::Match<'_>) -> Hit {
        let signature = found.signature;
        Hit { id: signature.id.clone(), cause: signature.cause.clone(), fix: signature.fix.clone() }
    }

    /// The line of the failure notice.
    #[must_use]
    pub fn line(&self) -> String {
        let cause = self.cause.trim_end_matches('.');
        format!("Likely cause: {cause} ({}). Fix: {}", self.id, self.fix)
    }
}

/// Runs `work` in a thread, and waits at most [`WAIT`] for it, so that
/// a log or a Spark that does not answer cannot hold up what asks.
fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(work());
    });
    receive.recv_timeout(WAIT).ok()
}

/// The best match for `start`, of `records`, on this system's facts: for
/// the failure notice.
#[must_use]
pub fn likely_cause(records: &[Start], start: &Start) -> Option<Hit> {
    let (records, start) = (records.to_vec(), start.clone());
    within(move || {
        let loaded = signatures::load();
        let facts = Facts::gather(&records, &start, &loaded.signatures.full_paths());
        matching::matching(&loaded.signatures, &facts).first().map(Hit::of)
    })
    .flatten()
}

/// The best match for what is known of service `name` alone: its log
/// lines and whether it failed, without the start's own outcome, for
/// `service explain`.
#[must_use]
pub fn best_for_service(name: &str) -> Option<Hit> {
    let name = name.to_string();
    within(move || {
        let records = starts::all();
        let newest = records.last()?;
        let loaded = signatures::load();
        let mut facts = Facts::gather(&records, newest, &loaded.signatures.full_paths());
        facts.start.outcome = Outcome::Good;
        facts.start.error = None;
        facts.changed.clear();
        facts.log.retain(|line| line.from == "service" && line.name == name);
        facts.failed.retain(|failed| *failed == name);
        matching::matching(&loaded.signatures, &facts).first().map(Hit::of)
    })
    .flatten()
}

/// What Rescue says of the likely cause, in lines: `records` are those
/// of the root's state, and `unusable` is why Rescue cannot use the
/// root, if so, which it says as a line of its own log. A newer file of
/// signatures than Rescue's own, kept in the root's state at `kept`, is
/// the one used.
#[must_use]
pub fn for_rescue(
    records: &[Start],
    unusable: Option<&str>,
    uuid: &str,
    kept: &std::path::Path,
) -> Vec<String> {
    let loaded = signatures::load_in(std::path::Path::new(signatures::SHIPPED), kept);
    rescue_lines(&loaded.signatures, records, unusable, uuid)
}

/// [`for_rescue`], with the signatures to go by.
fn rescue_lines(
    set: &signatures::Signatures,
    records: &[Start],
    unusable: Option<&str>,
    uuid: &str,
) -> Vec<String> {
    let start =
        records.iter().rev().find(|start| start.outcome != Outcome::Good).or(records.last());
    let mut facts = match start {
        Some(start) => Facts::of(start),
        None => Facts::of(&Start {
            number: 0,
            started: 0,
            slot: "a".into(),
            stage: "kernel".into(),
            outcome: Outcome::Failed,
            error: None,
            why: None,
            items: Vec::new(),
            times: None,
        }),
    };
    if let Some(start) = start
        && let Some(good) = starts::last_good(records, start.number)
    {
        facts.changed = starts::changes(records, good, start).into_iter().map(|c| c.kind).collect();
    }
    if let Some(why) = unusable {
        facts.log.push(Line {
            level: 3,
            from: "program".into(),
            name: "dukovo-rescue".into(),
            text: format!("the Dukovo root {uuid} cannot be used: {why}"),
        });
    }
    let found = matching::matching(set, &facts);
    if found.is_empty() {
        return vec!["No known cause matches what Rescue can see.".into()];
    }
    found.iter().take(BEST).flat_map(|found| lines_of(found)).collect()
}

/// The lines that say one match.
fn lines_of(found: &matching::Match<'_>) -> Vec<String> {
    let signature = found.signature;
    vec![
        format!("Likely cause: {} ({})", signature.cause.trim_end_matches('.'), signature.id),
        format!("Fix: {}", signature.fix),
        format!("More: {}", help_url(&signature.id)),
    ]
}

/// Which start `troubleshoot` takes when none is named.
#[derive(Debug, PartialEq, Eq)]
enum Pick<'a> {
    /// No start is recorded.
    Nothing,
    /// This one: the newest that is not good, or the newest if every one is.
    Start(&'a Start),
    /// The start of this boot is still being checked: its record is not
    /// good only because the health check has not seen it through. This
    /// is the one before it, which is shown instead.
    Before { checking: u64, shown: &'a Start },
    /// The start of this boot, still being checked, is the only one
    /// recorded.
    OnlyChecking(u64),
}

/// The start to take of `records` when none is named, oldest first.
/// `this` is the number of this boot's start, if the health check has
/// begun its record, and `checking` whether the health check is running.
/// While it is, this boot's record says nothing yet, and the start before
/// it is the one to look at. A start of an earlier boot that did not end
/// is kept, as is this boot's when the health check is not running: it
/// did not end, and that is what has gone wrong.
fn pick(records: &[Start], this: Option<u64>, checking: bool) -> Pick<'_> {
    // Only an unfinished record: one the health check has finished says
    // what it found, though the check may not have ended yet.
    let passed = this
        .filter(|_| checking)
        .filter(|n| records.iter().any(|r| r.number == *n && r.outcome == Outcome::Unfinished));
    let rest: Vec<&Start> = records.iter().filter(|r| Some(r.number) != passed).collect();
    let chosen =
        rest.iter().rev().find(|record| record.outcome != Outcome::Good).or(rest.last()).copied();
    match (passed, chosen) {
        (Some(checking), Some(shown)) => Pick::Before { checking, shown },
        (Some(checking), None) => Pick::OnlyChecking(checking),
        (None, Some(chosen)) => Pick::Start(chosen),
        (None, None) => Pick::Nothing,
    }
}

/// Whether `state` of the health check's service is one in which it is
/// at work on a start: starting, running, or waiting to run again.
fn is_checking(state: State) -> bool {
    matches!(state, State::Starting | State::Running | State::Waiting)
}

/// Whether Spark says the health check, the service `sysupdate`, is at
/// work. A Spark that does not answer says it is not.
fn health_check_running() -> bool {
    power::request("show").ok().and_then(|answer| show::parse(&answer).ok()).is_some_and(
        |document| {
            document
                .services
                .iter()
                .any(|service| service.name == "sysupdate" && is_checking(service.state))
        },
    )
}

/// troubleshoot(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut start: Option<u64> = None;
    let (mut all, mut json, mut about) = (false, false, false);
    let (mut from, mut save): (Option<PathBuf>, Option<PathBuf>) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "start" => start = Some(args.number()?),
            Arg::Long(name) if name == "all" => all = true,
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "about" => about = true,
            Arg::Long(name) if name == "facts" => from = Some(args.value()?.into()),
            Arg::Long(name) if name == "save-facts" => save = Some(args.value()?.into()),
            Arg::Long(name) if name == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let loaded = signatures::load();
    if about {
        print!("{}", about_text(&loaded));
        return Ok(0);
    }
    // The start of this boot, when it is passed over for the one before.
    let mut checking = None;
    let mut facts = match from {
        Some(dir) => Facts::load(&dir)?,
        None => {
            let records = starts::all();
            let chosen = match start {
                Some(number) => records
                    .iter()
                    .find(|record| record.number == number)
                    .ok_or_else(|| fail(format!("no record of start {number} is kept")))?,
                None => {
                    let this = starts::this_start();
                    let running = this.is_some() && health_check_running();
                    match pick(&records, this, running) {
                        Pick::Start(chosen) => chosen,
                        Pick::Before { checking: number, shown } => {
                            checking = Some(number);
                            shown
                        }
                        Pick::OnlyChecking(number) => {
                            println!(
                                "troubleshoot: start {number} is still being checked, and no \
                                 other start is recorded yet"
                            );
                            return Ok(1);
                        }
                        Pick::Nothing => {
                            println!("troubleshoot: no start has been recorded yet");
                            return Ok(1);
                        }
                    }
                }
            };
            Facts::gather(&records, chosen, &loaded.signatures.full_paths())
        }
    };
    if let Some(dir) = save {
        facts.save(&dir)?;
    }
    let searched = matching::search(&loaded.signatures, &facts);
    // Said with what could not be found out, but not saved with the facts.
    if searched.cut_short {
        facts.missing.push(matching::CUT_SHORT.into());
    }
    let found = searched.matches;
    if json {
        println!("{}", json_text(&facts, &loaded, &found, checking)?);
    } else {
        if let Some(number) = checking {
            println!(
                "Start {number} is still being checked; showing start {}.\n",
                facts.start.number
            );
        }
        print!("{}", text(&facts, &found, all));
    }
    Ok(i32::from(found.is_empty()))
}

/// What `--about` says of the signatures in use.
fn about_text(loaded: &signatures::Loaded) -> String {
    let set = &loaded.signatures;
    let from = match loaded.source {
        Source::Shipped => "the file this release ships",
        Source::Kept => "a newer file the update check kept",
    };
    let mut text = format!(
        "Signatures: version {}, from {from}.\n{} signatures are read",
        set.version,
        set.signatures.len()
    );
    if set.skipped > 0 {
        text.push_str(&format!(
            ", and {} more are skipped: each has a condition this release cannot read",
            set.skipped
        ));
    }
    text.push_str(".\n");
    for note in &loaded.notes {
        text.push_str(&format!("Left out: {note}\n"));
    }
    text
}

/// What is said of the `found` matches of `facts`.
fn text(facts: &Facts, found: &[matching::Match<'_>], all: bool) -> String {
    let mut text = format!("{}\n", crate::changes::start_line(&facts.start));
    if found.is_empty() {
        text.push_str("No known cause matches this start.\n");
        text.push_str(
            "changes says what changed since the last good start, and log what was written.\n",
        );
    }
    let shown = if all { found.len() } else { found.len().min(BEST) };
    for (at, one) in found.iter().take(shown).enumerate() {
        let signature = one.signature;
        text.push('\n');
        let heading = if at == 0 { "Likely cause" } else { "Also possible" };
        text.push_str(&format!(
            "{heading}: {} ({})\n",
            signature.cause.trim_end_matches('.'),
            signature.id
        ));
        text.push_str(&format!("Fix: {}\n", signature.fix));
        text.push_str(&format!("More: {}\n", help_url(&signature.id)));
    }
    if shown < found.len() {
        text.push_str(&format!("\n{} more match; --all says them.\n", found.len() - shown));
    }
    for why in &facts.missing {
        text.push_str(&format!("\nNot looked at: {why}\n"));
    }
    text
}

/// `--json`: the start, the signatures in use and the matches, best first.
fn json_text(
    facts: &Facts,
    loaded: &signatures::Loaded,
    found: &[matching::Match<'_>],
    checking: Option<u64>,
) -> Result<String, Fail> {
    #[derive(Serialize)]
    struct Said<'a> {
        start: u64,
        /// This boot's start, which the health check is still checking, and
        /// this one is shown in its place.
        #[serde(skip_serializing_if = "Option::is_none")]
        checking: Option<u64>,
        outcome: Outcome,
        error: Option<&'a str>,
        version: u64,
        skipped: usize,
        matches: Vec<Said2>,
        missing: &'a [String],
    }
    #[derive(Serialize)]
    struct Said2 {
        id: String,
        cause: String,
        fix: String,
        help: String,
        own: bool,
    }
    let said = Said {
        start: facts.start.number,
        checking,
        outcome: facts.start.outcome,
        error: facts.start.error.as_deref(),
        version: loaded.signatures.version,
        skipped: loaded.signatures.skipped,
        matches: found
            .iter()
            .map(|one| Said2 {
                id: one.signature.id.clone(),
                cause: one.signature.cause.clone(),
                fix: one.signature.fix.clone(),
                help: help_url(&one.signature.id),
                own: one.own,
            })
            .collect(),
        missing: &facts.missing,
    };
    serde_json::to_string(&said).map_err(|err| fail(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Where the facts of the planted faults are kept, one folder each.
    fn faults() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/troubleshoot-faults")
    }

    fn shipped() -> signatures::Signatures {
        let text = include_str!("../../../troubleshoot/signatures.toml");
        signatures::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn the_signatures_the_release_ships_all_read_and_none_is_skipped() {
        let set = shipped();
        assert_eq!(set.skipped, 0);
        let ids: Vec<&str> = set.signatures.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "FW-101", "FW-102", "FW-103", "BOOT-102", "BOOT-104", "BOOT-105", "BOOT-107",
                "BOOT-108"
            ]
        );
        // Each says what is wrong and what to do, and ends in a full stop.
        for signature in &set.signatures {
            assert!(
                signature.cause.ends_with('.') && signature.fix.ends_with('.'),
                "{}",
                signature.id
            );
        }
    }

    #[test]
    fn each_signature_matches_the_facts_of_its_planted_fault_and_no_other() {
        let set = shipped();
        let mut proved = Vec::new();
        for entry in std::fs::read_dir(faults()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            let facts = Facts::load(&entry.path()).unwrap();
            let found: Vec<String> =
                matching::matching(&set, &facts).iter().map(|m| m.signature.id.clone()).collect();
            if name == "healthy" {
                assert!(found.is_empty(), "a healthy start matches {found:?}");
                continue;
            }
            // The folder is named for the ID it plants, in lower case.
            let id = name.to_uppercase();
            assert_eq!(found, std::slice::from_ref(&id), "fault {name}");
            proved.push(id);
        }
        proved.sort();
        let mut every: Vec<String> = set.signatures.iter().map(|s| s.id.clone()).collect();
        every.sort();
        assert_eq!(proved, every, "a signature has no planted fault");
    }

    #[test]
    fn a_fault_does_not_match_when_a_piece_of_it_is_missing() {
        let set = shipped();
        let load = |name: &str| Facts::load(&faults().join(name)).unwrap();
        let matches = |facts: &Facts| -> Vec<String> {
            matching::matching(&set, facts).iter().map(|m| m.signature.id.clone()).collect()
        };
        // A full disk that said nothing, and a message on a disk that is not full.
        let mut full = load("boot-104");
        full.log.clear();
        assert!(matches(&full).is_empty());
        let mut full = load("boot-104");
        full.full[1].percent = 97;
        assert!(matches(&full).is_empty());
        let mut full = load("boot-104");
        full.full.retain(|f| f.path != "/system/state");
        assert!(matches(&full).is_empty());
        // The kernel's line written by a program, or only an information.
        let mut io = load("boot-107");
        for line in &mut io.log {
            line.from = "program".into();
        }
        assert!(matches(&io).is_empty());
        let mut io = load("boot-107");
        for line in &mut io.log {
            line.level = 6;
        }
        assert!(matches(&io).is_empty());
        // Options that did not change, and a start that was good.
        let mut options = load("boot-108");
        options.changed.clear();
        assert!(matches(&options).is_empty());
        let mut options = load("boot-108");
        options.start.outcome = Outcome::Good;
        assert!(matches(&options).is_empty());
        // A start's own error ranks first among the matches.
        let mut both = load("boot-108");
        both.start.error = Some("BOOT-102".into());
        assert_eq!(matches(&both), ["BOOT-102", "BOOT-108"]);
    }

    fn record(number: u64, outcome: Outcome) -> Start {
        Start {
            number,
            started: 1_800_000_000 + i64::try_from(number).unwrap(),
            slot: "a".into(),
            stage: "services".into(),
            outcome,
            error: None,
            why: None,
            items: Vec::new(),
            times: None,
        }
    }

    #[test]
    fn the_start_taken_is_the_newest_that_is_not_good_but_not_one_being_checked() {
        use Outcome::{Failed, Good, Unfinished};
        let number = |pick: Pick<'_>| match pick {
            Pick::Start(start) => ("start", start.number, 0),
            Pick::Before { checking, shown } => ("before", shown.number, checking),
            Pick::OnlyChecking(checking) => ("only", 0, checking),
            Pick::Nothing => ("nothing", 0, 0),
        };
        let records = [record(1, Good), record(2, Failed), record(3, Good), record(4, Unfinished)];
        // The health check is at work on this boot's start 4: start 2 is shown.
        assert_eq!(number(pick(&records, Some(4), true)), ("before", 2, 4));
        // It is not, so start 4 did not end, which is what went wrong.
        assert_eq!(number(pick(&records, Some(4), false)), ("start", 4, 0));
        // This boot's start is not known: the newest that is not good.
        assert_eq!(number(pick(&records, None, true)), ("start", 4, 0));
        // The unfinished start belongs to an earlier boot: it is kept.
        assert_eq!(number(pick(&records, Some(5), true)), ("start", 4, 0));
        // Nothing before this boot's start went wrong: the one before it.
        let good = [record(1, Good), record(2, Good), record(3, Unfinished)];
        assert_eq!(number(pick(&good, Some(3), true)), ("before", 2, 3));
        assert_eq!(number(pick(&good, Some(3), false)), ("start", 3, 0));
        // The unfinished start of an earlier boot is before this boot's.
        let two = [record(1, Good), record(2, Unfinished), record(3, Unfinished)];
        assert_eq!(number(pick(&two, Some(3), true)), ("before", 2, 3));
        // This boot's start failed its check: the check is over, whatever is said.
        let over = [record(1, Good), record(2, Failed)];
        assert_eq!(number(pick(&over, Some(2), false)), ("start", 2, 0));
        // Also while the health check has yet to end, once it has finished
        // the record.
        assert_eq!(number(pick(&over, Some(2), true)), ("start", 2, 0));
        // Only this boot's start, being checked, and nothing at all.
        assert_eq!(number(pick(&[record(1, Unfinished)], Some(1), true)), ("only", 0, 1));
        assert_eq!(number(pick(&[], Some(1), true)), ("nothing", 0, 0));
        assert_eq!(number(pick(&[], None, false)), ("nothing", 0, 0));
    }

    #[test]
    fn the_health_check_is_at_work_while_it_starts_runs_or_waits_to_run_again() {
        for state in [State::Starting, State::Running, State::Waiting] {
            assert!(is_checking(state), "{state:?}");
        }
        for state in [
            State::Stopped,
            State::Disabled,
            State::Pending,
            State::Done,
            State::Stopping,
            State::Failed,
        ] {
            assert!(!is_checking(state), "{state:?}");
        }
    }

    #[test]
    fn a_hit_says_cause_id_and_fix_in_a_line() {
        let hit = Hit {
            id: "BOOT-104".into(),
            cause: "The disk is full.".into(),
            fix: "Make room.".into(),
        };
        assert_eq!(hit.line(), "Likely cause: The disk is full (BOOT-104). Fix: Make room.");
    }

    #[test]
    fn the_text_says_the_best_three_or_all() {
        let set = shipped();
        let mut facts = Facts::load(&faults().join("boot-108")).unwrap();
        facts.start.error = Some("BOOT-102".into());
        facts.log.push(Line {
            level: 4,
            from: "kernel".into(),
            name: "kernel".into(),
            text: "Disk: vda: a flush failed: an I/O error".into(),
        });
        let found = matching::matching(&set, &facts);
        assert_eq!(found.len(), 3);
        let said = text(&facts, &found, false);
        assert!(said.contains("Likely cause: An essential service ended"), "{said}");
        assert!(said.contains("Also possible: A disk reported errors"), "{said}");
        assert!(said.contains("More: https://dukovo.org/support/"), "{said}");
        assert!(!said.contains("more match"), "{said}");
        // Said one at a time, then all.
        let mut more = facts.clone();
        more.full = vec![facts::Full { path: "/system/state".into(), percent: 99 }];
        more.log.push(Line {
            level: 3,
            from: "service".into(),
            name: "logd".into(),
            text: "No space left on device".into(),
        });
        let found = matching::matching(&set, &more);
        assert_eq!(found.len(), 4);
        assert!(text(&more, &found, false).contains("1 more match; --all says them"));
        assert!(!text(&more, &found, true).contains("more match"));
        // Nothing known.
        let nothing = Facts::load(&faults().join("healthy")).unwrap();
        let said = text(&nothing, &[], false);
        assert!(said.contains("No known cause"), "{said}");
    }

    #[test]
    fn rescue_says_the_cause_of_a_root_it_cannot_use_and_of_a_failed_start() {
        let set = shipped();
        let lines = rescue_lines(&set, &[], Some("no disk here holds it"), "6fd3a1c2");
        assert!(lines[0].starts_with("Likely cause: The Dukovo root did not mount"), "{lines:?}");
        assert!(lines.iter().any(|line| line.starts_with("Fix: ")));
        assert!(lines.iter().any(|line| line.starts_with("More: https://dukovo.org/support/")));
        let nothing = rescue_lines(&set, &[], None, "6fd3a1c2");
        assert_eq!(nothing, ["No known cause matches what Rescue can see."]);
        let facts = Facts::load(&faults().join("fw-102")).unwrap();
        let lines = rescue_lines(&set, std::slice::from_ref(&facts.start), None, "6fd3a1c2");
        assert!(lines[0].contains("(FW-102)"), "{lines:?}");
    }

    #[test]
    fn about_says_the_version_and_what_was_skipped_or_left_out() {
        let loaded = signatures::Loaded {
            signatures: signatures::Signatures { version: 4, signatures: Vec::new(), skipped: 2 },
            source: Source::Kept,
            notes: vec!["/x: broken".into()],
        };
        let said = about_text(&loaded);
        assert!(said.contains("version 4, from a newer file the update check kept"), "{said}");
        assert!(said.contains("and 2 more are skipped"), "{said}");
        assert!(said.contains("Left out: /x: broken"), "{said}");
    }

    #[test]
    fn json_has_the_start_and_each_match() {
        let loaded = signatures::Loaded {
            signatures: shipped(),
            source: Source::Shipped,
            notes: Vec::new(),
        };
        let facts = Facts::load(&faults().join("fw-101")).unwrap();
        let found = matching::matching(&loaded.signatures, &facts);
        let said = json_text(&facts, &loaded, &found, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&said).unwrap();
        assert_eq!(value["start"], 12);
        assert!(value.get("checking").is_none());
        let passed = json_text(&facts, &loaded, &found, Some(13)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&passed).unwrap();
        assert_eq!(value["start"], 12);
        assert_eq!(value["checking"], 13);
        assert_eq!(value["matches"][0]["id"], "FW-101");
        assert_eq!(value["matches"][0]["own"], true);
        assert!(value["matches"][0]["help"].as_str().unwrap().ends_with("/errors/FW-101.html"));
    }
}
