// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The program's side of `dukovo_title_bar_v1`: asking the window
//! server for a part of the window's title bar (ADR 0720).
//!
//! The protocol is spoken from the tables of `window-protocols`, by
//! the numbers of its requests and events, with no code written by a
//! tool: this is how a toolkit with no generator of its own for the
//! protocol would speak it.

use std::ffi::CString;
use std::os::fd::{OwnedFd, RawFd};
use std::sync::{Arc, Mutex, PoisonError};

use wayland_client::backend::protocol::{Argument, Interface, Message};
use wayland_client::backend::{Backend, ObjectData, ObjectId};
pub use window_protocols::title_bar::Mode;
use window_protocols::title_bar::{MANAGER, bar, manager};

use crate::Outcome;

/// The number of `wl_registry.bind` among the registry's requests.
pub(crate) const BIND: u16 = 0;

/// What the window server said of the window's title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarSaid {
    /// Whose the bar is.
    pub mode: Mode,
    /// How high the bar is.
    pub height: u32,
    /// The columns at the bar's left end the window server keeps.
    pub left: u32,
    /// The columns at its right end the window server keeps.
    pub right: u32,
}

/// What the title bar object last heard. An object's events come
/// wherever the connection is read, so they are kept behind a lock for
/// the program to look at.
#[derive(Debug, Default)]
struct Hears(Mutex<Option<BarSaid>>);

impl ObjectData for Hears {
    fn event(
        self: Arc<Self>,
        _backend: &Backend,
        event: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        // The one event is `bar`. A mode this program does not know is
        // one of a later version, which it did not ask for.
        if let [
            Argument::Uint(mode),
            Argument::Uint(height),
            Argument::Uint(left),
            Argument::Uint(right),
        ] = event.args[..]
            && let Some(mode) = Mode::of(mode)
        {
            let said = BarSaid { mode, height, left, right };
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(said);
        }
        None
    }

    fn destroyed(&self, _object: ObjectId) {}
}

/// A manager hears nothing: it has no events.
#[derive(Debug)]
pub(crate) struct Quiet;

impl ObjectData for Quiet {
    fn event(
        self: Arc<Self>,
        _backend: &Backend,
        _event: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        None
    }

    fn destroyed(&self, _object: ObjectId) {}
}

/// The window's title bar object.
#[derive(Debug)]
pub struct TitleBar {
    backend: Backend,
    object: ObjectId,
    hears: Arc<Hears>,
}

/// Sends the request `opcode` of `object` with `args`, and gives the
/// object it makes, if it makes one: `makes` is what hears that
/// object's events, and `bound` its interface and version where the
/// request's table does not say (`wl_registry.bind`).
pub(crate) fn request(
    backend: &Backend,
    object: &ObjectId,
    opcode: u16,
    args: impl IntoIterator<Item = Argument<ObjectId, RawFd>>,
    makes: Option<Arc<dyn ObjectData>>,
    bound: Option<(&'static Interface, u32)>,
) -> Outcome<ObjectId> {
    let request = Message { sender_id: object.clone(), opcode, args: args.into_iter().collect() };
    backend.send_request(request, makes, bound).map_err(|_| "the object is gone".to_owned())
}

impl TitleBar {
    /// Makes the title bar object of `toplevel`, with a manager bound
    /// from the global of the number `global` of `registry` and let go
    /// again: the object outlives it.
    ///
    /// # Errors
    ///
    /// When the registry or the toplevel is gone.
    pub fn of(
        backend: &Backend,
        registry: &ObjectId,
        global: u32,
        toplevel: &ObjectId,
    ) -> Outcome<TitleBar> {
        let name = CString::new(MANAGER.name).map_err(|err| err.to_string())?;
        let bind = [
            Argument::Uint(global),
            Argument::Str(Some(Box::new(name))),
            Argument::Uint(MANAGER.version),
            Argument::NewId(ObjectId::null()),
        ];
        let bound = Some((&MANAGER, MANAGER.version));
        let manager = request(backend, registry, BIND, bind, Some(Arc::new(Quiet)), bound)?;
        let hears = Arc::new(Hears::default());
        let get = [Argument::NewId(ObjectId::null()), Argument::Object(toplevel.clone())];
        let object =
            request(backend, &manager, manager::GET_TITLE_BAR, get, Some(hears.clone()), None)?;
        request(backend, &manager, manager::DESTROY, [], None, None)?;
        Ok(TitleBar { backend: backend.clone(), object, hears })
    }

    /// What the window server last said of the bar.
    pub fn said(&self) -> Option<BarSaid> {
        *self.hears.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Names the part of the bar the program draws, from the corner of
    /// the window, for the window's next commit.
    ///
    /// # Errors
    ///
    /// When the object is gone.
    pub fn region(&self, (x, y, width, height): (i32, i32, i32, i32)) -> Outcome {
        let args = [x, y, width, height].map(Argument::Int);
        request(&self.backend, &self.object, bar::SET_REGION, args, None, None).map(|_| ())
    }

    /// Gives the bar back to the window server.
    ///
    /// # Errors
    ///
    /// When the object is gone.
    pub fn give_back(self) -> Outcome {
        request(&self.backend, &self.object, bar::DESTROY, [], None, None).map(|_| ())
    }
}
