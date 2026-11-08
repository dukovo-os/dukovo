// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! feature(8): adds and removes what a machine does, a feature at a time
//! (ADR 0316).
//!
//! A feature, such as File sharing, is a file in
//! `/system/resources/features` (feature.toml(5)) that names the services
//! it turns on and the bundles it installs. The files are part of the
//! signed system, so what a feature does changes only with an update.
//!
//! - `feature` and `feature list` list the features, and whether each is
//!   added: every service it names on and every bundle it names
//!   installed for everyone. One with only some of them is *partly*
//!   added, as when `share off` turned smbd off.
//! - `feature info ID` says what one does, and each part's state.
//! - `feature add ID... [--yes]`, as root, installs each bundle a
//!   feature lists from the catalogs the system trusts (bundle(1)), and
//!   turns each service on, now and at each boot (service(8)).
//! - `feature remove ID... [--keep-data | --purge]`, as root, turns its
//!   services off and removes its bundles, but those another added
//!   feature needs. A bundle's settings and data stay unless `--purge`
//!   says to delete them.
//!
//! A service's file names the ports it listens on, so the firewall opens
//! them while it runs (ADR 0334): adding a feature changes no rules.

use std::fs;
use std::io;
use std::path::Path;

use manifest::feature::{self, Feature};
use manifest::service;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};
use crate::{bundles, servicefiles};

const HELP: &str = "Usage: feature [list]
       feature info ID
       feature add [--yes] ID...
       feature remove [--keep-data | --purge] ID...

Adds and removes what this machine does, a feature at a time: each turns
on services and installs bundles, as feature.toml(5) describes.

  list     the features, and whether each is added
  info     what feature ID does, and the state of each of its parts
  add      installs its bundles for everyone and turns its services on,
           now and at each boot; --yes installs without asking
  remove   turns its services off and removes its bundles, but those
           another added feature needs; --purge also deletes the
           bundles' settings and data, which --keep-data, the default,
           keeps";

/// feature(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(l)) if l == "help" => {
                println!("{HELP}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => list(),
        };
    };
    match subcommand.to_str() {
        Some("list") => {
            nothing_more(args)?;
            list()
        }
        Some("info") => {
            let ids = ids(args, &[])?;
            match ids.0.as_slice() {
                [id] => info(id),
                _ => Err(usage("feature info takes one feature's ID".into())),
            }
        }
        Some("add") => {
            let (ids, flags) = ids(args, &["yes"])?;
            add(&ids, flags.contains(&"yes"))
        }
        Some("remove") => {
            let (ids, flags) = ids(args, &["keep-data", "purge"])?;
            if flags.contains(&"keep-data") && flags.contains(&"purge") {
                return Err(usage("feature remove takes --keep-data or --purge, not both".into()));
            }
            remove(&ids, flags.contains(&"purge"))
        }
        _ => Err(usage(format!("no such command: {}\n{HELP}", subcommand.to_string_lossy()))),
    }
}

/// Fails if anything is left in `args`.
fn nothing_more(args: &mut Args) -> Result<(), Fail> {
    match args.next()? {
        None => Ok(()),
        Some(other) => Err(args.unknown(&other)),
    }
}

/// The IDs in `args`, at least one, and which of the long options
/// `flags` names were given.
fn ids(args: &mut Args, flags: &[&'static str]) -> Result<(Vec<String>, Vec<&'static str>), Fail> {
    let mut ids = Vec::new();
    let mut given = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if flags.contains(&l.as_str()) => {
                given.extend(flags.iter().find(|&&f| f == l).copied());
            }
            Arg::Operand(id) => ids.push(id.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    if ids.is_empty() {
        return Err(usage(format!("name a feature: `feature list` lists them\n{HELP}")));
    }
    for id in &ids {
        feature::check_id(id).map_err(usage)?;
    }
    Ok((ids, given))
}

/// Feature `id`'s file, read and checked.
fn read(id: &str) -> Result<Feature, Fail> {
    let path = Path::new(feature::DIR).join(format!("{id}.toml"));
    let text = fs::read_to_string(&path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => {
            fail(format!("no feature is called {id}: `feature list` lists them"))
        }
        _ => util::failed(&path.display(), e),
    })?;
    feature::from_toml(&text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

/// Every feature, by ID, in order.
fn all() -> Result<Vec<(String, Feature)>, Fail> {
    let entries = match fs::read_dir(feature::DIR) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(util::failed(&feature::DIR, e)),
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".toml")?;
            feature::check_id(id).is_ok().then(|| id.to_string())
        })
        .collect();
    ids.sort();
    ids.into_iter().map(|id| read(&id).map(|f| (id, f))).collect()
}

/// What is on: the services that start at boot, by name, and the bundles
/// installed for everyone, asked of bundled only when a feature lists one.
struct Now {
    services: Vec<String>,
    bundles: Vec<String>,
}

impl Now {
    /// The state of every service and bundle the features list.
    fn of(features: &[&Feature]) -> Result<Now, Fail> {
        let mut services = Vec::new();
        for name in features.iter().flat_map(|f| &f.services) {
            if !services.contains(name) && enabled(name) {
                services.push(name.clone());
            }
        }
        let bundles = if features.iter().any(|f| !f.bundles.is_empty()) {
            bundles::installed_for_everyone()?
        } else {
            Vec::new()
        };
        Ok(Now { services, bundles })
    }

    /// Whether `bundle` is installed for everyone: by its full ID, with
    /// its line if it has one, as bundled keys it (ADR 0387).
    fn installed(&self, bundle: &str) -> bool {
        self.bundles.iter().any(|id| id == bundle)
    }

    /// How many of `feature`'s parts are on, of how many.
    fn parts(&self, feature: &Feature) -> (usize, usize) {
        let on = feature.services.iter().filter(|s| self.services.contains(s)).count()
            + feature.bundles.iter().filter(|b| self.installed(b)).count();
        (on, feature.services.len() + feature.bundles.len())
    }

    /// `feature`'s state, as `list` shows it.
    fn state(&self, feature: &Feature) -> &'static str {
        match self.parts(feature) {
            (0, _) => "not added",
            (on, of) if on == of => "added",
            _ => "partly",
        }
    }
}

/// Whether service `name` starts at boot, as Spark reads its file: with
/// the override that sets keys over it, if there is one (ADR 0386). A
/// service with no file, or one that does not read, does not.
fn enabled(name: &str) -> bool {
    let over_path =
        Path::new(servicefiles::DIR).join(service::OVERRIDES).join(format!("{name}.toml"));
    let over = fs::read_to_string(over_path).ok().map(|text| service::Override::from_toml(&text));
    let over = match over {
        // An override that does not read is a problem `service check`
        // names, and Spark then starts the service from its last copy
        // that read; rather than guess, count the service as off.
        Some(Err(_)) => return false,
        Some(Ok(over)) => Some(over),
        None => None,
    };
    servicefiles::text(name)
        .ok()
        .and_then(|text| service::from_toml_over(&text, over.as_ref()).ok())
        .is_some_and(|service| service.enabled)
}

/// `feature list`.
fn list() -> Result<i32, Fail> {
    let features = all()?;
    if features.is_empty() {
        println!("this system has no features");
        return Ok(0);
    }
    let now = Now::of(&features.iter().map(|(_, f)| f).collect::<Vec<_>>())?;
    let width = features.iter().map(|(id, _)| id.len()).max().unwrap_or(2).max(2);
    println!("{:width$}  {:9}  NAME", "ID", "STATE");
    for (id, feature) in &features {
        println!("{id:width$}  {:9}  {}", now.state(feature), feature.name);
    }
    Ok(0)
}

/// `feature info ID`.
fn info(id: &str) -> Result<i32, Fail> {
    let feature = read(id)?;
    let now = Now::of(&[&feature])?;
    println!("{} ({id}): {}", feature.name, now.state(&feature));
    println!("{}", feature.about);
    for name in &feature.services {
        let state = if now.services.contains(name) { "on" } else { "off" };
        println!("  service {name}: {state}");
    }
    for bundle in &feature.bundles {
        let state = if now.installed(bundle) { "installed" } else { "not installed" };
        println!("  bundle {bundle}: {state}");
    }
    Ok(0)
}

/// Fails unless the program runs as root.
fn root_only(verb: &str) -> Result<(), Fail> {
    if util::own_user() == 0 {
        Ok(())
    } else {
        Err(fail(format!(
            "only root {verb}s features: elevate first, as `sudo feature {verb} ...`"
        )))
    }
}

/// `feature add ID...`.
fn add(ids: &[String], yes: bool) -> Result<i32, Fail> {
    root_only("add")?;
    let features = ids.iter().map(|id| read(id)).collect::<Result<Vec<_>, _>>()?;
    for feature in &features {
        let now = Now::of(&[feature])?;
        for bundle in feature.bundles.iter().filter(|b| !now.installed(b)) {
            if bundles::get(bundle, None, true, yes)? != 0 {
                return Err(fail(format!(
                    "{} is not added: {bundle} did not install",
                    feature.name
                )));
            }
        }
        for name in feature.services.iter().filter(|s| !now.services.contains(s)) {
            servicefiles::turn(name, true)?;
        }
        println!("{} is added{}", feature.name, running(&feature.services));
    }
    Ok(0)
}

/// What a feature's services do once added, for its message.
fn running(services: &[String]) -> String {
    match services {
        [] => ".".into(),
        [one] => format!(": {one} runs, and starts at each boot."),
        [rest @ .., last] => {
            format!(": {} and {last} run, and start at each boot.", rest.join(", "))
        }
    }
}

/// The features that stay when `going`, named `ids`, are removed, and
/// whose parts are kept: those added, or partly added, by a part of
/// their own. A part they share with a feature going does not count:
/// otherwise, with File sharing added, every other feature that lists
/// smbd would look partly added, and removing File sharing would never
/// turn smbd off.
fn staying<'a>(
    all: &'a [(String, Feature)],
    ids: &[String],
    going: &[Feature],
    now: &Now,
) -> Vec<&'a Feature> {
    let goes = |part: &String, of: fn(&Feature) -> &Vec<String>| {
        going.iter().any(|g| of(g).contains(part))
    };
    all.iter()
        .filter(|(id, _)| !ids.contains(id))
        .filter(|(_, f)| {
            f.services.iter().any(|s| !goes(s, |g| &g.services) && now.services.contains(s))
                || f.bundles.iter().any(|b| !goes(b, |g| &g.bundles) && now.installed(b))
        })
        .map(|(_, f)| f)
        .collect()
}

/// `feature remove ID...`.
fn remove(ids: &[String], purge: bool) -> Result<i32, Fail> {
    root_only("remove")?;
    let going = ids.iter().map(|id| read(id)).collect::<Result<Vec<_>, _>>()?;
    let all = all()?;
    let now = Now::of(&all.iter().map(|(_, f)| f).collect::<Vec<_>>())?;
    let staying = staying(&all, ids, &going, &now);
    let needed = |is: &dyn Fn(&Feature) -> bool| staying.iter().any(|f| is(f));
    for feature in &going {
        let mut kept = Vec::new();
        for name in &feature.services {
            if needed(&|f: &Feature| f.services.contains(name)) {
                kept.push(name.as_str());
            } else if now.services.contains(name) {
                servicefiles::turn(name, false)?;
            }
        }
        for bundle in &feature.bundles {
            if needed(&|f: &Feature| f.bundles.contains(bundle)) {
                kept.push(bundle.as_str());
            } else if now.installed(bundle) && bundles::remove(bundle, true, Some(purge))? != 0 {
                return Err(fail(format!("{} is not removed: {bundle} stays", feature.name)));
            }
        }
        if kept.is_empty() {
            println!("{} is removed.", feature.name);
        } else {
            let verb = if kept.len() == 1 { "stays" } else { "stay" };
            println!(
                "{} is removed; {} {verb} for the features that need them.",
                feature.name,
                kept.join(", ")
            );
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sharing() -> Feature {
        feature::from_toml(
            "feature_version = 1\nname = \"File sharing\"\nabout = \"x\"\nservices = \
             [\"smbd\", \"nfsd\"]\nbundles = [\"org.example.tool\"]\n",
        )
        .unwrap()
    }

    #[test]
    fn a_feature_is_added_when_every_part_is_on() {
        let feature = sharing();
        let none = Now { services: vec![], bundles: vec![] };
        assert_eq!(none.state(&feature), "not added");
        let some = Now { services: vec!["smbd".into()], bundles: vec![] };
        assert_eq!(some.state(&feature), "partly");
        assert_eq!(some.parts(&feature), (1, 3));
        let all = Now {
            services: vec!["nfsd".into(), "smbd".into()],
            bundles: vec!["org.example.tool".into()],
        };
        assert_eq!(all.state(&feature), "added");
    }

    #[test]
    fn a_part_stays_for_a_feature_that_has_its_own_part_on() {
        let feature = |services: &[&str]| {
            let list: Vec<String> = services.iter().map(|s| format!("\"{s}\"")).collect();
            feature::from_toml(&format!(
                "feature_version = 1\nname = \"x\"\nabout = \"x\"\nservices = [{}]\n",
                list.join(", ")
            ))
            .unwrap()
        };
        let all = vec![
            ("file-sharing".to_string(), feature(&["smbd", "nfsd"])),
            ("windows-sharing".to_string(), feature(&["smbd", "wsdd"])),
        ];
        let ids = vec!["file-sharing".to_string()];
        let going = vec![all[0].1.clone()];
        // Only File sharing was added: smbd is not another feature's.
        let now = Now { services: vec!["smbd".into(), "nfsd".into()], bundles: vec![] };
        assert!(staying(&all, &ids, &going, &now).is_empty());
        // The other is added too, by wsdd: smbd stays for it.
        let now =
            Now { services: vec!["smbd".into(), "nfsd".into(), "wsdd".into()], bundles: vec![] };
        assert_eq!(staying(&all, &ids, &going, &now).len(), 1);
    }

    #[test]
    fn the_message_names_the_services() {
        assert_eq!(running(&[]), ".");
        assert_eq!(running(&["smbd".into()]), ": smbd runs, and starts at each boot.");
        let three = ["a".into(), "b".into(), "c".into()];
        assert_eq!(running(&three), ": a, b and c run, and start at each boot.");
    }
}
