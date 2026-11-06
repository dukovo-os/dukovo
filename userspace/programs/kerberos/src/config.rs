// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! krb5.conf(5): the realm a name is in when it names none, where each
//! realm's KDCs are, which realm a host is in, and the lifetimes and
//! encryption types a request asks for. Dukovo reads the part of MIT's
//! format that a client needs, so a krb5.conf copied from a Linux host
//! works as it is: sections in brackets, `name = value` lines, `{ }`
//! groups, and comments after `#` or `;`. What it does not know, it skips.
//!
//! The file is /system/settings/krb5.conf, or the one `KRB5_CONFIG`
//! names. A missing file is no error: names must then give their realm,
//! and nothing says where a KDC is, which [`Config::kdcs`] says plainly.

use std::collections::HashMap;

use crate::crypto::{self, Enctype};
use crate::time;
use crate::{Error, Result};

/// Where krb5.conf is, unless `KRB5_CONFIG` says.
pub const PATH: &str = "/system/settings/krb5.conf";

/// A ticket's lifetime unless krb5.conf or kinit says: a day, as MIT's.
const LIFETIME: i64 = 24 * 3600;

/// What krb5.conf says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The realm of names that give none (`default_realm`).
    pub default_realm: Option<String>,
    /// How long a ticket asked for lasts (`ticket_lifetime`), in seconds.
    pub lifetime: i64,
    /// How long it may be renewed for (`renew_lifetime`), if at all.
    pub renew_lifetime: Option<i64>,
    /// Whether tickets are asked to be forwardable (`forwardable`).
    pub forwardable: bool,
    /// The encryption types asked for, the first choice first
    /// (`permitted_enctypes`, `default_tkt_enctypes`): those of
    /// [`crypto::ALL`] unless krb5.conf narrows them.
    pub enctypes: Vec<Enctype>,
    /// Each realm's KDCs, as `host` or `host:port`.
    realms: HashMap<String, Vec<String>>,
    /// Which realm hosts are in: a name, or `.domain` for every host in
    /// it (`[domain_realm]`).
    domain_realm: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_realm: None,
            lifetime: LIFETIME,
            renew_lifetime: None,
            forwardable: false,
            enctypes: crypto::ALL.to_vec(),
            realms: HashMap::new(),
            domain_realm: Vec::new(),
        }
    }
}

/// One `name = value` of a section, or a group of them.
enum Value {
    /// A value.
    Text(String),
    /// A `{ }` group.
    Group(Vec<(String, Value)>),
}

impl Config {
    /// The krb5.conf in force: `KRB5_CONFIG`'s or [`PATH`], or none.
    pub fn load() -> Result<Config> {
        let path = std::env::var("KRB5_CONFIG").unwrap_or_else(|_| PATH.into());
        match std::fs::read_to_string(&path) {
            Ok(text) => Config::parse(&text).map_err(|err| match err {
                Error::Other(why) => Error::Other(format!("{path}: {why}")),
                other => other,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(err) => Err(Error::Other(format!("reading {path}: {err}"))),
        }
    }

    /// The krb5.conf `text`.
    pub fn parse(text: &str) -> Result<Config> {
        let sections = sections(text)?;
        let mut config = Config::default();
        for (section, values) in sections {
            match section.as_str() {
                "libdefaults" => config.libdefaults(&values)?,
                "realms" => {
                    for (realm, value) in values {
                        if let Value::Group(group) = value {
                            let kdcs = texts(&group, "kdc");
                            config.realms.entry(realm).or_default().extend(kdcs);
                        }
                    }
                }
                "domain_realm" => {
                    for (domain, value) in values {
                        if let Value::Text(realm) = value {
                            config.domain_realm.push((domain.to_ascii_lowercase(), realm));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(config)
    }

    /// The `[libdefaults]` section's values.
    fn libdefaults(&mut self, values: &[(String, Value)]) -> Result<()> {
        for (name, value) in values {
            let Value::Text(text) = value else { continue };
            match name.as_str() {
                "default_realm" => self.default_realm = Some(text.clone()),
                "ticket_lifetime" => self.lifetime = duration(name, text)?,
                "renew_lifetime" => {
                    self.renew_lifetime = Some(duration(name, text)?).filter(|&s| s > 0);
                }
                "forwardable" => self.forwardable = truth(text),
                "permitted_enctypes" | "default_tkt_enctypes" => {
                    let named = enctypes(text);
                    // Each narrows what is asked for; neither widens it.
                    self.enctypes.retain(|e| named.contains(e));
                    if self.enctypes.is_empty() {
                        return Err(Error::Other(format!(
                            "{name} names none of the encryption types Dukovo speaks ({})",
                            crypto::ALL.map(Enctype::name).join(", ")
                        )));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The KDCs of `realm`, as `host` or `host:port`, in krb5.conf's
    /// order.
    pub fn kdcs(&self, realm: &str) -> Result<Vec<String>> {
        match self.realms.get(realm) {
            Some(kdcs) if !kdcs.is_empty() => Ok(kdcs.clone()),
            _ => Err(Error::Other(format!(
                "krb5.conf names no KDC for {realm}: add `{realm} = {{ kdc = HOST }}` under \
                 [realms] in {PATH}, or the file KRB5_CONFIG names"
            ))),
        }
    }

    /// The realm `host` is in by `[domain_realm]`: its own name first,
    /// then the nearest `.domain`.
    pub fn realm_of(&self, host: &str) -> Option<String> {
        let host = host.to_ascii_lowercase();
        if let Some((_, realm)) = self.domain_realm.iter().find(|(name, _)| *name == host) {
            return Some(realm.clone());
        }
        let mut rest = host.as_str();
        while let Some(dot) = rest.find('.') {
            let domain = &rest[dot..];
            if let Some((_, realm)) = self.domain_realm.iter().find(|(name, _)| name == domain) {
                return Some(realm.clone());
            }
            rest = &rest[dot + 1..];
        }
        None
    }
}

/// Every value named `name` among `values`.
fn texts(values: &[(String, Value)], name: &str) -> Vec<String> {
    values
        .iter()
        .filter_map(|(key, value)| match value {
            Value::Text(text) if key == name => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A lifetime's value.
fn duration(name: &str, text: &str) -> Result<i64> {
    time::parse_duration(text).ok_or_else(|| {
        Error::Other(format!("{name} = {text} is not a lifetime, such as 10h, 7d or 36000"))
    })
}

/// A yes-or-no value, as MIT reads one.
fn truth(text: &str) -> bool {
    matches!(text.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1" | "y" | "t")
}

/// The types a list of names in krb5.conf gives, skipping those Dukovo
/// does not speak. `aes` and `aes-sha2` name families, as in MIT's.
fn enctypes(text: &str) -> Vec<Enctype> {
    let mut out = Vec::new();
    for name in text.split(|c: char| c.is_whitespace() || c == ',').filter(|n| !n.is_empty()) {
        let family: &[Enctype] = match name.to_ascii_lowercase().as_str() {
            "aes" => &crypto::ALL,
            "aes-sha1" => &[Enctype::Aes256Sha1, Enctype::Aes128Sha1],
            "aes-sha2" => &[Enctype::Aes256Sha384, Enctype::Aes128Sha256],
            _ => &[],
        };
        out.extend(family);
        out.extend(Enctype::from_name(name));
    }
    out
}

/// A section or a { } group: its name, and its values in order.
type Named = (String, Vec<(String, Value)>);

/// krb5.conf's sections and their values, in the file's order.
fn sections(text: &str) -> Result<Vec<Named>> {
    let mut sections: Vec<Named> = Vec::new();
    // The groups open around the current line, innermost last.
    let mut open: Vec<Named> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let at = |why: &str| Error::Other(format!("line {}: {why}", number + 1));
        if let Some(name) = line.strip_prefix('[') {
            if !open.is_empty() {
                return Err(at("a section starts inside a { } group"));
            }
            let name = name.split(']').next().unwrap_or_default().trim();
            sections.push((name.to_string(), Vec::new()));
            continue;
        }
        if line.starts_with('}') {
            let (name, group) = open.pop().ok_or_else(|| at("a } that closes no group"))?;
            push(&mut sections, &mut open, name, Value::Group(group));
            continue;
        }
        // MIT's `include` and `includedir` lines, and a module's lines,
        // are skipped rather than refused.
        let Some((name, value)) = line.split_once('=') else { continue };
        let (name, value) = (name.trim().to_string(), value.trim());
        if sections.is_empty() {
            return Err(at("a value before the first [section]"));
        }
        if value == "{" {
            open.push((name, Vec::new()));
            continue;
        }
        // A trailing `*` marks a value final, which matters only when
        // files are layered; Dukovo reads one.
        let value = value.strip_suffix('*').map_or(value, str::trim_end);
        let value = value.trim_matches('"').to_string();
        push(&mut sections, &mut open, name, Value::Text(value));
    }
    if !open.is_empty() {
        return Err(Error::Other("a { group is not closed by the end of the file".into()));
    }
    Ok(sections)
}

/// `value` named `name` added to the innermost open group, or else to the
/// current section.
fn push(sections: &mut [Named], open: &mut [Named], name: String, value: Value) {
    let values = match open.last_mut() {
        Some((_, group)) => group,
        None => &mut sections.last_mut().expect("a section is open").1,
    };
    values.push((name, value));
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: &str = "\
# A Linux host's krb5.conf.
includedir /etc/krb5.conf.d/

[libdefaults]
    default_realm = EXAMPLE.ORG
    dns_lookup_realm = false
    ticket_lifetime = 10h
    renew_lifetime = 7d
    forwardable = true
    permitted_enctypes = aes256-cts aes128-cts rc4-hmac

[realms]
    EXAMPLE.ORG = {
        kdc = kdc1.example.org
        kdc = kdc2.example.org:750
        admin_server = kdc1.example.org
    }

[domain_realm]
    .example.org = EXAMPLE.ORG
    example.org = EXAMPLE.ORG
    lab.example.net = LAB.EXAMPLE.NET
";

    #[test]
    fn a_linux_krb5_conf_reads() {
        let config = Config::parse(LINUX).unwrap();
        assert_eq!(config.default_realm.as_deref(), Some("EXAMPLE.ORG"));
        assert_eq!(config.lifetime, 36_000);
        assert_eq!(config.renew_lifetime, Some(604_800));
        assert!(config.forwardable);
        assert_eq!(config.enctypes, [Enctype::Aes256Sha1, Enctype::Aes128Sha1]);
        assert_eq!(
            config.kdcs("EXAMPLE.ORG").unwrap(),
            ["kdc1.example.org", "kdc2.example.org:750"]
        );
        assert!(config.kdcs("OTHER.ORG").unwrap_err().to_string().contains("[realms]"));
    }

    #[test]
    fn hosts_find_their_realm() {
        let config = Config::parse(LINUX).unwrap();
        assert_eq!(config.realm_of("nfs.example.org").as_deref(), Some("EXAMPLE.ORG"));
        assert_eq!(config.realm_of("a.b.Example.Org").as_deref(), Some("EXAMPLE.ORG"));
        assert_eq!(config.realm_of("example.org").as_deref(), Some("EXAMPLE.ORG"));
        assert_eq!(config.realm_of("lab.example.net").as_deref(), Some("LAB.EXAMPLE.NET"));
        assert_eq!(config.realm_of("other.example.net"), None);
    }

    #[test]
    fn no_file_is_every_default() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.enctypes, crypto::ALL);
    }

    #[test]
    fn mistakes_say_where() {
        assert!(Config::parse("[realms]\nR = {\nkdc = x\n").is_err());
        assert!(Config::parse("}\n").is_err());
        let err = Config::parse("[libdefaults]\nticket_lifetime = soon\n").unwrap_err();
        assert!(err.to_string().contains("ticket_lifetime"));
        let err = Config::parse("[libdefaults]\npermitted_enctypes = des-cbc-crc\n").unwrap_err();
        assert!(err.to_string().contains("aes256"));
    }
}
