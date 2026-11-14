// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! authd in Rescue (ADR 0540). dukovo-rescue(8) starts authd and sshd
//! when the installed system had SSH on, so that its administrators can
//! repair it from where they reached it before. authd then reads the
//! installed system's accounts, password hashes and keys from the root
//! Rescue mounted ([`accounts::rescue`]), answers only SSH's sign-ins,
//! and those of the installed system's administrators alone, and changes
//! nothing: no account, no password, and nothing in the root. What it
//! keeps of a sign-in, and of the addresses it blocks, is in Rescue's own
//! memory. An administrator's SSH session is Rescue's menu, as root, as
//! an elevated command runs, with no password asked again.

use std::os::fd::OwnedFd;

use accounts::User;
use accounts::request::Request;

use super::Files;
use crate::elevate::{self, Plan};

/// Rescue's menu, which an administrator's SSH session runs.
const MENU: &str = "/system/commands/dukovo-rescue";

/// Why authd refuses, in Rescue, what is not an SSH sign-in.
const NOTHING_CHANGED: &str = "Rescue changes no account: authd answers only SSH's sign-ins here";

/// Why authd refuses, in Rescue, anyone but an administrator.
const ADMINS_ONLY: &str = "only this computer's administrators sign in to Rescue";

/// Why authd, in Rescue, refuses `request`, if it does: anything but
/// SSH's sign-ins and the list of blocked addresses, and the sign-ins of
/// anyone but the installed system's administrators, as `files` has them.
pub(super) fn refuses(request: &Request, files: &Files) -> Option<String> {
    let admin = |name: &str| {
        files
            .user(name)
            .is_some_and(|user| user.uid != 0 && accounts::is_admin(user, &files.groups))
    };
    match request {
        Request::SshKey { name, .. }
        | Request::SshPassword { name, .. }
        | Request::SshSession { name, .. } => (!admin(name)).then(|| ADMINS_ONLY.to_string()),
        Request::SshFailed { .. }
        | Request::SshEnded { .. }
        | Request::Blocked
        | Request::Unblock { .. } => None,
        _ => Some(NOTHING_CHANGED.to_string()),
    }
}

/// Starts Rescue's menu for the administrator `admin`, who signed in over
/// SSH at `connection` (SSH_CONNECTION's words), on `terminal`, whose
/// type is `term`: as root, as an elevated command runs, told who signed
/// in. Returns its process's number.
pub(super) fn start_menu(
    admin: &User,
    terminal: &OwnedFd,
    connection: &str,
    term: &str,
) -> Result<u64, String> {
    // Rescue's own root account, not the installed system's.
    let live = std::fs::read_to_string(accounts::PASSWD).unwrap_or_default();
    let root = elevate::root_account(accounts::users(&live).entries().cloned());
    let shown = format!("{MENU} --ssh {}", admin.name);
    let mut env = elevate::environment(&root, admin, term, &[], &shown);
    env.push(format!("SSH_CONNECTION={connection}"));
    let copy = || terminal.try_clone().map_err(|err| format!("cannot start Rescue's menu: {err}"));
    let fds = [copy()?, copy()?, copy()?];
    let plan = Plan {
        program: MENU.as_bytes().to_vec(),
        argv: vec![b"dukovo-rescue".to_vec(), b"--ssh".to_vec(), admin.name.as_bytes().to_vec()],
        directory: b"/".to_vec(),
        shown,
    };
    elevate::start(&fds, &plan, &env, None)
}
