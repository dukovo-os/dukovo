// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `website add --dry-run --json` (ADR 0569): the plan of an add, which
//! says what is checked, what would be made, written and started, what
//! the firewall says of the ports the site is reached on, and whether
//! nginx accepts the new set of sites, and changes nothing that stays.
//!
//! The checks are those of the add, all of them, so that a form can tell
//! each that fails; the check of nginx on the staged files is run when no
//! certificate is new to nginx, which can see it only after it restarts,
//! and the staged files are removed again.

use serde::Serialize;

use super::change::{
    AddRequest, Check, Checks, Ctx, Found, Now, Refusal, add_checks, intent, load, set_after,
};
use super::conf::Kind;
use super::host::{NGINX, NGINX_BUNDLE, PHP_FPM};
use super::plan::{self, SETTINGS};
use super::stage::{self, Staged};

/// One check of the plan.
#[derive(Debug, Serialize)]
struct CheckJson {
    id: Option<&'static str>,
    result: &'static str,
    words: String,
}

/// The folder of the site.
#[derive(Debug, Serialize)]
struct FolderJson {
    path: String,
    exists: bool,
    entries: usize,
    owner: Option<String>,
}

/// A file that would be written.
#[derive(Debug, Serialize)]
struct FileJson {
    path: String,
    new: bool,
    text: String,
}

/// A step, in words, and the command line that does it when there is one.
#[derive(Debug, Serialize)]
struct StepJson {
    words: String,
    argv: Vec<String>,
}

/// What the firewall says of a port.
#[derive(Debug, Serialize)]
struct FirewallJson {
    port: u16,
    allowed: bool,
    rule: Option<u64>,
    text: String,
}

/// The plan of an add.
#[derive(Debug, Serialize)]
pub struct Plan {
    format: u32,
    checks: Vec<CheckJson>,
    folder: FolderJson,
    files: Vec<FileJson>,
    steps: Vec<StepJson>,
    restarts_nginx: bool,
    turns_on: Vec<&'static str>,
    firewall: Vec<FirewallJson>,
    nginx_check: &'static str,
    nginx_line: Option<String>,
}

impl Plan {
    /// The plan in words: each check with how it came out, each file that
    /// is written, and the steps.
    pub fn plain(&self) -> String {
        let mut text = String::new();
        for check in &self.checks {
            let id = check.id.map_or_else(String::new, |id| format!(" ({id})"));
            text.push_str(&format!("{}: {}{id}\n", check.result, check.words));
        }
        for file in &self.files {
            let how = if file.new { "new" } else { "changes" };
            text.push_str(&format!("file {}: {how}\n", file.path));
        }
        for step in &self.steps {
            text.push_str(&format!("step: {}\n", step.words));
        }
        text.push_str(&format!("nginx's check of the new set: {}\n", self.nginx_check));
        text
    }

    /// The plan as the text `--json` prints.
    ///
    /// # Errors
    ///
    /// When it cannot be written as JSON.
    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }
}

/// A step of the plan with no command line of its own.
fn step(words: impl Into<String>) -> StepJson {
    StepJson { words: words.into(), argv: Vec::new() }
}

/// A step of the plan that is a command.
fn command(words: impl Into<String>, argv: &[&str]) -> StepJson {
    StepJson { words: words.into(), argv: argv.iter().map(ToString::to_string).collect() }
}

/// What `request` would do, and whether every check passes.
///
/// # Errors
///
/// When what the plan reads cannot be read.
pub fn plan(ctx: &Ctx<'_>, request: &AddRequest) -> Result<(Plan, bool), Refusal> {
    let mut state = load(ctx)?;
    let mut checks = Checks::all();
    let found = add_checks(ctx, &mut state, request, &mut checks, None)?;
    let spec = request.spec();
    let intent = intent(&state, &spec.name, Some(&spec));
    let installed = ctx.host.installed(NGINX_BUNDLE, NGINX);
    let mut files: Vec<FileJson> = intent
        .put
        .iter()
        .map(|raw| FileJson {
            path: format!("{SETTINGS}/{}", raw.file),
            new: !state.raw.iter().any(|old| old.file == raw.file),
            text: String::from_utf8_lossy(&raw.bytes).into_owned(),
        })
        .collect();
    let mut changes_service = false;
    if installed {
        match plan::override_text(&state.service, state.over.as_deref(), &intent.wanted) {
            Ok(Some(text)) => {
                changes_service = true;
                files.push(FileJson {
                    path: super::change::OVERRIDE.to_string(),
                    new: state.over.is_none(),
                    text,
                });
            }
            Ok(None) => {}
            Err(why) => {
                checks.fail(
                    Some("WEB-103"),
                    format!("nginx's service file cannot be changed: {why}"),
                    false,
                )?;
            }
        }
    }
    if !found.folder.exists || !found.folder.indexed {
        let (file, text) = plan::starter(spec.kind, &spec.name, &spec.folder);
        files.push(FileJson { path: format!("{}/{file}", spec.folder), new: true, text });
    }
    let (nginx_check, nginx_line) = staged_check(ctx, &state, &found, &intent, &mut checks)?;
    let running = state.nginx.is_some_and(|svc| svc.running);
    let php_running = state.php.is_some_and(|svc| svc.running);
    let mut turns_on = Vec::new();
    if !running {
        turns_on.push(NGINX);
    }
    if spec.kind == Kind::Php && !php_running {
        turns_on.push(PHP_FPM);
    }
    let restarts_nginx = changes_service && running;
    let passed = !checks.failed();
    let accounts = ctx.host.accounts();
    let owner = found
        .folder
        .owner
        .and_then(|uid| accounts.iter().find(|user| user.uid == uid).map(|user| user.name.clone()));
    let mut steps = Vec::new();
    if !found.folder.exists {
        steps.push(step(format!("Make the folder {}", spec.folder)));
    }
    if !found.folder.exists || !found.folder.indexed {
        steps.push(step("Write the starter page"));
    }
    if changes_service {
        steps.push(step("Give nginx's service file what the site needs"));
    }
    if restarts_nginx {
        steps.push(command("Restart nginx", &["service", "nginx", "restart"]));
    }
    steps.push(step("Check the new set of sites with nginx"));
    steps.push(step("Write the site's file"));
    steps.push(if running {
        command("Have nginx read its settings again", &["service", "nginx", "reload"])
    } else {
        command("Turn nginx on", &["service", "nginx", "enable"])
    });
    if turns_on.contains(&PHP_FPM) {
        steps.push(command("Turn PHP-FPM on", &["service", "php-fpm", "enable"]));
    }
    steps.push(step("Check that each name answers"));
    let mut ports = vec![80];
    if spec.certificate.is_some() {
        ports.push(443);
    }
    let firewall = ports
        .into_iter()
        .map(|port| {
            let rule = ctx.host.firewall(port);
            FirewallJson { port, allowed: rule.allowed, rule: rule.rule, text: rule.text }
        })
        .collect();
    let plan = Plan {
        format: 1,
        checks: checks.list.into_iter().map(check_json).collect(),
        folder: FolderJson {
            path: spec.folder.clone(),
            exists: found.folder.exists,
            entries: found.folder.entries,
            owner,
        },
        files,
        steps,
        restarts_nginx,
        turns_on,
        firewall,
        nginx_check,
        nginx_line,
    };
    Ok((plan, passed))
}

/// A check as the plan says it.
fn check_json(check: Check) -> CheckJson {
    CheckJson { id: check.id, result: check.result, words: check.words }
}

/// nginx's check of the new set, staged and removed again: how it came
/// out and nginx's line. It is left for later when a certificate is new
/// to nginx, when its settings were not checked or do not pass as they
/// are, or when they cannot be staged.
fn staged_check(
    ctx: &Ctx<'_>,
    state: &super::change::State,
    found: &Found,
    intent: &super::change::Intent,
    checks: &mut Checks,
) -> Result<(&'static str, Option<String>), Refusal> {
    match &found.now {
        Now::Failed(line) => return Ok(("failed", line.clone())),
        Now::Skipped => return Ok(("later", None)),
        Now::Passed => {}
    }
    if intent.new_certificate {
        return Ok(("later", None));
    }
    let Some(conf) = &state.conf else { return Ok(("later", None)) };
    let Ok(main) = &conf.main else { return Ok(("later", None)) };
    let files = set_after(state, intent);
    let staged = Staged::make(ctx.paths, &files, &conf.text, main).map_err(Refusal::plain)?;
    let checked = ctx.host.nginx_check(&staged.conf());
    drop(staged);
    match checked {
        Ok(checked) if checked.passed => {
            checks.pass("nginx accepts the new set of websites");
            Ok(("passed", None))
        }
        Ok(checked) => {
            let line = checked.line.map(|line| stage::as_live(&line));
            checks.fail(
                Some("WEB-103"),
                format!(
                    "nginx refuses the new set of websites: {}",
                    line.as_deref().unwrap_or("it gave no line")
                ),
                false,
            )?;
            Ok(("failed", line))
        }
        Err(why) => {
            checks.fail(None, format!("nginx's check could not be run: {why}"), false)?;
            Ok(("later", None))
        }
    }
}
