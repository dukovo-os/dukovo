// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The browsers that get alerts by Web Push (ADR 0566), from the command
//! line: `alerts push key` gives the machine's public key, which a browser
//! subscribes with, `alerts push add` keeps a browser's subscription, and
//! `alerts push remove` takes one away.
//!
//! alertd keeps the subscriptions, in a file of its own and not in
//! alerts.toml(5), so these go through its socket and change no file.
//! A subscription is a secret: whoever holds its endpoint and keys can
//! send to the browser. It is read from the standard input, never from an
//! argument, and no message here repeats any of it.

use std::io::{self, Read};

use serde::Deserialize;

use super::channels::{kinds_of, severity_of, yes_or_no};
use super::command::{self, WAIT};
use super::protocol::{Added, Browser, Kinds, PushKey, Request, Severity, Subscribe};
use crate::args::{Arg, Args};
use crate::prompt;
use crate::util::{self, Fail, fail, usage};

/// The most bytes of a subscription that are read: a browser's is under a
/// kilobyte, and the push service's address may be 1,024 bytes of that.
const SUBSCRIPTION_MAX: u64 = 4096;

/// `alerts push add`'s settings; the subscription comes on the standard
/// input.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Add {
    pub(super) name: String,
    pub(super) severity: Option<Severity>,
    pub(super) cleared: Option<bool>,
    pub(super) about: Kinds,
}

/// `alerts push`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Push {
    /// The machine's public key; as JSON when `json`.
    Key {
        json: bool,
    },
    Add(Add),
    /// Take away the browser with this ID.
    Remove(String),
}

/// The words after `alerts push`.
pub(super) fn parse(args: &mut Args) -> Result<Push, Fail> {
    match args.subcommand().as_deref().and_then(|name| name.to_str()) {
        Some("key") => Ok(Push::Key { json: command::json_only(args)? }),
        Some("add") => parse_add(args).map(Push::Add),
        Some("remove") => {
            let mut id = None;
            while let Some(arg) = args.next()? {
                match arg {
                    Arg::Operand(operand) if id.is_none() => {
                        id = Some(command::text_of(operand)?);
                    }
                    other => return Err(args.unknown(&other)),
                }
            }
            let id =
                id.ok_or_else(|| usage("an ID is needed, as alerts status lists it".into()))?;
            Ok(Push::Remove(check_id(&id)?))
        }
        _ => Err(usage("alerts push takes key, add or remove; `alerts --help` says how".into())),
    }
}

/// The options of `alerts push add`.
fn parse_add(args: &mut Args) -> Result<Add, Fail> {
    let (mut name, mut severity, mut cleared, mut about) = (None, None, None, Kinds::ALL);
    while let Some(arg) = args.next()? {
        match &arg {
            Arg::Long(option) if option == "name" => name = Some(args.text()?),
            Arg::Long(option) if option == "severity" => {
                severity = Some(severity_of(&args.text()?)?);
            }
            Arg::Long(option) if option == "cleared" => {
                cleared = Some(yes_or_no(&args.text()?)?);
            }
            Arg::Long(option) if option == "about" => about = kinds_of(&args.text()?)?,
            _ => return Err(args.unknown(&arg)),
        }
    }
    let name =
        name.ok_or_else(|| usage("--name NAME is needed: what the browser is called".into()))?;
    if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(usage("--name is 1 to 64 characters, with no control character".into()));
    }
    Ok(Add { name, severity, cleared, about })
}

/// `id`, if it is a browser's ID: 16 hexadecimal digits.
pub(super) fn check_id(id: &str) -> Result<String, Fail> {
    if id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(id.to_ascii_lowercase())
    } else {
        Err(usage(format!(
            "'{id}' is not the ID of a browser: 16 digits, as `alerts status` lists"
        )))
    }
}

/// What a browser's `PushSubscription.toJSON()` holds that alertd keeps.
#[derive(Deserialize)]
struct Given {
    endpoint: String,
    keys: Keys,
}

/// The keys of a browser's subscription, base64url.
#[derive(Deserialize)]
struct Keys {
    p256dh: String,
    auth: String,
}

/// The subscription in `text`: a browser's JSON, `{"endpoint", "keys":
/// {"p256dh", "auth"}}`, which may hold more (`expirationTime`).
///
/// The words of a failure say what is missing and never repeat what was
/// given, which is a secret.
///
/// # Errors
///
/// What is not a subscription.
fn subscription_of(text: &str, add: &Add, by: u32) -> Result<Subscribe, Fail> {
    let given: Given = serde_json::from_str(text.trim()).map_err(|_| {
        fail(
            "the standard input is not a browser's subscription: JSON with an endpoint and keys \
             with p256dh and auth"
                .into(),
        )
    })?;
    Ok(Subscribe {
        name: add.name.clone(),
        endpoint: given.endpoint,
        p256dh: given.keys.p256dh,
        auth: given.keys.auth,
        severity: add.severity,
        cleared: add.cleared,
        about: add.about.words(),
        by,
    })
}

/// The standard input, as text, up to [`SUBSCRIPTION_MAX`] bytes.
fn read_input() -> Result<String, Fail> {
    if prompt::interactive() {
        return Err(usage(
            "the subscription is read from the standard input: JSON a browser made, such as \
             `alerts push add --name NAME < subscription.json`"
                .into(),
        ));
    }
    let mut text = Vec::new();
    io::stdin()
        .lock()
        .take(SUBSCRIPTION_MAX + 1)
        .read_to_end(&mut text)
        .map_err(|error| util::failed(&"standard input", error))?;
    if text.len() as u64 > SUBSCRIPTION_MAX {
        return Err(fail(format!("the standard input is longer than {SUBSCRIPTION_MAX} bytes")));
    }
    String::from_utf8(text).map_err(|_| fail("the standard input is not UTF-8".into()))
}

/// A request to alertd for `ask`.
fn request(ask: &str) -> Request {
    Request { ask: ask.into(), ..Request::default() }
}

/// The one line alertd answers `request` with.
fn answer(request: &Request) -> Result<String, Fail> {
    let mut found = None;
    command::ask(request, WAIT, |line| {
        found.get_or_insert_with(|| line.to_string());
        Ok(())
    })?;
    found.ok_or_else(|| fail("alertd gave no answer".into()))
}

/// `alerts push`.
pub(super) fn run(push: &Push) -> Result<i32, Fail> {
    match push {
        Push::Key { json } => {
            let line = answer(&request("push-key"))?;
            let key: PushKey = serde_json::from_str(&line)
                .map_err(|_| fail("alertd gave a key that is not one".into()))?;
            if *json {
                println!("{}", serde_json::json!({"format": 1, "public_key": key.public_key}));
            } else {
                println!("{}", key.public_key);
            }
            Ok(0)
        }
        Push::Add(add) => {
            let subscribe = subscription_of(&read_input()?, add, util::asker())?;
            let line = answer(&Request { subscribe: Some(subscribe), ..request("push-add") })?;
            let added: Added = serde_json::from_str(&line)
                .map_err(|_| fail("alertd gave an ID that is not one".into()))?;
            println!("{}", serde_json::json!({"format": 1, "id": added.id}));
            Ok(0)
        }
        Push::Remove(id) => {
            command::ask(
                &Request { id: Some(id.clone()), ..request("push-remove") },
                WAIT,
                |_| Ok(()),
            )?;
            println!("alerts: browser {id} removed");
            Ok(0)
        }
    }
}

/// The browsers alertd keeps.
fn listed() -> Result<Vec<Browser>, Fail> {
    command::items(&request("push-list"), WAIT, false)
}

/// Makes `kinds` the kinds the browser `id` is told of, and gives its name.
///
/// # Errors
///
/// A browser there is not, and what alertd refuses.
pub(super) fn set_about(id: &str, kinds: Kinds) -> Result<String, Fail> {
    let name = listed()?
        .into_iter()
        .find(|browser| browser.id == id)
        .map(|browser| browser.name)
        .ok_or_else(|| {
        fail(format!("there is no browser push:{id}; `alerts status` lists them"))
    })?;
    let asked = Request { id: Some(id.into()), about: kinds.words(), ..request("push-about") };
    command::ask(&asked, WAIT, |_| Ok(()))?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::alerts::protocol::About;

    fn args(words: &[&str]) -> Args {
        let mut args = Args::new("alerts", words.iter().map(OsString::from).collect());
        assert_eq!(args.subcommand(), Some(OsString::from("push")));
        args
    }

    fn says(result: Result<Push, Fail>) -> String {
        let fail = result.unwrap_err();
        assert!(fail.usage, "{}", fail.message);
        fail.message
    }

    #[test]
    fn push_takes_key_add_and_remove() {
        assert_eq!(parse(&mut args(&["push", "key"])).unwrap(), Push::Key { json: false });
        assert_eq!(parse(&mut args(&["push", "key", "--json"])).unwrap(), Push::Key { json: true });
        let add = parse(&mut args(&["push", "add", "--name", "Phone"])).unwrap();
        assert_eq!(
            add,
            Push::Add(Add {
                name: "Phone".into(),
                severity: None,
                cleared: None,
                about: Kinds::ALL
            })
        );
        let full = parse(&mut args(&[
            "push",
            "add",
            "--name=Firefox on a phone",
            "--severity",
            "critical",
            "--cleared",
            "no",
            "--about",
            "data,stopped",
        ]))
        .unwrap();
        assert_eq!(
            full,
            Push::Add(Add {
                name: "Firefox on a phone".into(),
                severity: Some(Severity::Critical),
                cleared: Some(false),
                about: Kinds::of(&[About::Data, About::Stopped]),
            })
        );
        assert_eq!(
            parse(&mut args(&["push", "remove", "0123456789ABCDEF"])).unwrap(),
            Push::Remove("0123456789abcdef".into())
        );
    }

    #[test]
    fn a_mistake_in_push_is_a_usage_error() {
        assert!(says(parse(&mut args(&["push"]))).starts_with("alerts push takes"));
        assert!(says(parse(&mut args(&["push", "send"]))).starts_with("alerts push takes"));
        assert_eq!(
            says(parse(&mut args(&["push", "add"]))),
            "--name NAME is needed: what the browser is called"
        );
        assert!(says(parse(&mut args(&["push", "add", "--name", ""]))).contains("1 to 64"));
        let long = "x".repeat(65);
        assert!(says(parse(&mut args(&["push", "add", "--name", &long]))).contains("1 to 64"));
        assert!(says(parse(&mut args(&["push", "add", "--name", "a\nb"]))).contains("1 to 64"));
        assert!(
            says(parse(&mut args(&["push", "add", "--name", "a", "--about", "data,x"])))
                .contains("'x' is no kind of alert")
        );
        assert!(
            says(parse(&mut args(&["push", "add", "--name", "a", "--endpoint", "https://x"])))
                .contains("unrecognized option '--endpoint'")
        );
        assert!(says(parse(&mut args(&["push", "remove"]))).starts_with("an ID is needed"));
        assert!(says(parse(&mut args(&["push", "remove", "12"]))).contains("is not the ID"));
        assert!(says(parse(&mut args(&["push", "key", "now"]))).contains("unexpected argument"));
    }

    #[test]
    fn a_browsers_json_becomes_the_subscription_with_what_the_command_says() {
        let add = Add {
            name: "Phone".into(),
            severity: Some(Severity::Critical),
            cleared: Some(false),
            about: Kinds::of(&[About::Hardware]),
        };
        // What a browser's toJSON() gives, with the member alertd has no use for.
        let text = r#"{"endpoint":"https://push.example.net/send/abc","expirationTime":null,
            "keys":{"p256dh":"BNcR","auth":"tBHI"}}"#;
        let subscribe = subscription_of(text, &add, 1000).unwrap();
        assert_eq!(subscribe.name, "Phone");
        assert_eq!(subscribe.endpoint, "https://push.example.net/send/abc");
        assert_eq!((subscribe.p256dh.as_str(), subscribe.auth.as_str()), ("BNcR", "tBHI"));
        assert_eq!(subscribe.severity, Some(Severity::Critical));
        assert_eq!(subscribe.cleared, Some(false));
        assert_eq!(subscribe.about, Some(vec!["hardware".to_string()]));
        assert_eq!(subscribe.by, 1000);
        let plain = Add { name: "P".into(), severity: None, cleared: None, about: Kinds::ALL };
        assert_eq!(subscription_of(text, &plain, 0).unwrap().about, None);
    }

    #[test]
    fn a_subscription_that_is_not_one_is_refused_without_repeating_it() {
        let add = Add { name: "P".into(), severity: None, cleared: None, about: Kinds::ALL };
        for text in [
            "",
            "not json",
            r#"{"endpoint":"https://secret.example.net/abc"}"#,
            r#"{"endpoint":"https://secret.example.net/abc","keys":{"p256dh":"k"}}"#,
            r#"{"endpoint":7,"keys":{"p256dh":"k","auth":"a"}}"#,
        ] {
            let refused = subscription_of(text, &add, 0).unwrap_err();
            assert!(!refused.usage);
            assert!(
                refused.message.starts_with("the standard input is not a browser's subscription")
            );
            assert!(!refused.message.contains("secret.example.net"), "{}", refused.message);
        }
    }

    #[test]
    fn an_id_is_sixteen_hex_digits() {
        assert_eq!(check_id("00ff00ff00ff00FF").unwrap(), "00ff00ff00ff00ff");
        for wrong in ["", "00ff", "00ff00ff00ff00ff0", "00ff00ff00ff00fg", "../../etc/passwd"] {
            assert!(check_id(wrong).unwrap_err().usage, "{wrong}");
        }
    }
}
