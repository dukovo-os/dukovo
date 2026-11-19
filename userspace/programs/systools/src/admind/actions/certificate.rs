// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The certificates' actions (ADR 0515): the list, one certificate,
//! renewing one, what the checks before an order find, getting one from an
//! authority as a task, adding one the person made, revoking, removing,
//! what certd and `cert` did, and the certificates certd asks for again and
//! stopping that. Each runs `cert`, cert(8), which reads
//! the store certd keeps; a change runs as an administrator, as `elevate
//! cert renew NAME` does at a terminal.
//!
//! A certificate's name is checked by the rule certd and `cert` check it
//! by, which is also the rule a service file's `certificates` follows. The
//! names an order is for are checked by the rule `cert get` has for them,
//! so that what `cert` would refuse is refused before it runs.
//!
//! A renewal has no probe (ADR 0515): `cert renew` makes the certificate
//! anew whenever it is asked to, due or not, so it always changed
//! something and a comparison could only say so. The store is certd's, not
//! a file admind sees, so no change here names a file; getting and adding
//! compare what `cert` shows of the certificate before and after.
//!
//! A certificate's key and chain are one document a command reads on its
//! input (ADR 0567), more than a pipe holds: its field is a secret, so it
//! is never in the journal, the audit, a task's row or the shown command.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::secret::Secret;
use super::trial::refusal;
use super::{Action, Args, Bad, DOCUMENT_MAX, Examples, Field, Nothing, Runs, Tests, Undo};
use crate::cert::acme;
use crate::cert::machine;
use manifest::service::{CERTIFICATE_NAME_MAX, check_certificate};

/// The area of these actions: the scope a token needs, `certificates`.
const AREA: &str = "certificates";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::certificate::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// The tests of the reads that the Certificates page's list and panel
/// make: the boot tests are the generic ones, and the lab test is the
/// page's, which holds each to `cert list` and `cert show`.
const LISTED_TESTS: Tests = Tests { browser: "web-console/certificates", ..TESTS };

/// The tests of the actions that came with the Certificates page: the
/// boot test and the lab test of the page, which `lab/actions.toml` names.
const PAGE_TESTS: Tests = Tests {
    checks: "admind::actions::certificate::tests",
    route: "admin-certificates",
    same_as_command: "admin-certificates",
    browser: "web-console/certificates",
    ansible: "web-console/ansible",
};

/// What a certificate's name matches, as `check` checks it, apart from its
/// length: a lowercase letter or a digit, then more of them, and hyphens
/// and dots, with no two dots in a row.
pub(super) const NAME_PATTERN: &str = "^(?!.*\\.\\.)[a-z0-9][a-z0-9.-]*$";

/// A certificate, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    /// The certificate's name.
    pub name: String,
}

/// The field of a certificate's name, which `console.certificate` takes
/// too.
pub(super) const NAME: Field = Field::text(
    "name",
    "The certificate's name, as cert list shows it: this-machine, or a domain's name.",
    1,
    CERTIFICATE_NAME_MAX,
    Some(NAME_PATTERN),
);

/// Whether `name` is a certificate's name, as [`NAME`] checks it.
pub(super) fn check_name(name: &str) -> Result<(), Bad> {
    check_name_of("name", name)
}

/// [`check_name`] for the field `field`, which holds a certificate's name.
pub(super) fn check_name_of(field: &'static str, name: &str) -> Result<(), Bad> {
    if check_certificate(name).is_ok() {
        return Ok(());
    }
    Err(Bad {
        field,
        why: format!(
            "A certificate's name has 1 to {CERTIFICATE_NAME_MAX} lowercase letters, digits, \
             hyphens and dots, starts with a letter or a digit, and has no two dots in a row. \
             Nothing was done."
        ),
    })
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[NAME];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)
    }
}

/// The machine's own certificate, which every image has: the examples use
/// it.
fn example() -> Named {
    Named { name: machine::NAME.into() }
}

/// Arguments no certificate action that takes a name takes: an empty name,
/// capitals, a path, and a field it has none of.
fn bad_names() -> Vec<serde_json::Value> {
    vec![
        json!({ "name": "" }),
        json!({ "name": "This-Machine" }),
        json!({ "name": "../etc/passwd" }),
        json!({ "name": machine::NAME, "force": true }),
    ]
}

/// `cert list --json`: every certificate.
pub struct List;

impl Action for List {
    const ID: &'static str = "certificate.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = LISTED_TESTS;
    const TITLE: &'static str = "List the certificates";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["cert", "list", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": machine::NAME }), json!({ "expired": true })],
            undo: None,
        }
    }
}

/// `cert show NAME --json`: one certificate, what it holds and whether it
/// can be trusted.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "certificate.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates/{name}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = LISTED_TESTS;
    const TITLE: &'static str = "Show a certificate";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["cert", "show", args.name.as_str(), "--json"].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples { good: vec![example()], bad: bad_names(), undo: None }
    }
}

/// `cert renew NAME`: makes the machine's own certificate anew, or asks
/// the authority for a certificate that came from one again. Nothing puts
/// back what it did.
pub struct Renew;

impl Action for Renew {
    const ID: &'static str = "certificate.renew";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/renew";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Renew a certificate";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["cert", "renew", args.name.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples { good: vec![example()], bad: bad_names(), undo: None }
    }
}

/// How many names a request for a certificate takes, at most.
const NAMES_MOST: usize = 20;

/// The most characters of the names a request lists, which is free text in a
/// command's words.
const NAMES_CHARS: usize = 2_000;

/// What a list of names with a comma between them is made of: the letters,
/// digits and signs a name has, in 1 to [`NAMES_MOST`] names. That each is a
/// name `cert` takes is the rule that only `check` says.
const COMMA_NAMES: &str = "^[a-z0-9*.-]+(,[a-z0-9*.-]+){0,19}$";

/// [`COMMA_NAMES`] with a line end between the names.
const LINE_NAMES: &str = "^[a-z0-9*.-]+(\n[a-z0-9*.-]+){0,19}$";

/// What an address for an authority's account is made of: something, an
/// `@`, something, and no space or line end.
const EMAIL_PATTERN: &str = "^[^@ \t\r\n]+@[^@ \t\r\n]+$";

/// The most bytes of an address (RFC 5321).
const EMAIL_MAX: usize = 254;

/// The reasons `cert revoke` takes, as it says them.
const REASONS: [&str; 4] =
    ["unspecified", "key-compromise", "superseded", "cessation-of-operation"];

/// The words of a refusal of the names of an order, and which field.
fn bad_names_of(why: &str) -> Bad {
    Bad { field: "names", why: refusal(why) }
}

/// The names `text` lists, `separator` between them, checked as an order or
/// a check takes them: 1 to [`NAMES_MOST`] in 1 to [`NAMES_CHARS`]
/// characters, each as `cert get` reads a name (`to_check` lets a name of
/// one label through), none twice.
fn read_names(text: &str, separator: char, dns: bool, to_check: bool) -> Result<Vec<String>, Bad> {
    if text.chars().count() > NAMES_CHARS {
        return Err(bad_names_of(&format!("the names are {NAMES_CHARS} characters at most")));
    }
    let names: Vec<String> = text.split(separator).map(String::from).collect();
    if names.len() > NAMES_MOST {
        return Err(bad_names_of(&format!("a request takes 1 to {NAMES_MOST} names")));
    }
    let checked = if to_check {
        acme::check_names_to_check(&names, dns)
    } else {
        acme::check_names(&names, dns)
    };
    checked.map_err(|why| bad_names_of(&why))?;
    Ok(names)
}

/// What `certificate.check` is asked about.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Checking {
    /// The DNS names, with a comma between them.
    pub names: String,
    /// The certificate's name, when it is not the first name.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
    /// Whether the names are proved through DNS, not on port 80.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dns: bool,
}

impl Checking {
    /// The certificate's name: the one given, or the first name.
    fn certificate_name(&self) -> &str {
        self.certificate
            .as_deref()
            .unwrap_or_else(|| self.names.split(',').next().unwrap_or_default())
    }
}

impl Args for Checking {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "names",
            "The DNS names the certificate would be for, with a comma between them: 1 to 20 names, a wildcard only with dns.",
            1,
            NAMES_CHARS,
            Some(COMMA_NAMES),
        ),
        Field::text(
            "as",
            "The certificate's name, when it is not the first of the names.",
            1,
            CERTIFICATE_NAME_MAX,
            Some(NAME_PATTERN),
        )
        .optional(),
        Field::flag("dns", "Whether the names are proved by DNS records, not on port 80.")
            .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        read_names(&self.names, ',', self.dns, true)?;
        match &self.certificate {
            Some(name) => check_name_of("as", name),
            None => check_certificate(self.certificate_name()).map_err(|_| Bad {
                field: "as",
                why: refusal(
                    "the first name is not one a certificate can be called: give the certificate \
                     a name with as",
                ),
            }),
        }
    }
}

/// `cert get NAME --name N... [--dns] --check --json`: what the checks
/// before an order find, which tell what to fix, and orders nothing. It
/// ends with 1 when one of them failed and has printed what each found, so
/// both statuses are answers.
pub struct Check;

impl Action for Check {
    const ID: &'static str = "certificate.check";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates/check/{names}";
    const RUNS: Runs = Runs::AsThePerson;
    const ANSWERS: &'static [i32] = &[0, 1];
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Check the names for a certificate";
    type Args = Checking;

    fn argv(args: &Checking) -> Vec<String> {
        let mut argv = ["cert", "get", args.certificate_name()].map(String::from).to_vec();
        for name in args.names.split(',') {
            argv.extend(["--name".to_string(), name.to_string()]);
        }
        if args.dns {
            argv.push("--dns".into());
        }
        argv.extend(["--check", "--json"].map(String::from));
        argv
    }

    fn subject(args: &Checking) -> Option<String> {
        Some(args.certificate_name().to_string())
    }

    fn examples() -> Examples<Checking> {
        Examples {
            good: vec![
                Checking { names: "shop.example.org".into(), certificate: None, dns: false },
                Checking {
                    names: "shop.example.org,*.shop.example.org".into(),
                    certificate: Some("shop".into()),
                    dns: true,
                },
            ],
            bad: vec![
                json!({ "names": "shop..example.org" }),
                json!({ "names": "" }),
                json!({ "names": "Shop.example.org" }),
                json!({ "names": "*.example.org" }),
                json!({ "names": "a.example.org,a.example.org" }),
                json!({ "as": "Shop", "names": "shop.example.org" }),
                json!({ "as": "", "names": "shop.example.org" }),
                json!({ "dns": "yes", "names": "shop.example.org" }),
                json!({ "names": "shop.example.org", "port": 80 }),
                json!({}),
            ],
            undo: None,
        }
    }
}

/// What `certificate.get` orders.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Getting {
    /// The certificate's name.
    pub name: String,
    /// The DNS names, one a line.
    pub names: String,
    /// Whether the names are proved through DNS, not on port 80.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dns: bool,
    /// Whether to order from the authority's test service and install
    /// nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// The address the authority writes to, for a new account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// That the person agrees to the authority's terms.
    pub agree_terms: bool,
}

impl Args for Getting {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::text(
            "names",
            "The DNS names the certificate is for, one a line: 1 to 20 names, a wildcard only with dns.",
            1,
            NAMES_CHARS,
            Some(LINE_NAMES),
        ),
        Field::flag("dns", "Whether the names are proved by DNS records, not on port 80.")
            .optional(),
        Field::flag(
            "dry_run",
            "Whether to order from the authority's test service and install nothing.",
        )
        .optional(),
        Field::text(
            "email",
            "The address the authority writes to about the certificate, for a new account.",
            3,
            EMAIL_MAX,
            Some(EMAIL_PATTERN),
        )
        .optional(),
        Field::flag(
            "agree_terms",
            "That you agree to the authority's terms of service, which must be true.",
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if !self.agree_terms {
            return Err(Bad {
                field: "agree_terms",
                why: refusal("Agree to the authority's terms first"),
            });
        }
        read_names(&self.names, '\n', self.dns, false)?;
        if let Some(email) = &self.email {
            let fits = email.len() <= EMAIL_MAX
                && email.split_once('@').is_some_and(|(local, domain)| {
                    let plain = |part: &str| {
                        !part.is_empty() && !part.contains(['@', ' ', '\t', '\r', '\n'])
                    };
                    plain(local) && plain(domain)
                });
            if !fits {
                return Err(Bad {
                    field: "email",
                    why: refusal(&format!(
                        "an address is something, an @ and something, with no space, in \
                         {EMAIL_MAX} bytes at most"
                    )),
                });
            }
        }
        Ok(())
    }
}

/// `cert get NAME --name N... [--dns] [--dry-run] [--email E]
/// --agree-terms --yes --progress`: asks the authority for a certificate
/// and installs it, as a task that tells each step. It names no file: the
/// store is certd's, which admind does not see.
pub struct Get;

impl Action for Get {
    const ID: &'static str = "certificate.get";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/get";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Get a certificate";
    type Args = Getting;

    fn argv(args: &Getting) -> Vec<String> {
        let mut argv = ["cert", "get", args.name.as_str()].map(String::from).to_vec();
        for name in args.names.split('\n') {
            argv.extend(["--name".to_string(), name.to_string()]);
        }
        if args.dns {
            argv.push("--dns".into());
        }
        if args.dry_run {
            argv.push("--dry-run".into());
        }
        if let Some(email) = &args.email {
            argv.extend(["--email".to_string(), email.clone()]);
        }
        argv.extend(["--agree-terms", "--yes", "--progress"].map(String::from));
        argv
    }

    fn subject(args: &Getting) -> Option<String> {
        Some(args.name.clone())
    }

    /// `cert list --json`, which everyone may run: the certificate's names
    /// and when it runs out are what an order moves.
    fn probe(_: &Getting) -> Option<Vec<String>> {
        Some(["cert", "list", "--json"].map(String::from).to_vec())
    }

    /// The names and the end of the certificate called `args.name`, or
    /// `none` when the store has none.
    fn probed(args: &Getting, output: &str) -> Option<String> {
        let list: Value = serde_json::from_str(output).ok()?;
        let found = list
            .as_array()?
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(args.name.as_str()));
        Some(match found {
            Some(entry) => json!([entry.get("names"), entry.get("not_after")]).to_string(),
            None => "none".to_string(),
        })
    }

    /// A dry run installs nothing, so it moves nothing; for an order that
    /// installs, only the authority knows what it will give.
    fn settled(args: &Getting, _: &str) -> Option<bool> {
        args.dry_run.then_some(true)
    }

    fn examples() -> Examples<Getting> {
        Examples {
            good: vec![Getting {
                name: "shop".into(),
                names: "shop.example.org".into(),
                dns: false,
                dry_run: false,
                email: None,
                agree_terms: true,
            }],
            bad: vec![
                json!({ "name": "shop", "names": "shop.example.org" }),
                json!({ "agree_terms": false, "name": "shop", "names": "shop.example.org" }),
                json!({ "agree_terms": true, "name": "shop", "names": "shop..example.org" }),
                json!({ "agree_terms": true, "name": "shop", "names": "*.example.org" }),
                json!({ "agree_terms": true, "name": "shop", "names": "shop" }),
                json!({ "agree_terms": true, "name": "../x", "names": "shop.example.org" }),
                json!({ "agree_terms": true, "name": "shop", "names": "" }),
                json!({
                    "agree_terms": true, "email": "nobody", "name": "shop",
                    "names": "shop.example.org",
                }),
                json!({
                    "agree_terms": true, "email": "a b@example.org", "name": "shop",
                    "names": "shop.example.org",
                }),
                json!({ "agree_terms": "yes", "name": "shop", "names": "shop.example.org" }),
                json!({
                    "agree_terms": true, "name": "shop", "names": "shop.example.org",
                    "staging": true,
                }),
            ],
            undo: Some(Undo { action: "certificate.remove", args: json!({ "name": "shop" }) }),
        }
    }
}

/// A certificate a person made, and how to add it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Adding {
    /// The certificate's name.
    pub name: String,
    /// Its key, then its chain, the certificate first, as PEM: one
    /// document, which is a secret.
    pub pem: Secret,
    /// Whether it replaces a certificate of that name that was added too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
    /// Whether it is from an authority this server does not trust.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub untrusted: bool,
}

impl Args for Adding {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::secret_document(
            "pem",
            "The private key, then the certificate and the chain that leads to its authority, each in PEM.",
            DOCUMENT_MAX,
        ),
        Field::flag(
            "replace",
            "Whether it replaces a certificate of this name that was added by hand.",
        )
        .optional(),
        Field::flag("untrusted", "Whether it is from an authority this server does not trust.")
            .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)
    }
}

/// `cert add NAME --from - [--replace] [--untrusted]`: adds a certificate a
/// person made, its key and chain given on the input. The certificate is
/// checked as `cert add` checks it, and its refusals have their own IDs.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "certificate.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["CRT-111", "CRT-112", "CRT-113"];
    /// The key and the chain are each up to a document's most.
    const INPUT_MAX: usize = 2 * DOCUMENT_MAX;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Add a certificate";
    type Args = Adding;

    fn argv(args: &Adding) -> Vec<String> {
        let mut argv =
            ["cert", "add", args.name.as_str(), "--from", "-"].map(String::from).to_vec();
        if args.replace {
            argv.push("--replace".into());
        }
        if args.untrusted {
            argv.push("--untrusted".into());
        }
        argv
    }

    /// The document as it is: the key, then the chain.
    fn input(args: &Adding) -> Option<Secret> {
        Some(args.pem.copied())
    }

    fn subject(args: &Adding) -> Option<String> {
        Some(args.name.clone())
    }

    /// `cert show NAME --json`, which everyone may run: the fingerprint is
    /// what an addition moves.
    fn probe(args: &Adding) -> Option<Vec<String>> {
        Some(["cert", "show", args.name.as_str(), "--json"].map(String::from).to_vec())
    }

    /// The certificate's fingerprint.
    fn probed(_: &Adding, output: &str) -> Option<String> {
        let shown: Value = serde_json::from_str(output).ok()?;
        shown.get("fingerprint_sha256").and_then(Value::as_str).map(String::from)
    }

    fn examples() -> Examples<Adding> {
        Examples {
            good: vec![Adding {
                name: "shop".into(),
                pem: Secret::from(EXAMPLE_PEM.to_string()),
                replace: false,
                untrusted: false,
            }],
            bad: vec![
                json!({ "name": "shop" }),
                json!({ "name": "shop", "pem": "" }),
                json!({ "name": "Shop", "pem": EXAMPLE_PEM }),
                json!({ "name": "../x", "pem": EXAMPLE_PEM }),
                json!({ "name": "shop", "pem": "a\u{0}b" }),
                json!({ "name": "shop", "pem": EXAMPLE_PEM, "replace": "yes" }),
                json!({ "name": "shop", "pem": EXAMPLE_PEM, "force": true }),
            ],
            // A replacement is not put back: the old key is gone.
            undo: Some(Undo { action: "certificate.remove", args: json!({ "name": "shop" }) }),
        }
    }
}

/// A key and a chain as the examples give them: the shape of the document,
/// not a certificate any command would take.
const EXAMPLE_PEM: &str = "-----BEGIN PRIVATE KEY-----\nTUFSS0VS\n-----END PRIVATE KEY-----\n\
                           -----BEGIN CERTIFICATE-----\nQ0hBSU4=\n-----END CERTIFICATE-----\n";

/// A certificate and why it is revoked.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Revoking {
    /// The certificate's name.
    pub name: String,
    /// The reason, as `cert revoke` says it.
    pub reason: String,
}

impl Args for Revoking {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::one_of("reason", "Why it is revoked, which the authority records.", &REASONS),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if !REASONS.contains(&self.reason.as_str()) {
            return Err(Bad {
                field: "reason",
                why: refusal(&format!("the reasons are {}", REASONS.join(", "))),
            });
        }
        Ok(())
    }
}

/// `cert revoke NAME --reason R`: tells the authority that the certificate
/// is not to be trusted. Nothing puts it back.
pub struct Revoke;

impl Action for Revoke {
    const ID: &'static str = "certificate.revoke";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/revoke";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Revoke a certificate";
    type Args = Revoking;

    fn argv(args: &Revoking) -> Vec<String> {
        ["cert", "revoke", args.name.as_str(), "--reason", args.reason.as_str()]
            .map(String::from)
            .to_vec()
    }

    fn subject(args: &Revoking) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Revoking> {
        Examples {
            good: vec![Revoking { name: "shop".into(), reason: "unspecified".into() }],
            bad: vec![
                json!({ "name": "shop" }),
                json!({ "name": "shop", "reason": "" }),
                json!({ "name": "shop", "reason": "keyCompromise" }),
                json!({ "name": "shop", "reason": "Unspecified" }),
                json!({ "name": "../x", "reason": "unspecified" }),
                json!({ "name": "shop", "reason": "unspecified", "force": true }),
            ],
            undo: None,
        }
    }
}

/// `cert remove NAME`: takes a certificate out of the store, which `cert`
/// refuses while a service's file names it. Nothing puts it back.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "certificate.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Remove a certificate";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["cert", "remove", args.name.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples { good: vec![Named { name: "shop".into() }], bad: bad_names(), undo: None }
    }
}

/// `cert history --json`: what happened to the certificates, the oldest
/// first, from the file certd and `cert` keep.
pub struct History;

impl Action for History {
    const ID: &'static str = "certificate.history";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates/history/all";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Show what happened to the certificates";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["cert", "history", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": "shop" }), json!({ "n": 5 })],
            undo: None,
        }
    }
}

/// `cert wanted --json`: the certificates that could not be got, which
/// certd asks the authority for again, and how the tries went.
pub struct Wanted;

impl Action for Wanted {
    const ID: &'static str = "certificate.wanted";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates/wanted/all";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Show the certificates certd tries to get again";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["cert", "wanted", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": "shop" }), json!({ "given_up": true })],
            undo: None,
        }
    }
}

/// `cert forget NAME`: certd does not ask for the certificate again, and its
/// alert goes. Nothing puts the record back: getting the certificate again
/// makes a new one.
pub struct Forget;

impl Action for Forget {
    const ID: &'static str = "certificate.forget";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/forget";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = PAGE_TESTS;
    const TITLE: &'static str = "Stop trying to get a certificate";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["cert", "forget", args.name.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    /// `cert wanted --json`, which everyone may run: the record is what the
    /// change takes away.
    fn probe(_: &Named) -> Option<Vec<String>> {
        Some(["cert", "wanted", "--json"].map(String::from).to_vec())
    }

    /// The time of the first try of the record called `args.name`, or `none`
    /// when certd has none.
    fn probed(args: &Named, output: &str) -> Option<String> {
        let wanted: Value = serde_json::from_str(output).ok()?;
        let found =
            wanted.get("wanted")?.as_array()?.iter().find(|entry| {
                entry.get("name").and_then(Value::as_str) == Some(args.name.as_str())
            });
        Some(match found {
            Some(entry) => entry.get("first_ms")?.to_string(),
            None => "none".to_string(),
        })
    }

    fn examples() -> Examples<Named> {
        Examples { good: vec![Named { name: "shop".into() }], bad: bad_names(), undo: None }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{REGISTRY, Registered};
    use super::*;
    use crate::admind::errors::Kind;

    /// The registered action `id`.
    fn registered(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: serde_json::Value) -> Vec<String> {
        registered(id).prepare(args).unwrap().argv
    }

    #[test]
    fn the_reads_run_cert_with_json() {
        assert_eq!(argv("certificate.list", json!({})), ["cert", "list", "--json"]);
        assert_eq!(
            argv("certificate.show", json!({ "name": "this-machine" })),
            ["cert", "show", "this-machine", "--json"]
        );
    }

    #[test]
    fn a_renewal_always_changed_something_so_it_has_no_probe() {
        let renew = registered("certificate.renew");
        assert!(!renew.compares());
        let prepared = renew.prepare(json!({ "name": "this-machine" })).unwrap();
        assert_eq!(prepared.probe, None);
        assert!(!registered("certificate.show").compares());
    }

    #[test]
    fn a_renewal_runs_as_an_administrator_with_the_name_after_the_verb() {
        assert_eq!(
            argv("certificate.renew", json!({ "name": "example.org" })),
            ["cert", "renew", "example.org"]
        );
        let renew = registered("certificate.renew");
        assert_eq!(
            (renew.changes(), renew.runs(), renew.area()),
            (true, Runs::AsAdministrator, "certificates")
        );
        assert_eq!(renew.path(), "/api/1/certificates/{name}/renew");
        for id in ["certificate.list", "certificate.show"] {
            let read = registered(id);
            assert_eq!(
                (read.changes(), read.runs(), read.area()),
                (false, Runs::AsThePerson, "certificates")
            );
        }
        assert_eq!(registered("certificate.list").path(), "/api/1/certificates");
        assert_eq!(registered("certificate.show").path(), "/api/1/certificates/{name}");
    }

    #[test]
    fn a_name_is_one_certd_and_cert_keep() {
        for name in ["this-machine", "example.org", "a", "0", "a-b.c-d", &"a".repeat(64)] {
            assert_eq!(argv("certificate.show", json!({ "name": name }))[2], name);
        }
        let words = "A certificate's name has 1 to 64 lowercase letters, digits, hyphens and dots, \
                     starts with a letter or a digit, and has no two dots in a row. Nothing was \
                     done.";
        for name in [
            "",
            "This-Machine",
            "-x",
            ".x",
            "a..b",
            "a_b",
            "a b",
            "a/b",
            "../x",
            "caf\u{e9}",
            &"a".repeat(65),
        ] {
            for id in ["certificate.show", "certificate.renew"] {
                let failure = registered(id).prepare(json!({ "name": name })).unwrap_err();
                assert_eq!(
                    (failure.kind, failure.field),
                    (Kind::BadValue, Some("name")),
                    "{id} {name:?}"
                );
                assert_eq!(failure.detail, words, "{id} {name:?}");
            }
        }
    }

    #[test]
    fn arguments_the_action_does_not_take_do_not_read() {
        let wrong = json!({ "name": "this-machine", "force": true });
        for id in ["certificate.show", "certificate.renew"] {
            assert_eq!(
                registered(id).prepare(wrong.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
            assert_eq!(
                registered(id).prepare(json!({})).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
        }
        let list = registered("certificate.list").prepare(json!({ "name": "this-machine" }));
        assert_eq!(list.unwrap_err().kind, Kind::BadRequest);
    }

    #[test]
    fn the_ansible_task_is_named_for_the_action_and_the_certificate() {
        let task = registered("certificate.renew")
            .prepare(json!({ "name": "this-machine" }))
            .unwrap()
            .ansible;
        assert_eq!(task.name, "Renew a certificate: this-machine");
        assert!(task.become_root);
        assert_eq!(task.changed_when, None);
        let task = registered("certificate.list").prepare(json!({})).unwrap().ansible;
        assert_eq!(task.name, "List the certificates");
        assert_eq!(task.changed_when.as_deref(), Some("false"));
    }

    #[test]
    fn a_renewal_names_no_file() {
        let prepared =
            registered("certificate.renew").prepare(json!({ "name": "this-machine" })).unwrap();
        assert!(prepared.files.is_empty());
    }

    /// The ids of the actions C3 made, whose examples are the machine's own
    /// certificate.
    const FIRST_THREE: [&str; 3] = ["certificate.list", "certificate.show", "certificate.renew"];

    #[test]
    fn the_examples_use_the_machine_s_own_certificate_and_the_bad_ones_do_not_work() {
        for id in FIRST_THREE {
            let action = registered(id);
            let (good, bad, undo) = action.example_values();
            assert!(
                undo.is_none(),
                "{id}: nothing puts back a renewal, and a read changes nothing"
            );
            assert!(!bad.is_empty(), "{id}");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        let (good, _, _) = registered("certificate.renew").example_values();
        assert_eq!(good, [json!({ "name": "this-machine" })]);
    }

    /// Each action of the Certificates page: its id, its address, whether it
    /// changes anything, who runs it, and the command its first example
    /// makes.
    const PAGE: [(&str, &str, bool, Runs, &[&str]); 8] = [
        (
            "certificate.check",
            "/api/1/certificates/check/{names}",
            false,
            Runs::AsThePerson,
            &["cert", "get", "shop.example.org", "--name", "shop.example.org", "--check", "--json"],
        ),
        (
            "certificate.get",
            "/api/1/certificates/{name}/get",
            true,
            Runs::AsAdministrator,
            &[
                "cert",
                "get",
                "shop",
                "--name",
                "shop.example.org",
                "--agree-terms",
                "--yes",
                "--progress",
            ],
        ),
        (
            "certificate.add",
            "/api/1/certificates/{name}/add",
            true,
            Runs::AsAdministrator,
            &["cert", "add", "shop", "--from", "-"],
        ),
        (
            "certificate.revoke",
            "/api/1/certificates/{name}/revoke",
            true,
            Runs::AsAdministrator,
            &["cert", "revoke", "shop", "--reason", "unspecified"],
        ),
        (
            "certificate.remove",
            "/api/1/certificates/{name}/remove",
            true,
            Runs::AsAdministrator,
            &["cert", "remove", "shop"],
        ),
        (
            "certificate.history",
            "/api/1/certificates/history/all",
            false,
            Runs::AsThePerson,
            &["cert", "history", "--json"],
        ),
        (
            "certificate.wanted",
            "/api/1/certificates/wanted/all",
            false,
            Runs::AsThePerson,
            &["cert", "wanted", "--json"],
        ),
        (
            "certificate.forget",
            "/api/1/certificates/{name}/forget",
            true,
            Runs::AsAdministrator,
            &["cert", "forget", "shop"],
        ),
    ];

    /// The first example of `id`, as the arguments it takes.
    fn first_example(id: &str) -> serde_json::Value {
        registered(id).example_values().0.remove(0)
    }

    #[test]
    fn each_action_of_the_page_runs_the_command_the_design_names_for_the_caller_it_names() {
        for (id, path, changes, runs, words) in PAGE {
            let action = registered(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            assert_eq!(prepared.argv, words, "{id}");
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (changes, runs, "certificates", path),
                "{id}"
            );
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
            assert_eq!(prepared.ansible.changed_when.as_deref() == Some("false"), !changes, "{id}");
            assert_eq!(prepared.trial, None, "{id}");
            // None names a file: the store is certd's, which admind does not
            // see.
            assert!(prepared.files.is_empty(), "{id}");
            let tests = action.tests();
            assert_eq!(tests.checks, "admind::actions::certificate::tests", "{id}");
            assert_eq!(tests.route, "admin-certificates", "{id}");
            assert_eq!(tests.same_as_command, "admin-certificates", "{id}");
            assert_eq!(tests.browser, "web-console/certificates", "{id}");
            assert_eq!(tests.ansible, "web-console/ansible", "{id}");
            assert!(!action.lines() && !action.secret_answer(), "{id}");
            assert_eq!((action.not_self(), action.cuts_off_on()), (None, None), "{id}");
        }
        let ours = REGISTRY.iter().filter(|a| a.id().starts_with("certificate.")).count();
        assert_eq!(ours, FIRST_THREE.len() + PAGE.len());
    }

    #[test]
    fn only_getting_is_a_task_and_only_the_check_answers_after_1() {
        for (id, ..) in PAGE {
            let action = registered(id);
            assert_eq!(action.long(), id == "certificate.get", "{id}");
            let answers: &[i32] = if id == "certificate.check" { &[0, 1] } else { &[0] };
            assert_eq!(action.answers(), answers, "{id}");
        }
    }

    #[test]
    fn the_address_of_the_check_never_meets_the_one_of_a_certificate() {
        use super::super::{Found, find};
        // A certificate's own address has one part after `certificates`.
        let Found::Action(show, _) = find(REGISTRY, false, "/api/1/certificates/check") else {
            panic!("a certificate named check is not found");
        };
        assert_eq!(show.id(), "certificate.show");
        let Found::Action(check, params) =
            find(REGISTRY, false, "/api/1/certificates/check/shop.example.org%2C*.example.org")
        else {
            panic!("the check is not found");
        };
        assert_eq!(check.id(), "certificate.check");
        assert_eq!(params["names"], "shop.example.org,*.example.org");
        let Found::Action(history, _) = find(REGISTRY, false, "/api/1/certificates/history/all")
        else {
            panic!("the history is not found");
        };
        assert_eq!(history.id(), "certificate.history");
        let Found::Action(wanted, _) = find(REGISTRY, false, "/api/1/certificates/wanted/all")
        else {
            panic!("the wanted certificates are not found");
        };
        assert_eq!(wanted.id(), "certificate.wanted");
        let Found::Action(forget, params) = find(REGISTRY, true, "/api/1/certificates/shop/forget")
        else {
            panic!("forgetting is not found");
        };
        assert_eq!(forget.id(), "certificate.forget");
        assert_eq!(params["name"], "shop");
    }

    #[test]
    fn a_check_names_the_certificate_by_its_first_name_unless_it_is_told_another() {
        assert_eq!(
            argv("certificate.check", json!({ "names": "a.example,b.example" })),
            [
                "cert",
                "get",
                "a.example",
                "--name",
                "a.example",
                "--name",
                "b.example",
                "--check",
                "--json"
            ]
        );
        assert_eq!(
            argv(
                "certificate.check",
                json!({ "as": "wild", "dns": true, "names": "*.a.example,a.example" })
            ),
            [
                "cert",
                "get",
                "wild",
                "--name",
                "*.a.example",
                "--name",
                "a.example",
                "--dns",
                "--check",
                "--json"
            ]
        );
        let prepared = registered("certificate.check")
            .prepare(json!({ "names": "a.example,b.example" }))
            .unwrap();
        assert_eq!(prepared.ansible.name, "Check the names for a certificate: a.example");
        // A wildcard cannot name a certificate, so one that has it first is
        // given a name.
        let failure = registered("certificate.check")
            .prepare(json!({ "dns": true, "names": "*.a.example" }))
            .unwrap_err();
        assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("as")));
        assert!(failure.detail.ends_with("Nothing was done."), "{}", failure.detail);
        assert!(
            registered("certificate.check")
                .prepare(json!({ "as": "wild", "dns": true, "names": "*.a.example" }))
                .is_ok()
        );
    }

    #[test]
    fn a_check_takes_a_name_of_one_word_and_an_order_does_not() {
        // The check tells what is wrong with `localhost`; an order is refused.
        assert_eq!(argv("certificate.check", json!({ "names": "localhost" }))[2], "localhost");
        let refused = registered("certificate.get")
            .prepare(json!({ "agree_terms": true, "name": "shop", "names": "localhost" }))
            .unwrap_err();
        assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("names")));
        assert!(
            refused.detail.contains("is not a name an authority issues for"),
            "{}",
            refused.detail
        );
        // Neither takes a wildcard without dns, a name twice, an empty word,
        // more than 20 names or an address in place of a name.
        let twenty: Vec<String> = (0..20).map(|n| format!("n{n}.example")).collect();
        let more: Vec<String> = (0..21).map(|n| format!("n{n}.example")).collect();
        let cases = [
            (json!({ "names": "*.a.example" }), "add --dns"),
            (json!({ "names": "a.example,a.example" }), "named twice"),
            (json!({ "names": "a..example" }), "is not a name an authority issues for"),
            (json!({ "names": "203.0.113.7" }), "is an IP address"),
        ];
        for (args, words) in cases {
            let refused = registered("certificate.check").prepare(args.clone()).unwrap_err();
            assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("names")), "{args}");
            assert!(refused.detail.contains(words), "{args}: {}", refused.detail);
        }
        assert!(
            registered("certificate.check").prepare(json!({ "names": twenty.join(",") })).is_ok()
        );
        let refused = registered("certificate.check")
            .prepare(json!({ "names": more.join(",") }))
            .unwrap_err();
        assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("names")));
    }

    #[test]
    fn getting_is_refused_before_it_runs_unless_the_terms_are_agreed_to() {
        let refused = registered("certificate.get")
            .prepare(json!({ "agree_terms": false, "name": "shop", "names": "shop.example.org" }))
            .unwrap_err();
        assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("agree_terms")));
        assert_eq!(refused.detail, "Agree to the authority's terms first. Nothing was done.");
        let missing = registered("certificate.get")
            .prepare(json!({ "name": "shop", "names": "shop.example.org" }))
            .unwrap_err();
        assert_eq!(missing.kind, Kind::BadRequest);
    }

    #[test]
    fn getting_gives_each_name_each_option_and_the_words_that_make_it_a_task() {
        assert_eq!(
            argv(
                "certificate.get",
                json!({
                    "agree_terms": true, "dns": true, "dry_run": true, "email": "me@example.org",
                    "name": "wild", "names": "*.a.example\na.example",
                })
            ),
            [
                "cert",
                "get",
                "wild",
                "--name",
                "*.a.example",
                "--name",
                "a.example",
                "--dns",
                "--dry-run",
                "--email",
                "me@example.org",
                "--agree-terms",
                "--yes",
                "--progress"
            ]
        );
        // `this-machine` is cert's own refusal, which the task shows.
        assert!(
            registered("certificate.get")
                .prepare(json!({
                    "agree_terms": true, "name": "this-machine", "names": "a.example"
                }))
                .is_ok()
        );
    }

    #[test]
    fn an_address_is_something_an_at_and_something_with_no_space_in_254_bytes() {
        let take = |email: &str| {
            registered("certificate.get").prepare(json!({
                "agree_terms": true, "email": email, "name": "shop", "names": "shop.example.org"
            }))
        };
        for fine in ["a@b", "me@example.org", "a.b+c@d.e", &format!("{}@x", "a".repeat(252))] {
            assert!(take(fine).is_ok(), "{fine}");
        }
        for wrong in [
            "",
            "a",
            "@b",
            "a@",
            "a@b@c",
            "a b@c",
            "a@b c",
            "a\n@b",
            "a\t@b",
            &format!("{}@x", "a".repeat(253)),
        ] {
            assert!(take(wrong).is_err(), "{wrong:?}");
        }
        let refused = take("nobody").unwrap_err();
        assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("email")));
    }

    #[test]
    fn getting_compares_the_names_and_the_end_of_the_certificate_and_a_dry_run_moves_nothing() {
        let get = registered("certificate.get");
        let args = first_example("certificate.get");
        let prepared = get.prepare(args.clone()).unwrap();
        assert_eq!(prepared.probe.unwrap(), ["cert", "list", "--json"]);
        assert!(get.compares());
        let list = r#"[
            {"name": "this-machine", "names": ["dukovo"], "not_after": "2027-01-01T00:00:00Z"},
            {"name": "shop", "names": ["shop.example.org"], "not_after": "2027-02-01T00:00:00Z",
             "fingerprint_sha256": "AA"}
        ]"#;
        let now = get.probed(&args, list).unwrap();
        assert_eq!(now, r#"[["shop.example.org"],"2027-02-01T00:00:00Z"]"#);
        // A certificate the store does not have, and an answer that does not
        // read.
        assert_eq!(get.probed(&args, "[]").as_deref(), Some("none"));
        assert_eq!(get.probed(&args, "not json"), None);
        assert_eq!(get.probed(&args, "{}"), None);
        // Only a dry run can tell that nothing would move.
        assert_eq!(get.settled(&args, &now), None);
        let mut dry = args;
        dry["dry_run"] = json!(true);
        assert_eq!(get.settled(&dry, &now), Some(true));
        let undo = get.example_values().2.unwrap();
        assert_eq!((undo.action, undo.args), ("certificate.remove", json!({ "name": "shop" })));
    }

    /// A key and a chain of the size `bytes`, as one document.
    fn document(bytes: usize) -> String {
        let key = "-----BEGIN PRIVATE KEY-----\nTUFSS0VS\n-----END PRIVATE KEY-----\n";
        let mut text = String::from(key);
        while text.len() < bytes {
            text.push_str("-----BEGIN CERTIFICATE-----\nQ0hBSU4=\n-----END CERTIFICATE-----\n");
        }
        text.truncate(bytes);
        text
    }

    #[test]
    fn adding_gives_the_command_the_document_as_it_is_on_its_input() {
        let add = registered("certificate.add");
        let pem = document(1_000);
        let prepared = add
            .prepare(json!({ "name": "shop", "pem": pem, "replace": true, "untrusted": true }))
            .unwrap();
        assert_eq!(
            prepared.argv,
            ["cert", "add", "shop", "--from", "-", "--replace", "--untrusted"]
        );
        assert_eq!(prepared.input.as_ref().unwrap().bytes(), pem.as_bytes());
        // The document is nowhere else.
        let marker = &pem[..60];
        for text in [
            prepared.argv.join(" "),
            prepared.args.to_string(),
            format!("{prepared:?}"),
            crate::admind::actions::show::yaml(&prepared.ansible),
        ] {
            assert!(!text.contains(marker), "{text}");
        }
        assert_eq!(add.input_max(), 131_072);
        assert_eq!(add.errors(), ["CRT-111", "CRT-112", "CRT-113"]);
        // A read or another change gives no input.
        assert!(
            registered("certificate.remove")
                .prepare(first_example("certificate.remove"))
                .unwrap()
                .input
                .is_none()
        );
    }

    #[test]
    fn the_task_of_adding_reads_the_document_from_the_playbook_without_a_log() {
        let task = registered("certificate.add")
            .prepare(json!({ "name": "shop", "pem": document(100) }))
            .unwrap()
            .ansible;
        assert_eq!(task.name, "Add a certificate: shop");
        assert_eq!(task.module, "ansible.builtin.command");
        assert_eq!(task.params["stdin"], "{{ pem }}");
        assert!(task.no_log && task.become_root);
    }

    #[test]
    fn a_document_is_one_to_64_kib_with_no_nul_and_the_input_is_the_most_a_command_is_given() {
        let add = registered("certificate.add");
        let most = document(65_536);
        assert_eq!(most.len(), 65_536);
        assert!(add.prepare(json!({ "name": "shop", "pem": most })).is_ok());
        for pem in [String::new(), document(65_537), "a\u{0}b".to_string()] {
            let refused = add.prepare(json!({ "name": "shop", "pem": pem })).unwrap_err();
            assert_eq!(refused.kind, Kind::BadValue, "{}", pem.len());
        }
        // The key and the chain are one document, so the input is
        // the document and no more; the command has room for 64 KiB of each.
        assert_eq!(add.fields().iter().filter(|field| field.is_input()).count(), 1);
    }

    #[test]
    fn adding_compares_the_fingerprint_and_a_new_certificate_is_put_back_by_removing_it() {
        let add = registered("certificate.add");
        let args = first_example("certificate.add");
        assert_eq!(args["pem"], "(secret)");
        let args = json!({ "name": "shop", "pem": document(100) });
        let prepared = add.prepare(args.clone()).unwrap();
        assert_eq!(prepared.probe.unwrap(), ["cert", "show", "shop", "--json"]);
        assert!(add.compares());
        assert_eq!(
            add.probed(&args, r#"{"name": "shop", "fingerprint_sha256": "AB:CD"}"#).as_deref(),
            Some("AB:CD")
        );
        assert_eq!(add.probed(&args, "{}"), None);
        assert_eq!(add.probed(&args, "nonsense"), None);
        let undo = add.example_values().2.unwrap();
        assert_eq!((undo.action, undo.args), ("certificate.remove", json!({ "name": "shop" })));
    }

    #[test]
    fn forgetting_compares_the_first_try_of_the_record_and_nothing_puts_it_back() {
        let forget = registered("certificate.forget");
        let args = first_example("certificate.forget");
        let prepared = forget.prepare(args.clone()).unwrap();
        assert_eq!(prepared.probe.unwrap(), ["cert", "wanted", "--json"]);
        assert!(forget.compares());
        let wanted = r#"{"format": 1, "wanted": [
            {"name": "blog", "first_ms": 1700000000000},
            {"name": "shop", "first_ms": 1800000000000, "tries": 3}
        ]}"#;
        assert_eq!(forget.probed(&args, wanted).as_deref(), Some("1800000000000"));
        // A certificate certd has no record of, and an answer that does not
        // read.
        assert_eq!(forget.probed(&args, r#"{"format": 1, "wanted": []}"#).as_deref(), Some("none"));
        assert_eq!(forget.probed(&args, "not json"), None);
        assert_eq!(forget.probed(&args, "[]"), None);
        assert_eq!(forget.example_values().2.map(|undo| undo.action), None);
        // The store is certd's: the change names no file.
        assert!(prepared.files.is_empty());
    }

    #[test]
    fn the_wanted_certificates_are_a_read_that_takes_nothing() {
        let wanted = registered("certificate.wanted");
        assert!(!wanted.compares());
        assert_eq!(argv("certificate.wanted", json!({})), ["cert", "wanted", "--json"]);
        assert!(wanted.prepare(json!({ "name": "shop" })).is_err());
    }

    #[test]
    fn a_revocation_gives_the_reason_as_cert_says_it_and_nothing_puts_it_back() {
        for reason in ["unspecified", "key-compromise", "superseded", "cessation-of-operation"] {
            assert_eq!(
                argv("certificate.revoke", json!({ "name": "shop", "reason": reason })),
                ["cert", "revoke", "shop", "--reason", reason]
            );
        }
        for reason in ["", "keyCompromise", "Unspecified", "unspecified ", "other"] {
            let refused = registered("certificate.revoke")
                .prepare(json!({ "name": "shop", "reason": reason }))
                .unwrap_err();
            assert_eq!((refused.kind, refused.field), (Kind::BadValue, Some("reason")), "{reason}");
        }
        for id in ["certificate.revoke", "certificate.remove", "certificate.history"] {
            assert!(registered(id).example_values().2.is_none(), "{id}");
            assert!(!registered(id).compares(), "{id}");
        }
    }

    #[test]
    fn a_name_is_one_certd_and_cert_keep_for_each_change_of_the_page() {
        for id in ["certificate.get", "certificate.add", "certificate.revoke", "certificate.remove"]
        {
            for name in ["This-Machine", "-x", "a..b", "../x", ""] {
                let mut args = first_example(id);
                args["name"] = json!(name);
                if id == "certificate.add" {
                    args["pem"] = json!(document(100));
                }
                let failure = registered(id).prepare(args).unwrap_err();
                assert_eq!(
                    (failure.kind, failure.field),
                    (Kind::BadValue, Some("name")),
                    "{id} {name:?}"
                );
            }
        }
    }
}
