// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! update(8) and updated(8): one command and one service for every
//! update (ADR 0502): the system, its firmware set and the apps, which
//! sysupdate(8) and bundle(1) keep doing underneath.
//!
//! - `update` (or `update check`) says what waits: the system update the
//!   channel offers, the firmware set it offers, and the apps' updates.
//! - `update apply` stages the system update, or the firmware set when
//!   no system update waits, into the spare slot, to start at the next
//!   restart the owner picks, and updates the apps. The stage is the
//!   `sysupdate-stage` service ([`crate::sysupdate::request`]), which
//!   holds the root's system subvolumes and no home folder.
//! - `update undo system|firmware|ID` goes back on one thing: the system
//!   or the firmware set by making the previous slot the default, an app
//!   as `bundle rollback` does.
//! - `update hold system|firmware|ID` holds one back at the version
//!   offered now, until `update unhold` or a newer version; an app as
//!   `bundle hold` does.
//! - `update history` lists each change of the system and firmware by
//!   the start that first ran it, then the apps' history.
//! - `update advisories` says which security advisories of the update
//!   server (DKSA, ADR 0534) apply to the release this system runs and to
//!   its apps, and how the fix stands; `update check` has a Security part
//!   that says the same in brief ([`advisories`]).
//! - `update`, `update advisories` and `update history` take `--json`,
//!   which says the same as one document each (ADR 0541, [`documents`]).
//! - `update last` says the last check that ran as root, which updated
//!   and `elevate update check --json` keep ([`last`]).
//! - `update settings` says how this system updates, and `update set`
//!   changes it ([`settings`]); `update checks` says what a new version
//!   must do to prove itself, and `update checks add`, `remove` and
//!   `essential` change that ([`checks`]).
//! - `update apply --only system|firmware|apps|ID` does one part alone.
//!
//! updated(8), a service, checks once a day, ten minutes after the
//! machine starts, and does what updates.toml(5) says: by default it
//! tells whoever is signed in. It never restarts the machine, unless it
//! is unattended and has a restart time. A staged release the release
//! keys withdraw is unstaged at the next check; just before an
//! unattended restart into a staged release, updated asks the server
//! once more, and waits while its waves are held (ADRs 0456, 0469). A
//! canary checks every hour, and after its first check starts the job
//! that reports its state to the update server, so that a restart into a
//! new release is reported soon (ADR 0533).
//! Once a week it reads the running slot back against its signed list,
//! as `sysupdate verify` does. When a check finds that an advisory of
//! critical or high severity applies to the system, it tells whoever is
//! signed in, in one sentence.
//!
//! On a metered connection, which netd(8) says, a check still fetches
//! the channel's small signed files and tells of what it finds, but
//! downloads and stages nothing until a check finds the connection not
//! metered, unless updates.toml(5) says `metered = "download"`.

mod advisories;
mod checks;
mod documents;
mod held;
mod last;
mod settings;

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::alerts::client::Screen;
use crate::alerts::protocol::{Alert, Severity};
use crate::args::{Arg, Args};
use crate::notifyd::{self, Urgency};
use crate::sysupdate::checked::Found;
use crate::sysupdate::online::{self, Apps, Check, Metered, Plan, Settings};
use crate::sysupdate::starts::{self, Outcome};
use crate::sysupdate::{self, firmware, place, request, stage};
use crate::util::{self, Fail, fail};
use crate::{bundles, power};

const USAGE: &str = "usage: update [check [--json] | apply [--only PART] [--yes] | undo ITEM \
                     [--settings-from-before | --keep-settings] | hold ITEM | unhold ITEM | \
                     history [--json] | advisories [--json] | last [--json] | settings [--json] | \
                     set [--check WORD] [--apps WORD] [--unattended on|off] [--restart HH:MM | \
                     --no-restart] [--metered WORD] | checks [--json] | checks add NAME \
                     (--url URL [--insecure] | --service NAME) [--required] [--timeout S] | \
                     checks remove NAME | checks essential NAME on|off], where ITEM is system, \
                     firmware or an app's ID, and PART is system, firmware, apps or an app's ID";

/// How long updated waits after the machine starts before its first
/// check: the network is up by then.
const FIRST: Duration = Duration::from_mins(10);

/// How long it waits between checks.
const EVERY: Duration = Duration::from_hours(24);

/// How long a canary waits between checks: it takes each release as its
/// channel offers it, and the project learns of a bad one from it first
/// (ADR 0533).
const EVERY_CANARY: Duration = Duration::from_hours(1);

/// How long `settings` have updated wait between checks.
fn every(settings: &Settings) -> Duration {
    if settings.canary { EVERY_CANARY } else { EVERY }
}

/// What netd(8) says of the connection: `yes` on the first line while the
/// one that carries the default route is metered.
const METERED: &str = "/system/live/netd/metered";

/// update(8).
pub fn update(args: &mut Args) -> Result<i32, Fail> {
    let action = args.subcommand().map(|a| a.to_string_lossy().into_owned());
    // These read their own options.
    match action.as_deref() {
        Some("last") => return last::last(args),
        Some("settings") => return settings::settings(args),
        Some("set") => return settings::set(args),
        Some("checks") => return checks::checks(args),
        _ => {}
    }
    let mut yes = false;
    let mut json = false;
    let mut item = None;
    let mut only = None;
    let mut keep = sysupdate::Keep::Unsaid;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name)
                if name == "json"
                    && matches!(
                        action.as_deref(),
                        None | Some("check" | "history" | "advisories")
                    ) =>
            {
                json = true;
            }
            Arg::Long(name) if name == "yes" && action.as_deref() == Some("apply") => yes = true,
            Arg::Long(name)
                if name == "only" && action.as_deref() == Some("apply") && only.is_none() =>
            {
                only = Some(args.text()?);
            }
            Arg::Long(name)
                if name == "settings-from-before" && action.as_deref() == Some("undo") =>
            {
                keep = sysupdate::Keep::FromBefore;
            }
            Arg::Long(name) if name == "keep-settings" && action.as_deref() == Some("undo") => {
                keep = sysupdate::Keep::Today;
            }
            Arg::Operand(operand)
                if matches!(action.as_deref(), Some("undo" | "hold" | "unhold"))
                    && item.is_none() =>
            {
                item = Some(operand.to_string_lossy().into_owned());
            }
            _ => return Err(util::usage(USAGE.into())),
        }
    }
    let needs_item = || item.clone().ok_or_else(|| util::usage(USAGE.into()));
    match action.as_deref() {
        None | Some("check") => check(json),
        Some("apply") => apply(yes, only.as_deref()),
        Some("undo") => undo(&needs_item()?, keep),
        Some("hold") => hold(&needs_item()?, true),
        Some("unhold") => hold(&needs_item()?, false),
        Some("history") => history(json),
        Some("advisories") => advise(json),
        Some(_) => Err(util::usage(USAGE.into())),
    }
}

/// Runs bundle(1) with `words`, in this process.
fn bundle(words: &[&str]) -> Result<i32, Fail> {
    let items = words.iter().map(OsString::from).collect();
    bundles::main(&mut Args::new("bundle", items))
}

/// `update check`: the system, the firmware set and the apps; with
/// `json`, as the document update(8) describes.
fn check(json: bool) -> Result<i32, Fail> {
    if json {
        let line = crate::json::line(&found())?;
        println!("{line}");
        last::keep(last::By::UpdateCheck, &line);
        return Ok(0);
    }
    println!("System:");
    if let Err(why) = sysupdate::check(None, false) {
        println!("update: the system cannot be checked: {}", why.message);
    }
    if let Some(note) = withdrawn_running().ok().flatten() {
        println!("update: {}", note.0);
    }
    println!("\nFirmware:");
    match firmware_offer() {
        Ok(offer) => println!("update: {}", offer.said),
        Err(why) => println!("update: the firmware cannot be checked: {}", why.message),
    }
    println!("\nApps:");
    if let Err(why) = bundle(&["update", "--check"]) {
        println!("update: the apps cannot be checked: {}", why.message);
    }
    println!("\nSecurity:");
    for line in advisories::check_lines(&advisories::gather(true)) {
        println!("update: {line}");
    }
    Ok(0)
}

/// `update advisories`: the advisories that apply to this system and the
/// flaws of its apps; with `json`, as the document update(8) describes.
/// The exit status is 0 when nothing applies, 1 when something does, and 3
/// when nothing was found but a part could not be checked.
fn advise(json: bool) -> Result<i32, Fail> {
    let found = advisories::gather(true);
    if json {
        crate::json::print(&found)?;
    } else {
        advisories::print(&found);
    }
    Ok(advisories::status(&found))
}

/// What the channel offers of firmware, against the running slot's set.
struct FirmwareOffer {
    /// The set to stage, when it is newer and not held back.
    take: Option<String>,
    /// What to say.
    said: String,
    /// The set the running slot names.
    running: Option<String>,
    /// The set the channel offers.
    offered: Option<String>,
    /// What the offer comes to: `Current` when it offers none or none
    /// newer, `Waiting` for its wave, `Held`, or `Update`.
    found: Found,
    /// The version the hold names, when the offer is held.
    held: Option<String>,
}

/// The firmware set the channel offers, and whether to take it.
fn firmware_offer() -> Result<FirmwareOffer, Fail> {
    let root = place::root()?;
    let running = firmware::named(&root.slot(root.running)).map(|named| named.set);
    let settings = Settings::load()?;
    let hold = settings.hold.firmware.clone();
    let mut client = online::Client::connect(settings)?;
    let Some(offered) = client.channel()?.firmware else {
        let said = match &running {
            Some(set) => format!("this system runs firmware {set}; its channel offers none"),
            None => "the channel offers no firmware set, and this system runs none".into(),
        };
        return Ok(FirmwareOffer {
            take: None,
            said,
            running,
            offered: None,
            found: Found::Current,
            held: None,
        });
    };
    let newer = running.as_ref().is_none_or(|set| {
        system_release::firmware::compare(&offered, set) == std::cmp::Ordering::Greater
    });
    let held = hold.as_ref().is_some_and(|held| {
        system_release::firmware::compare(&offered, held) != std::cmp::Ordering::Greater
    });
    let now =
        running.as_ref().map_or_else(|| "no firmware set".into(), |set| format!("firmware {set}"));
    // A set held back waits whatever its wave; one not held waits for its
    // wave (ADR 0544).
    let waiting =
        if newer && !held { online::firmware_waiting(&mut client, &offered)? } else { None };
    let (take, said, found) = if !newer {
        (None, format!("this system runs {now}, the newest offered"), Found::Current)
    } else if let Some(lines) = waiting {
        let said = format!(
            "firmware {offered} is offered, waiting for its wave: {}; this system runs {now}",
            lines.join("; ")
        );
        (None, said, Found::Waiting)
    } else if held {
        let said = format!(
            "firmware {offered} is offered, and held back: `update unhold firmware` lifts the \
             hold; this system runs {now}"
        );
        (None, said, Found::Held)
    } else {
        let said = format!(
            "firmware {offered} is offered; this system runs {now}: `update apply` stages it"
        );
        (Some(offered.clone()), said, Found::Update)
    };
    let held = if found == Found::Held { hold } else { None };
    Ok(FirmwareOffer { take, said, running, offered: Some(offered), found, held })
}

/// The system update and the firmware set the channel offers, and the
/// apps' updates, for `update --json`: a part that cannot be checked is
/// named, as the text form says it, and the others still are.
fn found() -> documents::Updates {
    let mut updates = documents::Updates {
        offers: Vec::new(),
        running_withdrawn: None,
        unchecked: Vec::new(),
        notes: Vec::new(),
        security: documents::Security { system: Vec::new(), app_flaws: 0 },
    };
    let held = held::Held::read();
    match system_offer(&mut updates.notes, held.as_ref()) {
        Ok(offer) => updates.offers.push(offer),
        Err(why) => updates.unchecked.push(documents::Unchecked::new("system", &why.message)),
    }
    updates.running_withdrawn = running_withdrawal().ok().flatten().map(|(version, withdrawal)| {
        documents::RunningWithdrawn::new(&version, &withdrawal.reason, withdrawal.advice)
    });
    match firmware_offer() {
        Ok(offer) => updates.offers.push(documents::Offer::firmware(&offer, held.as_ref())),
        Err(why) => updates.unchecked.push(documents::Unchecked::new("firmware", &why.message)),
    }
    match bundles::pending_updates() {
        Ok(pending) => {
            updates.offers.extend(pending.rows.iter().map(documents::Offer::app));
            updates.notes.extend(pending.notes);
        }
        Err(why) => updates.unchecked.push(documents::Unchecked::new("apps", &why.message)),
    }
    let security = advisories::gather(true);
    updates.security =
        documents::Security { system: security.system, app_flaws: security.apps.len() };
    updates.unchecked.extend(
        security.unchecked.iter().map(|part| documents::Unchecked::new("security", &part.why)),
    );
    updates
}

/// What the channel offers of the system, as `sysupdate check` finds it:
/// the troubleshoot signatures it reads along are kept, and a note on
/// them goes to `notes`.
fn system_offer(
    notes: &mut Vec<String>,
    held: Option<&held::Held>,
) -> Result<documents::Offer, Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let mut client = online::Client::connect(Settings::load()?)?;
    notes.extend(online::keep_signatures(&mut client));
    let plan = online::plan(&mut client, &running, None)?;
    let download = match &plan {
        Plan::Update { release, .. } => {
            let needed = online::needed(&root.slot(root.running), release)?;
            Some(documents::Download { files: needed.fetch.len() as u64, bytes: needed.bytes })
        }
        _ => None,
    };
    Ok(documents::Offer::system(&plan, &running, download, held))
}

/// The release this system runs and its withdrawal, if the release keys
/// signed one.
fn running_withdrawal() -> Result<Option<(String, system_release::rollout::Withdrawal)>, Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let mut client = online::Client::connect(Settings::load()?)?;
    Ok(client.withdrawal(&running)?.map(|withdrawal| (running, withdrawal)))
}

/// When the release this system runs was withdrawn: what to say, and
/// whether its release keys advise going back.
fn withdrawn_running() -> Result<Option<(String, bool)>, Fail> {
    let Some((running, withdrawal)) = running_withdrawal()? else { return Ok(None) };
    let back = withdrawal.advice == system_release::rollout::Advice::Rollback;
    let advice = if back {
        "`update undo system` goes back to the version before it"
    } else {
        "stay on it until the next release"
    };
    Ok(Some((
        format!(
            "Dukovo {running}, which this system runs, was withdrawn: {}; {advice}",
            withdrawal.reason
        ),
        back,
    )))
}

/// What `update apply` does: all of it, or the part `--only` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Only<'a> {
    /// The system or the firmware set, and the apps.
    All,
    /// The system update alone.
    System,
    /// The firmware set alone.
    Firmware,
    /// Every app.
    Apps,
    /// One app, by its ID.
    App(&'a str),
}

impl<'a> Only<'a> {
    /// The part `word` names.
    fn of(word: Option<&'a str>) -> Only<'a> {
        match word {
            None => Only::All,
            Some("system") => Only::System,
            Some("firmware") => Only::Firmware,
            Some("apps") => Only::Apps,
            Some(id) => Only::App(id),
        }
    }
}

/// `update apply`: stages the system update, or the firmware set, and
/// updates the apps; or, with `only`, one part of that. Nothing restarts.
fn apply(yes: bool, only: Option<&str>) -> Result<i32, Fail> {
    let only = Only::of(only);
    let part = match only {
        Only::System => Part::System,
        Only::Firmware => Part::Firmware,
        Only::All => Part::Either,
        Only::Apps | Only::App(_) => return apps_only(yes, only),
    };
    match &stage_offered(part)? {
        Some(Staged { what, said }) => {
            for line in said {
                println!("{line}");
            }
            println!("update: {what} is staged: it starts at the next restart you choose");
        }
        None => println!(
            "update: no {} update waits",
            match part {
                Part::System => "system",
                Part::Firmware => "firmware",
                Part::Either => "system or firmware",
            }
        ),
    }
    if only != Only::All {
        return Ok(0);
    }
    let mut words = vec!["update", "--for-everyone"];
    if yes {
        words.push("--yes");
    }
    if let Err(why) = bundle(&words) {
        println!("update: the apps were not updated: {}", why.message);
    }
    Ok(0)
}

/// `update apply --only apps` or `--only ID`: the apps alone. Unlike the
/// apps part of `update apply`, what goes wrong is the answer, since it
/// is all that was asked.
fn apps_only(yes: bool, only: Only) -> Result<i32, Fail> {
    let mut words = vec!["update"];
    if let Only::App(id) = only {
        if id.starts_with('-') {
            return Err(util::usage(format!(
                "--only takes system, firmware, apps or an app's ID, not '{id}'"
            )));
        }
        words.push(id);
    }
    words.push("--for-everyone");
    if yes {
        words.push("--yes");
    }
    bundle(&words)
}

/// Which of the two parts a stage takes: they share the spare slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The system update, and the firmware set when none waits.
    Either,
    /// The system update alone.
    System,
    /// The firmware set alone.
    Firmware,
}

/// What a stage staged.
struct Staged {
    /// Which release or firmware set: `Dukovo 0.16.0`.
    what: String,
    /// What the stage printed as it went.
    said: Vec<String>,
}

/// Stages the system update the channel offers, or else the firmware set
/// it offers, as the `sysupdate-stage` service, which holds the root's
/// system subvolumes and no home folder (ADR 0473); what was staged, if
/// anything. The two share the spare slot, so the firmware waits for a
/// system update to prove itself.
fn stage_offered(part: Part) -> Result<Option<Staged>, Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let mut client = online::Client::connect(Settings::load()?)?;
    if part != Part::Firmware
        && let Plan::Update { release, .. } = online::plan(&mut client, &running, None)?
    {
        // A bridge release, when the one offered updates only from it.
        let staged = release.version;
        let said = request::ask(&request::What::System)?;
        if part == Part::Either && firmware_offer()?.take.is_some() {
            println!("update: the firmware set waits until Dukovo {staged} has proved itself");
        }
        return Ok(Some(Staged { what: format!("Dukovo {staged}"), said }));
    }
    if part == Part::System {
        return Ok(None);
    }
    if let Some(set) = firmware_offer()?.take {
        let said = request::ask(&request::What::Firmware(set.clone()))?;
        return Ok(Some(Staged { what: format!("firmware {set}"), said }));
    }
    Ok(None)
}

/// `update undo ITEM`.
fn undo(item: &str, keep: sysupdate::Keep) -> Result<i32, Fail> {
    if item != "system" && item != "firmware" {
        return bundle(&["rollback", item]);
    }
    let root = place::root()?;
    let (_, state) = place::state(&root)?;
    let previous = state
        .previous
        .filter(|&slot| slot != root.running)
        .ok_or_else(|| fail("there is no previous version to go back to".into()))?;
    let version = |slot| root.about(slot).map(|about| about.version);
    let set = |slot: slots::Slot| firmware::named(&root.slot(slot)).map(|named| named.set);
    let same_system = version(previous) == version(root.running);
    let same_set = set(previous) == set(root.running);
    match (item, same_system, same_set) {
        ("firmware", false, _) => {
            return Err(fail(format!(
                "the previous version is also another system, {}: `update undo system` goes back \
                 to both",
                place::named(&root, previous)
            )));
        }
        ("firmware", true, true) => {
            return Err(fail("the previous version runs the same firmware set".into()));
        }
        ("system", _, false) => println!(
            "update: the previous version names another firmware set, which comes back with it"
        ),
        _ => {}
    }
    sysupdate::rollback(keep)
}

/// `update hold ITEM` (`on`) or `update unhold ITEM`.
fn hold(item: &str, on: bool) -> Result<i32, Fail> {
    if item != "system" && item != "firmware" {
        // The hold of an administrator, which the console's is, is of the
        // app everyone has; another person's is of their own.
        let mut words = vec![if on { "hold" } else { "unhold" }, item];
        if last::is_root() {
            words.push("--for-everyone");
        }
        return bundle(&words);
    }
    let mut settings = Settings::load()?;
    let version = if !on {
        None
    } else if item == "system" {
        let root = place::root()?;
        let running = stage::running_version(&root)?;
        let mut client = online::Client::connect(settings.clone())?;
        let offered = client.channel()?.version;
        let newer = slots::version::Version::parse(&offered)
            .zip(slots::version::Version::parse(&running))
            .is_some_and(|(offered, running)| offered > running);
        Some(if newer { offered } else { running })
    } else {
        let offer = firmware_offer()?;
        let root = place::root()?;
        let running = firmware::named(&root.slot(root.running)).map(|named| named.set);
        Some(offer.take.or(running).ok_or_else(|| fail("no firmware set to hold".into()))?)
    };
    let slot =
        if item == "system" { &mut settings.hold.system } else { &mut settings.hold.firmware };
    slot.clone_from(&version);
    settings.save()?;
    if !on && item == "system" {
        // The record of a release the health check held goes with the hold.
        held::forget();
    }
    match version {
        Some(version) => println!(
            "update: {item} is held back at {version}: a newer version is taken as usual; until \
             then the held version gets no security fixes. `update unhold {item}` lifts the hold"
        ),
        None => println!("update: {item} is no longer held back"),
    }
    Ok(0)
}

/// `update history`: each change of the system and the firmware set, by
/// the start that first ran it, then the apps'; with `json`, as the
/// document update(8) describes.
fn history(json: bool) -> Result<i32, Fail> {
    let records = starts::all();
    let changes = documents::changes(&records);
    if json {
        let apps = bundles::history_events(None)?.map_err(fail)?;
        let history = documents::History {
            changes,
            starts_kept: records.len(),
            apps: apps.iter().map(documents::AppEvent::of).collect(),
        };
        return crate::json::print(&history).map(|()| 0);
    }
    println!("System and firmware, by the start that first ran each:");
    for change in &changes {
        let what = if change.item == "system" { "Dukovo" } else { "firmware" };
        let outcome = match change.outcome {
            Outcome::Good => "good".to_string(),
            Outcome::Unfinished => "unfinished".to_string(),
            Outcome::Failed => format!("failed ({})", change.error.as_deref().unwrap_or("?")),
        };
        println!(
            "  start {} on {}: {what} {} to {}; {outcome}",
            change.start,
            util::local_time(change.time, "%Y-%m-%d %H:%M"),
            change.from.as_deref().unwrap_or("none"),
            change.to.as_deref().unwrap_or("none")
        );
    }
    if changes.is_empty() {
        println!("  no change in the {} starts kept", records.len());
    }
    println!("\nApps:");
    bundle(&["history"])
}

/// updated(8): checks once a day, or once with `--once`, and does what
/// updates.toml(5) says.
pub fn updated(args: &mut Args) -> Result<i32, Fail> {
    let mut once = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "once" => once = true,
            other => return Err(args.unknown(&other)),
        }
    }
    if !once {
        std::thread::sleep(FIRST);
    }
    let mut first = true;
    loop {
        let settings = Settings::load()?;
        // The weekly read of the slot is the service's, not a single
        // check's.
        if !once {
            weekly_verify();
        }
        let staged = if settings.check == Check::Off {
            println!("updated: checks are off (updates.toml)");
            false
        } else {
            // An update staged before, whose restart waited for its
            // waves, still waits for one.
            round(&settings) || staged_before()
        };
        if once {
            return Ok(0);
        }
        // A canary reports once the machine has started and checked, so
        // that a restart into a new release is reported soon, not at the
        // job's next hour.
        if std::mem::take(&mut first) {
            report_canary(&settings);
        }
        match (&settings.restart, staged) {
            (Some(restart), true) if settings.unattended => {
                let wait = until(restart);
                println!("updated: restarting into the update at {restart}");
                std::thread::sleep(wait);
                // A pause or a withdrawal since the stage stops the
                // restart.
                match staged_since(&settings) {
                    Some(Since::Withdrawn { version, reason }) => {
                        unstage_withdrawn(&version, &reason);
                    }
                    Some(Since::Held { version, why }) => tell(&format!(
                        "the waves of Dukovo {version} are held: {why}; the restart into it waits"
                    )),
                    None => {
                        if let Err(why) = power::request("reboot") {
                            println!("updated: cannot ask Spark to restart: {}", why.message);
                        }
                    }
                }
            }
            _ => std::thread::sleep(every(&settings)),
        }
    }
}

/// Has the job that reports a canary's state do so now, when `settings`
/// name this machine a canary that reports; the job runs on its own each
/// hour as well (ADR 0533). It is not waited for: what it says is in its
/// log.
fn report_canary(settings: &Settings) {
    if !settings.canary || settings.canary_name.is_none() {
        return;
    }
    let started = crate::service::change(
        "start",
        sysupdate::canary::SERVICE,
        &["--no-wait".to_string(), "--quiet".to_string()],
    );
    if let Err(why) = started {
        println!("updated: cannot start {}: {}", sysupdate::canary::SERVICE, why.message);
    }
}

/// How long until the next `HH:MM`, local time.
fn until(time: &str) -> Duration {
    let wanted = online::minutes_of_day(time).unwrap_or(0);
    let now = i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(0);
    let local = online::minutes_of_day(&util::local_time(now, "%H:%M")).unwrap_or(0);
    Duration::from_secs(u64::from((wanted + 24 * 60 - local) % (24 * 60)) * 60)
}

/// What became, since its stage, of a system release staged in the
/// spare slot (ADRs 0456, 0469).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Since {
    /// The release keys withdrew it.
    Withdrawn { version: String, reason: String },
    /// Its waves are held: a pause, or a canary that is not green.
    Held { version: String, why: String },
}

/// What the server says now of the system release staged in the spare
/// slot: `None` when none is, when the spare slot holds the running
/// system with another firmware set, or when the server cannot be
/// asked, which leaves the staged release as it is.
fn staged_since(settings: &Settings) -> Option<Since> {
    let root = place::root().ok()?;
    let (_, state) = place::state(&root).ok()?;
    let slot = state.trying?;
    let version = root.about(slot)?.version;
    if root.about(root.running).is_some_and(|about| about.version == version) {
        return None;
    }
    let mut client = online::Client::connect(settings.clone()).ok()?;
    let withdrawal = client.withdrawal(&version).ok()?;
    let rollout = client.rollout(&version).ok()?;
    since(version, withdrawal, rollout)
}

/// [`Since`], from the release's withdrawal and its rollout, if the
/// server signs them.
fn since(
    version: String,
    withdrawal: Option<system_release::rollout::Withdrawal>,
    rollout: Option<system_release::rollout::Rollout>,
) -> Option<Since> {
    if let Some(withdrawal) = withdrawal {
        return Some(Since::Withdrawn { version, reason: withdrawal.reason });
    }
    rollout?.held.map(|why| Since::Held { version, why })
}

/// Whether an update staged before waits to be tried.
fn staged_before() -> bool {
    place::root()
        .and_then(|root| place::state(&root))
        .is_ok_and(|(_, state)| state.trying.is_some())
}

/// Unstages the withdrawn release `version`, and says so.
fn unstage_withdrawn(version: &str, reason: &str) {
    match sysupdate::unstage() {
        Ok(_) => tell(&format!(
            "Dukovo {version}, which was staged, was withdrawn: {reason}; it is unstaged and will \
             not be tried"
        )),
        Err(why) => tell(&format!(
            "Dukovo {version}, which is staged, was withdrawn: {reason}; it cannot be unstaged: {}; \
             `elevate sysupdate unstage` does it",
            why.message
        )),
    }
}

/// Once a week, reads the running slot back against its signed list
/// (`sysupdate verify`), and tells whoever is signed in when it is not
/// the release.
fn weekly_verify() {
    let now = i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(0);
    if !sysupdate::verify::due(now) {
        return;
    }
    if let Err(why) = sysupdate::verify::main(None) {
        let text = format!(
            "the system's files are not the release's: {}; `elevate sysupdate verify` checks \
             again, and Rescue reinstalls the release",
            why.message
        );
        println!("updated: {text}");
        if let Err(why) = notifyd::send("updated", &text, "", Urgency::Critical) {
            println!("updated: cannot tell the consoles: {}", why.message);
        }
        let detail = format!(
            "{}\n`elevate sysupdate verify` checks again, and Rescue reinstalls the release.",
            why.message
        );
        tell_alertd(&Alert::new(
            "update-not-verified".into(),
            Severity::Critical,
            "The system's files are not the release's",
            &detail,
        ));
    }
}

/// Tells alertd of `alert`, an event that updated has told the consoles
/// of in its own words already; what keeps it from alertd goes in the
/// log.
fn tell_alertd(alert: &Alert) {
    if let Err(why) = crate::alerts::client::event(alert, Screen::Shown) {
        println!("updated: cannot tell alertd: {why}");
    }
}

/// Keeps the troubleshoot signatures the server offers, when they are
/// newer than those in use (ADR 0474).
fn signatures_round(settings: &Settings) {
    let Ok(mut client) = online::Client::connect(settings.clone()) else { return };
    if let Some(said) = online::keep_signatures(&mut client) {
        println!("updated: {said}");
    }
}

/// One check, as `settings` say; whether an update was staged.
fn round(settings: &Settings) -> bool {
    let metered = connection_is_metered();
    if waits(settings, metered) {
        println!("updated: the connection is metered: nothing is downloaded until it is not");
    } else {
        signatures_round(settings);
    }
    if let Some(Since::Withdrawn { version, reason }) = staged_since(settings) {
        unstage_withdrawn(&version, &reason);
    }
    let mut staged = false;
    if let Ok(Some((said, back))) = withdrawn_running() {
        if back && settings.unattended {
            match sysupdate::rollback(sysupdate::Keep::Today) {
                Ok(_) => tell(&format!("{said}; going back to it, as this machine is unattended")),
                Err(why) => tell(&format!("{said}; cannot go back: {}", why.message)),
            }
            staged = true;
        } else {
            tell(&said);
        }
    }
    if !staged {
        staged = system_round(settings, metered);
    }
    security_round();
    apps_round(settings, metered);
    keep_last();
    staged
}

/// Keeps what a check finds now as the last one, for `update last`.
fn keep_last() {
    match crate::json::line(&found()) {
        Ok(line) => last::keep(last::By::Updated, &line),
        Err(why) => println!("updated: cannot keep the check: {}", why.message),
    }
}

/// Whether the connection is metered now, as netd says; not when it says
/// nothing.
fn connection_is_metered() -> bool {
    std::fs::read_to_string(METERED).is_ok_and(|text| says_metered(&text))
}

/// Whether netd's metered file `text` says the connection is.
fn says_metered(text: &str) -> bool {
    text.lines().next().is_some_and(|line| line.trim() == "yes")
}

/// Whether a check holds its downloads back: the connection is metered
/// and the settings do not take it as any other.
fn waits(settings: &Settings, metered: bool) -> bool {
    metered && settings.metered == Metered::Wait
}

/// Whether a check updates the apps itself, instead of telling of their
/// updates.
fn takes_apps(settings: &Settings, metered: bool) -> bool {
    settings.apps == Apps::Auto && !waits(settings, metered)
}

/// What a check does with the system or firmware update it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// Stages it, as an unattended machine does.
    Stage,
    /// Fetches its files, then tells.
    Download,
    /// Tells of it.
    Tell,
    /// Tells of it and says it waits for another connection.
    Wait,
}

/// What a check does with an update, for `settings` and a connection that
/// is `metered` or not. A check is not made at all with `check = "off"`.
fn choice(settings: &Settings, metered: bool) -> Choice {
    if waits(settings, metered) {
        Choice::Wait
    } else if settings.unattended {
        Choice::Stage
    } else if settings.check == Check::Download {
        Choice::Download
    } else {
        Choice::Tell
    }
}

/// What a check tells of the system update `offered`.
fn system_notice(offered: &str, choice: Choice) -> String {
    if choice == Choice::Wait {
        format!(
            "An update to Dukovo {offered} is ready to download. This connection is metered, so \
             it waits for another one, or for `elevate update apply`."
        )
    } else {
        format!("Dukovo {offered} is ready: `elevate update apply` stages it")
    }
}

/// What a check tells of the firmware set `set` on offer.
fn firmware_notice(set: &str, choice: Choice) -> String {
    if choice == Choice::Wait {
        format!(
            "Firmware {set} is ready to download. This connection is metered, so it waits for \
             another one, or for `elevate update apply`."
        )
    } else {
        format!("firmware {set} is ready: `elevate update apply` stages it")
    }
}

/// What a check tells of the apps' updates, `summary` being what
/// `bundle update` says.
fn apps_notice(summary: &str, waiting: bool) -> String {
    if waiting {
        format!(
            "App updates are ready to download: {summary}. This connection is metered, so they \
             wait for another one, or for `update apply`."
        )
    } else {
        format!("app updates are ready: {summary}; `update apply` takes them")
    }
}

/// The system and firmware part of a check.
fn system_round(settings: &Settings, metered: bool) -> bool {
    let choice = choice(settings, metered);
    if choice == Choice::Stage {
        return match stage_offered(Part::Either) {
            Ok(Some(Staged { what, .. })) => {
                tell(&format!("{what} is staged: it starts at the next restart"));
                true
            }
            Ok(None) => false,
            Err(why) => {
                tell(&format!("the update could not be staged: {}", why.message));
                tell_alertd(&Alert::new(
                    "update-not-staged".into(),
                    Severity::Warning,
                    "An update could not be staged",
                    &format!(
                        "{}\n`log updated` says more; the next check tries again.",
                        why.message
                    ),
                ));
                false
            }
        };
    }
    let waiting = place::root().and_then(|root| {
        let running = stage::running_version(&root)?;
        let mut client = online::Client::connect(settings.clone())?;
        online::plan(&mut client, &running, None)
    });
    if let Ok(Plan::Update { offered, .. }) = waiting {
        if choice == Choice::Download {
            let _ = sysupdate::check(None, true);
        }
        tell(&system_notice(&offered, choice));
    } else if let Ok(FirmwareOffer { take: Some(set), .. }) = firmware_offer() {
        tell(&firmware_notice(&set, choice));
    }
    false
}

/// The security part of a check: tells when an advisory of critical or
/// high severity applies to the system and its fix is not staged yet
/// (ADR 0534).
fn security_round() {
    if let Some(text) = advisories::notice(&advisories::gather(false).system) {
        tell(&text);
    }
}

/// The apps part of a check: updated for everyone, or told of.
fn apps_round(settings: &Settings, metered: bool) {
    let waiting = waits(settings, metered);
    let takes = takes_apps(settings, metered);
    let mut words = vec!["update", "--for-everyone"];
    words.push(if takes { "--yes" } else { "--check" });
    let ran = Command::new(Path::new("/system/commands/bundle")).args(&words).output();
    let Ok(output) = ran else { return };
    let text = String::from_utf8_lossy(&output.stdout);
    let summary = text.lines().last().unwrap_or_default().to_string();
    println!("updated: apps: {summary}");
    if !takes && text.lines().any(|line| line.starts_with("ID ")) {
        tell(&apps_notice(&summary, waiting));
    }
}

/// Logs `text` and tells whoever is signed in.
fn tell(text: &str) {
    println!("updated: {text}");
    if let Err(why) = notifyd::send("updated", text, "", Urgency::Normal) {
        println!("updated: cannot tell the consoles: {}", why.message);
    }
}

#[cfg(test)]
mod tests {
    use system_release::rollout::{Advice, Rollout, Withdrawal};

    use super::*;

    fn settings(check: Check, unattended: bool, metered: Metered, apps: Apps) -> Settings {
        Settings { check, unattended, metered, apps, ..Settings::default() }
    }

    #[test]
    fn a_canary_checks_every_hour_and_any_other_machine_once_a_day() {
        let canary = Settings { canary: true, ..Settings::default() };
        assert_eq!(every(&canary), Duration::from_hours(1));
        assert_eq!(every(&Settings::default()), Duration::from_hours(24));
        // The name alone, which `canary = true` makes a canary of, changes nothing.
        let named = Settings { canary_name: Some("ref-box".into()), ..Settings::default() };
        assert_eq!(every(&named), Duration::from_hours(24));
    }

    #[test]
    fn netd_s_metered_file_is_read_by_its_first_line() {
        assert!(says_metered("yes\nnet0 yes\n"));
        assert!(!says_metered("no\nnet0 no\nnet1 yes\n"));
        assert!(!says_metered(""));
        assert!(!says_metered("maybe\n"));
        assert!(!says_metered("\nyes\n"));
    }

    #[test]
    fn a_metered_connection_holds_back_what_is_downloaded() {
        let (wait, download) = (Metered::Wait, Metered::Download);
        for check in [Check::Notify, Check::Download] {
            for unattended in [false, true] {
                let on = |metered, setting| {
                    choice(&settings(check, unattended, setting, Apps::Notify), metered)
                };
                // Off a metered connection updated does as it always did.
                let usual = match (unattended, check) {
                    (true, _) => Choice::Stage,
                    (false, Check::Download) => Choice::Download,
                    (false, _) => Choice::Tell,
                };
                assert_eq!(on(false, wait), usual);
                assert_eq!(on(false, download), usual);
                // On one it waits, unless the settings say to download.
                assert_eq!(on(true, wait), Choice::Wait);
                assert_eq!(on(true, download), usual);
            }
        }
    }

    #[test]
    fn apps_are_not_updated_on_a_metered_connection_that_waits() {
        let takes = |apps, metered, setting| {
            takes_apps(&settings(Check::Notify, false, setting, apps), metered)
        };
        assert!(takes(Apps::Auto, false, Metered::Wait));
        assert!(!takes(Apps::Auto, true, Metered::Wait));
        assert!(takes(Apps::Auto, true, Metered::Download));
        assert!(!takes(Apps::Notify, false, Metered::Wait));
        assert!(!takes(Apps::Notify, true, Metered::Download));
    }

    #[test]
    fn what_waits_is_said_with_why() {
        let wait = system_notice("0.17.0", Choice::Wait);
        assert!(wait.starts_with("An update to Dukovo 0.17.0 is ready to download."));
        assert!(wait.contains("metered") && wait.contains("`elevate update apply`"));
        assert_eq!(
            system_notice("0.17.0", Choice::Tell),
            "Dukovo 0.17.0 is ready: `elevate update apply` stages it"
        );
        assert!(firmware_notice("2026.3", Choice::Wait).contains("metered"));
        assert!(!firmware_notice("2026.3", Choice::Download).contains("metered"));
        assert!(apps_notice("2 apps", true).contains("metered"));
        assert_eq!(
            apps_notice("2 apps", false),
            "app updates are ready: 2 apps; `update apply` takes them"
        );
    }

    #[test]
    fn a_staged_release_withdrawn_or_held_is_not_restarted_into() {
        let version = || "0.16.0".to_string();
        let rollout = |held: Option<&str>| Rollout {
            release: version(),
            wave: 2,
            held: held.map(str::to_string),
        };
        let withdrawal = Withdrawal {
            release: version(),
            reason: "it loses files".into(),
            advice: Advice::Stay,
        };
        assert_eq!(
            since(version(), Some(withdrawal), Some(rollout(Some("paused")))),
            Some(Since::Withdrawn { version: version(), reason: "it loses files".into() })
        );
        assert_eq!(
            since(version(), None, Some(rollout(Some("paused: a disk check")))),
            Some(Since::Held { version: version(), why: "paused: a disk check".into() })
        );
        assert_eq!(since(version(), None, Some(rollout(None))), None);
        assert_eq!(since(version(), None, None), None);
    }
}
