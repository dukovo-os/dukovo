// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lines `cert get --progress` writes for a person or a page that
//! watches an order: `step N of M: WORDS` when a step begins, again with
//! other words when what it is doing changes, and `progress P%` for how far
//! the step has come (admind(8)'s tasks read them).

use std::cell::Cell;

/// Which step an order is at, and whether it says so.
pub struct Steps {
    total: usize,
    at: Cell<usize>,
    on: bool,
}

impl Steps {
    /// Steps that say nothing: an order nobody watches by its steps.
    pub fn off() -> Steps {
        Steps { total: 0, at: Cell::new(0), on: false }
    }

    /// An order of `total` steps, each of which is told.
    pub fn new(total: usize) -> Steps {
        Steps { total, at: Cell::new(0), on: true }
    }

    /// Whether the steps are told.
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Begins the next step, called `words`.
    pub fn next(&self, words: &str) {
        self.at.set(self.at.get() + 1);
        self.tell(words);
    }

    /// Says the step it is at again, with `words` for what it is doing now.
    pub fn again(&self, words: &str) {
        self.tell(words);
    }

    /// Says how far the step has come, 0 to 100.
    pub fn percent(&self, percent: u64) {
        if self.on && self.at.get() > 0 {
            println!("{}", percent_line(percent));
        }
    }

    fn tell(&self, words: &str) {
        if self.on {
            println!("{}", step_line(self.at.get().min(self.total), self.total, words));
        }
    }
}

/// The line that says the order is at step `number` of `total`.
fn step_line(number: usize, total: usize, words: &str) -> String {
    format!("step {number} of {total}: {words}")
}

/// The line that says the step is `percent` done.
fn percent_line(percent: u64) -> String {
    format!("progress {}%", percent.min(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_are_the_ones_a_task_reads_as_progress() {
        assert_eq!(
            step_line(3, 5, "the authority checks port 80"),
            "step 3 of 5: the authority checks port 80"
        );
        assert_eq!(percent_line(40), "progress 40%");
        assert_eq!(percent_line(250), "progress 100%");
    }

    #[test]
    fn steps_that_are_off_count_but_do_not_say_anything() {
        let steps = Steps::off();
        assert!(!steps.is_on());
        steps.next("a");
        steps.again("b");
        steps.percent(10);
        let on = Steps::new(2);
        assert!(on.is_on());
        on.next("one");
        on.next("two");
        // Never beyond the last step.
        on.next("three");
        assert_eq!(step_line(on.at.get().min(on.total), on.total, "three"), "step 2 of 2: three");
    }
}
