// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob security`: the project's security advisories, DKSA (ADR 0534). They
//! are OSV 1.6 files in `security/advisories`, which `ob release` signs
//! for machines and `ob docs site` turns into pages and a feed.
//!
//! `ob security check` has system-release, built for this machine, read
//! every advisory the way the release keys' signing does; CI runs it.
//! `ob security advisory new` starts the next one from a template, whose
//! text still says TODO, which the check refuses until it is written.

use std::io::Write as _;
use std::path::PathBuf;

use crate::process::step;
use crate::{Context, Ctx, Result, bail, release};

/// Where the advisories are in the source tree.
pub const DIR: &str = "security/advisories";

/// `ob security ...`.
pub fn command(ctx: &Ctx, args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["check"] => check(ctx),
        ["advisory", "new"] => new_advisory(ctx),
        _ => bail!("usage: ./ob security check | advisory new"),
    }
}

/// `ob security check`: every advisory in [`DIR`] must parse as OSV 1.6
/// with Dukovo's fields, have the id its file is named for and a number
/// of its own, name versions of Dukovo's order, and say no TODO.
fn check(ctx: &Ctx) -> Result<()> {
    let args = ["advisories".into(), "--check".into(), ctx.root.join(DIR).into_os_string()];
    release::system_release::<std::ffi::OsString>(ctx, &args)
}

/// `ob security advisory new`: writes the next free advisory in [`DIR`]
/// from the template, and says which file to edit.
fn new_advisory(ctx: &Ctx) -> Result<()> {
    let dir = ctx.root.join(DIR);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .context(|| format!("reading {}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let id = next_id(&names, year_of(now));
    let path: PathBuf = dir.join(format!("{id}.json"));
    step(&format!("Writing {}", ctx.display(&path)));
    let mut file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .context(|| format!("creating {}", path.display()))?;
    file.write_all(template(&id, now).as_bytes())
        .context(|| format!("writing {}", path.display()))?;
    println!(
        "Edit {} (the runbook, docs/runbooks/security-advisory.md, says how), then run \
         ./ob security check.",
        ctx.display(&path)
    );
    Ok(())
}

/// The year of `seconds` since 1970, in UTC.
fn year_of(seconds: u64) -> u64 {
    crate::image::civil(seconds / 86_400).0
}

/// The next free advisory id for `year`, given the names of the files in
/// [`DIR`]: one more than the highest number of that year's, or 0001.
fn next_id(names: &[String], year: u64) -> String {
    let prefix = format!("DKSA-{year:04}-");
    let highest = names
        .iter()
        .filter_map(|name| name.strip_prefix(&prefix)?.strip_suffix(".json")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{prefix}{:04}", highest + 1)
}

/// An RFC 3339 time in UTC, as an advisory writes it.
fn rfc3339(seconds: u64) -> String {
    let (year, month, day) = crate::image::civil(seconds / 86_400);
    let secs = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// The file a new advisory starts from, written `seconds` since 1970: the
/// system, from the first release, medium. What a person must still write
/// says TODO, which `ob security check` refuses.
fn template(id: &str, seconds: u64) -> String {
    let now = rfc3339(seconds);
    format!(
        r#"{{
  "schema_version": "1.6.0",
  "id": "{id}",
  "modified": "{now}",
  "published": "{now}",
  "summary": "TODO: what is wrong, in one line",
  "details": "TODO: what the flaw is, which releases have it, who can use it and what it does, in plain sentences.",
  "affected": [
    {{
      "package": {{ "ecosystem": "Dukovo", "name": "dukovo" }},
      "ranges": [
        {{
          "type": "ECOSYSTEM",
          "events": [{{ "introduced": "0" }}, {{ "fixed": "TODO: the release with the fix" }}]
        }}
      ]
    }}
  ],
  "database_specific": {{
    "severity": "medium",
    "action": "TODO: what to do, in a sentence"
  }}
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_next_id_is_the_highest_number_of_the_year_and_one() {
        assert_eq!(next_id(&names(&["README.md"]), 2026), "DKSA-2026-0001");
        assert_eq!(
            next_id(&names(&["README.md", "DKSA-2026-0001.json", "DKSA-2026-0007.json"]), 2026),
            "DKSA-2026-0008"
        );
        // Another year's numbers do not count, and a new year starts again.
        assert_eq!(next_id(&names(&["DKSA-2026-0007.json"]), 2027), "DKSA-2027-0001");
        assert_eq!(
            next_id(&names(&["DKSA-2026-0007.json", "DKSA-2027-0002.json"]), 2026),
            "DKSA-2026-0008"
        );
        // A file that is no advisory's is not counted.
        assert_eq!(
            next_id(&names(&["DKSA-2026-0009.json.bak", "DKSA-2026-x.json"]), 2026),
            "DKSA-2026-0001"
        );
    }

    #[test]
    fn times_are_rfc_3339_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_794_000_000), "2026-11-06T21:20:00Z");
        assert_eq!(year_of(1_794_000_000), 2026);
        // The last second of a year, and the first of the next.
        assert_eq!(year_of(1_798_761_599), 2026);
        assert_eq!(year_of(1_798_761_600), 2027);
    }

    #[test]
    fn the_template_is_json_with_the_id_the_date_the_medium_severity_and_things_to_write() {
        let text = template("DKSA-2026-0003", 1_794_000_000);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], "1.6.0");
        assert_eq!(value["id"], "DKSA-2026-0003");
        assert_eq!(value["modified"], "2026-11-06T21:20:00Z");
        assert_eq!(value["published"], "2026-11-06T21:20:00Z");
        assert_eq!(value["database_specific"]["severity"], "medium");
        assert_eq!(value["affected"][0]["package"]["name"], "dukovo");
        assert!(value["summary"].as_str().unwrap().starts_with("TODO"));
        assert!(value["details"].as_str().unwrap().starts_with("TODO"));
        assert!(text.ends_with("}\n"));
    }
}
