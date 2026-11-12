// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An update from the update server, under UEFI on an installed system's
//! disk (ADRs 0466 and 0467). The host serves the test update server's
//! repository (`out/test/update-server`, [`TEST_SERVER`]), which the
//! update server's own scripts published and signed, over HTTP, as a
//! static file server does, with parts of files on request, and the
//! machine follows its stable channel from it. The machine's root is the
//! first of three: the second names the server's online key for snapshot
//! and timestamp, and the third replaces the release keys' own timestamp
//! key, as a year of a server's life would leave it, so the machine
//! walks to the third root. The channel offers `year`, which
//! updates only from `bridge`, so `sysupdate check` names `bridge`
//! first, and the number of files the machine lacks.
//!
//! A file served with a changed byte is refused, and so is a release
//! list with one; a timestamp older than the one the machine read before
//! is refused as a replay. A fetch cut off part way goes on from where it
//! stopped, and the files fetched are exactly those the running slot
//! lacks, each once. The stage takes `bridge`, the restart tries it, the
//! health check proves it, and `sysupdate check` then counts what `year`
//! needs: one file moved within the slot is copied, not fetched. An
//! older release, asked for or offered by the beta channel, is never
//! staged.
//!
//! The stable channel's release is paused on its first day (ADR 0469):
//! the machine, which is no canary and whose wave opens on day 7, is told
//! so and waits; as a canary it takes it. Once the bridge runs, update(8)
//! holds the release back and lets it go, lists the bridge by the start
//! that first ran it, and updated(8) says the release is ready (ADR
//! 0502). The release is staged; once the release keys have withdrawn
//! it and the server has published that, updated(8) unstages it, and
//! the machine says so and stages nothing.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::super::*;
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "update-online",
    about: "an update from the update server's stable channel (ADRs 0466, 0467): the machine walks from its root to the server's third, is told a bridge release comes first, refuses a changed file, a changed list and a replayed timestamp, goes on with a fetch cut short, fetches only the files it lacks, stages the bridge, starts it and proves it, and never takes an older release; it waits for its wave while the release is paused and takes it as a canary, and stages nothing once the release is withdrawn, which updated unstages once staged (ADR 0469); update holds it back and lets it go, and updated says it is ready (ADR 0502)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: update_online,
    disks: &[],
    second: None,
};

/// What the server does wrong, for a while.
#[derive(Default)]
struct Faults {
    /// Paths under the repository served with their last byte changed.
    damaged: Option<&'static str>,
    /// A timestamp served in place of the newest.
    stale: Option<Vec<u8>>,
    /// A path whose next answer stops after so many bytes of the body.
    cut: Option<(&'static str, usize)>,
}

/// What the server's connections share.
struct Shared {
    /// The folder served as `/system/`.
    repo: Mutex<PathBuf>,
    faults: Mutex<Faults>,
    /// Each request: its path, and its Range header.
    log: Mutex<Vec<(String, Option<String>)>>,
}

/// The test update server: the repository, over HTTP on a free port of
/// the host's loopback, which the machine reaches as 10.0.2.2. It stops
/// when dropped.
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

impl Server {
    fn start(repo: &Path) -> Result<Server, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|err| format!("no port for the update server: {err}"))?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            repo: Mutex::new(repo.to_path_buf()),
            faults: Mutex::new(Faults::default()),
            log: Mutex::new(Vec::new()),
        });
        let (stopping, serving) = (stop.clone(), shared.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let shared = serving.clone();
                    std::thread::spawn(move || {
                        let _ = shared.answer(stream);
                    });
                }
            }
        });
        Ok(Server { port, stop, shared })
    }

    /// Serves `repo` from now on.
    fn serve(&self, repo: &Path) {
        *self.shared.repo.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            repo.to_path_buf();
    }

    fn faults(&self) -> std::sync::MutexGuard<'_, Faults> {
        self.shared.faults.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The requests for files of the store so far, and the ranges asked.
    fn file_requests(&self) -> Vec<(String, Option<String>)> {
        let log = self.shared.log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        log.iter().filter(|(path, _)| path.starts_with("files/")).cloned().collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

impl Shared {
    /// Answers the one request on `stream`, as a static file server does.
    fn answer(&self, stream: TcpStream) -> Result<(), String> {
        let mut reader = BufReader::new(stream.try_clone().map_err(|err| err.to_string())?);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|err| err.to_string())?;
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let mut range = None;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).map_err(|err| err.to_string())?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("range")
            {
                range = Some(value.trim().to_string());
            }
        }
        let mut stream = stream;
        let path = target.strip_prefix("/system/").unwrap_or("").to_string();
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((path.clone(), range.clone()));
        if method != "GET" || path.is_empty() || path.split('/').any(|p| p == ".." || p.is_empty())
        {
            return reply(&mut stream, 404, &[], b"not here\n", None);
        }
        let mut faults = self.faults.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut data = match (&faults.stale, path.as_str()) {
            (Some(stale), "metadata/timestamp.json") => stale.clone(),
            _ => match std::fs::read(
                self.repo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).join(&path),
            ) {
                Ok(data) => data,
                Err(_) => return reply(&mut stream, 404, &[], b"not here\n", None),
            },
        };
        if faults.damaged.is_some_and(|prefix| path.starts_with(prefix))
            && let Some(last) = data.last_mut()
        {
            *last ^= 0x20;
        }
        let cut = match faults.cut {
            Some((prefix, after)) if path.starts_with(prefix) => {
                faults.cut = None;
                Some(after)
            }
            _ => None,
        };
        drop(faults);
        let from = range
            .as_deref()
            .and_then(|r| r.strip_prefix("bytes="))
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|n| n.parse::<usize>().ok());
        match from {
            Some(from) if from >= data.len() => reply(&mut stream, 416, &[], b"", None),
            Some(from) => {
                let content_range = format!("bytes {from}-{}/{}", data.len() - 1, data.len());
                reply(&mut stream, 206, &[("Content-Range", content_range)], &data[from..], cut)
            }
            None => reply(&mut stream, 200, &[], &data, cut),
        }
    }
}

/// Writes an answer, its body cut after `cut` bytes when asked.
fn reply(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(&str, String)],
    body: &[u8],
    cut: Option<usize>,
) -> Result<(), String> {
    let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let body = &body[..cut.unwrap_or(body.len()).min(body.len())];
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .map_err(|err| err.to_string())
}

/// How many files `name`'s update fetches, as the server's layout counted
/// them.
fn expected(dir: &Path, name: &str) -> Result<usize, String> {
    let path = dir.join("expected.toml");
    let mut text = String::new();
    std::fs::File::open(&path)
        .and_then(|mut file| file.read_to_string(&mut text))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{name} = ")))
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("{} does not count {name}", path.display()))
}

/// Check, refuse, fetch, stage, start, prove.
fn update_online(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    let dir = Path::new("out/test").join(TEST_SERVER);
    let (bridge, year, old) =
        (test_server_version("bridge"), test_server_version("year"), test_server_version("old"));
    let (bridge_files, year_files) = (expected(&dir, "bridge")?, expected(&dir, "year")?);
    // The repository as it was before the stable release was withdrawn.
    let server = Server::start(&dir.join(TEST_SERVER_BEFORE_PULL))?;
    let stale = std::fs::read(dir.join("stale-timestamp.json")).map_err(|err| err.to_string())?;

    started(vm, 'a')?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    let settings = "/system/settings/sysupdate";
    run(
        vm,
        &format!(
            "echo 'server = \"http://10.0.2.2:{}/system\"' > {settings}/updates.toml && echo \
             'stable = 10' > {settings}/health.toml && echo \"set: $((6 * 7))\"",
            server.port
        ),
        &["set: 42"],
    )?;
    // Wave 4, which opens on day 7, and the release is paused anyway.
    run(vm, "echo 500 > /system/state/sysupdate/wave && sysupdate check", &[
        &format!("sysupdate: the stable channel offers Dukovo {year}; this system runs Dukovo {VERSION}"),
        "sysupdate: it reaches machines in waves; this machine's opens on day 7",
        &format!("sysupdate: the waves are held: paused: {TEST_PAUSE}"),
        &format!("sysupdate: `sysupdate stage --version {year}` takes it now"),
    ])?;
    // A canary takes it on its first day.
    run(
        vm,
        &format!("echo 'canary = true' >> {settings}/updates.toml && sysupdate check"),
        &[
            &format!("sysupdate: the stable channel offers Dukovo {year}; this system runs Dukovo {VERSION}"),
            &format!("Dukovo {year} updates only from {bridge} on: Dukovo {bridge} comes first"),
            &format!("Dukovo {bridge} needs {bridge_files} files this system lacks"),
        ],
    )?;

    // A changed byte in a file of the store, then in a release list; a
    // replayed timestamp.
    server.faults().damaged = Some("files/");
    run(
        vm,
        "sysupdate download; echo \"download $?\"",
        &["is not the file the signed list names", "download 1"],
    )?;
    server.faults().damaged = Some("targets/system/");
    run(vm, "rm -r /system/state/sysupdate/lists; sysupdate check; echo \"check $?\"", &[
        "sha256 is ",
        "check 1",
    ])?;
    *server.faults() = Faults { stale: Some(stale), ..Faults::default() };
    run(vm, "sysupdate check; echo \"check $?\"", &["is older than the trusted", "check 1"])?;

    // The first file cut off after 1000 bytes: the next try asks for the
    // rest. Exactly the files the slot lacks are fetched.
    *server.faults() = Faults { cut: Some(("files/", 1000)), ..Faults::default() };
    let before = server.file_requests().len();
    run(vm, "sysupdate download", &[&format!(
        "sysupdate: fetched {bridge_files} files of Dukovo {bridge}"
    )])?;
    let requests = server.file_requests();
    let fetched: std::collections::BTreeSet<&str> =
        requests[before..].iter().map(|(path, _)| path.as_str()).collect();
    if fetched.len() != bridge_files {
        return Err(format!("{} files were fetched, not {bridge_files}: {fetched:?}", fetched.len()));
    }
    if !requests[before..].iter().any(|(_, range)| range.as_deref() == Some("bytes=1000-")) {
        return Err(format!("no fetch went on from byte 1000: {:?}", &requests[before..]));
    }

    // Older releases are never staged.
    run(vm, &format!("sysupdate stage --version {old}; echo \"stage $?\""), &[
        &format!("nothing to stage: Dukovo {old} was asked for, and this system runs Dukovo {VERSION}"),
        "stage 1",
    ])?;

    // The stage fetches nothing more.
    let before = server.file_requests().len();
    run(vm, "sysupdate stage", &[
        &format!("Dukovo {year} updates only from {bridge} on: Dukovo {bridge} comes first"),
        "fetching 0 files",
        &format!("sysupdate: staged Dukovo {bridge} in slot B: it starts at the next restart"),
    ])?;
    if server.file_requests().len() != before {
        return Err("the stage fetched files the download had fetched".into());
    }
    run(vm, "test -e /system/state/sysupdate/files || echo \"no files $((6 * 7))\"", &["no files 42"])?;

    restart(vm)?;
    started(vm, 'b')?;
    vm.expect(&format!("sysupdate: slot B (Dukovo {bridge}) proved itself: it is now the default"))?;
    run(vm, "cat /system/resources/test-update/notes.txt", &["bridge"])?;
    run(vm, "sysupdate check", &[
        &format!("sysupdate: the stable channel offers Dukovo {year}; this system runs Dukovo {bridge}"),
        &format!("Dukovo {year} needs {year_files} files this system lacks"),
    ])?;
    // One command for every update (ADR 0502): a hold, which the check
    // names, lifted; the history, by start; and updated's check.
    run(vm, "update hold system && update", &[
        &format!("update: system is held back at {year}"),
        &format!("it is held back, as Dukovo {year} is"),
        "Firmware:",
        "Apps:",
    ])?;
    run(vm, "update unhold system && update history", &[
        "update: system is no longer held back",
        &format!("Dukovo {VERSION} to {bridge}; good"),
    ])?;
    run(vm, "updated --once", &[&format!(
        "updated: Dukovo {year} is ready: `elevate update apply` stages it"
    )])?;
    // Staged, then the release keys withdraw it, and the server
    // publishes that: updated's next check unstages it.
    run(vm, &format!("sysupdate stage --version {year}"), &[&format!(
        "sysupdate: staged Dukovo {year} in slot A: it starts at the next restart"
    )])?;
    server.serve(&dir.join("system"));
    run(vm, "updated --once; sysupdate status", &[
        &format!("updated: Dukovo {year}, which was staged, was withdrawn: {TEST_PULL}"),
        "Next start: slot B",
    ])?;
    run(vm, "sysupdate check", &[&format!(
        "sysupdate: Dukovo {year} was withdrawn: {TEST_PULL}; nothing to do"
    )])?;
    run(vm, "sysupdate channel beta && sysupdate check", &[
        "this system follows the beta channel from now on",
        &format!("the beta channel offers Dukovo {old}, older than this system's {bridge}: nothing to do"),
    ])?;
    run(vm, "sysupdate channel", &["Channel: beta", &format!("Server:  http://10.0.2.2:{}/system/", server.port)])?;
    power_off(vm)
}
