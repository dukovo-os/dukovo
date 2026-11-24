// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an MCP client is offered of the console's actions, and how far
//! its scopes reach (ADR 0690).
//!
//! The registry says of each action whether it reads or changes, as whom
//! its command runs, and whether it takes or answers a secret
//! ([`Registered`]). [`offer`] reads only that, so an action that is
//! added to the registry is offered, or not, by what it is, and there is
//! no list of names here to keep.
//!
//! A client's scopes are its own grammar (`authd::console::clients`):
//!
//! | Scope | Gives |
//! |---|---|
//! | `AREA:read` | the area's reads that run as the client's owner |
//! | `AREA` | those, the reads that run as an administrator, and asking for the area's changes |
//! | `*:read`, `*` | the same, of every area |

use super::super::actions::{Field, Registered, Runs};
use super::super::errors::{Failure, Kind};
use crate::authd::console::clients;

/// The areas whose changes decide who gets in to the machine: the
/// accounts, each person's own account, the sign-in rules, SSH, the
/// private network, and the console's own settings, which hold the MCP
/// clients. A client reads them and never asks to change them.
pub const WHO_GETS_IN: [&str; 6] = ["account", "accounts", "console", "private", "security", "ssh"];

/// What a client may do with an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// Run it: a read that runs as the client's owner.
    Read,
    /// Run it: a read that runs as an administrator.
    ReadAll,
    /// Ask for it: a change, which runs when an administrator approves.
    Ask,
}

/// Why an action is offered to no client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Never {
    /// It takes a password, a key or a token, or answers one.
    Secret,
    /// Its command reads a document on its input.
    Document,
    /// It is a read that runs as a task.
    LongRead,
    /// It changes who gets in.
    WhoGetsIn,
    /// It is a change that runs as the person, not as an administrator.
    OwnChange,
}

impl Never {
    /// What a client is told, after the action's name.
    pub fn words(self) -> &'static str {
        match self {
            Never::Secret => "takes or shows a password, a key or a token, which no AI client sees",
            Never::Document => "takes a document, which no AI client sends",
            Never::LongRead => "runs as a task, which no AI client reads",
            Never::WhoGetsIn => "changes who gets in, which no AI client may ask for",
            Never::OwnChange => "changes its owner's own account, which no AI client may ask for",
        }
    }
}

/// What a client may do with `action`, by what the registry says of it.
///
/// # Errors
///
/// Why no client is offered it.
pub fn offer(action: &dyn Registered) -> Result<Offer, Never> {
    let fields = action.fields();
    if action.secret_answer() || fields.iter().any(Field::is_secret) {
        return Err(Never::Secret);
    }
    if fields.iter().any(Field::is_input) {
        return Err(Never::Document);
    }
    match (action.changes(), action.runs()) {
        (false, _) if action.long() => Err(Never::LongRead),
        (false, Runs::AsThePerson) => Ok(Offer::Read),
        (false, Runs::AsAdministrator) => Ok(Offer::ReadAll),
        (true, _) if WHO_GETS_IN.contains(&action.area()) => Err(Never::WhoGetsIn),
        (true, Runs::AsThePerson) => Err(Never::OwnChange),
        (true, Runs::AsAdministrator) => Ok(Offer::Ask),
    }
}

/// How far a client reaches into an area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Not at all.
    #[default]
    Off,
    /// The reads that run as its owner: what the owner's account sees
    /// before it unlocks.
    Read,
    /// Every read, and asking for changes.
    All,
}

impl Level {
    /// What `offer` needs.
    fn needed(offer: Offer) -> Level {
        match offer {
            Offer::Read => Level::Read,
            Offer::ReadAll | Offer::Ask => Level::All,
        }
    }
}

/// What a client's scopes give.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// What `*` gives, when the scopes have it.
    every: Level,
    /// What each area they name gives.
    areas: Vec<(String, Level)>,
}

impl Reach {
    /// What the scopes `words`, joined by commas, give. None when one of
    /// them is not a client's scope.
    pub fn parse(words: &str) -> Option<Reach> {
        let mut reach = Reach::default();
        for scope in words.split(',').filter(|scope| !scope.is_empty()) {
            if !clients::scope_ok(scope) {
                return None;
            }
            let (area, level) = match scope.strip_suffix(":read") {
                Some(area) => (area, Level::Read),
                None => (scope, Level::All),
            };
            if area == "*" {
                reach.every = reach.every.max(level);
            } else {
                reach.areas.push((area.into(), level));
            }
        }
        Some(reach)
    }

    /// How far it reaches into `area`.
    pub fn level(&self, area: &str) -> Level {
        let named = self.areas.iter().filter(|(name, _)| name == area).map(|(_, level)| *level);
        named.fold(self.every, Level::max)
    }

    /// What a client that reaches this far may do with `action`.
    ///
    /// # Errors
    ///
    /// ADM-144, with what to tell the client: no client is offered the
    /// action, or this one's scopes do not reach it.
    pub fn allows(&self, action: &dyn Registered) -> Result<Offer, Failure> {
        let may_not = |words: String| Failure::new(Kind::ClientMayNot, words);
        let (id, area) = (action.id(), action.area());
        let offer = offer(action).map_err(|never| may_not(format!("{id} {}", never.words())))?;
        let level = self.level(area);
        if level >= Level::needed(offer) {
            return Ok(offer);
        }
        Err(may_not(match (offer, level) {
            (Offer::Ask, _) => format!("this AI client may not ask for changes to {area}"),
            (Offer::ReadAll, Level::Read) => format!(
                "{id} is an administrator's read, and this AI client reads of {area} what its \
                 owner sees before unlocking"
            ),
            (Offer::Read | Offer::ReadAll, _) => format!("this AI client may not read {area}"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::actions;

    fn action(id: &str) -> &'static dyn Registered {
        let found = actions::offered().iter().copied().find(|action| action.id() == id);
        found.unwrap_or_else(|| panic!("no action {id}"))
    }

    #[test]
    fn an_action_is_offered_by_what_the_registry_says_of_it() {
        for (id, offered) in [
            ("service.list", Ok(Offer::Read)),
            ("logs.read-all", Ok(Offer::ReadAll)),
            ("service.restart", Ok(Offer::Ask)),
            // Takes a password; answers a key; takes a document.
            ("accounts.add", Err(Never::Secret)),
            ("backups.key", Err(Never::Secret)),
            ("reports.redact", Err(Never::Secret)),
            // Who gets in: read, never changed.
            ("accounts.list-all", Ok(Offer::ReadAll)),
            ("accounts.token-revoke", Err(Never::WhoGetsIn)),
            ("account.name", Err(Never::WhoGetsIn)),
            // A change that runs as the person.
            ("snapshots.restore", Err(Never::OwnChange)),
        ] {
            assert_eq!(offer(action(id)), offered, "{id}");
        }
    }

    #[test]
    fn no_client_is_offered_a_secret_or_a_change_to_who_gets_in() {
        let mut offered = [0usize; 3];
        for action in actions::offered() {
            let Ok(offer) = offer(*action) else { continue };
            let id = action.id();
            assert!(!action.secret_answer(), "{id} answers a secret");
            for field in action.fields() {
                assert!(!field.is_secret() && !field.is_input(), "{id} takes {}", field.name);
            }
            match offer {
                Offer::Read => {
                    assert!(!action.changes() && action.runs() == Runs::AsThePerson, "{id}");
                    offered[0] += 1;
                }
                Offer::ReadAll => {
                    assert!(!action.changes() && !action.long(), "{id}");
                    offered[1] += 1;
                }
                Offer::Ask => {
                    assert!(action.changes() && action.runs() == Runs::AsAdministrator, "{id}");
                    assert!(!WHO_GETS_IN.contains(&action.area()), "{id} changes who gets in");
                    offered[2] += 1;
                }
            }
        }
        // The console has reads of both kinds and changes to offer; were
        // one of these none, a rule above would have gone wrong.
        assert!(offered.iter().all(|count| *count >= 20), "{offered:?}");
    }

    #[test]
    fn every_area_that_decides_who_gets_in_is_one_of_the_registry_s() {
        for area in WHO_GETS_IN {
            let has = actions::offered().iter().any(|action| action.area() == area);
            assert!(has, "no action is in the area {area}");
        }
    }

    #[test]
    fn scopes_reach_an_area_as_far_as_the_most_of_them() {
        let reach = Reach::parse("*:read,services,logs:read").expect("scopes");
        assert_eq!(reach.level("services"), Level::All);
        assert_eq!(reach.level("logs"), Level::Read);
        assert_eq!(reach.level("storage"), Level::Read);
        let reach = Reach::parse("logs:read,logs,storage:read").expect("scopes");
        assert_eq!(reach.level("logs"), Level::All);
        assert_eq!(reach.level("storage"), Level::Read);
        assert_eq!(reach.level("services"), Level::Off);
        assert_eq!(Reach::parse("*").expect("scopes").level("anything"), Level::All);
        assert_eq!(Reach::parse("").expect("no scopes").level("logs"), Level::Off);
        for wrong in ["logs:write", "Logs", "*,logs:read:read", "a b"] {
            assert_eq!(Reach::parse(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_client_does_what_its_scopes_reach() {
        let reads = Reach::parse("services:read,logs:read").expect("scopes");
        let whole = Reach::parse("services,logs").expect("scopes");
        let other = Reach::parse("storage").expect("scopes");
        let refused = |reach: &Reach, id: &str| {
            let failure = reach.allows(action(id)).expect_err(id);
            assert_eq!(failure.kind, Kind::ClientMayNot);
            failure.detail
        };
        assert_eq!(reads.allows(action("service.list")).ok(), Some(Offer::Read));
        assert!(refused(&reads, "logs.read-all").contains("an administrator's read"));
        assert!(refused(&reads, "service.restart").contains("may not ask for changes to services"));
        assert_eq!(whole.allows(action("service.list")).ok(), Some(Offer::Read));
        assert_eq!(whole.allows(action("logs.read-all")).ok(), Some(Offer::ReadAll));
        assert_eq!(whole.allows(action("service.restart")).ok(), Some(Offer::Ask));
        assert!(refused(&other, "service.list").contains("may not read services"));
        assert!(refused(&other, "logs.read-all").contains("may not read logs"));
        // What no client is offered, whatever it reaches.
        let all = Reach::parse("*").expect("scopes");
        assert!(refused(&all, "accounts.add").contains("a password, a key or a token"));
        assert!(refused(&all, "accounts.token-revoke").contains("who gets in"));
        assert!(refused(&all, "snapshots.restore").contains("own account"));
    }
}
