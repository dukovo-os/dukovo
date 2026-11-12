// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sysupdate(8): Dukovo's atomic updates (ADRs 0451 to 0454). The system
//! runs from one of two read-only slots, `@system.a` and `@system.b`, on
//! its Btrfs root; an update is built into the other, the spare, while
//! the system runs (`stage`), and starts at the next restart, with three
//! tries to prove itself to the health check (`health`, a service). One
//! that does not falls back to the version before it, with its settings.
//! Updates come from the update server through the channel this system
//! follows (`check`, `download`, `channel`, ADR 0466), or from a
//! repository folder or an update file the owner names. `status` says
//! which slot runs and what the other holds, `unstage` takes a staged
//! update back, and `rollback` makes the previous version the one that
//! starts. Which slot starts is the slot state on the EFI
//! system partitions, which GRUB's menu reads and counts the tries in
//! (`place`).
//!
//! When an update will not stage, `sysupdate stage` says why in one line:
//! start there. When one fell back, the health check's line in the log
//! (`log sysupdate`) says what failed, and the settings it had
//! are kept in `@snapshots/update/settings-failed`.

mod health;
pub(crate) mod online;
pub(crate) mod place;
pub(crate) mod stage;

use std::path::PathBuf;

use slots::Slot;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail};

/// sysupdate(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let usage = || {
        util::usage(
            "say status, check, download, stage, unstage, rollback, channel or health".into(),
        )
    };
    let Some(action) = args.subcommand() else { return Err(usage()) };
    let action = action.to_str().unwrap_or_default().to_string();
    let (mut version, mut stop, mut repo) = (None, None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name)
                if name == "version"
                    && matches!(action.as_str(), "stage" | "check" | "download") =>
            {
                version = Some(args.text()?);
            }
            // For the boot tests: stop where a power cut is to fall.
            Arg::Long(name) if name == "test-stop-after" && action == "stage" => {
                stop = match args.text()?.as_str() {
                    "build" => Some(stage::Stop::Built),
                    other => return Err(util::usage(format!("no step called {other}"))),
                };
            }
            Arg::Operand(path)
                if matches!(action.as_str(), "stage" | "channel") && repo.is_none() =>
            {
                repo = Some(PathBuf::from(path));
            }
            other => return Err(args.unknown(&other)),
        }
    }
    match action.as_str() {
        "status" => status(),
        "stage" => {
            let root = place::root()?;
            let asked = stage::Asked { repo: repo.as_deref(), version: version.as_deref(), stop };
            stage::stage(&root, &asked).map(|()| 0)
        }
        "check" => check(version.as_deref(), false),
        "download" => check(version.as_deref(), true),
        "channel" => channel(repo.as_deref().and_then(|p| p.to_str())),
        "unstage" => unstage(),
        "rollback" => rollback(),
        "health" => health::run(),
        _ => Err(usage()),
    }
}

/// `sysupdate status`: which slot runs, what each holds, and what starts
/// next.
fn status() -> Result<i32, Fail> {
    let root = place::root()?;
    let (copies, state) = place::state(&root)?;
    let letter = |slot: Slot| slot.letter().to_ascii_uppercase();
    let version = |slot: Slot| {
        root.about(slot).map_or_else(|| "nothing".into(), |a| format!("Dukovo {}", a.version))
    };
    println!("Running:  slot {}, {}", letter(root.running), version(root.running));
    for slot in [Slot::A, Slot::B] {
        let what = if !root.slot(slot).exists() {
            "empty".to_string()
        } else if state.trying == Some(slot) && state.tries > 0 {
            format!("{}, an update being tried: {} tries left", version(slot), state.tries)
        } else if state.trying == Some(slot) {
            format!("{}, an update that did not prove itself", version(slot))
        } else if state.default == slot {
            format!("{}, the default", version(slot))
        } else if state.previous == Some(slot) {
            format!("{}, the previous version", version(slot))
        } else {
            format!("{}, not started", version(slot))
        };
        println!("Slot {}:   {what}", letter(slot));
    }
    let (next, fallback) = state.starts();
    let then = match fallback {
        Some(update) => format!(", as the update in slot {} has no tries left", letter(update)),
        None => String::new(),
    };
    println!("Next start: slot {}{then}", letter(next));
    if let Some(slot) = state.boot {
        println!(
            "Not seen through: the start of slot {}; the boot menu shows at the next start",
            letter(slot)
        );
    }
    if let Ok(text) = std::fs::read_to_string(health::LAST) {
        let mut words = text.trim().splitn(3, ' ');
        let (when, result, rest) = (words.next(), words.next(), words.next());
        if let (Some(when), Some(result)) =
            (when, result.filter(|r| *r == "passed" || *r == "failed"))
        {
            let when = when.parse().map_or_default(|s: i64| util::local_time(s, "%Y-%m-%d %H:%M"));
            println!("Last health check: {result}, {when}: {}", rest.unwrap_or_default());
        }
    }
    if let Some(about) = stage::about_update(&root) {
        let get = |key: &str| about.iter().find(|(k, _)| k == key).map_or("?", |(_, v)| v.as_str());
        let restored = if stage::restored(&root) {
            "; a fallback put the settings from before it back"
        } else {
            ""
        };
        println!(
            "Last update: {} to {} in slot {}{restored}",
            get("from"),
            get("to"),
            get("slot").to_ascii_uppercase()
        );
    }
    let partitions: Vec<String> = copies
        .iter()
        .map(|c| match c.newest() {
            Some((newest, _)) => format!("{} (generation {})", c.partition, newest.generation),
            None => format!("{} (no whole copy)", c.partition),
        })
        .collect();
    println!("Slot state: on {}", partitions.join(", "));
    Ok(0)
}

/// `sysupdate check` and `download`: what the update server offers this
/// system, and how much of it this system lacks; `download` then fetches
/// that, for the next `sysupdate stage`.
fn check(version: Option<&str>, download: bool) -> Result<i32, Fail> {
    let root = place::root()?;
    let running = stage::running_version(&root)?;
    let settings = online::Settings::load()?;
    let mut client = online::Client::connect(settings)?;
    let release = match online::plan(&mut client, &running, version)? {
        online::Plan::Current { offered } => {
            let compared = slots::version::Version::parse(&offered)
                .zip(slots::version::Version::parse(&running))
                .map_or(std::cmp::Ordering::Equal, |(offered, running)| offered.cmp(&running));
            let then = if compared == std::cmp::Ordering::Less {
                format!(", older than this system's {running}: nothing to do until it passes it")
            } else {
                ", which this system runs: it is up to date".into()
            };
            println!("sysupdate: {}{then}", online::offers(&client.settings, version, &offered));
            return Ok(0);
        }
        online::Plan::Update { offered, from, release } => {
            println!(
                "sysupdate: {}; this system runs Dukovo {running}",
                online::offers(&client.settings, version, &offered)
            );
            if release.version != offered {
                println!(
                    "sysupdate: Dukovo {offered} updates only from {from} on: Dukovo {} comes first",
                    release.version
                );
            }
            release
        }
    };
    let needed = online::needed(&root.slot(root.running), &release)?;
    if !download {
        println!(
            "sysupdate: Dukovo {} needs {} files this system lacks, {}; sysupdate stage fetches \
             and stages it",
            release.version,
            needed.fetch.len(),
            online::mib(needed.bytes)
        );
        return Ok(0);
    }
    let fetched = online::fetch(&client, &needed)?;
    println!(
        "sysupdate: fetched {} files of Dukovo {}, {}, and copied {}; sysupdate stage stages it",
        fetched.fetched,
        release.version,
        online::mib(fetched.bytes),
        fetched.copied
    );
    Ok(0)
}

/// `sysupdate channel [NAME]`: the channel this system follows, or, with
/// a name, follows from now on.
fn channel(name: Option<&str>) -> Result<i32, Fail> {
    let mut settings = online::Settings::load()?;
    let Some(name) = name else {
        println!("Channel: {}", settings.channel);
        println!("Server:  {}", settings.server);
        return Ok(0);
    };
    online::check_channel(name).map_err(util::usage)?;
    settings.channel = name.to_string();
    settings.save()?;
    println!(
        "sysupdate: this system follows the {name} channel from now on; it never goes back to an \
         older version: it waits until the channel passes the one it runs"
    );
    Ok(0)
}

/// `sysupdate unstage`: the staged update is not tried; it stays in its
/// slot until the next stage.
fn unstage() -> Result<i32, Fail> {
    let root = place::root()?;
    let _lock = place::lock()?;
    let (copies, state) = place::state(&root)?;
    let update = state.trying;
    let next = state.unstage(root.running).map_err(|why| fail(why.to_string()))?;
    place::write(&copies, next)?;
    if let Some(slot) = update {
        println!(
            "sysupdate: {} will not be tried; {} goes on starting",
            place::named(&root, slot),
            place::named(&root, root.running)
        );
    }
    Ok(0)
}

/// `sysupdate rollback`: the previous version starts from the next
/// restart on, with the settings as they are.
fn rollback() -> Result<i32, Fail> {
    let root = place::root()?;
    let _lock = place::lock()?;
    let (copies, state) = place::state(&root)?;
    let next = state.rollback().map_err(|why| fail(why.to_string()))?;
    if !root.slot(next.default).join("startup/radix").exists() {
        return Err(fail(format!(
            "slot {} has no kernel to start",
            next.default.letter().to_ascii_uppercase()
        )));
    }
    place::write(&copies, next)?;
    println!(
        "sysupdate: {} starts from the next restart on; restart to go back to it",
        place::named(&root, next.default)
    );
    Ok(0)
}
