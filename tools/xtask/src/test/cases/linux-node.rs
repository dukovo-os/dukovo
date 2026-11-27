// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Node.js, the catalog's bundles of Node.js's own Linux builds (ADR 0627),
//! runs a project's work on Dukovo: `npm ci` with install scripts, esbuild,
//! Rollup, SWC and sharp, each with its native code, all in the Linux view
//! of a Linux bundle (ADR 0625) with memory.jit (ADR 0624).
//!
//! The disk holds the Linux base's bundle and both lines' bundles, which ob
//! packed from ports/linux-base, ports/node-24 and ports/node-26; the
//! project in tools/xtask/src/test/linux-node; the npm cache its packages
//! install from; and what the work makes on the build machine's Linux, the
//! SHA-256 of each step's output. ob makes the cache and those sums with
//! the releases themselves, run on the build machine under strace, and
//! checks that each Linux call the releases' programs made there is one
//! Dukovo's Linux table has, or one they do without (FALLBACKS below): a
//! call a new release starts to make fails this test before it fails a
//! person.
//!
//! Dukovo's root grows to 3 GiB, for two lines of Node.js and a project's
//! packages. Alice installs the base and both lines for herself; node-24
//! runs line 24 and asks, on its first run, whether it may run with her
//! access; plain node runs the newest, 26. In the project, whose .nvmrc
//! says 24, node is line 24, and in a folder below whose .node-version
//! says 26 it is line 26. `npm ci` installs the locked packages from the
//! cache with no network, running the install scripts that allowScripts
//! lets run, and the work's four steps make, on Dukovo, what they made on
//! Linux. In her home, `npm install -g` with line 26 puts semver in that
//! line's add-ons, and its command runs.

use std::collections::{BTreeMap, BTreeSet};

use super::super::*;

/// The disk of the bundles and the project's files: its label.
const LABEL: &str = "NODE";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The start of the names of Node.js's bundle files: both lines', which
/// the disk holds and the other tests' disks of bundles leave out.
pub(in crate::test) const BUNDLE_PREFIX: &str = "org.nodejs.node-";

/// Each line's port, the version of its release and its bundle file, which
/// a unit test checks against the ports' recipes.
const LINES: [(&str, &str); 2] = [("node-24", "24.22.3"), ("node-26", "26.13.1")];

/// The bundle file of the line whose port is `port`.
fn bundle_file(port: &str) -> String {
    let (_, version) = LINES.iter().find(|(name, _)| *name == port).expect("a line's port");
    format!("{BUNDLE_PREFIX}{version}.dkv")
}

/// The project: its folder in the repository, from the root, and on the
/// disk.
const PROJECT: &str = "tools/xtask/src/test/linux-node";
const PROJECT_ON_DISK: &str = "linux-node";

/// The npm cache on the disk, and the sums of the work's steps.
const CACHE_ON_DISK: &str = "npm-cache";
const EXPECTED_ON_DISK: &str = "expected.txt";

/// The steps of the work, as workload.mjs names them.
const STEPS: [&str; 4] = ["esbuild", "rollup", "swc", "sharp"];

/// The package `npm install -g` adds, which the project's lock holds, so
/// its tarball is in the cache: its name, version and command.
const ADD_ON: (&str, &str) = ("semver", "7.8.6");

/// Where the kernel's table of Linux calls is, and linuxd's of the calls
/// the kernel forwards to it (ADR 0618), from the repository's root.
const TABLE: &str = "kernel/radix/src/user/linux/mod.rs";
const FORWARDED: &str = "userspace/programs/linuxd/src/serve.rs";

/// Linux calls that the releases' programs make on the build machine and
/// that Dukovo answers with ENOSYS, each with why the program goes on
/// without it. A call that is in the table's arm refused for good is
/// missing unless it is here.
const FALLBACKS: &[(&str, &str)] = &[
    ("rseq", "glibc registers a restartable sequence for each thread and goes on without one"),
    ("io_uring_setup", "libuv tries io_uring, and uses epoll and its thread pool without it"),
    ("io_uring_enter", "libuv calls it only on a ring io_uring_setup made"),
    ("io_uring_register", "libuv calls it only on a ring io_uring_setup made"),
];

// The waits of this test, in seconds, are guesses and not measurements:
// each is a generous multiple of what the step takes on a desktop, so that
// a slow emulated run still passes, and CI's timings will tune them.

/// Unpacking the Linux base and both lines, about 600 MiB, from the FAT
/// disk into the root.
const INSTALL_SECONDS: u64 = 900;
/// A line's first run: V8 starts, and the question is answered.
const FIRST_RUN_SECONDS: u64 = 120;
/// Copying the project and the cache, about 25 MiB, from the FAT disk.
const COPY_SECONDS: u64 = 120;
/// `npm ci`: 15 packages unpacked and two install scripts run.
const CI_SECONDS: u64 = 600;
/// The work's four steps, each a few seconds on a desktop.
const WORK_SECONDS: u64 = 600;
/// `npm install -g` of one package, and a run of its command.
const ADD_ON_SECONDS: u64 = 300;
/// What a step of one small command takes.
const QUICK_SECONDS: u64 = 60;

/// The room after the root the image makes, which is 512 MiB: with it the
/// root has 3 GiB.
const ROOM_KIB: u64 = 2560 * 1024;

/// Dukovo's root on a copy of the test image with room after it, and the
/// disk of the bundles and the project.
const DISKS: &[Disk] = &[
    Disk {
        label: image::ROOT_LABEL,
        kib: ROOM_KIB,
        ..btrfs_disk(Bus::Virtio, 512, ROOT_ROOM, root_checked)
    },
    linux_node_disk(384 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "linux-node",
    about: "Node.js runs a project's work on Dukovo (ADR 0627): the Linux base and the bundles of Node.js 24 and 26, packed from Node.js's own Linux builds, install for one person; node-24 runs line 24 and asks on its first run whether it may run with her access, plain node runs 26, a project's .nvmrc chooses 24 and a .node-version below it 26; npm ci installs the locked packages from an npm cache with no network, running the install scripts allowScripts lets run; esbuild bundles through npx, Rollup through its API, SWC compiles TypeScript and sharp resizes a picture, each making what it made on the build machine's Linux; npm install -g puts semver in line 26's add-ons and its command runs; and ob checks on the build machine, under strace, that every Linux call the releases make is one Dukovo has or does without",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 4,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: linux_node,
    disks: DISKS,
    second: None,
};

/// Where ob does the work on the build machine: a folder made anew for each
/// run.
fn work_folder(ctx: &Ctx) -> PathBuf {
    ctx.target.join("linux-node")
}

/// The npm cache, kept from run to run, so that the packages download
/// once.
fn cache_folder(ctx: &Ctx) -> PathBuf {
    ctx.target.join("linux-node-cache")
}

/// Makes the disk: FAT32 holding the Linux base's bundle and both lines',
/// the project, the npm cache and the work's sums, which [`on_linux`] makes.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_disk(ctx, disk, path, |name| {
        name.starts_with(cases::linux_view::BUNDLE_PREFIX) || name.starts_with(BUNDLE_PREFIX)
    })?;
    let bundles = crate::bundles::dir(ctx);
    for (port, _) in LINES {
        let file = bundles.join(bundle_file(port));
        if !file.exists() {
            return Err(format!("{} is not there (run `./ob ports`)", file.display()));
        }
    }
    let expected = on_linux(ctx)?;
    let image = path.display().to_string();
    let mcopy = |from: &Path, to: &str| {
        host_tool(
            std::process::Command::new("mcopy")
                .args(["-s", "-i", &image])
                .arg(from)
                .arg(format!("::/{to}")),
        )
        .map(drop)
    };
    mcopy(&ctx.root.join(PROJECT), PROJECT_ON_DISK)?;
    mcopy(&cache_folder(ctx), CACHE_ON_DISK)?;
    mcopy(&expected, EXPECTED_ON_DISK)
}

/// A line's release, unpacked from the archive its port pins, and the
/// command that runs its npm: the release's `npm` is a link, which the
/// program it names finds itself from, so ob runs that program with node.
struct Release {
    root: PathBuf,
}

impl Release {
    fn of(ctx: &Ctx, port: &str) -> Result<Self, String> {
        let root = crate::ports::archive_source(ctx, port).map_err(|err| err.to_string())?;
        Ok(Self { root })
    }

    fn node(&self) -> PathBuf {
        self.root.join("bin/node")
    }

    /// A command that runs `program` of the release with `arguments` in
    /// `folder`, under strace, which writes what it saw to `trace`. The
    /// release's `bin` is first in `PATH`, as the bundle's is in its view,
    /// for the commands that npm's scripts and the packages start.
    fn traced(
        &self,
        folder: &Path,
        trace: &Path,
        program: &[&Path],
        arguments: &[&str],
    ) -> std::process::Command {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.root.join("bin")];
        paths.extend(std::env::split_paths(&path));
        let mut command = std::process::Command::new("strace");
        command
            .args(["-f", "-qq", "-o"])
            .arg(trace)
            .args(program)
            .args(arguments)
            .current_dir(folder)
            .env("PATH", std::env::join_paths(paths).unwrap_or(path));
        command
    }

    /// npm's program, run with node.
    fn npm(&self) -> PathBuf {
        self.root.join("lib/node_modules/npm/bin/npm-cli.js")
    }
}

/// Does the work on the build machine, with the releases: `npm ci` and the
/// work with line 24, the project's, and `npm install -g` with line 26,
/// each under strace, and checks the calls they made ([`check_calls`]).
/// The npm cache keeps what they downloaded, including the add-on's
/// registry entry, which `npm install -g --offline` reads on Dukovo.
/// Returns the file of the work's sums.
fn on_linux(ctx: &Ctx) -> Result<PathBuf, String> {
    let work = work_folder(ctx);
    if work.exists() {
        std::fs::remove_dir_all(&work)
            .map_err(|err| format!("clearing {}: {err}", work.display()))?;
    }
    let project = work.join("project");
    copy_folder(&ctx.root.join(PROJECT), &project)?;
    let cache = cache_folder(ctx);
    let cache = cache.to_str().ok_or("the npm cache's path is not UTF-8")?;
    let logs = work.join("npm-logs");
    let logs = logs.to_str().ok_or("the npm logs' path is not UTF-8")?;
    let prefix = work.join("add-ons");
    let prefix = prefix.to_str().ok_or("the add-ons' path is not UTF-8")?;
    let quiet = ["--no-audit", "--no-fund", "--update-notifier=false", "--logs-dir", logs];
    let (project_line, newest) = (Release::of(ctx, "node-24")?, Release::of(ctx, "node-26")?);

    let mut traces = Vec::new();
    let mut run =
        |release: &Release, folder: &Path, name: &str, program: &[&Path], arguments: &[&str]| {
            let trace = work.join(format!("{name}.trace"));
            let out = host_tool(&mut release.traced(folder, &trace, program, arguments))
                .map_err(|err| format!("{name}, on the build machine: {err}"))?;
            traces.push(trace);
            Ok::<Vec<u8>, String>(out)
        };
    let (node, npm) = (project_line.node(), project_line.npm());
    let (node, npm) = (node.as_path(), npm.as_path());
    let ci: Vec<&str> = ["ci", "--cache", cache].iter().chain(&quiet).copied().collect();
    run(&project_line, &project, "ci", &[node, npm], &ci)?;
    let sums = run(&project_line, &project, "workload", &[node], &["workload.mjs"])?;
    let sums = String::from_utf8_lossy(&sums).into_owned();
    for step in STEPS {
        if !sums.lines().any(|line| line.starts_with(&format!("linux-node: {step} "))) {
            return Err(format!("the work, on the build machine, said no sum for {step}: {sums}"));
        }
    }
    let package = format!("{}@{}", ADD_ON.0, ADD_ON.1);
    let (node, npm) = (newest.node(), newest.npm());
    let (node, npm) = (node.as_path(), npm.as_path());
    let global: Vec<&str> = ["install", "-g", "--cache", cache, "--prefix", prefix, &package]
        .iter()
        .chain(&quiet)
        .copied()
        .collect();
    run(&newest, &work, "add-on", &[node, npm], &global)?;

    let read = |path: &str| {
        std::fs::read_to_string(ctx.root.join(path)).map_err(|err| format!("reading {path}: {err}"))
    };
    let answered = linux_table(&read(TABLE)?, &read(FORWARDED)?);
    let roots = [&project_line.root, &newest.root, &project];
    let mut made = BTreeMap::new();
    for trace in &traces {
        let text = std::fs::read_to_string(trace)
            .map_err(|err| format!("reading {}: {err}", trace.display()))?;
        for (program, calls) in traced_calls(&text) {
            if roots.iter().any(|root| Path::new(&program).starts_with(root)) {
                made.entry(program).or_insert_with(BTreeSet::new).extend(calls);
            }
        }
    }
    check_calls(&made, &answered)?;

    let expected = work.join(EXPECTED_ON_DISK);
    std::fs::write(&expected, sums)
        .map_err(|err| format!("writing {}: {err}", expected.display()))?;
    Ok(expected)
}

/// Copies the folder `from` to `to`, files and folders.
fn copy_folder(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|err| format!("making {}: {err}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|err| format!("reading {}: {err}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("reading {}: {err}", from.display()))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_folder(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|err| format!("copying {}: {err}", source.display()))?;
        }
    }
    Ok(())
}

/// The Linux calls Dukovo answers, by their Linux names in lower case: those
/// named in the arms of `dispatch` in `table`, kernel/radix/src/user/linux/
/// mod.rs, but for the arm that refuses calls for good with ENOSYS, and
/// those of `handle` in `forwarded`, linuxd's serve.rs, which answers what
/// the kernel forwards to it.
fn linux_table(table: &str, forwarded: &str) -> BTreeSet<String> {
    let mut answered = arms(table, "dispatch");
    answered.extend(arms(forwarded, "handle"));
    answered
}

/// The calls the arms of the `match` in the function `function` of `text`
/// name, by their Linux names in lower case: each arm a line, or lines,
/// of `nr::` constants and `|`, and the arms that end in `return
/// fail(ENOSYS)` left out.
fn arms(text: &str, function: &str) -> BTreeSet<String> {
    fn names(text: &str) -> impl Iterator<Item = String> + '_ {
        text.split("nr::").skip(1).map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect::<String>()
                .to_ascii_lowercase()
        })
    }
    // The function, to the end of its body.
    let body = text
        .split_once(&format!("fn {function}("))
        .map_or("", |(_, rest)| rest.split("\n}\n").next().unwrap_or(rest));
    let mut answered = BTreeSet::new();
    let mut pending = Vec::new();
    for line in body.lines().map(str::trim) {
        if !(line.starts_with("nr::") || line.starts_with("| nr::")) {
            pending.clear();
            continue;
        }
        let (head, rest) =
            line.split_once("=>").map_or((line, None), |(head, rest)| (head, Some(rest)));
        pending.extend(names(head));
        if let Some(rest) = rest {
            if !rest.trim().starts_with("return fail(ENOSYS)") {
                answered.extend(pending.drain(..));
            }
            pending.clear();
        }
    }
    answered.remove("");
    answered
}

/// The calls each program made in `trace`, the output of `strace -f`: by
/// the path of the program, as the `execve` that started it gave it, the
/// calls each of its processes and threads made. A process made with
/// `clone`, `clone3`, `fork` or `vfork` runs its parent's program until it
/// starts one of its own, even where strace wrote its calls before the
/// line where the parent's call returned. Calls before the first program
/// started are left out.
fn traced_calls(trace: &str) -> BTreeMap<String, BTreeSet<String>> {
    struct Call<'a> {
        pid: &'a str,
        name: &'a str,
        rest: &'a str,
    }
    let calls: Vec<Call> = trace
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.split_once(' ')?;
            let rest = rest.trim_start();
            if rest.starts_with("+++") || rest.starts_with("---") {
                return None;
            }
            let name = match rest.strip_prefix("<... ") {
                Some(resumed) => resumed.split_once(' ')?.0,
                None => rest.split_once('(')?.0,
            };
            Some(Call { pid, name, rest })
        })
        .collect();
    // Each process's programs, by where its execve returned, and its
    // parent and where the parent's call that made it returned.
    let mut started: BTreeMap<&str, Vec<(usize, String)>> = BTreeMap::new();
    let mut parents: BTreeMap<&str, (&str, usize)> = BTreeMap::new();
    let mut asked: BTreeMap<&str, String> = BTreeMap::new();
    for (at, call) in calls.iter().enumerate() {
        let returned = call.rest.trim_end().rsplit_once("= ").map(|(_, value)| value);
        if call.name == "execve" {
            if let Some(path) = call.rest.strip_prefix("execve(\"").and_then(|r| r.split_once('"'))
            {
                asked.insert(call.pid, path.0.to_string());
            }
            if returned == Some("0")
                && let Some(path) = asked.get(call.pid)
            {
                started.entry(call.pid).or_default().push((at, path.clone()));
            }
        }
        let made_one = matches!(call.name, "clone" | "clone3" | "fork" | "vfork");
        if made_one
            && let Some(child) = returned
            && !child.is_empty()
            && child.bytes().all(|b| b.is_ascii_digit())
        {
            parents.insert(child, (call.pid, at));
        }
    }
    fn program<'a>(
        pid: &str,
        at: usize,
        started: &'a BTreeMap<&str, Vec<(usize, String)>>,
        parents: &BTreeMap<&str, (&str, usize)>,
        depth: usize,
    ) -> Option<&'a str> {
        let own = started.get(pid).and_then(|s| s.iter().rev().find(|(when, _)| *when <= at));
        match own {
            Some((_, path)) => Some(path),
            None if depth < 64 => {
                let (parent, when) = parents.get(pid)?;
                program(parent, *when, started, parents, depth + 1)
            }
            None => None,
        }
    }
    let mut made: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (at, call) in calls.iter().enumerate() {
        if let Some(path) = program(call.pid, at, &started, &parents, 0) {
            made.entry(path.to_string()).or_default().insert(call.name.to_string());
        }
    }
    made
}

/// Fails, naming each program and its calls, if a program in `made` made a
/// call that is neither in `answered` nor one of the [`FALLBACKS`].
fn check_calls(
    made: &BTreeMap<String, BTreeSet<String>>,
    answered: &BTreeSet<String>,
) -> Result<(), String> {
    let missing: Vec<String> = made
        .iter()
        .filter_map(|(program, calls)| {
            let missing: Vec<&str> = calls
                .iter()
                .map(String::as_str)
                .filter(|call| {
                    !answered.contains(*call) && !FALLBACKS.iter().any(|(name, _)| name == call)
                })
                .collect();
            (!missing.is_empty()).then(|| format!("{program} made {}", missing.join(", ")))
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "on the build machine, Node.js's programs made Linux calls that neither Dukovo's table \
         ({TABLE}) nor linuxd ({FORWARDED}) answers, and that are not among the calls they do \
         without (FALLBACKS in tools/xtask/src/test/cases/linux-node.rs): {}",
        missing.join("; ")
    ))
}

fn linux_node(vm: &mut Vm, case: &Case) -> Result<(), String> {
    use super::go_toolchain::succeeds;
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // The root grows over the disk's room, in use: its partition, then its
    // Btrfs, named by a folder on it.
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate rdisk --grow /devices/vda")?;
    vm.expect_screen(&["/devices/vda3 grows from 512 MiB to 3 GiB."])?;
    answer_prompt(vm, "$", "clear; elevate device resize max /system/settings")?;
    vm.expect_screen(&["Resized device id 1 of '/system/settings' to "])?;

    // For herself, in ~/applications: the base, then both lines, each a
    // bundle of its own.
    let (line_24, line_26) = (bundle_file("node-24"), bundle_file("node-26"));
    for (name, file) in
        [("base", cases::linux_view::FILE), ("line-24", &line_24), ("line-26", &line_26)]
    {
        succeeds(vm, name, &format!("cd {VOLUME}; bundle install --yes {file}"), INSTALL_SECONDS)?;
    }
    answer_prompt(
        vm,
        "$",
        "clear; cd; cat /tmp/line-24 /tmp/line-26 | grep -e installed -e 'machine code'",
    )?;
    vm.expect_screen(&[
        "Make machine code as it runs, as JavaScript engines do",
        "installed org.nodejs.node@24 24.22.3, for you",
        "installed org.nodejs.node@26 26.13.1, for you",
    ])?;

    // Each line asks on its first run whether it may run with her access.
    let (version_24, version_26) = (LINES[0].1, LINES[1].1);
    run_answering_until(
        vm,
        "clear; echo \"line: $(node-24 -p process.version)\"",
        "line: v",
        FIRST_RUN_SECONDS,
    )?;
    vm.expect_screen(&[&format!("line: v{version_24}")])?;
    vm.expect_anywhere("bundled: alice allowed org.nodejs.node@24 runs-with-your-access")?;
    run_answering_until(
        vm,
        "clear; echo \"newest: $(node -p process.version)\"",
        "newest: v",
        FIRST_RUN_SECONDS,
    )?;
    vm.expect_screen(&[&format!("newest: v{version_26}")])?;

    // The project chooses its line, and a folder below it another.
    run_within(
        vm,
        &format!(
            "clear; cp -R {VOLUME}/{PROJECT_ON_DISK} ~/project; cp -R {VOLUME}/{CACHE_ON_DISK} \
             ~/npm-cache; cp {VOLUME}/{EXPECTED_ON_DISK} ~/expected.txt; cd ~/project; echo 24 > \
             .nvmrc; mkdir below; echo v26 > below/.node-version"
        ),
        COPY_SECONDS,
    )?;
    answer_prompt(
        vm,
        "$",
        "clear; echo \"here: $(node --version)\"; echo \"below: $(cd below; node --version)\"",
    )?;
    vm.expect_screen(&[&format!("here: v{version_24}"), &format!("below: v{version_26}")])?;

    // npm ci, from the cache with no network: the packages the lock names,
    // and the install scripts of esbuild and SWC, which allowScripts lets
    // run, each through the view's shell.
    succeeds(
        vm,
        "ci",
        "npm ci --offline --cache ~/npm-cache --no-audit --no-fund --update-notifier=false",
        CI_SECONDS,
    )?;
    answer_prompt(vm, "$", "clear; tail -n 2 /tmp/ci; ls node_modules/.bin")?;
    vm.expect_screen(&["added 15 packages", "esbuild", "rollup", "semver"])?;

    // The work makes what it made on Linux.
    succeeds(vm, "work", "node workload.mjs --expect ~/expected.txt", WORK_SECONDS)?;
    answer_prompt(vm, "$", "clear; cat /tmp/work")?;
    let mut lines = vec![format!("linux-node: node v{version_24} on linux-x64")];
    lines.extend(STEPS.iter().map(|step| format!("linux-node: {step} ok")));
    lines.push(format!("linux-node: all {} passed", STEPS.len()));
    vm.expect_screen(&lines.iter().map(String::as_str).collect::<Vec<_>>())?;

    // An add-on, from her home, where line 26 runs: in that line's add-ons
    // folder, its command linked beside hers.
    let (name, version) = ADD_ON;
    succeeds(
        vm,
        "global",
        &format!(
            "cd; npm install -g --offline --cache ~/npm-cache --no-audit --no-fund \
             --update-notifier=false {name}@{version}"
        ),
        ADD_ON_SECONDS,
    )?;
    run_within(
        vm,
        &format!(
            "clear; ls ~/applications/add-ons/org.nodejs.node@26/bin; echo \"bumped: $({name} \
             1.2.3 -i minor)\""
        ),
        QUICK_SECONDS,
    )?;
    vm.expect_screen(&[name, "bumped: 1.3.0"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository's root, from this crate's folder.
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn read(path: &str) -> String {
        std::fs::read_to_string(root().join(path)).expect(path)
    }

    #[test]
    fn the_lines_are_the_ports_releases() {
        for (port, version) in LINES {
            let recipe = read(&format!("ports/{port}/recipe.toml"));
            assert!(recipe.contains(&format!("version = \"{version}\"")), "{port}");
            let line = version.split('.').next().unwrap();
            assert!(recipe.contains(&format!("line = \"{line}\"")), "{port}");
            assert!(recipe.contains("id = \"org.nodejs.node\""), "{port}");
        }
        assert_eq!(bundle_file("node-26"), "org.nodejs.node-26.13.1.dkv");
    }

    #[test]
    fn the_lock_holds_what_the_project_pins() {
        let package = read(&format!("{PROJECT}/package.json"));
        let lock = read(&format!("{PROJECT}/package-lock.json"));
        let pins = package.split("\"dependencies\": {").nth(1).unwrap().split('}').next().unwrap();
        let mut count = 0;
        for pin in pins.lines().filter(|line| line.contains(':')) {
            let (name, version) = pin.trim().trim_end_matches(',').split_once(": ").unwrap();
            let (name, version) = (name.trim_matches('"'), version.trim_matches('"'));
            let entry = format!("\"node_modules/{name}\": {{\n      \"version\": \"{version}\"");
            assert!(lock.contains(&entry), "the lock has no {name} {version}");
            count += 1;
        }
        assert_eq!(count, 5);
        // The add-on is one of them, so its tarball is in the cache.
        assert!(pins.contains(&format!("\"{}\": \"{}\"", ADD_ON.0, ADD_ON.1)));
        // Install scripts run only for the packages named.
        assert!(package.contains("\"allowScripts\""));
    }

    #[test]
    fn the_work_says_the_lines_the_test_looks_for() {
        let work = read(&format!("{PROJECT}/workload.mjs"));
        for step in STEPS {
            assert!(work.contains(&format!("[\"{step}\", {step}]")), "{step}");
        }
        for line in [
            "linux-node: node ${process.version} on ${process.platform}-${process.arch}",
            "linux-node: ${name} ok",
            "linux-node: all ${STEPS.length} passed",
        ] {
            assert!(work.contains(line), "workload.mjs does not say `{line}`");
        }
    }

    #[test]
    fn the_table_is_the_kernels_arms() {
        let answered = linux_table(&read(TABLE), &read(FORWARDED));
        for call in ["read", "fstatfs", "clone3", "rt_sigaction", "personality", "socket", "mount"]
        {
            assert!(answered.contains(call), "{call}");
        }
        for call in ["io_uring_setup", "bpf", "rseq", ""] {
            assert!(!answered.contains(call), "{call}");
        }
        assert!(arms(&read(TABLE), "no_such_function").is_empty());
        // Each call done without is one Dukovo does not answer, by its
        // Linux name.
        for (call, _) in FALLBACKS {
            assert!(!answered.contains(*call), "{call} is answered");
            let constant = format!("{} = ", call.to_ascii_uppercase());
            assert!(read("kernel/lib/linux/src/nr.rs").contains(&constant), "{call}");
        }
    }

    #[test]
    fn a_trace_gives_each_program_its_calls() {
        let trace = "\
100 execve(\"/opt/node/bin/node\", [\"node\"], 0x7ffd /* 3 vars */) = 0
100 brk(NULL)                         = 0x55d0
101 set_robust_list(0x7f, 24 <unfinished ...>
100 clone3({flags=CLONE_VM, exit_signal=SIGCHLD}, 88 <unfinished ...>
101 <... set_robust_list resumed>)   = 0
100 <... clone3 resumed>)             = 101
101 execve(\"/usr/local/bin/sh\", [\"sh\"], 0x7f /* 3 vars */) = -1 ENOENT (No such file or directory)
101 execve(\"/bin/sh\", [\"sh\"], 0x7f /* 3 vars */) = 0
101 wait4(-1, 0x7ffc, 0, NULL)        = 102
102 rseq(0x7f, 0x20, 0, 0x53053053)   = 0
100 --- SIGCHLD {si_signo=SIGCHLD} ---
100 vfork( <unfinished ...>
102 +++ exited with 0 +++
100 <... vfork resumed>)              = 103
103 execve(\"/p/node_modules/.bin/esbuild\", [\"esbuild\"], 0x7f /* 3 vars */ <unfinished ...>
103 <... execve resumed>)             = 0
103 sched_getaffinity(0, 8192, [0])   = 8
";
        let made = traced_calls(trace);
        // The child's calls before its execve are node's, the execve that
        // failed among them; the vfork is too.
        let node: Vec<&str> = made["/opt/node/bin/node"].iter().map(String::as_str).collect();
        assert_eq!(node, ["brk", "clone3", "execve", "set_robust_list", "vfork"]);
        assert!(made["/bin/sh"].contains("wait4"));
        assert!(made["/p/node_modules/.bin/esbuild"].contains("sched_getaffinity"));
        // 102 has no parent in the trace and started nothing.
        assert!(!made.values().any(|calls| calls.contains("rseq")));

        let answered: BTreeSet<String> =
            ["brk", "clone3", "execve", "set_robust_list", "vfork", "wait4"]
                .map(String::from)
                .into();
        let error = check_calls(&made, &answered).unwrap_err();
        assert!(error.contains("/p/node_modules/.bin/esbuild made sched_getaffinity"), "{error}");
        assert!(!error.contains("/bin/sh made"), "{error}");
    }
}
