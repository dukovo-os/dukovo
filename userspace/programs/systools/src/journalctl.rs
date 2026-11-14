// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! journalctl(1): `log` with journalctl's options and defaults (ADR 0478),
//! for the habits and scripts that come from Linux. The reading is in
//! `log`, which asks logd(8) for the records; `-u`, `-f`, `-n`, `-b`
//! (`-b -1` is the boot before, which logd keeps), `--since`, `--until`,
//! `-p`, `-k`, `-o`, `-r`, `--no-pager` and `-g` are its.
//!
//! This file answers the options that journald has and logd does not:
//! each says what to run instead and ends with status 1, where `log`
//! would only say the option is unknown. The boot test `admin-commands`
//! runs it against the running logd.

use crate::args::Args;
use crate::log;
use crate::util::{Fail, usage};

/// The options journald has and logd does not, with what to run instead.
const NOT_KEPT: &[(&[&str], &str)] = &[
    (
        &["--user", "--user-unit", "-M", "--machine"],
        "Dukovo has one log; a person sees the records of their own programs with `journalctl`, \
         and an administrator all of them with `elevate journalctl`",
    ),
    (
        &["-D", "--directory", "--file", "--root", "--image", "--namespace"],
        "the log is logd's files in /system/state/log; `journalctl` reads them through logd",
    ),
    (
        &["--rotate", "--flush", "--sync", "--relinquish-var", "--smart-relinquish-var"],
        "logd starts a file each day and writes as it goes; `elevate logtrim` trims the log",
    ),
    (
        &["--verify", "--setup-keys", "--verify-key", "--interval", "--force"],
        "logd keeps plain day files and does not seal them",
    ),
    (
        &["--header", "--update-catalog", "--list-catalog", "--dump-catalog", "--new-id128"],
        "there are no catalogs or journal files to describe",
    ),
    (
        &["-F", "--field", "-N", "--fields", "--list-fields", "--output-fields"],
        "`journalctl -o json` writes every field of a record, for jq or a script to pick from",
    ),
    (
        &["-c", "--cursor", "--after-cursor", "--cursor-file", "--show-cursor"],
        "logd has no cursors; `journalctl --since TIME` starts at a time",
    ),
];

/// journalctl(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let items = args.rest();
    for item in &items {
        if let Some(advice) = advice(&item.to_string_lossy()) {
            return Err(usage(advice));
        }
    }
    log::journalctl(&mut Args::new("journalctl", items))
}

/// What to say for `item` when it is one of the options in [`NOT_KEPT`],
/// as `--user`, `--directory=/x` or `-D/x`.
fn advice(item: &str) -> Option<String> {
    let option = match item.strip_prefix("--") {
        Some(long) => format!("--{}", long.split('=').next().unwrap_or(long)),
        None if item.len() > 1 && item.starts_with('-') => item[..2].to_string(),
        None => return None,
    };
    // `-b`, `-n` and the other short options may be in a cluster (`-fu
    // nginx`), where an option of this list is not at the front; the
    // front is all this looks at, since a wrong guess would refuse a
    // command that `log` takes.
    NOT_KEPT
        .iter()
        .find(|(names, _)| names.contains(&option.as_str()))
        .map(|(_, what)| format!("{option}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nanoseconds of 2026-10-30 21:25:01 UTC.
    const NOW: u64 = 1_793_395_501_000_000_000;

    #[test]
    fn an_option_logd_lacks_says_what_to_run_instead() {
        let said = advice("--rotate").unwrap();
        assert!(said.starts_with("--rotate: ") && said.contains("elevate logtrim"), "{said}");
        assert!(advice("--directory=/var/log/journal").unwrap().contains("/system/state/log"));
        assert!(advice("-D/x").unwrap().contains("logd's files"));
        assert!(advice("--user").unwrap().contains("elevate journalctl"));
        assert!(advice("-F").unwrap().contains("-o json"));
        assert!(advice("--after-cursor=s=1").unwrap().contains("--since"));
    }

    #[test]
    fn the_options_log_takes_are_left_to_it() {
        for item in [
            "-u",
            "nginx",
            "-f",
            "-n",
            "50",
            "-b",
            "-1",
            "--since",
            "-1h",
            "--until",
            "today",
            "-p",
            "err..warning",
            "-k",
            "-o",
            "json",
            "-r",
            "--no-pager",
            "-g",
            "refused",
            "--list-boots",
            "--disk-usage",
            "-fu",
        ] {
            assert_eq!(advice(item), None, "{item}");
        }
    }

    /// The times journalctl's `--since` and `--until` take, as `log` reads
    /// them.
    #[test]
    fn journalctls_times_are_read() {
        let at = |text: &str| log::parse_time(text, NOW, true).unwrap();
        assert_eq!(at("2026-10-30"), 1_793_318_400_000_000_000);
        assert_eq!(at("2026-10-30 21:00"), 1_793_394_000_000_000_000);
        assert_eq!(at("2026-10-30 21:00:30"), 1_793_394_030_000_000_000);
        assert_eq!(at("today"), 1_793_318_400_000_000_000);
        assert_eq!(at("yesterday"), 1_793_318_400_000_000_000 - 86_400_000_000_000);
        assert_eq!(at("-1h"), NOW - 3_600_000_000_000);
        assert_eq!(at("10 min ago"), NOW - 600_000_000_000);
        assert_eq!(at("2 hours ago"), NOW - 7_200_000_000_000);
        assert!(log::parse_time("next week", NOW, true).is_err());
    }
}
