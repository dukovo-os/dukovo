// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pictures the test puts on the screens, and the changes that put
//! them there.
//!
//! A [`Stage`] is a holder with what it shows: for each screen it feeds,
//! the pipe, the mode and a [`Canvas`] on the pipe's main plane. Every
//! picture is a test pattern of a seed no other picture has, and each
//! time a screen is given one the log is told ([`display_pattern::said`]),
//! so that the host knows what to look for.

use std::time::Duration;

use display::Unmade;
use display::abi::{
    Buffer, CARD_FIRMWARE, CARD_SHARED_PAGES, FORMAT_ARGB8888, FORMAT_XRGB8888, KIND_PIPE,
    KIND_SCREEN, Mode, PIPE_ACTIVE, PIPE_MODE, PLANE_CHANGED, PLANE_HOT_SPOT, PLANE_MAIN,
    PLANE_PICTURE, PLANE_PIPE, PLANE_POINTER, Picture, SCREEN_PIPE,
};
use display::wire::{self, ChangeBytes, PutBytes};
use display_pattern::pattern;
use display_pattern::said::{Rect, Said};

use crate::rig::{Found, Outcome, Seat, say};
use crate::sys::{self, Mapping};

/// How many rows of a kept picture one PUT sends.
const PUT_ROWS: u32 = 64;

/// A picture of the holder's, and its pixels if the program draws in
/// them itself.
#[derive(Debug)]
pub struct Canvas {
    /// The picture's number.
    pub picture: u32,
    /// Its width.
    pub width: u32,
    /// Its height.
    pub height: u32,
    /// The bytes from one of its rows to the next.
    row_bytes: usize,
    /// Its pixels: the pages of its memory file. `None` for a picture
    /// the card keeps, which PUT writes.
    pixels: Option<Mapping>,
}

impl Canvas {
    /// A picture of `format` in a memory file of its own, sized as
    /// `card` wants it.
    fn in_memory(
        seat: &Seat,
        card: u32,
        format: u32,
        (width, height): (u32, u32),
    ) -> Outcome<Canvas> {
        let failed = |what: &str, err: std::io::Error| format!("{what} of {width}x{height}: {err}");
        let file = sys::memory_file().map_err(|err| failed("a memory file", err))?;
        let slot = sys::slot(std::os::fd::AsRawFd::as_raw_fd(&file));
        let asked = Buffer { file: slot, card, format, width, height, ..Buffer::default() };
        let (row_bytes, bytes) =
            seat.holder.make_buffer(&asked).map_err(|err| failed("MAKE_BUFFER", err))?;
        let pixels = Mapping::of(&file, bytes as usize).map_err(|err| failed("mapping", err))?;
        let described =
            Picture { file: slot, card, format, width, height, row_bytes, ..Picture::default() };
        let picture =
            seat.holder.make_picture(&described).map_err(|err| failed("MAKE_PICTURE", err))?;
        // The picture holds the file's pages from here on, and the
        // mapping maps them: the description is no longer needed.
        Ok(Canvas { picture, width, height, row_bytes: row_bytes as usize, pixels: Some(pixels) })
    }

    /// A picture `card` keeps.
    fn kept(seat: &Seat, card: u32, (width, height): (u32, u32)) -> Outcome<Canvas> {
        let picture = seat.holder.keep_picture(card, width, height);
        let picture =
            picture.map_err(|err| format!("a kept picture of {width}x{height}: {err}"))?;
        Ok(Canvas { picture, width, height, row_bytes: width as usize * 4, pixels: None })
    }

    /// Draws `rect` of the pattern of `seed` at its own place of the
    /// picture: in the program's memory, or with PUT.
    pub fn draw(&mut self, seat: &Seat, seed: u32, rect: Rect) -> Outcome {
        let Some(pixels) = &mut self.pixels else {
            return self.put(seat, seed, rect);
        };
        let from = rect.y as usize * self.row_bytes + rect.x as usize * 4;
        let to = (rect.y + rect.height) as usize * self.row_bytes;
        let rows = &mut pixels.bytes()[from..to];
        pattern::paint(seed, (rect.x, rect.y), rect.width, self.row_bytes, rows);
        Ok(())
    }

    /// Sends `rect` of the pattern of `seed` with PUT, a band of rows at
    /// a time.
    fn put(&self, seat: &Seat, seed: u32, rect: Rect) -> Outcome {
        let mut put = PutBytes::new();
        let mut done = 0;
        while done < rect.height {
            let rows = PUT_ROWS.min(rect.height - done);
            let (x, y) = (rect.x, rect.y + done);
            let band = put.rect(x as u16, y as u16, rect.width as u16, rows as u16);
            let band = band.ok_or("a PUT has no room for a band of rows")?;
            pattern::paint(seed, (x, y), rect.width, rect.width as usize * 4, band);
            seat.holder.put(self.picture, &mut put).map_err(|err| format!("PUT: {err}"))?;
            put.clear();
            done += rows;
        }
        Ok(())
    }

    /// All of it, as a rectangle.
    pub fn all(&self) -> Rect {
        Rect { x: 0, y: 0, width: self.width, height: self.height }
    }

    /// Makes every pixel solid, as a pointer's picture must be for the
    /// pattern to show as it is.
    fn make_solid(&mut self) {
        if let Some(pixels) = &mut self.pixels {
            for pixel in pixels.bytes().as_chunks_mut::<4>().0 {
                pixel[3] = 0xff;
            }
        }
    }
}

/// What a screen is to show: asked of [`Stage::arrange`].
#[derive(Clone, Debug)]
pub struct Plan {
    /// The screen's name.
    pub screen: String,
    /// The mode to show it in.
    pub mode: Mode,
    /// Whether its picture is one the card keeps.
    pub kept: bool,
}

/// One screen the stage feeds.
#[derive(Debug)]
pub struct Shown {
    /// The screen's name.
    pub name: String,
    /// The screen.
    pub screen: u32,
    /// Its card.
    pub card: u32,
    /// The pipe that feeds it.
    pub pipe: u32,
    /// The pipe's main plane.
    pub plane: u32,
    /// The mode.
    pub mode: Mode,
    /// The mode as a value of the holder's.
    mode_value: u32,
    /// The picture on the main plane.
    pub canvas: Canvas,
    /// The seed of the pattern all of the picture last was.
    pub seed: u32,
}

/// A pointer the stage put on a screen.
#[derive(Debug)]
pub struct Pointer {
    /// The place in [`Stage::shown`] of the screen it is on.
    pub on: usize,
    /// Its plane.
    pub plane: u32,
    /// Its picture, as many pixels wide as high.
    pub canvas: Canvas,
    /// The seed of its picture's pattern.
    seed: u32,
    /// The point of its picture that is placed.
    spot: (u16, u16),
    /// Where that point is on the screen.
    pub at: (i16, i16),
}

/// A holder and what it shows.
#[derive(Debug)]
pub struct Stage {
    /// The holder.
    pub seat: Seat,
    /// What the layer had when the stage last asked.
    pub found: Found,
    /// The screens it feeds.
    pub shown: Vec<Shown>,
    /// Pictures that are on no plane until a change of the test's own
    /// puts them there ([`Stage::spare`]).
    spares: Vec<Canvas>,
    /// The seed of the next picture.
    seed: u32,
}

/// Why a change failed, in words.
pub fn unmade(what: &str, unmade: &Unmade) -> String {
    format!("{what}: {unmade}")
}

impl Stage {
    /// A stage of `seat`, which shows nothing yet. Its pictures' seeds
    /// start at `seed`, so that two holders' pictures differ.
    pub fn of(seat: Seat, seed: u32) -> Outcome<Stage> {
        let found = Found::read(&seat.holder)?;
        Ok(Stage { seat, found, shown: Vec::new(), spares: Vec::new(), seed })
    }

    /// Asks the layer again what it has.
    pub fn look(&mut self) -> Outcome {
        self.found = Found::read(&self.seat.holder)?;
        Ok(())
    }

    /// A seed no picture has had.
    fn fresh(&mut self) -> u32 {
        self.seed += 1;
        self.seed
    }

    /// A plan for every screen with a monitor: the mode it prefers, and
    /// a picture in the program's memory.
    pub fn plans(&self) -> Vec<Plan> {
        let plugged = self.found.screens.iter().filter(|screen| screen.plugged());
        plugged
            .filter_map(|screen| {
                let mode = *screen.modes.first()?;
                Some(Plan { screen: screen.name.clone(), mode, kept: false })
            })
            .collect()
    }

    /// The picture and the places for one plan.
    fn prepare(&self, plan: &Plan) -> Outcome<Shown> {
        let screen = self.found.screen(&plan.screen)?;
        let (card, pipe) = (screen.thing.card, self.found.pipe_of(screen)?);
        let plane = self.found.plane_of(pipe, PLANE_MAIN)?;
        let size = (plan.mode.width, plan.mode.height);
        let canvas = if plan.kept {
            Canvas::kept(&self.seat, card, size)?
        } else {
            Canvas::in_memory(&self.seat, card, FORMAT_XRGB8888, size)?
        };
        let mode_value = self.seat.holder.make_mode(&plan.mode);
        let mode_value = mode_value.map_err(|err| format!("MAKE_BLOB of a mode: {err}"))?;
        let (name, screen, mode) = (plan.screen.clone(), screen.thing.id, plan.mode);
        Ok(Shown { name, screen, card, pipe, plane, mode, mode_value, canvas, seed: 0 })
    }

    /// The change that shows `shown` and nothing on any other screen:
    /// it names every screen, pipe and main plane the layer has.
    fn change_to(&self, shown: &[Shown], flags: u32) -> ChangeBytes {
        let mut change = ChangeBytes::new(flags, 0);
        for thing in self.found.of_kind(KIND_SCREEN) {
            let fed = shown.iter().find(|one| one.screen == thing.id);
            change.set(thing.id, SCREEN_PIPE, fed.map_or(0, |one| u64::from(one.pipe)));
        }
        for thing in self.found.of_kind(KIND_PIPE) {
            match shown.iter().find(|one| one.pipe == thing.id) {
                Some(one) => {
                    change.set(thing.id, PIPE_MODE, u64::from(one.mode_value));
                    change.set(thing.id, PIPE_ACTIVE, 1);
                }
                None => change.set(thing.id, PIPE_ACTIVE, 0),
            }
        }
        for plane in self.found.main_planes() {
            match shown.iter().find(|one| one.plane == plane.id) {
                Some(one) => {
                    change.set(plane.id, PLANE_PIPE, u64::from(one.pipe));
                    change.set(plane.id, PLANE_PICTURE, u64::from(one.canvas.picture));
                }
                None => change.set(plane.id, PLANE_PICTURE, 0),
            }
        }
        change
    }

    /// Lets go of the pictures and modes of `shown`.
    fn let_go(&self, shown: Vec<Shown>) {
        for one in shown {
            let _ = self.seat.holder.drop_object(one.canvas.picture);
            let _ = self.seat.holder.drop_object(one.mode_value);
        }
    }

    /// Shows what `plans` ask, each screen a new pattern, with a change
    /// of `flags`, and nothing on a screen no plan names. A change that
    /// only checks (CHECK_ONLY) or fails leaves the stage as it was.
    ///
    /// # Errors
    ///
    /// The change's refusal or failure; anything else that fails, as a
    /// failed request with the reason in words.
    pub fn arrange(&mut self, plans: &[Plan], flags: u32) -> Result<(), Unmade> {
        let other = |why: String| Unmade::Failed(std::io::Error::other(why));
        let mut new = Vec::new();
        for plan in plans {
            new.push(self.prepare(plan).map_err(other)?);
        }
        for one in &mut new {
            one.seed = self.fresh();
            // A kept picture's rows are sent once it is on its plane.
            if one.canvas.pixels.is_some() {
                one.canvas.draw(&self.seat, one.seed, one.canvas.all()).map_err(other)?;
            }
        }
        let made = self.seat.holder.change(&mut self.change_to(&new, flags));
        if made.is_err() || flags & display::abi::CHECK_ONLY != 0 {
            self.let_go(new);
            return made;
        }
        for one in &mut new {
            if one.canvas.pixels.is_none() {
                one.canvas.draw(&self.seat, one.seed, one.canvas.all()).map_err(other)?;
            }
        }
        let old = std::mem::replace(&mut self.shown, new);
        self.say_shown();
        for gone in old.iter().filter(|old| self.shown.iter().all(|one| one.name != old.name)) {
            say(&Said::Dark { screen: gone.name.clone() });
        }
        self.let_go(old);
        Ok(())
    }

    /// Says in the log what each screen the stage feeds shows.
    pub fn say_shown(&self) {
        for one in &self.shown {
            let (width, height) = (one.canvas.width, one.canvas.height);
            say(&Said::Shows { screen: one.name.clone(), width, height, seed: one.seed });
        }
    }

    /// The plans that ask for what the stage shows now.
    pub fn as_shown(&self) -> Vec<Plan> {
        let plan = |one: &Shown| Plan {
            screen: one.name.clone(),
            mode: one.mode,
            kept: one.canvas.pixels.is_none(),
        };
        self.shown.iter().map(plan).collect()
    }

    /// Shows every screen with a monitor, as [`Stage::plans`] has them.
    pub fn show_all(&mut self, flags: u32) -> Outcome {
        let plans = self.plans();
        self.arrange(&plans, flags).map_err(|err| unmade("showing every screen", &err))
    }

    /// Another picture for the screen at place `at`: as large as the
    /// one it shows, in the program's memory, with a pattern of its own.
    /// It is on no plane, and lives as long as the stage.
    pub fn spare(&mut self, at: usize) -> Outcome<u32> {
        let seed = self.fresh();
        let one = &self.shown[at];
        let size = (one.canvas.width, one.canvas.height);
        let mut canvas = Canvas::in_memory(&self.seat, one.card, FORMAT_XRGB8888, size)?;
        canvas.draw(&self.seat, seed, canvas.all())?;
        let picture = canvas.picture;
        self.spares.push(canvas);
        Ok(picture)
    }

    /// Draws a new pattern in every picture and shows them all with one
    /// change of `flags` that names each plane's picture again.
    pub fn repaint(&mut self, flags: u32) -> Result<(), Unmade> {
        let mut change = ChangeBytes::new(flags, 0);
        for at in 0..self.shown.len() {
            let seed = self.fresh();
            let one = &mut self.shown[at];
            let drawn = one.canvas.draw(&self.seat, seed, one.canvas.all());
            drawn.map_err(|why| Unmade::Failed(std::io::Error::other(why)))?;
            one.seed = seed;
            change.set(one.plane, PLANE_PICTURE, u64::from(one.canvas.picture));
        }
        self.seat.holder.change(&mut change)?;
        self.say_shown();
        Ok(())
    }

    /// Sends a new pattern for `rect` of the picture of the screen at
    /// place `at`, which its card keeps, with PUT: it shows at once.
    pub fn put_part(&mut self, at: usize, rect: Rect) -> Outcome {
        let seed = self.fresh();
        let one = &mut self.shown[at];
        if one.canvas.pixels.is_some() {
            return Err(format!("the picture of {} is not one its card keeps", one.name));
        }
        one.canvas.draw(&self.seat, seed, rect)?;
        say(&Said::Patch { screen: one.name.clone(), rect, seed });
        Ok(())
    }

    /// Tells the layer, with a change of `flags` and `cookie`, that a
    /// few pixels of the picture of the screen at place `at` changed.
    /// Nothing is drawn, so the screen shows what it did.
    pub fn touch(&self, at: usize, flags: u32, cookie: u64) -> Result<(), Unmade> {
        let mut change = ChangeBytes::new(flags, cookie);
        change.set(self.shown[at].plane, PLANE_CHANGED, wire::rect(0, 0, 8, 8));
        self.seat.holder.change(&mut change)
    }

    /// Draws a new pattern over all of the picture of the screen at
    /// place `at` of [`Stage::shown`], and tells the layer with a change
    /// of `flags` and `cookie` that only `rect` of it changed. Returns
    /// what the screen then shows: the new pattern in `rect` alone, or
    /// everywhere if the host shows the program's own pages.
    pub fn patch(
        &mut self,
        at: usize,
        rect: Rect,
        flags: u32,
        cookie: u64,
    ) -> Result<Said, Unmade> {
        let seed = self.fresh();
        let one = &mut self.shown[at];
        let drawn = one.canvas.draw(&self.seat, seed, one.canvas.all());
        drawn.map_err(|why| Unmade::Failed(std::io::Error::other(why)))?;
        let mut change = ChangeBytes::new(flags, cookie);
        let packed =
            wire::rect(rect.x as i16, rect.y as i16, rect.width as u16, rect.height as u16);
        change.set(one.plane, PLANE_CHANGED, packed);
        self.seat.holder.change(&mut change)?;
        let shared =
            self.found.card(one.card).is_ok_and(|card| card.flags & CARD_SHARED_PAGES != 0);
        let screen = one.name.clone();
        Ok(if shared {
            Said::Shows { screen, width: one.canvas.width, height: one.canvas.height, seed }
        } else {
            Said::Patch { screen, rect, seed }
        })
    }

    /// Whether the card of the screen at place `at` draws a pointer
    /// into its frames itself, so that a picture of the screen has it.
    /// Another card's host draws it over what the screen shows.
    fn draws_pointer(&self, at: usize) -> bool {
        self.found.card(self.shown[at].card).is_ok_and(|card| card.kind == CARD_FIRMWARE)
    }

    /// Puts a pointer of `size` pixels a side on the screen at place
    /// `on`, with the point `spot` of its picture at `at` of the screen,
    /// and says so in the log if a picture of the screen has it.
    pub fn point(
        &mut self,
        on: usize,
        size: u32,
        spot: (u16, u16),
        at: (i16, i16),
    ) -> Outcome<Pointer> {
        let seed = self.fresh();
        let one = &self.shown[on];
        let plane = self.found.plane_of(one.pipe, PLANE_POINTER)?;
        let mut canvas = Canvas::in_memory(&self.seat, one.card, FORMAT_ARGB8888, (size, size))?;
        canvas.draw(&self.seat, seed, canvas.all())?;
        canvas.make_solid();
        let mut change = ChangeBytes::new(0, 0);
        change.set(plane, PLANE_PIPE, u64::from(one.pipe));
        change.set(plane, PLANE_PICTURE, u64::from(canvas.picture));
        change.set(plane, PLANE_HOT_SPOT, u64::from(spot.0) | u64::from(spot.1) << 16);
        let made = self.seat.holder.change(&mut change);
        made.map_err(|err| unmade("putting a picture on the pointer's plane", &err))?;
        // The pointer shows at the screen's corner until it is moved,
        // and what it covered there is the screen's own picture again.
        let moved = self.seat.holder.move_pointer(plane, at.0, at.1);
        moved.map_err(|err| format!("MOVE_POINTER: {err}"))?;
        let pointer = Pointer { on, plane, canvas, seed, spot, at };
        if self.draws_pointer(on) {
            let screen = self.shown[on].name.clone();
            say(&Said::Pointer { screen, rect: Stage::under(&pointer), seed });
        }
        Ok(pointer)
    }

    /// The rectangle of its screen that `pointer` covers.
    fn under(pointer: &Pointer) -> Rect {
        let x = i32::from(pointer.at.0) - i32::from(pointer.spot.0);
        let y = i32::from(pointer.at.1) - i32::from(pointer.spot.1);
        let size = pointer.canvas.width;
        Rect { x: x as u32, y: y as u32, width: size, height: size }
    }

    /// Moves `pointer` to `to` of its screen with MOVE_POINTER, and says
    /// in the log what a picture of the screen then has: the screen's
    /// own picture where the pointer was, and the pointer where it is.
    pub fn move_pointer(&self, pointer: &mut Pointer, to: (i16, i16)) -> Outcome {
        let moved = self.seat.holder.move_pointer(pointer.plane, to.0, to.1);
        moved.map_err(|err| format!("MOVE_POINTER: {err}"))?;
        let was = Stage::under(pointer);
        pointer.at = to;
        if self.draws_pointer(pointer.on) {
            let one = &self.shown[pointer.on];
            say(&Said::Patch { screen: one.name.clone(), rect: was, seed: one.seed });
            let (rect, seed) = (Stage::under(pointer), pointer.seed);
            say(&Said::Pointer { screen: one.name.clone(), rect, seed });
        }
        Ok(())
    }

    /// Waits until the layer says a screen came, went or changed, and
    /// the screens are as `settled` wants them: the host's monitors come
    /// and go one at a time.
    pub fn until_screens(&mut self, what: &str, settled: impl Fn(&Found) -> bool) -> Outcome {
        const PATIENCE: Duration = Duration::from_secs(60);
        let until = std::time::Instant::now() + PATIENCE;
        loop {
            self.look()?;
            if settled(&self.found) {
                return Ok(());
            }
            let left = until.saturating_duration_since(std::time::Instant::now());
            let screen =
                |event: &display::abi::DisplayEvent| event.kind == display::abi::EVENT_SCREEN;
            self.seat
                .event("EVENT_SCREEN", left, screen)
                .map_err(|why| format!("{what}: {why}"))?;
        }
    }
}
