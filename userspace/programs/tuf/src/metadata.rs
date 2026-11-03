// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! TUF's four kinds of metadata and the envelope each comes in, read as
//! the specification (version 1) and python-tuf read them, and checks of
//! files against the lengths and hashes metadata gives them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fmt::Write as _;

use ring::digest;
use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::keys::Key;
use crate::{json, time};

/// The top-level roles, which root delegates to.
pub const TOP_LEVEL: [&str; 4] = ["root", "timestamp", "snapshot", "targets"];

/// Why metadata or a file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// It is not metadata of the kind wanted: malformed JSON, missing or
    /// wrong fields, or the wrong `_type`.
    Malformed(String),
    /// Fewer of the role's keys signed it than its threshold.
    Unsigned {
        /// The role.
        role: String,
        /// How many of its keys signed.
        signed: usize,
        /// How many must.
        threshold: u64,
    },
    /// Its version is not the one expected.
    Version(String),
    /// Its version is the trusted one's: nothing new.
    SameVersion,
    /// It has expired.
    Expired(String),
    /// A file is not the length or hash the metadata gives it.
    Mismatch(String),
    /// Something the workflow needs is missing, such as a role snapshot
    /// does not list.
    Repository(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed(why) => write!(f, "malformed metadata: {why}"),
            Error::Unsigned { role, signed, threshold } => {
                write!(f, "{role} was signed by {signed} of the {threshold} keys it needs")
            }
            Error::Version(why) | Error::Expired(why) | Error::Mismatch(why) => write!(f, "{why}"),
            Error::Repository(why) => write!(f, "{why}"),
            Error::SameVersion => write!(f, "the version is the one already trusted"),
        }
    }
}

impl std::error::Error for Error {}

fn malformed(why: impl Into<String>) -> Error {
    Error::Malformed(why.into())
}

/// Hashes of a file by algorithm, in the order the metadata lists them:
/// the first names a target in a consistent snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hashes(pub Vec<(String, String)>);

impl<'de> Deserialize<'de> for Hashes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Hashes;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an object of hashes by algorithm")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Hashes, A::Error> {
                let mut hashes = Vec::new();
                while let Some(entry) = map.next_entry::<String, String>()? {
                    hashes.push(entry);
                }
                if hashes.is_empty() {
                    return Err(de::Error::custom("a file's hashes are empty"));
                }
                Ok(Hashes(hashes))
            }
        }
        deserializer.deserialize_map(Ordered)
    }
}

/// The digest of `data` by `algorithm`, in hex; `None` for an algorithm
/// ring does not have.
fn digest_hex(algorithm: &str, data: &[u8]) -> Option<String> {
    let algorithm = match algorithm {
        "sha256" => &digest::SHA256,
        "sha384" => &digest::SHA384,
        "sha512" => &digest::SHA512,
        "sha512_256" => &digest::SHA512_256,
        _ => return None,
    };
    Some(digest::digest(algorithm, data).as_ref().iter().fold(String::new(), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    }))
}

/// Checks `data` against `length` and every one of `hashes`.
///
/// # Errors
///
/// [`Error::Mismatch`] if any differs, or a hash's algorithm is not one
/// this client has.
pub fn check_file(data: &[u8], length: Option<u64>, hashes: Option<&Hashes>) -> Result<(), Error> {
    if let Some(length) = length
        && data.len() as u64 != length
    {
        return Err(Error::Mismatch(format!("{} bytes, not the {length} expected", data.len())));
    }
    for (algorithm, expected) in hashes.map_or_default(|h| h.0.as_slice()) {
        match digest_hex(algorithm, data) {
            Some(seen) if seen == *expected => {}
            Some(seen) => {
                return Err(Error::Mismatch(format!("{algorithm} is {seen}, not {expected}")));
            }
            None => return Err(Error::Mismatch(format!("{algorithm} is not a hash this has"))),
        }
    }
    Ok(())
}

/// A file a role lists: another role's metadata, in timestamp's and
/// snapshot's `meta`.
#[derive(Debug, Clone, Deserialize)]
pub struct MetaFile {
    /// Its version.
    pub version: u64,
    /// Its length, if given.
    #[serde(default)]
    pub length: Option<u64>,
    /// Its hashes, if given.
    #[serde(default)]
    pub hashes: Option<Hashes>,
}

/// A target file, as targets metadata lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetFile {
    /// Its length.
    pub length: u64,
    /// Its hashes.
    pub hashes: Hashes,
    /// What the repository says of it besides.
    #[serde(default)]
    pub custom: Option<Value>,
}

/// The keys that may sign for a role, and how many must.
#[derive(Debug, Clone, Deserialize)]
pub struct Role {
    /// The keys, by ID.
    pub keyids: Vec<String>,
    /// How many of them must sign.
    pub threshold: u64,
}

/// Root: the keys of every top-level role.
#[derive(Debug, Clone, Deserialize)]
pub struct Root {
    /// Whether files are fetched by version or hash (TUF 6.2).
    #[serde(default)]
    pub consistent_snapshot: Option<bool>,
    /// The keys, by ID.
    pub keys: HashMap<String, Key>,
    /// The four top-level roles.
    pub roles: HashMap<String, Role>,
}

/// Timestamp: the snapshot that is current.
#[derive(Debug, Clone, Deserialize)]
pub struct Timestamp {
    /// `snapshot.json`'s version and, maybe, length and hashes.
    pub meta: BTreeMap<String, MetaFile>,
}

/// Snapshot: every targets role's current version.
#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    /// Each targets role's metadata file, by name.
    pub meta: BTreeMap<String, MetaFile>,
}

/// Targets: target files, and roles delegated some of them.
#[derive(Debug, Clone, Deserialize)]
pub struct Targets {
    /// The files, by path.
    pub targets: HashMap<String, TargetFile>,
    /// The roles this one delegates to.
    #[serde(default)]
    pub delegations: Option<Delegations>,
}

/// The roles a targets role delegates to, and their keys.
#[derive(Debug, Clone, Deserialize)]
pub struct Delegations {
    /// The keys, by ID.
    pub keys: HashMap<String, Key>,
    /// The roles, in the order they are searched.
    #[serde(default)]
    pub roles: Option<Vec<DelegatedRole>>,
    /// Hash bins in place of roles (TAP 15).
    #[serde(default)]
    pub succinct_roles: Option<SuccinctRoles>,
}

/// A delegated targets role.
#[derive(Debug, Clone, Deserialize)]
pub struct DelegatedRole {
    /// Its name.
    pub name: String,
    /// The keys that may sign for it.
    pub keyids: Vec<String>,
    /// How many must.
    pub threshold: u64,
    /// Whether the search for a target it is delegated stops at it.
    pub terminating: bool,
    /// The patterns of the target paths delegated to it.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// Or the prefixes of the hex SHA-256 of those paths.
    #[serde(default)]
    pub path_hash_prefixes: Option<Vec<String>>,
}

/// Hash bins (TAP 15): `bit_length` bits of a path's SHA-256 name the
/// role, `name_prefix-<bin>`.
#[derive(Debug, Clone, Deserialize)]
pub struct SuccinctRoles {
    /// The keys that may sign for every bin.
    pub keyids: Vec<String>,
    /// How many must.
    pub threshold: u64,
    /// How many bits name a bin.
    pub bit_length: u32,
    /// The bins' names before the bin's number.
    pub name_prefix: String,
}

/// Whether `text` matches `pattern`, a shell pattern of `*`, `?` and
/// `[...]` as Python's `fnmatch.fnmatchcase` takes it.
fn fnmatch(text: &[char], pattern: &[char]) -> bool {
    let Some((&first, rest)) = pattern.split_first() else { return text.is_empty() };
    match first {
        '*' => (0..=text.len()).any(|skip| fnmatch(&text[skip..], rest)),
        '?' => !text.is_empty() && fnmatch(&text[1..], rest),
        '[' => match set(rest) {
            Some((matches, after)) => {
                text.first().is_some_and(|&c| matches(c)) && fnmatch(&text[1..], after)
            }
            // An unclosed `[` is itself.
            None => text.first() == Some(&'[') && fnmatch(&text[1..], rest),
        },
        c => text.first() == Some(&c) && fnmatch(&text[1..], rest),
    }
}

/// The set a `[` opens in a pattern, up to its `]`: a test of a
/// character, and the pattern after the set; `None` if it is not closed.
fn set(pattern: &[char]) -> Option<(impl Fn(char) -> bool, &[char])> {
    let (negated, body) = match pattern.first() {
        Some('!') => (true, &pattern[1..]),
        _ => (false, pattern),
    };
    // A `]` first is a member, not the end.
    let end = body.iter().skip(1).position(|&c| c == ']')? + 1;
    let members: Vec<char> = body[..end].to_vec();
    let after = &body[end + 1..];
    let test = move |c: char| {
        let mut found = false;
        let mut i = 0;
        while i < members.len() {
            if i + 2 < members.len() && members[i + 1] == '-' {
                found |= members[i] <= c && c <= members[i + 2];
                i += 3;
            } else {
                found |= members[i] == c;
                i += 1;
            }
        }
        found != negated
    };
    Some((test, after))
}

impl DelegatedRole {
    /// Whether `path` is delegated to this role.
    #[must_use]
    pub fn delegates(&self, path: &str) -> bool {
        if let Some(prefixes) = &self.path_hash_prefixes {
            let hash = digest_hex("sha256", path.as_bytes()).unwrap_or_default();
            return prefixes.iter().any(|prefix| hash.starts_with(prefix.as_str()));
        }
        let Some(patterns) = &self.paths else { return false };
        let parts: Vec<Vec<char>> = path.split('/').map(|p| p.chars().collect()).collect();
        patterns.iter().any(|pattern| {
            let pattern: Vec<Vec<char>> = pattern.split('/').map(|p| p.chars().collect()).collect();
            pattern.len() == parts.len()
                && parts.iter().zip(&pattern).all(|(part, pattern)| fnmatch(part, pattern))
        })
    }
}

impl SuccinctRoles {
    /// How many hex digits a bin's number has.
    fn suffix_len(&self) -> usize {
        let bins_less_one = (1u64 << self.bit_length) - 1;
        format!("{bins_less_one:x}").len()
    }

    /// The bin `path` is in.
    #[must_use]
    pub fn role_for(&self, path: &str) -> String {
        let hash = digest::digest(&digest::SHA256, path.as_bytes());
        let first = u32::from_be_bytes(hash.as_ref()[..4].try_into().unwrap_or_default());
        let bin = u64::from(first) >> (32 - self.bit_length);
        format!("{}-{bin:0width$x}", self.name_prefix, width = self.suffix_len())
    }

    /// Whether `name` is one of the bins.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        let Some(suffix) = name.strip_prefix(&self.name_prefix).and_then(|s| s.strip_prefix('-'))
        else {
            return false;
        };
        suffix.len() == self.suffix_len()
            && u64::from_str_radix(suffix, 16).is_ok_and(|bin| bin < 1u64 << self.bit_length)
    }
}

impl Delegations {
    /// The roles `path` is delegated to, in the order to search them,
    /// each with whether the search ends at it.
    #[must_use]
    pub fn roles_for(&self, path: &str) -> Vec<(String, bool)> {
        if let Some(succinct) = &self.succinct_roles {
            return vec![(succinct.role_for(path), true)];
        }
        self.roles
            .iter()
            .flatten()
            .filter(|role| role.delegates(path))
            .map(|role| (role.name.clone(), role.terminating))
            .collect()
    }
}

/// What can delegate: root, to the top-level roles, and targets roles,
/// to theirs.
pub trait Delegator {
    /// The keys that may sign for `role`, and how many must; `None` if
    /// it delegates no such role.
    fn role(&self, role: &str) -> Option<(&[String], u64)>;
    /// The key `keyid`.
    fn key(&self, keyid: &str) -> Option<&Key>;
}

impl Delegator for Root {
    fn role(&self, role: &str) -> Option<(&[String], u64)> {
        self.roles.get(role).map(|r| (r.keyids.as_slice(), r.threshold))
    }

    fn key(&self, keyid: &str) -> Option<&Key> {
        self.keys.get(keyid)
    }
}

impl Delegator for Targets {
    fn role(&self, role: &str) -> Option<(&[String], u64)> {
        let delegations = self.delegations.as_ref()?;
        if let Some(succinct) = &delegations.succinct_roles {
            return succinct.has(role).then_some((succinct.keyids.as_slice(), succinct.threshold));
        }
        let found = delegations.roles.iter().flatten().find(|r| r.name == role)?;
        Some((found.keyids.as_slice(), found.threshold))
    }

    fn key(&self, keyid: &str) -> Option<&Key> {
        self.delegations.as_ref()?.keys.get(keyid)
    }
}

/// A kind of metadata.
pub trait Kind: DeserializeOwned {
    /// Its `_type`.
    const TYPE: &'static str;
    /// Checks what serde cannot.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for what TUF does not allow.
    fn check(&self) -> Result<(), Error>;
}

fn check_role(keyids: &[String], threshold: u64, name: &str) -> Result<(), Error> {
    let unique: HashSet<&String> = keyids.iter().collect();
    if unique.len() != keyids.len() {
        return Err(malformed(format!("{name} lists a key twice")));
    }
    if threshold < 1 {
        return Err(malformed(format!("{name}'s threshold is 0")));
    }
    Ok(())
}

fn check_meta(meta: &BTreeMap<String, MetaFile>) -> Result<(), Error> {
    for (name, file) in meta {
        if file.version == 0 {
            return Err(malformed(format!("{name}'s version is 0")));
        }
    }
    Ok(())
}

impl Kind for Root {
    const TYPE: &'static str = "root";

    fn check(&self) -> Result<(), Error> {
        let names: HashSet<&str> = self.roles.keys().map(String::as_str).collect();
        if names != HashSet::from(TOP_LEVEL) {
            return Err(malformed("root's roles are not the four top-level ones"));
        }
        for (name, role) in &self.roles {
            check_role(&role.keyids, role.threshold, name)?;
        }
        Ok(())
    }
}

impl Kind for Timestamp {
    const TYPE: &'static str = "timestamp";

    fn check(&self) -> Result<(), Error> {
        if !self.meta.contains_key("snapshot.json") {
            return Err(malformed("timestamp has no snapshot.json"));
        }
        check_meta(&self.meta)
    }
}

impl Timestamp {
    /// The snapshot it names.
    #[must_use]
    pub fn snapshot(&self) -> &MetaFile {
        &self.meta["snapshot.json"]
    }
}

impl Kind for Snapshot {
    const TYPE: &'static str = "snapshot";

    fn check(&self) -> Result<(), Error> {
        check_meta(&self.meta)
    }
}

impl Kind for Targets {
    const TYPE: &'static str = "targets";

    fn check(&self) -> Result<(), Error> {
        let Some(delegations) = &self.delegations else { return Ok(()) };
        match (&delegations.roles, &delegations.succinct_roles) {
            (Some(roles), None) => {
                let mut names = HashSet::new();
                for role in roles {
                    if role.name.is_empty() || TOP_LEVEL.contains(&role.name.as_str()) {
                        return Err(malformed(format!(
                            "a delegated role is named {:?}",
                            role.name
                        )));
                    }
                    if !names.insert(role.name.as_str()) {
                        return Err(malformed(format!("{} is delegated twice", role.name)));
                    }
                    if role.paths.is_some() == role.path_hash_prefixes.is_some() {
                        return Err(malformed(format!(
                            "{} needs paths or path_hash_prefixes, and not both",
                            role.name
                        )));
                    }
                    check_role(&role.keyids, role.threshold, &role.name)?;
                }
                Ok(())
            }
            (None, Some(succinct)) => {
                if !(1..=32).contains(&succinct.bit_length) {
                    return Err(malformed("succinct roles' bit_length is not 1 to 32"));
                }
                check_role(&succinct.keyids, succinct.threshold, &succinct.name_prefix)
            }
            _ => Err(malformed("delegations need roles or succinct_roles, and not both")),
        }
    }
}

/// Metadata of kind `T`, parsed, with what its signatures are over.
#[derive(Debug, Clone)]
pub struct Signed<T> {
    /// What it says.
    pub signed: T,
    /// Its version.
    pub version: u64,
    /// When it expires, in seconds since 1970.
    pub expires: i64,
    /// The canonical JSON of its `signed`, which its signatures are over.
    pub bytes: Vec<u8>,
    /// Its signatures, in hex, by key ID.
    pub signatures: HashMap<String, String>,
}

/// The envelope, as it comes.
#[derive(Deserialize)]
struct Envelope {
    signed: Value,
    signatures: Vec<Signature>,
}

#[derive(Deserialize)]
struct Signature {
    keyid: String,
    sig: String,
}

/// The envelope again, with what it says parsed as `T`: in the order the
/// file has it, which a [`serde_json::Value`] loses.
#[derive(Deserialize)]
struct Typed<T> {
    signed: T,
}

/// The fields every kind has.
#[derive(Deserialize)]
struct Common {
    #[serde(rename = "_type")]
    kind: String,
    spec_version: String,
    version: u64,
    expires: String,
}

impl<T: Kind> Signed<T> {
    /// Parses `data` as metadata of kind `T`. Nothing is verified but its
    /// form: its signatures are checked by [`Signed::verified_by`].
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] if it is not well-formed metadata of kind
    /// `T`.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let envelope: Envelope =
            serde_json::from_slice(data).map_err(|err| malformed(err.to_string()))?;
        let common =
            Common::deserialize(&envelope.signed).map_err(|err| malformed(err.to_string()))?;
        if common.kind != T::TYPE {
            return Err(malformed(format!("expected {}, got {}", T::TYPE, common.kind)));
        }
        let spec: Vec<&str> = common.spec_version.split('.').collect();
        if !(2..=3).contains(&spec.len())
            || spec.iter().any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
            || spec[0] != "1"
        {
            return Err(malformed(format!("spec_version {} is not 1.x", common.spec_version)));
        }
        if common.version == 0 {
            return Err(malformed("version 0"));
        }
        let expires = time::parse(&common.expires)
            .ok_or_else(|| malformed(format!("expires {} is not a time", common.expires)))?;
        let typed: Typed<T> =
            serde_json::from_slice(data).map_err(|err| malformed(err.to_string()))?;
        typed.signed.check()?;
        let mut signatures = HashMap::new();
        for signature in envelope.signatures {
            if signatures.insert(signature.keyid.clone(), signature.sig).is_some() {
                return Err(malformed(format!("two signatures by {}", signature.keyid)));
            }
        }
        let bytes = json::canonical(&envelope.signed).map_err(|err| malformed(err.to_string()))?;
        Ok(Signed { signed: typed.signed, version: common.version, expires, bytes, signatures })
    }

    /// Checks that enough of the keys `delegator` gives `role` signed it.
    ///
    /// # Errors
    ///
    /// [`Error::Unsigned`] if fewer than the threshold did, or
    /// `delegator` does not delegate `role`.
    pub fn verified_by(&self, delegator: &dyn Delegator, role: &str) -> Result<(), Error> {
        let Some((keyids, threshold)) = delegator.role(role) else {
            return Err(Error::Unsigned { role: role.into(), signed: 0, threshold: 1 });
        };
        let signed = keyids
            .iter()
            .filter(|keyid| {
                let (Some(key), Some(sig)) = (delegator.key(keyid), self.signatures.get(*keyid))
                else {
                    return false;
                };
                key.verifies(&self.bytes, sig)
            })
            .count();
        if (signed as u64) < threshold {
            return Err(Error::Unsigned { role: role.into(), signed, threshold });
        }
        Ok(())
    }

    /// Whether it has expired at `now`.
    #[must_use]
    pub fn expired(&self, now: i64) -> bool {
        now >= self.expires
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn matches(text: &str, pattern: &str) -> bool {
        fnmatch(&chars(text), &chars(pattern))
    }

    #[test]
    fn patterns_match_as_fnmatchcase_does() {
        assert!(matches("file.txt", "*.txt"));
        assert!(matches("", "*"));
        assert!(matches("a1", "a?"));
        assert!(!matches("a", "a?"));
        assert!(!matches("File.txt", "file.*"));
        assert!(matches("b", "[abc]"));
        assert!(matches("q", "[!abc]"));
        assert!(!matches("a", "[!abc]"));
        assert!(matches("m", "[a-z]"));
        assert!(matches("]", "[]]"));
        assert!(matches("[x", "[x"));
        assert!(matches("x-y", "*-*"));
    }

    fn role(paths: &[&str]) -> DelegatedRole {
        DelegatedRole {
            name: "r".into(),
            keyids: vec![],
            threshold: 1,
            terminating: false,
            paths: Some(paths.iter().map(|p| (*p).to_string()).collect()),
            path_hash_prefixes: None,
        }
    }

    #[test]
    fn paths_match_part_by_part() {
        assert!(role(&["targets/*.tgz"]).delegates("targets/foo.tgz"));
        assert!(!role(&["targets/*.tgz"]).delegates("targets/sub/foo.tgz"));
        assert!(!role(&["*"]).delegates("a/b"));
        assert!(role(&["*/*"]).delegates("a/b"));
        let hashed = DelegatedRole {
            paths: None,
            // The SHA-256 of "file1.txt" starts 55ae75d991.
            path_hash_prefixes: Some(vec!["55ae".into()]),
            ..role(&[])
        };
        assert!(hashed.delegates("file1.txt"));
        assert!(!hashed.delegates("file2.txt"));
    }

    #[test]
    fn bins_are_named_from_the_hash() {
        let bins = SuccinctRoles {
            keyids: vec![],
            threshold: 1,
            bit_length: 8,
            name_prefix: "bin".into(),
        };
        // The SHA-256 of "file1.txt" starts 0x55.
        assert_eq!(bins.role_for("file1.txt"), "bin-55");
        assert!(bins.has("bin-ff"));
        assert!(!bins.has("bin-100"));
        assert!(!bins.has("bin-f"));
        assert!(!bins.has("other-01"));
        let one = SuccinctRoles { bit_length: 1, ..bins.clone() };
        assert_eq!(one.role_for("file1.txt"), "bin-0");
        let all = SuccinctRoles { bit_length: 32, ..bins };
        assert_eq!(all.role_for("file1.txt").len(), "bin-".len() + 8);
    }

    #[test]
    fn files_are_checked_by_every_hash() {
        let sha256 = digest_hex("sha256", b"abc").unwrap();
        let sha512 = digest_hex("sha512", b"abc").unwrap();
        let both = Hashes(vec![("sha256".into(), sha256.clone()), ("sha512".into(), sha512)]);
        assert_eq!(check_file(b"abc", Some(3), Some(&both)), Ok(()));
        assert!(check_file(b"abd", Some(3), Some(&both)).is_err());
        assert!(check_file(b"abc", Some(4), None).is_err());
        let unknown = Hashes(vec![("md5".into(), "x".into())]);
        assert!(check_file(b"abc", None, Some(&unknown)).is_err());
        let hashes: Hashes = serde_json::from_str(r#"{"sha512": "b", "sha256": "a"}"#).unwrap();
        assert_eq!(hashes.0[0].0, "sha512");
        assert!(serde_json::from_str::<Hashes>("{}").is_err());
    }

    const ROOT: &str = r#"{"signed": {"_type": "root", "spec_version": "1.0.31",
        "version": 1, "expires": "2030-01-01T00:00:00Z", "consistent_snapshot": true,
        "keys": {}, "x-extra": [1, 2],
        "roles": {"root": {"keyids": [], "threshold": 1},
                  "timestamp": {"keyids": [], "threshold": 1},
                  "snapshot": {"keyids": [], "threshold": 1},
                  "targets": {"keyids": [], "threshold": 1}}},
        "signatures": [{"keyid": "a", "sig": "00"}]}"#;

    #[test]
    fn metadata_is_parsed_with_its_canonical_bytes() {
        let root = Signed::<Root>::parse(ROOT.as_bytes()).unwrap();
        assert_eq!(root.version, 1);
        assert_eq!(root.expires, time::parse("2030-01-01T00:00:00Z").unwrap());
        assert!(String::from_utf8(root.bytes.clone()).unwrap().starts_with(
            r#"{"_type":"root","consistent_snapshot":true,"expires":"2030-01-01T00:00:00Z","keys":{},"#
        ));
        assert_eq!(root.signed.role("root").map(|(_, t)| t), Some(1));
        assert!(matches!(root.verified_by(&root.signed, "root"), Err(Error::Unsigned { .. })));
        assert!(Signed::<Timestamp>::parse(ROOT.as_bytes()).is_err());
    }

    #[test]
    fn malformed_metadata_is_refused() {
        for (from, to) in [
            (r#""1.0.31""#, r#""2.0.0""#),
            (r#""1.0.31""#, r#""1""#),
            (r#""version": 1"#, r#""version": 0"#),
            ("2030-01-01T00:00:00Z", "soon"),
            (r#""threshold": 1}}}"#, r#""threshold": 0}}}"#),
            (r#""keyids": [], "threshold": 1},"#, r#""keyids": ["k", "k"], "threshold": 1},"#),
            (
                r#"{"keyid": "a", "sig": "00"}"#,
                r#"{"keyid": "a", "sig": "00"}, {"keyid": "a", "sig": "01"}"#,
            ),
            (r#""snapshot": {"#, r#""mirror": {"#),
            ("[1, 2]", "[1.5]"),
        ] {
            let text = ROOT.replacen(from, to, 1);
            assert_ne!(text, ROOT, "{from}");
            assert!(Signed::<Root>::parse(text.as_bytes()).is_err(), "{to}");
        }
    }
}
