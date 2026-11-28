// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A program that would rather show a frame late in a beat than wait
//! for the next (`wp_tearing_control_v1`, ADR 0728).
//!
//! A screen shows a new picture between two of its beats, so that the
//! whole screen changes at once. A program whose frames matter more the
//! sooner they show, as a game's do, may say of a surface that its
//! frames need not wait: the hint `async`. The hint is part of the
//! surface's state, and takes hold with the commit after it.
//!
//! windowd follows the hint where it costs no other program anything:
//! while the surface's buffer is all its screen shows
//! ([`crate::screens`]). The change that shows such a frame is made at
//! once, and the screen may show the frame before in its upper part and
//! this one below for one beat.

use std::sync::atomic::{AtomicBool, Ordering};

use smithay::reexports::wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_manager_v1::{
    self, WpTearingControlManagerV1,
};
use smithay::reexports::wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_v1::{
    self, PresentationHint, WpTearingControlV1,
};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::wayland::compositor::{Cacheable, with_states};

use crate::shell::Shell;

/// What a program said of when a surface's frames may show: a part of
/// the surface's state, which a commit makes the one that holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Hint {
    /// Whether a frame need not wait for the screen's beat.
    at_once: bool,
}

impl Cacheable for Hint {
    fn commit(&mut self, _display: &DisplayHandle) -> Hint {
        *self
    }

    fn merge_into(self, into: &mut Hint, _display: &DisplayHandle) {
        *into = self;
    }
}

/// Whether a surface has a tearing control object, of which it may
/// have one.
#[derive(Debug, Default)]
struct Controlled(AtomicBool);

/// Offers the tearing control manager on `display`. The global stays
/// for as long as the display does.
pub fn offer(display: &DisplayHandle) -> GlobalId {
    display.create_global::<Shell, WpTearingControlManagerV1, ()>(1, ())
}

/// Whether the program of `surface` said that its frames need not wait
/// for the screen's beat, as of the surface's last commit.
pub fn at_once(surface: &WlSurface) -> bool {
    with_states(surface, |states| states.cached_state.get::<Hint>().current().at_once)
}

/// Notes whether `surface` has a tearing control object, and returns
/// whether it had one.
fn control(surface: &WlSurface, has: bool) -> bool {
    with_states(surface, |states| {
        let had = states.data_map.get_or_insert_threadsafe(Controlled::default);
        had.0.swap(has, Ordering::Relaxed)
    })
}

/// Notes what the program said of `surface`, for the surface's next
/// commit to make hold.
fn say(surface: &WlSurface, at_once: bool) {
    with_states(surface, |states| {
        states.cached_state.get::<Hint>().pending().at_once = at_once;
    });
}

impl GlobalDispatch<WpTearingControlManagerV1, ()> for Shell {
    fn bind(
        _shell: &mut Shell,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<WpTearingControlManagerV1>,
        (): &(),
        init: &mut DataInit<'_, Shell>,
    ) {
        init.init(manager, ());
    }
}

impl Dispatch<WpTearingControlManagerV1, ()> for Shell {
    fn request(
        _shell: &mut Shell,
        _client: &Client,
        manager: &WpTearingControlManagerV1,
        request: wp_tearing_control_manager_v1::Request,
        (): &(),
        _display: &DisplayHandle,
        init: &mut DataInit<'_, Shell>,
    ) {
        use wp_tearing_control_manager_v1::{Error, Request};
        // `destroy`, and what a later version may add.
        let Request::GetTearingControl { id, surface } = request else { return };
        if control(&surface, true) {
            manager.post_error(
                Error::TearingControlExists,
                "the surface has a tearing control object already",
            );
        }
        init.init(id, surface);
    }
}

impl Dispatch<WpTearingControlV1, WlSurface> for Shell {
    fn request(
        _shell: &mut Shell,
        _client: &Client,
        _control: &WpTearingControlV1,
        request: wp_tearing_control_v1::Request,
        surface: &WlSurface,
        _display: &DisplayHandle,
        _init: &mut DataInit<'_, Shell>,
    ) {
        use wp_tearing_control_v1::Request;
        // `destroy` is told of in `destroyed`.
        let Request::SetPresentationHint { hint } = request else { return };
        // A surface that is gone has no state to keep the hint in.
        if !surface.is_alive() {
            return;
        }
        let at_once = hint == WEnum::Value(PresentationHint::Async);
        say(surface, at_once);
    }

    fn destroyed(
        _shell: &mut Shell,
        _client: ClientId,
        _control: &WpTearingControlV1,
        surface: &WlSurface,
    ) {
        // The surface's frames wait for the beat again, from its next
        // commit on.
        if !surface.is_alive() {
            return;
        }
        control(surface, false);
        say(surface, false);
    }
}
