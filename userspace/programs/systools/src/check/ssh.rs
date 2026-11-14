// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The SSH checks of dukovo-check(8): whether passwords sign in over SSH,
//! who may, and whether guessing passwords is slowed by blocking.

use accounts::sshd_config::{self, Settings};
use serde::Deserialize;

use super::context::Context;
use super::line::{Outcome, Problem};
use super::services::is_on;

/// Where blocking is turned on and off, which authd reads (authd(8)).
const BLOCKING: &str = "/system/settings/blocking.toml";

/// What `blocking.toml` says of whether authd blocks, which is all that
/// the check reads of it.
#[derive(Deserialize)]
struct Blocking {
    on: Option<bool>,
}

/// The settings sshd signs people in by: `None` while sshd is not on,
/// when nothing signs in over SSH whatever its file says.
///
/// # Errors
///
/// Why sshd's service or its file cannot be read, as the reason a check is
/// skipped.
fn sshd(context: &Context) -> Result<Option<Settings>, String> {
    match context.service("sshd")? {
        Some(service) if is_on(service) => {}
        _ => return Ok(None),
    }
    let text = context.read(sshd_config::PATH)?.unwrap_or_default();
    let settings = sshd_config::parse(&text).map_err(|mistakes| {
        let first = &mistakes[0];
        format!("{} line {}: {}", sshd_config::PATH, first.line, first.why)
    })?;
    Ok(Some(settings))
}

/// Warns when passwords sign in over SSH: a password can be guessed, and a
/// key cannot.
pub(super) fn passwords(context: &Context) -> Vec<Outcome> {
    let outcome = match sshd(context) {
        Err(why) => Outcome::skipped(why),
        Ok(None) => Outcome::passed("SSH is off"),
        Ok(Some(settings)) if !settings.passwords => Outcome::passed("only keys sign in over SSH"),
        Ok(Some(_)) => Outcome::warning(
            Problem::SshPasswords,
            "passwords sign in over SSH (sshd_config: PasswordAuthentication yes)",
        ),
    };
    vec![outcome]
}

/// Warns when passwords sign in over SSH and nothing names who may, so
/// that the password of every person's account is one to guess, the
/// accounts nobody uses among them.
pub(super) fn who_may_sign_in(context: &Context) -> Vec<Outcome> {
    let outcome = match sshd(context) {
        Err(why) => Outcome::skipped(why),
        Ok(None) => Outcome::passed("SSH is off"),
        Ok(Some(settings)) if !settings.passwords => {
            Outcome::passed("only keys sign in, and each account lists its own")
        }
        Ok(Some(settings))
            if settings.allow_users.is_empty() && settings.allow_groups.is_empty() =>
        {
            Outcome::warning(
                Problem::SshAnyone,
                "any account may sign in with its password (sshd_config sets neither AllowUsers \
                 nor AllowGroups)",
            )
        }
        Ok(Some(settings)) => {
            let names: Vec<String> = settings
                .allow_users
                .iter()
                .map(|name| format!("user {name}"))
                .chain(settings.allow_groups.iter().map(|name| format!("group {name}")))
                .collect();
            Outcome::passed(format!("only {} may sign in", names.join(", ")))
        }
    };
    vec![outcome]
}

/// Warns when authd does not block an address that keeps guessing
/// passwords. A missing file or key is blocking on, as authd has it.
pub(super) fn blocking(context: &Context) -> Vec<Outcome> {
    let text = match context.read(BLOCKING) {
        Ok(text) => text.unwrap_or_default(),
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let on = match toml::from_str::<Blocking>(&text) {
        Ok(blocking) => blocking.on.unwrap_or(true),
        Err(why) => {
            let why = why.message().lines().next().unwrap_or_default();
            return vec![Outcome::skipped(format!("{BLOCKING} does not read: {why}"))];
        }
    };
    let outcome = if on {
        Outcome::passed("on: an address that keeps guessing passwords is blocked for a while")
    } else {
        Outcome::warning(Problem::BlockingOff, "blocking is off (blocking.toml: on = false)")
    };
    vec![outcome]
}

#[cfg(test)]
mod tests {
    use spark_api::show::State;

    use super::super::context::planted::{System, service};
    use super::super::line::Verdict;
    use super::*;

    fn one(outcomes: Vec<Outcome>) -> Outcome {
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        outcomes.into_iter().next().unwrap()
    }

    /// A system whose sshd is in `state`, with `config` as its sshd_config.
    fn with(state: State, config: &str) -> System {
        let system = System::new(vec![service("sshd", state)]);
        system.plant(sshd_config::PATH, config);
        system
    }

    #[test]
    fn passwords_that_sign_in_over_ssh_warn_only_while_sshd_is_on() {
        let on = with(State::Running, "PasswordAuthentication yes\n");
        let outcome = one(passwords(&on.context));
        assert_eq!(outcome.verdict, Verdict::Warning);
        assert_eq!(outcome.id.as_deref(), Some("SEC-110"));
        assert_eq!(
            outcome.detail,
            "passwords sign in over SSH (sshd_config: PasswordAuthentication yes)"
        );
        let off = with(State::Disabled, "PasswordAuthentication yes\n");
        assert_eq!(one(passwords(&off.context)).detail, "SSH is off");
        let keys = with(State::Running, "PasswordAuthentication no\n");
        assert_eq!(one(passwords(&keys.context)).detail, "only keys sign in over SSH");
        // No file is sshd's own defaults, which are keys alone.
        let none = System::new(vec![service("sshd", State::Running)]);
        assert_eq!(one(passwords(&none.context)).verdict, Verdict::Passed);
    }

    #[test]
    fn a_service_that_waits_to_restart_or_start_is_on() {
        for state in [State::Starting, State::Waiting, State::Pending, State::Listening] {
            let system = with(state, "PasswordAuthentication yes\n");
            assert_eq!(one(passwords(&system.context)).verdict, Verdict::Warning, "{state:?}");
        }
        for state in [State::Failed, State::Stopped, State::Done] {
            let system = with(state, "PasswordAuthentication yes\n");
            assert_eq!(one(passwords(&system.context)).verdict, Verdict::Passed, "{state:?}");
        }
    }

    #[test]
    fn passwords_for_anyone_warn_and_a_named_group_or_user_does_not() {
        let anyone = with(State::Running, "PasswordAuthentication yes\n");
        let outcome = one(who_may_sign_in(&anyone.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-111")));
        let group = with(State::Running, "PasswordAuthentication yes\nAllowGroups admins\n");
        let outcome = one(who_may_sign_in(&group.context));
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(outcome.detail, "only group admins may sign in");
        let users = with(State::Running, "PasswordAuthentication yes\nAllowUsers alice bob\n");
        assert_eq!(
            one(who_may_sign_in(&users.context)).detail,
            "only user alice, user bob may sign in"
        );
        let keys = with(State::Running, "PasswordAuthentication no\n");
        assert_eq!(one(who_may_sign_in(&keys.context)).verdict, Verdict::Passed);
        let off = with(State::Disabled, "PasswordAuthentication yes\n");
        assert_eq!(one(who_may_sign_in(&off.context)).detail, "SSH is off");
    }

    #[test]
    fn a_file_sshd_does_not_use_skips_the_checks_with_the_line() {
        let system = with(State::Running, "PasswordAuthentication yes\nBanner none\n");
        for outcome in [passwords(&system.context), who_may_sign_in(&system.context)] {
            let outcome = one(outcome);
            assert_eq!(outcome.verdict, Verdict::Skipped);
            assert!(
                outcome.detail.starts_with(
                    "/system/settings/ssh/sshd_config line 2: sshd does not take Banner"
                ),
                "{}",
                outcome.detail
            );
        }
    }

    #[test]
    fn spark_that_cannot_say_skips_the_ssh_checks() {
        let mut system = System::new(Vec::new());
        system.context.services = Err("not running".into());
        assert_eq!(one(passwords(&system.context)).verdict, Verdict::Skipped);
        assert_eq!(one(who_may_sign_in(&system.context)).verdict, Verdict::Skipped);
    }

    #[test]
    fn blocking_is_on_unless_the_file_says_off() {
        let system = System::new(Vec::new());
        // No file, and a file that leaves `on` out, are blocking on.
        assert_eq!(one(blocking(&system.context)).verdict, Verdict::Passed);
        system.plant(BLOCKING, "tries = 5\nnever = [\"192.168.1.0/24\"]\n");
        assert_eq!(one(blocking(&system.context)).verdict, Verdict::Passed);
        system.plant(BLOCKING, "on = true\n");
        assert_eq!(one(blocking(&system.context)).verdict, Verdict::Passed);
        system.plant(BLOCKING, "on = false\ntries = 5\n");
        let outcome = one(blocking(&system.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-112")));
        assert_eq!(outcome.detail, "blocking is off (blocking.toml: on = false)");
    }

    #[test]
    fn a_blocking_file_that_does_not_read_skips_the_check() {
        let system = System::new(Vec::new());
        for text in ["on = \"yes\"\n", "on = \n"] {
            system.plant(BLOCKING, text);
            let outcome = one(blocking(&system.context));
            assert_eq!(outcome.verdict, Verdict::Skipped, "{text}");
            assert!(outcome.detail.starts_with("/system/settings/blocking.toml does not read: "));
        }
    }
}
