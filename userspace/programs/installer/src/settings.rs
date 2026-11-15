// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The new system's settings, as the answers give them, written into its
//! `@settings` and `@state` subvolumes before it first starts: its name
//! (hostname(5)), keyboard layout (keymap(1)), time zone, network cards
//! (network(5)), accounts (ADR 0218), and its consoles' text size and keys
//! and screen reader (ADR 0422). Spark copies every other default setting
//! in at the first boot, and leaves these as they are.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use crate::answers::{Account, Answers, Network};
use crate::machine;

/// Where a system's default settings are, in its slot.
const DEFAULTS: &str = "resources/default-settings";

/// Where a system's time zones are, in its slot, and so in `/system`.
const ZONEINFO: &str = "resources/zoneinfo";

/// The new system's folders, as the installer has them mounted.
pub struct Target<'a> {
    /// Its slot, `@system.a`: what `/system` will be.
    pub slot: &'a Path,
    /// `@settings`: `/system/settings`.
    pub settings: &'a Path,
    /// `@state`: `/system/state`.
    pub state: &'a Path,
}

/// Writes every setting the answers give.
///
/// # Errors
///
/// If a file cannot be written, or an answer names what the system does
/// not have, such as a time zone.
pub fn write(answers: &Answers, target: &Target) -> Result<(), String> {
    write_file(&target.settings.join("hostname"), &format!("{}\n", answers.hostname()), 0o644)?;
    write_file(
        &target.settings.join("keyboard"),
        &format!("layout = {}\n", answers.keyboard),
        0o644,
    )?;
    time_zone(answers.time_zone(), target)?;
    if !answers.network.is_empty() {
        write_file(&target.settings.join("network"), &network(answers), 0o644)?;
    }
    accessibility(answers, target, machine::with_speech())?;
    server_preset(answers, target, machine::under_hypervisor())?;
    accounts(answers, target)
}

/// The server preset's files, in `preset/`: each one says in its own
/// comments what it sets and how to change it, and `ob image --cloud`
/// writes the same ones.
const PRESET_JOBS: &str = include_str!("../preset/jobs.toml");
const PRESET_TUNE: &str = include_str!("../preset/tune.toml");
const PRESET_UPDATES: &str = include_str!("../preset/updates.toml");
const PRESET_CHECK: &str = include_str!("../preset/dukovo-check.toml");

/// What the server preset writes, for a profile that has it: that the
/// jobs of crontab and at run while their owner is signed out (authd(8)),
/// the performance profile `throughput`, or `vm-guest` when the computer
/// `under_hypervisor` (tune(8)), updates that stage and restart at 03:30
/// by themselves (updates.toml(5)), and dukovo-check's daily run turned
/// on explicitly (dukovo-check(8)).
fn server_preset(answers: &Answers, target: &Target, under_hypervisor: bool) -> Result<(), String> {
    if !answers.profile.server_preset() {
        return Ok(());
    }
    write_file(&target.settings.join("jobs.toml"), PRESET_JOBS, 0o644)?;
    let profile = if under_hypervisor { "vm-guest" } else { "throughput" };
    write_file(
        &target.settings.join("tune.toml"),
        &PRESET_TUNE.replace("@PROFILE@", profile),
        0o644,
    )?;
    let sysupdate = target.settings.join("sysupdate");
    make_dir(&sysupdate, 0o755)?;
    write_file(&sysupdate.join("updates.toml"), PRESET_UPDATES, 0o644)?;
    // An override file, which changes the service without a copy of its
    // file, so that a later release's file still reaches the machine.
    for dir in ["spark", "spark/services", "spark/services/settings.d"] {
        make_dir(&target.settings.join(dir), 0o755)?;
    }
    let check = target.settings.join("spark/services/settings.d/dukovo-check.toml");
    write_file(&check, PRESET_CHECK, 0o644)
}

/// What reader.toml(5) holds when the install turns the screen reader on:
/// readerd's voice, rate and pitch are their defaults until `reader`
/// changes them.
const READER_ON: &str = "\
# The screen reader (reader.toml(5)), turned on by the install. reader(1)
# changes it; readerd(8) reads it.
on = true
";

/// The consoles' accessibility.toml(5), if the answers change any of its
/// settings, and reader.toml(5) turned on, if the screen reader starts
/// with the system: as the answers say, or as the installer runs,
/// `with_speech` (ADR 0422).
fn accessibility(answers: &Answers, target: &Target, with_speech: bool) -> Result<(), String> {
    let access = &answers.accessibility;
    if !access.consoles_as_default() {
        let console = target.settings.join("console");
        make_dir(&console, 0o755)?;
        write_file(&console.join("accessibility.toml"), &access.console_file(None), 0o644)?;
    }
    if access.screen_reader(with_speech) {
        let reader = target.state.join("reader");
        make_dir(&reader, 0o755)?;
        write_file(&reader.join("reader.toml"), READER_ON, 0o644)?;
    }
    Ok(())
}

/// Links `localtime` to the zone, as timedatectl(1) does.
fn time_zone(zone: &str, target: &Target) -> Result<(), String> {
    if !target.slot.join(ZONEINFO).join(zone).is_file() {
        return Err(format!(
            "time_zone: the system has no time zone {zone}: timedatectl list-timezones lists them"
        ));
    }
    let link = target.settings.join("localtime");
    symlink(format!("/system/{ZONEINFO}/{zone}"), &link)
        .map_err(|err| format!("{}: {err}", link.display()))
}

/// The network file (network(5)): a section for each card the answers
/// name. A card left out, or given `"dhcp"`, asks DHCP.
fn network(answers: &Answers) -> String {
    let mut text = String::from("# Written by dukovo-install from the install's answers.\n");
    for (card, setup) in &answers.network {
        let _ = write!(text, "\n[{card}]\n");
        match setup {
            Network::Dhcp(_) => text.push_str("dhcp = yes\n"),
            Network::Fixed(fixed) => {
                let _ = writeln!(text, "address = {}", fixed.address);
                if let Some(router) = &fixed.gateway {
                    let _ = writeln!(text, "router = {router}");
                }
                for server in &fixed.dns {
                    let _ = writeln!(text, "dns = {server}");
                }
            }
        }
    }
    text
}

/// The accounts: the administrator and the other people, added to the
/// default `passwd` and `group` as authd(8) adds them, and their
/// passwords' hashes in `authd/passwords`, which only authd reads.
fn accounts(answers: &Answers, target: &Target) -> Result<(), String> {
    let read = |name: &str| {
        let path = target.slot.join(DEFAULTS).join(name);
        fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))
    };
    let mut users = accounts::users(&read("passwd")?);
    let mut groups = accounts::groups(&read("group")?);
    let mut passwords = accounts::passwords("");
    let all = std::iter::once((&answers.admin, true))
        .chain(answers.users.iter().map(|user| (user, false)));
    for (account, admin) in all {
        let Account { name, user, password_hash } = account;
        accounts::add(&mut users, &mut groups, user, name, admin, &[])
            .map_err(|why| format!("{user}: {why}"))?;
        let hash = password_hash.clone().unwrap_or_default();
        passwords.push(accounts::Password { name: user.clone(), hash });
    }
    write_file(&target.settings.join("passwd"), &users.text(), 0o644)?;
    write_file(&target.settings.join("group"), &groups.text(), 0o644)?;
    let authd = target.state.join("authd");
    make_dir(&authd, 0o700)?;
    write_file(&authd.join("passwords"), &passwords.text(), 0o600)
}

/// Makes the directory `path`, if it is not there, with `mode`.
///
/// # Errors
///
/// If it cannot be made.
pub fn make_dir(path: &Path, mode: u32) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|err| format!("making {}: {err}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|err| format!("setting the mode of {}: {err}", path.display()))
}

/// Writes `text` to the new file `path`, with `mode`.
///
/// # Errors
///
/// If it cannot be written.
pub fn write_file(path: &Path, text: &str, mode: u32) -> Result<(), String> {
    fs::write(path, text).map_err(|err| format!("writing {}: {err}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|err| format!("setting the mode of {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::answers::tests::EXAMPLE;

    #[test]
    fn writes_the_settings_and_accounts() {
        let dir = std::env::temp_dir().join(format!("installer-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for sub in [
            "slot/resources/default-settings",
            "slot/resources/zoneinfo/America",
            "settings",
            "state",
        ] {
            fs::create_dir_all(dir.join(sub)).unwrap();
        }
        fs::write(
            dir.join("slot/resources/default-settings/passwd"),
            "root:x:0:0:root:/users/root:/system/commands/nukesh\n",
        )
        .unwrap();
        fs::write(dir.join("slot/resources/default-settings/group"), "root:x:0:\n").unwrap();
        fs::write(dir.join("slot/resources/zoneinfo/America/Denver"), "TZif").unwrap();
        let target = Target {
            slot: &dir.join("slot"),
            settings: &dir.join("settings"),
            state: &dir.join("state"),
        };
        let answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        write(&answers, &target).unwrap();
        let read = |p: &str| fs::read_to_string(dir.join(p)).unwrap();
        assert_eq!(read("settings/hostname"), "web01\n");
        assert_eq!(read("settings/keyboard"), "layout = us\n");
        assert_eq!(
            fs::read_link(dir.join("settings/localtime")).unwrap(),
            Path::new("/system/resources/zoneinfo/America/Denver")
        );
        assert!(
            read("settings/network")
                .contains("[net0]\naddress = 10.0.0.20/24\nrouter = 10.0.0.1\ndns = 10.0.0.1\n")
        );
        assert!(read("settings/passwd").contains("alice:x:1000:1000:Alice Liddell:/users/alice:"));
        assert!(read("settings/passwd").contains("bob:x:1001:1001:Bob:"));
        assert!(read("settings/group").contains("admins:x:10:alice"));
        assert_eq!(read("state/authd/passwords"), "alice::\nbob::\n");
        let mode = fs::metadata(dir.join("state/authd/passwords")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_the_consoles_settings_and_turns_the_reader_on_when_asked() {
        let dir = std::env::temp_dir().join(format!("installer-access-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["settings", "state"] {
            fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let target =
            Target { slot: &dir, settings: &dir.join("settings"), state: &dir.join("state") };
        let mut answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        // The defaults write nothing: Spark copies the default file in.
        accessibility(&answers, &target, false).unwrap();
        assert!(!dir.join("settings/console").exists());
        assert!(!dir.join("state/reader").exists());
        // An installer that runs with speech turns the reader on.
        accessibility(&answers, &target, true).unwrap();
        let reader = fs::read_to_string(dir.join("state/reader/reader.toml")).unwrap();
        assert!(reader.ends_with("on = true\n"), "{reader}");
        fs::remove_dir_all(dir.join("state/reader")).unwrap();
        answers.accessibility.screen_reader = Some(false);
        answers.accessibility.text_size = crate::answers::TextSize::Largest;
        accessibility(&answers, &target, true).unwrap();
        assert!(!dir.join("state/reader").exists());
        let console = fs::read_to_string(dir.join("settings/console/accessibility.toml")).unwrap();
        assert!(console.contains("text-size = \"largest\"\n"), "{console}");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A target with the folders the preset writes into, and its answers
    /// for `profile`.
    fn preset_target(name: &str, profile: &str) -> (PathBuf, Answers) {
        let dir = std::env::temp_dir().join(format!("installer-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("settings")).unwrap();
        let text = EXAMPLE
            .replace("\"version\": 1,", &format!("\"version\": 1, \"profile\": \"{profile}\","));
        (dir, Answers::parse(text.as_bytes()).unwrap())
    }

    /// The settings `file` holds, without its comments, as a table.
    fn table(dir: &Path, file: &str) -> toml::Table {
        let text = fs::read_to_string(dir.join("settings").join(file)).unwrap();
        toml::from_str(&text).unwrap()
    }

    fn mode_of(dir: &Path, file: &str) -> u32 {
        fs::metadata(dir.join("settings").join(file)).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_server_profile_lets_jobs_run_signed_out() {
        let (dir, answers) = preset_target("preset-jobs", "server");
        let target = Target { slot: &dir, settings: &dir.join("settings"), state: &dir };
        server_preset(&answers, &target, false).unwrap();
        let jobs = table(&dir, "jobs.toml");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs["keep_running"].as_bool(), Some(true));
        assert_eq!(mode_of(&dir, "jobs.toml"), 0o644);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_server_profile_tunes_for_throughput_and_a_virtual_machine_for_its_guest() {
        let (dir, answers) = preset_target("preset-tune", "server");
        let target = Target { slot: &dir, settings: &dir.join("settings"), state: &dir };
        server_preset(&answers, &target, false).unwrap();
        let tune = table(&dir, "tune.toml");
        assert_eq!(tune.len(), 1);
        assert_eq!(tune["profile"].as_str(), Some("throughput"));
        server_preset(&answers, &target, true).unwrap();
        assert_eq!(table(&dir, "tune.toml")["profile"].as_str(), Some("vm-guest"));
        assert_eq!(mode_of(&dir, "tune.toml"), 0o644);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_server_profile_stages_updates_and_restarts_into_them_at_night() {
        let (dir, answers) = preset_target("preset-updates", "server");
        let target = Target { slot: &dir, settings: &dir.join("settings"), state: &dir };
        server_preset(&answers, &target, false).unwrap();
        let updates = table(&dir, "sysupdate/updates.toml");
        assert_eq!(updates.len(), 2);
        assert_eq!(updates["unattended"].as_bool(), Some(true));
        assert_eq!(updates["restart"].as_str(), Some("03:30"));
        assert_eq!(mode_of(&dir, "sysupdate/updates.toml"), 0o644);
        assert_eq!(mode_of(&dir, "sysupdate"), 0o755);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_server_profile_turns_the_daily_check_on_with_an_override() {
        let (dir, answers) = preset_target("preset-check", "server");
        let target = Target { slot: &dir, settings: &dir.join("settings"), state: &dir };
        server_preset(&answers, &target, false).unwrap();
        let check = table(&dir, "spark/services/settings.d/dukovo-check.toml");
        assert_eq!(check.len(), 1);
        assert_eq!(check["enabled"].as_bool(), Some(true));
        assert_eq!(mode_of(&dir, "spark/services/settings.d/dukovo-check.toml"), 0o644);
        // Nothing but the override: the service's own file is Spark's to copy.
        assert!(!dir.join("settings/spark/services/dukovo-check.toml").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_cloud_profile_has_the_preset_and_the_minimal_one_has_none() {
        let (dir, cloud) = preset_target("preset-cloud", "cloud");
        let target = Target { slot: &dir, settings: &dir.join("settings"), state: &dir };
        server_preset(&cloud, &target, true).unwrap();
        assert!(dir.join("settings/jobs.toml").is_file());
        assert!(dir.join("settings/tune.toml").is_file());
        assert!(dir.join("settings/sysupdate/updates.toml").is_file());
        fs::remove_dir_all(&dir).unwrap();

        let (dir, minimal) = preset_target("preset-minimal", "minimal");
        let target = Target { slot: &dir, settings: &dir.join("settings"), state: &dir };
        server_preset(&minimal, &target, false).unwrap();
        assert_eq!(fs::read_dir(dir.join("settings")).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_file_of_the_preset_says_how_to_change_it() {
        for (name, text) in [
            ("jobs.toml", PRESET_JOBS),
            ("tune.toml", PRESET_TUNE),
            ("updates.toml", PRESET_UPDATES),
            ("dukovo-check.toml", PRESET_CHECK),
        ] {
            let comments = text.lines().filter(|line| line.starts_with('#')).count();
            assert!(comments >= 5, "{name} has {comments} lines of comment");
            assert!(text.starts_with("# SPDX-FileCopyrightText:"), "{name}");
        }
    }

    #[test]
    fn refuses_a_zone_the_system_lacks() {
        let dir = std::env::temp_dir().join(format!("installer-zone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("settings")).unwrap();
        let target =
            Target { slot: &dir.join("slot"), settings: &dir.join("settings"), state: &dir };
        let why = time_zone("Mars/Olympus", &target).unwrap_err();
        assert!(why.contains("timedatectl list-timezones"), "{why}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
