// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The drafts reportd keeps (ADR 0568): the text it drafted, in memory,
//! for the person who asked, so that what is sent is that text and not
//! one a page sends back.
//!
//! A draft is kept by a random 128-bit ID, for its owner's user ID alone,
//! with the SHA-256 of its text as it stands. `send`, `text` and an edit
//! name both: an ID that is not the asker's, or has ended, is RPT-101;
//! a hash that is not the text's now is RPT-102, the text having changed
//! since the page read it. reportd keeps at most [`EACH`] drafts for a
//! person and [`ALL`] in all, each for [`MINUTES`] minutes from its last
//! change; a newer draft past either limit pushes out the oldest, and a
//! restart forgets them all.
//!
//! A person changes a draft with a list of edits ([`Edits`]), never with a
//! text: the edits are applied to the draft as reportd drafted it, so that
//! applying the same list twice gives the same text and the page holds
//! nothing reportd does not. A line the system gathered (a crash, a failed
//! start, a log) can be cut whole, or words of it cut to `[cut]`, but not
//! typed into; the head, which says where the report comes from, and the
//! headings cannot be changed at all; the person's answers can be typed
//! again. The text is then put together again from the drafted sections,
//! with a head line that counts the cuts, and redacted again whole, the
//! answers with the rest.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use ring::rand::{SecureRandom, SystemRandom};

use super::server::hex;
use crate::reports::draft::{self, Section};
use crate::reports::protocol::{
    ANSWER_MAX, CUTS_MAX, Cut, CutBy, CutByYou, Drafted, EDITS_MAX, Edits, Place, Size,
};
use crate::reports::redact::{self, Redactor};

/// How long a draft is kept after it was made or last changed.
pub const MINUTES: u64 = 60;
/// The most drafts kept for one person.
pub const EACH: usize = 8;
/// The most drafts kept in all.
pub const ALL: usize = 64;

/// The refusal for a draft that is not kept for the asker.
pub const ENDED: &str = "That draft has ended: draft it again (RPT-101)";
/// The refusal for a hash that is not the draft's text's now.
pub const NOT_READ: &str =
    "The report is not the one you read: read it again, then send it (RPT-102)";

/// The sections a person may cut lines of: those the system gathered.
const GATHERED: [&str; 4] = [draft::CRASH, draft::FAILED_START, draft::LOG, draft::START_LOG];

/// The part of the answer the head and the answers are counted as.
const HEAD_TITLE: &str = "What this computer says of itself";
const ANSWERS_TITLE: &str = "Your answers";

/// What a draft is made from, as reportd drafted it: its sections, each
/// redacted, and whether the parts it was drafted from had a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub sections: Vec<Section>,
    pub has_logs: bool,
}

impl Base {
    /// The base of `sections`, which are not yet redacted: redacted whole,
    /// as one text, and taken apart again by the same sections, since
    /// redacting leaves every line where it is.
    pub fn redacted(sections: &[Section], has_logs: bool, redactor: &Redactor) -> Base {
        let (text, _) = redactor.redact(&draft::text_of(sections));
        Base { sections: draft::split_like(sections, &text), has_logs }
    }
}

/// A draft kept.
#[derive(Debug)]
struct Kept {
    id: String,
    user: u32,
    /// The order it was made in, to push out the oldest.
    made: u64,
    /// When it ends, in seconds since 1970.
    until: u64,
    base: Base,
    /// The answer for the text as it stands, with its text and hash.
    answer: Drafted,
}

/// The drafts reportd keeps.
#[derive(Debug, Default)]
pub struct Drafts {
    kept: Mutex<Vec<Kept>>,
    /// How many drafts were made, for their order.
    made: Mutex<u64>,
}

/// A draft taken out to be sent, which is put back if it cannot be.
#[derive(Debug)]
pub struct Taken(Kept);

impl Taken {
    /// The text it holds.
    pub fn text(&self) -> &str {
        &self.0.answer.text
    }
}

impl Drafts {
    /// Keeps a new draft of `base` for `user`, and answers it as `draft`
    /// does, with its ID and hash.
    ///
    /// # Errors
    ///
    /// When no random ID can be made.
    pub fn keep(
        &self,
        user: u32,
        base: Base,
        redactor: &Redactor,
        now: u64,
    ) -> Result<Drafted, String> {
        let mut bytes = [0u8; 16];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| String::from("reportd cannot make a draft's ID: no random bytes"))?;
        let id = hex(&bytes);
        let mut answer = apply(&base, &Edits::default(), redactor)?;
        answer.draft = Some(id.clone());
        let made = {
            let mut made = self.made.lock().unwrap_or_else(PoisonError::into_inner);
            *made += 1;
            *made
        };
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.retain(|draft| draft.until > now);
        while kept.iter().filter(|draft| draft.user == user).count() >= EACH {
            drop_oldest(&mut kept, Some(user));
        }
        while kept.len() >= ALL {
            drop_oldest(&mut kept, None);
        }
        kept.push(Kept { id, user, made, until: now + MINUTES * 60, base, answer: answer.clone() });
        Ok(answer)
    }

    /// Applies `edits` to `user`'s draft `id`, whose text's hash must be
    /// `sha256`, keeps the result as the draft's text, and answers it. The
    /// draft's minutes start again.
    ///
    /// # Errors
    ///
    /// RPT-101, RPT-102, and an edit that is refused, with its ID and place.
    pub fn edit(
        &self,
        user: u32,
        id: &str,
        sha256: &str,
        edits: &Edits,
        redactor: &Redactor,
        now: u64,
    ) -> Result<Drafted, String> {
        // Applied with the lock let go, since it reads up to a report's
        // whole text; a change made meanwhile is a hash that no longer
        // matches, as a page that read an older text gets.
        let base = self.with(user, id, sha256, now, |kept| kept.base.clone())?;
        let mut answer = apply(&base, edits, redactor)?;
        answer.draft = Some(id.to_string());
        self.with(user, id, sha256, now, |kept| {
            kept.answer = answer.clone();
            kept.until = now + MINUTES * 60;
        })?;
        Ok(answer)
    }

    /// The text of `user`'s draft `id`, whose hash must be `sha256`.
    ///
    /// # Errors
    ///
    /// RPT-101 and RPT-102.
    pub fn text(&self, user: u32, id: &str, sha256: &str, now: u64) -> Result<String, String> {
        self.with(user, id, sha256, now, |kept| kept.answer.text.clone())
    }

    /// Takes `user`'s draft `id`, whose hash must be `sha256`, out of those
    /// kept, to send it: it is put back with [`Drafts::put_back`] if it
    /// cannot be queued, so that a second send of it is RPT-101 only once
    /// it is.
    ///
    /// # Errors
    ///
    /// RPT-101 and RPT-102.
    pub fn take(&self, user: u32, id: &str, sha256: &str, now: u64) -> Result<Taken, String> {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.retain(|draft| draft.until > now);
        let at = find(&kept, user, id, sha256)?;
        Ok(Taken(kept.remove(at)))
    }

    /// Puts back a draft that was taken and could not be queued.
    pub fn put_back(&self, taken: Taken) {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).push(taken.0);
    }

    /// Does `then` with `user`'s draft `id`, whose hash must be `sha256`.
    fn with<T>(
        &self,
        user: u32,
        id: &str,
        sha256: &str,
        now: u64,
        then: impl FnOnce(&mut Kept) -> T,
    ) -> Result<T, String> {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.retain(|draft| draft.until > now);
        let at = find(&kept, user, id, sha256)?;
        Ok(then(&mut kept[at]))
    }

    /// How many drafts are kept, ended or not.
    #[cfg(test)]
    fn count(&self) -> usize {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// Where `user`'s draft `id` is in `kept`, if its hash is `sha256`. A
/// draft of another person is not told from one that is not kept.
fn find(kept: &[Kept], user: u32, id: &str, sha256: &str) -> Result<usize, String> {
    let at = kept
        .iter()
        .position(|draft| draft.user == user && draft.id == id)
        .ok_or_else(|| ENDED.to_string())?;
    if kept[at].answer.sha256.as_deref() == Some(sha256) {
        Ok(at)
    } else {
        Err(NOT_READ.to_string())
    }
}

/// Drops the oldest draft of `user`, or of anyone.
fn drop_oldest(kept: &mut Vec<Kept>, user: Option<u32>) {
    let oldest = kept
        .iter()
        .enumerate()
        .filter(|(_, draft)| user.is_none_or(|user| draft.user == user))
        .min_by_key(|(_, draft)| draft.made)
        .map(|(at, _)| at);
    if let Some(at) = oldest {
        kept.remove(at);
    }
}

/// The edits of one section, put in order: the lines cut whole, and the
/// words cut of each other line, as character ranges, merged where they
/// meet.
#[derive(Debug, Default)]
struct SectionCuts {
    lines: BTreeSet<usize>,
    words: BTreeMap<usize, Vec<(usize, usize)>>,
}

/// The text `edits` make of `base`, with what the answer says of it: its
/// hash, the sizes of its parts, the marks in it, where each section
/// starts, and how much the person cut.
///
/// # Errors
///
/// An edit that is refused: of the head (RPT-104), of a line, a section or
/// an offset that is not there, or one that types into a gathered line
/// (RPT-105); too many cuts, or answers too long.
pub fn apply(base: &Base, edits: &Edits, redactor: &Redactor) -> Result<Drafted, String> {
    let size = serde_json::to_vec(edits).map_or(usize::MAX, |bytes| bytes.len());
    if size > EDITS_MAX {
        return Err(format!(
            "The changes are more than {} KiB: cut fewer at once",
            EDITS_MAX / 1024
        ));
    }
    if edits.cuts.len() > CUTS_MAX {
        return Err(format!("A report takes at most {CUTS_MAX} cuts: cut runs of lines whole"));
    }
    let cuts = checked(&base.sections, &edits.cuts)?;
    let mut sections: Vec<Section> = Vec::with_capacity(base.sections.len() + 1);
    let mut you = CutByYou::default();
    for section in &base.sections {
        if draft::ANSWERS.contains(&section.id) && edits.answers.is_some() {
            continue;
        }
        let Some(cut) = cuts.get(section.id) else {
            sections.push(section.clone());
            continue;
        };
        let mut lines = Vec::with_capacity(section.lines.len());
        for (at, line) in section.lines.iter().enumerate() {
            if cut.lines.contains(&at) {
                you.lines += 1;
                continue;
            }
            match cut.words.get(&at) {
                Some(ranges) => {
                    you.parts += ranges.len() as u32;
                    lines.push(cut_words(line, ranges));
                }
                None => lines.push(line.clone()),
            }
        }
        sections.push(Section { lines, ..section.clone() });
    }
    if let Some(answers) = &edits.answers {
        for (field, answer) in [
            ("happened", &answers.happened),
            ("expected", &answers.expected),
            ("again", &answers.again),
        ] {
            if answer.chars().count() > ANSWER_MAX {
                return Err(format!(
                    "An answer is at most {ANSWER_MAX} characters: {field} is longer"
                ));
            }
        }
        // The answers go back where they were: after the head, before
        // what the system gathered.
        let at = sections.iter().position(|s| s.id != draft::HEAD).unwrap_or(sections.len());
        let typed = draft::answer_sections(answers);
        sections.splice(at..at, typed);
    }
    if you.lines + you.parts > 0
        && let Some(head) = sections.iter_mut().find(|s| s.id == draft::HEAD)
    {
        head.lines.push(cut_line(you));
    }
    let (text, _) = redactor.redact(&draft::text_of(&sections));
    Ok(described(&sections, text, base.has_logs, you))
}

/// The head line that says what the sender cut: "Cut by the sender: 12
/// lines, 3 parts of lines".
fn cut_line(you: CutByYou) -> String {
    let lines = match you.lines {
        0 => None,
        1 => Some("1 line".to_string()),
        n => Some(format!("{n} lines")),
    };
    let parts = match you.parts {
        0 => None,
        1 => Some("1 part of a line".to_string()),
        n => Some(format!("{n} parts of lines")),
    };
    let said: Vec<String> = lines.into_iter().chain(parts).collect();
    format!("Cut by the sender: {}", said.join(", "))
}

/// The answer for `text`, made of `sections` (which redacting left where
/// they were): what `draft` answers, and where each section is.
fn described(sections: &[Section], text: String, has_logs: bool, you: CutByYou) -> Drafted {
    let sha256 = hex(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref());
    let cut_by = CutBy { you: you.lines + you.parts, ..redact::marks_in(&text) };
    let mut places = Vec::new();
    let mut parts: Vec<Size> = Vec::new();
    let mut answers_bytes = 0;
    let mut line = 0;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let bytes_of = |from: usize, count: usize| -> usize {
        lines.iter().skip(from).take(count).map(|line| line.len()).sum()
    };
    for section in sections.iter().filter(|s| s.id == draft::HEAD || !s.lines.is_empty()) {
        if section.id == draft::HEAD {
            let count = section.lines.len();
            places.push(Place {
                id: section.id.into(),
                title: HEAD_TITLE.into(),
                first_line: 0,
                lines: count,
            });
            // The head's blank line ends it.
            parts.push(Size { title: HEAD_TITLE.into(), bytes: bytes_of(0, count + 1) });
            line = count + 1;
            continue;
        }
        let title = draft::heading_of(&lines, line);
        let count = section.lines.len();
        places.push(Place {
            id: section.id.into(),
            title: title.clone(),
            first_line: line + 1,
            lines: count,
        });
        let bytes = bytes_of(line, count + 1);
        if draft::ANSWERS.contains(&section.id) {
            answers_bytes += bytes;
        } else {
            parts.push(Size { title, bytes });
        }
        line += count + 1;
    }
    if answers_bytes > 0 {
        parts
            .insert(1.min(parts.len()), Size { title: ANSWERS_TITLE.into(), bytes: answers_bytes });
    }
    let typed =
        places.iter().filter(|p| draft::ANSWERS.contains(&p.id.as_str())).cloned().collect();
    Drafted {
        cuts: cut_by.total(),
        text,
        has_logs,
        draft: None,
        sha256: Some(sha256),
        parts,
        cut_by,
        sections: places,
        typed,
        cut_by_you: Some(you),
    }
}

/// `line` with each character range of `ranges` replaced by `[cut]`.
fn cut_words(line: &str, ranges: &[(usize, usize)]) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    for &(from, to) in ranges {
        out.extend(&chars[at..from]);
        out.push_str(redact::CUT);
        at = to;
    }
    out.extend(&chars[at..]);
    out
}

/// The cuts of `cuts`, checked against `sections` and put in order by
/// section.
fn checked<'a>(
    sections: &'a [Section],
    cuts: &[Cut],
) -> Result<BTreeMap<&'a str, SectionCuts>, String> {
    let mut by: BTreeMap<&str, SectionCuts> = BTreeMap::new();
    for cut in cuts {
        if cut.section == draft::HEAD {
            return Err(
                "These lines say where the report came from and cannot be changed (RPT-104)".into(),
            );
        }
        let place = format!("{} line {}", cut.section, cut.line);
        let refused = |why: &str| format!("{why}: {place} (RPT-105)");
        if cut.text.is_some() {
            return Err(refused("A line the system gathered can be cut, not changed"));
        }
        let section = sections
            .iter()
            .find(|s| s.id == cut.section && GATHERED.contains(&s.id))
            .ok_or_else(|| refused("This report has no such part to cut"))?;
        let Some(line) = section.lines.get(cut.line) else {
            return Err(refused("This part has no such line"));
        };
        let cuts = by.entry(section.id).or_default();
        match (cut.through, cut.from, cut.to) {
            (None, None, None) => {
                cuts.lines.insert(cut.line);
            }
            (Some(through), None, None) if through >= cut.line && through < section.lines.len() => {
                cuts.lines.extend(cut.line..=through);
            }
            (Some(_), None, None) => return Err(refused("The run of lines ends past its part")),
            (None, Some(from), Some(to)) if from < to && to <= line.chars().count() => {
                cuts.words.entry(cut.line).or_default().push((from, to));
            }
            (None, Some(_), Some(_)) => return Err(refused("The words are not in that line")),
            _ => return Err(refused("A cut is of lines, or of words from one place to another")),
        }
    }
    for cuts in by.values_mut() {
        let lines = &cuts.lines;
        cuts.words.retain(|line, _| !lines.contains(line));
        for ranges in cuts.words.values_mut() {
            ranges.sort_unstable();
            let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
            for &(from, to) in ranges.iter() {
                match merged.last_mut() {
                    Some(last) if from <= last.1 => last.1 = last.1.max(to),
                    _ => merged.push((from, to)),
                }
            }
            *ranges = merged;
        }
    }
    Ok(by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::draft::Parts;
    use crate::reports::facts::Facts;
    use crate::reports::protocol::Answers;
    use crate::reports::redact::Lists;

    const NOW: u64 = 1_800_000_000;

    fn redactor() -> Redactor {
        Redactor::new(Lists { users: vec!["alice".into()], ..Lists::default() })
    }

    fn parts() -> Parts {
        Parts {
            about: "the service web (nginx 1.29.0)".into(),
            facts: Facts { version: "0.16.0".into(), channel: "stable".into(), ..Facts::default() },
            crash: Some(vec![
                "nginx stopped: SIGSEGV (address not mapped) at 0x0".into(),
                "in /users/alice/site from 10.1.2.3".into(),
                "Signature: 3fa2c4d19e0b7a61".into(),
            ]),
            log: Some((
                "Log of the service web (last 200 lines)".into(),
                vec![
                    "web: listening on 80".into(),
                    "web: alice signed in".into(),
                    "web: signal 11".into(),
                ],
            )),
            ..Parts::default()
        }
    }

    fn answers() -> Answers {
        Answers { happened: "The site stopped.".into(), ..Answers::default() }
    }

    fn base() -> Base {
        let parts = parts();
        let sections = draft::sections(&parts, &answers(), false);
        Base::redacted(&sections, draft::has_logs(&parts), &redactor())
    }

    fn edits(json: &str) -> Edits {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn no_edit_gives_the_drafted_text_back_byte_for_byte() {
        let drafted = redactor().redact(&draft::compose(&parts(), &answers(), false)).0;
        let answer = apply(&base(), &Edits::default(), &redactor()).unwrap();
        assert_eq!(answer.text, drafted);
        assert_eq!(answer.cut_by_you, Some(CutByYou::default()));
        assert_eq!((answer.cut_by.user, answer.cut_by.ipv4, answer.cut_by.you), (2, 1, 0));
        assert_eq!(answer.cuts, 3);
        assert!(answer.has_logs);
        let empty = apply(&base(), &edits(r#"{"cuts": []}"#), &redactor()).unwrap();
        assert_eq!(empty.sha256, answer.sha256);
    }

    #[test]
    fn the_sections_say_where_each_starts_so_a_line_is_told_by_its_part_and_number() {
        let answer = apply(&base(), &Edits::default(), &redactor()).unwrap();
        let lines: Vec<&str> = answer.text.lines().collect();
        let ids: Vec<&str> = answer.sections.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["head", "happened", "crash", "log"]);
        for place in &answer.sections {
            if place.id != "head" {
                assert_eq!(lines[place.first_line - 1], format!("== {}", place.title));
            }
        }
        let log = answer.sections.iter().find(|p| p.id == "log").unwrap();
        assert_eq!(lines[log.first_line + 2], "web: signal 11");
        assert_eq!(log.lines, 3);
        assert_eq!(answer.typed.len(), 1);
        assert_eq!(answer.typed[0].id, "happened");
        let titles: Vec<&str> = answer.parts.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(
            titles,
            [HEAD_TITLE, ANSWERS_TITLE, "Crash", "Log of the service web (last 200 lines)"]
        );
        assert_eq!(answer.parts.iter().map(|p| p.bytes).sum::<usize>(), answer.text.len());
    }

    #[test]
    fn a_line_a_run_of_lines_and_words_are_cut_and_counted_in_the_head() {
        let answer = apply(
            &base(),
            &edits(
                r#"{"cuts": [{"section": "log", "line": 0, "through": 1},
                             {"section": "crash", "line": 0, "from": 0, "to": 5},
                             {"section": "crash", "line": 0, "from": 3, "to": 13}]}"#,
            ),
            &redactor(),
        )
        .unwrap();
        assert!(answer.text.contains("Cut by the sender: 2 lines, 1 part of a line\n"));
        // "nginx stopped" is cut: the two runs of words overlap, and are one.
        assert!(answer.text.contains("== Crash\n[cut]: SIGSEGV"), "{}", answer.text);
        assert!(
            answer.text.contains("== Log of the service web (last 200 lines)\nweb: signal 11\n")
        );
        assert!(!answer.text.contains("listening"));
        assert_eq!(answer.cut_by_you, Some(CutByYou { lines: 2, parts: 1 }));
        assert_eq!(answer.cut_by.you, 3);
        // One user's name was in a line cut: it is no longer counted.
        assert_eq!(answer.cut_by.user, 1);
    }

    #[test]
    fn a_section_cut_whole_is_left_out_with_its_heading() {
        let answer = apply(
            &base(),
            &edits(r#"{"cuts": [{"section": "log", "line": 0, "through": 2}]}"#),
            &redactor(),
        )
        .unwrap();
        assert!(!answer.text.contains("== Log"));
        assert!(answer.sections.iter().all(|p| p.id != "log"));
        assert!(answer.text.contains("Cut by the sender: 3 lines\n"));
    }

    #[test]
    fn words_cut_inside_a_line_cut_whole_are_not_counted_twice() {
        let answer = apply(
            &base(),
            &edits(
                r#"{"cuts": [{"section": "log", "line": 2, "from": 0, "to": 3},
                             {"section": "log", "line": 2}]}"#,
            ),
            &redactor(),
        )
        .unwrap();
        assert_eq!(answer.cut_by_you, Some(CutByYou { lines: 1, parts: 0 }));
    }

    #[test]
    fn typed_answers_take_the_answers_place_and_are_redacted_and_a_heading_cannot_be_typed() {
        let answer = apply(
            &base(),
            &edits(
                r#"{"answers": {"happened": "From 10.1.2.3 it stopped.\n== Crash\nno",
                                "expected": "", "again": "Ask alice."}}"#,
            ),
            &redactor(),
        )
        .unwrap();
        assert!(
            answer.text.contains(
                "== What happened\nFrom [IPv4] it stopped.\n == Crash\nno\n\
                 == How to make it happen again\nAsk [user].\n== Crash\n"
            ),
            "{}",
            answer.text
        );
        let ids: Vec<&str> = answer.typed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["happened", "again"]);
        assert!(!answer.text.contains("Cut by the sender"));
    }

    #[test]
    fn the_head_a_typed_line_and_what_is_not_there_are_refused_with_their_ids() {
        let refused = |json: &str| apply(&base(), &edits(json), &redactor()).unwrap_err();
        assert!(refused(r#"{"cuts": [{"section": "head", "line": 0}]}"#).ends_with("(RPT-104)"));
        let typed = refused(r#"{"cuts": [{"section": "log", "line": 1, "text": "fine"}]}"#);
        assert_eq!(
            typed,
            "A line the system gathered can be cut, not changed: log line 1 (RPT-105)"
        );
        for json in [
            r#"{"cuts": [{"section": "log", "line": 3}]}"#,
            r#"{"cuts": [{"section": "happened", "line": 0}]}"#,
            r#"{"cuts": [{"section": "start-log", "line": 0}]}"#,
            r#"{"cuts": [{"section": "log", "line": 0, "through": 3}]}"#,
            r#"{"cuts": [{"section": "log", "line": 1, "through": 0}]}"#,
            r#"{"cuts": [{"section": "log", "line": 0, "from": 3, "to": 3}]}"#,
            r#"{"cuts": [{"section": "log", "line": 0, "from": 0, "to": 99}]}"#,
            r#"{"cuts": [{"section": "log", "line": 0, "from": 0}]}"#,
            r#"{"cuts": [{"section": "log", "line": 0, "through": 1, "from": 0, "to": 1}]}"#,
        ] {
            assert!(refused(json).ends_with("(RPT-105)"), "{json}");
        }
    }

    #[test]
    fn too_many_cuts_and_an_answer_too_long_are_refused() {
        let many: Vec<String> =
            (0..=CUTS_MAX).map(|_| r#"{"section":"log","line":0}"#.to_string()).collect();
        let json = format!(r#"{{"cuts": [{}]}}"#, many.join(","));
        assert!(apply(&base(), &edits(&json), &redactor()).unwrap_err().contains("at most 1000"));
        let long = "x".repeat(ANSWER_MAX + 1);
        let json = format!(r#"{{"answers": {{"happened": "{long}"}}}}"#);
        assert!(apply(&base(), &edits(&json), &redactor()).unwrap_err().contains("happened"));
    }

    #[test]
    fn the_same_edits_twice_give_the_same_hash() {
        let list = r#"{"cuts": [{"section": "crash", "line": 1}], "answers": {"happened": "x"}}"#;
        let first = apply(&base(), &edits(list), &redactor()).unwrap();
        let second = apply(&base(), &edits(list), &redactor()).unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.text, second.text);
    }

    #[test]
    fn a_draft_is_its_owner_s_until_its_hour_ends_and_its_hash_must_be_the_text_s() {
        let drafts = Drafts::default();
        let answer = drafts.keep(1000, base(), &redactor(), NOW).unwrap();
        let id = answer.draft.clone().unwrap();
        let sha = answer.sha256.clone().unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(drafts.text(1000, &id, &sha, NOW).unwrap(), answer.text);
        assert_eq!(drafts.text(1001, &id, &sha, NOW).unwrap_err(), ENDED);
        assert_eq!(drafts.text(1000, &id, &"0".repeat(64), NOW).unwrap_err(), NOT_READ);
        // An edit changes the hash, and the hour starts again.
        let later = NOW + 50 * 60;
        let cut = edits(r#"{"cuts": [{"section": "log", "line": 0}]}"#);
        let edited = drafts.edit(1000, &id, &sha, &cut, &redactor(), later).unwrap();
        assert_eq!(edited.draft.as_deref(), Some(id.as_str()));
        assert_eq!(drafts.text(1000, &id, &sha, later).unwrap_err(), NOT_READ);
        let new = edited.sha256.unwrap();
        assert_eq!(drafts.text(1000, &id, &new, later + 59 * 60).unwrap(), edited.text);
        assert_eq!(drafts.text(1000, &id, &new, later + 60 * 60).unwrap_err(), ENDED);
    }

    #[test]
    fn a_draft_taken_to_send_is_gone_unless_it_is_put_back() {
        let drafts = Drafts::default();
        let answer = drafts.keep(1000, base(), &redactor(), NOW).unwrap();
        let (id, sha) = (answer.draft.unwrap(), answer.sha256.unwrap());
        let taken = drafts.take(1000, &id, &sha, NOW).unwrap();
        assert_eq!(taken.text(), answer.text);
        assert_eq!(drafts.take(1000, &id, &sha, NOW).unwrap_err(), ENDED);
        drafts.put_back(taken);
        assert!(drafts.take(1000, &id, &sha, NOW).is_ok());
    }

    #[test]
    fn a_ninth_draft_of_a_person_and_a_sixty_fifth_in_all_push_out_the_oldest() {
        let drafts = Drafts::default();
        let ids: Vec<(String, String)> = (0..=EACH)
            .map(|_| {
                let answer = drafts.keep(1000, base(), &redactor(), NOW).unwrap();
                (answer.draft.unwrap(), answer.sha256.unwrap())
            })
            .collect();
        assert_eq!(drafts.count(), EACH);
        assert_eq!(drafts.text(1000, &ids[0].0, &ids[0].1, NOW).unwrap_err(), ENDED);
        assert!(drafts.text(1000, &ids[1].0, &ids[1].1, NOW).is_ok());
        for user in 0..ALL as u32 {
            drafts.keep(2000 + user, base(), &redactor(), NOW).unwrap();
        }
        assert_eq!(drafts.count(), ALL);
        assert_eq!(drafts.text(1000, &ids[EACH].0, &ids[EACH].1, NOW).unwrap_err(), ENDED);
    }
}
