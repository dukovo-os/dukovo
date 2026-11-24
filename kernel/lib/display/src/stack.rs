// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who is in front: the stack of holders over the kernel's log (ADR
//! 0702).
//!
//! Only the holder on top of the stack changes what the screens show.
//! The kernel's log is under them all and shows while the stack is empty.
//! The front is passed and never taken:
//!
//! - the log gives way to a holder only while no holder is stacked
//!   ([`Stack::leave_log`]);
//! - the holder in front lends the front to one that is not stacked
//!   ([`Stack::lend`]), and stays under it;
//! - the borrower gives it back ([`Stack::give_back`]), or ends
//!   ([`Stack::end`]), and the front goes to the holder under it, or to
//!   the log.
//!
//! A holder that ends in the middle of the stack leaves it, and the one
//! over it then gives back to the one under it: a lender that is gone
//! takes nothing with it.
//!
//! A holder is known here by the number the kernel gave its description,
//! which is never 0.

/// The most holders that are stacked at once. A text console that lent
/// to a desktop, which lent to a full-screen program, which lent to a
/// prompt, is four.
pub const DEPTH_MAX: usize = 8;

/// Why the front was not passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The caller is not in front.
    NotFront,
    /// The holder to lend to is stacked already.
    Stacked,
    /// [`DEPTH_MAX`] holders are stacked.
    Full,
}

/// The front going from one to another. `None` is the kernel's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passed {
    /// Who was in front.
    pub from: Option<u32>,
    /// Who is in front now.
    pub to: Option<u32>,
}

/// The stack of holders, the one in front last.
#[derive(Clone, Copy, Debug)]
pub struct Stack {
    holders: [u32; DEPTH_MAX],
    depth: usize,
}

impl Default for Stack {
    fn default() -> Stack {
        Stack::new()
    }
}

impl Stack {
    /// A stack of no holder: the kernel's log is in front.
    pub const fn new() -> Stack {
        Stack { holders: [0; DEPTH_MAX], depth: 0 }
    }

    /// The holder in front, or `None` while the kernel's log is.
    pub fn front(&self) -> Option<u32> {
        self.stacked().last().copied()
    }

    /// Whether `holder` is in the stack, in front or under another.
    pub fn contains(&self, holder: u32) -> bool {
        self.stacked().contains(&holder)
    }

    /// The holders that are stacked, the one in front last.
    pub fn stacked(&self) -> &[u32] {
        &self.holders[..self.depth.min(DEPTH_MAX)]
    }

    /// The kernel's log gives way to `holder`.
    ///
    /// # Errors
    ///
    /// [`Refused::NotFront`] while any holder is stacked: the front is
    /// that holder's to lend.
    pub fn leave_log(&mut self, holder: u32) -> Result<Passed, Refused> {
        if self.depth != 0 {
            return Err(Refused::NotFront);
        }
        self.push(holder);
        Ok(Passed { from: None, to: Some(holder) })
    }

    /// The holder in front, `from`, lends the front to `to`.
    ///
    /// # Errors
    ///
    /// [`Refused::NotFront`] if `from` is not in front,
    /// [`Refused::Stacked`] if `to` is in the stack already, and
    /// [`Refused::Full`] if the stack is as deep as it goes.
    pub fn lend(&mut self, from: u32, to: u32) -> Result<Passed, Refused> {
        if self.front() != Some(from) {
            return Err(Refused::NotFront);
        }
        if self.contains(to) {
            return Err(Refused::Stacked);
        }
        if self.depth >= DEPTH_MAX {
            return Err(Refused::Full);
        }
        self.push(to);
        Ok(Passed { from: Some(from), to: Some(to) })
    }

    /// The holder in front gives the front back to the one under it, or
    /// to the kernel's log.
    ///
    /// # Errors
    ///
    /// [`Refused::NotFront`] if `holder` is not in front.
    pub fn give_back(&mut self, holder: u32) -> Result<Passed, Refused> {
        if self.front() != Some(holder) {
            return Err(Refused::NotFront);
        }
        self.depth -= 1;
        Ok(Passed { from: Some(holder), to: self.front() })
    }

    /// `holder` ends. If it was in front, the front passes as
    /// [`Stack::give_back`] passes it and the answer says to whom;
    /// otherwise nothing that shows changes and the answer is `None`.
    pub fn end(&mut self, holder: u32) -> Option<Passed> {
        let at = self.stacked().iter().position(|&stacked| stacked == holder)?;
        let was_front = at + 1 == self.depth;
        self.holders.copy_within(at + 1..self.depth, at);
        self.depth -= 1;
        was_front.then(|| Passed { from: Some(holder), to: self.front() })
    }

    fn push(&mut self, holder: u32) {
        if let Some(slot) = self.holders.get_mut(self.depth) {
            *slot = holder;
            self.depth += 1;
        }
    }

    /// Whether the stack is what every call leaves it: no deeper than it
    /// goes, and no holder in it twice.
    #[cfg(any(test, kani))]
    fn is_sound(&self) -> bool {
        self.depth <= DEPTH_MAX
            && (0..self.depth).all(|i| (0..i).all(|j| self.holders[i] != self.holders[j]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONSOLE: u32 = 1;
    const DESKTOP: u32 = 2;
    const GAME: u32 = 3;

    fn passed(from: Option<u32>, to: Option<u32>) -> Result<Passed, Refused> {
        Ok(Passed { from, to })
    }

    #[test]
    fn the_log_gives_way_only_while_no_holder_is_stacked() {
        let mut stack = Stack::new();
        assert_eq!(stack.front(), None);
        assert_eq!(stack.leave_log(CONSOLE), passed(None, Some(CONSOLE)));
        assert_eq!(stack.front(), Some(CONSOLE));
        // A second holder cannot take the front, from the log or at all.
        assert_eq!(stack.leave_log(DESKTOP), Err(Refused::NotFront));
        assert_eq!(stack.leave_log(CONSOLE), Err(Refused::NotFront));
        assert_eq!(stack.stacked(), [CONSOLE]);
    }

    #[test]
    fn the_front_is_lent_and_given_back() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        assert_eq!(stack.lend(CONSOLE, DESKTOP), passed(Some(CONSOLE), Some(DESKTOP)));
        assert_eq!(stack.lend(DESKTOP, GAME), passed(Some(DESKTOP), Some(GAME)));
        assert_eq!(stack.stacked(), [CONSOLE, DESKTOP, GAME]);
        assert!(stack.contains(CONSOLE));
        assert_eq!(stack.give_back(GAME), passed(Some(GAME), Some(DESKTOP)));
        assert_eq!(stack.give_back(DESKTOP), passed(Some(DESKTOP), Some(CONSOLE)));
        assert_eq!(stack.give_back(CONSOLE), passed(Some(CONSOLE), None));
        assert_eq!(stack.front(), None);
        assert!(stack.is_sound());
    }

    #[test]
    fn only_the_front_lends_or_gives_back() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        stack.lend(CONSOLE, DESKTOP).unwrap();
        assert_eq!(stack.lend(CONSOLE, GAME), Err(Refused::NotFront));
        assert_eq!(stack.lend(GAME, CONSOLE), Err(Refused::NotFront));
        assert_eq!(stack.give_back(CONSOLE), Err(Refused::NotFront));
        assert_eq!(stack.give_back(GAME), Err(Refused::NotFront));
        // Nor does the front lend to one under it, or to itself.
        assert_eq!(stack.lend(DESKTOP, CONSOLE), Err(Refused::Stacked));
        assert_eq!(stack.lend(DESKTOP, DESKTOP), Err(Refused::Stacked));
        assert_eq!(stack.stacked(), [CONSOLE, DESKTOP]);
    }

    #[test]
    fn a_borrower_that_ends_passes_the_front_down() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        stack.lend(CONSOLE, DESKTOP).unwrap();
        assert_eq!(stack.end(DESKTOP), Some(Passed { from: Some(DESKTOP), to: Some(CONSOLE) }));
        assert_eq!(stack.end(CONSOLE), Some(Passed { from: Some(CONSOLE), to: None }));
        assert_eq!(stack.end(CONSOLE), None);
        assert_eq!(stack.front(), None);
    }

    #[test]
    fn a_lender_that_ends_takes_nothing_with_it() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        stack.lend(CONSOLE, DESKTOP).unwrap();
        stack.lend(DESKTOP, GAME).unwrap();
        // The desktop ends under the game: the game stays in front, and
        // gives back to the console.
        assert_eq!(stack.end(DESKTOP), None);
        assert_eq!(stack.front(), Some(GAME));
        assert_eq!(stack.give_back(GAME), passed(Some(GAME), Some(CONSOLE)));
        // With the console gone too, the log is next.
        stack.lend(CONSOLE, GAME).unwrap();
        assert_eq!(stack.end(CONSOLE), None);
        assert_eq!(stack.give_back(GAME), passed(Some(GAME), None));
        // And a new console finds the log ready to give way.
        assert_eq!(stack.leave_log(9), passed(None, Some(9)));
    }

    #[test]
    fn the_stack_is_no_deeper_than_it_goes() {
        let mut stack = Stack::new();
        stack.leave_log(1).unwrap();
        for holder in 2..=DEPTH_MAX as u32 {
            stack.lend(holder - 1, holder).unwrap();
        }
        let top = DEPTH_MAX as u32;
        assert_eq!(stack.lend(top, top + 1), Err(Refused::Full));
        assert_eq!(stack.front(), Some(top));
        assert!(stack.is_sound());
        // Ending one in the middle makes room.
        assert_eq!(stack.end(4), None);
        assert_eq!(stack.lend(top, top + 1), passed(Some(top), Some(top + 1)));
        assert!(stack.is_sound());
    }

    #[test]
    fn a_holder_that_was_never_stacked_ends_without_a_trace() {
        let mut stack = Stack::new();
        assert_eq!(stack.end(CONSOLE), None);
        stack.leave_log(CONSOLE).unwrap();
        assert_eq!(stack.end(DESKTOP), None);
        assert_eq!(stack.stacked(), [CONSOLE]);
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Any stack a call can leave behind.
    fn any_stack() -> Stack {
        let stack = Stack {
            holders: kani::any(),
            depth: kani::any_where(|&depth: &usize| depth <= DEPTH_MAX),
        };
        kani::assume(stack.is_sound());
        stack
    }

    /// Whatever is asked of a stack, in whatever state, it stays no
    /// deeper than it goes and holds no holder twice; and the front
    /// changes only as the answer says.
    #[kani::proof]
    #[kani::unwind(10)]
    fn every_call_leaves_a_sound_stack() {
        let mut stack = any_stack();
        let front = stack.front();
        let (a, b): (u32, u32) = (kani::any(), kani::any());
        let passed = match kani::any::<u8>() % 4 {
            0 => stack.leave_log(a).ok(),
            1 => stack.lend(a, b).ok(),
            2 => stack.give_back(a).ok(),
            _ => stack.end(a),
        };
        assert!(stack.is_sound());
        match passed {
            Some(passed) => {
                assert!(passed.from == front);
                assert!(passed.to == stack.front());
            }
            None => assert!(stack.front() == front),
        }
    }
}
