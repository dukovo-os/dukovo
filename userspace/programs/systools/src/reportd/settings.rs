// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `/system/settings/report.toml` says (report.toml(5)): the reports
//! server, the security box's recipients, and the extra root certificates
//! a staging server needs. A file that is not there is the defaults; a
//! setting with a mistake in it keeps its default, and the mistake is a
//! line of the log.
//!
//! ```toml
//! server = "https://reports.dukovo.org"
//! security-recipients = "/system/resources/report/security-recipients.txt"
//! ca = ["/system/settings/report/staging-ca.pem"]
//! ```
//!
//! There is no setting that sends a report by itself: nothing leaves the
//! machine without the person's `send`.

use std::fs;
use std::io;
use std::str::FromStr;

use toml::{Table, Value};

/// The settings file.
pub const FILE: &str = "/system/settings/report.toml";
/// The reports server, unless the file names another.
pub const SERVER: &str = "https://reports.dukovo.org";
/// The security team's age recipients, unless the file names others.
pub const RECIPIENTS: &str = "/system/resources/report/security-recipients.txt";

/// The settings, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The server's address: `https://` and a host, with no path.
    pub server: String,
    /// The file of age recipients the security box encrypts to.
    pub recipients: String,
    /// PEM files of root certificates trusted besides the system's.
    pub ca: Vec<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { server: SERVER.into(), recipients: RECIPIENTS.into(), ca: Vec::new() }
    }
}

impl Settings {
    /// The settings in [`FILE`], and the mistakes in it.
    pub fn load() -> (Settings, Vec<String>) {
        match fs::read_to_string(FILE) {
            Ok(text) => Settings::parse(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => (Settings::default(), Vec::new()),
            Err(err) => (Settings::default(), vec![format!("{FILE}: {err}")]),
        }
    }

    /// The settings `text` gives, and its mistakes.
    pub fn parse(text: &str) -> (Settings, Vec<String>) {
        let mut settings = Settings::default();
        let mut problems = Vec::new();
        let table = match Table::from_str(text) {
            Ok(table) => table,
            Err(err) => {
                problems.push(format!("{FILE}: {}", err.message()));
                return (settings, problems);
            }
        };
        for (key, value) in &table {
            match (key.as_str(), value) {
                ("server", Value::String(server)) => match check_server(server) {
                    Ok(server) => settings.server = server,
                    Err(why) => problems.push(format!("{FILE}: server: {why}")),
                },
                ("security-recipients", Value::String(path)) if path.starts_with('/') => {
                    settings.recipients.clone_from(path);
                }
                ("ca", Value::Array(paths)) => {
                    for path in paths {
                        match path {
                            Value::String(path) if path.starts_with('/') => {
                                settings.ca.push(path.clone());
                            }
                            _ => problems.push(format!("{FILE}: ca: each is a file's whole path")),
                        }
                    }
                }
                ("security-recipients", _) => {
                    problems.push(format!("{FILE}: security-recipients: a file's whole path"));
                }
                ("server" | "ca", _) => {
                    problems.push(format!("{FILE}: {key}: not the kind of value it takes"));
                }
                _ => problems.push(format!("{FILE}: {key} is not a setting report.toml has")),
            }
        }
        (settings, problems)
    }
}

/// `server`, if it is an `https://` address of a host with no path, without
/// a slash at its end.
fn check_server(server: &str) -> Result<String, String> {
    let server = server.trim_end_matches('/');
    let url = tuf::Url::parse(server).map_err(|_| format!("{server} is not an address"))?;
    if url.scheme() != "https" {
        return Err(format!("{server} is not an https:// address"));
    }
    if url.path_and_query() != "/" {
        return Err(format!("{server} has a path; the server's address is its host alone"));
    }
    Ok(server.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_file_is_the_defaults() {
        let (settings, problems) = Settings::parse("");
        assert_eq!(settings, Settings::default());
        assert!(problems.is_empty());
    }

    #[test]
    fn a_staging_server_and_its_authority() {
        let text = "server = \"https://10.0.2.2:8443/\"\nca = [\"/system/settings/report/ca.pem\"]\n\
                    security-recipients = \"/system/settings/report/keys.txt\"\n";
        let (settings, problems) = Settings::parse(text);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(settings.server, "https://10.0.2.2:8443");
        assert_eq!(settings.ca, ["/system/settings/report/ca.pem"]);
        assert_eq!(settings.recipients, "/system/settings/report/keys.txt");
    }

    #[test]
    fn a_mistake_keeps_the_default_and_is_said() {
        let (settings, problems) =
            Settings::parse("server = \"http://example.org\"\nsend = \"always\"\nca = [3]\n");
        assert_eq!(settings.server, SERVER);
        assert!(settings.ca.is_empty());
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("send is not a setting")));
        let (_, problems) = Settings::parse("server = \"https://example.org/reports\"\n");
        assert!(problems[0].contains("has a path"), "{problems:?}");
    }
}
