// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A Wayland program for the window server's tests (windowd(1)): it
//! shows one window whose pixels are a test pattern
//! ([`display_pattern::pattern`]), and keeps what the window server
//! tells it, in the order it was told, for a test to ask about.
//!
//! windowd's own tests run a [`Client`] against a desktop in the same
//! process, over a socket pair. The desktop boot tests run the
//! `window-test` program, which is a [`Client`] that says in the log
//! what the screen then shows and what reached it.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use display_pattern::pattern;
use sysapi::memory::MemoryFile;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry,
    wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// How a test's failure is told.
pub type Outcome<T = ()> = Result<T, String>;

/// The colour of the desktop where no window is, as red, green and blue
/// (windowd(1)).
pub const DESKTOP: [u8; 3] = [0x12, 0x16, 0x1e];

/// One thing the window server told the program.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// The window has the keyboard, or no longer has it.
    Keyboard(bool),
    /// A key of this Linux code went down or up.
    Key {
        /// The code.
        code: u32,
        /// Whether it went down.
        pressed: bool,
    },
    /// The pointer came onto the window at this place of it.
    Enter(f64, f64),
    /// The pointer left the window.
    Leave,
    /// The pointer moved to this place of the window.
    Motion(f64, f64),
    /// A button of this Linux code went down or up.
    Button {
        /// The code.
        code: u32,
        /// Whether it went down.
        pressed: bool,
    },
    /// The wheel turned this many 120ths of a notch, down or right.
    Wheel {
        /// Whether it is the horizontal wheel.
        horizontal: bool,
        /// How far.
        turned: i32,
    },
    /// A frame with the window's last commit was shown, at this time in
    /// milliseconds.
    Frame(u32),
    /// The window is, or is no longer, the one the person works in.
    Active(bool),
    /// The person asked for the window to close.
    Close,
}

/// A screen the window server told of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    /// Its name.
    pub name: String,
    /// The width of its mode, in pixels.
    pub width: u32,
    /// The height.
    pub height: u32,
    /// Where its left edge is on the desktop.
    pub x: i32,
    /// Where its top edge is.
    pub y: i32,
}

/// What the window server offers and has said.
#[derive(Debug, Default)]
struct Told {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Each screen, by the number of its global.
    screens: Vec<(u32, Screen)>,
    /// The bytes of the keyboard's layout, once it came.
    layout_bytes: Option<u32>,
    /// How long a held key waits and how often it repeats, once told.
    repeat: Option<(i32, i32)>,
    /// The serial of the configure the window is to answer.
    configured: Option<u32>,
    active: bool,
    /// The serial of the pointer's last coming onto the window.
    entered: Option<u32>,
    heard: VecDeque<Heard>,
}

/// The window: its surface, and the roles that make it one.
#[derive(Debug)]
struct Window {
    surface: wl_surface::WlSurface,
    /// Kept for as long as the window shows.
    _role: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    /// The buffer that shows, and its pixels' file.
    _shown: Option<(wl_buffer::WlBuffer, File)>,
}

/// The program.
#[derive(Debug)]
pub struct Client {
    queue: EventQueue<Told>,
    told: Told,
    window: Option<Window>,
    /// The pointer's picture over the window, kept for as long as it
    /// shows.
    _pointer: Option<(wl_surface::WlSurface, wl_buffer::WlBuffer, File)>,
}

impl Client {
    /// A program on the connection `stream` to a window server.
    ///
    /// # Errors
    ///
    /// When the window server does not offer what a window takes.
    pub fn on(stream: UnixStream) -> Outcome<Client> {
        let connection = Connection::from_socket(stream).map_err(|err| err.to_string())?;
        Client::start(&connection)
    }

    /// A program on the window server the environment names
    /// (WAYLAND_DISPLAY).
    ///
    /// # Errors
    ///
    /// When there is none, or it does not offer what a window takes.
    pub fn of_environment() -> Outcome<Client> {
        let connection = Connection::connect_to_env();
        Client::start(&connection.map_err(|err| format!("no window server: {err}"))?)
    }

    fn start(connection: &Connection) -> Outcome<Client> {
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut told = Told::default();
        // What is offered, and then what each thing offered says of
        // itself: a screen's mode, the seat's keyboard and pointer.
        for _ in 0..3 {
            queue.roundtrip(&mut told).map_err(|err| format!("the window server: {err}"))?;
        }
        let missing = [
            ("wl_compositor", told.compositor.is_none()),
            ("wl_shm", told.shm.is_none()),
            ("xdg_wm_base", told.shell.is_none()),
            ("wl_seat", told.seat.is_none()),
        ];
        if let Some((name, _)) = missing.iter().find(|(_, missing)| *missing) {
            return Err(format!("the window server offers no {name}"));
        }
        Ok(Client { queue, told, window: None, _pointer: None })
    }

    /// The screens.
    pub fn screens(&self) -> Vec<Screen> {
        self.told.screens.iter().map(|(_, screen)| screen.clone()).collect()
    }

    /// The bytes of the keyboard's layout, if the window server sent
    /// one.
    pub fn layout_bytes(&self) -> Option<u32> {
        self.told.layout_bytes
    }

    /// How long a held key waits before it repeats, in milliseconds,
    /// and how many times a second it repeats, if told.
    pub fn repeat(&self) -> Option<(i32, i32)> {
        self.told.repeat
    }

    /// Whether the seat has a keyboard, and whether it has a pointer.
    pub fn has(&self) -> (bool, bool) {
        (self.told.keyboard.is_some(), self.told.pointer.is_some())
    }

    /// Sends what is to be sent and takes what has come, for up to
    /// `patience` if nothing has.
    fn turn(&mut self, patience: Duration) -> Outcome {
        let gone = |err: &dyn core::fmt::Display| format!("the window server: {err}");
        self.queue.flush().map_err(|err| gone(&err))?;
        if let Some(reading) = self.queue.prepare_read() {
            let came = sysapi::poll::wait_readable(reading.connection_fd(), patience);
            if came.map_err(|err| gone(&err))? {
                reading.read().map_err(|err| gone(&err))?;
            }
        }
        self.queue.dispatch_pending(&mut self.told).map_err(|err| gone(&err))?;
        Ok(())
    }

    /// Shows the window, `width` by `height`, with the pattern of
    /// `seed` from its top left corner, or draws an open window anew.
    /// Returns once the window server has all of it; the window is
    /// then told of the frame that shows it ([`Heard::Frame`]).
    ///
    /// # Errors
    ///
    /// When the window server is gone or does not answer.
    pub fn show(&mut self, (width, height): (u32, u32), seed: u32) -> Outcome {
        let handle = self.queue.handle();
        let (Some(compositor), Some(shell)) = (&self.told.compositor, &self.told.shell) else {
            return Err("the window server offers no windows".into());
        };
        if self.window.is_none() {
            let surface = compositor.create_surface(&handle, ());
            let role = shell.get_xdg_surface(&surface, &handle, ());
            let toplevel = role.get_toplevel(&handle, ());
            toplevel.set_title("window-test".into());
            toplevel.set_app_id("org.dukovo.window-test".into());
            // A commit with no buffer asks for the first configure.
            surface.commit();
            self.window = Some(Window { surface, _role: role, _toplevel: toplevel, _shown: None });
            let until = Instant::now() + Duration::from_secs(10);
            while self.told.configured.is_none() {
                if Instant::now() >= until {
                    return Err("the window server did not configure the window".into());
                }
                self.turn(Duration::from_millis(100))?;
            }
        }
        let shown = self.buffer((width, height), seed)?;
        let Some(window) = &mut self.window else { return Err("no window".into()) };
        if let Some(serial) = self.told.configured.take() {
            window._role.ack_configure(serial);
        }
        window.surface.attach(Some(&shown.0), 0, 0);
        window.surface.damage_buffer(0, 0, width as i32, height as i32);
        window.surface.frame(&handle, ());
        window.surface.commit();
        // The buffer before is let go only now, when it shows no more.
        if let Some((before, _)) = window._shown.replace(shown) {
            before.destroy();
        }
        self.queue.roundtrip(&mut self.told).map_err(|err| format!("the window server: {err}"))?;
        Ok(())
    }

    /// Commits the window with nothing new and asks to be told of the
    /// next frame, as a program does that draws when it is its turn.
    ///
    /// # Errors
    ///
    /// When no window shows, or the window server is gone.
    pub fn ask_frame(&mut self) -> Outcome {
        let handle = self.queue.handle();
        let Some(window) = &self.window else { return Err("no window".into()) };
        window.surface.frame(&handle, ());
        window.surface.commit();
        self.turn(Duration::ZERO)
    }

    /// Gives the pointer, which is over the window, a picture of the
    /// program's own: a square of `side` pixels with the pattern of
    /// `seed`, whose top left corner is where the pointer points.
    ///
    /// # Errors
    ///
    /// When the pointer has not come onto the window, or the window
    /// server is gone.
    pub fn point_with(&mut self, side: u32, seed: u32) -> Outcome {
        let handle = self.queue.handle();
        let (Some(compositor), Some(pointer), Some(entered)) =
            (&self.told.compositor, &self.told.pointer, self.told.entered)
        else {
            return Err("the pointer is not over the window".into());
        };
        let surface = compositor.create_surface(&handle, ());
        pointer.set_cursor(entered, Some(&surface), 0, 0);
        let (buffer, file) = self.buffer((side, side), seed)?;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, side as i32, side as i32);
        surface.commit();
        self._pointer = Some((surface, buffer, file));
        self.queue.roundtrip(&mut self.told).map_err(|err| format!("the window server: {err}"))?;
        Ok(())
    }

    /// Shows a window whose pixels' file the program then cuts short,
    /// as a program does that breaks, or that wants the window server
    /// to read memory that is not there. Returns once the window server
    /// has let the program go, which it does when it finds the pixels
    /// gone.
    ///
    /// # Errors
    ///
    /// When the window server keeps the program for ten seconds.
    pub fn cut_short(&mut self, (width, height): (u32, u32)) -> Outcome {
        let handle = self.queue.handle();
        let (Some(compositor), Some(shell)) = (&self.told.compositor, &self.told.shell) else {
            return Err("the window server offers no windows".into());
        };
        let surface = compositor.create_surface(&handle, ());
        let role = shell.get_xdg_surface(&surface, &handle, ());
        let _toplevel = role.get_toplevel(&handle, ());
        surface.commit();
        let until = Instant::now() + Duration::from_secs(10);
        while self.told.configured.is_none() {
            if Instant::now() >= until {
                return Err("the window server did not configure the second window".into());
            }
            self.turn(Duration::from_millis(100))?;
        }
        let (buffer, file) = self.buffer((width, height), 0)?;
        // Once the round trip is made the window server has the pool,
        // mapped at its whole length.
        self.queue.roundtrip(&mut self.told).map_err(|err| format!("the window server: {err}"))?;
        file.set_len(0).map_err(|err| format!("cutting the pixels' file short: {err}"))?;
        if let Some(serial) = self.told.configured.take() {
            role.ack_configure(serial);
        }
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        while Instant::now() < until {
            if self.turn(Duration::from_millis(100)).is_err() {
                return Ok(());
            }
        }
        Err("the window server kept a program whose pixels' file was cut short".into())
    }

    /// A buffer of `size` with the pattern of `seed`, and the file its
    /// pixels are in.
    fn buffer(
        &mut self,
        (width, height): (u32, u32),
        seed: u32,
    ) -> Outcome<(wl_buffer::WlBuffer, File)> {
        let failed = |err: std::io::Error| format!("the window's pixels: {err}");
        let row_bytes = width as usize * 4;
        let mut pixels = vec![0u8; row_bytes * height as usize];
        pattern::paint(seed, (0, 0), width, row_bytes, &mut pixels);
        // The pixels are written, not mapped: the window server maps
        // the file, and this program needs them no more.
        let memory: OwnedFd = MemoryFile::new().and_then(MemoryFile::into_fd).map_err(failed)?;
        let mut file = File::from(memory);
        file.write_all(&pixels).map_err(failed)?;
        let handle = self.queue.handle();
        let Some(shm) = &self.told.shm else {
            return Err("the window server offers no wl_shm".into());
        };
        let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &handle, ());
        let (stride, format) = (row_bytes as i32, wl_shm::Format::Xrgb8888);
        let buffer =
            pool.create_buffer(0, width as i32, height as i32, stride, format, &handle, ());
        pool.destroy();
        Ok((buffer, file))
    }

    /// The next thing the window server told the program, waiting up to
    /// `patience` for one.
    ///
    /// # Errors
    ///
    /// When nothing came in that time, or the window server is gone.
    pub fn heard(&mut self, patience: Duration) -> Outcome<Heard> {
        let until = Instant::now() + patience;
        loop {
            if let Some(heard) = self.told.heard.pop_front() {
                return Ok(heard);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("the window server said nothing in {patience:?}"));
            }
            self.turn(left.min(Duration::from_millis(100)))?;
        }
    }

    /// Everything the window server has told the program and no test
    /// has asked about, in the order it came, without waiting for more.
    pub fn take_heard(&mut self) -> Vec<Heard> {
        self.told.heard.drain(..).collect()
    }

    /// Waits up to `patience` for the first thing `wanted` takes, and
    /// takes it out of what the program heard. What came before it
    /// stays, for [`Client::heard`].
    ///
    /// # Errors
    ///
    /// When none came in that time, or the window server is gone.
    pub fn wait<T>(
        &mut self,
        patience: Duration,
        mut wanted: impl FnMut(&Heard) -> Option<T>,
    ) -> Outcome<T> {
        let until = Instant::now() + patience;
        let mut looked_at = 0;
        loop {
            while let Some(heard) = self.told.heard.get(looked_at) {
                if let Some(found) = wanted(heard) {
                    self.told.heard.remove(looked_at);
                    return Ok(found);
                }
                looked_at += 1;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("the window server did not say it in {patience:?}"));
            }
            self.turn(left.min(Duration::from_millis(100)))?;
        }
    }

    /// Whether the window is the one the person works in, as the window
    /// server last said.
    pub fn active(&self) -> bool {
        self.told.active
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Told {
    fn event(
        told: &mut Told,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _connection: &Connection,
        handle: &QueueHandle<Told>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } => match interface.as_str() {
                "wl_compositor" => {
                    told.compositor = Some(registry.bind(name, version.min(4), handle, ()))
                }
                "wl_shm" => told.shm = Some(registry.bind(name, 1, handle, ())),
                "xdg_wm_base" => told.shell = Some(registry.bind(name, version.min(2), handle, ())),
                "wl_seat" => told.seat = Some(registry.bind(name, version.min(8), handle, ())),
                "wl_output" => {
                    let _: wl_output::WlOutput = registry.bind(name, version.min(4), handle, name);
                    told.screens.push((name, Screen::default()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                told.screens.retain(|(screen, _)| *screen != name);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for Told {
    fn event(
        told: &mut Told,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        let Some((_, screen)) = told.screens.iter_mut().find(|(name, _)| name == global) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } => (screen.x, screen.y) = (x, y),
            wl_output::Event::Mode { flags: WEnum::Value(flags), width, height, .. }
                if flags.contains(wl_output::Mode::Current) =>
            {
                (screen.width, screen.height) = (width as u32, height as u32);
            }
            wl_output::Event::Name { name } => screen.name = name,
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Told {
    fn event(
        told: &mut Told,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _connection: &Connection,
        handle: &QueueHandle<Told>,
    ) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(has) } = event else {
            return;
        };
        if has.contains(wl_seat::Capability::Keyboard) && told.keyboard.is_none() {
            told.keyboard = Some(seat.get_keyboard(handle, ()));
        }
        if has.contains(wl_seat::Capability::Pointer) && told.pointer.is_none() {
            told.pointer = Some(seat.get_pointer(handle, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Told {
    fn event(
        told: &mut Told,
        _keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { size, .. } => told.layout_bytes = Some(size),
            wl_keyboard::Event::RepeatInfo { rate, delay } => told.repeat = Some((delay, rate)),
            wl_keyboard::Event::Enter { .. } => told.heard.push_back(Heard::Keyboard(true)),
            wl_keyboard::Event::Leave { .. } => told.heard.push_back(Heard::Keyboard(false)),
            wl_keyboard::Event::Key { key, state, .. } => {
                let pressed = state == WEnum::Value(wl_keyboard::KeyState::Pressed);
                told.heard.push_back(Heard::Key { code: key, pressed });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Told {
    fn event(
        told: &mut Told,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        let heard = match event {
            wl_pointer::Event::Enter { serial, surface_x, surface_y, .. } => {
                told.entered = Some(serial);
                Heard::Enter(surface_x, surface_y)
            }
            wl_pointer::Event::Leave { .. } => Heard::Leave,
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                Heard::Motion(surface_x, surface_y)
            }
            wl_pointer::Event::Button { button, state, .. } => {
                let pressed = state == WEnum::Value(wl_pointer::ButtonState::Pressed);
                Heard::Button { code: button, pressed }
            }
            wl_pointer::Event::AxisValue120 { axis, value120 } => {
                let horizontal = axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll);
                Heard::Wheel { horizontal, turned: value120 }
            }
            _ => return,
        };
        told.heard.push_back(heard);
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Told {
    fn event(
        _told: &mut Told,
        shell: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Told {
    fn event(
        told: &mut Told,
        _role: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            told.configured = Some(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Told {
    fn event(
        told: &mut Told,
        _toplevel: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { states, .. } => {
                let activated = xdg_toplevel::State::Activated as u32;
                let active = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|state| u32::from_ne_bytes(*state) == activated);
                if active != told.active {
                    told.active = active;
                    told.heard.push_back(Heard::Active(active));
                }
            }
            xdg_toplevel::Event::Close => told.heard.push_back(Heard::Close),
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Told {
    fn event(
        told: &mut Told,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let wl_callback::Event::Done { callback_data } = event {
            told.heard.push_back(Heard::Frame(callback_data));
        }
    }
}

delegate_noop!(Told: wl_compositor::WlCompositor);
delegate_noop!(Told: ignore wl_shm::WlShm);
delegate_noop!(Told: wl_shm_pool::WlShmPool);
delegate_noop!(Told: ignore wl_buffer::WlBuffer);
delegate_noop!(Told: ignore wl_surface::WlSurface);
