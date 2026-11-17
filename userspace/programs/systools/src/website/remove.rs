// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `website remove` (ADR 0569): the site's file goes, nginx reads its
//! settings again, nginx's service file gives back the certificate and
//! the sockets for 443 that nothing else needs, the files of the site's log
//! of requests go, and, when asked, the site's folder goes too.
//!
//! The new set without the site is checked by nginx first, so that a
//! hand edit of another site that nginx refuses is told before anything is
//! changed. The folder is removed last and only once all else is done,
//! since nothing can bring it back.

use super::change::{Ctx, Done, Order, Refusal, apply, intent, load, rollback};
use super::count;
use super::folder;
use super::stage::Lock;

/// What a removal leaves for its last words.
struct Removed {
    /// The site's folder, when its file named one.
    folder: Option<String>,
    /// The certificate it was served with, if it was.
    certificate: Option<String>,
}

/// Removes the site `name`, and its folder with `files`.
///
/// # Errors
///
/// The refusal of a check, or the step that failed, with what was taken
/// back.
pub fn remove(ctx: &Ctx<'_>, name: &str, files: bool) -> Result<Vec<String>, Refusal> {
    let _lock = Lock::take(ctx.paths).map_err(Refusal::plain)?;
    let mut done = Done::default();
    let removed = match remove_steps(ctx, name, &mut done) {
        Ok(removed) => removed,
        Err(refusal) => {
            let left = rollback(ctx, &done);
            return Err(refusal.and_left(&left));
        }
    };
    let mut lines = vec![format!("Removed {name}")];
    // What nginx wrote of its requests is of no use without the site, and
    // is no reason to fail what is done.
    if let Err(why) = count::remove_logs(ctx.paths, name) {
        lines.push(format!("Its log of requests was not removed: {why}"));
    }
    let mut kept = "its folder was not named";
    if let Some(site_folder) = &removed.folder {
        if files {
            ctx.steps.next("Removing its folder");
            let others = other_folders(ctx, name)?;
            if folder::overlap(site_folder, &others).is_some() {
                kept = "another site serves it too";
                lines.push(format!("Its folder {site_folder} is kept: another site serves it too"));
            } else {
                folder::remove(ctx.paths, site_folder).map_err(|why| {
                    Refusal::plain(format!(
                        "The site {name} is removed, but its folder {site_folder} is not: {why}"
                    ))
                })?;
                kept = "its folder was removed";
                lines.push(format!("Removed its folder {site_folder} and what was in it"));
            }
        } else {
            kept = "its folder was kept";
            lines.push(format!(
                "Its folder {site_folder} is kept: remove it with --files, or by hand"
            ));
        }
    }
    if let Some(certificate) = &removed.certificate {
        lines.push(format!(
            "The certificate {certificate} is kept; `elevate cert remove {certificate}` removes it."
        ));
    }
    ctx.host.log(&format!("removed {name} ({kept})"));
    Ok(lines)
}

/// The folders of the sites other than `name`.
fn other_folders(ctx: &Ctx<'_>, name: &str) -> Result<Vec<(String, String)>, Refusal> {
    let state = load(ctx)?;
    Ok(state
        .sites
        .iter()
        .filter(|site| site.name != name)
        .filter_map(|site| Some((site.name.clone(), site.facts.as_ref().ok()?.folder.clone()?)))
        .collect())
}

/// The steps that take the site away, up to its folder.
fn remove_steps(ctx: &Ctx<'_>, name: &str, done: &mut Done) -> Result<Removed, Refusal> {
    let state = load(ctx)?;
    ctx.steps.next("Checking the new set of sites without it");
    let Some(site) = state.sites.iter().find(|site| site.name == name) else {
        return Err(Refusal::plain(format!("No website {name}")));
    };
    let facts = site.facts.as_ref().ok();
    let removed = Removed {
        folder: facts.and_then(|facts| facts.folder.clone()),
        certificate: facts.and_then(|facts| facts.certificate.clone()),
    };
    let intent = intent(&state, name, None);
    apply(ctx, &state, &intent, false, Order::ServiceLast, done)?;
    Ok(removed)
}
