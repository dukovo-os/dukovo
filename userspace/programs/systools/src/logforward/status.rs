// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where each destination of logforward(8) stands, which `logforward
//! status` shows (ADR 0482).
//!
//! The service writes one file of JSON for each destination, `<name>.json`
//! in `/system/live/logforward`, when its state changes and once a second
//! after that. A destination that stops being served, by a stop of the
//! service or by a change of the settings, has its file removed. The files
//! are in the live folder, so a boot begins without them; one that a
//! crash leaves says what the destination last did.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::util;

/// Where the status files are.
pub const DIRECTORY: &str = "/system/live/logforward";

/// What a destination is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The connection is up.
    Connected,
    /// A connection is being made.
    Connecting,
    /// The last attempt failed, and the next waits.
    Waiting,
}

impl State {
    /// The state as `logforward status` says it.
    pub fn name(self) -> &'static str {
        match self {
            State::Connected => "connected",
            State::Connecting => "connecting",
            State::Waiting => "waiting",
        }
    }
}

/// One destination's status file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The destination's name.
    pub name: String,
    /// Its address with the port.
    pub address: String,
    /// `tls`, `tcp` or `udp`.
    pub transport: String,
    /// What it is doing.
    pub state: State,
    /// Since when it has been in that state: RFC 3339, in UTC.
    pub since: String,
    /// What went wrong, while something has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How many messages have been sent since logforward started.
    pub sent: u64,
    /// How many bytes of the log lie after the destination's place in it.
    pub behind: u64,
    /// What the TLS handshake found, while connected over TLS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<String>,
}

impl Status {
    /// Writes the file for this destination in `dir`, a new one beside it
    /// put in its place, so that a reader never sees half of it.
    ///
    /// # Errors
    ///
    /// The file system's.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let new = dir.join(format!("{}.json.new", self.name));
        let json = serde_json::to_string(self).map_err(io::Error::other)?;
        fs::write(&new, format!("{json}\n"))?;
        fs::rename(&new, dir.join(format!("{}.json", self.name)))
    }
}

/// The status files in `dir`, by name; one that cannot be read is left
/// out.
pub fn read_all(dir: &Path) -> Vec<Status> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut all: Vec<Status> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
        })
        .collect();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

/// Removes the status file of destination `name` from `dir`.
pub fn remove(dir: &Path, name: &str) {
    // A file that is not there is what is wanted.
    let _ = fs::remove_file(dir.join(format!("{name}.json")));
}

/// Removes the status files in `dir` of destinations that are not `kept`:
/// those the settings no longer have.
pub fn remove_others(dir: &Path, kept: &[&str]) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let stem = path.file_stem().and_then(|stem| stem.to_str());
        if path.extension().is_some_and(|extension| extension == "json")
            && !stem.is_some_and(|stem| kept.contains(&stem))
        {
            // A file somebody else removed first is gone all the same.
            let _ = fs::remove_file(path);
        }
    }
}

/// The table `logforward status` prints for `all`, then a line for each
/// error and for each TLS connection's certificate.
pub fn table(all: &[Status]) -> String {
    let heads = ["NAME", "ADDRESS", "STATE", "SINCE", "SENT", "BEHIND"];
    let mut rows = vec![heads.map(str::to_string)];
    for status in all {
        rows.push([
            status.name.clone(),
            status.address.clone(),
            status.state.name().to_string(),
            since(&status.since),
            status.sent.to_string(),
            util::human(status.behind, false),
        ]);
    }
    let widths: Vec<usize> = (0..heads.len())
        .map(|at| rows.iter().map(|row| row[at].chars().count()).max().unwrap_or(0))
        .collect();
    let mut text = String::new();
    for row in &rows {
        let cells: Vec<String> =
            row.iter().zip(&widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
        text.push_str(cells.join("  ").trim_end());
        text.push('\n');
    }
    for status in all {
        for line in [
            status.error.as_deref(),
            status.tls.as_deref().filter(|_| status.state == State::Connected),
        ]
        .into_iter()
        .flatten()
        {
            text.push_str(&format!("{}: {line}\n", status.name));
        }
    }
    text
}

/// An RFC 3339 time as `YYYY-MM-DD HH:MM:SS`, which is UTC.
fn since(time: &str) -> String {
    match time.get(..19) {
        Some(seconds) => seconds.replace('T', " "),
        None => time.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(name: &str, state: State) -> Status {
        Status {
            name: name.into(),
            address: "logs.example.com:6514".into(),
            transport: "tls".into(),
            state,
            since: "2026-11-13T10:30:00.123456Z".into(),
            error: None,
            sent: 0,
            behind: 0,
            tls: None,
        }
    }

    fn folder(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("logforward-status-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_status_is_a_line_of_json_that_leaves_out_what_it_has_not() {
        let mut connected = status("siem", State::Connected);
        connected.sent = 12;
        connected.behind = 340;
        connected.tls =
            Some("TLS 1.3, certificate for logs.example.com, issued by Example CA".into());
        let json = serde_json::to_string(&connected).unwrap();
        assert_eq!(
            json,
            "{\"name\":\"siem\",\"address\":\"logs.example.com:6514\",\"transport\":\"tls\",\
             \"state\":\"connected\",\"since\":\"2026-11-13T10:30:00.123456Z\",\"sent\":12,\
             \"behind\":340,\"tls\":\"TLS 1.3, certificate for logs.example.com, issued by \
             Example CA\"}"
        );
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), connected);
        let mut waiting = status("siem", State::Waiting);
        waiting.error = Some("cannot connect to logs.example.com:6514: Connection refused".into());
        let json = serde_json::to_string(&waiting).unwrap();
        assert!(json.contains("\"state\":\"waiting\""), "{json}");
        assert!(
            json.contains(
                "\"error\":\"cannot connect to logs.example.com:6514: Connection refused\""
            )
        );
        assert!(!json.contains("\"tls\":"), "{json}");
    }

    #[test]
    fn written_files_are_read_back_by_name_and_replaced_whole() {
        let dir = folder("files");
        status("zeta", State::Connecting).write(&dir).unwrap();
        status("alpha", State::Connected).write(&dir).unwrap();
        let mut again = status("alpha", State::Waiting);
        again.error = Some("down".into());
        again.write(&dir).unwrap();
        fs::write(dir.join("broken.json"), "not json").unwrap();
        fs::write(dir.join("other.txt"), "{}").unwrap();
        let all = read_all(&dir);
        assert_eq!(all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["alpha", "zeta"]);
        assert_eq!(all[0], again);
        assert!(!dir.join("alpha.json.new").exists());
        assert!(read_all(&dir.join("missing")).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_files_of_destinations_that_are_gone_are_removed() {
        let dir = folder("remove");
        for name in ["one", "two", "three"] {
            status(name, State::Connected).write(&dir).unwrap();
        }
        remove_others(&dir, &["two"]);
        assert_eq!(read_all(&dir).len(), 1);
        remove(&dir, "two");
        remove(&dir, "two");
        assert!(read_all(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_table_has_a_row_for_each_destination_and_a_line_for_each_error() {
        let mut siem = status("siem", State::Connected);
        siem.sent = 1204;
        siem.behind = 2048;
        siem.tls = Some("TLS 1.3, certificate for logs.example.com, issued by Example CA".into());
        let mut backup = status("backup-box", State::Waiting);
        backup.address = "10.0.0.9:514".into();
        backup.transport = "udp".into();
        backup.since = "2026-11-13T09:00:05.000000Z".into();
        backup.error = Some("cannot connect to 10.0.0.9:514: Connection refused".into());
        backup.tls = Some("left over".into());
        assert_eq!(
            table(&[siem, backup]),
            "NAME        ADDRESS                STATE      SINCE                SENT  BEHIND\n\
             siem        logs.example.com:6514  connected  2026-11-13 10:30:00  1204  2.0K\n\
             backup-box  10.0.0.9:514           waiting    2026-11-13 09:00:05  0     0\n\
             siem: TLS 1.3, certificate for logs.example.com, issued by Example CA\n\
             backup-box: cannot connect to 10.0.0.9:514: Connection refused\n"
        );
    }
}
