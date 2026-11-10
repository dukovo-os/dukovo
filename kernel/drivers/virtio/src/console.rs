// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The virtio console (virtio 1.2, section 5.3): serial ports between
//! programs in the machine and programs on its host, which is how QEMU's
//! guest agent channel and cloud consoles reach a machine (ADR 0327).
//!
//! The driver asks for the multiport feature, without which a device has
//! one port and no names: the device then says which ports it has, and
//! what each is called, as messages on a pair of control queues (2 and 3),
//! and each port has a queue the device fills (`receiveq`) and one the
//! driver fills (`transmitq`): port 0's are queues 0 and 1, port `n`'s
//! `2n + 2` and `2n + 3`. Every queue must be set up before the driver
//! says it is ready, so [`Console::new`] sets up the control queues and
//! [`Console::port`] each port's, up to [`MAX_PORTS`], before
//! [`Console::start`].
//!
//! Then [`Console::events`] reads the control messages: a port added
//! (which the driver answers, as ready or not, with
//! [`Console::port_ready`]), taken away, named, made the console, or
//! opened or closed on the host's side. A program opening a port in the
//! machine is told to the host with [`Console::port_open`]. A port's data
//! come in [`BUFFER`]-byte buffers, [`QUEUE_SIZE`] of them each way, in a
//! page each way: [`Port::read`] takes what came, and gives each buffer
//! back to the device once it is read whole; [`Port::write`] copies what
//! is to go into a free buffer. Nothing here waits: every queue's
//! completions raise MSI-X entry `vector`, and the kernel's thread for the
//! device does the rest.

use radix_hw::{Dma, device_barrier};

use crate::queue::{Buffer, Queue};
use crate::{Error, Transport, feature as common_feature, status};

/// Console feature bits.
pub mod feature {
    /// The device says its console's size (`cols`, `rows`).
    pub const SIZE: u64 = 1 << 0;
    /// The device has several ports, which control messages describe.
    pub const MULTIPORT: u64 = 1 << 1;
    /// The device takes a byte written to its configuration, before any
    /// queue is set up.
    pub const EMERG_WRITE: u64 = 1 << 2;
}

/// The control messages (5.3.6.2).
pub mod event {
    /// The driver is ready for ports (driver to device).
    pub const DEVICE_READY: u16 = 0;
    /// A port was added (device to driver).
    pub const DEVICE_ADD: u16 = 1;
    /// A port was taken away (device to driver).
    pub const DEVICE_REMOVE: u16 = 2;
    /// The driver has set a port up, or failed to (driver to device).
    pub const PORT_READY: u16 = 3;
    /// A port is a console (device to driver).
    pub const CONSOLE_PORT: u16 = 4;
    /// A console's size changed (device to driver).
    pub const RESIZE: u16 = 5;
    /// A port was opened or closed, on the device's side or the driver's.
    pub const PORT_OPEN: u16 = 6;
    /// A port's name follows the message (device to driver).
    pub const PORT_NAME: u16 = 7;
}

/// The most ports the driver sets up: the device may say up to 31
/// (QEMU's default), each with two queues.
pub const MAX_PORTS: u32 = 8;
/// The buffers of a port's queue, each way.
pub const QUEUE_SIZE: u16 = 8;
/// A buffer's bytes: a port's buffers each way fill a page.
pub const BUFFER: usize = 512;
/// The longest port name the driver keeps.
pub const NAME_MAX: usize = 120;

/// The bytes of a control message: the port (4), the event (2) and a
/// value (2).
const MESSAGE: usize = 8;
/// A control buffer the device fills: a message and a port's name.
const CONTROL_BUFFER: usize = MESSAGE + NAME_MAX;
/// Where the buffers the driver sends control messages in start, in the
/// control page.
const CONTROL_TX: usize = 2048;

/// What a control message from the device says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Port `n` was added: the driver answers with
    /// [`Console::port_ready`].
    Added(u32),
    /// Port `n` was taken away.
    Removed(u32),
    /// Port `n` is called `name[..len]`.
    Named(u32, [u8; NAME_MAX], usize),
    /// Port `n` is a console.
    Console(u32),
    /// The program at the host's end of port `n` opened it (`true`) or
    /// closed it.
    HostOpen(u32, bool),
}

/// What the device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// How many ports it may have: its `max_nr_ports`, at most
    /// [`MAX_PORTS`].
    pub ports: u32,
}

/// A virtio console's control queues, set up.
pub struct Console {
    transport: Transport,
    control_rx: Queue,
    control_tx: Queue,
    /// The control buffers: those the device fills, then from
    /// [`CONTROL_TX`] those the driver sends.
    control: Dma,
    info: Info,
    features: u64,
}

/// The two queues of a port and their buffers.
pub struct Port {
    number: u32,
    rx: Queue,
    tx: Queue,
    /// The buffers: those the device fills in the first page, those the
    /// driver fills in the second.
    buffers: Dma,
    /// The buffer being read: its descriptor, where in it, and how much
    /// the device wrote.
    reading: Option<(u16, usize, usize)>,
}

/// The queue a port receives on, and the one after it, which it sends on.
fn port_queue(number: u32) -> u16 {
    if number == 0 { 0 } else { (2 * number + 2) as u16 }
}

impl Console {
    /// Sets the device up up to its ports: resets it, agrees on features
    /// (the multiport one is needed), reads how many ports it may have,
    /// and sets up its control queues, with completions on MSI-X entry
    /// `vector`. `alloc` gives zeroed DMA memory, in pages.
    pub fn new(
        transport: Transport,
        vector: u16,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    ) -> Result<Console, Error> {
        if !transport.reset() {
            return Err(Error::DeviceFailed);
        }
        transport.add_status(status::ACKNOWLEDGE);
        transport.add_status(status::DRIVER);
        let offered = transport.device_features();
        if offered & common_feature::VERSION_1 == 0 {
            transport.add_status(status::FAILED);
            return Err(Error::MissingFeature("virtio 1.0"));
        }
        if offered & feature::MULTIPORT == 0 {
            transport.add_status(status::FAILED);
            return Err(Error::MissingFeature("several ports"));
        }
        let common = common_feature::EVENT_IDX | common_feature::ACCESS_PLATFORM;
        let features = common_feature::VERSION_1 | feature::MULTIPORT | (offered & common);
        if let Err(err) = transport.set_driver_features(features) {
            transport.add_status(status::FAILED);
            return Err(err);
        }
        transport.no_config_interrupt();
        let ports = transport.read_config(|space| space.read32(4)).min(MAX_PORTS);
        let made = (|| {
            let control_rx =
                Queue::new(&transport, 2, QUEUE_SIZE, vector, features, (0, 0), alloc)?;
            let control_tx =
                Queue::new(&transport, 3, QUEUE_SIZE, vector, features, (0, 0), alloc)?;
            let page = alloc(1).ok_or(Error::NoMemory)?;
            Ok((control_rx, control_tx, page))
        })();
        let (control_rx, control_tx, control) = match made {
            Ok(made) => made,
            Err(err) => {
                transport.add_status(status::FAILED);
                return Err(err);
            }
        };
        let info = Info { ports };
        Ok(Console { transport, control_rx, control_tx, control, info, features })
    }

    /// What the device is.
    pub fn info(&self) -> Info {
        self.info
    }

    /// Sets up port `number`'s queues, with completions on MSI-X entry
    /// `vector`: before [`Console::start`], for each port the driver will
    /// take, up to [`Info::ports`].
    pub fn port(
        &self,
        number: u32,
        vector: u16,
        alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    ) -> Result<Port, Error> {
        if number >= self.info.ports {
            return Err(Error::NoQueue);
        }
        let index = port_queue(number);
        let features = self.features;
        let rx = Queue::new(&self.transport, index, QUEUE_SIZE, vector, features, (0, 0), alloc)?;
        let tx =
            Queue::new(&self.transport, index + 1, QUEUE_SIZE, vector, features, (0, 0), alloc)?;
        let buffers = alloc(2).ok_or(Error::NoMemory)?;
        Ok(Port { number, rx, tx, buffers, reading: None })
    }

    /// Tells the device the driver is ready, gives it buffers for its
    /// control messages, and asks it for its ports, which come as
    /// [`Event::Added`].
    pub fn start(&mut self) -> Result<(), Error> {
        self.transport.add_status(status::DRIVER_OK);
        while self.control_rx.room(1) > 0 {
            let head = self.control_rx.next_free();
            let phys = self.control.phys() + (usize::from(head) * CONTROL_BUFFER) as u64;
            let buffer = Buffer { phys, len: CONTROL_BUFFER as u32, device_writes: true };
            self.control_rx.add(1, |_| buffer)?;
        }
        self.control_rx.notify();
        self.send(0, event::DEVICE_READY, 1)
    }

    /// Whether the device asks to be reset after an error.
    pub fn failed(&self) -> bool {
        self.transport.status() & status::NEEDS_RESET != 0
    }

    /// Sends the control message `event` with `value` for port `port`.
    fn send(&mut self, port: u32, event: u16, value: u16) -> Result<(), Error> {
        // The messages the device has read free their buffers.
        while self.control_tx.used().is_some() {}
        if self.control_tx.room(1) == 0 {
            return Err(Error::Full);
        }
        let head = self.control_tx.next_free();
        let at = CONTROL_TX + usize::from(head) * MESSAGE;
        let mut message = [0u8; MESSAGE];
        message[..4].copy_from_slice(&port.to_le_bytes());
        message[4..6].copy_from_slice(&event.to_le_bytes());
        message[6..].copy_from_slice(&value.to_le_bytes());
        self.control.write(at, &message);
        let phys = self.control.phys() + at as u64;
        self.control_tx.add(1, |_| Buffer { phys, len: MESSAGE as u32, device_writes: false })?;
        self.control_tx.notify();
        Ok(())
    }

    /// Answers [`Event::Added`] for port `port`: it is set up and its
    /// buffers given ([`Port::fill`]), or (`ready` false) the driver does
    /// not take it.
    pub fn port_ready(&mut self, port: u32, ready: bool) -> Result<(), Error> {
        self.send(port, event::PORT_READY, u16::from(ready))
    }

    /// Tells the host a program in the machine opened port `port`
    /// (`open`) or closed it.
    pub fn port_open(&mut self, port: u32, open: bool) -> Result<(), Error> {
        self.send(port, event::PORT_OPEN, u16::from(open))
    }

    /// Reads the control messages the device sent, calling `f` with each
    /// that says something of a port, and gives their buffers back.
    pub fn events(&mut self, f: &mut dyn FnMut(Event)) {
        let mut any = false;
        while let Some((head, len)) = self.control_rx.used() {
            any = true;
            device_barrier();
            let at = usize::from(head) * CONTROL_BUFFER;
            let mut bytes = [0u8; CONTROL_BUFFER];
            let len = (len as usize).min(CONTROL_BUFFER);
            self.control.read(at, &mut bytes[..len]);
            if len >= MESSAGE {
                let port = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let kind = u16::from_le_bytes([bytes[4], bytes[5]]);
                let value = u16::from_le_bytes([bytes[6], bytes[7]]);
                let said = match kind {
                    event::DEVICE_ADD => Some(Event::Added(port)),
                    event::DEVICE_REMOVE => Some(Event::Removed(port)),
                    event::CONSOLE_PORT if value == 1 => Some(Event::Console(port)),
                    event::PORT_OPEN => Some(Event::HostOpen(port, value == 1)),
                    event::PORT_NAME => {
                        let mut name = [0u8; NAME_MAX];
                        let named = &bytes[MESSAGE..len];
                        // The name may end with a NUL.
                        let end = named.iter().position(|&b| b == 0).unwrap_or(named.len());
                        name[..end].copy_from_slice(&named[..end]);
                        Some(Event::Named(port, name, end))
                    }
                    _ => None,
                };
                if let Some(said) = said {
                    f(said);
                }
            }
            let phys = self.control.phys() + at as u64;
            let buffer = Buffer { phys, len: CONTROL_BUFFER as u32, device_writes: true };
            // The buffer just freed: there is room for it.
            let _ = self.control_rx.add(1, |_| buffer);
        }
        if any {
            self.control_rx.notify();
        }
    }

    /// Resets the device: it stops using every buffer it was given.
    pub fn reset(&self) {
        self.transport.reset();
    }
}

impl Drop for Console {
    /// Resets the device before its queues' memory is given back, so that
    /// it writes to none of it after.
    fn drop(&mut self) {
        self.transport.reset();
    }
}

impl Port {
    /// Its number.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// Gives the device every free buffer to fill with what comes: once,
    /// when it was added.
    pub fn fill(&mut self) {
        while self.rx.room(1) > 0 {
            let head = self.rx.next_free();
            let phys = self.buffers.phys() + (usize::from(head) * BUFFER) as u64;
            let buffer = Buffer { phys, len: BUFFER as u32, device_writes: true };
            if self.rx.add(1, |_| buffer).is_err() {
                break;
            }
        }
        self.rx.notify();
    }

    /// Copies what came from the host into `out`, as much as fits;
    /// returns how much. Each buffer read whole goes back to the device.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let mut done = 0;
        while done < out.len() {
            let (head, at, len) = match self.reading {
                Some(reading) => reading,
                None => match self.rx.used() {
                    Some((head, len)) => {
                        device_barrier();
                        (head, 0, (len as usize).min(BUFFER))
                    }
                    None => break,
                },
            };
            let n = (len - at).min(out.len() - done);
            let base = usize::from(head) * BUFFER;
            self.buffers.read(base + at, &mut out[done..done + n]);
            done += n;
            if at + n < len {
                self.reading = Some((head, at + n, len));
                break;
            }
            self.reading = None;
            let phys = self.buffers.phys() + base as u64;
            let buffer = Buffer { phys, len: BUFFER as u32, device_writes: true };
            // Its own descriptor came back with it.
            let _ = self.rx.add(1, |_| buffer);
            self.rx.notify();
        }
        done
    }

    /// Copies as much of `data` as a free buffer holds to go to the host;
    /// returns how much: none while every buffer waits for the device.
    pub fn write(&mut self, data: &[u8]) -> usize {
        while self.tx.used().is_some() {}
        if data.is_empty() || self.tx.room(1) == 0 {
            return 0;
        }
        let n = data.len().min(BUFFER);
        let head = self.tx.next_free();
        let at = 4096 + usize::from(head) * BUFFER;
        self.buffers.write(at, &data[..n]);
        let phys = self.buffers.phys() + at as u64;
        let buffer = Buffer { phys, len: n as u32, device_writes: false };
        if self.tx.add(1, |_| buffer).is_err() {
            return 0;
        }
        self.tx.notify();
        n
    }

    /// Whether what was written still waits for the device to take it.
    pub fn sending(&mut self) -> bool {
        while self.tx.used().is_some() {}
        self.tx.free() < self.tx.size()
    }
}
