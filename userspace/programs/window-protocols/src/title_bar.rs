// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `dukovo_title_bar_v1`: a program's own part of its window's title
//! bar (ADR 0720, `protocols/dukovo-title-bar-v1.xml`).

use wayland_backend::protocol::{AllowNull, ArgumentType, Interface, MessageDesc};

use crate::XDG_TOPLEVEL;

/// `dukovo_title_bar_manager_v1`: the global that gives windows their
/// title bar objects.
pub static MANAGER: Interface = Interface {
    name: "dukovo_title_bar_manager_v1",
    version: 1,
    requests: &[
        MessageDesc {
            name: "destroy",
            signature: &[],
            since: 1,
            is_destructor: true,
            child_interface: None,
            arg_interfaces: &[],
        },
        MessageDesc {
            name: "get_title_bar",
            signature: &[ArgumentType::NewId, ArgumentType::Object(AllowNull::No)],
            since: 1,
            is_destructor: false,
            child_interface: Some(&TITLE_BAR),
            arg_interfaces: &[&XDG_TOPLEVEL],
        },
    ],
    events: &[],
    c_ptr: None,
};

/// `dukovo_title_bar_v1`: one window's title bar, shared with the
/// window's program.
pub static TITLE_BAR: Interface = Interface {
    name: "dukovo_title_bar_v1",
    version: 1,
    requests: &[
        MessageDesc {
            name: "destroy",
            signature: &[],
            since: 1,
            is_destructor: true,
            child_interface: None,
            arg_interfaces: &[],
        },
        MessageDesc {
            name: "set_region",
            signature: &[ArgumentType::Int; 4],
            since: 1,
            is_destructor: false,
            child_interface: None,
            arg_interfaces: &[],
        },
    ],
    events: &[MessageDesc {
        name: "bar",
        signature: &[ArgumentType::Uint; 4],
        since: 1,
        is_destructor: false,
        child_interface: None,
        arg_interfaces: &[],
    }],
    c_ptr: None,
};

/// The numbers of the manager's requests and errors.
pub mod manager {
    /// The request `destroy`.
    pub const DESTROY: u16 = 0;
    /// The request `get_title_bar`: the new object, then the toplevel.
    pub const GET_TITLE_BAR: u16 = 1;
    /// The error `already_exists`.
    pub const ALREADY_EXISTS: u32 = 0;
}

/// The numbers of a title bar's requests and events.
pub mod bar {
    /// The request `destroy`.
    pub const DESTROY: u16 = 0;
    /// The request `set_region`: x, y, width, height.
    pub const SET_REGION: u16 = 1;
    /// The event `bar`: mode, height, left, right.
    pub const BAR: u16 = 0;
}

/// Whose a title bar is, as the `bar` event says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The window server's whole, above the window.
    Full = 0,
    /// Over the top rows of the window, with the program's region open.
    Shared = 1,
}

impl Mode {
    /// The mode `value` names in a `bar` event, if any.
    pub const fn of(value: u32) -> Option<Mode> {
        match value {
            0 => Some(Mode::Full),
            1 => Some(Mode::Shared),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::{self, Read};

    const XML: &str = include_str!("../protocols/dukovo-title-bar-v1.xml");

    #[test]
    fn the_tables_are_the_xml() {
        let modes = [("mode.full", Mode::Full as u32), ("mode.shared", Mode::Shared as u32)];
        let written: Vec<Read> = vec![
            xml::written(&MANAGER, &[("error.already_exists", manager::ALREADY_EXISTS)]),
            xml::written(&TITLE_BAR, &modes),
        ];
        assert_eq!(written, xml::read(XML));
    }

    #[test]
    fn the_numbers_are_the_messages_places() {
        assert_eq!(MANAGER.requests[usize::from(manager::DESTROY)].name, "destroy");
        assert_eq!(MANAGER.requests[usize::from(manager::GET_TITLE_BAR)].name, "get_title_bar");
        assert_eq!(TITLE_BAR.requests[usize::from(bar::DESTROY)].name, "destroy");
        assert_eq!(TITLE_BAR.requests[usize::from(bar::SET_REGION)].name, "set_region");
        assert_eq!(TITLE_BAR.events[usize::from(bar::BAR)].name, "bar");
    }

    #[test]
    fn a_mode_is_read_from_its_value() {
        assert_eq!(Mode::of(Mode::Full as u32), Some(Mode::Full));
        assert_eq!(Mode::of(Mode::Shared as u32), Some(Mode::Shared));
        assert_eq!(Mode::of(2), None);
    }
}
