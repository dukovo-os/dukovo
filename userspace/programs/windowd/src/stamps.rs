// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! When an input event happened, to the nanosecond
//! (`zwp_input_timestamps_v1`, ADR 0723).
//!
//! The time a key or pointer event carries is in milliseconds. A
//! program that wants it finer, as one that reckons a pointer's speed
//! or measures how long an event was on its way, makes a timestamps
//! object for its keyboard or its pointer, and is then sent the whole
//! time just before each event: the time the kernel's driver stamped
//! the event with, on the monotonic clock, of which the event's own
//! milliseconds are a part.
//!
//! A time is sent to the program an event may go to and to no other:
//! when another program's keys went down is not a program's to know.

use smithay::reexports::wayland_protocols::wp::input_timestamps::zv1::server::zwp_input_timestamps_manager_v1::{
    self, ZwpInputTimestampsManagerV1,
};
use smithay::reexports::wayland_protocols::wp::input_timestamps::zv1::server::zwp_input_timestamps_v1::ZwpInputTimestampsV1;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::shell::Shell;

/// The events a timestamps object is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Of {
    Keys,
    Pointer,
    /// The desktop has no touch screens yet: nothing is sent.
    Touch,
}

/// The programs' timestamps objects.
#[derive(Debug)]
pub struct Stamps {
    _global: GlobalId,
    objects: Vec<(Of, ZwpInputTimestampsV1)>,
}

impl Stamps {
    /// Offers the timestamps manager on `display`. The global stays for
    /// as long as the display does.
    pub fn offer(display: &DisplayHandle) -> Stamps {
        let global = display.create_global::<Shell, ZwpInputTimestampsManagerV1, ()>(1, ());
        Stamps { _global: global, objects: Vec::new() }
    }

    /// A key event of `time_ns` is about to go to the program of the
    /// surface `to`.
    pub fn keys(&self, to: Option<&WlSurface>, time_ns: u64) {
        self.send(Of::Keys, [to, None], time_ns);
    }

    /// A pointer event of `time_ns` is about to go to the program of
    /// one of the surfaces `to`: the one that has the pointer, or the
    /// one that is about to.
    pub fn pointer(&self, to: [Option<&WlSurface>; 2], time_ns: u64) {
        self.send(Of::Pointer, to, time_ns);
    }

    /// Sends `time_ns` to the objects for `of` of each program that has
    /// one of the surfaces `to`. A program that is sent a time and then
    /// no event comes to no harm: the time before an event is the
    /// event's.
    fn send(&self, of: Of, to: [Option<&WlSurface>; 2], time_ns: u64) {
        if self.objects.is_empty() {
            return;
        }
        let [first, second] = to.map(|surface| surface.and_then(Resource::client));
        let second = second.filter(|second| first.as_ref().map(Client::id) != Some(second.id()));
        let (seconds, nanoseconds) = (time_ns / 1_000_000_000, (time_ns % 1_000_000_000) as u32);
        for program in [first, second].into_iter().flatten() {
            let its = |object: &&(Of, ZwpInputTimestampsV1)| {
                object.0 == of
                    && object.1.client().is_some_and(|client| client.id() == program.id())
            };
            for (_, object) in self.objects.iter().filter(its) {
                object.timestamp((seconds >> 32) as u32, seconds as u32, nanoseconds);
            }
        }
    }
}

impl GlobalDispatch<ZwpInputTimestampsManagerV1, ()> for Shell {
    fn bind(
        _shell: &mut Shell,
        _display: &DisplayHandle,
        _client: &Client,
        manager: New<ZwpInputTimestampsManagerV1>,
        (): &(),
        init: &mut DataInit<'_, Shell>,
    ) {
        init.init(manager, ());
    }
}

impl Dispatch<ZwpInputTimestampsManagerV1, ()> for Shell {
    fn request(
        shell: &mut Shell,
        _client: &Client,
        _manager: &ZwpInputTimestampsManagerV1,
        request: zwp_input_timestamps_manager_v1::Request,
        (): &(),
        _display: &DisplayHandle,
        init: &mut DataInit<'_, Shell>,
    ) {
        use zwp_input_timestamps_manager_v1::Request;
        // The seat has one keyboard and one pointer, which every
        // keyboard and pointer object of a program is.
        let (id, of) = match request {
            Request::GetKeyboardTimestamps { id, .. } => (id, Of::Keys),
            Request::GetPointerTimestamps { id, .. } => (id, Of::Pointer),
            Request::GetTouchTimestamps { id, .. } => (id, Of::Touch),
            // `destroy`, and what a later version may add.
            _ => return,
        };
        let object = init.init(id, of);
        shell.stamps.objects.push((of, object));
    }
}

impl Dispatch<ZwpInputTimestampsV1, Of> for Shell {
    fn request(
        _shell: &mut Shell,
        _client: &Client,
        _object: &ZwpInputTimestampsV1,
        _destroy: <ZwpInputTimestampsV1 as Resource>::Request,
        _of: &Of,
        _display: &DisplayHandle,
        _init: &mut DataInit<'_, Shell>,
    ) {
        // `destroy` is the only request, and is told of in `destroyed`.
    }

    fn destroyed(shell: &mut Shell, _client: ClientId, object: &ZwpInputTimestampsV1, _of: &Of) {
        shell.stamps.objects.retain(|(_, other)| other != object);
    }
}
