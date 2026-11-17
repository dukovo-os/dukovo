// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! changes bisect (ADR 0475): when several changes came between the last
//! good start and a failed one, which one broke the start, found by
//! halving them across restarts.
//!
//! It compares the newest failed start, F, with the last good start
//! before it, G (their records, [`super::starts`]), and finds each change
//! a start can be made without: a **lever**. The slot is one, when F ran
//! one slot and the other still holds what G ran and the boot menu still
//! starts it; the boot options are one, a try starting with G's; and each
//! app installed for everyone that is new or changed in F and has
//! services is one, a try leaving its services out (Spark holds them off
//! as meant). The rest cannot be tried apart: the machine's hardware, an
//! app F no longer has, a slot rebuilt since G, an app that runs nothing
//! at start.
//!
//! Each try is a one-start trial ([`slots::trial`]): the boot menu starts
//! it once in place of the usual start and changes no slot state, so a
//! hang, a panic or a power cut during it costs only that try. The search
//! halves the candidates: each try undoes the first half of them. A try
//! that starts well puts the cause in the half it undid; one that fails
//! as F did (the same error ID, by the same service or check) puts it in
//! the half it kept. Four levers take two tries and eight three. A cause
//! is named **confirmed** when a good try undid it alone, and **by
//! elimination** otherwise, which `--confirm` settles with one more try
//! that undoes only it. A try that fails otherwise than F ends the
//! search: its halves cannot be judged. A try that never reached its
//! health check is tried once more, and counts as failed the second
//! time. One the boot menu did not start ends the search.
//!
//! The search lives in `/system/state/bisect/state.json`, written whole
//! or not at all, with a file for each try, `try-N`, the services Spark
//! leaves out when the kernel's command line says `trial=N`. The health
//! check of every start takes the search's next step while it waits on
//! a try ([`step`]); `changes bisect` begins it, and `--status`, `--stop`
//! and `--confirm` follow it (changes(1)). Nothing it finds is changed
//! by it: it names the change and the fix, and the owner picks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use slots::Slot;
use slots::trial::{self, MAX_NUMBER, Trial};

use super::place::{self, Root};
use super::starts::{self, Change, Item, Outcome, Start};
use crate::bundles;
use crate::util::{Fail, fail};

/// Where the search is kept.
pub const DIR: &str = "/system/state/bisect";

/// The search's file in [`DIR`].
const STATE: &str = "state.json";

/// The plan's file in [`DIR`]: the search of the newest failed start, as
/// the health check planned it when that start failed, before anyone
/// begins it.
const PLANNED: &str = "planned.json";

/// The kinds of a start's items that are the slot's: a try of the other
/// slot changes them all at once.
const SLOT_KINDS: [&str; 4] = ["system", "kernel", "firmware", "microcode"];

/// The kinds of a start's items that are the machine's hardware.
const HARDWARE_KINDS: [&str; 2] = ["card", "root"];

/// What a lever is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The slot: a try starts the slot G ran.
    Slot,
    /// The boot options: a try starts with G's.
    Options,
    /// An app installed for everyone: a try leaves its services out.
    App,
}

/// A change a try can undo for one start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lever {
    pub kind: Kind,
    /// Which: the app's ID; `system` or `boot options` otherwise.
    pub name: String,
    /// What G ran: the system and its firmware set, the options a try
    /// passes, or the app's version; none for an app new in F.
    pub before: Option<String>,
    /// What F ran, likewise.
    pub after: String,
    /// When what F ran was installed, in seconds since 1970, where known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<i64>,
    /// The first start kept that ran it, and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_used: Option<(u64, i64)>,
    /// An app's services, which a try that undoes it leaves out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

/// Why a change cannot be tried apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Why {
    /// A network card or a partition of the root: the machine's own.
    Hardware,
    /// An app F no longer has.
    Gone,
    /// An app with no services, which runs nothing at start.
    NoServices,
    /// The slot G ran was rebuilt since, or the boot menu no longer
    /// starts it.
    SlotGone,
    /// G's boot options, which a try cannot hold: why.
    Options(String),
    /// Options that differ only in the boot menu's own words, as an entry
    /// of Troubleshooting adds them.
    MenuWords,
    /// Anything else the record holds that no try changes.
    Other,
}

impl Why {
    /// Why, in words, after the change's own.
    #[must_use]
    pub fn says(&self) -> String {
        match self {
            Why::Hardware => "the machine's hardware, which no restart changes".into(),
            Why::Gone => "gone since, so a start cannot have it back for one try".into(),
            Why::NoServices => "it runs nothing at start, so it cannot have stopped one".into(),
            Why::SlotGone => {
                "the slot the good start ran no longer holds it, so no try can start it".into()
            }
            Why::Options(why) => why.clone(),
            Why::MenuWords => "only the boot menu's own words differ, as an entry of \
                               Troubleshooting adds them"
                .into(),
            Why::Other => "no try can change it".into(),
        }
    }
}

/// A change that cannot be tried apart, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Apart {
    /// What kind of thing, as [`Item::kind`].
    pub kind: String,
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub why: Why,
}

/// What a try found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// It started well: the cause is among what it undid.
    Good,
    /// It failed as F did: the cause is among what it kept.
    Failed,
    /// It never reached its health check, or never finished it.
    Unfinished,
    /// It failed otherwise than F did.
    Otherwise,
    /// The boot menu did not start it.
    NotStarted,
}

/// A try: one start with some levers undone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Try {
    /// Its number, from 1, as the boot menu passes it in `trial=`.
    pub number: u8,
    /// The levers it undoes, by their place in [`Search::levers`].
    pub undone: Vec<usize>,
    /// Whether it undoes one lever named by elimination, to confirm it.
    #[serde(default)]
    pub confirm: bool,
    /// Whether it is the try before it again, which never reached its
    /// health check.
    #[serde(default)]
    pub again: bool,
    /// The newest start kept when it was written: only a later start can
    /// be the try.
    pub after: u64,
    /// What it found, once judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// The start that was the try, once judged by one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
}

/// How a search ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum End {
    /// The cause: the lever at this place, and whether a good try undid
    /// it alone, or it is named by elimination.
    Found { lever: usize, confirmed: bool },
    /// The try that undid only the lever named by elimination failed as
    /// F did: no one change that can be tried apart explains the failure.
    NotFound,
    /// A try failed otherwise than F did, at that start: the halves
    /// cannot be judged.
    Otherwise { number: u8, start: u64 },
    /// The boot menu did not start the try.
    NotStarted { number: u8 },
    /// The try could not be written or followed: why.
    Broken { number: u8, why: String },
    /// The owner stopped the search.
    Stopped,
}

/// A search: what it compares, what it can undo, and how far it got.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Search {
    /// When it began, in seconds since 1970.
    pub begun: i64,
    /// F, the failed start, and how it failed: its error ID, the service
    /// or check that failed it, and why.
    pub failed: u64,
    pub error: Option<String>,
    pub failing: Option<String>,
    pub why: Option<String>,
    /// G, the last good start before F.
    pub good: u64,
    /// The slots F and G ran, by letter.
    pub failed_slot: String,
    pub good_slot: String,
    /// The boot options F and G ran, as a try passes them
    /// ([`trial::options_of`]).
    pub failed_options: String,
    pub good_options: String,
    pub levers: Vec<Lever>,
    pub apart: Vec<Apart>,
    /// The levers the cause may still be, by place, in order.
    pub candidates: Vec<usize>,
    pub tries: Vec<Try>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<End>,
}

/// Why a search cannot begin or go on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No start kept failed, after the last good one.
    NoFailure,
    /// Start N, after the newest failure, was good: nothing to find.
    GoodSince(u64),
    /// No good start before F is kept.
    NoGood(u64),
    /// Nothing between G and F can be tried apart.
    NoLever,
    /// F's own boot options cannot be passed to a try: why.
    Options(String),
    /// A search waits on try N.
    Running(u8),
    /// The search did not name a change by elimination, which is all
    /// `--confirm` confirms.
    NothingToConfirm,
    /// No search has run.
    NoSearch,
}

impl core::fmt::Display for Refused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refused::NoFailure => f.write_str("no start kept has failed since the last good one"),
            Refused::GoodSince(number) => write!(
                f,
                "start {number}, since the newest failure, was good: there is nothing to find"
            ),
            Refused::NoGood(number) => {
                write!(f, "no good start before start {number}, which failed, is kept")
            }
            Refused::NoLever => f.write_str(
                "nothing that changed since the last good start can be tried apart: \
                 `changes --failed` lists what did",
            ),
            Refused::Options(why) => write!(f, "the failed start cannot be tried again: {why}"),
            Refused::Running(number) => write!(
                f,
                "a search is under way, waiting on try {number}: `changes bisect --status` \
                 follows it, and `elevate changes bisect --stop` stops it"
            ),
            Refused::NothingToConfirm => f.write_str(
                "the last search named no change by elimination, which is what `--confirm` \
                 confirms",
            ),
            Refused::NoSearch => {
                f.write_str("no search has run: `elevate changes bisect` begins one")
            }
        }
    }
}

/// What the machine holds now that decides what can be tried.
pub struct Now<'a> {
    /// What the slot G ran holds now, as a start's record holds it
    /// ([`super::health::slot_items`]), when it is the other slot than
    /// F's and the boot menu still starts it.
    pub good_slot: Option<Vec<Item>>,
    /// The services of the app installed for everyone with this ID.
    pub services: &'a dyn Fn(&str) -> Vec<String>,
}

/// The most restarts a search of `levers` candidates takes, when every
/// try reaches its health check: one to try a lone lever, otherwise one
/// for each halving.
#[must_use]
pub fn restarts(levers: usize) -> u32 {
    match levers {
        0 => 0,
        1 => 1,
        n => usize::BITS - (n - 1).leading_zeros(),
    }
}

/// The value of the item of `kind` a start ran.
fn value<'a>(start: &'a Start, kind: &str) -> Option<&'a str> {
    start.items.iter().find(|item| item.kind == kind).map(|item| item.value.as_str())
}

/// The slot's items of `items`, as (kind, name, value), in order: what
/// two starts must share to have run the same slot.
fn slot_holds(items: &[Item]) -> Vec<(String, String, String)> {
    let mut holds: Vec<(String, String, String)> = items
        .iter()
        .filter(|item| SLOT_KINDS.contains(&item.kind.as_str()))
        .map(|item| (item.kind.clone(), item.name.clone(), item.value.clone()))
        .collect();
    holds.sort();
    holds
}

/// The system a start ran, in words: `Dukovo 0.16.1 with firmware set
/// 2026.11`, and its kernel too when `kernel` is set.
fn system_of(start: &Start, kernel: bool) -> String {
    let mut words = value(start, "system")
        .map_or_else(|| "a system with no version".to_string(), |v| format!("Dukovo {v}"));
    if kernel && let Some(hash) = value(start, "kernel") {
        words.push_str(&format!(", kernel {hash}"));
    }
    if let Some(set) = value(start, "firmware") {
        words.push_str(&format!(" with firmware set {set}"));
    }
    words
}

/// The options a try passes for a start: those of its record, less the
/// boot menu's own words.
fn options_of(start: &Start) -> String {
    trial::options_of(value(start, "options").unwrap_or_default())
}

/// What changed from `good` to `failed`, of `records`, as levers, in the
/// order a search halves them (the slot, the options, then the apps by
/// ID), and what cannot be tried apart, given what the machine holds
/// `now`.
#[must_use]
pub fn levers(
    records: &[Start],
    good: &Start,
    failed: &Start,
    now: &Now<'_>,
) -> (Vec<Lever>, Vec<Apart>) {
    let found = starts::changes(records, good, failed);
    let mut levers = Vec::new();
    let mut apart = Vec::new();
    let apart_of = |change: &Change, why: Why| Apart {
        kind: change.kind.clone(),
        name: change.name.clone(),
        before: change.before.clone(),
        after: change.after.clone(),
        why,
    };
    // The slot, whatever of it changed, as one.
    let in_slot: Vec<&Change> =
        found.iter().filter(|change| SLOT_KINDS.contains(&change.kind.as_str())).collect();
    if let Some(first) = in_slot.iter().find(|c| c.kind == "system").or(in_slot.first()) {
        let kernel_only = system_of(good, false) == system_of(failed, false);
        let (before, after) = (system_of(good, kernel_only), system_of(failed, kernel_only));
        let kept = good.slot != failed.slot
            && now.good_slot.as_deref().map(slot_holds) == Some(slot_holds(&good.items));
        if kept {
            levers.push(Lever {
                kind: Kind::Slot,
                name: "system".into(),
                before: Some(before),
                after,
                installed: first.installed,
                first_used: first.first_used,
                services: Vec::new(),
            });
        } else {
            apart.push(Apart {
                kind: "system".into(),
                name: "Dukovo".into(),
                before: Some(before),
                after: Some(after),
                why: Why::SlotGone,
            });
        }
    }
    if let Some(change) = found.iter().find(|change| change.kind == "options") {
        let (before, after) = (options_of(good), options_of(failed));
        if before == after {
            apart.push(apart_of(change, Why::MenuWords));
        } else if let Err(why) = Trial::new(1, Slot::A, &before) {
            apart.push(apart_of(change, Why::Options(why.to_string())));
        } else {
            levers.push(Lever {
                kind: Kind::Options,
                name: "boot options".into(),
                before: Some(before),
                after,
                installed: None,
                first_used: change.first_used,
                services: Vec::new(),
            });
        }
    }
    let mut apps: Vec<&Change> = found.iter().filter(|change| change.kind == "app").collect();
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    for change in apps {
        let Some(after) = &change.after else {
            apart.push(apart_of(change, Why::Gone));
            continue;
        };
        let mut services = (now.services)(&change.name);
        services.sort();
        services.dedup();
        if services.is_empty() {
            apart.push(apart_of(change, Why::NoServices));
            continue;
        }
        levers.push(Lever {
            kind: Kind::App,
            name: change.name.clone(),
            before: change.before.clone(),
            after: after.clone(),
            installed: change.installed,
            first_used: change.first_used,
            services,
        });
    }
    for change in &found {
        let kind = change.kind.as_str();
        if SLOT_KINDS.contains(&kind) || kind == "options" || kind == "app" {
            continue;
        }
        let why = if HARDWARE_KINDS.contains(&kind) { Why::Hardware } else { Why::Other };
        apart.push(apart_of(change, why));
    }
    (levers, apart)
}

/// The failed start a search would look into, of `records`, oldest
/// first, and the last good start before it: the newest failed start
/// that was no try, with no good start since but tries.
///
/// # Errors
///
/// If no start failed, one was good since, or no good start before it is
/// kept.
pub fn failed_and_good(records: &[Start]) -> Result<(&Start, &Start), Refused> {
    let usual = |start: &&Start| start.trial.is_none();
    let failed = records
        .iter()
        .rev()
        .filter(usual)
        .find(|start| start.outcome == Outcome::Failed)
        .ok_or(Refused::NoFailure)?;
    if let Some(since) = records
        .iter()
        .filter(usual)
        .find(|start| start.number > failed.number && start.outcome == Outcome::Good)
    {
        return Err(Refused::GoodSince(since.number));
    }
    let good = starts::last_good(records, failed.number).ok_or(Refused::NoGood(failed.number))?;
    Ok((failed, good))
}

impl Search {
    /// A search of `levers` from `good` to `failed`, as of `now` in
    /// seconds since 1970, with its first try written after start
    /// `newest`: the first half undone, or a lone lever on its own.
    ///
    /// # Errors
    ///
    /// If there is no lever, or F's options cannot be passed to a try.
    pub fn new(
        good: &Start,
        failed: &Start,
        levers: Vec<Lever>,
        apart: Vec<Apart>,
        now: i64,
        newest: u64,
    ) -> Result<Search, Refused> {
        if levers.is_empty() {
            return Err(Refused::NoLever);
        }
        let failed_options = options_of(failed);
        Trial::new(1, Slot::A, &failed_options).map_err(|why| Refused::Options(why.to_string()))?;
        let mut search = Search {
            begun: now,
            failed: failed.number,
            error: failed.error.clone(),
            failing: failed.failing.clone(),
            why: failed.why.clone(),
            good: good.number,
            failed_slot: failed.slot.clone(),
            good_slot: good.slot.clone(),
            failed_options,
            good_options: options_of(good),
            candidates: (0..levers.len()).collect(),
            levers,
            apart,
            tries: Vec::new(),
            end: None,
        };
        if search.levers.len() == 1 {
            search.push(vec![0], true, false, newest);
        } else {
            search.halve(newest);
        }
        Ok(search)
    }

    /// The try the search waits on, while it runs.
    #[must_use]
    pub fn waiting(&self) -> Option<&Try> {
        if self.end.is_some() {
            return None;
        }
        self.tries.last().filter(|t| t.verdict.is_none())
    }

    /// The most restarts left, when every try reaches its health check.
    #[must_use]
    pub fn restarts_left(&self) -> u32 {
        match self.waiting() {
            Some(waiting) if waiting.confirm => 1,
            Some(_) => restarts(self.candidates.len()),
            None => 0,
        }
    }

    /// Adds a try undoing `undone`, written after start `newest`.
    fn push(&mut self, undone: Vec<usize>, confirm: bool, again: bool, newest: u64) {
        let number = self.tries.last().map_or(1, |last| last.number.saturating_add(1));
        if number > MAX_NUMBER {
            let why = format!("a search has at most {MAX_NUMBER} tries");
            self.end = Some(End::Broken { number, why });
            return;
        }
        self.tries.push(Try {
            number,
            undone,
            confirm,
            again,
            after: newest,
            verdict: None,
            start: None,
        });
    }

    /// The next step once the candidates are as they are: a try undoing
    /// the first half of them, rounded up, or, with one left, the end.
    fn halve(&mut self, newest: u64) {
        match self.candidates.as_slice() {
            [] => self.end = Some(End::NotFound),
            [cause] => {
                let cause = *cause;
                let confirmed = self
                    .tries
                    .iter()
                    .any(|t| t.verdict == Some(Verdict::Good) && t.undone.as_slice() == [cause]);
                self.end = Some(End::Found { lever: cause, confirmed });
            }
            candidates => {
                let undone = candidates[..candidates.len().div_ceil(2)].to_vec();
                self.push(undone, false, false, newest);
            }
        }
    }

    /// Judges the try the search waits on as `verdict`, by start `start`
    /// where one was the try, and takes the next step, writing a new try
    /// after start `newest`. Does nothing when it waits on none.
    pub fn judge(&mut self, verdict: Verdict, start: Option<u64>, newest: u64) {
        if self.waiting().is_none() {
            return;
        }
        let Some(judged) = self.tries.last_mut() else { return };
        judged.verdict = Some(verdict);
        judged.start = start;
        let judged = judged.clone();
        // A try that never got to its health check is tried once more,
        // and counts as failed the second time.
        let verdict = match verdict {
            Verdict::Unfinished if !judged.again => {
                self.push(judged.undone, judged.confirm, true, newest);
                return;
            }
            Verdict::Unfinished => Verdict::Failed,
            verdict => verdict,
        };
        match verdict {
            Verdict::Good if judged.confirm => {
                self.end = Some(End::Found { lever: judged.undone[0], confirmed: true });
            }
            Verdict::Failed if judged.confirm => self.end = Some(End::NotFound),
            Verdict::Good => {
                self.candidates = judged.undone;
                self.halve(newest);
            }
            Verdict::Failed => {
                self.candidates.retain(|lever| !judged.undone.contains(lever));
                self.halve(newest);
            }
            Verdict::Otherwise => {
                let start = start.unwrap_or_default();
                self.end = Some(End::Otherwise { number: judged.number, start });
            }
            Verdict::NotStarted | Verdict::Unfinished => {
                self.end = Some(End::NotStarted { number: judged.number });
            }
        }
    }

    /// What the record of start `start` says of the try it was: good,
    /// failed as F did, failed otherwise, or unfinished.
    #[must_use]
    pub fn verdict_of(&self, start: &Start) -> Verdict {
        match start.outcome {
            Outcome::Good => Verdict::Good,
            Outcome::Failed if start.error == self.error && start.failing == self.failing => {
                Verdict::Failed
            }
            Outcome::Failed => Verdict::Otherwise,
            Outcome::Unfinished => Verdict::Unfinished,
        }
    }

    /// Takes the step the health check of start `this` takes while the
    /// search waits on a try: judges it by `this` when `this` is the
    /// try, or by the try's record among `records`, where its own check
    /// did not get to take the step; otherwise, as `block` (the try the
    /// boot menu's block holds, and whether it took it) says, as never
    /// finished when the menu took it, and as not started when not.
    /// Returns whether it judged a try.
    pub fn step(&mut self, this: &Start, records: &[Start], block: Option<(u8, bool)>) -> bool {
        let Some(waiting) = self.waiting().cloned() else { return false };
        let is_try =
            |start: &Start| start.trial == Some(waiting.number) && start.number > waiting.after;
        let newest = this.number.max(records.last().map_or(0, |last| last.number));
        if is_try(this) {
            self.judge(self.verdict_of(this), Some(this.number), newest);
        } else if let Some(record) =
            records.iter().find(|start| start.number != this.number && is_try(start))
        {
            self.judge(self.verdict_of(record), Some(record.number), newest);
        } else if let Some(other) = this.trial {
            let why = format!(
                "this start was try {other}, while the search waited on try {}",
                waiting.number
            );
            self.end = Some(End::Broken { number: waiting.number, why });
        } else if block == Some((waiting.number, true)) {
            self.judge(Verdict::Unfinished, None, newest);
        } else {
            self.judge(Verdict::NotStarted, None, newest);
        }
        true
    }

    /// Asks for one more try, after start `newest`, that undoes only the
    /// change the search named by elimination.
    ///
    /// # Errors
    ///
    /// If the search waits on a try, or named none by elimination.
    pub fn confirm(&mut self, newest: u64) -> Result<(), Refused> {
        if let Some(waiting) = self.waiting() {
            return Err(Refused::Running(waiting.number));
        }
        let Some(End::Found { lever, confirmed: false }) = self.end else {
            return Err(Refused::NothingToConfirm);
        };
        self.end = None;
        self.candidates = vec![lever];
        self.push(vec![lever], true, false, newest);
        Ok(())
    }

    /// Stops the search, wherever it is.
    pub fn stop(&mut self) {
        if self.end.is_none() {
            self.end = Some(End::Stopped);
        }
    }

    /// The start `t` makes: the slot, the options after the menu's own
    /// words, and the services Spark leaves out.
    #[must_use]
    pub fn plan(&self, t: &Try) -> Plan {
        let undoes = |kind| t.undone.iter().any(|&lever| self.levers[lever].kind == kind);
        let slot = if undoes(Kind::Slot) { &self.good_slot } else { &self.failed_slot };
        let options = if undoes(Kind::Options) { &self.good_options } else { &self.failed_options };
        let mut leave_out: Vec<String> = t
            .undone
            .iter()
            .flat_map(|&lever| self.levers[lever].services.iter().cloned())
            .collect();
        leave_out.sort();
        leave_out.dedup();
        Plan { slot: Slot::named(slot).unwrap_or(Slot::A), options: options.clone(), leave_out }
    }
}

/// The start a try makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub slot: Slot,
    pub options: String,
    pub leave_out: Vec<String>,
}

/// The search kept in `dir`, if there is one that reads.
#[must_use]
pub fn load_in(dir: &Path) -> Option<Search> {
    let bytes = std::fs::read(dir.join(STATE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Keeps `search` in `dir`, whole or not at all.
///
/// # Errors
///
/// If it cannot be written.
pub fn save_in(dir: &Path, search: &Search) -> std::io::Result<()> {
    keep_as(dir, STATE, search)
}

/// Keeps `search`, planned and not begun, in `dir`: a plan reads the slot
/// state, which only the system may read, and the page that offers the
/// search reads it as the person.
///
/// # Errors
///
/// If it cannot be written.
pub fn keep_planned_in(dir: &Path, search: &Search) -> std::io::Result<()> {
    keep_as(dir, PLANNED, search)
}

/// [`keep_planned_in`] [`DIR`].
///
/// # Errors
///
/// If it cannot be written.
pub fn keep_planned(search: &Search) -> std::io::Result<()> {
    keep_planned_in(Path::new(DIR), search)
}

/// The plan kept in `dir`, when it is of the failed start a search of
/// `records` would look into now and `kept`, the search kept, is none or
/// one that has ended about an older start.
#[must_use]
pub fn planned_in(dir: &Path, records: &[Start], kept: Option<&Search>) -> Option<Search> {
    let bytes = std::fs::read(dir.join(PLANNED)).ok()?;
    let plan: Search = serde_json::from_slice(&bytes).ok()?;
    let (failed, _) = failed_and_good(records).ok()?;
    let newer = kept.is_none_or(|search| search.end.is_some() && search.failed < plan.failed);
    (plan.failed == failed.number && newer).then_some(plan)
}

/// [`planned_in`] [`DIR`].
#[must_use]
pub fn planned(records: &[Start], kept: Option<&Search>) -> Option<Search> {
    planned_in(Path::new(DIR), records, kept)
}

/// Keeps `search` as `name` in `dir`, whole or not at all.
fn keep_as(dir: &Path, name: &str, search: &Search) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut bytes = serde_json::to_vec_pretty(search).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    write_whole(&dir.join(name), &bytes)
}

/// The path of try `number`'s list of services to leave out, in `dir`.
#[must_use]
pub fn leave_out_path(dir: &Path, number: u8) -> PathBuf {
    // The name Spark reads it by.
    dir.join(spark_api::bisect::file(number))
}

/// Writes the services try `t` of `search` leaves out in `dir`, a name a
/// line, whole or not at all.
///
/// # Errors
///
/// If it cannot be written.
pub fn write_leave_out_in(dir: &Path, search: &Search, t: &Try) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut text = String::new();
    for name in search.plan(t).leave_out {
        text.push_str(&name);
        text.push('\n');
    }
    write_whole(&leave_out_path(dir, t.number), text.as_bytes())
}

/// Removes every try's list in `dir`, as a new search begins.
pub fn forget_tries_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().starts_with("try-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Writes `bytes` as `path` through a file beside it, renamed over it.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// The block of try `t` of `search`, for the boot menu.
///
/// # Errors
///
/// If the try's options cannot be held, which [`Search::new`] and
/// [`levers`] rule out.
pub fn block_of(search: &Search, t: &Try) -> Result<[u8; slots::envblock::BLOCK], String> {
    let plan = search.plan(t);
    Trial::new(t.number, plan.slot, &plan.options)
        .map(|trial| trial.encode())
        .map_err(|why| why.to_string())
}

/// Which of `partitions` hold try `number` whole in their trial block,
/// and whether the boot menu took it on any: as the step reads the
/// block ([`Search::step`]).
pub(crate) fn block_says(partitions: &[String], number: u8) -> Option<(u8, bool)> {
    let mut holds = None;
    for name in partitions {
        let read = place::device(name, false).ok().and_then(|device| {
            disk::esp::read_file(device, trial::PATH, slots::envblock::BLOCK).ok().flatten()
        });
        if let Some((held, taken)) = read.as_deref().and_then(Trial::decode)
            && held.number == number
        {
            holds = Some((number, taken || holds == Some((number, true))));
        }
    }
    holds
}

/// Writes `block` as the trial block of every partition of
/// `partitions`.
///
/// # Errors
///
/// What went wrong on those that could not be written, when any could
/// not: a mirror's other disk would start the usual start, or an old
/// try.
pub(crate) fn write_blocks(partitions: &[String], block: &[u8]) -> Result<(), String> {
    let problems: Vec<String> = partitions
        .iter()
        .filter_map(|name| {
            place::device(name, true)
                .map_err(|why| why.message)
                .and_then(|device| disk::esp::overwrite_file(device, trial::PATH, block))
                .err()
                .map(|why| format!("/devices/{name}: {}: {why}", trial::PATH))
        })
        .collect();
    if partitions.is_empty() {
        return Err("the root's disks have no EFI system partition".into());
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

/// Writes what `search` now is in `dir` and on `partitions`: while it
/// waits on a try, the try's services to leave out, the search, then
/// the try's block; once it has ended, the blocks cleared, the tries'
/// lists removed, and the search. Returns the try to start, if one
/// waits; a try that cannot be written ends the search.
fn advance(dir: &Path, partitions: &[String], search: &mut Search) -> std::io::Result<Option<u8>> {
    if let Some(next) = search.waiting().cloned() {
        write_leave_out_in(dir, search, &next)?;
        save_in(dir, search)?;
        let written = block_of(search, &next).and_then(|block| write_blocks(partitions, &block));
        match written {
            Ok(()) => return Ok(Some(next.number)),
            Err(why) => search.end = Some(End::Broken { number: next.number, why }),
        }
    }
    if let Err(why) = write_blocks(partitions, &trial::cleared()) {
        println!("changes bisect: cannot clear the try: {why}");
    }
    forget_tries_in(dir);
    save_in(dir, search)?;
    Ok(None)
}

/// The health check's step of a search waiting on a try, at start `this`
/// of `records`, with the slot state's lock held ([`Search::step`]):
/// judges the try, then writes the next or ends the search. Returns the
/// try to restart into, if one waits, and the search, if one waited.
pub fn health_step(root: &Root, this: &Start, records: &[Start]) -> (Option<u8>, Option<Search>) {
    let dir = Path::new(DIR);
    let Some(mut search) = load_in(dir) else { return (None, None) };
    let Some(waiting) = search.waiting().map(|t| t.number) else { return (None, None) };
    let partitions = place::partitions(root).unwrap_or_default();
    search.step(this, records, block_says(&partitions, waiting));
    match advance(dir, &partitions, &mut search) {
        Ok(next) => (next, Some(search)),
        Err(err) => {
            println!("changes bisect: cannot keep the search in {DIR}: {err}");
            (None, Some(search))
        }
    }
}

/// Whether try `number` is still the one the search kept waits on: the
/// health check asks before it restarts, as `--stop` may have come.
#[must_use]
pub fn still_waiting_on(number: u8) -> bool {
    load_in(Path::new(DIR)).and_then(|search| search.waiting().map(|t| t.number)) == Some(number)
}

/// The services of the app installed for everyone with ID `id`, by the
/// records bundled keeps of the service files it wrote.
fn services_of(id: &str) -> Vec<String> {
    let dir = Path::new(bundles::registry::STATE).join(bundles::services::RECORDS);
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|text| bundles::services::Record::parse(&text))
                .is_some_and(|record| record.id == id)
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// The slot named by `letter` if the boot menu starts a try of it: the
/// default or the previous version.
pub(crate) fn startable(state: slots::State, letter: &str) -> Option<Slot> {
    Slot::named(letter).filter(|&slot| slot == state.default || state.previous == Some(slot))
}

/// Refuses while an update is being tried, which the boot menu starts in
/// place of any try.
fn no_update_tried(state: slots::State) -> Result<(), Fail> {
    match state.trying {
        Some(slot) => Err(fail(format!(
            "an update waits in slot {} to be tried, and the boot menu starts it in place of a \
             try: one thing at a time",
            slot.letter().to_ascii_uppercase()
        ))),
        None => Ok(()),
    }
}

/// Refuses while a memory test is asked for the next start (hardware(8)):
/// the start is a search's try or the test, and neither waits for the
/// other.
fn no_memory_test(asked: bool) -> Result<(), Fail> {
    if asked {
        return Err(fail(
            "a memory test is asked for the next start; `elevate hardware memtest --cancel` \
             first (HW-105)"
                .into(),
        ));
    }
    Ok(())
}

/// The search `changes bisect` would begin now, before anything is
/// written: from the newest failed start and the last good one, with
/// what the machine holds.
///
/// # Errors
///
/// What [`Refused`] says, and why the slots or the starts cannot be
/// read.
pub fn plan_now() -> Result<Search, Fail> {
    let root = place::root()?;
    let (_, state) = place::state(&root)?;
    no_update_tried(state)?;
    no_memory_test(crate::hardware::memtest::is_asked())?;
    if let Some(waiting) = load_in(Path::new(DIR)).as_ref().and_then(Search::waiting) {
        return Err(fail(Refused::Running(waiting.number).to_string()));
    }
    let records = starts::all();
    let (failed, good) = failed_and_good(&records).map_err(|why| fail(why.to_string()))?;
    if startable(state, &failed.slot).is_none() {
        return Err(fail(format!(
            "start {} ran slot {}, which the boot menu no longer starts",
            failed.number,
            failed.slot.to_uppercase()
        )));
    }
    let good_slot = (good.slot != failed.slot)
        .then(|| startable(state, &good.slot))
        .flatten()
        .map(|slot| super::health::slot_items(&root, slot));
    let now = Now { good_slot, services: &services_of };
    let (levers, apart) = levers(&records, good, failed, &now);
    let newest = records.last().map_or(0, |last| last.number);
    let at = i64::try_from(crate::util::now_ns() / 1_000_000_000).unwrap_or(0);
    Search::new(good, failed, levers, apart, at, newest).map_err(|why| fail(why.to_string()))
}

/// Begins `search` ([`plan_now`]), or the try `--confirm` asks for of
/// the search kept, when `confirm`: writes its first try, under the slot
/// state's lock, and returns its number, to restart into.
///
/// # Errors
///
/// If an update came to be tried or a search began meanwhile, or the try
/// cannot be written.
pub fn write_first(mut search: Search) -> Result<u8, Fail> {
    let root = place::root()?;
    let _lock = place::lock()?;
    let (_, state) = place::state(&root)?;
    no_update_tried(state)?;
    no_memory_test(crate::hardware::memtest::is_asked())?;
    let dir = Path::new(DIR);
    let kept = load_in(dir);
    if let Some(waiting) = kept.as_ref().and_then(Search::waiting)
        && kept.as_ref().map(|kept| kept.begun) != Some(search.begun)
    {
        return Err(fail(Refused::Running(waiting.number).to_string()));
    }
    if search.tries.len() == 1 {
        forget_tries_in(dir);
    }
    let partitions = place::partitions(&root)?;
    match advance(dir, &partitions, &mut search) {
        Ok(Some(number)) => Ok(number),
        Ok(None) => Err(fail(match search.end {
            Some(End::Broken { why, .. }) => format!("the try cannot be written: {why}"),
            _ => "the search has no try to write".into(),
        })),
        Err(err) => Err(crate::util::failed(&DIR, err)),
    }
}

/// The search kept, with the try `--confirm` asks for added: one that
/// undoes only the change named by elimination.
///
/// # Errors
///
/// If no search ran, it waits on a try, or it named no change by
/// elimination; or an update is being tried.
pub fn confirm_now() -> Result<Search, Fail> {
    let root = place::root()?;
    let (_, state) = place::state(&root)?;
    no_update_tried(state)?;
    no_memory_test(crate::hardware::memtest::is_asked())?;
    let mut search = load_in(Path::new(DIR)).ok_or_else(|| fail(Refused::NoSearch.to_string()))?;
    let newest = starts::all().last().map_or(0, |last| last.number);
    search.confirm(newest).map_err(|why| fail(why.to_string()))?;
    Ok(search)
}

/// Stops the search kept, wherever it is, and clears the try from the
/// boot menu's blocks. Returns the search, if there is one.
///
/// # Errors
///
/// If the slots or the search cannot be read or written.
pub fn stop() -> Result<Option<Search>, Fail> {
    let root = place::root()?;
    let _lock = place::lock()?;
    let dir = Path::new(DIR);
    let partitions = place::partitions(&root)?;
    let Some(mut search) = load_in(dir) else {
        write_blocks(&partitions, &trial::cleared()).map_err(fail)?;
        return Ok(None);
    };
    search.stop();
    advance(dir, &partitions, &mut search).map_err(|err| crate::util::failed(&DIR, err))?;
    Ok(Some(search))
}

/// The search kept, if there is one.
#[must_use]
pub fn kept() -> Option<Search> {
    load_in(Path::new(DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A start numbered `number` of slot `slot`, `outcome`, running
    /// `items`.
    fn start(number: u64, slot: &str, outcome: Outcome, items: Vec<Item>) -> Start {
        let failed = outcome == Outcome::Failed;
        Start {
            number,
            started: i64::try_from(number).unwrap() * 100,
            slot: slot.into(),
            stage: "services".into(),
            outcome,
            error: failed.then(|| "BOOT-102".into()),
            why: failed.then(|| "Spark gave up on sshd: it failed to start (BOOT-102)".into()),
            failing: failed.then(|| "sshd".into()),
            trial: None,
            items,
            times: None,
        }
    }

    fn system(version: &str, set: &str) -> Vec<Item> {
        vec![
            Item::new("system", "Dukovo", version, Some(10)),
            Item::new("kernel", "Radix", &format!("k{version}"), Some(10)),
            Item::new("firmware", "firmware set", set, Some(20)),
        ]
    }

    fn options(words: &str) -> Item {
        Item::new("options", "boot options", &format!("root=UUID=r panic=10 {words}"), None)
    }

    fn app(id: &str, version: &str) -> Item {
        Item::new("app", id, version, Some(30))
    }

    /// The services of each app: none for `quiet-*` ones.
    fn services(id: &str) -> Vec<String> {
        if id.starts_with("quiet") { Vec::new() } else { vec![format!("{id}-web"), id.to_string()] }
    }

    /// A good start of slot A and a failed one of slot B with `apps` new.
    fn pair(apps: &[&str]) -> (Start, Start) {
        let mut good = system("0.16.0", "2026.10");
        good.push(options("quiet"));
        let mut failed = system("0.16.1", "2026.11");
        failed.push(options("quiet loglevel=debug"));
        failed.extend(apps.iter().map(|id| app(id, "1.0")));
        (start(1, "a", Outcome::Good, good), start(2, "b", Outcome::Failed, failed))
    }

    fn now(good: &Start) -> Now<'static> {
        Now { good_slot: Some(good.items[..3].to_vec()), services: &services }
    }

    #[test]
    fn finds_the_slot_the_options_and_the_apps_with_services_as_levers() {
        let (good, mut failed) = pair(&["nginx", "quiet-tool", "beta"]);
        failed.items.push(Item::new("card", "0000:00:03.0", "Intel 82540EM", None));
        let records = [good.clone(), failed.clone()];
        let (levers, apart) = levers(&records, &good, &failed, &now(&good));
        let names: Vec<(Kind, &str)> = levers.iter().map(|l| (l.kind, l.name.as_str())).collect();
        assert_eq!(
            names,
            [
                (Kind::Slot, "system"),
                (Kind::Options, "boot options"),
                (Kind::App, "beta"),
                (Kind::App, "nginx")
            ]
        );
        assert_eq!(levers[0].before.as_deref(), Some("Dukovo 0.16.0 with firmware set 2026.10"));
        assert_eq!(levers[0].after, "Dukovo 0.16.1 with firmware set 2026.11");
        assert_eq!(levers[1].before.as_deref(), Some("quiet"));
        assert_eq!(levers[1].after, "quiet loglevel=debug");
        assert_eq!(levers[3].services, ["nginx", "nginx-web"]);
        assert_eq!((levers[3].before.as_deref(), levers[3].after.as_str()), (None, "1.0"));
        assert_eq!(levers[3].first_used, Some((2, 200)));
        let whys: Vec<(&str, &Why)> = apart.iter().map(|a| (a.name.as_str(), &a.why)).collect();
        assert_eq!(whys, [("quiet-tool", &Why::NoServices), ("0000:00:03.0", &Why::Hardware)]);
    }

    #[test]
    fn a_slot_rebuilt_since_or_the_same_slot_cannot_be_tried_apart() {
        let (good, failed) = pair(&["nginx"]);
        let records = [good.clone(), failed.clone()];
        let rebuilt = Now { good_slot: Some(system("0.16.2", "2026.11")), services: &services };
        let gone = Now { good_slot: None, services: &services };
        for now in [rebuilt, gone] {
            let (levers, apart) = levers(&records, &good, &failed, &now);
            assert!(levers.iter().all(|l| l.kind != Kind::Slot));
            assert_eq!((apart[0].name.as_str(), &apart[0].why), ("Dukovo", &Why::SlotGone));
        }
        // An update built over the slot that ran: G's system is gone.
        let same = Start { slot: "a".into(), ..failed.clone() };
        let (levers, apart) = levers(&records, &good, &same, &now(&good));
        assert!(levers.iter().all(|l| l.kind != Kind::Slot));
        assert_eq!(apart[0].why, Why::SlotGone);
        // Only the kernel changed: named by it.
        let mut kernel = good.clone();
        kernel.slot = "b".into();
        kernel.items[1].value = "kother".into();
        let (levers, _) = super::levers(&records, &good, &kernel, &now(&good));
        assert_eq!(levers[0].after, "Dukovo 0.16.0, kernel kother with firmware set 2026.10");
    }

    #[test]
    fn options_a_try_cannot_hold_or_only_the_menu_changed_are_set_apart() {
        let (good, mut failed) = pair(&[]);
        // An entry of Troubleshooting adds words before the root's.
        failed.items[3] =
            Item::new("options", "boot options", "essentials root=UUID=r panic=10 quiet", None);
        let records = [good.clone(), failed.clone()];
        let (levers, apart) = levers(&records, &good, &failed, &now(&good));
        assert_eq!(levers.len(), 1);
        assert_eq!(apart[0].why, Why::MenuWords);
        let mut odd = good.clone();
        odd.items[3] = options("quiet note=\"x\"");
        let (_, apart) = super::levers(&records, &odd, &pair(&[]).1, &now(&good));
        assert!(
            matches!(&apart[0].why, Why::Options(why) if why.contains("note=\"x\"")),
            "{apart:?}"
        );
        // F's own options a try cannot pass: no search.
        let (levers, apart) = super::levers(&records, &pair(&[]).1, &odd, &now(&good));
        let refused = Search::new(&pair(&[]).1, &odd, levers, apart, 0, 2).unwrap_err();
        assert!(matches!(refused, Refused::Options(_)), "{refused:?}");
    }

    #[test]
    fn apps_gone_since_cannot_be_tried() {
        let (mut good, failed) = pair(&["nginx"]);
        good.items.push(app("old", "2.0"));
        let records = [good.clone(), failed.clone()];
        let (levers, apart) = levers(&records, &good, &failed, &now(&good));
        assert_eq!(levers.len(), 3);
        assert_eq!((apart[0].name.as_str(), &apart[0].why), ("old", &Why::Gone));
    }

    /// `n` app levers, as a search of them begins.
    fn search_of(n: usize) -> Search {
        let ids: Vec<String> = (0..n).map(|i| format!("app{i}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (mut good, mut failed) = pair(&ids);
        // Only the apps changed.
        failed.items.retain(|item| item.kind == "app");
        failed.items.extend(good.items.iter().cloned());
        good.slot = "b".into();
        let records = [good.clone(), failed.clone()];
        let (levers, apart) = levers(&records, &good, &failed, &now(&good));
        assert_eq!(levers.len(), n);
        Search::new(&good, &failed, levers, apart, 1000, 2).unwrap()
    }

    /// `search` run to its end with the cause at `cause`, each try
    /// starting well when it undoes the cause.
    fn ended(mut search: Search, cause: usize) -> Search {
        let mut newest = 2;
        while let Some(waiting) = search.waiting().cloned() {
            newest += 1;
            let verdict =
                if waiting.undone.contains(&cause) { Verdict::Good } else { Verdict::Failed };
            search.judge(verdict, Some(newest), newest);
            assert!(search.tries.len() <= 20, "the search ends");
        }
        search
    }

    /// The end of `search` run with the cause at `cause`, and its tries.
    fn run(search: Search, cause: usize) -> (End, usize) {
        let search = ended(search, cause);
        (search.end.clone().unwrap(), search.tries.len())
    }

    #[test]
    fn halving_names_the_cause_at_every_place_within_the_restarts_it_says() {
        for n in 2..=9 {
            let most = restarts(n) as usize;
            assert_eq!(search_of(n).restarts_left(), restarts(n));
            for cause in 0..n {
                let search = ended(search_of(n), cause);
                let end = search.end.clone().unwrap();
                let End::Found { lever, confirmed } = end else { panic!("{n} {cause}: {end:?}") };
                assert_eq!(lever, cause, "{n} levers, cause at {cause}");
                let tries = search.tries.len();
                assert!(tries <= most, "{n} levers, cause at {cause}: {tries} tries");
                // Confirmed exactly when the last try undid it alone and
                // started well.
                let last = search.tries.last().unwrap();
                assert_eq!(
                    confirmed,
                    last.verdict == Some(Verdict::Good) && last.undone == [cause],
                    "{n} {cause}"
                );
            }
        }
        assert_eq!((restarts(4), restarts(8), restarts(9)), (2, 3, 4));
    }

    #[test]
    fn four_levers_name_the_first_app_confirmed_and_the_options_by_elimination() {
        // The boot test's plants: the slot, the options and two apps.
        let (good, failed) = pair(&["bad", "good"]);
        let records = [good.clone(), failed.clone()];
        let (levers, apart) = levers(&records, &good, &failed, &now(&good));
        let search = Search::new(&good, &failed, levers, apart, 0, 2).unwrap();
        assert_eq!(search.restarts_left(), 2);
        let first = search.waiting().unwrap();
        assert_eq!((first.number, first.undone.as_slice()), (1, &[0, 1][..]));
        let plan = search.plan(first);
        assert_eq!(plan, Plan { slot: Slot::A, options: "quiet".into(), leave_out: Vec::new() });
        assert_eq!(run(search.clone(), 2), (End::Found { lever: 2, confirmed: true }, 2));
        let (end, tries) = run(search.clone(), 1);
        assert_eq!((end, tries), (End::Found { lever: 1, confirmed: false }, 2));
        // --confirm: one more try, undoing only the options.
        let mut search = search;
        search.judge(Verdict::Good, Some(3), 3);
        search.judge(Verdict::Failed, Some(4), 4);
        assert_eq!(search.end, Some(End::Found { lever: 1, confirmed: false }));
        search.confirm(4).unwrap();
        let confirming = search.waiting().unwrap().clone();
        assert_eq!((confirming.number, confirming.undone.as_slice()), (3, &[1][..]));
        assert!(confirming.confirm);
        assert_eq!(search.restarts_left(), 1);
        let plan = search.plan(&confirming);
        assert_eq!((plan.slot, plan.options.as_str()), (Slot::B, "quiet"));
        assert_eq!(search.confirm(4), Err(Refused::Running(3)));
        let mut failed_again = search.clone();
        search.judge(Verdict::Good, Some(5), 5);
        assert_eq!(search.end, Some(End::Found { lever: 1, confirmed: true }));
        assert_eq!(search.confirm(5), Err(Refused::NothingToConfirm));
        // Failing without it too: no one change explains it.
        failed_again.judge(Verdict::Failed, Some(5), 5);
        assert_eq!(failed_again.end, Some(End::NotFound));
    }

    #[test]
    fn a_try_leaves_out_the_services_of_the_apps_it_undoes() {
        let mut search = search_of(4);
        search.judge(Verdict::Failed, Some(3), 3);
        let second = search.waiting().unwrap();
        assert_eq!(second.undone, [2]);
        let plan = search.plan(second);
        assert_eq!(plan.leave_out, ["app2", "app2-web"]);
        assert_eq!((plan.slot, plan.options.as_str()), (Slot::B, "quiet"));
    }

    #[test]
    fn a_lone_lever_is_tried_on_its_own() {
        let mut search = search_of(1);
        assert_eq!(search.restarts_left(), 1);
        let only = search.waiting().unwrap();
        assert!(only.confirm && only.undone == [0]);
        let mut fails = search.clone();
        search.judge(Verdict::Good, Some(3), 3);
        assert_eq!(search.end, Some(End::Found { lever: 0, confirmed: true }));
        fails.judge(Verdict::Failed, Some(3), 3);
        assert_eq!(fails.end, Some(End::NotFound));
    }

    #[test]
    fn a_try_that_fails_otherwise_ends_the_search() {
        let mut search = search_of(4);
        search.judge(Verdict::Otherwise, Some(3), 3);
        assert_eq!(search.end, Some(End::Otherwise { number: 1, start: 3 }));
        assert!(search.waiting().is_none());
        // Judging after the end does nothing.
        search.judge(Verdict::Good, Some(4), 4);
        assert_eq!(search.tries.len(), 1);
    }

    #[test]
    fn a_try_unfinished_is_tried_again_and_counts_as_failed_the_second_time() {
        let mut search = search_of(4);
        search.judge(Verdict::Unfinished, None, 3);
        let again = search.waiting().unwrap().clone();
        assert_eq!((again.number, again.undone.as_slice(), again.again), (2, &[0, 1][..], true));
        assert_eq!(again.after, 3);
        search.judge(Verdict::Unfinished, None, 4);
        // Counted as failed: the cause is in the half it kept.
        assert_eq!(search.candidates, [2, 3]);
        let third = search.waiting().unwrap();
        assert_eq!((third.number, third.undone.as_slice(), third.again), (3, &[2][..], false));
        // An unfinished try that is tried again and then judged.
        let mut search = search_of(4);
        search.judge(Verdict::Unfinished, None, 3);
        search.judge(Verdict::Good, Some(4), 4);
        assert_eq!(search.candidates, [0, 1]);
        // A confirming try unfinished twice: failed, so nothing found.
        let mut search = search_of(1);
        search.judge(Verdict::Unfinished, None, 3);
        assert!(search.waiting().unwrap().confirm);
        search.judge(Verdict::Unfinished, None, 4);
        assert_eq!(search.end, Some(End::NotFound));
    }

    #[test]
    fn a_search_is_refused_while_a_memory_test_is_asked_for_the_next_start() {
        assert!(no_memory_test(false).is_ok());
        let refused = no_memory_test(true).unwrap_err();
        assert_eq!(refused.code, 1);
        assert_eq!(
            refused.message,
            "a memory test is asked for the next start; `elevate hardware memtest --cancel` \
             first (HW-105)"
        );
    }

    #[test]
    fn the_health_check_judges_the_try_by_its_record_or_the_block() {
        let search = search_of(4);
        let failed = start(2, "b", Outcome::Failed, Vec::new());
        let records = vec![start(1, "a", Outcome::Good, Vec::new()), failed.clone()];
        let tried =
            |number, outcome| Start { trial: Some(1), ..start(number, "b", outcome, Vec::new()) };
        // This start is the try, good.
        let mut s = search.clone();
        assert!(s.step(&tried(3, Outcome::Good), &records, Some((1, true))));
        assert_eq!(s.tries[0].verdict, Some(Verdict::Good));
        assert_eq!(s.tries[0].start, Some(3));
        assert_eq!(s.waiting().unwrap().after, 3);
        // Failed as F, and otherwise.
        let mut s = search.clone();
        s.step(&tried(3, Outcome::Failed), &records, Some((1, true)));
        assert_eq!(s.tries[0].verdict, Some(Verdict::Failed));
        let mut s = search.clone();
        let mut other = tried(3, Outcome::Failed);
        other.error = Some("BOOT-103".into());
        s.step(&other, &records, Some((1, true)));
        assert_eq!(s.end, Some(End::Otherwise { number: 1, start: 3 }));
        let mut s = search.clone();
        other.error = Some("BOOT-102".into());
        other.failing = Some("netd".into());
        s.step(&other, &records, Some((1, true)));
        assert_eq!(s.end, Some(End::Otherwise { number: 1, start: 3 }));
        // A usual start after the try: the try's record, finished while
        // its own step was cut short.
        let mut s = search.clone();
        let mut with_try = records.clone();
        with_try.push(tried(3, Outcome::Good));
        let usual = start(4, "b", Outcome::Failed, Vec::new());
        s.step(&usual, &with_try, Some((1, true)));
        assert_eq!((s.tries[0].verdict, s.tries[0].start), (Some(Verdict::Good), Some(3)));
        // No record of it: taken by the menu, so unfinished, and tried
        // again; not taken, so not started.
        let mut s = search.clone();
        s.step(&usual, &records, Some((1, true)));
        assert_eq!(s.tries[0].verdict, Some(Verdict::Unfinished));
        assert_eq!(s.waiting().unwrap().number, 2);
        for block in [Some((1, false)), None, Some((7, true))] {
            let mut s = search.clone();
            s.step(&usual, &records, block);
            assert_eq!(s.end, Some(End::NotStarted { number: 1 }), "{block:?}");
        }
        // A try of an older search, before this one's was written, is not
        // this one's.
        let mut s = search.clone();
        let mut old = records.clone();
        old.insert(0, tried(0, Outcome::Good));
        s.step(&usual, &old, Some((1, false)));
        assert_eq!(s.end, Some(End::NotStarted { number: 1 }));
        // Another try than the one waited on: the search lost track.
        let mut s = search.clone();
        let stray = Start { trial: Some(5), ..start(3, "b", Outcome::Good, Vec::new()) };
        s.step(&stray, &records, Some((5, true)));
        assert!(matches!(s.end, Some(End::Broken { number: 1, .. })), "{:?}", s.end);
        // Nothing waits: nothing to do.
        let mut s = search.clone();
        s.stop();
        assert!(!s.step(&usual, &records, None));
        assert_eq!(s.end, Some(End::Stopped));
    }

    #[test]
    fn the_failed_and_the_good_start_are_the_newest_failure_and_the_good_before_it() {
        let usual = |number, outcome| start(number, "a", outcome, Vec::new());
        let tried = |number, outcome| Start { trial: Some(1), ..usual(number, outcome) };
        assert_eq!(failed_and_good(&[]).unwrap_err(), Refused::NoFailure);
        let records = [usual(1, Outcome::Good), usual(2, Outcome::Failed), tried(3, Outcome::Good)];
        let (failed, good) = failed_and_good(&records).unwrap();
        assert_eq!((failed.number, good.number), (2, 1));
        let records = [usual(1, Outcome::Good), usual(2, Outcome::Failed), usual(3, Outcome::Good)];
        assert_eq!(failed_and_good(&records).unwrap_err(), Refused::GoodSince(3));
        let records = [tried(1, Outcome::Good), usual(2, Outcome::Failed)];
        assert_eq!(failed_and_good(&records).unwrap_err(), Refused::NoGood(2));
        let records = [usual(1, Outcome::Good), tried(2, Outcome::Failed)];
        assert_eq!(failed_and_good(&records).unwrap_err(), Refused::NoFailure);
    }

    #[test]
    fn nothing_to_try_apart_is_refused() {
        let (good, mut failed) = pair(&["quiet-tool"]);
        failed.items = good.items.clone();
        failed.items.push(app("quiet-tool", "1.0"));
        let records = [good.clone(), failed.clone()];
        let (levers, apart) = levers(&records, &good, &failed, &now(&good));
        assert_eq!(Search::new(&good, &failed, levers, apart, 0, 2), Err(Refused::NoLever));
    }

    #[test]
    fn the_search_and_its_tries_are_kept_whole() {
        let dir = std::env::temp_dir().join(format!("bisect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_in(&dir), None);
        let mut search = search_of(4);
        search.judge(Verdict::Failed, Some(3), 3);
        save_in(&dir, &search).unwrap();
        assert_eq!(load_in(&dir), Some(search.clone()));
        let waiting = search.waiting().unwrap();
        write_leave_out_in(&dir, &search, waiting).unwrap();
        let listed = std::fs::read_to_string(leave_out_path(&dir, 2)).unwrap();
        assert_eq!(listed, "app2\napp2-web\n");
        // What Spark reads: the same file name, and the names the list holds.
        let file = leave_out_path(&dir, 2);
        assert_eq!(file.file_name().and_then(|name| name.to_str()), Some("try-2"));
        assert_eq!(file.file_name().unwrap().to_string_lossy(), spark_api::bisect::file(2));
        assert_eq!(spark_api::bisect::names(&listed), ["app2", "app2-web"]);
        let block = block_of(&search, waiting).unwrap();
        let (read, taken) = Trial::decode(&block).unwrap();
        assert_eq!(
            (read.number, read.slot, read.options.as_str(), taken),
            (2, Slot::B, "quiet", false)
        );
        forget_tries_in(&dir);
        assert!(!leave_out_path(&dir, 2).exists());
        assert!(dir.join(STATE).exists(), "the search stays");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(names.len(), 1, "no partial file is left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plan_is_read_while_it_is_of_the_newest_failed_start_and_no_search_is_of_it() {
        let dir = std::env::temp_dir().join(format!("bisect-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let usual = |number, outcome| start(number, "a", outcome, Vec::new());
        let records = [usual(1, Outcome::Good), usual(2, Outcome::Failed)];
        assert_eq!(planned_in(&dir, &records, None), None, "no plan is kept");
        let plan = search_of(4);
        keep_planned_in(&dir, &plan).unwrap();
        assert_eq!(planned_in(&dir, &records, None), Some(plan.clone()));
        // A search of that start, running or ended, is shown in its place.
        assert_eq!(planned_in(&dir, &records, Some(&plan)), None);
        let mut done = plan.clone();
        done.stop();
        assert_eq!(planned_in(&dir, &records, Some(&done)), None);
        // One that ended about an older start is not.
        let older = Search { failed: 1, ..done };
        assert_eq!(planned_in(&dir, &records, Some(&older)), Some(plan));
        // A newer failed start, or a good start since, leaves the plan behind.
        let newer = [usual(1, Outcome::Good), usual(2, Outcome::Failed), usual(3, Outcome::Failed)];
        assert_eq!(planned_in(&dir, &newer, None), None);
        let good = [usual(1, Outcome::Good), usual(2, Outcome::Failed), usual(3, Outcome::Good)];
        assert_eq!(planned_in(&dir, &good, None), None);
        assert!(!dir.join(STATE).exists(), "a plan is no search");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
