// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The layer: the cards and their things, the state that shows, the
//! holders and who is in front, and each pipe's beat and the change that
//! waits for it.
//!
//! This is where the rules of [`crate::state`], [`crate::stack`] and
//! [`crate::beat`] meet the cards. A state that passed the check is cut
//! into one [`Frame`] for each pipe, compared with the frame the pipe
//! shows, and handed to the pipe's card with what differs.
//!
//! The kernel keeps one [`Layer`] under one lock, and gives it the three
//! things it cannot know on the host: the allocator its lists come from,
//! what a picture's pixels are kept in and how it is shared ([`Picture`]),
//! and the cards ([`Card`]). It tells the layer the time as it takes the lock
//! ([`Layer::set_time`]), asks whether anyone is to be woken as it lets
//! go ([`Layer::take_news`]), and calls [`Layer::beat`] for each card by
//! the time the card was last told ([`Card::wake_for`]). The layer
//! itself never waits, and so runs the same in a test as in the kernel.

use alloc::vec::Vec;
use core::alloc::Allocator;
use core::ops::Deref;

use radix_abi::Status;
use radix_abi::display::{
    CHECK_ONLY, DisplayEvent, EVENT_BACK, EVENT_BEAT, EVENT_BORROWER_ENDED, EVENT_FAILED,
    EVENT_FRONT, EVENT_REPLACED, EVENT_SCREEN, EVENT_SHOWN, MAY_BLANK, Mode, NEWEST_WINS, NO_WAIT,
    NOW, PLANE_MAIN, PLANE_PICTURE, PLANE_POINTER,
};
use radix_abi::errors::{File, Kernel};

pub use self::plug::Found;
pub(crate) use self::stored::Stored;
use self::stored::{DAMAGE_MAX, Damage};
use crate::beat::Beat;
use crate::frame::{
    Card, CardSpec, Change, Frame, Picture, PlaneFault, PlaneSpec, ScreenSpec, Shown,
};
use crate::holder::Holder;
use crate::monitor::Monitor;
use crate::stack::{Passed, Refused, Stack};
use crate::state::{self, PipeState, PlaneState, Refusal, ScreenState, Things, Values, View};
use crate::wire::ChangeRequest;
use crate::{Rect, same_mode};

mod info;
mod plug;
mod stored;
#[cfg(test)]
mod tests;

/// The most cards.
pub const CARDS_MAX: usize = 4;
/// The most pipes of all cards: a change keeps a bit for each.
pub const PIPES_MAX: usize = 64;
/// The most screens of one card: a frame keeps a bit for each.
pub const SCREENS_MAX: usize = 32;
/// The most holders.
pub const HOLDERS_MAX: usize = 64;

fn no_memory() -> Status {
    Kernel::NoMemory.status(0)
}

fn invalid() -> Status {
    Kernel::InvalidArgument.status(0)
}

fn busy() -> Status {
    Kernel::Busy.status(0)
}

/// A change that waits for a pipe's beat.
struct Pending {
    /// The holder that asked for it.
    holder: u32,
    /// The holder's own number for it.
    cookie: u64,
    /// The layer's number for it, which a caller that waits looks for.
    seq: u64,
    /// Whether the holder is told with an EVENT_SHOWN.
    notify: bool,
    /// The first beat it may show at.
    at: u64,
    damage: Damage,
}

/// What the layer keeps of a pipe apart from its state.
struct PipeRun<A: Allocator> {
    /// Its beat, while it makes frames.
    beat: Option<Beat>,
    /// The number of its latest beat when it last stopped.
    count: u64,
    /// The latest beat that [`Layer::beat`] has dealt with.
    handled: u64,
    /// The change that waits for its next beat.
    pending: Option<Pending>,
    /// The holders that asked for a beat, each with the first beat that
    /// answers it.
    askers: Vec<(u32, u64), A>,
}

/// What the layer keeps of a card apart from what the check reads.
struct CardMore<C> {
    card: C,
    /// A CARD_*.
    kind: u32,
    name: [u8; 16],
    model: [u8; 32],
}

/// What the layer keeps of a screen apart from what the check reads.
struct ScreenMore<A: Allocator> {
    /// Its modes, the preferred one first.
    modes: Vec<Mode, A>,
    /// What its monitor says of itself.
    monitor: Monitor,
    /// The bytes the monitor said it with.
    data: Vec<u8, A>,
}

/// Everything the layer keeps: for pictures shared as `R`, of cards `C`,
/// in lists from `A`. An `R` is a counted reference to a picture, which
/// its holder and the plane that shows it each have one of.
pub struct Layer<R, C, A: Allocator> {
    alloc: A,
    /// The time of the call being served, which every event of it
    /// carries.
    now: u64,
    /// Set when something changed that a holder may wait for.
    news: bool,
    /// The number the next thing gets. Numbers are never used twice.
    next_id: u32,
    /// The number the last holder got.
    last_holder: u32,
    cards: Vec<state::Card, A>,
    screens: Vec<state::Screen, A>,
    pipes: Vec<state::Pipe, A>,
    planes: Vec<state::Plane, A>,
    /// An element for each card, in the order of `cards`.
    card_more: Vec<CardMore<C>, A>,
    /// An element for each screen.
    screen_more: Vec<ScreenMore<A>, A>,
    /// An element for each pipe.
    runs: Vec<PipeRun<A>, A>,
    /// The state that shows.
    shown: Stored<A>,
    /// The picture each plane shows, in the order of `planes`: kept here
    /// so that it lives while it shows, whatever its holder does.
    shown_pictures: Vec<Option<R>, A>,
    /// Where a change is built before it is checked.
    scratch: Stored<A>,
    holders: Vec<Holder<R, A>, A>,
    stack: Stack,
    /// The holder of `attention`, or 0.
    attention: u32,
    /// The card whose screen shows the kernel's log while no holder is
    /// in front, or 0.
    log_card: u32,
    /// The number the last change that waits got.
    seq: u64,
    /// Set as the machine ends: the screens are the log's from then on.
    closing: bool,
}

/// Why a change was not made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmade {
    /// The call fails with this.
    Status(Status),
    /// An entry failed: which, and a REASON_*.
    Refused(u32, u32),
}

impl From<Status> for Unmade {
    fn from(status: Status) -> Unmade {
        Unmade::Status(status)
    }
}

/// What a change that passed asks of its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Made {
    /// Nothing more.
    Done,
    /// To wait until the change with this number has shown
    /// ([`Layer::waits`]).
    Wait(u64),
}

/// What a change asks, read from its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asked {
    /// Only whether it would pass: nothing is made.
    pub check_only: bool,
    /// It may take the picture off a screen for a moment, as a new mode
    /// does.
    pub may_blank: bool,
    /// The call does not wait for it to show: an event tells of it.
    pub no_wait: bool,
    /// It shows at once, not at its pipes' next beat.
    pub now: bool,
    /// It takes the place of an earlier change that still waits.
    pub newest_wins: bool,
    /// The caller's own number for it, which its events carry.
    pub cookie: u64,
}

impl Asked {
    /// What `request` asks.
    pub fn of(request: &ChangeRequest<'_>) -> Asked {
        let flag = |flag: u32| request.flags & flag != 0;
        Asked {
            check_only: flag(CHECK_ONLY),
            may_blank: flag(MAY_BLANK),
            no_wait: flag(NO_WAIT),
            now: flag(NOW),
            newest_wins: flag(NEWEST_WINS),
            cookie: request.cookie,
        }
    }
}

/// The frame `view` asks of `pipe`, with each plane's picture found by
/// `picture`.
fn frame<'a, P: Picture>(
    things: &Things<'_>,
    view: &View<'_>,
    pipe: &state::Pipe,
    picture: &dyn Fn(usize, &PlaneState) -> Option<&'a P>,
) -> Frame<'a, P> {
    let state = view.pipe(pipe.id).copied().unwrap_or(PipeState::new(pipe.id));
    let place =
        things.pipes.iter().filter(|other| other.card == pipe.card).position(|p| p.id == pipe.id);
    let mut screens = 0;
    let of_card = things.screens.iter().filter(|screen| screen.card == pipe.card);
    for (on_card, screen) in of_card.enumerate().take(SCREENS_MAX) {
        if view.screen(screen.id).is_some_and(|state| state.pipe == pipe.id) {
            screens |= 1 << on_card;
        }
    }
    let shown = |kind: u32| {
        things.planes.iter().enumerate().find_map(|(at, plane)| {
            let state = view.plane(plane.id).filter(|state| state.pipe == pipe.id)?;
            let picture = picture(at, state).filter(|_| plane.kind == kind)?;
            let source = state.source_in(&picture.shape());
            let place = state.place_of(source);
            Some(Shown { picture, source, x: place.x, y: place.y, hot_spot: state.hot_spot })
        })
    };
    Frame {
        pipe: place.unwrap_or(0),
        active: state.active,
        mode: state.mode,
        screens,
        main: shown(PLANE_MAIN),
        pointer: shown(PLANE_POINTER),
    }
}

/// Whether two planes of two frames show the same pixels in the same
/// place, and whether they differ in place alone.
fn compare<P>(a: Option<&Shown<'_, P>>, b: Option<&Shown<'_, P>>) -> (bool, bool) {
    match (a, b) {
        (None, None) => (true, false),
        (Some(a), Some(b)) => {
            let same = core::ptr::eq(a.picture, b.picture)
                && a.source == b.source
                && a.hot_spot == b.hot_spot;
            (same && (a.x, a.y) == (b.x, b.y), same && (a.x, a.y) != (b.x, b.y))
        }
        _ => (false, false),
    }
}

/// What of `new` differs from `old`, with no rectangles yet.
fn difference<'d, P>(old: &Frame<'_, P>, new: &Frame<'_, P>) -> Change<'d> {
    let (same_main, moved_main) = compare(old.main.as_ref(), new.main.as_ref());
    let (same_pointer, moved) = compare(old.pointer.as_ref(), new.pointer.as_ref());
    Change {
        mode: old.active != new.active
            || old.screens != new.screens
            || (new.active && !same_mode(&old.mode, &new.mode)),
        main: !same_main || moved_main,
        pointer: !same_pointer && !moved,
        moved,
        damage: &[],
    }
}

/// Whether `a` and `b` are one picture.
fn same<P>(a: &P, b: &P) -> bool {
    core::ptr::eq(a, b)
}

/// Writes into `out` the rectangles of `damage` that are of `plane`'s
/// picture, which `shown` shows in a frame of `mode`, as rectangles of
/// that frame: those no part of which shows are left out. Returns how
/// many there are.
fn to_frame<P>(
    damage: &Damage,
    plane: u32,
    shown: &Shown<'_, P>,
    mode: &Mode,
    out: &mut [Rect; DAMAGE_MAX],
) -> usize {
    let within = Rect::sized(mode.width, mode.height);
    let mut count = 0;
    for rect in damage.of(plane) {
        let moved = Rect {
            x: rect.x - shown.source.x + shown.x,
            y: rect.y - shown.source.y + shown.y,
            ..rect
        };
        if let Some(rect) = moved.intersect(within) {
            out[count] = rect;
            count += 1;
        }
    }
    count
}

impl<R, C, A> Layer<R, C, A>
where
    R: Clone + Deref<Target: Picture + Sized>,
    C: Card<R::Target> + Clone,
    A: Allocator + Copy,
{
    /// A layer with no card, whose lists come from `alloc`.
    pub const fn new_in(alloc: A) -> Layer<R, C, A> {
        Layer {
            alloc,
            now: 0,
            news: false,
            next_id: 1,
            last_holder: 0,
            cards: Vec::new_in(alloc),
            screens: Vec::new_in(alloc),
            pipes: Vec::new_in(alloc),
            planes: Vec::new_in(alloc),
            card_more: Vec::new_in(alloc),
            screen_more: Vec::new_in(alloc),
            runs: Vec::new_in(alloc),
            shown: Stored::new_in(alloc),
            shown_pictures: Vec::new_in(alloc),
            scratch: Stored::new_in(alloc),
            holders: Vec::new_in(alloc),
            stack: Stack::new(),
            attention: 0,
            log_card: 0,
            seq: 0,
            closing: false,
        }
    }

    /// Tells the layer the time, in nanoseconds of the clock the beats
    /// are kept by: its caller does as it takes the layer's lock, so
    /// that everything one call does happens at one time.
    pub fn set_time(&mut self, now_ns: u64) {
        self.now = now_ns;
    }

    /// Whether something changed, since this was last asked, that a
    /// holder may wait for: an event to read, or a change that waited
    /// and waits no more. Its caller asks as it lets go of the lock, and
    /// wakes whoever waits.
    pub fn take_news(&mut self) -> bool {
        core::mem::take(&mut self.news)
    }

    /// The hardware, for the check.
    fn things(&self) -> Things<'_> {
        Things {
            cards: &self.cards,
            screens: &self.screens,
            pipes: &self.pipes,
            planes: &self.planes,
        }
    }

    /// The number the next thing gets.
    fn number(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// An event of now, to fill in.
    fn event(&self) -> DisplayEvent {
        DisplayEvent { time_ns: self.now, ..DisplayEvent::default() }
    }

    /// Adds the card `spec` describes, which `make` makes once the card
    /// has its number, and returns it. Everything is reserved first, so
    /// that a card is added whole or not at all. The first card that can
    /// show the kernel's log does from then on, while no holder is in
    /// front.
    ///
    /// # Errors
    ///
    /// Why the card was not added, for the log.
    pub fn add_card(
        &mut self,
        spec: &CardSpec<'_>,
        make: impl FnOnce(u32) -> Option<C>,
    ) -> Result<C, &'static str> {
        const NO_MEMORY: &str = "out of memory";
        let (screens, pipes) = (spec.screens.len(), spec.pipes);
        if self.cards.len() >= CARDS_MAX
            || self.pipes.len() + pipes > PIPES_MAX
            || screens > SCREENS_MAX
        {
            return Err("more cards, pipes or screens than the layer keeps");
        }
        if !self.reserve(screens, pipes) {
            return Err(NO_MEMORY);
        }
        let modes = self.first_modes(spec.screens).ok_or(NO_MEMORY)?;
        // A number that a card which is not made after all leaves unused
        // is one no thing ever had.
        let id = self.number();
        let card = make(id).ok_or(NO_MEMORY)?;
        // Room for every push from here on was reserved.
        let _ = self.cards.push_within_capacity(state::Card {
            id,
            flags: spec.flags,
            width_min: spec.width_min,
            height_min: spec.height_min,
            width_max: spec.width_max,
            height_max: spec.height_max,
        });
        let _ = self.card_more.push_within_capacity(CardMore {
            card: card.clone(),
            kind: spec.kind,
            name: spec.name,
            model: spec.model,
        });
        let first_pipe = self.next_id;
        self.add_pipes(id, pipes);
        self.add_screens(id, first_pipe, spec.screens, modes);
        self.add_planes(
            id,
            first_pipe,
            pipes,
            [(PLANE_MAIN, spec.main), (PLANE_POINTER, spec.pointer)],
        );
        // Nothing shows on the new things yet, and no holder asks
        // anything of them.
        let things = Things {
            cards: &self.cards,
            screens: &self.screens,
            pipes: &self.pipes,
            planes: &self.planes,
        };
        self.shown.grow(&things);
        for wanted in self.holders.iter_mut().filter_map(|holder| holder.wanted.as_mut()) {
            wanted.grow(&things);
        }
        if self.log_card == 0 && card.shows_log() {
            self.log_card = id;
            match self.stack.front() {
                None => self.log_front(),
                // A holder has the screens: the log waits its turn on
                // this one as it does on the rest.
                Some(_) => card.log_gives(),
            }
        }
        self.post_all(DisplayEvent { kind: EVENT_SCREEN, ..self.event() });
        Ok(card)
    }

    /// Makes room in every list for a card of so many screens and pipes,
    /// each pipe with its two planes. Returns whether there was memory
    /// for it.
    fn reserve(&mut self, screens: usize, pipes: usize) -> bool {
        let planes = pipes * 2;
        self.cards.try_reserve(1).is_ok()
            && self.card_more.try_reserve(1).is_ok()
            && self.screens.try_reserve(screens).is_ok()
            && self.screen_more.try_reserve(screens).is_ok()
            && self.pipes.try_reserve(pipes).is_ok()
            && self.runs.try_reserve(pipes).is_ok()
            && self.planes.try_reserve(planes).is_ok()
            && self.shown_pictures.try_reserve(planes).is_ok()
            && self.shown.reserve(screens, pipes, planes)
            && self.scratch.reserve(
                self.screens.len() + screens,
                self.pipes.len() + pipes,
                self.planes.len() + planes,
            )
            && self
                .holders
                .iter_mut()
                .filter_map(|holder| holder.wanted.as_mut())
                .all(|wanted| wanted.reserve(screens, pipes, planes))
    }

    /// The list of modes each of `screens` starts with: the one it was
    /// found in. `None` without the memory for them.
    fn first_modes(&self, screens: &[ScreenSpec]) -> Option<Vec<Vec<Mode, A>, A>> {
        let mut modes = Vec::new_in(self.alloc);
        modes.try_reserve_exact(screens.len()).ok()?;
        for screen in screens {
            let mut list = Vec::new_in(self.alloc);
            list.try_reserve_exact(1).ok()?;
            // Room was reserved for both.
            let _ = list.push_within_capacity(screen.native);
            let _ = modes.push_within_capacity(list);
        }
        Some(modes)
    }

    /// Adds `count` pipes of card `card`, which make no frames yet. Room
    /// for them was reserved.
    fn add_pipes(&mut self, card: u32, count: usize) {
        for _ in 0..count {
            let id = self.number();
            let _ = self.pipes.push_within_capacity(state::Pipe {
                id,
                card,
                color_table_entries: 0,
                variable_refresh: false,
            });
            let _ = self.runs.push_within_capacity(PipeRun {
                beat: None,
                count: 0,
                handled: 0,
                pending: None,
                askers: Vec::new_in(self.alloc),
            });
        }
    }

    /// Adds the screens of card `card`, whose first pipe has number
    /// `first_pipe`, each with its list of `modes`. Room for them was
    /// reserved.
    fn add_screens(
        &mut self,
        card: u32,
        first_pipe: u32,
        screens: &[ScreenSpec],
        modes: Vec<Vec<Mode, A>, A>,
    ) {
        for (screen, modes) in screens.iter().zip(modes) {
            let id = self.number();
            let _ = self.screens.push_within_capacity(state::Screen {
                id,
                card,
                plugged: screen.plugged,
                failed: false,
                fixed_pipe: screen.fixed_pipe.map_or(0, |pipe| first_pipe + pipe as u32),
                native: screen.native,
            });
            let _ = self.screen_more.push_within_capacity(ScreenMore {
                modes,
                monitor: Monitor::default(),
                data: Vec::new_in(self.alloc),
            });
        }
    }

    /// Adds a plane of each of `kinds` for each of the `pipes` pipes of
    /// card `card` from number `first_pipe` on. Room for them was
    /// reserved.
    fn add_planes(
        &mut self,
        card: u32,
        first_pipe: u32,
        pipes: usize,
        kinds: [(u32, PlaneSpec); 2],
    ) {
        for pipe in 0..pipes {
            for (kind, plane) in kinds {
                let id = self.number();
                let _ = self.planes.push_within_capacity(state::Plane {
                    id,
                    card,
                    kind,
                    fixed_pipe: first_pipe + pipe as u32,
                    width_max: plane.width_max,
                    height_max: plane.height_max,
                    formats: plane.formats,
                    scales: false,
                });
                let _ = self.shown_pictures.push_within_capacity(None);
            }
        }
    }

    /// The place of holder `id` in the list.
    fn holder_place(&self, id: u32) -> Option<usize> {
        self.holders.iter().position(|holder| holder.id == id)
    }

    /// Holder `id`.
    pub fn holder(&mut self, id: u32) -> Option<&mut Holder<R, A>> {
        self.holders.iter_mut().find(|holder| holder.id == id)
    }

    /// Adds `event` to what holder `id` has to read.
    fn post(&mut self, id: u32, event: DisplayEvent) {
        if let Some(holder) = self.holder(id) {
            holder.post(event);
            self.news = true;
        }
    }

    /// Adds `event` to what every holder has to read.
    fn post_all(&mut self, event: DisplayEvent) {
        for holder in &mut self.holders {
            holder.post(event);
        }
        self.news = true;
    }

    /// The card with number `id`.
    pub fn card(&self, id: u32) -> Option<&C> {
        let at = self.cards.iter().position(|other| other.id == id)?;
        Some(&self.card_more[at].card)
    }

    /// The card that shows the kernel's log while no holder is in
    /// front, if one can.
    pub fn log_card(&self) -> Option<&C> {
        self.card(self.log_card)
    }

    /// The frame `pipe` shows.
    fn shown_frame(&self, pipe: &state::Pipe) -> Frame<'_, R::Target> {
        let picture = |at: usize, _: &PlaneState| self.shown_pictures[at].as_deref();
        frame(&self.things(), &self.shown.view(), pipe, &picture)
    }

    /// The frame `view` asks of `pipe`, with `holder`'s pictures.
    fn wanted_frame<'a>(
        &self,
        holder: &'a Holder<R, A>,
        view: &View<'_>,
        pipe: &state::Pipe,
    ) -> Frame<'a, R::Target> {
        let picture = |_: usize, plane: &PlaneState| {
            holder.picture_ref(plane.picture).map(|picture| &**picture)
        };
        frame(&self.things(), view, pipe, &picture)
    }

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

    /// Keeps a pipe's beat in step with what it shows: started or
    /// started again where its mode is new, stopped where it stopped.
    fn keep_beat(&mut self, place: usize) {
        let now = self.now;
        let state = self.shown.pipes[place];
        let run = &mut self.runs[place];
        let rate = if state.active { state.mode.refresh_mhz } else { 0 };
        if run.beat.is_some_and(|beat| beat.refresh_mhz() == rate) {
            return;
        }
        if let Some(beat) = run.beat {
            run.count = beat.latest(now);
        }
        run.beat = Beat::start(now, run.count.saturating_add(1), rate);
        if run.beat.is_some() {
            run.count = run.count.saturating_add(1);
            run.handled = run.count;
        }
    }

    /// The kernel's log comes to the front: its card shows it, every
    /// other pipe shows nothing, and what waited for a beat is dropped.
    fn log_front(&mut self) {
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
            if log_pipe == Some(place) {
                card.log_takes(&new);
            } else if old.active {
                // A card that does not answer says so in the log itself;
                // the log shows on its own card either way.
                let change = Change { main: true, pointer: true, ..difference(&old, &new) };
                let _ = card.show(&new, &change);
            }
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

    /// The first holder takes the front from the kernel's log.
    fn leave_log(&mut self, holder: u32) -> Result<(), Status> {
        if self.closing {
            return Err(busy());
        }
        self.stack.leave_log(holder).map_err(|_| busy())?;
        if let Some(card) = self.card(self.log_card) {
            card.log_gives();
        }
        self.post(holder, DisplayEvent { kind: EVENT_FRONT, ..self.event() });
        Ok(())
    }

    /// Passes the front as `passed` says: the one that had it is told,
    /// the state of the one that has it now is made again, and it is
    /// told; or the kernel's log shows. `ended` says the one that had it
    /// is gone, which the one it gives to is told first.
    fn pass(&mut self, passed: Passed, ended: bool) {
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
        let Some(to) = passed.to else {
            self.log_front();
            return;
        };
        if ended {
            self.post(to, DisplayEvent { kind: EVENT_BORROWER_ENDED, ..self.event() });
        }
        self.put_back(to);
        self.post(to, DisplayEvent { kind: EVENT_FRONT, ..self.event() });
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

    /// Asks each card whether it can show what `view` asks of its pipes,
    /// with `holder`'s pictures.
    fn card_check(&self, holder: &Holder<R, A>, view: &View<'_>) -> Result<(), Refusal> {
        for pipe in &self.pipes {
            let Some(card) = self.card(pipe.card) else { continue };
            let wanted = self.wanted_frame(holder, view, pipe);
            if let Err(PlaneFault { kind, reason }) = card.check(&wanted) {
                let plane = self
                    .planes
                    .iter()
                    .find(|plane| plane.kind == kind && plane.fixed_pipe == pipe.id)
                    .map_or(pipe.id, |plane| plane.id);
                return Err(Refusal { thing: plane, reason });
            }
        }
        Ok(())
    }

    /// Makes the pipe at `place` show what the holder at `holder` of the
    /// list asked of it, if that differs from what it shows or `damage`
    /// names pixels of it, or `all` says its pixels may not be what the
    /// state says: a holder's first frames after the log's or after
    /// another holder's. Returns whether the pipe shows it now.
    // Out of line: its frame holds the rectangles and two frames of a
    // pipe, and the callers that hold a change's own are deep already.
    #[inline(never)]
    fn make_pipe(&mut self, place: usize, holder: usize, damage: &Damage, all: bool) -> bool {
        let pipe = self.pipes[place];
        let Some(card) = self.card(pipe.card).cloned() else { return false };
        let made = {
            let holder = &self.holders[holder];
            let Some(wanted) = holder.wanted.as_ref().map(Stored::view) else { return false };
            let (old, new) = (self.shown_frame(&pipe), self.wanted_frame(holder, &wanted, &pipe));
            // The plane of `kind` that is on the pipe.
            let plane = |kind: u32| {
                let on = |plane: &&state::Plane| {
                    plane.kind == kind
                        && wanted.plane(plane.id).is_some_and(|state| state.pipe == pipe.id)
                };
                self.planes.iter().find(on).map(|plane| plane.id)
            };
            let mut change = difference(&old, &new);
            change.main |= all || damage.everything();
            change.pointer |= all
                || damage.everything()
                || plane(PLANE_POINTER).is_some_and(|pointer| damage.names(pointer));
            let mut rects = [Rect::default(); DAMAGE_MAX];
            let count = match (plane(PLANE_MAIN), new.main.as_ref()) {
                (Some(main), Some(shown)) => to_frame(damage, main, shown, &new.mode, &mut rects),
                _ => 0,
            };
            change.damage = &rects[..count];
            let same = !(change.mode || change.main || change.pointer || change.moved);
            // A pipe that made no frames and is to make none has nothing
            // to draw.
            let idle = !old.active && !new.active;
            if idle || (same && count == 0) { Ok(()) } else { card.show(&new, &change) }
        };
        if made.is_err() {
            self.failed(place);
            return false;
        }
        self.shows(place, holder);
        self.keep_beat(place);
        true
    }

    /// Notes that the pipe at `place` shows what the holder at `holder`
    /// of the list asked of it.
    fn shows(&mut self, place: usize, holder: usize) {
        let pipe = self.pipes[place];
        let holder = &self.holders[holder];
        let Some(wanted) = holder.wanted.as_ref().map(Stored::view) else { return };
        if let Some(state) = wanted.pipe(pipe.id) {
            self.shown.pipes[place] = *state;
        }
        for (at, screen) in self.screens.iter().enumerate() {
            let Some(state) = wanted.screen(screen.id) else { continue };
            if state.pipe == pipe.id {
                self.shown.screens[at] = *state;
            } else if self.shown.screens[at].pipe == pipe.id {
                self.shown.screens[at] = ScreenState::new(screen.id);
            }
        }
        for (at, plane) in self.planes.iter().enumerate() {
            let Some(state) = wanted.plane(plane.id) else { continue };
            if state.pipe == pipe.id {
                self.shown.planes[at] = *state;
                self.shown_pictures[at] = holder.picture_ref(state.picture).cloned();
            } else if self.shown.planes[at].pipe == pipe.id {
                self.shown.planes[at] = PlaneState::new(plane.id);
                self.shown_pictures[at] = None;
            }
        }
    }

    /// The card did not show what the pipe at `place` was to: its
    /// screens are marked failed, and every holder is told.
    fn failed(&mut self, place: usize) {
        let pipe = self.pipes[place].id;
        for at in 0..self.screens.len() {
            if self.shown.screens[at].pipe == pipe {
                self.screens[at].failed = true;
                let thing = self.screens[at].id;
                self.post_all(DisplayEvent { kind: EVENT_SCREEN, thing, ..self.event() });
            }
        }
    }

    /// Drops the change that waits for the beat of the pipe at `place`,
    /// if one does: a caller that waits for it is to be woken.
    fn drop_pending(&mut self, place: usize) {
        if self.runs[place].pending.take().is_some() {
            self.news = true;
        }
    }

    /// CHANGE: alters the state holder `me` asked for by the entries of
    /// `request`, checks it, and makes it or leaves it to its pipes' next
    /// beats. Nothing is altered unless all of it passes. The first
    /// change of a holder while the kernel's log is in front takes the
    /// front.
    ///
    /// # Errors
    ///
    /// The entry that failed and why; or a status: busy for a holder
    /// that is not in front or whose earlier change of a pipe still
    /// waits, gone for no such holder, and no_memory.
    pub fn change(
        &mut self,
        me: u32,
        asked: Asked,
        request: &ChangeRequest<'_>,
    ) -> Result<Made, Unmade> {
        let at = self.holder_place(me).ok_or(Kernel::Gone.status(0))?;
        let front = self.stack.front();
        let takes = front.is_none() && !self.closing;
        if !asked.check_only && front != Some(me) && !takes {
            return Err(busy().into());
        }
        // Its first change starts from what shows.
        let first = self.holders[at].wanted.is_none();
        if first {
            let mut wanted = Stored::new_in(self.alloc);
            wanted.blank(&self.things())?;
            wanted.state().adopt(&self.shown.view());
            self.holders[at].wanted = Some(wanted);
        }
        let made = self.check_and_make(at, asked, request, takes);
        if first && (asked.check_only || made.is_err()) {
            // It has still made none: when it comes to the front, what
            // shows stays until it does.
            self.holders[at].wanted = None;
        }
        made
    }

    /// The rest of [`Layer::change`], for the holder at `at` of the
    /// list, which has a state to start from.
    fn check_and_make(
        &mut self,
        at: usize,
        asked: Asked,
        request: &ChangeRequest<'_>,
        takes: bool,
    ) -> Result<Made, Unmade> {
        let mut scratch = core::mem::replace(&mut self.scratch, Stored::new_in(self.alloc));
        let made = match self.ready(at, asked, request, takes, &mut scratch) {
            Ok(Some((damage, touched))) => {
                // The state that passed is the holder's from here.
                if let Some(wanted) = self.holders[at].wanted.as_mut() {
                    core::mem::swap(wanted, &mut scratch);
                }
                Ok(self.carry_out(at, asked, &damage, touched, takes))
            }
            Ok(None) => Ok(Made::Done),
            Err(unmade) => Err(unmade),
        };
        self.scratch = scratch;
        made
    }

    /// Builds in `scratch` the state a change asks for and checks it and
    /// that it may be made now; if `takes`, its holder then takes the
    /// front from the kernel's log. Returns what [`Layer::build`] does,
    /// or `None` for a change that only asked whether it would pass.
    fn ready(
        &mut self,
        at: usize,
        asked: Asked,
        request: &ChangeRequest<'_>,
        takes: bool,
        scratch: &mut Stored<A>,
    ) -> Result<Option<(Damage, u64)>, Unmade> {
        let (damage, touched) = self.build(at, asked, request, scratch)?;
        if asked.check_only {
            return Ok(None);
        }
        // A pipe whose earlier change still waits takes no other, unless
        // the newer is to win.
        let waits =
            |place: usize| touched & (1 << place) != 0 && self.runs[place].pending.is_some();
        if !asked.newest_wins && (0..self.pipes.len().min(PIPES_MAX)).any(waits) {
            return Err(busy().into());
        }
        if takes {
            self.leave_log(self.holders[at].id)?;
        }
        Ok(Some((damage, touched)))
    }

    /// Builds in `scratch` the state the holder at `at` of the list asks
    /// for with `request`, and checks it. Returns the rectangles the
    /// request says changed, and the pipes the new state touches, a bit
    /// for each by its place in the list.
    // Out of line, with a frame of its own for the rectangles.
    #[inline(never)]
    fn build(
        &self,
        at: usize,
        asked: Asked,
        request: &ChangeRequest<'_>,
        scratch: &mut Stored<A>,
    ) -> Result<(Damage, u64), Unmade> {
        let holder = &self.holders[at];
        let Some(wanted) = holder.wanted.as_ref() else {
            return Err(no_memory().into());
        };
        scratch.assign(wanted)?;
        let mut damage = Damage::new();
        for (index, entry) in request.entries().enumerate() {
            match state::apply(holder, &mut scratch.state(), &entry) {
                Ok(state::Applied::Set) => {}
                Ok(state::Applied::Changed { plane, rect }) => damage.add(plane, rect),
                Err(reason) => return Err(Unmade::Refused(index as u32, reason)),
            }
        }
        // A change that names a plane's picture and no rectangle of it
        // says all of the picture changed.
        for entry in request.entries().filter(|entry| entry.property == PLANE_PICTURE) {
            let picture = scratch.view().plane(entry.thing).map_or(0, |state| state.picture);
            if let Some(shape) = holder.picture(picture)
                && !damage.names(entry.thing)
            {
                damage.add(entry.thing, Rect::sized(shape.width, shape.height));
            }
        }
        let refused = |refusal: Refusal| {
            Unmade::Refused(state::entry_of(request, refusal.thing), refusal.reason)
        };
        state::check(&self.things(), holder, &self.shown.view(), &scratch.view(), asked.may_blank)
            .map_err(refused)?;
        self.card_check(holder, &scratch.view()).map_err(refused)?;
        Ok((damage, self.touched(holder, scratch, &damage)))
    }

    /// The pipes that `wanted`, a state of `holder`'s, would change: a
    /// bit for each, by its place in the list, whose frame would differ
    /// from the one that shows or whose planes have rectangles named in
    /// `damage`.
    fn touched(&self, holder: &Holder<R, A>, wanted: &Stored<A>, damage: &Damage) -> u64 {
        if damage.everything() {
            return u64::MAX;
        }
        let mut touched = 0;
        for (place, pipe) in self.pipes.iter().enumerate().take(PIPES_MAX) {
            let old = self.shown_frame(pipe);
            let new = self.wanted_frame(holder, &wanted.view(), pipe);
            let change = difference(&old, &new);
            let named =
                wanted.planes.iter().any(|plane| plane.pipe == pipe.id && damage.names(plane.id));
            if change.mode || change.main || change.pointer || change.moved || named {
                touched |= 1 << place;
            }
        }
        touched
    }

    /// Carries out a change that passed, of the holder at `at` of the
    /// list. Each pipe of `touched` is made at once if its mode changes,
    /// if the change asks for now, or if it has no beat to wait for, and
    /// is left to its next beat otherwise. With `all`, the holder has
    /// just taken the front from the kernel's log: every pipe is made at
    /// once, whatever the log left on it.
    // Out of line, with a frame of its own for the change that waits.
    #[inline(never)]
    fn carry_out(
        &mut self,
        at: usize,
        asked: Asked,
        damage: &Damage,
        touched: u64,
        all: bool,
    ) -> Made {
        let me = self.holders[at].id;
        self.seq += 1;
        let seq = self.seq;
        let mut waits = false;
        for place in 0..self.pipes.len().min(PIPES_MAX) {
            let touched = touched & (1 << place) != 0;
            if !touched && !all {
                continue;
            }
            let at_once = asked.now || all || self.sets_mode(place, at);
            match self.runs[place].beat.filter(|_| !at_once) {
                Some(beat) => {
                    let notify = asked.no_wait;
                    let pending = Pending {
                        holder: me,
                        cookie: asked.cookie,
                        seq,
                        notify,
                        at: 0,
                        damage: *damage,
                    };
                    self.leave_to_beat(place, beat, pending);
                    waits |= !asked.no_wait;
                }
                None => {
                    let shown = self.make_now(place, at, damage, all);
                    if asked.no_wait && touched {
                        let kind = if shown { EVENT_SHOWN } else { EVENT_FAILED };
                        self.tell(place, me, kind, asked.cookie);
                    }
                }
            }
        }
        if waits { Made::Wait(seq) } else { Made::Done }
    }

    /// Whether what the holder at `at` of the list asks of the pipe at
    /// `place` differs from what the pipe shows in whether it makes
    /// frames, in its mode, or in the screens it feeds.
    fn sets_mode(&self, place: usize, at: usize) -> bool {
        let (pipe, holder) = (&self.pipes[place], &self.holders[at]);
        let Some(wanted) = holder.wanted.as_ref() else { return false };
        let new = self.wanted_frame(holder, &wanted.view(), pipe);
        difference(&self.shown_frame(pipe), &new).mode
    }

    /// Makes the pipe at `place` show now what the holder at `at` of the
    /// list asked of it. An earlier change of the pipe that waited is
    /// part of what is made. Returns whether the pipe shows it.
    fn make_now(&mut self, place: usize, at: usize, damage: &Damage, all: bool) -> bool {
        let mut whole = *damage;
        if let Some(earlier) = self.runs[place].pending.take() {
            whole.join(&earlier.damage);
            self.replaced(&earlier);
        }
        self.make_pipe(place, at, &whole, all)
    }

    /// Leaves `pending` to the next beat of the pipe at `place`, which
    /// is `beat`, and has the pipe's card woken for it. What an earlier
    /// change that waited named never showed: it is still to be drawn,
    /// at the beat the earlier one waited for.
    fn leave_to_beat(&mut self, place: usize, beat: Beat, mut pending: Pending) {
        pending.at = beat.latest(self.now).saturating_add(1);
        if let Some(earlier) = self.runs[place].pending.take() {
            pending.damage.join(&earlier.damage);
            pending.at = pending.at.min(earlier.at);
            self.replaced(&earlier);
        }
        let due = beat.time_of(pending.at);
        self.runs[place].pending = Some(pending);
        if let Some(card) = self.card(self.pipes[place].card) {
            card.wake_for(due);
        }
    }

    /// A change that waited gave way to a newer one: its holder is told,
    /// if it asked to be, and a caller that waited for it is woken.
    fn replaced(&mut self, earlier: &Pending) {
        if earlier.notify {
            let event =
                DisplayEvent { kind: EVENT_REPLACED, cookie: earlier.cookie, ..self.event() };
            self.post(earlier.holder, event);
        }
        self.news = true;
    }

    /// Tells holder `id` that a change of the pipe at `place` showed, or
    /// did not, or of a beat: an event of `kind` with the pipe's latest
    /// beat.
    fn tell(&mut self, place: usize, id: u32, kind: u32, cookie: u64) {
        let pipe = self.pipes[place];
        let latch = self.card(pipe.card).map_or(0, Card::latch_ns);
        let mut event = DisplayEvent { kind, thing: pipe.id, cookie, ..self.event() };
        if let Some(beat) = self.runs[place].beat {
            let count = beat.latest(self.now);
            event.time_ns = beat.time_of(count);
            event.count = count;
            event.deadline_ns = beat.deadline(count, latch);
        }
        self.post(id, event);
    }

    /// Whether the change with number `seq` still waits for a beat.
    pub fn waits(&self, seq: u64) -> bool {
        self.runs.iter().any(|run| run.pending.as_ref().is_some_and(|pending| pending.seq == seq))
    }

    /// The beats of the pipes of card `card` that have come by now:
    /// each change that waited for one is made, and each holder that
    /// asked for one is told.
    pub fn beat(&mut self, card: u32) {
        for place in 0..self.pipes.len() {
            if self.pipes[place].card != card {
                continue;
            }
            let Some(beat) = self.runs[place].beat else { continue };
            let count = beat.latest(self.now);
            if count <= self.runs[place].handled {
                continue;
            }
            self.runs[place].handled = count;
            if let Some(pending) = self.runs[place].pending.take_if(|pending| pending.at <= count) {
                // Only the holder in front shows: one that lent the
                // front since has had what waited dropped already.
                let holder = self.holder_place(pending.holder);
                let in_front = self.stack.front() == Some(pending.holder);
                if let Some(holder) = holder.filter(|_| in_front) {
                    let shown = self.make_pipe(place, holder, &pending.damage, false);
                    if pending.notify {
                        let kind = if shown { EVENT_SHOWN } else { EVENT_FAILED };
                        self.tell(place, pending.holder, kind, pending.cookie);
                    }
                }
                self.news = true;
            }
            let mut asked =
                core::mem::replace(&mut self.runs[place].askers, Vec::new_in(self.alloc));
            asked.retain(|&(holder, at)| {
                if at <= count {
                    self.tell(place, holder, EVENT_BEAT, 0);
                }
                at > count
            });
            self.runs[place].askers = asked;
        }
    }

    /// When [`Layer::beat`] is next needed for card `card`: the time of
    /// the earliest beat that a change or a holder waits for.
    pub fn next_beat(&self, card: u32) -> Option<u64> {
        let waited = |place: usize| {
            let run = &self.runs[place];
            let beat = run.beat?;
            let change = run.pending.as_ref().map(|pending| pending.at);
            let asked = run.askers.iter().map(|&(_, at)| at).min();
            let first = [change, asked].into_iter().flatten().min()?;
            Some(beat.time_of(first.max(run.handled.saturating_add(1))))
        };
        (0..self.pipes.len())
            .filter(|&place| self.pipes[place].card == card)
            .filter_map(waited)
            .min()
    }

    /// ASK_BEAT: holder `me` is told of the next beat of pipe `pipe`.
    ///
    /// # Errors
    ///
    /// invalid_argument for no such pipe, or one that makes no frames,
    /// and no_memory.
    pub fn ask_beat(&mut self, me: u32, pipe: u32) -> Result<(), Status> {
        let place = self.pipes.iter().position(|other| other.id == pipe).ok_or_else(invalid)?;
        // A pipe that makes no frames has no beat to tell of.
        let beat = self.runs[place].beat.ok_or_else(invalid)?;
        let at = beat.latest(self.now).saturating_add(1);
        let askers = &mut self.runs[place].askers;
        if !askers.iter().any(|&(holder, _)| holder == me) {
            askers.try_reserve(1).map_err(|_| no_memory())?;
            // Room was reserved.
            let _ = askers.push_within_capacity((me, at));
        }
        if let Some(card) = self.card(self.pipes[place].card) {
            card.wake_for(beat.time_of(at));
        }
        Ok(())
    }

    /// Holder `id` has gone: the front passes on if it had it, what it
    /// asked of a beat is dropped, and its pictures go with it but for
    /// those that still show.
    pub fn end(&mut self, id: u32) {
        if self.attention == id {
            self.attention = 0;
        }
        for run in &mut self.runs {
            run.askers.retain(|&(holder, _)| holder != id);
        }
        match self.stack.end(id) {
            Some(passed) => self.pass(passed, true),
            None => {
                for place in 0..self.pipes.len() {
                    if self.runs[place].pending.as_ref().is_some_and(|pending| pending.holder == id)
                    {
                        self.drop_pending(place);
                    }
                }
            }
        }
        // Its pictures go with it, but for those a plane still shows.
        if let Some(at) = self.holder_place(id) {
            drop(self.holders.swap_remove(at));
        }
    }

    /// MOVE_POINTER: puts the hot spot of pointer plane `plane` of
    /// holder `me`, which is in front, at (`x`, `y`) of its pipe's
    /// frame, at once.
    ///
    /// # Errors
    ///
    /// busy for a holder that is not in front, invalid_argument for a
    /// plane that is no pointer plane or a holder that has made no
    /// change, and io if the card did not move it.
    pub fn move_pointer(&mut self, me: u32, plane: u32, x: i32, y: i32) -> Result<(), Status> {
        if self.stack.front() != Some(me) {
            return Err(busy());
        }
        let pointer = |other: &state::Plane| other.id == plane && other.kind == PLANE_POINTER;
        let at = self.planes.iter().position(pointer).ok_or_else(invalid)?;
        let holder = self.holder_place(me).ok_or(Kernel::Gone.status(0))?;
        let wanted = self.holders[holder].wanted.as_mut().ok_or_else(invalid)?;
        let state = wanted.planes.iter_mut().find(|state| state.id == plane).ok_or_else(invalid)?;
        state.place.x = x - i32::from(state.hot_spot.0);
        state.place.y = y - i32::from(state.hot_spot.1);
        let (pipe, place, picture) = (state.pipe, state.place, state.picture);
        // The pointer that shows moves now. One that a waiting change
        // brings moves with that change.
        let mine = self.holders[holder].picture_ref(picture);
        let shows = match (mine, &self.shown_pictures[at]) {
            (Some(mine), Some(shown)) => same::<R::Target>(mine, shown),
            _ => false,
        };
        if !shows || self.shown.planes[at].pipe != pipe {
            return Ok(());
        }
        let Some(pipe_place) = self.pipes.iter().position(|other| other.id == pipe) else {
            return Ok(());
        };
        let pipe = self.pipes[pipe_place];
        let Some(card) = self.card(pipe.card).cloned() else { return Ok(()) };
        (self.shown.planes[at].place.x, self.shown.planes[at].place.y) = (place.x, place.y);
        let change = Change { mode: false, main: false, pointer: false, moved: true, damage: &[] };
        card.show(&self.shown_frame(&pipe), &change).map_err(|_| File::Io.status(0))
    }

    /// PUT: writes `rows` into `rect` of `picture`, which its card keeps,
    /// for holder `me`, which is in front. The screens of the pipe whose
    /// main plane shows the picture show the rows at once.
    ///
    /// # Errors
    ///
    /// busy for a holder that is not in front.
    pub fn put(&self, me: u32, picture: &R, rect: Rect, rows: &[u8]) -> Result<(), Status> {
        if self.stack.front() != Some(me) {
            return Err(busy());
        }
        let shows = |(at, shown): (usize, &Option<R>)| {
            let this = shown.as_deref().is_some_and(|shown| same(shown, picture));
            (this && self.planes[at].kind == PLANE_MAIN).then_some(at)
        };
        let on = self.shown_pictures.iter().enumerate().find_map(shows).and_then(|at| {
            let pipe = self.pipes.iter().find(|pipe| pipe.id == self.shown.planes[at].pipe)?;
            Some(self.shown_frame(pipe))
        });
        if let Some(card) = self.card(picture.shape().card) {
            card.put(picture, rect, rows, on.as_ref());
        }
        Ok(())
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
            Refused::NotFront => busy(),
            Refused::Stacked => invalid(),
            Refused::Full => Kernel::TooMany.status(0),
        })?;
        self.pass(passed, false);
        Ok(())
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

    /// The number of the holder its opener knows by `key`, made a holder
    /// now if it is not one yet. `attention` says it is a description of
    /// that file, which one holder has at a time.
    ///
    /// # Errors
    ///
    /// busy for a second holder of `attention`, too_many at
    /// [`HOLDERS_MAX`], and no_memory.
    pub fn holder_of(&mut self, key: usize, attention: bool) -> Result<u32, Status> {
        if let Some(id) = self.holder_by_key(key) {
            return Ok(id);
        }
        if attention && self.attention != 0 {
            return Err(busy());
        }
        if self.holders.len() >= HOLDERS_MAX {
            return Err(Kernel::TooMany.status(0));
        }
        self.holders.try_reserve(1).map_err(|_| no_memory())?;
        // The next number no holder has: never 0, which is no holder.
        let id = loop {
            self.last_holder = self.last_holder.wrapping_add(1).max(1);
            if self.holder_place(self.last_holder).is_none() {
                break self.last_holder;
            }
        };
        // Room was reserved.
        let _ = self.holders.push_within_capacity(Holder::new(id, key, self.alloc));
        if attention {
            self.attention = id;
        }
        Ok(id)
    }

    /// The number of the holder its opener knows by `key`, if it is one.
    pub fn holder_by_key(&self, key: usize) -> Option<u32> {
        self.holders.iter().find(|holder| holder.key == key).map(|holder| holder.id)
    }

    /// The holder in front, or `None` while the kernel's log is.
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
