// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob signoff`: every commit carries a Developer Certificate of Origin
//! sign-off from a person (ADR 0025, CONTRIBUTING.md).

use std::process::Command;

use crate::process::{self, step};
use crate::{Ctx, Error, Result};

/// The branch a pull request merges into, used when no base is given.
const DEFAULT_BASE: &str = "origin/main";

/// Identities of tools that write commits but cannot certify the DCO.
const TOOL_EMAILS: &[&str] = &["noreply@anthropic.com"];

/// Separates the fields of one commit in `git log` output.
const FIELD: char = '\u{1f}';
/// Separates commits in `git log` output.
const RECORD: char = '\u{1e}';
/// Separates the Signed-off-by values of one commit.
const VALUE: char = '\u{1d}';

/// What a remediation line says between the person and the commit's hash:
/// the wording the DCO's own tools read.
const REMEDIATES: &str = ", hereby add my Signed-off-by to this commit: ";

/// One commit as `ob signoff` sees it.
#[derive(Debug, PartialEq)]
struct Commit {
    hash: String,
    subject: String,
    author: Identity,
    signoffs: Vec<Identity>,
}

/// A sign-off given afterwards to a commit that reached a shared branch
/// without one, where history is not rewritten: a line in the message of
/// a later commit, `I, Name <email>, hereby add my Signed-off-by to this
/// commit: <hash>`.
#[derive(Debug, PartialEq)]
struct Remediation {
    /// The commit whose message has the line.
    carrier: String,
    person: Identity,
    /// The commit the person signs off, by its full hash.
    hash: String,
}

/// A `Name <email>` pair.
#[derive(Clone, Debug, PartialEq)]
struct Identity {
    name: String,
    email: String,
}

impl Identity {
    /// Parses `Name <email>`, as in a Signed-off-by trailer.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (name, rest) = text.split_once('<')?;
        let email = rest.strip_suffix('>')?.trim();
        let name = name.trim();
        if name.is_empty() || !email.contains('@') {
            return None;
        }
        Some(Self { name: name.to_string(), email: email.to_string() })
    }

    /// Whether this is a tool rather than a person: an AI assistant or a
    /// GitHub App.
    fn is_tool(&self) -> bool {
        TOOL_EMAILS.iter().any(|tool| self.email.eq_ignore_ascii_case(tool))
            || self.name.ends_with("[bot]")
    }

    fn same_email(&self, other: &Identity) -> bool {
        self.email.eq_ignore_ascii_case(&other.email)
    }
}

/// `ob signoff [<base>]`.
pub fn check(ctx: &Ctx, base: Option<&str>) -> Result<()> {
    let base = match base {
        Some(base) => base.to_string(),
        None => merge_base(ctx)?,
    };
    step(&format!("Checking sign-offs on the commits after {}", short(&base)));
    let range = format!("{base}..HEAD");
    let mut cmd = git(ctx);
    cmd.args(["log", "--no-merges"]).arg(format!(
        "--format=%H{FIELD}%s{FIELD}%an{FIELD}%ae{FIELD}\
         %(trailers:key=Signed-off-by,valueonly,separator=%x1d){RECORD}"
    ));
    cmd.arg(&range);
    let output = process::output(ctx, &mut cmd)?;
    let mut commits = parse_log(&String::from_utf8_lossy(&output.stdout));
    // The whole messages, a NUL after each: no message has a NUL in it.
    let mut cmd = git(ctx);
    cmd.args(["log", "--no-merges", "-z", "--format=%H%n%B"]).arg(&range);
    let output = process::output(ctx, &mut cmd)?;
    let remediations = parse_remediations(&String::from_utf8_lossy(&output.stdout));
    for remediation in remediate(&mut commits, &remediations) {
        println!(
            "  {} is signed off afterwards, in {}",
            short(&remediation.hash),
            short(&remediation.carrier)
        );
    }

    let problems: Vec<String> = commits
        .iter()
        .filter_map(|commit| {
            problem(commit).map(|why| format!("{} {}: {why}", short(&commit.hash), commit.subject))
        })
        .collect();
    if problems.is_empty() {
        println!("All {} commits are signed off.", commits.len());
        return Ok(());
    }
    for line in &problems {
        eprintln!("  {line}");
    }
    Err(Error::new(format!(
        "{} of {} commits lack a Developer Certificate of Origin sign-off",
        problems.len(),
        commits.len()
    ))
    .hint(format!(
        "add one with `git rebase --signoff {}`, then force-push; CONTRIBUTING.md explains it",
        short(&base)
    )))
}

/// The commit where HEAD left the main branch.
fn merge_base(ctx: &Ctx) -> Result<String> {
    let mut cmd = git(ctx);
    cmd.args(["merge-base", DEFAULT_BASE, "HEAD"]);
    let output = process::output(ctx, &mut cmd).map_err(|err| {
        err.hint(format!("fetch {DEFAULT_BASE} first, or name the base: `./ob signoff <commit>`"))
    })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git(ctx: &Ctx) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(&ctx.root);
    cmd
}

fn short(hash: &str) -> &str {
    if is_full_hash(hash) { &hash[..12] } else { hash }
}

/// Whether `text` is a commit's whole hash, which names one commit and no
/// other.
fn is_full_hash(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_log(text: &str) -> Vec<Commit> {
    text.split(RECORD)
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').split(FIELD);
            let hash = fields.next()?.trim();
            if hash.is_empty() {
                return None;
            }
            let subject = fields.next()?.to_string();
            let author = Identity { name: fields.next()?.into(), email: fields.next()?.into() };
            let signoffs = fields
                .next()
                .unwrap_or_default()
                .split(VALUE)
                .filter_map(Identity::parse)
                .collect();
            Some(Commit { hash: hash.to_string(), subject, author, signoffs })
        })
        .collect()
}

/// The remediation lines in `text`: each commit's hash on a line, then its
/// message, with a NUL after each commit.
fn parse_remediations(text: &str) -> Vec<Remediation> {
    let mut found = Vec::new();
    for record in text.split('\0') {
        let mut lines = record.lines();
        let Some(carrier) = lines.next() else { continue };
        found.extend(lines.filter_map(|line| remediation(carrier.trim(), line)));
    }
    found
}

/// The remediation `line` of the commit `carrier`'s message makes, if it
/// is one. The commit is named by its whole hash, as a shorter one may
/// come to name two commits.
fn remediation(carrier: &str, line: &str) -> Option<Remediation> {
    let (person, hash) = line.trim().strip_prefix("I, ")?.split_once(REMEDIATES)?;
    let hash = hash.trim();
    if !is_full_hash(hash) {
        return None;
    }
    Some(Remediation {
        carrier: carrier.to_string(),
        person: Identity::parse(person)?,
        hash: hash.to_ascii_lowercase(),
    })
}

/// Gives each commit of `commits` that a remediation names the person's
/// sign-off, and returns the remediations that did. A remediation counts
/// only if the person signed off the commit that carries it, which is
/// then their certification of both: a line anyone could write is no
/// sign-off. Whether that person's sign-off is the one the commit needs
/// is [`problem`]'s to say, as for a sign-off of the commit's own.
fn remediate<'a>(commits: &mut [Commit], remediations: &'a [Remediation]) -> Vec<&'a Remediation> {
    let certified: Vec<&Remediation> = remediations
        .iter()
        .filter(|remediation| {
            !remediation.person.is_tool()
                && commits.iter().any(|carrier| {
                    carrier.hash == remediation.carrier
                        && carrier.signoffs.iter().any(|s| s.same_email(&remediation.person))
                })
        })
        .collect();
    let mut counted = Vec::new();
    for remediation in certified {
        if let Some(commit) = commits.iter_mut().find(|commit| commit.hash == remediation.hash) {
            commit.signoffs.push(remediation.person.clone());
            counted.push(remediation);
        }
    }
    counted
}

/// Why `commit` is not properly signed off, if it is not.
///
/// A person signs off their own commits. A tool cannot certify anything, so
/// its sign-offs do not count, and a commit a tool wrote needs the sign-off
/// of the person who submits it.
fn problem(commit: &Commit) -> Option<&'static str> {
    let people: Vec<&Identity> = commit.signoffs.iter().filter(|s| !s.is_tool()).collect();
    if commit.author.is_tool() {
        if people.is_empty() {
            return Some(
                "written by a tool, so it needs a Signed-off-by line from the person submitting it",
            );
        }
    } else if !people.iter().any(|s| s.same_email(&commit.author)) {
        return Some(if commit.signoffs.is_empty() {
            "no Signed-off-by line"
        } else {
            "no Signed-off-by line with the author's email address"
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(text: &str) -> Identity {
        Identity::parse(text).unwrap()
    }

    fn commit(author: &str, signoffs: &[&str]) -> Commit {
        Commit {
            hash: "0".repeat(40),
            subject: "Do a thing".into(),
            author: identity(author),
            signoffs: signoffs.iter().map(|s| identity(s)).collect(),
        }
    }

    #[test]
    fn parses_identities() {
        let id = identity(" Random J Developer <random@developer.example.org> ");
        assert_eq!(id.name, "Random J Developer");
        assert_eq!(id.email, "random@developer.example.org");
        assert!(Identity::parse("no email").is_none());
        assert!(Identity::parse("<only@email>").is_none());
        assert!(Identity::parse("Name <not-an-email>").is_none());
    }

    #[test]
    fn parses_git_log_output() {
        let log = format!(
            "aaaa{FIELD}First{FIELD}Ann{FIELD}ann@example.org{FIELD}\
             Ann <ann@example.org>{VALUE}Bob <bob@example.org>{RECORD}\n\
             bbbb{FIELD}Second{FIELD}Bob{FIELD}bob@example.org{FIELD}{RECORD}\n"
        );
        let commits = parse_log(&log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "First");
        assert_eq!(commits[0].signoffs.len(), 2);
        assert_eq!(commits[1].hash, "bbbb");
        assert!(commits[1].signoffs.is_empty());
    }

    #[test]
    fn people_sign_off_their_own_commits() {
        let ann = "Ann <ann@example.org>";
        assert_eq!(problem(&commit(ann, &[ann])), None);
        assert_eq!(problem(&commit(ann, &["ANN <Ann@Example.org>"])), None);
        assert!(problem(&commit(ann, &[])).is_some());
        assert!(problem(&commit(ann, &["Bob <bob@example.org>"])).is_some());
    }

    #[test]
    fn tools_need_a_person_to_sign_off() {
        let claude = "Claude <noreply@anthropic.com>";
        let bot = "dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>";
        assert_eq!(problem(&commit(claude, &["Ann <ann@example.org>"])), None);
        assert!(problem(&commit(claude, &[])).is_some());
        assert!(problem(&commit(claude, &[claude])).is_some());
        assert!(problem(&commit(bot, &[bot])).is_some());
        assert!(problem(&commit("Ann <ann@example.org>", &[claude])).is_some());
    }

    /// A commit with the hash `letter` forty times over.
    fn commit_at(letter: &str, author: &str, signoffs: &[&str]) -> Commit {
        Commit { hash: letter.repeat(40), ..commit(author, signoffs) }
    }

    /// What `git log -z --format=%H%n%B` prints for commits with these
    /// hashes and messages.
    fn messages(commits: &[(&str, &str)]) -> String {
        let mut text = String::new();
        for (letter, message) in commits {
            text += &format!("{}\n{message}\0", letter.repeat(40));
        }
        text
    }

    #[test]
    fn a_remediation_line_names_a_person_and_a_whole_hash() {
        let ann = "Ann <ann@example.org>";
        let line =
            format!("I, {ann}, hereby add my Signed-off-by to this commit: {}", "A".repeat(40));
        let text = messages(&[
            ("c", &format!("Fix a thing\n\nWhy it broke.\n\n  {line}  \n\nSigned-off-by: {ann}\n")),
            ("b", "No such line\n\nI, Ann, hereby do nothing.\n"),
        ]);
        let found = parse_remediations(&text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].carrier, "c".repeat(40));
        assert_eq!(found[0].person, identity(ann));
        assert_eq!(found[0].hash, "a".repeat(40));

        let carrier = "c".repeat(40);
        // A short hash may come to name two commits, and a person is a
        // name with an email address.
        assert!(remediation(&carrier, &line[..line.len() - 28]).is_none());
        let nameless = line.replace(ann, "<ann@example.org>");
        assert!(remediation(&carrier, &nameless).is_none());
        assert!(remediation(&carrier, &line.replace("I, ", "We, ")).is_none());
    }

    #[test]
    fn a_sign_off_given_afterwards_counts_for_the_commit_it_names() {
        let ann = "Ann <ann@example.org>";
        let line =
            format!("I, {ann}, hereby add my Signed-off-by to this commit: {}", "a".repeat(40));
        let remediations = parse_remediations(&messages(&[("c", &line)]));
        let mut commits = vec![commit_at("c", ann, &[ann]), commit_at("a", ann, &[])];
        assert!(problem(&commits[1]).is_some());
        let counted = remediate(&mut commits, &remediations);
        assert_eq!(counted.len(), 1);
        assert_eq!(problem(&commits[1]), None);
        // The carrier itself is checked as any commit is.
        assert_eq!(problem(&commits[0]), None);
    }

    #[test]
    fn a_remediation_nobody_certified_or_by_another_person_does_not_count() {
        let ann = "Ann <ann@example.org>";
        let bob = "Bob <bob@example.org>";
        let claude = "Claude <noreply@anthropic.com>";
        let names_a = |who: &str| {
            let line =
                format!("I, {who}, hereby add my Signed-off-by to this commit: {}", "a".repeat(40));
            parse_remediations(&messages(&[("c", &line)]))
        };
        // Bob wrote the line in Ann's name, in a commit only he signed off.
        let mut commits = vec![commit_at("c", bob, &[bob]), commit_at("a", ann, &[])];
        assert!(remediate(&mut commits, &names_a(ann)).is_empty());
        assert!(problem(&commits[1]).is_some());
        // Bob signs off Ann's commit: it still lacks its author's.
        let mut commits = vec![commit_at("c", bob, &[bob]), commit_at("a", ann, &[])];
        assert_eq!(remediate(&mut commits, &names_a(bob)).len(), 1);
        assert_eq!(
            problem(&commits[1]),
            Some("no Signed-off-by line with the author's email address")
        );
        // A tool certifies nothing, afterwards as before.
        let mut commits = vec![commit_at("c", claude, &[claude, ann]), commit_at("a", claude, &[])];
        assert!(remediate(&mut commits, &names_a(claude)).is_empty());
        assert!(problem(&commits[1]).is_some());
        // The line's commit is not among those checked, so nobody has
        // shown it is signed off.
        let mut commits = vec![commit_at("a", ann, &[])];
        assert!(remediate(&mut commits, &names_a(ann)).is_empty());
    }

    #[test]
    fn shortens_only_full_hashes() {
        assert_eq!(short(&"a".repeat(40)), "aaaaaaaaaaaa");
        assert_eq!(short("origin/main"), "origin/main");
    }
}
