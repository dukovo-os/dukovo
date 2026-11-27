// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the program's view holds (ADR 0622): a pure plan from the
//! run's facts to the places to attach, the links to make and the mounts
//! `/proc/mounts` shows. `sandbox.rs` carries the plan out; nothing here
//! touches the system, so every rule has a test.
//!
//! The view's fixed places are `/usr` and its links (the Linux base),
//! the shell and core commands in `/usr/bin` (Dukovo's own, ADR 0626),
//! `/proc` and `/sys` (linuxd's filesystem), `/dev`, `/etc`, `/tmp`, and
//! the network's folders. The person's own places are the folder the
//! program was started in, its data folder, its own folder, and what
//! `--allow`, `--allow-write`, `--home` and the questions at the terminal
//! add, each attached at its real path. A bundle's program (ADR 0625) has
//! its bundle's folder too, read-only, and the places its permissions
//! give; one in the bundle's sandbox starts in its data folder, and is
//! given no other. A person's place never goes under
//! a fixed one, and none goes under another attached place, since the
//! kernel refuses to attach beneath what is attached (radix-directory(2),
//! `attach`): the plan keeps the outermost.
//!
//! When a view lacks a place a program needs, `ls` it in a test program
//! first: the plan's order is the order of `sandbox.rs`'s attaches.

use super::etc::{DUKOVO_ZONEINFO, ZONEINFO};

/// The devices every program has, with whether it may write them.
pub const DEVICES: &[(&str, bool)] =
    &[("null", true), ("zero", false), ("full", false), ("random", false), ("urandom", false)];

/// The devices each open makes anew or opens for the opener alone: the
/// terminal of the opener, a new pseudo-terminal, and the terminals made.
/// Attached as files they would stand for the one that was open when the
/// view was made, so they are reached through their folder, as Spark's
/// views reach them (ADR 0473).
pub const TERMINAL_DEVICES: &[&str] = &["tty", "ptmx", "pts"];

/// The places that are the program's own in its view: no folder of the
/// person's is attached at or beneath them. `/system` is Dukovo's: the
/// programs of the view's shell are there, and the network's folders.
const OWN: &[&str] = &[
    "/usr", "/lib", "/lib64", "/bin", "/sbin", "/proc", "/sys", "/dev", "/devices", "/etc", "/tmp",
    "/system",
];

/// Dukovo's folder of commands, and the folder of the dynamic linker and
/// C library its programs load.
pub const COMMANDS: &str = "/system/commands";
pub const LIBRARIES: &str = "/system/libraries";

/// Dukovo's programs that are the view's shell and core commands (ADR
/// 0626), each with the names Linux keeps in `/usr/bin` that lead to it:
/// Nukesh, which is POSIX's shell by the name `sh` and bash's by `bash`,
/// and uutils' programs.
pub const SHELL: &[(&str, &str)] =
    &[("nukesh", "sh bash"), ("coreutils", CORE_UTILITIES), ("find", "find"), ("xargs", "xargs")];

/// The utilities of the one program uutils' coreutils is, as Dukovo
/// builds it (`ports/uutils-coreutils/build.sh`, which a test reads).
pub const CORE_UTILITIES: &str = "[ arch b2sum base32 base64 basename basenc cat cksum comm cp \
    csplit cut date dd dir dircolors dirname du echo env expand expr factor false fmt fold groups \
    head hostname id install join link ln logname ls md5sum mkdir mkfifo mknod mktemp more mv \
    nice nl nohup nproc numfmt od paste pathchk pr printenv printf ptx pwd readlink realpath rm \
    rmdir seq sha1sum sha224sum sha256sum sha384sum sha512sum shred shuf sleep sort split stat \
    stty sum sync tac tail tee test timeout touch tr true truncate tsort tty uname unexpand uniq \
    unlink vdir wc whoami yes";

/// What a place in the view leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file or folder of the person's own root, by its path.
    Person(String),
    /// A file or folder beneath the base bundle's sealed folder.
    Base { path: String, folder: bool },
    /// The folder `proc` or `sys` of linuxd's filesystem.
    Linuxd(&'static str),
    /// A memory filesystem of the run's own, as `/devices/shm` is, whose
    /// files programs map and share (radix-directory(2)'s
    /// `memory_filesystem`).
    Memory,
    /// A folder of the person's root with only `names` in it.
    Names { folder: String, names: Vec<String> },
    /// The bundle's folder, the very one that was checked: read-only.
    Bundle,
}

/// A place to attach: where it is in the view, what it leads to, and
/// whether the program may change it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attach {
    pub at: String,
    pub source: Source,
    pub write: bool,
}

/// A symbolic link of the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub at: String,
    pub target: String,
}

/// A line of the program's `/proc/mounts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub at: String,
    /// The filesystem's name, which stands for its device too.
    pub kind: &'static str,
    pub write: bool,
}

/// An entry of the base bundle's `usr` or `usr/share`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub folder: bool,
}

/// What the Linux base bundle holds, as far as the plan looks: the entries
/// of its `usr`, and of `usr/share` if that is a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub usr: Vec<Entry>,
    pub share: Option<Vec<Entry>>,
}

impl Base {
    /// Whether its `usr` has an entry called `name`.
    fn has(&self, name: &str) -> bool {
        self.usr.iter().any(|entry| entry.name == name)
    }

    fn has_zoneinfo(&self) -> bool {
        self.share.as_ref().is_some_and(|share| share.iter().any(|entry| entry.name == "zoneinfo"))
    }
}

/// What the program's terminal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    /// Its standard streams are no terminal.
    Absent,
    /// A terminal, which `/dev/tty` opens.
    Tty,
    /// A pseudo-terminal, as every pane and SSH session has: the program
    /// may make its own, so it gets `/dev/ptmx` and `/dev/pts`.
    Pty,
}

/// The facts a view is made from.
pub struct Inputs<'a> {
    /// The program's file, by its real path.
    pub program: &'a str,
    /// The folder `linux run` was started in, by its real path, when it is
    /// given: to `linux run`'s program, and to a bundle's command that
    /// runs with your access. A program in its bundle's sandbox starts in
    /// its data folder.
    pub started_in: Option<&'a str>,
    /// A bundle's folder, for a bundle's program.
    pub bundle: Option<&'a str>,
    /// The person's home.
    pub home: &'a str,
    /// The program's data folder.
    pub data: &'a str,
    /// The run's private folder, which holds the program's `/tmp` and
    /// the `/etc` files.
    pub private: &'a str,
    /// The base bundle, if the program is dynamic.
    pub base: Option<&'a Base>,
    /// Whether Dukovo has a time zone database to show.
    pub zoneinfo: bool,
    /// The programs of [`SHELL`] that [`COMMANDS`] has.
    pub commands: &'a [String],
    /// The zone `/etc/localtime` leads to, a path beneath it.
    pub zone: &'a str,
    /// The names of [`DEVICES`] and [`TERMINAL_DEVICES`] that
    /// `/devices` has.
    pub devices: &'a [String],
    pub terminal: Terminal,
    /// The names of the files of `/etc` that were written.
    pub etc: &'a [String],
    /// The root certificates' file.
    pub certificates: Option<&'a str>,
    /// The network's folders that are there, as `bundle run` attaches
    /// them for a bundle that connects out.
    pub network: &'a [String],
    /// What `--allow` and `--allow-write` name, and what the person said
    /// yes to at the terminal, by real paths.
    pub allow: &'a [String],
    pub allow_write: &'a [String],
    pub asked: &'a [String],
    /// `--home`: all of the person's files, in place of the data folder
    /// and the folder it was started in.
    pub whole_home: bool,
}

/// The plan.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct View {
    pub attaches: Vec<Attach>,
    pub links: Vec<Link>,
    pub mounts: Vec<Mount>,
    /// Where the program starts.
    pub working: String,
    /// What the person is told was left out, a line each.
    pub notes: Vec<String>,
}

impl View {
    fn attach(&mut self, at: &str, source: Source, write: bool) {
        self.attaches.push(Attach { at: at.into(), source, write });
    }

    fn link(&mut self, at: &str, target: &str) {
        self.links.push(Link { at: at.into(), target: target.into() });
    }

    fn mount(&mut self, at: &str, kind: &'static str, write: bool) {
        self.mounts.push(Mount { at: at.into(), kind, write });
    }

    /// Whether the program finds something at `path` in its view: a place
    /// that is its own, or one attached or linked at it or above it.
    pub fn holds(&self, path: &str) -> bool {
        reserved(path)
            || self.attaches.iter().any(|attach| within(path, &attach.at))
            || self.links.iter().any(|link| within(path, &link.at))
    }
}

/// Whether `path` is `outer` or beneath it.
pub fn within(path: &str, outer: &str) -> bool {
    path == outer
        || path
            .strip_prefix(outer)
            .is_some_and(|rest| outer.ends_with('/') || rest.starts_with('/'))
}

/// Whether `path` is the root or one of the program's own places.
pub fn reserved(path: &str) -> bool {
    path == "/" || OWN.iter().any(|own| within(path, own))
}

/// The view of a run, or why the person's wishes cannot all be had.
///
/// # Errors
///
/// A path given with `--allow` or `--allow-write` that is one of the
/// program's own places, or that is read-only where another path asks
/// to change something beneath it.
pub fn plan(inputs: &Inputs<'_>) -> Result<View, String> {
    let mut view = View::default();
    usr(inputs, &mut view);
    shell(inputs, &mut view);
    view.attach("/proc", Source::Linuxd("proc"), true);
    view.mount("/proc", "proc", true);
    view.attach("/sys", Source::Linuxd("sys"), true);
    view.mount("/sys", "sysfs", true);
    view.attach("/tmp", Source::Person(format!("{}/tmp", inputs.private)), true);
    view.mount("/tmp", "tmpfs", true);
    dev(inputs, &mut view);
    etc(inputs, &mut view);
    for folder in inputs.network {
        view.attach(folder, Source::Person(folder.clone()), false);
    }
    folders(inputs, &mut view)?;
    Ok(view)
}

/// `/usr` from the base, and the links that lead to it; or Dukovo's time
/// zone database alone, for a program with no base.
fn usr(inputs: &Inputs<'_>, view: &mut View) {
    let Some(base) = inputs.base else {
        zoneinfo(inputs, view);
        return;
    };
    view.mount("/usr", "linux-base", false);
    // Dukovo's zoneinfo goes beneath `/usr/share`, and the shell's links
    // beneath `/usr/bin`, which the kernel refuses beneath one attached
    // whole: then each entry is attached.
    let shell_beneath = has_shell(inputs) && !base.has("bin");
    if base.has_zoneinfo() && !shell_beneath {
        view.attach("/usr", Source::Base { path: "usr".into(), folder: true }, false);
    } else {
        for entry in base.usr.iter().filter(|entry| entry.name != "share") {
            let path = format!("usr/{}", entry.name);
            view.attach(&format!("/{path}"), Source::Base { path, folder: entry.folder }, false);
        }
        for entry in base.share.iter().flatten() {
            let path = format!("usr/share/{}", entry.name);
            view.attach(&format!("/{path}"), Source::Base { path, folder: entry.folder }, false);
        }
        if !base.has_zoneinfo() {
            zoneinfo(inputs, view);
        }
    }
    for name in ["lib", "lib64", "bin", "sbin"] {
        if base.has(name) {
            view.link(&format!("/{name}"), &format!("usr/{name}"));
        }
    }
}

/// Whether the view gets the shell's links in `/usr/bin`: some program of
/// [`SHELL`] is there, and the base has no `usr/bin` of its own.
fn has_shell(inputs: &Inputs<'_>) -> bool {
    !inputs.commands.is_empty() && !inputs.base.is_some_and(|base| base.has("bin"))
}

/// The view's shell and core commands (ADR 0626): the programs of
/// [`SHELL`] that are there, read-only at their own paths with the
/// libraries they load, and the links in `/usr/bin` that lead to them,
/// which `/bin` leads to, as on Linux. A base with a `usr/bin` of its own
/// gives its own commands, and nothing is added to them.
fn shell(inputs: &Inputs<'_>, view: &mut View) {
    if !has_shell(inputs) {
        return;
    }
    view.attach(LIBRARIES, Source::Person(LIBRARIES.into()), false);
    let there = |program: &str| inputs.commands.iter().any(|command| command == program);
    for &(program, names) in SHELL.iter().filter(|(program, _)| there(program)) {
        let path = format!("{COMMANDS}/{program}");
        view.attach(&path, Source::Person(path.clone()), false);
        for name in names.split_whitespace() {
            view.link(&format!("/usr/bin/{name}"), &path);
        }
    }
    view.link("/bin", "usr/bin");
}

/// Dukovo's time zone database at `/usr/share/zoneinfo`.
fn zoneinfo(inputs: &Inputs<'_>, view: &mut View) {
    if inputs.zoneinfo {
        view.attach(ZONEINFO, Source::Person(DUKOVO_ZONEINFO.into()), false);
    }
}

/// `/dev`: the devices every program has, `/dev/shm`, the terminals, and
/// the links Linux's `udev` and `procfs` make. `/dev/shm` is a memory
/// filesystem of the run's own, as a container's is a tmpfs of its own:
/// a folder of the person's `/tmp` would not do, as the kernel shares the
/// pages of a memory filesystem's files alone, and `shm_open`, `sem_open`
/// and LTP's library map theirs shared and writable.
fn dev(inputs: &Inputs<'_>, view: &mut View) {
    let there = |name: &str| inputs.devices.iter().any(|device| device == name);
    for &(name, write) in DEVICES.iter().filter(|(name, _)| there(name)) {
        view.attach(&format!("/dev/{name}"), Source::Person(format!("/devices/{name}")), write);
    }
    view.attach("/dev/shm", Source::Memory, true);
    view.mount("/dev/shm", "tmpfs", true);
    let wanted: &[&str] = match inputs.terminal {
        Terminal::Absent => &[],
        Terminal::Tty => &TERMINAL_DEVICES[..1],
        Terminal::Pty => TERMINAL_DEVICES,
    };
    let names: Vec<String> =
        wanted.iter().filter(|name| there(name)).map(|n| (*n).into()).collect();
    if !names.is_empty() {
        let folder = "/devices".to_string();
        for name in &names {
            view.link(&format!("/dev/{name}"), &format!("/devices/{name}"));
        }
        view.attach("/devices", Source::Names { folder, names }, true);
    }
    view.link("/dev/fd", "/proc/self/fd");
    view.link("/dev/stdin", "/proc/self/fd/0");
    view.link("/dev/stdout", "/proc/self/fd/1");
    view.link("/dev/stderr", "/proc/self/fd/2");
}

/// `/etc`: each file written, one by one, the certificates, and the time
/// zone.
fn etc(inputs: &Inputs<'_>, view: &mut View) {
    for name in inputs.etc {
        let source = Source::Person(format!("{}/etc/{name}", inputs.private));
        view.attach(&format!("/etc/{name}"), source, false);
    }
    if let Some(certificates) = inputs.certificates {
        view.attach(
            "/etc/ssl/certs/ca-certificates.crt",
            Source::Person(certificates.into()),
            false,
        );
    }
    let shown = inputs.zoneinfo || inputs.base.is_some_and(Base::has_zoneinfo);
    if shown {
        view.link("/etc/localtime", &format!("{ZONEINFO}/{}", inputs.zone));
    }
}

/// A folder to attach at its own path.
struct Wanted {
    path: String,
    write: bool,
    /// Said yes to at the terminal, not named on the command line.
    asked: bool,
    /// The bundle's folder.
    bundle: bool,
}

/// The person's folders: the program's own data, the folder it started
/// in, the one its file is in, its bundle's, and what was given or asked.
/// Where one is inside another, the outer one is attached.
fn folders(inputs: &Inputs<'_>, view: &mut View) -> Result<(), String> {
    let mut wanted = Vec::new();
    for (paths, write, asked) in [
        (inputs.allow, false, false),
        (inputs.allow_write, true, false),
        (inputs.asked, false, true),
    ] {
        for path in paths {
            if reserved(path) {
                return Err(format!(
                    "{path} is one of the program's own places: /usr, /etc, /tmp, /dev, /proc, \
                     /sys and /system are not yours to give"
                ));
            }
            wanted.push(Wanted { path: path.clone(), write, asked, bundle: false });
        }
    }
    let own = if inputs.whole_home { inputs.home } else { inputs.data };
    wanted.push(Wanted { path: own.into(), write: true, asked: false, bundle: false });
    view.working = inputs.data.into();
    if let Some(started) = inputs.started_in {
        if let Some(why) = leaves_out(inputs, started) {
            view.notes.push(format!(
                "{started} is not given to the program ({why}); it starts in its data folder"
            ));
        } else {
            wanted.push(Wanted { path: started.into(), write: true, asked: false, bundle: false });
            view.working = started.into();
        }
    }
    if let Some(bundle) = inputs.bundle {
        wanted.push(Wanted { path: bundle.into(), write: false, asked: false, bundle: true });
    }
    if let Some((folder, _)) = inputs.program.rsplit_once('/')
        && !folder.is_empty()
        && !reserved(folder)
    {
        wanted.push(Wanted { path: folder.into(), write: false, asked: false, bundle: false });
    }
    // A folder the person said yes to for reading is not given if a place
    // the program may change is inside it: attached whole, it would take
    // that place's right to change it away.
    let changeable: Vec<String> =
        wanted.iter().filter(|w| w.write).map(|w| w.path.clone()).collect();
    wanted.retain(|w| {
        let covers = w.asked && changeable.iter().any(|inner| within(inner, &w.path));
        if covers {
            view.notes.push(format!(
                "{} is not given to the program: a place it may change is inside it",
                w.path
            ));
        }
        !covers
    });
    // Outer folders first, and those that may be changed before those that
    // may not, so that a second place inside one that is attached is
    // passed over, or refused when it asks for more.
    let depth = |path: &str| path.matches('/').count();
    wanted.sort_by(|a, b| depth(&a.path).cmp(&depth(&b.path)).then(b.write.cmp(&a.write)));
    let mut kept: Vec<Wanted> = Vec::new();
    for next in wanted {
        match kept.iter().find(|outer| within(&next.path, &outer.path)) {
            Some(outer) if outer.write || !next.write => {}
            Some(outer) => {
                return Err(format!(
                    "{} is inside {}, which is given read-only, and cannot be changed there: \
                     give {} with --allow-write instead",
                    next.path, outer.path, outer.path
                ));
            }
            None => kept.push(next),
        }
    }
    for folder in kept {
        view.mount(&folder.path, "dukovo", folder.write);
        let source =
            if folder.bundle { Source::Bundle } else { Source::Person(folder.path.clone()) };
        view.attach(&folder.path, source, folder.write);
    }
    Ok(())
}

/// Why the folder the program was started in is not given: it is one of
/// the program's own places, or it holds the person's home, which
/// `--home` is for.
fn leaves_out(inputs: &Inputs<'_>, started: &str) -> Option<&'static str> {
    if reserved(started) {
        return Some("that place is the program's own");
    }
    let holds_home = within(inputs.data, started);
    let covered = inputs.whole_home && within(started, inputs.home);
    (holds_home && !covered).then_some("it holds your home folder, which --home gives")
}

/// The arguments of the program that name a file or folder the view does
/// not hold, by real path, the outermost of them, once each: `resolve`
/// gives the real path of what exists. Those are what the person is asked
/// about.
pub fn to_ask(
    args: &[String],
    resolve: impl Fn(&str) -> Option<String>,
    view: &View,
) -> Vec<String> {
    let outside: Vec<String> =
        args.iter().filter_map(|arg| resolve(arg)).filter(|path| !view.holds(path)).collect();
    let mut asked: Vec<String> = Vec::new();
    for path in &outside {
        let covered = outside.iter().any(|other| other != path && within(path, other));
        if !covered && !asked.contains(path) {
            asked.push(path.clone());
        }
    }
    asked
}

/// What the person is asked before `path` is given to the program
/// called `name`.
pub fn question(name: &str, path: &str) -> String {
    format!("Let {name} read {path}? [y/N] ")
}

/// Whether the person's `line` says yes: `y` or `yes`, in any case.
pub fn said_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The program's `/proc/mounts`, which linuxd reads from `LINUXD_MOUNTS`.
pub fn mounts_text(mounts: &[Mount]) -> String {
    let mut text = String::new();
    for mount in mounts {
        let mode = if mount.write { "rw" } else { "ro" };
        let extra = if matches!(mount.kind, "proc" | "sysfs") { ",noexec" } else { "" };
        let at = mount.at.replace('\\', "\\134").replace(' ', "\\040").replace('\t', "\\011");
        let at = at.replace('\n', "\\012");
        text += &format!("{kind} {at} {kind} {mode},nosuid,nodev{extra} 0 0\n", kind = mount.kind);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The files of `/etc` that `linux run` writes.
    const ETC_FILES: &[&str] = &[
        "passwd",
        "group",
        "hosts",
        "hostname",
        "resolv.conf",
        "nsswitch.conf",
        "os-release",
        "machine-id",
    ];

    fn entries(names: &[&str]) -> Vec<Entry> {
        names.iter().map(|name| Entry { name: (*name).into(), folder: true }).collect()
    }

    fn base(zoneinfo: bool) -> Base {
        let share = if zoneinfo { entries(&["zoneinfo", "locale"]) } else { entries(&["locale"]) };
        Base { usr: entries(&["bin", "lib", "lib64", "share"]), share: Some(share) }
    }

    /// The facts of a run of `/users/alice/tools/hello` from
    /// `/users/alice/work`, with everything there.
    struct Run {
        base: Option<Base>,
        terminal: Terminal,
        started_in: Option<String>,
        bundle: Option<String>,
        program: String,
        allow: Vec<String>,
        allow_write: Vec<String>,
        asked: Vec<String>,
        whole_home: bool,
        devices: Vec<String>,
        etc: Vec<String>,
        commands: Vec<String>,
    }

    /// Every program of the view's shell.
    fn all_commands() -> Vec<String> {
        SHELL.iter().map(|(program, _)| (*program).to_string()).collect()
    }

    fn run() -> Run {
        let strings = |names: &[&str]| names.iter().map(|n| (*n).to_string()).collect();
        Run {
            base: Some(base(true)),
            terminal: Terminal::Absent,
            started_in: Some("/users/alice/work".into()),
            bundle: None,
            program: "/users/alice/tools/hello".into(),
            allow: Vec::new(),
            allow_write: Vec::new(),
            asked: Vec::new(),
            whole_home: false,
            devices: strings(&["null", "zero", "random", "urandom", "tty", "ptmx", "pts"]),
            etc: ETC_FILES.iter().map(|name| (*name).to_string()).collect(),
            commands: Vec::new(),
        }
    }

    impl Run {
        fn plan(&self) -> Result<View, String> {
            let network = ["/system/live/resolved".to_string()];
            plan(&Inputs {
                program: &self.program,
                started_in: self.started_in.as_deref(),
                bundle: self.bundle.as_deref(),
                home: "/users/alice",
                data: "/users/alice/app-data/linux/hello",
                private: "/tmp/.linux-7",
                base: self.base.as_ref(),
                zoneinfo: true,
                commands: &self.commands,
                zone: "Europe/Berlin",
                devices: &self.devices,
                terminal: self.terminal,
                etc: &self.etc,
                certificates: Some("/system/resources/certificates/ca-certificates.crt"),
                network: &network,
                allow: &self.allow,
                allow_write: &self.allow_write,
                asked: &self.asked,
                whole_home: self.whole_home,
            })
        }
    }

    fn at<'a>(view: &'a View, place: &str) -> Option<&'a Attach> {
        view.attaches.iter().find(|attach| attach.at == place)
    }

    fn person(path: &str, write: bool) -> Option<(Source, bool)> {
        Some((Source::Person(path.into()), write))
    }

    fn what(view: &View, place: &str) -> Option<(Source, bool)> {
        at(view, place).map(|attach| (attach.source.clone(), attach.write))
    }

    #[test]
    fn the_base_gives_usr_and_the_links_that_lead_to_it() {
        let view = run().plan().unwrap();
        assert_eq!(
            what(&view, "/usr"),
            Some((Source::Base { path: "usr".into(), folder: true }, false))
        );
        let links: Vec<(&str, &str)> =
            view.links.iter().map(|l| (l.at.as_str(), l.target.as_str())).collect();
        assert!(links.contains(&("/lib", "usr/lib")), "{links:?}");
        assert!(links.contains(&("/lib64", "usr/lib64")), "{links:?}");
        assert!(links.contains(&("/bin", "usr/bin")), "{links:?}");
        // The base has no sbin: no link to nothing.
        assert!(!links.iter().any(|(at, _)| *at == "/sbin"), "{links:?}");
        // Its zone database is used as it is.
        assert_eq!(at(&view, "/usr/share/zoneinfo"), None);
        assert_eq!(
            view.links.iter().find(|l| l.at == "/etc/localtime").map(|l| l.target.as_str()),
            Some("/usr/share/zoneinfo/Europe/Berlin")
        );
    }

    #[test]
    fn a_base_without_a_zone_database_is_attached_by_its_entries() {
        let mut facts = run();
        facts.base = Some(base(false));
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/usr"), None);
        for place in ["/usr/bin", "/usr/lib", "/usr/lib64", "/usr/share/locale"] {
            let attach = at(&view, place).unwrap_or_else(|| panic!("{place}"));
            assert!(!attach.write);
        }
        assert_eq!(at(&view, "/usr/share"), None);
        assert_eq!(what(&view, "/usr/share/zoneinfo"), person("/system/resources/zoneinfo", false));
        // A base with no share folder at all gets the zone database alone.
        facts.base = Some(Base { usr: entries(&["bin", "lib"]), share: None });
        let view = facts.plan().unwrap();
        assert!(at(&view, "/usr/bin").is_some());
        assert_eq!(what(&view, "/usr/share/zoneinfo"), person("/system/resources/zoneinfo", false));
    }

    #[test]
    fn a_static_program_has_no_usr_but_the_zone_database() {
        let mut facts = run();
        facts.base = None;
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/usr"), None);
        assert!(
            view.links
                .iter()
                .all(|link| !["/lib", "/lib64", "/bin", "/sbin"].contains(&link.at.as_str()))
        );
        assert_eq!(what(&view, "/usr/share/zoneinfo"), person("/system/resources/zoneinfo", false));
        assert!(view.mounts.iter().all(|mount| mount.at != "/usr"));
        // Everything else is as for a dynamic program.
        assert!(at(&view, "/proc").is_some() && at(&view, "/etc/passwd").is_some());
    }

    fn link<'a>(view: &'a View, place: &str) -> Option<&'a str> {
        view.links.iter().find(|link| link.at == place).map(|link| link.target.as_str())
    }

    #[test]
    fn the_view_has_dukovos_shell_and_core_commands_where_linux_keeps_them() {
        let mut facts = run();
        // The base as it is: no usr/bin, and no zone database.
        facts.base = Some(Base { usr: entries(&["lib", "lib64", "share"]), share: None });
        facts.commands = all_commands();
        let view = facts.plan().unwrap();
        for place in ["/system/libraries", "/system/commands/nukesh", "/system/commands/coreutils"]
        {
            assert_eq!(what(&view, place), person(place, false), "{place}");
        }
        for place in ["/system/commands/find", "/system/commands/xargs"] {
            assert_eq!(what(&view, place), person(place, false), "{place}");
        }
        assert_eq!(link(&view, "/usr/bin/sh"), Some("/system/commands/nukesh"));
        assert_eq!(link(&view, "/usr/bin/bash"), Some("/system/commands/nukesh"));
        assert_eq!(link(&view, "/usr/bin/env"), Some("/system/commands/coreutils"));
        assert_eq!(link(&view, "/usr/bin/["), Some("/system/commands/coreutils"));
        assert_eq!(link(&view, "/usr/bin/xargs"), Some("/system/commands/xargs"));
        let linked = view.links.iter().filter(|l| l.at.starts_with("/usr/bin/")).count();
        assert_eq!(linked, 2 + CORE_UTILITIES.split_whitespace().count() + 2);
        // `/bin/sh` is reached as on Linux, through `/bin`.
        assert_eq!(link(&view, "/bin"), Some("usr/bin"));
        assert_eq!(view.links.iter().filter(|l| l.at == "/bin").count(), 1);
        assert!(view.holds("/bin/sh") && view.holds("/system/commands/nukesh"));
    }

    #[test]
    fn the_shell_has_only_the_programs_that_are_there() {
        let mut facts = run();
        facts.base = Some(Base { usr: entries(&["lib", "lib64"]), share: None });
        facts.commands = vec!["nukesh".into()];
        let view = facts.plan().unwrap();
        assert_eq!(link(&view, "/usr/bin/sh"), Some("/system/commands/nukesh"));
        assert_eq!(link(&view, "/usr/bin/env"), None);
        assert_eq!(at(&view, "/system/commands/coreutils"), None);
        // With none of them, there is no shell, nor the libraries for it.
        facts.commands = Vec::new();
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/system/libraries"), None);
        assert_eq!(link(&view, "/bin"), None);
        assert!(!view.links.iter().any(|l| l.at.starts_with("/usr/bin/")));
    }

    #[test]
    fn a_base_with_its_own_usr_bin_gives_its_own_commands() {
        let mut facts = run();
        facts.commands = all_commands();
        // base(true) has usr/bin and a zone database: /usr is attached whole.
        let view = facts.plan().unwrap();
        assert_eq!(
            what(&view, "/usr"),
            Some((Source::Base { path: "usr".into(), folder: true }, false))
        );
        assert!(!view.links.iter().any(|l| l.at.starts_with("/usr/bin/")));
        assert_eq!(at(&view, "/system/commands/nukesh"), None);
        assert_eq!(link(&view, "/bin"), Some("usr/bin"));
        assert_eq!(view.links.iter().filter(|l| l.at == "/bin").count(), 1);
    }

    #[test]
    fn a_base_with_a_zone_database_and_no_usr_bin_is_attached_by_its_entries() {
        let mut facts = run();
        facts.base = Some(Base {
            usr: entries(&["lib", "lib64", "share"]),
            share: Some(entries(&["zoneinfo", "locale"])),
        });
        facts.commands = all_commands();
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/usr"), None);
        // The base's zone database, not Dukovo's.
        assert_eq!(
            what(&view, "/usr/share/zoneinfo"),
            Some((Source::Base { path: "usr/share/zoneinfo".into(), folder: true }, false))
        );
        assert_eq!(view.attaches.iter().filter(|a| a.at == "/usr/share/zoneinfo").count(), 1);
        assert_eq!(link(&view, "/usr/bin/sh"), Some("/system/commands/nukesh"));
    }

    #[test]
    fn a_static_program_has_the_shell_too() {
        let mut facts = run();
        facts.base = None;
        facts.commands = all_commands();
        let view = facts.plan().unwrap();
        assert_eq!(link(&view, "/usr/bin/sh"), Some("/system/commands/nukesh"));
        assert_eq!(link(&view, "/bin"), Some("usr/bin"));
        assert_eq!(what(&view, "/system/libraries"), person("/system/libraries", false));
        assert_eq!(what(&view, "/usr/share/zoneinfo"), person("/system/resources/zoneinfo", false));
    }

    #[test]
    fn system_is_not_the_persons_to_give() {
        let mut facts = run();
        facts.allow = vec!["/system/settings".into()];
        let refused = facts.plan().unwrap_err();
        assert!(
            refused.contains("/system/settings is one of the program's own places"),
            "{refused}"
        );
    }

    /// The list of uutils' utilities is the one the port builds.
    #[test]
    fn the_core_utilities_are_those_the_port_builds() {
        let script =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../../ports/uutils-coreutils/build.sh");
        let text = std::fs::read_to_string(script).unwrap();
        let (_, rest) = text.split_once("utilities=\"").unwrap();
        let (list, _) = rest.split_once('"').unwrap();
        let built: Vec<&str> = list.split_whitespace().collect();
        assert_eq!(built, CORE_UTILITIES.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn proc_and_sys_are_linuxds_and_tmp_and_dev_shm_are_private() {
        let view = run().plan().unwrap();
        assert_eq!(what(&view, "/proc"), Some((Source::Linuxd("proc"), true)));
        assert_eq!(what(&view, "/sys"), Some((Source::Linuxd("sys"), true)));
        assert_eq!(what(&view, "/tmp"), person("/tmp/.linux-7/tmp", true));
        assert_eq!(what(&view, "/dev/shm"), Some((Source::Memory, true)));
    }

    #[test]
    fn dev_has_the_devices_that_are_there_and_the_links() {
        let mut facts = run();
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, "/dev/null"), person("/devices/null", true));
        assert_eq!(what(&view, "/dev/zero"), person("/devices/zero", false));
        assert_eq!(what(&view, "/dev/urandom"), person("/devices/urandom", false));
        // There is no /devices/full here.
        assert_eq!(at(&view, "/dev/full"), None);
        let links: Vec<(&str, &str)> = view
            .links
            .iter()
            .filter(|l| l.at.starts_with("/dev/"))
            .map(|l| (l.at.as_str(), l.target.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                ("/dev/fd", "/proc/self/fd"),
                ("/dev/stdin", "/proc/self/fd/0"),
                ("/dev/stdout", "/proc/self/fd/1"),
                ("/dev/stderr", "/proc/self/fd/2"),
            ]
        );
        // With no terminal, none of the devices a terminal opens.
        assert_eq!(at(&view, "/devices"), None);
        facts.terminal = Terminal::Tty;
        let view = facts.plan().unwrap();
        let tty = Source::Names { folder: "/devices".into(), names: vec!["tty".into()] };
        assert_eq!(what(&view, "/devices"), Some((tty, true)));
        assert!(view.links.iter().any(|l| l.at == "/dev/tty" && l.target == "/devices/tty"));
        assert!(!view.links.iter().any(|l| l.at == "/dev/ptmx"));
    }

    #[test]
    fn a_pseudo_terminal_gives_ptmx_and_pts_where_they_are() {
        let mut facts = run();
        facts.terminal = Terminal::Pty;
        let view = facts.plan().unwrap();
        let names = ["tty", "ptmx", "pts"].map(String::from).to_vec();
        let all = Source::Names { folder: "/devices".into(), names };
        assert_eq!(what(&view, "/devices"), Some((all, true)));
        for name in ["tty", "ptmx", "pts"] {
            let target = format!("/devices/{name}");
            assert!(
                view.links.iter().any(|l| l.at == format!("/dev/{name}") && l.target == target)
            );
        }
        // A machine that has no ptmx leaves it out.
        facts.devices.retain(|name| name != "ptmx");
        let view = facts.plan().unwrap();
        let some =
            Source::Names { folder: "/devices".into(), names: vec!["tty".into(), "pts".into()] };
        assert_eq!(what(&view, "/devices"), Some((some, true)));
        assert!(!view.links.iter().any(|l| l.at == "/dev/ptmx"));
    }

    #[test]
    fn etc_holds_each_file_written_the_certificates_and_the_zone() {
        let mut facts = run();
        facts.etc.retain(|name| name != "os-release");
        let view = facts.plan().unwrap();
        for name in
            ["passwd", "group", "hosts", "hostname", "resolv.conf", "nsswitch.conf", "machine-id"]
        {
            let place = format!("/etc/{name}");
            assert_eq!(
                what(&view, &place),
                person(&format!("/tmp/.linux-7/etc/{name}"), false),
                "{name}"
            );
        }
        assert_eq!(at(&view, "/etc/os-release"), None);
        assert_eq!(
            what(&view, "/etc/ssl/certs/ca-certificates.crt"),
            person("/system/resources/certificates/ca-certificates.crt", false)
        );
    }

    #[test]
    fn the_network_folders_are_attached_to_read() {
        let view = run().plan().unwrap();
        assert_eq!(what(&view, "/system/live/resolved"), person("/system/live/resolved", false));
    }

    #[test]
    fn the_started_in_folder_and_the_data_folder_are_attached_to_change() {
        let view = run().plan().unwrap();
        assert_eq!(what(&view, "/users/alice/work"), person("/users/alice/work", true));
        assert_eq!(
            what(&view, "/users/alice/app-data/linux/hello"),
            person("/users/alice/app-data/linux/hello", true)
        );
        assert_eq!(view.working, "/users/alice/work");
        // Its own folder, since it is elsewhere, to read.
        assert_eq!(what(&view, "/users/alice/tools"), person("/users/alice/tools", false));
        assert_eq!(at(&view, "/users/alice"), None);
    }

    #[test]
    fn the_programs_folder_inside_the_started_in_folder_is_not_attached_again() {
        let mut facts = run();
        facts.program = "/users/alice/work/bin/hello".into();
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/users/alice/work/bin"), None);
        assert_eq!(what(&view, "/users/alice/work"), person("/users/alice/work", true));
        // In the folder itself, too.
        facts.program = "/users/alice/work/hello".into();
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/users/alice/work/hello"), None);
    }

    #[test]
    fn home_is_attached_whole_in_place_of_the_data_and_started_in_folders() {
        let mut facts = run();
        facts.whole_home = true;
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, "/users/alice"), person("/users/alice", true));
        assert_eq!(at(&view, "/users/alice/work"), None);
        assert_eq!(at(&view, "/users/alice/app-data/linux/hello"), None);
        assert_eq!(at(&view, "/users/alice/tools"), None);
        assert_eq!(view.working, "/users/alice/work");
        // A folder outside the home is still its place to start in.
        facts.started_in = Some("/volumes/stick".into());
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, "/volumes/stick"), person("/volumes/stick", true));
        assert_eq!(view.working, "/volumes/stick");
    }

    #[test]
    fn a_bundles_sandboxed_program_has_its_folder_read_only_and_starts_in_its_data() {
        let mut facts = run();
        let bundle = "/users/alice/applications/org.example.tool";
        facts.started_in = None;
        facts.bundle = Some(bundle.into());
        facts.program = format!("{bundle}/bin/tool");
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, bundle), Some((Source::Bundle, false)));
        assert_eq!(at(&view, &format!("{bundle}/bin")), None);
        assert_eq!(at(&view, "/users/alice/work"), None);
        assert_eq!(view.working, "/users/alice/app-data/linux/hello");
        assert!(view.notes.is_empty(), "{:?}", view.notes);
        assert!(view.mounts.iter().any(|m| m.at == bundle && !m.write));
    }

    #[test]
    fn a_bundle_in_the_home_given_whole_is_reached_through_it() {
        let mut facts = run();
        facts.whole_home = true;
        let bundle = "/users/alice/applications/org.example.tool";
        facts.bundle = Some(bundle.into());
        facts.program = format!("{bundle}/bin/tool");
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, "/users/alice"), person("/users/alice", true));
        assert_eq!(at(&view, bundle), None);
        assert_eq!(view.working, "/users/alice/work");
        // Installed for everyone, it is attached beside the home.
        let everyone = "/applications/org.example.tool";
        facts.bundle = Some(everyone.into());
        facts.program = format!("{everyone}/bin/tool");
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, everyone), Some((Source::Bundle, false)));
        assert_eq!(at(&view, &format!("{everyone}/bin")), None);
    }

    #[test]
    fn what_is_allowed_is_attached_as_given_and_what_is_inside_is_passed_over() {
        let mut facts = run();
        facts.allow = vec!["/shared/photos".into(), "/users/alice/work/notes".into()];
        facts.allow_write = vec!["/users/alice/out".into()];
        facts.asked = vec!["/users/alice/docs/a.txt".into()];
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, "/shared/photos"), person("/shared/photos", false));
        assert_eq!(what(&view, "/users/alice/out"), person("/users/alice/out", true));
        assert_eq!(
            what(&view, "/users/alice/docs/a.txt"),
            person("/users/alice/docs/a.txt", false)
        );
        // Inside the folder it was started in, which may be changed.
        assert_eq!(at(&view, "/users/alice/work/notes"), None);
    }

    #[test]
    fn a_place_to_change_inside_one_given_to_read_is_refused() {
        let mut facts = run();
        facts.allow = vec!["/shared/photos".into()];
        facts.allow_write = vec!["/shared/photos/new".into()];
        let why = facts.plan().unwrap_err();
        assert_eq!(
            why,
            "/shared/photos/new is inside /shared/photos, which is given read-only, and cannot be \
             changed there: give /shared/photos with --allow-write instead"
        );
        // The data folder is inside the home given to read.
        let mut facts = run();
        facts.allow = vec!["/users/alice".into()];
        let why = facts.plan().unwrap_err();
        assert!(why.contains(" is inside /users/alice, which is given read-only"), "{why}");
        // Given both ways, the place is changeable.
        let mut facts = run();
        facts.allow = vec!["/shared/photos".into()];
        facts.allow_write = vec!["/shared/photos".into()];
        let view = facts.plan().unwrap();
        assert_eq!(what(&view, "/shared/photos"), person("/shared/photos", true));
    }

    #[test]
    fn the_programs_own_places_are_not_given() {
        for path in ["/usr/lib", "/etc", "/tmp/x", "/dev/sda", "/proc", "/sys/kernel", "/", "/bin"]
        {
            let mut facts = run();
            facts.allow = vec![path.into()];
            let why = facts.plan().unwrap_err();
            assert!(
                why.starts_with(&format!("{path} is one of the program's own places")),
                "{why}"
            );
        }
        // A name that merely starts like one is a place of the person's.
        let mut facts = run();
        facts.allow = vec!["/tmpfiles".into(), "/usr2/x".into()];
        assert!(facts.plan().is_ok());
    }

    #[test]
    fn a_started_in_folder_that_is_the_programs_own_is_left_out() {
        let mut facts = run();
        facts.started_in = Some("/tmp/build".into());
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/tmp/build"), None);
        assert_eq!(view.working, "/users/alice/app-data/linux/hello");
        assert_eq!(
            view.notes,
            [
                "/tmp/build is not given to the program (that place is the program's own); it starts in its data folder"
            ]
        );
    }

    #[test]
    fn a_started_in_folder_that_holds_the_home_is_left_out_unless_home_is_given() {
        for started in ["/users/alice", "/users", "/users/alice/app-data"] {
            let mut facts = run();
            facts.started_in = Some(started.into());
            let view = facts.plan().unwrap();
            assert_eq!(at(&view, started), None, "{started}");
            assert_eq!(view.working, "/users/alice/app-data/linux/hello", "{started}");
            assert!(view.notes[0].ends_with(
                "(it holds your home folder, which --home gives); it starts in its data folder"
            ));
        }
        let mut facts = run();
        facts.started_in = Some("/users/alice".into());
        facts.whole_home = true;
        let view = facts.plan().unwrap();
        assert_eq!(view.working, "/users/alice");
        assert!(view.notes.is_empty());
        assert_eq!(what(&view, "/users/alice"), person("/users/alice", true));
    }

    #[test]
    fn mounts_say_what_the_view_holds_in_linuxs_format() {
        let view = run().plan().unwrap();
        let text = mounts_text(&view.mounts);
        assert_eq!(
            text,
            "linux-base /usr linux-base ro,nosuid,nodev 0 0\n\
             proc /proc proc rw,nosuid,nodev,noexec 0 0\n\
             sysfs /sys sysfs rw,nosuid,nodev,noexec 0 0\n\
             tmpfs /tmp tmpfs rw,nosuid,nodev 0 0\n\
             tmpfs /dev/shm tmpfs rw,nosuid,nodev 0 0\n\
             dukovo /users/alice/work dukovo rw,nosuid,nodev 0 0\n\
             dukovo /users/alice/tools dukovo ro,nosuid,nodev 0 0\n\
             dukovo /users/alice/app-data/linux/hello dukovo rw,nosuid,nodev 0 0\n"
        );
    }

    #[test]
    fn a_mount_point_with_a_space_is_written_as_linux_escapes_it() {
        let mounts = [Mount { at: "/volumes/my stick\\1\n".into(), kind: "dukovo", write: true }];
        assert_eq!(
            mounts_text(&mounts),
            "dukovo /volumes/my\\040stick\\1341\\012 dukovo rw,nosuid,nodev 0 0\n"
        );
    }

    #[test]
    fn the_view_holds_what_is_attached_linked_or_its_own() {
        let view = run().plan().unwrap();
        for held in [
            "/users/alice/work",
            "/users/alice/work/src/main.c",
            "/etc/anything",
            "/lib/x86_64-linux-gnu",
            "/dev/null",
            "/system/live/resolved/resolv.conf",
        ] {
            assert!(view.holds(held), "{held}");
        }
        for not in ["/users/alice", "/users/alice/docs", "/users/alice/workshop", "/volumes/stick"]
        {
            assert!(!view.holds(not), "{not}");
        }
    }

    #[test]
    fn only_what_exists_outside_the_view_is_asked_about_once() {
        let view = run().plan().unwrap();
        let args: Vec<String> = [
            "--verbose",
            "/users/alice/docs/a.txt",
            "/users/alice/docs/b.txt",
            "/users/alice/docs",
            "notes.txt",
            "../docs/a.txt",
            "/etc/hosts",
            "/volumes/stick/x",
            "/users/alice/docs/a.txt",
            "missing",
        ]
        .map(String::from)
        .to_vec();
        let exists = |arg: &str| match arg {
            "/users/alice/docs/a.txt" | "../docs/a.txt" => Some("/users/alice/docs/a.txt".into()),
            "/users/alice/docs/b.txt" => Some(arg.into()),
            "/users/alice/docs" => Some(arg.into()),
            "notes.txt" => Some("/users/alice/work/notes.txt".into()),
            "/etc/hosts" | "/volumes/stick/x" => Some(arg.into()),
            _ => None,
        };
        // The folder covers the two files of it, which it was named after.
        assert_eq!(to_ask(&args, exists, &view), ["/users/alice/docs", "/volumes/stick/x"]);
    }

    #[test]
    fn nothing_is_asked_when_the_arguments_name_nothing_outside() {
        let view = run().plan().unwrap();
        let args = ["--help".to_string(), "notes.txt".to_string()];
        let resolve =
            |arg: &str| (arg == "notes.txt").then(|| "/users/alice/work/notes.txt".into());
        assert!(to_ask(&args, resolve, &view).is_empty());
        assert!(to_ask(&[], |_| None, &view).is_empty());
    }

    #[test]
    fn the_question_names_the_program_and_the_path() {
        assert_eq!(
            question("hello", "/users/alice/docs"),
            "Let hello read /users/alice/docs? [y/N] "
        );
    }

    #[test]
    fn only_a_yes_is_a_yes() {
        for yes in ["y", "Y", "yes", "YES", " yes \n", "y\n"] {
            assert!(said_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "ye", "1", "yes please"] {
            assert!(!said_yes(no), "{no:?}");
        }
    }

    #[test]
    fn a_folder_said_yes_to_that_holds_a_place_to_change_is_not_given() {
        let mut facts = run();
        facts.asked = vec!["/users".into(), "/users/alice/docs".into()];
        let view = facts.plan().unwrap();
        assert_eq!(at(&view, "/users"), None);
        assert_eq!(what(&view, "/users/alice/docs"), person("/users/alice/docs", false));
        assert_eq!(what(&view, "/users/alice/work"), person("/users/alice/work", true));
        assert_eq!(
            view.notes,
            ["/users is not given to the program: a place it may change is inside it"]
        );
    }
}
