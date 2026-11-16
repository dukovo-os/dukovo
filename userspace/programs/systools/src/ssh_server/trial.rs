// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Spark's trial `ssh` (ADR 0472, ADR 0562): a change of SSH that comes
//! undone by itself unless it is kept, so that a change that shuts the
//! person out of SSH does not last.
//!
//! `ssh-server set ... --try SECONDS` tries sshd_config and asks Spark to
//! reload sshd, with the trial and with its undo: sshd reads the file
//! again for the connections that come next, and the ones that are open
//! go on. `ssh-server off --try SECONDS` tries sshd's service file with
//! `enabled = false` and stops sshd, which ends the SSH sessions that are
//! open; Spark's undo puts the file back and starts sshd again, as it
//! restarts a service that ran when the trial began. A trial needs sshd
//! running, as there is no connection for it to keep otherwise: without
//! it the change is made as it is, with no `--try`.
//!
//! While the trial runs, every `ssh-server` change refuses with NET-182:
//! Spark's undo would put back the file as it was before both. `keep` and
//! `undo` end it; with no trial they say so, NET-179.

use std::path::Path;

use accounts::sshd_config::PATH;
use spark_api::show::State;

use super::SERVICE;
use super::TRIAL;
use super::edit::{Change, edited};
use super::set::{self, MODE};
use crate::trial::{self, ToUse, Tried};
use crate::util::{self, Fail, fail};
use crate::{net_errors, service, servicefiles};

/// The mode of a service's file: the one the files that ship have.
const SERVICE_FILE_MODE: u32 = 0o644;

/// NET-182: a change of SSH while the trial `ssh` runs. A Spark that does
/// not answer runs no trial: only a change that is tried (`seconds`)
/// needs it.
///
/// # Errors
///
/// NET-182, or Spark's silence for a change that is tried.
pub(super) fn not_tried(seconds: Option<u32>) -> Result<(), Fail> {
    match trial::running(TRIAL) {
        Ok(Some(_)) => Err(net_errors::ssh_is_tried()),
        Ok(None) => Ok(()),
        Err(_) if seconds.is_none() => Ok(()),
        Err(refused) => Err(refused),
    }
}

/// Fails unless sshd is running, which a trial needs: `doing` is the
/// change in a few words.
fn require_running(doing: &str) -> Result<(), Fail> {
    let document = service::shown(None)?;
    let running = document
        .services
        .iter()
        .any(|service| service.name == SERVICE && service.state == State::Running);
    if running {
        return Ok(());
    }
    Err(fail(format!(
        "sshd is not running, so there is no SSH connection for a trial to keep: {doing} \
         without --try"
    )))
}

/// What a trial that Spark began came to, as the command's status: 0 when
/// the change is in use, and 1, with why and what Spark undid, when sshd
/// did not take it.
fn tried(outcome: Tried, in_use: &str) -> i32 {
    match outcome {
        Tried::InUse => {
            println!("{in_use}");
            0
        }
        Tried::Undone { why, undone } => {
            eprintln!("ssh-server: {why}");
            println!("{undone}");
            1
        }
    }
}

/// `ssh-server set ... --try SECONDS`, as root: the change in sshd_config
/// as the trial `ssh`, which Spark's reload of sshd puts to use.
///
/// # Errors
///
/// The refusals of [`set::run`], a sshd that is not running, or Spark's
/// refusal of the trial.
pub(super) fn try_set(change: &Change, seconds: u32) -> Result<i32, Fail> {
    let (accounts, groups) = set::known()?;
    let path = Path::new(PATH);
    let old = set::read_config(path)?;
    let new = edited(&old, change, &accounts, &groups).map_err(set::refused)?;
    if new == old {
        println!("ssh-server: {PATH} says so already; nothing is tried");
        return Ok(0);
    }
    require_running("make the change")?;
    let outcome =
        trial::try_text(TRIAL, path, new.as_bytes(), MODE, seconds, ToUse::Reload(SERVICE))?;
    Ok(tried(
        outcome,
        &format!(
            "ssh-server: sshd reads {PATH} for the connections that come next, and the ones \
             that are open stay as they are. Spark undoes the change at {} unless it is kept, \
             with `ssh-server keep`.",
            trial::undone_at(seconds)
        ),
    ))
}

/// `ssh-server off --try SECONDS`, as root: sshd's service file with
/// `enabled = false` as the trial `ssh`, and sshd stopped; Spark's undo
/// puts the file back and starts sshd again. The file is its override
/// when that sets `enabled`, as the lab image's does.
///
/// # Errors
///
/// A service file that does not read, a sshd that is not running, or
/// Spark's refusal of the trial.
pub(super) fn try_off(seconds: u32) -> Result<i32, Fail> {
    if servicefiles::at_boot().get(SERVICE) == Some(&false) {
        println!("SSH is off already: nothing is tried.");
        return Ok(0);
    }
    let (path, new) = servicefiles::deciding_file(SERVICE, false)?;
    require_running("turn it off")?;
    let outcome = trial::try_text(
        TRIAL,
        &path,
        new.as_bytes(),
        SERVICE_FILE_MODE,
        seconds,
        ToUse::Stop(SERVICE),
    )?;
    Ok(tried(
        outcome,
        &format!(
            "SSH is off: sshd is stopped, which ended the SSH sessions that were open. Spark \
             turns it on again at {} unless the change is kept, with `ssh-server keep`.",
            trial::undone_at(seconds)
        ),
    ))
}

/// `ssh-server keep` and `ssh-server undo`: what the trial `ssh` comes to,
/// as root, whom Spark's control socket is for.
///
/// # Errors
///
/// NET-179 with no trial, or Spark's refusal.
pub(super) fn end(verb: &str) -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root keeps or undoes a change of SSH: run it with elevate, as `elevate \
             ssh-server {verb}`"
        )));
    }
    let words = trial::end(verb, TRIAL, || net_errors::nothing_is_tried("SSH's settings"))?;
    match verb {
        "keep" => println!("Kept: SSH stays as it is now."),
        _ => println!("{words}"),
    }
    Ok(0)
}
