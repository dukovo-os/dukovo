// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The sockets Spark makes and holds for a service (ADR 0477).
//!
//! A service file's `[[sockets]]` names TCP and UDP ports, and Unix
//! sockets in the service's live folder. Spark makes them before the
//! service runs, at boot for every enabled service, and holds them while
//! the service is enabled or started: a restart, asked for or after a
//! failure, keeps them open, so that a connection made while no process
//! accepts waits in the backlog and is served once the next one runs.
//! Each start of the service gets them from descriptor 3, in the file's
//! order, as sd_listen_fds(3) has them.
//!
//! A TCP or UDP socket is bound through a capability Spark derives from
//! its own network capability for that port alone, with the receive and
//! inbound rights the service's `network.listen` grant has: holding it
//! gives no more than the service's file does, and the firewall takes
//! what other machines start on the port while it is held (ADR 0334). A
//! Unix socket is named in the service's live folder:
//! `/system/live/services/ACCOUNT` for an account's service, which is the
//! account's and the socket with it, mode 0600, and `/system/live/NAME`
//! for the system's, mode 0666, as systemd's `SocketMode=` default is:
//! the services' views decide which reach it. A socket the file offers
//! others (`to` or `group`) is root's, so that the service cannot widen
//! who reaches it: mode 0666 for anyone, 0600 for root's programs, 0660
//! with the group for its members; the live folder is then anyone's to
//! look in, mode 0755, as Debian's `/run/postgresql` is, and the
//! sockets' modes keep the rest out. Its name goes once Spark closes it.
//!
//! None is non-blocking: the service shares each with Spark, status flags
//! and all, and a program that takes a socket from systemd expects it
//! blocking.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use libradix::abi::capability::{self, Rights};
use libradix::abi::directory::{self as directory_abi, FileKind};
use libradix::abi::errors::File as FileError;
use libradix::abi::file::{self as file_abi, SocketOption};
use libradix::abi::network::{self, Address, AddressFamily, SocketType};
use libradix::abi::process::{self, PipeFlags};
use libradix::abi::startup::SLOT_PROCESS;
use manifest::service::{Socket, SocketAccess, SocketAddress, SocketKind};
use radix_rt::fs::{Dir, File};
use radix_rt::{SYS, slots};

use crate::services::{self, Spec};

/// The most connections the kernel holds waiting at a socket
/// (radix-file(2)'s BACKLOG_MAX): a larger `backlog` gets this many, as
/// Linux's `somaxconn` caps one.
const BACKLOG_MAX: u32 = 4096;

/// The mode of a system service's Unix socket, and of an account's
/// service's.
const MODE_SYSTEM: u16 = 0o666;
const MODE_ACCOUNT: u16 = 0o600;

/// The mode of an account's live folder: its own, and anyone's to look
/// in once it offers a socket.
const FOLDER_OWN: u16 = 0o700;
const FOLDER_OFFERING: u16 = 0o755;

/// The sockets Spark holds for a service, in its file's order.
pub struct Held {
    /// The sockets.
    files: Vec<File>,
    /// What each was made from, to tell whether a file read again names
    /// others.
    made_from: Vec<Socket>,
    /// The names of the Unix sockets beneath the root, removed as they
    /// close.
    paths: Vec<String>,
}

impl Held {
    /// How many there are.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Their slots, in the file's order.
    pub fn slots(&self) -> impl Iterator<Item = u32> + '_ {
        self.files.iter().map(File::slot)
    }

    /// `LISTEN_FDNAMES`: their names, a colon between each.
    pub fn names(&self) -> String {
        let names: Vec<&str> = self.made_from.iter().map(|socket| socket.name.as_str()).collect();
        names.join(":")
    }

    /// Whether they are the sockets `sockets` names.
    pub fn made_from(&self, sockets: &[Socket]) -> bool {
        self.made_from == sockets
    }

    /// Closes them, and removes the Unix sockets' names beneath `root`.
    pub fn close(self, root: &Dir) {
        drop(self.files);
        for path in &self.paths {
            let _ = root.remove(path);
        }
    }
}

/// What a socket is, in words: "port 8080", "UDP port 53 on ::",
/// "/system/live/web/db".
pub fn words(socket: &Socket, live: &str) -> String {
    let on = |address: SocketAddress| match address.text() {
        Some(text) => format!(" on {text}"),
        None => String::new(),
    };
    match &socket.kind {
        SocketKind::Stream { port, address } => format!("port {port}{}", on(*address)),
        SocketKind::Datagram { port, address } => format!("UDP port {port}{}", on(*address)),
        SocketKind::Unix { path } => format!("/{live}/{path}"),
    }
}

/// The folder beneath the root where service `spec`'s Unix sockets are
/// named: its account's live folder, or its own.
pub fn live_folder(spec: &Spec) -> String {
    match &spec.account {
        Some(account) => format!("system/live/services/{account}"),
        None => format!("system/live/{}", spec.name),
    }
}

/// Makes service `spec`'s sockets: TCP and UDP through Spark's `network`
/// capability, Unix ones beneath `root`, given to `owner` (its account's
/// user and group) when it has one. Says why when one cannot be made, as
/// when another program has its port; none is held then.
pub fn make(
    root: &Dir,
    network: Option<u32>,
    spec: &Spec,
    owner: Option<(u32, u32)>,
) -> Result<Held, String> {
    let live = live_folder(spec);
    let mut held = Held { files: Vec::new(), made_from: Vec::new(), paths: Vec::new() };
    let offering = spec.sockets.iter().any(|socket| socket.access.offered());
    let groups = spec
        .sockets
        .iter()
        .any(|socket| matches!(socket.access, SocketAccess::Group(_)))
        .then(|| root.read_file(&manifest::service::GROUP[1..]).unwrap_or_default());
    for socket in &spec.sockets {
        let made = match &socket.kind {
            SocketKind::Stream { port, address } => {
                network_socket(network, SocketType::Stream, *address, *port, socket.backlog)
            }
            SocketKind::Datagram { port, address } => {
                network_socket(network, SocketType::Datagram, *address, *port, 0)
            }
            SocketKind::Unix { path } => {
                let at = format!("{live}/{path}");
                let made = access(&socket.access, owner, groups.as_deref()).and_then(|access| {
                    let folder = Folder { path: &live, owner, offering };
                    unix_socket(root, &folder, &at, access, socket.backlog)
                });
                if made.is_ok() {
                    held.paths.push(at);
                }
                made
            }
        };
        match made {
            Ok(file) => {
                held.files.push(file);
                held.made_from.push(socket.clone());
            }
            Err(why) => {
                let what = words(socket, &live);
                held.close(root);
                return Err(format!("cannot make its socket at {what}: {why}"));
            }
        }
    }
    Ok(held)
}

/// A TCP socket listening with `backlog`, or a UDP one, on `port` of
/// `address`, bound through a capability for that port alone derived
/// from `network`.
fn network_socket(
    network: Option<u32>,
    kind: SocketType,
    address: SocketAddress,
    port: u16,
    backlog: u32,
) -> Result<File, String> {
    let network = network.ok_or("Spark has no network capability")?;
    let (family, ip, v6_only): (AddressFamily, [u8; 16], Option<bool>) = match address {
        // IPv4 too, as IPv4-mapped addresses: one socket for both, as
        // systemd's ListenStream=80 makes.
        SocketAddress::Any => (AddressFamily::Ipv6, [0; 16], Some(false)),
        SocketAddress::AnyV6 => (AddressFamily::Ipv6, [0; 16], Some(true)),
        SocketAddress::V6(octets) => (AddressFamily::Ipv6, octets, None),
        SocketAddress::AnyV4 => (AddressFamily::Ipv4, [0; 16], None),
        SocketAddress::V4(octets) => {
            let mut ip = [0; 16];
            ip[..4].copy_from_slice(&octets);
            (AddressFamily::Ipv4, ip, None)
        }
    };
    let slot = slots::take().ok_or("no free slot")?;
    if let Err(status) = network::socket(&SYS, network, slot, family, kind, PipeFlags::NONE) {
        slots::give_back(slot);
        return Err(format!("{:#x}", status.0));
    }
    let file = File::own(slot).map_err(|err| err.to_string())?;
    let option = |option: SocketOption, on: bool| {
        file_abi::socket_option(&SYS, file.slot(), option, true, u64::from(on))
            .map(drop)
            .map_err(|status| format!("{:#x}", status.0))
    };
    if let Some(only) = v6_only {
        option(SocketOption::V6Only, only)?;
    }
    if kind == SocketType::Stream {
        // A port whose last connections are still closing is free to
        // listen on again, as a restart of Spark's own would need.
        option(SocketOption::ReuseAddress, true)?;
    }
    bind(network, &file, family, &ip, port)?;
    if kind == SocketType::Stream {
        file_abi::listen(&SYS, file.slot(), backlog.clamp(1, BACKLOG_MAX))
            .map_err(|status| format!("{:#x}", status.0))?;
    }
    Ok(file)
}

/// Binds `file` to `port` of `ip` in `family`, through a capability
/// derived from `network` for that port alone: the receive right, which
/// binding needs, and the inbound right, through which other machines
/// reach it (ADR 0334).
fn bind(
    network: u32,
    file: &File,
    family: AddressFamily,
    ip: &[u8; 16],
    port: u16,
) -> Result<(), String> {
    let mut address = [0; Address::SIZE];
    let at = Address::OFFSET_FAMILY;
    address[at..at + 4].copy_from_slice(&family.raw().to_le_bytes());
    let at = Address::OFFSET_PORT;
    address[at..at + 2].copy_from_slice(&port.to_le_bytes());
    let length = if family == AddressFamily::Ipv4 { 4 } else { 16 };
    let at = Address::OFFSET_IP;
    address[at..at + length].copy_from_slice(&ip[..length]);
    let slot = slots::take().ok_or("no free slot")?;
    let rights = Rights::RECEIVE.union(Rights::INBOUND);
    let bound = network::restrict(&SYS, network, slot, u32::from(port), 1, rights)
        .and_then(|()| file_abi::bind_address(&SYS, file.slot(), slot, &address));
    let _ = capability::delete(&SYS, slot);
    slots::give_back(slot);
    bound.map_err(|status| match libradix::abi::errors::Kernel::from_status(status) {
        Some(kind) => String::from(kind.name()),
        None => format!("{:#x}", status.0),
    })
}

/// Who a Unix socket is: its owner and group, and its mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Access {
    user: u32,
    group: u32,
    mode: u16,
}

/// The owner, group and mode of a socket that `access` says who connects
/// to, of a service run as `owner` (its account's user and group), or as
/// root; `groups` is the groups file, read when a socket names one.
fn access(
    access: &SocketAccess,
    owner: Option<(u32, u32)>,
    groups: Option<&[u8]>,
) -> Result<Access, String> {
    Ok(match access {
        SocketAccess::Own => match owner {
            Some((user, group)) => Access { user, group, mode: MODE_ACCOUNT },
            None => Access { user: 0, group: 0, mode: MODE_SYSTEM },
        },
        SocketAccess::Anyone => Access { user: 0, group: 0, mode: 0o666 },
        SocketAccess::System => Access { user: 0, group: 0, mode: 0o600 },
        SocketAccess::Group(name) => {
            let text = core::str::from_utf8(groups.unwrap_or_default()).unwrap_or_default();
            let group = manifest::service::group_number(text, name)
                .ok_or_else(|| format!("there is no group {name}"))?;
            Access { user: 0, group, mode: 0o660 }
        }
    })
}

/// A service's live folder: its path beneath the root, its account's
/// numbers, and whether it offers a socket to others.
struct Folder<'a> {
    path: &'a str,
    owner: Option<(u32, u32)>,
    offering: bool,
}

/// A Unix stream socket named `at` beneath `root`, in `folder`, listening
/// with `backlog`, with the owner, group and mode of `access`.
fn unix_socket(
    root: &Dir,
    folder: &Folder<'_>,
    at: &str,
    access: Access,
    backlog: u32,
) -> Result<File, String> {
    let live = folder.path;
    let new = root.stat(live).is_err();
    services::make_dirs(root, live)?;
    if new && let Some((user, group)) = folder.owner {
        // As a view gives a place its service may change (ADR 0217).
        root.set_access(live, Some(user), Some(group), Some(FOLDER_OWN))
            .map_err(|err| format!("cannot give /{live} to its account: {err}"))?;
    }
    if folder.offering {
        // Others reach what it offers by name; what they may connect to,
        // each socket's mode says.
        root.set_access(live, None, None, Some(FOLDER_OFFERING))
            .map_err(|err| format!("cannot open /{live} to those it offers sockets: {err}"))?;
    }
    // A name left at the path by a run before is no socket of ours: one of
    // another kind is a mistake that is not Spark's to remove.
    match root.stat(at) {
        Ok(stat) if stat.kind == FileKind::Socket => {
            root.remove(at).map_err(|err| format!("cannot remove the old /{at}: {err}"))?;
        }
        Ok(_) => return Err(format!("/{at} is there, and is no socket")),
        Err(err) if err.is(FileError::NotFound) => {}
        Err(err) => return Err(format!("/{at}: {err}")),
    }
    let slot = slots::take().ok_or("no free slot")?;
    if let Err(status) = process::socket(&SYS, SLOT_PROCESS, slot, PipeFlags::NONE) {
        slots::give_back(slot);
        return Err(format!("{:#x}", status.0));
    }
    let file = File::own(slot).map_err(|err| err.to_string())?;
    let name = format!("/{at}");
    directory_abi::bind_socket(&SYS, root.slot(), at.as_bytes(), file.slot(), name.as_bytes())
        .map_err(|status| format!("{:#x}", status.0))?;
    let listened = root
        .set_access(at, Some(access.user), Some(access.group), Some(access.mode))
        .map_err(|err| err.to_string())
        .and_then(|()| {
            file_abi::listen(&SYS, file.slot(), backlog.clamp(1, BACKLOG_MAX))
                .map_err(|status| format!("{:#x}", status.0))
        });
    if let Err(why) = listened {
        let _ = root.remove(at);
        return Err(why);
    }
    Ok(file)
}
