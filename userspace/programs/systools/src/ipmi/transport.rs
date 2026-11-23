// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The BMC through `/devices/ipmi0`, with Linux's IPMI ABI (the
//! `ipmi_req` and `ipmi_recv` records and the `IPMICTL_*` ioctls of
//! `linux/ipmi.h`, x86-64 layouts), which the kernel serves as Linux
//! does: a send does the exchange and queues the response on the open
//! file, `poll` says when it can be received, and a receive takes it.
//!
//! This is the one file that holds those numbers, so that a later change
//! to the ABI's generated constants (abi/ipmi.ridl) touches only it.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;

use super::Transport;

/// Where the kernel shows the BMC's interface.
pub(crate) const PATH: &str = "/devices/ipmi0";

/// The address type of the system interface, the BMC itself
/// (`IPMI_SYSTEM_INTERFACE_ADDR_TYPE`).
const SYSTEM_INTERFACE_ADDR_TYPE: i32 = 0x0C;

/// The channel number of the system interface (`IPMI_BMC_CHANNEL`).
const BMC_CHANNEL: i16 = 0x0F;

/// `IPMICTL_SEND_COMMAND`: `_IOR('i', 13, struct ipmi_req)`, which Linux
/// declares as a read although the program passes the request in.
const SEND_COMMAND: libc::Ioctl = ioctl_number(2, 13, 40);

/// `IPMICTL_RECEIVE_MSG_TRUNC`: `_IOWR('i', 11, struct ipmi_recv)`.
const RECEIVE_MSG_TRUNC: libc::Ioctl = ioctl_number(3, 11, 48);

/// The most bytes of a message in either direction that the kernel takes
/// (`IPMI_MAX_MSG_LENGTH`).
const MESSAGE_MAX: usize = 272;

/// How long a response is waited for, in milliseconds. The kernel sends
/// a request within 5 seconds or fails it.
const WAIT_MS: libc::c_int = 10_000;

/// Linux's `_IOC(direction, 'i', number, size)`.
const fn ioctl_number(direction: u32, number: u32, size: u32) -> libc::Ioctl {
    ((direction << 30) | (size << 16) | ((b'i' as u32) << 8) | number) as libc::Ioctl
}

/// `struct ipmi_system_interface_addr`.
#[repr(C)]
struct SystemAddress {
    addr_type: i32,
    channel: i16,
    lun: u8,
}

/// `struct ipmi_msg`.
#[repr(C)]
struct Message {
    netfn: u8,
    cmd: u8,
    data_len: u16,
    data: *mut u8,
}

/// `struct ipmi_req`.
#[repr(C)]
struct Request {
    addr: *mut SystemAddress,
    addr_len: u32,
    msgid: i64,
    msg: Message,
}

/// `struct ipmi_recv`.
#[repr(C)]
struct Received {
    recv_type: i32,
    addr: *mut SystemAddress,
    addr_len: u32,
    msgid: i64,
    msg: Message,
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Request>() == 40);
    assert!(size_of::<Received>() == 48);
    assert!(size_of::<SystemAddress>() == 8);
};

/// An open `/devices/ipmi0`.
pub(crate) struct Device {
    file: File,
    next_id: i64,
}

impl Device {
    /// Opens the BMC's interface.
    ///
    /// # Errors
    ///
    /// If the file is missing (the machine has no BMC Dukovo reaches) or
    /// the caller may not open it.
    pub(crate) fn open() -> io::Result<Device> {
        Device::open_path(PATH)
    }

    /// Opens the BMC's interface for a program that shows the BMC when it
    /// may: `None` for a machine with no BMC that Dukovo reaches and for a
    /// caller the interface is not open to, which look alike to a person
    /// who is not an administrator.
    ///
    /// # Errors
    ///
    /// If the file is there and openable but the open fails otherwise.
    pub(crate) fn open_if_allowed() -> io::Result<Option<Device>> {
        match Device::open() {
            Ok(device) => Ok(Some(device)),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn open_path(path: &str) -> io::Result<Device> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Device { file, next_id: 1 })
    }

    /// Waits for the response to be readable.
    fn wait(&self) -> io::Result<()> {
        let mut poll = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd, and the count says so.
        let ready = unsafe { libc::poll(&raw mut poll, 1, WAIT_MS) };
        match ready {
            1.. => Ok(()),
            0 => Err(io::Error::new(io::ErrorKind::TimedOut, "the BMC did not answer")),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Takes one queued response, whose message number is not `id`
    /// when it answers an earlier request that nobody read.
    fn receive(&self, id: i64) -> io::Result<Option<Vec<u8>>> {
        let mut address = SystemAddress { addr_type: 0, channel: 0, lun: 0 };
        let mut buffer = vec![0u8; MESSAGE_MAX];
        let mut received = Received {
            recv_type: 0,
            addr: &raw mut address,
            addr_len: size_of::<SystemAddress>() as u32,
            msgid: 0,
            msg: Message {
                netfn: 0,
                cmd: 0,
                data_len: MESSAGE_MAX as u16,
                data: buffer.as_mut_ptr(),
            },
        };
        // SAFETY: the descriptor is open; `received` points at an address
        // and a buffer of the sizes it gives, which live through the call.
        let done =
            unsafe { libc::ioctl(self.file.as_raw_fd(), RECEIVE_MSG_TRUNC, &raw mut received) };
        if done < 0 {
            let error = io::Error::last_os_error();
            // A message longer than the buffer is cut and still delivered.
            if error.raw_os_error() != Some(libc::EMSGSIZE) {
                return Err(error);
            }
        }
        if received.msgid != id {
            return Ok(None);
        }
        buffer.truncate(usize::from(received.msg.data_len).min(MESSAGE_MAX));
        Ok(Some(buffer))
    }
}

impl Transport for Device {
    fn request(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> io::Result<Vec<u8>> {
        let data_len = match u16::try_from(data.len()) {
            Ok(len) if data.len() <= MESSAGE_MAX => len,
            _ => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "the request is too long"));
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        let mut payload = data.to_vec();
        let mut address =
            SystemAddress { addr_type: SYSTEM_INTERFACE_ADDR_TYPE, channel: BMC_CHANNEL, lun: 0 };
        let mut request = Request {
            addr: &raw mut address,
            addr_len: size_of::<SystemAddress>() as u32,
            msgid: id,
            msg: Message { netfn, cmd, data_len, data: payload.as_mut_ptr() },
        };
        // SAFETY: the descriptor is open; the request points at an
        // address and data that live through the call, and the kernel
        // only reads them.
        let sent = unsafe { libc::ioctl(self.file.as_raw_fd(), SEND_COMMAND, &raw mut request) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        // Responses to requests that timed out earlier may be queued first.
        for _ in 0..16 {
            self.wait()?;
            if let Some(reply) = self.receive(id)? {
                return Ok(reply);
            }
        }
        Err(io::Error::new(io::ErrorKind::InvalidData, "the BMC's answer never came"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The request type is as wide as the C library makes it, so the cast
    // is to a width that holds them on every one.
    #[allow(clippy::unnecessary_cast)]
    #[test]
    fn the_ioctl_numbers_are_linuxs() {
        assert_eq!(SEND_COMMAND as u64, 0x8028_690D);
        assert_eq!(RECEIVE_MSG_TRUNC as u64, 0xC030_690B);
    }

    #[test]
    fn the_records_have_linuxs_x86_64_layout() {
        assert_eq!(size_of::<Request>(), 40);
        assert_eq!(std::mem::offset_of!(Request, msgid), 16);
        assert_eq!(std::mem::offset_of!(Request, msg), 24);
        assert_eq!(std::mem::offset_of!(Message, data), 8);
        assert_eq!(size_of::<Received>(), 48);
        assert_eq!(std::mem::offset_of!(Received, addr_len), 16);
        assert_eq!(std::mem::offset_of!(Received, msgid), 24);
        assert_eq!(std::mem::offset_of!(Received, msg), 32);
    }

    #[test]
    fn a_missing_device_is_not_found() {
        let error = Device::open_path("/devices/no-such-ipmi").err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
