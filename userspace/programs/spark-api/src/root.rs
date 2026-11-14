// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which root Spark started (ADR 0473): the record Spark writes at
//! [`PATH`] once Dukovo's root on a disk is mounted, so that the updater
//! and the services that hold the root's subvolumes by name find it
//! without reading a disk raw. A system with no root on a disk, the live
//! system or Rescue, has no record.
//!
//! The record is three lines of words, each a key and its values:
//!
//! ```text
//! disk sda2
//! uuid 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0
//! partitions sda2 sdb1
//! ```
//!
//! `disk` is where the root's Btrfs is mounted, `/volumes/<disk>`; `uuid`
//! is the Btrfs's, as the kernel's command line names it; `partitions`
//! are every partition, or whole disk, the Btrfs is on.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

/// Where Spark writes the record.
pub const PATH: &str = "/system/live/spark/root";

/// The most partitions a record names.
pub const PARTITIONS_MAX: usize = 64;

/// The root Spark started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Root {
    /// Its name under `/volumes`: the partition, or disk, it is mounted
    /// from.
    pub disk: String,
    /// The UUID of its Btrfs, in lowercase.
    pub uuid: String,
    /// The partitions, or whole disks, its Btrfs is on, `disk` among them.
    pub partitions: Vec<String>,
}

impl Root {
    /// The record's text.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "disk {}", self.disk);
        let _ = writeln!(text, "uuid {}", self.uuid);
        let _ = writeln!(text, "partitions {}", self.partitions.join(" "));
        text
    }

    /// Reads a record's `text`; `None` unless it names a disk, a UUID
    /// and the partitions, `disk` among them, each a plain name (no `/`,
    /// not `.` or `..`). A key it does not know is passed over, so that a
    /// later Spark may add one.
    pub fn parse(text: &str) -> Option<Root> {
        let mut root = Root::default();
        for line in text.lines() {
            let mut words = line.split_ascii_whitespace();
            let (Some(key), values) = (words.next(), words) else { continue };
            match key {
                "disk" => root.disk = only(values)?,
                "uuid" => root.uuid = only(values)?.to_ascii_lowercase(),
                "partitions" => {
                    root.partitions = values.map(ToString::to_string).collect();
                    if root.partitions.len() > PARTITIONS_MAX {
                        return None;
                    }
                }
                _ => {}
            }
        }
        let names = core::iter::once(&root.disk).chain(&root.partitions);
        let plain = names.into_iter().all(|name| plain(name));
        let uuid =
            root.uuid.len() == 36 && root.uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
        (plain && uuid && root.partitions.contains(&root.disk)).then_some(root)
    }
}

/// The one value of a key, which has one.
fn only<'a>(mut values: impl Iterator<Item = &'a str>) -> Option<String> {
    let value = values.next()?;
    values.next().is_none().then(|| value.to_string())
}

/// Whether `name` is a plain name under `/volumes` or `/devices`.
fn plain(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::Root;

    const UUID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

    #[test]
    fn a_record_reads_back_as_written() {
        let root = Root {
            disk: "nvme0n1p2".into(),
            uuid: UUID.into(),
            partitions: vec!["nvme0n1p2".into(), "sdb1".into()],
        };
        let text = root.to_text();
        assert_eq!(text, format!("disk nvme0n1p2\nuuid {UUID}\npartitions nvme0n1p2 sdb1\n"));
        assert_eq!(Root::parse(&text), Some(root));
        // A key a later Spark adds is passed over; the UUID is lowercase.
        let later = format!("disk a\nuuid {}\npartitions a\nslot b\n", UUID.to_uppercase());
        assert_eq!(Root::parse(&later).map(|root| root.uuid), Some(UUID.into()));
    }

    #[test]
    fn a_record_that_does_not_hold_together_is_none() {
        let cases = [
            String::new(),
            format!("uuid {UUID}\npartitions a\n"),
            "disk a\nuuid not-a-uuid\npartitions a\n".into(),
            format!("disk a b\nuuid {UUID}\npartitions a\n"),
            // The disk is one of the partitions, and every name is plain.
            format!("disk a\nuuid {UUID}\npartitions b\n"),
            format!("disk ..\nuuid {UUID}\npartitions ..\n"),
            format!("disk a\nuuid {UUID}\npartitions a ../../users\n"),
        ];
        for text in cases {
            assert_eq!(Root::parse(&text), None, "{text}");
        }
        let many: Vec<String> = (0..=super::PARTITIONS_MAX).map(|n| format!("p{n}")).collect();
        let text = format!("disk p0\nuuid {UUID}\npartitions {}\n", many.join(" "));
        assert_eq!(Root::parse(&text), None);
    }
}
