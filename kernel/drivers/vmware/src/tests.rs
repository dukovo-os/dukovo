// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The backdoor and the guest RPC against a simulated host, which keeps
//! its channels as VMware's hypervisors do: it checks each call's magic,
//! channel and cookies, takes a message's length and then its bytes, and
//! queues its own messages for the guest to read.

extern crate std;

use std::collections::VecDeque;
use std::string::{String, ToString};
use std::vec::Vec;

use super::*;

/// A channel the host opened.
struct Open {
    protocol: u32,
    cookies: (u32, u32),
    /// The length the guest said its next message has.
    length: Option<usize>,
    /// The host's message waiting to be read, and its number.
    waiting: Option<(Vec<u8>, u16)>,
}

/// A simulated VMware host.
#[derive(Default)]
struct Host {
    /// Whether it gives the guest RPC at all.
    rpc: bool,
    /// Whether it gives every register back unchanged, as a bad emulator.
    echo: bool,
    channels: Vec<Option<Open>>,
    /// The host's commands waiting for the toolbox.
    commands: VecDeque<String>,
    /// The toolbox's answers to them, and its empty pings.
    answers: Vec<String>,
    /// The guest's RPCI requests.
    requests: Vec<String>,
    /// Channels open now.
    open: usize,
    /// Whether to refuse the next message's bytes.
    refuse: bool,
}

impl Host {
    fn new() -> Self {
        Host { rpc: true, ..Host::default() }
    }

    fn channel(&mut self, regs: &Regs, port: u16) -> Option<&mut Open> {
        if regs.dx & 0xffff != u32::from(port) {
            return None;
        }
        let number = (regs.dx >> 16) as usize;
        let open = self.channels.get_mut(number)?.as_mut()?;
        let cookies = if port == PORT { (regs.si, regs.di) } else { (open.cookies.0, regs.di) };
        (cookies == open.cookies).then_some(open)
    }

    /// Takes a whole message from the guest on channel `number`.
    fn message(&mut self, number: usize, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).to_string();
        let open = self.channels[number].as_mut().unwrap();
        if open.protocol == RPCI {
            open.waiting = Some((b"1 ".to_vec(), 7));
            self.requests.push(text);
        } else {
            self.answers.push(text);
        }
    }
}

impl Backdoor for Host {
    fn call(&mut self, regs: &mut Regs) {
        if self.echo {
            return;
        }
        assert_eq!(regs.ax, MAGIC);
        let failed = |regs: &mut Regs| regs.cx = 0;
        match regs.cx & 0xffff {
            command::GET_VERSION => {
                regs.ax = 6;
                regs.bx = MAGIC;
            }
            command::GET_SPEED => regs.ax = 2400,
            command::RPC if !self.rpc => regs.ax = u32::MAX,
            command::RPC => match high(regs.cx) {
                step::OPEN => {
                    assert_eq!(regs.dx, u32::from(PORT));
                    let number = self.channels.len() as u32;
                    let cookies = (0x1000 + number, 0x2000 + number);
                    self.channels.push(Some(Open {
                        protocol: regs.bx & !FLAG_COOKIE,
                        cookies,
                        length: None,
                        waiting: None,
                    }));
                    self.open += 1;
                    regs.cx = REPLY_SUCCESS << 16;
                    regs.dx = number << 16;
                    (regs.si, regs.di) = cookies;
                }
                step::SET_LENGTH => {
                    let length = regs.bx as usize;
                    let number = (regs.dx >> 16) as usize;
                    let Some(open) = self.channel(regs, PORT) else { return failed(regs) };
                    open.length = Some(length);
                    regs.cx = REPLY_SUCCESS << 16;
                    if length == 0 {
                        self.message(number, b"");
                    }
                }
                step::GET_LENGTH => {
                    let number = (regs.dx >> 16) as usize;
                    let Some(open) = self.channel(regs, PORT) else { return failed(regs) };
                    let protocol = open.protocol;
                    if protocol == TCLO
                        && open.waiting.is_none()
                        && let Some(command) = self.commands.pop_front()
                    {
                        self.channels[number].as_mut().unwrap().waiting =
                            Some((command.into_bytes(), 3));
                    }
                    let open = self.channels[number].as_mut().unwrap();
                    match &open.waiting {
                        Some((bytes, id)) => {
                            regs.cx = (REPLY_SUCCESS | REPLY_RECEIVE) << 16;
                            regs.bx = bytes.len() as u32;
                            regs.dx = u32::from(*id) << 16;
                        }
                        None => regs.cx = REPLY_SUCCESS << 16,
                    }
                }
                step::GET_END => {
                    let Some(open) = self.channel(regs, PORT) else { return failed(regs) };
                    assert_eq!(open.waiting.as_ref().map(|w| w.1), Some(regs.bx as u16));
                    open.waiting = None;
                    regs.cx = REPLY_SUCCESS << 16;
                }
                step::CLOSE => {
                    let number = (regs.dx >> 16) as usize;
                    if self.channel(regs, PORT).is_none() {
                        return failed(regs);
                    }
                    self.channels[number] = None;
                    self.open -= 1;
                    regs.cx = REPLY_SUCCESS << 16;
                }
                other => panic!("step {other}"),
            },
            other => panic!("command {other:#x}"),
        }
    }

    fn send(&mut self, regs: &mut Regs, bytes: &[u8]) {
        assert_eq!((regs.ax, regs.bx), (MAGIC, ENHANCED));
        let number = (regs.dx >> 16) as usize;
        let refuse = core::mem::take(&mut self.refuse);
        let Some(open) = self.channel(regs, PORT_HIGH_BANDWIDTH) else { return };
        assert_eq!(regs.bp, open.cookies.0);
        assert_eq!(open.length.take(), Some(bytes.len()));
        assert_eq!(regs.cx as usize, bytes.len());
        if refuse {
            regs.bx = 0;
            return;
        }
        self.message(number, bytes);
    }

    fn receive(&mut self, regs: &mut Regs, bytes: &mut [u8]) {
        assert_eq!((regs.ax, regs.bx), (MAGIC, ENHANCED));
        assert_eq!(regs.cx as usize, bytes.len());
        let number = (regs.dx >> 16) as usize;
        let open = self.channels[number].as_ref().unwrap();
        assert_eq!((regs.si, regs.bp), open.cookies);
        let (message, _) = open.waiting.as_ref().unwrap();
        bytes.copy_from_slice(&message[..bytes.len()]);
    }
}

/// A guest up `uptime` hundredths of a second, at `address`.
struct Machine {
    uptime: u64,
    address: Option<[u8; 4]>,
}

impl Guest for Machine {
    fn uptime(&self) -> u64 {
        self.uptime
    }

    fn address(&self) -> Option<[u8; 4]> {
        self.address
    }
}

const MACHINE: Machine = Machine { uptime: 4200, address: Some([192, 168, 1, 20]) };

#[test]
fn the_backdoor_is_there_when_the_version_comes_with_the_magic() {
    let mut host = Host::new();
    assert_eq!(present(&mut host), Some(6));
    // An emulator that gives the registers back unchanged is not VMware.
    host.echo = true;
    assert_eq!(present(&mut host), None);
}

#[test]
fn a_request_goes_on_a_channel_of_its_own_and_its_answer_comes_back() {
    let mut host = Host::new();
    let mut answer = [0u8; 16];
    let came = rpci(&mut host, b"tools.set.version 2147483647", &mut answer).unwrap();
    assert!(worked(&answer[..came]));
    assert_eq!(host.requests, ["tools.set.version 2147483647"]);
    // The channel was closed.
    assert_eq!(host.open, 0);
    // An answer longer than the room is cut.
    let mut short = [0u8; 1];
    assert_eq!(rpci(&mut host, b"ping", &mut short), Ok(1));
    assert_eq!(host.open, 0);
}

#[test]
fn a_host_without_the_rpc_opens_no_channel() {
    let mut host = Host { rpc: false, ..Host::new() };
    assert_eq!(rpci(&mut host, b"x", &mut [0; 4]), Err(Error::Open));
    let mut toolbox = Toolbox::new();
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick, Tick { event: Event::NoChannel(Error::Open), again_ms: REOPEN_MS });
}

#[test]
fn the_toolbox_says_hello_then_pings_while_the_host_has_nothing() {
    let mut host = Host::new();
    let mut toolbox = Toolbox::new();
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick, Tick { event: Event::Opened, again_ms: POLL_MS });
    assert_eq!(host.answers, ["OK ATR toolbox"]);
    // The uptime was told on the first tick.
    assert_eq!(host.requests, ["SetGuestInfo  7 4200"]);
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick, Tick { event: Event::Nothing, again_ms: POLL_MS });
    // An empty message: nothing to answer.
    assert_eq!(host.answers, ["OK ATR toolbox", ""]);
    // Its channel stays open; each request's was closed.
    assert_eq!(host.open, 1);
}

#[test]
fn the_toolbox_answers_the_hosts_commands() {
    let mut host = Host::new();
    let mut toolbox = Toolbox::new();
    toolbox.tick(&mut host, &MACHINE);
    host.requests.clear();
    for command in ["ping", "reset", "Capabilities_Register", "Set_Option broadcastIP 1", "Foo"] {
        host.commands.push_back(command.into());
    }
    let mut events = Vec::new();
    loop {
        let tick = toolbox.tick(&mut host, &MACHINE);
        if tick.again_ms != 0 {
            break;
        }
        events.push(tick.event);
    }
    assert_eq!(
        events,
        [Event::Nothing, Event::Nothing, Event::Registered, Event::Nothing, Event::Unknown]
    );
    assert_eq!(
        host.answers[1..],
        // The ping first: the first tick found no command.
        ["", "OK ", "OK ATR toolbox", "OK ", "OK ", "ERROR Unknown command"]
    );
    assert_eq!(
        host.requests,
        [
            "vmx.capability.unified_loop toolbox",
            "tools.capability.statechange ",
            "tools.set.version 2147483647",
            // Told again once registered, on the next tick.
            "SetGuestInfo  7 4200",
            "info-set guestinfo.ip 192.168.1.20",
        ]
    );
}

#[test]
fn the_hosts_buttons_power_off_and_restart_after_saying_so() {
    let mut host = Host::new();
    let mut toolbox = Toolbox::new();
    toolbox.tick(&mut host, &MACHINE);
    host.requests.clear();
    host.commands.push_back("OS_Halt".into());
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick, Tick { event: Event::Asked(Asked::PowerOff), again_ms: 0 });
    host.commands.push_back("OS_Reboot".into());
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick.event, Event::Asked(Asked::Restart));
    host.commands.push_back("OS_Suspend".into());
    host.commands.push_back("OS_Resume".into());
    host.commands.push_back("OS_PowerOn".into());
    for _ in 0..3 {
        assert_eq!(toolbox.tick(&mut host, &MACHINE).event, Event::Nothing);
    }
    assert_eq!(
        host.requests,
        [
            "tools.os.statechange.status 1 1",
            "tools.os.statechange.status 1 2",
            "tools.os.statechange.status 1 5",
            "tools.os.statechange.status 1 4",
            "tools.os.statechange.status 1 3",
        ]
    );
    assert_eq!(host.answers[1..], ["", "OK ", "OK ", "OK ", "OK ", "OK "]);
}

#[test]
fn the_uptime_is_told_every_fifteen_seconds() {
    let mut host = Host::new();
    let mut toolbox = Toolbox::new();
    let mut machine = Machine { uptime: 100, address: None };
    toolbox.tick(&mut host, &machine);
    machine.uptime = 1599;
    toolbox.tick(&mut host, &machine);
    machine.uptime = 1600;
    toolbox.tick(&mut host, &machine);
    assert_eq!(host.requests, ["SetGuestInfo  7 100", "SetGuestInfo  7 1600"]);
    // Without an address, broadcastIP is answered with an error.
    host.commands.push_back("Set_Option broadcastIP 1".into());
    toolbox.tick(&mut host, &machine);
    assert_eq!(host.answers.last().unwrap(), "ERROR Unable to find guest IP address");
}

#[test]
fn a_failed_step_closes_the_channel_and_the_next_tick_opens_it_again() {
    let mut host = Host::new();
    let mut toolbox = Toolbox::new();
    toolbox.tick(&mut host, &MACHINE);
    host.commands.push_back("ping".into());
    // The host refuses the answer's bytes.
    host.refuse = true;
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick.event, Event::Failed(Error::Step("take the message")));
    assert_eq!(host.open, 0);
    let tick = toolbox.tick(&mut host, &MACHINE);
    assert_eq!(tick.event, Event::Opened);
    assert_eq!(host.answers.last().unwrap(), "OK ATR toolbox");
    assert_eq!(host.open, 1);
}
