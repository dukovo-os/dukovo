// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Storage page's actions (ADR 0561): the disks and volumes, how much
//! each is used over time, a disk's health, a check of every block of a
//! filesystem, growing a disk, and the log's own space and limits. The area
//! is `storage`, the scope a token needs.
//!
//! `storage.show` runs `storage show --json` as the person: it reads what
//! everyone may, including the live files of the diskhealth service, so a
//! reader sees each disk's verdict without opening a disk. `storage.usage`
//! runs `metrics show` as the person, with the series and the range that
//! make the page's two charts: what the disks read and write over the last
//! hour, and what the filesystems used and have over the last 30 days.
//!
//! `storage.raid` runs `storage raid --json` as the person too: the
//! RAID controllers, their volumes, disks and battery, which the kernel
//! shows to anyone.
//!
//! The rest are an administrator's: `storage.health` is `diskhealth --json
//! DISK`, which opens the disk, so it is root's, and its answer comes with
//! the status bits of `diskhealth` that tell a state, which are the
//! answer's too ([`Health::ANSWERS`]). `storage.scrub` runs `scrub start -B
//! --progress MOUNT` as a task: reading every block takes hours, and its
//! progress lines are the task's steps. A scrub that finds errors no copy
//! mends ends with status 3, and the kept result says what it found.
//! `storage.grow` is `storage grow DISK`, and says whether it changed
//! anything by reading `storage show --json` before and after, and
//! comparing the sizes of the disk's partitions and of the volumes that
//! use them.
//!
//! The log's three are `logtrim`'s: `storage.log` shows the room the log
//! takes and its limits (`logtrim --show --json`, which asks logd, so it is
//! root's), `storage.log-set` writes limits into `/system/settings/log`
//! (`logtrim set`), and `storage.log-trim` trims the log now. The limits
//! are checked by the command's own function ([`crate::logtrim::checked`]),
//! so that the page, the API and a terminal refuse the same values in the
//! same words.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::trial::refusal;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo};
use crate::logtrim::{self, SETTINGS};
use crate::storage::is_disk_name;

/// The area of these actions: the scope a token needs, `storage`.
const AREA: &str = "storage";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::storage::tests",
    route: "admin-storage",
    same_as_command: "admin-storage",
    browser: "web-console/storage",
    ansible: "web-console/ansible",
};

/// The most bytes a path of the machine has.
const PATH_MAX: usize = 4096;

/// What an absolute path matches, apart from its length: it starts with
/// `/` and has no NUL.
const PATH_PATTERN: &str = "^/[^\u{0}]*$";

/// What a disk's name is made of: the schema's words, which the rule
/// [`is_disk_name`] says exactly.
const DISK_PATTERN: &str = "^[a-z0-9]+$";

/// The most characters of a disk's name.
const DISK_MAX: usize = 32;

/// What a size is made of: digits, and a unit with `iB` or `B` after it.
/// [`crate::util::parse_size`], which `logtrim` checks with, is the exact
/// rule.
const SIZE_PATTERN: &str = "^[0-9]+([KkMmGgTtPp]([Ii]?[Bb])?)?$";

/// The most characters of a size: 20 digits, a unit and `iB`.
const SIZE_MAX: usize = 24;

/// The most days the log's keys take.
const DAYS_MOST: i64 = 3650;

/// Each range the usage chart offers: its word, the series it draws, and
/// the `--since` and `--every` that make it. The disks' reads and writes
/// are drawn over an hour, a point a minute; what the filesystems use and
/// have, over 30 days, a point every six hours.
const RANGES: [(&str, [&str; 2], &str, &str); 2] = [
    ("1h", ["disk.read", "disk.write"], "-1h", "1m"),
    ("30d", ["fs.used", "fs.size"], "-30d", "6h"),
];

/// The words of the ranges.
const WORDS: [&str; 2] = [RANGES[0].0, RANGES[1].0];

/// `storage show --json`: the disks, the Btrfs volumes and the other
/// filesystems.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "storage.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/storage";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the disks and volumes";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["storage", "show", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "all": true }), json!({ "disk": "vda" })],
            undo: None,
        }
    }
}

/// `storage raid --json`: the RAID controllers with their volumes, disks,
/// battery and last events; `{"controllers": []}` on a machine with none.
pub struct Raid;

impl Action for Raid {
    const ID: &'static str = "storage.raid";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/storage/raid";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the RAID controllers";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["storage", "raid", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "controller": 0 })], undo: None }
    }
}

/// How far back the usage goes.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    /// `1h` or `30d`.
    pub range: String,
}

impl Range {
    /// The series, `--since` and `--every` of the range, if it is one of
    /// the words.
    fn plan(&self) -> Option<(&'static [&'static str], &'static str, &'static str)> {
        RANGES
            .iter()
            .find(|(word, ..)| *word == self.range)
            .map(|(_, series, since, every)| (series.as_slice(), *since, *every))
    }
}

impl Args for Range {
    const FIELDS: &'static [Field] = &[Field::one_of(
        "range",
        "How far back the usage goes: 1h for what the disks read and write, 30d for the space used.",
        &WORDS,
    )];

    fn check(&self) -> Result<(), Bad> {
        if self.plan().is_some() {
            return Ok(());
        }
        Err(Bad { field: "range", why: refusal("The usage range is 1h or 30d") })
    }
}

/// `metrics show SERIES... --since R --every E --json`: the two series of
/// a chart over a range.
pub struct Usage;

impl Action for Usage {
    const ID: &'static str = "storage.usage";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/storage/usage/{range}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the disks' use over time";
    type Args = Range;

    fn argv(args: &Range) -> Vec<String> {
        let (series, since, every) = args.plan().unwrap_or((&[], "-1h", "1m"));
        let mut argv = vec!["metrics".to_string(), "show".into()];
        argv.extend(series.iter().map(|name| (*name).to_string()));
        argv.extend(["--since", since, "--every", every, "--json"].map(String::from));
        argv
    }

    fn subject(args: &Range) -> Option<String> {
        Some(args.range.clone())
    }

    fn examples() -> Examples<Range> {
        Examples {
            good: vec![Range { range: "1h".into() }, Range { range: "30d".into() }],
            bad: vec![
                json!({ "range": "2h" }),
                json!({ "range": "" }),
                json!({ "range": "1H" }),
                json!({ "range": "24h" }),
                json!({ "range": "1h", "series": "cpu" }),
                json!({}),
            ],
            undo: None,
        }
    }
}

/// A disk, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// The disk's name, as `storage show` gives it.
    pub disk: String,
}

impl Args for Disk {
    const FIELDS: &'static [Field] = &[Field::text(
        "disk",
        "The disk's name, as storage show gives it: vda, sda or nvme0n1.",
        1,
        DISK_MAX,
        Some(DISK_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        if is_disk_name(&self.disk) {
            return Ok(());
        }
        let why =
            "a disk's name is 1 to 32 lower-case letters and digits, as storage show gives it";
        Err(Bad { field: "disk", why: refusal(why) })
    }
}

/// What the disks are tried with: a name `/kernel/block` gives, and each
/// kind of name that is not a disk's.
fn disk_examples() -> Examples<Disk> {
    Examples {
        good: vec![Disk { disk: "vdb".into() }],
        bad: vec![
            json!({ "disk": "" }),
            json!({ "disk": "../x" }),
            json!({ "disk": "VDB" }),
            json!({ "disk": "vdb/" }),
            json!({ "disk": "v-b" }),
            json!({ "disk": "x".repeat(DISK_MAX + 1) }),
            json!({ "disk": "vdb", "all": true }),
            json!({}),
        ],
        undo: None,
    }
}

/// `diskhealth --json DISK`: what the disk says of its health.
pub struct Health;

impl Action for Health {
    const ID: &'static str = "storage.health";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/storage/disks/{disk}/health";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["DSK-101"];
    /// diskhealth's status has the bits of smartctl's: 2 for a disk that
    /// could not be read, 4 for one that warns and 8 for one that is
    /// failing, in any mix. After each the output is the answer, what the
    /// disk said; 1, a mistake or a disk the machine does not have
    /// (DSK-101), has none.
    const ANSWERS: &'static [i32] = &[0, 2, 4, 6, 8, 10, 12, 14];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show a disk's health";
    type Args = Disk;

    fn argv(args: &Disk) -> Vec<String> {
        ["diskhealth", "--json", args.disk.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Disk) -> Option<String> {
        Some(args.disk.clone())
    }

    fn examples() -> Examples<Disk> {
        Examples {
            good: vec![Disk { disk: "vda".into() }],
            bad: vec![
                json!({ "disk": "../x" }),
                json!({ "disk": "VDA" }),
                json!({ "disk": "" }),
                json!({ "disk": "vda", "all": true }),
                json!({}),
            ],
            undo: None,
        }
    }
}

/// A mounted filesystem, by where it is mounted.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    /// A path in the filesystem, from `/`: where it is mounted.
    pub mount: String,
}

impl Args for Mount {
    const FIELDS: &'static [Field] = &[Field::text(
        "mount",
        "Where the filesystem is mounted, as a path from /: /volumes/data.",
        1,
        PATH_MAX,
        Some(PATH_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        let why = if !self.mount.starts_with('/') {
            "The mount is a path that starts with /"
        } else if self.mount.len() > PATH_MAX {
            "The mount's path is at most 4096 bytes long"
        } else if self.mount.contains('\0') {
            "The mount's path has no NUL in it"
        } else if self.mount.split('/').any(|part| part == "..") {
            "The mount's path has no .. in it"
        } else {
            return Ok(());
        };
        Err(Bad { field: "mount", why: format!("{why}. Nothing was done.") })
    }
}

/// `scrub start -B --progress MOUNT`: reads every block of a filesystem
/// and mends what a good copy can, as a task.
pub struct Scrub;

impl Action for Scrub {
    const ID: &'static str = "storage.scrub";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/storage/scrub";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["DSK-102"];
    /// A scrub that found errors no copy mends ends with status 3, and
    /// has said all it found: the kept result says it again.
    const ANSWERS: &'static [i32] = &[0, 3];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Check every block of a filesystem";
    type Args = Mount;

    fn argv(args: &Mount) -> Vec<String> {
        ["scrub", "start", "-B", "--progress", args.mount.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Mount) -> Option<String> {
        Some(args.mount.clone())
    }

    /// `/users`, which is on Dukovo's root on a disk: `/` is the kernel's
    /// empty root, no filesystem that a scrub reads (radix-files(7)).
    fn examples() -> Examples<Mount> {
        Examples {
            good: vec![Mount { mount: "/users".into() }],
            bad: vec![
                json!({ "mount": "relative" }),
                json!({ "mount": "" }),
                json!({ "mount": "~/data" }),
                json!({ "mount": "/volumes/../etc" }),
                json!({ "mount": "/volumes/x\u{0}y" }),
                json!({ "mount": "/".repeat(PATH_MAX + 1) }),
                json!({ "mount": "/users", "repair": true }),
                json!({}),
            ],
            undo: None,
        }
    }
}

/// `storage grow DISK`: gives the disk's last partition the room after it,
/// and the Btrfs filesystem on it the room of the partition.
pub struct Grow;

impl Action for Grow {
    const ID: &'static str = "storage.grow";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/storage/disks/{disk}/grow";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["DSK-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Grow a disk";
    type Args = Disk;

    fn argv(args: &Disk) -> Vec<String> {
        ["storage", "grow", args.disk.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Disk) -> Option<String> {
        Some(args.disk.clone())
    }

    /// `storage show --json`, which everyone may run: what the grow moves
    /// is in it.
    fn probe(_: &Disk) -> Option<Vec<String>> {
        Some(["storage", "show", "--json"].map(String::from).to_vec())
    }

    /// The sizes of the disk's partitions and of the volumes some of them
    /// hold, which a grow moves. A disk the output does not list says
    /// nothing.
    fn probed(args: &Disk, output: &str) -> Option<String> {
        let document: Value = serde_json::from_str(output).ok()?;
        let named =
            |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(String::from);
        let disk = document
            .get("disks")?
            .as_array()?
            .iter()
            .find(|disk| named(disk, "name").as_deref() == Some(args.disk.as_str()))?;
        let partitions = disk.get("partitions")?.as_array()?;
        let names: Vec<String> = partitions.iter().filter_map(|part| named(part, "name")).collect();
        let sizes: Vec<Value> =
            partitions.iter().map(|part| json!([part.get("name"), part.get("size")])).collect();
        let volumes: Vec<Value> = document
            .get("volumes")?
            .as_array()?
            .iter()
            .filter(|volume| {
                let devices = volume.get("devices").and_then(Value::as_array);
                devices.is_some_and(|devices| {
                    devices
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|device| names.iter().any(|n| n == device))
                })
            })
            .map(|volume| json!([volume.get("uuid"), volume.get("size")]))
            .collect();
        Some(json!({ "partitions": sizes, "volumes": volumes }).to_string())
    }

    /// The boot test grows the disk image before it asks.
    fn examples() -> Examples<Disk> {
        disk_examples()
    }
}

/// `logtrim --show --json`: the room the log takes, and its limits.
pub struct Log;

impl Action for Log {
    const ID: &'static str = "storage.log";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/storage/log";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the log's space";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["logtrim", "--show", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// The limits of the log to set: each optional, one at least.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// How many days the log is kept: 0 to 3650, 0 for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_days: Option<i64>,
    /// The most the log may take: a size, or 0 for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<String>,
    /// The most of the log's file system the log may take, in percent: 0
    /// to 100, 0 for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_percent: Option<i64>,
    /// How many days the audit log is kept: 0 to 3650, 0 for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_keep_days: Option<i64>,
}

impl Limits {
    /// The settings given, as the command's key and the value as written,
    /// in the order the command and the settings file have them.
    fn given(&self) -> Vec<(&'static str, &'static str, String)> {
        let mut given = Vec::new();
        let mut add = |field: &'static str, key: &'static str, value: Option<String>| {
            given.extend(value.map(|value| (field, key, value)));
        };
        add("keep_days", "keep-days", self.keep_days.map(|days| days.to_string()));
        add("max_size", "max-size", self.max_size.clone());
        add("max_percent", "max-percent", self.max_percent.map(|share| share.to_string()));
        add("audit_keep_days", "audit-keep-days", self.audit_keep_days.map(|d| d.to_string()));
        given
    }
}

impl Args for Limits {
    const FIELDS: &'static [Field] = &[
        Field::number(
            "keep_days",
            "How many days the log is kept: 0 to 3650, and 0 for no limit.",
            0,
            DAYS_MOST,
        )
        .optional(),
        Field::text(
            "max_size",
            "The most the log may take, as a size such as 500M or 4G, and 0 for no limit.",
            1,
            SIZE_MAX,
            Some(SIZE_PATTERN),
        )
        .optional(),
        Field::number(
            "max_percent",
            "The most of the log's file system the log may take, in percent: 0 to 100, and 0 for no limit.",
            0,
            100,
        )
        .optional(),
        Field::number(
            "audit_keep_days",
            "How many days the audit log is kept: 0 to 3650, and 0 for no limit.",
            0,
            DAYS_MOST,
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        let given = self.given();
        if given.is_empty() {
            return Err(Bad {
                field: "keep_days",
                why: "Nothing to change: say at least one of keep_days, max_size, max_percent \
                      and audit_keep_days."
                    .into(),
            });
        }
        for (field, key, value) in given {
            logtrim::checked(key, &value).map_err(|fail| {
                let first = fail.message.lines().next().unwrap_or_default();
                Bad { field, why: refusal(first) }
            })?;
        }
        Ok(())
    }
}

/// `logtrim set KEY VALUE...`: changes the log's limits in the settings
/// file, and has logd read them.
pub struct LogSet;

impl Action for LogSet {
    const ID: &'static str = "storage.log-set";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/storage/log/limits";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set the log's limits";
    type Args = Limits;

    fn argv(args: &Limits) -> Vec<String> {
        let mut argv = vec!["logtrim".to_string(), "set".into()];
        for (_, key, value) in args.given() {
            argv.extend([key.to_string(), value]);
        }
        argv
    }

    fn files(_: &Limits) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    /// `logtrim set` writes nothing when its edit leaves the text as it
    /// is, and says so; with no file, it writes one.
    fn keeps_file(args: &Limits, _: &str, text: Option<&str>) -> Option<bool> {
        let changes: Vec<(&str, String)> =
            args.given().into_iter().map(|(_, key, value)| (key, value)).collect();
        let text = text.unwrap_or_default();
        Some(logtrim::edited(text, &changes) == text)
    }

    /// The limits `logtrim --show --json` gives are root's to read, so a
    /// probe cannot say; check mode and the journal compare the file.
    fn examples() -> Examples<Limits> {
        Examples {
            good: vec![Limits { keep_days: Some(60), ..Limits::default() }],
            bad: vec![
                json!({}),
                json!({ "keep_days": 3651 }),
                json!({ "keep_days": -1 }),
                json!({ "max_percent": 101 }),
                json!({ "audit_keep_days": 3651 }),
                json!({ "max_size": "lots" }),
                json!({ "max_size": "" }),
                json!({ "max_size": "-1G" }),
                json!({ "keep_days": "60" }),
                json!({ "keep_days": 60, "keep-days": 61 }),
            ],
            undo: Some(Undo {
                action: "storage.log-set",
                args: json!({
                    "keep_days": 90, "max_size": "4G", "max_percent": 10, "audit_keep_days": 365
                }),
            }),
        }
    }
}

/// `logtrim`: trims the log now, to its limits.
pub struct LogTrim;

impl Action for LogTrim {
    const ID: &'static str = "storage.log-trim";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/storage/log/trim";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Trim the log";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        vec!["logtrim".to_string()]
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "days": 1 })], undo: None }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::{
        Caller, Found, REGISTRY, Registered, authorize, find, query_arguments, schema,
    };
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// Each action of the page: its id, its address, whether it changes
    /// anything, who runs it, and the command its first example makes.
    const ACTIONS: [(&str, &str, bool, Runs, &[&str]); 9] = [
        (
            "storage.show",
            "/api/1/storage",
            false,
            Runs::AsThePerson,
            &["storage", "show", "--json"],
        ),
        (
            "storage.raid",
            "/api/1/storage/raid",
            false,
            Runs::AsThePerson,
            &["storage", "raid", "--json"],
        ),
        (
            "storage.usage",
            "/api/1/storage/usage/{range}",
            false,
            Runs::AsThePerson,
            &[
                "metrics",
                "show",
                "disk.read",
                "disk.write",
                "--since",
                "-1h",
                "--every",
                "1m",
                "--json",
            ],
        ),
        (
            "storage.health",
            "/api/1/storage/disks/{disk}/health",
            false,
            Runs::AsAdministrator,
            &["diskhealth", "--json", "vda"],
        ),
        (
            "storage.scrub",
            "/api/1/storage/scrub",
            true,
            Runs::AsAdministrator,
            &["scrub", "start", "-B", "--progress", "/users"],
        ),
        (
            "storage.grow",
            "/api/1/storage/disks/{disk}/grow",
            true,
            Runs::AsAdministrator,
            &["storage", "grow", "vdb"],
        ),
        (
            "storage.log",
            "/api/1/storage/log",
            false,
            Runs::AsAdministrator,
            &["logtrim", "--show", "--json"],
        ),
        (
            "storage.log-set",
            "/api/1/storage/log/limits",
            true,
            Runs::AsAdministrator,
            &["logtrim", "set", "keep-days", "60"],
        ),
        ("storage.log-trim", "/api/1/storage/log/trim", true, Runs::AsAdministrator, &["logtrim"]),
    ];

    /// The arguments of the first example of `id`.
    fn first_example(id: &str) -> Value {
        action(id).example_values().0.remove(0)
    }

    #[test]
    fn each_action_runs_the_command_the_design_names_for_the_caller_it_names() {
        for (id, path, changes, runs, words) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            assert_eq!(prepared.argv, words, "{id}");
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (changes, runs, "storage", path),
                "{id}"
            );
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
            assert_eq!(prepared.ansible.module, "ansible.builtin.command", "{id}");
            assert_eq!(prepared.ansible.params["argv"], json!(words), "{id}");
            // A read changes nothing, and says so to Ansible.
            assert_eq!(prepared.ansible.changed_when.as_deref() == Some("false"), !changes, "{id}");
            assert_eq!(prepared.trial, None, "{id}");
            let tests = action.tests();
            assert_eq!(tests.checks, "admind::actions::storage::tests", "{id}");
            assert_eq!(tests.route, "admin-storage", "{id}");
            assert_eq!(tests.same_as_command, "admin-storage", "{id}");
            assert_eq!(tests.browser, "web-console/storage", "{id}");
            assert_eq!(tests.ansible, "web-console/ansible", "{id}");
            assert!(!action.lines(), "{id}");
        }
        let ours: Vec<_> = REGISTRY.iter().filter(|a| a.id().starts_with("storage.")).collect();
        assert_eq!(ours.len(), ACTIONS.len());
    }

    #[test]
    fn only_the_scrub_is_a_task_and_each_says_which_statuses_and_errors_are_its_own() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            assert_eq!(action.long(), id == "storage.scrub", "{id}");
            let (errors, answers): (&[&str], &[i32]) = match id {
                "storage.health" => (&["DSK-101"], &[0, 2, 4, 6, 8, 10, 12, 14]),
                "storage.scrub" => (&["DSK-102"], &[0, 3]),
                "storage.grow" => (&["DSK-101"], &[0]),
                _ => (&[], &[0]),
            };
            assert_eq!((action.errors(), action.answers()), (errors, answers), "{id}");
        }
        // The Ansible tasks that answer after another status say which.
        let health = action("storage.health").prepare(json!({ "disk": "vda" })).unwrap();
        assert_eq!(
            health.ansible.failed_when.as_deref(),
            Some("answer.rc not in [0, 2, 4, 6, 8, 10, 12, 14]")
        );
        let scrub = action("storage.scrub").prepare(json!({ "mount": "/users" })).unwrap();
        assert_eq!(scrub.ansible.failed_when.as_deref(), Some("answer.rc not in [0, 3]"));
    }

    #[test]
    fn a_grow_is_compared_by_its_probe_and_a_limit_by_its_file_and_nothing_else_is() {
        for (id, _, changes, ..) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            let expected = matches!(id, "storage.grow" | "storage.log-set");
            assert_eq!(action.compares(), expected, "{id}");
            assert_eq!(prepared.probe.is_some(), id == "storage.grow", "{id}");
            assert_eq!(
                prepared.files,
                if id == "storage.log-set" {
                    vec!["/system/settings/log".to_string()]
                } else {
                    vec![]
                },
                "{id}"
            );
            assert!(changes || !expected, "{id}: a read has nothing to compare");
        }
        let grow = action("storage.grow").prepare(json!({ "disk": "vdb" })).unwrap();
        assert_eq!(grow.probe, Some(vec!["storage".to_string(), "show".into(), "--json".into()]));
    }

    #[test]
    fn the_usage_is_the_disks_over_an_hour_or_the_space_over_thirty_days() {
        assert_eq!(
            argv("storage.usage", json!({ "range": "1h" })),
            [
                "metrics",
                "show",
                "disk.read",
                "disk.write",
                "--since",
                "-1h",
                "--every",
                "1m",
                "--json"
            ]
        );
        assert_eq!(
            argv("storage.usage", json!({ "range": "30d" })),
            ["metrics", "show", "fs.used", "fs.size", "--since", "-30d", "--every", "6h", "--json"]
        );
        for range in ["2h", "", "1H", "24h", "7d", "1y", "30D", " 1h", "1h "] {
            assert_eq!(
                refused("storage.usage", json!({ "range": range })),
                (
                    Kind::BadValue,
                    Some("range"),
                    "The usage range is 1h or 30d. Nothing was done.".into()
                ),
                "{range:?}"
            );
        }
        for args in [json!({}), json!({ "range": 1 }), json!({ "range": "1h", "series": "cpu" })] {
            let failure = action("storage.usage").prepare(args.clone()).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{args}");
        }
        let prepared = action("storage.usage").prepare(json!({ "range": "30d" })).unwrap();
        assert_eq!(prepared.ansible.name, "Show the disks' use over time: 30d");
    }

    #[test]
    fn a_disk_is_named_by_one_to_32_lower_case_letters_and_digits() {
        for id in ["storage.health", "storage.grow"] {
            for disk in ["vda", "sda", "nvme0n1", "mmcblk0", "a", "x".repeat(32).as_str()] {
                let words = argv(id, json!({ "disk": disk }));
                assert_eq!(words.last().map(String::as_str), Some(disk), "{id} {disk}");
            }
            let words = "A disk's name is 1 to 32 lower-case letters and digits, as storage show \
                         gives it. Nothing was done.";
            for disk in [
                "../x",
                "VDA",
                "",
                "vda/",
                "v-a",
                "vd a",
                "vda\n",
                "vdá",
                "vda\0",
                "x".repeat(33).as_str(),
            ] {
                assert_eq!(
                    refused(id, json!({ "disk": disk })),
                    (Kind::BadValue, Some("disk"), words.into()),
                    "{id} {disk:?}"
                );
            }
            for args in [
                json!({}),
                json!({ "disk": 1 }),
                json!({ "disk": null }),
                json!({ "disk": "vda", "all": true }),
            ] {
                assert_eq!(
                    action(id).prepare(args.clone()).unwrap_err().kind,
                    Kind::BadRequest,
                    "{id} {args}"
                );
            }
        }
        // A name that is a word of an option is still one word, after the
        // options.
        assert_eq!(
            argv("storage.health", json!({ "disk": "vda" })),
            ["diskhealth", "--json", "vda"]
        );
    }

    #[test]
    fn the_scrub_s_mount_is_an_absolute_path_of_at_most_4096_bytes_with_no_nul_and_no_dot_dot() {
        for mount in [
            "/",
            "/volumes/LAB",
            "/users",
            "/with space/x",
            "/a/..b/c..",
            "/a/.../b",
            "/caf\u{e9}",
            format!("/{}", "a".repeat(4095)).as_str(),
        ] {
            assert_eq!(
                argv("storage.scrub", json!({ "mount": mount })),
                ["scrub", "start", "-B", "--progress", mount],
                "{mount:?}"
            );
        }
        let end = "Nothing was done.";
        for (mount, why) in [
            ("relative", "The mount is a path that starts with /"),
            ("", "The mount is a path that starts with /"),
            ("~/data", "The mount is a path that starts with /"),
            (" /volumes", "The mount is a path that starts with /"),
            ("/volumes\u{0}/x", "The mount's path has no NUL in it"),
            ("/volumes/../etc", "The mount's path has no .. in it"),
            ("/..", "The mount's path has no .. in it"),
            ("/a/b/..", "The mount's path has no .. in it"),
        ] {
            assert_eq!(
                refused("storage.scrub", json!({ "mount": mount })),
                (Kind::BadValue, Some("mount"), format!("{why}. {end}")),
                "{mount:?}"
            );
        }
        let long = format!("/{}", "a".repeat(4096));
        assert_eq!(
            refused("storage.scrub", json!({ "mount": long })),
            (
                Kind::BadValue,
                Some("mount"),
                format!("The mount's path is at most 4096 bytes long. {end}")
            )
        );
        let wide = format!("/{}", "\u{e9}".repeat(2049));
        assert_eq!(refused("storage.scrub", json!({ "mount": wide })).1, Some("mount"));
        for args in [json!({}), json!({ "mount": 7 }), json!({ "mount": "/", "repair": true })] {
            assert_eq!(
                action("storage.scrub").prepare(args.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{args}"
            );
        }
    }

    #[test]
    fn the_limits_are_written_in_the_order_of_the_file_whatever_order_they_come_in() {
        assert_eq!(
            argv(
                "storage.log-set",
                json!({ "audit_keep_days": 365, "max_percent": 10, "max_size": "4G", "keep_days": 90 })
            ),
            [
                "logtrim",
                "set",
                "keep-days",
                "90",
                "max-size",
                "4G",
                "max-percent",
                "10",
                "audit-keep-days",
                "365"
            ]
        );
        for (args, words) in [
            (json!({ "keep_days": 0 }), &["keep-days", "0"][..]),
            (json!({ "max_size": "0" }), &["max-size", "0"]),
            (json!({ "max_size": "500MB" }), &["max-size", "500MB"]),
            (json!({ "max_size": "1g" }), &["max-size", "1g"]),
            (json!({ "max_percent": 100 }), &["max-percent", "100"]),
            (json!({ "audit_keep_days": 3650 }), &["audit-keep-days", "3650"]),
            (
                json!({ "max_percent": 0, "keep_days": 3650 }),
                &["keep-days", "3650", "max-percent", "0"],
            ),
        ] {
            let mut expected = vec!["logtrim".to_string(), "set".into()];
            expected.extend(words.iter().map(ToString::to_string));
            assert_eq!(argv("storage.log-set", args.clone()), expected, "{args}");
        }
        // What was asked is kept as it was, and what was left out is not
        // made up.
        let prepared = action("storage.log-set").prepare(json!({ "keep_days": 60 })).unwrap();
        assert_eq!(prepared.args, json!({ "keep_days": 60 }));
    }

    #[test]
    fn a_limit_out_of_bounds_is_refused_in_the_commands_own_words_and_nothing_to_change_too() {
        for (args, field, words) in [
            (
                json!({ "keep_days": 3651 }),
                "keep_days",
                "'3651' is not a number of days, 0 to 3650. Nothing was done.",
            ),
            (
                json!({ "keep_days": -1 }),
                "keep_days",
                "'-1' is not a number of days, 0 to 3650. Nothing was done.",
            ),
            (
                json!({ "audit_keep_days": 3651 }),
                "audit_keep_days",
                "'3651' is not a number of days, 0 to 3650. Nothing was done.",
            ),
            (
                json!({ "max_percent": 101 }),
                "max_percent",
                "'101' is not a share in percent, 0 to 100. Nothing was done.",
            ),
            (
                json!({ "max_percent": -5 }),
                "max_percent",
                "'-5' is not a share in percent, 0 to 100. Nothing was done.",
            ),
            (
                json!({ "max_size": "lots" }),
                "max_size",
                "'lots' is not a size, as 500M or 4G, or 0 for no limit. Nothing was done.",
            ),
            (
                json!({ "max_size": "-1G" }),
                "max_size",
                "'-1G' is not a size, as 500M or 4G, or 0 for no limit. Nothing was done.",
            ),
            (
                json!({ "max_size": "4 G" }),
                "max_size",
                "'4 G' is not a size, as 500M or 4G, or 0 for no limit. Nothing was done.",
            ),
            (
                json!({ "max_size": "" }),
                "max_size",
                "'' is not a size, as 500M or 4G, or 0 for no limit. Nothing was done.",
            ),
            (
                json!({}),
                "keep_days",
                "Nothing to change: say at least one of keep_days, max_size, max_percent and \
                 audit_keep_days.",
            ),
        ] {
            assert_eq!(
                refused("storage.log-set", args.clone()),
                (Kind::BadValue, Some(field), words.into()),
                "{args}"
            );
        }
        // The first that is wrong is the one said, in the file's order.
        let (_, field, _) =
            refused("storage.log-set", json!({ "max_percent": 101, "keep_days": 3651 }));
        assert_eq!(field, Some("keep_days"));
        for args in [
            json!({ "keep_days": "60" }),
            json!({ "keep_days": 1.5 }),
            json!({ "max_size": 4 }),
            json!({ "keep_days": null, "max_size": null }),
            json!({ "keep_days": 60, "keep-days": 61 }),
            json!({ "keep_days": 60, "force": true }),
        ] {
            let failure = action("storage.log-set").prepare(args.clone());
            // A `null` is a field left out, so only the nothing-to-change
            // refusal is its.
            let kind = failure.map_or_else(|f| f.kind, |_| Kind::Internal);
            assert!(matches!(kind, Kind::BadRequest | Kind::BadValue), "{args}: {kind:?}");
        }
    }

    #[test]
    fn what_the_command_checks_the_action_checks_with_the_same_function() {
        // The arguments the terminal refuses are refused here, and the
        // ones it takes are taken.
        for (key, value) in [
            ("keep-days", "10"),
            ("keep-days", "3651"),
            ("max-percent", "100"),
            ("max-percent", "101"),
            ("max-size", "4G"),
            ("max-size", "lots"),
            ("audit-keep-days", "0"),
        ] {
            let field = match key {
                "keep-days" => "keep_days",
                "max-percent" => "max_percent",
                "max-size" => "max_size",
                _ => "audit_keep_days",
            };
            let number = value.parse::<i64>().ok();
            let given = if key == "max-size" { json!(value) } else { json!(number.unwrap()) };
            let ours = action("storage.log-set").prepare(json!({ field: given })).is_ok();
            assert_eq!(ours, logtrim::checked(key, value).is_ok(), "{key} {value}");
        }
    }

    #[test]
    fn the_grow_s_probe_picks_the_disks_partitions_and_the_volumes_they_hold() {
        let output = |a: u64, b: u64, volume: u64, other: u64| {
            json!({
                "format": 1,
                "disks": [
                    {"name": "vda", "partitions": [{"name": "vda1", "size": other}]},
                    {"name": "vdb", "partitions": [
                        {"name": "vdb1", "size": 1},
                        {"name": "vdb2", "size": a},
                    ]},
                    {"name": "vdc", "partitions": [{"name": "vdc1", "size": b}]},
                ],
                "volumes": [
                    {"uuid": "u-1", "devices": ["vdb2", "vdc1"], "size": volume},
                    {"uuid": "u-2", "devices": ["vda1"], "size": other},
                ],
            })
            .to_string()
        };
        let grow = action("storage.grow");
        let args = json!({ "disk": "vdb" });
        let picked = |output: &str| grow.probed(&args, output);
        let before = picked(&output(100, 200, 300, 400));
        assert!(before.is_some());
        // The same machine gives the same pick, with the spaces of the
        // output whatever they are.
        assert_eq!(before, picked(&output(100, 200, 300, 400)));
        // The partition grown moved, and so did the volume.
        assert_ne!(before, picked(&output(150, 200, 300, 400)));
        assert_ne!(before, picked(&output(100, 200, 350, 400)));
        // What another disk's partition does, and another volume, is not
        // what this grow moved.
        assert_eq!(before, picked(&output(100, 999, 300, 400)));
        assert_eq!(before, picked(&output(100, 200, 300, 999)));
        // A disk the machine does not list, or no JSON, says nothing.
        assert_eq!(grow.probed(&json!({ "disk": "vdz" }), &output(1, 2, 3, 4)), None);
        for text in ["", "not json", "{}", "[]", "{\"disks\": 1}"] {
            assert_eq!(picked(text), None, "{text:?}");
        }
        assert_eq!(grow.probed(&json!({ "disk": 7 }), &output(1, 2, 3, 4)), None);
    }

    #[test]
    fn each_address_finds_its_action_with_its_arguments_and_the_other_method_is_wrong() {
        for (path, id, name, value) in [
            ("/api/1/storage", "storage.show", None, None),
            ("/api/1/storage/usage/1h", "storage.usage", Some("range"), Some("1h")),
            ("/api/1/storage/usage/30d", "storage.usage", Some("range"), Some("30d")),
            ("/api/1/storage/disks/vda/health", "storage.health", Some("disk"), Some("vda")),
            ("/api/1/storage/log", "storage.log", None, None),
        ] {
            let found = find(REGISTRY, false, path);
            let Found::Action(action, params) = found else { panic!("{path}: {found:?}") };
            assert_eq!(action.id(), id, "{path}");
            assert_eq!(params.get(name.unwrap_or("")).and_then(Value::as_str), value, "{path}");
            assert!(matches!(find(REGISTRY, true, path), Found::WrongMethod), "{path}");
        }
        for (path, id, name, value) in [
            ("/api/1/storage/scrub", "storage.scrub", None, None),
            ("/api/1/storage/disks/vdb/grow", "storage.grow", Some("disk"), Some("vdb")),
            ("/api/1/storage/log/limits", "storage.log-set", None, None),
            ("/api/1/storage/log/trim", "storage.log-trim", None, None),
        ] {
            let found = find(REGISTRY, true, path);
            let Found::Action(action, params) = found else { panic!("{path}: {found:?}") };
            assert_eq!(action.id(), id, "{path}");
            assert_eq!(params.get(name.unwrap_or("")).and_then(Value::as_str), value, "{path}");
            assert!(matches!(find(REGISTRY, false, path), Found::WrongMethod), "{path}");
        }
        // A name with a slash in it is no part of an address, even when
        // escaped, and there is no address beyond these.
        for path in [
            "/api/1/storage/disks/..%2Fx/health",
            "/api/1/storage/disks/vda/",
            "/api/1/storage/disks/vda/health/x",
            "/api/1/storage/disks//health",
            "/api/1/storage/usage",
            "/api/1/storage/scrub/x",
            "/api/1/storage/logs",
        ] {
            assert!(matches!(find(REGISTRY, false, path), Found::None), "{path}");
            assert!(matches!(find(REGISTRY, true, path), Found::None), "{path}");
        }
        // The disk of an address is the text it has, which `check` holds to
        // the rule.
        let Found::Action(health, params) =
            find(REGISTRY, false, "/api/1/storage/disks/VDA/health")
        else {
            panic!("an address with a name")
        };
        let given = query_arguments(params, "", health.fields()).unwrap();
        assert_eq!(given, json!({ "disk": "VDA" }));
        assert_eq!(health.prepare(given).unwrap_err().field, Some("disk"));
    }

    #[test]
    fn a_session_reads_the_disks_and_an_administrator_the_rest() {
        for (id, person, unlocked_admin, locked_admin) in [
            ("storage.show", true, true, true),
            ("storage.usage", true, true, true),
            ("storage.health", false, true, false),
            ("storage.scrub", false, true, false),
            ("storage.grow", false, true, false),
            ("storage.log", false, true, false),
            ("storage.log-set", false, true, false),
            ("storage.log-trim", false, true, false),
        ] {
            let action = action(id);
            let session = |admin, unlocked| {
                authorize(&Caller::Session { name: "ann", admin, unlocked }, action)
            };
            assert_eq!(session(false, false).is_ok(), person, "{id}");
            assert_eq!(session(false, true).is_ok(), person, "{id}");
            assert_eq!(session(true, true).is_ok(), unlocked_admin, "{id}");
            assert_eq!(session(true, false).is_ok(), locked_admin, "{id}");
        }
        let refused = |id: &str, admin, unlocked| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id)).unwrap_err()
        };
        assert_eq!(refused("storage.health", false, true).kind, Kind::NotAdmin);
        assert_eq!(refused("storage.grow", false, true).kind, Kind::NotAdmin);
        assert_eq!(refused("storage.health", true, false).kind, Kind::Locked);
        assert_eq!(refused("storage.log-set", true, false).kind, Kind::Locked);
    }

    #[test]
    fn a_token_needs_the_storage_scope_and_for_what_runs_as_an_administrator_the_whole_of_it() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["storage.show", "storage.usage"] {
            for list in ["storage", "storage:read", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["services", "reliability", "console:read"] {
                assert_eq!(call(list, id).unwrap_err().kind, Kind::OutOfScope, "{id} {list}");
            }
        }
        for id in [
            "storage.health",
            "storage.scrub",
            "storage.grow",
            "storage.log",
            "storage.log-set",
            "storage.log-trim",
        ] {
            for list in ["storage", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            let refused = call("storage:read", id).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{id}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope storage or *",
                "{id}"
            );
            assert_eq!(call("services", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
    }

    #[test]
    fn the_schemas_say_the_ranges_the_names_and_which_limits_are_given() {
        let field = |id: &str, name: &str| schema(action(id).fields())["properties"][name].clone();
        assert_eq!(field("storage.usage", "range")["enum"], json!(["1h", "30d"]));
        assert_eq!(schema(action("storage.usage").fields())["required"], json!(["range"]));
        for id in ["storage.health", "storage.grow"] {
            let disk = field(id, "disk");
            assert_eq!((&disk["minLength"], &disk["maxLength"]), (&json!(1), &json!(32)), "{id}");
            assert_eq!(disk["pattern"], "^[a-z0-9]+$", "{id}");
            assert_eq!(schema(action(id).fields())["required"], json!(["disk"]), "{id}");
        }
        let mount = field("storage.scrub", "mount");
        assert_eq!((&mount["minLength"], &mount["maxLength"]), (&json!(1), &json!(4096)));
        assert_eq!(mount["pattern"], PATH_PATTERN);
        let set = schema(action("storage.log-set").fields());
        assert_eq!(set["required"], json!([]));
        assert_eq!(set["properties"]["keep_days"]["maximum"], 3650);
        assert_eq!(set["properties"]["keep_days"]["minimum"], 0);
        assert_eq!(set["properties"]["audit_keep_days"]["maximum"], 3650);
        assert_eq!(set["properties"]["max_percent"]["maximum"], 100);
        assert_eq!(set["properties"]["max_size"]["pattern"], SIZE_PATTERN);
        assert_eq!(set["additionalProperties"], false);
        for id in ["storage.show", "storage.log", "storage.log-trim"] {
            assert_eq!(schema(action(id).fields())["properties"], json!({}), "{id}");
        }
    }

    #[test]
    fn the_schema_s_size_pattern_lets_through_each_size_the_command_takes_and_no_other_shape() {
        let words = |text: &str| {
            let rest = text.trim_start_matches(|c: char| c.is_ascii_digit());
            let digits = text.len() > rest.len();
            let unit = match rest.len() {
                0 => true,
                1 => "KkMmGgTtPp".contains(rest),
                2 => "KkMmGgTtPp".contains(&rest[..1]) && "Bb".contains(&rest[1..]),
                3 => {
                    "KkMmGgTtPp".contains(&rest[..1])
                        && ["iB", "ib", "IB", "Ib"].contains(&&rest[1..])
                }
                _ => false,
            };
            digits && unit
        };
        for size in ["0", "500M", "4G", "1g", "2T", "10KB", "3MiB", "7pib", "123"] {
            assert!(words(size) && crate::util::parse_size(size).is_some(), "{size}");
        }
        for size in ["", "lots", "-1", "4 G", "G", "4GBB", "0x10"] {
            assert!(!words(size) || crate::util::parse_size(size).is_none(), "{size:?}");
        }
    }

    #[test]
    fn the_examples_work_and_the_bad_ones_do_not() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty(), "{id}");
            assert_eq!(undo.is_some(), id == "storage.log-set", "{id}: only a limit is put back");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        assert_eq!(
            action("storage.usage").example_values().0,
            [json!({ "range": "1h" }), json!({ "range": "30d" })]
        );
        assert_eq!(first_example("storage.health"), json!({ "disk": "vda" }));
        assert_eq!(first_example("storage.scrub"), json!({ "mount": "/users" }));
        assert_eq!(first_example("storage.grow"), json!({ "disk": "vdb" }));
        assert_eq!(first_example("storage.log-set"), json!({ "keep_days": 60 }));
        let (_, bad, _) = action("storage.health").example_values();
        assert!(
            bad.contains(&json!({ "disk": "../x" })) && bad.contains(&json!({ "disk": "VDA" }))
        );
        let (_, bad, _) = action("storage.scrub").example_values();
        assert!(bad.contains(&json!({ "mount": "relative" })));
        let (_, bad, _) = action("storage.log-set").example_values();
        for args in [json!({}), json!({ "max_percent": 101 }), json!({ "max_size": "lots" })] {
            assert!(bad.contains(&args), "{args}");
        }
        let (_, bad, _) = action("storage.log-trim").example_values();
        assert_eq!(bad, [json!({ "days": 1 })]);
    }

    #[test]
    fn what_puts_back_a_limit_is_the_default_the_machine_starts_with() {
        let (_, _, undo) = action("storage.log-set").example_values();
        let undo = undo.unwrap();
        assert_eq!(undo.action, "storage.log-set");
        assert_eq!(
            undo.args,
            json!({ "keep_days": 90, "max_size": "4G", "max_percent": 10, "audit_keep_days": 365 })
        );
        assert_eq!(
            argv(undo.action, undo.args),
            [
                "logtrim",
                "set",
                "keep-days",
                "90",
                "max-size",
                "4G",
                "max-percent",
                "10",
                "audit-keep-days",
                "365"
            ]
        );
    }

    #[test]
    fn a_check_of_the_limits_reads_the_settings_file_as_logtrim_set_edits_it() {
        let set = action("storage.log-set");
        let defaults =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../defaults/log"))
                .unwrap();
        // The limits a machine starts with are what the undo puts back,
        // and not the example's.
        let undo = set.example_values().2.unwrap().args;
        let sixty = first_example("storage.log-set");
        assert_eq!(set.keeps_file(&undo, SETTINGS, Some(&defaults)), Some(true));
        assert_eq!(set.keeps_file(&sixty, SETTINGS, Some(&defaults)), Some(false));
        assert_eq!(set.keeps_file(&sixty, SETTINGS, Some("keep-days=60\n")), Some(true));
        // The last line of a setting is the one that counts.
        let twice = Some("keep-days = 60\nkeep-days = 7\n");
        assert_eq!(set.keeps_file(&sixty, SETTINGS, twice), Some(false));
        let size = json!({ "max_size": "4G" });
        assert_eq!(set.keeps_file(&size, SETTINGS, Some("max-size = 4G")), Some(true));
        // With no file, `logtrim set` writes one.
        assert_eq!(set.keeps_file(&sixty, SETTINGS, None), Some(false));
    }
}
