// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `website list` and `website check` (ADR 0569): what the sites' files
//! say, as anyone may read it, and what nginx says of them, which only
//! root can ask.
//!
//! A site's facts come from its file, read by the reader of nginx's
//! syntax: a site is whatever file `*.conf` the folder holds that does not
//! start with `_`, one `website` wrote or one a person did. `by_hand` says
//! the file is not the one `website` would write for what it says, and
//! `added_by_hand` that it did not write it at all.

use std::fs;

use serde::Serialize;

use super::change::{Ctx, Refusal, Site, State, load, load_with};
use super::conf::{FILE_MAX, Kind};
use super::host::Svc;
use super::plan::{self, SETTINGS};
use super::stage::{self, conf_view};

/// A program as `list` says it.
#[derive(Debug, Serialize)]
struct ProgramJson {
    installed: bool,
    enabled: bool,
    running: bool,
}

/// What is wrong, that anyone can see.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The error's ID; none for what no ID names.
    pub id: Option<&'static str>,
    /// What is wrong.
    pub words: String,
}

/// A site as `list` says it.
#[derive(Debug, Serialize)]
struct SiteJson {
    name: String,
    names: Vec<String>,
    file: String,
    kind: &'static str,
    folder: Option<String>,
    owner: Option<String>,
    https: bool,
    certificate: Option<String>,
    certificate_path: Option<String>,
    redirects_http: bool,
    by_hand: bool,
    added_by_hand: bool,
    problems: Vec<Problem>,
    text: String,
}

/// What `list` says.
#[derive(Debug, Serialize)]
pub struct Listing {
    format: u32,
    nginx: ProgramJson,
    php_fpm: ProgramJson,
    websites: Vec<SiteJson>,
}

/// A program as Spark says it.
fn program(svc: Option<Svc>) -> ProgramJson {
    ProgramJson {
        installed: svc.is_some(),
        enabled: svc.is_some_and(|svc| svc.enabled),
        running: svc.is_some_and(|svc| svc.running),
    }
}

/// The name of the account that owns the path `folder`, when it is a
/// folder; none when it is not there.
fn owner_of(ctx: &Ctx<'_>, folder: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let found = fs::symlink_metadata(ctx.paths.real(folder)).ok().filter(fs::Metadata::is_dir)?;
    let accounts = ctx.host.accounts();
    accounts
        .iter()
        .find(|user| user.uid == found.uid())
        .map(|user| user.name.clone())
        .or_else(|| Some(found.uid().to_string()))
}

/// One site as `list` says it.
fn site_json(ctx: &Ctx<'_>, state: &State, site: &Site) -> SiteJson {
    let file = format!("{SETTINGS}/{}", site.file);
    let added_by_hand = plan::added_by_hand(&site.name, &site.text);
    let too_large = site.text.len() > FILE_MAX;
    let text = if too_large { String::new() } else { site.text.clone() };
    let mut problems = Vec::new();
    if too_large {
        problems.push(Problem {
            id: None,
            words: "Its file is over 64 KiB, so its text is not shown".into(),
        });
    }
    let facts = match &site.facts {
        Ok(facts) => facts,
        Err(why) => {
            if !too_large {
                problems.push(Problem {
                    id: None,
                    words: format!("Its file does not read as nginx's settings: {why}"),
                });
            }
            return SiteJson {
                name: site.name.clone(),
                names: Vec::new(),
                file,
                kind: Kind::Other.word(),
                folder: None,
                owner: None,
                https: false,
                certificate: None,
                certificate_path: None,
                redirects_http: false,
                by_hand: true,
                added_by_hand,
                problems,
                text,
            };
        }
    };
    let owner = facts.folder.as_deref().and_then(|folder| owner_of(ctx, folder));
    if let Some(folder) = &facts.folder
        && owner.is_none()
    {
        problems.push(Problem { id: None, words: format!("Its folder {folder} is not there") });
    }
    if let Some(certificate) = &facts.certificate {
        let named = plan::named_certificates(&state.service, state.over.as_deref());
        if named.is_ok_and(|named| !named.contains(certificate)) {
            problems.push(Problem {
                id: None,
                words: format!(
                    "nginx has no copy of its certificate: nginx's service file does not name {certificate}"
                ),
            });
        }
    }
    if facts.kind == Kind::Php && !state.php.is_some_and(|svc| svc.running) {
        let words = if state.php.is_some() {
            "PHP-FPM is off: its PHP pages fail"
        } else {
            "PHP-FPM is not installed: its PHP pages fail"
        };
        problems.push(Problem { id: None, words: words.into() });
    }
    SiteJson {
        name: site.name.clone(),
        names: facts.names.clone(),
        file,
        kind: facts.kind.word(),
        folder: facts.folder.clone(),
        owner,
        https: facts.https,
        certificate: facts.certificate.clone(),
        certificate_path: facts.certificate_path.clone(),
        redirects_http: facts.redirects_http,
        by_hand: plan::by_hand(&site.name, &site.text, facts),
        added_by_hand,
        problems,
        text,
    }
}

/// What `list` says of the sites.
///
/// # Errors
///
/// When the files cannot be read or Spark cannot be asked.
pub fn list(ctx: &Ctx<'_>) -> Result<Listing, Refusal> {
    // nginx's own settings are root's to read, and a listing needs none.
    let state = load_with(ctx, false)?;
    let websites = state.sites.iter().map(|site| site_json(ctx, &state, site)).collect();
    Ok(Listing { format: 1, nginx: program(state.nginx), php_fpm: program(state.php), websites })
}

impl Listing {
    /// The listing as the text `--json` prints.
    ///
    /// # Errors
    ///
    /// When it cannot be written as JSON.
    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    /// The listing as a table: name, kind, HTTPS and folder, with a line
    /// first for what is not as it should be.
    pub fn plain(&self) -> String {
        let mut text = String::new();
        if !self.nginx.installed {
            text.push_str("nginx is not installed.\n");
        } else if !self.nginx.running {
            text.push_str("nginx is off: no site answers.\n");
        }
        if self.websites.is_empty() {
            text.push_str("No websites.\n");
            return text;
        }
        let rows: Vec<[String; 4]> = self
            .websites
            .iter()
            .map(|site| {
                let https = match (&site.certificate, site.https) {
                    (Some(certificate), _) => certificate.clone(),
                    (None, true) => "yes".to_string(),
                    (None, false) => "no".to_string(),
                };
                [
                    site.name.clone(),
                    site.kind.to_string(),
                    https,
                    site.folder.clone().unwrap_or_default(),
                ]
            })
            .collect();
        let head =
            ["NAME".to_string(), "KIND".to_string(), "HTTPS".to_string(), "FOLDER".to_string()];
        let width =
            |at: usize| rows.iter().chain([&head]).map(|row| row[at].len()).max().unwrap_or(0);
        let widths = [width(0), width(1), width(2)];
        for row in [&head].into_iter().chain(rows.iter()) {
            let line = format!(
                "{:<w0$}  {:<w1$}  {:<w2$}  {}",
                row[0],
                row[1],
                row[2],
                row[3],
                w0 = widths[0],
                w1 = widths[1],
                w2 = widths[2]
            );
            text.push_str(line.trim_end());
            text.push('\n');
        }
        for site in &self.websites {
            for problem in &site.problems {
                text.push_str(&format!("{}: {}\n", site.name, problem.words));
            }
        }
        text
    }
}

/// `nginx.conf` as `check` says it.
#[derive(Debug, Serialize)]
struct ConfJson {
    path: String,
    includes: bool,
    hides_sites: bool,
}

/// The result of nginx's check.
#[derive(Debug, Serialize)]
struct CheckedJson {
    passed: bool,
    line: Option<String>,
}

/// What nginx says of a site.
#[derive(Debug, Serialize)]
struct SiteChecked {
    name: String,
    problems: Vec<Problem>,
}

/// What `check` says.
#[derive(Debug, Serialize)]
pub struct Report {
    format: u32,
    nginx_conf: ConfJson,
    check: CheckedJson,
    websites: Vec<SiteChecked>,
}

/// The message of nginx's `line`, and the file it names, when it names a
/// site's file: `nginx: [emerg] MESSAGE in /system/settings/websites/NAME.conf:N`.
fn site_of(line: &str) -> Option<(String, String)> {
    let (before, place) = line.rsplit_once(" in ")?;
    let (path, number) = place.rsplit_once(':')?;
    number.trim().parse::<u32>().ok()?;
    let file = path.strip_prefix(&format!("{SETTINGS}/"))?;
    let name =
        file.strip_suffix(".conf").filter(|name| !name.contains('/') && !name.starts_with('_'))?;
    let message = before.split_once("] ").map_or(before, |(_, rest)| rest);
    Some((name.to_string(), message.trim().to_string()))
}

/// Asks nginx whether it accepts the sites as they are.
///
/// # Errors
///
/// When the files cannot be read or nginx's check could not be run.
pub fn check(ctx: &Ctx<'_>) -> Result<(Report, bool), Refusal> {
    let state = load(ctx)?;
    let main = state.conf.as_ref().and_then(|conf| conf.main.as_ref().ok());
    let nginx_conf = ConfJson {
        path: conf_view(),
        includes: main.is_some_and(|main| main.includes),
        hides_sites: main.is_some_and(|main| main.hides_sites),
    };
    let (checked, problems) = if state.conf.is_none() {
        let why = if state.nginx.is_some() {
            "nginx has not started yet: it writes its settings as it starts"
        } else {
            "nginx is not installed"
        };
        (CheckedJson { passed: false, line: Some(why.to_string()) }, None)
    } else {
        match ctx.host.nginx_check(&conf_view()) {
            Ok(done) => {
                let line = done.line.map(|line| stage::as_live(&line));
                let problems = line.as_deref().and_then(site_of);
                (CheckedJson { passed: done.passed, line }, problems)
            }
            Err(why) => {
                return Err(Refusal::plain(format!("nginx's check could not be run: {why}")));
            }
        }
    };
    let ok = checked.passed && nginx_conf.includes && nginx_conf.hides_sites;
    let websites = state
        .sites
        .iter()
        .map(|site| SiteChecked {
            name: site.name.clone(),
            problems: problems
                .iter()
                .filter(|(name, _)| *name == site.name)
                .map(|(_, message)| Problem {
                    id: Some("WEB-103"),
                    words: format!("nginx refuses its file: {message}"),
                })
                .collect(),
        })
        .collect();
    Ok((Report { format: 1, nginx_conf, check: checked, websites }, ok))
}

impl Report {
    /// The report as the text `--json` prints.
    ///
    /// # Errors
    ///
    /// When it cannot be written as JSON.
    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    /// The report in words.
    pub fn plain(&self) -> String {
        let mut text = String::new();
        let yes = |switch: bool| if switch { "yes" } else { "no" };
        text.push_str(&format!("nginx settings: {}\n", self.nginx_conf.path));
        text.push_str(&format!(
            "  include the websites' folder: {}\n",
            yes(self.nginx_conf.includes)
        ));
        text.push_str(&format!(
            "  hide /sites/ in the default server: {}\n",
            yes(self.nginx_conf.hides_sites)
        ));
        match (&self.check.passed, &self.check.line) {
            (true, _) => text.push_str("nginx's check: passed\n"),
            (false, Some(line)) => text.push_str(&format!("nginx's check: failed: {line}\n")),
            (false, None) => text.push_str("nginx's check: failed\n"),
        }
        for site in &self.websites {
            for problem in &site.problems {
                text.push_str(&format!("{}: {}\n", site.name, problem.words));
            }
        }
        text
    }
}
