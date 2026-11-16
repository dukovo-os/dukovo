// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `website` asks of the machine besides its files (ADR 0569): Spark
//! and the services, nginx's own check, certd, the firewall, the accounts
//! and the system's log, and the paths of the files it works on.
//!
//! [`Host`] is the seam: [`System`] asks the machine, and the tests give
//! the changes a host of their own that answers from a script and records
//! what it was asked, so that every step of a change, and the taking back
//! of each, is run without nginx or Spark.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use spark_api::show::State;

use super::answer::{self, Fetch, Reply};
use crate::bundles::as_service;
use crate::cert::inspect::{chain_of, details};
use crate::cert::store::Store;
use crate::util::strip_os_error;
use crate::{bundled, firewall, logger, servicefiles};

/// The bundle nginx is, and the service in it.
pub const NGINX_BUNDLE: &str = "org.nginx.nginx";

/// The service nginx is.
pub const NGINX: &str = "nginx";

/// The bundle PHP is.
pub const PHP_BUNDLE: &str = "net.php.php";

/// The service PHP-FPM is.
pub const PHP_FPM: &str = "php-fpm";

/// Where the bundle's own copy of nginx's settings is, which nginx's
/// first start copies into its data folder.
pub const BUNDLE_CONF: &str = "/applications/org.nginx.nginx/conf/nginx.conf";

/// Where `website` keeps its lock.
pub const STATE: &str = "/system/state/websites";

/// An address outside the machine, for the firewall's answer: one the
/// documentation reserves.
const OUTSIDE: &str = "203.0.113.1";

/// How long a service gets to say it runs.
pub const START_WAIT: Duration = Duration::from_secs(30);

/// The paths `website` works on: the machine's, under `root`, which is
/// `/` for the system and a folder for a test. What the files hold and
/// what nginx is told are always the paths of the machine.
#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// The paths under `root`.
    pub fn under(root: &Path) -> Paths {
        Paths { root: root.to_path_buf() }
    }

    /// The machine's own.
    pub fn system() -> Paths {
        Paths::under(Path::new("/"))
    }

    /// Where the machine's `view` is.
    pub fn real(&self, view: &str) -> PathBuf {
        self.root.join(view.trim_start_matches('/'))
    }

    /// The same, as text, for the calls that take a path as text.
    pub fn text(&self, view: &str) -> String {
        self.real(view).to_string_lossy().into_owned()
    }
}

/// What Spark says of a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Svc {
    /// Whether it starts at boot.
    pub enabled: bool,
    /// Whether it runs and is ready.
    pub running: bool,
}

/// What nginx's check said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// Whether the settings pass.
    pub passed: bool,
    /// The line that says what is wrong, when they do not.
    pub line: Option<String>,
}

/// What the firewall says of a port from outside.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    /// Whether it lets a connection in; true unless a rule denies it.
    pub allowed: bool,
    /// The number of the rule that decides, if one does.
    pub rule: Option<u64>,
    /// That rule in words, or nothing.
    pub text: String,
}

/// What `website` asks of the machine.
pub trait Host {
    /// The accounts, as the user database has them.
    fn accounts(&self) -> Vec<accounts::User>;

    /// Whether the bundle `bundle` is installed for everyone with its
    /// service `service`.
    fn installed(&self, bundle: &str, service: &str) -> bool;

    /// What Spark says of `service`; none when it knows none of that name.
    ///
    /// # Errors
    ///
    /// When Spark cannot be asked.
    fn state(&self, service: &str) -> Result<Option<Svc>, String>;

    /// Spark's check of the service file of `service`, with its override.
    ///
    /// # Errors
    ///
    /// Spark's line, when the file does not pass.
    fn service_check(&self, service: &str) -> Result<(), String>;

    /// Restarts `service`, and waits until Spark says it runs.
    ///
    /// # Errors
    ///
    /// Why it does not.
    fn restart(&self, service: &str) -> Result<(), String>;

    /// Has `service` read its settings again.
    ///
    /// # Errors
    ///
    /// Why it does not.
    fn reload(&self, service: &str) -> Result<(), String>;

    /// Turns `service` on, now and at boot, or off, and when on waits
    /// until Spark says it runs.
    ///
    /// # Errors
    ///
    /// Why it does not.
    fn turn(&self, service: &str, on: bool) -> Result<(), String>;

    /// nginx's check of the settings in `conf` (a path in nginx's view),
    /// as the account nginx.
    ///
    /// # Errors
    ///
    /// Why the check could not be run, or did not end in time.
    fn nginx_check(&self, conf: &str) -> Result<Checked, String>;

    /// The names certificate `name` of certd's is valid for.
    ///
    /// # Errors
    ///
    /// Why they are not known: there is no such certificate, say.
    fn certificate_names(&self, name: &str) -> Result<Vec<String>, String>;

    /// What the firewall says of a connection to `port` from outside.
    fn firewall(&self, port: u16) -> Rule;

    /// One request to this machine's own port, as [`Fetch`] says.
    ///
    /// # Errors
    ///
    /// Why there was no answer.
    fn fetch(&self, request: &Fetch) -> Result<Reply, String>;

    /// How long a name gets to answer.
    fn patience(&self) -> Duration;

    /// Waits a moment before another try.
    fn pause(&self);

    /// Writes a line to the system's log.
    fn log(&self, line: &str);
}

/// The machine.
pub struct System;

/// The part of `output` that says what nginx's check found: its first
/// line that names an error, or else its last line, as [`plain_line`]
/// gives it.
pub fn check_line(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let named = lines.iter().find(|line| {
        ["[emerg]", "[alert]", "[crit]", "[error]"].iter().any(|level| line.contains(level))
    });
    named.or(lines.last()).map(|line| plain_line(line))
}

/// nginx's line as `nginx -t` prints it at a terminal: `nginx: [LEVEL]
/// MESSAGE`. Once its error log is set, nginx starts a line with the time
/// and puts its process and thread after the level, as `2026/11/16
/// 19:56:39 [emerg] 535#0: MESSAGE`, which change from one check to the
/// next: they are left out. Any other line is as it was.
pub fn plain_line(line: &str) -> String {
    let as_it_was = || line.to_string();
    let Some((time, rest)) = line.split_once(" [") else { return as_it_was() };
    let timed = !time.is_empty() && time.bytes().all(|b| b.is_ascii_digit() || b"/: ".contains(&b));
    let Some((level, message)) = rest.split_once("] ") else { return as_it_was() };
    if !timed {
        return as_it_was();
    }
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let message = match message.split_once(": ") {
        Some((ids, after)) if ids.split_once('#').is_some_and(|(a, b)| digits(a) && digits(b)) => {
            after
        }
        _ => message,
    };
    format!("nginx: [{level}] {message}")
}

/// Waits until Spark says service `name` runs, for `START_WAIT`.
fn running_within(name: &str) -> Result<(), String> {
    let until = Instant::now() + START_WAIT;
    loop {
        let state = crate::service::shown(Some(name)).map_err(|fail| fail.message)?;
        match state.services.iter().find(|service| service.name == name) {
            Some(found) if found.state == State::Running => return Ok(()),
            Some(found) if found.state == State::Failed => {
                let why = found.last_line.clone().or_else(|| found.why.clone());
                return Err(match why {
                    Some(why) => format!("{name} failed to start: {why}"),
                    None => format!("{name} failed to start"),
                });
            }
            Some(_) => {}
            None => return Err(format!("{name} is not a service of this machine")),
        }
        if Instant::now() >= until {
            return Err(format!("{name} did not run within {} seconds", START_WAIT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

impl Host for System {
    fn accounts(&self) -> Vec<accounts::User> {
        match fs::read_to_string(accounts::PASSWD) {
            Ok(text) => text.lines().filter_map(accounts::User::parse).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn installed(&self, bundle: &str, service: &str) -> bool {
        bundled::read_record(service).is_some_and(|record| record.id == bundle)
            && Path::new(&servicefiles::path(service)).exists()
    }

    fn state(&self, service: &str) -> Result<Option<Svc>, String> {
        match crate::service::shown(Some(service)) {
            Ok(shown) => {
                Ok(shown.services.iter().find(|found| found.name == service).map(|found| Svc {
                    enabled: found.enabled,
                    running: found.state == State::Running,
                }))
            }
            Err(fail) if fail.code == 4 => Ok(None),
            Err(fail) => Err(fail.message),
        }
    }

    fn service_check(&self, service: &str) -> Result<(), String> {
        let ran = Command::new("/system/commands/service")
            .args(["check", "--brief", service])
            .output()
            .map_err(|error| format!("service check: {}", strip_os_error(&error)))?;
        if ran.status.success() {
            return Ok(());
        }
        let said = format!(
            "{}\n{}",
            String::from_utf8_lossy(&ran.stderr),
            String::from_utf8_lossy(&ran.stdout)
        );
        Err(check_line(&said).unwrap_or_else(|| format!("service check {service} failed")))
    }

    fn restart(&self, service: &str) -> Result<(), String> {
        crate::service::change("restart", service, &["--quiet".to_string()])
            .map_err(|fail| fail.message)?;
        running_within(service)
    }

    fn reload(&self, service: &str) -> Result<(), String> {
        crate::service::ask(&format!("reload {service}")).map(|_| ()).map_err(|fail| fail.message)
    }

    fn turn(&self, service: &str, on: bool) -> Result<(), String> {
        servicefiles::turn(service, on).map_err(|fail| fail.message)?;
        if on { running_within(service) } else { Ok(()) }
    }

    fn nginx_check(&self, conf: &str) -> Result<Checked, String> {
        let ran = as_service::run(NGINX_BUNDLE, "nginx", NGINX, &["-t", "-q", "-c", conf])?;
        let output = String::from_utf8_lossy(&ran.output).into_owned();
        match ran.code {
            Some(0) => Ok(Checked { passed: true, line: None }),
            Some(_) => Ok(Checked { passed: false, line: check_line(&output) }),
            None => Err("nginx's check did not end in 30 seconds".to_string()),
        }
    }

    fn certificate_names(&self, name: &str) -> Result<Vec<String>, String> {
        let stored = Store::system().get(name)?;
        let text = String::from_utf8(stored.chain).map_err(|_| "its chain is not text")?;
        let chain = chain_of(&text)?;
        let first = chain.first().ok_or("its chain holds no certificate")?;
        Ok(details(first)?.names)
    }

    fn firewall(&self, port: u16) -> Rule {
        let port = port.to_string();
        let words = ["tcp", port.as_str(), "from", OUTSIDE, "--json"];
        let mut out = Vec::new();
        let said = firewall::report::test(&words, &mut out)
            .ok()
            .and_then(|_| serde_json::from_slice::<serde_json::Value>(&out).ok());
        let allowed =
            said.as_ref().and_then(|said| said.get("allowed")).and_then(serde_json::Value::as_bool);
        Rule {
            allowed: allowed != Some(false),
            rule: said
                .as_ref()
                .and_then(|said| said.get("rule"))
                .and_then(serde_json::Value::as_u64),
            text: said
                .as_ref()
                .and_then(|said| said.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        }
    }

    fn fetch(&self, request: &Fetch) -> Result<Reply, String> {
        let extra = match request.certificate.as_deref() {
            // The machine's own certificate is trusted by nothing else.
            Some("this-machine") => {
                let [chain, _] = Store::system().files("this-machine");
                vec![chain.to_string_lossy().into_owned()]
            }
            _ => Vec::new(),
        };
        answer::fetch(request, &extra)
    }

    fn patience(&self) -> Duration {
        Duration::from_secs(10)
    }

    fn pause(&self) {
        std::thread::sleep(Duration::from_millis(250));
    }

    fn log(&self, line: &str) {
        if let Err(fail) = logger::log("website", 6, line) {
            eprintln!("website: the log was not written: {}", fail.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_of_a_test_are_under_its_folder_and_the_machine_s_are_its_own() {
        let system = Paths::system();
        assert_eq!(system.real("/shared/www"), Path::new("/shared/www"));
        let tree = Paths::under(Path::new("/tmp/x"));
        assert_eq!(tree.real("/shared/www/sites"), Path::new("/tmp/x/shared/www/sites"));
        assert_eq!(tree.text("system/state"), "/tmp/x/system/state");
    }

    #[test]
    fn the_line_of_nginx_s_check_is_the_one_that_names_an_error() {
        let output = "nginx: the configuration file /x/nginx.conf syntax is ok\n\
                      nginx: [emerg] unknown directive \"listn\" in /conf/.website-next/a.conf:3\n\
                      nginx: configuration file /x/nginx.conf test failed\n";
        assert_eq!(
            check_line(output).as_deref(),
            Some("nginx: [emerg] unknown directive \"listn\" in /conf/.website-next/a.conf:3")
        );
        let logged = "2026/11/16 19:56:39 [emerg] 535#0: unknown directive \"x\" in /c/b.conf:4\n";
        assert_eq!(
            check_line(logged).as_deref(),
            Some("nginx: [emerg] unknown directive \"x\" in /c/b.conf:4")
        );
        assert_eq!(check_line("something odd\n\n").as_deref(), Some("something odd"));
        assert_eq!(check_line("\n  \n"), None);
        assert_eq!(check_line(""), None);
    }

    #[test]
    fn a_line_of_nginx_s_log_is_plain_without_its_time_and_process() {
        assert_eq!(
            plain_line("2026/11/16 19:56:39 [warn] 12#3: a warning: of this"),
            "nginx: [warn] a warning: of this"
        );
        assert_eq!(
            plain_line("2026/11/16 19:56:39 [emerg] no ids: here"),
            "nginx: [emerg] no ids: here"
        );
        for kept in ["nginx: [emerg] as it is", "odd [x] line", "[emerg] 1#2: no time", "x [y"] {
            assert_eq!(plain_line(kept), kept);
        }
    }
}
