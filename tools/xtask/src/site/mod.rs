// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob docs site`: the support center, built from the docs in the tree
//! (ADR 0419).
//!
//! Every page comes from a file the code's own pull requests change: the
//! man pages (docs/man and those made from abi/*.ridl), the handbook, the
//! design notes, the decision records and the roadmap, plus the guides,
//! tutorials, runbooks and error pages once their folders exist. The
//! result is a static site in out/docs/site that dukovo.org/support serves.

pub mod check;
mod layout;
mod man;
mod markdown;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result, docs};
use layout::{Group, Shell};

/// Folders of Markdown pages that are each a group in the contents, with
/// the kind search shows for them. A folder that does not exist yet is
/// left out.
const COLLECTIONS: &[(&str, &str, &str)] = &[
    ("docs/tutorials", "Tutorials", "Tutorial"),
    ("docs/guides", "Guides", "Guide"),
    ("docs/runbooks", "Troubleshoot", "Runbook"),
    ("docs/errors", "Error messages", "Error"),
];

/// The man page sections, in the order the command index lists them.
const SECTIONS: &[(&str, &str, &str)] = &[
    ("1", "commands", "Commands"),
    ("8", "administration", "Administration commands"),
    ("5", "file-formats", "File formats and settings"),
    ("7", "overviews", "Overviews"),
    ("4", "drivers", "Drivers"),
    ("2", "system-calls", "System calls"),
    ("3", "library", "Library functions"),
    ("6", "games", "Games"),
    ("9", "kernel", "Kernel interfaces"),
];

/// A page waiting to be written.
struct Doc {
    /// Where it goes, from the site's root.
    path: String,
    title: String,
    lead: String,
    /// A sentence for search results and link previews.
    summary: String,
    /// What search shows the page as: Command, Handbook, Guide...
    kind: &'static str,
    badges: Vec<String>,
    body: String,
    headings: Vec<(String, String)>,
    /// The file it is made from, from the repository's root.
    source: Option<String>,
    note: String,
    class: &'static str,
    /// Other names search finds the page by, such as a command's aliases.
    aliases: Vec<String>,
}

impl Doc {
    fn new(path: String, title: String, kind: &'static str) -> Self {
        Self {
            path,
            title,
            lead: String::new(),
            summary: String::new(),
            kind,
            badges: Vec::new(),
            body: String::new(),
            headings: Vec::new(),
            source: None,
            note: String::new(),
            class: "",
            aliases: Vec::new(),
        }
    }
}

/// `ob docs site [--check] [--version <name>]`.
pub fn build(ctx: &Ctx, version: Option<&str>, check: bool) -> Result<()> {
    for tool in ["scdoc", "mandoc"] {
        if process::find_program(tool).is_none() {
            return Err(Error::new(format!("{tool} is not installed"))
                .hint("run `./ob doctor` to see what is missing"));
        }
    }
    let version = version.unwrap_or(crate::VERSION);
    let out = ctx.out.join("docs/site");
    if out.exists() {
        std::fs::remove_dir_all(&out).context(|| format!("emptying {}", out.display()))?;
    }

    let mut docs = Vec::new();
    // Source path (from the repository's root) to site path, for links.
    let mut sites: BTreeMap<String, String> = BTreeMap::new();
    let mut groups = Vec::new();

    let man_pages = man_pages(ctx, &mut docs)?;
    for doc in &docs {
        if let Some(source) = &doc.source {
            sites.insert(source.clone(), doc.path.clone());
        }
    }
    let handbook = handbook(ctx, &mut sites)?;
    let design = folder(ctx, "docs/design", "design", &mut sites)?;
    let adrs = folder(ctx, "docs/adr", "adr", &mut sites)?;
    let roadmap = ctx.root.join("docs/roadmap.md");
    sites.insert("docs/roadmap.md".into(), "roadmap.html".into());
    let mut collections = Vec::new();
    for &(dir, title, kind) in COLLECTIONS {
        let files = folder(ctx, dir, &dir["docs/".len()..], &mut sites)?;
        if !files.is_empty() {
            collections.push((title, kind, files));
        }
    }

    // Handbook chapters, in the handbook's own order and parts.
    for (part, chapters) in &handbook {
        let mut links = Vec::new();
        for path in chapters {
            let doc = markdown_doc(ctx, path, "Handbook", &sites)?;
            links.push((doc.title.clone(), doc.path.clone()));
            docs.push(doc);
        }
        groups.push(Group { title: part.clone(), links });
    }
    for (title, kind, files) in &collections {
        let mut links = Vec::new();
        for path in files {
            let doc = markdown_doc(ctx, path, kind, &sites)?;
            links.push((doc.title.clone(), doc.path.clone()));
            docs.push(doc);
        }
        groups.push(Group { title: title.to_string(), links });
    }

    let mut reference = Vec::new();
    for &(section, id, title) in SECTIONS {
        if man_pages.iter().any(|(_, s, _)| s.starts_with(section)) {
            reference.push((title.to_string(), format!("man/index.html#{id}")));
        }
    }
    groups.push(Group { title: "Reference".into(), links: reference });
    docs.push(man_index(&man_pages));

    let mut how = Vec::new();
    for path in &design {
        let doc = markdown_doc(ctx, path, "How it works", &sites)?;
        how.push((doc.title.clone(), doc.path.clone()));
        docs.push(doc);
    }
    let mut records = Vec::new();
    for path in &adrs {
        let doc = markdown_doc(ctx, path, "Design record", &sites)?;
        records.push((doc.path.clone(), doc.title.clone(), status(&read(ctx, path)?)));
        docs.push(doc);
    }
    how.push(("Design records (ADRs)".into(), "adr/index.html".into()));
    groups.push(Group { title: "How it works".into(), links: how });
    docs.push(adr_index(&records));

    let mut road = markdown_doc(ctx, &roadmap, "Project", &sites)?;
    road.path = "roadmap.html".into();
    docs.push(road);
    groups.push(Group {
        title: "Project".into(),
        links: vec![("Roadmap".into(), "roadmap.html".into())],
    });

    step(&format!("Writing {} pages to {}", docs.len() + 1, ctx.display(&out)));
    let shell = Shell { version, groups: &groups };
    for doc in &docs {
        let html = layout::page(
            &shell,
            &layout::Page {
                path: &doc.path,
                title: &doc.title,
                lead: &doc.lead,
                description: if doc.summary.is_empty() { &doc.lead } else { &doc.summary },
                badges: &doc.badges,
                body: &doc.body,
                headings: &doc.headings,
                source: doc.source.as_deref(),
                note: &doc.note,
                class: doc.class,
            },
        );
        write(&out.join(&doc.path), &html)?;
    }
    let home = read(ctx, Path::new("docs/site/home.html"))?;
    let home = home.split_once("-->\n").map_or(home.as_str(), |(_, body)| body);
    write(
        &out.join("index.html"),
        &layout::home(
            &shell,
            "Dukovo Support",
            "Guides, every command, how Dukovo works inside, and how to build for it.",
            home,
        ),
    )?;
    write(&out.join("search-index.js"), &search_index(&docs))?;
    for asset in ["site.css", "site.js"] {
        let from = ctx.root.join("docs/site").join(asset);
        std::fs::copy(&from, out.join(asset)).context(|| format!("copying {}", from.display()))?;
    }
    if check {
        step("Checking the support center's links and search (ADR 0420)");
        let mut problems = check::link_problems(&out)?;
        problems.extend(check::search_problems(&index_entries(&docs), &wanted_searches(&docs)));
        if !problems.is_empty() {
            for problem in problems.iter().take(50) {
                eprintln!("    {problem}");
            }
            if problems.len() > 50 {
                eprintln!("    and {} more", problems.len() - 50);
            }
            crate::bail!("{} support center problem(s)", problems.len());
        }
    }
    step(&format!("Support center ready: {}/index.html", ctx.display(&out)));
    Ok(())
}

/// Compiles every man page and turns each into a page under man/.
/// Returns (name, section, summary) of each, for the index.
fn man_pages(ctx: &Ctx, docs: &mut Vec<Doc>) -> Result<Vec<(String, String, String)>> {
    let pages = docs::all_pages(ctx)?;
    let man_dir = ctx.out.join("docs/man");
    let names: BTreeSet<String> =
        pages.iter().map(|p| format!("{}.{}", p.name, p.section)).collect();
    let mut index = Vec::new();
    for page in &pages {
        let compiled = page.output(&man_dir);
        docs::compile(ctx, page, &compiled)?;
        let mut cmd = Command::new("mandoc");
        cmd.args(["-T", "html", "-O", "fragment"]).arg(&compiled);
        let fragment =
            String::from_utf8_lossy(&process::output(ctx, &mut cmd)?.stdout).into_owned();
        let html = man::tidy(&fragment, &names);
        let scdoc = std::fs::read_to_string(&page.source)
            .context(|| format!("reading {}", page.source.display()))?;
        let (aliases, summary) = man::name_line(&scdoc);

        let title = format!("{}({})", page.name, page.section);
        let mut doc = Doc::new(
            format!("man/{}.{}.html", page.name, page.section),
            title,
            kind_of(&page.section),
        );
        doc.lead.clone_from(&summary);
        doc.badges = vec![format!("Section {} · {}", page.section, section_title(&page.section))];
        doc.body = html.body;
        doc.headings = html.headings;
        doc.class = "man";
        doc.aliases = aliases.into_iter().filter(|a| a != &page.name).collect();
        doc.badges.push(format!("Also on your machine: man {} {}", &page.section[..1], page.name));
        match page.source.strip_prefix(&ctx.root) {
            Ok(relative) if !page.source.starts_with(&ctx.out) => {
                doc.source = Some(relative.to_string_lossy().into_owned());
            }
            _ => doc.note = "Generated from the source on every build".into(),
        }
        index.push((page.name.clone(), page.section.clone(), summary));
        docs.push(doc);
    }
    Ok(index)
}

fn section_title(section: &str) -> &'static str {
    SECTIONS.iter().find(|(s, _, _)| section.starts_with(s)).map_or("Manual", |(_, _, title)| title)
}

fn kind_of(section: &str) -> &'static str {
    match &section[..1] {
        "1" | "8" => "Command",
        "5" => "File format",
        "4" => "Driver",
        "2" => "System call",
        "3" => "Library",
        "7" => "Overview",
        _ => "Manual",
    }
}

/// man/index.html: every man page by section.
fn man_index(pages: &[(String, String, String)]) -> Doc {
    let mut body = String::new();
    let mut headings = Vec::new();
    for &(section, id, title) in SECTIONS {
        let mut in_section: Vec<_> =
            pages.iter().filter(|(_, s, _)| s.starts_with(section)).collect();
        if in_section.is_empty() {
            continue;
        }
        in_section.sort();
        headings.push((id.to_string(), title.to_string()));
        body.push_str(&format!("<h2 id=\"{id}\">{title}</h2><div class=\"pagelist\">"));
        for (name, section, summary) in in_section {
            body.push_str(&format!(
                "<a href=\"{name}.{section}.html\"><b>{}({section})</b><span>{}</span></a>",
                layout::escape(name),
                layout::escape(summary)
            ));
        }
        body.push_str("</div>");
    }
    let mut doc = Doc::new("man/index.html".into(), "Manual pages".into(), "Reference");
    doc.lead = format!(
        "Every command, file format, driver and interface: {} pages, the same as man shows on your machine.",
        pages.len()
    );
    doc.body = body;
    doc.headings = headings;
    doc
}

/// adr/index.html: every decision record, newest first.
fn adr_index(records: &[(String, String, String)]) -> Doc {
    let mut body = String::from(
        "<table class=\"adrs\"><thead><tr><th>Record</th><th>Status</th></tr></thead><tbody>",
    );
    for (path, title, status) in records.iter().rev() {
        let file = path.rsplit('/').next().unwrap_or(path);
        body.push_str(&format!(
            "<tr><td><a href=\"{file}\">{}</a></td><td>{}</td></tr>",
            layout::escape(title),
            layout::escape(status)
        ));
    }
    body.push_str("</tbody></table>");
    let mut doc = Doc::new("adr/index.html".into(), "Design records".into(), "How it works");
    doc.lead =
        "Every design decision the project has made, with the reasons and what it changed.".into();
    doc.body = body;
    doc
}

/// The `- Status:` line of a decision record.
fn status(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("- Status:"))
        .map_or_else(String::new, |s| s.trim().to_string())
}

/// The handbook's parts and chapters, in SUMMARY.md's order. Chapters
/// before the first part go under "Get started", with the first part's.
fn handbook(
    ctx: &Ctx,
    sites: &mut BTreeMap<String, String>,
) -> Result<Vec<(String, Vec<PathBuf>)>> {
    let dir = "docs/handbook/src";
    let summary = read(ctx, &Path::new(dir).join("SUMMARY.md"))?;
    let mut parts: Vec<(String, Vec<PathBuf>)> = vec![("Get started".into(), Vec::new())];
    let mut first_part = true;
    for line in summary.lines() {
        if let Some(part) = line.strip_prefix("# ") {
            if part == "Summary" {
                continue;
            }
            if first_part {
                first_part = false;
            } else {
                parts.push((part.trim().to_string(), Vec::new()));
            }
            continue;
        }
        let Some(file) = line.split("](").nth(1).and_then(|rest| rest.split(')').next()) else {
            continue;
        };
        if file.is_empty() {
            continue;
        }
        let source = format!("{dir}/{file}");
        let stem = file.trim_end_matches(".md");
        sites.insert(source.clone(), format!("handbook/{stem}.html"));
        parts.last_mut().expect("a part").1.push(ctx.root.join(source));
    }
    parts.retain(|(_, chapters)| !chapters.is_empty());
    Ok(parts)
}

/// The Markdown files of `dir`, sorted, each mapped to `<site_dir>/<stem>.html`.
/// README.md and the decision record template are left out.
fn folder(
    ctx: &Ctx,
    dir: &str,
    site_dir: &str,
    sites: &mut BTreeMap<String, String>,
) -> Result<Vec<PathBuf>> {
    let path = ctx.root.join(dir);
    if !path.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&path).context(|| format!("reading {dir}"))? {
        let file = entry.context(|| format!("reading {dir}"))?.path();
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let markdown = Path::new(&name).extension().is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !markdown || name == "README.md" || name.starts_with("0000-") {
            continue;
        }
        sites.insert(
            format!("{dir}/{name}"),
            format!("{site_dir}/{}.html", name.trim_end_matches(".md")),
        );
        files.push(file);
    }
    files.sort();
    Ok(files)
}

/// A page from a Markdown file, with its links pointed at the site's pages
/// where the file they name has one, and at GitHub otherwise.
fn markdown_doc(
    ctx: &Ctx,
    path: &Path,
    kind: &'static str,
    sites: &BTreeMap<String, String>,
) -> Result<Doc> {
    let source = path
        .strip_prefix(&ctx.root)
        .map_err(|_| Error::new(format!("{} is outside the repository", path.display())))?
        .to_string_lossy()
        .into_owned();
    let site_path = sites.get(&source).cloned().unwrap_or_else(|| source.replace(".md", ".html"));
    let text = read(ctx, path)?;
    let from_dir = Path::new(&source).parent().unwrap_or(Path::new("")).to_path_buf();
    let root = layout::root_of(&site_path);
    let link = |url: &str| resolve_link(url, &from_dir, &root, sites);
    let rendered = markdown::render(&text, &link);
    let title = if rendered.title.is_empty() { source.clone() } else { rendered.title };
    let mut doc = Doc::new(site_path, title, kind);
    doc.summary = rendered.summary;
    doc.body = rendered.body;
    doc.headings = rendered.headings;
    doc.source = Some(source);
    Ok(doc)
}

/// Where a link in a Markdown file under `from_dir` should go on the site.
fn resolve_link(
    url: &str,
    from_dir: &Path,
    root: &str,
    sites: &BTreeMap<String, String>,
) -> String {
    if url.is_empty() || url.starts_with('#') || url.contains("://") || url.starts_with("mailto:") {
        return url.to_string();
    }
    let (file, anchor) = match url.split_once('#') {
        Some((file, anchor)) => (file, format!("#{anchor}")),
        None => (url, String::new()),
    };
    let target = normalize(&from_dir.join(file));
    match sites.get(&target) {
        Some(site) => format!("{root}{site}{anchor}"),
        None if target.starts_with("..") => url.to_string(),
        None => format!("https://github.com/dukovo-os/dukovo/blob/main/{target}{anchor}"),
    }
}

/// `docs/handbook/src/../../adr/x.md` becomes `docs/adr/x.md`.
fn normalize(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                if parts.pop().is_none() {
                    parts.push("..".into());
                }
            }
            std::path::Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    parts.join("/")
}

/// The search index's entries: one per page and per other name of a page.
fn index_entries(docs: &[Doc]) -> Vec<check::Entry> {
    let mut entries = Vec::new();
    for doc in docs {
        let summary = if doc.summary.is_empty() { &doc.lead } else { &doc.summary };
        entries.push((doc.title.clone(), doc.kind, doc.path.clone(), summary.clone()));
        for alias in &doc.aliases {
            entries.push((
                alias.clone(),
                doc.kind,
                doc.path.clone(),
                format!("Another name for {}", doc.title),
            ));
        }
    }
    entries
}

/// What searching must find: each man page by its name and its other
/// names, and each error page by its ID, as (query, page).
fn wanted_searches(docs: &[Doc]) -> Vec<(String, String)> {
    let mut wanted = Vec::new();
    for doc in docs {
        if doc.class == "man" {
            let name = doc.title.split('(').next().unwrap_or(&doc.title);
            wanted.push((name.to_string(), doc.path.clone()));
            wanted.extend(doc.aliases.iter().map(|alias| (alias.clone(), doc.path.clone())));
        } else if doc.kind == "Error" {
            let id = doc.path.trim_start_matches("errors/").trim_end_matches(".html");
            wanted.push((id.to_string(), doc.path.clone()));
        }
    }
    wanted
}

/// search-index.js: [`index_entries`], for site.js.
fn search_index(docs: &[Doc]) -> String {
    let mut js = String::from(
        "// Written by `ob docs site`: [title, kind, link from the site's root, summary].\nvar DOCS=[\n",
    );
    for (title, kind, path, summary) in index_entries(docs) {
        js.push_str(&format!(
            "[{},{},{},{}],\n",
            json_string(&title),
            json_string(kind),
            json_string(&path),
            json_string(&summary)
        ));
    }
    js.push_str("];\n");
    js
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn read(ctx: &Ctx, path: &Path) -> Result<String> {
    let path = ctx.root.join(path);
    std::fs::read_to_string(&path).context(|| format!("reading {}", ctx.display(&path)))
}

fn write(path: &Path, text: &str) -> Result<()> {
    process::create_dir(path.parent().expect("a page is in a folder"))?;
    std::fs::write(path, text).context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_links_to_site_pages_or_github() {
        let sites = BTreeMap::from([
            ("docs/handbook/src/ssh.md".to_string(), "handbook/ssh.html".to_string()),
            ("docs/adr/0213-x.md".to_string(), "adr/0213-x.html".to_string()),
        ]);
        let from = Path::new("docs/handbook/src");
        assert_eq!(resolve_link("ssh.md#keys", from, "../", &sites), "../handbook/ssh.html#keys");
        assert_eq!(resolve_link("../../adr/0213-x.md", from, "../", &sites), "../adr/0213-x.html");
        assert_eq!(
            resolve_link("../../../tools/xtask/src/main.rs", from, "../", &sites),
            "https://github.com/dukovo-os/dukovo/blob/main/tools/xtask/src/main.rs"
        );
        assert_eq!(resolve_link("https://x.org/a", from, "../", &sites), "https://x.org/a");
        assert_eq!(resolve_link("#here", from, "../", &sites), "#here");
    }

    #[test]
    fn writes_a_search_index_with_aliases() {
        let mut doc = Doc::new("man/7zz.1.html".into(), "7zz(1)".into(), "Command");
        doc.lead = "the \"7-Zip\" archiver".into();
        doc.aliases = vec!["7z".into()];
        let js = search_index(&[doc]);
        assert!(js.contains(
            "[\"7zz(1)\",\"Command\",\"man/7zz.1.html\",\"the \\\"7-Zip\\\" archiver\"],"
        ));
        assert!(
            js.contains("[\"7z\",\"Command\",\"man/7zz.1.html\",\"Another name for 7zz(1)\"],")
        );
    }

    #[test]
    fn reads_decision_record_status() {
        assert_eq!(status("# 0001. A\n\n- Status: Accepted\n- Date: x\n"), "Accepted");
        assert_eq!(status("# no status\n"), "");
    }

    /// Every folder the site reads, and the handbook's contents, in the tree.
    #[test]
    fn the_handbook_summary_names_real_chapters() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let summary = std::fs::read_to_string(root.join("docs/handbook/src/SUMMARY.md")).unwrap();
        for line in summary.lines() {
            if let Some(file) = line.split("](").nth(1).and_then(|r| r.split(')').next())
                && !file.is_empty()
            {
                assert!(root.join("docs/handbook/src").join(file).exists(), "{file}");
            }
        }
    }
}
