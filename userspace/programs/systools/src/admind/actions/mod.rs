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
//! [`REGISTRY`] is the one list. The route table, `GET /api/1/actions`,
//! `admin actions` and `lab/actions.toml` are all made from it.

mod certificate;
mod console;
#[cfg(any(test, feature = "lab"))]
pub mod lab;
mod service;
pub mod show;

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

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

/// An action's arguments: JSON in, checked before anything runs.
pub trait Args: DeserializeOwned + Serialize {
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
    fn check(&self) -> Result<(), Bad> {
        Ok(())
    }
}

/// An action's Ansible task: a module with its parameters, or
/// `ansible.builtin.command` with the command's words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ansible {
    /// The task's name, as a person reads it in a play's output.
    pub name: String,
    /// The module: `dukovo.system.service`, or `ansible.builtin.command`.
    pub module: &'static str,
    /// The module's parameters, in order.
    pub params: Map<String, Value>,
    /// Whether the task runs as root (`become: true`).
    pub become_root: bool,
    /// `changed_when`, when the module does not say: `false` for a read.
    pub changed_when: Option<String>,
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
        }
    }
}

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
    /// Its five tests.
    const TESTS: Tests;
    /// What a person reads as its name in the palette and in the journal:
    /// "Restart a service".
    const TITLE: &'static str;

    /// What it takes.
    type Args: Args;

    /// The command, as words: the program first. A read's has `--json`.
    fn argv(args: &Self::Args) -> Vec<String>;

    /// What the action is done to, which the name of its Ansible task adds
    /// to the title: the service's name. None for an action that takes
    /// none.
    fn subject(_args: &Self::Args) -> Option<String> {
        None
    }

    /// The Ansible task that does the same.
    fn ansible(args: &Self::Args) -> Ansible {
        let name = match Self::subject(args) {
            Some(subject) => format!("{}: {subject}", Self::TITLE),
            None => Self::TITLE.to_string(),
        };
        let elevated = Self::RUNS == Runs::AsAdministrator;
        Ansible::command(name, &Self::argv(args), elevated, Self::CHANGES)
    }

    /// The files a change writes, for the Files tab and the journal.
    fn files(_args: &Self::Args) -> Vec<String> {
        Vec::new()
    }

    /// Arguments for the tests that take every action from the list.
    fn examples() -> Examples<Self::Args>;
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
    fn tests(&self) -> Tests;
    fn title(&self) -> &'static str;
    /// Reads and checks `args` and makes the command and the rest of
    /// them.
    ///
    /// # Errors
    ///
    /// ADM-108 when `args` is not what the action takes, ADM-125 when a
    /// value cannot work.
    fn prepare(&'static self, args: Value) -> Result<Prepared, Failure>;
    /// The examples, as JSON.
    fn example_values(&self) -> (Vec<Value>, Vec<Value>, Option<Undo>);
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
    fn tests(&self) -> Tests {
        A::TESTS
    }
    fn title(&self) -> &'static str {
        A::TITLE
    }

    fn prepare(&'static self, args: Value) -> Result<Prepared, Failure> {
        let typed: A::Args = serde_json::from_value(args).map_err(|err| {
            // serde's words name the field and the type, never the value
            // given, which could be a secret in the wrong field.
            Failure::new(
                Kind::BadRequest,
                format!("the arguments are not what {} takes: {err}", A::ID),
            )
        })?;
        typed.check().map_err(|bad| Failure::bad_value(bad.field, bad.why))?;
        let argv = A::argv(&typed);
        let ansible = A::ansible(&typed);
        let files = A::files(&typed);
        let args = serde_json::to_value(&typed)
            .map_err(|err| Failure::new(Kind::Internal, format!("{} arguments: {err}", A::ID)))?;
        Ok(Prepared { action: self, args, argv, ansible, files })
    }

    fn example_values(&self) -> (Vec<Value>, Vec<Value>, Option<Undo>) {
        let examples = A::examples();
        let json = |args: &A::Args| serde_json::to_value(args).unwrap_or(Value::Null);
        (examples.good.iter().map(json).collect(), examples.bad, examples.undo)
    }
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
    service::Start,
    service::Stop,
    service::Restart,
    service::Enable,
    service::Disable,
    certificate::List,
    certificate::Show,
    certificate::Renew,
    console::SetReach,
];

/// The actions the console offers: [`REGISTRY`], and in a lab build the
/// lab's own, which the tests of tasks run ([`lab`]).
pub fn offered() -> &'static [&'static dyn Registered] {
    #[cfg(feature = "lab")]
    {
        static ALL: std::sync::LazyLock<Vec<&'static dyn Registered>> =
            std::sync::LazyLock::new(|| {
                static LONG: Entry<lab::Long> = Entry::new();
                let mut all: Vec<&'static dyn Registered> = REGISTRY.to_vec();
                all.push(&LONG);
                all
            });
        &ALL
    }
    #[cfg(not(feature = "lab"))]
    REGISTRY
}

/// The list `GET /api/1/actions` answers and `admin actions` prints: each
/// action's id, title, area, whether it changes anything, its method and
/// address, who runs it, whether it is a task, its error IDs, the names of
/// its five tests, and its examples: the arguments that work, and for a
/// change the action and arguments that put back what the first did (null
/// when nothing does). The bad examples are in the unit tests alone.
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
                    "method": if action.changes() { "POST" } else { "GET" },
                    "path": action.path(),
                    "runs": action.runs(),
                    "long": action.long(),
                    "errors": action.errors(),
                    "tests": action.tests(),
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
        if action.changes() == post {
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
fn decode(part: &str) -> Option<String> {
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
/// ADM-122 for a person who is not an administrator asking for an
/// administrator's change, ADM-106 for an administrator's locked session.
pub fn authorize(caller: &Caller<'_>, action: &dyn Registered) -> Result<(), Failure> {
    let elevated = action.runs() == Runs::AsAdministrator;
    match *caller {
        Caller::Token { name, scopes } => token_may(name, scopes, action.area(), action.changes()),
        Caller::Session { name, admin, unlocked } => {
            if elevated {
                session_may_elevate(name, admin, unlocked, action.area())
            } else {
                Ok(())
            }
        }
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
