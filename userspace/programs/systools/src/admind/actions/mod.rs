// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's actions (ADR 0515): every read and every change the API
//! offers, each a typed value that says what it runs.
//!
//! An action names its area (the page it belongs to, and the scope a
//! token needs), whether it changes anything, its route, who runs it, the
//! arguments it takes with their checks, the command those make, its
//! Ansible task, the files a change writes, the error IDs its command can
//! give, and its five tests. The command is a list of words, never text
//! for a shell, and it is both what runs and what the console shows: the
//! "Command" a person copies is the command the button ran.
//!
//! A read runs in the person's worker, as the person, with `--json`, and
//! the API answers the JSON as the command printed it: a command's JSON
//! is its documented interface, so the API and the command cannot drift
//! apart. A change runs as an administrator through authd's
//! `console-elevate` when its action says so, and needs the session
//! unlocked or a token whose scopes name the area; otherwise it runs in
//! the worker as the person, whose own rights decide.
//!
//! A change's answer says whether it changed anything. An action with a
//! probe, a read of what the change moves, has it run in the person's
//! worker before the change and again after, and `changed` is whether the
//! part of the two outputs that [`Action::probed`] picks differs, so a
//! probe can read only what the person can. An action without a probe
//! always did something. The list says which are which, as `compares`.
//!
//! A secret, a password or a key, is never one of a command's words
//! (ADR 0561). A field made with [`Field::secret`] is read into a
//! [`Secret`], which the action gives its command on its standard input
//! ([`Action::input`]) and which shows as `(secret)` wherever it would be
//! printed or kept. An action whose command prints a secret says so
//! ([`Action::SECRET_ANSWER`]). Either is a `POST`, a read as well as a
//! change, so that a secret is never in an address.
//!
//! An action's Ansible task is the module of the collection `dukovo.system`
//! for it where the collection has one: the services' changes are
//! `dukovo.system.service`, which compares the service's state itself and
//! reports no change the second time. An action the collection has no
//! module for, the certificates' renewal and the console's reach, is
//! `ansible.builtin.command` with the command's words, which cannot tell a
//! change from none.
//!
//! [`REGISTRY`] is the one list. The route table, `GET /api/1/actions`,
//! `admin actions` and `lab/actions.toml` are all made from it.

mod account;
mod accounts;
mod alerts;
mod apps;
mod backups;
mod bisect;
mod certificate;
pub mod checks;
mod console;
mod firewall;
mod hardware;
#[cfg(any(test, feature = "lab"))]
pub mod lab;
pub mod logs;
mod machine;
mod metrics;
mod moving;
mod network;
mod power;
mod private;
mod reliability;
mod reports;
pub mod secret;
mod security;
mod service;
pub mod show;
mod snapshots;
mod ssh;
mod storage;
mod trial;
mod updates;

use std::fmt;
use std::net::IpAddr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub use self::secret::Secret;
use super::errors::{Failure, Kind};
use super::scopes::{Needed, Scopes};

/// Who runs an action's command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Runs {
    /// The person's worker, with the person's own rights.
    AsThePerson,
    /// authd, as an administrator, for an unlocked session or a token
    /// whose owner is an administrator.
    AsAdministrator,
}

/// An action's five tests, by the names the gate finds them by
/// (`./ob lab coverage --check`).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Tests {
    /// The host unit test of its arguments: good and bad ones, and the
    /// command each good one makes.
    pub checks: &'static str,
    /// The boot test case that calls its route as each kind of caller.
    pub route: &'static str,
    /// The boot test case that runs its shown command and compares.
    pub same_as_command: &'static str,
    /// The lab test that uses it in each browser.
    pub browser: &'static str,
    /// The lab test that runs its Ansible task twice.
    pub ansible: &'static str,
}

/// Why an argument cannot be used, in words the page shows under its
/// field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bad {
    /// The argument, as the API names it.
    pub field: &'static str,
    /// What is wrong and what was not done: "Ports go from 1 to 65535.
    /// Nothing was added."
    pub why: String,
}

/// What an argument's value is, and the limits it has, as the schema of
/// the action's input says it (ADR 0515).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// Text of `min` to `max` characters. When there is a `pattern`, the
    /// text matches it: a regular expression of the kind JSON Schema takes,
    /// which finds a match anywhere in the text, so it says `^` and `$`
    /// where it means the whole.
    Text {
        /// The fewest characters.
        min: usize,
        /// The most characters.
        max: usize,
        /// What the text matches, written as `check` enforces it.
        pattern: Option<&'static str>,
    },
    /// One of these words.
    OneOf(&'static [&'static str]),
    /// A whole number from `min` to `max`, both included.
    Number {
        /// The least.
        min: i64,
        /// The greatest.
        max: i64,
    },
    /// `true` or `false`.
    Flag,
    /// A password or a key: one line of 1 to `max` characters, with no
    /// NUL, read into a [`Secret`] (ADR 0561). The schema marks it
    /// `writeOnly`: it is sent, and never given back.
    Secret {
        /// The most characters.
        max: usize,
    },
    /// Text of many lines, 1 to `max` bytes with no NUL, that the action's
    /// command reads on its input as it is (ADR 0567): a certificate's
    /// chain, or its key. A `secret` one is read into a [`Secret`], as a
    /// secret line is, and marked `writeOnly`.
    Document {
        /// The most bytes.
        max: usize,
        /// Whether it is a secret.
        secret: bool,
    },
}

/// What a secret field's text matches: no line end and no NUL. The
/// characters are in the class as they are, which a JSON Schema pattern
/// (ECMAScript's) reads as written.
pub const SECRET_PATTERN: &str = "^[^\n\r\0]*$";

/// What a document's text matches: no NUL.
pub const DOCUMENT_PATTERN: &str = "^[^\0]*$";

/// The most bytes a document field takes.
pub const DOCUMENT_MAX: usize = 64 << 10;

/// One argument of an action, as its input's schema describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// The argument, as the API names it.
    pub name: &'static str,
    /// What its value is.
    pub kind: Type,
    /// Whether the action cannot be asked for without it.
    pub required: bool,
    /// What a person reads under it: one line, in plain words.
    pub about: &'static str,
}

impl Field {
    /// A required argument that is text of `min` to `max` characters,
    /// matching `pattern` if there is one.
    pub const fn text(
        name: &'static str,
        about: &'static str,
        min: usize,
        max: usize,
        pattern: Option<&'static str>,
    ) -> Field {
        Field { name, kind: Type::Text { min, max, pattern }, required: true, about }
    }

    /// A required argument that is one of `words`.
    pub const fn one_of(
        name: &'static str,
        about: &'static str,
        words: &'static [&'static str],
    ) -> Field {
        Field { name, kind: Type::OneOf(words), required: true, about }
    }

    /// A required argument that is a whole number from `min` to `max`.
    pub const fn number(name: &'static str, about: &'static str, min: i64, max: i64) -> Field {
        Field { name, kind: Type::Number { min, max }, required: true, about }
    }

    /// A required argument that is `true` or `false`.
    pub const fn flag(name: &'static str, about: &'static str) -> Field {
        Field { name, kind: Type::Flag, required: true, about }
    }

    /// A required argument that is a secret of 1 to `max` characters,
    /// which the action's command reads on its input ([`Action::input`]).
    pub const fn secret(name: &'static str, about: &'static str, max: usize) -> Field {
        Field { name, kind: Type::Secret { max }, required: true, about }
    }

    /// A required argument that is a document of 1 to `max` bytes, which
    /// the action's command reads on its input ([`Action::input`]). `max`
    /// is at most [`DOCUMENT_MAX`], which the registry's rules hold.
    pub const fn document(name: &'static str, about: &'static str, max: usize) -> Field {
        Field { name, kind: Type::Document { max, secret: false }, required: true, about }
    }

    /// A required argument that is a document of 1 to `max` bytes, which
    /// is a secret, as a certificate's key is: read into a [`Secret`], and
    /// never shown, kept or put in a log.
    pub const fn secret_document(name: &'static str, about: &'static str, max: usize) -> Field {
        Field { name, kind: Type::Document { max, secret: true }, required: true, about }
    }

    /// Whether it is a secret: a line or a document.
    pub const fn is_secret(&self) -> bool {
        matches!(self.kind, Type::Secret { .. } | Type::Document { secret: true, .. })
    }

    /// Whether the command reads it on its input: a secret or a document.
    pub const fn is_input(&self) -> bool {
        matches!(self.kind, Type::Secret { .. } | Type::Document { .. })
    }

    /// The most bytes it takes of the command's input: a secret line's
    /// four for each character and its end, a document's own most, and
    /// nothing for an argument the command does not read there.
    // The registry's rules hold each action's input to its most by it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const fn input_bytes(&self) -> usize {
        match self.kind {
            Type::Secret { max } => secret::line_bytes(max),
            Type::Document { max, .. } => max,
            _ => 0,
        }
    }

    /// The same argument, which the action can be asked for without. A
    /// client leaves it out; a `null` in its place is read as left out too,
    /// which the schema does not say, since it only holds what a client
    /// ought to send.
    pub const fn optional(self) -> Field {
        Field { required: false, ..self }
    }
}

/// The JSON Schema (draft 2020-12, the part with `type`, `description`,
/// `minLength`, `maxLength`, `pattern`, `enum`, `minimum`, `maximum`,
/// `writeOnly`, `required` and `additionalProperties`) of the arguments
/// `fields` describe: an object that has these and no others, so that a
/// client can check what it is about to send.
pub fn schema(fields: &[Field]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        let mut property = Map::new();
        let kind = match field.kind {
            Type::Text { min, max, pattern } => {
                property.insert("minLength".into(), min.into());
                property.insert("maxLength".into(), max.into());
                if let Some(pattern) = pattern {
                    property.insert("pattern".into(), pattern.into());
                }
                "string"
            }
            Type::OneOf(words) => {
                property.insert("enum".into(), Value::from(words.to_vec()));
                "string"
            }
            Type::Number { min, max } => {
                property.insert("minimum".into(), min.into());
                property.insert("maximum".into(), max.into());
                "integer"
            }
            Type::Flag => "boolean",
            Type::Secret { max } => {
                property.insert("minLength".into(), 1.into());
                property.insert("maxLength".into(), max.into());
                property.insert("pattern".into(), SECRET_PATTERN.into());
                property.insert("writeOnly".into(), true.into());
                "string"
            }
            // JSON Schema counts characters, and the action bytes: a text of
            // `max` characters that takes more bytes fits the schema and is
            // refused, which the action's words say.
            Type::Document { max, secret } => {
                property.insert("minLength".into(), 1.into());
                property.insert("maxLength".into(), max.into());
                property.insert("pattern".into(), DOCUMENT_PATTERN.into());
                if secret {
                    property.insert("writeOnly".into(), true.into());
                }
                "string"
            }
        };
        property.insert("type".into(), kind.into());
        property.insert("description".into(), field.about.into());
        properties.insert(field.name.into(), Value::Object(property));
        if field.required {
            required.push(field.name);
        }
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// An action's arguments: JSON in, checked before anything runs.
pub trait Args: DeserializeOwned + Serialize {
    /// Each argument it takes, which the schema of the action's input
    /// says. They say what [`Args::check`] and the types enforce, no more
    /// and no less.
    const FIELDS: &'static [Field];

    /// Whether the arguments can work, beyond their types.
    ///
    /// # Errors
    ///
    /// The first argument that cannot, and why.
    fn check(&self) -> Result<(), Bad>;
}

/// An action that takes nothing.
#[derive(Debug, Default, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Nothing {}

impl Args for Nothing {
    const FIELDS: &'static [Field] = &[];

    fn check(&self) -> Result<(), Bad> {
        Ok(())
    }
}

/// An action's Ansible task: a module of the collection `dukovo.system`
/// with its parameters, for an action the collection has a module for, or
/// `ansible.builtin.command` with the command's words. A module says
/// itself whether it changed anything, so its task has no `changed_when`;
/// a command cannot, so a change's task reports "changed" on every run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ansible {
    /// The task's name, as a person reads it in a play's output.
    pub name: String,
    /// The module: `dukovo.system.service`, or `ansible.builtin.command`.
    pub module: &'static str,
    /// The module's parameters, in the order of their names.
    pub params: Map<String, Value>,
    /// Whether the task runs as root (`become: true`).
    pub become_root: bool,
    /// `changed_when`, when the module does not say: `false` for a read.
    pub changed_when: Option<String>,
    /// The name the task's result is kept under, for `failed_when`.
    pub register: Option<&'static str>,
    /// `failed_when`, for a command whose answer comes with a status other
    /// than 0 as well ([`Action::ANSWERS`]).
    pub failed_when: Option<String>,
    /// `no_log: true`, for a task that gives its command a secret or whose
    /// command prints one (ADR 0561): Ansible then shows neither.
    pub no_log: bool,
    /// `when`, a condition the playbook must meet for the task to run, for
    /// a change that is only done on the playbook's own word.
    pub when: Option<String>,
    /// Whether the machine restarts after the task: the play then waits for
    /// it to answer again before its next task (ADR 0565).
    pub comes_back: bool,
}

impl Ansible {
    /// `ansible.builtin.command` running `argv`, as root when `elevated`,
    /// changing nothing when `changes` is false.
    pub fn command(
        name: impl Into<String>,
        argv: &[String],
        elevated: bool,
        changes: bool,
    ) -> Ansible {
        let mut params = Map::new();
        params.insert("argv".into(), Value::from(argv.to_vec()));
        Ansible {
            name: name.into(),
            module: "ansible.builtin.command",
            params,
            become_root: elevated,
            changed_when: (!changes).then(|| "false".into()),
            register: None,
            failed_when: None,
            no_log: false,
            when: None,
            comes_back: false,
        }
    }

    /// The task that runs `module` of the collection with `params`, as
    /// root when `elevated`. The module says whether it changed anything.
    pub fn module(
        name: impl Into<String>,
        module: &'static str,
        params: Map<String, Value>,
        elevated: bool,
    ) -> Ansible {
        Ansible {
            name: name.into(),
            module,
            params,
            become_root: elevated,
            changed_when: None,
            register: None,
            failed_when: None,
            no_log: false,
            when: None,
            comes_back: false,
        }
    }

    /// The task, failed only when its command ends with a status that is
    /// none of `answers`: the statuses after which its output is the
    /// answer. A task whose command answers only with 0 is left as it is.
    #[must_use]
    pub fn answering(mut self, answers: &[i32]) -> Ansible {
        if answers != [0] {
            let statuses: Vec<String> = answers.iter().map(i32::to_string).collect();
            self.register = Some(RESULT);
            self.failed_when = Some(format!("{RESULT}.rc not in [{}]", statuses.join(", ")));
        }
        self
    }

    /// The task, for a change that restarts the machine a minute after it
    /// answers: the play waits for the machine to answer again.
    #[must_use]
    pub fn coming_back(mut self) -> Ansible {
        self.comes_back = true;
        self
    }

    /// The task, run only when `condition`, an expression of the
    /// playbook's variables, holds.
    #[must_use]
    pub fn when(mut self, condition: &str) -> Ansible {
        self.when = Some(condition.to_string());
        self
    }

    /// The task, for an action whose fields are `fields`, with `args` as
    /// they serialize, and whose command prints a secret when `answer`
    /// (ADR 0561). Each secret and each document `args` gives is read from
    /// the playbook's variable of the field's name, on the command's input
    /// in the fields' order, as [`Action::input`] gives them: a secret line
    /// with its end, a document as it is (ADR 0567). Ansible shows nothing
    /// of a task that has a secret, and keeps the answer of one whose
    /// command prints one for the playbook.
    #[must_use]
    pub fn keeping_secrets(mut self, fields: &[Field], args: &Value, answer: bool) -> Ansible {
        let given: Vec<&Field> = fields
            .iter()
            .filter(|field| field.is_input() && args.get(field.name).is_some_and(|v| !v.is_null()))
            .collect();
        if !given.is_empty() {
            let stdin: Vec<String> =
                given.iter().map(|field| format!("{{{{ {} }}}}", field.name)).collect();
            self.params.insert("stdin".into(), stdin.join("\n").into());
            self.no_log |= given.iter().any(|field| field.is_secret());
        }
        if answer {
            self.register = Some(RESULT);
            self.no_log = true;
        }
        self
    }
}

/// The name an Ansible task keeps its result under when its failure is
/// told by [`Ansible::failed_when`], or when its answer is a secret.
const RESULT: &str = "answer";

/// Arguments to try an action with, for the boot and lab tests that take
/// every action from the list.
pub struct Examples<A> {
    /// Arguments that work on a machine the tests set up.
    pub good: Vec<A>,
    /// Arguments that cannot work, each refused by `check` or by the
    /// command: the refusal's field and words are in the unit test.
    pub bad: Vec<Value>,
    /// For a change, what puts back what `good[0]` did, so a test leaves
    /// the machine as it found it.
    pub undo: Option<Undo>,
}

/// The action that puts back what an example did, and the arguments it
/// takes to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Undo {
    /// The id of the action, which is a change of the list: another, or
    /// the action itself where it sets one value, as `console.reach` does.
    pub action: &'static str,
    /// Its arguments, as its route takes them.
    pub args: Value,
}

/// The area of the actions that set and cancel a restart or a shut down:
/// admind reads Spark as soon as one of its changes answers, as it does
/// after any change that restarts the machine, so that the open tabs are
/// told at once (`event: power`, ADR 0565).
pub const POWER: &str = power::AREA;

/// The ID admind refuses a change that restarts the machine with, and its
/// title, while the other kind of restart is set or runs: the updates'
/// UPD-106, since it is about them, with the status of ADM-128.
pub const OTHER_RESTART: (&str, &str) = ("UPD-106", "A restart is pending, or changes bisect runs");

/// What a change that restarts the machine is, for the guard that refuses
/// it while something a restart would spoil runs (ADR 0565).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restarts {
    /// It restarts or powers off the machine once, a minute from when it
    /// answers. It is refused while a trial of settings runs, which the
    /// restart would undo unasked, and while the search for a bad change
    /// runs, whose restarts are its own.
    Machine,
    /// It starts or goes on with the search for a bad change, which
    /// restarts the machine once for each try. It is refused while a
    /// trial runs, and while a restart or a shut down is set.
    Search,
}

/// The task of `A` run with `args` that runs its command: named by its
/// title and subject, as root when an administrator runs it, failed only
/// by a status that gives no answer. An action's task unless it says
/// otherwise.
pub fn command_task<A: Action + ?Sized>(args: &A::Args) -> Ansible {
    let name = match A::subject(args) {
        Some(subject) => format!("{}: {subject}", A::TITLE),
        None => A::TITLE.to_string(),
    };
    let elevated = A::RUNS == Runs::AsAdministrator;
    Ansible::command(name, &A::argv(args), elevated, A::CHANGES).answering(A::ANSWERS)
}

/// One action of the console.
pub trait Action: 'static {
    /// `area.verb`, such as `service.restart`.
    const ID: &'static str;
    /// The page it belongs to, and the scope a token needs:
    /// [`Scopes::allow`].
    const AREA: &'static str;
    /// Whether it changes anything. A read is a `GET`, a change a `POST`.
    const CHANGES: bool;
    /// Its address, with `{name}` for each part that is an argument:
    /// `/api/1/services/{name}/restart`.
    const PATH: &'static str;
    /// Who runs its command.
    const RUNS: Runs;
    /// Whether it may take longer than a request waits, and so runs as a
    /// task (C2c).
    const LONG: bool = false;
    /// The error IDs its command can give, besides the console's own.
    const ERRORS: &'static [&'static str] = &[];
    /// The exit statuses after which its command's output is the answer:
    /// 0, and for a read whose command also tells a state by its status,
    /// as `service NAME explain` exits 3 for a service that does not run,
    /// those statuses too. Any other status is ADM-123.
    const ANSWERS: &'static [i32] = &[0];
    /// Whether its command prints one JSON value a line, as `log -o json`
    /// does: the answer is then the array of the lines' values.
    const LINES: bool = false;
    /// Whether its command prints a secret, as `backup key NAME` prints a
    /// recovery key (ADR 0561). Its route is a `POST`, a read's too, and it
    /// is never a task, which keeps what its command prints.
    const SECRET_ANSWER: bool = false;
    /// The most bytes its command is given on its input: what a pipe
    /// holds, unless it takes a document that does not fit (ADR 0567),
    /// which is written while the command runs. The registry's rules hold
    /// its input fields to it, and name the actions that take more.
    const INPUT_MAX: usize = secret::INPUT_MAX;
    /// The field that names an account the caller may not do this to from
    /// the console or with their token (ADR 0562, ADM-136): disabling or
    /// removing it, taking its administrator right away, or ending one of
    /// its sessions could lock the caller out, so another administrator,
    /// or `elevate user ...` at a terminal, does it. None, the default, for
    /// an action that does nothing to an account.
    const NOT_SELF: Option<&'static str> = None;
    /// What the change could cut off: the browser that asks for it, when
    /// the browser comes through what the change takes away and Spark
    /// cannot try the change (ADR 0562, ADM-137). None, the default, for an
    /// action that cuts off nothing a trial does not put back.
    const CUTS_OFF: Option<CutsOff<Self::Args>> = None;
    /// Whether it restarts the machine, and how: such a change is refused
    /// while a trial of settings runs (ADM-142) and while the other kind
    /// of restart is set or runs (UPD-106), and when admind cannot tell,
    /// so that the guard fails closed. None, the default, for an action
    /// that restarts nothing. It answers before the machine goes down,
    /// a minute later, so it never needs to be [`Action::LONG`].
    const RESTARTS: Option<Restarts> = None;
    /// Its five tests.
    const TESTS: Tests;
    /// What a person reads as its name in the palette and in the journal:
    /// "Restart a service".
    const TITLE: &'static str;

    /// What it takes.
    type Args: Args;

    /// The command, as words: the program first. A read's has `--json`.
    /// It never holds a secret: [`Secret`] has nothing that gives its text
    /// but [`Secret::bytes`], which only [`Action::input`] uses.
    fn argv(args: &Self::Args) -> Vec<String>;

    /// What the command reads on its standard input: the secrets of
    /// `args`, each a line, in the order of the fields ([`Secret::lines`]),
    /// and a document as it is (ADR 0567). None, the default, for an
    /// action that takes neither, whose command's input ends at once.
    fn input(_args: &Self::Args) -> Option<Secret> {
        None
    }

    /// What the action is done to, which the name of its Ansible task adds
    /// to the title: the service's name. None for an action that takes
    /// none.
    fn subject(_args: &Self::Args) -> Option<String> {
        None
    }

    /// The Ansible task that does the same: a task of the command, unless
    /// the action overrides this with the collection's module for it.
    fn ansible(args: &Self::Args) -> Ansible {
        command_task::<Self>(args)
    }

    /// The files a change writes, for the Files tab and the journal.
    fn files(_args: &Self::Args) -> Vec<String> {
        Vec::new()
    }

    /// A read that says whether a change moved what it changes: the
    /// command, as words, which runs as the person before the change and
    /// again after it (ADR 0515). The change's `changed` is whether the
    /// two outputs differ in the part [`Action::probed`] picks.
    ///
    /// None, the default, is for a change that always does something, as
    /// a restart does: its `changed` is always true. A probe is there for
    /// all of an action's arguments or for none of them.
    fn probe(_args: &Self::Args) -> Option<Vec<String>> {
        None
    }

    /// The part of the probe's `output` that this change moves, as text to
    /// compare. The default is the whole output, trimmed. None when the
    /// output does not say: the thing is not in it, or it does not read.
    fn probed(_args: &Self::Args, output: &str) -> Option<String> {
        Some(output.trim().to_string())
    }

    /// What the change with `args` does to a trial of settings, which
    /// Spark undoes unless it is kept (ADR 0518): none by default.
    fn trial(_args: &Self::Args) -> Option<Trial> {
        None
    }

    /// Whether [`Action::NOT_SELF`] applies to `args`: always, the default,
    /// or only for some, as `accounts.set` only when it takes the
    /// administrator right away.
    fn not_self_applies(_args: &Self::Args) -> bool {
        true
    }

    /// Whether `now`, the part of the probe's output that
    /// [`Action::probed`] picks, is already what the change leaves, so that
    /// the change would move nothing of it: for check mode (ADR 0545).
    /// None when it cannot tell, as for a state in passing, which is what
    /// an action that does not say gives.
    fn settled(_args: &Self::Args, _now: &str) -> Option<bool> {
        None
    }

    /// Whether the change would leave the file at `path`, one of
    /// [`Action::files`], as it is, when the file holds `text` now (None
    /// when there is no such file): for check mode (ADR 0545). None when
    /// it cannot tell, which is what an action that does not say gives.
    fn keeps_file(_args: &Self::Args, _path: &str, _text: Option<&str>) -> Option<bool> {
        None
    }

    /// Arguments for the tests that take every action from the list.
    fn examples() -> Examples<Self::Args>;
}

/// [`Action::CUTS_OFF`]: how admind tells, before a change runs, whether it
/// would cut off the browser that asks for it. admind reads no file of the
/// thing the change is to: what it needs comes from `read`, a command run
/// as the caller's administrator, as the change will be.
pub struct CutsOff<A> {
    /// The requests the change can cut off.
    pub on: Through,
    /// The command that says what the change would take away, for these
    /// arguments. It prints no secret: admind reads what it prints.
    pub read: fn(&A) -> Vec<String>,
    /// Whether the change with these arguments leaves `browser` reaching
    /// the console, from what `read` printed: Err with the words of the
    /// refusal when it does not, and when what was printed does not read,
    /// so that the guard fails closed.
    pub cuts: fn(&A, &Browser<'_>, &str) -> Result<(), String>,
}

/// The requests a change can cut off ([`CutsOff::on`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Through {
    /// Those that came by this network card, as a device's tunnel: a
    /// request by another card is never cut off, and nothing is read for
    /// it (ADM-137).
    Card(&'static str),
    /// Any request, by the name its browser uses for the console, as a
    /// certificate that does not name it would (ADM-141, ADR 0567).
    Name,
}

impl Through {
    /// What `GET /api/1/actions` lists of it: the card's name, or `*` for
    /// a guard of any request.
    pub fn listed(self) -> &'static str {
        match self {
            Through::Card(card) => card,
            Through::Name => "*",
        }
    }
}

/// The browser that asks for a change, as [`CutsOff::cuts`] sees it.
#[derive(Clone, Copy, Debug)]
pub struct Browser<'a> {
    /// Its address, an IPv4 address as itself.
    pub from: IpAddr,
    /// The name or address it asked for, in small letters, without the
    /// port: the request's `Host`.
    pub host: &'a str,
}

/// The read of [`Action::CUTS_OFF`] for a change's arguments.
#[derive(Debug)]
pub struct CutsOffRead {
    /// The requests the change can cut off.
    pub on: Through,
    /// The command to run, as the caller's administrator, before the change.
    pub argv: Vec<String>,
}

/// What a change does to a trial of settings, by the trial's ID: admind
/// keeps when one began, so that a page can tell whether its browser
/// reaches the console over a connection opened after the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trial {
    /// It begins the trial.
    Begins(&'static str),
    /// It keeps or undoes it.
    Ends(&'static str),
}

/// An action ready to run: its checked arguments and what they make.
#[derive(Debug)]
pub struct Prepared {
    pub action: &'static dyn Registered,
    /// The arguments, as the action read them.
    pub args: Value,
    pub argv: Vec<String>,
    pub ansible: Ansible,
    pub files: Vec<String>,
    /// The command that reads what a change moves, for `changed`: None for
    /// a read, and for a change that always does something.
    pub probe: Option<Vec<String>>,
    /// What it does to a trial, when it answers well.
    pub trial: Option<Trial>,
    /// The account that [`Action::NOT_SELF`] names, when the guard
    /// applies to these arguments: the caller's own is refused.
    pub not_self: Option<String>,
    /// What [`Action::CUTS_OFF`] reads before the change, when the action
    /// has one.
    pub cuts_off: Option<CutsOffRead>,
    /// What its command reads on its input: the secrets it takes, as lines
    /// ([`Action::input`]), never kept and overwritten when it goes.
    pub input: Option<Secret>,
}

/// An action in the list, whatever its arguments' type.
pub trait Registered: Sync + fmt::Debug {
    fn id(&self) -> &'static str;
    fn area(&self) -> &'static str;
    fn changes(&self) -> bool;
    fn path(&self) -> &'static str;
    fn runs(&self) -> Runs;
    fn long(&self) -> bool;
    fn errors(&self) -> &'static [&'static str];
    /// [`Action::ANSWERS`].
    fn answers(&self) -> &'static [i32];
    /// [`Action::LINES`].
    fn lines(&self) -> bool;
    /// [`Action::SECRET_ANSWER`].
    fn secret_answer(&self) -> bool;
    /// [`Action::INPUT_MAX`], which the registry's rules check.
    #[cfg_attr(not(test), allow(dead_code))]
    fn input_max(&self) -> usize;
    /// [`Action::NOT_SELF`].
    fn not_self(&self) -> Option<&'static str>;
    /// The requests [`Action::CUTS_OFF`] guards, None for an action without
    /// one.
    fn cuts_off_on(&self) -> Option<Through>;
    /// Whether the change with the prepared `args` leaves `browser`
    /// reaching the console, from what [`CutsOffRead::argv`] printed
    /// (`shown`): Err with the refusal's words when it does not, when
    /// `shown` does not read, or when `args` do not read as the action's.
    /// Ok for an action with no [`Action::CUTS_OFF`].
    ///
    /// # Errors
    ///
    /// The words of the refusal, as above.
    fn cuts_off(&self, args: &Value, browser: &Browser<'_>, shown: &str) -> Result<(), String>;
    /// [`Action::RESTARTS`].
    fn restarts(&self) -> Option<Restarts>;
    fn tests(&self) -> Tests;
    fn title(&self) -> &'static str;
    /// Whether a change's `changed` comes from comparing, its probe's
    /// reads or the files it names (the journal's), and not from being
    /// always true. A read has none, so it is false.
    fn compares(&self) -> bool;
    /// The arguments it takes.
    fn fields(&self) -> &'static [Field];
    /// Reads and checks `args` and makes the command and the rest of
    /// them.
    ///
    /// # Errors
    ///
    /// ADM-108 when `args` is not what the action takes, ADM-125 when a
    /// value cannot work.
    fn prepare(&'static self, args: Value) -> Result<Prepared, Failure>;
    /// The part of the probe's `output` that the change with the prepared
    /// `args` moves, as text to compare. None when the output does not say,
    /// or `args` do not read as the action's.
    fn probed(&self, args: &Value, output: &str) -> Option<String>;
    /// [`Action::settled`], for the prepared `args`; None when they do not
    /// read as the action's.
    fn settled(&self, args: &Value, now: &str) -> Option<bool>;
    /// [`Action::keeps_file`], for the prepared `args`; None when they do
    /// not read as the action's.
    fn keeps_file(&self, args: &Value, path: &str, text: Option<&str>) -> Option<bool>;
    /// The examples, as JSON.
    fn example_values(&self) -> (Vec<Value>, Vec<Value>, Option<Undo>);

    /// Whether its route is a `POST`: a change's, and a read's that takes a
    /// secret or a document or answers a secret, so that the secret is in
    /// a body and never in an address, a browser's history or a log of
    /// addresses (ADR 0561). Any other read is a `GET`.
    fn posts(&self) -> bool {
        self.changes() || self.secret_answer() || self.fields().iter().any(Field::is_input)
    }
}

/// [`Registered`] for the action `A`.
pub struct Entry<A>(std::marker::PhantomData<fn() -> A>);

impl<A> Entry<A> {
    pub const fn new() -> Entry<A> {
        Entry(std::marker::PhantomData)
    }
}

impl<A: Action> fmt::Debug for Entry<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(A::ID)
    }
}

impl<A: Action> Registered for Entry<A> {
    fn id(&self) -> &'static str {
        A::ID
    }
    fn area(&self) -> &'static str {
        A::AREA
    }
    fn changes(&self) -> bool {
        A::CHANGES
    }
    fn path(&self) -> &'static str {
        A::PATH
    }
    fn runs(&self) -> Runs {
        A::RUNS
    }
    fn long(&self) -> bool {
        A::LONG
    }
    fn errors(&self) -> &'static [&'static str] {
        A::ERRORS
    }
    fn answers(&self) -> &'static [i32] {
        A::ANSWERS
    }
    fn lines(&self) -> bool {
        A::LINES
    }
    fn secret_answer(&self) -> bool {
        A::SECRET_ANSWER
    }
    fn input_max(&self) -> usize {
        A::INPUT_MAX
    }
    fn not_self(&self) -> Option<&'static str> {
        A::NOT_SELF
    }
    fn cuts_off_on(&self) -> Option<Through> {
        A::CUTS_OFF.map(|cuts_off| cuts_off.on)
    }
    fn cuts_off(&self, args: &Value, browser: &Browser<'_>, shown: &str) -> Result<(), String> {
        let Some(cuts_off) = A::CUTS_OFF else { return Ok(()) };
        let typed: A::Args = serde_json::from_value(args.clone())
            .map_err(|_| format!("the arguments of {} do not read", A::ID))?;
        (cuts_off.cuts)(&typed, browser, shown)
    }
    fn restarts(&self) -> Option<Restarts> {
        A::RESTARTS
    }
    fn tests(&self) -> Tests {
        A::TESTS
    }
    fn title(&self) -> &'static str {
        A::TITLE
    }
    fn fields(&self) -> &'static [Field] {
        A::Args::FIELDS
    }
    fn compares(&self) -> bool {
        // An action has a probe, and files, for all its arguments or for
        // none, which its tests keep, so its first example tells.
        A::CHANGES
            && A::examples()
                .good
                .first()
                .is_some_and(|args| A::probe(args).is_some() || !A::files(args).is_empty())
    }

    fn prepare(&'static self, args: Value) -> Result<Prepared, Failure> {
        // A secret is checked before serde reads it, since serde's words
        // about a value that is not what a field takes can quote it.
        inputs_checked(A::Args::FIELDS, &args)?;
        let typed: A::Args = serde_json::from_value(args).map_err(|err| {
            Failure::new(
                Kind::BadRequest,
                format!("the arguments are not what {} takes: {err}", A::ID),
            )
        })?;
        typed.check().map_err(|bad| Failure::bad_value(bad.field, bad.why))?;
        let argv = A::argv(&typed);
        let files = A::files(&typed);
        let probe = if A::CHANGES { A::probe(&typed) } else { None };
        let trial = A::trial(&typed);
        let input = A::input(&typed);
        if input.as_ref().is_some_and(|input| input.bytes().len() > A::INPUT_MAX) {
            // The fields' limits keep it within, which a unit test holds.
            return Err(Failure::new(Kind::Internal, format!("{}'s input is too long", A::ID)));
        }
        // The secrets serialize as `(secret)`.
        let args = serde_json::to_value(&typed)
            .map_err(|err| Failure::new(Kind::Internal, format!("{} arguments: {err}", A::ID)))?;
        let ansible = A::ansible(&typed).keeping_secrets(A::Args::FIELDS, &args, A::SECRET_ANSWER);
        let not_self = A::NOT_SELF
            .filter(|_| A::not_self_applies(&typed))
            .and_then(|field| args.get(field)?.as_str().map(String::from));
        let cuts_off = A::CUTS_OFF
            .map(|cuts_off| CutsOffRead { on: cuts_off.on, argv: (cuts_off.read)(&typed) });
        Ok(Prepared {
            action: self,
            args,
            argv,
            ansible,
            files,
            probe,
            trial,
            not_self,
            cuts_off,
            input,
        })
    }

    fn probed(&self, args: &Value, output: &str) -> Option<String> {
        let typed: A::Args = serde_json::from_value(args.clone()).ok()?;
        A::probed(&typed, output)
    }

    fn settled(&self, args: &Value, now: &str) -> Option<bool> {
        let typed: A::Args = serde_json::from_value(args.clone()).ok()?;
        A::settled(&typed, now)
    }

    fn keeps_file(&self, args: &Value, path: &str, text: Option<&str>) -> Option<bool> {
        let typed: A::Args = serde_json::from_value(args.clone()).ok()?;
        A::keeps_file(&typed, path, text)
    }

    fn example_values(&self) -> (Vec<Value>, Vec<Value>, Option<Undo>) {
        let examples = A::examples();
        let json = |args: &A::Args| serde_json::to_value(args).unwrap_or(Value::Null);
        (examples.good.iter().map(json).collect(), examples.bad, examples.undo)
    }
}

/// Checks each secret and each document that `args` gives for a field of
/// `fields`: a secret is one line of 1 to the field's most characters, with
/// no NUL (ADR 0561); a document 1 to the field's most bytes, with no NUL
/// (ADR 0567). What is wrong is said without the value.
///
/// # Errors
///
/// ADM-125 for one that is not such text, ADM-108 for one that is not
/// text.
fn inputs_checked(fields: &[Field], args: &Value) -> Result<(), Failure> {
    for field in fields.iter().filter(|field| field.is_input()) {
        match args.get(field.name) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => {
                let why = match field.kind {
                    Type::Secret { max } => {
                        let length = text.chars().count();
                        (length == 0 || length > max || text.contains(['\n', '\r', '\0'])).then(
                            || {
                                format!(
                                    "It is one line of 1 to {max} characters, with no line \
                                     break. Nothing was done."
                                )
                            },
                        )
                    }
                    Type::Document { max, .. } => (text.is_empty()
                        || text.len() > max
                        || text.contains('\0'))
                    .then(|| {
                        format!("It is text of 1 to {max} bytes, with no NUL. Nothing was done.")
                    }),
                    _ => None,
                };
                if let Some(why) = why {
                    return Err(Failure::bad_value(field.name, why));
                }
            }
            Some(_) => {
                let why = format!("\"{}\" is text", field.name);
                return Err(Failure::new(Kind::BadRequest, why));
            }
        }
    }
    Ok(())
}

/// Makes [`REGISTRY`] from the action types, in the order the console
/// lists them.
macro_rules! registry {
    ($($action:ty),* $(,)?) => {
        /// Every action the console has.
        pub static REGISTRY: &[&dyn Registered] = &[$(&Entry::<$action>::new()),*];
    };
}

registry![
    service::List,
    service::Explain,
    service::Status,
    service::Start,
    service::Stop,
    service::Restart,
    service::Enable,
    service::Disable,
    service::Usage,
    service::Timers,
    certificate::List,
    certificate::Show,
    certificate::Renew,
    certificate::Check,
    certificate::Get,
    certificate::Add,
    certificate::Revoke,
    certificate::Remove,
    certificate::History,
    machine::Name,
    machine::Rename,
    console::SetReach,
    console::SetCertificate,
    console::About,
    console::Notices,
    console::Keep,
    console::Revert,
    reliability::Export,
    checks::Last,
    checks::Run,
    metrics::Now,
    metrics::Usage,
    metrics::Around,
    logs::Own,
    logs::All,
    network::Show,
    network::Connections,
    network::Change,
    network::Keep,
    network::Revert,
    firewall::List,
    firewall::Test,
    firewall::Status,
    firewall::Dropped,
    firewall::Add,
    firewall::Replace,
    firewall::Remove,
    firewall::Keep,
    firewall::Revert,
    reliability::Starts,
    reliability::Failed,
    reliability::Start,
    reliability::Dumps,
    reliability::Alerts,
    alerts::List,
    alerts::Channels,
    alerts::Test,
    alerts::Email,
    alerts::EmailRemove,
    alerts::WebhookAdded,
    alerts::WebhookRemove,
    alerts::PushKey,
    alerts::PushAdded,
    alerts::PushRemove,
    alerts::ChannelAboutSet,
    storage::Show,
    storage::Usage,
    storage::Health,
    storage::Scrub,
    storage::Grow,
    storage::Log,
    storage::LogSet,
    storage::LogTrim,
    snapshots::List,
    snapshots::Browse,
    snapshots::BrowseAll,
    snapshots::Take,
    snapshots::Restore,
    snapshots::RestoreAll,
    backups::List,
    // Before Show: its address is one of a place called ssh-key.
    backups::SshKey,
    backups::Show,
    backups::Run,
    backups::Verify,
    backups::Add,
    backups::Remove,
    backups::Key,
    backups::Restore,
    moving::Find,
    moving::Plan,
    moving::Stage,
    moving::Cancel,
    accounts::List,
    accounts::ListAll,
    accounts::Last,
    accounts::Show,
    accounts::Add,
    accounts::Set,
    accounts::Password,
    accounts::Disable,
    accounts::Enable,
    accounts::Remove,
    accounts::KeyAdd,
    accounts::KeyRemove,
    accounts::TokenRevoke,
    accounts::StepsReset,
    accounts::End,
    account::Show,
    account::Name,
    account::Password,
    account::KeyAdd,
    account::KeyRemove,
    security::SignIn,
    security::SignInSet,
    security::Blocked,
    security::Unblock,
    security::NeverBlock,
    security::NeverBlockRemove,
    ssh::Show,
    ssh::SetThem,
    ssh::On,
    ssh::Off,
    ssh::Keep,
    ssh::Revert,
    private::Show,
    private::Serve,
    private::Add,
    private::Remove,
    power::Status,
    power::Restart,
    power::Shutdown,
    power::Cancel,
    bisect::Status,
    bisect::Begin,
    bisect::Stop,
    bisect::Confirm,
    hardware::Show,
    hardware::Inventory,
    hardware::Last,
    hardware::History,
    hardware::Check,
    hardware::Sensors,
    hardware::Highest,
    hardware::Devices,
    hardware::Memtest,
    hardware::MemtestCancel,
    hardware::MemtestStatus,
    updates::Last,
    updates::Check,
    updates::Slots,
    updates::History,
    updates::Advisories,
    updates::Settings,
    updates::Set,
    updates::SetChannel,
    updates::Checks,
    updates::CheckAdd,
    updates::CheckRemove,
    updates::SetEssential,
    updates::Apply,
    updates::Unstage,
    updates::Undoing,
    updates::Hold,
    updates::Unhold,
    updates::Later,
    updates::Failed,
    apps::List,
    apps::Show,
    apps::Update,
    apps::UpdateAll,
    apps::Tonight,
    apps::TonightAll,
    apps::NotTonight,
    apps::Rollback,
    apps::Hold,
    apps::Unhold,
    apps::Remove,
    reports::Crashes,
    reports::Draft,
    reports::Redact,
    reports::SendReport,
    reports::List,
    reports::Status,
    reports::Retry,
    reports::Delete,
];

/// The actions the console offers: [`REGISTRY`], and in a lab build the
/// lab's own, which the tests of tasks and of the error view run ([`lab`]).
pub fn offered() -> &'static [&'static dyn Registered] {
    #[cfg(feature = "lab")]
    {
        static ALL: std::sync::LazyLock<Vec<&'static dyn Registered>> =
            std::sync::LazyLock::new(|| {
                static LONG: Entry<lab::Long> = Entry::new();
                static MISSING: Entry<lab::Missing> = Entry::new();
                let mut all: Vec<&'static dyn Registered> = REGISTRY.to_vec();
                all.push(&LONG);
                all.push(&MISSING);
                all
            });
        &ALL
    }
    #[cfg(not(feature = "lab"))]
    REGISTRY
}

/// The list `GET /api/1/actions` answers and `admin actions` prints: each
/// action's id, title, area, whether it changes anything, its method and
/// address, who runs it, whether it is a task, whether its `changed` is
/// compared from a probe (`compares`), the field naming an account it is
/// never done to by the caller's own (`not_self`, null for none), the
/// card whose browsers it is refused for when it would cut them off
/// (`cuts_off`, null for none), whether it restarts the machine
/// (`restarts`), its error IDs, the names of its five
/// tests, the JSON Schema of its arguments (`input`), and its examples:
/// the arguments that work, and for a change the action and
/// arguments that put back what the first did (null when nothing does).
/// The bad examples are in the unit tests alone.
pub fn listed(actions: &[&dyn Registered]) -> Value {
    Value::Array(
        actions
            .iter()
            .map(|action| {
                let (good, _, undo) = action.example_values();
                serde_json::json!({
                    "id": action.id(),
                    "title": action.title(),
                    "area": action.area(),
                    "changes": action.changes(),
                    "method": if action.posts() { "POST" } else { "GET" },
                    "path": action.path(),
                    "runs": action.runs(),
                    "long": action.long(),
                    "secret_answer": action.secret_answer(),
                    "compares": action.compares(),
                    "not_self": action.not_self(),
                    "cuts_off": action.cuts_off_on().map(Through::listed),
                    "restarts": action.restarts().is_some(),
                    "errors": action.errors(),
                    "tests": action.tests(),
                    "input": schema(action.fields()),
                    "examples": { "good": good, "undo": undo },
                })
            })
            .collect(),
    )
}

/// The arguments an address gave, by the names of its `{name}` parts.
pub type Params = Map<String, Value>;

/// The action whose route a request asks for, with the arguments its
/// address gives, from `actions`.
#[derive(Debug)]
pub enum Found {
    Action(&'static dyn Registered, Params),
    /// An action's address, asked for with the other method.
    WrongMethod,
    None,
}

/// The action at `path` for a request that is a `POST` when `post`, in
/// `actions`.
pub fn find(actions: &'static [&'static dyn Registered], post: bool, path: &str) -> Found {
    let mut wrong_method = false;
    for &action in actions {
        let Some(params) = matches(action.path(), path) else { continue };
        if action.posts() == post {
            return Found::Action(action, params);
        }
        wrong_method = true;
    }
    if wrong_method { Found::WrongMethod } else { Found::None }
}

/// The arguments `path` gives when it is an address of `template`, whose
/// `{name}` parts are each one argument.
pub fn matches(template: &str, path: &str) -> Option<Params> {
    let mut params = Params::new();
    let mut given = path.split('/');
    for part in template.split('/') {
        let have = given.next()?;
        match part.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
            Some(name) => {
                let value = decode(have)?;
                params.insert(name.into(), Value::String(value));
            }
            None if part == have => {}
            None => return None,
        }
    }
    given.next().is_none().then_some(params)
}

/// The longest part of an address that is an argument, decoded.
const PART_MAX: usize = 255;

/// A part of an address with its `%XX` escapes decoded, if it is one
/// argument: not empty, at most 255 bytes, UTF-8, and no NUL, `/` or
/// control character, decoded or not.
pub(crate) fn decode(part: &str) -> Option<String> {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let byte = if bytes[at] == b'%' {
            let hex = bytes.get(at + 1..at + 3)?;
            let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            at += 3;
            value
        } else {
            at += 1;
            bytes[at - 1]
        };
        if byte == b'/' || byte.is_ascii_control() {
            return None;
        }
        out.push(byte);
    }
    if out.is_empty() || out.len() > PART_MAX {
        return None;
    }
    String::from_utf8(out).ok()
}

/// The arguments of a request: those of its address, and its body's,
/// which is a JSON object or nothing. A name in both is refused, so that
/// neither can quietly stand for the other.
///
/// # Errors
///
/// ADM-108 for a body that is not an object, or that names an argument
/// the address gives.
pub fn arguments(mut params: Params, body: &[u8]) -> Result<Value, Failure> {
    if body.is_empty() {
        return Ok(Value::Object(params));
    }
    let given: Value = serde_json::from_slice(body)
        .map_err(|_| Failure::new(Kind::BadRequest, "the body is not JSON"))?;
    let Value::Object(given) = given else {
        return Err(Failure::new(Kind::BadRequest, "the body is a JSON object of arguments"));
    };
    for (name, value) in given {
        if params.contains_key(&name) {
            let why = format!("\"{name}\" is in the address, and is not given again in the body");
            return Err(Failure::new(Kind::BadRequest, why));
        }
        params.insert(name, value);
    }
    Ok(Value::Object(params))
}

/// `params`, the address's `{name}` parts, each read as the type of the
/// field of its name says: a part of an address is text, and a number or a
/// flag in it is not, as one of a query is not. A part that names no field
/// is left as text for the action to refuse.
///
/// # Errors
///
/// ADM-108 for a number or a flag that is not written as one.
pub fn typed_params(mut params: Params, fields: &[Field]) -> Result<Params, Failure> {
    for (name, value) in &mut params {
        if let (Some(field), Value::String(text)) =
            (fields.iter().find(|field| field.name == *name), &*value)
        {
            *value = read_value(field, name, text.clone())?;
        }
    }
    Ok(params)
}

/// The arguments of a read: `params`, the address's `{name}` parts, read
/// as their type reads them, and each of `fields` that `query`, the
/// address's query, gives, as its type reads it: text and a word as given,
/// a number, `true` or `false`. A query is decoded as a form's is, `+` as a
/// space and `%XX` as a byte.
///
/// # Errors
///
/// ADM-108 for a name the read does not take, a name given twice or also
/// in the address, a value that is not text, or one that its type does
/// not read.
pub fn query_arguments(params: Params, query: &str, fields: &[Field]) -> Result<Value, Failure> {
    let bad = |why: String| Failure::new(Kind::BadRequest, why);
    let mut params = typed_params(params, fields)?;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name =
            form_decoded(name).ok_or_else(|| bad("the address's query is not text".into()))?;
        let value = form_decoded(value).ok_or_else(|| bad(format!("\"{name}\" is not text")))?;
        let Some(field) = fields.iter().find(|field| field.name == name) else {
            return Err(bad(format!("this read takes no \"{name}\"")));
        };
        if params.contains_key(&name) {
            return Err(bad(format!("\"{name}\" is given twice")));
        }
        let value = read_value(field, &name, value)?;
        params.insert(name, value);
    }
    Ok(Value::Object(params))
}

/// `text`, the value of `field` as it was written in an address, read as
/// the field's type reads it: text and a word as given, a whole number, or
/// `true` or `false`.
///
/// # Errors
///
/// ADM-108 for a number or a flag that is not written as one.
fn read_value(field: &Field, name: &str, text: String) -> Result<Value, Failure> {
    let bad = |why: String| Failure::new(Kind::BadRequest, why);
    Ok(match field.kind {
        Type::Text { .. } | Type::OneOf(_) => Value::String(text),
        Type::Number { .. } => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| bad(format!("\"{name}\" is a whole number")))?,
        Type::Flag => match text.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(bad(format!("\"{name}\" is true or false"))),
        },
        // A read that takes a secret or a document is a `POST`, whose
        // arguments are in its body: neither is read from an address.
        Type::Secret { .. } | Type::Document { secret: true, .. } => {
            return Err(bad(format!("\"{name}\" is a secret, never given in an address")));
        }
        Type::Document { secret: false, .. } => {
            return Err(bad(format!("\"{name}\" is a document, never given in an address")));
        }
    })
}

/// `text` decoded as a form's field is: `+` a space, `%XX` the byte XX.
/// None when a `%` is not followed by two hex digits, or the bytes are
/// not UTF-8.
fn form_decoded(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let hex = rest.get(..2)?;
                let hex = std::str::from_utf8(hex).ok()?;
                if !hex.bytes().all(|digit| digit.is_ascii_hexdigit()) {
                    return None;
                }
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                rest = &rest[2..];
            }
            _ => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).ok()
}

/// Who asks for an action, as far as may and may not goes.
#[derive(Clone, Debug)]
pub enum Caller<'a> {
    /// A signed-in session of `name`, an administrator or not, unlocked
    /// or not.
    Session { name: &'a str, admin: bool, unlocked: bool },
    /// An API token, by its name, with its scopes.
    Token { name: &'a str, scopes: &'a Scopes },
}

/// Whether the token `name` with `scopes` may read, or with `changes`
/// change, what the area `area` holds.
///
/// # Errors
///
/// ADM-124, naming the scope the token needs.
pub fn token_may(name: &str, scopes: &Scopes, area: &str, changes: bool) -> Result<(), Failure> {
    if scopes.allow(area, changes) {
        return Ok(());
    }
    let needed = Needed { area, changes };
    Err(Failure::new(
        Kind::OutOfScope,
        format!("the API token \"{name}\" may not do this: it needs the scope {needed}"),
    ))
}

/// Whether `caller` may ask for `action`, before anything runs. authd
/// checks again for an elevated command (an unlocked session of an
/// administrator, or a token whose owner is one and whose scopes name the
/// area), and the person's own rights decide in the worker: this check
/// gives the clear refusal first, and is not the only one.
///
/// # Errors
///
/// ADM-124 for a token whose scopes do not name the action's area,
/// ADM-122 for a person who is not an administrator asking for what runs
/// as one, ADM-106 for an administrator's locked session.
pub fn authorize(caller: &Caller<'_>, action: &dyn Registered) -> Result<(), Failure> {
    let elevated = action.runs() == Runs::AsAdministrator;
    match *caller {
        // authd lets a token run a command as an administrator only with
        // its area's full scope, a read's as well (ADR 0518).
        Caller::Token { name, scopes } => {
            token_may(name, scopes, action.area(), action.changes() || elevated)
        }
        Caller::Session { name, admin, unlocked } if elevated && !action.changes() => {
            session_may_read_elevated(name, admin, unlocked, action.area())
        }
        Caller::Session { name, admin, unlocked } if elevated => {
            session_may_elevate(name, admin, unlocked, action.area())
        }
        Caller::Session { .. } => Ok(()),
    }
}

/// Whether the session of `name` may run a read that needs an
/// administrator (ADR 0518): only an administrator's unlocked session.
///
/// # Errors
///
/// ADM-122 for a person who is not an administrator, ADM-106 for an
/// administrator's locked session.
pub fn session_may_read_elevated(
    name: &str,
    admin: bool,
    unlocked: bool,
    area: &str,
) -> Result<(), Failure> {
    if !admin {
        Err(Failure::new(
            Kind::NotAdmin,
            format!(
                "{name}'s account can look at its own part of {area}, and only an administrator at the rest"
            ),
        ))
    } else if !unlocked {
        Err(Failure::new(Kind::Locked, "unlock the console to read this"))
    } else {
        Ok(())
    }
}

/// Whether the session of `name`, an administrator's or not and unlocked
/// or not, may make a change that runs as an administrator to what `area`
/// holds: only an administrator's unlocked session.
///
/// # Errors
///
/// ADM-122 for a person who is not an administrator, ADM-106 for an
/// administrator's locked session.
pub fn session_may_elevate(
    name: &str,
    admin: bool,
    unlocked: bool,
    area: &str,
) -> Result<(), Failure> {
    if !admin {
        Err(Failure::new(
            Kind::NotAdmin,
            format!("{name}'s account can look at {area} but not change it"),
        ))
    } else if !unlocked {
        Err(Failure::new(Kind::Locked, "unlock the console to make this change"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub mod tests;
