// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What every part of the test stands on: a holder of the display layer
//! and its events ([`Seat`]), what the layer has ([`Found`]), the
//! keyboard the host presses to go on ([`Keys`]), and the kernel's own
//! account of the layer ([`State`]).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::time::{Duration, Instant};

use display::abi::{
    CardInfo, DisplayEvent, KIND_CARD, KIND_PIPE, KIND_PLANE, KIND_SCREEN, Mode, PLANE_MAIN,
    PLANE_POINTER, PlaneInfo, SCREEN_PLUGGED, ScreenInfo, Thing,
};
use display::{Control, Holder, wire};
use display_pattern::said::{self, Said};
use sysapi::{kernel, poll};

/// How a check ends: well, or with why it failed.
pub type Outcome<T = ()> = Result<T, String>;

/// Fails with `why` unless `holds`.
pub fn ensure(holds: bool, why: impl FnOnce() -> String) -> Outcome {
    if holds { Ok(()) } else { Err(why()) }
}

/// Says `line` in the log.
pub fn say(line: &Said) {
    println!("{line}");
}

/// Waits until `file` has something to read, for `patience` at most.
/// Returns whether it has: a wait that fails found nothing.
fn readable(file: &File, patience: Duration) -> bool {
    poll::wait_readable(file, patience).unwrap_or(false)
}

/// The file a holder is a description of.
const CONTROL: &str = "/devices/display/control";

/// A description of _/devices/display/control_.
#[derive(Debug)]
pub struct Device(File);

impl Control for Device {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        kernel::control(&self.0, request, argument, buffer).map_err(kernel::io_error)
    }
}

/// A holder, and the events it has read and not yet looked at.
#[derive(Debug)]
pub struct Seat {
    /// The holder.
    pub holder: Holder<Device>,
    waiting: VecDeque<DisplayEvent>,
}

impl Seat {
    /// A new holder, at the back.
    pub fn open() -> Outcome<Seat> {
        let file = File::options().read(true).write(true).open(CONTROL);
        Ok(Seat::of(file.map_err(|err| format!("{CONTROL}: {err}"))?))
    }

    /// The holder `file` is a description of.
    pub fn of(file: File) -> Seat {
        Seat { holder: Holder::new(Device(file)), waiting: VecDeque::new() }
    }

    /// Its description of the display layer's file.
    pub fn file(&self) -> &File {
        &self.holder.file().0
    }

    /// The slot its description is in, for another holder's LEND.
    pub fn slot(&self) -> u32 {
        kernel::slot_of(&self.holder.file().0)
    }

    /// The next event, the oldest first: one that waits already, or the
    /// first to come within `patience`.
    pub fn next(&mut self, patience: Duration) -> Outcome<Option<DisplayEvent>> {
        if self.waiting.is_empty() && readable(&self.holder.file().0, patience) {
            let mut bytes = [0; 16 * DisplayEvent::SIZE];
            let read = (&self.holder.file().0).read(&mut bytes);
            let read = read.map_err(|err| format!("reading the holder's events: {err}"))?;
            self.waiting.extend(wire::events(&bytes[..read]));
        }
        Ok(self.waiting.pop_front())
    }

    /// The first event within `patience` that `wanted` takes. Those
    /// before it are dropped. `what` names it if none comes.
    pub fn event(
        &mut self,
        what: &str,
        patience: Duration,
        wanted: impl Fn(&DisplayEvent) -> bool,
    ) -> Outcome<DisplayEvent> {
        let until = Instant::now() + patience;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.next(left)? {
                Some(event) if wanted(&event) => return Ok(event),
                Some(_) => {}
                None => return Err(format!("no {what} within {} ms", patience.as_millis())),
            }
        }
    }

    /// Reads the events the kernel has for the holder, and drops them:
    /// what a program does that makes changes and waits for none.
    pub fn drop_unread(&self) -> Outcome {
        let mut file = &self.holder.file().0;
        while readable(file, Duration::ZERO) {
            let mut bytes = [0; 16 * DisplayEvent::SIZE];
            file.read(&mut bytes).map_err(|err| format!("reading the holder's events: {err}"))?;
        }
        Ok(())
    }

    /// Forgets the events that wait and those that come within `quiet`.
    pub fn forget_events(&mut self, quiet: Duration) -> Outcome {
        while self.next(quiet)?.is_some() {}
        Ok(())
    }
}

/// One screen of a card.
#[derive(Clone, Debug)]
pub struct Screen {
    /// Its name in the test's lines: its card's name and its place.
    pub name: String,
    /// Where LIST has it.
    pub thing: Thing,
    /// What it is.
    pub info: ScreenInfo,
    /// Its modes, the preferred one first: none without a monitor.
    pub modes: Vec<Mode>,
}

impl Screen {
    /// Whether a monitor is plugged in.
    pub fn plugged(&self) -> bool {
        self.info.flags & SCREEN_PLUGGED != 0
    }
}

/// What the layer has.
#[derive(Clone, Debug, Default)]
pub struct Found {
    /// Every card, screen, pipe and plane.
    pub things: Vec<Thing>,
    /// Each card.
    pub cards: Vec<CardInfo>,
    /// Each screen.
    pub screens: Vec<Screen>,
    /// Each plane.
    pub planes: Vec<PlaneInfo>,
}

impl Found {
    /// Asks the layer what it has.
    pub fn read(holder: &Holder<Device>) -> Outcome<Found> {
        let failed = |err: io::Error| format!("asking what the layer has: {err}");
        let things = holder.things().map_err(failed)?;
        let mut found = Found { things, ..Found::default() };
        for thing in found.things.clone() {
            match thing.kind {
                KIND_CARD => found.cards.push(holder.card(thing.id).map_err(failed)?),
                KIND_SCREEN => {
                    let info = holder.screen(thing.id).map_err(failed)?;
                    let modes = holder.modes(&info).map_err(failed)?;
                    let card = found.cards.iter().find(|card| card.id == thing.card);
                    let card = card.map_or("card", |card| wire::text(&card.name));
                    let name = said::screen(card, thing.index);
                    found.screens.push(Screen { name, thing, info, modes });
                }
                KIND_PLANE => found.planes.push(holder.plane(thing.id).map_err(failed)?),
                _ => {}
            }
        }
        Ok(found)
    }

    /// The card with number `id`.
    pub fn card(&self, id: u32) -> Outcome<&CardInfo> {
        self.cards.iter().find(|card| card.id == id).ok_or_else(|| format!("no card {id}"))
    }

    /// The card named `name`.
    pub fn card_named(&self, name: &str) -> Outcome<&CardInfo> {
        let named = |card: &&CardInfo| wire::text(&card.name) == name;
        self.cards.iter().find(named).ok_or_else(|| format!("no card is named {name}"))
    }

    /// The screen named `name`.
    pub fn screen(&self, name: &str) -> Outcome<&Screen> {
        let found = self.screens.iter().find(|screen| screen.name == name);
        found.ok_or_else(|| format!("no screen is named {name}"))
    }

    /// The pipe at the same place on its card as `screen`.
    pub fn pipe_of(&self, screen: &Screen) -> Outcome<u32> {
        let same = |thing: &&Thing| {
            thing.kind == KIND_PIPE
                && thing.card == screen.thing.card
                && thing.index == screen.thing.index
        };
        let pipe = self.things.iter().find(same);
        pipe.map(|pipe| pipe.id).ok_or_else(|| format!("{} has no pipe of its own", screen.name))
    }

    /// The plane of `kind` that is `pipe`'s.
    pub fn plane_of(&self, pipe: u32, kind: u32) -> Outcome<u32> {
        let plane = self.planes.iter().find(|plane| plane.kind == kind && plane.fixed_pipe == pipe);
        let what = if kind == PLANE_POINTER { "pointer" } else { "main" };
        plane.map(|plane| plane.id).ok_or_else(|| format!("pipe {pipe} has no {what} plane"))
    }

    /// Every main plane.
    pub fn main_planes(&self) -> impl Iterator<Item = &PlaneInfo> {
        self.planes.iter().filter(|plane| plane.kind == PLANE_MAIN)
    }

    /// Every thing of `kind`.
    pub fn of_kind(&self, kind: u32) -> impl Iterator<Item = &Thing> {
        self.things.iter().filter(move |thing| thing.kind == kind)
    }
}

/// The keyboard: the host presses a key when it has looked at what a
/// step left on the screens.
#[derive(Debug)]
pub struct Keys(File);

/// A key event of _/devices/kbd_ is this long, and its `value`, at
/// byte 10, is this for a key that went down (radix-input(2)).
const KEY_EVENT: usize = 16;
const KEY_PRESSED: u16 = 1;

/// How long the host has to look at the screens and press a key.
const STEP_PATIENCE: Duration = Duration::from_secs(600);

impl Keys {
    /// Opens the keyboard. Keys pressed from here on are kept for
    /// [`Keys::step`].
    pub fn open() -> Outcome<Keys> {
        File::open("/devices/kbd").map(Keys).map_err(|err| format!("/devices/kbd: {err}"))
    }

    /// Says that step `name` is done, and waits for a key to go down.
    pub fn step(&mut self, name: &str) -> Outcome {
        say(&Said::Step(name.into()));
        let until = Instant::now() + STEP_PATIENCE;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if !readable(&self.0, left) {
                return Err(format!("no key was pressed after step {name}"));
            }
            let mut bytes = [0; 8 * KEY_EVENT];
            let read = self.0.read(&mut bytes).map_err(|err| format!("/devices/kbd: {err}"))?;
            let pressed =
                |event: &[u8; KEY_EVENT]| u16::from_le_bytes([event[10], event[11]]) == KEY_PRESSED;
            if bytes[..read].as_chunks::<KEY_EVENT>().0.iter().any(pressed) {
                return Ok(());
            }
        }
    }
}

/// The kernel's account of the layer, _/kernel/class/display/state_
/// (radix-sysfs(5)), as it was at one moment.
#[derive(Debug)]
pub struct State(String);

impl State {
    /// Reads it.
    pub fn read() -> Outcome<State> {
        const PATH: &str = "/kernel/class/display/state";
        std::fs::read_to_string(PATH).map(State).map_err(|err| format!("{PATH}: {err}"))
    }

    /// Who is in front: its first line.
    pub fn front(&self) -> &str {
        self.0.lines().next().unwrap_or("")
    }

    /// The number `key` has on the line of the `kind` with number `id`:
    /// a card's `wakes`, a pipe's `mode_sets`.
    pub fn number(&self, kind: &str, id: u32, key: &str) -> Outcome<u64> {
        let start = format!("{kind} {id} ");
        let line = self.0.lines().find(|line| line.starts_with(&start));
        let word = line.and_then(|line| {
            line.split(' ').find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
        });
        word.and_then(|number| number.parse().ok())
            .ok_or_else(|| format!("the kernel's account has no {key} for {kind} {id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_account_is_read_by_line_and_key() {
        let state = State(
            "front process 41\n\
             card 1 name=virtio0 kind=virtio hot_plug=1 wakes=17 model=virtio graphics card\n\
             pipe 2 card=1 index=0 active=1 width=1280 beat_count=412 mode_sets=3\n\
             pipe 21 card=1 index=1 active=0 width=0 beat_count=0 mode_sets=0\n"
                .into(),
        );
        assert_eq!(state.front(), "front process 41");
        assert_eq!(state.number("card", 1, "wakes"), Ok(17));
        assert_eq!(state.number("pipe", 2, "mode_sets"), Ok(3));
        // Pipe 2's line is not pipe 21's, and a key is a whole word.
        assert_eq!(state.number("pipe", 21, "mode_sets"), Ok(0));
        assert_eq!(
            state.number("pipe", 2, "count"),
            Err("the kernel's account has no count for pipe 2".into())
        );
        assert!(state.number("pipe", 3, "mode_sets").is_err());
    }
}
