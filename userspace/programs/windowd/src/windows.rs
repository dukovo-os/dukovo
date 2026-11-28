// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The windows and their frames: where each is, in which order they
//! lie, and what changes that (ADR 0721).
//!
//! Every window has windowd's frame around it (ADR 0719). A window is
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
//! A window's title bar is windowd's whole or shared with the window's
//! program ([`crate::titlebar`]). A window keeps its frame's box when
//! the bar changes hands: its own surface is the bar's rows higher
//! while the bar is shared.
//!
//! Nothing here reaches the seat: who has the keyboard is the
//! [`Shell`](crate::shell::Shell)'s to say.

pub mod desks;
pub mod resize;
pub mod zones;

use std::collections::BTreeMap;
use std::convert::Infallible;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::desktop::utils::with_surfaces_surface_tree;
use smithay::desktop::{PopupManager, Space, Window, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::{SurfaceData, send_surface_state, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelStateSet, XdgToplevelSurfaceData};
use window_frame::{Bar, Button, Canvas, Edge, Layout, Look, Margins, Part, Rect, Shape, Theme};

use desks::Row;
use resize::Resize;
use zones::{Fills, Outer, Side, Splits, Zone};

use crate::screens::layout;

use crate::titlebar::TitleBar;

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

/// Where a window that is maximized or snapped goes when it is
/// restored.
#[derive(Debug, Clone, Copy, Default)]
pub struct Restore {
    /// The corner of the frame's box, or `None` for the middle of the
    /// screen: the window was maximized when it first showed.
    at: Option<Point<i32, Logical>>,
    /// The size of the frame's box, or `None` for the size the program
    /// chooses.
    frame: Option<Size<i32, Logical>>,
}

/// The splits of a screen's desktop that a drag of an edge moves: the
/// edge of a snapped window where it meets the other zones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// The screen's name, and which of its desktops.
    screen: String,
    desk: usize,
    /// Whether the drag moves where the sides meet.
    across: bool,
    /// The side whose parts it moves the meeting of, if any.
    down: Option<Side>,
    /// Where the sides met, and that side's parts, when it began.
    from: Point<i32, Logical>,
}

impl Split {
    /// Whether a window snapped to `zone` has another area when the
    /// splits move.
    fn moves(&self, zone: Zone) -> bool {
        self.across || matches!(zone, Zone::Quarter(side, _) if Some(side) == self.down)
    }
}

/// Where a window was on a screen that went: it goes back there when
/// the screen is there again, unless the person has moved it since.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Home {
    /// The screen's name.
    screen: String,
    /// The screen's area of the desktop then.
    area: Rectangle<i32, Logical>,
    /// The corner of the frame's box then, or where a maximized window
    /// was to go when restored.
    at: Option<Point<i32, Logical>>,
    /// Which of the screen's desktops the window was on.
    desk: usize,
}

/// What changes of a screen's desktops (ADR 0726).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desk {
    /// A desktop is made after the one that shows, and shows.
    New,
    /// The one that shows closes: its windows are on the one before
    /// it, or on the one after for the first, which shows.
    Close,
    /// The one after shows, or the one before.
    Step {
        /// Whether the one before.
        back: bool,
    },
}

/// A screen: its name and its area of the desktop.
pub type Screen = (String, Rectangle<i32, Logical>);

/// The screen of `screens` most of `area` is on; of those that share
/// as much, the one furthest left.
fn most_of(screens: &[Screen], area: Rectangle<i32, Logical>) -> Option<&Screen> {
    let shared = |screen: &&Screen| {
        let shared = screen.1.intersection(area).map_or(0, |shared| shared.size.w * shared.size.h);
        (shared, core::cmp::Reverse(screen.1.loc.x))
    };
    screens.iter().max_by_key(shared)
}

/// Tells the program of `surface` the zoom and the turn of `screen`,
/// for it to draw the surface and those under it so.
fn tell(surface: &WlSurface, screen: &Output) {
    with_surfaces_surface_tree(surface, |surface, states| told(surface, states, screen));
}

/// Tells one surface so, if it was not told the same before: the whole
/// zoom and the turn by `wl_surface`, the zoom itself to a program
/// that asked by `wp_fractional_scale_v1`.
fn told(surface: &WlSurface, states: &SurfaceData, screen: &Output) {
    let zoom = screen.current_scale();
    send_surface_state(surface, states, zoom.integer_scale(), screen.current_transform());
    with_fractional_scale(states, |asked| asked.set_preferred_scale(zoom.fractional_scale()));
}

/// Has `states` say which sides of a window lie against its screen's
/// edges because it is snapped to `zone`, or none for `None`.
fn tiled(states: &mut ToplevelStateSet, zone: Option<Zone>) {
    let sides = |zone: Zone| -> [(State, bool); 4] {
        let (side, level) = match zone {
            Zone::Half(side) => (side, None),
            Zone::Quarter(side, level) => (side, Some(level)),
        };
        [
            (State::TiledLeft, side == zones::Side::Left),
            (State::TiledRight, side == zones::Side::Right),
            (State::TiledTop, level != Some(zones::Level::Lower)),
            (State::TiledBottom, level != Some(zones::Level::Upper)),
        ]
    };
    let none = [State::TiledLeft, State::TiledRight, State::TiledTop, State::TiledBottom];
    for (state, lies) in zone.map_or(none.map(|state| (state, false)), sides) {
        if lies {
            states.set(state);
        } else {
            states.unset(state);
        }
    }
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
    /// The name of the screen it is on, and which of that screen's
    /// desktops.
    screen: String,
    desk: usize,
    /// The window's title, as its program last gave it.
    title: String,
    /// Where a maximized window goes when it is restored.
    restore: Restore,
    /// The area a window that was asked to be maximized or snapped
    /// fills once its program has drawn it so.
    fills: Rectangle<i32, Logical>,
    /// The zone of its screen the window is snapped to.
    zone: Option<Zone>,
    /// The resize that is under way.
    resize: Option<Resize>,
    /// Where it goes back to when a screen that went is there again.
    home: Option<Home>,
    /// What the renderer knows the ground under its content by.
    ground: Id,
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

    /// The ground under the window's content, a point wider on each
    /// side, and what the renderer knows it by. On a screen whose zoom
    /// is between whole numbers, the frame and the content are each
    /// drawn to the nearest pixel, and may leave a pixel between them:
    /// the ground shows there, not the desktop.
    pub fn ground(&self) -> (&Id, Rectangle<i32, Logical>) {
        let content = self.layout.content;
        let corner = self.at + Point::from((content.x - 1, content.y - 1));
        (&self.ground, Rectangle::new(corner, (content.w + 2, content.h + 2).into()))
    }

    /// Where the top left corner of the window's own surface is on the
    /// desktop.
    pub fn surface_at(&self) -> Point<i32, Logical> {
        let within = Point::from((self.layout.surface.x, self.layout.surface.y));
        self.at + within - self.window.geometry().loc
    }

    /// Where the top left corner of the window's geometry is on the
    /// desktop: what the program reckons its window's places from.
    pub fn geometry_at(&self) -> Point<i32, Logical> {
        self.at + Point::from((self.layout.surface.x, self.layout.surface.y))
    }

    /// The parts of the desktop the window's own surface shows in: the
    /// content, and the region of a shared title bar.
    pub fn shows(&self) -> impl Iterator<Item = Rectangle<i32, Logical>> {
        let parts = [Some(self.layout.content), self.layout.region];
        parts.into_iter().flatten().map(|part| {
            Rectangle::new(self.at + Point::from((part.x, part.y)), (part.w, part.h).into())
        })
    }

    /// What the window fills of its screen, or is asked to.
    pub fn fills(&self) -> Fills {
        match self.zone {
            Some(zone) => Fills::Zone(zone),
            None if self.maximized() => Fills::All,
            None => Fills::Nothing,
        }
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
    /// nothing, that the window's surface can have under a title bar
    /// that is `bar`'s.
    fn fitted(&self, theme: &Theme, bar: Bar, (width, height): (i32, i32)) -> Size<i32, Logical> {
        let (least, most) = self.limits();
        let own = Layout::least(theme, bar);
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

/// The size of the surface of a window of `shape` whose frame's box is
/// `frame` and whose title bar is `bar`'s: a pixel each way at the
/// least.
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
    /// Each screen's desktops, by the screen's name. A screen with no
    /// entry has the one desktop, and one that went keeps its entry
    /// for when it is there again.
    rows: BTreeMap<String, Row>,
    /// Where the zones meet of a screen's desktop, by the screen's
    /// name and the desktop's place: in the middle with no entry.
    splits: BTreeMap<(String, usize), Splits>,
}

impl Default for Windows {
    fn default() -> Windows {
        Windows {
            theme: Theme::basic(),
            space: Space::default(),
            framed: Vec::new(),
            shown: 0,
            uses: 0,
            rows: BTreeMap::new(),
            splits: BTreeMap::new(),
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

    /// The windows of the desktops that show, minimized ones too, the
    /// one that last had the keyboard first: what the list of windows
    /// holds.
    pub fn listed(&self) -> Vec<&Framed> {
        let mut listed = self.by_use();
        listed.retain(|framed| self.on_shown_desk(framed));
        listed
    }

    /// The desktops of the screen named `screen`.
    fn row(&self, screen: &str) -> Row {
        self.rows.get(screen).copied().unwrap_or_default()
    }

    /// Whether the desktop `framed` is on is the one its screen shows.
    fn on_shown_desk(&self, framed: &Framed) -> bool {
        self.row(&framed.screen).shown() == framed.desk
    }

    /// The name of the screen the place `at` is on, or of the first.
    fn screen_name_at(&self, at: Point<f64, Logical>) -> Option<String> {
        let under = self.space.output_under(at).next();
        under.or_else(|| self.space.outputs().next()).map(Output::name)
    }

    /// The window at `index` is on the desktop that shows of the
    /// screen most of it is on, if that is another screen than it was
    /// on.
    fn join(&mut self, index: usize) {
        let screens = self.screens();
        let Some((name, _)) = most_of(&screens, Windows::on(&self.framed[index])) else { return };
        if self.framed[index].screen != *name {
            let desk = self.row(name).shown();
            let framed = &mut self.framed[index];
            (framed.screen, framed.desk) = (name.clone(), desk);
        }
    }

    /// Changes the desktops of the screen the place `at` is on as
    /// `change` says. The window `along`, if it is on that screen, is
    /// then on the desktop that shows. Returns whether another desktop
    /// shows: none does at the row's end, for the only desktop that
    /// would close, or in a row that is full.
    pub fn desk(&mut self, at: Point<f64, Logical>, change: Desk, along: Option<&Window>) -> bool {
        let Some(screen) = self.screen_name_at(at) else { return false };
        let mut row = self.row(&screen);
        let moved: fn(usize, usize) -> usize = match change {
            Desk::New => desks::after_add,
            Desk::Close => desks::after_close,
            Desk::Step { .. } => |desk, _| desk,
        };
        let changed = match change {
            Desk::New => row.add(),
            Desk::Close => row.close(),
            Desk::Step { back } => row.step(back),
        };
        let Some(changed) = changed else { return false };
        for framed in self.framed.iter_mut().filter(|framed| framed.screen == screen) {
            framed.desk = moved(framed.desk, changed);
            if along == Some(&framed.window) {
                framed.desk = row.shown();
            }
        }
        self.rows.insert(screen.clone(), row);
        // Each desktop's splits go where the desktop does: a new one
        // has none, and a closed one's are let go.
        let splits = core::mem::take(&mut self.splits);
        let kept = splits.into_iter().filter_map(|((name, desk), splits)| {
            let closed = change == Desk::Close && desk == changed;
            let desk = if name == screen { moved(desk, changed) } else { desk };
            (!closed || name != screen).then_some(((name, desk), splits))
        });
        self.splits = kept.collect();
        // The windows that show now are over one another as they were
        // last used.
        let mut order: Vec<usize> =
            (0..self.framed.len()).filter(|index| self.framed[*index].screen == screen).collect();
        order.sort_by_key(|index| self.framed[*index].used);
        for index in order {
            self.put(index);
            let window = self.framed[index].window.clone();
            if self.space.element_location(&window).is_some() {
                self.space.raise_element(&window, false);
            }
        }
        true
    }

    /// Whether `window` shows: it is not minimized, and on a desktop
    /// that shows.
    pub fn showing(&self, window: &Window) -> bool {
        self.framed(window).is_some_and(|framed| !framed.minimized && self.on_shown_desk(framed))
    }

    /// The window last used of those that show on the screen the place
    /// `at` is on.
    pub fn last_used_at(&self, at: Point<f64, Logical>) -> Option<Window> {
        let screen = self.screen_name_at(at)?;
        let shows = |framed: &&Framed| framed.screen == screen && self.showing(&framed.window);
        self.by_use().into_iter().find(shows).map(|framed| framed.window.clone())
    }

    /// The window in front.
    pub fn front(&self) -> Option<Window> {
        self.space.elements().next_back().cloned()
    }

    /// The screens, in the order they were shown on.
    pub fn screens(&self) -> Vec<Screen> {
        let named = |screen: &Output| Some((screen.name(), self.space.output_geometry(screen)?));
        self.space.outputs().filter_map(named).collect()
    }

    /// The screen most of `area` is on, or the first.
    fn screen_of(&self, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        most_of(&self.screens(), area).map_or_default(|(_, area)| *area)
    }

    /// Tells the program of `window` the zoom and the turn of the
    /// screen most of the window is on, for it to draw the window's
    /// surfaces and its popups' so.
    pub fn tell(&self, window: &Window) {
        let Some(framed) = self.framed(window) else { return };
        let screens = self.screens();
        let Some((name, _)) = most_of(&screens, Windows::on(framed)) else { return };
        let Some(screen) = self.space.outputs().find(|screen| screen.name() == *name) else {
            return;
        };
        window.with_surfaces(|surface, states| told(surface, states, screen));
        let Some(surface) = window.wl_surface() else { return };
        for (popup, _) in PopupManager::popups_for_surface(&surface) {
            tell(popup.wl_surface(), screen);
        }
    }

    /// Tells every window's program so.
    pub fn tell_all(&self) {
        self.framed.iter().for_each(|framed| self.tell(&framed.window));
    }

    /// Tells the program of `surface`, which shows at `at` or is about
    /// to, the zoom and the turn of the screen there, or of the first.
    pub fn tell_at(&self, surface: &WlSurface, at: Point<f64, Logical>) {
        let under = self.space.output_under(at).next();
        if let Some(screen) = under.or_else(|| self.space.outputs().next()) {
            tell(surface, screen);
        }
    }

    /// The area a window fills or is on most of: what it is carried
    /// from when the screens change.
    fn on(framed: &Framed) -> Rectangle<i32, Logical> {
        if framed.fills() == Fills::Nothing { framed.frame() } else { framed.fills }
    }

    /// The screens were `before` and are others now. A window on a
    /// screen that has another area is as far across and down it as it
    /// was, and fills it if it did. A window on a screen that went is
    /// carried so to the nearest screen there is, and back when its
    /// screen is there again (ADR 0725).
    pub fn screens_changed(&mut self, before: &[Screen]) {
        let now = self.screens();
        // With no screen, every window waits where it is for one.
        if now.is_empty() || now == before {
            return;
        }
        let areas: Vec<Rectangle<i32, Logical>> = now.iter().map(|(_, area)| *area).collect();
        let area_of = |screens: &[Screen], name: &str| {
            screens.iter().find(|(other, _)| other == name).map(|(_, area)| *area)
        };
        for index in 0..self.framed.len() {
            let framed = &self.framed[index];
            let back = framed.home.as_ref().and_then(|home| {
                let gone = area_of(before, &home.screen).is_none();
                Some((home, area_of(&now, &home.screen).filter(|_| gone)?))
            });
            if let Some((home, area)) = back {
                let at = home.at.map(|at| layout::across(home.area, area, at));
                let from = most_of(before, Windows::on(framed)).map_or(area, |(_, was)| *was);
                let desk = self.row(&home.screen).nearest(home.desk);
                self.carry(index, from, area);
                self.place(index, at);
                // On its screen again, it is on the desktop it was on.
                self.join(index);
                self.framed[index].desk = desk;
                self.put(index);
                self.framed[index].home = None;
                continue;
            }
            let Some((name, was)) = most_of(before, Windows::on(framed)) else { continue };
            match area_of(&now, name) {
                Some(area) => self.carry(index, *was, area),
                None => {
                    let Some(nearest) = layout::nearest(*was, &areas) else { continue };
                    let at = if framed.maximized() { framed.restore.at } else { Some(framed.at) };
                    let home = Home { screen: name.clone(), area: *was, at, desk: framed.desk };
                    self.carry(index, *was, areas[nearest]);
                    self.framed[index].home.get_or_insert(home);
                }
            }
        }
    }

    /// Has the corner of the frame of the window at `index` be at
    /// `at`, or a maximized window go there when it is restored.
    fn place(&mut self, index: usize, at: Option<Point<i32, Logical>>) {
        let framed = &mut self.framed[index];
        if framed.maximized() {
            framed.restore.at = at;
        } else if let Some(at) = at {
            if let Some(resize) = &mut framed.resize {
                resize.moved(at - framed.at);
            }
            framed.at = at;
        }
    }

    /// Carries the window at `index` from the screen whose area is or
    /// was `from` to the one whose area is `to`: as far across and
    /// down it, and filling it if it filled the other.
    fn carry(&mut self, index: usize, from: Rectangle<i32, Logical>, to: Rectangle<i32, Logical>) {
        if from == to {
            return;
        }
        let theme = self.theme;
        let zone = self.framed[index].zone.map(|zone| self.zone_area(index, to, zone));
        let framed = &mut self.framed[index];
        if framed.fills() == Fills::Nothing {
            let at = layout::across(from, to, framed.at);
            let at = self.reachable_on(to, at, self.framed[index].layout.frame.w);
            self.place(index, Some(at));
        } else {
            framed.restore.at = framed.restore.at.map(|at| layout::across(from, to, at));
            let (fills, shape) = match zone {
                Some(area) => (area, Shape::Normal),
                None => (to, Shape::Maximized),
            };
            let resized = framed.fills.size != fills.size;
            framed.fills = fills;
            if framed.layout.shape == shape {
                framed.at = fills.loc;
            }
            if resized && let Some(toplevel) = framed.window.toplevel() {
                let bar = TitleBar::of(&framed.window).told();
                let size = inside(&theme, shape, bar, fills.size);
                toplevel.with_pending_state(|state| state.size = Some(size));
                toplevel.send_pending_configure();
            }
        }
        self.put(index);
    }

    /// Where the zones meet on the desktop at `desk` of the screen
    /// named `screen`.
    fn splits_of(&self, screen: &str, desk: usize) -> Splits {
        self.splits.get(&(screen.to_owned(), desk)).copied().unwrap_or_default()
    }

    /// The area of `zone` on the screen whose area is `screen`, for
    /// the window at `index`: by the splits of the desktop it is on if
    /// that is its screen, and of the one that shows there otherwise.
    fn zone_area(
        &self,
        index: usize,
        screen: Rectangle<i32, Logical>,
        zone: Zone,
    ) -> Rectangle<i32, Logical> {
        let framed = &self.framed[index];
        let screens = self.screens();
        let name = screens.iter().find(|(_, area)| *area == screen).map(|(name, _)| name.as_str());
        let splits = match name {
            Some(name) if name == framed.screen => self.splits_of(name, framed.desk),
            Some(name) => self.splits_of(name, self.row(name).shown()),
            None => Splits::default(),
        };
        splits.area(screen, zone)
    }

    /// Has `window` fill `fills` of the screen most of it is on: the
    /// whole of it, a zone, or what it did before it filled either.
    pub fn fill(&mut self, window: &Window, fills: Fills) {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        self.join(index);
        let screens = self.screens();
        if let Some(screen) = most_of(&screens, Windows::on(&self.framed[index])) {
            self.fill_on(index, screen, fills);
        }
    }

    /// Has the window at `index` fill `fills` of `screen`.
    fn fill_on(&mut self, index: usize, screen: &Screen, fills: Fills) {
        match fills {
            Fills::All => self.maximize_on(index, screen.1),
            Fills::Zone(zone) => self.snap_on(index, screen, zone),
            Fills::Nothing => {
                let window = self.framed[index].window.clone();
                self.restore(&window);
            }
        }
    }

    /// Snaps the window at `index` to `zone` of `screen`: its frame is
    /// at the zone's corner, and its program is asked to draw it at the
    /// zone's size (ADR 0727).
    fn snap_on(&mut self, index: usize, (name, area): &Screen, zone: Zone) {
        let theme = self.theme;
        // On another screen than its own, it is on the desktop that
        // shows there.
        let framed = &self.framed[index];
        let desk = if framed.screen == *name { framed.desk } else { self.row(name).shown() };
        let fills = self.splits_of(name, desk).area(*area, zone);
        let framed = &mut self.framed[index];
        let Some(toplevel) = framed.window.toplevel() else { return };
        if framed.fills() == Fills::Nothing {
            framed.restore = Restore { at: Some(framed.at), frame: Some(framed.frame().size) };
        }
        (framed.screen, framed.desk) = (name.clone(), desk);
        (framed.zone, framed.fills, framed.resize) = (Some(zone), fills, None);
        let bar = TitleBar::of(&framed.window).told();
        let size = inside(&theme, Shape::Normal, bar, fills.size);
        toplevel.with_pending_state(|state| {
            state.states.unset(State::Maximized);
            state.states.unset(State::Resizing);
            tiled(&mut state.states, Some(zone));
            state.size = Some(size);
        });
        toplevel.send_pending_configure();
        // A window that was maximized goes there once its program has
        // drawn it with a frame's sides again.
        if framed.layout.shape == Shape::Normal {
            framed.at = fills.loc;
            self.put(index);
        }
    }

    /// Which edges of the screen whose area is `screen` no other screen
    /// lies beyond at the place `at`.
    fn outer(&self, screen: Rectangle<i32, Logical>, at: Point<f64, Logical>) -> Outer {
        let (left, top) = (f64::from(screen.loc.x), f64::from(screen.loc.y));
        let (right, bottom) = (left + f64::from(screen.size.w), top + f64::from(screen.size.h));
        let none_at = |x: f64, y: f64| self.space.output_under((x, y)).next().is_none();
        Outer {
            left: none_at(left - 1.0, at.y),
            right: none_at(right, at.y),
            top: none_at(at.x, top - 1.0),
            bottom: none_at(at.x, bottom),
        }
    }

    /// What a window that is dragged by its title bar fills if it is
    /// let go with the pointer at `at`, and the area of that: `None`
    /// away from the edges of the desktop.
    pub fn dropped_at(&self, at: Point<f64, Logical>) -> Option<(Fills, Rectangle<i32, Logical>)> {
        let screen = self.space.output_under(at).next()?;
        let area = self.space.output_geometry(screen)?;
        let fills = zones::dragged_to(area, at, self.outer(area, at));
        let name = screen.name();
        let filled = match fills {
            Fills::Nothing => return None,
            Fills::All => area,
            Fills::Zone(zone) => self.splits_of(&name, self.row(&name).shown()).area(area, zone),
        };
        Some((fills, filled))
    }

    /// What is offered after `window` was snapped: the largest zone
    /// of its screen's desktop that no snapped window has a place in,
    /// the zone's area, and the windows of that desktop that are not
    /// snapped, the one that last had the keyboard first. `None` with
    /// no such zone or no such window.
    pub fn offered(
        &self,
        window: &Window,
    ) -> Option<(Zone, Rectangle<i32, Logical>, Vec<&Framed>)> {
        let snapped = self.framed(window)?;
        let (name, desk) = (&snapped.screen, snapped.desk);
        let area = self.screens().into_iter().find(|(other, _)| other == name)?.1;
        let here = |framed: &&Framed| framed.screen == *name && framed.desk == desk;
        let taken = |framed: &Framed| framed.zone.filter(|_| !framed.minimized);
        let zones: Vec<Zone> = self.framed.iter().filter(here).filter_map(taken).collect();
        let zone = zones::free(&zones)?;
        let mut others = self.by_use();
        others.retain(|framed| here(framed) && taken(framed).is_none());
        let area = self.splits_of(name, desk).area(area, zone);
        (!others.is_empty()).then_some((zone, area, others))
    }

    /// Where `window` goes when it is restored: where it is, for one
    /// that fills nothing of its screen.
    pub fn restores_to(&self, window: &Window) -> Option<Restore> {
        let framed = self.framed(window)?;
        Some(match framed.fills() {
            Fills::Nothing => Restore { at: Some(framed.at), frame: Some(framed.frame().size) },
            _ => framed.restore,
        })
    }

    /// `window` was let go with the pointer at `at` after it was
    /// dragged by its title bar from where `was` says: it fills `fills`
    /// of the screen there, and goes back to there when it is restored.
    pub fn drop_at(
        &mut self,
        window: &Window,
        at: Point<f64, Logical>,
        fills: Fills,
        was: Option<Restore>,
    ) {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        let screens = self.screens();
        let Some(screen) = screens.iter().find(|(_, area)| area.to_f64().contains(at)) else {
            return;
        };
        self.fill_on(index, screen, fills);
        if let Some(was) = was {
            self.framed[index].restore = was;
        }
    }

    /// The edge `edge` of `window` is taken hold of. If the window is
    /// snapped and the edge lies where its zone meets the others, a
    /// drag of it moves the splits of its screen's desktop: every
    /// snapped window there with a side along them is resized, paced by
    /// its program as a resize is, until [`Windows::split_done`].
    pub fn split_from(&mut self, window: &Window, edge: Edge) -> Option<Split> {
        let framed = self.framed(window)?;
        let zone = framed.zone.filter(|zone| zone.meets_at(edge))?;
        let (screen, desk) = (framed.screen.clone(), framed.desk);
        let area = self.screens().into_iter().find(|(name, _)| *name == screen)?.1;
        let split = Split {
            across: edge.left() || edge.right(),
            down: (edge.top() || edge.bottom()).then_some(zone.side()),
            from: self.splits_of(&screen, desk).meet(area, zone.side()),
            screen,
            desk,
        };
        for index in self.along(&split) {
            let framed = &mut self.framed[index];
            let Some(zone) = framed.zone else { continue };
            let content = framed.window.geometry().size;
            framed.resize = Some(Resize::new(zone.inner(), framed.frame(), content));
        }
        Some(split)
    }

    /// The windows that show snapped on the desktop `split` is of and
    /// have a side along the splits it moves.
    fn along(&self, split: &Split) -> Vec<usize> {
        let lies = |framed: &Framed| {
            (framed.screen == split.screen && framed.desk == split.desk && !framed.minimized)
                && framed.layout.shape == Shape::Normal
                && framed.zone.is_some_and(|zone| split.moves(zone))
        };
        (0..self.framed.len()).filter(|index| lies(&self.framed[*index])).collect()
    }

    /// The least a frame's box can be around the window at `index`.
    fn least_frame(&self, index: usize) -> (i32, i32) {
        let framed = &self.framed[index];
        let bar = TitleBar::of(&framed.window).told();
        let least = framed.fitted(&self.theme, bar, (0, 0));
        Margins::of(&self.theme, Shape::Normal, bar).around((least.w, least.h))
    }

    /// The pointer has moved by `by` since the drag of `split` began:
    /// the splits are as far from where they were, or as near to that
    /// as leaves every snapped window the least its program takes, and
    /// each window along them is asked for the size of its zone.
    pub fn split_to(&mut self, split: &Split, by: Point<i32, Logical>) {
        let theme = self.theme;
        let Some((_, area)) = self.screens().into_iter().find(|(name, _)| *name == split.screen)
        else {
            return;
        };
        let along = self.along(split);
        // How near each edge of the screen the splits may come.
        let (mut left, mut top) = (area.loc.x, area.loc.y);
        let (mut right, mut bottom) = (area.loc.x + area.size.w, area.loc.y + area.size.h);
        for index in &along {
            let (width, height) = self.least_frame(*index);
            match self.framed[*index].zone.map(Zone::inner) {
                Some(inner) if inner.right() => left = left.max(area.loc.x + width),
                Some(_) => right = right.min(area.loc.x + area.size.w - width),
                None => {}
            }
            match self.framed[*index].zone.map(Zone::inner) {
                Some(inner) if inner.bottom() => top = top.max(area.loc.y + height),
                Some(inner) if inner.top() => {
                    bottom = bottom.min(area.loc.y + area.size.h - height);
                }
                _ => {}
            }
        }
        let to = split.from + by;
        let mut splits = self.splits_of(&split.screen, split.desk);
        if split.across {
            splits.meet_across(area, to.x.min(right).max(left));
        }
        if let Some(side) = split.down {
            splits.meet_down(area, side, to.y.min(bottom).max(top));
        }
        self.splits.insert((split.screen.clone(), split.desk), splits);
        for index in along {
            let framed = &mut self.framed[index];
            let Some(zone) = framed.zone else { continue };
            framed.fills = splits.area(area, zone);
            let bar = TitleBar::of(&framed.window).told();
            let margins = Margins::of(&theme, Shape::Normal, bar);
            let frame = framed.fills.size;
            let wanted = framed.fitted(&theme, bar, margins.within((frame.w, frame.h)));
            if let (Some(resize), Some(toplevel)) = (&mut framed.resize, framed.window.toplevel()) {
                resize.want(toplevel, wanted);
            }
        }
    }

    /// The drag of `split` is over: the program of each window along
    /// the splits is told so with the last size.
    pub fn split_done(&mut self, split: &Split) {
        for index in self.along(split) {
            let framed = &mut self.framed[index];
            if let (Some(resize), Some(toplevel)) = (&mut framed.resize, framed.window.toplevel())
                && resize.end(toplevel)
            {
                framed.resize = None;
            }
        }
    }

    /// Carries `window` to the screen after the one it is on, from
    /// left to right, or to the one before it for `back`.
    pub fn next_screen(&mut self, window: &Window, back: bool) {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        let screens = self.screens();
        let areas: Vec<Rectangle<i32, Logical>> = screens.iter().map(|(_, area)| *area).collect();
        let on = most_of(&screens, Windows::on(&self.framed[index])).map(|(_, area)| *area);
        let from = on.and_then(|on| areas.iter().position(|area| *area == on));
        let Some((from, to)) =
            from.and_then(|from| Some((from, layout::beside(&areas, from, back)?)))
        else {
            return;
        };
        self.framed[index].home = None;
        self.carry(index, areas[from], areas[to]);
    }

    /// The area of the screen the place `at` is on, or of the first.
    pub fn screen_at(&self, at: Point<f64, Logical>) -> Rectangle<i32, Logical> {
        let under = self.space.output_under(at).next();
        let screen = under.or_else(|| self.space.outputs().next());
        screen.and_then(|screen| self.space.output_geometry(screen)).unwrap_or_default()
    }

    /// The size a program is told the surface of its window, whose
    /// title bar is `bar`'s, has room for on the screen the pointer, at
    /// `pointer`, is on.
    pub fn room(&self, pointer: Point<f64, Logical>, bar: Bar) -> Size<i32, Logical> {
        inside(&self.theme, Shape::Normal, bar, self.screen_at(pointer).size)
    }

    /// The size of the surface of a window, whose title bar is `bar`'s,
    /// that fills the screen the pointer, at `pointer`, is on.
    pub fn filling(&self, pointer: Point<f64, Logical>, bar: Bar) -> Size<i32, Logical> {
        inside(&self.theme, Shape::Maximized, bar, self.screen_at(pointer).size)
    }

    /// Puts the surface of the window at `index` where its frame has
    /// it, without changing the order of the windows.
    fn put(&mut self, index: usize) {
        self.join(index);
        let framed = &self.framed[index];
        let window = framed.window.clone();
        if framed.minimized || !self.on_shown_desk(framed) {
            self.space.unmap_elem(&window);
            return;
        }
        let at = framed.at + Point::from((framed.layout.surface.x, framed.layout.surface.y));
        if self.space.element_location(&window) == Some(at) {
            return;
        }
        // Mapping brings to the front: those that were in front are
        // brought there again, in their order.
        let over: Vec<Window> =
            self.space.elements().skip_while(|other| **other != window).skip(1).cloned().collect();
        self.space.map_element(window.clone(), at, false);
        for other in &over {
            self.space.raise_element(other, false);
        }
        self.tell(&window);
    }

    /// The layout of a window's frame as the window's program has drawn
    /// it now.
    fn layout_of(theme: &Theme, window: &Window, shape: Shape) -> Layout {
        let size = window.geometry().size;
        let (bar, region) = TitleBar::of(window).drawn();
        Layout::of(theme, (size.w, size.h), shape, bar, region)
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
        let on = self.screen_name_at(pointer).unwrap_or_default();
        let desk = self.row(&on).shown();
        self.framed.push(Framed {
            window: window.clone(),
            at,
            layout,
            minimized: false,
            screen: on,
            desk,
            title,
            restore: Restore::default(),
            fills: screen,
            zone: None,
            resize: None,
            home: None,
            ground: Id::new(),
            strips: Vec::new(),
            shown: None,
            used: 0,
        });
        let within = Point::from((layout.surface.x, layout.surface.y));
        self.tell(&window);
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
        // A surface under the window's that is new is told too.
        self.tell(window);
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
            (_, Shape::Maximized) => framed.at = framed.fills.loc,
            (_, Shape::Normal) if framed.zone.is_some() => framed.at = framed.fills.loc,
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
        // Where the person puts it is where it stays.
        framed.home = None;
        if framed.layout.shape == Shape::Maximized {
            framed.restore.at = Some(at);
        } else {
            // A resize whose last size the program has yet to draw
            // goes with the window.
            if let Some(resize) = &mut framed.resize {
                resize.moved(at - framed.at);
            }
            framed.at = at;
            self.put(index);
        }
    }

    /// The place nearest to `at` for the corner of a frame `width` wide
    /// that leaves some of its title bar on a screen.
    pub fn reachable(&self, at: Point<i32, Logical>, width: i32) -> Point<i32, Logical> {
        let bar = Rectangle::new(at, (width, self.theme.bar).into());
        self.reachable_on(self.screen_of(bar), at, width)
    }

    /// The place nearest to `at` for the corner of a frame `width` wide
    /// that leaves some of its title bar on the screen whose area is
    /// `screen`.
    fn reachable_on(
        &self,
        screen: Rectangle<i32, Logical>,
        at: Point<i32, Logical>,
        width: i32,
    ) -> Point<i32, Logical> {
        let kept = KEPT.min(width);
        // With no screen, or one narrower than what is kept, the left
        // end of the bar is the part that stays.
        let (least, most) = (screen.loc.x + kept - width, screen.loc.x + screen.size.w - kept);
        let x = at.x.clamp(least.min(most), most);
        let y = at.y.clamp(screen.loc.y, screen.loc.y + (screen.size.h - self.theme.bar).max(0));
        Point::from((x, y))
    }

    /// Asks the program of `window` to draw it maximized, to fill the
    /// screen most of it is on.
    pub fn maximize(&mut self, window: &Window) {
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        let fills = self.screen_of(self.framed[index].frame());
        self.maximize_on(index, fills);
    }

    /// Asks the program of the window at `index` to draw it maximized,
    /// to fill the screen whose area is `fills`.
    fn maximize_on(&mut self, index: usize, fills: Rectangle<i32, Logical>) {
        let theme = self.theme;
        let framed = &mut self.framed[index];
        let Some(toplevel) = framed.window.toplevel() else { return };
        let asked = toplevel.with_pending_state(|state| state.states.contains(State::Maximized));
        if asked && framed.fills == fills {
            return;
        }
        // A snapped window goes back to where it was before the snap.
        if framed.layout.shape == Shape::Normal && framed.zone.is_none() {
            framed.restore = Restore { at: Some(framed.at), frame: Some(framed.frame().size) };
        }
        (framed.zone, framed.fills, framed.resize) = (None, fills, None);
        let bar = TitleBar::of(&framed.window).told();
        let size = inside(&theme, Shape::Maximized, bar, framed.fills.size);
        toplevel.with_pending_state(|state| {
            state.states.set(State::Maximized);
            state.states.unset(State::Resizing);
            tiled(&mut state.states, None);
            state.size = Some(size);
        });
        toplevel.send_pending_configure();
        // One that is drawn maximized already, as when it was asked to
        // be restored and is let go at the top of another screen, is
        // there at once.
        if framed.layout.shape == Shape::Maximized {
            framed.at = fills.loc;
            self.put(index);
        }
    }

    /// Asks the program of `window`, which is maximized or snapped, to
    /// draw it as it was before. A snapped window is where it was
    /// before at once.
    pub fn restore(&mut self, window: &Window) {
        let theme = self.theme;
        let Some(index) = self.framed.iter().position(|framed| framed.window == *window) else {
            return;
        };
        let framed = &mut self.framed[index];
        if framed.fills() == Fills::Nothing {
            return;
        }
        let Some(toplevel) = framed.window.toplevel() else { return };
        let bar = TitleBar::of(window).told();
        let size = framed.restore.frame.map(|frame| inside(&theme, Shape::Normal, bar, frame));
        toplevel.with_pending_state(|state| {
            state.states.unset(State::Maximized);
            tiled(&mut state.states, None);
            state.size = size;
        });
        toplevel.send_pending_configure();
        // A window that is drawn maximized goes there once its program
        // has drawn it with a frame's sides again.
        if framed.zone.take().is_some() && framed.layout.shape == Shape::Normal {
            let at = core::mem::take(&mut framed.restore).at;
            self.place(index, at);
            self.put(index);
        }
    }

    /// The width of the frame of `window`, which is maximized or
    /// snapped, once it
    /// is restored, if its program does not choose it.
    pub fn restored_width(&self, window: &Window) -> Option<i32> {
        Some(self.framed(window)?.restore.frame?.w)
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
    /// cannot. A snapped window is snapped no more: it is where it is,
    /// as a window the person put there.
    pub fn resize_from(&mut self, window: &Window, edge: Edge) -> bool {
        let Some(framed) = self.framed_mut(window) else { return false };
        if framed.maximized() || framed.minimized {
            return false;
        }
        if framed.zone.take().is_some() {
            framed.restore = Restore::default();
            if let Some(toplevel) = framed.window.toplevel() {
                // The program is told with the resize's first size.
                toplevel.with_pending_state(|state| tiled(&mut state.states, None));
            }
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
        let bar = TitleBar::of(window).told();
        let margins = Margins::of(&theme, Shape::Normal, bar);
        let wanted = framed.fitted(&theme, bar, margins.within((frame.w, frame.h)));
        let (Some(resize), Some(toplevel)) = (&mut framed.resize, framed.window.toplevel()) else {
            return;
        };
        resize.want(toplevel, wanted);
    }

    /// The program of `window` was told that its title bar changes
    /// hands, and that its surface is to be `grows` higher for it: a
    /// resize that is under way is for a surface as much higher.
    pub fn bar_told(&mut self, window: &Window, grows: i32) {
        if let Some(resize) = self.framed_mut(window).and_then(|framed| framed.resize.as_mut()) {
            resize.grow(grows);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_frame_has_a_place_with_no_screen() {
        // Twice what is kept of a bar is more than the frame and no
        // screen have between them.
        let windows = Windows::default();
        assert_eq!(windows.reachable((30, 20).into(), 60), Point::from((-KEPT, 0)));
    }
}
