// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The trials of the network's settings and the firewall's rules, as the
//! console sees them (ADR 0518).
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
//! undone, comes from Spark's read socket, which answers anyone.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::{Duration, Instant};

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

/// What admind knows of one trial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Began {
    /// A time at or after the moment its change took effect.
    at: Instant,
    /// When Spark undoes it unless it is kept, as last read: none until
    /// admind reads Spark after its own action began it.
    ends: Option<Instant>,
}

/// The trials being tried, by their IDs (`network`, `firewall`), and the
/// undos the journal has.
#[derive(Debug, Default)]
pub struct Trials {
    began: BTreeMap<String, Began>,
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
