// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Netlink route sockets (ADR 0621): what getifaddrs(3), Go's
//! net.Interfaces and Node's os.networkInterfaces read the network
//! interfaces, their addresses and the routes through.
//!
//! A socket is a file of linuxd's filesystem, `.netlink/<id>`, which
//! linuxd opens and gives the program as the descriptor `socket` returns.
//! The kernel forwards the socket calls on it (`bind`, `sendto`,
//! `recvmsg` and the rest) to linuxd, which finds the socket by the file's
//! node number; `read` and `write` on it reach [`Netlinks::read`] and
//! [`Netlinks::write`] through the filesystem.
//!
//! The socket speaks NETLINK_ROUTE to the kernel alone, as rtnetlink(7)
//! describes: a request for the interfaces (RTM_GETLINK), addresses
//! (RTM_GETADDR) or routes (RTM_GETROUTE) is answered with a list of
//! RTM_NEWLINK, RTM_NEWADDR or RTM_NEWROUTE messages and an NLMSG_DONE,
//! one interface by number or name too, and any other request with an
//! NLMSG_ERROR. No multicast event is ever sent, so joining a group is
//! accepted and changes nothing. The calls are in `calls`; the
//! sockets' queues, options and numbers in `socket`; the messages in
//! `wire` (framing and the request parser, which the fuzz target
//! `linuxd-netlink` runs) and `answer`; and what they list in `machine`,
//! read from the kernel's own files.
//!
//! When a program reads the interfaces wrongly, run `ip -j addr` and
//! `ip route` from a Linux sandbox and compare them with
//! `/kernel/class/net` and `/processes/net/route`: the answers are made
//! from those files alone. The host tests of this module cover the calls
//! of glibc's getifaddrs and Go's `syscall.NetlinkRIB`; the boot test
//! `linux-view` covers them on Dukovo.

mod answer;
mod calls;
pub mod machine;
mod socket;
#[cfg(test)]
mod tests;
pub mod wire;

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::call::{Answer, Process};

use self::machine::{Hardware, Machine};
use self::socket::{Caller, NlAddress, Table};

/// The kernel's folder of network interfaces, and of their counts and
/// routes.
const CLASS_NET: &str = "/kernel/class/net";
const PROCESS_NET: &str = "/processes/net";

/// How linuxd makes the file of a new socket.
pub trait SocketFiles: Send + Sync {
    /// Opens `.netlink/<id>` of linuxd's filesystem for reading and
    /// writing.
    fn open(&self, id: u64) -> Result<OwnedFd, i32>;
}

/// The socket files, opened beneath the filesystem's root.
pub struct RootFiles {
    root: OwnedFd,
}

impl RootFiles {
    /// Opens socket files beneath `root`, a descriptor of the
    /// filesystem's root.
    pub fn new(root: OwnedFd) -> RootFiles {
        RootFiles { root }
    }
}

impl SocketFiles for RootFiles {
    fn open(&self, id: u64) -> Result<OwnedFd, i32> {
        let path = CString::new(format!(".netlink/{id}")).map_err(|_| libc::EINVAL)?;
        // SAFETY: `root` is an open descriptor that lives as long as
        // `self`, and the path is a NUL-ended string that lives until
        // openat returns.
        let fd = unsafe {
            libc::openat(self.root.as_raw_fd(), path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC)
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO));
        }
        // SAFETY: openat made this descriptor and nothing else holds it.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

/// No socket files, for tests that open none.
pub struct NoFiles;

impl SocketFiles for NoFiles {
    fn open(&self, _id: u64) -> Result<OwnedFd, i32> {
        Err(libc::ENFILE)
    }
}

/// The node number of the file of socket `id`.
pub fn node_of(id: u64) -> u64 {
    NODE_BASE + id
}

/// The socket whose file has node number `node`, if it is one.
pub fn id_of(node: u64) -> Option<u64> {
    node.checked_sub(NODE_BASE)
}

/// Where the socket files' node numbers start: above every other node of
/// the filesystem's (`fs`).
pub const NODE_BASE: u64 = 1 << 62;

/// Every netlink socket linuxd serves.
pub struct Netlinks {
    files: Box<dyn SocketFiles>,
    machine: Box<dyn Machine>,
    table: Mutex<Table>,
}

impl Netlinks {
    /// No sockets yet; `files` makes their files, and the machine's
    /// network is read from the kernel's files.
    pub fn new(files: Box<dyn SocketFiles>) -> Netlinks {
        Netlinks::with_machine(files, Box::new(Hardware::new(CLASS_NET, PROCESS_NET)))
    }

    /// No sockets yet; `files` makes their files, and `machine` says what
    /// the network is.
    pub fn with_machine(files: Box<dyn SocketFiles>, machine: Box<dyn Machine>) -> Netlinks {
        Netlinks { files, machine, table: Mutex::new(Table::default()) }
    }

    /// The table, whose operations never leave it half changed, so a
    /// panic of another thread that held the lock does not make it
    /// unusable.
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// socket(AF_NETLINK, type, protocol) for `process`.
    pub fn socket(&self, process: &mut dyn Process, args: &[u64; 6]) -> Answer {
        calls::socket(self, process, args)
    }

    /// A socket call (`number`, Linux's) on a descriptor of `process`:
    /// ENOTSOCK unless the descriptor is one of these sockets.
    pub fn call(&self, process: &mut dyn Process, number: u64, args: &[u64; 6]) -> Answer {
        calls::call(self, process, number, args)
    }

    /// read(2) on socket `id`'s file, of at most `size` bytes: a receive.
    pub fn read(&self, id: u64, size: usize) -> Result<Vec<u8>, i32> {
        let mut datagram = self.table().receive(id, size, false).map_err(not_open)?;
        datagram.truncate(size);
        Ok(datagram)
    }

    /// write(2) on socket `id`'s file: a send. The file has no process to
    /// name, so a socket not bound yet is given a number of its own, and
    /// the writer is not taken for one who may change the network.
    pub fn write(&self, id: u64, bytes: &[u8]) -> Result<usize, i32> {
        self.send(id, &Caller::default(), None, bytes.len(), || Ok(bytes.to_vec()))
            .map_err(not_open)
    }

    /// The last description of socket `id`'s file has gone.
    pub fn release(&self, id: u64) {
        self.table().remove(id);
    }

    /// Whether socket `id` is open: its file is in `.netlink` while it is.
    pub fn exists(&self, id: u64) -> bool {
        self.table().contains(id)
    }

    /// The open sockets, for listing `.netlink`.
    pub fn ids(&self) -> Vec<u64> {
        self.table().ids()
    }

    /// Sends `length` bytes on socket `id` to `destination`, the kernel
    /// if none: `gather` reads them, once the socket has said they may
    /// be sent. Gives how many were sent. The datagram is answered at
    /// once, the answers queued for the socket.
    fn send(
        &self,
        id: u64,
        caller: &Caller,
        destination: Option<NlAddress>,
        length: usize,
        gather: impl FnOnce() -> Result<Vec<u8>, i32>,
    ) -> Result<usize, i32> {
        let prepared = self.table().prepare_send(id, caller, destination)?;
        if length > prepared.sndbuf {
            return Err(libc::EMSGSIZE);
        }
        let datagram = gather()?;
        if prepared.delivers {
            // The machine is read with the table free, since a file can
            // be slow.
            let replies = answer::replies(&datagram, &prepared.sender, &*self.machine);
            self.table().deliver(id, replies)?;
        }
        Ok(datagram.len())
    }
}

/// A file that has no socket is not open.
fn not_open(errno: i32) -> i32 {
    if errno == libc::ENOTSOCK { libc::EBADF } else { errno }
}
