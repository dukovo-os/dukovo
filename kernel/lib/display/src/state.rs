// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The model: what the cards have, the state a holder asks for, and the
//! check of that state (ADR 0700).
//!
//! [`Things`] is the hardware as the layer knows it: every card, screen,
//! pipe and plane, with what each can do. A [`State`] says what a holder
//! wants of them: for each pipe whether it makes frames and in which
//! mode, for each screen the pipe that feeds it, for each plane its pipe,
//! its picture and where the picture shows. Both are slices the kernel
//! owns, one element for each thing, found by the thing's number.
//!
//! A change is made in two steps, and neither touches a device.
//! [`apply`] alters a state by one entry and refuses a value the property
//! cannot take. [`check`] then looks at the state as a whole: every rule
//! that involves two things (a screen and its pipe's mode, a plane and
//! its picture) is checked there, once, after all the entries, so the
//! order of a change's entries never matters. A state that passes is one
//! the cards' drivers are able to make; a driver's own check may still
//! refuse what only it knows.
//!
//! When the check refuses, the answer names a thing and a reason, and
//! [`entry_of`] finds the entry of the change that set that thing.

use radix_abi::display::{
    BLOB_COLOR_TABLE, BLOB_HDR, CARD_MIRRORS, CARD_SETS_MODES, Entry, Mode, PIPE_ACTIVE,
    PIPE_COLOR_TABLE, PIPE_MODE, PIPE_VARIABLE_REFRESH, PLANE_CHANGED, PLANE_HOT_SPOT, PLANE_MAIN,
    PLANE_OPACITY, PLANE_ORDER, PLANE_PICTURE, PLANE_PIPE, PLANE_PLACE, PLANE_SOURCE, PLANE_TURN,
    REASON_BAD_VALUE, REASON_FORMAT, REASON_INCOMPLETE, REASON_MODE, REASON_NO_PROPERTY,
    REASON_NO_THING, REASON_NOT_OFFERED, REASON_OTHER_CARD, REASON_SIZE, REASON_UNPLUGGED,
    REASON_WOULD_BLANK, SCREEN_DEPTH, SCREEN_HDR_DETAILS, SCREEN_PIPE,
};

use crate::wire::ChangeRequest;
use crate::{Rect, same_mode};

#[cfg(test)]
mod tests;

/// The slowest and fastest rates a mode may have on a card that makes its
/// beat from the kernel's clock: one frame a second to a thousand.
const REFRESH_MHZ: core::ops::RangeInclusive<u32> = 1_000..=1_000_000;

/// How solid a plane is by default, and the only value a plane of
/// release 1.1 takes.
pub const OPAQUE: u16 = 0xffff;

/// What the check needs to know of a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// Its number.
    pub id: u32,
    /// CARD_* flag bits.
    pub flags: u32,
    /// The narrowest mode it sets, where it sets modes.
    pub width_min: u32,
    /// The shortest.
    pub height_min: u32,
    /// The widest mode it sets and picture it shows.
    pub width_max: u32,
    /// The tallest.
    pub height_max: u32,
}

/// What the check needs to know of a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    /// Its number.
    pub id: u32,
    /// Its card.
    pub card: u32,
    /// Whether a monitor is plugged in.
    pub plugged: bool,
    /// Whether the card stopped answering for it.
    pub failed: bool,
    /// The one pipe that can feed it, or 0 if any of its card's can.
    pub fixed_pipe: u32,
    /// The mode it was found in, which is the only one it shows on a
    /// card that sets no modes.
    pub native: Mode,
}

/// What the check needs to know of a pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pipe {
    /// Its number.
    pub id: u32,
    /// Its card.
    pub card: u32,
    /// How many entries its colour table has, or 0 if it has none.
    pub color_table_entries: u32,
    /// Whether its beat can follow its pictures.
    pub variable_refresh: bool,
}

/// What the check needs to know of a plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plane {
    /// Its number.
    pub id: u32,
    /// Its card.
    pub card: u32,
    /// A PLANE_MAIN, PLANE_POINTER or PLANE_OVERLAY.
    pub kind: u32,
    /// The one pipe it can be blended into, or 0 if any of its card's.
    pub fixed_pipe: u32,
    /// The widest picture it takes.
    pub width_max: u32,
    /// The tallest.
    pub height_max: u32,
    /// The formats it takes, padded with zeros.
    pub formats: [u32; 8],
    /// Whether the card draws its picture at another size.
    pub scales: bool,
}

/// The hardware as the layer knows it.
#[derive(Clone, Copy, Debug)]
pub struct Things<'a> {
    /// Every card.
    pub cards: &'a [Card],
    /// Every screen of every card.
    pub screens: &'a [Screen],
    /// Every pipe.
    pub pipes: &'a [Pipe],
    /// Every plane.
    pub planes: &'a [Plane],
}

impl Things<'_> {
    fn card(&self, id: u32) -> Option<&Card> {
        self.cards.iter().find(|card| card.id == id)
    }

    fn screen(&self, id: u32) -> Option<&Screen> {
        self.screens.iter().find(|screen| screen.id == id)
    }

    fn pipe(&self, id: u32) -> Option<&Pipe> {
        self.pipes.iter().find(|pipe| pipe.id == id)
    }

    fn plane(&self, id: u32) -> Option<&Plane> {
        self.planes.iter().find(|plane| plane.id == id)
    }
}

/// What a holder wants of a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenState {
    /// The screen's number.
    pub id: u32,
    /// The pipe that feeds it, or 0.
    pub pipe: u32,
    /// The bits a colour it is sent.
    pub depth: u8,
    /// Its high dynamic range details: a value's number, or 0.
    pub hdr: u32,
}

impl ScreenState {
    /// A screen that nothing feeds.
    pub const fn new(id: u32) -> ScreenState {
        ScreenState { id, pipe: 0, depth: 8, hdr: 0 }
    }
}

/// What a holder wants of a pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipeState {
    /// The pipe's number.
    pub id: u32,
    /// Whether it makes frames.
    pub active: bool,
    /// Whether its beat follows its pictures.
    pub variable_refresh: bool,
    /// Its colour table: a value's number, or 0.
    pub color_table: u32,
    /// Its mode; all zeros until one is set.
    pub mode: Mode,
}

impl PipeState {
    /// A pipe that makes no frames.
    pub const fn new(id: u32) -> PipeState {
        let mode = Mode {
            width: 0,
            height: 0,
            refresh_mhz: 0,
            flags: 0,
            clock_khz: 0,
            h_sync_start: 0,
            h_sync_end: 0,
            h_total: 0,
            v_sync_start: 0,
            v_sync_end: 0,
            v_total: 0,
        };
        PipeState { id, active: false, variable_refresh: false, color_table: 0, mode }
    }
}

/// What a holder wants of a plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneState {
    /// The plane's number.
    pub id: u32,
    /// The pipe it is blended into, or 0.
    pub pipe: u32,
    /// Its picture, or 0.
    pub picture: u32,
    /// The part of the picture it shows; empty for all of it.
    pub source: Rect,
    /// Where in the frame it shows; of no size for the size of the part
    /// shown.
    pub place: Rect,
    /// Its place in the order the card blends in.
    pub order: u32,
    /// How solid it is.
    pub opacity: u16,
    /// Its turn in degrees.
    pub turn: u16,
    /// The point of a pointer's picture that its place puts.
    pub hot_spot: (u16, u16),
}

impl PlaneState {
    /// A plane with no pipe and no picture.
    pub const fn new(id: u32) -> PlaneState {
        let nowhere = Rect { x: 0, y: 0, width: 0, height: 0 };
        PlaneState {
            id,
            pipe: 0,
            picture: 0,
            source: nowhere,
            place: nowhere,
            order: 0,
            opacity: OPAQUE,
            turn: 0,
            hot_spot: (0, 0),
        }
    }

    /// The part of `picture` it shows.
    pub fn source_in(&self, picture: &PictureShape) -> Rect {
        if self.source.is_empty() {
            Rect::sized(picture.width, picture.height)
        } else {
            self.source
        }
    }

    /// Where in the frame it shows `source`.
    pub fn place_of(&self, source: Rect) -> Rect {
        if self.place.is_empty() {
            Rect { width: source.width, height: source.height, ..self.place }
        } else {
            self.place
        }
    }
}

/// A state, to read.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    /// An element for each screen.
    pub screens: &'a [ScreenState],
    /// An element for each pipe.
    pub pipes: &'a [PipeState],
    /// An element for each plane.
    pub planes: &'a [PlaneState],
}

impl View<'_> {
    /// What it wants of screen `id`.
    pub fn screen(&self, id: u32) -> Option<&ScreenState> {
        self.screens.iter().find(|screen| screen.id == id)
    }

    /// What it wants of pipe `id`.
    pub fn pipe(&self, id: u32) -> Option<&PipeState> {
        self.pipes.iter().find(|pipe| pipe.id == id)
    }

    /// What it wants of plane `id`.
    pub fn plane(&self, id: u32) -> Option<&PlaneState> {
        self.planes.iter().find(|plane| plane.id == id)
    }

    /// Whether any of its planes shows picture `number`.
    pub fn shows(&self, number: u32) -> bool {
        self.planes.iter().any(|plane| plane.picture == number)
    }

    /// Whether any of its pipes or screens uses value `number`.
    pub fn uses(&self, number: u32) -> bool {
        self.pipes.iter().any(|pipe| pipe.color_table == number)
            || self.screens.iter().any(|screen| screen.hdr == number)
    }
}

/// A state, to alter.
#[derive(Debug)]
pub struct State<'a> {
    /// An element for each screen.
    pub screens: &'a mut [ScreenState],
    /// An element for each pipe.
    pub pipes: &'a mut [PipeState],
    /// An element for each plane.
    pub planes: &'a mut [PlaneState],
}

impl State<'_> {
    /// The state, to read.
    pub fn view(&self) -> View<'_> {
        View { screens: self.screens, pipes: self.pipes, planes: self.planes }
    }

    /// Starts a holder's state from the one that shows: the same pipes,
    /// modes and screens, so that its first change sets no mode it does
    /// not ask for, and each plane on its pipe with no picture, since a
    /// picture and a value are their holder's alone.
    pub fn adopt(&mut self, shown: &View<'_>) {
        for screen in self.screens.iter_mut() {
            let pipe = shown.screen(screen.id).map_or(0, |shown| shown.pipe);
            *screen = ScreenState { pipe, ..ScreenState::new(screen.id) };
        }
        for pipe in self.pipes.iter_mut() {
            let (active, mode) =
                shown.pipe(pipe.id).map_or((false, pipe.mode), |shown| (shown.active, shown.mode));
            *pipe = PipeState { active, mode, ..PipeState::new(pipe.id) };
        }
        for plane in self.planes.iter_mut() {
            let pipe = shown.plane(plane.id).map_or(0, |shown| shown.pipe);
            *plane = PlaneState { pipe, ..PlaneState::new(plane.id) };
        }
    }
}

/// What a picture is, as far as the check looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureShape {
    /// The card it is for.
    pub card: u32,
    /// Its format.
    pub format: u32,
    /// Its width in pixels.
    pub width: u32,
    /// Its height.
    pub height: u32,
}

/// A holder's pictures and values, by their numbers.
pub trait Values {
    /// Picture `number`, if the holder has it.
    fn picture(&self, number: u32) -> Option<PictureShape>;
    /// The mode that value `number` holds, if it is a BLOB_MODE.
    fn mode(&self, number: u32) -> Option<Mode>;
    /// What value `number` is, a BLOB_*, if the holder has it.
    fn blob_kind(&self, number: u32) -> Option<u32>;
}

/// What an entry did to a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// It set a property.
    Set,
    /// It named a rectangle of a plane's picture that changed, which is
    /// no part of the state.
    Changed {
        /// The plane's number.
        plane: u32,
        /// The rectangle of its picture.
        rect: Rect,
    },
}

/// Why a state cannot be shown: the thing at fault, and a REASON_*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The thing's number.
    pub thing: u32,
    /// The reason.
    pub reason: u32,
}

fn refuse(thing: u32, reason: u32) -> Result<(), Refusal> {
    Err(Refusal { thing, reason })
}

/// An entry's value as a thing's or a value's number.
fn number(value: u64) -> Result<u32, u32> {
    u32::try_from(value).map_err(|_| REASON_BAD_VALUE)
}

/// An entry's value as a yes or a no.
fn truth(value: u64) -> Result<bool, u32> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(REASON_BAD_VALUE),
    }
}

/// Alters `state` by one entry of a change.
///
/// # Errors
///
/// A REASON_*: REASON_NO_THING if the state has no such thing,
/// REASON_NO_PROPERTY if the thing has no such property, and
/// REASON_BAD_VALUE if the value is not one the property takes, or names
/// a pipe, picture or value that is not there.
pub fn apply(values: &dyn Values, state: &mut State<'_>, entry: &Entry) -> Result<Applied, u32> {
    let pipe_exists = |pipes: &[PipeState], id: u32| id == 0 || pipes.iter().any(|p| p.id == id);
    if let Some(pipe) = state.pipes.iter_mut().find(|pipe| pipe.id == entry.thing) {
        match entry.property {
            PIPE_ACTIVE => pipe.active = truth(entry.value)?,
            PIPE_MODE => {
                let mode = values.mode(number(entry.value)?).ok_or(REASON_BAD_VALUE)?;
                if mode.width == 0 || mode.height == 0 || mode.refresh_mhz == 0 {
                    return Err(REASON_BAD_VALUE);
                }
                pipe.mode = mode;
            }
            PIPE_VARIABLE_REFRESH => pipe.variable_refresh = truth(entry.value)?,
            PIPE_COLOR_TABLE => {
                let table = number(entry.value)?;
                if table != 0 && values.blob_kind(table) != Some(BLOB_COLOR_TABLE) {
                    return Err(REASON_BAD_VALUE);
                }
                pipe.color_table = table;
            }
            _ => return Err(REASON_NO_PROPERTY),
        }
        return Ok(Applied::Set);
    }
    if let Some(at) = state.screens.iter().position(|screen| screen.id == entry.thing) {
        match entry.property {
            SCREEN_PIPE => {
                let pipe = number(entry.value)?;
                if !pipe_exists(state.pipes, pipe) {
                    return Err(REASON_BAD_VALUE);
                }
                state.screens[at].pipe = pipe;
            }
            SCREEN_DEPTH => match entry.value {
                8 | 10 | 12 => state.screens[at].depth = entry.value as u8,
                _ => return Err(REASON_BAD_VALUE),
            },
            SCREEN_HDR_DETAILS => {
                let details = number(entry.value)?;
                if details != 0 && values.blob_kind(details) != Some(BLOB_HDR) {
                    return Err(REASON_BAD_VALUE);
                }
                state.screens[at].hdr = details;
            }
            _ => return Err(REASON_NO_PROPERTY),
        }
        return Ok(Applied::Set);
    }
    let Some(at) = state.planes.iter().position(|plane| plane.id == entry.thing) else {
        return Err(REASON_NO_THING);
    };
    let plane = &mut state.planes[at];
    match entry.property {
        PLANE_PIPE => {
            let pipe = number(entry.value)?;
            if !pipe_exists(state.pipes, pipe) {
                return Err(REASON_BAD_VALUE);
            }
            plane.pipe = pipe;
        }
        PLANE_PICTURE => {
            let picture = number(entry.value)?;
            if picture != 0 && values.picture(picture).is_none() {
                return Err(REASON_BAD_VALUE);
            }
            plane.picture = picture;
        }
        PLANE_SOURCE => plane.source = Rect::unpack(entry.value, false),
        PLANE_PLACE => plane.place = Rect::unpack(entry.value, true),
        PLANE_ORDER => plane.order = number(entry.value)?,
        PLANE_OPACITY => {
            plane.opacity = u16::try_from(entry.value).map_err(|_| REASON_BAD_VALUE)?
        }
        PLANE_TURN => match entry.value {
            0 | 90 | 180 | 270 => plane.turn = entry.value as u16,
            _ => return Err(REASON_BAD_VALUE),
        },
        PLANE_HOT_SPOT => {
            let spot = number(entry.value)?;
            plane.hot_spot = (spot as u16, (spot >> 16) as u16);
        }
        PLANE_CHANGED => {
            let rect = Rect::unpack(entry.value, false);
            if rect.is_empty() {
                return Err(REASON_BAD_VALUE);
            }
            return Ok(Applied::Changed { plane: plane.id, rect });
        }
        _ => return Err(REASON_NO_PROPERTY),
    }
    Ok(Applied::Set)
}

/// The entry of `request` to name when the check refuses `thing`: the
/// last one that set it, or the number of entries if none did, which is
/// a fault in the state the change started from.
pub fn entry_of(request: &ChangeRequest<'_>, thing: u32) -> u32 {
    let last = (0..request.len())
        .rev()
        .find(|&index| request.entry(index).is_some_and(|entry| entry.thing == thing));
    last.unwrap_or(request.len()) as u32
}

/// Checks that the cards can show `wanted`, and that going to it from
/// `shown` blanks no screen unless `may_blank` allows it.
///
/// # Errors
///
/// The first thing at fault, pipes before screens before planes, each in
/// the order of its slice, with the reason.
pub fn check(
    things: &Things<'_>,
    values: &dyn Values,
    shown: &View<'_>,
    wanted: &View<'_>,
    may_blank: bool,
) -> Result<(), Refusal> {
    for pipe in wanted.pipes {
        check_pipe(things, wanted, pipe)?;
    }
    for screen in wanted.screens {
        check_screen(things, wanted, screen)?;
    }
    for plane in wanted.planes {
        check_plane(things, values, wanted, plane)?;
    }
    if !may_blank {
        check_blanking(shown, wanted)?;
    }
    Ok(())
}

fn check_pipe(things: &Things<'_>, wanted: &View<'_>, state: &PipeState) -> Result<(), Refusal> {
    let Some(pipe) = things.pipe(state.id) else { return refuse(state.id, REASON_NO_THING) };
    if state.variable_refresh && !pipe.variable_refresh {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    if state.color_table != 0 && pipe.color_table_entries == 0 {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    if !state.active {
        return Ok(());
    }
    let feeds_a_screen = wanted.screens.iter().any(|screen| screen.pipe == state.id);
    if state.mode.width == 0 || !feeds_a_screen {
        return refuse(state.id, REASON_INCOMPLETE);
    }
    Ok(())
}

fn check_screen(
    things: &Things<'_>,
    wanted: &View<'_>,
    state: &ScreenState,
) -> Result<(), Refusal> {
    let Some(screen) = things.screen(state.id) else { return refuse(state.id, REASON_NO_THING) };
    // No card of release 1.1 sends more than 8 bits a colour or high
    // dynamic range.
    if state.depth != 8 || state.hdr != 0 {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    if state.pipe == 0 {
        return Ok(());
    }
    let (Some(pipe), Some(fed_by)) = (things.pipe(state.pipe), wanted.pipe(state.pipe)) else {
        return refuse(state.id, REASON_BAD_VALUE);
    };
    let Some(card) = things.card(screen.card) else { return refuse(state.id, REASON_NO_THING) };
    if pipe.card != screen.card {
        return refuse(state.id, REASON_OTHER_CARD);
    }
    if screen.fixed_pipe != 0 && screen.fixed_pipe != state.pipe {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    if !screen.plugged || screen.failed {
        return refuse(state.id, REASON_UNPLUGGED);
    }
    if !fed_by.active {
        return refuse(state.id, REASON_INCOMPLETE);
    }
    let mirrors = wanted.screens.iter().filter(|other| other.pipe == state.pipe).count() > 1;
    if mirrors && card.flags & CARD_MIRRORS == 0 {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    let mode = &fed_by.mode;
    let fits = if card.flags & CARD_SETS_MODES == 0 {
        same_mode(mode, &screen.native)
    } else {
        (card.width_min..=card.width_max).contains(&mode.width)
            && (card.height_min..=card.height_max).contains(&mode.height)
            && REFRESH_MHZ.contains(&mode.refresh_mhz)
    };
    if !fits {
        return refuse(state.id, REASON_MODE);
    }
    Ok(())
}

fn check_plane(
    things: &Things<'_>,
    values: &dyn Values,
    wanted: &View<'_>,
    state: &PlaneState,
) -> Result<(), Refusal> {
    let Some(plane) = things.plane(state.id) else { return refuse(state.id, REASON_NO_THING) };
    // No plane of release 1.1 blends its picture or turns it.
    if state.opacity != OPAQUE || state.turn != 0 {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    if state.pipe == 0 {
        return Ok(());
    }
    let (Some(pipe), Some(blended_into)) = (things.pipe(state.pipe), wanted.pipe(state.pipe))
    else {
        return refuse(state.id, REASON_BAD_VALUE);
    };
    if pipe.card != plane.card {
        return refuse(state.id, REASON_OTHER_CARD);
    }
    if plane.fixed_pipe != 0 && plane.fixed_pipe != state.pipe {
        return refuse(state.id, REASON_NOT_OFFERED);
    }
    if state.picture == 0 {
        return Ok(());
    }
    let Some(picture) = values.picture(state.picture) else {
        return refuse(state.id, REASON_BAD_VALUE);
    };
    if picture.card != plane.card {
        return refuse(state.id, REASON_OTHER_CARD);
    }
    if !plane.formats.contains(&picture.format) {
        return refuse(state.id, REASON_FORMAT);
    }
    let source = state.source_in(&picture);
    let place = state.place_of(source);
    let fits = source.within(picture.width, picture.height)
        && picture.width <= plane.width_max
        && picture.height <= plane.height_max
        && (plane.scales || (place.width, place.height) == (source.width, source.height));
    if !fits {
        return refuse(state.id, REASON_SIZE);
    }
    if plane.kind == PLANE_MAIN {
        if !blended_into.active {
            return refuse(state.id, REASON_INCOMPLETE);
        }
        // The main plane is the frame: it covers the mode and nothing
        // else.
        if place != Rect::sized(blended_into.mode.width, blended_into.mode.height) {
            return refuse(state.id, REASON_SIZE);
        }
    }
    Ok(())
}

/// Refuses a change that would blank a screen that shows: one whose pipe
/// stops or changes its mode, or that is moved to another pipe.
fn check_blanking(shown: &View<'_>, wanted: &View<'_>) -> Result<(), Refusal> {
    for now in shown.pipes.iter().filter(|pipe| pipe.active) {
        let keeps =
            wanted.pipe(now.id).is_some_and(|then| then.active && same_mode(&then.mode, &now.mode));
        if !keeps {
            return refuse(now.id, REASON_WOULD_BLANK);
        }
    }
    for now in shown.screens.iter().filter(|screen| screen.pipe != 0) {
        if wanted.screen(now.id).is_some_and(|then| then.pipe != now.pipe) {
            return refuse(now.id, REASON_WOULD_BLANK);
        }
    }
    Ok(())
}
