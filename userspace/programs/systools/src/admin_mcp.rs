// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admin mcp` (ADR 0690): the clients that the web console lets in
//! over MCP, from a terminal, and the way in for one that connects over
//! SSH.
//!
//! ```text
//! admin mcp add NAME [--https] --scope SCOPE... [--days N]
//! admin mcp list [--all] [--json]
//! admin mcp remove NAME [--of OWNER]
//! admin mcp NAME [--check]
//! ```
//!
//! `add`, `list` and `remove` are about the clients of the person who
//! runs them, an administrator: the requests go to authd as that person,
//! as `admin token`'s do, and authd checks the password and the rules.
//! `--all` and `--of` are about every account's, for an administrator
//! inside elevate; they are the commands of the console's
//! `console.clients` and `console.client-remove` actions.
//!
//! A client that connects over SSH has no secret to keep: its MCP
//! settings run `ssh OWNER@HOST admin mcp NAME`, and what lets it in is
//! its owner's own SSH sign-in. `admin mcp NAME` asks authd for a pass
//! for the client, gives it to the console's socket, and carries the
//! client's messages and the console's answers ([`bridge`]). `--check`
//! connects the same way, says what the client reaches, and leaves.
//!
//! A client that connects over HTTPS is given a token, shown once, to
//! send to the console's `/mcp`.
//!
//! When it misbehaves, `admin mcp NAME --check` says whether the client
//! connects, `log admind` has a line for each of its calls, `log
//! --audit` what authd gave and refused, and the `admin-mcp` boot test
//! covers it.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use accounts::console::{Console, Way};
use serde::Deserialize;
use serde_json::{Value, json};
use sysapi::fd::Standard;

use crate::account::Input;
use crate::admin::{ask, table, told};
use crate::admind::mcp::areas::AREAS;
use crate::admind::mcp::local;
use crate::args::{Arg, Args};
use crate::authd::console::clients;
use crate::deadline::Deadline;
use crate::util::{self, Fail, fail, usage};

mod bridge;

use bridge::Link;

/// What a mistake in the command line is told.
pub const USAGE: &str = "admin mcp add NAME [--https] --scope SCOPE... [--days N]\n       \
                         admin mcp list [--all] [--json]\n       \
                         admin mcp remove NAME [--of OWNER]\n       \
                         admin mcp NAME [--check]";

/// How many days a client lasts when `--days` does not say.
const DAYS_DEFAULT: u32 = 90;
/// How long the console has to greet a connection.
const GREET_TIME: Duration = Duration::from_secs(10);
/// How long the console has to answer `--check`.
const CHECK_TIME: Duration = Duration::from_secs(30);
/// The longest line read of the console's greeting, and of its answer to
/// `--check`.
const GREETING_MAX: usize = 1024;
const CHECK_MAX: usize = 1024 * 1024;

/// `admin mcp ...`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(first) = args.subcommand() else {
        return Err(usage(format!("which? add, list, remove, or a client's name\nusage: {USAGE}")));
    };
    match first.to_str() {
        Some("add") => add(args),
        Some("list") => list(args),
        Some("remove") => remove(args),
        Some(name) => client(name, args),
        None => Err(usage("an AI client's name is 1 to 32 of a to z, 0 to 9 and -".into())),
    }
}

/// The scopes `values` give, each a scope or a comma-separated list of
/// them, one by one.
fn scopes_of(values: &[String]) -> Result<Vec<String>, Fail> {
    let mut scopes = Vec::new();
    for scope in values.iter().flat_map(|value| value.split(',')) {
        let area = scope.strip_suffix(":read").unwrap_or(scope);
        if !clients::scope_ok(scope) {
            return Err(usage(format!(
                "{scope} is not a scope: the name of an area is all its reads and asking for \
                 its changes, the name with :read after it is the reads you have before you \
                 unlock, and * in place of the name is every area"
            )));
        }
        if area != "*" && !AREAS.iter().any(|(id, _)| *id == area) {
            let areas: Vec<&str> = AREAS.iter().map(|(id, _)| *id).collect();
            return Err(usage(format!(
                "{scope} is not a scope: there is no area {area}. The areas are {}.",
                areas.join(", ")
            )));
        }
        scopes.push(scope.to_string());
    }
    Ok(scopes)
}

/// `admin mcp add NAME [--https] --scope SCOPE... [--days N]`.
fn add(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut way, mut scopes, mut days) = (None, Way::Ssh, Vec::new(), DAYS_DEFAULT);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "https" => way = Way::Https,
            Arg::Long(option) if option == "scope" => scopes.push(args.text()?),
            Arg::Long(option) if option == "days" => days = args.number()?,
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name else {
        return Err(usage("add takes the client's name and its scopes".into()));
    };
    if !clients::name_ok(&name) {
        return Err(usage(
            "an AI client's name is 1 to 32 of a to z, 0 to 9 and -, and not add, list, remove \
             or help"
                .into(),
        ));
    }
    if scopes.is_empty() {
        return Err(usage(
            "add takes at least one --scope: the name of an area, alone or with :read, or *".into(),
        ));
    }
    let scopes = scopes_of(&scopes)?;
    let proof = Input::new().password("Password: ")?;
    let text = told(ask(Console::ClientAdd { proof, name: name.clone(), way, scopes, days })?)?;
    match way {
        Way::Ssh => println!("{}", connects_over_ssh(&util::user_name(util::own_user()), &name)),
        Way::Https => {
            let mut secret = text.into_bytes();
            println!("{}", String::from_utf8_lossy(&secret));
            accounts::wipe(&mut secret);
            eprintln!(
                "This token is not shown again: keep it now. The AI client sends it to the \
                 console's /mcp, as `Authorization: Bearer TOKEN`; `admin fingerprint` says \
                 where the console is."
            );
        }
    }
    Ok(0)
}

/// What is said of a client that was let in over SSH: the command its
/// MCP settings run.
fn connects_over_ssh(owner: &str, name: &str) -> String {
    format!(
        "The AI client {name} is let in. In its MCP settings, the command to run is:\n  ssh \
         {owner}@{} admin mcp {name}\nIt changes nothing by itself: what it asks to change waits \
         for an administrator's yes in the console.",
        util::hostname()
    )
}

/// A client as authd lists it.
#[derive(Debug, Deserialize)]
struct Listed {
    owner: String,
    name: String,
    way: String,
    scopes: Vec<String>,
    until: i64,
    used: i64,
    from: String,
    ended: bool,
}

/// The clients authd's answer `text` lists.
fn clients_in(text: &str) -> Result<Vec<Listed>, Fail> {
    #[derive(Deserialize)]
    struct Answer {
        clients: Vec<Listed>,
    }
    let answer: Answer = serde_json::from_str(text)
        .map_err(|_| fail("authd answered a list of AI clients that does not read".into()))?;
    Ok(answer.clients)
}

/// `clients` as a table: a line each, with its owner when they are every
/// account's, its name, how it connects, what it reaches, the day it
/// ends, and when and from where it was last used.
fn clients_table(clients: &[Listed], owners: bool) -> String {
    let heads = ["OWNER", "NAME", "CONNECTS", "REACHES", "ENDS", "LAST USED"];
    let skipped = usize::from(!owners);
    let mut rows = vec![heads[skipped..].iter().map(|head| (*head).to_string()).collect()];
    for client in clients {
        let day = util::local_time(client.until, "%Y-%m-%d");
        let ends = if client.ended { format!("{day} (ended)") } else { day };
        let used = match (client.used, client.from.as_str()) {
            (0, _) => "never".to_string(),
            (used, "") => util::local_time(used, "%Y-%m-%d %H:%M"),
            (used, from) => format!("{} from {from}", util::local_time(used, "%Y-%m-%d %H:%M")),
        };
        let way = if client.way == Way::Ssh.word() { "over SSH" } else { "over HTTPS" };
        let row = [
            client.owner.clone(),
            client.name.clone(),
            way.to_string(),
            client.scopes.join(", "),
            ends,
            used,
        ];
        rows.push(row[skipped..].to_vec());
    }
    table(&rows)
}

/// Refuses what only an administrator inside elevate may ask, said as
/// the command that works.
fn inside_elevate(what: &str, command: &str) -> Result<(), Fail> {
    if util::own_user() == 0 {
        return Ok(());
    }
    Err(fail(format!("only an administrator {what}: run it with elevate, as `elevate {command}`")))
}

/// `admin mcp list [--all] [--json]`.
fn list(args: &mut Args) -> Result<i32, Fail> {
    let (mut all, mut json) = (false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "all" => all = true,
            Arg::Long(option) if option == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let text = if all {
        inside_elevate("lists every account's AI clients", "admin mcp list --all")?;
        told(ask(Console::ClientsAll)?)?
    } else {
        told(ask(Console::Clients)?)?
    };
    if json {
        println!("{text}");
        return Ok(0);
    }
    let clients = clients_in(&text)?;
    match (clients.is_empty(), all) {
        (true, true) => println!("No account has an AI client."),
        (true, false) => println!("You have no AI clients."),
        (false, _) => print!("{}", clients_table(&clients, all)),
    }
    Ok(0)
}

/// `admin mcp remove NAME [--of OWNER]`.
fn remove(args: &mut Args) -> Result<i32, Fail> {
    let (mut name, mut owner) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "of" => owner = Some(args.text()?),
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(name) = name else {
        return Err(usage("remove takes the name of an AI client".into()));
    };
    match owner {
        Some(owner) => {
            let command = format!("admin mcp remove {name} --of {owner}");
            inside_elevate("removes another account's AI clients", &command)?;
            told(ask(Console::ClientRemoveOf { owner: owner.clone(), name: name.clone() })?)?;
            println!("The AI client {name} of {owner} is removed.");
        }
        None => {
            told(ask(Console::ClientRemove { name: name.clone() })?)?;
            println!("The AI client {name} is removed.");
        }
    }
    Ok(0)
}

/// The address the SSH session this runs in came from, the first of
/// `SSH_CONNECTION`'s words, or nothing: authd keeps it as where the
/// client was last used from.
fn came_from(connection: &str) -> String {
    connection
        .split_whitespace()
        .next()
        .filter(|address| address.parse::<std::net::IpAddr>().is_ok())
        .unwrap_or_default()
        .to_string()
}

/// What the console's greeting `line` says: nothing more when it takes
/// the pass.
///
/// # Errors
///
/// The console's refusal, in its words, with its ID.
fn greeted(line: &str) -> Result<(), String> {
    match line.split_once(' ') {
        Some(("ok", _)) => Ok(()),
        Some(("no", why)) => Err(why.to_string()),
        _ => Err("the console's greeting does not read".into()),
    }
}

/// Reads one line of at most `limit` bytes from `stream`, without its
/// end.
fn line_from(stream: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if byte[0] == b'\n' {
            return Ok(line);
        }
        if line.len() == limit {
            return Err(io::ErrorKind::InvalidData.into());
        }
        line.push(byte[0]);
    }
}

/// Asks authd for a pass for the client `name` of whoever runs this, and
/// gives it to the console's socket: the connection, once the console
/// has taken the pass.
///
/// # Errors
///
/// In words for the person: authd's refusal, the console's, or that the
/// console cannot be reached.
fn connect(name: &str) -> Result<Link, String> {
    let from = came_from(&std::env::var("SSH_CONNECTION").unwrap_or_default());
    let answer =
        ask(Console::ClientPass { name: name.into(), from }).map_err(|fail| fail.message)?;
    let text = told(answer).map_err(|fail| fail.message)?;
    let unread = || "authd answered a pass that does not read".to_string();
    let (pass, until) = text.split_once(' ').ok_or_else(unread)?;
    let until: i64 = until.trim().parse().map_err(|_| unread())?;
    let mut hello = format!("{pass}\n").into_bytes();
    let mut text = text.into_bytes();
    accounts::wipe(&mut text);
    let unreachable = |err: io::Error| {
        format!(
            "cannot reach the web console at {}: {}. `service admind status` says whether it runs.",
            local::PATH,
            util::strip_os_error(&err)
        )
    };
    let stream = UnixStream::connect(local::PATH).map_err(unreachable);
    let greeting = stream.and_then(|stream| {
        let mut timed = Deadline::new(&stream, GREET_TIME).map_err(unreachable)?;
        let sent = timed.write_all(&hello);
        let line = sent.and_then(|()| line_from(&mut timed, GREETING_MAX)).map_err(unreachable)?;
        Ok((stream, line))
    });
    accounts::wipe(&mut hello);
    let (stream, line) = greeting?;
    greeted(&String::from_utf8_lossy(&line))?;
    Ok(Link { stream, until })
}

/// The Unix time.
fn unix_now() -> i64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(now.as_secs()).unwrap_or(i64::MAX)
}

/// What `--check` says of the console's answer to a call of `areas`:
/// what the client `name` reaches.
///
/// # Errors
///
/// What the console refused the call with, in its words.
fn reaches(name: &str, answer: &Value) -> Result<String, String> {
    let result = &answer["result"];
    if let Some(error) = answer["error"]["message"].as_str() {
        return Err(error.to_string());
    }
    if result["isError"] == true {
        let error = &result["structuredContent"]["error"];
        let words = error["detail"].as_str().unwrap_or("the console refused the call");
        return Err(match error["id"].as_str() {
            Some(id) => format!("{id}: {words}"),
            None => words.to_string(),
        });
    }
    let Some(areas) = result["structuredContent"]["areas"].as_array() else {
        return Err("the console's answer does not read".into());
    };
    if areas.is_empty() {
        return Ok(format!(
            "The AI client {name} connects, and reaches nothing: its scopes name no area that \
             this machine has something to offer in.\n"
        ));
    }
    let mut rows =
        vec![["AREA", "REACH", "READS", "CHANGES TO ASK FOR"].map(String::from).to_vec()];
    for area in areas {
        let text = |key: &str| area[key].as_str().unwrap_or_default().to_string();
        let count = |key: &str| area[key].as_u64().unwrap_or(0).to_string();
        rows.push(vec![text("area"), text("reach"), count("reads"), count("changes")]);
    }
    Ok(format!("The AI client {name} connects. It reaches:\n{}", table(&rows)))
}

/// `admin mcp NAME --check`: connects as the client would, asks what it
/// reaches, and says so.
fn check(name: &str) -> Result<i32, Fail> {
    let link = connect(name).map_err(fail)?;
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "areas", "arguments": {} },
    });
    let silent = |err: io::Error| {
        fail(format!("the web console did not answer: {}", util::strip_os_error(&err)))
    };
    let mut timed = Deadline::new(&link.stream, CHECK_TIME).map_err(silent)?;
    timed.write_all(format!("{call}\n").as_bytes()).map_err(silent)?;
    let line = line_from(&mut timed, CHECK_MAX).map_err(silent)?;
    let answer: Value = serde_json::from_slice(&line)
        .map_err(|_| fail("the web console's answer does not read".into()))?;
    print!("{}", reaches(name, &answer).map_err(fail)?);
    Ok(0)
}

/// `admin mcp NAME [--check]`: the bridge for the client `name`, or a
/// check that it connects.
fn client(name: &str, args: &mut Args) -> Result<i32, Fail> {
    let mut checks = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "check" => checks = true,
            other => return Err(args.unknown(&other)),
        }
    }
    if !clients::name_ok(name) {
        return Err(usage(format!(
            "{name} is not an AI client's name, which is 1 to 32 of a to z, 0 to 9 and -\nusage: \
             {USAGE}"
        )));
    }
    if checks {
        return check(name);
    }
    // Said before the first message, so that a client that cannot
    // connect ends with words its settings' page shows.
    let first = connect(name).map_err(fail)?;
    let mut again = || connect(name);
    let carried = bridge::carry(
        Standard::Input.fd(),
        io::stdout().lock(),
        Some(first),
        &mut again,
        &unix_now,
    );
    match carried {
        Ok(()) => Ok(0),
        // The client went without reading its last answers.
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        Err(err) => {
            Err(fail(format!("the AI client's connection: {}", util::strip_os_error(&err))))
        }
    }
}

#[cfg(test)]
mod tests;
