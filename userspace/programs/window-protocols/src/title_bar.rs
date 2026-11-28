// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `dukovo_title_bar_v1`: a program's own part of its window's title
//! bar (ADR 0720, `protocols/dukovo-title-bar-v1.xml`).

use wayland_backend::protocol::{AllowNull, ArgumentType, Interface, MessageDesc};

/// The interface a window is named by in `get_title_bar`. An argument's
/// interface is held to its name alone, so the name is all there is of
/// it here: the interface itself is the xdg-shell protocol's.
pub static XDG_TOPLEVEL: Interface =
    Interface { name: "xdg_toplevel", version: 1, requests: &[], events: &[], c_ptr: None };

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
    use quick_xml::events::{BytesStart, Event};
    use quick_xml::{Reader, XmlVersion};

    use super::*;

    const XML: &str = include_str!("../protocols/dukovo-title-bar-v1.xml");

    /// A request or an event as the XML has it.
    #[derive(Debug, Default, PartialEq)]
    struct Message {
        name: String,
        destructor: bool,
        /// Each argument's type, and the interface it names.
        args: Vec<(String, Option<String>)>,
    }

    /// An interface as the XML has it.
    #[derive(Debug, Default, PartialEq)]
    struct Read {
        name: String,
        version: u32,
        requests: Vec<Message>,
        events: Vec<Message>,
        /// Every entry of every enum, as `enum.entry`, with its value.
        entries: Vec<(String, u32)>,
    }

    fn attribute(of: &BytesStart<'_>, name: &str) -> Option<String> {
        let found = of.try_get_attribute(name).expect("an attribute that cannot be read")?;
        let value = found.normalized_value(XmlVersion::Explicit1_0);
        Some(value.expect("an attribute's value that cannot be read").into_owned())
    }

    /// The interfaces of the XML.
    fn read() -> Vec<Read> {
        let mut reader = Reader::from_str(XML);
        let mut interfaces: Vec<Read> = Vec::new();
        // The enum whose entries are read, and whether the message
        // whose arguments are read is an event.
        let (mut within, mut event) = (String::new(), false);
        loop {
            let part = match reader.read_event().expect("XML that cannot be read") {
                Event::Start(part) | Event::Empty(part) => part,
                Event::Eof => return interfaces,
                _ => continue,
            };
            let named = || attribute(&part, "name").expect("a part with no name");
            let number = |name: &str| {
                let value = attribute(&part, name).expect("a number that is not there");
                value.parse::<u32>().expect("a number that cannot be read")
            };
            let message = || Message {
                name: named(),
                destructor: attribute(&part, "type").is_some_and(|kind| kind == "destructor"),
                args: Vec::new(),
            };
            let kind = String::from_utf8_lossy(part.name().as_ref()).into_owned();
            if kind == "interface" {
                let interface =
                    Read { name: named(), version: number("version"), ..Read::default() };
                interfaces.push(interface);
                continue;
            }
            let Some(interface) = interfaces.last_mut() else { continue };
            match kind.as_str() {
                "request" => {
                    interface.requests.push(message());
                    event = false;
                }
                "event" => {
                    interface.events.push(message());
                    event = true;
                }
                "enum" => within = named(),
                "entry" => {
                    interface.entries.push((format!("{within}.{}", named()), number("value")));
                }
                "arg" => {
                    assert_eq!(attribute(&part, "allow-null"), None, "no table has one");
                    let messages =
                        if event { &mut interface.events } else { &mut interface.requests };
                    let arg = (attribute(&part, "type").unwrap(), attribute(&part, "interface"));
                    messages.last_mut().unwrap().args.push(arg);
                }
                _ => {}
            }
        }
    }

    /// A table's messages as the XML would have them.
    fn written(messages: &[MessageDesc]) -> Vec<Message> {
        let of = |message: &MessageDesc| {
            let mut named = message.arg_interfaces.iter().map(|interface| interface.name);
            let arg = |kind: &ArgumentType| match kind {
                ArgumentType::Int => ("int".to_owned(), None),
                ArgumentType::Uint => ("uint".to_owned(), None),
                ArgumentType::NewId => {
                    let made = message.child_interface.expect("a new object of no interface");
                    ("new_id".to_owned(), Some(made.name.to_owned()))
                }
                ArgumentType::Object(AllowNull::No) => {
                    ("object".to_owned(), named.next().map(str::to_owned))
                }
                other => panic!("the tests do not read {other:?} yet"),
            };
            assert_eq!(message.since, 1, "the XML has no later message");
            Message {
                name: message.name.to_owned(),
                destructor: message.is_destructor,
                args: message.signature.iter().map(arg).collect(),
            }
        };
        messages.iter().map(of).collect()
    }

    #[test]
    fn the_tables_are_the_xml() {
        let entries = |entries: &[(&str, u32)]| -> Vec<(String, u32)> {
            entries.iter().map(|(name, value)| ((*name).to_owned(), *value)).collect()
        };
        let tables = [
            (&MANAGER, entries(&[("error.already_exists", manager::ALREADY_EXISTS)])),
            (
                &TITLE_BAR,
                entries(&[("mode.full", Mode::Full as u32), ("mode.shared", Mode::Shared as u32)]),
            ),
        ];
        let written: Vec<Read> = tables
            .into_iter()
            .map(|(table, entries)| Read {
                name: table.name.to_owned(),
                version: table.version,
                requests: written(table.requests),
                events: written(table.events),
                entries,
            })
            .collect();
        assert_eq!(written, read());
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
