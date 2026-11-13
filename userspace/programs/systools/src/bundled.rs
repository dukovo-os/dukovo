// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! bundled(8): the bundle runtime, which installs bundles, seals them and
//! removes them for whoever asks (ADR 0223), links their commands where
//! the shell finds them, and tells bundle(1) which bundle to run and
//! what it recorded of it (ADR 0225).
//!
//! It answers on `/system/live/bundled/socket`, one request per
//! connection, each on a thread of its own, and tells who asks by the
//! user the kernel gives for the connection, never by what the request
//! says. It keeps what it installed in `/system/state/bundled`
//! (`bundles::registry`), which only it writes.
//!
//! A bundle is unpacked by the confined unpacker (ADR 0222) into a folder
//! bundled makes in the place's folder, checked again here, shown as the
//! install sheet, sealed (each file and folder root's and read-only), and
//! renamed to its ID. Every name in a person's folder is looked up
//! beneath a descriptor of that folder, never through a link, so nothing
//! there leads bundled elsewhere.
//!
//! Each place has a `commands` folder of bundled's, root's, which holds a
//! link by each command its bundles add, to bundle(1): `/applications/
//! commands` and each person's `applications/commands`, both on a
//! session's PATH. bundled makes it again from what it recorded after
//! each install and removal, and at its start.
//!
//! It keeps what each person said of each app's permissions, the grants
//! (ADR 0224, `bundles::grants`), in `/system/state/grants`, and asks them
//! through the trusted prompt: it sends authd(8) the question in the
//! system's own words, which consoled(8) shows on the console the
//! program runs on, where no program can draw or answer it. `bundle
//! grant` goes through the same prompt; taking a permission away needs
//! none, and reaches each run of the app at once, through the `watch`
//! connection its runner keeps.
//!
//! It reads the catalogs (ADR 0383, `bundles::catalogs`) with the TUF
//! client, as root, so that what a catalog vouches for is never a
//! person's word: `bundle search` searches what they list, `bundle
//! install ID` fetches the newest version from them, and a file installed
//! from anywhere else is trusted as a catalog's when a catalog lists its
//! very bytes.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use accounts::request::{Choice, Question};
use manifest::bundle::permissions::{self, Tier};
use manifest::bundle::{Manifest, Need, Request as Asked, RunsWith};
use spark_api::show;

use crate::args::Args;
use crate::bundles::catalogs::{self, Catalog};
use crate::bundles::feeds;
use crate::bundles::folder::{DEPTH_MAX, Dir, check, fd_stat, same};
use crate::bundles::grants::{self, Grant, Grants, Said};
use crate::bundles::history::{self, Event, What};
use crate::bundles::minisign::PublicKey;
use crate::bundles::protocol::{
    self, AddOn, Answer, Available, Find, Flags, Found, Hit, LINE_MAX, Lines, Listed, Request,
    Shown,
};
use crate::bundles::registry::{self, Installed, Place, Registry, Trust, base_id};
use crate::bundles::services::{self, Record};
use crate::bundles::sheet::{self, Facts};
use crate::bundles::{MANIFEST, confine, signed, snapshots, sums};
use crate::util::{self, Fail};
use crate::{proc, signin, sys};

/// Where bundles for everyone are.
pub const EVERYONE: &str = "/applications";
/// A person's bundles' folder, in their home.
pub const APPLICATIONS: &str = "applications";
/// A person's apps' data folder, in their home.
pub const APP_DATA: &str = "app-data";
/// The folder of a place that holds a link by each command of its
/// bundles, which a session's PATH names.
pub const COMMANDS: &str = "commands";
/// Where each command's link leads: bundle(1), which runs the command of
/// that name (ADR 0225).
pub const LAUNCHER: &str = "/system/commands/bundle";

/// The folder in a place's applications folder that holds the add-ons
/// of each line, by its full ID (ADR 0388).
pub const ADD_ONS: &str = "add-ons";

/// The system's commands, which no add-on's is linked beside.
const SYSTEM_COMMANDS: &str = "/system/commands";
/// How long a request may take to come.
const REQUEST_TIME: Duration = Duration::from_secs(10);
/// How long the install sheet's question waits for its answer.
const ANSWER_TIME: Duration = Duration::from_mins(10);
/// The most connections answered at once.
const CONNECTIONS_MAX: usize = 16;
/// How long a permission's question waits for the person: authd's own
/// wait, and a little more.
const CHOICE_TIME: Duration = Duration::from_mins(6);
/// The most runs of apps one person's watching connections follow.
const WATCHERS_MAX: usize = 64;
/// The longest app name a question shows.
const NAME_SHOWN: usize = 40;
/// When bundles that update by themselves first do, after bundled
/// starts, and then how often (ADR 0384).
const AUTOMATIC_FIRST: Duration = Duration::from_mins(10);
const AUTOMATIC_EVERY: Duration = Duration::from_hours(24);
/// The most events `bundle history` shows.
const HISTORY_SHOWN: usize = 200;

/// An audit event (ADR 0211): a security event of the log.
macro_rules! audit {
    ($($arg:tt)*) => { println!("<86>bundled: {}", format_args!($($arg)*)) };
}

/// Makes `fd`'s file root's, with `mode`.
fn set_root(fd: &OwnedFd, mode: libc::mode_t) -> io::Result<()> {
    // SAFETY: plain calls on a descriptor we hold.
    check(unsafe { libc::fchown(fd.as_raw_fd(), 0, 0) })?;
    // SAFETY: as above.
    check(unsafe { libc::fchmod(fd.as_raw_fd(), mode) })
}

/// Seals what is in `dir`: every folder root's and 0555, every file
/// root's and 0444, or 0555 if it may run. Anything but folders and
/// regular files is refused: the unpacker makes nothing else.
fn seal(dir: &Dir, depth: usize) -> Result<(), String> {
    if depth > DEPTH_MAX {
        return Err("its folders go too deep".into());
    }
    for name in dir.names().map_err(|e| format!("listing it: {e}"))? {
        let fd = dir.open_at(&name, false).map_err(|e| format!("{name}: {e}"))?;
        let st = fd_stat(&fd).map_err(|e| format!("{name}: {e}"))?;
        match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                let inner = Dir(fd);
                seal(&inner, depth + 1).map_err(|why| format!("{name}/{why}"))?;
                set_root(&inner.0, 0o555).map_err(|e| format!("{name}: {e}"))?;
            }
            libc::S_IFREG => {
                // fchown and fchmod need a descriptor open for writing.
                let writable = dir.open_writable(&name).map_err(|e| format!("{name}: {e}"))?;
                let again = fd_stat(&writable).map_err(|e| format!("{name}: {e}"))?;
                if !same(&st, &again) {
                    return Err(format!("{name}: changed as it was sealed"));
                }
                let mode = if st.st_mode & 0o111 != 0 { 0o555 } else { 0o444 };
                set_root(&writable, mode).map_err(|e| format!("{name}: {e}"))?;
            }
            _ => return Err(format!("{name}: neither a file nor a folder")),
        }
    }
    Ok(())
}

/// What bundled shares between its threads.
struct Shared {
    registry: Mutex<Registry>,
    grants: Mutex<Grants>,
    /// The runs of apps that follow what is taken away from them.
    watchers: Mutex<Vec<Watcher>>,
    connections: AtomicUsize,
    /// Held while a catalog's client reads or fetches, one at a time.
    catalogs: Mutex<()>,
    /// Set, and signalled, to have the bundles that update by themselves
    /// do so now.
    wake: (Mutex<bool>, Condvar),
}

/// A run of an app, by its runner's `watch` connection.
struct Watcher {
    uid: u32,
    id: String,
    stream: UnixStream,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn grants(&self) -> MutexGuard<'_, Grants> {
        self.grants.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn watchers(&self) -> MutexGuard<'_, Vec<Watcher>> {
        self.watchers.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn catalogs(&self) -> MutexGuard<'_, ()> {
        self.catalogs.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn state(name: &str) -> String {
    format!("{}/{name}", registry::STATE)
}

fn read_state(name: &str) -> io::Result<String> {
    match fs::read_to_string(state(name)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        other => other,
    }
}

/// Writes `text` to the file `path`, whole or not at all, with mode
/// `mode`.
fn write_whole(path: &str, text: &str, mode: u32) -> Result<(), String> {
    let new = format!("{path}.new");
    // One left behind would keep its mode.
    let _ = fs::remove_file(&new);
    let file = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&new);
    let written = file.and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        f.sync_all()
    });
    written.and_then(|()| fs::rename(&new, path)).map_err(|e| format!("{path}: {e}"))
}

/// Writes the registry's files, each whole or not at all.
fn save(registry: &Registry) -> Result<(), String> {
    for (name, text) in registry::FILES.iter().zip(registry.texts()) {
        write_whole(&state(name), &text, 0o644)?;
    }
    Ok(())
}

/// Writes user `uid`'s grants, whole or not at all.
fn save_grants(grants: &Grants, uid: u32) -> Result<(), String> {
    let records = grants.0.get(&uid).map_or(&[][..], Vec::as_slice);
    write_whole(&format!("{}/{uid}", grants::FOLDER), &Grants::text(records), 0o600)
}

/// Reads every person's grants. A file bundled cannot read is set aside,
/// as `UID.bad`, and its person is asked again.
fn read_grants() -> Result<Grants, Fail> {
    let folder = grants::FOLDER;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .and_then(|()| fs::set_permissions(folder, fs::Permissions::from_mode(0o700)))
        .map_err(|e| util::failed(&folder, e))?;
    let mut grants = Grants::default();
    for entry in fs::read_dir(folder).map_err(|e| util::failed(&folder, e))?.flatten() {
        let name = entry.file_name();
        let Some(uid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let path = format!("{folder}/{uid}");
        let read = fs::read_to_string(&path).map_err(|e| e.to_string());
        match read.and_then(|text| Grants::read(&text)) {
            Ok(records) => {
                grants.0.insert(uid, records);
            }
            Err(why) => {
                eprintln!("bundled: {path}: {why}; set aside as {uid}.bad, and asked again");
                let _ = fs::rename(&path, format!("{folder}/{uid}.bad"));
            }
        }
    }
    Ok(grants)
}

/// Now, in seconds since 1970.
fn now() -> u64 {
    util::now_ns() / 1_000_000_000
}

/// The user at the other end of `stream`, as the kernel gives it.
fn asker(stream: &UnixStream) -> Option<u32> {
    let slot = sys::slot_of(stream.as_raw_fd());
    radix_abi::file::socket_state(&sys::SYS, slot).ok().map(|state| state.user)
}

/// The terminal of the process at the other end of `stream`, as
/// /processes names it, if it acts for `uid`: where to ask its person.
fn terminal_of(stream: &UnixStream, uid: u32) -> Option<String> {
    let slot = sys::slot_of(stream.as_raw_fd());
    let state = radix_abi::file::socket_state(&sys::SYS, slot).ok()?;
    let process = proc::process(state.peer).ok()?;
    if process.user != uid {
        return None;
    }
    process.terminal.filter(|t| t != "none")
}

/// The account of user `uid`.
pub(crate) fn account(uid: u32) -> Result<accounts::User, String> {
    let text =
        fs::read_to_string(accounts::PASSWD).map_err(|e| format!("{}: {e}", accounts::PASSWD))?;
    accounts::users(&text)
        .entries()
        .find(|u| u.uid == uid)
        .cloned()
        .ok_or_else(|| format!("user {uid} has no account"))
}

/// The folder of `place`, as a path, and the user who must own it.
pub(crate) fn place_folder(place: Place) -> Result<(String, u32), String> {
    match place {
        Place::Everyone => Ok((EVERYONE.into(), 0)),
        Place::User(uid) => {
            let user = account(uid)?;
            Ok((format!("{}/{APPLICATIONS}", user.home.trim_end_matches('/')), uid))
        }
    }
}

/// Opens the folder of `place`, checking that it is a folder of its
/// owner's, not a link.
fn open_place(place: Place) -> Result<(Dir, String), String> {
    let (path, owner) = place_folder(place)?;
    let dir = Dir::open(&path).map_err(|e| format!("{path}: {e}"))?;
    let st = dir.stat().map_err(|e| format!("{path}: {e}"))?;
    if st.st_uid != owner {
        return Err(format!("{path} is not its owner's: bundled installs nothing there"));
    }
    Ok((dir, path))
}

fn whom(place: Place) -> String {
    match place {
        Place::Everyone => "everyone".into(),
        Place::User(uid) => account(uid).map_or_else(|_| format!("user {uid}"), |u| u.name),
    }
}

/// A connection, which answers lines.
struct Connection {
    stream: UnixStream,
    input: BufReader<Timed>,
}

/// The asker's side of a connection, each read of which waits no later
/// than `deadline`: a local socket takes no read timeout here, so poll(2)
/// keeps a client that sends a part of a line, or nothing, from holding
/// a thread.
struct Timed {
    stream: UnixStream,
    deadline: Instant,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        readable(&self.stream, self.deadline)?;
        self.stream.read(buf)
    }
}

impl Connection {
    fn send(&mut self, answer: &Answer) -> io::Result<()> {
        self.stream.write_all(&answer.bytes())
    }

    /// The next line the asker sends, without its newline.
    fn line(&mut self, time: Duration) -> io::Result<String> {
        self.input.get_mut().deadline = Instant::now() + time;
        let mut line = Vec::new();
        self.input.by_ref().take(LINE_MAX as u64 + 1).read_until(b'\n', &mut line)?;
        if line.pop() != Some(b'\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "no line"));
        }
        String::from_utf8(line).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not text"))
    }
}

/// Waits until `stream` has something to read, or `deadline` passes.
fn readable(stream: &UnixStream, deadline: Instant) -> io::Result<()> {
    let mut wanted = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one pollfd, valid for the call.
        let n = unsafe { libc::poll(&raw mut wanted, 1, ms) };
        if n > 0 {
            return Ok(());
        }
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// A name nobody else uses for a folder of bundled's in a place.
fn working_name(what: &str) -> Result<String, String> {
    let mut bytes = [0u8; 8];
    File::open(crate::bundles::RANDOM)
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("random bytes: {e}"))?;
    Ok(format!(".{what}-{:016x}", u64::from_le_bytes(bytes)))
}

/// Whether `name` is one of bundled's working folders.
fn is_working(name: &str) -> bool {
    name.starts_with(".installing-")
        || name.starts_with(".removing-")
        || name.starts_with(".swapping-")
}

/// Removes the working folders bundled left in `dir` when it stopped,
/// those that are root's.
fn tidy(dir: &Dir) {
    for name in dir.names().unwrap_or_default().into_iter().filter(|n| is_working(n)) {
        if let Ok(Some(st)) = dir.stat_at(&name)
            && st.st_uid == 0
        {
            let _ = dir.remove_tree(&name, 0);
        }
    }
}

/// How an install goes.
#[derive(Clone, Copy, Default)]
struct How<'a> {
    /// Installs without asking after the sheet.
    yes: bool,
    /// The catalog it was fetched from.
    from: Option<&'a str>,
    /// An update (ADR 0384): no sheet and no question, unless the new
    /// version asks for more than the one it replaces.
    update: bool,
    /// Asked by nobody, as updating by itself is: the sheet goes nowhere,
    /// and a version that asks for more waits.
    automatic: bool,
    /// Fetched from the bundle's own feed (ADR 0390).
    feed: bool,
}

/// What an update that asks for more says, when nobody is there to ask.
const WAITS: &str = "waits for your yes: it asks for more than ";

fn install(
    shared: &Shared,
    talk: Option<&mut Connection>,
    asker: u32,
    archive: &File,
    everyone: bool,
    how: How<'_>,
) -> Result<String, String> {
    if everyone && asker != 0 {
        return Err("installing for everyone is an administrator's: run elevate bundle install \
                    --for-everyone"
            .into());
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let (places, path) = open_place(place)?;
    tidy(&places);
    let working = working_name("installing")?;
    places.make(&working, 0o700).map_err(|e| format!("{path}/{working}: {e}"))?;
    let to = Unpacking { place, places: &places, path: &path, working: &working };
    let result = install_in(shared, talk, asker, archive, &to, how);
    if result.is_err() {
        let _ = places.remove_tree(&working, 0);
    }
    result
}

/// Where an install unpacks: the place, its folder, the folder's path, and
/// the working folder's name in it.
struct Unpacking<'a> {
    place: Place,
    places: &'a Dir,
    path: &'a str,
    working: &'a str,
}

/// The SHA-256, in hex, and length of `archive`, read from its start.
fn archive_hash(archive: &File) -> Result<(String, u64), String> {
    let mut file = archive.try_clone().map_err(|e| format!("{e}"))?;
    io::Seek::seek(&mut file, io::SeekFrom::Start(0)).map_err(|e| format!("{e}"))?;
    let mut hasher = sums::Hasher::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| format!("{e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        length += n as u64;
    }
    Ok((sums::hex(&hasher.finish()), length))
}

/// The catalogs that list a file of `id` with this SHA-256 and length,
/// from the indexes they last gave, while those still vouch for it:
/// `from` first, when it is one.
fn listing(id: &str, sha256: &str, length: u64, from: Option<&str>) -> Vec<String> {
    let (catalogs, _) = read_catalogs();
    let now = tuf::time::now();
    let state = Path::new(catalogs::STATE);
    let mut listed: Vec<String> = catalogs
        .iter()
        .filter(|c| !c.off)
        .filter(|c| {
            catalogs::read_index(state, &c.name)
                .is_some_and(|index| index.current(now) && index.lists(id, sha256, length))
        })
        .map(|c| c.name.clone())
        .collect();
    if let Some(from) = from
        && let Some(at) = listed.iter().position(|name| name == from)
    {
        let name = listed.remove(at);
        listed.insert(0, name);
    }
    listed
}

fn install_in(
    shared: &Shared,
    mut talk: Option<&mut Connection>,
    asker: u32,
    archive: &File,
    to: &Unpacking<'_>,
    how: How<'_>,
) -> Result<String, String> {
    let Unpacking { place, places, path, working } = *to;
    let folder = places.dir_at(working).map_err(|e| format!("{path}/{working}: {e}"))?;
    let made = folder.stat().map_err(|e| format!("{path}/{working}: {e}"))?;
    if made.st_uid != 0 || made.st_mode & 0o7777 != 0o700 {
        return Err(format!("{path}/{working} changed as it was made"));
    }
    let report = confine::unpack_into(archive, &folder.0)?;
    // Checked again here, as the unpacker read what came from anywhere:
    // the signature, the list and every byte, and nothing else there.
    let unpacked = format!("{path}/{working}");
    let still = |what: &str| -> Result<(), String> {
        match places.stat_at(working) {
            Ok(Some(st)) if same(&st, &made) => Ok(()),
            _ => Err(format!("{unpacked} changed {what}")),
        }
    };
    still("as it was unpacked")?;
    let checked = signed::check_folder(Path::new(&unpacked))?;
    still("as it was checked")?;
    // A bundle with a line is a bundle of its own, ID@LINE (ADR 0387).
    let id = checked.manifest.full_id();
    if checked.key != report.key
        || checked.manifest.id.0 != report.id
        || checked.manifest.version.0 != report.version
    {
        return Err("the unpacker's report is not what it unpacked".into());
    }
    let list = fs::read(Path::new(&unpacked).join(sums::LIST)).map_err(|e| format!("{e}"))?;
    let list = sums::hex(&sums::sha256(&list));
    // The bytes a catalog may vouch for: those of the archive, which
    // the unpacked folder came from and has been checked against.
    let (sha256, length) = archive_hash(archive)?;
    let listed = listing(&id, &sha256, length, how.from);
    if let Some(from) = how.from
        && listed.first().map(String::as_str) != Some(from)
    {
        return Err(format!("catalog {from} does not list what was fetched from it"));
    }
    let (trust, others, before) = {
        let registry = shared.lock();
        let trust = registry.trust(place, asker, &id, &checked.key, &listed)?;
        let before = registry.installed(place, &id).map(|i| i.version.clone());
        (trust, commands_in_reach(&registry, place, &id), before)
    };
    for command in &checked.manifest.commands {
        let name = command.name.to_string();
        if Path::new("/system/commands").join(&name).exists() {
            return Err(format!("its command {name} is one of the system's"));
        }
        if let Some(other) = others.iter().find(|(c, _)| *c == name) {
            return Err(format!("its command {name} is one {} already adds", other.1));
        }
    }
    // Services run for everyone, each by a name of its own (ADR 0385).
    check_services(&checked.manifest, place)?;
    // What it asks for that the version it replaces did not (ADR 0384).
    // One whose manifest cannot be read is taken to have asked for
    // nothing, so that the sheet is shown.
    let old = before.as_ref().and_then(|_| installed_manifest(place, &id).ok());
    let more = match &before {
        Some(_) => asks_more(old.as_ref(), &checked.manifest),
        None => Vec::new(),
    };
    if how.automatic && !more.is_empty() {
        return Err(format!(
            "{WAITS}{} did: {}",
            before.as_deref().unwrap_or_default(),
            more.join(", ")
        ));
    }
    let flaws = kept_flaws(&trust);
    let facts = Facts {
        manifest: &checked.manifest,
        key: &checked.key,
        trust: &trust,
        place,
        files: checked.files,
        bytes: checked.bytes,
        replaces: before.as_deref().map(|v| (v, more.as_slice())),
        flaws: flaws.as_ref(),
        all_flaws: false,
    };
    if !how.update || !more.is_empty() {
        if let Some(talk) = talk.as_deref_mut() {
            talk.send(&Answer::Sheet(sheet::write(&facts))).map_err(|e| format!("{e}"))?;
        }
        if !how.yes {
            let Some(talk) = talk else {
                return Err("not installed: nobody was there to say yes".into());
            };
            talk.send(&Answer::Question).map_err(|e| format!("{e}"))?;
            match talk.line(ANSWER_TIME).as_deref() {
                Ok("yes") => {}
                Ok(_) => return Err("not installed, as you said".into()),
                Err(_) => return Err("not installed: no answer came".into()),
            }
        }
    }
    // Its services' accounts and data folders, before it is in place.
    make_accounts(&checked.manifest)?;
    seal(&folder, 0)?;
    set_root(&folder.0, 0o555).map_err(|e| format!("{unpacked}: {e}"))?;
    still("as it was sealed")?;
    let version = checked.manifest.version.0.clone();
    let mut registry = shared.lock();
    // Bound since? Another install may have come in between.
    let trust = registry.trust(place, asker, &id, &checked.key, &listed)?;
    switch_in(places, path, working, &id)?;
    match places.stat_at(&id) {
        Ok(Some(st)) if same(&st, &made) => {}
        _ => return Err(format!("{path}/{id} changed as it was installed")),
    }
    let when = util::now_ns() / 1_000_000_000;
    let previous = registry.installed(place, &id).map(|i| i.version.clone());
    let catalog = match &trust {
        Trust::Catalog { name, .. } => Some(name.as_str()),
        _ => None,
    };
    registry.record(
        Installed { place, id: id.clone(), version: version.clone(), key: checked.key, list, when },
        catalog,
    );
    save(&registry)?;
    drop(registry);
    let why = match &trust {
        Trust::Catalog { name, .. } => format!("from catalog {name}"),
        Trust::Developer => "their developer key".into(),
        Trust::Bound { .. } | Trust::BoundForEveryone { .. } => "its key".into(),
        Trust::Unknown => "a key this computer did not know".into(),
    };
    let asked_by =
        if how.automatic { "updating by itself, bundled".into() } else { whom(Place::User(asker)) };
    let over = previous.as_ref().map_or_default(|v| format!(" in place of {v}"));
    audit!(
        "{asked_by} installed {id} {version} for {}{over}, signed by key {} ({why})",
        whom(place),
        checked.key.encoded()
    );
    let source = match &trust {
        Trust::Catalog { name, .. } => format!("from catalog {name}"),
        _ if how.feed => "from its feed".into(),
        _ => "from a file".into(),
    };
    add_history(&Event {
        when,
        place,
        id: id.clone(),
        what: if previous.is_some() { What::Updated } else { What::Installed },
        from: previous.clone(),
        to: Some(version.clone()),
        by: (!how.automatic).then_some(asker),
        note: source,
    });
    let whose = if place == Place::Everyone { "everyone" } else { "you" };
    let linked = link_commands(&shared.lock(), place)
        .err()
        .map_or_default(|why| format!("; its commands are not linked: {why}"));
    let ran = run_services(&checked.manifest, old.as_ref());
    let installed = format!("installed {id} {version}{over}, for {whose}{linked}{}", ran.said);
    // A new version of a service bundle proves itself, or the version it
    // replaced comes back, with its data (ADR 0386).
    if let (Some(from), Place::Everyone) = (&previous, place)
        && let Err(why) = health(&checked.manifest, &ran.started)
    {
        audit!("{id} {version} failed its health check: {why}; going back to {from}");
        let note = format!("{version} failed its health check: {why}");
        return Err(match go_back(shared, place, &id, None, note) {
            Ok(back) => format!("{installed}; but {why}, so bundled {back}"),
            Err(e) => format!("{installed}; but {why}, and it could not go back: {e}"),
        });
    }
    Ok(installed)
}

/// Checks that `manifest`'s services may be installed in `place` (ADR
/// 0385): for everyone only, as they run for no one, and each by a name
/// no service of the system's or of another bundle has.
fn check_services(manifest: &Manifest, place: Place) -> Result<(), String> {
    let id = &manifest.id.0;
    if manifest.services.is_empty() {
        return Ok(());
    }
    if place != Place::Everyone {
        return Err(format!(
            "{id} runs services, which run for everyone: an administrator installs it with \
             elevate bundle install --for-everyone"
        ));
    }
    for service in &manifest.services {
        let name = &service.name.0;
        match read_record(name) {
            Some(record) if record.id == *id => {}
            Some(record) => return Err(format!("its service {name} is {}'s", record.id)),
            None if Path::new(&services::file_path(name)).exists() => {
                return Err(format!("its service {name} is one of the system's"));
            }
            None => {}
        }
    }
    Ok(())
}

/// The record of service `name`'s file, if bundled wrote it.
fn read_record(name: &str) -> Option<Record> {
    let path = state(&format!("{}/{name}", services::RECORDS));
    Record::parse(&fs::read_to_string(path).ok()?)
}

/// The services bundled wrote files for, by name, with their records.
fn records() -> Vec<(String, Record)> {
    let Ok(entries) = fs::read_dir(state(services::RECORDS)) else { return Vec::new() };
    let mut all: Vec<(String, Record)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let record = read_record(&name)?;
            Some((name, record))
        })
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

/// Makes the accounts of `manifest`'s services through authd(8), and
/// their data folders, each its account's alone (ADR 0385), and the
/// folders of `/shared` they only read that are not there (ADR 0491). An
/// account and a folder kept from an earlier install are taken up again.
fn make_accounts(manifest: &Manifest) -> Result<(), String> {
    if manifest.services.is_empty() {
        return Ok(());
    }
    let id = &manifest.id.0;
    make_folder(Path::new(services::DATA), 0o755)?;
    let bundle = format!("{}/{id}", services::DATA);
    match fs::symlink_metadata(&bundle) {
        Ok(st) if st.is_dir() && st.uid() == 0 => {}
        Ok(_) => return Err(format!("{bundle} is in the way: it is not bundled's")),
        Err(_) => make_folder(Path::new(&bundle), 0o755)?,
    }
    for name in services::accounts(manifest) {
        let home = services::data_folder(id, &name);
        let request = accounts::request::Request::Service {
            name: name.clone(),
            whose: id.clone(),
            home: home.clone(),
        };
        let answer = crate::account::ask(&request).map_err(|fail| fail.message)?;
        let number = match answer {
            accounts::request::Answer::Done(word) => word.parse::<u32>().ok(),
            accounts::request::Answer::Refused(why) => {
                return Err(format!("cannot make the account {name}: {why}"));
            }
        };
        let user = number
            .and_then(|uid| account(uid).ok())
            .ok_or_else(|| format!("authd made the account {name}, but it is not there"))?;
        match fs::symlink_metadata(&home) {
            Ok(st) if st.is_dir() && st.uid() == user.uid => continue,
            Ok(_) => return Err(format!("{home} is in the way: it is not {name}'s")),
            Err(_) => {}
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .and_then(|()| std::os::unix::fs::chown(&home, Some(user.uid), Some(user.gid)))
            .and_then(|()| fs::set_permissions(&home, fs::Permissions::from_mode(0o700)))
            .map_err(|e| format!("{home}: {e}"))?;
    }
    // The folders of /shared its services only read, made empty when they
    // are not there, so that each starts before what it reads is put there.
    for folder in services::read_folders(manifest) {
        if fs::symlink_metadata(&folder).is_err() {
            make_folder(Path::new(&folder), 0o755)?;
        }
    }
    Ok(())
}

/// Asks Spark to `verb` service `name`: what went wrong, if anything.
fn tell_spark(verb: &str, name: &str) -> Option<String> {
    match crate::power::request(&format!("{verb} {name}")) {
        Ok(answer) => answer.strip_prefix("error: ").map(|why| why.trim_end().to_string()),
        Err(fail) => Some(fail.message),
    }
}

/// What [`run_services`] did: what to say after the install's message,
/// and the services it started.
struct Ran {
    said: String,
    started: Vec<String>,
}

/// How long a service has to stop when bundled stops it, before it
/// carries on: Spark's own wait, and some.
const STOP_TIME: Duration = Duration::from_secs(15);

/// How long the services of a new version have to answer their health
/// checks (ADR 0386).
const HEALTH_TIME: Duration = Duration::from_secs(60);

/// What Spark says each of `names` is doing; nothing for one it does not
/// know.
fn service_states(names: &[String]) -> Vec<(String, show::State)> {
    let Ok(answer) = crate::power::request("show") else { return Vec::new() };
    let Ok(document) = show::parse(&answer) else { return Vec::new() };
    document
        .services
        .into_iter()
        .filter(|service| names.contains(&service.name))
        .map(|service| (service.name, service.state))
        .collect()
}

/// Stops each of `names` and waits, up to [`STOP_TIME`], until none
/// runs: what went wrong, if anything.
fn stop_services(names: &[String]) -> Option<String> {
    for name in names {
        if let Some(why) = tell_spark("stop", name)
            && !why.starts_with("no service is called")
        {
            return Some(format!("its service {name} did not stop: {why}"));
        }
    }
    let deadline = Instant::now() + STOP_TIME;
    loop {
        let states = service_states(names);
        let running: Vec<&str> = states
            .iter()
            .filter(|(_, state)| {
                !matches!(
                    state,
                    show::State::Stopped
                        | show::State::Disabled
                        | show::State::Done
                        | show::State::Failed
                )
            })
            .map(|(name, _)| name.as_str())
            .collect();
        if running.is_empty() {
            return None;
        }
        if Instant::now() >= deadline {
            return Some(format!("{} did not stop in time", running.join(", ")));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Writes the files of `manifest`'s services, installed for everyone, in
/// place of those of `old`, the version it replaced, if any (ADR 0385,
/// 0386): takes away the services `old` ran that it does not; stops the
/// others `old` ran, so that the copy of their data is whole; keeps a
/// copy of each account's data as `old` left it, or, going back to a
/// version whose copy is kept, brings that back; writes each file, but
/// beside one an administrator changed, as `NAME.toml.default`; and
/// starts each that is enabled. What went wrong is said after the
/// install's message: the bundle is installed whatever it is.
fn run_services(manifest: &Manifest, old: Option<&Manifest>) -> Ran {
    let id = &manifest.id.0;
    let mut said = String::new();
    let mut started = Vec::new();
    for (name, record) in records() {
        if record.id == *id
            && !manifest.services.iter().any(|s| s.name.0 == name)
            && let Some(why) = drop_service(&name)
        {
            let _ = write!(said, "; its old service {name} is not wholly removed: {why}");
        }
    }
    if let Some(old) = old {
        let ran: Vec<String> = old
            .services
            .iter()
            .map(|s| s.name.0.clone())
            .filter(|name| read_record(name).is_some_and(|r| r.id == *id))
            .collect();
        if let Some(why) = stop_services(&ran) {
            let _ = write!(said, "; {why}");
        }
    }
    let data = Path::new(services::DATA).join(id);
    for account in services::accounts(manifest) {
        match snapshots::carry_owned(&data, &account, &manifest.version.0, SystemTime::now()) {
            Ok(None) => {}
            Ok(Some(done)) => {
                let _ = write!(said, "; the data of its account {done}");
            }
            Err(why) => {
                let _ = write!(said, "; the data of its account {account} is as it was: {why}");
            }
        }
    }
    let folder = format!("{EVERYONE}/{id}");
    for service in &manifest.services {
        let name = &service.name.0;
        let path = services::file_path(name);
        let record = read_record(name);
        let current = record.as_ref().and_then(|_| fs::read_to_string(&path).ok());
        // An administrator's enable or disable stays.
        let enabled = current.as_deref().and_then(services::enabled);
        // As does a file an administrator changed: the new one goes
        // beside it, and service check says what differs.
        let changed = matches!((&record, &current), (Some(r), Some(t)) if r.changed(t));
        let default = format!("{path}.default");
        let written = services::file(manifest, service, &folder, enabled).and_then(|text| {
            let record = Record {
                id: id.clone(),
                account: services::account(service),
                written: services::fingerprint(&text),
            };
            make_folder(Path::new(&state(services::RECORDS)), 0o700)?;
            if changed {
                write_whole(&default, &text, 0o644)?;
            } else {
                write_whole(&path, &text, 0o644)?;
                let _ = fs::remove_file(&default);
            }
            write_whole(&state(&format!("{}/{name}", services::RECORDS)), &record.text(), 0o600)?;
            Ok(())
        });
        if let Err(why) = written {
            let _ = write!(said, "; its service {name} cannot run: {why}");
            continue;
        }
        if changed {
            let _ = write!(
                said,
                "; its service {name} keeps the file an administrator changed, and the new one \
                 is {default}: service check {name} says what differs"
            );
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        if services::enabled(&text) != Some(true) {
            let _ = write!(
                said,
                "; its service {name} starts once an administrator enables it: elevate \
                 service {name} enable"
            );
            continue;
        }
        match tell_spark("start", name) {
            None => {
                let _ = write!(said, "; its service {name} runs");
                started.push(name.clone());
            }
            Some(why) => {
                let _ = write!(said, "; its service {name} did not start: {why}");
            }
        }
    }
    Ran { said, started }
}

/// Whether the services of `manifest` that were `started` work: each
/// whose manifest names a health check answers on its socket, in its
/// account's live folder, within [`HEALTH_TIME`] (ADR 0386). Why not, if
/// one does not.
fn health(manifest: &Manifest, started: &[String]) -> Result<(), String> {
    let deadline = Instant::now() + HEALTH_TIME;
    for service in &manifest.services {
        let check = service.health.as_ref().and_then(|health| health.socket.as_ref());
        let (Some(check), true) = (check, started.contains(&service.name.0)) else {
            continue;
        };
        let socket = format!("{}/{}", services::live_folder(&services::account(service)), check);
        loop {
            if UnixStream::connect(&socket).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "its service {} did not answer at {socket} within {} seconds",
                    service.name,
                    HEALTH_TIME.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// Stops service `name` and takes its file and record away: what went
/// wrong, if anything.
fn drop_service(name: &str) -> Option<String> {
    let stopped = tell_spark("stop", name);
    let path = services::file_path(name);
    if let Err(e) = fs::remove_file(&path)
        && e.kind() != io::ErrorKind::NotFound
    {
        return Some(format!("{path}: {e}"));
    }
    let _ = fs::remove_file(state(&format!("{}/{name}", services::RECORDS)));
    stopped.filter(|why| !why.starts_with("no service is called"))
}

/// Stops and takes away the services of bundle `id`, installed for
/// everyone; with `purge`, their accounts and data folders too, which are
/// kept otherwise, for a later install to take up. What it did, for the
/// removal's message.
fn remove_services(id: &str, purge: bool) -> Result<String, String> {
    let mut accounts: Vec<String> = Vec::new();
    let mut said = String::new();
    for (name, record) in records() {
        if record.id != id {
            continue;
        }
        if let Some(why) = drop_service(&name) {
            let _ = write!(said, "; its service {name} is not wholly removed: {why}");
        }
        if !accounts.contains(&record.account) {
            accounts.push(record.account);
        }
    }
    let data = format!("{}/{id}", services::DATA);
    let kept = fs::symlink_metadata(&data).is_ok_and(|st| st.is_dir() && st.uid() == 0);
    if !purge {
        if kept {
            let _ = write!(said, "; its services' data stays in {data}, for --purge to remove");
        }
        return Ok(said);
    }
    for name in &accounts {
        let request = accounts::request::Request::Remove { name: name.clone() };
        match crate::account::ask(&request).map_err(|fail| fail.message)? {
            accounts::request::Answer::Done(_) => {}
            accounts::request::Answer::Refused(why) => {
                let _ = write!(said, "; its account {name} is not removed: {why}");
            }
        }
    }
    if kept {
        fs::remove_dir_all(&data).map_err(|e| format!("{data}: {e}"))?;
        let _ = write!(said, ", with its services' accounts and data");
    }
    Ok(said)
}

/// The name the version a bundle replaced is kept by, beside it.
fn previous_name(id: &str) -> String {
    format!(".previous-{id}")
}

/// Puts the sealed folder `working` in `places` in place of the bundle
/// `id`, whose folder, if it is there, is kept as the version it replaced
/// (ADR 0384), in place of the one kept before. Each step is a rename in
/// the one folder, done under the registry's lock, so a run finds the old
/// version or the new, recorded as it is.
fn switch_in(places: &Dir, path: &str, working: &str, id: &str) -> Result<(), String> {
    let kept = previous_name(id);
    let mut gone = None;
    if places.stat_at(id).map_err(|e| format!("{path}/{id}: {e}"))?.is_some() {
        match places.stat_at(&kept).map_err(|e| format!("{path}/{kept}: {e}"))? {
            Some(st) if st.st_uid == 0 => {
                let old = working_name("removing")?;
                places.rename(&kept, &old).map_err(|e| format!("{path}/{kept}: {e}"))?;
                gone = Some(old);
            }
            Some(_) => {
                return Err(format!("{path}/{kept} is in the way: it is not bundled's"));
            }
            None => {}
        }
        places.rename(id, &kept).map_err(|e| format!("{path}/{id}: {e}"))?;
    }
    if let Err(e) = places.rename(working, id) {
        let _ = places.rename(&kept, id);
        return Err(format!("{path}/{id}: {e}"));
    }
    if let Some(old) = gone {
        let _ = places.remove_tree(&old, 0);
    }
    Ok(())
}

/// The permissions `new` asks for that `old` did not, or asks for more
/// of: more folders or apps, or other ports; and a command that runs
/// with the person's access that `old` did not have.
fn asks_more(old: Option<&Manifest>, new: &Manifest) -> Vec<String> {
    let mut more = Vec::new();
    for (name, request) in new.permissions.iter() {
        let wider = match old.and_then(|old| old.permissions.get(name)) {
            None => true,
            Some(was) => {
                !grants::covers(&grants::words(was), &grants::words(request))
                    || request.ports.iter().any(|p| !was.ports.contains(p))
            }
        };
        if wider {
            more.push(name.to_string());
        }
    }
    let yours = |m: &Manifest| -> Vec<String> {
        m.commands
            .iter()
            .filter(|c| c.runs_with == RunsWith::YourAccess)
            .map(|c| c.name.to_string())
            .collect()
    };
    let was = old.map_or_default(yours);
    if yours(new).iter().any(|c| !was.contains(c)) {
        more.push(grants::YOUR_ACCESS.into());
    }
    // A service runs for no one, from boot: one more is more.
    let ran = |name: &str| old.is_some_and(|old| old.services.iter().any(|s| s.name.0 == name));
    if new.services.iter().any(|s| !ran(&s.name.0)) {
        more.push(sheet::SERVICES.into());
    }
    more
}

/// Adds `event` to the history; one that cannot be written is said.
fn add_history(event: &Event) {
    if let Err(e) = history::add(&state(history::HISTORY), event) {
        eprintln!("bundled: the history: {e}");
    }
}

/// The commands of the bundles in reach of `place` but `id`, and whose.
fn commands_in_reach(registry: &Registry, place: Place, id: &str) -> Vec<(String, String)> {
    let mut commands = Vec::new();
    for installed in &registry.installed {
        let reach = installed.place == place || installed.place == Place::Everyone;
        // The lines of an ID may add the same commands (ADR 0387).
        let line_of_it =
            id.contains('@') && installed.id.contains('@') && base_id(&installed.id) == base_id(id);
        if !reach || installed.id == id || line_of_it {
            continue;
        }
        if let Ok(manifest) = installed_manifest(installed.place, &installed.id) {
            for command in &manifest.commands {
                commands.push((command.name.to_string(), installed.id.clone()));
            }
        }
    }
    commands
}

/// The commands of the bundles installed in `place`, and each command
/// of a line a second time as `NAME-LINE`, which runs that line (ADR
/// 0387).
fn commands_of(registry: &Registry, place: Place) -> Vec<String> {
    let mut commands: Vec<String> = registry
        .installed
        .iter()
        .filter(|i| i.place == place)
        .filter_map(|i| installed_manifest(i.place, &i.id).ok())
        .flat_map(|m| {
            let line = m.line.map(|l| l.0);
            m.commands.into_iter().flat_map(move |c| {
                let name = c.name.to_string();
                let lined = line.as_ref().map(|line| format!("{name}-{line}"));
                std::iter::once(name).chain(lined)
            })
        })
        .collect();
    commands.sort();
    commands.dedup();
    commands
}

/// Makes the `commands` folder of `place` hold a link to [`LAUNCHER`] by
/// each command of its bundles, and nothing else: root's, mode 0755, made
/// if it is not there. A `commands` there that is not a folder of root's
/// is not bundled's, and bundled links nothing in it.
fn link_commands(registry: &Registry, place: Place) -> Result<(), String> {
    let (places, path) = open_place(place)?;
    let folder = format!("{path}/{COMMANDS}");
    match places.stat_at(COMMANDS).map_err(|e| format!("{folder}: {e}"))? {
        None => {
            places.make(COMMANDS, 0o755).map_err(|e| format!("{folder}: {e}"))?;
            let made = places.dir_at(COMMANDS).map_err(|e| format!("{folder}: {e}"))?;
            set_root(&made.0, 0o755).map_err(|e| format!("{folder}: {e}"))?;
        }
        Some(st) if st.st_mode & libc::S_IFMT == libc::S_IFDIR && st.st_uid == 0 => {}
        Some(_) => return Err(format!("{folder} is not bundled's, so it links nothing there")),
    }
    let links = places.dir_at(COMMANDS).map_err(|e| format!("{folder}: {e}"))?;
    let st = links.stat().map_err(|e| format!("{folder}: {e}"))?;
    if st.st_uid != 0 {
        return Err(format!("{folder} changed as it was opened"));
    }
    let mut wanted = commands_of(registry, place);
    let system = Dir::open(SYSTEM_COMMANDS).and_then(|d| d.names()).unwrap_or_default();
    wanted.extend(
        add_on_commands(registry, place)
            .into_iter()
            .map(|a| a.0)
            .filter(|name| !system.contains(name)),
    );
    wanted.sort();
    wanted.dedup();
    for name in links.names().map_err(|e| format!("{folder}: {e}"))? {
        let target = links.read_link(&name).map_err(|e| format!("{folder}/{name}: {e}"))?;
        let keep = wanted.contains(&name) && target.as_deref() == Some(LAUNCHER);
        if target.is_some() && !keep {
            links.unlink(&name).map_err(|e| format!("{folder}/{name}: {e}"))?;
        }
    }
    for name in &wanted {
        if links.stat_at(name).map_err(|e| format!("{folder}/{name}: {e}"))?.is_none() {
            links.symlink(LAUNCHER, name).map_err(|e| format!("{folder}/{name}: {e}"))?;
        }
    }
    Ok(())
}

/// Whose add-ons `asker`'s are: an administrator's are everyone's (ADR
/// 0388).
fn add_on_place(asker: u32) -> Place {
    if asker == 0 { Place::Everyone } else { Place::User(asker) }
}

/// The lines with add-ons in reach of `place`, by full ID, and the
/// folder in their add-ons that holds commands (ADR 0388).
fn add_on_lines(registry: &Registry, place: Place) -> Vec<(String, String)> {
    let mut lines: Vec<(String, String)> = Vec::new();
    for at in [place, Place::Everyone] {
        for installed in registry.installed.iter().filter(|i| i.place == at && i.id.contains('@')) {
            if lines.iter().any(|(id, _)| *id == installed.id) {
                continue;
            }
            if let Ok(manifest) = installed_manifest(installed.place, &installed.id)
                && let Some(add_ons) = manifest.add_ons
            {
                lines.push((installed.id.clone(), add_ons.commands.0));
            }
        }
    }
    lines.sort();
    lines
}

/// Opens the folder `inner` of the add-ons of line `id` in `place`, each
/// part its owner's and not a link: it, and its path.
fn add_ons_folder(place: Place, id: &str, inner: &str) -> Result<(Dir, String), String> {
    let (places, path) = open_place(place)?;
    let owner = place_folder(place)?.1;
    let mut path = format!("{path}/{ADD_ONS}");
    let mut dir = places.dir_at(ADD_ONS).map_err(|e| format!("{path}: {e}"))?;
    for part in std::iter::once(id).chain(inner.split('/')) {
        if dir.stat().map_err(|e| format!("{path}: {e}"))?.st_uid != owner {
            return Err(format!("{path} is not its owner's"));
        }
        path = format!("{path}/{part}");
        dir = dir.dir_at(part).map_err(|e| format!("{path}: {e}"))?;
    }
    if dir.stat().map_err(|e| format!("{path}: {e}"))?.st_uid != owner {
        return Err(format!("{path} is not its owner's"));
    }
    Ok((dir, path))
}

/// The commands of the add-ons of `place` (ADR 0388): each one's name,
/// line and path, by name; of two lines' commands of one name, the line
/// first by full ID. Only names a command may have, and nothing of a
/// folder that is not its owner's.
fn add_on_commands(registry: &Registry, place: Place) -> Vec<(String, String, String)> {
    let mut commands: Vec<(String, String, String)> = Vec::new();
    for (id, inner) in add_on_lines(registry, place) {
        let Ok((dir, path)) = add_ons_folder(place, &id, &inner) else { continue };
        for name in dir.names().unwrap_or_default() {
            let free = !commands.iter().any(|c| c.0 == name);
            if free && manifest::bundle::check_command_name(&name).is_ok() {
                commands.push((name.clone(), id.clone(), format!("{path}/{name}")));
            }
        }
    }
    commands.sort();
    commands
}

/// The add-on command `name` that `asker` runs: their own before
/// everyone's (ADR 0388).
fn add_on(registry: &Registry, asker: u32, name: &str) -> Option<AddOn> {
    let places =
        if asker == 0 { vec![Place::Everyone] } else { vec![Place::User(asker), Place::Everyone] };
    places.into_iter().find_map(|place| {
        add_on_commands(registry, place)
            .into_iter()
            .find(|c| c.0 == name)
            .map(|(_, id, path)| AddOn { place, id, path })
    })
}

/// Removes the add-ons of line `id` in `place`: where they were, if they
/// were.
fn remove_add_ons(place: Place, id: &str) -> Result<Option<String>, String> {
    let (places, path) = open_place(place)?;
    let owner = place_folder(place)?.1;
    let folder = format!("{path}/{ADD_ONS}");
    let Ok(add_ons) = places.dir_at(ADD_ONS) else { return Ok(None) };
    if add_ons.stat().map_err(|e| format!("{folder}: {e}"))?.st_uid != owner {
        return Err(format!("{folder} is not its owner's"));
    }
    if add_ons.stat_at(id).map_err(|e| format!("{folder}/{id}: {e}"))?.is_none() {
        return Ok(None);
    }
    add_ons.remove_tree(id, 0).map_err(|e| format!("{folder}/{id}: {e}"))?;
    Ok(Some(format!("{folder}/{id}")))
}

/// The lines of the ID that `name` names, installed for `asker` or for
/// everyone: `name` is the ID, or a command one of its lines adds, the
/// asker's own before everyone's (ADR 0387). `None` when no line is.
fn lines_of(registry: &Registry, asker: u32, name: &str) -> Option<Lines> {
    let places = [Place::User(asker), Place::Everyone];
    let in_reach =
        || registry.installed.iter().filter(|i| places.contains(&i.place) && i.id.contains('@'));
    let named = |i: &Installed| {
        base_id(&i.id) == name
            || installed_manifest(i.place, &i.id)
                .is_ok_and(|m| m.commands.iter().any(|c| c.name == *name))
    };
    let first = places
        .iter()
        .find_map(|place| in_reach().filter(|i| i.place == *place).find(|i| named(i)))?;
    let id = base_id(&first.id).to_string();
    let mut lines: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for installed in in_reach().filter(|i| base_id(&i.id) == id) {
        let Ok(manifest) = installed_manifest(installed.place, &installed.id) else { continue };
        let Some(line) = manifest.line.map(|l| l.0) else { continue };
        if !lines.contains(&line) {
            lines.push(line);
        }
        for file in manifest.line_files {
            if !files.contains(&file.0) {
                files.push(file.0);
            }
        }
    }
    lines.sort_by(|a, b| catalogs::compare_versions(a, b));
    let default = |place| registry.default_line(place, &id).map(String::from);
    let mine = if asker == 0 { None } else { default(Place::User(asker)) };
    let everyone = default(Place::Everyone);
    Some(Lines { id, lines, files, mine, everyone })
}

/// `bundle default NAME [LINE]`: makes `line` the line of the ID `name`
/// names that runs unless a project asks for another, the asker's or
/// everyone's, or forgets it (ADR 0387).
fn set_default(
    shared: &Shared,
    asker: u32,
    name: &str,
    line: Option<&str>,
    everyone: bool,
) -> Result<String, String> {
    if everyone && asker != 0 {
        return Err("everyone's default line is an administrator's to set: run elevate bundle \
                    default --for-everyone"
            .into());
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let mut registry = shared.lock();
    let lines = lines_of(&registry, asker, name)
        .ok_or_else(|| format!("no line of {name} is installed"))?;
    let id = lines.id;
    if let Some(line) = line {
        let full = format!("{id}@{line}");
        let there = if everyone {
            registry.installed(Place::Everyone, &full).is_some()
        } else {
            lines.lines.iter().any(|l| l == line)
        };
        if !there {
            let whom = if everyone { " for everyone" } else { "" };
            return Err(format!("{full} is not installed{whom}"));
        }
    }
    registry.set_default_line(place, &id, line);
    save(&registry)?;
    drop(registry);
    let whose = if everyone { "everyone's" } else { "your" };
    if everyone {
        audit!("root made {} everyone's default line of {id}", line.unwrap_or("none"));
    }
    Ok(match line {
        Some(line) => format!("{line} is {whose} default line of {id}"),
        None if everyone => format!("{id} has no default line for everyone now"),
        None => format!("{id} has no default line for you now"),
    })
}

/// The bundle to run that `find` names, the asker's own before
/// everyone's, and what bundled recorded of it; or, when it names an ID
/// or a command that several lines of an ID have, those lines, for the
/// asker to choose from (ADR 0387).
fn find(
    shared: &Shared,
    connection: &mut Connection,
    asker: u32,
    find: &Find,
) -> Result<String, String> {
    let found = {
        let registry = shared.lock();
        let places = [Place::User(asker), Place::Everyone];
        let first = |matches: &dyn Fn(&Installed) -> bool| {
            places.iter().find_map(|place| {
                registry.installed.iter().filter(|i| i.place == *place).find(|i| matches(i))
            })
        };
        let adds = |i: &Installed, name: &str| {
            installed_manifest(i.place, &i.id)
                .is_ok_and(|m| m.commands.iter().any(|c| c.name == *name))
        };
        let exact = match find {
            Find::Id(id) => first(&|i| i.id == *id),
            Find::Command(name) => first(&|i| adds(i, name)),
        };
        let name = match find {
            Find::Id(id) => id,
            Find::Command(name) => name,
        };
        let lines = match find {
            Find::Id(id) if exact.is_some() || id.contains('@') => None,
            Find::Command(_) if exact.is_some_and(|i| !i.id.contains('@')) => None,
            _ => lines_of(&registry, asker, name).filter(|l| l.lines.len() > 1),
        };
        if let Some(lines) = lines {
            connection.send(&Answer::Lines(lines)).map_err(|e| format!("{e}"))?;
            return Ok("choose a line".into());
        }
        // An ID with one line installed, or NAME-LINE, a command of a line.
        let one = || match find {
            Find::Id(id) => first(&|i| base_id(&i.id) == id && i.id.contains('@')),
            Find::Command(name) => first(&|i| {
                i.id.split_once('@').is_some_and(|(_, line)| {
                    name.strip_suffix(line)
                        .and_then(|rest| rest.strip_suffix('-'))
                        .is_some_and(|command| adds(i, command))
                })
            }),
        };
        exact.or_else(one).cloned()
    };
    let Some(installed) = found else {
        return Err(match find {
            Find::Id(id) => format!("{id} is not installed"),
            Find::Command(name) => {
                // A command of a line's add-ons (ADR 0388); a link to one
                // that went goes too.
                let registry = shared.lock();
                if let Some(add_on) = add_on(&registry, asker, name) {
                    let id = add_on.id.clone();
                    connection.send(&Answer::AddOn(add_on)).map_err(|e| format!("{e}"))?;
                    return Ok(format!("{name}, an add-on of {id}"));
                }
                let _ = link_commands(&registry, add_on_place(asker));
                format!("no bundle installed adds the command {name}")
            }
        });
    };
    let answer = Answer::Found(Found {
        place: installed.place,
        id: installed.id.clone(),
        key: installed.key,
        list: installed.list.clone(),
    });
    connection.send(&answer).map_err(|e| format!("{e}"))?;
    send_grants(shared, connection, asker, &installed.id, &installed.key)?;
    Ok(format!("{} {}", installed.id, installed.version))
}

/// Sends what `asker` said of app `id`'s permissions, signed by `key`.
fn send_grants(
    shared: &Shared,
    connection: &mut Connection,
    asker: u32,
    id: &str,
    key: &PublicKey,
) -> Result<(), String> {
    let said = shared.grants().of(asker, id, key);
    for grant in said {
        connection.send(&Answer::Granted(grant)).map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

/// The manifest of the bundle `id` installed in `place`.
fn installed_manifest(place: Place, id: &str) -> Result<manifest::bundle::Manifest, String> {
    let (path, _) = place_folder(place)?;
    let bytes =
        fs::read(format!("{path}/{id}/{MANIFEST}")).map_err(|e| format!("{path}/{id}: {e}"))?;
    signed::parse_manifest(&bytes)
}

fn list(shared: &Shared, connection: &mut Connection, asker: u32) -> Result<String, String> {
    let installed: Vec<Installed> = shared
        .lock()
        .installed
        .iter()
        .filter(|i| i.place == Place::Everyone || i.place == Place::User(asker))
        .cloned()
        .collect();
    let state = Path::new(catalogs::STATE);
    let mut lists = std::collections::HashMap::new();
    for i in &installed {
        let name = installed_manifest(i.place, &i.id).map_or_else(|_| "?".into(), |m| m.name.0);
        let bound = shared.lock().catalog_bound(i.place, &i.id).map(|b| b.catalog.clone());
        let flags = bound.map_or(Flags::Unknown, |catalog| {
            let list = lists
                .entry(catalog)
                .or_insert_with_key(|catalog| catalogs::read_flaws(state, catalog));
            catalogs::flags(list.as_ref(), &i.id, &i.version)
        });
        let listed = Listed {
            place: i.place,
            id: i.id.clone(),
            version: i.version.clone(),
            key: i.key,
            when: i.when,
            name: sheet::shown(&name, 60),
            flags,
        };
        connection.send(&Answer::Listed(listed)).map_err(|e| format!("{e}"))?;
    }
    Ok(format!("{} bundles", installed.len()))
}

fn info(
    shared: &Shared,
    connection: &mut Connection,
    asker: u32,
    id: &str,
) -> Result<String, String> {
    let (installed, trust) = {
        let registry = shared.lock();
        let own = registry.installed(Place::User(asker), id);
        let found = own.or_else(|| registry.installed(Place::Everyone, id)).cloned();
        let found = found.ok_or_else(|| format!("{id} is not installed"))?;
        let trust = match registry.catalog_bound(found.place, id) {
            Some(b) => Trust::Catalog { name: b.catalog.clone(), since: Some(b.since) },
            None => Trust::Bound {
                since: registry.bound(found.place, id).map_or(found.when, |b| b.since),
            },
        };
        (found, trust)
    };
    let (path, _) = place_folder(installed.place)?;
    let folder = format!("{path}/{id}");
    let manifest = installed_manifest(installed.place, id)?;
    let list =
        fs::read_to_string(format!("{folder}/{}", sums::LIST)).map_err(|e| format!("{e}"))?;
    let listed = sums::read(&list)?;
    let bytes: u64 = listed
        .iter()
        .filter_map(|e| fs::metadata(format!("{folder}/{}", e.path)).ok())
        .map(|m| m.len())
        .sum();
    let flaws = kept_flaws(&trust);
    let facts = Facts {
        manifest: &manifest,
        key: &installed.key,
        trust: &trust,
        place: installed.place,
        files: listed.len(),
        bytes,
        replaces: None,
        flaws: flaws.as_ref(),
        all_flaws: true,
    };
    connection.send(&Answer::Sheet(sheet::write(&facts))).map_err(|e| format!("{e}"))?;
    send_grants(shared, connection, asker, id, &installed.key)?;
    let data = match installed.place {
        Place::Everyone => format!("each person's {APP_DATA}/{id}"),
        Place::User(_) => account(asker)
            .map_or_else(|_| format!("{APP_DATA}/{id}"), |u| format!("{}/{APP_DATA}/{id}", u.home)),
    };
    let when =
        util::time_text(i64::try_from(installed.when).unwrap_or(0), "%Y-%m-%d %H:%M UTC", true);
    let (kept, held) = {
        let registry = shared.lock();
        let kept = registry.previous(installed.place, id).map(|p| p.version.clone());
        (kept, registry.held(installed.place, id).is_some())
    };
    let kept = kept.map_or_default(|v| format!("; it can go back to {v}"));
    let held = if held { "; held at its version" } else { "" };
    Ok(format!("installed on {when}; in {folder}; its data: {data}{kept}{held}"))
}

fn remove(
    shared: &Shared,
    asker: u32,
    id: &str,
    everyone: bool,
    purge: bool,
) -> Result<String, String> {
    if everyone && asker != 0 {
        return Err("removing for everyone is an administrator's: run elevate bundle remove \
                    --for-everyone"
            .into());
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let mut registry = shared.lock();
    let Some(installed) = registry.installed(place, id).cloned() else {
        if !everyone && registry.installed(Place::Everyone, id).is_some() {
            return Err(format!(
                "{id} is installed for everyone: an administrator removes it with elevate bundle \
                 remove --for-everyone {id}"
            ));
        }
        return Err(format!("{id} is not installed"));
    };
    let (places, path) = open_place(place)?;
    // Its services stop before their program goes (ADR 0385).
    let services =
        if place == Place::Everyone { remove_services(id, purge)? } else { String::new() };
    if places.stat_at(id).map_err(|e| format!("{path}/{id}: {e}"))?.is_some() {
        let old = working_name("removing")?;
        places.rename(id, &old).map_err(|e| format!("{path}/{id}: {e}"))?;
        places.remove_tree(&old, 0).map_err(|e| format!("{path}/{old}: {e}"))?;
    }
    // The version it replaced goes with it.
    let kept = previous_name(id);
    if let Ok(Some(st)) = places.stat_at(&kept)
        && st.st_uid == 0
    {
        let old = working_name("removing")?;
        if places.rename(&kept, &old).is_ok() {
            let _ = places.remove_tree(&old, 0);
        }
    }
    registry.forget(place, id);
    save(&registry)?;
    let _ = link_commands(&registry, place);
    drop(registry);
    let mut data = String::new();
    let mut purged = false;
    if purge && let Place::User(uid) = place {
        let user = account(uid)?;
        let folder = format!("{}/{APP_DATA}", user.home.trim_end_matches('/'));
        let dir = Dir::open(&folder).map_err(|e| format!("{folder}: {e}"))?;
        if dir.stat().is_ok_and(|st| st.st_uid == uid) {
            dir.remove_tree(id, 0).map_err(|e| format!("{folder}/{id}: {e}"))?;
            // And the copies of its data each version left.
            if let Ok(copies) = dir.dir_at(crate::bundles::snapshots::SNAPSHOTS)
                && copies.stat().is_ok_and(|st| st.st_uid == uid)
            {
                let _ = copies.remove_tree(id, 0);
            }
            data = format!(", and its data in {folder}/{id}");
            purged = true;
        }
        // What they said of it goes with its data.
        let mut grants = shared.grants();
        if grants.forget(uid, id) {
            save_grants(&grants, uid)?;
        }
    }
    audit!(
        "{} removed {id} {} for {}{data}",
        whom(Place::User(asker)),
        installed.version,
        whom(place)
    );
    add_history(&Event {
        when: now(),
        place,
        id: id.into(),
        what: What::Removed,
        from: Some(installed.version.clone()),
        to: None,
        by: Some(asker),
        note: if purged { "with its data".into() } else { String::new() },
    });
    // Its add-ons are its data: what each person added to the line (ADR
    // 0388).
    let mut add_ons = String::new();
    if purge && id.contains('@') {
        match remove_add_ons(place, id) {
            Ok(Some(folder)) => add_ons = format!(", and its add-ons in {folder}"),
            Ok(None) => {}
            Err(why) => add_ons = format!("; its add-ons stay: {why}"),
        }
    }
    let and = if purged { ", and its data" } else { "" };
    Ok(format!("removed {id} {}{and}{add_ons}{services}", installed.version))
}

/// The bundle `id` that `asker` runs: their own before everyone's.
fn installed_for(shared: &Shared, asker: u32, id: &str) -> Result<Installed, String> {
    let registry = shared.lock();
    let own = registry.installed(Place::User(asker), id);
    own.or_else(|| registry.installed(Place::Everyone, id))
        .cloned()
        .ok_or_else(|| format!("{id} is not installed"))
}

/// What `manifest` asks of `permission`: as it names it; for
/// runs-with-your-access, also as its commands that run with your access
/// ask for it.
fn asked_of(manifest: &Manifest, permission: &str) -> Option<Asked> {
    let named = manifest.permissions.iter().find(|(name, _)| *name == permission);
    if let Some((_, request)) = named {
        return Some(request.clone());
    }
    let yours = manifest.commands.iter().any(|c| c.runs_with == RunsWith::YourAccess);
    (permission == grants::YOUR_ACCESS && yours).then(|| Asked {
        need: Need::Required,
        why: None,
        folders: Vec::new(),
        ports: Vec::new(),
        apps: Vec::new(),
    })
}

/// Asks `name`, on `terminal`, the question `text`, through authd(8) and
/// the console's trusted prompt: what they chose ([`Choice::word`]),
/// [`accounts::request::CANCELLED`] or [`accounts::request::ELSEWHERE`].
fn ask_person(
    terminal: &str,
    name: &str,
    question: Question,
    text: &str,
) -> Result<String, String> {
    let stream = UnixStream::connect(accounts::SOCKET).map_err(|e| {
        format!("cannot reach authd at {}: {}", accounts::SOCKET, util::strip_os_error(&e))
    })?;
    let request = accounts::request::Request::Choose {
        terminal: terminal.into(),
        name: name.into(),
        question,
        text: text.into(),
    };
    (&stream)
        .write_all(&request.encode())
        .and_then(|()| stream.shutdown(Shutdown::Write))
        .map_err(|e| format!("cannot ask authd: {e}"))?;
    let mut input = BufReader::new(Timed { stream, deadline: Instant::now() + CHOICE_TIME });
    let mut line = String::new();
    input
        .by_ref()
        .take(LINE_MAX as u64)
        .read_line(&mut line)
        .map_err(|e| format!("authd did not answer: {e}"))?;
    match accounts::request::Answer::decode(&line) {
        accounts::request::Answer::Done(word) => Ok(word),
        accounts::request::Answer::Refused(why) => Err(why),
    }
}

/// Keeps what `uid` said of app `id`, signed by `key`, and writes it.
fn record(shared: &Shared, uid: u32, id: &str, key: PublicKey, grant: Grant) -> Result<(), String> {
    let mut grants = shared.grants();
    grants.set(uid, id, key, grant)?;
    save_grants(&grants, uid)
}

/// The words of a grant, for the audit log.
fn shown_words(words: &[String]) -> String {
    if words.is_empty() { String::new() } else { format!(" ({})", words.join(", ")) }
}

/// The question an app's permission is asked with: a question's words
/// begin with the app's name and ID, as the manifest says them.
fn named(manifest: &Manifest) -> String {
    format!("{} ({})", sheet::shown(&manifest.name.0, NAME_SHOWN), manifest.id)
}

/// `ask`: asks the person whether app `id` may have `permission`, as its
/// manifest asks for it, the first time it runs wanting it (ADR 0224).
fn ask(
    shared: &Shared,
    connection: &Connection,
    asker: u32,
    id: &str,
    permission: &str,
    list: &str,
) -> Result<String, String> {
    let installed = installed_for(shared, asker, id)?;
    if installed.list != list {
        return Err(format!("{id} changed as it started: run it again"));
    }
    let manifest = installed_manifest(installed.place, id)?;
    let found = permissions::find(permission).ok_or("no such permission")?;
    let request = asked_of(&manifest, permission)
        .ok_or_else(|| format!("{id} does not ask for {permission}"))?;
    if !matches!(found.tier, Tier::FirstUse | Tier::Warning) {
        return Err(format!("{permission} is not asked: it is given {}", found.tier.words()));
    }
    grants::givable(found).map_err(|why| format!("{permission}, {why}"))?;
    let asked = grants::words(&request);
    // Another run may have asked meanwhile: what it was answered holds,
    // if it was all of this or none of it.
    let before = shared.grants().of(asker, id, &installed.key);
    let held =
        before.iter().find(|g| g.permission == permission && grants::covers(&g.asked, &asked));
    match held {
        Some(g) if g.said == Said::Never => return Ok(Choice::Never.word().into()),
        Some(g) if grants::covers(&g.given, &asked) => return Ok(Choice::Always.word().into()),
        _ => {}
    }
    let Some(terminal) = terminal_of(&connection.stream, asker) else {
        return Ok(accounts::request::ELSEWHERE.into());
    };
    let user = account(asker)?;
    let question =
        if found.tier == Tier::Warning { Question::Warning } else { Question::Permission };
    let text =
        format!("{} asks to: {}", named(&manifest), grants::phrase(found, Some(&request), &asked));
    let word = ask_person(&terminal, &user.name, question, &text)?;
    let Some(choice) = Choice::from_word(&word) else { return Ok(word) };
    let words = shown_words(&asked);
    let said = match choice {
        Choice::Once => {
            audit!("{} allowed {id} {permission}{words} once, on {terminal}", user.name);
            return Ok(word);
        }
        Choice::Always => Said::Allow,
        Choice::Never => Said::Never,
    };
    let given = if said == Said::Allow { asked.clone() } else { Vec::new() };
    let grant = Grant { permission: permission.into(), said, since: now(), given, asked };
    record(shared, asker, id, installed.key, grant)?;
    let how = if said == Said::Allow { "allowed" } else { "did not allow" };
    audit!("{} {how} {id} {permission}{words}, from now on, on {terminal}", user.name);
    Ok(word)
}

/// `bundle grant`: gives app `id` `permission`, with `words` or as it
/// asks for it, once the person says yes at the trusted prompt, so that
/// no program can give itself anything by running bundle.
fn grant(
    shared: &Shared,
    connection: &Connection,
    asker: u32,
    id: &str,
    permission: &str,
    words: Vec<String>,
) -> Result<String, String> {
    let installed = installed_for(shared, asker, id)?;
    let manifest = installed_manifest(installed.place, id)?;
    let found = permissions::find(permission).ok_or("no such permission")?;
    grants::givable(found).map_err(|why| format!("{permission}, {why}"))?;
    let request = asked_of(&manifest, permission)
        .ok_or_else(|| format!("{id} does not ask for {permission}: there is nothing to give"))?;
    let asked = grants::words(&request);
    if !grants::covers(&asked, &words) {
        return Err(format!(
            "{id} asks for no more than {}: give that, or less",
            if asked.is_empty() { "the permission".into() } else { asked.join(" ") }
        ));
    }
    let mut given = if words.is_empty() { asked.clone() } else { words };
    given.sort();
    given.dedup();
    let folders: Vec<&str> =
        given.iter().filter_map(|w| w.split_once('=')).map(|(name, _)| name).collect();
    if let Some(twice) = folders.iter().enumerate().find(|(i, f)| folders[..*i].contains(f)) {
        return Err(format!("{} is named twice: give it to read, or to read and change", twice.1));
    }
    let elsewhere = "bundle grant asks you on a console, where no program can answer for you, \
                     and this terminal is not one";
    let terminal = terminal_of(&connection.stream, asker).ok_or(elsewhere)?;
    let user = account(asker)?;
    let text =
        format!("Give {}: {}", named(&manifest), grants::phrase(found, Some(&request), &given));
    let word = ask_person(&terminal, &user.name, Question::Grant, &text)?;
    match Choice::from_word(&word) {
        Some(Choice::Always) => {}
        Some(_) => return Err("not given: you said no".into()),
        None if word == accounts::request::CANCELLED => {
            return Err("not given: you closed the question".into());
        }
        None => return Err(elsewhere.into()),
    }
    let shown = shown_words(&given);
    let grant =
        Grant { permission: permission.into(), said: Said::Allow, since: now(), given, asked };
    record(shared, asker, id, installed.key, grant)?;
    audit!("{} gave {id} {permission}{shown}, on {terminal}", user.name);
    Ok(format!("gave {id} {permission}{shown}"))
}

/// `bundle revoke`: takes `permission` away from app `id`, now and from
/// now on: each run of it that watches is told at once. Taking away needs
/// no prompt.
fn revoke(shared: &Shared, asker: u32, id: &str, permission: &str) -> Result<String, String> {
    let installed = installed_for(shared, asker, id)?;
    let manifest = installed_manifest(installed.place, id)?;
    let found = permissions::find(permission).ok_or("no such permission")?;
    let request = asked_of(&manifest, permission)
        .ok_or_else(|| format!("{id} does not ask for {permission}"))?;
    if found.tier == Tier::Admin {
        return Err(format!("{permission} is an administrator's, and no app has it yet"));
    }
    let asked = grants::words(&request);
    let grant = Grant {
        permission: permission.into(),
        said: Said::Never,
        since: now(),
        given: Vec::new(),
        asked,
    };
    record(shared, asker, id, installed.key, grant)?;
    let user = account(asker).map_or_else(|_| format!("user {asker}"), |u| u.name);
    audit!("{user} took {permission} away from {id}");
    let told = tell_watchers(shared, asker, id, permission);
    let running = if told > 0 { format!("; {told} running told at once") } else { String::new() };
    Ok(format!("took {permission} away from {id}{running}"))
}

/// Tells each run of app `id` of user `uid` that watches that `permission`
/// was taken away; how many were told.
fn tell_watchers(shared: &Shared, uid: u32, id: &str, permission: &str) -> usize {
    let line = Answer::Revoked(permission.into()).bytes();
    let mut told = 0;
    shared.watchers().retain(|w| {
        if w.uid != uid || w.id != id {
            return true;
        }
        let sent = (&w.stream).write_all(&line).is_ok();
        told += usize::from(sent);
        sent
    });
    told
}

/// Keeps a run's `watch` connection, after forgetting those whose runs
/// ended: one that has something to read has ended, as a runner sends
/// nothing on it. Its writes never wait: a runner that does not read is
/// forgotten rather than hold up every revoke.
fn watch(shared: &Shared, uid: u32, id: String, stream: UnixStream) {
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let mut watchers = shared.watchers();
    watchers.retain(|w| readable(&w.stream, Instant::now()).is_err());
    let theirs = watchers.iter().filter(|w| w.uid == uid).count();
    if theirs >= WATCHERS_MAX
        && let Some(oldest) = watchers.iter().position(|w| w.uid == uid)
    {
        watchers.remove(oldest);
    }
    watchers.push(Watcher { uid, id, stream });
}

/// Answers one connection.
/// The catalogs set, and why each that is left out is.
fn read_catalogs() -> (Vec<Catalog>, Vec<String>) {
    catalogs::read_all(Path::new(catalogs::SETTINGS), Path::new(catalogs::DEFAULTS))
}

/// The catalogs read anew, and why each other could not be.
type Opened = (Vec<(Catalog, catalogs::Open)>, Vec<String>);

/// Reads every catalog that is on anew, or `only`: each one read, and why
/// each other could not be.
fn open_catalogs(only: Option<&str>) -> Result<Opened, String> {
    let (all, _) = read_catalogs();
    let chosen: Vec<Catalog> = match only {
        Some(name) => {
            let catalog = all.into_iter().find(|c| c.name == name).ok_or_else(|| {
                format!("there is no catalog {name}: `bundle catalog` lists them")
            })?;
            if catalog.off {
                return Err(format!("catalog {name} is off"));
            }
            vec![catalog]
        }
        None => all.into_iter().filter(|c| !c.off).collect(),
    };
    if chosen.is_empty() {
        return Err("no catalog is set: `bundle catalog add` adds one".into());
    }
    let state = Path::new(catalogs::STATE);
    let mut open = Vec::new();
    let mut failed = Vec::new();
    for catalog in chosen {
        match catalogs::open(&catalog, state) {
            Ok(read) => open.push((catalog, read)),
            Err(why) => failed.push(format!("catalog {} could not be read: {why}", catalog.name)),
        }
    }
    Ok((open, failed))
}

/// `bundle search WORD...`: what the catalogs list, read anew; a catalog
/// that cannot be read is searched as it last was, and said.
fn search(
    shared: &Shared,
    connection: &mut Connection,
    words: &[String],
) -> Result<String, String> {
    let _reading = shared.catalogs();
    let (open, failed) = open_catalogs(None)?;
    let state = Path::new(catalogs::STATE);
    let (all, _) = read_catalogs();
    let mut notes = failed;
    let mut found = 0;
    for catalog in all.iter().filter(|c| !c.off) {
        let index = match open.iter().find(|(c, _)| c.name == catalog.name) {
            Some((_, read)) => Some(read.index.clone()),
            None => catalogs::read_index(state, &catalog.name).inspect(|index| {
                let day = util::time_text(index.read, "%Y-%m-%d", true);
                notes.push(format!("catalog {} as it was on {day}", catalog.name));
            }),
        };
        let flaws = catalogs::read_flaws(state, &catalog.name);
        for entry in index.iter().flat_map(|index| index.search(words)) {
            let hit = Hit {
                catalog: catalog.name.clone(),
                id: entry.id.clone(),
                version: entry.version.clone(),
                name: entry.name.clone(),
                flags: catalogs::flags(flaws.as_ref(), &entry.id, &entry.version),
            };
            connection.send(&Answer::Hit(hit)).map_err(|e| format!("{e}"))?;
            found += 1;
        }
    }
    let mut said = format!("{found} found");
    for note in notes {
        said += &format!("; {note}");
    }
    Ok(said)
}

/// `bundle install ID`: the newest version of `id` that `catalog`, or the
/// first catalog that lists it, lists; or, when `id` came from a catalog
/// already here, that catalog's. Fetched by the client, checked against
/// its targets, then installed as a file is.
#[allow(clippy::too_many_arguments)]
fn get(
    shared: &Shared,
    connection: &mut Connection,
    asker: u32,
    id: &str,
    catalog: Option<&str>,
    everyone: bool,
    yes: bool,
) -> Result<String, String> {
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let bound = {
        let registry = shared.lock();
        let own = if everyone { None } else { registry.catalog_bound(place, id) };
        own.or_else(|| registry.catalog_bound(Place::Everyone, id)).map(|b| b.catalog.clone())
    };
    if let (Some(bound), Some(asked)) = (&bound, catalog)
        && bound != asked
    {
        return Err(format!("{id} comes from catalog {bound}, since its first install from it"));
    }
    let only = catalog.or(bound.as_deref());
    let (name, file) = {
        let _reading = shared.catalogs();
        let (mut open, failed) = open_catalogs(only)?;
        let mut chosen = None;
        for (catalog, read) in &mut open {
            if let Some(entry) = read.index.newest(id).cloned() {
                chosen = Some((catalog.name.clone(), read, entry));
                break;
            }
        }
        let Some((name, read, entry)) = chosen else {
            let mut why = match only {
                Some(only) => format!("catalog {only} does not list {id}"),
                None => format!("no catalog lists {id}"),
            };
            for note in failed {
                why += &format!("; {note}");
            }
            return Err(why);
        };
        (name, read.fetch(&entry)?)
    };
    let archive = File::open(&file).map_err(|e| format!("{}: {e}", file.display()));
    let result = archive.and_then(|archive| {
        let how = How { yes, from: Some(&name), ..How::default() };
        install(shared, Some(connection), asker, &archive, everyone, how)
    });
    let _ = fs::remove_file(&file);
    result
}

/// A bundle whose catalog, or own feed, lists a newer version.
struct Pending {
    installed: Installed,
    /// Its catalog, or none for its own feed (ADR 0390).
    catalog: Option<String>,
    newest: String,
    held: bool,
    /// The newer version, fetched and checked by the client.
    file: Option<std::path::PathBuf>,
}

/// The list of known flaws kept of the catalog a bundle is trusted as
/// coming from, if it is one's (ADR 0389).
fn kept_flaws(trust: &Trust) -> Option<dkv::flaws::List> {
    match trust {
        Trust::Catalog { name, .. } => catalogs::read_flaws(Path::new(catalogs::STATE), name),
        _ => None,
    }
}

/// The most flaws a history event names.
const FLAWS_NAMED: usize = 5;

/// The daily check of installed bundles (ADR 0389): reads anew each
/// catalog that installed bundles come from, which fetches its list of
/// known flaws, then tells the history and the log of each installed
/// version that list knows flaws of, once for each change.
fn check_flaws(shared: &Shared) {
    let installed: Vec<(Installed, String)> = {
        let registry = shared.lock();
        registry
            .installed
            .iter()
            .filter_map(|i| {
                let bound = registry.catalog_bound(i.place, &i.id)?;
                Some((i.clone(), bound.catalog.clone()))
            })
            .collect()
    };
    let mut names: Vec<&str> = installed.iter().map(|(_, c)| c.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let (all, _) = read_catalogs();
    let kept = Path::new(catalogs::STATE);
    let history = history::read(&state(history::HISTORY));
    // What the next sign-in says (ADR 0504); kept as it was when a
    // catalog could not be read, since its bundles' flaws are not known.
    let mut lines = Vec::new();
    let mut read_all = true;
    for name in names {
        let Some(catalog) = all.iter().find(|c| c.name == name && !c.off) else { continue };
        let read = {
            let _reading = shared.catalogs();
            catalogs::open(catalog, kept)
        };
        if let Err(why) = read {
            eprintln!("bundled: the daily check could not read catalog {name}: {why}");
            read_all = false;
            continue;
        }
        let Some(list) = catalogs::read_flaws(kept, name) else { continue };
        for (i, _) in installed.iter().filter(|(_, c)| c == name) {
            let found: Vec<_> = list.of(&i.id, &i.version).collect();
            if found.is_empty() {
                continue;
            }
            let mut note: Vec<String> = found
                .iter()
                .take(FLAWS_NAMED)
                .map(|f| format!("{} ({}) in {}", f.id, f.severity.word(), f.component))
                .collect();
            if found.len() > FLAWS_NAMED {
                note.push(format!("and {} more", found.len() - FLAWS_NAMED));
            }
            let note = note.join(", ");
            lines.push(format!(
                "bundled: {} {} for {} has known flaws: {note}",
                i.id,
                i.version,
                whom(i.place)
            ));
            let said = history
                .iter()
                .rev()
                .find(|e| {
                    e.what == What::Flawed
                        && e.place == i.place
                        && e.id == i.id
                        && e.from.as_deref() == Some(i.version.as_str())
                })
                .is_some_and(|e| e.note == note);
            if said {
                continue;
            }
            eprintln!(
                "bundled: {} {} for {} has known flaws: {note}",
                i.id,
                i.version,
                whom(i.place)
            );
            add_history(&Event {
                when: now(),
                place: i.place,
                id: i.id.clone(),
                what: What::Flawed,
                from: Some(i.version.clone()),
                to: None,
                by: None,
                note,
            });
        }
    }
    if read_all && crate::sign_in_notices::put("bundled", &lines) {
        let body = format!(
            "{}\nbundle update installs a fixed version once its catalog has one",
            lines.join("\n")
        );
        let _ = crate::notifyd::send(
            "bundled",
            "Installed apps have known flaws",
            &body,
            crate::notifyd::Urgency::Normal,
        );
    }
}

/// The bundles of `place`, or those of `ids`, whose catalogs list a newer
/// version, each fetched when `fetch` and it is not held; what could not
/// be read; and how many are up to date. The caller holds the catalogs.
fn find_updates(
    shared: &Shared,
    place: Place,
    ids: &[String],
    fetch: bool,
) -> Result<(Vec<Pending>, Vec<String>, usize), String> {
    let mut notes = Vec::new();
    let candidates: Vec<(Installed, Option<String>, bool)> = {
        let registry = shared.lock();
        let mut all = Vec::new();
        for id in ids {
            if registry.installed(place, id).is_none() {
                return Err(format!("{id} is not installed for {}", whose_word(place)));
            }
        }
        for installed in registry.installed.iter().filter(|i| i.place == place) {
            if !ids.is_empty() && !ids.contains(&installed.id) {
                continue;
            }
            let catalog = registry.catalog_bound(place, &installed.id).map(|b| b.catalog.clone());
            let held = registry.held(place, &installed.id).is_some();
            all.push((installed.clone(), catalog, held));
        }
        all
    };
    let mut names: Vec<&str> = candidates.iter().filter_map(|(_, c, _)| c.as_deref()).collect();
    names.sort_unstable();
    names.dedup();
    let mut open = Vec::new();
    for name in names {
        match open_catalogs(Some(name)) {
            Ok((read, failed)) => {
                open.extend(read);
                notes.extend(failed);
            }
            Err(why) => notes.push(why),
        }
    }
    let mut pending = Vec::new();
    let mut current = 0;
    for (installed, catalog, held) in candidates {
        let Some(catalog) = catalog else {
            match from_feed(place, &installed, held, fetch, ids.contains(&installed.id)) {
                Ok(Feeding::Newer(p)) => pending.push(*p),
                Ok(Feeding::Current) => current += 1,
                Ok(Feeding::None) => {}
                Err(why) => notes.push(why),
            }
            continue;
        };
        let Some((_, read)) = open.iter_mut().find(|(c, _)| c.name == catalog) else { continue };
        let newest = read.index.newest(&installed.id).cloned();
        let Some(entry) = newest.filter(|e| {
            catalogs::compare_versions(&e.version, &installed.version)
                == std::cmp::Ordering::Greater
        }) else {
            current += 1;
            continue;
        };
        let file = if fetch && !held {
            match read.fetch(&entry) {
                Ok(file) => Some(file),
                Err(why) => {
                    notes.push(format!("{} {}: {why}", installed.id, entry.version));
                    None
                }
            }
        } else {
            None
        };
        let catalog = Some(catalog);
        pending.push(Pending { installed, catalog, newest: entry.version, held, file });
    }
    Ok((pending, notes, current))
}

/// What a bundle from no catalog's own feed says.
enum Feeding {
    /// It names no feed.
    None,
    /// Its feed lists no newer version.
    Current,
    /// Its feed lists this one.
    Newer(Box<Pending>),
}

/// What `installed`, from no catalog, has from its own feed (ADR 0390):
/// a newer version, fetched when `fetch` and it is not held. That it
/// names no feed is said only when it was `asked` for.
fn from_feed(
    place: Place,
    installed: &Installed,
    held: bool,
    fetch: bool,
    asked: bool,
) -> Result<Feeding, String> {
    let id = &installed.id;
    let manifest = installed_manifest(place, id)?;
    let Some(update) = &manifest.update else {
        return if asked {
            Err(format!("{id} came from a file and names no feed, so nothing updates it"))
        } else {
            Ok(Feeding::None)
        };
    };
    let state = Path::new(feeds::STATE);
    let mut open = feeds::open(state, place, id, &update.feed.0, &update.key.0)
        .map_err(|why| format!("{id}: its feed: {why}"))?;
    let Some(entry) = open.index.newest(id).cloned().filter(|e| {
        catalogs::compare_versions(&e.version, &installed.version) == std::cmp::Ordering::Greater
    }) else {
        return Ok(Feeding::Current);
    };
    let file = if fetch && !held {
        Some(open.fetch(&entry).map_err(|why| format!("{id} {}: {why}", entry.version))?)
    } else {
        None
    };
    Ok(Feeding::Newer(Box::new(Pending {
        installed: installed.clone(),
        catalog: None,
        newest: entry.version,
        held,
        file,
    })))
}

/// Whose bundles a place holds, as words.
fn whose_word(place: Place) -> &'static str {
    match place {
        Place::Everyone => "everyone",
        Place::User(_) => "you",
    }
}

/// `bundle update`: updates the asker's bundles or everyone's, each from
/// the catalog it came from, to the newest version that catalog lists
/// (ADR 0384). One that asks for more than the version it replaces shows
/// its sheet and asks; a held one is left as it is.
#[allow(clippy::too_many_arguments)]
fn update(
    shared: &Shared,
    connection: &mut Connection,
    asker: u32,
    ids: &[String],
    everyone: bool,
    check: bool,
    yes: bool,
) -> Result<String, String> {
    if everyone && asker != 0 && !check {
        return Err("updating for everyone is an administrator's: run elevate bundle update \
                    --for-everyone"
            .into());
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let (pending, notes, current) = {
        let _reading = shared.catalogs();
        find_updates(shared, place, ids, !check)?
    };
    let mut said = String::new();
    if check {
        for p in &pending {
            let available = Available {
                place,
                id: p.installed.id.clone(),
                from: p.installed.version.clone(),
                to: p.newest.clone(),
                catalog: p.catalog.clone(),
                held: p.held,
            };
            connection.send(&Answer::Available(available)).map_err(|e| format!("{e}"))?;
        }
        if let Some(since) = shared.lock().automatic(place) {
            let day = util::time_text(i64::try_from(since).unwrap_or(0), "%Y-%m-%d", true);
            let whose = if everyone { "Everyone's" } else { "Your" };
            let note = format!("{whose} bundles update by themselves, daily, since {day}");
            connection.send(&Answer::Note(note)).map_err(|e| format!("{e}"))?;
        }
        said += &format!("{} to update, {current} up to date", pending.len());
    } else {
        let (mut updated, mut left) = (0, 0);
        for p in pending {
            let id = &p.installed.id;
            if p.held {
                let note = format!(
                    "{id} is held at {}, so {} waits: `bundle unhold {id}` lets it update",
                    p.installed.version, p.newest
                );
                connection.send(&Answer::Note(note)).map_err(|e| format!("{e}"))?;
                left += 1;
                continue;
            }
            let Some(file) = p.file else {
                left += 1;
                continue;
            };
            let how = How {
                yes,
                from: p.catalog.as_deref(),
                update: true,
                automatic: false,
                feed: p.catalog.is_none(),
            };
            let result = File::open(&file)
                .map_err(|e| format!("{}: {e}", file.display()))
                .and_then(|archive| {
                    install(shared, Some(&mut *connection), asker, &archive, everyone, how)
                });
            let _ = fs::remove_file(&file);
            let note = match result {
                Ok(what) => {
                    updated += 1;
                    what
                }
                Err(why) => {
                    left += 1;
                    format!("{id} {}: {why}", p.newest)
                }
            };
            connection.send(&Answer::Note(note)).map_err(|e| format!("{e}"))?;
        }
        said += &format!("{updated} updated, {left} not, {current} up to date");
    }
    for note in notes {
        said += &format!("; {note}");
    }
    Ok(said)
}

/// Updates the bundles of each place that updates by itself (ADR 0384):
/// as `bundle update` does, but one that asks for more than the version
/// it replaces waits for a person's yes, and a held one is left alone.
fn update_by_themselves(shared: &Shared) {
    let places: Vec<Place> = shared.lock().automatic.iter().map(|(place, _)| *place).collect();
    for place in places {
        let found = {
            let _reading = shared.catalogs();
            find_updates(shared, place, &[], true)
        };
        let (pending, notes, _) = match found {
            Ok(found) => found,
            Err(why) => {
                eprintln!("bundled: updating the bundles of {} by themselves: {why}", whom(place));
                continue;
            }
        };
        for note in notes {
            eprintln!("bundled: updating by themselves: {note}");
        }
        for p in pending.into_iter().filter(|p| !p.held) {
            let Some(file) = p.file else { continue };
            let (asker, everyone) = match place {
                Place::Everyone => (0, true),
                Place::User(uid) => (uid, false),
            };
            let how = How {
                yes: true,
                from: p.catalog.as_deref(),
                update: true,
                automatic: true,
                feed: p.catalog.is_none(),
            };
            let result = File::open(&file)
                .map_err(|e| format!("{}: {e}", file.display()))
                .and_then(|archive| install(shared, None, asker, &archive, everyone, how));
            let _ = fs::remove_file(&file);
            let id = &p.installed.id;
            let (from, to) = (&p.installed.version, &p.newest);
            match result {
                Ok(_) => eprintln!(
                    "bundled: updated {id} {from} to {to} for {} by itself, {}",
                    whom(place),
                    p.catalog.as_ref().map_or_else(
                        || "from its feed".to_string(),
                        |c| format!("from catalog {c}")
                    )
                ),
                Err(why) => {
                    let waits = why.starts_with(WAITS);
                    eprintln!("bundled: {id} {to} for {}: {why}", whom(place));
                    // Said once for each version, not each day.
                    let said = history::read(&state(history::HISTORY)).iter().any(|e| {
                        e.what == What::Waiting
                            && e.place == place
                            && e.id == *id
                            && e.to.as_deref() == Some(to.as_str())
                    });
                    if !said || !waits {
                        add_history(&Event {
                            when: now(),
                            place,
                            id: id.clone(),
                            what: if waits { What::Waiting } else { What::Failed },
                            from: Some(from.clone()),
                            to: Some(to.clone()),
                            by: None,
                            note: why,
                        });
                    }
                }
            }
        }
    }
}

/// Runs [`check_flaws`], and [`update_by_themselves`] where a place
/// turned it on, some minutes after bundled starts, then daily, and at
/// once when a place turns updating by themselves on.
fn updater(shared: &Shared) {
    let mut next = Instant::now() + AUTOMATIC_FIRST;
    loop {
        {
            let (now_please, signal) = &shared.wake;
            let mut asked = now_please.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*asked {
                let left = next.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                asked = signal
                    .wait_timeout(asked, left)
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
            *asked = false;
        }
        next = Instant::now() + AUTOMATIC_EVERY;
        check_flaws(shared);
        if !shared.lock().automatic.is_empty() {
            update_by_themselves(shared);
        }
    }
}

/// `bundle update --automatic on|off`: whether the asker's bundles, or
/// everyone's, update by themselves.
fn automatic(shared: &Shared, asker: u32, everyone: bool, on: bool) -> Result<String, String> {
    if everyone && asker != 0 {
        return Err("this is an administrator's: run elevate bundle update --for-everyone \
                    --automatic"
            .into());
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let whose = if everyone { "Everyone's bundles" } else { "Your bundles" };
    let mut registry = shared.lock();
    if !registry.set_automatic(place, on, now()) {
        return Ok(format!(
            "{whose} {} by themselves already",
            if on { "update" } else { "do not update" }
        ));
    }
    save(&registry)?;
    drop(registry);
    audit!(
        "{} turned updating by themselves {} for the bundles of {}",
        whom(Place::User(asker)),
        if on { "on" } else { "off" },
        whom(place)
    );
    if on {
        let (now_please, signal) = &shared.wake;
        *now_please.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        signal.notify_all();
        Ok(format!(
            "{whose} update by themselves from now on, daily, from their catalogs; one that asks \
             for more waits for your yes"
        ))
    } else {
        Ok(format!("{whose} update only when you say, from now on"))
    }
}

/// `bundle rollback`: goes back to the version the bundle replaced, which
/// is then the one kept, so that doing it again goes forward (ADR 0384).
fn rollback(shared: &Shared, asker: u32, id: &str, everyone: bool) -> Result<String, String> {
    if everyone && asker != 0 {
        return Err(format!(
            "going back for everyone is an administrator's: run elevate bundle rollback \
             --for-everyone {id}"
        ));
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    go_back(shared, place, id, Some(asker), String::new())
}

/// Goes back to the version of bundle `id` in `place` that the one there
/// replaced, and holds it there: for `by`, who asked, or by bundled
/// itself when a new version failed its health check, which `note`
/// says, in the history.
fn go_back(
    shared: &Shared,
    place: Place,
    id: &str,
    by: Option<u32>,
    note: String,
) -> Result<String, String> {
    let mut registry = shared.lock();
    let Some(current) = registry.installed(place, id).cloned() else {
        return Err(format!("{id} is not installed for {}", whose_word(place)));
    };
    let Some(then) = registry.previous(place, id).cloned() else {
        return Err(format!("{id} {} replaced no version that is kept", current.version));
    };
    let (places, path) = open_place(place)?;
    let kept = previous_name(id);
    let folder = format!("{path}/{kept}");
    // The kept folder: bundled's, sealed, and the very one recorded.
    match places.stat_at(&kept).map_err(|e| format!("{folder}: {e}"))? {
        Some(st)
            if st.st_uid == 0
                && st.st_mode & libc::S_IFMT == libc::S_IFDIR
                && st.st_mode & 0o7777 == 0o555 => {}
        _ => return Err(format!("{folder} is not the sealed version bundled kept")),
    }
    let checked = signed::check_folder(Path::new(&folder))?;
    let list = fs::read(format!("{folder}/{}", sums::LIST)).map_err(|e| format!("{e}"))?;
    let list = sums::hex(&sums::sha256(&list));
    if checked.key != then.key || list != then.list || checked.manifest.version.0 != then.version {
        return Err(format!("{folder} is not the version recorded"));
    }
    let others = commands_in_reach(&registry, place, id);
    for command in &checked.manifest.commands {
        let name = command.name.to_string();
        if let Some(other) = others.iter().find(|(c, _)| *c == name) {
            return Err(format!("its command {name} is one {} adds now", other.1));
        }
    }
    // The version it goes back from, whose services it replaces.
    let from = installed_manifest(place, id).ok();
    let swapping = working_name("swapping")?;
    places.rename(id, &swapping).map_err(|e| format!("{path}/{id}: {e}"))?;
    if let Err(e) = places.rename(&kept, id) {
        let _ = places.rename(&swapping, id);
        return Err(format!("{folder}: {e}"));
    }
    places.rename(&swapping, &kept).map_err(|e| format!("{path}/{swapping}: {e}"))?;
    registry.roll_back(place, id);
    // Held, so that no update, by itself or not, undoes it unasked.
    let held = registry.set_held(place, id, true, now());
    save(&registry)?;
    let linked = link_commands(&registry, place)
        .err()
        .map_or_default(|why| format!("; its commands are not linked: {why}"));
    drop(registry);
    let who = by.map_or_else(|| "bundled".into(), |asker| whom(Place::User(asker)));
    audit!(
        "{who} went back to {id} {} for {}, from {}",
        then.version,
        whom(place),
        current.version
    );
    add_history(&Event {
        when: now(),
        place,
        id: id.into(),
        what: What::RolledBack,
        from: Some(current.version.clone()),
        to: Some(then.version.clone()),
        by,
        note,
    });
    let held =
        if held { format!("; it is held there until `bundle unhold {id}`") } else { String::new() };
    let (data, ran) = if place == Place::Everyone && !checked.manifest.services.is_empty() {
        ("", run_services(&checked.manifest, from.as_ref()).said)
    } else {
        ("; its data comes back at its next run", String::new())
    };
    Ok(format!(
        "went back to {id} {}, from {}{data}{held}; `bundle rollback {id}` goes forward \
         again{linked}{ran}",
        then.version, current.version
    ))
}

/// `bundle hold` and `bundle unhold`: whether updates leave the bundle as
/// it is.
fn hold(shared: &Shared, asker: u32, id: &str, everyone: bool, on: bool) -> Result<String, String> {
    if everyone && asker != 0 {
        return Err("holding everyone's bundles is an administrator's: run it with elevate and \
                    --for-everyone"
            .into());
    }
    let place = if everyone { Place::Everyone } else { Place::User(asker) };
    let mut registry = shared.lock();
    let Some(installed) = registry.installed(place, id).cloned() else {
        return Err(format!("{id} is not installed for {}", whose_word(place)));
    };
    if !registry.set_held(place, id, on, now()) {
        return Ok(format!("{id} is {} already", if on { "held" } else { "not held" }));
    }
    save(&registry)?;
    drop(registry);
    add_history(&Event {
        when: now(),
        place,
        id: id.into(),
        what: if on { What::Held } else { What::Released },
        from: Some(installed.version.clone()),
        to: None,
        by: Some(asker),
        note: String::new(),
    });
    Ok(if on {
        format!(
            "held {id} at {}: updates leave it as it is until `bundle unhold {id}`",
            installed.version
        )
    } else {
        format!("{id} updates again")
    })
}

/// `bundle history`: what bundled did to the asker's bundles and
/// everyone's, or to `id`, the newest last; root sees everyone's.
fn history_of(connection: &mut Connection, asker: u32, id: Option<&str>) -> Result<String, String> {
    let events: Vec<Event> = history::read(&state(history::HISTORY))
        .into_iter()
        .filter(|e| asker == 0 || e.place == Place::Everyone || e.place == Place::User(asker))
        .filter(|e| id.is_none_or(|id| e.id == id))
        .collect();
    let shown = &events[events.len().saturating_sub(HISTORY_SHOWN)..];
    for event in shown {
        connection.send(&Answer::Event(event.clone())).map_err(|e| format!("{e}"))?;
    }
    Ok(format!("{} of {} events", shown.len(), events.len()))
}

/// `bundle catalog`: each catalog, and what bundled last read of it.
fn list_catalogs(connection: &mut Connection) -> Result<String, String> {
    let (all, errors) = read_catalogs();
    let state = Path::new(catalogs::STATE);
    for catalog in &all {
        let index = catalogs::read_index(state, &catalog.name);
        let shown = Shown {
            name: catalog.name.clone(),
            url: catalog.url.clone(),
            off: catalog.off,
            default: catalog.default,
            bundles: index.as_ref().map_or(0, |i| i.entries.len()),
            read: index.as_ref().map(|i| i.read),
            expires: index.as_ref().map(|i| i.expires),
        };
        connection.send(&Answer::Catalog(shown)).map_err(|e| format!("{e}"))?;
    }
    let mut said = format!("{} catalogs", all.len());
    for why in errors {
        said += &format!("; left out: {why}");
    }
    Ok(said)
}

/// Makes `path`, a folder, root's with `mode`.
fn make_folder(path: &Path, mode: u32) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(path)
        .and_then(|()| fs::set_permissions(path, fs::Permissions::from_mode(mode)))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `bundle catalog add NAME URL ROOT`: an administrator's. The root must
/// be a root signed by its own keys, and the catalog must read with it
/// before it is kept.
fn catalog_add(
    shared: &Shared,
    asker: u32,
    name: &str,
    url: &str,
    root: &File,
) -> Result<String, String> {
    if asker != 0 {
        return Err("adding a catalog is an administrator's: run elevate bundle catalog add".into());
    }
    let mut bytes = Vec::new();
    root.take(512 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| format!("its root: {e}"))?;
    if bytes.len() > 512 * 1024 {
        return Err("its root is larger than a root may be".into());
    }
    catalogs::check_root(&bytes).map_err(|why| format!("its root: {why}"))?;
    let dir = Path::new(catalogs::SETTINGS).join(name);
    let (all, _) = read_catalogs();
    if let Some(there) = all.iter().find(|c| c.name == name)
        && !there.off
    {
        return Err(format!("there is a catalog {name} already"));
    }
    let catalog = Catalog {
        name: name.into(),
        url: url.into(),
        root: bytes.clone(),
        off: false,
        default: false,
    };
    let _reading = shared.catalogs();
    let state = Path::new(catalogs::STATE);
    let read = catalogs::open(&catalog, state).map_err(|why| {
        catalogs::forget(state, name);
        format!("{url} does not read as a catalog with that root: {why}")
    })?;
    make_folder(Path::new(catalogs::SETTINGS), 0o755)?;
    make_folder(&dir, 0o755)?;
    write_whole(&dir.join(catalogs::URL_FILE).display().to_string(), &format!("{url}\n"), 0o644)?;
    let root_path = dir.join(catalogs::ROOT_FILE);
    let written = String::from_utf8(bytes).map_err(|_| "its root is not text".to_string())?;
    write_whole(&root_path.display().to_string(), &written, 0o644)?;
    let _ = fs::remove_file(dir.join(catalogs::OFF_FILE));
    let count = read.index.entries.len();
    let lists = format!("{count} bundle{}", if count == 1 { "" } else { "s" });
    audit!("root added catalog {name}, at {url}, which lists {lists}");
    Ok(format!("added catalog {name}, which lists {lists}"))
}

/// `bundle catalog remove NAME`: an administrator's. One the system comes
/// with is turned off, so that Spark does not bring it back; any other
/// is removed. Bundles installed from it stay, bound to it.
fn catalog_remove(shared: &Shared, asker: u32, name: &str) -> Result<String, String> {
    if asker != 0 {
        return Err(
            "removing a catalog is an administrator's: run elevate bundle catalog remove".into()
        );
    }
    let (all, _) = read_catalogs();
    let catalog =
        all.iter().find(|c| c.name == name).ok_or_else(|| format!("there is no catalog {name}"))?;
    let dir = Path::new(catalogs::SETTINGS).join(name);
    let _reading = shared.catalogs();
    if catalog.default {
        write_whole(&dir.join(catalogs::OFF_FILE).display().to_string(), "", 0o644)?;
    } else {
        fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    catalogs::forget(Path::new(catalogs::STATE), name);
    audit!("root removed catalog {name}");
    Ok(if catalog.default {
        format!("turned catalog {name} off; `bundle catalog add` with its root turns it on again")
    } else {
        format!("removed catalog {name}")
    })
}

fn serve(shared: &Shared, stream: UnixStream) {
    let Some(asker) = asker(&stream) else { return };
    let deadline = Instant::now() + REQUEST_TIME;
    let mut fds = Vec::new();
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 512];
    while !bytes.contains(&b'\n') && bytes.len() <= LINE_MAX {
        if readable(&stream, deadline).is_err() {
            return;
        }
        match signin::receive(&stream, &mut buffer, &mut fds) {
            Ok(0) | Err(_) => return,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
    let Some(end) = bytes.iter().position(|&b| b == b'\n') else { return };
    let rest = bytes.split_off(end + 1);
    bytes.pop();
    let Ok(input) = stream.try_clone() else { return };
    let input = Timed { stream: input, deadline };
    let mut connection = Connection { stream, input: BufReader::new(input) };
    if !rest.is_empty() {
        // Nothing comes after a request but an answer to a question.
        let _ = connection.send(&Answer::Error("one request at a time".into()));
        return;
    }
    let request = String::from_utf8(bytes).map_err(|_| "not text".to_string());
    let request = request.and_then(|line| Request::read(&line));
    let result = match request {
        Err(why) => Err(why),
        Ok(Request::Install { everyone, yes }) => match fds.pop() {
            Some(fd) if fds.is_empty() => {
                let how = How { yes, ..How::default() };
                install(shared, Some(&mut connection), asker, &File::from(fd), everyone, how)
            }
            _ => Err("an install request comes with its archive, and nothing else".into()),
        },
        Ok(Request::CatalogAdd { name, url }) => match fds.pop() {
            Some(fd) if fds.is_empty() => catalog_add(shared, asker, &name, &url, &File::from(fd)),
            _ => Err("adding a catalog comes with its root, and nothing else".into()),
        },
        Ok(_) if !fds.is_empty() => {
            Err("only an install request, or adding a catalog, comes with a file".into())
        }
        Ok(Request::Get { id, catalog, everyone, yes }) => {
            get(shared, &mut connection, asker, &id, catalog.as_deref(), everyone, yes)
        }
        Ok(Request::Search { words }) => search(shared, &mut connection, &words),
        Ok(Request::Catalogs) => list_catalogs(&mut connection),
        Ok(Request::CatalogRemove { name }) => catalog_remove(shared, asker, &name),
        Ok(Request::Update { ids, everyone, check, yes }) => {
            update(shared, &mut connection, asker, &ids, everyone, check, yes)
        }
        Ok(Request::Rollback { id, everyone }) => rollback(shared, asker, &id, everyone),
        Ok(Request::Hold { id, everyone, on }) => hold(shared, asker, &id, everyone, on),
        Ok(Request::Automatic { everyone, on }) => automatic(shared, asker, everyone, on),
        Ok(Request::History { id }) => history_of(&mut connection, asker, id.as_deref()),
        Ok(Request::Lines { name }) => match lines_of(&shared.lock(), asker, &name) {
            Some(lines) => connection
                .send(&Answer::Lines(lines))
                .map_err(|e| format!("{e}"))
                .map(|()| format!("the lines of {name}")),
            None => Err(format!("no line of {name} is installed")),
        },
        Ok(Request::Default { name, line, everyone }) => {
            set_default(shared, asker, &name, line.as_deref(), everyone)
        }
        Ok(Request::List) => list(shared, &mut connection, asker),
        Ok(Request::Info { id }) => info(shared, &mut connection, asker, &id),
        Ok(Request::Remove { id, everyone, purge }) => remove(shared, asker, &id, everyone, purge),
        Ok(Request::Run { find: what }) => find(shared, &mut connection, asker, &what),
        Ok(Request::AddOns) => link_commands(&shared.lock(), add_on_place(asker))
            .map(|()| format!("linked the commands of {}'s add-ons", whom(add_on_place(asker)))),
        Ok(Request::Ask { id, permission, list }) => {
            ask(shared, &connection, asker, &id, &permission, &list)
        }
        Ok(Request::Grant { id, permission, words }) => {
            grant(shared, &connection, asker, &id, &permission, words)
        }
        Ok(Request::Revoke { id, permission }) => revoke(shared, asker, &id, &permission),
        Ok(Request::Watch { id }) => {
            if installed_for(shared, asker, &id).is_ok()
                && connection.send(&Answer::Ok(format!("watching {id}"))).is_ok()
            {
                watch(shared, asker, id, connection.stream);
            }
            return;
        }
        Ok(Request::DeveloperKey { key }) => {
            let mut registry = shared.lock();
            registry.set_developer_key(asker, key);
            save(&registry).map(|()| format!("kept your developer key {}", key.encoded()))
        }
    };
    let answer = match result {
        Ok(what) => Answer::Ok(what),
        Err(why) => Answer::Error(why),
    };
    let _ = connection.send(&answer);
}

/// Forgets the bundles whose folders went while bundled was not running.
fn notice(registry: &mut Registry) -> bool {
    let gone: Vec<(Place, String)> = registry
        .installed
        .iter()
        .filter(|i| {
            place_folder(i.place).is_ok_and(|(path, _)| {
                fs::symlink_metadata(format!("{path}/{}", i.id))
                    .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
            })
        })
        .map(|i| (i.place, i.id.clone()))
        .collect();
    for (place, id) in &gone {
        if let Some(i) = registry.forget(*place, id) {
            audit!("forgot {id} {} of {}: its folder is gone", i.version, whom(*place));
        }
    }
    // A kept version whose folder is gone, or is not bundled's, cannot be
    // gone back to.
    let lost: Vec<(Place, String)> = registry
        .previous
        .iter()
        .filter(|p| {
            place_folder(p.place).is_ok_and(|(path, _)| {
                fs::symlink_metadata(format!("{path}/{}", previous_name(&p.id)))
                    .map_or(true, |m| std::os::unix::fs::MetadataExt::uid(&m) != 0)
            })
        })
        .map(|p| (p.place, p.id.clone()))
        .collect();
    for (place, id) in &lost {
        registry.forget_previous(*place, id);
    }
    !gone.is_empty() || !lost.is_empty()
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    // SAFETY: setting a signal's disposition touches no memory; no other
    // thread runs yet. A client that goes away must not end bundled.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(registry::STATE)
        .map_err(|e| util::failed(&registry::STATE, e))?;
    let mut texts: [String; 8] = Default::default();
    for (text, name) in texts.iter_mut().zip(registry::FILES) {
        *text = read_state(name).map_err(|e| util::failed(&state(name), e))?;
    }
    let mut registry = Registry::read(&texts).map_err(util::fail)?;
    if notice(&mut registry) {
        save(&registry).map_err(util::fail)?;
    }
    let grants = read_grants()?;
    if let Ok(dir) = Dir::open(EVERYONE) {
        tidy(&dir);
    }
    catalogs::tidy(Path::new(catalogs::STATE));
    feeds::tidy(Path::new(feeds::STATE));
    let (known, errors) = read_catalogs();
    for why in errors {
        eprintln!("bundled: a catalog is left out: {why}");
    }
    // The links, from what it recorded: for everyone, and for each person
    // with a bundle.
    let mut places: Vec<Place> = registry.installed.iter().map(|i| i.place).collect();
    places.push(Place::Everyone);
    places.sort();
    places.dedup();
    for place in places {
        if let Err(why) = link_commands(&registry, place) {
            eprintln!("bundled: the commands of {}: {why}", whom(place));
        }
    }
    let socket = protocol::SOCKET;
    let folder = Path::new(socket).parent().unwrap_or(Path::new("/"));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(folder)
        .map_err(|e| util::failed(&folder.display(), e))?;
    let _ = fs::remove_file(socket);
    let listener = UnixListener::bind(socket).map_err(|e| util::failed(&socket, e))?;
    // Anyone may ask: it tells who asks by the connection.
    fs::set_permissions(socket, fs::Permissions::from_mode(0o666))
        .map_err(|e| util::failed(&socket, e))?;
    let on: Vec<&str> = known.iter().filter(|c| !c.off).map(|c| c.name.as_str()).collect();
    if !on.is_empty() {
        eprintln!("bundled: installs from the catalogs {}", on.join(", "));
    }
    eprintln!("bundled: {} bundles installed; answering at {socket}", registry.installed.len());
    let shared = Arc::new(Shared {
        registry: Mutex::new(registry),
        grants: Mutex::new(grants),
        watchers: Mutex::new(Vec::new()),
        connections: AtomicUsize::new(0),
        catalogs: Mutex::new(()),
        wake: (Mutex::new(false), Condvar::new()),
    });
    let updating = Arc::clone(&shared);
    if std::thread::Builder::new().name("updater".into()).spawn(move || updater(&updating)).is_err()
    {
        eprintln!("bundled: cannot start the thread that updates bundles by themselves");
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= CONNECTIONS_MAX {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            let mut stream = stream;
            let _ = stream.write_all(&Answer::Error("bundled is busy: try again".into()).bytes());
            continue;
        }
        let answering = Arc::clone(&shared);
        let started = std::thread::Builder::new().name("answer".into()).spawn(move || {
            serve(&answering, stream);
            answering.connections.fetch_sub(1, Ordering::SeqCst);
        });
        if started.is_err() {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            eprintln!("bundled: cannot start a thread for a connection");
        }
    }
    Ok(0)
}
