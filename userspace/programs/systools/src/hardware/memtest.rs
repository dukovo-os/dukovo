// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `hardware memtest`: the memory tested at the next start, once.
//!
//! The kernel tests the free memory before the first program when its
//! command line says `memtest=once`, takes each bad frame out of use, and
//! leaves the result in `/kernel/boot/memtest` (radix-sysfs(5)). This
//! asks for that at the next start only. The start menu already takes
//! one start of a slot with given boot options in place of the usual
//! start, for `changes bisect` ([`slots::trial`]); the test is such a
//! start, numbered [`MEMTEST_TRY`], whose options are this start's and
//! `memtest=once`. It changes no slot state, and a test that hangs or
//! loses the power leaves the next start the usual one.
//!
//! `--next-start` writes the asked file, then the block on every EFI
//! system partition of the root's disks; `--cancel` takes both away
//! while the menu has not taken the start; `--status` says what is asked
//! and what the last tests found. The health check calls [`after_start`]
//! once a start: it keeps the kernel's result, raises or clears the
//! alert, and clears what was asked, whether the test ran, did not
//! finish or was never taken (an update was tried first).
//!
//! Nothing here runs the test: the memory a program can reach is not the
//! memory the kernel keeps for itself, so `hardware check memory` tests
//! up to 1 GiB of it while the system runs, and this tests all that is
//! free.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use slots::Slot;
use slots::trial::{self, MAX_NUMBER, Trial};

use super::history::{self, Line};
use super::saved;
use crate::alerts::client::{self, Screen};
use crate::alerts::protocol::{About, Alert, Severity};
use crate::args::{Arg, Args};
use crate::proc;
use crate::sysupdate::bisect;
use crate::sysupdate::place::{self, Root};
use crate::sysupdate::starts::Start;
use crate::util::{self, Fail, fail, usage};

/// The number of the start the menu takes for the test: the last a try can
/// have, so that it is never a search's, which are numbered from 1.
pub(crate) const MEMTEST_TRY: u8 = MAX_NUMBER;

/// Where the test was asked for, until the start that is told of it.
pub(crate) const ASKED: &str = "/system/state/hardware/memtest-asked.json";

/// The name of the kept results in the state folder.
const KEPT: &str = "memtest.json";

/// The name of the asked file in the state folder.
const ASKED_NAME: &str = "memtest-asked.json";

/// The kernel's result of the test it ran at this start.
const RESULT: &str = "/kernel/boot/memtest";

/// The kernel's command line.
const CMDLINE: &str = "/kernel/cmdline";

/// The word that asks the kernel for the test.
const WORD: &str = "memtest=once";

/// The words of the same option, which a start's own options never carry
/// into the test: the test sets it.
const OPTION: &str = "memtest=";

/// How many tests the history keeps.
const HISTORY: usize = 10;

/// How long [`after_start`] waits for alertd, which may start after the
/// health check.
const ALERTD_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The memory a minute of the test covers, as the kernel's test goes.
const GIB_PER_MINUTE: u64 = 4;

const USAGE: &str = "usage: hardware memtest --next-start | --cancel | --status [--json]";

/// How a test at a start ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Ended {
    /// It ran and found nothing.
    Passed,
    /// It ran and found words that read back wrong.
    Errors,
    /// It was asked, the menu took it, and the start never told: the power
    /// went or the machine stopped.
    Unfinished,
    /// It was asked and the menu started something else: an update was
    /// tried first.
    NotTaken,
}

/// A range of bad memory the test found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Bad {
    /// The first byte, as the kernel writes it: `0x1000000`.
    pub(crate) from: String,
    /// The last byte.
    pub(crate) to: String,
    /// The words in the range that read back wrong.
    pub(crate) errors: u64,
}

/// What the test at one start was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Run {
    /// When it was kept, in milliseconds since 1970.
    pub(crate) at_ms: u64,
    /// The number of the start that kept it.
    pub(crate) start: u64,
    pub(crate) result: Ended,
    /// Whether it tested all the free memory: it stops after an hour.
    pub(crate) complete: bool,
    pub(crate) tested_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) errors: u64,
    /// Where the errors were, at most 64 ranges.
    pub(crate) bad: Vec<Bad>,
    /// The pages taken out of use until the next restart.
    pub(crate) retired: u64,
    pub(crate) seconds: u64,
}

/// `memtest.json`: the last test and the ten before it, the newest first,
/// the last again as the first of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    format: u32,
    last: Option<Run>,
    history: Vec<Run>,
}

/// `memtest-asked.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Asked {
    format: u32,
    /// When it was asked, in milliseconds since 1970.
    at_ms: u64,
    /// The user who asked: the one `elevate` ran this for.
    by: u32,
    /// The slot the test starts: the one that runs.
    slot: String,
}

/// What `--status --json` says of what is asked.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct AskedSaid {
    at_ms: u64,
    /// The user's number.
    by: u32,
    /// The user's name, or the number again when there is none.
    by_name: String,
}

/// What `--status --json` says.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Status {
    format: u32,
    asked: Option<AskedSaid>,
    last: Option<Run>,
    history: Vec<Run>,
}

/// Where the files are, so that tests use folders of their own.
struct Places {
    /// The state folder: the asked file, the kept results and the
    /// history of checks.
    dir: PathBuf,
    /// The kernel's result.
    result: PathBuf,
    /// The kernel's command line.
    cmdline: PathBuf,
    /// The memory the kernel manages.
    meminfo: PathBuf,
}

impl Places {
    /// The places of a running system.
    fn system() -> Places {
        Places {
            dir: PathBuf::from(saved::DIR),
            result: PathBuf::from(RESULT),
            cmdline: PathBuf::from(CMDLINE),
            meminfo: Path::new(proc::PROC).join("meminfo"),
        }
    }

    /// The memory the kernel manages, in bytes; 0 when it cannot be read.
    fn memory(&self) -> u64 {
        fs::read(&self.meminfo).map_or(0, |text| proc::parse_memory(&text).total * 1024)
    }
}

/// The start menu's block of the test, on the EFI system partitions.
trait Block {
    /// Whether the block holds the test's try, and whether the menu has
    /// taken it. A search's try is not the test's.
    fn holds(&self) -> Option<(u8, bool)>;

    /// Writes `block` on every partition.
    ///
    /// # Errors
    ///
    /// What went wrong on those that could not be written.
    fn write(&mut self, block: &[u8]) -> Result<(), String>;
}

/// The EFI system partitions of the root's disks, by name.
struct Partitions(Vec<String>);

impl Block for Partitions {
    fn holds(&self) -> Option<(u8, bool)> {
        bisect::block_says(&self.0, MEMTEST_TRY)
    }

    fn write(&mut self, block: &[u8]) -> Result<(), String> {
        bisect::write_blocks(&self.0, block)
    }
}

/// What the slot state and a search say of the next start.
struct Machine {
    /// The slot that runs.
    running: Slot,
    state: slots::State,
    /// The try a search of `changes bisect` waits on, if one does.
    search: Option<u8>,
}

/// What a start's work tells alertd.
#[derive(Debug, PartialEq, Eq)]
enum Told {
    /// An alert that stays until it is cleared.
    Raise(Alert),
    /// The alert of that key is over.
    Clear(&'static str),
    /// Something that happened once.
    Event(Alert),
}

/// hardware(8)'s `memtest`.
pub(crate) fn main(args: &mut Args) -> Result<i32, Fail> {
    #[derive(PartialEq)]
    enum What {
        NextStart,
        Cancel,
        Status,
    }
    let mut what: Option<What> = None;
    let mut json = false;
    while let Some(arg) = args.next()? {
        let asked = match &arg {
            Arg::Long(name) if name == "next-start" => What::NextStart,
            Arg::Long(name) if name == "cancel" => What::Cancel,
            Arg::Long(name) if name == "status" => What::Status,
            Arg::Long(name) if name == "json" => {
                json = true;
                continue;
            }
            other => return Err(args.unknown(other)),
        };
        if what.is_some() {
            return Err(usage(USAGE.into()));
        }
        what = Some(asked);
    }
    match what {
        Some(What::NextStart) if !json => next_start(),
        Some(What::Cancel) if !json => cancel(),
        Some(What::Status) => status(json),
        _ => Err(usage(USAGE.into())),
    }
}

/// Whether a test is asked for the next start: `changes bisect` leaves
/// the start to it.
pub(crate) fn is_asked() -> bool {
    Path::new(ASKED).exists()
}

/// `--next-start`.
fn next_start() -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail(
            "this writes the start menu's block: run `elevate hardware memtest --next-start`"
                .into(),
        ));
    }
    let root = place::root()?;
    let _lock = place::lock()?;
    let (_, state) = place::state(&root)?;
    let machine = Machine {
        running: root.running,
        state,
        search: bisect::kept().and_then(|search| search.waiting().map(|t| t.number)),
    };
    let mut block = Partitions(place::partitions(&root)?);
    let line = ask(&Places::system(), &machine, &mut block, util::now_ns() / 1_000_000, asker())?;
    println!("{line}");
    Ok(0)
}

/// The user who asked: the one `elevate` ran this for, when it is run as
/// root through it, else this process's own.
fn asker() -> u32 {
    let elevated = std::env::var("SUDO_UID").ok().and_then(|uid| uid.parse().ok());
    match util::own_user() {
        0 => elevated.unwrap_or(0),
        own => own,
    }
}

/// Asks for the test at the next start, if nothing else takes it, and
/// returns what to say. A test asked for already is not asked again.
fn ask(
    places: &Places,
    machine: &Machine,
    block: &mut dyn Block,
    now_ms: u64,
    by: u32,
) -> Result<String, Fail> {
    let line = asked_words(places.memory());
    if read_asked(&places.dir).is_some() && block.holds() == Some((MEMTEST_TRY, false)) {
        return Ok(line);
    }
    refuse(machine)?;
    let cmdline = fs::read_to_string(&places.cmdline)
        .map_err(|error| util::failed(&places.cmdline.display(), error))?;
    let kept: Vec<String> = trial::options_of(&cmdline)
        .split_ascii_whitespace()
        .filter(|word| !word.starts_with(OPTION))
        .map(String::from)
        .collect();
    let options = format!("{} {WORD}", kept.join(" "));
    let block_of = Trial::new(MEMTEST_TRY, machine.running, &options)
        .map_err(|why| fail(format!("the boot options cannot be passed to the test: {why}")))?
        .encode();
    let asked = Asked { format: 1, at_ms: now_ms, by, slot: machine.running.letter().to_string() };
    put(&places.dir, ASKED_NAME, &asked)
        .map_err(|error| util::failed(&places.dir.join(ASKED_NAME).display(), error))?;
    if let Err(why) = block.write(&block_of) {
        let _ = fs::remove_file(places.dir.join(ASKED_NAME));
        return Err(fail(why));
    }
    Ok(line)
}

/// What is said once the test is asked for: how long it takes, and how to
/// restart.
fn asked_words(memory: u64) -> String {
    let long = if memory == 0 {
        String::new()
    } else {
        let minutes = memory.div_ceil(GIB_PER_MINUTE << 30).clamp(1, 60);
        format!(
            " (about {minutes} minute{} for {})",
            if minutes == 1 { "" } else { "s" },
            size_words(memory, memory)
        )
    };
    format!(
        "hardware: the next start tests the free memory first{long}, then starts the system; \
         restart when it suits: `elevate shutdown -r now`"
    )
}

/// Refuses while the next start is another's: an update waits to be
/// tried, a search waits on a try, or the menu does not start the slot
/// that runs for a try.
///
/// # Errors
///
/// Why, with HW-105.
fn refuse(machine: &Machine) -> Result<(), Fail> {
    let taken = |why: String| fail(format!("The next start is taken already: {why} (HW-105)"));
    if let Some(slot) = machine.state.trying {
        return Err(taken(format!(
            "an update waits in slot {} to be tried, and the start menu starts it first",
            slot.letter().to_ascii_uppercase()
        )));
    }
    if let Some(number) = machine.search {
        return Err(taken(format!(
            "a search of changes bisect waits on try {number}; `elevate changes bisect --stop` \
             first"
        )));
    }
    let running = machine.running;
    if bisect::startable(machine.state, running.letter()).is_none() {
        return Err(taken(format!(
            "the start menu does not start slot {}, which runs now, for a test; it starts slot {}",
            running.letter().to_ascii_uppercase(),
            machine.state.default.letter().to_ascii_uppercase()
        )));
    }
    Ok(())
}

/// `--cancel`.
fn cancel() -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail(
            "this writes the start menu's block: run `elevate hardware memtest --cancel`".into(),
        ));
    }
    let root = place::root()?;
    let _lock = place::lock()?;
    let mut block = Partitions(place::partitions(&root)?);
    let line = cancel_in(&Places::system(), &mut block)?;
    println!("{line}");
    Ok(0)
}

/// Takes the asked file away, and the test's block while the menu has not
/// taken it; returns what to say.
fn cancel_in(places: &Places, block: &mut dyn Block) -> Result<String, Fail> {
    let asked = places.dir.join(ASKED_NAME);
    let was_asked = asked.exists();
    let held = block.holds() == Some((MEMTEST_TRY, false));
    if held {
        block.write(&trial::cleared()).map_err(fail)?;
    }
    match fs::remove_file(&asked) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(util::failed(&asked.display(), error)),
    }
    Ok(if was_asked || held {
        "hardware: the memory test at the next start is cancelled".into()
    } else {
        "hardware: no memory test was asked for the next start".into()
    })
}

/// `--status`.
fn status(json: bool) -> Result<i32, Fail> {
    let places = Places::system();
    let said = said_of(&places, &util::user_name);
    if json {
        let text = serde_json::to_string(&said)
            .map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        println!("{text}");
    } else {
        print!("{}", status_text(&said));
    }
    Ok(0)
}

/// What is asked and what was kept, with the name of the user who asked
/// from `name_of`.
fn said_of(places: &Places, name_of: &dyn Fn(u32) -> String) -> Status {
    let kept = read_kept(&places.dir);
    Status {
        format: 1,
        asked: read_asked(&places.dir).map(|asked| AskedSaid {
            at_ms: asked.at_ms,
            by: asked.by,
            by_name: name_of(asked.by),
        }),
        last: kept.last,
        history: kept.history,
    }
}

/// What a terminal shows of the status: whether a test is asked, the last
/// test and where it found errors.
fn status_text(status: &Status) -> String {
    let mut text = match &status.asked {
        Some(asked) => {
            let time =
                util::local_time(i64::try_from(asked.at_ms / 1000).unwrap_or(0), "%Y-%m-%d %H:%M");
            format!("asked for the next start ({time})\n")
        }
        None => "nothing asked\n".to_string(),
    };
    match &status.last {
        None => text.push_str("last: no test has run at a start\n"),
        Some(run) => {
            let date = util::local_time(i64::try_from(run.at_ms / 1000).unwrap_or(0), "%Y-%m-%d");
            let _ = writeln!(text, "last: {date}, start {}: {}", run.start, run_words(run));
            for bad in &run.bad {
                let _ = writeln!(
                    text,
                    "  {} to {}: {} error{}",
                    bad.from,
                    bad.to,
                    bad.errors,
                    if bad.errors == 1 { "" } else { "s" }
                );
            }
        }
    }
    text
}

/// What a run was, in words: how it ended, and what it tested and in how
/// long.
fn run_words(run: &Run) -> String {
    let tested = format!(
        "{} tested in {}",
        tested_words(run.tested_bytes, run.total_bytes),
        time_words(run.seconds)
    );
    match run.result {
        Ended::Passed => format!("passed, {tested}"),
        Ended::Errors => {
            let places = run.bad.len();
            format!(
                "{} error{} in {places} place{}, {tested}; {} pages are out of use until the \
                 next restart (HW-103)",
                run.errors,
                if run.errors == 1 { "" } else { "s" },
                if places == 1 { "" } else { "s" },
                run.retired
            )
        }
        Ended::Unfinished => "did not finish (HW-104)".into(),
        Ended::NotTaken => "not taken: an update was tried first".into(),
    }
}

/// `bytes`, in the unit `of` is in: `16 GiB`, `15.2 GiB`, `480 MiB`.
fn size_words(bytes: u64, of: u64) -> String {
    let (unit, name) = if of >= 1 << 30 { (1u64 << 30, "GiB") } else { (1 << 20, "MiB") };
    format!("{} {name}", number_words(bytes, unit))
}

/// `tested` of `total`, as one amount: `15.2 of 16 GiB`.
fn tested_words(tested: u64, total: u64) -> String {
    let (unit, name) = if total >= 1 << 30 { (1u64 << 30, "GiB") } else { (1 << 20, "MiB") };
    format!("{} of {} {name}", number_words(tested, unit), number_words(total, unit))
}

/// `bytes` over `unit`, with one decimal where it is not whole.
fn number_words(bytes: u64, unit: u64) -> String {
    let text = format!("{:.1}", bytes as f64 / unit as f64);
    match text.strip_suffix(".0") {
        Some(whole) => whole.to_string(),
        None => text,
    }
}

/// A time the test took: `less than a minute`, `4 minutes`.
fn time_words(seconds: u64) -> String {
    match (seconds + 30) / 60 {
        _ if seconds < 60 => "less than a minute".into(),
        0 | 1 => "1 minute".into(),
        minutes => format!("{minutes} minutes"),
    }
}

/// The kernel's result, as the test at this start. `None` when the text
/// is not one: no result, or one that cannot be read.
fn read_result(text: &str) -> Option<Run> {
    let mut run = Run {
        at_ms: 0,
        start: 0,
        result: Ended::Passed,
        complete: false,
        tested_bytes: 0,
        total_bytes: 0,
        errors: 0,
        bad: Vec::new(),
        retired: 0,
        seconds: 0,
    };
    let mut seen = 0u8;
    for line in text.lines() {
        let mut words = line.split_ascii_whitespace();
        let number = |word: Option<&str>| word.and_then(|word| word.parse::<u64>().ok());
        match words.next() {
            Some("result") => {
                run.result = match words.next()? {
                    "passed" => Ended::Passed,
                    "errors" => Ended::Errors,
                    _ => return None,
                };
                seen |= 1;
            }
            Some("complete") => {
                run.complete = match words.next()? {
                    "yes" => true,
                    "no" => false,
                    _ => return None,
                };
                seen |= 2;
            }
            Some("tested") => {
                run.tested_bytes = number(words.next())?;
                seen |= 4;
            }
            Some("total") => {
                run.total_bytes = number(words.next())?;
                seen |= 8;
            }
            Some("errors") => {
                run.errors = number(words.next())?;
                seen |= 16;
            }
            Some("seconds") => run.seconds = number(words.next())?,
            Some("retired") => run.retired = number(words.next())?,
            Some("bad") => {
                let (from, to) = (words.next()?, words.next()?);
                let errors = number(words.next())?;
                run.bad.push(Bad { from: from.into(), to: to.into(), errors });
            }
            _ => {}
        }
    }
    (seen == 31).then_some(run)
}

/// Keeps the test of this start, and what the start was asked to do, when
/// the health check has seen it through, under the slot state's lock: the
/// kernel's result kept, the alert raised or cleared, and what was asked
/// cleared, in each case: whether the test ran, did not finish, or was
/// never taken.
pub(crate) fn after_start(root: &Root, this: &Start) {
    // Most starts have neither: nothing to look for on the disks.
    let places = Places::system();
    if !places.result.exists() && !places.dir.join(ASKED_NAME).exists() {
        return;
    }
    let partitions = match place::partitions(root) {
        Ok(partitions) => partitions,
        Err(why) => {
            println!("hardware: cannot find the start menu's block: {}", why.message);
            Vec::new()
        }
    };
    let mut tell = |told: Told| {
        let sent = match &told {
            Told::Raise(alert) => client::raise_at_start(alert, ALERTD_WAIT),
            Told::Clear(key) => client::clear_at_start(key, ALERTD_WAIT),
            Told::Event(alert) => client::event_at_start(alert, Screen::Tell, ALERTD_WAIT),
        };
        if let Err(why) = sent {
            println!("hardware: cannot tell alertd of the memory test: {why}");
        }
    };
    after_start_in(
        &places,
        &mut Partitions(partitions),
        &mut tell,
        this,
        util::now_ns() / 1_000_000,
    );
}

/// [`after_start`] with its places, the block and the way to tell alertd
/// given, at `now_ms`.
fn after_start_in(
    places: &Places,
    block: &mut dyn Block,
    tell: &mut dyn FnMut(Told),
    this: &Start,
    now_ms: u64,
) {
    let asked = places.dir.join(ASKED_NAME).exists();
    let ran = fs::read_to_string(&places.result).ok().and_then(|text| read_result(&text));
    let kept = |ended: Ended| Run {
        at_ms: now_ms,
        start: this.number,
        result: ended,
        complete: false,
        tested_bytes: 0,
        total_bytes: 0,
        errors: 0,
        bad: Vec::new(),
        retired: 0,
        seconds: 0,
    };
    let run = match ran {
        Some(ran) => Some(Run { at_ms: now_ms, start: this.number, ..ran }),
        None if asked => {
            // The menu took the try and nothing came of it, or it never did.
            let taken =
                block.holds() == Some((MEMTEST_TRY, true)) || this.trial == Some(MEMTEST_TRY);
            Some(kept(if taken { Ended::Unfinished } else { Ended::NotTaken }))
        }
        None => None,
    };
    if let Some(run) = &run {
        keep(places, run);
        for told in alerts_of(run) {
            tell(told);
        }
    }
    if asked || run.is_some() {
        match fs::remove_file(places.dir.join(ASKED_NAME)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => println!("hardware: cannot clear what was asked: {error}"),
        }
        if block.holds().is_some()
            && let Err(why) = block.write(&trial::cleared())
        {
            println!("hardware: cannot clear the memory test from the start menu: {why}");
        }
    }
}

/// Keeps `run` as the last test, in the history of tests, and as a line of
/// the history of checks, when it ran or did not finish.
fn keep(places: &Places, run: &Run) {
    let mut kept = read_kept(&places.dir);
    kept.format = 1;
    kept.history.insert(0, run.clone());
    kept.history.truncate(HISTORY);
    kept.last = Some(run.clone());
    if let Err(error) = put(&places.dir, KEPT, &kept) {
        println!("hardware: cannot keep the result of the memory test: {error}");
    }
    let line = match run.result {
        Ended::Passed => Some((0, 1, 0, 0, Vec::new())),
        Ended::Errors => Some((1, 0, 0, 1, vec!["HW-103".to_string()])),
        Ended::Unfinished => Some((2, 0, 1, 0, vec!["HW-104".to_string()])),
        Ended::NotTaken => None,
    };
    if let Some((status, passed, warnings, failed, ids)) = line {
        let line = Line {
            at_ms: i64::try_from(run.at_ms).unwrap_or(0),
            full: false,
            status,
            passed,
            warnings,
            failed,
            skipped: 0,
            ids,
        };
        if let Err(error) = history::append(&places.dir, &line) {
            println!("hardware: cannot add the memory test to the history: {error}");
        }
    }
}

/// What a test at a start tells alertd: errors raise `memory-test`, a pass
/// clears it, a test that did not finish is told once.
fn alerts_of(run: &Run) -> Vec<Told> {
    match run.result {
        Ended::Passed => vec![Told::Clear("memory-test")],
        Ended::Errors => {
            let title = format!(
                "The memory test at the start found {} error{}",
                run.errors,
                if run.errors == 1 { "" } else { "s" }
            );
            let detail = "Those parts of the memory are out of use until the next restart. The \
                          memory should be replaced; `hardware memtest --status` lists where. \
                          (HW-103)";
            let alert = Alert::new("memory-test".into(), Severity::Critical, &title, detail);
            vec![Told::Raise(alert.about(About::Hardware))]
        }
        Ended::Unfinished => {
            let title = "The memory test at the start did not finish (HW-104)";
            let detail = "The power went or the machine stopped during it, which failing memory \
                          can cause. `hardware memtest --status` shows what the last test found.";
            let alert = Alert::new("memory-test".into(), Severity::Warning, title, detail);
            vec![Told::Event(alert.about(About::Hardware))]
        }
        Ended::NotTaken => Vec::new(),
    }
}

/// The asked file in `dir`, if it is there and reads.
fn read_asked(dir: &Path) -> Option<Asked> {
    serde_json::from_str(&fs::read_to_string(dir.join(ASKED_NAME)).ok()?).ok()
}

/// The kept results in `dir`; none where there are none, or the file is
/// not one.
fn read_kept(dir: &Path) -> Kept {
    fs::read_to_string(dir.join(KEPT))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Writes `value` as `name` in `dir`, whole or not at all, for anyone to
/// read.
fn put(dir: &Path, name: &str, value: &impl Serialize) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let text = serde_json::to_string(value).map_err(std::io::Error::other)?;
    saved::write_whole(dir, name, &text)?;
    fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o644))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use slots::envblock;
    use slots::trial::VARIABLES;

    use super::*;
    use crate::sysupdate::starts::Outcome;

    /// The block of the partitions, which a test holds in a variable.
    #[derive(Default)]
    struct Fake {
        block: Option<[u8; envblock::BLOCK]>,
        /// Why a write fails, when it does.
        fails: Option<String>,
    }

    impl Block for Fake {
        fn holds(&self) -> Option<(u8, bool)> {
            let (held, taken) = Trial::decode(self.block.as_ref()?)?;
            (held.number == MEMTEST_TRY).then_some((held.number, taken))
        }

        fn write(&mut self, block: &[u8]) -> Result<(), String> {
            if let Some(why) = &self.fails {
                return Err(why.clone());
            }
            self.block = block.try_into().ok();
            Ok(())
        }
    }

    impl Fake {
        /// The block of `trial` as the menu leaves it once it has taken it.
        fn taken(trial: &Trial) -> Fake {
            let number = trial.number.to_string();
            let check = trial.check();
            let block = envblock::write(
                &["# taken"],
                &[
                    (VARIABLES[0], &number),
                    (VARIABLES[1], trial.slot.letter()),
                    (VARIABLES[2], &trial.options),
                    (VARIABLES[3], &check),
                    (VARIABLES[4], &number),
                ],
            );
            Fake { block, fails: None }
        }

        /// The try the block holds, whatever its number.
        fn trial(&self) -> Option<Trial> {
            Trial::decode(self.block.as_ref()?).map(|(trial, _)| trial)
        }
    }

    /// A folder of the test's own.
    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hardware-memtest-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The places of a machine of 16 GiB that started with `cmdline`.
    fn places(name: &str, cmdline: &str) -> Places {
        let dir = folder(name);
        fs::write(dir.join("cmdline"), cmdline).unwrap();
        fs::write(dir.join("meminfo"), "total: 16777216 kB\nfree: 100 kB\n").unwrap();
        Places {
            dir: dir.join("state"),
            result: dir.join("memtest"),
            cmdline: dir.join("cmdline"),
            meminfo: dir.join("meminfo"),
        }
    }

    const CMDLINE: &str = "slot=a root=UUID=1234 panic=10 console=ttyS0,115200n8 quiet";

    /// A machine that runs slot A, the default, with slot B the version
    /// before, and nothing else waiting.
    fn machine() -> Machine {
        Machine {
            running: Slot::A,
            state: slots::State {
                generation: 0,
                default: Slot::A,
                previous: Some(Slot::B),
                trying: None,
                tries: 0,
                boot: None,
            },
            search: None,
        }
    }

    const AT: u64 = 1_794_564_000_000;

    fn start(number: u64, trial: Option<u8>) -> Start {
        Start {
            number,
            started: 1_794_564_000,
            slot: "a".into(),
            stage: "services".into(),
            outcome: Outcome::Good,
            error: None,
            why: None,
            failing: None,
            trial,
            items: Vec::new(),
            times: None,
        }
    }

    /// What the kernel leaves for a test that found `errors`.
    fn result(errors: u64, bad: &[(&str, &str, u64)]) -> String {
        let mut text = format!(
            "result {}\ncomplete yes\ntested 16319750144\ntotal 17179869184\nerrors {errors}\n\
             seconds 241\nretired {}\n",
            if errors == 0 { "passed" } else { "errors" },
            bad.len()
        );
        for (from, to, count) in bad {
            text.push_str(&format!("bad {from} {to} {count}\n"));
        }
        text
    }

    fn told(places: &Places, block: &mut Fake, this: &Start) -> Vec<Told> {
        let mut said = Vec::new();
        after_start_in(places, block, &mut |told| said.push(told), this, AT);
        said
    }

    #[test]
    fn the_test_is_asked_with_this_starts_options_and_the_word_and_says_how_long_it_takes() {
        let places = places("ask", &format!("{CMDLINE} memtest=fault"));
        let mut block = Fake::default();
        let said = ask(&places, &machine(), &mut block, AT, 1000).unwrap();
        assert_eq!(
            said,
            "hardware: the next start tests the free memory first (about 4 minutes for 16 GiB), \
             then starts the system; restart when it suits: `elevate shutdown -r now`"
        );
        let trial = block.trial().unwrap();
        assert_eq!(
            (trial.number, trial.slot, trial.options.as_str()),
            (99, Slot::A, "console=ttyS0,115200n8 quiet memtest=once")
        );
        assert_eq!(block.holds(), Some((99, false)));
        let asked: Asked =
            serde_json::from_str(&fs::read_to_string(places.dir.join(ASKED_NAME)).unwrap())
                .unwrap();
        assert_eq!(asked, Asked { format: 1, at_ms: AT, by: 1000, slot: "a".into() });
        // The file is for anyone to read.
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(places.dir.join(ASKED_NAME)).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o644);
        assert!(is_asked_in(&places));
    }

    /// Whether the asked file of `places` is there.
    fn is_asked_in(places: &Places) -> bool {
        places.dir.join(ASKED_NAME).exists()
    }

    #[test]
    fn asked_again_while_asked_says_the_same_and_writes_nothing() {
        let places = places("again", CMDLINE);
        let mut block = Fake::default();
        let first = ask(&places, &machine(), &mut block, AT, 1000).unwrap();
        let before = fs::read_to_string(places.dir.join(ASKED_NAME)).unwrap();
        // Another user, later, and a search that came meanwhile do not change it.
        let busy = Machine { search: Some(2), ..machine() };
        assert_eq!(ask(&places, &busy, &mut block, AT + 5000, 1001).unwrap(), first);
        assert_eq!(fs::read_to_string(places.dir.join(ASKED_NAME)).unwrap(), before);
        // A block that was lost is written again, and the asked time stays.
        block.block = None;
        assert_eq!(ask(&places, &machine(), &mut block, AT + 9000, 1001).unwrap(), first);
        assert_eq!(block.holds(), Some((99, false)));
    }

    #[test]
    fn the_start_is_not_asked_for_while_an_update_a_search_or_the_menu_has_it() {
        let refused = |machine: &Machine| {
            let places = places("refuse", CMDLINE);
            let mut block = Fake::default();
            let error = ask(&places, machine, &mut block, AT, 1000).unwrap_err();
            // Nothing is written by a refusal, and its status is 1.
            assert!(!is_asked_in(&places) && block.block.is_none());
            assert_eq!((error.code, error.usage), (1, false));
            error.message
        };
        let mut trying = machine();
        trying.state.trying = Some(Slot::B);
        assert_eq!(
            refused(&trying),
            "The next start is taken already: an update waits in slot B to be tried, and the \
             start menu starts it first (HW-105)"
        );
        let searching = Machine { search: Some(2), ..machine() };
        assert_eq!(
            refused(&searching),
            "The next start is taken already: a search of changes bisect waits on try 2; \
             `elevate changes bisect --stop` first (HW-105)"
        );
        let mut elsewhere = machine();
        elsewhere.state.default = Slot::B;
        elsewhere.state.previous = None;
        assert_eq!(
            refused(&elsewhere),
            "The next start is taken already: the start menu does not start slot A, which runs \
             now, for a test; it starts slot B (HW-105)"
        );
    }

    #[test]
    fn a_block_that_cannot_be_written_leaves_nothing_asked_and_says_why() {
        let places = places("nowrite", CMDLINE);
        let mut block = Fake {
            fails: Some("the root's disks have no EFI system partition".into()),
            ..Fake::default()
        };
        let error = ask(&places, &machine(), &mut block, AT, 1000).unwrap_err();
        assert_eq!(error.message, "the root's disks have no EFI system partition");
        assert_eq!(error.code, 1);
        assert!(!is_asked_in(&places));
        // Options the menu cannot pass on are said, not written.
        let odd = self::places("odd", "slot=a root=UUID=1 panic=10 a;b");
        let error = ask(&odd, &machine(), &mut Fake::default(), AT, 1000).unwrap_err();
        assert!(error.message.starts_with("the boot options cannot be passed to the test"));
        assert!(!is_asked_in(&odd));
    }

    #[test]
    fn a_start_with_a_memory_test_in_its_options_asks_for_one_word_of_it() {
        let places = places("word", "slot=a root=UUID=1 panic=10 memtest=once quiet");
        let mut block = Fake::default();
        ask(&places, &machine(), &mut block, AT, 0).unwrap();
        assert_eq!(block.trial().unwrap().options, "quiet memtest=once");
    }

    #[test]
    fn cancel_takes_the_asked_file_and_a_block_the_menu_has_not_taken() {
        let places = places("cancel", CMDLINE);
        let mut block = Fake::default();
        assert_eq!(
            cancel_in(&places, &mut block).unwrap(),
            "hardware: no memory test was asked for the next start"
        );
        ask(&places, &machine(), &mut block, AT, 1000).unwrap();
        assert_eq!(
            cancel_in(&places, &mut block).unwrap(),
            "hardware: the memory test at the next start is cancelled"
        );
        assert!(!is_asked_in(&places) && block.holds().is_none());
        assert_eq!(block.trial(), None, "the block is the one with no try");
    }

    #[test]
    fn cancel_leaves_a_block_the_menu_took_and_one_that_is_a_searchs() {
        let places = places("cancel-taken", CMDLINE);
        let trial = Trial::new(MEMTEST_TRY, Slot::A, "memtest=once").unwrap();
        let mut block = Fake::taken(&trial);
        put(&places.dir, ASKED_NAME, &Asked { format: 1, at_ms: AT, by: 0, slot: "a".into() })
            .unwrap();
        cancel_in(&places, &mut block).unwrap();
        assert_eq!(block.holds(), Some((99, true)), "the menu took it: it is not for us to clear");
        assert!(!is_asked_in(&places));
        // A search's try is never cleared by it.
        let theirs = Trial::new(2, Slot::A, "quiet").unwrap();
        let mut search = Fake { block: Some(theirs.encode()), fails: None };
        assert_eq!(
            cancel_in(&places, &mut search).unwrap(),
            "hardware: no memory test was asked for the next start"
        );
        assert_eq!(search.trial(), Some(theirs));
    }

    #[test]
    fn what_the_kernel_leaves_is_read_in_every_member_and_nothing_less_is() {
        let text = result(3, &[("0x1000000", "0x1002fff", 2), ("0x7f000000", "0x7f000fff", 1)]);
        let run = read_result(&text).unwrap();
        assert_eq!(
            run,
            Run {
                at_ms: 0,
                start: 0,
                result: Ended::Errors,
                complete: true,
                tested_bytes: 16_319_750_144,
                total_bytes: 17_179_869_184,
                errors: 3,
                bad: vec![
                    Bad { from: "0x1000000".into(), to: "0x1002fff".into(), errors: 2 },
                    Bad { from: "0x7f000000".into(), to: "0x7f000fff".into(), errors: 1 },
                ],
                retired: 2,
                seconds: 241,
            }
        );
        assert_eq!(read_result(&result(0, &[])).unwrap().result, Ended::Passed);
        // A member missing or not a number, or a word the kernel does not say.
        for bad in [
            "",
            "result passed\n",
            &text.replace("tested 16319750144\n", ""),
            &text.replace("result errors", "result maybe"),
            &text.replace("complete yes", "complete soon"),
            &text.replace("errors 3", "errors many"),
            &text.replace("bad 0x1000000 0x1002fff 2", "bad 0x1000000"),
        ] {
            assert_eq!(read_result(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_test_that_passed_is_kept_clears_the_alert_and_what_was_asked() {
        let places = places("passed", CMDLINE);
        let trial = Trial::new(MEMTEST_TRY, Slot::A, "quiet memtest=once").unwrap();
        let mut block = Fake::taken(&trial);
        put(
            &places.dir,
            ASKED_NAME,
            &Asked { format: 1, at_ms: AT - 600_000, by: 1000, slot: "a".into() },
        )
        .unwrap();
        fs::write(&places.result, result(0, &[])).unwrap();
        let said = told(&places, &mut block, &start(212, Some(99)));
        assert_eq!(said, [Told::Clear("memory-test")]);
        assert!(!is_asked_in(&places) && block.holds().is_none());
        let kept = read_kept(&places.dir);
        let last = kept.last.unwrap();
        assert_eq!(
            (last.at_ms, last.start, last.result, last.complete, last.errors),
            (AT, 212, Ended::Passed, true, 0)
        );
        assert_eq!(kept.history, [last]);
        // A line of the history of checks, for the page's days.
        let lines = history::read(&places.dir.join("history")).unwrap();
        assert_eq!(
            lines,
            [Line {
                at_ms: AT as i64,
                full: false,
                status: 0,
                passed: 1,
                warnings: 0,
                failed: 0,
                skipped: 0,
                ids: Vec::new()
            }]
        );
    }

    #[test]
    fn a_test_that_found_errors_raises_a_critical_alert_that_says_where_to_look() {
        let places = places("errors", CMDLINE);
        let mut block = Fake::default();
        fs::write(
            &places.result,
            result(3, &[("0x1000000", "0x1002fff", 2), ("0x7f000000", "0x7f000fff", 1)]),
        )
        .unwrap();
        // Run by hand, with `memtest=` given at the menu: nothing was asked.
        let said = told(&places, &mut block, &start(213, None));
        let [Told::Raise(alert)] = said.as_slice() else { panic!("{said:?}") };
        assert_eq!((alert.key.as_str(), alert.severity), ("memory-test", Severity::Critical));
        assert_eq!(alert.kind(), Some(About::Hardware));
        assert_eq!(alert.title, "The memory test at the start found 3 errors");
        assert_eq!(
            alert.detail,
            "Those parts of the memory are out of use until the next restart. The memory should \
             be replaced; `hardware memtest --status` lists where. (HW-103)"
        );
        let kept = read_kept(&places.dir);
        assert_eq!(kept.last.as_ref().map(|run| (run.errors, run.bad.len())), Some((3, 2)));
        let lines = history::read(&places.dir.join("history")).unwrap();
        assert_eq!(
            (lines[0].status, lines[0].failed, &lines[0].ids[..]),
            (1, 1, &["HW-103".to_string()][..])
        );
        // One error says "error".
        let one = alerts_of(&Run { errors: 1, ..kept.last.unwrap() });
        let [Told::Raise(alert)] = one.as_slice() else { panic!("{one:?}") };
        assert_eq!(alert.title, "The memory test at the start found 1 error");
    }

    #[test]
    fn a_test_the_menu_took_that_never_told_is_unfinished_and_one_it_never_took_is_not_taken() {
        let ask_for = |name: &str| {
            let places = places(name, CMDLINE);
            put(&places.dir, ASKED_NAME, &Asked { format: 1, at_ms: AT, by: 0, slot: "a".into() })
                .unwrap();
            places
        };
        let trial = Trial::new(MEMTEST_TRY, Slot::A, "memtest=once").unwrap();

        // The block says the menu took it, and this is the usual start.
        let places = ask_for("unfinished");
        let mut block = Fake::taken(&trial);
        let said = told(&places, &mut block, &start(5, None));
        let [Told::Event(alert)] = said.as_slice() else { panic!("{said:?}") };
        assert_eq!((alert.key.as_str(), alert.severity), ("memory-test", Severity::Warning));
        assert_eq!(alert.kind(), Some(About::Hardware));
        assert_eq!(alert.title, "The memory test at the start did not finish (HW-104)");
        let last = read_kept(&places.dir).last.unwrap();
        assert_eq!(
            (last.result, last.complete, last.tested_bytes, last.seconds),
            (Ended::Unfinished, false, 0, 0)
        );
        assert!(!is_asked_in(&places) && block.holds().is_none());
        let lines = history::read(&places.dir.join("history")).unwrap();
        assert_eq!(
            (lines[0].status, lines[0].warnings, &lines[0].ids[..]),
            (2, 1, &["HW-104".to_string()][..])
        );

        // The kernel took the start as the test and left no result.
        let places = ask_for("no-result");
        let mut block = Fake { block: Some(trial.encode()), fails: None };
        told(&places, &mut block, &start(6, Some(99)));
        assert_eq!(read_kept(&places.dir).last.unwrap().result, Ended::Unfinished);

        // The block was never taken: an update was tried first.
        let places = ask_for("not-taken");
        let mut block = Fake { block: Some(trial.encode()), fails: None };
        let said = told(&places, &mut block, &start(7, None));
        assert!(said.is_empty());
        assert_eq!(read_kept(&places.dir).last.unwrap().result, Ended::NotTaken);
        assert!(!is_asked_in(&places) && block.holds().is_none(), "asked and block are cleared");
        // It is not a check that ran: no line in the history of checks.
        assert!(history::read(&places.dir.join("history")).unwrap().is_empty());

        // The block was lost too.
        let places = ask_for("lost");
        let said = told(&places, &mut Fake::default(), &start(8, None));
        assert!(said.is_empty());
        assert_eq!(read_kept(&places.dir).last.unwrap().result, Ended::NotTaken);
    }

    #[test]
    fn a_start_with_nothing_asked_and_no_result_keeps_and_clears_nothing() {
        let places = places("nothing", CMDLINE);
        // A search's try in the block is not ours to clear.
        let theirs = Trial::new(2, Slot::A, "quiet").unwrap();
        let mut block = Fake { block: Some(theirs.encode()), fails: None };
        assert!(told(&places, &mut block, &start(9, None)).is_empty());
        assert_eq!(block.trial(), Some(theirs));
        assert!(!places.dir.join(KEPT).exists());
        // Nor is a block someone wrote by hand with nothing asked.
        let ours = Trial::new(MEMTEST_TRY, Slot::A, "memtest=once").unwrap();
        let mut block = Fake { block: Some(ours.encode()), fails: None };
        assert!(told(&places, &mut block, &start(9, None)).is_empty());
        assert_eq!(block.holds(), Some((99, false)));
        assert!(!places.dir.join(KEPT).exists());
    }

    #[test]
    fn the_history_keeps_ten_tests_the_newest_first() {
        let places = places("history", CMDLINE);
        for number in 1..=12u64 {
            fs::write(&places.result, result(0, &[])).unwrap();
            after_start_in(
                &places,
                &mut Fake::default(),
                &mut |_| {},
                &start(number, None),
                AT + number,
            );
        }
        let kept = read_kept(&places.dir);
        let starts: Vec<u64> = kept.history.iter().map(|run| run.start).collect();
        assert_eq!(starts, [12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
        assert_eq!(kept.last.map(|run| run.start), Some(12));
    }

    #[test]
    fn a_file_that_is_not_a_result_of_ours_is_no_history() {
        let places = places("foreign", CMDLINE);
        fs::create_dir_all(&places.dir).unwrap();
        fs::write(places.dir.join(KEPT), "{\"format\":1,\"last\":7}").unwrap();
        fs::write(places.dir.join(ASKED_NAME), "{}").unwrap();
        assert_eq!(read_kept(&places.dir), Kept::default());
        assert_eq!(read_asked(&places.dir), None);
    }

    /// The status of a machine with a test asked and two kept.
    fn busy(name: &str) -> Places {
        let places = places(name, CMDLINE);
        let mut block = Fake::default();
        for (number, text) in [
            (210, result(0, &[])),
            (212, result(3, &[("0x1000000", "0x1002fff", 2), ("0x7f000000", "0x7f000fff", 1)])),
        ] {
            fs::write(&places.result, text).unwrap();
            after_start_in(&places, &mut block, &mut |_| {}, &start(number, None), AT + number);
        }
        ask(&places, &machine(), &mut block, AT + 1000, 1000).unwrap();
        places
    }

    #[test]
    fn the_status_json_has_what_is_asked_the_last_test_and_the_ones_before() {
        let places = busy("json");
        let said = said_of(&places, &|uid| format!("user{uid}"));
        let value = serde_json::to_value(&said).unwrap();
        assert_eq!(value["format"], 1);
        assert_eq!(
            value["asked"],
            serde_json::json!({"at_ms": AT + 1000, "by": 1000, "by_name": "user1000"})
        );
        assert_eq!(value["last"]["result"], "errors");
        assert_eq!(value["last"]["start"], 212);
        assert_eq!(
            value["last"]["bad"][0],
            serde_json::json!({"from": "0x1000000", "to": "0x1002fff", "errors": 2})
        );
        let members: Vec<&str> =
            value["last"].as_object().unwrap().keys().map(String::as_str).collect();
        for member in [
            "at_ms",
            "start",
            "result",
            "complete",
            "tested_bytes",
            "total_bytes",
            "errors",
            "bad",
            "retired",
            "seconds",
        ] {
            assert!(members.contains(&member), "{member}");
        }
        assert_eq!(value["history"].as_array().unwrap().len(), 2);
        assert_eq!(value["history"][1]["start"], 210);
        // The members come in the order the design gives them.
        let text = serde_json::to_string(&said).unwrap();
        assert!(text.starts_with("{\"format\":1,\"asked\":{\"at_ms\":"), "{text}");
        assert!(text.contains("\"last\":{\"at_ms\":"), "{text}");
    }

    #[test]
    fn with_nothing_asked_and_nothing_kept_the_status_is_null_null_and_empty() {
        let places = places("empty", CMDLINE);
        let said = said_of(&places, &|uid| uid.to_string());
        assert_eq!(
            serde_json::to_string(&said).unwrap(),
            "{\"format\":1,\"asked\":null,\"last\":null,\"history\":[]}"
        );
        assert_eq!(status_text(&said), "nothing asked\nlast: no test has run at a start\n");
    }

    #[test]
    fn the_status_text_says_what_is_asked_and_where_the_last_test_found_errors() {
        let places = busy("text");
        let text = status_text(&said_of(&places, &|uid| uid.to_string()));
        let asked = util::local_time(i64::try_from((AT + 1000) / 1000).unwrap(), "%Y-%m-%d %H:%M");
        let date = util::local_time(i64::try_from((AT + 212) / 1000).unwrap(), "%Y-%m-%d");
        assert_eq!(
            text,
            format!(
                "asked for the next start ({asked})\n\
                 last: {date}, start 212: 3 errors in 2 places, 15.2 of 16 GiB tested in 4 \
                 minutes; 2 pages are out of use until the next restart (HW-103)\n\
                 \x20 0x1000000 to 0x1002fff: 2 errors\n\
                 \x20 0x7f000000 to 0x7f000fff: 1 error\n"
            )
        );
    }

    #[test]
    fn each_end_of_a_test_has_its_words() {
        let run = |result: Ended, tested: u64, total: u64, seconds: u64| Run {
            at_ms: 0,
            start: 1,
            result,
            complete: true,
            tested_bytes: tested,
            total_bytes: total,
            errors: 0,
            bad: Vec::new(),
            retired: 0,
            seconds,
        };
        assert_eq!(
            run_words(&run(Ended::Passed, 16_319_750_144, 17_179_869_184, 241)),
            "passed, 15.2 of 16 GiB tested in 4 minutes"
        );
        assert_eq!(
            run_words(&run(Ended::Passed, 480 << 20, 512 << 20, 45)),
            "passed, 480 of 512 MiB tested in less than a minute"
        );
        assert_eq!(
            run_words(&run(Ended::Passed, 4 << 30, 4 << 30, 60)),
            "passed, 4 of 4 GiB tested in 1 minute"
        );
        assert_eq!(run_words(&run(Ended::Unfinished, 0, 0, 0)), "did not finish (HW-104)");
        assert_eq!(
            run_words(&run(Ended::NotTaken, 0, 0, 0)),
            "not taken: an update was tried first"
        );
        let one = Run {
            errors: 1,
            retired: 1,
            bad: vec![Bad { from: "0x1".into(), to: "0x2".into(), errors: 1 }],
            ..run(Ended::Errors, 1 << 30, 1 << 30, 100)
        };
        assert_eq!(
            run_words(&one),
            "1 error in 1 place, 1 of 1 GiB tested in 2 minutes; 1 pages are out of use until \
             the next restart (HW-103)"
        );
    }

    #[test]
    fn the_time_is_a_minute_for_each_4_gib_at_least_one_and_at_most_an_hour() {
        let said = |bytes: u64| asked_words(bytes);
        assert!(said(512 << 20).contains("(about 1 minute for 512 MiB)"), "{}", said(512 << 20));
        assert!(said(16 << 30).contains("(about 4 minutes for 16 GiB)"));
        assert!(said(17 << 30).contains("(about 5 minutes for 17 GiB)"));
        assert!(said(512 << 30).contains("(about 60 minutes for 512 GiB)"));
        // Memory that cannot be read says nothing of the time.
        assert!(said(0).starts_with("hardware: the next start tests the free memory first, then"));
    }

    #[test]
    fn the_command_line_takes_one_of_the_three_and_json_only_for_the_status() {
        let run = |items: &[&str]| {
            main(&mut Args::new("hardware", items.iter().map(OsString::from).collect()))
        };
        for wrong in [
            &[][..],
            &["--json"],
            &["--next-start", "--cancel"],
            &["--next-start", "--json"],
            &["--cancel", "--json"],
            &["--status", "--status", "--cancel"],
            &["--now"],
            &["soon"],
        ] {
            let error = run(wrong).unwrap_err();
            assert!(error.usage, "{wrong:?}: {}", error.message);
        }
        assert!(run(&["--cancel", "--next-start"]).unwrap_err().usage);
    }
}
