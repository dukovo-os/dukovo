// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `dukovo_session_v1`: the names windows are known again by, for the
//! window server to show them where they were (ADR 0729,
//! `protocols/dukovo-session-v1.xml`).

use wayland_backend::protocol::{AllowNull, ArgumentType, Interface, MessageDesc};

use crate::XDG_TOPLEVEL;

/// How many bytes a window's name has at the most.
pub const NAME_MOST: usize = 255;

/// `dukovo_session_manager_v1`: the global that names windows.
pub static MANAGER: Interface = Interface {
    name: "dukovo_session_manager_v1",
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
            name: "name_window",
            signature: &[ArgumentType::Object(AllowNull::No), ArgumentType::Str(AllowNull::No)],
            since: 1,
            is_destructor: false,
            child_interface: None,
            arg_interfaces: &[&XDG_TOPLEVEL],
        },
    ],
    events: &[],
    c_ptr: None,
};

/// The numbers of the manager's requests and errors.
pub mod manager {
    /// The request `destroy`.
    pub const DESTROY: u16 = 0;
    /// The request `name_window`: the toplevel, then the name.
    pub const NAME_WINDOW: u16 = 1;
    /// The error `bad_name`.
    pub const BAD_NAME: u32 = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml;

    const XML: &str = include_str!("../protocols/dukovo-session-v1.xml");

    #[test]
    fn the_table_is_the_xml() {
        let written = xml::written(&MANAGER, &[("error.bad_name", manager::BAD_NAME)]);
        assert_eq!(vec![written], xml::read(XML));
    }

    #[test]
    fn the_numbers_are_the_messages_places() {
        assert_eq!(MANAGER.requests[usize::from(manager::DESTROY)].name, "destroy");
        assert_eq!(MANAGER.requests[usize::from(manager::NAME_WINDOW)].name, "name_window");
    }

    #[test]
    fn the_xml_says_how_long_a_name_may_be() {
        assert!(XML.contains(&format!("longer than {NAME_MOST} bytes")));
        assert!(XML.contains(&format!("of more than {NAME_MOST}, or that is not UTF-8")));
    }
}
