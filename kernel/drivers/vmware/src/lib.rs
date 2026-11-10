// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMware's backdoor and guest RPC (ADR 0424): how a guest of VMware's
//! hypervisors, which give no device for it, tells the host its tools
//! run, hears the host's Shut Down Guest and Restart Guest, and says how
//! long it has been up and at which address.
//!
//! The *backdoor* is an I/O port, 0x5658, that VMware's hypervisors answer
//! themselves: `in eax, dx` with [`MAGIC`] in EAX and a command in CX does
//! the command, its arguments and results in the other registers.
//! [`present`] asks for the backdoor's version, which a hypervisor that
//! has one answers with [`MAGIC`] in EBX.
//!
//! The *guest RPC* is the backdoor's command 0x1e: it opens a channel,
//! sends a message's length and then the message, asks how long the host's
//! next message is, reads it, and closes the channel ([`Channel`]). The
//! bytes go as strings (`rep outsb` and `rep insb`) through the second
//! port, 0x5659, the *high-bandwidth* backdoor. A channel speaks one of
//! two protocols:
//!
//! - *RPCI*, the guest's requests, each a message out and its answer back
//!   ([`rpci`]): `tools.set.version 2147483647`, which says the guest's
//!   tools are not VMware's own, or `SetGuestInfo  7 <uptime>`;
//! - *TCLO*, the host's commands, which the guest polls for each second
//!   ([`Toolbox`]): it sends its answer to the last command, or an empty
//!   message when there was none, then reads the next. `OS_Halt` and
//!   `OS_Reboot` are the host's buttons to shut the guest down and to
//!   restart it.
//!
//! The registers each call uses, the commands and the answers follow
//! OpenBSD's vmt(4) (ISC license, credits/openbsd-vmt.md).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use core::fmt::{self, Write};

/// What EAX holds for every backdoor call, and what EBX holds after the
/// version is asked: "VMXh".
pub const MAGIC: u32 = 0x564d_5868;
/// The backdoor's port.
pub const PORT: u16 = 0x5658;
/// The high-bandwidth backdoor's port, which strings go through.
pub const PORT_HIGH_BANDWIDTH: u16 = 0x5659;

/// The backdoor's commands, in CX's low half.
mod command {
    /// The processor's speed, in MHz: what a host that only gives
    /// registers back cannot answer.
    pub const GET_SPEED: u32 = 0x01;
    /// The backdoor's version, with [`super::MAGIC`] in EBX.
    pub const GET_VERSION: u32 = 0x0a;
    /// The guest RPC: its step in CX's high half.
    pub const RPC: u32 = 0x1e;
}

/// The guest RPC's steps, in CX's high half.
mod step {
    pub const OPEN: u32 = 0;
    pub const SET_LENGTH: u32 = 1;
    pub const GET_LENGTH: u32 = 3;
    pub const GET_END: u32 = 5;
    pub const CLOSE: u32 = 6;
}

/// The protocol of a channel of the guest's requests: "RPCI".
pub const RPCI: u32 = 0x4943_5052;
/// The protocol of a channel of the host's commands: "TCLO".
pub const TCLO: u32 = 0x4f4c_4354;
/// Asks for a channel with cookies, which the host checks on each call.
const FLAG_COOKIE: u32 = 0x8000_0000;
/// In EBX for a string through the high-bandwidth backdoor, and back.
const ENHANCED: u32 = 0x1_0000;

/// The flags of an answer, in CX's high half: the step worked.
const REPLY_SUCCESS: u32 = 0x0001;
/// The host has a message for the guest to read.
const REPLY_RECEIVE: u32 = 0x0002;

/// The longest message from the host the toolbox reads; the rest of a
/// longer one is not read.
pub const LONGEST: usize = 256;

/// What the guest's tools call themselves: not VMware's own, so that the
/// host never offers to upgrade them.
pub const UNMANAGED: u32 = 0x7fff_ffff;

/// The registers of a backdoor call: what the caller sets, and after the
/// call what the hypervisor left in them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    /// EAX: [`MAGIC`] in, a result out.
    pub ax: u32,
    /// EBX: an argument in, a result out.
    pub bx: u32,
    /// ECX: the command in its low half and the step in its high half,
    /// or a length; flags out.
    pub cx: u32,
    /// EDX: the port in its low half and the channel in its high half.
    pub dx: u32,
    /// ESI: a cookie.
    pub si: u32,
    /// EDI: a cookie.
    pub di: u32,
    /// EBP: a cookie, for the high-bandwidth backdoor.
    pub bp: u32,
}

/// The instructions that reach the backdoor, which the architecture gives.
pub trait Backdoor {
    /// `in eax, dx`, the registers set from `regs` and read back into it.
    fn call(&mut self, regs: &mut Regs);
    /// `rep outsb`: sends `bytes`, RSI at them and RCX their length
    /// whatever `regs` says, the other registers set from `regs`, and all
    /// read back into it.
    fn send(&mut self, regs: &mut Regs, bytes: &[u8]);
    /// `rep insb`: reads `bytes.len()` bytes into `bytes`, RDI at them and
    /// RCX their length whatever `regs` says, the other registers set from
    /// `regs`, and all read back into it.
    fn receive(&mut self, regs: &mut Regs, bytes: &mut [u8]);
}

/// The backdoor's version, if the hypervisor answers as VMware's do: the
/// version asked for comes with [`MAGIC`] in EBX, and the processor's
/// speed does not come back as [`MAGIC`], which an emulator that gave the
/// registers back unchanged would do.
pub fn present(door: &mut impl Backdoor) -> Option<u32> {
    let mut version = Regs {
        ax: MAGIC,
        bx: !MAGIC,
        cx: command::GET_VERSION | 0xffff << 16,
        dx: u32::from(PORT),
        ..Regs::default()
    };
    door.call(&mut version);
    if version.ax == u32::MAX || version.bx != MAGIC {
        return None;
    }
    let mut speed =
        Regs { ax: MAGIC, cx: command::GET_SPEED, dx: u32::from(PORT), ..Regs::default() };
    door.call(&mut speed);
    (speed.ax != MAGIC).then_some(version.ax)
}

/// Why a guest RPC failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The host opened no channel: it gives no guest RPC, or no more
    /// channels.
    Open,
    /// The message is longer than the RPC can say.
    TooLong,
    /// A step the host did not take: which.
    Step(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Open => f.write_str("the host opened no channel"),
            Error::TooLong => f.write_str("the message is too long"),
            Error::Step(step) => write!(f, "the host did not {step}"),
        }
    }
}

/// CX for the guest RPC's `step`.
const fn rpc(step: u32) -> u32 {
    command::RPC | step << 16
}

/// The high half of a register.
const fn high(word: u32) -> u32 {
    word >> 16
}

/// An open channel of the guest RPC: its number, and the cookies the host
/// checks on each call.
#[derive(Debug, PartialEq, Eq)]
pub struct Channel {
    number: u16,
    cookies: (u32, u32),
}

impl Channel {
    /// Opens a channel of `protocol`, [`RPCI`] or [`TCLO`].
    pub fn open(door: &mut impl Backdoor, protocol: u32) -> Result<Channel, Error> {
        let mut regs = Regs {
            ax: MAGIC,
            bx: protocol | FLAG_COOKIE,
            cx: rpc(step::OPEN),
            dx: u32::from(PORT),
            ..Regs::default()
        };
        door.call(&mut regs);
        if high(regs.cx) != REPLY_SUCCESS || regs.dx & 0xffff != 0 {
            return Err(Error::Open);
        }
        Ok(Channel { number: (regs.dx >> 16) as u16, cookies: (regs.si, regs.di) })
    }

    /// EDX for a call on this channel through `port`.
    fn port(&self, port: u16) -> u32 {
        u32::from(port) | u32::from(self.number) << 16
    }

    /// The registers of `step` on this channel, with `bx`.
    fn step(&self, step: u32, bx: u32) -> Regs {
        Regs {
            ax: MAGIC,
            bx,
            cx: rpc(step),
            dx: self.port(PORT),
            si: self.cookies.0,
            di: self.cookies.1,
            ..Regs::default()
        }
    }

    /// Sends `message`: its length, then, unless it is empty, its bytes.
    /// An empty message is the toolbox's ping.
    pub fn send(&self, door: &mut impl Backdoor, message: &[u8]) -> Result<(), Error> {
        let length = u32::try_from(message.len()).map_err(|_| Error::TooLong)?;
        let mut regs = self.step(step::SET_LENGTH, length);
        door.call(&mut regs);
        if high(regs.cx) & REPLY_SUCCESS == 0 {
            return Err(Error::Step("take the message's length"));
        }
        if message.is_empty() {
            return Ok(());
        }
        let mut regs = Regs {
            ax: MAGIC,
            bx: ENHANCED,
            cx: length,
            dx: self.port(PORT_HIGH_BANDWIDTH),
            di: self.cookies.1,
            bp: self.cookies.0,
            ..Regs::default()
        };
        door.send(&mut regs, message);
        if regs.bx != ENHANCED {
            return Err(Error::Step("take the message"));
        }
        Ok(())
    }

    /// The host's next message: its length and the number to read it
    /// with, or `None` if it has none.
    pub fn waiting(&self, door: &mut impl Backdoor) -> Result<Option<(usize, u16)>, Error> {
        let mut regs = self.step(step::GET_LENGTH, 0);
        door.call(&mut regs);
        if high(regs.cx) & REPLY_SUCCESS == 0 {
            return Err(Error::Step("say how long its message is"));
        }
        if high(regs.cx) & REPLY_RECEIVE == 0 {
            return Ok(None);
        }
        Ok(Some((regs.bx as usize, (regs.dx >> 16) as u16)))
    }

    /// Reads the host's message, numbered `number` by [`Self::waiting`],
    /// into `bytes`, which is as long as the message or shorter, and tells
    /// the host it came.
    pub fn receive(
        &self,
        door: &mut impl Backdoor,
        bytes: &mut [u8],
        number: u16,
    ) -> Result<(), Error> {
        if !bytes.is_empty() {
            let mut regs = Regs {
                ax: MAGIC,
                bx: ENHANCED,
                cx: u32::try_from(bytes.len()).map_err(|_| Error::TooLong)?,
                dx: self.port(PORT_HIGH_BANDWIDTH),
                si: self.cookies.0,
                bp: self.cookies.1,
                ..Regs::default()
            };
            door.receive(&mut regs, bytes);
            if regs.bx != ENHANCED {
                return Err(Error::Step("give its message"));
            }
        }
        let mut regs = self.step(step::GET_END, u32::from(number));
        door.call(&mut regs);
        if high(regs.cx) == 0 {
            return Err(Error::Step("take the end of its message"));
        }
        Ok(())
    }

    /// Closes the channel.
    pub fn close(self, door: &mut impl Backdoor) -> Result<(), Error> {
        let mut regs = self.step(step::CLOSE, 0);
        door.call(&mut regs);
        if high(regs.cx) == 0 || regs.cx & 0xffff != 0 {
            return Err(Error::Step("close the channel"));
        }
        Ok(())
    }
}

/// Sends the request `request` on an RPCI channel of its own, and reads
/// the host's answer into `answer`, as much of it as fits: how many bytes
/// came. An answer that starts `1 ` says it worked ([`worked`]).
pub fn rpci(door: &mut impl Backdoor, request: &[u8], answer: &mut [u8]) -> Result<usize, Error> {
    let channel = Channel::open(door, RPCI)?;
    let came = ask(door, &channel, request, answer);
    let closed = channel.close(door);
    let came = came?;
    closed.map(|()| came)
}

/// Sends `request` on `channel` and reads the answer into `answer`.
fn ask(
    door: &mut impl Backdoor,
    channel: &Channel,
    request: &[u8],
    answer: &mut [u8],
) -> Result<usize, Error> {
    channel.send(door, request)?;
    let Some((length, number)) = channel.waiting(door)? else { return Ok(0) };
    let length = length.min(answer.len());
    channel.receive(door, &mut answer[..length], number)?;
    Ok(length)
}

/// Whether the host's answer to an RPCI request says it worked.
pub fn worked(answer: &[u8]) -> bool {
    answer.starts_with(b"1 ")
}

/// What the host asked the guest to do, which the kernel then does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Shut down and power off: the host's Shut Down Guest.
    PowerOff,
    /// Shut down and restart: Restart Guest.
    Restart,
}

/// What the guest is, which the toolbox tells the host.
pub trait Guest {
    /// How long the machine has been up, in hundredths of a second.
    fn uptime(&self) -> u64;
    /// Its first IPv4 address, past the loopback interface's.
    fn address(&self) -> Option<[u8; 4]>;
}

/// What a tick of the toolbox did, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Nothing to tell.
    Nothing,
    /// The channel of the host's commands opened: the host knows the
    /// toolbox runs.
    Opened,
    /// The channel of the host's commands could not be opened.
    NoChannel(Error),
    /// The host registered the toolbox's capabilities.
    Registered,
    /// The host asked for this: the kernel does it now.
    Asked(Asked),
    /// The host sent a command the toolbox does not do: it said so.
    Unknown,
    /// A step failed: the channel is opened again on the next tick.
    Failed(Error),
}

/// What one tick of the toolbox did, and when the next is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// What it did.
    pub event: Event,
    /// Milliseconds until the next tick: none when the host may have
    /// queued more commands.
    pub again_ms: u32,
}

/// How long the toolbox waits between polls of the host's commands.
pub const POLL_MS: u32 = 1000;
/// How long it waits to open the channel again after it could not.
pub const REOPEN_MS: u32 = 15_000;
/// How often it tells the host the machine's uptime, in hundredths of a
/// second, as VMware's tools do.
pub const UPTIME_EVERY: u64 = 1500;

/// The guest's toolbox: the channel of the host's commands, polled each
/// [`POLL_MS`] by [`Self::tick`].
#[derive(Debug)]
pub struct Toolbox {
    /// The channel of the host's commands, while open.
    channel: Option<Channel>,
    /// Whether the last poll found no command, so that the next sends the
    /// empty message that stands for "nothing to answer".
    ping: bool,
    /// When the uptime was last told, as [`Guest::uptime`] counts.
    told: Option<u64>,
}

impl Default for Toolbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolbox {
    /// A toolbox whose channel opens on the first tick.
    pub const fn new() -> Self {
        Toolbox { channel: None, ping: false, told: None }
    }

    /// Opens the channel if it is not, answers the last command, reads
    /// the next and does it.
    pub fn tick(&mut self, door: &mut impl Backdoor, guest: &impl Guest) -> Tick {
        match self.poll(door, guest) {
            Ok(tick) => tick,
            Err(err) => {
                // Opened again, and the host told "reset", next time.
                if let Some(channel) = self.channel.take() {
                    let _ = channel.close(door);
                }
                Tick { event: Event::Failed(err), again_ms: POLL_MS }
            }
        }
    }

    fn poll(&mut self, door: &mut impl Backdoor, guest: &impl Guest) -> Result<Tick, Error> {
        let mut opened = false;
        if self.channel.is_none() {
            let channel = match Channel::open(door, TCLO) {
                Ok(channel) => channel,
                Err(err) => {
                    return Ok(Tick { event: Event::NoChannel(err), again_ms: REOPEN_MS });
                }
            };
            channel.send(door, b"OK ATR toolbox")?;
            self.channel = Some(channel);
            self.ping = false;
            opened = true;
        }
        let uptime = guest.uptime();
        if self.told.is_none_or(|told| uptime >= told + UPTIME_EVERY) {
            self.told = Some(uptime);
            tell_uptime(door, uptime)?;
        }
        let channel = self.channel.as_ref().ok_or(Error::Open)?;
        if self.ping {
            channel.send(door, b"")?;
        }
        let mut bytes = [0u8; LONGEST];
        let Some((length, number)) = channel.waiting(door)? else {
            self.ping = true;
            let event = if opened { Event::Opened } else { Event::Nothing };
            return Ok(Tick { event, again_ms: POLL_MS });
        };
        let length = length.min(LONGEST);
        channel.receive(door, &mut bytes[..length], number)?;
        self.ping = false;
        let (answer, event) = act(door, guest, &bytes[..length])?;
        channel.send(door, answer)?;
        if let Event::Registered = event {
            // The host forgets what it was told before it registered.
            self.told = None;
        }
        // The host may have queued more.
        Ok(Tick { event, again_ms: 0 })
    }
}

/// Does the host's command `command`: what to answer it, and what it was.
fn act(
    door: &mut impl Backdoor,
    guest: &impl Guest,
    command: &[u8],
) -> Result<(&'static [u8], Event), Error> {
    const OK: &[u8] = b"OK ";
    Ok(match command {
        b"reset" => (&b"OK ATR toolbox"[..], Event::Nothing),
        b"ping" => (OK, Event::Nothing),
        b"OS_Halt" => {
            state_change(door, 1)?;
            (OK, Event::Asked(Asked::PowerOff))
        }
        b"OS_Reboot" => {
            state_change(door, 2)?;
            (OK, Event::Asked(Asked::Restart))
        }
        b"OS_PowerOn" => {
            state_change(door, 3)?;
            (OK, Event::Nothing)
        }
        b"OS_Resume" => {
            state_change(door, 4)?;
            (OK, Event::Nothing)
        }
        b"OS_Suspend" => {
            state_change(door, 5)?;
            (OK, Event::Nothing)
        }
        b"Capabilities_Register" => {
            request(door, b"vmx.capability.unified_loop toolbox")?;
            // The trailing space is part of it.
            request(door, b"tools.capability.statechange ")?;
            let mut line = Line::new();
            let _ = write!(line, "tools.set.version {UNMANAGED}");
            request(door, line.bytes())?;
            (OK, Event::Registered)
        }
        b"Set_Option broadcastIP 1" => match guest.address() {
            Some([a, b, c, d]) => {
                let mut line = Line::new();
                let _ = write!(line, "info-set guestinfo.ip {a}.{b}.{c}.{d}");
                request(door, line.bytes())?;
                (OK, Event::Nothing)
            }
            None => (&b"ERROR Unable to find guest IP address"[..], Event::Nothing),
        },
        _ => (&b"ERROR Unknown command"[..], Event::Unknown),
    })
}

/// Tells the host the guest is now in `state` (1 halted, 2 restarting, 3
/// powered on, 4 resumed, 5 suspended), and that it got there.
fn state_change(door: &mut impl Backdoor, state: u8) -> Result<(), Error> {
    let mut line = Line::new();
    let _ = write!(line, "tools.os.statechange.status 1 {state}");
    request(door, line.bytes())
}

/// Tells the host the machine has been up `uptime` hundredths of a
/// second (guest info 7; the two spaces are part of it).
fn tell_uptime(door: &mut impl Backdoor, uptime: u64) -> Result<(), Error> {
    let mut line = Line::new();
    let _ = write!(line, "SetGuestInfo  7 {uptime}");
    request(door, line.bytes())
}

/// Sends the RPCI request `request`. The host's answer is not looked at:
/// a host that does not take one still takes the others.
fn request(door: &mut impl Backdoor, request: &[u8]) -> Result<(), Error> {
    let mut answer = [0u8; 16];
    rpci(door, request, &mut answer).map(|_| ())
}

/// A short request, written in place.
struct Line {
    bytes: [u8; 64],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Line { bytes: [0; 64], len: 0 }
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        let place = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        place.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
