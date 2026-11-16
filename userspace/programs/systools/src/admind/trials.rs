// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The trials of settings, as the console sees them (ADR 0518): the
//! network's settings and the firewall's rules, and from C5 SSH's, the
//! console's reach and a service's file.
//!
//! A change tried from the Networking or Firewall page is undone by Spark
//! unless it is kept (ADR 0472). Before the page lets the person keep it,
//! it asks admind whether its browser still reaches the console over the
//! new settings, by a request that came on a connection opened after the
//! change took effect: a firewall rule is checked when a connection
//! begins, and one that is open goes on, so an older connection proves
//! nothing.
//!
//! admind keeps, for each trial, a time at or after the moment its change
//! took effect: when the console's action that began it answered, or, for
//! a trial begun elsewhere or before admind started, when admind first saw
//! Spark list it. A connection accepted after that time was opened after
//! the change. The time only ever errs late, which costs the browser one
//! more connection and never passes an old one.
//!
//! Spark's `show` document, which lists the trials and the last ones
//! undone, comes from Spark's read socket, which answers anyone. It also
//! says whether a restart or a shut down is set ([`Power`]), which the
//! open tabs are told as `event: power` when it changes (ADR 0565).
//!
//! The open tabs are told which trials run, with the time each has left,
//! as `event: trial` (`routes::events`): when a trial begins or ends by a
//! console action, at each round of admind's upkeep, which reads Spark and
//! so sees a trial begun, ended or undone elsewhere, and while any runs,
//! at every round, which corrects the countdowns the tabs keep. A trial
//! is told with the page that has its banner and the words for the
//! header's chip.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::{Duration, Instant};

use serde::Serialize;
use spark_api::show::{self, Show};
use spark_api::trial::Why;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::util;

/// Spark's read socket (spark(8)).
pub const READ: &str = crate::power::READ;

/// How long Spark has to answer.
const SPARK_TIME: Duration = Duration::from_secs(2);

/// The most of Spark's document admind reads: far more than a machine's
/// services make.
const DOCUMENT_MAX: u64 = 4 << 20;

/// How far apart two readings of when one trial ends may be and still be
/// of the same trial: Spark's `left_ms` and admind's clock agree to well
/// within it.
const SAME_END: Duration = Duration::from_secs(2);

/// The page that draws the banner of each trial the console knows the
/// way to, and what it says is being tried, in words that start a
/// sentence. A service's trial is `service.NAME` ([`owner`]).
const KNOWN: [(&str, &str, &str); 4] = [
    ("network", "/networking", "The network settings"),
    ("firewall", "/firewall", "The firewall's rules"),
    ("ssh", "/security#ssh", "SSH's settings"),
    ("console", "/security#reach", "The console's address or certificate"),
];

/// The page that owns the trial `id` and the words for what it tries.
/// A trial the console does not know the way to is a change on the
/// services' page.
pub fn owner(id: &str) -> (String, String) {
    if let Some((_, page, what)) = KNOWN.iter().find(|(known, ..)| *known == id) {
        return (page.to_string(), what.to_string());
    }
    match id.strip_prefix("service.").filter(|name| !name.is_empty()) {
        Some(name) => (format!("/services/{name}"), format!("{name}'s service file")),
        None => ("/services".to_string(), "A change".to_string()),
    }
}

/// A trial that runs, as `event: trial` tells a tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Running {
    /// Its ID: `network`, `firewall`, `ssh`, `console` or `service.NAME`.
    pub id: String,
    /// How long until Spark undoes it unless it is kept, in milliseconds.
    pub left_ms: u64,
    /// The page that has its banner.
    pub page: String,
    /// What is being tried, in words that start a sentence.
    pub what: String,
}

/// What admind knows of one trial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Began {
    /// A time at or after the moment its change took effect.
    at: Instant,
    /// When Spark undoes it unless it is kept, as last read: none until
    /// admind reads Spark after its own action began it.
    ends: Option<Instant>,
}

/// The restart or shut down that is set, as admind last read it from
/// Spark, and how often it has changed: the tabs are told it as `event:
/// power` each time it does (ADR 0565).
#[derive(Debug, Default)]
pub struct Power {
    /// The one that is set; None for none, and before Spark first answers.
    set: Option<crate::power::Pending>,
    /// How many times it has changed since admind started.
    times: u64,
}

impl Power {
    /// Takes `set`, read from Spark now, and says whether it differs from
    /// what was read before: another end, or a time that moved by more
    /// than [`SAME_END`]. The time is to the second, so one read again
    /// moves a little.
    pub fn read(&mut self, set: Option<crate::power::Pending>) -> bool {
        let same = match (&self.set, &set) {
            (None, None) => true,
            (Some(was), Some(now)) => {
                let moved = was.at_ms.abs_diff(now.at_ms);
                was.end == now.end && u128::from(moved) <= SAME_END.as_millis()
            }
            _ => false,
        };
        self.set = set;
        if !same {
            self.times += 1;
        }
        !same
    }

    /// How many times what is set has changed, and what is set now.
    pub fn told(&self) -> (u64, Option<crate::power::Pending>) {
        (self.times, self.set.clone())
    }
}

/// The trials being tried, by their IDs (`network`, `firewall`), and the
/// undos the journal has.
#[derive(Debug, Default)]
pub struct Trials {
    began: BTreeMap<String, Began>,
    /// The trials the open tabs were last told run, by their IDs, so that
    /// the round tells them once more when the last has ended.
    told: BTreeSet<String>,
    /// How many times the tabs have been told, which each tab's stream
    /// compares with the number it has sent.
    times_told: u64,
    /// The undos of Spark's list that the journal has, by their IDs and
    /// times on Spark's clock.
    noted: BTreeSet<(String, u64)>,
    /// What admind last said of Spark not answering, so that the log says
    /// it once and not every 30 seconds.
    complained: Option<String>,
}

/// A trial that Spark undid by itself, for the journal's row (ADR 0518).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undo {
    /// Its ID and when it was undone on Spark's clock: which undo it is.
    key: (String, u64),
    /// The Unix time it was undone.
    pub at: i64,
    /// Why, in Spark's words: "it was not kept in time".
    pub why: String,
    /// The files Spark put back.
    pub files: Vec<String>,
}

impl Trials {
    /// The console's action that begins `id` answered at `now`.
    pub fn begun(&mut self, id: &str, now: Instant) {
        self.began.insert(id.to_string(), Began { at: now, ends: None });
    }

    /// The console's action that keeps or undoes `id` answered.
    pub fn ended(&mut self, id: &str) {
        self.began.remove(id);
    }

    /// admind read its settings again at `now`, by Spark's reload. The
    /// trial `console` tries admin.toml, and its change takes effect only
    /// when admind has read it: a connection accepted before then was
    /// answered by the old settings, so the trial begins again now for
    /// [`Trials::fresh`]. Any other reload while it runs moves the time
    /// later too, which only errs late.
    pub fn reloaded(&mut self, now: Instant) {
        if let Some(began) = self.began.get_mut("console") {
            began.at = began.at.max(now);
        }
    }

    /// Takes in the trials Spark lists at `now`, each with how long until
    /// it is undone, and forgets the ones it no longer lists.
    ///
    /// A trial admind has no time for began, as far as it can tell, now.
    /// So did one whose end moved: it was kept or undone, and begun again,
    /// since admind last looked.
    pub fn listed<'a>(&mut self, trials: impl IntoIterator<Item = (&'a str, u64)>, now: Instant) {
        let mut listed = BTreeMap::new();
        for (id, left_ms) in trials {
            let ends = now + Duration::from_millis(left_ms);
            let began = match self.began.get(id) {
                Some(&Began { at, ends: None }) => Began { at, ends: Some(ends) },
                Some(&Began { at, ends: Some(was) }) if apart(was, ends) <= SAME_END => {
                    Began { at, ends: Some(was) }
                }
                _ => Began { at: now, ends: Some(ends) },
            };
            listed.insert(id.to_string(), began);
        }
        self.began = listed;
    }

    /// Takes in the trials of Spark's `document`, read at `now`.
    pub fn read(&mut self, document: &Show, now: Instant) {
        self.listed(document.trials.iter().map(|trial| (trial.id.as_str(), trial.left_ms)), now);
    }

    /// Whether a connection accepted at `accepted` was opened after the
    /// change of the trial `id` took effect; none when `id` is not being
    /// tried.
    pub fn fresh(&self, id: &str, accepted: Instant) -> Option<bool> {
        self.began.get(id).map(|began| accepted > began.at)
    }

    /// The trials that run at `now`, with the time each has left, by their
    /// IDs. One whose end admind does not know yet (the console's action
    /// that began it answered, and Spark has not been read since) is not
    /// among them, nor is one whose end has passed.
    pub fn running(&self, now: Instant) -> Vec<Running> {
        self.began
            .iter()
            .filter_map(|(id, began)| {
                let left =
                    began.ends?.checked_duration_since(now).filter(|left| !left.is_zero())?;
                let (page, what) = owner(id);
                let left_ms = u64::try_from(left.as_millis()).unwrap_or(u64::MAX);
                Some(Running { id: id.clone(), left_ms, page, what })
            })
            .collect()
    }

    /// The open tabs are told, at `now`, which trials run.
    pub fn tell(&mut self, now: Instant) {
        self.told = self.running(now).into_iter().map(|trial| trial.id).collect();
        self.times_told += 1;
    }

    /// How many times the tabs have been told.
    pub fn times_told(&self) -> u64 {
        self.times_told
    }

    /// Whether a round that has read Spark tells the tabs at `now`: while
    /// a trial runs, every time, and once more when the last has ended, with
    /// none.
    pub fn round_tells(&self, now: Instant) -> bool {
        !self.told.is_empty() || !self.running(now).is_empty()
    }

    /// The undos of Spark's `document`, read at the Unix time `now`, that
    /// the journal has not been given. One a person asked for is left out:
    /// its row is the console action's, or the round's hand edit by the
    /// person the audit names.
    pub fn undos(&mut self, document: &Show, now: i64) -> Vec<Undo> {
        let listed: BTreeSet<(String, u64)> =
            document.undone.iter().map(|undone| (undone.id.clone(), undone.at_ms)).collect();
        self.noted.retain(|key| listed.contains(key));
        let asked = Why::Asked.words();
        document
            .undone
            .iter()
            .filter(|undone| undone.why != asked)
            .map(|undone| {
                let ago = document.now_ms.saturating_sub(undone.at_ms) / 1000;
                Undo {
                    key: (undone.id.clone(), undone.at_ms),
                    at: now.saturating_sub(i64::try_from(ago).unwrap_or(i64::MAX)),
                    why: undone.why.clone(),
                    files: undone.files.clone(),
                }
            })
            .filter(|undo| !self.noted.contains(&undo.key))
            .collect()
    }

    /// The journal has the row of `undo`.
    pub fn noted(&mut self, undo: &Undo) {
        self.noted.insert(undo.key.clone());
    }

    /// Whether the log should say `why`, Spark's failure to answer: when
    /// it differs from what it said last. None is Spark answering again.
    pub fn complain(&mut self, why: Option<&str>) -> bool {
        if self.complained.as_deref() == why {
            return false;
        }
        self.complained = why.map(str::to_string);
        true
    }
}

/// How far apart `a` and `b` are.
fn apart(a: Instant, b: Instant) -> Duration {
    a.saturating_duration_since(b).max(b.saturating_duration_since(a))
}

/// Spark's `show` document, from its read socket at `socket`
/// ([`READ`]).
///
/// # Errors
///
/// In words, if Spark does not answer in time, refuses, or answers what
/// does not read.
pub async fn document(socket: &str) -> Result<Show, String> {
    let asked = async {
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_all(b"show\n").await?;
        stream.shutdown().await?;
        let mut answer = String::new();
        stream.take(DOCUMENT_MAX).read_to_string(&mut answer).await?;
        Ok::<_, io::Error>(answer)
    };
    let answer = tokio::time::timeout(SPARK_TIME, asked)
        .await
        .map_err(|_| String::from("Spark did not answer in time"))?
        .map_err(|err| format!("cannot ask Spark: {}", util::strip_os_error(&err)))?;
    if let Some(why) = answer.strip_prefix("error: ") {
        return Err(format!("Spark refused: {}", why.trim_end()));
    }
    show::parse(&answer).map_err(|why| format!("Spark's answer does not read: {why}"))
}

/// A Spark that answers each of `answers` to one connection on a socket
/// at `path`, and gives back what each asked.
#[cfg(test)]
pub fn fake_spark(path: &str, answers: Vec<String>) -> std::thread::JoinHandle<Vec<String>> {
    use std::io::{Read, Write};
    // A socket left by an earlier run of the test would be in the way.
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        let mut asked = Vec::new();
        for answer in answers {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            asked.push(request);
            stream.write_all(answer.as_bytes()).unwrap();
        }
        asked
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::Pending;

    #[test]
    fn what_is_set_changes_by_its_end_and_a_time_that_moved_and_not_by_a_read_again() {
        let restart = |at_ms| Some(Pending { end: "reboot", in_seconds: 60, at_ms });
        let mut power = Power::default();
        assert!(!power.read(None), "none is what admind starts with");
        assert!(power.read(restart(100_000)));
        assert!(!power.read(restart(101_000)), "read again a second later");
        assert!(power.read(restart(160_000)), "asked again a minute later");
        let off = Some(Pending { end: "poweroff", in_seconds: 60, at_ms: 160_000 });
        assert!(power.read(off));
        assert!(power.read(None), "cancelled");
        assert_eq!(power.told(), (4, None));
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn a_connection_is_fresh_only_when_accepted_after_the_trial_began() {
        let start = Instant::now();
        let mut trials = Trials::default();
        assert_eq!(trials.fresh("network", start), None, "nothing is tried");
        trials.begun("network", start + SECOND);
        assert_eq!(trials.fresh("network", start), Some(false));
        assert_eq!(trials.fresh("network", start + SECOND), Some(false), "at the same moment");
        assert_eq!(trials.fresh("network", start + 2 * SECOND), Some(true));
        assert_eq!(trials.fresh("firewall", start + 2 * SECOND), None);
        trials.ended("network");
        assert_eq!(trials.fresh("network", start + 2 * SECOND), None);
    }

    #[test]
    fn a_trial_the_console_began_keeps_its_time_when_spark_lists_it() {
        let start = Instant::now();
        let mut trials = Trials::default();
        trials.begun("firewall", start);
        trials.listed([("firewall", 60_000)], start + SECOND);
        assert_eq!(trials.fresh("firewall", start + SECOND / 2), Some(true));
        // Spark's count goes down as admind's clock goes on: the same end.
        trials.listed([("firewall", 50_100)], start + 11 * SECOND);
        assert_eq!(trials.fresh("firewall", start + SECOND / 2), Some(true));
    }

    #[test]
    fn the_console_s_trial_begins_again_when_admind_reads_its_settings() {
        let start = Instant::now();
        let mut trials = Trials::default();
        trials.begun("console", start);
        trials.begun("firewall", start);
        trials.listed([("console", 60_000), ("firewall", 60_000)], start);
        // Spark's reload reached admind after the action answered.
        trials.reloaded(start + SECOND * 2);
        assert_eq!(trials.fresh("console", start + SECOND), Some(false));
        assert_eq!(trials.fresh("console", start + SECOND * 3), Some(true));
        // Spark lists it again, and the later time stays.
        trials.listed([("console", 59_000), ("firewall", 59_000)], start + SECOND * 3);
        assert_eq!(trials.fresh("console", start + SECOND), Some(false));
        // Another trial is not moved, and an earlier reload moves nothing.
        assert_eq!(trials.fresh("firewall", start + SECOND), Some(true));
        trials.reloaded(start);
        assert_eq!(trials.fresh("console", start + SECOND * 3), Some(true));
        // With no `console` trial, a reload begins none.
        let mut none = Trials::default();
        none.reloaded(start);
        assert_eq!(none.fresh("console", start + SECOND), None);
    }

    #[test]
    fn a_trial_begun_elsewhere_began_when_admind_first_saw_it() {
        let start = Instant::now();
        let mut trials = Trials::default();
        // Begun at a terminal, or before admind started.
        trials.listed([("network", 300_000)], start + 5 * SECOND);
        assert_eq!(trials.fresh("network", start + 4 * SECOND), Some(false));
        assert_eq!(trials.fresh("network", start + 6 * SECOND), Some(true));
    }

    #[test]
    fn a_trial_whose_end_moved_was_begun_again_and_one_no_longer_listed_is_forgotten() {
        let start = Instant::now();
        let mut trials = Trials::default();
        trials.listed([("network", 60_000), ("firewall", 60_000)], start);
        // Kept at a terminal and tried again for 5 minutes, between two looks.
        trials.listed([("network", 300_000)], start + 10 * SECOND);
        assert_eq!(trials.fresh("network", start + 5 * SECOND), Some(false));
        assert_eq!(trials.fresh("network", start + 11 * SECOND), Some(true));
        assert_eq!(trials.fresh("firewall", start + 11 * SECOND), None, "kept or undone");
    }

    /// What a running trial is told as, by its words alone.
    fn told_as(id: &str) -> (String, String) {
        let mut trials = Trials::default();
        let start = Instant::now();
        trials.listed([(id, 60_000)], start);
        let [only] = <[Running; 1]>::try_from(trials.running(start)).unwrap();
        assert_eq!((only.id.as_str(), only.left_ms), (id, 60_000));
        (only.page, only.what)
    }

    #[test]
    fn each_trial_is_told_with_the_page_that_has_its_banner_and_words_that_start_a_sentence() {
        let told = |id| told_as(id);
        assert_eq!(told("network"), ("/networking".into(), "The network settings".into()));
        assert_eq!(told("firewall"), ("/firewall".into(), "The firewall's rules".into()));
        assert_eq!(told("ssh"), ("/security#ssh".into(), "SSH's settings".into()));
        assert_eq!(
            told("console"),
            ("/security#reach".into(), "The console's address or certificate".into())
        );
        assert_eq!(
            told("service.nginx"),
            ("/services/nginx".into(), "nginx's service file".into())
        );
        assert_eq!(
            told("service.php-fpm.8"),
            ("/services/php-fpm.8".into(), "php-fpm.8's service file".into())
        );
        // A trial the table does not know, and a service's with no name,
        // are a change on the services' page.
        assert_eq!(told("backup"), ("/services".into(), "A change".into()));
        assert_eq!(told("service."), ("/services".into(), "A change".into()));
    }

    #[test]
    fn the_time_left_is_as_of_the_moment_it_is_asked_and_a_trial_with_none_left_does_not_run() {
        let start = Instant::now();
        let mut trials = Trials::default();
        trials.listed([("network", 60_000), ("firewall", 5_000)], start);
        let left = |at: Instant| -> Vec<(String, u64)> {
            trials.running(at).into_iter().map(|trial| (trial.id, trial.left_ms)).collect()
        };
        assert_eq!(left(start), [("firewall".into(), 5_000), ("network".into(), 60_000)]);
        assert_eq!(
            left(start + 4 * SECOND),
            [("firewall".into(), 1_000), ("network".into(), 56_000)]
        );
        assert_eq!(left(start + 5 * SECOND), [("network".into(), 55_000)], "its end has come");
        assert_eq!(left(start + 61 * SECOND), []);
    }

    #[test]
    fn a_trial_the_console_began_does_not_run_until_spark_has_said_how_long_it_has() {
        let start = Instant::now();
        let mut trials = Trials::default();
        trials.begun("network", start);
        assert_eq!(trials.running(start + SECOND), []);
        trials.listed([("network", 59_000)], start + SECOND);
        let running = trials.running(start + SECOND);
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].left_ms, 59_000);
        trials.ended("network");
        assert_eq!(trials.running(start + SECOND), []);
    }

    #[test]
    fn the_tabs_are_counted_as_told_and_the_round_tells_while_a_trial_runs_and_once_more_after() {
        let start = Instant::now();
        let mut trials = Trials::default();
        assert!(!trials.round_tells(start), "nothing runs and nothing was told");
        assert_eq!(trials.times_told(), 0);
        trials.listed([("network", 40_000)], start);
        assert!(trials.round_tells(start), "begun elsewhere");
        trials.tell(start);
        assert_eq!(trials.times_told(), 1);
        // Told again at each round while it runs, to correct the countdown.
        assert!(trials.round_tells(start + 30 * SECOND));
        trials.tell(start + 30 * SECOND);
        // Spark undid it between two rounds: the tabs are told none, once.
        trials.listed([], start + 60 * SECOND);
        assert!(trials.round_tells(start + 60 * SECOND));
        trials.tell(start + 60 * SECOND);
        assert_eq!(trials.times_told(), 3);
        assert!(!trials.round_tells(start + 90 * SECOND));
        assert!(!trials.round_tells(start + 120 * SECOND));
    }

    #[test]
    fn a_trial_that_ends_by_a_keep_is_told_as_none_though_spark_is_not_asked() {
        let start = Instant::now();
        let mut trials = Trials::default();
        trials.listed([("firewall", 50_000), ("network", 20_000)], start);
        trials.tell(start);
        trials.ended("firewall");
        trials.tell(start + SECOND);
        assert_eq!(trials.running(start + SECOND).len(), 1);
        trials.ended("network");
        // The round that follows finds the tabs told of the last one and
        // tells none; the action's own call has done so already.
        trials.tell(start + 2 * SECOND);
        assert!(trials.running(start + 2 * SECOND).is_empty());
        assert!(!trials.round_tells(start + 3 * SECOND));
    }

    fn undone(id: &str, why: &str, at_ms: u64) -> show::Undone {
        show::Undone {
            id: id.into(),
            why: why.into(),
            at_ms,
            files: vec![format!("/system/settings/{id}")],
        }
    }

    #[test]
    fn the_journal_is_given_each_undo_spark_made_by_itself_once_with_its_time() {
        let mut trials = Trials::default();
        let mut document = Show {
            now_ms: 100_000,
            undone: vec![
                undone("network", &Why::TimeUp.words(), 40_000),
                undone("firewall", &Why::Asked.words(), 50_000),
                undone("service.web", &Why::Failed("web".into()).words(), 99_400),
            ],
            ..Show::default()
        };
        let undos = trials.undos(&document, 1_700_000_000);
        let seen: Vec<(&str, i64)> = undos.iter().map(|u| (u.key.0.as_str(), u.at)).collect();
        assert_eq!(seen, [("network", 1_700_000_000 - 60), ("service.web", 1_700_000_000)]);
        assert_eq!(undos[0].files, ["/system/settings/network"]);
        assert_eq!(undos[0].why, "it was not kept in time");
        trials.noted(&undos[0]);
        let left = trials.undos(&document, 1_700_000_030);
        assert_eq!(left.len(), 1, "the journal has the network's");
        assert_eq!(left[0].key.0, "service.web");
        // Spark keeps the last eight: one that went from its list is
        // forgotten, and the same trial undone again is a new undo.
        document.undone = vec![undone("network", &Why::TimeUp.words(), 160_000)];
        document.now_ms = 170_000;
        assert_eq!(trials.undos(&document, 1_700_000_100).len(), 1);
        assert!(trials.noted.is_empty());
    }

    #[test]
    fn spark_not_answering_is_said_once_until_it_answers_again() {
        let mut trials = Trials::default();
        assert!(!trials.complain(None));
        assert!(trials.complain(Some("cannot ask Spark")));
        assert!(!trials.complain(Some("cannot ask Spark")));
        assert!(trials.complain(Some("Spark did not answer in time")));
        assert!(trials.complain(None));
        assert!(!trials.complain(None));
    }

    #[test]
    fn spark_s_document_comes_from_its_read_socket() {
        let path = std::env::temp_dir().join(format!("admind-spark-{}", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        let shown = Show {
            trials: vec![show::Trial {
                id: String::from("network"),
                left_ms: 60_000,
                ..show::Trial::default()
            }],
            ..Show::default()
        };
        let answers = vec![show::to_json(&shown).unwrap(), "error: busy\n".into(), "{".into()];
        let fake = fake_spark(&path, answers);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let read = runtime.block_on(document(&path));
        assert_eq!(read.unwrap().trials, shown.trials);
        let refused = runtime.block_on(document(&path));
        assert_eq!(refused.unwrap_err(), "Spark refused: busy");
        let garbled = runtime.block_on(document(&path));
        assert!(garbled.unwrap_err().starts_with("Spark's answer does not read"));
        assert_eq!(fake.join().unwrap(), ["show\n"; 3]);
        std::fs::remove_file(&path).unwrap();
        let gone = runtime.block_on(document(&path)).unwrap_err();
        assert!(gone.starts_with("cannot ask Spark"), "{gone}");
    }
}
