// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the pointer and the keys do to the windows themselves: moving
//! and resizing them, their buttons, the window menu and the list of
//! windows (ADR 0721), and snapping them to the sides of a screen
//! (ADR 0727).
//!
//! While windowd does any of that, it has a [`Hold`]: the pointer is
//! over no program's surface, as the programs see it, and for a menu,
//! a list and a move or a resize by the keys, the keys reach none
//! either. A hold begins with a press on a frame, a key of windowd's
//! or a program's request to be moved or resized, and ends with the
//! button's release, a choice, Esc, or the window's going.
//!
//! A window dragged by its title bar to an edge of the desktop shows a
//! preview of the side, the part of one or the whole screen it fills
//! when it is let go there. After a snap, the other windows of the
//! desktop are offered for the largest zone left, as a list in it.
//! A pointer that rests on a maximize button is shown the picker of
//! layouts, to choose a zone for the window from.
//!
//! The [`Manager`] is told each press, release, motion and key by the
//! desktop, which then gives the programs what is left for them.
//! Nothing here waits or draws.

use input::abi::{
    BTN_LEFT, BTN_RIGHT, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_KPENTER, KEY_LEFT, KEY_RIGHT, KEY_SPACE,
    KEY_TAB, KEY_UP,
};
use smithay::desktop::Window;
use smithay::utils::{Logical, Point};
use window_frame::panel::Mark;
use window_frame::{Button, Edge, Part};

use smithay::utils::Rectangle;

use crate::keys::Ask;
use crate::panel::{Panel, Row};
use crate::picker::Picker;
use crate::pointer::Picture;
use crate::scene::{Over, Preview, Shade};
use crate::settings::Settings;
use crate::shell::{Shell, app_of};
use crate::titlebar::TitleBar;
use crate::windows::zones::{self, Fills, Then, Zone};
use crate::windows::{Desk, Framed, Restore, Split, Under};

/// How far the pointer moves with a button held on a title bar before
/// the window follows it, and how far apart the two presses of a double
/// click may be.
const SLOP: f64 = 4.0;

/// How long the pointer rests on a maximize button before the picker
/// of layouts shows.
const REST_NS: u64 = 500_000_000;

/// How far an arrow key moves a window or its edges, and how far with
/// Ctrl held.
const STEPS: (i32, i32) = (16, 1);

/// What a line of the window menu does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Restore,
    Move,
    Size,
    Minimize,
    Maximize,
    /// Whether the window's application has windowd's full title bar
    /// whatever it asks.
    FullBar,
    Close,
}

/// The window menu's rows: what each does, or `None` for the room
/// between two groups, and what it says. The row of
/// [`Item::FullBar`], with the room after it, is in the menu of a
/// window whose program asks to share the title bar, and of no other.
const MENU: [(Option<Item>, &str, &str); 9] = [
    (Some(Item::Restore), "Restore", ""),
    (Some(Item::Move), "Move", ""),
    (Some(Item::Size), "Size", ""),
    (Some(Item::Minimize), "Minimize", ""),
    (Some(Item::Maximize), "Maximize", ""),
    (None, "", ""),
    (Some(Item::FullBar), "Full title bar for this app", ""),
    (None, "", ""),
    (Some(Item::Close), "Close", "Alt+F4"),
];

/// What a line of the list says for a window without a title.
const UNTITLED: &str = "Window";

/// What a program asked of the window manager.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// To have the pointer move its window, with the button of this
    /// code held.
    Move(Window, u16),
    /// To have the pointer resize its window by an edge, with the
    /// button of this code held.
    Resize(Window, Edge, u16),
    /// The window menu, at a place of the desktop.
    Menu(Window, Point<i32, Logical>),
}

/// What windowd has the pointer or the keys for.
#[derive(Debug)]
enum Hold {
    /// Nothing: they are the programs'.
    None,
    /// A button of a frame is held down.
    Button { window: Window, button: Button },
    /// A window is moved by the pointer, with the button `code` held.
    /// `grip` is where in the frame's box the pointer has hold of it,
    /// once it has moved far enough from `pressed` for the window to
    /// follow, and `was` where the window was then. `to` is what it
    /// fills if it is let go where the pointer is, and the area of
    /// that.
    Move {
        window: Window,
        code: u16,
        pressed: Point<f64, Logical>,
        grip: Option<Point<f64, Logical>>,
        was: Option<Restore>,
        to: Option<(Fills, Rectangle<i32, Logical>)>,
    },
    /// A window is resized by the pointer, which was at `pressed` when
    /// it began.
    Resize { window: Window, edge: Edge, code: u16, pressed: Point<f64, Logical> },
    /// The pointer, which was at `pressed` when it began, moves where
    /// the zones of a screen meet, by the edge `edge` of a snapped
    /// window.
    Split { split: Split, edge: Edge, code: u16, pressed: Point<f64, Logical> },
    /// The window menu shows: `items` is what each row does.
    Menu { window: Window, panel: Panel, items: Vec<Option<Item>> },
    /// The list of windows shows: `listed` is the window of each row.
    /// With `held`, letting Alt go chooses.
    List { panel: Panel, listed: Vec<Window>, held: bool },
    /// The windows that are not snapped are offered for `zone` of the
    /// screen the place `at` is on, as a list in it: `listed` is the
    /// window of each row.
    Offer { panel: Panel, listed: Vec<Window>, zone: Zone, at: Point<f64, Logical> },
    /// The picker of layouts shows for `window`, until the pointer
    /// chooses a zone or leaves `around`, the rectangle around the
    /// picker and the button it shows under.
    Picker { window: Window, picker: Picker, around: Rectangle<i32, Logical> },
    /// The arrow keys move a window, or resize it, from where its
    /// frame's corner was at `start`; `by` is how far they have.
    Keys { window: Window, size: bool, start: Point<i32, Logical>, by: Point<i32, Logical> },
}

/// A press on a title bar, which a second makes a double click.
#[derive(Debug)]
struct Click {
    window: Window,
    at: Point<f64, Logical>,
    time_ns: u64,
}

/// The window manager's state over the seat.
#[derive(Debug)]
pub struct Manager {
    hold: Hold,
    clicked: Option<Click>,
    /// How long after a press a second makes a double click
    /// (windowd.json(5)).
    pub double_click_ns: u64,
    /// What the renderer knows the preview of a snap by.
    shade: Shade,
    /// The window whose maximize button the pointer is on with nothing
    /// held, and since when.
    rest: Option<(Window, u64)>,
}

impl Default for Manager {
    fn default() -> Manager {
        let double_click_ns = u64::from(Settings::default().double_click_ms) * 1_000_000;
        let shade = Shade::default();
        Manager { hold: Hold::None, clicked: None, double_click_ns, shade, rest: None }
    }
}

/// A button of a window's frame.
pub type OnButton<'a> = Option<(&'a Window, Button)>;

/// How far `one` is from `other`.
fn apart(one: Point<f64, Logical>, other: Point<f64, Logical>) -> f64 {
    (one.x - other.x).hypot(one.y - other.y)
}

impl Manager {
    /// Whether windowd keeps the pointer from the programs now.
    pub fn takes_pointer(&self) -> bool {
        !matches!(self.hold, Hold::None)
    }

    /// Whether windowd keeps the keys from the programs now.
    pub fn takes_keys(&self) -> bool {
        let keys = matches!(self.hold, Hold::Menu { .. } | Hold::List { .. } | Hold::Keys { .. });
        keys || self.offers()
    }

    /// Whether the windows are offered for a zone now. A key of the
    /// window manager's own asks what it does at any time then, and the
    /// offer goes for it.
    pub fn offers(&self) -> bool {
        matches!(self.hold, Hold::Offer { .. })
    }

    /// The box that shows over the windows: a panel or the picker.
    pub fn over(&self) -> Option<Over<'_>> {
        match &self.hold {
            Hold::Menu { panel, .. } | Hold::List { panel, .. } | Hold::Offer { panel, .. } => {
                Some(Over { at: panel.area().loc, pixels: panel.pixels() })
            }
            Hold::Picker { picker, .. } => {
                Some(Over { at: picker.area().loc, pixels: picker.pixels() })
            }
            _ => None,
        }
    }

    /// The pointer is over `under` at `time_ns`: it rests on a
    /// maximize button from when it came to it, with nothing held.
    pub fn rests(&mut self, under: &Under, time_ns: u64) {
        let on = match (&self.hold, under) {
            (Hold::None, Under::Part { window, part: Part::Button(Button::Maximize) }) => {
                Some(window)
            }
            _ => None,
        };
        let since = self.rest.take().filter(|(window, _)| Some(window) == on);
        self.rest = since.or_else(|| Some((on?.clone(), time_ns)));
    }

    /// When the pointer has rested long enough for the picker to show,
    /// on the monotonic clock, if it rests on a maximize button.
    pub fn due(&self) -> Option<u64> {
        let (_, since) = self.rest.as_ref()?;
        Some(since.saturating_add(REST_NS))
    }

    /// It is `now_ns`: the picker of layouts shows under the maximize
    /// button the pointer has rested on long enough. Returns whether
    /// it does.
    pub fn rested(&mut self, shell: &mut Shell, now_ns: u64) -> bool {
        if self.due().is_none_or(|due| due > now_ns) {
            return false;
        }
        let Some((window, _)) = self.rest.take() else { return false };
        if !matches!(self.hold, Hold::None) {
            return false;
        }
        let Some(framed) = shell.windows.framed(&window) else { return false };
        let button = framed.layout().button(Button::Maximize);
        let button = Rectangle::new(
            framed.at() + Point::from((button.x, button.y)),
            (button.w, button.h).into(),
        );
        let screen = shell.windows.screen_at(button.loc.to_f64());
        let picker = Picker::new(shell.windows.theme(), button, screen);
        let around = button.merge(picker.area());
        self.hold = Hold::Picker { window, picker, around };
        shell.changed.windows = true;
        true
    }

    /// The preview of what the window that is dragged fills if it is
    /// let go now.
    pub fn preview(&self) -> Option<Preview<'_>> {
        match &self.hold {
            Hold::Move { window, to: Some((_, area)), .. } => {
                Some(Preview { window, area: *area, shade: &self.shade })
            }
            _ => None,
        }
    }

    /// The button of a frame the pointer is on, and the one that is
    /// held down and shows so, when the pointer is over `under`.
    pub fn buttons<'a>(&'a self, under: &'a Under) -> (OnButton<'a>, OnButton<'a>) {
        let over = match under {
            Under::Part { window, part: Part::Button(button) } => Some((window, *button)),
            _ => None,
        };
        match &self.hold {
            Hold::None => (over, None),
            Hold::Button { window, button } => {
                let on = over.filter(|over| *over == (window, *button));
                (on, on)
            }
            // The button the picker shows under is lit while it does.
            Hold::Picker { window, .. } => (Some((window, Button::Maximize)), None),
            _ => (None, None),
        }
    }

    /// The picture windowd shows as the pointer over `under`, or `None`
    /// for the picture of the program whose surface is there.
    pub fn picture(&self, under: &Under) -> Option<Picture> {
        match (&self.hold, under) {
            (Hold::Resize { edge, .. } | Hold::Split { edge, .. }, _) => Some(Picture::of(*edge)),
            (Hold::None, Under::Part { part: Part::Edge(edge), .. }) => Some(Picture::of(*edge)),
            (Hold::None, Under::Surface { .. }) => None,
            _ => Some(Picture::Arrow),
        }
    }

    /// Ends the hold. A resize that was under way is over.
    fn let_go(&mut self, shell: &mut Shell) {
        match core::mem::replace(&mut self.hold, Hold::None) {
            Hold::Resize { window, .. } | Hold::Keys { window, size: true, .. } => {
                shell.windows.resize_done(&window);
            }
            Hold::Split { split, .. } => shell.windows.split_done(&split),
            _ => {}
        }
        shell.changed.windows = true;
    }

    /// Ends a hold whose window is gone or off the screens.
    pub fn check(&mut self, shell: &mut Shell) {
        let there = |window: &Window| shell.windows.framed(window).is_some();
        let shows = |window: &Window| {
            shell.windows.framed(window).is_some_and(|framed| !framed.minimized())
        };
        let holds = match &self.hold {
            Hold::None | Hold::Split { .. } => true,
            Hold::Button { window, .. }
            | Hold::Move { window, .. }
            | Hold::Resize { window, .. }
            | Hold::Menu { window, .. }
            | Hold::Picker { window, .. }
            | Hold::Keys { window, .. } => shows(window),
            Hold::List { listed, .. } | Hold::Offer { listed, .. } => listed.iter().all(there),
        };
        if !holds {
            self.let_go(shell);
        }
    }

    /// A button of Linux code `code` went down at `at`, which is over
    /// `under`, at `time_ns`.
    pub fn press(
        &mut self,
        shell: &mut Shell,
        under: &Under,
        at: Point<f64, Logical>,
        code: u16,
        time_ns: u64,
    ) {
        self.rest = None;
        // A press on a target of the picker waits for the release; any
        // other puts the picker away, and is what it is without it.
        if let Hold::Picker { picker, .. } = &self.hold {
            if picker.zone_at(at).is_some() {
                return;
            }
            self.let_go(shell);
        }
        match &mut self.hold {
            Hold::None => {}
            Hold::Menu { panel, .. } | Hold::List { panel, .. } | Hold::Offer { panel, .. } => {
                // A press beside the panel puts it away. One in it
                // chooses the line it is on, which the release does.
                if !panel.holds(at) {
                    self.let_go(shell);
                } else if panel.choose(shell.windows.theme(), panel.row_at(at)) {
                    shell.changed.windows = true;
                }
                return;
            }
            // A press keeps what the keys did.
            Hold::Keys { .. } => return self.let_go(shell),
            _ => return,
        }
        let Some(window) = under.window().cloned() else { return };
        shell.focus(Some(&window));
        let Under::Part { part, .. } = under else { return };
        match (code, *part) {
            (BTN_LEFT, Part::Title) => {
                let second = self.clicked.take().is_some_and(|first| {
                    first.window == window
                        && apart(first.at, at) <= SLOP
                        && time_ns.saturating_sub(first.time_ns) <= self.double_click_ns
                });
                if second {
                    toggle(shell, &window);
                } else {
                    self.clicked = Some(Click { window: window.clone(), at, time_ns });
                    self.hold = moved(window, code, at);
                }
            }
            (BTN_LEFT, Part::Button(button)) => self.hold = Hold::Button { window, button },
            (BTN_LEFT, Part::Edge(edge)) => self.by_edge(shell, window, edge, code, at),
            (BTN_RIGHT, Part::Title) => self.menu(shell, window, at.to_i32_floor()),
            _ => {}
        }
    }

    /// A button of Linux code `code` went up at `at`, which is over
    /// `under`.
    pub fn release(
        &mut self,
        shell: &mut Shell,
        under: &Under,
        at: Point<f64, Logical>,
        code: u16,
    ) {
        match &self.hold {
            Hold::Button { window, button } if code == BTN_LEFT => {
                let (window, button) = (window.clone(), *button);
                let on =
                    *under == Under::Part { window: window.clone(), part: Part::Button(button) };
                self.let_go(shell);
                if on {
                    match button {
                        Button::Minimize => shell.minimize(&window),
                        Button::Maximize => toggle(shell, &window),
                        Button::Close => close(&window),
                    }
                }
            }
            Hold::Move { window, code: held, was, to, .. } if *held == code => {
                let dropped = to.map(|(fills, _)| (window.clone(), fills, *was));
                self.let_go(shell);
                if let Some((window, fills, was)) = dropped {
                    shell.windows.drop_at(&window, at, fills, was);
                    self.offer(shell, &window);
                }
            }
            Hold::Resize { code: held, .. } | Hold::Split { code: held, .. } if *held == code => {
                self.let_go(shell);
            }
            Hold::Picker { window, picker, .. } if code == BTN_LEFT => {
                let Some(zone) = picker.zone_at(at) else { return };
                let window = window.clone();
                self.let_go(shell);
                shell.windows.fill(&window, Fills::Zone(zone));
                self.offer(shell, &window);
            }
            Hold::Menu { panel, .. } | Hold::List { panel, .. } | Hold::Offer { panel, .. }
                if code == BTN_LEFT =>
            {
                if let Some(row) = panel.row_at(at) {
                    self.choose(shell, row);
                }
            }
            _ => {}
        }
    }

    /// The pointer moved to `at`.
    pub fn motion(&mut self, shell: &mut Shell, at: Point<f64, Logical>) {
        match &mut self.hold {
            Hold::Move { window, pressed, grip, was, to, .. } => {
                let Some(framed) = shell.windows.framed(window) else { return };
                let frame = framed.frame();
                if grip.is_none() && apart(*pressed, at) >= SLOP {
                    *was = shell.windows.restores_to(window);
                }
                let hold = match *grip {
                    Some(grip) => grip,
                    None if apart(*pressed, at) < SLOP => return,
                    None if framed.fills() != Fills::Nothing => {
                        // The window is restored under the pointer, at
                        // the same share of its width.
                        let share = (pressed.x - f64::from(frame.loc.x)) / f64::from(frame.size.w);
                        let width = shell.windows.restored_width(window).unwrap_or(frame.size.w);
                        let within = (share * f64::from(width), pressed.y - f64::from(frame.loc.y));
                        shell.windows.restore(window);
                        Point::from(within)
                    }
                    None => *pressed - frame.loc.to_f64(),
                };
                *grip = Some(hold);
                self.clicked = None;
                shell.windows.move_to(window, (at - hold).to_i32_round());
                *to = shell.windows.dropped_at(at);
                shell.changed.windows = true;
            }
            Hold::Resize { window, pressed, .. } => {
                shell.windows.resize_by(window, (at - *pressed).to_i32_round());
                shell.changed.windows = true;
            }
            Hold::Split { split, pressed, .. } => {
                shell.windows.split_to(split, (at - *pressed).to_i32_round());
                shell.changed.windows = true;
            }
            Hold::Picker { picker, around, .. } => {
                if !around.to_f64().contains(at) {
                    self.let_go(shell);
                } else if picker.point(shell.windows.theme(), at) {
                    shell.changed.windows = true;
                }
            }
            Hold::Menu { panel, .. } | Hold::List { panel, .. } | Hold::Offer { panel, .. } => {
                // The pointer chooses the line it comes to, and leaves
                // the keys' choice when it is on none.
                let row = panel.row_at(at);
                if row.is_some() && panel.choose(shell.windows.theme(), row) {
                    shell.changed.windows = true;
                }
            }
            _ => {}
        }
    }

    /// A key that is no modifier, of Linux code `code`, went down while
    /// windowd has the keys, with Ctrl or Shift held or not.
    pub fn key(&mut self, shell: &mut Shell, code: u16, ctrl: bool, shift: bool) {
        let theme = *shell.windows.theme();
        let offers = self.offers();
        match &mut self.hold {
            Hold::Menu { panel, .. } | Hold::List { panel, .. } | Hold::Offer { panel, .. } => {
                let back = match code {
                    KEY_UP | KEY_LEFT => Some(true),
                    KEY_DOWN | KEY_RIGHT => Some(false),
                    KEY_TAB => Some(shift),
                    _ => None,
                };
                if let Some(back) = back {
                    panel.step(&theme, back);
                    shell.changed.windows = true;
                    return;
                }
                let chosen = panel.chosen();
                match (code, chosen) {
                    (KEY_ENTER | KEY_KPENTER | KEY_SPACE, Some(row)) => self.choose(shell, row),
                    (KEY_ESC, _) => self.let_go(shell),
                    // Any other key puts an offer away.
                    _ if offers => self.let_go(shell),
                    _ => {}
                }
            }
            Hold::Keys { window, size, start, by } => {
                let step = if ctrl { STEPS.1 } else { STEPS.0 };
                let moved = match code {
                    KEY_LEFT => (-step, 0),
                    KEY_RIGHT => (step, 0),
                    KEY_UP => (0, -step),
                    KEY_DOWN => (0, step),
                    KEY_ESC => (-by.x, -by.y),
                    KEY_ENTER | KEY_KPENTER => return self.let_go(shell),
                    _ => return,
                };
                *by += Point::from(moved);
                if *size {
                    shell.windows.resize_by(window, *by);
                } else {
                    let width =
                        shell.windows.framed(window).map_or(0, |framed| framed.frame().size.w);
                    let to = shell.windows.reachable(*start + *by, width);
                    *by = to - *start;
                    shell.windows.move_to(window, to);
                }
                shell.changed.windows = true;
                if code == KEY_ESC {
                    self.let_go(shell);
                }
            }
            _ => {}
        }
    }

    /// Alt was let go: a list that waited for that has its choice.
    pub fn alt_let_go(&mut self, shell: &mut Shell) {
        if let Hold::List { panel, held: true, .. } = &self.hold {
            match panel.chosen() {
                Some(row) => self.choose(shell, row),
                None => self.let_go(shell),
            }
        }
    }

    /// A key of windowd's asked `ask`, with the pointer at `at`.
    pub fn ask(&mut self, shell: &mut Shell, ask: Ask, at: Point<f64, Logical>) {
        let theme = *shell.windows.theme();
        if self.offers() {
            self.let_go(shell);
        }
        match (ask, &mut self.hold) {
            (Ask::Next | Ask::Previous, Hold::List { panel, .. }) => {
                panel.step(&theme, ask == Ask::Previous);
                shell.changed.windows = true;
            }
            (Ask::Next | Ask::Previous, Hold::None) => {
                self.list(shell, at, Some(ask == Ask::Previous));
            }
            (Ask::List, Hold::List { .. }) => self.let_go(shell),
            (Ask::List, Hold::None) => self.list(shell, at, None),
            (Ask::DeskNew, Hold::None) => shell.desk(Desk::New, false),
            (Ask::DeskClose, Hold::None) => shell.desk(Desk::Close, false),
            (Ask::DeskStep { back, along }, Hold::None) => shell.desk(Desk::Step { back }, along),
            (Ask::Close | Ask::Menu | Ask::Arrow(_) | Ask::ToScreen { .. }, Hold::None) => {
                let Some(window) = shell.active().cloned() else { return };
                let Some(framed) = shell.windows.framed(&window) else { return };
                match ask {
                    Ask::Close => close(&window),
                    Ask::Menu => {
                        let under_bar = framed.at() + Point::from((0, framed.layout().bar_rows()));
                        self.menu(shell, window, under_bar);
                    }
                    Ask::ToScreen { back } => shell.windows.next_screen(&window, back),
                    Ask::Arrow(arrow) => match zones::after(framed.fills(), arrow) {
                        Then::Fill(fills) => {
                            shell.windows.fill(&window, fills);
                            if matches!(fills, Fills::Zone(_)) {
                                self.offer(shell, &window);
                            }
                        }
                        Then::Minimize => shell.minimize(&window),
                        Then::Stay => {}
                    },
                    _ => {}
                }
                shell.changed.windows = true;
            }
            _ => {}
        }
    }

    /// A program asked `asked`, with the pointer at `at`.
    pub fn asked(&mut self, shell: &mut Shell, asked: Asked, at: Point<f64, Logical>) {
        if !matches!(self.hold, Hold::None) {
            return;
        }
        match asked {
            // A full-screen window has no place to be moved to.
            Asked::Move(window, _) if shell.windows.fills(&window) == Fills::Screen => {}
            Asked::Move(window, code) => self.hold = moved(window, code, at),
            Asked::Resize(window, edge, code) => self.by_edge(shell, window, edge, code, at),
            Asked::Menu(window, at) => self.menu(shell, window, at),
        }
        shell.changed.windows = true;
    }

    /// The edge `edge` of `window` is taken hold of with the button
    /// `code`, the pointer at `at`: the edge of a snapped window where
    /// its zone meets the others moves the splits, and any other
    /// resizes the window.
    fn by_edge(
        &mut self,
        shell: &mut Shell,
        window: Window,
        edge: Edge,
        code: u16,
        at: Point<f64, Logical>,
    ) {
        if let Some(split) = shell.windows.split_from(&window, edge) {
            self.hold = Hold::Split { split, edge, code, pressed: at };
        } else if shell.windows.resize_from(&window, edge) {
            self.hold = Hold::Resize { window, edge, code, pressed: at };
        }
    }

    /// Offers the windows that are not snapped for the largest zone
    /// that is free beside `window`, which was snapped just now: a list
    /// of them in the zone, if there is such a zone and such a window.
    fn offer(&mut self, shell: &mut Shell, window: &Window) {
        let Some((zone, area, others)) = shell.windows.offered(window) else { return };
        let listed: Vec<Window> = others.iter().map(|framed| framed.window.clone()).collect();
        let rows: Vec<Row> = others.iter().map(|framed| row_of(framed)).collect();
        let theme = shell.windows.theme();
        let width = theme.panel.list_width.min(area.size.w - 2 * theme.panel.padding);
        let panel = Panel::in_the_middle(theme, width, rows, Some(0), area);
        let at = area.loc.to_f64() + area.size.to_f64().downscale(2.0).to_point();
        self.hold = Hold::Offer { panel, listed, zone, at };
        shell.changed.windows = true;
    }

    /// Shows the window menu of `window` with its corner at `at`.
    fn menu(&mut self, shell: &mut Shell, window: Window, at: Point<i32, Logical>) {
        let Some(framed) = shell.windows.framed(&window) else { return };
        let fills = framed.fills();
        // An application of no name has no entry in the settings to
        // change.
        let app = window.toplevel().map(app_of).filter(|app| !app.is_empty());
        let shares = app.filter(|_| TitleBar::of(&window).asked());
        let full = shares.as_ref().map(|app| !shell.title_bars.lets(app));
        let mut items = Vec::new();
        let mut rows = Vec::new();
        for (index, (item, words, key)) in MENU.iter().enumerate() {
            let after_bar =
                index.checked_sub(1).is_some_and(|before| MENU[before].0 == Some(Item::FullBar));
            if (*item == Some(Item::FullBar) || after_bar) && full.is_none() {
                continue;
            }
            items.push(*item);
            rows.push(match item {
                None => Row::Gap,
                Some(item) => {
                    // A window that fills a zone or the screen is
                    // restored first to be moved or resized.
                    let off = match item {
                        Item::Restore => fills == Fills::Nothing,
                        Item::Move | Item::Size => fills != Fills::Nothing,
                        Item::Maximize => matches!(fills, Fills::All | Fills::Screen),
                        Item::Minimize | Item::FullBar | Item::Close => false,
                    };
                    let ticked = *item == Item::FullBar && full == Some(true);
                    let mark = if ticked { Mark::Tick } else { Mark::None };
                    Row::Line { words: (*words).to_owned(), key, mark, off }
                }
            });
        }
        let theme = shell.windows.theme();
        let screen = shell.windows.screen_at(at.to_f64());
        let panel = Panel::new(theme, theme.panel.menu_width, rows, None, at, screen);
        self.hold = Hold::Menu { window, panel, items };
        shell.changed.windows = true;
    }

    /// Shows the list of windows on the screen the pointer, at `at`, is
    /// on. With `back` it is one that Alt holds open, with the window
    /// used before the present one chosen, or for `back` the one used
    /// longest ago.
    fn list(&mut self, shell: &mut Shell, at: Point<f64, Logical>, back: Option<bool>) {
        let all = shell.windows.listed();
        if all.is_empty() {
            return;
        }
        let rows: Vec<Row> = all.iter().map(|framed| row_of(framed)).collect();
        let listed: Vec<Window> = all.iter().map(|framed| framed.window.clone()).collect();
        let chosen = match back {
            Some(true) => listed.len() - 1,
            Some(false) => 1.min(listed.len() - 1),
            None => 0,
        };
        let theme = shell.windows.theme();
        let screen = shell.windows.screen_at(at);
        let width = theme.panel.list_width;
        let panel = Panel::in_the_middle(theme, width, rows, Some(chosen), screen);
        self.hold = Hold::List { panel, listed, held: back.is_some() };
        shell.changed.windows = true;
    }

    /// The person chose the row `row` of the panel that shows.
    fn choose(&mut self, shell: &mut Shell, row: usize) {
        match core::mem::replace(&mut self.hold, Hold::None) {
            Hold::List { listed, .. } => shell.focus(listed.get(row)),
            Hold::Offer { listed, zone, at, .. } => {
                let Some(window) = listed.get(row) else { return };
                shell.windows.drop_at(window, at, Fills::Zone(zone), None);
                shell.focus(Some(window));
                self.offer(shell, window);
            }
            Hold::Menu { window, items, .. } => match items.get(row).copied().flatten() {
                Some(Item::Restore) => shell.windows.restore(&window),
                Some(Item::Maximize) => shell.windows.maximize(&window),
                Some(Item::Minimize) => shell.minimize(&window),
                Some(Item::Close) => close(&window),
                Some(Item::FullBar) => {
                    let Some(app) = window.toplevel().map(app_of) else { return };
                    let lets = shell.title_bars.lets(&app);
                    shell.title_bars.set(&app, !lets);
                    shell.settle_bars();
                    shell.chosen.push((app, !lets));
                }
                Some(item @ (Item::Move | Item::Size)) => {
                    let size = item == Item::Size;
                    let Some(start) = shell.windows.framed(&window).map(Framed::at) else {
                        return;
                    };
                    if !size || shell.windows.resize_from(&window, Edge::BottomRight) {
                        self.hold = Hold::Keys { window, size, start, by: Point::default() };
                    }
                }
                None => {}
            },
            other => self.hold = other,
        }
        shell.changed.windows = true;
    }
}

/// The hold of `window` by its title bar, with the button `code` held
/// and the pointer at `at`.
fn moved(window: Window, code: u16, at: Point<f64, Logical>) -> Hold {
    Hold::Move { window, code, pressed: at, grip: None, was: None, to: None }
}

/// The row of a list for `framed`.
fn row_of(framed: &Framed) -> Row {
    let title = if framed.title().is_empty() { UNTITLED } else { framed.title() };
    Row::Line { words: title.to_owned(), key: "", mark: Mark::Icon, off: false }
}

/// Maximizes `window`, or restores it if it is maximized.
fn toggle(shell: &mut Shell, window: &Window) {
    if shell.windows.framed(window).is_some_and(Framed::maximized) {
        shell.windows.restore(window);
    } else {
        shell.windows.maximize(window);
    }
}

/// Asks the program of `window` to close it.
fn close(window: &Window) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_close();
    }
}
