// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The move of the update server to Dukovo (ADR 0542), as
//! docs/runbooks/update-server-move.md has it: the setup, jobs and nginx
//! settings of tools/update-server/dukovo/, and the deploys of *ob*, which
//! need only ssh, sh and tar on the server. The machine is an ordinary
//! Dukovo with nginx's bundle installed and alice's key in its sshd. Its
//! clock is set to noon of the next day, so that the six hours the test
//! moves it by later pass the time of no daily job. The host brings the
//! online keys to a folder of /tmp as the move does, each secret half as
//! base64 and each public half beside it, copies the setup over with scp,
//! as *ob catalog server setup --dukovo* does, and runs it through
//! elevate for a name that has no certificate: setup takes the keys,
//! certd cannot get a certificate for the name, setup says so, and nginx
//! serves over HTTP. Each of the six jobs is enabled with its timer, each
//! online key is a secret of the jobs that sign with it, and the folder
//! of keys is gone.
//!
//! The host deploys a release of two files and a catalog of one bundle with
//! the code of *ob release deploy* and *ob catalog deploy*, from
//! repositories whose roots name those keys. A deploy left without the word
//! ready is not published, though the publish timer ran: what the server's
//! tar unpacked is in the group the jobs act as, folders of mode 770 and
//! files of 660, and the host still reads one release. Told ready, the next
//! deploy is published within the timer's minute. The host reads both
//! repositories back through nginx as a client does, system-release served
//! from the first root and catalog-repo check, which fetches every bundle.
//! nginx sends Caddy's cache headers, sends the bare address to the
//! project's site, and answers 404 for a folder and for anything else.
//!
//! Once each refresh timer has run, the clock moves forward six hours and
//! each signs a new timestamp. Setup run again, for the name this-machine,
//! which certd made, keeps the online keys; nginx then serves HTTPS with
//! that certificate to a client that trusts it, and sends HTTP there. A
//! certificate from an ACME authority, the version numbers a move carries
//! over, and the install of nginx from the catalog, which setup does when
//! nginx is not there, are not tried.

use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::super::*;
use super::help_session::{as_root, free_port, keys, scan_host_key, sshd_on};
use crate::deploy::Server;

/// The test's files, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/update-server-move";

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    bundle_files_disk(128 * 1024, LABEL),
];

/// A name that has no certificate, and cannot get one.
const PLAIN_HOST: &str = "updates.test";

/// The name of the certificate certd makes for the machine.
const CERTD_HOST: &str = "this-machine";

/// The test.
pub const CASE: Case = Case {
    name: "runbook-update-server-move",
    about: "the move of the update server to Dukovo (ADR 0542): the online keys brought in a folder of /tmp and setup run through elevate make the six jobs with timers, each online key a secret of the jobs that sign with it, and nginx, over HTTP for a name that has no certificate; deploys with ssh and tar, whose folders are the jobs' group's, are published by the timers and read back by a client from the first root, system releases and catalog with its bundle, with Caddy's cache headers and refusals; a deploy not told ready is not published, and is when told; each refresh timer signs a new timestamp after the clock moves six hours; run again for this-machine, setup keeps the keys and nginx serves HTTPS with certd's certificate and sends HTTP to it",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, certd and timerd among
    // them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: update_server,
    disks: DISKS,
    second: None,
};

/// What a GET of nginx answered.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    /// The value of the header `name`, in any case.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The answer in `bytes`, as it came: a status line, headers, a blank
/// line and the body.
fn parse_reply(bytes: &[u8]) -> Result<Reply, String> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the answer has no end of its headers")?;
    let head = String::from_utf8_lossy(&bytes[..split]);
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or("the answer has no status")?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    Ok(Reply { status, headers, body: bytes[split + 4..].to_vec() })
}

/// GETs `path` of the guest's nginx through the host's `port`, naming the
/// update server's host.
fn get(port: u16, path: &str) -> Result<Reply, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|err| format!("nginx is not reached at port {port}: {err}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|err| err.to_string())?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {PLAIN_HOST}\r\nConnection: close\r\n\r\n")
        .map_err(|err| format!("asking nginx for {path}: {err}"))?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).map_err(|err| format!("reading nginx's answer: {err}"))?;
    parse_reply(&bytes).map_err(|err| format!("{path}: {err}"))
}

/// The version of the TUF metadata in `body`.
fn metadata_version(body: &[u8]) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    json["signed"]["version"].as_u64()
}

/// The version of the timestamp that `path` of nginx serves.
fn timestamp_version(port: u16, path: &str) -> Result<u64, String> {
    let reply = get(port, path)?;
    if reply.status != 200 {
        return Err(format!("{path} answered {}, not 200", reply.status));
    }
    metadata_version(&reply.body).ok_or_else(|| format!("{path} is no TUF metadata"))
}

/// Fails unless `reply` has `status` and, for each of `headers`, the value.
fn check_reply(
    path: &str,
    reply: &Reply,
    status: u16,
    headers: &[(&str, &str)],
) -> Result<(), String> {
    if reply.status != status {
        return Err(format!("{path} answered {}, not {status}", reply.status));
    }
    for (name, value) in headers {
        if reply.header(name) != Some(value) {
            return Err(format!(
                "{path} has {name}: {:?}, not {value:?}",
                reply.header(name).unwrap_or("none")
            ));
        }
    }
    Ok(())
}

/// Runs `script` on the guest as alice, over ssh: whether it succeeded and
/// what it wrote.
fn guest(server: &Server, script: &str) -> Result<(bool, String), String> {
    let output = server.ssh(script).output().map_err(|err| format!("running ssh: {err}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), text))
}

/// [`guest`], failing with what it wrote unless the script succeeded.
fn guest_ok(server: &Server, script: &str) -> Result<String, String> {
    let (succeeded, text) = guest(server, script)?;
    if succeeded {
        Ok(text)
    } else {
        Err(format!("on the machine, `{script}` failed:\n{}", text.trim_end()))
    }
}

/// Fails unless `text` holds `needle`; `what` says what wrote `text`.
fn require(text: &str, needle: &str, what: &str) -> Result<(), String> {
    if text.contains(needle) {
        Ok(())
    } else {
        Err(format!("{what} does not say \"{needle}\": {}", text.trim_end()))
    }
}

/// How many lines of the serial log hold `needle`.
fn log_count(vm: &Vm, needle: &str) -> usize {
    vm.log().lines().filter(|line| line.contains(needle)).count()
}

/// Waits up to `seconds` until more than `before` lines of the serial log
/// hold `needle`.
fn wait_log(vm: &Vm, needle: &str, before: usize, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while log_count(vm, needle) <= before {
        if Instant::now() >= deadline {
            return Err(format!("the log has no new line with \"{needle}\" in {seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// The host's `date` for `when`, as the machine's `timedatectl set-time`
/// takes it: `YYYY-MM-DD HH:MM:SS`.
fn host_date(when: &str) -> Result<String, String> {
    let output = Command::new("date")
        .args(["-u", "-d", when, "+%Y-%m-%d %H:%M:%S"])
        .output()
        .map_err(|err| format!("running date: {err}"))?;
    if !output.status.success() {
        return Err(format!("date does not know \"{when}\""));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `system-release` with `args`, built for this machine.
fn release_tool<S: AsRef<OsStr>>(ctx: &Ctx, args: &[S]) -> Result<(), String> {
    crate::release::system_release(ctx, args).map_err(|err| err.to_string())
}

/// The update repository the test deploys, and what signs and reads it.
struct Releases {
    /// The repository `system-release add` writes.
    repo: PathBuf,
    /// The release keys.
    keys: PathBuf,
    /// The first root, which a client starts from.
    root: PathBuf,
}

impl Releases {
    /// Release keys of the test's own, whose second root names the update
    /// server's online key, the public half of which is the file
    /// `online`; and an empty repository.
    fn new(ctx: &Ctx, dir: &Path, online: &Path) -> Result<Releases, String> {
        let releases = Releases {
            repo: dir.join("repository"),
            keys: dir.join("release-keys"),
            root: dir.join("first-root.json"),
        };
        let os = OsStr::new;
        release_tool(ctx, &[os("keys"), releases.keys.as_os_str()])?;
        // The first root, before the second names the server's key.
        std::fs::copy(releases.keys.join("root.json"), &releases.root)
            .map_err(|err| format!("copying the first root: {err}"))?;
        release_tool(
            ctx,
            &[os("keys"), os("--online"), online.as_os_str(), releases.keys.as_os_str()],
        )?;
        Ok(releases)
    }

    /// Adds a release of two small files, `version` and the stable
    /// channel's.
    fn add(&self, ctx: &Ctx, dir: &Path, version: &str) -> Result<(), String> {
        let tree = dir.join(format!("tree-{version}"));
        std::fs::create_dir_all(&tree)
            .map_err(|err| format!("making {}: {err}", tree.display()))?;
        std::fs::write(tree.join("notes.txt"), format!("release {version}\n"))
            .and_then(|()| std::fs::write(tree.join("data.bin"), vec![0x5a; 4096]))
            .map_err(|err| format!("writing {}: {err}", tree.display()))?;
        let release = dir.join(format!("release-{version}"));
        crate::release::make_with(ctx, &self.keys, &tree, &release, version, version)
            .map_err(|err| err.to_string())?;
        let command = |subcommand: &str, rest: &[&OsStr]| -> Vec<OsString> {
            let mut args: Vec<OsString> = vec![
                subcommand.into(),
                "--repo".into(),
                self.repo.clone().into_os_string(),
                "--keys".into(),
                self.keys.clone().into_os_string(),
            ];
            args.extend(rest.iter().map(|word| (*word).to_os_string()));
            args
        };
        let channel = [OsStr::new("stable"), OsStr::new(version)];
        for args in [command("add", &[release.as_os_str()]), command("channel", &channel)] {
            release_tool(ctx, &args)?;
        }
        Ok(())
    }

    /// What a client reads at `url` from the first root.
    fn served(&self, ctx: &Ctx, url: &str) -> Result<String, String> {
        let tool = crate::release::tool_for(ctx, false).map_err(|err| err.to_string())?;
        let output = Command::new(tool)
            .arg("served")
            .arg("--root")
            .arg(&self.root)
            .arg(url)
            .output()
            .map_err(|err| format!("running system-release: {err}"))?;
        if !output.status.success() {
            return Err(format!(
                "system-release served {url} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim_end()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The host's ssh options to the machine through `port`, which take only
/// the host key in `dir`'s known_hosts and alice's key; each is one of
/// ssh's and scp's `-o` words.
fn ssh_options(dir: &Path, port: u16) -> Vec<OsString> {
    let known = format!("UserKnownHostsFile={}", dir.join("known_hosts").display());
    let words = [
        "-F",
        "/dev/null",
        "-o",
        &format!("Port={port}"),
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        &known,
        "-o",
        "LogLevel=ERROR",
        "-o",
        "ConnectTimeout=30",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "BatchMode=yes",
        "-i",
    ];
    let mut options: Vec<OsString> = words.iter().map(OsString::from).collect();
    options.push(dir.join("alice").into_os_string());
    options
}

/// The two online keys a move brings to the server: each secret half as
/// base64, as the old server's files are carried, and each public half.
struct OnlineKeys {
    /// The public halves of the system releases' key and the catalog's.
    system: PathBuf,
    catalog: PathBuf,
    /// What setup takes from its folder, by file name.
    files: Vec<(&'static str, Vec<u8>)>,
}

impl OnlineKeys {
    /// Makes the keys in `dir`, as the old server made them.
    fn new(ctx: &Ctx, dir: &Path) -> Result<OnlineKeys, String> {
        let (secret, public) = (dir.join("system.key"), dir.join("system.json"));
        let os = OsStr::new;
        release_tool(
            ctx,
            &[os("online-key"), os("--out"), public.as_os_str(), secret.as_os_str()],
        )?;
        let (catalog_secret, catalog_public) =
            crate::catalog::test_online_key(ctx, &dir.join("catalog-key"))
                .map_err(|err| err.to_string())?;
        let text = |path: &Path| {
            std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))
        };
        Ok(OnlineKeys {
            files: vec![
                ("catalog.key", base64_of(&catalog_secret)?),
                ("system.key", base64_of(&secret)?),
                ("catalog.json", text(&catalog_public)?),
                ("system.json", text(&public)?),
            ],
            system: public,
            catalog: catalog_public,
        })
    }

    /// Puts the keys in `/tmp/dukovo-update-keys` of the machine, a folder
    /// of its login's that no one else reaches, as step 4 of the runbook
    /// does from the old server.
    fn send(&self, server: &Server) -> Result<(), String> {
        guest_ok(server, "umask 077 && mkdir /tmp/dukovo-update-keys")?;
        for (name, bytes) in &self.files {
            let mut cmd = server.ssh(&format!("umask 077 && cat > /tmp/dukovo-update-keys/{name}"));
            cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|err| format!("running ssh: {err}"))?;
            child
                .stdin
                .take()
                .expect("the input was piped")
                .write_all(bytes)
                .map_err(|err| format!("sending {name}: {err}"))?;
            let output =
                child.wait_with_output().map_err(|err| format!("sending {name}: {err}"))?;
            if !output.status.success() {
                return Err(format!(
                    "sending {name}: {}",
                    String::from_utf8_lossy(&output.stderr).trim_end()
                ));
            }
        }
        Ok(())
    }

    /// Fails unless the public halves setup keeps are the ones sent, and
    /// the folder of keys is gone.
    fn kept(&self, server: &Server) -> Result<(), String> {
        for (kind, sent) in [("system", &self.system), ("catalog", &self.catalog)] {
            let kept = guest_ok(server, &format!("cat /var/lib/dukovo-{kind}/online.json"))?;
            let sent = std::fs::read_to_string(sent)
                .map_err(|err| format!("reading {}: {err}", sent.display()))?;
            if kept.trim() != sent.trim() {
                return Err(format!("the {kind} online key's public half is not the one sent"));
            }
        }
        if guest(server, "ls /tmp/dukovo-update-keys")?.0 {
            return Err("/tmp/dukovo-update-keys is still there after setup took the keys".into());
        }
        Ok(())
    }
}

/// The file `path` as base64 on one line, as `base64 -w0` writes it.
fn base64_of(path: &Path) -> Result<Vec<u8>, String> {
    let output = Command::new("base64")
        .arg("-w0")
        .arg(path)
        .output()
        .map_err(|err| format!("running base64: {err}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!("base64 could not read {}", path.display()))
    }
}

/// Runs setup on the machine for `host`, through elevate, and gives what
/// it said.
fn run_setup(server: &Server, host: &str) -> Result<String, String> {
    guest_ok(server, &as_root(&format!("sh \"$PWD/dukovo-update-server/dukovo/setup\" {host}")))
}

/// What the machine's folders and nginx do for the host's deploys, once
/// setup has run for a name without a certificate: both repositories deployed and
/// read back, the cache headers and the refusals.
fn deployed(
    ctx: &Ctx,
    server: &Server,
    dir: &Path,
    web: u16,
    keys: &OnlineKeys,
) -> Result<Releases, String> {
    let releases = Releases::new(ctx, dir, &keys.system)?;
    releases.add(ctx, dir, "9.9.9-test.one")?;
    crate::release::publish_to(ctx, server, &releases.repo).map_err(|err| err.to_string())?;

    let bundle = crate::bundles::pack_test(
        ctx,
        &crate::bundles::TestBundle {
            id: "org.example.served",
            version: "1.0.0",
            script: "#!/bin/sh\necho served\n",
            permissions: "{}",
            services: "",
            provides: "",
            line: "",
            add_ons: "",
            feed: "",
            uses: "",
            posix: "",
        },
    )
    .map_err(|err| err.to_string())?;
    let (repo, store) =
        crate::catalog::test_repo(ctx, &dir.join("catalog"), &[bundle], &keys.catalog)
            .map_err(|err| err.to_string())?;
    crate::catalog::publish_to(ctx, server, &repo, &store).map_err(|err| err.to_string())?;

    let served = releases.served(ctx, &format!("http://127.0.0.1:{web}/system/"))?;
    require(&served, "offers 1 release, newest Dukovo 9.9.9-test.one", "system-release served")?;
    let read =
        crate::catalog::check_test_repo(ctx, &repo, &format!("http://127.0.0.1:{web}/catalog/"))
            .map_err(|err| err.to_string())?;
    require(&read, "fetches all", "catalog-repo check")?;
    headers_and_refusals(server, web)?;
    Ok(releases)
}

/// nginx's cache headers and refusals, which are Caddy's.
fn headers_and_refusals(server: &Server, web: u16) -> Result<(), String> {
    let immutable = "public, max-age=31536000, immutable";
    for path in ["/system/metadata/timestamp.json", "/catalog/metadata/timestamp.json"] {
        let reply = get(web, path)?;
        check_reply(
            path,
            &reply,
            200,
            &[
                ("cache-control", "no-cache"),
                ("x-content-type-options", "nosniff"),
                ("strict-transport-security", "max-age=31536000"),
            ],
        )?;
    }
    let path = "/system/metadata/1.root.json";
    check_reply(path, &get(web, path)?, 200, &[("cache-control", immutable)])?;
    let file = guest_ok(server, "ls /var/lib/dukovo-system-site/system/files | head -n 1")?;
    let path = format!("/system/files/{}", file.trim());
    check_reply(&path, &get(web, &path)?, 200, &[("cache-control", immutable)])?;
    // The bare address is for people; a folder and anything else is 404.
    check_reply("/", &get(web, "/")?, 302, &[("location", "https://dukovo.org/")])?;
    for path in ["/nothing", "/system/", "/catalog/metadata/", "/index.html"] {
        check_reply(path, &get(web, path)?, 404, &[])?;
    }
    Ok(())
}

/// The setup's reach on the machine: the six jobs with timers, the secrets
/// and who can read what.
fn jobs_and_keys(server: &Server) -> Result<(), String> {
    let timers = guest_ok(server, &as_root("service list --timers"))?;
    for job in [
        "dukovo-system-publish",
        "dukovo-catalog-publish",
        "dukovo-system-refresh",
        "dukovo-catalog-refresh",
        "dukovo-catalog-flaws",
        "dukovo-flaws-fetch",
    ] {
        require(&timers, job, "service list --timers")?;
    }
    // Each online key is a secret of the jobs that sign with it, and the
    // job that reaches the network has none.
    let signing = [
        "dukovo-system-publish",
        "dukovo-system-refresh",
        "dukovo-catalog-publish",
        "dukovo-catalog-refresh",
        "dukovo-catalog-flaws",
    ];
    for job in signing {
        let secrets = guest_ok(server, &as_root(&format!("service secret list {job}")))?;
        require(&secrets, "online-key", &format!("the secrets of {job}"))?;
    }
    let secrets = guest_ok(server, &as_root("service secret list dukovo-flaws-fetch"))?;
    if secrets.contains("online-key") {
        return Err("dukovo-flaws-fetch has an online key, and it reaches the network".into());
    }
    Ok(())
}

/// The modes and group of what a deploy left in the incoming folder
/// without `ready`: the group the jobs act as, 770 for a folder and 660
/// for a file.
fn left_for_the_group(server: &Server) -> Result<(), String> {
    let incoming = "/var/lib/dukovo-system-incoming";
    let listed = guest_ok(
        server,
        &format!(
            "stat -c '%n %a %g' {incoming}/metadata {incoming}/targets; for f in \
             {incoming}/files/*; do stat -c '%n %a %g' $f; break; done; ls {incoming}"
        ),
    )?;
    for line in listed.lines().filter(|line| line.starts_with(incoming)) {
        let mode = line.split_whitespace().nth(1).unwrap_or("");
        let group = line.split_whitespace().nth(2).unwrap_or("");
        let wanted = if line.contains("/files/") { "660" } else { "770" };
        if !mode.ends_with(wanted) || group != "0" {
            return Err(format!("{line}: not mode {wanted} of group 0, which the jobs act as"));
        }
    }
    if listed.lines().any(|line| line.trim() == "ready") {
        return Err("the incoming folder has `ready`, which this deploy did not say".into());
    }
    Ok(())
}

fn update_server(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let ctx = Ctx::load(false).map_err(|err| err.to_string())?;
    let dir = std::env::current_dir().map_err(|err| err.to_string())?.join(DIR);
    let alice_key = keys(&dir)?;
    // What the machine is set up from, made while it boots.
    let staging = crate::catalog::stage_dukovo(&ctx).map_err(|err| err.to_string())?;

    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    let (port, web) = (free_port()?, free_port()?);
    sshd_on(vm, &alice_key, port)?;
    vm.monitor(&format!("hostfwd_add n0 tcp:127.0.0.1:{web}-:80"))?;
    let host_key = scan_host_key(port)?;
    std::fs::write(dir.join("known_hosts"), format!("{host_key}\n"))
        .map_err(|err| format!("writing known_hosts: {err}"))?;
    let server = Server::with_options("alice@127.0.0.1", ssh_options(&dir, port));

    // The clock to noon tomorrow: the six hours the test moves it by
    // then pass no daily job's time.
    let today = guest_ok(&server, "date +%Y-%m-%d")?;
    let noon = host_date(&format!("{} 12:00:00 UTC + 1 day", today.trim()))?;
    guest_ok(&server, &as_root(&format!("timedatectl set-time '{noon}'")))?;

    // nginx's bundle, which setup would fetch from the catalog.
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere("spark: mounted the FAT filesystem of vdb at /volumes/vdb")?;
    let installed = guest_ok(
        &server,
        &as_root("sh -c 'bundle install --for-everyone --yes /volumes/vdb/org.nginx.nginx-*.dkv'"),
    )?;
    require(&installed, "installed org.nginx.nginx", "bundle install")?;

    // The keys come as step 4 of the runbook brings them, setup is copied
    // as ob copies it, and run for a name without a certificate.
    let online_keys = OnlineKeys::new(&ctx, &dir)?;
    online_keys.send(&server)?;
    crate::catalog::copy_staging(&ctx, &server, &staging).map_err(|err| err.to_string())?;
    let said = run_setup(&server, PLAIN_HOST)?;
    require(&said, "Taking the catalog online key from /tmp/dukovo-update-keys.", "setup")?;
    require(&said, "Taking the system online key from /tmp/dukovo-update-keys.", "setup")?;
    require(&said, &format!("{PLAIN_HOST} has no certificate yet"), "setup")?;
    online_keys.kept(&server)?;
    jobs_and_keys(&server)?;

    // Both repositories are deployed, published by the timers and read
    // back through nginx.
    let releases = deployed(&ctx, &server, &dir, web, &online_keys)?;

    // A deploy that is not told ready is not published, though the timer
    // ran.
    releases.add(&ctx, &dir, "9.9.9-test.two")?;
    crate::release::copy_to(&ctx, &server, &releases.repo).map_err(|err| err.to_string())?;
    left_for_the_group(&server)?;
    let publish_runs = "timerd: dukovo-system-publish: started by its timer";
    wait_log(vm, publish_runs, log_count(vm, publish_runs), 150)?;
    std::thread::sleep(Duration::from_secs(15));
    let url = format!("http://127.0.0.1:{web}/system/");
    let served = releases.served(&ctx, &url)?;
    require(&served, "offers 1 release, newest Dukovo 9.9.9-test.one", "system-release served")?;
    // Told ready, it is published within the timer's minute.
    crate::release::publish_to(&ctx, &server, &releases.repo).map_err(|err| err.to_string())?;
    let served = releases.served(&ctx, &url)?;
    require(&served, "offers 2 releases, newest Dukovo 9.9.9-test.two", "system-release served")?;

    refresh_after_six_hours(vm, &server, web)?;

    // Run again for the name certd made: the keys stay, and nginx serves
    // HTTPS.
    let before = guest_ok(&server, &sums_of_keys())?;
    let said = run_setup(&server, CERTD_HOST)?;
    require(&said, "The catalog online key is set.", "setup run again")?;
    require(&said, "The system online key is set.", "setup run again")?;
    let after = guest_ok(&server, &sums_of_keys())?;
    if before != after {
        return Err("setup run again changed an online key".into());
    }
    https(&server, web)?;
    super::install_uefi::power_off(vm)
}

/// The command that prints the SHA-256 of each job's online key, which
/// only root reads: the shell that runs it as root finds the files.
fn sums_of_keys() -> String {
    as_root("sh -c 'sha256sum /system/state/spark/secrets/*/online-key'")
}

/// Each refresh timer has run once, which gives it a last run; the clock
/// then moves forward six hours, and each signs a timestamp again.
fn refresh_after_six_hours(vm: &mut Vm, server: &Server, web: u16) -> Result<(), String> {
    let jobs = ["dukovo-system-refresh", "dukovo-catalog-refresh"];
    let needle = |job: &str| format!("timerd: {job}: started by its timer");
    // A timer that has not run counts its first start from the machine's
    // start, which moves with the clock: it has to have run.
    for job in jobs {
        wait_log(vm, &needle(job), 0, 900)?;
    }
    std::thread::sleep(Duration::from_secs(20));
    let paths = ["/system/metadata/timestamp.json", "/catalog/metadata/timestamp.json"];
    let before: Vec<u64> =
        paths.iter().map(|path| timestamp_version(web, path)).collect::<Result<_, _>>()?;
    let runs: Vec<usize> = jobs.iter().map(|job| log_count(vm, &needle(job))).collect();

    let now = guest_ok(server, "date '+%Y-%m-%d %H:%M:%S'")?;
    let later = host_date(&format!("{} UTC + 6 hours 2 minutes", now.trim()))?;
    guest_ok(server, &as_root(&format!("timedatectl set-time '{later}'")))?;
    for (job, before) in jobs.iter().zip(runs) {
        wait_log(vm, &needle(job), before, 240)?;
    }
    std::thread::sleep(Duration::from_secs(20));
    for (path, before) in paths.iter().zip(before) {
        let after = timestamp_version(web, path)?;
        if after <= before {
            return Err(format!("{path} is at version {after}, no newer than {before}"));
        }
    }
    Ok(())
}

/// nginx serves HTTPS with certd's certificate to a client that trusts
/// it, and sends HTTP there.
fn https(server: &Server, web: u16) -> Result<(), String> {
    let answered = guest_ok(
        server,
        "cert show this-machine --pem > /tmp/machine.pem; for i in 1 2 3 4 5 6 7 8 9 10; do \
         curl -s --cacert /tmp/machine.pem -o /dev/null -w 'https %{http_code}\\n' \
         https://localhost/system/metadata/timestamp.json && break; sleep 3; done",
    )?;
    require(&answered, "https 200", "curl with certd's certificate")?;
    let path = "/system/metadata/timestamp.json";
    let location = format!("https://{CERTD_HOST}{path}");
    check_reply(path, &get(web, path)?, 301, &[("location", &location)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_its_status_its_headers_and_its_body() {
        let reply = parse_reply(
            b"HTTP/1.1 200 OK\r\nCache-Control: no-cache\r\nContent-Length: 2\r\n\r\n{}",
        )
        .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.header("cache-control"), Some("no-cache"));
        assert_eq!(reply.header("etag"), None);
        assert_eq!(reply.body, b"{}");
    }

    #[test]
    fn an_answer_that_stops_in_its_headers_is_no_reply() {
        assert!(parse_reply(b"HTTP/1.1 200 OK\r\nCache-Control: no-").is_err());
        assert!(parse_reply(b"nonsense\r\n\r\n").is_err());
    }

    #[test]
    fn the_version_of_metadata_is_inside_its_signed_part() {
        assert_eq!(metadata_version(br#"{"signed": {"version": 7}, "signatures": []}"#), Some(7));
        assert_eq!(metadata_version(br#"{"version": 7}"#), None);
        assert_eq!(metadata_version(b"not json"), None);
    }

    #[test]
    fn a_reply_with_another_header_or_status_fails_the_check() {
        let reply =
            parse_reply(b"HTTP/1.1 404 Not Found\r\nCache-Control: no-cache\r\n\r\n").unwrap();
        assert!(check_reply("/x", &reply, 404, &[("cache-control", "no-cache")]).is_ok());
        assert!(check_reply("/x", &reply, 200, &[]).is_err());
        let wrong = check_reply("/x", &reply, 404, &[("cache-control", "immutable")]);
        assert!(wrong.unwrap_err().contains("no-cache"));
    }
}
