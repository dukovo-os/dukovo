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
//! - OSV is asked, in one `querybatch`, about what the recipes pin:
//!   - a port pinned to a git commit, and each of its extra sources, by
//!     that commit, which covers projects with no package ecosystem
//!     through the git ranges of OSV's records;
//!   - a port pinned by a release archive, by the commit of the tag its
//!     `[upstream]` names, which `git ls-remote` resolves, one call for
//!     each port. A port whose `[upstream]` says `none` has no commit to
//!     ask by, and is listed as not covered, with its reason;
//!   - each crate a recipe lists (`[[crate]]`), by its crates.io name and
//!     version, unless a Cargo.lock that cargo-audit read lists it.
//!
//! A finding has the severity its advisory gives, which is the severity
//! its database names or the base score of its CVSS 3 vector, and none
//! when the advisory gives neither: the scan never rates a flaw itself. The scan
//! ends well whatever it finds, and fails only when it could not run to
//! its end, since a scan that missed a source would close issues for
//! findings it did not look for: that includes a tag it could not resolve,
//! which names its port. When it misbehaves, run it by hand with
//! `--verbose`: each cargo-audit, git and curl command is printed.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::{Value, json};

use crate::ports::{self, Recipe, Upstream};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result, bail, release, upstream, vex};

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
/// of its own, name versions of Dukovo's order, and say no TODO; and
/// [`vex::FILE`] must be OpenVEX 0.2.0 as [`vex`] reads it.
fn check(ctx: &Ctx) -> Result<()> {
    let statements = vex::read(&ctx.root)?;
    step(&format!("{} has {} statements", vex::FILE, statements.statements.len()));
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
    /// A crate and its version, or a port and its commit (see [`Asked`]).
    component: String,
    /// The package URL of the part, as a statement of [`vex::FILE`] names
    /// it (see [`vex::port_product`]).
    product: String,
    source: Source,
    /// The advisory, where a person reads more of it.
    url: String,
    /// The Cargo.lock, or the recipe, that holds the component.
    location: String,
}

/// A port that the scan does not ask OSV about: one pinned by a release
/// archive of a project with no public git repository to find the
/// archive's commit in, or one that takes only data from its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct NotCovered {
    port: String,
    version: String,
    url: String,
    /// Why, from the recipe: its `[upstream]`'s `none` or its `data_only`.
    reason: String,
}

/// A finding that a statement of [`vex::FILE`] clears, left out of the
/// findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct NotAffected {
    id: String,
    product: String,
    component: String,
    /// `not_affected` or `fixed`.
    status: &'static str,
    justification: Option<String>,
    /// Why, as the statement says.
    statement: String,
}

/// A product of a statement of [`vex::FILE`] that clears no finding: the
/// advisory no longer matches it, or the part has moved to another
/// version, so the statement can go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Unused {
    id: String,
    product: String,
}

/// What the scan found, what of it the statements clear, what it did not
/// look at, and which statements cleared nothing.
#[derive(Debug, Serialize)]
struct Report {
    findings: Vec<Finding>,
    not_affected: Vec<NotAffected>,
    not_covered: Vec<NotCovered>,
    unused_statements: Vec<Unused>,
}

/// What OSV is asked, in the form its `querybatch` takes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Query {
    /// The advisories whose git ranges hold a commit.
    Commit(String),
    /// The advisories of a crate of crates.io at a version.
    Crate { name: String, version: String },
}

impl Query {
    /// The query as `querybatch` reads it, asking for the page of a long
    /// answer that `token` names, if it does.
    fn json(&self, token: Option<&str>) -> Value {
        let mut query = match self {
            Self::Commit(commit) => json!({ "commit": commit }),
            Self::Crate { name, version } => json!({
                "package": { "ecosystem": "crates.io", "name": name },
                "version": version,
            }),
        };
        if let Some(token) = token {
            query["page_token"] = json!(token);
        }
        query
    }
}

/// A part of the system that OSV is asked about, and the recipe that pins it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asked {
    /// How a finding names it: a port and the commit it is asked by, as
    /// `bat 0123...`; a port's tag too, for a release archive, and its
    /// path for an extra source; a crate and its version, as cargo-audit
    /// names one.
    component: String,
    /// Its package URL (see [`vex::port_product`]).
    product: String,
    /// The recipe, as a path in the repository.
    location: String,
    query: Query,
}

/// A port pinned by a release archive, with the repository and the tag its
/// `[upstream]` names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchivePort {
    name: String,
    product: String,
    git: String,
    tag: String,
}

/// What the recipes pin, by how OSV is asked about it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Plan {
    /// Ports and extra sources pinned to a git commit.
    commits: Vec<Asked>,
    /// Ports pinned by a release archive, whose tag is yet to be resolved
    /// to a commit.
    archives: Vec<ArchivePort>,
    /// The crates that recipes list.
    crates: Vec<Asked>,
    /// Ports with no repository to ask by.
    not_covered: Vec<NotCovered>,
}

/// The options of `ob security scan`.
#[derive(Debug, PartialEq, Eq)]
struct ScanOptions {
    json: bool,
    osv_url: String,
}

/// OSV's address; its API takes `POST /v1/querybatch` and `GET /v1/vulns/<id>`.
const OSV_URL: &str = "https://api.osv.dev";

/// How many pages of one query's answers the scan reads: OSV pages an
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
    let statements = vex::read(&ctx.root)?;
    upstream::require_cargo_audit()?;
    let mut findings = Vec::new();
    let mut locked = BTreeSet::new();
    for (lock, location) in system_locks(ctx)? {
        findings.extend(audit_lock(ctx, &lock, &location)?);
        locked.extend(locked_crates(&lock, &location)?);
    }
    for found in upstream::bundle_locks(ctx)? {
        let location = format!("{} {} (its Cargo.lock)", found.port, found.version);
        findings.extend(audit_lock(ctx, &found.lock, &location)?);
        locked.extend(locked_crates(&found.lock, &location)?);
    }

    let recipes: Vec<Recipe> = ports::recipes(ctx)?.into_iter().map(|(recipe, _)| recipe).collect();
    let plan = plan(&recipes);
    step(&format!("Finding the commits of {} release tags", plan.archives.len()));
    let mut asked = plan.commits;
    asked.extend(resolve_archives(&plan.archives, &|git, tag| ls_remote(ctx, git, tag))?);
    asked.extend(unlocked(plan.crates, &locked));
    step(&format!("Asking OSV about {} ports, sources and crates", asked.len()));
    let osv_url = options.osv_url;
    let ask = |path: &str, body: Option<&str>| curl(ctx, &format!("{osv_url}{path}"), body);
    findings.extend(osv_findings(&ask, &asked)?);

    let report = report(&statements, sorted(findings), plan.not_covered);
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

/// `cargo audit --json` on one lock file, which only reads it. Each call
/// brings the advisory database up to date, which costs a moment, and the
/// crates.io index's entries for the lock's crates, without which
/// cargo-audit's check for yanked crates reports an error for each crate.
fn audit_lock(ctx: &Ctx, lock: &Path, location: &str) -> Result<Vec<Finding>> {
    step(&format!("Auditing {location}"));
    let mut cmd = Command::new("cargo");
    cmd.args(["audit", "--json"]);
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
        product: vex::crate_product(name, version),
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

/// What the recipes pin, and how OSV is asked about each part: a port, and
/// each extra source, that pins a git commit by that commit; a port that
/// pins a release archive by the tag its `[upstream]` names, which is a
/// commit once resolved, or as not covered when it says `none`; each
/// crate a recipe lists by its name and version. A port that takes only
/// data from its source is not asked about, and is listed as not covered
/// with its reason. A port of other sources has none of these.
fn plan(recipes: &[Recipe]) -> Plan {
    let mut plan = Plan::default();
    for recipe in recipes {
        let source = &recipe.source;
        let location = format!("ports/{}/recipe.toml", recipe.name);
        let product = vex::port_product(&recipe.name, &recipe.version);
        if let Some(reason) = &recipe.data_only {
            plan.not_covered.push(NotCovered {
                port: recipe.name.clone(),
                version: recipe.version.clone(),
                url: if source.git.is_empty() { source.url.clone() } else { source.git.clone() },
                reason: format!("only data is taken: {reason}"),
            });
            continue;
        }
        if !source.git.is_empty() && !source.commit.is_empty() {
            plan.commits.push(Asked {
                component: format!("{} {}", recipe.name, source.commit),
                product: product.clone(),
                location: location.clone(),
                query: Query::Commit(source.commit.clone()),
            });
        } else if !source.url.is_empty() {
            let upstream = recipe.upstream.as_ref();
            if let Some((git, tag)) = upstream.and_then(Upstream::tagged) {
                plan.archives.push(ArchivePort {
                    name: recipe.name.clone(),
                    product: product.clone(),
                    git: git.to_string(),
                    tag: tag.to_string(),
                });
            } else {
                plan.not_covered.push(NotCovered {
                    port: recipe.name.clone(),
                    version: recipe.version.clone(),
                    url: source.url.clone(),
                    reason: upstream.and_then(|upstream| upstream.none.as_ref()).map_or_else(
                        || "the recipe has no [upstream]".to_string(),
                        |none| format!("no git repository to ask by: {none}"),
                    ),
                });
            }
        }
        for extra in &recipe.extra {
            plan.commits.push(Asked {
                component: format!("{} {} {}", recipe.name, extra.path, extra.commit),
                product: vex::port_product(repository_name(&extra.git), &extra.commit),
                location: location.clone(),
                query: Query::Commit(extra.commit.clone()),
            });
        }
        for krate in &recipe.crates {
            plan.crates.push(Asked {
                component: format!("{} {}", krate.name, krate.version),
                product: vex::crate_product(&krate.name, &krate.version),
                location: location.clone(),
                query: Query::Crate { name: krate.name.clone(), version: krate.version.clone() },
            });
        }
    }
    plan
}

/// The name of the repository at `git`, the last part of its URL without
/// `.git`, as `pcre2` of `https://github.com/PCRE2Project/pcre2.git`: an
/// extra source's product is named by it and by the commit pinned, so that
/// a statement about an extra source lapses when its pin moves, whatever
/// the port's own version.
fn repository_name(git: &str) -> &str {
    let path = git.trim_end_matches('/');
    let last = path.rsplit('/').next().unwrap_or(path);
    last.strip_suffix(".git").unwrap_or(last)
}

/// The report of `findings`: those a statement clears are moved to its
/// `not_affected`, and each product of a clearing statement that cleared
/// no finding is listed as unused.
fn report(
    statements: &vex::Document,
    findings: Vec<Finding>,
    not_covered: Vec<NotCovered>,
) -> Report {
    let mut kept = Vec::new();
    let mut not_affected = Vec::new();
    let mut used = BTreeSet::new();
    for finding in findings {
        let mut ids = vec![finding.id.as_str()];
        ids.extend(finding.aliases.iter().map(String::as_str));
        let Some(statement) = statements.clearing(&ids, &finding.product) else {
            kept.push(finding);
            continue;
        };
        used.insert((statement.ids[0].clone(), finding.product.clone()));
        not_affected.push(NotAffected {
            id: finding.id,
            product: finding.product,
            component: finding.component,
            status: statement.status.name(),
            justification: statement.justification.clone(),
            statement: statement.reason.clone(),
        });
    }
    not_affected.dedup();
    let unused_statements = statements
        .statements
        .iter()
        .filter(|statement| statement.status.clears())
        .flat_map(|statement| {
            statement.products.iter().map(|product| (statement.ids[0].clone(), product.clone()))
        })
        .filter(|key| !used.contains(key))
        .map(|(id, product)| Unused { id, product })
        .collect();
    Report { findings: kept, not_affected, not_covered, unused_statements }
}

/// The commits of the tags that archive ports name, as `ls_remote(git, tag)`
/// lists them, each to ask OSV by. It fails, naming every port whose tag it
/// could not resolve, since a scan that did not see a port must not be
/// taken for one that saw it.
fn resolve_archives(
    ports: &[ArchivePort],
    ls_remote: &dyn Fn(&str, &str) -> Result<String>,
) -> Result<Vec<Asked>> {
    let mut asked = Vec::new();
    let mut failed = Vec::new();
    for port in ports {
        let listing = match ls_remote(&port.git, &port.tag) {
            Ok(listing) => listing,
            Err(err) => {
                failed.push(format!("{}: {err}", port.name));
                continue;
            }
        };
        let Some(commit) = tag_commit(&listing, &port.tag) else {
            failed.push(format!("{}: {} has no tag {}", port.name, port.git, port.tag));
            continue;
        };
        asked.push(Asked {
            component: format!("{} {} {commit}", port.name, port.tag),
            product: port.product.clone(),
            location: format!("ports/{}/recipe.toml", port.name),
            query: Query::Commit(commit),
        });
    }
    if failed.is_empty() {
        return Ok(asked);
    }
    Err(Error::new(format!(
        "could not find the commit of the release of:\n  {}",
        failed.join("\n  ")
    ))
    .hint("the scan reads the tag of each [upstream] from its repository with git ls-remote"))
}

/// `git ls-remote` of the tag in the repository `git`, and of the commit
/// the tag points to when it is annotated, tried as
/// [`upstream::ls_remote`] tries.
fn ls_remote(ctx: &Ctx, git: &str, tag: &str) -> Result<String> {
    upstream::ls_remote(
        ctx,
        git,
        &[],
        &[format!("refs/tags/{tag}"), format!("refs/tags/{tag}^{{}}")],
    )
}

/// The commit `tag` names, from the listing `git ls-remote` printed for it.
/// An annotated tag has the commit it points to on a line of its own,
/// `refs/tags/<tag>^{}`, and that is the one taken: the tag's own line has
/// the ID of the tag object, which no advisory names. A tag that is not
/// annotated names its commit on its own line.
fn tag_commit(listing: &str, tag: &str) -> Option<String> {
    let (own, peeled) = (format!("refs/tags/{tag}"), format!("refs/tags/{tag}^{{}}"));
    let mut found = None;
    for line in listing.lines() {
        let Some((id, name)) = line.split_once('\t') else { continue };
        if !matches!(id.len(), 40 | 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        if name == peeled {
            return Some(id.to_string());
        }
        if name == own {
            found = Some(id.to_string());
        }
    }
    found
}

/// The crates.io crates that a Cargo.lock lists, as name and version: the
/// packages whose `source` is crates.io's index, which leaves out the
/// repository's own crates and any from git or from a path.
fn locked_crates(lock: &Path, location: &str) -> Result<BTreeSet<(String, String)>> {
    let text = std::fs::read_to_string(lock).context(|| format!("reading {}", lock.display()))?;
    crates_in_lock(&text, location)
}

/// The crates of the Cargo.lock whose text is `text`; `location` names the
/// lock in an error.
fn crates_in_lock(text: &str, location: &str) -> Result<BTreeSet<(String, String)>> {
    let document: toml::Table = text
        .parse()
        .map_err(|err| Error::new(format!("the Cargo.lock of {location} is not TOML: {err}")))?;
    let packages = document.get("package").and_then(toml::Value::as_array);
    Ok(packages
        .into_iter()
        .flatten()
        .filter_map(|package| {
            let field = |key| package.get(key).and_then(toml::Value::as_str);
            let (name, version, source) = (field("name")?, field("version")?, field("source")?);
            source.contains("crates.io").then(|| (name.to_string(), version.to_string()))
        })
        .collect())
}

/// The crates to ask OSV about: each name and version once, though two
/// ports list it, and none that a Cargo.lock cargo-audit read lists, since
/// the RustSec database has answered for those, and OSV would name the
/// same advisories again.
fn unlocked(crates: Vec<Asked>, locked: &BTreeSet<(String, String)>) -> Vec<Asked> {
    let mut seen = BTreeSet::new();
    crates
        .into_iter()
        .filter(|asked| {
            let Query::Crate { name, version } = &asked.query else { return true };
            let key = (name.clone(), version.clone());
            !locked.contains(&key) && seen.insert(key)
        })
        .collect()
}

/// Asks OSV: `ask(path, body)` is a GET of the path, or a POST of the body,
/// answered with the text OSV sent.
type Ask<'a> = &'a dyn Fn(&str, Option<&str>) -> Result<String>;

/// OSV's advisories for what was asked about, each with the summary,
/// aliases and severity of its own record.
fn osv_findings(ask: Ask, asked: &[Asked]) -> Result<Vec<Finding>> {
    let queries: Vec<&Query> = asked.iter().map(|asked| &asked.query).collect();
    let hits = osv_ids(ask, &queries)?;
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
    for (part, ids) in asked.iter().zip(&hits) {
        for id in ids {
            if let Some(finding) = osv_finding(&records[id], id, part) {
                findings.push(finding);
            }
        }
    }
    Ok(findings)
}

/// The IDs of the advisories OSV has for each query, in the order of
/// `queries`, with the pages of a long answer put together.
fn osv_ids(ask: Ask, queries: &[&Query]) -> Result<Vec<Vec<String>>> {
    let mut ids: Vec<Vec<String>> = vec![Vec::new(); queries.len()];
    let mut pending: Vec<(usize, Option<String>)> = (0..queries.len()).map(|n| (n, None)).collect();
    for _ in 0..OSV_PAGES {
        if pending.is_empty() {
            return Ok(ids);
        }
        let body: Vec<Value> =
            pending.iter().map(|(n, token)| queries[*n].json(token.as_deref())).collect();
        let answer = ask("/v1/querybatch", Some(&json!({ "queries": body }).to_string()))?;
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
    bail!("OSV answered with more than {OSV_PAGES} pages for one query")
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
            "OSV's answer to querybatch does not have one result for each of the {asked} queries"
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

/// The finding of OSV's record of `id` for what was asked about, or none
/// for a record that its database has withdrawn.
fn osv_finding(record: &Value, id: &str, asked: &Asked) -> Option<Finding> {
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
        component: asked.component.clone(),
        product: asked.product.clone(),
        source: Source::Osv,
        url: format!("https://osv.dev/vulnerability/{}", path_segment(id)),
        location: asked.location.clone(),
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
        out.push_str(
            "No advisory matches the Cargo.lock files, the ports' commits or their crates.\n",
        );
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
    if !report.not_affected.is_empty() {
        let _ = writeln!(
            out,
            "\n{} findings are cleared by statements of {}:",
            report.not_affected.len(),
            vex::FILE
        );
        for cleared in &report.not_affected {
            let _ = writeln!(out, "  {} in {}: {}", cleared.id, cleared.product, cleared.status);
        }
    }
    if !report.not_covered.is_empty() {
        let _ = writeln!(out, "\nNot asked about, {} ports:", report.not_covered.len());
        for port in &report.not_covered {
            let _ = writeln!(out, "  {} {}: {}", port.port, port.version, port.reason);
        }
    }
    if !report.unused_statements.is_empty() {
        let _ = writeln!(out, "\nStatements of {} that clear no finding, which can go:", vex::FILE);
        for unused in &report.unused_statements {
            let _ = writeln!(out, "  {} in {}", unused.id, unused.product);
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

    fn lua() -> Recipe {
        recipe("lua", &format!("git = \"https://example.org/lua.git\"\ncommit = \"{COMMIT}\""))
    }

    const ARCHIVE: &str = "url = \"https://example.org/curl-8.23.0.tar.xz\"\nsha256 = \"ab\"";

    #[test]
    fn a_port_is_asked_by_its_commit_and_its_extra_sources_by_theirs() {
        let other = "0123456789abcdef0123456789abcdef01234567";
        let nginx = recipe(
            "nginx",
            &format!(
                "git = \"https://example.org/nginx.git\"\ncommit = \"{COMMIT}\"\n\
                 [[extra]]\npath = \"pcre2\"\ngit = \"https://example.org/pcre2.git\"\n\
                 commit = \"{other}\"\nlicense = \"BSD-3-Clause\""
            ),
        );
        let plan = plan(&[lua(), nginx]);
        let asked: Vec<_> =
            plan.commits.iter().map(|a| (&*a.component, &*a.location, &a.query)).collect();
        assert_eq!(
            asked,
            [
                (
                    &*format!("lua {COMMIT}"),
                    "ports/lua/recipe.toml",
                    &Query::Commit(COMMIT.to_string())
                ),
                (
                    &*format!("nginx {COMMIT}"),
                    "ports/nginx/recipe.toml",
                    &Query::Commit(COMMIT.to_string())
                ),
                (
                    &*format!("nginx pcre2 {other}"),
                    "ports/nginx/recipe.toml",
                    &Query::Commit(other.to_string())
                ),
            ]
        );
        let products: Vec<_> = plan.commits.iter().map(|a| &*a.product).collect();
        assert_eq!(
            products,
            [
                "pkg:generic/lua@1.2.3",
                "pkg:generic/nginx@1.2.3",
                &*format!("pkg:generic/pcre2@{other}")
            ]
        );
        assert!(plan.archives.is_empty() && plan.crates.is_empty() && plan.not_covered.is_empty());
    }

    #[test]
    fn a_repository_is_named_by_the_last_part_of_its_url() {
        assert_eq!(repository_name("https://github.com/PCRE2Project/pcre2.git"), "pcre2");
        assert_eq!(repository_name("https://github.com/antirez/linenoise.git/"), "linenoise");
        assert_eq!(repository_name("https://gitlab.example.org/group/frigg"), "frigg");
    }

    #[test]
    fn a_port_pinned_by_an_archive_is_asked_by_its_upstream_tag_or_not_covered_with_the_reason() {
        let curl = recipe(
            "curl",
            &format!(
                "{ARCHIVE}\n[upstream]\ngit = \"https://example.org/curl.git\"\ntag = \"curl-8_23_0\""
            ),
        );
        let zip = recipe("zip", &format!("{ARCHIVE}\n[upstream]\nnone = \"no repository\""));
        // The check refuses this recipe, so no scan reads it; if one did it
        // would say why the port is not covered, not leave it out.
        let bare = recipe("bare", ARCHIVE);
        // Nothing to ask about: a git source with no commit, a port of no source.
        let loose = recipe("loose", "git = \"https://example.org/loose.git\"");
        let none = recipe("rust-src", "");
        let plan = plan(&[curl, zip, bare, loose, none]);
        assert_eq!(
            plan.archives,
            [ArchivePort {
                name: "curl".to_string(),
                product: "pkg:generic/curl@1.2.3".to_string(),
                git: "https://example.org/curl.git".to_string(),
                tag: "curl-8_23_0".to_string()
            }]
        );
        let url = "https://example.org/curl-8.23.0.tar.xz";
        let not_covered = |port: &str, reason: &str| NotCovered {
            port: port.to_string(),
            version: "1.2.3".to_string(),
            url: url.to_string(),
            reason: reason.to_string(),
        };
        assert_eq!(
            plan.not_covered,
            [
                not_covered("zip", "no git repository to ask by: no repository"),
                not_covered("bare", "the recipe has no [upstream]")
            ]
        );
        assert!(plan.commits.is_empty() && plan.crates.is_empty());
    }

    #[test]
    fn a_crate_a_recipe_lists_is_asked_by_its_name_and_version() {
        let crates = recipe(
            "rust-crates",
            "rust-src = true\n[[crate]]\nname = \"tokio\"\nversion = \"1.54.1\"\nsha256 = \"ab\"\n\
             license = \"MIT\"",
        );
        let plan = plan(&[crates]);
        assert_eq!(
            plan.crates,
            [Asked {
                component: "tokio 1.54.1".to_string(),
                product: "pkg:cargo/tokio@1.54.1".to_string(),
                location: "ports/rust-crates/recipe.toml".to_string(),
                query: Query::Crate { name: "tokio".to_string(), version: "1.54.1".to_string() },
            }]
        );
    }

    /// What `git ls-remote <git> refs/tags/<tag> refs/tags/<tag>^{}` printed
    /// for a tag of curl, which is annotated, and of jq, which is not.
    const ANNOTATED: &str = "4c24dd7ba7244e0206d91a72fe414823add8a034\trefs/tags/curl-8_23_0\n\
                             3163ec065643ba20709f190111f42c3e1fe95273\trefs/tags/curl-8_23_0^{}\n";
    const LIGHTWEIGHT: &str = "34f7186b86743a083a589741b6cea95293524108\trefs/tags/jq-1.8.2\n";

    #[test]
    fn the_commit_of_a_tag_is_the_peeled_one_of_an_annotated_tag_else_the_tags_own() {
        assert_eq!(
            tag_commit(ANNOTATED, "curl-8_23_0").as_deref(),
            Some("3163ec065643ba20709f190111f42c3e1fe95273")
        );
        // The peeled line first or last, as a server lists it.
        let lines: Vec<&str> = ANNOTATED.lines().rev().collect();
        let reversed = lines.join("\n");
        assert_eq!(tag_commit(&reversed, "curl-8_23_0"), tag_commit(ANNOTATED, "curl-8_23_0"));
        assert_eq!(
            tag_commit(LIGHTWEIGHT, "jq-1.8.2").as_deref(),
            Some("34f7186b86743a083a589741b6cea95293524108")
        );
        // The tag of another name, or none, is not found, though other
        // tags are listed.
        assert_eq!(tag_commit(ANNOTATED, "curl-8_22_0"), None);
        assert_eq!(tag_commit(ANNOTATED, "curl-8_23"), None);
        assert_eq!(tag_commit("", "jq-1.8.2"), None);
        // A line that is no ID and a name, or has a short or a non-hexadecimal ID, is no tag.
        let broken = "warning: redirecting\nabc\trefs/tags/jq-1.8.2\n\
                      zz6f7186b86743a083a589741b6cea95293524108\trefs/tags/jq-1.8.2\n\
                      34f7186b86743a083a589741b6cea95293524108 refs/tags/jq-1.8.2\n";
        assert_eq!(tag_commit(broken, "jq-1.8.2"), None);
        // A repository that hashes with SHA-256 has IDs of 64 digits.
        let long = format!("{}\trefs/tags/v1\n", "ab".repeat(32));
        assert_eq!(tag_commit(&long, "v1"), Some("ab".repeat(32)));
    }

    fn archive(name: &str, git: &str, tag: &str) -> ArchivePort {
        ArchivePort {
            name: name.to_string(),
            product: format!("pkg:generic/{name}@1"),
            git: git.to_string(),
            tag: tag.to_string(),
        }
    }

    #[test]
    fn each_archive_port_is_resolved_with_one_ls_remote_and_asked_by_the_commit_of_its_tag() {
        let ports = [
            archive("curl", "https://example.org/curl.git", "curl-8_23_0"),
            archive("jq", "https://example.org/jq.git", "jq-1.8.2"),
        ];
        let calls = std::cell::RefCell::new(Vec::new());
        let ls = |git: &str, tag: &str| -> Result<String> {
            calls.borrow_mut().push((git.to_string(), tag.to_string()));
            Ok(if tag == "jq-1.8.2" { LIGHTWEIGHT } else { ANNOTATED }.to_string())
        };
        let asked = resolve_archives(&ports, &ls).unwrap();
        assert_eq!(
            *calls.borrow(),
            [
                ("https://example.org/curl.git".to_string(), "curl-8_23_0".to_string()),
                ("https://example.org/jq.git".to_string(), "jq-1.8.2".to_string()),
            ]
        );
        let curl_commit = "3163ec065643ba20709f190111f42c3e1fe95273";
        assert_eq!(
            asked,
            [
                Asked {
                    component: format!("curl curl-8_23_0 {curl_commit}"),
                    product: "pkg:generic/curl@1".to_string(),
                    location: "ports/curl/recipe.toml".to_string(),
                    query: Query::Commit(curl_commit.to_string()),
                },
                Asked {
                    component: "jq jq-1.8.2 34f7186b86743a083a589741b6cea95293524108".to_string(),
                    product: "pkg:generic/jq@1".to_string(),
                    location: "ports/jq/recipe.toml".to_string(),
                    query: Query::Commit("34f7186b86743a083a589741b6cea95293524108".to_string()),
                },
            ]
        );
        assert!(resolve_archives(&[], &ls).unwrap().is_empty());
    }

    #[test]
    fn a_scan_that_could_not_resolve_a_tag_fails_and_names_each_port_it_could_not() {
        let ports = [
            archive("curl", "https://example.org/curl.git", "curl-8_23_0"),
            archive("gone", "https://example.org/gone.git", "v1"),
            archive("jq", "https://example.org/jq.git", "jq-1.9.0"),
            archive("down", "https://example.org/down.git", "v2"),
        ];
        let ls = |git: &str, _: &str| -> Result<String> {
            if git.ends_with("down.git") {
                return Err(Error::new("unable to access the repository"));
            }
            Ok(if git.ends_with("curl.git") { ANNOTATED } else { LIGHTWEIGHT }.to_string())
        };
        let err = resolve_archives(&ports, &ls).unwrap_err().to_string();
        // The tag that the repository does not have, and the repository
        // that cannot be reached, each by its port; not the ports that were found.
        assert!(err.contains("gone: https://example.org/gone.git has no tag v1"), "{err}");
        assert!(err.contains("jq: https://example.org/jq.git has no tag jq-1.9.0"), "{err}");
        assert!(err.contains("down: unable to access the repository"), "{err}");
        assert!(!err.contains("curl:"), "{err}");
    }

    const LOCK: &str = r#"
version = 4

[[package]]
name = "tokio"
version = "1.54.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ab"

[[package]]
name = "serde"
version = "1.0.230"
source = "sparse+https://index.crates.io/"

[[package]]
name = "dukovo-own"
version = "0.15.0"

[[package]]
name = "from-git"
version = "0.1.0"
source = "git+https://example.org/x.git?rev=ab#0123456789012345678901234567890123456789"

[[package]]
name = "elsewhere"
version = "2.0.0"
source = "registry+https://registry.example.org/index"
"#;

    #[test]
    fn the_crates_of_a_lock_are_the_packages_from_crates_io() {
        let crates = crates_in_lock(LOCK, "Cargo.lock").unwrap();
        let expected: BTreeSet<(String, String)> = [("tokio", "1.54.1"), ("serde", "1.0.230")]
            .into_iter()
            .map(|(name, version)| (name.to_string(), version.to_string()))
            .collect();
        assert_eq!(crates, expected);
        // A lock with no packages lists none, and a lock that is not TOML is an error.
        assert!(crates_in_lock("version = 4\n", "Cargo.lock").unwrap().is_empty());
        let err = crates_in_lock("not = [toml", "kernel/Cargo.lock").unwrap_err().to_string();
        assert!(err.contains("kernel/Cargo.lock"), "{err}");
    }

    fn crate_asked(port: &str, name: &str, version: &str) -> Asked {
        Asked {
            component: format!("{name} {version}"),
            product: format!("pkg:cargo/{name}@{version}"),
            location: format!("ports/{port}/recipe.toml"),
            query: Query::Crate { name: name.to_string(), version: version.to_string() },
        }
    }

    #[test]
    fn a_crate_a_lock_lists_is_not_asked_about_and_a_crate_two_ports_list_is_asked_once() {
        let locked = crates_in_lock(LOCK, "Cargo.lock").unwrap();
        let crates = vec![
            crate_asked("rust-crates", "tokio", "1.54.1"),
            crate_asked("rust-crates", "tokio", "1.40.0"),
            crate_asked("rust-crates", "nix", "0.31.3"),
            crate_asked("rust-std", "nix", "0.31.3"),
            crate_asked("rust-std", "serde", "1.0.230"),
        ];
        // Only the version the lock holds is left out; each other crate is
        // asked once, by the first port that lists it.
        assert_eq!(
            unlocked(crates, &locked),
            [
                crate_asked("rust-crates", "tokio", "1.40.0"),
                crate_asked("rust-crates", "nix", "0.31.3")
            ]
        );
        // A commit is not a crate: it is kept.
        let commit = Asked {
            component: "lua c1".to_string(),
            product: "pkg:generic/lua@1".to_string(),
            location: "ports/lua/recipe.toml".to_string(),
            query: Query::Commit("c1".to_string()),
        };
        assert_eq!(unlocked(vec![commit.clone()], &locked), [commit]);
    }

    #[test]
    fn a_query_is_a_commit_or_a_crate_of_crates_io_with_the_page_of_a_long_answer() {
        let commit = Query::Commit("c1".to_string());
        assert_eq!(commit.json(None), json!({"commit": "c1"}));
        assert_eq!(commit.json(Some("t1")), json!({"commit": "c1", "page_token": "t1"}));
        let krate = Query::Crate { name: "tokio".to_string(), version: "1.54.1".to_string() };
        assert_eq!(
            krate.json(None),
            json!({"package": {"ecosystem": "crates.io", "name": "tokio"}, "version": "1.54.1"})
        );
        assert_eq!(
            krate.json(Some("t2")),
            json!({
                "package": {"ecosystem": "crates.io", "name": "tokio"},
                "version": "1.54.1",
                "page_token": "t2"
            })
        );
    }

    fn pinned(name: &str, commit: &str) -> Asked {
        Asked {
            component: format!("{name} {commit}"),
            product: format!("pkg:generic/{name}@1"),
            location: format!("ports/{name}/recipe.toml"),
            query: Query::Commit(commit.to_string()),
        }
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
    fn osv_is_asked_once_for_everything_and_once_for_each_advisory() {
        let batch = json!({"results": [
            {"vulns": [{"id": "GHSA-1", "modified": "x"}, {"id": "CVE-2"}]},
            {},
            {"vulns": [{"id": "GHSA-1"}]},
            {"vulns": [{"id": "RUSTSEC-2026-1"}]}
        ]})
        .to_string();
        let ghsa = json!({
            "id": "GHSA-1", "summary": "Heap overflow", "aliases": ["CVE-1"],
            "database_specific": {"severity": "HIGH"}
        })
        .to_string();
        let cve =
            json!({"id": "CVE-2", "details": "\nFirst line of details.\nSecond."}).to_string();
        let rustsec = json!({"id": "RUSTSEC-2026-1", "summary": "Leak in tokio"}).to_string();
        let osv = FakeOsv::new(vec![
            ("/v1/querybatch", batch),
            ("/v1/vulns/GHSA-1", ghsa),
            ("/v1/vulns/CVE-2", cve),
            ("/v1/vulns/RUSTSEC-2026-1", rustsec),
        ]);
        let ports = [
            pinned("a", "c1"),
            pinned("b", "c2"),
            pinned("c", "c3"),
            crate_asked("rust-crates", "tokio", "1.54.1"),
        ];
        let found = osv_findings(&|path, body| osv.ask(path, body), &ports).unwrap();

        let asked = osv.asked.borrow();
        // One batch for the three commits and the crate, and one record for
        // each ID, though GHSA-1 matches two ports.
        assert_eq!(asked.len(), 4);
        let sent: Value = serde_json::from_str(asked[0].1.as_deref().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"queries": [
                {"commit": "c1"},
                {"commit": "c2"},
                {"commit": "c3"},
                {"package": {"ecosystem": "crates.io", "name": "tokio"}, "version": "1.54.1"}
            ]})
        );
        assert!(asked[1..].iter().all(|(_, body)| body.is_none()));

        assert_eq!(found.len(), 4);
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
        // A crate is named as cargo-audit names one, and has its recipe for its place.
        assert_eq!(found[3].id, "RUSTSEC-2026-1");
        assert_eq!(found[3].component, "tokio 1.54.1");
        assert_eq!(found[3].location, "ports/rust-crates/recipe.toml");
        assert_eq!(found[3].source, Source::Osv);
    }

    #[test]
    fn osv_pages_of_one_query_are_put_together() {
        let first = json!({"results": [
            {"vulns": [{"id": "A"}], "next_page_token": "t1"}, {}
        ]})
        .to_string();
        // The next page asks only for the commit that has one.
        let second = json!({"results": [{"vulns": [{"id": "B"}]}]}).to_string();
        let osv = FakeOsv::new(vec![("/v1/querybatch", first), ("/v1/querybatch", second)]);
        let queries = [Query::Commit("c1".to_string()), Query::Commit("c2".to_string())];
        let ids = osv_ids(&|path, body| osv.ask(path, body), &[&queries[0], &queries[1]]).unwrap();
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
        let c1 = Query::Commit("c1".to_string());
        assert!(osv_ids(&ask_with("<html>"), &[&c1]).is_err());
        // One result for each query asked.
        assert!(osv_ids(&ask_with(r#"{"results": []}"#), &[&c1]).is_err());
        assert!(osv_ids(&ask_with("{}"), &[&c1]).is_err());
        // A server that never ends its pages is not asked for ever.
        let endless = ask_with(r#"{"results": [{"next_page_token": "t"}]}"#);
        assert!(osv_ids(&endless, &[&c1]).is_err());
        // Nothing to ask is no question.
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
            product: format!("pkg:generic/{}@1", component.split(' ').next().unwrap_or_default()),
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
    fn the_json_is_the_findings_and_the_ports_not_covered_with_their_reasons() {
        let report = Report {
            findings: vec![finding("A", Some(Severity::High), "x 1"), finding("B", None, "y 2")],
            not_affected: vec![NotAffected {
                id: "CVE-3".to_string(),
                product: "pkg:generic/z@1".to_string(),
                component: "z c3".to_string(),
                status: "not_affected",
                justification: Some("vulnerable_code_not_present".to_string()),
                statement: "Not built.".to_string(),
            }],
            not_covered: vec![NotCovered {
                port: "zip".to_string(),
                version: "3.0".to_string(),
                url: "https://example.org/zip30.tar.gz".to_string(),
                reason: "no public repository".to_string(),
            }],
            unused_statements: vec![Unused {
                id: "CVE-4".to_string(),
                product: "pkg:generic/w@2".to_string(),
            }],
        };
        let value = serde_json::to_value(&report).unwrap();
        let first = &value["findings"][0];
        assert_eq!(first["id"], "A");
        assert_eq!(first["severity"], "high");
        assert_eq!(first["source"], "cargo-audit");
        assert_eq!(first["component"], "x 1");
        assert_eq!(first["url"], "https://example.org/A");
        assert_eq!(first["product"], "pkg:generic/x@1");
        assert!(first["aliases"].as_array().unwrap().is_empty());
        // No severity is null, which is what a reader that triages looks for.
        assert!(value["findings"][1]["severity"].is_null());
        assert_eq!(
            value["not_covered"],
            json!([{
                "port": "zip",
                "version": "3.0",
                "url": "https://example.org/zip30.tar.gz",
                "reason": "no public repository"
            }])
        );
        assert_eq!(
            value["not_affected"],
            json!([{
                "id": "CVE-3",
                "product": "pkg:generic/z@1",
                "component": "z c3",
                "status": "not_affected",
                "justification": "vulnerable_code_not_present",
                "statement": "Not built."
            }])
        );
        assert_eq!(
            value["unused_statements"],
            json!([{"id": "CVE-4", "product": "pkg:generic/w@2"}])
        );
        let mut osv = finding("C", None, "lua c1");
        osv.source = Source::Osv;
        assert_eq!(serde_json::to_value(&osv).unwrap()["source"], "osv");
    }

    /// The scan's parts as `scan` puts them together, with the lookups of
    /// tags and of advisories answered from tables: the commit of each
    /// kind of source reaches OSV in one batch, and what OSV says of it
    /// reaches the report under the recipe that pins it.
    #[test]
    fn the_scan_asks_osv_once_by_commit_tag_extra_source_and_crate() {
        let other = "0123456789abcdef0123456789abcdef01234567";
        let recipes = [
            lua(),
            recipe(
                "curl",
                &format!(
                    "{ARCHIVE}\n[upstream]\ngit = \"https://example.org/curl.git\"\n\
                     tag = \"curl-8_23_0\"\n[[extra]]\npath = \"pcre2\"\n\
                     git = \"https://example.org/pcre2.git\"\ncommit = \"{other}\"\n\
                     license = \"MIT\""
                ),
            ),
            recipe("zip", &format!("{ARCHIVE}\n[upstream]\nnone = \"no repository\"")),
            recipe(
                "rust-crates",
                "rust-src = true\n[[crate]]\nname = \"tokio\"\nversion = \"1.54.1\"\n\
                 sha256 = \"ab\"\nlicense = \"MIT\"\n[[crate]]\nname = \"nix\"\n\
                 version = \"0.31.3\"\nsha256 = \"ab\"\nlicense = \"MIT\"",
            ),
        ];
        let plan = plan(&recipes);
        let ls = |_: &str, _: &str| -> Result<String> { Ok(ANNOTATED.to_string()) };
        let mut asked = plan.commits;
        asked.extend(resolve_archives(&plan.archives, &ls).unwrap());
        // cargo-audit read a lock that holds tokio.
        let locked = crates_in_lock(LOCK, "Cargo.lock").unwrap();
        asked.extend(unlocked(plan.crates, &locked));

        let curl_commit = "3163ec065643ba20709f190111f42c3e1fe95273";
        let batch = json!({"results": [{}, {"vulns": [{"id": "CVE-7"}]}, {}, {}]}).to_string();
        let cve = json!({"id": "CVE-7", "summary": "Overflow in pcre2"}).to_string();
        let osv = FakeOsv::new(vec![("/v1/querybatch", batch), ("/v1/vulns/CVE-7", cve)]);
        let found = osv_findings(&|path, body| osv.ask(path, body), &asked).unwrap();

        let sent: Value =
            serde_json::from_str(osv.asked.borrow()[0].1.as_deref().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"queries": [
                {"commit": COMMIT},
                {"commit": other},
                {"commit": curl_commit},
                {"package": {"ecosystem": "crates.io", "name": "nix"}, "version": "0.31.3"}
            ]})
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].component, format!("curl pcre2 {other}"));
        assert_eq!(found[0].location, "ports/curl/recipe.toml");
        assert_eq!(found[0].product, format!("pkg:generic/pcre2@{other}"));

        let report = report(&vex::Document::default(), sorted(found), plan.not_covered);
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["findings"][0]["id"], "CVE-7");
        assert_eq!(value["findings"][0]["source"], "osv");
        assert_eq!(value["not_covered"][0]["port"], "zip");
        assert_eq!(value["not_covered"][0]["reason"], "no git repository to ask by: no repository");
        assert!(value["not_affected"].as_array().unwrap().is_empty());
    }

    /// The JSON of a statement document with one statement of `status` of
    /// `id` in `products`.
    fn statements(id: &str, status: &str, products: &[&str]) -> vex::Document {
        let products: Vec<Value> = products.iter().map(|p| json!({ "@id": p })).collect();
        let text = json!({
            "@context": "https://openvex.dev/ns/v0.2.0",
            "@id": "https://example.org/vex.json",
            "author": "a",
            "timestamp": "2026-11-15T00:00:00Z",
            "version": 1,
            "statements": [{
                "vulnerability": { "name": id },
                "products": products,
                "status": status,
                "status_notes": "Fixed before this release.",
                "action_statement": "Move on."
            }]
        });
        vex::parse(&text.to_string()).unwrap()
    }

    #[test]
    fn a_statement_moves_the_findings_it_clears_out_and_one_that_clears_none_is_listed() {
        let mut aliased = finding("GHSA-1", Some(Severity::High), "espeak-ng c1");
        aliased.aliases = vec!["CVE-1".to_string()];
        let elsewhere = finding("CVE-1", Some(Severity::High), "other c2");
        let unrelated = finding("CVE-2", None, "espeak-ng c1");
        let found = vec![aliased, elsewhere.clone(), unrelated.clone()];
        let fixed =
            statements("CVE-1", "fixed", &["pkg:generic/espeak-ng@1", "pkg:generic/espeak-ng@0.9"]);
        let cleared = report(&fixed, found.clone(), Vec::new());
        // By its alias, and only in the product the statement names.
        assert_eq!(cleared.findings, [elsewhere, unrelated]);
        assert_eq!(
            cleared.not_affected,
            [NotAffected {
                id: "GHSA-1".to_string(),
                product: "pkg:generic/espeak-ng@1".to_string(),
                component: "espeak-ng c1".to_string(),
                status: "fixed",
                justification: None,
                statement: "Fixed before this release.".to_string(),
            }]
        );
        assert_eq!(
            cleared.unused_statements,
            [Unused { id: "CVE-1".to_string(), product: "pkg:generic/espeak-ng@0.9".to_string() }]
        );
        // A statement that the part is affected clears nothing, and is not
        // listed as unused either.
        let affected = statements("CVE-1", "affected", &["pkg:generic/espeak-ng@1"]);
        let kept = report(&affected, found, Vec::new());
        assert_eq!(kept.findings.len(), 3);
        assert!(kept.not_affected.is_empty() && kept.unused_statements.is_empty());
    }

    #[test]
    fn a_port_that_takes_only_data_is_not_asked_about_and_says_why() {
        let locales: Recipe = toml::from_str(&format!(
            "name = \"locales\"\nversion = \"2.42\"\nsummary = \"s\"\nlicense = \"MIT\"\n\
             homepage = \"https://example.org\"\ndata_only = \"only the locale sources\"\n\
             [source]\ngit = \"https://example.org/glibc.git\"\ncommit = \"{COMMIT}\"\n"
        ))
        .unwrap();
        let plan = plan(&[locales, lua()]);
        assert_eq!(plan.commits.len(), 1);
        assert_eq!(plan.commits[0].product, "pkg:generic/lua@1.2.3");
        assert_eq!(
            plan.not_covered,
            [NotCovered {
                port: "locales".to_string(),
                version: "2.42".to_string(),
                url: "https://example.org/glibc.git".to_string(),
                reason: "only data is taken: only the locale sources".to_string(),
            }]
        );
    }

    #[test]
    fn the_text_lists_findings_and_what_was_not_covered() {
        let mut report = Report {
            findings: Vec::new(),
            not_affected: Vec::new(),
            not_covered: Vec::new(),
            unused_statements: Vec::new(),
        };
        let clean = render_text(&report);
        assert!(clean.starts_with("No advisory matches"), "{clean}");
        assert!(!clean.contains("Not asked about") && !clean.contains("statements"), "{clean}");

        let mut found = finding("A", Some(Severity::High), "x 1");
        found.aliases = vec!["CVE-1".to_string()];
        report.findings = vec![found, finding("B", None, "y 2")];
        report.not_covered = vec![NotCovered {
            port: "zip".to_string(),
            version: "3.0".to_string(),
            url: "u".to_string(),
            reason: "no public repository".to_string(),
        }];
        let text = render_text(&report);
        assert!(text.starts_with("2 findings:"), "{text}");
        assert!(text.contains("A  high  x 1"), "{text}");
        assert!(text.contains("B  no severity  y 2"), "{text}");
        assert!(text.contains("also CVE-1"), "{text}");
        assert!(text.contains("found in Cargo.lock by cargo-audit"), "{text}");
        assert!(text.contains("Not asked about, 1 ports:"), "{text}");
        assert!(text.contains("  zip 3.0: no public repository"), "{text}");

        report.not_affected = vec![NotAffected {
            id: "CVE-3".to_string(),
            product: "pkg:generic/z@1".to_string(),
            component: "z c3".to_string(),
            status: "fixed",
            justification: None,
            statement: "s".to_string(),
        }];
        report.unused_statements =
            vec![Unused { id: "CVE-4".to_string(), product: "pkg:generic/w@2".to_string() }];
        let text = render_text(&report);
        assert!(
            text.contains("1 findings are cleared by statements of security/vex.json:"),
            "{text}"
        );
        assert!(text.contains("  CVE-3 in pkg:generic/z@1: fixed"), "{text}");
        assert!(
            text.contains("clear no finding, which can go:\n  CVE-4 in pkg:generic/w@2"),
            "{text}"
        );
    }
}
