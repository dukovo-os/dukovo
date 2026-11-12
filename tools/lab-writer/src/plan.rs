// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lab's plan for one write: which disk, how big the image is, its
//! hash, and where it holds data. The lab serves it as JSON at `plan`
//! (dukovo-os/lab's docs/net-boot.md):
//!
//! ```json
//! {"version": 1, "disk": "S3Z5NB0K123456A", "size": 608174080,
//!  "sha256": "<hex>", "extents": [[0, 1048576], [4194304, 65536]]}
//! ```
//!
//! Everything in the image's first `size` bytes outside the extents is
//! zeros, which the writer zeroes on the disk itself rather than fetching.

use serde_json::Value;

/// The plan's format this writer knows.
pub const VERSION: u64 = 1;

/// One write, checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The serial number of the disk to write, as the lab's machine file
    /// gives it.
    pub disk: String,
    /// The image's size in bytes.
    pub size: u64,
    /// The SHA-256 of the whole image, in lower-case hexadecimal.
    pub sha256: String,
    /// Where the image holds data, as offset and length in bytes: in
    /// order, never touching, none empty, all within `size`.
    pub extents: Vec<(u64, u64)>,
}

impl Plan {
    /// Reads a plan from the lab's JSON, refusing one this writer cannot
    /// follow exactly.
    pub fn parse(json: &[u8]) -> Result<Plan, String> {
        let value: Value =
            serde_json::from_slice(json).map_err(|err| format!("the plan is not JSON: {err}"))?;
        let version = value.get("version").and_then(Value::as_u64);
        if version != Some(VERSION) {
            return Err(format!(
                "the plan's version is {}, and this writer knows version {VERSION} alone",
                value.get("version").map_or_else(|| "missing".into(), Value::to_string)
            ));
        }
        let disk = value
            .get("disk")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|disk| !disk.is_empty())
            .ok_or("the plan names no disk")?
            .to_string();
        let size = value.get("size").and_then(Value::as_u64).ok_or("the plan gives no size")?;
        let sha256 = value
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("the plan's sha256 is not 64 hexadecimal digits")?
            .to_ascii_lowercase();
        let mut extents = Vec::new();
        for extent in
            value.get("extents").and_then(Value::as_array).ok_or("the plan has no extents")?
        {
            let pair = extent.as_array().filter(|pair| pair.len() == 2);
            let numbers = pair.and_then(|pair| Some((pair[0].as_u64()?, pair[1].as_u64()?)));
            let Some((offset, length)) = numbers else {
                return Err(format!("the plan's extent {extent} is not [offset, length]"));
            };
            extents.push((offset, length));
        }
        let plan = Plan { disk, size, sha256, extents };
        plan.check()?;
        Ok(plan)
    }

    /// Checks the extents: each holds data, follows the one before with
    /// a gap between them, and ends within the image.
    fn check(&self) -> Result<(), String> {
        let mut end_before: Option<u64> = None;
        for &(offset, length) in &self.extents {
            let end = offset.checked_add(length).filter(|_| length > 0);
            let Some(end) = end.filter(|&end| end <= self.size) else {
                return Err(format!(
                    "the plan's extent [{offset}, {length}] is empty or ends past the image's \
                     {} bytes",
                    self.size
                ));
            };
            if end_before.is_some_and(|before| offset <= before) {
                return Err(format!(
                    "the plan's extent at {offset} is out of order or touches the one before"
                ));
            }
            end_before = Some(end);
        }
        Ok(())
    }

    /// How many bytes of data the lab sends: the extents' lengths.
    pub fn data_bytes(&self) -> u64 {
        self.extents.iter().map(|&(_, length)| length).sum()
    }

    /// The runs of zeros in the image's first `size` bytes: everything
    /// between the extents, as offset and length.
    pub fn gaps(&self) -> Vec<(u64, u64)> {
        let mut gaps = Vec::new();
        let mut at = 0;
        for &(offset, length) in &self.extents {
            if offset > at {
                gaps.push((at, offset - at));
            }
            at = offset + length;
        }
        if self.size > at {
            gaps.push((at, self.size - at));
        }
        gaps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    fn plan(extents: &str) -> Result<Plan, String> {
        Plan::parse(
            format!(
                r#"{{"version": 1, "disk": " S3Z5NB0K ", "size": 1000, "sha256": "{HASH}",
                    "extents": {extents}}}"#
            )
            .as_bytes(),
        )
    }

    #[test]
    fn reads_the_labs_plan() {
        let plan = plan("[[0, 100], [300, 50], [900, 100]]").unwrap();
        assert_eq!(plan.disk, "S3Z5NB0K");
        assert_eq!(plan.sha256, HASH.to_ascii_lowercase());
        assert_eq!(plan.data_bytes(), 250);
        assert_eq!(plan.gaps(), [(100, 200), (350, 550)]);
    }

    #[test]
    fn an_image_of_zeros_alone_is_one_gap() {
        let plan = plan("[]").unwrap();
        assert_eq!(plan.data_bytes(), 0);
        assert_eq!(plan.gaps(), [(0, 1000)]);
    }

    #[test]
    fn refuses_what_it_cannot_follow_exactly() {
        for (extents, says) in [
            ("[[0, 100], [100, 50]]", "touches"),
            ("[[300, 50], [0, 100]]", "out of order"),
            ("[[0, 0]]", "empty"),
            ("[[900, 101]]", "past the image"),
            ("[[18446744073709551615, 2]]", "past the image"),
            ("[[0]]", "is not [offset, length]"),
            ("[[0, -1]]", "is not [offset, length]"),
        ] {
            let err = plan(extents).unwrap_err();
            assert!(err.contains(says), "{extents}: {err}");
        }
        let other = format!(r#"{{"version": 2, "disk": "D", "size": 1, "sha256": "{HASH}"}}"#);
        assert!(Plan::parse(other.as_bytes()).unwrap_err().contains("version 1 alone"));
        let no_disk = format!(r#"{{"version": 1, "disk": " ", "size": 1, "sha256": "{HASH}"}}"#);
        assert!(Plan::parse(no_disk.as_bytes()).unwrap_err().contains("no disk"));
        let short = r#"{"version": 1, "disk": "D", "size": 1, "sha256": "abc", "extents": []}"#;
        assert!(Plan::parse(short.as_bytes()).unwrap_err().contains("64 hexadecimal"));
        assert!(Plan::parse(b"<html>").unwrap_err().contains("not JSON"));
    }
}
