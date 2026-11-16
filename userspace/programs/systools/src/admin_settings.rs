// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's settings, `/system/settings/admin.toml`
//! (admin.toml(5), ADRs 0514 and 0517): the sign-in rules, which authd(8)
//! keeps, where admind(8) answers and whom, and who may open a terminal
//! in the console, which authd checks at each request for one.
//!
//! A file that is not there gives the defaults; a key that is not there
//! gives its default; a key with a value out of its range is refused, so
//! that a typo never loosens a rule unnoticed.
//!
//! `admin settings [--json]` says what the file gives, as a list or as JSON
//! for the Security page ([`settings`]), and `admin sign-in` says or
//! changes the `[sign-in]` rules ([`sign_in`]), through [`settings_edit`]
//! as `admin reach` changes `[console]`, so that every comment of the file
//! stays. A change is root's, as the file is, and has Spark reload admind,
//! so that the console's sessions follow the rules at once; authd reads
//! them at each sign-in.
//!
//! When a rule does not apply, `admin settings` says whether the file
//! reads, and `log admind` and `log --audit` say what the console and authd
//! did with it.

use std::ops::RangeInclusive;
use std::path::Path;

use serde::{Deserialize, Serialize};
use toml_edit::DocumentMut;

use crate::args::{Arg, Args};
use crate::service;
use crate::settings_edit::{self, Changed};
use crate::util::{self, Fail, fail, usage};

/// The file.
pub const PATH: &str = "/system/settings/admin.toml";

/// The mode of the file when it is made, as the file that ships has it:
/// every account reads it, and root writes it.
const MODE: u32 = 0o644;

/// The hours a session may last after its sign-in (`session-hours`).
pub const SESSION_HOURS: RangeInclusive<u32> = 1..=168;

/// The minutes a session may last with no console page open
/// (`idle-minutes`).
pub const IDLE_MINUTES: RangeInclusive<u32> = 5..=1440;

/// Who must give a second step at sign-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondStep {
    /// Those who have added one.
    #[default]
    IfSet,
    /// Administrators, and those who have added one.
    Administrators,
    /// Everyone.
    Everyone,
}

impl SecondStep {
    /// The word admin.toml and the console's API write it with.
    pub fn word(self) -> &'static str {
        match self {
            SecondStep::IfSet => "if-set",
            SecondStep::Administrators => "administrators",
            SecondStep::Everyone => "everyone",
        }
    }

    /// The second step that `word` names.
    pub fn parse(word: &str) -> Option<SecondStep> {
        [SecondStep::IfSet, SecondStep::Administrators, SecondStep::Everyone]
            .into_iter()
            .find(|step| step.word() == word)
    }
}

/// Whom admind answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    /// Machines on private networks: this one, its own networks, and the
    /// private, shared and local address ranges.
    #[default]
    Private,
    /// Any machine.
    Any,
}

impl Reach {
    /// The word admin.toml and the console's API write it with.
    pub fn word(self) -> &'static str {
        match self {
            Reach::Private => "private",
            Reach::Any => "any",
        }
    }

    /// The reach that `word` names.
    pub fn parse(word: &str) -> Option<Reach> {
        [Reach::Private, Reach::Any].into_iter().find(|reach| reach.word() == word)
    }
}

/// The `[sign-in]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SignIn {
    /// Who gives a second step.
    pub second_step: SecondStep,
    /// The hours a session lasts after its sign-in, 1 to 168.
    pub session_hours: u32,
    /// The minutes a session lasts with no console page open, 5 to 1440.
    pub idle_minutes: u32,
}

impl Default for SignIn {
    fn default() -> SignIn {
        SignIn { second_step: SecondStep::IfSet, session_hours: 12, idle_minutes: 30 }
    }
}

/// The `[console]` table.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Console {
    /// The port admind answers on: 443, or 1024 to 65535.
    pub listen: u16,
    /// Whom it answers.
    pub reach: Reach,
    /// The certificate it serves, by certd's name for it.
    pub certificate: String,
}

impl Default for Console {
    fn default() -> Console {
        Console { listen: 9090, reach: Reach::Private, certificate: "this-machine".into() }
    }
}

/// Who may open a terminal in the console.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Allowed {
    /// Each account that may use the console and has a shell, as at the
    /// machine's screen and over SSH.
    #[default]
    Everyone,
    /// Administrators.
    Administrators,
    /// No one.
    Nobody,
}

impl Allowed {
    /// Whether it lets an account open a terminal: an administrator's
    /// when `admin`.
    pub fn lets(self, admin: bool) -> bool {
        match self {
            Allowed::Everyone => true,
            Allowed::Administrators => admin,
            Allowed::Nobody => false,
        }
    }

    /// The word admin.toml writes it with.
    pub fn word(self) -> &'static str {
        match self {
            Allowed::Everyone => "everyone",
            Allowed::Administrators => "administrators",
            Allowed::Nobody => "nobody",
        }
    }
}

/// The most terminals an account may keep, which `sessions-per-account`
/// may lower.
pub const TERMINALS_MAX: u32 = 64;

/// The `[terminal]` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Terminal {
    /// Who may open one.
    pub allowed: Allowed,
    /// How many terminals each account may keep, 1 to [`TERMINALS_MAX`].
    pub sessions_per_account: u32,
}

impl Default for Terminal {
    fn default() -> Terminal {
        Terminal { allowed: Allowed::Everyone, sessions_per_account: 32 }
    }
}

/// The whole file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Settings {
    /// The sign-in rules.
    pub sign_in: SignIn,
    /// The console's server.
    pub console: Console,
    /// The console's terminal.
    pub terminal: Terminal,
}

/// What the range of the setting `name` is, in words.
fn bounds(name: &str, range: &RangeInclusive<u32>) -> String {
    format!("{name} is {} to {}", range.start(), range.end())
}

impl Settings {
    /// The settings in `text`, checked.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let settings: Settings =
            toml::from_str(text).map_err(|err| format!("{PATH}: {}", err.message()))?;
        let sign_in = &settings.sign_in;
        if !SESSION_HOURS.contains(&sign_in.session_hours) {
            return Err(format!("{PATH}: {}", bounds("session-hours", &SESSION_HOURS)));
        }
        if !IDLE_MINUTES.contains(&sign_in.idle_minutes) {
            return Err(format!("{PATH}: {}", bounds("idle-minutes", &IDLE_MINUTES)));
        }
        let port = settings.console.listen;
        if port != 443 && port < 1024 {
            return Err(format!("{PATH}: listen is 443, or 1024 to 65535"));
        }
        if let Err(err) = manifest::service::check_certificate(&settings.console.certificate) {
            return Err(format!("{PATH}: certificate: {err}"));
        }
        if !(1..=TERMINALS_MAX).contains(&settings.terminal.sessions_per_account) {
            return Err(format!("{PATH}: sessions-per-account is 1 to {TERMINALS_MAX}"));
        }
        Ok(settings)
    }

    /// The settings in [`PATH`], or the defaults when it is not there.
    pub fn read() -> Result<Settings, String> {
        Settings::read_at(Path::new(PATH))
    }

    /// The settings in the file `path`, which a test keeps elsewhere than
    /// [`PATH`], or the defaults when it is not there.
    pub fn read_at(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(err) => Err(format!("cannot read {}: {err}", path.display())),
        }
    }
}

/// What `admin settings --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Report {
    /// The shape of this object: 1.
    format: u8,
    sign_in: SignInReport,
    console: ConsoleReport,
    terminal: TerminalReport,
    /// Why the file does not read, null when it does; the settings are
    /// then the defaults.
    wrong: Option<String>,
}

/// The `[sign-in]` table in the report.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct SignInReport {
    second_step: &'static str,
    session_hours: u32,
    idle_minutes: u32,
}

/// The `[console]` table in the report.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct ConsoleReport {
    listen: u16,
    reach: &'static str,
    certificate: String,
}

/// The `[terminal]` table in the report.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct TerminalReport {
    allowed: &'static str,
    sessions_per_account: u32,
}

/// What the file says, or the defaults and why it does not read.
fn report(read: Result<Settings, String>) -> Report {
    let (settings, wrong) = match read {
        Ok(settings) => (settings, None),
        Err(wrong) => (Settings::default(), Some(wrong)),
    };
    Report {
        format: 1,
        sign_in: SignInReport {
            second_step: settings.sign_in.second_step.word(),
            session_hours: settings.sign_in.session_hours,
            idle_minutes: settings.sign_in.idle_minutes,
        },
        console: ConsoleReport {
            listen: settings.console.listen,
            reach: settings.console.reach.word(),
            certificate: settings.console.certificate,
        },
        terminal: TerminalReport {
            allowed: settings.terminal.allowed.word(),
            sessions_per_account: settings.terminal.sessions_per_account,
        },
        wrong,
    }
}

/// The settings of `report` as the list a person reads, and the reason the
/// file does not read when it does not.
fn text(report: &Report) -> String {
    let rows = [
        ("sign-in.second-step", report.sign_in.second_step.to_string()),
        ("sign-in.session-hours", report.sign_in.session_hours.to_string()),
        ("sign-in.idle-minutes", report.sign_in.idle_minutes.to_string()),
        ("console.listen", report.console.listen.to_string()),
        ("console.reach", report.console.reach.to_string()),
        ("console.certificate", report.console.certificate.clone()),
        ("terminal.allowed", report.terminal.allowed.to_string()),
        ("terminal.sessions-per-account", report.terminal.sessions_per_account.to_string()),
    ];
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut shown = String::new();
    for (name, value) in &rows {
        shown += &format!("{name:<width$}  {value}\n");
    }
    if let Some(wrong) = &report.wrong {
        shown += &format!("{wrong}\nThe file does not read, so the defaults are shown.\n");
    }
    shown
}

/// `admin settings [--json]`: what admin.toml says, to anyone.
pub fn settings(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let report = report(Settings::read());
    if json {
        let line = serde_json::to_string(&report).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
    } else {
        print!("{}", text(&report));
    }
    Ok(0)
}

/// What `admin sign-in` is asked to set: each rule is left as it is when
/// none.
#[derive(Debug, Default, PartialEq, Eq)]
struct Rules {
    second_step: Option<SecondStep>,
    session_hours: Option<u32>,
    idle_minutes: Option<u32>,
}

/// Puts `value` in `slot`, which `option` has not filled before.
fn once<T>(slot: &mut Option<T>, option: &str, value: T) -> Result<(), Fail> {
    if slot.replace(value).is_some() {
        return Err(usage(format!("{option} is given twice")));
    }
    Ok(())
}

/// `value`, which `name` takes when it is in `range`.
fn within(name: &str, range: &RangeInclusive<u32>, value: u32) -> Result<u32, Fail> {
    if range.contains(&value) { Ok(value) } else { Err(usage(bounds(name, range))) }
}

/// The rules the options of `admin sign-in` ask to set.
fn rules_asked(args: &mut Args) -> Result<Rules, Fail> {
    let mut rules = Rules::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "second-step" => {
                let word = args.text()?;
                let step = SecondStep::parse(&word).ok_or_else(|| {
                    usage(format!(
                        "{word} is not a second step: if-set, administrators or everyone"
                    ))
                })?;
                once(&mut rules.second_step, "--second-step", step)?;
            }
            Arg::Long(option) if option == "session-hours" => {
                let hours = within("session-hours", &SESSION_HOURS, args.number()?)?;
                once(&mut rules.session_hours, "--session-hours", hours)?;
            }
            Arg::Long(option) if option == "idle-minutes" => {
                let minutes = within("idle-minutes", &IDLE_MINUTES, args.number()?)?;
                once(&mut rules.idle_minutes, "--idle-minutes", minutes)?;
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(rules)
}

/// Whether the file's `current` rules say what `rules` ask already.
fn says_already(current: &SignIn, rules: &Rules) -> bool {
    rules.second_step.is_none_or(|step| step == current.second_step)
        && rules.session_hours.is_none_or(|hours| hours == current.session_hours)
        && rules.idle_minutes.is_none_or(|minutes| minutes == current.idle_minutes)
}

/// Makes the `[sign-in]` rules of the admin.toml at `path` what `rules`
/// ask, with every comment of the file kept, and nothing written when it
/// says so already; gives whether it wrote. A file that admind would refuse
/// is left as it is, and what is wrong with it told, for the person to
/// mend first.
fn set_rules(path: &Path, rules: &Rules) -> Result<bool, Fail> {
    if says_already(&Settings::read_at(path).map_err(fail)?.sign_in, rules) {
        return Ok(false);
    }
    let edit = |document: &mut DocumentMut| {
        let table = settings_edit::table(document, "sign-in")
            .ok_or_else(|| format!("{PATH}: sign-in is not a table"))?;
        if let Some(step) = rules.second_step {
            settings_edit::set_string(table, "second-step", step.word());
        }
        if let Some(hours) = rules.session_hours {
            settings_edit::set_integer(table, "session-hours", i64::from(hours));
        }
        if let Some(minutes) = rules.idle_minutes {
            settings_edit::set_integer(table, "idle-minutes", i64::from(minutes));
        }
        Ok(())
    };
    match settings_edit::change(path, MODE, edit) {
        Ok(changed) => Ok(changed),
        // The edit's words name the file already.
        Err(Changed::Edit { error, .. }) => Err(fail(error)),
        Err(other) => Err(fail(other.to_string())),
    }
}

/// The rules as the list `admin sign-in` prints.
fn rules_text(sign_in: &SignIn) -> String {
    format!(
        "second-step    {}\nsession-hours  {}\nidle-minutes   {}\n",
        sign_in.second_step.word(),
        sign_in.session_hours,
        sign_in.idle_minutes
    )
}

/// `admin sign-in [--second-step WORD] [--session-hours N] [--idle-minutes
/// N]`: says the sign-in rules, or sets those given, as root.
pub fn sign_in(args: &mut Args) -> Result<i32, Fail> {
    let rules = rules_asked(args)?;
    let path = Path::new(PATH);
    if rules == Rules::default() {
        print!("{}", rules_text(&Settings::read_at(path).map_err(fail)?.sign_in));
        return Ok(0);
    }
    if util::own_user() != 0 {
        return Err(fail(
            "only root may change the sign-in rules: run it with elevate, as `elevate admin \
             sign-in ...`"
                .into(),
        ));
    }
    // ADM-138: the trial `console` covers this file, and its undo would
    // put back the rules as they were before both changes.
    crate::admin::console_not_tried(None)?;
    if !set_rules(path, &rules)? {
        println!("The sign-in rules say so already.");
        return Ok(0);
    }
    // Spark signals admind, which reads the file again; it would within 30
    // seconds anyway, and when it starts. authd reads it at each sign-in.
    service::ask("reload admind").map_err(|why| {
        fail(format!(
            "{PATH} says so now, but admind was not told to read it again ({}): it does within \
             30 seconds, and when it starts",
            why.message
        ))
    })?;
    println!(
        "The sign-in rules are changed: admind has read them again, and authd reads them at each sign-in."
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_gives_the_defaults() {
        let settings = Settings::parse("").unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.sign_in.second_step, SecondStep::IfSet);
        assert_eq!((settings.sign_in.session_hours, settings.sign_in.idle_minutes), (12, 30));
        assert_eq!(settings.console.listen, 9090);
        assert_eq!(settings.console.reach, Reach::Private);
        assert_eq!(settings.console.certificate, "this-machine");
        assert_eq!(settings.terminal.allowed, Allowed::Everyone);
        assert_eq!(settings.terminal.sessions_per_account, 32);
    }

    #[test]
    fn who_may_open_a_terminal_is_the_word_the_file_gives() {
        let cases = [
            (Allowed::Everyone, [true, true]),
            (Allowed::Administrators, [false, true]),
            (Allowed::Nobody, [false, false]),
        ];
        for (allowed, [person, admin]) in cases {
            let text = format!("[terminal]\nallowed = \"{}\"\n", allowed.word());
            assert_eq!(Settings::parse(&text).unwrap().terminal.allowed, allowed);
            assert_eq!((allowed.lets(false), allowed.lets(true)), (person, admin), "{allowed:?}");
        }
        let fewest = Settings::parse("[terminal]\nsessions-per-account = 1\n").unwrap();
        assert_eq!(fewest.terminal.sessions_per_account, 1);
        let most = format!("[terminal]\nsessions-per-account = {TERMINALS_MAX}\n");
        assert_eq!(Settings::parse(&most).unwrap().terminal.sessions_per_account, TERMINALS_MAX);
    }

    #[test]
    fn a_reach_is_named_by_the_word_the_file_reads_it_by() {
        for reach in [Reach::Private, Reach::Any] {
            let text = format!("[console]\nreach = \"{}\"\n", reach.word());
            assert_eq!(Settings::parse(&text).unwrap().console.reach, reach);
            assert_eq!(Reach::parse(reach.word()), Some(reach));
        }
        for word in ["", "Any", "public", "private ", "everyone"] {
            assert_eq!(Reach::parse(word), None, "{word:?}");
        }
    }

    #[test]
    fn a_file_that_is_not_there_gives_the_defaults_and_one_that_is_gives_its_settings() {
        let dir = std::env::temp_dir().join(format!("admin-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("admin.toml");
        assert_eq!(Settings::read_at(&path).unwrap(), Settings::default());
        std::fs::write(&path, "[console]\nreach = \"any\"\n").unwrap();
        assert_eq!(Settings::read_at(&path).unwrap().console.reach, Reach::Any);
        let unreadable = Settings::read_at(&dir).unwrap_err();
        assert!(
            unreadable.starts_with(&format!("cannot read {}: ", dir.display())),
            "{unreadable}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_key_reads() {
        let settings = Settings::parse(
            "[sign-in]\nsecond-step = \"administrators\"\nsession-hours = 1\nidle-minutes = 1440\n\
             [console]\nlisten = 443\nreach = \"any\"\ncertificate = \"admin.example.org\"\n",
        )
        .unwrap();
        assert_eq!(settings.sign_in.second_step, SecondStep::Administrators);
        assert_eq!((settings.sign_in.session_hours, settings.sign_in.idle_minutes), (1, 1440));
        assert_eq!(settings.console.listen, 443);
        assert_eq!(settings.console.reach, Reach::Any);
        assert_eq!(settings.console.certificate, "admin.example.org");
        let everyone = Settings::parse("[sign-in]\nsecond-step = \"everyone\"\n").unwrap();
        assert_eq!(everyone.sign_in.second_step, SecondStep::Everyone);
    }

    /// A folder of its own for each call, as the tests run at once.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new() -> Folder {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("sign-in-{}-{call}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Folder(dir)
        }

        fn file(&self) -> std::path::PathBuf {
            self.0.join("admin.toml")
        }

        fn text(&self) -> String {
            std::fs::read_to_string(self.file()).unwrap()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn args(items: &[&str]) -> Args {
        Args::new("admin", items.iter().map(Into::into).collect())
    }

    fn json_of(read: Result<Settings, String>) -> serde_json::Value {
        serde_json::to_value(report(read)).unwrap()
    }

    #[test]
    fn the_json_has_the_fields_the_page_reads() {
        let settings = Settings::parse(
            "[sign-in]\nsecond-step = \"administrators\"\nsession-hours = 8\n\
             [console]\nreach = \"any\"\ncertificate = \"admin.example.org\"\n\
             [terminal]\nallowed = \"nobody\"\nsessions-per-account = 4\n",
        )
        .unwrap();
        let golden: serde_json::Value =
            serde_json::from_str(include_str!("admin_settings/settings.golden.json")).unwrap();
        assert_eq!(json_of(Ok(settings)), golden);
    }

    #[test]
    fn the_fields_come_in_the_order_the_manual_gives() {
        let line = serde_json::to_string(&report(Ok(Settings::default()))).unwrap();
        assert_eq!(
            line,
            "{\"format\":1,\"sign_in\":{\"second_step\":\"if-set\",\"session_hours\":12,\
             \"idle_minutes\":30},\"console\":{\"listen\":9090,\"reach\":\"private\",\
             \"certificate\":\"this-machine\"},\"terminal\":{\"allowed\":\"everyone\",\
             \"sessions_per_account\":32},\"wrong\":null}"
        );
    }

    #[test]
    fn a_file_that_does_not_read_shows_the_defaults_and_why() {
        let shown = json_of(Err("/system/settings/admin.toml: session-hours is 1 to 168".into()));
        assert_eq!(shown["wrong"], "/system/settings/admin.toml: session-hours is 1 to 168");
        assert_eq!(shown["sign_in"]["session_hours"], 12);
        assert_eq!(shown["console"]["reach"], "private");
        let shown = text(&report(Err("it is wrong".into())));
        assert!(shown.starts_with("sign-in.second-step"), "{shown}");
        assert!(
            shown.ends_with("it is wrong\nThe file does not read, so the defaults are shown.\n")
        );
    }

    #[test]
    fn the_list_says_each_setting() {
        let shown = text(&report(Ok(Settings::default())));
        for line in [
            "sign-in.second-step            if-set\n",
            "sign-in.session-hours          12\n",
            "sign-in.idle-minutes           30\n",
            "console.listen                 9090\n",
            "console.reach                  private\n",
            "console.certificate            this-machine\n",
            "terminal.allowed               everyone\n",
            "terminal.sessions-per-account  32\n",
        ] {
            assert!(shown.contains(line), "{line:?} in {shown}");
        }
    }

    #[test]
    fn a_second_step_is_named_by_the_word_the_file_reads_it_by() {
        for step in [SecondStep::IfSet, SecondStep::Administrators, SecondStep::Everyone] {
            let text = format!("[sign-in]\nsecond-step = \"{}\"\n", step.word());
            assert_eq!(Settings::parse(&text).unwrap().sign_in.second_step, step);
            assert_eq!(SecondStep::parse(step.word()), Some(step));
        }
        for word in ["", "If-Set", "ifset", "admins", "never"] {
            assert_eq!(SecondStep::parse(word), None, "{word:?}");
        }
    }

    #[test]
    fn the_options_name_the_rules_to_set_and_each_is_taken_once() {
        let rules = rules_asked(&mut args(&[
            "--second-step",
            "everyone",
            "--session-hours=168",
            "--idle-minutes",
            "5",
        ]))
        .unwrap();
        let want = Rules {
            second_step: Some(SecondStep::Everyone),
            session_hours: Some(168),
            idle_minutes: Some(5),
        };
        assert_eq!(rules, want);
        assert_eq!(rules_asked(&mut args(&[])).unwrap(), Rules::default());
        let rules = rules_asked(&mut args(&["--session-hours", "1"])).unwrap();
        assert_eq!(rules, Rules { session_hours: Some(1), ..Rules::default() });
    }

    #[test]
    fn a_rule_out_of_its_bounds_or_a_word_that_is_none_is_a_mistake_in_the_command_line() {
        for (items, said) in [
            (&["--session-hours", "0"][..], "session-hours is 1 to 168"),
            (&["--session-hours", "169"], "session-hours is 1 to 168"),
            (&["--idle-minutes", "4"], "idle-minutes is 5 to 1440"),
            (&["--idle-minutes", "1441"], "idle-minutes is 5 to 1440"),
            (&["--idle-minutes", "soon"], "not a valid number"),
            (&["--idle-minutes", "-5"], "not a valid number"),
            (&["--second-step", "never"], "never is not a second step"),
            (&["--second-step", "If-Set"], "If-Set is not a second step"),
            (&["--second-step", "everyone", "--second-step", "if-set"], "given twice"),
            (&["--session-hours"], "needs a value"),
        ] {
            let refused = rules_asked(&mut args(items)).unwrap_err();
            assert!(refused.usage, "{items:?}");
            assert!(refused.message.contains(said), "{items:?}: {}", refused.message);
        }
        for items in [&["--reach", "any"][..], &["private"], &["--json"]] {
            assert!(rules_asked(&mut args(items)).unwrap_err().usage, "{items:?}");
        }
    }

    #[test]
    fn rules_that_are_set_change_their_values_and_no_other_line_of_the_file() {
        let folder = Folder::new();
        let before = "# Mine.\n\n[sign-in]\nsecond-step = \"if-set\" # as shipped\n\
                      session-hours = 8 # a working day\n\n[console]\nlisten = 9090\n";
        std::fs::write(folder.file(), before).unwrap();
        let rules = Rules {
            second_step: Some(SecondStep::Administrators),
            session_hours: Some(10),
            idle_minutes: Some(60),
        };
        assert!(set_rules(&folder.file(), &rules).unwrap());
        assert_eq!(
            folder.text(),
            "# Mine.\n\n[sign-in]\nsecond-step = \"administrators\" # as shipped\n\
             session-hours = 10 # a working day\nidle-minutes = 60\n\n[console]\nlisten = 9090\n"
        );
        // Only what is given changes.
        let rules = Rules { idle_minutes: Some(5), ..Rules::default() };
        assert!(set_rules(&folder.file(), &rules).unwrap());
        assert!(folder.text().contains("session-hours = 10 # a working day\nidle-minutes = 5\n"));
    }

    #[test]
    fn rules_that_are_set_already_write_nothing_and_a_file_that_is_not_there_is_made_when_it_must()
    {
        use std::os::unix::fs::MetadataExt;

        let folder = Folder::new();
        // The defaults are what a file that is not there says.
        let defaults = Rules {
            second_step: Some(SecondStep::IfSet),
            session_hours: Some(12),
            idle_minutes: Some(30),
        };
        assert!(!set_rules(&folder.file(), &defaults).unwrap());
        assert!(!folder.file().exists());
        std::fs::write(folder.file(), "[sign-in]\nsession-hours = 8 # single\n").unwrap();
        let ino = std::fs::metadata(folder.file()).unwrap().ino();
        let eight = Rules { session_hours: Some(8), ..Rules::default() };
        assert!(!set_rules(&folder.file(), &eight).unwrap());
        assert_eq!(std::fs::metadata(folder.file()).unwrap().ino(), ino);
        std::fs::remove_file(folder.file()).unwrap();
        let everyone = Rules { second_step: Some(SecondStep::Everyone), ..Rules::default() };
        assert!(set_rules(&folder.file(), &everyone).unwrap());
        assert_eq!(folder.text(), "[sign-in]\nsecond-step = \"everyone\"\n");
        assert_eq!(std::fs::metadata(folder.file()).unwrap().mode() & 0o7777, MODE);
    }

    #[test]
    fn a_file_admind_would_refuse_is_left_as_it_is_and_said() {
        let folder = Folder::new();
        let rules = Rules { session_hours: Some(10), ..Rules::default() };
        for (text, said) in [
            ("[sign-in]\nsession-hours = 500\n", "session-hours is 1 to 168"),
            ("[console]\nlisten = 80\n", "listen is 443, or 1024 to 65535"),
            ("[sign-in]\nsession-hour = 8\n", "unknown field `session-hour`"),
            ("sign-in = 5\n", "invalid type: integer `5`"),
            ("[sign-in\n", "unclosed table"),
        ] {
            std::fs::write(folder.file(), text).unwrap();
            let refused = set_rules(&folder.file(), &rules).unwrap_err();
            assert!(!refused.usage);
            assert!(
                refused.message.starts_with("/system/settings/admin.toml: "),
                "{text:?}: {}",
                refused.message
            );
            assert!(refused.message.contains(said), "{text:?}: {}", refused.message);
            assert_eq!(folder.text(), text);
        }
    }

    #[test]
    fn the_rules_a_person_reads_are_the_ones_the_file_gives() {
        let current = Settings::parse("[sign-in]\nsecond-step = \"everyone\"\n").unwrap().sign_in;
        assert_eq!(
            rules_text(&current),
            "second-step    everyone\nsession-hours  12\nidle-minutes   30\n"
        );
        assert!(says_already(&current, &Rules::default()));
        assert!(says_already(&current, &Rules { session_hours: Some(12), ..Rules::default() }));
        assert!(!says_already(&current, &Rules { session_hours: Some(13), ..Rules::default() }));
    }

    #[test]
    fn a_value_out_of_range_or_a_typo_is_refused() {
        for text in [
            "[sign-in]\nsession-hours = 0\n",
            "[sign-in]\nsession-hours = 169\n",
            "[sign-in]\nidle-minutes = 4\n",
            "[sign-in]\nsecond-step = \"never\"\n",
            "[sign-in]\nsecond-steps = \"everyone\"\n",
            "[console]\nlisten = 80\n",
            "[console]\nlisten = 70000\n",
            "[console]\nreach = \"public\"\n",
            "[console]\ncertificate = \"../key\"\n",
            "[consoles]\n",
            "[terminal]\nallowed = \"admins\"\n",
            "[terminal]\nsessions-per-account = 0\n",
            "[terminal]\nsessions-per-account = 65\n",
            "[terminal]\nkeep-days = 7\n",
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
    }
}
