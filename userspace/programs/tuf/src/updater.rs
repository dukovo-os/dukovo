// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! TUF's client workflow (TUF 5.3 to 5.7), as python-tuf's `ngclient`
//! runs it: refresh root, timestamp, snapshot and top-level targets in
//! turn, keeping what passes in a directory; find a target by walking
//! the delegations depth first, in order; and fetch it, checked against
//! its length and hashes.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::fetch;
use crate::metadata::{self, Signed, TargetFile, Targets};
use crate::time;
use crate::trusted::Trusted;
use crate::url::Url;

/// The limits of an update: how far it goes and how large each file may
/// be. python-tuf's defaults.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The most new roots one update takes.
    pub root_rotations: u64,
    /// The most targets roles one search visits.
    pub delegations: usize,
    /// The largest root.
    pub root: u64,
    /// The largest timestamp.
    pub timestamp: u64,
    /// The largest snapshot, when the timestamp does not say.
    pub snapshot: u64,
    /// The largest targets role, when the snapshot does not say.
    pub targets: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            root_rotations: 256,
            delegations: 32,
            root: 512_000,
            timestamp: 16_384,
            snapshot: 2_000_000,
            targets: 5_000_000,
        }
    }
}

/// Why an update or a target fetch failed.
#[derive(Debug)]
pub enum Error {
    /// Metadata or a target failed a check.
    Metadata(metadata::Error),
    /// A file could not be fetched.
    Fetch(Url, fetch::Error),
    /// The directory could not be read or written.
    Io(PathBuf, io::Error),
    /// A URL could not be made, or there is none for targets.
    Url(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Metadata(err) => write!(f, "{err}"),
            Error::Fetch(url, err) => write!(f, "{url}: {err}"),
            Error::Io(path, err) => write!(f, "{}: {err}", path.display()),
            Error::Url(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<metadata::Error> for Error {
    fn from(err: metadata::Error) -> Self {
        Error::Metadata(err)
    }
}

/// `text` percent-encoded but for letters, digits and `_.-~`, as
/// Python's `urllib.parse.quote(text, "")`: a role's or a target's name
/// as one file name or URL segment.
#[must_use]
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `path` escaped for a URL's path: all but letters, digits, `/` and
/// the characters a path may have as they are. A target's path is put
/// after the targets' URL as it is, as python-tuf puts it, but a `?` or
/// `#` in it is part of its name.
#[must_use]
pub fn escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Writes `data` to `path` whole: to a file beside it first.
fn write_whole(path: &Path, data: &[u8]) -> Result<(), Error> {
    let io = |err| Error::Io(path.to_path_buf(), err);
    let name = path.file_name().map_or_default(|n| n.to_string_lossy().into_owned());
    let partial = path.with_file_name(format!(".{name}.partial"));
    std::fs::write(&partial, data).map_err(io)?;
    std::fs::rename(&partial, path).map_err(|err| {
        let _ = std::fs::remove_file(&partial);
        io(err)
    })
}

/// A repository's client: its trusted metadata, kept in a directory.
#[derive(Debug)]
pub struct Updater {
    dir: PathBuf,
    metadata: Url,
    targets: Option<Url>,
    limits: Limits,
    trusted: Trusted,
}

impl Updater {
    /// A client for the repository whose metadata is at `metadata` and
    /// targets at `targets`, starting from the root in `dir`, which
    /// came another way than from the repository.
    ///
    /// # Errors
    ///
    /// If `dir`'s root cannot be read, or is not root metadata signed by
    /// its own keys.
    pub fn new(dir: &Path, metadata: &Url, targets: Option<&Url>) -> Result<Self, Error> {
        let path = dir.join("root.json");
        let root = std::fs::read(&path).map_err(|err| Error::Io(path, err))?;
        Ok(Updater {
            dir: dir.to_path_buf(),
            metadata: metadata.base(),
            targets: targets.map(Url::base),
            limits: Limits::default(),
            trusted: Trusted::new(&root, time::now())?,
        })
    }

    /// The trusted metadata.
    #[must_use]
    pub fn trusted(&self) -> &Trusted {
        &self.trusted
    }

    /// Brings root, timestamp, snapshot and the top-level targets up to
    /// date, keeping each that passes in the directory.
    ///
    /// # Errors
    ///
    /// If any cannot be fetched or fails a check; what passed before it
    /// stays trusted and kept.
    pub fn refresh(&mut self) -> Result<(), Error> {
        self.load_root()?;
        self.load_timestamp()?;
        self.load_snapshot()?;
        self.load_targets("targets", "root")?;
        Ok(())
    }

    fn download(&self, role: &str, most: u64, version: Option<u64>) -> Result<Vec<u8>, Error> {
        let name = match version {
            Some(version) => format!("{version}.{}.json", quote(role)),
            None => format!("{}.json", quote(role)),
        };
        let url = Url::parse(&format!("{}{name}", self.metadata))
            .map_err(|err| Error::Url(err.to_string()))?;
        fetch::fetch(&url, most).map_err(|err| Error::Fetch(url, err))
    }

    fn local(&self, role: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(format!("{}.json", quote(role)))).ok()
    }

    fn keep(&self, role: &str, data: &[u8]) -> Result<(), Error> {
        write_whole(&self.dir.join(format!("{}.json", quote(role))), data)
    }

    /// TUF 5.3: each next root in turn, until there is none.
    fn load_root(&mut self) -> Result<(), Error> {
        let first = self.trusted.root().version + 1;
        for version in first..first + self.limits.root_rotations {
            let data = match self.download("root", self.limits.root, Some(version)) {
                Ok(data) => data,
                Err(Error::Fetch(_, fetch::Error::NotFound)) => break,
                Err(err) => return Err(err),
            };
            self.trusted.update_root(&data)?;
            self.keep("root", &data)?;
        }
        Ok(())
    }

    /// TUF 5.4: the kept timestamp first, for rollback checks, then the
    /// repository's.
    fn load_timestamp(&mut self) -> Result<(), Error> {
        if let Some(data) = self.local("timestamp") {
            // Kept but no longer valid as the final one: it still guards
            // against rollback if it joined.
            let _ = self.trusted.update_timestamp(&data);
        }
        let data = self.download("timestamp", self.limits.timestamp, None)?;
        match self.trusted.update_timestamp(&data) {
            Ok(()) => self.keep("timestamp", &data),
            Err(metadata::Error::SameVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// TUF 5.5: the kept snapshot if it is still the one, or the
    /// repository's.
    fn load_snapshot(&mut self) -> Result<(), Error> {
        if let Some(data) = self.local("snapshot")
            && self.trusted.update_snapshot(&data, true).is_ok()
        {
            return Ok(());
        }
        let timestamp =
            self.trusted.timestamp().ok_or_else(|| Error::Url("no timestamp".into()))?;
        let meta = timestamp.signed.snapshot();
        let most = meta.length.unwrap_or(self.limits.snapshot);
        let version = self.consistent().then_some(meta.version);
        let data = self.download("snapshot", most, version)?;
        self.trusted.update_snapshot(&data, false)?;
        self.keep("snapshot", &data)
    }

    fn consistent(&self) -> bool {
        self.trusted.root().signed.consistent_snapshot == Some(true)
    }

    /// TUF 5.6: the targets role `role`, as `delegator` delegates it:
    /// the kept one if it is still the one, or the repository's.
    fn load_targets(&mut self, role: &str, delegator: &str) -> Result<&Signed<Targets>, Error> {
        if !self.trusted.has(role, delegator) {
            let kept = self
                .local(role)
                .is_some_and(|data| self.trusted.update_targets(&data, role, delegator).is_ok());
            if !kept {
                let snapshot =
                    self.trusted.snapshot().ok_or_else(|| Error::Url("no snapshot".into()))?;
                let Some(meta) = snapshot.signed.meta.get(&format!("{role}.json")) else {
                    return Err(metadata::Error::Repository(format!(
                        "{role} is delegated but snapshot does not list it"
                    ))
                    .into());
                };
                let most = meta.length.unwrap_or(self.limits.targets);
                let version = self.consistent().then_some(meta.version);
                let data = self.download(role, most, version)?;
                self.trusted.update_targets(&data, role, delegator)?;
                self.keep(role, &data)?;
            }
        }
        self.trusted.targets(role).ok_or_else(|| Error::Url(format!("{role} did not load")))
    }

    /// The target `path`, found by searching the delegations depth first
    /// in their order (TUF 5.6.7): `None` if no role that may list it
    /// does. Refreshes first if that has not been done.
    ///
    /// # Errors
    ///
    /// If a role on the way cannot be loaded.
    pub fn target(&mut self, path: &str) -> Result<Option<TargetFile>, Error> {
        if self.trusted.targets("targets").is_none() {
            self.refresh()?;
        }
        let mut to_visit = vec![("targets".to_string(), "root".to_string())];
        let mut visited: Vec<String> = Vec::new();
        while visited.len() <= self.limits.delegations {
            let Some((role, delegator)) = to_visit.pop() else { break };
            if visited.contains(&role) {
                continue;
            }
            let targets = self.load_targets(&role, &delegator)?;
            if let Some(target) = targets.signed.targets.get(path) {
                return Ok(Some(target.clone()));
            }
            let children = targets
                .signed
                .delegations
                .as_ref()
                .map_or_default(|delegations| delegations.roles_for(path));
            visited.push(role.clone());
            let mut next = Vec::new();
            for (child, terminating) in children {
                next.push((child, role.clone()));
                if terminating {
                    to_visit.clear();
                    break;
                }
            }
            to_visit.extend(next.into_iter().rev());
        }
        Ok(None)
    }

    /// Where `target`, by the path `path`, is kept in `dir`: its path,
    /// quoted as one name.
    #[must_use]
    pub fn target_path(dir: &Path, path: &str) -> PathBuf {
        dir.join(quote(path))
    }

    /// Whether `file` is already `target`.
    #[must_use]
    pub fn cached(file: &Path, target: &TargetFile) -> bool {
        std::fs::read(file).is_ok_and(|data| {
            metadata::check_file(&data, Some(target.length), Some(&target.hashes)).is_ok()
        })
    }

    /// Fetches `target`, by the path `path`, into `file`: by its hash in
    /// a consistent snapshot (TUF 5.7.3), checked against its length and
    /// hashes before it is written.
    ///
    /// # Errors
    ///
    /// If there is no URL for targets, it cannot be fetched, it is not
    /// what the metadata says, or it cannot be written.
    pub fn download_target(
        &self,
        path: &str,
        target: &TargetFile,
        file: &Path,
    ) -> Result<(), Error> {
        let base = self.targets.as_ref().ok_or_else(|| Error::Url("no URL for targets".into()))?;
        let name = match (self.consistent(), target.hashes.0.first()) {
            (true, Some((_, hash))) => match path.rsplit_once('/') {
                Some((dir, file)) => format!("{dir}/{hash}.{file}"),
                None => format!("{hash}.{path}"),
            },
            _ => path.to_string(),
        };
        let url = Url::parse(&format!("{base}{}", escape_path(&name)))
            .map_err(|err| Error::Url(err.to_string()))?;
        let data = fetch::fetch(&url, target.length).map_err(|err| Error::Fetch(url, err))?;
        metadata::check_file(&data, Some(target.length), Some(&target.hashes))?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Error::Io(parent.to_path_buf(), err))?;
        }
        write_whole(file, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_quoted_as_python_quotes_them() {
        assert_eq!(quote("targets"), "targets");
        assert_eq!(quote("a/b c?#%"), "a%2Fb%20c%3F%23%25");
        assert_eq!(quote("../x~_.-"), "..%2Fx~_.-");
        assert_eq!(quote("é"), "%C3%A9");
    }

    #[test]
    fn target_paths_are_escaped_for_urls() {
        assert_eq!(escape_path("a/b-c_d.tar.gz"), "a/b-c_d.tar.gz");
        assert_eq!(escape_path("a b?c#d%e"), "a%20b%3Fc%23d%25e");
        assert_eq!(escape_path("é"), "%C3%A9");
    }

    #[test]
    fn files_are_written_whole() {
        let dir = std::env::temp_dir().join(format!("tuf-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_whole(&dir.join("root.json"), b"{}").unwrap();
        write_whole(&dir.join("root.json"), b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(dir.join("root.json")).unwrap(), b"{\"a\":1}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
