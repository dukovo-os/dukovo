// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The websites' actions (ADR 0569): the list, nginx's check, the plan of
//! an add, adding a site, serving it over HTTPS or over HTTP only again,
//! and removing it. Each runs `website`, website(8), which reads and
//! writes nginx's settings; the changes run as an administrator, as
//! `elevate website add NAME ...` does at a terminal, and are tasks, since
//! nginx restarts or reloads and each site is asked for a file before the
//! change is kept.
//!
//! What a person types reaches the command only after it passes the rule
//! `website` holds it to, which is checked here first: a name by the
//! certificates' rule, a folder under the sites' folder in the characters
//! `website` writes into a file as they are, an account's name and a
//! certificate's name. No word of the command starts with `-` or holds a
//! space, a quote or a line end, so no value can be read as an option or
//! as more than one word.
//!
//! A change names the files it writes, which are in the settings admind's
//! journal sees: the site's file, the default server's and nginx's service
//! file's override. Its probe is `website list --json`, from which the
//! site's own row is compared before and after.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::certificate;
use super::trial::refusal;
use super::{
    Action, Ansible, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo, command_task,
};
use crate::website::{
    DEFAULT_FILE, FOLDER_MAX, NAME_MAX, NAMES_MAX, OVERRIDE, SETTINGS, check_folder, check_name,
    check_site_name,
};
use manifest::service::{CERTIFICATE_NAME_MAX, check_account};

/// The area of these actions: the scope a token needs, `websites`.
const AREA: &str = "websites";

/// The tests of the reads, which `lab/actions.toml` names: the boot test
/// that calls every route calls them, on a machine without nginx.
const TESTS: Tests = Tests {
    checks: "admind::actions::websites::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/websites",
    ansible: "web-console/ansible",
};

/// The tests of the changes: they need nginx, which the boot test of the
/// Websites page installs.
const CHANGE_TESTS: Tests = Tests {
    checks: "admind::actions::websites::tests",
    route: "admin-websites",
    same_as_command: "admin-websites",
    browser: "web-console/websites",
    ansible: "web-console/ansible",
};

/// The kinds of site `website add` makes.
const KINDS: &[&str] = &["static", "php"];

/// What a site's name matches, as `check` checks it apart from the
/// address by number, which only the code refuses: labels of lowercase
/// letters, digits and hyphens, a hyphen never first or last, up to 63
/// characters, with a dot between them.
const NAME_PATTERN: &str =
    "^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$";

/// What the other names of a site are made of: the letters, digits and
/// signs a name has, a line end between them, at most [`NAMES_MAX`] less
/// one. That each is a name is the rule that only `check` says.
const LINE_NAMES: &str = "^[a-z0-9.-]+(\n[a-z0-9.-]+){0,18}$";

/// The most characters of the other names of a site: each of at most 253
/// and a line end.
const NAMES_CHARS: usize = (NAMES_MAX - 1) * 254;

/// What a site's folder matches: under the sites' folder, each part of
/// 1 to 255 characters that `website` writes into a file as they are,
/// the first a letter, a digit or `_`.
const FOLDER_PATTERN: &str = "^/shared/www/sites/[A-Za-z0-9_][A-Za-z0-9._@+-]{0,254}\
                              (/[A-Za-z0-9_][A-Za-z0-9._@+-]{0,254})*$";

/// What an account's name matches, as `user` has it: 1 to 32 lowercase
/// letters, digits, `_`, `-` and `.`, starting with a letter or `_`.
const ACCOUNT_PATTERN: &str = "^[a-z_][a-z0-9_.-]{0,31}$";

/// The site the examples are about.
const EXAMPLE: &str = "shop.lab.example";

/// The account the examples' folders belong to.
const OWNER: &str = "webdev";

/// The field of a site's name, which every action of these takes.
const NAME: Field = Field::text(
    "name",
    "The site's name, which is its first name and its file's: lowercase letters, digits and hyphens in labels, with a dot between them.",
    1,
    NAME_MAX,
    Some(NAME_PATTERN),
);

/// The fields of a site to make: its names, kind, folder, owner and
/// certificate.
const SITE_FIELDS: &[Field] = &[
    NAME,
    Field::text(
        "names",
        "The other names the site answers to, one a line: 19 at most, each a name as the site's is.",
        1,
        NAMES_CHARS,
        Some(LINE_NAMES),
    )
    .optional(),
    Field::one_of(
        "kind",
        "static serves the files of the folder; php also hands the files that end in .php to PHP-FPM.",
        KINDS,
    ),
    Field::text(
        "folder",
        "The site's folder, under /shared/www/sites, when it is not named for the site.",
        1,
        FOLDER_MAX,
        Some(FOLDER_PATTERN),
    )
    .optional(),
    Field::text(
        "owner",
        "The person whose the folder is, by the account's name.",
        1,
        32,
        Some(ACCOUNT_PATTERN),
    ),
    Field::text(
        "certificate",
        "The certificate that serves the site over HTTPS, as cert list shows it: it must name every name of the site.",
        1,
        CERTIFICATE_NAME_MAX,
        Some(certificate::NAME_PATTERN),
    )
    .optional(),
];

/// The words of a refusal of a site's name, and which field.
fn bad_name(field: &'static str) -> Bad {
    Bad {
        field,
        why: refusal(
            "a site's name is labels of lowercase letters, digits and hyphens, up to 63 \
             characters each, with a dot between them, and at most 250 characters in all; a \
             name that is not ASCII is written in punycode, as xn--, and an address by number \
             or a wildcard is no name",
        ),
    }
}

/// Checks the name of a site, as `website` takes it.
fn check_site(name: &str) -> Result<(), Bad> {
    check_site_name(name).map_err(|_| bad_name("name"))
}

/// Checks the other names of a site, `text`, one a line, beside its own,
/// `first`: at most [`NAMES_MAX`] in all, each a name as `website` takes
/// it, none twice.
fn check_names(first: &str, text: &str) -> Result<(), Bad> {
    let bad = |why: &str| Bad { field: "names", why: refusal(why) };
    if text.chars().count() > NAMES_CHARS {
        return Err(bad(&format!("the other names are {NAMES_CHARS} characters at most")));
    }
    let names: Vec<&str> = text.split('\n').collect();
    if names.len() + 1 > NAMES_MAX {
        return Err(bad(&format!(
            "a site answers to {NAMES_MAX} names at most: its own and {} more",
            NAMES_MAX - 1
        )));
    }
    if names.iter().any(|name| check_name(name).is_err()) {
        return Err(bad(
            "each name is labels of lowercase letters, digits and hyphens, up to 63 characters \
             each, with a dot between them, and at most 253 characters in all; a name that is \
             not ASCII is written in punycode, as xn--, and an address by number or a wildcard \
             is no name",
        ));
    }
    let mut all = vec![first];
    all.extend(&names);
    all.sort_unstable();
    if all.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(bad("a name is given twice"));
    }
    Ok(())
}

/// Checks the folder of a site, as `website add --folder` takes it.
fn check_folder_of(folder: &str) -> Result<(), Bad> {
    check_folder(folder).map_err(|_| Bad {
        field: "folder",
        why: refusal(&format!(
            "a site's folder is under /shared/www/sites, in parts of letters, digits and \
             . _ @ + - that start with a letter, a digit or _, in {FOLDER_MAX} characters at \
             most"
        )),
    })
}

/// Checks the account that owns a site's folder, by the rule accounts
/// follow. That it is a person's is for `website` to say.
fn check_owner(owner: &str) -> Result<(), Bad> {
    check_account(owner).map_err(|_| Bad {
        field: "owner",
        why: refusal(
            "an account's name has 1 to 32 lowercase letters, digits, _, - and ., and starts \
             with a letter or _",
        ),
    })
}

/// The file of the site `name`.
fn site_file(name: &str) -> String {
    format!("{SETTINGS}/{name}.conf")
}

/// The files a change of the site `name` writes.
fn files_of(name: &str) -> Vec<String> {
    vec![site_file(name), format!("{SETTINGS}/{DEFAULT_FILE}"), OVERRIDE.to_string()]
}

/// `website list --json`, which everyone may run: what the probes read.
fn list_argv() -> Vec<String> {
    ["website", "list", "--json"].map(String::from).to_vec()
}

/// What `output`, the list, says of the site `name`: None when it does not
/// read as the list, Some(None) when the list has no such site.
fn entry(name: &str, output: &str) -> Option<Option<Value>> {
    let list: Value = serde_json::from_str(output).ok()?;
    let sites = list.get("websites")?.as_array()?;
    Some(sites.iter().find(|site| site.get("name").and_then(Value::as_str) == Some(name)).cloned())
}

/// What the list says of the site `name`, of the members `keys`, as a line
/// of JSON, or `none` when it has no such site: what a change moves.
fn picked(name: &str, output: &str, keys: &[&str]) -> Option<String> {
    Some(match entry(name, output)? {
        Some(site) => {
            let values: Vec<Value> =
                keys.iter().map(|key| site.get(*key).cloned().unwrap_or(Value::Null)).collect();
            Value::from(values).to_string()
        }
        None => "none".to_string(),
    })
}

/// What a site is made with: its names, kind, folder, owner and
/// certificate.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    /// The site's name, which is its first name.
    pub name: String,
    /// The other names, one a line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<String>,
    /// `static` or `php`.
    pub kind: String,
    /// Its folder, when it is not named for the site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The account whose the folder is.
    pub owner: String,
    /// The certificate that serves it over HTTPS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
}

impl Args for Site {
    const FIELDS: &'static [Field] = SITE_FIELDS;

    fn check(&self) -> Result<(), Bad> {
        check_site(&self.name)?;
        if let Some(names) = &self.names {
            check_names(&self.name, names)?;
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err(Bad { field: "kind", why: refusal("a site's kind is static or php") });
        }
        if let Some(folder) = &self.folder {
            check_folder_of(folder)?;
        }
        check_owner(&self.owner)?;
        if let Some(certificate) = &self.certificate {
            certificate::check_name_of("certificate", certificate)?;
        }
        Ok(())
    }
}

impl Site {
    /// The command that makes the site, ending with `ending`: the names,
    /// then each option the site has. The first word of each value starts
    /// with a letter, a digit or `/`, which the rules above hold to.
    fn command(&self, ending: &[&str]) -> Vec<String> {
        let mut argv = ["website", "add", self.name.as_str()].map(String::from).to_vec();
        for name in self.names.iter().flat_map(|names| names.split('\n')) {
            argv.extend(["--name".to_string(), name.to_string()]);
        }
        argv.extend(["--kind".to_string(), self.kind.clone()]);
        if let Some(folder) = &self.folder {
            argv.extend(["--folder".to_string(), folder.clone()]);
        }
        argv.extend(["--owner".to_string(), self.owner.clone()]);
        if let Some(certificate) = &self.certificate {
            argv.extend(["--certificate".to_string(), certificate.clone()]);
        }
        argv.extend(ending.iter().map(|word| (*word).to_string()));
        argv
    }
}

/// The examples' site: static, HTTP only, with the example account.
fn example() -> Site {
    Site {
        name: EXAMPLE.into(),
        names: None,
        kind: "static".into(),
        folder: None,
        owner: OWNER.into(),
        certificate: None,
    }
}

/// Arguments no action that takes a site takes: a name that is none, a name
/// twice, an address by number, a kind there is not, a folder outside the
/// sites' folder, an account that is none, and a field it has none of.
fn bad_sites() -> Vec<Value> {
    let site = |change: Value| {
        let mut args = json!({ "name": EXAMPLE, "kind": "static", "owner": OWNER });
        if let (Some(args), Some(change)) = (args.as_object_mut(), change.as_object()) {
            args.extend(change.clone());
        }
        args
    };
    let many = (0..20).map(|n| format!("n{n}.lab.example")).collect::<Vec<_>>().join("\n");
    vec![
        json!({}),
        site(json!({ "name": "-x" })),
        site(json!({ "name": "a..b" })),
        site(json!({ "name": "*.x" })),
        site(json!({ "name": "Shop.lab.example" })),
        site(json!({ "name": "10.0.0.1" })),
        site(json!({ "kind": "cgi" })),
        site(json!({ "folder": "/etc" })),
        site(json!({ "folder": "/shared/www/sites/../x" })),
        site(json!({ "owner": "Webdev" })),
        site(json!({ "certificate": "../x" })),
        site(json!({ "names": EXAMPLE })),
        site(json!({ "names": "a..b" })),
        site(json!({ "names": many })),
        site(json!({ "port": 80 })),
    ]
}

/// `website list --json`: the sites, and whether nginx and PHP-FPM are
/// there to serve them.
pub struct List;

impl Action for List {
    const ID: &'static str = "websites.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/websites";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the websites";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        list_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": EXAMPLE }), json!({ "json": true })],
            undo: None,
        }
    }
}

/// `website check --json`: whether nginx accepts the settings as they are
/// and has the two parts the sites need. It reads nginx's own folder,
/// which is root's, so it runs as an administrator, and ends with 1 when
/// the check failed, which its JSON says: both statuses are answers.
pub struct Check;

impl Action for Check {
    const ID: &'static str = "websites.check";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/websites/check";
    const RUNS: Runs = Runs::AsAdministrator;
    const ANSWERS: &'static [i32] = &[0, 1];
    const ERRORS: &'static [&'static str] = &["WEB-102", "WEB-103"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Check the websites";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["website", "check", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": EXAMPLE }), json!({ "json": true })],
            undo: None,
        }
    }
}

/// `website add NAME ... --dry-run --json`: what an add would check, make,
/// write and start, which changes nothing that stays. It ends with 1 when
/// a check failed, which its JSON says: both statuses are answers.
pub struct Plan;

impl Action for Plan {
    const ID: &'static str = "websites.plan";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/websites/plan/{name}";
    const RUNS: Runs = Runs::AsAdministrator;
    const ANSWERS: &'static [i32] = &[0, 1];
    const ERRORS: &'static [&'static str] =
        &["WEB-101", "WEB-102", "WEB-103", "WEB-104", "WEB-105", "WEB-106"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Plan a website";
    type Args = Site;

    fn argv(args: &Site) -> Vec<String> {
        args.command(&["--dry-run", "--json"])
    }

    fn subject(args: &Site) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Site> {
        Examples { good: vec![example()], bad: bad_sites(), undo: None }
    }
}

/// `website add NAME ... --progress`: makes the site, with its folder and
/// its file, as a task that tells each step. Its undo is the removal,
/// which keeps the folder.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "websites.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/websites/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] =
        &["WEB-101", "WEB-102", "WEB-103", "WEB-104", "WEB-105", "WEB-106"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Add a website";
    type Args = Site;

    fn argv(args: &Site) -> Vec<String> {
        args.command(&["--progress"])
    }

    fn subject(args: &Site) -> Option<String> {
        Some(args.name.clone())
    }

    /// The command, run only when the site's file is not there: a second run
    /// of the play changes nothing.
    fn ansible(args: &Site) -> Ansible {
        let mut task = command_task::<Self>(args);
        task.params.insert("creates".into(), site_file(&args.name).into());
        task
    }

    fn files(args: &Site) -> Vec<String> {
        files_of(&args.name)
    }

    fn probe(_: &Site) -> Option<Vec<String>> {
        Some(list_argv())
    }

    /// The site's names, kind, folder and certificate in the list, or `none`
    /// when it has no such site.
    fn probed(args: &Site, output: &str) -> Option<String> {
        picked(&args.name, output, &["names", "kind", "folder", "https", "certificate"])
    }

    /// An add of a site that is there is refused and moves nothing.
    fn settled(_: &Site, now: &str) -> Option<bool> {
        Some(now != "none")
    }

    fn examples() -> Examples<Site> {
        Examples {
            good: vec![example()],
            bad: bad_sites(),
            // The site's folder stays, which the page says.
            undo: Some(Undo { action: "websites.remove", args: json!({ "name": EXAMPLE }) }),
        }
    }
}

/// What `websites.https` does to a site: serves it over HTTPS with a
/// certificate, or over HTTP only again.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Securing {
    /// The site.
    pub name: String,
    /// The certificate that serves it over HTTPS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
    /// Whether to serve it over HTTP only again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
}

impl Args for Securing {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::text(
            "certificate",
            "The certificate that serves the site over HTTPS, as cert list shows it: it must name every name of the site. Not with off.",
            1,
            CERTIFICATE_NAME_MAX,
            Some(certificate::NAME_PATTERN),
        )
        .optional(),
        Field::flag(
            "off",
            "Whether to serve the site over HTTP only again, which gives back the certificate. Not with certificate.",
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_site(&self.name)?;
        match (&self.certificate, self.off) {
            (Some(certificate), false) => certificate::check_name_of("certificate", certificate),
            (None, true) => Ok(()),
            _ => Err(Bad {
                field: "certificate",
                why: refusal("give the certificate to serve the site with, or off, one of them"),
            }),
        }
    }
}

/// `website https NAME (--certificate C | --off) --progress`: serves the
/// site over HTTPS with the certificate, or over HTTP only again, as a
/// task. Its undo is the other of the two.
pub struct Https;

impl Action for Https {
    const ID: &'static str = "websites.https";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/websites/{name}/https";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] =
        &["WEB-102", "WEB-103", "WEB-104", "WEB-105", "WEB-106", "WEB-107"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Serve a website over HTTPS";
    type Args = Securing;

    fn argv(args: &Securing) -> Vec<String> {
        let mut argv = ["website", "https", args.name.as_str()].map(String::from).to_vec();
        match &args.certificate {
            Some(certificate) => argv.extend(["--certificate".to_string(), certificate.clone()]),
            None => argv.push("--off".into()),
        }
        argv.push("--progress".into());
        argv
    }

    fn subject(args: &Securing) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(args: &Securing) -> Vec<String> {
        files_of(&args.name)
    }

    fn probe(_: &Securing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    /// Whether the site is served over HTTPS and with which certificate, or
    /// `none` when it has no such site.
    fn probed(args: &Securing, output: &str) -> Option<String> {
        picked(&args.name, output, &["https", "certificate"])
    }

    /// The site is as the change leaves it when it has that certificate, or
    /// serves HTTP only. A site that is not there, which the change would
    /// refuse, is not told.
    fn settled(args: &Securing, now: &str) -> Option<bool> {
        let state: Vec<Value> = serde_json::from_str(now).ok()?;
        let [https, certificate] = state.as_slice() else { return None };
        Some(match &args.certificate {
            Some(wanted) => https == &json!(true) && certificate == &json!(wanted),
            None => https == &json!(false),
        })
    }

    fn examples() -> Examples<Securing> {
        Examples {
            good: vec![Securing {
                name: EXAMPLE.into(),
                certificate: Some("this-machine".into()),
                off: false,
            }],
            bad: vec![
                json!({}),
                json!({ "name": EXAMPLE }),
                json!({ "name": EXAMPLE, "off": false }),
                json!({ "name": EXAMPLE, "certificate": "this-machine", "off": true }),
                json!({ "name": "Shop", "certificate": "this-machine" }),
                json!({ "name": "a..b", "off": true }),
                json!({ "name": EXAMPLE, "certificate": "This-Machine" }),
                json!({ "name": EXAMPLE, "certificate": "../x" }),
                json!({ "name": EXAMPLE, "off": "yes" }),
                json!({ "name": EXAMPLE, "certificate": "this-machine", "port": 443 }),
            ],
            undo: Some(Undo {
                action: "websites.https",
                args: json!({ "name": EXAMPLE, "off": true }),
            }),
        }
    }
}

/// A site to remove, and whether its folder goes with it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Removing {
    /// The site.
    pub name: String,
    /// Whether to remove its folder and what is in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub files: bool,
}

impl Args for Removing {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::flag(
            "files",
            "Whether to remove the site's folder and all that is in it, which nothing brings back.",
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_site(&self.name)
    }
}

/// `website remove NAME [--files] --progress`: takes the site away, and
/// with `files` its folder too, as a task. The folder is the person's,
/// so nothing puts it back: the playbook has to say it is meant too, in
/// `dukovo_confirm_purge`.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "websites.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/websites/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["WEB-103"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Remove a website";
    type Args = Removing;

    fn argv(args: &Removing) -> Vec<String> {
        let mut argv = ["website", "remove", args.name.as_str()].map(String::from).to_vec();
        if args.files {
            argv.push("--files".into());
        }
        argv.push("--progress".into());
        argv
    }

    fn subject(args: &Removing) -> Option<String> {
        Some(args.name.clone())
    }

    /// The command, run only when the site's file is there, and with the
    /// folder only when the playbook says so too.
    fn ansible(args: &Removing) -> Ansible {
        let mut task = command_task::<Self>(args);
        task.params.insert("removes".into(), site_file(&args.name).into());
        if args.files { task.when("dukovo_confirm_purge | bool") } else { task }
    }

    fn files(args: &Removing) -> Vec<String> {
        files_of(&args.name)
    }

    fn probe(_: &Removing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    /// The site's names in the list, or `none` when it has no such site.
    fn probed(args: &Removing, output: &str) -> Option<String> {
        picked(&args.name, output, &["names"])
    }

    /// The site is gone when the list has no row of it.
    fn settled(_: &Removing, now: &str) -> Option<bool> {
        Some(now == "none")
    }

    fn examples() -> Examples<Removing> {
        Examples {
            good: vec![
                Removing { name: EXAMPLE.into(), files: false },
                Removing { name: EXAMPLE.into(), files: true },
            ],
            bad: vec![
                json!({}),
                json!({ "name": "Shop" }),
                json!({ "name": "a..b" }),
                json!({ "name": "../x" }),
                json!({ "name": EXAMPLE, "files": "yes" }),
                json!({ "name": EXAMPLE, "folder": "/shared/www/sites/x" }),
            ],
            // The site is made again by its add, which the page's form has.
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{REGISTRY, Registered, show};
    use super::*;
    use crate::admind::errors::Kind;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// The arguments of a site, with `more` set over them.
    fn site(more: Value) -> Value {
        let mut args = json!({ "name": EXAMPLE, "kind": "php", "owner": OWNER });
        if let (Some(args), Value::Object(more)) = (args.as_object_mut(), more) {
            args.extend(more);
        }
        args
    }

    /// What `website list --json` prints for two sites, the second over
    /// HTTPS.
    const LIST: &str = r##"{
        "format": 1,
        "nginx": {"installed": true, "enabled": true, "running": true},
        "php_fpm": {"installed": false, "enabled": false, "running": false},
        "websites": [
            {"name": "blog.lab.example", "names": ["blog.lab.example"], "kind": "static",
             "folder": "/shared/www/sites/blog.lab.example", "https": false, "certificate": null,
             "by_hand": false, "text": "# the file\n"},
            {"name": "shop.lab.example", "names": ["shop.lab.example", "www.shop.lab.example"],
             "kind": "php", "folder": "/shared/www/sites/shop.lab.example", "https": true,
             "certificate": "this-machine", "by_hand": false, "text": "# the file\n"}
        ]
    }"##;

    #[test]
    fn the_reads_are_the_commands_json_and_the_checks_run_as_an_administrator() {
        for (id, path, argv, args, runs) in [
            (
                "websites.list",
                "/api/1/websites",
                vec!["website", "list", "--json"],
                json!({}),
                Runs::AsThePerson,
            ),
            (
                "websites.check",
                "/api/1/websites/check",
                vec!["website", "check", "--json"],
                json!({}),
                Runs::AsAdministrator,
            ),
            (
                "websites.plan",
                "/api/1/websites/plan/{name}",
                vec![
                    "website",
                    "add",
                    EXAMPLE,
                    "--kind",
                    "static",
                    "--owner",
                    OWNER,
                    "--dry-run",
                    "--json",
                ],
                json!({ "name": EXAMPLE, "kind": "static", "owner": OWNER }),
                Runs::AsAdministrator,
            ),
        ] {
            let read = action(id);
            assert_eq!(
                (read.changes(), read.runs(), read.path(), read.long(), read.posts()),
                (false, runs, path, false, false),
                "{id}"
            );
            assert!(!read.compares(), "{id}");
            let prepared = read.prepare(args).unwrap();
            assert_eq!(prepared.argv, argv, "{id}");
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{id}");
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
            assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"), "{id}");
        }
        // A check and a plan end with 1 when what they look at failed, and
        // print what failed: both statuses are the answer.
        assert_eq!(action("websites.list").answers(), [0]);
        assert_eq!(action("websites.check").answers(), [0, 1]);
        assert_eq!(action("websites.plan").answers(), [0, 1]);
        assert_eq!(action("websites.check").errors(), ["WEB-102", "WEB-103"]);
        assert_eq!(
            action("websites.plan").errors(),
            ["WEB-101", "WEB-102", "WEB-103", "WEB-104", "WEB-105", "WEB-106"]
        );
        assert!(action("websites.list").errors().is_empty());
        for id in ["websites.list", "websites.check"] {
            assert_eq!(refused(id, json!({ "name": EXAMPLE })).0, Kind::BadRequest, "{id}");
        }
        // The administrators' reads are tried by the test that calls every
        // route; the changes need nginx, which the Websites page's does.
        for id in ["websites.list", "websites.check", "websites.plan"] {
            assert_eq!(action(id).tests().route, "admin-actions", "{id}");
        }
        for id in ["websites.add", "websites.https", "websites.remove"] {
            assert_eq!(action(id).tests().route, "admin-websites", "{id}");
            assert_eq!(action(id).tests().same_as_command, "admin-websites", "{id}");
        }
    }

    #[test]
    fn the_changes_are_tasks_that_an_administrator_runs_and_every_area_is_websites() {
        for (id, path) in [
            ("websites.add", "/api/1/websites/add"),
            ("websites.https", "/api/1/websites/{name}/https"),
            ("websites.remove", "/api/1/websites/{name}/remove"),
        ] {
            let change = action(id);
            assert_eq!(
                (change.changes(), change.runs(), change.long(), change.path(), change.posts()),
                (true, Runs::AsAdministrator, true, path, true),
                "{id}"
            );
            assert!(change.compares(), "{id}");
            assert_eq!(change.answers(), [0], "{id}");
            assert!(!change.errors().is_empty(), "{id}");
        }
        for id in [
            "websites.list",
            "websites.check",
            "websites.plan",
            "websites.add",
            "websites.https",
            "websites.remove",
        ] {
            assert_eq!(action(id).area(), "websites", "{id}");
            assert_eq!(action(id).tests().checks, "admind::actions::websites::tests", "{id}");
        }
        // Nothing of these cuts off the browser or restarts the machine.
        assert!(REGISTRY.iter().filter(|each| each.area() == "websites").all(|each| {
            each.cuts_off_on().is_none() && each.restarts().is_none() && !each.secret_answer()
        }));
    }

    #[test]
    fn a_site_is_planned_and_added_with_each_of_its_options_in_the_commands_words() {
        let args = site(json!({
            "names": "www.shop.lab.example\nshop2.lab.example",
            "folder": "/shared/www/sites/shop",
            "certificate": "this-machine",
        }));
        let words = [
            "website",
            "add",
            EXAMPLE,
            "--name",
            "www.shop.lab.example",
            "--name",
            "shop2.lab.example",
            "--kind",
            "php",
            "--folder",
            "/shared/www/sites/shop",
            "--owner",
            OWNER,
            "--certificate",
            "this-machine",
        ];
        let plan = [&words[..], &["--dry-run", "--json"]].concat();
        assert_eq!(argv("websites.plan", args.clone()), plan);
        let add = [&words[..], &["--progress"]].concat();
        assert_eq!(argv("websites.add", args.clone()), add);
        // What the page leaves out is left out of the command.
        assert_eq!(
            argv("websites.add", site(json!({}))),
            ["website", "add", EXAMPLE, "--kind", "php", "--owner", OWNER, "--progress"]
        );
        // As typed at a terminal, which an administrator needs elevate for.
        let prepared = action("websites.add").prepare(args).unwrap();
        assert_eq!(
            show::command(&prepared),
            "elevate website add shop.lab.example --name www.shop.lab.example --name \
             shop2.lab.example --kind php --folder /shared/www/sites/shop --owner webdev \
             --certificate this-machine --progress"
        );
        for id in ["websites.plan", "websites.add"] {
            assert_eq!(action(id).prepare(site(json!({}))).unwrap().args, site(json!({})), "{id}");
        }
    }

    #[test]
    fn the_names_of_a_site_are_the_rule_of_certificates_and_a_site_is_named_by_its_first() {
        for id in ["websites.plan", "websites.add"] {
            for name in [
                "a",
                "0",
                "a-b",
                "a.b-c.example",
                "xn--caf-dma.example",
                "1.2.3",
                &format!("{}.example", "a".repeat(63)),
                &"a".repeat(63),
            ] {
                assert_eq!(argv(id, site(json!({ "name": name })))[2], name, "{id} {name}");
            }
            for name in [
                "",
                "-x",
                "x-",
                ".x",
                "x.",
                "a..b",
                "*.example.org",
                "Shop.example.org",
                "shop_example",
                "a b",
                "a/b",
                "../x",
                "caf\u{e9}.example",
                "a\n",
                "a\nb",
                "a;b",
                "a$(b)",
                "10.0.0.1",
                "127.0.0.1",
                "::1",
                &"a".repeat(64),
                &format!("{}.example", "a".repeat(64)),
                // A site's name is its file's too: 250 characters at most.
                &format!(
                    "{}.{}.{}.{}.a",
                    "a".repeat(63),
                    "b".repeat(63),
                    "c".repeat(63),
                    "d".repeat(59)
                ),
            ] {
                let (kind, field, words) = refused(id, site(json!({ "name": name })));
                assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{id} {name:?}");
                assert!(words.ends_with("Nothing was done."), "{words}");
                // The words never hold what was typed.
                assert!(name.is_empty() || !words.contains(name), "{words}");
            }
        }
        // 250 characters is the longest, and 251 is refused.
        let longest =
            format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(58));
        assert_eq!(longest.len(), 250);
        assert_eq!(argv("websites.add", site(json!({ "name": longest })))[2], longest);
        let (kind, field, _) =
            refused("websites.add", site(json!({ "name": format!("{longest}d") })));
        assert_eq!((kind, field), (Kind::BadValue, Some("name")));
    }

    #[test]
    fn the_other_names_are_one_a_line_at_most_nineteen_each_a_name_and_none_twice() {
        let good = (0..19).map(|n| format!("n{n}.lab.example")).collect::<Vec<_>>().join("\n");
        let prepared = argv("websites.add", site(json!({ "names": good })));
        assert_eq!(prepared.iter().filter(|word| *word == "--name").count(), 19);
        for names in [
            String::new(),
            "\n".into(),
            "a\n".into(),
            "\na".into(),
            "a\n\nb".into(),
            "a\r\nb".into(),
            "a b".into(),
            "-a".into(),
            "a-".into(),
            "a..b".into(),
            "*.a".into(),
            "A".into(),
            "a,b".into(),
            "a;b".into(),
            "$(a)".into(),
            "--kind".into(),
            "10.0.0.1".into(),
            EXAMPLE.into(),
            "a\nb\na".into(),
            format!("a\n{EXAMPLE}"),
            (0..20).map(|n| format!("n{n}.lab.example")).collect::<Vec<_>>().join("\n"),
            "a".repeat(NAMES_CHARS + 1),
            "a".repeat(254),
        ] {
            for id in ["websites.plan", "websites.add"] {
                let (kind, field, words) = refused(id, site(json!({ "names": names })));
                assert_eq!((kind, field), (Kind::BadValue, Some("names")), "{id} {names:?}");
                assert!(words.ends_with("Nothing was done."), "{words}");
            }
        }
        assert_eq!(NAMES_CHARS, 4826);
        // The pattern of the field counts what the code does: 19 more.
        assert!(LINE_NAMES.ends_with("{0,18}$"));
        assert_eq!(NAMES_MAX, 20);
    }

    #[test]
    fn a_kind_is_static_or_php_a_folder_is_under_the_sites_and_an_owner_is_an_account() {
        for kind in ["static", "php"] {
            let words = argv("websites.add", site(json!({ "kind": kind })));
            assert_eq!((words[3].as_str(), words[4].as_str()), ("--kind", kind));
        }
        for kind in ["", "cgi", "Static", "static ", "php\n", "static,php"] {
            let (error, field, _) = refused("websites.add", site(json!({ "kind": kind })));
            assert_eq!((error, field), (Kind::BadValue, Some("kind")), "{kind:?}");
        }
        for folder in [
            "/shared/www/sites/a",
            "/shared/www/sites/a/b",
            "/shared/www/sites/_x/y.z",
            "/shared/www/sites/a@b+c-d_e.f",
            "/shared/www/sites/shop.lab.example",
        ] {
            let words = argv("websites.add", site(json!({ "folder": folder })));
            assert_eq!(
                words[words.iter().position(|word| word == "--folder").unwrap() + 1],
                folder
            );
        }
        let long_part = "a".repeat(256);
        let too_long = format!("/shared/www/sites/{}", vec!["a".repeat(250); 5].join("/"));
        for folder in [
            "",
            "/",
            "/etc",
            "shared/www/sites/a",
            "/shared/www/sites",
            "/shared/www/sites/",
            "/shared/www/sites//a",
            "/shared/www/sites/a/",
            "/shared/www/sites/..",
            "/shared/www/sites/../etc",
            "/shared/www/sites/a/../../b",
            "/shared/www/sites/./a",
            "/shared/www/sites/.hidden",
            "/shared/www/sites/-a",
            "/shared/www/sites/a b",
            "/shared/www/sites/a\"b",
            "/shared/www/sites/a'b",
            "/shared/www/sites/a;b",
            "/shared/www/sites/a$b",
            "/shared/www/sites/a`b`",
            "/shared/www/sites/a\\b",
            "/shared/www/sites/a{b}",
            "/shared/www/sites/a#b",
            "/shared/www/sites/a\nb",
            "/shared/www/sites/a\0b",
            "/shared/www/sites/caf\u{e9}",
            "/shared/www/sitesx/a",
            "/shared/www/sites/a\n    root /etc;",
            &format!("/shared/www/sites/{long_part}"),
            too_long.as_str(),
        ] {
            let (error, field, words) = refused("websites.add", site(json!({ "folder": folder })));
            assert_eq!((error, field), (Kind::BadValue, Some("folder")), "{folder:?}");
            assert!(words.ends_with("Nothing was done."), "{words}");
        }
        assert!(FOLDER_PATTERN.starts_with("^/shared/www/sites/"));
        for owner in ["webdev", "_x", "a.b", "a-b", "a_b", "a1", &"a".repeat(32)] {
            let words = argv("websites.add", site(json!({ "owner": owner })));
            assert_eq!(words[words.iter().position(|word| word == "--owner").unwrap() + 1], owner);
        }
        for owner in [
            "",
            "Webdev",
            "-a",
            ".a",
            "1a",
            "a b",
            "a;b",
            "a$b",
            "a/b",
            "../x",
            "a\n",
            "caf\u{e9}",
            &"a".repeat(33),
            "--owner",
        ] {
            let (error, field, _) = refused("websites.add", site(json!({ "owner": owner })));
            assert_eq!((error, field), (Kind::BadValue, Some("owner")), "{owner:?}");
        }
        for certificate in
            ["", "This", "-x", ".x", "a..b", "a_b", "a b", "a/b", "../x", &"a".repeat(65)]
        {
            let (error, field, _) =
                refused("websites.add", site(json!({ "certificate": certificate })));
            assert_eq!((error, field), (Kind::BadValue, Some("certificate")), "{certificate:?}");
        }
        for certificate in ["this-machine", "shop.lab.example", "a", &"a".repeat(64)] {
            let words = argv("websites.add", site(json!({ "certificate": certificate })));
            assert!(words.contains(&certificate.to_string()));
        }
    }

    #[test]
    fn no_value_reaches_the_command_as_an_option_or_as_more_than_one_word() {
        // Every value a field takes is a plain word, which starts with a
        // letter, a digit, an underscore or a slash and holds no space,
        // quote or line end, so that a command line is exactly the words
        // it was made of.
        let args = site(json!({
            "names": "a-b.example\n0.example",
            "folder": "/shared/www/sites/_a/b@c",
            "certificate": "this-machine",
        }));
        for id in ["websites.plan", "websites.add"] {
            let words = argv(id, args.clone());
            for word in &words {
                assert!(
                    word.starts_with("--")
                        || word.starts_with(|c: char| c.is_ascii_alphanumeric()
                            || c == '_'
                            || c == '/'),
                    "{id}: {word}"
                );
                assert!(
                    !word.contains(|c: char| c.is_whitespace() || "'\"`$;&|<>\\*?#()".contains(c)),
                    "{word}"
                );
            }
            // The options are the command's own; the rest are values.
            let options: Vec<&String> = words.iter().filter(|word| word.starts_with('-')).collect();
            assert!(options.iter().all(|word| {
                [
                    "--name",
                    "--kind",
                    "--folder",
                    "--owner",
                    "--certificate",
                    "--dry-run",
                    "--json",
                    "--progress",
                ]
                .contains(&word.as_str())
            }));
        }
        // The shown command is the words as they are, quoted for nothing.
        let shown = show::command(&action("websites.add").prepare(args).unwrap());
        assert!(!shown.contains(['\'', '"']), "{shown}");
        for name in [
            "a;b", "a b", "$(x)", "`x`", "--files", "-x", "a\nb", "a'b", "a\"b", "a|b", "a&b",
            "a>b",
        ] {
            for (id, args) in [
                ("websites.https", json!({ "name": name, "off": true })),
                ("websites.remove", json!({ "name": name })),
            ] {
                let (error, field, _) = refused(id, args);
                assert_eq!((error, field), (Kind::BadValue, Some("name")), "{id} {name:?}");
            }
        }
    }

    #[test]
    fn https_takes_a_certificate_or_off_and_never_both_or_neither() {
        assert_eq!(
            argv("websites.https", json!({ "name": EXAMPLE, "certificate": "this-machine" })),
            ["website", "https", EXAMPLE, "--certificate", "this-machine", "--progress"]
        );
        assert_eq!(
            argv("websites.https", json!({ "name": EXAMPLE, "off": true })),
            ["website", "https", EXAMPLE, "--off", "--progress"]
        );
        assert_eq!(
            argv("websites.https", json!({ "name": EXAMPLE, "certificate": "c", "off": false })),
            ["website", "https", EXAMPLE, "--certificate", "c", "--progress"]
        );
        for args in [
            json!({ "name": EXAMPLE }),
            json!({ "name": EXAMPLE, "off": false }),
            json!({ "name": EXAMPLE, "certificate": "this-machine", "off": true }),
        ] {
            let (error, field, words) = refused("websites.https", args.clone());
            assert_eq!((error, field), (Kind::BadValue, Some("certificate")), "{args}");
            assert!(words.ends_with("Nothing was done."), "{words}");
        }
        for certificate in ["", "This", "../x", "a b", "a;b", "--off"] {
            let (error, field, _) =
                refused("websites.https", json!({ "name": EXAMPLE, "certificate": certificate }));
            assert_eq!((error, field), (Kind::BadValue, Some("certificate")), "{certificate:?}");
        }
        assert_eq!(refused("websites.https", json!({ "certificate": "c" })).0, Kind::BadRequest);
        assert_eq!(
            refused("websites.https", json!({ "name": EXAMPLE, "off": "yes" })).0,
            Kind::BadRequest
        );
        assert_eq!(
            action("websites.https").errors(),
            ["WEB-102", "WEB-103", "WEB-104", "WEB-105", "WEB-106", "WEB-107"]
        );
        // What puts HTTPS back is HTTP only, and the other way round.
        let undo = action("websites.https").example_values().2.unwrap();
        assert_eq!(
            (undo.action, undo.args),
            ("websites.https", json!({ "name": EXAMPLE, "off": true }))
        );
    }

    #[test]
    fn a_removal_names_the_site_and_the_folder_goes_only_when_it_is_asked_and_the_playbook_confirms()
     {
        assert_eq!(
            argv("websites.remove", json!({ "name": EXAMPLE })),
            ["website", "remove", EXAMPLE, "--progress"]
        );
        assert_eq!(
            argv("websites.remove", json!({ "name": EXAMPLE, "files": true })),
            ["website", "remove", EXAMPLE, "--files", "--progress"]
        );
        assert_eq!(
            argv("websites.remove", json!({ "name": EXAMPLE, "files": false })),
            ["website", "remove", EXAMPLE, "--progress"]
        );
        let keep = action("websites.remove").prepare(json!({ "name": EXAMPLE })).unwrap();
        let purge =
            action("websites.remove").prepare(json!({ "name": EXAMPLE, "files": true })).unwrap();
        assert_eq!(keep.ansible.when, None);
        assert_eq!(purge.ansible.when.as_deref(), Some("dukovo_confirm_purge | bool"));
        let yaml = show::yaml(&purge.ansible);
        assert!(yaml.ends_with("  when: \"dukovo_confirm_purge | bool\"\n"), "{yaml}");
        assert!(!show::yaml(&keep.ansible).contains("when:"));
        assert_eq!(action("websites.remove").errors(), ["WEB-103"]);
        // Nothing puts a removed site back: its form makes it again.
        assert_eq!(action("websites.remove").example_values().2, None);
        for files in [json!("yes"), json!(1), json!([])] {
            let args = json!({ "name": EXAMPLE, "files": files });
            assert_eq!(refused("websites.remove", args).0, Kind::BadRequest);
        }
    }

    #[test]
    fn an_add_runs_in_the_playbook_only_while_the_site_is_not_there_and_a_removal_while_it_is() {
        let add = action("websites.add").prepare(site(json!({}))).unwrap();
        assert_eq!(
            add.ansible.params.get("creates"),
            Some(&json!("/system/settings/websites/shop.lab.example.conf"))
        );
        assert!(add.ansible.become_root && add.ansible.changed_when.is_none());
        assert_eq!(add.ansible.name, "Add a website: shop.lab.example");
        let yaml = show::yaml(&add.ansible);
        assert!(
            yaml.contains("creates: \"/system/settings/websites/shop.lab.example.conf\""),
            "{yaml}"
        );
        assert!(yaml.contains("ansible.builtin.command"), "{yaml}");
        let remove = action("websites.remove").prepare(json!({ "name": EXAMPLE })).unwrap();
        assert_eq!(
            remove.ansible.params.get("removes"),
            Some(&json!("/system/settings/websites/shop.lab.example.conf"))
        );
        assert_eq!(remove.ansible.name, "Remove a website: shop.lab.example");
        // The change that is not told by a file has none.
        let https = action("websites.https")
            .prepare(json!({ "name": EXAMPLE, "certificate": "this-machine" }))
            .unwrap();
        assert!(https.ansible.params.get("creates").is_none());
        assert!(https.ansible.params.get("removes").is_none());
    }

    #[test]
    fn a_change_names_the_site_s_file_the_default_server_s_and_nginx_s_override() {
        let files = [
            "/system/settings/websites/shop.lab.example.conf",
            "/system/settings/websites/_default.conf",
            "/system/settings/spark/services/settings.d/nginx.toml",
        ];
        for (id, args) in [
            ("websites.add", site(json!({}))),
            ("websites.https", json!({ "name": EXAMPLE, "off": true })),
            ("websites.remove", json!({ "name": EXAMPLE })),
        ] {
            let prepared = action(id).prepare(args).unwrap();
            assert_eq!(prepared.files, files, "{id}");
            assert_eq!(
                prepared.probe,
                Some(vec!["website".into(), "list".into(), "--json".into()]),
                "{id}"
            );
        }
        assert_eq!(OVERRIDE, files[2]);
        assert_eq!(format!("{SETTINGS}/{DEFAULT_FILE}"), files[1]);
    }

    #[test]
    fn the_probe_picks_what_each_change_moves_of_the_site_s_row() {
        let add = site(json!({ "name": "shop.lab.example" }));
        let picked = action("websites.add").probed(&add, LIST).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&picked).unwrap(),
            json!([
                ["shop.lab.example", "www.shop.lab.example"],
                "php",
                "/shared/www/sites/shop.lab.example",
                true,
                "this-machine"
            ])
        );
        // A file that a hand changed, or its text, is not what moves.
        let by_hand = LIST.replace("\"by_hand\": false", "\"by_hand\": true");
        assert_eq!(action("websites.add").probed(&add, &by_hand), Some(picked));
        let blog = site(json!({ "name": "blog.lab.example" }));
        assert!(action("websites.add").probed(&blog, LIST).unwrap().contains("static"));
        let other = site(json!({ "name": "other.lab.example" }));
        assert_eq!(action("websites.add").probed(&other, LIST), Some("none".into()));
        let https = json!({ "name": "shop.lab.example", "off": true });
        assert_eq!(
            action("websites.https").probed(&https, LIST),
            Some(r#"[true,"this-machine"]"#.into())
        );
        let blog = json!({ "name": "blog.lab.example", "off": true });
        assert_eq!(action("websites.https").probed(&blog, LIST), Some("[false,null]".into()));
        let remove = json!({ "name": "blog.lab.example" });
        assert_eq!(
            action("websites.remove").probed(&remove, LIST),
            Some(r#"[["blog.lab.example"]]"#.into())
        );
        assert_eq!(
            action("websites.remove").probed(&json!({ "name": "none.example" }), LIST),
            Some("none".into())
        );
        // A list that does not read says nothing, and so do arguments that
        // are not the action's.
        for output in ["", "not json", "[]", r#"{"websites": 3}"#, r#"{"format": 1}"#] {
            assert_eq!(action("websites.remove").probed(&remove, output), None, "{output:?}");
        }
        assert_eq!(action("websites.remove").probed(&json!({ "name": 3 }), LIST), None);
    }

    #[test]
    fn check_mode_knows_a_site_that_is_there_a_certificate_that_is_served_and_a_site_that_is_gone()
    {
        let add = site(json!({}));
        assert_eq!(action("websites.add").settled(&add, "none"), Some(false));
        assert_eq!(
            action("websites.add").settled(&add, "[[],\"php\",\"/x\",false,null]"),
            Some(true)
        );
        let remove = json!({ "name": EXAMPLE });
        assert_eq!(action("websites.remove").settled(&remove, "none"), Some(true));
        assert_eq!(action("websites.remove").settled(&remove, "[[\"a\"]]"), Some(false));
        let with = json!({ "name": EXAMPLE, "certificate": "this-machine" });
        assert_eq!(action("websites.https").settled(&with, r#"[true,"this-machine"]"#), Some(true));
        assert_eq!(action("websites.https").settled(&with, r#"[true,"other"]"#), Some(false));
        assert_eq!(action("websites.https").settled(&with, "[false,null]"), Some(false));
        let off = json!({ "name": EXAMPLE, "off": true });
        assert_eq!(action("websites.https").settled(&off, "[false,null]"), Some(true));
        assert_eq!(action("websites.https").settled(&off, r#"[true,"this-machine"]"#), Some(false));
        // A site that is not there is a change that is refused, not one that
        // is done.
        assert_eq!(action("websites.https").settled(&off, "none"), None);
        assert_eq!(action("websites.https").settled(&off, "[1]"), None);
    }

    #[test]
    fn the_examples_are_the_ones_the_boot_test_s_machine_can_try_and_the_bad_ones_do_not_work() {
        for id in [
            "websites.list",
            "websites.check",
            "websites.plan",
            "websites.add",
            "websites.https",
            "websites.remove",
        ] {
            let (good, bad, undo) = action(id).example_values();
            assert!(!good.is_empty() && !bad.is_empty(), "{id}");
            for args in &good {
                assert!(action(id).prepare(args.clone()).is_ok(), "{id} {args}");
            }
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id} {args}");
            }
            if let Some(undo) = undo {
                assert!(action(undo.action).changes(), "{id}");
            }
        }
        let (good, _, undo) = action("websites.plan").example_values();
        assert_eq!(good, [json!({ "name": EXAMPLE, "kind": "static", "owner": OWNER })]);
        assert_eq!(undo, None);
        let (good, _, undo) = action("websites.add").example_values();
        assert_eq!(good, [json!({ "name": EXAMPLE, "kind": "static", "owner": OWNER })]);
        assert_eq!(undo.unwrap().action, "websites.remove");
        let (good, _, _) = action("websites.https").example_values();
        assert_eq!(good, [json!({ "name": EXAMPLE, "certificate": "this-machine" })]);
    }

    #[test]
    fn the_fields_are_what_the_code_takes_and_the_schema_says() {
        // The patterns hold the prefix and the counts the code has.
        assert!(NAME_PATTERN.starts_with("^[a-z0-9]"));
        assert!(FOLDER_PATTERN.starts_with(&format!("^{}/", "/shared/www/sites")));
        assert_eq!(NAME.name, "name");
        let fields = action("websites.plan").fields();
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(names, ["name", "names", "kind", "folder", "owner", "certificate"]);
        let required: Vec<&str> =
            fields.iter().filter(|field| field.required).map(|field| field.name).collect();
        assert_eq!(required, ["name", "kind", "owner"]);
        let fields = action("websites.https").fields();
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(names, ["name", "certificate", "off"]);
        let fields = action("websites.remove").fields();
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(names, ["name", "files"]);
    }
}
