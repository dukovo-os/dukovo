// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Running a bundle (ADR 0225): `bundle run`, and a bundle's command by
//! its name, through the link bundled(8) made for it.
//!
//! bundled says which bundle it is, where, and the SHA-256 of the signed
//! list it recorded at install. The runner believes nothing else: it
//! opens the bundle's folder, not through a link, and checks that it is
//! still sealed (root's, mode 0555), that its list is the one recorded and
//! signed by the key recorded, and that its manifest is the one the list
//! names. The program it runs is a regular file of root's that nobody may
//! change, named in the list.
//!
//! A command that runs with your access is executed as a program of the
//! system's, once the person allowed runs-with-your-access. Any other
//! runs in the bundle's sandbox: a process of its own whose root is a
//! view (ADR 0215) of the system's commands, libraries, resources and
//! settings, read-only, the devices every program has, the bundle's
//! folder, read-only, its data folder, `~/app-data/ID`, which it may
//! change, and a `/tmp` of its own, each at its real path; with the
//! network only as its manifest asks, a clean environment, and the
//! terminal as descriptors 0 to 2.
//!
//! What else it gets is what the person allowed (ADR 0224,
//! `bundles::grants`): bundled says what they said of the app's
//! permissions, and asks them, through the trusted prompt, about each one
//! asked on first use or with a warning that they have not answered for
//! good. Each folder given is attached at its own path, read-only or not
//! as given. A run that holds a permission follows what the person takes
//! away while it runs, through a connection to bundled: as a view keeps
//! what is attached to it and a socket outlives the capability it was
//! made with, the program is stopped, SIGTERM and two seconds later
//! SIGKILL, and its next run goes without.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use manifest::bundle::{
    AddOns, Arch, BUNDLE_LINES, Manifest, Need, Request as Asked, RunsWith, Wish,
};
use manifest::service::Ports;

use super::folder::{Dir, fd_stat};
use super::grants::{self, Grant, Step, YOUR_ACCESS};
use super::protocol::{AddOn, Answer, Find, Found, Lines, Request};
use super::signed::{self, LIST_MAX, MANIFEST_MAX, SIGNATURE_MAX};
use super::sums::{self, Entry, LIST, SIGNATURE};
use super::{MANIFEST, answer, ask};
use crate::bundled;
use crate::util::{self, Fail, fail};

/// The architecture this system runs.
pub const ARCH: Arch = Arch::X86_64;

/// What a sandboxed program's view holds besides its bundle, its data
/// and its `/tmp`, each at its path, and whether it may change it: the
/// system's own, read-only, and the devices every program has. A place
/// this system lacks is left out.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const ALWAYS: &[(&str, bool)] = &[
    ("/system/commands", false),
    ("/system/libraries", false),
    ("/system/resources", false),
    ("/system/settings", false),
    ("/system/live/notify", false),
    ("/processes", false),
    ("/kernel", false),
    ("/devices/null", true),
    ("/devices/zero", false),
    ("/devices/random", false),
    ("/devices/urandom", false),
];

/// What a bundle that connects out also gets: resolved's folder, which
/// holds the resolver's settings that `/system/settings/resolv.conf`
/// leads to (ADR 0193), and netd's, which an older one leads to.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const NETWORK: &[&str] = &["/system/live/resolved", "/system/live/netd"];

/// What a bundle that reaches the local network (`network.local`) also
/// gets: discoverd's folder, whose socket says what is on it (ADR 0338).
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const DISCOVERY: &str = crate::discoverd::FOLDER;

/// Where drives you plug in appear: `files.volumes`.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const VOLUMES: &str = "/volumes";

/// The folders of `/shared`: `files.shared`.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub const SHARED: &str = "/shared";

/// The serial ports' terminals that `devices.serial` gives, by the
/// beginnings of their names in `/devices` (ADR 0246).
const SERIAL: &[&str] = &["ttyACM", "ttyUSB"];

/// `bundle run [--command NAME] ID [ARGUMENT...]`.
pub fn run(id: &str, command: Option<&str>, args: Vec<OsString>) -> Result<i32, Fail> {
    manifest::bundle::check_full_id(id).map_err(util::usage)?;
    match locate(&Find::Id(id.into()))? {
        Target::Bundle(found, said) => start(&found, &said, command, args),
        Target::AddOn(_) => Err(fail("bundled answered out of turn".into())),
    }
}

/// A bundle's command, run as `name` through its link (`notes` for
/// `org.example.notes`'s): its exit status.
pub fn command(name: &str, args: Vec<OsString>) -> i32 {
    let result = manifest::bundle::check_command_name(name)
        .map_err(fail)
        .and_then(|()| locate(&Find::Command(name.into())))
        .and_then(|target| match target {
            Target::Bundle(found, said) => start(&found, &said, Some(name), args),
            Target::AddOn(add_on) => run_add_on(&add_on, name, args),
        });
    match result {
        Ok(code) => code,
        Err(failure) => {
            eprintln!("{name}: {}", failure.message);
            if failure.code == 0 { 1 } else { failure.code }
        }
    }
}

/// Why a line runs (ADR 0387).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// A project's file asks for it.
    Project(PathBuf),
    /// It is the person's default, or everyone's.
    Mine,
    Everyone,
    /// It is the newest installed.
    Newest,
    /// The add-on that runs it was added with it (ADR 0388).
    AddOn,
}

/// The lines the add-on commands running now were added with, as
/// [`BUNDLE_LINES`] names them, between spaces.
#[must_use]
pub fn pins() -> String {
    std::env::var(BUNDLE_LINES).unwrap_or_default()
}

/// The line of `lines` to run in the folder `here`, and why (ADR 0387):
/// the one an add-on that runs it was added with, as `pins` names it
/// (ADR 0388), then the one the nearest project file asks for, then the
/// person's default, everyone's, and the newest. A project file that asks
/// for a version no line installed is or holds stops it.
pub fn pick(lines: &Lines, here: &Path, pins: &str) -> Result<(String, Why), String> {
    for pin in pins.split_whitespace() {
        if let Ok((id, Some(line))) = manifest::bundle::split_full_id(pin)
            && id == lines.id
            && lines.lines.iter().any(|l| l == line)
        {
            return Ok((line.into(), Why::AddOn));
        }
    }
    if let Some((path, text)) = project_file(&lines.files, here) {
        let all: Vec<&str> = lines.lines.iter().map(String::as_str).collect();
        match manifest::bundle::wished_line(&text, &all) {
            Wish::Line(line) => return Ok((line, Why::Project(path))),
            Wish::Missing(version) => {
                return Err(format!(
                    "this project uses {} {version}, as {} says, which is not installed: bundle \
                     search {} lists the lines the catalogs have",
                    lines.id,
                    path.display(),
                    lines.id
                ));
            }
            Wish::Nothing => {}
        }
    }
    for (default, why) in [(&lines.mine, Why::Mine), (&lines.everyone, Why::Everyone)] {
        if let Some(line) = default
            && lines.lines.contains(line)
        {
            return Ok((line.clone(), why));
        }
    }
    let newest = lines.lines.last().cloned();
    newest
        .map(|line| (line, Why::Newest))
        .ok_or_else(|| format!("no line of {} is installed", lines.id))
}

/// The nearest of `files` from the folder `here` up to `/`: its path and
/// what it starts with.
fn project_file(files: &[String], here: &Path) -> Option<(PathBuf, String)> {
    for folder in here.ancestors() {
        for name in files {
            let path = folder.join(name);
            let Ok(file) = File::open(&path) else { continue };
            if !file.metadata().is_ok_and(|m| m.is_file()) {
                continue;
            }
            let mut bytes = Vec::new();
            let _ = file.take(256).read_to_end(&mut bytes);
            return Some((path, String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    None
}

/// What `run` runs.
enum Target {
    /// A bundle's program, and what the person said of its permissions.
    Bundle(Found, Vec<Grant>),
    /// A command of a line's add-ons (ADR 0388).
    AddOn(AddOn),
}

/// Asks bundled which bundle `find` names: it, and what the person
/// said of its permissions. When it names several lines of an ID, the
/// one [`pick`] picks in the working folder (ADR 0387); a command no
/// bundle adds may be an add-on's (ADR 0388).
fn locate(find: &Find) -> Result<Target, Fail> {
    let (_stream, mut input) = ask(&Request::Run { find: find.clone() }, None).map_err(fail)?;
    let mut found = None;
    let mut lines = None;
    let mut add_on = None;
    let mut said = Vec::new();
    let command = matches!(find, Find::Command(_));
    loop {
        let first = found.is_none() && lines.is_none() && add_on.is_none();
        match answer(&mut input)? {
            Answer::Found(f) if first => found = Some(f),
            Answer::Lines(l) if first => lines = Some(l),
            Answer::AddOn(a) if first && command => add_on = Some(a),
            Answer::Granted(g) if found.is_some() => said.push(g),
            Answer::Ok(_) => break,
            Answer::Error(why) => return Err(fail(why)),
            _ => return Err(fail("bundled answered out of turn".into())),
        }
    }
    if let Some(add_on) = add_on {
        return Ok(Target::AddOn(add_on));
    }
    if let Some(lines) = lines {
        let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let (line, _) = pick(&lines, &here, &pins()).map_err(fail)?;
        let full = format!("{}@{line}", lines.id);
        if find == &Find::Id(full.clone()) {
            return Err(fail("bundled answered out of turn".into()));
        }
        return locate(&Find::Id(full));
    }
    let found = found.ok_or_else(|| fail("bundled found nothing".into()))?;
    let asked = match find {
        Find::Id(id) => found.id == *id || super::registry::base_id(&found.id) == id,
        Find::Command(_) => true,
    };
    if !asked {
        return Err(fail("bundled found another bundle than the one asked for".into()));
    }
    Ok(Target::Bundle(found, said))
}

/// The most lines [`BUNDLE_LINES`] names.
const PINS_MAX: usize = 16;

/// Runs `add_on`, a command of a line's add-ons, as `name` with `args`
/// (ADR 0388): with the person's access, as a program of theirs, with its
/// line first in [`BUNDLE_LINES`], so that the line's commands it runs
/// are of the line it was added with.
fn run_add_on(add_on: &AddOn, name: &str, args: Vec<OsString>) -> Result<i32, Fail> {
    use std::os::unix::process::CommandExt;
    let base = super::registry::base_id(&add_on.id);
    let now = pins();
    let others = now.split_whitespace().filter(|pin| super::registry::base_id(pin) != base);
    let pins: Vec<&str> =
        std::iter::once(add_on.id.as_str()).chain(others).take(PINS_MAX).collect();
    let error = std::process::Command::new(&add_on.path)
        .arg0(name)
        .args(args)
        .env(BUNDLE_LINES, pins.join(" "))
        .exec();
    Err(util::failed(&add_on.path, error))
}

/// Runs `command`, a command of `found`'s line that runs with your
/// access, with the line's add-ons folder in the variables `add_ons`
/// names, made if it is not there (ADR 0388). When the folder of their
/// commands changed, bundled links them beside the person's commands.
/// Its exit status, as a shell gives it.
fn with_add_ons(
    found: &Found,
    add_ons: &AddOns,
    mut command: std::process::Command,
) -> Result<i32, Fail> {
    use std::os::unix::process::ExitStatusExt;
    // SAFETY: getuid has no failure and touches no memory.
    let uid = unsafe { libc::getuid() };
    let folder = add_ons_folder(&found.id, uid).map_err(fail)?;
    for variable in &add_ons.variables {
        command.env(&variable.0, &folder);
    }
    let commands = format!("{folder}/{}", add_ons.commands);
    let before = stamp(&commands);
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command.spawn().map_err(|e| util::failed(&program, e))?;
    // As system(3): the terminal's Ctrl+C is the program's to take.
    // SAFETY: setting dispositions touches no memory.
    let old = unsafe {
        (libc::signal(libc::SIGINT, libc::SIG_IGN), libc::signal(libc::SIGQUIT, libc::SIG_IGN))
    };
    let status = child.wait();
    // SAFETY: as above.
    unsafe {
        libc::signal(libc::SIGINT, old.0);
        libc::signal(libc::SIGQUIT, old.1);
    }
    let status = status.map_err(|e| util::failed(&program, e))?;
    if stamp(&commands) != before
        && let Err(why) = link_add_ons()
    {
        eprintln!("bundle: {}: the commands of its add-ons are not linked: {why}", found.id);
    }
    Ok(status.code().or_else(|| status.signal().map(|s| 128 + s)).unwrap_or(1))
}

/// The add-ons folder of line `id` for `uid`, made theirs if it is not
/// there: `~/applications/add-ons/ID`, or for an administrator,
/// everyone's in `/applications` (ADR 0388).
fn add_ons_folder(id: &str, uid: u32) -> Result<String, String> {
    let applications = if uid == 0 {
        bundled::EVERYONE.to_string()
    } else {
        let user = bundled::account(uid)?;
        format!("{}/{}", user.home.trim_end_matches('/'), bundled::APPLICATIONS)
    };
    let add_ons = format!("{applications}/{}", bundled::ADD_ONS);
    let folder = format!("{add_ons}/{id}");
    for path in [&applications, &add_ons, &folder] {
        folder_of(path, uid, 0o755)?;
    }
    Ok(folder)
}

/// When the folder at `path` last changed, and which it is; none if
/// there is none.
fn stamp(path: &str) -> Option<(i64, i64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).ok().map(|m| (m.mtime(), m.mtime_nsec(), m.ino()))
}

/// Has bundled link the commands of the person's add-ons anew.
fn link_add_ons() -> Result<(), String> {
    let (_stream, mut input) = ask(&Request::AddOns, None)?;
    match answer(&mut input).map_err(|f| f.message)? {
        Answer::Ok(_) => Ok(()),
        Answer::Error(why) => Err(why),
        _ => Err("bundled answered out of turn".into()),
    }
}

/// Asks bundled to ask the person whether the bundle `found` names may
/// have `permission`: their answer's word.
fn ask_person(found: &Found, permission: &str) -> Result<String, String> {
    let request = Request::Ask {
        id: found.id.clone(),
        permission: permission.into(),
        list: found.list.clone(),
    };
    let (_stream, mut input) = ask(&request, None)?;
    match answer(&mut input).map_err(|f| f.message)? {
        Answer::Ok(word) => Ok(word),
        Answer::Error(why) => Err(why),
        _ => Err("bundled answered out of turn".into()),
    }
}

/// Follows what the person takes away from a run of `found`'s bundle:
/// bundled's answer to `watch`, and the connection to read the rest from.
fn watch(found: &Found) -> Option<BufReader<UnixStream>> {
    let (_stream, mut input) = ask(&Request::Watch { id: found.id.clone() }, None).ok()?;
    match answer(&mut input) {
        Ok(Answer::Ok(_)) => Some(input),
        _ => None,
    }
}

/// A bundle's folder, checked: still as bundled sealed it, its list the
/// one recorded, its manifest the one the list names.
pub struct Sealed {
    pub folder: Dir,
    /// Its path.
    pub path: String,
    pub manifest: Manifest,
    pub listed: Vec<Entry>,
}

/// Whether `st` is root's and nobody may change it.
fn sealed_mode(st: &libc::stat) -> bool {
    st.st_uid == 0 && st.st_mode & 0o222 == 0
}

/// Reads the file `name` of the sealed folder, which must be sealed too.
fn sealed_file(folder: &Dir, path: &str, name: &str, max: u64) -> Result<Vec<u8>, String> {
    let (bytes, st) = folder.read(name, max).map_err(|e| format!("{path}/{name}: {e}"))?;
    if !sealed_mode(&st) {
        return Err(format!("{path}/{name} is not sealed: bundled did not install it so"));
    }
    Ok(bytes)
}

/// Opens and checks the bundle `found` names.
pub fn open_sealed(found: &Found) -> Result<Sealed, String> {
    let (places, owner) = bundled::place_folder(found.place)?;
    let dir = Dir::open(&places).map_err(|e| format!("{places}: {e}"))?;
    if dir.stat().map_err(|e| format!("{places}: {e}"))?.st_uid != owner {
        return Err(format!("{places} is not its owner's"));
    }
    let path = format!("{places}/{}", found.id);
    let folder = dir.dir_at(&found.id).map_err(|e| format!("{path}: {e}"))?;
    let st = folder.stat().map_err(|e| format!("{path}: {e}"))?;
    if st.st_uid != 0 || st.st_mode & 0o7777 != 0o555 {
        return Err(format!("{path} is not sealed: bundled did not install it so"));
    }
    let list = sealed_file(&folder, &path, LIST, LIST_MAX)?;
    if sums::hex(&sums::sha256(&list)) != found.list {
        return Err(format!("{path}: its signed list is not the one bundled installed"));
    }
    let signature = sealed_file(&folder, &path, SIGNATURE, SIGNATURE_MAX)?;
    let (key, _, listed) = signed::check_list(&list, &signature)?;
    if key != found.key {
        return Err(format!("{path}: it is signed by another key than the one installed"));
    }
    let bytes = sealed_file(&folder, &path, MANIFEST, MANIFEST_MAX)?;
    let manifest = signed::manifest_of(&bytes, &listed)?;
    if manifest.full_id() != found.id {
        return Err(format!("{path}: its manifest is {}'s", manifest.full_id()));
    }
    Ok(Sealed { folder, path, manifest, listed })
}

/// The program to run, and how.
#[derive(Debug, PartialEq, Eq)]
pub struct Chosen {
    /// Its name among the bundle's programs.
    pub program: String,
    /// Its path in the bundle.
    pub path: String,
    pub runs_with: RunsWith,
    /// The name it is run by: its command's, or its own.
    pub arg0: String,
}

/// The program `command` runs: that command's; or with none, the
/// bundle's first command's, or its only program.
pub fn choose(manifest: &Manifest, command: Option<&str>) -> Result<Chosen, String> {
    let id = &manifest.id;
    let (arg0, program, runs_with) = match command {
        Some(name) => {
            // A line's command runs as NAME-LINE too (ADR 0387).
            let lined = || {
                let line = manifest.line.as_ref()?;
                let base = name.strip_suffix(line.0.as_str())?.strip_suffix('-')?;
                manifest.commands.iter().find(|c| c.name == *base)
            };
            let found = manifest.commands.iter().find(|c| c.name == *name).or_else(lined);
            let c = found.ok_or_else(|| format!("{id} adds no command {name}"))?;
            (c.name.to_string(), c.program.to_string(), c.runs_with)
        }
        None => match manifest.commands.first() {
            Some(c) => (c.name.to_string(), c.program.to_string(), c.runs_with),
            None if manifest.programs.len() == 1 => {
                let (name, _) = manifest.programs.iter().next().expect("one program");
                (name.to_string(), name.to_string(), RunsWith::Sandbox)
            }
            None => return Err(format!("{id} adds no command, so there is none to run")),
        },
    };
    let found = manifest.programs.get(program.as_str());
    let p = found.ok_or_else(|| format!("{id} has no program {program}"))?;
    let path = p
        .path
        .get(&ARCH)
        .ok_or_else(|| format!("{id} has no {program} for {} computers", ARCH.word()))?;
    Ok(Chosen { program, path: path.0.clone(), runs_with, arg0 })
}

/// What a sandboxed program is given of the permissions its bundle asks
/// for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Given {
    /// `network.outbound`: to connect out, on every port, to machines
    /// beyond this one's networks.
    pub outbound: bool,
    /// `network.local`: to reach the machines on this one's networks, and
    /// find them (ADR 0337).
    pub local: bool,
    /// `network.listen`: the ports, each with whether it is required.
    pub listen: Vec<(Ports, bool)>,
    /// `files.folder`: your folders, by name, each with whether it may
    /// change it.
    pub folders: Vec<(String, bool)>,
    /// `files.shared`: folders of `/shared`, the same way.
    pub shared: Vec<(String, bool)>,
    /// `files.home`: all of your files.
    pub home: bool,
    /// `files.volumes`: the drives you plug in.
    pub volumes: bool,
    /// `files.app-data`: other apps' data, by ID, to read.
    pub app_data: Vec<String>,
    /// `devices.serial`: the serial ports' terminals.
    pub serial: bool,
    /// `keyring.shared`: every entry of your keyring, beside its own
    /// (ADR 0396).
    pub keyring_shared: bool,
    /// The permissions given, by name: taking one away stops the program.
    pub names: Vec<String>,
    /// The optional permissions it runs without, by name.
    pub without: Vec<String>,
}

/// The permissions `program` of `manifest` keeps, with what it asks of
/// each: those its bundle asks for, as many as the program keeps
/// (`programs.NAME.permissions`). runs-with-your-access is a command's
/// to ask for, not a sandboxed program's.
pub fn kept<'m>(manifest: &'m Manifest, program: &str) -> Vec<(&'m str, &'m Asked)> {
    let narrowed = manifest.programs.get(program).map_or(&[][..], |p| p.permissions.as_slice());
    manifest
        .permissions
        .iter()
        .filter(|(name, _)| narrowed.is_empty() || narrowed.iter().any(|n| n == name))
        .filter(|(name, _)| *name != YOUR_ACCESS)
        .collect()
}

/// Adds permission `name`, asked as `request`, given with `words`.
pub fn give(given: &mut Given, name: &str, request: &Asked, words: &[String]) {
    let required = request.need == Need::Required;
    let folders = || -> Vec<(String, bool)> {
        words
            .iter()
            .filter_map(|w| w.split_once('='))
            .map(|(folder, access)| (folder.to_string(), access == "read-write"))
            .collect()
    };
    match name {
        "network.outbound" => given.outbound = true,
        "network.local" => given.local = true,
        "network.listen" => given.listen.extend(request.ports.iter().map(|&p| (p, required))),
        "files.folder" => given.folders.extend(folders()),
        "files.shared" => given.shared.extend(folders()),
        "files.home" => given.home = true,
        "files.volumes" => given.volumes = true,
        "files.app-data" => given.app_data.extend(words.iter().cloned()),
        "devices.serial" => given.serial = true,
        "keyring.shared" => given.keyring_shared = true,
        // Notifications are always allowed; starting at sign-in is the
        // session's to do, not the program's.
        _ => {}
    }
    given.names.push(name.into());
}

/// What `program` of `manifest` is given, from what the person said of
/// its permissions (`said`); `person` asks them about each one they have
/// not answered for good, and gives their answer's word. A permission it
/// requires and does not get stops it, saying why.
pub fn given(
    manifest: &Manifest,
    program: &str,
    said: &[Grant],
    mut person: impl FnMut(&str) -> Result<String, String>,
) -> Result<Given, String> {
    let id = &manifest.full_id();
    let mut given = Given::default();
    for (name, request) in kept(manifest, program) {
        let grant = said.iter().find(|g| g.permission == name);
        let step = match grants::step(id, name, request, grant)? {
            Step::Ask => grants::answered(id, name, request, &person(name)?)?,
            step => step,
        };
        match step {
            Step::Give(words) => give(&mut given, name, request, &words),
            Step::Without | Step::Ask => given.without.push(name.into()),
        }
    }
    Ok(given)
}

/// Whether a command of `found`'s bundle that runs with your access may:
/// once the person allowed runs-with-your-access, asked with a warning.
fn your_access(found: &Found, said: &[Grant]) -> Result<(), String> {
    let request = Asked {
        need: Need::Required,
        why: None,
        folders: Vec::new(),
        ports: Vec::new(),
        apps: Vec::new(),
    };
    let grant = said.iter().find(|g| g.permission == YOUR_ACCESS);
    let step = match grants::step(&found.id, YOUR_ACCESS, &request, grant)? {
        Step::Ask => {
            grants::answered(&found.id, YOUR_ACCESS, &request, &ask_person(found, YOUR_ACCESS)?)?
        }
        step => step,
    };
    match step {
        Step::Give(_) => Ok(()),
        _ => Err(format!("{} may not run with your access", found.id)),
    }
}

/// The environment a sandboxed program starts with: a clean one (ADR
/// 0225), its data folder as its home, and the caller's terminal type,
/// language and time zone.
pub fn environment(user: &accounts::User, home: &str, caller: &[(String, String)]) -> Vec<String> {
    let mut env = vec![
        format!("HOME={home}"),
        format!("USER={}", user.name),
        format!("LOGNAME={}", user.name),
        "PATH=/system/commands".into(),
    ];
    let term = caller.iter().find(|(name, _)| name == "TERM").map_or("", |(_, v)| v.as_str());
    env.push(format!("TERM={}", crate::signin::terminal_type(term)));
    for (name, value) in caller {
        let entry = format!("{name}={value}");
        let zone = name == "TZ"
            && value.len() <= 64
            && value.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-:,.".contains(c));
        if zone || crate::elevate::language_setting(&entry) {
            env.push(entry);
        }
    }
    env
}

/// Opens the program at `path` in the sealed folder, one part at a time,
/// never through a link: a regular file of root's, nobody's to change,
/// that may run, and that the signed list names.
fn open_program(sealed: &Sealed, path: &str) -> Result<OwnedFd, String> {
    let shown = format!("{}/{path}", sealed.path);
    if !sealed.listed.iter().any(|e| e.path == path) {
        return Err(format!("{shown}: its signed list does not name it"));
    }
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().ok_or_else(|| format!("{shown}: no file"))?;
    let mut dirs: Vec<Dir> = Vec::new();
    for part in parts {
        let at = dirs.last().unwrap_or(&sealed.folder);
        let next = at.dir_at(part).map_err(|e| format!("{shown}: {e}"))?;
        dirs.push(next);
    }
    let at = dirs.last().unwrap_or(&sealed.folder);
    let fd = at.open_at(name, false).map_err(|e| format!("{shown}: {e}"))?;
    let st = fd_stat(&fd).map_err(|e| format!("{shown}: {e}"))?;
    let regular = st.st_mode & libc::S_IFMT == libc::S_IFREG;
    if !regular || !sealed_mode(&st) || st.st_mode & 0o111 == 0 {
        return Err(format!("{shown} is not a sealed program"));
    }
    Ok(fd)
}

/// The caller's environment, as pairs.
fn caller_environment() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// Makes the folder `path` if it is not there, mode 0700, and checks that
/// it is a folder of `uid`'s, not a link.
fn own_folder(path: &str, uid: u32) -> Result<(), String> {
    folder_of(path, uid, 0o700)
}

/// Makes the folder at `path` with `mode` if it is not there, and checks
/// that it is a folder of `uid`'s.
fn folder_of(path: &str, uid: u32, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(mode).create(path) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => {
            return Err(format!("{path}: {}", util::strip_os_error(&e)));
        }
        _ => {}
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(format!("{path} is not a folder of yours"));
    }
    Ok(())
}

/// The serial ports' terminals there are now, by path.
fn serial_ports() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/devices") else { return Vec::new() };
    let mut ports: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| SERIAL.iter().any(|p| name.starts_with(p)))
        .map(|name| format!("/devices/{name}"))
        .collect();
    ports.sort();
    ports
}

/// Runs what `command` names of the bundle `found` names, with `args`,
/// given what the person allows of what `said` says.
fn start(
    found: &Found,
    said: &[Grant],
    command: Option<&str>,
    args: Vec<OsString>,
) -> Result<i32, Fail> {
    let sealed = open_sealed(found).map_err(fail)?;
    let chosen = choose(&sealed.manifest, command).map_err(fail)?;
    let program = open_program(&sealed, &chosen.path).map_err(fail)?;
    let path = format!("{}/{}", sealed.path, chosen.path);
    if chosen.runs_with == RunsWith::YourAccess {
        your_access(found, said).map_err(fail)?;
        // As a program of the system's: with all the caller holds.
        use std::os::unix::process::CommandExt;
        drop(program);
        let mut command = std::process::Command::new(&path);
        command.arg0(&chosen.arg0).args(args);
        if let Some(add_ons) = &sealed.manifest.add_ons {
            return with_add_ons(found, add_ons, command);
        }
        let error = command.exec();
        return Err(util::failed(&path, error));
    }
    let given = given(&sealed.manifest, &chosen.program, said, |name| ask_person(found, name))
        .map_err(fail)?;
    // SAFETY: getuid has no failure and touches no memory.
    let uid = unsafe { libc::getuid() };
    let user = bundled::account(uid).map_err(fail)?;
    let home = user.home.trim_end_matches('/');
    // Each folder of theirs it is given, there to attach, and theirs.
    if !given.home {
        for (folder, _) in &given.folders {
            own_folder(&format!("{home}/{folder}"), uid).map_err(fail)?;
        }
    }
    let serial = if given.serial { serial_ports() } else { Vec::new() };
    let app_data = format!("{home}/{}", bundled::APP_DATA);
    own_folder(&app_data, uid).map_err(fail)?;
    let data = format!("{app_data}/{}", found.id);
    own_folder(&data, uid).map_err(fail)?;
    // Another version than last ran: its data as that one left it is
    // kept first, or brought back (ADR 0384). As the person, so only what
    // they may read is copied.
    let version = &sealed.manifest.version.0;
    match super::snapshots::carry(Path::new(&app_data), &found.id, version, SystemTime::now()) {
        Ok(Some(said)) => eprintln!("bundle: {said}"),
        Ok(None) => {}
        Err(why) => eprintln!("bundle: {}: its data as it was is not kept: {why}", found.id),
    }
    let tmp = format!("/tmp/.app-{}", found.id);
    own_folder(&tmp, uid).map_err(fail)?;
    // Its keyring, served for this run at authd's path in its view (ADR
    // 0396), from a folder of the person's alone. Without it, the program
    // runs and finds authd unreachable.
    let keyring = format!("{tmp}.keyring-{}", std::process::id());
    let served = own_folder(&keyring, uid)
        .and_then(|()| super::keyring::serve(keyring, &found.id, given.keyring_shared))
        .inspect_err(|why| eprintln!("bundle: {}: no keyring: {why}", found.id))
        .ok();
    let env = environment(&user, &data, &caller_environment());
    let mut argv: Vec<Vec<u8>> = vec![chosen.arg0.clone().into_bytes()];
    argv.extend(args.iter().map(|a| a.as_bytes().to_vec()));
    let plan = Plan {
        sealed: &sealed,
        program: &program,
        path: &path,
        argv: &argv,
        env: &env,
        given: &given,
        home,
        data: &data,
        tmp: &tmp,
        serial: &serial,
        keyring: served.as_ref(),
    };
    // Opened before the sandbox takes its slots, as reading anything is.
    let watching = if given.names.is_empty() { None } else { watch(found) };
    sandbox::run(&plan, watching).map_err(|why| fail(format!("{}: {why}", found.id)))
}

/// What the sandbox is built from.
#[cfg_attr(not(target_os = "radix"), allow(dead_code))]
pub struct Plan<'a> {
    pub sealed: &'a Sealed,
    /// The program, open.
    pub program: &'a OwnedFd,
    /// Its path, the same in the view.
    pub path: &'a str,
    pub argv: &'a [Vec<u8>],
    pub env: &'a [String],
    pub given: &'a Given,
    /// The person's home.
    pub home: &'a str,
    /// Its data folder and its `/tmp`, which it may change.
    pub data: &'a str,
    pub tmp: &'a str,
    /// The serial ports' terminals it is given.
    pub serial: &'a [String],
    /// The keyring served for it, whose folder its view holds at authd's
    /// path; the sandbox opens it once it is built.
    pub keyring: Option<&'a super::keyring::Served>,
}

#[cfg(not(target_os = "radix"))]
mod sandbox {
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;

    use super::Plan;

    /// Built for another system, for tests: there is no sandbox to run in.
    pub fn run(_plan: &Plan<'_>, _watching: Option<BufReader<UnixStream>>) -> Result<i32, String> {
        Err("bundles run on Dukovo".into())
    }
}

#[cfg(target_os = "radix")]
mod sandbox {
    use std::io::{BufReader, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use radix_abi::capability::{self, ObjectKind, Rights};
    use radix_abi::directory::{self, OpenFlags};
    use radix_abi::{Status, network, process, startup};

    use super::{ALWAYS, DISCOVERY, NETWORK, Plan, SHARED, VOLUMES};
    use crate::bundled::APP_DATA;
    use crate::bundles::keyring;
    use crate::bundles::protocol::Answer;
    use crate::signin::{self, Spares};
    use crate::sys::{self, SYS};

    /// How long a program whose permission was taken away has to end
    /// after SIGTERM, before SIGKILL.
    const STOP_TIME: Duration = Duration::from_secs(2);

    /// To read and change, and to copy and pass on, as `fork` and `exec`
    /// need; never the bypass right, and never control.
    const GIVEN: Rights = Rights(
        Rights::READ.bits() | Rights::WRITE.bits() | Rights::COPY.bits() | Rights::TRANSFER.bits(),
    );
    /// To read, and to copy and pass on.
    const READ_ONLY: Rights =
        Rights(Rights::READ.bits() | Rights::COPY.bits() | Rights::TRANSFER.bits());

    fn failed(what: String) -> impl Fn(Status) -> String {
        move |status| format!("{what}: {}", sys::status_name(status.0))
    }

    /// Slots for the work.
    struct Slots {
        base: u32,
        view: u32,
        given: u32,
        scratch: u32,
        child: u32,
        network: u32,
    }

    /// Builds the view, starts the program in it, and waits for it,
    /// stopping it if a permission it was given is taken away while it
    /// runs, as `watching` says.
    pub fn run(plan: &Plan<'_>, watching: Option<BufReader<UnixStream>>) -> Result<i32, String> {
        // Read before the spare slots are taken: an empty spare is a free
        // descriptor to the C library, which reading a folder would take.
        let links = links();
        let spares = Spares::new(6).map_err(|e| format!("no slots: {e}"))?;
        let [base, view, given, scratch, child, network] =
            [0, 1, 2, 3, 4, 5].map(|i| spares.slot(i));
        let slots = Slots { base, view, given, scratch, child, network };
        let number = build_and_start(plan, &slots, &links)?;
        drop(spares);
        // Only now: a connection taken while the spares were held could
        // land in one of their slots, which dropping them empties.
        if let Some(served) = plan.keyring {
            served.open();
        }
        let ended = Arc::new(Mutex::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let following = watching.and_then(|input| {
            let id = plan.sealed.manifest.full_id();
            let flags = (Arc::clone(&ended), Arc::clone(&stopping));
            follow(input, number, plan.given.names.clone(), id, flags)
        });
        let code = wait(number);
        *ended.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        // Stopping what it started may take the rest of STOP_TIME.
        if stopping.load(Ordering::SeqCst)
            && let Some(following) = following
        {
            let _ = following.join();
        }
        Ok(code)
    }

    /// The processes of the program `number` and those it started,
    /// as /processes shows them now.
    fn tree(number: u64) -> Vec<u64> {
        let all = crate::proc::processes().unwrap_or_default();
        let mut found = vec![number];
        let mut next = 0;
        while let Some(&parent) = found.get(next) {
            let children = all.iter().filter(|p| p.parent == parent && p.state != "exited");
            found.extend(
                children.map(|p| p.number).filter(|n| !found.contains(n)).collect::<Vec<_>>(),
            );
            next += 1;
        }
        found
    }

    /// Stops program `number` and what it started when bundled says that
    /// a permission it was given (`names`) was taken away: SIGTERM, then
    /// SIGKILL to each that has not ended `STOP_TIME` later. `ended` says
    /// the program was waited for, after which its number is no longer
    /// its.
    fn follow(
        mut input: BufReader<UnixStream>,
        number: u64,
        names: Vec<String>,
        id: String,
        (ended, stopping): (Arc<Mutex<bool>>, Arc<AtomicBool>),
    ) -> Option<JoinHandle<()>> {
        let signal = move |processes: &[u64], signal: i32| {
            let ended = ended.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for &process in processes {
                let Ok(pid) = i32::try_from(process) else { continue };
                if process != number || !*ended {
                    // SAFETY: kill(2) touches no memory.
                    unsafe { libc::kill(pid, signal) };
                }
            }
        };
        let started = std::thread::Builder::new().name("watch".into()).spawn(move || {
            loop {
                match Answer::read(&mut input) {
                    Ok(Some(Answer::Revoked(name))) if names.contains(&name) => {
                        stopping.store(true, Ordering::SeqCst);
                        // In one write, as other programs write to the
                        // same terminal.
                        let line = format!("{id}: stopped, as you took {name} away from it\n");
                        let _ = std::io::stderr().write_all(line.as_bytes());
                        // Each one found before any is signalled: one whose
                        // parent ends is no longer found as its child.
                        let mut found = tree(number);
                        signal(&found, libc::SIGTERM);
                        std::thread::sleep(STOP_TIME);
                        found.extend(
                            tree(number)
                                .into_iter()
                                .filter(|n| !found.contains(n))
                                .collect::<Vec<_>>(),
                        );
                        signal(&found, libc::SIGKILL);
                        return;
                    }
                    Ok(Some(_)) => {}
                    // bundled went away: nothing can be taken away now.
                    _ => return,
                }
            }
        });
        if started.is_err() {
            eprintln!("bundle: cannot follow what is taken away from the program");
        }
        started.ok()
    }

    /// The links of the root and of `/system`, by their paths without the
    /// leading slash, so `/bin/sh` and `/usr/bin/env` lead where they do.
    fn links() -> Vec<(String, Vec<u8>)> {
        let mut links = Vec::new();
        for dir in ["/", "/system"] {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(target) = std::fs::read_link(&path) else { continue };
                let Some(at) = path.to_str().map(|p| p.trim_start_matches('/').to_string()) else {
                    continue;
                };
                links.push((at, target.into_os_string().into_encoded_bytes()));
            }
        }
        links
    }

    fn build_and_start(
        plan: &Plan<'_>,
        s: &Slots,
        links: &[(String, Vec<u8>)],
    ) -> Result<u64, String> {
        // Without the bypass right or control, whatever this program's
        // root has: the view acts for whoever runs it, as the owners and
        // modes of its files let them.
        let rights =
            capability::info(&SYS, startup::SLOT_ROOT).map_err(failed("the root".into()))?.rights;
        let narrowed = Rights(rights.bits() & GIVEN.bits());
        capability::derive(&SYS, startup::SLOT_ROOT, s.base, narrowed, 0)
            .map_err(failed("narrowing the root".into()))?;
        directory::make_view(&SYS, s.base, s.view).map_err(failed("making its view".into()))?;
        // Opens `path` of the root and attaches it at the same path.
        let attach = |path: &str, write: bool| -> Result<(), String> {
            let at = path.trim_start_matches('/');
            let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{path}: {e}"))?;
            let flags = if meta.is_dir() { OpenFlags::DIRECTORY } else { OpenFlags::NONE };
            let rights = if write { GIVEN } else { READ_ONLY };
            let flags = OpenFlags(flags.bits() | OpenFlags::NO_FOLLOW.bits());
            directory::open(&SYS, s.base, at.as_bytes(), flags, rights, s.scratch, 0)
                .map_err(failed(format!("opening {path}")))?;
            let attached = directory::attach(&SYS, s.view, at.as_bytes(), s.scratch);
            let _ = capability::delete(&SYS, s.scratch);
            attached.map_err(failed(format!("attaching {path}")))
        };
        for &(path, write) in ALWAYS {
            if std::fs::symlink_metadata(path).is_ok() {
                attach(path, write)?;
            }
        }
        for path in NETWORK.iter().filter(|_| plan.given.outbound || plan.given.local) {
            if std::fs::symlink_metadata(path).is_ok() {
                attach(path, false)?;
            }
        }
        if plan.given.local && std::fs::symlink_metadata(DISCOVERY).is_ok() {
            attach(DISCOVERY, false)?;
        }
        // What the person gave it: all of their files, which hold its
        // data, or the folders given and its data; the drives, the
        // folders of /shared and the serial ports given.
        if plan.given.home {
            attach(plan.home, true)?;
        } else {
            for (folder, write) in &plan.given.folders {
                attach(&format!("{}/{folder}", plan.home), *write)?;
            }
            // Its own is its data folder, attached as it may change it.
            let own = &plan.sealed.manifest.full_id();
            for app in plan.given.app_data.iter().filter(|app| *app != own) {
                let path = format!("{}/{APP_DATA}/{app}", plan.home);
                if std::fs::symlink_metadata(&path).is_ok() {
                    attach(&path, false)?;
                }
            }
        }
        if plan.given.volumes && std::fs::symlink_metadata(VOLUMES).is_ok() {
            attach(VOLUMES, true)?;
        }
        for (folder, write) in &plan.given.shared {
            let path = format!("{SHARED}/{folder}");
            if std::fs::symlink_metadata(&path).is_ok() {
                attach(&path, *write)?;
            }
        }
        // A port the person cannot open is not theirs to give.
        for port in plan.serial {
            if let Err(why) = attach(port, true) {
                eprintln!("bundle: {port} left out: {why}");
            }
        }
        // The bundle's folder: the very one checked, read-only; through
        // the home, if that is attached and holds it, as nothing is
        // attached beneath what is.
        let in_home = plan.sealed.path.starts_with(&format!("{}/", plan.home));
        if !(plan.given.home && in_home) {
            let folder = sys::slot_of(plan.sealed.folder.0.as_raw_fd());
            capability::derive(&SYS, folder, s.scratch, READ_ONLY, 0)
                .map_err(failed(plan.sealed.path.clone()))?;
            let at = plan.sealed.path.trim_start_matches('/');
            let attached = directory::attach(&SYS, s.view, at.as_bytes(), s.scratch);
            let _ = capability::delete(&SYS, s.scratch);
            attached.map_err(failed(format!("attaching {}", plan.sealed.path)))?;
        }
        // In the home, if that is attached.
        if !plan.given.home {
            attach(plan.data, true)?;
        }
        // Its own /tmp, at /tmp.
        let tmp = plan.tmp.trim_start_matches('/');
        let flags = OpenFlags(OpenFlags::DIRECTORY.bits() | OpenFlags::NO_FOLLOW.bits());
        directory::open(&SYS, s.base, tmp.as_bytes(), flags, GIVEN, s.scratch, 0)
            .map_err(failed(format!("opening {}", plan.tmp)))?;
        let attached = directory::attach(&SYS, s.view, b"tmp", s.scratch);
        let _ = capability::delete(&SYS, s.scratch);
        attached.map_err(failed("attaching /tmp".into()))?;
        // Its keyring, which the runner serves, where authd is: to reach,
        // not to change.
        if let Some(folder) = plan.keyring.map(keyring::Served::folder) {
            let from = folder.trim_start_matches('/');
            directory::open(&SYS, s.base, from.as_bytes(), flags, READ_ONLY, s.scratch, 0)
                .map_err(failed(format!("opening {folder}")))?;
            let at = keyring::AT.as_bytes();
            let attached = directory::attach(&SYS, s.view, at, s.scratch);
            let _ = capability::delete(&SYS, s.scratch);
            attached.map_err(failed(format!("attaching {folder}")))?;
        }
        for (at, target) in links {
            // A name the view has already keeps what it has.
            let _ = directory::attach_link(&SYS, s.view, at.as_bytes(), target);
        }
        // Without control: nothing more is attached to it.
        capability::derive(&SYS, s.view, s.given, GIVEN, 0)
            .map_err(failed("narrowing its view".into()))?;

        process::create(&SYS, startup::SLOT_PROCESS, ObjectKind::Process, signin::SLOTS, s.child)
            .map_err(failed("making its process".into()))?;
        let mut shares = vec![
            (s.given, startup::SLOT_ROOT),
            (startup::SLOT_RANDOM, startup::SLOT_RANDOM),
            (startup::SLOT_LOG, startup::SLOT_LOG),
        ];
        for fd in 0..3 {
            let slot = sys::slot_of(fd);
            if capability::info(&SYS, slot).is_ok() {
                shares.push((slot, startup::SLOT_FD_BASE + fd as u32));
            }
        }
        for (from, to) in shares {
            process::share(&SYS, s.child, from, to, Rights::ALL, 0)
                .map_err(failed("giving it its capabilities".into()))?;
        }
        give_network(plan, s)?;
        let argv: Vec<&[u8]> = plan.argv.iter().map(Vec::as_slice).collect();
        let block =
            signin::exec_block_in(plan.data.as_bytes(), plan.path.as_bytes(), &argv, plan.env);
        let program = sys::slot_of(plan.program.as_raw_fd());
        process::exec(&SYS, s.child, program, &block).map_err(failed("starting it".into()))?;
        let number = process::info(&SYS, s.child).map_err(failed("its number".into()))?.number;
        Ok(number)
    }

    /// The caller's network capabilities, in their slots: each with its
    /// rights and the ports it covers, from `first` to before `past`.
    fn held() -> Vec<(u32, Rights, u32, u32)> {
        let end = startup::SLOT_NETWORK + startup::NETWORK_SLOTS;
        (startup::SLOT_NETWORK..end)
            .filter_map(|slot| {
                let info = capability::info(&SYS, slot).ok()?;
                if info.kind != ObjectKind::Network {
                    return None;
                }
                let ports = network::ports(&SYS, slot).ok()?;
                Some((slot, info.rights, ports.first, ports.first.saturating_add(ports.count)))
            })
            .collect()
    }

    /// Gives the program the network its manifest asks for, out of what
    /// the caller holds, in its slots from SLOT_NETWORK: to connect out,
    /// beyond this machine's networks (the send right) and to them (the
    /// local right, ADR 0337) as it asks, then each range of ports to
    /// listen on, in order. A required range the caller cannot listen on
    /// stops it; an optional one is left out.
    fn give_network(plan: &Plan<'_>, s: &Slots) -> Result<(), String> {
        let kept = Rights::COPY.union(Rights::TRANSFER);
        let held = held();
        let end = startup::SLOT_NETWORK + startup::NETWORK_SLOTS;
        let mut to = startup::SLOT_NETWORK;
        let mut give = |from: u32, first: u32, count: u32, right: Rights| -> Result<(), String> {
            if to >= end {
                return Err("it asks for more ranges of ports than a program holds".into());
            }
            network::restrict(&SYS, from, s.network, first, count, right.union(kept))
                .map_err(failed("its network".into()))?;
            let shared = process::share(&SYS, s.child, s.network, to, Rights::ALL, 0);
            let _ = capability::delete(&SYS, s.network);
            shared.map_err(failed("giving it its network".into()))?;
            to += 1;
            Ok(())
        };
        let reach = (if plan.given.outbound { Rights::SEND.bits() } else { 0 })
            | (if plan.given.local { Rights::LOCAL.bits() } else { 0 });
        for &(slot, rights, first, past) in &held {
            let has = rights.bits() & reach;
            if let Some(right) = Rights::from_bits(has).filter(|_| has != 0 && first < past) {
                give(slot, first, past - first, right)?;
            }
        }
        for &(ports, required) in &plan.given.listen {
            let (first, past) = (u32::from(ports.first), u32::from(ports.last) + 1);
            let covering = held.iter().find(|&&(_, rights, from, until)| {
                rights.bits() & Rights::RECEIVE.bits() != 0 && from <= first && past <= until
            });
            match covering {
                Some(&(slot, ..)) => give(slot, first, past - first, Rights::RECEIVE)?,
                None if required => {
                    return Err(format!(
                        "it listens on port {ports}, which you may not listen on: below 1024, \
                         only an administrator's commands do"
                    ));
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Waits for the program, and gives its exit status as a shell does:
    /// 128 and the signal's number for one a signal ended.
    fn wait(number: u64) -> i32 {
        // As system(3): the terminal's Ctrl+C is the program's to take.
        // SAFETY: setting dispositions touches no memory.
        let old = unsafe {
            (libc::signal(libc::SIGINT, libc::SIG_IGN), libc::signal(libc::SIGQUIT, libc::SIG_IGN))
        };
        let mut status = 0;
        let pid = i32::try_from(number).unwrap_or(-1);
        loop {
            // SAFETY: waitpid writes the status.
            let r = unsafe { libc::waitpid(pid, &raw mut status, 0) };
            if r >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break;
            }
        }
        // SAFETY: as above.
        unsafe {
            libc::signal(libc::SIGINT, old.0);
            libc::signal(libc::SIGQUIT, old.1);
        }
        if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            128 + libc::WTERMSIG(status)
        } else {
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::grants::Said;

    #[test]
    fn a_project_chooses_its_line_then_the_defaults_then_the_newest() {
        let root = std::env::temp_dir().join(format!("lines-{}", std::process::id()));
        let deep = root.join("project/src/parts");
        std::fs::create_dir_all(&deep).unwrap();
        let mut lines = Lines {
            id: "org.nodejs.node".into(),
            lines: vec!["20".into(), "22".into()],
            files: vec![".nvmrc".into(), ".node-version".into()],
            mine: None,
            everyone: None,
        };
        let picked = |lines: &Lines| pick(lines, &deep, "");
        // No file anywhere up: the newest, then everyone's default, then
        // the person's own.
        assert_eq!(picked(&lines), Ok(("22".into(), Why::Newest)));
        lines.everyone = Some("20".into());
        assert_eq!(picked(&lines), Ok(("20".into(), Why::Everyone)));
        lines.mine = Some("22".into());
        assert_eq!(picked(&lines), Ok(("22".into(), Why::Mine)));
        // A default whose line is not installed is passed over.
        lines.mine = Some("18".into());
        assert_eq!(picked(&lines), Ok(("20".into(), Why::Everyone)));
        // The nearest file, of the first name in a folder, chooses.
        let project = root.join("project");
        std::fs::write(project.join(".node-version"), "22.11.0\n").unwrap();
        let file = project.join(".node-version");
        assert_eq!(picked(&lines), Ok(("22".into(), Why::Project(file))));
        std::fs::write(project.join(".nvmrc"), "v20\n").unwrap();
        let file = project.join(".nvmrc");
        assert_eq!(picked(&lines), Ok(("20".into(), Why::Project(file))));
        std::fs::write(deep.join(".nvmrc"), "lts/iron\n").unwrap();
        assert_eq!(picked(&lines), Ok(("20".into(), Why::Everyone)));
        std::fs::write(deep.join(".nvmrc"), "18\n").unwrap();
        let why = picked(&lines).unwrap_err();
        assert!(why.starts_with("this project uses org.nodejs.node 18, as "), "{why}");
        assert!(why.contains("parts/.nvmrc says, which is not installed"), "{why}");
        // The line an add-on was added with comes first, even there.
        let pinned = |pins: &str| pick(&lines, &deep, pins);
        assert_eq!(pinned("org.example.app@1 org.nodejs.node@22"), Ok(("22".into(), Why::AddOn)));
        // One not installed, or of another ID, is passed over.
        assert!(pinned("org.nodejs.node@24").is_err());
        assert!(pinned("org.nodejs.nodejs@22 node@22").is_err());
        // A bundle whose lines name no files reads none.
        lines.files.clear();
        assert_eq!(picked(&lines), Ok(("20".into(), Why::Everyone)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn manifest(extra: &str, permissions: &str) -> Manifest {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example", "MIT")
            .replace("\"permissions\": {}", &format!("\"permissions\": {permissions}"))
            .replace("\"data\":", &format!("{extra}\"data\":"));
        manifest::bundle::from_json(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"))
    }

    #[test]
    fn the_command_chooses_the_program() {
        let m = manifest("", "{}");
        let chosen = choose(&m, None).unwrap();
        assert_eq!(
            chosen,
            Chosen {
                program: "notes".into(),
                path: "bin/notes".into(),
                runs_with: RunsWith::Sandbox,
                arg0: "notes".into(),
            }
        );
        assert_eq!(choose(&m, Some("notes")).unwrap(), chosen);
        assert_eq!(choose(&m, Some("ls")).unwrap_err(), "org.example.notes adds no command ls");
    }

    /// Asks nobody: a test that asks fails.
    fn nobody(name: &str) -> Result<String, String> {
        Err(format!("asked about {name}"))
    }

    fn grant(permission: &str, said: Said, words: &[&str]) -> Grant {
        let words: Vec<String> = words.iter().map(|w| (*w).to_string()).collect();
        Grant { permission: permission.into(), said, since: 1, given: words.clone(), asked: words }
    }

    #[test]
    fn what_install_gives_is_given() {
        let m = manifest("", "{}");
        assert_eq!(given(&m, "notes", &[], nobody).unwrap(), Given::default());
        let net = r#"{"network.outbound": {"need": "required"},
            "network.listen": {"need": "optional", "ports": [8080, "9000-9009"]},
            "notifications": {"need": "optional"}}"#;
        let g = given(&manifest("", net), "notes", &[], nobody).unwrap();
        assert!(g.outbound);
        let ports: Vec<String> = g.listen.iter().map(|(p, _)| p.to_string()).collect();
        assert_eq!(ports, ["8080", "9000-9009"]);
        assert!(g.listen.iter().all(|&(_, required)| !required));
        assert_eq!(g.names, ["network.outbound", "network.listen", "notifications"]);
        // Taken away, it is not.
        let off = [grant("network.outbound", Said::Never, &[])];
        let optional = net.replace("\"required\"", "\"optional\"");
        let g = given(&manifest("", &optional), "notes", &off, nobody).unwrap();
        assert!(!g.outbound);
        assert_eq!(g.without, ["network.outbound"]);
        let why = given(&manifest("", net), "notes", &off, nobody).unwrap_err();
        assert!(why.contains("bundle grant org.example.notes network.outbound"), "{why}");
    }

    #[test]
    fn what_is_asked_on_first_use_is_given_once_allowed() {
        let folder =
            r#"{"files.folder": {"need": "required", "folders": {"documents": "read-write"}}}"#;
        let m = manifest("", folder);
        // Not said yet: the person is asked, and their answer decides.
        let mut asked = Vec::new();
        let g = given(&m, "notes", &[], |name| {
            asked.push(name.to_string());
            Ok("once".into())
        })
        .unwrap();
        assert_eq!(asked, ["files.folder"]);
        assert_eq!(g.folders, [("documents".to_string(), true)]);
        let why = given(&m, "notes", &[], |_| Ok("never".into())).unwrap_err();
        assert!(why.contains("which you did not allow"), "{why}");
        let why = given(&m, "notes", &[], |_| Ok("elsewhere".into())).unwrap_err();
        assert!(why.contains("bundle grant org.example.notes files.folder"), "{why}");
        // Allowed for good, or given to read only: not asked again.
        let always = [grant("files.folder", Said::Allow, &["documents=read-write"])];
        assert_eq!(
            given(&m, "notes", &always, nobody).unwrap().folders,
            [("documents".to_string(), true)]
        );
        let mut read = grant("files.folder", Said::Allow, &["documents=read"]);
        read.asked = vec!["documents=read-write".into()];
        assert_eq!(
            given(&m, "notes", &[read], nobody).unwrap().folders,
            [("documents".to_string(), false)]
        );
        // Optional and not allowed: it runs without, and is not asked.
        let optional = folder.replace("required", "optional");
        let never = [grant("files.folder", Said::Never, &["documents=read-write"])];
        let g = given(&manifest("", &optional), "notes", &never, nobody).unwrap();
        assert_eq!(g.without, ["files.folder"]);
        assert!(g.folders.is_empty());
        // What only an administrator gives, or nothing yet, never runs.
        let debug = r#"{"processes.debug": {"need": "required"}}"#;
        let why = given(&manifest("", debug), "notes", &[], nobody).unwrap_err();
        assert!(why.contains("cannot give yet"), "{why}");
    }

    #[test]
    fn a_program_narrows_itself() {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example", "MIT")
            .replace(
                "\"path\": { \"x86_64\": \"bin/notes\" } }",
                "\"path\": { \"x86_64\": \"bin/notes\" }, \"permissions\": [\"network.listen\"] }",
            )
            .replace(
                "\"permissions\": {}",
                r#""permissions": {"network.outbound": {"need": "required"},
                    "network.listen": {"need": "required", "ports": [8080]}}"#,
            );
        let m = manifest::bundle::from_json(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        let g = given(&m, "notes", &[], nobody).unwrap();
        assert!(!g.outbound);
        assert_eq!(g.listen.len(), 1);
        assert_eq!(g.names, ["network.listen"]);
    }

    #[test]
    fn each_file_permission_says_what_it_gives() {
        let all = r#"{"files.home": {"need": "required"},
            "files.volumes": {"need": "required"},
            "files.shared": {"need": "required", "folders": {"team": "read"}},
            "files.app-data": {"need": "required", "apps": ["org.example.mail"]},
            "devices.serial": {"need": "required"},
            "keyring.shared": {"need": "required"}}"#;
        let g = given(&manifest("", all), "notes", &[], |_| Ok("always".into())).unwrap();
        assert!(g.home && g.volumes && g.serial && g.keyring_shared);
        assert_eq!(g.shared, [("team".to_string(), false)]);
        assert_eq!(g.app_data, ["org.example.mail"]);
    }

    #[test]
    fn the_environment_is_clean() {
        let user = accounts::User {
            name: "alice".into(),
            uid: 1000,
            gid: 1000,
            full_name: String::new(),
            home: "/users/alice".into(),
            shell: "/system/commands/nukesh".into(),
        };
        let caller: Vec<(String, String)> = [
            ("TERM", "linux"),
            ("LANG", "en_US.UTF-8"),
            ("LC_TIME", "C"),
            ("TZ", "Europe/Paris"),
            ("SECRET", "x"),
            ("LD_PRELOAD", "/tmp/x.so"),
            ("PATH", "/users/alice/bin"),
        ]
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
        let env = environment(&user, "/users/alice/app-data/org.example.notes", &caller);
        assert_eq!(
            env,
            [
                "HOME=/users/alice/app-data/org.example.notes",
                "USER=alice",
                "LOGNAME=alice",
                "PATH=/system/commands",
                "TERM=linux",
                "LANG=en_US.UTF-8",
                "LC_TIME=C",
                "TZ=Europe/Paris",
            ]
        );
    }
}
