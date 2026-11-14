// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The JSON a browser's WebAuthn API takes and gives, and authd's words
//! for the same things (ADR 0514). The page passes a browser's credential
//! on as JSON, in the form `PublicKeyCredential.toJSON()` writes it:
//! binary values are base64url without padding. authd takes them as
//! bytes, and wants the challenge named apart, which the browser has put
//! in the client data it signed, so it is read from there. authd checks
//! that it gave the challenge, for this purpose and this process: what
//! admind reads out of the client data is only the name of the challenge,
//! never trusted for more.
//!
//! The options the page hands the browser are made here too, in the form
//! `navigator.credentials.get` and `create` take once the page has turned
//! the base64url values into `ArrayBuffer`s.

use accounts::console::{Assertion, from_hex};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::{Value, json};

use super::errors::{Failure, Kind};
use crate::authd::proofs::ALGORITHMS;

/// How long a browser has to answer, in milliseconds.
const TIMEOUT: u64 = 120_000;

/// A passkey's answer to a challenge, as the browser wrote it.
#[derive(Deserialize)]
pub struct Credential {
    /// The passkey's id.
    id: String,
    response: AssertionResponse,
}

#[derive(Deserialize)]
struct AssertionResponse {
    #[serde(rename = "clientDataJSON")]
    client_data: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
    #[serde(rename = "userHandle", default)]
    user_handle: Option<String>,
}

/// A new passkey, as the browser wrote it.
#[derive(Deserialize)]
pub struct Made {
    response: MadeResponse,
}

#[derive(Deserialize)]
struct MadeResponse {
    #[serde(rename = "clientDataJSON")]
    client_data: String,
    #[serde(rename = "attestationObject")]
    attestation: String,
}

/// What the worker's `passkey-add` is told of a new passkey, from the page.
#[derive(Debug, PartialEq, Eq)]
pub struct Registration {
    /// The challenge it answers, in base64url.
    pub challenge: String,
    /// The client data, in base64url.
    pub client_data: String,
    /// The attestation object, in base64url.
    pub attestation: String,
}

fn bad(detail: impl Into<String>) -> Failure {
    Failure::new(Kind::BadRequest, detail)
}

/// The bytes of `text`, which the browser wrote as base64url.
fn unbase64(what: &str, text: &str) -> Result<Vec<u8>, Failure> {
    URL_SAFE_NO_PAD.decode(text).map_err(|_| bad(format!("{what} is not base64url")))
}

/// The challenge that `client_data`, a browser's client data JSON, says it
/// answers.
fn challenge_of(client_data: &[u8]) -> Result<Vec<u8>, Failure> {
    #[derive(Deserialize)]
    struct Data {
        challenge: String,
    }
    let data: Data = serde_json::from_slice(client_data)
        .map_err(|_| bad("the client data is not the JSON of a browser's, with a challenge"))?;
    unbase64("the challenge in the client data", &data.challenge)
}

/// What a passkey's answer is to authd, for a console at `origin` and
/// relying party `relying_party`.
///
/// # Errors
///
/// If a value is not base64url, or the client data names no challenge.
pub fn assertion(
    credential: &Credential,
    origin: &str,
    relying_party: &str,
) -> Result<Assertion, Failure> {
    let response = &credential.response;
    let client_data = unbase64("the client data", &response.client_data)?;
    Ok(Assertion {
        challenge: challenge_of(&client_data)?,
        origin: origin.into(),
        relying_party: relying_party.into(),
        credential: unbase64("the passkey's id", &credential.id)?,
        client_data,
        authenticator_data: unbase64("the authenticator data", &response.authenticator_data)?,
        signature: unbase64("the signature", &response.signature)?,
        // A passkey that is the second step has no user handle to give.
        user_handle: match &response.user_handle {
            Some(handle) => unbase64("the user handle", handle)?,
            None => Vec::new(),
        },
    })
}

/// What the worker is told of a new passkey `made`.
///
/// # Errors
///
/// If a value is not base64url, or the client data names no challenge.
pub fn registration(made: &Made) -> Result<Registration, Failure> {
    let client_data = unbase64("the client data", &made.response.client_data)?;
    unbase64("the attestation", &made.response.attestation)?;
    Ok(Registration {
        challenge: URL_SAFE_NO_PAD.encode(challenge_of(&client_data)?),
        client_data: made.response.client_data.clone(),
        attestation: made.response.attestation.clone(),
    })
}

fn authd_unreadable() -> Failure {
    Failure::new(Kind::AuthdSilent, "authd answered a challenge that does not read")
}

/// The options for `navigator.credentials.get` from authd's answer
/// `text` to a challenge, `CHALLENGE ID...` in hexadecimal: with
/// `verification`, `required` or `preferred`, for `relying_party`.
///
/// # Errors
///
/// If the answer does not read.
pub fn request_options(
    text: &str,
    relying_party: &str,
    verification: &str,
) -> Result<Value, Failure> {
    let mut words = text.split_whitespace();
    let base64 = |hex: &str| from_hex(hex.as_bytes()).map(|bytes| URL_SAFE_NO_PAD.encode(bytes));
    let challenge = words.next().and_then(base64).ok_or_else(authd_unreadable)?;
    let allowed: Option<Vec<Value>> =
        words.map(|hex| base64(hex).map(|id| json!({ "type": "public-key", "id": id }))).collect();
    Ok(json!({ "publicKey": {
        "challenge": challenge,
        "rpId": relying_party,
        "allowCredentials": allowed.ok_or_else(authd_unreadable)?,
        "userVerification": verification,
        "timeout": TIMEOUT,
    }}))
}

/// What the worker answers to `passkey-begin`: the challenge, the person
/// and the ids of the passkeys they have already, all as the browser
/// takes them.
#[derive(Deserialize)]
struct Begun {
    challenge: String,
    user: Value,
    exclude: Vec<String>,
}

/// The options for `navigator.credentials.create` from the worker's
/// answer `begun`, for `relying_party`, which the browser shows as
/// `Dukovo (machine)`.
///
/// # Errors
///
/// If the worker's answer does not read.
pub fn creation_options(
    begun: Value,
    relying_party: &str,
    machine: &str,
) -> Result<Value, Failure> {
    let begun: Begun = serde_json::from_value(begun).map_err(|_| {
        Failure::new(Kind::WorkerSilent, "the worker answered a challenge that does not read")
    })?;
    let parameters: Vec<Value> =
        ALGORITHMS.iter().map(|alg| json!({ "type": "public-key", "alg": alg })).collect();
    let exclude: Vec<Value> =
        begun.exclude.iter().map(|id| json!({ "type": "public-key", "id": id })).collect();
    Ok(json!({ "publicKey": {
        "challenge": begun.challenge,
        "rp": { "id": relying_party, "name": format!("Dukovo ({machine})") },
        "user": begun.user,
        "pubKeyCredParams": parameters,
        "excludeCredentials": exclude,
        "authenticatorSelection": { "residentKey": "required", "userVerification": "required" },
        "attestation": "none",
        "timeout": TIMEOUT,
    }}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// The client data a browser writes for a challenge.
    fn client_data(challenge: &[u8], kind: &str) -> String {
        let json = json!({
            "type": kind,
            "challenge": b64(challenge),
            "origin": "https://duke-nas.local:9090",
            "crossOrigin": false,
        });
        b64(json.to_string().as_bytes())
    }

    fn credential(challenge: &[u8], handle: Option<&str>) -> Credential {
        let mut response = json!({
            "clientDataJSON": client_data(challenge, "webauthn.get"),
            "authenticatorData": b64(b"auth"),
            "signature": b64(b"sig"),
        });
        if let Some(handle) = handle {
            response["userHandle"] = b64(handle.as_bytes()).into();
        }
        let browser = json!({
            "id": b64(&[1, 2, 3]),
            "rawId": b64(&[1, 2, 3]),
            "type": "public-key",
            "authenticatorAttachment": "platform",
            "clientExtensionResults": {},
            "response": response,
        });
        serde_json::from_value(browser).unwrap()
    }

    #[test]
    fn a_credential_goes_to_authd_with_the_challenge_its_client_data_names() {
        let challenge = [7u8; 32];
        let assertion = assertion(
            &credential(&challenge, Some("handle")),
            "https://duke-nas.local:9090",
            "duke-nas.local",
        )
        .unwrap();
        assert_eq!(assertion.challenge, challenge);
        assert_eq!(assertion.origin, "https://duke-nas.local:9090");
        assert_eq!(assertion.relying_party, "duke-nas.local");
        assert_eq!(assertion.credential, [1, 2, 3]);
        assert_eq!(assertion.authenticator_data, b"auth");
        assert_eq!(assertion.signature, b"sig");
        assert_eq!(assertion.user_handle, b"handle");
        assert_eq!(
            assertion.client_data,
            serde_json::to_vec(&json!({
                "type": "webauthn.get",
                "challenge": b64(&challenge),
                "origin": "https://duke-nas.local:9090",
                "crossOrigin": false,
            }))
            .unwrap()
        );
        // As a second step a passkey gives no user handle.
        let second = self::assertion(&credential(&challenge, None), "o", "r").unwrap();
        assert!(second.user_handle.is_empty());
    }

    #[test]
    fn a_credential_that_does_not_read_is_refused_in_words() {
        let mut broken = credential(&[1], None);
        broken.response.signature = "!!".into();
        assert_eq!(
            assertion(&broken, "o", "r").err().unwrap().detail,
            "the signature is not base64url"
        );
        let mut broken = credential(&[1], None);
        broken.response.client_data = b64(b"not json");
        let why = assertion(&broken, "o", "r").err().unwrap();
        assert_eq!(why.kind, Kind::BadRequest);
        assert!(why.detail.contains("client data"), "{}", why.detail);
        let mut broken = credential(&[1], None);
        broken.response.client_data = b64(br#"{"challenge": "!!"}"#);
        assert!(assertion(&broken, "o", "r").err().unwrap().detail.contains("the challenge"));
        // The browser's own JSON is what is read: a credential with no
        // response is no credential.
        assert!(serde_json::from_value::<Credential>(json!({"id": "AQ"})).is_err());
    }

    #[test]
    fn a_new_passkey_goes_to_the_worker_with_its_challenge() {
        let made: Made = serde_json::from_value(json!({
            "id": "AQ",
            "response": {
                "clientDataJSON": client_data(&[9, 9], "webauthn.create"),
                "attestationObject": b64(b"attestation"),
                "transports": ["usb"],
            },
        }))
        .unwrap();
        let registration = registration(&made).unwrap();
        assert_eq!(registration.challenge, b64(&[9, 9]));
        assert_eq!(registration.client_data, client_data(&[9, 9], "webauthn.create"));
        assert_eq!(registration.attestation, b64(b"attestation"));
        let broken: Made = serde_json::from_value(json!({"response": {
            "clientDataJSON": client_data(&[9], "webauthn.create"),
            "attestationObject": "!",
        }}))
        .unwrap();
        assert_eq!(
            super::registration(&broken).unwrap_err().detail,
            "the attestation is not base64url"
        );
    }

    #[test]
    fn the_options_to_sign_in_name_the_challenge_the_host_and_the_passkeys_to_use() {
        let options = request_options("0a0b 01 ff02", "duke-nas.local", "preferred").unwrap();
        assert_eq!(
            options,
            json!({ "publicKey": {
                "challenge": b64(&[0x0a, 0x0b]),
                "rpId": "duke-nas.local",
                "allowCredentials": [
                    { "type": "public-key", "id": b64(&[1]) },
                    { "type": "public-key", "id": b64(&[0xff, 2]) },
                ],
                "userVerification": "preferred",
                "timeout": 120_000,
            }})
        );
        // A sign-in with a passkey alone has none to name.
        let alone = request_options("0a0b", "duke-nas.local", "required").unwrap();
        assert_eq!(alone["publicKey"]["allowCredentials"], json!([]));
        assert_eq!(alone["publicKey"]["userVerification"], "required");
        for wrong in ["", "zz", "0a zz", "0"] {
            let why = request_options(wrong, "h", "required").unwrap_err();
            assert_eq!(why.kind, Kind::AuthdSilent, "{wrong}");
        }
    }

    #[test]
    fn the_options_to_add_a_passkey_ask_for_the_three_kinds_authd_checks() {
        let begun = json!({
            "challenge": b64(&[5; 4]),
            "user": { "id": b64(b"handle"), "name": "chase", "displayName": "Chase Y" },
            "exclude": [b64(&[1]), b64(&[2])],
        });
        let options = creation_options(begun, "duke-nas.local", "duke-nas").unwrap();
        let key = &options["publicKey"];
        assert_eq!(key["rp"], json!({ "id": "duke-nas.local", "name": "Dukovo (duke-nas)" }));
        assert_eq!(key["user"]["displayName"], "Chase Y");
        assert_eq!(
            key["pubKeyCredParams"],
            json!([
                { "type": "public-key", "alg": -7 },
                { "type": "public-key", "alg": -8 },
                { "type": "public-key", "alg": -257 },
            ])
        );
        assert_eq!(key["excludeCredentials"][1], json!({ "type": "public-key", "id": b64(&[2]) }));
        assert_eq!(
            key["authenticatorSelection"],
            json!({ "residentKey": "required", "userVerification": "required" })
        );
        assert_eq!(
            (key["attestation"].as_str(), key["timeout"].as_u64()),
            (Some("none"), Some(120_000))
        );
        let why = creation_options(json!({"challenge": 1}), "h", "m").unwrap_err();
        assert_eq!(why.kind, Kind::WorkerSilent);
    }
}
