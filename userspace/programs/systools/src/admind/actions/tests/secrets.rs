// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The secrets an action takes or answers (ADR 0561): the rules every
//! action keeps ([`keeps_secrets`]), the test actions that take them, and
//! the tests of a secret field, its route's method and how it is shown.

use super::*;
use crate::admind::actions::secret::{INPUT_LIMIT, INPUT_MAX, SHOWN, line_bytes};
use crate::admind::actions::{DOCUMENT_MAX, DOCUMENT_PATTERN};

/// The actions whose command may be given more than a pipe holds (ADR
/// 0567): each takes a document, which is written while it runs. The last
/// is these tests' own.
const LARGE_INPUT: &[&str] = &["certificate.add", "lab.certificate"];

/// What each secret is replaced with in the rules' run of an example: text
/// that no command, address or task would make by itself.
fn marker(name: &str) -> String {
    format!("mk-{name}-5f1e a secret")
}

/// The rules every action keeps of secrets and documents: its route is a
/// `POST` when it takes either or answers a secret; what its command reads
/// fits its [`Registered::input_max`], which is a pipe's unless the action
/// is one of [`LARGE_INPUT`]; neither is in its address, and a secret is
/// named as a shell's variables are, in small letters; an action that
/// answers a secret is never a task. And, for each of its examples with
/// each secret it gives set to a marker: its command reads the secrets as
/// lines and the documents as they are, in the order of its fields, and
/// nothing else of it holds a byte of a secret, not its words, its
/// arguments as a task keeps them, its Ansible task (which reads them from
/// the playbook, with `no_log`) nor anything the Show command shows, with
/// the secrets sent or without them.
pub(super) fn keeps_secrets(action: &'static dyn Registered) {
    let id = action.id();
    let secrets: Vec<&Field> = action.fields().iter().filter(|field| field.is_secret()).collect();
    let inputs: Vec<&Field> = action.fields().iter().filter(|field| field.is_input()).collect();
    assert_eq!(
        action.posts(),
        action.changes() || action.secret_answer() || !inputs.is_empty(),
        "{id}: a change's route is a POST, as is a read's that takes a secret or a document or \
         answers a secret"
    );
    let method = if action.posts() { "POST" } else { "GET" };
    let listed = listed(&[action]);
    assert_eq!(listed[0]["method"], method, "{id}");
    assert_eq!(listed[0]["secret_answer"], action.secret_answer(), "{id}");
    assert!(!(action.secret_answer() && action.long()), "{id}: a task keeps what it printed");
    let room: usize = inputs.iter().map(|field| field.input_bytes()).sum();
    let most = action.input_max();
    assert!(room <= most, "{id}: its input takes {room} bytes, more than its {most}");
    assert!(most <= INPUT_LIMIT, "{id}: {most} bytes of input is more than any action takes");
    assert_eq!(
        most > INPUT_MAX,
        LARGE_INPUT.contains(&id),
        "{id}: only the actions named take more than a pipe holds"
    );
    let in_address: Vec<&str> = action
        .path()
        .split('/')
        .filter_map(|part| part.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')))
        .collect();
    for field in &inputs {
        let name = field.name;
        assert!(!in_address.contains(&name), "{id}: {name}, which it reads, is in its address");
        assert!(
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{id}: {name} is a shell variable's name in small letters"
        );
        let fits = match field.kind {
            Type::Secret { max } => max >= 1,
            Type::Document { max, .. } => (1..=DOCUMENT_MAX).contains(&max),
            _ => false,
        };
        assert!(fits, "{id}: {name}");
    }
    let (good, _, _) = action.example_values();
    for example in good {
        // The examples list a secret as `(secret)`, never as itself.
        let mut args = example.clone();
        let (mut given, mut read) = (Vec::new(), Vec::new());
        for field in &inputs {
            let Some(value) = args.get(field.name).filter(|value| !value.is_null()) else {
                continue;
            };
            let text = if field.is_secret() {
                assert_eq!(value, SHOWN, "{id}: its example lists {}", field.name);
                args[field.name] = Value::from(marker(field.name));
                given.push(marker(field.name));
                marker(field.name)
            } else {
                value.as_str().unwrap_or_else(|| panic!("{id}: {} is text", field.name)).into()
            };
            read.push(match field.kind {
                Type::Secret { .. } => text + "\n",
                _ => text,
            });
        }
        let prepared =
            action.prepare(args.clone()).unwrap_or_else(|f| panic!("{id}: {}", f.detail));
        assert_eq!(
            prepared.input.as_ref().map(|input| input.bytes().to_vec()),
            (!read.is_empty()).then(|| read.concat().into_bytes()),
            "{id}: its command reads each secret given as a line and each document as it is, \
             in the order of its fields"
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
        match read.len() {
            0 => assert!(task.params.get("stdin").is_none(), "{id}"),
            _ => {
                assert_eq!(task.module, "ansible.builtin.command", "{id}: stdin is the command's");
                let names: Vec<String> = inputs
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

/// Values to try a document field of up to `max` bytes with. None has a
/// character of more than one byte: the schema counts characters, and the
/// action bytes.
pub(super) fn document_candidates(max: usize) -> Vec<Value> {
    let mut values = vec![
        json!("a"),
        json!("line one\nline two\n"),
        json!("a\r\nb\r\n"),
        json!("\"quoted\" $HOME `x`\n"),
        json!("a".repeat(max)),
        json!("a".repeat(max + 1)),
        json!(""),
        json!("a\u{0}b"),
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

/// A change for the tests alone that adds a certificate from its key and
/// its chain, which its command reads on its input, more than a pipe holds
/// at once, as `cert add NAME` does (ADR 0567): the key, a secret, and then
/// the chain, which is not.
struct Certificate;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct KeyAndChain {
    key: Secret,
    chain: String,
}

impl Args for KeyAndChain {
    const FIELDS: &'static [Field] = &[
        Field::secret_document("key", "Its private key, in PEM.", DOCUMENT_MAX),
        Field::document("chain", "Its chain, in PEM.", DOCUMENT_MAX),
    ];

    fn check(&self) -> Result<(), Bad> {
        Ok(())
    }
}

impl Action for Certificate {
    const ID: &'static str = "lab.certificate";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/certificate";
    const RUNS: Runs = Runs::AsAdministrator;
    const INPUT_MAX: usize = 2 * DOCUMENT_MAX;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add the lab's certificate";
    type Args = KeyAndChain;

    fn argv(_: &KeyAndChain) -> Vec<String> {
        ["admin", "lab-steps", "1"].map(String::from).to_vec()
    }

    fn input(args: &KeyAndChain) -> Option<Secret> {
        // The room is taken once, as `Secret::lines` takes it.
        let mut text = String::with_capacity(args.key.bytes().len() + args.chain.len());
        text.push_str(std::str::from_utf8(args.key.bytes()).ok()?);
        text.push_str(&args.chain);
        Some(Secret::from(text))
    }

    fn examples() -> Examples<KeyAndChain> {
        Examples {
            good: vec![KeyAndChain {
                key: Secret::from(KEY_PEM.to_string()),
                chain: CHAIN_PEM.into(),
            }],
            bad: vec![
                json!({ "key": "", "chain": CHAIN_PEM }),
                json!({ "key": KEY_PEM, "chain": "a\u{0}b" }),
                json!({ "key": KEY_PEM }),
            ],
            undo: None,
        }
    }
}

/// A key and a chain, as the lab's certificate gives them.
const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nTUFSS0VS\n-----END PRIVATE KEY-----\n";
const CHAIN_PEM: &str = "-----BEGIN CERTIFICATE-----\nQ0hBSU4=\n-----END CERTIFICATE-----\n";

static PASSWORD: Entry<Password> = Entry::new();
static OWN_PASSWORD: Entry<OwnPassword> = Entry::new();
static UNLOCK: Entry<Unlock> = Entry::new();
static KEY: Entry<Key> = Entry::new();
static RESTORE: Entry<Restore> = Entry::new();
/// These tests' actions that take or answer a secret, which the route
/// tests use too.
pub static SECRETS: &[&dyn Registered] = &[&PASSWORD, &OWN_PASSWORD, &UNLOCK, &KEY, &RESTORE];

static CERTIFICATE: Entry<Certificate> = Entry::new();
/// These tests' action that takes documents.
pub static DOCUMENTS: &[&dyn Registered] = &[&CERTIFICATE];

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

/// The body word of a `curl` line for `plain` arguments and `secrets`,
/// none of them a document.
fn show_body(plain: &Map<String, Value>, secrets: &[&str]) -> String {
    let lines: Vec<(&str, bool)> = secrets.iter().map(|name| (*name, false)).collect();
    show::body_with_variables(plain, &lines)
}

#[test]
fn a_document_field_s_schema_is_text_of_many_lines_and_a_secret_one_is_never_given_back() {
    let fields = [
        Field::secret_document("key", "Its key.", 100),
        Field::document("chain", "Its chain.", 200).optional(),
    ];
    assert_eq!(
        schema(&fields),
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string", "description": "Its key.",
                    "minLength": 1, "maxLength": 100, "pattern": "^[^\0]*$",
                    "writeOnly": true,
                },
                "chain": {
                    "type": "string", "description": "Its chain.",
                    "minLength": 1, "maxLength": 200, "pattern": "^[^\0]*$",
                },
            },
            "required": ["key"],
            "additionalProperties": false,
        })
    );
    assert!(fields[0].is_secret() && fields[0].is_input());
    assert!(!fields[1].is_secret() && fields[1].is_input() && !fields[1].required);
    assert_eq!((fields[0].input_bytes(), fields[1].input_bytes()), (100, 200));
    let line = Field::secret("password", "A password.", 10);
    assert_eq!((line.input_bytes(), Field::flag("a", "A.").input_bytes()), (line_bytes(10), 0));
    for (text, fits) in [("a\nb\r\n", true), ("", true), ("a\0b", false)] {
        assert_eq!(validate::matches(DOCUMENT_PATTERN, text).unwrap(), fits, "{text:?}");
    }
    // A document of the most bytes is matched without a deep search.
    assert!(validate::matches(DOCUMENT_PATTERN, &"a\n".repeat(DOCUMENT_MAX / 2)).unwrap());
}

#[test]
fn a_document_is_checked_in_bytes_before_it_is_read_and_a_refusal_never_quotes_it() {
    let certificate = DOCUMENTS[0];
    let with = |key: Value, chain: Value| json!({ "key": key, "chain": chain });
    // The key and the chain, each of the most bytes, are its input together.
    let (key, chain) = ("k".repeat(DOCUMENT_MAX), "c".repeat(DOCUMENT_MAX));
    let most = certificate.prepare(with(json!(key), json!(chain))).unwrap();
    let input = most.input.as_ref().unwrap();
    assert_eq!(input.bytes().len(), INPUT_LIMIT);
    assert!(input.bytes().starts_with(b"kkk") && input.bytes().ends_with(b"ccc"));
    assert_eq!(certificate.input_max(), INPUT_LIMIT);
    let ok = certificate.prepare(with(json!(KEY_PEM), json!(CHAIN_PEM))).unwrap();
    assert_eq!(ok.input.unwrap().bytes(), format!("{KEY_PEM}{CHAIN_PEM}").as_bytes());
    // Bytes, not characters: 32,769 of `é` are 65,538 bytes.
    let wide = "\u{e9}".repeat(DOCUMENT_MAX / 2 + 1);
    let marked = format!("mk-key-5f1e{}", "x".repeat(DOCUMENT_MAX));
    for (key, chain, kind) in [
        (json!(wide), json!(CHAIN_PEM), Kind::BadValue),
        (json!(marked), json!(CHAIN_PEM), Kind::BadValue),
        (json!(""), json!(CHAIN_PEM), Kind::BadValue),
        (json!("mk-key-5f1e\0"), json!(CHAIN_PEM), Kind::BadValue),
        (json!(KEY_PEM), json!("c".repeat(DOCUMENT_MAX + 1)), Kind::BadValue),
        (json!(["mk-key-5f1e"]), json!(CHAIN_PEM), Kind::BadRequest),
    ] {
        let refused = certificate.prepare(with(key.clone(), chain)).unwrap_err();
        assert_eq!(refused.kind, kind, "{key}");
        assert!(!format!("{refused:?}").contains("mk-"), "{refused:?}");
        if kind == Kind::BadValue {
            assert!(refused.detail.contains("bytes, with no NUL"), "{}", refused.detail);
        }
    }
}

#[test]
fn a_document_is_shown_as_given_on_the_input_and_a_secret_one_from_a_json_variable() {
    let certificate = DOCUMENTS[0];
    let prepared = show::prepare(certificate, json!({ "chain": CHAIN_PEM })).unwrap();
    assert_eq!(show::typed(&prepared).as_deref(), Some("with the key and the chain on its input"));
    let shown = show::shown(&prepared, "https://nas.example:9090");
    let api = shown["api"].as_str().unwrap();
    assert!(api.contains("$KEY_JSON") && !api.contains("PRIVATE KEY"), "{api}");
    let body = api.split(" -d ").nth(1).unwrap().rsplit_once(' ').unwrap().0.to_owned();
    let made = std::process::Command::new("sh")
        .args(["-c", &format!("printf %s {body}")])
        .env("KEY_JSON", json!(KEY_PEM).to_string())
        .output()
        .unwrap();
    let made: Value = serde_json::from_slice(&made.stdout).unwrap();
    assert_eq!(made, json!({ "chain": CHAIN_PEM, "key": KEY_PEM }));
    let task = &prepared.ansible;
    assert_eq!(task.params["stdin"], "{{ key }}\n{{ chain }}");
    assert!(task.no_log);
}
