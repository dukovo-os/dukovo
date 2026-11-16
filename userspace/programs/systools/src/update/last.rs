// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The last check that ran as root, kept in
//! `/system/state/sysupdate/last-check.json`, and `update last`, which
//! says it.
//!
//! The daily check of updated(8) and `elevate update check --json` keep
//! the document `update check --json` printed, with when and by whom. A
//! check by a person who is not root cannot write there and keeps
//! nothing: what the web console shows of the last check is what root
//! last saw, and "Check now" shows a person's own.

use std::io;
use std::path::Path;

use serde::Deserialize;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail};

/// The record, one line of JSON, 0644, replaced whole.
pub(crate) const FILE: &str = "/system/state/sysupdate/last-check.json";

/// What `update last` exits with when no check is kept.
const NONE_KEPT: i32 = 3;

/// Who made the check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum By {
    /// updated(8), the daily check.
    Updated,
    /// `elevate update check`.
    UpdateCheck,
}

impl By {
    /// The word the record has for it.
    fn word(self) -> &'static str {
        match self {
            By::Updated => "updated",
            By::UpdateCheck => "update check",
        }
    }
}

/// The record, as far as `update last` reads it; the line is printed as
/// it was kept.
#[derive(Debug, Deserialize)]
struct Record {
    format: u32,
    checked_ms: u64,
    by: String,
    updates: serde_json::Value,
}

/// The text of the record of a check made at `checked_ms` by `by`, whose
/// document, as `update check --json` printed it, is `updates`: one line.
fn text(checked_ms: u64, by: By, updates: &str) -> String {
    format!(
        "{{\"format\":{},\"checked_ms\":{checked_ms},\"by\":\"{}\",\"updates\":{updates}}}\n",
        crate::json::FORMAT,
        by.word()
    )
}

/// Keeps the check `updates`, the line `update check --json` printed, as
/// the last one, made now by `by`. A person who is not root cannot, and
/// it is no fault: there is nothing to keep it in.
pub(crate) fn keep(by: By, updates: &str) {
    if !is_root() {
        return;
    }
    let now = util::now_ns() / 1_000_000;
    if let Err(why) = keep_in(Path::new(FILE), &text(now, by, updates)) {
        eprintln!("update: cannot keep the check in {FILE}: {}", util::strip_os_error(&why));
    }
}

/// Whether this process runs as root: the one that may write what update
/// keeps for everyone.
pub(crate) fn is_root() -> bool {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Writes `text` to `file`: to a file beside it, which is renamed over it,
/// so that a reader finds the old record or the new one.
fn keep_in(file: &Path, text: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut partial = file.as_os_str().to_owned();
    partial.push(".partial");
    std::fs::write(&partial, text)?;
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&partial, file)
}

/// `update last [--json]`: the last check kept, or none.
pub(crate) fn last(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let kept = match std::fs::read_to_string(FILE) {
        Ok(text) => Some(text),
        Err(why) if why.kind() == io::ErrorKind::NotFound => None,
        Err(why) => return Err(util::failed(&FILE, why)),
    };
    say(kept.as_deref(), json)
}

/// What `update last` says of the record `kept`.
fn say(kept: Option<&str>, json: bool) -> Result<i32, Fail> {
    let Some(text) = kept else {
        if json {
            println!("null");
        } else {
            println!("update: no check is kept yet: the daily check keeps the first");
        }
        return Ok(NONE_KEPT);
    };
    let record: Record = serde_json::from_str(text)
        .map_err(|why| fail(format!("{FILE}: not a record of a check: {why}")))?;
    if record.format != crate::json::FORMAT {
        return Err(fail(format!("{FILE}: format {} is not one this update reads", record.format)));
    }
    if json {
        print!("{}", text.trim_end().to_string() + "\n");
        return Ok(0);
    }
    let when = i64::try_from(record.checked_ms / 1000).unwrap_or(0);
    println!("update: last checked {} by {}", util::local_time(when, "%Y-%m-%d %H:%M"), record.by);
    for offer in record.updates["offers"].as_array().into_iter().flatten() {
        let word = |key: &str| offer[key].as_str().unwrap_or("none").to_string();
        println!("  {}: {} to {}, {}", word("item"), word("from"), word("to"), word("state"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A check's document, as `update check --json` prints it.
    const UPDATES: &str = "{\"format\":1,\"offers\":[{\"item\":\"system\",\"from\":\"1.0.0\",\
         \"to\":\"1.0.1\",\"state\":\"update\"},{\"item\":\"firmware\",\"from\":null,\"to\":null,\
         \"state\":\"current\"}],\"unchecked\":[]}";

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("update-last-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_record_holds_the_check_as_it_was_printed_with_when_and_by_whom() {
        let record = text(1_794_000_000_123, By::Updated, UPDATES);
        assert!(
            record.starts_with("{\"format\":1,\"checked_ms\":1794000000123,\"by\":\"updated\",")
        );
        assert!(record.ends_with(&format!("\"updates\":{UPDATES}}}\n")));
        assert_eq!(record.matches('\n').count(), 1);
        let read: Record = serde_json::from_str(&record).unwrap();
        assert_eq!((read.format, read.checked_ms), (1, 1_794_000_000_123));
        assert_eq!(read.updates["offers"][0]["to"], "1.0.1");
        let by_hand = text(1, By::UpdateCheck, UPDATES);
        assert!(by_hand.contains("\"by\":\"update check\""));
    }

    #[test]
    fn a_new_record_replaces_the_old_whole_and_is_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("keeps");
        let file = dir.join("last-check.json");
        keep_in(&file, &text(1, By::Updated, UPDATES)).unwrap();
        keep_in(&file, &text(2, By::UpdateCheck, UPDATES)).unwrap();
        let read: Record = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((read.checked_ms, read.by.as_str()), (2, "update check"));
        assert!(!dir.join("last-check.json.partial").exists());
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o644);
        let _ = std::fs::remove_dir_all(dir); // a test's leftovers
    }

    #[test]
    fn with_none_kept_it_is_null_and_status_3() {
        assert_eq!(say(None, true).unwrap(), 3);
        assert_eq!(say(None, false).unwrap(), 3);
    }

    #[test]
    fn a_record_that_is_not_one_is_an_error_not_none() {
        assert!(say(Some("not json"), true).unwrap_err().message.contains("not a record"));
        let later = text(1, By::Updated, UPDATES).replacen("\"format\":1", "\"format\":2", 1);
        assert!(say(Some(&later), true).unwrap_err().message.contains("format 2"));
        assert_eq!(say(Some(&text(1, By::Updated, UPDATES)), true).unwrap(), 0);
        assert_eq!(say(Some(&text(1, By::Updated, UPDATES)), false).unwrap(), 0);
    }
}
