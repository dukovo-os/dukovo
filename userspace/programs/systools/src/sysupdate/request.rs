// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A stage asked for by another program (ADR 0473). The root's top level
//! is held only by the services that need it, so `update apply` and
//! updated (which also holds the whole tree for the apps it updates) do
//! not stage a system update themselves: they write what to stage to
//! [`REQUEST`] and start [`SERVICE`], a service whose program is
//! `sysupdate stage --request` and whose view holds the system's
//! subvolumes, the EFI system partitions and the network, and no home
//! folder. The stage writes what it did, and how it ended, to
//! [`OUTCOME`]; the asker reads it back.
//!
//! The request is one line: `system`, to stage the release the channel
//! offers, or `firmware SET`, to stage a firmware set. The outcome is a
//! `said` line for each line the stage printed, then `ok` or
//! `failed REASON`. A stage that ends with its outcome written has
//! handled its request, however it came out, so it exits 0 and Spark does
//! not count it a failed service. When a stage does not behave, `log
//! sysupdate-stage` holds what it printed; the `update-online` boot test
//! stages a release through it.
//!
//! One asker at a time: an asker holds [`ASKING`] from writing its
//! request to reading the outcome, and starts the stage only once no
//! stage runs, so the outcome it reads is its own request's. updated's
//! unattended round and `update apply` at the same moment take turns.

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use spark_api::show::{self, State};

use super::{firmware, place, stage};
use crate::util::{self, Fail, fail};
use crate::{power, service};

/// Where the asker writes what to stage.
pub const REQUEST: &str = "/system/live/sysupdate/request";

/// Where the stage writes what it did.
pub const OUTCOME: &str = "/system/live/sysupdate/outcome";

/// The lock an asker holds while it asks.
pub const ASKING: &str = "/system/live/sysupdate/asking";

/// The service that stages.
pub const SERVICE: &str = "sysupdate-stage";

/// How often the asker looks at whether the stage still runs, once Spark
/// has stopped waiting for it.
const LOOK: Duration = Duration::from_secs(2);

/// The lines the stage prints while its outcome is being recorded.
static SAID: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// What to stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// The release the channel offers.
    System,
    /// The firmware set of this name.
    Firmware(String),
}

impl What {
    /// The request's text.
    fn to_text(&self) -> String {
        match self {
            What::System => "system\n".into(),
            What::Firmware(set) => format!("firmware {set}\n"),
        }
    }

    /// The request in `text`.
    ///
    /// # Errors
    ///
    /// What is wrong with it.
    fn parse(text: &str) -> Result<What, String> {
        let mut words = text.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("system"), None, None) => Ok(What::System),
            (Some("firmware"), Some(set), None) if plain(set) => Ok(What::Firmware(set.into())),
            _ => Err(format!("{REQUEST} asks for neither `system` nor `firmware SET`")),
        }
    }
}

/// Whether `set` can only name a firmware set: no `/`, and no leading dot.
fn plain(set: &str) -> bool {
    !set.starts_with('.') && set.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
}

/// Records a line the stage prints, for its outcome, while one is being
/// recorded.
pub(crate) fn record(line: &str) {
    if let Ok(mut said) = SAID.lock()
        && let Some(lines) = said.as_mut()
    {
        // One `said` line each: a line break in what it printed would
        // start a line of the outcome's own, `ok` say.
        lines.extend(line.lines().map(str::to_string));
    }
}

/// The outcome's text: `said` the lines, then how it ended.
fn outcome_text(lines: &[String], result: &Result<(), Fail>) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str("said ");
        text.push_str(line);
        text.push('\n');
    }
    match result {
        Ok(()) => text.push_str("ok\n"),
        Err(why) => text.push_str(&format!("failed {}\n", why.message.replace('\n', " "))),
    }
    text
}

/// The lines of an outcome's `text`, and how the stage ended; `None` for
/// text with no ending, as the stage's not having finished leaves.
fn read_outcome(text: &str) -> Option<(Vec<String>, Result<(), String>)> {
    let mut lines = Vec::new();
    for line in text.lines() {
        if let Some(said) = line.strip_prefix("said ") {
            lines.push(said.to_string());
        } else if line == "ok" {
            return Some((lines, Ok(())));
        } else if let Some(why) = line.strip_prefix("failed ") {
            return Some((lines, Err(why.to_string())));
        }
    }
    None
}

/// `sysupdate stage --request FILE`: the service's program. Stages what
/// the request in `file` asks, and writes the outcome.
///
/// # Errors
///
/// If the request cannot be read or the outcome cannot be written.
pub fn serve(file: &Path) -> Result<i32, Fail> {
    let text = std::fs::read_to_string(file).map_err(|err| util::failed(&file.display(), err))?;
    let what = What::parse(&text).map_err(fail)?;
    if let Ok(mut said) = SAID.lock() {
        *said = Some(Vec::new());
    }
    let result = stage_what(&what);
    let lines = SAID.lock().ok().and_then(|mut said| said.take()).unwrap_or_default();
    if let Err(why) = &result {
        println!("sysupdate: {}", why.message);
    }
    write_outcome(&outcome_text(&lines, &result))?;
    Ok(0)
}

/// Stages `what`, as `sysupdate stage` and `sysupdate firmware stage` do.
fn stage_what(what: &What) -> Result<(), Fail> {
    let root = place::root()?;
    match what {
        What::System => {
            stage::stage(&root, &stage::Asked { repo: None, version: None, stop: None })
        }
        What::Firmware(set) => firmware::stage(&root, None, Some(set)),
    }
}

/// Writes `text` as [`OUTCOME`], whole or not at all.
fn write_outcome(text: &str) -> Result<(), Fail> {
    let path = Path::new(OUTCOME);
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|err| util::failed(&OUTCOME, err))
}

/// Has [`SERVICE`] stage `what`, and waits for it. Returns the lines the
/// stage printed.
///
/// # Errors
///
/// What stopped the stage, in its words; or that it could not be asked or
/// ended without an outcome.
pub fn ask(what: &What) -> Result<Vec<String>, Fail> {
    let folder = Path::new(REQUEST).parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(folder).map_err(|err| util::failed(&folder.display(), err))?;
    let asking = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ASKING)
        .map_err(|err| util::failed(&ASKING, err))?;
    // SAFETY: flock takes a descriptor `asking` owns; the lock goes when
    // it is closed, as this function returns or the program ends.
    if unsafe { libc::flock(asking.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        println!("sysupdate: another update is being staged; waiting for it to finish");
        // SAFETY: as above.
        if unsafe { libc::flock(asking.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(util::failed(&ASKING, std::io::Error::last_os_error()));
        }
    }
    // A stage an asker that has gone started may still run: its outcome
    // is not this request's.
    if stage_runs() {
        println!("sysupdate: a stage runs already; waiting for it to finish");
        while stage_runs() {
            std::thread::sleep(LOOK);
        }
    }
    let _ = std::fs::remove_file(OUTCOME);
    std::fs::write(REQUEST, what.to_text()).map_err(|err| util::failed(&REQUEST, err))?;
    let started = service::change("start", SERVICE, &["--quiet".to_string()]);
    let outcome = wait_for_outcome();
    match (outcome, started) {
        (Some((lines, Ok(()))), _) => Ok(lines),
        (Some((_, Err(why))), _) => Err(fail(why)),
        (None, Err(why)) => Err(why),
        (None, Ok(_)) => Err(fail(format!(
            "the stage ended without saying how: `log {SERVICE}` shows what it printed"
        ))),
    }
}

/// The outcome of the stage, once it has written one or no longer runs.
fn wait_for_outcome() -> Option<(Vec<String>, Result<(), String>)> {
    loop {
        if let Some(outcome) =
            std::fs::read_to_string(OUTCOME).ok().as_deref().and_then(read_outcome)
        {
            return Some(outcome);
        }
        if !stage_runs() {
            return None;
        }
        std::thread::sleep(LOOK);
    }
}

/// Whether Spark says [`SERVICE`] has a process or is about to.
fn stage_runs() -> bool {
    let Ok(answer) = power::request("show") else { return false };
    let Ok(document) = show::parse(&answer) else { return false };
    document.services.iter().any(|service| {
        service.name == SERVICE
            && matches!(
                service.state,
                State::Pending
                    | State::Waiting
                    | State::Starting
                    | State::Running
                    | State::Stopping
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_what_the_stage_reads_back() {
        for what in [What::System, What::Firmware("2026.2.1".into())] {
            assert_eq!(What::parse(&what.to_text()), Ok(what));
        }
        for text in ["", "system now\n", "firmware\n", "firmware a b\n", "firmware ../x\n"] {
            assert!(What::parse(text).is_err(), "{text:?}");
        }
        assert!(What::parse("firmware .hidden\n").is_err());
    }

    #[test]
    fn an_outcome_says_what_was_printed_and_how_it_ended() {
        let lines = ["sysupdate: building slot B".to_string(), "sysupdate: staged".to_string()];
        let text = outcome_text(&lines, &Ok(()));
        assert_eq!(text, "said sysupdate: building slot B\nsaid sysupdate: staged\nok\n");
        assert_eq!(read_outcome(&text), Some((lines.to_vec(), Ok(()))));
        let failed = outcome_text(&lines[..1], &Err(fail("no space\nleft".into())));
        assert_eq!(
            read_outcome(&failed),
            Some((vec![lines[0].clone()], Err("no space left".into())))
        );
        // A stage that has not finished has said no ending yet.
        assert_eq!(read_outcome("said sysupdate: building slot B\n"), None);
        assert_eq!(read_outcome(""), None);
    }

    #[test]
    fn the_lines_printed_while_recording_are_kept_and_others_are_not() {
        record("before");
        *SAID.lock().unwrap() = Some(Vec::new());
        record("one");
        record("two");
        // A line break in what it printed starts no line of the outcome's
        // own: `ok` stays a `said` line.
        record("three\nok");
        let kept = SAID.lock().unwrap().take().unwrap();
        record("after");
        assert_eq!(kept, ["one", "two", "three", "ok"]);
        assert!(SAID.lock().unwrap().is_none());
        let text = outcome_text(&kept, &Err(fail("no space".into())));
        assert_eq!(read_outcome(&text), Some((kept, Err("no space".into()))));
    }
}
