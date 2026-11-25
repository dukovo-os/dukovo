// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the layer keeps of a state and of a change: the state of every
//! thing in lists of the layer's own, and the rectangles a change says
//! changed.

use alloc::vec::Vec;
use core::alloc::Allocator;

use radix_abi::Status;

use super::no_memory;
use crate::Rect;
use crate::state::{PipeState, PlaneState, ScreenState, State, Things, View};

/// The most rectangles of changed pixels a change keeps apart. Past
/// them, a plane's are joined into one.
pub(super) const DAMAGE_MAX: usize = 16;

/// A state the layer keeps: an element for each thing, in the order of
/// the layer's lists.
pub(crate) struct Stored<A: Allocator> {
    pub(super) screens: Vec<ScreenState, A>,
    pub(super) pipes: Vec<PipeState, A>,
    pub(super) planes: Vec<PlaneState, A>,
}

impl<A: Allocator + Copy> Stored<A> {
    /// A state of no thing.
    pub(super) const fn new_in(alloc: A) -> Stored<A> {
        Stored {
            screens: Vec::new_in(alloc),
            pipes: Vec::new_in(alloc),
            planes: Vec::new_in(alloc),
        }
    }

    /// The state, to read.
    pub(crate) fn view(&self) -> View<'_> {
        View { screens: &self.screens, pipes: &self.pipes, planes: &self.planes }
    }

    /// The state, to alter.
    pub(super) fn state(&mut self) -> State<'_> {
        State { screens: &mut self.screens, pipes: &mut self.pipes, planes: &mut self.planes }
    }

    /// Makes room for the states of so many more things. Returns whether
    /// there was memory for them.
    pub(super) fn reserve(&mut self, screens: usize, pipes: usize, planes: usize) -> bool {
        self.screens.try_reserve(screens).is_ok()
            && self.pipes.try_reserve(pipes).is_ok()
            && self.planes.try_reserve(planes).is_ok()
    }

    /// Adds a state in which nothing shows for each of `things` it has
    /// none for: those of a card added since it was made, which are the
    /// last of each list. Room for them was reserved.
    pub(super) fn grow(&mut self, things: &Things<'_>) {
        for screen in things.screens.iter().skip(self.screens.len()) {
            let _ = self.screens.push_within_capacity(ScreenState::new(screen.id));
        }
        for pipe in things.pipes.iter().skip(self.pipes.len()) {
            let _ = self.pipes.push_within_capacity(PipeState::new(pipe.id));
        }
        for plane in things.planes.iter().skip(self.planes.len()) {
            let _ = self.planes.push_within_capacity(PlaneState::new(plane.id));
        }
    }

    /// Makes it the state of `things` in which nothing shows.
    pub(super) fn blank(&mut self, things: &Things<'_>) -> Result<(), Status> {
        fn fill<T, S, A: Allocator>(
            out: &mut Vec<S, A>,
            things: &[T],
            new: impl Fn(&T) -> S,
        ) -> Result<(), Status> {
            out.clear();
            out.try_reserve(things.len()).map_err(|_| no_memory())?;
            for thing in things {
                // Room was reserved.
                let _ = out.push_within_capacity(new(thing));
            }
            Ok(())
        }
        fill(&mut self.screens, things.screens, |screen| ScreenState::new(screen.id))?;
        fill(&mut self.pipes, things.pipes, |pipe| PipeState::new(pipe.id))?;
        fill(&mut self.planes, things.planes, |plane| PlaneState::new(plane.id))
    }

    /// Makes it a copy of `other`.
    pub(super) fn assign(&mut self, other: &Stored<A>) -> Result<(), Status> {
        fn copy<S: Copy, A: Allocator>(out: &mut Vec<S, A>, from: &[S]) -> Result<(), Status> {
            out.clear();
            out.try_reserve(from.len()).map_err(|_| no_memory())?;
            for &state in from {
                // Room was reserved.
                let _ = out.push_within_capacity(state);
            }
            Ok(())
        }
        copy(&mut self.screens, &other.screens)?;
        copy(&mut self.pipes, &other.pipes)?;
        copy(&mut self.planes, &other.planes)
    }
}

/// The rectangles of planes' pictures that a change says changed.
#[derive(Clone, Copy)]
pub(super) struct Damage {
    /// Each a plane's number and a rectangle of its picture.
    rects: [(u32, Rect); DAMAGE_MAX],
    len: usize,
    /// Set when there were more planes than rectangles to keep them in:
    /// every picture then counts as changed all over.
    everything: bool,
}

impl Damage {
    /// No rectangle.
    pub(super) const fn new() -> Damage {
        let none = Rect { x: 0, y: 0, width: 0, height: 0 };
        Damage { rects: [(0, none); DAMAGE_MAX], len: 0, everything: false }
    }

    /// Adds a rectangle of `plane`'s picture.
    pub(super) fn add(&mut self, plane: u32, rect: Rect) {
        if let Some(slot) = self.rects.get_mut(self.len) {
            *slot = (plane, rect);
            self.len += 1;
            return;
        }
        match self.rects.iter_mut().find(|(other, _)| *other == plane) {
            Some((_, joined)) => *joined = joined.union(rect),
            None => self.everything = true,
        }
    }

    /// Adds every rectangle of `other`.
    pub(super) fn join(&mut self, other: &Damage) {
        self.everything |= other.everything;
        for &(plane, rect) in &other.rects[..other.len] {
            self.add(plane, rect);
        }
    }

    /// Whether every picture counts as changed all over.
    pub(super) fn everything(&self) -> bool {
        self.everything
    }

    /// Whether it names a rectangle of `plane`'s picture.
    pub(super) fn names(&self, plane: u32) -> bool {
        self.of(plane).next().is_some()
    }

    /// The rectangles of `plane`'s picture.
    pub(super) fn of(&self, plane: u32) -> impl Iterator<Item = Rect> + '_ {
        self.rects[..self.len].iter().filter(move |(of, _)| *of == plane).map(|&(_, rect)| rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_past_the_most_are_joined_by_plane() {
        let mut damage = Damage::new();
        for index in 0..DAMAGE_MAX as i32 {
            damage.add(1 + (index as u32 % 2), Rect { x: index, y: 0, width: 1, height: 1 });
        }
        assert_eq!(damage.of(1).count(), DAMAGE_MAX / 2);
        assert!(damage.names(2) && !damage.names(3) && !damage.everything());
        // One more of a plane that has some is joined into its first.
        damage.add(2, Rect { x: 100, y: 5, width: 1, height: 1 });
        assert_eq!(damage.of(2).count(), DAMAGE_MAX / 2);
        assert_eq!(damage.of(2).next(), Some(Rect { x: 1, y: 0, width: 100, height: 6 }));
        assert!(!damage.everything());
        // One of a plane there is no room for says everything changed.
        damage.add(3, Rect::sized(1, 1));
        assert!(damage.everything());
    }

    #[test]
    fn joined_damage_has_the_rectangles_of_both() {
        let (mut first, mut second) = (Damage::new(), Damage::new());
        first.add(1, Rect::sized(2, 2));
        second.add(1, Rect::sized(3, 3));
        second.add(2, Rect::sized(4, 4));
        first.join(&second);
        let of_one: Vec<Rect> = first.of(1).collect();
        assert_eq!(of_one, [Rect::sized(2, 2), Rect::sized(3, 3)]);
        assert_eq!(first.of(2).next(), Some(Rect::sized(4, 4)));
    }
}
