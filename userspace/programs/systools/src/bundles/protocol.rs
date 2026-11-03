// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bundle(1) and bundled(8) say to each other on bundled's socket
//! (ADRs 0223, 0224 and 0225). A request is one line, tab-separated; an
//! `install` request comes with the archive's descriptor (SCM_RIGHTS).
//! bundled answers with lines too: the install sheet (`sheet N` and N bytes
//! of text), a question for a yes or a no, a listed bundle, the bundle to
//! run, what the asker said of its permissions, and last `ok` or `error`
//! with why. A `watch` connection then stays open, for the permissions
//! the person takes away while the app runs.

use std::io::{self, BufRead, Read};

use super::grants::{self, Grant, Said};
use super::history::Event;
use super::minisign::PublicKey;
use super::registry::Place;

/// Where bundled answers.
pub const SOCKET: &str = "/system/live/bundled/socket";
/// The longest request line, and the longest answer line.
pub const LINE_MAX: usize = 4096;
/// The longest sheet.
pub const SHEET_MAX: usize = 64 * 1024;

/// What bundle asks bundled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Installs the archive sent with the request, for everyone or for the
    /// asker; with `yes`, without asking after the sheet.
    Install { everyone: bool, yes: bool },
    /// Lists the bundles the asker can run.
    List,
    /// Shows an installed bundle's sheet.
    Info { id: String },
    /// Removes a bundle, for everyone or the asker's; with `purge`, the
    /// asker's data of it too.
    Remove { id: String, everyone: bool, purge: bool },
    /// Keeps the asker's developer key.
    DeveloperKey { key: PublicKey },
    /// Finds the bundle to run, of the asker's or everyone's.
    Run { find: Find },
    /// Asks the person, through the trusted prompt, whether app `id`,
    /// whose signed list's SHA-256 is `list`, may have `permission`, as
    /// its manifest asks for it (ADR 0224).
    Ask { id: String, permission: String, list: String },
    /// Gives app `id` `permission`, with `words`, or as it asks for it
    /// with none, after the trusted prompt's yes.
    Grant { id: String, permission: String, words: Vec<String> },
    /// Takes `permission` away from app `id`, at once.
    Revoke { id: String, permission: String },
    /// Keeps the connection, to say each permission the person takes
    /// away from app `id` while it runs.
    Watch { id: String },
    /// Installs the newest version of `id` a catalog lists, from
    /// `catalog` or the first that lists it (ADR 0383), as `Install`.
    Get { id: String, catalog: Option<String>, everyone: bool, yes: bool },
    /// Lists the bundles the catalogs list whose ID or name holds every
    /// one of `words`.
    Search { words: Vec<String> },
    /// Lists the catalogs.
    Catalogs,
    /// Adds catalog `name` at `url`, with the root sent with the request:
    /// an administrator's.
    CatalogAdd { name: String, url: String },
    /// Removes catalog `name`, or turns off one the system comes with:
    /// an administrator's.
    CatalogRemove { name: String },
    /// Updates the bundles `ids`, or every one, of the asker's or
    /// everyone's (ADR 0384); with `check`, only says which would update;
    /// with `yes`, without asking about one that asks for more.
    Update { ids: Vec<String>, everyone: bool, check: bool, yes: bool },
    /// Goes back to the version bundle `id` replaced.
    Rollback { id: String, everyone: bool },
    /// Holds bundle `id` at its version, or lets it go.
    Hold { id: String, everyone: bool, on: bool },
    /// Turns updating by themselves on or off, for the asker's bundles or
    /// everyone's.
    Automatic { everyone: bool, on: bool },
    /// What bundled did to the asker's bundles and everyone's, or to `id`.
    History { id: Option<String> },
}

/// Which bundle `run` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Find {
    /// The bundle of this ID.
    Id(String),
    /// The bundle that adds this command.
    Command(String),
}

fn whose(everyone: bool) -> &'static str {
    if everyone { "everyone" } else { "me" }
}

fn checked_id(id: &str) -> Result<String, String> {
    manifest::bundle::check_id(id).map_err(|why| format!("{id:?}: {why}"))?;
    Ok(id.into())
}

fn checked_catalog(name: &str) -> Result<String, String> {
    super::catalogs::check_name(name)?;
    Ok(name.into())
}

/// A word to search for: printable, without tabs.
fn checked_word(word: &str) -> Result<String, String> {
    if word.is_empty() || word.chars().any(char::is_control) {
        return Err(format!("{word:?} is no word to search for"));
    }
    Ok(word.into())
}

fn checked_permission(name: &str) -> Result<String, String> {
    manifest::bundle::permissions::find(name)
        .map(|p| p.name.to_string())
        .ok_or_else(|| format!("{name:?} is no permission of the list"))
}

/// A list's SHA-256, as 64 hex digits, in lowercase.
fn checked_list(list: &str) -> Result<String, String> {
    let hex = list.len() == 64 && list.bytes().all(|b| b.is_ascii_hexdigit());
    hex.then(|| list.to_ascii_lowercase()).ok_or_else(|| "not a list's SHA-256".into())
}

/// Words, separated by commas, `-` for none.
fn word_list(words: &[String]) -> String {
    if words.is_empty() { "-".into() } else { words.join(",") }
}

/// Words a request or an answer carries: each of a permission's words
/// as [`grants::check_word`] takes them.
fn read_words(permission: &str, field: &str) -> Result<Vec<String>, String> {
    if field == "-" {
        return Ok(Vec::new());
    }
    let found = manifest::bundle::permissions::find(permission)
        .ok_or_else(|| format!("{permission:?} is no permission of the list"))?;
    let words: Vec<String> = field.split(',').map(String::from).collect();
    for word in &words {
        grants::check_word(found, word)?;
    }
    Ok(words)
}

impl Request {
    /// The request's line, with its newline.
    pub fn line(&self) -> String {
        match self {
            Request::Install { everyone, yes } => {
                format!("install\t{}\t{}\n", whose(*everyone), if *yes { "yes" } else { "ask" })
            }
            Request::List => "list\n".into(),
            Request::Info { id } => format!("info\t{id}\n"),
            Request::Remove { id, everyone, purge } => {
                let data = if *purge { "purge" } else { "keep-data" };
                format!("remove\t{id}\t{}\t{data}\n", whose(*everyone))
            }
            Request::DeveloperKey { key } => format!("developer-key\t{}\n", key.encoded()),
            Request::Run { find: Find::Id(id) } => format!("run\tid\t{id}\n"),
            Request::Run { find: Find::Command(name) } => format!("run\tcommand\t{name}\n"),
            Request::Ask { id, permission, list } => format!("ask\t{id}\t{permission}\t{list}\n"),
            Request::Grant { id, permission, words } => {
                format!("grant\t{id}\t{permission}\t{}\n", word_list(words))
            }
            Request::Revoke { id, permission } => format!("revoke\t{id}\t{permission}\n"),
            Request::Watch { id } => format!("watch\t{id}\n"),
            Request::Get { id, catalog, everyone, yes } => format!(
                "get\t{id}\t{}\t{}\t{}\n",
                catalog.as_deref().unwrap_or("-"),
                whose(*everyone),
                if *yes { "yes" } else { "ask" }
            ),
            Request::Search { words } => format!("search\t{}\n", words.join("\t")),
            Request::Catalogs => "catalogs\n".into(),
            Request::CatalogAdd { name, url } => format!("catalog-add\t{name}\t{url}\n"),
            Request::CatalogRemove { name } => format!("catalog-remove\t{name}\n"),
            Request::Update { ids, everyone, check, yes } => {
                let mut line = format!(
                    "update\t{}\t{}\t{}",
                    whose(*everyone),
                    if *check { "check" } else { "apply" },
                    if *yes { "yes" } else { "ask" }
                );
                for id in ids {
                    line += &format!("\t{id}");
                }
                line + "\n"
            }
            Request::Rollback { id, everyone } => format!("rollback\t{id}\t{}\n", whose(*everyone)),
            Request::Hold { id, everyone, on } => {
                format!("hold\t{id}\t{}\t{}\n", whose(*everyone), if *on { "on" } else { "off" })
            }
            Request::Automatic { everyone, on } => {
                format!("automatic\t{}\t{}\n", whose(*everyone), if *on { "on" } else { "off" })
            }
            Request::History { id } => format!("history\t{}\n", id.as_deref().unwrap_or("-")),
        }
    }

    /// Reads a request's line, without its newline.
    pub fn read(line: &str) -> Result<Request, String> {
        let words: Vec<&str> = line.split('\t').collect();
        let everyone = |word: &str| match word {
            "everyone" => Ok(true),
            "me" => Ok(false),
            _ => Err(format!("{word:?} is neither everyone nor me")),
        };
        let yes = |word: &str| match word {
            "yes" => Ok(true),
            "ask" => Ok(false),
            _ => Err(format!("{word:?} is neither yes nor ask")),
        };
        let on = |word: &str| match word {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => Err(format!("{word:?} is neither on nor off")),
        };
        match words.as_slice() {
            ["install", place, answer] => {
                Ok(Request::Install { everyone: everyone(place)?, yes: yes(answer)? })
            }
            ["get", id, catalog, place, answer] => Ok(Request::Get {
                id: checked_id(id)?,
                catalog: if *catalog == "-" { None } else { Some(checked_catalog(catalog)?) },
                everyone: everyone(place)?,
                yes: yes(answer)?,
            }),
            ["search", words @ ..] if !words.is_empty() => Ok(Request::Search {
                words: words.iter().map(|w| checked_word(w)).collect::<Result<_, _>>()?,
            }),
            ["catalogs"] => Ok(Request::Catalogs),
            ["catalog-add", name, url] => Ok(Request::CatalogAdd {
                name: checked_catalog(name)?,
                url: super::catalogs::check_url(url)?,
            }),
            ["catalog-remove", name] => Ok(Request::CatalogRemove { name: checked_catalog(name)? }),
            ["update", place, mode, answer, ids @ ..] => Ok(Request::Update {
                ids: ids.iter().map(|id| checked_id(id)).collect::<Result<_, _>>()?,
                everyone: everyone(place)?,
                check: match *mode {
                    "check" => true,
                    "apply" => false,
                    _ => return Err(format!("{mode:?} is neither check nor apply")),
                },
                yes: yes(answer)?,
            }),
            ["rollback", id, place] => {
                Ok(Request::Rollback { id: checked_id(id)?, everyone: everyone(place)? })
            }
            ["hold", id, place, state] => Ok(Request::Hold {
                id: checked_id(id)?,
                everyone: everyone(place)?,
                on: on(state)?,
            }),
            ["automatic", place, state] => {
                Ok(Request::Automatic { everyone: everyone(place)?, on: on(state)? })
            }
            ["history", id] => {
                Ok(Request::History { id: if *id == "-" { None } else { Some(checked_id(id)?) } })
            }
            ["list"] => Ok(Request::List),
            ["info", id] => Ok(Request::Info { id: checked_id(id)? }),
            ["remove", id, place, data] => {
                let purge = match *data {
                    "purge" => true,
                    "keep-data" => false,
                    _ => return Err(format!("{data:?} is neither purge nor keep-data")),
                };
                Ok(Request::Remove { id: checked_id(id)?, everyone: everyone(place)?, purge })
            }
            ["developer-key", key] => Ok(Request::DeveloperKey { key: PublicKey::decode(key)? }),
            ["run", "id", id] => Ok(Request::Run { find: Find::Id(checked_id(id)?) }),
            ["run", "command", name] => {
                manifest::bundle::check_command_name(name)?;
                Ok(Request::Run { find: Find::Command((*name).into()) })
            }
            ["ask", id, permission, list] => Ok(Request::Ask {
                id: checked_id(id)?,
                permission: checked_permission(permission)?,
                list: checked_list(list)?,
            }),
            ["grant", id, permission, words] => {
                let permission = checked_permission(permission)?;
                let words = read_words(&permission, words)?;
                Ok(Request::Grant { id: checked_id(id)?, permission, words })
            }
            ["revoke", id, permission] => Ok(Request::Revoke {
                id: checked_id(id)?,
                permission: checked_permission(permission)?,
            }),
            ["watch", id] => Ok(Request::Watch { id: checked_id(id)? }),
            _ => Err("not a request bundled knows".into()),
        }
    }
}

/// A bundle as `list` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub place: Place,
    pub id: String,
    pub version: String,
    pub key: PublicKey,
    /// When it was installed, in seconds since 1970.
    pub when: u64,
    /// Its name, as it says it, shown.
    pub name: String,
}

/// The bundle to run, as `run` finds it: where, and what bundled
/// recorded of it, which the runner checks the folder against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub place: Place,
    pub id: String,
    pub key: PublicKey,
    /// The SHA-256 of its signed list, in hex.
    pub list: String,
}

/// A bundle a catalog lists, as `search` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub catalog: String,
    pub id: String,
    pub version: String,
    /// Its name, as the catalog gives it, shown.
    pub name: String,
}

/// A catalog, as `catalogs` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub name: String,
    pub url: String,
    /// Whether it is off, or the system's own.
    pub off: bool,
    pub default: bool,
    /// How many bundles it lists, and when bundled last read it and what
    /// it read expires, in seconds since 1970; none if never.
    pub bundles: usize,
    pub read: Option<i64>,
    pub expires: Option<i64>,
}

/// A bundle that would update, as `update --check` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub place: Place,
    pub id: String,
    /// The version installed, and the newest its catalog lists.
    pub from: String,
    pub to: String,
    pub catalog: String,
    /// Whether it is held at its version.
    pub held: bool,
}

/// What bundled answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The install sheet's text.
    Sheet(String),
    /// Whether to go on: bundle answers `yes` or `no`.
    Question,
    /// A bundle of a list.
    Listed(Listed),
    /// The bundle to run.
    Found(Found),
    /// What the asker said of one of its permissions.
    Granted(Grant),
    /// A permission the person took away, on a `watch` connection.
    Revoked(String),
    /// A bundle a catalog lists.
    Hit(Hit),
    /// A catalog.
    Catalog(Shown),
    /// A bundle that would update.
    Available(Available),
    /// Something bundled did, of the history.
    Event(Event),
    /// What became of one of several things asked, such as one bundle of
    /// an update.
    Note(String),
    /// Done, and what was done.
    Ok(String),
    /// Not done, and why.
    Error(String),
}

/// Text on one line, as an answer carries it.
fn one_line(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

impl Answer {
    /// The answer's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Answer::Sheet(text) => {
                let text: String = text.chars().filter(|&c| c == '\n' || !c.is_control()).collect();
                format!("sheet {}\n{text}", text.len()).into_bytes()
            }
            Answer::Question => b"question\n".to_vec(),
            Answer::Listed(l) => format!(
                "listed\t{}\t{}\t{}\t{}\t{}\t{}\n",
                l.place,
                l.id,
                one_line(&l.version),
                l.key.encoded(),
                l.when,
                one_line(&l.name)
            )
            .into_bytes(),
            Answer::Found(f) => {
                format!("found\t{}\t{}\t{}\t{}\n", f.place, f.id, f.key.encoded(), f.list)
                    .into_bytes()
            }
            Answer::Granted(g) => format!(
                "granted\t{}\t{}\t{}\t{}\t{}\n",
                g.permission,
                g.said.word(),
                g.since,
                word_list(&g.given),
                word_list(&g.asked)
            )
            .into_bytes(),
            Answer::Revoked(permission) => format!("revoked\t{permission}\n").into_bytes(),
            Answer::Hit(h) => format!(
                "hit\t{}\t{}\t{}\t{}\n",
                h.catalog,
                h.id,
                one_line(&h.version),
                one_line(&h.name)
            )
            .into_bytes(),
            Answer::Catalog(c) => {
                let time = |t: Option<i64>| t.map_or_else(|| "-".into(), |t| t.to_string());
                format!(
                    "catalog\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    c.name,
                    one_line(&c.url),
                    if c.off { "off" } else { "on" },
                    if c.default { "default" } else { "added" },
                    c.bundles,
                    time(c.read),
                    time(c.expires)
                )
                .into_bytes()
            }
            Answer::Available(a) => format!(
                "available\t{}\t{}\t{}\t{}\t{}\t{}\n",
                a.place,
                a.id,
                one_line(&a.from),
                one_line(&a.to),
                a.catalog,
                if a.held { "held" } else { "free" }
            )
            .into_bytes(),
            Answer::Event(e) => format!("event\t{}", e.line()).into_bytes(),
            Answer::Note(what) => format!("note {}\n", one_line(what)).into_bytes(),
            Answer::Ok(what) => format!("ok {}\n", one_line(what)).into_bytes(),
            Answer::Error(why) => format!("error {}\n", one_line(why)).into_bytes(),
        }
    }

    /// Reads the next answer from `input`; none at its end.
    pub fn read(input: &mut impl BufRead) -> io::Result<Option<Answer>> {
        let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
        let mut line = Vec::new();
        let n = input.by_ref().take(LINE_MAX as u64 + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(None);
        }
        if line.pop() != Some(b'\n') {
            return Err(bad("bundled's answer has no end"));
        }
        let line = String::from_utf8(line).map_err(|_| bad("bundled's answer is not text"))?;
        if let Some(length) = line.strip_prefix("sheet ") {
            let length: usize = length.parse().map_err(|_| bad("a sheet without its length"))?;
            if length > SHEET_MAX {
                return Err(bad("bundled's sheet is too long"));
            }
            let mut text = vec![0; length];
            input.read_exact(&mut text)?;
            let text = String::from_utf8(text).map_err(|_| bad("bundled's sheet is not text"))?;
            return Ok(Some(Answer::Sheet(
                text.chars().filter(|&c| c == '\n' || !c.is_control()).collect(),
            )));
        }
        if line == "question" {
            return Ok(Some(Answer::Question));
        }
        if let Some(what) = line.strip_prefix("ok ") {
            return Ok(Some(Answer::Ok(one_line(what))));
        }
        if let Some(why) = line.strip_prefix("error ") {
            return Ok(Some(Answer::Error(one_line(why))));
        }
        if let Some(what) = line.strip_prefix("note ") {
            return Ok(Some(Answer::Note(one_line(what))));
        }
        if let Some(event) = line.strip_prefix("event\t") {
            return Ok(Some(Answer::Event(Event::read(event).map_err(|why| bad(&why))?)));
        }
        let words: Vec<&str> = line.split('\t').collect();
        let place = |word: &str| match word {
            "everyone" => Ok(Place::Everyone),
            uid => uid.parse().map(Place::User).map_err(|_| bad("a bundle's place")),
        };
        if let ["found", at, id, key, list] = words.as_slice() {
            return Ok(Some(Answer::Found(Found {
                place: place(at)?,
                id: checked_id(id).map_err(|why| bad(&why))?,
                key: PublicKey::decode(key).map_err(|why| bad(&why))?,
                list: checked_list(list).map_err(|_| bad("a found bundle's list"))?,
            })));
        }
        if let ["granted", permission, said, since, given, asked] = words.as_slice() {
            let permission = checked_permission(permission).map_err(|why| bad(&why))?;
            return Ok(Some(Answer::Granted(Grant {
                said: Said::from_word(said).ok_or_else(|| bad("neither allow nor never"))?,
                since: since.parse().map_err(|_| bad("a grant's time"))?,
                given: read_words(&permission, given).map_err(|why| bad(&why))?,
                asked: read_words(&permission, asked).map_err(|why| bad(&why))?,
                permission,
            })));
        }
        if let ["hit", catalog, id, version, name] = words.as_slice() {
            return Ok(Some(Answer::Hit(Hit {
                catalog: checked_catalog(catalog).map_err(|why| bad(&why))?,
                id: checked_id(id).map_err(|why| bad(&why))?,
                version: one_line(version),
                name: one_line(name),
            })));
        }
        if let ["catalog", name, url, off, default, bundles, read, expires] = words.as_slice() {
            let time = |t: &str| -> io::Result<Option<i64>> {
                if t == "-" {
                    return Ok(None);
                }
                t.parse().map(Some).map_err(|_| bad("a catalog's time"))
            };
            return Ok(Some(Answer::Catalog(Shown {
                name: checked_catalog(name).map_err(|why| bad(&why))?,
                url: one_line(url),
                off: match *off {
                    "off" => true,
                    "on" => false,
                    _ => return Err(bad("neither on nor off")),
                },
                default: match *default {
                    "default" => true,
                    "added" => false,
                    _ => return Err(bad("neither default nor added")),
                },
                bundles: bundles.parse().map_err(|_| bad("a catalog's bundles"))?,
                read: time(read)?,
                expires: time(expires)?,
            })));
        }
        if let ["available", at, id, from, to, catalog, held] = words.as_slice() {
            return Ok(Some(Answer::Available(Available {
                place: place(at)?,
                id: checked_id(id).map_err(|why| bad(&why))?,
                from: one_line(from),
                to: one_line(to),
                catalog: checked_catalog(catalog).map_err(|why| bad(&why))?,
                held: match *held {
                    "held" => true,
                    "free" => false,
                    _ => return Err(bad("neither held nor free")),
                },
            })));
        }
        if let ["revoked", permission] = words.as_slice() {
            let permission = checked_permission(permission).map_err(|why| bad(&why))?;
            return Ok(Some(Answer::Revoked(permission)));
        }
        if let ["listed", place_word, id, version, key, when, name] = words.as_slice() {
            let place = place(place_word)?;
            return Ok(Some(Answer::Listed(Listed {
                place,
                id: checked_id(id).map_err(|why| bad(&why))?,
                version: one_line(version),
                key: PublicKey::decode(key).map_err(|why| bad(&why))?,
                when: when.parse().map_err(|_| bad("a listed bundle's time"))?,
                name: one_line(name),
            })));
        }
        Err(bad("not an answer bundle knows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::minisign::SecretKey;

    fn key() -> PublicKey {
        let mut random = [9u8; 40];
        random[0] = 1;
        SecretKey::generate(&random).unwrap().public()
    }

    #[test]
    fn requests_read_back() {
        for request in [
            Request::Install { everyone: false, yes: false },
            Request::Install { everyone: true, yes: true },
            Request::List,
            Request::Info { id: "org.example.notes".into() },
            Request::Remove { id: "org.example.notes".into(), everyone: false, purge: true },
            Request::Remove { id: "org.example.notes".into(), everyone: true, purge: false },
            Request::DeveloperKey { key: key() },
            Request::Run { find: Find::Id("org.example.notes".into()) },
            Request::Run { find: Find::Command("notes".into()) },
            Request::Ask {
                id: "org.example.notes".into(),
                permission: "files.folder".into(),
                list: "ab".repeat(32),
            },
            Request::Grant {
                id: "org.example.notes".into(),
                permission: "files.folder".into(),
                words: vec!["documents=read".into(), "music=read-write".into()],
            },
            Request::Grant {
                id: "org.example.notes".into(),
                permission: "files.home".into(),
                words: Vec::new(),
            },
            Request::Revoke {
                id: "org.example.notes".into(),
                permission: "network.outbound".into(),
            },
            Request::Watch { id: "org.example.notes".into() },
            Request::Get {
                id: "io.github.burntsushi.ripgrep".into(),
                catalog: None,
                everyone: false,
                yes: false,
            },
            Request::Get {
                id: "io.github.burntsushi.ripgrep".into(),
                catalog: Some("dukovo".into()),
                everyone: true,
                yes: true,
            },
            Request::Search { words: vec!["rip".into(), "grep tool".into()] },
            Request::Catalogs,
            Request::CatalogAdd { name: "feed".into(), url: "https://example.org/catalog/".into() },
            Request::CatalogRemove { name: "dukovo".into() },
            Request::Update { ids: Vec::new(), everyone: false, check: true, yes: false },
            Request::Update {
                ids: vec!["org.example.notes".into(), "org.example.tea".into()],
                everyone: true,
                check: false,
                yes: true,
            },
            Request::Rollback { id: "org.example.notes".into(), everyone: false },
            Request::Hold { id: "org.example.notes".into(), everyone: true, on: true },
            Request::Hold { id: "org.example.notes".into(), everyone: false, on: false },
            Request::Automatic { everyone: false, on: true },
            Request::History { id: None },
            Request::History { id: Some("org.example.notes".into()) },
        ] {
            let line = request.line();
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
            assert_eq!(Request::read(line.trim_end_matches('\n')), Ok(request));
        }
        for bad in [
            "",
            "install\tme",
            "install\tall\tyes",
            "install\tme\tsure",
            "info\tNotes",
            "info\torg.example.notes\textra",
            "remove\torg.example.notes\tme\tall",
            "developer-key\tRWnope",
            "uninstall\torg.example.notes",
            "run\tid\tNotes",
            "run\tcommand\t-rf",
            "run\tcommand\ta/b",
            "run\tprogram\tnotes",
            "ask\torg.example.notes\tfiles.folder\tab",
            "ask\torg.example.notes\tfiles.everything\t-",
            "grant\torg.example.notes\tfiles.folder\tsecrets=read",
            "grant\torg.example.notes\tfiles.home\tdocuments=read",
            "grant\torg.example.notes\tfiles.folder",
            "revoke\torg.example.notes",
            "watch\tNotes",
            "get\tRipgrep\t-\tme\task",
            "get\tio.github.burntsushi.ripgrep\tDukovo\tme\task",
            "search",
            "search\t",
            "catalog-add\tfeed\tftp://example.org/",
            "catalog-remove\t../x",
            "update\tme\tcheck",
            "update\tme\tmaybe\task",
            "update\tme\tapply\task\tNotes",
            "rollback\torg.example.notes",
            "hold\torg.example.notes\tme\tsometimes",
            "automatic\tall\ton",
            "history",
        ] {
            assert!(Request::read(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn answers_read_back() {
        let listed = Listed {
            place: Place::User(1000),
            id: "org.example.notes".into(),
            version: "0.1.0".into(),
            key: key(),
            when: 1_793_000_000,
            name: "Notes".into(),
        };
        let answers = [
            Answer::Sheet("Notes 0.1.0, for yourself\n  Signed by key RW...\n".into()),
            Answer::Question,
            Answer::Listed(listed),
            Answer::Found(Found {
                place: Place::Everyone,
                id: "org.example.notes".into(),
                key: key(),
                list: "ab".repeat(32),
            }),
            Answer::Granted(Grant {
                permission: "files.folder".into(),
                said: Said::Allow,
                since: 1_793_000_000,
                given: vec!["documents=read".into()],
                asked: vec!["documents=read-write".into()],
            }),
            Answer::Granted(Grant {
                permission: "network.outbound".into(),
                said: Said::Never,
                since: 1,
                given: Vec::new(),
                asked: Vec::new(),
            }),
            Answer::Revoked("files.folder".into()),
            Answer::Hit(Hit {
                catalog: "dukovo".into(),
                id: "io.github.sharkdp.bat".into(),
                version: "0.26.1".into(),
                name: "bat".into(),
            }),
            Answer::Catalog(Shown {
                name: "dukovo".into(),
                url: "https://updates.dukovo.org/catalog/".into(),
                off: false,
                default: true,
                bundles: 2,
                read: Some(1_793_000_000),
                expires: Some(1_793_086_400),
            }),
            Answer::Catalog(Shown {
                name: "feed".into(),
                url: "file:///volumes/vdb/catalog/".into(),
                off: true,
                default: false,
                bundles: 0,
                read: None,
                expires: None,
            }),
            Answer::Available(Available {
                place: Place::User(1000),
                id: "org.example.notes".into(),
                from: "1.0.0".into(),
                to: "1.1.0".into(),
                catalog: "dukovo".into(),
                held: true,
            }),
            Answer::Event(crate::bundles::history::Event {
                when: 1_793_000_000,
                place: Place::Everyone,
                id: "org.example.notes".into(),
                what: crate::bundles::history::What::RolledBack,
                from: Some("1.1.0".into()),
                to: Some("1.0.0".into()),
                by: Some(0),
                note: String::new(),
            }),
            Answer::Note("updated org.example.notes 1.0.0 to 1.1.0".into()),
            Answer::Ok("installed org.example.notes 0.1.0".into()),
            Answer::Error("it is not signed".into()),
        ];
        let bytes: Vec<u8> = answers.iter().flat_map(Answer::bytes).collect();
        let mut input = io::Cursor::new(bytes);
        for answer in &answers {
            assert_eq!(Answer::read(&mut input).unwrap().as_ref(), Some(answer));
        }
        assert_eq!(Answer::read(&mut input).unwrap(), None);
        // What could draw on the terminal never arrives.
        let sneaky = Answer::Sheet("a\u{1b}[2Jb\n".into()).bytes();
        let read = Answer::read(&mut io::Cursor::new(sneaky)).unwrap();
        assert_eq!(read, Some(Answer::Sheet("a[2Jb\n".into())));
        let sneaky = Answer::Error("x\ny\u{7}".into()).bytes();
        assert_eq!(sneaky, b"error x y \n");
        for bad in [
            &b"sheet 5\nab"[..],
            b"sheet x\n",
            b"maybe\n",
            b"ok fine",
            b"sheet 99999999\n",
            b"found\teveryone\torg.example.notes\tRWnope\tab\n",
            b"granted\tfiles.folder\tmaybe\t1\t-\t-\n",
            b"granted\tfiles.folder\tallow\t1\tsecrets=read\t-\n",
            b"revoked\tfiles.everything\n",
            b"available\t1000\torg.example.notes\t1\t2\tdukovo\tmaybe\n",
            b"event\t1\t1000\tNotes\tupdated\t-\t-\t1\t-\n",
        ] {
            assert!(Answer::read(&mut io::Cursor::new(bad.to_vec())).is_err(), "{bad:?}");
        }
    }
}
