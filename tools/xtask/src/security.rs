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
//!
//! `ob security scan` is the daily scan of what the system is built from,
//! which CI runs and turns into issues (`.github/scripts/sbom-scan.js`).
//! It asks two sources:
//!
//! - cargo-audit reads every Cargo.lock of the repository, and the
//!   Cargo.lock of each Rust bundle's port (`upstream::bundle_locks`),
//!   against the RustSec advisory database.
//! - OSV is asked about each port whose recipe pins a git commit, by that
//!   commit, which covers projects with no package ecosystem through the
//!   git ranges of OSV's records. A port pinned by a release archive has
//!   no commit to ask by; it is listed as not covered.
//!
//! A finding has the severity its advisory gives, which is the severity
//! its database names or the base score of its CVSS 3 vector, and none
//! when the advisory gives neither: the scan never rates a flaw itself. The scan
//! ends well whatever it finds, and fails only when it could not run to
//! its end, since a scan that missed a source would close issues for
//! findings it did not look for. When it misbehaves, run it by hand with
//! `--verbose`: each cargo-audit and curl command is printed.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::{Value, json};

use crate::ports::{self, Recipe};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result, bail, release, upstream};

/// Where the advisories are in the source tree.
pub const DIR: &str = "security/advisories";

/// `ob security ...`.
pub fn command(ctx: &Ctx, args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["check"] => check(ctx),
        ["advisory", "new"] => new_advisory(ctx),
        ["scan", options @ ..] => scan(ctx, options),
        _ => bail!("usage: ./ob security check | advisory new | scan [--json] [--osv-url <url>]"),
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

/// How severe an advisory says a flaw is, as ADR 0458's table has it.
/// The order is the order of severity, the most severe first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    /// The severity an advisory database names: GitHub's CRITICAL, HIGH,
    /// MODERATE and LOW, in any case, and MEDIUM.
    fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "critical" => Some(Self::Critical),
            "high" => Some(Self::High),
            "moderate" | "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    /// The severity of a CVSS base score: Critical from 9.0, High from 7.0,
    /// Medium from 4.0 and Low below.
    fn from_score(score: f64) -> Self {
        if score >= 9.0 {
            Self::Critical
        } else if score >= 7.0 {
            Self::High
        } else if score >= 4.0 {
            Self::Medium
        } else {
            Self::Low
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// Which source a finding came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Source {
    #[serde(rename = "cargo-audit")]
    CargoAudit,
    #[serde(rename = "osv")]
    Osv,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::CargoAudit => "cargo-audit",
            Self::Osv => "osv",
        }
    }
}

/// One advisory that matches a part of the system, as `--json` has it.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Finding {
    /// The advisory's own ID: RUSTSEC-2026-0001, CVE-2026-1234, GHSA-....
    id: String,
    aliases: Vec<String>,
    /// None when the advisory gives no severity.
    severity: Option<Severity>,
    summary: String,
    /// A crate and its version, or a port and its commit.
    component: String,
    source: Source,
    /// The advisory, where a person reads more of it.
    url: String,
    /// The Cargo.lock, or the recipe, that holds the component.
    location: String,
}

/// A port that the scan cannot ask OSV about, since its recipe pins a
/// release archive and no git commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct NotCovered {
    port: String,
    version: String,
    url: String,
}

/// What the scan found, and what it could not look at.
#[derive(Debug, Serialize)]
struct Report {
    findings: Vec<Finding>,
    not_covered: Vec<NotCovered>,
}

/// A port whose recipe pins the git commit to ask OSV by.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PinnedPort {
    name: String,
    version: String,
    commit: String,
}

/// The options of `ob security scan`.
#[derive(Debug, PartialEq, Eq)]
struct ScanOptions {
    json: bool,
    osv_url: String,
}

/// OSV's address; its API takes `POST /v1/querybatch` and `GET /v1/vulns/<id>`.
const OSV_URL: &str = "https://api.osv.dev";

/// How many pages of one commit's answers the scan reads: OSV pages an
/// answer that is very long, and a port has far fewer advisories.
const OSV_PAGES: usize = 20;

fn scan_options(args: &[&str]) -> Result<ScanOptions> {
    let mut options = ScanOptions { json: false, osv_url: OSV_URL.to_string() };
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        if arg == "--json" {
            options.json = true;
        } else if arg == "--osv-url" {
            let Some(&url) = args.next() else { bail!("--osv-url needs the server's address") };
            options.osv_url = url.to_string();
        } else if let Some(url) = arg.strip_prefix("--osv-url=") {
            options.osv_url = url.to_string();
        } else {
            bail!("usage: ./ob security scan [--json] [--osv-url <url>]");
        }
    }
    options.osv_url = options.osv_url.trim_end_matches('/').to_string();
    Ok(options)
}

/// `ob security scan [--json] [--osv-url <url>]`: see the module's text.
fn scan(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let options = scan_options(args)?;
    upstream::require_cargo_audit()?;
    let mut findings = Vec::new();
    let mut fetched = false;
    for (lock, location) in system_locks(ctx)? {
        findings.extend(audit_lock(ctx, &lock, &location, !fetched)?);
        fetched = true;
    }
    for found in upstream::bundle_locks(ctx)? {
        let location = format!("{} {} (its Cargo.lock)", found.port, found.version);
        findings.extend(audit_lock(ctx, &found.lock, &location, !fetched)?);
        fetched = true;
    }

    let recipes: Vec<Recipe> = ports::recipes(ctx)?.into_iter().map(|(recipe, _)| recipe).collect();
    let (pinned, not_covered) = split_ports(&recipes);
    step(&format!("Asking OSV about {} ports by their commits", pinned.len()));
    let osv_url = options.osv_url;
    let ask = |path: &str, body: Option<&str>| curl(ctx, &format!("{osv_url}{path}"), body);
    findings.extend(osv_findings(&ask, &pinned)?);

    let report = Report { findings: sorted(findings), not_covered };
    if options.json {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|err| Error::new(format!("writing the scan's findings: {err}")))?;
        println!("{text}");
    } else {
        print!("{}", render_text(&report));
    }
    Ok(())
}

/// The Cargo.lock files the repository tracks, each as a path to read it
/// by and the name to show it by. The list comes from git, so that a lock
/// a build left behind, such as one under `kernel/patched`, is not in it.
fn system_locks(ctx: &Ctx) -> Result<Vec<(PathBuf, String)>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&ctx.root).args(["ls-files", "-z"]);
    let output = process::output(ctx, &mut cmd)?;
    Ok(lock_files(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .map(|name| (ctx.root.join(&name), name))
        .collect())
}

/// The Cargo.lock files among the NUL-separated paths `git ls-files -z`
/// prints, in order.
fn lock_files(listing: &str) -> Vec<String> {
    let mut found: Vec<String> = listing
        .split('\0')
        .filter(|path| Path::new(path).file_name().is_some_and(|name| name == "Cargo.lock"))
        .map(str::to_string)
        .collect();
    found.sort();
    found
}

/// `cargo audit --json` on one lock file, which only reads it. The first
/// call fetches the advisory database; the others use what it fetched.
fn audit_lock(ctx: &Ctx, lock: &Path, location: &str, fetch: bool) -> Result<Vec<Finding>> {
    step(&format!("Auditing {location}"));
    let mut cmd = Command::new("cargo");
    cmd.args(["audit", "--json"]);
    if !fetch {
        cmd.arg("--no-fetch");
    }
    cmd.arg("--file").arg(lock).stderr(Stdio::inherit());
    if ctx.verbose {
        eprintln!("+ {}", process::render(&cmd));
    }
    let output = cmd.output().map_err(|err| process::spawn_error("cargo", &err))?;
    // Status 1 is the audit's way to say it found something, with its JSON
    // on standard output as always.
    parse_cargo_audit(&String::from_utf8_lossy(&output.stdout), location).map_err(|err| {
        Error::new(format!("{err} (`cargo audit` ended with {})", output.status))
            .hint("cargo-audit's own messages are above")
    })
}

/// The vulnerabilities in the JSON that `cargo audit --json` prints for
/// the lock file called `location`. Its warnings, about crates that are
/// unmaintained, unsound or yanked, are not findings: they are no
/// vulnerability, and a port often cannot leave such a crate.
fn parse_cargo_audit(json: &str, location: &str) -> Result<Vec<Finding>> {
    let document: Value = serde_json::from_str(json).map_err(|err| {
        Error::new(format!("cargo-audit's output for {location} is not JSON: {err}"))
    })?;
    let Some(list) = document["vulnerabilities"]["list"].as_array() else {
        bail!("cargo-audit's output for {location} has no list of vulnerabilities");
    };
    list.iter().map(|item| cargo_finding(item, location)).collect()
}

fn cargo_finding(item: &Value, location: &str) -> Result<Finding> {
    let advisory = &item["advisory"];
    let package = &item["package"];
    let (Some(id), Some(name), Some(version)) =
        (advisory["id"].as_str(), package["name"].as_str(), package["version"].as_str())
    else {
        bail!(
            "cargo-audit's output for {location} has a vulnerability with no id, crate or version"
        );
    };
    let url = match advisory["url"].as_str() {
        Some(url) if !url.is_empty() => url.to_string(),
        _ => format!("https://rustsec.org/advisories/{id}"),
    };
    Ok(Finding {
        id: id.to_string(),
        aliases: strings(&advisory["aliases"]),
        severity: advisory["cvss"].as_str().and_then(cvss3_base_score).map(Severity::from_score),
        summary: advisory["title"].as_str().unwrap_or(id).to_string(),
        component: format!("{name} {version}"),
        source: Source::CargoAudit,
        url,
        location: location.to_string(),
    })
}

/// The strings of a JSON array, or none when it is not one.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// The base score of a CVSS 3.0 or 3.1 vector, as FIRST's specification of
/// 3.1 computes it (section 7.1), or none when the text is not such a
/// vector or lacks one of its eight base metrics. 3.0 differs from 3.1
/// only in how it rounds, by at most 0.1. CVSS 4.0 has no formula but a
/// table of 270 scores, which the scan does not carry: a vector of 4.0 gives
/// no score.
fn cvss3_base_score(vector: &str) -> Option<f64> {
    let mut parts = vector.split('/');
    if !matches!(parts.next()?, "CVSS:3.0" | "CVSS:3.1") {
        return None;
    }
    let mut metrics = BTreeMap::new();
    for part in parts {
        let (key, value) = part.split_once(':')?;
        if metrics.insert(key, value).is_some() {
            return None;
        }
    }
    let metric = |key: &str| metrics.get(key).copied();
    let changed = match metric("S")? {
        "U" => false,
        "C" => true,
        _ => return None,
    };
    let attack_vector = match metric("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let complexity = match metric("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let privileges = match (metric("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let interaction = match metric("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let weight = |key: &str| match metric(key)? {
        "H" => Some(0.56),
        "L" => Some(0.22),
        "N" => Some(0.0),
        _ => None,
    };
    let (confidentiality, integrity, availability) = (weight("C")?, weight("I")?, weight("A")?);

    let impact_base: f64 = 1.0 - (1.0 - confidentiality) * (1.0 - integrity) * (1.0 - availability);
    let impact = if changed {
        7.52 * (impact_base - 0.029) - 3.25 * (impact_base - 0.02).powi(15)
    } else {
        6.42 * impact_base
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability = 8.22 * attack_vector * complexity * privileges * interaction;
    let sum: f64 = if changed { 1.08 * (impact + exploitability) } else { impact + exploitability };
    Some(round_up(sum.min(10.0)))
}

/// The smallest number with one decimal that is not less than `value`,
/// as specification 3.1 defines it, so that floating point error in a
/// sum such as 4.000000000000001 does not round up to 4.1.
fn round_up(value: f64) -> f64 {
    // A cast of a score of at most 10, scaled, which an i64 holds.
    let scaled = (value * 100_000.0).round() as i64;
    if scaled % 10_000 == 0 {
        scaled as f64 / 100_000.0
    } else {
        ((scaled / 10_000) + 1) as f64 / 10.0
    }
}

/// The ports to ask OSV about, which are those whose recipe pins a git
/// commit, and those it cannot be asked about, which are pinned by a
/// release archive alone. A port of crates or of other sources has
/// neither.
fn split_ports(recipes: &[Recipe]) -> (Vec<PinnedPort>, Vec<NotCovered>) {
    let mut pinned = Vec::new();
    let mut not_covered = Vec::new();
    for recipe in recipes {
        let source = &recipe.source;
        if !source.git.is_empty() && !source.commit.is_empty() {
            pinned.push(PinnedPort {
                name: recipe.name.clone(),
                version: recipe.version.clone(),
                commit: source.commit.clone(),
            });
        } else if !source.url.is_empty() {
            not_covered.push(NotCovered {
                port: recipe.name.clone(),
                version: recipe.version.clone(),
                url: source.url.clone(),
            });
        }
    }
    (pinned, not_covered)
}

/// Asks OSV: `ask(path, body)` is a GET of the path, or a POST of the body,
/// answered with the text OSV sent.
type Ask<'a> = &'a dyn Fn(&str, Option<&str>) -> Result<String>;

/// OSV's advisories for the commits the ports pin, each with the summary,
/// aliases and severity of its own record.
fn osv_findings(ask: Ask, ports: &[PinnedPort]) -> Result<Vec<Finding>> {
    let commits: Vec<&str> = ports.iter().map(|port| port.commit.as_str()).collect();
    let hits = osv_ids(ask, &commits)?;
    let mut records: BTreeMap<String, Value> = BTreeMap::new();
    for id in hits.iter().flatten() {
        if !records.contains_key(id) {
            let text = ask(&format!("/v1/vulns/{}", path_segment(id)), None)?;
            let record = serde_json::from_str(&text)
                .map_err(|err| Error::new(format!("OSV's record of {id} is not JSON: {err}")))?;
            records.insert(id.clone(), record);
        }
    }
    let mut findings = Vec::new();
    for (port, ids) in ports.iter().zip(&hits) {
        for id in ids {
            if let Some(finding) = osv_finding(&records[id], id, port) {
                findings.push(finding);
            }
        }
    }
    Ok(findings)
}

/// The IDs of the advisories OSV has for each commit, in the order of
/// `commits`, with the pages of a long answer put together.
fn osv_ids(ask: Ask, commits: &[&str]) -> Result<Vec<Vec<String>>> {
    let mut ids: Vec<Vec<String>> = vec![Vec::new(); commits.len()];
    let mut pending: Vec<(usize, Option<String>)> = (0..commits.len()).map(|n| (n, None)).collect();
    for _ in 0..OSV_PAGES {
        if pending.is_empty() {
            return Ok(ids);
        }
        let queries: Vec<Value> = pending
            .iter()
            .map(|(n, token)| match token {
                Some(token) => json!({ "commit": commits[*n], "page_token": token }),
                None => json!({ "commit": commits[*n] }),
            })
            .collect();
        let answer = ask("/v1/querybatch", Some(&json!({ "queries": queries }).to_string()))?;
        let results = batch_results(&answer, pending.len())?;
        let asked = std::mem::take(&mut pending);
        for ((n, _), (found, next)) in asked.into_iter().zip(results) {
            ids[n].extend(found);
            if let Some(token) = next {
                pending.push((n, Some(token)));
            }
        }
    }
    if pending.is_empty() {
        return Ok(ids);
    }
    bail!("OSV answered with more than {OSV_PAGES} pages for one commit")
}

/// The results of a `querybatch` answer: for each of the `asked` queries,
/// the IDs of its advisories, and the token of its next page when it has
/// one. A query with no advisory has an empty object for a result.
fn batch_results(answer: &str, asked: usize) -> Result<Vec<(Vec<String>, Option<String>)>> {
    let document: Value = serde_json::from_str(answer)
        .map_err(|err| Error::new(format!("OSV's answer to querybatch is not JSON: {err}")))?;
    let Some(results) = document["results"].as_array().filter(|results| results.len() == asked)
    else {
        bail!(
            "OSV's answer to querybatch does not have one result for each of the {asked} commits"
        );
    };
    Ok(results
        .iter()
        .map(|result| {
            let found = result["vulns"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|vuln| vuln["id"].as_str().map(str::to_string))
                .collect();
            let next = result["next_page_token"].as_str().map(str::to_string);
            (found, next)
        })
        .collect())
}

/// An ID as one part of a URL's path: what is not a letter, a digit or one
/// of `-._~` is percent-encoded.
fn path_segment(id: &str) -> String {
    let mut out = String::new();
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The finding of OSV's record of `id` for a port, or none for a record
/// that its database has withdrawn.
fn osv_finding(record: &Value, id: &str, port: &PinnedPort) -> Option<Finding> {
    if record["withdrawn"].as_str().is_some() {
        return None;
    }
    let summary = record["summary"]
        .as_str()
        .filter(|summary| !summary.trim().is_empty())
        .or_else(|| {
            record["details"].as_str().and_then(|d| d.lines().find(|l| !l.trim().is_empty()))
        })
        .unwrap_or(id);
    Some(Finding {
        id: id.to_string(),
        aliases: strings(&record["aliases"]),
        severity: osv_severity(record),
        summary: summary.trim().to_string(),
        component: format!("{} {}", port.name, port.commit),
        source: Source::Osv,
        url: format!("https://osv.dev/vulnerability/{}", path_segment(id)),
        location: format!("ports/{}/recipe.toml", port.name),
    })
}

/// The severity of an OSV record: the one its database names
/// (`database_specific.severity`, GitHub's CRITICAL, HIGH, MODERATE or
/// LOW), else the most severe of the scores its CVSS 3 entries give. A
/// CVSS 4.0 vector or an unreadable one gives none, and a record with none
/// has no severity.
fn osv_severity(record: &Value) -> Option<Severity> {
    if let Some(named) =
        record["database_specific"]["severity"].as_str().and_then(Severity::from_name)
    {
        return Some(named);
    }
    record["severity"]
        .as_array()?
        .iter()
        .filter_map(|entry| entry["score"].as_str())
        .filter_map(|score| score.parse::<f64>().ok().or_else(|| cvss3_base_score(score)))
        .map(Severity::from_score)
        .min()
}

/// `curl` asking `url`: a POST of `body` as JSON, or a GET without one.
fn curl(ctx: &Ctx, url: &str, body: Option<&str>) -> Result<String> {
    let mut cmd = Command::new("curl");
    cmd.args(["--silent", "--show-error", "--fail", "--max-time", "120", "--retry", "3"]);
    cmd.args(["--header", "Accept: application/json"]);
    if body.is_some() {
        cmd.args(["--header", "Content-Type: application/json", "--data-binary", "@-"]);
    }
    cmd.arg(url).stdin(if body.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if ctx.verbose {
        eprintln!("+ {}", process::render(&cmd));
    }
    let mut child = cmd.spawn().map_err(|err| process::spawn_error("curl", &err))?;
    if let (Some(body), Some(mut stdin)) = (body, child.stdin.take()) {
        stdin.write_all(body.as_bytes()).context(|| format!("sending a request to {url}"))?;
    }
    let output = child.wait_with_output().context(|| format!("waiting for {url}"))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "asking {url} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .hint("the scan needs to reach OSV; --osv-url names another server"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The findings, most severe first, then those with no severity, each
/// group by ID.
fn sorted(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|a, b| {
        let by_severity = match (a.severity, b.severity) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_severity
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.component.cmp(&b.component))
            .then_with(|| a.location.cmp(&b.location))
    });
    findings.dedup();
    findings
}

/// The scan's text for a person.
fn render_text(report: &Report) -> String {
    let mut out = String::new();
    if report.findings.is_empty() {
        out.push_str("No advisory matches the Cargo.lock files or the ports' commits.\n");
    } else {
        let _ = writeln!(out, "{} findings:", report.findings.len());
        for finding in &report.findings {
            let severity = finding.severity.map_or("no severity", Severity::name);
            let _ = writeln!(out, "\n{}  {severity}  {}", finding.id, finding.component);
            let _ = writeln!(out, "  {}", finding.summary);
            let _ = writeln!(out, "  found in {} by {}", finding.location, finding.source.name());
            if !finding.aliases.is_empty() {
                let _ = writeln!(out, "  also {}", finding.aliases.join(", "));
            }
            let _ = writeln!(out, "  {}", finding.url);
        }
    }
    if !report.not_covered.is_empty() {
        let _ = writeln!(
            out,
            "\nNot covered, since {} ports are pinned by a release archive and not by a git commit, \
             which is what OSV is asked by:",
            report.not_covered.len()
        );
        for port in &report.not_covered {
            let _ = writeln!(out, "  {} {}", port.port, port.version);
        }
    }
    out
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

    fn recipe(name: &str, source: &str) -> Recipe {
        toml::from_str(&format!(
            "name = \"{name}\"\nversion = \"1.2.3\"\nsummary = \"s\"\nlicense = \"MIT\"\n\
             homepage = \"https://example.org\"\n[source]\n{source}\n"
        ))
        .unwrap()
    }

    const COMMIT: &str = "0123456789012345678901234567890123456789";

    #[test]
    fn scan_options_are_json_and_the_osv_address() {
        assert_eq!(
            scan_options(&[]).unwrap(),
            ScanOptions { json: false, osv_url: OSV_URL.to_string() }
        );
        assert_eq!(
            scan_options(&["--json", "--osv-url", "http://127.0.0.1:9/"]).unwrap(),
            ScanOptions { json: true, osv_url: "http://127.0.0.1:9".to_string() }
        );
        assert_eq!(scan_options(&["--osv-url=http://x.test"]).unwrap().osv_url, "http://x.test");
        assert!(scan_options(&["--osv-url"]).is_err());
        assert!(scan_options(&["--fix"]).is_err());
    }

    #[test]
    fn severities_are_named_by_databases_and_ranked_by_score() {
        assert_eq!(Severity::from_name("CRITICAL"), Some(Severity::Critical));
        assert_eq!(Severity::from_name("High"), Some(Severity::High));
        assert_eq!(Severity::from_name("MODERATE"), Some(Severity::Medium));
        assert_eq!(Severity::from_name("medium"), Some(Severity::Medium));
        assert_eq!(Severity::from_name("LOW"), Some(Severity::Low));
        assert_eq!(Severity::from_name("UNKNOWN"), None);
        assert_eq!(Severity::from_name(""), None);

        // The ADR's table: Critical from 9.0, High from 7.0, Medium from 4.0.
        assert_eq!(Severity::from_score(10.0), Severity::Critical);
        assert_eq!(Severity::from_score(9.0), Severity::Critical);
        assert_eq!(Severity::from_score(8.9), Severity::High);
        assert_eq!(Severity::from_score(7.0), Severity::High);
        assert_eq!(Severity::from_score(6.9), Severity::Medium);
        assert_eq!(Severity::from_score(4.0), Severity::Medium);
        assert_eq!(Severity::from_score(3.9), Severity::Low);
        assert_eq!(Severity::from_score(0.0), Severity::Low);
        assert!(Severity::Critical < Severity::High && Severity::Medium < Severity::Low);
    }

    #[test]
    fn cvss_3_scores_follow_the_specification() {
        // Scores that FIRST's calculator and the advisories' own give.
        let score = |vector: &str| cvss3_base_score(vector);
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), Some(9.8));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H"), Some(10.0));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"), Some(7.5));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:H"), Some(8.8));
        assert_eq!(score("CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:H"), Some(7.8));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N"), Some(6.1));
        assert_eq!(score("CVSS:3.1/AV:L/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N"), Some(1.8));
        assert_eq!(score("CVSS:3.1/AV:P/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N"), Some(1.6));
        assert_eq!(score("CVSS:3.0/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), Some(9.8));
        // No impact is no score.
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N"), Some(0.0));
        // Temporal and environmental metrics after the base ones change nothing.
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H/E:P/RL:O/RC:C"), Some(9.8));
    }

    #[test]
    fn cvss_vectors_that_are_not_3_give_no_score() {
        // 4.0 has no formula, and nothing is guessed.
        assert_eq!(
            cvss3_base_score("CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"),
            None
        );
        assert_eq!(cvss3_base_score(""), None);
        assert_eq!(cvss3_base_score("9.8"), None);
        assert_eq!(cvss3_base_score("AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), None);
        // A base metric missing, repeated, or with a value it does not have.
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H"), None);
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), None);
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:X/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), None);
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:Z"), None);
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:Q/C:H/I:H/A:H"), None);
    }

    #[test]
    fn scores_round_up_to_one_decimal() {
        assert_eq!(round_up(4.0), 4.0);
        // Floating point error above a whole step does not round up a step.
        assert_eq!(round_up(4.000_000_000_000_001), 4.0);
        assert_eq!(round_up(4.02), 4.1);
        assert_eq!(round_up(4.1), 4.1);
        assert_eq!(round_up(9.81), 9.9);
        assert_eq!(round_up(0.0), 0.0);
        assert_eq!(round_up(10.0), 10.0);
    }

    #[test]
    fn lock_files_are_found_by_name_in_a_listing_from_git() {
        let listing = "Cargo.lock\0Cargo.toml\0userspace/programs/host/Cargo.lock\0\
                       kernel/Cargo.lock\0docs/Cargo.lock.md\0userspace/Cargo.lock\0\
                       tools/xtask/src/Cargo.lock.rs\0ports/x/Cargo.lock.in\0";
        assert_eq!(
            lock_files(listing),
            [
                "Cargo.lock",
                "kernel/Cargo.lock",
                "userspace/Cargo.lock",
                "userspace/programs/host/Cargo.lock"
            ]
        );
        assert!(lock_files("").is_empty());
    }

    const AUDIT: &str = r#"{
        "database": {"advisory-count": 900},
        "lockfile": {"dependency-count": 3},
        "vulnerabilities": {"found": true, "count": 2, "list": [
            {"advisory": {"id": "RUSTSEC-2026-0001", "title": "Buffer overflow in foo",
                          "aliases": ["CVE-2026-1111", "GHSA-aaaa-bbbb-cccc"],
                          "cvss": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
                          "url": "https://github.com/x/foo/issues/1"},
             "package": {"name": "foo", "version": "1.0.0"}},
            {"advisory": {"id": "RUSTSEC-2026-0002", "title": "Leak in bar",
                          "aliases": [], "cvss": null, "url": null},
             "package": {"name": "bar", "version": "0.3.1"}}
        ]},
        "warnings": {"unmaintained": [{"kind": "unmaintained",
            "advisory": {"id": "RUSTSEC-2025-0999", "title": "baz is unmaintained"},
            "package": {"name": "baz", "version": "2.0.0"}}]}
    }"#;

    #[test]
    fn cargo_audit_vulnerabilities_are_findings_and_its_warnings_are_not() {
        let findings = parse_cargo_audit(AUDIT, "Cargo.lock").unwrap();
        assert_eq!(findings.len(), 2);
        let first = &findings[0];
        assert_eq!(first.id, "RUSTSEC-2026-0001");
        assert_eq!(first.aliases, ["CVE-2026-1111", "GHSA-aaaa-bbbb-cccc"]);
        assert_eq!(first.severity, Some(Severity::Critical));
        assert_eq!(first.summary, "Buffer overflow in foo");
        assert_eq!(first.component, "foo 1.0.0");
        assert_eq!(first.source, Source::CargoAudit);
        assert_eq!(first.url, "https://github.com/x/foo/issues/1");
        assert_eq!(first.location, "Cargo.lock");
        // An advisory with no score has no severity, and RustSec's page is its URL.
        let second = &findings[1];
        assert_eq!(second.severity, None);
        assert!(second.aliases.is_empty());
        assert_eq!(second.url, "https://rustsec.org/advisories/RUSTSEC-2026-0002");
    }

    #[test]
    fn a_clean_audit_has_no_findings_and_a_broken_one_is_an_error() {
        let clean = r#"{"vulnerabilities": {"found": false, "count": 0, "list": []}}"#;
        assert!(parse_cargo_audit(clean, "Cargo.lock").unwrap().is_empty());
        let err = parse_cargo_audit("error: no lock", "kernel/Cargo.lock").unwrap_err();
        assert!(err.to_string().contains("kernel/Cargo.lock"), "{err}");
        assert!(parse_cargo_audit("{}", "Cargo.lock").is_err());
        let incomplete = r#"{"vulnerabilities": {"list": [{"advisory": {"title": "x"}}]}}"#;
        assert!(parse_cargo_audit(incomplete, "Cargo.lock").is_err());
    }

    #[test]
    fn osv_severity_is_the_databases_name_then_a_cvss_3_score_and_never_a_guess() {
        let severity = |text: &str| osv_severity(&serde_json::from_str(text).unwrap());
        assert_eq!(
            severity(r#"{"database_specific": {"severity": "CRITICAL"}}"#),
            Some(Severity::Critical)
        );
        assert_eq!(
            severity(r#"{"database_specific": {"severity": "MODERATE"}}"#),
            Some(Severity::Medium)
        );
        // The name wins over a vector.
        assert_eq!(
            severity(
                r#"{"database_specific": {"severity": "LOW"},
                    "severity": [{"type": "CVSS_V3",
                        "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}]}"#
            ),
            Some(Severity::Low)
        );
        assert_eq!(
            severity(
                r#"{"severity": [{"type": "CVSS_V3",
                    "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"}]}"#
            ),
            Some(Severity::High)
        );
        // A plain score is read as it is; the most severe entry counts.
        assert_eq!(
            severity(
                r#"{"severity": [{"type": "CVSS_V3", "score": "3.1"},
                                  {"type": "CVSS_V3", "score": "5.5"}]}"#
            ),
            Some(Severity::Medium)
        );
        // CVSS 4.0 alone, an unknown name and nothing at all give none.
        assert_eq!(
            severity(
                r#"{"severity": [{"type": "CVSS_V4", "score":
                    "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"}]}"#
            ),
            None
        );
        assert_eq!(severity(r#"{"database_specific": {"severity": "UNKNOWN"}}"#), None);
        assert_eq!(severity("{}"), None);
        assert_eq!(severity(r#"{"severity": []}"#), None);
    }

    #[test]
    fn ports_are_asked_by_commit_or_listed_as_not_covered() {
        let recipes = [
            recipe("lua", &format!("git = \"https://example.org/lua.git\"\ncommit = \"{COMMIT}\"")),
            recipe("bash", "url = \"https://example.org/bash.tar.gz\"\nsha256 = \"ab\""),
            // Neither: the recipe pins nothing OSV can be asked about.
            recipe("rust-src", ""),
            // A git source with no commit pins nothing either.
            recipe("loose", "git = \"https://example.org/loose.git\""),
        ];
        let (pinned, not_covered) = split_ports(&recipes);
        assert_eq!(
            pinned,
            [PinnedPort {
                name: "lua".to_string(),
                version: "1.2.3".to_string(),
                commit: COMMIT.to_string()
            }]
        );
        assert_eq!(
            not_covered,
            [NotCovered {
                port: "bash".to_string(),
                version: "1.2.3".to_string(),
                url: "https://example.org/bash.tar.gz".to_string()
            }]
        );
    }

    fn pinned(name: &str, commit: &str) -> PinnedPort {
        PinnedPort { name: name.to_string(), version: "1".to_string(), commit: commit.to_string() }
    }

    /// An OSV that answers from a table, and keeps what it was asked.
    struct FakeOsv {
        asked: std::cell::RefCell<Vec<(String, Option<String>)>>,
        answers: Vec<(&'static str, String)>,
    }

    impl FakeOsv {
        fn new(answers: Vec<(&'static str, String)>) -> Self {
            Self { asked: std::cell::RefCell::new(Vec::new()), answers }
        }

        fn ask(&self, path: &str, body: Option<&str>) -> Result<String> {
            self.asked.borrow_mut().push((path.to_string(), body.map(str::to_string)));
            self.answers
                .iter()
                .filter(|(prefix, _)| path.starts_with(prefix))
                .map(|(_, answer)| answer.clone())
                .nth(self.asked.borrow().iter().filter(|(p, _)| p == path).count() - 1)
                .ok_or_else(|| Error::new(format!("unexpected request for {path}")))
        }
    }

    #[test]
    fn osv_is_asked_once_for_all_commits_and_once_for_each_advisory() {
        let batch = json!({"results": [
            {"vulns": [{"id": "GHSA-1", "modified": "x"}, {"id": "CVE-2"}]},
            {},
            {"vulns": [{"id": "GHSA-1"}]}
        ]})
        .to_string();
        let ghsa = json!({
            "id": "GHSA-1", "summary": "Heap overflow", "aliases": ["CVE-1"],
            "database_specific": {"severity": "HIGH"}
        })
        .to_string();
        let cve =
            json!({"id": "CVE-2", "details": "\nFirst line of details.\nSecond."}).to_string();
        let osv = FakeOsv::new(vec![
            ("/v1/querybatch", batch),
            ("/v1/vulns/GHSA-1", ghsa),
            ("/v1/vulns/CVE-2", cve),
        ]);
        let ports = [pinned("a", "c1"), pinned("b", "c2"), pinned("c", "c3")];
        let found = osv_findings(&|path, body| osv.ask(path, body), &ports).unwrap();

        let asked = osv.asked.borrow();
        // One batch for the three commits, and one record for each ID,
        // though GHSA-1 matches two ports.
        assert_eq!(asked.len(), 3);
        let sent: Value = serde_json::from_str(asked[0].1.as_deref().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"queries": [{"commit": "c1"}, {"commit": "c2"}, {"commit": "c3"}]})
        );
        assert!(asked[1..].iter().all(|(_, body)| body.is_none()));

        assert_eq!(found.len(), 3);
        assert_eq!(found[0].id, "GHSA-1");
        assert_eq!(found[0].component, "a c1");
        assert_eq!(found[0].severity, Some(Severity::High));
        assert_eq!(found[0].aliases, ["CVE-1"]);
        assert_eq!(found[0].source, Source::Osv);
        assert_eq!(found[0].url, "https://osv.dev/vulnerability/GHSA-1");
        assert_eq!(found[0].location, "ports/a/recipe.toml");
        assert_eq!(found[1].id, "CVE-2");
        assert_eq!(found[1].summary, "First line of details.");
        assert_eq!(found[1].severity, None);
        assert_eq!(found[2].component, "c c3");
    }

    #[test]
    fn osv_pages_of_one_commit_are_put_together() {
        let first = json!({"results": [
            {"vulns": [{"id": "A"}], "next_page_token": "t1"}, {}
        ]})
        .to_string();
        // The next page asks only for the commit that has one.
        let second = json!({"results": [{"vulns": [{"id": "B"}]}]}).to_string();
        let osv = FakeOsv::new(vec![("/v1/querybatch", first), ("/v1/querybatch", second)]);
        let ids = osv_ids(&|path, body| osv.ask(path, body), &["c1", "c2"]).unwrap();
        assert_eq!(ids, [vec!["A".to_string(), "B".to_string()], Vec::<String>::new()]);
        let asked = osv.asked.borrow();
        let again: Value = serde_json::from_str(asked[1].1.as_deref().unwrap()).unwrap();
        assert_eq!(again, json!({"queries": [{"commit": "c1", "page_token": "t1"}]}));
    }

    #[test]
    fn osv_answers_that_do_not_fit_are_errors() {
        let ask_with = |answer: &'static str| {
            move |_: &str, _: Option<&str>| -> Result<String> { Ok(answer.to_string()) }
        };
        assert!(osv_ids(&ask_with("<html>"), &["c1"]).is_err());
        // One result for each commit asked.
        assert!(osv_ids(&ask_with(r#"{"results": []}"#), &["c1"]).is_err());
        assert!(osv_ids(&ask_with("{}"), &["c1"]).is_err());
        // A server that never ends its pages is not asked for ever.
        let endless = ask_with(r#"{"results": [{"next_page_token": "t"}]}"#);
        assert!(osv_ids(&endless, &["c1"]).is_err());
        // No port is no question.
        let never = |_: &str, _: Option<&str>| -> Result<String> { panic!("asked") };
        assert!(osv_findings(&never, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_withdrawn_osv_record_is_no_finding() {
        let port = pinned("a", "c1");
        let withdrawn = json!({"id": "GHSA-9", "withdrawn": "2026-01-01T00:00:00Z"});
        assert!(osv_finding(&withdrawn, "GHSA-9", &port).is_none());
        // A record with no summary or details is named by its ID.
        let bare = osv_finding(&json!({"id": "CVE-5"}), "CVE-5", &port).unwrap();
        assert_eq!(bare.summary, "CVE-5");
        assert_eq!(bare.severity, None);
    }

    #[test]
    fn ids_are_one_safe_part_of_a_path() {
        assert_eq!(path_segment("GHSA-xxxx-1234"), "GHSA-xxxx-1234");
        assert_eq!(path_segment("a/b c?d"), "a%2Fb%20c%3Fd");
        assert_eq!(path_segment("../x"), "..%2Fx");
    }

    fn finding(id: &str, severity: Option<Severity>, component: &str) -> Finding {
        Finding {
            id: id.to_string(),
            aliases: Vec::new(),
            severity,
            summary: format!("{id} summary"),
            component: component.to_string(),
            source: Source::CargoAudit,
            url: format!("https://example.org/{id}"),
            location: "Cargo.lock".to_string(),
        }
    }

    #[test]
    fn findings_are_sorted_by_severity_with_those_with_none_last_and_are_not_repeated() {
        let all = sorted(vec![
            finding("D", None, "x 1"),
            finding("C", Some(Severity::Low), "x 1"),
            finding("B", Some(Severity::Critical), "y 1"),
            finding("A", Some(Severity::High), "x 1"),
            finding("B", Some(Severity::Critical), "x 1"),
            finding("B", Some(Severity::Critical), "x 1"),
        ]);
        let order: Vec<_> = all.iter().map(|f| (f.id.as_str(), f.component.as_str())).collect();
        assert_eq!(order, [("B", "x 1"), ("B", "y 1"), ("A", "x 1"), ("C", "x 1"), ("D", "x 1")]);
    }

    #[test]
    fn the_json_is_the_findings_and_the_ports_not_covered() {
        let report = Report {
            findings: vec![finding("A", Some(Severity::High), "x 1"), finding("B", None, "y 2")],
            not_covered: vec![NotCovered {
                port: "bash".to_string(),
                version: "5.2".to_string(),
                url: "https://example.org/bash.tar.gz".to_string(),
            }],
        };
        let value = serde_json::to_value(&report).unwrap();
        let first = &value["findings"][0];
        assert_eq!(first["id"], "A");
        assert_eq!(first["severity"], "high");
        assert_eq!(first["source"], "cargo-audit");
        assert_eq!(first["component"], "x 1");
        assert_eq!(first["url"], "https://example.org/A");
        assert!(first["aliases"].as_array().unwrap().is_empty());
        // No severity is null, which is what a reader that triages looks for.
        assert!(value["findings"][1]["severity"].is_null());
        assert_eq!(value["not_covered"][0]["port"], "bash");
        let mut osv = finding("C", None, "lua c1");
        osv.source = Source::Osv;
        assert_eq!(serde_json::to_value(&osv).unwrap()["source"], "osv");
    }

    #[test]
    fn the_text_lists_findings_and_what_was_not_covered() {
        let mut report = Report { findings: Vec::new(), not_covered: Vec::new() };
        let clean = render_text(&report);
        assert!(clean.starts_with("No advisory matches"), "{clean}");
        assert!(!clean.contains("Not covered"));

        let mut found = finding("A", Some(Severity::High), "x 1");
        found.aliases = vec!["CVE-1".to_string()];
        report.findings = vec![found, finding("B", None, "y 2")];
        report.not_covered = vec![NotCovered {
            port: "bash".to_string(),
            version: "5.2".to_string(),
            url: "u".to_string(),
        }];
        let text = render_text(&report);
        assert!(text.starts_with("2 findings:"), "{text}");
        assert!(text.contains("A  high  x 1"), "{text}");
        assert!(text.contains("B  no severity  y 2"), "{text}");
        assert!(text.contains("also CVE-1"), "{text}");
        assert!(text.contains("found in Cargo.lock by cargo-audit"), "{text}");
        assert!(text.contains("Not covered, since 1 ports"), "{text}");
        assert!(text.contains("  bash 5.2"), "{text}");
    }
}
