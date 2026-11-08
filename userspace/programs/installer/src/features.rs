// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The features an install adds (ADR 0317): those the Software page
//! ticks, or the answers' profile and `features` choose. A feature is a
//! file in the system's `resources/features` (feature.toml(5), ADR 0316);
//! adding one at install turns each of its services on in the new
//! system's settings, so that Spark starts them from its first boot, as
//! `feature add` would on the running system.
//!
//! The install stick's live system is the release the installer copies,
//! so the Software page and [`check`] read the features from the live
//! `/system`, before anything is written, and [`turn_on`] reads them
//! again from the new system's slot.

use std::fs;
use std::path::Path;

use manifest::{feature, service};

use crate::answers::Answers;
use crate::log::Log;
use crate::settings::{self, Target};

/// Where a system keeps its features, below `/system`.
const FEATURES: &str = "resources/features";

/// Where a system keeps its own service files, below `/system`.
const SERVICES: &str = "resources/default-settings/spark/services";

/// Where the settings keep the service files Spark starts, below
/// `/system/settings`.
const SETTINGS_SERVICES: &str = "spark/services";

/// A feature the release offers.
#[derive(Clone, Debug)]
pub struct Offered {
    /// Its ID, as feature(8) names it: `file-sharing`.
    pub id: String,
    /// Its file.
    pub feature: feature::Feature,
}

impl Offered {
    /// Whether the installer can add it: it installs no bundle. Bundles
    /// come from a catalog, which a new system reaches only once it runs,
    /// so a feature that has some is added after the first boot.
    #[must_use]
    pub fn at_install(&self) -> bool {
        self.feature.bundles.is_empty()
    }
}

/// The features of the system at `system` (`/system`, or a new slot), by
/// ID. A file that does not read is left out, so that the installer
/// refuses it as one the release lacks; feature(8) says what is wrong
/// with it.
#[must_use]
pub fn offered(system: &Path) -> Vec<Offered> {
    let Ok(entries) = fs::read_dir(system.join(FEATURES)) else { return Vec::new() };
    let mut offered: Vec<Offered> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".toml")?;
            feature::check_id(id).ok()?;
            let text = fs::read_to_string(entry.path()).ok()?;
            let feature = feature::from_toml(&text).ok()?;
            Some(Offered { id: id.to_string(), feature })
        })
        .collect();
    offered.sort_by(|a, b| a.id.cmp(&b.id));
    offered
}

/// Checks the features the answers choose against the system at
/// `system`: each one is there, installs no bundle, and has a file for
/// each of its services. Gives them in the answers' order.
///
/// # Errors
///
/// The first problem, naming the answers' `features` key.
pub fn check(answers: &Answers, system: &Path) -> Result<Vec<Offered>, String> {
    let chosen = check_offered(answers, &offered(system))?;
    for found in &chosen {
        for name in &found.feature.services {
            if !system.join(SERVICES).join(format!("{name}.toml")).is_file() {
                let id = &found.id;
                return Err(format!("features: {id} turns on {name}, which this release lacks"));
            }
        }
    }
    Ok(chosen)
}

/// Checks the features the answers choose against those `offered`: each
/// one is there and installs no bundle, as the screens check a setup
/// file before its review. Gives them in the answers' order.
///
/// # Errors
///
/// The first problem, naming the answers' `features` key.
pub fn check_offered(answers: &Answers, offered: &[Offered]) -> Result<Vec<Offered>, String> {
    let mut chosen = Vec::new();
    for id in answers.chosen_features() {
        let Some(found) = offered.iter().find(|offered| offered.id == id) else {
            let ids: Vec<&str> = offered.iter().map(|offered| offered.id.as_str()).collect();
            return Err(format!(
                "features: this release has no feature {id}; it has {}",
                if ids.is_empty() { "none".to_string() } else { ids.join(", ") }
            ));
        };
        if !found.at_install() {
            return Err(format!(
                "features: {id} installs bundles from a catalog, which the new system reaches \
                 only once it runs: add it after the first boot, with `feature add {id}`"
            ));
        }
        chosen.push(found.clone());
    }
    Ok(chosen)
}

/// Turns on, in the new system's settings, each service of the features
/// the answers choose: its own file from the new slot, saying `enabled =
/// true`, which Spark keeps over its default at the first boot.
///
/// # Errors
///
/// If a feature or a service's file is not in the slot, or a file
/// cannot be written.
pub fn turn_on(answers: &Answers, target: &Target, log: &mut Log) -> Result<(), String> {
    let chosen = check(answers, target.slot)?;
    if chosen.is_empty() {
        return Ok(());
    }
    let dir = target.settings.join(SETTINGS_SERVICES);
    settings::make_dir(&target.settings.join("spark"), 0o755)?;
    settings::make_dir(&dir, 0o755)?;
    for offered in &chosen {
        for name in &offered.feature.services {
            let from = target.slot.join(SERVICES).join(format!("{name}.toml"));
            let text = fs::read_to_string(&from)
                .map_err(|err| format!("reading {}: {err}", from.display()))?;
            let text = service::with_enabled(&text, true);
            // The file Spark will read must still read, enabled.
            service::from_toml(&text).map_err(|err| format!("{}: {err}", from.display()))?;
            settings::write_file(&dir.join(format!("{name}.toml")), &text, 0o644)?;
        }
        log.note(&format!(
            "feature {}: {} turned on",
            offered.id,
            offered.feature.services.join(" and ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answers::tests::EXAMPLE;
    use crate::answers::{Answers, Profile};

    const SHARING: &str = "feature_version = 1\nname = \"File sharing\"\nabout = \"Shares \
                           folders.\"\nservices = [\"smbd\", \"nfsd\"]\n";

    const SMBD: &str = "service_version = 1\nprogram = \"/system/commands/smbd\"\nenabled = \
                        false\n";

    /// A system with File sharing, and a feature with a bundle.
    fn system(dir: &Path) {
        fs::create_dir_all(dir.join(FEATURES)).unwrap();
        fs::create_dir_all(dir.join(SERVICES)).unwrap();
        fs::write(dir.join(FEATURES).join("file-sharing.toml"), SHARING).unwrap();
        let web = format!("{SHARING}bundles = [\"org.nginx.nginx\"]\n");
        fs::write(dir.join(FEATURES).join("web-server.toml"), web).unwrap();
        fs::write(dir.join(FEATURES).join("Not-an-id.toml"), SHARING).unwrap();
        fs::write(dir.join(SERVICES).join("smbd.toml"), SMBD).unwrap();
        fs::write(dir.join(SERVICES).join("nfsd.toml"), SMBD.replace("smbd", "nfsd")).unwrap();
    }

    fn adding(ids: &[&str]) -> Answers {
        let mut answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        answers.profile = Profile::Server;
        answers.features.add = ids.iter().map(|id| (*id).to_string()).collect();
        answers
    }

    #[test]
    fn the_release_offers_its_features_by_id() {
        let dir = std::env::temp_dir().join(format!("features-offered-{}", std::process::id()));
        system(&dir);
        let offered = offered(&dir);
        let ids: Vec<&str> = offered.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["file-sharing", "web-server"]);
        assert!(offered[0].at_install());
        assert!(!offered[1].at_install());
        assert!(super::offered(&dir.join("none")).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_install_turns_the_features_services_on() {
        let dir = std::env::temp_dir().join(format!("features-on-{}", std::process::id()));
        let slot = dir.join("slot");
        system(&slot);
        let problem = |ids: &[&str]| check(&adding(ids), &slot).unwrap_err();
        assert!(problem(&["mail"]).contains("no feature mail; it has file-sharing, web-server"));
        assert!(problem(&["web-server"]).contains("feature add web-server"));
        fs::remove_file(slot.join(SERVICES).join("nfsd.toml")).unwrap();
        assert!(problem(&["file-sharing"]).contains("nfsd, which this release lacks"));
        fs::write(slot.join(SERVICES).join("nfsd.toml"), SMBD.replace("smbd", "nfsd")).unwrap();

        let settings = dir.join("settings");
        let state = dir.join("state");
        fs::create_dir_all(&settings).unwrap();
        let target = Target { slot: &slot, settings: &settings, state: &state };
        let mut log = Log::create(&dir.join("install.log"), false).unwrap();
        // None chosen: nothing is written.
        turn_on(&adding(&[]), &target, &mut log).unwrap();
        assert!(!settings.join("spark").exists());
        turn_on(&adding(&["file-sharing"]), &target, &mut log).unwrap();
        for name in ["smbd", "nfsd"] {
            let file = settings.join(SETTINGS_SERVICES).join(format!("{name}.toml"));
            let text = fs::read_to_string(file).unwrap();
            assert!(service::from_toml(&text).unwrap().enabled, "{text}");
        }
        let logged = fs::read_to_string(dir.join("install.log")).unwrap();
        assert!(logged.contains("feature file-sharing: smbd and nfsd turned on"), "{logged}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
