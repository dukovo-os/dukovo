// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The update server's canary flow (ADR 0533), on the server's own
//! scripts, run on the host in a folder that stands in for the server's
//! `/` (`DUKOVO_SERVER_ROOT`, as the boot tests' update server is): the
//! receiver, `system-release canary-receive`, listens on a socket there;
//! canaries are added with `canaries.sh`; sealed reports are posted
//! through the socket; and `system-canaries.sh` holds the waves of a
//! release or frees them. What is signed is read back as a computer reads
//! it, from the first root. The times the canaries are judged at are
//! given with `--now`, so that nothing waits.

#![forbid(unsafe_code)]

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use system_release::canary::{Check, Health, Key, Report};
use system_release::rollout::Plans;
use system_release::{cli, updates};

/// tools/update-server, whose scripts these are.
const SCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../tools/update-server");

/// The program the server runs.
const PROGRAM: &str = env!("CARGO_BIN_EXE_system-release");

/// The release the server offers on its stable channel.
const VERSION: &str = "0.16.0";

const NAME: &str = "ref-box";

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_string()).collect()
}

fn run(words: &[&str]) {
    run_all(&strings(words));
}

fn run_all(words: &[String]) {
    assert_eq!(cli::run(words), 0, "system-release {words:?}");
}

/// Stops the receiver when the test ends, however it ends.
struct Receiver(Child);

impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The server: a folder standing in for its `/`.
struct Server {
    root: PathBuf,
    keys: PathBuf,
    socket: PathBuf,
}

impl Server {
    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    fn lib(&self) -> PathBuf {
        self.path("usr/local/lib/dukovo-system")
    }

    /// Runs the server's script `name` with `args`.
    fn script(&self, name: &str, args: &[&str]) -> Output {
        Command::new("sh")
            .arg(self.lib().join(name))
            .args(args)
            .env("DUKOVO_SERVER_ROOT", &self.root)
            .env("DUKOVO_LOCK", self.path("run/lock"))
            .output()
            .unwrap()
    }

    /// Runs it and gives what it said, which it must have succeeded in.
    fn ok(&self, name: &str, args: &[&str]) -> String {
        let output = self.script(name, args);
        assert!(
            output.status.success(),
            "{name} {args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The folder the server serves now.
    fn served(&self) -> PathBuf {
        std::fs::canonicalize(self.path("srv/dukovo-system/system")).unwrap()
    }

    /// The signed timestamp the server serves, which changes with every
    /// signing.
    fn timestamp(&self) -> Vec<u8> {
        std::fs::read(self.served().join("metadata/timestamp.json")).unwrap()
    }

    /// The highest wave open of the release and why no further one is,
    /// read back as a computer reads it, from the first root.
    fn rollout(&self) -> (u8, Option<String>) {
        let base = tuf::Url::parse(&format!("file://{}/", self.served().display())).unwrap();
        let root = std::fs::read(self.keys.join("root.json")).unwrap();
        let status = updates::status(&base, &root).unwrap();
        let rollout = status.releases.iter().find(|r| r.version == VERSION).unwrap();
        let rollout = rollout.rollout.as_ref().expect("the release has a rollout");
        (rollout.wave, rollout.held.clone())
    }

    fn plans(&self) -> Plans {
        Plans::load(&self.path("var/lib/dukovo-system/rollout.json")).unwrap()
    }

    /// Posts `report`, sealed by `key`, through the receiver's socket:
    /// the status it answers.
    fn post(&self, key: &Key, report: &Report) -> u16 {
        let body = key.seal(report);
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        let head = format!(
            "POST /canary/v1/report HTTP/1.1\r\nHost: updates\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer[9..12].parse().unwrap()
    }
}

/// A report of the canary, made at `time`, healthy unless `change` says
/// otherwise, running `version` on the stable channel.
fn report(time: i64, version: &str, change: impl FnOnce(&mut Report)) -> Report {
    let mut report = Report::new(NAME, time, version, "stable");
    report.health = Some(Health { passed: true, time, detail: String::new() });
    report.check = Some(Check { failures: 0, warnings: 0, failed: Vec::new() });
    change(&mut report);
    report
}

/// A small slot, as `make` signs one.
fn tree(dir: &Path) {
    std::fs::create_dir_all(dir.join("commands")).unwrap();
    std::fs::write(dir.join("commands/spark"), b"spark's bytes").unwrap();
    std::fs::write(dir.join("empty"), b"").unwrap();
}

fn copy(from: &Path, to: &Path) {
    let status = Command::new("cp").arg("-R").arg(from).arg(to).status().unwrap();
    assert!(status.success());
}

/// Sets the server up as setup.sh does, deploys release [`VERSION`] on
/// its stable channel as `ob release deploy` does and has it published,
/// and starts the receiver.
fn server(what: &str) -> (Server, Receiver) {
    let root = std::env::temp_dir().join(format!("update-server-{}-{what}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let server = Server {
        keys: root.join("keys"),
        socket: root.join("run/receive.sock"),
        root: root.clone(),
    };
    let lib = server.lib();
    for folder in [
        "var/lib/dukovo-system",
        "var/lib/dukovo-system-incoming",
        "var/lib/dukovo-canaries/reports",
        "etc/dukovo-update-server",
        "srv/dukovo-system",
        "run",
    ] {
        std::fs::create_dir_all(server.path(folder)).unwrap();
    }
    std::fs::create_dir_all(&lib).unwrap();
    for script in ["system-site.sh", "system-publish.sh", "system-refresh.sh", "system-canaries.sh"]
    {
        std::fs::copy(Path::new(SCRIPTS).join(script), lib.join(script)).unwrap();
    }
    std::fs::copy(Path::new(SCRIPTS).join("canaries.sh"), lib.join("canaries.sh")).unwrap();
    std::fs::copy(PROGRAM, lib.join("system-release")).unwrap();

    // The server's online key, which the release keys' next root names,
    // and the release keys' repository, signed by them.
    let home = server.path("var/lib/dukovo-system");
    let (online, secret) = (home.join("online.json"), home.join("online.key"));
    let path = |path: &Path| path.display().to_string();
    run(&["online-key", "--out", &path(&online), &path(&secret)]);
    run(&["keys", &path(&server.keys)]);
    run(&["keys", "--online", &path(&online), &path(&server.keys)]);
    tree(&root.join("slot"));
    let make = [
        "make",
        "--tree",
        &path(&root.join("slot")),
        "--keys",
        &path(&server.keys),
        "--out",
        &path(&root.join("release")),
        "--version",
        VERSION,
        "--commit",
        "01d66b7",
        "--toolchain",
        "nightly-2026-10-15",
        "--built",
        "1793923200",
    ];
    run(&make);
    let repo = ["--repo", &path(&root.join("repository")), "--keys", &path(&server.keys)];
    let with = |words: &[&str]| -> Vec<String> {
        let mut all = strings(&words[..1]);
        all.extend(strings(&repo));
        all.extend(strings(&words[1..]));
        all
    };
    run_all(&with(&["add", &path(&root.join("release"))]));
    run_all(&with(&["channel", "stable", VERSION]));

    // The deploy: what the release keys signed, in the incoming folder.
    let incoming = server.path("var/lib/dukovo-system-incoming");
    for folder in ["metadata", "targets", "files"] {
        copy(&root.join("repository").join(folder), &incoming);
    }
    std::fs::write(incoming.join("ready"), b"").unwrap();
    server.ok("system-publish.sh", &[]);
    assert_eq!(
        std::fs::read_to_string(incoming.join("result")).unwrap().lines().next(),
        Some("published")
    );

    let list = server.path("etc/dukovo-update-server/canaries.json");
    let receiver = Command::new(server.lib().join("system-release"))
        .args(["canary-receive", "--socket"])
        .arg(&server.socket)
        .arg("--canaries")
        .arg(&list)
        .arg("--reports")
        .arg(server.path("var/lib/dukovo-canaries/reports"))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let receiver = Receiver(receiver);
    for _ in 0..1000 {
        if server.socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(server.socket.exists(), "the receiver made no socket");
    (server, receiver)
}

#[test]
fn a_canary_that_is_not_green_holds_the_waves_and_a_green_one_frees_them() {
    let (server, _receiver) = server("system");
    let now = tuf::time::now();
    let day = 24 * 3600;

    // The release was offered eight days ago: with no canary on the list,
    // the waves open on schedule, as they always did.
    let mut plans = server.plans();
    plans.releases.get_mut(VERSION).unwrap().released = now - 8 * day;
    plans.save(&server.path("var/lib/dukovo-system/rollout.json")).unwrap();
    // Nothing to judge, so nothing to sign.
    let before = server.timestamp();
    assert_eq!(server.ok("system-canaries.sh", &[]), "");
    assert_eq!(server.timestamp(), before);
    assert_eq!(server.rollout(), (0, None));

    // A canary enrolled holds the waves until it reports.
    let key = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
    let said = server.ok("canaries.sh", &["add", NAME, &key.public_hex()]);
    assert!(said.contains("canary ref-box added"), "{said}");
    let list = std::fs::read_to_string(server.path("etc/dukovo-update-server/canaries.json"));
    assert!(list.unwrap().contains(&key.public_hex()));
    let said = server.ok("system-canaries.sh", &[]);
    assert!(said.contains("the canaries not green are ref-box"), "{said}");
    assert_eq!(server.rollout(), (0, Some("canaries not green: ref-box".into())));
    assert_eq!(server.plans().red, [NAME]);

    // Nothing has changed since: nothing is signed again.
    let signed = (server.timestamp(), server.served());
    assert_eq!(server.ok("system-canaries.sh", &[]), "");
    assert_eq!((server.timestamp(), server.served()), signed);

    // A report from a key that is not on the list, and one posing as
    // another's, are refused, and the waves stay held.
    let stranger = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
    assert_eq!(server.post(&stranger, &report(now - 100, VERSION, |_| {})), 403);

    // A healthy report from the canary, which takes the release, frees
    // them: wave 4 on day 8.
    assert_eq!(server.post(&key, &report(now - 90, VERSION, |_| {})), 200);
    assert_eq!(server.post(&key, &report(now - 90, VERSION, |_| {})), 409);
    let said = server.ok("system-canaries.sh", &[]);
    assert!(said.contains("the canaries not green are none"), "{said}");
    assert_eq!(server.rollout(), (4, None));
    assert!(server.plans().red.is_empty());
    let signed = (server.timestamp(), server.served());
    assert_eq!(server.ok("system-canaries.sh", &[]), "");
    assert_eq!((server.timestamp(), server.served()), signed);

    // Three hours with no report, it is silent: held again.
    let later = (now + 3 * 3600).to_string();
    server.ok("system-canaries.sh", &["--now", &later]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    // And a report in time frees it.
    assert_eq!(server.post(&key, &report(now - 60, VERSION, |_| {})), 200);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);

    // A failed health check holds them, and a report that fixes it frees
    // them.
    let sick = report(now - 50, VERSION, |r| {
        r.health = Some(Health { passed: false, time: now - 50, detail: "no network".into() });
    });
    assert_eq!(server.post(&key, &sick), 200);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    let shown = server.ok("canaries.sh", &["show", "--json"]);
    let shown: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(shown["red"], serde_json::json!([NAME]));
    assert_eq!(shown["in_step"], true);
    assert_eq!(shown["canaries"][0]["reasons"][0], "its health check failed: no network");
    let text = server.ok("canaries.sh", &["show"]);
    assert!(text.contains("ref-box: not green; its health check failed: no network"), "{text}");
    assert_eq!(server.post(&key, &report(now - 40, VERSION, |_| {})), 200);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);

    // A canary that runs an older release than its channel has offered
    // for more than six hours has not taken it.
    let behind = report(now - 30, "0.15.0", |_| {});
    assert_eq!(server.post(&key, &behind), 200);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    let shown = server.ok("canaries.sh", &["show", "--json"]);
    assert!(shown.contains("has not taken Dukovo 0.16.0 in 192 hours"), "{shown}");
    assert_eq!(server.post(&key, &report(now - 20, VERSION, |_| {})), 200);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);

    // A colour set by hand wins: red over a healthy report, and green over
    // a failed one, each until `auto`.
    server.ok("canaries.sh", &[NAME, "red", "disk", "swap"]);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    let sick = report(now - 10, VERSION, |r| {
        r.services_failed = vec!["sshd".into()];
    });
    assert_eq!(server.post(&key, &sick), 200);
    server.ok("canaries.sh", &[NAME, "green", "known", "fault"]);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);
    server.ok("canaries.sh", &[NAME, "auto"]);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    let text = server.ok("canaries.sh", &["show"]);
    assert!(text.contains("failed services: sshd"), "{text}");

    // Taking the canary off the list frees the waves, as with no canary.
    server.ok("canaries.sh", &["remove", NAME]);
    server.ok("system-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);
    assert!(server.plans().red.is_empty());
    let signed = (server.timestamp(), server.served());
    assert_eq!(server.ok("system-canaries.sh", &[]), "");
    assert_eq!((server.timestamp(), server.served()), signed);
    // Its report is refused now, with no canary by that key.
    assert_eq!(server.post(&key, &report(now, VERSION, |_| {})), 403);

    // The scripts refuse what they should: a bad key, no reason, a name
    // that is not there.
    for args in [&["add", "x", "abcd"][..], &[NAME, "red"], &["remove", NAME], &["nobody", "auto"]]
    {
        assert!(!server.script("canaries.sh", args).status.success(), "{args:?}");
    }
    assert!(!server.script("canaries.sh", &[]).status.success());
    let _ = std::fs::remove_dir_all(&server.root);
}

#[test]
fn nothing_is_held_before_a_release_is_deployed() {
    let root = std::env::temp_dir().join(format!("update-server-{}-empty", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("usr/local/lib/dukovo-system");
    for folder in ["var/lib/dukovo-system", "srv/dukovo-system", "run", "etc/dukovo-update-server"]
    {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    std::fs::create_dir_all(&lib).unwrap();
    for script in ["system-site.sh", "system-canaries.sh"] {
        std::fs::copy(Path::new(SCRIPTS).join(script), lib.join(script)).unwrap();
    }
    std::fs::copy(PROGRAM, lib.join("system-release")).unwrap();
    // The online key exists (setup makes it), the first root does not.
    run(&[
        "online-key",
        "--out",
        &root.join("var/lib/dukovo-system/online.json").display().to_string(),
        &root.join("var/lib/dukovo-system/online.key").display().to_string(),
    ]);
    let output = Command::new("sh")
        .arg(lib.join("system-canaries.sh"))
        .env("DUKOVO_SERVER_ROOT", &root)
        .env("DUKOVO_LOCK", root.join("run/lock"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("No system release has been deployed")
    );
    let _ = std::fs::remove_dir_all(&root);
}
