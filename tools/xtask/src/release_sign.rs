// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob release sign VERSION` and `ob release record VERSION`: the
//! project's release signed on a YubiKey, after three builds agree and
//! every gate is green (ADRs 0450, 0457 and 0778), and its signature
//! recorded in Sigstore's public Rekor log, as the signature of its
//! install images' checksums is ([`crate::release_images`]).
//!
//! `sign` refuses unless the checkout has nothing changed, builds this
//! version, and its commit is on main or a release branch on GitHub with
//! CI passed, a full run of the update test matrix and a reproducible
//! run green. It downloads the two builds of that run, builds the release
//! a third time here, and refuses unless all three are the same. Then
//! system-release fetches what updates.dukovo.org serves into
//! `out/release-sign/VERSION`, shows what it is about to sign, asks for
//! the version to be typed, and signs the release and the channel's move
//! on the YubiKey with a PIN and a touch. The repository is deployed with
//! `./ob release deploy`, as any other.
//!
//! When it stops, the message says which gate or build it was: a gate's
//! run is on GitHub's Actions page for the commit, and a build that
//! differs from CI's is read with `diff` of the two `files.txt`, which
//! are left in `out/release-sign/ci-COMMIT/`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::catalog_pr::{GITHUB, gh_ready};
use crate::process::{self, step};
use crate::release::{self, BUILD_FILES, BUILD_TREE, REPO, URL};
use crate::{Context, Ctx, Error, Result, VERSION, bail};

/// Sigstore's public Rekor log. `$DUKOVO_REKOR` names another, such as
/// its staging log at `https://rekor.sigstage.dev`, for a trial.
const REKOR: &str = "https://rekor.sigstore.dev";

/// The name a full run of the update test matrix is given
/// (`.github/workflows/update-matrix.yml`).
const MATRIX_FULL: &str = "Update matrix, full";

/// The artifacts of the reproducible workflow's two builds, which hold
/// [`BUILD_FILES`] each.
const CI_BUILDS: [&str; 2] = ["reproducible-first", "reproducible-second"];

/// What `sign` is asked for.
#[derive(Debug, PartialEq, Eq)]
struct Request<'a> {
    version: &'a str,
    channel: &'a str,
    serial: Option<&'a str>,
}

/// `VERSION [--channel CHANNEL] [--serial N]`: a release goes on the beta
/// channel unless another is named (ADR 0456).
fn request<'a>(args: &[&'a str]) -> Result<Request<'a>> {
    let usage = "usage: ./ob release sign <version> [--channel <channel>] [--serial <number>]";
    let [version, rest @ ..] = args else { bail!("{usage}") };
    let mut request = Request { version, channel: "beta", serial: None };
    let mut rest = rest.iter();
    while let Some(option) = rest.next() {
        match (*option, rest.next()) {
            ("--channel", Some(channel)) => request.channel = channel,
            ("--serial", Some(serial)) => request.serial = Some(serial),
            _ => bail!("{usage}"),
        }
    }
    if version.starts_with('-') {
        bail!("{usage}");
    }
    Ok(request)
}

/// Where `sign` leaves what it makes: the repository of each version, its
/// Rekor entry and the log's answer, and CI's builds.
fn sign_dir(ctx: &Ctx) -> PathBuf {
    ctx.out.join("release-sign")
}

/// `ob release sign VERSION [--channel CHANNEL] [--serial N]`.
pub fn sign(ctx: &Ctx, args: &[&str]) -> Result<()> {
    let request = request(args)?;
    if request.version != VERSION {
        bail!(
            "this checkout builds Dukovo {VERSION}, not {}: a release is signed from the commit \
             that bumps the version to it",
            request.version
        );
    }
    release::production_root(ctx)?;
    let repo = sign_dir(ctx).join(request.version);
    if repo.exists() {
        bail!(
            "{} is there already: deploy it with `./ob release deploy {}`, or remove it to sign \
             again",
            ctx.display(&repo),
            ctx.display(&repo)
        );
    }
    unchanged(ctx)?;
    let commit = text(ctx, git(ctx).args(["rev-parse", "HEAD"]))?;
    gh_ready(ctx, "the gates are read from GitHub")?;
    step(&format!("Checking the gates of commit {commit} on GitHub"));
    let branch = merged(ctx, &commit)?;
    ci_passed(ctx, &commit)?;
    matrix_passed(ctx, &commit, &branch)?;
    let ci = ci_builds(ctx, &commit, &branch)?;
    step("Building the release a third time, on this machine");
    let built = ctx.out.join("release-build");
    let digest = release::build_release(ctx, &built)?;
    same_builds(&built, &ci)?;
    step(&format!("The three builds agree: the release list's SHA-256 is {digest}"));
    if let Err(err) = offer(ctx, &request, &repo) {
        // Nothing of it is deployed, and signing again starts from what
        // the server serves.
        let _ = std::fs::remove_dir_all(&repo);
        return Err(err);
    }
    let shown = ctx.display(&repo);
    let (entry, answer) = entry_paths(ctx, request.version);
    let what = format!("Dukovo {}", request.version);
    record_entry(ctx, request.version, &what, &entry, &answer).map_err(|err| {
        err.hint(format!(
            "the release is signed: `./ob release record {}` records it, and `./ob release deploy \
             {shown}` deploys it",
            request.version
        ))
    })?;
    println!("Deploy it with `./ob release deploy {shown}`.");
    Ok(())
}

/// Refuses unless the checkout has nothing changed, so that the release is
/// its commit's.
pub(crate) fn unchanged(ctx: &Ctx) -> Result<()> {
    let changes = text(ctx, git(ctx).args(["status", "--porcelain"]))?;
    if !changes.is_empty() {
        bail!("the checkout has changes: a release is signed from its commit alone\n{changes}");
    }
    Ok(())
}

/// The name of the branch on GitHub that holds `commit`, main or a
/// release branch: only a commit that merged is signed.
fn merged(ctx: &Ctx, commit: &str) -> Result<String> {
    let branches = gh(ctx, &format!("repos/{GITHUB}/branches?per_page=100"), ".[].name")?;
    let candidates = std::iter::once("main")
        .chain(branches.iter().map(String::as_str).filter(|b| b.starts_with("release/")));
    for branch in candidates {
        let status = gh(ctx, &format!("repos/{GITHUB}/compare/{branch}...{commit}"), ".status")?;
        if status.first().is_some_and(|status| holds(status)) {
            return Ok(branch.to_string());
        }
    }
    bail!(
        "commit {commit} is on neither main nor a release branch on GitHub: only a commit that \
         merged is signed"
    )
}

/// Whether a branch holds a commit, by the status of GitHub's comparison
/// of the branch with the commit.
fn holds(status: &str) -> bool {
    matches!(status, "identical" | "behind")
}

/// Refuses unless CI passed on `commit`.
fn ci_passed(ctx: &Ctx, commit: &str) -> Result<()> {
    let path = format!("repos/{GITHUB}/commits/{commit}/check-runs?check_name=CI%20passed");
    if !gh(ctx, &path, ".check_runs[].conclusion")?.iter().any(|c| c == "success") {
        bail!("CI has not passed on commit {commit}");
    }
    Ok(())
}

/// Refuses unless a full run of the update test matrix passed on `commit`
/// (ADR 0536).
fn matrix_passed(ctx: &Ctx, commit: &str, branch: &str) -> Result<()> {
    let path = format!(
        "repos/{GITHUB}/actions/workflows/update-matrix.yml/runs?head_sha={commit}&status=success"
    );
    if !gh(ctx, &path, ".workflow_runs[].display_title")?.iter().any(|t| t == MATRIX_FULL) {
        return Err(Error::new(format!(
            "no full run of the update test matrix passed on commit {commit}"
        ))
        .hint(format!(
            "while {branch} is at that commit: gh workflow run update-matrix.yml -R {GITHUB} \
             --ref {branch} -f full=true"
        )));
    }
    Ok(())
}

/// The folders CI's two builds of `commit` were downloaded into, from the
/// newest run of the reproducible workflow that passed on it: they were
/// the same, or it would not have passed.
fn ci_builds(ctx: &Ctx, commit: &str, branch: &str) -> Result<[PathBuf; 2]> {
    let path = format!(
        "repos/{GITHUB}/actions/workflows/reproducible.yml/runs?head_sha={commit}&status=success"
    );
    let rerun = format!(
        "while {branch} is at that commit: gh workflow run reproducible.yml -R {GITHUB} --ref \
         {branch}; its builds are kept 7 days"
    );
    let Some(run) = gh(ctx, &path, ".workflow_runs[].id")?.into_iter().next() else {
        return Err(Error::new(format!(
            "no reproducible run passed on commit {commit}: CI has not built it twice"
        ))
        .hint(rerun));
    };
    let dir = sign_dir(ctx).join(format!("ci-{}", &commit[..commit.len().min(12)]));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context(|| format!("removing {}", dir.display()))?;
    }
    step(&format!("Downloading CI's two builds, from run {run}"));
    let download = |name: &str| {
        let folder = dir.join(name);
        let mut cmd = Command::new("gh");
        cmd.current_dir(&ctx.root).args(["run", "download", &run, "-R", GITHUB, "-n", name, "-D"]);
        process::output(ctx, cmd.arg(&folder)).map_err(|err| err.hint(rerun.clone()))?;
        Ok(folder)
    };
    Ok([download(CI_BUILDS[0])?, download(CI_BUILDS[1])?])
}

/// Refuses unless the build in `built` is the same as each of CI's in
/// `ci`: the release list's SHA-256 and the list of the tree, as the
/// reproducible workflow compares its two (ADR 0535).
fn same_builds(built: &Path, ci: &[PathBuf; 2]) -> Result<()> {
    let [files, _, sum] = BUILD_FILES;
    for (folder, name) in ci.iter().zip(CI_BUILDS) {
        for file in [sum, files] {
            let read = |path: PathBuf| {
                std::fs::read(&path).context(|| format!("reading {}", path.display()))
            };
            if read(built.join(file))? != read(folder.join(file))? {
                return Err(Error::new(format!(
                    "the release this machine built is not CI's ({name}): their {file} differ, \
                     and nothing was signed"
                ))
                .hint(format!(
                    "diff {} {} shows what differs",
                    folder.join(files).display(),
                    built.join(files).display()
                )));
            }
        }
    }
    Ok(())
}

/// system-release fetches what the update server serves into `repo`, and
/// offers the release built there on the request's channel, signed on the
/// YubiKey, with the security advisories, writing the Rekor entry beside
/// it.
fn offer(ctx: &Ctx, request: &Request, repo: &Path) -> Result<()> {
    let metadata = ctx.root.join(REPO).join("metadata");
    let mut fetch = Command::new(release::tool(ctx)?);
    // A root the source tree has and the server does not serve yet goes
    // out with the release (ADR 0780).
    fetch
        .arg("fetch")
        .arg("--root")
        .arg(metadata.join("1.root.json"))
        .arg("--roots")
        .arg(&metadata)
        .arg("--out")
        .arg(repo)
        .arg(URL);
    process::run(ctx, &mut fetch)?;
    let mut offer = Command::new(release::tool(ctx)?);
    offer
        .arg("offer")
        .arg("--repo")
        .arg(repo)
        .arg("--yubikey-helper")
        .arg(ctx.root.join(crate::catalog::YUBIKEY_HELPER))
        .args(request.serial.map(|serial| ["--serial", serial]).into_iter().flatten())
        .arg("--tree")
        .arg(ctx.out.join(BUILD_TREE))
        .arg("--advisories")
        .arg(ctx.root.join(crate::security::DIR))
        .arg("--record")
        .arg(entry_paths(ctx, request.version).0)
        .arg(ctx.out.join("release-build").join(BUILD_FILES[1]))
        .arg(request.channel);
    // The terminal, for the version to be typed and the YubiKey's PIN.
    process::run(ctx, &mut offer)
}

/// The Rekor entry of `version`'s signature, which `sign` writes, and
/// where the log's answer is kept.
fn entry_paths(ctx: &Ctx, version: &str) -> (PathBuf, PathBuf) {
    let dir = sign_dir(ctx);
    (dir.join(format!("{version}.rekor-entry.json")), dir.join(format!("{version}.rekor.json")))
}

/// `ob release record VERSION`: sends the Rekor entries of `version`'s
/// signatures to the log, the release's and its install images'
/// checksums', each that is there and whose answer is not kept already,
/// and keeps each answer beside it. The log holding an entry already is
/// its answer too.
pub fn record(ctx: &Ctx, version: &str) -> Result<()> {
    let entries = [
        (format!("Dukovo {version}"), entry_paths(ctx, version)),
        (
            format!("the checksums of Dukovo {version}'s install images"),
            crate::release_images::entry_paths(ctx, version),
        ),
    ];
    let mut recorded = false;
    for (what, (entry, answer)) in entries {
        if entry.is_file() {
            record_entry(ctx, version, &what, &entry, &answer)?;
            recorded = true;
        }
    }
    if !recorded {
        bail!(
            "nothing of Dukovo {version} is signed here: `./ob release sign {version}` and \
             `./ob release images {version}` write the entries"
        );
    }
    Ok(())
}

/// Sends the Rekor entry `entry` of the signature of `what`, of release
/// `version`, to the log, unless its answer is kept in `answer` already,
/// and keeps the answer there.
pub(crate) fn record_entry(
    ctx: &Ctx,
    version: &str,
    what: &str,
    entry: &Path,
    answer: &Path,
) -> Result<()> {
    if !entry.is_file() {
        bail!("{} is not there", ctx.display(entry));
    }
    if !answer.is_file() {
        let log = std::env::var("DUKOVO_REKOR").unwrap_or_else(|_| REKOR.to_string());
        step(&format!("Recording the signature of {what} in the Rekor log at {log}"));
        send(ctx, &log, entry, answer)
            .map_err(|err| err.hint(format!("./ob release record {version} sends it again")))?;
    }
    let text = std::fs::read(answer).context(|| format!("reading {}", answer.display()))?;
    let Some(index) = log_index(&text) else {
        bail!("{} is not an answer of the Rekor log: remove it and record again", answer.display())
    };
    println!(
        "The signature of {what} is entry {index} of the Rekor log: \
         https://search.sigstore.dev/?logIndex={index}"
    );
    Ok(())
}

/// POSTs `entry` to the Rekor log at `log` and writes its answer to
/// `answer`: the entry as logged, or, when the log holds it already, that
/// entry as the log gives it at the address it names.
fn send(ctx: &Ctx, log: &str, entry: &Path, answer: &Path) -> Result<()> {
    let partial = answer.with_extension("partial");
    let headers = answer.with_extension("headers");
    let mut post = Command::new("curl");
    post.args(["--proto", "=https", "--tlsv1.2", "-sS", "--retry", "3", "-w", "%{http_code}"])
        .args(["-H", "Content-Type: application/json", "--data-binary"])
        .arg(format!("@{}", entry.display()))
        .arg("-D")
        .arg(&headers)
        .arg("-o")
        .arg(&partial)
        .arg(format!("{log}/api/v1/log/entries"));
    let status = text(ctx, &mut post)?;
    let said = std::fs::read_to_string(&partial).unwrap_or_default();
    match status.as_str() {
        "201" => {}
        "409" => {
            let heard = std::fs::read_to_string(&headers).unwrap_or_default();
            let Some(at) = location(&heard) else {
                bail!("the Rekor log holds the entry already, and did not say where: {said}")
            };
            let at = if at.starts_with('/') { format!("{log}{at}") } else { at };
            let mut get = Command::new("curl");
            get.args(["--proto", "=https", "--tlsv1.2", "-fsS", "--retry", "3", "-o"])
                .arg(&partial)
                .arg(&at);
            process::output(ctx, &mut get)?;
        }
        other => bail!("the Rekor log answered {other}, and did not record the signature: {said}"),
    }
    let _ = std::fs::remove_file(&headers);
    std::fs::rename(&partial, answer).context(|| format!("writing {}", answer.display()))
}

/// The `Location` header among `headers`, as curl writes them.
fn location(headers: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location").then(|| value.trim().to_string())
    })
}

/// The log index of the one entry in a Rekor answer, which is an object
/// of that entry by its UUID.
fn log_index(answer: &[u8]) -> Option<u64> {
    let answer: serde_json::Value = serde_json::from_slice(answer).ok()?;
    let entries = answer.as_object()?;
    match entries.values().collect::<Vec<_>>().as_slice() {
        [entry] => entry["logIndex"].as_u64(),
        _ => None,
    }
}

/// What `gh api PATH --paginate --jq JQ` prints, line by line.
fn gh(ctx: &Ctx, path: &str, jq: &str) -> Result<Vec<String>> {
    let mut cmd = Command::new("gh");
    cmd.current_dir(&ctx.root).args(["api", "--paginate", path, "--jq", jq]);
    Ok(text(ctx, &mut cmd)?.lines().map(str::to_string).collect())
}

fn git(ctx: &Ctx) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(&ctx.root);
    cmd
}

/// What `cmd` prints, trimmed.
fn text(ctx: &Ctx, cmd: &mut Command) -> Result<String> {
    let output = process::output(ctx, cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_goes_on_beta_unless_another_channel_is_named() {
        let beta = Request { version: "1.0.0-alpha.1", channel: "beta", serial: None };
        assert_eq!(request(&["1.0.0-alpha.1"]).unwrap(), beta);
        let named = request(&["1.0.0", "--serial", "123", "--channel", "stable"]).unwrap();
        assert_eq!(named, Request { version: "1.0.0", channel: "stable", serial: Some("123") });
        for wrong in [&[][..], &["--serial", "1"], &["1.0.0", "--serial"], &["1.0.0", "x"]] {
            assert!(request(wrong).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn a_branch_holds_a_commit_it_is_at_or_past() {
        assert!(holds("identical") && holds("behind"));
        assert!(!holds("ahead") && !holds("diverged"));
    }

    #[test]
    fn the_log_says_where_an_entry_it_holds_is_and_its_index() {
        let headers = "HTTP/2 409\r\ncontent-type: application/json\r\nLocation: \
                       /api/v1/log/entries/24296fb2\r\n\r\n";
        assert_eq!(location(headers).as_deref(), Some("/api/v1/log/entries/24296fb2"));
        assert_eq!(location("HTTP/2 201\r\n\r\n"), None);
        let answer = br#"{"24296fb2": {"logIndex": 151252, "integratedTime": 1795000000}}"#;
        assert_eq!(log_index(answer), Some(151_252));
        assert_eq!(log_index(br#"{"a": {"logIndex": 1}, "b": {"logIndex": 2}}"#), None);
        assert_eq!(log_index(b"<html>"), None);
    }

    #[test]
    fn three_builds_agree_only_on_the_same_list_and_tree() {
        let dir = std::env::temp_dir().join(format!("ob-release-sign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |folder: &str, sum: &str| {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join("files.txt"), "f\t0644\tab\t1\t0\tx\n").unwrap();
            std::fs::write(dir.join(folder).join("system.json.sha256"), sum).unwrap();
        };
        for folder in ["built", "first", "second"] {
            write(folder, "ab  system.json\n");
        }
        let ci = [dir.join("first"), dir.join("second")];
        same_builds(&dir.join("built"), &ci).unwrap();
        write("second", "cd  system.json\n");
        let error = same_builds(&dir.join("built"), &ci).unwrap_err().to_string();
        assert!(error.contains("reproducible-second") && error.contains("system.json.sha256"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
