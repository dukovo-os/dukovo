// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The program's side of `dukovo_session_v1`: giving a window a name
//! the window server knows it again by (ADR 0729).
//!
//! The protocol is spoken from the table of `window-protocols`, as the
//! title bar's is ([`crate::bar`]).

use std::ffi::CString;
use std::sync::Arc;

use wayland_client::backend::protocol::Argument;
use wayland_client::backend::{Backend, ObjectId};
use window_protocols::session::{MANAGER, manager};

use crate::Outcome;
use crate::bar::{BIND, Quiet, request};

/// Gives `toplevel` the name `name`, with a manager bound from the
/// global of the number `global` of `registry` and let go again: the
/// name outlives it.
///
/// # Errors
///
/// When the registry or the toplevel is gone, or the name has a zero
/// byte in it.
pub fn name(
    backend: &Backend,
    registry: &ObjectId,
    global: u32,
    toplevel: &ObjectId,
    name: &str,
) -> Outcome {
    let interface = CString::new(MANAGER.name).map_err(|err| err.to_string())?;
    let bind = [
        Argument::Uint(global),
        Argument::Str(Some(Box::new(interface))),
        Argument::Uint(MANAGER.version),
        Argument::NewId(ObjectId::null()),
    ];
    let bound = Some((&MANAGER, MANAGER.version));
    let manager = request(backend, registry, BIND, bind, Some(Arc::new(Quiet)), bound)?;
    let name = CString::new(name).map_err(|err| err.to_string())?;
    let names = [Argument::Object(toplevel.clone()), Argument::Str(Some(Box::new(name)))];
    request(backend, &manager, manager::NAME_WINDOW, names, None, None)?;
    request(backend, &manager, manager::DESTROY, [], None, None).map(|_| ())
}
