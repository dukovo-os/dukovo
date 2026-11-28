// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The programs' windows: what Smithay keeps of the Wayland protocols
//! windowd speaks (ADR 0712), and the choices a protocol leaves to the
//! compositor.
//!
//! A program's window shows once the program has drawn it, in front of
//! the others and with the keyboard ([`crate::windows`] says where). A
//! click on a window brings it to the front and gives it the keyboard.
//! When the window with the keyboard goes, the one then in front has
//! it.
//!
//! A program may ask for its window to be maximized, restored or
//! minimized, which is done as the frame's buttons do it. It may ask
//! for the pointer to move or resize the window, or for the window's
//! menu, with the serial of a button it was told went down: those are
//! kept in [`Shell::asked`] for the window manager
//! ([`crate::manager`]). A program that asks for a window to fill the
//! screen with nothing around it is answered with the window as it is,
//! in this step of release 1.1 (docs/deferred.md).
//!
//! A popup that asks for the keyboard and the pointer has them until it
//! goes. A press outside it and its program's other surfaces ends it,
//! and so does whatever gives the keyboard to another window: the
//! popup's program is told it is done, and the keyboard goes where it
//! was to go.
//!
//! Every window has windowd's frame, whatever its program asks: a
//! program that asks who draws the window's decorations
//! (xdg-decoration) is answered that the window server does. A program
//! may ask to draw a part of the title bar itself
//! ([`crate::titlebar`]); [`Shell::settle_bar`] says whether it does,
//! by the person's settings.
//!
//! Nothing here reaches the screens or a device. What a request of a
//! program changes is kept in [`Changed`] and [`Shell::lights`], which
//! the desktop reads after it has let the programs speak.

use std::borrow::Cow;
use std::time::Duration;

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{
    PopupGrab, PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy,
    Window, find_popup_root_surface,
};
use smithay::input::keyboard::LedState;
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{
    ResizeEdge, State, WmCapabilities, XdgToplevel,
};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, get_parent,
    is_sync_subsurface, with_states,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_xdg_decoration,
    delegate_xdg_shell,
};
use sysapi::time::monotonic_ns;
use window_frame::{Bar, Edge};

use crate::clients::Peer;
use crate::manager::Asked;
use crate::settings::TitleBars;
use crate::titlebar::{self, TitleBar};
use crate::windows::Windows;
use crate::windows::resize::answered;

/// What the programs' requests changed since the desktop last looked.
#[derive(Clone, Copy, Debug, Default)]
pub struct Changed {
    /// What a screen shows: a window came, went, moved or was drawn
    /// anew.
    pub windows: bool,
    /// The picture the pointer shows.
    pub pointer: bool,
    /// Which applications may draw a part of their title bars: the
    /// person chose for one, and their settings are to be written.
    pub title_bars: bool,
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
    /// Kept for as long as programs may ask who draws a window's
    /// decorations.
    _decorations: XdgDecorationState,
    /// Kept for as long as programs may ask for a part of a title bar.
    _title_bars: GlobalId,
    /// Where events of windowd's own protocols are sent from.
    display: DisplayHandle,
    popups: PopupManager,
    /// The popups that have the keyboard and the pointer, with the
    /// surface of the window they are of.
    grabbed: Option<(WlSurface, PopupGrab<Shell>)>,
    /// The windows a program has asked for and not yet drawn.
    waiting: Vec<Window>,
    /// The window the person works in: the one with the keyboard.
    active: Option<Window>,
    /// The one seat: every keyboard and pointer of the machine.
    pub seat: Seat<Shell>,
    /// The windows a program has drawn, and the screens.
    pub windows: Windows,
    /// What the programs asked of the window manager.
    pub asked: Vec<Asked>,
    /// What the pointer shows over a program's surface.
    pub pointer: CursorImageStatus,
    /// Which applications may draw a part of their title bars.
    pub title_bars: TitleBars,
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
        let does = [WmCapabilities::WindowMenu, WmCapabilities::Maximize, WmCapabilities::Minimize];
        Shell {
            compositor: CompositorState::new::<Shell>(display),
            xdg: XdgShellState::new_with_capabilities::<Shell>(display, does),
            shm: ShmState::new::<Shell>(display, Vec::new()),
            seats,
            _outputs: OutputManagerState::new_with_xdg_output::<Shell>(display),
            _decorations: XdgDecorationState::new::<Shell>(display),
            _title_bars: titlebar::offer(display),
            display: display.clone(),
            popups: PopupManager::default(),
            grabbed: None,
            waiting: Vec::new(),
            active: None,
            seat,
            windows: Windows::default(),
            asked: Vec::new(),
            pointer: CursorImageStatus::default_named(),
            title_bars: TitleBars::default(),
            changed: Changed::default(),
            lights: None,
        }
    }

    /// The window whose own surface is `surface`, drawn or not yet.
    pub fn window_with(&self, surface: &WlSurface) -> Option<Window> {
        let waits = |window: &&Window| window.wl_surface().is_some_and(|its| *its == *surface);
        self.windows.of_surface(surface).or_else(|| self.waiting.iter().find(waits)).cloned()
    }

    /// The window `toplevel` is the protocol's object of, drawn or not
    /// yet.
    pub fn window_of_toplevel(&self, toplevel: &XdgToplevel) -> Option<Window> {
        self.window_with(self.xdg.get_toplevel(toplevel)?.wl_surface())
    }

    /// Settles whose the title bar of `window` is: shared with its
    /// program if the program asks and the person's settings let its
    /// application, and windowd's whole otherwise. A program that has
    /// that to hear is told, and asked to draw the window so: with the
    /// bar's rows more or fewer of its own, for the same frame.
    pub fn settle_bar(&mut self, window: &Window) {
        let Some(toplevel) = window.toplevel().filter(|toplevel| toplevel.alive()) else { return };
        let theme = *self.windows.theme();
        let mut bar = TitleBar::of(window);
        let shared = bar.asked() && self.title_bars.lets(&app_of(toplevel));
        let wanted = if shared { Bar::Shared } else { Bar::Full };
        if !bar.owes(wanted) {
            return;
        }
        let grows = match (bar.told(), wanted) {
            (Bar::Full, Bar::Shared) => theme.bar,
            (Bar::Shared, Bar::Full) => -theme.bar,
            _ => 0,
        };
        bar.tell(&self.display, &theme, wanted);
        toplevel.with_pending_state(|state| {
            if let Some(size) = &mut state.size {
                size.h = (size.h + grows).max(1);
            }
        });
        // A window with no configure yet has its first when its
        // program commits, after what it was told here.
        let serial = toplevel.is_initial_configure_sent().then(|| toplevel.send_configure());
        bar.sent(wanted, serial);
        drop(bar);
        self.windows.bar_told(window, grows);
    }

    /// Settles whose every window's title bar is: the person's
    /// settings changed.
    pub fn settle_bars(&mut self) {
        let drawn = self.windows.by_use().into_iter().map(|framed| framed.window.clone());
        let all: Vec<Window> = drawn.chain(self.waiting.iter().cloned()).collect();
        for window in &all {
            self.settle_bar(window);
        }
    }

    /// The window whose surface, or whose surface's ancestor, `surface`
    /// is, among those a program has drawn.
    fn window_of(&self, surface: &WlSurface) -> Option<Window> {
        self.windows.of_surface(&root_of(surface)).cloned()
    }

    /// The window of `toplevel`, if its program has drawn it.
    fn window(&self, toplevel: &ToplevelSurface) -> Option<Window> {
        self.windows.of_surface(toplevel.wl_surface()).cloned()
    }

    /// Where the pointer is.
    fn pointer_at(&self) -> Point<f64, Logical> {
        self.seat.get_pointer().map_or_default(|pointer| pointer.current_location())
    }

    /// The window the person works in.
    pub fn active(&self) -> Option<&Window> {
        self.active.as_ref()
    }

    /// Gives the keyboard to `window`, in front of the others, or to no
    /// window. A window that is minimized shows again.
    pub fn focus(&mut self, window: Option<&Window>) {
        self.end_popups(window);
        if let Some(window) = window {
            self.windows.unminimize(window);
            self.windows.used(window);
        }
        // Each window is told here whether it is the one the person
        // works in, and only when that changed.
        for other in self.windows.by_use() {
            let Some(toplevel) = other.window.toplevel() else { continue };
            if other.window.set_activated(window == Some(&other.window)) {
                toplevel.send_pending_configure();
            }
        }
        let surface = window.and_then(|window| window.wl_surface().map(Cow::into_owned));
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
        self.active = window.cloned();
        self.changed.windows = true;
    }

    /// The popups that have the keyboard and the pointer are done,
    /// unless they are of `keep`: the keyboard is to go elsewhere, or a
    /// key of windowd's was pressed.
    pub fn end_popups(&mut self, keep: Option<&Window>) {
        let Some((root, mut grab)) = self.grabbed.take() else { return };
        if grab.has_ended() {
            return;
        }
        if keep.and_then(WaylandFocus::wl_surface).is_some_and(|its| *its == root) {
            self.grabbed = Some((root, grab));
            return;
        }
        grab.ungrab(PopupUngrabStrategy::All);
        let (serial, before) = (grab.serial(), grab.previous_serial());
        let next = SERIAL_COUNTER.next_serial();
        if let Some(pointer) = self.seat.get_pointer()
            && (pointer.has_grab(serial) || before.is_some_and(|before| pointer.has_grab(before)))
        {
            // The pointer's grab takes the keyboard's with it, and the
            // keyboard is the popups' window's again.
            pointer.unset_grab(self, next, now_ms());
        }
        if let Some(keyboard) = self.seat.get_keyboard()
            && (keyboard.has_grab(serial) || before.is_some_and(|before| keyboard.has_grab(before)))
        {
            keyboard.unset_grab(self);
            keyboard.set_focus(self, Some(root), next);
        }
    }

    /// Gives the keyboard to the window in front, if `window`, which
    /// went or is off the screens, had it.
    fn pass_on(&mut self, window: &Window) {
        if self.active.as_ref() == Some(window) {
            let front = self.windows.front();
            self.focus(front.as_ref());
        }
        self.changed.windows = true;
    }

    /// Takes `window` off the screens. If it had the keyboard, the
    /// window then in front has it.
    pub fn minimize(&mut self, window: &Window) {
        self.windows.minimize(window);
        self.pass_on(window);
    }

    /// A program drew `surface` for the first time, if it is a window
    /// that waits: the window shows.
    fn show(&mut self, surface: &WlSurface) {
        let waits = |window: &Window| window.wl_surface().is_some_and(|its| *its == *surface);
        let Some(at) = self.waiting.iter().position(waits) else { return };
        let Some(toplevel) = self.waiting[at].toplevel().cloned() else { return };
        if !toplevel.is_initial_configure_sent() {
            let bar = TitleBar::of(&self.waiting[at]).told();
            let room = self.windows.room(self.pointer_at(), bar);
            toplevel.with_pending_state(|state| state.bounds = Some(room));
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
        self.windows.show(window.clone(), self.pointer_at());
        self.focus(Some(&window));
    }

    /// The window of `toplevel`, if `serial` is that of a button's press
    /// its program was told of and the button is still down, with the
    /// button's Linux code. The pointer is the program's no longer.
    fn pressed_on(&mut self, toplevel: &ToplevelSurface, serial: Serial) -> Option<(Window, u16)> {
        let pointer = self.seat.get_pointer()?;
        if !pointer.has_grab(serial) {
            return None;
        }
        let start = pointer.grab_start_data()?;
        let (pressed, _) = start.focus?;
        if !pressed.id().same_client_as(&toplevel.wl_surface().id()) {
            return None;
        }
        let window = self.window(toplevel)?;
        // The desktop tells the program next that the pointer left.
        pointer.unset_grab(self, serial, now_ms());
        Some((window, start.button as u16))
    }

    /// Tidies what the programs' requests left: windows and popups that
    /// went.
    pub fn refresh(&mut self) {
        self.windows.refresh();
        self.popups.cleanup();
        if self.grabbed.as_ref().is_some_and(|(_, grab)| grab.has_ended()) {
            self.grabbed = None;
        }
        if self.active.as_ref().is_some_and(|active| self.windows.framed(active).is_none()) {
            self.active = None;
        }
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
        let space = &self.windows.space;
        let first = move |window: &&Window| {
            space.outputs_for_element(window).first().is_some_and(|its| its == screen)
        };
        space.elements().filter(first)
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

/// The time now as the protocols carry it: the monotonic clock's, in
/// milliseconds, the low 32 bits.
fn now_ms() -> u32 {
    (monotonic_ns() / 1_000_000) as u32
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

/// The name the program of `toplevel` gave its application, or none.
pub fn app_of(toplevel: &ToplevelSurface) -> String {
    with_states(toplevel.wl_surface(), |states| {
        let role = states.data_map.get::<XdgToplevelSurfaceData>();
        role.and_then(|role| role.lock().ok()?.app_id.clone()).unwrap_or_default()
    })
}

/// The edge of a frame a program names with `edges`.
fn edge_of(edges: ResizeEdge) -> Option<Edge> {
    Some(match edges {
        ResizeEdge::Top => Edge::Top,
        ResizeEdge::Bottom => Edge::Bottom,
        ResizeEdge::Left => Edge::Left,
        ResizeEdge::Right => Edge::Right,
        ResizeEdge::TopLeft => Edge::TopLeft,
        ResizeEdge::TopRight => Edge::TopRight,
        ResizeEdge::BottomLeft => Edge::BottomLeft,
        ResizeEdge::BottomRight => Edge::BottomRight,
        _ => return None,
    })
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
        if let Some(window) = self.window_with(surface)
            && let Some(toplevel) = window.toplevel()
        {
            TitleBar::of(&window).committed(answered(toplevel));
        }
        if let Some(window) = self.window_of(surface) {
            window.on_commit();
            self.windows.committed(&window);
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
        let Some(window) = self.window(&surface) else { return };
        self.windows.gone(&window);
        self.pass_on(&window);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_with(surface.wl_surface()) {
            self.settle_bar(&window);
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window(&surface) {
            self.windows.retitled(&window);
            self.changed.windows = true;
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, serial: Serial) {
        if let Some((window, code)) = self.pressed_on(&surface, serial) {
            self.asked.push(Asked::Move(window, code));
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: WlSeat,
        serial: Serial,
        edges: ResizeEdge,
    ) {
        let Some(edge) = edge_of(edges) else { return };
        if let Some((window, code)) = self.pressed_on(&surface, serial) {
            self.asked.push(Asked::Resize(window, edge, code));
        }
    }

    fn show_window_menu(
        &mut self,
        surface: ToplevelSurface,
        _seat: WlSeat,
        serial: Serial,
        location: Point<i32, Logical>,
    ) {
        // A program may ask with a key's serial as well. With that of
        // a button that is still down, the pointer is the program's no
        // longer.
        let _ = self.pressed_on(&surface, serial);
        let Some(window) = self.window(&surface) else { return };
        let Some(framed) = self.windows.framed(&window) else { return };
        let at = framed.geometry_at() + location;
        self.asked.push(Asked::Menu(window, at));
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        match self.window(&surface) {
            Some(window) => self.windows.maximize(&window),
            // A window that has not shown yet shows maximized.
            None => {
                let bar = self.window_with(surface.wl_surface());
                let bar = bar.map_or(Bar::Full, |window| TitleBar::of(&window).told());
                let size = self.windows.filling(self.pointer_at(), bar);
                surface.with_pending_state(|state| {
                    state.states.set(State::Maximized);
                    state.size = Some(size);
                });
            }
        }
        // The protocol has the program wait for an answer, whether or
        // not anything changed.
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        match self.window(&surface) {
            Some(window) => self.windows.restore(&window),
            None => surface.with_pending_state(|state| {
                state.states.unset(State::Maximized);
                state.size = None;
            }),
        }
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window(&surface) {
            self.minimize(&window);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // A popup whose parent is gone already is tracked by nothing,
        // and shows nowhere.
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, serial: Serial) {
        let popup = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&popup) else { return };
        let seat = self.seat.clone();
        let Ok(mut grab) = self.popups.grab_popup(root.clone(), popup, &seat, serial) else {
            return;
        };
        // The keyboard and the pointer are the popup's unless another
        // has them that is not a popup under this one. A popup that may
        // not have them is told it is done.
        let before = grab.previous_serial().unwrap_or(serial);
        if let Some(keyboard) = seat.get_keyboard() {
            let under = keyboard.has_grab(serial) || keyboard.has_grab(before);
            if keyboard.is_grabbed() && !under {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            let under = pointer.has_grab(serial) || pointer.has_grab(before);
            if pointer.is_grabbed() && !under {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
        self.grabbed = Some((root, grab));
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

impl XdgDecorationHandler for Shell {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        answer_decoration(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: Mode) {
        answer_decoration(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        answer_decoration(&toplevel);
    }
}

/// Answers the program of `toplevel`, which said how it would have the
/// window decorated, or that it can leave that to the window server:
/// whatever it asks, the frame is windowd's (ADR 0718). The protocol
/// has the program wait for a configure, which for a window that has
/// had none yet is its first.
fn answer_decoration(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(Mode::ServerSide));
    if toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
    }
}

delegate_compositor!(Shell);
delegate_shm!(Shell);
delegate_xdg_shell!(Shell);
delegate_seat!(Shell);
delegate_output!(Shell);
delegate_xdg_decoration!(Shell);
