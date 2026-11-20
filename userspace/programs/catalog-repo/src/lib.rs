// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog's TUF repository (ADR 0381), made and signed on the
//! computer its YubiKeys are plugged into, through `ob catalog`.
//!
//! What is signed offline is kept in the source tree, in `catalog/`:
//! the public half of each key in `keys/`, and in `metadata/` every
//! version of root, `N.root.json`, the newest also as `root.json`, and the
//! top-level targets, `targets.json`, which lists each bundle with its
//! length, SHA-256, ID and version. Root and targets are signed by a
//! YubiKey, either of the two alone; snapshot and timestamp, which only
//! say which targets are current and that it is still so, by the update
//! server's key, online. The bundles' bytes wait in a store until they
//! are published: `publish` lays out what a server serves, every file by
//! its version or hash, and reads it back with the client before it is
//! done.
//!
//! The online key also signs two delegated roles, whose files wait in the
//! store too: the list of known flaws (ADR 0389), and the rollout of the
//! newest version of each bundle (ADR 0544), which says which wave of
//! machines may take it ([`Repo::sign_rollouts`]).

pub mod keys;
pub mod plan;
mod verify;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use system_release::rollout::{self, Plan, Plans};
use tuf::metadata::{Delegator as _, Root, Signed, TargetFile, Targets};
use tuf::repo::{self, Signer};

use crate::keys::{KeyFile, Signers};
pub use crate::verify::Verified;

/// The four top-level roles.
pub const ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];

/// How long each role's metadata is good for, in days, unless it is told
/// otherwise: root a year, targets half one, so that a batch is signed at
/// least twice a year; snapshot a week and timestamp a day, re-signed by
/// the server every six hours.
pub const EXPIRES: [(&str, i64); 4] =
    [("root", 365), ("targets", 182), ("snapshot", 7), ("timestamp", 1)];

/// How long before it expires `status` warns that a role needs signing.
pub const WARN_DAYS: i64 = 30;

/// The role the targets delegate the catalog's list of known flaws to
/// (ADR 0389).
pub const FLAWS_ROLE: &str = dkv::flaws::ROLE;
/// The list's path among the targets.
pub const FLAWS_TARGET: &str = dkv::flaws::TARGET;
/// How long a list of flaws is good for, in days: a week, though the
/// server signs one each day.
pub const FLAWS_DAYS: i64 = 7;
/// Where the store keeps the flaws role's metadata.
const FLAWS_METADATA: &str = "flaws.role.json";
/// The target paths the targets delegate to the rollout role, whose
/// rollouts are `rollout/ID/VERSION.json`. TUF matches a pattern part by
/// part, so this reaches paths of three parts, the last a `.json` file,
/// and no others.
const ROLLOUT_PATHS: &str = "rollout/*/*.json";
/// How long the rollout role is good for, in days: the update server
/// signs it each time it refreshes, as the waves open on schedule.
pub const ROLLOUT_DAYS: i64 = 1;
/// How long the rollout role must still be good for [`Repo::sign_rollouts`]
/// to keep it as it is: two refreshes.
const ROLLOUT_MARGIN: i64 = 12 * 3600;
/// Where the store keeps the rollout role's metadata.
const ROLLOUT_METADATA: &str = "rollout.role.json";

/// The days `role`'s metadata is good for, by default.
#[must_use]
pub fn expires_days(role: &str) -> i64 {
    EXPIRES.iter().find(|(r, _)| *r == role).map_or(1, |(_, days)| *days)
}

/// The catalog's repository in the source tree.
pub struct Repo {
    dir: PathBuf,
}

/// A role's metadata as read: its bytes and what they say.
pub struct Loaded<T> {
    /// The file.
    pub bytes: Vec<u8>,
    /// What it says, checked as metadata.
    pub signed: Signed<T>,
}

fn read<T: tuf::metadata::Kind>(path: &Path) -> Result<Option<Loaded<T>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let signed = Signed::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(Some(Loaded { bytes, signed }))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, data).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What a batch changes in targets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Paths added, with what they list.
    pub added: BTreeMap<String, Value>,
    /// Paths taken out.
    pub removed: Vec<String>,
    /// Paths listed already, signed before the catalog vouched for their
    /// ID's publisher, that are listed as verified from now on.
    pub verified: Vec<String>,
    /// Paths listed that are withdrawn from now on (ADR 0593), with the
    /// `custom.withdrawn` object each gets: `kind`, `reason` and `since`.
    pub withdrawn: BTreeMap<String, Value>,
    /// Paths listed already whose bundle was packed again as other bytes.
    /// They sign nothing: the catalog keeps the bytes it signed, since a
    /// version, once signed, is never signed again as something else, and
    /// a rebuild reaches computers under a new version.
    pub rebuilt: Vec<Rebuilt>,
}

/// A bundle packed again as other bytes than those the catalog lists for
/// its version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebuilt {
    /// Its path among the targets.
    pub path: String,
    /// The file packed again.
    pub file: PathBuf,
    /// Its full ID, as the target's `custom.id` has it.
    pub id: String,
    /// Its version.
    pub version: String,
    /// The SHA-256 of the bytes the catalog lists, in hex.
    pub signed_sha256: String,
    /// The SHA-256 of the file packed again, in hex.
    pub packed_sha256: String,
}

/// The kinds of withdrawal, the lesser first.
pub const WITHDRAWAL_KINDS: [&str; 2] = ["broken", "malicious"];

/// The most bytes a withdrawal's reason may have.
pub const REASON_MAX: usize = 200;

impl Changes {
    /// Whether it changes nothing. The bundles packed again change
    /// nothing, as they are not signed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.verified.is_empty()
            && self.withdrawn.is_empty()
    }

    /// One line for each change.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (path, info) in &self.added {
            let custom = &info["custom"];
            let verified = if custom["owner"] == "verified" { ", publisher verified" } else { "" };
            lines.push(format!(
                "+ {path} ({} {}, {} bytes{verified})",
                custom["id"].as_str().unwrap_or("?"),
                custom["version"].as_str().unwrap_or("?"),
                info["length"]
            ));
        }
        lines.extend(self.removed.iter().map(|path| format!("- {path}")));
        lines.extend(self.verified.iter().map(|path| format!("~ {path}: publisher verified")));
        for (path, withdrawal) in &self.withdrawn {
            lines.push(format!(
                "! {path} withdrawn as {}: {}",
                withdrawal["kind"].as_str().unwrap_or("?"),
                withdrawal["reason"].as_str().unwrap_or("?")
            ));
        }
        lines.extend(self.rebuilt.iter().map(|rebuilt| {
            format!(
                "= {}: packed again as other bytes; the catalog keeps the signed ones (a new \
                 version publishes the rebuild)",
                rebuilt.path
            )
        }));
        lines
    }

    /// What the targets `new` changed since `old`, the targets they were
    /// signed after, if any: the paths added and taken out, those newly
    /// listed as verified, and those whose withdrawal is new or other.
    /// Nothing is rebuilt: that is known only where a bundle is packed.
    #[must_use]
    pub fn between(old: Option<&Targets>, new: &Targets) -> Changes {
        let empty = std::collections::HashMap::new();
        let before = old.map_or(&empty, |old| &old.targets);
        let text = |target: &TargetFile, key: &str| {
            target.custom.as_ref().and_then(|c| c.get(key)).cloned().unwrap_or(Value::Null)
        };
        let mut changes = Changes::default();
        for (path, target) in &new.targets {
            let Some(was) = before.get(path) else {
                changes.added.insert(path.clone(), target_json(target));
                continue;
            };
            if text(target, "owner") == "verified" && text(was, "owner") != "verified" {
                changes.verified.push(path.clone());
            }
            let withdrawn = text(target, "withdrawn");
            if !withdrawn.is_null() && withdrawn != text(was, "withdrawn") {
                changes.withdrawn.insert(path.clone(), withdrawn);
            }
        }
        changes.removed =
            before.keys().filter(|p| !new.targets.contains_key(*p)).cloned().collect();
        changes.removed.sort();
        changes.verified.sort();
        changes
    }
}

/// A target as targets lists it: length, hashes and custom fields.
fn target_json(target: &TargetFile) -> Value {
    let hashes: serde_json::Map<String, Value> = target
        .hashes
        .0
        .iter()
        .map(|(algorithm, hex)| (algorithm.clone(), hex.as_str().into()))
        .collect();
    let mut info = json!({ "length": target.length, "hashes": hashes });
    if let Some(custom) = &target.custom {
        info["custom"] = custom.clone();
    }
    info
}

/// The SHA-256 of `data`, in hex.
fn sha256_of_data(data: &[u8]) -> String {
    repo::file_info(data)["hashes"]["sha256"].as_str().unwrap_or_default().to_string()
}

/// The SHA-256 of a file listed with `hashes`, in hex, or "" when it
/// lists none.
fn sha256_of(hashes: &tuf::metadata::Hashes) -> String {
    hashes
        .0
        .iter()
        .find(|(algorithm, _)| algorithm == "sha256")
        .map_or_else(String::new, |(_, hex)| hex.clone())
}

/// A bundle's target: its path, `bundles/ID-VERSION.dkv`, and what
/// targets lists for it: length, SHA-256, and its ID and version. A
/// bundle with a line is listed by its full ID, `ID@LINE`, so that its
/// updates stay in its line (ADR 0387). A `verified` bundle is one whose
/// ID's publisher the catalog vouches for: its custom fields say
/// `"owner": "verified"` (ADR 0592).
///
/// # Errors
///
/// If `data` is not a bundle with an ID and version.
pub fn bundle_target(data: &[u8], verified: bool) -> Result<(String, Value), String> {
    let manifest = read_manifest(data)?;
    let field = |name: &str| {
        manifest[name].as_str().map(str::to_string).ok_or(format!("manifest.json has no {name}"))
    };
    let (id, version) = (field("id")?, field("version")?);
    let safe = |text: &str| {
        !text.is_empty()
            && !text.starts_with('.')
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+~".contains(&b))
    };
    if !safe(&id) || !safe(&version) {
        return Err(format!("{id} {version} cannot name a file"));
    }
    let mut info = repo::file_info(data);
    let full = match manifest["line"].as_str() {
        Some(line) if safe(line) => format!("{id}@{line}"),
        Some(line) => return Err(format!("{id} {version}: line {line} cannot name a bundle")),
        None => id.clone(),
    };
    let mut custom = json!({ "id": full, "version": version });
    if let Ok(name) = field("name") {
        custom["name"] = name.into();
    }
    if verified {
        custom["owner"] = "verified".into();
    }
    info["custom"] = custom;
    Ok((format!("bundles/{id}-{version}.{}", dkv::EXTENSION), info))
}

fn read_manifest(data: &[u8]) -> Result<Value, String> {
    let archive = dkv::zip::Archive::read(data)?;
    let entry = archive.file(dkv::MANIFEST).ok_or("no manifest.json")?;
    serde_json::from_slice(&archive.read_small(entry, 1 << 20)?)
        .map_err(|e| format!("manifest.json: {e}"))
}

impl Repo {
    /// The repository in `dir`, the source tree's `catalog/`.
    #[must_use]
    pub fn new(dir: &Path) -> Repo {
        Repo { dir: dir.to_path_buf() }
    }

    /// Where the public keys are.
    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.dir.join("keys")
    }

    fn metadata(&self, name: &str) -> PathBuf {
        self.dir.join("metadata").join(name)
    }

    /// The newest root, if there is one yet.
    ///
    /// # Errors
    ///
    /// If it cannot be read or is not root metadata.
    pub fn root(&self) -> Result<Option<Loaded<Root>>, String> {
        read(&self.metadata("root.json"))
    }

    /// The targets, if any have been signed.
    ///
    /// # Errors
    ///
    /// If they cannot be read or are not targets metadata.
    pub fn targets(&self) -> Result<Option<Loaded<Targets>>, String> {
        read(&self.metadata("targets.json"))
    }

    /// Makes and signs the next root, from the keys in `keys/`: each role
    /// signed for by every key that names it, any one of them alone. It is
    /// signed by every key that `signers` can sign for of the old root's
    /// root keys and the new one's, and must be signed by enough of both.
    ///
    /// # Errors
    ///
    /// If a role has no key, or it cannot be signed by enough keys.
    pub fn sign_root(&self, signers: &Signers, days: i64) -> Result<u64, String> {
        let keys = keys::read_all(&self.keys_dir())?;
        let old = self.root()?;
        let version = old.as_ref().map_or(1, |old| old.signed.version + 1);
        let mut roles = Vec::new();
        for role in ROLES {
            let role_keys: Vec<_> =
                keys.iter().filter(|k| k.signs_for(role)).map(|k| &k.key).collect();
            if role_keys.is_empty() {
                return Err(format!("no key in {} signs for {role}", self.keys_dir().display()));
            }
            roles.push((role, role_keys, 1));
        }
        let signed = repo::root(version, &repo::expires_in(days), &roles, true);
        // The old root's root keys sign too, those still in keys/ through
        // what holds them, any others only through a file given.
        let mut eligible: Vec<KeyFile> =
            keys.iter().filter(|k| k.signs_for("root")).cloned().collect();
        if let Some(old) = &old {
            for id in &old.signed.signed.roles["root"].keyids {
                let Some(key) = old.signed.signed.keys.get(id) else { continue };
                if !keys.iter().any(|k| &k.key == key) {
                    let name = id.clone();
                    eligible.push(KeyFile { name, roles: vec![], yubikey: None, key: key.clone() });
                }
            }
        }
        let found: Vec<Box<dyn Signer + '_>> =
            eligible.iter().filter_map(|k| signers.for_key(k)).collect();
        if found.is_empty() {
            return Err(
                "nothing here can sign for root: plug in a YubiKey that holds a root key".into()
            );
        }
        let refs: Vec<&dyn Signer> = found.iter().map(AsRef::as_ref).collect();
        let bytes = repo::sign(&signed, &refs)?;
        let new = Signed::<Root>::parse(&bytes).map_err(|e| e.to_string())?;
        new.verified_by(&new.signed, "root").map_err(|e| format!("the new root: {e}"))?;
        if let Some(old) = &old {
            new.verified_by(&old.signed.signed, "root")
                .map_err(|e| format!("the old root's keys: {e}"))?;
        }
        write(&self.metadata(&format!("{version}.root.json")), &bytes)?;
        write(&self.metadata("root.json"), &bytes)?;
        Ok(version)
    }

    /// What signing the bundles in `bundles` and taking out `remove`
    /// would change in targets. A bundle whose ID is one of `verified` is
    /// listed as one whose publisher the catalog vouches for. A bundle
    /// whose path is listed already as other bytes goes in
    /// [`Changes::rebuilt`] and is not signed: a version, once signed, is
    /// never signed again as something else.
    ///
    /// # Errors
    ///
    /// If a bundle cannot be read, or a path to take out is not listed.
    pub fn changes(
        &self,
        bundles: &[PathBuf],
        remove: &[String],
        verified: &[String],
    ) -> Result<Changes, String> {
        let listed = self.targets()?.map_or_default(|t| t.signed.signed.targets);
        let mut changes = Changes::default();
        for file in bundles {
            let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let named = |e: String| format!("{}: {e}", file.display());
            let id = read_manifest(&data).map_err(named)?["id"].as_str().map(str::to_string);
            let verified = id.is_some_and(|id| verified.contains(&id));
            let (path, info) = bundle_target(&data, verified).map_err(named)?;
            match listed.get(&path) {
                Some(old)
                    if old.length == data.len() as u64
                        && tuf::metadata::check_file(
                            &data,
                            Some(old.length),
                            Some(&old.hashes),
                        )
                        .is_ok() => {}
                Some(old) => {
                    let custom = &info["custom"];
                    let field = |key: &str| custom[key].as_str().unwrap_or_default().to_string();
                    changes.rebuilt.push(Rebuilt {
                        path,
                        file: file.clone(),
                        id: field("id"),
                        version: field("version"),
                        signed_sha256: sha256_of(&old.hashes),
                        packed_sha256: sha256_of_data(&data),
                    });
                }
                None => {
                    changes.added.insert(path, info);
                }
            }
        }
        for path in remove {
            if !listed.contains_key(path) {
                return Err(format!("{path} is not listed"));
            }
            changes.removed.push(path.clone());
        }
        for (path, target) in &listed {
            let custom = target.custom.as_ref();
            let text = |key: &str| custom.and_then(|c| c.get(key)?.as_str());
            if text("id").is_some_and(|id| verified.iter().any(|v| v == id))
                && text("owner") != Some("verified")
                && !changes.added.contains_key(path)
                && !remove.contains(path)
            {
                changes.verified.push(path.clone());
            }
        }
        Ok(changes)
    }

    /// What withdrawing version `version` of the bundle whose full ID, as a
    /// target's `custom.id` has it, is `id`, as `kind` (`broken` or
    /// `malicious`), since `since` seconds after 1970, would change in
    /// targets. The target stays listed (ADR 0593).
    ///
    /// # Errors
    ///
    /// If no target lists that ID and version, `kind` is neither word, the
    /// reason is empty, has control characters or is over [`REASON_MAX`]
    /// bytes, or the version is withdrawn already as `kind` or worse: a
    /// malicious withdrawal is never made broken again.
    pub fn withdraw(
        &self,
        id: &str,
        version: &str,
        kind: &str,
        reason: &str,
        since: u64,
    ) -> Result<Changes, String> {
        let rank = |word: &str| WITHDRAWAL_KINDS.iter().position(|k| *k == word);
        let new_rank = rank(kind).ok_or(format!("{kind:?} is neither broken nor malicious"))?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err("the reason is empty: say why the version is withdrawn".into());
        }
        if reason.chars().any(char::is_control) {
            return Err("the reason has control characters: give it as one line of text".into());
        }
        if reason.len() > REASON_MAX {
            return Err(format!(
                "the reason is {} bytes; at most {REASON_MAX} are kept",
                reason.len()
            ));
        }
        let listed = self.targets()?.map_or_default(|t| t.signed.signed.targets);
        let mut changes = Changes::default();
        for (path, target) in &listed {
            let custom = target.custom.as_ref();
            let text = |key: &str| custom.and_then(|c| c.get(key)?.as_str());
            if text("id") != Some(id) || text("version") != Some(version) {
                continue;
            }
            // A field that does not read counts as broken, as clients read
            // it, so that it can still be made malicious.
            let old_rank = custom
                .and_then(|c| c.get("withdrawn"))
                .map(|w| w.get("kind").and_then(Value::as_str).and_then(rank).unwrap_or_default());
            if let Some(old) = old_rank.filter(|old| *old >= new_rank) {
                return Err(format!(
                    "{id} {version} is withdrawn already as {}; it cannot be withdrawn as {kind}",
                    WITHDRAWAL_KINDS[old]
                ));
            }
            let withdrawal = json!({ "kind": kind, "reason": reason, "since": since });
            changes.withdrawn.insert(path.clone(), withdrawal);
        }
        if changes.withdrawn.is_empty() {
            return Err(format!("no target lists {id} {version}"));
        }
        Ok(changes)
    }

    /// Signs the next targets with `changes`, with one key: one touch of a
    /// YubiKey for the whole batch. The new bundles' bytes are put in
    /// `store`, to be published. The targets delegate the list of flaws
    /// and the rollouts to the online key.
    ///
    /// # Errors
    ///
    /// If there is no root yet, nothing can sign for targets, or a file
    /// cannot be written.
    pub fn sign_targets(
        &self,
        changes: &Changes,
        bundles: &[PathBuf],
        store: &Path,
        signers: &Signers,
        days: i64,
    ) -> Result<u64, String> {
        let root = self.root()?.ok_or("there is no root yet: make one with `root` first")?;
        let keys = keys::read_all(&self.keys_dir())?;
        let ids = &root.signed.signed.roles["targets"].keyids;
        let signer = keys
            .iter()
            .filter(|k| ids.contains(&k.key.id()))
            .find_map(|k| signers.for_key(k))
            .ok_or("nothing here can sign for targets: plug in one of the catalog's YubiKeys")?;
        let old = self.targets()?;
        let version = old.as_ref().map_or(1, |t| t.signed.version + 1);
        let mut listed: BTreeMap<String, Value> = match &old {
            Some(old) => {
                let file: Value = serde_json::from_slice(&old.bytes).map_err(|e| e.to_string())?;
                serde_json::from_value(file["signed"]["targets"].clone())
                    .map_err(|e| e.to_string())?
            }
            None => BTreeMap::new(),
        };
        for path in &changes.removed {
            listed.remove(path);
        }
        for file in bundles {
            let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let (path, info) = bundle_target(&data, false)?;
            if let Some(listed) = changes.added.get(&path) {
                // The bytes stored are those the changes list, not a file
                // packed again since they were read.
                if listed["length"] != info["length"] || listed["hashes"] != info["hashes"] {
                    return Err(format!(
                        "{}: other bytes than those listed for {path}: list the changes again",
                        file.display()
                    ));
                }
                write(&store.join(&path), &data)?;
            }
        }
        listed.extend(changes.added.clone());
        for path in &changes.verified {
            if let Some(target) = listed.get_mut(path) {
                target["custom"]["owner"] = "verified".into();
            }
        }
        for (path, withdrawal) in &changes.withdrawn {
            if let Some(target) = listed.get_mut(path) {
                target["custom"]["withdrawn"] = withdrawal.clone();
            }
        }
        let mut signed = repo::targets(version, &repo::expires_in(days), &listed);
        signed["delegations"] = online_delegations(&root.signed.signed, &keys)?;
        let bytes = repo::sign(&signed, &[signer.as_ref()])?;
        let new = Signed::<Targets>::parse(&bytes).map_err(|e| e.to_string())?;
        new.verified_by(&root.signed.signed, "targets").map_err(|e| e.to_string())?;
        write(&self.metadata("targets.json"), &bytes)?;
        Ok(version)
    }

    /// Signs the catalog's list of known flaws anew (ADR 0389): those that
    /// `advisories` know of in the components each bundle's SBOM lists,
    /// for every bundle the targets list, whose bytes are in `store`. The
    /// flaws role, with the online key, lists the list; both go in
    /// `store`, for [`Repo::publish`] to serve. Returns the role's version
    /// and how many flaws it lists.
    ///
    /// # Errors
    ///
    /// If the targets do not delegate the flaws, a bundle or its SBOM
    /// cannot be read, or nothing here signs for the online key.
    pub fn sign_flaws(
        &self,
        store: &Path,
        signers: &Signers,
        advisories: &[dkv::flaws::Advisory],
        days: i64,
    ) -> Result<(u64, usize), String> {
        let root = self.root()?.ok_or("there is no root yet")?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        if targets.signed.signed.delegations.is_none() {
            return Err("the targets delegate no flaws: sign the targets again".into());
        }
        let keys = keys::read_all(&self.keys_dir())?;
        let signer = online_signer(&root, &keys, signers, "timestamp")?;
        let mut list = dkv::flaws::List {
            made: tuf::time::format(tuf::time::now()),
            ..dkv::flaws::List::default()
        };
        for (path, target) in &targets.signed.signed.targets {
            let custom = target.custom.as_ref();
            let text = |key: &str| custom.and_then(|c| c.get(key)?.as_str().map(String::from));
            let (Some(id), Some(version)) = (text("id"), text("version")) else { continue };
            let file = std::fs::File::open(store.join(path)).map_err(|e| format!("{path}: {e}"))?;
            let archive = dkv::zip::Archive::read(&file).map_err(|e| format!("{path}: {e}"))?;
            let Some(entry) = archive.file(dkv::sbom::SBOM) else { continue };
            let sbom = archive
                .read_small(entry, dkv::sbom::SBOM_MAX)
                .map_err(|e| format!("{path}: {}: {e}", dkv::sbom::SBOM))?;
            let sbom = String::from_utf8_lossy(&sbom);
            list.flaws.extend(dkv::flaws::find(&id, &version, &sbom, advisories));
            list.checked.push((id, version));
        }
        let count = list.flaws.len();
        let list = list.write();
        write(&store.join(FLAWS_TARGET), &list)?;
        let old = std::fs::read(store.join(FLAWS_METADATA)).ok();
        let version = old
            .and_then(|bytes| Signed::<Targets>::parse(&bytes).ok())
            .map_or(1, |old| old.version + 1);
        let listed = BTreeMap::from([(FLAWS_TARGET.to_string(), repo::file_info(&list))]);
        let signed = repo::targets(version, &repo::expires_in(days), &listed);
        let bytes = repo::sign(&signed, &[signer.as_ref()])?;
        let new = Signed::<Targets>::parse(&bytes).map_err(|e| e.to_string())?;
        new.verified_by(&targets.signed.signed, FLAWS_ROLE).map_err(|e| e.to_string())?;
        write(&store.join(FLAWS_METADATA), &bytes)?;
        Ok((version, count))
    }

    /// Signs the rollout of the newest version of each bundle the targets
    /// list (ADR 0544), as `plans` hold them at `now`, with the online
    /// key: `rollout/ID/VERSION.json`, the highest wave open, which
    /// [`rollout::next`] gives. A version seen for the first time is added
    /// to `plans`, released `now`, and the plans of versions that are no
    /// longer the newest are dropped. The rollouts go in `store`, and
    /// the rollout role that lists them beside them, signed anew only when
    /// what it lists changed or it has less than twelve hours left, for
    /// [`Repo::publish`] to serve. Returns the role's version, or none
    /// when the targets delegate no rollouts, as those of a catalog signed
    /// before them do not: every machine then takes its updates as it did.
    /// Without `plans`, every version has every wave open: targets that
    /// delegate the rollouts need them signed, or no computer reads the
    /// catalog, and a server not yet given a plan must not stop it.
    ///
    /// # Errors
    ///
    /// If there is no root or targets yet, a file cannot be written,
    /// nothing here signs for the online key, or the role does not check
    /// against the targets.
    pub fn sign_rollouts(
        &self,
        store: &Path,
        signers: &Signers,
        plans: Option<&mut Plans>,
        now: i64,
    ) -> Result<Option<u64>, String> {
        let root = self.root()?.ok_or("there is no root yet")?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        if targets.signed.signed.role(rollout::ROLE).is_none() {
            return Ok(None);
        }
        let keys = keys::read_all(&self.keys_dir())?;
        let signer = online_signer(&root, &keys, signers, "timestamp")?;
        let role = store.join(ROLLOUT_METADATA);
        let old = read::<Targets>(&role)?;
        // Before the first rollouts every version reached every computer
        // the day it was listed: those listed then start with every wave
        // open, and only the versions listed later go through the waves.
        let mut unplanned = Plans::default();
        let (plans, first) = match plans {
            Some(plans) if old.is_some() => (plans, 0),
            Some(plans) => (plans, rollout::LAST),
            None => (&mut unplanned, rollout::LAST),
        };
        let mut listed = BTreeMap::new();
        let mut newest = Vec::new();
        for (id, version) in newest_versions(&targets.signed.signed) {
            let release = format!("{id}/{version}");
            let plan = plans.releases.entry(release.clone()).or_insert(Plan {
                released: now,
                open: first,
                paused: None,
            });
            let signed = rollout::next(&release, plan, &plans.red, now);
            plan.open = signed.wave;
            let mut bytes = serde_json::to_vec_pretty(&signed).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            let path = rollout::target_path(&release);
            write(&store.join(&path), &bytes)?;
            listed.insert(path, repo::file_info(&bytes));
            newest.push(release);
        }
        plans.releases.retain(|release, _| newest.contains(release));
        if let Some(old) = &old {
            // The role is kept only while it follows from these targets: a
            // new online key makes the delegation another.
            let file: Value = serde_json::from_slice(&old.bytes).map_err(|e| e.to_string())?;
            if file["signed"]["targets"] == json!(listed)
                && old.signed.expires > now + ROLLOUT_MARGIN
                && old.signed.verified_by(&targets.signed.signed, rollout::ROLE).is_ok()
            {
                return Ok(Some(old.signed.version));
            }
        }
        let version = old.map_or(1, |old| old.signed.version + 1);
        let expires = tuf::time::format(now + ROLLOUT_DAYS * 86_400);
        let signed = repo::targets(version, &expires, &listed);
        let bytes = repo::sign(&signed, &[signer.as_ref()])?;
        let new = Signed::<Targets>::parse(&bytes).map_err(|e| e.to_string())?;
        new.verified_by(&targets.signed.signed, rollout::ROLE)
            .map_err(|e| format!("the rollouts do not check against the targets: {e}"))?;
        write(&role, &bytes)?;
        Ok(Some(version))
    }

    /// Lays out in `out` what a server serves, with consistent snapshots:
    /// every root by its version, the targets by theirs, a new snapshot
    /// and timestamp signed by `signers`' online key, and each target
    /// from `store` by its SHA-256, the flaws and the rollouts among them
    /// once the server has signed them and while they are good. The
    /// client then reads it all back from `out`, starting from the first
    /// root, as a new computer does.
    ///
    /// # Errors
    ///
    /// If there is no root or targets yet, a target is missing or not
    /// what targets says, nothing here signs for snapshot and timestamp,
    /// or the client refuses what was made.
    pub fn publish(
        &self,
        store: &Path,
        out: &Path,
        signers: &Signers,
        days: (i64, i64),
    ) -> Result<(u64, u64), String> {
        let root = self.root()?.ok_or("there is no root yet")?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        let keys = keys::read_all(&self.keys_dir())?;
        let (snapshot_signer, timestamp_signer) = (
            online_signer(&root, &keys, signers, "snapshot")?,
            online_signer(&root, &keys, signers, "timestamp")?,
        );
        for version in 1..=root.signed.version {
            let name = format!("{version}.root.json");
            let bytes = std::fs::read(self.metadata(&name)).map_err(|e| format!("{name}: {e}"))?;
            write(&out.join("metadata").join(&name), &bytes)?;
        }
        write(&out.join("metadata/root.json"), &root.bytes)?;
        let targets_version = targets.signed.version;
        write(&out.join(format!("metadata/{targets_version}.targets.json")), &targets.bytes)?;
        for (path, target) in &targets.signed.signed.targets {
            serve_target(store, out, path, target)?;
        }
        let previous: Option<Value> = std::fs::read(out.join("metadata/timestamp.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let (timestamp_version, snapshot_version) = match &previous {
            Some(file) => (
                file["signed"]["version"].as_u64().unwrap_or(0) + 1,
                file["signed"]["meta"]["snapshot.json"]["version"].as_u64().unwrap_or(0) + 1,
            ),
            None => (1, 1),
        };
        let mut meta = BTreeMap::from([(
            "targets.json".to_string(),
            repo::meta(targets_version, &targets.bytes),
        )]);
        // The list of flaws, once the server has signed one for these
        // targets, while it is good (ADR 0389).
        if let Some((parsed, bytes)) =
            &online_role(store, FLAWS_METADATA, FLAWS_ROLE, &targets.signed.signed)
        {
            let target =
                parsed.signed.targets.get(FLAWS_TARGET).ok_or("the flaws role lists no list")?;
            serve_target(store, out, FLAWS_TARGET, target)?;
            write(&out.join(format!("metadata/{}.{FLAWS_ROLE}.json", parsed.version)), bytes)?;
            meta.insert(format!("{FLAWS_ROLE}.json"), repo::meta(parsed.version, bytes));
        }
        // The rollouts, on the same terms (ADR 0544).
        let rollouts = online_role(store, ROLLOUT_METADATA, rollout::ROLE, &targets.signed.signed);
        if let Some((parsed, bytes)) = &rollouts {
            for (path, target) in &parsed.signed.targets {
                serve_target(store, out, path, target)?;
            }
            write(&out.join(format!("metadata/{}.{}.json", parsed.version, rollout::ROLE)), bytes)?;
            meta.insert(format!("{}.json", rollout::ROLE), repo::meta(parsed.version, bytes));
        }
        let snapshot = repo::sign(
            &repo::snapshot(snapshot_version, &repo::expires_in(days.0), &meta),
            &[snapshot_signer.as_ref()],
        )?;
        write(&out.join(format!("metadata/{snapshot_version}.snapshot.json")), &snapshot)?;
        let timestamp = repo::sign(
            &repo::timestamp(
                timestamp_version,
                &repo::expires_in(days.1),
                snapshot_version,
                &snapshot,
            ),
            &[timestamp_signer.as_ref()],
        )?;
        write(&out.join("metadata/timestamp.json"), &timestamp)?;
        let rollouts = rollouts.as_ref().map(|(parsed, _)| &parsed.signed);
        check_served(out, &targets.signed.signed, rollouts)?;
        Ok((snapshot_version, timestamp_version))
    }

    /// Keeps what `out` serves current, as the update server does every
    /// six hours: a new timestamp alone while `out` serves this root, a
    /// snapshot of these targets and the flaws and rollouts of `store`,
    /// good for more than [`RENEW_DAYS`], and everything again, as
    /// [`Repo::publish`], otherwise.
    ///
    /// # Errors
    ///
    /// As [`Repo::publish`].
    pub fn refresh(
        &self,
        store: &Path,
        out: &Path,
        signers: &Signers,
        days: (i64, i64),
    ) -> Result<Refreshed, String> {
        let root = self.root()?.ok_or("there is no root yet")?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        let flaws = online_role(store, FLAWS_METADATA, FLAWS_ROLE, &targets.signed.signed);
        let rollouts = online_role(store, ROLLOUT_METADATA, rollout::ROLE, &targets.signed.signed);
        if let Some(served) = Served::read(out)
            && served.root == root.bytes
            && served.targets_version == Some(targets.signed.version)
            && served.flaws_version == flaws.map(|(flaws, _)| flaws.version)
            && served.rollout_version == rollouts.as_ref().map(|(role, _)| role.version)
            && served.snapshot_expires.is_some_and(|t| t - tuf::time::now() > RENEW_DAYS * 86_400)
        {
            let keys = keys::read_all(&self.keys_dir())?;
            let signer = online_signer(&root, &keys, signers, "timestamp")?;
            let version = served.timestamp_version + 1;
            let timestamp = repo::sign(
                &repo::timestamp(
                    version,
                    &repo::expires_in(days.1),
                    served.snapshot_version,
                    &served.snapshot,
                ),
                &[signer.as_ref()],
            )?;
            write(&out.join("metadata/timestamp.json"), &timestamp)?;
            let rollouts = rollouts.as_ref().map(|(role, _)| &role.signed);
            check_served(out, &targets.signed.signed, rollouts)?;
            return Ok(Refreshed::Timestamp(version));
        }
        let (snapshot, timestamp) = self.publish(store, out, signers, days)?;
        Ok(Refreshed::All { snapshot, timestamp })
    }

    /// Reads the catalog served at `url` with the client, from this
    /// repository's first root, as a new computer would, and fetches
    /// every target these targets list. Returns how many there are.
    ///
    /// # Errors
    ///
    /// If the client refuses what is served or cannot fetch a target.
    pub fn check_url(&self, url: &str) -> Result<usize, String> {
        let first =
            std::fs::read(self.metadata("1.root.json")).map_err(|e| format!("1.root.json: {e}"))?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        let url = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
        let base = tuf::Url::parse(&url).map_err(|e| format!("{url}: {e}"))?;
        check_at(&base, &first, &targets.signed.signed, None)?;
        Ok(targets.signed.signed.targets.len())
    }

    /// Reads the catalog served at `url` with the client, from this
    /// repository's first root, as a new computer would, and says what it
    /// serves: each role's version and expiry, every bundle with its
    /// known flaws, and the rollouts. Unlike [`Repo::check_url`] it
    /// fetches no bundle, only the metadata, the list of flaws and the
    /// rollouts, and every signature is checked as usual. The result is
    /// JSON in the shape `catalog-repo served --json` prints.
    ///
    /// # Errors
    ///
    /// If there is no first root, or the client refuses what is served or
    /// cannot fetch a target.
    pub fn served(&self, url: &str) -> Result<Value, String> {
        let first =
            std::fs::read(self.metadata("1.root.json")).map_err(|e| format!("1.root.json: {e}"))?;
        read_served(url, &first)
    }
}

/// How many days before the served snapshot expires `refresh` signs a new
/// one, though targets have not changed.
pub const RENEW_DAYS: i64 = 2;

/// What [`Repo::refresh`] signed.
#[derive(Debug, PartialEq, Eq)]
pub enum Refreshed {
    /// A timestamp alone, of this version.
    Timestamp(u64),
    /// Everything again, as `publish`: these versions of snapshot and
    /// timestamp.
    All {
        /// The snapshot's version.
        snapshot: u64,
        /// The timestamp's version.
        timestamp: u64,
    },
}

/// What a published repository serves now, as `refresh` reads it.
struct Served {
    root: Vec<u8>,
    timestamp_version: u64,
    snapshot_version: u64,
    snapshot: Vec<u8>,
    targets_version: Option<u64>,
    flaws_version: Option<u64>,
    rollout_version: Option<u64>,
    snapshot_expires: Option<i64>,
}

impl Served {
    fn read(out: &Path) -> Option<Served> {
        let metadata = out.join("metadata");
        let timestamp: Value =
            serde_json::from_slice(&std::fs::read(metadata.join("timestamp.json")).ok()?).ok()?;
        let snapshot_version = timestamp["signed"]["meta"]["snapshot.json"]["version"].as_u64()?;
        let snapshot =
            std::fs::read(metadata.join(format!("{snapshot_version}.snapshot.json"))).ok()?;
        let parsed: Value = serde_json::from_slice(&snapshot).ok()?;
        Some(Served {
            root: std::fs::read(metadata.join("root.json")).ok()?,
            timestamp_version: timestamp["signed"]["version"].as_u64()?,
            snapshot_version,
            targets_version: parsed["signed"]["meta"]["targets.json"]["version"].as_u64(),
            flaws_version: parsed["signed"]["meta"][format!("{FLAWS_ROLE}.json")]["version"]
                .as_u64(),
            rollout_version: parsed["signed"]["meta"][format!("{}.json", rollout::ROLE)]["version"]
                .as_u64(),
            snapshot_expires: parsed["signed"]["expires"].as_str().and_then(tuf::time::parse),
            snapshot,
        })
    }
}

/// The online key's role `role`, kept in `store` as `file`, that
/// [`Repo::publish`] serves with `targets`, with its bytes: once the
/// server has signed one, while it follows from these targets and has
/// not expired. A list of flaws a week old is left out, not served: the
/// catalog stays good, and clients say they cannot tell the flaws (ADR
/// 0389); rollouts a day old are, and every machine then takes its
/// updates at once (ADR 0544).
fn online_role(
    store: &Path,
    file: &str,
    role: &str,
    targets: &Targets,
) -> Option<(Signed<Targets>, Vec<u8>)> {
    let bytes = std::fs::read(store.join(file)).ok()?;
    let parsed = Signed::<Targets>::parse(&bytes).ok()?;
    parsed.verified_by(targets, role).ok()?;
    (!parsed.expired(tuf::time::now())).then_some((parsed, bytes))
}

/// Writes the target `path`, as `target` lists it, from `store` into what
/// `out` serves, named by its SHA-256 as consistent snapshots name files.
fn serve_target(store: &Path, out: &Path, path: &str, target: &TargetFile) -> Result<(), String> {
    let data = std::fs::read(store.join(path)).map_err(|e| format!("{path}: {e}"))?;
    tuf::metadata::check_file(&data, Some(target.length), Some(&target.hashes))
        .map_err(|e| format!("{path} in the store: {e}"))?;
    let (_, sha256) = target
        .hashes
        .0
        .iter()
        .find(|(name, _)| name == "sha256")
        .ok_or(format!("{path} has no SHA-256"))?;
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let served =
        if dir.is_empty() { format!("{sha256}.{name}") } else { format!("{dir}/{sha256}.{name}") };
    write(&out.join("targets").join(served), &data)
}

/// The newest version of each bundle `targets` list, by full ID, as a
/// client reads them (ADR 0383): of two versions that order alike, the
/// one whose path comes last.
fn newest_versions(targets: &Targets) -> BTreeMap<String, String> {
    let by_path: BTreeMap<_, _> = targets.targets.iter().collect();
    let mut newest: BTreeMap<String, String> = BTreeMap::new();
    for target in by_path.values() {
        let custom = target.custom.as_ref();
        let text = |key: &str| custom.and_then(|c| c.get(key)?.as_str());
        let (Some(id), Some(version)) = (text("id"), text("version")) else { continue };
        match newest.get_mut(id) {
            Some(held) if dkv::flaws::compare(held, version).is_gt() => {}
            Some(held) => *held = version.to_string(),
            None => {
                newest.insert(id.to_string(), version.to_string());
            }
        }
    }
    newest
}

/// The targets' delegation of the list of flaws and of the rollouts to
/// the online key, the key `root` gives timestamp (ADRs 0389 and 0544):
/// `flaws/` and [`ROLLOUT_PATHS`] alone, and no further.
fn online_delegations(root: &Root, keys: &[KeyFile]) -> Result<Value, String> {
    let ids = &root.roles["timestamp"].keyids;
    let online = keys.iter().find(|k| ids.contains(&k.key.id())).ok_or(
        "keys/ has no file of the online key, which signs the list of flaws and the rollouts",
    )?;
    let id = online.key.id();
    let role = |name: &str, paths: &str| {
        json!({
            "name": name,
            "keyids": [id],
            "threshold": 1,
            "paths": [paths],
            "terminating": true,
        })
    };
    Ok(json!({
        "keys": { id.clone(): online.key },
        "roles": [role(FLAWS_ROLE, "flaws/*"), role(rollout::ROLE, ROLLOUT_PATHS)],
    }))
}

/// What signs for `role`, one of the online roles: a key of `root`'s for
/// it that `signers` holds.
fn online_signer<'a>(
    root: &Loaded<Root>,
    keys: &'a [KeyFile],
    signers: &'a Signers,
    role: &str,
) -> Result<Box<dyn Signer + 'a>, String> {
    let ids = &root.signed.signed.roles[role].keyids;
    keys.iter()
        .filter(|k| ids.contains(&k.key.id()))
        .find_map(|k| signers.for_key(k))
        .ok_or(format!("nothing here can sign for {role}: give the online key's file"))
}

/// Reads `out` with the client, from its first root, as a new computer
/// would, and finds every target there, and each the rollout role
/// `rollouts` lists.
fn check_served(out: &Path, targets: &Targets, rollouts: Option<&Targets>) -> Result<(), String> {
    let first = std::fs::read(out.join("metadata/1.root.json")).map_err(|e| e.to_string())?;
    let absolute = std::fs::canonicalize(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let base =
        tuf::Url::parse(&format!("file://{}/", absolute.display())).map_err(|e| e.to_string())?;
    check_at(&base, &first, targets, rollouts)
}

/// A new folder for the client's metadata while it reads a repository,
/// one for each read, since reads may run at once.
fn client_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("catalog-repo-check-{}-{n}", std::process::id()))
}

/// Reads the repository at `base` with the client, from the root `first`,
/// in a folder of its own, and fetches every target `targets` lists, and
/// each the rollout role `rollouts` lists.
fn check_at(
    base: &tuf::Url,
    first: &[u8],
    targets: &Targets,
    rollouts: Option<&Targets>,
) -> Result<(), String> {
    let client = client_dir();
    let client = client.as_path();
    let _ = std::fs::remove_dir_all(client);
    tuf::init(client, first).map_err(|e| e.to_string())?;
    let metadata = base.resolve("metadata/").map_err(|e| e.to_string())?;
    let served = base.resolve("targets/").map_err(|e| e.to_string())?;
    let result = (|| {
        let mut updater =
            tuf::Updater::new(client, &metadata, Some(&served)).map_err(|e| e.to_string())?;
        updater.refresh().map_err(|e| format!("the client refuses {base}: {e}"))?;
        // The list of flaws, once the server has signed one (ADR 0389).
        let listed = updater
            .trusted()
            .snapshot()
            .is_some_and(|s| s.signed.meta.contains_key(&format!("{FLAWS_ROLE}.json")));
        let flaws = (targets.delegations.is_some() && listed).then_some(FLAWS_TARGET);
        let rolled = rollouts.into_iter().flat_map(|role| role.targets.keys().map(String::as_str));
        for path in targets.targets.keys().map(String::as_str).chain(flaws).chain(rolled) {
            let target = updater
                .target(path)
                .map_err(|e| e.to_string())?
                .ok_or(format!("the client cannot find {path}"))?;
            let file = client.join("target");
            updater.download_target(path, &target, &file).map_err(|e| format!("{path}: {e}"))?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(client);
    result
}

/// One role's version and expiry as the client trusts it, for `served`.
fn role_json(role: &str, version: u64, expires: i64) -> Value {
    json!({ "role": role, "version": version, "expires": expires })
}

/// Fetches the target `path` that `updater` finds, if it does, into
/// `file`, and reads it.
fn read_target(
    updater: &mut tuf::Updater,
    path: &str,
    file: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let Some(target) = updater.target(path).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    updater.download_target(path, &target, file).map_err(|e| format!("{path}: {e}"))?;
    std::fs::read(file).map(Some).map_err(|e| format!("{path}: {e}"))
}

/// Reads the catalog served at `url` with the client, from the root
/// `first`, as a new computer would, and says what it serves: each
/// role's version and expiry, every bundle with its known flaws, and the
/// rollouts. The result is JSON in the shape `catalog-repo served --json`
/// prints.
fn read_served(url: &str, first: &[u8]) -> Result<Value, String> {
    let url = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
    let base = tuf::Url::parse(&url).map_err(|e| format!("{url}: {e}"))?;
    let client = client_dir();
    let _ = std::fs::remove_dir_all(&client);
    tuf::init(&client, first).map_err(|e| e.to_string())?;
    let result = served_at(&base, &client);
    let _ = std::fs::remove_dir_all(&client);
    result
}

/// [`served`], with the client's folder `client` already made.
fn served_at(base: &tuf::Url, client: &Path) -> Result<Value, String> {
    let metadata = base.resolve("metadata/").map_err(|e| e.to_string())?;
    let targets_url = base.resolve("targets/").map_err(|e| e.to_string())?;
    let mut updater =
        tuf::Updater::new(client, &metadata, Some(&targets_url)).map_err(|e| e.to_string())?;
    updater.refresh().map_err(|e| format!("the client refuses {base}: {e}"))?;
    let file = client.join("target");
    let targets =
        updater.trusted().targets("targets").ok_or("the client holds no targets")?.signed.clone();
    // The list of flaws, once the server has signed one (ADR 0389).
    let listed = updater
        .trusted()
        .snapshot()
        .is_some_and(|s| s.signed.meta.contains_key(&format!("{FLAWS_ROLE}.json")));
    let list = match (targets.delegations.is_some() && listed)
        .then(|| read_target(&mut updater, FLAWS_TARGET, &file))
        .transpose()?
        .flatten()
    {
        Some(bytes) => Some(dkv::flaws::List::read(&bytes)?),
        None => None,
    };
    let mut bundles = Vec::new();
    for (path, target) in &targets.targets {
        let custom = target.custom.as_ref();
        let text = |key: &str| custom.and_then(|c| c.get(key)?.as_str());
        let (Some(id), Some(version)) = (text("id"), text("version")) else { continue };
        let flaws: Vec<Value> = list
            .iter()
            .flat_map(|list| list.of(id, version))
            .map(|f| {
                json!({
                    "id": f.id,
                    "aliases": f.aliases,
                    "severity": f.severity.word(),
                    "summary": f.summary,
                    "component": f.component,
                    "fixed": f.fixed,
                })
            })
            .collect();
        bundles.push(json!({
            "id": id,
            "name": text("name"),
            "version": version,
            "path": path,
            "length": target.length,
            "flaws": flaws,
        }));
    }
    let key = |bundle: &Value| {
        let text = |name: &str| bundle[name].as_str().unwrap_or_default().to_string();
        (text("id"), text("version"))
    };
    bundles.sort_by(|a, b| {
        let ((a_id, a_version), (b_id, b_version)) = (key(a), key(b));
        a_id.cmp(&b_id).then_with(|| dkv::flaws::compare(&a_version, &b_version))
    });
    // The rollout of each bundle's newest version (ADR 0544).
    let mut rollouts = Vec::new();
    for (id, version) in newest_versions(&targets) {
        let path = rollout::target_path(&format!("{id}/{version}"));
        let Some(bytes) = read_target(&mut updater, &path, &file)? else { continue };
        let signed: rollout::Rollout =
            serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))?;
        rollouts.push(json!({
            "bundle": id,
            "version": version,
            "wave": signed.wave,
            "held": signed.held,
        }));
    }
    let trusted = updater.trusted();
    let mut roles = vec![role_json("root", trusted.root().version, trusted.root().expires)];
    if let Some(s) = trusted.timestamp() {
        roles.push(role_json("timestamp", s.version, s.expires));
    }
    if let Some(s) = trusted.snapshot() {
        roles.push(role_json("snapshot", s.version, s.expires));
    }
    for (name, role) in [("targets", "targets"), ("flaws", FLAWS_ROLE), ("rollout", rollout::ROLE)]
    {
        if let Some(s) = trusted.targets(role) {
            roles.push(role_json(name, s.version, s.expires));
        }
    }
    Ok(json!({
        "url": base.to_string(),
        "checked": tuf::time::now(),
        "roles": roles,
        "bundles": bundles,
        "flaws_listed": list.is_some(),
        "rollouts": rollouts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("catalog-repo-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in for tools/yubikey-piv.py, YubiKey 12345678 with a P-256
    /// key that openssl holds, which signs as the YubiKey does: ECDSA with
    /// SHA-256, in DER.
    fn fake_yubikey(dir: &Path) -> PathBuf {
        let openssl = |args: &[&str]| {
            let status = Command::new("openssl").args(args).current_dir(dir).status();
            assert!(
                status.is_ok_and(|s| s.success()),
                "openssl {args:?} failed: the catalog's tests need openssl"
            );
        };
        openssl(&["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "secret.pem"]);
        openssl(&["ec", "-in", "secret.pem", "-pubout", "-out", "public.pem"]);
        let helper = dir.join("yubikey");
        let script = format!(
            "#!/bin/sh\ncd '{}' || exit 1\ncase \"$1 $2\" in\n\
             'list ') echo 12345678 ;;\n\
             'public 12345678') cat public.pem ;;\n\
             'sign 12345678') openssl dgst -sha256 -sign secret.pem | od -An -tx1 | tr -d ' \\n'; echo ;;\n\
             *) exit 1 ;;\nesac\n",
            dir.display()
        );
        std::fs::write(&helper, script).unwrap();
        Command::new("chmod").arg("755").arg(&helper).status().unwrap();
        helper
    }

    fn bundle(dir: &Path, id: &str, version: &str, content: &[u8]) -> PathBuf {
        bundle_in(dir, id, "", version, content)
    }

    /// A bundle of the line `line`, if it is not empty.
    fn bundle_in(dir: &Path, id: &str, line: &str, version: &str, content: &[u8]) -> PathBuf {
        let line = if line.is_empty() { String::new() } else { format!(r#", "line": "{line}""#) };
        let manifest = format!(r#"{{"id": "{id}", "version": "{version}", "name": "Test"{line}}}"#);
        let path = dir.join(format!("{id}-{version}-{}.dkv", content.len()));
        let mut zip = dkv::zip::Writer::new(std::fs::File::create(&path).unwrap());
        let sbom = format!(
            "PackageName: {id}\nExternalRef: PACKAGE-MANAGER purl pkg:cargo/regex@1.10.2\n"
        );
        let files = [
            (dkv::MANIFEST, manifest.as_bytes()),
            ("bin/tool", content),
            (dkv::sbom::SBOM, sbom.as_bytes()),
        ];
        for (name, data) in files {
            zip.add(&dkv::zip::Input { path: name, executable: false, data }).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn a_verified_bundle_is_listed_as_owned() {
        let dir = scratch("verified");
        let file = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let data = std::fs::read(&file).unwrap();
        let (_, plain) = bundle_target(&data, false).unwrap();
        assert_eq!(plain["custom"].get("owner"), None);
        let (_, owned) = bundle_target(&data, true).unwrap();
        assert_eq!(owned["custom"]["owner"], "verified");
        assert_eq!(owned["custom"]["id"], "org.dukovo.ripgrep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_catalog_is_signed_and_published_as_the_client_reads_it() {
        let dir = scratch("catalog");
        let repo = Repo::new(&dir.join("catalog"));
        let helper = fake_yubikey(&dir);
        let yubikey = keys::from_yubikey(&helper, None).unwrap();
        assert_eq!(
            (yubikey.name.as_str(), yubikey.yubikey),
            ("yubikey-12345678", Some(12_345_678))
        );
        yubikey.write(&repo.keys_dir()).unwrap();
        assert!(yubikey.write(&repo.keys_dir()).is_err(), "a key is not written over");
        let roles = vec!["snapshot".to_string(), "timestamp".to_string()];
        let online_file = dir.join("online.key");
        keys::new_file_key("online", roles.clone(), &online_file)
            .unwrap()
            .write(&repo.keys_dir())
            .unwrap();
        assert!(
            keys::new_file_key("again", roles, &online_file).is_err(),
            "a secret is not written over"
        );

        assert!(
            repo.sign_root(&Signers::default(), 365).unwrap_err().contains("plug in a YubiKey")
        );
        let both = Signers::new(Some(&helper), std::slice::from_ref(&online_file)).unwrap();
        assert_eq!(repo.sign_root(&both, 365), Ok(1));

        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let bat = bundle(&dir, "org.dukovo.bat", "0.25.0", b"bat");
        let changes = repo.changes(&[ripgrep.clone(), bat.clone()], &[], &[]).unwrap();
        assert_eq!(changes.lines().len(), 2);
        assert!(
            changes.lines()[1].starts_with(
                "+ bundles/org.dukovo.ripgrep-14.1.1.dkv (org.dukovo.ripgrep 14.1.1, "
            )
        );
        let store = dir.join("store");
        let online_only = Signers::new(None, std::slice::from_ref(&online_file)).unwrap();
        assert!(
            repo.sign_targets(&changes, &[ripgrep.clone(), bat.clone()], &store, &online_only, 182)
                .is_err()
        );
        assert_eq!(
            repo.sign_targets(&changes, &[ripgrep.clone(), bat.clone()], &store, &both, 182),
            Ok(1)
        );
        assert!(repo.changes(std::slice::from_ref(&ripgrep), &[], &[]).unwrap().is_empty());
        // Signed before the catalog vouched for its publisher: the next
        // batch lists it as verified, with no new bytes.
        let verified = vec!["org.dukovo.ripgrep".to_string()];
        assert_eq!(
            repo.changes(&[], &[], &verified).unwrap().lines(),
            vec!["~ bundles/org.dukovo.ripgrep-14.1.1.dkv: publisher verified"]
        );
        let other = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"something else");
        let kept = repo.changes(&[other], &[], &[]).unwrap();
        assert!(kept.is_empty() && kept.rebuilt.len() == 1, "{kept:?}");

        let out = dir.join("site");
        assert_eq!(repo.publish(&store, &out, &online_only, (7, 1)), Ok((1, 1)));
        assert_eq!(repo.publish(&store, &out, &online_only, (7, 1)), Ok((2, 2)));
        assert!(out.join("metadata/1.targets.json").is_file());
        assert!(out.join("metadata/2.snapshot.json").is_file());

        // The server's refresh: a timestamp alone while the snapshot is
        // good for long enough, and everything again once it is not.
        assert_eq!(repo.refresh(&store, &out, &online_only, (7, 1)), Ok(Refreshed::Timestamp(3)));
        assert_eq!(repo.publish(&store, &out, &online_only, (1, 1)), Ok((3, 4)));
        assert_eq!(
            repo.refresh(&store, &out, &online_only, (7, 1)),
            Ok(Refreshed::All { snapshot: 4, timestamp: 5 })
        );

        // A second root, signed by the same YubiKey, and a batch that
        // takes a bundle out, still read back from the first root.
        assert_eq!(repo.sign_root(&both, 365), Ok(2));
        let gone = vec!["bundles/org.dukovo.bat-0.25.0.dkv".to_string()];
        let changes = repo.changes(&[], &gone, &[]).unwrap();
        assert_eq!(changes.lines(), vec!["- bundles/org.dukovo.bat-0.25.0.dkv"]);
        assert_eq!(repo.sign_targets(&changes, &[], &store, &both, 182), Ok(2));
        // A new root or targets: refresh signs everything again.
        assert_eq!(
            repo.refresh(&store, &out, &online_only, (7, 1)),
            Ok(Refreshed::All { snapshot: 5, timestamp: 6 })
        );
        assert!(out.join("metadata/2.root.json").is_file());
        let targets = repo.targets().unwrap().unwrap();
        assert_eq!(targets.signed.signed.targets.len(), 1);
        let served = std::fs::canonicalize(&out).unwrap();
        assert_eq!(repo.check_url(&format!("file://{}", served.display())), Ok(1));

        // The server's list of flaws (ADR 0389), signed by the online key
        // alone, which may list nothing but it.
        let advisories = dkv::flaws::read_osv(
            br#"{"id": "RUSTSEC-2026-0001", "summary": "A flaw", "affected": [{"package":
                {"ecosystem": "crates.io", "name": "regex"}, "versions": ["1.10.2"]}]}"#,
        )
        .unwrap();
        assert_eq!(repo.sign_flaws(&store, &online_only, &advisories, 7), Ok((1, 1)));
        assert_eq!(
            repo.refresh(&store, &out, &online_only, (7, 1)),
            Ok(Refreshed::All { snapshot: 6, timestamp: 7 })
        );
        assert_eq!(repo.refresh(&store, &out, &online_only, (7, 1)), Ok(Refreshed::Timestamp(8)));
        let list = std::fs::read(store.join(FLAWS_TARGET)).unwrap();
        let list = dkv::flaws::List::read(&list).unwrap();
        assert!(list.checked("org.dukovo.ripgrep", "14.1.1"));
        let flaws = list.flaws;
        assert_eq!(flaws.len(), 1);
        assert_eq!(
            (flaws[0].bundle.as_str(), flaws[0].id.as_str(), flaws[0].component.as_str()),
            ("org.dukovo.ripgrep", "RUSTSEC-2026-0001", "regex 1.10.2")
        );
        assert!(out.join("metadata/1.flaws.json").is_file());
        assert_eq!(repo.check_url(&format!("file://{}", served.display())), Ok(1));
        assert_eq!(repo.sign_flaws(&store, &online_only, &[], 7), Ok((2, 0)));
        // A list that has expired is left out, and the catalog stays good.
        assert_eq!(repo.sign_flaws(&store, &online_only, &advisories, -1), Ok((3, 1)));
        assert_eq!(
            repo.refresh(&store, &out, &online_only, (7, 1)),
            Ok(Refreshed::All { snapshot: 7, timestamp: 9 })
        );
        assert_eq!(repo.check_url(&format!("file://{}", served.display())), Ok(1));
        assert_eq!(repo.refresh(&store, &out, &online_only, (7, 1)), Ok(Refreshed::Timestamp(10)));

        // Marked verified, the target keeps its bytes and gains the owner.
        let marked = repo.changes(&[], &[], &verified).unwrap();
        let version = repo.sign_targets(&marked, &[], &store, &both, 182).unwrap();
        let targets = repo.targets().unwrap().unwrap();
        assert_eq!(targets.signed.version, version);
        let ripgrep = &targets.signed.signed.targets["bundles/org.dukovo.ripgrep-14.1.1.dkv"];
        let custom = ripgrep.custom.as_ref().unwrap();
        assert_eq!(custom.get("owner").and_then(|o| o.as_str()), Some("verified"));
        assert_eq!(custom.get("version").and_then(|v| v.as_str()), Some("14.1.1"));
        assert!(repo.changes(&[], &[], &verified).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A catalog in `dir` whose targets list `bundles`, signed with a fake
    /// YubiKey and the online key's file: its repository, its store, the
    /// signers for everything and the signers for the online key alone.
    fn catalog_of(dir: &Path, bundles: &[PathBuf]) -> (Repo, PathBuf, Signers, Signers) {
        let repo = Repo::new(&dir.join("catalog"));
        let helper = fake_yubikey(dir);
        keys::from_yubikey(&helper, None).unwrap().write(&repo.keys_dir()).unwrap();
        let online_file = dir.join("online.key");
        let roles = vec!["snapshot".to_string(), "timestamp".to_string()];
        keys::new_file_key("online", roles, &online_file).unwrap().write(&repo.keys_dir()).unwrap();
        let both = Signers::new(Some(&helper), std::slice::from_ref(&online_file)).unwrap();
        repo.sign_root(&both, 365).unwrap();
        let changes = repo.changes(bundles, &[], &[]).unwrap();
        let store = dir.join("store");
        repo.sign_targets(&changes, bundles, &store, &both, 182).unwrap();
        let online = Signers::new(None, std::slice::from_ref(&online_file)).unwrap();
        (repo, store, both, online)
    }

    /// The rollout at `path` that the client reads from what `out` serves,
    /// starting from the root `first`; none if it finds none. Each read has
    /// a client folder of its own, as the tests that read run at once in
    /// one process.
    fn read_rollout(out: &Path, first: &[u8], path: &str) -> Option<rollout::Rollout> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let work = scratch(&format!("client-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        tuf::init(&work, first).unwrap();
        let absolute = std::fs::canonicalize(out).unwrap();
        let base = tuf::Url::parse(&format!("file://{}/", absolute.display())).unwrap();
        let metadata = base.resolve("metadata/").unwrap();
        let mut updater =
            tuf::Updater::new(&work, &metadata, Some(&base.resolve("targets/").unwrap())).unwrap();
        updater.refresh().unwrap();
        let found = updater.target(path).unwrap().map(|target| {
            let file = work.join("target");
            updater.download_target(path, &target, &file).unwrap();
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap()
        });
        std::fs::remove_dir_all(&work).unwrap();
        found
    }

    #[test]
    fn the_targets_delegate_the_rollouts_to_the_online_key_alone() {
        let dir = scratch("delegation");
        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, ..) = catalog_of(&dir, &[ripgrep]);
        let root = repo.root().unwrap().unwrap();
        let targets = repo.targets().unwrap().unwrap();
        let delegations = targets.signed.signed.delegations.as_ref().unwrap();
        let role = |name: &str| delegations.roles.iter().flatten().find(|r| r.name == name);
        let rollouts = role(rollout::ROLE).unwrap();
        assert_eq!(rollouts.keyids, root.signed.signed.roles["timestamp"].keyids);
        assert_eq!((rollouts.threshold, rollouts.terminating), (1, true));
        for path in [
            "rollout/org.dukovo.ripgrep/14.1.1.json",
            "rollout/org.nodejs.node@22/22.1.0-rc.1+b5.json",
        ] {
            assert!(rollouts.delegates(path), "{path}");
        }
        // The system's own shape, one part too many or few, another kind
        // of file, and the other roles' paths are none of its.
        for path in [
            "rollout/14.1.1.json",
            "rollout/org/dukovo/14.1.1.json",
            "rollout/org.dukovo.ripgrep/14.1.1.txt",
            "rollout",
            "flaws/flaws.json",
            "bundles/org.dukovo.ripgrep-14.1.1.dkv",
        ] {
            assert!(!rollouts.delegates(path), "{path}");
        }
        let flaws = role(FLAWS_ROLE).unwrap();
        assert!(flaws.delegates(FLAWS_TARGET));
        assert!(!flaws.delegates("rollout/org.dukovo.ripgrep/14.1.1.json"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rollouts_open_wave_by_wave_and_are_read_back_by_the_client() {
        let dir = scratch("rollouts");
        let older = bundle(&dir, "org.dukovo.ripgrep", "14.1.0", b"rg, older");
        let newer = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let bat_file = bundle(&dir, "org.dukovo.bat", "0.25.0", b"bat");
        let (repo, store, both, online) = catalog_of(&dir, std::slice::from_ref(&older));
        let (out, first) = (dir.join("site"), std::fs::read(repo.metadata("1.root.json")).unwrap());
        let (now, day) = (tuf::time::now(), 86_400);
        let mut plans = Plans::default();
        let rolled = |when: i64, plans: &mut Plans| {
            assert!(repo.sign_rollouts(&store, &online, Some(plans), when).unwrap().is_some());
            repo.refresh(&store, &out, &online, (7, 1)).unwrap()
        };
        let (ripgrep, bat) = ("org.dukovo.ripgrep/14.1.1", "org.dukovo.bat/0.25.0");
        let read = |release: &str| read_rollout(&out, &first, &rollout::target_path(release));
        let at =
            |release: &str, wave| rollout::Rollout { release: release.into(), wave, held: None };

        // A version listed before the first rollouts reached every
        // computer the day it was listed: every wave is open.
        assert_eq!(rolled(now, &mut plans), Refreshed::All { snapshot: 1, timestamp: 1 });
        let listed_before = "org.dukovo.ripgrep/14.1.0";
        assert_eq!(read(listed_before), Some(at(listed_before, rollout::LAST)));
        let added = [newer, bat_file];
        let changes = repo.changes(&added, &[], &[]).unwrap();
        repo.sign_targets(&changes, &added, &store, &both, 182).unwrap();

        // The newest version of each bundle listed after them opens to the
        // canaries on its first day, and an older one has no rollout.
        assert_eq!(rolled(now, &mut plans), Refreshed::All { snapshot: 2, timestamp: 2 });
        assert_eq!(read(ripgrep), Some(at(ripgrep, 0)));
        assert_eq!(read(bat), Some(at(bat, 0)));
        assert_eq!(plans.releases[ripgrep].released, now);
        assert_eq!(read("org.dukovo.ripgrep/14.1.0"), None);
        assert!(!plans.releases.contains_key("org.dukovo.ripgrep/14.1.0"));
        // Nothing outside the rollouts' paths is found through the
        // delegation, nor a version the catalog never listed.
        for path in ["rollout/14.1.1.json", "rollout/a/b/c.json", "rollout/a/b.txt"] {
            assert_eq!(read_rollout(&out, &first, path), None, "{path}");
        }
        assert_eq!(read("org.dukovo.ripgrep/9.9.9"), None);

        // With nothing new to sign, a refresh signs a timestamp alone.
        assert!(repo.sign_rollouts(&store, &online, Some(&mut plans), now).unwrap().is_some());
        assert_eq!(repo.refresh(&store, &out, &online, (7, 1)), Ok(Refreshed::Timestamp(3)));

        // Wave 1 on the second day; a pause holds it there while the
        // schedule goes on for the other bundle.
        assert_eq!(rolled(now + day, &mut plans), Refreshed::All { snapshot: 3, timestamp: 4 });
        assert_eq!(read(ripgrep), Some(at(ripgrep, 1)));
        plans.releases.get_mut(ripgrep).unwrap().paused = Some("disk errors".into());
        rolled(now + 3 * day, &mut plans);
        let held = read(ripgrep).unwrap();
        assert_eq!((held.wave, held.held.as_deref()), (1, Some("paused: disk errors")));
        assert_eq!(read(bat), Some(at(bat, 2)));
        plans.releases.get_mut(ripgrep).unwrap().paused = None;
        rolled(now + 5 * day, &mut plans);
        assert_eq!(read(ripgrep), Some(at(ripgrep, 3)));

        // The role is signed anew once it has less than twelve hours left,
        // and not before.
        let version = repo.sign_rollouts(&store, &online, Some(&mut plans), now + 5 * day).unwrap();
        let later = now + 6 * day - 11 * 3600;
        assert_ne!(repo.sign_rollouts(&store, &online, Some(&mut plans), later).unwrap(), version);

        // A newer version takes the place of the newest: the plan of the
        // one before goes, and the new one opens to the canaries.
        let third = bundle(&dir, "org.dukovo.ripgrep", "14.2.0", b"rg, newer still");
        let changes = repo.changes(std::slice::from_ref(&third), &[], &[]).unwrap();
        repo.sign_targets(&changes, &[third], &store, &both, 182).unwrap();
        let (newest, released) = ("org.dukovo.ripgrep/14.2.0", now + 5 * day);
        rolled(released, &mut plans);
        assert_eq!(read(newest), Some(at(newest, 0)));
        assert_eq!(read(ripgrep), None);
        assert_eq!(plans.releases.keys().collect::<Vec<_>>(), [bat, newest]);
        assert_eq!(plans.releases[newest].released, released);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn served_says_what_a_catalog_serves_without_fetching_its_bundles() {
        let dir = scratch("served");
        let older = bundle(&dir, "org.dukovo.ripgrep", "14.1.0", b"rg, older");
        let newer = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, store, both, online) = catalog_of(&dir, std::slice::from_ref(&older));
        let out = dir.join("site");
        let mut plans = Plans::default();
        let now = tuf::time::now();
        repo.sign_rollouts(&store, &online, Some(&mut plans), now).unwrap();
        let added = [newer];
        let changes = repo.changes(&added, &[], &[]).unwrap();
        repo.sign_targets(&changes, &added, &store, &both, 182).unwrap();
        plans.releases.insert(
            "org.dukovo.ripgrep/14.1.1".into(),
            Plan { released: now, open: 0, paused: Some("disk errors".into()) },
        );
        repo.sign_rollouts(&store, &online, Some(&mut plans), now).unwrap();
        let advisories = dkv::flaws::read_osv(
            br#"{"id": "RUSTSEC-2026-0001", "summary": "A flaw", "affected": [{"package":
                {"ecosystem": "crates.io", "name": "regex"}, "versions": ["1.10.2"]}]}"#,
        )
        .unwrap();
        repo.sign_flaws(&store, &online, &advisories, 7).unwrap();
        repo.refresh(&store, &out, &online, (7, 1)).unwrap();
        // Remove every bundle from what is served: reading it must not
        // fetch them.
        for entry in std::fs::read_dir(out.join("targets/bundles")).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }

        let url = format!("file://{}", std::fs::canonicalize(&out).unwrap().display());
        let served = repo.served(&url).unwrap();
        assert!(served["checked"].as_i64().unwrap() >= now);
        let roles: Vec<(&str, u64)> = served["roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["role"].as_str().unwrap(), r["version"].as_u64().unwrap()))
            .collect();
        let names: Vec<&str> = roles.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["root", "timestamp", "snapshot", "targets", "flaws", "rollout"]);
        assert_eq!(roles[0], ("root", 1));
        assert_eq!(roles[3], ("targets", 2));
        let expires = |role: &str| {
            let found = served["roles"].as_array().unwrap().iter().find(|r| r["role"] == role);
            found.unwrap()["expires"].as_i64().unwrap()
        };
        assert!((expires("targets") - now - 182 * 86_400).abs() < 100);
        assert!((expires("timestamp") - now - 86_400).abs() < 100);
        let bundles = served["bundles"].as_array().unwrap();
        let listed: Vec<(&str, &str)> = bundles
            .iter()
            .map(|b| (b["id"].as_str().unwrap(), b["version"].as_str().unwrap()))
            .collect();
        assert_eq!(listed, [("org.dukovo.ripgrep", "14.1.0"), ("org.dukovo.ripgrep", "14.1.1")]);
        assert_eq!(bundles[1]["name"], "Test");
        assert_eq!(bundles[1]["path"], "bundles/org.dukovo.ripgrep-14.1.1.dkv");
        assert_eq!(served["flaws_listed"], true);
        let flaws = bundles[1]["flaws"].as_array().unwrap();
        assert_eq!(flaws.len(), 1);
        assert_eq!(
            (flaws[0]["id"].as_str(), flaws[0]["component"].as_str(), flaws[0]["fixed"].is_null()),
            (Some("RUSTSEC-2026-0001"), Some("regex 1.10.2"), true)
        );
        assert_eq!(flaws[0]["severity"], dkv::flaws::Severity::Unknown.word());
        let rollouts = served["rollouts"].as_array().unwrap();
        assert_eq!(rollouts.len(), 1);
        assert_eq!(
            (rollouts[0]["bundle"].as_str(), rollouts[0]["version"].as_str()),
            (Some("org.dukovo.ripgrep"), Some("14.1.1"))
        );
        assert_eq!(rollouts[0]["wave"], 0);
        assert_eq!(rollouts[0]["held"], "paused: disk errors");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lines_id_and_a_builds_version_make_a_rollout_path_the_client_reads() {
        let dir = scratch("rollout-paths");
        let node = bundle_in(&dir, "org.nodejs.node", "22", "22.1.0+b5", b"node");
        let (repo, store, _, online) = catalog_of(&dir, &[node]);
        let (out, first) = (dir.join("site"), std::fs::read(repo.metadata("1.root.json")).unwrap());
        let mut plans = Plans::default();
        repo.sign_rollouts(&store, &online, Some(&mut plans), tuf::time::now()).unwrap().unwrap();
        repo.publish(&store, &out, &online, (7, 1)).unwrap();
        let release = "org.nodejs.node@22/22.1.0+b5";
        let found = read_rollout(&out, &first, &rollout::target_path(release));
        let wave = rollout::LAST;
        assert_eq!(found, Some(rollout::Rollout { release: release.into(), wave, held: None }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_a_plan_a_new_version_has_every_wave_open() {
        let dir = scratch("unplanned");
        let older = bundle(&dir, "org.dukovo.ripgrep", "14.1.0", b"rg, older");
        let (repo, store, both, online) = catalog_of(&dir, std::slice::from_ref(&older));
        let (out, first) = (dir.join("site"), std::fs::read(repo.metadata("1.root.json")).unwrap());
        let now = tuf::time::now();
        let mut plans = Plans::default();
        repo.sign_rollouts(&store, &online, Some(&mut plans), now).unwrap().unwrap();
        // A version listed after the first rollouts, signed by a server
        // that was given no plan.
        let newer = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let changes = repo.changes(std::slice::from_ref(&newer), &[], &[]).unwrap();
        repo.sign_targets(&changes, &[newer], &store, &both, 182).unwrap();
        repo.sign_rollouts(&store, &online, None, now).unwrap().unwrap();
        repo.publish(&store, &out, &online, (7, 1)).unwrap();
        let release = "org.dukovo.ripgrep/14.1.1";
        let found = read_rollout(&out, &first, &rollout::target_path(release));
        let wave = rollout::LAST;
        assert_eq!(found, Some(rollout::Rollout { release: release.into(), wave, held: None }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_catalog_whose_targets_delegate_no_rollouts_serves_none() {
        let dir = scratch("no-rollouts");
        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, store, _, online) = catalog_of(&dir, &[ripgrep]);
        let mut plans = Plans::default();
        let now = tuf::time::now();
        assert!(repo.sign_rollouts(&store, &online, Some(&mut plans), now).unwrap().is_some());
        // Targets as an older catalog signed them, with the flaws alone.
        let path = repo.metadata("targets.json");
        let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let roles = file["signed"]["delegations"]["roles"].as_array_mut().unwrap();
        roles.retain(|role| role["name"] == FLAWS_ROLE);
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let mut plans = Plans::default();
        assert_eq!(repo.sign_rollouts(&store, &online, Some(&mut plans), now), Ok(None));
        assert!(plans.releases.is_empty());
        let targets = repo.targets().unwrap().unwrap();
        let kept = online_role(&store, ROLLOUT_METADATA, rollout::ROLE, &targets.signed.signed);
        assert!(kept.is_none(), "rollouts are not served for targets that do not delegate them");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The `custom.withdrawn` of the target at `path`, as signed.
    fn withdrawn_of(repo: &Repo, path: &str) -> Option<Value> {
        let targets = repo.targets().unwrap().unwrap();
        targets.signed.signed.targets[path].custom.as_ref()?.get("withdrawn").cloned()
    }

    #[test]
    fn a_withdrawal_is_signed_into_the_target_that_stays_listed() {
        let dir = scratch("withdraw");
        let old = bundle_in(&dir, "org.nodejs.node", "22", "22.0.0", b"old");
        let new = bundle_in(&dir, "org.nodejs.node", "22", "22.1.0", b"new");
        let (repo, store, both, _) = catalog_of(&dir, &[old, new]);
        let path = "bundles/org.nodejs.node-22.1.0.dkv";
        let changes = repo
            .withdraw(
                "org.nodejs.node@22",
                "22.1.0",
                "broken",
                "it crashes at start",
                1_700_000_000,
            )
            .unwrap();
        assert!(!changes.is_empty());
        assert_eq!(changes.lines(), [format!("! {path} withdrawn as broken: it crashes at start")]);
        assert_eq!(withdrawn_of(&repo, path), None);
        repo.sign_targets(&changes, &[], &store, &both, 182).unwrap();
        let expected =
            json!({"kind": "broken", "reason": "it crashes at start", "since": 1_700_000_000});
        assert_eq!(withdrawn_of(&repo, path), Some(expected));
        // The other version and the target's own fields are as they were.
        assert_eq!(withdrawn_of(&repo, "bundles/org.nodejs.node-22.0.0.dkv"), None);
        let targets = repo.targets().unwrap().unwrap();
        let custom = targets.signed.signed.targets[path].custom.as_ref().unwrap();
        assert_eq!(custom["id"], "org.nodejs.node@22");
        assert_eq!(custom["version"], "22.1.0");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_withdrawal_is_refused_when_it_names_nothing_or_says_nothing() {
        let dir = scratch("withdraw-refused");
        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, ..) = catalog_of(&dir, &[ripgrep]);
        let id = "org.dukovo.ripgrep";
        let refused = |id: &str, version: &str, kind: &str, reason: &str| {
            repo.withdraw(id, version, kind, reason, 5).unwrap_err()
        };
        assert!(refused(id, "14.1.2", "broken", "it crashes").contains("no target lists"));
        assert!(refused("org.dukovo.fd", "14.1.1", "broken", "it crashes").contains("no target"));
        // The full ID is what a target says; the line-less ID is not another's.
        assert!(refused("org.dukovo.ripgrep@14", "14.1.1", "broken", "x").contains("no target"));
        assert!(refused(id, "14.1.1", "evil", "it crashes").contains("neither broken nor"));
        assert!(refused(id, "14.1.1", "broken", "").contains("empty"));
        assert!(refused(id, "14.1.1", "broken", "  \t ").contains("empty"));
        assert!(refused(id, "14.1.1", "broken", "two\nlines").contains("control"));
        let long = "x".repeat(REASON_MAX + 1);
        assert!(refused(id, "14.1.1", "broken", &long).contains("201 bytes"));
        let longest = "x".repeat(REASON_MAX);
        assert!(repo.withdraw(id, "14.1.1", "broken", &longest, 5).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_withdrawal_can_get_worse_but_never_better() {
        let dir = scratch("withdraw-worse");
        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, store, both, _) = catalog_of(&dir, &[ripgrep]);
        let (id, path) = ("org.dukovo.ripgrep", "bundles/org.dukovo.ripgrep-14.1.1.dkv");
        let sign = |kind: &str, reason: &str| {
            let changes = repo.withdraw(id, "14.1.1", kind, reason, 9)?;
            repo.sign_targets(&changes, &[], &store, &both, 182).map(|_| ())
        };
        sign("broken", "it crashes").unwrap();
        assert!(sign("broken", "again").unwrap_err().contains("already"));
        sign("malicious", "it steals keys").unwrap();
        let expected = json!({"kind": "malicious", "reason": "it steals keys", "since": 9});
        assert_eq!(withdrawn_of(&repo, path), Some(expected.clone()));
        assert!(sign("broken", "it crashes").unwrap_err().contains("malicious"));
        assert!(sign("malicious", "again").unwrap_err().contains("already"));
        assert_eq!(withdrawn_of(&repo, path), Some(expected));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_withdrawal_that_does_not_read_can_still_be_made_malicious() {
        let dir = scratch("withdraw-unreadable");
        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, ..) = catalog_of(&dir, &[ripgrep]);
        let path = repo.metadata("targets.json");
        let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let target = &mut file["signed"]["targets"]["bundles/org.dukovo.ripgrep-14.1.1.dkv"];
        target["custom"]["withdrawn"] = json!("gone");
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let id = "org.dukovo.ripgrep";
        assert!(repo.withdraw(id, "14.1.1", "broken", "it crashes", 5).is_err());
        assert!(repo.withdraw(id, "14.1.1", "malicious", "it steals keys", 5).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_listed_version_packed_again_as_other_bytes_is_kept_and_named() {
        let dir = scratch("rebuilt");
        let signed = bundle(&dir, "org.dukovo.ripgrep", "15.2.0", b"rg");
        let (repo, ..) = catalog_of(&dir, std::slice::from_ref(&signed));
        let packed = bundle(&dir, "org.dukovo.ripgrep", "15.2.0", b"rg, linked again");
        let changes = repo.changes(std::slice::from_ref(&packed), &[], &[]).unwrap();
        assert!(changes.added.is_empty());
        assert!(changes.is_empty(), "a version kept as signed signs nothing");
        let path = "bundles/org.dukovo.ripgrep-15.2.0.dkv";
        let data = |file: &Path| repo::file_info(&std::fs::read(file).unwrap());
        assert_eq!(
            changes.rebuilt,
            [Rebuilt {
                path: path.into(),
                file: packed.clone(),
                id: "org.dukovo.ripgrep".into(),
                version: "15.2.0".into(),
                signed_sha256: data(&signed)["hashes"]["sha256"].as_str().unwrap().into(),
                packed_sha256: data(&packed)["hashes"]["sha256"].as_str().unwrap().into(),
            }]
        );
        assert_eq!(changes.lines().len(), 1);
        assert!(changes.lines()[0].starts_with(&format!("= {path}: packed again")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_with_a_repacked_version_signs_the_rest() {
        let dir = scratch("rebuilt-batch");
        let signed = bundle(&dir, "org.dukovo.ripgrep", "15.2.0", b"rg");
        let (repo, store, both, _) = catalog_of(&dir, std::slice::from_ref(&signed));
        let path = "bundles/org.dukovo.ripgrep-15.2.0.dkv";
        let before = target_json(&repo.targets().unwrap().unwrap().signed.signed.targets[path]);
        let stored = std::fs::read(store.join(path)).unwrap();
        let packed = bundle(&dir, "org.dukovo.ripgrep", "15.2.0", b"rg, linked again");
        let bat = bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat");
        let batch = [packed, bat];
        let changes = repo.changes(&batch, &[], &[]).unwrap();
        assert_eq!(changes.added.keys().collect::<Vec<_>>(), ["bundles/org.dukovo.bat-0.26.1.dkv"]);
        assert_eq!(changes.rebuilt.len(), 1);
        assert_eq!(repo.sign_targets(&changes, &batch, &store, &both, 182), Ok(2));
        let targets = repo.targets().unwrap().unwrap().signed.signed.targets;
        assert_eq!(targets.len(), 2);
        assert_eq!(target_json(&targets[path]), before, "the signed version's target is as it was");
        assert_eq!(std::fs::read(store.join(path)).unwrap(), stored, "its bytes too");
        assert!(store.join("bundles/org.dukovo.bat-0.26.1.dkv").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bytes_packed_again_after_the_list_are_not_stored() {
        let dir = scratch("moved-bytes");
        let first = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, store, both, _) = catalog_of(&dir, &[first]);
        let bat = bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat");
        let changes = repo.changes(std::slice::from_ref(&bat), &[], &[]).unwrap();
        let other = bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat, packed again");
        std::fs::rename(&other, &bat).unwrap();
        let err = repo.sign_targets(&changes, &[bat], &store, &both, 182).unwrap_err();
        assert!(err.contains("list the changes again"), "{err}");
        assert_eq!(repo.targets().unwrap().unwrap().signed.version, 1);
        assert!(!store.join("bundles/org.dukovo.bat-0.26.1.dkv").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plans_digest_follows_what_is_signed_and_not_the_clock() {
        use crate::plan::{Action, Plan};
        let dir = scratch("digest");
        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let old = bundle(&dir, "org.dukovo.old", "1.0", b"old");
        let (repo, ..) = catalog_of(&dir, &[ripgrep, old]);
        let bat = bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat");
        let digest = |action, after, changes| Plan { action, after, changes }.digest().unwrap();
        let listed = |files: &[PathBuf], remove: &[String], verified: &[String]| {
            repo.changes(files, remove, verified).unwrap()
        };
        let base = digest(Action::Sign, 1, listed(std::slice::from_ref(&bat), &[], &[]));
        assert_eq!(base.len(), 64);
        assert!(base.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(base, digest(Action::Sign, 1, listed(std::slice::from_ref(&bat), &[], &[])));
        let mut differ = vec![
            ("the action", digest(Action::Renew, 1, listed(std::slice::from_ref(&bat), &[], &[]))),
            ("after", digest(Action::Sign, 2, listed(std::slice::from_ref(&bat), &[], &[]))),
        ];
        // One byte other, packed in a folder of its own.
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let other = bundle(&elsewhere, "org.dukovo.bat", "0.26.1", b"bau");
        differ.push(("a byte", digest(Action::Sign, 1, listed(&[other], &[], &[]))));
        let mark = vec!["org.dukovo.bat".to_string()];
        differ.push((
            "a mark",
            digest(Action::Sign, 1, listed(std::slice::from_ref(&bat), &[], &mark)),
        ));
        let removal = vec!["bundles/org.dukovo.old-1.0.dkv".to_string()];
        differ.push((
            "a removal",
            digest(Action::Sign, 1, listed(std::slice::from_ref(&bat), &removal, &[])),
        ));
        for (what, other) in &differ {
            assert_ne!(*other, base, "{what} changes the digest");
        }
        // A withdrawal: its time and the expiry are not in it, its reason is.
        let id = "org.dukovo.ripgrep";
        let withdrawal =
            |reason, since| repo.withdraw(id, "14.1.1", "broken", reason, since).unwrap();
        let at_one = digest(Action::Withdraw, 1, withdrawal("it crashes", 1));
        assert_eq!(at_one, digest(Action::Withdraw, 1, withdrawal("it crashes", 2)));
        assert_ne!(at_one, digest(Action::Withdraw, 1, withdrawal("it crashes at start", 1)));
        let plan = Plan { action: Action::Sign, after: 1, changes: listed(&[bat], &[], &[]) };
        let (week, year) = (repo.plan_json(&plan, 7).unwrap(), repo.plan_json(&plan, 365).unwrap());
        assert_ne!(week["targets"]["expires"], year["targets"]["expires"]);
        assert_eq!(week["digest"], year["digest"]);
        assert_eq!(week["digest"], base);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plan_file_says_each_change_and_who_can_sign_it() {
        use crate::plan::Action;
        let dir = scratch("plan-file");
        let rg = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let old = bundle(&dir, "org.dukovo.old", "1.0", b"old");
        let (repo, ..) = catalog_of(&dir, &[rg, old]);
        let rg_new = bundle(&dir, "org.dukovo.ripgrep", "15.2.0", b"rg 15");
        let bat = bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat");
        let again = bundle(&dir, "org.dukovo.old", "1.0", b"old, packed again");
        let remove = ["bundles/org.dukovo.old-1.0.dkv".to_string()];
        let verified = ["org.dukovo.ripgrep".to_string(), "org.dukovo.bat".to_string()];
        let changes = repo.changes(&[rg_new, bat, again], &remove, &verified).unwrap();
        let plan = repo.plan(Action::Sign, changes).unwrap();
        let file = dir.join("out/catalog/plan.json");
        let written = repo.write_plan(&plan, 182, &file).unwrap();
        let read: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(read, written);
        assert_eq!(read["format"], 1);
        assert_eq!(read["action"], "sign");
        assert_eq!(read["targets"]["after"], 1);
        assert_eq!(read["targets"]["version"], 2);
        assert_eq!(read["signers"], json!([{"name": "yubikey-12345678", "serial": 12_345_678}]));
        let added = read["added"].as_array().unwrap();
        assert_eq!(added.len(), 2);
        assert_eq!(added[0]["path"], "bundles/org.dukovo.bat-0.26.1.dkv");
        assert_eq!(added[0]["change"], "new");
        assert_eq!(added[0]["verified"], true);
        assert_eq!(added[1]["id"], "org.dukovo.ripgrep");
        assert_eq!(added[1]["version"], "15.2.0");
        assert_eq!(added[1]["name"], "Test");
        assert_eq!(added[1]["change"], "update");
        assert_eq!(added[1]["sha256"].as_str().unwrap().len(), 64);
        let removed = json!([{"path": remove[0], "id": "org.dukovo.old", "version": "1.0"}]);
        assert_eq!(read["removed"], removed);
        let marked = json!([{
            "path": "bundles/org.dukovo.ripgrep-14.1.1.dkv", "id": "org.dukovo.ripgrep",
            "version": "14.1.1"
        }]);
        assert_eq!(read["verified"], marked);
        assert_eq!(
            read["rebuilt"],
            json!([{"path": remove[0], "id": "org.dukovo.old", "version": "1.0"}])
        );
        assert_eq!(read["digest"], plan.digest().unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn between_names_what_signed_targets_changed() {
        let dir = scratch("between");
        let rg = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let old = bundle(&dir, "org.dukovo.old", "1.0", b"old");
        let (repo, store, both, _) = catalog_of(&dir, &[rg, old]);
        let first = repo.targets().unwrap().unwrap().signed.signed;
        let bat = bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat");
        let remove = ["bundles/org.dukovo.old-1.0.dkv".to_string()];
        let verified = ["org.dukovo.ripgrep".to_string()];
        let changes = repo.changes(std::slice::from_ref(&bat), &remove, &verified).unwrap();
        repo.sign_targets(&changes, &[bat], &store, &both, 182).unwrap();
        let withdrawal =
            repo.withdraw("org.dukovo.ripgrep", "14.1.1", "broken", "it crashes", 7).unwrap();
        repo.sign_targets(&withdrawal, &[], &store, &both, 182).unwrap();
        let last = repo.targets().unwrap().unwrap().signed.signed;
        let since = Changes::between(Some(&first), &last);
        assert_eq!(since.added.keys().collect::<Vec<_>>(), ["bundles/org.dukovo.bat-0.26.1.dkv"]);
        assert_eq!(since.removed, remove);
        assert_eq!(since.verified, ["bundles/org.dukovo.ripgrep-14.1.1.dkv"]);
        assert_eq!(since.withdrawn.len(), 1);
        let lines = since.lines();
        assert!(
            lines[0].starts_with("+ bundles/org.dukovo.bat-0.26.1.dkv (org.dukovo.bat 0.26.1, ")
        );
        assert_eq!(lines[1], "- bundles/org.dukovo.old-1.0.dkv");
        assert_eq!(lines[2], "~ bundles/org.dukovo.ripgrep-14.1.1.dkv: publisher verified");
        assert_eq!(
            lines[3],
            "! bundles/org.dukovo.ripgrep-14.1.1.dkv withdrawn as broken: it crashes"
        );
        assert_eq!(Changes::between(Some(&last), &last), Changes::default());
        assert_eq!(Changes::between(None, &first).added.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_takes_a_signed_catalog_and_refuses_a_changed_byte() {
        let dir = scratch("verify");
        let rg = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let line = bundle_in(&dir, "org.nodejs.node", "22", "22.1.0", b"node");
        let (repo, ..) = catalog_of(&dir, &[rg, line]);
        let now = tuf::time::now();
        let verified = repo.verify(now).unwrap();
        assert_eq!((verified.root, verified.targets, verified.bundles), (1, 1, 2));
        assert_eq!(verified.signed_by.len(), 1);
        let line = verified.line();
        assert!(line.starts_with("catalog: root 1, targets version 1 signed by "), "{line}");
        assert!(line.contains("…, 2 bundles, good until 20"), "{line}");
        // Half a year and more on, the targets have expired; past a year,
        // the root has too.
        let day = 86_400;
        assert!(repo.verify(now + 190 * day).unwrap_err().contains("targets version 1 expired"));
        assert!(repo.verify(now + 400 * day).unwrap_err().contains("root version 1 expired"));
        // A byte of the signed targets changed.
        let path = repo.metadata("targets.json");
        let good = std::fs::read(&path).unwrap();
        let mut file: Value = serde_json::from_slice(&good).unwrap();
        file["signed"]["targets"]["bundles/org.dukovo.ripgrep-14.1.1.dkv"]["length"] = 9.into();
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(repo.verify(now).unwrap_err().contains("targets.json is not signed"));
        std::fs::write(&path, &good).unwrap();
        // root.json that is not the newest numbered root.
        let root = repo.metadata("root.json");
        let root_bytes = std::fs::read(&root).unwrap();
        std::fs::write(&root, b"{}").unwrap();
        assert!(repo.verify(now).is_err());
        std::fs::write(&root, &root_bytes).unwrap();
        assert!(repo.verify(now).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_refuses_a_root_the_one_before_did_not_sign() {
        let dir = scratch("verify-chain");
        let rg = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, ..) = catalog_of(&dir, &[rg]);
        // Root 2, signed by a new key alone: a key root 1 never named.
        let secret = dir.join("usurper.key");
        let roles: Vec<String> = ROLES.iter().map(|r| (*r).to_string()).collect();
        let usurper = keys::new_file_key("usurper", roles, &secret).unwrap();
        let roles: Vec<(&str, Vec<&tuf::keys::Key>, u64)> =
            ROLES.iter().map(|role| (*role, vec![&usurper.key], 1)).collect();
        let signed = repo::root(2, &repo::expires_in(365), &roles, true);
        let signers = Signers::new(None, std::slice::from_ref(&secret)).unwrap();
        let signer = signers.for_key(&usurper).unwrap();
        let bytes = repo::sign(&signed, &[signer.as_ref()]).unwrap();
        write(&repo.metadata("2.root.json"), &bytes).unwrap();
        write(&repo.metadata("root.json"), &bytes).unwrap();
        let err = repo.verify(tuf::time::now()).unwrap_err();
        assert!(err.contains("2.root.json is not signed by the root keys of version 1"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_refuses_a_target_under_another_path() {
        let dir = scratch("verify-path");
        let rg = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let (repo, store, both, _) = catalog_of(&dir, &[rg]);
        let mut changes = Changes::default();
        let bat = std::fs::read(bundle(&dir, "org.dukovo.bat", "0.26.1", b"bat")).unwrap();
        let (_, info) = bundle_target(&bat, false).unwrap();
        changes.added.insert("bundles/org.dukovo.ripgrep-99.dkv".into(), info);
        repo.sign_targets(&changes, &[], &store, &both, 182).unwrap();
        let err = repo.verify(tuf::time::now()).unwrap_err();
        assert!(
            err.contains(
                "lists org.dukovo.bat 0.26.1, whose path is bundles/org.dukovo.bat-0.26.1.dkv"
            ),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
