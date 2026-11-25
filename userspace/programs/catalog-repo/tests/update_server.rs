// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The update server's canary flow for the catalog (ADR 0533), on the
//! server's own scripts, run on the host in a folder that stands in for
//! the server's `/`: a catalog is deployed and published by
//! `publish.sh`, a canary is on the list, its sealed reports are posted
//! to the receiver on a socket, and `catalog-canaries.sh` holds the waves
//! of a bundle's newest version or frees them. What is signed is read
//! back as a computer reads it, from the first root. The times the
//! canaries are judged at are given with `--now`, so that nothing waits.
//! The system's side is the same flow in system-release's tests.

#![forbid(unsafe_code)]

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use system_release::canary::{Canaries, Check, Colour, Health, Key, Report};
use system_release::receiver::{self, Paths};
use system_release::rollout::Plans;

/// tools/update-server, whose scripts these are.
const SCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../tools/update-server");

/// The program the server runs.
const PROGRAM: &str = env!("CARGO_BIN_EXE_catalog-repo");

const ID: &str = "org.example.tool";
const NAME: &str = "ref-box";

/// The server: a folder standing in for its `/`.
struct Server {
    root: PathBuf,
    store: PathBuf,
    test_key: PathBuf,
    socket: PathBuf,
}

impl Server {
    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    fn lib(&self) -> PathBuf {
        self.path("usr/local/lib/dukovo-catalog")
    }

    /// Runs the server's script `name` with `args`, with the places the
    /// service files of a Dukovo server give.
    fn script(&self, name: &str, args: &[&str]) -> Output {
        Command::new("sh")
            .arg(self.lib().join(name))
            .args(args)
            .env("DUKOVO_CATALOG_HOME", self.path("var/lib/dukovo-catalog"))
            .env("DUKOVO_CATALOG_INCOMING", self.path("var/lib/dukovo-catalog-incoming"))
            .env("DUKOVO_CATALOG_PROGRAM", self.lib().join("catalog-repo"))
            .env("DUKOVO_CATALOG_SITE", self.path("srv/dukovo"))
            .env("DUKOVO_CANARIES", self.path("etc/dukovo-update-server/canaries.json"))
            .env("DUKOVO_CANARY_REPORTS", self.path("var/lib/dukovo-canaries/reports"))
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

    fn list(&self) -> PathBuf {
        self.path("etc/dukovo-update-server/canaries.json")
    }

    fn plan(&self) -> PathBuf {
        self.path("var/lib/dukovo-catalog/rollout.json")
    }

    /// The folder the server serves now, and what it signed last: both
    /// change with each signing.
    fn signed(&self) -> (PathBuf, Vec<u8>) {
        let served = std::fs::canonicalize(self.path("srv/dukovo/catalog")).unwrap();
        let timestamp = std::fs::read(served.join("metadata/timestamp.json")).unwrap();
        (served, timestamp)
    }

    /// The highest wave open of the newest version of the bundle and why
    /// no further one is, read back as a computer reads it.
    fn rollout(&self) -> (u64, Option<String>) {
        let url = format!("file://{}/", self.signed().0.display());
        let repo = self.path("var/lib/dukovo-catalog/repo");
        let said = catalog_repo(&repo, &["served", "--json", &url]);
        let served: serde_json::Value = serde_json::from_str(&said).unwrap();
        let rollouts = served["rollouts"].as_array().unwrap();
        let rollout =
            rollouts.iter().find(|r| r["bundle"] == ID).expect("the bundle has a rollout");
        (rollout["wave"].as_u64().unwrap(), rollout["held"].as_str().map(str::to_string))
    }

    /// Signs the bundle's `version` into the release keys' repository and
    /// deploys what changed, as `ob catalog deploy` does, and has the
    /// server publish it.
    fn deploy(&self, version: &str) {
        let (repo, incoming) =
            (self.path("repository"), self.path("var/lib/dukovo-catalog-incoming"));
        let bundle = bundle(&self.root, version);
        let store = self.store.display().to_string();
        let key = self.test_key.display().to_string();
        let bundle = bundle.display().to_string();
        catalog_repo(&repo, &["sign", "--yes", "--key", &key, "--store", &store, &bundle]);
        for folder in ["catalog", "store"] {
            let _ = std::fs::remove_dir_all(incoming.join(folder));
        }
        for (from, to) in [
            (repo.join("keys"), incoming.join("catalog/keys")),
            (repo.join("metadata"), incoming.join("catalog/metadata")),
            (self.store.join("bundles"), incoming.join("store/bundles")),
        ] {
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            let status = Command::new("cp").arg("-R").arg(from).arg(to).status().unwrap();
            assert!(status.success());
        }
        std::fs::write(incoming.join("ready"), b"").unwrap();
        self.ok("publish.sh", &[]);
        let result = std::fs::read_to_string(incoming.join("result")).unwrap();
        assert_eq!(result.lines().next(), Some("published"), "{result}");
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

/// Runs catalog-repo on the repository `repo` with `args`.
fn catalog_repo(repo: &Path, args: &[&str]) -> String {
    let output = Command::new(PROGRAM).arg("--repo").arg(repo).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "catalog-repo {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A report of the canary, made at `time`, healthy, running `version`.
fn report(time: i64, version: &str) -> Report {
    let mut report = Report::new(NAME, time, version, "stable");
    report.health = Some(Health { passed: true, time, detail: String::new() });
    report.check = Some(Check { failures: 0, warnings: 0, failed: Vec::new() });
    report
}

/// The bundle `ID` at `version`, in `dir`.
fn bundle(dir: &Path, version: &str) -> PathBuf {
    let manifest = format!(r#"{{"id": "{ID}", "version": "{version}", "name": "Test"}}"#);
    let path = dir.join(format!("{ID}-{version}.dkv"));
    let mut zip = dkv::zip::Writer::new(std::fs::File::create(&path).unwrap());
    let sbom =
        format!("PackageName: {ID}\nExternalRef: PACKAGE-MANAGER purl pkg:cargo/regex@1.10.2\n");
    let tool = format!("the tool, {version}");
    let files = [
        (dkv::MANIFEST, manifest.as_bytes()),
        ("bin/tool", tool.as_bytes()),
        (dkv::sbom::SBOM, sbom.as_bytes()),
    ];
    for (name, data) in files {
        zip.add(&dkv::zip::Input { path: name, executable: false, data }).unwrap();
    }
    zip.finish().unwrap();
    path
}

/// Sets the server up as setup.sh does, with the receiver listening and
/// no catalog deployed yet.
fn server(what: &str) -> Server {
    let root = std::env::temp_dir().join(format!("update-server-{}-{what}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let server = Server {
        store: root.join("store"),
        test_key: root.join("test.key"),
        socket: root.join("run/receive.sock"),
        root: root.clone(),
    };
    for folder in [
        "var/lib/dukovo-catalog",
        "var/lib/dukovo-catalog-incoming",
        "var/lib/dukovo-canaries/reports",
        "etc/dukovo-update-server",
        "usr/local/lib/dukovo-catalog",
        "srv/dukovo",
        "run",
    ] {
        std::fs::create_dir_all(server.path(folder)).unwrap();
    }
    for script in ["site.sh", "publish.sh", "refresh.sh", "catalog-canaries.sh"] {
        std::fs::copy(Path::new(SCRIPTS).join(script), server.lib().join(script)).unwrap();
    }
    std::fs::copy(PROGRAM, server.lib().join("catalog-repo")).unwrap();

    // The server's online key, which the catalog's root lists, and the
    // signer's repository, whose root and bundles a test key signs.
    let home = server.path("var/lib/dukovo-catalog");
    let new = root.join("online-new");
    catalog_repo(
        &new,
        &[
            "key",
            "new",
            &home.join("online.key").display().to_string(),
            "online",
            "snapshot",
            "timestamp",
        ],
    );
    std::fs::copy(new.join("keys/online.json"), home.join("online.json")).unwrap();
    let repo = root.join("repository");
    let key = server.test_key.display().to_string();
    catalog_repo(&repo, &["key", "new", &key, "test", "root", "targets"]);
    std::fs::copy(home.join("online.json"), repo.join("keys/online.json")).unwrap();
    catalog_repo(&repo, &["root", "--key", &key]);

    // The receiver, in this process, on a socket of the folder.
    let paths =
        Paths { canaries: server.list(), reports: server.path("var/lib/dukovo-canaries/reports") };
    let socket = server.socket.clone();
    std::thread::spawn(move || receiver::serve(&socket, &paths));
    for _ in 0..1000 {
        if server.socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(server.socket.exists(), "the receiver made no socket");
    server
}

#[test]
fn a_canary_that_is_not_green_holds_the_catalogs_waves_and_a_green_one_frees_them() {
    let server = server("catalog");
    let now = tuf::time::now();
    let day = 24 * 3600;

    // Nothing is held, nor signed, before a catalog is deployed.
    let said = server.ok("catalog-canaries.sh", &[]);
    assert!(said.contains("No catalog has been deployed yet"), "{said}");

    // Two versions of a bundle, deployed and published: the second goes
    // through the waves, from wave 0, as the first rollouts began.
    server.deploy("1.0.0");
    server.deploy("1.0.1");
    assert_eq!(server.rollout(), (0, None));
    // It was released eight days ago: the waves open on schedule, when
    // nothing holds them.
    let mut plans = Plans::load(&server.plan()).unwrap();
    let release = format!("{ID}/1.0.1");
    plans.releases.get_mut(&release).unwrap().released = now - 8 * day;
    plans.save(&server.plan()).unwrap();

    // With no canary on the list, nothing is judged and nothing signed.
    let before = server.signed();
    assert_eq!(server.ok("catalog-canaries.sh", &[]), "");
    assert_eq!(server.signed(), before);

    // A canary enrolled holds the waves until it reports.
    let key = Key::from_pkcs8(&Key::generate().unwrap()).unwrap();
    let mut list = Canaries::default();
    list.add(NAME, &key.public_hex(), now).unwrap();
    list.save(&server.list()).unwrap();
    let said = server.ok("catalog-canaries.sh", &[]);
    assert!(said.contains("the canaries not green are ref-box"), "{said}");
    assert_eq!(server.rollout(), (0, Some("canaries not green: ref-box".into())));
    assert_eq!(Plans::load(&server.plan()).unwrap().red, [NAME]);
    let signed = server.signed();
    assert_eq!(server.ok("catalog-canaries.sh", &[]), "");
    assert_eq!(server.signed(), signed);

    // Its first healthy report frees them, though it runs an older system
    // release than its channel offers: that rule is the system's alone.
    assert_eq!(server.post(&key, &report(now - 60, "0.0.1")), 200);
    let said = server.ok("catalog-canaries.sh", &[]);
    assert!(said.contains("the canaries not green are none"), "{said}");
    assert_eq!(server.rollout(), (4, None));
    let signed = server.signed();
    assert_eq!(server.ok("catalog-canaries.sh", &[]), "");
    assert_eq!(server.signed(), signed);

    // Three hours with no report, it is silent: held again; a report in
    // time frees it.
    let later = (now + 3 * 3600).to_string();
    server.ok("catalog-canaries.sh", &["--now", &later]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    assert_eq!(server.post(&key, &report(now - 30, "0.0.1")), 200);
    server.ok("catalog-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);

    // A colour set by hand wins, until it is taken away.
    let mut list = Canaries::load(&server.list()).unwrap();
    list.set_override(NAME, Some((Colour::Red, "disk swap")), now).unwrap();
    list.save(&server.list()).unwrap();
    server.ok("catalog-canaries.sh", &[]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    list.set_override(NAME, None, now).unwrap();
    list.save(&server.list()).unwrap();
    server.ok("catalog-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);

    // Taking the canary off the list frees the waves, as with none.
    list.set_override(NAME, Some((Colour::Red, "again")), now).unwrap();
    list.save(&server.list()).unwrap();
    server.ok("catalog-canaries.sh", &[]);
    assert_eq!(server.rollout().1.as_deref(), Some("canaries not green: ref-box"));
    Canaries::default().save(&server.list()).unwrap();
    server.ok("catalog-canaries.sh", &[]);
    assert_eq!(server.rollout().1, None);
    assert!(Plans::load(&server.plan()).unwrap().red.is_empty());
    let signed = server.signed();
    assert_eq!(server.ok("catalog-canaries.sh", &[]), "");
    assert_eq!(server.signed(), signed);

    // The old way to mark a canary is refused by the program itself.
    let output = Command::new(PROGRAM)
        .arg("--repo")
        .arg(server.path("var/lib/dukovo-catalog/repo"))
        .args(["rollout", "canary", NAME, "red"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let _ = std::fs::remove_dir_all(&server.root);
}
