// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The routes of a signed-in session (ADR 0514): who it is and whether it
//! is unlocked, signing out, and unlocking, which is the person proving
//! again that it is them so that the session may make changes as an
//! administrator for 15 minutes.
//!
//! Whether the person is an administrator and what their full name is come
//! from the account files, which admind reads itself; the ways they can
//! unlock come from their worker, which is only a hint for the page, since
//! authd checks whatever proof is given. What a session is unlocked for is
//! authd's word, kept as it answered.

use std::time::Instant;

use accounts::console::{Console, Proof, Purpose};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Context, Live, Outcome, Reply, Text, ask, shown};
use crate::admind::errors::{Attempt, Failure, Kind};
use crate::admind::guard::{self, SESSION_COOKIE};
use crate::admind::reach;
use crate::admind::shared::{seconds_left, unix_now};
use crate::admind::terminal::protocol::Why;
use crate::admind::trials;
use crate::admind::webauthn::{self, Credential};
use crate::admind::who::{self, Account};

/// The ways to unlock that an account has, which `steps` says, for a page
/// opened as `host`: the password always, the app when there is one, and a
/// passkey when one was made for `host`.
fn kinds_of(steps: &Value, host: &str) -> Vec<&'static str> {
    let mut kinds = vec!["password"];
    if steps["app"].is_object() {
        kinds.push("app");
    }
    let here = |passkey: &Value| passkey["relying-party"].as_str() == Some(host);
    if steps["passkeys"].as_array().is_some_and(|passkeys| passkeys.iter().any(here)) {
        kinds.push("passkey");
    }
    kinds
}

/// `GET /api/1/session`: who this is, and what the page needs to go on.
pub async fn show(cx: &Context<'_>, live: &Live) -> Outcome {
    let who = who::read(&cx.shared.accounts, Account::Name(&live.name))
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let (unlocked, csrf) = cx
        .shared
        .with_session(&live.cookie, |session| (session.unlocked_until, session.csrf_text()))
        .ok_or_else(super::ended)?;
    // The password is the proof every account has, so it is what the page
    // offers when the worker does not say more.
    let kinds = match live.link.call("steps", Value::Null).await {
        Ok(steps) => kinds_of(&steps, &cx.host),
        Err(_) => vec!["password"],
    };
    Ok(Reply::json(json!({
        "name": live.name,
        "full_name": who.full_name,
        "admin": who.admin,
        "host": cx.shared.identity.borrow().host,
        "csrf": csrf,
        "unlock_seconds": seconds_left(unlocked, unix_now()),
        "needs_step": live.needs_step,
        "kinds": kinds,
        "terminal": super::terminals::offered(cx.shared, &live.name),
        "from": cx.peer.to_string(),
        "via": via(cx),
    })))
}

/// The address and port the request's connection came to, and the card
/// that holds the address: null when admind does not know them.
fn via(cx: &Context<'_>) -> Value {
    match cx.connection.to {
        Some(to) => {
            let address = reach::plain(to.ip());
            json!({ "address": address.to_string(), "port": to.port(), "card": reach::card_of(address) })
        }
        None => Value::Null,
    }
}

/// `GET /api/1/console/fresh/{trial}` (ADR 0518): whether this request's
/// connection was opened after the change of the trial `trial` took
/// effect, so that its getting here proves the browser reaches the
/// console over the new settings. One opened before is asked to end once
/// this answer is sent (a GOAWAY on HTTP/2), so that the browser's next
/// request opens a new one.
pub async fn fresh(cx: &Context<'_>) -> Outcome {
    let id = cx.params.get("trial").and_then(Value::as_str).unwrap_or_default();
    spark_api::trial::check_id(id).map_err(|why| Failure::new(Kind::BadRequest, why))?;
    let document = trials::document(cx.shared.spark.get())
        .await
        .map_err(|why| Failure::new(Kind::Internal, why))?;
    let fresh = {
        let mut trials = cx.shared.trials.borrow_mut();
        trials.read(&document, Instant::now());
        trials.fresh(id, cx.connection.accepted)
    };
    match fresh {
        Some(true) => {
            Ok(Reply::json(json!({ "fresh": true, "from": cx.peer.to_string(), "via": via(cx) })))
        }
        Some(false) => {
            cx.connection.end_after().await;
            let why = "this request came by a connection opened before the change took effect";
            Err(Failure::new(Kind::OldConnection, why))
        }
        None => {
            let why = format!("Spark is trying nothing called {id}: it was kept or undone");
            Err(Failure::new(Kind::NoTrial, why))
        }
    }
}

/// `POST /api/1/sign-out`.
pub fn sign_out(cx: &Context<'_>, live: &Live) -> Outcome {
    let session = cx.shared.sessions.borrow_mut().remove(&live.cookie);
    if let Some(session) = session {
        drop(cx.shared.finish(session, "signed-out"));
    }
    Ok(Reply::json(json!({ "done": true })).with_cookie(guard::clear_cookie(SESSION_COOKIE)))
}

/// How the person proves, as the page names it.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Way {
    Password,
    Code,
    Passkey,
}

/// What to give `what`, which the request left out.
fn missing(what: &str) -> Failure {
    Failure::new(Kind::BadRequest, format!("give the {what}"))
}

/// `POST /api/1/unlock`: the proof, and authd's word on it.
pub async fn unlock(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        kind: Way,
        password: Option<Text>,
        code: Option<Text>,
        credential: Option<Credential>,
    }
    let body: Body = cx.json()?;
    let (proof, attempt) = match body.kind {
        Way::Password => {
            let password = body.password.ok_or_else(|| missing("password"))?;
            (Proof::Password(password.as_bytes().to_vec()), Attempt::Password)
        }
        Way::Code => {
            let code = body.code.ok_or_else(|| missing("code"))?;
            (Proof::Code(code.as_bytes().to_vec()), Attempt::Code)
        }
        Way::Passkey => {
            let credential = body.credential.ok_or_else(|| missing("credential"))?;
            let assertion = webauthn::assertion(&credential, &cx.origin, &cx.host)?;
            (Proof::Passkey(assertion), Attempt::Passkey)
        }
    };
    let console = Console::Unlock { session: live.authd.clone(), proof };
    let what = format!("to unlock {}'s session", shown(&live.name));
    let text = ask(cx, console, attempt, &what).await?;
    unlocked(cx, live, &text)
}

/// `POST /api/1/unlock/passkey/options`: what the browser asks a passkey
/// with, to unlock.
pub async fn unlock_options(cx: &Context<'_>, live: &Live) -> Outcome {
    let console = Console::Challenge {
        purpose: Purpose::Unlock(live.authd.clone()),
        relying_party: cx.host.clone(),
    };
    let what = format!("a passkey challenge to unlock {}'s session", shown(&live.name));
    let text = ask(cx, console, Attempt::Passkey, &what).await?;
    // Unlocking is a second proof of the person, so the authenticator must
    // have checked them too.
    Ok(Reply::json(webauthn::request_options(&text, &cx.host, "required")?))
}

/// `POST /api/1/unlock/extend`.
pub async fn extend(cx: &Context<'_>, live: &Live) -> Outcome {
    let console = Console::Extend { session: live.authd.clone() };
    let what = format!("to extend {}'s unlock", shown(&live.name));
    let text = ask(cx, console, Attempt::Other, &what).await?;
    unlocked(cx, live, &text)
}

/// `POST /api/1/lock`.
pub async fn lock(cx: &Context<'_>, live: &Live) -> Outcome {
    // The administrators' panes and the follow of the whole log end first,
    // for `locked`: authd ends their shells and the follow's command as it
    // locks, and one that ended while authd was asked would say only that
    // its program exited.
    cx.shared.terminals.borrow_mut().end_admin_panes(&live.authd, Why::Locked);
    cx.shared.follows.borrow_mut().lock(&live.cookie);
    let what = format!("to lock {}'s session", shown(&live.name));
    ask(cx, Console::Lock { session: live.authd.clone() }, Attempt::Other, &what).await?;
    cx.shared.set_unlocked(&live.cookie, None);
    Ok(Reply::json(json!({ "unlock_seconds": null })))
}

/// The answer to an unlock or an extension, when authd said it ends at the
/// Unix time `text`.
fn unlocked(cx: &Context<'_>, live: &Live, text: &str) -> Outcome {
    let until: i64 = text
        .trim()
        .parse()
        .map_err(|_| Failure::new(Kind::AuthdSilent, "authd answered a time that does not read"))?;
    cx.shared.set_unlocked(&live.cookie, Some(until));
    Ok(Reply::json(json!({ "unlock_seconds": seconds_left(Some(until), unix_now()) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ways_to_unlock_are_the_password_and_what_the_account_has_for_this_name() {
        let steps = json!({
            "app": { "added": 1 },
            "passkeys": [
                { "id": "01", "relying-party": "other.example", "name": "a" },
                { "id": "02", "relying-party": "duke-nas.local", "name": "b" },
            ],
        });
        assert_eq!(kinds_of(&steps, "duke-nas.local"), ["password", "app", "passkey"]);
        // A passkey made for another name is not offered.
        assert_eq!(kinds_of(&steps, "192.168.1.10"), ["password", "app"]);
        let none = json!({ "app": null, "passkeys": [], "needed": false });
        assert_eq!(kinds_of(&none, "duke-nas.local"), ["password"]);
        assert_eq!(kinds_of(&json!({}), "duke-nas.local"), ["password"]);
        assert_eq!(kinds_of(&Value::Null, "duke-nas.local"), ["password"]);
    }
}
