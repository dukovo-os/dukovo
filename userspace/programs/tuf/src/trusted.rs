// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The metadata a client trusts during one update (TUF 5.3 to 5.6), as
//! python-tuf's `TrustedMetadataSet` keeps it: each new file is checked
//! against what is already trusted before it joins, so that the set is
//! only ever metadata that passed.
//!
//! An expired timestamp or snapshot still joins, so that it guards a
//! newer one against rollback, but nothing after it is loaded.

use std::collections::{HashMap, HashSet};

use crate::metadata::{Delegator, Error, Root, Signed, Snapshot, Targets, Timestamp, check_file};

/// The trusted metadata of one update.
#[derive(Debug)]
pub struct Trusted {
    /// The time it is checked against, fixed when the update starts.
    now: i64,
    root: Signed<Root>,
    timestamp: Option<Signed<Timestamp>>,
    snapshot: Option<Signed<Snapshot>>,
    /// Targets roles by name, top-level `targets` and delegated alike.
    targets: HashMap<String, Signed<Targets>>,
    /// Which delegator each targets role was checked against.
    delegations: HashSet<(String, String)>,
}

fn order(why: &str) -> Error {
    Error::Repository(format!("the update's steps are out of order: {why}"))
}

impl Trusted {
    /// Starts from `root`, the root this computer trusts, which only
    /// itself vouches for. Its expiry is checked later, once newer roots
    /// have had their chance to replace it.
    ///
    /// # Errors
    ///
    /// If it is not root metadata signed by its own root keys.
    pub fn new(root: &[u8], now: i64) -> Result<Self, Error> {
        let root = Signed::<Root>::parse(root)?;
        root.verified_by(&root.signed, "root")?;
        Ok(Trusted {
            now,
            root,
            timestamp: None,
            snapshot: None,
            targets: HashMap::new(),
            delegations: HashSet::new(),
        })
    }

    /// The trusted root.
    #[must_use]
    pub fn root(&self) -> &Signed<Root> {
        &self.root
    }

    /// The trusted timestamp, if one has been loaded.
    #[must_use]
    pub fn timestamp(&self) -> Option<&Signed<Timestamp>> {
        self.timestamp.as_ref()
    }

    /// The trusted snapshot, if one has been loaded.
    #[must_use]
    pub fn snapshot(&self) -> Option<&Signed<Snapshot>> {
        self.snapshot.as_ref()
    }

    /// The trusted targets role `role`, if it has been loaded.
    #[must_use]
    pub fn targets(&self, role: &str) -> Option<&Signed<Targets>> {
        self.targets.get(role)
    }

    /// Whether `role` was loaded as `delegator`'s delegate.
    #[must_use]
    pub fn has(&self, role: &str, delegator: &str) -> bool {
        self.delegations.contains(&(delegator.to_string(), role.to_string()))
    }

    /// Takes `data` as the next root (TUF 5.3.4 to 5.3.8): signed by the
    /// trusted root's root keys and by its own, one version on. An
    /// expired root may lead to the next.
    ///
    /// # Errors
    ///
    /// If it fails any of those checks, or a timestamp is already loaded.
    pub fn update_root(&mut self, data: &[u8]) -> Result<(), Error> {
        if self.timestamp.is_some() {
            return Err(order("root after timestamp"));
        }
        let new = Signed::<Root>::parse(data)?;
        new.verified_by(&self.root.signed, "root")?;
        if new.version != self.root.version + 1 {
            return Err(Error::Version(format!(
                "expected root version {}, got {}",
                self.root.version + 1,
                new.version
            )));
        }
        new.verified_by(&new.signed, "root")?;
        self.root = new;
        Ok(())
    }

    /// Takes `data` as the timestamp (TUF 5.4), once the final root has
    /// been checked unexpired (5.3.10).
    ///
    /// # Errors
    ///
    /// [`Error::SameVersion`] if it is the trusted timestamp's version,
    /// which stays; [`Error::Expired`] if it has expired, though it is
    /// still trusted for rollback checks; and any failed check.
    pub fn update_timestamp(&mut self, data: &[u8]) -> Result<(), Error> {
        if self.snapshot.is_some() {
            return Err(order("timestamp after snapshot"));
        }
        if self.root.expired(self.now) {
            return Err(Error::Expired("the final root has expired".into()));
        }
        let new = Signed::<Timestamp>::parse(data)?;
        new.verified_by(&self.root.signed, "timestamp")?;
        if let Some(old) = &self.timestamp {
            if new.version < old.version {
                return Err(Error::Version(format!(
                    "timestamp version {} is older than the trusted {}",
                    new.version, old.version
                )));
            }
            if new.version == old.version {
                return Err(Error::SameVersion);
            }
            let (was, is) = (old.signed.snapshot().version, new.signed.snapshot().version);
            if is < was {
                return Err(Error::Version(format!(
                    "snapshot version {is} is older than the trusted {was}"
                )));
            }
        }
        self.timestamp = Some(new);
        self.check_timestamp()
    }

    fn check_timestamp(&self) -> Result<(), Error> {
        match &self.timestamp {
            Some(timestamp) if timestamp.expired(self.now) => {
                Err(Error::Expired("timestamp.json has expired".into()))
            }
            Some(_) => Ok(()),
            None => Err(order("no timestamp")),
        }
    }

    /// Takes `data` as the snapshot (TUF 5.5): `trusted` when it was
    /// checked once before and kept, so that the timestamp's hashes of a
    /// newer snapshot do not apply to it.
    ///
    /// # Errors
    ///
    /// [`Error::Expired`] or [`Error::Version`] if it has expired or is
    /// not the version timestamp names, though it is still trusted for
    /// rollback checks; and any failed check.
    pub fn update_snapshot(&mut self, data: &[u8], trusted: bool) -> Result<(), Error> {
        let Some(timestamp) = &self.timestamp else {
            return Err(order("snapshot before timestamp"));
        };
        if !self.targets.is_empty() {
            return Err(order("snapshot after targets"));
        }
        self.check_timestamp()?;
        let meta = timestamp.signed.snapshot();
        if !trusted {
            check_file(data, meta.length, meta.hashes.as_ref())?;
        }
        let new = Signed::<Snapshot>::parse(data)?;
        new.verified_by(&self.root.signed, "snapshot")?;
        if let Some(old) = &self.snapshot {
            for (name, file) in &old.signed.meta {
                let Some(new_file) = new.signed.meta.get(name) else {
                    return Err(Error::Repository(format!("the new snapshot leaves out {name}")));
                };
                if new_file.version < file.version {
                    return Err(Error::Version(format!(
                        "{name} version {} is older than the trusted {}",
                        new_file.version, file.version
                    )));
                }
            }
        }
        self.snapshot = Some(new);
        self.check_snapshot()
    }

    fn check_snapshot(&self) -> Result<(), Error> {
        let (Some(timestamp), Some(snapshot)) = (&self.timestamp, &self.snapshot) else {
            return Err(order("no snapshot"));
        };
        if snapshot.expired(self.now) {
            return Err(Error::Expired("snapshot.json has expired".into()));
        }
        let expected = timestamp.signed.snapshot().version;
        if snapshot.version != expected {
            return Err(Error::Version(format!(
                "expected snapshot version {expected}, got {}",
                snapshot.version
            )));
        }
        Ok(())
    }

    /// Takes `data` as the targets role `role`, which `delegator` (root
    /// for the top-level targets) delegates (TUF 5.6).
    ///
    /// # Errors
    ///
    /// If the final snapshot is not valid, the delegator is not trusted,
    /// or `data` fails a check.
    pub fn update_targets(
        &mut self,
        data: &[u8],
        role: &str,
        delegator: &str,
    ) -> Result<(), Error> {
        if self.snapshot.is_none() {
            return Err(order("targets before snapshot"));
        }
        self.check_snapshot()?;
        let by: &dyn Delegator = if delegator == "root" {
            &self.root.signed
        } else {
            match self.targets.get(delegator) {
                Some(targets) => &targets.signed,
                None => return Err(order("a role before its delegator")),
            }
        };
        let snapshot =
            self.snapshot.as_ref().map(|s| &s.signed).ok_or_else(|| order("no snapshot"))?;
        let Some(meta) = snapshot.meta.get(&format!("{role}.json")) else {
            return Err(Error::Repository(format!("snapshot does not list {role}")));
        };
        check_file(data, meta.length, meta.hashes.as_ref())?;
        let new = Signed::<Targets>::parse(data)?;
        new.verified_by(by, role)?;
        if new.version != meta.version {
            return Err(Error::Version(format!(
                "expected {role} version {}, got {}",
                meta.version, new.version
            )));
        }
        if new.expired(self.now) {
            return Err(Error::Expired(format!("{role} has expired")));
        }
        self.targets.insert(role.to_string(), new);
        self.delegations.insert((delegator.to_string(), role.to_string()));
        Ok(())
    }
}
