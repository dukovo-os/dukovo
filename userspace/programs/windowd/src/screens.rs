// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens the desktop shows on, and the frames it draws on them
//! (ADR 0714).
//!
//! The desktop shows on every screen a monitor is plugged into, each in
//! the mode it prefers, side by side from left to right in the layer's
//! order ([`display::outputs`]). A screen has pictures of its mode's
//! size that windowd draws its frames in with pixman: one, which the
//! card copies from when a change says what changed, or two that take
//! turns on a card that reads the program's pages at any moment
//! (`CARD_SHARED_PAGES`), so that such a card never reads a frame that
//! is half drawn.
//!
//! A frame is drawn only for a screen whose content changed, only where
//! it changed, and no more often than the screen's beat ([`pace`]). The
//! change that shows it does not wait: the layer says with an event
//! when it shows, and windowd then tells the programs on that screen
//! that a frame was shown, which is when they draw their next.
//!
//! windowd asks for no frame while the one before it waits for its
//! beat, but for one: when it lost sight of what a screen shows, as
//! when the desktop comes to the front or screens come and go, it draws
//! the screen whole at once. That frame takes the place of one that
//! still waits (`NEWEST_WINS`).

pub mod pace;

use std::io;

use display::abi::{
    CARD_SHARED_PAGES, DisplayEvent, EVENT_BACK, EVENT_BEAT, EVENT_FAILED, EVENT_FRONT,
    EVENT_OVERFLOW, EVENT_SCREEN, EVENT_SHOWN, FORMAT_ARGB8888, FORMAT_XRGB8888, NEWEST_WINS,
    NO_WAIT, PLANE_CHANGED, PLANE_HOT_SPOT, PLANE_PICTURE, PLANE_PIPE, PLANE_POINTER,
};
use display::outputs::{self, Found};
use display::wire::{self, ChangeBytes};
use display::{Holder, Unmade};
use smithay::backend::renderer::damage::{Error as DamageError, OutputDamageTracker};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::pixman::{PixmanError, PixmanRenderer};
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Bind, Color32F, Frame, Renderer};
use smithay::desktop::space::render_output;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};
use sysapi::time::monotonic_ns;

use crate::link::Link;
use crate::pointer::{self, SIDE};
use crate::shell::Shell;
use pace::Pace;

/// The colour of the desktop where no window is, a dark blue grey: red,
/// green and blue.
pub const BACKGROUND: [u8; 3] = [0x12, 0x16, 0x1e];

/// [`BACKGROUND`] as the renderer takes a colour.
fn background() -> Color32F {
    let [red, green, blue] = BACKGROUND.map(|part| f32::from(part) / 255.0);
    Color32F::new(red, green, blue, 1.0)
}

/// How many rectangles of a frame a change names at most. A frame that
/// changed in more places names the one rectangle around them all.
const RECTS_MAX: usize = 32;

/// The pictures a plane's frames are drawn in, in turn.
struct Pictures<L: Link> {
    /// Each picture's number and its pixels.
    made: Vec<(u32, L::Pixels)>,
    /// Which is drawn in next.
    next: usize,
    /// How many of them hold a frame drawn since the last was lost
    /// sight of.
    drawn: usize,
}

impl<L: Link> Pictures<L> {
    /// Makes the pictures of a plane of `card`: two for a card that
    /// reads the pages at any moment, and otherwise one.
    fn make(holder: &Holder<L>, card: u32, format: u32, size: (u32, u32)) -> io::Result<Self> {
        let shared = holder.card(card)?.flags & CARD_SHARED_PAGES != 0;
        let count = if shared { 2 } else { 1 };
        let made = (0..count).map(|_| L::pixels(holder, card, format, size));
        Ok(Pictures { made: made.collect::<io::Result<_>>()?, next: 0, drawn: 0 })
    }

    /// How many frames ago the picture drawn in next was drawn in, or 0
    /// when what it holds is not known.
    fn age(&self) -> usize {
        if self.drawn == self.made.len() { self.made.len() } else { 0 }
    }

    /// The picture to draw in next: its number and its pixels.
    fn target(&mut self) -> (u32, &mut L::Pixels) {
        let (picture, pixels) = &mut self.made[self.next];
        (*picture, pixels)
    }

    /// The picture drawn in next was drawn in and is shown.
    fn drew(&mut self) {
        self.next = (self.next + 1) % self.made.len();
        self.drawn = (self.drawn + 1).min(self.made.len());
    }

    /// A frame may not have shown: every picture is drawn anew.
    fn forget(&mut self) {
        self.drawn = 0;
    }

    /// Their numbers.
    fn numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.made.iter().map(|(picture, _)| *picture)
    }
}

/// A screen's pointer plane, and what it shows.
struct PointerPlane<L: Link> {
    plane: u32,
    pictures: Pictures<L>,
    /// Which of the pointer's pictures the layer was last asked to show
    /// here ([`Pointing::picture`]), or `None` for no pointer.
    asked: Option<u64>,
}

/// One screen the desktop shows on.
struct Screen<L: Link> {
    /// The layer's screen, pipe, plane and mode.
    on: outputs::Output,
    /// What the programs are told of it.
    output: Output,
    /// Its place among what the programs are offered.
    global: GlobalId,
    /// The mode, as a value of the holder's.
    mode_value: u32,
    pictures: Pictures<L>,
    /// What changed between its frames.
    tracker: OutputDamageTracker,
    pace: Pace,
    pointer: Option<PointerPlane<L>>,
}

/// Where the pointer is and what it shows, for the screens' frames.
#[derive(Debug)]
pub struct Pointing<'a> {
    /// Where it is on the desktop.
    pub at: Point<f64, Logical>,
    /// What it shows.
    pub shown: &'a CursorImageStatus,
    /// Counts the pictures the pointer has shown: another number is
    /// another picture.
    pub picture: u64,
    /// windowd's own arrow.
    pub arrow: &'a MemoryRenderBuffer,
}

/// What an event of the layer's means for the desktop.
#[derive(Debug, PartialEq)]
pub enum Told {
    /// Nothing more than the screens took from it.
    Nothing,
    /// A frame shows on this screen, at this time on the monotonic
    /// clock: the programs on it draw their next.
    Shown(Output, u64),
    /// The desktop is in front again.
    Front,
    /// Screens came or went: [`Screens::arrange`] is due.
    Rearrange,
}

/// Why the screens could not be drawn on.
#[derive(Debug)]
pub enum Failed {
    /// A request of the layer's failed.
    Layer(io::Error),
    /// A change was refused: the entry and the reason.
    Refused(u32, u32),
    /// The renderer could not draw.
    Renderer(String),
    /// The protocols could not be set up.
    Start(String),
}

impl core::fmt::Display for Failed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Failed::Layer(error) => write!(f, "the display layer: {error}"),
            Failed::Refused(entry, reason) => {
                write!(f, "the display layer refused entry {entry} of a change for reason {reason}")
            }
            Failed::Renderer(error) => write!(f, "drawing: {error}"),
            Failed::Start(error) => f.write_str(error),
        }
    }
}

impl From<io::Error> for Failed {
    fn from(error: io::Error) -> Failed {
        Failed::Layer(error)
    }
}

impl From<PixmanError> for Failed {
    fn from(error: PixmanError) -> Failed {
        Failed::Renderer(error.to_string())
    }
}

impl From<DamageError<PixmanError>> for Failed {
    fn from(error: DamageError<PixmanError>) -> Failed {
        Failed::Renderer(error.to_string())
    }
}

/// Whether `made` failed because the holder is not in front, which a
/// holder learns from an event it may not have read yet.
fn in_back(made: &Result<(), Unmade>) -> bool {
    matches!(made, Err(Unmade::Failed(error)) if error.raw_os_error() == Some(libc::EBUSY))
}

/// A change's failure, as the screens'.
fn unmade(unmade: Unmade) -> Failed {
    match unmade {
        Unmade::Refused { entry, reason } => Failed::Refused(entry, reason),
        Unmade::Failed(error) => Failed::Layer(error),
    }
}

/// The screens.
pub struct Screens<L: Link> {
    holder: Holder<L>,
    found: Found,
    shown: Vec<Screen<L>>,
    /// Whether the desktop is in front: only then is a frame drawn.
    front: bool,
    /// The cookie of the last change.
    cookie: u64,
    /// Which screen the pointer was last put on.
    pointed: Option<usize>,
}

impl<L: Link> Screens<L>
where
    PixmanRenderer: Bind<L::Pixels>,
{
    /// The screens `holder`, which is in front, reaches. None shows
    /// until [`Screens::arrange`].
    pub fn new(holder: Holder<L>) -> Screens<L> {
        Screens {
            holder,
            found: Found::default(),
            shown: Vec::new(),
            front: true,
            cookie: 0,
            pointed: None,
        }
    }

    /// The holder.
    pub fn holder(&self) -> &Holder<L> {
        &self.holder
    }

    /// Whether the desktop is in front.
    #[cfg(test)]
    pub fn front(&self) -> bool {
        self.front
    }

    /// Reads which screens there are, shows on each that is new or
    /// changed its mode, lets go of those that went, and draws each a
    /// first frame.
    ///
    /// # Errors
    ///
    /// What the layer's requests fail with, and what the renderer does.
    pub fn arrange(
        &mut self,
        renderer: &mut PixmanRenderer,
        shell: &mut Shell,
        display: &DisplayHandle,
    ) -> Result<(), Failed> {
        self.found = Found::read(&self.holder)?;
        let wanted = outputs::outputs(&self.found);
        let mut before = core::mem::take(&mut self.shown);
        let mut left = 0;
        for on in wanted {
            let kept = before.iter().position(|screen| screen.on == on);
            let screen = match kept {
                Some(at) => before.remove(at),
                None => self.screen(on, display)?,
            };
            let mode = screen.output.current_mode();
            screen.output.change_current_state(mode, None, None, Some((left, 0).into()));
            shell.space.map_output(&screen.output, (left, 0));
            left += on.mode.width as i32;
            self.shown.push(screen);
        }
        for gone in &before {
            shell.space.unmap_output(&gone.output);
            display.remove_global::<Shell>(gone.global.clone());
        }
        self.pointed = None;
        self.first_frames(renderer, shell)?;
        // The change above stopped showing them.
        for gone in before {
            let pointer = gone.pointer.iter().flat_map(|pointer| pointer.pictures.numbers());
            for number in gone.pictures.numbers().chain(pointer).chain([gone.mode_value]) {
                self.holder.drop_object(number)?;
            }
        }
        Ok(())
    }

    /// A screen for `on`: its pictures, its pointer plane's, and what
    /// the programs are told of it.
    fn screen(&self, on: outputs::Output, display: &DisplayHandle) -> io::Result<Screen<L>> {
        let (holder, found) = (&self.holder, &self.found);
        let size = (on.mode.width, on.mode.height);
        let pictures = Pictures::make(holder, on.card, FORMAT_XRGB8888, size)?;
        let takes = |plane: &&display::abi::PlaneInfo| {
            plane.kind == PLANE_POINTER
                && plane.card == on.card
                && (plane.fixed_pipe == on.pipe || plane.fixed_pipe == 0)
                && plane.formats.contains(&FORMAT_ARGB8888)
                && plane.width_max >= SIDE
                && plane.height_max >= SIDE
        };
        let pointer = match found.planes.iter().find(takes) {
            Some(plane) => Some(PointerPlane {
                plane: plane.id,
                pictures: Pictures::make(holder, on.card, FORMAT_ARGB8888, (SIDE, SIDE))?,
                asked: None,
            }),
            None => None,
        };
        let info = found.screens.iter().find(|(info, _)| info.id == on.screen);
        let info = info.map_or_default(|(info, _)| *info);
        let thing = found.things.iter().find(|thing| thing.id == on.screen);
        let card = holder.card(on.card)?;
        let name = format!("{}/{}", wire::text(&card.name), thing.map_or(0, |thing| thing.index));
        let output = Output::new(
            name,
            PhysicalProperties {
                size: (info.width_mm as i32, info.height_mm as i32).into(),
                subpixel: Subpixel::Unknown,
                make: wire::text(&info.maker).to_owned(),
                model: wire::text(&info.name).to_owned(),
            },
        );
        let mode = Mode {
            size: (on.mode.width as i32, on.mode.height as i32).into(),
            refresh: on.mode.refresh_mhz as i32,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            None,
        );
        output.set_preferred(mode);
        Ok(Screen {
            on,
            global: output.create_global::<Shell>(display),
            tracker: OutputDamageTracker::from_output(&output),
            output,
            mode_value: holder.make_mode(&on.mode)?,
            pictures,
            pace: Pace::default(),
            pointer,
        })
    }

    /// Draws every screen a whole frame and shows them with one change,
    /// which sets the modes and may blank. No pointer shows until the
    /// frames after.
    fn first_frames(&mut self, renderer: &mut PixmanRenderer, shell: &Shell) -> Result<(), Failed> {
        let mut made = Vec::new();
        for screen in &mut self.shown {
            screen.pictures.forget();
            let (picture, _) = screen.draw(renderer, shell)?;
            screen.pictures.drew();
            made.push((picture, screen.mode_value));
        }
        let on: Vec<outputs::Output> = self.shown.iter().map(|screen| screen.on).collect();
        let mut change = outputs::change(&self.found, &on, &made);
        for plane in self.found.planes.iter().filter(|plane| plane.kind == PLANE_POINTER) {
            change.set(plane.id, PLANE_PICTURE, 0);
        }
        let made = self.holder.change(&mut change);
        for screen in &mut self.shown {
            screen.pace.reset();
            screen.pace.stale();
            if let Some(pointer) = &mut screen.pointer {
                pointer.asked = None;
            }
        }
        if in_back(&made) {
            self.front = false;
            return Ok(());
        }
        made.map_err(unmade)
    }

    /// What `event` of the layer's means: the screens take from it what
    /// is theirs.
    pub fn told(&mut self, event: &DisplayEvent) -> Told {
        let pipe = event.thing;
        match event.kind {
            EVENT_SHOWN | EVENT_BEAT => {
                let Some(screen) = self.of_pipe(pipe) else { return Told::Nothing };
                screen.pace.shown(event.deadline_ns);
                Told::Shown(screen.output.clone(), event.time_ns)
            }
            EVENT_FAILED => {
                if let Some(screen) = self.of_pipe(pipe) {
                    screen.lost();
                }
                Told::Nothing
            }
            EVENT_BACK => {
                self.front = false;
                self.shown.iter_mut().for_each(|screen| screen.pace.reset());
                Told::Nothing
            }
            EVENT_FRONT => {
                self.front = true;
                self.shown.iter_mut().for_each(Screen::lost);
                Told::Front
            }
            EVENT_SCREEN | EVENT_OVERFLOW => Told::Rearrange,
            _ => Told::Nothing,
        }
    }

    /// The screen `pipe` feeds.
    fn of_pipe(&mut self, pipe: u32) -> Option<&mut Screen<L>> {
        self.shown.iter_mut().find(|screen| screen.on.pipe == pipe)
    }

    /// What the screens show changed: each is due a frame.
    pub fn stale(&mut self) {
        self.shown.iter_mut().for_each(|screen| screen.pace.stale());
    }

    /// When the next frame is to be drawn, on the monotonic clock, if
    /// one is due.
    pub fn due(&self, now_ns: u64) -> Option<u64> {
        if !self.front {
            return None;
        }
        self.shown.iter().filter_map(|screen| screen.pace.due(now_ns)).min()
    }

    /// Draws a frame for each screen that is due one at `now_ns`.
    ///
    /// # Errors
    ///
    /// What the layer's requests fail with, and what the renderer does.
    pub fn draw(
        &mut self,
        renderer: &mut PixmanRenderer,
        shell: &Shell,
        pointing: &Pointing<'_>,
        now_ns: u64,
    ) -> Result<(), Failed> {
        if !self.front {
            return Ok(());
        }
        let on = self.on(pointing.at);
        for at in 0..self.shown.len() {
            if self.shown[at].pace.due(now_ns).is_none_or(|due| due > now_ns) {
                continue;
            }
            let pointer = (on == Some(at)).then_some(pointing);
            self.cookie += 1;
            let screen = &mut self.shown[at];
            let made = screen.frame(&self.holder, renderer, shell, pointer, self.cookie);
            match made {
                // The place the pointer was put at was reckoned from
                // the hot spot its picture had before.
                Ok(true) => self.point(pointing.at),
                Ok(false) => {}
                Err(Failed::Layer(error)) if error.raw_os_error() == Some(libc::EBUSY) => {
                    // The layer's event that says so is on its way.
                    self.shown[at].lost();
                    self.front = false;
                    return Ok(());
                }
                Err(failed) => {
                    // The picture is drawn in part, and shows as it was.
                    self.shown[at].lost();
                    return Err(failed);
                }
            }
        }
        Ok(())
    }

    /// Which screen the place `at` of the desktop is on.
    fn on(&self, at: Point<f64, Logical>) -> Option<usize> {
        self.shown.iter().position(|screen| screen.area().to_f64().contains(at))
    }

    /// The place nearest to `at` that is on a screen.
    pub fn within(&self, at: Point<f64, Logical>) -> Point<f64, Logical> {
        let nearest = |screen: &Screen<L>| {
            let area = screen.area().to_f64();
            let x = at.x.clamp(area.loc.x, area.loc.x + area.size.w - 1.0);
            let y = at.y.clamp(area.loc.y, area.loc.y + area.size.h - 1.0);
            Point::<f64, Logical>::from((x, y))
        };
        let far = |to: &Point<f64, Logical>| (to.x - at.x).powi(2) + (to.y - at.y).powi(2);
        let nearest =
            self.shown.iter().map(nearest).min_by(|one, other| far(one).total_cmp(&far(other)));
        nearest.unwrap_or_default()
    }

    /// The first screen's area of the desktop, which a device that
    /// points at places points into.
    pub fn first(&self) -> Option<Rectangle<i32, Logical>> {
        self.shown.first().map(Screen::area)
    }

    /// Puts the pointer at `at` of the desktop, at once. The screen it
    /// came to and the one it left are due a frame, which shows it on
    /// the one and takes it from the other.
    pub fn point(&mut self, at: Point<f64, Logical>) {
        let on = self.on(at);
        if on != self.pointed {
            for crossed in [on, self.pointed].into_iter().flatten() {
                if let Some(screen) = self.shown.get_mut(crossed) {
                    screen.pace.stale();
                }
            }
            self.pointed = on;
        }
        let Some(screen) = on.map(|on| &self.shown[on]) else { return };
        let Some(pointer) = &screen.pointer else { return };
        let within = at - screen.area().loc.to_f64();
        if self.front {
            // A holder that went to the back meanwhile moves nothing,
            // and is told so by an event.
            let _ = self.holder.move_pointer(pointer.plane, within.x as i16, within.y as i16);
        }
    }

    /// The size of the screen at place `at`, and the pixels it last
    /// showed.
    #[cfg(test)]
    pub fn pixels(&mut self, at: usize) -> ((u32, u32), &mut L::Pixels) {
        let screen = &mut self.shown[at];
        let pictures = &mut screen.pictures;
        let last = (pictures.next + pictures.made.len() - 1) % pictures.made.len();
        ((screen.on.mode.width, screen.on.mode.height), &mut pictures.made[last].1)
    }
}

impl<L: Link> Screen<L>
where
    PixmanRenderer: Bind<L::Pixels>,
{
    /// Its area of the desktop.
    fn area(&self) -> Rectangle<i32, Logical> {
        let size = Size::from((self.on.mode.width as i32, self.on.mode.height as i32));
        Rectangle::new(self.output.current_location(), size)
    }

    /// A frame may not have shown, or what shows is not known: the next
    /// frame is drawn whole and at once.
    fn lost(&mut self) {
        self.pictures.forget();
        self.pace.reset();
        self.pace.stale();
        if let Some(pointer) = &mut self.pointer {
            pointer.pictures.forget();
        }
    }

    /// Draws what the screen shows into the picture that is next.
    /// Returns the picture and the rectangles of it that changed.
    fn draw(
        &mut self,
        renderer: &mut PixmanRenderer,
        shell: &Shell,
    ) -> Result<(u32, Vec<Rectangle<i32, Physical>>), Failed> {
        let age = self.pictures.age();
        let (picture, pixels) = self.pictures.target();
        let mut target = renderer.bind(pixels)?;
        let none: &[WaylandSurfaceRenderElement<PixmanRenderer>] = &[];
        let drawn = render_output(
            &self.output,
            renderer,
            &mut target,
            1.0,
            age,
            [&shell.space],
            none,
            &mut self.tracker,
            background(),
        )?;
        Ok((picture, drawn.damage.cloned().unwrap_or_default()))
    }

    /// Draws the screen a frame, with the pointer's picture if the
    /// pointer is on it, and asks the layer to show it at the next beat
    /// without waiting. A frame in which nothing changed is not shown;
    /// the beat alone is asked for if a program on the screen waits to
    /// hear of one. Returns whether the pointer's picture is another.
    fn frame(
        &mut self,
        holder: &Holder<L>,
        renderer: &mut PixmanRenderer,
        shell: &Shell,
        pointing: Option<&Pointing<'_>>,
        cookie: u64,
    ) -> Result<bool, Failed> {
        let started = monotonic_ns();
        // A frame that still waits for its beat is one the screen was
        // lost sight of after: this one, drawn whole, takes its place,
        // so that busy says only that the desktop is not in front.
        let mut change = ChangeBytes::new(NO_WAIT | NEWEST_WINS, cookie);
        let (picture, changed) = self.draw(renderer, shell)?;
        let drew = !changed.is_empty();
        if drew {
            if self.pictures.made.len() > 1 {
                change.set(self.on.plane, PLANE_PICTURE, u64::from(picture));
            }
            for rect in few(changed) {
                let (at, size) = (rect.loc, rect.size);
                let packed = wire::rect(at.x as i16, at.y as i16, size.w as u16, size.h as u16);
                change.set(self.on.plane, PLANE_CHANGED, packed);
            }
        }
        let pointer = self.point(renderer, pointing, &mut change)?;
        if change.is_empty() {
            if !shell.owes_frame(&self.output) {
                self.pace.unchanged();
                return Ok(false);
            }
            holder.ask_beat(self.on.pipe)?;
        } else {
            holder.change(&mut change).map_err(unmade)?;
        }
        if drew {
            self.pictures.drew();
        }
        if let (Some(asked), Some(plane)) = (pointer, &mut self.pointer) {
            plane.asked = asked;
            if asked.is_some() {
                plane.pictures.drew();
            }
        }
        self.pace.drawn(monotonic_ns().saturating_sub(started));
        Ok(pointer.is_some_and(|asked| asked.is_some()))
    }

    /// Adds to `change` what makes the pointer plane show the pointer
    /// `pointing` describes, or no pointer for `None`, if it does not
    /// show that already, drawing the pointer's picture if need be.
    /// Returns what the plane is then asked to show, if that is new.
    fn point(
        &mut self,
        renderer: &mut PixmanRenderer,
        pointing: Option<&Pointing<'_>>,
        change: &mut ChangeBytes,
    ) -> Result<Option<Option<u64>>, Failed> {
        let Some(plane) = &mut self.pointer else { return Ok(None) };
        let drawn = pointing.and_then(|pointing| {
            let drawn = pointer::drawn(renderer, pointing.shown, pointing.arrow)?;
            Some((pointing.picture, drawn))
        });
        let wanted = drawn.as_ref().map(|(picture, _)| *picture);
        if wanted == plane.asked {
            return Ok(None);
        }
        let Some((_, (drawn, spot))) = drawn else {
            change.set(plane.plane, PLANE_PICTURE, 0);
            return Ok(Some(None));
        };
        let (picture, pixels) = plane.pictures.target();
        let mut target = renderer.bind(pixels)?;
        let all = Rectangle::from_size(Size::from((SIDE as i32, SIDE as i32)));
        let mut frame = renderer.render(&mut target, all.size, Transform::Normal)?;
        frame.clear(Color32F::TRANSPARENT, &[all])?;
        draw_render_elements(&mut frame, 1.0, &drawn, &[all])?;
        // pixman has drawn by the time it returns: nothing to wait for.
        let _ = frame.finish()?;
        change.set(plane.plane, PLANE_PIPE, u64::from(self.on.pipe));
        change.set(plane.plane, PLANE_PICTURE, u64::from(picture));
        change.set(plane.plane, PLANE_HOT_SPOT, u64::from(spot.0) | u64::from(spot.1) << 16);
        Ok(Some(wanted))
    }
}

/// `rects`, or the one rectangle around them when they are more than a
/// change names.
fn few(rects: Vec<Rectangle<i32, Physical>>) -> Vec<Rectangle<i32, Physical>> {
    if rects.len() <= RECTS_MAX {
        return rects;
    }
    let around = rects.iter().copied().reduce(Rectangle::merge);
    around.into_iter().collect()
}
