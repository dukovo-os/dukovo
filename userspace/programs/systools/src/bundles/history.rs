// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bundled(8) did to bundles, and when (ADR 0384): each install,
//! update, rollback and removal, each hold and its release, and each
//! update that waits for a person's approval. `bundle history` shows a
//! person theirs and everyone's. It is a file of lines, tab-separated,
//! in `/system/state/bundled`, which only bundled writes, and which keeps
//! the newest thousand or so.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;

use super::registry::Place;

/// The file, in `/system/state/bundled`.
pub const HISTORY: &str = "history";
/// Past this size, the file is written again with the newest lines.
const MAX_BYTES: u64 = 256 * 1024;
/// How many lines are kept then.
const KEPT: usize = 1000;

/// What was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Installed,
    Updated,
    RolledBack,
    Removed,
    Held,
    Released,
    /// An update that asks for more than the version it would replace,
    /// which waits for a person to say yes.
    Waiting,
    /// An update that failed.
    Failed,
}

impl What {
    pub fn word(self) -> &'static str {
        match self {
            What::Installed => "installed",
            What::Updated => "updated",
            What::RolledBack => "rolled-back",
            What::Removed => "removed",
            What::Held => "held",
            What::Released => "released",
            What::Waiting => "waiting",
            What::Failed => "failed",
        }
    }

    fn read(word: &str) -> Result<What, String> {
        Ok(match word {
            "installed" => What::Installed,
            "updated" => What::Updated,
            "rolled-back" => What::RolledBack,
            "removed" => What::Removed,
            "held" => What::Held,
            "released" => What::Released,
            "waiting" => What::Waiting,
            "failed" => What::Failed,
            _ => return Err(format!("{word:?} is not something bundled does")),
        })
    }
}

/// Something bundled did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// When, in seconds since 1970.
    pub when: u64,
    pub place: Place,
    pub id: String,
    pub what: What,
    /// The version before and after, as far as there was one.
    pub from: Option<String>,
    pub to: Option<String>,
    /// Who asked for it; none when bundled did it by itself.
    pub by: Option<u32>,
    /// Where it came from, or why: a few words.
    pub note: String,
}

/// A field: one line, without tabs; `-` for none.
fn field(text: Option<&str>) -> String {
    match text {
        None | Some("") => "-".into(),
        Some(text) => text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect(),
    }
}

fn optional(word: &str) -> Option<String> {
    (word != "-").then(|| word.into())
}

impl Event {
    /// Its line, with its newline.
    pub fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.when,
            self.place,
            field(Some(&self.id)),
            self.what.word(),
            field(self.from.as_deref()),
            field(self.to.as_deref()),
            self.by.map_or_else(|| "auto".into(), |uid| uid.to_string()),
            field(Some(&self.note)),
        )
    }

    /// Reads a line, without its newline.
    pub fn read(line: &str) -> Result<Event, String> {
        let words: Vec<&str> = line.split('\t').collect();
        let [when, place, id, what, from, to, by, note] = words.as_slice() else {
            return Err(format!("{} fields where 8 belong", words.len()));
        };
        manifest::bundle::check_full_id(id)?;
        Ok(Event {
            when: when.parse().map_err(|_| format!("{when:?} is not a time"))?,
            place: Place::parse(place)?,
            id: (*id).into(),
            what: What::read(what)?,
            from: optional(from),
            to: optional(to),
            by: match *by {
                "auto" => None,
                uid => Some(uid.parse().map_err(|_| format!("{uid:?} is not a user"))?),
            },
            note: optional(note).unwrap_or_default(),
        })
    }
}

/// Adds `event` to the history at `path`, which is made, mode 0600, if
/// it is not there; past [`MAX_BYTES`], the newest [`KEPT`] lines are
/// kept.
///
/// # Errors
///
/// If the file cannot be written.
pub fn add(path: &str, event: &Event) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().append(true).create(true).mode(0o600).open(path)?;
    file.write_all(event.line().as_bytes())?;
    if file.metadata()?.len() > MAX_BYTES {
        let text = fs::read_to_string(path)?;
        let lines: Vec<&str> = text.lines().collect();
        let kept = lines[lines.len().saturating_sub(KEPT)..].join("\n") + "\n";
        let new = format!("{path}.new");
        let _ = fs::remove_file(&new);
        let mut out = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&new)?;
        out.write_all(kept.as_bytes())?;
        out.sync_all()?;
        fs::rename(&new, path)?;
    }
    Ok(())
}

/// The history at `path`, oldest first. A line that does not read is
/// left out: the history only tells, and decides nothing.
pub fn read(path: &str) -> Vec<Event> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| Event::read(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(n: u64) -> Event {
        Event {
            when: 1_793_000_000 + n,
            place: Place::User(1000),
            id: "org.example.notes".into(),
            what: What::Updated,
            from: Some("1.0.0".into()),
            to: Some("1.1.0".into()),
            by: Some(1000),
            note: "from catalog dukovo".into(),
        }
    }

    #[test]
    fn events_read_back() {
        let e = event(0);
        assert_eq!(
            e.line(),
            "1793000000\t1000\torg.example.notes\tupdated\t1.0.0\t1.1.0\t1000\tfrom catalog dukovo\n"
        );
        assert_eq!(Event::read(e.line().trim_end()), Ok(e));
        let by_itself = Event {
            place: Place::Everyone,
            what: What::Waiting,
            from: None,
            by: None,
            note: "it now asks for\tnetwork.outbound".into(),
            ..event(1)
        };
        let line = by_itself.line();
        assert!(line.contains("\teveryone\t") && line.contains("\t-\t1.1.0\tauto\t"), "{line}");
        let back = Event::read(line.trim_end()).unwrap();
        assert_eq!(back.note, "it now asks for network.outbound");
        assert_eq!(back.by, None);
        assert!(Event::read("1\t1000\tNotes\tupdated\t-\t-\t1\t-").is_err());
        assert!(Event::read("1\t1000\torg.example.notes\tbought\t-\t-\t1\t-").is_err());
    }

    #[test]
    fn the_newest_are_kept() {
        let dir = std::env::temp_dir().join(format!("history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(HISTORY);
        let path = path.to_str().unwrap();
        let _ = fs::remove_file(path);
        for n in 0..3000 {
            add(path, &event(n)).unwrap();
        }
        let all = read(path);
        assert!(all.len() <= 3000 && all.len() >= KEPT, "{}", all.len());
        assert_eq!(all.last().unwrap().when, 1_793_000_000 + 2999);
        assert!(fs::metadata(path).unwrap().len() <= MAX_BYTES + 200);
        fs::write(path, "junk\n").unwrap();
        add(path, &event(5)).unwrap();
        assert_eq!(read(path), vec![event(5)]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
