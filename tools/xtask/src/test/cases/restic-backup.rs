// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Backups in restic's format (ADR 0465), on a Dukovo root of Btrfs, to a
//! share of the host's Samba and to an S3 server the test runs on the host,
//! both reached through QEMU's user network at 10.0.2.2. The S3 server
//! keeps one bucket's objects as files and answers only requests signed by
//! its key with AWS's Signature Version 4, a page of ten keys at a time.
//! Alice, the owner, keeps a folder of notes and numbers. As an
//! administrator she mounts the share with `mount -t cifs` and adds it as a
//! destination with *--format restic*, keeping the newest backup of each
//! place alone; a wrong secret for the S3 server is refused by its
//! signature check; the right one adds the bucket, which keeps restic's
//! format by itself, as an append-only destination. A round
//! sends each place to both repositories with the machine's record; she
//! changes her folder, and a second round sends the changes. *backup show*
//! lists one backup of each place on the share, which forgot the first
//! round's, and two in the bucket, which forgets nothing. Both are read
//! through and each place restored and found as its snapshot. With her note
//! deleted, the bucket's newest backup of */users* is restored and *snap
//! restore* brings the note back from it; */shared*'s, restored from the
//! share's folder with the recovery key alone, comes back too. On the host,
//! Linux's `restic` reads both repositories through with their recovery
//! keys (*check --read-data*), finds five snapshots on the share and ten in the
//! bucket, and prints her note from the newest backup of */users*; a host
//! without restic leaves that part out, with a note. With the share
//! unmounted, a round refuses its empty folder and writes nothing there.
//! The power button ends it.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::super::*;
use super::backup::elevated;
use super::smb;

/// Where the share's and the bucket's files are, from the top of the
/// tree, which QEMU runs in.
const DIR: &str = "out/test/restic-backup";

/// The S3 server's one bucket.
const BUCKET: &str = "dukovo";

/// The S3 server's key: the access key's id and its secret.
const KEY_ID: &str = "DUKOVOTESTKEY0000001";
const SECRET: &str = "dukovo-s3-test-secret";

/// The most keys the S3 server lists at a time, few so that the client
/// asks for the next page.
const PAGE: usize = 10;

/// Dukovo's root on a copy of the test image, so that /users, /shared,
/// /applications and /system/settings are its subvolumes.
const DISKS: &[Disk] =
    &[Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) }];

/// The test.
pub const CASE: Case = Case {
    name: "restic-backup",
    about: "backups in restic's format to the host's Samba share and to an S3 server that checks AWS's signatures (ADR 0465): a wrong secret refused, the share's repository forgetting the first round and the bucket's append-only; read through and restored as they were; a note restored from the bucket with snap restore, and /shared's from the share with the recovery key alone; Linux's restic reads both through and prints the note; the share's folder refused while the share is unmounted",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts netd, which takes QEMU's address.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: restic_backup,
    disks: DISKS,
    second: None,
};

fn restic_backup(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let root: PathBuf = std::env::current_dir().map_err(|err| err.to_string())?;
    let dir = root.join(DIR);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
    }
    let (_samba, smb_port, user) = smb::serve(&dir.join("smb"))?;
    let share = dir.join("smb/share");
    let s3 = S3::start(&dir.join("s3"))?;
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;

    answer_prompt(
        vm,
        "$",
        "clear; cd; mkdir -p work; echo 'first draft' > work/notes.txt; seq 1 40000 > \
         work/numbers.txt; ln -s notes.txt work/latest; echo made",
    )?;
    vm.expect_screen(&["made"])?;

    // The share mounted and added; the bucket refused a wrong secret and
    // added with the right one; the recovery key kept on the share, for
    // the host's restic.
    let bucket = format!("s3+http://10.0.2.2:{}/{BUCKET}", s3.port);
    let setup = [
        "mkdir -p /volumes/nas".to_string(),
        format!(
            "mount -t cifs -o username={user},password={} //10.0.2.2:{smb_port}/share \
             /volumes/nas && echo mounted",
            smb::PASSWORD
        ),
        "backup add --format restic --every hand --verify never nas /volumes/nas | grep -e \
         Added -e restic"
            .to_string(),
        // The newest backup of each place alone is kept there.
        "for keep in daily weekly monthly; do echo \"$keep = 0\" >> \
         /system/settings/backup/nas.toml; done"
            .to_string(),
        format!(
            "AWS_ACCESS_KEY_ID={KEY_ID} AWS_SECRET_ACCESS_KEY=wrong backup add wrong {bucket} > \
             /tmp/wrong.txt 2>&1; echo \"wrong $?\"; grep -o \"HTTP 403, SignatureDoesNotMatch\" \
             /tmp/wrong.txt | head -1"
        ),
        format!(
            "AWS_ACCESS_KEY_ID={KEY_ID} AWS_SECRET_ACCESS_KEY={SECRET} backup add --append-only \
             --every hand --verify never cloud {bucket} | head -1"
        ),
        "backup key nas > /volumes/nas/recovery.key && backup key cloud > \
         /volumes/nas/cloud.key && echo kept"
            .to_string(),
    ];
    elevated(vm, "setup.sh", &setup.iter().map(String::as_str).collect::<Vec<_>>(), true)?;
    vm.expect_screen(&[
        "mounted",
        "Added nas: /volumes/nas/dukovo-backup/",
        ", and is the password of its restic repository:",
        "wrong 1",
        "HTTP 403, SignatureDoesNotMatch",
        &format!("Added cloud: {bucket}/dukovo-backup/"),
        "kept",
    ])?;
    let refused = s3.refused();
    if refused == 0 {
        return Err("the S3 server refused no request signed with the wrong secret".into());
    }

    // The first round: every place to both.
    answer_prompt(vm, "$", "clear; elevate backup run nas cloud")?;
    answer_prompt_within(vm, "$", "", 300)?;
    vm.expect_screen(&[
        "backup: nas: /users: ",
        "backup: nas: /applications: ",
        "backup: nas: done, to /volumes/nas/dukovo-backup/",
        "backup: cloud: /system/settings: ",
        &format!("backup: cloud: done, to {bucket}/dukovo-backup/"),
    ])?;

    // Changes, and the second round; the share keeps the newest backup of
    // each place, the bucket both.
    answer_prompt(
        vm,
        "$",
        "clear; cd work; echo 'second draft' >> notes.txt; rm numbers.txt; echo fresh > \
         new.txt; cd; echo changed",
    )?;
    vm.expect_screen(&["changed"])?;
    let second = ["backup run nas cloud | grep done", "backup show nas", "backup show cloud"];
    elevated(vm, "second.sh", &second, false)?;
    vm.expect_screen(&[
        "backup: nas: done",
        "backup: cloud: done",
        "nas: /volumes/nas/dukovo-backup/",
        " (restic)",
        "  /applications:",
        "  /shared:",
        "  /system/settings:",
        "  /users:",
    ])?;
    let screen = vm.screen()?;
    let times: Vec<usize> = screen
        .iter()
        .filter(|line| line.starts_with("    "))
        .map(|line| line.split(", ").count())
        .collect();
    if times != [1, 1, 1, 1, 2, 2, 2, 2] {
        return Err(format!("backups of each place on the share, then in the bucket: {times:?}"));
    }

    let check = ["backup verify --restore nas", "backup verify --restore cloud"];
    elevated(vm, "check.sh", &check, false)?;
    vm.expect_screen(&[
        "backup: nas: checked the repository and read 4 backups and 1 records through, all whole",
        "backup: nas: /applications: restored the backup of ",
        "backup: nas: /users: restored the backup of ",
        "backup: cloud: checked the repository and read 8 backups and 2 records through, all \
         whole",
        "backup: cloud: /shared: restored the backup of ",
        "backup: cloud: /system/settings: restored the backup of ",
        "files and folders as they were",
    ])?;

    // Her note, deleted, comes back from the bucket's backup; /shared
    // from the share's folder with the key alone.
    answer_prompt(vm, "$", "clear; rm work/notes.txt; echo deleted")?;
    vm.expect_screen(&["deleted"])?;
    let restore = [
        "backup restore --place /users cloud",
        "backup restore --key /volumes/nas/recovery.key --place shared \
         /volumes/nas/dukovo-backup/*",
    ];
    elevated(vm, "restore.sh", &restore, false)?;
    vm.expect_screen(&[
        "Restored the backup of users at ",
        " as /snapshots/users/restored-",
        "Restored the backup of shared at ",
        " as /snapshots/shared/restored-",
    ])?;
    answer_prompt(
        vm,
        "$",
        "clear; snap restore --from $(ls /snapshots/users | grep restored-) work/notes.txt; cat \
         work/notes.txt",
    )?;
    vm.expect_screen(&["first draft", "second draft"])?;

    linux_restic(&share, s3.port)?;

    // With the share unmounted, its folder is refused rather than written.
    let unmounted =
        ["umount /volumes/nas && backup run nas; echo \"run $?\"; ls /volumes/nas | wc -l"];
    elevated(vm, "unmounted.sh", &unmounted, false)?;
    vm.expect_screen(&[
        "backup: nas: /volumes/nas/dukovo-backup/",
        "has no backup.toml, which backup add wrote there",
        "run 1",
    ])?;
    if !vm.screen()?.iter().any(|line| line.trim() == "0") {
        return Err("a round wrote in the share's folder while the share was not mounted".into());
    }
    if s3.refused() != refused {
        return Err(format!(
            "the S3 server refused {} requests signed with the right key: see {DIR}/s3/log",
            s3.refused() - refused
        ));
    }

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Linux's restic, with the recovery keys the guest left on the share,
/// reads the share's repository and the bucket's through, counts their
/// snapshots, and prints Alice's note from the newest backup of /users.
fn linux_restic(share: &Path, s3_port: u16) -> Result<(), String> {
    if Command::new("restic").arg("version").output().is_err() {
        println!(
            "    note: restic is not installed; Linux's restic does not read the repositories"
        );
        return Ok(());
    }
    let key = |name: &str| {
        fs::read_to_string(share.join(name))
            .map(|key| key.trim().to_string())
            .map_err(|err| format!("reading the recovery key the guest left: {err}"))
    };
    let folder = share.join("dukovo-backup");
    let machines: Vec<String> = fs::read_dir(&folder)
        .map_err(|err| format!("reading {}: {err}", folder.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    let [machine] = machines.as_slice() else {
        return Err(format!("{} holds {machines:?}, not one machine's folder", folder.display()));
    };
    let repositories = [
        (folder.join(machine).display().to_string(), key("recovery.key")?, 5),
        (
            format!("s3:http://127.0.0.1:{s3_port}/{BUCKET}/dukovo-backup/{machine}"),
            key("cloud.key")?,
            10,
        ),
    ];
    for (repository, key, snapshots) in repositories {
        let restic = |args: &[&str]| -> Result<String, String> {
            let output = Command::new("restic")
                .args(["--repo", &repository, "--no-lock", "--no-cache"])
                .args(args)
                .env("RESTIC_PASSWORD", &key)
                .env("AWS_ACCESS_KEY_ID", KEY_ID)
                .env("AWS_SECRET_ACCESS_KEY", SECRET)
                .env_remove("RESTIC_REPOSITORY")
                .env_remove("RESTIC_PASSWORD_FILE")
                .env_remove("HTTP_PROXY")
                .env_remove("http_proxy")
                .output()
                .map_err(|err| format!("running restic: {err}"))?;
            if !output.status.success() {
                return Err(format!(
                    "restic {} on {repository}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        };
        let checked = restic(&["check", "--read-data"])?;
        if !checked.contains("no errors were found") {
            return Err(format!("restic check on {repository}: {}", checked.trim()));
        }
        let listed = restic(&["snapshots", "--json"])?;
        let found = listed.matches("\"short_id\"").count();
        if found != snapshots {
            return Err(format!("restic finds {found} snapshots in {repository}, not {snapshots}"));
        }
        let note = restic(&["dump", "latest", "--path", "/users", "/users/alice/work/notes.txt"])?;
        if note != "first draft\nsecond draft\n" {
            return Err(format!("restic reads her note in {repository} as {note:?}"));
        }
        println!("    restic read {repository} through: {found} snapshots");
    }
    Ok(())
}

/// The S3 server: one bucket, its objects kept as files in a folder,
/// answering only requests signed by KEY_ID and SECRET with AWS's
/// Signature Version 4. It stops when dropped.
struct S3 {
    port: u16,
    stop: Arc<AtomicBool>,
    server: Arc<Server>,
}

/// What the S3 server's connections share.
struct Server {
    /// The bucket's folder: each object a file, its key the file's path.
    objects: PathBuf,
    /// A line for each request and how it was answered.
    log: Mutex<fs::File>,
    /// How many requests it refused for their signature.
    refused: AtomicUsize,
}

impl S3 {
    /// Listens on a free port of the host's loopback, with the bucket's
    /// objects and the log in `dir`.
    fn start(dir: &Path) -> Result<S3, String> {
        let objects = dir.join(BUCKET);
        fs::create_dir_all(&objects)
            .map_err(|err| format!("making {}: {err}", objects.display()))?;
        let log =
            fs::File::create(dir.join("log")).map_err(|err| format!("making the log: {err}"))?;
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|err| format!("no port for S3: {err}"))?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let server =
            Arc::new(Server { objects, log: Mutex::new(log), refused: AtomicUsize::new(0) });
        let (stopping, serving) = (stop.clone(), server.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let server = serving.clone();
                    std::thread::spawn(move || server.converse(stream));
                }
            }
        });
        Ok(S3 { port, stop, server })
    }

    fn refused(&self) -> usize {
        self.server.refused.load(Ordering::SeqCst)
    }
}

impl Drop for S3 {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// An HTTP request.
struct Request {
    method: String,
    /// The path and the query as sent, still percent-encoded.
    path: String,
    query: String,
    /// The headers, their names in lowercase.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    /// The header `name`'s values, joined by commas as SigV4 joins them.
    fn header(&self, name: &str) -> Option<String> {
        let values: Vec<&str> =
            self.headers.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect();
        (!values.is_empty()).then(|| values.join(","))
    }

    /// The next request on the connection; none once the client closes it.
    fn read(reader: &mut BufReader<TcpStream>) -> Result<Option<Request>, String> {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|err| err.to_string())? == 0 {
            return Ok(None);
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return Err(format!("a request line of {line:?}"));
        };
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mut request = Request {
            method: method.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        loop {
            line.clear();
            reader.read_line(&mut line).map_err(|err| err.to_string())?;
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            let (name, value) =
                line.split_once(':').ok_or_else(|| format!("a header line of {line:?}"))?;
            request.headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
        if request.header("transfer-encoding").is_some() {
            return Err("a body in chunks, which this server does not read".into());
        }
        if request.header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .map_err(|e| e.to_string())?;
        }
        let length: usize = request
            .header("content-length")
            .map_or(Ok(0), |l| l.parse())
            .map_err(|_| "a Content-Length that is not a number".to_string())?;
        request.body = vec![0; length];
        reader.read_exact(&mut request.body).map_err(|err| err.to_string())?;
        Ok(Some(request))
    }
}

/// An HTTP answer.
struct Answer {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn new(status: u16, body: impl Into<Vec<u8>>) -> Answer {
        Answer { status, headers: Vec::new(), body: body.into() }
    }

    /// S3's answer for an error.
    fn error(status: u16, code: &str, message: &str) -> Answer {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{}\
             </Message></Error>",
            escape(message)
        );
        Answer::new(status, xml).with("Content-Type", "application/xml")
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Answer {
        self.headers.push((name, value.into()));
        self
    }

    /// Writes the answer; only its headers when it answers a HEAD.
    fn write(&self, stream: &mut TcpStream, head: bool) -> std::io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            204 => "No Content",
            206 => "Partial Content",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            416 => "Range Not Satisfiable",
            _ => "Not Implemented",
        };
        let mut text = format!("HTTP/1.1 {} {reason}\r\n", self.status);
        for (name, value) in &self.headers {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        text.push_str(&format!("Content-Length: {}\r\n\r\n", self.body.len()));
        stream.write_all(text.as_bytes())?;
        if !head {
            stream.write_all(&self.body)?;
        }
        stream.flush()
    }
}

/// When every object was last changed, as S3's listings and headers say.
const MODIFIED_XML: &str = "2026-01-01T00:00:00.000Z";
const MODIFIED_HTTP: &str = "Thu, 01 Jan 2026 00:00:00 GMT";

impl Server {
    /// Answers the requests on `stream` until the client closes it.
    fn converse(&self, stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
        let Ok(mut writer) = stream.try_clone() else { return };
        let mut reader = BufReader::new(stream);
        loop {
            let request = match Request::read(&mut reader) {
                Ok(Some(request)) => request,
                Ok(None) => return,
                Err(err) => {
                    self.note(&format!("unread: {err}"));
                    let _ = Answer::error(400, "BadRequest", &err).write(&mut writer, false);
                    return;
                }
            };
            let answer = self.answer(&request);
            self.note(&format!(
                "{} {}?{} -> {}",
                request.method, request.path, request.query, answer.status
            ));
            let close =
                request.header("connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
            if answer.write(&mut writer, request.method == "HEAD").is_err() || close {
                return;
            }
        }
    }

    fn note(&self, line: &str) {
        if let Ok(mut log) = self.log.lock() {
            let _ = writeln!(log, "{line}");
        }
    }

    fn answer(&self, request: &Request) -> Answer {
        if let Err((status, code, message)) = check_signature(request, KEY_ID, SECRET) {
            self.refused.fetch_add(1, Ordering::SeqCst);
            self.note(&format!("refused: {code}: {message}"));
            return Answer::error(status, code, &message);
        }
        let path = decode(&request.path);
        let path = path.strip_prefix('/').unwrap_or(&path);
        let (bucket, key) = path.split_once('/').unwrap_or((path, ""));
        if bucket != BUCKET {
            return Answer::error(404, "NoSuchBucket", "The specified bucket does not exist");
        }
        let query = query_pairs(&request.query);
        match (request.method.as_str(), key) {
            ("GET", "") if query.contains_key("location") => Answer::new(
                200,
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LocationConstraint \
                 xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"></LocationConstraint>",
            ),
            ("GET", "") if query.get("list-type").is_some_and(|t| t == "2") => self.list(&query),
            ("HEAD", "") => Answer::new(200, ""),
            ("GET" | "HEAD", key) if !key.is_empty() => {
                self.get(key, request.header("range").filter(|_| request.method == "GET"))
            }
            ("PUT", key) if !key.is_empty() => self.put(key, request),
            ("DELETE", key) if !key.is_empty() => match self.file(key) {
                Some(file) => match fs::remove_file(file) {
                    Ok(()) => Answer::new(204, ""),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Answer::new(204, ""),
                    Err(err) => Answer::error(500, "InternalError", &err.to_string()),
                },
                None => Answer::error(400, "InvalidArgument", "a key this server does not keep"),
            },
            _ => Answer::error(501, "NotImplemented", "not a request this server answers"),
        }
    }

    /// The file holding the object `key`, if the key is one a file can be.
    fn file(&self, key: &str) -> Option<PathBuf> {
        let fine = key
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\0'));
        fine.then(|| self.objects.join(key))
    }

    /// Every object's key and size, in order.
    fn keys(&self) -> Vec<(String, u64)> {
        fn walk(dir: &Path, top: &Path, found: &mut Vec<(String, u64)>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    walk(&path, top, found);
                } else if let Ok(key) = path.strip_prefix(top) {
                    found.push((key.to_string_lossy().into_owned(), meta.len()));
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.objects, &self.objects, &mut found);
        found.sort();
        found
    }

    /// ListObjectsV2: the keys after the token under the prefix, those
    /// sharing a part up to the delimiter as one, PAGE at most.
    fn list(&self, query: &BTreeMap<String, String>) -> Answer {
        let get = |name: &str| query.get(name).map_or("", String::as_str);
        let (prefix, delimiter) = (get("prefix"), get("delimiter"));
        let after = query.get("continuation-token").or(query.get("start-after"));
        let max = get("max-keys").parse().unwrap_or(1000).min(PAGE);
        // Each entry: a key with its size, or a common prefix.
        let mut entries: Vec<(String, Option<u64>)> = Vec::new();
        for (key, size) in self.keys() {
            let Some(rest) = key.strip_prefix(prefix) else { continue };
            match rest.find(delimiter).filter(|_| !delimiter.is_empty()) {
                Some(at) => {
                    let common = key[..prefix.len() + at + delimiter.len()].to_string();
                    if entries.last().is_none_or(|(last, _)| *last != common) {
                        entries.push((common, None));
                    }
                }
                None => entries.push((key, Some(size))),
            }
        }
        entries.retain(|(name, _)| after.is_none_or(|after| name > after));
        let truncated = entries.len() > max;
        entries.truncate(max);
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ListBucketResult \
             xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{BUCKET}</Name>\
             <Prefix>{}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{max}</MaxKeys>\
             <IsTruncated>{truncated}</IsTruncated>",
            escape(prefix),
            entries.len()
        );
        if !delimiter.is_empty() {
            xml.push_str(&format!("<Delimiter>{}</Delimiter>", escape(delimiter)));
        }
        if truncated && let Some((last, _)) = entries.last() {
            xml.push_str(&format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                escape(last)
            ));
        }
        for (name, size) in &entries {
            match size {
                Some(size) => xml.push_str(&format!(
                    "<Contents><Key>{}</Key><LastModified>{MODIFIED_XML}</LastModified>\
                     <ETag>&quot;{}&quot;</ETag><Size>{size}</Size>\
                     <StorageClass>STANDARD</StorageClass></Contents>",
                    escape(name),
                    &hex(&Sha256::digest(name.as_bytes()))[..32]
                )),
                None => xml.push_str(&format!(
                    "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                    escape(name)
                )),
            }
        }
        xml.push_str("</ListBucketResult>");
        Answer::new(200, xml).with("Content-Type", "application/xml")
    }

    /// An object, or the bytes of it a Range header asks for.
    fn get(&self, key: &str, range: Option<String>) -> Answer {
        let Some(file) = self.file(key) else {
            return Answer::error(400, "InvalidArgument", "a key this server does not keep");
        };
        let Ok(data) = fs::read(&file) else {
            return Answer::error(404, "NoSuchKey", "The specified key does not exist.");
        };
        let size = data.len() as u64;
        let answer = |status, body: &[u8]| {
            Answer::new(status, body)
                .with("Content-Type", "application/octet-stream")
                .with("ETag", format!("\"{}\"", etag(&data)))
                .with("Last-Modified", MODIFIED_HTTP)
                .with("Accept-Ranges", "bytes")
        };
        let Some(range) = range else { return answer(200, &data) };
        let asked = range.strip_prefix("bytes=").and_then(|r| r.split_once('-')).and_then(
            |(first, last)| match (first.parse::<u64>(), last.parse::<u64>()) {
                (Ok(first), Ok(last)) if first <= last => Some((first, last.min(size - 1))),
                (Ok(first), Err(_)) if last.is_empty() => Some((first, size.saturating_sub(1))),
                (Err(_), Ok(count)) if first.is_empty() => {
                    Some((size.saturating_sub(count), size.saturating_sub(1)))
                }
                _ => None,
            },
        );
        match asked {
            Some((first, last)) if first < size => {
                answer(206, &data[first as usize..=last as usize])
                    .with("Content-Range", format!("bytes {first}-{last}/{size}"))
            }
            _ => Answer::error(416, "InvalidRange", "The requested range is not satisfiable"),
        }
    }

    /// PutObject, or CopyObject when the request names a source.
    fn put(&self, key: &str, request: &Request) -> Answer {
        let Some(file) = self.file(key) else {
            return Answer::error(400, "InvalidArgument", "a key this server does not keep");
        };
        let (data, copied) = match request.header("x-amz-copy-source") {
            Some(source) => {
                let source = decode(&source);
                let source = source.trim_start_matches('/');
                let from = source
                    .strip_prefix(BUCKET)
                    .and_then(|rest| rest.strip_prefix('/'))
                    .and_then(|key| self.file(key));
                match from.map(fs::read) {
                    Some(Ok(data)) => (data, true),
                    _ => {
                        return Answer::error(
                            404,
                            "NoSuchKey",
                            "The specified key does not exist.",
                        );
                    }
                }
            }
            None => (request.body.clone(), false),
        };
        let written = file.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| {
            let partial = file.with_extension("s3-partial");
            fs::write(&partial, &data).and_then(|()| fs::rename(&partial, &file))
        });
        if let Err(err) = written {
            return Answer::error(500, "InternalError", &err.to_string());
        }
        let tag = etag(&data);
        if copied {
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<CopyObjectResult><LastModified>\
                 {MODIFIED_XML}</LastModified><ETag>&quot;{tag}&quot;</ETag></CopyObjectResult>"
            );
            return Answer::new(200, xml).with("Content-Type", "application/xml");
        }
        Answer::new(200, "").with("ETag", format!("\"{tag}\""))
    }
}

/// Checks a request's AWS Signature Version 4 against the key `id` and
/// `secret`: its scope, that it signs the host, the date and the body's
/// hash, the body against that hash, and the signature. What S3 answers
/// when one fails.
fn check_signature(
    request: &Request,
    id: &str,
    secret: &str,
) -> Result<(), (u16, &'static str, String)> {
    let refused = |message: &str| (403, "AccessDenied", message.to_string());
    let authorization = request.header("authorization").ok_or_else(|| refused("unsigned"))?;
    let fields = authorization
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or_else(|| refused("not signed with AWS4-HMAC-SHA256"))?;
    let field = |name: &str| {
        fields.split(',').map(str::trim).find_map(|f| f.strip_prefix(name)).map(str::to_string)
    };
    let (Some(credential), Some(signed), Some(signature)) =
        (field("Credential="), field("SignedHeaders="), field("Signature="))
    else {
        return Err(refused("an Authorization header without its three parts"));
    };
    let scope: Vec<&str> = credential.split('/').collect();
    let [key_id, day, region, "s3", "aws4_request"] = scope.as_slice() else {
        return Err(refused("a credential scope that is not S3's"));
    };
    if *key_id != id {
        return Err((403, "InvalidAccessKeyId", "The access key Id does not exist".into()));
    }
    let date = request.header("x-amz-date").ok_or_else(|| refused("no x-amz-date"))?;
    if !date.starts_with(day) {
        return Err(refused("an x-amz-date of another day than the scope's"));
    }
    let payload =
        request.header("x-amz-content-sha256").ok_or_else(|| refused("no x-amz-content-sha256"))?;
    if payload.starts_with("STREAMING-") {
        return Err((501, "NotImplemented", "bodies signed in chunks".into()));
    }
    if payload != "UNSIGNED-PAYLOAD" && payload != hex(&Sha256::digest(&request.body)) {
        return Err((400, "XAmzContentSHA256Mismatch", "the body is not the one hashed".into()));
    }
    let names: Vec<&str> = signed.split(';').collect();
    for needed in ["host", "x-amz-date", "x-amz-content-sha256"] {
        if !names.contains(&needed) {
            return Err(refused(&format!("{needed} is not signed")));
        }
    }
    let mut headers = String::new();
    for name in &names {
        let value = request.header(name).unwrap_or_default();
        headers.push_str(&format!(
            "{name}:{}\n",
            value.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    let canonical = format!(
        "{}\n{}\n{}\n{headers}\n{signed}\n{payload}",
        request.method,
        encode(&decode(&request.path), false),
        canonical_query(&request.query)
    );
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{date}\n{day}/{region}/s3/aws4_request\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let mut key = hmac(format!("AWS4{secret}").as_bytes(), day.as_bytes());
    for part in [region, "s3", "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    if hex(&hmac(&key, to_sign.as_bytes())) != signature {
        return Err((
            403,
            "SignatureDoesNotMatch",
            "The request signature we calculated does not match the signature you provided.".into(),
        ));
    }
    Ok(())
}

/// HMAC-SHA256 (RFC 2104).
fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new().chain_update(block.map(|b| b ^ 0x36)).chain_update(data).finalize();
    Sha256::new().chain_update(block.map(|b| b ^ 0x5c)).chain_update(inner).finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        text.push(char::from(DIGITS[usize::from(b >> 4)]));
        text.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    text
}

/// An object's ETag: the first half of its SHA-256, the length of S3's.
fn etag(data: &[u8]) -> String {
    hex(&Sha256::digest(data))[..32].to_string()
}

/// `text` with every byte but the unreserved ones percent-encoded, as
/// SigV4 encodes; slashes too only when `slashes` says so.
fn encode(text: &str, slashes: bool) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) || (byte == b'/' && !slashes) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `text` with its percent-encoded bytes decoded.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes.get(at + 1..at + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[at], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// A query's names and values, decoded.
fn query_pairs(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

/// A query as SigV4 signs it: each name and value encoded again, in order.
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (encode(&decode(name), true), encode(&decode(value), true))
        })
        .collect();
    pairs.sort();
    pairs.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&")
}

/// `text` with XML's special characters escaped.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS's own example of a signed GET of part of an object, from the
    /// S3 API reference's "Signature Calculations for the Authorization
    /// Header", passes; changed, it is refused.
    #[test]
    fn the_signature_check_passes_aws_example() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let mut request = Request {
            method: "GET".into(),
            path: "/test.txt".into(),
            query: String::new(),
            headers: vec![
                ("host".into(), "examplebucket.s3.amazonaws.com".into()),
                ("range".into(), "bytes=0-9".into()),
                ("x-amz-content-sha256".into(), empty.into()),
                ("x-amz-date".into(), "20130524T000000Z".into()),
                (
                    "authorization".into(),
                    "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/\
                     aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,\
                     Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
                        .into(),
                ),
            ],
            body: Vec::new(),
        };
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        assert_eq!(check_signature(&request, "AKIAIOSFODNN7EXAMPLE", secret), Ok(()));
        assert_eq!(
            check_signature(&request, "AKIAIOSFODNN7EXAMPLE", "wrong").unwrap_err().1,
            "SignatureDoesNotMatch"
        );
        assert_eq!(check_signature(&request, "OTHER", secret).unwrap_err().1, "InvalidAccessKeyId");
        request.headers[1].1 = "bytes=0-10".into();
        assert_eq!(
            check_signature(&request, "AKIAIOSFODNN7EXAMPLE", secret).unwrap_err().1,
            "SignatureDoesNotMatch"
        );
        request.headers[1].1 = "bytes=0-9".into();
        request.body = b"more".to_vec();
        assert_eq!(
            check_signature(&request, "AKIAIOSFODNN7EXAMPLE", secret).unwrap_err().1,
            "XAmzContentSHA256Mismatch"
        );
    }

    #[test]
    fn queries_are_signed_as_aws_signs_them() {
        assert_eq!(
            canonical_query("prefix=a%2Fb&list-type=2&max-keys=10&location"),
            "list-type=2&location=&max-keys=10&prefix=a%2Fb"
        );
        assert_eq!(encode(&decode("/dukovo/a%20b/c"), false), "/dukovo/a%20b/c");
        assert_eq!(decode("100%"), "100%");
    }
}
