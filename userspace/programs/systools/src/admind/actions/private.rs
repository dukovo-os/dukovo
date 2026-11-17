// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The private access actions (ADR 0562): the WireGuard interface `wg0`
//! that lets a phone or a laptop away from home reach the machine, and
//! the devices that have a key for it. The area is `private`, the scope a
//! token needs.
//!
//! All four run `wireguard` as an administrator, as `elevate wireguard
//! ...` does at a terminal: the interface's file holds the server's
//! private key, and wgd's socket is the system's. `private.show` is a
//! read that needs root for that reason (`wireguard show --json` prints
//! public keys and addresses only), as `security.blocked` is.
//!
//! The values are checked as the command checks them
//! ([`crate::private_access`]'s readers), so that the page, the API and
//! the terminal refuse the same values with the same words. A device's
//! name is 1 to 32 letters, digits, `-` and `_`, and does not start with
//! `-`, as a backup place's is.
//!
//! `private.add` makes a device's key and prints the device's file, with
//! its private key in it, once: the server keeps no copy of it. It says
//! so ([`Action::SECRET_ANSWER`]): its route is a `POST`, it is never a
//! task, and nothing of what it prints is kept (ADR 0561). A device
//! reaches the server's address on the tunnel, and so the console and the
//! machine's other services there; Dukovo does not route between its
//! network cards, so a device does not reach the other machines of the
//! server's networks.
//!
//! The file is `/system/state/wireguard/wg0.conf`, mode 0600, which
//! admind's service does not see: the actions name no file, as the journal
//! would find it absent before a change and after it, and so each says it
//! changed something when it ended well. `private.remove` of the
//! device a browser comes through would cut that browser off, and no trial
//! puts a key back: a request that came through `wg0` reads the devices
//! with `wireguard show --json` first, and is refused (ADM-137) when the
//! device to remove holds the browser's address, or when what was read
//! does not say.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::trial::refusal;
use super::{
    Action, Args, Bad, Browser, CutsOff, Examples, Field, Nothing, Runs, Tests, Through, Undo,
};
use crate::private_access::{
    INTERFACE, NAME_MAX, parse_address, parse_host, parse_name, parse_port,
};
use crate::wireguard::Prefix;

/// The area of these actions: the scope a token needs, `private`.
const AREA: &str = "private";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::private::tests",
    route: "admin-security",
    same_as_command: "admin-security",
    browser: "web-console/security",
    ansible: "web-console/ansible",
};

/// What a device's name matches: letters, digits, `-` and `_`, and not
/// starting with `-`.
const NAME_PATTERN: &str = "^[A-Za-z0-9_][A-Za-z0-9_-]*$";

/// What a host the devices reach the server by is made of: the characters
/// of a name or of an address. The command's own check is the exact one.
const HOST_PATTERN: &str = "^[A-Za-z0-9.:-]+$";

/// The most bytes of a host's name.
const HOST_MAX: usize = 253;

/// What the server's address on the tunnel matches: an address and its
/// length.
const ADDRESS_PATTERN: &str = "^[0-9A-Fa-f:.]+/[0-9]+$";

/// The most characters of an address with its length: an IPv6 one.
const ADDRESS_MAX: usize = 49;

/// `wireguard show --json`: the interface, and the devices it has, with
/// what wgd says of each.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "private.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/private";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show private access";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["wireguard", "show", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "json": true }), json!({ "name": "lab-phone" })],
            undo: None,
        }
    }
}

/// What the server is set up with: what is given, and the defaults for the
/// rest on a first run.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Serving {
    /// What the devices are told to reach: a name, or an address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The UDP port, 1024 to 65535.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    /// The server's address on the tunnel, with its length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

impl Args for Serving {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "endpoint",
            "What the devices are told to reach: a host name, or an address. The machine's name \
             if left out.",
            1,
            HOST_MAX,
            Some(HOST_PATTERN),
        )
        .optional(),
        Field::number(
            "port",
            "The UDP port the server listens on, 1024 to 65535. 51820 if left out.",
            1024,
            65535,
        )
        .optional(),
        Field::text(
            "address",
            "The server's address on the tunnel, with its length, as 10.8.0.1/24: IPv4 with a \
             length of 16 to 30, or IPv6 of 48 to 120. 10.8.0.1/24 if left out.",
            1,
            ADDRESS_MAX,
            Some(ADDRESS_PATTERN),
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        let refused = |field: &'static str, why: String| Bad { field, why: refusal(&why) };
        if let Some(endpoint) = &self.endpoint {
            parse_host(endpoint).map_err(|why| refused("endpoint", why))?;
        }
        if let Some(port) = self.port {
            parse_port(&port.to_string()).map_err(|why| refused("port", why))?;
        }
        if let Some(address) = &self.address {
            parse_address(address).map_err(|why| refused("address", why))?;
        }
        Ok(())
    }
}

/// `wireguard serve [--endpoint HOST] [--port PORT] [--address ADDRESS]`:
/// sets the server up, or changes only what is given, and has wgd read
/// the file.
pub struct Serve;

impl Action for Serve {
    const ID: &'static str = "private.serve";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/private/serve";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set up private access";
    type Args = Serving;

    fn argv(args: &Serving) -> Vec<String> {
        let mut argv = vec!["wireguard".to_string(), "serve".into()];
        if let Some(endpoint) = &args.endpoint {
            argv.extend(["--endpoint".to_string(), endpoint.clone()]);
        }
        if let Some(port) = args.port {
            argv.extend(["--port".to_string(), port.to_string()]);
        }
        if let Some(address) = &args.address {
            argv.extend(["--address".to_string(), address.clone()]);
        }
        argv
    }

    /// There is no undo, as there is no first state to go back to: the
    /// interface did not exist.
    fn examples() -> Examples<Serving> {
        Examples {
            good: vec![Serving { endpoint: Some("lab.example.org".into()), ..Serving::default() }],
            bad: vec![
                json!({ "port": 80 }),
                json!({ "port": 1023 }),
                json!({ "port": 65536 }),
                json!({ "port": "51820" }),
                json!({ "endpoint": "not a host" }),
                json!({ "endpoint": "-lab.example.org" }),
                json!({ "address": "10.8.0.1" }),
                json!({ "address": "10.8.0.1/31" }),
                json!({ "address": "10.8.0.0/24" }),
                json!({ "address": "10.8.0.255/24" }),
                json!({ "endpoint": "lab.example.org", "name": "x" }),
            ],
            undo: None,
        }
    }
}

/// The field of a device's name, for the actions that take one.
const NAME: Field = Field::text(
    "name",
    "The device's name: 1 to 32 letters, digits, - and _, not starting with -.",
    1,
    NAME_MAX,
    Some(NAME_PATTERN),
);

/// The refusal of a name that is no device's.
fn check_name(name: &str) -> Result<(), Bad> {
    parse_name(name).map(drop).map_err(|why| Bad { field: "name", why: refusal(&why) })
}

/// A device, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    /// What the person knows it by: 1 to 32 letters, digits, `-` and `_`,
    /// as `private.show` gives it.
    pub name: String,
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[NAME];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)
    }
}

/// `wireguard add NAME --json`: adds a device, makes its key and prints
/// the device's file, once.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "private.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/private/devices/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-184", "NET-185"];
    const SECRET_ANSWER: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add a device";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["wireguard", "add", args.name.as_str(), "--json"].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-phone".into() }],
            bad: vec![
                json!({ "name": "a b" }),
                json!({ "name": "-x" }),
                json!({ "name": "" }),
                json!({ "name": "lab.phone" }),
                json!({}),
                json!({ "name": "lab-phone", "reach": "console" }),
                json!({ "name": "lab-phone", "key": "x" }),
            ],
            undo: Some(Undo { action: "private.remove", args: json!({ "name": "lab-phone" }) }),
        }
    }
}

/// What `wireguard show --json` prints that [`cuts_off`] reads: each
/// interface's devices and their addresses.
#[derive(Deserialize)]
struct Shown {
    interfaces: Vec<ShownInterface>,
}

#[derive(Deserialize)]
struct ShownInterface {
    name: String,
    devices: Vec<ShownDevice>,
}

#[derive(Deserialize)]
struct ShownDevice {
    name: String,
    /// Its `AllowedIPs`, as `10.8.0.2/32`.
    addresses: Vec<String>,
}

/// Whether removing the device `args.name` leaves a browser at `from`,
/// which came through `wg0`, reaching the console: not when the device
/// holds `from`, and not when `shown`, what `wireguard show --json`
/// printed, does not read.
fn cuts_off(args: &Named, browser: &Browser<'_>, shown: &str) -> Result<(), String> {
    let unread = || {
        format!(
            "this request came through {INTERFACE}, and its devices could not be read: remove \
             {} from another connection",
            args.name
        )
    };
    let shown: Shown = serde_json::from_str(shown).map_err(|_| unread())?;
    let interface = shown.interfaces.iter().find(|one| one.name == INTERFACE);
    let devices = interface.map_or(&[][..], |one| one.devices.as_slice());
    for device in devices.iter().filter(|device| device.name == args.name) {
        for address in &device.addresses {
            let Some(held) = Prefix::parse(address) else { return Err(unread()) };
            if held.contains(browser.from) {
                return Err(format!(
                    "this browser reaches the console through {}: remove it from another \
                     connection",
                    args.name
                ));
            }
        }
    }
    Ok(())
}

/// `wireguard remove NAME`: removes a device, so that its key no longer
/// reaches the machine.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "private.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/private/devices/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-184"];
    const CUTS_OFF: Option<CutsOff<Named>> = Some(CutsOff {
        on: Through::Card(INTERFACE),
        read: |_| ["wireguard", "show", "--json"].map(String::from).to_vec(),
        cuts: cuts_off,
    });
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove a device";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["wireguard", "remove", args.name.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    /// There is no undo: the device's key is gone from the machine, and
    /// the device's own file does not hold the means to add it again.
    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-phone".into() }],
            bad: vec![
                json!({}),
                json!({ "name": "" }),
                json!({ "name": "a b" }),
                json!({ "name": "-x" }),
                json!({ "name": "../wg0" }),
                json!({ "name": "lab-phone", "reach": "console" }),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, REGISTRY, Registered, authorize};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The failure of `args` for `id`: its kind, its field and its words.
    fn refused(id: &str, args: serde_json::Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    #[test]
    fn all_four_run_as_an_administrator_and_the_changes_are_posts_with_the_interface_s_file() {
        for (id, changes, path, title) in [
            ("private.show", false, "/api/1/private", "Show private access"),
            ("private.serve", true, "/api/1/private/serve", "Set up private access"),
            ("private.add", true, "/api/1/private/devices/add", "Add a device"),
            ("private.remove", true, "/api/1/private/devices/remove", "Remove a device"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.long()),
                (changes, Runs::AsAdministrator, "private", false),
                "{id}"
            );
            assert_eq!((action.path(), action.title(), action.posts()), (path, title, changes));
            assert!(!action.compares(), "{id}: admind does not see the file");
        }
        for (id, args) in [
            ("private.serve", json!({})),
            ("private.add", json!({ "name": "lab-phone" })),
            ("private.remove", json!({ "name": "lab-phone" })),
        ] {
            let prepared = action(id).prepare(args).unwrap();
            assert!(prepared.files.is_empty(), "{id}: admind does not see the file");
            assert_eq!(prepared.probe, None, "{id}: the file holds a key, and show is root's");
            assert_eq!(prepared.trial, None, "{id}");
            assert!(prepared.ansible.become_root, "{id}");
        }
    }

    #[test]
    fn show_reads_the_interface_as_json_and_takes_nothing() {
        let action = action("private.show");
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["wireguard", "show", "--json"]);
        assert_eq!((prepared.probe, prepared.files.len()), (None, 0));
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        assert!(action.errors().is_empty() && action.fields().is_empty());
        let (good, bad, undo) = action.example_values();
        assert_eq!((good, undo), (vec![json!({})], None));
        for args in bad {
            assert_eq!(action.prepare(args).unwrap_err().kind, Kind::BadRequest);
        }
    }

    #[test]
    fn serve_gives_the_command_what_is_given_and_nothing_else() {
        let serve = |args: serde_json::Value| action("private.serve").prepare(args).unwrap().argv;
        assert_eq!(serve(json!({})), ["wireguard", "serve"]);
        assert_eq!(
            serve(json!({ "endpoint": "lab.example.org" })),
            ["wireguard", "serve", "--endpoint", "lab.example.org"]
        );
        assert_eq!(
            serve(json!({ "address": "10.9.0.1/24", "port": 51821, "endpoint": "203.0.113.5" })),
            [
                "wireguard",
                "serve",
                "--endpoint",
                "203.0.113.5",
                "--port",
                "51821",
                "--address",
                "10.9.0.1/24"
            ]
        );
        for port in [1024, 51820, 65535] {
            assert!(action("private.serve").prepare(json!({ "port": port })).is_ok(), "{port}");
        }
        for address in ["10.8.0.1/24", "172.16.0.9/16", "10.0.0.5/30", "fd00:1::1/64", "fd00::1/48"]
        {
            assert!(
                action("private.serve").prepare(json!({ "address": address })).is_ok(),
                "{address}"
            );
        }
        for host in ["lab", "lab.example.org", "203.0.113.5", "2001:db8::1", "a-b.c"] {
            assert!(action("private.serve").prepare(json!({ "endpoint": host })).is_ok(), "{host}");
        }
        let prepared = action("private.serve").prepare(json!({ "port": 51821 })).unwrap();
        assert_eq!(prepared.args, json!({ "port": 51821 }));
        assert_eq!(action("private.serve").example_values().2, None);
    }

    #[test]
    fn serve_refuses_what_the_command_would_with_the_field_named() {
        for (args, field, why) in [
            (json!({ "port": 80 }), "port", "80 is not a port: it is a number from 1024 to 65535"),
            (json!({ "port": 1023 }), "port", "1023 is not a port"),
            (json!({ "port": 65536 }), "port", "65536 is not a port"),
            (json!({ "port": -1 }), "port", "-1 is not a port"),
            (
                json!({ "endpoint": "not a host" }),
                "endpoint",
                "not a host is not a host's name or an address",
            ),
            (json!({ "endpoint": "-lab" }), "endpoint", "is not a host's name or an address"),
            (json!({ "endpoint": "lab..org" }), "endpoint", "is not a host's name or an address"),
            (json!({ "address": "10.8.0.1" }), "address", "give its length too"),
            (json!({ "address": "10.8.0.1/31" }), "address", "its length is 16 to 30 for IPv4"),
            (json!({ "address": "10.8.0.0/24" }), "address", "is not an address for the tunnel"),
            (json!({ "address": "10.8.0.255/24" }), "address", "is not an address for the tunnel"),
            (json!({ "address": "fd00::1/64x" }), "address", "is not an address for the tunnel"),
        ] {
            let (kind, got, words) = refused("private.serve", args.clone());
            assert_eq!((kind, got), (Kind::BadValue, Some(field)), "{args}");
            // A refusal starts with a capital.
            assert!(words.to_lowercase().contains(&why.to_lowercase()), "{args}: {words}");
            assert!(words.ends_with(". Nothing was done."), "{words}");
        }
        let long = "a".repeat(64);
        let host = format!("{long}.example.org");
        assert_eq!(refused("private.serve", json!({ "endpoint": host })).1, Some("endpoint"));
        for args in [
            json!({ "port": "51820" }),
            json!({ "port": 1.5 }),
            json!({ "endpoint": 7 }),
            json!({ "address": ["10.8.0.1/24"] }),
            json!({ "endpoint": "lab", "name": "x" }),
        ] {
            assert_eq!(
                action("private.serve").prepare(args.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{args}"
            );
        }
    }

    #[test]
    fn add_prints_a_secret_so_it_is_a_post_and_adds_a_device_that_remove_takes_out() {
        let action = action("private.add");
        assert!(action.secret_answer() && action.posts() && !action.long());
        assert_eq!(action.errors(), ["NET-184", "NET-185"]);
        assert_eq!(action.fields().len(), 1);
        let prepared = action.prepare(json!({ "name": "lab-phone" })).unwrap();
        assert_eq!(prepared.argv, ["wireguard", "add", "lab-phone", "--json"]);
        assert_eq!(prepared.ansible.name, "Add a device: lab-phone");
        assert!(prepared.ansible.no_log, "what it prints is a secret");
        assert!(prepared.cuts_off.is_none(), "adding a device takes nothing away");
        let (good, bad, undo) = action.example_values();
        assert_eq!(good, [json!({ "name": "lab-phone" })]);
        assert_eq!(
            undo,
            Some(Undo { action: "private.remove", args: json!({ "name": "lab-phone" }) })
        );
        for args in bad {
            assert!(action.prepare(args.clone()).is_err(), "{args}");
        }
        let longest = "a".repeat(NAME_MAX);
        for name in ["a", "lab-phone", "Lab_2", "_x", longest.as_str()] {
            assert!(action.prepare(json!({ "name": name })).is_ok(), "{name}");
        }
        let long = "a".repeat(NAME_MAX + 1);
        for name in
            ["", "a b", "-x", "lab.phone", "a/b", "../x", "caf\u{e9}", "a\nb", long.as_str()]
        {
            let (kind, field, words) = refused("private.add", json!({ "name": name }));
            assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{name:?}");
            assert_eq!(
                words,
                "A device's name is 1 to 32 letters, digits, - and _, and does not start with -. \
                 Nothing was done."
            );
        }
        // A device reaches the server's address on the tunnel: there is no
        // choice of what it reaches.
        let refused = action.prepare(json!({ "name": "lab-phone", "reach": "network" }));
        assert_eq!(refused.unwrap_err().kind, Kind::BadRequest);
    }

    #[test]
    fn remove_names_the_device_and_has_no_undo() {
        let action = action("private.remove");
        assert_eq!(action.errors(), ["NET-184"]);
        let prepared = action.prepare(json!({ "name": "lab-phone" })).unwrap();
        assert_eq!(prepared.argv, ["wireguard", "remove", "lab-phone"]);
        assert_eq!(prepared.ansible.name, "Remove a device: lab-phone");
        assert!(!prepared.ansible.no_log);
        let read = prepared.cuts_off.unwrap();
        assert_eq!(read.on, Through::Card("wg0"));
        assert_eq!(read.argv, ["wireguard", "show", "--json"]);
        let (good, bad, undo) = action.example_values();
        assert_eq!((good, undo), (vec![json!({ "name": "lab-phone" })], None));
        for args in bad {
            assert!(action.prepare(args.clone()).is_err(), "{args}");
        }
        for name in ["", "a b", "-x", "..", "a/b"] {
            assert_eq!(
                refused("private.remove", json!({ "name": name })).1,
                Some("name"),
                "{name:?}"
            );
        }
        // The pattern of the schema and the rule are one.
        assert_eq!(NAME_PATTERN, "^[A-Za-z0-9_][A-Za-z0-9_-]*$");
        assert_eq!(NAME_MAX, 32);
    }

    #[test]
    fn a_token_needs_the_full_scope_even_to_read_since_the_command_is_root_s() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["private.show", "private.serve", "private.add", "private.remove"] {
            for list in ["private", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["private:read", "network", "security"] {
                let refused = call(list, id).unwrap_err();
                assert_eq!(refused.kind, Kind::OutOfScope, "{id} {list}");
                assert_eq!(
                    refused.detail,
                    "the API token \"ci\" may not do this: it needs the scope private or *"
                );
            }
        }
    }

    /// What `wireguard show --json` prints for wg0 with `devices`, each a
    /// name and its addresses.
    fn shown(devices: &[(&str, &[&str])]) -> String {
        let devices: Vec<_> = devices
            .iter()
            .map(|(name, addresses)| {
                json!({ "name": name, "public_key": "k", "addresses": addresses,
                        "endpoint": null, "latest_handshake_ms": null, "received": null, "sent": null })
            })
            .collect();
        json!({ "format": 1, "interfaces": [{ "name": "wg0", "address": "10.8.0.1/24", "port": 51820,
                "endpoint": "home.example.org", "public_key": "k", "up": true, "devices": devices }] })
        .to_string()
    }

    /// Whether removing `name` leaves a browser at `from` reaching the
    /// console, as `private.remove`'s guard says.
    fn guard(name: &str, from: &str, printed: &str) -> Result<(), String> {
        let browser = Browser { from: from.parse().unwrap(), host: "10.8.0.1" };
        action("private.remove").cuts_off(&json!({ "name": name }), &browser, printed)
    }

    #[test]
    fn removing_the_device_this_browser_comes_through_is_refused() {
        let printed =
            shown(&[("phone", &["10.8.0.2/32"]), ("laptop", &["10.8.0.3/32", "fd00:8::3/128"])]);
        assert_eq!(
            guard("phone", "10.8.0.2", &printed).unwrap_err(),
            "this browser reaches the console through phone: remove it from another connection"
        );
        assert!(guard("laptop", "fd00:8::3", &printed).is_err());
        // A device that holds a network holds the browser in it.
        let printed = shown(&[("office", &["10.8.0.8/29"])]);
        assert!(guard("office", "10.8.0.13", &printed).is_err());
    }

    #[test]
    fn removing_another_device_or_one_that_is_not_there_is_let_through() {
        let printed = shown(&[("phone", &["10.8.0.2/32"]), ("laptop", &["10.8.0.3/32"])]);
        assert_eq!(guard("laptop", "10.8.0.2", &printed), Ok(()));
        // The command says that no such device exists, as it does from any
        // other connection.
        assert_eq!(guard("tablet", "10.8.0.2", &printed), Ok(()));
        assert_eq!(guard("phone", "10.8.0.20", &printed), Ok(()));
        // Two devices of one name: the one the browser comes through counts.
        let printed = shown(&[("phone", &["10.8.0.2/32"]), ("phone", &["10.8.0.4/32"])]);
        assert!(guard("phone", "10.8.0.4", &printed).is_err());
        // Set up, with no device: nothing to cut off.
        assert_eq!(guard("phone", "10.8.0.2", &shown(&[])), Ok(()));
        let none = json!({ "format": 1, "interfaces": [] }).to_string();
        assert_eq!(guard("phone", "10.8.0.2", &none), Ok(()));
    }

    #[test]
    fn a_read_that_does_not_say_refuses_the_removal() {
        let unread = "this request came through wg0, and its devices could not be read: remove \
                      phone from another connection";
        for printed in [
            String::new(),
            "not json".to_string(),
            json!({ "format": 1 }).to_string(),
            json!({ "interfaces": [{ "name": "wg0" }] }).to_string(),
            json!({ "interfaces": [{ "name": "wg0", "devices": [{ "name": "phone" }] }] })
                .to_string(),
            shown(&[("phone", &["10.8.0.2"])]).replace("10.8.0.2", "nonsense"),
        ] {
            assert_eq!(guard("phone", "10.8.0.2", &printed).unwrap_err(), unread, "{printed}");
        }
    }
}
