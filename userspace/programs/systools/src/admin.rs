// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! admin(8): the web console from a terminal (ADR 0515). It says where the
//! console is and which certificate it serves, tells what the Overview
//! shows of the machine and what is kept for the next sign-in
//! (`admin_about`), lists the actions the API offers, shows an action as
//! the command, the API call and the Ansible task that do it, says or sets
//! whom the console answers, makes, lists and removes the API tokens of
//! the person who runs it, and, for root, puts back a change of the
//! console's journal (`admin_put_back`, ADR 0516).
//!
//! The list and the forms an action shows as are those of the console
//! itself, from `admind::actions`, so that what is printed here is what the
//! API answers. The tokens are the person's own: the requests go to authd
//! as that person, the same as the console's worker sends them for a
//! signed-in session, and authd checks the password and the rules. There
//! is no `admin run`: the command an action runs is the command shown, and
//! a person runs it as it is.
//!
//! `admin reach` is the command of the console's `console.reach` action
//! (ADR 0516). It sets `reach` in admin.toml(5) through [`settings_edit`],
//! so that every comment in the file stays, and has Spark reload admind,
//! through the code `service admind reload` runs, so that the console
//! answers whom the file says at once. It is root's, as the file is. With
//! `--try SECONDS` the change is Spark's trial `console` (ADR 0562): Spark
//! keeps admin.toml as it was and puts it back, and reloads admind, unless
//! `admin keep` comes in time; `admin undo` puts it back now. While the
//! trial runs, a change of admin.toml is refused (ADM-138), since the undo
//! would put back the file as it was before both.
//!
//! When it misbehaves, `admin fingerprint` says whether admind has written
//! its address, `log admind` and `log --audit` say what the console and
//! authd did, and the `admin-actions` boot test covers it.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use accounts::console::Console;
use accounts::request::{Answer, Request};
use serde::Deserialize;
use serde_json::{Map, Value};
use toml_edit::DocumentMut;

use crate::account::{self, Input};
use crate::admin_about;
use crate::admin_errors;
use crate::admin_export;
use crate::admin_put_back;
use crate::admin_settings::{self, Reach, Settings};
use crate::admind::ISSUE;
use crate::admind::actions::{self, REGISTRY, Registered, show};
use crate::admind::errors::Failure;
use crate::admind::scopes::Scopes;
use crate::args::{Arg, Args};
use crate::settings_edit::{self, Changed, Unedited};
use crate::trial::{self, ToUse, Tried};
use crate::util::{self, Fail, fail, usage};
use crate::{net_errors, service};

/// What a mistake in the command line is told.
const USAGE: &str = "usage: admin fingerprint\n       admin about [--json]\n       \
                     admin notices [--json]\n       admin actions [--json]\n       \
                     admin show ACTION [NAME=VALUE | NAME:=JSON]...\n       \
                     admin reach [private | any] [--try SECONDS]\n       \
                     admin keep\n       admin undo\n       \
                     admin settings [--json]\n       \
                     admin sign-in [--second-step if-set|administrators|everyone]\n                     \
                     [--session-hours N] [--idle-minutes N]\n       \
                     admin token add NAME --scope SCOPE... [--days N]\n       \
                     admin token list\n       admin token remove NAME\n       \
                     admin put-back ID\n       admin export DIR";

/// The most of the issue file that is read, as consoled reads it: it holds
/// two short lines.
const ISSUE_MAX: u64 = 1024;

/// What an address in a shown API call is until the console says its own.
const ADDRESS_UNKNOWN: &str = "https://HOST:PORT";

/// How many days a token lasts when `--days` does not say.
const DAYS_DEFAULT: u32 = 90;

/// admin(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(help)) if help == "help" => {
                println!("{USAGE}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => Err(usage(USAGE.into())),
        };
    };
    match subcommand.to_str() {
        Some("fingerprint") => fingerprint(args),
        Some("about") => admin_about::main(args, admin_about::Which::About),
        Some("notices") => admin_about::main(args, admin_about::Which::Notices),
        Some("actions") => list_actions(args),
        Some("show") => show_action(args),
        Some("reach") => reach(args),
        Some(verb @ ("keep" | "undo")) => end(verb, args),
        Some("settings") => admin_settings::settings(args),
        Some("sign-in") => admin_settings::sign_in(args),
        Some("token") => token(args),
        Some("export") => admin_export::main(args),
        Some("put-back") => admin_put_back::main(args),
        // A lab build's long command, for the lab's long action.
        #[cfg(feature = "lab")]
        Some("lab-steps") => crate::admin_lab::main(args),
        _ => Err(usage(format!("no such command: {}\n{USAGE}", subcommand.to_string_lossy()))),
    }
}

/// The text admind wrote in the issue file at `path`: the console's address
/// and its certificate's fingerprint.
fn read_issue(path: &Path) -> Result<String, Fail> {
    let mut text = String::new();
    match File::open(path).and_then(|file| file.take(ISSUE_MAX).read_to_string(&mut text)) {
        Ok(_) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(fail(
            "The web console is not running: start it with elevate service admind start.".into(),
        )),
        Err(err) => Err(util::failed(&path.display(), err)),
    }
}

/// `admin fingerprint`.
fn fingerprint(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    print!("{}", read_issue(Path::new(ISSUE))?);
    Ok(0)
}

/// `admin actions [--json]`.
fn list_actions(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    if json {
        println!("{}", actions::listed(REGISTRY));
    } else {
        print!("{}", actions_table(REGISTRY));
    }
    Ok(0)
}

/// `actions` as a table: a line each, with its id, whether it reads or
/// changes, who runs it, its method and address, and "e.g." and the command
/// of its first good example.
fn actions_table(actions: &[&'static dyn Registered]) -> String {
    let mut rows = vec![["ID", "KIND", "RUNS AS", "ROUTE", ""].map(String::from).to_vec()];
    for &action in actions {
        let method = if action.changes() { "POST" } else { "GET" };
        let runs = match action.runs() {
            actions::Runs::AsThePerson => "the person",
            actions::Runs::AsAdministrator => "administrator",
        };
        rows.push(vec![
            action.id().to_string(),
            if action.changes() { "change" } else { "read" }.to_string(),
            runs.to_string(),
            format!("{method} {}", action.path()),
            example_command(action),
        ]);
    }
    table(&rows)
}

/// `rows` as lines whose columns line up: each cell but the last of a row
/// is as wide as the widest of its column, and two spaces follow it.
fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0))
        .collect();
    let mut text = String::new();
    for row in rows {
        let line: Vec<String> =
            row.iter().zip(&widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
        text.push_str(line.join("  ").trim_end());
        text.push('\n');
    }
    text
}

/// "e.g." and the command of the first good example of `action`, or
/// nothing for an action with no example that works.
fn example_command(action: &'static dyn Registered) -> String {
    let (good, _, _) = action.example_values();
    let command = good
        .into_iter()
        .next()
        .and_then(|args| action.prepare(args).ok())
        .map(|prepared| show::command(&prepared));
    command.map_or_else(String::new, |command| format!("e.g. {command}"))
}

/// One argument of `admin show`: `NAME=VALUE` gives a string, and
/// `NAME:=JSON` gives the JSON value.
fn argument(text: &str) -> Result<(String, Value), Fail> {
    let not_one = || usage(format!("{text} is not NAME=VALUE or NAME:=JSON"));
    let (name, value) = text.split_once('=').ok_or_else(not_one)?;
    let (name, is_json) = name.strip_suffix(':').map_or((name, false), |name| (name, true));
    if name.is_empty() {
        return Err(not_one());
    }
    if !is_json {
        return Ok((name.to_string(), Value::String(value.to_string())));
    }
    let json = serde_json::from_str(value)
        .map_err(|err| usage(format!("{name}:= takes a JSON value, and {err}")))?;
    Ok((name.to_string(), json))
}

/// The arguments the operands after the action give, each once.
fn arguments(operands: &[String]) -> Result<Map<String, Value>, Fail> {
    let mut given = Map::new();
    for text in operands {
        let (name, value) = argument(text)?;
        if given.insert(name.clone(), value).is_some() {
            return Err(usage(format!("{name} is given twice")));
        }
    }
    Ok(given)
}

/// `admin show ACTION [NAME=VALUE | NAME:=JSON]...`.
fn show_action(args: &mut Args) -> Result<i32, Fail> {
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some((id, given)) = operands.split_first() else {
        return Err(usage("show takes the name of an action: admin actions lists them".into()));
    };
    let origin = read_issue(Path::new(ISSUE))
        .ok()
        .and_then(|text| text.lines().next().map(str::to_string))
        .filter(|address| address.starts_with("https://"))
        .unwrap_or_else(|| ADDRESS_UNKNOWN.to_string());
    print!("{}", show_for(id, given, &origin)?);
    Ok(0)
}

/// What `admin show` prints for the action `id` with the arguments
/// `given`, as the console at `origin` would show it.
fn show_for(id: &str, given: &[String], origin: &str) -> Result<String, Fail> {
    let refused = |failure: Failure| fail(format!("{} ({})", failure.detail, failure.kind.id()));
    let action = show::lookup(REGISTRY, id).map_err(refused)?;
    // A secret is not needed, and one given is not kept (ADR 0561).
    let prepared = show::prepare(action, Value::Object(arguments(given)?)).map_err(refused)?;
    Ok(shown_text(&show::shown(&prepared, origin)))
}

/// What `shown` (the answer of `POST /api/1/show`) says, as text with a
/// heading for each part. What is typed when the command asks follows the
/// command, on a line of its own.
fn shown_text(shown: &Value) -> String {
    let part = |key: &str| shown[key].as_str().unwrap_or_default().trim_end();
    let files: Vec<&str> =
        shown["files"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let files = if files.is_empty() { "none".to_string() } else { files.join("\n") };
    let typed = shown["typed"].as_str().map_or_default(|typed| format!("\n{typed}"));
    format!(
        "Command\n{}{typed}\n\nAPI\n{}\n\nAnsible\n{}\n\nFiles\n{files}\n",
        part("command"),
        part("api"),
        part("ansible"),
    )
}

/// The mode of admin.toml when it is made, as the file that ships has it:
/// every account reads it, and root writes it.
const SETTINGS_MODE: u32 = 0o644;

/// What `admin reach WORD` found to do.
#[derive(Debug, PartialEq, Eq)]
enum Reached {
    /// admin.toml said it already.
    Already,
    /// admin.toml has been changed.
    Changed,
}

/// Fails unless `user` is root, whom the settings file and Spark's control
/// socket are for.
fn require_root(user: u32) -> Result<(), Fail> {
    if user == 0 {
        return Ok(());
    }
    Err(fail(
        "only root may change the console's reach: run it with elevate, as `elevate admin reach ...`".into(),
    ))
}

/// The trial that tries a change of admin.toml (C5 9.2).
const TRIAL: &str = "console";

/// The text of the admin.toml at `path` with `reach` set, every comment of
/// the file kept: none when the file says it already. A file that admind
/// would refuse is left as it is, and what is wrong with it told, for the
/// person to mend first.
fn reach_text(path: &Path, reach: Reach) -> Result<Option<String>, Fail> {
    if Settings::read_at(path).map_err(fail)?.console.reach == reach {
        return Ok(None);
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(fail(Changed::<String>::Read { path: path.into(), error }.to_string()));
        }
    };
    let edit = |document: &mut DocumentMut| {
        let console = settings_edit::table(document, "console")
            .ok_or_else(|| format!("{}: console is not a table", admin_settings::PATH))?;
        settings_edit::set_string(console, "reach", reach.word());
        Ok(())
    };
    settings_edit::edited(&text, edit).map_err(|unedited| match unedited {
        // The edit's words name the file already.
        Unedited::Edit(error) => fail(error),
        Unedited::Parse { line, message } => {
            fail(Changed::<String>::Parse { path: path.into(), line, message }.to_string())
        }
    })
}

/// Makes `reach` what the admin.toml at `path` says, as [`reach_text`]
/// gives it, and nothing written when it says it already.
fn set_reach(path: &Path, reach: Reach) -> Result<Reached, Fail> {
    let Some(text) = reach_text(path, reach)? else { return Ok(Reached::Already) };
    match settings_edit::write(path, text.as_bytes(), SETTINGS_MODE) {
        Ok(true) => Ok(Reached::Changed),
        Ok(false) => Ok(Reached::Already),
        Err(error) => Err(fail(Changed::<String>::Write { path: path.into(), error }.to_string())),
    }
}

/// ADM-138: a change of admin.toml, the console's reach or its sign-in
/// rules, while the trial `console` runs, since Spark's undo would put
/// back the file as it was before both. Spark that does not answer runs
/// no trial: only a change that is tried (`seconds`) needs it.
///
/// # Errors
///
/// ADM-138, or Spark's silence for a change that is tried.
pub(crate) fn console_not_tried(seconds: Option<u32>) -> Result<(), Fail> {
    match trial::running(TRIAL) {
        Ok(Some(_)) => Err(admin_errors::console_is_tried()),
        Ok(None) => Ok(()),
        Err(_) if seconds.is_none() => Ok(()),
        Err(refused) => Err(refused),
    }
}

/// `admin reach [private | any] [--try SECONDS]`: says whom the console
/// answers, or makes it answer that, as root, at once or as the trial
/// `console`.
fn reach(args: &mut Args) -> Result<i32, Fail> {
    let (mut word, mut seconds) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if word.is_none() => {
                word = Some(operand.to_string_lossy().into_owned());
            }
            Arg::Long(option) if option == "try" && seconds.is_none() => {
                seconds = Some(trial::seconds(&args.text()?)?);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let path = Path::new(admin_settings::PATH);
    let Some(word) = word else {
        if seconds.is_some() {
            return Err(usage(format!("--try goes with a reach: private or any\n{USAGE}")));
        }
        println!("{}", Settings::read_at(path).map_err(fail)?.console.reach.word());
        return Ok(0);
    };
    let wanted = Reach::parse(&word)
        .ok_or_else(|| usage(format!("{word} is not a reach: private or any")))?;
    require_root(util::own_user())?;
    console_not_tried(seconds)?;
    if let Some(seconds) = seconds {
        return try_reach(path, wanted, seconds);
    }
    match set_reach(path, wanted)? {
        Reached::Already => println!("The console's reach is already {word}."),
        Reached::Changed => {
            // Spark signals admind, which reads the file again; it would
            // within 30 seconds anyway, and when it starts.
            service::ask("reload admind").map_err(|why| {
                fail(format!(
                    "{} says reach = \"{word}\" now, but admind was not told to read it again \
                     ({}): it does within 30 seconds, and when it starts",
                    admin_settings::PATH,
                    why.message
                ))
            })?;
            println!("The console's reach is now {word}: admind has read it again.");
        }
    }
    Ok(0)
}

/// `admin reach WORD --try SECONDS`: the new reach as the trial `console`,
/// which Spark's reload of admind puts to use; admind's reload marks the
/// moment, so that the page's check that its browser still reaches the
/// console counts only a connection made after it (C5 9.2). A reload that
/// fails is undone at once, and says so, with 1.
fn try_reach(path: &Path, reach: Reach, seconds: u32) -> Result<i32, Fail> {
    let word = reach.word();
    let Some(text) = reach_text(path, reach)? else {
        println!("The console's reach is already {word}: nothing is tried.");
        return Ok(0);
    };
    let to_use = ToUse::Reload("admind");
    match trial::try_text(TRIAL, path, text.as_bytes(), SETTINGS_MODE, seconds, to_use)? {
        Tried::InUse => {
            println!(
                "The console's reach is now {word}: Spark undoes it at {} unless it is kept, \
                 with `admin keep`.",
                trial::undone_at(seconds)
            );
            Ok(0)
        }
        Tried::Undone { why, undone } => {
            eprintln!("admin: {why}");
            println!("{undone}");
            Ok(1)
        }
    }
}

/// `admin keep` and `admin undo`: what the trial `console` comes to, as
/// root, whom Spark's control socket is for.
fn end(verb: &str, args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root keeps or undoes a change of the console's reach: run it with elevate, as \
             `elevate admin {verb}`"
        )));
    }
    let words = trial::end(verb, TRIAL, || net_errors::nothing_is_tried("the console's settings"))?;
    match verb {
        "keep" => println!("Kept: the console's reach stays as it is."),
        _ => println!("{words}"),
    }
    Ok(0)
}

/// `admin token ...`.
fn token(args: &mut Args) -> Result<i32, Fail> {
    let which = "which? add, list or remove";
    let Some(subcommand) = args.subcommand() else {
        return Err(usage(format!("{which}\n{USAGE}")));
    };
    match subcommand.to_str() {
        Some("add") => token_add(args),
        Some("list") => token_list(args),
        Some("remove") => token_remove(args),
        _ => Err(usage(format!("{which}\n{USAGE}"))),
    }
}

/// Asks authd `console` as the person who runs this, and gives its answer.
/// Whatever secret `console` holds is wiped once it has been sent.
fn ask(console: Console) -> Result<Answer, Fail> {
    let mut request = Request::Console(console);
    let answer = account::ask(&request);
    request.wipe();
    answer
}

/// The text of an answer of authd's, or its refusal in its own words.
fn told(answer: Answer) -> Result<String, Fail> {
    match answer {
        Answer::Done(text) => Ok(text),
        Answer::Refused(why) => Err(fail(why)),
    }
}

/// The scopes `values` give, each a scope or a comma-separated list of
/// them, one by one.
fn scopes_of(values: &[String]) -> Result<Vec<String>, Fail> {
    let mut scopes = Vec::new();
    for value in values {
        if Scopes::parse(value).is_none() {
            return Err(usage(format!(
                "{value} is not a scope: * is all that you may do, the name of an area is that \
                 area, and the name with :read after it is reading it alone"
            )));
        }
        scopes.extend(value.split(',').map(str::to_string));
    }
    Ok(scopes)
}

/// `admin token add NAME --scope SCOPE... [--days N]`.
fn token_add(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut scopes, mut days) = (None, Vec::new(), DAYS_DEFAULT);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "scope" => scopes.push(args.text()?),
            Arg::Long(option) if option == "days" => days = args.number()?,
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name else {
        return Err(usage("add takes the token's name and its scopes".into()));
    };
    if scopes.is_empty() {
        return Err(usage("add takes at least one --scope: * or the name of an area".into()));
    }
    let scopes = scopes_of(&scopes)?;
    let proof = Input::new().password("Password: ")?;
    let text = told(ask(Console::TokenAdd { proof, name, scopes, days })?)?;
    let mut secret = text.into_bytes();
    println!("{}", String::from_utf8_lossy(&secret));
    accounts::wipe(&mut secret);
    eprintln!("This token is not shown again: keep it now.");
    Ok(0)
}

/// A token as `steps` lists it.
#[derive(Debug, Deserialize)]
struct Listed {
    name: String,
    scopes: Vec<String>,
    until: i64,
    expired: bool,
}

/// The tokens authd's `steps` answer `text` lists.
fn tokens_in(text: &str) -> Result<Vec<Listed>, Fail> {
    #[derive(Deserialize)]
    struct Steps {
        tokens: Vec<Listed>,
    }
    let steps: Steps = serde_json::from_str(text)
        .map_err(|_| fail("authd answered a list of tokens that does not read".into()))?;
    Ok(steps.tokens)
}

/// `tokens` as a table: a line each, with the token's name, its scopes and
/// the day it ends, which says so when it has.
fn tokens_table(tokens: &[Listed]) -> String {
    let mut rows = vec![["NAME", "SCOPES", "ENDS"].map(String::from).to_vec()];
    for token in tokens {
        let day = util::local_time(token.until, "%Y-%m-%d");
        let ends = if token.expired { format!("{day} (ended)") } else { day };
        rows.push(vec![token.name.clone(), token.scopes.join(", "), ends]);
    }
    table(&rows)
}

/// `admin token list`.
fn token_list(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let tokens = tokens_in(&told(ask(Console::Steps)?)?)?;
    if tokens.is_empty() {
        println!("You have no API tokens.");
    } else {
        print!("{}", tokens_table(&tokens));
    }
    Ok(0)
}

/// `admin token remove NAME`.
fn token_remove(args: &mut Args) -> Result<i32, Fail> {
    let mut name = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name else {
        return Err(usage("remove takes the name of a token".into()));
    };
    told(ask(Console::TokenRemove { name: name.clone() })?)?;
    println!("The API token {name} is removed.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use serde_json::json;

    use super::*;
    use crate::admind::actions::tests::LAB;

    fn args(items: &[&str]) -> Args {
        Args::new("admin", items.iter().map(OsString::from).collect())
    }

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    /// Noon on 2027-01-15 in UTC, which is that day in every time zone
    /// within eleven hours of it.
    const NOON: i64 = 1_800_014_400;

    #[test]
    fn name_equals_value_is_a_string_and_name_colon_equals_json_is_a_value() {
        let one = |text: &str| argument(text).unwrap();
        assert_eq!(one("name=nginx"), ("name".into(), json!("nginx")));
        assert_eq!(one("wait:=5"), ("wait".into(), json!(5)));
        assert_eq!(one("wait=5"), ("wait".into(), json!("5")));
        assert_eq!(one("force:=true"), ("force".into(), json!(true)));
        assert_eq!(one("names:=[\"a\",\"b\"]"), ("names".into(), json!(["a", "b"])));
        assert_eq!(one("note:=\"x y\""), ("note".into(), json!("x y")));
        assert_eq!(one("nothing:=null"), ("nothing".into(), json!(null)));
        assert_eq!(one("path=a=b"), ("path".into(), json!("a=b")));
        assert_eq!(one("empty="), ("empty".into(), json!("")));
        assert_eq!(one("text=:=1"), ("text".into(), json!(":=1")));
        assert_eq!(one("a:b=c"), ("a:b".into(), json!("c")));
    }

    #[test]
    fn an_argument_that_is_neither_form_or_not_json_is_a_mistake_in_the_command_line() {
        for text in ["name", "", "=x", ":=1", ":="] {
            let fail = argument(text).unwrap_err();
            assert!(fail.usage, "{text:?}");
            assert_eq!(fail.message, format!("{text} is not NAME=VALUE or NAME:=JSON"));
        }
        let fail = argument("wait:=five").unwrap_err();
        assert!(fail.usage);
        assert!(fail.message.starts_with("wait:= takes a JSON value, and "), "{}", fail.message);
        assert!(argument("wait:=").is_err());
    }

    #[test]
    fn each_argument_is_given_once() {
        let given = arguments(&words(&["name=nginx", "wait:=5"])).unwrap();
        assert_eq!(Value::Object(given), json!({ "name": "nginx", "wait": 5 }));
        assert!(arguments(&[]).unwrap().is_empty());
        let twice = arguments(&words(&["name=nginx", "name:=1"])).unwrap_err();
        assert_eq!(twice.message, "name is given twice");
    }

    #[test]
    fn the_actions_are_a_table_of_their_route_and_a_command_that_works() {
        assert_eq!(
            actions_table(LAB),
            "ID           KIND    RUNS AS        ROUTE\n\
             lab.restart  change  administrator  POST /api/1/lab/{name}/restart  e.g. elevate service lab-web restart\n\
             lab.list     read    the person     GET /api/1/lab                  e.g. service list --json\n\
             lab.long     change  administrator  POST /api/1/lab/long            e.g. elevate admin lab-steps 20\n\
             lab.set      change  administrator  POST /api/1/lab/{name}/set      e.g. elevate service lab-web set\n\
             lab.missing  read    the person     GET /api/1/lab/missing          e.g. lab-missing --json\n\
             lab.log      read    administrator  GET /api/1/lab/log              e.g. elevate lab-log --output json --lines 5\n"
        );
    }

    #[test]
    fn every_action_of_the_console_is_in_the_table_with_an_example() {
        let table = actions_table(REGISTRY);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), REGISTRY.len() + 1);
        for (line, action) in lines[1..].iter().zip(REGISTRY) {
            assert!(line.starts_with(action.id()), "{line}");
            assert!(line.contains("  e.g. "), "{line}");
        }
        let line = |id: &str| lines.iter().find(|line| line.starts_with(id)).unwrap();
        assert!(line("service.restart").contains("  POST /api/1/services/{name}/restart  "));
        assert!(line("service.restart").ends_with("  e.g. elevate service diskhealth restart"));
        assert!(line("service.list").contains("  GET /api/1/services  "));
        assert!(line("service.list").ends_with("  e.g. service list --json"));
        assert!(line("certificate.renew").ends_with("  e.g. elevate cert renew this-machine"));
        assert!(!table.lines().any(|line| line.ends_with(' ')));
    }

    #[test]
    fn what_a_showing_prints_has_a_heading_for_each_part() {
        let text =
            show_for("service.restart", &words(&["name=diskhealth"]), "https://nas.example:9090")
                .unwrap();
        assert_eq!(
            text,
            "Command\n\
             elevate service diskhealth restart\n\n\
             API\n\
             curl -sS -X POST -H \"Authorization: Bearer $DUKOVO_TOKEN\" -H \"Content-Type: application/json\" -d '{}' https://nas.example:9090/api/1/services/diskhealth/restart\n\n\
             Ansible\n\
             - name: \"Restart a service: diskhealth\"\n  dukovo.system.service:\n    name: \"diskhealth\"\n    state: \"restarted\"\n  become: true\n\n\
             Files\n\
             none\n"
        );
    }

    #[test]
    fn files_are_listed_a_line_each() {
        let shown = json!({ "command": "c", "api": "a", "ansible": "y\n", "files": ["/a", "/b"] });
        assert_eq!(shown_text(&shown), "Command\nc\n\nAPI\na\n\nAnsible\ny\n\nFiles\n/a\n/b\n");
    }

    #[test]
    fn a_showing_is_refused_in_the_words_of_the_action_s_route_and_its_id() {
        let origin = "https://nas.example:9090";
        let refused = |id: &str, given: &[&str]| show_for(id, &words(given), origin).unwrap_err();
        assert_eq!(
            refused("service.explode", &[]).message,
            "There is no action service.explode: admin actions lists them. Nothing was done. (ADM-125)"
        );
        assert_eq!(
            refused("service.restart", &["name=admind"]).message,
            "The console runs on admind: stop or restart it at a terminal, with elevate service \
             admind restart. Nothing was done. (ADM-125)"
        );
        assert!(
            refused("service.restart", &["name=x", "force:=true"]).message.ends_with(" (ADM-108)")
        );
        assert!(refused("service.restart", &[]).message.ends_with(" (ADM-108)"));
        assert!(refused("service.restart", &["name"]).usage);
    }

    #[test]
    fn a_scope_is_checked_as_the_console_checks_one() {
        let ok = |values: &[&str]| scopes_of(&words(values)).unwrap();
        assert_eq!(ok(&["services:read"]), ["services:read"]);
        assert_eq!(ok(&["*"]), ["*"]);
        assert_eq!(
            ok(&["services:read,firewall", "storage"]),
            ["services:read", "firewall", "storage"]
        );
        for bad in ["", "Services", "services:write", "a,,b", "a b", "services,", "-x", "**"] {
            let fail = scopes_of(&words(&["services", bad])).unwrap_err();
            assert!(fail.usage, "{bad:?}");
            assert_eq!(
                fail.message,
                format!(
                    "{bad} is not a scope: * is all that you may do, the name of an area is that \
                     area, and the name with :read after it is reading it alone"
                )
            );
        }
    }

    #[test]
    fn the_tokens_are_read_from_the_steps_authd_lists_without_what_else_they_hold() {
        let text = json!({
            "app": { "added": 1 },
            "passkeys": [],
            "recovery": null,
            "trusted": 0,
            "tokens": [
                { "name": "ansible", "scopes": ["services:read", "firewall"], "until": NOON, "made": 1, "used": 0, "expired": false },
                { "name": "old", "scopes": ["*"], "until": NOON, "made": 1, "used": 0, "expired": true },
            ],
            "needed": false,
        })
        .to_string();
        let tokens = tokens_in(&text).unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(
            tokens_table(&tokens),
            "NAME     SCOPES                   ENDS\n\
             ansible  services:read, firewall  2027-01-15\n\
             old      *                        2027-01-15 (ended)\n"
        );
        assert_eq!(tokens_in(r#"{ "tokens": [] }"#).unwrap().len(), 0);
        for bad in ["", "not json", "{}", r#"{ "tokens": [{ "name": "x" }] }"#] {
            assert_eq!(
                tokens_in(bad).unwrap_err().message,
                "authd answered a list of tokens that does not read"
            );
        }
    }

    #[test]
    fn authd_s_refusal_is_told_as_it_words_it() {
        assert_eq!(told(Answer::Done("dkv_x".into())).unwrap(), "dkv_x");
        let why = "alice has a token named ci: remove it, or choose another name";
        let fail = told(Answer::Refused(why.into())).unwrap_err();
        assert_eq!((fail.code, fail.message.as_str(), fail.usage), (1, why, false));
    }

    #[test]
    fn the_issue_file_is_what_admind_wrote_and_its_absence_says_how_to_start_the_console() {
        let dir = std::env::temp_dir().join(format!("admin-issue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("issue");
        let fail = read_issue(&path).unwrap_err();
        assert_eq!(
            (fail.code, fail.message.as_str()),
            (1, "The web console is not running: start it with elevate service admind start.")
        );
        let text = "https://duke-nas.local:9090\nSHA-256 fingerprint AB:CD\n";
        std::fs::write(&path, text).unwrap();
        assert_eq!(read_issue(&path).unwrap(), text);
        // A file that is not admind's is read only as far as the limit.
        std::fs::write(&path, "x".repeat(5000)).unwrap();
        assert_eq!(read_issue(&path).unwrap().len(), 1024);
        // A folder is no such file, and says what it is.
        let fail = read_issue(&dir).unwrap_err();
        assert!(fail.message.starts_with(&dir.display().to_string()), "{}", fail.message);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mistake_in_the_command_line_is_told_with_the_usage() {
        let fail = main(&mut args(&[])).unwrap_err();
        assert!(fail.usage && fail.message.starts_with("usage: admin fingerprint"));
        let fail = main(&mut args(&["run"])).unwrap_err();
        assert!(fail.usage && fail.message.starts_with("no such command: run\nusage: admin"));
        assert!(main(&mut args(&["--oops"])).unwrap_err().usage);
        assert!(
            main(&mut args(&["token"]))
                .unwrap_err()
                .message
                .starts_with("which? add, list or remove")
        );
        assert!(main(&mut args(&["token", "show"])).unwrap_err().message.starts_with("which? add"));
        assert!(main(&mut args(&["show"])).unwrap_err().usage);
        assert!(main(&mut args(&["actions", "--yaml"])).unwrap_err().usage);
        assert!(main(&mut args(&["fingerprint", "now"])).unwrap_err().usage);
        assert!(main(&mut args(&["token", "list", "now"])).unwrap_err().usage);
        assert!(main(&mut args(&["token", "remove"])).unwrap_err().usage);
        assert!(main(&mut args(&["token", "remove", "a", "b"])).unwrap_err().usage);
        assert_eq!(main(&mut args(&["--help"])).unwrap(), 0);
    }

    /// admin.toml as it ships: every setting in it is a comment.
    const SHIPPED: &str = include_str!("../../../defaults/admin.toml");

    /// A folder of its own under the system's temporary folder, taken away
    /// at the end, and the settings file in it.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let dir =
                std::env::temp_dir().join(format!("admin-reach-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Folder(dir)
        }

        fn file(&self) -> std::path::PathBuf {
            self.0.join("admin.toml")
        }

        fn text(&self) -> String {
            std::fs::read_to_string(self.file()).unwrap()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_reach_is_set_in_the_file_that_ships_with_its_comments_kept_above_the_table() {
        let folder = Folder::new("shipped");
        std::fs::write(folder.file(), SHIPPED).unwrap();
        assert_eq!(set_reach(&folder.file(), Reach::Any).unwrap(), Reached::Changed);
        assert_eq!(folder.text(), format!("{SHIPPED}\n[console]\nreach = \"any\"\n"));
        assert_eq!(Settings::read_at(&folder.file()).unwrap().console.reach, Reach::Any);
        // And back, in the same place and with the same comments.
        assert_eq!(set_reach(&folder.file(), Reach::Private).unwrap(), Reached::Changed);
        assert_eq!(folder.text(), format!("{SHIPPED}\n[console]\nreach = \"private\"\n"));
    }

    #[test]
    fn a_reach_that_is_set_changes_its_value_and_no_other_line_of_the_file() {
        let folder = Folder::new("kept");
        let before = "# Mine.\n\n[sign-in]\nsession-hours = 8 # a working day\n\n[console]\n\
                      listen = 9090\n# Whom.\nreach = \"private\" # as shipped\ncertificate = \"this-machine\"\n";
        std::fs::write(folder.file(), before).unwrap();
        assert_eq!(set_reach(&folder.file(), Reach::Any).unwrap(), Reached::Changed);
        assert_eq!(folder.text(), before.replace("\"private\" # as", "\"any\" # as"));
        // A key that is not there is added to its table.
        std::fs::write(folder.file(), "[console]\nlisten = 9090\n\n[sign-in]\nidle-minutes = 60\n")
            .unwrap();
        assert_eq!(set_reach(&folder.file(), Reach::Any).unwrap(), Reached::Changed);
        assert_eq!(
            folder.text(),
            "[console]\nlisten = 9090\nreach = \"any\"\n\n[sign-in]\nidle-minutes = 60\n"
        );
    }

    #[test]
    fn a_reach_that_is_set_already_writes_nothing_and_a_file_that_is_not_there_is_made_when_it_must()
     {
        use std::os::unix::fs::MetadataExt;

        let folder = Folder::new("already");
        // The default is what a file that is not there says: nothing is made.
        assert_eq!(set_reach(&folder.file(), Reach::Private).unwrap(), Reached::Already);
        assert!(!folder.file().exists());
        std::fs::write(folder.file(), "[console]\nreach = 'any' # single quotes\n").unwrap();
        let ino = std::fs::metadata(folder.file()).unwrap().ino();
        assert_eq!(set_reach(&folder.file(), Reach::Any).unwrap(), Reached::Already);
        assert_eq!(folder.text(), "[console]\nreach = 'any' # single quotes\n");
        assert_eq!(std::fs::metadata(folder.file()).unwrap().ino(), ino);
        // A file that is not there is made for a reach it does not give.
        std::fs::remove_file(folder.file()).unwrap();
        assert_eq!(set_reach(&folder.file(), Reach::Any).unwrap(), Reached::Changed);
        assert_eq!(folder.text(), "[console]\nreach = \"any\"\n");
        assert_eq!(std::fs::metadata(folder.file()).unwrap().mode() & 0o7777, 0o644);
    }

    #[test]
    fn a_file_admind_would_refuse_or_that_is_no_toml_is_left_as_it_is_and_said() {
        let folder = Folder::new("refused");
        for (text, said) in [
            ("[sign-in]\nsession-hours = 500\n", "session-hours is 1 to 168"),
            ("[console]\nlisten = 80\n", "listen is 443, or 1024 to 65535"),
            ("[console]\nreech = \"any\"\n", "unknown field `reech`"),
            ("[console]\nreach = \"everyone\"\n", "unknown variant `everyone`"),
            ("console = 5\n", "invalid type: integer `5`"),
            ("[console\n", "unclosed table"),
        ] {
            std::fs::write(folder.file(), text).unwrap();
            let fail = set_reach(&folder.file(), Reach::Any).unwrap_err();
            assert!(!fail.usage);
            assert!(fail.message.starts_with("/system/settings/admin.toml: "), "{}", fail.message);
            assert!(fail.message.contains(said), "{text:?}: {}", fail.message);
            assert_eq!(folder.text(), text);
        }
    }

    #[test]
    fn only_root_changes_the_reach() {
        assert!(require_root(0).is_ok());
        for user in [1, 1000, 65_534] {
            let fail = require_root(user).unwrap_err();
            assert_eq!(
                fail.message,
                "only root may change the console's reach: run it with elevate, as `elevate admin reach ...`"
            );
            assert!(!fail.usage);
        }
    }

    #[test]
    fn a_reach_that_is_none_or_a_second_word_is_a_mistake_in_the_command_line() {
        let fail = main(&mut args(&["reach", "everyone"])).unwrap_err();
        assert!(fail.usage);
        assert_eq!(fail.message, "everyone is not a reach: private or any");
        for items in [&["reach", "Any"][..], &["reach", "private", "any"], &["reach", "--now"]] {
            assert!(main(&mut args(items)).unwrap_err().usage, "{items:?}");
        }
        // A trial needs a reach, and Spark's 10 seconds to an hour.
        let fail = main(&mut args(&["reach", "--try", "60"])).unwrap_err();
        assert!(fail.usage && fail.message.starts_with("--try goes with a reach"), "{fail:?}");
        for items in [
            &["reach", "any", "--try", "5"][..],
            &["reach", "any", "--try=3601"],
            &["reach", "any", "--try"],
            &["reach", "any", "--try", "60", "--try", "60"],
            &["keep", "now"],
            &["undo", "--all"],
        ] {
            assert!(main(&mut args(items)).unwrap_err().usage, "{items:?}");
        }
    }

    #[test]
    fn a_tried_reach_is_the_text_a_reach_set_at_once_writes() {
        let folder = Folder::new("tried");
        std::fs::write(folder.file(), SHIPPED).unwrap();
        let tried = reach_text(&folder.file(), Reach::Any).unwrap().unwrap();
        // Nothing is written until the trial begins.
        assert_eq!(folder.text(), SHIPPED);
        set_reach(&folder.file(), Reach::Any).unwrap();
        assert_eq!(folder.text(), tried);
        assert_eq!(reach_text(&folder.file(), Reach::Any).unwrap(), None);
    }

    #[test]
    fn a_token_is_made_with_a_name_and_a_scope_before_authd_is_asked_or_the_password() {
        let add = |items: &[&str]| token_add(&mut args(items)).unwrap_err();
        assert!(add(&[]).message.starts_with("add takes the token's name"));
        assert!(add(&["ci"]).message.starts_with("add takes at least one --scope"));
        assert!(add(&["ci", "--scope", "services:write"]).message.contains("is not a scope"));
        assert!(add(&["ci", "--scope"]).usage);
        assert!(
            add(&["ci", "--scope", "*", "--days", "soon"]).message.contains("not a valid number")
        );
        assert!(add(&["ci", "other", "--scope", "*"]).usage);
    }
}
