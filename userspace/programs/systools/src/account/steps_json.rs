// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An account's second steps and API tokens as `user show --json` gives
//! them, read from what authd answers: [`accounts::console::Console::Steps`]
//! for a person's own, and `StepsOf` for root's. authd keeps no secret in
//! the answer, and this keeps less: what the page names and no more.

use serde::Serialize;
use serde_json::Value;

/// What an account has to sign in with besides its password.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Steps {
    /// Whether an authenticator app is added.
    pub app: bool,
    /// The passkeys, by the name their owner gave.
    pub passkeys: Vec<Passkey>,
    /// How many recovery codes are left, 0 if none were made.
    pub recovery_left: u64,
    /// How many browsers are trusted still.
    pub trusted_browsers: u64,
}

/// A passkey.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Passkey {
    pub name: String,
    /// Its identifier, which names it to authd.
    pub id: String,
}

/// An API token, never its secret.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Token {
    pub name: String,
    /// What it may do: `*`, or an area's name, alone or with `:read`.
    pub scopes: Vec<String>,
    /// When it runs out, in Unix milliseconds.
    pub until_ms: i64,
}

/// The steps and the tokens in authd's answer `text`, or `None` if it is
/// not shaped as authd writes it.
pub(super) fn parse(text: &str) -> Option<(Steps, Vec<Token>)> {
    let answer: Value = serde_json::from_str(text).ok()?;
    let text_of = |value: &Value, key: &str| Some(value.get(key)?.as_str()?.to_string());
    let passkeys = answer
        .get("passkeys")?
        .as_array()?
        .iter()
        .map(|p| Some(Passkey { name: text_of(p, "name")?, id: text_of(p, "id")? }))
        .collect::<Option<Vec<_>>>()?;
    let tokens = answer
        .get("tokens")?
        .as_array()?
        .iter()
        .map(|t| {
            let scopes = t.get("scopes")?.as_array()?;
            Some(Token {
                name: text_of(t, "name")?,
                scopes: scopes
                    .iter()
                    .map(|s| Some(s.as_str()?.to_string()))
                    .collect::<Option<_>>()?,
                until_ms: t.get("until")?.as_i64()?.saturating_mul(1000),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    // No codes were made, or all went: none are left.
    let recovery_left = match answer.get("recovery")? {
        Value::Null => 0,
        made => made.get("left")?.as_u64()?,
    };
    let steps = Steps {
        app: !answer.get("app")?.is_null(),
        passkeys,
        recovery_left,
        trusted_browsers: answer.get("trusted")?.as_u64()?,
    };
    Some((steps, tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What authd answers for an account with an app, a passkey, eight
    /// codes, two trusted browsers and a token.
    const ANSWER: &str = r#"{"app":{"added":1760900000},
        "passkeys":[{"id":"AbC-1","relying-party":"lab","name":"Security key","added":1,"used":2}],
        "recovery":{"left":8,"made":3},"trusted":2,
        "tokens":[{"name":"ansible","scopes":["*"],"until":1765600000,"made":1,"used":0,
                   "expired":false}],
        "needed":true}"#;

    #[test]
    fn authds_answer_gives_the_steps_and_the_tokens_without_more() {
        let (steps, tokens) = parse(ANSWER).unwrap();
        assert_eq!(
            steps,
            Steps {
                app: true,
                passkeys: vec![Passkey { name: "Security key".into(), id: "AbC-1".into() }],
                recovery_left: 8,
                trusted_browsers: 2,
            }
        );
        assert_eq!(
            tokens,
            [Token {
                name: "ansible".into(),
                scopes: vec!["*".into()],
                until_ms: 1_765_600_000_000
            }]
        );
    }

    #[test]
    fn an_account_with_no_steps_has_none() {
        let (steps, tokens) = parse(
            r#"{"app":null,"passkeys":[],"recovery":null,"trusted":0,"tokens":[],"needed":false}"#,
        )
        .unwrap();
        assert_eq!(
            steps,
            Steps { app: false, passkeys: Vec::new(), recovery_left: 0, trusted_browsers: 0 }
        );
        assert!(tokens.is_empty());
    }

    #[test]
    fn an_answer_that_is_not_authds_is_none() {
        for text in [
            "",
            "ok",
            "{}",
            r#"{"app":null,"passkeys":[{"id":"x"}],"recovery":null,"trusted":0,"tokens":[]}"#,
            r#"{"app":null,"passkeys":[],"recovery":{},"trusted":0,"tokens":[]}"#,
            r#"{"app":null,"passkeys":[],"recovery":null,"trusted":0,"tokens":[{"name":"a"}]}"#,
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
