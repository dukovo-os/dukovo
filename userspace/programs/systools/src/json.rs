// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The JSON that `--json` prints (ADR 0541): one value on one line, an
//! object whose first member is `"format": 1`, so that a script reads
//! what the commands say without parsing their text.
//!
//! Each command's document is a typed struct, serialized with serde, and
//! its man page lists the fields. Names are snake_case, times are RFC
//! 3339 in UTC, sizes are bytes, and a value the system does not know is
//! `null`, not left out. A later field may be added without a new
//! [`FORMAT`]; a script ignores the members it does not know. An error
//! prints as the text form's does, to standard error with the command's
//! exit status, and no JSON.
//!
//! `dukovo.system`, the Ansible collection, reads these documents. The
//! unit test of each command holds its fields against a golden file in
//! `tests/json`.

use serde::Serialize;

use crate::util::{self, Fail, fail};

/// The version of the documents. A script refuses a later one: it may
/// mean something else by the members it has in common.
pub(crate) const FORMAT: u32 = 1;

/// A command's document: `body`'s members after `format`.
#[derive(Serialize)]
struct Document<'a, Body: Serialize> {
    format: u32,
    #[serde(flatten)]
    body: &'a Body,
}

/// The line `--json` prints for `body`, without its newline.
pub(crate) fn line<Body: Serialize>(body: &Body) -> Result<String, Fail> {
    serde_json::to_string(&Document { format: FORMAT, body })
        .map_err(|why| fail(format!("cannot write JSON: {why}")))
}

/// Prints the line for `body`.
pub(crate) fn print<Body: Serialize>(body: &Body) -> Result<(), Fail> {
    println!("{}", line(body)?);
    Ok(())
}

/// `seconds` since 1970 as RFC 3339 in UTC, as every time of a document
/// is written.
pub(crate) fn time(seconds: i64) -> String {
    util::time_text(seconds, "%Y-%m-%dT%H:%M:%SZ", true)
}

/// Writes `seconds` as [`time`] does, for a member marked
/// `#[serde(serialize_with = "json::serialize_time")]`, which calls it with
/// a reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn serialize_time<S: serde::Serializer>(
    seconds: &i64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&time(*seconds))
}

/// Fails unless `line`, a document, has the members and values of the
/// golden file `golden`, and its first member is `format`. The failure
/// prints the document, to paste into the golden file when the change is
/// meant.
#[cfg(test)]
pub(crate) fn assert_golden(line: &str, golden: &str) {
    assert!(line.starts_with("{\"format\":1,"), "{line}");
    let document: serde_json::Value = serde_json::from_str(line).expect("a document is JSON");
    let expected: serde_json::Value = serde_json::from_str(golden).expect("a golden file is JSON");
    assert_eq!(
        document,
        expected,
        "the document is:\n{}",
        serde_json::to_string_pretty(&document).unwrap()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Body {
        things: Vec<u8>,
        when: Option<String>,
    }

    #[test]
    fn a_document_starts_with_its_format_and_keeps_the_members_in_order() {
        let body = Body { things: vec![1, 2], when: None };
        assert_eq!(line(&body).unwrap(), r#"{"format":1,"things":[1,2],"when":null}"#);
    }

    #[test]
    fn times_are_rfc_3339_in_utc() {
        assert_eq!(time(0), "1970-01-01T00:00:00Z");
        assert_eq!(time(1_794_000_000), "2026-11-06T21:20:00Z");
    }
}
