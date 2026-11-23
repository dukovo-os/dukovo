// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/devices/ipmi0`, the BMC's interface for programs, with Linux's
//! requests and structures (ADR 0661; radix-abi `ipmi` says what each
//! request does).
//!
//! A command is sent at once, under the interface's lock in
//! `crate::ipmi`, and its answer kept for the description that sent it
//! until the program receives it, so a receive never waits and `poll`
//! finds the file readable while an answer is kept. The answers are kept
//! here by the description's address and dropped when it closes.
//!
//! `ipmi_req` and `ipmi_recv` hold pointers into the program's memory: the
//! address and the message's data. Each is read or written with the
//! checked copy of the system calls, at most `addr_len` and `data_len`
//! bytes, and those are bounded first, so a program can name only its own
//! memory, and never more of it than an address or a message holds.

use alloc::vec::Vec;
use core::any::Any;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::ipmi::{
    BMC_CHANNEL, DATA_MAX, GET_MY_ADDRESS, GET_MY_LUN, GET_TIMING, QUEUE_MAX, RECEIVE_MESSAGE,
    RECEIVE_MESSAGE_TRUNCATED, RESPONSE_RECEIVE, SEND_COMMAND, SEND_COMMAND_SETTIME,
    SET_GETS_EVENTS, SET_MY_ADDRESS, SET_MY_LUN, SET_TIMING, SYSTEM_INTERFACE_ADDRESS,
    TIMEOUT_COMPLETION_CODE,
};
use radix_hal::UserMode;
use radix_klog::warn;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::Mutex;
use radix_vfs::{Access, In, Kind, Node, NodeRef, Out, Stat, mem, next_id};

use super::FileObject;
use crate::Arch;
use crate::user::posix::poll::{self, Polled};

/// What polls of the file look at: an answer kept wakes them.
pub static POLLED: Polled = Polled::new();

/// The bytes of a message, at most: a command's data has two fewer.
const DATA: usize = DATA_MAX as usize;

/// The answers a description keeps, at most.
const QUEUE: usize = QUEUE_MAX as usize;

/// The most descriptions of the file open at once that have sent a
/// command: the tools open it once each.
const MAX_OPEN: usize = 32;

/// Linux's x86-64 layout of `struct ipmi_req`: the address's pointer and
/// length, the message id, then `struct ipmi_msg` (network function,
/// command, data length, the data's pointer).
mod req {
    pub const SIZE: usize = 40;
    pub const ADDR: usize = 0;
    pub const ADDR_LEN: usize = 8;
    pub const MSGID: usize = 16;
    pub const NETFN: usize = 24;
    pub const CMD: usize = 25;
    pub const DATA_LEN: usize = 26;
    pub const DATA: usize = 32;
}

/// Linux's x86-64 layout of `struct ipmi_recv`: the type, the address's
/// pointer and length, the message id, then `struct ipmi_msg`.
mod recv {
    pub const SIZE: usize = 48;
    pub const RECV_TYPE: usize = 0;
    pub const ADDR: usize = 8;
    pub const ADDR_LEN: usize = 16;
    pub const MSGID: usize = 24;
    pub const NETFN: usize = 32;
    pub const CMD: usize = 33;
    pub const DATA_LEN: usize = 34;
    pub const DATA: usize = 40;
}

/// The bytes of `struct ipmi_system_interface_addr`: its type (an int),
/// its channel (a short) and its LUN, padded to 8.
const SYSTEM_ADDRESS_SIZE: usize = 8;

/// The bytes of `struct ipmi_addr`, the largest address a program may
/// give: a type, a channel and 32 bytes, padded to 40.
const ADDRESS_MAX: usize = 40;

/// The node, numbered as its id.
pub struct Ipmi(pub u64);

impl Node for Ipmi {
    // Root's alone: the BMC can restart the machine, change its network
    // and clear its event log. A service is given it by its service file.
    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o600 }
    }

    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::CharDevice,
            size: 0,
            allocated: 0,
            id: self.0,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::CharDevice
    }

    // Everything goes through `control`.
    fn read(&self, _offset: u64, _out: &mut dyn Out) -> radix_vfs::Result<usize> {
        Err(radix_vfs::Error::NotSupported)
    }

    fn write(&self, _offset: u64, _data: &dyn In) -> radix_vfs::Result<usize> {
        Err(radix_vfs::Error::NotSupported)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Puts `ipmi0` in `/devices`: a BMC has answered.
pub fn publish() {
    let added = mem::arc(Ipmi(next_id()))
        .map_err(radix_vfs::Error::from)
        .and_then(|node| super::devfs::add("ipmi0", node));
    if let Err(err) = added {
        warn!("IPMI: could not add /devices/ipmi0: {err}");
    }
}

/// Whether `node` is `/devices/ipmi0`.
pub fn is_ipmi(node: &NodeRef) -> bool {
    radix_vfs::downcast::<Ipmi>(&**node).is_some()
}

/// An answer kept for a description.
struct Answer {
    msgid: i64,
    netfn: u8,
    cmd: u8,
    len: u16,
    /// The completion code, then the answer's data.
    data: [u8; DATA],
}

/// What a description keeps: its answers, oldest first, and the settings
/// Linux keeps for it.
struct Open {
    file: usize,
    answers: Vec<Answer, Kmalloc>,
    my_address: u32,
    my_lun: u32,
    timing: [u8; 8],
}

/// The descriptions that have used the file, made by the first.
static OPEN: Mutex<Option<Vec<Open, Kmalloc>>> = Mutex::new(None);

fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

fn invalid() -> Status {
    Kernel::InvalidArgument.status(0)
}

fn bad_address() -> Status {
    Kernel::BadAddress.status(0)
}

/// Runs `act` on what `file` keeps, made the first time.
fn with_open<T>(
    file: &FileObject,
    act: impl FnOnce(&mut Open) -> Result<T, Status>,
) -> Result<T, Status> {
    let me = file.address();
    let mut open = OPEN.lock();
    let open = open.get_or_insert_with(|| Vec::new_in(kmalloc()));
    let index = match open.iter().position(|entry| entry.file == me) {
        Some(index) => index,
        None => {
            if open.len() >= MAX_OPEN {
                return Err(Kernel::Busy.status(0));
            }
            open.try_reserve(1).map_err(|_| Kernel::NoMemory.status(0))?;
            let answers = Vec::new_in(kmalloc());
            let entry = Open { file: me, answers, my_address: 0x20, my_lun: 2, timing: [0; 8] };
            // Within the capacity just reserved.
            let _ = open.push_within_capacity(entry);
            open.len() - 1
        }
    };
    act(&mut open[index])
}

/// What a poll of `file` finds: readable while an answer is kept.
pub fn events(file: &FileObject) -> u16 {
    let me = file.address();
    let open = OPEN.lock();
    let waiting = open
        .as_ref()
        .and_then(|open| open.iter().find(|entry| entry.file == me))
        .is_some_and(|entry| !entry.answers.is_empty());
    if waiting { poll::READABLE | poll::WRITABLE } else { poll::WRITABLE }
}

/// The description at `file` has gone: its answers go with it.
pub fn closed(file: &FileObject) {
    let me = file.address();
    if let Some(open) = OPEN.lock().as_mut() {
        open.retain(|entry| entry.file != me);
    }
}

fn word_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(word)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

/// Reads the address an `ipmi_req` points at and checks that it is the
/// system interface's, the only one the kernel sends to.
fn check_address(pointer: u64, len: u32) -> Result<(), Status> {
    // As Linux: any length up to `struct ipmi_addr`'s, and at least the
    // system interface address's, the only part read.
    if !(SYSTEM_ADDRESS_SIZE..=ADDRESS_MAX).contains(&(len as usize)) {
        return Err(invalid());
    }
    let mut address = [0u8; SYSTEM_ADDRESS_SIZE];
    if !Arch::copy_from_user(&mut address, pointer) {
        return Err(bad_address());
    }
    if u32_at(&address, 0) != SYSTEM_INTERFACE_ADDRESS {
        // IPMB and LAN addresses, which bridge through the BMC: not yet.
        return Err(Kernel::NotSupported.status(0));
    }
    if u32::from(u16_at(&address, 4)) != BMC_CHANNEL {
        return Err(invalid());
    }
    Ok(())
}

/// `control`'s requests of `file`: `request` with the `size` bytes at
/// `buffer`, which `copy_in` reads and `copy_out` writes, the structures'
/// own pointers read and written here.
///
/// # Errors
///
/// As radix-abi `ipmi` says: invalid_argument for a structure too short or
/// an address's length out of bounds, bad_address for memory the program
/// does not have, not_supported for an address other than the BMC's,
/// busy with QUEUE_MAX answers kept (or too many descriptions open),
/// would_block with none, message_too_long for a command or an answer
/// that does not fit; not_terminal for another request.
pub fn control(
    file: &FileObject,
    request: u32,
    size: u64,
    copy_in: impl Fn(&mut [u8]) -> Result<(), Status>,
    copy_out: impl Fn(&[u8]) -> Result<(), Status>,
) -> Result<(), Status> {
    match request {
        SEND_COMMAND | SEND_COMMAND_SETTIME => {
            let need = if request == SEND_COMMAND { req::SIZE } else { req::SIZE + 8 };
            if size < need as u64 {
                return Err(invalid());
            }
            let mut bytes = [0u8; req::SIZE + 8];
            copy_in(&mut bytes[..need])?;
            send(file, &bytes[..req::SIZE])
        }
        RECEIVE_MESSAGE | RECEIVE_MESSAGE_TRUNCATED => {
            if size < recv::SIZE as u64 {
                return Err(invalid());
            }
            let mut bytes = [0u8; recv::SIZE];
            copy_in(&mut bytes)?;
            receive(file, &mut bytes, request == RECEIVE_MESSAGE_TRUNCATED, &copy_out)
        }
        // Events are not delivered yet; asking for them is not an error,
        // which ipmitool would take as no BMC.
        SET_GETS_EVENTS => {
            let mut word = [0u8; 4];
            copy_in(&mut word)
        }
        SET_MY_ADDRESS | SET_MY_LUN => {
            let mut word = [0u8; 4];
            copy_in(&mut word)?;
            let value = u32::from_le_bytes(word);
            if value > 0xff || (request == SET_MY_LUN && value > 3) {
                return Err(invalid());
            }
            with_open(file, |open| {
                match request {
                    SET_MY_ADDRESS => open.my_address = value,
                    _ => open.my_lun = value,
                }
                Ok(())
            })
        }
        GET_MY_ADDRESS | GET_MY_LUN => {
            let value = with_open(file, |open| {
                Ok(if request == GET_MY_ADDRESS { open.my_address } else { open.my_lun })
            })?;
            copy_out(&value.to_le_bytes())
        }
        SET_TIMING => {
            let mut timing = [0u8; 8];
            copy_in(&mut timing)?;
            with_open(file, |open| {
                open.timing = timing;
                Ok(())
            })
        }
        GET_TIMING => copy_out(&with_open(file, |open| Ok(open.timing))?),
        _ => Err(Kernel::NotTerminal.status(0)),
    }
}

/// Sends the command of the `ipmi_req` in `bytes`, and keeps its answer.
fn send(file: &FileObject, bytes: &[u8]) -> Result<(), Status> {
    check_address(word_at(bytes, req::ADDR), u32_at(bytes, req::ADDR_LEN))?;
    let msgid = word_at(bytes, req::MSGID) as i64;
    let (netfn, cmd) = (bytes[req::NETFN], bytes[req::CMD]);
    // A network function is six bits, and a request's is even.
    if netfn > 0x3f || netfn & 1 != 0 {
        return Err(invalid());
    }
    // A message holds its network function and command besides, as
    // Linux counts it.
    let len = usize::from(u16_at(bytes, req::DATA_LEN));
    if len + 2 > DATA {
        return Err(Kernel::MessageTooLong.status(0));
    }
    let mut data = [0u8; DATA];
    if len > 0 && !Arch::copy_from_user(&mut data[..len], word_at(bytes, req::DATA)) {
        return Err(bad_address());
    }
    // Room for the answer first, so a full queue sends nothing.
    with_open(file, |open| {
        if open.answers.len() >= QUEUE {
            return Err(Kernel::Busy.status(0));
        }
        open.answers.try_reserve(1).map_err(|_| Kernel::NoMemory.status(0))
    })?;
    let mut answer = Answer { msgid, netfn: netfn | 1, cmd, len: 1, data: [0; DATA] };
    match crate::ipmi::request(netfn, cmd, &data[..len], &mut answer.data) {
        Ok(got) if got > 0 => answer.len = got as u16,
        // No answer, or one without its completion code: as Linux, the
        // program gets a timeout's.
        _ => answer.data[0] = TIMEOUT_COMPLETION_CODE as u8,
    }
    with_open(file, |open| {
        // Room was reserved, and only this description adds to its queue;
        // a full one now means another thread of the program sent too.
        open.answers.push_within_capacity(answer).map(|_| ()).map_err(|_| Kernel::Busy.status(0))
    })?;
    POLLED.changed();
    Ok(())
}

/// Fills in the `ipmi_recv` in `bytes` with the oldest answer kept, and
/// writes it back with `copy_out`.
fn receive(
    file: &FileObject,
    bytes: &mut [u8; recv::SIZE],
    truncate: bool,
    copy_out: &impl Fn(&[u8]) -> Result<(), Status>,
) -> Result<(), Status> {
    let address_pointer = word_at(bytes, recv::ADDR);
    let address_room = u32_at(bytes, recv::ADDR_LEN) as usize;
    let data_pointer = word_at(bytes, recv::DATA);
    let data_room = usize::from(u16_at(bytes, recv::DATA_LEN));
    with_open(file, |open| {
        let Some(answer) = open.answers.first() else {
            return Err(Kernel::WouldBlock.status(0));
        };
        if address_room < SYSTEM_ADDRESS_SIZE {
            return Err(invalid());
        }
        let len = usize::from(answer.len);
        let fits = len <= data_room;
        if !fits && !truncate {
            return Err(Kernel::MessageTooLong.status(0));
        }
        let written = len.min(data_room);
        let mut address = [0u8; SYSTEM_ADDRESS_SIZE];
        address[0..4].copy_from_slice(&SYSTEM_INTERFACE_ADDRESS.to_le_bytes());
        address[4..6].copy_from_slice(&(BMC_CHANNEL as u16).to_le_bytes());
        // The LUN the answer came to, as Linux sets it: none of our own.
        if !Arch::copy_to_user(address_pointer, &address) {
            return Err(bad_address());
        }
        if written > 0 && !Arch::copy_to_user(data_pointer, &answer.data[..written]) {
            return Err(bad_address());
        }
        bytes[recv::RECV_TYPE..recv::RECV_TYPE + 4]
            .copy_from_slice(&RESPONSE_RECEIVE.to_le_bytes());
        bytes[recv::ADDR_LEN..recv::ADDR_LEN + 4]
            .copy_from_slice(&(SYSTEM_ADDRESS_SIZE as u32).to_le_bytes());
        bytes[recv::MSGID..recv::MSGID + 8].copy_from_slice(&answer.msgid.to_le_bytes());
        bytes[recv::NETFN] = answer.netfn;
        bytes[recv::CMD] = answer.cmd;
        bytes[recv::DATA_LEN..recv::DATA_LEN + 2].copy_from_slice(&(written as u16).to_le_bytes());
        copy_out(&bytes[..])?;
        // Taken only once the program has it.
        open.answers.remove(0);
        if fits { Ok(()) } else { Err(Kernel::MessageTooLong.status(0)) }
    })
}
