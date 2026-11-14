// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The network checks of dukovo-check(8): what other machines reach, the
//! firewall's rules, and whether the firewall service loaded them.

use spark_api::show::State;

use super::context::Context;
use super::line::{Outcome, Problem};
use crate::firewall::{self, Action, Protocol, Rule};

/// The settings file the firewall's rules are in, which `firewall load`
/// reads.
const RULES: &str = firewall::SETTINGS;

/// Lists the ports other machines reach, as `firewall status` says them,
/// so that the report says what is open. It never warns: a port a service
/// listens on is open by the service's file, and the checks that follow
/// look at whether it should be.
pub(super) fn reachable(context: &Context) -> Vec<Outcome> {
    let reached = match &context.reached {
        Ok(reached) => reached,
        Err(why) => return vec![Outcome::skipped(why.clone())],
    };
    if reached.is_empty() {
        return vec![Outcome::passed("nothing listens for other machines")];
    }
    let ports: Vec<String> = reached
        .iter()
        .map(|reached| {
            let protocol = if reached.tcp { "tcp" } else { "udp" };
            let only = if reached.everywhere { "" } else { ", some machines" };
            format!("{protocol} {} ({}{only})", reached.port, reached.program)
        })
        .collect();
    vec![Outcome::passed(format!("from other machines: {}", ports.join(", ")))]
}

/// Whether `rule` lets every port in from every address, so that it
/// decides for everything that is started: it names no interface and no
/// program, and is no ping's.
fn lets_everything_in(rule: &Rule) -> bool {
    let every_port = rule.ports.is_none_or(|(first, last)| first <= 1 && last == u16::MAX);
    let every_address = rule.from.is_none_or(|(_, prefix)| prefix == 0);
    rule.protocol != Protocol::Ping
        && every_port
        && every_address
        && rule.on.is_none()
        && rule.program.is_none()
}

/// Warns of a rule that allows every port from everywhere, which makes
/// each socket a person's program opens reachable from other machines,
/// where the firewall otherwise leaves those to this machine alone. The
/// first rule that matches everything decides, so a rule after a `deny`
/// of everything never counts.
pub(super) fn rules(context: &Context) -> Vec<Outcome> {
    let text = match context.read(RULES) {
        Ok(text) => text.unwrap_or_default(),
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let rules = firewall::rules_in(&text);
    for (number, rule) in rules.iter().enumerate() {
        if !lets_everything_in(rule) {
            continue;
        }
        if rule.action == Action::Deny {
            break;
        }
        return vec![Outcome::warning(
            Problem::FirewallAllowsAll,
            format!("rule {} (`{rule}`) lets every port in from everywhere", number + 1),
        )];
    }
    let count = match rules.len() {
        0 => {
            return vec![Outcome::passed(
                "no rules: other machines reach only what services listen on",
            )];
        }
        1 => "1 rule".to_string(),
        count => format!("{count} rules"),
    };
    vec![Outcome::passed(format!("{count}; none lets every port in from everywhere"))]
}

/// Warns when the `firewall` service, which gives the kernel the rules at
/// each start, is off or gave up.
///
/// The service loads the rules and ends, so a start that went well leaves
/// it stopped, with the status 0 that it ended with: that is a firewall
/// that is on. One that was stopped by a signal, or never started, is not.
pub(super) fn service_on(context: &Context) -> Vec<Outcome> {
    let service = match context.service("firewall") {
        Ok(service) => service,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let Some(service) = service else {
        return vec![Outcome::warning(Problem::FirewallOff, "there is no firewall service")];
    };
    let loaded = Outcome::passed("the firewall service loaded its rules");
    let outcome = match service.state {
        State::Running | State::Done => loaded,
        State::Stopped if service.last_status == Some(0) => loaded,
        State::Failed => Outcome::warning(
            Problem::FirewallOff,
            "the firewall service failed, so the rules are not loaded",
        ),
        State::Disabled | State::Stopped => {
            Outcome::warning(Problem::FirewallOff, "the firewall service is off")
        }
        other => Outcome::skipped(format!("the firewall service is {}", other.word())),
    };
    vec![outcome]
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::{System, service};
    use super::super::line::Verdict;
    use super::*;
    use crate::firewall::Reached;

    fn one(outcomes: Vec<Outcome>) -> Outcome {
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        outcomes.into_iter().next().unwrap()
    }

    fn reached(tcp: bool, port: u16, program: &str, everywhere: bool) -> Reached {
        Reached { tcp, port, program: program.into(), everywhere }
    }

    #[test]
    fn what_other_machines_reach_is_listed_and_never_warns() {
        let mut system = System::new(Vec::new());
        system.context.reached = Ok(vec![
            reached(true, 22, "sshd", true),
            reached(true, 443, "nginx", true),
            reached(false, 53, "resolved", false),
        ]);
        let outcome = one(reachable(&system.context));
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(
            outcome.detail,
            "from other machines: tcp 22 (sshd), tcp 443 (nginx), udp 53 (resolved, some machines)"
        );
        system.context.reached = Ok(Vec::new());
        assert_eq!(one(reachable(&system.context)).detail, "nothing listens for other machines");
    }

    #[test]
    fn a_firewall_that_cannot_say_is_skipped_with_its_words() {
        let mut system = System::new(Vec::new());
        system.context.reached = Err("firewall: this needs the firewall capability".into());
        let outcome = one(reachable(&system.context));
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert!(outcome.detail.contains("firewall capability"));
    }

    #[test]
    fn a_rule_that_allows_every_port_from_everywhere_warns_and_a_narrow_one_does_not() {
        let system = System::new(Vec::new());
        assert_eq!(
            one(rules(&system.context)).detail,
            "no rules: other machines reach only what services listen on"
        );
        for allowing in ["allow", "allow tcp", "allow udp 1-65535", "allow tcp from 0.0.0.0/0"] {
            system.plant(RULES, &format!("# ssh\nallow tcp 22\n{allowing}\n"));
            let outcome = one(rules(&system.context));
            assert_eq!(outcome.verdict, Verdict::Warning, "{allowing}");
            assert_eq!(outcome.id.as_deref(), Some("SEC-101"));
            assert!(outcome.detail.starts_with("rule 2 (`"), "{}", outcome.detail);
        }
        for narrow in [
            "allow tcp 22",
            "allow tcp 8000-8100",
            "allow from 192.168.1.0/24",
            "allow on net1",
            "allow program /users/alice/bin/server",
            "allow ping",
            "deny",
        ] {
            system.plant(RULES, &format!("{narrow}\n"));
            let outcome = one(rules(&system.context));
            assert_eq!(outcome.verdict, Verdict::Passed, "{narrow}");
            assert_eq!(outcome.detail, "1 rule; none lets every port in from everywhere");
        }
    }

    #[test]
    fn the_first_rule_that_matches_everything_decides() {
        let system = System::new(Vec::new());
        system.plant(RULES, "deny\nallow\n");
        let outcome = one(rules(&system.context));
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(outcome.detail, "2 rules; none lets every port in from everywhere");
        system.plant(RULES, "allow tcp 22\nallow\ndeny\n");
        assert_eq!(one(rules(&system.context)).verdict, Verdict::Warning);
    }

    #[test]
    fn a_line_that_is_no_rule_is_left_out_as_the_firewall_leaves_it_out() {
        let system = System::new(Vec::new());
        system.plant(RULES, "permit everything\nallow tcp 22\n");
        assert_eq!(
            one(rules(&system.context)).detail,
            "1 rule; none lets every port in from everywhere"
        );
    }

    #[test]
    fn the_firewall_service_warns_when_it_is_off_or_gave_up() {
        let check = |services| {
            let system = System::new(services);
            one(service_on(&system.context))
        };
        let done = check(vec![service("firewall", State::Done)]);
        assert_eq!(done.verdict, Verdict::Passed);
        assert_eq!(check(vec![service("firewall", State::Running)]).verdict, Verdict::Passed);
        // What a start that went well leaves: the service ended with status
        // 0, and is stopped.
        let mut ended = service("firewall", State::Stopped);
        ended.last_status = Some(0);
        assert_eq!(check(vec![ended]).verdict, Verdict::Passed);
        // One killed by a signal, or never started, is not on.
        let mut killed = service("firewall", State::Stopped);
        killed.last_status = Some(15);
        for outcome in [
            check(vec![killed]),
            check(vec![service("firewall", State::Failed)]),
            check(vec![service("firewall", State::Disabled)]),
            check(vec![service("firewall", State::Stopped)]),
        ] {
            assert_eq!(outcome.verdict, Verdict::Warning, "{}", outcome.detail);
            assert_eq!(outcome.id.as_deref(), Some("SEC-102"));
        }
        let missing = check(vec![service("sshd", State::Running)]);
        assert_eq!(
            (missing.verdict, missing.detail.as_str()),
            (Verdict::Warning, "there is no firewall service")
        );
        // A start still on its way is not judged.
        assert_eq!(check(vec![service("firewall", State::Starting)]).verdict, Verdict::Skipped);
    }

    #[test]
    fn spark_that_cannot_say_skips_the_service_check() {
        let mut system = System::new(Vec::new());
        system.context.services = Err("not running".into());
        assert_eq!(one(service_on(&system.context)).verdict, Verdict::Skipped);
    }
}
