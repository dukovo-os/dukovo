// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The gate on the web console's actions (`ob lab coverage --check`): each
//! action in `lab/actions.toml` names five tests, and each must be in the
//! tree.
//!
//! The registry in `systools/src/admind/actions` makes the file (its host
//! test `actions_toml` writes it with `UPDATE_ACTIONS=1`), so a new action
//! is in it as soon as it is registered, and the gate fails until its tests
//! are written. The five are:
//!
//! - `checks`, a Rust module path under `systools/src`, such as
//!   `admind::actions::service::tests`, whose file declares `mod tests`
//!   with a `#[test]` in it;
//! - `route` and `same_as_command`, boot test cases by name;
//! - `browser` and `ansible`, lab tests by category and name, as
//!   `web-console/actions`, which are `lab/tests/web-console/actions.toml`.
//!
//! A test several actions name is looked for once and told once, with the
//! actions that name it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Context as _, Ctx, Error, Result};

/// The golden list of the actions.
pub(super) const ACTIONS: &str = "lab/actions.toml";

/// The folder of the host tests' modules.
const SYSTOOLS: &str = "userspace/programs/systools/src";

/// How many of the actions that name a missing test the gate names.
const NAMED: usize = 3;

/// `lab/actions.toml`: a table for each action.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    #[serde(default)]
    action: Vec<Action>,
}

/// An action, as the registry's host test writes it. Only its id and tests
/// are held to anything here; the rest is the registry's own record.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    id: String,
    #[allow(dead_code)] // Read so that the file is held to its form.
    title: String,
    #[allow(dead_code)] // As above.
    area: String,
    #[allow(dead_code)] // As above.
    method: String,
    #[allow(dead_code)] // As above.
    path: String,
    #[allow(dead_code)] // As above.
    runs: String,
    #[allow(dead_code)] // As above.
    long: bool,
    tests: Tests,
}

/// An action's five tests, by name.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tests {
    checks: String,
    route: String,
    same_as_command: String,
    browser: String,
    ansible: String,
}

/// What kind of test a name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Host,
    Boot,
    Lab,
}

/// What the check found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Audit {
    /// How many actions have all five tests.
    pub covered: usize,
    /// How many actions there are.
    pub total: usize,
    /// What is wrong, a sentence each.
    pub problems: Vec<String>,
}

/// Holds the actions of `lab/actions.toml` to the tests in the tree.
///
/// # Errors
///
/// If the file cannot be read or is not the registry's form.
pub fn check(ctx: &Ctx) -> Result<Audit> {
    let path = ctx.root.join(ACTIONS);
    let text = std::fs::read_to_string(&path).context(|| format!("reading {ACTIONS}"))?;
    let list: List =
        toml::from_str(&text).map_err(|err| Error::new(format!("{ACTIONS}: {err}")))?;
    let boot: Vec<&str> = crate::test::all_cases().map(|case| case.name).collect();
    let tests = ctx.root.join("lab/tests");
    let systools = ctx.root.join(SYSTOOLS);
    Ok(audit(
        &list,
        &|name| boot.contains(&name),
        &|id| super::coverage::test_text(&tests, id).is_some(),
        &|path| module_problem(&systools, path),
    ))
}

/// Holds `list` to the tests that exist: `boot` says whether a boot test
/// case is called so, `lab` whether a lab test of that category and name is
/// there, and `module` gives the reason a host test's module path is not
/// found, if it is not.
fn audit(
    list: &List,
    boot: &dyn Fn(&str) -> bool,
    lab: &dyn Fn(&str) -> bool,
    module: &dyn Fn(&str) -> Option<String>,
) -> Audit {
    let mut named: BTreeMap<(Kind, &str), Vec<&str>> = BTreeMap::new();
    for action in &list.action {
        let tests = &action.tests;
        let all = [
            (Kind::Host, &tests.checks),
            (Kind::Boot, &tests.route),
            (Kind::Boot, &tests.same_as_command),
            (Kind::Lab, &tests.browser),
            (Kind::Lab, &tests.ansible),
        ];
        for (kind, name) in all {
            let by = named.entry((kind, name.as_str())).or_default();
            if by.last() != Some(&action.id.as_str()) {
                by.push(&action.id);
            }
        }
    }
    let mut missing: BTreeMap<(Kind, &str), String> = BTreeMap::new();
    let mut problems = Vec::new();
    for (&(kind, name), by) in &named {
        let who = who_names(by);
        let problem = match kind {
            Kind::Host => module(name).map(|why| {
                format!("the host tests `{name}`, named by {who}, are not found: {why}")
            }),
            Kind::Boot => (!boot(name)).then(|| {
                format!("the boot test `{name}`, named by {who}, is not a boot test case")
            }),
            Kind::Lab => (!lab(name)).then(|| {
                format!(
                    "the lab test `{name}`, named by {who}, is not there: there is no \
                     lab/tests/{name}.toml"
                )
            }),
        };
        if let Some(problem) = problem {
            problems.push(problem);
            missing.insert((kind, name), name.to_string());
        }
    }
    let covered = list
        .action
        .iter()
        .filter(|action| {
            let tests = &action.tests;
            let all = [
                (Kind::Host, &tests.checks),
                (Kind::Boot, &tests.route),
                (Kind::Boot, &tests.same_as_command),
                (Kind::Lab, &tests.browser),
                (Kind::Lab, &tests.ansible),
            ];
            all.iter().all(|(kind, name)| !missing.contains_key(&(*kind, name.as_str())))
        })
        .count();
    Audit { covered, total: list.action.len(), problems }
}

/// "the action `a`", or "the actions `a`, `b`, `c` and 7 more".
fn who_names(ids: &[&str]) -> String {
    let shown: Vec<String> = ids.iter().take(NAMED).map(|id| format!("`{id}`")).collect();
    match ids.len() {
        1 => format!("the action {}", shown[0]),
        n if n <= NAMED => {
            let (last, first) = shown.split_last().expect("at least two");
            format!("the actions {} and {last}", first.join(", "))
        }
        n => format!("the actions {} and {} more", shown.join(", "), n - NAMED),
    }
}

/// Why `path`, a Rust module path such as `admind::actions::service::tests`,
/// is not the module of host tests of a file under `systools`, if it is
/// not: the modules before the last are files, the last is `tests`,
/// declared in the file of the one before it, and it holds a `#[test]`.
fn module_problem(systools: &Path, path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split("::").collect();
    let name_ok = |part: &&str| {
        part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
    };
    if parts.len() < 2 || !parts.iter().all(name_ok) {
        return Some("it is not a module path, as `admind::actions::service::tests`".into());
    }
    if parts.last() != Some(&"tests") {
        return Some("a module of host tests is called `tests`".into());
    }
    let parent = &parts[..parts.len() - 1];
    let Some(file) = module_file(systools, parent) else {
        return Some(format!(
            "there is no file of the module `{}` in {SYSTOOLS}",
            parent.join("::")
        ));
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Some(format!("{} does not read", file.display()));
    };
    let Some(inside) = declared(&text) else {
        return Some(format!("{} does not declare `mod tests`", file.display()));
    };
    let tests = match inside {
        Declared::Inline(body) => body.to_string(),
        Declared::File => tests_file(&file).unwrap_or_default(),
    };
    (!tests.contains("#[test]")).then(|| "the module has no `#[test]`".into())
}

/// The file of the module `parts`: `a/b/c.rs`, or `a/b/c/mod.rs`.
fn module_file(systools: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut folder = systools.to_path_buf();
    folder.extend(&parts[..parts.len() - 1]);
    let last = parts[parts.len() - 1];
    [folder.join(format!("{last}.rs")), folder.join(last).join("mod.rs")]
        .into_iter()
        .find(|file| file.is_file())
}

/// How a file declares `mod tests`.
enum Declared<'a> {
    /// With a body, which is what follows the declaration.
    Inline(&'a str),
    /// With a `;`: the module is another file.
    File,
}

/// The declaration of `mod tests` in `text`, with or without a visibility.
fn declared(text: &str) -> Option<Declared<'_>> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let code = line.trim_start();
        let code =
            code.strip_prefix("pub(crate) ").or_else(|| code.strip_prefix("pub ")).unwrap_or(code);
        if let Some(rest) = code.strip_prefix("mod tests") {
            match rest.trim_start().chars().next() {
                Some(';') => return Some(Declared::File),
                Some('{') => return Some(Declared::Inline(&text[at + line.len()..])),
                _ => {}
            }
        }
        at += line.len();
    }
    None
}

/// The text of the file of the module `tests` declared in `file`: next to
/// a file `NAME.rs` it is `NAME/tests.rs`, and next to a `mod.rs` it is
/// `tests.rs`.
fn tests_file(file: &Path) -> Option<String> {
    let folder = if file.file_name()? == "mod.rs" {
        file.parent()?.to_path_buf()
    } else {
        file.with_extension("")
    };
    [folder.join("tests.rs"), folder.join("tests").join("mod.rs")]
        .iter()
        .find_map(|candidate| std::fs::read_to_string(candidate).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(text: &str) -> List {
        toml::from_str(text).unwrap()
    }

    /// A table for the action `id` whose tests are named as `tests` says.
    fn action(id: &str, tests: [&str; 5]) -> String {
        let [checks, route, same, browser, ansible] = tests;
        format!(
            "[[action]]\nid = \"{id}\"\ntitle = \"T\"\narea = \"services\"\nmethod = \"GET\"\n\
             path = \"/api/1/x\"\nruns = \"as-the-person\"\nlong = false\n\
             tests = {{ checks = \"{checks}\", route = \"{route}\", same_as_command = \"{same}\", \
             browser = \"{browser}\", ansible = \"{ansible}\" }}\n"
        )
    }

    const GOOD: [&str; 5] = ["a::tests", "boot-one", "boot-two", "web/one", "web/two"];

    fn found(list: &List) -> Audit {
        audit(list, &|name| name.starts_with("boot-"), &|id| id.starts_with("web/"), &|path| {
            (!path.starts_with("a::")).then(|| "no such file".to_string())
        })
    }

    #[test]
    fn an_action_whose_five_tests_are_there_is_covered() {
        let audit = found(&list(&action("service.list", GOOD)));
        assert_eq!(audit, Audit { covered: 1, total: 1, problems: vec![] });
    }

    #[test]
    fn a_test_that_is_missing_is_told_once_with_the_actions_that_name_it() {
        let missing = ["b::tests", "none", "none", "gone/one", "gone/two"];
        let text = [
            action("service.list", missing),
            action("service.start", missing),
            action("service.stop", missing),
            action("service.restart", missing),
            action("certificate.list", GOOD),
        ]
        .concat();
        let audit = found(&list(&text));
        assert_eq!((audit.covered, audit.total), (1, 5));
        assert_eq!(
            audit.problems,
            [
                "the host tests `b::tests`, named by the actions `service.list`, `service.start`, \
                 `service.stop` and 1 more, are not found: no such file",
                "the boot test `none`, named by the actions `service.list`, `service.start`, \
                 `service.stop` and 1 more, is not a boot test case",
                "the lab test `gone/one`, named by the actions `service.list`, `service.start`, \
                 `service.stop` and 1 more, is not there: there is no lab/tests/gone/one.toml",
                "the lab test `gone/two`, named by the actions `service.list`, `service.start`, \
                 `service.stop` and 1 more, is not there: there is no lab/tests/gone/two.toml",
            ]
        );
    }

    #[test]
    fn one_missing_test_leaves_the_action_uncovered_and_the_others_covered() {
        let text = [
            action("service.list", ["a::tests", "boot-one", "boot-one", "web/one", "gone/one"]),
            action("service.start", GOOD),
        ]
        .concat();
        let audit = found(&list(&text));
        assert_eq!((audit.covered, audit.total), (1, 2));
        assert_eq!(
            audit.problems,
            [
                "the lab test `gone/one`, named by the action `service.list`, is not there: there is \
              no lab/tests/gone/one.toml"
            ]
        );
    }

    #[test]
    fn an_action_that_names_a_test_for_two_of_its_five_is_named_once() {
        let audit = found(&list(&action(
            "service.list",
            ["a::tests", "nope", "nope", "web/one", "web/two"],
        )));
        assert_eq!(
            audit.problems,
            ["the boot test `nope`, named by the action `service.list`, is not a boot test case"]
        );
    }

    #[test]
    fn the_actions_named_are_the_first_three_and_a_count() {
        assert_eq!(who_names(&["a"]), "the action `a`");
        assert_eq!(who_names(&["a", "b"]), "the actions `a` and `b`");
        assert_eq!(who_names(&["a", "b", "c"]), "the actions `a`, `b` and `c`");
        assert_eq!(who_names(&["a", "b", "c", "d"]), "the actions `a`, `b`, `c` and 1 more");
        assert_eq!(
            who_names(&["a", "b", "c", "d", "e", "f"]),
            "the actions `a`, `b`, `c` and 3 more"
        );
    }

    #[test]
    fn a_list_with_no_action_is_covered_and_a_list_of_another_form_is_not_read() {
        assert_eq!(found(&list("")), Audit::default());
        assert!(toml::from_str::<List>("[[action]]\nid = \"x\"\n").is_err());
        assert!(toml::from_str::<List>("[[action]]\nid = 1\n").is_err());
        let extra = action("x", GOOD) + "more = 1\n";
        assert!(toml::from_str::<List>(&extra).is_err());
    }

    /// A folder of its own with the files `(path, text)`, which `body`
    /// gets, and which is removed.
    fn tree(what: &str, files: &[(&str, &str)], body: impl FnOnce(&Path)) {
        let dir = std::env::temp_dir().join(format!("ob-actions-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let place = dir.join(path);
            std::fs::create_dir_all(place.parent().unwrap()).unwrap();
            std::fs::write(place, text).unwrap();
        }
        body(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_module_of_host_tests_is_a_file_that_declares_tests_with_a_test_in_it() {
        let inline = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_works() {}\n}\n";
        let outside = "#[cfg(test)]\npub mod tests;\n";
        let files = [
            ("a/b/inline.rs", inline),
            ("a/b/outside.rs", outside),
            ("a/b/outside/tests.rs", "#[test]\nfn it_works() {}\n"),
            ("a/b/mod.rs", "#[cfg(test)]\npub(crate) mod tests;\n"),
            ("a/b/tests.rs", "#[test]\nfn it_works() {}\n"),
            ("a/c/mod.rs", "#[cfg(test)]\nmod tests;\n"),
            ("a/c/tests/mod.rs", "#[test]\nfn it_works() {}\n"),
            ("a/empty.rs", "mod tests {\n    fn helper() {}\n}\n"),
            ("a/none.rs", "fn tests() {}\n// mod tests;\n"),
            ("a/emptyfile.rs", "mod tests;\n"),
            ("a/empty/tests.rs", "fn helper() {}\n"),
        ];
        tree("modules", &files, |dir| {
            for good in
                ["a::b::inline::tests", "a::b::outside::tests", "a::b::tests", "a::c::tests"]
            {
                assert_eq!(module_problem(dir, good), None, "{good}");
            }
            let problem = |path: &str| module_problem(dir, path).unwrap();
            assert_eq!(
                problem("tests"),
                "it is not a module path, as `admind::actions::service::tests`"
            );
            assert_eq!(
                problem("a::b::"),
                "it is not a module path, as `admind::actions::service::tests`"
            );
            assert_eq!(
                problem("a::1b::tests"),
                "it is not a module path, as `admind::actions::service::tests`"
            );
            assert_eq!(
                problem("a/b::tests"),
                "it is not a module path, as `admind::actions::service::tests`"
            );
            assert_eq!(problem("a::b::inline"), "a module of host tests is called `tests`");
            assert!(
                problem("a::nothing::tests")
                    .starts_with("there is no file of the module `a::nothing`")
            );
            assert!(problem("a::none::tests").contains("does not declare `mod tests`"));
            assert_eq!(problem("a::empty::tests"), "the module has no `#[test]`");
            assert_eq!(problem("a::emptyfile::tests"), "the module has no `#[test]`");
        });
    }

    #[test]
    fn the_golden_list_of_the_tree_reads_and_its_host_tests_are_in_the_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let text = std::fs::read_to_string(root.join(ACTIONS)).unwrap();
        let list: List = toml::from_str(&text).unwrap();
        assert!(!list.action.is_empty());
        for action in &list.action {
            let checks = &action.tests.checks;
            assert_eq!(module_problem(&root.join(SYSTOOLS), checks), None, "{}", action.id);
        }
    }
}
