// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A window's title bar, shared with the window's program
//! (`dukovo_title_bar_v1`, ADR 0720).
//!
//! A program that wants to draw a part of its window's title bar makes
//! a title bar object for the window. Whether it may is the person's to
//! say ([`crate::settings`]); the [`Shell`] settles it and tells the
//! program with the `bar` event, each time before a configure of the
//! window.
//!
//! Every window has a [`TitleBar`], with or without such an object:
//! whose the bar is as the program was last told, and as the program
//! last drew. The two differ from the configure that tells the program
//! until the commit that answers it, and the window's frame is drawn
//! as the program drew, so that frame and content never differ on the
//! screen.
//!
//! The protocol is spoken from the tables of `window-protocols`, with
//! no code written by a tool.

use std::cell::{RefCell, RefMut};
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use smithay::reexports::wayland_server::backend::protocol::{Argument, Message};
use smithay::reexports::wayland_server::backend::{
    ClientId, GlobalHandler, GlobalId, Handle, ObjectData, ObjectId,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::Serial;
use window_frame::{Bar, Layout, Rect, Theme};
use window_protocols::title_bar::{MANAGER, Mode, bar, manager};

use crate::shell::Shell;

/// Further than any window reaches: a region a program names is kept
/// within this of the window's corner, so that no sum of its sides
/// leaves the numbers' range.
const FAR: i32 = 1 << 20;

/// Whose the title bar of one window is.
#[derive(Debug)]
pub struct TitleBar {
    /// The program's title bar object for the window, while it has one.
    object: Option<ObjectId>,
    /// Whether the object has been sent a `bar` event.
    heard: bool,
    /// The region the program last named, which is the window's from
    /// its next commit.
    named: Option<Rect>,
    /// The region the program draws.
    region: Option<Rect>,
    /// What the program was told and has not drawn yet, each with the
    /// serial of the configure that followed.
    out: VecDeque<(Serial, Bar)>,
    /// Whose the bar is in what the program last drew.
    drawn: Bar,
}

impl TitleBar {
    /// The title bar of `window`.
    ///
    /// # Panics
    ///
    /// If it is taken while it is held already: every use here is over
    /// before the next begins.
    pub fn of(window: &Window) -> RefMut<'_, TitleBar> {
        let new = || {
            RefCell::new(TitleBar {
                object: None,
                heard: false,
                named: None,
                region: None,
                out: VecDeque::new(),
                drawn: Bar::Full,
            })
        };
        window.user_data().get_or_insert(new).borrow_mut()
    }

    /// Whether the window's program asks to share the bar.
    pub fn asked(&self) -> bool {
        self.object.is_some()
    }

    /// Whose the bar is as the program was last told.
    pub fn told(&self) -> Bar {
        self.out.back().map_or(self.drawn, |(_, bar)| *bar)
    }

    /// Whose the bar is in what the program last drew, and the region
    /// the program draws of a bar that is shared, from the corner of
    /// the window's geometry.
    pub fn drawn(&self) -> (Bar, Option<Rect>) {
        match self.drawn {
            Bar::Full => (Bar::Full, None),
            Bar::Shared => (Bar::Shared, self.region),
        }
    }

    /// The program committed the window's surface, having answered the
    /// configure of `answered` and those before it.
    pub fn committed(&mut self, answered: Option<Serial>) {
        self.region = self.named;
        while let Some((serial, bar)) = self.out.front().copied()
            && answered.is_some_and(|answered| answered.is_no_older_than(&serial))
        {
            self.drawn = bar;
            self.out.pop_front();
        }
    }

    /// Whether the program is to hear that the bar is `bar`'s: it was
    /// last told otherwise, or its object has heard nothing yet.
    pub fn owes(&self, bar: Bar) -> bool {
        bar != self.told() || (self.object.is_some() && !self.heard)
    }

    /// Tells the program's object, if there is one, that the bar is
    /// `bar`'s, with the sizes of `theme`.
    pub fn tell(&mut self, display: &DisplayHandle, theme: &Theme, bar: Bar) {
        let Some(object) = &self.object else { return };
        let mode = match bar {
            Bar::Full => Mode::Full,
            Bar::Shared => Mode::Shared,
        };
        let (left, right) = Layout::insets(theme);
        let args = [mode as u32, theme.bar as u32, left as u32, right as u32];
        let event = Message {
            sender_id: object.clone(),
            opcode: bar::BAR,
            args: args.into_iter().map(Argument::Uint).collect(),
        };
        // An object whose program is gone hears nothing.
        let _ = display.backend_handle().send_event(event);
        self.heard = true;
    }

    /// The program draws as it was told from its commit that answers
    /// the configure of `serial`, or, for a window that has had no
    /// configure yet, from its first.
    pub fn sent(&mut self, bar: Bar, serial: Option<Serial>) {
        match serial {
            Some(serial) => self.out.push_back((serial, bar)),
            None => self.drawn = bar,
        }
    }
}

/// The global, and each program's bound manager: a manager keeps
/// nothing.
#[derive(Debug)]
struct Manager;

/// A program's title bar object for the window whose own surface is
/// `surface`, or for none: the window was gone when it was made.
#[derive(Debug)]
struct Object {
    surface: Option<WlSurface>,
}

impl Object {
    /// The window `object`, which this is the data of, is the title
    /// bar object of. A surface may have become another window's since
    /// the object was made; the object then does nothing more.
    fn window(&self, shell: &Shell, object: &ObjectId) -> Option<Window> {
        let window = shell.window_with(self.surface.as_ref()?)?;
        let its = TitleBar::of(&window).object.as_ref() == Some(object);
        its.then_some(window)
    }
}

/// Offers the title bar manager on `display`. The global stays for as
/// long as the display does.
pub fn offer(display: &DisplayHandle) -> GlobalId {
    display.backend_handle().create_global::<Shell>(&MANAGER, MANAGER.version, Arc::new(Manager))
}

impl GlobalHandler<Shell> for Manager {
    fn bind(
        self: Arc<Self>,
        _handle: &Handle,
        _shell: &mut Shell,
        _client: ClientId,
        _global: GlobalId,
        _manager: ObjectId,
    ) -> Arc<dyn ObjectData<Shell>> {
        self
    }
}

impl ObjectData<Shell> for Manager {
    fn request(
        self: Arc<Self>,
        handle: &Handle,
        shell: &mut Shell,
        _client: ClientId,
        request: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<Shell>>> {
        // The backend has held the arguments to the table: the other
        // request, `destroy`, has none and makes nothing.
        let [Argument::NewId(object), Argument::Object(toplevel)] = &request.args[..] else {
            return None;
        };
        debug_assert_eq!(request.opcode, manager::GET_TITLE_BAR);
        let display = DisplayHandle::from(handle.clone());
        let window = XdgToplevel::from_id(&display, toplevel.clone())
            .ok()
            .and_then(|toplevel| shell.window_of_toplevel(&toplevel));
        let Some(window) = window else { return Some(Arc::new(Object { surface: None })) };
        let has = TitleBar::of(&window).asked();
        if has {
            let said = c"the toplevel has a title bar object already".to_owned();
            handle.post_error(request.sender_id, manager::ALREADY_EXISTS, said);
            return None;
        }
        {
            let mut bar = TitleBar::of(&window);
            bar.object = Some(object.clone());
            bar.heard = false;
            bar.named = None;
        }
        let surface = window.toplevel().map(|toplevel| toplevel.wl_surface().clone());
        shell.settle_bar(&window);
        Some(Arc::new(Object { surface }))
    }

    fn destroyed(
        self: Arc<Self>,
        _handle: &Handle,
        _shell: &mut Shell,
        _client: ClientId,
        _manager: ObjectId,
    ) {
    }
}

impl ObjectData<Shell> for Object {
    fn request(
        self: Arc<Self>,
        _handle: &Handle,
        shell: &mut Shell,
        _client: ClientId,
        request: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<Shell>>> {
        // `destroy` has no arguments, and is told of in `destroyed`.
        let [Argument::Int(x), Argument::Int(y), Argument::Int(width), Argument::Int(height)] =
            request.args[..]
        else {
            return None;
        };
        debug_assert_eq!(request.opcode, bar::SET_REGION);
        let window = self.window(shell, &request.sender_id)?;
        let near = |value: i32| value.clamp(-FAR, FAR);
        let named = Rect::new(near(x), near(y), near(width), near(height));
        TitleBar::of(&window).named = (!named.is_empty()).then_some(named);
        None
    }

    fn destroyed(
        self: Arc<Self>,
        _handle: &Handle,
        shell: &mut Shell,
        _client: ClientId,
        object: ObjectId,
    ) {
        let Some(window) = self.window(shell, &object) else { return };
        {
            let mut bar = TitleBar::of(&window);
            bar.object = None;
            bar.named = None;
        }
        shell.settle_bar(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> TitleBar {
        TitleBar {
            object: None,
            heard: false,
            named: None,
            region: None,
            out: VecDeque::new(),
            drawn: Bar::Full,
        }
    }

    #[test]
    fn the_bar_is_as_drawn_until_the_program_answers_what_it_was_told() {
        let mut bar = bar();
        let (first, second) = (Serial::from(7), Serial::from(9));
        bar.sent(Bar::Shared, Some(first));
        bar.sent(Bar::Full, Some(second));
        assert_eq!((bar.told(), bar.drawn().0), (Bar::Full, Bar::Full));
        // A commit that answers an older configure changes nothing.
        bar.committed(Some(Serial::from(6)));
        assert_eq!(bar.drawn().0, Bar::Full);
        bar.committed(Some(first));
        assert_eq!((bar.told(), bar.drawn().0), (Bar::Full, Bar::Shared));
        bar.committed(Some(Serial::from(8)));
        assert_eq!(bar.drawn().0, Bar::Shared);
        // One that answers a later configure answers those before it.
        bar.committed(Some(Serial::from(12)));
        assert_eq!((bar.told(), bar.drawn().0), (Bar::Full, Bar::Full));
        assert!(bar.out.is_empty());
    }

    #[test]
    fn a_window_with_no_configure_yet_draws_as_it_is_told() {
        let mut bar = bar();
        bar.sent(Bar::Shared, None);
        assert_eq!((bar.told(), bar.drawn().0), (Bar::Shared, Bar::Shared));
        assert!(!bar.owes(Bar::Shared));
        assert!(bar.owes(Bar::Full));
    }

    #[test]
    fn a_region_is_the_windows_from_the_next_commit_and_of_a_shared_bar_alone() {
        let mut bar = bar();
        let region = Rect::new(40, 4, 200, 28);
        bar.named = Some(region);
        bar.sent(Bar::Shared, None);
        assert_eq!(bar.drawn(), (Bar::Shared, None));
        bar.committed(None);
        assert_eq!(bar.drawn(), (Bar::Shared, Some(region)));
        bar.sent(Bar::Full, None);
        assert_eq!(bar.drawn(), (Bar::Full, None));
    }
}
