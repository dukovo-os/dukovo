// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An emulated USB CD drive for tests (ADR 0447): QEMU has none that
//! plays audio CDs, as its drives present a disc's data alone, in
//! sectors of 2,048 bytes, so this one answers a CD drive's commands
//! itself, over a disc read from a cue sheet and the files it names
//! (audio tracks, data tracks and CD-Text) or from an ISO image.
//!
//! QEMU's `usb-redir` device plugs it into the machine's USB bus: QEMU
//! listens on a Unix socket, this side connects to it and speaks the
//! usbredir protocol as the "usb-host" a real device would be behind
//! ([`redir`]), the device is a Bulk-Only mass storage device ([`usb`]),
//! and the drive answers the SCSI and MMC commands it carries ([`mmc`])
//! over the disc in it ([`disc`]).
//!
//! Boot tests plug one in with [`DiscDrive::plug_in`], and put discs in,
//! take them out and press its eject button through it; `ob disc-drive`
//! runs one for the lab's harness, which says what to do on its standard
//! input ([`command`]).
//!
//! Start with [`DiscDrive`]; [`mmc::Drive`] is the drive itself.

pub mod disc;
pub mod mmc;
pub mod redir;
pub mod usb;

#[cfg(test)]
mod tests;

use std::io::{self, BufRead};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use self::disc::Disc;
use self::mmc::Drive;
use self::redir::{Endpoint, Packet, Wire, status};
use self::usb::Device;
use crate::{Args, Ctx, Error, Result, bail};

/// What the drive is asked to do, by the test or the lab's harness.
enum Ask {
    Insert(Disc),
    TakeOut,
    Button,
    Unplug,
    Locked(Sender<bool>),
}

/// What the drive's thread hears: a packet from QEMU, or an ask.
enum Heard {
    Packet(Packet),
    Ask(Ask),
    /// QEMU closed the connection, or it failed.
    Closed,
}

/// The emulated drive, plugged into a machine: a thread speaks to QEMU
/// for it.
pub struct DiscDrive {
    asks: Sender<Heard>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl DiscDrive {
    /// Connects to QEMU's `usb-redir` character device at `socket`, which
    /// QEMU listens on (trying for up to ten seconds, while QEMU starts),
    /// and plugs the drive in, with `disc` in it.
    pub fn plug_in(socket: &Path, disc: Option<Disc>) -> io::Result<DiscDrive> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(stream) => break stream,
                Err(err) if Instant::now() >= deadline => return Err(err),
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        };
        let (asks, heard) = mpsc::channel();
        let reader = stream.try_clone()?;
        let wire = Wire::hello(stream, "dukovo test disc drive")?;
        let mut packets = wire.sharing(reader);
        let to_drive = asks.clone();
        thread::spawn(move || {
            loop {
                let heard = match packets.read() {
                    Ok(packet) => Heard::Packet(packet),
                    Err(_) => Heard::Closed,
                };
                let closed = matches!(heard, Heard::Closed);
                if to_drive.send(heard).is_err() || closed {
                    return;
                }
            }
        });
        let thread = thread::spawn(move || serve(wire, Device::new(Drive::new(disc)), &heard));
        Ok(DiscDrive { asks, thread: Some(thread) })
    }

    /// Puts `disc` in, the tray shut.
    pub fn insert(&self, disc: Disc) {
        let _ = self.asks.send(Heard::Ask(Ask::Insert(disc)));
    }

    /// Takes the disc out, whether or not the machine locked it in.
    pub fn take_out(&self) {
        let _ = self.asks.send(Heard::Ask(Ask::TakeOut));
    }

    /// Presses the eject button: a disc the machine locked in stays, and
    /// the drive asks the machine to let it go; another comes out.
    pub fn press_button(&self) {
        let _ = self.asks.send(Heard::Ask(Ask::Button));
    }

    /// Whether the machine keeps the disc locked in.
    pub fn locked(&self) -> bool {
        let (tell, told) = mpsc::channel();
        let _ = self.asks.send(Heard::Ask(Ask::Locked(tell)));
        told.recv_timeout(Duration::from_secs(5)).unwrap_or(false)
    }

    /// Unplugs the drive, and waits for its thread.
    pub fn unplug(mut self) {
        let _ = self.asks.send(Heard::Ask(Ask::Unplug));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The device's endpoints, as usbredir's endpoint information says them.
fn endpoints() -> [Endpoint; 4] {
    let control =
        |address| Endpoint { address, kind: 0, interface: 0, max_packet: usb::CONTROL_PACKET };
    let bulk = |address| Endpoint { address, kind: 2, interface: 0, max_packet: usb::BULK_PACKET };
    [control(0x00), control(0x80), bulk(usb::BULK_IN), bulk(usb::BULK_OUT)]
}

/// Runs the device for QEMU until the connection closes or the drive is
/// unplugged.
fn serve(
    mut wire: Wire<UnixStream>,
    mut device: Device,
    heard: &Receiver<Heard>,
) -> io::Result<()> {
    let ids = [usb::VENDOR_ID, usb::PRODUCT_ID, usb::DEVICE_VERSION];
    wire.connect(&endpoints(), usb::INTERFACE, ids)?;
    // Transfers in that wait for data: their ids and lengths.
    let mut waiting: Vec<(u64, u32)> = Vec::new();
    while let Ok(heard) = heard.recv() {
        match heard {
            Heard::Closed => return Ok(()),
            Heard::Ask(ask) => match ask {
                Ask::Insert(disc) => device.drive.insert(disc),
                Ask::TakeOut => drop(device.drive.take_out()),
                Ask::Button => drop(device.drive.press_button()),
                Ask::Locked(tell) => drop(tell.send(device.drive.locked())),
                Ask::Unplug => return wire.disconnect(),
            },
            Heard::Packet(packet) => packet_for(&mut wire, &mut device, packet, &mut waiting)?,
        }
        // A command that came may have data, or a status, for transfers
        // in that wait.
        while let Some(&(id, length)) = waiting.first() {
            let Some(data) = device.bulk_in(length) else { break };
            waiting.remove(0);
            wire.bulk_reply(id, usb::BULK_IN, status::SUCCESS, data.len() as u32, &data)?;
        }
    }
    Ok(())
}

/// Answers a packet from QEMU.
fn packet_for(
    wire: &mut Wire<UnixStream>,
    device: &mut Device,
    packet: Packet,
    waiting: &mut Vec<(u64, u32)>,
) -> io::Result<()> {
    match packet {
        Packet::Reset => {
            device.reset();
            waiting.clear();
            Ok(())
        }
        Packet::SetConfiguration { id, configuration } => {
            let done = device.set_configuration(configuration);
            wire.endpoints(&endpoints())?;
            wire.interface(usb::INTERFACE)?;
            let status = if done { status::SUCCESS } else { status::STALL };
            wire.configuration_status(id, status, device.configuration())
        }
        Packet::GetConfiguration { id } => {
            wire.configuration_status(id, status::SUCCESS, device.configuration())
        }
        Packet::SetAltSetting { id, interface, alt } => {
            let done = interface == 0 && alt == 0;
            wire.endpoints(&endpoints())?;
            wire.interface(usb::INTERFACE)?;
            let status = if done { status::SUCCESS } else { status::STALL };
            wire.alt_setting_status(id, status, interface, 0)
        }
        Packet::GetAltSetting { id, interface } => {
            let status = if interface == 0 { status::SUCCESS } else { status::INVAL };
            wire.alt_setting_status(id, status, interface, 0)
        }
        Packet::Control { id, setup, data: _ } => {
            match device.control(
                setup.request_type,
                setup.request,
                setup.value,
                setup.index,
                setup.length,
            ) {
                Ok(data) => wire.control_reply(id, setup, status::SUCCESS, &data),
                Err(usb::Stall) => wire.control_reply(id, setup, status::STALL, &[]),
            }
        }
        Packet::Bulk { id, endpoint: usb::BULK_OUT, length: _, data } => {
            let length = data.len() as u32;
            match device.bulk_out(&data) {
                Ok(()) => wire.bulk_reply(id, usb::BULK_OUT, status::SUCCESS, length, &[]),
                Err(usb::Stall) => wire.bulk_reply(id, usb::BULK_OUT, status::STALL, 0, &[]),
            }
        }
        Packet::Bulk { id, endpoint: usb::BULK_IN, length, .. } => {
            waiting.push((id, length));
            Ok(())
        }
        Packet::Bulk { id, endpoint, .. } => wire.bulk_reply(id, endpoint, status::INVAL, 0, &[]),
        Packet::Cancel { id } => {
            // Only a transfer in can still wait; one answered already is
            // QEMU's to drop.
            if let Some(at) = waiting.iter().position(|&(waits, _)| waits == id) {
                waiting.remove(at);
                wire.bulk_reply(id, usb::BULK_IN, status::CANCELLED, 0, &[])?;
            }
            Ok(())
        }
        Packet::Other(_) => Ok(()),
    }
}

/// `ob disc-drive <socket> [<disc>]`: runs the drive for the lab's
/// harness, plugged into the QEMU listening on `socket`, with the disc
/// `disc` (a cue sheet or an ISO image) in it. Each line on standard
/// input asks something of it: `insert <disc>`, `take-out`, `button`,
/// `locked` (which prints `locked yes` or `locked no`), and `unplug`,
/// which ends it, as the end of its input does.
pub fn command(_ctx: &Ctx, args: &mut Args) -> Result<()> {
    let Some(socket) = args.positional() else {
        bail!("usage: ob disc-drive <socket> [<disc>]");
    };
    let disc = match args.positional() {
        Some(path) => Some(open(&PathBuf::from(path))?),
        None => None,
    };
    args.finish()?;
    let drive = DiscDrive::plug_in(Path::new(&socket), disc)
        .map_err(|err| Error::new(format!("cannot plug the drive into {socket}: {err}")))?;
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|err| Error::new(format!("standard input: {err}")))?;
        // The path of a disc to insert is the rest of the line, spaces and
        // all.
        match line.trim().split_once(' ').map(|(word, rest)| (word, rest.trim())) {
            Some(("insert", path)) => drive.insert(open(Path::new(path))?),
            _ => match line.trim() {
                "take-out" => drive.take_out(),
                "button" => drive.press_button(),
                "locked" => println!("locked {}", if drive.locked() { "yes" } else { "no" }),
                "unplug" => break,
                "" => {}
                _ => eprintln!("disc-drive: not understood: {line}"),
            },
        }
    }
    drive.unplug();
    Ok(())
}

/// The disc in the file `path`.
fn open(path: &Path) -> Result<Disc> {
    Disc::open(path).map_err(|err| Error::new(format!("{}: {err}", path.display())))
}
