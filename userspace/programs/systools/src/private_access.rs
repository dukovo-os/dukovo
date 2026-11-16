// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! wireguard(8): private access to this machine, set up and kept in the
//! words a person uses, over the interface `wg0` that wgd(8) carries.
//!
//! `wireguard show` says what the interface is and which devices it has,
//! as a list or as JSON, which the web console's Security page reads.
//! `wireguard serve` makes the server: its key, and `wg0.conf` with its
//! address, port and the name devices are told to reach, then has wgd
//! read it; run again it changes only what it is given. `wireguard add`
//! makes a device's keys, gives it the next free address of the tunnel,
//! and prints the device's file once: the device's private key is written
//! nowhere on the machine. `wireguard remove` takes a device away. All of
//! it is root's, as the file holds a private key and wgd's socket is the
//! system's alone. wg(8) stays for everything else.
//!
//! A device reaches the server's address on the tunnel, and so every
//! service of the machine that answers there. It does not reach the other
//! machines of the server's networks: Dukovo does not route between its
//! network cards.
//!
//! The parts are [`show`], the report; and [`file`], the text of the file
//! with its two remarks (`# endpoint:` and `# name:`), the edits `serve`,
//! `add` and `remove` make of it as functions of the text, and the device's
//! file. Here are the command line and the steps that touch the machine:
//! reading and writing the file, and asking Spark to have wgd read it.
//!
//! The command's module is not `wireguard.rs`, which wg(8) and wgd(8)
//! share (ADR 0340): this builds on its keys, its file reader and its
//! socket. When a device is not where it should be, `wireguard show` says
//! what the file and wgd say of it, and `service status wgd` whether wgd
//! runs.

mod file;
mod show;

use std::fs::{self, DirBuilder};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};
use crate::wireguard::{self, Key, Prefix};
use crate::{net_errors, service, servicefiles, trial};
use file::{Settings, ToDevice};

const USAGE: &str = "Usage: wireguard [show] [--json]\n       \
                     wireguard serve [--endpoint HOST] [--port PORT]\n                       \
                     [--address ADDRESS/LENGTH]\n       \
                     wireguard add NAME [--json]\n       \
                     wireguard remove NAME\n  \
                     --json  the interface and its devices, or the device added, as JSON";

/// The interface private access is made of.
pub(crate) const INTERFACE: &str = "wg0";

/// The service that carries it.
const SERVICE: &str = "wgd";

/// The mode of the interface's file, which holds a private key.
const MODE: u32 = 0o600;

/// The UDP port `serve` gives a server that is not told one, which wgd's
/// service file lets it listen on.
const PORT: u16 = 51820;

/// The server's address on the tunnel, when `serve` is not told one.
const ADDRESS: Prefix = Prefix { ip: IpAddr::V4(Ipv4Addr::new(10, 8, 0, 1)), len: 24 };

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// `show`, with the option.
    Show { json: bool },
    /// `serve` and what it is told.
    Serve(Settings),
    /// `add` of a device with this name, with the option.
    Add { name: String, json: bool },
    /// `remove` of the device with this name.
    Remove(String),
}

/// Whether `label` can be one part of a host's name.
fn label_ok(label: &str) -> bool {
    (1..=63).contains(&label.len())
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// What devices are told to reach: a name of 1 to 253 bytes, or an address.
/// Kept as the plain words it is, since it is written into a remark of the
/// file, and a device's file as the host of its `Endpoint`.
pub(crate) fn parse_host(text: &str) -> Result<String, String> {
    if text.parse::<IpAddr>().is_ok() || (text.len() <= 253 && text.split('.').all(label_ok)) {
        return Ok(text.to_string());
    }
    Err(format!("{text} is not a host's name or an address"))
}

/// The most characters of a device's name.
pub(crate) const NAME_MAX: usize = 32;

/// A device's name, as the console gives one and `add` takes it: 1 to 32
/// letters, digits, `-` and `_`, and not starting with `-`, as a backup
/// place is named. It is a word of one line, which the file keeps as a
/// remark, and which `remove` finds the device by.
pub(crate) fn parse_name(text: &str) -> Result<String, String> {
    let fits = (1..=NAME_MAX).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        && !text.starts_with('-');
    if fits {
        return Ok(text.to_string());
    }
    Err(format!(
        "a device's name is 1 to {NAME_MAX} letters, digits, - and _, and does not start with -"
    ))
}

/// A UDP port devices reach the server on: 1024 to 65535.
pub(crate) fn parse_port(text: &str) -> Result<u16, String> {
    text.parse::<u16>()
        .ok()
        .filter(|port| *port >= 1024)
        .ok_or_else(|| format!("{text} is not a port: it is a number from 1024 to 65535"))
}

/// The server's address on the tunnel, with its length: IPv4 with a length
/// of 16 to 30, or IPv6 with one of 48 to 120, and an address the network
/// can give out, which is not its first or its last.
pub(crate) fn parse_address(text: &str) -> Result<Prefix, String> {
    let bad = |why: &str| format!("{text} is not an address for the tunnel: {why}");
    if !text.contains('/') {
        return Err(bad("give its length too, as 10.8.0.1/24"));
    }
    let prefix = Prefix::parse(text).ok_or_else(|| bad("it is ADDRESS/LENGTH"))?;
    let (lengths, host_bits, bits) = match prefix.ip {
        IpAddr::V4(ip) => (16..=30, 32 - u32::from(prefix.len), u128::from(u32::from(ip))),
        IpAddr::V6(ip) => (48..=120, 128 - u32::from(prefix.len), u128::from(ip)),
    };
    if !lengths.contains(&prefix.len) {
        return Err(bad(&format!(
            "its length is {} to {} for {}",
            lengths.start(),
            lengths.end(),
            if prefix.ip.is_ipv4() { "IPv4" } else { "IPv6" }
        )));
    }
    let all_ones = (1u128 << host_bits) - 1;
    let own = bits & all_ones;
    let broadcast = prefix.ip.is_ipv4() && own == all_ones;
    if own == 0 || broadcast || prefix.ip.is_loopback() || prefix.ip.is_multicast() {
        return Err(bad("it is not one a device can reach"));
    }
    Ok(prefix)
}

/// Reads the command line, which is `Ok(None)` for `--help`.
fn parse(args: &mut Args) -> Result<Option<Command>, Fail> {
    let wrong = |why: &str| usage(format!("{USAGE}\n{why}"));
    let mut words = Vec::new();
    let mut json = false;
    let mut settings = Settings::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "help" => return Ok(None),
            Arg::Long(name) if name == "endpoint" => {
                settings.endpoint = Some(parse_host(&args.text()?).map_err(|why| wrong(&why))?);
            }
            Arg::Long(name) if name == "port" => {
                settings.port = Some(parse_port(&args.text()?).map_err(|why| wrong(&why))?);
            }
            Arg::Long(name) if name == "address" => {
                settings.address = Some(parse_address(&args.text()?).map_err(|why| wrong(&why))?);
            }
            Arg::Operand(word) => words.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let told = settings != Settings::default();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        [] | ["show"] if told => {
            return Err(wrong("--endpoint, --port and --address go with serve"));
        }
        [] | ["show"] => Command::Show { json },
        ["add", name] if !told => {
            Command::Add { name: parse_name(name).map_err(|why| wrong(&why))?, json }
        }
        ["add", _] => return Err(wrong("--endpoint, --port and --address go with serve")),
        ["add", ..] => return Err(wrong("add takes one name, the device's")),
        _ if json => return Err(wrong("--json goes with show and add")),
        ["serve"] => Command::Serve(settings),
        ["remove", name] if !told => Command::Remove((*name).to_string()),
        ["remove", _] => return Err(wrong("--endpoint, --port and --address go with serve")),
        ["remove", ..] => return Err(wrong("remove takes one name, the device's")),
        _ => return Err(wrong(&format!("no such command: {}", words.join(" ")))),
    };
    Ok(Some(command))
}

/// Fails unless this is root, whom the file and wgd's socket are for.
fn require_root(verb: &str) -> Result<(), Fail> {
    if util::own_user() == 0 {
        return Ok(());
    }
    Err(fail(format!(
        "only root may use private access: run it with elevate, as `elevate wireguard {verb}`"
    )))
}

/// Where the interface's file is.
pub(crate) fn path() -> PathBuf {
    Path::new(wireguard::STATE).join(format!("{INTERFACE}.conf"))
}

/// The text of the file at `path`: none when there is no such file.
///
/// # Errors
///
/// When it is there and cannot be read.
fn read_file(path: &Path) -> Result<Option<String>, Fail> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(util::failed(&path.display(), error)),
    }
}

/// The file `text` of `path`, read.
///
/// # Errors
///
/// What is wrong with it, with its path and line.
fn reading(path: &Path, text: &str) -> Result<file::File, Fail> {
    file::read(text).map_err(|why| fail(format!("{}: {why}", path.display())))
}

/// Puts `text` in place of the file at `path`, whole or not at all, with
/// the mode it has, or [`MODE`] for a file that is new, in a folder that
/// only the system opens.
///
/// # Errors
///
/// When the folder or the file cannot be made or written.
fn write(path: &Path, text: &str) -> Result<(), Fail> {
    if let Some(folder) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)
            .map_err(|error| util::failed(&folder.display(), error))?;
    }
    trial::replace(path, text.as_bytes(), MODE)
}

/// Asks Spark to have wgd read its files again, which keeps the sessions of
/// the devices that stay: whether it did, which is false when wgd is not
/// running and so reads them when it starts.
///
/// # Errors
///
/// When Spark cannot be reached or refuses.
fn reload() -> Result<bool, Fail> {
    match service::ask(&format!("reload {SERVICE}")) {
        Ok(_) => Ok(true),
        Err(refused) if refused.message == format!("{SERVICE} is not running") => Ok(false),
        Err(refused) => Err(refused),
    }
}

/// The file `serve` makes for a machine with no private access: `settings`
/// and, where they say nothing, the defaults, with the machine's name to be
/// reached by.
fn made(settings: &Settings, machine: &str, key: &Key) -> Result<String, String> {
    let endpoint = match &settings.endpoint {
        Some(endpoint) => endpoint.clone(),
        None => parse_host(machine).map_err(|_| {
            format!(
                "the machine's name, {machine}, is not one devices can reach it by: give one \
                 with --endpoint"
            )
        })?,
    };
    let port = settings.port.unwrap_or(PORT);
    Ok(file::first_file(key, &endpoint, port, settings.address.unwrap_or(ADDRESS)))
}

/// `wireguard serve`.
fn serve(settings: &Settings) -> Result<i32, Fail> {
    require_root("serve")?;
    let path = path();
    let old = read_file(&path)?;
    let new = match &old {
        Some(text) => {
            file::serve(text, settings).map_err(|why| fail(format!("{}: {why}", path.display())))?
        }
        None => {
            let key = Key::generate().map_err(|error| {
                fail(format!("cannot make the server's key: {}", util::strip_os_error(&error)))
            })?;
            made(settings, &util::hostname(), &key).map_err(fail)?
        }
    };
    if old.as_deref() == Some(new.as_str()) {
        println!("{} says so already, so it is not changed.", path.display());
        return Ok(0);
    }
    let port = reading(&path, &new)?.config.listen_port;
    write(&path, &new)?;
    println!("{} is {}.", path.display(), if old.is_some() { "changed" } else { "made" });
    if reload()? {
        println!("{SERVICE} reads it again, and keeps the devices' sessions.");
    } else {
        servicefiles::enable(SERVICE, true)?;
    }
    if let Some(port) = port {
        println!("Let devices in with `firewall allow udp {port}`.");
        if port != PORT {
            println!(
                "{SERVICE}'s service file lets it listen on UDP port {PORT} only: add {port} to \
                 its network.listen (wireguard.conf(5))."
            );
        }
    }
    Ok(0)
}

/// What `wireguard add --json` prints: the device, its file, which holds
/// its private key, and whether wgd lets it in now.
#[derive(Serialize)]
struct Added<'a> {
    name: &'a str,
    /// Its address on the tunnel, alone, as `10.8.0.2/32`.
    address: String,
    /// The file the device sets itself up with.
    config: &'a str,
    /// Whether wgd read the file again; when not, it reads it when it
    /// starts or is reloaded.
    live: bool,
}

/// `wireguard add NAME`: a device with a new key pair and the next free
/// address of the tunnel, added to the file and to wgd, and its file
/// printed once. Its private key is written nowhere on the machine.
fn add(name: &str, json: bool) -> Result<i32, Fail> {
    require_root("add")?;
    let path = path();
    let Some(text) = read_file(&path)? else { return Err(net_errors::no_private_access()) };
    let file = reading(&path, &text)?;
    if !file.called(name).is_empty() {
        return Err(net_errors::device_exists(name));
    }
    // What the device's file needs, each found before anything changes.
    let missing = |what: &str, how: &str| fail(format!("{} has no {what}: {how}", path.display()));
    let server = file.address().ok_or_else(|| {
        missing("Address", "set one with `wireguard serve --address ADDRESS/LENGTH`")
    })?;
    let endpoint = file
        .endpoint
        .clone()
        .ok_or_else(|| missing("# endpoint:", "set one with `wireguard serve --endpoint HOST`"))?;
    let port = file
        .config
        .listen_port
        .ok_or_else(|| missing("ListenPort", "set one with `wireguard serve --port PORT`"))?;
    let server_key = file.config.private_key.as_ref().map(Key::public).ok_or_else(|| {
        missing(
            "PrivateKey",
            "add one with `wg genkey`, or move the file away and run `wireguard serve`",
        )
    })?;
    let address = file::free_address(&file, server).ok_or_else(|| {
        fail(format!(
            "the tunnel's network, {}, has no address left for another device: remove one, \
             or give the tunnel a larger network with `wireguard serve --address`",
            server.network()
        ))
    })?;
    let key = Key::generate().map_err(|error| {
        fail(format!("cannot make the device's key: {}", util::strip_os_error(&error)))
    })?;
    let new = file::with_device(&text, name, &key.public(), address);
    // The new text reads, with the device in it once, before it is written.
    if reading(&path, &new)?.called(name).len() != 1 {
        return Err(fail(format!("{}: {name} could not be added to the file", path.display())));
    }
    write(&path, &new)?;
    let config = file::device_file(&ToDevice {
        private_key: &key,
        address,
        server_key: &server_key,
        endpoint: &endpoint,
        port,
        server_address: server.ip,
    });
    // The device is in the file now, and its private key is only here: a
    // reload that fails is said, and the file is printed all the same.
    let reloaded = reload();
    if let Err(refused) = &reloaded {
        eprintln!(
            "wireguard: {}: wgd reads the file when it starts, or at `service {SERVICE} reload`",
            refused.message
        );
    }
    let live = matches!(reloaded, Ok(true));
    if json {
        let added = Added { name, address: file::host(address).to_string(), config: &config, live };
        let line = serde_json::to_string(&added).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
        return Ok(0);
    }
    println!("{name} is added to {} with the address {address}.", path.display());
    match reloaded {
        Ok(true) => println!("{SERVICE} reads the file again, and lets {name} in."),
        Ok(false) => println!("{SERVICE} is not running, so it reads the file when it starts."),
        Err(_) => {}
    }
    println!(
        "\n{name}'s file follows. It holds {name}'s private key, which is kept nowhere on this \
         machine: put it in the WireGuard app on {name}, then delete it.\n"
    );
    print!("{config}");
    Ok(0)
}

/// `wireguard remove`.
fn remove(name: &str) -> Result<i32, Fail> {
    require_root("remove")?;
    let path = path();
    let Some(text) = read_file(&path)? else { return Err(net_errors::no_private_access()) };
    let file = reading(&path, &text)?;
    let device = match file.called(name).as_slice() {
        [] => return Err(net_errors::no_such_device(name)),
        [device] => *device,
        more => {
            return Err(fail(format!(
                "{} devices are called {name}: remove the ones that go from {} by hand",
                more.len(),
                path.display()
            )));
        }
    };
    write(&path, &file::without(&text, &device.lines))?;
    println!("{name} is removed from {}: its key no longer reaches this machine.", path.display());
    if reload()? {
        println!("{SERVICE} reads the file again, and closes its tunnel.");
    } else {
        println!("{SERVICE} is not running, so it reads the file when it starts.");
    }
    Ok(0)
}

/// wireguard(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    match parse(args)? {
        None => {
            println!("{USAGE}");
            Ok(0)
        }
        Some(Command::Show { json }) => {
            require_root("show")?;
            show::run(json)
        }
        Some(Command::Serve(settings)) => serve(&settings),
        Some(Command::Add { name, json }) => add(&name, json),
        Some(Command::Remove(name)) => remove(&name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(items: &[&str]) -> Result<Option<Command>, Fail> {
        parse(&mut Args::new("wireguard", items.iter().map(Into::into).collect()))
    }

    fn told(endpoint: Option<&str>, port: Option<u16>, address: Option<&str>) -> Settings {
        Settings {
            endpoint: endpoint.map(str::to_string),
            port,
            address: address.map(|text| Prefix::parse(text).unwrap()),
        }
    }

    #[test]
    fn the_words_and_the_options_are_read_wherever_the_options_come() {
        assert_eq!(read(&[]).unwrap(), Some(Command::Show { json: false }));
        assert_eq!(read(&["show"]).unwrap(), Some(Command::Show { json: false }));
        assert_eq!(read(&["--json"]).unwrap(), Some(Command::Show { json: true }));
        assert_eq!(read(&["show", "--json"]).unwrap(), Some(Command::Show { json: true }));
        assert_eq!(read(&["--json", "show"]).unwrap(), Some(Command::Show { json: true }));
        assert_eq!(read(&["--help"]).unwrap(), None);
        assert_eq!(read(&["serve"]).unwrap(), Some(Command::Serve(Settings::default())));
        assert_eq!(
            read(&["serve", "--port", "40000", "--endpoint", "vpn.example.net"]).unwrap(),
            Some(Command::Serve(told(Some("vpn.example.net"), Some(40000), None)))
        );
        assert_eq!(
            read(&["--address", "10.9.0.1/24", "serve"]).unwrap(),
            Some(Command::Serve(told(None, None, Some("10.9.0.1/24"))))
        );
        assert_eq!(read(&["remove", "phone"]).unwrap(), Some(Command::Remove("phone".to_string())));
        let add = |json| Some(Command::Add { name: "phone".to_string(), json });
        assert_eq!(read(&["add", "phone"]).unwrap(), add(false));
        assert_eq!(read(&["add", "phone", "--json"]).unwrap(), add(true));
        assert_eq!(read(&["--json", "add", "phone"]).unwrap(), add(true));
    }

    #[test]
    fn an_option_or_a_command_it_does_not_have_is_a_mistake() {
        for items in [
            &["--all"][..],
            &["-x"],
            &["--json=yes"],
            &["--try", "30"],
            &["add"],
            &["add", "a", "b"],
            &["add", "-x"],
            &["add", "a b"],
            &["add", "phone", "--port", "40000"],
            &["keep"],
            &["restart"],
            &["serve", "now"],
            &["serve", "--json"],
            &["serve", "--port"],
            &["remove"],
            &["remove", "a", "b"],
            &["remove", "phone", "--json"],
            &["remove", "phone", "--port", "40000"],
            &["show", "--port", "40000"],
            &["--endpoint", "home.example.org"],
            &["show", "now"],
        ] {
            assert!(read(items).unwrap_err().usage, "{items:?}");
        }
    }

    /// A name of labels with `lengths` characters.
    fn name_of(lengths: &[usize]) -> String {
        lengths.iter().map(|length| "a".repeat(*length)).collect::<Vec<_>>().join(".")
    }

    #[test]
    fn a_device_is_named_as_a_backup_place_is() {
        for name in ["phone", "lab-phone", "a", "_x", "Lab_2", &"a".repeat(NAME_MAX)] {
            assert_eq!(parse_name(name).as_deref(), Ok(name), "{name}");
        }
        for name in
            ["", "-x", "a b", "a.b", "a/b", "../x", "ph\u{f6}ne", "a\nb", &"a".repeat(NAME_MAX + 1)]
        {
            assert!(parse_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_host_is_a_name_or_an_address_and_nothing_that_could_break_the_file() {
        for bad in ["", "-", "a b", "a..b", "a-.b", "-a.b", "a_b.c", "[::1]", "x/y", "a\nb", "a#b"]
        {
            assert!(parse_host(bad).is_err(), "{bad:?}");
        }
        assert!(parse_host(&name_of(&[64])).is_err());
        assert!(parse_host(&name_of(&[63, 63, 63, 62])).is_err());
        assert_eq!(name_of(&[63, 63, 63, 61]).len(), 253);
        assert!(parse_host(&name_of(&[63, 63, 63, 61])).is_ok());
        for good in ["home", "home.example.org", "10.0.0.1", "2001:db8::1", "a-b.c1", "x"] {
            assert_eq!(parse_host(good).unwrap(), good);
        }
    }

    #[test]
    fn a_port_is_1024_to_65535() {
        for bad in ["80", "1023", "65536", "-1", "x", "", "51820.5"] {
            assert!(parse_port(bad).is_err(), "{bad:?}");
        }
        assert_eq!((parse_port("1024").unwrap(), parse_port("65535").unwrap()), (1024, 65535));
    }

    #[test]
    fn an_address_has_its_length_in_range_and_is_one_a_device_can_reach() {
        for bad in [
            "10.8.0.1",
            "10.8.0.1/15",
            "10.8.0.1/31",
            "10.8.0.0/24",
            "10.8.0.255/24",
            "10.8.0.1/33",
            "127.0.0.1/24",
            "224.0.0.1/24",
            "fd00::1/47",
            "fd00::1/121",
            "fd00::/64",
            "ff02::1/64",
            "::1/64",
            "x/24",
            "10.8.0.1/",
        ] {
            assert!(parse_address(bad).is_err(), "{bad:?}");
        }
        for good in [
            "10.8.0.1/24",
            "10.8.0.1/16",
            "10.8.0.1/30",
            "10.0.1.0/16",
            "10.8.0.254/24",
            "fd00::1/64",
            "fd00::1/48",
            "fd00::1/120",
        ] {
            assert_eq!(parse_address(good).unwrap(), Prefix::parse(good).unwrap(), "{good}");
        }
    }

    #[test]
    fn the_first_file_takes_the_machines_name_and_the_defaults_where_serve_is_told_nothing() {
        let key = Key([7; 32]);
        let text = made(&Settings::default(), "lab-one", &key).unwrap();
        assert_eq!(
            text,
            format!(
                "[Interface]\n# endpoint: lab-one\nAddress = 10.8.0.1/24\nListenPort = 51820\n\
                 PrivateKey = {}\n",
                key.base64()
            )
        );
        let given = told(Some("vpn.example.net"), Some(40000), Some("10.9.0.1/24"));
        let text = made(&given, "lab-one", &key).unwrap();
        assert!(
            text.contains(
                "# endpoint: vpn.example.net\nAddress = 10.9.0.1/24\nListenPort = 40000\n"
            )
        );
        // A name that cannot be reached by is not guessed at.
        let why = made(&Settings::default(), "localhost.", &key).unwrap_err();
        assert!(why.contains("give one with --endpoint"), "{why}");
        assert!(made(&given, "not a name", &key).is_ok());
    }

    /// A folder of its own for each call, as the tests run at once.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Folder {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            let call = CALLS.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("wireguard-{}-{call}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Folder(dir)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn the_file_is_made_in_a_folder_only_the_system_opens_and_replaced_with_the_mode_it_has() {
        let folder = Folder::new();
        let path = folder.0.join("state").join("wg0.conf");
        write(&path, "one\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\n");
        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(mode_of(path.parent().unwrap()), 0o700);
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o640)).unwrap();
        write(&path, "two\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(mode_of(&path), 0o640);
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["wg0.conf"]);
    }

    #[test]
    fn a_file_that_is_not_there_is_none_and_one_that_does_not_read_says_where() {
        let folder = Folder::new();
        fs::create_dir_all(&folder.0).unwrap();
        let path = folder.0.join("wg0.conf");
        assert_eq!(read_file(&path).unwrap(), None);
        let why = reading(&path, "[Interface]\nMTU = x\n").unwrap_err().message;
        assert!(why.starts_with(&format!("{}: line 2: ", path.display())), "{why}");
        // A folder where the file should be is an error, not a missing file.
        fs::create_dir(&path).unwrap();
        assert!(read_file(&path).is_err());
    }
}
