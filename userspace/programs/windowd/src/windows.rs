// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The windows and their frames: where each is, in which order they
//! lie, and what changes that (ADR 0720).
//!
//! Every window has windowd's frame around it (ADR 0718). A window is
//! where its frame's box is; the program's own surface lies in the box
//! as the frame's layout says, and shows cut to it, so that nothing a
//! program draws reaches over the frame.
//!
//! A window is as its program last drew it. Maximizing, restoring and
//! resizing ask the program for a size, and the frame takes the new
//! shape when the program has drawn that size, so that frame and
//! content never differ on the screen. A resize asks for one size at a
//! time ([`resize`]).
//!
//! Nothing here reaches the seat: who has the keyboard is the
//! [`Shell`](crate::shell::Shell)'s to say.

pub mod resize;

use std::convert::Infallible;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::desktop::{Space, Window, WindowSurfaceType};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData};
use window_frame::{Bar, Button, Canvas, Edge, Layout, Look, Margins, Part, Rect, Shape, Theme};

use resize::Resize;

/// How far down and to the right of the window before it a new window
/// shows.
const STEP: i32 = 32;

/// After how many windows the steps start again at the middle.
const STEPS: i32 = 8;

/// How much of a title bar's width stays on a screen when the keys move
/// a window.
const KEPT: i32 = 48;

/// What is under a place of the desktop.
#[derive(Debug, Clone, PartialEq)]
pub enum Under {
    /// No window.
    Nothing,
    /// A surface of a program's, which hears the pointer.
    Surface {
        /// The window it is of.
        window: Window,
        /// The surface.
        surface: WlSurface,
        /// Where the surface's top left corner is on the desktop.
        at: Point<f64, Logical>,
    },
    /// A part of a window that is windowd's: its frame, or a place of
    /// its content where the program hears no pointer.
    Part {
        /// The window.
        window: Window,
        /// The part.
        part: Part,
    },
}

impl Under {
    /// The window, if any.
    pub fn window(&self) -> Option<&Window> {
        match self {
            Under::Nothing => None,
            Under::Surface { window, .. } | Under::Part { window, .. } => Some(window),
        }
    }
}

/// One of the rectangles a frame is drawn in, and its pixels.
#[derive(Debug)]
pub struct Strip {
    /// Where it lies, from the corner of the frame's box.
    pub rect: Rect,
    /// Its pixels.
    pub buffer: MemoryRenderBuffer,
}

/// Where a maximized window goes when it is restored.
#[derive(Debug, Clone, Copy, Default)]
struct Restore {
    /// The corner of the frame's box, or `None` for the middle of the
    /// screen: the window was maximized when it first showed.
    at: Option<Point<i32, Logical>>,
    /// The size of the content, or `None` for the size the program
    /// chooses.
    size: Option<Size<i32, Logical>>,
}

/// What a frame's strips were last drawn to show.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    layout: Layout,
    title: String,
    active: bool,
    hot: Option<Button>,
    pressed: Option<Button>,
}

/// A window and its frame.
#[derive(Debug)]
pub struct Framed {
    /// The program's window.
    pub window: Window,
    /// Where the corner of the frame's box is on the desktop.
    at: Point<i32, Logical>,
    /// Where the parts of the frame are.
    layout: Layout,
    /// Whether the window is off the screens, in the list alone.
    minimized: bool,
    /// The window's title, as its program last gave it.
    title: String,
    /// Where a maximized window goes when it is restored.
    restore: Restore,
    /// The screen a window that asked to be maximized fills once its
    /// program has drawn it so.
    fills: Rectangle<i32, Logical>,
    /// The resize that is under way.
    resize: Option<Resize>,
    /// The frame's pixels.
    strips: Vec<Strip>,
    shown: Option<Shown>,
    /// Counts the times a window was given the keyboard: the larger,
    /// the later.
    used: u64,
}

impl Framed {
    /// Where the corner of the frame's box is on the desktop.
    pub fn at(&self) -> Point<i32, Logical> {
        self.at
    }

    /// Where the parts of the frame are.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The frame's pixels, each strip where the layout has it.
    pub fn strips(&self) -> &[Strip] {
        &self.strips
    }

    /// The window's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the window is off the screens.
    pub fn minimized(&self) -> bool {
        self.minimized
    }

    /// The frame's box on the desktop.
    pub fn frame(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.at, (self.layout.frame.w, self.layout.frame.h).into())
    }

    /// Where the top left corner of the window's own surface is on the
    /// desktop.
    pub fn surface_at(&self) -> Point<i32, Logical> {
        let within = Point::from((self.layout.surface.x, self.layout.surface.y));
        self.at + within - self.window.geometry().loc
    }

    /// The part of the desktop the window's own surface shows in.
    pub fn surface_shows(&self) -> Rectangle<i32, Logical> {
        let surface = self.layout.surface;
        Rectangle::new(self.at + Point::from((surface.x, surface.y)), (surface.w, surface.h).into())
    }

    /// Whether the program was asked to draw the window maximized, or
    /// has.
    pub fn maximized(&self) -> bool {
        let asked = self.window.toplevel().is_some_and(|toplevel| {
            toplevel.with_pending_state(|state| state.states.contains(State::Maximized))
        });
        asked || self.layout.shape == Shape::Maximized
    }

    /// The smallest and the largest size the program takes for the
    /// window's content, where 0 is no limit.
    fn limits(&self) -> (Size<i32, Logical>, Size<i32, Logical>) {
        let Some(surface) = self.window.wl_surface() else { return Default::default() };
        with_states(&surface, |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (current.min_size, current.max_size)
        })
    }

    /// The size nearest to `width` by `height`, which may be less than
    /// nothing, that the window's content can have.
    fn fitted(&self, theme: &Theme, (width, height): (i32, i32)) -> Size<i32, Logical> {
        let (least, most) = self.limits();
        let own = Layout::least(theme);
        let fit = |wanted: i32, least: i32, most: i32, own: i32| {
            let most = if most > 0 { most } else { i32::MAX };
            wanted.min(most).max(least).max(own)
        };
        Size::from((fit(width, least.w, most.w, own.0), fit(height, least.h, most.h, own.1)))
    }

    /// Draws the strips anew if they do not show the frame already as
    /// it is to look: of the window the person works in or not, with
    /// the button `hot` under the pointer and `pressed` held down.
    /// Returns whether they were drawn.
    fn draw(
        &mut self,
        theme: &Theme,
        active: bool,
        hot: Option<Button>,
        pressed: Option<Button>,
    ) -> bool {
        let look = Look { title: &self.title, active, hot, pressed };
        let before = self.shown.as_ref();
        let framed =
            before.is_some_and(|shown| shown.layout == self.layout && shown.active == active);
        let titled = before.is_some_and(|shown| {
            shown.title == self.title && shown.hot == hot && shown.pressed == pressed
        });
        if framed && titled {
            return false;
        }
        // The strip with the title bar is the first: the others show
        // neither the title nor a button.
        let top_alone = framed;
        let rects = self.layout.strips();
        self.strips.truncate(rects.len());
        for (at, rect) in rects.into_iter().enumerate() {
            if at == self.strips.len() {
                let buffer = MemoryRenderBuffer::new(
                    Fourcc::Argb8888,
                    (rect.w, rect.h),
                    1,
                    Transform::Normal,
                    None,
                );
                self.strips.push(Strip { rect, buffer });
            } else if top_alone && at > 0 {
                continue;
            }
            let strip = &mut self.strips[at];
            strip.rect = rect;
            let mut context = strip.buffer.render();
            context.resize((rect.w, rect.h));
            let drawn = context.draw(|pixels| {
                window_frame::draw::frame(
                    theme,
                    &self.layout,
                    &look,
                    &mut Canvas::over(rect, pixels),
                );
                Ok::<_, Infallible>(vec![Rectangle::from_size((rect.w, rect.h).into())])
            });
            let Ok(()) = drawn;
        }
        self.shown =
            Some(Shown { layout: self.layout, title: self.title.clone(), active, hot, pressed });
        true
    }
}

/// The size of the own pixels of a window of `shape` whose frame's box
/// is `frame`: a pixel each way at the least.
fn inside(theme: &Theme, shape: Shape, bar: Bar, frame: Size<i32, Logical>) -> Size<i32, Logical> {
    let (width, height) = Margins::of(theme, shape, bar).within((frame.w, frame.h));
    Size::from((width.max(1), height.max(1)))
}

/// The windows.
#[derive(Debug)]
pub struct Windows {
    /// The sizes and colours of the frames.
    theme: Theme,
    /// The windows that show, in their order from back to front, and
    /// the screens.
    pub space: Space<Window>,
    /// Every window a program has drawn, minimized ones too.
    framed: Vec<Framed>,
    /// How many windows have shown, for the step of the next.
    shown: i32,
    /// Counts the times a window was given the keyboard.
    uses: u64,
}

impl Default for Windows {
    fn default() -> Windows {
        Windows {
            theme: Theme::basic(),
            space: Space::default(),
            framed: Vec::new(),
            shown: 0,
            uses: 0,
        }
    }
}

impl Windows {
    /// The sizes and colours of the frames.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// `window` with its frame.
    pub fn framed(&self, window: &Window) -> Option<&Framed> {
        self.framed.iter().find(|framed| framed.window == *window)
    }

    fn framed_mut(&mut self, window: &Window) -> Option<&mut Framed> {
        self.framed.iter_mut().find(|framed| framed.window == *window)
    }

    /// The window whose own surface is `surface`.
    pub fn of_surface(&self, surface: &WlSurface) -> Option<&Window> {
        let has = |framed: &&Framed| framed.window.wl_surface().is_some_and(|its| *its == *surface);
        self.framed.iter().find(has).map(|framed| &framed.window)
    }

    /// The windows that show, from the front to the back.
    pub fn stacked(&self) -> impl Iterator<Item = &Framed> {
        self.space.elements().rev().filter_map(|window| self.framed(window))
    }

    /// Every window, the one that last had the keyboard first.
    pub fn by_use(&self) -> Vec<&Framed> {
        let mut all: Vec<&Framed> = self.framed.iter().collect();
        all.sort_by_key(|framed| core::cmp::Reverse(framed.used));
        all
    }

    /// The window in front.
    pub fn front(&self) -> Option<Window> {
        self.space.elements().next_back().cloned()
    }

    /// The screen most of `area` is on, or the first.
    fn screen_of(&self, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let screens = self.space.outputs().filter_map(|screen| self.space.output_geometry(screen));
        let shared = |screen: &Rectangle<i32, Logical>| {
            let shared =
                screen.intersection(area).map_or(0, |shared| shared.size.w * shared.size.h);
            // Of screens that share as much, the first.
            (shared, core::cmp::Reverse(screen.loc.x))
        };
        screens.max_by_key(shared).unwrap_or_default()
    }

    /// The area of the screen the place `at` is on, or of the first.
    pub fn screen_at(&self, at: Point<f64, Logical>) -> Rectangle<i32, Logical> {
        let under = self.space.output_under(at).next();
        let screen = under.or_else(|| self.space.outputs().next());
        screen.and_then(|screen| self.space.output_geometry(screen)).unwrap_or_default()
    }

    /// The size a program is told its window has room for on the
    /// screen the pointer, at `pointer`, is on.
    pub fn room(&self, pointer: Point<f64, Logical>) -> Size<i32, Logical> {
        inside(&self.theme, Shape::Normal, Bar::Full, self.screen_at(pointer).size)
    }

    /// The size of the own pixels of a window that fills the screen
    /// the pointer, at `pointer`, is on.
    pub fn filling(&self, pointer: Point<f64, Logical>) -> Size<i32, Logical> {
        inside(&self.theme, Shape::Maximized, Bar::Full, self.screen_at(pointer).size)
    }

    /// Puts the surface of the window at `index` where its frame has
    /// it, without changing the order of the windows.
    fn put(&mut self, index: usize) {
        let framed = &self.framed[index];
        if framed.minimized {
            return;
        }
        let window = framed.window.clone();
        let at = framed.at + Point::from((framed.layout.surface.x, framed.layout.surface.y));
        if self.space.element_location(&window) == Some(at) {
            return;
        }
        // Mapping brings to the front: those that were in front are
        // brought there again, in their order.
        let over: Vec<Window> =
            self.space.elements().skip_while(|other| **other != window).skip(1).cloned().collect();
        self.space.map_element(window, at, false);
        for other in &over {
            self.space.raise_element(other, false);
        }
    }

    /// The layout of a window's frame as the window's program has drawn
    /// it now.
    fn layout_of(theme: &Theme, window: &Window, shape: Shape) -> Layout {
        let size = window.geometry().size;
        Layout::of(theme, (size.w, size.h), shape, Bar::Full, None)
    }

    /// Where the corner of a frame `frame` wide and high first goes on
    /// `screen`: the middle, a step down and to the right of the window
    /// before it.
    fn first_place(
        &mut self,
        screen: Rectangle<i32, Logical>,
        frame: Size<i32, Logical>,
    ) -> Point<i32, Logical> {
        let step = self.shown % STEPS * STEP;
        self.shown += 1;
        let middle = |room: i32, taken: i32| ((room - taken) / 2 + step).min(room - taken).max(0);
        let within = (middle(screen.size.w, frame.w), middle(screen.size.h, frame.h));
        screen.loc + Point::from(within)
    }

    /// A program drew `window` for the first time: it shows on the
    /// screen the pointer, at `pointer`, is on, in front.
    pub fn show(&mut self, window: Window, pointer: Point<f64, Logical>) {
        let maximized = window
            .toplevel()
            .is_some_and(|toplevel| toplevel.current_state().states.contains(State::Maximized));
        let shape = if maximized { Shape::Maximized } else { Shape::Normal };
        let layout = Windows::layout_of(&self.theme, &window, shape);
        let screen = self.screen_at(pointer);
        let at = match shape {
            Shape::Maximized => screen.loc,
            Shape::Normal => self.first_place(screen, (layout.frame.w, layout.frame.h).into()),
        };
        let title = title_of(&window);
        self.framed.push(Framed {
            window: window.clone(),
            at,
            layout,
            minimized: false,
            title,
            restore: Restore::default(),
            fills: screen,
            resize: None,
            strips: Vec::new(),
            shown: None,
            used: 0,
        });
        let within = Point::from((layout.surface.x, layout.surface.y));
        self.space.map_element(window, at + within, false);
    }

    /// `window` is gone.
    pub fn gone(&mut self, window: &Window) {
        self.space.unmap_elem(window);
        self.framed.retain(|framed| framed.window != *window);
    }

    /// Lets go of the windows whose programs are gone.
    pub fn refresh(&mut self) {
        self.space.refresh();
        self.framed.retain(|framed| framed.window.alive());
    }

    /// The program of `window` drew it anew: the frame is as the
    /// window is now. Returns whether the window was one that shows or
    /// is minimized.
    pub fn committed(&mut self, window: &Window) -> bool {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return false;
        };
        let theme = self.theme;
        let framed = &mut self.framed[index];
        let Some(toplevel) = framed.window.toplevel().cloned() else { return false };
        let drawn = toplevel.current_state().states.contains(State::Maximized);
        let shape = if drawn { Shape::Maximized } else { Shape::Normal };
        let before = framed.layout;
        framed.layout = Windows::layout_of(&theme, window, shape);
        if framed.layout == before && framed.resize.is_none() {
            return true;
        }
        let frame = Size::from((framed.layout.frame.w, framed.layout.frame.h));
        match (before.shape, shape) {
            (Shape::Normal, Shape::Maximized) => framed.at = framed.fills.loc,
            (Shape::Maximized, Shape::Normal) => {
                let (fills, restore) = (framed.fills, core::mem::take(&mut framed.restore));
                let at = restore.at.unwrap_or_else(|| self.first_place(fills, frame));
                self.framed[index].at = at;
            }
            _ => {}
        }
        let framed = &mut self.framed[index];
        if let Some(resize) = &mut framed.resize {
            if shape == Shape::Normal {
                framed.at = resize.placed(frame);
            }
            if resize.committed(&toplevel) {
                framed.resize = None;
            }
        }
        self.put(index);
        true
    }

    /// The program of `window` gave it another title.
    pub fn retitled(&mut self, window: &Window) {
        if let Some(framed) = self.framed_mut(window) {
            framed.title = title_of(window);
        }
    }

    /// `window` was given the keyboard: it is in front, and the first
    /// of the list.
    pub fn used(&mut self, window: &Window) {
        self.uses += 1;
        let uses = self.uses;
        if let Some(framed) = self.framed_mut(window) {
            framed.used = uses;
        }
        self.space.raise_element(window, false);
    }

    /// What is under the place `at` of the desktop.
    pub fn under(&self, at: Point<f64, Logical>) -> Under {
        for framed in self.stacked() {
            let window = &framed.window;
            let origin = framed.surface_at().to_f64();
            let popups = WindowSurfaceType::POPUP | WindowSurfaceType::SUBSURFACE;
            if let Some((surface, place)) = window.surface_under(at - origin, popups) {
                let at = (place + framed.surface_at()).to_f64();
                return Under::Surface { window: window.clone(), surface, at };
            }
            let within = (at - framed.at.to_f64()).to_i32_floor::<i32>();
            let part = framed.layout.part_at(within.x, within.y);
            match part {
                Part::Outside => continue,
                Part::Content => {
                    let own = WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE;
                    if let Some((surface, place)) = window.surface_under(at - origin, own) {
                        let at = (place + framed.surface_at()).to_f64();
                        return Under::Surface { window: window.clone(), surface, at };
                    }
                }
                _ => {}
            }
            return Under::Part { window: window.clone(), part };
        }
        Under::Nothing
    }

    /// Moves `window` so that the corner of its frame's box is at
    /// `at`. A maximized window is moved to there once it is restored.
    pub fn move_to(&mut self, window: &Window, at: Point<i32, Logical>) {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        let framed = &mut self.framed[index];
        if framed.layout.shape == Shape::Maximized {
            framed.restore.at = Some(at);
        } else {
            framed.at = at;
            self.put(index);
        }
    }

    /// The place nearest to `at` for the corner of a frame `width` wide
    /// that leaves some of its title bar on a screen.
    pub fn reachable(&self, at: Point<i32, Logical>, width: i32) -> Point<i32, Logical> {
        let bar = Rectangle::new(at, (width, self.theme.bar).into());
        let screen = self.screen_of(bar);
        let kept = KEPT.min(width);
        let x = at.x.clamp(screen.loc.x + kept - width, screen.loc.x + screen.size.w - kept);
        let y = at.y.clamp(screen.loc.y, screen.loc.y + (screen.size.h - self.theme.bar).max(0));
        Point::from((x, y))
    }

    /// Asks the program of `window` to draw it maximized, to fill the
    /// screen most of it is on.
    pub fn maximize(&mut self, window: &Window) {
        let theme = self.theme;
        let Some(fills) = self.framed(window).map(|framed| self.screen_of(framed.frame())) else {
            return;
        };
        let Some(framed) = self.framed_mut(window) else { return };
        if framed.maximized() {
            return;
        }
        let Some(toplevel) = framed.window.toplevel() else { return };
        framed.fills = fills;
        if framed.layout.shape == Shape::Normal {
            let size = framed.window.geometry().size;
            framed.restore = Restore { at: Some(framed.at), size: Some(size) };
        }
        framed.resize = None;
        let size = inside(&theme, Shape::Maximized, framed.layout.bar, framed.fills.size);
        toplevel.with_pending_state(|state| {
            state.states.set(State::Maximized);
            state.states.unset(State::Resizing);
            state.size = Some(size);
        });
        toplevel.send_pending_configure();
    }

    /// Asks the program of `window`, which is maximized, to draw it as
    /// it was before.
    pub fn restore(&mut self, window: &Window) {
        let Some(framed) = self.framed_mut(window) else { return };
        if !framed.maximized() {
            return;
        }
        let Some(toplevel) = framed.window.toplevel() else { return };
        let size = framed.restore.size;
        toplevel.with_pending_state(|state| {
            state.states.unset(State::Maximized);
            state.size = size;
        });
        toplevel.send_pending_configure();
    }

    /// The width of the frame of `window`, which is maximized, once it
    /// is restored, if its program does not choose it.
    pub fn restored_width(&self, window: &Window) -> Option<i32> {
        let framed = self.framed(window)?;
        let size = framed.restore.size?;
        let margins = Margins::of(&self.theme, Shape::Normal, framed.layout.bar);
        Some(margins.around((size.w, size.h)).0)
    }

    /// Takes `window` off the screens. It stays in the list.
    pub fn minimize(&mut self, window: &Window) {
        let Some(framed) = self.framed_mut(window) else { return };
        framed.minimized = true;
        self.space.unmap_elem(window);
    }

    /// Shows `window`, which is minimized, where it was, in front.
    pub fn unminimize(&mut self, window: &Window) {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        if core::mem::take(&mut self.framed[index].minimized) {
            self.put(index);
        }
    }

    /// Starts a resize of `window` by `edge`: the sides `edge` does not
    /// name stay where they are until [`Windows::resize_done`]. Returns
    /// whether the window can be resized now: one that is maximized
    /// cannot.
    pub fn resize_from(&mut self, window: &Window, edge: Edge) -> bool {
        let Some(framed) = self.framed_mut(window) else { return false };
        if framed.maximized() || framed.minimized {
            return false;
        }
        framed.resize = Some(Resize::new(edge, framed.frame(), framed.window.geometry().size));
        true
    }

    /// The pointer has moved by `by` since the resize of `window`
    /// began: the program is asked for the size that puts the edge
    /// under it.
    pub fn resize_by(&mut self, window: &Window, by: Point<i32, Logical>) {
        let theme = self.theme;
        let Some(framed) = self.framed_mut(window) else { return };
        let Some(frame) = framed.resize.as_ref().map(|resize| resize.dragged(by)) else { return };
        let margins = Margins::of(&theme, Shape::Normal, framed.layout.bar);
        let wanted = framed.fitted(&theme, margins.within((frame.w, frame.h)));
        let (Some(resize), Some(toplevel)) = (&mut framed.resize, framed.window.toplevel()) else {
            return;
        };
        resize.want(toplevel, wanted);
    }

    /// The resize of `window` is over: its program is told so with the
    /// last size.
    pub fn resize_done(&mut self, window: &Window) {
        let Some(framed) = self.framed_mut(window) else { return };
        let (Some(resize), Some(toplevel)) = (&mut framed.resize, framed.window.toplevel()) else {
            return;
        };
        if resize.end(toplevel) {
            framed.resize = None;
        }
    }

    /// Draws anew the frames that do not show what they should: `active`
    /// is the window the person works in, `hot` the button the pointer
    /// is on and `pressed` the button that is held down. Returns
    /// whether any frame changed.
    pub fn redraw(
        &mut self,
        active: Option<&Window>,
        hot: Option<(&Window, Button)>,
        pressed: Option<(&Window, Button)>,
    ) -> bool {
        let theme = self.theme;
        let mut changed = false;
        for framed in self.framed.iter_mut().filter(|framed| !framed.minimized) {
            let of = |button: Option<(&Window, Button)>| {
                button.filter(|(window, _)| **window == framed.window).map(|(_, button)| button)
            };
            let (hot, pressed) = (of(hot), of(pressed));
            let active = active == Some(&framed.window);
            changed |= framed.draw(&theme, active, hot, pressed);
        }
        changed
    }
}

/// The title the program of `window` gave it, or none.
fn title_of(window: &Window) -> String {
    let Some(surface) = window.wl_surface() else { return String::new() };
    with_states(&surface, |states| {
        let role = states.data_map.get::<XdgToplevelSurfaceData>();
        let title = role.and_then(|role| role.lock().ok()?.title.clone());
        title.unwrap_or_default()
    })
}
