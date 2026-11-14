// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob lab coverage`: every command, service, console page and API route
//! that Phase 16 adds, beside the lab tests that cover it, and with
//! `--check` CI's gate that fails while one has none.
//!
//! Three things say what there is:
//!
//! - the tree: the commands are the manual pages of sections 1 and 8 in
//!   `docs/man` and the commands of the catalog's bundles, and the
//!   services are the system's service files and the bundles' services,
//!   each bundle's read from its port's recipe;
//! - `lab/before-phase-16.txt`: what of that there was before Phase 16,
//!   which needs no entry;
//! - `lab/coverage.toml`: each Phase 16 item and its tests, by id. An
//!   item is a command, a command with its subcommand (`service
//!   explain`), a service, a page of the web console or a route of its
//!   API.
//!
//! The check fails when something new since Phase 15 has no entry, when
//! an entry names no test or one that is not in `lab/tests`, when a test
//! never names its item (each of its words, in the test's file, in a file
//! beside it that the test uses as `{here}/NAME`, or in the script of a
//! `browser = "NAME"` step, which is where a page's path is named), and
//! when an entry is of a command or service the tree no longer has. Pages
//! and routes have no list in the tree to compare with; their entries are
//! checked against their tests alone. When the gate fails in CI, `./ob lab
//! coverage` shows the whole table with the same messages.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::ports;
use crate::{Context as _, Ctx, Error, Result};

/// The tests, a folder for each category.
const TESTS: &str = "lab/tests";

/// Each Phase 16 item and its tests.
const COVERAGE: &str = "lab/coverage.toml";

/// The lab's categories, which dukovo-lab refuses to load with a name
/// twice.
const CATEGORIES: &str = "lab/tests/categories.toml";

/// What there was before Phase 16.
const BEFORE: &str = "lab/before-phase-16.txt";

/// The system's service files.
const SERVICES: &str = "userspace/defaults/spark/services";

/// The manual pages.
const MAN: &str = "docs/man";

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Command,
    Service,
    Page,
    Route,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Command, Kind::Service, Kind::Page, Kind::Route];

    /// The word `lab/before-phase-16.txt` and the report say it with.
    fn word(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Service => "service",
            Kind::Page => "page",
            Kind::Route => "route",
        }
    }
}

/// A command, service, page or route.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Item {
    kind: Kind,
    name: String,
}

impl Item {
    fn new(kind: Kind, name: &str) -> Item {
        Item { kind, name: name.to_string() }
    }
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} `{}`", self.kind.word(), self.name)
    }
}

/// `lab/coverage.toml`: each table maps an item's name to its tests.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entries {
    #[serde(default)]
    commands: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    services: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pages: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    routes: BTreeMap<String, Vec<String>>,
}

impl Entries {
    /// Every entry's item and tests, by kind and then name.
    fn items(&self) -> Vec<(Item, &[String])> {
        let tables = [&self.commands, &self.services, &self.pages, &self.routes];
        Kind::ALL
            .into_iter()
            .zip(tables)
            .flat_map(|(kind, table)| {
                table.iter().map(move |(name, tests)| (Item::new(kind, name), tests.as_slice()))
            })
            .collect()
    }
}

/// What the check found.
#[derive(Debug, Default, PartialEq, Eq)]
struct Audit {
    /// Each entry, with its tests, by kind and then name.
    rows: Vec<(Item, Vec<String>)>,
    /// How many entries have tests that are there and name their item.
    covered: usize,
    /// How many items are new since Phase 15 and have no entry.
    unlisted: usize,
    /// What is wrong, a sentence each.
    problems: Vec<String>,
}

/// `ob lab coverage [--check]`: the table, or with `check` the summary
/// alone; fails when anything is wrong.
pub fn command(ctx: &Ctx, check: bool) -> Result<()> {
    let found = in_tree(ctx)?;
    let before = read_before(&ctx.root.join(BEFORE))?;
    let path = ctx.root.join(COVERAGE);
    let text = std::fs::read_to_string(&path).context(|| format!("reading {COVERAGE}"))?;
    let entries: Entries =
        toml::from_str(&text).map_err(|err| Error::new(format!("{COVERAGE}: {err}")))?;
    let tests = ctx.root.join(TESTS);
    let mut audit = audit(&found, &before, &entries, &|id| test_text(&tests, id));
    let categories = std::fs::read_to_string(ctx.root.join(CATEGORIES))
        .context(|| format!("reading {CATEGORIES}"))?;
    audit.problems.extend(named_twice(&categories).into_iter().map(|name| {
        format!("{CATEGORIES}: category \"{name}\" is named twice, and dukovo-lab loads no test")
    }));
    if !check {
        for (item, tests) in &audit.rows {
            println!("{:<8} {:<30} {}", item.kind.word(), item.name, tests.join(", "));
        }
        println!();
    }
    let total = audit.rows.len() + audit.unlisted;
    let percent = (audit.covered * 100).checked_div(total).unwrap_or(100);
    println!(
        "Phase 16 lab coverage: {} of {total} items have lab tests ({percent}%)",
        audit.covered
    );
    if audit.problems.is_empty() {
        return Ok(());
    }
    for problem in &audit.problems {
        eprintln!("  {problem}");
    }
    let problems = match audit.problems.len() {
        1 => "a problem".to_string(),
        n => format!("{n} problems"),
    };
    Err(Error::new(format!(
        "{problems} in the lab's coverage of Phase 16: {COVERAGE} lists each item with the tests \
         that cover it (the handbook's Lab tests chapter, Coverage)"
    )))
}

/// The names of `categories.toml`'s categories that it gives twice.
fn named_twice(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut twice = Vec::new();
    for line in text.lines() {
        let Some(value) = line.trim().strip_prefix("name = ") else { continue };
        let name = value.trim_matches('"').to_string();
        if !seen.insert(name.clone()) && !twice.contains(&name) {
            twice.push(name);
        }
    }
    twice
}

/// Holds `entries` to the items `found` in the tree, less those there
/// `before` Phase 16, with `test` giving a test's text by id, or None for
/// one that is not there.
fn audit(
    found: &BTreeSet<Item>,
    before: &BTreeSet<Item>,
    entries: &Entries,
    test: &dyn Fn(&str) -> Option<String>,
) -> Audit {
    let mut audit = Audit::default();
    let items = entries.items();
    let listed: BTreeSet<&Item> = items.iter().map(|(item, _)| item).collect();
    for item in found.iter().filter(|item| !before.contains(item) && !listed.contains(item)) {
        audit.problems.push(format!(
            "{item} is new since Phase 15 and has no entry in {COVERAGE}: add it there with the \
             lab tests that cover it"
        ));
        audit.unlisted += 1;
    }
    for (item, tests) in items {
        let mut covered = !tests.is_empty();
        if let Some(why) = gone(&item, found) {
            audit.problems.push(why);
            covered = false;
        }
        if tests.is_empty() {
            audit.problems.push(format!("{item} names no lab test in {COVERAGE}"));
        }
        for id in tests {
            let problem = match test(id) {
                None => format!("{item}: `{id}` is no lab test: there is no {TESTS}/{id}.toml"),
                Some(text) if !names(&text, &item.name) => {
                    format!("{item}: {TESTS}/{id}.toml never names it")
                }
                Some(_) => continue,
            };
            audit.problems.push(problem);
            covered = false;
        }
        audit.covered += usize::from(covered);
        audit.rows.push((item, tests.to_vec()));
    }
    audit
}

/// Why `item` is no longer in the tree, if it is not: a command whose
/// first word has no manual page and is no bundle's, or a service with no
/// file.
fn gone(item: &Item, found: &BTreeSet<Item>) -> Option<String> {
    match item.kind {
        Kind::Command => {
            let command = item.name.split_whitespace().next().unwrap_or("");
            (!found.contains(&Item::new(Kind::Command, command))).then(|| {
                format!(
                    "{item}: `{command}` has no manual page in {MAN} and is no bundle's command"
                )
            })
        }
        Kind::Service => (!found.contains(item)).then(|| {
            format!("{item} has no service file in {SERVICES} and is no bundle's service")
        }),
        Kind::Page | Kind::Route => None,
    }
}

/// Whether `text` holds each word of `name` as a word of its own:
/// `python` is not named by `python3` or by a bundle's ID,
/// `org.python.python`.
fn names(text: &str, name: &str) -> bool {
    let part_of_word = |c: char| c.is_ascii_alphanumeric() || "_-.".contains(c);
    name.split_whitespace().all(|word| {
        text.match_indices(word).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(part_of_word) && !after.is_some_and(part_of_word)
        })
    })
}

/// The text of test `id`, `category/name`, with the files beside it that
/// it uses as `{here}/FILE` and the scripts of its browser steps; None if
/// there is no such test.
fn test_text(tests: &Path, id: &str) -> Option<String> {
    let (category, name) = id.split_once('/')?;
    let plain = |part: &str| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    };
    if !plain(category) || !plain(name) {
        return None;
    }
    let dir = tests.join(category);
    let mut text = std::fs::read_to_string(dir.join(format!("{name}.toml"))).ok()?;
    let used: BTreeSet<&str> = text
        .match_indices("{here}/")
        .map(|(at, found)| file_name(&text[at + found.len()..]))
        .chain(browser_scripts(&text))
        .filter(|file| !file.is_empty() && !file.starts_with('.'))
        .collect();
    let beside: String =
        used.iter().filter_map(|file| std::fs::read_to_string(dir.join(file)).ok()).collect();
    text.push_str(&beside);
    Some(text)
}

/// The file name that `rest` starts with.
fn file_name(rest: &str) -> &str {
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || "-_.".contains(c)))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The scripts the `browser = "NAME"` steps of a test file's `text` name.
fn browser_scripts(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|line| {
        let value = line.trim_start().strip_prefix("browser")?.trim_start().strip_prefix('=')?;
        Some(file_name(value.trim_start().strip_prefix('"')?))
    })
}

/// The commands and services the tree has.
fn in_tree(ctx: &Ctx) -> Result<BTreeSet<Item>> {
    let mut found = BTreeSet::new();
    for name in file_names(&ctx.root.join(MAN))? {
        if let Some(command) = name.strip_suffix(".1.scd").or_else(|| name.strip_suffix(".8.scd")) {
            found.insert(Item::new(Kind::Command, command));
        }
    }
    for name in file_names(&ctx.root.join(SERVICES))? {
        if let Some(service) = name.strip_suffix(".toml") {
            found.insert(Item::new(Kind::Service, service));
        }
    }
    for (recipe, _) in ports::recipes(ctx)? {
        let Some(bundle) = recipe.bundle else { continue };
        let commands = bundle.commands.iter().chain(bundle.aliases.keys());
        found.extend(commands.map(|command| Item::new(Kind::Command, command)));
        for service in &bundle.services {
            let Some(name) = service.get("name").and_then(toml::Value::as_str) else {
                return Err(Error::new(format!(
                    "ports/{}/recipe.toml: a service of its bundle has no name",
                    recipe.name
                )));
            };
            found.insert(Item::new(Kind::Service, name));
        }
    }
    Ok(found)
}

/// The names of the files in `dir`.
fn file_names(dir: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(dir).context(|| format!("reading {}", dir.display()))?;
    entries
        .map(|entry| {
            let entry = entry.context(|| format!("reading {}", dir.display()))?;
            Ok(entry.file_name().to_string_lossy().into_owned())
        })
        .collect()
}

/// Reads `lab/before-phase-16.txt`: a `KIND NAME` line for each item,
/// and `#` comments.
fn read_before(path: &Path) -> Result<BTreeSet<Item>> {
    let text = std::fs::read_to_string(path).context(|| format!("reading {BEFORE}"))?;
    parse_before(&text).map_err(|why| Error::new(format!("{BEFORE}: {why}")))
}

fn parse_before(text: &str) -> std::result::Result<BTreeSet<Item>, String> {
    let mut items = BTreeSet::new();
    for (number, line) in (1..).zip(text.lines()) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let kind = line.split_once(' ').and_then(|(word, name)| {
            let kind = Kind::ALL.into_iter().find(|kind| kind.word() == word)?;
            Some(Item::new(kind, name.trim()))
        });
        let Some(item) = kind else {
            return Err(format!(
                "line {number}: `{line}` is not a kind (command, service, page or route) and a name"
            ));
        };
        items.insert(item);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_category_named_twice_is_a_problem() {
        let one = "[[category]]\nname = \"time\"\n\n[[category]]\nname = \"monitoring\"\n";
        assert!(named_twice(one).is_empty());
        let two = format!("{one}\n[[category]]\nname = \"monitoring\"\ntitle = \"Monitoring\"\n");
        assert_eq!(named_twice(&two), ["monitoring"]);
    }

    fn items(list: &[(Kind, &str)]) -> BTreeSet<Item> {
        list.iter().map(|&(kind, name)| Item::new(kind, name)).collect()
    }

    fn entries(text: &str) -> Entries {
        toml::from_str(text).unwrap()
    }

    /// Two tests: one that runs `service nginx explain`, and one that runs
    /// a script beside it.
    fn test(id: &str) -> Option<String> {
        match id {
            "services/explain" => Some("run = \"service nginx explain\"".into()),
            "services/valkey" => Some("run = \"valkey-cli ping\"".into()),
            _ => None,
        }
    }

    #[test]
    fn an_entry_whose_tests_name_it_is_covered() {
        let found = items(&[(Kind::Command, "service"), (Kind::Service, "nginx")]);
        let before = items(&[(Kind::Command, "service")]);
        let listed = entries(
            "[commands]\n\"service explain\" = [\"services/explain\"]\n\
             [services]\nnginx = [\"services/explain\"]\n",
        );
        let audit = audit(&found, &before, &listed, &test);
        assert_eq!(audit.problems, Vec::<String>::new());
        assert_eq!((audit.covered, audit.unlisted, audit.rows.len()), (2, 0, 2));
    }

    #[test]
    fn something_new_without_an_entry_fails() {
        let found = items(&[(Kind::Command, "service"), (Kind::Command, "troubleshoot")]);
        let before = items(&[(Kind::Command, "service")]);
        let audit = audit(&found, &before, &Entries::default(), &test);
        assert_eq!(audit.unlisted, 1);
        assert_eq!(
            audit.problems,
            ["the command `troubleshoot` is new since Phase 15 and has no entry in \
              lab/coverage.toml: add it there with the lab tests that cover it"]
        );
    }

    #[test]
    fn an_entry_needs_tests_that_are_there_and_name_it() {
        let found = items(&[(Kind::Command, "valkey-cli"), (Kind::Command, "valkey-benchmark")]);
        let listed = entries(
            "[commands]\nvalkey-cli = []\n\
             valkey-benchmark = [\"services/valkey\", \"services/gone\"]\n",
        );
        let audit = audit(&found, &BTreeSet::new(), &listed, &test);
        assert_eq!(audit.covered, 0);
        assert_eq!(
            audit.problems,
            [
                "the command `valkey-benchmark`: lab/tests/services/valkey.toml never names it",
                "the command `valkey-benchmark`: `services/gone` is no lab test: there is no \
                 lab/tests/services/gone.toml",
                "the command `valkey-cli` names no lab test in lab/coverage.toml",
            ]
        );
    }

    #[test]
    fn an_entry_of_what_the_tree_no_longer_has_fails() {
        let listed = entries(
            "[commands]\n\"service explain\" = [\"services/explain\"]\n\
             [services]\nnginx = [\"services/explain\"]\n\
             [routes]\n\"GET /api/1/services\" = []\n",
        );
        let audit = audit(&BTreeSet::new(), &BTreeSet::new(), &listed, &test);
        assert_eq!(
            audit.problems,
            [
                "the command `service explain`: `service` has no manual page in docs/man and is \
                 no bundle's command",
                "the service `nginx` has no service file in userspace/defaults/spark/services and \
                 is no bundle's service",
                "the route `GET /api/1/services` names no lab test in lab/coverage.toml",
            ]
        );
    }

    #[test]
    fn an_item_is_named_by_whole_words() {
        assert!(names("run = \"python3 -m pip --version\"", "python3"));
        assert!(!names("run = \"python3 -m pip --version\"", "python"));
        assert!(names("/applications/org.python.python/bin/pip3 list", "pip3"));
        assert!(!names("services/nginx", "service"));
        assert!(!names("/applications/org.python.python/bin/python3", "python"));
        assert!(names("service nginx explain", "service explain"));
        assert!(!names("valkey-cli", "valkey"));
    }

    #[test]
    fn a_test_is_named_by_its_category_and_name_alone() {
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lab/tests");
        assert!(test_text(&tests, "services/restart").is_some());
        assert_eq!(test_text(&tests, "services/../services/restart"), None);
        assert_eq!(test_text(&tests, "services"), None);
        assert_eq!(test_text(&tests, "/services/restart"), None);
    }

    #[test]
    fn a_test_s_text_holds_the_scripts_it_uses() {
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lab/tests");
        let nginx = test_text(&tests, "services/nginx").unwrap();
        assert!(nginx.contains("{here}/nginx-load.sh"));
        let script = std::fs::read_to_string(tests.join("services/nginx-load.sh")).unwrap();
        assert!(nginx.ends_with(&script));
    }

    #[test]
    fn a_page_named_only_in_the_browser_script_is_covered() {
        let tests = std::env::temp_dir().join(format!("ob-lab-coverage-{}", std::process::id()));
        let folder = tests.join("web-console");
        std::fs::create_dir_all(&folder).unwrap();
        let toml = "[[step]]\nbrowser = \"overview.mjs\"\nengines = [\"chromium\"]\n";
        std::fs::write(folder.join("overview.toml"), toml).unwrap();
        std::fs::write(folder.join("overview.mjs"), "await t.open(\"/overview\");\n").unwrap();
        assert!(!names(toml, "/overview"), "the test file alone must not name the page");

        let listed = entries("[pages]\n\"/overview\" = [\"web-console/overview\"]\n");
        let covered =
            audit(&BTreeSet::new(), &BTreeSet::new(), &listed, &|id| test_text(&tests, id));
        assert_eq!(covered.problems, Vec::<String>::new());
        assert_eq!(covered.covered, 1);

        // A script the test does not name as its own covers nothing.
        std::fs::write(folder.join("overview.toml"), "[[step]]\nrun = \"true\"\n").unwrap();
        let bare = audit(&BTreeSet::new(), &BTreeSet::new(), &listed, &|id| test_text(&tests, id));
        assert_eq!(
            bare.problems,
            ["the page `/overview`: lab/tests/web-console/overview.toml never names it"]
        );
        std::fs::remove_dir_all(&tests).unwrap();
    }

    #[test]
    fn reads_what_there_was_before_phase_16() {
        let before = parse_before("# The commands.\ncommand service\n\nservice logd\n").unwrap();
        assert_eq!(before, items(&[(Kind::Command, "service"), (Kind::Service, "logd")]));
        assert_eq!(
            parse_before("program spark\n"),
            Err("line 1: `program spark` is not a kind (command, service, page or route) and a \
                 name"
                .into())
        );
    }
}
