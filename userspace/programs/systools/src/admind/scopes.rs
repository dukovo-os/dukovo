// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an API token may do (ADR 0514): its scopes, as authd keeps them,
//! against the area of an action and whether the action reads or changes.
//!
//! A scope is `*`, everything its owner may do; an area's name, such as
//! `services`, for reading and changing what the area holds; or the name
//! with `:read`, for reading alone. A token is never more than its owner:
//! these say what the token may ask for, and the owner's own rights, which
//! the worker and authd hold, say what is done. An elevated change needs
//! the owner to be an administrator besides, which authd checks.

use std::fmt;

/// The scopes of a token, as authd answered them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scopes(Vec<Scope>);

/// One scope.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    /// `*`: anything.
    All,
    /// An area, for reading and changing.
    Area(String),
    /// An area, for reading alone.
    Read(String),
}

impl Scopes {
    /// The scopes of authd's answer, a comma-separated list, or `None`
    /// when it holds none or one that cannot be a scope. A list authd made
    /// is always good; one that is not is refused rather than read loosely.
    pub fn parse(list: &str) -> Option<Scopes> {
        let scopes = list.split(',').map(Scope::parse).collect::<Option<Vec<_>>>()?;
        (!scopes.is_empty()).then_some(Scopes(scopes))
    }

    /// Whether these let a token read, or with `changes` change, what the
    /// area `area` holds.
    pub fn allow(&self, area: &str, changes: bool) -> bool {
        self.0.iter().any(|scope| match scope {
            Scope::All => true,
            Scope::Area(name) => name == area,
            Scope::Read(name) => name == area && !changes,
        })
    }
}

impl Scope {
    /// One scope, if `text` is one: as authd's `scope_ok` takes them.
    fn parse(text: &str) -> Option<Scope> {
        if text == "*" {
            return Some(Scope::All);
        }
        let (area, read) = match text.strip_suffix(":read") {
            Some(area) => (area, true),
            None => (text, false),
        };
        if !area_name_ok(area) {
            return None;
        }
        Some(if read { Scope::Read(area.into()) } else { Scope::Area(area.into()) })
    }
}

/// Whether `name` can be an area's: 1 to 32 lowercase letters, digits and
/// hyphens, starting with a letter.
pub fn area_name_ok(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The scope an action of `area` needs, for the words of a refusal.
pub struct Needed<'a> {
    pub area: &'a str,
    pub changes: bool,
}

impl fmt::Display for Needed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.changes {
            write!(f, "{} or *", self.area)
        } else {
            write!(f, "{0}:read, {0} or *", self.area)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_allows_everything() {
        let scopes = Scopes::parse("*").unwrap();
        assert!(scopes.allow("services", false));
        assert!(scopes.allow("services", true));
        assert!(scopes.allow("firewall", true));
    }

    #[test]
    fn an_area_allows_reading_and_changing_it_alone() {
        let scopes = Scopes::parse("services").unwrap();
        assert!(scopes.allow("services", false));
        assert!(scopes.allow("services", true));
        assert!(!scopes.allow("firewall", false));
        assert!(!scopes.allow("services-extra", false));
    }

    #[test]
    fn a_read_scope_allows_reading_alone() {
        let scopes = Scopes::parse("services:read").unwrap();
        assert!(scopes.allow("services", false));
        assert!(!scopes.allow("services", true));
        assert!(!scopes.allow("firewall", false));
    }

    #[test]
    fn several_scopes_add_up() {
        let scopes = Scopes::parse("services:read,firewall").unwrap();
        assert!(scopes.allow("services", false));
        assert!(!scopes.allow("services", true));
        assert!(scopes.allow("firewall", true));
        assert!(!scopes.allow("storage", false));
    }

    #[test]
    fn a_list_with_anything_that_is_not_a_scope_is_none() {
        for list in [
            "",
            ",",
            "services,",
            "Services",
            "services:write",
            "services:read:read",
            "1services",
            "-services",
            "serv ices",
            "**",
            ":read",
            &"a".repeat(33),
        ] {
            assert_eq!(Scopes::parse(list), None, "{list:?}");
        }
        assert!(Scopes::parse(&"a".repeat(32)).is_some());
        assert!(Scopes::parse("a-1:read").is_some());
    }

    #[test]
    fn the_needed_scope_is_named_in_words() {
        assert_eq!(Needed { area: "services", changes: true }.to_string(), "services or *");
        assert_eq!(
            Needed { area: "services", changes: false }.to_string(),
            "services:read, services or *"
        );
    }
}
