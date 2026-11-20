// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob catalog pr`: records in a pull request the catalog this tree
//! signed and updates.dukovo.org now serves (ADR 0594), so that main
//! holds what computers are served and the next signing follows it.
//!
//! The commit is built on `origin/main` with an index of its own, from
//! the files of `catalog/keys/` and `catalog/metadata/` that differ from
//! main's: HEAD, the branch, the index and the working tree are left as
//! they are, so it works on whatever branch the checkout is on. Only the
//! signed files are then staged, so that a later `git pull --ff-only`
//! takes main's identical copies instead of stopping on them. `gh` opens
//! the pull request and turns on auto-merge.

use std::path::Path;
use std::process::Command;

use crate::catalog::{self, Versions};
use crate::process::{self, step};
use crate::{Context, Ctx, Error, Result, bail};

/// The repository the pull request goes to.
const GITHUB: &str = "dukovo-os/dukovo";

/// The branch it goes into, as fetched.
const MAIN: &str = "origin/main";

/// The folders a signing changes.
const SIGNED: [&str; 2] = ["catalog/keys", "catalog/metadata"];

/// The metadata files whose versions say what was signed.
const ROOT: &str = "catalog/metadata/root.json";
const TARGETS: &str = "catalog/metadata/targets.json";

/// What a pull request records: the targets and the root signed since
/// main's, each its new version, or `None` when main's is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Signed {
    targets: Option<u64>,
    root: Option<u64>,
}

impl Signed {
    /// The branch it is proposed from.
    fn branch(self) -> String {
        match (self.targets, self.root) {
            (Some(targets), _) => format!("catalog-targets-{targets}"),
            (None, Some(root)) => format!("catalog-root-{root}"),
            (None, None) => String::new(),
        }
    }

    /// The commit's and the pull request's title.
    fn subject(self) -> String {
        match (self.targets, self.root) {
            (Some(targets), Some(root)) => {
                format!("Sign the catalog's targets version {targets}, and root version {root}")
            }
            (Some(targets), None) => format!("Sign the catalog's targets version {targets}"),
            (None, Some(root)) => format!("Sign the catalog's root version {root}"),
            (None, None) => String::new(),
        }
    }

    /// The line that says it is live, which Dukovo Ops reads before the
    /// pull request's URL.
    fn live(self, open: bool) -> String {
        let which = if open { "this open pull request" } else { "this pull request" };
        match (self.targets, self.root) {
            (Some(targets), _) => {
                format!("Targets version {targets} is live, and {which} records it:")
            }
            (None, Some(root)) => format!("Root version {root} is live, and {which} records it:"),
            (None, None) => String::new(),
        }
    }
}

/// What this tree signed, against main's catalog: `tree` and `main` the
/// versions, and whether the targets and the root of the same version
/// are the same bytes.
fn signed_since(tree: Versions, main: Versions, same: (bool, bool)) -> Result<Signed> {
    let (same_targets, same_root) = same;
    if tree.targets < main.targets || tree.root < main.root {
        bail!(
            "catalog/ is behind main (targets version {} and root {}, main has {} and {}): pull \
             main first",
            tree.targets,
            tree.root,
            main.targets,
            main.root
        );
    }
    if tree.targets == main.targets && !same_targets {
        bail!(
            "targets version {} on main is other bytes than catalog/'s: one of them was signed \
             elsewhere; keep main's and sign again",
            main.targets
        );
    }
    if tree.root == main.root && !same_root {
        bail!("root version {} on main is other bytes than catalog/'s", main.root);
    }
    let signed = Signed {
        targets: (tree.targets > main.targets).then_some(tree.targets),
        root: (tree.root > main.root).then_some(tree.root),
    };
    if signed.targets.is_none() && signed.root.is_none() {
        bail!("catalog/ differs from main but signs no new version: commit what changed by hand");
    }
    Ok(signed)
}

/// Refuses unless updates.dukovo.org serves what this tree signed: the
/// pull request records only what is live.
fn is_live(tree: Versions, served: Versions) -> Result<()> {
    if served.targets < tree.targets || served.root < tree.root {
        bail!(
            "targets version {} is not live yet ({} serves targets version {}): ./ob catalog \
             deploy",
            tree.targets,
            catalog::HOST,
            served.targets
        );
    }
    if served != tree {
        bail!(
            "{} serves targets version {} and root {}, newer than catalog/'s: pull main first",
            catalog::HOST,
            served.targets,
            served.root
        );
    }
    Ok(())
}

/// The commit message: the subject, what the targets changed since
/// main's, as `catalog-repo since` lists it, and the sign-off.
fn message(signed: Signed, lines: &[String], identity: &str) -> String {
    let mut text = format!("{}\n\n", signed.subject());
    match signed.targets {
        Some(targets) => {
            text.push_str(&format!("Targets version {targets}, live on {}:\n\n", catalog::HOST));
            for line in lines {
                text.push_str(&format!("{line}\n"));
            }
            if lines.is_empty() {
                text.push_str("No bundle changes: the targets are renewed.\n");
            }
        }
        None => text.push_str(&format!("Live on {}.\n", catalog::HOST)),
    }
    text.push_str(&format!("\nSigned-off-by: {identity}\n"));
    text
}

/// The pull request's description, in the repository's template.
fn description(signed: Signed, lines: &[String], verified: &str, superseded: &[String]) -> String {
    let mut text = String::from("## What changes\n\n");
    match signed.targets {
        Some(targets) => {
            text.push_str(&format!(
                "The catalog's targets version {targets}, which {} serves now",
                catalog::HOST
            ));
            if lines.is_empty() {
                text.push_str(": the same bundles, good for longer.\n");
            } else {
                text.push_str(":\n\n");
                for line in lines {
                    text.push_str(&format!("- `{line}`\n"));
                }
            }
        }
        None => {
            text.push_str(&format!(
                "The catalog's new root, which {} serves now.\n",
                catalog::HOST
            ));
        }
    }
    if let Some(root) = signed.root {
        text.push_str(&format!("\nIt also records root version {root}.\n"));
    }
    text.push_str(&format!(
        "\n## Why\n\nmain records what {} serves, so that the next signing follows it.\n",
        catalog::HOST
    ));
    for pull in superseded {
        text.push_str(&format!(
            "\nThis supersedes {pull}, which can be closed once this merges.\n"
        ));
    }
    text.push_str(&format!(
        "\n## How it was tested\n\n- Signed offline, published and read back by `./ob catalog \
         deploy`, as a computer reads it from the first root.\n- `./ob catalog verify`: \
         {verified}\n\n## Checklist\n\n- [x] Every commit is signed off (`git commit -s`)\n- [x] \
         No code changes: CI checks the signed metadata with `./ob catalog verify`\n"
    ));
    text
}

/// `ob catalog pr`.
pub fn propose(ctx: &Ctx) -> Result<()> {
    step("Checking catalog/ as a computer checks it");
    let verified = catalog::verify(ctx)?;
    println!("{verified}");
    step("Fetching main");
    run(ctx, git(ctx).args(["fetch", "--quiet", "origin", "main"]))?;
    let base = text(ctx, git(ctx).args(["rev-parse", "--verify", &format!("{MAIN}^{{commit}}")]))?;
    let files = changed_files(ctx, &base)?;
    if files.is_empty() {
        println!("catalog/ is main's: nothing to propose");
        return Ok(());
    }
    // A file main has that this tree does not is one this tree has not
    // pulled yet, never one to take out of main.
    if let Some(missing) = files.iter().find(|file| !ctx.root.join(file).is_file()) {
        bail!("catalog/ has no {missing}, which main has: pull main first");
    }
    let tree = Versions::of_tree(ctx)?;
    let main_targets = file_at(ctx, &base, TARGETS)?;
    let main_root = file_at(ctx, &base, ROOT)?;
    let main = Versions::of(main_root.as_deref(), main_targets.as_deref())?;
    let read = |path: &str| std::fs::read(ctx.root.join(path)).ok();
    let same = (read(TARGETS) == main_targets, read(ROOT) == main_root);
    let signed = signed_since(tree, main, same)?;
    step(&format!("Reading what {} serves", catalog::HOST));
    is_live(tree, catalog::served_versions(ctx)?)?;
    let identity = identity(ctx)?;
    gh_ready(ctx)?;

    let branch = signed.branch();
    if let Some(url) = open_pull(ctx, &branch)? {
        stage(ctx, &files)?;
        println!("{}", signed.live(true));
        println!("{url}");
        return Ok(());
    }
    let remote = text(
        ctx,
        git(ctx).args(["ls-remote", "--heads", "origin", &format!("refs/heads/{branch}")]),
    )?;
    if !remote.is_empty() {
        bail!(
            "the branch {branch} is on GitHub with no open pull request: open one for it, or \
             delete it and run ./ob catalog pr again"
        );
    }
    let lines = since_main(ctx, main_targets.as_deref())?;
    let message = message(signed, &lines, &identity);
    let dir = ctx.out.join("catalog");
    process::create_dir(&dir)?;
    let commit = commit(ctx, &base, &files, &message)?;
    step(&format!("Pushing {branch}"));
    run(
        ctx,
        git(ctx).args(["push", "--quiet", "origin", &format!("{commit}:refs/heads/{branch}")]),
    )?;

    let superseded = superseded(ctx, &branch)?;
    let body = dir.join("pr.body");
    std::fs::write(&body, description(signed, &lines, &verified, &superseded))
        .context(|| format!("writing {}", body.display()))?;
    step("Opening the pull request");
    let mut create = Command::new("gh");
    create.current_dir(&ctx.root).args(["pr", "create", "--repo", GITHUB, "--base", "main"]);
    create.args(["--head", &branch, "--title", &signed.subject(), "--body-file"]).arg(&body);
    let url = text(ctx, &mut create)
        .map_err(|err| {
            err.hint(format!(
                "{branch} is pushed: open its pull request on GitHub, or delete the branch and run \
                 ./ob catalog pr again"
            ))
        })?
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    // The squashed commit keeps the message, sign-off and all.
    let squashed = dir.join("pr.message");
    let (_, squashed_body) = message.split_once("\n\n").unwrap_or_default();
    std::fs::write(&squashed, squashed_body)
        .context(|| format!("writing {}", squashed.display()))?;
    let mut merge = Command::new("gh");
    merge.current_dir(&ctx.root).args(["pr", "merge", &url, "--auto", "--squash"]);
    merge.args(["--subject", &signed.subject(), "--body-file"]).arg(&squashed);
    if process::output(ctx, &mut merge).is_err() {
        eprintln!("ob: auto-merge is not on for {url}: merge it when its checks pass");
    }
    stage(ctx, &files)?;
    println!("{}", signed.live(false));
    println!("{url}");
    Ok(())
}

/// The files under [`SIGNED`] that differ from `base`'s, tracked or not,
/// as paths from the tree's root.
fn changed_files(ctx: &Ctx, base: &str) -> Result<Vec<String>> {
    let tracked = text(ctx, git(ctx).args(["diff", "--name-only", base, "--"]).args(SIGNED))?;
    let untracked = text(
        ctx,
        git(ctx).args(["ls-files", "--others", "--exclude-standard", "--"]).args(SIGNED),
    )?;
    let mut files: Vec<String> =
        tracked.lines().chain(untracked.lines()).map(str::to_string).collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// The bytes of `path` in the commit `base`, if it has that file.
fn file_at(ctx: &Ctx, base: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let listed = text(ctx, git(ctx).args(["ls-tree", "--name-only", base, "--", path]))?;
    if listed.is_empty() {
        return Ok(None);
    }
    let output =
        process::output(ctx, git(ctx).args(["cat-file", "blob", &format!("{base}:{path}")]))?;
    Ok(Some(output.stdout))
}

/// What the targets changed since main's `main_targets`, as `catalog-repo
/// since` lists it.
fn since_main(ctx: &Ctx, main_targets: Option<&[u8]>) -> Result<Vec<String>> {
    let mut cmd = catalog::catalog_repo(ctx)?;
    cmd.arg("since");
    if let Some(bytes) = main_targets {
        let file = ctx.out.join("catalog/main-targets.json");
        process::create_dir(&ctx.out.join("catalog"))?;
        std::fs::write(&file, bytes).context(|| format!("writing {}", file.display()))?;
        cmd.arg(&file);
    }
    Ok(text(ctx, &mut cmd)?.lines().map(str::to_string).collect())
}

/// The sign-off's `NAME <EMAIL>`, from git's settings.
fn identity(ctx: &Ctx) -> Result<String> {
    let setting = |key: &str| {
        let output = git(ctx).args(["config", key]).output().ok()?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !value.is_empty()).then_some(value)
    };
    match (setting("user.name"), setting("user.email")) {
        (Some(name), Some(email)) => Ok(format!("{name} <{email}>")),
        _ => Err(Error::new("git has no user.name and user.email: the commit is signed off with them")
            .hint("git config --global user.name 'Your Name'; git config --global user.email you@example.org")),
    }
}

/// Refuses unless `gh` is installed and signed in.
fn gh_ready(ctx: &Ctx) -> Result<()> {
    let hint = "install it with `sudo apt install gh`, then sign in with `gh auth login`";
    if process::find_program("gh").is_none() {
        return Err(Error::new(
            "the pull request is opened with GitHub's gh, which is not installed",
        )
        .hint(hint));
    }
    let mut status = Command::new("gh");
    status.current_dir(&ctx.root).args(["auth", "status", "--hostname", "github.com"]);
    if process::output(ctx, &mut status).is_err() {
        return Err(
            Error::new("gh is not signed in to GitHub").hint("sign in with `gh auth login`")
        );
    }
    Ok(())
}

/// The URL of the open pull request from `branch`, if there is one.
fn open_pull(ctx: &Ctx, branch: &str) -> Result<Option<String>> {
    let mut list = Command::new("gh");
    list.current_dir(&ctx.root).args(["pr", "list", "--repo", GITHUB, "--head", branch]);
    list.args(["--state", "open", "--json", "url", "--jq", ".[].url"]);
    Ok(text(ctx, &mut list)?.lines().next().map(str::to_string))
}

/// The open pull requests of other catalog signings, as "#N (BRANCH)",
/// which the new one supersedes. They are named, not closed: closing one
/// is for a person to do.
fn superseded(ctx: &Ctx, branch: &str) -> Result<Vec<String>> {
    let mut list = Command::new("gh");
    list.current_dir(&ctx.root).args(["pr", "list", "--repo", GITHUB, "--state", "open"]);
    list.args(["--json", "number,headRefName", "--jq", ".[] | \"\\(.number) \\(.headRefName)\""]);
    let listed = text(ctx, &mut list)?;
    Ok(older_signings(&listed, branch))
}

/// [`superseded`]'s rule over `gh`'s lines, `NUMBER BRANCH`.
fn older_signings(listed: &str, branch: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, head)| {
            (head.starts_with("catalog-targets-") || head.starts_with("catalog-root-"))
                && *head != branch
        })
        .map(|(number, head)| format!("#{number} ({head})"))
        .collect()
}

/// Commits `files` from the working tree onto `base` with `message`, in
/// an index of its own, and gives the commit's hash. HEAD, the branch,
/// the index and the working tree are not touched. Each file must be in
/// the working tree: a commit of a signing adds and changes files only.
fn commit(ctx: &Ctx, base: &str, files: &[String], message: &str) -> Result<String> {
    let index = ctx.out.join("catalog/pr.index");
    let _ = std::fs::remove_file(&index);
    let result = commit_in(ctx, &index, base, files, message);
    let _ = std::fs::remove_file(&index);
    result
}

fn commit_in(
    ctx: &Ctx,
    index: &Path,
    base: &str,
    files: &[String],
    message: &str,
) -> Result<String> {
    let in_index = || {
        let mut cmd = git(ctx);
        cmd.env("GIT_INDEX_FILE", index);
        cmd
    };
    run(ctx, in_index().args(["read-tree", base]))?;
    for file in files {
        let blob = text(ctx, git(ctx).args(["hash-object", "-w", "--"]).arg(ctx.root.join(file)))?;
        let info = format!("100644,{blob},{file}");
        run(ctx, in_index().args(["update-index", "--add", "--cacheinfo", &info]))?;
    }
    let tree = text(ctx, in_index().arg("write-tree"))?;
    let file = index.with_extension("message");
    std::fs::write(&file, message).context(|| format!("writing {}", file.display()))?;
    let commit = text(ctx, git(ctx).args(["commit-tree", &tree, "-p", base, "-F"]).arg(&file));
    let _ = std::fs::remove_file(&file);
    commit
}

/// Stages `files` in the checkout's own index: main's identical copies
/// then come in with `git pull --ff-only`, which stops on files changed
/// but not staged.
fn stage(ctx: &Ctx, files: &[String]) -> Result<()> {
    run(ctx, git(ctx).args(["add", "-A", "--"]).args(files))
}

fn git(ctx: &Ctx) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(&ctx.root);
    cmd
}

fn run(ctx: &Ctx, cmd: &mut Command) -> Result<()> {
    process::output(ctx, cmd).map(drop)
}

/// What `cmd` prints, trimmed.
fn text(ctx: &Ctx, cmd: &mut Command) -> Result<String> {
    let output = process::output(ctx, cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(root: u64, targets: u64) -> Versions {
        Versions { root, targets }
    }

    #[test]
    fn a_new_targets_version_is_proposed_from_its_own_branch() {
        let signed = signed_since(versions(1, 4), versions(1, 3), (false, true)).unwrap();
        assert_eq!(signed, Signed { targets: Some(4), root: None });
        assert_eq!(signed.branch(), "catalog-targets-4");
        assert_eq!(signed.subject(), "Sign the catalog's targets version 4");
        let both = signed_since(versions(2, 4), versions(1, 3), (false, false)).unwrap();
        assert_eq!(both.subject(), "Sign the catalog's targets version 4, and root version 2");
        assert_eq!(both.branch(), "catalog-targets-4");
        let root = signed_since(versions(2, 3), versions(1, 3), (true, false)).unwrap();
        assert_eq!(root.branch(), "catalog-root-2");
        assert_eq!(root.live(false), "Root version 2 is live, and this pull request records it:");
        assert_eq!(
            signed.live(true),
            "Targets version 4 is live, and this open pull request records it:"
        );
    }

    #[test]
    fn a_tree_behind_main_or_signed_twice_proposes_nothing() {
        let err = |tree, main, same| signed_since(tree, main, same).unwrap_err().to_string();
        assert!(err(versions(1, 2), versions(1, 3), (false, true)).contains("behind main"));
        assert!(err(versions(1, 3), versions(2, 3), (true, false)).contains("behind main"));
        assert!(err(versions(1, 3), versions(1, 3), (false, true)).contains("signed elsewhere"));
        assert!(err(versions(1, 4), versions(1, 4), (true, true)).contains("no new version"));
        assert!(err(versions(1, 4), versions(1, 3), (false, false)).contains("root version 1"));
    }

    #[test]
    fn only_what_is_live_is_proposed() {
        assert!(is_live(versions(1, 4), versions(1, 4)).is_ok());
        let err = is_live(versions(1, 4), versions(1, 3)).unwrap_err().to_string();
        assert!(err.contains("not live yet") && err.contains("./ob catalog deploy"), "{err}");
        let err = is_live(versions(1, 4), versions(1, 5)).unwrap_err().to_string();
        assert!(err.contains("pull main first"), "{err}");
        assert!(is_live(versions(2, 4), versions(1, 4)).is_err());
    }

    #[test]
    fn the_message_lists_the_changes_and_ends_with_the_sign_off() {
        let signed = Signed { targets: Some(4), root: None };
        let lines =
            vec!["+ bundles/org.dukovo.bat-0.26.1.dkv (org.dukovo.bat 0.26.1, 3 bytes)".into()];
        let text = message(signed, &lines, "Ann Example <ann@example.org>");
        assert!(
            text.starts_with("Sign the catalog's targets version 4\n\nTargets version 4, live on ")
        );
        assert!(text.contains(&format!("\n{}\n", lines[0])));
        assert!(text.ends_with("\n\nSigned-off-by: Ann Example <ann@example.org>\n"));
        let renewed = message(signed, &[], "A <a@b.c>");
        assert!(renewed.contains("No bundle changes"));
        let body =
            description(signed, &lines, "catalog: root 1, ...", &["#7 (catalog-targets-3)".into()]);
        for heading in ["## What changes", "## Why", "## How it was tested", "## Checklist"] {
            assert!(body.contains(heading), "{heading}");
        }
        assert!(body.contains(&format!("- `{}`", lines[0])));
        assert!(body.contains("supersedes #7 (catalog-targets-3)"));
    }

    #[test]
    fn older_signings_are_named_and_others_are_not() {
        let listed = "12 catalog-targets-3\n13 fix-typo\n14 catalog-targets-4\n15 catalog-root-2\n";
        assert_eq!(
            older_signings(listed, "catalog-targets-4"),
            ["#12 (catalog-targets-3)", "#15 (catalog-root-2)"]
        );
        assert!(older_signings("", "catalog-targets-4").is_empty());
    }

    /// A checkout in `dir/tree` of a repository whose `origin` is a bare
    /// one in `dir/origin.git`, with main holding catalog/ at targets
    /// version 3, and the checkout on a branch of its own.
    fn checkout(dir: &Path) -> Ctx {
        let git_in = |cwd: &Path, args: &[&str]| {
            let output = Command::new("git")
                .current_dir(cwd)
                .args(["-c", "user.name=Ann Example", "-c", "user.email=ann@example.org"])
                .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        git_in(dir, &["init", "--quiet", "--bare", "origin.git"]);
        git_in(dir, &["clone", "--quiet", "origin.git", "tree"]);
        let tree = dir.join("tree");
        let metadata = tree.join("catalog/metadata");
        std::fs::create_dir_all(&metadata).unwrap();
        std::fs::write(metadata.join("root.json"), r#"{"signed":{"version":1}}"#).unwrap();
        std::fs::write(metadata.join("targets.json"), r#"{"signed":{"version":3}}"#).unwrap();
        std::fs::write(tree.join("README.md"), "main\n").unwrap();
        git_in(&tree, &["add", "."]);
        git_in(&tree, &["commit", "--quiet", "-m", "main"]);
        git_in(&tree, &["push", "--quiet", "origin", "HEAD:main"]);
        git_in(&tree, &["checkout", "--quiet", "-b", "work"]);
        std::fs::write(tree.join("README.md"), "work\n").unwrap();
        git_in(&tree, &["commit", "--quiet", "-am", "work"]);
        git_in(&tree, &["fetch", "--quiet", "origin"]);
        Ctx {
            root: tree.clone(),
            out: tree.join("out"),
            target: dir.join("target"),
            config: crate::config::Config::default(),
            verbose: false,
        }
    }

    #[test]
    fn the_commit_is_mains_tree_and_the_signed_files_alone() {
        let dir = std::env::temp_dir().join(format!("ob-catalog-pr-{}", std::process::id()));
        let ctx = checkout(&dir);
        let metadata = ctx.root.join("catalog/metadata");
        std::fs::write(metadata.join("targets.json"), r#"{"signed":{"version":4}}"#).unwrap();
        std::fs::create_dir_all(ctx.root.join("catalog/keys")).unwrap();
        std::fs::write(ctx.root.join("catalog/keys/online.json"), "{}").unwrap();
        // Something else changed in the checkout stays out of it.
        std::fs::write(ctx.root.join("README.md"), "changed\n").unwrap();
        let git_text = |args: &[&str]| text(&ctx, git(&ctx).args(args)).unwrap();
        let (head, status) =
            (git_text(&["rev-parse", "HEAD"]), git_text(&["status", "--porcelain"]));
        let base = git_text(&["rev-parse", "origin/main"]);

        let files = changed_files(&ctx, &base).unwrap();
        assert_eq!(files, ["catalog/keys/online.json", "catalog/metadata/targets.json"]);
        assert_eq!(Versions::of_tree(&ctx).unwrap(), versions(1, 4));
        let main = Versions::of(
            file_at(&ctx, &base, ROOT).unwrap().as_deref(),
            file_at(&ctx, &base, TARGETS).unwrap().as_deref(),
        )
        .unwrap();
        assert_eq!(main, versions(1, 3));
        assert_eq!(file_at(&ctx, &base, "catalog/keys/online.json").unwrap(), None);

        let signed = Signed { targets: Some(4), root: None };
        let message = message(signed, &[], "Ann Example <ann@example.org>");
        process::create_dir(&ctx.out.join("catalog")).unwrap();
        let commit = commit(&ctx, &base, &files, &message).unwrap();
        assert_eq!(git_text(&["rev-parse", &format!("{commit}^")]), base);
        let changed = git_text(&["diff", "--name-only", &base, &commit]);
        assert_eq!(changed, "catalog/keys/online.json\ncatalog/metadata/targets.json");
        assert_eq!(git_text(&["show", &format!("{commit}:README.md")]), "main");
        assert_eq!(
            git_text(&["show", &format!("{commit}:catalog/metadata/targets.json")]),
            r#"{"signed":{"version":4}}"#
        );
        let logged = git_text(&["log", "-1", "--format=%B", &commit]);
        assert!(logged.ends_with("Signed-off-by: Ann Example <ann@example.org>"), "{logged}");
        // The checkout is as it was: HEAD, its branch, its index and files.
        assert_eq!(git_text(&["rev-parse", "HEAD"]), head);
        assert_eq!(git_text(&["rev-parse", "--abbrev-ref", "HEAD"]), "work");
        assert_eq!(git_text(&["status", "--porcelain"]), status);
        assert!(!ctx.out.join("catalog/pr.index").exists());

        // Staged, the signed files leave the rest of the checkout alone.
        stage(&ctx, &files).unwrap();
        let staged = git_text(&["diff", "--cached", "--name-only"]);
        assert_eq!(staged, "catalog/keys/online.json\ncatalog/metadata/targets.json");
        assert_eq!(git_text(&["diff", "--name-only"]), "README.md");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
