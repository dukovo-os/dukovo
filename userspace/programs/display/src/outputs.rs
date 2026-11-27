// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which screens a program shows on when it shows on them all, and the
//! change that puts a picture on each: the consoles' choice
//! (consoled(8)) and the desktop's (windowd(1)).
//!
//! Such a program shows on every screen a monitor is plugged into
//! (radix-display(2)), each fed by a pipe of its own in the mode the
//! screen prefers, with a picture of that size on the pipe's main plane.
//! A change names every screen, pipe and main plane the layer has, so
//! that it asks for the same state whatever the one before it was.

use std::io;

use crate::abi::{
    FORMAT_XRGB8888, KIND_CARD, KIND_PIPE, KIND_PLANE, KIND_SCREEN, MAY_BLANK, Mode, PIPE_ACTIVE,
    PIPE_MODE, PLANE_MAIN, PLANE_PICTURE, PLANE_PIPE, PlaneInfo, SCREEN_FAILED, SCREEN_PIPE,
    SCREEN_PLUGGED, ScreenInfo, Thing,
};
use crate::wire::ChangeBytes;
use crate::{Control, Holder};

/// The narrowest and the shortest screen that is shown on: 16 columns
/// and 4 rows of the consoles' smallest font.
pub const SMALLEST: (u32, u32) = (128, 64);

/// What the layer has, as far as such a program looks.
#[derive(Clone, Debug, Default)]
pub struct Found {
    /// Every card, screen, pipe and plane.
    pub things: Vec<Thing>,
    /// Each screen, with the mode it prefers if a monitor is plugged in.
    pub screens: Vec<(ScreenInfo, Option<Mode>)>,
    /// Each plane.
    pub planes: Vec<PlaneInfo>,
}

impl Found {
    /// Asks the layer what it has.
    pub fn read<C: Control>(holder: &Holder<C>) -> io::Result<Found> {
        let things = holder.things()?;
        let (mut screens, mut planes) = (Vec::new(), Vec::new());
        for thing in &things {
            match thing.kind {
                KIND_SCREEN => {
                    let info = holder.screen(thing.id)?;
                    let plugged = info.flags & SCREEN_PLUGGED != 0;
                    let mode = if plugged { holder.modes(&info)?.first().copied() } else { None };
                    screens.push((info, mode));
                }
                KIND_PLANE => planes.push(holder.plane(thing.id)?),
                _ => {}
            }
        }
        Ok(Found { things, screens, planes })
    }

    /// Whether there is a graphics card at all.
    pub fn has_card(&self) -> bool {
        self.things.iter().any(|thing| thing.kind == KIND_CARD)
    }

    fn of_kind(&self, kind: u32) -> impl Iterator<Item = &Thing> {
        self.things.iter().filter(move |thing| thing.kind == kind)
    }
}

/// One screen to show on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    /// The screen.
    pub screen: u32,
    /// Its card.
    pub card: u32,
    /// The pipe that feeds it.
    pub pipe: u32,
    /// The pipe's main plane.
    pub plane: u32,
    /// The mode the screen prefers.
    pub mode: Mode,
}

/// The screens to show on, in the layer's order: each screen with a
/// monitor plugged in and a mode large enough, that a pipe of its card
/// and a main plane for that pipe are left for. A screen keeps the pipe
/// that feeds it now, so that nothing blanks where nothing changed, and
/// otherwise takes the pipe at its own place on the card.
pub fn outputs(found: &Found) -> Vec<Output> {
    let mut outputs: Vec<Output> = Vec::new();
    for thing in found.of_kind(KIND_SCREEN) {
        let Some((info, Some(mode))) = found.screens.iter().find(|(info, _)| info.id == thing.id)
        else {
            continue;
        };
        if info.flags & SCREEN_FAILED != 0 || mode.width < SMALLEST.0 || mode.height < SMALLEST.1 {
            continue;
        }
        let free = |pipe: &&Thing| {
            pipe.card == thing.card && outputs.iter().all(|output| output.pipe != pipe.id)
        };
        let pipe = found
            .of_kind(KIND_PIPE)
            .find(|pipe| free(pipe) && pipe.id == info.pipe)
            .or_else(|| {
                found.of_kind(KIND_PIPE).find(|pipe| free(pipe) && pipe.index == thing.index)
            })
            .or_else(|| found.of_kind(KIND_PIPE).find(free));
        let Some(pipe) = pipe else { continue };
        let usable = |plane: &&PlaneInfo| {
            plane.card == thing.card
                && plane.kind == PLANE_MAIN
                && plane.formats.contains(&FORMAT_XRGB8888)
                && mode.width <= plane.width_max
                && mode.height <= plane.height_max
                && outputs.iter().all(|output| output.plane != plane.id)
        };
        let plane =
            found.planes.iter().find(|plane| usable(plane) && plane.fixed_pipe == pipe.id).or_else(
                || found.planes.iter().find(|plane| usable(plane) && plane.fixed_pipe == 0),
            );
        let Some(plane) = plane else { continue };
        outputs.push(Output {
            screen: thing.id,
            card: thing.card,
            pipe: pipe.id,
            plane: plane.id,
            mode: *mode,
        });
    }
    outputs
}

/// The change that shows on each of `outputs` the picture and in the
/// mode `made` has for it, as (picture, mode value), and nothing on any
/// other screen.
pub fn change(found: &Found, outputs: &[Output], made: &[(u32, u32)]) -> ChangeBytes {
    // A new mode may blank a screen for a moment.
    let mut change = ChangeBytes::new(MAY_BLANK, 0);
    let made = |at: usize| made.get(at).copied().unwrap_or((0, 0));
    for thing in found.of_kind(KIND_SCREEN) {
        let fed = outputs.iter().find(|output| output.screen == thing.id);
        change.set(thing.id, SCREEN_PIPE, fed.map_or(0, |output| u64::from(output.pipe)));
    }
    for thing in found.of_kind(KIND_PIPE) {
        match outputs.iter().position(|output| output.pipe == thing.id) {
            Some(at) => {
                change.set(thing.id, PIPE_MODE, u64::from(made(at).1));
                change.set(thing.id, PIPE_ACTIVE, 1);
            }
            None => change.set(thing.id, PIPE_ACTIVE, 0),
        }
    }
    for plane in found.planes.iter().filter(|plane| plane.kind == PLANE_MAIN) {
        match outputs.iter().position(|output| output.plane == plane.id) {
            Some(at) => {
                change.set(plane.id, PLANE_PIPE, u64::from(outputs[at].pipe));
                change.set(plane.id, PLANE_PICTURE, u64::from(made(at).0));
            }
            None => change.set(plane.id, PLANE_PICTURE, 0),
        }
    }
    change
}

#[cfg(test)]
mod tests {
    use crate::abi::{Change, Entry, FORMAT_ARGB8888, PLANE_POINTER};

    use super::*;

    fn mode(width: u32, height: u32) -> Mode {
        Mode { width, height, refresh_mhz: 60_000, ..Mode::default() }
    }

    fn thing(id: u32, kind: u32, card: u32, index: u32) -> Thing {
        Thing { id, kind, card, index }
    }

    fn screen(id: u32, card: u32, plugged: bool, pipe: u32) -> ScreenInfo {
        let flags = if plugged { SCREEN_PLUGGED } else { 0 };
        ScreenInfo { id, card, flags, pipe, modes: u32::from(plugged), ..ScreenInfo::default() }
    }

    fn plane(id: u32, card: u32, kind: u32, fixed_pipe: u32) -> PlaneInfo {
        let format = if kind == PLANE_MAIN { FORMAT_XRGB8888 } else { FORMAT_ARGB8888 };
        PlaneInfo {
            id,
            card,
            kind,
            fixed_pipe,
            width_max: 16384,
            height_max: 16384,
            formats: [format, 0, 0, 0, 0, 0, 0, 0],
            ..PlaneInfo::default()
        }
    }

    /// The firmware's card, as the kernel numbers it: one screen that
    /// its one pipe feeds while the log shows.
    fn firmware() -> Found {
        Found {
            things: vec![
                thing(1, KIND_CARD, 1, 0),
                thing(3, KIND_SCREEN, 1, 0),
                thing(2, KIND_PIPE, 1, 0),
                thing(4, KIND_PLANE, 1, 0),
                thing(5, KIND_PLANE, 1, 1),
            ],
            screens: vec![(screen(3, 1, true, 2), Some(mode(1280, 800)))],
            planes: vec![plane(4, 1, PLANE_MAIN, 2), plane(5, 1, PLANE_POINTER, 2)],
        }
    }

    /// The firmware's card and a virtio card of three screens, the
    /// second with no monitor, none fed yet.
    fn two_cards() -> Found {
        let mut found = firmware();
        found.things.push(thing(6, KIND_CARD, 6, 0));
        for index in 0..3 {
            found.things.push(thing(10 + index, KIND_SCREEN, 6, index));
            found.things.push(thing(20 + index, KIND_PIPE, 6, index));
            found.things.push(thing(30 + index, KIND_PLANE, 6, 2 * index));
            found.things.push(thing(40 + index, KIND_PLANE, 6, 2 * index + 1));
            found.planes.push(plane(30 + index, 6, PLANE_MAIN, 20 + index));
            found.planes.push(plane(40 + index, 6, PLANE_POINTER, 20 + index));
        }
        found.screens.push((screen(10, 6, true, 0), Some(mode(1920, 1080))));
        found.screens.push((screen(11, 6, false, 0), None));
        found.screens.push((screen(12, 6, true, 0), Some(mode(1024, 768))));
        found
    }

    /// The entries of `change`, read from its bytes, as (thing,
    /// property, value).
    fn entries(change: &mut ChangeBytes) -> Vec<(u32, u32, u64)> {
        let count = change.len();
        let bytes = change.bytes_mut();
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        (0..count)
            .map(|index| {
                let at = Change::SIZE + index * Entry::SIZE;
                let value = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
                (word(at + Entry::OFFSET_THING), word(at + Entry::OFFSET_PROPERTY), value)
            })
            .collect()
    }

    #[test]
    fn the_firmwares_screen_keeps_its_pipe() {
        let found = firmware();
        assert!(found.has_card());
        let outputs = outputs(&found);
        assert_eq!(
            outputs,
            [Output { screen: 3, card: 1, pipe: 2, plane: 4, mode: mode(1280, 800) }]
        );
        let mut change = change(&found, &outputs, &[(1, 2)]);
        assert_eq!(
            entries(&mut change),
            [
                (3, SCREEN_PIPE, 2),
                (2, PIPE_MODE, 2),
                (2, PIPE_ACTIVE, 1),
                (4, PLANE_PIPE, 2),
                (4, PLANE_PICTURE, 1),
            ]
        );
    }

    #[test]
    fn every_plugged_screen_of_every_card_gets_a_pipe_of_its_own() {
        let found = two_cards();
        let outputs = outputs(&found);
        let places: Vec<_> = outputs.iter().map(|o| (o.screen, o.pipe, o.plane)).collect();
        // Each virtio screen takes the pipe at its own place; the one
        // with no monitor is left out.
        assert_eq!(places, [(3, 2, 4), (10, 20, 30), (12, 22, 32)]);
        let mut change = change(&found, &outputs, &[(1, 4), (2, 5), (3, 6)]);
        let entries = entries(&mut change);
        // The unplugged screen is fed by nothing, its pipe makes no
        // frames, and its main plane shows nothing.
        assert!(entries.contains(&(11, SCREEN_PIPE, 0)));
        assert!(entries.contains(&(21, PIPE_ACTIVE, 0)));
        assert!(entries.contains(&(31, PLANE_PICTURE, 0)));
        assert!(entries.contains(&(22, PIPE_MODE, 6)));
        assert!(entries.contains(&(32, PLANE_PICTURE, 3)));
        // No pointer plane is touched.
        assert!(entries.iter().all(|&(thing, _, _)| !(40..43).contains(&thing) && thing != 5));
    }

    #[test]
    fn a_screen_too_small_failed_or_with_no_plane_left_is_left_out() {
        let mut found = two_cards();
        found.screens[1].1 = Some(mode(100, 600));
        found.screens[3].0.flags |= SCREEN_FAILED;
        let shown: Vec<u32> = outputs(&found).iter().map(|output| output.screen).collect();
        assert_eq!(shown, [3]);
        // A main plane that takes no picture of the mode's size.
        let mut found = firmware();
        found.planes[0].width_max = 1024;
        assert!(outputs(&found).is_empty());
        // No card at all.
        assert!(!Found::default().has_card());
        assert!(outputs(&Found::default()).is_empty());
    }

    #[test]
    fn a_screen_whose_pipe_is_taken_gets_the_next_free_one() {
        // Two screens that the layer says the same pipe feeds, as a
        // mirror another holder left: the second takes its own place's.
        let mut found = two_cards();
        found.screens[1].0.pipe = 22;
        found.screens[3].0.pipe = 22;
        let places: Vec<_> = outputs(&found).iter().map(|o| (o.screen, o.pipe)).collect();
        assert_eq!(places, [(3, 2), (10, 22), (12, 20)]);
    }
}
