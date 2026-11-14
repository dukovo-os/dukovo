// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The signatures troubleshoot(8) matches (ADR 0474), troubleshoot-
//! signatures(5): TOML data, with nothing in it that runs. The release
//! ships a file, and the daily update check may keep a newer one in the
//! state, which [`keep_in`] reads from the network's bytes: it is the
//! one reader of data that came from outside the system, so it is plain
//! and strict. Each limit is checked before what it limits is built, a
//! file that is not whole and right in every part it reads is refused
//! whole, and a file is never taken whose version is not newer than the
//! one shipped and the one kept.
//!
//! What it does not refuse is what it cannot read yet: a condition whose
//! kind, modifier or value this release does not know makes its
//! signature one this release skips, counted in [`Signatures::skipped`],
//! so a file from a newer release still helps an older system.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use super::glob::Glob;
use crate::sysupdate::starts::{self, Outcome};

/// The format this release reads.
pub const FORMAT: i64 = 1;

/// The most bytes a file may have, checked before it is read.
pub const MAX_BYTES: usize = 256 * 1024;

/// The most signatures a file may have.
pub const MAX_SIGNATURES: usize = 512;

/// The most conditions a signature may have.
pub const MAX_CONDITIONS: usize = 8;

/// The most conditions on the log all of a file's signatures may have
/// together: each is matched against every line of the start's log, which
/// is the dearest thing a match does.
pub const MAX_LOG_CONDITIONS: usize = 64;

/// The most bytes a cause or a fix may have.
pub const MAX_TEXT: usize = 500;

/// The most bytes a service's name or a filesystem's path may have.
const MAX_NAME: usize = 64;
const MAX_PATH: usize = 200;

/// The file the release ships.
pub const SHIPPED: &str = "/system/resources/troubleshoot/signatures.toml";

/// Where a newer file is kept, and the list of those refused.
pub const KEPT_DIR: &str = "/system/state/troubleshoot";
const KEPT_FILE: &str = "signatures.toml";
const REFUSED_FILE: &str = "refused";

/// How many refused files are remembered.
const REFUSED_KEPT: usize = 64;

/// The kinds of item a start's record lists, which `changed` may name.
const CHANGED: [&str; 8] =
    ["system", "kernel", "firmware", "microcode", "options", "card", "root", "app"];

/// The keys that make a condition of a kind.
const KINDS: [&str; 5] = ["log", "start", "changed", "failed", "full"];

/// A set of signatures.
#[derive(Debug)]
pub struct Signatures {
    /// The file's version: a newer one replaces an older.
    pub version: u64,
    /// The signatures this release reads.
    pub signatures: Vec<Signature>,
    /// How many more the file has, with a condition this release cannot
    /// read.
    pub skipped: usize,
}

impl Signatures {
    /// The paths whose fullness the signatures ask about, each once.
    #[must_use]
    pub fn full_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .signatures
            .iter()
            .flat_map(|signature| &signature.conditions)
            .filter_map(|condition| match condition {
                Condition::Full { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// One signature: a cause and its fix, when all its conditions hold.
#[derive(Debug)]
pub struct Signature {
    /// The error ID, such as `BOOT-104`, whose page tells more.
    pub id: String,
    /// What is probably wrong.
    pub cause: String,
    /// What to do.
    pub fix: String,
    /// What must all hold.
    pub conditions: Vec<Condition>,
}

/// One thing that holds of a start, or does not.
#[derive(Debug)]
pub enum Condition {
    /// A line of the start's log, matched by a glob, of one service (or
    /// `kernel`), at a level or a more urgent one.
    Log { glob: Glob, service: Option<String>, level: Option<Level> },
    /// The start ended so, with this error ID if one is named.
    Start { outcome: Outcome, error: Option<String> },
    /// An item of this kind differs from the last good start's.
    Changed(String),
    /// A service failed.
    Failed(Failed),
    /// The filesystem at `path` is at least `percent` full.
    Full { path: String, percent: u8 },
}

/// The levels a log condition may name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

impl Level {
    /// The least urgent level's number a line may have, as logd numbers them.
    #[must_use]
    pub fn limit(self) -> u8 {
        match self {
            Level::Error => 3,
            Level::Warning => 4,
        }
    }
}

/// Which service failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Failed {
    /// This one.
    Service(String),
    /// Any.
    Any,
    /// One the health check needs.
    Essential,
}

/// Reads the signatures `bytes` hold.
///
/// # Errors
///
/// What is wrong with them, in a line. The file is then refused whole.
pub fn parse(bytes: &[u8]) -> Result<Signatures, String> {
    if bytes.len() > MAX_BYTES {
        return Err(format!("it is {} bytes, and at most {MAX_BYTES} are allowed", bytes.len()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_string())?;
    let table = text.parse::<Table>().map_err(|err| {
        let line = err.span().map_or(1, |span| text[..span.start].matches('\n').count() + 1);
        format!("line {line}: {}", err.message())
    })?;
    if let Some(key) =
        table.keys().find(|key| !["format", "version", "signature"].contains(&key.as_str()))
    {
        return Err(format!("`{key}` is no part of format {FORMAT}"));
    }
    match table.get("format") {
        Some(Value::Integer(FORMAT)) => {}
        Some(Value::Integer(other)) => {
            return Err(format!("its format is {other}, and this release reads {FORMAT}"));
        }
        _ => return Err(format!("it does not say `format = {FORMAT}`")),
    }
    let Some(Value::Integer(version)) = table.get("version") else {
        return Err("it has no `version`, a whole number".into());
    };
    let version = u64::try_from(*version).map_err(|_| "its version is negative".to_string())?;
    let list = match table.get("signature") {
        None => &Vec::new(),
        Some(Value::Array(list)) => list,
        Some(_) => return Err("`signature` is not a list of [[signature]] tables".into()),
    };
    if list.len() > MAX_SIGNATURES {
        return Err(format!(
            "it has {} signatures, and at most {MAX_SIGNATURES} are allowed",
            list.len()
        ));
    }
    // Counted by their key, whether or not this release reads the rest of
    // the signature, so that every release refuses the same files.
    let on_the_log = list
        .iter()
        .filter_map(Value::as_table)
        .filter_map(|entry| entry.get("match")?.as_array())
        .flatten()
        .filter(|condition| condition.as_table().is_some_and(|table| table.contains_key("log")))
        .count();
    if on_the_log > MAX_LOG_CONDITIONS {
        return Err(format!(
            "it has {on_the_log} conditions on the log, and at most {MAX_LOG_CONDITIONS} are \
             allowed"
        ));
    }
    let mut found = Signatures { version, signatures: Vec::new(), skipped: 0 };
    for (at, entry) in list.iter().enumerate() {
        let Value::Table(entry) = entry else {
            return Err(format!("signature {} is not a table", at + 1));
        };
        match signature(entry).map_err(|why| format!("signature {}: {why}", at + 1))? {
            Some(read) => found.signatures.push(read),
            None => found.skipped += 1,
        }
    }
    Ok(found)
}

/// The signature `table` is, or `None` if one of its conditions is of a
/// kind this release does not know.
fn signature(table: &Table) -> Result<Option<Signature>, String> {
    if let Some(key) =
        table.keys().find(|key| !["id", "cause", "fix", "match"].contains(&key.as_str()))
    {
        return Err(format!("`{key}` is no part of a signature"));
    }
    let id = string(table, "id")?.ok_or("it has no `id`")?;
    if !starts::is_error_id(id) {
        return Err(format!("the id {id:?} is not an error ID, such as BOOT-104"));
    }
    let id = id.to_string();
    let cause = line(table, "cause")?;
    let fix = line(table, "fix")?;
    let Some(Value::Array(list)) = table.get("match") else {
        return Err(format!("{id}: `match` is not a list of conditions"));
    };
    if list.is_empty() || list.len() > MAX_CONDITIONS {
        return Err(format!(
            "{id}: it has {} conditions, and from 1 to {MAX_CONDITIONS} are allowed",
            list.len()
        ));
    }
    let mut conditions = Vec::new();
    let mut known = true;
    for entry in list {
        let Value::Table(entry) = entry else {
            return Err(format!("{id}: a condition is not a table"));
        };
        match condition(entry).map_err(|why| format!("{id}: {why}"))? {
            Some(read) => conditions.push(read),
            None => known = false,
        }
    }
    Ok(known.then_some(Signature { id, cause, fix, conditions }))
}

/// The text of `key`, which must be a string if it is there.
fn string<'a>(table: &'a Table, key: &str) -> Result<Option<&'a str>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("`{key}` is not a string")),
    }
}

/// The text of `key`, which must be there, one line of readable text of
/// up to [`MAX_TEXT`] bytes.
fn line(table: &Table, key: &str) -> Result<String, String> {
    let text = string(table, key)?.ok_or(format!("it has no `{key}`"))?;
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err(format!(
            "`{key}` is {} bytes, and from 1 to {MAX_TEXT} are allowed",
            text.len()
        ));
    }
    if !readable(text) {
        return Err(format!("`{key}` has a character that is not for reading"));
    }
    Ok(text.to_string())
}

/// Whether `text` has only characters a person reads: no control
/// characters, which could move a terminal's cursor or change its
/// colours, and none that reverse or hide what follows.
fn readable(text: &str) -> bool {
    !text.chars().any(|c| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
    })
}

/// Whether `text` can be a service's name.
fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NAME
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

/// The service name in `key`, if it is there.
fn name<'a>(table: &'a Table, key: &str) -> Result<Option<&'a str>, String> {
    let found = string(table, key)?;
    match found {
        Some(text) if !is_name(text) => Err(format!("`{key}` {text:?} is not a service's name")),
        _ => Ok(found),
    }
}

/// The condition `table` is, or `None` if it is one this release cannot
/// read.
fn condition(table: &Table) -> Result<Option<Condition>, String> {
    let kinds: Vec<&str> = KINDS.into_iter().filter(|kind| table.contains_key(*kind)).collect();
    let kind = match kinds.as_slice() {
        [] => return Ok(None),
        [kind] => *kind,
        [first, second, ..] => {
            return Err(format!("a condition has both `{first}` and `{second}`"));
        }
    };
    // What is read of it is checked first, so that a wrong type is refused
    // however the rest reads.
    let only = |keys: &[&str]| table.keys().all(|key| keys.contains(&key.as_str()));
    let value = string(table, kind)?;
    let read = match kind {
        "log" => {
            let pattern = value.unwrap_or_default();
            let glob = Glob::new(pattern).map_err(|why| format!("the glob {pattern:?}: {why}"))?;
            let service = name(table, "service")?.map(str::to_string);
            let level = match string(table, "level")? {
                None => Some(None),
                Some("error") => Some(Some(Level::Error)),
                Some("warning") => Some(Some(Level::Warning)),
                Some(_) => None,
            };
            level.filter(|_| only(&["log", "service", "level"])).map(|level| Condition::Log {
                glob,
                service,
                level,
            })
        }
        "start" => {
            let error = string(table, "error")?;
            if let Some(id) = error.filter(|id| !starts::is_error_id(id)) {
                return Err(format!("the error {id:?} is not an error ID, such as BOOT-102"));
            }
            let outcome = match value {
                Some("failed") => Some(Outcome::Failed),
                Some("unfinished") => Some(Outcome::Unfinished),
                _ => None,
            };
            outcome
                .filter(|_| only(&["start", "error"]))
                .map(|outcome| Condition::Start { outcome, error: error.map(str::to_string) })
        }
        "changed" => value
            .filter(|kind| CHANGED.contains(kind) && only(&["changed"]))
            .map(|kind| Condition::Changed(kind.to_string())),
        "failed" => {
            let failed = match value.unwrap_or_default() {
                "*" => Failed::Any,
                "essential" => Failed::Essential,
                service if is_name(service) => Failed::Service(service.to_string()),
                other => return Err(format!("`failed` {other:?} is not a service's name")),
            };
            only(&["failed"]).then_some(Condition::Failed(failed))
        }
        _ => {
            let path = value.unwrap_or_default();
            if !path.starts_with('/') || path.len() > MAX_PATH || !readable(path) {
                return Err(format!("`full` {path:?} is not a path of up to {MAX_PATH} bytes"));
            }
            let percent = match table.get("percent") {
                Some(Value::Integer(percent)) => {
                    u8::try_from(*percent).ok().filter(|p| (1..=100).contains(p))
                }
                _ => None,
            };
            let percent = percent.ok_or("`full` needs a `percent` from 1 to 100")?;
            only(&["full", "percent"]).then(|| Condition::Full { path: path.to_string(), percent })
        }
    };
    Ok(read)
}

/// Where a set of signatures came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The release's file.
    Shipped,
    /// The newer file the update check kept.
    Kept,
}

/// The signatures in use.
#[derive(Debug)]
pub struct Loaded {
    pub signatures: Signatures,
    pub source: Source,
    /// What was wrong with a file that was left out, a line each.
    pub notes: Vec<String>,
}

/// The file at `path`, up to one byte more than [`MAX_BYTES`], so that a
/// large file is not read whole.
fn read_limited(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The signatures in use: the kept file's if its version is newer than
/// the shipped one's, else the shipped.
#[must_use]
pub fn load() -> Loaded {
    load_in(Path::new(SHIPPED), Path::new(KEPT_DIR))
}

/// [`load`], with the shipped file at `shipped` and the kept one in `dir`.
#[must_use]
pub fn load_in(shipped: &Path, dir: &Path) -> Loaded {
    let mut notes = Vec::new();
    let mut read = |path: &Path| match read_limited(path) {
        Ok(bytes) => {
            parse(&bytes).map_err(|why| notes.push(format!("{}: {why}", path.display()))).ok()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            notes.push(format!("{}: {err}", path.display()));
            None
        }
    };
    let ours = read(shipped);
    let kept = read(&dir.join(KEPT_FILE));
    match (ours, kept) {
        (Some(ours), Some(kept)) if kept.version > ours.version => {
            Loaded { signatures: kept, source: Source::Kept, notes }
        }
        (Some(ours), _) => Loaded { signatures: ours, source: Source::Shipped, notes },
        (None, Some(kept)) => Loaded { signatures: kept, source: Source::Kept, notes },
        (None, None) => {
            let none = Signatures { version: 0, signatures: Vec::new(), skipped: 0 };
            Loaded { signatures: none, source: Source::Shipped, notes }
        }
    }
}

/// What became of a file offered to [`keep_in`].
#[derive(Debug, PartialEq, Eq)]
pub enum Kept {
    /// It is kept now, and is in use.
    New(u64),
    /// Its version is not newer than that of the file in use.
    NotNewer { offered: u64, have: u64 },
    /// It is refused, for this reason; `first` is whether it was
    /// refused before (the same bytes), which is logged once.
    Refused { why: String, first: bool },
}

/// Keeps the file `bytes` in `dir` if it reads, and its version is newer
/// than the version of the file at `shipped` and of the one kept in
/// `dir`; else leaves what is there as it is.
///
/// # Errors
///
/// If it cannot be written.
pub fn keep_in(shipped: &Path, dir: &Path, bytes: &[u8]) -> Result<Kept, String> {
    let offered = match parse(bytes) {
        Ok(offered) => offered,
        Err(why) => return Ok(Kept::Refused { first: remember_refused(dir, bytes)?, why }),
    };
    let have = load_in(shipped, dir).signatures.version;
    if offered.version <= have {
        return Ok(Kept::NotNewer { offered: offered.version, have });
    }
    let work = |err: std::io::Error| format!("{}: {err}", dir.display());
    std::fs::create_dir_all(dir).map_err(work)?;
    let partial = dir.join(format!("{KEPT_FILE}.partial"));
    std::fs::write(&partial, bytes).map_err(work)?;
    std::fs::rename(&partial, dir.join(KEPT_FILE)).map_err(work)?;
    Ok(Kept::New(offered.version))
}

/// Keeps the signatures `bytes` that the update server or an update file
/// offered, if they are newer than those in use; what is worth saying
/// of it, if anything: that they were kept, that they are refused (the
/// first time), or that they could not be kept. An older file, and one
/// refused before, say nothing.
#[must_use]
pub fn offered(bytes: &[u8]) -> Option<String> {
    match keep(bytes) {
        Ok(Kept::New(version)) => {
            Some(format!("kept the troubleshoot signatures of version {version}"))
        }
        Ok(Kept::Refused { why, first: true }) => {
            Some(format!("refused the troubleshoot signatures it was offered: {why}"))
        }
        Ok(Kept::NotNewer { .. } | Kept::Refused { .. }) => None,
        Err(why) => Some(format!("cannot keep the troubleshoot signatures: {why}")),
    }
}

/// [`keep_in`], for the system's files.
///
/// # Errors
///
/// If it cannot be written.
pub fn keep(bytes: &[u8]) -> Result<Kept, String> {
    keep_in(Path::new(SHIPPED), Path::new(KEPT_DIR), bytes)
}

/// Notes that the file `bytes` was refused, in `dir`: whether it is the
/// first time.
fn remember_refused(dir: &Path, bytes: &[u8]) -> Result<bool, String> {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let hash = digest.as_ref().iter().fold(String::new(), |mut hash, byte| {
        let _ = write!(hash, "{byte:02x}");
        hash
    });
    let list: PathBuf = dir.join(REFUSED_FILE);
    let before = std::fs::read_to_string(&list).unwrap_or_default();
    let mut hashes: Vec<&str> = before.lines().collect();
    if hashes.contains(&hash.as_str()) {
        return Ok(false);
    }
    hashes.push(&hash);
    let from = hashes.len().saturating_sub(REFUSED_KEPT);
    let work = |err: std::io::Error| format!("{}: {err}", dir.display());
    std::fs::create_dir_all(dir).map_err(work)?;
    std::fs::write(&list, format!("{}\n", hashes[from..].join("\n"))).map_err(work)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "format = 1\nversion = 5\n";

    const FULL: &str = r#"
[[signature]]
id = "BOOT-104"
cause = "The disk that holds the system is full."
fix = "Free some space."
match = [
  { full = "/system/state", percent = 98 },
  { log = "*No space left on device*", service = "logd", level = "error" },
  { start = "failed", error = "BOOT-102" },
  { changed = "options" },
  { failed = "essential" },
]
"#;

    /// A signature with the conditions `conditions`, in TOML.
    fn with(conditions: &str) -> String {
        format!(
            "{HEADER}[[signature]]\nid = \"TST-001\"\ncause = \"c\"\nfix = \"f\"\nmatch = [{conditions}]\n"
        )
    }

    fn refused(text: &str, said: &str) {
        let why = parse(text.as_bytes()).expect_err(text);
        assert!(why.contains(said), "{why:?} does not say {said:?}");
    }

    /// A file of exactly `size` bytes that reads: the header and a comment.
    fn of_size(size: usize) -> String {
        let mut text = HEADER.to_string();
        text.push('#');
        text.extend(std::iter::repeat_n('x', size - text.len()));
        text
    }

    #[test]
    fn reads_every_kind_of_condition() {
        let read = parse(format!("{HEADER}{FULL}").as_bytes()).unwrap();
        assert_eq!((read.version, read.skipped, read.signatures.len()), (5, 0, 1));
        let signature = &read.signatures[0];
        assert_eq!(signature.id, "BOOT-104");
        assert_eq!(signature.cause, "The disk that holds the system is full.");
        assert_eq!(signature.fix, "Free some space.");
        let [full, log, start, changed, failed] = signature.conditions.as_slice() else {
            panic!("{:?}", signature.conditions);
        };
        assert!(matches!(full, Condition::Full { path, percent: 98 } if path == "/system/state"));
        assert!(
            matches!(log, Condition::Log { service: Some(s), level: Some(Level::Error), .. } if s == "logd")
        );
        assert!(
            matches!(start, Condition::Start { outcome: Outcome::Failed, error: Some(e) } if e == "BOOT-102")
        );
        assert!(matches!(changed, Condition::Changed(kind) if kind == "options"));
        assert!(matches!(failed, Condition::Failed(Failed::Essential)));
        // The other forms of the same kinds.
        let read = parse(
            with(
                r#"{ log = "a?" }, { log = "b", level = "warning" }, { log = "c", service = "kernel" },
                { start = "unfinished" }, { failed = "*" }, { failed = "sshd" }"#,
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(read.signatures[0].conditions.len(), 6);
        assert!(matches!(
            &read.signatures[0].conditions[1],
            Condition::Log { level: Some(Level::Warning), .. }
        ));
        assert!(matches!(&read.signatures[0].conditions[4], Condition::Failed(Failed::Any)));
        assert!(
            matches!(&read.signatures[0].conditions[5], Condition::Failed(Failed::Service(s)) if s == "sshd")
        );
        // A file with no signature is a file.
        assert_eq!(parse(HEADER.as_bytes()).unwrap().signatures.len(), 0);
    }

    #[test]
    fn the_paths_asked_about_are_named_once() {
        let text = with(
            r#"{ full = "/b", percent = 90 }, { full = "/a", percent = 90 }, { log = "x" }, { full = "/b", percent = 95 }"#,
        );
        assert_eq!(parse(text.as_bytes()).unwrap().full_paths(), ["/a", "/b"]);
    }

    #[test]
    fn the_size_the_repository_reads_is_the_size_a_file_may_have() {
        assert_eq!(system_release::repo::SIGNATURES_MAX, MAX_BYTES as u64);
        assert_eq!(system_release::repo::SIGNATURES_TARGET, "troubleshoot/signatures.toml");
    }

    #[test]
    fn a_file_is_at_most_256_kib() {
        assert_eq!(MAX_BYTES, 262_144);
        assert!(parse(of_size(MAX_BYTES).as_bytes()).is_ok());
        let over = of_size(MAX_BYTES + 1);
        refused(&over, "262145 bytes");
        // Whatever it holds: it is not read at all.
        let rubbish = vec![b'{'; MAX_BYTES + 1];
        refused(std::str::from_utf8(&rubbish).unwrap(), "262145 bytes");
        assert!(parse(&vec![0xff; MAX_BYTES + 1]).unwrap_err().contains("bytes"));
    }

    #[test]
    fn a_file_has_at_most_512_signatures() {
        let one = "[[signature]]\nid = \"TST-001\"\ncause = \"c\"\nfix = \"f\"\nmatch = [{ start = \"failed\" }]\n";
        let read = parse(format!("{HEADER}{}", one.repeat(MAX_SIGNATURES)).as_bytes()).unwrap();
        assert_eq!(read.signatures.len(), 512);
        refused(&format!("{HEADER}{}", one.repeat(MAX_SIGNATURES + 1)), "513 signatures");
    }

    #[test]
    fn a_signature_has_from_1_to_8_conditions() {
        let one = "{ start = \"failed\" }";
        let many = |count: usize| with(&vec![one; count].join(", "));
        assert_eq!(
            parse(many(MAX_CONDITIONS).as_bytes()).unwrap().signatures[0].conditions.len(),
            8
        );
        refused(&many(MAX_CONDITIONS + 1), "9 conditions");
        // A signature with no condition would match every start.
        refused(&many(0), "0 conditions");
    }

    #[test]
    fn a_file_has_at_most_64_conditions_on_the_log_in_all() {
        // Eight signatures of eight each are 64, which is allowed.
        let entry = |id: &str, conditions: &str| {
            format!(
                "[[signature]]\nid = \"{id}\"\ncause = \"c\"\nfix = \"f\"\nmatch = [{conditions}]\n"
            )
        };
        let logs = |count: usize| vec!["{ log = \"a*\" }"; count].join(", ");
        let file = |signatures: usize, each: usize| {
            let one: String =
                (0..signatures).map(|n| entry(&format!("TST-{n:03}"), &logs(each))).collect();
            format!("{HEADER}{one}")
        };
        let read = parse(file(8, 8).as_bytes()).unwrap();
        assert_eq!(read.signatures.len(), 8);
        refused(&file(65, 1), "65 conditions on the log");
        refused(&file(9, 8), "72 conditions on the log");
        // Conditions of other kinds are not counted.
        let others = entry("TST-100", &["{ start = \"failed\" }"; 8].join(", "));
        assert!(parse(format!("{}{others}{others}", file(8, 8)).as_bytes()).is_ok());
        // A signature this release skips still counts, as it does for a
        // release that reads it.
        let new = entry("TST-101", "{ log = \"a*\" }, { battery = 5 }");
        let text = format!("{}{new}", file(8, 8));
        refused(&text, "65 conditions on the log");
    }

    #[test]
    fn a_glob_has_at_most_200_bytes() {
        let glob = |length: usize| with(&format!("{{ log = \"{}\" }}", "a".repeat(length)));
        assert!(parse(glob(super::super::glob::MAX_LEN).as_bytes()).is_ok());
        refused(&glob(super::super::glob::MAX_LEN + 1), "201 bytes");
        refused(&glob(0), "it is empty");
        refused(&with("{ log = \"caf\u{e9}\" }"), "not printable ASCII");
        refused(&with("{ log = \"a\\tb\" }"), "not printable ASCII");
    }

    #[test]
    fn a_cause_and_a_fix_are_one_line_of_at_most_500_bytes() {
        let text = |key: &str, value: &str| {
            let (cause, fix) = if key == "cause" { (value, "f") } else { ("c", value) };
            format!(
                "{HEADER}[[signature]]\nid = \"TST-001\"\ncause = \"{cause}\"\nfix = \"{fix}\"\nmatch = [{{ start = \"failed\" }}]\n"
            )
        };
        for key in ["cause", "fix"] {
            assert!(parse(text(key, &"x".repeat(MAX_TEXT)).as_bytes()).is_ok());
            refused(&text(key, &"x".repeat(MAX_TEXT + 1)), &format!("`{key}` is 501 bytes"));
            refused(&text(key, ""), &format!("`{key}` is 0 bytes"));
            for bad in
                ["a\\nb", "a\\u001b[31mb", "a\\u0007b", "a\\u202eb", "a\\u200bb", "a\\u007fb"]
            {
                refused(&text(key, bad), "not for reading");
            }
            // Text that is not English is as readable.
            assert!(
                parse(text(key, "\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442} \u{1f600}").as_bytes())
                    .is_ok()
            );
        }
    }

    #[test]
    fn what_is_not_in_the_format_refuses_the_file_whole() {
        let good = with("{ start = \"failed\" }");
        assert!(parse(good.as_bytes()).is_ok());
        refused("", "does not say `format = 1`");
        refused("version = 1\n", "does not say `format = 1`");
        refused("format = 2\nversion = 1\n", "its format is 2");
        refused("format = \"1\"\nversion = 1\n", "does not say `format = 1`");
        refused("format = 1\n", "no `version`");
        refused("format = 1\nversion = -1\n", "negative");
        refused("format = 1\nversion = \"1\"\n", "no `version`");
        refused("format = 1\nversion = 1.5\n", "no `version`");
        refused(&format!("{good}note = 1\n"), "`note` is no part of");
        refused("format = 1\nversion = 1\nsignature = 3\n", "not a list");
        refused("format = 1\nversion = 1\nsignature = [3]\n", "signature 1 is not a table");
        refused(
            &good.replace("fix = \"f\"", "fix = \"f\"\nnote = \"n\""),
            "`note` is no part of a signature",
        );
        refused(&good.replace("id = \"TST-001\"\n", ""), "no `id`");
        refused(&good.replace("cause = \"c\"\n", ""), "no `cause`");
        refused(&good.replace("fix = \"f\"\n", ""), "no `fix`");
        refused(&good.replace("id = \"TST-001\"", "id = 1"), "`id` is not a string");
        for id in [
            "tst-001",
            "TST-01",
            "TST-0001",
            "T-001",
            "TSTUVW-001",
            "TST_001",
            "TST-00a",
            "TST001",
            "-001",
            "",
        ] {
            refused(&good.replace("TST-001", id), "not an error ID");
        }
        refused(
            &good.replace("match = [{ start = \"failed\" }]", "match = 3"),
            "not a list of conditions",
        );
        refused(&good.replace("match = [{ start = \"failed\" }]", "match = [3]"), "not a table");
        refused("not toml at all [", "line 1");
        refused(&format!("{good}\n\n= 1"), "line");
        assert!(parse(&[0xff, 0xfe, b'f']).unwrap_err().contains("UTF-8"));
        // A refusal of the second signature refuses the first with it.
        refused(
            &format!("{good}{}", good.replace(HEADER, "").replace("TST-001", "bad")),
            "signature 2",
        );
    }

    #[test]
    fn a_condition_is_refused_when_it_is_wrong_and_not_when_it_is_new() {
        for bad in [
            "{ log = 5 }",
            "{ log = \"a\", service = 5 }",
            "{ log = \"a\", service = \"a b\" }",
            "{ log = \"a\", service = \"\" }",
            "{ log = \"a\", level = 3 }",
            "{ start = 1 }",
            "{ start = \"failed\", error = \"nope\" }",
            "{ start = \"failed\", error = 7 }",
            "{ changed = 1 }",
            "{ failed = 1 }",
            "{ failed = \"a b\" }",
            "{ failed = \"\" }",
            "{ full = 1, percent = 90 }",
            "{ full = \"/\" }",
            "{ full = \"relative\", percent = 90 }",
            "{ full = \"/a\\nb\", percent = 90 }",
            "{ full = \"/\", percent = 0 }",
            "{ full = \"/\", percent = 101 }",
            "{ full = \"/\", percent = -1 }",
            "{ full = \"/\", percent = \"90\" }",
            "{ full = \"/\", percent = 90.5 }",
            "{ log = \"a\", start = \"failed\" }",
            "{ failed = \"*\", changed = \"app\" }",
        ] {
            assert!(parse(with(bad).as_bytes()).is_err(), "{bad}");
        }
        assert!(parse(with(&format!("{{ failed = \"{}\" }}", "a".repeat(64))).as_bytes()).is_ok());
        assert!(parse(with(&format!("{{ failed = \"{}\" }}", "a".repeat(65))).as_bytes()).is_err());
        assert!(
            parse(with(&format!("{{ full = \"/{}\", percent = 1 }}", "a".repeat(199))).as_bytes())
                .is_ok()
        );
        assert!(
            parse(with(&format!("{{ full = \"/{}\", percent = 1 }}", "a".repeat(200))).as_bytes())
                .is_err()
        );
        for percent in [1, 50, 100] {
            assert!(
                parse(with(&format!("{{ full = \"/\", percent = {percent} }}")).as_bytes()).is_ok()
            );
        }
        // What this release does not know skips the signature, and only it.
        let known = "{ start = \"failed\" }";
        for new in [
            "{ battery = 5 }",
            "{ battery = { low = true } }",
            "{}",
            "{ unrelated = \"x\" }",
            "{ log = \"a\", host = \"x\" }",
            "{ log = \"a\", level = \"fatal\" }",
            "{ start = \"slow\" }",
            "{ start = \"failed\", since = 3 }",
            "{ changed = \"wifi\" }",
            "{ changed = \"app\", name = \"x\" }",
            "{ failed = \"*\", since = 3 }",
            "{ full = \"/\", percent = 90, inodes = true }",
        ] {
            let both = format!(
                "{}[[signature]]\nid = \"TST-002\"\ncause = \"c\"\nfix = \"f\"\nmatch = [{known}, {new}]\n",
                with(known)
            );
            let read = parse(both.as_bytes()).unwrap_or_else(|why| panic!("{new}: {why}"));
            assert_eq!((read.signatures.len(), read.skipped), (1, 1), "{new}");
            assert_eq!(read.signatures[0].id, "TST-001");
        }
        // But a signature with something new is read as far as the rest of the file goes.
        let both = format!("{}{}", with("{ battery = 5 }"), "[[signature]]\nid = \"bad\"\n");
        assert!(parse(both.as_bytes()).is_err());
    }

    #[test]
    fn a_file_nested_deeper_than_the_parser_allows_is_refused_and_does_not_end_the_program() {
        // The TOML reader stops at a depth of its own, so that what is read
        // from a file of up to 256 KiB cannot use the stack up. This is what
        // holds it to that: if it did not, these would end the program.
        let mut nested = Vec::new();
        for (open, close, times) in [("[", "]", 120_000), ("{ a = [", "] }", 20_000)] {
            nested.push(format!("signature = {}{}\n", open.repeat(times), close.repeat(times)));
        }
        // Keys that dot down as far as the file allows.
        nested.push(format!("{}x = 1\n", "a.".repeat(120_000)));
        nested.push(format!("[{}x]\n", "a.".repeat(120_000)));
        for body in nested {
            let text = format!("{HEADER}{body}");
            assert!(text.len() <= MAX_BYTES, "{}", text.len());
            refused(&text, "recurs");
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("signatures-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_of_version(version: u64, id: &str) -> String {
        format!(
            "format = 1\nversion = {version}\n[[signature]]\nid = \"{id}\"\ncause = \"c\"\nfix = \"f\"\nmatch = [{{ start = \"failed\" }}]\n"
        )
    }

    #[test]
    fn a_file_is_kept_only_when_newer_than_what_is_shipped_and_what_is_kept() {
        let dir = scratch("version");
        let shipped = dir.join("shipped.toml");
        let kept = dir.join("kept");
        std::fs::write(&shipped, file_of_version(5, "TST-005")).unwrap();
        let ids = |loaded: &Loaded| -> Vec<String> {
            loaded.signatures.signatures.iter().map(|s| s.id.clone()).collect()
        };
        let loaded = load_in(&shipped, &kept);
        assert_eq!((loaded.signatures.version, loaded.source), (5, Source::Shipped));
        // The same version, and older ones, are not taken.
        for version in [5, 4, 0] {
            let bytes = file_of_version(version, "TST-099");
            assert_eq!(
                keep_in(&shipped, &kept, bytes.as_bytes()),
                Ok(Kept::NotNewer { offered: version, have: 5 })
            );
        }
        assert!(!kept.exists(), "nothing is written for them");
        // A newer is.
        assert_eq!(
            keep_in(&shipped, &kept, file_of_version(7, "TST-007").as_bytes()),
            Ok(Kept::New(7))
        );
        let loaded = load_in(&shipped, &kept);
        assert_eq!(
            (loaded.signatures.version, loaded.source, ids(&loaded)),
            (7, Source::Kept, vec!["TST-007".to_string()])
        );
        // And then only a newer than that.
        for version in [7, 6, 5] {
            let bytes = file_of_version(version, "TST-099");
            assert_eq!(
                keep_in(&shipped, &kept, bytes.as_bytes()),
                Ok(Kept::NotNewer { offered: version, have: 7 })
            );
        }
        assert_eq!(ids(&load_in(&shipped, &kept)), ["TST-007"]);
        assert_eq!(
            keep_in(&shipped, &kept, file_of_version(8, "TST-008").as_bytes()),
            Ok(Kept::New(8))
        );
        assert_eq!(ids(&load_in(&shipped, &kept)), ["TST-008"]);
        assert!(!kept.join("signatures.toml.partial").exists());
        // A system update ships a newer one than the kept: the shipped is used,
        // and the kept one is not taken over by an offer that is not newer than it.
        std::fs::write(&shipped, file_of_version(9, "TST-009")).unwrap();
        let loaded = load_in(&shipped, &kept);
        assert_eq!((loaded.signatures.version, loaded.source), (9, Source::Shipped));
        assert_eq!(
            keep_in(&shipped, &kept, file_of_version(9, "TST-098").as_bytes()),
            Ok(Kept::NotNewer { offered: 9, have: 9 })
        );
        // A kept file that does not read any more is left out, with a word.
        std::fs::write(&shipped, file_of_version(3, "TST-003")).unwrap();
        std::fs::write(kept.join("signatures.toml"), "garbage [").unwrap();
        let loaded = load_in(&shipped, &kept);
        assert_eq!(
            (loaded.signatures.version, loaded.source, loaded.notes.len()),
            (3, Source::Shipped, 1)
        );
        assert_eq!(
            keep_in(&shipped, &kept, file_of_version(4, "TST-004").as_bytes()),
            Ok(Kept::New(4))
        );
        assert_eq!(load_in(&shipped, &kept).signatures.version, 4);
        // A file larger than the limit on the disk is not read whole.
        std::fs::write(kept.join("signatures.toml"), vec![b'#'; MAX_BYTES * 4]).unwrap();
        let loaded = load_in(&shipped, &kept);
        assert_eq!((loaded.signatures.version, loaded.notes.len()), (3, 1));
        assert!(loaded.notes[0].contains("bytes"), "{:?}", loaded.notes);
        // Nothing shipped and nothing kept is no signatures, not a failure.
        let empty = load_in(&dir.join("none"), &dir.join("none-either"));
        assert_eq!(
            (empty.signatures.version, empty.signatures.signatures.len(), empty.notes.len()),
            (0, 0, 0)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_does_not_read_is_refused_and_remembered_by_its_hash() {
        let dir = scratch("refused");
        let shipped = dir.join("shipped.toml");
        let kept = dir.join("kept");
        std::fs::write(&shipped, file_of_version(5, "TST-005")).unwrap();
        let bad = b"format = 1\nversion = 9\nnonsense = 1\n";
        let Ok(Kept::Refused { why, first: true }) = keep_in(&shipped, &kept, bad) else {
            panic!("not refused");
        };
        assert!(why.contains("nonsense"), "{why}");
        // The same bytes are refused again without being new.
        assert!(matches!(keep_in(&shipped, &kept, bad), Ok(Kept::Refused { first: false, .. })));
        // Others are new each.
        let other = b"format = 1\nversion = 9\nother = 1\n";
        assert!(matches!(keep_in(&shipped, &kept, other), Ok(Kept::Refused { first: true, .. })));
        assert!(matches!(keep_in(&shipped, &kept, bad), Ok(Kept::Refused { first: false, .. })));
        // What was refused is not in use.
        let loaded = load_in(&shipped, &kept);
        assert_eq!((loaded.signatures.version, loaded.source), (5, Source::Shipped));
        assert!(!kept.join("signatures.toml").exists());
        // Only the last 64 are remembered.
        for number in 0..70 {
            let text = format!("format = 1\nversion = 9\nn{number} = 1\n");
            assert!(matches!(
                keep_in(&shipped, &kept, text.as_bytes()),
                Ok(Kept::Refused { first: true, .. })
            ));
        }
        let list = std::fs::read_to_string(kept.join("refused")).unwrap();
        assert_eq!(list.lines().count(), REFUSED_KEPT);
        assert!(
            list.lines()
                .all(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        );
        // A file over the size is refused whole as well, whatever its version.
        let big = of_size(MAX_BYTES + 1).replace("version = 5", "version = 99");
        assert!(matches!(
            keep_in(&shipped, &kept, big.as_bytes()),
            Ok(Kept::Refused { first: true, .. })
        ));
        assert!(!kept.join("signatures.toml").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
