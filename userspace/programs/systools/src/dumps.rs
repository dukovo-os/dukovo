// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dumps(8): the kernel's crash dumps (ADR 0483).
//!
//! When the kernel panics it writes a text dump of what it knew into the
//! RAM that the crash log keeps across the restart, and the next boot
//! gives that dump, then the crashed boot's whole console log, as
//! `/devices/crashdump` (radix-debug(7)). `dumps save` copies it to
//! [`FOLDER`]; the `dumps` service runs it at each boot, right after
//! logd, so that its warning is in the log. `list`, `show`, `path` and
//! `delete` work on the saved files, and `dumps test` panics the machine
//! on purpose, to check once that the whole chain works.
//!
//! Only root reads a dump, which holds what the kernel held: the device
//! and the saved files are mode 0600. When a dump is missing, the kernel's
//! `Crash log:` lines at boot say whether one was kept, and the log's
//! `dumps:` lines say what was done with it. The boot test `crash-dump`
//! covers the tool.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::alerts::protocol::{Alert, Severity};
use crate::args::{Arg, Args};
use crate::logd;
use crate::prompt::{self, Answer};
use crate::ps::print_table;
use crate::util::{self, Fail, fail, usage};

/// The previous boot's dump, then that boot's whole log.
const DEVICE: &str = "/devices/crashdump";

/// Where the dumps are saved.
const FOLDER: &str = "/system/state/crash";
/// How long `dumps save` waits for alertd, which starts after it, to
/// tell it of the panic.
const ALERTD_WAIT: Duration = Duration::from_secs(60);

/// How many saved dumps are kept, and the most bytes they take together.
const KEEP_DUMPS: usize = 10;
const KEEP_BYTES: u64 = 64 << 20;

/// How much of a saved dump `list` reads: its header and the start of its
/// panic report come first.
const HEAD: u64 = 4096;

const HELP: &str = "Usage: dumps [list]
       dumps show [NAME]
       dumps path [NAME]
       dumps delete NAME... | --all
       dumps save
       dumps test [--yes]

Saves, lists, shows and deletes the kernel's crash dumps, as dumps(8)
describes.

  list     the saved dumps, newest first
  show     one dump, the newest without a NAME, through $PAGER on a terminal
  path     where a dump's file is
  delete   the named dumps, or every one with --all
  save     copies the previous boot's dump from the kernel; the dumps
           service runs it at each boot
  test     panics the kernel on purpose, after asking, to check that dumps
           are saved; --yes goes on without asking";

/// dumps(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let folder = Path::new(FOLDER);
    let Some(subcommand) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(name)) if name == "help" => {
                println!("{HELP}");
                Ok(0)
            }
            Some(other) => Err(args.unknown(&other)),
            None => {
                root_only()?;
                list(folder)
            }
        };
    };
    match subcommand.to_str() {
        Some("list") => {
            root_only()?;
            nothing_more(args)?;
            list(folder)
        }
        Some("show") => {
            root_only()?;
            show(folder, name_operand(args)?.as_deref())
        }
        Some("path") => {
            root_only()?;
            path(folder, name_operand(args)?.as_deref())
        }
        Some("delete") => {
            root_only()?;
            delete(args, folder)
        }
        Some("save") => {
            root_only()?;
            nothing_more(args)?;
            save(folder)
        }
        Some("test") => {
            root_only()?;
            test_panic(args)
        }
        _ => Err(usage(format!("no such command: {}\n{HELP}", subcommand.to_string_lossy()))),
    }
}

/// Fails unless the program runs as root, whose dumps these are.
fn root_only() -> Result<(), Fail> {
    if util::own_user() == 0 {
        Ok(())
    } else {
        Err(fail("only root reads the crash dumps; try elevate dumps".into()))
    }
}

/// Fails if anything is left in `args`.
fn nothing_more(args: &mut Args) -> Result<(), Fail> {
    match args.next()? {
        None => Ok(()),
        Some(other) => Err(args.unknown(&other)),
    }
}

/// The dump's name that `args` ends with, if it has one.
fn name_operand(args: &mut Args) -> Result<Option<String>, Fail> {
    let mut name = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(name)
}

/// What the start of a dump says.
#[derive(Debug, PartialEq, Eq)]
struct Summary {
    /// The number of the boot that panicked.
    boot: u32,
    /// When it panicked, as RFC 3339 in UTC to the second; none when the
    /// kernel did not know the time of day.
    panicked_at: Option<String>,
    /// What the kernel said was wrong: the first line of its panic report
    /// without the CPU and the time that start it.
    panic: String,
}

impl Summary {
    /// What the start of `dump` says, or why it is no crash dump. The dump
    /// is read as the kernel writes it: three lines, then sections, each a
    /// blank line and `== Name`, the first of them `== Panic`.
    fn of(dump: &[u8]) -> Result<Summary, &'static str> {
        let mut lines = dump.split(|&byte| byte == b'\n').map(String::from_utf8_lossy);
        let (Some(title), Some(version), Some(when)) = (lines.next(), lines.next(), lines.next())
        else {
            return Err("it is shorter than a dump's header");
        };
        if title != "Radix crash dump" {
            return Err("it does not start with `Radix crash dump`");
        }
        let boot = version
            .rsplit_once(", boot ")
            .and_then(|(_, boot)| boot.parse().ok())
            .ok_or("its second line does not give the boot's number")?;
        let when = when
            .strip_prefix("Panic at ")
            .and_then(|when| when.split_once(", "))
            .ok_or("its third line does not say when it panicked")?
            .0;
        let panicked_at = if when == "an unknown time of day" {
            None
        } else if shaped(when, "####-##-##T##:##:##Z") {
            Some(when.to_string())
        } else {
            return Err("its third line does not say when it panicked");
        };
        let report = lines
            .skip_while(|line| *line != "== Panic")
            .nth(1)
            .ok_or("it has no `== Panic` section")?;
        // The report's first line is `Radix panic on CPU N at S.US s: MESSAGE`.
        let message = report
            .strip_prefix("Radix panic on CPU ")
            .and_then(|rest| rest.split_once(" s: "))
            .map_or(&*report, |(_, message)| message);
        Ok(Summary { boot, panicked_at, panic: message.to_string() })
    }
}

/// Whether `text` has the shape `pattern`, where `#` stands for a digit
/// and any other character for itself.
fn shaped(text: &str, pattern: &str) -> bool {
    text.len() == pattern.len()
        && text
            .bytes()
            .zip(pattern.bytes())
            .all(|(byte, shape)| if shape == b'#' { byte.is_ascii_digit() } else { byte == shape })
}

/// The 64-bit FNV-1a hash of `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The name `dump` is saved under: the time of its panic and its boot's
/// number, as `2026-11-13T10-02-11Z-boot42.txt`. A dump whose time the
/// kernel did not know has the low 32 bits of the FNV-1a hash of the whole
/// dump after its boot's number: the boot number alone is not unique, as
/// it starts again at 1 when RAM was not kept, and the same dump gets the
/// same name, so saving it twice saves it once.
fn name_of(summary: &Summary, dump: &[u8]) -> String {
    match &summary.panicked_at {
        Some(time) => format!("{}-boot{}.txt", time.replace(':', "-"), summary.boot),
        None => {
            format!("unknown-time-boot{}-{:08x}.txt", summary.boot, fnv1a(dump) & 0xffff_ffff)
        }
    }
}

/// Whether `name` is one [`name_of`] gives. A file of the folder that is
/// not is none of dumps's to list or delete.
fn is_dump_name(name: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let Some(stem) = name.strip_suffix(".txt") else { return false };
    if let Some(rest) = stem.strip_prefix("unknown-time-boot") {
        let Some((boot, hash)) = rest.split_once('-') else { return false };
        return digits(boot)
            && hash.len() == 8
            && hash.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    }
    stem.split_once("Z-boot")
        .is_some_and(|(time, boot)| shaped(time, "####-##-##T##-##-##") && digits(boot))
}

/// A saved dump: a file of the folder.
#[derive(Debug)]
struct Saved {
    name: String,
    bytes: u64,
    /// When the file was written, which puts the newest first.
    written: SystemTime,
}

/// The dumps saved in `folder`, newest first; none if it is not there.
fn saved_in(folder: &Path) -> Result<Vec<Saved>, Fail> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(util::failed(&folder.display(), err)),
    };
    let mut saved = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| util::failed(&folder.display(), err))?;
        let Ok(name) = entry.file_name().into_string() else { continue };
        if !is_dump_name(&name) {
            continue;
        }
        let meta = entry.metadata().map_err(|err| util::failed(&name, err))?;
        if !meta.is_file() {
            continue;
        }
        let written = meta.modified().map_err(|err| util::failed(&name, err))?;
        saved.push(Saved { name, bytes: meta.len(), written });
    }
    saved.sort_by(|a, b| b.written.cmp(&a.written).then_with(|| b.name.cmp(&a.name)));
    Ok(saved)
}

/// The dump of `saved` called `name`, or the newest if there is no name.
fn find<'a>(saved: &'a [Saved], name: Option<&str>) -> Result<&'a Saved, Fail> {
    match name {
        None => saved.first().ok_or_else(|| fail("no crash dumps saved".into())),
        Some(name) => saved.iter().find(|dump| dump.name == name).ok_or_else(|| {
            fail(format!("no saved dump is called {name}; `dumps list` names the saved ones"))
        }),
    }
}

/// `dumps list`: a table of the saved dumps.
fn list(folder: &Path) -> Result<i32, Fail> {
    let saved = saved_in(folder)?;
    if saved.is_empty() {
        println!("no crash dumps saved");
        return Ok(0);
    }
    let mut table = vec![["NAME", "TIME", "BOOT", "SIZE", "PANIC"].map(String::from).to_vec()];
    for dump in &saved {
        let mut head = Vec::new();
        File::open(folder.join(&dump.name))
            .and_then(|file| file.take(HEAD).read_to_end(&mut head))
            .map_err(|err| util::failed(&dump.name, err))?;
        table.push(row(dump, &head));
    }
    let width = if util::is_terminal(1) {
        util::terminal_size(1).map(|(columns, _)| columns)
    } else {
        None
    };
    print_table(&table, &[false, false, true, true, false], width)?;
    Ok(0)
}

/// The line of `list`'s table for `dump`, whose file starts with `head`.
fn row(dump: &Saved, head: &[u8]) -> Vec<String> {
    let size = util::human(dump.bytes, false);
    match Summary::of(head) {
        Ok(summary) => {
            let time = summary.panicked_at.as_deref().and_then(logd::parse_rfc3339).map_or_else(
                || "unknown".to_string(),
                |ns| util::local_time((ns / 1_000_000_000) as i64, "%Y-%m-%d %H:%M"),
            );
            vec![dump.name.clone(), time, summary.boot.to_string(), size, summary.panic]
        }
        Err(why) => {
            vec![dump.name.clone(), "?".into(), "?".into(), size, format!("not a dump: {why}")]
        }
    }
}

/// `dumps path`: where the dump `name` names, or the newest, is.
fn path(folder: &Path, name: Option<&str>) -> Result<i32, Fail> {
    let saved = saved_in(folder)?;
    println!("{FOLDER}/{}", find(&saved, name)?.name);
    Ok(0)
}

/// `dumps show`: the dump `name` names, or the newest, through the pager.
fn show(folder: &Path, name: Option<&str>) -> Result<i32, Fail> {
    let saved = saved_in(folder)?;
    let dump = find(&saved, name)?;
    let text = fs::read(folder.join(&dump.name)).map_err(|err| util::failed(&dump.name, err))?;
    page(&text)?;
    Ok(0)
}

/// Writes `text` through the pager, `$PAGER` or else `less`, when the
/// standard output is a terminal, and straight to it otherwise or when
/// the pager cannot be started.
fn page(text: &[u8]) -> Result<(), Fail> {
    if util::is_terminal(1) {
        let pager = std::env::var("PAGER").ok().filter(|pager| !pager.trim().is_empty());
        let mut words = pager.as_deref().unwrap_or("less").split_whitespace();
        if let Some(program) = words.next()
            && let Ok(mut child) = Command::new(program).args(words).stdin(Stdio::piped()).spawn()
        {
            if let Some(mut input) = child.stdin.take() {
                // A pager the person quit before its end has closed its input.
                let _ = input.write_all(text);
            }
            child.wait()?;
            return Ok(());
        }
    }
    io::stdout().write_all(text)?;
    Ok(())
}

/// `dumps delete`: the dumps the operands name, or all of them.
fn delete(args: &mut Args, folder: &Path) -> Result<i32, Fail> {
    let (mut all, mut names) = (false, Vec::new());
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "all" => all = true,
            Arg::Operand(name) => names.push(name.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    // Names or --all: neither is nothing to delete, and both is a doubt.
    let named = !names.is_empty();
    if all == named {
        return Err(usage("delete takes the names of dumps, or --all alone".into()));
    }
    for name in delete_in(folder, &names, all)? {
        println!("Deleted {name}");
    }
    Ok(0)
}

/// Deletes the dumps of `folder` called `names`, or every one if `all`,
/// and gives their names. A name that is no saved dump's fails before any
/// is deleted.
fn delete_in(folder: &Path, names: &[String], all: bool) -> Result<Vec<String>, Fail> {
    let saved = saved_in(folder)?;
    let mut doomed: Vec<&Saved> = if all {
        saved.iter().collect()
    } else {
        names.iter().map(|name| find(&saved, Some(name))).collect::<Result<_, _>>()?
    };
    doomed.sort_by(|a, b| a.name.cmp(&b.name));
    doomed.dedup_by(|a, b| a.name == b.name);
    doomed
        .into_iter()
        .map(|dump| {
            fs::remove_file(folder.join(&dump.name))
                .map_err(|err| util::failed(&dump.name, err))
                .map(|()| dump.name.clone())
        })
        .collect()
}

/// What `dumps save` did.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The dump is a new file of this name; the kernel's report said this.
    Saved { name: String, panic: String },
    /// The file of this name was there.
    AlreadySaved(String),
}

/// `dumps save`: copies the previous boot's dump from the kernel into the
/// folder, and keeps only the newest dumps there.
fn save(folder: &Path) -> Result<i32, Fail> {
    let dump = fs::read(DEVICE).map_err(|err| util::failed(&DEVICE, err))?;
    if dump.is_empty() {
        return Ok(0);
    }
    let (name, panic) = match save_in(folder, &dump)? {
        Outcome::Saved { name, panic } => (name, panic),
        Outcome::AlreadySaved(name) => {
            println!("dumps: the previous boot's dump is already saved as {name}");
            return Ok(0);
        }
    };
    // A `<4>` in front makes the line a warning in the log, where Spark
    // passes it on to the kernel's; a person who runs the command would
    // see it, so a terminal gets the line without it.
    let level = if util::is_terminal(2) { "" } else { "<4>" };
    eprintln!(
        "{level}dumps: the previous boot ended with a kernel panic: {panic}; the dump is \
         {FOLDER}/{name} (dumps show)"
    );
    prune(folder, &name)?;
    let detail = format!("{panic}\nThe dump is {FOLDER}/{name}; `elevate dumps show` shows it.");
    let alert = Alert::new(
        "kernel-panic".into(),
        Severity::Critical,
        "The machine restarted after a kernel panic",
        &detail,
    );
    // This runs early in the start, before alertd: it waits for it.
    if let Err(why) = crate::alerts::client::event_at_start(&alert, ALERTD_WAIT) {
        eprintln!("dumps: cannot tell alertd: {why}");
    }
    Ok(0)
}

/// Saves `dump`, as the kernel gave it, in `folder` unless it is there.
fn save_in(folder: &Path, dump: &[u8]) -> Result<Outcome, Fail> {
    let summary = Summary::of(dump).map_err(|why| {
        fail(format!("{DEVICE} holds no crash dump: {why}; `cat {DEVICE}` shows what it holds"))
    })?;
    let name = name_of(&summary, dump);
    let path = folder.join(&name);
    if path.exists() {
        return Ok(Outcome::AlreadySaved(name));
    }
    // The folder may be there already, made with another mode before any
    // dump was saved in it: it is made root's alone either way.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .and_then(|()| fs::set_permissions(folder, fs::Permissions::from_mode(0o700)))
        .map_err(|err| util::failed(&folder.display(), err))?;
    // To a new file beside it first, made to last, then put in its place,
    // as authd does for its files, so that a file of the folder is whole
    // whatever happens.
    let mut new = path.as_os_str().to_owned();
    new.push(".new");
    let new = Path::new(&new);
    let written = (|| {
        let mut file =
            OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(new)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(dump)?;
        file.sync_all()?;
        fs::rename(new, &path)?;
        File::open(folder)?.sync_all()
    })();
    written.map_err(|err| {
        // After a failure of the last step the new file is already the dump's.
        let _ = fs::remove_file(new);
        util::failed(&path.display(), err)
    })?;
    Ok(Outcome::Saved { name, panic: summary.panic })
}

/// Deletes the saved dumps past [`KEEP_DUMPS`] and [`KEEP_BYTES`], the
/// oldest first, but not `newest`, the one just saved.
fn prune(folder: &Path, newest: &str) -> Result<(), Fail> {
    let saved = saved_in(folder)?;
    for old in surplus(&saved, newest) {
        fs::remove_file(folder.join(&old.name)).map_err(|err| util::failed(&old.name, err))?;
    }
    Ok(())
}

/// The dumps of `saved`, newest first, that go: those past the tenth, and
/// those from the one that takes the total past 64 MiB, but never
/// `newest`.
fn surplus<'a>(saved: &'a [Saved], newest: &str) -> Vec<&'a Saved> {
    let (mut count, mut bytes, mut doomed) = (0, 0u64, Vec::new());
    for dump in saved {
        count += 1;
        bytes = bytes.saturating_add(dump.bytes);
        if (count > KEEP_DUMPS || bytes > KEEP_BYTES) && dump.name != newest {
            doomed.push(dump);
        }
    }
    doomed
}

/// `dumps test`: asks the kernel to panic.
fn test_panic(args: &mut Args) -> Result<i32, Fail> {
    let mut yes = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "yes" => yes = true,
            other => return Err(args.unknown(&other)),
        }
    }
    if !yes {
        confirm()?;
    }
    let mut device =
        OpenOptions::new().write(true).open(DEVICE).map_err(|err| util::failed(&DEVICE, err))?;
    // What the filesystems hold in memory would be lost with the panic, as
    // at any crash, so they write it out first, as sync does.
    File::open("/").and_then(|root| root.sync_all()).map_err(|err| {
        fail(format!(
            "cannot write the filesystems' changes out first ({}), so the kernel was not asked \
             to panic",
            util::strip_os_error(&err)
        ))
    })?;
    // The kernel panics inside this write, which it takes only whole.
    device.write_all(b"panic\n").map_err(|err| util::failed(&DEVICE, err))?;
    Ok(0)
}

/// Asks at the terminal for `panic` to be typed.
fn confirm() -> Result<(), Fail> {
    if !prompt::interactive() {
        return Err(fail(
            "this panics the kernel: run it at a terminal to be asked, or give --yes".into(),
        ));
    }
    let question = "This panics the kernel and restarts the computer now. Type panic to go on: ";
    match prompt::ask(question, true) {
        Ok(Answer::Line(line)) if line.trim_ascii() == b"panic" => Ok(()),
        Ok(_) => Err(fail("not confirmed, so the kernel was not asked to panic".into())),
        Err(err) => Err(util::failed(&"standard input", err)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;

    /// A dump as the kernel writes one, whose third line is `when`.
    fn dump(boot: u32, when: &str) -> Vec<u8> {
        let report =
            "Radix panic on CPU 0 at 3612.481930 s: selftest crash-reboot: deliberate panic";
        dump_of(boot, when, report)
    }

    /// A dump whose panic report starts with the line `report`.
    fn dump_of(boot: u32, when: &str, report: &str) -> Vec<u8> {
        format!(
            "Radix crash dump\n\
             Radix 0.16.0 on x86_64, boot {boot}\n\
             Panic at {when}, 3612.481930 s after boot\n\
             \n\
             == Panic\n\
             {report}\n  \
             at kernel/radix/src/selftest.rs:118:13\n\
             Backtrace:\n  \
             #0  0xffffffffa031416e  radix::selftest::crash_reboot+0x3e\n\
             \n\
             == CPUs\n\
             2 CPUs, interrupts by x2APIC, timer at 100 Hz:\n\
             \n\
             == Stack of CPU 0\n\
             ffff800000a3fe40  ffff8000001234ab  radix::fs::writeback::run+0x4b\n\
             \n\
             == Log\n\
             [    0.000000] Radix 0.16.0 on x86_64\n"
        )
        .into_bytes()
    }

    const KNOWN: &str = "2026-11-13T10:02:11Z";
    const UNKNOWN: &str = "an unknown time of day";

    /// An empty folder of its own for the test `test`.
    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dumps-test-{}-{test}", std::process::id()));
        // Left by an earlier run of the test, if the test was stopped.
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// `count` saved dumps a day apart, the newest first, each `bytes` big.
    fn saved(count: u64, bytes: u64) -> Vec<Saved> {
        (0..count)
            .map(|day| Saved {
                name: format!("2026-11-{:02}T10-00-00Z-boot{day}.txt", 20 - day),
                bytes,
                written: SystemTime::UNIX_EPOCH + Duration::from_secs((100 - day) * 86_400),
            })
            .collect()
    }

    fn names(saved: &[&Saved]) -> Vec<String> {
        saved.iter().map(|dump| dump.name.clone()).collect()
    }

    #[test]
    fn a_dump_is_named_by_the_time_of_its_panic_and_its_boot() {
        let text = dump(42, KNOWN);
        let summary = Summary::of(&text).unwrap();
        assert_eq!(
            summary,
            Summary {
                boot: 42,
                panicked_at: Some(KNOWN.to_string()),
                panic: "selftest crash-reboot: deliberate panic".to_string(),
            }
        );
        let name = name_of(&summary, &text);
        assert_eq!(name, "2026-11-13T10-02-11Z-boot42.txt");
        assert!(is_dump_name(&name));
    }

    #[test]
    fn a_dump_with_an_unknown_time_is_named_by_its_content() {
        let text = dump(42, UNKNOWN);
        let summary = Summary::of(&text).unwrap();
        assert_eq!(summary.panicked_at, None);
        let name = name_of(&summary, &text);
        assert_eq!(name, format!("unknown-time-boot42-{:08x}.txt", fnv1a(&text) & 0xffff_ffff));
        assert!(is_dump_name(&name));
        // The same dump has the same name; another, even of the same boot,
        // has another.
        assert_eq!(name_of(&summary, &dump(42, UNKNOWN)), name);
        let mut other = text.clone();
        *other.last_mut().unwrap() = b'!';
        assert_ne!(name_of(&summary, &other), name);
    }

    #[test]
    fn the_hash_of_a_dump_is_fnv_1a_of_all_its_bytes() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_panic_line_is_the_reports_first_line_without_its_start() {
        let report = |line: &str| Summary::of(&dump_of(1, KNOWN, line)).unwrap().panic;
        // A message may hold the start's own separator.
        assert_eq!(
            report("Radix panic on CPU 3 at 5.000001 s: heap corruption (took 2 s: late)"),
            "heap corruption (took 2 s: late)"
        );
        // A line the kernel did not write is shown as it is.
        assert_eq!(report("something else"), "something else");
    }

    #[test]
    fn what_is_no_dump_is_refused_with_the_reason() {
        let text = String::from_utf8(dump(7, KNOWN)).unwrap();
        let refused = |text: &str| Summary::of(text.as_bytes()).unwrap_err();
        assert!(refused("").contains("shorter"));
        assert!(refused("Radix crash dump\nRadix 0.16.0 on x86_64, boot 7").contains("shorter"));
        assert!(refused(&text.replace("Radix crash dump", "Radix")).contains("does not start"));
        assert!(refused(&text.replace(", boot 7", "")).contains("boot's number"));
        assert!(refused(&text.replace(", boot 7", ", boot seven")).contains("boot's number"));
        assert!(refused(&text.replace("Panic at ", "Crashed at ")).contains("when it panicked"));
        assert!(refused(&text.replace(KNOWN, "yesterday")).contains("when it panicked"));
        assert!(refused(&text.replace("2026-11-13T10:02", "2026-11-13T10-02")).contains("when"));
        assert!(refused(&text.replace("== Panic", "== Oops")).contains("== Panic"));
        assert!(refused(&text[..text.find("== Panic").unwrap() + 8]).contains("== Panic"));
    }

    #[test]
    fn a_dump_cut_in_the_middle_of_a_character_still_has_its_start() {
        // The log at its end is a ring, which may start in the middle of
        // a character.
        let mut text = dump(9, KNOWN);
        text.extend_from_slice(&[0xe2, 0x82]);
        assert_eq!(Summary::of(&text).unwrap().boot, 9);
    }

    #[test]
    fn only_the_names_saving_gives_are_dump_names() {
        for good in [
            "2026-11-13T10-02-11Z-boot42.txt",
            "2026-11-13T10-02-11Z-boot1.txt",
            "unknown-time-boot3-00c0ffee.txt",
        ] {
            assert!(is_dump_name(good), "{good}");
        }
        for bad in [
            "notes.txt",
            "2026-11-13T10-02-11Z-boot42.txt.new",
            "2026-11-13T10-02-11Z-boot.txt",
            "2026-11-13T10:02:11Z-boot42.txt",
            "2026-11-13T10-02-11-boot42.txt",
            "unknown-time-boot3-c0ffee.txt",
            "unknown-time-boot3-00C0FFEE.txt",
            "unknown-time-boot-00c0ffee.txt",
            "../2026-11-13T10-02-11Z-boot42.txt",
            "",
        ] {
            assert!(!is_dump_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_ten_newest_dumps_are_kept() {
        let all = saved(13, 1 << 20);
        let doomed = surplus(&all, "none");
        assert_eq!(names(&doomed), names(&all[10..].iter().collect::<Vec<_>>()));
        assert!(surplus(&all[..10], "none").is_empty());
    }

    #[test]
    fn dumps_past_the_size_limit_are_deleted_oldest_first() {
        // Two of 30 MiB are 60 MiB; the third takes the total past 64 MiB,
        // and every older one goes with it.
        let all = saved(5, 30 << 20);
        assert_eq!(names(&surplus(&all, "none")), names(&all[2..].iter().collect::<Vec<_>>()));
        let exact = saved(4, 16 << 20);
        assert!(surplus(&exact, "none").is_empty());
    }

    #[test]
    fn the_dump_just_saved_is_never_deleted() {
        // A clock that was wrong when it was written makes it the oldest.
        let all = saved(12, 1 << 20);
        let oldest = all[11].name.clone();
        assert_eq!(names(&surplus(&all, &oldest)), [all[10].name.clone()]);
    }

    #[test]
    fn a_dump_is_saved_whole_and_for_root_alone_and_only_once() {
        let dir = folder("saved-once");
        let text = dump(42, KNOWN);
        let saved = save_in(&dir, &text).unwrap();
        let name = "2026-11-13T10-02-11Z-boot42.txt";
        assert_eq!(
            saved,
            Outcome::Saved {
                name: name.to_string(),
                panic: "selftest crash-reboot: deliberate panic".to_string(),
            }
        );
        assert_eq!(fs::read(dir.join(name)).unwrap(), text);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join(name)), 0o600);
        assert_eq!(mode(&dir), 0o700);
        // Nothing is left beside it, and a second save says it is there.
        assert_eq!(save_in(&dir, &text).unwrap(), Outcome::AlreadySaved(name.to_string()));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_was_there_is_made_roots_alone() {
        let dir = folder("open-folder");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        save_in(&dir, &dump(3, KNOWN)).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_the_kernel_gave_that_is_no_dump_is_not_saved() {
        let dir = folder("no-dump");
        let failure = save_in(&dir, b"Radix\n").unwrap_err();
        assert!(failure.message.contains("holds no crash dump"), "{}", failure.message);
        assert!(!dir.exists());
    }

    #[test]
    fn a_dump_of_an_unknown_time_saved_twice_is_saved_once() {
        let dir = folder("unknown-twice");
        let text = dump(5, UNKNOWN);
        let first = save_in(&dir, &text).unwrap();
        let Outcome::Saved { name, .. } = first else { panic!("not saved: {first:?}") };
        assert_eq!(save_in(&dir, &text).unwrap(), Outcome::AlreadySaved(name));
        // The next boot's dump, with the same boot number after RAM was
        // lost, is another file.
        let mut other = dump(5, UNKNOWN);
        other.extend_from_slice(b"another log\n");
        assert!(matches!(save_in(&dir, &other).unwrap(), Outcome::Saved { .. }));
        assert_eq!(saved_in(&dir).unwrap().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_past_the_tenth_dump_deletes_the_oldest() {
        let dir = folder("prune");
        fs::create_dir_all(&dir).unwrap();
        for day in 1..=11u32 {
            let name = format!("2026-10-{day:02}T08-00-00Z-boot{day}.txt");
            let file = File::create(dir.join(&name)).unwrap();
            file.set_modified(
                SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(day) * 86_400),
            )
            .unwrap();
        }
        // Files that are not dumps stay, however many there are.
        fs::write(dir.join("notes.txt"), "mine").unwrap();
        prune(&dir, "2026-10-11T08-00-00Z-boot11.txt").unwrap();
        let kept: Vec<String> = saved_in(&dir).unwrap().into_iter().map(|d| d.name).collect();
        assert_eq!(kept.len(), 10);
        assert_eq!(kept[0], "2026-10-11T08-00-00Z-boot11.txt");
        assert_eq!(kept[9], "2026-10-02T08-00-00Z-boot2.txt");
        assert!(dir.join("notes.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_newest_dump_is_the_one_a_name_is_not_needed_for() {
        let all = saved(3, 10);
        assert_eq!(find(&all, None).unwrap().name, all[0].name);
        assert_eq!(find(&all, Some(&all[2].name)).unwrap().name, all[2].name);
        let missing = find(&all, Some("nothing.txt")).unwrap_err();
        assert!(missing.message.contains("no saved dump is called nothing.txt"));
        assert_eq!(find(&[], None).unwrap_err().message, "no crash dumps saved");
    }

    #[test]
    fn a_name_that_is_wrong_deletes_nothing() {
        let dir = folder("delete");
        fs::create_dir_all(&dir).unwrap();
        let (one, two) = ("2026-10-01T08-00-00Z-boot1.txt", "2026-10-02T08-00-00Z-boot2.txt");
        for name in [one, two] {
            fs::write(dir.join(name), "dump").unwrap();
        }
        let typo = [one.to_string(), "2026-10-03T08-00-00Z-boot3.txt".to_string()];
        assert!(delete_in(&dir, &typo, false).is_err());
        assert!(dir.join(one).exists() && dir.join(two).exists());
        // A name twice is deleted once.
        let twice = [two.to_string(), two.to_string()];
        assert_eq!(delete_in(&dir, &twice, false).unwrap(), [two]);
        assert!(dir.join(one).exists() && !dir.join(two).exists());
        assert_eq!(delete_in(&dir, &[], true).unwrap(), [one]);
        assert!(saved_in(&dir).unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_is_not_there_holds_no_dumps() {
        let dir = folder("absent");
        assert!(saved_in(&dir).unwrap().is_empty());
    }

    #[test]
    fn a_line_of_the_list_gives_the_boot_the_size_and_what_went_wrong() {
        let all = saved(1, 3 << 20);
        let known = row(&all[0], &dump(42, KNOWN));
        assert_eq!(known[0], all[0].name);
        assert!(shaped(&known[1], "####-##-## ##:##"), "{}", known[1]);
        assert_eq!(known[2..], ["42", "3.0M", "selftest crash-reboot: deliberate panic"]);
        assert_eq!(row(&all[0], &dump(42, UNKNOWN))[1], "unknown");
        let broken = row(&all[0], b"not a dump at all");
        assert_eq!(broken[1..3], ["?", "?"]);
        assert!(broken[4].starts_with("not a dump: "), "{}", broken[4]);
    }
}
