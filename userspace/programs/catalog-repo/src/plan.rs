// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The plan of a signing (ADR 0594): what a batch would change in the
//! catalog's targets, written down to be read before anything is signed,
//! and its digest, which the signing is then held to. What is signed is
//! listed again when it is signed; a digest that differs from the one read
//! means the list moved, and nothing is signed.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{Changes, Repo, keys, write};

/// The plan file's format, and that of what its digest is over.
pub const FORMAT: u64 = 1;

/// What a plan signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Bundles added or taken out, and publishers marked verified.
    Sign,
    /// The targets again with no bundle read: their expiry renewed, with
    /// any publishers marked verified.
    Renew,
    /// A version withdrawn (ADR 0593).
    Withdraw,
}

impl Action {
    /// Its word in a plan.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Action::Sign => "sign",
            Action::Renew => "renew",
            Action::Withdraw => "withdraw",
        }
    }
}

/// A batch to sign, as listed before it is.
#[derive(Debug)]
pub struct Plan {
    /// What it signs.
    pub action: Action,
    /// The version of the targets it is signed after: 0 for none.
    pub after: u64,
    /// What it changes.
    pub changes: Changes,
}

impl Plan {
    /// The SHA-256, in lower-case hex, of what signing the plan writes into
    /// the targets: the action, the targets version it follows, each path
    /// added as it is listed (length, hashes, ID, version, name and
    /// publisher mark), the paths taken out and newly marked verified, and
    /// each withdrawal's kind and reason. Taken over canonical JSON, whose
    /// object keys are sorted, so that it does not hang on the order a map
    /// keeps.
    ///
    /// It leaves out what the clock gives at the moment of signing, a
    /// withdrawal's `since` and the targets' expiry, so that a plan listed
    /// again a minute later has the same digest; and the bundles packed
    /// again, which sign nothing.
    ///
    /// # Errors
    ///
    /// Only if a listed value is a number that is not an integer, which
    /// no target has.
    pub fn digest(&self) -> Result<String, String> {
        let changes = &self.changes;
        let added: Map<String, Value> =
            changes.added.iter().map(|(path, info)| (path.clone(), info.clone())).collect();
        let withdrawn: Map<String, Value> = changes
            .withdrawn
            .iter()
            .map(|(path, w)| (path.clone(), json!({ "kind": w["kind"], "reason": w["reason"] })))
            .collect();
        let sorted = |paths: &[String]| {
            let mut paths = paths.to_vec();
            paths.sort();
            paths
        };
        let value = json!({
            "format": FORMAT,
            "action": self.action.word(),
            "after": self.after,
            "added": added,
            "removed": sorted(&changes.removed),
            "verified": sorted(&changes.verified),
            "withdrawn": withdrawn,
        });
        let bytes = tuf::json::canonical(&value).map_err(|e| e.to_string())?;
        Ok(crate::sha256_of_data(&bytes))
    }
}

impl Repo {
    /// The plan of signing `changes` as `action` after the targets signed
    /// now.
    ///
    /// # Errors
    ///
    /// If the targets cannot be read.
    pub fn plan(&self, action: Action, changes: Changes) -> Result<Plan, String> {
        let after = self.targets()?.map_or(0, |targets| targets.signed.version);
        Ok(Plan { action, after, changes })
    }

    /// The plan as a person reads it, and as Dukovo Ops shows it: the
    /// targets version it follows and the one it makes, with the expiry
    /// signing now would give over `days`; the keys in `keys/` that the
    /// root names for targets; each change with its bundle's ID and
    /// version, a version's next build with the build it replaces, its
    /// files that differ and its reason (ADR 0595); the bundles packed
    /// again, which stay as signed, with their files and the next build
    /// that would publish them; and the digest that signing is held to.
    ///
    /// # Errors
    ///
    /// If the targets, the root or the keys cannot be read.
    pub fn plan_json(&self, plan: &Plan, days: i64) -> Result<Value, String> {
        let listed = self.targets()?.map_or_default(|t| t.signed.signed.targets);
        let field = |path: &str, key: &str| {
            listed
                .get(path)
                .and_then(|t| t.custom.as_ref()?.get(key).cloned())
                .unwrap_or(Value::Null)
        };
        let named = |path: &String| json!({ "path": path, "id": field(path, "id"), "version": field(path, "version") });
        let ids: BTreeSet<String> = listed
            .keys()
            .filter_map(|path| field(path, "id").as_str().map(str::to_string))
            .collect();
        let changes = &plan.changes;
        let added: Vec<Value> = changes
            .added
            .iter()
            .map(|(path, info)| {
                let custom = &info["custom"];
                let mut entry = json!({
                    "path": path,
                    "id": custom["id"],
                    "name": custom["name"],
                    "version": custom["version"],
                    "length": info["length"],
                    "sha256": info["hashes"]["sha256"],
                    "verified": custom["owner"] == "verified",
                    "change": if custom["id"].as_str().is_some_and(|id| ids.contains(id)) {
                        "update"
                    } else {
                        "new"
                    },
                });
                if let Some(rebuild) = changes.rebuilds.get(path) {
                    entry["change"] = "rebuild".into();
                    entry["replaces"] = rebuild.replaces.clone().into();
                    entry["files"] = rebuild.files.json();
                    entry["reason"] = custom["rebuild"]["reason"].clone();
                }
                entry
            })
            .collect();
        let withdrawn: Vec<Value> = changes
            .withdrawn
            .iter()
            .map(|(path, w)| {
                let mut entry = named(path);
                entry["kind"] = w["kind"].clone();
                entry["reason"] = w["reason"].clone();
                entry
            })
            .collect();
        let rebuilt: Vec<Value> = changes
            .rebuilt
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "id": r.id,
                    "version": r.version,
                    "files": r.files.json(),
                    "next": r.next,
                })
            })
            .collect();
        let signers: Vec<Value> = match self.root()? {
            Some(root) => {
                let ids =
                    &root.signed.signed.roles.get("targets").map_or_default(|r| r.keyids.clone());
                keys::read_all(&self.keys_dir())?
                    .iter()
                    .filter(|key| ids.contains(&key.key.id()))
                    .map(|key| json!({ "name": key.name, "serial": key.yubikey }))
                    .collect()
            }
            None => Vec::new(),
        };
        Ok(json!({
            "format": FORMAT,
            "action": plan.action.word(),
            "targets": {
                "after": plan.after,
                "version": plan.after + 1,
                "expires": tuf::repo::expires_in(days),
            },
            "signers": signers,
            "added": added,
            "removed": changes.removed.iter().map(named).collect::<Vec<_>>(),
            "verified": changes.verified.iter().map(named).collect::<Vec<_>>(),
            "withdrawn": withdrawn,
            "rebuilt": rebuilt,
            "digest": plan.digest()?,
        }))
    }

    /// Writes [`Repo::plan_json`] to `file`, whole or not at all, and gives
    /// what it wrote.
    ///
    /// # Errors
    ///
    /// If the plan cannot be made or `file` cannot be written.
    pub fn write_plan(&self, plan: &Plan, days: i64, file: &Path) -> Result<Value, String> {
        let value = self.plan_json(plan, days)?;
        let mut text = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
        text.push(b'\n');
        write(file, &text)?;
        Ok(value)
    }
}
