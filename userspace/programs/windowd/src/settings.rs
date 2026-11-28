// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the person has set of the desktop (windowd.json(5), ADR 0721).

use std::collections::BTreeSet;

/// Which applications may draw a part of their windows' title bars
/// (ADR 0719).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleBars {
    /// Whether an application with no entry of its own has the full
    /// title bar whatever it asks. Otherwise it decides.
    pub all_full: bool,
    /// The applications that have the full title bar whatever they ask.
    pub full: BTreeSet<String>,
    /// The applications that decide, whatever the others do.
    pub shared: BTreeSet<String>,
}

impl TitleBars {
    /// Whether the application `app` may share its windows' title
    /// bars. An application of no name has no entry.
    pub fn lets(&self, app: &str) -> bool {
        if self.full.contains(app) {
            return false;
        }
        self.shared.contains(app) || !self.all_full
    }

    /// Gives the application `app` an entry of its own if it needs one
    /// to be let, or not let, share its windows' title bars.
    pub fn set(&mut self, app: &str, lets: bool) {
        self.full.remove(app);
        self.shared.remove(app);
        if lets != self.lets(app) {
            let entries = if lets { &mut self.shared } else { &mut self.full };
            entries.insert(app.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_decides_unless_the_person_says_otherwise() {
        let mut bars = TitleBars::default();
        assert!(bars.lets("org.example.browser"));
        assert!(bars.lets(""));
        bars.set("org.example.browser", false);
        assert!(!bars.lets("org.example.browser"));
        assert!(bars.lets("org.example.editor"));
        // An entry that says what holds anyway is not kept.
        bars.set("org.example.browser", true);
        assert_eq!(bars, TitleBars::default());
    }

    #[test]
    fn an_applications_own_entry_wins_over_the_rule_for_all() {
        let mut bars = TitleBars { all_full: true, ..TitleBars::default() };
        assert!(!bars.lets("org.example.browser"));
        bars.set("org.example.browser", true);
        assert!(bars.lets("org.example.browser"));
        assert!(!bars.lets("org.example.editor"));
        assert_eq!(bars.shared.len(), 1);
        bars.set("org.example.browser", false);
        assert!(bars.shared.is_empty() && bars.full.is_empty());
    }
}
