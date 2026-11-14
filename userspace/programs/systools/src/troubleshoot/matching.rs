// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which signatures hold of the facts of a start (ADR 0474), and which
//! of them to say first.

use super::facts::Facts;
use super::signatures::{Condition, Failed, Signature, Signatures};

/// A signature all of whose conditions hold.
#[derive(Debug)]
pub struct Match<'a> {
    pub signature: &'a Signature,
    /// Whether it names the error ID of the start itself.
    pub own: bool,
}

/// How much work matching may do in all, in steps: one for each line of
/// the log looked at, and one for each byte of a line a glob reads. A
/// glob does the same few word operations for each byte, whatever it is
/// and whatever came before (glob.rs), so this bounds the time matching
/// takes, a second or two in a build with optimizations, however many
/// signatures there are and whatever they ask. A start's log has at most
/// 20000 lines of 400 bytes (facts.rs), and a file of signatures at most
/// 64 conditions on it (signatures.rs): every condition against every
/// line would be 500 million steps, while the shipped signatures against
/// a log of ordinary lines take a few million.
pub const WORK: u64 = 100_000_000;

/// What `troubleshoot` says when matching reached [`WORK`].
pub const CUT_SHORT: &str = "matching reached its limit of work before every signature had \
    looked through the log, so a cause that needs a line of it may be missing";

/// What matching has left it may do.
struct Budget {
    left: u64,
    /// Whether a step was asked for that it could not give.
    cut_short: bool,
}

impl Budget {
    /// Takes `steps` from what is left; whether there were that many.
    /// Nothing is taken when there are not, and what is left is then
    /// nothing, so that every later step is refused too.
    fn take(&mut self, steps: u64) -> bool {
        if steps > self.left {
            self.left = 0;
            self.cut_short = true;
            return false;
        }
        self.left -= steps;
        true
    }
}

/// Whether `condition` holds of `facts`, within `budget`. A condition on
/// the log that cannot be looked through for want of work does not hold:
/// what matches is only what was shown to.
fn holds(condition: &Condition, facts: &Facts, budget: &mut Budget) -> bool {
    match condition {
        Condition::Log { glob, service, level } => {
            for line in &facts.log {
                if !budget.take(1) {
                    return false;
                }
                let of_it = level.is_none_or(|level| line.level <= level.limit())
                    && service.as_deref().is_none_or(|service| {
                        if service == "kernel" {
                            line.from == "kernel"
                        } else {
                            line.from == "service" && line.name == service
                        }
                    });
                if !of_it {
                    continue;
                }
                // The most a glob reads of a line is all of it.
                if !budget.take(line.text.len() as u64) {
                    return false;
                }
                if glob.matches(&line.text) {
                    return true;
                }
            }
            false
        }
        Condition::Start { outcome, error } => {
            facts.start.outcome == *outcome
                && error.as_ref().is_none_or(|id| facts.start.error.as_ref() == Some(id))
        }
        Condition::Changed(kind) => facts.changed.contains(kind),
        Condition::Failed(Failed::Service(name)) => facts.failed.contains(name),
        Condition::Failed(Failed::Any) => !facts.failed.is_empty(),
        Condition::Failed(Failed::Essential) => {
            facts.failed.iter().any(|name| facts.essential.contains(name))
        }
        Condition::Full { path, percent } => {
            facts.full.iter().any(|full| &full.path == path && full.percent >= *percent)
        }
    }
}

/// The signatures that matched, and whether matching was cut short.
#[derive(Debug)]
pub struct Found<'a> {
    /// The signatures of the set whose conditions all hold, best first.
    pub matches: Vec<Match<'a>>,
    /// Whether matching reached its limit of work: signatures that needed
    /// the log after that were not matched, so there may be more causes
    /// than `matches` has.
    pub cut_short: bool,
    /// The steps taken, never more than the limit, which the tests hold
    /// matching to.
    #[cfg(test)]
    pub steps: u64,
}

/// The signatures of `set` whose conditions all hold of `facts`, best
/// first: those that name the start's own error ID, then those with the
/// most conditions, then by ID, then as the file has them. Matching does
/// at most [`WORK`] steps; when they are spent, what is matched is
/// right as far as it goes, and [`Found::cut_short`] says so.
#[must_use]
pub fn search<'a>(set: &'a Signatures, facts: &Facts) -> Found<'a> {
    search_within(set, facts, WORK)
}

/// [`search`], with at most `work` steps.
#[must_use]
pub fn search_within<'a>(set: &'a Signatures, facts: &Facts, work: u64) -> Found<'a> {
    let mut budget = Budget { left: work, cut_short: false };
    let mut matches: Vec<Match<'a>> = set
        .signatures
        .iter()
        .filter(|signature| {
            // The log is the dearest to look through: last.
            let is_log = |condition: &&Condition| matches!(condition, Condition::Log { .. });
            let (logs, others): (Vec<&Condition>, Vec<&Condition>) =
                signature.conditions.iter().partition(is_log);
            others.iter().chain(&logs).all(|condition| holds(condition, facts, &mut budget))
        })
        .map(|signature| Match {
            signature,
            own: facts.start.error.as_ref() == Some(&signature.id),
        })
        .collect();
    // A stable sort keeps the file's order among equals.
    matches.sort_by(|a, b| {
        b.own
            .cmp(&a.own)
            .then(b.signature.conditions.len().cmp(&a.signature.conditions.len()))
            .then(a.signature.id.cmp(&b.signature.id))
    });
    Found {
        matches,
        cut_short: budget.cut_short,
        #[cfg(test)]
        steps: work - budget.left,
    }
}

/// [`search`], for those that do not say whether it was cut short.
#[must_use]
pub fn matching<'a>(set: &'a Signatures, facts: &Facts) -> Vec<Match<'a>> {
    search(set, facts).matches
}

#[cfg(test)]
mod tests {
    use super::super::facts::{Full, Line};
    use super::super::signatures;
    use super::*;
    use crate::sysupdate::starts::{Item, Outcome, Start};

    fn start(outcome: Outcome, error: Option<&str>) -> Start {
        Start {
            number: 9,
            started: 1_800_000_000,
            slot: "a".into(),
            stage: "services".into(),
            outcome,
            error: error.map(str::to_string),
            why: None,
            items: vec![Item::new("system", "Dukovo", "0.15.0", None)],
            times: None,
        }
    }

    fn line(level: u8, from: &str, name: &str, text: &str) -> Line {
        Line { level, from: from.into(), name: name.into(), text: text.into() }
    }

    fn set(text: &str) -> Signatures {
        signatures::parse(format!("format = 1\nversion = 1\n{text}").as_bytes()).unwrap()
    }

    fn signature(id: &str, conditions: &str) -> String {
        format!(
            "[[signature]]\nid = \"{id}\"\ncause = \"c {id}\"\nfix = \"f\"\nmatch = [{conditions}]\n"
        )
    }

    fn ids(found: &[Match<'_>]) -> Vec<String> {
        found.iter().map(|m| m.signature.id.clone()).collect()
    }

    #[test]
    fn each_kind_of_condition_holds_of_what_it_names() {
        let mut facts = Facts::of(&start(Outcome::Failed, Some("BOOT-102")));
        facts.changed = vec!["options".into()];
        facts.log = vec![
            line(6, "service", "sshd", "listening on port 22"),
            line(3, "service", "logd", "cannot write: No space left on device"),
            line(4, "kernel", "kernel", "blk: I/O error, dev vda"),
            line(3, "program", "volumed", "volumed: lies"),
        ];
        facts.failed = vec!["netd".into(), "sshd".into()];
        facts.essential = vec!["sshd".into(), "logd".into()];
        facts.full = vec![Full { path: "/system/state".into(), percent: 99 }];
        let text = [
            signature("TST-001", r#"{ log = "*No space*" }"#),
            signature("TST-002", r#"{ log = "*No space*", service = "logd", level = "error" }"#),
            signature("TST-003", r#"{ log = "*No space*", service = "sshd" }"#),
            signature("TST-004", r#"{ log = "*I/O error*", service = "kernel" }"#),
            signature("TST-005", r#"{ log = "*I/O error*", level = "error" }"#),
            signature("TST-006", r#"{ log = "*I/O error*", level = "warning" }"#),
            signature("TST-007", r#"{ log = "*lies*", service = "volumed" }"#),
            signature("TST-008", r#"{ log = "listening on port 22" }"#),
            signature("TST-009", r#"{ log = "listening" }"#),
            signature("TST-010", r#"{ start = "failed" }"#),
            signature("TST-011", r#"{ start = "failed", error = "BOOT-102" }"#),
            signature("TST-012", r#"{ start = "failed", error = "BOOT-101" }"#),
            signature("TST-013", r#"{ start = "unfinished" }"#),
            signature("TST-014", r#"{ changed = "options" }"#),
            signature("TST-015", r#"{ changed = "app" }"#),
            signature("TST-016", r#"{ failed = "netd" }"#),
            signature("TST-017", r#"{ failed = "volumed" }"#),
            signature("TST-018", r#"{ failed = "*" }"#),
            signature("TST-019", r#"{ failed = "essential" }"#),
            signature("TST-020", r#"{ full = "/system/state", percent = 99 }"#),
            signature("TST-021", r#"{ full = "/system/state", percent = 100 }"#),
            signature("TST-022", r#"{ full = "/system", percent = 1 }"#),
        ]
        .concat();
        let mut held = ids(&matching(&set(&text), &facts));
        held.sort();
        // TST-003: sshd did not write that line. TST-005: the kernel's line is
        // a warning. TST-007: a program wrote that line, whatever it named
        // itself, and a service's is a service's. TST-009: a glob is the whole
        // line. TST-012 to 13, 15, 17, 21 and 22: not so.
        assert_eq!(
            held,
            [
                "TST-001", "TST-002", "TST-004", "TST-006", "TST-008", "TST-010", "TST-011",
                "TST-014", "TST-016", "TST-018", "TST-019", "TST-020"
            ]
        );
        // None hold of facts that say nothing.
        let nothing = Facts::of(&start(Outcome::Good, None));
        assert!(matching(&set(&text), &nothing).is_empty());
        // Essential needs the failed service to be on the list.
        let essential = set(&signature("TST-019", r#"{ failed = "essential" }"#));
        assert_eq!(matching(&essential, &facts).len(), 1);
        facts.essential = vec!["logd".into()];
        assert!(matching(&essential, &facts).is_empty());
    }

    #[test]
    fn all_conditions_must_hold() {
        let mut facts = Facts::of(&start(Outcome::Failed, None));
        facts.changed = vec!["options".into()];
        let both = set(&signature("TST-001", r#"{ start = "failed" }, { changed = "options" }"#));
        assert_eq!(ids(&matching(&both, &facts)), ["TST-001"]);
        facts.changed.clear();
        assert!(matching(&both, &facts).is_empty());
    }

    #[test]
    fn the_best_come_first() {
        let mut facts = Facts::of(&start(Outcome::Failed, Some("TST-003")));
        facts.changed = vec!["options".into()];
        let text = [
            signature("TST-009", r#"{ start = "failed" }"#),
            signature("TST-002", r#"{ start = "failed" }"#),
            signature("TST-005", r#"{ start = "failed" }, { changed = "options" }"#),
            signature("TST-003", r#"{ start = "failed" }"#),
            signature("TST-001", r#"{ start = "failed" }"#),
        ]
        .concat();
        let signatures = set(&text);
        let found = matching(&signatures, &facts);
        // The start's own error ID, then the most conditions, then by ID.
        assert_eq!(ids(&found), ["TST-003", "TST-005", "TST-001", "TST-002", "TST-009"]);
        assert_eq!(found.iter().filter(|m| m.own).count(), 1);
        // Two of one ID stay in the file's order.
        let twins = set(&[
            signature("TST-001", r#"{ start = "failed" }"#),
            signature("TST-001", r#"{ start = "failed", error = "TST-003" }"#),
        ]
        .concat());
        let found = matching(&twins, &facts);
        assert_eq!(found[0].signature.cause, "c TST-001");
        assert_eq!(found[1].signature.conditions.len(), 1);
    }

    /// A log of `lines` lines of `bytes` bytes of a, a service's, and at
    /// its end one that says `needle`.
    fn log_with_needle(lines: usize, bytes: usize) -> Facts {
        let mut facts = Facts::of(&start(Outcome::Failed, None));
        facts.log = (0..lines).map(|_| line(6, "service", "a", &"a".repeat(bytes))).collect();
        facts.log.push(line(6, "service", "a", "needle"));
        facts
    }

    #[test]
    fn matching_stops_when_its_work_is_spent_and_says_so_and_matches_nothing_wrong() {
        // 1000 lines of 50 bytes, then the needle: one look through the log
        // is 1001 lines and 50006 bytes, 51007 steps.
        let facts = log_with_needle(1000, 50);
        let wanted = set(&signature("TST-001", r#"{ log = "needle" }"#));
        let found = search_within(&wanted, &facts, 51_007);
        assert_eq!(
            (ids(&found.matches), found.cut_short, found.steps),
            (vec!["TST-001".into()], false, 51_007)
        );
        // One step less is not enough to read the last line: no match, and it says so.
        let found = search_within(&wanted, &facts, 51_006);
        assert_eq!((ids(&found.matches), found.cut_short), (Vec::<String>::new(), true));
        assert!(found.steps <= 51_006);
        // The needle's own line is what is missed, not a wrong one taken: a
        // log that holds no needle, looked through, matches nothing.
        let mut hay = log_with_needle(1000, 50);
        hay.log.pop();
        let found = search_within(&wanted, &hay, 1_000_000);
        assert_eq!((found.matches.len(), found.cut_short), (0, false));
        // Signatures that need no log are matched whatever is left, and
        // so is one that looked before the work ran out. Each look at the
        // log's first line, which `a*` matches, takes 51 steps.
        let text = [
            signature("TST-001", r#"{ log = "a*" }"#),
            signature("TST-002", r#"{ log = "needle" }"#),
            signature("TST-003", r#"{ start = "failed" }"#),
        ]
        .concat();
        let signatures = set(&text);
        let found = search_within(&signatures, &facts, 40);
        assert_eq!(ids(&found.matches), ["TST-003"]);
        assert!(found.cut_short && found.steps <= 40);
        // With work for the first look and not the second, the first is
        // matched and the second is not.
        let found = search_within(&signatures, &facts, 60);
        assert_eq!(ids(&found.matches), ["TST-001", "TST-003"]);
        assert!(found.cut_short);
        // With enough, all three, and not cut short.
        let found = search_within(&signatures, &facts, 200_000);
        assert_eq!(ids(&found.matches), ["TST-001", "TST-002", "TST-003"]);
        assert!(!found.cut_short);
        // Without a log condition there is nothing to cut short.
        let cheap = set(&signature("TST-004", r#"{ start = "failed" }"#));
        let found = search_within(&cheap, &facts, 0);
        assert_eq!((found.matches.len(), found.cut_short, found.steps), (1, false, 0));
    }

    #[test]
    fn the_most_a_file_can_ask_of_the_longest_log_is_cut_short_at_the_work_it_has() {
        // 64 conditions, the most a file may have on the log, each the
        // hardest a glob can be, against 20000 lines of the longest.
        let mut facts = log_with_needle(20_000, 400);
        facts.log.pop();
        let hard = format!("{{ log = \"{}b\" }}", "*a".repeat(99) + "*");
        let text: String = (0..signatures::MAX_LOG_CONDITIONS)
            .map(|n| signature(&format!("TST-{n:03}"), &hard))
            .collect();
        let signatures = set(&text);
        // What it would take is far more than the limit, which is the point of it.
        let everything = signatures::MAX_LOG_CONDITIONS as u64 * 20_000 * 401;
        assert!(WORK < everything / 4, "{WORK} {everything}");
        // The whole limit is too slow to spend in a test built without
        // optimizing: a tenth of it shows the same.
        let found = search_within(&signatures, &facts, WORK / 10);
        assert!(found.matches.is_empty());
        assert!(found.cut_short);
        assert!(found.steps <= WORK / 10, "{}", found.steps);
        // A log of ordinary lines is looked through by the shipped sort of
        // signature well within the limit.
        let ordinary = log_with_needle(20_000, 100);
        let usual = set(&signature("TST-001", r#"{ log = "*needle*" }"#));
        let found = search(&usual, &ordinary);
        assert_eq!((found.matches.len(), found.cut_short), (1, false));
        assert!(found.steps < WORK / 10, "{}", found.steps);
    }

    #[test]
    fn a_log_of_the_longest_lines_is_searched_in_time_that_grows_with_its_size() {
        // The worst a glob does: every line is what it could still match.
        let mut facts = Facts::of(&start(Outcome::Failed, None));
        facts.log = (0..20_000).map(|_| line(6, "service", "a", &"a".repeat(400))).collect();
        let hard = format!("{{ log = \"{}b\" }}", "*a".repeat(99) + "*");
        let text = signature("TST-001", &hard);
        assert!(matching(&set(&text), &facts).is_empty());
    }
}
