// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! metricsd(8)'s Prometheus endpoint (ADR 0484): `GET /metrics` on the
//! addresses metrics.toml(5) names, answered in Prometheus's text format
//! 0.0.4, with node_exporter's names for what a machine has and
//! `dukovo_` ones for what only Dukovo has.
//!
//! The values are read when they are asked for, not taken from the
//! history, so that a scrape sees the machine as it is and counters count
//! from boot. One thread answers one connection at a time. Only addresses
//! in `allow` are answered, and the others are closed before a byte is
//! read. A request has [`HEAD_TIME`] to arrive and [`HEAD_MAX`] bytes to
//! fit in; the answer closes the connection. The endpoint serves plain
//! HTTP, for a network the owner trusts: TLS and passwords are not
//! there, and the setting that opens it says who may ask.
//!
//! An address that cannot be bound yet, because the card has not got its
//! address, is tried again every [`BIND_EVERY`], and said once.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::sample::{Disk, Filesystem, Network as Card, Service, Snapshot, Volume};
use super::settings::Network;
use crate::alerts::protocol::{Active, End, Refusal, Request, Severity};
use crate::deadline::Deadline;
use crate::util;

/// The longest a request's head may be, and how long it has to arrive.
pub const HEAD_MAX: usize = 8 * 1024;
pub const HEAD_TIME: Duration = Duration::from_secs(5);
/// How long an answer has to be taken by the asker.
const ANSWER_TIME: Duration = Duration::from_secs(5);
/// How often an address that could not be bound is tried again.
pub const BIND_EVERY: Duration = Duration::from_secs(10);
/// How long the thread waits for a connection before it looks at
/// whether it is to stop.
const WATCH: Duration = Duration::from_millis(500);

/// The type of the answer.
const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// What alertd's `list` is given to answer in, and the most of its answer
/// that is read.
const ALERTD_LINES_MAX: usize = 10_000;

/// A way to make the text of a scrape.
pub type Scrape = Arc<dyn Fn() -> String + Send + Sync>;

/// Who is answered, where, and how patiently.
#[derive(Clone)]
pub struct Config {
    pub listen: Vec<SocketAddr>,
    pub allow: Vec<Network>,
    pub head_time: Duration,
    pub bind_every: Duration,
}

impl Config {
    pub fn new(listen: Vec<SocketAddr>, allow: Vec<Network>) -> Config {
        Config { listen, allow, head_time: HEAD_TIME, bind_every: BIND_EVERY }
    }
}

/// The thread that answers, which runs until it is stopped.
pub struct Endpoint {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Endpoint {
    /// Starts answering on the addresses of `config` with what `scrape`
    /// makes.
    pub fn start(config: Config, scrape: Scrape) -> Endpoint {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::Builder::new()
            .name("prometheus".into())
            .spawn(move || serve(&config, &scrape, &flag))
            .map_err(|error| {
                eprintln!(
                    "<3>metricsd: cannot start the Prometheus endpoint's thread: {}",
                    util::strip_os_error(&error)
                );
            })
            .ok();
        Endpoint { stop, thread }
    }

    /// Stops answering and lets go of the addresses, which a new endpoint
    /// may then bind.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // A thread that panicked has nothing left to say.
            let _ = thread.join();
        }
    }
}

/// The thread: binds the addresses, as they become possible, and answers
/// on them until `stop`.
fn serve(config: &Config, scrape: &Scrape, stop: &AtomicBool) {
    let mut listeners: Vec<TcpListener> = Vec::new();
    let mut waiting: Vec<SocketAddr> = config.listen.clone();
    let mut tried: Option<Instant> = None;
    let mut said: Vec<SocketAddr> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        if !waiting.is_empty() && tried.is_none_or(|at| at.elapsed() >= config.bind_every) {
            tried = Some(Instant::now());
            bind_waiting(&mut waiting, &mut listeners, &mut said);
        }
        for listener in ready(&listeners) {
            // Another connection may have been taken since, and an
            // accept that failed is another's to try.
            if let Ok((stream, peer)) = listener.accept() {
                answer(stream, peer, config, scrape);
            }
        }
    }
}

/// Binds each address of `waiting` that it can, saying the first time
/// one cannot.
fn bind_waiting(
    waiting: &mut Vec<SocketAddr>,
    listeners: &mut Vec<TcpListener>,
    said: &mut Vec<SocketAddr>,
) {
    waiting.retain(|address| match TcpListener::bind(address) {
        Ok(listener) => {
            // Not blocking, so that a connection that went away between
            // the poll and the accept does not hold the thread.
            if let Err(error) = listener.set_nonblocking(true) {
                eprintln!(
                    "<3>metricsd: cannot use {address} for Prometheus: {}",
                    util::strip_os_error(&error)
                );
                return false;
            }
            println!("metricsd: answering Prometheus on http://{address}/metrics");
            listeners.push(listener);
            false
        }
        Err(error) => {
            if !said.contains(address) {
                said.push(*address);
                eprintln!(
                    "<3>metricsd: cannot listen on {address} for Prometheus: {}; trying again \
                     every {} seconds",
                    util::strip_os_error(&error),
                    BIND_EVERY.as_secs()
                );
            }
            true
        }
    });
}

/// The listeners with a connection to accept, after waiting up to half a
/// second for one; with no listener it only waits.
fn ready(listeners: &[TcpListener]) -> Vec<&TcpListener> {
    if listeners.is_empty() {
        thread::sleep(WATCH);
        return Vec::new();
    }
    let mut fds: Vec<libc::pollfd> = listeners
        .iter()
        .map(|listener| libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 })
        .collect();
    // SAFETY: `fds` is a valid array of its length for the call.
    let found = unsafe {
        libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, WATCH.as_millis() as libc::c_int)
    };
    if found <= 0 {
        return Vec::new();
    }
    listeners
        .iter()
        .zip(&fds)
        .filter(|(_, fd)| fd.revents & libc::POLLIN != 0)
        .map(|(l, _)| l)
        .collect()
}

/// Answers the connection `stream` from `peer`, or closes it at once if
/// the peer is not in `allow`.
fn answer(mut stream: TcpStream, peer: SocketAddr, config: &Config, scrape: &Scrape) {
    if !config.allow.iter().any(|network| network.contains(peer.ip())) {
        return;
    }
    // A connection that was accepted without blocking may still be
    // inheriting that on some systems.
    if stream.set_nonblocking(false).is_err()
        || stream.set_write_timeout(Some(ANSWER_TIME)).is_err()
    {
        return;
    }
    let bytes = match read_head(&mut stream, config.head_time) {
        Ok(head) => reply(&head, scrape),
        Err(Head::TooBig) => {
            text_response(431, "Request Header Fields Too Large", &[], "the request is too long\n")
        }
        Err(Head::Gone) => return,
    };
    // An asker that went away has nothing to be told.
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

/// Why a request's head was not read.
#[derive(Debug, PartialEq, Eq)]
enum Head {
    /// It is longer than [`HEAD_MAX`].
    TooBig,
    /// The asker closed the connection, did not finish in time, or the
    /// connection failed.
    Gone,
}

/// The end of the head in `bytes`, after its blank line, if it is in
/// them.
fn head_end(bytes: &[u8]) -> Option<usize> {
    let crlf = bytes.windows(4).position(|w| w == b"\r\n\r\n").map(|at| at + 4);
    let lf = bytes.windows(2).position(|w| w == b"\n\n").map(|at| at + 2);
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Reads a request's head from `stream`: all of it must arrive within
/// `time`, not only each piece of it, so that a slow asker cannot hold the
/// thread.
fn read_head(stream: &mut TcpStream, time: Duration) -> Result<Vec<u8>, Head> {
    let deadline = Instant::now() + time;
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return Err(Head::Gone);
        }
        let count = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(Head::Gone),
            Ok(count) => count,
        };
        head.extend_from_slice(&chunk[..count]);
        match head_end(&head) {
            Some(end) if end <= HEAD_MAX => {
                head.truncate(end);
                return Ok(head);
            }
            Some(_) => return Err(Head::TooBig),
            None if head.len() >= HEAD_MAX => return Err(Head::TooBig),
            None => {}
        }
    }
}

/// What a request asks for.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Metrics,
    NotFound,
    WrongMethod,
    Bad,
}

/// What the request line of `head` asks for: `GET /metrics`, with or
/// without a query. Any other path is not found, any other method on it
/// not allowed.
fn route(head: &[u8]) -> Route {
    let line = head.split(|&b| b == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let Ok(line) = std::str::from_utf8(line) else { return Route::Bad };
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Route::Bad;
    };
    if !version.starts_with("HTTP/") || method.is_empty() {
        return Route::Bad;
    }
    if target.split(['?', '#']).next() != Some("/metrics") {
        Route::NotFound
    } else if method == "GET" {
        Route::Metrics
    } else {
        Route::WrongMethod
    }
}

/// The whole answer to the request `head`, which `scrape` makes the text
/// of when it is a scrape.
fn reply(head: &[u8], scrape: &Scrape) -> Vec<u8> {
    match route(head) {
        Route::Metrics => {
            let body = scrape();
            let mut answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {CONTENT_TYPE}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            answer.extend_from_slice(body.as_bytes());
            answer
        }
        Route::NotFound => text_response(404, "Not Found", &[], "only /metrics is here\n"),
        Route::WrongMethod => {
            text_response(405, "Method Not Allowed", &[("Allow", "GET")], "only GET is answered\n")
        }
        Route::Bad => text_response(400, "Bad Request", &[], "that is not an HTTP request\n"),
    }
}

/// A plain text answer with `status`.
fn text_response(status: u16, reason: &str, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut answer = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        // Writing to a string cannot fail.
        let _ = write!(answer, "{name}: {value}\r\n");
    }
    answer.push_str("\r\n");
    answer.push_str(body);
    answer.into_bytes()
}

/// How many alerts are active, as alertd's `list` says: warnings, then
/// critical ones. None when alertd is not there, does not answer in
/// `patience`, refuses or answers with something else.
pub fn active_counts(socket: &Path, patience: Duration) -> Option<(u64, u64)> {
    let stream = UnixStream::connect(socket).ok()?;
    let mut timed = Deadline::new(&stream, patience).ok()?;
    let mut request =
        serde_json::to_string(&Request { ask: "list".into(), ..Request::default() }).ok()?;
    request.push('\n');
    timed.write_all(request.as_bytes()).ok()?;
    let (mut warnings, mut critical) = (0, 0);
    for line in BufReader::new(timed.take(1 << 22)).lines().take(ALERTD_LINES_MAX) {
        let line = line.ok()?;
        if serde_json::from_str::<End>(&line).is_ok_and(|end| end.end) {
            return Some((warnings, critical));
        }
        if serde_json::from_str::<Refusal>(&line).is_ok() {
            return None;
        }
        match serde_json::from_str::<Active>(&line).ok()?.alert.severity {
            Severity::Warning => warnings += 1,
            Severity::Critical => critical += 1,
        }
    }
    // An answer that was cut short is not to be taken for a count.
    None
}

/// What names the machine in `node_uname_info`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    pub release: String,
    pub architecture: String,
    pub nodename: String,
}

impl Machine {
    /// This machine's.
    pub fn read() -> Machine {
        let (_, release, architecture) = util::uname();
        Machine { release, architecture, nodename: util::hostname() }
    }
}

/// A sample of a metric: its labels and its value.
struct Sample {
    labels: Vec<(&'static str, String)>,
    value: f64,
}

fn sample(labels: &[(&'static str, &str)], value: f64) -> Sample {
    Sample {
        labels: labels.iter().map(|&(name, value)| (name, value.to_string())).collect(),
        value,
    }
}

/// A label's value as the text format writes it: a backslash, a double
/// quote and a line feed are escaped.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// A number as the text format writes it.
fn number(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 { "+Inf".into() } else { "-Inf".into() }
    } else {
        value.to_string()
    }
}

/// The text of a metric and its samples, none if it has none: a help
/// line, a type line, then a line for each sample.
fn family(out: &mut String, name: &str, kind: &str, help: &str, samples: &[Sample]) {
    if samples.is_empty() {
        return;
    }
    // Writing to a string cannot fail.
    let _ = writeln!(out, "# HELP {name} {}", help.replace('\\', "\\\\").replace('\n', "\\n"));
    let _ = writeln!(out, "# TYPE {name} {kind}");
    for sample in samples {
        out.push_str(name);
        if !sample.labels.is_empty() {
            let labels: Vec<String> = sample
                .labels
                .iter()
                .map(|(label, value)| format!("{label}=\"{}\"", escape(value)))
                .collect();
            let _ = write!(out, "{{{}}}", labels.join(","));
        }
        let _ = writeln!(out, " {}", number(sample.value));
    }
}

/// One family with one sample for each of `items`, a label and a value
/// each.
fn per<T>(
    items: &[T],
    labels: impl Fn(&T) -> Vec<(&'static str, String)>,
    value: impl Fn(&T) -> f64,
) -> Vec<Sample> {
    items.iter().map(|item| Sample { labels: labels(item), value: value(item) }).collect()
}

/// Gauge or counter, for a family's type line.
const GAUGE: &str = "gauge";
const COUNTER: &str = "counter";

/// The text of a scrape: `snapshot` as node_exporter's metrics and
/// Dukovo's own, at `now` seconds since 1970. `alerts` are the warnings
/// and critical alerts active, left out when alertd did not say.
pub fn exposition(
    snapshot: &Snapshot,
    machine: &Machine,
    alerts: Option<(u64, u64)>,
    now: f64,
) -> String {
    let mut out = String::new();
    system_metrics(&mut out, snapshot, machine, now);
    storage_metrics(&mut out, snapshot);
    network_metrics(&mut out, snapshot);
    dukovo_metrics(&mut out, snapshot, alerts);
    out
}

/// The processors, load, memory, sensors and clocks.
fn system_metrics(out: &mut String, snapshot: &Snapshot, machine: &Machine, now: f64) {
    let seconds = |ns: u64| ns as f64 / 1e9;
    let mut cpu = Vec::new();
    for (index, times) in &snapshot.cpus {
        let index = index.to_string();
        for (mode, ns) in
            [("idle", times.idle_ns), ("system", times.system_ns), ("user", times.user_ns)]
        {
            cpu.push(sample(&[("cpu", &index), ("mode", mode)], seconds(ns)));
        }
    }
    family(
        out,
        "node_cpu_seconds_total",
        COUNTER,
        "Seconds the processors spent in each mode.",
        &cpu,
    );
    for (name, help, value) in [
        ("node_load1", "Load average over 1 minute.", snapshot.load[0]),
        ("node_load5", "Load average over 5 minutes.", snapshot.load[1]),
        ("node_load15", "Load average over 15 minutes.", snapshot.load[2]),
    ] {
        family(out, name, GAUGE, help, &[sample(&[], value)]);
    }
    let memory = &snapshot.memory;
    let bytes = |kib: u64| (kib * 1024) as f64;
    if memory.total > 0 {
        for (name, help, kib) in [
            ("node_memory_MemTotal_bytes", "Memory in the machine.", memory.total),
            (
                "node_memory_MemAvailable_bytes",
                "Memory programs can still have, the cache included.",
                memory.available(),
            ),
            ("node_memory_SwapTotal_bytes", "Swap space there is.", memory.swap_total),
            ("node_memory_SwapFree_bytes", "Swap space that is free.", memory.swap_free),
        ] {
            family(out, name, GAUGE, help, &[sample(&[], bytes(kib))]);
        }
    }
    let temperatures = per(
        &snapshot.temperatures,
        |t| vec![("chip", t.chip.clone()), ("sensor", t.sensor.clone())],
        |t| t.celsius,
    );
    family(
        out,
        "node_hwmon_temp_celsius",
        GAUGE,
        "Temperature of a hardware monitor's sensor.",
        &temperatures,
    );
    if snapshot.boot_time_ns > 0 {
        let boot = sample(&[], seconds(snapshot.boot_time_ns));
        family(
            out,
            "node_boot_time_seconds",
            GAUGE,
            "When the system started, in seconds since 1970.",
            &[boot],
        );
    }
    family(
        out,
        "node_time_seconds",
        GAUGE,
        "The system's time, in seconds since 1970.",
        &[sample(&[], now)],
    );
    let uname = sample(
        &[
            ("sysname", "Dukovo"),
            ("release", &machine.release),
            ("machine", &machine.architecture),
            ("nodename", &machine.nodename),
        ],
        1.0,
    );
    family(out, "node_uname_info", GAUGE, "What uname says of the machine.", &[uname]);
}

/// The filesystems and disks.
fn storage_metrics(out: &mut String, snapshot: &Snapshot) {
    let labels = |f: &Filesystem| {
        vec![
            ("device", f.device.clone()),
            ("fstype", f.fstype.clone()),
            ("mountpoint", f.mount.clone()),
        ]
    };
    type Size = fn(&Filesystem) -> u64;
    let sizes: [(&str, &str, Size); 3] = [
        ("node_filesystem_size_bytes", "Size of the filesystem.", |f| f.size),
        ("node_filesystem_free_bytes", "Space free on the filesystem.", |f| f.free),
        ("node_filesystem_avail_bytes", "Space free for programs that are not root's.", |f| {
            f.available
        }),
    ];
    for (name, help, read) in sizes {
        family(out, name, GAUGE, help, &per(&snapshot.filesystems, labels, |f| read(f) as f64));
    }
    let disks = &snapshot.disks;
    let device = |d: &Disk| vec![("device", d.name.clone())];
    type Read = fn(&Disk) -> f64;
    let counters: [(&str, &str, Read); 5] = [
        ("node_disk_read_bytes_total", "Bytes read from the disk.", |d| {
            (d.read_sectors * 512) as f64
        }),
        ("node_disk_written_bytes_total", "Bytes written to the disk.", |d| {
            (d.written_sectors * 512) as f64
        }),
        ("node_disk_reads_completed_total", "Reads the disk completed.", |d| d.reads as f64),
        ("node_disk_writes_completed_total", "Writes the disk completed.", |d| d.writes as f64),
        ("node_disk_io_time_seconds_total", "Seconds the disk had a request in flight.", |d| {
            d.busy_ms as f64 / 1000.0
        }),
    ];
    for (name, help, value) in counters {
        family(out, name, COUNTER, help, &per(disks, device, value));
    }
}

/// The network cards.
fn network_metrics(out: &mut String, snapshot: &Snapshot) {
    let device = |n: &Card| vec![("device", n.name.clone())];
    type Read = fn(&Card) -> f64;
    let counters: [(&str, &str, Read); 4] = [
        ("node_network_receive_bytes_total", "Bytes the card received.", |n| n.rx_bytes as f64),
        ("node_network_transmit_bytes_total", "Bytes the card sent.", |n| n.tx_bytes as f64),
        ("node_network_receive_errs_total", "Receive errors of the card.", |n| n.rx_errors as f64),
        ("node_network_transmit_errs_total", "Send errors of the card.", |n| n.tx_errors as f64),
    ];
    for (name, help, value) in counters {
        family(out, name, COUNTER, help, &per(&snapshot.networks, device, value));
    }
}

/// What only Dukovo has: the services' use, Btrfs volumes' disks, and
/// the active alerts.
fn dukovo_metrics(out: &mut String, snapshot: &Snapshot, alerts: Option<(u64, u64)>) {
    let service = |s: &Service| vec![("service", s.name.clone())];
    family(
        out,
        "dukovo_service_cpu_seconds_total",
        COUNTER,
        "Seconds of processor time a service's processes have used.",
        &per(&snapshot.services, service, |s| s.cpu_ns as f64 / 1e9),
    );
    family(
        out,
        "dukovo_service_memory_bytes",
        GAUGE,
        "Memory a service's processes hold.",
        &per(&snapshot.services, service, |s| s.memory_bytes as f64),
    );
    let volume = |v: &Volume| vec![("volume", v.name().to_string())];
    family(
        out,
        "dukovo_btrfs_disks_missing",
        GAUGE,
        "Disks of a Btrfs volume that are missing or failed.",
        &per(&snapshot.volumes, volume, |v| v.missing as f64),
    );
    family(
        out,
        "dukovo_btrfs_disks_behind",
        GAUGE,
        "Disks of a Btrfs volume that missed changes.",
        &per(&snapshot.volumes, volume, |v| v.behind as f64),
    );
    if let Some((warnings, critical)) = alerts {
        let active = [
            sample(&[("severity", "warning")], warnings as f64),
            sample(&[("severity", "critical")], critical as f64),
        ];
        family(out, "dukovo_alerts_active", GAUGE, "Alerts that are active, by severity.", &active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metricsd::sample::Temperature;
    use crate::proc::{CpuTimes, Memory};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    fn machine() -> Machine {
        Machine {
            release: "0.16.0".into(),
            architecture: "x86_64".into(),
            nodename: "dukovo-1".into(),
        }
    }

    fn fixture() -> Snapshot {
        Snapshot {
            monotonic_ns: 5,
            at: 1_800_000_000,
            boot_time_ns: 1_799_000_000_500_000_000,
            cpus: vec![
                (
                    0,
                    CpuTimes {
                        user_ns: 2_000_000_000,
                        system_ns: 1_000_000_000,
                        idle_ns: 7_000_000_000,
                        switches: 0,
                    },
                ),
                (
                    1,
                    CpuTimes {
                        user_ns: 1_500_000_000,
                        system_ns: 500_000_000,
                        idle_ns: 8_000_000_000,
                        switches: 0,
                    },
                ),
            ],
            cpu: CpuTimes::default(),
            load: [0.5, 0.25, 0.125],
            memory: Memory {
                total: 4096,
                free: 1024,
                cache: 512,
                swap_total: 2048,
                swap_free: 2000,
            },
            disks: vec![Disk {
                name: "vda".into(),
                reads: 10,
                read_sectors: 200,
                writes: 5,
                written_sectors: 100,
                busy_ms: 1500,
            }],
            filesystems: vec![
                Filesystem {
                    device: "vda1".into(),
                    fstype: "btrfs".into(),
                    mount: "/".into(),
                    read_only: false,
                    size: 1000,
                    free: 400,
                    available: 300,
                },
                Filesystem {
                    device: "vdb1".into(),
                    fstype: "vfat".into(),
                    mount: "/volumes/a \"b\"\\c\nd".into(),
                    read_only: false,
                    size: 10,
                    free: 5,
                    available: 5,
                },
            ],
            networks: vec![
                Card { name: "lo".into(), rx_bytes: 1, tx_bytes: 1, ..Card::default() },
                Card {
                    name: "net0".into(),
                    rx_bytes: 600,
                    tx_bytes: 300,
                    rx_errors: 2,
                    tx_errors: 3,
                    ..Card::default()
                },
            ],
            temperatures: vec![Temperature {
                chip: "coretemp-isa-0000".into(),
                sensor: "temp1".into(),
                label: "Package id 0".into(),
                celsius: 45.5,
                max: None,
                crit: None,
            }],
            zones: Vec::new(),
            services: vec![Service {
                name: "metricsd".into(),
                cpu_ns: 1_500_000_000,
                memory_bytes: 8192,
            }],
            volumes: vec![Volume {
                uuid: "u".into(),
                label: "data".into(),
                disks: 2,
                missing: 1,
                behind: 0,
            }],
        }
    }

    /// Checks `text` is what Prometheus reads: each family has its help
    /// and type once, before its samples, which are `name{labels} value`
    /// lines, and its counters end in `_total`.
    fn check_format(text: &str) {
        let mut typed: Vec<String> = Vec::new();
        let mut current = String::new();
        for line in text.lines() {
            if let Some(help) = line.strip_prefix("# HELP ") {
                current = help.split(' ').next().unwrap().to_string();
                assert!(help.len() > current.len() + 1, "no help text: {line}");
            } else if let Some(kind) = line.strip_prefix("# TYPE ") {
                let (name, kind) = kind.split_once(' ').unwrap();
                assert_eq!(name, current, "type line out of place: {line}");
                assert!(!typed.contains(&name.to_string()), "{name} is typed twice");
                assert!(["gauge", "counter"].contains(&kind), "{line}");
                assert_eq!(kind == "counter", name.ends_with("_total"), "{line}");
                typed.push(name.to_string());
            } else {
                let name_end = line.find(['{', ' ']).unwrap();
                assert_eq!(&line[..name_end], current, "sample outside its family: {line}");
                if line.as_bytes()[name_end] == b'{' {
                    let close = line.rfind("} ").unwrap_or_else(|| panic!("no close: {line}"));
                    assert!(
                        line[name_end + 1..close].split("\",").all(|pair| pair.contains("=\"")),
                        "{line}"
                    );
                    line[close + 2..].parse::<f64>().unwrap();
                } else {
                    line[name_end + 1..].parse::<f64>().unwrap();
                }
            }
        }
        assert!(text.ends_with('\n') || text.is_empty());
    }

    #[test]
    fn a_scrape_has_node_exporters_names_and_dukovos_own() {
        let text = exposition(&fixture(), &machine(), Some((2, 1)), 1_800_000_001.25);
        check_format(&text);
        for line in [
            "node_cpu_seconds_total{cpu=\"0\",mode=\"idle\"} 7",
            "node_cpu_seconds_total{cpu=\"1\",mode=\"user\"} 1.5",
            "node_load1 0.5",
            "node_load5 0.25",
            "node_load15 0.125",
            "node_memory_MemTotal_bytes 4194304",
            "node_memory_MemAvailable_bytes 1572864",
            "node_memory_SwapTotal_bytes 2097152",
            "node_memory_SwapFree_bytes 2048000",
            "node_filesystem_size_bytes{device=\"vda1\",fstype=\"btrfs\",mountpoint=\"/\"} 1000",
            "node_filesystem_free_bytes{device=\"vda1\",fstype=\"btrfs\",mountpoint=\"/\"} 400",
            "node_filesystem_avail_bytes{device=\"vda1\",fstype=\"btrfs\",mountpoint=\"/\"} 300",
            "node_disk_read_bytes_total{device=\"vda\"} 102400",
            "node_disk_written_bytes_total{device=\"vda\"} 51200",
            "node_disk_reads_completed_total{device=\"vda\"} 10",
            "node_disk_writes_completed_total{device=\"vda\"} 5",
            "node_disk_io_time_seconds_total{device=\"vda\"} 1.5",
            "node_network_receive_bytes_total{device=\"net0\"} 600",
            "node_network_transmit_bytes_total{device=\"net0\"} 300",
            "node_network_receive_errs_total{device=\"net0\"} 2",
            "node_network_transmit_errs_total{device=\"net0\"} 3",
            "node_network_receive_bytes_total{device=\"lo\"} 1",
            "node_hwmon_temp_celsius{chip=\"coretemp-isa-0000\",sensor=\"temp1\"} 45.5",
            "node_boot_time_seconds 1799000000.5",
            "node_time_seconds 1800000001.25",
            "node_uname_info{sysname=\"Dukovo\",release=\"0.16.0\",machine=\"x86_64\",nodename=\"dukovo-1\"} 1",
            "dukovo_service_cpu_seconds_total{service=\"metricsd\"} 1.5",
            "dukovo_service_memory_bytes{service=\"metricsd\"} 8192",
            "dukovo_btrfs_disks_missing{volume=\"data\"} 1",
            "dukovo_btrfs_disks_behind{volume=\"data\"} 0",
            "dukovo_alerts_active{severity=\"warning\"} 2",
            "dukovo_alerts_active{severity=\"critical\"} 1",
        ] {
            assert!(text.lines().any(|l| l == line), "{line}\n{text}");
        }
    }

    #[test]
    fn label_values_are_escaped_and_the_help_is_one_line() {
        let text = exposition(&fixture(), &machine(), None, 1.0);
        check_format(&text);
        assert!(
            text.contains(
                "node_filesystem_size_bytes{device=\"vdb1\",fstype=\"vfat\",mountpoint=\"/volumes/a \\\"b\\\"\\\\c\\nd\"} 10"
            ),
            "{text}"
        );
        // Every line of the text is a line: the newline in the mount point
        // did not break one.
        assert_eq!(text.lines().filter(|l| l.contains("mountpoint=\"/volumes/a")).count(), 3);
        assert_eq!(escape("a\\b\"c\nd"), "a\\\\b\\\"c\\nd");
    }

    #[test]
    fn what_alertd_did_not_say_and_what_is_not_there_is_left_out() {
        let text = exposition(&fixture(), &machine(), None, 1.0);
        assert!(!text.contains("dukovo_alerts_active"));
        let empty = exposition(&Snapshot::default(), &machine(), None, 1.0);
        check_format(&empty);
        for gone in [
            "node_cpu_",
            "node_memory_",
            "node_filesystem_",
            "node_disk_",
            "node_network_",
            "node_hwmon_",
            "dukovo_service_",
            "dukovo_btrfs_",
        ] {
            assert!(!empty.contains(gone), "{gone}");
        }
        assert!(empty.contains("node_load1 0\n") && empty.contains("node_uname_info{"));
        assert_eq!(number(f64::NAN), "NaN");
        assert_eq!(number(f64::INFINITY), "+Inf");
        assert_eq!(number(4_294_967_296.0), "4294967296");
    }

    #[test]
    fn requests_are_routed_by_method_and_path() {
        assert_eq!(route(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n"), Route::Metrics);
        assert_eq!(route(b"GET /metrics?collect%5B%5D=cpu HTTP/1.0\n\n"), Route::Metrics);
        assert_eq!(route(b"GET /other HTTP/1.1\r\n\r\n"), Route::NotFound);
        assert_eq!(route(b"GET / HTTP/1.1\r\n\r\n"), Route::NotFound);
        assert_eq!(route(b"GET /metrics/ HTTP/1.1\r\n\r\n"), Route::NotFound);
        assert_eq!(route(b"POST /metrics HTTP/1.1\r\n\r\n"), Route::WrongMethod);
        assert_eq!(route(b"HEAD /metrics HTTP/1.1\r\n\r\n"), Route::WrongMethod);
        assert_eq!(route(b"DELETE /other HTTP/1.1\r\n\r\n"), Route::NotFound);
        for bad in [
            &b"hello\r\n\r\n"[..],
            b"GET /metrics\r\n\r\n",
            b"GET /metrics FTP/1\r\n\r\n",
            b"\xff\xfe /metrics HTTP/1.1\r\n\r\n",
            b"GET /metrics HTTP/1.1 extra\r\n\r\n",
            b"\r\n\r\n",
        ] {
            assert_eq!(route(bad), Route::Bad, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    fn scrape() -> Scrape {
        Arc::new(|| "node_load1 1\n".to_string())
    }

    fn text_of(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn the_answers_are_http_with_their_lengths() {
        let ok = text_of(&reply(b"GET /metrics HTTP/1.1\r\n\r\n", &scrape()));
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(ok.contains("Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n"));
        assert!(ok.contains("Content-Length: 13\r\n") && ok.contains("Connection: close\r\n"));
        assert!(ok.ends_with("\r\n\r\nnode_load1 1\n"));
        let missing = text_of(&reply(b"GET /x HTTP/1.1\r\n\r\n", &scrape()));
        assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"), "{missing}");
        let wrong = text_of(&reply(b"POST /metrics HTTP/1.1\r\n\r\n", &scrape()));
        assert!(
            wrong.starts_with("HTTP/1.1 405 Method Not Allowed\r\n")
                && wrong.contains("Allow: GET\r\n"),
            "{wrong}"
        );
        let bad = text_of(&reply(b"nonsense\r\n\r\n", &scrape()));
        assert!(bad.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{bad}");
        // A scrape that is not asked for is not made.
        let never: Scrape = Arc::new(|| panic!("not asked for"));
        reply(b"GET /nothing HTTP/1.1\r\n\r\n", &never);
    }

    #[test]
    fn the_head_ends_at_its_blank_line() {
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n\r\nrest"), Some(18));
        assert_eq!(head_end(b"GET / HTTP/1.1\n\n"), Some(16));
        assert_eq!(head_end(b"GET / HTTP/1.1\r\nHost: x\r\n"), None);
    }

    /// An endpoint on a free port of the loopback, answering `allow`, and
    /// its address.
    fn endpoint(allow: &[&str], head_time: Duration) -> (Endpoint, SocketAddr) {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let allow = allow.iter().map(|n| Network::parse(n).unwrap()).collect();
        let config = Config {
            head_time,
            bind_every: Duration::from_millis(100),
            ..Config::new(vec![address], allow)
        };
        (Endpoint::start(config, scrape()), address)
    }

    /// Connects, sends `request` and reads what comes back until the
    /// connection ends.
    fn ask(address: SocketAddr, request: &[u8]) -> String {
        let mut stream = loop {
            // The thread binds as it starts.
            match TcpStream::connect(address) {
                Ok(stream) => break stream,
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = stream.write_all(request);
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        text_of(&answer)
    }

    #[test]
    fn the_endpoint_answers_a_scrape_a_404_and_a_405_and_then_closes() {
        let (endpoint, address) = endpoint(&["127.0.0.0/8"], HEAD_TIME);
        let ok = ask(address, b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n") && ok.ends_with("node_load1 1\n"), "{ok}");
        assert!(ask(address, b"GET /nope HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404 "));
        assert!(ask(address, b"PUT /metrics HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405 "));
        endpoint.stop();
    }

    #[test]
    fn a_head_over_8_kib_is_refused_and_a_slow_one_is_dropped() {
        let (endpoint, address) = endpoint(&["127.0.0.1"], Duration::from_millis(300));
        let mut long = b"GET /metrics HTTP/1.1\r\nX-Long: ".to_vec();
        long.extend(std::iter::repeat_n(b'a', HEAD_MAX));
        long.extend_from_slice(b"\r\n\r\n");
        assert!(ask(address, &long).starts_with("HTTP/1.1 431 "));
        // One that is exactly as long as allowed is taken.
        let mut limit = b"GET /metrics HTTP/1.1\r\nX-Long: ".to_vec();
        limit.extend(std::iter::repeat_n(b'a', HEAD_MAX - limit.len() - 4));
        limit.extend_from_slice(b"\r\n\r\n");
        assert_eq!(limit.len(), HEAD_MAX);
        assert!(ask(address, &limit).starts_with("HTTP/1.1 200 "));
        // A head that never ends is closed without an answer, when its
        // time is out, and the thread is free for the next.
        let started = Instant::now();
        assert_eq!(ask(address, b"GET /metrics HTTP/1.1\r\n"), "");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(ask(address, b"GET /metrics HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 200 "));
        endpoint.stop();
    }

    #[test]
    fn an_address_outside_allow_is_closed_before_a_byte_is_read() {
        let (endpoint, address) = endpoint(&["10.0.0.0/8"], HEAD_TIME);
        assert_eq!(ask(address, b"GET /metrics HTTP/1.1\r\n\r\n"), "");
        endpoint.stop();
    }

    #[test]
    fn an_address_that_cannot_be_bound_yet_is_tried_again() {
        // Held by another listener for now.
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = held.local_addr().unwrap();
        let config = Config {
            bind_every: Duration::from_millis(100),
            ..Config::new(vec![address], vec![Network::parse("127.0.0.1").unwrap()])
        };
        let endpoint = Endpoint::start(config, scrape());
        thread::sleep(Duration::from_millis(300));
        drop(held);
        let ok = ask(address, b"GET /metrics HTTP/1.1\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 "), "{ok}");
        endpoint.stop();
        // Stopped, it lets go of the address.
        assert!(TcpListener::bind(address).is_ok());
    }

    /// An alertd that answers `lines` and then closes, at a socket of the
    /// test's own.
    fn alertd(name: &str, lines: Vec<String>) -> (PathBuf, JoinHandle<String>) {
        alertd_holding(name, lines, Duration::ZERO)
    }

    /// As [`alertd`], but it keeps the connection open for `hold` after
    /// its lines.
    fn alertd_holding(
        name: &str,
        lines: Vec<String>,
        hold: Duration,
    ) -> (PathBuf, JoinHandle<String>) {
        let dir =
            std::env::temp_dir().join(format!("metricsd-prometheus-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("socket");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
            let mut stream = stream;
            for line in lines {
                let _ = writeln!(stream, "{line}");
            }
            thread::sleep(hold);
            request
        });
        (path, server)
    }

    fn active(key: &str, severity: &str) -> String {
        format!(
            "{{\"source\":\"x\",\"alert\":{{\"key\":\"{key}\",\"severity\":\"{severity}\",\"title\":\"t\"}},\"since\":1,\"changed\":1}}"
        )
    }

    #[test]
    fn the_active_alerts_are_counted_from_alertds_list() {
        let (path, server) = alertd(
            "list",
            vec![
                active("a", "warning"),
                active("b", "critical"),
                active("c", "warning"),
                "{\"end\":true}".into(),
            ],
        );
        assert_eq!(active_counts(&path, Duration::from_secs(2)), Some((2, 1)));
        assert_eq!(server.join().unwrap().trim_end(), "{\"ask\":\"list\"}");
        let (path, server) = alertd("none", vec!["{\"end\":true}".into()]);
        assert_eq!(active_counts(&path, Duration::from_secs(2)), Some((0, 0)));
        server.join().unwrap();
    }

    #[test]
    fn an_alertd_that_refuses_is_cut_short_or_is_not_there_gives_no_count() {
        let (path, server) = alertd("refuses", vec!["{\"error\":\"only root may ask\"}".into()]);
        assert_eq!(active_counts(&path, Duration::from_secs(2)), None);
        server.join().unwrap();
        // An answer that stops before its end is no count.
        let (path, server) = alertd("cut", vec![active("a", "warning")]);
        assert_eq!(active_counts(&path, Duration::from_secs(2)), None);
        server.join().unwrap();
        let (path, server) = alertd("odd", vec!["hello".into()]);
        assert_eq!(active_counts(&path, Duration::from_secs(2)), None);
        server.join().unwrap();
        assert_eq!(active_counts(Path::new("/nonexistent/socket"), Duration::from_secs(1)), None);
    }

    #[test]
    fn an_alertd_that_does_not_answer_in_time_gives_no_count() {
        let (path, server) = alertd_holding("silent", Vec::new(), Duration::from_millis(1500));
        let started = Instant::now();
        assert_eq!(active_counts(&path, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}
