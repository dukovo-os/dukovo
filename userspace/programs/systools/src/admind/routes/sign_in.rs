// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The routes that sign a browser in (ADR 0514): a password, a second step
//! when the account has one, or a passkey alone, and then the session.
//!
//! authd decides every step: whether the password is right, whether the
//! account may use the console, whether a code or a passkey is. admind
//! holds the sign-in between the steps as a ticket that only the address
//! it was given to may use, and once authd says it is done, has authd start
//! the session's worker and keeps the session under a cookie. The name of
//! the session is the one authd checked: the name typed with a password it
//! accepted, or the name it gives for a passkey.

use std::time::Instant;

use accounts::console::{Console, Purpose, from_hex};
use accounts::request::{Answer, WRONG_PASSWORD};
use serde::Deserialize;
use serde_json::json;

use super::{Context, Outcome, Reply, Text, ask, refused, shown};
use crate::admind::authd;
use crate::admind::errors::{Attempt, Failure, Kind, refusal};
use crate::admind::guard::{self, SESSION_COOKIE, TRUST_COOKIE};
use crate::admind::link::{self, Link};
use crate::admind::log;
use crate::admind::sessions::Session;
use crate::admind::webauthn::{self, Credential};

/// How long "Trust this browser" lasts, in seconds.
const TRUSTED_SECONDS: u64 = 30 * 24 * 60 * 60;

/// What authd's answer to the password says is next.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The sign-in is done.
    Done(String),
    /// A second step, one of `kinds`, is wanted.
    Step(String, Vec<String>),
    /// The sign-in rules want a second step the account has not got.
    NeedsStep(String),
}

/// Whether `text` is hexadecimal, as authd writes a ticket.
fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// What authd's answer `text` to the password says.
fn next(text: &str) -> Option<Next> {
    let words: Vec<&str> = text.split(' ').collect();
    match words[..] {
        ["done", ticket] if is_hex(ticket) => Some(Next::Done(ticket.into())),
        ["needs-step", ticket] if is_hex(ticket) => Some(Next::NeedsStep(ticket.into())),
        ["step", ticket, kinds] if is_hex(ticket) => {
            Some(Next::Step(ticket.into(), kinds.split(',').map(str::to_string).collect()))
        }
        _ => None,
    }
}

/// The ticket and the account's name in authd's answer `text` to a passkey
/// alone.
fn alone(text: &str) -> Option<(String, String)> {
    match text.split(' ').collect::<Vec<_>>()[..] {
        ["done", ticket, name] if is_hex(ticket) => Some((ticket.into(), name.into())),
        _ => None,
    }
}

/// What authd's answer to `start` says: the session's ID, and the
/// browser's new trust token if it was trusted.
#[derive(Debug, PartialEq, Eq)]
struct Started {
    session: String,
    trust: Option<String>,
}

/// What authd's answer `text` to `start` says: `PROCESS SESSION [TOKEN]`.
fn started(text: &str) -> Option<Started> {
    let (session, trust) = match text.split(' ').collect::<Vec<_>>()[..] {
        [process, session] if process.bytes().all(|b| b.is_ascii_digit()) => (session, None),
        [process, session, token] if process.bytes().all(|b| b.is_ascii_digit()) => {
            (session, Some(token))
        }
        _ => return None,
    };
    let readable = is_hex(session) && trust.is_none_or(is_hex);
    readable.then(|| Started { session: session.into(), trust: trust.map(str::to_string) })
}

/// The person's words for the browser that a `User-Agent` names, such as
/// `Firefox on Linux`, for the list of the browsers they trust.
fn browser_name(user_agent: &str) -> String {
    let browsers = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ];
    // Android's and iOS's names come before the systems they are told as.
    let systems = [
        ("Android", "Android"),
        ("iPhone", "iOS"),
        ("iPad", "iOS"),
        ("Windows", "Windows"),
        ("Macintosh", "macOS"),
        ("Linux", "Linux"),
        ("X11", "Linux"),
    ];
    let find = |names: &[(&str, &'static str)]| {
        names.iter().find(|(mark, _)| user_agent.contains(mark)).map(|(_, name)| *name)
    };
    match (find(&browsers), find(&systems)) {
        (Some(browser), Some(system)) => format!("{browser} on {system}"),
        (Some(browser), None) => browser.to_string(),
        (None, Some(system)) => format!("a browser on {system}"),
        (None, None) => "a browser".to_string(),
    }
}

fn authd_unreadable() -> Failure {
    Failure::new(Kind::AuthdSilent, "authd gave an answer that does not read")
}

/// The account that `ticket` was given to this address for.
fn name_of(cx: &Context<'_>, ticket: &str) -> Result<String, Failure> {
    let tickets = cx.shared.tickets.borrow();
    let name = tickets.name_for(ticket, cx.peer, Instant::now());
    name.map(str::to_string).ok_or_else(|| {
        Failure::new(
            Kind::SessionEnded,
            "that sign-in is not one under way from this address, or began more than 5 minutes ago",
        )
    })
}

/// The bytes of `ticket`, which authd wrote in hexadecimal.
fn ticket_bytes(ticket: &str) -> Result<Vec<u8>, Failure> {
    from_hex(ticket.as_bytes()).ok_or_else(authd_unreadable)
}

/// `POST /api/1/sign-in`: the name and the password.
pub async fn password(cx: &Context<'_>) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        name: Text,
        password: Text,
    }
    let body: Body = cx.json()?;
    let what = format!("a sign-in of {}", shown(&body.name));
    if body.name.is_empty() || body.password.is_empty() {
        return Err(refused(cx, &what, Failure::new(Kind::WrongPassword, WRONG_PASSWORD)));
    }
    let console = Console::Begin {
        from: cx.peer.to_string(),
        name: body.name.to_string(),
        password: body.password.as_bytes().to_vec(),
    };
    let text = ask(cx, console, Attempt::Password, &what).await?;
    let name = body.name.to_string();
    match next(&text).ok_or_else(authd_unreadable)? {
        Next::Done(ticket) => start(cx, &ticket, &name, false, false).await,
        Next::NeedsStep(ticket) => start(cx, &ticket, &name, true, false).await,
        Next::Step(ticket, kinds) => second_step(cx, ticket, name, kinds).await,
    }
}

/// What to do once a password is right and a second step is wanted: a
/// browser that was trusted for 30 days is let in, and any other is asked
/// for the step.
async fn second_step(
    cx: &Context<'_>,
    ticket: String,
    name: String,
    kinds: Vec<String>,
) -> Outcome {
    let token = cx.trust.as_deref().and_then(|token| from_hex(token.as_bytes()));
    let mut forgotten = false;
    if let Some(token) = token {
        let console = Console::Trusted { ticket: ticket_bytes(&ticket)?, token };
        let (answer, _) = authd::ask(&cx.shared.authd, console, &[])
            .await
            .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
        match answer {
            Answer::Done(_) => return start(cx, &ticket, &name, false, false).await,
            Answer::Refused(_) => forgotten = true,
        }
    }
    cx.shared.tickets.borrow_mut().give(&ticket, &name, cx.peer, Instant::now());
    let reply = Reply::json(json!({ "step": { "ticket": ticket, "kinds": kinds } }));
    // A browser that is trusted no more asks for the step, and forgets
    // its cookie.
    Ok(if forgotten { reply.with_cookie(guard::clear_cookie(TRUST_COOKIE)) } else { reply })
}

/// `POST /api/1/sign-in/code`: an app's code or a recovery code.
pub async fn code(cx: &Context<'_>) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        ticket: Text,
        code: Text,
        #[serde(default)]
        trust: bool,
    }
    let body: Body = cx.json()?;
    let name = name_of(cx, &body.ticket)?;
    let console =
        Console::Code { ticket: ticket_bytes(&body.ticket)?, code: body.code.as_bytes().to_vec() };
    let what = format!("a second step of {}'s sign-in", shown(&name));
    ask(cx, console, Attempt::Code, &what).await?;
    start(cx, &body.ticket, &name, false, body.trust).await
}

/// `POST /api/1/sign-in/passkey/options`: what the browser asks a passkey
/// with, for the second step of a sign-in or for a sign-in alone.
pub async fn passkey_options(cx: &Context<'_>) -> Outcome {
    #[derive(Deserialize, Default)]
    struct Body {
        ticket: Option<Text>,
    }
    let body: Body = cx.optional_json()?;
    let (purpose, verification, what) = match &body.ticket {
        Some(ticket) => {
            let name = name_of(cx, ticket)?;
            let what = format!("a passkey challenge for {}'s sign-in", shown(&name));
            (Purpose::SignIn(ticket_bytes(ticket)?), "preferred", what)
        }
        None => {
            (Purpose::Alone(cx.peer.to_string()), "required", "a passkey challenge".to_string())
        }
    };
    let console = Console::Challenge { purpose, relying_party: cx.host.clone() };
    let text = ask(cx, console, Attempt::Passkey, &what).await?;
    Ok(Reply::json(webauthn::request_options(&text, &cx.host, verification)?))
}

/// `POST /api/1/sign-in/passkey`: a passkey's answer, as the second step
/// or the whole of the sign-in.
pub async fn passkey(cx: &Context<'_>) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        ticket: Option<Text>,
        #[serde(default)]
        trust: bool,
        credential: Credential,
    }
    let body: Body = cx.json()?;
    let assertion = webauthn::assertion(&body.credential, &cx.origin, &cx.host)?;
    let Some(ticket) = &body.ticket else {
        let console = Console::Passkey { purpose: Purpose::Alone(cx.peer.to_string()), assertion };
        let text = ask(cx, console, Attempt::Passkey, "a sign-in with a passkey").await?;
        let (ticket, name) = alone(&text).ok_or_else(authd_unreadable)?;
        return start(cx, &ticket, &name, false, body.trust).await;
    };
    let name = name_of(cx, ticket)?;
    let console = Console::Passkey { purpose: Purpose::SignIn(ticket_bytes(ticket)?), assertion };
    let what = format!("a passkey for {}'s sign-in", shown(&name));
    ask(cx, console, Attempt::Passkey, &what).await?;
    start(cx, ticket, &name, false, body.trust).await
}

/// Has authd start the session of the sign-in `ticket`, which is done, for
/// the account `name`, and keeps it under a cookie. With `needs_step` it
/// may only add a second step; with `trust` the browser is trusted for 30
/// days.
async fn start(
    cx: &Context<'_>,
    ticket: &str,
    name: &str,
    needs_step: bool,
    trust: bool,
) -> Outcome {
    let internal =
        |err: std::io::Error| Failure::new(Kind::Internal, format!("no link to a worker: {err}"));
    let (ours, theirs) = link::pipes().map_err(internal)?;
    let console = Console::Start {
        ticket: ticket_bytes(ticket)?,
        trust: trust && !needs_step,
        browser: browser_name(&cx.user_agent),
    };
    let [calls, replies] = theirs.fds();
    let fds = [calls, replies, libc::STDERR_FILENO];
    let (answer, _) = authd::ask(&cx.shared.authd, console, &fds)
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    drop(theirs);
    cx.shared.tickets.borrow_mut().forget(ticket);
    let text = match answer {
        Answer::Done(text) => text,
        Answer::Refused(why) => {
            let what = format!("to start {}'s session", shown(name));
            return Err(refused(cx, &what, refusal(Attempt::Other, &why)));
        }
    };
    let started = started(&text).ok_or_else(authd_unreadable)?;
    let link = Link::new(ours).map_err(internal)?;
    let session = Session::new(
        name.to_string(),
        started.session.clone(),
        (cx.peer, browser_name(&cx.user_agent)),
        needs_step,
        link.clone(),
    )
    .map_err(|why| Failure::new(Kind::Internal, why))?;
    let cookie = cx.shared.admit(session)?;
    cx.shared.watch(&link, started.session);
    log(6, &format!("{} signed in from {}", shown(name), cx.peer));
    let body = if needs_step { json!({ "needs_step": true }) } else { json!({ "done": true }) };
    let mut reply = Reply::json(body).with_cookie(guard::set_cookie(SESSION_COOKIE, &cookie, None));
    if let Some(token) = started.trust {
        reply = reply.with_cookie(guard::set_cookie(TRUST_COOKIE, &token, Some(TRUSTED_SECONDS)));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authds_answer_to_the_password_says_what_is_next() {
        assert_eq!(next("done 0a0b"), Some(Next::Done("0a0b".into())));
        assert_eq!(next("needs-step 0a0b"), Some(Next::NeedsStep("0a0b".into())));
        assert_eq!(
            next("step 0a0b app,passkey,recovery"),
            Some(Next::Step(
                "0a0b".into(),
                vec!["app".into(), "passkey".into(), "recovery".into()]
            ))
        );
        for wrong in ["", "done", "done zz", "done 0a 0b", "step 0a0b", "later 0a0b", "done  0a0b"]
        {
            assert_eq!(next(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn authds_answers_to_a_passkey_alone_and_to_start_are_read_whole_or_not_at_all() {
        assert_eq!(alone("done 0a0b chase"), Some(("0a0b".into(), "chase".into())));
        assert_eq!(alone("done 0a0b"), None);
        assert_eq!(alone("done xyz chase"), None);
        let session = "3f".repeat(16);
        assert_eq!(
            started(&format!("81 {session}")),
            Some(Started { session: session.clone(), trust: None })
        );
        assert_eq!(
            started(&format!("81 {session} ab12")),
            Some(Started { session: session.clone(), trust: Some("ab12".into()) })
        );
        for wrong in [
            "",
            "81",
            &format!("x {session}"),
            "81 zz",
            &format!("81 {session} zz"),
            &format!("81 {session} ab12 cd"),
        ] {
            assert_eq!(started(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn a_browser_is_named_as_a_person_would() {
        let cases = [
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
                "Firefox on Linux",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 Edg/126.0",
                "Edge on Windows",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
                "Safari on macOS",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36",
                "Chrome on Android",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
                "Safari on iOS",
            ),
            ("curl/8.0", "a browser"),
            ("", "a browser"),
            ("Lynx on Linux", "a browser on Linux"),
        ];
        for (agent, name) in cases {
            assert_eq!(browser_name(agent), name, "{agent}");
        }
    }
}
