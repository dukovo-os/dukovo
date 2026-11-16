// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ssh-server show`: whether sshd is on and running, what sshd_config
//! says of who may sign in and how, and the key clients know the machine
//! by, as a list or as JSON for the web console.
//!
//! The settings are read as sshd reads them, with `accounts::sshd_config`'s
//! parser, so a file it does not take shows its mistakes and no settings.
//! Whether sshd is on is what its service file says, which is what the
//! next boot does; whether it runs, and on which port, is Spark's word.
//! A Spark that does not answer, a service Spark lacks, a key sshd has not
//! made yet: each is null, not a guess.
//!
//! When a field is null that should not be, `service status sshd` says
//! what Spark knows, and `/system/state/sshd` holds the key.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use accounts::sshd_config::{self, Forwarding, GatewayPorts, Mistake, Settings};
use serde::Serialize;
use spark_api::show::{self, State};

use super::{SERVICE, TRIAL};
use crate::util::{self, Fail, fail};
use crate::{service, servicefiles, sshkeys};

/// What `ssh-server show --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Report {
    /// The shape of this object: 1.
    format: u8,
    /// Whether sshd starts at boot, as its service file says; null when
    /// neither the file nor Spark says.
    on: Option<bool>,
    /// Whether Spark has it running and ready; null when Spark does not
    /// answer or does not know the service.
    running: Option<bool>,
    /// The port it listens on, as its service file lets it: null when
    /// Spark does not say, or the file names a range or several.
    port: Option<u16>,
    /// PasswordAuthentication; this and the settings below are null when
    /// the file does not read.
    passwords: Option<bool>,
    /// PubkeyAuthentication.
    keys: Option<bool>,
    /// AllowUsers: the names of the one list, in the file's order.
    allow_users: Option<Vec<String>>,
    /// AllowGroups.
    allow_groups: Option<Vec<String>>,
    /// DenyUsers.
    deny_users: Option<Vec<String>>,
    /// DenyGroups.
    deny_groups: Option<Vec<String>>,
    /// AllowTcpForwarding: `yes`, `local`, `remote` or `no`.
    forwarding: Option<&'static str>,
    /// GatewayPorts: `no`, `yes` or `clientspecified`.
    gateway_ports: Option<&'static str>,
    /// What is wrong with the file, a line each; none when it reads.
    mistakes: Vec<Wrong>,
    /// The host key's fingerprint, as `ssh-keygen -l` shows it; null
    /// while sshd has made none.
    host_key: Option<String>,
    /// The `ssh` trial, null when SSH's settings are not being tried.
    trial: Option<Trial>,
}

/// A line of sshd_config that sshd does not take.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Wrong {
    /// Its number, from 1.
    line: usize,
    /// What it says.
    text: String,
    /// Why sshd does not take it.
    why: String,
}

/// A trial that runs.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Trial {
    /// How long until Spark undoes it unless it is kept.
    left_ms: u64,
}

/// What the report is made from.
struct Sources<'a> {
    /// The text of sshd_config; empty when there is no file.
    config: &'a str,
    /// Spark's document, none when Spark does not answer.
    spark: Option<&'a show::Show>,
    /// Whether each service starts at boot, as its file says.
    at_boot: &'a BTreeMap<String, bool>,
    /// The text of sshd's public key file, none when there is none.
    host_key: Option<&'a str>,
}

/// The word for how forwarding is allowed.
fn forwarding_word(forwarding: Forwarding) -> &'static str {
    match forwarding {
        Forwarding::Yes => "yes",
        Forwarding::Local => "local",
        Forwarding::Remote => "remote",
        Forwarding::No => "no",
    }
}

/// The word for where forwarded ports listen.
fn gateway_ports_word(gateway_ports: GatewayPorts) -> &'static str {
    match gateway_ports {
        GatewayPorts::No => "no",
        GatewayPorts::Yes => "yes",
        GatewayPorts::ClientSpecified => "clientspecified",
    }
}

/// The mistakes of `config`, each with the line it names.
fn wrong(config: &str, mistakes: Vec<Mistake>) -> Vec<Wrong> {
    let lines: Vec<&str> = config.lines().collect();
    mistakes
        .into_iter()
        .map(|mistake| {
            let text = mistake.line.checked_sub(1).and_then(|at| lines.get(at));
            Wrong {
                line: mistake.line,
                text: text.map_or_else(String::new, |line| (*line).to_string()),
                why: mistake.why,
            }
        })
        .collect()
}

/// The one port a service may listen on, which is none for a list or a
/// range.
fn port_of(listen: &[String]) -> Option<u16> {
    match listen {
        [one] => one.parse().ok(),
        _ => None,
    }
}

/// What the machine says of SSH, from `sources`.
fn report(sources: &Sources) -> Report {
    let (settings, mistakes) = match sshd_config::parse(sources.config) {
        Ok(settings) => (Some(settings), Vec::new()),
        Err(mistakes) => (None, wrong(sources.config, mistakes)),
    };
    let sshd = sources
        .spark
        .and_then(|document| document.services.iter().find(|service| service.name == SERVICE));
    let get = |pick: fn(&Settings) -> Vec<String>| settings.as_ref().map(pick);
    Report {
        format: 1,
        on: sources.at_boot.get(SERVICE).copied().or(sshd.map(|sshd| sshd.enabled)),
        running: sshd.map(|sshd| sshd.state == State::Running),
        port: sshd.and_then(|sshd| port_of(&sshd.holds.listen)),
        passwords: settings.as_ref().map(|settings| settings.passwords),
        keys: settings.as_ref().map(|settings| settings.keys),
        allow_users: get(|settings| settings.allow_users.clone()),
        allow_groups: get(|settings| settings.allow_groups.clone()),
        deny_users: get(|settings| settings.deny_users.clone()),
        deny_groups: get(|settings| settings.deny_groups.clone()),
        forwarding: settings.as_ref().map(|settings| forwarding_word(settings.forwarding)),
        gateway_ports: settings.as_ref().map(|settings| gateway_ports_word(settings.gateway_ports)),
        mistakes,
        host_key: sources.host_key.and_then(sshkeys::file_fingerprint),
        trial: sources
            .spark
            .and_then(|document| service::left_of(document, TRIAL))
            .map(|left_ms| Trial { left_ms }),
    }
}

/// A yes, a no, or a dash for what is not known.
fn word(known: Option<bool>) -> &'static str {
    match known {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    }
}

/// The names of a list, or a word for no list.
fn names(list: Option<&[String]>, none: &'static str) -> String {
    match list {
        None => "-".into(),
        Some([]) => none.into(),
        Some(list) => list.join(" "),
    }
}

/// `report` as the list a person reads.
fn text(report: &Report) -> String {
    let port = report.port.map_or_else(String::new, |port| format!(" (port {port})"));
    let mut rows = vec![
        ("on", word(report.on).to_string()),
        ("running", format!("{}{port}", word(report.running))),
    ];
    if report.mistakes.is_empty() {
        rows.push(("passwords", word(report.passwords).to_string()));
        rows.push(("keys", word(report.keys).to_string()));
        rows.push(("allow users", names(report.allow_users.as_deref(), "everyone")));
        rows.push(("allow groups", names(report.allow_groups.as_deref(), "everyone")));
        rows.push(("deny users", names(report.deny_users.as_deref(), "none")));
        rows.push(("deny groups", names(report.deny_groups.as_deref(), "none")));
        rows.push(("forwarding", report.forwarding.unwrap_or("-").to_string()));
        rows.push(("gateway ports", report.gateway_ports.unwrap_or("-").to_string()));
    }
    rows.push(("host key", report.host_key.clone().unwrap_or_else(|| "-".into())));
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut shown = String::new();
    for (name, value) in &rows {
        shown += &format!("{name:<width$}  {value}\n");
    }
    for mistake in &report.mistakes {
        shown += &format!(
            "{} line {}: {} ({})\n",
            sshd_config::PATH,
            mistake.line,
            mistake.why,
            mistake.text.trim()
        );
    }
    if let Some(trial) = &report.trial {
        shown += &format!(
            "SSH's settings are being tried: {} left before they are undone unless kept.\n",
            util::elapsed(trial.left_ms / 1000)
        );
    }
    shown
}

/// The text of `path`, none when there is no such file.
fn file(path: &str) -> Result<Option<String>, Fail> {
    match fs::read_to_string(Path::new(path)) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(util::failed(&path, error)),
    }
}

/// `ssh-server show [--json]`.
pub fn run(json: bool) -> Result<i32, Fail> {
    let config = file(sshd_config::PATH)?.unwrap_or_default();
    // A key file that cannot be read says nothing, as one not made yet.
    let host_key = fs::read_to_string(sshd_config::HOST_KEY_PUBLIC).ok();
    // A Spark that does not answer leaves out what only it says.
    let spark = service::shown(None).ok();
    let at_boot = servicefiles::at_boot();
    let report = report(&Sources {
        config: &config,
        spark: spark.as_ref(),
        at_boot: &at_boot,
        host_key: host_key.as_deref(),
    });
    if json {
        let line = serde_json::to_string(&report).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
    } else {
        print!("{}", text(&report));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use spark_api::show::{Holds, Service, Trial as SparkTrial};

    use super::*;

    /// An Ed25519 public key file, and its fingerprint as `ssh-keygen -l`
    /// shows it, worked out apart from this code.
    const KEY_FILE: &str = "ssh-ed25519 \
        AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f root@dukovo\n";
    const FINGERPRINT: &str = "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA";

    const CONFIG: &str = "# SSH\n\
        PasswordAuthentication yes\n\
        AllowUsers alice bob\n\
        DenyGroups guests\n\
        AllowTcpForwarding remote\n\
        GatewayPorts clientspecified\n";

    /// Spark's document with sshd as `state` listening on `listen`, and
    /// the `ssh` trial when `left_ms` says.
    fn spark(state: State, listen: &[&str], left_ms: Option<u64>) -> show::Show {
        let sshd = Service {
            name: SERVICE.into(),
            enabled: false,
            state,
            holds: Holds {
                listen: listen.iter().map(ToString::to_string).collect(),
                ..Holds::default()
            },
            ..Service::default()
        };
        let other = Service { name: "netd".into(), state: State::Running, ..Service::default() };
        let trials = left_ms
            .map(|left_ms| SparkTrial { id: TRIAL.into(), left_ms, ..SparkTrial::default() })
            .into_iter()
            .collect();
        show::Show { services: vec![other, sshd], trials, ..show::Show::default() }
    }

    fn at_boot(on: Option<bool>) -> BTreeMap<String, bool> {
        let mut map = BTreeMap::from([("netd".to_string(), true)]);
        map.extend(on.map(|on| (SERVICE.to_string(), on)));
        map
    }

    fn json_of(
        config: &str,
        spark: Option<&show::Show>,
        at_boot: &BTreeMap<String, bool>,
        host_key: Option<&str>,
    ) -> Value {
        let sources = Sources { config, spark, at_boot, host_key };
        serde_json::to_value(report(&sources)).unwrap()
    }

    #[test]
    fn the_json_has_the_fields_the_page_reads() {
        let document = spark(State::Running, &["22"], Some(41_000));
        let shown = json_of(CONFIG, Some(&document), &at_boot(Some(true)), Some(KEY_FILE));
        let golden: Value = serde_json::from_str(include_str!("show.golden.json")).unwrap();
        assert_eq!(shown, golden);
    }

    #[test]
    fn the_fields_come_in_the_order_the_manual_gives() {
        let document = spark(State::Running, &["22"], None);
        let sources = Sources {
            config: CONFIG,
            spark: Some(&document),
            at_boot: &at_boot(Some(true)),
            host_key: None,
        };
        let line = serde_json::to_string(&report(&sources)).unwrap();
        let order = [
            "format",
            "on",
            "running",
            "port",
            "passwords",
            "keys",
            "allow_users",
            "allow_groups",
            "deny_users",
            "deny_groups",
            "forwarding",
            "gateway_ports",
            "mistakes",
            "host_key",
            "trial",
        ];
        let places: Vec<usize> =
            order.iter().map(|key| line.find(&format!("\"{key}\":")).expect(key)).collect();
        assert!(places.windows(2).all(|pair| pair[0] < pair[1]), "{line}");
    }

    #[test]
    fn a_file_that_is_not_there_says_the_defaults() {
        let shown = json_of("", None, &at_boot(None), None);
        assert_eq!(shown["passwords"], json!(false));
        assert_eq!(shown["keys"], json!(true));
        assert_eq!(shown["allow_users"], json!([]));
        assert_eq!(shown["forwarding"], json!("yes"));
        assert_eq!(shown["gateway_ports"], json!("no"));
        assert_eq!(shown["mistakes"], json!([]));
    }

    #[test]
    fn a_file_with_mistakes_gives_no_settings_and_the_lines_that_are_wrong() {
        let config = "# first\nPasswordAuthentication yes\nPort 2222\nAllowUsers a@b\n";
        let shown = json_of(config, None, &at_boot(None), None);
        for key in [
            "passwords",
            "keys",
            "allow_users",
            "allow_groups",
            "deny_users",
            "deny_groups",
            "forwarding",
            "gateway_ports",
        ] {
            assert_eq!(shown[key], Value::Null, "{key}");
        }
        let mistakes = shown["mistakes"].as_array().unwrap();
        assert_eq!(mistakes.len(), 2);
        assert_eq!((&mistakes[0]["line"], &mistakes[0]["text"]), (&json!(3), &json!("Port 2222")));
        assert!(mistakes[0]["why"].as_str().unwrap().contains("Port"));
        assert_eq!(
            (&mistakes[1]["line"], &mistakes[1]["text"]),
            (&json!(4), &json!("AllowUsers a@b"))
        );
    }

    #[test]
    fn what_only_spark_says_is_null_when_spark_does_not_answer() {
        let shown = json_of(CONFIG, None, &at_boot(Some(false)), None);
        assert_eq!(shown["on"], json!(false));
        assert_eq!(shown["running"], Value::Null);
        assert_eq!(shown["port"], Value::Null);
        assert_eq!(shown["trial"], Value::Null);
        assert_eq!(shown["host_key"], Value::Null);
    }

    #[test]
    fn a_spark_that_lacks_the_service_says_nothing_of_it() {
        let document = show::Show { services: Vec::new(), ..show::Show::default() };
        let shown = json_of(CONFIG, Some(&document), &at_boot(None), None);
        assert_eq!(
            (&shown["on"], &shown["running"], &shown["port"]),
            (&Value::Null, &Value::Null, &Value::Null)
        );
    }

    #[test]
    fn on_is_the_file_s_word_and_then_spark_s() {
        let mut document = spark(State::Stopped, &["22"], None);
        document.services[1].enabled = true;
        assert_eq!(
            json_of(CONFIG, Some(&document), &at_boot(Some(false)), None)["on"],
            json!(false)
        );
        assert_eq!(json_of(CONFIG, Some(&document), &at_boot(None), None)["on"], json!(true));
        assert_eq!(json_of(CONFIG, Some(&document), &at_boot(None), None)["running"], json!(false));
    }

    #[test]
    fn running_is_a_service_that_is_ready() {
        let running = |state| {
            json_of("", Some(&spark(state, &["22"], None)), &at_boot(None), None)["running"].clone()
        };
        assert_eq!(running(State::Running), json!(true));
        for state in [State::Starting, State::Stopped, State::Disabled, State::Failed] {
            assert_eq!(running(state), json!(false), "{state:?}");
        }
    }

    #[test]
    fn the_port_is_the_one_the_service_may_listen_on() {
        let port = |listen: &[&str]| {
            json_of("", Some(&spark(State::Running, listen, None)), &at_boot(None), None)["port"]
                .clone()
        };
        assert_eq!(port(&["22"]), json!(22));
        assert_eq!(port(&["2222"]), json!(2222));
        for none in [&[][..], &["22", "2222"], &["8000-8010"], &["99999"], &["ssh"]] {
            assert_eq!(port(none), Value::Null, "{none:?}");
        }
    }

    #[test]
    fn a_key_file_that_holds_no_key_gives_no_fingerprint() {
        let key = |file: Option<&str>| json_of("", None, &at_boot(None), file)["host_key"].clone();
        assert_eq!(key(Some(KEY_FILE)), json!(FINGERPRINT));
        for none in [None, Some(""), Some("garbage\n")] {
            assert_eq!(key(none), Value::Null, "{none:?}");
        }
    }

    #[test]
    fn the_trial_is_its_time_left() {
        let tried = spark(State::Running, &["22"], Some(41_000));
        let shown = json_of("", Some(&tried), &at_boot(None), None);
        assert_eq!(shown["trial"], json!({"left_ms": 41_000}));
        // Another file's trial is not SSH's.
        let mut other = spark(State::Running, &["22"], Some(5_000));
        other.trials[0].id = "network".into();
        assert_eq!(json_of("", Some(&other), &at_boot(None), None)["trial"], Value::Null);
    }

    #[test]
    fn the_list_says_each_setting_and_the_trial() {
        let document = spark(State::Running, &["22"], Some(41_000));
        let sources = Sources {
            config: CONFIG,
            spark: Some(&document),
            at_boot: &at_boot(Some(true)),
            host_key: Some(KEY_FILE),
        };
        let shown = text(&report(&sources));
        for line in [
            "on             yes\n",
            "running        yes (port 22)\n",
            "passwords      yes\n",
            "keys           yes\n",
            "allow users    alice bob\n",
            "allow groups   everyone\n",
            "deny users     none\n",
            "deny groups    guests\n",
            "forwarding     remote\n",
            "gateway ports  clientspecified\n",
        ] {
            assert!(shown.contains(line), "{line:?} in {shown}");
        }
        assert!(shown.contains(&format!("host key       {FINGERPRINT}\n")), "{shown}");
        assert!(shown.contains("being tried: 00:41 left"), "{shown}");
    }

    #[test]
    fn the_list_names_the_mistakes_in_place_of_the_settings() {
        let sources =
            Sources { config: "Port 22\n", spark: None, at_boot: &at_boot(None), host_key: None };
        let shown = text(&report(&sources));
        assert!(shown.contains("running   -\n"), "{shown}");
        assert!(shown.contains("/system/settings/ssh/sshd_config line 1: "), "{shown}");
        assert!(shown.contains("(Port 22)"), "{shown}");
        assert!(!shown.contains("passwords"), "{shown}");
    }
}
