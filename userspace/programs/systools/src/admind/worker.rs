// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admind --worker`: what a signed-in session of the console does as the
//! person (ADR 0514). authd starts it for the person and their groups,
//! without the bypass right, with its ends of the session's link (the
//! pipe it reads calls from as descriptor 0, and the pipe it writes
//! replies to as 3) and admind's log as 1 and 2. It answers admind's calls
//! over [`link`](super::link), many at once, and opens no socket of its
//! own. It ends when the link closes, or when no call has come for 12
//! hours, which is the backstop for a session whose admind has gone
//! without closing it.
//!
//! Its methods are `whoami`; `run`, which runs an action's command as the
//! person ([`run`]); `preferences` and `preferences-change`, which read
//! and change the person's own preferences file in their home
//! ([`preferences`]); and one for each of the person's own second steps
//! and tokens, which it passes to authd on a connection of its own,
//! so that authd sees the person asking, as it does for `passwd`. A
//! request's `proof` is the person's password, or empty while the session
//! is unlocked, which authd accepts only from this worker itself.
//! Binary values that a browser gives or takes (challenges, client data,
//! attestations) are base64url in the JSON, as it writes them; the ids of
//! passkeys are hexadecimal, as `steps` lists them.

use std::io;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::Duration;

use accounts::console::{Console, Way, from_hex};
use accounts::request::Answer;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sysapi::fd::Standard;

use super::authd;
use super::errors::Kind;
use super::link::{self, Ended, Fault};
use super::log;
use super::preferences::{self, Change};
use super::who::{self, Account, Sources, Who};
use crate::util::{self, Fail, fail};

mod run;

/// How long the worker waits for a call before it ends.
const IDLE: Duration = Duration::from_hours(12);

/// The descriptor of the pipe the worker reads calls from.
const CALLS: RawFd = 0;

/// The descriptor of the pipe the worker writes replies to.
const REPLIES: RawFd = 3;

/// `admind --worker`.
///
/// # Errors
///
/// If descriptors 0 and 3 are not the link's pipes, or the link breaks.
pub fn run() -> Result<i32, Fail> {
    // Each is closed in what the worker starts, so that no command it runs
    // can read the calls or keep the link open after the worker ends. The
    // calls' pipe leaves descriptor 0, which keeps the null device.
    let missing = |err: io::Error| {
        fail(format!(
            "the worker is started by authd, with the link's pipes as descriptors {CALLS} and \
             {REPLIES}: {}",
            util::strip_os_error(&err)
        ))
    };
    let replies = sysapi::fd::handed_down(REPLIES).map_err(missing)?;
    let calls = sysapi::fd::take_standard(Standard::Input).map_err(missing)?;
    let theirs = link::Ends::new(calls, replies);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| util::failed(&"the runtime", err))?;
    let local = tokio::task::LocalSet::new();
    let ended = local.block_on(&runtime, link::serve(theirs, IDLE, handle)).map_err(|err| {
        fail(format!(
            "the worker is started by authd, with the link's pipes as descriptors {CALLS} and \
             {REPLIES}: {}",
            util::strip_os_error(&err)
        ))
    })?;
    match ended {
        Ended::Closed => Ok(0),
        Ended::Idle => {
            log(6, "the worker ends: no call came for 12 hours");
            Ok(0)
        }
        Ended::Broken(why) => Err(fail(format!("the link broke: {why}"))),
    }
}

/// Does the call of `method` with `params`.
async fn handle(method: String, params: Value) -> Result<Value, Fault> {
    if method == "whoami" {
        nothing(&params)?;
        return Ok(whoami()?.json());
    }
    if method == "run" {
        return run::run(params).await;
    }
    if method == "preferences" {
        nothing(&params)?;
        return Ok(json!(preferences::read(&home()?)?));
    }
    if method == "preferences-change" {
        let change: Change = take(&method, params)?;
        return Ok(json!(preferences::change(&home()?, &change)?));
    }
    let (console, shape) = request(&method, params)?;
    let (answer, _) = authd::ask(accounts::SOCKET, console, &[])
        .await
        .map_err(|why| Fault::new(Kind::AuthdSilent, why))?;
    match answer {
        Answer::Done(text) => result(&shape, &text, whoami),
        Answer::Refused(why) => Err(Fault::new(Kind::Refused, why)),
    }
}

/// How authd's answer to a request is told to admind.
#[derive(Debug, PartialEq, Eq)]
enum Shape {
    /// authd's JSON, as it is.
    Steps,
    /// An `otpauth://` URI: `{uri}`.
    Uri,
    /// Recovery codes, which may be none: `{codes}`.
    Codes,
    /// How many browsers were forgotten: `{forgotten}`.
    Forgotten,
    /// A new API token: `{token}`.
    Token,
    /// A new MCP client: `{token}`, its token for one that connects over
    /// HTTPS, and null for one that connects over SSH, which has none.
    Client,
    /// Nothing to tell: `{done: true}`.
    Done,
    /// What the browser needs to make a passkey: the challenge, the user
    /// and the passkeys to leave out.
    Creation,
}

/// The parameters of the calls that have any, which must be all there is.
mod takes {
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Proof {
        pub proof: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AppBegin {
        pub proof: String,
        pub host: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct AppConfirm {
        pub code: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PasskeyBegin {
        pub proof: String,
        pub relying_party: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PasskeyAdd {
        pub challenge: String,
        pub origin: String,
        pub relying_party: String,
        pub name: String,
        pub client_data: String,
        pub attestation: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PasskeyRemove {
        pub proof: String,
        pub id: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PasskeyRename {
        pub id: String,
        pub name: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct TokenAdd {
        pub proof: String,
        pub name: String,
        pub scopes: Vec<String>,
        pub days: u32,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct TokenRemove {
        pub name: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ClientAdd {
        pub proof: String,
        pub name: String,
        pub way: String,
        pub scopes: Vec<String>,
        pub days: u32,
    }
}

/// The parameters `value` as the type `T` says they are.
fn take<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, Fault> {
    serde_json::from_value(value)
        .map_err(|err| Fault::new(Kind::BadRequest, format!("{method} does not take that: {err}")))
}

/// Checks that a call that takes no parameters has none.
fn nothing(value: &Value) -> Result<(), Fault> {
    if value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(());
    }
    Err(Fault::new(Kind::BadRequest, "that call takes no parameters"))
}

/// The bytes of `text`, which the browser wrote as base64url.
fn unbase64(what: &str, text: &str) -> Result<Vec<u8>, Fault> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| Fault::new(Kind::BadRequest, format!("{what} is not base64url")))
}

/// The bytes of a passkey's id, which `steps` lists in hexadecimal.
fn passkey_id(text: &str) -> Result<Vec<u8>, Fault> {
    from_hex(text.as_bytes())
        .ok_or_else(|| Fault::new(Kind::BadRequest, "a passkey's id is hexadecimal"))
}

/// The request to authd that `method` with `params` is, and how its
/// answer is told.
fn request(method: &str, params: Value) -> Result<(Console, Shape), Fault> {
    use takes as p;
    Ok(match method {
        "steps" => (nothing(&params).map(|()| Console::Steps)?, Shape::Steps),
        "trusted-forget" => (nothing(&params).map(|()| Console::TrustedForget)?, Shape::Forgotten),
        "app-begin" => {
            let p: p::AppBegin = take(method, params)?;
            (Console::AppBegin { proof: p.proof.into_bytes(), host: p.host }, Shape::Uri)
        }
        "app-confirm" => {
            let p: p::AppConfirm = take(method, params)?;
            (Console::AppConfirm { code: p.code.into_bytes() }, Shape::Codes)
        }
        "app-remove" => {
            let p: p::Proof = take(method, params)?;
            (Console::AppRemove { proof: p.proof.into_bytes() }, Shape::Done)
        }
        "passkey-begin" => {
            let p: p::PasskeyBegin = take(method, params)?;
            let console = Console::PasskeyBegin {
                proof: p.proof.into_bytes(),
                relying_party: p.relying_party,
            };
            (console, Shape::Creation)
        }
        "passkey-add" => {
            let p: p::PasskeyAdd = take(method, params)?;
            let console = Console::PasskeyAdd {
                challenge: unbase64("the challenge", &p.challenge)?,
                origin: p.origin,
                relying_party: p.relying_party,
                name: p.name,
                client_data: unbase64("the client data", &p.client_data)?,
                attestation: unbase64("the attestation", &p.attestation)?,
            };
            (console, Shape::Codes)
        }
        "passkey-remove" => {
            let p: p::PasskeyRemove = take(method, params)?;
            let console =
                Console::PasskeyRemove { proof: p.proof.into_bytes(), id: passkey_id(&p.id)? };
            (console, Shape::Done)
        }
        "passkey-rename" => {
            let p: p::PasskeyRename = take(method, params)?;
            (Console::PasskeyRename { id: passkey_id(&p.id)?, name: p.name }, Shape::Done)
        }
        "codes" => {
            let p: p::Proof = take(method, params)?;
            (Console::Codes { proof: p.proof.into_bytes() }, Shape::Codes)
        }
        "token-add" => {
            let p: p::TokenAdd = take(method, params)?;
            let console = Console::TokenAdd {
                proof: p.proof.into_bytes(),
                name: p.name,
                scopes: p.scopes,
                days: p.days,
            };
            (console, Shape::Token)
        }
        "token-remove" => {
            let p: p::TokenRemove = take(method, params)?;
            (Console::TokenRemove { name: p.name }, Shape::Done)
        }
        "clients" => (nothing(&params).map(|()| Console::Clients)?, Shape::Steps),
        "client-add" => {
            let p: p::ClientAdd = take(method, params)?;
            let way = Way::of(p.way.as_bytes()).ok_or_else(|| {
                Fault::new(Kind::BadRequest, "an AI client connects over ssh or over https")
            })?;
            let console = Console::ClientAdd {
                proof: p.proof.into_bytes(),
                name: p.name,
                way,
                scopes: p.scopes,
                days: p.days,
            };
            (console, Shape::Client)
        }
        "client-remove" => {
            let p: p::TokenRemove = take(method, params)?;
            (Console::ClientRemove { name: p.name }, Shape::Done)
        }
        other => {
            return Err(Fault::new(Kind::NoRoute, format!("the worker has no method {other}")));
        }
    })
}

/// What admind is told of authd's answer `text`; `who` finds out whose
/// session it is, for the answers that name the person.
fn result(
    shape: &Shape,
    text: &str,
    who: impl FnOnce() -> Result<Who, Fault>,
) -> Result<Value, Fault> {
    let unreadable = |what: &str| {
        Fault::new(Kind::AuthdSilent, format!("authd answered {what} that does not read"))
    };
    Ok(match shape {
        Shape::Steps => serde_json::from_str(text).map_err(|_| unreadable("a list"))?,
        Shape::Uri => json!({ "uri": text }),
        Shape::Codes => json!({ "codes": text.split_whitespace().collect::<Vec<_>>() }),
        Shape::Forgotten => {
            json!({ "forgotten": text.trim().parse::<u32>().map_err(|_| unreadable("a count"))? })
        }
        Shape::Token => json!({ "token": text }),
        Shape::Client => json!({ "token": (!text.is_empty()).then_some(text) }),
        Shape::Done => json!({ "done": true }),
        Shape::Creation => creation(text, &who()?)?,
    })
}

/// What a browser makes a passkey with, from authd's `CHALLENGE HANDLE
/// ID...`, all in hexadecimal: the challenge, the person, and the ids of
/// the passkeys it already has for this machine, in base64url.
fn creation(text: &str, who: &Who) -> Result<Value, Fault> {
    let unreadable =
        || Fault::new(Kind::AuthdSilent, "authd answered a challenge that does not read");
    let mut words = text.split_whitespace();
    let (Some(challenge), Some(handle)) = (words.next(), words.next()) else {
        return Err(unreadable());
    };
    let base64 = |hex: &str| from_hex(hex.as_bytes()).map(|bytes| URL_SAFE_NO_PAD.encode(bytes));
    let exclude: Option<Vec<String>> = words.map(base64).collect();
    let (Some(challenge), Some(handle), Some(exclude)) =
        (base64(challenge), base64(handle), exclude)
    else {
        return Err(unreadable());
    };
    let display = if who.full_name.is_empty() { &who.name } else { &who.full_name };
    Ok(json!({
        "challenge": challenge,
        "user": { "id": handle, "name": who.name, "displayName": display },
        "exclude": exclude,
    }))
}

/// The home folder of the person this worker acts for, which authd gives
/// it as `HOME`.
fn home() -> Result<PathBuf, Fault> {
    preferences::home_of(std::env::var_os("HOME")).map_err(|why| Fault::new(Kind::Internal, why))
}

/// Who this worker acts for, by its own user number.
fn whoami() -> Result<Who, Fault> {
    who::read(&Sources::default(), Account::Number(util::own_user()))
        .map_err(|why| Fault::new(Kind::Refused, why))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(method: &str, params: Value) -> Result<(Console, Shape), Fault> {
        request(method, params)
    }

    fn bad(method: &str, params: Value) -> String {
        let fault = ask(method, params).err().unwrap();
        assert_eq!(fault.id, "ADM-108", "{method}: {}", fault.detail);
        fault.detail
    }

    #[test]
    fn each_method_is_one_request_to_authd() {
        let hello = URL_SAFE_NO_PAD.encode(b"hello");
        let cases: Vec<(&str, Value, Console, Shape)> = vec![
            ("steps", Value::Null, Console::Steps, Shape::Steps),
            ("steps", json!({}), Console::Steps, Shape::Steps),
            ("trusted-forget", Value::Null, Console::TrustedForget, Shape::Forgotten),
            (
                "app-begin",
                json!({"proof": "pw", "host": "duke-nas"}),
                Console::AppBegin { proof: b"pw".to_vec(), host: "duke-nas".into() },
                Shape::Uri,
            ),
            (
                "app-confirm",
                json!({"code": "123456"}),
                Console::AppConfirm { code: b"123456".to_vec() },
                Shape::Codes,
            ),
            (
                "app-remove",
                json!({"proof": ""}),
                Console::AppRemove { proof: Vec::new() },
                Shape::Done,
            ),
            (
                "passkey-begin",
                json!({"proof": "pw", "relying_party": "duke-nas.local"}),
                Console::PasskeyBegin {
                    proof: b"pw".to_vec(),
                    relying_party: "duke-nas.local".into(),
                },
                Shape::Creation,
            ),
            (
                "passkey-add",
                json!({
                    "challenge": hello, "origin": "https://duke-nas.local:9090",
                    "relying_party": "duke-nas.local", "name": "YubiKey",
                    "client_data": hello, "attestation": hello,
                }),
                Console::PasskeyAdd {
                    challenge: b"hello".to_vec(),
                    origin: "https://duke-nas.local:9090".into(),
                    relying_party: "duke-nas.local".into(),
                    name: "YubiKey".into(),
                    client_data: b"hello".to_vec(),
                    attestation: b"hello".to_vec(),
                },
                Shape::Codes,
            ),
            (
                "passkey-remove",
                json!({"proof": "", "id": "0aff"}),
                Console::PasskeyRemove { proof: Vec::new(), id: vec![0x0a, 0xff] },
                Shape::Done,
            ),
            (
                "passkey-rename",
                json!({"id": "0aff", "name": "Desk key"}),
                Console::PasskeyRename { id: vec![0x0a, 0xff], name: "Desk key".into() },
                Shape::Done,
            ),
            (
                "codes",
                json!({"proof": "pw"}),
                Console::Codes { proof: b"pw".to_vec() },
                Shape::Codes,
            ),
            (
                "token-add",
                json!({"proof": "pw", "name": "ansible", "scopes": ["services:read"], "days": 30}),
                Console::TokenAdd {
                    proof: b"pw".to_vec(),
                    name: "ansible".into(),
                    scopes: vec!["services:read".into()],
                    days: 30,
                },
                Shape::Token,
            ),
            (
                "token-remove",
                json!({"name": "ansible"}),
                Console::TokenRemove { name: "ansible".into() },
                Shape::Done,
            ),
            ("clients", Value::Null, Console::Clients, Shape::Steps),
            (
                "client-add",
                json!({"proof": "", "name": "desk", "way": "https", "scopes": ["*:read", "services"],
                       "days": 90}),
                Console::ClientAdd {
                    proof: Vec::new(),
                    name: "desk".into(),
                    way: Way::Https,
                    scopes: vec!["*:read".into(), "services".into()],
                    days: 90,
                },
                Shape::Client,
            ),
            (
                "client-remove",
                json!({"name": "desk"}),
                Console::ClientRemove { name: "desk".into() },
                Shape::Done,
            ),
        ];
        for (method, params, console, shape) in cases {
            let (asked, how) = ask(method, params).unwrap();
            assert!(asked == console, "{method}");
            assert_eq!(how, shape, "{method}");
        }
    }

    #[test]
    fn a_call_that_is_not_what_the_method_takes_is_refused_in_words() {
        assert!(bad("steps", json!({"proof": "pw"})).contains("no parameters"));
        assert!(bad("app-begin", json!({"proof": "pw"})).contains("missing field `host`"));
        assert!(bad("app-confirm", json!({"code": "1", "extra": 1})).contains("unknown field"));
        assert!(bad("app-remove", Value::Null).contains("app-remove does not take that"));
        assert!(
            bad("token-add", json!({"proof": "", "name": "a", "scopes": "*", "days": 1}))
                .contains("expected a sequence")
        );
        let ok = json!({"challenge": "!!", "origin": "o", "relying_party": "r", "name": "n",
                        "client_data": "AA", "attestation": "AA"});
        assert_eq!(bad("passkey-add", ok), "the challenge is not base64url");
        assert_eq!(
            bad("passkey-remove", json!({"proof": "", "id": "xyz"})),
            "a passkey's id is hexadecimal"
        );
        let carried = json!({"proof": "", "name": "a", "way": "carrier", "scopes": [], "days": 1});
        assert_eq!(bad("client-add", carried), "an AI client connects over ssh or over https");
        assert!(bad("client-remove", json!({"name": "a", "of": "bob"})).contains("unknown field"));
        let fault = ask("rm-rf", Value::Null).err().unwrap();
        assert_eq!(
            (fault.id.as_str(), fault.detail.as_str()),
            ("ADM-116", "the worker has no method rm-rf")
        );
    }

    fn who() -> Result<Who, Fault> {
        Ok(Who {
            name: "chase".into(),
            full_name: String::new(),
            admin: true,
            groups: vec![],
            shell: true,
        })
    }

    #[test]
    fn authds_answers_are_told_as_the_page_reads_them() {
        let told = |shape: Shape, text: &str| result(&shape, text, who).unwrap();
        assert_eq!(
            told(Shape::Steps, r#"{"app":null,"needed":false}"#),
            json!({"app": null, "needed": false})
        );
        assert_eq!(
            told(Shape::Uri, "otpauth://totp/x?secret=A"),
            json!({"uri": "otpauth://totp/x?secret=A"})
        );
        assert_eq!(
            told(Shape::Codes, "AAAA-BBBB CCCC-DDDD"),
            json!({"codes": ["AAAA-BBBB", "CCCC-DDDD"]})
        );
        assert_eq!(told(Shape::Codes, ""), json!({"codes": []}));
        assert_eq!(told(Shape::Forgotten, "3"), json!({"forgotten": 3}));
        assert_eq!(told(Shape::Token, "dkv_ansible_ab"), json!({"token": "dkv_ansible_ab"}));
        // A client that connects over SSH has no token: its owner's SSH key lets it in.
        assert_eq!(told(Shape::Client, "dkc_chase_ab"), json!({"token": "dkc_chase_ab"}));
        assert_eq!(told(Shape::Client, ""), json!({"token": null}));
        assert_eq!(told(Shape::Done, ""), json!({"done": true}));
        for (shape, text) in
            [(Shape::Steps, "not json"), (Shape::Forgotten, "many"), (Shape::Creation, "00")]
        {
            let fault = result(&shape, text, who).unwrap_err();
            assert_eq!(fault.id, "ADM-111", "{text}");
        }
    }

    #[test]
    fn a_passkey_is_made_with_the_challenge_the_person_and_the_passkeys_to_leave_out() {
        let who = Who {
            name: "chase".into(),
            full_name: "Chase Y".into(),
            admin: true,
            groups: vec![],
            shell: true,
        };
        let made = creation("0a0b 0c0d 01 ff02", &who).unwrap();
        assert_eq!(
            made,
            json!({
                "challenge": URL_SAFE_NO_PAD.encode([0x0a, 0x0b]),
                "user": {
                    "id": URL_SAFE_NO_PAD.encode([0x0c, 0x0d]),
                    "name": "chase",
                    "displayName": "Chase Y",
                },
                "exclude": [URL_SAFE_NO_PAD.encode([1]), URL_SAFE_NO_PAD.encode([0xff, 2])],
            })
        );
        // Without a full name the account's name is what shows.
        let plain = Who { full_name: String::new(), ..who };
        assert_eq!(creation("0a 0b", &plain).unwrap()["user"]["displayName"], "chase");
        assert!(creation("0a", &plain).is_err());
        assert!(creation("0a 0b zz", &plain).is_err());
    }
}
