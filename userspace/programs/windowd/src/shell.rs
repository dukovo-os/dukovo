// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The programs' windows: what Smithay keeps of the Wayland protocols
//! windowd speaks (ADR 0712), and the choices a protocol leaves to the
//! compositor.
//!
//! A program's window shows once the program has drawn it: in the
//! middle of the screen the pointer is on, a step down and to the right
//! of the window before it, in front of the others and with the
//! keyboard. A click on a window brings it to the front and gives it
//! the keyboard. When the window with the keyboard goes, the one then
//! in front has it.
//!
//! Nothing here reaches the screens or a device. What a request of a
//! program changes is kept in [`Changed`] and [`Shell::lights`], which
//! the desktop reads after it has let the programs speak.

use std::borrow::Cow;
use std::time::Duration;

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{PopupKind, PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::keyboard::LedState;
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Serial, Size};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, get_parent,
    is_sync_subsurface,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
};

use crate::clients::Peer;

/// How far down and to the right of the window before it a new window
/// shows.
const STEP: i32 = 32;

/// After how many windows the steps start again at the middle.
const STEPS: i32 = 8;

/// What the programs' requests changed since the desktop last looked.
#[derive(Clone, Copy, Debug, Default)]
pub struct Changed {
    /// What a screen shows: a window came, went, moved or was drawn
    /// anew.
    pub windows: bool,
    /// The picture the pointer shows.
    pub pointer: bool,
}

/// The state of the protocols, and the windows.
#[derive(Debug)]
pub struct Shell {
    compositor: CompositorState,
    xdg: XdgShellState,
    shm: ShmState,
    seats: SeatState<Shell>,
    /// Kept for as long as the screens' globals are offered.
    _outputs: OutputManagerState,
    popups: PopupManager,
    /// The windows a program has asked for and not yet drawn.
    waiting: Vec<Window>,
    /// How many windows have shown, for the step of the next.
    shown: i32,
    /// The one seat: every keyboard and pointer of the machine.
    pub seat: Seat<Shell>,
    /// The windows that show, and the screens they show on.
    pub space: Space<Window>,
    /// What the pointer shows.
    pub pointer: CursorImageStatus,
    /// What changed.
    pub changed: Changed,
    /// The lock lights the keyboards are to show, when they changed.
    pub lights: Option<LedState>,
}

impl Shell {
    /// The protocols windowd speaks, offered on `display`, with a seat
    /// that has a pointer. The keyboard is added once its layout is
    /// known ([`crate::keys`]).
    pub fn new(display: &DisplayHandle) -> Shell {
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(display, "seat0");
        seat.add_pointer();
        Shell {
            compositor: CompositorState::new::<Shell>(display),
            xdg: XdgShellState::new::<Shell>(display),
            shm: ShmState::new::<Shell>(display, Vec::new()),
            seats,
            _outputs: OutputManagerState::new_with_xdg_output::<Shell>(display),
            popups: PopupManager::default(),
            waiting: Vec::new(),
            shown: 0,
            seat,
            space: Space::default(),
            pointer: CursorImageStatus::default_named(),
            changed: Changed::default(),
            lights: None,
        }
    }

    /// The window whose surface, or whose surface's ancestor, `surface`
    /// is, among those that show.
    fn window_of(&self, surface: &WlSurface) -> Option<Window> {
        let root = root_of(surface);
        let has = |window: &&Window| window.wl_surface().is_some_and(|its| *its == root);
        self.space.elements().find(has).cloned()
    }

    /// Where a window of `size` first shows.
    fn place(&self, size: Size<i32, Logical>) -> Point<i32, Logical> {
        let at = self.seat.get_pointer().map(|pointer| pointer.current_location());
        let under = at.and_then(|at| self.space.output_under(at).next().cloned());
        let screen = under.or_else(|| self.space.outputs().next().cloned());
        let Some(screen) = screen.and_then(|screen| self.space.output_geometry(&screen)) else {
            return Point::from((0, 0));
        };
        let step = self.shown % STEPS * STEP;
        let middle = |room: i32, taken: i32| ((room - taken) / 2 + step).min(room - taken).max(0);
        let x = screen.loc.x + middle(screen.size.w, size.w);
        let y = screen.loc.y + middle(screen.size.h, size.h);
        Point::from((x, y))
    }

    /// Gives the keyboard to `window`, in front of the others, or to no
    /// window.
    pub fn focus(&mut self, window: Option<&Window>) {
        if let Some(window) = window {
            self.space.raise_element(window, false);
        }
        // Each window is told here whether it is the one the person
        // works in, and only when that changed.
        for other in self.space.elements() {
            let Some(toplevel) = other.toplevel() else { continue };
            if other.set_activated(window == Some(other)) {
                toplevel.send_pending_configure();
            }
        }
        let surface = window.and_then(|window| window.wl_surface().map(Cow::into_owned));
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
        self.changed.windows = true;
    }

    /// The window in front, which has the keyboard when another goes.
    fn front(&self) -> Option<Window> {
        self.space.elements().next_back().cloned()
    }

    /// A program drew `surface` for the first time, if it is a window
    /// that waits: the window shows.
    fn show(&mut self, surface: &WlSurface) {
        let waits = |window: &Window| window.wl_surface().is_some_and(|its| *its == *surface);
        let Some(at) = self.waiting.iter().position(waits) else { return };
        let Some(toplevel) = self.waiting[at].toplevel().cloned() else { return };
        if !toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
            return;
        }
        self.waiting[at].on_commit();
        let size = self.waiting[at].geometry().size;
        if size.w == 0 || size.h == 0 {
            // No buffer yet: the program answered the configure alone.
            return;
        }
        let window = self.waiting.remove(at);
        self.space.map_element(window.clone(), self.place(size), false);
        self.shown += 1;
        self.focus(Some(&window));
    }

    /// The surface under `at`, a place of the desktop, and where its
    /// top left corner is.
    pub fn under(&self, at: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (window, place) = self.space.element_under(at)?;
        let (surface, within) =
            window.surface_under(at - place.to_f64(), WindowSurfaceType::ALL)?;
        Some((surface, (within + place).to_f64()))
    }

    /// The window under `at`.
    pub fn window_under(&self, at: Point<f64, Logical>) -> Option<Window> {
        self.space.element_under(at).map(|(window, _)| window.clone())
    }

    /// Tidies what the programs' requests left: windows and popups that
    /// went.
    pub fn refresh(&mut self) {
        self.space.refresh();
        self.popups.cleanup();
        let gone =
            matches!(&self.pointer, CursorImageStatus::Surface(surface) if !surface.is_alive());
        if gone {
            self.pointer = CursorImageStatus::default_named();
            self.changed.pointer = true;
        }
    }

    /// Whether `surface` is the picture the pointer shows.
    fn is_pointer(&self, surface: &WlSurface) -> bool {
        matches!(&self.pointer, CursorImageStatus::Surface(shown) if shown == surface)
    }

    /// The windows that show on `screen`, each once: on the first of
    /// the screens it is on.
    fn windows_on<'a>(&'a self, screen: &'a Output) -> impl Iterator<Item = &'a Window> {
        let first = move |window: &&Window| {
            self.space.outputs_for_element(window).first().is_some_and(|its| its == screen)
        };
        self.space.elements().filter(first)
    }

    /// Whether a program with a window on `screen` waits to hear that a
    /// frame was shown.
    pub fn owes_frame(&self, screen: &Output) -> bool {
        self.windows_on(screen).any(|window| {
            let mut owed = false;
            window.with_surfaces(|_, states| {
                let mut attributes = states.cached_state.get::<SurfaceAttributes>();
                owed |= !attributes.current().frame_callbacks.is_empty();
            });
            owed
        })
    }

    /// A frame showed on `screen` at `time` on the monotonic clock: the
    /// programs with a window on it, and the one whose surface the
    /// pointer shows, are told, and draw their next.
    pub fn frame_shown(&self, screen: &Output, time: Duration) {
        for window in self.windows_on(screen) {
            window.send_frame(screen, time, None, |_, _| Some(screen.clone()));
        }
        if let CursorImageStatus::Surface(surface) = &self.pointer {
            send_frames_surface_tree(surface, screen, time, None, |_, _| Some(screen.clone()));
        }
    }
}

/// The surface at the top of `surface`'s tree: a window's own, for one
/// of its subsurfaces.
fn root_of(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

impl CompositorHandler for Shell {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Every client is one windowd took in itself, with its state.
        &client.get_data::<Peer>().expect("a client windowd did not take in").compositor
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Shell>(surface);
        if is_sync_subsurface(surface) {
            // It shows with its parent's next commit.
            return;
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            // A popup that was destroyed meanwhile has nobody to
            // configure.
            let _ = popup.send_configure();
        }
        if let Some(window) = self.window_of(surface) {
            window.on_commit();
        }
        self.show(surface);
        self.changed.pointer |= self.is_pointer(surface);
        self.changed.windows = true;
    }
}

impl BufferHandler for Shell {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Shell {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl XdgShellHandler for Shell {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.waiting.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let is = |window: &Window| window.toplevel().is_some_and(|its| *its == surface);
        self.waiting.retain(|window| !is(window));
        let Some(window) = self.space.elements().find(|window| is(window)).cloned() else {
            return;
        };
        self.space.unmap_elem(&window);
        let front = self.front();
        self.focus(front.as_ref());
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // A popup whose parent is gone already is tracked by nothing,
        // and shows nowhere.
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        // A popup shows without a grab in release 1.1: a click outside
        // it reaches what is there (docs/deferred.md).
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}

impl SeatHandler for Shell {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Shell> {
        &mut self.seats
    }

    fn cursor_image(&mut self, _seat: &Seat<Shell>, image: CursorImageStatus) {
        // Asked for again whenever the pointer comes to another
        // program's surface, mostly for what it shows already.
        self.changed.pointer |= image != self.pointer;
        self.pointer = image;
    }

    fn led_state_changed(&mut self, _seat: &Seat<Shell>, lights: LedState) {
        self.lights = Some(lights);
    }
}

impl OutputHandler for Shell {}

delegate_compositor!(Shell);
delegate_shm!(Shell);
delegate_xdg_shell!(Shell);
delegate_seat!(Shell);
delegate_output!(Shell);
