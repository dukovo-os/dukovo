// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `/system/settings/metrics.toml` says (metrics.toml(5)): where the
//! Prometheus endpoint listens and who may ask it, and how early the
//! warnings come (ADR 0484).
//!
//! A setting with a mistake in it keeps its default, or is left out of a
//! list, while the others go on, so that a typo never switches the history
//! off. The one exception is the endpoint: `listen` without a usable
//! `allow` stays off, since an endpoint that answers anyone is worse than
//! none. Each mistake is a line that names the file, the table and the
//! setting; the service logs them when it starts and at each reload
//! (`log metricsd` shows them).

use std::net::{IpAddr, SocketAddr};
use std::ops::RangeInclusive;

use toml::{Table, Value};

use crate::ip::{Cidr, Cidr6};

/// The settings file.
pub const FILE: &str = "/system/settings/metrics.toml";

/// The settings of `[prometheus]`, and of `[warnings]`.
const PROMETHEUS_KEYS: [&str; 2] = ["listen", "allow"];
const WARNINGS_KEYS: [&str; 4] =
    ["filesystem", "filling-days", "temperature-margin", "certificate-days"];

/// The values each warning setting may have.
const FILESYSTEM: RangeInclusive<u32> = 20..=96;
const FILLING_DAYS: RangeInclusive<u32> = 1..=30;
const TEMPERATURE_MARGIN: RangeInclusive<u32> = 1..=30;
/// A certificate is critical within 3 days, so a warning has to come
/// before that.
const CERTIFICATE_DAYS: RangeInclusive<u32> = 4..=365;

/// A network the endpoint answers: an address and the length of its
/// prefix, as `192.168.1.0/24` or `fd00::/8`. An address alone is the
/// network of that one machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    V4(Cidr),
    V6(Cidr6),
}

impl Network {
    /// The network `text` names, if it names one.
    pub fn parse(text: &str) -> Option<Network> {
        if text.contains(':') {
            Cidr6::parse(text).map(Network::V6)
        } else {
            Cidr::parse(text).map(Network::V4)
        }
    }

    /// Whether `ip` is in it. An IPv4 address that arrived over an IPv6
    /// socket, `::ffff:192.168.1.5`, counts as the IPv4 address.
    pub fn contains(self, ip: IpAddr) -> bool {
        match (self, ip.to_canonical()) {
            (Network::V4(network), IpAddr::V4(ip)) => network.contains(ip),
            (Network::V6(network), IpAddr::V6(ip)) => network.contains(ip),
            _ => false,
        }
    }
}

/// When the warnings come (metrics.toml(5), `[warnings]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// The percent of a filesystem in use that is a warning. At 97 it is
    /// critical.
    pub filesystem: u32,
    /// The days within which a filesystem that fills at the rate of the
    /// last 6 hours is a warning.
    pub filling_days: u32,
    /// How many degrees below a sensor's critical temperature is a
    /// warning.
    pub temperature_margin: u32,
    /// The days before a certificate ends that are a warning.
    pub certificate_days: u32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { filesystem: 90, filling_days: 3, temperature_margin: 5, certificate_days: 14 }
    }
}

/// All of metrics.toml.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// The addresses and ports the endpoint listens on; none while it is
    /// off.
    pub listen: Vec<SocketAddr>,
    /// The networks that may ask it. Others are closed at once.
    pub allow: Vec<Network>,
    pub warnings: Thresholds,
}

impl Settings {
    /// Whether the endpoint answers a request from `ip`.
    pub fn allows(&self, ip: IpAddr) -> bool {
        self.allow.iter().any(|network| network.contains(ip))
    }
}

/// The settings the file text `text` holds, and what is wrong in it, a
/// line each.
pub fn read_settings(text: &str) -> (Settings, Vec<String>) {
    let table = match text.parse::<Table>() {
        Ok(table) => table,
        Err(error) => {
            let line = error.span().map_or(1, |span| text[..span.start].matches('\n').count() + 1);
            return (Settings::default(), vec![format!("{FILE}:{line}: {}", error.message())]);
        }
    };
    let mut settings = Settings::default();
    let mut wrong = Vec::new();
    for (key, value) in &table {
        match (key.as_str(), value.as_table()) {
            ("prometheus", Some(section)) => prometheus(section, &mut settings, &mut wrong),
            ("warnings", Some(section)) => warnings(section, &mut settings.warnings, &mut wrong),
            ("prometheus" | "warnings", None) => {
                wrong.push(format!("{FILE}: {key} is a table: write [{key}] and its settings"));
            }
            _ => wrong.push(format!(
                "{FILE}: no setting is called {key}; the tables are [prometheus] and [warnings]"
            )),
        }
    }
    // A scrape that anyone may ask is not what a mistyped allow should
    // give: with no network to answer, the endpoint stays off.
    if !settings.listen.is_empty() && settings.allow.is_empty() {
        wrong.push(format!(
            "{FILE}: [prometheus] listen needs allow, the networks that may ask, such as \
             [\"192.168.1.0/24\"]; the endpoint stays off"
        ));
        settings.listen.clear();
    }
    (settings, wrong)
}

/// Says each key of `section`, a table called `name`, that is not one of
/// `known`.
fn unknown_keys(name: &str, section: &Table, known: &[&str], wrong: &mut Vec<String>) {
    for key in section.keys().filter(|key| !known.contains(&key.as_str())) {
        wrong.push(format!(
            "{FILE}: [{name}] has no setting called {key}; its settings are {}",
            known.join(", ")
        ));
    }
}

/// `[prometheus]`.
fn prometheus(section: &Table, settings: &mut Settings, wrong: &mut Vec<String>) {
    unknown_keys("prometheus", section, &PROMETHEUS_KEYS, wrong);
    let address = "an address and a port, such as 192.168.1.10:9100 or [fd00::10]:9100";
    let network = "a network, such as 192.168.1.0/24 or fd00::/8";
    settings.listen = list(section, "listen", address, wrong, |text| text.parse().ok());
    settings.allow = list(section, "allow", network, wrong, Network::parse);
}

/// The entries of the list `key` of `section` that `parse` takes, each
/// once; an entry it does not take is said, as not being `example`, and
/// left out.
fn list<T: PartialEq>(
    section: &Table,
    key: &str,
    example: &str,
    wrong: &mut Vec<String>,
    parse: impl Fn(&str) -> Option<T>,
) -> Vec<T> {
    let Some(value) = section.get(key) else { return Vec::new() };
    let Some(entries) = value.as_array() else {
        wrong.push(format!("{FILE}: [prometheus] {key} is a list, such as [\"...\"]"));
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries {
        let Some(text) = entry.as_str() else {
            wrong.push(format!("{FILE}: [prometheus] {key}: each entry is text, {example}"));
            continue;
        };
        match parse(text) {
            Some(item) if !found.contains(&item) => found.push(item),
            // The same entry twice is harmless.
            Some(_) => {}
            None => {
                wrong.push(format!("{FILE}: [prometheus] {key}: {text:?} is not {example}"));
            }
        }
    }
    found
}

/// `[warnings]`.
fn warnings(section: &Table, thresholds: &mut Thresholds, wrong: &mut Vec<String>) {
    unknown_keys("warnings", section, &WARNINGS_KEYS, wrong);
    let mut set = |key: &str, range: &RangeInclusive<u32>, what: &str, slot: &mut u32| {
        if let Some(number) = whole(section, key, range, what, wrong) {
            *slot = number;
        }
    };
    set("filesystem", &FILESYSTEM, "a percentage", &mut thresholds.filesystem);
    set("filling-days", &FILLING_DAYS, "a number of days", &mut thresholds.filling_days);
    set(
        "temperature-margin",
        &TEMPERATURE_MARGIN,
        "a number of degrees",
        &mut thresholds.temperature_margin,
    );
    set(
        "certificate-days",
        &CERTIFICATE_DAYS,
        "a number of days",
        &mut thresholds.certificate_days,
    );
}

/// The whole number `key` of `section` holds, if it has one in `range`;
/// if it holds another, that is said.
fn whole(
    section: &Table,
    key: &str,
    range: &RangeInclusive<u32>,
    what: &str,
    wrong: &mut Vec<String>,
) -> Option<u32> {
    let value = section.get(key)?;
    let number =
        value.as_integer().and_then(|n| u32::try_from(n).ok()).filter(|n| range.contains(n));
    if number.is_none() {
        let shown = match value {
            Value::String(text) => format!("{text:?}"),
            Value::Integer(number) => number.to_string(),
            Value::Float(number) => number.to_string(),
            Value::Boolean(flag) => flag.to_string(),
            other => format!("a {}", other.type_str()),
        };
        wrong.push(format!(
            "{FILE}: [warnings] {key} is {shown}, but it is {what}, a whole number from {} to {}",
            range.start(),
            range.end()
        ));
    }
    number
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        let (settings, wrong) = read_settings("");
        assert_eq!(settings, Settings::default());
        assert!(wrong.is_empty());
        assert!(settings.listen.is_empty());
        assert_eq!(
            settings.warnings,
            Thresholds {
                filesystem: 90,
                filling_days: 3,
                temperature_margin: 5,
                certificate_days: 14
            }
        );
        // The shipped file holds only comments.
        assert_eq!(read_settings("# Nothing is set.\n").0, Settings::default());
    }

    #[test]
    fn the_design_file_is_read() {
        let (settings, wrong) = read_settings(
            "[prometheus]\n\
             listen = [\"192.168.1.10:9100\"]\n\
             allow = [\"192.168.1.0/24\", \"fd00::/8\"]\n\
             [warnings]\n\
             filesystem = 85\nfilling-days = 5\ntemperature-margin = 8\ncertificate-days = 30\n",
        );
        assert!(wrong.is_empty(), "{wrong:?}");
        assert_eq!(settings.listen, ["192.168.1.10:9100".parse::<SocketAddr>().unwrap()]);
        assert_eq!(settings.allow.len(), 2);
        assert_eq!(
            settings.warnings,
            Thresholds {
                filesystem: 85,
                filling_days: 5,
                temperature_margin: 8,
                certificate_days: 30
            }
        );
    }

    #[test]
    fn networks_are_matched_by_their_prefix() {
        let settings = read_settings(
            "[prometheus]\nlisten = [\"[::]:9100\"]\n\
             allow = [\"192.168.1.0/24\", \"fd00::/8\", \"10.0.0.7\"]\n",
        )
        .0;
        assert_eq!(settings.listen, ["[::]:9100".parse::<SocketAddr>().unwrap()]);
        for yes in ["192.168.1.200", "fd12:3456::1", "10.0.0.7", "::ffff:192.168.1.5"] {
            assert!(settings.allows(ip(yes)), "{yes}");
        }
        for no in ["192.168.2.1", "10.0.0.8", "2001:db8::1", "fe80::1", "::ffff:192.168.2.5"] {
            assert!(!settings.allows(ip(no)), "{no}");
        }
    }

    #[test]
    fn listen_without_allow_stays_off() {
        let (settings, wrong) = read_settings("[prometheus]\nlisten = [\"192.168.1.10:9100\"]\n");
        assert!(settings.listen.is_empty());
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].contains("listen needs allow"), "{wrong:?}");
        // So does an allow of which every entry is wrong, with one line each.
        let (settings, wrong) =
            read_settings("[prometheus]\nlisten = [\"192.168.1.10:9100\"]\nallow = [\"lan\"]\n");
        assert!(settings.listen.is_empty() && settings.allow.is_empty());
        assert_eq!(wrong.len(), 2, "{wrong:?}");
        assert!(wrong[0].contains("\"lan\" is not a network"));
        // Allow alone does nothing and is no mistake.
        let (settings, wrong) = read_settings("[prometheus]\nallow = [\"10.0.0.0/8\"]\n");
        assert!(wrong.is_empty() && settings.listen.is_empty());
    }

    #[test]
    fn a_wrong_entry_is_left_out_and_the_rest_stay() {
        let (settings, wrong) = read_settings(
            "[prometheus]\n\
             listen = [\"0.0.0.0:9100\", \"9100\", \"[fd00::1]:9100\", \"0.0.0.0:9100\", 5]\n\
             allow = [\"10.0.0.0/8\", \"10.0.0.0/33\", \"10.0.0.0/8\"]\n",
        );
        assert_eq!(settings.listen.len(), 2);
        assert_eq!(settings.allow.len(), 1);
        assert_eq!(wrong.len(), 3, "{wrong:?}");
        assert!(wrong[0].contains("listen: \"9100\" is not an address and a port"), "{wrong:?}");
        assert!(wrong[1].contains("listen: each entry is text"), "{wrong:?}");
        assert!(wrong[2].contains("allow: \"10.0.0.0/33\" is not a network"), "{wrong:?}");
    }

    #[test]
    fn a_wrong_threshold_keeps_its_default() {
        let (settings, wrong) = read_settings(
            "[warnings]\nfilesystem = 120\nfilling-days = \"three\"\n\
             temperature-margin = 2.5\ncertificate-days = 3\n",
        );
        assert_eq!(settings.warnings, Thresholds::default());
        assert_eq!(wrong.len(), 4, "{wrong:?}");
        assert!(
            wrong[0].contains(
                "[warnings] filesystem is 120, but it is a percentage, a whole number from 20 to 96"
            ),
            "{wrong:?}"
        );
        assert!(wrong.iter().any(|w| w.contains("filling-days is \"three\"")));
        assert!(wrong.iter().any(|w| w.contains("temperature-margin is 2.5")));
        assert!(wrong.iter().any(|w| w.contains("certificate-days is 3,")));
        // One wrong setting does not take the others with it.
        let (settings, wrong) = read_settings("[warnings]\nfilesystem = 50\nfilling-days = 0\n");
        assert_eq!((settings.warnings.filesystem, settings.warnings.filling_days), (50, 3));
        assert_eq!(wrong.len(), 1);
    }

    #[test]
    fn unknown_names_are_said() {
        let (_, wrong) = read_settings(
            "[prometheus]\nlisten_on = []\n[warnings]\nbattery = 5\n[history]\nkeep = 3\n",
        );
        assert_eq!(wrong.len(), 3, "{wrong:?}");
        for said in [
            "[prometheus] has no setting called listen_on",
            "no setting is called history",
            "[warnings] has no setting called battery",
        ] {
            assert!(wrong.iter().any(|line| line.contains(said)), "{said}: {wrong:?}");
        }
        let (_, wrong) = read_settings("prometheus = 5\n");
        assert_eq!(
            wrong,
            [format!("{FILE}: prometheus is a table: write [prometheus] and its settings")]
        );
    }

    #[test]
    fn a_file_that_is_not_toml_is_said_with_its_line() {
        let (settings, wrong) = read_settings("[warnings]\nfilesystem = 90\nfilling-days = = 3\n");
        assert_eq!(settings, Settings::default());
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].starts_with(&format!("{FILE}:3: ")), "{wrong:?}");
    }
}
