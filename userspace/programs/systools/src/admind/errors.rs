// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's errors (ADR 0514), all in this one place. Each has an ID
//! a person can quote, a title, the HTTP status it is answered with, and
//! a fix the person can try. The detail is the sentence for this one
//! time: where authd refused, its own words, which are written for people
//! already. Both admind's routes and the worker's methods fail with these,
//! so the page sees one shape whichever of them said no:
//! `{"error": {"id", "title", "detail", "fix"}}`.

use accounts::request::WRONG_PASSWORD;
use serde_json::{Value, json};

/// A kind of error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// ADM-101: the name or the password is wrong.
    WrongPassword,
    /// ADM-102: a second-step code is wrong, used already or not a code.
    WrongCode,
    /// ADM-103: a passkey was not accepted.
    PasskeyRefused,
    /// ADM-104: the account may not use the console.
    NotAllowed,
    /// ADM-105: too many tries, or too much at once: wait.
    Wait,
    /// ADM-106: the session is locked, and the request needs it unlocked.
    Locked,
    /// ADM-107: there is no such session, or it has ended.
    SessionEnded,
    /// ADM-108: the request is not what the route takes.
    BadRequest,
    /// ADM-109: the request did not come from the console's own page.
    ForeignRequest,
    /// ADM-110: authd refused, for a reason of its own.
    Refused,
    /// ADM-111: authd did not answer.
    AuthdSilent,
    /// ADM-112: the session's worker did not answer.
    WorkerSilent,
    /// ADM-113: the request or the answer is too big.
    TooBig,
    /// ADM-114: the request names a host that is not this machine's.
    WrongHost,
    /// ADM-115: an API token came, and the console takes none yet.
    NoTokens,
    /// ADM-116: there is nothing at that address.
    NoRoute,
    /// ADM-117: the route does not take that method.
    NoMethod,
    /// ADM-118: the session may only add a second step.
    SecondStepOnly,
    /// ADM-119: the request has a body that is not JSON.
    NotJson,
    /// ADM-120: the console holds as many sessions as it may.
    Busy,
    /// ADM-121: the console could not do what it is for, for a reason of
    /// its own.
    Internal,
}

impl Kind {
    /// Every kind, in the order of their IDs.
    pub const ALL: [Kind; 21] = [
        Kind::WrongPassword,
        Kind::WrongCode,
        Kind::PasskeyRefused,
        Kind::NotAllowed,
        Kind::Wait,
        Kind::Locked,
        Kind::SessionEnded,
        Kind::BadRequest,
        Kind::ForeignRequest,
        Kind::Refused,
        Kind::AuthdSilent,
        Kind::WorkerSilent,
        Kind::TooBig,
        Kind::WrongHost,
        Kind::NoTokens,
        Kind::NoRoute,
        Kind::NoMethod,
        Kind::SecondStepOnly,
        Kind::NotJson,
        Kind::Busy,
        Kind::Internal,
    ];

    /// Its ID, `ADM-101` and on.
    pub fn id(self) -> &'static str {
        match self {
            Kind::WrongPassword => "ADM-101",
            Kind::WrongCode => "ADM-102",
            Kind::PasskeyRefused => "ADM-103",
            Kind::NotAllowed => "ADM-104",
            Kind::Wait => "ADM-105",
            Kind::Locked => "ADM-106",
            Kind::SessionEnded => "ADM-107",
            Kind::BadRequest => "ADM-108",
            Kind::ForeignRequest => "ADM-109",
            Kind::Refused => "ADM-110",
            Kind::AuthdSilent => "ADM-111",
            Kind::WorkerSilent => "ADM-112",
            Kind::TooBig => "ADM-113",
            Kind::WrongHost => "ADM-114",
            Kind::NoTokens => "ADM-115",
            Kind::NoRoute => "ADM-116",
            Kind::NoMethod => "ADM-117",
            Kind::SecondStepOnly => "ADM-118",
            Kind::NotJson => "ADM-119",
            Kind::Busy => "ADM-120",
            Kind::Internal => "ADM-121",
        }
    }

    /// The kind with ID `id`.
    pub fn of(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// The HTTP status it is answered with.
    pub fn status(self) -> u16 {
        match self {
            Kind::WrongPassword
            | Kind::WrongCode
            | Kind::PasskeyRefused
            | Kind::SessionEnded
            | Kind::NoTokens => 401,
            Kind::NotAllowed
            | Kind::Locked
            | Kind::ForeignRequest
            | Kind::Refused
            | Kind::SecondStepOnly => 403,
            Kind::Wait => 429,
            Kind::BadRequest => 400,
            Kind::AuthdSilent | Kind::WorkerSilent => 502,
            Kind::TooBig => 413,
            Kind::WrongHost => 421,
            Kind::NoRoute => 404,
            Kind::NoMethod => 405,
            Kind::NotJson => 415,
            Kind::Busy => 503,
            Kind::Internal => 500,
        }
    }

    /// Its title: what happened, in a few words.
    pub fn title(self) -> &'static str {
        match self {
            Kind::WrongPassword => "The name or the password is wrong",
            Kind::WrongCode => "That code is not right",
            Kind::PasskeyRefused => "The passkey was not accepted",
            Kind::NotAllowed => "This account may not use the console",
            Kind::Wait => "Too many tries: wait a moment",
            Kind::Locked => "The console is locked",
            Kind::SessionEnded => "This session has ended",
            Kind::BadRequest => "The request is not right",
            Kind::ForeignRequest => "The request did not come from the console's page",
            Kind::Refused => "That was refused",
            Kind::AuthdSilent => "authd did not answer",
            Kind::WorkerSilent => "The session's worker did not answer",
            Kind::TooBig => "That is too big",
            Kind::WrongHost => "This is not the name of this machine",
            Kind::NoTokens => "API tokens are not accepted yet",
            Kind::NoRoute => "There is nothing at that address",
            Kind::NoMethod => "That method is not used here",
            Kind::SecondStepOnly => "This session may only add a second step",
            Kind::NotJson => "The request is not JSON",
            Kind::Busy => "The console is full",
            Kind::Internal => "The console could not do that",
        }
    }

    /// What a person can try, where there is something.
    pub fn fix(self) -> Option<&'static str> {
        match self {
            Kind::WrongPassword => Some("Check the name and the password, then try again."),
            Kind::WrongCode => Some("Give the code the app shows now, or a recovery code."),
            Kind::PasskeyRefused => Some("Try the passkey again, or sign in another way."),
            Kind::NotAllowed => Some("Ask an administrator to let this account use the console."),
            Kind::Wait => Some("Wait a little, then try again."),
            Kind::Locked => Some("Unlock the console to make changes."),
            Kind::SessionEnded => Some("Sign in again."),
            Kind::ForeignRequest => Some("Reload the console's page and try again."),
            Kind::AuthdSilent => Some("Try again in a moment."),
            Kind::WorkerSilent => Some("Sign out and sign in again."),
            Kind::WrongHost => Some("Open the console by this machine's name or its address."),
            Kind::NoTokens => Some("Sign in with a name and a password."),
            Kind::SecondStepOnly => Some("Add a second step: you sign in again once it is added."),
            Kind::Busy => Some("Try again when someone has signed out."),
            Kind::Internal => Some("Try again; if it keeps happening, look in the log of admind."),
            Kind::NotJson
            | Kind::BadRequest
            | Kind::Refused
            | Kind::TooBig
            | Kind::NoRoute
            | Kind::NoMethod => None,
        }
    }
}

/// An error, as it is answered: its kind and what happened this time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What kind it is.
    pub kind: Kind,
    /// What happened, in a sentence.
    pub detail: String,
    /// For [`Kind::Wait`], the seconds after which to try again.
    pub retry_after: Option<u64>,
}

impl Failure {
    /// A failure of `kind`, with `detail`.
    pub fn new(kind: Kind, detail: impl Into<String>) -> Failure {
        Failure { kind, detail: detail.into(), retry_after: None }
    }

    /// A [`Kind::Wait`] failure that says to try again in `seconds`.
    pub fn wait(detail: impl Into<String>, seconds: u64) -> Failure {
        Failure { kind: Kind::Wait, detail: detail.into(), retry_after: Some(seconds) }
    }

    /// The answer's JSON.
    pub fn body(&self) -> Value {
        let mut error = json!({
            "id": self.kind.id(),
            "title": self.kind.title(),
            "detail": self.detail,
        });
        if let Some(fix) = self.kind.fix() {
            error["fix"] = fix.into();
        }
        json!({ "error": error })
    }
}

/// What a person was doing when authd refused them, which says what kind
/// of error its refusal is when its words do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    /// Giving a password.
    Password,
    /// Giving an app's code or a recovery code.
    Code,
    /// Giving a passkey.
    Passkey,
    /// Anything else.
    Other,
}

/// The sentences authd refuses with when an account may not use the
/// console: root's, a disabled account's, and one that is neither an
/// administrator nor in the `console` group.
const NOT_ALLOWED: [&str; 3] =
    ["does not sign in to the web console", "may not use the web console", "is disabled"];

/// The failure that authd's refusal `why` is, for what `attempt` was. authd
/// words every refusal for a person, so the detail is its own sentence; the
/// kind is what the page tells apart.
pub fn refusal(attempt: Attempt, why: &str) -> Failure {
    let kind = if why == WRONG_PASSWORD {
        Kind::WrongPassword
    } else if why.contains("may only add a second step") {
        Kind::SecondStepOnly
    } else if why.contains("is locked") {
        Kind::Locked
    } else if why.ends_with("sign in again") {
        Kind::SessionEnded
    } else if NOT_ALLOWED.iter().any(|phrase| why.contains(phrase)) {
        Kind::NotAllowed
    } else {
        match attempt {
            Attempt::Code if why.contains("code") || why.contains("digits") => Kind::WrongCode,
            Attempt::Passkey => Kind::PasskeyRefused,
            Attempt::Password | Attempt::Code | Attempt::Other => Kind::Refused,
        }
    };
    Failure::new(kind, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_run_from_101_without_a_gap_and_each_is_found_again() {
        for (at, kind) in Kind::ALL.into_iter().enumerate() {
            assert_eq!(kind.id(), format!("ADM-{}", 101 + at));
            assert_eq!(Kind::of(kind.id()), Some(kind));
        }
        assert_eq!(Kind::of("ADM-100"), None);
        assert_eq!(Kind::of("ADM-122"), None);
        assert_eq!(Kind::of(""), None);
    }

    #[test]
    fn the_first_seven_are_those_the_design_names() {
        let named = [
            (Kind::WrongPassword, "ADM-101", 401),
            (Kind::WrongCode, "ADM-102", 401),
            (Kind::PasskeyRefused, "ADM-103", 401),
            (Kind::NotAllowed, "ADM-104", 403),
            (Kind::Wait, "ADM-105", 429),
            (Kind::Locked, "ADM-106", 403),
            (Kind::SessionEnded, "ADM-107", 401),
        ];
        for (kind, id, status) in named {
            assert_eq!((kind.id(), kind.status()), (id, status));
        }
    }

    #[test]
    fn an_error_has_the_shape_the_page_reads() {
        let failure = Failure::new(Kind::Locked, "unlock it first");
        assert_eq!(
            failure.body(),
            json!({ "error": {
                "id": "ADM-106",
                "title": "The console is locked",
                "detail": "unlock it first",
                "fix": "Unlock the console to make changes.",
            }})
        );
        // A kind with nothing to try has no fix at all, not an empty one.
        let bad = Failure::new(Kind::BadRequest, "no JSON").body();
        assert!(bad["error"].get("fix").is_none());
        assert_eq!(Failure::wait("slow down", 7).retry_after, Some(7));
    }

    #[test]
    fn every_title_is_a_phrase_and_no_status_is_a_success() {
        for kind in Kind::ALL {
            assert!(kind.title().len() > 10, "{}", kind.id());
            assert!((400..600).contains(&kind.status()), "{}", kind.id());
        }
    }

    #[test]
    fn authds_refusals_are_told_apart_by_what_they_say_and_what_was_tried() {
        // The sentences are the ones in authd/console.rs.
        let cases = [
            (Attempt::Password, WRONG_PASSWORD, Kind::WrongPassword),
            (Attempt::Other, WRONG_PASSWORD, Kind::WrongPassword),
            (
                Attempt::Password,
                "bob may not use the web console: an administrator lets them with `elevate user set --console bob`",
                Kind::NotAllowed,
            ),
            (
                Attempt::Password,
                "root does not sign in to the web console: sign in as an administrator",
                Kind::NotAllowed,
            ),
            (Attempt::Passkey, "the account bob is disabled", Kind::NotAllowed),
            (Attempt::Code, "that code is not right", Kind::WrongCode),
            (Attempt::Code, "that code was used already: give the app's next one", Kind::WrongCode),
            (Attempt::Code, "unlocking takes the app's 6 digits", Kind::WrongCode),
            (Attempt::Code, "authd cannot write /system/state/authd", Kind::Refused),
            (
                Attempt::Passkey,
                "that passkey challenge is not one authd gave, or was used: try again",
                Kind::PasskeyRefused,
            ),
            (
                Attempt::Other,
                "that passkey challenge is not one authd gave, or was used: try again",
                Kind::Refused,
            ),
            (Attempt::Other, "the session is locked: unlock it to make changes", Kind::Locked),
            (Attempt::Other, "the session is locked: unlock it again", Kind::Locked),
            (
                Attempt::Code,
                "that web console session has ended: sign in again",
                Kind::SessionEnded,
            ),
            (
                Attempt::Code,
                "that sign-in is not one under way, or began more than 5 minutes ago: sign in again",
                Kind::SessionEnded,
            ),
            (
                Attempt::Other,
                "this session may only add a second step: sign in again once it is added",
                Kind::SecondStepOnly,
            ),
            (Attempt::Other, "64 web console sessions are signed in already", Kind::Refused),
        ];
        for (attempt, why, kind) in cases {
            let failure = refusal(attempt, why);
            assert_eq!((failure.kind, failure.detail.as_str()), (kind, why), "{why}");
        }
    }
}
