// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A protocol as its XML has it, for the tests that hold the tables to
//! the XML.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use wayland_backend::protocol::{AllowNull, ArgumentType, Interface, MessageDesc};

/// A request or an event as the XML has it.
#[derive(Debug, Default, PartialEq)]
pub struct Message {
    pub name: String,
    pub destructor: bool,
    /// Each argument's type, and the interface it names.
    pub args: Vec<(String, Option<String>)>,
}

/// An interface as the XML has it.
#[derive(Debug, Default, PartialEq)]
pub struct Read {
    pub name: String,
    pub version: u32,
    pub requests: Vec<Message>,
    pub events: Vec<Message>,
    /// Every entry of every enum, as `enum.entry`, with its value.
    pub entries: Vec<(String, u32)>,
}

fn attribute(of: &BytesStart<'_>, name: &str) -> Option<String> {
    let found = of.try_get_attribute(name).expect("an attribute that cannot be read")?;
    let value = found.normalized_value(XmlVersion::Explicit1_0);
    Some(value.expect("an attribute's value that cannot be read").into_owned())
}

/// The interfaces of the protocol `xml` describes.
pub fn read(xml: &str) -> Vec<Read> {
    let mut reader = Reader::from_str(xml);
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
            let interface = Read { name: named(), version: number("version"), ..Read::default() };
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
                let messages = if event { &mut interface.events } else { &mut interface.requests };
                let arg = (attribute(&part, "type").unwrap(), attribute(&part, "interface"));
                messages.last_mut().unwrap().args.push(arg);
            }
            _ => {}
        }
    }
}

/// A table's messages as the XML would have them.
fn messages(messages: &[MessageDesc]) -> Vec<Message> {
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
            ArgumentType::Str(AllowNull::No) => ("string".to_owned(), None),
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

/// The table `table` as the XML would have its interface, with the
/// `entries` of its enums, each as `enum.entry` with its value.
pub fn written(table: &Interface, entries: &[(&str, u32)]) -> Read {
    Read {
        name: table.name.to_owned(),
        version: table.version,
        requests: messages(table.requests),
        events: messages(table.events),
        entries: entries.iter().map(|(name, value)| ((*name).to_owned(), *value)).collect(),
    }
}
