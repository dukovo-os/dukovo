// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an authority's problems say, in plain words (ADR 0513).
//!
//! An ACME authority answers a request it refuses with a problem document
//! whose `type` names the kind of trouble. [`explain`] says what is wrong
//! and what to do about it for the kinds a person can act on, and keeps the
//! authority's own detail after it, since the detail is where the authority
//! says which name, which address and why.

use instant_acme::{Error, Problem};

use crate::util;

/// What every problem's `type` begins with, which is left out of the words.
const PREFIX: &str = "urn:ietf:params:acme:error:";

/// The words for `error`, which `names` were asked for, at time `now`
/// (seconds since 1970): `retry_after` is the authority's Retry-After, if
/// the last answer had one.
pub fn explain(error: &Error, names: &[String], retry_after: Option<&str>, now: i64) -> String {
    match error {
        Error::Api(problem) => problem_words(problem, names, retry_after, now),
        Error::Timeout(_) => {
            String::from("the authority did not finish in time: try again in a few minutes")
        }
        Error::Unsupported(what) => format!("the authority does not support {what}"),
        Error::Json(why) => format!("the authority's answer is not what ACME says: {why}"),
        // The words of what could not be reached, or what is too large.
        Error::Other(why) => why.to_string(),
        Error::Str(why) => format!("the authority's answer is incomplete: {why}"),
        other => format!("the request to the authority failed: {other}"),
    }
}

/// The kind of a problem, without the prefix: `connection`, `dns`, and so
/// on.
fn kind(problem: &Problem) -> &str {
    let text = problem.r#type.as_deref().unwrap_or_default();
    text.strip_prefix(PREFIX).unwrap_or(text)
}

/// A problem in plain words: one line for each of its subproblems, which
/// name the names, or one for the whole.
fn problem_words(
    problem: &Problem,
    names: &[String],
    retry_after: Option<&str>,
    now: i64,
) -> String {
    if problem.subproblems.is_empty() {
        let name = match names {
            [only] => only.as_str(),
            _ => "one of the names",
        };
        return one(kind(problem), name, problem.detail.as_deref(), retry_after, now);
    }
    let lines: Vec<String> = problem
        .subproblems
        .iter()
        .map(|sub| {
            let name = sub.identifier.as_ref().map_or_else(
                || String::from("one of the names"),
                |id| id.authorized(false).to_string(),
            );
            let kind = sub
                .r#type
                .as_deref()
                .map_or_else(|| kind(problem), |text| text.strip_prefix(PREFIX).unwrap_or(text));
            one(kind, &name, sub.detail.as_deref(), retry_after, now)
        })
        .collect();
    lines.join("; ")
}

/// The words for one problem of `kind` about `name`, with the authority's
/// `detail` after them.
fn one(
    kind: &str,
    name: &str,
    detail: Option<&str>,
    retry_after: Option<&str>,
    now: i64,
) -> String {
    let said = match kind {
        "connection" => format!(
            "the authority could not reach http://{name}/.well-known/acme-challenge/: check \
             that {name} leads to this machine and that port 80 reaches it from the internet \
             (a router may need to forward it)"
        ),
        "dns" => format!("the authority could not look {name} up"),
        "unauthorized" | "incorrectResponse" => format!(
            "the authority reached {name} but got another answer: another server answers \
             for {name}"
        ),
        "caa" => format!("{name}'s CAA record does not let this authority issue for it"),
        "rateLimited" => match retry_after.and_then(|after| retry_time(after, now)) {
            Some(time) => format!("the authority's rate limit was reached: try again after {time}"),
            None => String::from("the authority's rate limit was reached: try again later"),
        },
        "rejectedIdentifier" => format!("the authority does not issue for {name}"),
        "badNonce" => String::from(
            "the authority kept refusing the request's nonce, which it gives for each request: \
             try again",
        ),
        _ => {
            return match detail {
                Some(detail) => format!("the authority refused: {detail}"),
                None => format!(
                    "the authority refused ({})",
                    if kind.is_empty() { "no reason" } else { kind }
                ),
            };
        }
    };
    match detail {
        Some(detail) => format!("{said} (the authority said: {detail})"),
        None => said,
    }
}

/// When a Retry-After says to try again: a number of seconds from `now`,
/// or a date, which is shown as the authority wrote it.
fn retry_time(after: &str, now: i64) -> Option<String> {
    let after = after.trim();
    if after.is_empty() {
        return None;
    }
    match after.parse::<i64>() {
        Ok(seconds) => Some(util::local_time(now.saturating_add(seconds), "%F %T")),
        Err(_) => Some(after.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn problem(json: &str) -> Error {
        Error::Api(serde_json::from_str(json).unwrap())
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn a_connection_problem_says_what_to_check() {
        let error = problem(
            r#"{"type":"urn:ietf:params:acme:error:connection","detail":"Timeout during connect"}"#,
        );
        let text = explain(&error, &names(&["a.example"]), None, NOW);
        assert_eq!(
            text,
            "the authority could not reach http://a.example/.well-known/acme-challenge/: check \
             that a.example leads to this machine and that port 80 reaches it from the internet \
             (a router may need to forward it) (the authority said: Timeout during connect)"
        );
    }

    #[test]
    fn each_kind_a_person_can_act_on_has_its_words() {
        let one = names(&["a.example"]);
        let words = |kind: &str| {
            let json = format!(r#"{{"type":"urn:ietf:params:acme:error:{kind}"}}"#);
            explain(&problem(&json), &one, None, NOW)
        };
        assert_eq!(words("dns"), "the authority could not look a.example up");
        assert_eq!(
            words("unauthorized"),
            "the authority reached a.example but got another answer: another server answers \
             for a.example"
        );
        assert_eq!(words("incorrectResponse"), words("unauthorized"));
        assert_eq!(words("caa"), "a.example's CAA record does not let this authority issue for it");
        assert_eq!(words("rejectedIdentifier"), "the authority does not issue for a.example");
        assert!(words("badNonce").starts_with("the authority kept refusing the request's nonce"));
        assert_eq!(words("serverInternal"), "the authority refused (serverInternal)");
    }

    #[test]
    fn a_rate_limit_says_when_to_try_again() {
        let json = r#"{"type":"urn:ietf:params:acme:error:rateLimited","detail":"too many"}"#;
        let error = problem(json);
        let n = names(&["a.example"]);
        let seconds = explain(&error, &n, Some("3600"), NOW);
        assert!(
            seconds.starts_with("the authority's rate limit was reached: try again after 2027-01-"),
            "{seconds}"
        );
        assert!(seconds.ends_with("(the authority said: too many)"), "{seconds}");
        let date = explain(&error, &n, Some("Fri, 31 Dec 2027 23:59:59 GMT"), NOW);
        assert!(date.contains("try again after Fri, 31 Dec 2027 23:59:59 GMT"), "{date}");
        let none = explain(&error, &n, None, NOW);
        assert!(none.starts_with("the authority's rate limit was reached: try again later"));
        let empty = explain(&error, &n, Some("  "), NOW);
        assert!(empty.contains("try again later"));
    }

    #[test]
    fn a_problem_that_is_not_one_of_those_gives_the_authoritys_detail() {
        let error = problem(
            r#"{"type":"urn:ietf:params:acme:error:malformed","detail":"Invalid contact"}"#,
        );
        assert_eq!(
            explain(&error, &names(&["a.example"]), None, NOW),
            "the authority refused: Invalid contact"
        );
        let bare = problem(r"{}");
        assert_eq!(explain(&bare, &[], None, NOW), "the authority refused (no reason)");
    }

    #[test]
    fn subproblems_name_their_names_and_several_names_are_not_guessed() {
        let error = problem(
            r#"{"type":"urn:ietf:params:acme:error:compound","detail":"Error creating new order",
                "subproblems":[
                  {"type":"urn:ietf:params:acme:error:rejectedIdentifier",
                   "detail":"policy forbids it","identifier":{"type":"dns","value":"b.example"}},
                  {"type":"urn:ietf:params:acme:error:dns",
                   "identifier":{"type":"dns","value":"c.example"}}]}"#,
        );
        let text = explain(&error, &names(&["a.example", "b.example", "c.example"]), None, NOW);
        assert_eq!(
            text,
            "the authority does not issue for b.example (the authority said: policy forbids it); \
             the authority could not look c.example up"
        );
        let several = problem(r#"{"type":"urn:ietf:params:acme:error:dns"}"#);
        assert_eq!(
            explain(&several, &names(&["a.example", "b.example"]), None, NOW),
            "the authority could not look one of the names up"
        );
    }

    #[test]
    fn errors_that_are_not_problems_keep_their_own_words() {
        assert_eq!(
            explain(&Error::Timeout(None), &[], None, NOW),
            "the authority did not finish in time: try again in a few minutes"
        );
        assert_eq!(
            explain(&Error::Unsupported("ACME renewal information (ARI)"), &[], None, NOW),
            "the authority does not support ACME renewal information (ARI)"
        );
        let other = Error::Other("cannot reach the authority at a.example: refused".into());
        assert_eq!(
            explain(&other, &[], None, NOW),
            "cannot reach the authority at a.example: refused"
        );
        assert!(
            explain(&Error::Crypto, &[], None, NOW)
                .starts_with("the request to the authority failed")
        );
    }
}
