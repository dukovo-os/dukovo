// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The messages of the Terminal's socket (design 13.7 of C3, ADR 0517).
//! Text messages are JSON; binary ones carry bytes, after the pane's
//! number.
//!
//! ```text
//! page -> admind
//!   {"attach": 12, "cols": 120, "rows": 40, "since": 81920}   since: optional
//!   {"detach": 12}
//!   {"resize": 12, "cols": 120, "rows": 40}
//!   {"ack": 12, "offset": 90112}
//!   {"answer": 12, "text": "\u001b[24;1R"}
//!   binary: [u32 pane, big-endian][typed bytes, at most 16 KiB]
//!
//! admind -> page
//!   {"attached": 12, "terminal": 3, "name": "alice@duke-nas", "admin": false,
//!    "cols": 120, "rows": 40, "offset": 65536, "live": 98304, "replay": true,
//!    "modes": "..."}
//!   {"ended": 12, "why": "exited", "status": 0}
//!   {"detached": 12, "why": "slow"}
//!   {"changed": 3}
//!   {"error": {"id", "title", "detail"}, "pane": 12}
//!   binary: [u32 pane][u64 offset of the first byte, big-endian][output, at most 16 KiB]
//! ```
//!
//! A message from the page that is none of these, or has a key the
//! message does not take or a number out of its range, is refused, and
//! the socket closes with 1008: the page and admind come from one build.
//! `modes`, in an answer to an attach that replays, holds the sequences
//! that put the page's screen in the pane's modes before the replay
//! (`modes.rs`); it is not part of the pane's output and has no offset.
//!
//! `live` is the offset of the first byte the pane writes after the
//! attach. What comes before it was written before the page saw it, and
//! its questions (where is the cursor, what terminal is this) have had
//! their answers or gone unanswered: the page's screen reads it and
//! answers none of them. `answer` carries the screen's answer to a
//! question asked after: admind writes it to the pane from one page alone
//! (`Terminals::answer`), so that a program that asks once is answered
//! once, however many pages show the pane.

use serde_json::{Map, Value, json};

use crate::admind::errors::Failure;

/// The most bytes of typing or output in one binary message.
pub const CHUNK_MAX: usize = 16 << 10;

/// The sizes a pane may have.
pub const COLUMNS: std::ops::RangeInclusive<u16> = 2..=500;
pub const ROWS: std::ops::RangeInclusive<u16> = 1..=300;

/// The longest answer to a question a program asks of its terminal, in
/// bytes.
pub const ANSWER_MAX: usize = 64;

/// A text message from the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Show the pane on this socket, from `since` if the page has seen
    /// its output up to there.
    Attach { pane: u32, cols: u16, rows: u16, since: Option<u64> },
    /// Stop showing it.
    Detach { pane: u32 },
    /// The pane's new size.
    Resize { pane: u32, cols: u16, rows: u16 },
    /// The page has taken in the pane's output up to `offset`.
    Ack { pane: u32, offset: u64 },
    /// The page's screen answers a question the pane's program asked.
    Answer { pane: u32, text: String },
}

/// The message the JSON `text` is, or `None` for one the socket does not
/// take.
pub fn asked(text: &str) -> Option<Asked> {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else { return None };
    let only = |keys: &[&str]| message.keys().all(|key| keys.contains(&key.as_str()));
    if message.contains_key("attach") && only(&["attach", "cols", "rows", "since"]) {
        let since = match message.get("since") {
            None => None,
            Some(value) => Some(value.as_u64()?),
        };
        let (cols, rows) = size(&message)?;
        Some(Asked::Attach { pane: pane(&message, "attach")?, cols, rows, since })
    } else if message.contains_key("detach") && only(&["detach"]) {
        Some(Asked::Detach { pane: pane(&message, "detach")? })
    } else if message.contains_key("resize") && only(&["resize", "cols", "rows"]) {
        let (cols, rows) = size(&message)?;
        Some(Asked::Resize { pane: pane(&message, "resize")?, cols, rows })
    } else if message.contains_key("ack") && only(&["ack", "offset"]) {
        let offset = message.get("offset")?.as_u64()?;
        Some(Asked::Ack { pane: pane(&message, "ack")?, offset })
    } else if message.contains_key("answer") && only(&["answer", "text"]) {
        // An escape sequence of printable ASCII: nothing else can be typed this way.
        let text = message.get("text")?.as_str()?;
        let rest = text.strip_prefix('\x1b')?;
        let fits = !rest.is_empty()
            && text.len() <= ANSWER_MAX
            && rest.bytes().all(|byte| (0x20..0x7f).contains(&byte));
        let pane = pane(&message, "answer")?;
        fits.then(|| Asked::Answer { pane, text: text.to_owned() })
    } else {
        None
    }
}

/// The pane's number under `key`.
fn pane(message: &Map<String, Value>, key: &str) -> Option<u32> {
    u32::try_from(message.get(key)?.as_u64()?).ok()
}

/// The columns and rows a message gives, within [`COLUMNS`] and [`ROWS`].
fn size(message: &Map<String, Value>) -> Option<(u16, u16)> {
    let number = |key: &str, range: std::ops::RangeInclusive<u16>| {
        u16::try_from(message.get(key)?.as_u64()?).ok().filter(|n| range.contains(n))
    };
    Some((number("cols", COLUMNS)?, number("rows", ROWS)?))
}

/// The pane and the bytes of a binary message from the page: what was
/// typed in it. `None` for one with no pane or more than [`CHUNK_MAX`]
/// bytes.
pub fn typed(message: &[u8]) -> Option<(u32, &[u8])> {
    let (pane, bytes) = message.split_first_chunk::<4>()?;
    (bytes.len() <= CHUNK_MAX).then_some((u32::from_be_bytes(*pane), bytes))
}

/// The binary message that gives the page `output` of `pane`, whose first
/// byte is at `offset`.
pub fn output(pane: u32, offset: u64, output: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(12 + output.len());
    message.extend_from_slice(&pane.to_be_bytes());
    message.extend_from_slice(&offset.to_be_bytes());
    message.extend_from_slice(output);
    message
}

/// What the page is told when it attached to a pane.
#[derive(Debug)]
pub struct Attached<'a> {
    pub pane: u32,
    pub terminal: u32,
    pub name: &'a str,
    pub admin: bool,
    pub cols: u16,
    pub rows: u16,
    /// The offset of the first byte that follows.
    pub offset: u64,
    /// The offset of the first byte written after the attach.
    pub live: u64,
    /// Whether what follows is a replay, which the page draws on a new
    /// screen in the modes of `modes`, or exactly what follows what it
    /// had seen.
    pub replay: bool,
    pub modes: String,
}

/// The JSON of `attached`.
pub fn attached(attached: &Attached<'_>) -> String {
    let mut message = json!({
        "attached": attached.pane,
        "terminal": attached.terminal,
        "name": attached.name,
        "admin": attached.admin,
        "cols": attached.cols,
        "rows": attached.rows,
        "offset": attached.offset,
        "live": attached.live,
        "replay": attached.replay,
    });
    if attached.replay {
        message["modes"] = Value::from(attached.modes.as_str());
    }
    message.to_string()
}

/// Why a pane ended, as the page is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Its shell exited.
    Exited,
    /// Someone ended it, or it was not opened for its days.
    Ended,
    /// The console locked: an administrator's pane.
    Locked,
    /// The session signed out: an administrator's pane.
    SignedOut,
    /// The session ran its hours, or was idle: an administrator's pane.
    Idle,
    /// admind stopped.
    Restart,
}

impl Why {
    /// The word the page is told.
    pub fn word(self) -> &'static str {
        match self {
            Why::Exited => "exited",
            Why::Ended => "ended",
            Why::Locked => "locked",
            Why::SignedOut => "signed-out",
            Why::Idle => "idle",
            Why::Restart => "restart",
        }
    }
}

/// The JSON that says `pane` ended for `why`, with the shell's status when
/// it is known; when it is not, the message has no `status`.
pub fn ended(pane: u32, why: Why, status: Option<i32>) -> String {
    let mut message = json!({ "ended": pane, "why": why.word() });
    if let Some(status) = status {
        message["status"] = Value::from(status);
    }
    message.to_string()
}

/// The JSON that says `pane` is no longer shown on this socket, as the page
/// took in its output too slowly.
pub fn detached_slow(pane: u32) -> String {
    json!({ "detached": pane, "why": "slow" }).to_string()
}

/// The JSON that says `terminal` changed elsewhere.
pub fn changed(terminal: u32) -> String {
    json!({ "changed": terminal }).to_string()
}

/// The JSON of `failure`, about `pane`.
pub fn error(failure: &Failure, pane: u32) -> String {
    let mut message = failure.body();
    message["pane"] = Value::from(pane);
    message.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::errors::Kind;

    #[test]
    fn the_pages_five_messages_read() {
        assert_eq!(
            asked(r#"{"attach": 12, "cols": 120, "rows": 40, "since": 81920}"#),
            Some(Asked::Attach { pane: 12, cols: 120, rows: 40, since: Some(81920) })
        );
        assert_eq!(
            asked(r#"{"attach": 12, "cols": 2, "rows": 1}"#),
            Some(Asked::Attach { pane: 12, cols: 2, rows: 1, since: None })
        );
        assert_eq!(asked(r#"{"detach": 4294967295}"#), Some(Asked::Detach { pane: u32::MAX }));
        assert_eq!(
            asked(r#"{"resize": 1, "cols": 500, "rows": 300}"#),
            Some(Asked::Resize { pane: 1, cols: 500, rows: 300 })
        );
        assert_eq!(
            asked(r#"{"ack": 7, "offset": 9007199254740991}"#),
            Some(Asked::Ack { pane: 7, offset: 9_007_199_254_740_991 })
        );
        assert_eq!(
            asked(r#"{"answer": 3, "text": "\u001b[24;1R"}"#),
            Some(Asked::Answer { pane: 3, text: "\x1b[24;1R".into() })
        );
        let longest = format!(r#"{{"answer": 3, "text": "\u001b{}"}}"#, "1".repeat(ANSWER_MAX - 1));
        assert!(asked(&longest).is_some(), "an answer of {ANSWER_MAX} bytes");
    }

    #[test]
    fn a_message_out_of_its_shape_or_range_is_refused() {
        for text in [
            "",
            "[]",
            "12",
            "{}",
            r#"{"attach": 12}"#,
            r#"{"attach": 12, "cols": 1, "rows": 40}"#,
            r#"{"attach": 12, "cols": 501, "rows": 40}"#,
            r#"{"attach": 12, "cols": 80, "rows": 0}"#,
            r#"{"attach": 12, "cols": 80, "rows": 301}"#,
            r#"{"attach": -1, "cols": 80, "rows": 24}"#,
            r#"{"attach": 4294967296, "cols": 80, "rows": 24}"#,
            r#"{"attach": 1.5, "cols": 80, "rows": 24}"#,
            r#"{"attach": "1", "cols": 80, "rows": 24}"#,
            r#"{"attach": 1, "cols": 80, "rows": 24, "since": -4}"#,
            r#"{"attach": 1, "cols": 80, "rows": 24, "since": null}"#,
            r#"{"attach": 1, "cols": 80, "rows": 24, "extra": 1}"#,
            r#"{"detach": 1, "cols": 80}"#,
            r#"{"resize": 1, "cols": 80}"#,
            r#"{"ack": 1}"#,
            r#"{"ack": 1, "offset": -1}"#,
            r#"{"ack": 1, "detach": 2, "offset": 3}"#,
            r#"{"hello": 1}"#,
            r#"{"answer": 1}"#,
            r#"{"answer": 1, "text": ""}"#,
            r#"{"answer": 1, "text": "\u001b"}"#,
            r#"{"answer": 1, "text": "ls\r"}"#,
            r#"{"answer": 1, "text": "[24;1R"}"#,
            r#"{"answer": 1, "text": "\u001b[24;1R\r"}"#,
            r#"{"answer": 1, "text": "\u001b[24;\u001b1R"}"#,
            r#"{"answer": 1, "text": "\u001b[é"}"#,
            r#"{"answer": 1, "text": 5}"#,
            r#"{"answer": 1, "text": "\u001b[0n", "pane": 2}"#,
        ] {
            assert_eq!(asked(text), None, "{text}");
        }
        let long = format!(r#"{{"answer": 3, "text": "\u001b{}"}}"#, "1".repeat(ANSWER_MAX));
        assert_eq!(asked(&long), None, "an answer of more than {ANSWER_MAX} bytes");
    }

    #[test]
    fn typing_names_its_pane_and_is_at_most_16_kib() {
        assert_eq!(typed(&[0, 0, 0, 12, b'l', b's']), Some((12, &b"ls"[..])));
        assert_eq!(typed(&[0, 0, 1, 0]), Some((256, &b""[..])));
        assert_eq!(typed(&[0, 0, 0]), None);
        let mut most = vec![0, 0, 0, 1];
        most.extend(vec![b'a'; CHUNK_MAX]);
        assert_eq!(typed(&most).map(|(_, bytes)| bytes.len()), Some(CHUNK_MAX));
        most.push(b'a');
        assert_eq!(typed(&most), None);
    }

    #[test]
    fn output_carries_its_pane_and_offset() {
        assert_eq!(output(12, 0x0102, b"hi"), [0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 1, 2, b'h', b'i']);
    }

    #[test]
    fn admind_messages_are_the_designs() {
        let mut shown = Attached {
            pane: 12,
            terminal: 3,
            name: "alice@duke-nas",
            admin: false,
            cols: 120,
            rows: 40,
            offset: 65536,
            live: 98304,
            replay: true,
            modes: "\x1b[?1049h".into(),
        };
        let json: Value = serde_json::from_str(&attached(&shown)).unwrap();
        assert_eq!(
            json,
            json!({"attached": 12, "terminal": 3, "name": "alice@duke-nas", "admin": false,
                   "cols": 120, "rows": 40, "offset": 65536, "live": 98304, "replay": true,
                   "modes": "\x1b[?1049h"})
        );
        shown.replay = false;
        let json: Value = serde_json::from_str(&attached(&shown)).unwrap();
        assert_eq!(json.get("modes"), None, "only a replay has modes");
        let ended: Value = serde_json::from_str(&ended(12, Why::Exited, Some(0))).unwrap();
        assert_eq!(ended, json!({"ended": 12, "why": "exited", "status": 0}));
        let unknown: Value = serde_json::from_str(&super::ended(12, Why::Locked, None)).unwrap();
        assert_eq!(unknown, json!({"ended": 12, "why": "locked"}), "an unknown status is left out");
        let words: Vec<_> =
            [Why::Exited, Why::Ended, Why::Locked, Why::SignedOut, Why::Idle, Why::Restart]
                .map(Why::word)
                .into();
        assert_eq!(words, ["exited", "ended", "locked", "signed-out", "idle", "restart"]);
        assert_eq!(detached_slow(12), r#"{"detached":12,"why":"slow"}"#);
        assert_eq!(changed(3), r#"{"changed":3}"#);
        let error: Value =
            serde_json::from_str(&error(&Failure::new(Kind::NoTerminal, "none"), 9)).unwrap();
        assert_eq!(error["error"]["id"], "ADM-131");
        assert_eq!(error["pane"], 9);
    }
}
