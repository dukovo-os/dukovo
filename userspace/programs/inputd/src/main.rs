// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! inputd(8): hands the input devices to whoever holds the screens (ADR
//! 0707).
//!
//! Each keyboard, mouse and tablet is a file of its own,
//! _/devices/input/N_, which only root may open (radix-input(2)). A
//! program that draws the screens, a desktop, needs them all, and needs
//! each new one as it is plugged in. It asks inputd, on
//! [`service::SOCKET`], and proves what it is by what it holds: it sends
//! a copy of its description of the display layer's file. inputd opens
//! every device's file afresh for that program, ties it to that holder
//! of the screens (INPUT_TIE), so that it reads events only while the
//! holder is in front, and sends the descriptions over the socket, each
//! with the device's number and what it is. Then it sends each device
//! that comes, and tells of each that goes. `input::service` is the
//! exchange, record for record.
//!
//! inputd reads no event: a key goes from the kernel to the program that
//! is in front and wakes nobody else. It keeps no list of programs it
//! trusts either. A program that sends anything but a holder of the
//! screens is refused; what a holder is handed is silent whenever the
//! holder is not in front, and for as long as the secure key's hold
//! lasts (ADR 0708).
//!
//! inputd waits on the devices that come and go and on its socket, and
//! on nothing else: while no device is plugged in or pulled out and no
//! program connects, it does not run. The consoles do not depend on it:
//! consoled(8) reads _/devices/kbd_ itself.
//!
//! [`Broker`] is all of that but the files and the socket, which reach
//! it through [`Devices`] and [`Peer`], so that the tests hand devices
//! out on the build machine.
//!
//! inputd is a program of its own, and not one of systools(1)'s: it
//! runs as root for as long as the system does, so it holds no code but
//! its own, and none of the memory a program of many commands sets up
//! for them all.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use input::abi::{DEVICE_ADDED, DEVICE_REMOVED, InputChange};
use input::kind::Kinds;
use input::service::{
    self, HOLDER, REFUSED_FULL, REFUSED_NOT_A_HOLDER, REFUSED_VERSION, Record, VERSION,
};
use input::{Control, Device};
use sysapi::poll::{self, PollFd};
use sysapi::signal::{self, Caught};
use sysapi::{kernel, net};

#[cfg(test)]
mod tests;

/// The file that lists the devices and tells of each that comes or goes.
const CONTROL: &str = "/devices/input/control";

/// The folder of the devices' files, each named by its number.
const DEVICES: &str = "/devices/input";

/// The merged keyboard file, which is there whatever devices are: what a
/// program sent is a holder of the screens if this file can be tied to
/// it.
const KEYBOARD: &str = "/devices/kbd";

/// The most programs that are connected at once: one for each holder of
/// the screens there can be (radix-display(2)), and a few that are still
/// to say what they hold.
pub(crate) const CLIENTS_MAX: usize = 16;

/// How long a program that connected has to send its holder.
const HELLO_TIME: Duration = Duration::from_secs(2);

/// Set when inputd is told to end.
static STOP: Caught = Caught::new();

/// The devices and their files, as inputd reaches them.
pub(crate) trait Devices {
    /// A description of a device's file, opened for one program.
    type File;
    /// What a program sent to say which holder of the screens it is.
    type Holder;

    /// The number of each device there is, lowest first.
    fn list(&self) -> io::Result<Vec<u32>>;

    /// Opens the file of device `number` afresh.
    fn open(&self, number: u32) -> io::Result<Self::File>;

    /// What the device `file` is of is: a keyboard, a pointer.
    fn kinds(&self, file: &Self::File) -> io::Result<Kinds>;

    /// Ties `file` to `holder`: it reads events only while that holder
    /// of the screens is in front.
    fn tie(&self, file: &Self::File, holder: &Self::Holder) -> io::Result<()>;

    /// Whether `holder` is a holder of the screens.
    fn holds(&self, holder: &Self::Holder) -> io::Result<bool>;
}

/// A program inputd hands devices to: how a record reaches it.
pub(crate) trait Peer<F> {
    /// Sends `record`, and `file` with it if there is one.
    ///
    /// # Errors
    ///
    /// If the program has gone, or does not read what it is sent.
    fn send(&mut self, record: Record, file: Option<&F>) -> io::Result<()>;
}

/// A program that holds a screen, and what it has been handed.
struct Client<P, H> {
    id: u64,
    peer: P,
    holder: H,
    /// The devices it was sent and not told have gone.
    has: BTreeSet<u32>,
}

/// The program is gone, or does not take what it is sent: it is served
/// no longer.
struct Left;

/// Why a program is refused, for the log.
fn why(reason: u32) -> &'static str {
    match reason {
        REFUSED_VERSION => "its first record is not a holder of a version inputd knows",
        REFUSED_FULL => "as many programs as inputd serves are connected already",
        _ => "it sent no holder of the screens",
    }
}

/// Who is handed the devices: every program that proved it holds a
/// screen, and what each has.
pub(crate) struct Broker<D: Devices, P> {
    devices: D,
    clients: Vec<Client<P, D::Holder>>,
    /// The number the next program is known by.
    next: u64,
}

impl<D: Devices, P: Peer<D::File>> Broker<D, P> {
    /// A broker of `devices` that serves nobody yet.
    pub(crate) fn new(devices: D) -> Broker<D, P> {
        Broker { devices, clients: Vec::new(), next: 1 }
    }

    /// The devices, as inputd reaches them.
    pub(crate) fn devices(&self) -> &D {
        &self.devices
    }

    /// The programs that are served: what each is known by, and how
    /// records reach it.
    pub(crate) fn peers(&self) -> impl Iterator<Item = (u64, &P)> {
        self.clients.iter().map(|client| (client.id, &client.peer))
    }

    /// A program said `first`, with `holder` if a descriptor came with
    /// it. If it is a holder of the screens it is handed every device
    /// there is and told that is all, and from then on served; the
    /// answer is what it is known by. Otherwise it is told why not, and
    /// the answer is `None`. The log says which.
    pub(crate) fn join(
        &mut self,
        mut peer: P,
        first: Record,
        holder: Option<D::Holder>,
    ) -> Option<u64> {
        let refusal = if first.what != HOLDER || first.number != VERSION {
            Some(REFUSED_VERSION)
        } else if self.clients.len() >= CLIENTS_MAX {
            Some(REFUSED_FULL)
        } else {
            None
        };
        let holder = match (refusal, holder) {
            (None, Some(holder)) if self.holds(&holder) => holder,
            (refusal, _) => {
                let reason = refusal.unwrap_or(REFUSED_NOT_A_HOLDER);
                eprintln!("inputd: a program is refused the input devices: {}", why(reason));
                // It may be gone already, which changes nothing.
                let _ = peer.send(Record::refused(reason), None);
                return None;
            }
        };
        let id = self.next;
        self.next += 1;
        let mut client = Client { id, peer, holder, has: BTreeSet::new() };
        let listed = self.devices.list().unwrap_or_else(|err| {
            eprintln!("inputd: cannot list the input devices: {err}");
            Vec::new()
        });
        for number in listed {
            Self::hand(&self.devices, &mut client, number).ok()?;
        }
        let count = u32::try_from(client.has.len()).unwrap_or(u32::MAX);
        client.peer.send(Record::ready(count), None).ok()?;
        eprintln!("inputd: {count} input devices are handed to a program that holds the screens");
        self.clients.push(client);
        Some(id)
    }

    /// Whether `holder` is a holder of the screens. If that cannot be
    /// asked, nobody is one.
    fn holds(&self, holder: &D::Holder) -> bool {
        self.devices.holds(holder).unwrap_or_else(|err| {
            eprintln!("inputd: cannot tell who holds the screens: {err}");
            false
        })
    }

    /// Hands `client` the device `number`: its file, opened for this
    /// program and tied to its holder. A device that went meanwhile, or
    /// cannot be opened or tied, is passed over, with a line in the log
    /// unless it is simply gone.
    fn hand(devices: &D, client: &mut Client<P, D::Holder>, number: u32) -> Result<(), Left> {
        let tied = devices.open(number).and_then(|file| {
            devices.tie(&file, &client.holder)?;
            Ok((devices.kinds(&file)?, file))
        });
        let (kinds, file) = match tied {
            Ok(tied) => tied,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                eprintln!("inputd: input device {number} is not handed out: {err}");
                return Ok(());
            }
        };
        client.peer.send(Record::device(number, kinds), Some(&file)).map_err(|_| Left)?;
        client.has.insert(number);
        Ok(())
        // inputd's copy of the description closes here: the program's is
        // the only one.
    }

    /// The device `number` came: every program is handed it.
    pub(crate) fn added(&mut self, number: u32) {
        let devices = &self.devices;
        self.clients.retain_mut(|client| {
            client.has.contains(&number) || Self::hand(devices, client, number).is_ok()
        });
    }

    /// The device `number` went: every program that had it is told.
    pub(crate) fn removed(&mut self, number: u32) {
        self.clients.retain_mut(|client| {
            !client.has.remove(&number) || client.peer.send(Record::gone(number), None).is_ok()
        });
    }

    /// What came and went was lost, more than the kernel keeps: the
    /// devices are listed, and every program is told of each it has that
    /// is gone and handed each it has not.
    pub(crate) fn relist(&mut self) {
        let listed: BTreeSet<u32> = match self.devices.list() {
            Ok(listed) => listed.into_iter().collect(),
            Err(err) => {
                eprintln!("inputd: cannot list the input devices: {err}");
                return;
            }
        };
        let had: BTreeSet<u32> =
            self.clients.iter().flat_map(|client| client.has.iter().copied()).collect();
        for &number in had.difference(&listed) {
            self.removed(number);
        }
        for &number in &listed {
            self.added(number);
        }
    }

    /// The program known by `id` has gone, or broke the exchange: it is
    /// served no longer, and inputd's copy of its holder closes.
    pub(crate) fn leave(&mut self, id: u64) {
        self.clients.retain(|client| client.id != id);
    }
}

/// How a request reaches a file of the kernel's.
struct Raw<'a>(&'a File);

impl Control for Raw<'_> {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        kernel::control(self.0, request, argument, buffer).map_err(kernel::io_error)
    }
}

/// The machine's own devices.
struct Machine {
    /// _/devices/input/control_, which is also read for what comes and
    /// goes.
    control: File,
}

impl Devices for Machine {
    type File = File;
    type Holder = OwnedFd;

    fn list(&self) -> io::Result<Vec<u32>> {
        input::devices(&Raw(&self.control))
    }

    // A program waits for its devices with poll(2), so a read of one
    // never waits; and none is passed on to a program this one starts.
    fn open(&self, number: u32) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(format!("{DEVICES}/{number}"))
    }

    fn kinds(&self, file: &File) -> io::Result<Kinds> {
        Device::new(Raw(file)).kinds()
    }

    fn tie(&self, file: &File, holder: &OwnedFd) -> io::Result<()> {
        Device::new(Raw(file)).tie(kernel::slot_of(holder))
    }

    // The kernel says so: only a holder of the screens can be tied to.
    // The description that was tied is closed again at once.
    fn holds(&self, holder: &OwnedFd) -> io::Result<bool> {
        let probe = OpenOptions::new().read(true).custom_flags(libc::O_CLOEXEC).open(KEYBOARD)?;
        match self.tie(&probe, holder) {
            Ok(()) => Ok(true),
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => Ok(false),
            Err(err) => Err(err),
        }
    }
}

/// A program's connection.
struct Wire(UnixStream);

impl Peer<File> for Wire {
    // The socket does not wait: a program that leaves what it is sent
    // unread until the socket is full fails here, and is dropped.
    fn send(&mut self, record: Record, file: Option<&File>) -> io::Result<()> {
        let file = file.map(AsFd::as_fd);
        net::send_with_fds(&self.0, &record.to_bytes(), file.as_slice())
    }
}

/// A program that connected and has not yet said what it holds.
struct Waiting {
    stream: UnixStream,
    /// The bytes of its first record that have come.
    said: Vec<u8>,
    /// The descriptor that came with them.
    holder: Option<OwnedFd>,
    /// When it is too late.
    until: Instant,
}

/// What a connection that is read came to.
enum Came {
    /// Its first record, whole.
    Said(Record),
    /// Not all of it yet.
    Wait,
    /// It closed, or the read failed.
    Gone,
}

impl Waiting {
    /// Reads what the program sent of its first record.
    fn read(&mut self) -> Came {
        let mut bytes = [0u8; Record::SIZE];
        let room = Record::SIZE - self.said.len();
        let mut passed = Vec::new();
        match net::receive_with_fds(&self.stream, &mut bytes[..room], &mut passed, 1) {
            Ok(0) => return Came::Gone,
            Ok(n) => self.said.extend_from_slice(&bytes[..n]),
            Err(err)
                if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) =>
            {
                return Came::Wait;
            }
            Err(_) => return Came::Gone,
        }
        self.holder = self.holder.take().or(passed.pop());
        match <&[u8; Record::SIZE]>::try_from(self.said.as_slice()) {
            Ok(whole) => Came::Said(Record::from_bytes(whole)),
            Err(_) => Came::Wait,
        }
    }
}

/// inputd as it runs: its socket, the programs still to say what they
/// hold, and the broker of those that did.
struct Inputd {
    listener: UnixListener,
    waiting: Vec<Waiting>,
    broker: Broker<Machine, Wire>,
}

impl Inputd {
    /// Takes in the devices that came and went.
    fn changes(&mut self) {
        let mut bytes = [0u8; 16 * InputChange::SIZE];
        loop {
            let read = (&self.broker.devices().control).read(&mut bytes);
            let n = match read {
                Ok(0) => return,
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(err) => {
                    eprintln!("inputd: cannot read which devices came and went: {err}");
                    return;
                }
            };
            for change in input::wire::changes(&bytes[..n]) {
                match change.what {
                    DEVICE_ADDED => self.broker.added(change.device),
                    DEVICE_REMOVED => self.broker.removed(change.device),
                    // DEVICES_LOST, and whatever a later kernel adds.
                    _ => self.broker.relist(),
                }
            }
        }
    }

    /// Takes every connection that waits to be taken, while there is
    /// room for one.
    fn accept(&mut self, now: Instant) {
        while self.waiting.len() + self.broker.peers().count() < CLIENTS_MAX {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(err) => {
                    eprintln!("inputd: a program could not connect: {err}");
                    return;
                }
            };
            if stream.set_nonblocking(true).is_ok() {
                let until = now + HELLO_TIME;
                self.waiting.push(Waiting { stream, said: Vec::new(), holder: None, until });
            }
        }
    }

    /// Reads the first record of the waiting program at `at`, and has
    /// the broker answer it once it is whole. Returns whether the
    /// program still waits.
    fn hello(&mut self, at: usize) -> bool {
        let first = match self.waiting[at].read() {
            Came::Said(first) => first,
            Came::Wait => return true,
            Came::Gone => {
                self.waiting.swap_remove(at);
                return false;
            }
        };
        let waiting = self.waiting.swap_remove(at);
        // The broker's log says whether it is served, or why not.
        let _ = self.broker.join(Wire(waiting.stream), first, waiting.holder);
        false
    }

    /// Whether the served program `peer` closed its connection, or said
    /// something, which it may not, and which the log says.
    fn left(peer: &Wire) -> bool {
        let mut byte = [0u8; 1];
        match (&peer.0).read(&mut byte) {
            Err(err) => {
                !matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
            }
            Ok(0) => true,
            Ok(_) => {
                eprintln!("inputd: a program said more than its first record, and is disconnected");
                true
            }
        }
    }

    /// Serves until inputd is told to end.
    fn run(&mut self) -> io::Result<()> {
        while !STOP.is_set() {
            let now = Instant::now();
            let served: Vec<u64> = self.broker.peers().map(|(id, _)| id).collect();
            let events = {
                let room = self.waiting.len() + served.len() < CLIENTS_MAX;
                let mut fds = vec![
                    PollFd::new(self.broker.devices().control.as_fd(), libc::POLLIN),
                    PollFd::optional(room.then(|| self.listener.as_fd()), libc::POLLIN),
                ];
                fds.extend(
                    self.waiting.iter().map(|w| PollFd::new(w.stream.as_fd(), libc::POLLIN)),
                );
                fds.extend(
                    self.broker.peers().map(|(_, peer)| PollFd::new(peer.0.as_fd(), libc::POLLIN)),
                );
                // Only a program that is still to say what it holds is
                // waited for by the clock.
                let until = self.waiting.iter().map(|waiting| waiting.until).min();
                let timeout = until.map(|until| until.saturating_duration_since(now));
                match poll::poll(&mut fds, timeout) {
                    Ok(_) => {}
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(err),
                }
                fds.iter().map(PollFd::revents).collect::<Vec<_>>()
            };
            if events[0] != 0 {
                self.changes();
            }
            let (waiting, peers) = events[2..].split_at(self.waiting.len());
            // The served programs that left, before any is taken in.
            for (&id, _) in served.iter().zip(peers).filter(|(_, revents)| **revents != 0) {
                let gone = self
                    .broker
                    .peers()
                    .find(|(peer, _)| *peer == id)
                    .is_some_and(|(_, peer)| Self::left(peer));
                if gone {
                    self.broker.leave(id);
                }
            }
            // From the last, so that one taken out moves none still to
            // be looked at.
            let now = Instant::now();
            for at in (0..waiting.len()).rev() {
                let still = waiting[at] == 0 || self.hello(at);
                if still && self.waiting[at].until <= now {
                    self.waiting.swap_remove(at);
                }
            }
            if events[1] != 0 {
                self.accept(now);
            }
        }
        Ok(())
    }
}

/// A socket listening at `path`, which any program may connect to
/// (ADR 0713): a desktop runs as the person it is for. Only root opens
/// the display layer's files, so a holder of the screens, which is what
/// a program is handed the devices for, is one root or consoled(8) gave
/// it.
fn listen(path: &str) -> io::Result<UnixListener> {
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// How inputd is run, and what `--help` says after it.
const USAGE: &str = "usage: inputd";
const HELP: &str = "Hands each keyboard, mouse and tablet to the program that holds the
screens, and tells it of each one that comes or goes. Spark runs it as
the inputd service: see inputd(8).";

/// Serves until it is told to stop. An error says what could not be
/// done, and on what.
fn serve() -> Result<(), String> {
    // A program that goes away must not end inputd: the write to it
    // fails instead.
    let _ = signal::ignore(libc::SIGPIPE);
    let _ = signal::catch(libc::SIGTERM, &STOP, 0);
    let control = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(CONTROL)
        .map_err(|err| format!("{CONTROL}: {err}"))?;
    let listener = listen(service::SOCKET).map_err(|err| format!("{}: {err}", service::SOCKET))?;
    eprintln!("inputd: handing the input devices out at {}", service::SOCKET);
    let broker = Broker::new(Machine { control });
    let mut inputd = Inputd { listener, waiting: Vec::new(), broker };
    inputd.run().map_err(|err| err.to_string())?;
    let _ = std::fs::remove_file(service::SOCKET);
    Ok(())
}

/// Serves when run with no argument, as Spark runs it.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => match serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => {
                eprintln!("inputd: {why}");
                ExitCode::FAILURE
            }
        },
        ["--help" | "-h"] => {
            println!("{USAGE}\n\n{HELP}");
            ExitCode::SUCCESS
        }
        ["--version"] => {
            println!("inputd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
