// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `admin put-back ID` (ADR 0516): writes back the earlier text of each
//! file that change `ID` of the web console's journal changed, as root.
//! The console's "Put back" button runs it, as `elevate admin put-back ID`,
//! so the command it shows is the one that runs.
//!
//! The journal is admind's store, which this command opens read-only: it
//! finds the row, takes the text of each file from before the change, and
//! writes it with `settings_edit::write`, which keeps the file's owner and
//! mode, a new file mode 0644, and writes nothing if the file holds the
//! text already. It says what it did for each file, and when the file was
//! not as the change had left it, because something changed it again, it
//! says that too: the earlier text is written over the later one.
//!
//! A service file, in `/system/settings/spark/services` or its override in
//! `settings.d` beside it, is Spark's, not the service's: Spark reads it
//! when the service next starts, as it does for `service NAME edit`. Put
//! back writes it and says so, with the command that puts it to use now,
//! and asks Spark nothing: a signal such as SIGHUP is for a service's own
//! files, and a service that does not handle it dies of it. A file in a
//! bundle service's data folder, `/shared/services/ID/ACCOUNT`, is the
//! service's own, so each service whose record bundled keeps for that bundle
//! and account is asked to read it again, with Spark's `reload`. A service
//! that does not run reads its file when it starts, and a file that belongs
//! to no service is only written. When Spark cannot be asked, the files stay
//! written, the command says so and exits 1.
//!
//! The two files of the Networking and Firewall pages are put to use too,
//! as the pages' own changes are (ADR 0518): after `/system/settings/network`
//! is written, Spark is asked to reload netd, as `service netd reload`
//! does, which keeps the connections on the addresses that stay; after
//! `/system/settings/firewall`, the kernel is given the file's rules, as
//! `firewall load` does, which the command can do because elevate gives it
//! the firewall capability. When either fails the files stay written, and
//! the command says so and exits 1.
//!
//! A change that cannot be put back is said so in words, and nothing is
//! written: a file the change made, a secret, a file not everyone may
//! read, one longer than 1 MiB and one whose earlier text is among the
//! versions the store has forgotten (it keeps the last 20 of a file).
//!
//! When it fails, look at `elevate log admind` for the store's own words,
//! and at the change in the console's "What changed" page.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::admind::journal::{self, Restore};
use crate::admind::store::changes::{Change, Seen};
use crate::admind::store::versions::sha256;
use crate::admind::store::{self, Store};
use crate::args::{Arg, Args};
use crate::bundles::{registry, services};
use crate::util::{self, Fail, fail, usage};
use crate::{firewall, netd, power, servicefiles, settings_edit};

/// What a mistake in the command line is told.
const USAGE: &str = "usage: admin put-back ID";

/// The mode of a file that is made again: a settings file's.
const NEW_MODE: u32 = 0o644;

/// Where the files that name services are.
struct Places {
    /// The service files, and `settings.d` in it.
    service_files: PathBuf,
    /// The bundle services' data folders, `ID/ACCOUNT` in each.
    data: PathBuf,
    /// bundled's records of the services' files, by name.
    records: PathBuf,
    /// netd's settings, network(5): netd reads them again when asked.
    network: PathBuf,
    /// The firewall's rules, which `firewall load` gives the kernel.
    firewall: PathBuf,
}

impl Places {
    /// Where Spark and bundled keep them.
    fn system() -> Places {
        Places {
            service_files: PathBuf::from(servicefiles::DIR),
            data: PathBuf::from(services::DATA),
            records: Path::new(registry::STATE).join(services::RECORDS),
            network: PathBuf::from(netd::SETTINGS),
            firewall: PathBuf::from(firewall::SETTINGS),
        }
    }
}

/// `admin put-back ID`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut id = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(value) if id.is_none() => id = Some(value),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(id) = id else { return Err(usage(USAGE.into())) };
    let id =
        id.to_str().and_then(|text| text.parse::<i64>().ok()).filter(|id| *id > 0).ok_or_else(
            || usage(format!("{} is not a change's number\n{USAGE}", id.to_string_lossy())),
        )?;
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root may put a change back: run it with elevate, as `elevate admin put-back {id}`"
        )));
    }
    let store = Store::open_read_only(Path::new(store::PATH))
        .map_err(|why| fail(format!("{}: {why}; is admind running?", store::PATH)))?;
    let mut out = std::io::stdout().lock();
    let load_rules = |out: &mut dyn Write| firewall::load(out, true);
    put_back(&store, id, &Places::system(), &power::request, &load_rules, &mut out)
}

/// Puts change `id` back, as the module says, and writes what it did to
/// `out`. `ask` gives Spark's answer to a request, and `load_rules` gives
/// the kernel the firewall's rules, saying so on the `out` it is given.
fn put_back(
    store: &Store,
    id: i64,
    places: &Places,
    ask: &dyn Fn(&str) -> Result<String, Fail>,
    load_rules: &dyn Fn(&mut dyn Write) -> Result<i32, Fail>,
    out: &mut dyn Write,
) -> Result<i32, Fail> {
    let change = store
        .change(id)
        .map_err(|why| fail(why.to_string()))?
        .ok_or_else(|| fail(format!("change {id} is not in the journal")))?;
    let plan =
        journal::restore_plan(store, &change).map_err(|why| fail(format!("change {id}: {why}")))?;
    for restore in &plan {
        check_path(&restore.path)?;
    }
    let (mut spark_files, mut services) = (Vec::new(), Vec::new());
    let (mut network, mut rules) = (false, false);
    for restore in &plan {
        let told = write_back(restore, &change)?;
        say(out, &told)?;
        network |= Path::new(&restore.path) == places.network;
        rules |= Path::new(&restore.path) == places.firewall;
        if let Some(name) = service_file_of(&restore.path, places)
            && !spark_files.contains(&name)
        {
            spark_files.push(name);
        }
        for name in bundle_services_of(&restore.path, places) {
            if !services.contains(&name) {
                services.push(name);
            }
        }
    }
    for name in &spark_files {
        say(
            out,
            &format!(
                "Spark reads {name}'s service file when {name} next starts: `service {name} \
                 restart` puts it to use now"
            ),
        )?;
    }
    // What could not be put to use, in words.
    let mut unreached = Vec::new();
    // netd reads the settings file itself, as a bundle's service reads its
    // data file, so it is asked to read it again in the same way.
    let asked = services.iter().map(String::as_str).chain(network.then_some("netd"));
    for name in asked {
        match reload(name, ask) {
            Ok(told) => say(out, &told)?,
            Err(why) => unreached.push(why),
        }
    }
    if rules && let Err(why) = load_rules(out) {
        unreached.push(format!("the firewall's rules could not be loaded: {}", why.message));
    }
    if !unreached.is_empty() {
        return Err(fail(format!(
            "change {id} is put back in its files, but {}",
            unreached.join("; ")
        )));
    }
    let files = plan.len();
    say(
        out,
        &format!("change {id} is put back: {files} {}", if files == 1 { "file" } else { "files" }),
    )?;
    Ok(0)
}

/// Says `line` on `out`.
fn say(out: &mut dyn Write, line: &str) -> Result<(), Fail> {
    writeln!(out, "{line}").map_err(|err| util::failed(&"standard output", err))
}

/// Why `path` is not one this command writes: it is not an absolute path
/// with no `..` in it. The journal only holds such paths, and a command
/// that runs as root takes no more than it needs on trust.
fn check_path(path: &str) -> Result<(), Fail> {
    let path = Path::new(path);
    if path.is_absolute() && path.components().all(|part| !matches!(part, Component::ParentDir)) {
        return Ok(());
    }
    Err(fail(format!("{}: the journal names a path that is not an absolute one", path.display())))
}

/// Writes `restore`'s text to its file and says what that did, and that
/// the file was changed again after `change` when it was.
fn write_back(restore: &Restore, change: &Change) -> Result<String, Fail> {
    let path = Path::new(&restore.path);
    let now = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(util::failed(&path.display(), err)),
    };
    let left = change.files.iter().find(|file| file.path == restore.path).map(|file| &file.after);
    let as_left = match (left, &now) {
        (Some(Seen::Text { sha256: hash, .. }), Some(bytes)) => *hash == sha256(bytes),
        (Some(Seen::Absent), None) => true,
        (Some(Seen::Text { .. }), None) | (Some(Seen::Absent), Some(_)) => false,
        // What the change left is not known by its text: nothing to compare.
        _ => true,
    };
    let written = settings_edit::write(path, &restore.text, NEW_MODE)
        .map_err(|err| util::failed(&path.display(), err))?;
    if !written {
        return Ok(format!("{} holds the earlier text already", restore.path));
    }
    let mut told = format!("put back {}", restore.path);
    if !as_left {
        told.push_str(&format!(
            "; it was changed again after change {}, and that later text is gone",
            change.id
        ));
    }
    Ok(told)
}

/// The service whose service file the file at `path` is, or whose override
/// in `settings.d`: the service of its name.
fn service_file_of(path: &str, places: &Places) -> Option<String> {
    let path = Path::new(path);
    let folder = path.parent()?;
    if folder != places.service_files
        && folder != places.service_files.join(manifest::service::OVERRIDES)
    {
        return None;
    }
    let name = path.file_name()?.to_str()?.strip_suffix(".toml")?;
    (!name.is_empty() && !name.starts_with('.')).then(|| name.to_string())
}

/// The services whose own file the file at `path` is: those of the bundle
/// and account whose data folder holds it, from bundled's records.
fn bundle_services_of(path: &str, places: &Places) -> Vec<String> {
    let Ok(rest) = Path::new(path).strip_prefix(&places.data) else { return Vec::new() };
    let mut parts = rest.components().filter_map(|part| part.as_os_str().to_str());
    let (Some(id), Some(account), Some(_file)) = (parts.next(), parts.next(), parts.next()) else {
        return Vec::new();
    };
    let Ok(records) = fs::read_dir(&places.records) else { return Vec::new() };
    let mut found: Vec<String> = records
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let record = services::Record::parse(&fs::read_to_string(entry.path()).ok()?)?;
            (record.id == id && record.account == account).then_some(name)
        })
        .collect();
    found.sort();
    found
}

/// Asks Spark to reload service `name`, and says what came of it.
///
/// # Errors
///
/// In words, if Spark cannot be asked or refuses for a reason other than
/// that the service does not run.
fn reload(name: &str, ask: &dyn Fn(&str) -> Result<String, Fail>) -> Result<String, String> {
    let answer = ask(&format!("reload {name}"))
        .map_err(|fail| format!("{name} could not be asked to reload: {}", fail.message))?;
    let answer = answer.trim_end();
    match answer.strip_prefix("error: ") {
        Some(why) if why.ends_with("is not running") => {
            Ok(format!("{name} is not running, so it reads its file when it starts"))
        }
        Some(why) if why.starts_with("no service is called") => {
            Ok(format!("Spark has no service called {name}, so nothing was asked to reload"))
        }
        Some(why) => Err(format!("{name} could not be asked to reload: {why}")),
        None => Ok(format!("asked {name} to reload")),
    }
}

#[cfg(test)]
mod tests;
