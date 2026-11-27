// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The keyboards and pointers: the device files inputd hands the
//! desktop (inputd(8), ADR 0707), and what they say.
//!
//! windowd asks inputd once, with a copy of its holder of the screens,
//! and is handed a description of each device's file, tied to that
//! holder: the kernel gives it events only while the desktop is in
//! front (radix-input(2)). Devices that come later are handed on as
//! they come. What a device says is put together report by report
//! ([`decode`]) and told the desktop.

pub mod decode;

use std::fs::File;
use std::io::ErrorKind::{Interrupted, WouldBlock};
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;

use input::abi::{ABS_X, ABS_Y, InputEvent};
use input::kind::Kinds;
use input::service::{DEVICE, GONE, Inbox, REFUSED, Record, SOCKET};
use input::{Control, Device};
use radix_abi::capability::Rights;
use smithay::backend::renderer::Bind;
use smithay::backend::renderer::pixman::PixmanRenderer;
use sysapi::{fd, kernel, net};

use crate::desktop::Desktop;
use crate::link::Link;
use decode::{Heard, Report};

/// How many events of a device are read at once.
const EVENTS: usize = 64;

/// How many devices are kept at most: one more is left unread. Each
/// takes a place of the list windowd waits on.
const DEVICES_MAX: usize = 64;

/// A device's file.
#[derive(Debug)]
struct Reached(File);

impl Control for Reached {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        kernel::control(&self.0, request, argument, buffer).map_err(kernel::io_error)
    }
}

/// One device inputd handed out.
#[derive(Debug)]
struct Handed {
    /// inputd's number for it.
    number: u32,
    kinds: Kinds,
    device: Device<Reached>,
    /// The report its file is in the middle of.
    report: Report,
    /// The least and the most each of its axes says, if it points at
    /// places.
    spans: [Option<(i32, i32)>; 2],
}

impl Handed {
    fn of(record: Record, file: OwnedFd) -> io::Result<Handed> {
        fd::set_nonblocking(&file, true)?;
        let device = Device::new(Reached(File::from(file)));
        // A device with no such axis, as a mouse, has no span.
        let span = |code| device.axis(code).ok().map(|axis| (axis.min, axis.max));
        let spans = [span(ABS_X), span(ABS_Y)];
        Ok(Handed {
            number: record.number,
            kinds: record.kinds(),
            device,
            report: Report::default(),
            spans,
        })
    }

    /// How far along its axis `at` the device points for `value`: 0 at
    /// the least it says, 1 at the most.
    fn along(&self, at: usize, value: Option<i32>) -> Option<f64> {
        let (least, most) = self.spans[at]?;
        let span = f64::from(most) - f64::from(least);
        (span > 0.0).then_some((f64::from(value?) - f64::from(least)) / span)
    }
}

/// The devices.
#[derive(Debug)]
pub struct Devices {
    inputd: UnixStream,
    inbox: Inbox<OwnedFd>,
    handed: Vec<Handed>,
}

impl Devices {
    /// Asks inputd for the devices, for the holder of the screens whose
    /// description `holder` is.
    ///
    /// # Errors
    ///
    /// When inputd is not there, or the request could not be sent.
    pub fn ask(holder: BorrowedFd<'_>) -> io::Result<Devices> {
        // A copy that names the holder and can do nothing but be
        // passed on.
        let named = kernel::narrowed(holder, Rights::COPY).map_err(kernel::io_error)?;
        let inputd = UnixStream::connect(SOCKET)?;
        net::send_with_fds(&inputd, &Record::holder().to_bytes(), &[named.as_fd()])?;
        inputd.set_nonblocking(true)?;
        Ok(Devices { inputd, inbox: Inbox::new(), handed: Vec::new() })
    }

    /// inputd's connection, to wait on.
    pub fn inputd(&self) -> BorrowedFd<'_> {
        self.inputd.as_fd()
    }

    /// Each device's file, to wait on, in an order that holds until
    /// [`Devices::inputd_wrote`] runs.
    pub fn waited_on(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        self.handed.iter().map(|handed| handed.device.file().0.as_fd())
    }

    /// Takes in what inputd sent: devices that came and went.
    ///
    /// # Errors
    ///
    /// When inputd refused or is gone: no device comes any more.
    pub fn inputd_wrote(&mut self) -> io::Result<()> {
        let mut bytes = [0u8; 4 * Record::SIZE];
        let mut files = Vec::new();
        let read = match net::receive_with_fds(&self.inputd, &mut bytes, &mut files, 4) {
            Err(error) if error.kind() == WouldBlock => return Ok(()),
            read => read?,
        };
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "inputd is gone"));
        }
        self.inbox.take(&bytes[..read], files);
        while let Some(next) = self.inbox.next() {
            let (record, file) = next.map_err(io::Error::other)?;
            match (record.what, file) {
                (DEVICE, Some(file)) if self.handed.len() < DEVICES_MAX => {
                    self.handed.push(Handed::of(record, file)?);
                }
                (GONE, _) => self.handed.retain(|handed| handed.number != record.number),
                (REFUSED, _) => {
                    let why = format!("inputd refused, for reason {}", record.number);
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
                }
                // READY, a device beyond the most, and whatever a later
                // inputd says.
                _ => {}
            }
        }
        Ok(())
    }

    /// The devices wrote whose places in [`Devices::waited_on`] `wrote`
    /// names: what they said is told `desktop`.
    pub fn devices_wrote<L: Link>(
        &mut self,
        wrote: impl IntoIterator<Item = usize>,
        desktop: &mut Desktop<L>,
    ) where
        PixmanRenderer: Bind<L::Pixels>,
    {
        let mut bytes = [0u8; EVENTS * InputEvent::SIZE];
        let mut heard = Vec::new();
        let mut went = Vec::new();
        for at in wrote {
            let Some(handed) = self.handed.get_mut(at) else { continue };
            let read = match (&handed.device.file().0).read(&mut bytes) {
                Ok(read) => read,
                Err(error) if matches!(error.kind(), WouldBlock | Interrupted) => continue,
                Err(_) => {
                    // The device went. Its file says so at every wait
                    // from here on, so it is not waited on until inputd
                    // says the same.
                    went.push(handed.number);
                    continue;
                }
            };
            for event in input::wire::events(&bytes[..read]) {
                handed.report.take(&event, &mut heard);
                for heard in heard.drain(..) {
                    tell(desktop, handed, event.time_ns, heard);
                }
            }
        }
        self.handed.retain(|handed| !went.contains(&handed.number));
    }

    /// Lights the lock lights `lights` names, in INPUT_LIGHTS's bits, on
    /// every keyboard.
    pub fn light(&self, lights: u32) {
        // One request lights every keyboard: the first keyboard that is
        // heard and has lights takes it.
        let keyboards = self.handed.iter().filter(|handed| handed.kinds.is(Kinds::KEYBOARD));
        let _ = keyboards.map(|handed| handed.device.light(lights)).find(Result::is_ok);
    }
}

/// Tells `desktop` what `handed` said at `time_ns`.
fn tell<L: Link>(desktop: &mut Desktop<L>, handed: &Handed, time_ns: u64, heard: Heard)
where
    PixmanRenderer: Bind<L::Pixels>,
{
    match heard {
        Heard::Key { code, pressed } => desktop.key(time_ns, code, pressed),
        Heard::Button { code, pressed } => desktop.button(time_ns, code, pressed),
        Heard::Moved { dx, dy } => desktop.moved(time_ns, dx, dy),
        Heard::Points { x, y } => {
            desktop.pointed(time_ns, handed.along(0, x), handed.along(1, y));
        }
        Heard::Wheel { vertical, horizontal } => desktop.wheel(time_ns, vertical, horizontal),
        Heard::LetGo => desktop.let_go(time_ns),
    }
}
