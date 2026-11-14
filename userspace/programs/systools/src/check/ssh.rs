// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The SSH checks of dukovo-check(8): whether passwords sign in over SSH,
//! who may, and whether guessing passwords is slowed by blocking.

use accounts::sshd_config::{self, Settings};

use super::context::Context;
use super::line::{Outcome, Problem};
use super::services::is_on;
use crate::authd::blocking as authd_blocking;

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
/// passwords. The file is read by the code authd reads it with, so that
/// the check and authd cannot disagree about what it says; a missing file
/// is the defaults, which is blocking on.
pub(super) fn blocking(context: &Context) -> Vec<Outcome> {
    let text = match context.read(authd_blocking::SETTINGS) {
        Ok(text) => text.unwrap_or_default(),
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let settings = match authd_blocking::Settings::parse(&text) {
        Ok(settings) => settings,
        Err(why) => {
            let why = why.lines().next().unwrap_or_default();
            return vec![Outcome::skipped(format!(
                "{} does not read: {why}",
                authd_blocking::SETTINGS
            ))];
        }
    };
    let outcome = if settings.on {
        Outcome::passed(format!("on: {}", blocks_in_words(&settings)))
    } else {
        Outcome::warning(Problem::BlockingOff, "blocking is off (blocking.toml: on = false)")
    };
    vec![outcome]
}

/// What the settings make blocking do, as `5 wrong tries in 10 minutes
/// block an address for 10 minutes, doubling up to 1 day`. A block that
/// cannot grow, since the first is the longest, says no doubling.
fn blocks_in_words(settings: &authd_blocking::Settings) -> String {
    let span = authd_blocking::span;
    let blocks = format!(
        "{} wrong tries in {} block an address for {}",
        settings.tries,
        span(settings.within),
        span(settings.first)
    );
    if settings.longest > settings.first {
        format!("{blocks}, doubling up to {}", span(settings.longest))
    } else {
        blocks
    }
}

#[cfg(test)]
mod tests {
    use spark_api::show::State;

    use super::super::context::planted::{System, service};
    use super::super::line::Verdict;
    use super::authd_blocking::SETTINGS;
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
        system.plant(SETTINGS, "tries = 5\nnever = [\"192.168.1.0/24\"]\n");
        assert_eq!(one(blocking(&system.context)).verdict, Verdict::Passed);
        system.plant(SETTINGS, "on = true\n");
        assert_eq!(one(blocking(&system.context)).verdict, Verdict::Passed);
        system.plant(SETTINGS, "on = false\ntries = 5\n");
        let outcome = one(blocking(&system.context));
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-112")));
        assert_eq!(outcome.detail, "blocking is off (blocking.toml: on = false)");
    }

    #[test]
    fn the_default_file_and_no_file_say_the_same_settings() {
        let system = System::new(Vec::new());
        let none = one(blocking(&system.context));
        assert_eq!(
            none.detail,
            "on: 5 wrong tries in 10 minutes block an address for 10 minutes, doubling up to 1 day"
        );
        system.plant(SETTINGS, include_str!("../../../../defaults/blocking.toml"));
        assert_eq!(one(blocking(&system.context)).detail, none.detail);
    }

    #[test]
    fn the_detail_says_the_settings_the_file_sets() {
        let system = System::new(Vec::new());
        system.plant(SETTINGS, "tries = 3\nwithin = \"2m\"\nfirst = \"30m\"\nlongest = \"6h\"\n");
        assert_eq!(
            one(blocking(&system.context)).detail,
            "on: 3 wrong tries in 2 minutes block an address for 30 minutes, doubling up to 6 hours"
        );
        // A first block that is the longest never doubles, so it says so.
        system.plant(SETTINGS, "first = \"1h\"\nlongest = \"1h\"\n");
        assert_eq!(
            one(blocking(&system.context)).detail,
            "on: 5 wrong tries in 10 minutes block an address for 1 hour"
        );
    }

    #[test]
    fn a_blocking_file_authd_would_not_read_skips_the_check() {
        let system = System::new(Vec::new());
        // The file's own mistakes, then values authd refuses.
        for text in [
            "on = \"yes\"\n",
            "on = \n",
            "tries = 1\n",
            "within = \"10 minutes\"\n",
            "never = [\"192.168.1.5/24\"]\n",
            "blocks = 5\n",
        ] {
            system.plant(SETTINGS, text);
            let outcome = one(blocking(&system.context));
            assert_eq!(outcome.verdict, Verdict::Skipped, "{text}");
            assert!(
                outcome.detail.starts_with("/system/settings/blocking.toml does not read: "),
                "{}",
                outcome.detail
            );
        }
    }
}
