// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `update settings`, which says how this system updates, and `update
//! set`, which changes it: the keys of updates.toml(5) the web console
//! offers, written with [`settings_edit`] so that the comments and the
//! order of the file stay.
//!
//! What is compared and shown is what updated goes by: the file with its
//! defaults filled in. A key a change would give the value it already has
//! by default is not written.

use std::path::Path;

use serde::Serialize;
use toml_edit::DocumentMut;

use super::held::{Held, HeldDocument};
use crate::args::{Arg, Args};
use crate::bundles::registry::{self, Place};
use crate::json;
use crate::settings_edit::{self, Changed};
use crate::sysupdate::online::{Apps, Check, Hold, Metered, SETTINGS, Settings, minutes_of_day};
use crate::util::{self, Fail, fail, usage};

/// The members of `update settings --json` after `format`.
#[derive(Debug, Serialize)]
pub(crate) struct Document {
    /// `stable`, `beta` or `nightly`.
    channel: String,
    /// `notify`, `download` or `off`.
    check: &'static str,
    /// `notify` or `auto`.
    apps: &'static str,
    /// `wait` or `download`.
    metered: &'static str,
    unattended: bool,
    /// `HH:MM`; none for no restart time.
    restart: Option<String>,
    hold: HoldDocument,
    /// The release the health check held after it fell back from it,
    /// while the hold it made is there.
    held: Option<HeldDocument>,
    /// Whether bundled updates the apps of everyone by themselves.
    apps_automatic: bool,
    /// Whether `apps` and bundled say different things of that.
    apps_disagree: bool,
    server: String,
    canary: bool,
}

/// `hold` of the document: the versions held back.
#[derive(Debug, Serialize)]
pub(crate) struct HoldDocument {
    system: Option<String>,
    firmware: Option<String>,
}

impl Document {
    /// The settings `settings` as they stand, with what the health check
    /// kept in `held`, and whether bundled updates by themselves.
    pub(crate) fn of(settings: &Settings, held: Option<&Held>, automatic: bool) -> Document {
        let Hold { system, firmware } = settings.hold.clone();
        let held = held
            .filter(|held| system.as_deref() == Some(held.version.as_str()))
            .map(Held::document);
        Document {
            channel: settings.channel.clone(),
            check: check_word(settings.check),
            apps: apps_word(settings.apps),
            metered: metered_word(settings.metered),
            unattended: settings.unattended,
            restart: settings.restart.clone(),
            hold: HoldDocument { system, firmware },
            held,
            apps_automatic: automatic,
            apps_disagree: (settings.apps == Apps::Auto) != automatic,
            server: settings.server.clone(),
            canary: settings.canary,
        }
    }
}

/// The word updates.toml(5) has for `check`.
fn check_word(check: Check) -> &'static str {
    match check {
        Check::Notify => "notify",
        Check::Download => "download",
        Check::Off => "off",
    }
}

/// The word updates.toml(5) has for `apps`.
fn apps_word(apps: Apps) -> &'static str {
    match apps {
        Apps::Notify => "notify",
        Apps::Auto => "auto",
    }
}

/// The word updates.toml(5) has for `metered`.
fn metered_word(metered: Metered) -> &'static str {
    match metered {
        Metered::Wait => "wait",
        Metered::Download => "download",
    }
}

/// The word `update set` has for a switch.
fn switch_word(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Whether the text of bundled's `automatic` file says the apps of
/// everyone update by themselves: a line `everyone`, a tab and the time
/// it began.
fn says_automatic(text: &str) -> bool {
    text.lines()
        .filter_map(|line| line.split('\t').next())
        .any(|place| Place::parse(place) == Ok(Place::Everyone))
}

/// Whether bundled updates the apps of everyone by themselves, as the file
/// it keeps says; no when there is none or it cannot be read.
fn automatic() -> bool {
    let file = Path::new(registry::STATE).join(registry::AUTOMATIC);
    std::fs::read_to_string(file).is_ok_and(|text| says_automatic(&text))
}

/// `update settings`: how this system updates, in words, or with `json`
/// as the document update(8) describes.
pub(crate) fn settings(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let document = Document::of(&Settings::load()?, Held::read().as_ref(), automatic());
    if json {
        return json::print(&document).map(|()| 0);
    }
    for line in lines(&document) {
        println!("{line}");
    }
    Ok(0)
}

/// What `update settings` says, a line for each setting and then what
/// needs a word more.
fn lines(document: &Document) -> Vec<String> {
    let or_none = |text: &Option<String>| text.clone().unwrap_or_else(|| "none".into());
    let mut lines = vec![
        format!("channel = {}", document.channel),
        format!("check = {}", document.check),
        format!("apps = {}", document.apps),
        format!("metered = {}", document.metered),
        format!("unattended = {}", switch_word(document.unattended)),
        format!("restart = {}", or_none(&document.restart)),
        format!("hold system = {}", or_none(&document.hold.system)),
        format!("hold firmware = {}", or_none(&document.hold.firmware)),
    ];
    if let Some(held) = &document.held {
        lines.push(format!(
            "update: Dukovo {} is held: it fell back, and is not staged again until `update \
             unhold system`",
            held.version
        ));
    }
    if document.apps_disagree {
        lines.push(format!(
            "update: apps = {}, but bundled {} update the apps of everyone by themselves: \
             `update set --apps {}` makes them agree",
            document.apps,
            if document.apps_automatic { "does" } else { "does not" },
            document.apps
        ));
    }
    lines
}

/// What `update set` was asked to change.
#[derive(Debug, Default, PartialEq, Eq)]
struct Wanted {
    check: Option<Check>,
    apps: Option<Apps>,
    unattended: Option<bool>,
    /// `Some(None)` for `--no-restart`.
    restart: Option<Option<String>>,
    metered: Option<Metered>,
}

/// The word `option` takes, from `words`.
fn word<T: Copy>(args: &mut Args, option: &str, words: &[(&str, T)]) -> Result<T, Fail> {
    let said = args.text()?;
    match words.iter().find(|(word, _)| *word == said) {
        Some((_, value)) => Ok(*value),
        None => {
            let names: Vec<&str> = words.iter().map(|(word, _)| *word).collect();
            Err(usage(format!("{option} takes {}, not '{said}'", names.join(", "))))
        }
    }
}

/// What the command line of `update set` asks for.
fn wanted(args: &mut Args) -> Result<Wanted, Fail> {
    let mut wanted = Wanted::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "check" => {
                let words =
                    [("notify", Check::Notify), ("download", Check::Download), ("off", Check::Off)];
                wanted.check = Some(word(args, "--check", &words)?);
            }
            Arg::Long(name) if name == "apps" => {
                let words = [("notify", Apps::Notify), ("auto", Apps::Auto)];
                wanted.apps = Some(word(args, "--apps", &words)?);
            }
            Arg::Long(name) if name == "unattended" => {
                wanted.unattended =
                    Some(word(args, "--unattended", &[("on", true), ("off", false)])?);
            }
            Arg::Long(name) if name == "metered" => {
                let words = [("wait", Metered::Wait), ("download", Metered::Download)];
                wanted.metered = Some(word(args, "--metered", &words)?);
            }
            Arg::Long(name) if name == "restart" && wanted.restart.is_none() => {
                wanted.restart = Some(Some(args.text()?));
            }
            Arg::Long(name) if name == "no-restart" && wanted.restart.is_none() => {
                wanted.restart = Some(None);
            }
            Arg::Long(name) if name == "restart" || name == "no-restart" => {
                return Err(usage("--restart and --no-restart are one choice".into()));
            }
            other => return Err(args.unknown(&other)),
        }
    }
    if wanted == Wanted::default() {
        return Err(usage(
            "update set needs a setting to change: --check, --apps, --unattended, --restart, \
             --no-restart or --metered"
                .into(),
        ));
    }
    Ok(wanted)
}

/// What a change comes to, against the settings as they stand.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// Each key that changes, and the value it gets in words: what `update
    /// set` prints.
    changes: Vec<(&'static str, String)>,
    /// What bundled is to be told, when it does not do as `apps` says.
    automatic: Option<bool>,
    /// The settings after the change.
    after: Settings,
}

/// What `wanted` changes of `now`, when bundled's updating by themselves
/// is `automatic`.
///
/// # Errors
///
/// Why it is refused: a time that is not `HH:MM`, or a restart time for a
/// machine that is not unattended.
fn plan(now: &Settings, automatic: bool, wanted: &Wanted) -> Result<Plan, String> {
    let mut after = now.clone();
    let mut changes = Vec::new();
    if let Some(check) = wanted.check.filter(|&check| check != now.check) {
        after.check = check;
        changes.push(("check", check_word(check).to_string()));
    }
    let mut tell_bundled = None;
    if let Some(apps) = wanted.apps {
        let auto = apps == Apps::Auto;
        if apps != now.apps || auto != automatic {
            changes.push(("apps", apps_word(apps).to_string()));
        }
        after.apps = apps;
        tell_bundled = (auto != automatic).then_some(auto);
    }
    if let Some(unattended) = wanted.unattended.filter(|&on| on != now.unattended) {
        after.unattended = unattended;
        changes.push(("unattended", switch_word(unattended).to_string()));
    }
    if let Some(Some(time)) = &wanted.restart
        && minutes_of_day(time).is_none()
    {
        return Err(format!("{time} is not a time of day: use HH:MM, as 03:30"));
    }
    // A restart time is for an unattended machine, so that turning that
    // off takes the time with it.
    let restart = match &wanted.restart {
        Some(time) => time.clone(),
        None if after.unattended => now.restart.clone(),
        None => None,
    };
    if restart.is_some() && !after.unattended {
        return Err("a restart time is for an unattended machine: set --unattended on too".into());
    }
    if restart != now.restart {
        changes.push(("restart", restart.clone().unwrap_or_else(|| "none".into())));
        after.restart = restart;
    }
    if let Some(metered) = wanted.metered.filter(|&metered| metered != now.metered) {
        after.metered = metered;
        changes.push(("metered", metered_word(metered).to_string()));
    }
    Ok(Plan { changes, automatic: tell_bundled, after })
}

/// Writes what `plan` changes of the keys into `document`.
fn edit(document: &mut DocumentMut, plan: &Plan) -> Result<(), String> {
    let root = document.as_table_mut();
    let after = &plan.after;
    for (key, _) in &plan.changes {
        match *key {
            "check" => settings_edit::set_string(root, "check", check_word(after.check)),
            "apps" => settings_edit::set_string(root, "apps", apps_word(after.apps)),
            "metered" => settings_edit::set_string(root, "metered", metered_word(after.metered)),
            "unattended" => settings_edit::set_bool(root, "unattended", after.unattended),
            _ => match &after.restart {
                Some(time) => settings_edit::set_string(root, "restart", time),
                None => {
                    settings_edit::remove_key(root, "restart");
                }
            },
        }
    }
    // What is written is what updated reads, or nothing is written.
    Settings::parse(&document.to_string()).map(|_| ())
}

/// `update set`: changes the settings given.
pub(crate) fn set(args: &mut Args) -> Result<i32, Fail> {
    let wanted = wanted(args)?;
    let now = Settings::load()?;
    let plan = plan(&now, automatic(), &wanted).map_err(fail)?;
    if plan.changes.is_empty() {
        println!("update: nothing to change");
        return Ok(0);
    }
    let path = Path::new(SETTINGS);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|why| util::failed(&folder.display(), why))?;
    }
    settings_edit::change(path, 0o644, |document| edit(document, &plan)).map_err(
        |why| match why {
            Changed::Edit { error, .. } => fail(error),
            other => fail(other.to_string()),
        },
    )?;
    if let Some(on) = plan.automatic {
        let words = ["update", "--automatic", switch_word(on), "--for-everyone"];
        if let Err(why) = super::bundle(&words) {
            return Err(fail(format!(
                "the settings were saved, but bundled would not change how the apps update: {}",
                why.message
            )));
        }
    }
    for (key, value) in &plan.changes {
        println!("update: settings changed: {key} = {value}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file as a person keeps it, with a comment for each key.
    const KEPT: &str = "\
# SPDX-FileCopyrightText: 2026 The Dukovo contributors

# Which releases this system is offered (sysupdate(8)).
channel = \"beta\" # for the new things

# Tell me, do not download.
check = \"notify\"
unattended = true
restart = \"03:30\"

# Versions held back until `update unhold` (update(8)).
[hold]
system = \"1.0.0-alpha.2\"
";

    fn args(items: &[&str]) -> Args {
        Args::new("update", items.iter().map(std::ffi::OsString::from).collect())
    }

    fn kept() -> Settings {
        Settings::parse(KEPT).unwrap()
    }

    /// The file `wanted` makes of `text`, and what is said of it.
    fn made(text: &str, automatic: bool, said: &[&str]) -> Result<(String, Plan), String> {
        let now = Settings::parse(text).unwrap();
        let plan = plan(&now, automatic, &wanted(&mut args(said)).map_err(|why| why.message)?)?;
        let text = settings_edit::edited(text, |document| edit(document, &plan))
            .map_err(|why| match why {
                settings_edit::Unedited::Edit(why) => why,
                settings_edit::Unedited::Parse { message, .. } => message,
            })?
            .unwrap_or_else(|| text.to_string());
        Ok((text, plan))
    }

    fn said(plan: &Plan) -> Vec<String> {
        plan.changes.iter().map(|(key, value)| format!("{key} = {value}")).collect()
    }

    #[test]
    fn the_document_says_the_settings_with_their_defaults_filled_in() {
        let held = Held {
            version: "1.0.0-alpha.2".into(),
            why: "fell-back".into(),
            at_ms: 1_794_000_000_000,
            start: 14,
        };
        let document = Document::of(&kept(), Some(&held), true);
        let line = json::line(&document).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/update-settings.json"));
        // The defaults, where the file says nothing.
        let plain = serde_json::to_value(Document::of(&Settings::default(), None, false)).unwrap();
        assert_eq!(plain["check"], "notify");
        assert_eq!(plain["restart"], serde_json::Value::Null);
        assert_eq!(plain["held"], serde_json::Value::Null);
        assert_eq!(plain["apps_disagree"], false);
        assert_eq!(plain["server"], "https://updates.dukovo.org/system/");
    }

    #[test]
    fn a_hold_the_health_check_made_is_told_only_while_that_hold_is_there() {
        let held = Held {
            version: "1.0.0-alpha.2".into(),
            why: "fell-back".into(),
            at_ms: 1_794_000_000_000,
            start: 14,
        };
        let value = |settings: &Settings| {
            serde_json::to_value(Document::of(settings, Some(&held), false)).unwrap()
        };
        assert_eq!(value(&kept())["held"]["why"], "fell-back");
        // Lifted, or held at another version since.
        assert_eq!(value(&Settings::default())["held"], serde_json::Value::Null);
        let mut other = kept();
        other.hold.system = Some("1.0.0-alpha.3".into());
        assert_eq!(value(&other)["held"], serde_json::Value::Null);
    }

    #[test]
    fn apps_and_bundled_that_say_different_things_disagree() {
        let apps = |apps, automatic| {
            let settings = Settings { apps, ..Settings::default() };
            serde_json::to_value(Document::of(&settings, None, automatic)).unwrap()
        };
        assert_eq!(apps(Apps::Auto, true)["apps_disagree"], false);
        assert_eq!(apps(Apps::Notify, false)["apps_disagree"], false);
        assert_eq!(apps(Apps::Auto, false)["apps_disagree"], true);
        assert_eq!(apps(Apps::Notify, true)["apps_disagree"], true);
        assert_eq!(apps(Apps::Notify, true)["apps_automatic"], true);
    }

    #[test]
    fn bundled_s_file_says_whether_everyone_s_apps_update_by_themselves() {
        assert!(says_automatic("everyone\t1793000000\n"));
        assert!(says_automatic("1000\t1793000000\neveryone\t1793000001\n"));
        assert!(!says_automatic("1000\t1793000000\n"));
        assert!(!says_automatic(""));
    }

    #[test]
    fn a_setting_is_written_in_its_place_and_the_comments_stay() {
        let (text, plan) =
            made(KEPT, false, &["--check", "download", "--metered", "download"]).unwrap();
        assert_eq!(said(&plan), ["check = download", "metered = download"]);
        let expected = KEPT
            .replace("check = \"notify\"\n", "check = \"download\"\n")
            .replace("restart = \"03:30\"\n", "restart = \"03:30\"\nmetered = \"download\"\n");
        assert_eq!(text, expected);
        assert!(Settings::parse(&text).is_ok());
    }

    #[test]
    fn what_the_file_says_already_changes_nothing() {
        let (text, plan) = made(KEPT, false, &["--check", "notify", "--unattended", "on"]).unwrap();
        assert!(plan.changes.is_empty());
        assert_eq!(text, KEPT);
        // A default that the file leaves unsaid is not written.
        let (text, plan) =
            made("channel = \"stable\"\n", false, &["--metered", "wait", "--no-restart"]).unwrap();
        assert!(plan.changes.is_empty());
        assert_eq!(text, "channel = \"stable\"\n");
    }

    #[test]
    fn a_restart_time_needs_an_unattended_machine_and_is_a_time() {
        let refused = |text: &str, said: &[&str]| made(text, false, said).unwrap_err();
        assert!(refused("", &["--restart", "03:30"]).contains("unattended"));
        assert!(
            refused(KEPT, &["--unattended", "off", "--restart", "04:00"]).contains("unattended")
        );
        for bad in ["25:00", "3:30", "03:60", "noon", "03:30:00", ""] {
            assert!(refused(KEPT, &["--restart", bad]).contains("HH:MM"), "{bad}");
        }
        // Both together, or with unattended given, are fine.
        let (text, _) = made("", false, &["--unattended", "on", "--restart", "03:30"]).unwrap();
        assert_eq!(text, "unattended = true\nrestart = \"03:30\"\n");
        let (text, _) = made(KEPT, false, &["--restart", "04:15"]).unwrap();
        assert!(text.contains("restart = \"04:15\"\n"));
    }

    #[test]
    fn a_machine_that_is_no_longer_unattended_loses_its_restart_time() {
        let (text, plan) = made(KEPT, false, &["--unattended", "off"]).unwrap();
        assert_eq!(said(&plan), ["unattended = off", "restart = none"]);
        assert!(text.contains("unattended = false\n"));
        assert!(!text.contains("restart"));
        assert!(text.contains("# Versions held back"));
        let (text, plan) = made(KEPT, false, &["--no-restart"]).unwrap();
        assert_eq!(said(&plan), ["restart = none"]);
        assert!(text.contains("unattended = true\n") && !text.contains("restart"));
    }

    #[test]
    fn apps_also_tell_bundled_when_it_does_not_agree() {
        // Both say notify: nothing to change.
        let (_, plan) = made("", false, &["--apps", "notify"]).unwrap();
        assert!(plan.changes.is_empty() && plan.automatic.is_none());
        // The file says auto and bundled does not: only bundled changes.
        let (text, plan) = made("apps = \"auto\"\n", false, &["--apps", "auto"]).unwrap();
        assert_eq!(
            (said(&plan), plan.automatic),
            (["apps = auto".to_string()].to_vec(), Some(true))
        );
        assert_eq!(text, "apps = \"auto\"\n");
        // Both change.
        let (text, plan) = made("", false, &["--apps", "auto"]).unwrap();
        assert_eq!(
            (said(&plan), plan.automatic),
            (["apps = auto".to_string()].to_vec(), Some(true))
        );
        assert_eq!(text, "apps = \"auto\"\n");
        let (text, plan) = made("apps = \"auto\"\n", true, &["--apps", "notify"]).unwrap();
        assert_eq!(plan.automatic, Some(false));
        assert_eq!(text, "apps = \"notify\"\n");
    }

    #[test]
    fn a_command_line_that_asks_for_nothing_or_nonsense_is_a_usage_error() {
        for said in [
            &[][..],
            &["--check", "sometimes"],
            &["--apps", "all"],
            &["--unattended", "yes"],
            &["--metered", "never"],
            &["--restart", "03:00", "--no-restart"],
            &["--no-restart", "--restart", "03:00"],
            &["--channel", "beta"],
            &["check"],
        ] {
            let why = wanted(&mut args(said)).unwrap_err();
            assert!(why.usage, "{said:?}");
        }
    }

    #[test]
    fn the_words_say_each_setting_and_what_needs_a_word_more() {
        let document = Document::of(&Settings::default(), None, true);
        let lines = lines(&document);
        assert_eq!(lines[0], "channel = stable");
        assert!(lines.contains(&"restart = none".to_string()));
        assert!(lines.last().unwrap().contains("`update set --apps notify`"));
        assert_eq!(super::lines(&Document::of(&Settings::default(), None, false)).len(), 8);
    }
}
