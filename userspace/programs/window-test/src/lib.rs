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

mod bar;

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use display_pattern::pattern;
use sysapi::memory::MemoryFile;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry,
    wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::input_timestamps::zv1::client::{
    zwp_input_timestamps_manager_v1, zwp_input_timestamps_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
pub use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

pub use bar::{BarSaid, Mode};

/// How a test's failure is told.
pub type Outcome<T = ()> = Result<T, String>;

/// The colour of the desktop where no window is, as red, green and blue
/// (windowd(1)).
pub const DESKTOP: [u8; 3] = [0x12, 0x16, 0x1e];

/// One thing the window server told the program.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// The key or pointer event that comes next happened at this time
    /// on the monotonic clock, in nanoseconds. Told to a program that
    /// asked for the times ([`Client::ask_times`]).
    At(u64),
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
    /// The window server asked for the window's own pixels to be of
    /// another size, where 0 leaves a side to the program, or for the
    /// window to be drawn maximized or no longer.
    Size {
        /// The width.
        width: i32,
        /// The height.
        height: i32,
        /// Whether the window is to be drawn maximized.
        maximized: bool,
    },
    /// The window server asked for the window to be drawn full screen,
    /// or no longer. The size to draw it at follows ([`Heard::Size`]).
    Full(bool),
    /// The popup is done: the person went elsewhere.
    PopupDone,
    /// The window server said who draws the window's frame: itself, or
    /// the program.
    Decorated {
        /// Whether the window server does.
        by_server: bool,
    },
}

/// How a window is asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Open {
    /// The program asks for the window to be maximized from the first.
    pub maximized: bool,
    /// The program asks for a part of the window's title bar before the
    /// window is first configured.
    pub shares_bar: bool,
    /// The program says how it would have the window decorated before
    /// the window is first configured, as a toolkit does.
    pub decoration: Option<Decoration>,
}

/// How a program would have its window decorated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoration {
    /// By itself.
    Own,
    /// By the window server.
    Servers,
    /// As the window server likes.
    Either,
}

/// Which of the program's surfaces a configure is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Window,
    Popup,
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
    /// The whole zoom a program is to draw for it at.
    pub scale: i32,
    /// How many quarters, counter-clockwise, the protocol says a
    /// program's picture is turned by to undo the screen's turn.
    pub turned: u32,
}

/// What the window server offers and has said.
#[derive(Debug, Default)]
struct Told {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<xdg_wm_base::XdgWmBase>,
    /// The registry, and the number of the title bar manager's global.
    registry: Option<wl_registry::WlRegistry>,
    title_bars: Option<u32>,
    decorations: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    times: Option<zwp_input_timestamps_manager_v1::ZwpInputTimestampsManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractions: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    /// The whole zoom each surface was last told to draw at
    /// (`wl_surface.preferred_buffer_scale`).
    zooms: Vec<(wl_surface::WlSurface, i32)>,
    /// The zoom the window was last told to draw at, in 120ths
    /// (`wp_fractional_scale_v1.preferred_scale`).
    fraction: Option<u32>,
    /// The objects that tell the times of the keys and of the pointer,
    /// once the program asked for them.
    told_times: Vec<zwp_input_timestamps_v1::ZwpInputTimestampsV1>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Each screen, by the number of its global.
    screens: Vec<(u32, Screen)>,
    /// Each screen's object, by the same number.
    outputs: Vec<(u32, wl_output::WlOutput)>,
    /// Whether the window server last asked for the window to be full
    /// screen.
    full: bool,
    /// The bytes of the keyboard's layout, once it came.
    layout_bytes: Option<u32>,
    /// How long a held key waits and how often it repeats, once told.
    repeat: Option<(i32, i32)>,
    /// The serial of the configure the window is to answer.
    configured: Option<u32>,
    /// The serial of the configure the popup is to answer.
    popup_configured: Option<u32>,
    /// The size the window server last asked for, and whether it asked
    /// for the window to be maximized.
    asked: (i32, i32, bool),
    /// The serial of the last press of a button.
    pressed: Option<u32>,
    active: bool,
    /// The serial of the pointer's last coming onto the window.
    entered: Option<u32>,
    /// The surface the program has the pointer show over the window. A
    /// pointer shows what it was given until it leaves, so the program
    /// gives it again each time the pointer comes onto the window.
    pointer_surface: Option<wl_surface::WlSurface>,
    heard: VecDeque<Heard>,
}

/// The window: its surface, and the roles that make it one.
#[derive(Debug)]
struct Window {
    surface: wl_surface::WlSurface,
    role: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    /// The object the program says with how it would be decorated.
    decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    /// The window's title bar object, while the program asks for a
    /// part of the bar.
    bar: Option<bar::TitleBar>,
    /// The buffer that shows, and its pixels' file.
    shown: Option<(wl_buffer::WlBuffer, File)>,
    /// The width and height of the buffer that shows.
    size: (u32, u32),
    /// A second buffer of that size, which the window is drawn into
    /// while the first shows ([`Client::repaint`]).
    spare: Option<(wl_buffer::WlBuffer, File)>,
    /// The object the window is told its zoom in 120ths through.
    _fraction: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    /// The object the program says with which part of its buffer
    /// shows, at which size.
    view: Option<wp_viewport::WpViewport>,
}

/// How a buffer's pixels are a window's points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// As many pixels across and down a point as this
    /// (`wl_surface.set_buffer_scale`).
    Zoomed(i32),
    /// The whole buffer shows this many points wide and tall
    /// (`wp_viewport.set_destination`).
    Viewed(i32, i32),
}

/// A popup of the window, with the buffer it shows and its pixels'
/// file.
#[derive(Debug)]
struct Popup {
    surface: wl_surface::WlSurface,
    role: xdg_surface::XdgSurface,
    popup: xdg_popup::XdgPopup,
    shown: (wl_buffer::WlBuffer, File),
}

/// The program.
#[derive(Debug)]
pub struct Client {
    connection: Connection,
    queue: EventQueue<Told>,
    told: Told,
    window: Option<Window>,
    popup: Option<Popup>,
    /// The pointer's picture over the window, and its pixels' file.
    pointer_shown: Option<(wl_surface::WlSurface, wl_buffer::WlBuffer, File)>,
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
        let registry = connection.display().get_registry(&handle, ());
        let mut told = Told { registry: Some(registry), ..Told::default() };
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
        let connection = connection.clone();
        Ok(Client { connection, queue, told, window: None, popup: None, pointer_shown: None })
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

    /// Waits up to `patience` for the seat to have a keyboard and a
    /// pointer. A window server that finds its devices after it starts
    /// has neither at first.
    ///
    /// # Errors
    ///
    /// When one is still missing then, or the window server is gone.
    pub fn await_seat(&mut self, patience: Duration) -> Outcome {
        let until = Instant::now() + patience;
        while self.has() != (true, true) {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("the seat's keyboard and pointer: {:?}", self.has()));
            }
            self.turn(left.min(Duration::from_millis(100)))?;
        }
        self.sent()
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
        // What the program answered while it took that in, as the
        // pointer's picture when the pointer came onto the window.
        self.queue.flush().map_err(|err| gone(&err))
    }

    /// Asks for the window, which shows once it is drawn
    /// ([`Client::show`]), and waits for the window server's answer.
    /// A window that is asked for maximized is answered with the size
    /// to draw it ([`Heard::Size`]); one that asks for a part of its
    /// title bar, with whose the bar is ([`Client::bar`]).
    ///
    /// # Errors
    ///
    /// When the window server is gone or does not answer.
    pub fn open(&mut self, how: Open) -> Outcome {
        let handle = self.queue.handle();
        let (Some(compositor), Some(shell)) = (&self.told.compositor, &self.told.shell) else {
            return Err("the window server offers no windows".into());
        };
        let surface = compositor.create_surface(&handle, ());
        let role = shell.get_xdg_surface(&surface, &handle, Role::Window);
        let toplevel = role.get_toplevel(&handle, ());
        let fraction = self.told.fractions.as_ref();
        let fraction = fraction.map(|all| all.get_fractional_scale(&surface, &handle, ()));
        toplevel.set_title("window-test".into());
        toplevel.set_app_id("org.dukovo.window-test".into());
        if how.maximized {
            toplevel.set_maximized();
        }
        let window = Window {
            surface,
            role,
            toplevel,
            decoration: None,
            bar: None,
            shown: None,
            size: (0, 0),
            spare: None,
            _fraction: fraction,
            view: None,
        };
        self.window = Some(window);
        if how.shares_bar {
            self.share_bar()?;
        }
        if let Some(decoration) = how.decoration {
            self.decorate(decoration)?;
        }
        // A commit with no buffer asks for the first configure.
        self.window()?.surface.commit();
        let until = Instant::now() + Duration::from_secs(10);
        while self.told.configured.is_none() {
            if Instant::now() >= until {
                return Err("the window server did not configure the window".into());
            }
            self.turn(Duration::from_millis(100))?;
        }
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
        self.show_pixels((width, height), &Client::pattern((width, height), seed))
    }

    /// As [`Client::show`], with every pixel of one `shade` of grey:
    /// a picture that costs next to nothing to make, for a program that
    /// measures what showing it costs.
    ///
    /// # Errors
    ///
    /// As [`Client::show`].
    pub fn show_plain(&mut self, (width, height): (u32, u32), shade: u8) -> Outcome {
        self.show_pixels((width, height), &vec![shade; width as usize * 4 * height as usize])
    }

    /// As [`Client::show`], with a buffer of `width` by `height`
    /// pixels that are the window's points as `drawn` says: for a
    /// screen that is zoomed, or a picture shown at another size than
    /// its own.
    ///
    /// # Errors
    ///
    /// As [`Client::show`], and when the window server does not offer
    /// what `drawn` needs.
    pub fn show_as(&mut self, (width, height): (u32, u32), seed: u32, drawn: Drawn) -> Outcome {
        self.show_pixels_as((width, height), &Client::pattern((width, height), seed), Some(drawn))
    }

    /// The zoom the window was last told to draw at: the whole number
    /// (`wl_surface.preferred_buffer_scale`) and the 120ths
    /// (`wp_fractional_scale_v1`), each if it was told.
    pub fn zoom(&self) -> (Option<i32>, Option<u32>) {
        let whole = self.window.as_ref().and_then(|window| {
            let told = self.told.zooms.iter().find(|(surface, _)| *surface == window.surface);
            told.map(|(_, whole)| *whole)
        });
        (whole, self.told.fraction)
    }

    /// Shows the window with `pixels`, `width` by `height` of them, a
    /// pixel a point.
    fn show_pixels(&mut self, size: (u32, u32), pixels: &[u8]) -> Outcome {
        self.show_pixels_as(size, pixels, None)
    }

    /// Shows the window with `pixels`, `width` by `height` of them,
    /// which are its points as `drawn` says, or as they were for
    /// `None`.
    fn show_pixels_as(
        &mut self,
        (width, height): (u32, u32),
        pixels: &[u8],
        drawn: Option<Drawn>,
    ) -> Outcome {
        if self.window.is_none() {
            self.open(Open::default())?;
        }
        let handle = self.queue.handle();
        let shown = self.buffer((width, height), pixels)?;
        let Some(window) = &mut self.window else { return Err("no window".into()) };
        if let Some(serial) = self.told.configured.take() {
            window.role.ack_configure(serial);
        }
        match drawn {
            Some(Drawn::Zoomed(zoom)) => window.surface.set_buffer_scale(zoom),
            Some(Drawn::Viewed(wide, tall)) => {
                let Some(viewporter) = &self.told.viewporter else {
                    return Err("the window server offers no wp_viewporter".into());
                };
                let view = window
                    .view
                    .get_or_insert_with(|| viewporter.get_viewport(&window.surface, &handle, ()));
                view.set_destination(wide, tall);
            }
            None => {}
        }
        window.surface.attach(Some(&shown.0), 0, 0);
        window.surface.damage_buffer(0, 0, width as i32, height as i32);
        window.surface.frame(&handle, ());
        window.surface.commit();
        // The buffer before is let go only now, when it shows no more,
        // and a spare of another size with it.
        if let Some((before, _)) = window.shown.replace(shown) {
            before.destroy();
        }
        if let Some((spare, _)) = window.spare.take() {
            spare.destroy();
        }
        window.size = (width, height);
        self.queue.roundtrip(&mut self.told).map_err(|err| format!("the window server: {err}"))?;
        Ok(())
    }

    /// Draws the window anew at the size it shows, every pixel of one
    /// `shade` of grey, as a program draws frame after frame: into the
    /// one of two buffers that does not show, which then shows in the
    /// other's place. Returns once the frame is sent; the window is
    /// then told of the frame that shows it ([`Heard::Frame`]).
    ///
    /// # Errors
    ///
    /// When no window shows, or the window server is gone.
    pub fn repaint(&mut self, shade: u8) -> Outcome {
        let handle = self.queue.handle();
        let Some(window) = &self.window else { return Err("no window".into()) };
        let (width, height) = window.size;
        let pixels = vec![shade; width as usize * 4 * height as usize];
        let spare = match self.window.as_mut().and_then(|window| window.spare.take()) {
            Some(spare) => {
                let failed = |err: std::io::Error| format!("the window's pixels: {err}");
                spare.1.write_all_at(&pixels, 0).map_err(failed)?;
                spare
            }
            None => self.buffer((width, height), &pixels)?,
        };
        let Some(window) = &mut self.window else { return Err("no window".into()) };
        window.surface.attach(Some(&spare.0), 0, 0);
        window.surface.damage_buffer(0, 0, width as i32, height as i32);
        window.surface.frame(&handle, ());
        window.surface.commit();
        window.spare = window.shown.replace(spare);
        self.queue.flush().map_err(|err| format!("the window server: {err}"))
    }

    /// Sends what the program asked and waits until the window server
    /// has all of it.
    fn sent(&mut self) -> Outcome {
        let gone = |err| format!("the window server: {err}");
        self.queue.roundtrip(&mut self.told).map(|_| ()).map_err(gone)
    }

    /// Waits until the program has read what the window server had to
    /// say of everything there is now: a screen that came since has
    /// said its mode and its place.
    ///
    /// # Errors
    ///
    /// If the window server is gone.
    pub fn caught_up(&mut self) -> Outcome {
        // What is offered now, and then what each new thing says of
        // itself.
        self.sent()?;
        self.sent()
    }

    /// The window, if it shows.
    fn window(&self) -> Outcome<&Window> {
        self.window.as_ref().ok_or_else(|| "no window".into())
    }

    /// The seat, and the serial of the last press of a button the
    /// program was told of.
    fn pressed(&self) -> Outcome<(&wl_seat::WlSeat, u32)> {
        let seat = self.told.seat.as_ref().ok_or("the window server offers no wl_seat")?;
        Ok((seat, self.told.pressed.ok_or("no button was pressed on the window")?))
    }

    /// Gives the window the title `title`.
    ///
    /// # Errors
    ///
    /// When no window shows, or the window server is gone.
    pub fn title(&mut self, title: &str) -> Outcome {
        self.window()?.toplevel.set_title(title.into());
        self.sent()
    }

    /// Asks the window server to have the pointer move the window, as a
    /// program does when a button goes down on a title bar of its own.
    ///
    /// # Errors
    ///
    /// When no button was pressed on the window, or the window server
    /// is gone.
    pub fn ask_move(&mut self) -> Outcome {
        let (seat, serial) = self.pressed()?;
        self.window()?.toplevel._move(seat, serial);
        self.sent()
    }

    /// Asks the window server to have the pointer resize the window by
    /// `edge`.
    ///
    /// # Errors
    ///
    /// As [`Client::ask_move`].
    pub fn ask_resize(&mut self, edge: ResizeEdge) -> Outcome {
        let (seat, serial) = self.pressed()?;
        self.window()?.toplevel.resize(seat, serial, edge);
        self.sent()
    }

    /// Asks the window server for the window's menu at (`x`, `y`) of
    /// the window.
    ///
    /// # Errors
    ///
    /// As [`Client::ask_move`].
    pub fn ask_menu(&mut self, x: i32, y: i32) -> Outcome {
        let (seat, serial) = self.pressed()?;
        self.window()?.toplevel.show_window_menu(seat, serial, x, y);
        self.sent()
    }

    /// Asks the window server for a part of the window's title bar. The
    /// window server says whose the bar then is ([`Client::bar`]), and
    /// with a shared bar the program draws the window the bar's rows
    /// higher, from the answer to the configure that follows.
    ///
    /// # Errors
    ///
    /// When there is no window, the window server does not offer title
    /// bars to share, or it is gone.
    pub fn share_bar(&mut self) -> Outcome {
        let backend = self.connection.backend();
        let (Some(registry), Some(global)) = (&self.told.registry, self.told.title_bars) else {
            return Err("the window server offers no title bars to share".into());
        };
        let toplevel = self.window()?.toplevel.id();
        let bar = bar::TitleBar::of(&backend, &registry.id(), global, &toplevel)?;
        if let Some(window) = &mut self.window {
            window.bar = Some(bar);
        }
        self.sent()
    }

    /// What the window server last said of the window's title bar, if
    /// the program asks for a part of it, once the program has all the
    /// window server sent.
    ///
    /// # Errors
    ///
    /// When the window server is gone.
    pub fn bar(&mut self) -> Outcome<Option<BarSaid>> {
        self.sent()?;
        Ok(self.window.as_ref().and_then(|window| window.bar.as_ref()?.said()))
    }

    /// Names the part of the title bar the program draws, as its left
    /// edge, its top, its width and its height from the window's
    /// corner. It is the window's when the window is next drawn
    /// ([`Client::show`]).
    ///
    /// # Errors
    ///
    /// When the program has not asked for a part of the bar, or the
    /// window server is gone.
    pub fn region(&mut self, region: (i32, i32, i32, i32)) -> Outcome {
        let bar = self.window()?.bar.as_ref().ok_or("the program shares no title bar")?;
        bar.region(region)?;
        self.sent()
    }

    /// Gives the title bar back to the window server, which then asks
    /// for the window to be drawn under its full bar.
    ///
    /// # Errors
    ///
    /// As [`Client::region`].
    pub fn give_bar_back(&mut self) -> Outcome {
        let bar = self.window.as_mut().and_then(|window| window.bar.take());
        bar.ok_or("the program shares no title bar")?.give_back()?;
        self.sent()
    }

    /// Says how the program would have its window decorated. The
    /// window server answers who draws the frame ([`Heard::Decorated`]).
    ///
    /// # Errors
    ///
    /// When there is no window, the window server does not offer the
    /// choice, or it is gone.
    pub fn decorate(&mut self, how: Decoration) -> Outcome {
        let handle = self.queue.handle();
        let Some(decorations) = &self.told.decorations else {
            return Err("the window server offers no choice of decorations".into());
        };
        let window = self.window.as_mut().ok_or("no window")?;
        let decoration = window.decoration.get_or_insert_with(|| {
            decorations.get_toplevel_decoration(&window.toplevel, &handle, ())
        });
        match how {
            Decoration::Own => decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ClientSide),
            Decoration::Servers => {
                decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            }
            Decoration::Either => decoration.unset_mode(),
        }
        self.sent()
    }

    /// Asks the window server for the time of each key and pointer
    /// event to the nanosecond, which is then told before the event
    /// ([`Heard::At`]).
    ///
    /// # Errors
    ///
    /// When the window server does not tell the times, or is gone.
    pub fn ask_times(&mut self) -> Outcome {
        let handle = self.queue.handle();
        let times = self.told.times.as_ref().ok_or("the window server tells no times")?;
        let keyboard = self.told.keyboard.as_ref().ok_or("no keyboard")?;
        let pointer = self.told.pointer.as_ref().ok_or("no pointer")?;
        self.told.told_times = vec![
            times.get_keyboard_timestamps(keyboard, &handle, ()),
            times.get_pointer_timestamps(pointer, &handle, ()),
        ];
        self.sent()
    }

    /// Whether the window server tells the times of input events
    /// ([`Client::ask_times`]).
    pub fn tells_times(&self) -> bool {
        self.told.times.is_some()
    }

    /// Asks the window server to maximize the window, or to restore it.
    /// The window server then asks for a size ([`Heard::Size`]).
    ///
    /// # Errors
    ///
    /// When no window shows, or the window server is gone.
    pub fn maximize(&mut self, maximized: bool) -> Outcome {
        let toplevel = &self.window()?.toplevel;
        if maximized {
            toplevel.set_maximized();
        } else {
            toplevel.unset_maximized();
        }
        self.sent()
    }

    /// Asks the window server to show the window full screen, on the
    /// screen named `on` or on the one the window server chooses, or
    /// for `full` false to show it as it was before. The window server
    /// then says so ([`Heard::Full`]) and asks for a size
    /// ([`Heard::Size`]). The window may be one that is open and not
    /// yet drawn.
    ///
    /// # Errors
    ///
    /// When there is no window or no screen of that name, or the
    /// window server is gone.
    pub fn full(&mut self, full: bool, on: Option<&str>) -> Outcome {
        let toplevel = &self.window()?.toplevel;
        let screen = match on {
            None => None,
            Some(name) => {
                let named = self.told.screens.iter().find(|(_, screen)| screen.name == name);
                let global = named.ok_or("no screen of that name")?.0;
                self.told.outputs.iter().find(|(other, _)| *other == global).map(|found| &found.1)
            }
        };
        if full {
            toplevel.set_fullscreen(screen);
        } else {
            toplevel.unset_fullscreen();
        }
        self.sent()
    }

    /// Asks the window server to minimize the window.
    ///
    /// # Errors
    ///
    /// When no window shows, or the window server is gone.
    pub fn minimize(&mut self) -> Outcome {
        self.window()?.toplevel.set_minimized();
        self.sent()
    }

    /// Shows a popup of the window, `width` by `height` with the
    /// pattern of `seed`, with its top left corner at (`x`, `y`) of the
    /// window. With `grabs` it asks for the keyboard and the pointer,
    /// as a menu does, with the serial of the last press of a button.
    /// The program hears [`Heard::PopupDone`] when the window server
    /// ends it.
    ///
    /// # Errors
    ///
    /// When no window shows, no button was pressed for a popup that
    /// grabs, or the window server is gone or does not answer.
    pub fn popup(
        &mut self,
        (width, height): (u32, u32),
        (x, y): (i32, i32),
        seed: u32,
        grabs: bool,
    ) -> Outcome {
        let handle = self.queue.handle();
        let (Some(compositor), Some(shell)) = (&self.told.compositor, &self.told.shell) else {
            return Err("the window server offers no windows".into());
        };
        let places = shell.create_positioner(&handle, ());
        places.set_size(width as i32, height as i32);
        places.set_anchor_rect(x, y, 1, 1);
        places.set_anchor(xdg_positioner::Anchor::TopLeft);
        places.set_gravity(xdg_positioner::Gravity::BottomRight);
        let surface = compositor.create_surface(&handle, ());
        let role = shell.get_xdg_surface(&surface, &handle, Role::Popup);
        let popup = role.get_popup(Some(&self.window()?.role), &places, &handle, ());
        places.destroy();
        if grabs {
            let (seat, serial) = self.pressed()?;
            popup.grab(seat, serial);
        }
        surface.commit();
        let until = Instant::now() + Duration::from_secs(10);
        while self.told.popup_configured.is_none() {
            if Instant::now() >= until {
                return Err("the window server did not configure the popup".into());
            }
            self.turn(Duration::from_millis(100))?;
        }
        let shown = self.buffer((width, height), &Client::pattern((width, height), seed))?;
        if let Some(serial) = self.told.popup_configured.take() {
            role.ack_configure(serial);
        }
        surface.attach(Some(&shown.0), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        self.close_popup();
        self.popup = Some(Popup { surface, role, popup, shown });
        self.sent()
    }

    /// Takes the popup away, if one shows.
    fn close_popup(&mut self) {
        if let Some(Popup { surface, role, popup, shown }) = self.popup.take() {
            popup.destroy();
            role.destroy();
            surface.destroy();
            shown.0.destroy();
        }
    }

    /// Takes the popup away, as a program does that was told it is
    /// done, or whose person chose from it.
    ///
    /// # Errors
    ///
    /// When the window server is gone.
    pub fn end_popup(&mut self) -> Outcome {
        self.close_popup();
        self.sent()
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
        let (buffer, file) = self.buffer((side, side), &Client::pattern((side, side), seed))?;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, side as i32, side as i32);
        surface.commit();
        // A picture the pointer showed before is let go.
        self.told.pointer_surface = Some(surface.clone());
        if let Some((before, pixels, _)) = self.pointer_shown.replace((surface, buffer, file)) {
            before.destroy();
            pixels.destroy();
        }
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
        let role = shell.get_xdg_surface(&surface, &handle, Role::Window);
        role.get_toplevel(&handle, ());
        surface.commit();
        let until = Instant::now() + Duration::from_secs(10);
        while self.told.configured.is_none() {
            if Instant::now() >= until {
                return Err("the window server did not configure the second window".into());
            }
            self.turn(Duration::from_millis(100))?;
        }
        let (buffer, file) = self.buffer((width, height), &Client::pattern((width, height), 0))?;
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

    /// The pixels of the pattern of `seed`, `width` by `height`.
    fn pattern((width, height): (u32, u32), seed: u32) -> Vec<u8> {
        let row_bytes = width as usize * 4;
        let mut pixels = vec![0u8; row_bytes * height as usize];
        pattern::paint(seed, (0, 0), width, row_bytes, &mut pixels);
        pixels
    }

    /// A buffer of `width` by `height` with `pixels`, and the file its
    /// pixels are in.
    fn buffer(
        &mut self,
        (width, height): (u32, u32),
        pixels: &[u8],
    ) -> Outcome<(wl_buffer::WlBuffer, File)> {
        let failed = |err: std::io::Error| format!("the window's pixels: {err}");
        let row_bytes = width as usize * 4;
        // The pixels are written, not mapped: the window server maps
        // the file, and this program needs them no more.
        let memory: OwnedFd = MemoryFile::new().and_then(MemoryFile::into_fd).map_err(failed)?;
        let mut file = File::from(memory);
        file.write_all(pixels).map_err(failed)?;
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

    /// Takes in what the window server has sent, waiting up to
    /// `patience` for something: for a program that waits on what the
    /// client keeps, as the keys' repeat, and not on something heard.
    ///
    /// # Errors
    ///
    /// When the window server is gone.
    pub fn listen(&mut self, patience: Duration) -> Outcome {
        self.turn(patience)
    }

    /// As [`Client::heard`], with `None` when nothing came in that
    /// time: for a program that listens for more than one of its
    /// windows.
    ///
    /// # Errors
    ///
    /// When the window server is gone.
    pub fn heard_within(&mut self, patience: Duration) -> Outcome<Option<Heard>> {
        if self.told.heard.is_empty() {
            self.turn(patience)?;
        }
        Ok(self.told.heard.pop_front())
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
                let heard = &self.told.heard;
                return Err(format!(
                    "the window server did not say it in {patience:?}; it said {heard:?}"
                ));
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
                    told.compositor = Some(registry.bind(name, version.min(6), handle, ()));
                }
                "wl_shm" => told.shm = Some(registry.bind(name, 1, handle, ())),
                "xdg_wm_base" => told.shell = Some(registry.bind(name, version.min(2), handle, ())),
                "wl_seat" => told.seat = Some(registry.bind(name, version.min(8), handle, ())),
                "zxdg_decoration_manager_v1" => {
                    told.decorations = Some(registry.bind(name, 1, handle, ()));
                }
                "dukovo_title_bar_manager_v1" => told.title_bars = Some(name),
                "zwp_input_timestamps_manager_v1" => {
                    told.times = Some(registry.bind(name, 1, handle, ()));
                }
                "wp_viewporter" => told.viewporter = Some(registry.bind(name, 1, handle, ())),
                "wp_fractional_scale_manager_v1" => {
                    told.fractions = Some(registry.bind(name, 1, handle, ()));
                }
                "wl_output" => {
                    told.outputs.push((name, registry.bind(name, version.min(4), handle, name)));
                    told.screens.push((name, Screen::default()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                told.screens.retain(|(screen, _)| *screen != name);
                told.outputs.retain(|(screen, _)| *screen != name);
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
            wl_output::Event::Geometry { x, y, transform, .. } => {
                (screen.x, screen.y) = (x, y);
                screen.turned = match transform {
                    WEnum::Value(wl_output::Transform::_90) => 1,
                    WEnum::Value(wl_output::Transform::_180) => 2,
                    WEnum::Value(wl_output::Transform::_270) => 3,
                    _ => 0,
                };
            }
            wl_output::Event::Scale { factor } => screen.scale = factor,
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
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        let heard = match event {
            wl_pointer::Event::Enter { serial, surface_x, surface_y, .. } => {
                told.entered = Some(serial);
                if let Some(shown) = &told.pointer_surface {
                    pointer.set_cursor(serial, Some(shown), 0, 0);
                }
                Heard::Enter(surface_x, surface_y)
            }
            wl_pointer::Event::Leave { .. } => Heard::Leave,
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                Heard::Motion(surface_x, surface_y)
            }
            wl_pointer::Event::Button { serial, button, state, .. } => {
                let pressed = state == WEnum::Value(wl_pointer::ButtonState::Pressed);
                if pressed {
                    told.pressed = Some(serial);
                }
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

impl Dispatch<xdg_surface::XdgSurface, Role> for Told {
    fn event(
        told: &mut Told,
        _role: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        role: &Role,
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            match role {
                Role::Window => told.configured = Some(serial),
                Role::Popup => told.popup_configured = Some(serial),
            }
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for Told {
    fn event(
        told: &mut Told,
        _popup: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            told.heard.push_back(Heard::PopupDone);
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
            xdg_toplevel::Event::Configure { width, height, states } => {
                let has = |state: xdg_toplevel::State| {
                    let (states, _) = states.as_chunks::<4>();
                    states.iter().any(|other| u32::from_ne_bytes(*other) == state as u32)
                };
                let active = has(xdg_toplevel::State::Activated);
                if active != told.active {
                    told.active = active;
                    told.heard.push_back(Heard::Active(active));
                }
                let full = has(xdg_toplevel::State::Fullscreen);
                if full != told.full {
                    told.full = full;
                    told.heard.push_back(Heard::Full(full));
                }
                let asked = (width, height, has(xdg_toplevel::State::Maximized));
                if asked != told.asked {
                    told.asked = asked;
                    told.heard.push_back(Heard::Size { width, height, maximized: asked.2 });
                }
            }
            xdg_toplevel::Event::Close => told.heard.push_back(Heard::Close),
            _ => {}
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for Told {
    fn event(
        told: &mut Told,
        _decoration: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            let by_server = mode == WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            told.heard.push_back(Heard::Decorated { by_server });
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

impl Dispatch<zwp_input_timestamps_v1::ZwpInputTimestampsV1, ()> for Told {
    fn event(
        told: &mut Told,
        _times: &zwp_input_timestamps_v1::ZwpInputTimestampsV1,
        event: zwp_input_timestamps_v1::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let zwp_input_timestamps_v1::Event::Timestamp { tv_sec_hi, tv_sec_lo, tv_nsec } = event {
            let seconds = u64::from(tv_sec_hi) << 32 | u64::from(tv_sec_lo);
            told.heard.push_back(Heard::At(seconds * 1_000_000_000 + u64::from(tv_nsec)));
        }
    }
}

delegate_noop!(Told: wl_compositor::WlCompositor);
delegate_noop!(Told: xdg_positioner::XdgPositioner);
delegate_noop!(Told: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(Told: zwp_input_timestamps_manager_v1::ZwpInputTimestampsManagerV1);
delegate_noop!(Told: ignore wl_shm::WlShm);
delegate_noop!(Told: wl_shm_pool::WlShmPool);
delegate_noop!(Told: ignore wl_buffer::WlBuffer);
delegate_noop!(Told: wp_viewporter::WpViewporter);
delegate_noop!(Told: wp_viewport::WpViewport);
delegate_noop!(Told: wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);

impl Dispatch<wl_surface::WlSurface, ()> for Told {
    fn event(
        told: &mut Told,
        surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        let wl_surface::Event::PreferredBufferScale { factor } = event else { return };
        told.zooms.retain(|(other, _)| other != surface);
        told.zooms.push((surface.clone(), factor));
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Told {
    fn event(
        told: &mut Told,
        _fraction: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        (): &(),
        _connection: &Connection,
        _handle: &QueueHandle<Told>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            told.fraction = Some(scale);
        }
    }
}
