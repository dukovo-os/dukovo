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

use std::io;
use std::os::fd::BorrowedFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use display::Holder;
use display::abi::DisplayEvent;
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::backend::renderer::Bind;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::input::keyboard::{FilterResult, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use sysapi::time::monotonic_ns;

use crate::clients::Clients;
use crate::curve::Curve;
use crate::keys::{self, Chord};
use crate::link::Link;
use crate::pointer;
use crate::screens::{Failed, Pointing, Screens, Told};
use crate::shell::Shell;

/// How long a key is held before the program that has it repeats it,
/// in milliseconds, and how many times a second it repeats then: as at
/// the consoles (radix-input(2)).
const REPEAT: (i32, i32) = (250, 30);

/// How far one notch of a wheel scrolls, as the programs reckon it.
const NOTCH: f64 = 15.0;

/// How many frames in a row may fail to be drawn before the desktop
/// ends. One fails when a program's pixels cannot be read, and that
/// program is gone before the next is drawn; frames that go on failing
/// fail for a reason that letting a program go does not mend.
const SPOILED_MOST: u8 = 3;

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
    arrow: MemoryRenderBuffer,
    /// Counts the pictures the pointer has shown.
    pointer_picture: u64,
    curve: Curve,
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
        let (delay, rate) = REPEAT;
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
            arrow: pointer::arrow(),
            pointer_picture: 0,
            curve: Curve::default(),
            kept: Vec::new(),
            held: Vec::new(),
            asked: None,
            spoiled: 0,
        };
        desktop.centre();
        Ok(desktop)
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
    /// programs what is theirs, lets go of those that are gone, and
    /// notes which screens are due a frame.
    fn settle(&mut self) {
        // A program that reads nothing or is gone fails here, and is
        // found gone just below.
        let _ = self.display.flush_clients();
        self.clients.let_go(&mut self.display, &mut self.shell);
        self.shell.refresh();
        let changed = core::mem::take(&mut self.shell.changed);
        if changed.windows {
            self.find_under();
        }
        if changed.pointer {
            self.pointer_picture += 1;
        }
        if changed.windows || changed.pointer {
            self.screens.stale();
        }
        let _ = self.display.flush_clients();
    }

    /// Tells the surface that is under the pointer now, if it was not
    /// before: windows came, went or changed under a pointer that did
    /// not move.
    fn find_under(&mut self) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let at = pointer.current_location();
        let under = self.shell.under(at);
        if under.as_ref().map(|(surface, _)| surface) == pointer.current_focus().as_ref() {
            return;
        }
        let serial = SERIAL_COUNTER.next_serial();
        let moved = MotionEvent { location: at, serial, time: ms(monotonic_ns()) };
        pointer.motion(&mut self.shell, under, &moved);
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
        let kept = &mut self.kept;
        let serial = SERIAL_COUNTER.next_serial();
        // XKB's codes are Linux's and 8.
        let xkb = (u32::from(code) + 8).into();
        let chord =
            keyboard.input(&mut self.shell, xkb, state, serial, ms(time_ns), |_, held, _| {
                if !pressed {
                    let was_kept = kept.iter().position(|other| *other == code);
                    return match was_kept.map(|at| kept.swap_remove(at)) {
                        Some(_) => FilterResult::Intercept(None),
                        None => FilterResult::Forward,
                    };
                }
                match keys::chord(held, code) {
                    Some(chord) => {
                        kept.push(code);
                        FilterResult::Intercept(Some(chord))
                    }
                    None => FilterResult::Forward,
                }
            });
        self.asked = self.asked.or(chord.flatten());
        self.settle();
    }

    /// A pointer's button, of Linux code `code`, went down or up. A
    /// press on a window gives the window the keyboard and brings it
    /// to the front.
    pub fn button(&mut self, time_ns: u64, code: u16, pressed: bool) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        if pressed {
            if let Some(window) = self.shell.window_under(pointer.current_location()) {
                self.shell.focus(Some(&window));
            }
        }
        self.held.retain(|held| *held != code);
        if pressed {
            self.held.push(code);
        }
        let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
        let serial = SERIAL_COUNTER.next_serial();
        let event = ButtonEvent { serial, time: ms(time_ns), button: u32::from(code), state };
        pointer.button(&mut self.shell, &event);
        pointer.frame(&mut self.shell);
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
        let Some(first) = self.screens.first().map(|first| first.to_f64()) else { return };
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
    /// tells the program whose surface is under it.
    fn point(&mut self, time_ns: u64, at: Point<f64, Logical>) {
        let Some(pointer) = self.shell.seat.get_pointer() else { return };
        let at = self.screens.within(at);
        let under = self.shell.under(at);
        let serial = SERIAL_COUNTER.next_serial();
        let moved = MotionEvent { location: at, serial, time: ms(time_ns) };
        pointer.motion(&mut self.shell, under, &moved);
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
        pointer.axis(&mut self.shell, frame);
        pointer.frame(&mut self.shell);
        self.settle();
    }

    /// Every key and button that is held counts as let go: the desktop
    /// went to the back, or a device's events were lost.
    pub fn let_go(&mut self, time_ns: u64) {
        let time = ms(time_ns);
        if let Some(keyboard) = self.shell.seat.get_keyboard() {
            for held in keyboard.pressed_keys() {
                let (serial, up) = (SERIAL_COUNTER.next_serial(), KeyState::Released);
                let shell = &mut self.shell;
                keyboard
                    .input(shell, held, up, serial, time, |_, _, _| FilterResult::<()>::Forward);
            }
        }
        self.kept.clear();
        if let Some(pointer) = self.shell.seat.get_pointer() {
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
        let pointing = Pointing {
            at: pointer.current_location(),
            shown: &self.shell.pointer,
            picture: self.pointer_picture,
            arrow: &self.arrow,
        };
        match self.screens.draw(&mut self.renderer, &self.shell, &pointing, now_ns) {
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
        use smithay::utils::Rectangle;
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
