// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The release list, `system.json`: one release of the system, as the
//! updates design's section 4 has it (ADR 0308). It names the version,
//! the commit and toolchain the release was built from and the oldest
//! version that may update to it directly, and then every directory,
//! file and link of `/system` with its path, kind and mode, each file's
//! size and SHA-256, and each link's target.
//!
//! A list is checked whole when it is read ([`Release::parse`]): a path
//! that climbs out of `/system`, a file inside a link, or a field that
//! does not belong to its kind is refused before anything is written
//! from it, so the code that installs a release can trust its shape.

use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The format of the lists this crate writes and reads.
pub const FORMAT: u32 = 1;

/// One release of the system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    /// The list's format, [`FORMAT`].
    pub format: u32,
    /// Dukovo's version, such as `0.14.0`.
    pub version: String,
    /// The git commit it was built from.
    pub commit: String,
    /// The Rust toolchain it was built with, as rust-toolchain.toml pins
    /// it.
    pub toolchain: String,
    /// When it was built, in TUF's form of a time (`2026-11-06T00:00:00Z`).
    pub built: String,
    /// The oldest version that may update to it directly; an older one
    /// goes through a bridge release first (the design's section 8).
    pub updates_from: String,
    /// Every directory, file and link under `/system`, parents before
    /// what they hold.
    pub files: Vec<Entry>,
}

/// One directory, file or link of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Its path under `/system`, such as `commands/spark`.
    pub path: String,
    /// What it is.
    pub kind: Kind,
    /// Its mode in octal, such as `0755`; `0777` for a link.
    pub mode: String,
    /// A file's size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// A file's SHA-256, in lowercase hex, which also names it in a store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// What a link leads to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A directory.
    Directory,
    /// A file.
    File,
    /// A symbolic link.
    Link,
}

/// What a release says about itself beyond its files, for
/// [`Release::of_tree`].
pub struct About {
    /// Dukovo's version.
    pub version: String,
    /// The git commit.
    pub commit: String,
    /// The Rust toolchain.
    pub toolchain: String,
    /// When it was built, in seconds since 1970.
    pub built: i64,
    /// The oldest version that may update to it directly.
    pub updates_from: String,
}

impl Entry {
    /// Its mode's bits, which [`Release::parse`] has checked are octal
    /// and no more than `07777`.
    #[must_use]
    pub fn mode_bits(&self) -> u32 {
        u32::from_str_radix(&self.mode, 8).unwrap_or(0)
    }

    /// A file's SHA-256 and size, or `None` for a directory or link.
    #[must_use]
    pub fn file(&self) -> Option<(&str, u64)> {
        match (self.kind, &self.sha256, self.size) {
            (Kind::File, Some(sha256), Some(size)) => Some((sha256, size)),
            _ => None,
        }
    }
}

impl Release {
    /// The list of the tree at `dir`, the slot of a system as `ob`
    /// stages it, with `about` for the rest.
    ///
    /// # Errors
    ///
    /// If the tree cannot be read, or holds something that is neither a
    /// directory, a file nor a link, or a name that is not UTF-8.
    pub fn of_tree(dir: &Path, about: About) -> Result<Release, String> {
        let mut files = Vec::new();
        walk(dir, "", &mut files)?;
        let release = Release {
            format: FORMAT,
            version: about.version,
            commit: about.commit,
            toolchain: about.toolchain,
            built: tuf::time::format(about.built),
            updates_from: about.updates_from,
            files,
        };
        release.check()?;
        Ok(release)
    }

    /// The list in `bytes`, checked whole ([`Release::check`]).
    ///
    /// # Errors
    ///
    /// If it is not JSON of a release list, is of a newer format, or
    /// fails a check.
    pub fn parse(bytes: &[u8]) -> Result<Release, String> {
        // The format first, so that a newer list says so instead of
        // failing on a field this program does not know.
        let format = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|err| format!("the release list is not JSON: {err}"))?
            .get("format")
            .and_then(serde_json::Value::as_u64);
        match format {
            Some(n) if n == u64::from(FORMAT) => {}
            Some(n) => {
                return Err(format!(
                    "the release list is of format {n}, and this program reads format {FORMAT}: \
                     install the newer release with a newer system"
                ));
            }
            None => return Err("the release list has no format".into()),
        }
        let release: Release = serde_json::from_slice(bytes)
            .map_err(|err| format!("the release list is malformed: {err}"))?;
        release.check()?;
        Ok(release)
    }

    /// The list as it is signed: JSON in two-space lines, so that two
    /// releases' lists compare well with diff.
    ///
    /// # Panics
    ///
    /// Never: a release list's fields are all JSON's.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("a release list is JSON");
        bytes.push(b'\n');
        bytes
    }

    /// The files' total size in bytes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.files.iter().filter_map(Entry::file).map(|(_, size)| size).sum()
    }

    /// Checks the whole list: its version, and each entry's path, place
    /// and fields.
    ///
    /// # Errors
    ///
    /// The first thing wrong, naming the entry.
    pub fn check(&self) -> Result<(), String> {
        let version_ok = |v: &str| {
            !v.is_empty()
                && v.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b))
        };
        if !version_ok(&self.version) || !version_ok(&self.updates_from) {
            return Err(format!(
                "the release list's version {:?} or updates_from {:?} is not a version",
                self.version, self.updates_from
            ));
        }
        if tuf::time::parse(&self.built).is_none() {
            return Err(format!("the release list's build time {:?} is not a time", self.built));
        }
        let mut directories: Vec<&str> = Vec::new();
        let mut previous: Option<&str> = None;
        for entry in &self.files {
            let path = entry.path.as_str();
            check_path(path)?;
            // Parents before what they hold, and no path twice: in order
            // of their names, compared one name at a time.
            if previous.is_some_and(|before| before.split('/').cmp(path.split('/')).is_ge()) {
                return Err(format!("{path}: the release list is out of order there"));
            }
            previous = Some(path);
            // Its folder must be a directory of the list, not a link, so
            // that nothing is ever written through a link.
            if let Some((parent, _)) = path.rsplit_once('/')
                && !directories.contains(&parent)
            {
                return Err(format!("{path}: its folder {parent} is not a directory of the list"));
            }
            entry.check_fields()?;
            if entry.kind == Kind::Directory {
                directories.push(path);
            }
        }
        Ok(())
    }
}

impl Entry {
    /// Checks that the entry has the fields of its kind and only those.
    fn check_fields(&self) -> Result<(), String> {
        let path = &self.path;
        let octal = !self.mode.is_empty() && self.mode.bytes().all(|b| (b'0'..=b'7').contains(&b));
        if !octal
            || self.mode.len() > 5
            || u32::from_str_radix(&self.mode, 8).unwrap_or(u32::MAX) > 0o7777
        {
            return Err(format!("{path}: its mode {:?} is not a mode in octal", self.mode));
        }
        let shaped = match self.kind {
            Kind::Directory => {
                self.size.is_none() && self.sha256.is_none() && self.target.is_none()
            }
            Kind::File => {
                self.size.is_some()
                    && self.sha256.as_deref().is_some_and(is_sha256)
                    && self.target.is_none()
            }
            Kind::Link => {
                self.size.is_none()
                    && self.sha256.is_none()
                    && self.mode == "0777"
                    && self.target.as_deref().is_some_and(|t| !t.is_empty() && !t.contains('\0'))
            }
        };
        if !shaped {
            return Err(format!(
                "{path}: a {} has a mode, {}",
                kind_name(self.kind),
                match self.kind {
                    Kind::Directory => "and nothing more",
                    Kind::File => "a size and a SHA-256 of 64 lowercase hex digits",
                    Kind::Link => "0777, and a target",
                }
            ));
        }
        Ok(())
    }
}

/// Whether `text` is a SHA-256 as the list writes it.
#[must_use]
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Directory => "directory",
        Kind::File => "file",
        Kind::Link => "link",
    }
}

/// Checks that `path` stays inside `/system`: names joined by single
/// slashes, none of them `.` or `..`.
fn check_path(path: &str) -> Result<(), String> {
    let ok = !path.is_empty()
        && !path.contains('\0')
        && path.split('/').all(|name| !name.is_empty() && name != "." && name != "..");
    if ok { Ok(()) } else { Err(format!("{path:?}: not a path inside /system")) }
}

/// Adds the entries of the directory `dir`, whose path in the release is
/// `prefix`, to `files`: each name in order, and what a directory holds
/// right after it.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<Entry>) -> Result<(), String> {
    let read = |err: std::io::Error| format!("{}: {err}", dir.display());
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(read)? {
        let name = entry.map_err(read)?.file_name();
        let name = name
            .into_string()
            .map_err(|name| format!("{}: the name {name:?} is not UTF-8", dir.display()))?;
        names.push(name);
    }
    names.sort();
    for name in names {
        let full = dir.join(&name);
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let meta =
            std::fs::symlink_metadata(&full).map_err(|err| format!("{}: {err}", full.display()))?;
        let mode = format!("{:04o}", meta.permissions().mode() & 0o7777);
        let kind = meta.file_type();
        if kind.is_symlink() {
            let target =
                std::fs::read_link(&full).map_err(|err| format!("{}: {err}", full.display()))?;
            let target = target
                .into_os_string()
                .into_string()
                .map_err(|_| format!("{}: its target is not UTF-8", full.display()))?;
            files.push(Entry {
                path,
                kind: Kind::Link,
                mode: "0777".into(),
                size: None,
                sha256: None,
                target: Some(target),
            });
        } else if kind.is_dir() {
            files.push(Entry {
                path: path.clone(),
                kind: Kind::Directory,
                mode,
                size: None,
                sha256: None,
                target: None,
            });
            walk(&full, &path, files)?;
        } else if kind.is_file() {
            let sha256 = sha256_file(&full)?;
            files.push(Entry {
                path,
                kind: Kind::File,
                mode,
                size: Some(meta.len()),
                sha256: Some(sha256),
                target: None,
            });
        } else {
            return Err(format!("{}: neither a directory, a file nor a link", full.display()));
        }
    }
    Ok(())
}

/// The SHA-256 of the file at `path`, in lowercase hex.
///
/// # Errors
///
/// If it cannot be read.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let got = file.read(&mut buffer).map_err(|err| format!("{}: {err}", path.display()))?;
        if got == 0 {
            break;
        }
        context.update(&buffer[..got]);
    }
    Ok(hex(context.finish().as_ref()))
}

/// `bytes` in lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn scratch(what: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("system-release-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn about() -> About {
        About {
            version: "0.14.0".into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            built: 1_793_923_200,
            updates_from: "0.14.0".into(),
        }
    }

    /// A small slot: a folder, a file in it, an empty file, a link.
    pub(crate) fn tree(dir: &Path) {
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(dir.join("commands/spark"), b"spark's bytes").unwrap();
        std::fs::set_permissions(
            dir.join("commands/spark"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::set_permissions(dir.join("commands"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(dir.join("empty"), b"").unwrap();
        std::fs::set_permissions(dir.join("empty"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::os::unix::fs::symlink("commands", dir.join("bin")).unwrap();
    }

    #[test]
    fn a_tree_is_listed_parents_first_and_reads_back() {
        let dir = scratch("tree");
        tree(&dir);
        let release = Release::of_tree(&dir, about()).unwrap();
        let paths: Vec<&str> = release.files.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["bin", "commands", "commands/spark", "empty"]);
        let spark = &release.files[2];
        assert_eq!((spark.kind, spark.mode.as_str(), spark.size), (Kind::File, "0755", Some(13)));
        assert_eq!(
            spark.sha256.as_deref(),
            Some(
                hex(ring::digest::digest(&ring::digest::SHA256, b"spark's bytes").as_ref())
                    .as_str()
            )
        );
        assert_eq!(release.files[0].target.as_deref(), Some("commands"));
        assert_eq!(release.built, "2026-11-06T00:00:00Z");
        assert_eq!(release.bytes(), 13);
        assert_eq!(Release::parse(&release.to_bytes()).unwrap(), release);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn parsed_with(edit: impl Fn(&mut serde_json::Value)) -> Result<Release, String> {
        let dir = scratch(&format!("edit-{}", rand_suffix()));
        tree(&dir);
        let release = Release::of_tree(&dir, about()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&release.to_bytes()).unwrap();
        edit(&mut value);
        Release::parse(&serde_json::to_vec(&value).unwrap())
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn lists_that_could_write_outside_the_slot_are_refused() {
        let climbs = parsed_with(|v| v["files"][2]["path"] = "commands/../../users".into());
        assert!(climbs.unwrap_err().contains("not a path inside /system"));
        let absolute = parsed_with(|v| v["files"][0]["path"] = "/bin".into());
        assert!(absolute.unwrap_err().contains("not a path inside /system"));
        // A file inside the link `bin` would be written wherever it leads.
        let through_link = parsed_with(|v| {
            v["files"][2]["path"] = "bin/spark".into();
            v["files"].as_array_mut().unwrap().swap(1, 2);
        });
        let why = through_link.unwrap_err();
        assert!(why.contains("is not a directory of the list"), "{why}");
        let twice = parsed_with(|v| v["files"][3]["path"] = "commands/spark".into());
        assert!(twice.unwrap_err().contains("out of order"));
    }

    #[test]
    fn entries_have_the_fields_of_their_kind() {
        let no_hash = parsed_with(|v| {
            v["files"][2].as_object_mut().unwrap().remove("sha256");
        });
        assert!(no_hash.unwrap_err().contains("a file has a mode, a size and a SHA-256"));
        let upper = parsed_with(|v| {
            let hash = v["files"][2]["sha256"].as_str().unwrap().to_uppercase();
            v["files"][2]["sha256"] = hash.into();
        });
        assert!(upper.is_err());
        let mode = parsed_with(|v| v["files"][1]["mode"] = "0789".into());
        assert!(mode.unwrap_err().contains("not a mode in octal"));
        let big = parsed_with(|v| v["files"][1]["mode"] = "17777".into());
        assert!(big.unwrap_err().contains("not a mode in octal"));
        let unknown = parsed_with(|v| v["files"][1]["owner"] = "root".into());
        assert!(unknown.unwrap_err().contains("malformed"));
    }

    #[test]
    fn a_newer_format_says_so() {
        let newer = parsed_with(|v| v["format"] = 2.into());
        assert!(newer.unwrap_err().contains("format 2, and this program reads format 1"));
    }
}
