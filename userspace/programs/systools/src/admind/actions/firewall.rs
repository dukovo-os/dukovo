// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firewall's actions (ADR 0518): the rules, what a connection would
//! meet, what the kernel lets in and what it dropped, and adding,
//! replacing and removing a rule with a window to undo it. The area is
//! `firewall`, the scope a token needs.
//!
//! `firewall.list` and `firewall.test` run as the person, who reads the
//! settings file. `firewall.status` and `firewall.dropped` run as an
//! administrator: the kernel's table of rules, and the kernel's log, are
//! root's. The changes run as an administrator, as `elevate firewall ...`
//! does at a terminal.
//!
//! A rule is five fields (`action`, `protocol`, `ports`, `from`, `on`) that
//! the command's own parser, [`crate::firewall::Rule::parse`], reads as the
//! words of a rule in the order firewall(8) writes them: `allow tcp 443
//! from 192.168.1.0/24 on net0`. The argv has the rule as the command
//! prints it, so that what is shown, what `firewall list` says of the rule
//! afterwards and what `remove` and `replace` find the rule by are the same
//! words. `replace` takes two rules, `old_` and `new_`. With `try` the
//! change is tried (the trial `firewall`): the argv ends with `--try
//! SECONDS` and Spark undoes the change unless `firewall.keep` keeps it.
//!
//! A change says whether it changed anything by reading `firewall list
//! --json` before and after, and comparing the texts of the rules, which
//! are what the file holds; the problems and the trial it lists are not
//! the rules.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::trial::{TRY, check_try, refusal, try_words};
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Trial, Undo};
use crate::firewall::{Rule, SETTINGS, TRIAL};

/// The area of these actions: the scope a token needs, `firewall`.
const AREA: &str = "firewall";

/// The tests of the reads, which `lab/actions.toml` names.
const READ_TESTS: Tests = Tests {
    checks: "admind::actions::firewall::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/firewall",
    ansible: "web-console/ansible",
};

/// The tests of the changes: the trials' boot test calls their routes.
const CHANGE_TESTS: Tests = Tests {
    checks: "admind::actions::firewall::tests",
    route: "admin-trials",
    same_as_command: "admin-actions",
    browser: "web-console/firewall",
    ansible: "web-console/ansible",
};

/// What a rule does, and what it matches.
const ACTIONS: [&str; 2] = ["allow", "deny"];
const PROTOCOLS: [&str; 4] = ["any", "tcp", "udp", "ping"];

/// What a test of a connection asks about: the two that have ports.
const TESTED: [&str; 2] = ["tcp", "udp"];

/// How far back the dropped connections are looked for.
const SINCE: [&str; 3] = ["1h", "24h", "7d"];

/// What the schema says of the fields of a rule: the shape, which the
/// command's own check holds to the exact rule. A port is a number or
/// two, an address or network has the characters of one, and an interface
/// has a name of 1 to 15 letters, digits, `.`, `-` and `_`, as a card's
/// name does.
const PORTS_PATTERN: &str = "^[0-9]+(-[0-9]+)?$";
const FROM_PATTERN: &str = "^[0-9A-Fa-f:.]+(/[0-9]+)?$";
const ADDRESS_PATTERN: &str = "^[0-9A-Fa-f:.]+$";
const ON_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]*$";

/// The most characters of an interface's name.
const ON_MAX: usize = 15;

/// The names the fields of a rule go by in an action's arguments: plain,
/// or with a prefix when an action takes two rules.
struct Names {
    action: &'static str,
    protocol: &'static str,
    ports: &'static str,
    from: &'static str,
    on: &'static str,
}

/// The fields of the one rule of `add` and `remove`.
const PLAIN: Names =
    Names { action: "action", protocol: "protocol", ports: "ports", from: "from", on: "on" };

/// The fields of the rule `replace` takes out, and of the one it puts in.
const OLD: Names = Names {
    action: "old_action",
    protocol: "old_protocol",
    ports: "old_ports",
    from: "old_from",
    on: "old_on",
};
const NEW: Names = Names {
    action: "new_action",
    protocol: "new_protocol",
    ports: "new_ports",
    from: "new_from",
    on: "new_on",
};

/// The field of whether a rule allows or denies, named `name`.
const fn action_field(name: &'static str) -> Field {
    Field::one_of(name, "Whether the rule allows or denies: allow or deny.", &ACTIONS)
}

/// The field of what a rule matches, named `name`.
const fn protocol_field(name: &'static str) -> Field {
    Field::one_of(
        name,
        "What the rule matches: any, tcp, udp or ping. A ping has no ports.",
        &PROTOCOLS,
    )
}

/// The field of a rule's ports, named `name`.
const fn ports_field(name: &'static str) -> Field {
    Field::text(
        name,
        "The ports it matches: one, as 443, or a range, as 8000-8100, from 1 to 65535. All of \
         them if left out.",
        1,
        11,
        Some(PORTS_PATTERN),
    )
    .optional()
}

/// The field of where a rule's connections come from, named `name`.
const fn from_field(name: &'static str) -> Field {
    Field::text(
        name,
        "The address or network they come from, as 192.168.1.20 or 192.168.1.0/24. Anywhere if \
         left out.",
        1,
        49,
        Some(FROM_PATTERN),
    )
    .optional()
}

/// The field of the card a rule's connections come by, named `name`.
const fn on_field(name: &'static str) -> Field {
    Field::text(
        name,
        "The card they come by, as net0. Any card if left out.",
        1,
        ON_MAX,
        Some(ON_PATTERN),
    )
    .optional()
}

/// A rule's fields as an action's arguments hold them.
struct Given<'a> {
    action: &'a str,
    protocol: &'a str,
    ports: Option<&'a str>,
    from: Option<&'a str>,
    on: Option<&'a str>,
}

impl Given<'_> {
    /// The words of the rule, in the order firewall(8) writes them. The
    /// protocol is left out for `any`, as the command prints it.
    fn words(&self) -> Vec<&str> {
        let mut words = vec![self.action];
        if self.protocol != "any" {
            words.push(self.protocol);
        }
        words.extend(self.ports);
        if let Some(from) = self.from {
            words.extend(["from", from]);
        }
        if let Some(on) = self.on {
            words.extend(["on", on]);
        }
        words
    }

    /// The rule, as the command reads the words, with `names` for the
    /// fields a refusal is about.
    ///
    /// # Errors
    ///
    /// The first field that cannot work, and why.
    fn check(&self, names: &Names) -> Result<Rule, Bad> {
        if !ACTIONS.contains(&self.action) {
            return Err(Bad { field: names.action, why: refusal("A rule allows or denies") });
        }
        if !PROTOCOLS.contains(&self.protocol) {
            let why = "A rule matches any, tcp, udp or ping";
            return Err(Bad { field: names.protocol, why: refusal(why) });
        }
        // A port is read from its first digit: another word would be read
        // as a part of the rule it is not.
        if let Some(ports) = self.ports
            && !(ports.starts_with(|c: char| c.is_ascii_digit())
                && ports.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
        {
            let why = format!("`{ports}` is not a port, 1 to 65535, or a range of them");
            return Err(Bad { field: names.ports, why: refusal(&why) });
        }
        if let Some(on) = self.on
            && !card_shape(on)
        {
            let why = format!("`{on}` is not an interface's name");
            return Err(Bad { field: names.on, why: refusal(&why) });
        }
        let rule = Rule::parse(&self.words())
            .map_err(|why| Bad { field: names.field_of(&why), why: refusal(&why) })?;
        Ok(rule)
    }

    /// The rule's words as `firewall` prints them, whatever way the fields
    /// wrote them: `192.168.1.5/32` is the address alone.
    fn argv_words(&self) -> Vec<String> {
        match Rule::parse(&self.words()) {
            Ok(rule) => rule.to_string().split(' ').map(String::from).collect(),
            Err(_) => self.words().into_iter().map(String::from).collect(),
        }
    }
}

impl Names {
    /// The field of `names` a refusal of `Rule::parse` is about.
    fn field_of(&self, why: &str) -> &'static str {
        if why.contains("address or network") || why.starts_with("`from`") {
            self.from
        } else if why.contains("interface") || why.starts_with("`on`") {
            self.on
        } else if why.contains("allow or deny") {
            self.action
        } else {
            self.ports
        }
    }
}

/// Whether `name` has the shape of an interface's name: it starts with a
/// letter or a digit, so that it is never read as an option.
fn card_shape(name: &str) -> bool {
    name.len() <= ON_MAX
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
}

/// A rule's texts out of what `firewall list --json` printed, one a line:
/// what a change of the rules moves.
fn rule_texts(output: &str) -> Option<String> {
    let document: Value = serde_json::from_str(output).ok()?;
    let texts: Option<Vec<&str>> = document
        .get("rules")?
        .as_array()?
        .iter()
        .map(|rule| rule.get("text").and_then(Value::as_str))
        .collect();
    Some(texts?.join("\n"))
}

/// The command that reads the rules, for the changes' probes.
fn probe_argv() -> Vec<String> {
    ["firewall", "list", "--json"].map(String::from).to_vec()
}

/// `firewall list --json`: the rules in the file, and what is wrong with
/// its lines.
pub struct List;

impl Action for List {
    const ID: &'static str = "firewall.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/firewall";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = READ_TESTS;
    const TITLE: &'static str = "List the firewall's rules";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        probe_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "all": true }), json!({ "number": 1 })],
            undo: None,
        }
    }
}

/// What a connection is asked about: its protocol, port and address, and
/// the card it comes by.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    /// `tcp` or `udp`.
    pub protocol: String,
    /// The port it comes to.
    pub port: i64,
    /// The address it comes from.
    pub from: String,
    /// The card it comes by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
}

impl Args for Question {
    const FIELDS: &'static [Field] = &[
        Field::one_of("protocol", "The connection's protocol: tcp or udp.", &TESTED),
        Field::number("port", "The port the connection comes to, 1 to 65535.", 1, 65535),
        Field::text(
            "from",
            "The address the connection comes from, as 192.168.1.20: not a network.",
            1,
            45,
            Some(ADDRESS_PATTERN),
        ),
        Field::text(
            "on",
            "The card it comes by, as net0. Any card if left out.",
            1,
            ON_MAX,
            Some(ON_PATTERN),
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        if !TESTED.contains(&self.protocol.as_str()) {
            return Err(Bad { field: "protocol", why: refusal("A connection is tcp or udp") });
        }
        if !(1..=65535).contains(&self.port) {
            return Err(Bad { field: "port", why: refusal("A port is 1 to 65535") });
        }
        if self.from.parse::<IpAddr>().is_err() {
            let why = format!("`{}` is not an address: a network is not one here", self.from);
            return Err(Bad { field: "from", why: refusal(&why) });
        }
        if let Some(on) = &self.on {
            if !card_shape(on) {
                let why = format!("`{on}` is not an interface's name");
                return Err(Bad { field: "on", why: refusal(&why) });
            }
            // The command's parser has the last word on a name.
            Rule::parse(&["allow", "on", on])
                .map_err(|why| Bad { field: "on", why: refusal(&why) })?;
        }
        Ok(())
    }
}

/// `firewall test tcp|udp PORT from ADDRESS [on CARD] --json`: which rule
/// of the file decides a new connection.
pub struct Test;

impl Action for Test {
    const ID: &'static str = "firewall.test";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/firewall/test";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = READ_TESTS;
    const TITLE: &'static str = "Test a connection against the firewall";
    type Args = Question;

    fn argv(args: &Question) -> Vec<String> {
        let mut argv = ["firewall", "test"].map(String::from).to_vec();
        argv.extend([
            args.protocol.clone(),
            args.port.to_string(),
            "from".into(),
            args.from.clone(),
        ]);
        if let Some(on) = &args.on {
            argv.extend(["on".to_string(), on.clone()]);
        }
        argv.push("--json".into());
        argv
    }

    fn examples() -> Examples<Question> {
        Examples {
            good: vec![
                Question {
                    protocol: "tcp".into(),
                    port: 22,
                    from: "192.168.1.20".into(),
                    on: None,
                },
                Question {
                    protocol: "udp".into(),
                    port: 53,
                    from: "2001:db8::20".into(),
                    on: Some("net0".into()),
                },
            ],
            bad: vec![
                json!({ "port": 0 }),
                json!({ "protocol": "tcp", "port": 0, "from": "192.168.1.20" }),
                json!({ "protocol": "tcp", "port": 65536, "from": "192.168.1.20" }),
                json!({ "protocol": "icmp", "port": 22, "from": "192.168.1.20" }),
                json!({ "protocol": "tcp", "port": 22, "from": "192.168.1.0/24" }),
                json!({ "protocol": "tcp", "port": 22, "from": "192.168.1.20", "on": "a/b" }),
                json!({ "all": true }),
            ],
            undo: None,
        }
    }
}

/// `firewall status --json`: what the kernel lets in, port by port.
pub struct Status;

impl Action for Status {
    const ID: &'static str = "firewall.status";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/firewall/status";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = READ_TESTS;
    const TITLE: &'static str = "Show what the firewall lets in";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["firewall", "status", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// How far back the dropped connections are looked for.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Back {
    /// `1h`, `24h` or `7d`: a day if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

impl Args for Back {
    const FIELDS: &'static [Field] = &[Field::one_of(
        "since",
        "How far back the dropped connections are looked for: 1h, 24h or 7d. A day if left out.",
        &SINCE,
    )
    .optional()];

    fn check(&self) -> Result<(), Bad> {
        match &self.since {
            Some(since) if !SINCE.contains(&since.as_str()) => {
                Err(Bad { field: "since", why: refusal("The time to look back is 1h, 24h or 7d") })
            }
            _ => Ok(()),
        }
    }
}

/// `firewall dropped --since -24h --json`: the connections the kernel
/// dropped, from its log.
pub struct Dropped;

impl Action for Dropped {
    const ID: &'static str = "firewall.dropped";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/firewall/dropped";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = READ_TESTS;
    const TITLE: &'static str = "Show what the firewall dropped";
    type Args = Back;

    fn argv(args: &Back) -> Vec<String> {
        let since = format!("-{}", args.since.as_deref().unwrap_or("24h"));
        ["firewall", "dropped", "--since", &since, "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Back> {
        Examples {
            good: vec![Back::default(), Back { since: Some("1h".into()) }],
            bad: vec![
                json!({ "since": "2h" }),
                json!({ "since": "-1h" }),
                json!({ "since": "" }),
                json!({ "all": true }),
            ],
            undo: None,
        }
    }
}

/// A rule, and how long the change that makes it is tried for.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Wanted {
    /// `allow` or `deny`.
    pub action: String,
    /// `any`, `tcp`, `udp` or `ping`.
    pub protocol: String,
    /// One port, or a range of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<String>,
    /// The address or network the connections come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The card they come by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Wanted {
    /// The fields of the rule.
    fn given(&self) -> Given<'_> {
        Given {
            action: &self.action,
            protocol: &self.protocol,
            ports: self.ports.as_deref(),
            from: self.from.as_deref(),
            on: self.on.as_deref(),
        }
    }
}

impl Args for Wanted {
    const FIELDS: &'static [Field] = &[
        action_field("action"),
        protocol_field("protocol"),
        ports_field("ports"),
        from_field("from"),
        on_field("on"),
        TRY,
    ];

    fn check(&self) -> Result<(), Bad> {
        self.given().check(&PLAIN)?;
        check_try(self.seconds)?;
        Ok(())
    }
}

/// Two rules, the one that goes and the one that comes in its place, and
/// how long the change is tried for.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Swap {
    /// `allow` or `deny`, of the rule that goes.
    pub old_action: String,
    /// `any`, `tcp`, `udp` or `ping`, of the rule that goes.
    pub old_protocol: String,
    /// The ports of the rule that goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_ports: Option<String>,
    /// Where the connections of the rule that goes come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_from: Option<String>,
    /// The card they come by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_on: Option<String>,
    /// `allow` or `deny`, of the rule that comes in its place.
    pub new_action: String,
    /// `any`, `tcp`, `udp` or `ping`, of the rule that comes.
    pub new_protocol: String,
    /// The ports of the rule that comes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_ports: Option<String>,
    /// Where the connections of the rule that comes come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_from: Option<String>,
    /// The card they come by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_on: Option<String>,
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Swap {
    /// The fields of the rule that goes.
    fn goes(&self) -> Given<'_> {
        Given {
            action: &self.old_action,
            protocol: &self.old_protocol,
            ports: self.old_ports.as_deref(),
            from: self.old_from.as_deref(),
            on: self.old_on.as_deref(),
        }
    }

    /// The fields of the rule that comes.
    fn comes(&self) -> Given<'_> {
        Given {
            action: &self.new_action,
            protocol: &self.new_protocol,
            ports: self.new_ports.as_deref(),
            from: self.new_from.as_deref(),
            on: self.new_on.as_deref(),
        }
    }

    /// The change that puts back what this one did: the rule that comes
    /// goes, and the one that went comes in its place.
    fn reversed(&self) -> Swap {
        Swap {
            old_action: self.new_action.clone(),
            old_protocol: self.new_protocol.clone(),
            old_ports: self.new_ports.clone(),
            old_from: self.new_from.clone(),
            old_on: self.new_on.clone(),
            new_action: self.old_action.clone(),
            new_protocol: self.old_protocol.clone(),
            new_ports: self.old_ports.clone(),
            new_from: self.old_from.clone(),
            new_on: self.old_on.clone(),
            seconds: None,
        }
    }
}

impl Args for Swap {
    const FIELDS: &'static [Field] = &[
        action_field("old_action"),
        protocol_field("old_protocol"),
        ports_field("old_ports"),
        from_field("old_from"),
        on_field("old_on"),
        action_field("new_action"),
        protocol_field("new_protocol"),
        ports_field("new_ports"),
        from_field("new_from"),
        on_field("new_on"),
        TRY,
    ];

    fn check(&self) -> Result<(), Bad> {
        self.goes().check(&OLD)?;
        self.comes().check(&NEW)?;
        check_try(self.seconds)?;
        Ok(())
    }
}

/// The example rule: a port that nothing listens on, which a test can
/// allow and take away again.
fn example_rule() -> Wanted {
    Wanted {
        action: "allow".into(),
        protocol: "tcp".into(),
        ports: Some("7".into()),
        from: None,
        on: None,
        seconds: None,
    }
}

/// The words of `firewall` and of the rule, and the window, for a change
/// of the rules: `firewall allow tcp 7 --try 30`.
fn change_argv(first: &[&str], rule: &Given<'_>, seconds: Option<i64>) -> Vec<String> {
    let mut argv = vec!["firewall".to_string()];
    argv.extend(first.iter().copied().map(String::from));
    argv.extend(rule.argv_words());
    argv.extend(try_words(seconds));
    argv
}

/// `firewall allow|deny RULE`: adds a rule at the end of the file.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "firewall.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/firewall/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-177"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Add a firewall rule";
    type Args = Wanted;

    fn argv(args: &Wanted) -> Vec<String> {
        change_argv(&[], &args.given(), args.seconds)
    }

    fn subject(args: &Wanted) -> Option<String> {
        Some(args.given().argv_words().join(" "))
    }

    fn files(_: &Wanted) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    fn probe(_: &Wanted) -> Option<Vec<String>> {
        Some(probe_argv())
    }

    fn probed(_: &Wanted, output: &str) -> Option<String> {
        rule_texts(output)
    }

    fn trial(args: &Wanted) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    fn examples() -> Examples<Wanted> {
        Examples {
            good: vec![example_rule()],
            bad: vec![
                json!({ "action": "allow", "protocol": "tcp", "ports": "70000" }),
                json!({ "action": "allow", "protocol": "ping", "ports": "7" }),
                json!({ "action": "allow", "protocol": "tcp", "ports": "7", "try": 5 }),
                json!({ "action": "allow", "protocol": "tcp", "ports": "9-7" }),
                json!({ "action": "allow", "protocol": "tcp", "from": "300.1.1.1" }),
                json!({ "action": "allow", "protocol": "tcp", "on": "a/b" }),
                json!({ "action": "permit", "protocol": "tcp" }),
                json!({ "action": "allow", "protocol": "icmp" }),
                json!({ "action": "allow" }),
            ],
            undo: Some(Undo { action: "firewall.remove", args: json!(example_rule()) }),
        }
    }
}

/// `firewall replace RULE with RULE`: puts a rule in the place of another.
pub struct Replace;

impl Action for Replace {
    const ID: &'static str = "firewall.replace";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/firewall/replace";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-177", "NET-180"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Replace a firewall rule";
    type Args = Swap;

    fn argv(args: &Swap) -> Vec<String> {
        let mut argv = vec!["firewall".to_string(), "replace".into()];
        argv.extend(args.goes().argv_words());
        argv.push("with".into());
        argv.extend(args.comes().argv_words());
        argv.extend(try_words(args.seconds));
        argv
    }

    fn subject(args: &Swap) -> Option<String> {
        Some(format!(
            "{} with {}",
            args.goes().argv_words().join(" "),
            args.comes().argv_words().join(" ")
        ))
    }

    fn files(_: &Swap) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    fn probe(_: &Swap) -> Option<Vec<String>> {
        Some(probe_argv())
    }

    fn probed(_: &Swap, output: &str) -> Option<String> {
        rule_texts(output)
    }

    fn trial(args: &Swap) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    fn examples() -> Examples<Swap> {
        let example = example_swap();
        let back = json!(example.reversed());
        Examples {
            good: vec![example],
            bad: vec![
                json!({
                    "old_action": "allow", "old_protocol": "tcp", "old_ports": "70000",
                    "new_action": "deny", "new_protocol": "tcp", "new_ports": "7"
                }),
                json!({
                    "old_action": "allow", "old_protocol": "tcp", "old_ports": "7",
                    "new_action": "deny", "new_protocol": "ping", "new_ports": "7"
                }),
                json!({
                    "old_action": "allow", "old_protocol": "tcp", "old_ports": "7",
                    "new_action": "deny", "new_protocol": "tcp", "new_ports": "7", "try": 5
                }),
                json!({ "old_action": "allow", "old_protocol": "tcp", "old_ports": "7" }),
                json!({ "new_action": "deny", "new_protocol": "tcp", "new_ports": "7" }),
            ],
            undo: Some(Undo { action: "firewall.replace", args: back }),
        }
    }
}

/// The example swap: the example rule, denied instead.
fn example_swap() -> Swap {
    Swap {
        old_action: "allow".into(),
        old_protocol: "tcp".into(),
        old_ports: Some("7".into()),
        old_from: None,
        old_on: None,
        new_action: "deny".into(),
        new_protocol: "tcp".into(),
        new_ports: Some("7".into()),
        new_from: None,
        new_on: None,
        seconds: None,
    }
}

/// `firewall remove RULE`: takes the first rule with those words out.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "firewall.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/firewall/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-177", "NET-180"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Remove a firewall rule";
    type Args = Wanted;

    fn argv(args: &Wanted) -> Vec<String> {
        change_argv(&["remove"], &args.given(), args.seconds)
    }

    fn subject(args: &Wanted) -> Option<String> {
        Some(args.given().argv_words().join(" "))
    }

    fn files(_: &Wanted) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    fn probe(_: &Wanted) -> Option<Vec<String>> {
        Some(probe_argv())
    }

    fn probed(_: &Wanted, output: &str) -> Option<String> {
        rule_texts(output)
    }

    fn trial(args: &Wanted) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    fn examples() -> Examples<Wanted> {
        // The rule `firewall.add` made: removing it puts the rules back as
        // they were before, which is what the add's example undoes with.
        Examples {
            good: vec![example_rule()],
            bad: vec![
                json!({ "action": "allow", "protocol": "tcp", "ports": "70000" }),
                json!({ "action": "allow", "protocol": "ping", "ports": "7" }),
                json!({ "action": "allow", "protocol": "tcp", "ports": "7", "try": 5 }),
                json!({ "action": "permit", "protocol": "tcp" }),
                json!({ "number": 1 }),
            ],
            undo: Some(Undo { action: "firewall.add", args: json!(example_rule()) }),
        }
    }
}

/// `firewall keep`: keeps the change that is being tried.
pub struct Keep;

impl Action for Keep {
    const ID: &'static str = "firewall.keep";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/firewall/trial/keep";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Keep the firewall's rules";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["firewall", "keep"].map(String::from).to_vec()
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        // What puts it back is the Put back of the change that was tried
        // (`admin put-back`), which is no action.
        Examples { good: vec![Nothing {}], bad: vec![json!({ "id": "x" })], undo: None }
    }
}

/// `firewall undo`: undoes the change that is being tried.
pub struct Revert;

impl Action for Revert {
    const ID: &'static str = "firewall.undo";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/firewall/trial/undo";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Undo the firewall's rules";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["firewall", "undo"].map(String::from).to_vec()
    }

    fn files(_: &Nothing) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(probe_argv())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        rule_texts(output)
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "now": true })], undo: None }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, Found, Params, Registered, authorize, find, query_arguments};
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// What `id` refuses `args` with: ADM-125, the field and the words.
    fn refused(id: &str, args: Value) -> (Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        assert_eq!(failure.kind, Kind::BadValue, "{}", failure.detail);
        (failure.field, failure.detail)
    }

    #[test]
    fn the_reads_run_firewall_with_json_and_the_two_that_need_root_run_as_an_administrator() {
        assert_eq!(argv("firewall.list", json!({})), ["firewall", "list", "--json"]);
        assert_eq!(argv("firewall.status", json!({})), ["firewall", "status", "--json"]);
        for (id, path, runs) in [
            ("firewall.list", "/api/1/firewall", Runs::AsThePerson),
            ("firewall.test", "/api/1/firewall/test", Runs::AsThePerson),
            ("firewall.status", "/api/1/firewall/status", Runs::AsAdministrator),
            ("firewall.dropped", "/api/1/firewall/dropped", Runs::AsAdministrator),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (false, runs, "firewall", path),
                "{id}"
            );
            assert!(action.errors().is_empty(), "{id}");
            assert!(!action.compares(), "{id}");
        }
        for id in ["firewall.list", "firewall.status"] {
            assert_eq!(action(id).prepare(json!({})).unwrap().probe, None, "{id}");
            for bad in [json!({ "all": true }), json!({ "number": 1 })] {
                assert_eq!(action(id).prepare(bad).unwrap_err().kind, Kind::BadRequest, "{id}");
            }
        }
    }

    #[test]
    fn what_was_dropped_is_looked_for_since_an_hour_a_day_or_a_week_back() {
        assert_eq!(
            argv("firewall.dropped", json!({})),
            ["firewall", "dropped", "--since", "-24h", "--json"]
        );
        for since in ["1h", "24h", "7d"] {
            assert_eq!(
                argv("firewall.dropped", json!({ "since": since })),
                ["firewall", "dropped", "--since", &format!("-{since}"), "--json"]
            );
        }
        for since in ["2h", "", "-1h", "1H", "24h ", "7", "1y", "all"] {
            assert_eq!(
                refused("firewall.dropped", json!({ "since": since })),
                (Some("since"), "The time to look back is 1h, 24h or 7d. Nothing was done.".into()),
                "{since:?}"
            );
        }
        assert_eq!(
            action("firewall.dropped").prepare(json!({ "since": 24 })).unwrap_err().kind,
            Kind::BadRequest
        );
    }

    #[test]
    fn a_connection_is_tested_with_its_protocol_port_address_and_card() {
        assert_eq!(
            argv("firewall.test", json!({ "protocol": "tcp", "port": 22, "from": "192.168.1.20" })),
            ["firewall", "test", "tcp", "22", "from", "192.168.1.20", "--json"]
        );
        assert_eq!(
            argv(
                "firewall.test",
                json!({ "protocol": "udp", "port": 65535, "from": "2001:db8::20", "on": "net0" })
            ),
            ["firewall", "test", "udp", "65535", "from", "2001:db8::20", "on", "net0", "--json"]
        );
    }

    #[test]
    fn a_connection_that_is_none_is_refused_by_its_field() {
        let ask = |more: Value| {
            let mut args = json!({ "protocol": "tcp", "port": 22, "from": "192.168.1.20" });
            for (name, value) in more.as_object().unwrap() {
                args[name] = value.clone();
            }
            refused("firewall.test", args)
        };
        let end = "Nothing was done.";
        assert_eq!(
            ask(json!({ "port": 0 })),
            (Some("port"), format!("A port is 1 to 65535. {end}"))
        );
        assert_eq!(ask(json!({ "port": 65536 })).0, Some("port"));
        assert_eq!(ask(json!({ "port": -1 })).0, Some("port"));
        assert_eq!(
            ask(json!({ "protocol": "icmp" })),
            (Some("protocol"), format!("A connection is tcp or udp. {end}"))
        );
        assert_eq!(ask(json!({ "protocol": "ping" })).0, Some("protocol"));
        assert_eq!(
            ask(json!({ "from": "192.168.1.0/24" })),
            (
                Some("from"),
                format!("`192.168.1.0/24` is not an address: a network is not one here. {end}")
            )
        );
        for from in ["", "host", "300.1.1.1", "192.168.1.20 ", "::g"] {
            assert_eq!(ask(json!({ "from": from })).0, Some("from"), "{from:?}");
        }
        for on in ["a/b", "", "-x", "a b", "net0\n", "a23456789012345 6"] {
            assert_eq!(ask(json!({ "on": on })).0, Some("on"), "{on:?}");
        }
        let failure = action("firewall.test").prepare(json!({ "port": "22" })).unwrap_err();
        assert_eq!(failure.kind, Kind::BadRequest);
    }

    #[test]
    fn the_test_takes_its_fields_from_the_query_as_a_read_does() {
        let fields = action("firewall.test").fields();
        let query = "protocol=tcp&port=22&from=192.168.1.20&on=net0";
        let args = query_arguments(Params::new(), query, fields).unwrap();
        assert_eq!(
            args,
            json!({ "protocol": "tcp", "port": 22, "from": "192.168.1.20", "on": "net0" })
        );
        assert!(action("firewall.test").prepare(args).is_ok());
        let fields = action("firewall.dropped").fields();
        let args = query_arguments(Params::new(), "since=7d", fields).unwrap();
        assert_eq!(argv("firewall.dropped", args)[3], "-7d");
        let wrong = query_arguments(Params::new(), "all=true", fields).unwrap_err();
        assert_eq!(wrong.kind, Kind::BadRequest);
    }

    /// The arguments of the rule `allow tcp 7`, with `more` added.
    fn rule(more: &Value) -> Value {
        let mut args = json!({ "action": "allow", "protocol": "tcp", "ports": "7" });
        for (name, value) in more.as_object().unwrap() {
            args[name] = value.clone();
        }
        args
    }

    #[test]
    fn a_rule_is_added_with_its_words_in_the_order_firewall_writes_them() {
        assert_eq!(argv("firewall.add", rule(&json!({}))), ["firewall", "allow", "tcp", "7"]);
        assert_eq!(
            argv(
                "firewall.add",
                json!({
                    "action": "deny", "protocol": "tcp", "ports": "8000-8100",
                    "from": "192.168.1.0/24", "on": "net0", "try": 30,
                })
            ),
            [
                "firewall",
                "deny",
                "tcp",
                "8000-8100",
                "from",
                "192.168.1.0/24",
                "on",
                "net0",
                "--try",
                "30"
            ]
        );
        // The fields may come in any order; the words do not.
        assert_eq!(
            argv(
                "firewall.add",
                json!({ "on": "net0", "from": "10.0.0.1", "ports": "443", "protocol": "udp", "action": "allow" })
            ),
            ["firewall", "allow", "udp", "443", "from", "10.0.0.1", "on", "net0"]
        );
        // `any` is not written, as the command does not print it, and a
        // rule needs no port.
        assert_eq!(
            argv("firewall.add", json!({ "action": "deny", "protocol": "any" })),
            ["firewall", "deny"]
        );
        assert_eq!(
            argv("firewall.add", json!({ "action": "allow", "protocol": "any", "ports": "53" })),
            ["firewall", "allow", "53"]
        );
        assert_eq!(
            argv(
                "firewall.add",
                json!({ "action": "allow", "protocol": "ping", "from": "10.0.0.0/8" })
            ),
            ["firewall", "allow", "ping", "from", "10.0.0.0/8"]
        );
    }

    #[test]
    fn the_words_are_the_rule_as_the_command_prints_it() {
        // An address alone is its version's whole length, and an IPv6
        // address is written as the command writes it.
        for (from, written) in [
            ("192.168.1.5/32", "192.168.1.5"),
            ("192.168.1.5", "192.168.1.5"),
            ("192.168.1.5/24", "192.168.1.5/24"),
            ("2001:DB8:0:0::1", "2001:db8::1"),
            ("2001:db8::/32", "2001:db8::/32"),
            ("::1/128", "::1"),
        ] {
            let words = argv("firewall.add", rule(&json!({ "from": from })));
            assert_eq!(words[4..], ["from", written], "{from}");
        }
        assert_eq!(argv("firewall.add", rule(&json!({ "ports": "007" })))[3], "7");
        assert_eq!(argv("firewall.add", rule(&json!({ "ports": "9-9" })))[3], "9");
    }

    #[test]
    fn each_argv_is_a_rule_the_commands_parser_reads_back() {
        for args in [
            rule(&json!({})),
            rule(&json!({ "action": "deny", "from": "192.168.1.0/24", "on": "net0" })),
            json!({ "action": "allow", "protocol": "ping" }),
            json!({ "action": "allow", "protocol": "any", "ports": "1-65535" }),
            json!({ "action": "deny", "protocol": "udp", "from": "2001:db8::/32", "on": "eth0.5" }),
        ] {
            let words = argv("firewall.add", args.clone());
            let rule =
                Rule::parse(&words[1..].iter().map(String::as_str).collect::<Vec<_>>()).unwrap();
            assert_eq!(rule.action == crate::firewall::Action::Allow, args["action"] == "allow");
            assert_eq!(rule.ports.is_some(), args.get("ports").is_some());
            assert_eq!(rule.from.is_some(), args.get("from").is_some());
            assert_eq!(rule.on.as_deref(), args["on"].as_str());
            assert_eq!(rule.program, None);
            assert_eq!(rule.to_string(), words[1..].join(" "));
        }
    }

    #[test]
    fn a_rule_that_cannot_work_is_adm_125_on_the_field_it_is_about() {
        let end = "Nothing was done.";
        let ports =
            |text: &str| format!("`{text}` is not a port, 1 to 65535, or a range of them. {end}");
        for (more, field, why) in [
            (json!({ "ports": "70000" }), "ports", ports("70000")),
            (json!({ "ports": "0" }), "ports", ports("0")),
            (json!({ "ports": "9-7" }), "ports", ports("9-7")),
            (json!({ "ports": "7-" }), "ports", ports("7-")),
            (json!({ "ports": "7-9-11" }), "ports", ports("7-9-11")),
            (json!({ "ports": "-7" }), "ports", ports("-7")),
            (json!({ "ports": "tcp" }), "ports", ports("tcp")),
            (json!({ "ports": "7 from 1.2.3.4" }), "ports", ports("7 from 1.2.3.4")),
            (json!({ "ports": "7\n" }), "ports", ports("7\n")),
            (json!({ "protocol": "ping" }), "ports", format!("A ping has no port. {end}")),
            (
                json!({ "from": "300.1.1.1" }),
                "from",
                format!("`300.1.1.1` is not an address or network. {end}"),
            ),
            (
                json!({ "from": "192.168.1.0/33" }),
                "from",
                format!("`192.168.1.0/33` is not an address or network. {end}"),
            ),
            (
                json!({ "from": "1.2.3.4 on eth0" }),
                "from",
                format!("`1.2.3.4 on eth0` is not an address or network. {end}"),
            ),
            (json!({ "on": "a/b" }), "on", format!("`a/b` is not an interface's name. {end}")),
            (json!({ "on": "-x" }), "on", format!("`-x` is not an interface's name. {end}")),
            (
                json!({ "on": "a234567890123456" }),
                "on",
                format!("`a234567890123456` is not an interface's name. {end}"),
            ),
            (json!({ "action": "permit" }), "action", format!("A rule allows or denies. {end}")),
            (
                json!({ "protocol": "icmp" }),
                "protocol",
                format!("A rule matches any, tcp, udp or ping. {end}"),
            ),
        ] {
            let args = rule(&more);
            for id in ["firewall.add", "firewall.remove"] {
                assert_eq!(refused(id, args.clone()), (Some(field), why.clone()), "{id} {more}");
            }
        }
    }

    #[test]
    fn a_window_is_checked_and_ends_the_words() {
        let words = "A change is tried for 10 to 3600 seconds. Nothing was done.";
        for seconds in [5, 9, 3601, 0, -1] {
            for id in ["firewall.add", "firewall.remove"] {
                assert_eq!(
                    refused(id, rule(&json!({ "try": seconds }))),
                    (Some("try"), words.into()),
                    "{id} {seconds}"
                );
            }
        }
        assert_eq!(
            argv("firewall.add", rule(&json!({ "try": 10 }))),
            ["firewall", "allow", "tcp", "7", "--try", "10"]
        );
        assert_eq!(
            argv("firewall.remove", rule(&json!({ "try": 3600 }))),
            ["firewall", "remove", "allow", "tcp", "7", "--try", "3600"]
        );
        let prepared = action("firewall.add").prepare(rule(&json!({ "try": 30 }))).unwrap();
        assert_eq!(prepared.args["try"], 30);
        assert!(prepared.args.get("seconds").is_none());
    }

    /// The arguments of the swap of `allow tcp 7` for `deny tcp 7`, with
    /// `more` added.
    fn swap(more: &Value) -> Value {
        let mut args = json!({
            "old_action": "allow", "old_protocol": "tcp", "old_ports": "7",
            "new_action": "deny", "new_protocol": "tcp", "new_ports": "7",
        });
        for (name, value) in more.as_object().unwrap() {
            args[name] = value.clone();
        }
        args
    }

    #[test]
    fn a_rule_is_replaced_by_another_with_the_word_with_between_and_the_window_last() {
        assert_eq!(
            argv("firewall.replace", swap(&json!({}))),
            ["firewall", "replace", "allow", "tcp", "7", "with", "deny", "tcp", "7"]
        );
        assert_eq!(
            argv(
                "firewall.replace",
                swap(&json!({
                    "old_from": "192.168.1.0/24", "old_on": "net0", "new_protocol": "any",
                    "new_ports": "8000-8100", "new_from": "10.0.0.1", "try": 30,
                }))
            ),
            [
                "firewall",
                "replace",
                "allow",
                "tcp",
                "7",
                "from",
                "192.168.1.0/24",
                "on",
                "net0",
                "with",
                "deny",
                "8000-8100",
                "from",
                "10.0.0.1",
                "--try",
                "30"
            ]
        );
    }

    #[test]
    fn a_replacement_that_cannot_work_says_which_of_the_two_rules_and_which_field() {
        let end = "Nothing was done.";
        let ports =
            |text: &str| format!("`{text}` is not a port, 1 to 65535, or a range of them. {end}");
        for (more, field, why) in [
            (json!({ "old_ports": "70000" }), "old_ports", ports("70000")),
            (json!({ "new_ports": "70000" }), "new_ports", ports("70000")),
            (json!({ "new_protocol": "ping" }), "new_ports", format!("A ping has no port. {end}")),
            (json!({ "old_protocol": "ping" }), "old_ports", format!("A ping has no port. {end}")),
            (
                json!({ "old_from": "x" }),
                "old_from",
                format!("`x` is not an address or network. {end}"),
            ),
            (
                json!({ "new_from": "x" }),
                "new_from",
                format!("`x` is not an address or network. {end}"),
            ),
            (
                json!({ "old_on": "a/b" }),
                "old_on",
                format!("`a/b` is not an interface's name. {end}"),
            ),
            (
                json!({ "new_on": "a/b" }),
                "new_on",
                format!("`a/b` is not an interface's name. {end}"),
            ),
            (json!({ "old_action": "x" }), "old_action", format!("A rule allows or denies. {end}")),
            (
                json!({ "new_protocol": "x" }),
                "new_protocol",
                format!("A rule matches any, tcp, udp or ping. {end}"),
            ),
            (
                json!({ "try": 5 }),
                "try",
                format!("A change is tried for 10 to 3600 seconds. {end}"),
            ),
        ] {
            assert_eq!(refused("firewall.replace", swap(&more)), (Some(field), why), "{more}");
        }
        // The rule that goes is looked at first.
        let both = swap(&json!({ "old_ports": "70000", "new_ports": "70001" }));
        assert_eq!(refused("firewall.replace", both).0, Some("old_ports"));
    }

    #[test]
    fn a_rule_is_removed_by_its_words_after_the_word_remove() {
        assert_eq!(
            argv("firewall.remove", rule(&json!({}))),
            ["firewall", "remove", "allow", "tcp", "7"]
        );
        assert_eq!(
            argv(
                "firewall.remove",
                rule(&json!({ "from": "192.168.1.0/24", "on": "net0", "try": 30 }))
            ),
            [
                "firewall",
                "remove",
                "allow",
                "tcp",
                "7",
                "from",
                "192.168.1.0/24",
                "on",
                "net0",
                "--try",
                "30"
            ]
        );
    }

    #[test]
    fn the_changes_name_the_file_and_compare_the_texts_of_the_rules_before_and_after() {
        let list = ["firewall", "list", "--json"].map(String::from).to_vec();
        for (id, args) in [
            ("firewall.add", rule(&json!({}))),
            ("firewall.remove", rule(&json!({}))),
            ("firewall.replace", swap(&json!({}))),
            ("firewall.undo", json!({})),
        ] {
            let prepared = action(id).prepare(args).unwrap();
            assert_eq!(prepared.files, ["/system/settings/firewall"], "{id}");
            assert_eq!(prepared.probe, Some(list.clone()), "{id}");
            assert!(action(id).compares(), "{id}");
        }
        let keep = action("firewall.keep").prepare(json!({})).unwrap();
        assert_eq!((keep.files, keep.probe), (Vec::new(), None));
        assert!(!action("firewall.keep").compares());
        let output = json!({
            "rules": [
                { "number": 1, "text": "allow tcp 22", "action": "allow", "protocol": "tcp",
                  "ports": [22, 22], "from": null, "on": null, "program": null },
                { "number": 2, "text": "deny tcp 7 from 10.0.0.0/8", "action": "deny",
                  "protocol": "tcp", "ports": [7, 7], "from": "10.0.0.0/8", "on": null,
                  "program": null },
            ],
            "problems": [{ "line": 3, "text": "allow tcp 99999", "why": "x" }],
            "trial": { "id": "firewall", "left_ms": 41000 },
        })
        .to_string();
        let picks = |id: &str, output: &str| action(id).probed(&json!({}), output);
        for id in ["firewall.add", "firewall.remove", "firewall.undo"] {
            let args = if id == "firewall.undo" { json!({}) } else { rule(&json!({})) };
            assert_eq!(
                action(id).probed(&args, &output).as_deref(),
                Some("allow tcp 22\ndeny tcp 7 from 10.0.0.0/8"),
                "{id}"
            );
        }
        // The problems and the trial are not the rules: a trial that ends
        // moves neither.
        let later = output.replace("41000", "12000").replace("allow tcp 99999", "allow tcp 99998");
        assert_eq!(picks("firewall.undo", &output), picks("firewall.undo", &later));
        let fewer = output.replace("deny tcp 7 from 10.0.0.0/8", "deny tcp 8");
        assert_ne!(picks("firewall.undo", &output), picks("firewall.undo", &fewer));
        assert_eq!(picks("firewall.undo", r#"{"rules":[]}"#).as_deref(), Some(""));
        for output in ["", "not json", "[]", "{}", r#"{"rules":{}}"#, r#"{"rules":[{"number":1}]}"#]
        {
            assert_eq!(picks("firewall.undo", output), None, "{output:?}");
        }
    }

    #[test]
    fn keep_and_undo_run_firewall_as_an_administrator_with_no_arguments() {
        assert_eq!(argv("firewall.keep", json!({})), ["firewall", "keep"]);
        assert_eq!(argv("firewall.undo", json!({})), ["firewall", "undo"]);
        for (id, path) in [
            ("firewall.keep", "/api/1/firewall/trial/keep"),
            ("firewall.undo", "/api/1/firewall/trial/undo"),
            ("firewall.add", "/api/1/firewall/add"),
            ("firewall.replace", "/api/1/firewall/replace"),
            ("firewall.remove", "/api/1/firewall/remove"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (true, Runs::AsAdministrator, "firewall", path),
                "{id}"
            );
        }
        for (id, errors) in [
            ("firewall.add", &["NET-177"][..]),
            ("firewall.replace", &["NET-177", "NET-180"]),
            ("firewall.remove", &["NET-177", "NET-180"]),
            ("firewall.keep", &["NET-179"]),
            ("firewall.undo", &["NET-179"]),
        ] {
            assert_eq!(action(id).errors(), errors, "{id}");
        }
        for (id, bad) in
            [("firewall.keep", json!({ "id": "x" })), ("firewall.undo", json!({ "now": true }))]
        {
            assert_eq!(action(id).prepare(bad).unwrap_err().kind, Kind::BadRequest, "{id}");
        }
    }

    #[test]
    fn each_example_works_each_bad_one_does_not_and_what_puts_back_is_the_other_way() {
        for id in [
            "firewall.list",
            "firewall.test",
            "firewall.status",
            "firewall.dropped",
            "firewall.add",
            "firewall.replace",
            "firewall.remove",
            "firewall.keep",
            "firewall.undo",
        ] {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty(), "{id}");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id} {args}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
            assert_eq!(
                undo.is_some(),
                id.ends_with("add") || id.ends_with("replace") || id.ends_with("remove"),
                "{id}"
            );
        }
        // add, then remove, then add: each puts back what the other did.
        let (add, _, undo) = action("firewall.add").example_values();
        let undo = undo.unwrap();
        assert_eq!((undo.action, &undo.args), ("firewall.remove", &add[0]));
        assert_eq!(add[0], rule(&json!({})));
        let (remove, _, undo) = action("firewall.remove").example_values();
        let undo = undo.unwrap();
        assert_eq!((undo.action, &undo.args), ("firewall.add", &remove[0]));
        // replace goes back by the replace the other way.
        let (replace, _, undo) = action("firewall.replace").example_values();
        let undo = undo.unwrap();
        assert_eq!(undo.action, "firewall.replace");
        assert_eq!(replace[0], swap(&json!({})));
        let back = swap(&json!({
            "old_action": "deny", "new_action": "allow",
        }));
        assert_eq!(undo.args, back);
        assert!(action("firewall.replace").prepare(undo.args.clone()).is_ok());
        let again: Swap = serde_json::from_value(undo.args.clone()).unwrap();
        assert_eq!(json!(again.reversed()), replace[0]);
    }

    #[test]
    fn the_bad_examples_of_the_design_are_each_refused_by_the_field_they_are_about() {
        let fields = |id: &str| -> Vec<Option<&'static str>> {
            let (_, bad, _) = action(id).example_values();
            bad.iter().map(|args| action(id).prepare(args.clone()).unwrap_err().field).collect()
        };
        assert_eq!(
            fields("firewall.add"),
            [
                Some("ports"),
                Some("ports"),
                Some("try"),
                Some("ports"),
                Some("from"),
                Some("on"),
                Some("action"),
                Some("protocol"),
                None
            ]
        );
    }

    #[test]
    fn the_routes_are_told_apart_and_the_reads_are_get_and_the_changes_post() {
        for (post, path, id) in [
            (false, "/api/1/firewall", "firewall.list"),
            (false, "/api/1/firewall/test", "firewall.test"),
            (false, "/api/1/firewall/status", "firewall.status"),
            (false, "/api/1/firewall/dropped", "firewall.dropped"),
            (true, "/api/1/firewall/add", "firewall.add"),
            (true, "/api/1/firewall/replace", "firewall.replace"),
            (true, "/api/1/firewall/remove", "firewall.remove"),
            (true, "/api/1/firewall/trial/keep", "firewall.keep"),
            (true, "/api/1/firewall/trial/undo", "firewall.undo"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(matches!(&found, Found::Action(action, _) if action.id() == id), "{path}");
            assert!(matches!(find(REGISTRY, !post, path), Found::WrongMethod), "{path}");
        }
    }

    #[test]
    fn the_status_and_what_was_dropped_need_an_unlocked_administrator_and_a_full_scope() {
        let session = |admin, unlocked| Caller::Session { name: "dave", admin, unlocked };
        for id in ["firewall.status", "firewall.dropped"] {
            let action = action(id);
            assert_eq!(authorize(&session(false, true), action).unwrap_err().kind, Kind::NotAdmin);
            assert_eq!(authorize(&session(true, false), action).unwrap_err().kind, Kind::Locked);
            assert!(authorize(&session(true, true), action).is_ok(), "{id}");
            let token = |list: &str| {
                let scopes = Scopes::parse(list).unwrap();
                authorize(&Caller::Token { name: "ci", scopes: &scopes }, action)
            };
            assert_eq!(token("firewall:read").unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(token("firewall").is_ok(), "{id}");
        }
        // The list and the test run as the person: any session, and the
        // read scope.
        for id in ["firewall.list", "firewall.test"] {
            assert!(authorize(&session(false, false), action(id)).is_ok(), "{id}");
            let scopes = Scopes::parse("firewall:read").unwrap();
            assert!(authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id)).is_ok());
        }
    }

    #[test]
    fn the_ansible_task_is_the_command_and_the_name_has_the_rule() {
        let task = action("firewall.add").prepare(rule(&json!({ "try": 30 }))).unwrap().ansible;
        assert_eq!(task.name, "Add a firewall rule: allow tcp 7");
        assert!(task.become_root);
        assert_eq!(task.params["argv"], json!(["firewall", "allow", "tcp", "7", "--try", "30"]));
        let task = action("firewall.replace").prepare(swap(&json!({}))).unwrap().ansible;
        assert_eq!(task.name, "Replace a firewall rule: allow tcp 7 with deny tcp 7");
        let task = action("firewall.list").prepare(json!({})).unwrap().ansible;
        assert!(!task.become_root);
        assert_eq!(task.changed_when.as_deref(), Some("false"));
        let task = action("firewall.status").prepare(json!({})).unwrap().ansible;
        assert!(task.become_root);
        assert_eq!(task.changed_when.as_deref(), Some("false"));
    }
}
