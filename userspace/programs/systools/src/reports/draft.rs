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
        let start = self.start_log.as_deref().map(|lines| (START_LOG, lines));
        log.into_iter().chain(start).filter(|(_, lines)| !lines.is_empty()).collect()
    }
}

/// The heading of the failed start's log lines.
const START_LOG: &str = "Warnings and errors of the failed start";

/// The text of the report for `parts` and the person's `answers`; without
/// the logs when `without_logs` is set.
pub fn compose(parts: &Parts, answers: &Answers, without_logs: bool) -> String {
    let mut lines = vec!["Dukovo problem report".to_string()];
    let mut facts = parts.facts.lines();
    lines.push(facts.remove(0));
    lines.push(format!("About: {}", parts.about));
    lines.extend(facts);
    lines.push(String::new());
    let mut text = lines.join("\n") + "\n";
    for (heading, body) in [
        ("What happened", &answers.happened),
        ("What was expected", &answers.expected),
        ("How to make it happen again", &answers.again),
    ] {
        section(&mut text, heading, answer_lines(body).into_iter());
    }
    if let Some(crash) = &parts.crash {
        section(&mut text, "Crash", crash.iter().map(String::as_str));
    }
    if let Some(notice) = &parts.notice {
        section(&mut text, "Failed start", notice.iter().map(String::as_str));
    }
    if !without_logs {
        for (heading, lines) in parts.logs() {
            section(&mut text, heading, lines.iter().map(String::as_str));
        }
    }
    text
}

/// The lines of an answer: as typed, without trailing spaces and without
/// blank lines before and after it.
fn answer_lines(answer: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = answer.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let blank = lines.iter().take_while(|line| line.is_empty()).count();
    lines.split_off(blank)
}

/// Adds `== heading` and `lines` to `text`, unless there are no lines.
fn section<'a>(text: &mut String, heading: &str, lines: impl Iterator<Item = &'a str>) {
    let mut lines = lines.peekable();
    if lines.peek().is_none() {
        return;
    }
    text.push_str("== ");
    text.push_str(heading);
    text.push('\n');
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
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
}
