// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `linux run`: runs a Linux x86-64 program in a sandbox of its own, with
//! linuxd beside it to answer what the kernel's Linux table does not (ADRs
//! 0621 and 0622, linux(1)).
//!
//! The command reads the program's ELF header ([`elf`]), finds the Linux
//! base bundle for a program that needs its libraries, makes a private
//! folder for the program's `/tmp` and `/etc` ([`private`], [`etc`]),
//! plans what the program's view holds ([`view`]) and asks the person
//! about the files its arguments name that the view does not hold. The
//! sandbox ([`sandbox`]) then carries the plan out, starts linuxd and the
//! program, and waits for it, starting linuxd again if it ends
//! ([`supervise`]).
//!
//! When a run fails before the program starts, the line `linux` prints
//! names what it could not do; the files it wrote for the program are in
//! `/tmp/.linux-PID` until it ends.

pub(crate) mod elf;
mod environment;
mod etc;
mod private;
mod sandbox;
mod supervise;
mod view;

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use sysapi::fd::{self, Standard};
use sysapi::terminal;

pub(crate) use self::sandbox::Follow;
use self::view::{Base, Entry, Inputs, Terminal, View};
use crate::args::{Arg, Args};
use crate::bundled;
use crate::bundles::folder::Dir;
use crate::bundles::protocol::Find;
use crate::bundles::run::{self, Given, Sealed, Target};
use crate::util::{self, Fail, fail_with, strip_os_error};

const USAGE: &str =
    "Usage: linux run [--allow PATH]... [--allow-write PATH]... [--home] [--allow-jit] [--]
                 PROGRAM [ARGUMENT...]
       linux help";

/// The Linux base bundle, which holds the libraries a dynamic program
/// loads, and which a Linux bundle's programs run on (ADR 0625).
pub(crate) const BASE_ID: &str = "org.dukovo.linux-base";
/// What a person types to have it.
const INSTALL: &str = "bundle install org.dukovo.linux-base";

/// The variable a program in a Linux view finds set: Nukesh there runs a
/// Linux program as it is, in the view it is in (ADR 0626).
const IN_VIEW: &str = "DUKOVO_LINUX_VIEW";

/// The exit status when `linux` itself could not run the program, as
/// env(1) gives; the program's own status is any other.
const NOT_RUN: i32 = 125;
/// The exit status for a file that cannot be run, as a shell gives it.
const CANNOT_RUN: i32 = 126;
/// The exit status for a file that is not there, as a shell gives it.
const NOT_FOUND: i32 = 127;

/// The sound of a refusal: `linux` could not run the program.
fn refused(message: String) -> Fail {
    fail_with(NOT_RUN, message)
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Run(Options),
}

/// The arguments of `linux run`.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    /// `--allow`, as typed.
    allow: Vec<String>,
    /// `--allow-write`, as typed.
    allow_write: Vec<String>,
    /// `--home`.
    home: bool,
    /// `--allow-jit`: memory that is writable and executable (ADR 0624).
    allow_jit: bool,
    program: OsString,
    args: Vec<OsString>,
}

/// `linux`'s command: `linux run` and `linux help`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    match parse(args)? {
        Command::Help => {
            println!("{USAGE}");
            Ok(0)
        }
        Command::Run(options) => run(&options),
    }
}

fn parse(args: &mut Args) -> Result<Command, Fail> {
    let word = args.subcommand().map(|word| word.to_string_lossy().into_owned());
    match word.as_deref() {
        Some("run") => {}
        Some("help") => return Ok(Command::Help),
        Some(other) => return Err(refused(format!("{other} is no command of linux\n{USAGE}"))),
        None => {
            return match args.next()? {
                Some(Arg::Long(long)) if long == "help" => Ok(Command::Help),
                Some(other) => Err(mistake(args, &other)),
                None => Err(refused(USAGE.into())),
            };
        }
    }
    args.stop_at_operand();
    let mut options = Options {
        allow: Vec::new(),
        allow_write: Vec::new(),
        home: false,
        allow_jit: false,
        program: OsString::new(),
        args: Vec::new(),
    };
    loop {
        match args.next()? {
            Some(Arg::Long(long)) if long == "allow" => options.allow.push(args.text()?),
            Some(Arg::Long(long)) if long == "allow-write" => {
                options.allow_write.push(args.text()?);
            }
            Some(Arg::Long(long)) if long == "home" => options.home = true,
            Some(Arg::Long(long)) if long == "allow-jit" => options.allow_jit = true,
            Some(Arg::Long(long)) if long == "help" => return Ok(Command::Help),
            Some(Arg::Operand(program)) => {
                options.program = program;
                options.args = args.rest();
                return Ok(Command::Run(options));
            }
            Some(other) => return Err(mistake(args, &other)),
            None => return Err(refused(USAGE.into())),
        }
    }
}

/// An option `linux` has not, as a refusal.
fn mistake(args: &Args, arg: &Arg) -> Fail {
    let mut failure = args.unknown(arg);
    failure.code = NOT_RUN;
    failure
}

/// The program `linux run` was asked to run.
struct Program {
    /// Its name, which its data folder and the questions use.
    name: String,
    /// Its path, with no link in it.
    real: String,
    /// The file, open to run.
    file: OwnedFd,
    linking: elf::Linking,
}

fn run(options: &Options) -> Result<i32, Fail> {
    let started_in = here()?;
    let given = absolute(&options.program.to_string_lossy(), &started_in);
    let program = open_program(&given)?;
    let base = match program.linking {
        elf::Linking::Static => None,
        elf::Linking::Dynamic => Some(open_base(&program.name)?),
    };
    let uid = util::own_user();
    let user = bundled::account(uid).map_err(refused)?;
    let home = user.home.trim_end_matches('/').to_string();
    let data =
        private::data_folder(&home, uid, &private::data_name(&program.name)).map_err(refused)?;
    let jit = options.allow_jit.then_some(true);
    launch(Launch {
        arg0: options.program.clone(),
        args: &options.args,
        base,
        bundle: None,
        data: text(&data)?,
        your_home: false,
        started_in: Some(started_in),
        allow: resolve_all(&options.allow, "--allow")?,
        allow_write: resolve_all(&options.allow_write, "--allow-write")?,
        whole_home: options.home,
        given: Given { outbound: true, local: true, jit, ..Given::default() },
        path_first: Vec::new(),
        more_env: Vec::new(),
        ask: true,
        jit_hint: "run it with linux run --allow-jit",
        follow: None,
        program,
        user,
    })
}

/// The folder this was started in, by its real path.
fn here() -> Result<String, Fail> {
    let here = fs::canonicalize(".").map_err(|e| refused(format!(".: {}", strip_os_error(&e))))?;
    text(&here)
}

/// A Linux program to start in a view of its own, and what it is given:
/// what `linux run` and a Linux bundle's run both start.
struct Launch<'a> {
    program: Program,
    /// The name it is run by, and its arguments.
    arg0: OsString,
    args: &'a [OsString],
    /// The Linux base, for a program that needs its libraries.
    base: Option<Sealed>,
    /// The bundle it is of, checked.
    bundle: Option<&'a Sealed>,
    user: accounts::User,
    /// Its data folder, made.
    data: String,
    /// Whether its `HOME` is the person's home, as for a command that runs
    /// with their access; otherwise its data folder.
    your_home: bool,
    /// The folder it starts in, if it is given it.
    started_in: Option<String>,
    /// What it may read and change besides, by real paths.
    allow: Vec<String>,
    allow_write: Vec<String>,
    whole_home: bool,
    given: Given,
    /// Folders its `PATH` names before Linux's: its bundle's commands'.
    path_first: Vec<String>,
    /// More of its environment: its add-ons' variables.
    more_env: Vec<String>,
    /// Whether the person is asked about the files its arguments name
    /// that its view does not hold, when they are at a terminal.
    ask: bool,
    /// What the person is told to do when it was refused memory that is
    /// writable and executable.
    jit_hint: &'a str,
    follow: Option<sandbox::Follow>,
}

/// Plans the view of `run`, makes its private folder and starts it.
fn launch(run: Launch<'_>) -> Result<i32, Fail> {
    let layout = match &run.base {
        Some(sealed) => Some(base_layout(&sealed.folder, &sealed.path).map_err(refused)?),
        None => None,
    };
    let uid = run.user.uid;
    let home = run.user.home.trim_end_matches('/').to_string();
    let home_env = if run.your_home { home.clone() } else { run.data.clone() };
    let private = private::Private::make(private::TMP, uid, std::process::id()).map_err(refused)?;
    let etc = write_etc(&private, &run.user, &home_env, &home)?;
    let network = network(&run.given);

    let facts = Facts {
        program: run.program.real.clone(),
        started_in: run.started_in.clone(),
        bundle: run.bundle.map(|sealed| sealed.path.clone()),
        home: home.clone(),
        data: run.data.clone(),
        private: private.path().to_string(),
        base: layout,
        zoneinfo: Path::new(etc::DUKOVO_ZONEINFO).is_dir(),
        commands: shell_commands(),
        zone: zone(),
        devices: devices(),
        terminal: terminal_of(first_terminal().as_deref()),
        etc,
        certificates: certificates(),
        network,
        allow: run.allow,
        allow_write: run.allow_write,
        whole_home: run.whole_home,
    };
    let mut view = facts.plan(&[]).map_err(refused)?;
    if run.ask && util::is_terminal(Standard::Input.fd()) {
        let asked = ask_about(run.args, &run.program.name, &view);
        if !asked.is_empty() {
            view = facts.plan(&asked).map_err(refused)?;
        }
    }
    for note in &view.notes {
        eprintln!("linux: {note}");
    }
    private.write("mounts", &view::mounts_text(&view.mounts), 0o600).map_err(refused)?;

    let mut argv: Vec<Vec<u8>> = vec![run.arg0.as_bytes().to_vec()];
    argv.extend(run.args.iter().map(|arg| arg.as_bytes().to_vec()));
    let mut env = environment::environment(&run.user, &home_env, &run::caller_environment());
    if !run.path_first.is_empty() {
        let path = format!("PATH={}:{}", run.path_first.join(":"), environment::PATH);
        env.retain(|entry| !entry.starts_with("PATH="));
        env.insert(0, path);
    }
    env.extend(run.more_env);
    // Nukesh in the view runs a Linux program as it is, in the same view.
    env.retain(|entry| !entry.starts_with(&format!("{IN_VIEW}=")));
    env.push(format!("{IN_VIEW}=1"));
    let daemon_env =
        [format!("LINUXD_MOUNTS={}", private.at("mounts")), "PATH=/system/commands".into()];
    let plan = sandbox::Plan {
        view: &view,
        base: run.base.as_ref(),
        program: &run.program.file,
        path: &run.program.real,
        argv: &argv,
        env: &env,
        daemon_env: &daemon_env,
        given: &run.given,
        bundle: run.bundle,
        jit_hint: run.jit_hint,
    };
    sandbox::run(&plan, run.follow).map_err(refused)
}

/// A Linux bundle's program, as `bundle run` found and checked it (ADR
/// 0625).
pub(crate) struct BundleRun<'a> {
    /// The bundle's full ID.
    pub id: &'a str,
    pub sealed: &'a Sealed,
    /// The program, open, and its path: in the bundle, or in its line's
    /// add-ons.
    pub program: OwnedFd,
    pub path: String,
    pub arg0: String,
    pub args: Vec<OsString>,
    /// What its permissions give it.
    pub given: Given,
    /// It runs with the person's access: all of their files, the folder
    /// it was started in, the network, and `HOME` their own.
    pub your_access: bool,
    /// Its data folder, made.
    pub data: String,
    /// Its line's add-ons folder, which it may change, and the variables
    /// that name it.
    pub add_ons: Option<(String, Vec<String>)>,
    /// bundled's connection, to stop it by.
    pub follow: Option<sandbox::Follow>,
}

/// Runs a Linux bundle's program in a Linux view, as `linux run` runs one,
/// with what its bundle is given: its exit status.
pub(crate) fn run_bundle(run: BundleRun<'_>) -> Result<i32, Fail> {
    let name = run.path.rsplit('/').next().unwrap_or(&run.path).to_string();
    let program =
        Program { name, real: run.path.clone(), file: run.program, linking: elf::Linking::Dynamic };
    let base = open_base(&format!("{} of {}", program.name, run.id))?;
    let uid = util::own_user();
    let user = bundled::account(uid).map_err(refused)?;
    let home = user.home.trim_end_matches('/').to_string();
    let given = run.given;
    let mut allow = Vec::new();
    let mut allow_write = Vec::new();
    let whole_home = run.your_access || given.home;
    if !whole_home {
        for (folder, write) in &given.folders {
            let path = format!("{home}/{folder}");
            if *write { allow_write.push(path) } else { allow.push(path) }
        }
        let own = run.sealed.manifest.full_id();
        for app in given.app_data.iter().filter(|app| **app != own) {
            let path = format!("{home}/{}/{app}", bundled::APP_DATA);
            if fs::symlink_metadata(&path).is_ok() {
                allow.push(path);
            }
        }
    }
    if given.volumes && fs::symlink_metadata(run::VOLUMES).is_ok() {
        allow_write.push(run::VOLUMES.into());
    }
    for (folder, write) in &given.shared {
        let path = format!("{}/{folder}", run::SHARED);
        if fs::symlink_metadata(&path).is_ok() {
            if *write { allow_write.push(path) } else { allow.push(path) }
        }
    }
    let mut more_env = Vec::new();
    let mut path_first = commands_folders(run.sealed);
    if let Some((folder, variables)) = &run.add_ons {
        allow_write.push(folder.clone());
        more_env.extend(variables.iter().map(|name| format!("{name}={folder}")));
        if let Some(commands) = &run.sealed.manifest.add_ons {
            path_first.push(format!("{folder}/{}", commands.commands));
        }
    }
    let given = if run.your_access {
        Given { outbound: true, local: true, jit: given.jit, ..Given::default() }
    } else {
        given
    };
    let hint = format!("{} does not ask for memory.jit", run.id);
    launch(Launch {
        program,
        arg0: run.arg0.into(),
        args: &run.args,
        base: Some(base),
        bundle: Some(run.sealed),
        user,
        data: run.data,
        your_home: run.your_access,
        started_in: if run.your_access { Some(here()?) } else { None },
        allow,
        allow_write,
        whole_home,
        given,
        path_first,
        more_env,
        ask: run.your_access,
        jit_hint: &hint,
        follow: run.follow,
    })
}

/// The folders of the bundle's commands' programs, in its folder, once
/// each, in the order of its commands: what a program of it that runs
/// another of its commands by name finds them by.
fn commands_folders(sealed: &Sealed) -> Vec<String> {
    let manifest = &sealed.manifest;
    let mut folders: Vec<String> = Vec::new();
    for command in &manifest.commands {
        let Some(program) = manifest.programs.get(command.program.0.as_str()) else { continue };
        let Some(path) = program.path.get(&run::ARCH) else { continue };
        let folder = match path.0.rsplit_once('/') {
            Some((folder, _)) => format!("{}/{folder}", sealed.path),
            None => sealed.path.clone(),
        };
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders
}

/// What the plan of the view is made from, besides what the person asks.
struct Facts {
    program: String,
    started_in: Option<String>,
    bundle: Option<String>,
    home: String,
    data: String,
    private: String,
    base: Option<Base>,
    zoneinfo: bool,
    commands: Vec<String>,
    zone: String,
    devices: Vec<String>,
    terminal: Terminal,
    etc: Vec<String>,
    certificates: Option<String>,
    network: Vec<String>,
    allow: Vec<String>,
    allow_write: Vec<String>,
    whole_home: bool,
}

impl Facts {
    /// The view, with the files `asked` after added to what was given.
    fn plan(&self, asked: &[String]) -> Result<View, String> {
        view::plan(&Inputs {
            program: &self.program,
            started_in: self.started_in.as_deref(),
            bundle: self.bundle.as_deref(),
            home: &self.home,
            data: &self.data,
            private: &self.private,
            base: self.base.as_ref(),
            zoneinfo: self.zoneinfo,
            commands: &self.commands,
            zone: &self.zone,
            devices: &self.devices,
            terminal: self.terminal,
            etc: &self.etc,
            certificates: self.certificates.as_deref(),
            network: &self.network,
            allow: &self.allow,
            allow_write: &self.allow_write,
            asked,
            whole_home: self.whole_home,
        })
    }
}

/// `path` as text, or why it cannot be.
fn text(path: &Path) -> Result<String, Fail> {
    path.to_str().map(String::from).ok_or_else(|| refused(format!("{}: not text", path.display())))
}

/// `path` made absolute against `here`, with `.`, `..` and repeated
/// slashes taken out as the text goes, without looking at the files.
fn absolute(path: &str, here: &str) -> String {
    let joined = if path.starts_with('/') { path.to_string() } else { format!("{here}/{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Opens the program at `given`, an absolute path, and reads how it is
/// linked.
fn open_program(given: &str) -> Result<Program, Fail> {
    let cannot = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => fail_with(NOT_FOUND, format!("{given}: no such file")),
        _ => fail_with(CANNOT_RUN, format!("{given}: {}", strip_os_error(&e))),
    };
    let real = fs::canonicalize(given).map_err(cannot)?;
    let mut file = File::open(&real).map_err(cannot)?;
    if !file.metadata().map_err(cannot)?.is_file() {
        return Err(fail_with(CANNOT_RUN, format!("{given}: it is not a file")));
    }
    let mut head = Vec::new();
    (&mut file).take(elf::HEAD_MAX as u64).read_to_end(&mut head).map_err(cannot)?;
    file.rewind().map_err(cannot)?;
    let linking =
        elf::inspect(&head).map_err(|why| fail_with(CANNOT_RUN, format!("{given}: {why}")))?;
    let name = given.rsplit('/').next().unwrap_or(given).to_string();
    Ok(Program { name, real: text(&real)?, file: file.into(), linking })
}

/// Whether bundled's refusal says the Linux base is not installed.
fn base_missing(why: &str) -> bool {
    why.starts_with(&format!("{BASE_ID} is not installed"))
}

/// The Linux base bundle, found as `bundle run` finds a bundle and
/// checked as it checks one.
fn open_base(program: &str) -> Result<Sealed, Fail> {
    let found = match run::locate(&Find::Id(BASE_ID.into())) {
        Ok(Target::Bundle(found, _)) => found,
        Ok(Target::AddOn(_)) => return Err(refused("bundled answered out of turn".into())),
        Err(why) if base_missing(&why.message) => {
            return Err(fail_with(
                CANNOT_RUN,
                format!("{program} needs the Linux base: {INSTALL}"),
            ));
        }
        Err(why) => return Err(refused(why.message)),
    };
    run::open_sealed(&found).map_err(|why| refused(format!("{BASE_ID}: {why}")))
}

/// What the base holds that the view plan needs to know: the entries of
/// its `usr`, and of `usr/share` if that is a folder. `path` is the
/// base's, for messages.
fn base_layout(folder: &Dir, path: &str) -> Result<Base, String> {
    let wrong = |what: &str, e: io::Error| format!("{path}/{what}: {}", strip_os_error(&e));
    let usr = folder.dir_at("usr").map_err(|e| wrong("usr", e))?;
    let entries = |dir: &Dir, what: &str| -> Result<Vec<Entry>, String> {
        let mut names = dir.names().map_err(|e| wrong(what, e))?;
        names.sort();
        let mut found = Vec::new();
        for name in names {
            let Some(st) = dir.stat_at(&name).map_err(|e| wrong(what, e))? else { continue };
            match st.st_mode & libc::S_IFMT {
                libc::S_IFDIR => found.push(Entry { name, folder: true }),
                libc::S_IFREG => found.push(Entry { name, folder: false }),
                _ => {}
            }
        }
        Ok(found)
    };
    let top = entries(&usr, "usr")?;
    let share = match top.iter().find(|entry| entry.name == "share" && entry.folder) {
        Some(_) => {
            let dir = usr.dir_at("share").map_err(|e| wrong("usr/share", e))?;
            Some(entries(&dir, "usr/share")?)
        }
        None => None,
    };
    Ok(Base { usr: top, share })
}

/// Writes the program's `/etc` files into its private folder, and says
/// which: its account's home is `own_home`, its `HOME`, and `home` is the
/// person's. A file whose source is not there is left out, and the program
/// finds none at its name.
fn write_etc(
    private: &private::Private,
    user: &accounts::User,
    own_home: &str,
    home: &str,
) -> Result<Vec<String>, Fail> {
    let mut written = Vec::new();
    let mut put = |name: &str, text: &str| -> Result<(), Fail> {
        private.write_etc(name, text).map_err(refused)?;
        written.push(name.to_string());
        Ok(())
    };
    let host = etc::host_name(&util::hostname());
    put("passwd", &etc::passwd(user, own_home))?;
    put("group", &etc::group(user, &util::group_name(user.gid)))?;
    put("hosts", &etc::hosts(&host))?;
    put("hostname", &format!("{host}\n"))?;
    put("nsswitch.conf", etc::NSSWITCH)?;
    if let Ok(text) = fs::read_to_string("/system/settings/resolv.conf") {
        put("resolv.conf", &text)?;
    }
    if let Ok(text) = fs::read_to_string("/system/libraries/os-release") {
        put("os-release", &text)?;
    }
    // A bundle's program keeps its data in the bundle's folder, so this
    // may be the first of the person's Linux programs to need app-data/linux.
    let store = private::linux_folder(home, user.uid).map_err(refused)?.join(".machine-id");
    let random = || {
        let mut bytes = [0u8; 16];
        File::open("/devices/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes)
    };
    put("machine-id", &private::machine_id(&store, random).map_err(refused)?)?;
    Ok(written)
}

/// The zone the system's `localtime` leads to.
fn zone() -> String {
    let target = fs::read_link("/system/settings/localtime").unwrap_or_default();
    etc::zone_of(&target.to_string_lossy()).to_string()
}

/// The programs of the view's shell that this system has, each a file.
fn shell_commands() -> Vec<String> {
    let there = |program: &str| {
        fs::symlink_metadata(format!("{}/{program}", view::COMMANDS)).is_ok_and(|m| m.is_file())
    };
    view::SHELL
        .iter()
        .map(|(program, _)| *program)
        .filter(|program| there(program))
        .map(String::from)
        .collect()
}

/// The names of the devices the view has that this system has.
fn devices() -> Vec<String> {
    let wanted =
        view::DEVICES.iter().map(|(name, _)| *name).chain(view::TERMINAL_DEVICES.iter().copied());
    wanted
        .filter(|name| fs::symlink_metadata(format!("{}/{name}", util::DEVICES)).is_ok())
        .map(String::from)
        .collect()
}

/// The folders of the network that this system has that `given` reaches,
/// as `bundle run` attaches them: the resolver's and netd's to connect out
/// or reach the local network, discoverd's to find the machines on it.
fn network(given: &Given) -> Vec<String> {
    let reaches = given.outbound || given.local;
    let mut folders: Vec<&str> = if reaches { run::NETWORK.to_vec() } else { Vec::new() };
    if given.local {
        folders.push(run::DISCOVERY);
    }
    folders
        .into_iter()
        .filter(|folder| fs::symlink_metadata(folder).is_ok())
        .map(String::from)
        .collect()
}

/// The root certificates' file the system trusts, by its real path: the
/// administrator's, or the system's own.
fn certificates() -> Option<String> {
    [
        "/system/settings/ssl/certs/ca-certificates.crt",
        "/system/resources/certificates/ca-certificates.crt",
    ]
    .iter()
    .find_map(|path| fs::canonicalize(path).ok())
    .and_then(|path| path.into_os_string().into_string().ok())
}

/// `paths`, each made absolute and without links, which must be there.
fn resolve_all(paths: &[String], option: &str) -> Result<Vec<String>, Fail> {
    paths
        .iter()
        .map(|path| {
            let real = fs::canonicalize(path)
                .map_err(|e| refused(format!("{option} {path}: {}", strip_os_error(&e))))?;
            text(&real)
        })
        .collect()
}

/// The path of the terminal one of the standard descriptors is, if any.
fn first_terminal() -> Option<String> {
    [Standard::Input, Standard::Output, Standard::Error]
        .into_iter()
        .map(Standard::fd)
        .filter(|fd| util::is_terminal(fd))
        .find_map(|fd| terminal::path(fd).ok())
        .and_then(|path| path.to_str().map(String::from))
}

/// What the terminal at `path` is: a pseudo-terminal, as every pane and
/// SSH session has, or any other.
fn terminal_of(path: Option<&str>) -> Terminal {
    match path {
        None => Terminal::Absent,
        Some(path) if util::terminal_name(path).starts_with("pts/") => Terminal::Pty,
        Some(_) => Terminal::Tty,
    }
}

/// Asks the person about each file the program's arguments name that
/// its view does not hold, and gives those they said yes to.
fn ask_about(args: &[OsString], name: &str, view: &View) -> Vec<String> {
    let args: Vec<String> = args.iter().filter_map(|arg| arg.to_str().map(String::from)).collect();
    let resolve = |arg: &str| -> Option<String> {
        let real = fs::canonicalize(arg).ok()?;
        real.into_os_string().into_string().ok()
    };
    view::to_ask(&args, resolve, view)
        .into_iter()
        .filter(|path| confirm(&view::question(name, path)))
        .collect()
}

/// Asks `question` on the terminal; whether the answer is yes. No answer
/// is no.
fn confirm(question: &str) -> bool {
    let mut err = io::stderr();
    let _ = err.write_all(question.as_bytes());
    let _ = err.flush();
    view::said_yes(&read_line())
}

/// A line of standard input, read a byte at a time so that none of what
/// follows it is taken from the program.
fn read_line() -> String {
    let mut line = Vec::new();
    loop {
        let mut byte = 0u8;
        let n = match fd::read(Standard::Input.fd(), std::slice::from_mut(&mut byte)) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            read => read.unwrap_or(0),
        };
        if n == 0 || byte == b'\n' || line.len() >= 64 {
            return String::from_utf8_lossy(&line).into_owned();
        }
        line.push(byte);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn parsed(items: &[&str]) -> Result<Command, Fail> {
        let mut args = Args::new("linux", items.iter().map(OsString::from).collect());
        parse(&mut args)
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn a_run_names_the_program_and_what_it_is_given() {
        let Ok(Command::Run(options)) = parsed(&[
            "run",
            "--allow",
            "/shared/photos",
            "--allow-write=/users/alice/out",
            "--allow",
            "docs",
            "--home",
            "--allow-jit",
            "--",
            "./hello",
            "-v",
            "--allow",
            "x",
        ]) else {
            panic!("not a run");
        };
        assert_eq!(options.allow, strings(&["/shared/photos", "docs"]));
        assert_eq!(options.allow_write, strings(&["/users/alice/out"]));
        assert!(options.home);
        assert!(options.allow_jit);
        assert_eq!(options.program, OsString::from("./hello"));
        // What follows the program is the program's.
        assert_eq!(options.args, ["-v", "--allow", "x"].map(OsString::from));
    }

    #[test]
    fn what_follows_the_program_is_its_own_without_a_double_dash() {
        let Ok(Command::Run(options)) = parsed(&["run", "hello", "--home", "-x"]) else {
            panic!("not a run");
        };
        assert!(!options.home && !options.allow_jit);
        assert_eq!(options.program, OsString::from("hello"));
        assert_eq!(options.args, ["--home", "-x"].map(OsString::from));
    }

    #[test]
    fn help_is_asked_for_by_name_or_option() {
        assert!(matches!(parsed(&["help"]), Ok(Command::Help)));
        assert!(matches!(parsed(&["--help"]), Ok(Command::Help)));
        assert!(matches!(parsed(&["run", "--help"]), Ok(Command::Help)));
    }

    #[test]
    fn a_command_line_that_names_no_program_is_refused() {
        for items in [
            &[][..],
            &["run"],
            &["run", "--home"],
            &["walk", "x"],
            &["--home"],
            &["run", "--nope", "x"],
        ] {
            let failure = parsed(items).unwrap_err();
            assert_eq!(failure.code, NOT_RUN, "{items:?}");
            assert!(!failure.message.is_empty(), "{items:?}");
        }
    }

    #[test]
    fn a_path_is_made_absolute_as_text() {
        let here = "/users/alice/work";
        for (given, absolute_path) in [
            ("hello", "/users/alice/work/hello"),
            ("./hello", "/users/alice/work/hello"),
            ("../tools/./hello", "/users/alice/tools/hello"),
            ("/usr//bin/../bin/hello", "/usr/bin/hello"),
            ("/..", "/"),
            ("a/b/../../..", "/users/alice"),
        ] {
            assert_eq!(absolute(given, here), absolute_path, "{given}");
        }
    }

    #[test]
    fn a_pseudo_terminal_is_told_from_other_terminals() {
        assert_eq!(terminal_of(None), Terminal::Absent);
        assert_eq!(terminal_of(Some("/devices/pts/3")), Terminal::Pty);
        assert_eq!(terminal_of(Some("/dev/pts/0")), Terminal::Pty);
        assert_eq!(terminal_of(Some("/devices/tty1")), Terminal::Tty);
        assert_eq!(terminal_of(Some("/devices/ttyS0")), Terminal::Tty);
    }

    #[test]
    fn bundled_says_when_the_base_is_not_installed() {
        assert!(base_missing("org.dukovo.linux-base is not installed"));
        assert!(base_missing("org.dukovo.linux-base is not installed for you"));
        assert!(!base_missing("cannot reach bundled at /system/live/bundled: no such file"));
        assert!(!base_missing("org.example.other is not installed"));
    }

    /// A folder of the test's own, which the test removes.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_base_layout_lists_folders_and_files_of_usr_and_its_share() {
        let root = std::env::temp_dir().join(format!("linux-run-base-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let scratch = Scratch(root.clone());
        for folder in ["usr/lib64", "usr/bin", "usr/share/zoneinfo", "usr/share/locale"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        fs::write(root.join("usr/share/COPYING"), "x").unwrap();
        fs::write(root.join("usr/README"), "x").unwrap();
        std::os::unix::fs::symlink("bin", root.join("usr/sbin")).unwrap();
        let dir = Dir::open(root.to_str().unwrap()).unwrap();
        let entry = |name: &str, folder: bool| Entry { name: name.into(), folder };
        let base = base_layout(&dir, "/base").unwrap();
        assert_eq!(
            base.usr,
            [
                entry("README", false),
                entry("bin", true),
                entry("lib64", true),
                entry("share", true)
            ]
        );
        assert_eq!(
            base.share,
            Some(vec![entry("COPYING", false), entry("locale", true), entry("zoneinfo", true)])
        );
        // No share folder, or a file by that name, gives none.
        fs::remove_dir_all(root.join("usr/share")).unwrap();
        assert_eq!(base_layout(&dir, "/base").unwrap().share, None);
        fs::write(root.join("usr/share"), "x").unwrap();
        assert_eq!(base_layout(&dir, "/base").unwrap().share, None);
        // A base with no usr is no base.
        fs::remove_dir_all(root.join("usr")).unwrap();
        let why = base_layout(&dir, "/base").unwrap_err();
        assert!(why.starts_with("/base/usr: "), "{why}");
        drop(scratch);
    }

    #[test]
    fn a_program_that_is_not_there_or_not_elf_is_refused_with_the_shells_status() {
        let missing = open_program("/no/such/program").err().unwrap();
        assert_eq!(
            (missing.code, missing.message.as_str()),
            (127, "/no/such/program: no such file")
        );
        let script = std::env::temp_dir().join(format!("linux-run-script-{}", std::process::id()));
        fs::write(&script, "#!/bin/sh\n").unwrap();
        let scratch = Scratch(script.clone());
        let refused = open_program(script.to_str().unwrap()).err().unwrap();
        assert_eq!(refused.code, 126);
        assert!(refused.message.ends_with(": it is not an ELF program"), "{}", refused.message);
        let folder = open_program("/").err().unwrap();
        assert_eq!((folder.code, folder.message.as_str()), (126, "/: it is not a file"));
        drop(scratch);
    }
}
