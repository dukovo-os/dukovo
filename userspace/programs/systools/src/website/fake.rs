// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine of the tests: a tree of files under a temporary folder, and
//! a [`Host`] that answers from it like nginx, Spark and certd would.
//!
//! Every call that can fail is told apart by its words and numbered as it
//! is made, so that a test can make the one that comes n-th fail, or every
//! call from the n-th on, and look at what is left of the tree and of the
//! services. The web server is the sites' files as nginx last read them
//! (when it started, restarted or reloaded) and the files in the sites'
//! folders, so that a site answers only once it is live and read.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::answer::{Fetch, Reply};
use super::change::{self, AddRequest, Ctx, OVERRIDE, SERVICE};
use super::conf::{self, Facts, Kind};
use super::host::{BUNDLE_CONF, Checked, Host, NGINX, PHP_FPM, Paths, Rule, STATE, Svc};
use super::plan::{LOGS, NGINX_HOME, SETTINGS};
use super::stage::{CONF, conf_view};
use crate::cert::acme::steps::Steps;

/// The text of nginx's service file as the bundle writes it.
pub const NGINX_SERVICE: &str = "service_version = 1\ndescription = \"nginx\"\n\
    program = \"/applications/org.nginx.nginx/bin/nginx-service\"\n\
    enabled = true\n\n[permissions]\nnetwork.outbound = false\nnetwork.listen = [\"80\", \"443\"]\n\
    files.read = [\"/applications/org.nginx.nginx\", \"/shared/www\"]\n\
    files.read-write = [\"/shared/services/org.nginx.nginx\"]\n\n\
    [[sockets]]\nport = 80\naddress = \"0.0.0.0\"\nname = \"http\"\nnon-blocking = true\n\n\
    [[sockets]]\nport = 80\naddress = \"::\"\nname = \"http6\"\nnon-blocking = true\n";

/// The bundle's own `nginx.conf`.
pub const NGINX_CONF: &str = include_str!("../../../../../ports/nginx/nginx.conf");

/// What a file holds that nginx refuses wherever it is.
pub const REFUSED: &str = "nginx-refuses-this";

/// What the check of nginx saw while it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Whether it was the staged copy that was checked.
    pub staged: bool,
    /// The files of the set it read, by name.
    pub set: Vec<String>,
    /// The files that were live at that moment, by name.
    pub live: Vec<String>,
}

/// The machine.
pub struct Fake {
    root: PathBuf,
    paths: Paths,
    steps: Steps,
    /// What Spark says of nginx; none when it is not installed.
    pub nginx: Cell<Option<Svc>>,
    /// What Spark says of PHP-FPM; none when it is not installed.
    pub php: Cell<Option<Svc>>,
    /// Whether nginx's first start writes its settings.
    pub writes_conf: Cell<bool>,
    calls: RefCell<Vec<String>>,
    fail_at: Cell<Option<usize>>,
    fail_from: Cell<Option<usize>>,
    fail_calls: RefCell<Vec<String>>,
    certificates: RefCell<BTreeMap<String, Vec<String>>>,
    blocked: RefCell<Vec<u16>>,
    refused: RefCell<Vec<String>>,
    loaded: RefCell<Vec<Facts>>,
    seen: RefCell<Vec<Seen>>,
    saw: RefCell<Vec<Option<String>>>,
    logged: RefCell<Vec<String>>,
    held: RefCell<Vec<bool>>,
}

/// This process's user and group numbers.
fn me() -> (u32, u32) {
    // SAFETY: both calls have no failure and touch no memory.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// The number of the first person: the process's own when it is the
/// account of a person, so that what is made for them can be chowned, and
/// 1000 when the tests run as root.
pub fn person() -> u32 {
    let (uid, _) = me();
    if (accounts::FIRST_USER..=accounts::LAST_USER).contains(&uid) { uid } else { 1000 }
}

/// A new, empty folder for one test.
fn scratch() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "website-fake-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("the test's folder");
    path
}

/// What an add of `names` asks for, as `alice` with the default folder.
pub fn request(names: &[&str], kind: Kind, certificate: Option<&str>) -> AddRequest {
    AddRequest {
        names: names.iter().map(ToString::to_string).collect(),
        kind,
        folder: format!("/shared/www/sites/{}", names[0]),
        owner: "alice".to_string(),
        certificate: certificate.map(ToString::to_string),
    }
}

impl Fake {
    /// A machine with nginx running with its settings in place, PHP-FPM
    /// installed and off, and certd holding `shop.example.org`,
    /// `wild.example.org` (for `*.example.org`) and `other.example.net`.
    pub fn new() -> Fake {
        let root = scratch();
        let fake = Fake {
            paths: Paths::under(&root),
            root,
            steps: Steps::off(),
            nginx: Cell::new(Some(Svc { enabled: true, running: true })),
            php: Cell::new(Some(Svc { enabled: false, running: false })),
            writes_conf: Cell::new(true),
            calls: RefCell::default(),
            fail_at: Cell::new(None),
            fail_from: Cell::new(None),
            fail_calls: RefCell::default(),
            certificates: RefCell::default(),
            blocked: RefCell::default(),
            refused: RefCell::default(),
            loaded: RefCell::default(),
            seen: RefCell::default(),
            saw: RefCell::default(),
            logged: RefCell::default(),
            held: RefCell::default(),
        };
        fake.put(SERVICE, NGINX_SERVICE);
        fake.put(BUNDLE_CONF, NGINX_CONF);
        fake.put(&conf_view(), NGINX_CONF);
        // nginx's service makes the folder of the sites' logs when it starts.
        fs::create_dir_all(fake.real(LOGS)).expect("the sites' logs folder");
        // The installer makes the folder the overrides are in.
        fs::create_dir_all(fake.real("/system/settings/spark/services/settings.d")).expect("it");
        fs::create_dir_all(fake.real("/shared/www")).expect("the sites' folder");
        fs::set_permissions(fake.real("/shared/www"), fs::Permissions::from_mode(0o755))
            .expect("its mode");
        let names = |names: &[&str]| names.iter().map(ToString::to_string).collect();
        let mut held = fake.certificates.borrow_mut();
        held.insert(
            "shop.example.org".into(),
            names(&["shop.example.org", "www.shop.example.org"]),
        );
        held.insert("wild.example.org".into(), names(&["*.example.org"]));
        held.insert("other.example.net".into(), names(&["other.example.net"]));
        drop(held);
        fake.read_again();
        fake
    }

    /// nginx is not installed.
    pub fn without_nginx(self) -> Fake {
        self.nginx.set(None);
        self
    }

    /// nginx is installed and off.
    pub fn nginx_off(self) -> Fake {
        self.nginx.set(Some(Svc { enabled: false, running: false }));
        self
    }

    /// nginx has never run, so it has no settings of its own yet.
    pub fn never_started(self) -> Fake {
        let nginx = self.nginx_off();
        fs::remove_file(nginx.real(&conf_view())).expect("nginx's settings");
        fs::remove_dir_all(nginx.real(&format!("{NGINX_HOME}/logs"))).expect("nginx's logs");
        nginx
    }

    /// PHP-FPM is not installed.
    pub fn without_php(self) -> Fake {
        self.php.set(None);
        self
    }

    /// PHP-FPM runs.
    pub fn php_on(self) -> Fake {
        self.php.set(Some(Svc { enabled: true, running: true }));
        self
    }

    /// The paths of the tree.
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// What a change is run with.
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx { paths: &self.paths, host: self, steps: &self.steps }
    }

    /// Where the machine's `view` is in the tree.
    pub fn real(&self, view: &str) -> PathBuf {
        self.paths.real(view)
    }

    /// What the file at `view` holds, or none when it is not there.
    pub fn text(&self, view: &str) -> Option<String> {
        fs::read_to_string(self.real(view)).ok()
    }

    /// Writes the file at `view`, and its folders.
    pub fn put(&self, view: &str, text: &str) {
        let path = self.real(view);
        fs::create_dir_all(path.parent().expect("a file has a folder")).expect("its folder");
        fs::write(path, text).expect("the file");
    }

    /// The names of the files in the folder at `view`.
    pub fn names(&self, view: &str) -> Vec<String> {
        listed(&self.real(view))
    }

    /// The owner's number of what is at `view`.
    pub fn owner_of(&self, view: &str) -> u32 {
        fs::symlink_metadata(self.real(view)).expect("it is there").uid()
    }

    /// The firewall holds back `port`.
    pub fn blocks(self, port: u16) -> Fake {
        self.blocked.borrow_mut().push(port);
        self
    }

    /// nginx refuses a set of sites that has the site `name` in it, whether
    /// it is staged or live.
    pub fn refuses(&self, name: &str) {
        self.refused.borrow_mut().push(format!("{name}.conf"));
    }

    /// What the machine was asked that can fail, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    /// What nginx's checks saw.
    pub fn checks(&self) -> Vec<Seen> {
        self.seen.borrow().clone()
    }

    /// The text of the override at each restart of nginx: the service file
    /// nginx was started with.
    pub fn restarted_with(&self) -> Vec<Option<String>> {
        self.saw.borrow().clone()
    }

    /// For each call that can fail, whether the lock on the websites was
    /// held when it was made.
    pub fn held(&self) -> Vec<bool> {
        self.held.borrow().clone()
    }

    /// What was said to the log.
    pub fn logged(&self) -> Vec<String> {
        self.logged.borrow().clone()
    }

    /// Forgets the calls made so far, and with them the numbers the next
    /// failures are counted from.
    pub fn clear(&self) {
        self.calls.borrow_mut().clear();
        self.seen.borrow_mut().clear();
        self.saw.borrow_mut().clear();
        self.logged.borrow_mut().clear();
        self.held.borrow_mut().clear();
    }

    /// The `n`-th call from now (counting from 1) fails.
    pub fn fail_at(&self, n: usize) {
        self.fail_at.set(Some(n));
    }

    /// Every call from the `n`-th on fails.
    pub fn fail_from(&self, n: usize) {
        self.fail_from.set(Some(n));
    }

    /// Every call whose words start with `prefix` fails.
    pub fn fail_calls(&self, prefix: &str) {
        self.fail_calls.borrow_mut().push(prefix.to_string());
    }

    /// Whether a call that changes something was made: a restart, a reload,
    /// a service turned on or off.
    pub fn changed_services(&self) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|call| ["restart", "reload", "turn"].iter().any(|word| call.starts_with(word)))
    }

    /// The names the sites answer to that nginx has read, sorted.
    pub fn serving(&self) -> Vec<String> {
        let mut names: Vec<String> =
            self.loaded.borrow().iter().flat_map(|facts| facts.names.clone()).collect();
        names.sort();
        names
    }

    /// Every file and folder of the tree but the lock's, with what is in
    /// it and its mode.
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        fn walk(base: &Path, at: &Path, into: &mut BTreeMap<String, String>) {
            for entry in fs::read_dir(at).expect("a folder").flatten() {
                let path = entry.path();
                let view = format!("/{}", path.strip_prefix(base).expect("under it").display());
                if view == "/system/state" {
                    continue;
                }
                let meta = fs::symlink_metadata(&path).expect("it is there");
                let mode = meta.permissions().mode() & 0o7777;
                if meta.is_dir() {
                    into.insert(view, format!("folder {mode:o}"));
                    walk(base, &path, into);
                } else if meta.is_symlink() {
                    let to = fs::read_link(&path).expect("a link");
                    into.insert(view, format!("link to {}", to.display()));
                } else {
                    let text =
                        String::from_utf8_lossy(&fs::read(&path).expect("a file")).into_owned();
                    into.insert(view, format!("file {mode:o} {text}"));
                }
            }
        }
        let mut into = BTreeMap::new();
        walk(&self.root, &self.root, &mut into);
        into
    }

    /// What nginx's check leaves behind in its `conf` folder.
    pub fn staged_left(&self) -> Vec<String> {
        self.names(CONF).into_iter().filter(|name| name.starts_with(".website")).collect()
    }

    /// Adds a site with the real change, which must succeed, and forgets
    /// the calls it made.
    pub fn added(&self, names: &[&str], kind: Kind, certificate: Option<&str>) {
        change::add(&self.ctx(), &request(names, kind, certificate)).expect("the site is added");
        self.clear();
    }

    /// nginx reads the sites' files again.
    fn read_again(&self) {
        let mut sites = Vec::new();
        for name in self.names(SETTINGS) {
            if let Some(text) = self.text(&format!("{SETTINGS}/{name}"))
                && has_extension(&name, "conf")
                && let Ok(read) = conf::parse(&text)
            {
                sites.push(conf::facts(&read));
            }
        }
        *self.loaded.borrow_mut() = sites;
    }

    /// Notes a call, and says whether it is one that fails.
    fn tick(&self, call: &str) -> Result<(), String> {
        // Whether another change would have to wait: the lock is taken.
        let busy = fs::OpenOptions::new()
            .write(true)
            .open(self.real(STATE).join("lock"))
            .is_ok_and(|file| matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)));
        self.held.borrow_mut().push(busy);
        let mut calls = self.calls.borrow_mut();
        calls.push(call.to_string());
        let number = calls.len();
        let scripted = self.fail_at.get() == Some(number)
            || self.fail_from.get().is_some_and(|from| number >= from)
            || self.fail_calls.borrow().iter().any(|prefix| call.starts_with(prefix.as_str()));
        if scripted { Err(format!("{call} failed, as the test said")) } else { Ok(()) }
    }

    /// The service `name`'s state.
    fn cell(&self, name: &str) -> Result<&Cell<Option<Svc>>, String> {
        match name {
            NGINX => Ok(&self.nginx),
            PHP_FPM => Ok(&self.php),
            other => Err(format!("{other} is not a service of this machine")),
        }
    }

    /// What nginx answers to a request: the sites as it last read them.
    fn serve(&self, request: &Fetch) -> Result<Reply, String> {
        let page = |status: u16, location: Option<String>, body: &str| Reply {
            status,
            location,
            body: body.as_bytes().to_vec(),
        };
        if !self.nginx.get().is_some_and(|svc| svc.running) {
            return Err("connection refused".to_string());
        }
        let loaded = self.loaded.borrow();
        let Some(site) = loaded.iter().find(|facts| facts.names.contains(&request.host)) else {
            return Ok(page(404, None, ""));
        };
        match (request.port, site.https) {
            (443, false) => return Ok(page(404, None, "")),
            (443, true) if site.certificate != request.certificate => {
                return Err("the certificate is not the one that was asked for".to_string());
            }
            (80, _) if site.redirects_http => {
                let to = format!("https://{}{}", request.host, request.path);
                return Ok(page(301, Some(to), ""));
            }
            _ => {}
        }
        let Some(folder) = &site.folder else { return Ok(page(404, None, "")) };
        let file = self.real(folder).join(request.path.trim_start_matches('/'));
        let Ok(text) = fs::read_to_string(file) else { return Ok(page(404, None, "")) };
        if !has_extension(&request.path, "php") {
            return Ok(page(200, None, &text));
        }
        if site.kind != Kind::Php {
            return Ok(page(404, None, ""));
        }
        if !self.php.get().is_some_and(|svc| svc.running) {
            return Ok(page(502, None, ""));
        }
        let said = text.split_once("echo \"").and_then(|(_, rest)| rest.split_once("\";"));
        Ok(page(200, None, said.map_or("", |(said, _)| said)))
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Whether the file name `name` ends in `.extension`, in those letters.
fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name).extension().is_some_and(|found| found == extension)
}

/// The names of the files in `dir`, sorted.
fn listed(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

impl Host for Fake {
    fn accounts(&self) -> Vec<accounts::User> {
        let (uid, gid) = me();
        let user = |name: &str, uid: u32, gid: u32| accounts::User {
            name: name.to_string(),
            uid,
            gid,
            full_name: String::new(),
            home: format!("/users/{name}"),
            shell: "/system/commands/nukesh".to_string(),
        };
        // As a user that is not root the folders can only be given to the
        // process itself, so the accounts are that user.
        let (first, group) = if uid == 0 { (person(), person()) } else { (person(), gid) };
        let (php, php_group) = if uid == 0 { (150, 150) } else { (uid, gid) };
        vec![
            user("root", 0, 0),
            user("daemon", 1, 1),
            user("alice", first, group),
            user("bob", first + 1, group),
            user("php", php, php_group),
        ]
    }

    fn installed(&self, _bundle: &str, service: &str) -> bool {
        self.cell(service).is_ok_and(|cell| cell.get().is_some())
    }

    fn state(&self, service: &str) -> Result<Option<Svc>, String> {
        Ok(self.cell(service)?.get())
    }

    fn service_check(&self, service: &str) -> Result<(), String> {
        self.tick(&format!("service_check {service}"))?;
        let file = self.text(SERVICE).ok_or("there is no service file")?;
        let over = match self.text(OVERRIDE) {
            Some(text) => Some(
                manifest::service::Override::from_toml(&text).map_err(|error| error.to_string())?,
            ),
            None => None,
        };
        manifest::service::from_toml_over(&file, over.as_ref())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn restart(&self, service: &str) -> Result<(), String> {
        let result = self.tick(&format!("restart {service}"));
        let cell = self.cell(service)?;
        let mut svc = cell.get().ok_or("it is not installed")?;
        if service == NGINX {
            self.saw.borrow_mut().push(self.text(OVERRIDE));
        }
        svc.running = result.is_ok();
        cell.set(Some(svc));
        if result.is_ok() && service == NGINX {
            self.read_again();
        }
        result
    }

    fn reload(&self, service: &str) -> Result<(), String> {
        self.tick(&format!("reload {service}"))?;
        if service == NGINX {
            self.read_again();
        }
        Ok(())
    }

    fn turn(&self, service: &str, on: bool) -> Result<(), String> {
        let result = self.tick(&format!("turn {service} {}", if on { "on" } else { "off" }));
        let cell = self.cell(service)?;
        let mut svc = cell.get().ok_or("it is not installed")?;
        svc.enabled = on;
        svc.running = on && result.is_ok();
        cell.set(Some(svc));
        if !on && service == NGINX {
            self.loaded.borrow_mut().clear();
        }
        if on && service == NGINX {
            if self.writes_conf.get() && self.text(&conf_view()).is_none() {
                self.put(&conf_view(), NGINX_CONF);
            }
            if result.is_ok() {
                self.read_again();
            }
        }
        result
    }

    fn nginx_check(&self, conf: &str) -> Result<Checked, String> {
        let staged = conf != conf_view();
        self.tick(&format!("nginx_check {}", if staged { "staged" } else { "live" }))?;
        let view = if staged { format!("{CONF}/.website-next") } else { SETTINGS.to_string() };
        let set = listed(&self.real(&view));
        let live = listed(&self.real(SETTINGS));
        self.seen.borrow_mut().push(Seen { staged, set: set.clone(), live });
        if !self.real(conf).is_file() {
            let line =
                format!("nginx: [emerg] open() \"{conf}\" failed (2: No such file or directory)");
            return Ok(Checked { passed: false, line: Some(line) });
        }
        for file in set.iter().filter(|file| has_extension(file, "conf")) {
            let text = self.text(&format!("{view}/{file}")).unwrap_or_default();
            if conf::parse(&text).is_err()
                || text.contains(REFUSED)
                || self.refused.borrow().contains(file)
            {
                let line =
                    format!("nginx: [emerg] unknown directive \"refused\" in {view}/{file}:7");
                return Ok(Checked { passed: false, line: Some(line) });
            }
        }
        Ok(Checked { passed: true, line: None })
    }

    fn certificate_names(&self, name: &str) -> Result<Vec<String>, String> {
        self.tick(&format!("certificate_names {name}"))?;
        self.certificates
            .borrow()
            .get(name)
            .cloned()
            .ok_or_else(|| format!("there is no certificate {name}"))
    }

    fn firewall(&self, port: u16) -> Rule {
        if self.blocked.borrow().contains(&port) {
            Rule { allowed: false, rule: Some(3), text: format!("3: deny tcp {port}") }
        } else {
            Rule { allowed: true, rule: None, text: String::new() }
        }
    }

    fn fetch(&self, request: &Fetch) -> Result<Reply, String> {
        let what = if has_extension(&request.path, "php") { "php" } else { "file" };
        self.tick(&format!("fetch {} {} {what}", request.port, request.host))?;
        self.serve(request)
    }

    fn patience(&self) -> Duration {
        Duration::ZERO
    }

    fn pause(&self) {}

    fn log(&self, line: &str) {
        self.logged.borrow_mut().push(line.to_string());
    }
}
