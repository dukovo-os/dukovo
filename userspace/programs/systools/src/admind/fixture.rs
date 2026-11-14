// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the tests of admind's routes and server stand on: a console with
//! a certificate from an authority of the test's own and a folder of
//! pages, an authd that is a thread answering from a script, and workers
//! that are threads answering from another. Nothing here is admind's own
//! code, so each test says what it expects of authd and of the worker
//! where it reads, in its own words.

use std::future::Future;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ring::digest;
use rustls::RootCertStore;
use serde_json::Value;

use super::files::Files;
use super::link::{self, Fault};
use super::reach::Network;
use super::shared::{Identity, Shared, unix_now};
use super::tls;
use super::who::Sources;
use crate::admin_settings::Settings;
use crate::cert::fixtures::{self, DAY, Scratch, Server};
use crate::signin;

/// What authd was asked, as the wire says it: the verb and its fields.
#[derive(Clone, Debug)]
pub struct Asked {
    /// `begin`, `unlock` and so on.
    pub verb: String,
    /// The fields after it, as bytes.
    pub fields: Vec<Vec<u8>>,
}

impl Asked {
    /// Field `at` as text.
    pub fn text(&self, at: usize) -> String {
        String::from_utf8_lossy(&self.fields[at]).into_owned()
    }
}

/// What a test's authd says to a request, and the descriptors that came
/// with it: the lines to write, the first of which is the answer.
pub type Script = dyn Fn(&Asked, Vec<OwnedFd>) -> Vec<String> + Send + Sync;

/// An authd that is a thread, answering each connection from a script.
pub struct Authd {
    /// Its socket.
    pub path: String,
    /// Every request it has had, as `verb field field`.
    pub seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Authd {
    /// A thread that listens at a socket named for `what` and answers by
    /// `script`.
    pub fn start(what: &str, script: Arc<Script>) -> Authd {
        let path = std::env::temp_dir()
            .join(format!("admind-test-{}-{what}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        // A socket left by an earlier run would be in the way.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("a socket");
        listener.set_nonblocking(true).expect("a socket that does not wait");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (record, quit) = (Arc::clone(&seen), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            while !quit.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (script, record) = (Arc::clone(&script), Arc::clone(&record));
                        std::thread::spawn(move || answer(&stream, &*script, &record));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(3)),
                }
            }
        });
        Authd { path, seen, stop, thread: Some(thread) }
    }

    /// The verbs asked so far, in order.
    pub fn verbs(&self) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        seen.iter().map(|line| line.split(' ').next().unwrap_or_default().to_string()).collect()
    }
}

impl Drop for Authd {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Reads the request on `stream` whole, with the descriptors that came,
/// and writes what `script` says.
fn answer(stream: &UnixStream, script: &Script, record: &Mutex<Vec<String>>) {
    let (mut bytes, mut fds, mut buffer) = (Vec::new(), Vec::new(), [0u8; 8192]);
    loop {
        let n = signin::receive(stream, &mut buffer, &mut fds).expect("a request");
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let body = bytes.strip_suffix(&[0]).expect("a request that ends in NUL");
    let mut parts = body.split(|&byte| byte == 0);
    assert_eq!(parts.next(), Some(&b"console"[..]));
    let verb = String::from_utf8(parts.next().expect("a verb").to_vec()).unwrap();
    let fields: Vec<Vec<u8>> = parts.map(<[u8]>::to_vec).collect();
    let asked = Asked { verb, fields };
    let said: Vec<String> =
        asked.fields.iter().map(|f| String::from_utf8_lossy(f).into_owned()).collect();
    record
        .lock()
        .unwrap()
        .push(format!("{} {}", asked.verb, said.join(" ")).trim_end().to_string());
    let mut stream = stream;
    for line in script(&asked, fds) {
        // A client that gave up has gone.
        if stream.write_all(format!("{line}\n").as_bytes()).is_err() {
            return;
        }
    }
}

/// A worker that is a thread: serves the link `fd` is the end of, and
/// answers each call by `answer`, noting it in `calls`.
pub fn worker(
    fd: OwnedFd,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    answer: Arc<Answer>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let socket = UnixStream::from(fd);
        socket.set_nonblocking(true).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async move {
            let socket = tokio::net::UnixStream::from_std(socket).unwrap();
            link::serve(socket, Duration::from_secs(60), move |method, params| {
                calls.lock().unwrap().push((method.clone(), params.clone()));
                let outcome = answer(&method, &params);
                async move { outcome }
            })
            .await;
        });
    })
}

/// What a test's worker says to a call: the method and its parameters, and
/// the answer or the fault.
pub type Answer = dyn Fn(&str, &Value) -> Result<Value, Fault> + Send + Sync;

/// A console as the tests run it.
pub struct Console {
    /// What the connections share.
    pub shared: Rc<Shared>,
    /// What trusts the certificate admind serves.
    pub roots: RootCertStore,
    /// Its authd.
    pub authd: Authd,
    /// The folder of the account files and the pages, which goes when the
    /// console does.
    pub _folder: Scratch,
}

/// The accounts the tests sign in: an administrator, a person who is
/// not, and the account of the second step.
pub const PASSWD: &str = "root:x:0:0:root:/system/root:/system/commands/nukesh\n\
                          chase:x:1000:1000:Chase Y,,:/users/chase:/system/commands/nukesh\n\
                          bob:x:1001:1001::/users/bob:/system/commands/nukesh\n\
                          alice:x:1002:1002:Alice,,:/users/alice:/system/commands/nukesh\n\
                          eve:x:1003:1003::/users/eve:/system/commands/nukesh\n";
/// Their groups.
pub const GROUP: &str = "root:x:0:\nchase:x:1000:\nbob:x:1001:\nalice:x:1002:\neve:x:1003:\n\
                         admins:x:10:chase,alice\n";

/// The page the test console serves.
pub const PAGE: &str = "<!doctype html><title>Dukovo Admin</title>";

impl Console {
    /// A console named for `what`, whose authd answers by `script`.
    ///
    /// # Panics
    ///
    /// Outside a runtime's `LocalSet`, or if the folders cannot be made.
    pub fn new(what: &str, script: Arc<Script>) -> Console {
        let folder = Scratch::new(what);
        let authority = fixtures::root("Test Root");
        // A certificate valid now, whatever the fixtures' clock is.
        let now = unix_now();
        let leaf = fixtures::server(
            &authority,
            &Server { not_before: now - DAY, not_after: now + 90 * DAY, ..Server::default() },
        );
        let certificates = folder.0.join("certificate");
        std::fs::create_dir_all(&certificates).unwrap();
        std::fs::write(certificates.join("fullchain.pem"), fixtures::chain_file(&[&leaf.der]))
            .unwrap();
        std::fs::write(certificates.join("key.pem"), &leaf.key).unwrap();
        let certificate = tls::load(&certificates).unwrap();
        let pages = folder.0.join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("index.html"), PAGE).unwrap();
        std::fs::write(pages.join("app.js"), "export {};").unwrap();
        let hash =
            |bytes: &[u8]| accounts::request::hex(digest::digest(&digest::SHA256, bytes).as_ref());
        let manifest =
            format!("{}  index.html\n{}  app.js\n", hash(PAGE.as_bytes()), hash(b"export {};"));
        std::fs::write(pages.join("manifest.txt"), manifest).unwrap();
        let files = Files::read(&pages, &mut Vec::new()).unwrap();
        let loopback = Network { ip: [127, 0, 0, 1].into(), prefix: 8 };
        let identity = Identity::new("nas", vec![loopback], &certificate.names);
        let authd = Authd::start(what, script);
        let mut shared =
            Shared::new(Settings::default(), identity, certificate, files, &authd.path);
        let passwd = folder.0.join("passwd");
        let group = folder.0.join("group");
        std::fs::write(&passwd, PASSWD).unwrap();
        std::fs::write(&group, GROUP).unwrap();
        Rc::get_mut(&mut shared).expect("nothing else holds it yet").accounts = Sources {
            passwd: passwd.to_string_lossy().into_owned(),
            group: group.to_string_lossy().into_owned(),
        };
        Console { shared, roots: fixtures::roots(&[&authority]), authd, _folder: folder }
    }
}

/// Runs `future` to its end on a runtime of its own, with a `LocalSet`
/// for the tasks it spawns.
pub fn run<F: Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, future)
}
