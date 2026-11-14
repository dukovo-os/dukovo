// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The person's own second steps and API tokens (ADR 0514): the
//! authenticator app, passkeys, recovery codes, the browsers they trust,
//! and the tokens that stand for them to scripts.
//!
//! Each route is one call to the session's worker, which asks authd as the
//! person, so that authd sees them asking, as it does for `passwd`. admind
//! only turns what a browser sends into the call, and the worker's answer
//! into what the page reads; whether the password or the unlock is enough
//! is authd's to say. A `proof` is the person's password, or empty while
//! the session is unlocked.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Context, Live, Outcome, Reply, Text, refused, shown};
use crate::admind::errors::{self, Attempt, Failure, Kind};
use crate::admind::link::CallError;
use crate::admind::webauthn::{self, Made};

/// The failure that a call to the worker that went wrong is, for what
/// `attempt` was.
fn failed(error: CallError, attempt: Attempt) -> Failure {
    match error {
        // The worker puts authd's own words in a refusal, which tell what
        // kind it is.
        CallError::Worker(fault) if fault.id == Kind::Refused.id() => {
            errors::refusal(attempt, &fault.detail)
        }
        CallError::Worker(fault) => {
            Failure::new(Kind::of(&fault.id).unwrap_or(Kind::Internal), fault.detail)
        }
        CallError::Silent => {
            Failure::new(Kind::WorkerSilent, "the session's worker did not answer in time")
        }
        CallError::Gone => Failure::new(Kind::SessionEnded, "the session's worker has ended"),
    }
}

/// Has the worker of `live` do `method` with `params`, and gives what it
/// says, or why it could not; a refusal goes to the log.
async fn call(
    cx: &Context<'_>,
    live: &Live,
    method: &str,
    params: Value,
    attempt: Attempt,
) -> Result<Value, Failure> {
    match live.link.call(method, params).await {
        Ok(value) => Ok(value),
        Err(error) => {
            let what = format!("{method} for {}", shown(&live.name));
            Err(refused(cx, &what, failed(error, attempt)))
        }
    }
}

/// What the page sends to change something that asks for the password.
#[derive(Deserialize)]
struct Proof {
    /// The password, or nothing while the session is unlocked.
    #[serde(default)]
    proof: Text,
}

/// `GET /api/1/account/second-step`: what the account has.
pub async fn steps(live: &Live) -> Outcome {
    let steps =
        live.link.call("steps", Value::Null).await.map_err(|e| failed(e, Attempt::Other))?;
    Ok(Reply::json(steps))
}

/// `POST /api/1/account/second-step/app/begin`: a new secret for an app,
/// which is not in use until a code of it is confirmed.
pub async fn app_begin(cx: &Context<'_>, live: &Live) -> Outcome {
    let body: Proof = cx.json()?;
    let host = cx.shared.identity.borrow().host.clone();
    let params = json!({ "proof": &*body.proof, "host": host });
    Ok(Reply::json(call(cx, live, "app-begin", params, Attempt::Password).await?))
}

/// `POST /api/1/account/second-step/app/confirm`: the first code of the
/// new secret, which turns the app on and gives recovery codes.
pub async fn app_confirm(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        code: Text,
    }
    let body: Body = cx.json()?;
    let params = json!({ "code": &*body.code });
    Ok(Reply::json(call(cx, live, "app-confirm", params, Attempt::Code).await?))
}

/// `POST /api/1/account/second-step/app/remove`.
pub async fn app_remove(cx: &Context<'_>, live: &Live) -> Outcome {
    let body: Proof = cx.json()?;
    let params = json!({ "proof": &*body.proof });
    Ok(Reply::json(call(cx, live, "app-remove", params, Attempt::Password).await?))
}

/// `POST /api/1/account/second-step/passkey/begin`: what the browser makes
/// a passkey with, for the name it opened the console by.
pub async fn passkey_begin(cx: &Context<'_>, live: &Live) -> Outcome {
    let body: Proof = cx.json()?;
    let params = json!({ "proof": &*body.proof, "relying_party": cx.host });
    let begun = call(cx, live, "passkey-begin", params, Attempt::Password).await?;
    let machine = cx.shared.identity.borrow().host.clone();
    Ok(Reply::json(webauthn::creation_options(begun, &cx.host, &machine)?))
}

/// `POST /api/1/account/second-step/passkey/add`: the passkey the browser
/// made, with the name the person gave it.
pub async fn passkey_add(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        name: Text,
        credential: Made,
    }
    let body: Body = cx.json()?;
    let made = webauthn::registration(&body.credential)?;
    let params = json!({
        "challenge": made.challenge,
        "origin": cx.origin,
        "relying_party": cx.host,
        "name": &*body.name,
        "client_data": made.client_data,
        "attestation": made.attestation,
    });
    Ok(Reply::json(call(cx, live, "passkey-add", params, Attempt::Passkey).await?))
}

/// `POST /api/1/account/second-step/passkey/remove`.
pub async fn passkey_remove(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        proof: Text,
        id: Text,
    }
    let body: Body = cx.json()?;
    let params = json!({ "proof": &*body.proof, "id": &*body.id });
    Ok(Reply::json(call(cx, live, "passkey-remove", params, Attempt::Password).await?))
}

/// `POST /api/1/account/second-step/passkey/rename`.
pub async fn passkey_rename(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        id: Text,
        name: Text,
    }
    let body: Body = cx.json()?;
    let params = json!({ "id": &*body.id, "name": &*body.name });
    Ok(Reply::json(call(cx, live, "passkey-rename", params, Attempt::Other).await?))
}

/// `POST /api/1/account/second-step/codes`: new recovery codes, which
/// replace the old.
pub async fn codes(cx: &Context<'_>, live: &Live) -> Outcome {
    let body: Proof = cx.json()?;
    let params = json!({ "proof": &*body.proof });
    Ok(Reply::json(call(cx, live, "codes", params, Attempt::Password).await?))
}

/// `POST /api/1/account/second-step/trusted/forget`: every browser the
/// person trusted, which asks for the second step again.
pub async fn trusted_forget(live: &Live) -> Outcome {
    let forgotten = live
        .link
        .call("trusted-forget", Value::Null)
        .await
        .map_err(|error| failed(error, Attempt::Other))?;
    Ok(Reply::json(forgotten))
}

/// `POST /api/1/account/tokens/add`: a new API token, which is shown this
/// once.
pub async fn token_add(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        proof: Text,
        name: Text,
        scopes: Vec<Text>,
        days: u32,
    }
    let body: Body = cx.json()?;
    let scopes: Vec<&str> = body.scopes.iter().map(|scope| &**scope).collect();
    let params = json!({
        "proof": &*body.proof,
        "name": &*body.name,
        "scopes": scopes,
        "days": body.days,
    });
    Ok(Reply::json(call(cx, live, "token-add", params, Attempt::Password).await?))
}

/// `POST /api/1/account/tokens/remove`.
pub async fn token_remove(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        name: Text,
    }
    let body: Body = cx.json()?;
    let params = json!({ "name": &*body.name });
    Ok(Reply::json(call(cx, live, "token-remove", params, Attempt::Other).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::link::Fault;

    #[test]
    fn a_worker_that_could_not_ask_authd_has_its_own_words_passed_on() {
        let fault = Fault::new(Kind::BadRequest, "app-confirm does not take that");
        let failure = failed(CallError::Worker(fault), Attempt::Code);
        assert_eq!(
            (failure.kind, failure.detail.as_str()),
            (Kind::BadRequest, "app-confirm does not take that")
        );
        // An ID that is none of the console's is an internal fault.
        let odd = Fault { id: "ADM-999".into(), title: "x".into(), detail: "odd".into() };
        assert_eq!(failed(CallError::Worker(odd), Attempt::Other).kind, Kind::Internal);
    }

    #[test]
    fn authds_refusals_that_the_worker_passes_on_are_told_apart_by_their_words() {
        let refusal = |why: &str, attempt| {
            failed(CallError::Worker(Fault::new(Kind::Refused, why)), attempt).kind
        };
        assert_eq!(
            refusal(accounts::request::WRONG_PASSWORD, Attempt::Password),
            Kind::WrongPassword
        );
        assert_eq!(refusal("that code is not right", Attempt::Code), Kind::WrongCode);
        assert_eq!(refusal("the console is locked", Attempt::Other), Kind::Locked);
        assert_eq!(refusal("a token with that name exists", Attempt::Other), Kind::Refused);
    }

    #[test]
    fn a_worker_that_is_silent_or_gone_is_told_as_such() {
        assert_eq!(failed(CallError::Silent, Attempt::Other).kind, Kind::WorkerSilent);
        assert_eq!(failed(CallError::Gone, Attempt::Other).kind, Kind::SessionEnded);
    }
}
