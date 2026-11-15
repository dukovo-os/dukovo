// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `sysupdate canary`: the machine's side of a canary that reports (ADR
//! 0533). A canary is a machine the project owns that takes every release
//! first (ADR 0456), and tells the update server each hour how it does,
//! so that the server holds a release's waves while any canary is not
//! green.
//!
//! - `enrol [NAME]` makes the machine's key, `canary.key` beside
//!   `updates.toml`, root's alone, and sets `canary = true` and
//!   `canary_name` there. It prints the line to run on the maintainer's
//!   machine, `./ob release canary add NAME KEY`, which puts the name and
//!   the public half of the key on the update server's list. Run on an
//!   enrolled machine, it prints the line again and changes nothing.
//! - `report [--dry-run]` builds a [`Report`] of the machine's own state,
//!   signs it with the key, and posts it to the update server, at the host
//!   `updates.toml` names. The Spark job `dukovo-canary-report` runs it
//!   each hour, and updated(8) starts the job once after its first check,
//!   so that a restart into a new release is reported soon. On a machine
//!   that is not an enrolled canary it says so in one line and ends with
//!   status 0, so that the job is quiet on every other machine.
//!
//! The report is built from what `sysupdate status` reads, in this
//! process: the release that runs, the one staged and the channel; the
//! health check's last result; the last update, when it fell back
//! ([`fell_back`]); what the checks of dukovo-check(8) find, run now but
//! kept nowhere; and the services Spark gives up on or finds not running
//! as asked, as `service list --failed` says them. The receiver on the
//! update server takes the report only as the exact bytes this signs
//! (`system_release::canary`), and bounds every text and list in it, which
//! [`bound`] keeps to.

use std::io::Read as _;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use spark_api::show::Service;
// FIELD_MAX and LIST_MAX are the bounds the receiver keeps a report to.
use system_release::canary::{self, Check, FIELD_MAX, FellBack, Health, Key, LIST_MAX, Report};

use super::documents::Facts;
use super::online::{self, Settings};
use super::place;
use super::starts::{self, Outcome, Start};
use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail};

/// Where the canary's signing key is: beside `updates.toml`, in PKCS #8,
/// root's alone.
pub(crate) const KEY: &str = "/system/settings/sysupdate/canary.key";

/// The Spark job that reports (`dukovo-canary-report.toml`): each hour,
/// and once when updated(8) asks after its first check.
pub(crate) const SERVICE: &str = "dukovo-canary-report";

/// The most bytes of a refusal's words that are read.
const WORDS_MAX: u64 = 512;

/// What `report` says on a machine that is not an enrolled canary.
const NOT_A_CANARY: &str = "sysupdate: this machine is not a canary, so it reports nothing; \
                            `sysupdate canary enrol` makes it one";

/// Where a canary's settings and key are; the tests keep their own.
struct Place {
    settings: PathBuf,
    key: PathBuf,
}

impl Place {
    /// This system's.
    fn system() -> Place {
        Place { settings: PathBuf::from(online::SETTINGS), key: PathBuf::from(KEY) }
    }
}

/// `sysupdate canary enrol [NAME]` and `report [--dry-run]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let action = args.subcommand().map(|a| a.to_string_lossy().into_owned());
    let (mut name, mut dry_run) = (None, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(option) if option == "dry-run" && action.as_deref() == Some("report") => {
                dry_run = true;
            }
            Arg::Operand(operand) if action.as_deref() == Some("enrol") && name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(action @ ("enrol" | "report")) = action.as_deref() else {
        return Err(util::usage("say enrol [NAME] or report [--dry-run]".into()));
    };
    if util::own_user() != 0 {
        return Err(fail(format!("needs an administrator: run elevate sysupdate canary {action}")));
    }
    let place = Place::system();
    if action == "enrol" {
        let enrolled = enrol_in(&place, name.as_deref(), &util::hostname())?;
        for line in enrolled.lines() {
            println!("{line}");
        }
        return Ok(0);
    }
    match report_in(&place, dry_run, gather, post)? {
        Done::NotCanary => println!("{NOT_A_CANARY}"),
        Done::Dry { report, envelope, url } => {
            println!("sysupdate: the report, as the canary signs it, laid out to read:");
            println!("{report}");
            println!(
                "sysupdate: --dry-run sends nothing. The next line is the exact bytes that a \
                 report sends to {url}, as the body of a POST:"
            );
            println!("{}", String::from_utf8_lossy(&envelope));
        }
        Done::Sent { name, url } => println!("sysupdate: reported as the canary {name} to {url}"),
    }
    Ok(0)
}

/// What `enrol` came to: the canary's name and the public half of its key.
#[derive(Debug, PartialEq, Eq)]
struct Enrolled {
    name: String,
    /// The public key, 32 bytes in hex, as the update server's list keeps
    /// it.
    key: String,
}

impl Enrolled {
    /// What `enrol` prints: one line on what the machine does now, then
    /// the line to run on the maintainer's machine, by itself so that it
    /// copies whole.
    fn lines(&self) -> [String; 2] {
        [
            format!(
                "sysupdate: this machine is the canary {}: it takes every release first and \
                 reports to the update server each hour; to list it there, run this on the \
                 maintainer's machine:",
                self.name
            ),
            format!("./ob release canary add {} {}", self.name, self.key),
        ]
    }
}

/// Makes this machine a canary: its key, unless it has one, and
/// `canary = true` and `canary_name` in the settings. `asked` is the name
/// the owner gave; without one, the name the settings have, or `host`.
/// Run again on an enrolled machine, it changes nothing.
///
/// # Errors
///
/// If the name is not a canary's, or is not the one the machine has; if
/// the key there is not a key, or the key or the settings cannot be
/// written.
fn enrol_in(place: &Place, asked: Option<&str>, host: &str) -> Result<Enrolled, Fail> {
    let mut settings = Settings::load_from(&place.settings)?;
    let name = match (asked, settings.canary_name.as_deref()) {
        (Some(asked), Some(held)) if settings.canary && asked != held && place.key.exists() => {
            return Err(fail(format!(
                "this machine is enrolled as {held} already: to rename it, set canary_name in {} \
                 and run `sysupdate canary enrol` again",
                place.settings.display()
            )));
        }
        (Some(asked), _) => {
            if !canary::valid_name(asked) {
                return Err(util::usage(format!("`{asked}` is not a canary's name: {NAMES}")));
            }
            asked.to_string()
        }
        (None, Some(held)) => held.to_string(),
        (None, None) if canary::valid_name(host) => host.to_string(),
        (None, None) => {
            return Err(fail(format!(
                "the host name `{host}` is not a canary's name ({NAMES}): give a NAME, as \
                 `sysupdate canary enrol NAME`"
            )));
        }
    };
    // The key first, so that a failure leaves no name without a key.
    let key = match read_key(&place.key)? {
        Some(key) => key,
        None => {
            let bytes = Key::generate().map_err(fail)?;
            write_private(&place.key, &bytes)
                .map_err(|err| util::failed(&place.key.display(), err))?;
            Key::from_pkcs8(&bytes).map_err(fail)?
        }
    };
    if !settings.canary || settings.canary_name.as_deref() != Some(name.as_str()) {
        settings.canary = true;
        settings.canary_name = Some(name.clone());
        settings.save_to(&place.settings)?;
    }
    Ok(Enrolled { name, key: key.public_hex() })
}

/// What a canary's name is made of, for a message.
const NAMES: &str = "use 1 to 32 of a-z, 0-9 and -, not first -";

/// The key kept at `path`, or `None` if there is none.
///
/// # Errors
///
/// If the file cannot be read or is not an Ed25519 key.
fn read_key(path: &Path) -> Result<Option<Key>, Fail> {
    match std::fs::read(path) {
        Ok(bytes) => Key::from_pkcs8(&bytes).map(Some).map_err(|_| {
            fail(format!(
                "{}: not an Ed25519 key; remove the file to make a new one",
                path.display()
            ))
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(util::failed(&path.display(), err)),
    }
}

/// Keeps `bytes` at `path`, for its owner alone: written to a file beside
/// it, with mode 0600 from its first byte, then renamed into place.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("key.partial");
    let _ = std::fs::remove_file(&partial);
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&partial)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)
}

/// What `report` did.
#[derive(Debug, PartialEq, Eq)]
enum Done {
    /// The machine is not an enrolled canary: nothing was built or sent.
    NotCanary,
    /// `--dry-run`: the report, laid out to read, the envelope that a
    /// report sends, and where it goes.
    Dry { report: String, envelope: Vec<u8>, url: String },
    /// The update server took the report of canary `name`.
    Sent { name: String, url: String },
}

/// Reports, or with `dry_run` shows what it would. `gather` builds the
/// report from the machine's state for the canary named, in the
/// settings, and `send` posts the envelope to the url.
///
/// # Errors
///
/// If the settings or the key cannot be read, the state cannot be
/// gathered or the update server does not take the report.
fn report_in(
    place: &Place,
    dry_run: bool,
    gather: impl FnOnce(&Settings, &str) -> Result<Report, Fail>,
    send: impl FnOnce(&tuf::Url, &[u8]) -> Result<(), Fail>,
) -> Result<Done, Fail> {
    let settings = Settings::load_from(&place.settings)?;
    let (true, Some(name)) = (settings.canary, settings.canary_name.clone()) else {
        return Ok(Done::NotCanary);
    };
    let Some(key) = read_key(&place.key)? else { return Ok(Done::NotCanary) };
    let url = report_url(&settings.server)?;
    let mut report = gather(&settings, &name)?;
    let envelope = seal_within(&key, &mut report);
    let shown = shown(&url);
    if dry_run {
        let report = serde_json::to_string_pretty(&report).map_err(|err| fail(err.to_string()))?;
        return Ok(Done::Dry { report, envelope, url: shown });
    }
    send(&url, &envelope)?;
    Ok(Done::Sent { name, url: shown })
}

/// Where a canary sends its report: the update server's host, and port
/// when `server` names one, and the path the receiver answers at.
fn report_url(server: &str) -> Result<tuf::Url, Fail> {
    tuf::Url::parse(server)
        .and_then(|url| url.resolve(canary::REPORT_PATH))
        .map_err(|err| fail(format!("server = \"{server}\": {err}")))
}

/// `url` as a person reads it.
fn shown(url: &tuf::Url) -> String {
    format!("{}://{}{}", url.scheme(), tuf::http::host_header(url), url.path())
}

/// Posts `body`, a sealed report, to `url`: over TLS with the system's
/// root certificates when it is an https address, as the rest of the
/// update server's files are fetched.
///
/// # Errors
///
/// If the server cannot be reached, or does not answer 2xx; its words
/// are in the message.
fn post(url: &tuf::Url, body: &[u8]) -> Result<(), Fail> {
    let mut answer =
        tuf::http::exchange("POST", url, &[("Content-Type", "application/json")], body)
            .map_err(|err| fail(format!("cannot send the report to {}: {err}", shown(url))))?;
    if (200..300).contains(&answer.status) {
        return Ok(());
    }
    let mut words = String::new();
    // What the receiver says is a line or so; a body that is not text says
    // nothing.
    let _ = (&mut answer.body).take(WORDS_MAX).read_to_string(&mut words);
    let words = clip(&words);
    let because = if words.is_empty() { String::new() } else { format!(": {words}") };
    Err(fail(format!(
        "the update server refused the report, HTTP {}{because} ({})",
        answer.status,
        shown(url)
    )))
}

/// The report of canary `name` for this machine, read now.
///
/// # Errors
///
/// If the slots, Spark or the update server's settings cannot be read:
/// a report that cannot say which services failed would be green for the
/// wrong reason, so none is made.
fn gather(settings: &Settings, name: &str) -> Result<Report, Fail> {
    let root = place::root()?;
    let facts = super::read_facts(&root)?;
    let starts = starts::all();
    let (lines, services) = crate::check::run_now();
    let services = services
        .map_err(|why| fail(format!("cannot ask Spark which services have failed: {why}")))?;
    let now = i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(0);
    build(
        name,
        now,
        &settings.channel,
        &facts,
        &starts,
        crate::check::for_canary(&lines),
        &services,
    )
}

/// The report of canary `name` at `now`, from the machine's `facts`, the
/// records of its `starts`, what the checks found (`check`) and its
/// `services`, within the bounds the receiver keeps.
///
/// # Errors
///
/// If the running slot does not say its release.
fn build(
    name: &str,
    now: i64,
    channel: &str,
    facts: &Facts,
    starts: &[Start],
    check: Option<Check>,
    services: &[Service],
) -> Result<Report, Fail> {
    let running = facts.running_version().ok_or_else(|| {
        fail("the running slot does not say its release (startup/slot.env)".into())
    })?;
    let mut report = Report::new(name, now, &running, channel);
    report.staged = facts.staged_version();
    report.health = facts.health.as_ref().map(|health| Health {
        passed: health.passed,
        // A file that does not say when is no reason to leave out a
        // failure.
        time: health.time.unwrap_or(0),
        detail: health.detail.clone(),
    });
    report.fell_back = fell_back(facts, starts);
    report.check = check;
    report.services_failed = services
        .iter()
        .filter(|service| crate::service::not_as_asked(service))
        .map(|service| service.name.clone())
        .collect();
    bound(&mut report);
    Ok(report)
}

/// Whether the last update fell back: it was tried and never proved
/// itself, and the machine runs the release before it.
///
/// A fallback leaves its mark in the update's folder (`restored`: Spark
/// put the settings from before the update back, ADR 0454). That mark is
/// also left by `sysupdate rollback --settings-from-before`, which goes
/// back from an update that proved itself, and a start of the update's
/// slot that the health check saw through since the update was staged
/// tells the two apart. An update that is staged and not tried yet, or
/// that is unstaged, has no mark; a new stage clears the folder, so a
/// newer update replaces the account of an older one that failed.
fn fell_back(facts: &Facts, starts: &[Start]) -> Option<FellBack> {
    let update = facts.update.as_ref().filter(|update| update.restored)?;
    let (tried, slot) = (update.to.as_ref()?, update.slot.as_deref()?);
    if slot == facts.running.letter() {
        return None;
    }
    let proved = starts.iter().any(|start| {
        start.slot == slot
            && start.outcome == Outcome::Good
            && update.staged.is_none_or(|staged| start.started >= staged)
    });
    if proved {
        return None;
    }
    Some(FellBack { tried: tried.clone(), back_to: facts.running_version()? })
}

/// `report` sealed, as an envelope that the receiver takes by its size
/// ([`canary::REPORT_MAX`]). The bounds on each text and list let a
/// report with a great many long failures come to more than that; the
/// longer list is cut by half, and half again, until the envelope fits,
/// which keeps the first of each, the counts and everything else.
fn seal_within(key: &Key, report: &mut Report) -> Vec<u8> {
    loop {
        let envelope = key.seal(report);
        let checks = report.check.as_ref().map_or(0, |check| check.failed.len());
        let services = report.services_failed.len();
        if envelope.len() <= canary::REPORT_MAX || checks.max(services) <= 1 {
            return envelope;
        }
        if checks >= services {
            if let Some(check) = &mut report.check {
                check.failed.truncate(checks / 2);
            }
        } else {
            report.services_failed.truncate(services / 2);
        }
    }
}

/// Brings every text and list of `report` within the receiver's bounds,
/// so that a long detail never costs a canary its report.
fn bound(report: &mut Report) {
    let text = |field: &mut String| *field = clip(field);
    text(&mut report.running);
    text(&mut report.channel);
    report.staged.iter_mut().for_each(text);
    if let Some(health) = &mut report.health {
        text(&mut health.detail);
    }
    if let Some(fell) = &mut report.fell_back {
        text(&mut fell.tried);
        text(&mut fell.back_to);
    }
    if let Some(check) = &mut report.check {
        check.failed.truncate(LIST_MAX);
        check.failed.iter_mut().for_each(text);
    }
    report.services_failed.truncate(LIST_MAX);
    report.services_failed.iter_mut().for_each(text);
}

/// `text` on one line, of at most [`FIELD_MAX`] bytes, cut at a
/// character and ended with `...` when it was longer.
fn clip(text: &str) -> String {
    let flat: String = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.len() <= FIELD_MAX {
        return flat;
    }
    let mut end = FIELD_MAX - 3;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &flat[..end])
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::MetadataExt;

    use slots::{Slot, State};
    use spark_api::show::State as Is;

    use super::*;
    use crate::sysupdate::documents::{Health as Last, SlotFacts, Update};

    const NOW: i64 = 1_800_000_000;

    /// A folder of a test's own, with a `Place` in it.
    fn scratch(call: &str) -> (PathBuf, Place) {
        let dir =
            std::env::temp_dir().join(format!("sysupdate-canary-{}-{call}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let place = Place { settings: dir.join("updates.toml"), key: dir.join("canary.key") };
        (dir, place)
    }

    fn slot_with(slot: Slot, version: &str) -> SlotFacts {
        SlotFacts { slot, exists: true, version: Some(version.into()), firmware: None }
    }

    /// Slot A runs 0.16.0, the default; the state says what is tried.
    fn facts(trying: Option<Slot>, tries: u8, update: Option<Update>) -> Facts {
        Facts {
            running: Slot::A,
            state: State {
                generation: 1,
                default: Slot::A,
                previous: None,
                trying,
                tries,
                boot: None,
            },
            slots: [slot_with(Slot::A, "0.16.0"), slot_with(Slot::B, "0.16.1")],
            health: Last::parse("1799999000 passed 14 services up"),
            update,
            rescue: None,
            partitions: Vec::new(),
        }
    }

    fn update(restored: bool) -> Update {
        Update {
            from: Some("0.16.0".into()),
            to: Some("0.16.1".into()),
            slot: Some("b".into()),
            staged: Some(NOW - 7200),
            restored,
        }
    }

    fn start(slot: &str, outcome: Outcome, started: i64) -> Start {
        Start {
            number: 1,
            started,
            slot: slot.into(),
            stage: "services".into(),
            outcome,
            error: None,
            why: None,
            failing: None,
            trial: None,
            items: Vec::new(),
            times: None,
        }
    }

    fn service(name: &str, state: Is) -> Service {
        Service { name: name.into(), state, ..Service::default() }
    }

    fn built(facts: &Facts, starts: &[Start], services: &[Service]) -> Report {
        build("ref-box", NOW, "beta", facts, starts, None, services).unwrap()
    }

    #[test]
    fn a_report_says_what_runs_what_waits_the_health_check_the_checks_and_the_failed_services() {
        let waiting = facts(Some(Slot::B), 3, Some(update(false)));
        let check = Check { failures: 1, warnings: 2, failed: vec!["health: hardware: x".into()] };
        let services = [
            service("logd", Is::Running),
            service("web", Is::Failed),
            service("backup", Is::Disabled),
            service("sshd", Is::Waiting),
        ];
        let report =
            build("ref-box", NOW, "beta", &waiting, &[], Some(check.clone()), &services).unwrap();
        assert_eq!(report.kind, canary::REPORT_TYPE);
        assert_eq!((report.canary.as_str(), report.time), ("ref-box", NOW));
        assert_eq!((report.running.as_str(), report.channel.as_str()), ("0.16.0", "beta"));
        assert_eq!(report.staged.as_deref(), Some("0.16.1"));
        assert_eq!(
            report.health,
            Some(Health { passed: true, time: 1_799_999_000, detail: "14 services up".into() })
        );
        assert_eq!(report.fell_back, None);
        assert_eq!(report.check, Some(check));
        // As `service list --failed` reads them: given up on, or waiting to start again.
        assert_eq!(report.services_failed, ["web", "sshd"]);

        // Nothing staged, no health check yet, and no checks run.
        let mut idle = facts(None, 0, None);
        idle.health = None;
        let report = built(&idle, &[], &[]);
        assert_eq!((report.staged, report.health, report.check), (None, None, None));
        assert!(report.services_failed.is_empty());

        // An update with no tries left is not staged for the next start; nor is a firmware set
        // staged beside the release that runs.
        assert_eq!(built(&facts(Some(Slot::B), 0, None), &[], &[]).staged, None);
        let mut firmware = facts(Some(Slot::B), 3, None);
        firmware.slots[1].version = Some("0.16.0".into());
        assert_eq!(built(&firmware, &[], &[]).staged, None);
        // A running slot that does not say its release cannot be reported.
        let mut unsaid = facts(None, 0, None);
        unsaid.slots[0].version = None;
        assert!(build("ref-box", NOW, "beta", &unsaid, &[], None, &[]).is_err());
        // A health check that failed is said, with no time when the file has none.
        let mut sick = facts(None, 0, None);
        sick.health = Last::parse("soon failed netd gave up");
        let health = built(&sick, &[], &[]).health.unwrap();
        assert_eq!(
            (health.passed, health.time, health.detail.as_str()),
            (false, 0, "netd gave up")
        );
    }

    #[test]
    fn an_update_that_failed_and_went_back_is_said_with_the_release_tried_and_gone_back_to() {
        // The default runs, the fallback is acknowledged, the settings are put back.
        let fell = facts(None, 0, Some(update(true)));
        let failed =
            [start("a", Outcome::Good, NOW - 9000), start("b", Outcome::Failed, NOW - 3600)];
        assert_eq!(
            built(&fell, &failed, &[]).fell_back,
            Some(FellBack { tried: "0.16.1".into(), back_to: "0.16.0".into() })
        );
        // It hung before the health check began: no record, and the mark still says.
        assert!(built(&fell, &[], &[]).fell_back.is_some());
        // Only a fallback or a rollback leaves the mark: no mark, no fallback.
        let unmarked = facts(None, 0, Some(update(false)));
        assert_eq!(built(&unmarked, &failed, &[]).fell_back, None);
        // No update on record.
        assert_eq!(built(&facts(None, 0, None), &failed, &[]).fell_back, None);
        // A rollback the owner asked for, from an update that proved itself: not a failed update.
        let proved = [start("b", Outcome::Good, NOW - 3600)];
        assert_eq!(built(&fell, &proved, &[]).fell_back, None);
        // A start of that slot from an update before this one proves nothing of this one.
        let earlier =
            [start("b", Outcome::Good, NOW - 90_000), start("b", Outcome::Failed, NOW - 3600)];
        assert!(built(&fell, &earlier, &[]).fell_back.is_some());
        // The slot of the update is the one that runs: it did not go back.
        let mut here = facts(None, 0, Some(update(true)));
        here.running = Slot::B;
        assert_eq!(built(&here, &failed, &[]).fell_back, None);
        // An update that does not say where it went, or what it was.
        let mut vague = update(true);
        vague.slot = None;
        assert_eq!(built(&facts(None, 0, Some(vague)), &failed, &[]).fell_back, None);
        let mut nameless = update(true);
        nameless.to = None;
        assert_eq!(built(&facts(None, 0, Some(nameless)), &failed, &[]).fell_back, None);
    }

    #[test]
    fn a_report_is_always_within_what_the_receiver_takes() {
        let long = "x".repeat(2000);
        let mut sick = facts(Some(Slot::B), 3, Some(update(true)));
        sick.health = Last::parse(&format!("1799999000 failed {long}"));
        let many: Vec<Service> =
            (0..100).map(|n| service(&format!("svc{n}"), Is::Failed)).collect();
        let check = Check {
            failures: 100,
            warnings: 0,
            failed: (0..100).map(|n| format!("area: check: {n} é{long}")).collect(),
        };
        let report = build("ref-box", NOW, "beta", &sick, &[], Some(check), &many).unwrap();
        assert_eq!(report.services_failed.len(), 32);
        assert_eq!(report.check.as_ref().unwrap().failed.len(), 32);
        assert_eq!(report.check.as_ref().unwrap().failures, 100);
        assert!(report.health.as_ref().unwrap().detail.ends_with("..."));

        // Bounded as they are, so many long texts come to more than the receiver takes: the lists are
        // cut to fit, and the receiver's own verdict is that it takes the report.
        let key = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
        let mut list = canary::Canaries::default();
        list.add("ref-box", &key.public_hex(), NOW).unwrap();
        assert!(key.seal(&report).len() > canary::REPORT_MAX);
        let mut report = report;
        let envelope = seal_within(&key, &mut report);
        assert!(envelope.len() <= canary::REPORT_MAX);
        let verified = canary::verify(&envelope, &list, NOW).unwrap();
        assert_eq!(verified.report, report);
        // The first of each list, the counts and the rest of the report are kept.
        let check = report.check.as_ref().unwrap();
        assert_eq!(check.failures, 100);
        assert!(check.failed.first().is_some_and(|f| f.starts_with("area: check: 0 é")));
        assert_eq!(report.services_failed.first().map(String::as_str), Some("svc0"));
        assert_eq!((report.running.as_str(), report.fell_back.is_some()), ("0.16.0", true));
        // A report that fits is left as it is.
        let mut small = Report::new("ref-box", NOW, "0.16.0", "beta");
        small.services_failed = vec!["web".into()];
        let before = small.clone();
        assert!(seal_within(&key, &mut small).len() < 1024);
        assert_eq!(small, before);

        // Text is cut at a character, on one line, with no control characters.
        let cut = clip(&format!("é{}\n\tz\u{1b}[0m", "é".repeat(600)));
        assert!(cut.len() <= FIELD_MAX && cut.ends_with("..."));
        assert!(cut.chars().all(|c| !c.is_control()));
        assert_eq!(clip("  a \n b\t\tc  "), "a b c");
        assert_eq!(clip(""), "");
    }

    /// A key listed under `ref-box`, and a settings file that enrols it.
    fn enrolled(call: &str) -> (PathBuf, Place, Key) {
        let (dir, place) = scratch(call);
        enrol_in(&place, Some("ref-box"), "host").unwrap();
        let key = read_key(&place.key).unwrap().unwrap();
        (dir, place, key)
    }

    #[test]
    fn a_machine_that_is_not_an_enrolled_canary_reports_nothing_and_asks_for_nothing() {
        let (dir, place) = scratch("quiet");
        let sent = RefCell::new(0);
        let gathered = RefCell::new(0);
        let run = |dry_run| {
            report_in(
                &place,
                dry_run,
                |_, _| {
                    *gathered.borrow_mut() += 1;
                    Err(fail("not asked".into()))
                },
                |_, _| {
                    *sent.borrow_mut() += 1;
                    Ok(())
                },
            )
        };
        // No settings at all: an ordinary machine.
        assert_eq!(run(false).unwrap(), Done::NotCanary);
        assert_eq!(run(true).unwrap(), Done::NotCanary);
        // `canary = true` alone takes releases first, but is no canary that reports.
        std::fs::write(&place.settings, "canary = true\n").unwrap();
        assert_eq!(run(false).unwrap(), Done::NotCanary);
        // A name without `canary = true`, with a key.
        std::fs::write(&place.settings, "canary_name = \"ref-box\"\n").unwrap();
        write_private(&place.key, &Key::generate().unwrap()).unwrap();
        assert_eq!(run(false).unwrap(), Done::NotCanary);
        // `canary = true` and a name, and no key.
        std::fs::remove_file(&place.key).unwrap();
        std::fs::write(&place.settings, "canary = true\ncanary_name = \"ref-box\"\n").unwrap();
        assert_eq!(run(false).unwrap(), Done::NotCanary);
        assert_eq!((*sent.borrow(), *gathered.borrow()), (0, 0));
        // A settings file that is wrong, or a key that is not one, is no quiet case.
        std::fs::write(&place.settings, "canary = \"yes\"\n").unwrap();
        assert!(run(false).is_err());
        std::fs::write(&place.settings, "canary = true\ncanary_name = \"ref-box\"\n").unwrap();
        std::fs::write(&place.key, b"junk").unwrap();
        assert!(run(false).unwrap_err().message.contains("not an Ed25519 key"));
        // Spark's job on an ordinary machine prints one line and ends with 0.
        assert!(!NOT_A_CANARY.contains('\n') && NOT_A_CANARY.starts_with("sysupdate: "));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dry_run_shows_the_envelope_that_a_report_sends_and_sends_nothing() {
        let (dir, place, key) = enrolled("dry");
        std::fs::write(
            &place.settings,
            "canary = true\ncanary_name = \"ref-box\"\nserver = \"https://updates.example.org:8443/system\"\n",
        )
        .unwrap();
        let sent = RefCell::new(Vec::new());
        let dry = report_in(
            &place,
            true,
            |_, name| fixture_for(name),
            |_, body| {
                sent.borrow_mut().push(body.to_vec());
                Ok(())
            },
        )
        .unwrap();
        let Done::Dry { report, envelope, url } = dry else { panic!("not a dry run") };
        assert!(sent.borrow().is_empty());
        assert_eq!(url, "https://updates.example.org:8443/canary/v1/report");
        assert!(report.contains("\"canary\": \"ref-box\"") && report.contains('\n'));
        // The bytes are the envelope the update server verifies, of the report shown.
        let mut list = canary::Canaries::default();
        list.add("ref-box", &key.public_hex(), NOW).unwrap();
        let verified = canary::verify(&envelope, &list, NOW).unwrap();
        assert_eq!(serde_json::to_string_pretty(&verified.report).unwrap(), report);

        // Sending posts those same bytes to the same place.
        let done = report_in(
            &place,
            false,
            |_, name| fixture_for(name),
            |at, body| {
                assert_eq!(shown(at), "https://updates.example.org:8443/canary/v1/report");
                sent.borrow_mut().push(body.to_vec());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            done,
            Done::Sent {
                name: "ref-box".into(),
                url: "https://updates.example.org:8443/canary/v1/report".into()
            }
        );
        let posted = sent.borrow();
        assert_eq!(posted.len(), 1);
        assert!(canary::verify(&posted[0], &list, NOW).is_ok());
        // A server that does not take the report is a failure.
        let refused = report_in(
            &place,
            false,
            |_, name| fixture_for(name),
            |_, _| Err(fail("refused".into())),
        );
        assert_eq!(refused.unwrap_err().message, "refused");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn fixture_for(name: &str) -> Result<Report, Fail> {
        Ok(Report::new(name, NOW, "0.16.0", "stable"))
    }

    #[test]
    fn the_report_goes_to_the_host_of_the_update_server_in_its_scheme() {
        let url = |server: &str| shown(&report_url(server).unwrap());
        assert_eq!(
            url("https://updates.dukovo.org/system/"),
            "https://updates.dukovo.org/canary/v1/report"
        );
        assert_eq!(url("http://10.0.2.2:8080/system/"), "http://10.0.2.2:8080/canary/v1/report");
        assert!(report_url("not a url").is_err());
    }

    #[test]
    fn a_report_is_posted_and_the_servers_words_come_back_when_it_refuses() {
        let serve = |answer: &'static str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let thread = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let head = String::from_utf8(head).unwrap();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut body = vec![0u8; length];
                stream.read_exact(&mut body).unwrap();
                stream.write_all(answer.as_bytes()).unwrap();
                (head, body)
            });
            (port, thread)
        };
        let post_to = |port: u16, body: &[u8]| {
            let url = report_url(&format!("http://127.0.0.1:{port}/system/")).unwrap();
            post(&url, body)
        };

        let (port, thread) =
            serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        post_to(port, br#"{"report":"r","key":"k","signature":"s"}"#).unwrap();
        let (head, body) = thread.join().unwrap();
        assert!(head.starts_with("POST /canary/v1/report HTTP/1.1\r\n"), "{head}");
        assert!(head.contains(&format!("Host: 127.0.0.1:{port}\r\n")), "{head}");
        assert!(head.contains("Content-Type: application/json\r\n"), "{head}");
        assert_eq!(body, br#"{"report":"r","key":"k","signature":"s"}"#);

        let (port, thread) = serve(
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 34\r\nConnection: close\r\n\r\n\
             no canary on the list has that key",
        );
        let refused = post_to(port, b"{}").unwrap_err().message;
        thread.join().unwrap();
        assert!(
            refused.starts_with(
                "the update server refused the report, HTTP 403: no canary on the list has that key"
            ),
            "{refused}"
        );

        // Nothing listens: a failure that says where.
        let none = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let unreachable = post_to(none, b"{}").unwrap_err().message;
        assert!(
            unreachable.starts_with("cannot send the report to http://127.0.0.1:"),
            "{unreachable}"
        );
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().mode() & 0o777
    }

    #[test]
    fn enrolling_a_fresh_machine_makes_a_key_for_root_alone_and_settings_that_say_canary() {
        let (dir, place) = scratch("fresh");
        std::fs::write(&place.settings, "channel = \"beta\"\n\n[hold]\nsystem = \"0.15.9\"\n")
            .unwrap();
        let enrolled = enrol_in(&place, None, "ref-box").unwrap();
        assert_eq!(enrolled.name, "ref-box");
        assert_eq!(mode(&place.key), 0o600);
        let key = read_key(&place.key).unwrap().unwrap();
        assert_eq!(enrolled.key, key.public_hex());
        assert_eq!(enrolled.key.len(), 64);
        // The settings keep what they had, and add what a canary is.
        let settings = Settings::load_from(&place.settings).unwrap();
        assert_eq!(settings.channel, "beta");
        assert_eq!(settings.hold.system.as_deref(), Some("0.15.9"));
        assert!(settings.canary);
        assert_eq!(settings.canary_name.as_deref(), Some("ref-box"));
        let text = std::fs::read_to_string(&place.settings).unwrap();
        assert!(text.contains("canary = true\ncanary_name = \"ref-box\"\n"), "{text}");
        // What it prints: one line, and the line for the maintainer, alone.
        let lines = enrolled.lines();
        assert!(lines[0].starts_with(
            "sysupdate: this machine is the canary ref-box: it takes every release first"
        ));
        assert!(lines[0].contains("reports to the update server each hour"));
        assert_eq!(lines[1], format!("./ob release canary add ref-box {}", key.public_hex()));
        assert!(!std::path::Path::new(&place.key.with_extension("key.partial")).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enrolling_an_enrolled_machine_prints_the_same_line_and_changes_nothing() {
        let (dir, place) = scratch("again");
        let first = enrol_in(&place, Some("ref-box"), "other").unwrap();
        let (key, settings) =
            (std::fs::read(&place.key).unwrap(), std::fs::read(&place.settings).unwrap());
        for asked in [None, Some("ref-box")] {
            let again = enrol_in(&place, asked, "other").unwrap();
            assert_eq!(again, first);
            assert_eq!(again.lines(), first.lines());
            assert_eq!(std::fs::read(&place.key).unwrap(), key);
            assert_eq!(std::fs::read(&place.settings).unwrap(), settings);
        }
        // Another name is not taken in its place: the machine has the key of ref-box on the list.
        let renamed = enrol_in(&place, Some("do-nyc"), "other").unwrap_err().message;
        assert!(renamed.contains("enrolled as ref-box already") && renamed.contains("canary_name"));
        assert_eq!(std::fs::read(&place.settings).unwrap(), settings);
        // A machine that has a name and no key yet gets its key; a key it has is kept.
        std::fs::remove_file(&place.key).unwrap();
        let rekeyed = enrol_in(&place, None, "other").unwrap();
        assert_eq!(rekeyed.name, "ref-box");
        assert_ne!(rekeyed.key, first.key);
        assert_eq!(mode(&place.key), 0o600);
        let key = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
        std::fs::write(&place.settings, "canary = true\n").unwrap();
        std::fs::remove_file(&place.key).unwrap();
        let bytes = Key::generate().unwrap();
        write_private(&place.key, &bytes).unwrap();
        let kept = enrol_in(&place, Some("kept"), "other").unwrap();
        assert_eq!(kept.key, Key::from_pkcs8(&bytes).unwrap().public_hex());
        assert_ne!(kept.key, key.public_hex());
        assert_eq!(std::fs::read(&place.key).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_name_is_the_one_asked_for_or_the_host_name_and_must_be_a_canary_s() {
        let (dir, place) = scratch("names");
        for bad in ["", "-x", "Ref", "a.b", "a/b", &"a".repeat(33)] {
            let err = enrol_in(&place, Some(bad), "host").unwrap_err();
            assert!(err.message.contains("is not a canary's name"), "{bad:?}: {}", err.message);
        }
        // A host name that is no canary's name: the owner is told to give one.
        let err = enrol_in(&place, None, "Ref-Box.example.org").unwrap_err().message;
        assert!(err.contains("`Ref-Box.example.org`") && err.contains("give a NAME"), "{err}");
        assert!(!place.key.exists() && !place.settings.exists(), "nothing is made on a mistake");
        // A good host name is the default; the longest name there is is taken.
        assert_eq!(enrol_in(&place, None, "do-nyc-1").unwrap().name, "do-nyc-1");
        let _ = std::fs::remove_dir_all(dir);
        let (dir, place) = scratch("names-long");
        let longest = "a".repeat(32);
        assert_eq!(enrol_in(&place, Some(&longest), "host").unwrap().name, longest);
        let _ = std::fs::remove_dir_all(dir);
        // A key file that is not a key is not replaced, and no name is set.
        let (dir, place) = scratch("junk-key");
        std::fs::write(&place.key, b"junk").unwrap();
        let err = enrol_in(&place, Some("ref-box"), "host").unwrap_err().message;
        assert!(err.contains("not an Ed25519 key"), "{err}");
        assert_eq!(std::fs::read(&place.key).unwrap(), b"junk");
        assert!(!place.settings.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
