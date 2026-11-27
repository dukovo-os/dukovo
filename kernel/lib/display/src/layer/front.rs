// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who has the screens: the kernel's log, the holder in front, or for a
//! moment the last picture of a holder that ended (ADR 0702).
//!
//! The front passes as [`crate::stack`] says. This is what each pass
//! does to what shows:
//!
//! - A holder takes the front from the log with its first change, and
//!   the log's card is told to draw no more.
//! - A holder that is lent the front for the first time has asked for
//!   nothing yet: what the lender showed stays until it does.
//! - A holder that is given the front back, or lent it again, has the
//!   state it last asked for made again at once, every frame drawn
//!   whole. Where that state has the modes that show, no mode is set.
//! - A holder that ends in front leaves its last picture showing. Its
//!   lender is told that the borrower ended and that it is in front,
//!   and nothing is drawn for it: the picture stays until the lender
//!   asks for a change, which makes all of its state that differs from
//!   what shows. With no holder under the one that ended, the kernel's
//!   log waits [`LOG_WAIT_NS`] before it draws, so that whatever starts
//!   in the ended holder's place takes the screens from the picture with
//!   no log between. An urgent message of the log ends the wait
//!   ([`Layer::log_urgent`]), and so does whatever moves the log: its
//!   card taken away, a first card for it added, or a screen of its card
//!   that comes, goes or changes.
//! - The holder at the bottom that gives the front back leaves the
//!   screens to the log at once: it asked to.
//! - The secure key raises the holder of the attention file over them
//!   all (ADR 0708, [`Layer::secure_key`]). Its state is made as for a
//!   holder given the front back, and the one that was in front is told
//!   it is in the back. When the raised holder is done
//!   ([`Layer::attention_end`]) or ends, the top of the stack has the
//!   front back the same way.
//! - The holder of the attention file that stands under the stack (ADR
//!   0716, [`Layer::stand_under`]) changes nothing that shows and is
//!   told nothing: the front comes to it as to any lender.

use core::alloc::Allocator;
use core::ops::Deref;

use radix_abi::Status;
use radix_abi::display::{
    DisplayEvent, EVENT_ATTENTION, EVENT_BACK, EVENT_BORROWER_ENDED, EVENT_FRONT,
};
use radix_abi::errors::Kernel;

use super::stored::Damage;
use super::{Layer, Stored, busy, difference, frame, invalid};
use crate::frame::{Card, Change, Picture};
use crate::stack::{Passed, Refused};
use crate::state;

/// How long the kernel's log leaves the last picture of a holder that
/// ended with none under it. The service that restarts a text console
/// or a desktop does so well inside it.
pub const LOG_WAIT_NS: u64 = 3_000_000_000;

impl<R, C, A> Layer<R, C, A>
where
    R: Clone + Deref<Target: Picture + Sized>,
    C: Card<R::Target> + Clone,
    A: Allocator + Copy,
{
    /// Fills `into`, a blank state, with what shows while the kernel's
    /// log is in front: the log card's first plugged screen fed in the
    /// mode it was found in, with the planes of its pipe on it. Returns
    /// the pipe's place in the list, if there is such a screen.
    fn log_state(&self, into: &mut Stored<A>) -> Option<usize> {
        let screen =
            self.screens.iter().find(|screen| screen.card == self.log_card && screen.plugged)?;
        let pipe = match screen.fixed_pipe {
            0 => self.pipes.iter().find(|pipe| pipe.card == self.log_card)?.id,
            fixed => fixed,
        };
        let place = self.pipes.iter().position(|other| other.id == pipe)?;
        let state = into.state();
        if let Some(fed) = state.screens.iter_mut().find(|state| state.id == screen.id) {
            fed.pipe = pipe;
        }
        if let Some(run) = state.pipes.iter_mut().find(|state| state.id == pipe) {
            (run.active, run.mode) = (true, screen.native);
        }
        for plane in self.planes.iter().filter(|plane| plane.fixed_pipe == pipe) {
            if let Some(on) = state.planes.iter_mut().find(|state| state.id == plane.id) {
                on.pipe = pipe;
            }
        }
        Some(place)
    }

    /// The kernel's log comes to the front: its card shows it, every
    /// other pipe shows nothing, and what waited for a beat is dropped.
    pub(super) fn log_front(&mut self) {
        self.kept_since = None;
        let mut scratch = core::mem::replace(&mut self.scratch, Stored::new_in(self.alloc));
        if scratch.blank(&self.things()).is_err() {
            self.scratch = scratch;
            return;
        }
        let log_pipe = self.log_state(&mut scratch);
        for place in 0..self.pipes.len() {
            self.drop_pending(place);
            let pipe = self.pipes[place];
            let old = self.shown_frame(&pipe);
            let new = frame(&self.things(), &scratch.view(), &pipe, &|_, _| None);
            let Some(card) = self.card(pipe.card) else { continue };
            let change = Change { main: true, pointer: true, ..difference(&old, &new) };
            let mode_set = change.mode;
            if log_pipe == Some(place) {
                card.log_takes(&new);
            } else if old.active {
                // A card that does not answer says so in the log itself;
                // the log shows on its own card either way.
                let _ = card.show(&new, &change);
            }
            self.runs[place].mode_sets += u64::from(mode_set);
        }
        for picture in &mut self.shown_pictures {
            *picture = None;
        }
        core::mem::swap(&mut self.shown, &mut scratch);
        self.scratch = scratch;
        for place in 0..self.pipes.len() {
            self.keep_beat(place);
        }
    }

    /// The first holder takes the front: from the kernel's log, or from
    /// the last picture of a holder that ended.
    pub(super) fn leave_log(&mut self, holder: u32) -> Result<(), Status> {
        if self.closing {
            return Err(busy());
        }
        self.stack.leave_log(holder).map_err(|_| busy())?;
        self.log_gives_way();
        self.post(holder, DisplayEvent { kind: EVENT_FRONT, ..self.event() });
        Ok(())
    }

    /// The kernel's log leaves the screens to a holder. A log that still
    /// waited its turn never drew: only one that has the screens is told
    /// to give them.
    fn log_gives_way(&mut self) {
        let waited = self.kept_since.take().is_some();
        if let Some(card) = self.card(self.log_card).filter(|_| !waited) {
            card.log_gives();
        }
    }

    /// The secure key was pressed (ADR 0708): the holder of the
    /// attention file is raised over the stack, told it is in front if
    /// it was not, and told of the key, with a count of 1 if the front
    /// passed to it and so passes back when it is done. Returns that
    /// holder; `None` if no holder has the attention file, or the
    /// machine ends. A key pressed again while the holder is raised
    /// changes nothing: the holder reads it as keys.
    pub fn secure_key(&mut self) -> Option<u32> {
        if self.closing || self.attention == 0 {
            return None;
        }
        let id = self.attention;
        let Ok(passed) = self.stack.raise(id) else {
            return self.stack.raised();
        };
        if let Some(passed) = passed {
            if passed.from.is_none() {
                self.log_gives_way();
            }
            self.pass(passed, false);
        }
        let count = u64::from(passed.is_some());
        self.post(id, DisplayEvent { kind: EVENT_ATTENTION, count, ..self.event() });
        Some(id)
    }

    /// ATTENTION_END: holder `me`, which the secure key raised, is done.
    /// The top of the stack has the front again, or the kernel's log.
    ///
    /// # Errors
    ///
    /// busy for a holder the key did not raise.
    pub fn attention_end(&mut self, me: u32) -> Result<(), Status> {
        if let Some(passed) = self.stack.lower(me).map_err(|_| busy())? {
            self.pass(passed, false);
        }
        Ok(())
    }

    /// The holder the secure key raised, until it is done.
    pub fn raised(&self) -> Option<u32> {
        self.stack.raised()
    }

    /// The holder of the attention file, or 0.
    pub fn attention(&self) -> u32 {
        self.attention
    }

    /// Passes the front as `passed` says, and tells the one that had it
    /// and the one that has it now. `ended` says the one that had it is
    /// gone: its last picture then stays, for its lender or the kernel's
    /// log to draw over in their time. Otherwise the state of the one
    /// that has the front now is made again, or the log shows.
    pub(super) fn pass(&mut self, passed: Passed, ended: bool) {
        if let Some(from) = passed.from {
            for place in 0..self.pipes.len() {
                if self.runs[place].pending.as_ref().is_some_and(|pending| pending.holder == from) {
                    self.drop_pending(place);
                }
            }
            if !ended {
                self.post(from, DisplayEvent { kind: EVENT_BACK, ..self.event() });
            }
        }
        match (passed.to, ended) {
            (None, false) => self.log_front(),
            (None, true) => self.keep_last(),
            (Some(to), false) => {
                self.put_back(to);
                self.post(to, DisplayEvent { kind: EVENT_FRONT, ..self.event() });
            }
            (Some(to), true) => {
                self.post(to, DisplayEvent { kind: EVENT_BORROWER_ENDED, ..self.event() });
                self.post(to, DisplayEvent { kind: EVENT_FRONT, ..self.event() });
            }
        }
    }

    /// Makes again the state holder `id`, now in front, last asked for,
    /// if it asked for one and the cards can still show it: every frame
    /// of it is drawn whole, since a change of its own that waited when
    /// the front passed was dropped with the rectangles it named. A
    /// holder that has asked for nothing leaves what shows until it
    /// does, and one whose state names a screen that has gone was told
    /// of that with an EVENT_SCREEN and makes a new change.
    fn put_back(&mut self, id: u32) {
        let Some(at) = self.holder_place(id) else { return };
        let Some(wanted) = self.holders[at].wanted.as_ref() else { return };
        let holder = &self.holders[at];
        let checked =
            state::check(&self.things(), holder, &self.shown.view(), &wanted.view(), true);
        if checked.is_err() || self.card_check(holder, &wanted.view()).is_err() {
            return;
        }
        for place in 0..self.pipes.len() {
            self.make_pipe(place, at, &Damage::new(), true);
        }
    }

    /// The holder in front ended with none under it: what it showed
    /// stays, and the kernel's log waits its turn. Every card's beat is
    /// asked for by the end of the wait, and the first to come then
    /// brings the log to the front ([`Layer::beat`]).
    fn keep_last(&mut self) {
        self.kept_since = Some(self.now);
        let Some(due) = self.log_due() else { return };
        for more in &self.card_more {
            more.card.wake_for(due);
        }
    }

    /// When the kernel's log comes to the front, while it waits its
    /// turn behind the last picture of a holder that ended.
    pub(super) fn log_due(&self) -> Option<u64> {
        self.kept_since.map(|since| since.saturating_add(LOG_WAIT_NS))
    }

    /// Whether the kernel's log waits its turn behind the last picture
    /// of a holder that ended: no holder is in front, and the log does
    /// not draw yet.
    pub fn keeps_last(&self) -> bool {
        self.kept_since.is_some()
    }

    /// The kernel's log last had an urgent message at `at_ns`. If the
    /// log waits its turn and the message came in the wait, the log
    /// comes to the front now: what is urgent is not kept behind a
    /// picture nobody draws any more.
    pub fn log_urgent(&mut self, at_ns: u64) {
        if self.kept_since.is_some_and(|since| at_ns >= since) {
            self.log_front();
        }
    }

    /// Holder `id` has gone. If it was in front, the front passes on and
    /// its last picture stays. What it asked of a beat is dropped, and
    /// its pictures go with it but for those that still show.
    pub fn end(&mut self, id: u32) {
        if self.attention == id {
            self.attention = 0;
        }
        for run in &mut self.runs {
            run.askers.retain(|&(holder, _)| holder != id);
        }
        // A raised holder that ends over another leaves the front as one
        // that is done does. The top of the stack that ends behind a
        // raised holder passes no front, and its lender is told now: it
        // may be the raised holder itself, which is given no front later.
        let (raised, top) = (self.stack.raised(), self.stack.top());
        let over_others = raised == Some(id) && top.is_some_and(|top| top != id);
        let behind = raised.is_some_and(|raised| raised != id) && top == Some(id);
        match self.stack.end(id) {
            Some(passed) => self.pass(passed, !over_others),
            None => {
                for place in 0..self.pipes.len() {
                    if self.runs[place].pending.as_ref().is_some_and(|pending| pending.holder == id)
                    {
                        self.drop_pending(place);
                    }
                }
                if let Some(lender) = self.stack.top().filter(|_| behind) {
                    self.post(lender, DisplayEvent { kind: EVENT_BORROWER_ENDED, ..self.event() });
                }
            }
        }
        // Its pictures go with it, but for those a plane still shows.
        if let Some(at) = self.holder_place(id) {
            drop(self.holders.swap_remove(at));
        }
    }

    /// LEND: holder `from`, which is in front, lends the front to `to`.
    ///
    /// # Errors
    ///
    /// busy for a holder that is not in front, invalid_argument for a
    /// borrower that is in the stack already, and too_many for a stack
    /// as deep as it goes.
    pub fn lend(&mut self, from: u32, to: u32) -> Result<(), Status> {
        let passed = self.stack.lend(from, to).map_err(|refused| match refused {
            Refused::NotFront | Refused::Empty => busy(),
            Refused::Stacked => invalid(),
            Refused::Full => Kernel::TooMany.status(0),
        })?;
        self.pass(passed, false);
        Ok(())
    }

    /// STAND_UNDER: holder `me`, which has the attention file, takes the
    /// bottom of the stack while others are stacked (ADR 0716): the
    /// place of the lender it started in place of. Nothing that shows
    /// changes, and the front comes to it when the holders over it give
    /// it back or end.
    ///
    /// # Errors
    ///
    /// not_permitted for a holder that has not the attention file, busy
    /// for one that is stacked already or while no holder is, and
    /// too_many for a stack as deep as it goes.
    pub fn stand_under(&mut self, me: u32) -> Result<(), Status> {
        if self.attention != me {
            return Err(Kernel::NotPermitted.status(0));
        }
        self.stack.stand_under(me).map_err(|refused| match refused {
            Refused::Full => Kernel::TooMany.status(0),
            Refused::NotFront | Refused::Stacked | Refused::Empty => busy(),
        })
    }

    /// RETURN: holder `me`, which is in front, gives the front back to
    /// the holder under it, or to the kernel's log.
    ///
    /// # Errors
    ///
    /// busy for a holder that is not in front.
    pub fn give_back(&mut self, me: u32) -> Result<(), Status> {
        let passed = self.stack.give_back(me).map_err(|_| busy())?;
        self.pass(passed, false);
        Ok(())
    }

    /// The holder in front, or `None` while the kernel's log is, or
    /// waits its turn.
    pub fn front(&self) -> Option<u32> {
        self.stack.front()
    }

    /// The machine ends: the screens are the log's, and stay so.
    pub fn close(&mut self) {
        self.closing = true;
        while let Some(front) = self.stack.front() {
            let _ = self.stack.end(front);
            self.post(front, DisplayEvent { kind: EVENT_BACK, ..self.event() });
        }
        self.log_front();
    }
}
