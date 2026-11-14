// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The preview of a report (ADR 0532): the whole text, exactly as it would
//! leave this computer, in the pager, and then one key for what to do with
//! it. Nothing is sent or saved before a key says so.
//!
//! | key | does                                                         |
//! |-----|--------------------------------------------------------------|
//! | `S` | sends it                                                     |
//! | `L` | drafts it again without the log sections, when it has any    |
//! | `E` | opens it in `$VISUAL`, `$EDITOR` or `nano`, then cuts it again |
//! | `F` | saves it to a file                                           |
//! | `P` | serves it once to a phone                                    |
//! | `C` | cancels                                                      |
//!
//! Which key does what is [`choose`], a function of the key and nothing
//! else. The key is read as [`crate::prompt::key`] does: at once on a
//! terminal, which is put back as it was whatever happens next, and as the
//! first letter of a line when the input is a script's.
//!
//! The editor works on a copy in a file only its user reads, with a name
//! nobody can guess, which is removed when the editor ends or fails. The
//! text it leaves goes back to reportd to be redacted again, so that a name
//! typed into it is cut like the rest; what the person sees next is what
//! reportd gave back.
//!
//! When the editor does not start: set `EDITOR` to one that is installed.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::protocol::TEXT_MAX;
use super::save;
use crate::dumps;
use crate::prompt::{self, Pressed};
use crate::util::{Fail, fail};

/// What the person chose to do with the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Send it.
    Send,
    /// Draft it again without the logs.
    LeaveOutLogs,
    /// Edit it.
    Edit,
    /// Save it to a file.
    Save,
    /// Serve it once to a phone.
    Phone,
    /// Do none of these.
    Cancel,
}

/// What `key` chooses, as upper or lower case; none for a key that is not
/// one of the choices. `logs` is whether leaving the logs out is offered.
pub fn choose(key: u8, logs: bool) -> Option<Choice> {
    match key.to_ascii_uppercase() {
        b'S' => Some(Choice::Send),
        b'L' if logs => Some(Choice::LeaveOutLogs),
        b'E' => Some(Choice::Edit),
        b'F' => Some(Choice::Save),
        b'P' => Some(Choice::Phone),
        b'C' => Some(Choice::Cancel),
        _ => None,
    }
}

/// The keys offered, as the words of a sentence: `S, E, F, P or C`.
fn keys(logs: bool) -> String {
    let mut keys = vec!["S"];
    if logs {
        keys.push("L");
    }
    keys.extend(["E", "F", "P"]);
    format!("{} or C", keys.join(", "))
}

/// How wide a line of the keys may be.
const LINE_WIDTH: usize = 78;

/// The sentence that names the keys, `Press S to send it, ..., or C to
/// cancel: `, broken into lines of at most [`LINE_WIDTH`] columns between
/// two keys. The last line is the question the key answers, and has no
/// end of line.
fn key_lines(logs: bool, security: bool) -> Vec<String> {
    let send =
        if security { "S to send it to the security team, encrypted" } else { "S to send it" };
    let mut keys = vec![send];
    if logs {
        keys.push("L to leave out the logs");
    }
    keys.extend(["E to edit it", "F to save it to a file", "P to download it on a phone"]);
    let mut pieces: Vec<String> = keys.iter().map(|key| format!("{key}, ")).collect();
    pieces.push("or C to cancel: ".to_string());
    let mut lines = Vec::new();
    let mut line = String::from("Press ");
    for piece in pieces {
        if line.len() + piece.trim_end().len() > LINE_WIDTH && line != "Press " {
            lines.push(line);
            line = String::new();
        }
        line.push_str(&piece);
    }
    lines.push(line);
    lines
}

/// Shows `text` as it would leave, in the pager, and says what the person
/// sees in it: how much redaction cut out, and where it goes.
///
/// # Errors
///
/// If the text cannot be written to the terminal.
pub fn show(text: &str, cuts: u32, security: bool) -> Result<(), Fail> {
    dumps::page(text.as_bytes())?;
    println!();
    for line in notes(text, cuts, security) {
        println!("{line}");
    }
    Ok(())
}

/// What is said under the text: its size, what was cut, and where it goes.
fn notes(text: &str, cuts: u32, security: bool) -> Vec<String> {
    let lines = text.lines().count();
    let mut notes = vec![format!(
        "That is the whole report: {lines} {}, {} bytes. Nothing has left this computer.",
        if lines == 1 { "line" } else { "lines" },
        text.len()
    )];
    if cuts > 0 {
        notes.push(format!(
            "{cuts} {} cut out as names or addresses, each shown as a mark such as [user].",
            if cuts == 1 { "piece was" } else { "pieces were" }
        ));
    }
    if security {
        notes.push(
            "It goes to the security team only, encrypted: no one else reads it.".to_string(),
        );
    }
    notes
}

/// Shows the keys and reads one that chooses, asking again for any other.
///
/// # Errors
///
/// If the terminal cannot be read, or the input ended or Ctrl+C stopped
/// the question: no key was pressed, so nothing was chosen.
pub fn ask(logs: bool, security: bool) -> Result<Choice, Fail> {
    let mut lines = key_lines(logs, security);
    let mut prompt_text = lines.pop().unwrap_or_default();
    for line in lines {
        println!("{}", line.trim_end());
    }
    loop {
        match prompt::key(&prompt_text) {
            Ok(Pressed::Key(key)) => {
                if let Some(choice) = choose(key, logs) {
                    return Ok(choice);
                }
                prompt_text = format!("Press {}: ", keys(logs));
            }
            Ok(Pressed::Stopped) => {
                return Err(fail("stopped: nothing was sent or saved".into()));
            }
            Err(err) => return Err(crate::util::failed(&"the terminal", err)),
        }
    }
}

/// The editor's words from the environment: `$VISUAL`, else `$EDITOR`,
/// else `nano`; the first is the program and the rest its options.
fn editor_words(visual: Option<&str>, editor: Option<&str>) -> Vec<String> {
    [visual, editor]
        .into_iter()
        .flatten()
        .map(|text| text.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .find(|words| !words.is_empty())
        .unwrap_or_else(|| vec!["nano".to_string()])
}

/// A file only its user reads, holding the text while it is edited, and
/// gone when this is dropped.
struct Copy {
    path: PathBuf,
}

impl Copy {
    /// Makes the file in `folder` with `text` in it.
    fn make(folder: &Path, text: &str) -> Result<Copy, Fail> {
        let token = save::token().map_err(fail)?;
        let path = folder.join(format!("report-{}-{token}.txt", std::process::id()));
        let made = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        // It is the file's own, already made; whatever failed after it is
        // not left behind.
        let copy = Copy { path };
        made.map_err(|err| crate::util::failed(&copy.path.display(), err))?;
        Ok(copy)
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Opens `text` in the person's editor, in a copy in `folder`.
///
/// Whether it changed: the new text, or none if the editor left it as it
/// was.
fn edit_in(folder: &Path, text: &str, words: &[String]) -> Result<Option<String>, Fail> {
    let copy = Copy::make(folder, text)?;
    let status =
        Command::new(&words[0]).args(&words[1..]).arg(&copy.path).status().map_err(|err| {
            fail(format!(
                "cannot start {}: {}; set EDITOR to an editor that is installed",
                words[0],
                crate::util::strip_os_error(&err)
            ))
        })?;
    if !status.success() {
        return Err(fail(format!("{} ended with {status}: the text is as it was", words[0])));
    }
    let bytes =
        fs::read(&copy.path).map_err(|err| crate::util::failed(&copy.path.display(), err))?;
    let edited = String::from_utf8(bytes)
        .map_err(|_| fail("the edited text is not UTF-8 text: the text is as it was".into()))?;
    if edited.len() > TEXT_MAX {
        return Err(fail(format!(
            "the edited text is longer than {} KiB: the text is as it was",
            TEXT_MAX / 1024
        )));
    }
    Ok((edited != text).then_some(edited))
}

/// Opens `text` in the editor `$VISUAL`, `$EDITOR` or `nano` names.
///
/// # Errors
///
/// If the editor does not start or fails, or leaves text that is not
/// UTF-8 or is too long: the text stays as it was.
pub fn edit(text: &str) -> Result<Option<String>, Fail> {
    let words = editor_words(
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    );
    edit_in(&std::env::temp_dir(), text, &words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn each_key_chooses_in_either_case() {
        for (key, choice) in [
            (b'S', Choice::Send),
            (b'E', Choice::Edit),
            (b'F', Choice::Save),
            (b'P', Choice::Phone),
            (b'C', Choice::Cancel),
            (b'L', Choice::LeaveOutLogs),
        ] {
            assert_eq!(choose(key, true), Some(choice));
            assert_eq!(choose(key.to_ascii_lowercase(), true), Some(choice));
        }
    }

    #[test]
    fn leaving_the_logs_out_is_a_choice_only_when_offered() {
        assert_eq!(choose(b'L', true), Some(Choice::LeaveOutLogs));
        assert_eq!(choose(b'l', false), None);
        assert_eq!(choose(b'S', false), Some(Choice::Send));
    }

    #[test]
    fn other_keys_choose_nothing() {
        // Enter, Escape, a space, a digit, a letter of no choice, a byte
        // of a longer character, and W, which once sent without logs.
        for key in [b'\n', b'\r', 0x1b, b' ', b'1', b'Q', b'W', 0xc3, 0] {
            assert_eq!(choose(key, true), None, "{key:#x}");
        }
    }

    #[test]
    fn the_keys_are_named_in_one_sentence_that_ends_with_the_cancel() {
        assert_eq!(keys(true), "S, L, E, F, P or C");
        assert_eq!(keys(false), "S, E, F, P or C");
        let with = key_lines(true, false);
        assert_eq!(
            with.concat(),
            "Press S to send it, L to leave out the logs, E to edit it, F to save it to a file, \
             P to download it on a phone, or C to cancel: "
        );
        assert!(with.last().unwrap().ends_with("or C to cancel: "));
        let without = key_lines(false, false).concat();
        assert!(!without.contains("leave out") && without.contains("E to edit it"));
        assert!(key_lines(false, true)[0].contains("S to send it to the security team"));
    }

    #[test]
    fn the_lines_of_the_keys_fit_a_narrow_terminal_and_break_between_keys() {
        for (logs, security) in [(true, false), (true, true), (false, false), (false, true)] {
            let lines = key_lines(logs, security);
            assert!(lines.len() >= 2);
            for line in &lines {
                assert!(line.trim_end().chars().count() <= LINE_WIDTH, "{line}");
            }
            // A line that breaks ends after a key's comma.
            for line in &lines[..lines.len() - 1] {
                assert!(line.trim_end().ends_with(','), "{line}");
            }
        }
    }

    #[test]
    fn the_notes_say_the_size_the_cuts_and_where_it_goes() {
        let plain = notes("a\nb\n", 0, false);
        assert_eq!(plain.len(), 1);
        assert!(plain[0].contains("2 lines, 4 bytes") && plain[0].contains("Nothing has left"));
        assert!(notes("a", 0, false)[0].contains("1 line,"));
        let cut = notes("a\n", 1, true);
        assert!(cut[1].starts_with("1 piece was cut out"));
        assert!(cut[2].contains("security team only"));
        assert!(notes("a\n", 3, false)[1].starts_with("3 pieces were cut out"));
    }

    #[test]
    fn the_editor_is_visual_then_editor_then_nano() {
        let words = |visual, editor| editor_words(visual, editor);
        assert_eq!(words(Some("vi -u NONE"), Some("ed")), ["vi", "-u", "NONE"]);
        assert_eq!(words(None, Some("ed")), ["ed"]);
        assert_eq!(words(Some("  "), Some("ed")), ["ed"]);
        assert_eq!(words(Some(""), None), ["nano"]);
        assert_eq!(words(None, None), ["nano"]);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("report-preview-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_editor_that_changes_the_text_gives_the_new_text_and_leaves_no_copy() {
        let dir = scratch("changed");
        // The copy's path comes last on the command line, as the shell's `$1`.
        let words =
            vec!["sh".to_string(), "-c".into(), "printf 'new\\n' > \"$1\"".into(), "editor".into()];
        let edited = edit_in(&dir, "old\n", &words).unwrap();
        assert_eq!(edited.as_deref(), Some("new\n"));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_editor_that_changes_nothing_gives_none() {
        let dir = scratch("same");
        let words = vec!["true".to_string()];
        assert_eq!(edit_in(&dir, "text\n", &words).unwrap(), None);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_copy_is_the_users_alone_while_the_editor_has_it() {
        let dir = scratch("private");
        let seen = dir.join("mode");
        let script = format!("stat -c %a \"$1\" > {}", seen.display());
        let words = vec!["sh".to_string(), "-c".into(), script, "editor".into()];
        edit_in(&dir, "text\n", &words).unwrap();
        assert_eq!(fs::read_to_string(&seen).unwrap().trim(), "600");
        fs::remove_dir_all(&dir).unwrap();
        let copy_dir = scratch("mode");
        let copy = Copy::make(&copy_dir, "x").unwrap();
        assert_eq!(fs::metadata(&copy.path).unwrap().permissions().mode() & 0o777, 0o600);
        drop(copy);
        assert_eq!(fs::read_dir(&copy_dir).unwrap().count(), 0);
        fs::remove_dir_all(&copy_dir).unwrap();
    }

    #[test]
    fn an_editor_that_fails_or_is_missing_leaves_the_text_and_no_copy() {
        let dir = scratch("fails");
        let err = edit_in(&dir, "text\n", &["false".to_string()]).unwrap_err();
        assert!(err.message.contains("the text is as it was"), "{}", err.message);
        let err = edit_in(&dir, "text\n", &["no-such-editor-here".to_string()]).unwrap_err();
        assert!(err.message.contains("set EDITOR"), "{}", err.message);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn text_that_is_not_utf8_or_too_long_is_refused() {
        let dir = scratch("bad");
        let words =
            vec!["sh".to_string(), "-c".into(), "printf '\\377' > \"$1\"".into(), "editor".into()];
        assert!(edit_in(&dir, "text\n", &words).unwrap_err().message.contains("not UTF-8"));
        let long = format!("head -c {} /dev/zero | tr '\\0' x > \"$1\"", TEXT_MAX + 1);
        let words = vec!["sh".to_string(), "-c".into(), long, "editor".into()];
        assert!(edit_in(&dir, "text\n", &words).unwrap_err().message.contains("longer than"));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
