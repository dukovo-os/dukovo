// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! wgd(8): WireGuard, on boringtun (ADR 0340).
//!
//! For each file NAME.conf in /system/state/wireguard (wireguard.conf(5),
//! wg-quick's format), wgd makes the tunnel interface NAME (ADR 0339), or
//! takes the one of that name its last run left, opens it, gives it the
//! file's addresses, and carries its packets. What the machine sends
//! through it, wgd seals for the peer whose AllowedIPs hold the packet's
//! destination, the longest prefix first, and sends to the peer's
//! endpoint by UDP from the interface's port. What comes from a peer it
//! opens and gives the tunnel if the peer's AllowedIPs hold the packet's
//! source, as cryptokey routing asks (the WireGuard paper, section 2),
//! and the peer's endpoint becomes where it came from, so that a peer
//! that roams is followed.
//!
//! wg(8) talks to it on /system/live/wireguard/NAME.sock, which only the
//! system may open, in the text protocol every userspace WireGuard
//! speaks (`crate::wireguard`). SIGHUP (`service wgd reload`) reads the
//! files again: an interface whose file is new comes up, one whose file
//! went goes away, and the others take their files' settings as `wg
//! syncconf` would give them, keeping the sessions of the peers whose
//! keys did not change. What wg changed lasts until then. SIGTERM takes
//! the interfaces away and ends wgd.
//!
//! One thread carries every interface: it polls their tunnels, their
//! sockets and wg's connections, and every quarter second runs each
//! peer's timers (handshakes, keepalives, the end of old sessions), as
//! boringtun's own device does; every second it lets the handshakes'
//! rate limiter count afresh.

use std::cell::Cell;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use boringtun::noise::handshake::parse_handshake_anon;
use boringtun::noise::rate_limiter::RateLimiter;
use boringtun::noise::{Packet, Tunn, TunnResult};
use boringtun::x25519;
use radix_abi::Status;
use radix_abi::network::{self, Ipv6Flags, Ipv6Settings, LinkKind};
use radix_abi::process::PipeFlags;

use crate::args::Args;
use crate::ip::{self, Cidr, Cidr6, Network as _};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};
use crate::wireguard::{self, Config, Key, Prefix};

/// How often each peer's timers run, as boringtun's device runs them.
const TICK: Duration = Duration::from_millis(250);

/// How often the rate limiter counts afresh.
const LIMITER_RESET: Duration = Duration::from_secs(1);

/// The handshakes a second an interface takes before it asks for
/// cookies: boringtun's device's 100. Each is counted twice, once as wgd
/// finds its peer and once as the peer's tunnel opens it.
const HANDSHAKES_MAX: u64 = 200;

/// The packets read from one tunnel or socket before the others' turn.
const BATCH: usize = 64;

/// A buffer for a packet: a tunnel's at most TAGGED_FRAME bytes, a
/// datagram at most that and WireGuard's 32, with room to spare.
const BUFFER: usize = 4096;

/// The longest request wg may send.
const REQUEST_MAX: usize = 1 << 20;

/// How long wgd waits for the rest of a request.
const REQUEST_WAIT: Duration = Duration::from_secs(1);

/// The protocol's error numbers.
const EINVAL: i32 = libc::EINVAL;
const EADDRINUSE: i32 = libc::EADDRINUSE;

/// Set by SIGTERM, and by SIGHUP; the handler writes a byte into
/// [`WAKE`] too, which the poll waits on.
static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
    wake();
}

extern "C" fn on_hangup(_: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
    wake();
}

fn wake() {
    let fd = WAKE.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe; one byte from the stack.
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

/// A peer of an interface.
struct Peer {
    public: Key,
    preshared: Option<Key>,
    keepalive: Option<u16>,
    endpoint: Option<SocketAddr>,
    /// Its AllowedIPs, their networks.
    allowed: Vec<Prefix>,
    /// Its tunnel's index, which its packets carry: the receiver's index
    /// is it shifted up 8 bits.
    index: u32,
    /// Its session, while the interface has a private key.
    tunn: Option<Box<Tunn>>,
    /// Bytes it sent and received in sessions now gone.
    carried: (u64, u64),
    /// When its latest handshake was, as `get` last said it.
    handshake_at: Cell<Option<SystemTime>>,
}

impl Peer {
    /// When its latest handshake was, given `at`, what boringtun's time
    /// since it says: the time said before while `at` is within a second
    /// of it, as boringtun's clock moves on only at each tick, so that
    /// wg(8) shows one handshake one time, as Linux's does.
    fn handshake_time(&self, at: SystemTime) -> SystemTime {
        if let Some(before) = self.handshake_at.get()
            && at.duration_since(before).unwrap_or_else(|e| e.duration()) < Duration::from_secs(1)
        {
            return before;
        }
        self.handshake_at.set(Some(at));
        at
    }
}

/// An interface wgd carries.
struct Interface {
    name: String,
    index: u32,
    tunnel: File,
    socket: Option<UdpSocket>,
    port: u16,
    fwmark: u32,
    secret: Option<x25519::StaticSecret>,
    public: x25519::PublicKey,
    limiter: Arc<RateLimiter>,
    peers: Vec<Peer>,
    next_index: u32,
    listener: UnixListener,
    /// The addresses its file gives it.
    addresses: Vec<Prefix>,
    /// Taken away under wgd: it goes.
    gone: bool,
}

/// wgd(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills the two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } < 0 {
        return Err(util::failed(&"pipe", io::Error::last_os_error()));
    }
    WAKE.store(fds[1], Ordering::Relaxed);
    // SAFETY: setting signals' dispositions touches no memory; no other
    // thread runs. A wg that goes away must not end wgd.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    for (signal, handler) in
        [(libc::SIGTERM, on_term as extern "C" fn(libc::c_int)), (libc::SIGHUP, on_hangup)]
    {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers store to atomics and write(2).
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
    fs::create_dir_all(wireguard::STATE).map_err(|e| util::failed(&wireguard::STATE, e))?;
    // Its files hold private keys.
    fs::set_permissions(wireguard::STATE, fs::Permissions::from_mode(0o700))
        .map_err(|e| util::failed(&wireguard::STATE, e))?;
    fs::create_dir_all(wireguard::LIVE).map_err(|e| util::failed(&wireguard::LIVE, e))?;
    let mut interfaces = Vec::new();
    reload(&mut interfaces);
    let (mut tick, mut reset) = (Instant::now() + TICK, Instant::now() + LIMITER_RESET);
    let mut buffers = Buffers { input: vec![0; BUFFER], output: vec![0; BUFFER] };
    loop {
        if STOP.load(Ordering::Relaxed) {
            for interface in interfaces.drain(..) {
                interface.take_away();
            }
            return Ok(0);
        }
        if RELOAD.swap(false, Ordering::Relaxed) {
            eprintln!("wgd: reading {} again", wireguard::STATE);
            reload(&mut interfaces);
        }
        let now = Instant::now();
        let wait = tick.min(reset).saturating_duration_since(now);
        poll(fds[0], &mut interfaces, wait, &mut buffers);
        let now = Instant::now();
        if now >= tick {
            tick = now + TICK;
            for interface in &mut interfaces {
                interface.timers(&mut buffers.output);
            }
        }
        if now >= reset {
            reset = now + LIMITER_RESET;
            for interface in &interfaces {
                interface.limiter.reset_count();
            }
        }
        interfaces.retain(|interface| {
            if interface.gone {
                eprintln!("wgd: {} was taken away", interface.name);
                let _ = fs::remove_file(wireguard::socket_path(&interface.name));
            }
            !interface.gone
        });
    }
}

/// The buffers a packet goes through.
struct Buffers {
    input: Vec<u8>,
    output: Vec<u8>,
}

/// Waits up to `wait` for the wake pipe `wake`, a tunnel, a socket or a
/// connection of wg's, and does what came.
fn poll(wake: RawFd, interfaces: &mut [Interface], wait: Duration, buffers: &mut Buffers) {
    let event = |fd: RawFd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let mut fds = vec![event(wake)];
    for interface in interfaces.iter() {
        fds.push(event(interface.tunnel.as_raw_fd()));
        fds.push(event(interface.socket.as_ref().map_or(-1, AsRawFd::as_raw_fd)));
        fds.push(event(interface.listener.as_raw_fd()));
    }
    let ms = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX).max(1);
    let count = libc::nfds_t::try_from(fds.len()).unwrap_or(libc::nfds_t::MAX);
    // SAFETY: `fds` holds `count` pollfds.
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, ms) };
    if ready <= 0 {
        return;
    }
    if fds[0].revents != 0 {
        let mut drain = [0u8; 64];
        // SAFETY: reading into a buffer of ours, from the pipe we made.
        while unsafe { libc::read(wake, drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
    }
    for (interface, fds) in interfaces.iter_mut().zip(fds[1..].chunks(3)) {
        if fds[0].revents != 0 {
            interface.seal(buffers);
        }
        if fds[1].revents != 0 {
            interface.open(buffers);
        }
        if fds[2].revents != 0 {
            interface.answer_wg();
        }
    }
}

/// Reads the configuration files: brings up the interfaces whose files
/// are new, takes away those whose files went, and gives the others
/// their files' settings. One whose file has a mistake keeps what it
/// had, so that a slip of the pen leaves its tunnel up.
fn reload(interfaces: &mut Vec<Interface>) {
    let files = read_files();
    let (kept, gone): (Vec<Interface>, Vec<Interface>) = interfaces
        .drain(..)
        .partition(|interface| files.iter().any(|(name, _)| *name == interface.name));
    *interfaces = kept;
    for interface in gone {
        eprintln!("wgd: {}.conf went: taking {} away", interface.name, interface.name);
        interface.take_away();
    }
    for (name, config) in files {
        let found = interfaces.iter_mut().find(|i| i.name == name);
        match (found, config) {
            (Some(interface), Some(config)) => interface.sync(&config),
            (Some(_), None) => eprintln!("wgd: {name} keeps what it had, until its file is right"),
            (None, Some(config)) => match Interface::bring_up(&name, &config) {
                Ok(interface) => interfaces.push(interface),
                Err(why) => eprintln!("wgd: {name}: {why}"),
            },
            (None, None) => {}
        }
    }
}

/// The configuration files, by interface name: what each says, or
/// `None` for one that could not be read or has a mistake, which is
/// said.
fn read_files() -> Vec<(String, Option<Config>)> {
    let Ok(entries) = fs::read_dir(wireguard::STATE) else { return Vec::new() };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Ok(file) = entry.file_name().into_string() else { continue };
        let Some(name) = file.strip_suffix(".conf") else { continue };
        let path = entry.path();
        if !wireguard::valid_name(name) {
            eprintln!(
                "wgd: {}: not an interface's name (1 to 15 letters, digits, _=+.-)",
                path.display()
            );
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && meta.permissions().mode() & 0o077 != 0
        {
            eprintln!("wgd: {} is readable by others; its keys are private", path.display());
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("wgd: {}: {}", path.display(), util::strip_os_error(&e));
                files.push((name.to_string(), None));
                continue;
            }
        };
        let mut warn = |why: String| eprintln!("wgd: {}: {why}", path.display());
        let config = wireguard::parse_config(&text, &mut warn)
            .inspect_err(|why| eprintln!("wgd: {}: {why}", path.display()))
            .ok();
        files.push((name.to_string(), config));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// What the kernel refused, as the C library says it.
fn refused(status: Status) -> String {
    util::strip_os_error(&sys::io_error(status))
}

/// The tunnel interface `name`: the one wgd's last run left, or a new
/// one. Its number.
fn make_tunnel(name: &str) -> Result<u32, String> {
    let interfaces =
        ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES)).unwrap_or_default();
    if let Some(found) = interfaces.iter().find(|interface| interface.name == name) {
        if found.kind != ip::Kind::Tunnel {
            return Err(format!("{name} is an interface already, and not a tunnel"));
        }
        return Ok(found.index);
    }
    ip::Slots.add_link(LinkKind::Tunnel, 0, 0, name).map_err(|status| match status {
        status if sys::errno_of(status) == libc::EPERM => {
            "making a tunnel needs admin.network (spark(8))".to_string()
        }
        status => format!("making the tunnel: {}", refused(status)),
    })
}

/// Opens tunnel `index`'s file, for its packets: radix-network(2)'s
/// `open_tunnel`, into the slot of the lowest free descriptor.
fn open_tunnel(index: u32) -> Result<File, String> {
    let flags = PipeFlags(PipeFlags::NON_BLOCKING.bits() | PipeFlags::CLOSE_ON_EXEC.bits());
    loop {
        // SAFETY: opening and closing a descriptor of our own.
        let fd = unsafe { libc::open(c"/devices/null".as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            return Err(util::strip_os_error(&io::Error::last_os_error()));
        }
        // SAFETY: `fd` was just opened here.
        unsafe { libc::close(fd) };
        let opened = ip::through_slots(|slot| {
            network::open_tunnel(&SYS, slot, sys::slot_of(fd), index, flags)
        });
        match opened {
            // SAFETY: the kernel put the tunnel's file in descriptor
            // `fd`'s slot, which nothing else owns.
            Ok(()) => return Ok(unsafe { File::from_raw_fd(fd) }),
            // Another thread took the descriptor: try the next.
            Err(status)
                if radix_abi::errors::Kernel::from_status(status)
                    == Some(radix_abi::errors::Kernel::Occupied) => {}
            Err(status) if sys::errno_of(status) == libc::EBUSY => {
                return Err("another program has the tunnel open".into());
            }
            Err(status) => return Err(format!("opening the tunnel: {}", refused(status))),
        }
    }
}

/// wg's socket for interface `name`, made anew, for the system alone.
fn listen(name: &str) -> io::Result<UnixListener> {
    let path = wireguard::socket_path(name);
    match fs::remove_file(&path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// The UDP socket of port `port`, 0 for any, for IPv6 and IPv4 both.
fn bind(port: u16) -> io::Result<UdpSocket> {
    let socket =
        UdpSocket::bind(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0)))?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// `address` as the socket sends to it: an IPv4 one IPv4-mapped.
fn mapped(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V4(v4) => {
            SocketAddr::V6(SocketAddrV6::new(v4.ip().to_ipv6_mapped(), v4.port(), 0, 0))
        }
        v6 @ SocketAddr::V6(_) => v6,
    }
}

/// A random number, for the first tunnel index.
fn random_u32() -> u32 {
    Key::random().map_or(1, |key| u32::from_le_bytes([key.0[0], key.0[1], key.0[2], key.0[3]]))
}

impl Interface {
    /// Makes interface `name` as `config` says.
    fn bring_up(name: &str, config: &Config) -> Result<Interface, String> {
        let index = make_tunnel(name)?;
        let tunnel = open_tunnel(index)?;
        let listener = listen(name).map_err(|e| {
            format!("{}: {}", wireguard::socket_path(name).display(), util::strip_os_error(&e))
        })?;
        let public = x25519::PublicKey::from([0u8; 32]);
        let mut interface = Interface {
            name: name.to_string(),
            index,
            tunnel,
            socket: None,
            port: 0,
            fwmark: 0,
            secret: None,
            limiter: Arc::new(RateLimiter::new(&public, HANDSHAKES_MAX)),
            public,
            peers: Vec::new(),
            next_index: random_u32() & 0x00ff_ffff,
            listener,
            addresses: Vec::new(),
            gone: false,
        };
        if config.listen_port.is_none() {
            interface.set_port(0).map_err(|_| "no UDP port is free".to_string())?;
        }
        interface.sync(config);
        eprintln!("wgd: {name} is up, on UDP port {}", interface.port);
        Ok(interface)
    }

    /// Gives it `config`'s settings, as `wg syncconf` would: peers the
    /// file does not name go, and those whose keys stayed keep their
    /// sessions.
    fn sync(&mut self, config: &Config) {
        let mut request = String::from("set=1\n");
        let key = config.private_key.unwrap_or(Key([0; 32]));
        let _ = writeln!(request, "private_key={}", key.hex());
        if let Some(port) = config.listen_port {
            let _ = writeln!(request, "listen_port={port}");
        }
        let _ = writeln!(request, "fwmark={}", config.fwmark.unwrap_or(0));
        for peer in &self.peers {
            if !config.peers.iter().any(|p| p.public_key == peer.public) {
                let _ = writeln!(request, "public_key={}\nremove=true", peer.public.hex());
            }
        }
        for peer in &config.peers {
            let mut peer = peer.clone();
            if let Some(endpoint) = &peer.endpoint
                && let Err(why) = wireguard::resolve_endpoint(endpoint)
            {
                eprintln!("wgd: {}: a peer's endpoint {why}; it has none for now", self.name);
                peer.endpoint = None;
            }
            // Resolved, so cannot fail.
            let _ = wireguard::peer_lines(&mut request, &peer);
        }
        let lines: Vec<&str> = request.lines().skip(1).collect();
        if let Err(errno) = self.set(&lines) {
            eprintln!(
                "wgd: {}: its file's settings: {}",
                self.name,
                wireguard::errno_text(&errno.to_string())
            );
        }
        if config.addresses != self.addresses {
            self.give_addresses(&config.addresses);
        }
    }

    /// Gives it `addresses`, in place of those it had.
    fn give_addresses(&mut self, addresses: &[Prefix]) {
        let mut four = Vec::new();
        let mut six = Vec::new();
        for prefix in addresses {
            match prefix.ip {
                IpAddr::V4(ip) => four.push(Cidr { ip, prefix: prefix.len }),
                IpAddr::V6(ip) => six.push(Cidr6 { ip, prefix: prefix.len }),
            }
        }
        if let Err(status) = ip::Slots.configure(self.index, &four, None) {
            eprintln!("wgd: {}: its IPv4 addresses: {}", self.name, refused(status));
        }
        // IPv6 on, with only the addresses it is given, or off without
        // any: a tunnel makes none of its own (ADR 0339).
        let flags = if six.is_empty() { Ipv6Flags::NONE } else { Ipv6Flags::ENABLED };
        let settings = Ipv6Settings { flags: flags.bits(), ..Ipv6Settings::default() };
        if let Err(status) = ip::Slots.configure_ipv6(self.index, Some(&settings), &six, None) {
            eprintln!("wgd: {}: its IPv6 addresses: {}", self.name, refused(status));
        }
        self.addresses = addresses.to_vec();
    }

    /// Listens on UDP port `port`, 0 for any free one.
    fn set_port(&mut self, port: u16) -> Result<(), i32> {
        if self.socket.is_some() && port == self.port && port != 0 {
            return Ok(());
        }
        let socket = bind(port).map_err(|_| EADDRINUSE)?;
        self.port = socket.local_addr().map_or(port, |address| address.port());
        self.socket = Some(socket);
        Ok(())
    }

    /// Takes it away: its tunnel goes, with its addresses, and its
    /// socket.
    fn take_away(self) {
        let (name, index) = (self.name.clone(), self.index);
        let _ = fs::remove_file(wireguard::socket_path(&name));
        // Its file closed first, as a tunnel a program has open stays.
        drop(self);
        match ip::Slots.remove_link(index) {
            Ok(()) => eprintln!("wgd: {name} is down"),
            Err(status) => eprintln!("wgd: taking {name} away: {}", refused(status)),
        }
    }

    /// A new index for a peer's tunnel, which no other peer has.
    fn new_index(&mut self) -> u32 {
        loop {
            self.next_index = (self.next_index + 1) & 0x00ff_ffff;
            let index = self.next_index;
            if !self.peers.iter().any(|peer| peer.index == index) {
                return index;
            }
        }
    }

    /// Makes peer `at`'s session afresh, with the interface's key and its
    /// own settings, or none without a key.
    fn renew(&mut self, at: usize) {
        let secret = self.secret.clone();
        let limiter = Arc::clone(&self.limiter);
        let peer = &mut self.peers[at];
        if let Some(tunn) = &peer.tunn {
            let (_, sent, received, _, _) = tunn.stats();
            peer.carried.0 += sent as u64;
            peer.carried.1 += received as u64;
        }
        peer.tunn = secret.map(|secret| {
            Box::new(Tunn::new(
                secret,
                x25519::PublicKey::from(peer.public.0),
                peer.preshared.map(|key| key.0),
                peer.keepalive,
                peer.index,
                Some(limiter),
            ))
        });
    }

    /// The peer whose AllowedIPs hold `ip`, the longest prefix first.
    fn route(&self, ip: IpAddr) -> Option<usize> {
        let mut best: Option<(usize, u8)> = None;
        for (at, peer) in self.peers.iter().enumerate() {
            for prefix in &peer.allowed {
                if prefix.contains(ip) && best.is_none_or(|(_, len)| prefix.len > len) {
                    best = Some((at, prefix.len));
                }
            }
        }
        best.map(|(at, _)| at)
    }

    /// Sends `packet` to `to`.
    fn send(&self, packet: &[u8], to: SocketAddr) {
        if let Some(socket) = &self.socket {
            let _ = socket.send_to(packet, mapped(to));
        }
    }

    /// Seals what the machine sent through the tunnel for its peers.
    fn seal(&mut self, buffers: &mut Buffers) {
        for _ in 0..BATCH {
            let read = match self.tunnel.read(&mut buffers.input) {
                Ok(read) => read,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                    self.gone = true;
                    return;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!(
                        "wgd: {}: reading the tunnel: {}",
                        self.name,
                        util::strip_os_error(&e)
                    );
                    return;
                }
            };
            let packet = &buffers.input[..read];
            // A packet no peer's AllowedIPs hold goes nowhere, as on Linux.
            let Some(to) = Tunn::dst_address(packet).and_then(|ip| self.route(ip)) else {
                continue;
            };
            let peer = &mut self.peers[to];
            // Without an endpoint, it waits for none: it is lost, as on
            // Linux.
            let (Some(tunn), Some(endpoint)) = (peer.tunn.as_mut(), peer.endpoint) else {
                continue;
            };
            if let TunnResult::WriteToNetwork(sealed) =
                tunn.encapsulate(packet, &mut buffers.output)
            {
                let sealed: &[u8] = sealed;
                if let Some(socket) = &self.socket {
                    let _ = socket.send_to(sealed, mapped(endpoint));
                }
            }
        }
    }

    /// Opens what came from the network for its peers.
    fn open(&mut self, buffers: &mut Buffers) {
        for _ in 0..BATCH {
            let Some(socket) = &self.socket else { return };
            let (read, from) = match socket.recv_from(&mut buffers.input) {
                Ok(got) => got,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            let from = SocketAddr::new(from.ip().to_canonical(), from.port());
            self.datagram(read, from, buffers);
        }
    }

    /// Opens the datagram of `len` bytes in `buffers.input` from `from`.
    fn datagram(&mut self, len: usize, from: SocketAddr, buffers: &mut Buffers) {
        let datagram = &buffers.input[..len];
        // The MACs checked and handshakes counted first, which costs
        // little, before a handshake's key is worked out to find its peer.
        let peer = match self.limiter.verify_packet(Some(from.ip()), datagram, &mut buffers.output)
        {
            Ok(Packet::HandshakeInit(init)) => {
                let Some(secret) = &self.secret else { return };
                let Ok(half) = parse_handshake_anon(secret, &self.public, &init) else { return };
                self.peers.iter().position(|peer| peer.public.0 == half.peer_static_public)
            }
            Ok(
                Packet::HandshakeResponse(boringtun::noise::HandshakeResponse {
                    receiver_idx, ..
                })
                | Packet::PacketCookieReply(boringtun::noise::PacketCookieReply {
                    receiver_idx, ..
                })
                | Packet::PacketData(boringtun::noise::PacketData { receiver_idx, .. }),
            ) => self.peers.iter().position(|peer| peer.index == receiver_idx >> 8),
            // Too many handshakes: the cookie that asks the sender to
            // prove its address.
            Err(TunnResult::WriteToNetwork(cookie)) => {
                let cookie: &[u8] = cookie;
                self.send(cookie, from);
                return;
            }
            Err(_) => return,
        };
        let Some(at) = peer else { return };
        let mut flush = false;
        // A packet for the tunnel: where it is in the output buffer, and
        // its source.
        let mut opened = None;
        {
            let Some(tunn) = self.peers[at].tunn.as_mut() else { return };
            let start = buffers.output.as_ptr().addr();
            match tunn.decapsulate(Some(from.ip()), &buffers.input[..len], &mut buffers.output) {
                TunnResult::Done => {}
                TunnResult::Err(_) => return,
                TunnResult::WriteToNetwork(answer) => {
                    flush = true;
                    let answer: &[u8] = answer;
                    if let Some(socket) = &self.socket {
                        let _ = socket.send_to(answer, mapped(from));
                    }
                }
                TunnResult::WriteToTunnelV4(packet, source) => {
                    let at = packet.as_ptr().addr() - start;
                    opened = Some((at..at + packet.len(), IpAddr::V4(source)));
                }
                TunnResult::WriteToTunnelV6(packet, source) => {
                    let at = packet.as_ptr().addr() - start;
                    opened = Some((at..at + packet.len(), IpAddr::V6(source)));
                }
            }
        }
        // Cryptokey routing: the tunnel takes it only if its source is
        // the peer's by the AllowedIPs of every peer, the longest prefix
        // first; else a peer could send as another's addresses.
        if let Some((range, source)) = opened
            && !range.is_empty()
            && self.route(source) == Some(at)
        {
            let _ = self.tunnel.write(&buffers.output[range]);
        }
        // The packets that waited for the handshake go now.
        if flush {
            while let Some(tunn) = self.peers[at].tunn.as_mut()
                && let TunnResult::WriteToNetwork(queued) =
                    tunn.decapsulate(None, &[], &mut buffers.output)
            {
                let queued: &[u8] = queued;
                if let Some(socket) = &self.socket {
                    let _ = socket.send_to(queued, mapped(from));
                }
            }
        }
        // It came from there: the peer is there now.
        self.peers[at].endpoint = Some(from);
    }

    /// Runs each peer's timers.
    fn timers(&mut self, output: &mut [u8]) {
        for peer in &mut self.peers {
            let (Some(tunn), Some(endpoint)) = (peer.tunn.as_mut(), peer.endpoint) else {
                continue;
            };
            if let TunnResult::WriteToNetwork(packet) = tunn.update_timers(output) {
                let packet: &[u8] = packet;
                if let Some(socket) = &self.socket {
                    let _ = socket.send_to(packet, mapped(endpoint));
                }
            }
        }
    }

    /// Answers the connections wg made.
    fn answer_wg(&mut self) {
        loop {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            let _ = self.answer(stream);
        }
    }

    /// Reads a request from `stream`, and answers it.
    fn answer(&mut self, mut stream: UnixStream) -> io::Result<()> {
        stream.set_nonblocking(false)?;
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        while !request.ends_with(b"\n\n") && request.len() < REQUEST_MAX {
            if !wireguard::readable(&stream, REQUEST_WAIT)? {
                return Ok(());
            }
            let read = stream.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
        }
        let request = String::from_utf8_lossy(&request);
        let mut lines = request.lines();
        let answer = match lines.next() {
            Some("get=1") => self.get(),
            Some("set=1") => {
                let lines: Vec<&str> = lines.take_while(|line| !line.is_empty()).collect();
                let errno = self.set(&lines).err().unwrap_or(0);
                format!("errno={errno}\n\n")
            }
            _ => format!("errno={EINVAL}\n\n"),
        };
        stream.write_all(answer.as_bytes())
    }

    /// The answer to `get=1`.
    fn get(&self) -> String {
        let mut text = String::new();
        if let Some(secret) = &self.secret {
            let _ = writeln!(text, "private_key={}", Key(secret.to_bytes()).hex());
        }
        let _ = writeln!(text, "listen_port={}", self.port);
        if self.fwmark != 0 {
            let _ = writeln!(text, "fwmark={}", self.fwmark);
        }
        let now = SystemTime::now();
        for peer in &self.peers {
            let _ = writeln!(text, "public_key={}", peer.public.hex());
            let preshared = peer.preshared.unwrap_or(Key([0; 32]));
            let _ = writeln!(text, "preshared_key={}\nprotocol_version=1", preshared.hex());
            if let Some(endpoint) = peer.endpoint {
                let _ = writeln!(text, "endpoint={}", wireguard::endpoint_text(endpoint));
            }
            let (since, sent, received) = peer.tunn.as_ref().map_or((None, 0, 0), |tunn| {
                let (since, sent, received, _, _) = tunn.stats();
                (since, sent as u64, received as u64)
            });
            let at = since
                .and_then(|since| now.checked_sub(since))
                .map(|at| peer.handshake_time(at))
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .unwrap_or_default();
            let _ = writeln!(
                text,
                "last_handshake_time_sec={}\nlast_handshake_time_nsec={}\ntx_bytes={}\nrx_bytes={}\n\
                 persistent_keepalive_interval={}",
                at.as_secs(),
                at.subsec_nanos(),
                peer.carried.0 + sent,
                peer.carried.1 + received,
                peer.keepalive.unwrap_or(0)
            );
            for prefix in &peer.allowed {
                let _ = writeln!(text, "allowed_ip={prefix}");
            }
        }
        text.push_str("errno=0\n\n");
        text
    }

    /// Does what the lines of a `set=1` request say, in order: the error
    /// number of the first it could not do, the rest then left undone.
    fn set(&mut self, lines: &[&str]) -> Result<(), i32> {
        // The peer the lines after `public_key` change; whether this
        // request made it; whether its lines are passed over, after
        // `remove`, or for the interface's own key.
        let mut peer: Option<usize> = None;
        let mut made = false;
        let mut skipping = false;
        // Peers whose sessions are made afresh once the request is done.
        let mut renew: Vec<Key> = Vec::new();
        let result = (|| {
            for line in lines {
                let (key, value) = line.split_once('=').ok_or(EINVAL)?;
                let yes = || if value == "true" { Ok(()) } else { Err(EINVAL) };
                match key {
                    "private_key" if peer.is_none() => {
                        self.set_key(Key::from_hex(value).ok_or(EINVAL)?);
                    }
                    "listen_port" if peer.is_none() => {
                        self.set_port(value.parse().map_err(|_| EINVAL)?)?;
                    }
                    "fwmark" if peer.is_none() => {
                        self.fwmark = value.parse().map_err(|_| EINVAL)?;
                    }
                    "replace_peers" if peer.is_none() => {
                        yes()?;
                        self.peers.clear();
                    }
                    "public_key" => {
                        let key = Key::from_hex(value).ok_or(EINVAL)?;
                        (made, skipping) = (false, false);
                        peer = self.peers.iter().position(|p| p.public == key);
                        if peer.is_none() {
                            if self.secret.is_some() && key.0 == self.public.to_bytes() {
                                // The interface's own key is no peer's.
                                skipping = true;
                                continue;
                            }
                            let index = self.new_index();
                            self.peers.push(Peer {
                                public: key,
                                preshared: None,
                                keepalive: None,
                                endpoint: None,
                                allowed: Vec::new(),
                                index,
                                tunn: None,
                                carried: (0, 0),
                                handshake_at: Cell::new(None),
                            });
                            renew.push(key);
                            (peer, made) = (Some(self.peers.len() - 1), true);
                        }
                    }
                    _ if skipping => {}
                    "remove" | "update_only" => {
                        yes()?;
                        let at = peer.ok_or(EINVAL)?;
                        // update_only: a peer only to change, so one this
                        // request made goes again.
                        if key == "remove" || made {
                            let key = self.peers.remove(at).public;
                            renew.retain(|k| *k != key);
                            (peer, skipping) = (None, true);
                        }
                    }
                    "allowed_ip" => {
                        let at = peer.ok_or(EINVAL)?;
                        let prefix = Prefix::parse(value).ok_or(EINVAL)?.network();
                        // A prefix is one peer's: given to another, it
                        // moves, as on Linux.
                        for (other, p) in self.peers.iter_mut().enumerate() {
                            if other != at {
                                p.allowed.retain(|q| *q != prefix);
                            }
                        }
                        if !self.peers[at].allowed.contains(&prefix) {
                            self.peers[at].allowed.push(prefix);
                        }
                    }
                    _ => {
                        let current = &mut self.peers[peer.ok_or(EINVAL)?];
                        match key {
                            "preshared_key" => {
                                let key = Key::from_hex(value).ok_or(EINVAL)?;
                                let key = (!key.is_zero()).then_some(key);
                                if current.preshared != key {
                                    current.preshared = key;
                                    renew.push(current.public);
                                }
                            }
                            "endpoint" => {
                                let endpoint: SocketAddr = value.parse().map_err(|_| EINVAL)?;
                                current.endpoint = Some(SocketAddr::new(
                                    endpoint.ip().to_canonical(),
                                    endpoint.port(),
                                ));
                            }
                            "persistent_keepalive_interval" => {
                                let seconds: u16 = value.parse().map_err(|_| EINVAL)?;
                                let seconds = (seconds != 0).then_some(seconds);
                                if current.keepalive != seconds {
                                    current.keepalive = seconds;
                                    renew.push(current.public);
                                }
                            }
                            "replace_allowed_ips" => {
                                yes()?;
                                current.allowed.clear();
                            }
                            "protocol_version" if value == "1" => {}
                            _ => return Err(EINVAL),
                        }
                    }
                }
            }
            Ok(())
        })();
        // What was done stays done, the sessions it changed made afresh.
        renew.sort();
        renew.dedup();
        for key in renew {
            if let Some(at) = self.peers.iter().position(|p| p.public == key) {
                self.renew(at);
            }
        }
        result
    }

    /// Gives the interface the private key `key`, or none if it is zero:
    /// every peer's session is made afresh.
    fn set_key(&mut self, key: Key) {
        let same = self.secret.as_ref().map(x25519::StaticSecret::to_bytes)
            == (!key.is_zero()).then_some(key.0);
        if same {
            return;
        }
        if key.is_zero() {
            self.secret = None;
            self.public = x25519::PublicKey::from([0u8; 32]);
        } else {
            let secret = x25519::StaticSecret::from(key.0);
            self.public = x25519::PublicKey::from(&secret);
            self.secret = Some(secret);
            // A peer with the interface's own key is none.
            let public = Key(self.public.to_bytes());
            self.peers.retain(|peer| peer.public != public);
        }
        self.limiter = Arc::new(RateLimiter::new(&self.public, HANDSHAKES_MAX));
        for at in 0..self.peers.len() {
            self.renew(at);
        }
    }
}
