// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Forwarding (ADR 0618): the Linux calls the table does not answer,
//! sent to linuxd, the program that answers them for a Linux view's
//! programs (linuxd(8)).
//!
//! A program that `linux run` starts has, in its slot SLOT_LINUX, an
//! endpoint linuxd receives on. A call the table does not know, a
//! `socket` of a family the kernel does not have, and a socket call on a
//! descriptor that is no socket of the kernel's go there as a call
//! ([`forward`], radix-linux-call(2)): the call's number and its six
//! arguments, which linuxd answers with Linux's result. The thread that
//! received it takes a capability to the call with its process's
//! `forwarded_call` ([`claim`]): a [`LinuxCall`], through which it reads
//! and writes the caller's memory and gives and takes its descriptors
//! ([`method`]). A program without the endpoint gets what the table
//! answered before, so nothing changes outside a Linux view.
//!
//! A call is its thread's own: it is forwarded while the thread waits in
//! the kernel, and it ends when the thread has its answer or stops
//! waiting; from then on every method of its capability fails with
//! `gone`, so the capability reaches the process for no longer than the
//! kernel's own work on a call would. A signal cuts short a call still
//! in the endpoint's queue, which then runs again or fails with EINTR, as
//! any call that waits does; one linuxd has taken runs to its answer,
//! which never waits.

use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicBool, Ordering};

use radix_abi::UserBuffer;
use radix_abi::capability::{ObjectKind as AbiKind, SlotFlags};
use radix_abi::endpoint::MESSAGE_WORDS;
use radix_abi::errors::{File, Kernel};
use radix_abi::linux_call::{
    self as abi, COPY_MAX, FORWARDED, GiveArgs, GiveFlags, GiveResults, InfoArgs, InfoResults,
    ReadArgs, ReadStringArgs, ReadStringResults, TakeArgs, TakeResults, WriteArgs,
};
use radix_abi::startup::{SLOT_FD_BASE, SLOT_LINUX};
use radix_cap::{CapError, Derivation, ObjectKind, Rights, SlotRef};
use radix_hal::UserMode;
use radix_ipc::info::Info;
use radix_ipc::range::user_bytes;
use radix_linux::errno::{EIO, ENOSYS};
use radix_mm::Kmalloc;

use super::super::ipc::{self, Message};
use super::super::methods::{Answer, answer};
use super::super::objects::{self, Object};
use super::super::{UserRef, UserThread, cap_error, error};
use super::{Args, Error, Result, files, is, kmalloc, state};
use crate::Arch;
use crate::caps;
use crate::sched::ThreadRef;

/// A forwarded call, which its capability reaches.
pub struct LinuxCall {
    /// The thread that made it, while it lives.
    caller: Weak<UserThread, Kmalloc>,
    /// Its process's number.
    process: u64,
    /// Its thread's number.
    thread: u64,
    /// The user its process acted for when it made the call.
    user: u32,
    /// That user's group.
    group: u32,
    /// The call's number.
    number: u64,
    /// Set once the call has its answer, or its thread stopped waiting.
    ended: AtomicBool,
}

/// A reference to a forwarded call.
pub type CallRef = Arc<LinuxCall, Kmalloc>;

/// A thread's call while it is forwarded: its number, and the call's
/// object once the thread that received it took a capability to it.
pub struct Forwarding {
    number: u64,
    call: Option<CallRef>,
}

impl LinuxCall {
    /// The thread that made the call, while the call lasts: `gone` once it
    /// has ended.
    fn caller(&self) -> super::super::Result<Arc<UserThread, Kmalloc>> {
        if self.ended.load(Ordering::Acquire) {
            return Err(error(Kernel::Gone));
        }
        self.caller.upgrade().ok_or(error(Kernel::Gone))
    }
}

/// Forwards call `number` with `args`, which the table does not answer,
/// made by `me`, `user`'s thread, to the endpoint in its process's slot
/// SLOT_LINUX, and waits for the answer: what goes in rax. `None` if the
/// slot holds no endpoint the process may send on: the table answers as
/// it does without linuxd.
pub fn forward(me: &ThreadRef, user: &UserRef, number: u64, args: &Args) -> Option<Result<u64>> {
    let (cap, endpoint) = objects::endpoint(user.cspace, SLOT_LINUX, Rights::SEND).ok()?;
    let mut words = [0u64; MESSAGE_WORDS as usize];
    words[0] = number;
    words[1..7].copy_from_slice(args);
    let info = Info { label: FORWARDED, words: 7, caps: 0, move_caps: false, no_block: false };
    // Without the grant right: the answer carries no capability into the
    // program, which takes what linuxd gives it only as descriptors.
    let message =
        Message { info, words, badge: cap.badge(), call: true, grant: false, cspace: user.cspace };
    *user.linux.forwarded.lock() = Some(Forwarding { number, call: None });
    let sent = ipc::send(me, user, &endpoint, message);
    // Answered or not, the call ends here: its capability reaches the
    // process no longer. The object goes once the lock has.
    let ended = user.linux.forwarded.lock().take();
    if let Some(Forwarding { call: Some(call), .. }) = ended {
        call.ended.store(true, Ordering::Release);
    }
    Some(match sent {
        // The answer is word 0; a reply without it answers nothing.
        Ok(Some(delivery)) => match Info::decode(delivery.info) {
            Ok(info) if info.words > 0 => Ok(delivery.words[0]),
            _ => Err(Error::Errno(EIO)),
        },
        // linuxd has gone: as if it never was.
        Err(status) if is(status, Kernel::Gone) => Err(Error::Errno(ENOSYS)),
        // Interrupted in the queue: runs again, or EINTR (`super::call`).
        Err(status) => Err(Error::Native(status)),
        Ok(None) => Err(Error::Errno(ENOSYS)),
    })
}

/// radix-process(2)'s `forwarded_call`: puts in `user`'s slot `to` a
/// capability to the forwarded call its thread received and holds for
/// its reply. `no_caller` if it holds none, `wrong_kind` if the call it
/// holds is not one the kernel forwarded.
pub fn claim(user: &UserRef, to: u32) -> super::super::Result<()> {
    let caller = ipc::held_caller(user).ok_or(error(Kernel::NoCaller))?;
    let caller_user = caller.user().ok_or(error(Kernel::WrongKind))?.clone();
    let taken = match &*caller_user.linux.forwarded.lock() {
        Some(Forwarding { call: Some(call), .. }) => Ok(call.clone()),
        Some(Forwarding { number, call: None }) => Err(*number),
        None => return Err(error(Kernel::WrongKind)),
    };
    let call = match taken {
        Ok(call) => call,
        Err(number) => {
            // Made with no lock held, and kept unless another thread of
            // linuxd's took the call meanwhile.
            let made = made_for(&caller, &caller_user, number)?;
            let kept = match &mut *caller_user.linux.forwarded.lock() {
                Some(forwarding) if forwarding.number == number => {
                    Ok(forwarding.call.get_or_insert_with(|| made.clone()).clone())
                }
                _ => Err(error(Kernel::Gone)),
            };
            kept?
        }
    };
    // Its one capability, without the copy and transfer rights: it never
    // leaves the thread's process.
    let rights = Rights::READ.union(Rights::WRITE).union(Rights::CONTROL);
    let mut object = Some(Object::LinuxCall(call));
    objects::create(SlotRef::new(user.cspace, to), ObjectKind::LinuxCall, rights, &mut object, None)
        .map(drop)
}

/// The object of call `number`, which `caller`, `caller_user`'s thread,
/// forwarded.
fn made_for(
    caller: &ThreadRef,
    caller_user: &Arc<UserThread, Kmalloc>,
    number: u64,
) -> super::super::Result<CallRef> {
    let identity = caller_user.process.identity();
    let call = LinuxCall {
        caller: Arc::downgrade(caller_user),
        process: caller_user.process.number(),
        thread: u64::from(state::tid(caller)),
        user: identity.user,
        group: identity.group,
        number,
        ended: AtomicBool::new(false),
    };
    Arc::try_new_in(call, kmalloc()).map_err(|_| error(Kernel::NoMemory))
}

/// The bytes copied through the kernel's stack at a time.
const BOUNCE: usize = 1024;

/// The linux_call interface, on the capability in `user`'s slot `slot`.
pub fn method(
    user: &UserRef,
    slot: u32,
    label: u32,
    args: &[u64; 8],
) -> super::super::Result<Answer> {
    let needs = match label {
        abi::INFO => Rights::NONE,
        abi::READ | abi::READ_STRING => Rights::READ,
        abi::WRITE => Rights::WRITE,
        abi::GIVE | abi::TAKE => Rights::CONTROL,
        _ => return Err(error(Kernel::NoSuchMethod)),
    };
    let (_, Object::LinuxCall(call)) =
        objects::lookup(user.cspace, slot, ObjectKind::LinuxCall, needs)?
    else {
        return Err(error(Kernel::Gone));
    };
    let caller = call.caller()?;
    match label {
        abi::INFO => {
            InfoArgs::decode(args)?;
            let results = InfoResults {
                process: call.process,
                thread: call.thread,
                user: call.user,
                group: call.group,
                number: call.number,
            };
            answer(label, InfoResults::WORDS, results.encode())
        }
        abi::READ => {
            let a = ReadArgs::decode(args)?;
            read(&caller, a.address, a.buffer)?;
            answer(label, 0, [0; 8])
        }
        abi::WRITE => {
            let a = WriteArgs::decode(args)?;
            write(&caller, a.address, a.buffer)?;
            answer(label, 0, [0; 8])
        }
        abi::READ_STRING => {
            let a = ReadStringArgs::decode(args)?;
            let len = read_string(&caller, a.address, a.buffer)?;
            answer(label, ReadStringResults::WORDS, ReadStringResults { len }.encode())
        }
        abi::GIVE => {
            let a = GiveArgs::decode(args)?;
            let close_on_exec = a.flags.contains(GiveFlags::CLOSE_ON_EXEC);
            let descriptor = give(user, &caller, a.from, a.lowest, close_on_exec)?;
            answer(label, GiveResults::WORDS, GiveResults { descriptor }.encode())
        }
        abi::TAKE => {
            let a = TakeArgs::decode(args)?;
            let kind = take(user, &caller, a.descriptor, a.to)?;
            answer(label, TakeResults::WORDS, TakeResults { kind }.encode())
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}

/// Checks a method's buffer in the calling program: at most COPY_MAX
/// bytes, all of them its own.
fn checked(buffer: UserBuffer) -> super::super::Result<usize> {
    if buffer.len > COPY_MAX {
        return Err(Kernel::InvalidArgument.status(1));
    }
    if !user_bytes(buffer.address, buffer.len) {
        return Err(error(Kernel::BadAddress));
    }
    Ok(buffer.len as usize)
}

/// `read`: `buffer`'s length in bytes from the caller's memory at
/// `address`, into `buffer`, through the kernel's stack.
fn read(caller: &UserThread, address: u64, buffer: UserBuffer) -> super::super::Result<()> {
    let len = checked(buffer)?;
    let mut bounce = [0u8; BOUNCE];
    let mut done = 0;
    while done < len {
        let n = BOUNCE.min(len - done);
        let from = address.checked_add(done as u64).ok_or(error(Kernel::BadAddress))?;
        if !caller.space().read_other(from, &mut bounce[..n])
            || !Arch::copy_to_user(buffer.address + done as u64, &bounce[..n])
        {
            return Err(error(Kernel::BadAddress));
        }
        done += n;
    }
    Ok(())
}

/// `write`: `buffer` into the caller's memory at `address`, through the
/// kernel's stack.
fn write(caller: &UserThread, address: u64, buffer: UserBuffer) -> super::super::Result<()> {
    let len = checked(buffer)?;
    let mut bounce = [0u8; BOUNCE];
    let mut done = 0;
    while done < len {
        let n = BOUNCE.min(len - done);
        let to = address.checked_add(done as u64).ok_or(error(Kernel::BadAddress))?;
        if !Arch::copy_from_user(&mut bounce[..n], buffer.address + done as u64)
            || !caller.space().write_other(to, &bounce[..n])
        {
            return Err(error(Kernel::BadAddress));
        }
        done += n;
    }
    Ok(())
}

/// `read_string`: the string at `address` in the caller's memory, without
/// its NUL, into `buffer`; its length. Read a page's part at a time, so
/// that a string that ends just before memory the caller does not have
/// is read whole.
fn read_string(caller: &UserThread, address: u64, buffer: UserBuffer) -> super::super::Result<u64> {
    let len = checked(buffer)?;
    let page = radix_abi::address_space::PAGE_SIZE as usize;
    let mut bounce = [0u8; BOUNCE];
    let mut done = 0;
    while done < len {
        let from = address.checked_add(done as u64).ok_or(error(Kernel::BadAddress))?;
        let n = BOUNCE.min(len - done).min(page - from as usize % page);
        if !caller.space().read_other(from, &mut bounce[..n]) {
            return Err(error(Kernel::BadAddress));
        }
        let end = bounce[..n].iter().position(|&byte| byte == 0);
        let keep = end.unwrap_or(n);
        if !Arch::copy_to_user(buffer.address + done as u64, &bounce[..keep]) {
            return Err(error(Kernel::BadAddress));
        }
        done += keep;
        if end.is_some() {
            return Ok(done as u64);
        }
    }
    Err(File::NameTooLong.status(0))
}

/// `give`: a copy of the capability in `user`'s slot `from` into the
/// caller's lowest free descriptor from `lowest`, below its limit of open
/// files, closed on exec if `close_on_exec`; the descriptor. Another
/// descriptor if a thread of the caller's took the one found first.
fn give(
    user: &UserRef,
    caller: &Arc<UserThread, Kmalloc>,
    from: u32,
    lowest: u32,
    close_on_exec: bool,
) -> super::super::Result<u32> {
    let from = SlotRef::new(user.cspace, from);
    let flags = if close_on_exec { SlotFlags::CLOSE_ON_EXEC.bits() as u8 } else { 0 };
    for _ in 0..files::TAKEN_TRIES {
        let Ok(slot) = files::free_descriptor(caller, lowest) else {
            return Err(error(Kernel::NoRoom));
        };
        let to = SlotRef::new(caller.cspace, slot);
        match caps::derive(from, to, Derivation::COPY) {
            Err(CapError::Occupied) => continue,
            Err(err) => return Err(cap_error(err)),
            Ok(()) => {}
        }
        // A slot keeps no flags once emptied; set them all the same, as
        // the descriptor is new to the program.
        caps::store().and_then(|store| store.set_slot_flags(to, flags)).map_err(cap_error)?;
        return Ok(slot - SLOT_FD_BASE);
    }
    Err(error(Kernel::NoRoom))
}

/// `take`: a copy of the capability of the caller's descriptor
/// `descriptor`, with its rights, into `user`'s slot `to`; its kind.
fn take(
    user: &UserRef,
    caller: &UserThread,
    descriptor: u32,
    to: u32,
) -> super::super::Result<AbiKind> {
    let slot = SLOT_FD_BASE.checked_add(descriptor).ok_or(error(Kernel::NoSuchSlot))?;
    let from = SlotRef::new(caller.cspace, slot);
    let kind = caps::get(from).map_err(cap_error)?.kind();
    caps::derive(from, SlotRef::new(user.cspace, to), Derivation::COPY).map_err(cap_error)?;
    AbiKind::from_raw(kind as u8).ok_or(error(Kernel::WrongKind))
}
