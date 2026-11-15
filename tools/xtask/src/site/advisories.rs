// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The support center's security advisories (ADR 0534): an index page, a
//! page for each advisory of `security/advisories`, newest first, and an
//! RSS 2.0 feed, `security/feed.xml`, for people. Machines read the same
//! files signed, through the update server.
//!
//! `ob security check` is what validates an advisory; the pages here read
//! what they show leniently and name the file when one cannot be read.
//! The feed's links are absolute, under [`SUPPORT_URL`], because a feed
//! reader has no site to resolve them against. A tree with no advisory
//! gets an index that says none has been published and a feed with no
//! item, which is still a valid feed.

use std::path::Path;

use serde_json::Value;

use super::layout::{self, Group};
use super::{Doc, read, write};
use crate::{Context, Ctx, Error, Result};

/// Where the support center is served, which the feed's links start with.
pub const SUPPORT_URL: &str = "https://dukovo.org/support/";

/// Where the advisories are in the tree.
const DIR: &str = crate::security::DIR;

/// The pages' folder on the site.
const FOLDER: &str = "security";

/// The feed's path on the site.
pub const FEED: &str = "security/feed.xml";

/// The kind search shows an advisory's page as.
pub const KIND: &str = "Advisory";

/// An advisory, as far as the pages show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    /// `DKSA-YYYY-NNNN`.
    pub id: String,
    pub summary: String,
    pub details: String,
    /// RFC 3339 in UTC.
    pub published: String,
    pub modified: String,
    /// `critical`, `high`, `medium` or `low`.
    pub severity: String,
    pub action: Option<String>,
    pub aliases: Vec<String>,
    /// (type, URL).
    pub references: Vec<(String, String)>,
    pub credits: Vec<String>,
    pub affected: Vec<Affected>,
}

/// A package an advisory affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Affected {
    /// `dukovo` for the system, or an app's ID.
    pub package: String,
    /// What each range says, in words.
    pub ranges: Vec<String>,
    /// The versions that fix it, in the order the advisory gives them.
    pub fixed: Vec<String>,
}

/// What the advisories add to the site.
pub struct Pages {
    /// The index and the page of each advisory.
    pub docs: Vec<Doc>,
    /// The group of the contents.
    pub group: Group,
    /// The feed's text.
    pub feed: String,
}

/// Reads the advisories of the tree and makes their pages and feed.
pub fn build(ctx: &Ctx) -> Result<Pages> {
    let advisories = read_all(ctx)?;
    Ok(pages(&advisories))
}

/// Writes the feed, which is not a page.
pub fn write_feed(out: &Path, feed: &str) -> Result<()> {
    write(&out.join(FEED), feed)
}

/// Every `DKSA-*.json` of the tree, newest first.
fn read_all(ctx: &Ctx) -> Result<Vec<Advisory>> {
    let dir = ctx.root.join(DIR);
    let mut advisories = Vec::new();
    if dir.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .context(|| format!("reading {DIR}"))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                name.starts_with("DKSA-")
                    && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .collect();
        files.sort();
        for file in files {
            let relative = format!("{DIR}/{}", file.file_name().unwrap().to_string_lossy());
            let text = read(ctx, Path::new(&relative))?;
            advisories.push(parse(&text).map_err(|why| Error::new(format!("{relative}: {why}")))?);
        }
    }
    newest_first(&mut advisories);
    Ok(advisories)
}

/// By time of publication, the newest first, and by id within a time.
fn newest_first(advisories: &mut [Advisory]) {
    advisories.sort_by(|a, b| b.published.cmp(&a.published).then_with(|| b.id.cmp(&a.id)));
}

/// An advisory from its OSV text.
///
/// # Errors
///
/// What is missing or of the wrong kind, as `ob security check` says it in
/// full.
pub fn parse(text: &str) -> std::result::Result<Advisory, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let string = |value: &Value, field: &str| -> std::result::Result<String, String> {
        value
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("has no {field}"))
    };
    let strings = |field: &str| -> Vec<String> {
        value.get(field).and_then(Value::as_array).map_or_else(Vec::new, |list| {
            list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        })
    };
    let database = value.get("database_specific");
    let severity = database
        .and_then(|d| d.get("severity"))
        .and_then(Value::as_str)
        .ok_or("has no database_specific.severity")?
        .to_string();
    let references =
        value.get("references").and_then(Value::as_array).map_or_else(Vec::new, |list| {
            list.iter()
                .filter_map(|r| {
                    Some((
                        r.get("type")?.as_str()?.to_string(),
                        r.get("url")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        });
    let credits = value.get("credits").and_then(Value::as_array).map_or_else(Vec::new, |list| {
        list.iter().filter_map(|c| c.get("name")?.as_str().map(str::to_string)).collect()
    });
    let affected = value
        .get("affected")
        .and_then(Value::as_array)
        .ok_or("has no affected")?
        .iter()
        .map(affected_of)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Advisory {
        id: string(&value, "id")?,
        summary: string(&value, "summary")?,
        details: string(&value, "details")?,
        published: string(&value, "published")?,
        modified: string(&value, "modified")?,
        severity,
        action: database.and_then(|d| d.get("action")).and_then(Value::as_str).map(str::to_string),
        aliases: strings("aliases"),
        references,
        credits,
        affected,
    })
}

/// One `affected` entry.
fn affected_of(entry: &Value) -> std::result::Result<Affected, String> {
    let package = entry
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .ok_or("an affected entry has no package name")?
        .to_string();
    let mut ranges = Vec::new();
    let mut fixed = Vec::new();
    for range in entry.get("ranges").and_then(Value::as_array).into_iter().flatten() {
        let events: Vec<(&str, &str)> = range
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|event| {
                let (kind, version) = event.as_object()?.iter().next()?;
                Some((kind.as_str(), version.as_str()?))
            })
            .collect();
        fixed
            .extend(events.iter().filter(|(kind, _)| *kind == "fixed").map(|(_, v)| v.to_string()));
        ranges.push(describe_range(&events));
    }
    fixed.dedup();
    Ok(Affected { package, ranges, fixed })
}

/// A range's events in words: `1.4.0 up to before 1.4.2`, or `every version
/// before 1.4.2` for one that starts at 0. A range that never ends says so.
fn describe_range(events: &[(&str, &str)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut from: Option<&str> = None;
    for (kind, version) in events {
        match *kind {
            "introduced" => from = Some(version),
            "fixed" | "last_affected" => {
                let end = if *kind == "fixed" {
                    format!("before {version}")
                } else {
                    format!("up to and including {version}")
                };
                parts.push(match from.take() {
                    Some("0") | None => format!("every version {end}"),
                    Some(start) => format!("from {start}, {end}"),
                });
            }
            _ => {}
        }
    }
    if let Some(start) = from {
        parts.push(if start == "0" {
            "every version, with no fix yet".into()
        } else {
            format!("from {start}, with no fix yet")
        });
    }
    parts.join("; ")
}

/// How a severity is written for people.
fn severity_word(severity: &str) -> String {
    let mut chars = severity.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The system's name in prose, or the app's ID.
fn package_name(package: &str) -> String {
    if package == "dukovo" { "Dukovo (the system)".into() } else { package.to_string() }
}

/// Where an alias is described, when its kind is known.
fn alias_url(alias: &str) -> Option<String> {
    if alias.starts_with("CVE-") {
        Some(format!("https://nvd.nist.gov/vuln/detail/{alias}"))
    } else if alias.starts_with("GHSA-") {
        Some(format!("https://github.com/advisories/{alias}"))
    } else if alias.starts_with("RUSTSEC-") {
        Some(format!("https://rustsec.org/advisories/{alias}"))
    } else {
        None
    }
}

/// `text` as paragraphs, with `code` in backticks as code.
fn paragraphs(text: &str) -> String {
    let mut html = String::new();
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        html.push_str("<p>");
        for (n, part) in paragraph.split('`').enumerate() {
            if n % 2 == 1 {
                html.push_str(&format!("<code>{}</code>", layout::escape(part)));
            } else {
                html.push_str(&layout::escape(part));
            }
        }
        html.push_str("</p>");
    }
    html
}

/// A link when `url` is an http(s) address, and plain text otherwise.
fn link(url: &str, text: &str) -> String {
    if url.starts_with("https://") || url.starts_with("http://") {
        format!("<a href=\"{}\">{}</a>", layout::escape(url), layout::escape(text))
    } else {
        layout::escape(text)
    }
}

/// The date of an RFC 3339 time, `2026-11-10`.
fn date(time: &str) -> &str {
    time.split('T').next().unwrap_or(time)
}

/// The page of `advisory`, `security/DKSA-YYYY-NNNN.html`.
fn doc(advisory: &Advisory) -> Doc {
    let mut doc = Doc::new(format!("{FOLDER}/{}.html", advisory.id), advisory.id.clone(), KIND);
    doc.lead.clone_from(&advisory.summary);
    doc.summary = format!("{} severity: {}", severity_word(&advisory.severity), advisory.summary);
    doc.badges = vec![
        format!("{} severity", severity_word(&advisory.severity)),
        format!("Published {}", date(&advisory.published)),
    ];
    if advisory.modified != advisory.published {
        doc.badges.push(format!("Updated {}", date(&advisory.modified)));
    }
    doc.source = Some(format!("{DIR}/{}.json", advisory.id));
    doc.aliases.clone_from(&advisory.aliases);

    let mut body = String::new();
    let mut section = |id: &str, title: &str, html: &str| {
        body.push_str(&format!("<h2 id=\"{id}\">{title}</h2>{html}"));
        doc.headings.push((id.to_string(), title.to_string()));
    };
    section("details", "Details", &paragraphs(&advisory.details));
    let mut affected = String::from("<ul>");
    for entry in &advisory.affected {
        affected.push_str(&format!(
            "<li><b>{}</b>: {}</li>",
            layout::escape(&package_name(&entry.package)),
            layout::escape(&entry.ranges.join("; "))
        ));
    }
    affected.push_str("</ul>");
    section("affected", "Affected versions", &affected);
    if advisory.affected.iter().any(|entry| !entry.fixed.is_empty()) {
        let mut fixed = String::from("<ul>");
        for entry in advisory.affected.iter().filter(|entry| !entry.fixed.is_empty()) {
            fixed.push_str(&format!(
                "<li><b>{}</b>: {}</li>",
                layout::escape(&package_name(&entry.package)),
                layout::escape(&entry.fixed.join(", "))
            ));
        }
        fixed.push_str("</ul>");
        section("fixed", "Fixed in", &fixed);
    }
    if let Some(action) = &advisory.action {
        section("action", "What to do", &paragraphs(action));
    }
    if !advisory.aliases.is_empty() {
        let mut list = String::from("<ul>");
        for alias in &advisory.aliases {
            let item =
                alias_url(alias).map_or_else(|| layout::escape(alias), |url| link(&url, alias));
            list.push_str(&format!("<li>{item}</li>"));
        }
        list.push_str("</ul>");
        section("aliases", "Also known as", &list);
    }
    if !advisory.references.is_empty() {
        let mut list = String::from("<ul>");
        for (kind, url) in &advisory.references {
            list.push_str(&format!(
                "<li>{}: {}</li>",
                layout::escape(&kind.to_lowercase()),
                link(url, url)
            ));
        }
        list.push_str("</ul>");
        section("references", "References", &list);
    }
    if !advisory.credits.is_empty() {
        let names: Vec<String> = advisory.credits.iter().map(|c| layout::escape(c)).collect();
        section("credits", "Credits", &format!("<p>{}</p>", names.join(", ")));
    }
    doc.body = body;
    doc
}

/// The index page: how to follow the advisories, and each one, newest
/// first.
fn index(advisories: &[Advisory]) -> Doc {
    let mut doc =
        Doc::new(format!("{FOLDER}/index.html"), "Security advisories".into(), "Security");
    doc.lead = "The flaws in Dukovo and its apps that have been fixed, which releases have them, \
                and what to do."
        .into();
    doc.headings =
        vec![("follow".into(), "Follow them".into()), ("advisories".into(), "Advisories".into())];
    let mut body = String::from(
        "<h2 id=\"follow\">Follow them</h2>\
         <p>Subscribe to the <a href=\"feed.xml\">RSS feed</a> in a feed reader. A machine reads \
         the same advisories signed by the update server: <code>update advisories</code> says \
         which of them apply to it and what to do, and sends nothing about the machine. Each is \
         an <a href=\"https://ossf.github.io/osv-schema/\">OSV</a> file, so that OSV scanners \
         read it as it is. To report a flaw, see <a \
         href=\"https://github.com/dukovo-os/dukovo/blob/main/SECURITY.md\">SECURITY.md</a>.</p>\
         <h2 id=\"advisories\">Advisories</h2>",
    );
    if advisories.is_empty() {
        body.push_str("<p>No security advisory has been published yet.</p>");
    } else {
        body.push_str(
            "<table><thead><tr><th>Advisory</th><th>Severity</th><th>Published</th>\
             <th>What is wrong</th></tr></thead><tbody>",
        );
        for advisory in advisories {
            body.push_str(&format!(
                "<tr><td><a href=\"{id}.html\">{id}</a></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                layout::escape(&severity_word(&advisory.severity)),
                layout::escape(date(&advisory.published)),
                layout::escape(&advisory.summary),
                id = layout::escape(&advisory.id),
            ));
        }
        body.push_str("</tbody></table>");
    }
    doc.body = body;
    doc
}

/// An RFC 822 date, as RSS has them, from an RFC 3339 time in UTC; none
/// for one that is not of that form.
fn rfc822(time: &str) -> Option<String> {
    let (day, clock) = time.strip_suffix('Z')?.split_once('T')?;
    let mut parts = day.split('-').map(str::parse::<i64>);
    let (year, month, date) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&date) {
        return None;
    }
    // Howard Hinnant's days_from_civil, whose day zero is 1970-01-01, a Thursday.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + date - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
        [usize::try_from(days.rem_euclid(7)).ok()?];
    let name = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        [usize::try_from(month - 1).ok()?];
    Some(format!("{weekday}, {date:02} {name} {year:04} {clock} +0000"))
}

/// Escapes text for XML.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The feed of `advisories`, newest first: RSS 2.0, with absolute links.
/// With none, it has no item and says so.
fn feed(advisories: &[Advisory]) -> String {
    let home = format!("{SUPPORT_URL}{FOLDER}/index.html");
    let description = if advisories.is_empty() {
        "No security advisory has been published yet.".to_string()
    } else {
        "The flaws in Dukovo and its apps that have been fixed, newest first.".to_string()
    };
    let mut text = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\">\n\
         <channel>\n\
         <title>Dukovo security advisories</title>\n\
         <link>{}</link>\n\
         <description>{}</description>\n\
         <language>en</language>\n\
         <atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>\n",
        xml(&home),
        xml(&description),
        xml(&format!("{SUPPORT_URL}{FEED}")),
    );
    // The newest change, not the build time, so that the same advisories
    // make the same feed.
    let newest = advisories.iter().map(|a| &a.modified).max();
    if let Some(date) = newest.and_then(|time| rfc822(time)) {
        text.push_str(&format!("<lastBuildDate>{date}</lastBuildDate>\n"));
    }
    for advisory in advisories {
        let url = format!("{SUPPORT_URL}{FOLDER}/{}.html", advisory.id);
        let affected: Vec<String> = advisory
            .affected
            .iter()
            .map(|entry| format!("{}: {}", package_name(&entry.package), entry.ranges.join("; ")))
            .collect();
        let mut said = format!(
            "{}. Affected: {}.",
            advisory.summary.trim_end_matches('.'),
            affected.join("; ")
        );
        for entry in advisory.affected.iter().filter(|entry| !entry.fixed.is_empty()) {
            said.push_str(&format!(
                " Fixed in {}: {}.",
                package_name(&entry.package),
                entry.fixed.join(", ")
            ));
        }
        if let Some(action) = &advisory.action {
            said.push(' ');
            said.push_str(action);
        }
        text.push_str(&format!(
            "<item>\n<title>{}</title>\n<link>{}</link>\n<guid isPermaLink=\"true\">{}</guid>\n",
            xml(&format!(
                "{} ({}): {}",
                advisory.id,
                severity_word(&advisory.severity),
                advisory.summary
            )),
            xml(&url),
            xml(&url),
        ));
        if let Some(date) = rfc822(&advisory.published) {
            text.push_str(&format!("<pubDate>{date}</pubDate>\n"));
        }
        text.push_str(&format!(
            "<category>{}</category>\n<description>{}</description>\n</item>\n",
            xml(&advisory.severity),
            xml(&said)
        ));
    }
    text.push_str("</channel>\n</rss>\n");
    text
}

/// The pages, the contents group and the feed for `advisories`, which are
/// in any order.
pub fn pages(advisories: &[Advisory]) -> Pages {
    let mut sorted = advisories.to_vec();
    newest_first(&mut sorted);
    let mut docs = vec![index(&sorted)];
    docs.extend(sorted.iter().map(doc));
    let group = Group {
        title: "Security advisories".into(),
        links: vec![
            ("All advisories".into(), format!("{FOLDER}/index.html")),
            ("RSS feed".into(), FEED.into()),
        ],
    };
    Pages { docs, group, feed: feed(&sorted) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!(
        "../../../../userspace/programs/system-release/tests/fixtures/DKSA-2026-0001.json"
    );

    fn fixture() -> Advisory {
        parse(FIXTURE).unwrap()
    }

    fn later() -> Advisory {
        Advisory {
            id: "DKSA-2026-0002".into(),
            summary: "A later flaw & its <fix>".into(),
            published: "2026-11-12T08:00:00Z".into(),
            modified: "2026-11-14T08:00:00Z".into(),
            severity: "critical".into(),
            action: None,
            aliases: Vec::new(),
            ..fixture()
        }
    }

    /// Whether `xml` is well formed as far as nesting goes: each element
    /// closed by its name, none left open.
    fn nests(xml: &str) -> bool {
        let mut open: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(at) = rest.find('<') {
            rest = &rest[at + 1..];
            let Some(end) = rest.find('>') else { return false };
            let tag = &rest[..end];
            rest = &rest[end + 1..];
            if tag.starts_with('?') || tag.ends_with('/') {
                continue;
            }
            let name = tag.trim_start_matches('/').split_whitespace().next().unwrap_or_default();
            if tag.starts_with('/') {
                if open.pop().as_deref() != Some(name) {
                    return false;
                }
            } else {
                open.push(name.to_string());
            }
        }
        open.is_empty()
    }

    #[test]
    fn an_advisory_is_read_from_its_osv_file() {
        let advisory = fixture();
        assert_eq!(advisory.id, "DKSA-2026-0001");
        assert_eq!(advisory.severity, "high");
        assert_eq!(advisory.aliases, ["CVE-2026-12345", "GHSA-xxxx-xxxx-xxxx"]);
        assert_eq!(advisory.credits, ["Example Reporter"]);
        assert_eq!(advisory.references.len(), 2);
        assert_eq!(
            advisory.action.as_deref(),
            Some("Update to Dukovo 1.4.2 with `update apply`, then restart.")
        );
        assert_eq!(advisory.affected.len(), 2);
        assert_eq!(advisory.affected[0].package, "dukovo");
        assert_eq!(
            advisory.affected[0].ranges,
            ["from 1.4.0, before 1.4.2", "from 1.3.5, before 1.3.9.1"]
        );
        assert_eq!(advisory.affected[0].fixed, ["1.4.2", "1.3.9.1"]);
        assert_eq!(advisory.affected[1].ranges, ["every version before 14.1.1"]);
        assert!(parse("{").unwrap_err().starts_with("not JSON"));
        assert_eq!(parse("{}").unwrap_err(), "has no database_specific.severity");
    }

    #[test]
    fn a_range_is_said_in_words() {
        assert_eq!(
            describe_range(&[("introduced", "0"), ("fixed", "1.2.0")]),
            "every version before 1.2.0"
        );
        assert_eq!(
            describe_range(&[("introduced", "1.0.0"), ("last_affected", "1.1.0")]),
            "from 1.0.0, up to and including 1.1.0"
        );
        assert_eq!(describe_range(&[("introduced", "1.0.0")]), "from 1.0.0, with no fix yet");
        assert_eq!(describe_range(&[("introduced", "0")]), "every version, with no fix yet");
        assert_eq!(
            describe_range(&[
                ("introduced", "1.0.0"),
                ("fixed", "1.1.0"),
                ("introduced", "2.0.0"),
                ("fixed", "2.0.3")
            ]),
            "from 1.0.0, before 1.1.0; from 2.0.0, before 2.0.3"
        );
    }

    #[test]
    fn the_index_lists_the_newest_first_with_severity_and_summary() {
        let built = pages(&[fixture(), later()]);
        let index = &built.docs[0];
        assert_eq!(index.path, "security/index.html");
        let first = index.body.find("DKSA-2026-0002.html").unwrap();
        let second = index.body.find("DKSA-2026-0001.html").unwrap();
        assert!(first < second, "the newest comes first");
        assert!(index.body.contains("<td>Critical</td>"));
        assert!(index.body.contains("<td>2026-11-12</td>"));
        assert!(index.body.contains("A later flaw &amp; its &lt;fix&gt;"));
        assert!(index.body.contains("href=\"feed.xml\""));
        assert!(!index.body.contains("No security advisory"));
        // A page for each, and the group links the index and the feed.
        let paths: Vec<&str> = built.docs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(
            paths,
            ["security/index.html", "security/DKSA-2026-0002.html", "security/DKSA-2026-0001.html"]
        );
        let links: Vec<&str> = built.group.links.iter().map(|(_, path)| path.as_str()).collect();
        assert_eq!(links, ["security/index.html", "security/feed.xml"]);
    }

    #[test]
    fn a_page_has_severity_affected_and_fixed_versions_aliases_with_links_and_the_action() {
        let page = doc(&fixture());
        assert_eq!(page.path, "security/DKSA-2026-0001.html");
        assert_eq!(page.title, "DKSA-2026-0001");
        assert_eq!(page.kind, KIND);
        assert_eq!(page.badges, ["High severity", "Published 2026-11-10", "Updated 2026-11-15"]);
        assert_eq!(page.source.as_deref(), Some("security/advisories/DKSA-2026-0001.json"));
        let ids: Vec<&str> = page.headings.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["details", "affected", "fixed", "action", "aliases", "references", "credits"]
        );
        for needle in [
            "<b>Dukovo (the system)</b>: from 1.4.0, before 1.4.2; from 1.3.5, before 1.3.9.1",
            "<b>org.dukovo.ripgrep</b>: every version before 14.1.1",
            "<b>Dukovo (the system)</b>: 1.4.2, 1.3.9.1",
            "<code>update apply</code>",
            "<a href=\"https://nvd.nist.gov/vuln/detail/CVE-2026-12345\">CVE-2026-12345</a>",
            "<a href=\"https://github.com/advisories/GHSA-xxxx-xxxx-xxxx\">GHSA-xxxx-xxxx-xxxx</a>",
            "advisory: <a href=\"https://example.org/advisories/CVE-2026-12345\">",
            "Example Reporter",
        ] {
            assert!(page.body.contains(needle), "{needle}\n{}", page.body);
        }
        // Search finds it by an alias as well as by its id.
        assert_eq!(page.aliases, ["CVE-2026-12345", "GHSA-xxxx-xxxx-xxxx"]);
    }

    #[test]
    fn no_advisory_says_so_and_the_feed_is_still_a_feed() {
        let built = pages(&[]);
        assert_eq!(built.docs.len(), 1);
        assert!(built.docs[0].body.contains("No security advisory has been published yet."));
        assert!(!built.docs[0].body.contains("<table"));
        assert!(
            built
                .feed
                .starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\"")
        );
        assert!(
            built.feed.contains(
                "<description>No security advisory has been published yet.</description>"
            )
        );
        assert!(!built.feed.contains("<item>"));
        assert!(!built.feed.contains("lastBuildDate"));
        assert!(nests(&built.feed), "{}", built.feed);
    }

    #[test]
    fn the_feed_has_an_item_for_each_with_absolute_links_and_a_date() {
        let built = pages(&[fixture(), later()]);
        let feed = &built.feed;
        assert!(nests(feed), "{feed}");
        assert_eq!(feed.matches("<item>").count(), 2);
        // Newest first.
        assert!(feed.find("DKSA-2026-0002").unwrap() < feed.find("DKSA-2026-0001").unwrap());
        for line in feed.lines().filter(|l| l.starts_with("<link>") || l.starts_with("<guid")) {
            assert!(line.contains(SUPPORT_URL), "{line}");
        }
        assert!(
            feed.contains("<link>https://dukovo.org/support/security/DKSA-2026-0001.html</link>")
        );
        assert!(feed.contains(
            "<atom:link href=\"https://dukovo.org/support/security/feed.xml\" rel=\"self\" type=\"application/rss+xml\"/>"
        ));
        assert!(feed.contains("<pubDate>Tue, 10 Nov 2026 09:00:00 +0000</pubDate>"));
        // The newest change, not the build time, so the same advisories give the same feed.
        assert!(feed.contains("<lastBuildDate>Sun, 15 Nov 2026 09:30:00 +0000</lastBuildDate>"));
        assert!(feed.contains("A later flaw &amp; its &lt;fix&gt;"));
        assert!(feed.contains("<category>critical</category>"));
        assert!(feed.contains("Fixed in Dukovo (the system): 1.4.2, 1.3.9.1."));
        assert_eq!(pages(&[later(), fixture()]).feed, *feed);
    }

    #[test]
    fn dates_are_rfc_822_with_their_weekday() {
        assert_eq!(
            rfc822("1970-01-01T00:00:00Z").as_deref(),
            Some("Thu, 01 Jan 1970 00:00:00 +0000")
        );
        assert_eq!(
            rfc822("2024-02-29T12:30:45Z").as_deref(),
            Some("Thu, 29 Feb 2024 12:30:45 +0000")
        );
        assert_eq!(
            rfc822("2026-12-31T23:59:59Z").as_deref(),
            Some("Thu, 31 Dec 2026 23:59:59 +0000")
        );
        assert_eq!(rfc822("2026-13-01T00:00:00Z"), None);
        assert_eq!(rfc822("yesterday"), None);
        assert_eq!(rfc822("2026-11-10T09:00:00+01:00"), None);
    }
}
