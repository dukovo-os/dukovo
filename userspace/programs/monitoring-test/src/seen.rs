// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the receivers heard, read back from their files so that the test
//! types one short command and not a pipeline of `grep` and `awk`.
//!
//! A phrase is counted in what a message says, not in how it is
//! addressed: in the body of each mail (after its headers, so that a
//! subject does not count) and in the body of each request (so that
//! ntfy's `Title` header does not). It is counted as many times as it
//! occurs, so that a batch which holds a change twice shows it.

use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::KINDS;
use crate::records::{HOOKS, MAIL};

/// How long [`wait`] listens after the mail, for the webhooks that
/// are sent at the same time as it and may come a moment later.
const SETTLE: Duration = Duration::from_secs(3);

/// How often [`wait`] looks.
const LOOK: Duration = Duration::from_millis(500);

/// What one receiver heard a phrase in.
#[derive(Debug, Default, PartialEq, Eq)]
struct Heard {
    /// How many times the phrase occurs in all it was sent.
    times: usize,
    /// How the last message that held it came, for [`report`]'s details.
    how: String,
}

/// The blocks of a records file, each as its lines between the line that
/// starts it and `== end`.
fn blocks(text: &str) -> Vec<Vec<&str>> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line == "== end" {
            blocks.extend(current.take());
        } else if line.starts_with("== ") {
            current = Some(Vec::new());
        } else if let Some(block) = &mut current {
            block.push(line);
        }
    }
    blocks
}

/// The value of the first field `name` of a block: the lines before the
/// one that starts what was sent.
fn field<'a>(block: &[&'a str], name: &str) -> Option<&'a str> {
    let prefix = format!("{name}: ");
    block
        .iter()
        .take_while(|line| !line.starts_with("-- "))
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

/// Every value of the field `name` of a block.
fn fields<'a>(block: &[&'a str], name: &str) -> Vec<&'a str> {
    let prefix = format!("{name}: ");
    block
        .iter()
        .take_while(|line| !line.starts_with("-- "))
        .filter_map(|line| line.strip_prefix(prefix.as_str()))
        .collect()
}

/// The lines of what was sent: a request's body, or a message after its
/// headers.
fn sent<'a>(block: &[&'a str], is_mail: bool) -> Vec<&'a str> {
    let after_marker: Vec<&str> =
        block.iter().skip_while(|line| !line.starts_with("-- ")).skip(1).copied().collect();
    if !is_mail {
        return after_marker;
    }
    match after_marker.iter().position(|line| line.is_empty()) {
        Some(at) => after_marker[at + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// How many times `phrase` occurs in what `block` sent.
fn times(block: &[&str], is_mail: bool, phrase: &str) -> usize {
    sent(block, is_mail).iter().map(|line| line.matches(phrase).count()).sum()
}

/// What the mail server heard `phrase` in.
fn heard_mail(text: &str, phrase: &str) -> Heard {
    let mut heard = Heard::default();
    for block in blocks(text) {
        let count = times(&block, true, phrase);
        if count == 0 {
            continue;
        }
        heard.times += count;
        heard.how = format!(
            "tls={} user={} to={}",
            field(&block, "tls").unwrap_or("-"),
            field(&block, "user").unwrap_or("-"),
            fields(&block, "envelope-to")
                .iter()
                .map(|address| address.trim_matches(['<', '>']))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    heard
}

/// What the webhook receiver heard `phrase` in, in the requests for the
/// webhook `kind`.
fn heard_hooks(text: &str, kind: &str, phrase: &str) -> Heard {
    let mut heard = Heard::default();
    for block in blocks(text) {
        if field(&block, "kind") != Some(kind) {
            continue;
        }
        let count = times(&block, false, phrase);
        if count == 0 {
            continue;
        }
        heard.times += count;
        let token = field(&block, "authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or("none");
        heard.how = format!("tls={} token={token}", field(&block, "tls").unwrap_or("-"));
    }
    heard
}

/// The lines `seen` prints for each of `phrases`: one that counts it for
/// each receiver, `seen alarm: mail 1 ntfy 1 slack 1 discord 1 matrix 1
/// json 0`, and with `details` a line for each receiver that heard it,
/// saying whether TLS was on and what it was signed in or authorized
/// with.
///
/// # Errors
///
/// Why the records could not be read.
pub fn report(folder: &Path, phrases: &[&str], details: bool) -> io::Result<Vec<String>> {
    let mail = fs::read_to_string(folder.join(MAIL))?;
    let hooks = fs::read_to_string(folder.join(HOOKS))?;
    let mut lines = Vec::new();
    for phrase in phrases {
        let mut heard = vec![("mail", heard_mail(&mail, phrase))];
        heard.extend(KINDS.iter().map(|kind| (*kind, heard_hooks(&hooks, kind, phrase))));
        let counts: Vec<String> =
            heard.iter().map(|(name, heard)| format!("{name} {}", heard.times)).collect();
        lines.push(format!("seen {phrase}: {}", counts.join(" ")));
        if details {
            let how = heard.iter().filter(|(_, heard)| heard.times > 0);
            lines.extend(how.map(|(name, heard)| format!("heard {name} {}", heard.how)));
        }
    }
    Ok(lines)
}

/// Waits until the mail server has heard `phrase`, for up to `seconds`,
/// and then a little longer for the webhooks.
///
/// # Errors
///
/// That nothing was heard in time, or why the records could not be read.
pub fn wait(folder: &Path, phrase: &str, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let text = fs::read_to_string(folder.join(MAIL)).map_err(|err| err.to_string())?;
        if heard_mail(&text, phrase).times > 0 {
            thread::sleep(SETTLE);
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("no mail holds \"{phrase}\" after {seconds} seconds"));
        }
        thread::sleep(LOOK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIL_TEXT: &str = "== mail\nenvelope-from: <alerts@example.test>\n\
         envelope-to: <ops@example.test>\nenvelope-to: <me@example.test>\n\
         user: alerts@example.test\ntls: yes\n-- message\nSubject: [dukovo] Warning: Boot test alert\n\
         \nRaised, warning: Boot test alert\n== end\n";

    const NTFY_TEXT: &str = "== hook\nrequest: POST /ntfy-topic\nkind: ntfy\ntls: yes\n\
         title: [dukovo] Warning: Boot test alert\n-- body\nRaised, warning: Boot test alert\n\
         == end\n";

    const MATRIX_TEXT: &str = "== hook\nrequest: PUT /_matrix/client/v3/rooms/x/send/m.room.message/1\n\
         kind: matrix\ntls: yes\nauthorization: Bearer matrix-token\n-- body\n\
         {\"body\":\"Boot test alert\"}\n== end\n";

    fn folder(name: &str, mail: &str, hooks: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("monitoring-seen-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MAIL), mail).unwrap();
        fs::write(dir.join(HOOKS), hooks).unwrap();
        dir
    }

    #[test]
    fn a_phrase_is_counted_in_the_body_and_not_in_a_subject_or_a_header() {
        let heard = heard_mail(MAIL_TEXT, "Boot test alert");
        assert_eq!(heard.times, 1);
        assert_eq!(
            heard.how,
            "tls=yes user=alerts@example.test to=ops@example.test,me@example.test"
        );
        // Only the body has "Raised".
        assert_eq!(heard_mail(MAIL_TEXT, "Warning").times, 0);
        let ntfy = heard_hooks(NTFY_TEXT, "ntfy", "Boot test alert");
        assert_eq!((ntfy.times, ntfy.how.as_str()), (1, "tls=yes token=none"));
    }

    #[test]
    fn every_occurrence_counts_so_that_a_batch_that_holds_a_change_twice_shows_it() {
        let twice = "== hook\nkind: json\ntls: no\n-- body\n\
             {\"title\":\"Boot test alert\"},{\"title\":\"Boot test alert\"}\n== end\n";
        assert_eq!(heard_hooks(twice, "json", "Boot test alert").times, 2);
    }

    #[test]
    fn each_kind_is_counted_for_itself_with_its_token() {
        let both = format!("{NTFY_TEXT}{MATRIX_TEXT}");
        assert_eq!(heard_hooks(&both, "ntfy", "Boot test alert").times, 1);
        let matrix = heard_hooks(&both, "matrix", "Boot test alert");
        assert_eq!((matrix.times, matrix.how.as_str()), (1, "tls=yes token=matrix-token"));
        assert_eq!(heard_hooks(&both, "slack", "Boot test alert"), Heard::default());
    }

    #[test]
    fn the_report_counts_each_receiver_and_the_details_say_how_each_heard() {
        let dir = folder("report", MAIL_TEXT, &format!("{NTFY_TEXT}{MATRIX_TEXT}"));
        assert_eq!(
            report(&dir, &["Boot test alert"], false).unwrap(),
            ["seen Boot test alert: mail 1 ntfy 1 slack 0 discord 0 matrix 1 json 0"]
        );
        assert_eq!(
            report(&dir, &["Boot test alert"], true).unwrap(),
            [
                "seen Boot test alert: mail 1 ntfy 1 slack 0 discord 0 matrix 1 json 0",
                "heard mail tls=yes user=alerts@example.test to=ops@example.test,me@example.test",
                "heard ntfy tls=yes token=none",
                "heard matrix tls=yes token=matrix-token",
            ]
        );
        // Each phrase has a line of its own.
        assert_eq!(
            report(&dir, &["nothing like it", "Raised"], false).unwrap(),
            [
                "seen nothing like it: mail 0 ntfy 0 slack 0 discord 0 matrix 0 json 0",
                "seen Raised: mail 1 ntfy 1 slack 0 discord 0 matrix 0 json 0",
            ]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_message_cut_short_is_not_a_block() {
        let cut = "== hook\nkind: ntfy\n-- body\nBoot test alert\n";
        assert_eq!(heard_hooks(cut, "ntfy", "Boot test alert").times, 0);
    }

    #[test]
    fn waiting_ends_when_the_mail_has_the_phrase_and_fails_when_it_does_not_come() {
        let dir = folder("wait", MAIL_TEXT, "");
        // The settling pause is of seconds: this is the slow test.
        assert!(wait(&dir, "Boot test alert", 5).is_ok());
        let err = wait(&dir, "never said", 1).unwrap_err();
        assert_eq!(err, "no mail holds \"never said\" after 1 seconds");
        fs::remove_dir_all(dir).unwrap();
    }
}
