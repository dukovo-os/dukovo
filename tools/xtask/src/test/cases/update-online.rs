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
//! staged. Once the bridge proves itself, Rescue is written again from it
//! (ADR 0543), from the metadata the client kept, which reads from the
//! machine's first root through the roots it walked to.
//!
//! The stable channel's release is paused on its first day (ADR 0469):
//! the machine, which is no canary and whose wave opens on day 7, is told
//! so and waits; as a canary it takes it. The channel's firmware set has
//! waves of its own (ADR 0544): `update check` says it waits for its
//! wave, and `sysupdate firmware stage` refuses it, until the machine is
//! a canary. Once the bridge runs, update(8)
//! holds the release back and lets it go, lists the bridge by the start
//! that first ran it, and updated(8) says the release is ready (ADR
//! 0502). The release is staged by update apply, through the
//! sysupdate-stage service, whose view holds the system's subvolumes
//! and no home folder (ADR 0473); once the release keys have withdrawn
//! it and the server has published that, updated(8) unstages it, and
//! the machine says so and stages nothing.
//!
//! Last, the machine, a canary by now, enrols as `boot-canary` and reports
//! (ADR 0533). `sysupdate canary enrol` makes its key and prints the line
//! that lists it on the update server, which the host reads from the screen
//! and gives to the update server's own system-release, as `canary add`,
//! for a canary list in a fresh folder; that program's `canary-receive`
//! listens on a unix socket there, as a child of the test that ends with
//! it, however it ends. The host's HTTP server passes a POST to the path
//! /canary/v1/report on to that socket, as the web server in front of the
//! receiver does, and refuses a body of more than 16 KiB; it answers
//! everything else as it did. `sysupdate canary report --dry-run` says it
//! sends nothing and prints the signed envelope, and the receiver keeps
//! nothing; the report itself is taken, `boot-canary.json` is kept, and
//! the command `canaries show --json` lists the canary with a report of the
//! release the machine runs and the channel it follows. The test does not
//! ask it to be green, as a machine under QEMU may have a warning or a
//! failed service, but its reasons are neither that it has not reported
//! nor that it is silent. A body that is no report is refused with 400,
//! and one that is too large with 413, both sent from the host through the
//! same path, and the folder of reports keeps no other file.
//!
//! Before that, the machine reads the security advisory that the release
//! keys signed with the withdrawal (ADR 0534), through its TUF client:
//! *update advisories* names it, as high, with the withdrawn release as
//! its fix, which no channel offers, and ends with status 1.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;

use super::super::*;
use super::update_stage::{power_off, restart, run, started};

pub const CASE: Case = Case {
    name: "update-online",
    about: "an update from the update server's stable channel (ADRs 0466, 0467): the machine walks from its root to the server's third, is told a bridge release comes first, refuses a changed file, a changed list and a replayed timestamp, goes on with a fetch cut short, fetches only the files it lacks, stages the bridge, starts it and proves it, and never takes an older release; it waits for its wave while the release is paused and takes it as a canary, as it does the channel's firmware set (ADR 0544), and stages nothing once the release is withdrawn, which updated unstages once staged (ADR 0469); Rescue is written again from the bridge it proved, from the metadata its client kept (ADR 0543); update holds it back and lets it go, and updated says it is ready (ADR 0502); last, it enrols as a canary and reports, first as a dry run that sends nothing and then to the update server's receiver, which keeps the signed report and shows the canary with the release it runs and the channel it follows, and refuses a body that is no report and one that is too large (ADR 0533); before that, update advisories names the signed advisory the bridge has the flaw of, with its fix (ADR 0534)",
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
    /// The unix socket of the update server's receiver of canaries'
    /// reports, which `POST /canary/v1/report` is passed on to once a step
    /// of the case starts one (ADR 0533); without it that path is not here,
    /// as every other path that is not a file of the repository.
    relay: Mutex<Option<PathBuf>>,
}

/// The test update server: the repository, over HTTP on a free port of
/// the host's loopback, which the machine reaches as 10.0.2.2, and, when
/// asked, the path canaries post their reports to. It stops when dropped.
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
            relay: Mutex::new(None),
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

    /// Passes the reports posted to the server on to the receiver listening
    /// at `socket`, or, with `None`, no longer.
    fn relay(&self, socket: Option<&Path>) {
        *self.shared.relay.lock().unwrap_or_else(PoisonError::into_inner) =
            socket.map(Path::to_path_buf);
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
        let mut length = None;
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
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let mut stream = stream;
        let relay = self.relay.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if method == "POST"
            && target == REPORT_PATH
            && let Some(socket) = relay
        {
            return relay_report(&mut reader, &mut stream, &socket, length);
        }
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

/// Where a canary posts its report, which the update server's receiver
/// answers (ADR 0533).
const REPORT_PATH: &str = "/canary/v1/report";

/// The most bytes of a report body that are passed on: the receiver's own
/// limit, which refuses more.
const REPORT_MAX: usize = 16 * 1024;

/// The most bytes of the receiver's answer that are passed back.
const ANSWER_MAX: u64 = 64 * 1024;

/// Passes the report on `stream` to the receiver at `socket`, as the web
/// server in front of it does: the head, as `POST` with a length and
/// nothing else the machine sent, and the body, which is at most
/// [`REPORT_MAX`] bytes; then the whole of the receiver's answer back. A
/// receiver that cannot be reached is answered 502, with why.
fn relay_report(
    reader: &mut BufReader<TcpStream>,
    stream: &mut TcpStream,
    socket: &Path,
    length: Option<usize>,
) -> Result<(), String> {
    let Some(length) = length else {
        return refuse(reader, stream, 411, "send the report with a length\n");
    };
    if length > REPORT_MAX {
        return refuse(reader, stream, 413, "a report is at most 16 KiB\n");
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|err| format!("reading the report: {err}"))?;
    match through(socket, &body) {
        Ok(answer) => {
            stream.write_all(&answer).and_then(|()| stream.flush()).map_err(|err| err.to_string())
        }
        Err(why) => reply(stream, 502, &[], format!("{why}\n").as_bytes(), None),
    }
}

/// Sends `body` to the receiver at `socket` as a POST of its path, and
/// reads its whole answer, which the receiver ends by closing.
fn through(socket: &Path, body: &[u8]) -> Result<Vec<u8>, String> {
    let at = |err: std::io::Error| format!("the receiver at {}: {err}", socket.display());
    let mut receiver = UnixStream::connect(socket).map_err(at)?;
    let wait = Some(Duration::from_secs(20));
    receiver.set_read_timeout(wait).and_then(|()| receiver.set_write_timeout(wait)).map_err(at)?;
    let head = format!(
        "POST {REPORT_PATH} HTTP/1.1\r\nHost: updates\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n",
        body.len()
    );
    receiver
        .write_all(head.as_bytes())
        .and_then(|()| receiver.write_all(body))
        .and_then(|()| receiver.flush())
        .map_err(at)?;
    let mut answer = Vec::new();
    (&receiver).take(ANSWER_MAX).read_to_end(&mut answer).map_err(at)?;
    if answer.is_empty() {
        return Err(format!("the receiver at {} answered nothing", socket.display()));
    }
    Ok(answer)
}

/// Answers `status` and `words` without reading the body, then reads and
/// drops what the machine still sends, briefly, so that closing does not
/// reset the connection before the answer is read.
fn refuse(
    reader: &mut BufReader<TcpStream>,
    stream: &mut TcpStream,
    status: u16,
    words: &str,
) -> Result<(), String> {
    reply(stream, status, &[], words.as_bytes(), None)?;
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let (mut sink, mut drained) = ([0_u8; 4096], 0);
    while drained < 4 * REPORT_MAX {
        match reader.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(read) => drained += read,
        }
    }
    Ok(())
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

/// The name the machine enrols under.
const CANARY: &str = "boot-canary";

/// The update server's receiver of canaries' reports, `system-release
/// canary-receive` (ADR 0533), on a unix socket: a child of the test, ended
/// when it is dropped, however the step that started it ends.
struct Receiver {
    child: Child,
    socket: PathBuf,
    /// What it says on its standard error, a line for each report it took
    /// or refused.
    log: PathBuf,
}

impl Receiver {
    /// Starts `tool`'s receiver, which keeps its reports in `reports` for
    /// the canaries on `list`, with its socket and log in `folder`, and
    /// waits until it has made its socket.
    fn start(tool: &Path, folder: &Path, list: &Path, reports: &Path) -> Result<Receiver, String> {
        let (socket, log) = (folder.join("receive.sock"), folder.join("receiver.log"));
        let stderr =
            std::fs::File::create(&log).map_err(|err| format!("{}: {err}", log.display()))?;
        let child = Command::new(tool)
            .arg("canary-receive")
            .arg("--socket")
            .arg(&socket)
            .arg("--canaries")
            .arg(list)
            .arg("--reports")
            .arg(reports)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .map_err(|err| format!("running {}: {err}", tool.display()))?;
        // From here the guard ends the child on every way out.
        let mut receiver = Receiver { child, socket, log };
        for _ in 0..200 {
            if receiver.socket.exists() {
                return Ok(receiver);
            }
            if let Ok(Some(status)) = receiver.child.try_wait() {
                return Err(format!(
                    "the receiver ended ({status}) before it listened: {}",
                    receiver.said()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!("the receiver made no socket in 10 s: {}", receiver.said()))
    }

    /// What it has said so far, on one line.
    fn said(&self) -> String {
        let said = std::fs::read_to_string(&self.log).unwrap_or_default();
        said.trim().replace('\n', " | ")
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs the update server's `system-release` with `args` and gives what it
/// printed.
fn server_release(tool: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(|err| format!("running {}: {err}", tool.display()))?;
    if !output.status.success() {
        return Err(format!(
            "system-release {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The names of the files in `folder`, in order.
fn names_in(folder: &Path) -> Result<Vec<String>, String> {
    let entries =
        std::fs::read_dir(folder).map_err(|err| format!("{}: {err}", folder.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", folder.display()))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// The canary's key, from the line `sysupdate canary enrol` prints for the
/// maintainer's machine, `./ob release canary add NAME KEY`, as the screen
/// shows it: 64 characters of hex, which a narrow screen would break into
/// two rows.
fn enrolled_key(screen: &[String]) -> Result<String, String> {
    let start = format!("./ob release canary add {CANARY} ");
    let (at, first) = screen
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, row)| row.split_once(&start).map(|(_, key)| (at, key.trim())))
        .ok_or_else(|| format!("the screen does not show the line `{start}KEY`"))?;
    let mut key = first.to_string();
    for row in &screen[at + 1..] {
        if key.len() >= 64 {
            break;
        }
        key.push_str(row.trim());
    }
    if key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(key)
    } else {
        Err(format!("`{key}` after `{start}` is not a key of 64 hex digits"))
    }
}

/// POSTs `body` to the test update server's path for reports, as the
/// machine does, and gives the status of the answer and its body.
fn post_report(port: u16, body: &[u8]) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|err| format!("no connection to the update server: {err}"))?;
    let wait = Some(Duration::from_secs(20));
    stream
        .set_read_timeout(wait)
        .and_then(|()| stream.set_write_timeout(wait))
        .map_err(|err| err.to_string())?;
    let head = format!(
        "POST {REPORT_PATH} HTTP/1.1\r\nHost: 10.0.2.2:{port}\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n",
        body.len()
    );
    // A refusal may come before the whole body is sent, so the answer is
    // read whatever the write said.
    let sent = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body));
    let mut answer = Vec::new();
    let read = stream.read_to_end(&mut answer);
    let text = String::from_utf8_lossy(&answer).into_owned();
    let status = text
        .strip_prefix("HTTP/1.1 ")
        .and_then(|rest| rest.get(..3))
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| {
            format!("no answer to the report (sent: {sent:?}, read: {read:?}): {text:?}")
        })?;
    let words = text.split_once("\r\n\r\n").map_or("", |(_, words)| words);
    Ok((status, words.trim().to_string()))
}

/// Checks the standing `canaries show --json` (`shown`) gives the canary
/// of a machine that runs `running` on `channel`: it has reported, the
/// report says what the machine runs and follows, and no reason is that
/// the update server never heard from it or has not for hours. It does not
/// ask for green, which a machine under QEMU may not be. Gives the reasons,
/// or that there are none, for the log.
fn check_standing(shown: &Value, running: &str, channel: &str) -> Result<String, String> {
    let standing = shown["canaries"]
        .as_array()
        .and_then(|all| all.iter().find(|standing| standing["name"] == CANARY))
        .ok_or_else(|| format!("canaries show does not list {CANARY}: {shown}"))?;
    let reasons: Vec<&str> = standing["reasons"]
        .as_array()
        .map_or_default(|reasons| reasons.iter().filter_map(Value::as_str).collect());
    let said = if reasons.is_empty() { "none".to_string() } else { reasons.join("; ") };
    if standing["reported"].as_i64().is_none() {
        return Err(format!("{CANARY} has no `reported` time; its reasons: {said}: {standing}"));
    }
    let report = &standing["report"];
    for (field, wanted) in [("canary", CANARY), ("running", running), ("channel", channel)] {
        if report[field] != wanted {
            return Err(format!(
                "the report says {field} {}, not {wanted}: {standing}",
                report[field]
            ));
        }
    }
    let unheard = |reason: &&str| {
        *reason == "no report since it was added"
            || reason.starts_with("silent")
            || reason.contains("does not verify")
    };
    if let Some(reason) = reasons.iter().copied().find(unheard) {
        return Err(format!("{CANARY} reported, and the update server says: {reason} ({said})"));
    }
    Ok(said)
}

/// The machine, a canary by now, enrols as [`CANARY`] and reports to the
/// update server's receiver, which `server` passes its POST to, and the
/// host reads back what the receiver kept (ADR 0533). `dir` is the test
/// update server's folder, whose `server` is its `/`; the machine runs
/// `running` on `channel`.
fn canary_report(
    vm: &mut Vm,
    server: &Server,
    dir: &Path,
    running: &str,
    channel: &str,
) -> Result<(), String> {
    // The program the update server runs, which the test server's folder
    // holds.
    let tool = dir.join("server/usr/local/lib/dukovo-system/system-release");
    let folder = Path::new("out/test/update-online-canaries");
    if folder.exists() {
        std::fs::remove_dir_all(folder).map_err(|err| format!("{}: {err}", folder.display()))?;
    }
    let reports = folder.join("reports");
    std::fs::create_dir_all(&reports).map_err(|err| format!("{}: {err}", reports.display()))?;
    let list = folder.join("canaries.json");

    // The key the machine made, which the update server lists by name.
    run(
        vm,
        &format!("sysupdate canary enrol {CANARY}"),
        &[
            &format!("sysupdate: this machine is the canary {CANARY}"),
            &format!("./ob release canary add {CANARY} "),
        ],
    )?;
    let key = enrolled_key(&vm.screen()?)?;
    server_release(
        &tool,
        &["canary", "--canaries", &list.display().to_string(), "add", CANARY, &key],
    )?;
    let receiver = Receiver::start(&tool, folder, &list, &reports)?;
    server.relay(Some(&receiver.socket));

    // A dry run says it sends nothing, and the receiver keeps nothing.
    run(vm, "sysupdate canary report --dry-run", &["--dry-run sends nothing", "{\"report\":\""])?;
    let kept = names_in(&reports)?;
    if !kept.is_empty() {
        return Err(format!("the dry run sent a report: the receiver kept {kept:?}"));
    }
    // The report itself is taken.
    run(vm, "sysupdate canary report", &[&format!("reported as the canary {CANARY}")])
        .map_err(|err| format!("{err}; the receiver said: {}", receiver.said()))?;
    let kept = names_in(&reports)?;
    if kept != [format!("{CANARY}.json")] {
        return Err(format!(
            "the receiver kept {kept:?}, not {CANARY}.json; it said: {}",
            receiver.said()
        ));
    }
    let shown = server_release(
        &tool,
        &[
            "canaries",
            "show",
            "--canaries",
            &list.display().to_string(),
            "--reports",
            &reports.display().to_string(),
            "--json",
        ],
    )?;
    let shown: Value = serde_json::from_str(shown.trim())
        .map_err(|err| format!("canaries show --json said {shown:?}: {err}"))?;
    let reasons = check_standing(&shown, running, channel)?;
    eprintln!(
        "          update-online: {CANARY} reported {running} on {channel}; reasons: {reasons}"
    );

    // What is no report is refused, through the same path, and nothing
    // more is kept: a body that is no envelope, and one past 16 KiB.
    for (body, wanted) in [
        (b"this is not a canary's report\n".to_vec(), [400, 403]),
        (vec![b' '; REPORT_MAX + 1], [413, 413]),
    ] {
        let (status, words) = post_report(server.port, &body)?;
        if !wanted.contains(&status) {
            return Err(format!(
                "a body of {} bytes was answered {status} {words:?}, not {}; the receiver said: {}",
                body.len(),
                wanted.map(|status| status.to_string()).join(" or "),
                receiver.said()
            ));
        }
    }
    let kept = names_in(&reports)?;
    if kept != [format!("{CANARY}.json")] {
        return Err(format!(
            "the receiver kept {kept:?} after the refusals, not {CANARY}.json alone"
        ));
    }
    Ok(())
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
    // The channel's firmware set has waves of its own, not paused, whose
    // first is the canaries' (ADR 0544).
    let set = test_firmware_version("good");
    // Each needle within a row of the screen.
    run(vm, "update check", &[&format!("firmware {set} is offered, waiting for its wave")])?;
    run(vm, "sysupdate firmware stage; echo \"stage $?\"", &[
        &format!("firmware {set}: it reaches machines in waves; this machine's opens on day 7"),
        "stage 1",
    ])?;
    // A canary takes both on their first day.
    run(
        vm,
        &format!("echo 'canary = true' >> {settings}/updates.toml && sysupdate check"),
        &[
            &format!("sysupdate: the stable channel offers Dukovo {year}; this system runs Dukovo {VERSION}"),
            &format!("Dukovo {year} updates only from {bridge} on: Dukovo {bridge} comes first"),
            &format!("Dukovo {bridge} needs {bridge_files} files this system lacks"),
        ],
    )?;
    run(vm, "update check", &[&format!(
        "firmware {set} is offered; this system runs no firmware set"
    )])?;

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
    vm.expect(&format!("sysupdate: Rescue is being written from Dukovo {bridge}"))?;
    run(
        vm,
        &format!(
            "while service sysupdate-stage status > /dev/null; do sleep 2; done; echo \"written \
             $(log sysupdate-stage | grep -c 'holds Dukovo {bridge} now')\"; sysupdate status"
        ),
        &["written 1", &format!("Rescue: Dukovo {bridge}")],
    )?;
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
    // Staged, as the update command does, through the sysupdate-stage
    // service, which holds the root's system subvolumes and no home folder
    // (ADR 0473); then the release keys withdraw it, and the server
    // publishes that: updated's next check unstages it.
    run(vm, "update apply", &[
        &format!("sysupdate: staged Dukovo {year} in slot A: it starts at the next restart"),
        &format!("update: Dukovo {year} is staged: it starts at the next restart you choose"),
    ])?;
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
    // The release keys signed an advisory with the withdrawal, which the
    // withdrawn release fixes (ADR 0534): the bridge has its flaw, and no
    // channel offers the fix. Only the advisories are fetched to say so.
    run(vm, "update advisories; echo \"advisories $?\"", &[
        &format!("Dukovo: {TEST_ADVISORY}, high: "),
        &format!("fixed in Dukovo {year}; this system"),
        "advisories 1",
    ])?;

    // Last, the machine, which runs the bridge on the beta channel now,
    // enrols as a canary and reports to the update server's receiver (ADR
    // 0533). After the checks above, so that nothing of theirs depends on
    // what the enrolment changes in updates.toml; the relay ends with the
    // step.
    let reported = canary_report(vm, &server, &dir, &bridge, "beta");
    server.relay(None);
    reported?;
    power_off(vm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn screen(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|row| (*row).to_string()).collect()
    }

    #[test]
    fn the_key_is_read_from_the_line_enrol_prints_whole_or_wrapped() {
        let line = format!("./ob release canary add {CANARY} {KEY}");
        let whole = screen(&[
            "# sysupdate canary enrol boot-canary",
            "sysupdate: this machine",
            &line,
            "#",
        ]);
        assert_eq!(enrolled_key(&whole).unwrap(), KEY);
        let (head, tail) = KEY.split_at(40);
        let wrapped = screen(&[&format!("./ob release canary add {CANARY} {head}"), tail, "#"]);
        assert_eq!(enrolled_key(&wrapped).unwrap(), KEY);
        // The command typed shows the name too, but not the line.
        let typed = screen(&["# sysupdate canary enrol boot-canary", "#"]);
        assert!(enrolled_key(&typed).is_err());
        let short = screen(&[&format!("./ob release canary add {CANARY} abcd"), "#"]);
        assert!(enrolled_key(&short).is_err());
    }

    fn shown(reasons: &[&str], report: &Value) -> Value {
        json!({"canaries": [{"name": CANARY, "green": reasons.is_empty(), "reasons": reasons,
            "reported": 1_800_000_000, "report": report}], "red": []})
    }

    #[test]
    fn a_canary_that_reported_what_runs_is_accepted_green_or_not() {
        let report = json!({"canary": CANARY, "running": "0.16.0-test.bridge", "channel": "beta"});
        let ok = |reasons: &[&str]| {
            check_standing(&shown(reasons, &report), "0.16.0-test.bridge", "beta")
        };
        assert_eq!(ok(&[]).unwrap(), "none");
        assert_eq!(
            ok(&["failed services: x", "dukovo-check found 1 failures"])
                .unwrap()
                .matches("; ")
                .count(),
            1
        );
        for unheard in ["no report since it was added", "silent since 2026-11-15T09:12:00Z"] {
            assert!(ok(&[unheard]).is_err(), "{unheard}");
        }
        assert!(check_standing(&shown(&[], &report), "0.16.0-test.year", "beta").is_err());
        assert!(check_standing(&shown(&[], &report), "0.16.0-test.bridge", "stable").is_err());
        let none =
            json!({"canaries": [{"name": CANARY, "green": false, "reasons": [], "added": 1}]});
        assert!(
            check_standing(&none, "0.16.0-test.bridge", "beta")
                .unwrap_err()
                .contains("no `reported`")
        );
        assert!(check_standing(&json!({"canaries": []}), "x", "y").is_err());
    }

    /// A receiver at `socket` that answers 200 to the body `a report` and
    /// 400 to any other, and sends each request it read to the channel.
    fn receiver_at(socket: &Path) -> mpsc::Receiver<Vec<u8>> {
        let listener = UnixListener::bind(socket).unwrap();
        let (sent, seen) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let (mut request, mut chunk) = (Vec::new(), [0_u8; 1024]);
                let wanted = loop {
                    let read = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&request[..end]).into_owned();
                        let length = head.split_once("Content-Length: ").unwrap().1;
                        let length: usize = length.split("\r\n").next().unwrap().parse().unwrap();
                        break end + 4 + length;
                    }
                };
                while request.len() < wanted {
                    let read = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..read]);
                }
                let kept = request.ends_with(b"\r\n\r\na report");
                sent.send(request).unwrap();
                let (status, body) = if kept {
                    ("200 OK", "{\"kept\":true}\n")
                } else {
                    ("400 Bad Request", "{\"error\":\"no\"}\n")
                };
                let answer = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(answer.as_bytes()).unwrap();
            }
        });
        seen
    }

    #[test]
    fn the_server_passes_a_report_on_to_the_receiver_and_its_answer_back() {
        let socket =
            std::env::temp_dir().join(format!("update-online-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let seen = receiver_at(&socket);
        let server = Server::start(Path::new(".")).unwrap();

        // With no receiver the path is not here, as before.
        assert_eq!(post_report(server.port, b"a report").unwrap().0, 404);
        server.relay(Some(&socket));
        assert_eq!(post_report(server.port, b"a report").unwrap(), (200, "{\"kept\":true}".into()));
        let sent = String::from_utf8(seen.recv().unwrap()).unwrap();
        assert_eq!(
            sent,
            "POST /canary/v1/report HTTP/1.1\r\nHost: updates\r\nContent-Length: 8\r\nConnection: close\r\n\r\na report"
        );
        // What the receiver refuses is answered as it says.
        assert_eq!(
            post_report(server.port, b"garbage").unwrap(),
            (400, "{\"error\":\"no\"}".into())
        );
        seen.recv().unwrap();
        // A body of 16 KiB goes on, one byte more does not.
        assert_eq!(post_report(server.port, &vec![b' '; REPORT_MAX]).unwrap().0, 400);
        let sent = seen.recv().unwrap();
        assert!(sent.starts_with(b"POST /canary/v1/report HTTP/1.1\r\n"));
        assert!(sent.ends_with(&vec![b' '; REPORT_MAX]));
        assert_eq!(post_report(server.port, &vec![b' '; REPORT_MAX + 1]).unwrap().0, 413);
        assert!(seen.try_recv().is_err());
        // A receiver that is gone is a bad gateway, said on the screen.
        let _ = std::fs::remove_file(&socket);
        let (status, words) = post_report(server.port, b"a report").unwrap();
        assert_eq!(status, 502, "{words}");
        server.relay(None);
        assert_eq!(post_report(server.port, b"a report").unwrap().0, 404);
        let _ = std::fs::remove_file(&socket);
    }
}
