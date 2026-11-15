// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The secrets an action takes or answers (ADR 0561): the rules every
//! action keeps ([`keeps_secrets`]), the test actions that take them, and
//! the tests of a secret field, its route's method and how it is shown.

use super::*;
use crate::admind::actions::secret::{INPUT_MAX, SHOWN, line_bytes};

/// What each secret is replaced with in the rules' run of an example: text
/// that no command, address or task would make by itself.
fn marker(name: &str) -> String {
    format!("mk-{name}-5f1e a secret")
}

/// The rules every action keeps of secrets: its route is a `POST` when it
/// takes or answers one; its secrets fit a pipe together, are never in its
/// address and are named as a shell's variables are, in small letters; an
/// action that answers one is never a task. And, for each of its examples
/// with each secret it gives set to a marker: its command reads the
/// secrets as lines in the order of its fields, and nothing else of it
/// holds a byte of them, not its words, its arguments as a task keeps
/// them, its Ansible task (which reads them from the playbook, with
/// `no_log`) nor anything the Show command shows, with the secrets sent
/// or without them.
pub(super) fn keeps_secrets(action: &'static dyn Registered) {
    let id = action.id();
    let secrets: Vec<&Field> = action.fields().iter().filter(|field| field.is_secret()).collect();
    assert_eq!(
        action.posts(),
        action.changes() || action.secret_answer() || !secrets.is_empty(),
        "{id}: a change's route is a POST, as is a read's that takes or answers a secret"
    );
    let method = if action.posts() { "POST" } else { "GET" };
    let listed = listed(&[action]);
    assert_eq!(listed[0]["method"], method, "{id}");
    assert_eq!(listed[0]["secret_answer"], action.secret_answer(), "{id}");
    assert!(!(action.secret_answer() && action.long()), "{id}: a task keeps what it printed");
    let room: usize = secrets
        .iter()
        .map(|field| match field.kind {
            Type::Secret { max } => line_bytes(max),
            _ => 0,
        })
        .sum();
    assert!(room <= INPUT_MAX, "{id}: its secrets take {room} bytes, more than a pipe holds");
    let in_address: Vec<&str> = action
        .path()
        .split('/')
        .filter_map(|part| part.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')))
        .collect();
    for field in &secrets {
        let name = field.name;
        assert!(!in_address.contains(&name), "{id}: the secret {name} is in its address");
        assert!(
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{id}: {name} is a shell variable's name in small letters"
        );
        assert!(matches!(field.kind, Type::Secret { max } if max >= 1), "{id}: {name}");
    }
    let (good, _, _) = action.example_values();
    for example in good {
        // The examples list a secret as `(secret)`, never as itself.
        let mut args = example.clone();
        let mut given = Vec::new();
        for field in &secrets {
            if args.get(field.name).is_some_and(|value| !value.is_null()) {
                assert_eq!(args[field.name], SHOWN, "{id}: its example lists {}", field.name);
                args[field.name] = Value::from(marker(field.name));
                given.push(marker(field.name));
            }
        }
        let prepared =
            action.prepare(args.clone()).unwrap_or_else(|f| panic!("{id}: {}", f.detail));
        let lines = given.iter().fold(String::new(), |lines, secret| lines + secret + "\n");
        assert_eq!(
            prepared.input.as_ref().map(|input| input.bytes().to_vec()),
            (!given.is_empty()).then(|| lines.into_bytes()),
            "{id}: its command reads each secret given as a line, in the order of its fields"
        );
        let shown = show::shown(&prepared, "https://nas.example:9090");
        let words = [
            prepared.argv.join(" "),
            prepared.args.to_string(),
            show::yaml(&prepared.ansible),
            prepared.files.join(" "),
            shown.to_string(),
            format!("{prepared:?}"),
        ];
        for text in &words {
            assert!(!text.contains("mk-"), "{id}: a secret is in {text}");
        }
        let task = &prepared.ansible;
        assert_eq!(task.no_log, !given.is_empty() || action.secret_answer(), "{id}: no_log");
        match given.len() {
            0 => assert!(task.params.get("stdin").is_none(), "{id}"),
            _ => {
                assert_eq!(task.module, "ansible.builtin.command", "{id}: stdin is the command's");
                let names: Vec<String> = secrets
                    .iter()
                    .filter(|field| example.get(field.name).is_some_and(|value| !value.is_null()))
                    .map(|field| format!("{{{{ {} }}}}", field.name))
                    .collect();
                assert_eq!(task.params["stdin"], names.join("\n"), "{id}: its playbook's");
            }
        }
        if action.secret_answer() {
            assert_eq!(task.register, Some("answer"), "{id}: the playbook keeps the answer");
        }
        // Shown before the secrets are typed, as a page shows a form not
        // yet filled in, each given as empty, it says the same.
        let mut without = args.clone();
        for field in &secrets {
            if without.get(field.name).is_some_and(|value| !value.is_null()) {
                without[field.name] = json!("");
            }
        }
        let bare = show::prepare(action, without).unwrap_or_else(|f| panic!("{id}: {}", f.detail));
        assert_eq!(bare.argv, prepared.argv, "{id}");
        let shown_sent = show::prepare(action, args).unwrap();
        assert!(!format!("{:?}", shown_sent.input).contains("mk-"), "{id}");
        let shown_sent = show::shown(&shown_sent, "https://nas.example:9090");
        assert!(!shown_sent.to_string().contains("mk-"), "{id}");
    }
}

/// Values to try a secret field of up to `max` characters with.
pub(super) fn candidates(max: usize) -> Vec<Value> {
    let mut values = vec![
        json!("a"),
        json!("a secret with spaces"),
        json!("caf\u{e9} \u{1f511}"),
        json!("a".repeat(max)),
        json!("\u{e9}".repeat(max)),
        json!("a".repeat(max + 1)),
        json!(""),
        json!("a\nb"),
        json!("a\rb"),
        json!("ab\n"),
        json!("a\u{0}b"),
        json!("\"quoted\" $HOME `x`"),
    ];
    values.extend([json!(7), json!(null), json!(true), json!(["a"]), json!({})]);
    values
}

/// A change for the tests alone that sets an account's password, which
/// its command reads from a pipe, as `passwd NAME` does.
struct Password;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NamedPassword {
    name: String,
    password: Secret,
}

impl Args for NamedPassword {
    const FIELDS: &'static [Field] = &[
        Field::text("name", "An account's name.", 1, 32, Some("^[a-z][a-z0-9-]*$")),
        Field::secret("password", "The new password.", 128),
    ];

    fn check(&self) -> Result<(), Bad> {
        let mut name = self.name.chars();
        let first_ok = name.next().is_some_and(|c| c.is_ascii_lowercase());
        if !first_ok
            || self.name.len() > 32
            || !name.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Bad { field: "name", why: "An account's name. Nothing was set.".into() });
        }
        Ok(())
    }
}

impl Action for Password {
    const ID: &'static str = "lab.password";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/{name}/password";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set a lab account's password";
    type Args = NamedPassword;

    fn argv(args: &NamedPassword) -> Vec<String> {
        vec!["passwd".into(), args.name.clone()]
    }

    fn subject(args: &NamedPassword) -> Option<String> {
        Some(args.name.clone())
    }

    fn input(args: &NamedPassword) -> Option<Secret> {
        Some(Secret::lines(&[&args.password]))
    }

    fn examples() -> Examples<NamedPassword> {
        Examples {
            good: vec![NamedPassword {
                name: "lab-reader".into(),
                password: Secret::from("lab-pass-1".to_string()),
            }],
            bad: vec![
                json!({ "name": "lab-reader", "password": "" }),
                json!({ "name": "lab-reader", "password": "two\nlines" }),
                json!({ "name": "lab-reader" }),
            ],
            undo: None,
        }
    }
}

/// A change for the tests alone that a person makes as themselves, with
/// their password and a new one, as `passwd` reads them from a pipe.
struct OwnPassword;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TwoPasswords {
    password: Secret,
    new_password: Secret,
}

impl Args for TwoPasswords {
    const FIELDS: &'static [Field] = &[
        Field::secret("password", "Your password now.", 128),
        Field::secret("new_password", "The new password.", 128),
    ];

    fn check(&self) -> Result<(), Bad> {
        Ok(())
    }
}

impl Action for OwnPassword {
    const ID: &'static str = "lab.own-password";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/password";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change your lab password";
    type Args = TwoPasswords;

    fn argv(_: &TwoPasswords) -> Vec<String> {
        vec!["passwd".into()]
    }

    fn input(args: &TwoPasswords) -> Option<Secret> {
        Some(Secret::lines(&[&args.password, &args.new_password]))
    }

    fn examples() -> Examples<TwoPasswords> {
        Examples {
            good: vec![TwoPasswords {
                password: Secret::from("lab-pass-1".to_string()),
                new_password: Secret::from("lab-pass-2".to_string()),
            }],
            bad: vec![json!({ "password": "x" })],
            undo: None,
        }
    }
}

/// A read for the tests alone that takes a recovery key or the file that
/// holds one, as `move.plan` reads a backup.
struct Unlock;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct KeyOrFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
}

impl Args for KeyOrFile {
    const FIELDS: &'static [Field] = &[
        Field::secret("key", "The recovery key.", 64).optional(),
        Field::text("file", "The file that holds it.", 1, 255, Some("^/")).optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        if self.key.is_some() == self.file.is_some() {
            return Err(Bad { field: "key", why: "The key or its file, one of them.".into() });
        }
        if self.file.as_ref().is_some_and(|file| !file.starts_with('/') || file.len() > 255) {
            return Err(Bad { field: "file", why: "A file's whole path.".into() });
        }
        Ok(())
    }
}

impl Action for Unlock {
    const ID: &'static str = "lab.unlock";
    const AREA: &'static str = "services";
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/lab/unlock";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Read a locked lab backup";
    type Args = KeyOrFile;

    fn argv(args: &KeyOrFile) -> Vec<String> {
        let from = args.file.clone().unwrap_or_else(|| "-".into());
        vec!["lab-unlock".into(), "--json".into(), "--key".into(), from]
    }

    fn input(args: &KeyOrFile) -> Option<Secret> {
        args.key.as_ref().map(|key| Secret::lines(&[key]))
    }

    fn examples() -> Examples<KeyOrFile> {
        Examples {
            good: vec![
                KeyOrFile { key: Some(Secret::from("ABCD-EFGH".to_string())), file: None },
                KeyOrFile { key: None, file: Some("/users/lab/key.txt".into()) },
            ],
            bad: vec![json!({}), json!({ "key": "a\nb" })],
            undo: None,
        }
    }
}

/// A read for the tests alone whose command prints a secret, as `backup
/// key NAME` prints a recovery key.
struct Key;

impl Action for Key {
    const ID: &'static str = "lab.key";
    const AREA: &'static str = "services";
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/lab/{name}/key";
    const RUNS: Runs = Runs::AsAdministrator;
    const SECRET_ANSWER: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show a lab key again";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["lab-key".into(), args.name.clone(), "--json".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-web".into(), wait: None }],
            bad: vec![],
            undo: None,
        }
    }
}

/// A long change for the tests alone that takes a key, as `move.stage`
/// does: a task, whose command reads it as a short one would.
struct Restore;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct KeyOnly {
    key: Secret,
}

impl Args for KeyOnly {
    const FIELDS: &'static [Field] = &[Field::secret("key", "The recovery key.", 64)];

    fn check(&self) -> Result<(), Bad> {
        Ok(())
    }
}

impl Action for Restore {
    const ID: &'static str = "lab.restore";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/restore";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Restore the lab's backup";
    type Args = KeyOnly;

    fn argv(_: &KeyOnly) -> Vec<String> {
        ["admin", "lab-steps", "1", "--key", "-"].map(String::from).to_vec()
    }

    fn input(args: &KeyOnly) -> Option<Secret> {
        Some(Secret::lines(&[&args.key]))
    }

    fn examples() -> Examples<KeyOnly> {
        Examples {
            good: vec![KeyOnly { key: Secret::from("ABCD-EFGH".to_string()) }],
            bad: vec![json!({ "key": "" })],
            undo: None,
        }
    }
}

static PASSWORD: Entry<Password> = Entry::new();
static OWN_PASSWORD: Entry<OwnPassword> = Entry::new();
static UNLOCK: Entry<Unlock> = Entry::new();
static KEY: Entry<Key> = Entry::new();
static RESTORE: Entry<Restore> = Entry::new();
/// These tests' actions that take or answer a secret, which the route
/// tests use too.
pub static SECRETS: &[&dyn Registered] = &[&PASSWORD, &OWN_PASSWORD, &UNLOCK, &KEY, &RESTORE];

/// `id` of [`SECRETS`].
fn secret_action(id: &str) -> &'static dyn Registered {
    SECRETS.iter().copied().find(|action| action.id() == id).unwrap()
}

#[test]
fn a_secret_field_s_schema_is_one_line_of_text_sent_and_never_given_back() {
    let fields = [Field::secret("password", "The new password.", 128)];
    assert_eq!(
        schema(&fields),
        json!({
            "type": "object",
            "properties": {
                "password": {
                    "type": "string", "description": "The new password.",
                    "minLength": 1, "maxLength": 128, "pattern": "^[^\n\r\0]*$",
                    "writeOnly": true,
                },
            },
            "required": ["password"],
            "additionalProperties": false,
        })
    );
    assert!(fields[0].is_secret() && !Field::text("a", "A.", 0, 1, None).is_secret());
    let optional = fields[0].optional();
    assert!(optional.is_secret() && !optional.required);
    // The pattern is read as written, its characters as they are.
    for (text, one_line) in [("a b", true), ("a\nb", false), ("a\rb", false), ("a\0b", false)] {
        assert_eq!(validate::matches(SECRET_PATTERN, text).unwrap(), one_line, "{text:?}");
    }
}

#[test]
fn a_secret_is_checked_before_it_is_read_and_a_refusal_never_quotes_it() {
    let password = secret_action("lab.password");
    let with = |value: Value| json!({ "name": "bob", "password": value });
    let ok = password.prepare(with(json!("right one"))).unwrap();
    assert_eq!(ok.input.unwrap().bytes(), b"right one\n");
    let too_long = format!("{}x", "s".repeat(128));
    for (value, kind) in [
        (json!(""), Kind::BadValue),
        (json!("one\nhunter2"), Kind::BadValue),
        (json!("one\rhunter2"), Kind::BadValue),
        (json!("one\0hunter2"), Kind::BadValue),
        (json!(too_long), Kind::BadValue),
        (json!(5_551_234), Kind::BadRequest),
        (json!(["hunter2"]), Kind::BadRequest),
    ] {
        let refused = password.prepare(with(value.clone())).unwrap_err();
        assert_eq!(refused.kind, kind, "{value}");
        let said = refused.body().to_string();
        for quoted in ["hunter2", "5551234", "sssss"] {
            assert!(!said.contains(quoted), "{value}: {said}");
        }
        if kind == Kind::BadValue {
            assert_eq!(refused.field, Some("password"), "{value}");
            assert_eq!(
                refused.detail,
                "It is one line of 1 to 128 characters, with no line break. Nothing was done."
            );
        }
    }
    // Left out, it is refused as any required argument is.
    let missing = password.prepare(json!({ "name": "bob" })).unwrap_err();
    assert_eq!(missing.kind, Kind::BadRequest);
}

#[test]
fn a_read_that_takes_or_answers_a_secret_is_a_post_and_never_reads_an_address_s_query() {
    for (path, id) in [("/api/1/lab/unlock", "lab.unlock"), ("/api/1/lab/web/key", "lab.key")] {
        let Found::Action(action, _) = find(SECRETS, true, path) else { panic!("{path}") };
        assert_eq!(action.id(), id);
        assert!(!action.changes() && action.posts(), "{id}");
        assert!(matches!(find(SECRETS, false, path), Found::WrongMethod), "{path}");
    }
    let listed = listed(SECRETS);
    let methods: Vec<(&str, &str, bool)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (a["id"].as_str().unwrap(), a["method"].as_str().unwrap(), a["secret_answer"] == true)
        })
        .collect();
    assert_eq!(
        methods,
        [
            ("lab.password", "POST", false),
            ("lab.own-password", "POST", false),
            ("lab.unlock", "POST", false),
            ("lab.key", "POST", true),
            ("lab.restore", "POST", false),
        ]
    );
    // A secret given in a query is refused by name, and not quoted.
    let refused =
        query_arguments(Params::new(), "key=hunter2", secret_action("lab.unlock").fields())
            .unwrap_err();
    assert_eq!(refused.kind, Kind::BadRequest);
    assert!(refused.detail.contains("\"key\" is a secret") && !refused.detail.contains("hunter2"));
}

#[test]
fn a_secret_is_shown_as_typed_when_asked_as_a_variable_and_as_the_playbook_s() {
    let origin = "https://nas.example:9090";
    // Shown before it is typed: the page sends no password.
    let prepared = show::prepare(secret_action("lab.password"), json!({ "name": "bob" })).unwrap();
    let shown = show::shown(&prepared, origin);
    assert_eq!(shown["command"], "elevate passwd bob");
    assert_eq!(shown["typed"], "then the password, typed when asked");
    assert_eq!(
        shown["api"],
        "curl -sS -X POST -H \"Authorization: Bearer $DUKOVO_TOKEN\" -H \"Content-Type: \
         application/json\" -d \"{\\\"password\\\":\\\"$PASSWORD\\\"}\" \
         https://nas.example:9090/api/1/lab/bob/password"
    );
    assert_eq!(
        shown["ansible"],
        "- name: \"Set a lab account's password: bob\"\n  ansible.builtin.command:\n    argv:\n      \
         - \"passwd\"\n      - \"bob\"\n    stdin: \"{{ password }}\"\n  become: true\n  \
         no_log: true\n"
    );
    // Two, in the order of the fields, and none for a command that reads
    // none.
    let two = show::prepare(secret_action("lab.own-password"), json!({})).unwrap();
    let shown = show::shown(&two, origin);
    assert_eq!(shown["command"], "passwd");
    assert_eq!(shown["typed"], "then the password and the new password, typed when asked");
    assert_eq!(two.ansible.params["stdin"], "{{ password }}\n{{ new_password }}");
    let file = show::prepare(secret_action("lab.unlock"), json!({ "file": "/k" })).unwrap();
    let shown = show::shown(&file, origin);
    assert_eq!(shown["typed"], Value::Null);
    assert!(!file.ansible.no_log && file.input.is_none());
    assert_eq!(
        shown["api"],
        "curl -sS -X POST -H \"Authorization: Bearer $DUKOVO_TOKEN\" -H \"Content-Type: \
         application/json\" -d '{\"file\":\"/k\"}' https://nas.example:9090/api/1/lab/unlock"
    );
    // A secret answer: the playbook keeps it, and shows nothing of it.
    let key = show::prepare(secret_action("lab.key"), json!({ "name": "web" })).unwrap();
    let yaml = show::yaml(&key.ansible);
    assert!(yaml.contains("  no_log: true\n  register: answer\n"), "{yaml}");
}

#[test]
fn the_curl_body_of_a_secret_is_the_json_the_shell_makes_of_its_variables() {
    let prepared = show::prepare(secret_action("lab.own-password"), json!({})).unwrap();
    let api =
        show::shown(&prepared, "https://nas.example:9090")["api"].as_str().unwrap().to_owned();
    let body = api.split(" -d ").nth(1).unwrap().rsplit_once(' ').unwrap().0.to_owned();
    // sh reads the word as curl would be given it.
    let script = format!("printf %s {body}");
    let made = std::process::Command::new("sh")
        .args(["-c", &script])
        .env("PASSWORD", "now it's $x")
        .env("NEW_PASSWORD", "`new` one")
        .output()
        .unwrap();
    let made: Value = serde_json::from_slice(&made.stdout).unwrap();
    assert_eq!(made, json!({ "password": "now it's $x", "new_password": "`new` one" }));
    // With other arguments beside it, and characters sh would read.
    let mut plain = Map::new();
    plain.insert("name".into(), json!("a\"b$c`d\\e"));
    let word = show_body(&plain, &["key"]);
    let made = std::process::Command::new("sh")
        .args(["-c", &format!("printf %s {word}")])
        .env("KEY", "K-1")
        .output()
        .unwrap();
    let made: Value = serde_json::from_slice(&made.stdout).unwrap();
    assert_eq!(made, json!({ "name": "a\"b$c`d\\e", "key": "K-1" }));
}

/// The body word of a `curl` line for `plain` arguments and `secrets`.
fn show_body(plain: &Map<String, Value>, secrets: &[&str]) -> String {
    show::body_with_variables(plain, secrets)
}
