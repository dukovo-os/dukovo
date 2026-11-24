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

mod elf;
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

use self::view::{Base, Entry, Inputs, Terminal, View};
use crate::args::{Arg, Args};
use crate::bundled;
use crate::bundles::folder::Dir;
use crate::bundles::protocol::Find;
use crate::bundles::run::{self, Given, Sealed, Target};
use crate::util::{self, Fail, fail_with, strip_os_error};

const USAGE: &str =
    "Usage: linux run [--allow PATH]... [--allow-write PATH]... [--home] [--] PROGRAM [ARGUMENT...]
       linux help";

/// The Linux base bundle, which holds the libraries a dynamic program
/// loads.
const BASE_ID: &str = "org.dukovo.linux-base";
/// What a person types to have it.
const INSTALL: &str = "bundle install org.dukovo.linux-base";

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
    let here = fs::canonicalize(".").map_err(|e| refused(format!(".: {}", strip_os_error(&e))))?;
    let started_in = text(&here)?;
    let given = absolute(&options.program.to_string_lossy(), &started_in);
    let program = open_program(&given)?;
    let base = match program.linking {
        elf::Linking::Static => None,
        elf::Linking::Dynamic => Some(open_base(&program.name)?),
    };
    let layout = match &base {
        Some(sealed) => Some(base_layout(&sealed.folder, &sealed.path).map_err(refused)?),
        None => None,
    };

    let uid = util::own_user();
    let user = bundled::account(uid).map_err(refused)?;
    let home = user.home.trim_end_matches('/').to_string();
    let data =
        private::data_folder(&home, uid, &private::data_name(&program.name)).map_err(refused)?;
    let data = text(&data)?;
    let private = private::Private::make(private::TMP, uid, std::process::id()).map_err(refused)?;
    let etc = write_etc(&private, &user, &data, &home)?;

    let facts = Facts {
        program: program.real.clone(),
        started_in,
        home: home.clone(),
        data,
        private: private.path().to_string(),
        base: layout,
        zoneinfo: Path::new(etc::DUKOVO_ZONEINFO).is_dir(),
        zone: zone(),
        devices: devices(),
        terminal: terminal_of(first_terminal().as_deref()),
        etc,
        certificates: certificates(),
        network: network(),
        allow: resolve_all(&options.allow, "--allow")?,
        allow_write: resolve_all(&options.allow_write, "--allow-write")?,
        whole_home: options.home,
    };
    let mut view = facts.plan(&[]).map_err(refused)?;
    if util::is_terminal(0) {
        let asked = ask_about(&options.args, &program.name, &view);
        if !asked.is_empty() {
            view = facts.plan(&asked).map_err(refused)?;
        }
    }
    for note in &view.notes {
        eprintln!("linux: {note}");
    }
    private.write("mounts", &view::mounts_text(&view.mounts), 0o600).map_err(refused)?;

    let mut argv: Vec<Vec<u8>> = vec![options.program.as_bytes().to_vec()];
    argv.extend(options.args.iter().map(|arg| arg.as_bytes().to_vec()));
    let env = environment::environment(&user, &facts.data, &run::caller_environment());
    let given = Given { outbound: true, local: true, ..Given::default() };
    let daemon_env =
        [format!("LINUXD_MOUNTS={}", private.at("mounts")), "PATH=/system/commands".into()];
    let plan = sandbox::Plan {
        view: &view,
        base: base.as_ref(),
        program: &program.file,
        path: &program.real,
        argv: &argv,
        env: &env,
        daemon_env: &daemon_env,
        given: &given,
    };
    sandbox::run(&plan).map_err(refused)
}

/// What the plan of the view is made from, besides what the person asks.
struct Facts {
    program: String,
    started_in: String,
    home: String,
    data: String,
    private: String,
    base: Option<Base>,
    zoneinfo: bool,
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
            started_in: &self.started_in,
            home: &self.home,
            data: &self.data,
            private: &self.private,
            base: self.base.as_ref(),
            zoneinfo: self.zoneinfo,
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
/// which. A file whose source is not there is left out, and the program
/// finds none at its name.
fn write_etc(
    private: &private::Private,
    user: &accounts::User,
    data: &str,
    home: &str,
) -> Result<Vec<String>, Fail> {
    let mut written = Vec::new();
    let mut put = |name: &str, text: &str| -> Result<(), Fail> {
        private.write_etc(name, text).map_err(refused)?;
        written.push(name.to_string());
        Ok(())
    };
    let host = etc::host_name(&util::hostname());
    put("passwd", &etc::passwd(user, data))?;
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
    let store = Path::new(home).join(bundled::APP_DATA).join("linux").join(".machine-id");
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

/// The names of the devices the view has that this system has.
fn devices() -> Vec<String> {
    let wanted =
        view::DEVICES.iter().map(|(name, _)| *name).chain(view::TERMINAL_DEVICES.iter().copied());
    wanted
        .filter(|name| fs::symlink_metadata(format!("{}/{name}", util::DEVICES)).is_ok())
        .map(String::from)
        .collect()
}

/// The folders of the network that this system has: those a bundle that
/// connects out and reaches the local network gets.
fn network() -> Vec<String> {
    run::NETWORK
        .iter()
        .copied()
        .chain([run::DISCOVERY])
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
    (0..3).filter(|&fd| util::is_terminal(fd)).find_map(terminal_path)
}

/// The path of the terminal `fd` is.
fn terminal_path(fd: i32) -> Option<String> {
    let mut buffer = [0 as libc::c_char; 128];
    // SAFETY: the buffer is valid for its length.
    if unsafe { libc::ttyname_r(fd, buffer.as_mut_ptr(), buffer.len()) } != 0 {
        return None;
    }
    // SAFETY: ttyname_r wrote a C string in the buffer.
    let name = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
    name.to_str().ok().map(String::from)
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
        // SAFETY: reads one byte into `byte`, which is valid for it.
        let n = unsafe { libc::read(0, (&raw mut byte).cast(), 1) };
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 || byte == b'\n' || line.len() >= 64 {
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
        assert_eq!(options.program, OsString::from("./hello"));
        // What follows the program is the program's.
        assert_eq!(options.args, ["-v", "--allow", "x"].map(OsString::from));
    }

    #[test]
    fn what_follows_the_program_is_its_own_without_a_double_dash() {
        let Ok(Command::Run(options)) = parsed(&["run", "hello", "--home", "-x"]) else {
            panic!("not a run");
        };
        assert!(!options.home);
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
