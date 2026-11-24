// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's own actions (ADR 0514, ADR 0516): the settings of admind
//! itself, which live in `/system/settings/admin.toml` (admin.toml(5)).
//! The first is `console.reach`, whom the console answers. It runs `admin
//! reach`, admin(8), as an administrator, as `elevate admin reach any`
//! does at a terminal: the command changes the file, keeping every comment
//! in it, and has Spark reload admind.
//!
//! The area is `console`, the scope a token needs. It is the thing the
//! actions are done to, as `services` and `certificates` are, and it is not
//! a wider name such as `settings`, which a later action of another thing's
//! settings would share: a token that may change the reach may open the
//! console to every address, which no token for something else should
//! carry with it.
//!
//! The reach says whether it changed anything by reading `admin reach`
//! before and after (ADR 0515). With `try` it is tried: the argv ends with
//! `--try SECONDS`, and Spark undoes the change unless `console.keep`
//! keeps it (the trial `console`, which covers all of `admin.toml`), so
//! that a reach that shuts the person out is put back by itself.
//! `console.undo` undoes it at once, and the sign-in rules
//! (`security.sign-in-set`) are in that file and so in that trial.
//!
//! Two reads are in the area too: `console.about`, what the Overview shows
//! of this server, and `console.notices`, what is kept for the next
//! sign-in. They run `admin about` and `admin notices` with `--json` as
//! the person, which print what the person can read of the machine, and
//! `null` for the rest, so they are answered 200 on any machine.
//!
//! The MCP clients of every account are here as well (ADR 0690):
//! `console.clients` lists them and `console.client-remove` takes one
//! away, as `admin mcp list --all` and `admin mcp remove NAME --of OWNER`
//! do inside elevate. A person's own clients are not actions: they are
//! made and removed with the person's password, which goes to authd and
//! never to a command (`routes/account.rs`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::certificate;
use super::trial::{TRY, check_try, try_words};
use super::{
    Action, Args, Bad, Browser, CutsOff, Examples, Field, Nothing, Runs, Tests, Through, Trial,
    Undo,
};
use crate::admin_settings::{self, Reach, Settings};
use crate::authd::console::clients;

/// The area of these actions: the scope a token needs, `console`.
const AREA: &str = "console";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::console::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// The tests of the trial's actions, `console.keep` and `console.undo`,
/// which the Security page's boot and lab tests call.
const TRIAL_TESTS: Tests = Tests {
    checks: "admind::actions::console::tests",
    route: "admin-security",
    same_as_command: "admin-security",
    browser: "web-console/security",
    ansible: "web-console/ansible",
};

/// The trial that covers `admin.toml`, which Spark keeps by this name. It
/// is the area's, as every trial is.
const TRIAL: &str = AREA;

/// `admin about --json`: what the Overview shows of this server.
pub struct About;

impl Action for About {
    const ID: &'static str = "console.about";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/console/about";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show this server";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["admin", "about", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "host": "nas" }), json!({ "all": true })],
            undo: None,
        }
    }
}

/// `admin notices --json`: what is kept for the next sign-in.
pub struct Notices;

impl Action for Notices {
    const ID: &'static str = "console.notices";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/console/notices";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the sign-in notices";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["admin", "notices", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "name": "backup" })], undo: None }
    }
}

/// Whom the console is to answer.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reaching {
    /// `private` or `any`, as admin.toml writes it.
    pub reach: String,
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Args for Reaching {
    const FIELDS: &'static [Field] = &[
        Field::one_of(
            "reach",
            "Whom the console answers: private for machines on private networks, any for every machine.",
            &["private", "any"],
        ),
        TRY,
    ];

    fn check(&self) -> Result<(), Bad> {
        if Reach::parse(&self.reach).is_none() {
            return Err(Bad {
                field: "reach",
                why: "The console's reach is private or any. Nothing was done.".into(),
            });
        }
        check_try(self.seconds)?;
        Ok(())
    }
}

/// The reach the examples use, which is the one admin.toml has when it says
/// nothing. The boot and lab tests run the examples on machines they reach
/// from private addresses, QEMU's user network and the lab PC's, so that
/// the example leaves the console answering them, and a machine that has
/// not been changed is as the example leaves it.
const EXAMPLE: Reach = Reach::Private;

/// `admin reach WORD [--try SECONDS]`: makes admind answer the machines on
/// private networks only, or any machine, and has it read the setting at
/// once, or tries the change for a window. What puts it back is itself,
/// with the reach the console has when admin.toml says nothing.
pub struct SetReach;

impl Action for SetReach {
    const ID: &'static str = "console.reach";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/reach";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ADM-138"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set whom the console answers";
    type Args = Reaching;

    fn argv(args: &Reaching) -> Vec<String> {
        let mut argv = ["admin", "reach", args.reach.as_str()].map(String::from).to_vec();
        argv.extend(try_words(args.seconds));
        argv
    }

    fn subject(args: &Reaching) -> Option<String> {
        Some(args.reach.clone())
    }

    fn files(_: &Reaching) -> Vec<String> {
        vec![admin_settings::PATH.into()]
    }

    /// `admin reach` with no word, which prints the reach the console has.
    fn probe(_: &Reaching) -> Option<Vec<String>> {
        Some(["admin", "reach"].map(String::from).to_vec())
    }

    /// The reach it printed, if it is one.
    fn probed(_: &Reaching, output: &str) -> Option<String> {
        Reach::parse(output.trim()).map(|reach| reach.word().to_string())
    }

    fn trial(args: &Reaching) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    /// The console's reach is already the one asked for when `admin reach`
    /// prints it.
    fn settled(args: &Reaching, now: &str) -> Option<bool> {
        Some(now == args.reach)
    }

    /// `admin reach` writes nothing when admin.toml says the reach already,
    /// and a file with no reach in it, or no file, says `private`. On a
    /// file that admind would refuse, the command fails, which check mode
    /// cannot tell of.
    fn keeps_file(args: &Reaching, _: &str, text: Option<&str>) -> Option<bool> {
        let settings = Settings::parse(text.unwrap_or_default()).ok()?;
        Some(settings.console.reach.word() == args.reach)
    }

    fn examples() -> Examples<Reaching> {
        let word = EXAMPLE.word();
        // The first is the example the tests run and put back with the
        // action itself; the second is the change tried, which Spark
        // undoes after the window, as the first leaves the reach.
        Examples {
            good: vec![
                Reaching { reach: word.into(), seconds: None },
                Reaching { reach: word.into(), seconds: Some(30) },
            ],
            bad: vec![
                json!({ "reach": "everyone" }),
                json!({ "reach": "" }),
                json!({ "reach": "Any" }),
                json!({ "reach": word, "port": 443 }),
                json!({ "reach": word, "try": 5 }),
                json!({}),
            ],
            undo: Some(Undo { action: Self::ID, args: json!({ "reach": word }) }),
        }
    }
}

/// The tests of `console.certificate`, which the Certificates page's boot
/// and lab tests call.
const CERTIFICATE_TESTS: Tests = Tests {
    checks: "admind::actions::console::tests",
    route: "admin-certificates",
    same_as_command: "admin-certificates",
    browser: "web-console/certificates",
    ansible: "web-console/ansible",
};

/// The certificate the console is to serve.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Serving {
    /// The certificate's name, as certd keeps it.
    pub name: String,
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Args for Serving {
    const FIELDS: &'static [Field] = &[certificate::NAME, TRY];

    fn check(&self) -> Result<(), Bad> {
        certificate::check_name(&self.name)?;
        check_try(self.seconds)?;
        Ok(())
    }
}

/// What `cert show NAME --json` prints that the guard reads: the names
/// the certificate is for, DNS names and IP addresses as text.
#[derive(Deserialize)]
struct Shown {
    names: Vec<String>,
}

/// Whether the certificate `shown` describes (what `cert show --json`
/// printed) names the address `browser` uses for the console, so that the
/// browser keeps reaching it without a warning once it is served: Err with
/// the refusal's words when it does not, and when `shown` does not read
/// (ADM-141).
fn names_the_browser(args: &Serving, browser: &Browser<'_>, shown: &str) -> Result<(), String> {
    let unread = || {
        format!(
            "{}'s names could not be read, so the console cannot tell whether this browser's \
             address, {}, is one of them: nothing was done",
            args.name, browser.host
        )
    };
    let shown: Shown = serde_json::from_str(shown).map_err(|_| unread())?;
    if shown.names.iter().any(|name| names(name, browser.host)) {
        return Ok(());
    }
    Err(format!(
        "this browser reaches the console as {}, which {}'s certificate does not name: open the \
         console at an address it names first, then choose it there",
        browser.host, args.name
    ))
}

/// Whether a certificate's `name` covers `host`, as a browser checks it:
/// an IP address is the same address; a DNS name is the same, small and
/// capital letters alike; and a wildcard stands for the left-most word
/// alone, so that `*.example.org` covers `nas.example.org` and neither
/// `example.org` nor `a.nas.example.org`.
fn names(name: &str, host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return name.parse::<std::net::IpAddr>().is_ok_and(|named| named == address);
    }
    if let Some(rest) = name.strip_prefix("*.") {
        return host.split_once('.').is_some_and(|(first, after)| {
            !first.is_empty() && !first.contains('*') && after.eq_ignore_ascii_case(rest)
        });
    }
    !name.contains('*') && name.eq_ignore_ascii_case(host)
}

/// `admin certificate NAME [--try SECONDS]`: makes admind serve
/// certificate NAME, of those certd keeps, to the connections that come
/// after, or tries it for a window, as the trial `console`. Refused when
/// the browser that asks reaches the console by a name the certificate
/// does not have (ADM-141). What puts it back is itself, with the
/// machine's own certificate.
pub struct SetCertificate;

impl Action for SetCertificate {
    const ID: &'static str = "console.certificate";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/certificate";
    const RUNS: Runs = Runs::AsAdministrator;
    // ADM-141 is the guard's, which the API lists from it.
    const ERRORS: &'static [&'static str] = &["ADM-138"];
    const CUTS_OFF: Option<CutsOff<Serving>> = Some(CutsOff {
        on: Through::Name,
        read: |args| ["cert", "show", args.name.as_str(), "--json"].map(String::from).to_vec(),
        cuts: names_the_browser,
    });
    const TESTS: Tests = CERTIFICATE_TESTS;
    const TITLE: &'static str = "Serve a certificate for the console";
    type Args = Serving;

    fn argv(args: &Serving) -> Vec<String> {
        let mut argv = ["admin", "certificate", args.name.as_str()].map(String::from).to_vec();
        argv.extend(try_words(args.seconds));
        argv
    }

    fn subject(args: &Serving) -> Option<String> {
        Some(args.name.clone())
    }

    /// admin.toml, and admind's service file, whose `certificates` has
    /// Spark copy the certificate for it.
    fn files(_: &Serving) -> Vec<String> {
        vec![admin_settings::PATH.into(), crate::servicefiles::path(ADMIND)]
    }

    /// `admin certificate` with no name, which prints the certificate the
    /// console serves.
    fn probe(_: &Serving) -> Option<Vec<String>> {
        Some(["admin", "certificate"].map(String::from).to_vec())
    }

    /// The name it printed, if it is a certificate's.
    fn probed(_: &Serving, output: &str) -> Option<String> {
        let name = output.trim();
        manifest::service::check_certificate(name).is_ok().then(|| name.to_string())
    }

    fn trial(args: &Serving) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    /// The console serves the certificate already when `admin
    /// certificate` prints its name.
    fn settled(args: &Serving, now: &str) -> Option<bool> {
        Some(now == args.name)
    }

    fn examples() -> Examples<Serving> {
        let name = || crate::cert::machine::NAME.to_string();
        // The machine's own certificate, which every image has and the
        // console serves when admin.toml says nothing; the second is the
        // change tried, which Spark undoes after the window.
        Examples {
            good: vec![
                Serving { name: name(), seconds: None },
                Serving { name: name(), seconds: Some(30) },
            ],
            bad: vec![
                json!({ "name": "../x" }),
                json!({ "name": "" }),
                json!({ "name": "This-Machine" }),
                json!({ "name": name(), "try": 5 }),
                json!({ "name": name(), "port": 443 }),
                json!({}),
            ],
            undo: Some(Undo { action: Self::ID, args: json!({ "name": name() }) }),
        }
    }
}

/// The service that serves the console.
const ADMIND: &str = "admind";

/// `admin keep`: keeps the change of `admin.toml` that is being tried.
pub struct Keep;

impl Action for Keep {
    const ID: &'static str = "console.keep";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/trial/keep";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = TRIAL_TESTS;
    const TITLE: &'static str = "Keep the console's settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["admin", "keep"].map(String::from).to_vec()
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        // What puts it back is the change that was tried, which Spark
        // undoes, or `console.undo`.
        Examples { good: vec![Nothing {}], bad: vec![json!({ "id": "x" })], undo: None }
    }
}

/// `admin undo`: undoes the change of `admin.toml` that is being tried.
pub struct Revert;

impl Action for Revert {
    const ID: &'static str = "console.undo";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/trial/undo";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = TRIAL_TESTS;
    const TITLE: &'static str = "Undo the console's settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["admin", "undo"].map(String::from).to_vec()
    }

    fn files(_: &Nothing) -> Vec<String> {
        vec![admin_settings::PATH.into()]
    }

    /// `admin settings --json`: what the file says of the reach and of the
    /// sign-in rules, which the trial covers.
    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(["admin", "settings", "--json"].map(String::from).to_vec())
    }

    /// The three tables the file holds, which the trial covers whole. What
    /// the output says of a mistake in the file (`wrong`) is not a setting.
    fn probed(_: &Nothing, output: &str) -> Option<String> {
        let document: Value = serde_json::from_str(output).ok()?;
        let mut picked = Map::new();
        for key in ["sign_in", "console", "terminal"] {
            picked.insert(key.into(), document.get(key)?.clone());
        }
        Some(Value::Object(picked).to_string())
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "id": "x" })], undo: None }
    }
}

/// The tests of the clients' actions, which the boot and lab tests of the
/// AI clients page call.
const CLIENTS_TESTS: Tests = Tests {
    checks: "admind::actions::console::tests",
    route: "admin-mcp",
    same_as_command: "admin-mcp",
    browser: "web-console/clients",
    ansible: "web-console/ansible",
};

/// `admin mcp list --all --json`: the MCP clients of every account, as
/// authd keeps them: whose each is, how it connects, what it reaches, when
/// it ends and when and from where it was last used. authd keeps no token
/// and no pass as it was given, so the list holds none.
pub struct Clients;

impl Action for Clients {
    const ID: &'static str = "console.clients";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/console/clients";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = CLIENTS_TESTS;
    const TITLE: &'static str = "List every account's AI clients";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["admin", "mcp", "list", "--all", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "owner": "alice" })], undo: None }
    }
}

/// What a client's name matches, as far as a pattern can say it: an API
/// token's letters, and none of the words `admin mcp` reads as what to do.
const CLIENT_PATTERN: &str = "^(?!(add|list|remove|help)$)[a-z0-9-]+$";

/// The most characters of a client's name.
const CLIENT_MAX: usize = 32;

/// A client of an account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientOf {
    /// The account whose client it is.
    pub owner: String,
    /// The client's name, as its owner gave it.
    pub name: String,
}

impl Args for ClientOf {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "owner",
            "The account whose AI client it is, as console.clients lists it.",
            1,
            accounts::NAME_MAX,
            Some(super::accounts::NAME_PATTERN),
        ),
        Field::text(
            "name",
            "The AI client's name, as console.clients lists it: 1 to 32 of a to z, 0 to 9 and -.",
            1,
            CLIENT_MAX,
            Some(CLIENT_PATTERN),
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        if accounts::check_name(&self.owner).is_err() {
            return Err(Bad {
                field: "owner",
                why: format!(
                    "An account's name is 1 to {} lowercase letters, digits, _, - and ., \
                     starting with a letter or _, and is not help. Nothing was removed.",
                    accounts::NAME_MAX
                ),
            });
        }
        if !clients::name_ok(&self.name) {
            return Err(Bad {
                field: "name",
                why: "An AI client's name is 1 to 32 of a to z, 0 to 9 and -, and not add, list, \
                      remove or help. Nothing was removed."
                    .into(),
            });
        }
        Ok(())
    }
}

/// `admin mcp remove NAME --of OWNER`: takes an MCP client from an
/// account, so that its token or its passes let nothing in from the next
/// message on, and what it asked for and nobody answered is refused.
/// authd keeps the state, and nothing puts the client back: its owner
/// makes it again, with a new token.
pub struct ClientRemove;

impl Action for ClientRemove {
    const ID: &'static str = "console.client-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/clients/{owner}/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = CLIENTS_TESTS;
    const TITLE: &'static str = "Remove an AI client of an account";
    type Args = ClientOf;

    fn argv(args: &ClientOf) -> Vec<String> {
        ["admin", "mcp", "remove", args.name.as_str(), "--of", args.owner.as_str()]
            .map(String::from)
            .to_vec()
    }

    fn subject(args: &ClientOf) -> Option<String> {
        Some(format!("{} of {}", args.name, args.owner))
    }

    fn examples() -> Examples<ClientOf> {
        // The client is the boot test's to make: in admin-mcp alice, the
        // administrator of the test image, lets in `lab-read`, and removes
        // it with the command this shows. No lab machine has it, so the lab
        // runs neither this example's task nor its call.
        Examples {
            good: vec![ClientOf { owner: "alice".into(), name: "lab-read".into() }],
            bad: vec![
                json!({ "owner": "alice", "name": "" }),
                json!({ "owner": "alice", "name": "Lab Client" }),
                json!({ "owner": "alice", "name": "list" }),
                json!({ "owner": "a b", "name": "lab-read" }),
                json!({ "name": "lab-read" }),
                json!({ "owner": "alice" }),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::super::tests::validate;
    use super::super::{Browser, Through};
    use super::super::{REGISTRY, Registered};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::reach;
    use crate::admind::scopes::Scopes;

    /// The action, which is in the registry.
    fn registered() -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == "console.reach").unwrap()
    }

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    #[test]
    fn about_and_notices_are_reads_of_the_person_that_ask_the_admin_command_for_json() {
        for (id, path, word, title) in [
            ("console.about", "/api/1/console/about", "about", "Show this server"),
            ("console.notices", "/api/1/console/notices", "notices", "Show the sign-in notices"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.long()),
                (false, Runs::AsThePerson, "console", false),
                "{id}"
            );
            assert_eq!((action.path(), action.title()), (path, title));
            assert!(action.errors().is_empty(), "{id}");
            assert!(!action.compares(), "{id}");
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, ["admin", word, "--json"], "{id}");
            assert_eq!(prepared.probe, None);
            assert!(prepared.files.is_empty());
            assert_eq!(prepared.ansible.name, title);
            assert_eq!(prepared.ansible.module, "ansible.builtin.command");
            assert_eq!(prepared.ansible.params["argv"], json!(["admin", word, "--json"]));
            assert!(!prepared.ansible.become_root);
            assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        }
    }

    #[test]
    fn about_and_notices_take_nothing() {
        for id in ["console.about", "console.notices"] {
            let (good, bad, undo) = action(id).example_values();
            assert_eq!((good, undo), (vec![json!({})], None), "{id}");
            for args in bad {
                let failure = action(id).prepare(args.clone()).unwrap_err();
                assert_eq!(failure.kind, Kind::BadRequest, "{id} {args}");
            }
            assert_eq!(action(id).fields().len(), 0);
        }
    }

    #[test]
    fn a_token_reads_them_with_the_console_scope_or_its_read_scope() {
        for id in ["console.about", "console.notices"] {
            let call = |list: &str| {
                let scopes = Scopes::parse(list).unwrap();
                let caller = super::super::Caller::Token { name: "ci", scopes: &scopes };
                super::super::authorize(&caller, action(id))
            };
            for list in ["console", "console:read", "*"] {
                assert!(call(list).is_ok(), "{id} {list}");
            }
            for list in ["services", "metrics:read", "security"] {
                let refused = call(list).unwrap_err();
                assert_eq!(refused.kind, Kind::OutOfScope, "{id} {list}");
                assert_eq!(
                    refused.detail,
                    "the API token \"ci\" may not do this: it needs the scope console:read, \
                     console or *"
                );
            }
        }
    }

    #[test]
    fn the_command_is_admin_reach_with_the_word_after_it() {
        for word in ["private", "any"] {
            let prepared = registered().prepare(json!({ "reach": word })).unwrap();
            assert_eq!(prepared.argv, ["admin", "reach", word]);
            assert_eq!(prepared.args, json!({ "reach": word }));
        }
    }

    #[test]
    fn with_a_window_the_change_is_tried_and_its_command_ends_with_the_seconds() {
        let action = registered();
        for word in ["private", "any"] {
            let prepared = action.prepare(json!({ "reach": word, "try": 30 })).unwrap();
            assert_eq!(prepared.argv, ["admin", "reach", word, "--try", "30"]);
            assert_eq!(prepared.args, json!({ "reach": word, "try": 30 }));
            assert_eq!(prepared.trial, Some(Trial::Begins("console")));
            assert_eq!(
                prepared.ansible.params["argv"],
                json!(["admin", "reach", word, "--try", "30"])
            );
            // At once, it has no window and no trial.
            let at_once = action.prepare(json!({ "reach": word })).unwrap();
            assert_eq!((at_once.argv.len(), at_once.trial), (3, None));
        }
        let range = spark_api::trial::SECONDS;
        for seconds in [*range.start(), *range.end()] {
            assert!(action.prepare(json!({ "reach": "any", "try": seconds })).is_ok());
        }
        // The window is Spark's, and the refusal names the field.
        for seconds in [json!(5), json!(9), json!(3601), json!(0), json!(-30)] {
            let failure = action.prepare(json!({ "reach": "any", "try": seconds })).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("try")), "{seconds}");
            assert_eq!(
                failure.detail,
                "A change is tried for 10 to 3600 seconds. Nothing was done."
            );
        }
        // The probe is the same with or without a window.
        let tried = action.prepare(json!({ "reach": "any", "try": 30 })).unwrap();
        assert_eq!(tried.probe.as_deref(), Some(&["admin".into(), "reach".into()][..]));
    }

    #[test]
    fn a_reach_is_private_or_any_and_the_refusal_names_the_field() {
        let words = "The console's reach is private or any. Nothing was done.";
        for reach in ["everyone", "", "Any", "PRIVATE", " any", "any ", "private\n", "0", "*"] {
            let failure = registered().prepare(json!({ "reach": reach })).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("reach")), "{reach:?}");
            assert_eq!(failure.detail, words, "{reach:?}");
        }
    }

    #[test]
    fn the_probe_reads_the_reach_the_console_has_and_compares_the_word() {
        let action = registered();
        assert!(action.compares());
        for word in ["private", "any"] {
            let prepared = action.prepare(json!({ "reach": word })).unwrap();
            assert_eq!(prepared.probe.as_deref(), Some(&["admin".into(), "reach".into()][..]));
            // `admin reach` prints the word and a line end.
            let probed = |output: &str| action.probed(&prepared.args, output);
            assert_eq!(probed("private\n").as_deref(), Some("private"));
            assert_eq!(probed("any\n").as_deref(), Some("any"));
            assert_eq!(probed(" any \n\n").as_deref(), Some("any"));
        }
        // Anything else is not a reach, and the change then says it changed.
        let args = json!({ "reach": "any" });
        for output in ["", "\n", "Any\n", "everyone", "any private", "The reach is any.\n"] {
            assert_eq!(action.probed(&args, output), None, "{output:?}");
        }
        // The reach asked for has no part in what the probe picks.
        assert_eq!(action.probed(&json!({ "reach": "private" }), "any\n").as_deref(), Some("any"));
    }

    #[test]
    fn arguments_the_action_does_not_take_do_not_read() {
        for args in [
            json!({}),
            json!({ "reach": "any", "port": 443 }),
            json!({ "reach": true }),
            json!({ "reach": ["any"] }),
            json!({ "reach": null }),
            json!({ "reach": "any", "try": "30" }),
            json!({ "reach": "any", "try": 1.5 }),
        ] {
            let failure = registered().prepare(args.clone()).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn it_changes_the_settings_file_as_an_administrator_and_is_a_task_for_nobody() {
        let action = registered();
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (true, Runs::AsAdministrator, "console", false)
        );
        assert_eq!(action.path(), "/api/1/console/reach");
        assert_eq!(action.errors(), ["ADM-138"], "a console that is being tried cannot be changed");
        let prepared = action.prepare(json!({ "reach": "any" })).unwrap();
        assert_eq!(prepared.files, ["/system/settings/admin.toml"]);
        assert_eq!(prepared.ansible.name, "Set whom the console answers: any");
        assert_eq!(prepared.ansible.module, "ansible.builtin.command");
        assert_eq!(prepared.ansible.params["argv"], json!(["admin", "reach", "any"]));
        assert!(prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when, None);
    }

    #[test]
    fn a_token_needs_the_console_scope_to_change_it_and_may_read_nothing_of_it() {
        let call = |list: &str| {
            let scopes = Scopes::parse(list).unwrap();
            let caller = super::super::Caller::Token { name: "ci", scopes: &scopes };
            super::super::authorize(&caller, registered())
        };
        assert!(call("console").is_ok());
        assert!(call("*").is_ok());
        for list in ["console:read", "services", "certificates,services:read"] {
            let refused = call(list).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{list}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope console or *"
            );
        }
    }

    #[test]
    fn the_example_is_the_default_and_leaves_the_machines_the_tests_run_from_answered() {
        let (good, bad, undo) = registered().example_values();
        assert_eq!(good, [json!({ "reach": "private" }), json!({ "reach": "private", "try": 30 })]);
        assert_eq!(Reach::parse("private"), Some(Reach::default()));
        // The boot test's guest is reached from QEMU's user network, and
        // the lab's machine from the lab PC, which is on a private
        // network: neither is an address that the example shuts out.
        let from = ["10.0.2.2", "192.168.1.20", "172.16.0.9", "127.0.0.1"];
        for address in from {
            let ip: IpAddr = address.parse().unwrap();
            assert!(reach::answers(EXAMPLE, ip, &[]), "{address}");
        }
        assert!(!reach::answers(EXAMPLE, "203.0.113.9".parse().unwrap(), &[]));
        // Nothing but itself puts it back: it sets the reach to a word, and
        // the word of the example is the one the machine starts with.
        assert_eq!(
            undo,
            Some(Undo { action: "console.reach", args: json!({ "reach": "private" }) })
        );
        for args in bad {
            assert!(registered().prepare(args.clone()).is_err(), "{args}");
        }
        assert_eq!(
            registered().prepare(good[0].clone()).unwrap().argv,
            ["admin", "reach", "private"]
        );
    }

    #[test]
    fn keep_and_undo_end_the_trial_as_an_administrator_with_nothing_to_say() {
        for (id, path, word, title) in [
            ("console.keep", "/api/1/console/trial/keep", "keep", "Keep the console's settings"),
            ("console.undo", "/api/1/console/trial/undo", "undo", "Undo the console's settings"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.long()),
                (true, Runs::AsAdministrator, "console", false),
                "{id}"
            );
            assert_eq!((action.path(), action.title()), (path, title), "{id}");
            assert_eq!(action.errors(), ["NET-179"], "{id}");
            assert!(action.posts(), "{id}");
            assert_eq!(action.fields().len(), 0, "{id}");
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, ["admin", word], "{id}");
            assert_eq!(prepared.trial, Some(Trial::Ends("console")), "{id}");
            assert_eq!(prepared.ansible.name, title);
            assert!(prepared.ansible.become_root);
            assert_eq!(
                (action.tests().route, action.tests().browser),
                ("admin-security", "web-console/security")
            );
            let (good, bad, undo) = action.example_values();
            assert_eq!((good, undo), (vec![json!({})], None), "{id}");
            for args in bad {
                assert_eq!(
                    action.prepare(args.clone()).unwrap_err().kind,
                    Kind::BadRequest,
                    "{id}"
                );
            }
        }
    }

    #[test]
    fn undo_names_the_settings_file_and_compares_what_the_file_says() {
        let action = action("console.undo");
        assert!(action.compares());
        assert!(!self::action("console.keep").compares());
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.files, ["/system/settings/admin.toml"]);
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["admin".to_string(), "settings".into(), "--json".into()][..])
        );
        let printed = |hours: u32, wrong: &str| {
            format!(
                r#"{{"format":1,"sign_in":{{"second_step":"if-set","session_hours":{hours},"idle_minutes":30}},"console":{{"listen":9090,"reach":"private","certificate":"c"}},"terminal":{{"allowed":"nobody","sessions_per_account":4}},"wrong":{wrong}}}"#
            )
        };
        let picked = |output: &str| action.probed(&prepared.args, output);
        assert_ne!(picked(&printed(12, "null")), picked(&printed(8, "null")));
        assert_eq!(picked(&printed(12, "null")), picked(&printed(12, r#""x""#)));
        assert!(picked(&printed(12, "null")).is_some());
        for output in ["", "{}", "[]", r#"{"sign_in":{}}"#, "not json"] {
            assert_eq!(picked(output), None, "{output:?}");
        }
    }

    #[test]
    fn a_check_of_the_reach_reads_what_admin_reach_prints_and_admin_toml() {
        let reach = registered();
        let any = json!({ "reach": "any" });
        let private = json!({ "reach": "private" });
        assert_eq!(reach.settled(&any, "any"), Some(true));
        assert_eq!(reach.settled(&any, "private"), Some(false));
        let path = admin_settings::PATH;
        // No file, and a file with no reach, say private.
        for text in [None, Some(""), Some("# the console\n[console]\nlisten = 9090\n")] {
            assert_eq!(reach.keeps_file(&private, path, text), Some(true), "{text:?}");
            assert_eq!(reach.keeps_file(&any, path, text), Some(false), "{text:?}");
        }
        let said = Some("[console]\nreach = \"any\"\n");
        assert_eq!(reach.keeps_file(&any, path, said), Some(true));
        assert_eq!(reach.keeps_file(&private, path, said), Some(false));
        // A file admind refuses fails the command: nothing to tell.
        assert_eq!(reach.keeps_file(&any, path, Some("[console\n")), None);
    }

    #[test]
    fn a_certificate_covers_its_own_names_an_address_as_itself_and_a_wildcard_one_word_deep() {
        // A DNS name, small and capital letters alike, and a final dot.
        assert!(names("nas.example.org", "nas.example.org"));
        assert!(names("NAS.Example.org", "nas.example.org"));
        assert!(names("nas.example.org", "nas.example.org."));
        assert!(!names("nas.example.org", "nas.example"));
        assert!(!names("nas.example.org", "a.nas.example.org"));
        // A wildcard stands for the left-most word alone.
        assert!(names("*.example.org", "nas.example.org"));
        assert!(!names("*.example.org", "example.org"));
        assert!(!names("*.example.org", "a.nas.example.org"));
        assert!(!names("*.example.org", ".example.org"));
        assert!(!names("nas.*.org", "nas.example.org"));
        assert!(!names("*", "nas"));
        // An address is the same address, however it is written; a name
        // never covers an address, nor a wildcard one.
        assert!(names("192.168.1.10", "192.168.1.10"));
        assert!(names("2001:db8::1", "2001:db8:0:0::1"));
        assert!(!names("192.168.1.10", "192.168.1.11"));
        assert!(!names("*.168.1.10", "192.168.1.10"));
        assert!(!names("nas.example.org", "192.168.1.10"));
    }

    #[test]
    fn the_console_s_certificate_is_refused_for_a_browser_at_a_name_it_does_not_have() {
        let serve = action("console.certificate");
        let prepared = serve.prepare(json!({ "name": "nas.example.org", "try": 300 })).unwrap();
        let read = prepared.cuts_off.as_ref().unwrap();
        assert_eq!(read.on, Through::Name);
        assert_eq!(read.argv, ["cert", "show", "nas.example.org", "--json"]);
        let shown = r#"{"name":"nas.example.org","names":["nas.example.org","*.lan.example.org","192.168.1.10"],"trusted":true}"#;
        let from: IpAddr = "192.168.1.20".parse().unwrap();
        let guard = |host: &str, shown: &str| {
            serve.cuts_off(&prepared.args, &Browser { from, host }, shown)
        };
        for host in ["nas.example.org", "files.lan.example.org", "192.168.1.10"] {
            assert_eq!(guard(host, shown), Ok(()), "{host}");
        }
        assert_eq!(
            guard("192.168.1.11", shown).unwrap_err(),
            "this browser reaches the console as 192.168.1.11, which nas.example.org's \
             certificate does not name: open the console at an address it names first, then \
             choose it there"
        );
        assert!(guard("nas.local", shown).is_err());
        assert!(guard("lan.example.org", shown).is_err());
        // What does not read refuses: the guard fails closed.
        for unread in ["", "{}", r#"{"names":"nas.example.org"}"#, "not json"] {
            let refused = guard("nas.example.org", unread).unwrap_err();
            assert!(refused.contains("could not be read"), "{unread:?}: {refused}");
            assert!(refused.contains("nas.example.org"), "{refused}");
        }
        // The guard is listed as one of any request.
        let listed = super::super::listed(&[serve]);
        assert_eq!(listed[0]["cuts_off"], json!("*"));
    }

    #[test]
    fn the_console_s_certificate_is_admin_certificate_with_both_files_tried_as_the_console() {
        let serve = action("console.certificate");
        let tried = serve.prepare(json!({ "name": "nas.example.org", "try": 300 })).unwrap();
        assert_eq!(tried.argv, ["admin", "certificate", "nas.example.org", "--try", "300"]);
        assert_eq!(
            tried.files,
            ["/system/settings/admin.toml", "/system/settings/spark/services/admind.toml"]
        );
        assert_eq!(tried.trial, Some(Trial::Begins("console")));
        let at_once = serve.prepare(json!({ "name": "this-machine" })).unwrap();
        assert_eq!(at_once.argv, ["admin", "certificate", "this-machine"]);
        assert_eq!(at_once.trial, None);
        assert_eq!(
            at_once.probe.as_deref(),
            Some(&["admin".to_string(), "certificate".into()][..])
        );
        assert_eq!(
            serve.probed(&at_once.args, "nas.example.org\n").as_deref(),
            Some("nas.example.org")
        );
        assert_eq!(serve.probed(&at_once.args, "../key\n"), None);
        assert_eq!(serve.settled(&at_once.args, "this-machine"), Some(true));
        assert_eq!(serve.settled(&at_once.args, "nas.example.org"), Some(false));
        assert_eq!(serve.errors(), ["ADM-138"]);
        assert!(serve.changes() && serve.runs() == Runs::AsAdministrator);
        let refused = serve.prepare(json!({ "name": "../key" })).unwrap_err();
        assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("name")));
    }

    #[test]
    fn every_account_s_clients_are_a_read_an_administrator_runs_that_takes_nothing() {
        let list = action("console.clients");
        assert_eq!(
            (list.changes(), list.runs(), list.area(), list.long()),
            (false, Runs::AsAdministrator, "console", false)
        );
        assert_eq!((list.path(), list.posts()), ("/api/1/console/clients", false));
        assert_eq!(list.title(), "List every account's AI clients");
        assert!(list.errors().is_empty() && !list.compares() && !list.secret_answer());
        assert_eq!(list.fields().len(), 0);
        let prepared = list.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["admin", "mcp", "list", "--all", "--json"]);
        assert_eq!((prepared.probe, prepared.files.len()), (None, 0));
        // The command is authd's to answer inside elevate, so the task
        // becomes root, and a read's task says it changed nothing.
        assert!(prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        assert_eq!(prepared.ansible.params["argv"], json!(prepared.argv));
        let (good, bad, undo) = list.example_values();
        assert_eq!((good, undo), (vec![json!({})], None));
        for args in bad {
            assert_eq!(list.prepare(args.clone()).unwrap_err().kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn a_client_is_removed_by_its_name_and_its_owner_s_which_the_address_gives() {
        let remove = action("console.client-remove");
        assert_eq!(
            (remove.changes(), remove.runs(), remove.area(), remove.long()),
            (true, Runs::AsAdministrator, "console", false)
        );
        assert_eq!(remove.path(), "/api/1/console/clients/{owner}/{name}/remove");
        assert_eq!(remove.title(), "Remove an AI client of an account");
        // A client that is not there is the command's to say, as ADM-123,
        // and nothing tells whether a removal moved anything but that.
        assert!(remove.errors().is_empty() && !remove.compares());
        let prepared = remove.prepare(json!({ "owner": "ann", "name": "desk" })).unwrap();
        assert_eq!(prepared.argv, ["admin", "mcp", "remove", "desk", "--of", "ann"]);
        assert_eq!((prepared.probe, prepared.trial), (None, None));
        // authd keeps the clients, and no file of the settings is written.
        assert!(prepared.files.is_empty());
        assert_eq!(prepared.ansible.name, "Remove an AI client of an account: desk of ann");
        assert!(prepared.ansible.become_root);
        // The address names both, and its other method is no route.
        let path = "/api/1/console/clients/ann/desk/remove";
        match super::super::find(REGISTRY, true, path) {
            super::super::Found::Action(found, params) => {
                assert_eq!(found.id(), "console.client-remove");
                assert_eq!(Value::Object(params), json!({ "owner": "ann", "name": "desk" }));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            super::super::find(REGISTRY, false, path),
            super::super::Found::WrongMethod
        ));
        match super::super::find(REGISTRY, false, "/api/1/console/clients") {
            super::super::Found::Action(found, _) => assert_eq!(found.id(), "console.clients"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_client_s_name_and_its_owner_s_are_what_authd_takes_and_the_refusal_names_the_field() {
        let refused = |owner: &str, name: &str| {
            let failure = action("console.client-remove")
                .prepare(json!({ "owner": owner, "name": name }))
                .unwrap_err();
            (failure.kind, failure.field, failure.detail)
        };
        let schema = super::super::schema(ClientOf::FIELDS);
        let fits = |owner: &str, name: &str| {
            validate::validate(&schema, &json!({ "owner": owner, "name": name })).is_ok()
        };
        for name in ["desk", "ci-2", "0", "added", "lists", &"a".repeat(32)] {
            assert!(clients::name_ok(name) && fits("ann", name), "{name}");
            assert!(
                action("console.client-remove")
                    .prepare(json!({ "owner": "ann", "name": name }))
                    .is_ok()
            );
        }
        // The words `admin mcp` reads as what to do would not be a
        // client's name in the command, and no client has one.
        for name in ["", "Desk", "a b", "desk_1", "add", "list", "remove", "help", &"a".repeat(33)]
        {
            assert!(!clients::name_ok(name) && !fits("ann", name), "{name:?}");
            let (kind, field, why) = refused("ann", name);
            assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{name:?}");
            assert_eq!(
                why,
                "An AI client's name is 1 to 32 of a to z, 0 to 9 and -, and not add, list, \
                 remove or help. Nothing was removed."
            );
        }
        // An option's look in the owner's place is no account's name, so
        // the command is given none.
        for owner in ["", "Ann", "a b", "--all", "-x", "help", "9lives", "a/b"] {
            assert!(!fits(owner, "desk"), "{owner:?}");
            let (kind, field, why) = refused(owner, "desk");
            assert_eq!((kind, field), (Kind::BadValue, Some("owner")), "{owner:?}");
            assert!(why.starts_with("An account's name is 1 to "), "{why}");
            assert!(why.ends_with("Nothing was removed."), "{why}");
        }
        for args in [json!({ "name": "desk" }), json!({ "owner": "ann" }), json!({})] {
            let failure = action("console.client-remove").prepare(args.clone()).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{args}");
        }
        let more = json!({ "owner": "ann", "name": "desk", "all": true });
        assert_eq!(
            action("console.client-remove").prepare(more).unwrap_err().kind,
            Kind::BadRequest
        );
    }

    #[test]
    fn the_clients_of_every_account_need_the_whole_console_scope_and_an_administrator_unlocked() {
        for id in ["console.clients", "console.client-remove"] {
            let token = |list: &str| {
                let scopes = Scopes::parse(list).unwrap();
                let caller = super::super::Caller::Token { name: "ci", scopes: &scopes };
                super::super::authorize(&caller, action(id)).map_err(|refused| refused.kind)
            };
            assert_eq!(token("console"), Ok(()), "{id}");
            assert_eq!(token("*"), Ok(()), "{id}");
            // The list is a read an administrator runs, which the read
            // scope does not give.
            for list in ["console:read", "accounts", "security,services"] {
                assert_eq!(token(list), Err(Kind::OutOfScope), "{id} {list}");
            }
            let session = |admin: bool, unlocked: bool| {
                let caller = super::super::Caller::Session { name: "ann", admin, unlocked };
                super::super::authorize(&caller, action(id)).map_err(|refused| refused.kind)
            };
            assert_eq!(session(false, true), Err(Kind::NotAdmin), "{id}");
            assert_eq!(session(true, false), Err(Kind::Locked), "{id}");
            assert_eq!(session(true, true), Ok(()), "{id}");
        }
    }

    #[test]
    fn the_example_removes_the_client_the_boot_test_has_its_administrator_let_in() {
        let (good, bad, undo) = action("console.client-remove").example_values();
        assert_eq!(good, [json!({ "owner": "alice", "name": "lab-read" })]);
        // Nothing puts a client back: its owner makes it again, with the
        // password, which no action takes.
        assert_eq!(undo, None);
        for args in bad {
            assert!(action("console.client-remove").prepare(args.clone()).is_err(), "{args}");
        }
        let both = [CLIENTS_TESTS.route, CLIENTS_TESTS.same_as_command];
        assert_eq!(both, ["admin-mcp", "admin-mcp"]);
        for id in ["console.clients", "console.client-remove"] {
            assert_eq!(action(id).tests().browser, "web-console/clients", "{id}");
        }
    }
}
