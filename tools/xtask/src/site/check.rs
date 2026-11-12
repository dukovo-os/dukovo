// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The checks that keep the support center as current as the lab's tests
//! (ADR 0420).
//!
//! `ob docs --check` reads the tree: every example marked as a test is one
//! the `docs-examples` boot test can type, every guide, tutorial and
//! runbook has a lab test of its own, and every error ID the code prints
//! has a page, as every page has an ID the code prints. `ob docs site
//! --check` reads the built site: every link and anchor between its pages
//! leads somewhere, and searching for each command, each of its other
//! names and each error ID finds its page.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::{Context, Ctx, Result};

/// The info string of a Markdown code block that the `docs-examples` boot
/// test runs.
pub const EXAMPLE_FENCE: &str = "```console test";

/// The folders whose Markdown may hold examples marked as tests.
pub const EXAMPLE_DIRS: &[&str] =
    &["docs/handbook/src", "docs/guides", "docs/tutorials", "docs/runbooks"];

/// Folders of pages that each need a lab test of their own, and the
/// prefix of that test's name: `docs/guides/media-server.md` needs the
/// boot test `guide-media-server`.
pub const TESTED_PAGES: &[(&str, &str)] =
    &[("docs/guides/", "guide-"), ("docs/tutorials/", "tutorial-"), ("docs/runbooks/", "runbook-")];

/// Where the error pages are, one per ID, and the list of the areas that
/// give their errors IDs.
const ERRORS: &str = "docs/errors";

/// The folders whose files the error ID check reads for IDs the code
/// prints, and the kinds of file it reads.
const CODE_DIRS: &[&str] = &["abi", "kernel", "userspace", "tools"];
const CODE_FILES: &[&str] = &["rs", "c", "h", "ridl", "toml", "sh", "json"];

/// One command of an example and what the screen must show after it.
#[derive(Debug, PartialEq)]
pub struct Step {
    pub command: String,
    /// Lines, or parts of lines, the screen shows once it has run.
    pub shows: Vec<String>,
}

/// An example marked as a test, from the line of its opening fence.
#[derive(Debug, PartialEq)]
pub struct Example {
    pub line: usize,
    pub steps: Vec<Step>,
}

/// The examples marked as tests in a Markdown file: blocks that open with
/// ```` ```console test ````, in which a line that starts with `# ` or
/// `$ ` is a command and each other line is output the screen must show
/// after it. A line of `...` stands for output left out.
pub fn examples(text: &str) -> std::result::Result<Vec<Example>, String> {
    let mut examples = Vec::new();
    let mut fence: Option<(usize, Option<Example>)> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            match fence.take() {
                None if trimmed == EXAMPLE_FENCE => {
                    fence = Some((number, Some(Example { line: number, steps: Vec::new() })));
                }
                None => fence = Some((number, None)),
                Some((opened, Some(example))) => {
                    if example.steps.is_empty() {
                        return Err(format!("{opened}: the example has no command"));
                    }
                    examples.push(example);
                }
                Some((_, None)) => {}
            }
            continue;
        }
        let Some((_, Some(example))) = fence.as_mut() else { continue };
        if trimmed.is_empty() || trimmed == "..." {
            continue;
        }
        if !trimmed.is_ascii() || trimmed.chars().any(|c| c.is_ascii_control()) {
            return Err(format!(
                "{number}: the boot test types commands and reads the screen in ASCII alone"
            ));
        }
        if let Some(command) = trimmed.strip_prefix("# ").or_else(|| trimmed.strip_prefix("$ ")) {
            example.steps.push(Step { command: command.trim().to_string(), shows: Vec::new() });
        } else if let Some(step) = example.steps.last_mut() {
            step.shows.push(trimmed.to_string());
        } else {
            return Err(format!("{number}: an example starts with a command, after `# ` or `$ `"));
        }
    }
    match fence {
        Some((opened, _)) => Err(format!("{opened}: the code block is not closed")),
        None => Ok(examples),
    }
}

/// Whether `path`, from the repository's root, is a Markdown file of
/// [`EXAMPLE_DIRS`], where examples marked as tests run.
pub fn example_file(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
        && EXAMPLE_DIRS
            .iter()
            .any(|dir| path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/')))
}

/// Whether the file at `path` holds an example marked as a test.
pub fn has_examples(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| text.lines().any(|l| l.trim() == EXAMPLE_FENCE))
}

/// The Markdown files under `dir`, at any depth, sorted.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Every example marked as a test in the tree under `root`, with the file
/// it is in, from the root.
pub fn tree_examples(root: &Path) -> std::result::Result<Vec<(String, Example)>, String> {
    let mut all = Vec::new();
    for dir in EXAMPLE_DIRS {
        for path in markdown_files(&root.join(dir)) {
            let name = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
            let examples = examples(&text).map_err(|e| format!("{name}:{e}"))?;
            all.extend(examples.into_iter().map(|example| (name.clone(), example)));
        }
    }
    Ok(all)
}

/// The boot test a page in [`TESTED_PAGES`] needs, if `path` is one.
pub fn page_test(path: &str) -> Option<String> {
    TESTED_PAGES.iter().find_map(|(dir, prefix)| {
        let stem = path.strip_prefix(dir)?.strip_suffix(".md")?;
        (stem != "README" && !stem.contains('/')).then(|| format!("{prefix}{stem}"))
    })
}

/// What `ob docs --check` finds wrong in the tree's examples, tested pages
/// and error IDs.
pub fn tree_problems(ctx: &Ctx) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for dir in EXAMPLE_DIRS {
        for path in markdown_files(&ctx.root.join(dir)) {
            let text =
                std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
            if let Err(err) = examples(&text) {
                problems.push(format!("{}:{err} (ADR 0420)", ctx.display(&path)));
            }
        }
    }

    let cases: BTreeSet<&str> = crate::test::all_cases().map(|case| case.name).collect();
    for (dir, _) in TESTED_PAGES {
        for path in markdown_files(&ctx.root.join(dir)) {
            let relative = path.strip_prefix(&ctx.root).unwrap_or(&path).display().to_string();
            if let Some(test) = page_test(&relative)
                && !cases.contains(test.as_str())
            {
                problems.push(format!(
                    "{relative}: has no lab test; add the boot test `{test}`, which follows it \
                     from start to end (ADR 0420)"
                ));
            }
        }
    }

    problems.extend(error_id_problems(ctx)?);
    Ok(problems)
}

/// The areas whose errors have IDs: the first cell of each row of the
/// table in docs/errors/README.md, such as `` `UPD` ``.
fn error_prefixes(readme: &str) -> Vec<String> {
    readme
        .lines()
        .filter_map(|line| line.strip_prefix('|')?.split('|').next())
        .map(|cell| cell.trim().trim_matches('`').to_string())
        .filter(|cell| {
            (2..=5).contains(&cell.len()) && cell.bytes().all(|b| b.is_ascii_uppercase())
        })
        .collect()
}

/// The error IDs in `text`, with the line each is on: a prefix of
/// `prefixes`, `-` and three digits, standing apart from the words beside
/// it.
fn find_ids(text: &str, prefixes: &[String]) -> Vec<(usize, String)> {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for prefix in prefixes {
            let needle = format!("{prefix}-");
            let mut from = 0;
            while let Some(at) = line[from..].find(&needle) {
                let start = from + at;
                let digits = &line[start + needle.len()..];
                from = start + needle.len();
                if word(line[..start].chars().next_back()) {
                    continue;
                }
                if digits.len() >= 3
                    && digits.as_bytes()[..3].iter().all(u8::is_ascii_digit)
                    && !word(digits[3..].chars().next())
                {
                    found.push((index + 1, format!("{needle}{}", &digits[..3])));
                }
            }
        }
    }
    found
}

/// Every ID the code prints needs a page in docs/errors, and every page
/// an ID the code prints (ADR 0420).
fn error_id_problems(ctx: &Ctx) -> Result<Vec<String>> {
    let dir = ctx.root.join(ERRORS);
    let readme = std::fs::read_to_string(dir.join("README.md")).unwrap_or_default();
    let prefixes = error_prefixes(&readme);
    let mut problems = Vec::new();

    let mut pages = BTreeSet::new();
    for path in markdown_files(&dir) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name == "README.md" {
            continue;
        }
        let stem = name.trim_end_matches(".md");
        if find_ids(stem, &prefixes).first().is_some_and(|(_, id)| id == stem) {
            pages.insert(stem.to_string());
        } else {
            problems.push(format!(
                "{}: an error page is named for its ID, such as UPD-101.md, of an area listed in \
                 {ERRORS}/README.md",
                ctx.display(&path)
            ));
        }
    }
    if prefixes.is_empty() {
        return Ok(problems);
    }

    // Where the code prints each ID: the first place, for the message.
    let mut printed: BTreeMap<String, String> = BTreeMap::new();
    let mut dirs: Vec<PathBuf> = CODE_DIRS.iter().map(|d| ctx.root.join(d)).collect();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !name.starts_with('.') && name != "target" {
                    dirs.push(path);
                }
                continue;
            }
            if !path.extension().is_some_and(|e| CODE_FILES.iter().any(|k| e == *k)) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for (line, id) in find_ids(&text, &prefixes) {
                printed.entry(id).or_insert_with(|| format!("{}:{line}", ctx.display(&path)));
            }
        }
    }
    for (id, place) in &printed {
        if !pages.contains(id) {
            problems.push(format!(
                "{place}: the error {id} has no page; add {ERRORS}/{id}.md, saying what it means \
                 and what to do (ADR 0420)"
            ));
        }
    }
    for id in &pages {
        if !printed.contains_key(id) {
            problems.push(format!(
                "{ERRORS}/{id}.md: no code gives the error {id} any more; remove its page \
                 (ADR 0420)"
            ));
        }
    }
    Ok(problems)
}

/// An entry of the search index: title, kind, link from the site's root
/// and summary.
pub type Entry = (String, &'static str, String, String);

/// What the support center's search shows for `query`: the same entries,
/// in the same order, as `wireSearch` and `rank` in docs/site/site.js,
/// which this follows. Keep the two alike.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let query = query.trim().to_lowercase();
    let rank = |entry: &Entry| {
        let title = entry.0.to_lowercase();
        if title == query || title.starts_with(&format!("{query}(")) {
            0
        } else if title.starts_with(&query) {
            1
        } else if title.contains(&query) {
            2
        } else {
            3
        }
    };
    let mut found: Vec<&Entry> = entries
        .iter()
        .filter(|(title, kind, _, summary)| {
            format!("{title} {kind} {summary}").to_lowercase().contains(&query)
        })
        .collect();
    found.sort_by_key(|entry| rank(entry));
    found.truncate(10);
    found
}

/// Searching for each of `wanted`, (query, page), must show the page.
pub fn search_problems(entries: &[Entry], wanted: &[(String, String)]) -> Vec<String> {
    wanted
        .iter()
        .filter(|(query, page)| !search(entries, query).iter().any(|entry| &entry.2 == page))
        .map(|(query, page)| {
            format!("searching for `{query}` does not show {page} among its results (ADR 0420)")
        })
        .collect()
}

/// The places a page links to (`href` and `src`), and the IDs it has.
fn links_and_ids(html: &str) -> (Vec<String>, BTreeSet<String>) {
    let attribute = |name: &str| -> Vec<String> {
        let needle = format!(" {name}=\"");
        let mut values = Vec::new();
        let mut rest = html;
        while let Some(at) = rest.find(&needle) {
            rest = &rest[at + needle.len()..];
            if let Some(end) = rest.find('"') {
                values.push(rest[..end].replace("&amp;", "&"));
                rest = &rest[end..];
            }
        }
        values
    };
    let mut links = attribute("href");
    links.extend(attribute("src"));
    (links, attribute("id").into_iter().collect())
}

/// Every link from one of the site's pages to another, under `site`, must
/// lead to a file, and to an element of the page when it names one.
/// Links to other sites, and to dukovo.org's own paths (`/fonts/`), are
/// left to the weekly check of outside links.
pub fn link_problems(site: &Path) -> Result<Vec<String>> {
    let mut pages = BTreeMap::new();
    let mut dirs = vec![site.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).context(|| format!("reading {}", dir.display()))? {
            let path = entry.context(|| format!("reading {}", dir.display()))?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "html") {
                let html = std::fs::read_to_string(&path)
                    .context(|| format!("reading {}", path.display()))?;
                let relative = path.strip_prefix(site).unwrap_or(&path).display().to_string();
                pages.insert(relative, links_and_ids(&html));
            }
        }
    }

    let mut problems = Vec::new();
    for (page, (links, _)) in &pages {
        let dir = Path::new(page).parent().unwrap_or(Path::new(""));
        for link in links {
            if link.is_empty()
                || link.starts_with('/')
                || link.contains("://")
                || link.starts_with("mailto:")
                || link.starts_with("data:")
            {
                continue;
            }
            let (file, anchor) = link.split_once('#').unwrap_or((link.as_str(), ""));
            let file = file.split('?').next().unwrap_or_default();
            let target = if file.is_empty() {
                page.clone()
            } else {
                match normalize(&dir.join(file)) {
                    Some(target) => target,
                    None => {
                        problems.push(format!("{page}: `{link}` leads out of the site"));
                        continue;
                    }
                }
            };
            if let Some((_, ids)) = pages.get(&target) {
                if !anchor.is_empty() && !ids.contains(anchor) {
                    problems.push(format!("{page}: `{link}`: {target} has no `#{anchor}`"));
                }
            } else if !site.join(&target).is_file() {
                problems.push(format!("{page}: `{link}`: there is no {target}"));
            }
        }
    }
    Ok(problems)
}

/// `a/../b/./c` as `b/c`; `None` for a path that climbs out of the root.
fn normalize(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            std::path::Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_examples_marked_as_tests() {
        let text = "Some text.\n\n```sh\n# not a test\n```\n\n```console test\n\
                    # realpath /bin /etc\n/system/commands\n...\n/system/settings\n\n\
                    $ echo hi\nhi\n# true\n```\n";
        assert_eq!(
            examples(text).unwrap(),
            [Example {
                line: 7,
                steps: vec![
                    Step {
                        command: "realpath /bin /etc".into(),
                        shows: vec!["/system/commands".into(), "/system/settings".into()]
                    },
                    Step { command: "echo hi".into(), shows: vec!["hi".into()] },
                    Step { command: "true".into(), shows: vec![] },
                ]
            }]
        );
    }

    #[test]
    fn refuses_examples_the_test_cannot_run() {
        assert_eq!(
            examples("```console test\nhi\n```\n").unwrap_err(),
            "2: an example starts with a command, after `# ` or `$ `"
        );
        assert_eq!(
            examples("```console test\n```\n").unwrap_err(),
            "1: the example has no command"
        );
        assert_eq!(
            examples("```console test\n# ls\n").unwrap_err(),
            "1: the code block is not closed"
        );
        assert!(examples("```console test\n# echo “hi”\n```\n").unwrap_err().contains("ASCII"));
    }

    #[test]
    fn knows_where_examples_run() {
        assert!(example_file("docs/handbook/src/files.md"));
        assert!(example_file("docs/guides/media-server.md"));
        assert!(!example_file("CONTRIBUTING.md"));
        assert!(!example_file("docs/handbook/src-old/a.md"));
        assert!(!example_file("docs/handbook/src/a.png"));
    }

    #[test]
    fn names_the_test_a_page_needs() {
        assert_eq!(page_test("docs/guides/media-server.md").as_deref(), Some("guide-media-server"));
        assert_eq!(page_test("docs/runbooks/disk-full.md").as_deref(), Some("runbook-disk-full"));
        assert_eq!(page_test("docs/guides/README.md"), None);
        assert_eq!(page_test("docs/handbook/src/ssh.md"), None);
    }

    #[test]
    fn finds_error_ids() {
        let readme =
            "| Prefix | Area |\n| --- | --- |\n| `XYZ` | The example area |\n| AB | Another |\n";
        let prefixes = error_prefixes(readme);
        assert_eq!(prefixes, ["XYZ", "AB"]);
        let code = "fail(\"XYZ-103: the disk is full\");\nlet hash = \"SHA-256\"; // AXYZ-104, XYZ-1050, XYZ-10\n\
                    AB-001 and (XYZ-200).";
        assert_eq!(
            find_ids(code, &prefixes),
            [(1, "XYZ-103".to_string()), (3, "XYZ-200".to_string()), (3, "AB-001".to_string())]
        );
    }

    #[test]
    fn searches_as_the_site_does() {
        let entry = |title: &str, path: &str, summary: &str| -> Entry {
            (title.into(), "Command", path.into(), summary.into())
        };
        let entries = vec![
            entry("lsblk(8)", "man/lsblk.8.html", "list block devices"),
            entry("tools", "man/x.html", "uses ls a lot"),
            entry("ls(1)", "man/ls.1.html", "list directory contents"),
            entry("7z", "man/7zz.1.html", "Another name for 7zz(1)"),
        ];
        let found = search(&entries, "ls");
        assert_eq!(found[0].2, "man/ls.1.html");
        assert_eq!(found[1].2, "man/lsblk.8.html");
        assert!(search_problems(&entries, &[("7z".into(), "man/7zz.1.html".into())]).is_empty());
        assert_eq!(
            search_problems(&entries, &[("zip".into(), "man/7zz.1.html".into())]),
            ["searching for `zip` does not show man/7zz.1.html among its results (ADR 0420)"]
        );
    }

    /// The examples in the tree are ones the `docs-examples` test can run.
    #[test]
    fn the_examples_in_the_tree_can_run() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let examples = tree_examples(&root).unwrap();
        assert!(examples.iter().any(|(file, _)| file == "docs/handbook/src/files.md"));
    }

    #[test]
    fn checks_links_and_anchors() {
        let dir = std::env::temp_dir().join(format!("ob-site-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("man")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<a href=\"man/ls.1.html#OPTIONS\">ls</a><a href=\"man/ls.1.html#NOPE\">x</a>\
             <a href=\"https://dukovo.org/\">site</a><a href=\"/fonts/a.woff2\">f</a>\
             <a href=\"#top\">top</a><a href=\"gone.html\">gone</a><script src=\"site.js\"></script>",
        )
        .unwrap();
        std::fs::write(
            dir.join("man/ls.1.html"),
            "<h2 id=\"OPTIONS\">Options</h2><a href=\"../../x.html\">up</a>",
        )
        .unwrap();
        std::fs::write(dir.join("site.js"), "").unwrap();
        let problems = link_problems(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            problems,
            [
                "index.html: `man/ls.1.html#NOPE`: man/ls.1.html has no `#NOPE`",
                "index.html: `#top`: index.html has no `#top`",
                "index.html: `gone.html`: there is no gone.html",
                "man/ls.1.html: `../../x.html` leads out of the site",
            ]
        );
    }
}
