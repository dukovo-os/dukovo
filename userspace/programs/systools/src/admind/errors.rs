// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's errors (ADRs 0514, 0515, 0517, 0562 and 0690), all in this one place. Each
//! has an ID a person can quote, a title, the HTTP status it is answered
//! with, and a fix the person can try. The detail is the sentence for this
//! one time: where authd refused, its own words, which are written for
//! people already. Both admind's routes and the worker's methods fail with
//! these, so the page sees one shape whichever of them said no:
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
    /// ADM-115: an API token came that is not one: unknown, ended, or of
    /// an account that may not sign in.
    TokenRefused,
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
    /// ADM-122: the person may read what an area holds, and the change
    /// needs an administrator.
    NotAdmin,
    /// ADM-123: an action's command ran and did not work.
    CommandFailed,
    /// ADM-124: an API token's scopes do not name what it asked for.
    OutOfScope,
    /// ADM-125: an argument's value cannot work.
    BadValue,
    /// ADM-126: the console's store is not open, so it keeps no journal
    /// and no tasks.
    NoStore,
    /// ADM-127: the change or the task asked for is not one the console
    /// keeps.
    NotKept,
    /// ADM-128: what was asked cannot be done to this change or task as
    /// it is.
    Cannot,
    /// ADM-129: an action's program is not on this machine: it comes
    /// with a later update.
    NoProgram,
    /// ADM-130: the account, the terminal or the console holds as many
    /// terminals or panes as it may.
    TooManyTerminals,
    /// ADM-131: there is no such terminal or pane for this account.
    NoTerminal,
    /// ADM-132: the terminal is not open to this account.
    TerminalClosed,
    /// ADM-133: the connection a question came by was opened before the
    /// change it asks about.
    OldConnection,
    /// ADM-134: the console follows as many logs as it may.
    TooManyFollows,
    /// ADM-135: Spark has no trial by that name.
    NoTrial,
    /// ADM-136: the request would end the account the caller is signed in
    /// with, or the session it is using.
    NotSelf,
    /// ADM-137: the change would take away what the browser comes
    /// through.
    CutsOff,
    /// ADM-139: the console session or terminal named has ended already.
    SessionGone,
    /// ADM-140: the one-time link was read already, or its time ran out,
    /// or admind restarted since it was made.
    OnceGone,
    /// ADM-141: the certificate does not name the address the browser
    /// asks for the console by.
    NotNamed,
    /// ADM-142: a trial of settings runs, which a restart or a shut down
    /// would end half way.
    TrialRuns,
    /// ADM-143: an MCP client's token or pass was not accepted (ADR 0690).
    ClientRefused,
    /// ADM-144: an MCP client asked for what it may not reach.
    ClientMayNot,
    /// ADM-145: there is no such request of an MCP client's.
    NoRequest,
    /// ADM-146: as many requests of MCP clients wait as may.
    TooManyRequests,
}

impl Kind {
    /// Every kind, in the order of their IDs.
    pub const ALL: [Kind; 45] = [
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
        Kind::TokenRefused,
        Kind::NoRoute,
        Kind::NoMethod,
        Kind::SecondStepOnly,
        Kind::NotJson,
        Kind::Busy,
        Kind::Internal,
        Kind::NotAdmin,
        Kind::CommandFailed,
        Kind::OutOfScope,
        Kind::BadValue,
        Kind::NoStore,
        Kind::NotKept,
        Kind::Cannot,
        Kind::NoProgram,
        Kind::TooManyTerminals,
        Kind::NoTerminal,
        Kind::TerminalClosed,
        Kind::OldConnection,
        Kind::TooManyFollows,
        Kind::NoTrial,
        Kind::NotSelf,
        Kind::CutsOff,
        Kind::SessionGone,
        Kind::OnceGone,
        Kind::NotNamed,
        Kind::TrialRuns,
        Kind::ClientRefused,
        Kind::ClientMayNot,
        Kind::NoRequest,
        Kind::TooManyRequests,
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
            Kind::TokenRefused => "ADM-115",
            Kind::NoRoute => "ADM-116",
            Kind::NoMethod => "ADM-117",
            Kind::SecondStepOnly => "ADM-118",
            Kind::NotJson => "ADM-119",
            Kind::Busy => "ADM-120",
            Kind::Internal => "ADM-121",
            Kind::NotAdmin => "ADM-122",
            Kind::CommandFailed => "ADM-123",
            Kind::OutOfScope => "ADM-124",
            Kind::BadValue => "ADM-125",
            Kind::NoStore => "ADM-126",
            Kind::NotKept => "ADM-127",
            Kind::Cannot => "ADM-128",
            Kind::NoProgram => "ADM-129",
            Kind::TooManyTerminals => "ADM-130",
            Kind::NoTerminal => "ADM-131",
            Kind::TerminalClosed => "ADM-132",
            Kind::OldConnection => "ADM-133",
            Kind::TooManyFollows => "ADM-134",
            Kind::NoTrial => "ADM-135",
            Kind::NotSelf => "ADM-136",
            Kind::CutsOff => "ADM-137",
            Kind::SessionGone => "ADM-139",
            Kind::OnceGone => "ADM-140",
            Kind::NotNamed => "ADM-141",
            Kind::TrialRuns => "ADM-142",
            Kind::ClientRefused => "ADM-143",
            Kind::ClientMayNot => "ADM-144",
            Kind::NoRequest => "ADM-145",
            Kind::TooManyRequests => "ADM-146",
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
            | Kind::TokenRefused
            | Kind::ClientRefused => 401,
            Kind::NotAllowed
            | Kind::Locked
            | Kind::ForeignRequest
            | Kind::Refused
            | Kind::SecondStepOnly
            | Kind::NotAdmin
            | Kind::OutOfScope
            | Kind::TerminalClosed
            | Kind::ClientMayNot => 403,
            Kind::Wait | Kind::TooManyFollows | Kind::TooManyRequests => 429,
            Kind::BadRequest | Kind::BadValue => 400,
            // The command was asked for and ran: what it said is the
            // answer, and nothing about the request was wrong.
            Kind::CommandFailed => 422,
            // What was asked is not now what it can be done to, the
            // account holds as many terminals as it may until it ends one,
            // the connection is one that proves nothing about a change made
            // after it was opened, or the change would cut off the one who
            // asks.
            Kind::Cannot
            | Kind::TooManyTerminals
            | Kind::OldConnection
            | Kind::NotSelf
            | Kind::CutsOff
            | Kind::NotNamed
            | Kind::TrialRuns => 409,
            // The link was good and is not now: it is never good again. A
            // token that was never made is answered 404 by its route.
            Kind::OnceGone => 410,
            // The console has the action, and this machine not its program.
            Kind::NoProgram => 501,
            Kind::AuthdSilent | Kind::WorkerSilent => 502,
            Kind::TooBig => 413,
            Kind::WrongHost => 421,
            Kind::NoRoute
            | Kind::NotKept
            | Kind::NoTerminal
            | Kind::NoTrial
            | Kind::SessionGone
            | Kind::NoRequest => 404,
            Kind::NoMethod => 405,
            Kind::NotJson => 415,
            Kind::Busy | Kind::NoStore => 503,
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
            Kind::TokenRefused => "The API token is not accepted",
            Kind::NoRoute => "There is nothing at that address",
            Kind::NoMethod => "That method is not used here",
            Kind::SecondStepOnly => "This session may only add a second step",
            Kind::NotJson => "The request is not JSON",
            Kind::Busy => "The console is full",
            Kind::Internal => "The console could not do that",
            Kind::NotAdmin => "This account can look but not change",
            Kind::CommandFailed => "The command did not work",
            Kind::OutOfScope => "The API token may not do this",
            Kind::BadValue => "A value cannot work",
            Kind::NoStore => "The console keeps no journal or tasks now",
            Kind::NotKept => "The console has no such change or task",
            Kind::Cannot => "That cannot be done to this one",
            Kind::NoProgram => "This server does not have that command yet",
            Kind::TooManyTerminals => "Too many terminals",
            Kind::NoTerminal => "There is no such terminal",
            Kind::TerminalClosed => "The terminal is not open to this account",
            Kind::OldConnection => "This connection was opened before the change",
            Kind::TooManyFollows => "The console follows as many logs as it may",
            Kind::NoTrial => "Nothing is being tried by that name",
            Kind::NotSelf => "Not on your own account from the console",
            Kind::CutsOff => "This browser comes through what the change takes away",
            Kind::SessionGone => "That session has ended already",
            Kind::OnceGone => "The one-time link was used or has ended",
            Kind::NotNamed => "The certificate does not name the address this browser uses",
            Kind::TrialRuns => "A trial runs: keep or undo it before a restart",
            Kind::ClientRefused => "That AI client was not let in",
            Kind::ClientMayNot => "This AI client may not do that",
            Kind::NoRequest => "There is no such request",
            Kind::TooManyRequests => "Too many requests wait for a yes",
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
            Kind::TokenRefused => {
                Some("Check the token, or make a new one: a token ends on its date.")
            }
            Kind::NotAdmin => {
                Some("Ask an administrator to make the change, or to make this account one.")
            }
            Kind::OutOfScope => Some("Make a token whose scopes name this area."),
            Kind::BadValue => Some("Change the value, then try again."),
            Kind::SecondStepOnly => Some("Add a second step: you sign in again once it is added."),
            Kind::Busy => Some("Try again when someone has signed out."),
            Kind::Internal => Some("Try again; if it keeps happening, look in the log of admind."),
            Kind::NoStore => Some("Look in the log of admind for why its store did not open."),
            Kind::NoProgram => Some("It comes with a later update of Dukovo."),
            Kind::TooManyTerminals => Some("End a terminal you no longer use, then try again."),
            Kind::NoTerminal => {
                Some("Open the terminal from the Sessions page, or start a new one.")
            }
            Kind::OldConnection => Some("Ask again: the next request opens a new connection."),
            Kind::TooManyFollows => {
                Some("Wait a little, then follow again, or end a follow you no longer need.")
            }
            Kind::NotSelf => Some("Do this from another administrator's account."),
            Kind::CutsOff => Some("Make the change from a browser that does not come through it."),
            Kind::SessionGone => Some("Reload the list: it shows what is signed in now."),
            Kind::OnceGone => Some("Ask the person who sent it for a new link."),
            Kind::NotNamed => {
                Some("Open the console by a name the certificate has, then change it from there.")
            }
            Kind::TrialRuns => {
                Some("Keep or undo the change being tried, then restart or shut down.")
            }
            Kind::ClientRefused => {
                Some("Add the client again on the AI clients page: a client ends on its date.")
            }
            Kind::ClientMayNot => {
                Some("An administrator can add a client that reaches more on the AI clients page.")
            }
            Kind::NoRequest => Some("A request lapses after an hour: the client can ask again."),
            Kind::TooManyRequests => {
                Some("Approve or refuse the requests that wait on the AI clients page.")
            }
            Kind::NotJson
            | Kind::BadRequest
            | Kind::Refused
            | Kind::TooBig
            | Kind::NoRoute
            | Kind::NoMethod
            | Kind::NotKept
            | Kind::Cannot
            | Kind::CommandFailed
            | Kind::TerminalClosed
            | Kind::NoTrial => None,
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
    /// For [`Kind::BadValue`], the argument whose value cannot work, which
    /// the page shows the detail under.
    pub field: Option<&'static str>,
    /// For [`Kind::CommandFailed`], the ID the command gave, when its
    /// action names it: the answer's `id` instead of ADM-123.
    pub own_id: Option<String>,
    /// For [`Kind::CommandFailed`], the command: `{argv, status, stderr}`,
    /// its words, its status and its error output, for the page's "the
    /// program's own words".
    pub command: Option<Value>,
}

impl Failure {
    /// A failure of `kind`, with `detail`.
    pub fn new(kind: Kind, detail: impl Into<String>) -> Failure {
        Failure {
            kind,
            detail: detail.into(),
            retry_after: None,
            field: None,
            own_id: None,
            command: None,
        }
    }

    /// A [`Kind::CommandFailed`] failure: `argv` ended with `status`,
    /// saying `errors`; `detail` is its last line, and `id` the error ID
    /// that line gave, if the action names it.
    pub fn command(
        id: Option<&str>,
        detail: String,
        argv: &[String],
        status: i32,
        errors: &str,
    ) -> Failure {
        Failure {
            own_id: id.map(str::to_string),
            command: Some(json!({ "argv": argv, "status": status, "stderr": errors })),
            ..Failure::new(Kind::CommandFailed, detail)
        }
    }

    /// A [`Kind::BadValue`] failure of the argument `field`, saying `why`.
    pub fn bad_value(field: &'static str, why: impl Into<String>) -> Failure {
        Failure { field: Some(field), ..Failure::new(Kind::BadValue, why) }
    }

    /// A [`Kind::Wait`] failure that says to try again in `seconds`.
    pub fn wait(detail: impl Into<String>, seconds: u64) -> Failure {
        Failure { retry_after: Some(seconds), ..Failure::new(Kind::Wait, detail) }
    }

    /// The answer's JSON.
    pub fn body(&self) -> Value {
        let mut error = json!({
            "id": self.own_id.as_deref().unwrap_or(self.kind.id()),
            "title": self.kind.title(),
            "detail": self.detail,
        });
        if let Some(fix) = self.kind.fix() {
            error["fix"] = fix.into();
        }
        if let Some(field) = self.field {
            error["field"] = field.into();
        }
        if let Some(command) = &self.command {
            error["command"] = command.clone();
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

/// How authd ends its refusal of a session when it holds as many as it
/// may, which is the console being full as much as admind's own limit.
const FULL: &str = "web console sessions are signed in already";

/// How authd refuses an unlock, or a command as an administrator, to an
/// account that is no administrator: for a session's unlock and for an API
/// token whose owner is not one, as admind itself refuses a session's
/// change, ADM-122.
const NOT_ADMIN: &str = "is not an administrator, and makes no changes";

/// How a program that is in neither folder of commands is told, after its
/// name: authd refuses an elevated command with it (`elevate::plan`), and
/// the worker uses it for a command it was to run as the person, so that
/// ADM-129 says the same whichever of them looked.
pub const NOT_FOUND: &str = ": command not found";

/// The failure that authd's refusal `why` is, for what `attempt` was. authd
/// words every refusal for a person, so the detail is its own sentence; the
/// kind is what the page tells apart.
pub fn refusal(attempt: Attempt, why: &str) -> Failure {
    // A refusal authd gives an ID of the console's own, as the terminal
    // request's: `ADM-130: words`.
    if let Some((id, words)) = why.split_once(": ")
        && let Some(kind) = Kind::of(id)
    {
        return Failure::new(kind, words);
    }
    let kind = if why == WRONG_PASSWORD {
        Kind::WrongPassword
    } else if why.contains("may only add a second step") {
        Kind::SecondStepOnly
    } else if why.contains("is locked") {
        Kind::Locked
    } else if why.ends_with("sign in again") {
        Kind::SessionEnded
    } else if why.ends_with(NOT_ADMIN) {
        Kind::NotAdmin
    } else if why.ends_with(NOT_FOUND) {
        Kind::NoProgram
    } else if NOT_ALLOWED.iter().any(|phrase| why.contains(phrase)) {
        Kind::NotAllowed
    } else if why.ends_with(FULL) {
        Kind::Busy
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

    /// The ID that no kind has, since a command gives it: ADM-138, `admin
    /// reach` or `admin sign-in` while the `console` trial runs.
    const COMMAND_GIVEN: u16 = 138;

    #[test]
    fn ids_run_from_101_without_a_gap_but_the_one_a_command_gives_and_each_is_found_again() {
        let numbers = (101..).filter(|number| *number != COMMAND_GIVEN);
        for (number, kind) in numbers.zip(Kind::ALL) {
            assert_eq!(kind.id(), format!("ADM-{number}"));
            assert_eq!(Kind::of(kind.id()), Some(kind));
        }
        // Built, not written, so that `ob docs --check` finds no ID here
        // that has no page.
        let before = format!("ADM-{}", 101 - 1);
        let command = format!("ADM-{COMMAND_GIVEN}");
        let after = format!("ADM-{}", 101 + Kind::ALL.len() + 1);
        assert_eq!(Kind::of(&before), None);
        assert_eq!(Kind::of(&command), None);
        assert_eq!(Kind::of(&after), None);
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
    fn a_missing_program_and_the_terminals_have_their_own() {
        let named = [
            (Kind::NoProgram, "ADM-129", 501),
            (Kind::TooManyTerminals, "ADM-130", 409),
            (Kind::NoTerminal, "ADM-131", 404),
            (Kind::TerminalClosed, "ADM-132", 403),
        ];
        for (kind, id, status) in named {
            assert_eq!((kind.id(), kind.status()), (id, status));
        }
        // ADM-132's detail says which of its two reasons it is, and so
        // what to do; the others have a fix of their own.
        assert_eq!(Kind::TerminalClosed.fix(), None);
        assert!(Kind::NoProgram.fix().is_some_and(|fix| fix.contains("later update")));
    }

    #[test]
    fn the_trial_and_the_follow_have_their_own() {
        let named = [
            (Kind::OldConnection, "ADM-133", 409),
            (Kind::TooManyFollows, "ADM-134", 429),
            (Kind::NoTrial, "ADM-135", 404),
        ];
        for (kind, id, status) in named {
            assert_eq!((kind.id(), kind.status()), (id, status));
        }
        // A browser told that its connection is old asks again at once; a
        // follow refused for the limit waits, as the answer's retry says.
        assert!(Kind::OldConnection.fix().is_some_and(|fix| fix.contains("new connection")));
        assert!(Kind::TooManyFollows.fix().is_some_and(|fix| fix.contains("follow again")));
        assert_eq!(Kind::NoTrial.fix(), None);
        let refused = Failure {
            retry_after: Some(30),
            ..Failure::new(Kind::TooManyFollows, "eight logs are followed already")
        };
        assert_eq!(refused.body()["error"]["id"], "ADM-134");
        assert_eq!(refused.retry_after, Some(30));
    }

    #[test]
    fn acting_on_oneself_cutting_off_and_a_session_gone_have_their_own() {
        let named = [
            (Kind::NotSelf, "ADM-136", 409),
            (Kind::CutsOff, "ADM-137", 409),
            (Kind::SessionGone, "ADM-139", 404),
        ];
        for (kind, id, status) in named {
            assert_eq!((kind.id(), kind.status()), (id, status));
        }
        // The two that refuse a change say what to do instead; a session
        // that is gone needs nothing but a fresh list.
        assert!(Kind::NotSelf.fix().is_some_and(|fix| fix.contains("another administrator")));
        assert!(Kind::CutsOff.fix().is_some_and(|fix| fix.contains("does not come through")));
        assert!(Kind::SessionGone.fix().is_some_and(|fix| fix.contains("Reload")));
    }

    #[test]
    fn a_used_link_a_certificate_s_names_and_a_trial_before_a_restart_have_their_own() {
        let named = [
            (Kind::OnceGone, "ADM-140", 410),
            (Kind::NotNamed, "ADM-141", 409),
            (Kind::TrialRuns, "ADM-142", 409),
        ];
        for (kind, id, status) in named {
            assert_eq!((kind.id(), kind.status()), (id, status));
            assert!(kind.fix().is_some(), "{id} says what to do");
        }
        assert!(Kind::TrialRuns.fix().is_some_and(|fix| fix.contains("Keep or undo")));
    }

    #[test]
    fn an_ai_client_s_refusals_have_their_own() {
        let named = [
            (Kind::ClientRefused, "ADM-143", 401),
            (Kind::ClientMayNot, "ADM-144", 403),
            (Kind::NoRequest, "ADM-145", 404),
            (Kind::TooManyRequests, "ADM-146", 429),
        ];
        for (kind, id, status) in named {
            assert_eq!((kind.id(), kind.status()), (id, status));
            assert!(kind.fix().is_some(), "{id} says what to do");
        }
        // authd words its own refusal of a client with the console's ID.
        let refused = refusal(Attempt::Other, "ADM-144: an AI client changes nothing by itself");
        assert_eq!(refused.kind, Kind::ClientMayNot);
        assert_eq!(refused.detail, "an AI client changes nothing by itself");
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
        // A command that failed says its words, status and error output as
        // the page reads them.
        let argv = ["service".to_string(), "nginx".into(), "restart".into()];
        let failed = Failure::command(None, "nginx: no".into(), &argv, 1, "nginx: no\n").body();
        assert_eq!(
            failed["error"]["command"],
            json!({ "argv": ["service", "nginx", "restart"], "status": 1, "stderr": "nginx: no\n" })
        );
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
            (Attempt::Other, "64 web console sessions are signed in already", Kind::Busy),
            (Attempt::Other, "chase is not an administrator, and makes no changes", Kind::NotAdmin),
            (Attempt::Other, "dukovo-check: command not found", Kind::NoProgram),
        ];
        for (attempt, why, kind) in cases {
            let failure = refusal(attempt, why);
            assert_eq!((failure.kind, failure.detail.as_str()), (kind, why), "{why}");
        }
    }

    #[test]
    fn the_words_told_apart_are_still_authds() {
        // authd words its refusals in authd/console.rs; a sentence changed
        // there would turn its kind into a plain refusal here, quietly.
        let authd = include_str!("../authd/console.rs");
        for phrase in NOT_ALLOWED.into_iter().chain([
            FULL,
            NOT_ADMIN,
            "may only add a second step",
            "the session is locked",
            ": sign in again",
        ]) {
            assert!(authd.contains(phrase), "authd no longer says {phrase:?}");
        }
    }

    #[test]
    fn a_program_authd_cannot_find_is_adm_129_with_authds_words_naming_it() {
        // The refusal is the one elevate::plan gives for a command that is in
        // neither folder, which authd passes on as its refusal.
        let root = crate::elevate::root_account(std::iter::empty());
        let command = [b"dukovo-check".to_vec(), b"--last".to_vec()];
        let why = crate::elevate::plan(accounts::request::Mode::Command, &command, b"/", &root)
            .unwrap_err();
        let failure = refusal(Attempt::Other, &why);
        assert_eq!(failure.kind, Kind::NoProgram);
        assert_eq!(failure.detail, "dukovo-check: command not found");
        assert_eq!(failure.body()["error"]["id"], "ADM-129");
        // Any other refusal of a command is still authd's own.
        for other in [
            "the web console runs the system's and the apps' commands alone, not /bin/ls",
            "the web console runs commands, never a shell",
            "command not found",
        ] {
            assert_eq!(refusal(Attempt::Other, other).kind, Kind::Refused, "{other}");
        }
    }
}
