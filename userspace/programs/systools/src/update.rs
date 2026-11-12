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
//!   restart the owner picks, and updates the apps.
//! - `update undo system|firmware|ID` goes back on one thing: the system
//!   or the firmware set by making the previous slot the default, an app
//!   as `bundle rollback` does.
//! - `update hold system|firmware|ID` holds one back at the version
//!   offered now, until `update unhold` or a newer version; an app as
//!   `bundle hold` does.
//! - `update history` lists each change of the system and firmware by
//!   the start that first ran it, then the apps' history.
//!
//! updated(8), a service, checks once a day, ten minutes after the
//! machine starts, and does what updates.toml(5) says: by default it
//! tells whoever is signed in. It never restarts the machine, unless it
//! is unattended and has a restart time. A staged release the release
//! keys withdraw is unstaged at the next check; just before an
//! unattended restart into a staged release, updated asks the server
//! once more, and waits while its waves are held (ADRs 0456, 0469).
//! Once a week it reads the running slot back against its signed list,
//! as `sysupdate verify` does.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::args::{Arg, Args};
use crate::notifyd::{self, Urgency};
use crate::sysupdate::online::{self, Apps, Check, Plan, Settings};
use crate::sysupdate::starts::{self, Outcome};
use crate::sysupdate::{self, firmware, place, stage};
use crate::util::{self, Fail, fail};
use crate::{bundles, power};

const USAGE: &str = "usage: update [check | apply [--yes] | undo ITEM [--settings-from-before | \
                     --keep-settings] | hold ITEM | unhold ITEM | history], where ITEM is system, \
                     firmware or an app's ID";

/// How long updated waits after the machine starts before its first
/// check: the network is up by then.
const FIRST: Duration = Duration::from_mins(10);

/// How long it waits between checks.
const EVERY: Duration = Duration::from_hours(24);

/// update(8).
pub fn update(args: &mut Args) -> Result<i32, Fail> {
    let action = args.subcommand().map(|a| a.to_string_lossy().into_owned());
    let mut yes = false;
    let mut item = None;
    let mut keep = sysupdate::Keep::Unsaid;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "yes" && action.as_deref() == Some("apply") => yes = true,
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
        None | Some("check") => check(),
        Some("apply") => apply(yes),
        Some("undo") => undo(&needs_item()?, keep),
        Some("hold") => hold(&needs_item()?, true),
        Some("unhold") => hold(&needs_item()?, false),
        Some("history") => history(),
        Some(_) => Err(util::usage(USAGE.into())),
    }
}

/// Runs bundle(1) with `words`, in this process.
fn bundle(words: &[&str]) -> Result<i32, Fail> {
    let items = words.iter().map(OsString::from).collect();
    bundles::main(&mut Args::new("bundle", items))
}

/// `update check`: the system, the firmware set and the apps.
fn check() -> Result<i32, Fail> {
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
    Ok(0)
}

/// What the channel offers of firmware, against the running slot's set.
struct FirmwareOffer {
    /// The set to stage, when it is newer and not held back.
    take: Option<String>,
    /// What to say.
    said: String,
}

/// The firmware set the channel offers, and whether to take it.
fn firmware_offer() -> Result<FirmwareOffer, Fail> {
    let root = place::root()?;
    let running = firmware::named(&root.slot(root.running)).map(|named| named.set);
    let settings = Settings::load()?;
    let hold = settings.hold.firmware.clone();
    let mut client = online::Client::connect(settings)?;
    let Some(offered) = client.channel()?.firmware else {
        let said = match running {
            Some(set) => format!("this system runs firmware {set}; its channel offers none"),
            None => "the channel offers no firmware set, and this system runs none".into(),
        };
        return Ok(FirmwareOffer { take: None, said });
    };
    let newer = running.as_ref().is_none_or(|set| {
        system_release::firmware::compare(&offered, set) == std::cmp::Ordering::Greater
    });
    let held = hold.as_ref().is_some_and(|held| {
        system_release::firmware::compare(&offered, held) != std::cmp::Ordering::Greater
    });
    let now = running.map_or_else(|| "no firmware set".into(), |set| format!("firmware {set}"));
    Ok(if !newer {
        FirmwareOffer { take: None, said: format!("this system runs {now}, the newest offered") }
    } else if held {
        FirmwareOffer {
            take: None,
            said: format!(
                "firmware {offered} is offered, and held back: `update unhold firmware` lifts \
                 the hold; this system runs {now}"
            ),
        }
    } else {
        FirmwareOffer {
            said: format!(
                "firmware {offered} is offered; this system runs {now}: `update apply` stages it"
            ),
            take: Some(offered),
        }
    })
}

/// When the release this system runs was withdrawn: what to say, and
/// whether its release keys advise going back.
fn withdrawn_running() -> Result<Option<(String, bool)>, Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let mut client = online::Client::connect(Settings::load()?)?;
    let Some(withdrawal) = client.withdrawal(&running)? else { return Ok(None) };
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

/// `update apply`: stages the system update, or the firmware set, and
/// updates the apps. Nothing restarts.
fn apply(yes: bool) -> Result<i32, Fail> {
    let staged = stage_offered()?;
    match &staged {
        Some(what) => {
            println!("update: {what} is staged: it starts at the next restart you choose");
        }
        None => println!("update: no system or firmware update waits"),
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

/// Stages the system update the channel offers, or else the firmware set
/// it offers; what was staged, if anything. The two share the spare
/// slot, so the firmware waits for a system update to prove itself.
fn stage_offered() -> Result<Option<String>, Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let mut client = online::Client::connect(Settings::load()?)?;
    if let Plan::Update { release, .. } = online::plan(&mut client, &running, None)? {
        // A bridge release, when the one offered updates only from it.
        let staged = release.version;
        let asked = stage::Asked { repo: None, version: None, stop: None };
        stage::stage(&root, &asked)?;
        if firmware_offer()?.take.is_some() {
            println!("update: the firmware set waits until Dukovo {staged} has proved itself");
        }
        return Ok(Some(format!("Dukovo {staged}")));
    }
    if let Some(set) = firmware_offer()?.take {
        firmware::stage(&root, None, Some(&set))?;
        return Ok(Some(format!("firmware {set}")));
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
        return bundle(&[if on { "hold" } else { "unhold" }, item]);
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
/// the start that first ran it, then the apps'.
fn history() -> Result<i32, Fail> {
    println!("System and firmware, by the start that first ran each:");
    let records = starts::all();
    let value = |start: &starts::Start, kind: &str| {
        start.items.iter().find(|item| item.kind == kind).map(|item| item.value.clone())
    };
    let mut any = false;
    for pair in records.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        for (kind, what) in [("system", "Dukovo"), ("firmware", "firmware")] {
            let (from, to) = (value(before, kind), value(after, kind));
            if from == to {
                continue;
            }
            any = true;
            let outcome = match after.outcome {
                Outcome::Good => "good".to_string(),
                Outcome::Unfinished => "unfinished".to_string(),
                Outcome::Failed => format!("failed ({})", after.error.as_deref().unwrap_or("?")),
            };
            println!(
                "  start {} on {}: {what} {} to {}; {outcome}",
                after.number,
                util::local_time(after.started, "%Y-%m-%d %H:%M"),
                from.as_deref().unwrap_or("none"),
                to.as_deref().unwrap_or("none")
            );
        }
    }
    if !any {
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
            _ => std::thread::sleep(EVERY),
        }
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
    }
}

/// One check, as `settings` say; whether an update was staged.
fn round(settings: &Settings) -> bool {
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
        staged = system_round(settings);
    }
    apps_round(settings);
    staged
}

/// The system and firmware part of a check.
fn system_round(settings: &Settings) -> bool {
    if settings.unattended {
        return match stage_offered() {
            Ok(Some(what)) => {
                tell(&format!("{what} is staged: it starts at the next restart"));
                true
            }
            Ok(None) => false,
            Err(why) => {
                tell(&format!("the update could not be staged: {}", why.message));
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
        if settings.check == Check::Download {
            let _ = sysupdate::check(None, true);
        }
        tell(&format!("Dukovo {offered} is ready: `elevate update apply` stages it"));
    } else if let Ok(FirmwareOffer { take: Some(set), .. }) = firmware_offer() {
        tell(&format!("firmware {set} is ready: `elevate update apply` stages it"));
    }
    false
}

/// The apps part of a check: updated for everyone, or told of.
fn apps_round(settings: &Settings) {
    let mut words = vec!["update", "--for-everyone"];
    words.push(if settings.apps == Apps::Auto { "--yes" } else { "--check" });
    let ran = Command::new(Path::new("/system/commands/bundle")).args(&words).output();
    let Ok(output) = ran else { return };
    let text = String::from_utf8_lossy(&output.stdout);
    let summary = text.lines().last().unwrap_or_default().to_string();
    println!("updated: apps: {summary}");
    if settings.apps == Apps::Notify && text.lines().any(|line| line.starts_with("ID ")) {
        tell(&format!("app updates are ready: {summary}; `update apply` takes them"));
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
