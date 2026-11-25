// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What comes and goes while the layer runs: a monitor plugged in or
//! taken out, a screen whose modes change, and a card taken away.
//!
//! A card looks at its screens when its device says they changed, and
//! tells the layer what it found of each ([`Layer::screen_found`]).
//! Every holder is told with an EVENT_SCREEN and asks again what there
//! is. The layer alters no state for it: a holder whose state feeds a
//! screen that has gone makes a new change, and until then the state
//! that shows is the one it asked for.

use alloc::vec::Vec;
use core::alloc::Allocator;
use core::ops::Deref;

use radix_abi::display::{DisplayEvent, EVENT_SCREEN, Mode};

use super::{Layer, Stored};
use crate::frame::{Card, Picture};
use crate::monitor::Monitor;

const NO_MEMORY: &str = "out of memory";

/// What a card finds of one of its screens when it looks.
#[derive(Clone, Copy, Debug)]
pub struct Found<'a> {
    /// Whether a monitor is plugged in.
    pub plugged: bool,
    /// The modes the screen takes now, the preferred one first. With
    /// none, it keeps the modes it had.
    pub modes: &'a [Mode],
    /// The bytes the monitor says what it is with, or none.
    pub data: &'a [u8],
}

/// A copy of `from` in a list from `alloc`, or `None` without the
/// memory for it.
fn copy_in<T: Copy, A: Allocator>(from: &[T], alloc: A) -> Option<Vec<T, A>> {
    let mut list = Vec::new_in(alloc);
    list.try_reserve_exact(from.len()).ok()?;
    for &item in from {
        // Room was reserved.
        let _ = list.push_within_capacity(item);
    }
    Some(list)
}

impl<R, C, A> Layer<R, C, A>
where
    R: Clone + Deref<Target: Picture + Sized>,
    C: Card<R::Target> + Clone,
    A: Allocator + Copy,
{
    /// Takes in what card `card` found of its screen at place `index`.
    /// If that differs from what the layer knew, the screen is no
    /// longer marked failed, every holder is told with an EVENT_SCREEN,
    /// and the kernel's log, if it is in front on this card, is laid out
    /// again: on the card's first screen with a monitor, in the mode
    /// that screen prefers. Returns whether it differed.
    ///
    /// # Errors
    ///
    /// Why nothing was taken in, for the log.
    pub fn screen_found(
        &mut self,
        card: u32,
        index: usize,
        found: &Found<'_>,
    ) -> Result<bool, &'static str> {
        let mut of_card = (0..self.screens.len()).filter(|&at| self.screens[at].card == card);
        let at = of_card.nth(index).ok_or("no such screen")?;
        let (screen, more) = (&self.screens[at], &self.screen_more[at]);
        let modes = Some(found.modes).filter(|modes| !modes.is_empty());
        let same = screen.plugged == found.plugged
            && modes.is_none_or(|modes| *modes == more.modes[..])
            && *found.data == more.data[..];
        if same {
            return Ok(false);
        }
        // The new lists first: the screen changes whole or not at all.
        let data = copy_in(found.data, self.alloc).ok_or(NO_MEMORY)?;
        let modes = match modes {
            Some(modes) => Some(copy_in(modes, self.alloc).ok_or(NO_MEMORY)?),
            None => None,
        };
        let (screen, more) = (&mut self.screens[at], &mut self.screen_more[at]);
        (screen.plugged, screen.failed) = (found.plugged, false);
        if let Some(modes) = modes {
            screen.native = modes[0];
            more.modes = modes;
        }
        // Data that says nothing a monitor would is kept, and read as
        // saying nothing.
        more.monitor = Monitor::read(found.data).unwrap_or_default();
        more.data = data;
        let thing = screen.id;
        self.post_all(DisplayEvent { kind: EVENT_SCREEN, thing, ..self.event() });
        if self.stack.front().is_none() && self.log_card == card {
            self.log_front();
        }
        Ok(true)
    }

    /// Every state the layer keeps of the things: the one that shows,
    /// and each holder's.
    fn states(&mut self) -> impl Iterator<Item = &mut Stored<A>> {
        let wanted = self.holders.iter_mut().filter_map(|holder| holder.wanted.as_mut());
        core::iter::once(&mut self.shown).chain(wanted)
    }

    /// Takes card `id` away, with its screens, pipes and planes and all
    /// that every state says of them, and returns it. Changes that
    /// waited for its beats are dropped, the pictures its planes showed
    /// are let go of, and every holder is told with an EVENT_SCREEN. If
    /// it showed the kernel's log, the next card that can does. The card
    /// is asked nothing more, not even to give the log up: what its
    /// screens show from here on is its driver's to see to.
    ///
    /// A holder's pictures of the card stay the holder's, to drop: no
    /// plane is left that takes them.
    pub fn remove_card(&mut self, id: u32) -> Option<C> {
        let at = self.cards.iter().position(|card| card.id == id)?;
        // From the last of each list, so that a place still to be looked
        // at is not moved by what is taken out.
        for place in (0..self.pipes.len()).rev() {
            if self.pipes[place].card == id {
                self.drop_pending(place);
                self.pipes.remove(place);
                self.runs.remove(place);
                for state in self.states() {
                    state.pipes.remove(place);
                }
            }
        }
        for place in (0..self.screens.len()).rev() {
            if self.screens[place].card == id {
                self.screens.remove(place);
                self.screen_more.remove(place);
                for state in self.states() {
                    state.screens.remove(place);
                }
            }
        }
        for place in (0..self.planes.len()).rev() {
            if self.planes[place].card == id {
                self.planes.remove(place);
                self.shown_pictures.remove(place);
                for state in self.states() {
                    state.planes.remove(place);
                }
            }
        }
        self.cards.remove(at);
        let card = self.card_more.remove(at).card;
        if self.log_card == id {
            self.pass_log();
        }
        self.post_all(DisplayEvent { kind: EVENT_SCREEN, ..self.event() });
        Some(card)
    }

    /// The card that showed the kernel's log was taken away: the first
    /// card left that can show it does, as soon as no holder is in
    /// front.
    fn pass_log(&mut self) {
        let next = self.card_more.iter().position(|more| more.card.shows_log());
        self.log_card = next.map_or(0, |at| self.cards[at].id);
        match next {
            Some(_) if self.stack.front().is_none() => self.log_front(),
            // A holder has the screens: the log waits its turn.
            Some(at) => self.card_more[at].card.log_gives(),
            None => {}
        }
    }
}
