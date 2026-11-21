// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog in the source tree checked offline as a computer checks
//! what it is served: the chain of roots from the first, the targets
//! signed by the newest root's keys, nothing expired, and every bundle
//! listed under the path its ID and version give. CI runs it on every
//! change, so a pull request that records a signing is checked without a
//! build (ADR 0594).

use tuf::metadata::{Root, Signed, Targets};

use crate::Repo;

/// What a catalog that verifies holds.
#[derive(Debug, PartialEq, Eq)]
pub struct Verified {
    /// The newest root's version.
    pub root: u64,
    /// The targets' version: 0 when none are signed yet.
    pub targets: u64,
    /// The IDs of the keys whose signatures on the targets the root
    /// names for them, sorted.
    pub signed_by: Vec<String>,
    /// How many bundles the targets list.
    pub bundles: usize,
    /// When the targets expire, in seconds since 1970; the root's expiry
    /// when no targets are signed yet.
    pub expires: i64,
}

impl Verified {
    /// It in one line.
    #[must_use]
    pub fn line(&self) -> String {
        let date = tuf::time::format(self.expires);
        let date = date.get(..10).unwrap_or(&date);
        if self.targets == 0 {
            return format!(
                "catalog: root {}, no targets signed yet, good until {date}",
                self.root
            );
        }
        let keys: Vec<String> =
            self.signed_by.iter().map(|id| format!("{}…", id.get(..8).unwrap_or(id))).collect();
        format!(
            "catalog: root {}, targets version {} signed by {}, {} bundles, good until {date}",
            self.root,
            self.targets,
            keys.join(" and "),
            self.bundles
        )
    }
}

impl Repo {
    /// Checks the catalog at `now`, in seconds since 1970, as a computer
    /// checks it: each `N.root.json` from 1 on is signed by its own root
    /// keys and, from 2 on, by those of root N-1; `root.json` is the
    /// newest of them, and has not expired; `targets.json`, if there is
    /// one, is signed by the newest root's targets keys, has not expired,
    /// and lists each bundle as `bundles/ID-VERSION.dkv` of its own ID,
    /// less any line, and version.
    ///
    /// # Errors
    ///
    /// The first thing that does not hold, said so as to be fixed.
    pub fn verify(&self, now: i64) -> Result<Verified, String> {
        let mut newest: Option<(Signed<Root>, Vec<u8>)> = None;
        for version in 1.. {
            let name = format!("{version}.root.json");
            let bytes = match std::fs::read(self.metadata(&name)) {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
                Err(err) => return Err(format!("{name}: {err}")),
            };
            let root = Signed::<Root>::parse(&bytes).map_err(|e| format!("{name}: {e}"))?;
            if root.version != version {
                return Err(format!("{name} says it is root version {}", root.version));
            }
            root.verified_by(&root.signed, "root")
                .map_err(|e| format!("{name} is not signed by its own root keys: {e}"))?;
            if let Some((old, _)) = &newest {
                root.verified_by(&old.signed, "root").map_err(|e| {
                    format!("{name} is not signed by the root keys of version {}: {e}", old.version)
                })?;
            }
            newest = Some((root, bytes));
        }
        let (root, bytes) = newest.ok_or("there is no 1.root.json: sign a root first")?;
        let current =
            std::fs::read(self.metadata("root.json")).map_err(|e| format!("root.json: {e}"))?;
        if current != bytes {
            return Err(format!("root.json is not {}.root.json, the newest root", root.version));
        }
        if root.expired(now) {
            return Err(format!(
                "root version {} expired at {}: sign the root again",
                root.version,
                tuf::time::format(root.expires)
            ));
        }
        let Some(targets) = self.targets()? else {
            return Ok(Verified {
                root: root.version,
                targets: 0,
                signed_by: Vec::new(),
                bundles: 0,
                expires: root.expires,
            });
        };
        let targets = targets.signed;
        targets.verified_by(&root.signed, "targets").map_err(|e| {
            format!(
                "targets.json is not signed by root version {}'s targets keys: {e}",
                root.version
            )
        })?;
        if targets.expired(now) {
            return Err(format!(
                "targets version {} expired at {}: sign them again",
                targets.version,
                tuf::time::format(targets.expires)
            ));
        }
        check_paths(&targets)?;
        let keyids = root.signed.roles.get("targets").map_or_default(|role| role.keyids.clone());
        let mut signed_by: Vec<String> =
            targets.signatures.keys().filter(|id| keyids.contains(id)).cloned().collect();
        signed_by.sort();
        Ok(Verified {
            root: root.version,
            targets: targets.version,
            signed_by,
            bundles: targets.signed.targets.len(),
            expires: targets.expires,
        })
    }
}

/// Checks that each target is listed as `bundles/ID-VERSION.dkv` of its
/// own `custom.id`, less any `@LINE`, and `custom.version`, as
/// [`crate::bundle_target`] lists it.
fn check_paths(targets: &Signed<Targets>) -> Result<(), String> {
    for (path, target) in &targets.signed.targets {
        let custom = target.custom.as_ref();
        let text = |key: &str| custom.and_then(|c| c.get(key)?.as_str());
        let (Some(id), Some(version)) = (text("id"), text("version")) else {
            return Err(format!("{path} lists no ID and version"));
        };
        let id = id.split_once('@').map_or(id, |(id, _)| id);
        let expected = format!("bundles/{id}-{version}.{}", dkv::EXTENSION);
        if *path != expected {
            return Err(format!("{path} lists {id} {version}, whose path is {expected}"));
        }
    }
    Ok(())
}
