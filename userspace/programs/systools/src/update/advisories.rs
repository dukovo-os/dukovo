// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the security advisories say of this machine (ADR 0534): `update
//! advisories`, the Security part of `update check`, and the sentence
//! updated tells when a critical or high advisory applies.
//!
//! The update server signs each Dukovo Security Advisory, DKSA, as a
//! target of its repository, and an index of them. This machine reads
//! both through its TUF client, and matches them here against what it
//! runs: nothing about it is sent. An advisory about the package `dukovo`
//! applies to the release the running slot names; one about an app's ID
//! applies to the app's installed version. The apps' catalogs sign lists
//! of known flaws too (ADR 0389), which are read as `bundle list` reads
//! them; an advisory and a catalog's flaw that share an ID or alias are
//! one finding.
//!
//! The matching and the words are plain functions of the advisories and
//! of what is installed; [`gather`] reads them from the server, the slots
//! and bundled.

use std::collections::HashMap;
use std::path::Path;

use dkv::flaws::{Flaw, List, Severity};
use serde::{Serialize, Serializer};
use system_release::advisory::{self, Advisory};

use super::documents::Unchecked;
use crate::bundles::catalogs;
use crate::bundles::protocol::Listed;
use crate::sysupdate::online::{self, Plan, Settings};
use crate::sysupdate::{place, stage};
use crate::util::Fail;
use crate::{bundles, util};

/// How the fix of an advisory stands on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Fix {
    /// A release with the fix is staged in the spare slot, and starts at
    /// the next restart.
    Staged,
    /// The channel offers a release with the fix, which `update apply`
    /// stages.
    Offered,
    /// The channel offers it, and it reaches this machine in a wave not
    /// open yet.
    Waiting,
    /// The channel offers it, and a hold keeps it back.
    Held,
    /// No release with the fix is staged or offered.
    #[serde(rename = "none")]
    Missing,
}

/// What the channel offers of the system, as far as the fix of an
/// advisory goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Offer {
    /// Nothing newer, or what was withdrawn, or what could not be read.
    #[default]
    Nothing,
    /// The release, which `update apply` stages.
    Update(String),
    /// The release, which waits for this machine's wave.
    Waiting(String),
    /// The release, which a hold keeps back.
    Held(String),
}

impl Offer {
    /// What `plan` offers.
    pub(crate) fn of(plan: &Plan) -> Offer {
        match plan {
            Plan::Current { .. } | Plan::Withdrawn { .. } => Offer::Nothing,
            Plan::Waiting { offered, .. } => Offer::Waiting(offered.clone()),
            Plan::Held { offered, .. } => Offer::Held(offered.clone()),
            Plan::Update { offered, .. } => Offer::Update(offered.clone()),
        }
    }
}

/// What the system has staged and what its channel offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Standing {
    /// The release staged in the spare slot.
    pub(crate) staged: Option<String>,
    /// What the channel offers.
    pub(crate) offer: Offer,
}

/// An advisory that applies to the system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SystemFlaw {
    /// `DKSA-YYYY-NNNN`.
    pub(crate) id: String,
    /// `critical`, `high`, `medium` or `low`.
    #[serde(serialize_with = "word")]
    pub(crate) severity: Severity,
    /// What is wrong, in a line.
    pub(crate) summary: String,
    /// The other IDs it is known by, such as a CVE's.
    pub(crate) aliases: Vec<String>,
    /// The releases that have the fix, the oldest first: one for each
    /// line of releases.
    pub(crate) fixed_in: Vec<String>,
    /// What to do, when the advisory says.
    pub(crate) action: Option<String>,
    /// How the fix stands here.
    pub(crate) fix: Fix,
    /// With `fix` `staged`, `offered`, `waiting` or `held`: the release
    /// that has the fix.
    pub(crate) release: Option<String>,
}

/// A flaw of an installed app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AppFlaw {
    /// The app's ID.
    pub(crate) item: String,
    /// The version installed.
    pub(crate) version: String,
    /// The advisory's ID: a DKSA, or what the catalog's list names.
    pub(crate) id: String,
    /// `critical`, `high`, `medium`, `low` or `unknown`.
    #[serde(serialize_with = "word")]
    pub(crate) severity: Severity,
    /// What is wrong, in a line.
    pub(crate) summary: String,
    /// The other IDs it is known by.
    pub(crate) aliases: Vec<String>,
    /// The versions that have the fix, the oldest first: of the app for
    /// a DKSA, of its `component` for a catalog's flaw.
    pub(crate) fixed_in: Vec<String>,
    /// What to do, when the advisory says.
    pub(crate) action: Option<String>,
    /// `dksa` or `catalog`.
    pub(crate) source: &'static str,
    /// For a catalog's flaw, the component it is in, as `NAME VERSION`.
    pub(crate) component: Option<String>,
    /// The version of the app its catalog offers, when it has the fix.
    pub(crate) fixed_by_update: Option<String>,
}

/// What the advisories say of this machine.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Advisories {
    /// The advisories that apply to the system, the worst first.
    pub(crate) system: Vec<SystemFlaw>,
    /// The flaws of the apps installed, the worst first.
    pub(crate) apps: Vec<AppFlaw>,
    /// The parts that could not be checked, and why.
    pub(crate) unchecked: Vec<Unchecked>,
    /// The release this system runs, when it is known.
    #[serde(skip)]
    pub(crate) running: Option<String>,
}

impl Advisories {
    /// Whether anything applies.
    pub(crate) fn applies(&self) -> bool {
        !self.system.is_empty() || !self.apps.is_empty()
    }
}

/// An app that is installed, as far as the advisories go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Installed {
    /// Its full ID.
    pub(crate) id: String,
    /// Its version.
    pub(crate) version: String,
    /// The catalog it is bound to.
    pub(crate) catalog: Option<String>,
    /// The newest version its catalog lists, when newer.
    pub(crate) update: Option<String>,
}

impl From<&Listed> for Installed {
    fn from(listed: &Listed) -> Installed {
        Installed {
            id: listed.id.clone(),
            version: listed.version.clone(),
            catalog: listed.catalog.clone(),
            update: listed.update.clone(),
        }
    }
}

/// Writes a severity as its word.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn word<S: Serializer>(severity: &Severity, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(severity.word())
}

/// An advisory's severity, as the catalogs' lists have it.
fn severity_of(advisory: &Advisory) -> Severity {
    Severity::from_word(advisory.severity.word()).unwrap_or(Severity::Unknown)
}

/// An app ID without what follows an `@`, which names a variant of it.
fn base_id(id: &str) -> &str {
    id.split_once('@').map_or(id, |(base, _)| base)
}

/// The worst first, and by ID within a severity.
fn by_severity<T>(items: &mut [T], key: impl Fn(&T) -> (Severity, &str)) {
    items.sort_by(|a, b| {
        let (a, b) = (key(a), key(b));
        b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1))
    });
}

/// The advisories that apply to the system running `running`, the worst
/// first, with how the fix of each stands for `standing`.
pub(crate) fn system_flaws(
    advisories: &[Advisory],
    running: &str,
    standing: &Standing,
) -> Vec<SystemFlaw> {
    let lacks = |advisory: &Advisory, version: &str| {
        !advisory::affects(advisory, advisory::SYSTEM, version)
    };
    let mut flaws: Vec<SystemFlaw> = advisories
        .iter()
        .filter(|advisory| advisory::affects(advisory, advisory::SYSTEM, running))
        .map(|advisory| {
            let (fix, release) = match (&standing.staged, &standing.offer) {
                (Some(staged), _) if lacks(advisory, staged) => (Fix::Staged, Some(staged)),
                (_, Offer::Update(offered)) if lacks(advisory, offered) => {
                    (Fix::Offered, Some(offered))
                }
                (_, Offer::Waiting(offered)) if lacks(advisory, offered) => {
                    (Fix::Waiting, Some(offered))
                }
                (_, Offer::Held(offered)) if lacks(advisory, offered) => (Fix::Held, Some(offered)),
                _ => (Fix::Missing, None),
            };
            SystemFlaw {
                id: advisory.id.clone(),
                severity: severity_of(advisory),
                summary: advisory.summary.clone(),
                aliases: advisory.aliases.clone(),
                fixed_in: advisory::fixed_versions(advisory, advisory::SYSTEM),
                action: advisory.action.clone(),
                fix,
                release: release.cloned(),
            }
        })
        .collect();
    by_severity(&mut flaws, |flaw| (flaw.severity, &flaw.id));
    flaws
}

/// The names a flaw is known by: its ID and its aliases.
fn names<'a>(id: &'a str, aliases: &'a [String]) -> Vec<&'a str> {
    std::iter::once(id).chain(aliases.iter().map(String::as_str)).collect()
}

/// The flaws of the `apps`, the worst first: each advisory that names an
/// app's ID in a range with its version, and each flaw of the list the
/// app's catalog signed (`lists`, by the catalog's name) for its version,
/// but those the advisories already name.
pub(crate) fn app_flaws(
    advisories: &[Advisory],
    apps: &[Installed],
    lists: &HashMap<String, List>,
) -> Vec<AppFlaw> {
    let mut found = Vec::new();
    for app in apps {
        let list = app.catalog.as_ref().and_then(|name| lists.get(name));
        let mut own = Vec::new();
        for advisory in advisories {
            let package = [app.id.as_str(), base_id(&app.id)]
                .into_iter()
                .find(|package| advisory::affects(advisory, package, &app.version));
            let Some(package) = package else { continue };
            let fixed_by_update =
                app.update.clone().filter(|update| !advisory::affects(advisory, package, update));
            own.push(AppFlaw {
                item: app.id.clone(),
                version: app.version.clone(),
                id: advisory.id.clone(),
                severity: severity_of(advisory),
                summary: advisory.summary.clone(),
                aliases: advisory.aliases.clone(),
                fixed_in: advisory::fixed_versions(advisory, package),
                action: advisory.action.clone(),
                source: "dksa",
                component: None,
                fixed_by_update,
            });
        }
        if let Some(list) = list {
            for flaw in list.of(&app.id, &app.version) {
                let known = names(&flaw.id, &flaw.aliases);
                let same = own.iter().any(|dksa| {
                    names(&dksa.id, &dksa.aliases).iter().any(|name| known.contains(name))
                });
                if !same {
                    own.push(catalog_flaw(app, flaw, list));
                }
            }
        }
        found.extend(own);
    }
    by_severity(&mut found, |flaw| (flaw.severity, &flaw.id));
    found
}

/// The flaw `flaw` that `list` knows in `app`.
fn catalog_flaw(app: &Installed, flaw: &Flaw, list: &List) -> AppFlaw {
    let known = names(&flaw.id, &flaw.aliases);
    // The offered version has the fix when the list looked at it and
    // names the flaw no more.
    let fixed_by_update = app.update.clone().filter(|update| {
        list.checked(&app.id, update)
            && !list
                .of(&app.id, update)
                .any(|left| names(&left.id, &left.aliases).iter().any(|name| known.contains(name)))
    });
    AppFlaw {
        item: app.id.clone(),
        version: app.version.clone(),
        id: flaw.id.clone(),
        severity: flaw.severity,
        summary: flaw.summary.clone(),
        aliases: flaw.aliases.clone(),
        fixed_in: flaw.fixed.iter().cloned().collect(),
        action: None,
        source: "catalog",
        component: Some(flaw.component.clone()),
        fixed_by_update,
    }
}

/// The advisories of the update server, each read and checked.
fn fetch(client: &mut online::Client) -> Result<Vec<Advisory>, Fail> {
    let Some(index) = client.advisories_index()? else { return Ok(Vec::new()) };
    let mut advisories = Vec::new();
    for entry in &index.advisories {
        let Some(advisory) = client.advisory(&entry.id)? else {
            return Err(util::fail(format!(
                "the index lists {}, and the server does not offer it",
                entry.id
            )));
        };
        advisories.push(advisory);
    }
    Ok(advisories)
}

/// The release staged in the spare slot, when this can read the slot
/// state and it is another release than `running`.
fn staged(root: &place::Root, running: &str) -> Option<String> {
    let (_, state) = place::state(root).ok()?;
    let slot = state.trying.filter(|slot| *slot != root.running)?;
    root.about(slot).map(|about| about.version).filter(|version| version != running)
}

/// The system part: the release this system runs, the advisories that
/// apply to it, and why the channel's offer could not be read, if so.
fn system_part(
    client: &mut online::Client,
    advisories: &[Advisory],
) -> Result<(String, Vec<SystemFlaw>, Option<String>), Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let mut standing = Standing::default();
    let mut problem = None;
    // Only an advisory that applies is worth the look at the offer.
    if advisories.iter().any(|advisory| advisory::affects(advisory, advisory::SYSTEM, &running)) {
        standing.staged = staged(&root, &running);
        match online::plan(client, &running, None) {
            Ok(plan) => standing.offer = Offer::of(&plan),
            Err(why) => problem = Some(why.message),
        }
    }
    let flaws = system_flaws(advisories, &running, &standing);
    Ok((running, flaws, problem))
}

/// The apps part: what is installed, and the lists of flaws of the
/// catalogs they are bound to, as bundled kept them.
fn apps_part(advisories: &[Advisory]) -> Result<Vec<AppFlaw>, Fail> {
    let mut apps: Vec<Installed> = Vec::new();
    for listed in &bundles::installed()? {
        let app = Installed::from(listed);
        if !apps.iter().any(|seen| seen.id == app.id && seen.version == app.version) {
            apps.push(app);
        }
    }
    let state = Path::new(catalogs::STATE);
    let mut lists = HashMap::new();
    for name in apps.iter().filter_map(|app| app.catalog.as_ref()) {
        if !lists.contains_key(name)
            && let Some(list) = catalogs::read_flaws(state, name)
        {
            lists.insert(name.clone(), list);
        }
    }
    Ok(app_flaws(advisories, &apps, &lists))
}

/// What the advisories say of this machine: the system's, and with
/// `with_apps` the apps'. A part that cannot be read is named, and the
/// others still are: with the server out of reach, the apps' flaws from
/// their catalogs' lists, as bundled last kept them, are still found.
pub(crate) fn gather(with_apps: bool) -> Advisories {
    let mut found = Advisories::default();
    let server = Settings::load().and_then(online::Client::connect).and_then(|mut client| {
        let advisories = fetch(&mut client)?;
        Ok((client, advisories))
    });
    let mut advisories = Vec::new();
    match server {
        Ok((mut client, read)) => {
            advisories = read;
            match system_part(&mut client, &advisories) {
                Ok((running, flaws, problem)) => {
                    found.running = Some(running);
                    found.system = flaws;
                    if let Some(why) = problem {
                        let said = format!("the release the channel offers cannot be read: {why}");
                        found.unchecked.push(Unchecked::new("system", &said));
                    }
                }
                Err(why) => {
                    let said =
                        format!("the system cannot be checked for advisories: {}", why.message);
                    found.unchecked.push(Unchecked::new("system", &said));
                }
            }
        }
        Err(why) => {
            let said = format!("the advisories cannot be read: {}", why.message);
            found.unchecked.push(Unchecked::new("system", &said));
        }
    }
    if with_apps {
        match apps_part(&advisories) {
            Ok(flaws) => found.apps = flaws,
            Err(why) => {
                let said = format!("the apps cannot be checked for flaws: {}", why.message);
                found.unchecked.push(Unchecked::new("apps", &said));
            }
        }
    }
    found
}

/// What to say of how the fix of a system advisory stands.
fn fix_said(flaw: &SystemFlaw) -> String {
    let release = flaw.release.as_deref().unwrap_or_default();
    match flaw.fix {
        Fix::Staged => format!("Dukovo {release} has the fix and is staged: restart to finish"),
        Fix::Offered => {
            format!("Dukovo {release} has the fix and is offered: `update apply` stages it")
        }
        Fix::Waiting => {
            format!("Dukovo {release} has the fix and is offered; it waits for this machine's wave")
        }
        Fix::Held => format!(
            "Dukovo {release} has the fix, and a hold keeps it back: `update unhold system` \
             lifts the hold"
        ),
        Fix::Missing if flaw.fixed_in.is_empty() => "no fix is known yet".into(),
        Fix::Missing => format!(
            "fixed in Dukovo {}; this system is not offered it yet",
            flaw.fixed_in.join(" and ")
        ),
    }
}

/// What to say of how the fix of an app's flaw stands.
fn app_fix_said(flaw: &AppFlaw) -> String {
    if let Some(update) = &flaw.fixed_by_update {
        return format!("version {update} has the fix and is offered: `update apply` takes it");
    }
    if flaw.fixed_in.is_empty() {
        return "no fix is known yet".into();
    }
    let versions = flaw.fixed_in.join(" and ");
    match &flaw.component {
        Some(component) => format!("{component} is fixed in {versions}"),
        None => format!("fixed in {versions}"),
    }
}

/// `text`'s lines, each with the prefix every line of `update` has.
fn said(lines: &[String]) {
    for line in lines {
        println!("update: {line}");
    }
}

/// The lines of `update advisories`: what applies, with how the fix
/// stands and what the advisory says to do, or that nothing does.
pub(crate) fn lines(found: &Advisories) -> Vec<String> {
    let mut lines = Vec::new();
    for flaw in &found.system {
        let aliases = if flaw.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", flaw.aliases.join(", "))
        };
        lines.push(format!(
            "Dukovo: {}, {}: {}{aliases}",
            flaw.id,
            flaw.severity.word(),
            flaw.summary
        ));
        lines.push(format!("  {}", fix_said(flaw)));
        lines.extend(flaw.action.iter().map(|action| format!("  {action}")));
    }
    for flaw in &found.apps {
        let aliases = if flaw.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", flaw.aliases.join(", "))
        };
        let source = match &flaw.component {
            Some(component) => format!(", in {component}"),
            None => String::new(),
        };
        lines.push(format!(
            "{} {}: {}, {}: {}{aliases}{source}",
            flaw.item,
            flaw.version,
            flaw.id,
            flaw.severity.word(),
            flaw.summary
        ));
        lines.push(format!("  {}", app_fix_said(flaw)));
        lines.extend(flaw.action.iter().map(|action| format!("  {action}")));
    }
    lines.extend(found.unchecked.iter().map(|part| part.why.clone()));
    if !found.applies() && found.unchecked.is_empty() {
        lines.push(match &found.running {
            Some(running) => {
                format!("no advisory affects Dukovo {running}, and no flaw is known in its apps")
            }
            None => "no advisory affects this system, and no flaw is known in its apps".into(),
        });
    }
    lines
}

/// Tells of `found` as `update advisories` does.
pub(crate) fn print(found: &Advisories) {
    said(&lines(found));
}

/// The exit status of `update advisories`: 0 when nothing applies, 1
/// when something does, and 3 when nothing was found but a part could not
/// be checked.
pub(crate) fn status(found: &Advisories) -> i32 {
    if found.applies() {
        1
    } else if found.unchecked.is_empty() {
        0
    } else {
        3
    }
}

/// The Security part of `update check`: a line for each advisory that
/// applies to the system, then how many flaws the apps have.
pub(crate) fn check_lines(found: &Advisories) -> Vec<String> {
    let mut lines: Vec<String> = found
        .system
        .iter()
        .map(|flaw| {
            format!(
                "{} ({}) affects this system: {}; {}",
                flaw.id,
                flaw.severity.word(),
                flaw.summary,
                fix_said(flaw)
            )
        })
        .collect();
    match found.apps.len() {
        0 => {}
        1 => lines.push("1 flaw is known in the apps: `update advisories` lists it".into()),
        count => lines
            .push(format!("{count} flaws are known in the apps: `update advisories` lists them")),
    }
    lines.extend(found.unchecked.iter().map(|part| part.why.clone()));
    if !found.applies() && found.unchecked.is_empty() {
        lines.push("no advisory affects this system, and no flaw is known in its apps".into());
    }
    lines
}

/// What updated tells when an advisory of high or critical severity
/// applies to the system and its fix is not staged yet: one sentence, or
/// none.
pub(crate) fn notice(flaws: &[SystemFlaw]) -> Option<String> {
    let serious: Vec<&SystemFlaw> = flaws
        .iter()
        .filter(|flaw| flaw.severity >= Severity::High && flaw.fix != Fix::Staged)
        .collect();
    let worst = serious.iter().map(|flaw| flaw.severity).max()?;
    let word = worst.word();
    Some(if let [_] = serious.as_slice() {
        format!(
            "A {word} security advisory affects this system: `update advisories` says what to do"
        )
    } else {
        format!(
            "{} security advisories, the worst {word}, affect this system: `update advisories` \
             says what to do",
            serious.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example advisory of system-release's tests: the system from
    /// 1.4.0 to before 1.4.2 and from 1.3.5 to before 1.3.9.1, high, and
    /// the app `org.dukovo.ripgrep` before 14.1.1.
    const FIXTURE: &str =
        include_str!("../../../system-release/tests/fixtures/DKSA-2026-0001.json");

    fn fixture() -> Advisory {
        Advisory::parse(FIXTURE.as_bytes()).unwrap()
    }

    /// An advisory about `package`, `severity`, from `introduced` to
    /// before `fixed`.
    fn advisory(
        id: &str,
        severity: &str,
        package: &str,
        introduced: &str,
        fixed: &str,
    ) -> Advisory {
        let text = format!(
            r#"{{
  "schema_version": "1.6.0",
  "id": "{id}",
  "modified": "2026-11-15T09:30:00Z",
  "published": "2026-11-10T09:00:00Z",
  "summary": "A flaw of {package}",
  "details": "Details.",
  "affected": [{{
    "package": {{"ecosystem": "Dukovo", "name": "{package}"}},
    "ranges": [{{"type": "ECOSYSTEM", "events": [{{"introduced": "{introduced}"}}, {{"fixed": "{fixed}"}}]}}]
  }}],
  "database_specific": {{"severity": "{severity}"}}
}}"#
        );
        Advisory::parse(text.as_bytes()).unwrap()
    }

    fn offer(version: &str) -> Standing {
        Standing { staged: None, offer: Offer::Update(version.into()) }
    }

    #[test]
    fn a_system_advisory_applies_to_the_versions_in_its_ranges() {
        let advisories = [fixture()];
        let ids = |running: &str| -> Vec<String> {
            system_flaws(&advisories, running, &Standing::default())
                .into_iter()
                .map(|flaw| flaw.id)
                .collect()
        };
        assert_eq!(ids("1.4.1"), ["DKSA-2026-0001"]);
        assert_eq!(ids("1.4.1.1"), ["DKSA-2026-0001"]);
        assert_eq!(ids("1.3.7"), ["DKSA-2026-0001"]);
        assert!(ids("1.4.2").is_empty());
        assert!(ids("1.3.4").is_empty());
        assert!(ids("1.3.9.1").is_empty());
        assert!(ids("2.0.0").is_empty());
    }

    #[test]
    fn the_fix_stands_as_staged_offered_waiting_held_or_missing() {
        let advisories = [fixture()];
        let flaw = |standing: &Standing| system_flaws(&advisories, "1.4.1", standing).remove(0);

        let staged = flaw(&Standing { staged: Some("1.4.2".into()), offer: Offer::Nothing });
        assert_eq!((staged.fix, staged.release.as_deref()), (Fix::Staged, Some("1.4.2")));
        assert_eq!(staged.fixed_in, ["1.3.9.1", "1.4.2"]);

        let offered = flaw(&offer("1.4.2"));
        assert_eq!((offered.fix, offered.release.as_deref()), (Fix::Offered, Some("1.4.2")));

        let waiting = flaw(&Standing { staged: None, offer: Offer::Waiting("1.4.2".into()) });
        assert_eq!(waiting.fix, Fix::Waiting);

        let held = flaw(&Standing { staged: None, offer: Offer::Held("1.4.2".into()) });
        assert_eq!(held.fix, Fix::Held);

        // A release that still has the flaw is no fix, staged or offered.
        let stale =
            flaw(&Standing { staged: Some("1.4.1.1".into()), offer: offer("1.4.1.1").offer });
        assert_eq!((stale.fix, stale.release), (Fix::Missing, None));
        let nothing = flaw(&Standing::default());
        assert_eq!((nothing.fix, nothing.release), (Fix::Missing, None));
    }

    #[test]
    fn the_worst_comes_first() {
        let advisories = [
            advisory("DKSA-2026-0002", "low", "dukovo", "0", "9.0.0"),
            advisory("DKSA-2026-0004", "critical", "dukovo", "0", "9.0.0"),
            advisory("DKSA-2026-0003", "critical", "dukovo", "1.0.0", "9.0.0"),
            advisory("DKSA-2026-0005", "medium", "other", "0", "9.0.0"),
        ];
        let flaws = system_flaws(&advisories, "1.4.1", &Standing::default());
        let ids: Vec<&str> = flaws.iter().map(|flaw| flaw.id.as_str()).collect();
        assert_eq!(ids, ["DKSA-2026-0003", "DKSA-2026-0004", "DKSA-2026-0002"]);
    }

    fn app(id: &str, version: &str, catalog: Option<&str>, update: Option<&str>) -> Installed {
        Installed {
            id: id.into(),
            version: version.into(),
            catalog: catalog.map(Into::into),
            update: update.map(Into::into),
        }
    }

    fn flaw(app: &str, version: &str, id: &str, aliases: &[&str], severity: Severity) -> Flaw {
        Flaw {
            bundle: app.into(),
            version: version.into(),
            id: id.into(),
            aliases: aliases.iter().map(ToString::to_string).collect(),
            component: "libxyz 1.2.0".into(),
            severity,
            summary: "a flaw of the library".into(),
            fixed: Some("1.2.1".into()),
        }
    }

    fn list(checked: &[(&str, &str)], flaws: Vec<Flaw>) -> List {
        List {
            made: "2026-11-15T00:00:00Z".into(),
            checked: checked.iter().map(|(id, v)| (id.to_string(), v.to_string())).collect(),
            flaws,
        }
    }

    #[test]
    fn an_app_has_the_flaws_of_the_advisories_that_name_its_id() {
        let advisories = [fixture()];
        // Padded: the app's version has two numbers.
        let old = app("org.dukovo.ripgrep", "14.0", None, Some("14.1.1"));
        let found = app_flaws(&advisories, &[old], &HashMap::new());
        assert_eq!(found.len(), 1);
        let flaw = &found[0];
        assert_eq!((flaw.id.as_str(), flaw.source), ("DKSA-2026-0001", "dksa"));
        assert_eq!(flaw.fixed_in, ["14.1.1"]);
        assert_eq!(flaw.fixed_by_update.as_deref(), Some("14.1.1"));

        // The fixed version, another app, and the system's own range.
        let fixed = app("org.dukovo.ripgrep", "14.1.1", None, None);
        let other = app("org.dukovo.fd", "1.0.0", None, None);
        let named_dukovo = app("dukovo", "1.4.1", None, None);
        assert!(app_flaws(&advisories, &[fixed, other], &HashMap::new()).is_empty());
        // `dukovo` is the system's name: an app of that ID is matched like any.
        assert_eq!(app_flaws(&advisories, &[named_dukovo], &HashMap::new()).len(), 1);
    }

    #[test]
    fn an_app_variant_is_matched_by_its_base_id_too() {
        let advisories = [fixture()];
        let variant = app("org.dukovo.ripgrep@x86_64", "14.0.0", None, None);
        let found = app_flaws(&advisories, &[variant], &HashMap::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].item, "org.dukovo.ripgrep@x86_64");
        assert!(found[0].fixed_by_update.is_none());
    }

    #[test]
    fn an_app_has_the_flaws_of_its_catalog_s_list_and_an_update_that_ends_them() {
        let lists = HashMap::from([(
            "dukovo".to_string(),
            list(
                &[("org.example.notes", "1.0.0"), ("org.example.notes", "1.1.0")],
                vec![
                    flaw(
                        "org.example.notes",
                        "1.0.0",
                        "RUSTSEC-2026-0001",
                        &["CVE-2026-1"],
                        Severity::High,
                    ),
                    flaw("org.example.notes", "1.0.0", "RUSTSEC-2026-0002", &[], Severity::Low),
                    flaw("org.example.notes", "1.1.0", "RUSTSEC-2026-0002", &[], Severity::Low),
                    flaw("org.example.other", "1.0.0", "RUSTSEC-2026-0003", &[], Severity::High),
                ],
            ),
        )]);
        let notes = app("org.example.notes", "1.0.0", Some("dukovo"), Some("1.1.0"));
        let found = app_flaws(&[], &[notes], &lists);
        let ids: Vec<&str> = found.iter().map(|flaw| flaw.id.as_str()).collect();
        assert_eq!(ids, ["RUSTSEC-2026-0001", "RUSTSEC-2026-0002"]);
        assert_eq!(found[0].source, "catalog");
        assert_eq!(found[0].component.as_deref(), Some("libxyz 1.2.0"));
        assert_eq!(found[0].fixed_in, ["1.2.1"]);
        // The update drops the first flaw and keeps the second.
        assert_eq!(found[0].fixed_by_update.as_deref(), Some("1.1.0"));
        assert_eq!(found[1].fixed_by_update, None);

        // An app of no catalog, or of one with no list, has none.
        let own = app("org.example.notes", "1.0.0", None, None);
        let unlisted = app("org.example.notes", "1.0.0", Some("elsewhere"), None);
        assert!(app_flaws(&[], &[own, unlisted], &lists).is_empty());
    }

    #[test]
    fn an_update_the_list_did_not_look_at_is_no_fix() {
        let lists = HashMap::from([(
            "dukovo".to_string(),
            list(
                &[("org.example.notes", "1.0.0")],
                vec![flaw("org.example.notes", "1.0.0", "RUSTSEC-2026-0001", &[], Severity::High)],
            ),
        )]);
        let notes = app("org.example.notes", "1.0.0", Some("dukovo"), Some("1.1.0"));
        let found = app_flaws(&[], &[notes], &lists);
        assert_eq!(found[0].fixed_by_update, None);
    }

    #[test]
    fn an_advisory_and_a_catalog_flaw_that_share_a_name_are_one_finding() {
        let advisories = [fixture()];
        let lists = HashMap::from([(
            "dukovo".to_string(),
            list(
                &[("org.dukovo.ripgrep", "14.0.0")],
                vec![
                    flaw(
                        "org.dukovo.ripgrep",
                        "14.0.0",
                        "RUSTSEC-2026-0009",
                        &["CVE-2026-12345"],
                        Severity::High,
                    ),
                    flaw(
                        "org.dukovo.ripgrep",
                        "14.0.0",
                        "RUSTSEC-2026-0010",
                        &[],
                        Severity::Medium,
                    ),
                ],
            ),
        )]);
        let ripgrep = app("org.dukovo.ripgrep", "14.0.0", Some("dukovo"), None);
        let found = app_flaws(&advisories, &[ripgrep], &lists);
        let ids: Vec<(&str, &str)> = found.iter().map(|f| (f.id.as_str(), f.source)).collect();
        assert_eq!(ids, [("DKSA-2026-0001", "dksa"), ("RUSTSEC-2026-0010", "catalog")]);
    }

    fn found(system: Vec<SystemFlaw>, apps: Vec<AppFlaw>) -> Advisories {
        Advisories { system, apps, unchecked: Vec::new(), running: Some("1.4.1".into()) }
    }

    #[test]
    fn the_text_says_what_applies_how_its_fix_stands_and_what_to_do() {
        let system = system_flaws(&[fixture()], "1.4.1", &offer("1.4.2"));
        let apps = app_flaws(
            &[fixture()],
            &[app("org.dukovo.ripgrep", "14.0.0", None, Some("14.1.1"))],
            &HashMap::new(),
        );
        let said = lines(&found(system, apps));
        assert_eq!(
            said,
            [
                "Dukovo: DKSA-2026-0001, high: Example: a local user can read another user's \
                 files through a flaw in the file service (CVE-2026-12345, GHSA-xxxx-xxxx-xxxx)",
                "  Dukovo 1.4.2 has the fix and is offered: `update apply` stages it",
                "  Update to Dukovo 1.4.2 with `update apply`, then restart.",
                "org.dukovo.ripgrep 14.0.0: DKSA-2026-0001, high: Example: a local user can \
                 read another user's files through a flaw in the file service (CVE-2026-12345, \
                 GHSA-xxxx-xxxx-xxxx)",
                "  version 14.1.1 has the fix and is offered: `update apply` takes it",
                "  Update to Dukovo 1.4.2 with `update apply`, then restart.",
            ]
        );
    }

    #[test]
    fn each_standing_of_a_fix_has_its_words() {
        let say = |standing: &Standing| {
            fix_said(&system_flaws(&[fixture()], "1.4.1", standing).remove(0))
        };
        assert_eq!(
            say(&Standing { staged: Some("1.4.2".into()), offer: Offer::Nothing }),
            "Dukovo 1.4.2 has the fix and is staged: restart to finish"
        );
        assert_eq!(
            say(&Standing { staged: None, offer: Offer::Waiting("1.4.2".into()) }),
            "Dukovo 1.4.2 has the fix and is offered; it waits for this machine's wave"
        );
        assert!(
            say(&Standing { staged: None, offer: Offer::Held("1.4.2".into()) })
                .contains("`update unhold system`")
        );
        assert_eq!(
            say(&Standing::default()),
            "fixed in Dukovo 1.3.9.1 and 1.4.2; this system is not offered it yet"
        );
        let open = advisory("DKSA-2026-0002", "low", "dukovo", "0", "9.0.0");
        let mut unfixed = system_flaws(&[open], "1.4.1", &Standing::default()).remove(0);
        unfixed.fixed_in.clear();
        assert_eq!(fix_said(&unfixed), "no fix is known yet");
    }

    #[test]
    fn nothing_that_applies_is_said_and_a_part_not_checked_is_not_nothing() {
        let none = found(Vec::new(), Vec::new());
        assert_eq!(
            lines(&none),
            ["no advisory affects Dukovo 1.4.1, and no flaw is known in its apps"]
        );
        assert_eq!(status(&none), 0);

        let mut unreached = Advisories::default();
        unreached.unchecked.push(Unchecked::new("system", "the advisories cannot be read: down"));
        assert_eq!(lines(&unreached), ["the advisories cannot be read: down"]);
        assert_eq!(status(&unreached), 3);

        // What the apps have is still found, and is what the status says.
        unreached.apps = app_flaws(
            &[fixture()],
            &[app("org.dukovo.ripgrep", "14.0.0", None, None)],
            &HashMap::new(),
        );
        assert_eq!(status(&unreached), 1);
        assert_eq!(lines(&unreached).len(), 4);
    }

    #[test]
    fn a_check_has_a_line_for_each_system_advisory_and_a_count_of_the_apps_flaws() {
        let system = system_flaws(&[fixture()], "1.4.1", &offer("1.4.2"));
        let apps = app_flaws(
            &[fixture()],
            &[app("org.dukovo.ripgrep", "14.0.0", None, None)],
            &HashMap::new(),
        );
        let said = check_lines(&found(system.clone(), apps.clone()));
        assert_eq!(said.len(), 2);
        assert!(said[0].starts_with("DKSA-2026-0001 (high) affects this system: Example:"));
        assert!(
            said[0].ends_with("Dukovo 1.4.2 has the fix and is offered: `update apply` stages it")
        );
        assert_eq!(said[1], "1 flaw is known in the apps: `update advisories` lists it");

        let two = [apps[0].clone(), apps[0].clone()].to_vec();
        assert_eq!(
            check_lines(&found(Vec::new(), two)),
            ["2 flaws are known in the apps: `update advisories` lists them"]
        );
        assert_eq!(
            check_lines(&found(Vec::new(), Vec::new())),
            ["no advisory affects this system, and no flaw is known in its apps"]
        );
        let mut down = Advisories::default();
        down.unchecked.push(Unchecked::new("system", "down"));
        assert_eq!(check_lines(&down), ["down"]);
    }

    #[test]
    fn updated_tells_of_a_critical_or_high_advisory_whose_fix_is_not_staged() {
        let high = system_flaws(&[fixture()], "1.4.1", &Standing::default());
        assert_eq!(
            notice(&high).unwrap(),
            "A high security advisory affects this system: `update advisories` says what to do"
        );
        let both = [
            advisory("DKSA-2026-0002", "critical", "dukovo", "0", "9.0.0"),
            fixture(),
            advisory("DKSA-2026-0003", "medium", "dukovo", "0", "9.0.0"),
        ];
        let flaws = system_flaws(&both, "1.4.1", &Standing::default());
        assert_eq!(
            notice(&flaws).unwrap(),
            "2 security advisories, the worst critical, affect this system: `update advisories` \
             says what to do"
        );
        // A fix staged is told already; a medium or low one is not told.
        let staged = Standing { staged: Some("9.0.0".into()), offer: Offer::Nothing };
        assert_eq!(notice(&system_flaws(&both, "1.4.1", &staged)), None);
        let minor = system_flaws(
            &[advisory("DKSA-2026-0003", "medium", "dukovo", "0", "9.0.0")],
            "1.4.1",
            &Standing::default(),
        );
        assert_eq!(notice(&minor), None);
        assert_eq!(notice(&[]), None);
    }

    /// An update server at `dir/server` that signs the example advisory
    /// and a second, and the root it starts from.
    fn server_of(dir: &Path) -> Vec<u8> {
        use system_release::repo::Keys;
        let now = tuf::time::now();
        let keys_dir = dir.join("keys");
        Keys::create(&keys_dir, now).unwrap();
        let (online, secret) = tuf::repo::Ed25519::generate().unwrap();
        let keys = Keys::set_online(&keys_dir, tuf::repo::Signer::key(&online), now).unwrap();
        let source = dir.join("advisories");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("DKSA-2026-0001.json"), FIXTURE).unwrap();
        let second =
            FIXTURE.replace("DKSA-2026-0001", "DKSA-2026-0002").replace("\"high\"", "\"critical\"");
        std::fs::write(source.join("DKSA-2026-0002.json"), second).unwrap();
        let sources = system_release::advisory::load_dir(&source).unwrap();
        let maintainer = dir.join("maintainer");
        system_release::updates::set_advisories(&maintainer, &keys, &sources, now).unwrap();
        let server = dir.join("server");
        system_release::updates::publish(&server, &maintainer, &secret, None, now).unwrap();
        keys.roots()[0].clone()
    }

    #[test]
    fn a_client_reads_the_signed_index_and_each_advisory_through_tuf() {
        let dir = std::env::temp_dir().join(format!("update-advisories-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = server_of(&dir);
        let state = dir.join("state");
        tuf::init(&state.join("metadata"), &root).unwrap();
        let settings =
            Settings { server: format!("file://{}/server/", dir.display()), ..Settings::default() };
        let mut client = online::Client::open_in(settings, &state).unwrap();
        client.refresh().unwrap();

        let index = client.advisories_index().unwrap().unwrap();
        let ids: Vec<&str> = index.advisories.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["DKSA-2026-0001", "DKSA-2026-0002"]);
        assert_eq!(index.advisories[1].severity, advisory::Severity::Critical);
        let first = client.advisory("DKSA-2026-0001").unwrap().unwrap();
        assert_eq!(first.summary, fixture().summary);
        // One the server does not list is none, not a failure.
        assert!(client.advisory("DKSA-2026-0003").unwrap().is_none());

        // Fetched whole, they match what is installed; the worst comes first.
        let advisories = fetch(&mut client).unwrap();
        assert_eq!(advisories.len(), 2);
        let flaws = system_flaws(&advisories, "1.4.1", &Standing::default());
        let said: Vec<(&str, &str)> =
            flaws.iter().map(|flaw| (flaw.id.as_str(), flaw.severity.word())).collect();
        assert_eq!(said, [("DKSA-2026-0002", "critical"), ("DKSA-2026-0001", "high")]);

        // Read again from what the client kept, with the server gone.
        std::fs::remove_dir_all(dir.join("server/targets/advisories")).unwrap();
        assert!(client.advisory("DKSA-2026-0001").unwrap().is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plan_is_an_offer_of_the_release_it_names_or_nothing() {
        let release = system_release::list::Release {
            format: 1,
            version: "1.4.2".into(),
            commit: "0123456789abcdef".into(),
            toolchain: "1.95.0".into(),
            built: "2026-11-06T00:00:00Z".into(),
            updates_from: "1.4.0".into(),
            files: Vec::new(),
        };
        let offered = || "1.4.2".to_string();
        assert_eq!(
            Offer::of(&Plan::Update { offered: offered(), from: "1.4.0".into(), release }),
            Offer::Update(offered())
        );
        assert_eq!(
            Offer::of(&Plan::Waiting { offered: offered(), day: 3, held: None }),
            Offer::Waiting(offered())
        );
        assert_eq!(
            Offer::of(&Plan::Held { offered: offered(), held: "1.4.1".into() }),
            Offer::Held(offered())
        );
        assert_eq!(Offer::of(&Plan::Current { offered: offered() }), Offer::Nothing);
        assert_eq!(
            Offer::of(&Plan::Withdrawn { offered: offered(), reason: "it loses files".into() }),
            Offer::Nothing
        );
    }

    #[test]
    fn the_document_says_what_applies_and_what_could_not_be_checked() {
        let system = system_flaws(&[fixture()], "1.4.1", &offer("1.4.2"));
        let lists = HashMap::from([(
            "dukovo".to_string(),
            list(
                &[("org.example.notes", "1.0.0"), ("org.example.notes", "1.1.0")],
                vec![flaw(
                    "org.example.notes",
                    "1.0.0",
                    "RUSTSEC-2026-0001",
                    &[],
                    Severity::Medium,
                )],
            ),
        )]);
        let apps = app_flaws(
            &[fixture()],
            &[
                app("org.dukovo.ripgrep", "14.0.0", None, Some("14.1.1")),
                app("org.example.notes", "1.0.0", Some("dukovo"), Some("1.1.0")),
            ],
            &lists,
        );
        let mut all = found(system, apps);
        all.unchecked.push(Unchecked::new("apps", "bundled does not answer"));
        let line = crate::json::line(&all).unwrap();
        crate::json::assert_golden(&line, include_str!("../../tests/json/update-advisories.json"));
    }
}
