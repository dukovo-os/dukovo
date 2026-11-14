// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Crash records (ADR 0530, radix-crashes(4)): what the kernel keeps of a
//! thread whose crash ends its process, for reportd(8), which turns it into
//! a stack trace and keeps nothing of the memory it holds.
//!
//! A record is made in the thread that crashed, before its process ends,
//! while its address space is still its own: the Header, then a Mapping
//! for each part of a file the process had mapped to run, then the stack
//! from the stack pointer up. The stack is copied a page at a time and
//! ends at the first page that is not there, at the end of its region, or
//! at STACK_MAX bytes. The record goes to [`crate::fs::crashes`], whose
//! queue reportd reads. Memory short for the record keeps none: the
//! kernel's log has the crash's line either way.

use radix_abi::address_space::PAGE_SIZE;
use radix_abi::crash::{
    DOMAIN_MAX, Header, MAGIC, MAPPINGS_MAX, Mapping, PATH_CUT, PATH_UNKNOWN, RECORD_MAX,
    STACK_MAX, VERSION,
};
use radix_hal::{Timer, UserMode};
use radix_mm::{Flags, HEAP, Kmalloc};

use super::UserThread;
use crate::fs::Opened;
use crate::sched::{self, ThreadRef};
use crate::{Arch, fs};

const _: () = assert!(
    Header::SIZE + MAPPINGS_MAX as usize * Mapping::SIZE + STACK_MAX as usize
        == RECORD_MAX as usize
);
const _: () = assert!(radix_abi::resource_domain::NAME_MAX == DOMAIN_MAX as u64);

/// How many of a process's ancestors a record names.
const ANCESTORS: usize = 4;

/// Keeps a record of `me`, a thread of `user`'s process, which `signal`
/// with `code` (and `address`, for a fault) is ending; nothing for Spark,
/// with records turned off, or without memory for one.
pub fn keep(me: &ThreadRef, user: &UserThread, signal: u32, code: i32, address: u64) {
    let process = &user.process;
    if process.is_vital() || !crate::settings().crash_records {
        return;
    }
    let Some(registers) = Arch::user_registers() else { return };
    let [pc, sp, fp] = Arch::frame_registers(&registers);
    let space = &user.space;
    // The stack's room: to its region's end, at most STACK_MAX.
    let stack_end = space.region_end(sp).unwrap_or(sp).min(sp.saturating_add(u64::from(STACK_MAX)));
    let stack_room = (stack_end - sp) as usize;
    let Ok(mut record) = radix_mm::bytes::zeroed(RECORD_MAX as usize, heap()) else { return };

    // The mappings, straight after the header.
    let mut count = 0usize;
    space.each_running_file(&mut |start, end, offset, name| {
        if count == MAPPINGS_MAX as usize {
            return;
        }
        let at = Header::SIZE + count * Mapping::SIZE;
        let mapping = &mut record[at..at + Mapping::SIZE];
        put(mapping, Mapping::OFFSET_START, &start.to_le_bytes());
        put(mapping, Mapping::OFFSET_END, &end.to_le_bytes());
        put(mapping, Mapping::OFFSET_OFFSET, &offset.to_le_bytes());
        put_path(
            mapping,
            Mapping::OFFSET_PATH_FLAGS,
            Mapping::OFFSET_PATH_LEN,
            Mapping::OFFSET_PATH,
            name,
        );
        count += 1;
    });

    // The stack, a page at a time, to the first that is not there.
    let stack_at = Header::SIZE + count * Mapping::SIZE;
    let mut copied = 0usize;
    while copied < stack_room {
        let from = sp + copied as u64;
        let chunk = ((PAGE_SIZE - from % PAGE_SIZE) as usize).min(stack_room - copied);
        let to = &mut record[stack_at + copied..stack_at + copied + chunk];
        if !Arch::copy_from_user(to, from) {
            to.fill(0);
            break;
        }
        copied += chunk;
    }
    let size = stack_at + copied;
    record.truncate(size);

    // The header, last, with what the rest came to.
    let identity = process.identity();
    let header = &mut record[..Header::SIZE];
    put(header, Header::OFFSET_MAGIC, &MAGIC.to_le_bytes());
    put(header, Header::OFFSET_VERSION, &VERSION.to_le_bytes());
    put(header, Header::OFFSET_SIZE, &(size as u32).to_le_bytes());
    put(header, Header::OFFSET_SIGNAL, &signal.to_le_bytes());
    put(header, Header::OFFSET_CODE, &code.to_le_bytes());
    put(header, Header::OFFSET_MAPPINGS, &(count as u32).to_le_bytes());
    put(header, Header::OFFSET_ADDRESS, &address.to_le_bytes());
    put(header, Header::OFFSET_PROCESS, &process.number().to_le_bytes());
    put(header, Header::OFFSET_THREAD, &me.id().to_le_bytes());
    put(header, Header::OFFSET_USER, &identity.user.to_le_bytes());
    put(header, Header::OFFSET_GROUP, &identity.group.to_le_bytes());
    let mut pid = process.number();
    for at in 0..ANCESTORS {
        let Some((parent, ..)) = super::posix::tree_place(pid).filter(|p| p.0 != 0) else { break };
        put(header, Header::OFFSET_ANCESTORS + at * 8, &parent.to_le_bytes());
        pid = parent;
    }
    let wall_ns = Arch::wall_clock_ns_without_waiting().unwrap_or(0);
    put(header, Header::OFFSET_WALL_NS, &wall_ns.to_le_bytes());
    put(header, Header::OFFSET_BOOT_NS, &sched::now_ns().to_le_bytes());
    let name = me.name();
    let name = name.as_str().as_bytes();
    let name = &name[..name.len().min(16)];
    put(header, Header::OFFSET_THREAD_NAME, name);
    put(header, Header::OFFSET_PC, &pc.to_le_bytes());
    put(header, Header::OFFSET_SP, &sp.to_le_bytes());
    put(header, Header::OFFSET_FP, &fp.to_le_bytes());
    put(header, Header::OFFSET_STACK_ADDRESS, &sp.to_le_bytes());
    put(header, Header::OFFSET_STACK_LEN, &(copied as u32).to_le_bytes());
    let image = process.posix.lock().image.clone();
    put_path(
        header,
        Header::OFFSET_PATH_FLAGS,
        Header::OFFSET_PATH_LEN,
        Header::OFFSET_PATH,
        image.as_deref(),
    );
    drop(image);
    let domain = process.account().domain().name().as_str().as_bytes();
    put(header, Header::OFFSET_DOMAIN_LEN, &(domain.len() as u32).to_le_bytes());
    put(header, Header::OFFSET_DOMAIN, domain);
    fs::crashes::keep(record);
}

/// The heap, for a record: it may wait for memory, as the thread is on its
/// way out and holds no lock.
fn heap() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// Writes `bytes` into `place` at `at`.
fn put(place: &mut [u8], at: usize, bytes: &[u8]) {
    place[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Writes `path`'s flags, length and bytes into `place` at the offsets
/// given: PATH_UNKNOWN for none, PATH_CUT for one whose end was cut off.
fn put_path(
    place: &mut [u8],
    flags_at: usize,
    len_at: usize,
    path_at: usize,
    path: Option<&Opened>,
) {
    let (bytes, flags) = match path.and_then(Opened::path) {
        Some((bytes, cut)) => (bytes, if cut { PATH_CUT } else { 0 }),
        None => (&[][..], PATH_UNKNOWN),
    };
    put(place, flags_at, &flags.to_le_bytes());
    put(place, len_at, &(bytes.len() as u32).to_le_bytes());
    put(place, path_at, bytes);
}
