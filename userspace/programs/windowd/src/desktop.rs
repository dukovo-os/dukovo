// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The desktop: the programs' windows on the screens, and the keys and
//! the pointer that reach them.
//!
//! [`Desktop`] is told what happened, one thing at a time: a program
//! wrote, a device said something, the display layer has an event, the
//! time to draw has come. It waits for nothing itself, so `main` waits
//! for all of it in one place, and the tests tell it each thing in the
//! order they choose.
//!
//! Each press, motion and key is the window manager's first
//! ([`crate::manager`]): what it does not keep reaches the program
//! whose surface is under the pointer, or which has the keyboard.

use std::io;
use std::os::fd::BorrowedFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use display::Holder;
use display::abi::DisplayEvent;
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::backend::renderer::Bind;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::input::keyboard::{FilterResult, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::Display;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};
use sysapi::time::monotonic_ns;

use crate::clients::Clients;
use crate::curve::Curve;
use crate::keys::{self, Ask, Chord};
use crate::link::Link;
use crate::manager::Manager;
use crate::pointer::{Arrows, Picture, Shown};
use crate::scene::Scene;
use crate::screens::{Failed, Pointing, Screens, Told};
use crate::settings::Settings;
use crate::shell::Shell;
use crate::windows::Under;

/// How far one notch of a wheel scrolls, as the programs reckon it.
const NOTCH: f64 = 15.0;

/// How many frames in a row may fail to be drawn before the desktop
/// ends. One fails when a program's pixels cannot be read, and that
/// program is gone before the next is drawn; frames that go on failing
/// fail for a reason that letting a program go does not mend.
const SPOILED_MOST: u8 = 3;

/// What a key that reaches no program is windowd's for.
#[derive(Clone, Copy, Debug)]
enum Kept {
    /// A chord that `main` sees to.
    Chord(Chord),
    /// A key of the window manager's.
    Ask(Ask),
    /// A key while the window manager has the keys, with Ctrl and Shift
    /// held or not.
    Key { ctrl: bool, shift: bool },
}

/// A time of the monotonic clock as the protocols carry it: in
/// milliseconds, the low 32 bits.
fn ms(time_ns: u64) -> u32 {
    (time_ns / 1_000_000) as u32
}

/// The desktop.
pub struct Desktop<L: Link>
where
    PixmanRenderer: Bind<L::Pixels>,
{
    display: Display<Shell>,
    shell: Shell,
    clients: Clients,
    renderer: PixmanRenderer,
    screens: Screens<L>,
    manager: Manager,
    /// What is under the pointer.
    under: Under,
    /// windowd's own pictures of the pointer.
    pictures: Arrows,
    /// Which of them the pointer shows, or `None` for the picture of
    /// the program whose surface is under it.
    own: Option<Picture>,
    /// Counts the pictures the pointer has shown.
    pointer_picture: u64,
    curve: Curve,
    /// Whether a Super key is down and nothing else was pressed since:
    /// letting it go shows the list of windows.
    super_alone: bool,
    /// The keys whose press was a chord of windowd's: their release
    /// reaches no program either.
    kept: Vec<u16>,
    /// The pointers' buttons that are held.
    held: Vec<u16>,
    /// What a chord asked for, until `main` takes it.
    asked: Option<Chord>,
    /// How many frames in a row could not be drawn.
    spoiled: u8,
}

impl<L: Link> Desktop<L>
where
    PixmanRenderer: Bind<L::Pixels>,
{
    /// A desktop on the screens `holder` reaches, which is in front,
    /// whose keyboard has the consoles' layout `layout`.
    ///
    /// # Errors
    ///
    /// What the layer's requests fail with, what the renderer does, and
    /// [`Failed::Start`] when the protocols could not be set up.
    pub fn new(holder: Holder<L>, layout: &str) -> Result<Desktop<L>, Failed> {
        let start =
            |what: &str, why: &dyn core::fmt::Display| Failed::Start(format!("{what}: {why}"));
        let display = Display::new().map_err(|err| start("the Wayland display", &err))?;
        let mut shell = Shell::new(&display.handle());
        let (layout, variant) = keys::xkb_names(layout);
        let named = XkbConfig { layout, variant, ..XkbConfig::default() };
        // As built, until the person's settings are laid
        // ([`Desktop::set`]).
        let as_built = Settings::default();
        let (delay, rate) = (as_built.repeat_delay_ms as i32, as_built.repeat_rate as i32);
        // A layout the XKB data has not is the default's, which the
        // data always has.
        let added = shell.seat.add_keyboard(named, delay, rate);
        let added = added.or_else(|_| shell.seat.add_keyboard(XkbConfig::default(), delay, rate));
        added.map_err(|err| start("the keyboard's layout", &err))?;
        let mut renderer = PixmanRenderer::new()?;
        let mut screens = Screens::new(holder);
        screens.arrange(&mut renderer, &mut shell, &display.handle())?;
        let mut desktop = Desktop {
            display,
            shell,
            clients: Clients::default(),
            renderer,
            screens,
            manager: Manager::default(),
            under: Under::Nothing,
            pictures: Arrows::default(),
            own: Some(Picture::Arrow),
            pointer_picture: 0,
            curve: Curve::default(),
            super_alone: false,
            kept: Vec::new(),
            held: Vec::new(),
            asked: None,
            spoiled: 0,
        };
        desktop.centre();
        Ok(desktop)
    }

    /// Has the desktop be as `settings`, the person's, say
    /// (windowd.json(5)): from here on, and for the windows and the
    /// programs that are there.
    pub fn set(&mut self, settings: &Settings) {
        self.curve.set_speed(settings.pointer_speed);
        self.manager.double_click_ns = u64::from(settings.double_click_ms) * 1_000_000;
        if let Some(keyboard) = self.shell.seat.get_keyboard() {
            // The ranges a setting is held to fit both.
            let (rate, delay) = (settings.repeat_rate as i32, settings.repeat_delay_ms as i32);
            keyboard.change_repeat_info(rate, delay);
        }
        self.shell.title_bars = settings.title_bars.clone();
        self.shell.settle_bars();
        self.settle();
    }

    /// The applications the person chose, from a window's menu, to
    /// give the full title bar or to let share it, each with whether
    /// it may share, since this was last asked.
    pub fn chosen(&mut self) -> Vec<(String, bool)> {
        core::mem::take(&mut self.shell.chosen)
    }

    /// The holder of the screens.
    pub fn holder(&self) -> &Holder<L> {
        self.screens.holder()
    }

    /// Whether the desktop is in front.
    #[cfg(test)]
    pub fn front(&self) -> bool {
        self.screens.front()
    }

    /// Puts the pointer in the middle of the first screen.
    fn centre(&mut self) {
        let Some(first) = self.screens.first() else { return };
        let middle = first.loc + first.size.downscale(2).to_point();
        if let Some(pointer) = self.shell.seat.get_pointer() {
            pointer.set_location(middle.to_f64());
        }
        self.screens.point(middle.to_f64());
    }

    /// Takes the program in that connected as `stream`.
    ///
    /// # Errors
    ///
    /// As [`Clients::take_in`].
    pub fn take_in(&mut self, stream: UnixStream) -> io::Result<()> {
        self.clients.take_in(&self.display, stream)
    }

    /// What to wait on for each program, in an order that holds until
    /// the desktop is next told anything.
    pub fn waited_on(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        self.clients.waited_on()
    }

    /// The programs wrote whose places in [`Desktop::waited_on`]
    /// `wrote` names.
    pub fn programs_wrote(&mut self, wrote: impl IntoIterator<Item = usize>) {
        self.clients.hear(&mut self.display, &mut self.shell, wrote);
        self.settle();
    }

    /// Does what follows from whatever the desktop was told: sends the
    /// programs what is theirs, lets go of those that are gone, does
    /// what they asked of the window manager, and notes which screens
    /// are due a frame.
    fn settle(&mut self) {
        // A program that reads nothing or is gone fails here, and is
        // found gone just below.
        let _ = self.display.flush_clients();
        self.clients.let_go(&mut self.display, &mut self.shell);
        self.shell.refresh();
        if let Some(pointer) = self.shell.seat.get_pointer() {
            for asked in core::mem::take(&mut self.shell.asked) {
                self.manager.asked(&mut self.shell, asked, pointer.current_location());
            }
        }
        self.manager.check(&mut self.shell);
        let changed = core::mem::take(&mut self.shell.changed);
        if changed.windows {
            self.find_under();
        }
        let stale = self.look() | changed.windows | changed.pointer;
        if changed.pointer {
            self.pointer_picture += 1;
        }
        if stale {
            self.screens.stale();
        }
        // What the steps above sent, as the first flush.
        let _ = self.display.flush_clients();
    }

    /// Has the frames and the pointer show what they should now: the
    /// button under the pointer, the one held down, and the pointer's
    /// picture over a frame. Returns whether any changed.
    fn look(&mut self) -> bool {
        // While a program has the pointer for a button that is down,
        // the frames it passes over stay as they are.
        let programs = !self.held.is_empty() && !self.manager.takes_pointer();
        let nothing = Under::Nothing;
        let under = if programs { &nothing } else { &self.under };
        let (hot, pressed) = self.manager.buttons(under);
        let active = self.shell.active().cloned();
        let mut changed = self.shell.windows.redraw(active.as_ref(), hot, pressed);
        let own = if programs { None } else { self.manager.picture(&self.under) };
        if own != self.own {
            self.own = own;
            self.pointer_picture += 1;
            changed = true;
        }
        changed
    }

    /// The surface the pointer is over as the programs are told, and
    /// where its corner is: none while the window manager has the
    /// pointer, or over a frame.
    fn focus(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        match &self.under {
            Under::Surface { surface, at, .. } if !self.manager.takes_pointer() => {
                Some((surface.clone(), *at))
            }
            _ => None,
        }
    }

    /// Finds what is under the pointer now, and tells the program whose
    /// surface it is, if it was not before: windows came, went or
    /// changed under a pointer that did not move.
    fn find_under(&mut self) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let at = pointer.current_location();
        self.under = self.shell.windows.under(at);
        let focus = self.focus();
        if focus.as_ref().map(|(surface, _)| surface) == pointer.current_focus().as_ref() {
            return;
        }
        let (serial, now) = (SERIAL_COUNTER.next_serial(), monotonic_ns());
        let moved = MotionEvent { location: at, serial, time: ms(now) };
        let comes = focus.as_ref().map(|(surface, _)| surface);
        self.shell.stamps.pointer([comes, None], now);
        pointer.motion(&mut self.shell, focus, &moved);
        pointer.frame(&mut self.shell);
    }

    /// The display layer said `event`.
    ///
    /// # Errors
    ///
    /// What showing on screens that came fails with.
    pub fn layer_said(&mut self, event: &DisplayEvent) -> Result<(), Failed> {
        match self.screens.told(event) {
            Told::Nothing => {}
            Told::Shown(screen, time_ns) => {
                self.shell.frame_shown(&screen, Duration::from_nanos(time_ns));
            }
            Told::Front => self.put_pointer(),
            Told::Rearrange => {
                let display = self.display.handle();
                self.screens.arrange(&mut self.renderer, &mut self.shell, &display)?;
                self.put_pointer();
            }
        }
        self.settle();
        Ok(())
    }

    /// Puts the pointer where it is, on screens that are new or the
    /// desktop's again, or at the nearest place that is on one.
    fn put_pointer(&mut self) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let at = self.screens.within(pointer.current_location());
        pointer.set_location(at);
        self.screens.point(at);
        self.shell.changed.windows = true;
    }

    /// A key of a keyboard, of Linux code `code`, went down or up at
    /// `time_ns`.
    pub fn key(&mut self, time_ns: u64, code: u16, pressed: bool) {
        let Some(keyboard) = self.shell.seat.get_keyboard() else { return };
        let state = if pressed { KeyState::Pressed } else { KeyState::Released };
        let (kept, takes) = (&mut self.kept, self.manager.takes_keys());
        let serial = SERIAL_COUNTER.next_serial();
        // XKB's codes are Linux's and 8.
        let xkb = (u32::from(code) + 8).into();
        let (what, modifiers_changed) =
            keyboard.input_intercept(&mut self.shell, xkb, state, |_, held, _| {
                if !pressed {
                    let was_kept = kept.iter().position(|other| *other == code);
                    return match was_kept.map(|at| kept.swap_remove(at)) {
                        Some(_) => FilterResult::Intercept(None),
                        None => FilterResult::Forward,
                    };
                }
                let what = if let Some(chord) = keys::chord(held, code) {
                    Kept::Chord(chord)
                } else if keys::modifier(code) {
                    return FilterResult::Forward;
                } else if takes {
                    Kept::Key { ctrl: held.ctrl, shift: held.shift }
                } else if let Some(ask) = keys::ask(held, code) {
                    Kept::Ask(ask)
                } else {
                    return FilterResult::Forward;
                };
                kept.push(code);
                FilterResult::Intercept(Some(what))
            });
        let what = match what {
            FilterResult::Intercept(what) => what,
            // The key is the program's that has the keyboard.
            FilterResult::Forward => {
                self.shell.stamps.keys(keyboard.current_focus().as_ref(), time_ns);
                let (shell, time) = (&mut self.shell, ms(time_ns));
                keyboard.input_forward(shell, xkb, state, serial, time, modifiers_changed);
                None
            }
        };
        let at = self.shell.seat.get_pointer().map(|pointer| pointer.current_location());
        let at = at.unwrap_or_default();
        // Super pressed and let go with nothing between shows the list.
        let tapped = !pressed && keys::is_super(code) && self.super_alone;
        self.super_alone = pressed && keys::is_super(code) && keyboard.pressed_keys().len() == 1;
        match what {
            Some(Kept::Chord(chord)) => self.asked = self.asked.or(Some(chord)),
            Some(Kept::Ask(ask)) => self.ask(ask, at),
            Some(Kept::Key { ctrl, shift }) => self.manager.key(&mut self.shell, code, ctrl, shift),
            None if tapped => self.ask(Ask::List, at),
            None => {}
        }
        if !keyboard.modifier_state().alt {
            self.manager.alt_let_go(&mut self.shell);
        }
        self.settle();
    }

    /// A key of the window manager's asked `ask`, with the pointer at
    /// `at`. A popup that had the keys is done.
    fn ask(&mut self, ask: Ask, at: Point<f64, Logical>) {
        self.shell.end_popups(None);
        self.manager.ask(&mut self.shell, ask, at);
    }

    /// A pointer's button, of Linux code `code`, went down or up. A
    /// press on a window gives the window the keyboard and brings it
    /// to the front; one on a frame is the window manager's.
    pub fn button(&mut self, time_ns: u64, code: u16, pressed: bool) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let at = pointer.current_location();
        self.super_alone = false;
        let under = self.shell.windows.under(at);
        if pressed {
            self.manager.press(&mut self.shell, &under, at, code, time_ns);
        } else {
            self.manager.release(&mut self.shell, &under, at, code);
        }
        self.held.retain(|held| *held != code);
        if pressed {
            self.held.push(code);
        }
        // The program under the pointer hears of the button; over a
        // frame, and while the window manager has the pointer, none is.
        let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
        let serial = SERIAL_COUNTER.next_serial();
        let event = ButtonEvent { serial, time: ms(time_ns), button: u32::from(code), state };
        self.shell.stamps.pointer([pointer.current_focus().as_ref(), None], time_ns);
        pointer.button(&mut self.shell, &event);
        pointer.frame(&mut self.shell);
        self.shell.changed.windows = true;
        self.settle();
    }

    /// A mouse moved (`dx`, `dy`) counts right and down.
    pub fn moved(&mut self, time_ns: u64, dx: i32, dy: i32) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let by: Point<f64, Logical> = self.curve.moved(time_ns, dx, dy).into();
        self.point(time_ns, pointer.current_location() + by);
    }

    /// A device points at a place of the first screen: `x` of the way
    /// from its left edge to its right, `y` of the way down, each from
    /// 0 to 1. An axis that is `None` stays where the pointer is.
    pub fn pointed(&mut self, time_ns: u64, x: Option<f64>, y: Option<f64>) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let Some(first) = self.screens.first().map(Rectangle::to_f64) else {
            return;
        };
        let mut at = pointer.current_location();
        if let Some(x) = x {
            at.x = first.loc.x + x.clamp(0.0, 1.0) * (first.size.w - 1.0);
        }
        if let Some(y) = y {
            at.y = first.loc.y + y.clamp(0.0, 1.0) * (first.size.h - 1.0);
        }
        self.point(time_ns, at);
    }

    /// Moves the pointer to the place on a screen nearest to `at`, and
    /// tells the window manager, then the program whose surface is
    /// under it.
    fn point(&mut self, time_ns: u64, at: Point<f64, Logical>) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let at = self.screens.within(at);
        self.manager.motion(&mut self.shell, at);
        self.under = self.shell.windows.under(at);
        let focus = self.focus();
        let serial = SERIAL_COUNTER.next_serial();
        let moved = MotionEvent { location: at, serial, time: ms(time_ns) };
        // A pointer that is held stays with the program that has it;
        // otherwise the one under it hears of the motion.
        let (has, comes) = (pointer.current_focus(), focus.as_ref().map(|(surface, _)| surface));
        self.shell.stamps.pointer([has.as_ref(), comes], time_ns);
        pointer.motion(&mut self.shell, focus, &moved);
        pointer.frame(&mut self.shell);
        self.screens.point(at);
        self.settle();
    }

    /// A mouse's wheels turned, in 120ths of a notch: `vertical` away
    /// from the person, `horizontal` to the right.
    pub fn wheel(&mut self, time_ns: u64, vertical: i32, horizontal: i32) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let mut frame = AxisFrame::new(ms(time_ns)).source(AxisSource::Wheel);
        // The protocol's vertical axis grows towards the person.
        for (axis, turned) in [(Axis::Vertical, -vertical), (Axis::Horizontal, horizontal)] {
            if turned != 0 {
                frame = frame.v120(axis, turned).value(axis, f64::from(turned) * NOTCH / 120.0);
            }
        }
        self.shell.stamps.pointer([pointer.current_focus().as_ref(), None], time_ns);
        pointer.axis(&mut self.shell, frame);
        pointer.frame(&mut self.shell);
        self.settle();
    }

    /// Every key and button that is held counts as let go: the desktop
    /// went to the back, or a device's events were lost.
    pub fn let_go(&mut self, time_ns: u64) {
        let time = ms(time_ns);
        if let Some(keyboard) = self.shell.seat.get_keyboard() {
            self.shell.stamps.keys(keyboard.current_focus().as_ref(), time_ns);
            for held in keyboard.pressed_keys() {
                let (serial, up) = (SERIAL_COUNTER.next_serial(), KeyState::Released);
                let shell = &mut self.shell;
                keyboard
                    .input(shell, held, up, serial, time, |_, _, _| FilterResult::<()>::Forward);
            }
        }
        self.kept.clear();
        if let Some(pointer) = self.shell.seat.get_pointer() {
            self.shell.stamps.pointer([pointer.current_focus().as_ref(), None], time_ns);
            for button in self.held.drain(..).map(u32::from) {
                let (serial, state) = (SERIAL_COUNTER.next_serial(), ButtonState::Released);
                pointer.button(&mut self.shell, &ButtonEvent { serial, time, button, state });
            }
            pointer.frame(&mut self.shell);
        }
        self.settle();
    }

    /// When the next frame is to be drawn, on the monotonic clock, if
    /// one is due.
    pub fn due(&self, now_ns: u64) -> Option<u64> {
        self.screens.due(now_ns)
    }

    /// Draws the frames that are due at `now_ns`.
    ///
    /// A frame that cannot be drawn because a program's pixels cannot
    /// be read, as when the program cut their file short, is drawn anew
    /// once that program is let go, which Smithay has seen to by then.
    ///
    /// # Errors
    ///
    /// What the layer's requests fail with, and what the renderer does
    /// [`SPOILED_MOST`] frames in a row.
    pub fn draw(&mut self, now_ns: u64) -> Result<(), Failed> {
        let Some(pointer) = self.shell.seat.get_pointer() else { return Ok(()) };
        let shown = match self.own {
            Some(own) => Shown::Own(own),
            None => Shown::Programs(&self.shell.pointer),
        };
        let pointing = Pointing {
            at: pointer.current_location(),
            shown,
            picture: self.pointer_picture,
            own: &self.pictures,
        };
        let scene = Scene { windows: &self.shell.windows, panel: self.manager.panel() };
        match self.screens.draw(&mut self.renderer, &self.shell, &scene, &pointing, now_ns) {
            Err(Failed::Renderer(_)) if self.spoiled < SPOILED_MOST => {
                self.spoiled += 1;
                self.settle();
                Ok(())
            }
            drawn => {
                self.spoiled = 0;
                drawn
            }
        }
    }

    /// The lock lights the keyboards are to show, as INPUT_LIGHTS takes
    /// them, if they changed since this was last asked.
    pub fn lights(&mut self) -> Option<u32> {
        self.shell.lights.take().map(keys::light_bits)
    }

    /// What the screen at place `at` last showed: its width, and its
    /// pixels as FORMAT_XRGB8888 rows with nothing between them.
    #[cfg(test)]
    pub fn shows(&mut self, at: usize) -> (u32, Vec<u8>) {
        use smithay::backend::allocator::Fourcc;
        use smithay::backend::renderer::ExportMem;
        let ((width, height), pixels) = self.screens.pixels(at);
        let target = self.renderer.bind(pixels).unwrap();
        let all = Rectangle::from_size((width as i32, height as i32).into());
        let copy = self.renderer.copy_framebuffer(&target, all, Fourcc::Xrgb8888).unwrap();
        (width, self.renderer.map_texture(&copy).unwrap().to_vec())
    }

    /// What a chord asked of windowd since this was last asked.
    pub fn asked(&mut self) -> Option<Chord> {
        self.asked.take()
    }
}
