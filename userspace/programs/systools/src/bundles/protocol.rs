// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bundle(1) and bundled(8) say to each other on bundled's socket
//! (ADRs 0223, 0224 and 0225). A request is one line, tab-separated; an
//! `install` request comes with the archive's descriptor (SCM_RIGHTS).
//! bundled answers with lines too: the install sheet (`sheet N` and N bytes
//! of text), a question for a yes or a no, a listed bundle, the bundle to
//! run, or the lines of an ID to choose it from (ADR 0387), what the
//! asker said of its permissions, and last `ok` or `error` with why. A `watch` connection then stays open, for the permissions
//! the person takes away while the app runs.

use std::io::{self, BufRead, Read};

use super::catalogs::Kind;
use super::grants::{self, Grant, Said};
use super::history::Event;
use super::minisign::PublicKey;
use super::registry::{Place, Source};
use dkv::flaws::Severity;

/// Where bundled answers.
pub const SOCKET: &str = "/system/live/bundled/socket";
/// The longest request line, and the longest answer line.
pub const LINE_MAX: usize = 4096;
/// The longest sheet.
pub const SHEET_MAX: usize = 64 * 1024;
/// The most characters of the reason kept with a hold.
pub const WHY_MAX: usize = 200;
/// What an answer says between what was done and the reason kept with it.
pub const REASON_KEPT: &str = "; the reason kept with it: ";

/// Whether `text` can be the reason kept with a hold: one line of 1 to
/// [`WHY_MAX`] characters, with no control character (a tab would break
/// the history's fields and a request's).
#[must_use]
pub fn why_ok(text: &str) -> bool {
    (1..=WHY_MAX).contains(&text.chars().count()) && !text.chars().any(char::is_control)
}

/// What bundle asks bundled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Installs the archive sent with the request, for everyone or for the
    /// asker; with `yes`, without asking after the sheet.
    Install { everyone: bool, yes: bool },
    /// Lists the bundles the asker can run.
    List,
    /// Shows an installed bundle's sheet, or, with `json`, says its facts
    /// as one JSON document (ADR 0541).
    Info { id: String, json: bool },
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
    /// `catalog` or the first that lists it (ADR 0383), as `Install`; with
    /// `sha256`, only a file that has it, and otherwise nothing is
    /// fetched or unpacked.
    Get { id: String, catalog: Option<String>, everyone: bool, yes: bool, sha256: Option<String> },
    /// Lists the bundles the catalogs list whose ID or name holds every
    /// one of `words`; with `json`, as one JSON document of the bundles of
    /// the catalogs read now and of the catalogs that could not be, and
    /// then `words` may be none, for every bundle.
    Search { words: Vec<String>, json: bool },
    /// Fetches the newest version of `id` from `from`, or the first
    /// catalog that lists it, checks it as an install does, and says its
    /// sheet, and then its facts as one JSON document; installs nothing.
    Sheet { id: String, from: Option<String> },
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
    /// Holds bundle `id` at its version, or lets it go; a hold may keep
    /// the reason `why`, which [`why_ok`] accepts. `by` is the person who
    /// asked, which root may say for the person it runs for (the web
    /// console runs `bundle hold` elevated); bundled takes anyone else as
    /// themselves.
    Hold { id: String, everyone: bool, on: bool, by: u32, why: Option<String> },
    /// Has the updates of `ids`, or of every app that has one when there
    /// is none, installed in tonight's window and not now (`on`), or takes
    /// the apps off it (ADR 0565). `by` is the user who asked, which
    /// `elevate` knows and the connection does not say. An administrator's.
    Tonight { ids: Vec<String>, on: bool, yes: bool, by: u32 },
    /// Turns updating by themselves on or off, for the asker's bundles or
    /// everyone's.
    Automatic { everyone: bool, on: bool },
    /// Whether everyone's bundles update by themselves: `on` or `off`,
    /// which anyone may ask, since bundled's state is root's to read.
    AutomaticForEveryone,
    /// What bundled did to the asker's bundles and everyone's, or to `id`.
    History { id: Option<String> },
    /// The lines of `name`, an ID or a command one of its lines adds,
    /// installed for the asker or for everyone (ADR 0387).
    Lines { name: String },
    /// Makes `line` the default line of `name`'s ID, the asker's or
    /// everyone's, or forgets it with none.
    Default { name: String, line: Option<String>, everyone: bool },
    /// Links the commands of the asker's add-ons anew: their own, or
    /// everyone's for an administrator (ADR 0388).
    AddOns,
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

/// An ID, or a full ID with its line (ADR 0387).
fn checked_id(id: &str) -> Result<String, String> {
    manifest::bundle::check_full_id(id).map_err(|why| format!("{id:?}: {why}"))?;
    Ok(id.into())
}

/// An ID or a command's name, as `bundle default` takes them.
fn checked_name(name: &str) -> Result<String, String> {
    if manifest::bundle::check_id(name).is_ok() {
        return Ok(name.into());
    }
    manifest::bundle::check_command_name(name).map_err(|why| format!("{name:?}: {why}"))?;
    Ok(name.into())
}

fn checked_line(line: &str) -> Result<String, String> {
    manifest::bundle::check_version_line(line).map_err(|why| format!("{line:?}: {why}"))?;
    Ok(line.into())
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

/// A file's SHA-256, as 64 hex digits, kept in lowercase.
fn checked_sha256(hex: &str) -> Result<String, String> {
    let ok = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| hex.to_ascii_lowercase()).ok_or_else(|| "not a SHA-256 of 64 hex digits".into())
}

/// A list's SHA-256, as 64 hex digits, in lowercase.
fn checked_list(list: &str) -> Result<String, String> {
    checked_sha256(list).map_err(|_| "not a list's SHA-256".into())
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
            Request::Info { id, json: false } => format!("info\t{id}\n"),
            Request::Info { id, json: true } => format!("info\t{id}\tjson\n"),
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
            Request::Get { id, catalog, everyone, yes, sha256 } => format!(
                "get\t{id}\t{}\t{}\t{}{}\n",
                catalog.as_deref().unwrap_or("-"),
                whose(*everyone),
                if *yes { "yes" } else { "ask" },
                // An older bundled reads no more than the four words.
                sha256.as_ref().map_or_else(String::new, |hex| format!("\t{hex}"))
            ),
            Request::Search { words, json: false } => format!("search\t{}\n", words.join("\t")),
            Request::Search { words, json: true } => {
                let mut line = "search-json".to_string();
                for word in words {
                    line += &format!("\t{word}");
                }
                line + "\n"
            }
            Request::Sheet { id, from } => {
                format!("sheet\t{id}\t{}\n", from.as_deref().unwrap_or("-"))
            }
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
            Request::Hold { id, everyone, on, by, why } => format!(
                "hold\t{id}\t{}\t{}\t{by}{}\n",
                whose(*everyone),
                if *on { "on" } else { "off" },
                why.as_ref().map_or_else(String::new, |why| format!("\t{why}"))
            ),
            Request::Tonight { ids, on, yes, by } => {
                let mut line = format!(
                    "tonight\t{}\t{}\t{by}",
                    if *on { "on" } else { "off" },
                    if *yes { "yes" } else { "ask" }
                );
                for id in ids {
                    line += &format!("\t{id}");
                }
                line + "\n"
            }
            Request::Automatic { everyone, on } => {
                format!("automatic\t{}\t{}\n", whose(*everyone), if *on { "on" } else { "off" })
            }
            Request::AutomaticForEveryone => "automatic\n".into(),
            Request::History { id } => format!("history\t{}\n", id.as_deref().unwrap_or("-")),
            Request::Lines { name } => format!("lines\t{name}\n"),
            Request::Default { name, line, everyone } => format!(
                "default\t{name}\t{}\t{}\n",
                line.as_deref().unwrap_or("-"),
                whose(*everyone)
            ),
            Request::AddOns => "add-ons\n".into(),
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
            ["get", id, catalog, place, answer, rest @ ..] if rest.len() <= 1 => Ok(Request::Get {
                id: checked_id(id)?,
                catalog: if *catalog == "-" { None } else { Some(checked_catalog(catalog)?) },
                everyone: everyone(place)?,
                yes: yes(answer)?,
                sha256: rest.first().map(|hex| checked_sha256(hex)).transpose()?,
            }),
            ["search", words @ ..] if !words.is_empty() => Ok(Request::Search {
                words: words.iter().map(|w| checked_word(w)).collect::<Result<_, _>>()?,
                json: false,
            }),
            ["search-json", words @ ..] => Ok(Request::Search {
                words: words.iter().map(|w| checked_word(w)).collect::<Result<_, _>>()?,
                json: true,
            }),
            ["sheet", id, from] => Ok(Request::Sheet {
                id: checked_id(id)?,
                from: if *from == "-" { None } else { Some(checked_catalog(from)?) },
            }),
            ["catalogs"] => Ok(Request::Catalogs),
            ["add-ons"] => Ok(Request::AddOns),
            ["lines", name] => Ok(Request::Lines { name: checked_name(name)? }),
            ["default", name, line, place] => Ok(Request::Default {
                name: checked_name(name)?,
                line: if *line == "-" { None } else { Some(checked_line(line)?) },
                everyone: everyone(place)?,
            }),
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
            ["hold", id, place, state, by, rest @ ..] => Ok(Request::Hold {
                id: checked_id(id)?,
                everyone: everyone(place)?,
                on: on(state)?,
                by: by.parse().map_err(|_| "who asked is not a user ID".to_string())?,
                why: match rest {
                    [] => None,
                    [why] if why_ok(why) => Some((*why).into()),
                    _ => return Err("the reason is not one line of 1 to 200 characters".into()),
                },
            }),
            ["tonight", state, answer, by, ids @ ..] => Ok(Request::Tonight {
                ids: ids.iter().map(|id| checked_id(id)).collect::<Result<_, _>>()?,
                on: on(state)?,
                yes: yes(answer)?,
                by: by.parse().map_err(|_| "who asked is not a user ID".to_string())?,
            }),
            ["automatic", place, state] => {
                Ok(Request::Automatic { everyone: everyone(place)?, on: on(state)? })
            }
            ["automatic"] => Ok(Request::AutomaticForEveryone),
            ["history", id] => {
                Ok(Request::History { id: if *id == "-" { None } else { Some(checked_id(id)?) } })
            }
            ["list"] => Ok(Request::List),
            ["info", id] => Ok(Request::Info { id: checked_id(id)?, json: false }),
            ["info", id, "json"] => Ok(Request::Info { id: checked_id(id)?, json: true }),
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
    /// What its catalog's list of flaws says of it.
    pub flags: Flags,
    /// The catalog it is bound to, none for one that updates from its own
    /// feed or from none.
    pub catalog: Option<String>,
    /// Whether it is held at its version.
    pub held: bool,
    /// While it is held, since when, in seconds since 1970.
    pub held_since: Option<u64>,
    /// While it is held, who held it: the user ID the newest event of its
    /// hold names.
    pub held_by: Option<u32>,
    /// While it is held, the reason kept with it, if one was.
    pub held_reason: Option<String>,
    /// The newest version the catalog listed when bundled last read it,
    /// when that is newer than this one.
    pub update: Option<String>,
    /// While its update is asked for tonight, what was asked.
    pub tonight: Option<Tonight>,
    /// While a catalog's or its feed's withdrawal applies to this version
    /// (ADR 0593), what it says. Boxed, so that a listed bundle is not
    /// the largest answer by far.
    pub withdrawn: Option<Box<WithdrawnBy>>,
}

/// A withdrawal that applies to an installed version, as `list` says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawnBy {
    pub source: Source,
    pub kind: Kind,
    /// Since when, as the source says, in seconds since 1970.
    pub since: u64,
    /// Why, as the source says: one line.
    pub reason: String,
    /// Whether it is stopped: a malicious version stays installed, but
    /// cannot run.
    pub stopped: bool,
}

impl WithdrawnBy {
    /// What `bundle list` and `bundle info` say of bundle `id` in words.
    #[must_use]
    pub fn line(&self, id: &str) -> String {
        let who = self.source.words();
        if self.stopped {
            format!(
                "stopped by {who}: {}; remove it, or `bundle update {id}` takes a fixed version",
                self.reason
            )
        } else {
            format!("withdrawn by {who}: {}", self.reason)
        }
    }
}

/// An update asked for tonight's window, as `list` says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tonight {
    /// The version it installs, which was the newest when it was asked.
    pub to: String,
    /// When it was asked, in milliseconds since 1970.
    pub asked_ms: u64,
    /// The user ID of who asked.
    pub by: u32,
    /// When a window first passed without it, if one did.
    pub missed_ms: Option<u64>,
    /// When the window it installs in starts, in milliseconds since 1970:
    /// now, in the window.
    pub starts_ms: u64,
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

/// A command of a line's add-ons, as `run` finds it (ADR 0388): the
/// add-ons' place, the line, and the command's path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddOn {
    pub place: Place,
    pub id: String,
    pub path: String,
    /// The line is a Linux one, whose add-ons run in its Linux view (ADR
    /// 0625).
    pub linux: bool,
}

/// The lines of an ID installed for the asker or for everyone, to choose
/// the one to run from (ADR 0387).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lines {
    /// The ID.
    pub id: String,
    /// Its lines, oldest first.
    pub lines: Vec<String>,
    /// The files a project keeps that say which it wants, of every line.
    pub files: Vec<String>,
    /// The asker's default line, and everyone's.
    pub mine: Option<String>,
    pub everyone: Option<String>,
}

/// A bundle a catalog lists, as `search` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub catalog: String,
    pub id: String,
    pub version: String,
    /// Its name, as the catalog gives it, shown.
    pub name: String,
    /// What the catalog's list of flaws says of it.
    pub flags: Flags,
}

/// What a catalog's list of known flaws says of a version of a bundle,
/// in short (ADR 0389).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flags {
    /// Nothing: it is from no catalog, or its catalog's list does not
    /// list it, or there is no list that is good.
    #[default]
    Unknown,
    /// The list knows of no flaw in it.
    None,
    /// It knows of `count`, the worst of them this bad.
    Known { count: u32, worst: Severity },
}

impl Flags {
    /// As a word: `unknown`, `none`, or `COUNT-WORST`, such as `2-high`.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            Flags::Unknown => "unknown".into(),
            Flags::None => "none".into(),
            Flags::Known { count, worst } => format!("{count}-{}", worst.word()),
        }
    }

    /// Reads [`Flags::word`].
    #[must_use]
    pub fn from_word(word: &str) -> Option<Flags> {
        match word {
            "unknown" => Some(Flags::Unknown),
            "none" => Some(Flags::None),
            _ => {
                let (count, worst) = word.split_once('-')?;
                let count = count.parse().ok().filter(|&c| c > 0)?;
                Some(Flags::Known { count, worst: Severity::from_word(worst)? })
            }
        }
    }

    /// As `bundle list` and `bundle search` show it: `-`, `none`, or
    /// such as `2, high`.
    #[must_use]
    pub fn shown(self) -> String {
        match self {
            Flags::Unknown => "-".into(),
            Flags::None => "none".into(),
            Flags::Known { count, worst } => format!("{count}, {}", worst.word()),
        }
    }
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
    /// The version installed, and the newest its catalog, or its own
    /// feed, lists.
    pub from: String,
    pub to: String,
    /// Its catalog, or none for its own feed (ADR 0390).
    pub catalog: Option<String>,
    /// Whether it is held at its version.
    pub held: bool,
    /// While it is held, the reason kept with the hold, if one was.
    pub held_reason: Option<String>,
    /// Why the newest version was built again, when it is a rebuild and
    /// its catalog says.
    pub rebuild_reason: Option<String>,
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
    /// A JSON document, the answer to a request that asks for one.
    Document(String),
    /// The bundle to run.
    Found(Found),
    /// The lines to choose the bundle to run from.
    Lines(Lines),
    /// The add-on command to run.
    AddOn(AddOn),
    /// What the asker said of one of its permissions.
    Granted(Grant),
    /// A permission the person took away, on a `watch` connection.
    Revoked(String),
    /// Why the app's version was stopped, on a `watch` connection: a
    /// catalog or its feed withdrew it as malicious (ADR 0593), and the
    /// run ends.
    Withdrawn(String),
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
            Answer::Document(text) => format!("document {}\n{text}", text.len()).into_bytes(),
            Answer::Listed(l) => format!(
                "listed\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                l.place,
                l.id,
                one_line(&l.version),
                l.key.encoded(),
                l.when,
                l.flags.word(),
                l.catalog.as_deref().unwrap_or("-"),
                if l.held { "held" } else { "free" },
                l.update.as_deref().map_or_else(|| "-".into(), one_line),
                l.held_since.map_or_else(|| "-".into(), |since| since.to_string()),
                l.held_by.map_or_else(|| "-".into(), |by| by.to_string()),
                // Empty for none: a reason may be any one line, `-` too.
                l.held_reason.as_deref().map_or_else(String::new, one_line),
                // Five words for what was asked for tonight, a dash each for none.
                l.tonight.as_ref().map_or_else(|| "-".into(), |t| one_line(&t.to)),
                l.tonight.as_ref().map_or_else(|| "-".into(), |t| t.asked_ms.to_string()),
                l.tonight.as_ref().map_or_else(|| "-".into(), |t| t.by.to_string()),
                l.tonight
                    .as_ref()
                    .and_then(|t| t.missed_ms)
                    .map_or_else(|| "-".into(), |ms| ms.to_string()),
                l.tonight.as_ref().map_or_else(|| "-".into(), |t| t.starts_ms.to_string()),
                // Five words for the withdrawal that applies, a dash each for none.
                l.withdrawn.as_ref().map_or_else(|| "-".into(), |w| w.source.to_string()),
                l.withdrawn.as_ref().map_or_else(|| "-".into(), |w| w.kind.word().to_string()),
                l.withdrawn.as_ref().map_or_else(|| "-".into(), |w| w.since.to_string()),
                l.withdrawn.as_ref().map_or_else(|| "-".into(), |w| w.stopped.to_string()),
                l.withdrawn.as_ref().map_or_else(|| "-".into(), |w| one_line(&w.reason)),
                one_line(&l.name)
            )
            .into_bytes(),
            Answer::Found(f) => {
                format!("found\t{}\t{}\t{}\t{}\n", f.place, f.id, f.key.encoded(), f.list)
                    .into_bytes()
            }
            Answer::Lines(l) => format!(
                "lines\t{}\t{}\t{}\t{}\t{}\n",
                l.id,
                word_list(&l.lines),
                if l.files.is_empty() { "-".into() } else { l.files.join("/") },
                l.mine.as_deref().unwrap_or("-"),
                l.everyone.as_deref().unwrap_or("-")
            )
            .into_bytes(),
            Answer::AddOn(a) => {
                let runtime = if a.linux { "linux" } else { "native" };
                format!("add-on\t{}\t{}\t{}\t{runtime}\n", a.place, a.id, a.path).into_bytes()
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
            Answer::Withdrawn(why) => format!("withdrawn\t{}\n", one_line(why)).into_bytes(),
            Answer::Hit(h) => format!(
                "hit\t{}\t{}\t{}\t{}\t{}\n",
                h.catalog,
                h.id,
                one_line(&h.version),
                h.flags.word(),
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
                "available\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                a.place,
                a.id,
                one_line(&a.from),
                one_line(&a.to),
                a.catalog.as_deref().unwrap_or("-"),
                if a.held { "held" } else { "free" },
                a.held_reason.as_deref().map_or_else(String::new, one_line),
                a.rebuild_reason.as_deref().map_or_else(String::new, one_line)
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
        if let Some(length) = line.strip_prefix("document ") {
            let length: usize = length.parse().map_err(|_| bad("a document without its length"))?;
            if length > SHEET_MAX {
                return Err(bad("bundled's document is too long"));
            }
            let mut text = vec![0; length];
            input.read_exact(&mut text)?;
            let text =
                String::from_utf8(text).map_err(|_| bad("bundled's document is not text"))?;
            return Ok(Some(Answer::Document(text)));
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
        if let ["lines", id, lines, files, mine, everyone] = words.as_slice() {
            let line = |word: &str| -> Result<Option<String>, io::Error> {
                if word == "-" {
                    Ok(None)
                } else {
                    checked_line(word).map(Some).map_err(|w| bad(&w))
                }
            };
            let lines = if *lines == "-" {
                Vec::new()
            } else {
                lines.split(',').map(checked_line).collect::<Result<_, _>>().map_err(|w| bad(&w))?
            };
            let files: Vec<String> = if *files == "-" {
                Vec::new()
            } else {
                files.split('/').map(String::from).collect()
            };
            if files.iter().any(|f| f.is_empty() || f == "." || f == ".." || f.len() > 64) {
                return Err(bad("a line's file"));
            }
            manifest::bundle::check_id(id).map_err(|why| bad(&why))?;
            return Ok(Some(Answer::Lines(Lines {
                id: (*id).into(),
                lines,
                files,
                mine: line(mine)?,
                everyone: line(everyone)?,
            })));
        }
        if let ["add-on", at, id, path, runtime] = words.as_slice() {
            let path_ok = path.starts_with('/')
                && path.len() <= 4096
                && path.split('/').skip(1).all(|p| !p.is_empty() && p != "." && p != "..")
                && !path.chars().any(char::is_control);
            if !path_ok {
                return Err(bad("an add-on's path"));
            }
            let linux = match *runtime {
                "native" => false,
                "linux" => true,
                _ => return Err(bad("an add-on's runtime")),
            };
            return Ok(Some(Answer::AddOn(AddOn {
                place: place(at)?,
                id: checked_id(id).map_err(|why| bad(&why))?,
                path: (*path).into(),
                linux,
            })));
        }
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
        if let ["hit", catalog, id, version, flags, name] = words.as_slice() {
            return Ok(Some(Answer::Hit(Hit {
                catalog: checked_catalog(catalog).map_err(|why| bad(&why))?,
                id: checked_id(id).map_err(|why| bad(&why))?,
                version: one_line(version),
                name: one_line(name),
                flags: Flags::from_word(flags).ok_or_else(|| bad("a hit's flaws"))?,
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
        if let ["available", at, id, from, to, catalog, held, reason, rebuilt] = words.as_slice() {
            return Ok(Some(Answer::Available(Available {
                place: place(at)?,
                id: checked_id(id).map_err(|why| bad(&why))?,
                from: one_line(from),
                to: one_line(to),
                catalog: match *catalog {
                    "-" => None,
                    name => Some(checked_catalog(name).map_err(|why| bad(&why))?),
                },
                held: match *held {
                    "held" => true,
                    "free" => false,
                    _ => return Err(bad("neither held nor free")),
                },
                held_reason: (!reason.is_empty()).then(|| one_line(reason)),
                rebuild_reason: (!rebuilt.is_empty()).then(|| one_line(rebuilt)),
            })));
        }
        if let ["withdrawn", why] = words.as_slice() {
            return Ok(Some(Answer::Withdrawn(one_line(why))));
        }
        if let ["revoked", permission] = words.as_slice() {
            let permission = checked_permission(permission).map_err(|why| bad(&why))?;
            return Ok(Some(Answer::Revoked(permission)));
        }
        if let [
            "listed",
            place_word,
            id,
            version,
            key,
            when,
            flags,
            catalog,
            held,
            update,
            since,
            by,
            reason,
            to,
            asked,
            asked_by,
            missed,
            starts,
            withdrawn_source,
            withdrawn_kind,
            withdrawn_since,
            withdrawn_stopped,
            withdrawn_reason,
            name,
        ] = words.as_slice()
        {
            let withdrawn = match *withdrawn_source {
                "-" => None,
                text => {
                    let source = match text.split_once(':') {
                        Some(("catalog", name)) => {
                            Source::Catalog(checked_catalog(name).map_err(|why| bad(&why))?)
                        }
                        Some(("feed", at)) => Source::Feed(place(at)?),
                        _ => return Err(bad("a withdrawal's source")),
                    };
                    Some(WithdrawnBy {
                        source,
                        kind: Kind::read(withdrawn_kind).map_err(|why| bad(&why))?,
                        since: withdrawn_since.parse().map_err(|_| bad("a withdrawal's time"))?,
                        reason: one_line(withdrawn_reason),
                        stopped: match *withdrawn_stopped {
                            "true" => true,
                            "false" => false,
                            _ => return Err(bad("neither true nor false")),
                        },
                    })
                }
            };
            let place = place(place_word)?;
            return Ok(Some(Answer::Listed(Listed {
                place,
                id: checked_id(id).map_err(|why| bad(&why))?,
                version: one_line(version),
                key: PublicKey::decode(key).map_err(|why| bad(&why))?,
                when: when.parse().map_err(|_| bad("a listed bundle's time"))?,
                name: one_line(name),
                flags: Flags::from_word(flags).ok_or_else(|| bad("a listed bundle's flaws"))?,
                catalog: match *catalog {
                    "-" => None,
                    name => Some(checked_catalog(name).map_err(|why| bad(&why))?),
                },
                held: match *held {
                    "held" => true,
                    "free" => false,
                    _ => return Err(bad("neither held nor free")),
                },
                update: (*update != "-").then(|| one_line(update)),
                held_since: match *since {
                    "-" => None,
                    since => Some(since.parse().map_err(|_| bad("a hold's time"))?),
                },
                held_by: match *by {
                    "-" => None,
                    by => Some(by.parse().map_err(|_| bad("who held a bundle"))?),
                },
                held_reason: (!reason.is_empty()).then(|| one_line(reason)),
                tonight: match *to {
                    "-" => None,
                    to => Some(Tonight {
                        to: one_line(to),
                        asked_ms: asked.parse().map_err(|_| bad("when it was asked"))?,
                        by: asked_by.parse().map_err(|_| bad("who asked"))?,
                        missed_ms: match *missed {
                            "-" => None,
                            ms => Some(ms.parse().map_err(|_| bad("when a window passed"))?),
                        },
                        starts_ms: starts.parse().map_err(|_| bad("when a window starts"))?,
                    }),
                },
                withdrawn: withdrawn.map(Box::new),
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
            Request::Info { id: "org.example.notes".into(), json: false },
            Request::Info { id: "org.example.notes".into(), json: true },
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
                sha256: None,
            },
            Request::Get {
                id: "io.github.burntsushi.ripgrep".into(),
                catalog: Some("dukovo".into()),
                everyone: true,
                yes: true,
                sha256: None,
            },
            Request::Get {
                id: "org.nginx.nginx".into(),
                catalog: None,
                everyone: true,
                yes: true,
                sha256: Some("ab".repeat(32)),
            },
            Request::Search { words: vec!["rip".into(), "grep tool".into()], json: false },
            Request::Search { words: vec!["rip".into(), "grep tool".into()], json: true },
            Request::Search { words: Vec::new(), json: true },
            Request::Sheet { id: "org.nginx.nginx".into(), from: None },
            Request::Sheet { id: "org.nodejs.node@22".into(), from: Some("dukovo".into()) },
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
            Request::Hold {
                id: "org.example.notes".into(),
                everyone: true,
                on: true,
                by: 0,
                why: None,
            },
            Request::Hold {
                id: "org.example.notes".into(),
                everyone: false,
                on: false,
                by: 1000,
                why: None,
            },
            Request::Hold {
                id: "org.example.notes".into(),
                everyone: true,
                on: true,
                by: 1000,
                why: Some("app tests use this version".into()),
            },
            Request::Hold {
                id: "org.example.notes".into(),
                everyone: true,
                on: true,
                by: 1000,
                why: Some("-".into()),
            },
            Request::Tonight { ids: Vec::new(), on: true, yes: true, by: 1000 },
            Request::Tonight {
                ids: vec!["org.example.notes".into(), "org.example.tea@2".into()],
                on: true,
                yes: false,
                by: 1000,
            },
            Request::Tonight {
                ids: vec!["org.example.notes".into()],
                on: false,
                yes: false,
                by: 0,
            },
            Request::Automatic { everyone: false, on: true },
            Request::History { id: None },
            Request::History { id: Some("org.example.notes".into()) },
            Request::Run { find: Find::Id("org.nodejs.node@22".into()) },
            Request::Rollback { id: "org.python.python@3.13".into(), everyone: true },
            Request::Lines { name: "node".into() },
            Request::Lines { name: "org.nodejs.node".into() },
            Request::Default { name: "node".into(), line: Some("22".into()), everyone: false },
            Request::Default { name: "org.nodejs.node".into(), line: None, everyone: true },
            Request::AddOns,
            Request::AutomaticForEveryone,
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
            "get\torg.nginx.nginx\t-\teveryone\tyes\tabcd",
            "get\torg.nginx.nginx\t-\teveryone\tyes\t",
            "search",
            "search\t",
            "search-json\t",
            "search-json\trip\t",
            "sheet",
            "sheet\torg.nginx.nginx",
            "sheet\tNginx\t-",
            "sheet\torg.nginx.nginx\tDukovo",
            "sheet\torg.nginx.nginx\t-\textra",
            "catalog-add\tfeed\tftp://example.org/",
            "catalog-remove\t../x",
            "update\tme\tcheck",
            "update\tme\tmaybe\task",
            "update\tme\tapply\task\tNotes",
            "rollback\torg.example.notes",
            "hold\torg.example.notes\tme\tsometimes\t1000",
            "hold\torg.example.notes\tme\ton",
            "hold\torg.example.notes\tme\ton\tsomeone",
            "hold\torg.example.notes\tme\ton\t1000\t",
            "hold\torg.example.notes\tme\ton\t1000\ttwo\ttabs",
            "automatic\tall\ton",
            "tonight\ton\tyes",
            "tonight\ton\tyes\tme",
            "tonight\tmaybe\tyes\t1000",
            "tonight\ton\tsure\t1000",
            "tonight\ton\task\t1000\tNotes",
            "history",
        ] {
            assert!(Request::read(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_lines_an_older_bundle_sends_still_read() {
        // An install by ID names no SHA-256, and a later one may; it is read
        // in lowercase.
        assert_eq!(
            Request::read("get\torg.nginx.nginx\t-\tme\task"),
            Ok(Request::Get {
                id: "org.nginx.nginx".into(),
                catalog: None,
                everyone: false,
                yes: false,
                sha256: None,
            })
        );
        let upper = format!("get\torg.nginx.nginx\t-\tme\task\t{}", "AB".repeat(32));
        let Ok(Request::Get { sha256, .. }) = Request::read(&upper) else { panic!("{upper}") };
        assert_eq!(sha256, Some("ab".repeat(32)));
        // The words of a search with no `--json`, and a search of no words
        // that only `--json` makes.
        assert_eq!(
            Request::read("search\trip"),
            Ok(Request::Search { words: vec!["rip".into()], json: false })
        );
        assert_eq!(Request::read("search-json"), Ok(Request::Search { words: vec![], json: true }));
        assert_eq!(Request::read("search"), Err("not a request bundled knows".into()));
    }

    #[test]
    fn a_reason_is_one_line_of_at_most_200_characters() {
        assert!(why_ok("Waiting for the nginx fix"));
        assert!(why_ok(&"é".repeat(200)));
        for bad in ["", "two\nlines", "a\ttab", "bell\x07", "del\x7f"] {
            assert!(!why_ok(bad), "{bad:?}");
        }
        assert!(!why_ok(&"x".repeat(201)));
        // A request line of the longest reason is no longer than a line may be.
        let longest = Request::Hold {
            id: "org.example.notes".into(),
            everyone: true,
            on: true,
            by: u32::MAX,
            why: Some("é".repeat(200)),
        };
        assert!(longest.line().len() < LINE_MAX);
        assert_eq!(Request::read(longest.line().trim_end()), Ok(longest));
        let too_long = format!("hold\torg.example.notes\tme\ton\t1000\t{}", "x".repeat(201));
        assert!(Request::read(&too_long).is_err());
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
            flags: Flags::Known { count: 2, worst: Severity::High },
            catalog: Some("dukovo".into()),
            held: true,
            held_since: Some(1_793_100_000),
            held_by: Some(1000),
            held_reason: Some("app tests use this version".into()),
            update: Some("0.2.0".into()),
            tonight: Some(Tonight {
                to: "0.2.0".into(),
                asked_ms: 1_793_200_000_000,
                by: 1000,
                missed_ms: Some(1_793_250_000_000),
                starts_ms: 1_793_300_000_000,
            }),
            withdrawn: Some(Box::new(WithdrawnBy {
                source: Source::Catalog("dukovo".into()),
                kind: Kind::Malicious,
                since: 1_793_400_000,
                reason: "it sends your files away".into(),
                stopped: true,
            })),
        };
        let unbound = Listed {
            catalog: None,
            held: false,
            held_since: None,
            held_by: None,
            held_reason: None,
            update: None,
            tonight: None,
            withdrawn: None,
            ..listed.clone()
        };
        // Broken, not stopped, by its own feed, with a reason that is a dash.
        let by_feed = Listed {
            withdrawn: Some(Box::new(WithdrawnBy {
                source: Source::Feed(Place::User(1000)),
                kind: Kind::Broken,
                since: 1_793_400_001,
                reason: "-".into(),
                stopped: false,
            })),
            ..listed.clone()
        };
        // Asked, with no window missed yet.
        let fresh = Listed {
            tonight: Some(Tonight { missed_ms: None, ..listed.tonight.clone().unwrap() }),
            ..listed.clone()
        };
        // A hold with no reason, and one whose reason is a dash.
        let unsaid = Listed { held_reason: None, ..listed.clone() };
        let dash = Listed { held_reason: Some("-".into()), ..listed.clone() };
        let answers = [
            Answer::Sheet("Notes 0.1.0, for yourself\n  Signed by key RW...\n".into()),
            Answer::Question,
            Answer::Listed(listed),
            Answer::Listed(unbound),
            Answer::Listed(by_feed),
            Answer::Listed(fresh),
            Answer::Listed(unsaid),
            Answer::Listed(dash),
            Answer::Document("{\"format\":1,\"id\":\"org.example.notes\"}".into()),
            Answer::Lines(Lines {
                id: "org.nodejs.node".into(),
                lines: vec!["20".into(), "22".into()],
                files: vec![".nvmrc".into(), ".node-version".into()],
                mine: Some("20".into()),
                everyone: None,
            }),
            Answer::Lines(Lines {
                id: "org.example.tide".into(),
                lines: vec!["1".into()],
                files: Vec::new(),
                mine: None,
                everyone: Some("1".into()),
            }),
            Answer::AddOn(AddOn {
                place: Place::User(1000),
                id: "org.nodejs.node@22".into(),
                path: "/users/alice/applications/add-ons/org.nodejs.node@22/bin/tsc".into(),
                linux: true,
            }),
            Answer::AddOn(AddOn {
                place: Place::Everyone,
                id: "org.python.python@3.14".into(),
                path: "/applications/add-ons/org.python.python@3.14/bin/black".into(),
                linux: false,
            }),
            Answer::Found(Found {
                place: Place::Everyone,
                id: "org.example.notes@2".into(),
                key: key(),
                list: "cd".repeat(32),
            }),
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
            Answer::Withdrawn("catalog dukovo: it sends the keys away".into()),
            Answer::Hit(Hit {
                catalog: "dukovo".into(),
                id: "io.github.sharkdp.bat".into(),
                version: "0.26.1".into(),
                name: "bat".into(),
                flags: Flags::None,
            }),
            Answer::Hit(Hit {
                catalog: "dukovo".into(),
                id: "org.example.notes".into(),
                version: "1.0.0".into(),
                name: "Notes".into(),
                flags: Flags::Unknown,
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
                catalog: Some("dukovo".into()),
                held: true,
                held_reason: Some("app tests use this version".into()),
                rebuild_reason: Some("Linked with the C library fix".into()),
            }),
            Answer::Available(Available {
                place: Place::Everyone,
                id: "org.example.notes@2".into(),
                from: "2.0.0".into(),
                to: "2.1.0".into(),
                catalog: None,
                held: false,
                held_reason: None,
                rebuild_reason: None,
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
            b"available\t1000\torg.example.notes\t1\t2\tdukovo\tfree\t\n",
            b"event\t1\t1000\tNotes\tupdated\t-\t-\t1\t-\n",
            b"add-on\t1000\torg.nodejs.node@22\tbin/tsc\n",
            b"add-on\t1000\torg.nodejs.node@22\t/users/alice/../bob/tsc\n",
            b"add-on\t1000\tnode@22\t/users/alice/tsc\n",
            b"hit\tdukovo\torg.example.notes\t1\t0-high\tNotes\n",
            b"hit\tdukovo\torg.example.notes\t1\t2-dire\tNotes\n",
            b"hit\tdukovo\torg.example.notes\t1\tNotes\n",
        ] {
            assert!(Answer::read(&mut io::Cursor::new(bad.to_vec())).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_listed_bundles_withdrawal_is_read_strictly() {
        let row = Answer::Listed(Listed {
            place: Place::Everyone,
            id: "org.example.notes".into(),
            version: "0.1.0".into(),
            key: key(),
            when: 1_793_000_000,
            name: "Notes".into(),
            flags: Flags::None,
            catalog: None,
            held: false,
            held_since: None,
            held_by: None,
            held_reason: None,
            update: None,
            tonight: None,
            withdrawn: None,
        })
        .bytes();
        let row = String::from_utf8(row).unwrap();
        let words: Vec<&str> = row.trim_end().split('\t').collect();
        // The word before the name is the withdrawal's reason, and four
        // more come before that: source, kind, time and whether stopped.
        let at = words.len() - 6;
        let sound = ["catalog:dukovo", "broken", "5", "false"];
        let with = |change: [&str; 4]| {
            let mut changed = words.clone();
            changed[at..at + 4].copy_from_slice(&change);
            format!("{}\n", changed.join("\t")).into_bytes()
        };
        assert!(Answer::read(&mut io::Cursor::new(with(sound))).is_ok());
        for bad in [
            ["depot:x", "-", "-", "-"],
            ["catalog:Bad Name", "broken", "5", "false"],
            ["catalog:dukovo", "evil", "5", "false"],
            ["catalog:dukovo", "broken", "soon", "false"],
            ["catalog:dukovo", "broken", "5", "maybe"],
            ["feed:someone", "broken", "5", "false"],
        ] {
            let line = with(bad);
            assert!(Answer::read(&mut io::Cursor::new(line.clone())).is_err(), "{line:?}");
        }
    }

    #[test]
    fn a_withdrawal_says_what_the_person_may_do() {
        let by = |source: Source, stopped: bool| WithdrawnBy {
            source,
            kind: if stopped { Kind::Malicious } else { Kind::Broken },
            since: 5,
            reason: "it crashes".into(),
            stopped,
        };
        let id = "org.example.notes";
        assert_eq!(
            by(Source::Catalog("dukovo".into()), false).line(id),
            "withdrawn by catalog dukovo: it crashes"
        );
        assert_eq!(
            by(Source::Catalog("dukovo".into()), true).line(id),
            "stopped by catalog dukovo: it crashes; remove it, or `bundle update \
             org.example.notes` takes a fixed version"
        );
        assert_eq!(
            by(Source::Feed(Place::Everyone), false).line(id),
            "withdrawn by its publisher's feed: it crashes"
        );
    }
}
