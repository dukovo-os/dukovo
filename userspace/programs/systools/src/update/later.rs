// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `update later ITEM (--for 1d | --for 7d | --off)` (ADR 0565): puts the
//! reminder of an update off for a day or a week, or brings it back.
//!
//! ITEM is `system`, `firmware` or an app's ID. The command reads the
//! offers as `update check` does and keeps nothing but a reminder that
//! hides no fix: an item with nothing on offer, one that is held and one
//! whose update fixes an advisory are refused (UPD-107 for the last), and
//! so is one whose advisories could not be read, which fails closed. The
//! entry is kept in [`later`]'s file, and every reader asks [`later::stands`].

use super::advisories::{self, Fixing};
use super::{FirmwareOffer, asker, firmware_offer, last};
use crate::args::{Arg, Args};
use crate::bundles;
use crate::sysupdate::checked::Found;
use crate::sysupdate::later::{Entry, Later};
use crate::sysupdate::online::{self, Plan, Settings};
use crate::sysupdate::{place, stage};
use crate::util::{self, Fail, fail, usage};

/// What an item has on offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Look {
    /// No update, or one that is not ready yet.
    Nothing,
    /// An update, held back.
    Held,
    /// An update to this version.
    Offered(String),
}

/// How many milliseconds a day takes.
const DAY_MS: u64 = 86_400_000;

/// What the person asked of `update later`.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// Put the reminder off for this many days.
    For(u64),
    /// Bring it back.
    Off,
}

/// `update later ITEM (--for 1d | --for 7d | --off)`.
pub(super) fn later(args: &mut Args) -> Result<i32, Fail> {
    let (item, asked) = read(args)?;
    if !last::is_root() {
        return Err(fail(format!(
            "putting an update off is root's: run `elevate update later {item} ...`"
        )));
    }
    let now_ms = util::now_ns() / 1_000_000;
    let mut kept = Later::read();
    let entry = match asked {
        Asked::Off => None,
        Asked::For(days) => {
            let look = look_at(&item)?;
            // The server is asked of advisories for an update there is.
            let fixing = match look {
                Look::Offered(ref version) => advisories::gather(true).fixing(&item, version),
                _ => Fixing::Nothing,
            };
            Some(decide(&item, &look, &fixing, now_ms, days, asker()).map_err(fail)?)
        }
    };
    let said = match entry {
        Some(entry) => {
            let said = put_off_said(&item, &entry);
            kept.put(entry);
            said
        }
        None => {
            if kept.off(&item) {
                format!("{item} is no longer put off: its reminders are back")
            } else {
                format!("{item} was not put off")
            }
        }
    };
    kept.tidy(now_ms);
    kept.save().map_err(|why| {
        fail(format!("{}: {}", crate::sysupdate::later::FILE, util::strip_os_error(&why)))
    })?;
    println!("update: {said}");
    Ok(0)
}

/// The item and what is asked of it, from the command line.
fn read(args: &mut Args) -> Result<(String, Asked), Fail> {
    let mut item = None;
    let mut asked = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "for" && asked.is_none() => {
                asked = Some(match args.text()?.as_str() {
                    "1d" => Asked::For(1),
                    "7d" => Asked::For(7),
                    other => return Err(usage(format!("--for takes 1d or 7d, not '{other}'"))),
                });
            }
            Arg::Long(name) if name == "off" && asked.is_none() => asked = Some(Asked::Off),
            Arg::Operand(operand) if item.is_none() => {
                item = Some(operand.to_string_lossy().into_owned());
            }
            _ => return Err(usage(super::USAGE.into())),
        }
    }
    let (Some(item), Some(asked)) = (item, asked) else {
        return Err(usage(super::USAGE.into()));
    };
    if item != "system" && item != "firmware" && manifest::bundle::check_full_id(&item).is_err() {
        return Err(usage(format!(
            "'{item}' is not system, firmware or an app's ID, as org.example.app"
        )));
    }
    Ok((item, asked))
}

/// What `item` has on offer now, as `update check` says it.
fn look_at(item: &str) -> Result<Look, Fail> {
    match item {
        "system" => {
            let root = place::root()?;
            let running = stage::running_version(&root)?;
            let mut client = online::Client::connect(Settings::load()?)?;
            Ok(match online::plan(&mut client, &running, None)? {
                Plan::Update { offered, .. } => Look::Offered(offered),
                Plan::Held { .. } => Look::Held,
                _ => Look::Nothing,
            })
        }
        "firmware" => Ok(match firmware_offer()? {
            FirmwareOffer { found: Found::Held, .. } => Look::Held,
            FirmwareOffer { take: Some(set), .. } => Look::Offered(set),
            _ => Look::Nothing,
        }),
        app => {
            let pending = bundles::pending_updates()?;
            Ok(match pending.rows.iter().find(|row| row.id == app) {
                Some(row) if row.held => Look::Held,
                Some(row) => Look::Offered(row.to.clone()),
                None => Look::Nothing,
            })
        }
    }
}

/// What to call the update `version` of `item`.
fn subject(item: &str, version: &str) -> String {
    match item {
        "system" => format!("Dukovo {version}"),
        "firmware" => format!("firmware {version}"),
        app => format!("{app} {version}"),
    }
}

/// The advisories an update fixes, as a sentence's words: the first three
/// and how many more.
fn named(fixed: &[(String, dkv::flaws::Severity)]) -> String {
    let mut names: Vec<String> =
        fixed.iter().take(3).map(|(id, severity)| format!("{id} ({})", severity.word())).collect();
    if fixed.len() > 3 {
        names.push(format!("{} more", fixed.len() - 3));
    }
    names.join(", ")
}

/// The entry that puts off the reminder of what `look` offers for `days`
/// from `now_ms`, made by user `by`, or why not, as `update` says it
/// without its name.
pub(super) fn decide(
    item: &str,
    look: &Look,
    fixing: &Fixing,
    now_ms: u64,
    days: u64,
    by: u32,
) -> Result<Entry, String> {
    let version = match look {
        Look::Nothing => return Err(format!("nothing of {item} is offered")),
        Look::Held => return Err(format!("{item} is held already")),
        Look::Offered(version) => version,
    };
    match fixing {
        Fixing::Unread => Err("whether it fixes an advisory cannot be told: the advisories \
                               could not be read (UPD-107)"
            .into()),
        Fixing::Fixes(fixed) => Err(format!(
            "{} fixes {}: it is not put off (UPD-107)",
            subject(item, version),
            named(fixed)
        )),
        Fixing::Nothing => Ok(Entry {
            item: item.into(),
            version: version.clone(),
            until_ms: now_ms + days * DAY_MS,
            by,
            at_ms: now_ms,
        }),
    }
}

/// What `update later` says of the entry it made.
fn put_off_said(item: &str, entry: &Entry) -> String {
    let until = util::local_time(i64::try_from(entry.until_ms / 1000).unwrap_or(0), "%Y-%m-%d");
    format!(
        "{} is put off until {until}: no reminder of it until then; `update later {item} --off` \
         brings it back",
        subject(item, &entry.version)
    )
}

/// The line `update check` says under an offer whose entry stands, `until_ms`
/// being its time.
pub(super) fn standing_line(until_ms: u64) -> String {
    let until = util::local_time(i64::try_from(until_ms / 1000).unwrap_or(0), "%Y-%m-%d");
    format!("put off until {until} (update later)")
}

#[cfg(test)]
mod tests {
    use dkv::flaws::Severity;

    use super::*;

    const NOW: u64 = 1_794_657_630_000;

    fn read_words(words: &[&str]) -> Result<(String, Asked), Fail> {
        read(&mut Args::new("update", words.iter().map(std::ffi::OsString::from).collect()))
    }

    #[test]
    fn the_command_line_names_an_item_and_a_day_a_week_or_off() {
        let ask = |words: &[&str]| read_words(words).unwrap();
        assert_eq!(ask(&["system", "--for", "1d"]), ("system".into(), Asked::For(1)));
        assert_eq!(ask(&["firmware", "--for", "7d"]), ("firmware".into(), Asked::For(7)));
        assert_eq!(ask(&["org.example.notes", "--off"]), ("org.example.notes".into(), Asked::Off));
        assert_eq!(ask(&["--for", "7d", "system"]), ("system".into(), Asked::For(7)));
        for words in [
            vec![],
            vec!["system"],
            vec!["--off"],
            vec!["system", "--for", "2d"],
            vec!["system", "--for", "1d", "--off"],
            vec!["system", "--off", "--off"],
            vec!["system", "firmware", "--off"],
            vec!["apps", "--off"],
            vec!["../x", "--for", "1d"],
            vec!["Notes", "--for", "1d"],
        ] {
            let failed = read_words(&words).unwrap_err();
            assert!(failed.usage, "{words:?}");
        }
    }

    #[test]
    fn an_offer_is_put_off_for_a_day_or_a_week_from_now() {
        let look = Look::Offered("1.0.0-alpha.2".into());
        let day = decide("system", &look, &Fixing::Nothing, NOW, 1, 1000).unwrap();
        assert_eq!(
            day,
            Entry {
                item: "system".into(),
                version: "1.0.0-alpha.2".into(),
                until_ms: NOW + DAY_MS,
                by: 1000,
                at_ms: NOW,
            }
        );
        let week = decide(
            "org.example.notes",
            &Look::Offered("1.3.0".into()),
            &Fixing::Nothing,
            NOW,
            7,
            0,
        )
        .unwrap();
        assert_eq!(
            (week.until_ms, week.by, week.item.as_str()),
            (NOW + 7 * DAY_MS, 0, "org.example.notes")
        );
    }

    #[test]
    fn an_item_with_nothing_on_offer_a_held_one_and_one_that_fixes_an_advisory_are_refused() {
        let refuse = |item: &str, look: Look, fixing: Fixing| {
            decide(item, &look, &fixing, NOW, 1, 1000).unwrap_err()
        };
        assert_eq!(
            refuse("system", Look::Nothing, Fixing::Nothing),
            "nothing of system is offered"
        );
        assert_eq!(
            refuse("org.example.notes", Look::Nothing, Fixing::Nothing),
            "nothing of org.example.notes is offered"
        );
        assert_eq!(refuse("firmware", Look::Held, Fixing::Nothing), "firmware is held already");
        // Held is told before an advisory is, and nothing offered before both.
        let fixes = Fixing::Fixes(vec![("DKSA-2026-0007".into(), Severity::High)]);
        assert_eq!(refuse("system", Look::Held, fixes.clone()), "system is held already");
        assert_eq!(
            refuse("system", Look::Offered("1.0.0-alpha.2".into()), fixes.clone()),
            "Dukovo 1.0.0-alpha.2 fixes DKSA-2026-0007 (high): it is not put off (UPD-107)"
        );
        assert_eq!(
            refuse("firmware", Look::Offered("2026.3".into()), fixes.clone()),
            "firmware 2026.3 fixes DKSA-2026-0007 (high): it is not put off (UPD-107)"
        );
        assert_eq!(
            refuse("org.example.notes", Look::Offered("1.3.0".into()), fixes),
            "org.example.notes 1.3.0 fixes DKSA-2026-0007 (high): it is not put off (UPD-107)"
        );
        // Advisories that could not be read: the update is not put off either.
        assert_eq!(
            refuse("system", Look::Offered("1.0.0-alpha.2".into()), Fixing::Unread),
            "whether it fixes an advisory cannot be told: the advisories could not be read \
             (UPD-107)"
        );
    }

    #[test]
    fn an_update_that_fixes_several_names_the_first_three_and_counts_the_rest() {
        let many: Vec<(String, Severity)> = [
            "DKSA-2026-0001",
            "DKSA-2026-0002",
            "DKSA-2026-0003",
            "DKSA-2026-0004",
            "DKSA-2026-0005",
        ]
        .iter()
        .zip([Severity::Critical, Severity::High, Severity::High, Severity::Low, Severity::Low])
        .map(|(id, severity)| ((*id).to_string(), severity))
        .collect();
        assert_eq!(
            named(&many),
            "DKSA-2026-0001 (critical), DKSA-2026-0002 (high), DKSA-2026-0003 (high), 2 more"
        );
        assert_eq!(named(&many[..1]), "DKSA-2026-0001 (critical)");
    }

    #[test]
    fn the_words_say_until_when_and_how_to_bring_it_back() {
        let entry = Entry {
            item: "system".into(),
            version: "1.0.0-alpha.2".into(),
            until_ms: NOW + 7 * DAY_MS,
            by: 1000,
            at_ms: NOW,
        };
        let until = util::local_time(i64::try_from((NOW + 7 * DAY_MS) / 1000).unwrap(), "%Y-%m-%d");
        assert_eq!(
            put_off_said("system", &entry),
            format!(
                "Dukovo 1.0.0-alpha.2 is put off until {until}: no reminder of it until then; \
                 `update later system --off` brings it back"
            )
        );
        assert_eq!(standing_line(entry.until_ms), format!("put off until {until} (update later)"));
    }
}
