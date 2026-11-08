// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Features (feature.toml(5), ADR 0316): named groups of what a machine
//! does, such as File sharing, that feature(8) adds and removes at once,
//! and that the installer offers by profile.
//!
//! ```toml
//! feature_version = 1
//! name = "File sharing"
//! about = "Shares folders with Windows PCs, Macs and Linux machines."
//! services = ["smbd", "nfsd"]
//! bundles = []
//! ```
//!
//! A feature's ID is its file's name without `.toml`, in
//! `/system/resources/features`, part of the signed system. Adding it
//! installs each bundle it lists, for everyone, from the catalogs the
//! system trusts, and turns on each service, now and at each boot.
//! Removing it turns them off again and removes the bundles no other
//! added feature needs. A service's own file says which ports it listens
//! on, so the firewall opens them while it runs and a feature names no
//! ports of its own (ADR 0334).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde::Deserialize;

use crate::{Error, service};

/// Where the system's features are, one file each.
pub const DIR: &str = "/system/resources/features";

/// The version of the file this reads.
pub const VERSION: u32 = 1;

/// The longest ID, in characters.
pub const ID_MAX: usize = 40;

/// The longest name, in characters.
pub const NAME_MAX: usize = 40;

/// The longest description, in characters.
pub const ABOUT_MAX: usize = 400;

/// The most services, and the most bundles, a feature lists.
pub const PARTS_MAX: usize = 32;

/// A feature, as its file describes it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    /// The version of the file, at most [`VERSION`].
    pub feature_version: u32,
    /// Its name, as lists and the installer show it.
    pub name: String,
    /// What it is for, in a sentence or two.
    pub about: String,
    /// The services it turns on, by their names in service.toml(5).
    #[serde(default)]
    pub services: Vec<String>,
    /// The bundles it installs, by their IDs (manifest.json(5)).
    #[serde(default)]
    pub bundles: Vec<String>,
}

/// Reads a feature file's `text`, checking every setting.
pub fn from_toml(text: &str) -> Result<Feature, Error> {
    let feature: Feature = toml::from_str(text).map_err(|err| {
        let mut error = Error::from_toml(text, &err);
        error.message = error.message.replace("service.toml(5)", "feature.toml(5)");
        error
    })?;
    if feature.feature_version == 0 || feature.feature_version > VERSION {
        return Err(Error::new(format!(
            "feature_version {} is not one this Dukovo reads, 1 to {VERSION}",
            feature.feature_version
        )));
    }
    let name = feature.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
        return Err(Error::new(format!("name has 1 to {NAME_MAX} characters, on one line")));
    }
    if feature.about.trim().is_empty() || feature.about.chars().count() > ABOUT_MAX {
        return Err(Error::new(format!("about has 1 to {ABOUT_MAX} characters")));
    }
    if feature.services.is_empty() && feature.bundles.is_empty() {
        return Err(Error::new("a feature lists at least one service or bundle"));
    }
    if feature.services.len() > PARTS_MAX || feature.bundles.len() > PARTS_MAX {
        return Err(Error::new(format!(
            "a feature lists at most {PARTS_MAX} services and {PARTS_MAX} bundles"
        )));
    }
    for (i, name) in feature.services.iter().enumerate() {
        service::check_name(name).map_err(|why| Error::new(format!("services: {why}")))?;
        if feature.services[..i].contains(name) {
            return Err(Error::new(format!("services: {name} is listed twice")));
        }
    }
    for (i, id) in feature.bundles.iter().enumerate() {
        check_bundle(id).map_err(|why| Error::new(format!("bundles: {why}")))?;
        if feature.bundles[..i].contains(id) {
            return Err(Error::new(format!("bundles: {id} is listed twice")));
        }
    }
    Ok(feature)
}

/// Checks a feature's ID: 1 to [`ID_MAX`] lowercase letters, digits and
/// `-`, starting with a letter, as `file-sharing`.
pub fn check_id(id: &str) -> Result<(), String> {
    let fits = (1..=ID_MAX).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && id.starts_with(|c: char| c.is_ascii_lowercase());
    if fits {
        Ok(())
    } else {
        Err(format!(
            "`{id}` is not a feature's ID: it has 1 to {ID_MAX} lowercase letters, digits and \
             `-`, and starts with a letter; `feature list` lists them"
        ))
    }
}

/// Checks the form of a bundle's ID, as `org.nginx.nginx` or with its
/// line, `org.postgresql.postgresql@17`: bundle(1) checks the rest when
/// the bundle is installed.
fn check_bundle(id: &str) -> Result<(), String> {
    let fits = (3..=128).contains(&id.len())
        && id.contains('.')
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
        && !id.starts_with(['.', '-', '@']);
    if fits { Ok(()) } else { Err(format!("`{id}` is not a bundle's ID")) }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    const SHARING: &str = "feature_version = 1\nname = \"File sharing\"\nabout = \"Shares \
                           folders.\"\nservices = [\"smbd\", \"nfsd\"]\n";

    #[test]
    fn a_feature_lists_its_services_and_bundles() {
        let feature = from_toml(SHARING).unwrap();
        assert_eq!(feature.name, "File sharing");
        assert_eq!(feature.services, ["smbd", "nfsd"]);
        assert!(feature.bundles.is_empty());
        let web = format!("{SHARING}bundles = [\"org.nginx.nginx\", \"org.php.php-fpm@8\"]\n");
        assert_eq!(from_toml(&web).unwrap().bundles.len(), 2);
    }

    #[test]
    fn what_is_wrong_is_said_plainly() {
        let problem = |text: &str| from_toml(text).unwrap_err().to_string();
        assert!(problem(&SHARING.replace("= 1", "= 2")).contains("not one this Dukovo reads"));
        assert!(problem(&format!("{SHARING}ports = [445]\n")).contains("feature.toml(5)"));
        let empty = SHARING.replace("[\"smbd\", \"nfsd\"]", "[]");
        assert!(problem(&empty).contains("at least one service or bundle"));
        let twice = SHARING.replace("\"nfsd\"", "\"smbd\"");
        assert!(problem(&twice).contains("smbd is listed twice"));
        assert!(problem(&SHARING.replace("\"nfsd\"", "\"../x\"")).contains("service's name"));
        let bundle = format!("{SHARING}bundles = [\"nginx\"]\n");
        assert!(problem(&bundle).contains("not a bundle's ID"));
        assert!(problem(&SHARING.replace("File sharing", "")).contains("name has"));
    }

    #[test]
    fn an_id_is_a_files_plain_name() {
        assert!(check_id("file-sharing").is_ok());
        for bad in ["", "File-sharing", "-x", "1x", "a/b", "a.toml", &"x".repeat(41)] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
    }
}
