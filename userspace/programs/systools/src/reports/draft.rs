// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The text of a problem report (ADR 0531): plain text, and exactly the
//! bytes that leave the machine if the person sends it. reportd(8) makes
//! it from the parts it gathers as root and the person's answers, and
//! redacts it ([`super::redact`]); report(1) shows it as it is.
//!
//! ```text
//! Dukovo problem report
//! Version: 0.16.0 (stable, server)
//! About: the service web (nginx 1.29.0)
//! Hardware: Dell Inc. OptiPlex 7040, Intel Core i7-6700, 16 GiB
//! Devices: 8086:15b8, 8086:a102
//! Language: en_US.UTF-8
//!
//! == What happened
//! ...
//! == Crash
//! ...
//! ```
//!
//! A section with nothing in it is left out, and so is an answer the
//! person left empty. When a section is missing that should be there,
//! look at the [`Parts`] reportd made: it leaves a part out when it may
//! not read it for the person who asks, or cannot read it at all.

use super::facts::Facts;
use super::protocol::Answers;

/// What reportd gathers for a report, before the person's answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    /// What the report is about, as the `About:` line says it: `the
    /// service web (nginx 1.29.0)`.
    pub about: String,
    /// The machine and the system.
    pub facts: Facts,
    /// The lines of a crash: what stopped, its signature, its frames.
    pub crash: Option<Vec<String>>,
    /// A log: the heading of its section, `Log of the service web (last
    /// 200 lines)`, and its lines.
    pub log: Option<(String, Vec<String>)>,
    /// The notice of a failed start (ADR 0501) and what `changes` lists.
    pub notice: Option<Vec<String>>,
    /// The warnings and errors in the log of the failed start.
    pub start_log: Option<Vec<String>>,
}

impl Parts {
    /// The log sections, which `without_logs` leaves out: the heading of
    /// each and its lines.
    fn logs(&self) -> Vec<(&str, &[String])> {
        let log = self.log.as_ref().map(|(heading, lines)| (heading.as_str(), lines.as_slice()));
        let start = self.start_log.as_deref().map(|lines| (START_LOG_HEADING, lines));
        log.into_iter().chain(start).filter(|(_, lines)| !lines.is_empty()).collect()
    }
}

/// The heading of the failed start's log lines.
const START_LOG_HEADING: &str = "Warnings and errors of the failed start";

/// The IDs of a report's sections, as a kept draft's answer names them
/// (ADR 0568): the head, the three answers, and what the system gathered.
pub const HEAD: &str = "head";
pub const HAPPENED: &str = "happened";
pub const EXPECTED: &str = "expected";
pub const AGAIN: &str = "again";
pub const CRASH: &str = "crash";
pub const FAILED_START: &str = "failed-start";
pub const LOG: &str = "log";
pub const START_LOG: &str = "start-log";

/// The sections the person typed.
pub const ANSWERS: [&str; 3] = [HAPPENED, EXPECTED, AGAIN];

/// A section of a report's text: its ID, its heading (the head has none),
/// and its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub id: &'static str,
    pub heading: Option<String>,
    pub lines: Vec<String>,
}

impl Section {
    fn of<T: Into<String>>(
        id: &'static str,
        heading: &str,
        lines: impl Iterator<Item = T>,
    ) -> Section {
        Section { id, heading: Some(heading.to_string()), lines: lines.map(Into::into).collect() }
    }
}

/// The text of the report for `parts` and the person's `answers`; without
/// the logs when `without_logs` is set.
pub fn compose(parts: &Parts, answers: &Answers, without_logs: bool) -> String {
    text_of(&sections(parts, answers, without_logs))
}

/// The sections of the report for `parts` and `answers`, the head first,
/// then each that has lines.
pub fn sections(parts: &Parts, answers: &Answers, without_logs: bool) -> Vec<Section> {
    let mut head = vec!["Dukovo problem report".to_string()];
    let mut facts = parts.facts.lines();
    head.push(facts.remove(0));
    head.push(format!("About: {}", parts.about));
    head.extend(facts);
    let mut sections = vec![Section { id: HEAD, heading: None, lines: head }];
    sections.extend(answer_sections(answers));
    if let Some(crash) = &parts.crash {
        sections.push(Section::of(CRASH, "Crash", crash.iter().map(String::as_str)));
    }
    if let Some(notice) = &parts.notice {
        sections.push(Section::of(FAILED_START, "Failed start", notice.iter().map(String::as_str)));
    }
    if !without_logs {
        if let Some((heading, lines)) = &parts.log {
            sections.push(Section::of(LOG, heading, lines.iter().map(String::as_str)));
        }
        if let Some(lines) = &parts.start_log {
            sections.push(Section::of(
                START_LOG,
                START_LOG_HEADING,
                lines.iter().map(String::as_str),
            ));
        }
    }
    sections.retain(|section| section.id == HEAD || !section.lines.is_empty());
    sections
}

/// The sections of the person's answers, each that is not empty.
pub fn answer_sections(answers: &Answers) -> Vec<Section> {
    [
        (HAPPENED, "What happened", &answers.happened),
        (EXPECTED, "What was expected", &answers.expected),
        (AGAIN, "How to make it happen again", &answers.again),
    ]
    .into_iter()
    .map(|(id, heading, answer)| Section::of(id, heading, answer_lines(answer).into_iter()))
    .filter(|section| !section.lines.is_empty())
    .collect()
}

/// The text of `sections`: the head's lines and a blank line, then each
/// other section that has lines, as `== heading` and its lines.
pub fn text_of(sections: &[Section]) -> String {
    let mut text = String::new();
    for section in sections {
        match &section.heading {
            None => {
                for line in &section.lines {
                    text.push_str(line);
                    text.push('\n');
                }
                text.push('\n');
            }
            Some(heading) if !section.lines.is_empty() => {
                text.push_str("== ");
                text.push_str(heading);
                text.push('\n');
                for line in &section.lines {
                    text.push_str(line);
                    text.push('\n');
                }
            }
            Some(_) => {}
        }
    }
    text
}

/// `sections` again, with the headings and lines of `text`, which is their
/// text redacted: redaction leaves each line where it is.
pub fn split_like(sections: &[Section], text: &str) -> Vec<Section> {
    let mut lines = text.split('\n');
    let mut out = Vec::with_capacity(sections.len());
    for section in sections {
        let mut next = || lines.next().unwrap_or_default().to_string();
        match &section.heading {
            None => {
                let head = section.lines.iter().map(|_| next()).collect();
                next();
                out.push(Section { lines: head, ..section.clone() });
            }
            Some(_) if !section.lines.is_empty() => {
                let heading = next();
                let heading = heading.strip_prefix("== ").unwrap_or(&heading).to_string();
                let body = section.lines.iter().map(|_| next()).collect();
                out.push(Section { id: section.id, heading: Some(heading), lines: body });
            }
            Some(_) => out.push(section.clone()),
        }
    }
    out
}

/// The heading on line `at` of `lines` (each with its end of line), without
/// its `== `.
pub fn heading_of(lines: &[&str], at: usize) -> String {
    let line = lines.get(at).copied().unwrap_or_default().trim_end_matches('\n');
    line.strip_prefix("== ").unwrap_or(line).to_string()
}

/// The lines of an answer: as typed, without trailing spaces and without
/// blank lines before and after it. A line that starts `==` gets a space
/// before it, so that nothing typed passes for a heading of the report.
fn answer_lines(answer: &str) -> Vec<String> {
    let mut lines: Vec<&str> = answer.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let blank = lines.iter().take_while(|line| line.is_empty()).count();
    lines
        .split_off(blank)
        .into_iter()
        .map(|line| if line.starts_with("==") { format!(" {line}") } else { line.to_string() })
        .collect()
}

/// Whether the report for `parts` has a log, which `without_logs` leaves
/// out.
pub fn has_logs(parts: &Parts) -> bool {
    !parts.logs().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            version: "0.16.0".into(),
            channel: "stable".into(),
            edition: Some("server".into()),
            maker: Some("Dell Inc.".into()),
            model: Some("OptiPlex 7040".into()),
            processor: Some("Intel Core i7-6700".into()),
            memory_kib: Some(16 * 1024 * 1024),
            pci: vec!["8086:15b8".into(), "8086:a102".into()],
            usb: Vec::new(),
            language: Some("en_US.UTF-8".into()),
        }
    }

    fn answers() -> Answers {
        Answers {
            happened: "The web server stopped\nright after start.\n".into(),
            expected: "It serves pages.".into(),
            again: "Open the front page twice.".into(),
        }
    }

    fn crashed_service() -> Parts {
        Parts {
            about: "the service web (nginx 1.29.0)".into(),
            facts: facts(),
            crash: Some(vec![
                "nginx stopped: SIGSEGV (address not mapped) at 0x0".into(),
                "Signature: 3fa2c4d19e0b7a61".into(),
                " 0 ngx_http_core_run_phases+0x4c  nginx 1.29.0 +0x5a2c1c  build 7be951e4".into(),
            ]),
            log: Some((
                "Log of the service web (last 200 lines)".into(),
                vec!["web: listening on 80".into(), "web: signal 11".into()],
            )),
            notice: None,
            start_log: None,
        }
    }

    #[test]
    fn a_crash_of_a_service_has_the_layout_of_the_design() {
        let text = compose(&crashed_service(), &answers(), false);
        assert_eq!(
            text,
            "Dukovo problem report\n\
             Version: 0.16.0 (stable, server)\n\
             About: the service web (nginx 1.29.0)\n\
             Hardware: Dell Inc. OptiPlex 7040, Intel Core i7-6700, 16 GiB\n\
             Devices: 8086:15b8, 8086:a102\n\
             Language: en_US.UTF-8\n\
             \n\
             == What happened\n\
             The web server stopped\n\
             right after start.\n\
             == What was expected\n\
             It serves pages.\n\
             == How to make it happen again\n\
             Open the front page twice.\n\
             == Crash\n\
             nginx stopped: SIGSEGV (address not mapped) at 0x0\n\
             Signature: 3fa2c4d19e0b7a61\n \
             0 ngx_http_core_run_phases+0x4c  nginx 1.29.0 +0x5a2c1c  build 7be951e4\n\
             == Log of the service web (last 200 lines)\n\
             web: listening on 80\n\
             web: signal 11\n"
        );
    }

    #[test]
    fn without_logs_leaves_the_log_out_and_nothing_else() {
        let parts = crashed_service();
        let with = compose(&parts, &answers(), false);
        let without = compose(&parts, &answers(), true);
        assert!(!without.contains("== Log of"));
        assert!(!without.contains("web: signal 11"));
        assert_eq!(with.split("== Log of").next().unwrap(), without);
    }

    #[test]
    fn an_empty_answer_and_an_empty_section_are_left_out() {
        let parts = Parts {
            about: "the hardware".into(),
            facts: Facts { version: "0.16.0".into(), channel: "stable".into(), ..Facts::default() },
            crash: Some(Vec::new()),
            log: Some(("Log".into(), Vec::new())),
            ..Parts::default()
        };
        let answers = Answers {
            happened: "No sound.\n\n".into(),
            expected: "  \n".into(),
            ..Answers::default()
        };
        assert_eq!(
            compose(&parts, &answers, false),
            "Dukovo problem report\n\
             Version: 0.16.0 (stable)\n\
             About: the hardware\n\
             \n\
             == What happened\n\
             No sound.\n"
        );
        assert!(!has_logs(&parts));
    }

    #[test]
    fn a_failed_start_has_its_notice_and_its_log_lines() {
        let parts = Parts {
            about: "the failed start 3".into(),
            facts: Facts { version: "0.16.0".into(), channel: "stable".into(), ..Facts::default() },
            notice: Some(vec![
                "Start 3 failed its health check: web did not start".into(),
                "Dukovo went back to 0.15.0.".into(),
            ]),
            start_log: Some(vec!["web: cannot bind port 80".into()]),
            ..Parts::default()
        };
        let text = compose(&parts, &Answers::default(), false);
        assert!(text.ends_with(
            "\n== Failed start\n\
             Start 3 failed its health check: web did not start\n\
             Dukovo went back to 0.15.0.\n\
             == Warnings and errors of the failed start\n\
             web: cannot bind port 80\n"
        ));
        assert!(!text.contains("== What happened"));
        assert!(has_logs(&parts));
        let without = compose(&parts, &Answers::default(), true);
        assert!(without.ends_with("Dukovo went back to 0.15.0.\n"));
    }

    #[test]
    fn a_report_has_logs_when_either_log_has_lines() {
        let mut parts = crashed_service();
        assert!(has_logs(&parts));
        parts.log = None;
        assert!(!has_logs(&parts));
        parts.start_log = Some(vec!["x".into()]);
        assert!(has_logs(&parts));
    }

    #[test]
    fn answers_keep_their_lines_but_lose_blank_ends_and_trailing_spaces() {
        let answers =
            Answers { happened: "\n\n  first  \n\nsecond \n\n".into(), ..Answers::default() };
        let text = compose(&Parts::default(), &answers, false);
        assert!(text.ends_with("== What happened\n  first\n\nsecond\n"), "{text}");
    }

    #[test]
    fn a_typed_line_that_looks_like_a_heading_gets_a_space_before_it() {
        let answers = Answers { happened: "== Crash\nfine\n==x".into(), ..Answers::default() };
        let text = compose(&Parts::default(), &answers, false);
        assert!(text.ends_with("== What happened\n == Crash\nfine\n ==x\n"), "{text}");
    }

    #[test]
    fn the_sections_make_the_text_and_a_redacted_text_splits_back_into_them() {
        let parts = crashed_service();
        let sections = sections(&parts, &answers(), false);
        let ids: Vec<&str> = sections.iter().map(|s| s.id).collect();
        assert_eq!(ids, [HEAD, HAPPENED, EXPECTED, AGAIN, CRASH, LOG]);
        let text = text_of(&sections);
        assert_eq!(text, compose(&parts, &answers(), false));
        let changed = text.replace("web", "[user]");
        let split = split_like(&sections, &changed);
        assert_eq!(text_of(&split), changed);
        assert_eq!(split[5].heading.as_deref(), Some("Log of the service [user] (last 200 lines)"));
    }
}
