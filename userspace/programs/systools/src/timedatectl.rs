// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! timedatectl(1): the clock, the time zone, and what timed(8) did (ADR
//! 0194), with systemd's timedatectl's commands.
//!
//! The time zone is `/etc/localtime`, a link into the IANA time zone
//! database in `/system/resources/zoneinfo`, as on Linux; `set-timezone`
//! points it elsewhere, which needs root's write to `/system/settings`.
//! `set-time` sets the clock through the clock capability (radix-clock(2)),
//! which root's shells at the console and elevate(8) hold, while timed is
//! not keeping it.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;

use radix_abi::clock;
use radix_abi::errors::Kernel;
use radix_abi::startup::SLOT_CLOCK;
use sysapi::process;
use sysapi::time::{self, Civil};

use crate::args::{Arg, Args};
use crate::sys::SYS;
use crate::timed;
use crate::util::{self, Fail, fail, usage};

/// The time zone database.
pub const ZONEINFO: &str = "/system/resources/zoneinfo";
/// The time zone: a link into [`ZONEINFO`], which `/etc/localtime` is.
pub const LOCALTIME: &str = "/system/settings/localtime";

/// Runs timedatectl.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(word) => operands.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let mut operands = operands.into_iter();
    let command = operands.next();
    let rest: Vec<String> = operands.collect();
    match command.as_deref() {
        None | Some("status") => {
            status();
            Ok(0)
        }
        Some("timesync-status" | "show-timesync") => {
            let text = fs::read_to_string(timed::STATUS)
                .map_err(|e| util::failed(&format!("timedatectl: {}", timed::STATUS), e))?;
            print!("{text}");
            Ok(0)
        }
        Some("list-timezones") => {
            for zone in zones() {
                println!("{zone}");
            }
            Ok(0)
        }
        Some("set-timezone") => {
            let [zone] = rest.as_slice() else {
                return Err(usage("timedatectl: set-timezone takes one time zone".into()));
            };
            set_timezone(zone)?;
            Ok(0)
        }
        Some("set-time") => {
            if rest.is_empty() {
                return Err(usage(
                    "timedatectl: set-time takes a time, as 2026-11-01 12:00:00".into(),
                ));
            }
            set_time(&rest.join(" "))?;
            Ok(0)
        }
        Some(other) => Err(usage(format!(
            "timedatectl: no command '{other}': status, timesync-status, list-timezones, \
             set-timezone or set-time"
        ))),
    }
}

/// The time zone `/etc/localtime` names, by the link's target.
pub fn timezone() -> String {
    let Ok(target) = fs::read_link(LOCALTIME) else {
        return if Path::new(LOCALTIME).exists() {
            "a file of its own".into()
        } else {
            "UTC".into()
        };
    };
    let target = target.to_string_lossy().into_owned();
    for prefix in [ZONEINFO, "/usr/share/zoneinfo"] {
        if let Some(zone) = target.strip_prefix(prefix).and_then(|z| z.strip_prefix('/')) {
            return zone.to_string();
        }
    }
    target
}

/// Whether timed runs: its process number names a process.
fn timed_runs() -> bool {
    fs::read_to_string(timed::PID)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .is_some_and(|pid| process::kill(pid, 0).is_ok())
}

fn status() {
    let now = util::now_ns() / 1_000_000_000;
    let now = i64::try_from(now).unwrap_or(0);
    let rtc = clock::state(&SYS, SLOT_CLOCK).ok().map(|state| state.rtc_seconds).filter(|&s| s > 0);
    let synced =
        fs::read_to_string(timed::STATUS).is_ok_and(|text| text.starts_with("Synchronized: yes"));
    let zone = timezone();
    let rows = [
        ("Local time", util::time_text(now, "%a %F %T %Z", false)),
        ("Universal time", util::time_text(now, "%a %F %T UTC", true)),
        (
            "RTC time",
            rtc.map_or_else(
                || "n/a (only with the clock capability)".into(),
                |s| util::time_text(i64::try_from(s).unwrap_or(0), "%a %F %T", true),
            ),
        ),
        ("Time zone", format!("{zone} ({})", util::time_text(now, "%Z, %z", false))),
        ("System clock synchronized", if synced { "yes" } else { "no" }.into()),
        ("NTP service", if timed_runs() { "active" } else { "inactive" }.into()),
        ("RTC in local TZ", "no".into()),
    ];
    for (name, value) in rows {
        println!("{name:>25}: {value}");
    }
}

/// The time zones of the database: zone1970.tab's, and UTC, sorted.
fn zones() -> Vec<String> {
    let mut zones: Vec<String> = fs::read_to_string(format!("{ZONEINFO}/zone1970.tab"))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(2).map(String::from))
        .collect();
    zones.push("UTC".into());
    zones.sort();
    zones.dedup();
    zones
}

/// Whether `path` holds a TZif file.
fn is_tzif(path: &Path) -> bool {
    fs::read(path).is_ok_and(|data| data.starts_with(b"TZif"))
}

/// Whether `zone` names a time zone of the database in `database`: a
/// relative name with no empty, `.` or `..` part, so that it stays inside
/// the database, whose file there is a TZif file. A link of the database,
/// such as `Asia/Calcutta`, is a zone too, as it is to `set-timezone`;
/// browsers still name zones by their links.
fn is_zone_in(database: &Path, zone: &str) -> bool {
    let safe = !zone.is_empty()
        && !zone.starts_with('/')
        && zone.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
    safe && is_tzif(&database.join(zone))
}

/// Whether `zone` names a time zone of the system's database,
/// [`ZONEINFO`]: what `set-timezone` takes, and the web console's
/// preferences.
pub(crate) fn is_zone(zone: &str) -> bool {
    is_zone_in(Path::new(ZONEINFO), zone)
}

fn set_timezone(zone: &str) -> Result<(), Fail> {
    if !is_zone(zone) {
        return Err(fail(format!(
            "timedatectl: no time zone '{zone}': timedatectl list-timezones lists them"
        )));
    }
    let target = format!("{ZONEINFO}/{zone}");
    let new = format!("{LOCALTIME}.new");
    let _ = fs::remove_file(&new);
    symlink(&target, &new)
        .and_then(|()| fs::rename(&new, LOCALTIME))
        .map_err(|e| util::failed(&format!("timedatectl: {LOCALTIME}"), e))?;
    Ok(())
}

/// Reads `text` as a local time, `YYYY-MM-DD HH:MM[:SS]`, `YYYY-MM-DD`
/// or `HH:MM[:SS]` today, into seconds since 1970.
fn parse_local(text: &str) -> Option<i64> {
    let now = i64::try_from(util::now_ns() / 1_000_000_000).ok()?;
    let today = util::time_text(now, "%F", false);
    let (date, time) = match text.trim().split_once(' ') {
        Some((date, time)) => (date.to_string(), time.to_string()),
        None if text.contains('-') => (text.trim().to_string(), "00:00:00".into()),
        None => (today, text.trim().to_string()),
    };
    let date: Vec<i32> = date.split('-').map(str::parse).collect::<Result<_, _>>().ok()?;
    let time: Vec<i32> = time.split(':').map(str::parse).collect::<Result<_, _>>().ok()?;
    let ([year, month, day], [hour, minute, rest @ ..]) = (date.as_slice(), time.as_slice()) else {
        return None;
    };
    let second = match rest {
        [] => 0,
        [second] => *second,
        _ => return None,
    };
    let wanted = Civil {
        year: i64::from(*year),
        month: u32::try_from(*month).ok()?,
        day: u32::try_from(*day).ok()?,
        hour: u32::try_from(*hour).ok()?,
        minute: u32::try_from(*minute).ok()?,
        second: u32::try_from(second).ok()?,
        ..Civil::default()
    };
    let (seconds, found) = time::from_local(&wanted).ok()?;
    // A field out of range comes back changed.
    ((found.month, found.day, found.hour, found.minute)
        == (wanted.month, wanted.day, wanted.hour, wanted.minute))
        .then_some(seconds)
}

fn set_time(text: &str) -> Result<(), Fail> {
    if timed_runs() {
        return Err(fail(
            "timedatectl: timed keeps the clock from the network; `service timed stop` first"
                .into(),
        ));
    }
    let seconds = parse_local(text).ok_or_else(|| {
        usage(format!("timedatectl: '{text}' is not a time, as 2026-11-01 12:00:00"))
    })?;
    let ns = u64::try_from(seconds).unwrap_or(0).saturating_mul(1_000_000_000);
    clock::set(&SYS, SLOT_CLOCK, ns).map_err(|status| {
        if Kernel::from_status(status) == Some(Kernel::InvalidArgument) {
            fail(format!("timedatectl: the clock takes no time before 1980 or from 2500: '{text}'"))
        } else {
            // No capability, or another kind in its slot: as Linux says
            // without CAP_SYS_TIME.
            util::failed(
                &"timedatectl: setting the clock",
                io::Error::from_raw_os_error(libc::EPERM),
            )
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_zone_is_a_tzif_file_inside_the_database() {
        let dir = std::env::temp_dir().join(format!("timedatectl-zones-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Europe")).unwrap();
        std::fs::write(dir.join("Europe/Berlin"), b"TZif2 rest").unwrap();
        std::fs::write(dir.join("UTC"), b"TZif2").unwrap();
        std::fs::write(dir.join("zone1970.tab"), "# a table\n").unwrap();
        std::os::unix::fs::symlink("Berlin", dir.join("Europe/Kiel")).unwrap();
        std::fs::write(dir.join("outside"), b"TZif2").unwrap();
        for zone in ["Europe/Berlin", "UTC", "Europe/Kiel"] {
            assert!(is_zone_in(&dir, zone), "{zone}");
        }
        let names = [
            "",
            "/UTC",
            "Europe",
            "Europe/",
            "Europe//Berlin",
            "./UTC",
            "Europe/../UTC",
            "../outside",
            "zone1970.tab",
            "Europe/Paris",
            "UTC\0",
            "europe/berlin",
        ];
        for zone in names {
            assert!(!is_zone_in(&dir, zone), "{zone:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn local_times_read() {
        // TZ is unset in the tests' environment or UTC there; either way
        // a whole date reads.
        assert!(parse_local("2026-11-01 12:00:00").is_some());
        assert!(parse_local("2026-11-01 12:00").is_some());
        assert!(parse_local("2026-11-01").is_some());
        assert!(parse_local("12:30").is_some());
        assert_eq!(parse_local("2026-13-01 12:00"), None);
        assert_eq!(parse_local("noon"), None);
    }
}
