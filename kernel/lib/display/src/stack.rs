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
//! One thing is over the stack: the secure key (ADR 0708). When it is
//! pressed the kernel raises the holder of the attention file
//! ([`Stack::raise`]), which is then in front whatever is stacked, and
//! whether it is stacked itself or not. Nothing below it moves: while a
//! holder is raised, nobody lends the front or gives it back. When the
//! raised holder says it is done ([`Stack::lower`]), or ends, the front
//! is again the top of the stack's. No program raises a holder; only
//! the key does.
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

/// The stack of holders, the one in front last, and the holder raised
/// over it.
#[derive(Clone, Copy, Debug)]
pub struct Stack {
    holders: [u32; DEPTH_MAX],
    depth: usize,
    /// The holder the secure key raised, or 0.
    raised: u32,
}

impl Default for Stack {
    fn default() -> Stack {
        Stack::new()
    }
}

impl Stack {
    /// A stack of no holder: the kernel's log is in front.
    pub const fn new() -> Stack {
        Stack { holders: [0; DEPTH_MAX], depth: 0, raised: 0 }
    }

    /// The holder in front, or `None` while the kernel's log is: the
    /// raised holder if there is one, else the top of the stack.
    pub fn front(&self) -> Option<u32> {
        self.raised().or(self.top())
    }

    /// The holder on top of the stack, which is in front unless another
    /// is raised.
    pub fn top(&self) -> Option<u32> {
        self.stacked().last().copied()
    }

    /// The holder the secure key raised, if one is.
    pub fn raised(&self) -> Option<u32> {
        (self.raised != 0).then_some(self.raised)
    }

    /// The secure key raises `holder` over the stack. The answer says
    /// how the front passed; `None` if `holder` was in front already, as
    /// the top of the stack, and is now also raised.
    ///
    /// # Errors
    ///
    /// [`Refused::Stacked`] while a holder is raised already, and for 0,
    /// which is no holder.
    pub fn raise(&mut self, holder: u32) -> Result<Option<Passed>, Refused> {
        if self.raised != 0 || holder == 0 {
            return Err(Refused::Stacked);
        }
        let top = self.top();
        self.raised = holder;
        Ok((top != Some(holder)).then_some(Passed { from: top, to: Some(holder) }))
    }

    /// The raised holder, `holder`, is done: the front is the top of the
    /// stack's again. The answer says how the front passed; `None` if
    /// `holder` is the top of the stack and so stays in front.
    ///
    /// # Errors
    ///
    /// [`Refused::NotFront`] if `holder` is not raised.
    pub fn lower(&mut self, holder: u32) -> Result<Option<Passed>, Refused> {
        if self.raised() != Some(holder) {
            return Err(Refused::NotFront);
        }
        self.raised = 0;
        let top = self.top();
        Ok((top != Some(holder)).then_some(Passed { from: Some(holder), to: top }))
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
    /// [`Refused::NotFront`] while any holder is stacked, as the front
    /// is that holder's to lend, or while one is raised.
    pub fn leave_log(&mut self, holder: u32) -> Result<Passed, Refused> {
        if self.depth != 0 || self.raised != 0 {
            return Err(Refused::NotFront);
        }
        self.push(holder);
        Ok(Passed { from: None, to: Some(holder) })
    }

    /// The holder in front, `from`, lends the front to `to`.
    ///
    /// # Errors
    ///
    /// [`Refused::NotFront`] if `from` is not in front or a holder is
    /// raised, [`Refused::Stacked`] if `to` is in the stack already, and
    /// [`Refused::Full`] if the stack is as deep as it goes.
    pub fn lend(&mut self, from: u32, to: u32) -> Result<Passed, Refused> {
        if self.raised != 0 || self.front() != Some(from) {
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
    /// [`Refused::NotFront`] if `holder` is not in front, or a holder is
    /// raised.
    pub fn give_back(&mut self, holder: u32) -> Result<Passed, Refused> {
        if self.raised != 0 || self.front() != Some(holder) {
            return Err(Refused::NotFront);
        }
        self.depth -= 1;
        Ok(Passed { from: Some(holder), to: self.front() })
    }

    /// `holder` ends. If it was in front, the front passes as
    /// [`Stack::give_back`] passes it, or from a raised holder to the
    /// top of the stack, and the answer says to whom; otherwise nothing
    /// that shows changes and the answer is `None`.
    pub fn end(&mut self, holder: u32) -> Option<Passed> {
        let was_front = self.front() == Some(holder);
        if self.raised == holder {
            self.raised = 0;
        }
        if let Some(at) = self.stacked().iter().position(|&stacked| stacked == holder) {
            self.holders.copy_within(at + 1..self.depth, at);
            self.depth -= 1;
        }
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
    fn the_secure_key_raises_a_holder_over_the_stack_and_nothing_below_moves() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        stack.lend(CONSOLE, DESKTOP).unwrap();
        assert_eq!(
            stack.raise(CONSOLE),
            Ok(Some(Passed { from: Some(DESKTOP), to: Some(CONSOLE) }))
        );
        assert_eq!(
            (stack.front(), stack.top(), stack.raised()),
            (Some(CONSOLE), Some(DESKTOP), Some(CONSOLE))
        );
        // Nobody lends or gives back meanwhile, the raised holder
        // included, and no second holder is raised.
        assert_eq!(stack.lend(CONSOLE, GAME), Err(Refused::NotFront));
        assert_eq!(stack.lend(DESKTOP, GAME), Err(Refused::NotFront));
        assert_eq!(stack.give_back(DESKTOP), Err(Refused::NotFront));
        assert_eq!(stack.give_back(CONSOLE), Err(Refused::NotFront));
        assert_eq!(stack.raise(GAME), Err(Refused::Stacked));
        assert_eq!(stack.lower(DESKTOP), Err(Refused::NotFront));
        assert_eq!(stack.stacked(), [CONSOLE, DESKTOP]);
        assert_eq!(
            stack.lower(CONSOLE),
            Ok(Some(Passed { from: Some(CONSOLE), to: Some(DESKTOP) }))
        );
        assert_eq!(stack.front(), Some(DESKTOP));
        assert_eq!(stack.lower(CONSOLE), Err(Refused::NotFront));
        assert!(stack.is_sound());
    }

    #[test]
    fn a_holder_raised_while_it_is_on_top_stays_in_front_throughout() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        assert_eq!(stack.raise(CONSOLE), Ok(None));
        assert_eq!(stack.give_back(CONSOLE), Err(Refused::NotFront));
        assert_eq!(stack.lower(CONSOLE), Ok(None));
        assert_eq!(stack.front(), Some(CONSOLE));
        assert_eq!(stack.raise(0), Err(Refused::Stacked));
    }

    #[test]
    fn a_holder_that_is_not_stacked_is_raised_over_the_log_or_over_others() {
        let mut stack = Stack::new();
        assert_eq!(stack.raise(GAME), Ok(Some(Passed { from: None, to: Some(GAME) })));
        assert_eq!(stack.leave_log(CONSOLE), Err(Refused::NotFront));
        assert_eq!(stack.lower(GAME), Ok(Some(Passed { from: Some(GAME), to: None })));
        stack.leave_log(CONSOLE).unwrap();
        stack.raise(GAME).unwrap();
        assert_eq!(stack.stacked(), [CONSOLE]);
        assert_eq!(stack.front(), Some(GAME));
    }

    #[test]
    fn holders_that_end_while_one_is_raised() {
        let mut stack = Stack::new();
        stack.leave_log(CONSOLE).unwrap();
        stack.lend(CONSOLE, DESKTOP).unwrap();
        stack.raise(CONSOLE).unwrap();
        // The top of the stack ends behind the raised holder: the front
        // does not pass.
        assert_eq!(stack.end(DESKTOP), None);
        assert_eq!((stack.front(), stack.top()), (Some(CONSOLE), Some(CONSOLE)));
        assert_eq!(stack.lower(CONSOLE), Ok(None));
        // The raised holder ends: the front is the top's again, and the
        // raised holder is out of the stack too.
        stack.lend(CONSOLE, DESKTOP).unwrap();
        stack.raise(CONSOLE).unwrap();
        assert_eq!(stack.end(CONSOLE), Some(Passed { from: Some(CONSOLE), to: Some(DESKTOP) }));
        assert_eq!((stack.raised(), stack.stacked()), (None, &[DESKTOP][..]));
        // Raised and on top, it ends as any front does.
        stack.raise(DESKTOP).unwrap();
        assert_eq!(stack.end(DESKTOP), Some(Passed { from: Some(DESKTOP), to: None }));
        assert_eq!(stack.front(), None);
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
            raised: kani::any(),
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
        let passed = match kani::any::<u8>() % 6 {
            0 => stack.leave_log(a).ok(),
            1 => stack.lend(a, b).ok(),
            2 => stack.give_back(a).ok(),
            3 => stack.raise(a).ok().flatten(),
            4 => stack.lower(a).ok().flatten(),
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
