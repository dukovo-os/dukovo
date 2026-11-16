// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Alerts of the web console, Dukovo Admin: the email account, the
//! webhooks and the browsers that alerts are sent to, which alice, the
//! owner and an administrator, sets up and takes away through the
//! console's actions, and what alertd(8) then sends to them (ADR 0485, ADR
//! 0566). bob may use the console and is no administrator.
//!
//! Two programs of the test stand where the services of the internet
//! would. *monitoring-test*, the *monitoring* test's program
//! (userspace/programs/monitoring-test), runs as a service on the machine
//! and listens on 127.0.0.1 for a mail server on port 587, which offers
//! STARTTLS and takes one account, and for a webhook receiver on port
//! 8080; they write what they are sent to /shared/monitoring. A browser's
//! push service is *push-receiver.py* on the host
//! (userspace/spark/tests/services/admin-alerts), in HTTPS with a
//! certificate from an authority made here with openssl, which the machine
//! reaches at 10.0.2.2. The machine trusts that authority as
//! dukovo-certificates(7) says, by adding it to the file of roots, and
//! restarts alertd, which reads the roots as it starts. The receiver makes
//! the keys of a browser that does not exist and the subscription it would
//! give, answers 201, and keeps each push as a file: what it decrypted of
//! it as RFC 8291 says, the headers that matter, and whether the token of
//! its Authorization header is RFC 8292's and signed by the key.
//!
//! The first steps are the console's and bob's. alertd has no channel:
//! *alerts.channels* is an empty list, and for bob it is ADM-122 and for
//! alice with her session locked ADM-106, as *alerts.email* and
//! *alerts.test* are. *monitoring-test setup* then starts the receivers
//! and makes the machine trust the mail server's authority, the push
//! service's authority is added to the roots, and alerts.toml is made
//! with its push section alone, which lets a push service be on the
//! network of 10.0.2.2.
//!
//! The email account. *alerts.email* with the mail server's account, whose
//! password goes through the input: the channel is listed with its kind
//! and its host and never its password, alerts.toml is root's at mode 600
//! and keeps the push section, *alerts.test* sends to it, and the mail
//! server heard the test over TLS as the account it takes, to the address
//! named. *alerts.email-remove* takes it away.
//!
//! The webhook and the kinds. *alerts.webhook-add* of the json kind with a
//! token, told of hardware alone: *alerts.channels* has its kind and its
//! number. Three alerts are raised at the console, one of each kind of
//! alert, data at risk, hardware and none; the receiver is sent the
//! hardware one and the one of no kind in one message, within the 30
//! seconds alertd waits, and never the one about data. *alerts.channel-about*
//! with every kind leaves no *about* in the file and in the channel. An
//! alerts.toml written by hand with an *about* that names a kind and one
//! that is no kind has ALR-106 in the log, and the channel is told of the
//! kind. *alerts.webhook-remove* takes it away.
//!
//! The browser. *alerts.push-key* gives 65 bytes in base64url, the same
//! after alertd restarts. *alerts.push-add* with the subscription of the
//! browser the receiver made up: the receiver is sent one push when an
//! alert is raised, which it decrypts to the message the page describes,
//! with a token that verifies with the key and names the receiver's own
//! origin, and for a critical alert, Urgency high. When it answers 410 the
//! subscription is gone, the history has the event *push-ended* and the
//! log ALR-104. An endpoint on a private network that alerts.toml does not
//! list is refused, and so is one in http.
//!
//! Last, *dukovo-check* with a channel that every attempt failed at, a
//! webhook the receiver answers 403, says SEC-170 for the alerts line, and
//! with the channel taken away says that no channel is set up. What the
//! password, the token and the browser's secrets are is in no answer of the
//! console, nor in admind's log, authd's audit, logd's records, the
//! journal's rows or the tasks. The power button ends it.
//!
//! What the pane's commands print reaches the host through nc, to a port
//! on the host's loopback that QEMU's user network gives the guest at
//! 10.0.2.2.

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::console_client::{ALICE_PASSWORD, Browser, Console, Pane, Reply, elevated};
use super::super::*;

/// The test.
pub const CASE: Case = Case {
    name: "admin-alerts",
    about: "The web console's Alerts: alerts.channels is an empty list at first, ADM-122 for bob and ADM-106 for alice locked, as alerts.email and alerts.test are; alerts.email with the mail server's account, its password through the input, lists the channel with its kind and host and never the password, leaves alerts.toml root's at mode 600 with its push section, alerts.test reaches the mail server over TLS as the account it takes, and alerts.email-remove takes it away; alerts.webhook-add of the json kind with a token told of hardware alone lists its kind and number, of three alerts raised, one of each kind and one of none, the receiver is sent the hardware one and the one of none and never the one about data, alerts.channel-about with every kind leaves no about, an about written by hand that names a kind and one that is none has ALR-106 and the kind is told of, and alerts.webhook-remove takes it away; alerts.push-key is 65 bytes and the same after alertd restarts, alerts.push-add of a browser's subscription is sent one push for an alert by a push service on the host in HTTPS, which decrypts it as RFC 8291 says to the message of the page, with a token RFC 8292's that verifies with the key and names its origin, and Urgency high for a critical one, a push service that answers 410 ends the subscription with the event push-ended in the history and ALR-104 in the log, an endpoint on a private network alerts.toml does not list and one in http are refused; dukovo-check says SEC-170 with a webhook that is refused and the passing line with none; no secret is in an answer, the log, the audit, the journal or the tasks (ADR 0485, ADR 0566)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, admind and alertd
    // among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", "user,id=n0", "-device", "virtio-net-pci,netdev=n0"],
    script: alerts,
    disks: &[],
    second: None,
};

/// The password of bob, whom alice makes: he may use the console, and is
/// no administrator.
const BOB_PASSWORD: &str = "looking-glass";

/// The test's program that holds the mail server and the webhook receiver.
const MONITORING: &str = "/system/tests/posix/monitoring-test";

/// alerts.toml, the file of roots programs read, and the file the push
/// service's authority is kept in.
const SETTINGS: &str = "/system/settings/alerts.toml";
const ROOTS: &str = "/system/settings/ssl/certs/ca-certificates.crt";
const AUTHORITY: &str = "/system/settings/ssl/push-test-ca.pem";

/// Where the test keeps its files on the host, from the top of the tree,
/// and the push service, from the top of the tree.
const DIR: &str = "out/test/admin-alerts";
const RECEIVER: &str = "userspace/spark/tests/services/admin-alerts/push-receiver.py";

/// The account the mail server takes, and the token a webhook is given.
const MAIL_USER: &str = "alerts@example.test";
const MAIL_PASSWORD: &str = "tea party";
const WEBHOOK_TOKEN: &str = "json-token";

/// The address of the webhook receiver, and of its path that is answered
/// 403.
const HOOK: &str = "http://127.0.0.1:8080/json-hook";
const DENIED: &str = "http://127.0.0.1:8080/denied";

/// The endpoint of the browser that is made up, as the receiver's
/// subscription has it, and what is in that address and no other.
const ENDPOINT_PATH: &str = "/push/lab-browser";

/// How long alertd waits before it sends, with the time to send and to
/// read the result, in seconds.
const SEND_SECONDS: u64 = 100;

/// How many times a second a wait looks, as a pause in milliseconds.
const LOOK_MS: u64 = 500;

/// Who asks the console.
#[derive(Clone, Copy)]
enum Who {
    Alice,
    Bob,
}

/// What the test keeps from one step to the next.
struct Test {
    console: Console,
    pane: Pane,
    alice: Browser,
    bob: Browser,
    host: Host,
    /// The authentication secret of the receiver's browser, which is no
    /// one's but the subscription's.
    auth: String,
    /// The ID alertd gave the browser once it was added.
    browser: String,
}

impl Test {
    /// Opens the console and the pane once the machine has its network and
    /// admind answers.
    fn open(vm: &mut Vm, host: Host) -> Result<Test, String> {
        vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
        vm.expect_anywhere("admind: answers at ")?;
        let auth = host.subscription()?["keys"]["auth"]
            .as_str()
            .ok_or("the subscription has no authentication secret")?
            .to_string();
        Ok(Test {
            console: Console::open(vm)?,
            pane: Pane::open()?,
            alice: Browser::default(),
            bob: Browser::default(),
            host,
            auth,
            browser: String::new(),
        })
    }

    /// Signs alice and bob in, as the console's page does, each with a
    /// session of its own, locked.
    fn sign_in(&mut self) -> Result<(), String> {
        self.alice = Browser::default();
        self.console.sign_in(&mut self.alice, "alice", ALICE_PASSWORD)?;
        self.console.read_session(&mut self.alice)?;
        self.bob = Browser::default();
        self.console.sign_in(&mut self.bob, "bob", BOB_PASSWORD)?;
        self.console.read_session(&mut self.bob)?;
        Ok(())
    }

    /// A read of `path` for `who`, whatever the answer.
    fn get(&mut self, who: Who, path: &str) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.get(browser, path)
    }

    /// What `path` says to `who`, which must be 200.
    fn read(&mut self, who: Who, path: &str) -> Result<Value, String> {
        self.get(who, path)?.must(path, 200)?.json()
    }

    /// A change at `path` with `body`, for `who`, whatever the answer.
    fn post(&mut self, who: Who, path: &str, body: &Value) -> Result<Reply, String> {
        let console = &self.console;
        let browser = match who {
            Who::Alice => &mut self.alice,
            Who::Bob => &mut self.bob,
        };
        console.post(browser, path, body)
    }

    /// A change alice asks for, which must be done: the answer says so,
    /// and no secret is in it.
    fn change(&mut self, path: &str, body: &Value) -> Result<Value, String> {
        let reply = self.post(Who::Alice, path, body)?.must(path, 200)?;
        self.free_of_secrets(path, &reply.body)?;
        let done = reply.json()?;
        if done["done"] != json!(true) {
            return Err(format!("{path} was answered {done}, which does not say it is done"));
        }
        Ok(done)
    }

    /// The first line of a change's output, which the command printed as
    /// JSON, as JSON.
    fn printed(done: &Value) -> Result<Value, String> {
        let line = done["output"][0].as_str().ok_or_else(|| format!("{done} has no output"))?;
        serde_json::from_str(line).map_err(|err| format!("the command printed {line:?}: {err}"))
    }

    /// Unlocks alice's session with her password, if it is locked.
    fn unlocked(&mut self) -> Result<(), String> {
        let session =
            self.console.get(&mut self.alice, "/api/1/session")?.must("her session", 200)?;
        if session.json()?["unlock_seconds"].as_u64().is_some_and(|left| left > 120) {
            return Ok(());
        }
        let unlock = json!({"kind": "password", "password": ALICE_PASSWORD});
        self.console.post(&mut self.alice, "/api/1/unlock", &unlock)?.must("her unlocking", 200)?;
        Ok(())
    }

    /// Locks alice's session, as the page's Lock does.
    fn locked(&mut self) -> Result<(), String> {
        self.post(Who::Alice, "/api/1/lock", &json!({}))?.must("alice's lock", 200).map(drop)
    }

    /// A change that bob may not ask for, ADM-122, and alice may not while
    /// her session is locked, ADM-106: she is locked first, and the caller
    /// unlocks her again.
    fn refused_for_both(&mut self, path: &str, body: &Value) -> Result<(), String> {
        self.locked()?;
        self.post(Who::Bob, path, body)?.refused(&format!("{path} asked by bob"), 403, "ADM-122")?;
        let locked = self.post(Who::Alice, path, body)?;
        locked.refused(&format!("{path} asked by alice, locked"), 403, "ADM-106")?;
        Ok(())
    }

    /// A read that runs as an administrator, which bob may not ask for,
    /// ADM-122, and alice may not while her session is locked, ADM-106.
    fn read_refused_for_both(&mut self, path: &str) -> Result<(), String> {
        self.locked()?;
        self.get(Who::Bob, path)?.refused(&format!("{path} read by bob"), 403, "ADM-122")?;
        let locked = self.get(Who::Alice, path)?;
        locked.refused(&format!("{path} read by alice, locked"), 403, "ADM-106")?;
        Ok(())
    }

    /// What must be in no answer of the console and no record: the mail
    /// account's password, the browser's secret and its endpoint's path.
    fn secrets(&self) -> [&str; 3] {
        [MAIL_PASSWORD, self.auth.as_str(), ENDPOINT_PATH]
    }

    /// An error if `text`, which is what `what` says, holds a secret, or
    /// the token and the path of the webhook, which no answer says either.
    fn free_of_secrets(&self, what: &str, text: &str) -> Result<(), String> {
        let hook = ["json-hook", WEBHOOK_TOKEN];
        for secret in self.secrets().into_iter().chain(hook) {
            if text.contains(secret) {
                return Err(format!("{what} holds {secret:?}, which is a secret"));
            }
        }
        Ok(())
    }

    /// The channels alertd lists, once `ready` is true of them, within
    /// `seconds`: alertd takes alerts.toml again some moments after a change
    /// of it, and answers after it has started.
    fn channels_where(
        &mut self,
        seconds: u64,
        what: &str,
        ready: impl Fn(&[Value]) -> bool,
    ) -> Result<Vec<Value>, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let reply = self.get(Who::Alice, "/api/1/alerts/channels")?;
            let last = if reply.status == 200 {
                self.free_of_secrets("alerts.channels", &reply.body)?;
                let list = reply.json()?;
                let channels = list.as_array().ok_or_else(|| format!("{list} is not a list"))?;
                if ready(channels) {
                    return Ok(channels.clone());
                }
                format!("the channels are {list}")
            } else {
                format!("it was answered {} {}", reply.status, reply.body)
            };
            if Instant::now() >= deadline {
                return Err(format!("{what}: after {seconds} s {last}"));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// No channel is listed, within `seconds`.
    fn no_channel(&mut self, seconds: u64, what: &str) -> Result<(), String> {
        self.channels_where(seconds, what, <[Value]>::is_empty).map(drop)
    }

    /// What `command` prints in the pane.
    fn said(&self, vm: &mut Vm, command: &str) -> Result<String, String> {
        self.pane.output(vm, command)
    }

    /// How many times the receiver `kind` was sent `phrase`, as the line
    /// that `monitoring-test seen` prints says, or why it says none.
    fn heard(said: &str, phrase: &str, kind: &str) -> Result<u32, String> {
        let line = said
            .lines()
            .find(|line| line.starts_with(&format!("seen {phrase}:")))
            .ok_or_else(|| format!("`seen` has no line for {phrase}: {said:?}"))?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|word| *word == kind);
        at.and_then(|at| words.get(at + 1)).and_then(|count| count.parse().ok()).ok_or_else(|| {
            format!("the line {line:?} does not say how many times {kind} was sent {phrase}")
        })
    }
}

/// Waits until `ready` holds, for up to `seconds`.
fn within(seconds: u64, what: &str, ready: impl Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        if Instant::now() >= deadline {
            return Err(format!("{what} did not happen within {seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(LOOK_MS));
    }
    Ok(())
}

/// Waits until the screen shows each of `needles`, for up to `seconds`.
fn shown_within(vm: &mut Vm, needles: &[&str], seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match vm.expect_screen(needles) {
            Err(_) if Instant::now() < deadline => {}
            done => return done,
        }
    }
}

/// Types `command` at alice's prompt, on a cleared screen, and checks that
/// the screen shows each of `shown` once it has run: up to `seconds`. Each
/// of `shown` is output, never in the command itself, so that the command's
/// own echo is not taken for its answer.
fn run(vm: &mut Vm, command: &str, seconds: u64, shown: &[&str]) -> Result<(), String> {
    answer_prompt(vm, "$", &format!("clear; {command}"))?;
    shown_within(vm, shown, seconds).map_err(|err| format!("`{command}`: {err}"))
}

/// A port of the host's loopback that nothing listens on.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|err| format!("no free port: {err}"))
}

/// Runs openssl in `dir`.
fn openssl(dir: &Path, args: &[&str]) -> Result<(), String> {
    host_tool(Command::new("openssl").current_dir(dir).args(args)).map(|_| ())
}

/// Makes, in `dir`, an authority of the test's own and a certificate it
/// signs for 10.0.2.2, which is how the machine names the host. The keys
/// are P-256, as quick to make as any, good for two days, and protect
/// nothing.
fn authority(dir: &Path) -> Result<(), String> {
    for key in ["ca.key", "server.key"] {
        openssl(
            dir,
            &["genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", key],
        )?;
    }
    openssl(
        dir,
        &[
            "req",
            "-x509",
            "-new",
            "-key",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "2",
            "-subj",
            "/CN=Alerts test authority",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign",
        ],
    )?;
    let extensions = "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n\
                      extendedKeyUsage=serverAuth\nsubjectAltName=IP:10.0.2.2\n";
    fs::write(dir.join("server.ext"), extensions).map_err(|err| format!("server.ext: {err}"))?;
    let request = ["req", "-new", "-key", "server.key", "-subj", "/CN=10.0.2.2"];
    openssl(dir, &[&request[..], &["-out", "server.csr"]].concat())?;
    let signing = ["x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key"];
    openssl(
        dir,
        &[
            &signing[..],
            &["-CAcreateserial", "-days", "2", "-extfile", "server.ext", "-out", "server.pem"],
        ]
        .concat(),
    )?;
    Ok(())
}

/// The push service and a file server beside it, on the host's loopback,
/// which the machine reaches as 10.0.2.2. Both end with it.
struct Host {
    dir: PathBuf,
    /// The push service's port, and the file server's, which gives the
    /// machine the authority's certificate.
    port: u16,
    files_port: u16,
    files: Child,
    receiver: Option<Child>,
}

impl Drop for Host {
    fn drop(&mut self) {
        for child in self.receiver.iter_mut().chain([&mut self.files]) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Host {
    /// Makes the authority and starts both servers, keeping what they say in
    /// `DIR`, from the top of the tree, and waits until the push service
    /// says it listens: it has made the browser's subscription by then.
    fn start() -> Result<Host, String> {
        let root = std::env::current_dir().map_err(|err| err.to_string())?;
        let dir = root.join(DIR);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|err| format!("removing {}: {err}", dir.display()))?;
        }
        let web = dir.join("web");
        fs::create_dir_all(&web).map_err(|err| format!("making {}: {err}", web.display()))?;
        authority(&dir)?;
        fs::copy(dir.join("ca.pem"), web.join("ca.pem")).map_err(|err| format!("ca.pem: {err}"))?;
        let (port, files_port) = (free_port()?, free_port()?);
        let log = fs::File::create(dir.join("files.log")).map_err(|err| err.to_string())?;
        let files = Command::new("python3")
            .args(["-I", "-m", "http.server", &files_port.to_string(), "--bind", "127.0.0.1"])
            .arg("--directory")
            .arg(&web)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|err| err.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|err| format!("starting the file server: {err}"))?;
        let mut host = Host { dir, port, files_port, files, receiver: None };
        within(10, "the file server's listening", || {
            TcpStream::connect(("127.0.0.1", files_port)).is_ok()
        })?;
        host.start_receiver()?;
        Ok(host)
    }

    /// Where the push service keeps its files.
    fn kept_dir(&self) -> PathBuf {
        self.dir.join("kept")
    }

    /// Runs the push service and waits until it says it listens.
    fn start_receiver(&mut self) -> Result<(), String> {
        let log = self.dir.join("receiver.log");
        let output = fs::File::create(&log).map_err(|err| format!("{}: {err}", log.display()))?;
        let child = Command::new("python3")
            .args(["-I", RECEIVER, "serve", "--bind", "127.0.0.1", "--host", "10.0.2.2"])
            .args(["--port", &self.port.to_string()])
            .arg("--cert")
            .arg(self.dir.join("server.pem"))
            .arg("--key")
            .arg(self.dir.join("server.key"))
            .arg("--dir")
            .arg(self.kept_dir())
            .stdin(Stdio::null())
            .stdout(output.try_clone().map_err(|err| err.to_string())?)
            .stderr(output)
            .spawn()
            .map_err(|err| format!("starting the push service: {err}"))?;
        self.receiver = Some(child);
        within(20, "the push service's readiness", || {
            fs::read_to_string(&log).is_ok_and(|said| said.contains("push receiver ready"))
        })
    }

    /// The subscription of the browser that the push service made up.
    fn subscription(&self) -> Result<Value, String> {
        let file = self.kept_dir().join("subscription.json");
        let text = fs::read_to_string(&file).map_err(|err| format!("{}: {err}", file.display()))?;
        serde_json::from_str(&text).map_err(|err| format!("{}: {err}", file.display()))
    }

    /// Has the push service answer `status` from now on, as a push service
    /// does for a browser that has ended its subscription.
    fn answers(&self, status: u16) -> Result<(), String> {
        let file = self.kept_dir().join("status");
        fs::write(&file, format!("{status}\n")).map_err(|err| format!("{}: {err}", file.display()))
    }

    /// Every push the service has kept, in the order they came.
    fn pushes(&self) -> Vec<Value> {
        let Ok(entries) = fs::read_dir(self.kept_dir()) else { return Vec::new() };
        let mut numbers: Vec<u32> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| is_a_push(name))
            .filter_map(|name| name.trim_end_matches(".json").parse().ok())
            .collect();
        numbers.sort_unstable();
        let read = |number: &u32| {
            let text = fs::read_to_string(self.kept_dir().join(format!("{number}.json"))).ok()?;
            serde_json::from_str(&text).ok()
        };
        numbers.iter().filter_map(read).collect()
    }

    /// The push of the alert `key`, once it has come: the service has the
    /// alert's key from what it decrypted, so a push it could not read is
    /// not found, and is said in the failure.
    fn push_of(&self, key: &str) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(SEND_SECONDS);
        loop {
            let pushes = self.pushes();
            if let Some(found) = pushes.iter().find(|push| push["plain"]["first"]["key"] == key) {
                return Ok(found.clone());
            }
            if Instant::now() >= deadline {
                let kept: Vec<String> = pushes.iter().map(summary).collect();
                return Err(format!(
                    "the push service was not sent the alert {key} within {SEND_SECONDS} s; it \
                     was sent {kept:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(LOOK_MS));
        }
    }

    /// The keys of the alerts the service has been sent, in order.
    fn keys(&self) -> Vec<String> {
        self.pushes().iter().map(summary).collect()
    }
}

/// What a push the service kept is told by in a failure: the key of its
/// alert, or why the service could not read it.
fn summary(push: &Value) -> String {
    match push["plain"]["first"]["key"].as_str() {
        Some(key) => key.to_string(),
        None => format!("unreadable: {}", push["plain_error"]),
    }
}

/// Whether `name` is the file of a push the service kept: a number and
/// `.json`.
fn is_a_push(name: &str) -> bool {
    let number = name.strip_suffix(".json").unwrap_or_default();
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Makes bob, in group console and no administrator, once the owner alice
/// is signed in at the console, and starts the console's sessions for
/// alice and bob; alice's pane is elevated.
fn people(vm: &mut Vm, host: Host) -> Result<Test, String> {
    let mut test = Test::open(vm, host)?;
    elevated(vm)?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'echo {BOB_PASSWORD} | user add bob; user set --yes --console \
             bob'"
        ),
    )?;
    vm.expect("authd: changed the account bob")?;
    vm.expect_screen(&["Made bob"])?;
    test.sign_in()?;
    Ok(test)
}

fn alerts(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    let host = Host::start()?;
    let mut test = people(vm, host)?;

    nothing_is_set_up(&mut test)?;
    the_receivers(vm, &mut test)?;
    the_email_account(vm, &mut test)?;
    a_webhook_and_the_kinds(vm, &mut test)?;
    a_browser(vm, &mut test)?;
    a_browser_that_ends_its_subscription(vm, &mut test)?;
    endpoints_that_are_refused(&mut test)?;
    the_check(vm, &mut test)?;
    secrets_are_kept_nowhere(vm, &mut test)?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Step 1: alertd has no channel, which bob and a locked alice are told
/// they may not ask.
fn nothing_is_set_up(test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    test.no_channel(60, "alerts.channels with nothing set up")?;
    if !test.read(Who::Alice, "/api/1/alerts")?.is_array() {
        return Err("alerts.list is no list".into());
    }
    let email = json!({"to": "ops@example.test", "server": "127.0.0.1"});
    test.read_refused_for_both("/api/1/alerts/channels")?;
    test.refused_for_both("/api/1/alerts/email", &email)?;
    test.refused_for_both("/api/1/alerts/test", &json!({}))?;
    test.unlocked()
}

/// *monitoring-test setup* starts the mail server and the webhook receiver
/// and makes the machine trust their authority, and writes the alerts.toml
/// that names them; the machine then also trusts the push service's
/// authority, which it fetches from the host, and alerts.toml is made
/// again to name no channel but to let a push service be on the host's
/// network. alertd starts again to read the roots.
fn the_receivers(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    run(
        vm,
        &format!("elevate {MONITORING} setup; echo \"setup $?\""),
        90,
        &["monitoring-test: the receivers listen, and alerts.toml names them", "setup 0"],
    )?;
    let fetch = format!(
        "elevate sh -c 'curl -sf http://10.0.2.2:{}/ca.pem > {AUTHORITY} && echo >> {ROOTS} && \
         cat {AUTHORITY} >> {ROOTS}'; echo \"trusted $?\"",
        test.host.files_port
    );
    run(vm, &fetch, 60, &["trusted 0"])?;
    let settings = format!(
        "elevate sh -c 'printf \"[push]\\nprivate = [\\\"10.0.2.0/24\\\"]\\n\" > {SETTINGS} && \
         chmod 600 {SETTINGS} && service alertd restart'; sleep 3; echo \"settings $?\""
    );
    run(vm, &settings, 60, &["settings 0"])?;
    test.no_channel(60, "alerts.channels with a push section alone")
}

/// Step 1, the rest: the email account with the mail server's, its test,
/// and its removal.
fn the_email_account(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let body = json!({
        "to": "ops@example.test",
        "server": "127.0.0.1",
        "user": MAIL_USER,
        "from": MAIL_USER,
        "password": MAIL_PASSWORD,
    });
    let done = test.change("/api/1/alerts/email", &body)?;
    if done["changed"] != json!(true) {
        return Err(format!("alerts.email says {done}, which is not a change"));
    }
    let channels = test.channels_where(60, "the email account listed", |all| all.len() == 1)?;
    let email = &channels[0];
    let wanted = [
        ("channel", json!("email to ops@example.test")),
        ("kind", json!("email")),
        ("host", json!("127.0.0.1")),
        ("severity", json!("warning")),
        ("cleared", json!(true)),
    ];
    for (key, value) in wanted {
        if email[key] != value {
            return Err(format!("the email channel has {key} {}, not {value}: {email}", email[key]));
        }
    }

    // alerts.toml is root's alone, has the push section the test wrote, and
    // holds the password, as the file must.
    let said = test.said(
        vm,
        &format!(
            "elevate ls -ln {SETTINGS} > /tmp/alerts.txt; elevate grep -c '^private' {SETTINGS} \
             >> /tmp/alerts.txt; elevate grep -c 'tea party' {SETTINGS} >> /tmp/alerts.txt; cat \
             /tmp/alerts.txt"
        ),
    )?;
    let lines: Vec<&str> = said.lines().map(str::trim).collect();
    let owner = lines.first().map(|line| line.split_whitespace().collect::<Vec<_>>());
    let root_alone = owner.is_some_and(|words| {
        let mode = words.first().is_some_and(|mode| mode.starts_with("-rw-------"));
        mode && words.get(2) == Some(&"0")
    });
    if !root_alone || lines.get(1) != Some(&"1") || lines.get(2) != Some(&"1") {
        return Err(format!("alerts.toml is not root's alone with its push section: {said:?}"));
    }

    // The test reaches it, as the account the mail server takes.
    let done = test.change("/api/1/alerts/test", &json!({}))?;
    let printed = Test::printed(&done)?;
    let sent = printed["deliveries"].as_array().ok_or_else(|| format!("{printed} has no list"))?;
    let [only] = &sent[..] else { return Err(format!("alerts.test sent to {sent:?}, not one")) };
    if only["ok"] != json!(true) || only["channel"] != json!("email to ops@example.test") {
        return Err(format!("alerts.test did not reach the email account: {printed}"));
    }
    let said = test.said(vm, &format!("{MONITORING} seen --details 'can read this'"))?;
    for wanted in [
        "seen can read this: mail 1 ntfy 0 slack 0 discord 0 matrix 0 json 0",
        "heard mail tls=yes user=alerts@example.test to=ops@example.test",
    ] {
        if !said.contains(wanted) {
            return Err(format!("the mail server did not hear the test as {wanted:?}: {said:?}"));
        }
    }

    test.change("/api/1/alerts/email/remove", &json!({}))?;
    test.no_channel(60, "alerts.channels after the email account is removed")
}

/// Types the commands that raise alerts, `raises` being the arguments of
/// each `alerts raise`, and waits until they are done.
fn raised(vm: &mut Vm, raises: &[&str]) -> Result<(), String> {
    let commands: Vec<String> =
        raises.iter().map(|arguments| format!("elevate alerts raise {arguments}")).collect();
    run(vm, &format!("{}; echo \"raised $?\"", commands.join("; ")), 60, &["raised 0"])
}

/// Step 2 and the kinds: a webhook with a token, told of hardware alone.
fn a_webhook_and_the_kinds(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let hook = json!({"kind": "json", "url": HOOK, "token": WEBHOOK_TOKEN, "about": "hardware"});
    test.change("/api/1/alerts/webhooks", &hook)?;
    let channels = test.channels_where(60, "the webhook listed", |all| all.len() == 1)?;
    let webhook = &channels[0];
    let wanted = [
        ("kind", json!("json")),
        ("number", json!(1)),
        ("host", json!("127.0.0.1")),
        ("about", json!(["hardware"])),
    ];
    for (key, value) in wanted {
        if webhook[key] != value {
            return Err(format!("the webhook has {key} {}, not {value}: {webhook}", webhook[key]));
        }
    }

    // One alert of each kind and one of none, raised together, so that the
    // one message of the batch is what the receiver is sent: the webhook is
    // told of hardware, so the alert about data is not in it and the one of
    // no kind is.
    raised(
        vm,
        &[
            "kinds-data --title \"Kinds data\" --detail kindsdata --about data",
            "kinds-hardware --title \"Kinds hardware\" --detail kindshardware --about hardware",
            "kinds-none --title \"Kinds none\" --detail kindsnone",
        ],
    )?;
    let said = test.said(
        vm,
        &format!(
            "for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 \
             29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45; do {MONITORING} seen \
             kindshardware | grep -q 'json 1' && break; sleep 2; done; {MONITORING} seen \
             kindsdata kindshardware kindsnone"
        ),
    )?;
    for (phrase, times) in [("kindsdata", 0), ("kindshardware", 1), ("kindsnone", 1)] {
        let heard = Test::heard(&said, phrase, "json")?;
        if heard != times {
            return Err(format!("the webhook heard {phrase} {heard} times, not {times}: {said:?}"));
        }
    }
    if !test.read(Who::Alice, "/api/1/alerts")?.to_string().contains("kinds-data") {
        return Err("alerts.list does not hold the alert about data, which is still active".into());
    }

    // Every kind again: no about is left in the channel nor in the file.
    let every = json!({"channel": "webhook:1", "about": "all"});
    test.change("/api/1/alerts/channels/about", &every)?;
    let channels = test.channels_where(60, "the webhook told of every kind", |all| {
        all.len() == 1 && all[0]["about"].is_null()
    })?;
    let said = test.said(vm, &format!("elevate grep -c about {SETTINGS}"))?;
    if said.trim() != "0" {
        return Err(format!("alerts.toml still has {} lines with about: {channels:?}", said.trim()));
    }

    // A file written by hand: an about of a kind and a word that is none.
    // The kind is kept, and the log has ALR-106 for the other.
    let written = format!(
        "printf '%s\\n' '[push]' 'private = [\"10.0.2.0/24\"]' '' '[[webhook]]' 'kind = \
         \"json\"' 'url = \"{HOOK}\"' 'token = \"{WEBHOOK_TOKEN}\"' 'about = [\"data\", \
         \"colour\"]' > /tmp/hand.toml; elevate cp /tmp/hand.toml {SETTINGS}; elevate chmod 600 \
         {SETTINGS}; elevate service alertd reload; sleep 3; echo \"written $?\""
    );
    run(vm, &written, 60, &["written 0"])?;
    test.channels_where(60, "the webhook told of data", |all| {
        all.len() == 1 && all[0]["about"] == json!(["data"])
    })?;
    let said = test.said(vm, "elevate log -s alertd -p warning | grep -c ALR-106")?;
    if said.trim().parse::<u32>().unwrap_or(0) == 0 {
        return Err(format!("the log has no ALR-106 for the word that is no kind: {said:?}"));
    }

    test.change("/api/1/alerts/webhooks/1/remove", &json!({}))?;
    test.no_channel(60, "alerts.channels after the webhook is removed")
}

/// What a push the receiver kept must be, for the alert `key` raised with
/// `title` and `detail`, of `severity`, which the machine's key `public`
/// signed for: the message of the page, the headers, and the token.
fn is_this_push(
    kept: &Value,
    port: u16,
    public: &str,
    (key, title, detail): (&str, &str, &str),
    (severity, urgency): (&str, &str),
) -> Result<(), String> {
    let first = &kept["plain"]["first"];
    let wanted = [
        ("the path", &kept["path"], json!(ENDPOINT_PATH)),
        ("its answer", &kept["answered"], json!(201)),
        ("the message's version", &kept["plain"]["v"], json!(1)),
        ("the message's count", &kept["plain"]["count"], json!(1)),
        ("the alert's state", &first["state"], json!("raised")),
        ("the alert's severity", &first["severity"], json!(severity)),
        ("the alert's title", &first["title"], json!(title)),
        ("the alert's detail", &first["detail"], json!(detail)),
        ("the alert's key", &first["key"], json!(key)),
        ("Content-Encoding", &kept["headers"]["content-encoding"], json!("aes128gcm")),
        ("Content-Type", &kept["headers"]["content-type"], json!("application/octet-stream")),
        ("TTL", &kept["headers"]["ttl"], json!("86400")),
        ("Urgency", &kept["headers"]["urgency"], json!(urgency)),
        ("the token's verdict", &kept["vapid"]["ok"], json!(true)),
        ("the token's key", &kept["vapid"]["key"], json!(public)),
        ("the token's audience", &kept["vapid"]["aud"], json!(format!("https://10.0.2.2:{port}"))),
    ];
    for (what, got, want) in wanted {
        if *got != want {
            return Err(format!("{what} is {got}, not {want}: {kept}"));
        }
    }
    let topic = kept["headers"]["topic"].as_str().unwrap_or_default();
    let host = kept["plain"]["host"].as_str().unwrap_or_default();
    let source = first["source"].as_str().unwrap_or_default();
    if topic.len() != 32 || host.is_empty() || source.is_empty() {
        return Err(format!("the push has Topic {topic:?}, host {host:?}, source {source:?}"));
    }
    let (at, now) = (first["at_ms"].as_u64().unwrap_or(0) / 1000, unix_now());
    if at.abs_diff(now) > 86_400 {
        return Err(format!("the alert is of {at}, which is not about {now}: {kept}"));
    }
    Ok(())
}

/// Seconds since 1970 on the host.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Step 3: the machine's key, a browser added, and a push for a warning
/// and for a critical alert.
fn a_browser(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    test.unlocked()?;
    let key = test.read(Who::Alice, "/api/1/alerts/push/key")?;
    let public = key["public_key"].as_str().ok_or_else(|| format!("{key} has no key"))?.to_string();
    // 65 bytes in base64url without padding are 87 characters, and the first
    // of an uncompressed point is 4: a B.
    let url_safe =
        |text: &str| text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte));
    if public.len() != 87 || !public.starts_with('B') || !url_safe(&public) {
        return Err(format!("alerts.push-key is {public:?}, not 65 bytes in base64url"));
    }
    let restart = "elevate service alertd restart; sleep 3; echo \"restarted $?\"";
    run(vm, restart, 60, &["restarted 0"])?;
    let again = test.read(Who::Alice, "/api/1/alerts/push/key")?;
    if again["public_key"] != json!(public) {
        return Err(format!("the key was {public} and after alertd restarted it is {again}"));
    }

    // The browser, as the receiver made it: its subscription as it is.
    let subscription = test.host.subscription()?.to_string();
    let body = json!({"subscription": subscription, "name": "Phone"});
    let added = test.change("/api/1/alerts/push", &body)?;
    let id = Test::printed(&added)?["id"].as_str().map_or_default(str::to_string);
    if id.len() != 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("alerts.push-add answered {added}, which has no ID"));
    }
    let channels = test.channels_where(60, "the browser listed", |all| all.len() == 1)?;
    let browser = &channels[0];
    let wanted = [
        ("kind", json!("push")),
        ("id", json!(id)),
        ("name", json!("Phone")),
        ("host", json!("10.0.2.2")),
    ];
    for (key, value) in wanted {
        if browser[key] != value {
            return Err(format!("the browser has {key} {}, not {value}: {browser}", browser[key]));
        }
    }
    test.browser = id;

    // A warning: one push, Urgency normal.
    raised(vm, &["push-a --title \"Push A\" --detail pushmark1"])?;
    let kept = test.host.push_of("push-a")?;
    let (port, public) = (test.host.port, public.as_str());
    is_this_push(&kept, port, public, ("push-a", "Push A", "pushmark1"), ("warning", "normal"))?;
    if !test.read(Who::Alice, "/api/1/alerts")?.to_string().contains("push-a") {
        return Err("alerts.list does not hold the alert that was pushed".into());
    }

    // A critical one: Urgency high. Its notice stays on the screen's bottom
    // line until a key is pressed (ADR 0212): Enter takes it off.
    run(
        vm,
        "elevate alerts raise push-b --critical --title \"Push B\" --detail pushmark2; echo \
         \"raised $?\"",
        60,
        &["raised 0", "alertd (alerts): Push B"],
    )?;
    vm.type_text("\n")?;
    let kept = test.host.push_of("push-b")?;
    is_this_push(&kept, port, public, ("push-b", "Push B", "pushmark2"), ("critical", "high"))?;
    let sent = test.host.keys();
    if sent != ["push-a", "push-b"] {
        return Err(format!("the push service was sent {sent:?}, not push-a and push-b"));
    }
    Ok(())
}

/// Step 4: the push service answers 410, which is a browser that has ended
/// its subscription: it is no longer a channel, the history has the event,
/// and the log ALR-104.
fn a_browser_that_ends_its_subscription(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let id = test.browser.clone();
    test.host.answers(410)?;
    raised(vm, &["push-c --title \"Push C\" --detail pushmark3"])?;
    let kept = test.host.push_of("push-c")?;
    if kept["answered"] != json!(410) {
        return Err(format!("the third push was answered {}, not 410", kept["answered"]));
    }
    test.no_channel(60, "alerts.channels after the browser's subscription ended")?;
    let said = test.said(vm, "elevate alerts history --json")?;
    if !said.contains(&format!("push-ended:{id}")) {
        return Err(format!("the history has no push-ended:{id}: {said:?}"));
    }
    let said = test.said(vm, "elevate log -s alertd -p warning | grep -c ALR-104")?;
    if said.trim().parse::<u32>().unwrap_or(0) == 0 {
        return Err(format!("the log has no ALR-104: {said:?}"));
    }
    Ok(())
}

/// Step 5: an endpoint on a private network that alerts.toml does not
/// list, and one in http, are refused with the command's words, and no
/// channel is made.
fn endpoints_that_are_refused(test: &mut Test) -> Result<(), String> {
    test.unlocked()?;
    let subscription = test.host.subscription()?;
    let plain = format!("http://10.0.2.2:{}{ENDPOINT_PATH}", test.host.port);
    let asks = [
        ("an endpoint on a private network that is not listed", "https://192.168.7.8/push/x"),
        ("an endpoint in http", plain.as_str()),
    ];
    for (what, endpoint) in asks {
        let mut other = subscription.clone();
        other["endpoint"] = json!(endpoint);
        let body = json!({"subscription": other.to_string(), "name": "Elsewhere"});
        let reply = test.post(Who::Alice, "/api/1/alerts/push", &body)?;
        let reply = reply.refused(what, 422, "ADM-123")?;
        let words = reply.detail();
        let said = if endpoint.starts_with("https") { "private" } else { "https" };
        if !words.contains(said) {
            return Err(format!("{what} is refused with {words:?}, which does not say {said:?}"));
        }
    }
    test.no_channel(30, "alerts.channels after the endpoints that are refused")
}

/// What `dukovo-check` says of the alerts and how it ended, as the lines
/// the pane prints: the status, the lines for the alerts, the warnings for
/// SEC-170, and the line for no channel.
fn checked(vm: &mut Vm, test: &Test) -> Result<Vec<String>, String> {
    let said = test.said(
        vm,
        "elevate dukovo-check > /tmp/check.txt; echo \"ended $?\" > /tmp/seen.txt; grep -c -E \
         '^alerts +reach +warning ' /tmp/check.txt >> /tmp/seen.txt; grep -c '^ *SEC-170: ' \
         /tmp/check.txt >> /tmp/seen.txt; grep -c -E '^alerts +reach +passed +no channel is set \
         up' /tmp/check.txt >> /tmp/seen.txt; cat /tmp/seen.txt",
    )?;
    Ok(said.lines().map(|line| line.trim().to_string()).collect())
}

/// Step 6: with the one channel failing, the alerts line of *dukovo-check*
/// is SEC-170, and with none it passes.
fn the_check(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    test.unlocked()?;
    let denied = json!({"kind": "json", "url": DENIED, "token": WEBHOOK_TOKEN});
    test.change("/api/1/alerts/webhooks", &denied)?;
    test.channels_where(60, "the webhook the receiver refuses", |all| all.len() == 1)?;
    let done = test.change("/api/1/alerts/test", &json!({}))?;
    let printed = Test::printed(&done)?;
    let first = &printed["deliveries"][0];
    let refused = first["error"].as_str().is_some_and(|why| why.contains("403"));
    if first["ok"] != json!(false) || !refused {
        return Err(format!("alerts.test did not say the receiver's 403: {printed}"));
    }
    let lines = checked(vm, test)?;
    // The status, then how many lines say the alerts line warns, say SEC-170
    // and say that no channel is set up.
    let wanted = ["1", "1", "0"];
    if lines.len() != 4 || lines[1..] != wanted || !lines[0].starts_with("ended ") {
        return Err(format!("dukovo-check did not warn of SEC-170 as it should: {lines:?}"));
    }

    test.change("/api/1/alerts/webhooks/1/remove", &json!({}))?;
    test.no_channel(60, "alerts.channels after the refused webhook is removed")?;
    let lines = checked(vm, test)?;
    let wanted = ["0", "0", "1"];
    if lines.len() != 4 || lines[1..] != wanted || !lines[0].starts_with("ended ") {
        return Err(format!("dukovo-check did not pass with no channel as it should: {lines:?}"));
    }
    Ok(())
}

/// What the machine keeps that a secret must not be in: admind's log and
/// authd's audit lines, which the serial console has, logd's records, the
/// journal's rows and the tasks, each with its name.
fn secrets_are_kept_nowhere(vm: &mut Vm, test: &mut Test) -> Result<(), String> {
    elevated(vm)?;
    test.unlocked()?;
    // What was typed in the guest may be in the serial console's log: the
    // webhook's token is, in the file written by hand, so only what the
    // test never typed is looked for there.
    let serial = vm.log();
    let logd = test.said(vm, "elevate log --all --output json --lines 5000")?;
    let changes = test.read(Who::Alice, "/api/1/changes?limit=200")?;
    let mut journal = changes.to_string();
    for row in changes["changes"].as_array().into_iter().flatten() {
        if let Some(id) = row["id"].as_i64() {
            journal.push_str(&test.read(Who::Alice, &format!("/api/1/changes/{id}"))?.to_string());
        }
    }
    let tasks = test.read(Who::Alice, "/api/1/tasks")?.to_string();
    for secret in test.secrets() {
        if serial.contains(secret) {
            return Err(format!("{secret:?} is in the serial console's log"));
        }
    }
    let places =
        [("logd's records", &logd), ("the journal's rows", &journal), ("the tasks", &tasks)];
    for (what, text) in places {
        test.free_of_secrets(what, text)?;
    }
    // The journal is not empty: the email account was set up, and its row
    // is there without its password.
    let rows = changes["changes"].as_array().ok_or("the journal has no list of changes")?;
    if !rows.iter().any(|row| row["action"] == json!("alerts.email")) {
        return Err("the journal has no row for alerts.email, which was done".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_is_the_file_of_a_number() {
        for name in ["1.json", "12.json", "104.json"] {
            assert!(is_a_push(name), "{name}");
        }
        for name in ["subscription.json", "status", ".json", "1.json.new", "1.txt", "a1.json"] {
            assert!(!is_a_push(name), "{name}");
        }
    }

    #[test]
    fn what_seen_says_of_a_receiver_is_read_by_its_name() {
        let said = "seen kindsdata: mail 0 ntfy 0 slack 0 discord 0 matrix 0 json 0\n\
                    seen kindshardware: mail 0 ntfy 0 slack 0 discord 0 matrix 0 json 1\n";
        assert_eq!(Test::heard(said, "kindsdata", "json"), Ok(0));
        assert_eq!(Test::heard(said, "kindshardware", "json"), Ok(1));
        assert_eq!(Test::heard(said, "kindshardware", "mail"), Ok(0));
        assert!(Test::heard(said, "kindsnone", "json").is_err());
        assert!(Test::heard(said, "kindsdata", "email").is_err());
    }

    /// A push as the receiver keeps it, of the alert `push-a`.
    fn kept(urgency: &str) -> Value {
        json!({
            "path": ENDPOINT_PATH,
            "answered": 201,
            "headers": {
                "content-encoding": "aes128gcm",
                "content-type": "application/octet-stream",
                "ttl": "86400",
                "urgency": urgency,
                "topic": "abcdefghijklmnopqrstuvwxyz012345",
            },
            "plain": {
                "v": 1,
                "host": "dukovo",
                "count": 1,
                "first": {
                    "state": "raised",
                    "severity": "warning",
                    "title": "Push A",
                    "detail": "pushmark1",
                    "source": "alerts",
                    "key": "push-a",
                    "at_ms": unix_now() * 1000,
                },
            },
            "vapid": {"ok": true, "key": "BKEY", "aud": "https://10.0.2.2:4433"},
        })
    }

    #[test]
    fn a_push_that_is_as_it_should_be_is_accepted_and_each_difference_is_not() {
        let alert = ("push-a", "Push A", "pushmark1");
        let said = |kept: &Value| is_this_push(kept, 4433, "BKEY", alert, ("warning", "normal"));
        assert_eq!(said(&kept("normal")), Ok(()));
        assert!(said(&kept("high")).unwrap_err().contains("Urgency"));
        let mut other = kept("normal");
        other["vapid"]["ok"] = json!(false);
        assert!(said(&other).unwrap_err().contains("the token's verdict"));
        let mut other = kept("normal");
        other["vapid"]["key"] = json!("BOTHER");
        assert!(said(&other).unwrap_err().contains("the token's key"));
        let mut other = kept("normal");
        other["plain"]["first"]["at_ms"] = json!(1_000);
        assert!(said(&other).unwrap_err().contains("not about"));
        let mut other = kept("normal");
        other["headers"]["topic"] = json!("short");
        assert!(said(&other).unwrap_err().contains("Topic"));
        assert!(
            is_this_push(&kept("normal"), 4433, "BKEY", alert, ("critical", "normal")).is_err()
        );
    }

    #[test]
    fn what_a_step_waits_for_is_not_in_the_command_that_is_typed() {
        // Each word a step looks for on the screen is output, never in the
        // command that makes it.
        for (typed, shown) in [
            ("elevate service alertd restart; sleep 3; echo \"restarted $?\"", "restarted 0"),
            ("elevate sh -c 'curl -sf URL'; echo \"trusted $?\"", "trusted 0"),
            ("elevate alerts raise push-b --critical; echo \"raised $?\"", "raised 0"),
        ] {
            assert!(!typed.contains(shown), "{shown}");
        }
    }
}
