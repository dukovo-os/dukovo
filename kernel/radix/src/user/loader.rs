// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Loading programs in the kernel: Spark (`init`) and the self-tests' test
//! programs (`launch`). Every other program is loaded by radix-rt in user
//! space, the same way (radix-startup(2)).
//!
//! [`start`] makes a process and gives it what radix-startup(2) promises:
//!
//! - its thread, address space, a memory object to make frames from, the
//!   random number generator, the kernel log and its own process (read
//!   and control rights) in the slots radix-startup(2) lists, and from
//!   `SLOT_FIRST_EXTRA` on the capabilities its starter passes it;
//! - its image's segments, placed at random in the image window when the
//!   program is position-independent; a stack whose top is a random page
//!   below `STACK_TOP` for such a program, at `STACK_TOP` for a fixed one;
//!   and the read-only start-up page at `STARTUP_ADDRESS`, with its
//!   arguments;
//! - one thread, at the image's entry point with the start-up page's
//!   address as its argument.
//!
//! The frames for the image, the stack and the start-up page are kept in
//! the process's hidden capability space, so the program cannot pull its
//! own code from under itself by accident; they come from the program's
//! own memory object, which says how much of it they took.
//!
//! No program gets the boot archive mapped: Spark and everything after it
//! read its files through `/system`, which the kernel serves from the boot
//! loader's copy (ADR 0392).

use radix_abi::Status;
use radix_abi::address_space::{PAGE_SIZE, USER_START};
use radix_abi::errors::Kernel;
use radix_abi::startup::{self, Startup};
use radix_cap::{Derivation, ObjectKind, Rights, SlotRef};
use radix_elf::{Access, Kind};
use radix_hal::{UserAccess, UserMode};
use radix_klog::warn;

use super::account::Account;
use super::memory::{FrameObject, FrameRef, SpaceRef};
use super::methods::retype;
use super::objects::{self, Object};
use super::process::{self, ProcessObject, ProcessRef, Start};
use super::{Result, cap_error, error};
use crate::domains::DomainRef;
use crate::{Arch, caps};

/// The most capabilities a program gets from its starter: the start-up
/// page says what each is.
pub const EXTRA_MAX: usize = 16;

/// The hidden slots of the image, stack and start-up page frames.
const KEPT_STACK: u32 = 1;
const KEPT_IMAGE: u32 = 2;
const KEPT_PAGE: u32 = 3;

/// A program to start.
pub struct Program<'a> {
    /// Its name, which its process and thread take.
    pub name: &'a str,
    /// Its image, an ELF64 executable.
    pub image: &'a [u8],
    /// The CPU its thread is pinned to, if it is.
    pub cpu: Option<usize>,
    /// Its memory object's size: 2^order pages.
    pub memory_order: u32,
    /// What it is started to do, for test programs with many roles.
    pub role: u64,
    /// A word for it.
    pub argument: u64,
    /// Its arguments, the first its path; they must fit in the start-up
    /// page.
    pub args: &'a [&'a str],
    /// The capabilities it gets, from `SLOT_FIRST_EXTRA` on: where each
    /// is, the rights to keep, and a badge (0 for none).
    pub extra: &'a [(SlotRef, Rights, u64)],
    /// Whether it gets the root of the file tree, with every right, in
    /// `SLOT_ROOT`: Spark does.
    pub root: bool,
    /// The domain its threads run in and its objects are charged to.
    pub domain: DomainRef,
    /// Called once it has exited.
    pub on_exit: Option<fn(&ProcessObject, u64)>,
}

/// Why starting failed, logged.
fn failed(name: &str, what: &str, status: Status) -> Status {
    warn!("User: could not start {name}: {what} failed with {:#x}", status.0);
    status
}

/// Starts `program` in a new process whose original capability goes in
/// `at`. On a failure, the process made so far is ended.
pub fn start(at: SlotRef, program: &Program<'_>) -> Result<ProcessRef> {
    let name = program.name;
    if program.extra.len() > EXTRA_MAX {
        return Err(failed(name, "checking its capabilities", error(Kernel::InvalidArgument)));
    }
    let account = Account::new(program.domain.clone()).ok_or(error(Kernel::NoMemory))?;
    let process =
        process::create(Some(at), name, startup::SLOTS, account, None, None, program.on_exit)
            .map_err(|err| failed(name, "making its process", err))?;
    match build(at, &process, program) {
        Ok(()) => Ok(process),
        Err(err) => {
            process.exit(radix_abi::process::EXIT_STOPPED);
            Err(err)
        }
    }
}

/// Fills `process`, whose original capability is in `at`, and starts it.
fn build(at: SlotRef, process: &ProcessRef, program: &Program<'_>) -> Result<()> {
    let name = program.name;
    let cspace = process.cspace();
    let keep = process.keep_space();
    let mine = |slot| SlotRef::new(cspace, slot);
    let kept = |slot| SlotRef::new(keep, slot);
    let space = process.space().ok_or(error(Kernel::Gone))?;

    let image = radix_elf::read(program.image).map_err(|err| {
        warn!("User: {name} cannot run: {err}");
        error(Kernel::InvalidArgument)
    })?;
    // A fixed program runs where it was linked, below its stack; a
    // position-independent one at a random place in the image window.
    let (range, stack_top) = match image.kind {
        Kind::Fixed => {
            (USER_START + PAGE_SIZE..startup::STACK_TOP - startup::STACK_SIZE, startup::STACK_TOP)
        }
        Kind::Relocatable => {
            let pages = startup::STACK_WINDOW / PAGE_SIZE;
            let top = startup::STACK_TOP - crate::user::random::place(pages + 1) * PAGE_SIZE;
            (startup::IMAGE_BASE..startup::IMAGE_BASE + startup::WINDOW, top)
        }
    };
    let placed = image
        .random_bias(range.clone(), crate::user::random::place_bits())
        .and_then(|bias| image.place(bias, range))
        .map_err(|err| {
            warn!("User: {name} does not fit: {err}");
            error(Kernel::InvalidArgument)
        })?;

    // The capabilities radix-startup(2) lists, but its thread.
    let derive = |from: SlotRef, to: SlotRef, rights: Rights, badge: u64| {
        let how = Derivation { rights, badge: (badge != 0).then_some(badge), range: None };
        caps::derive(from, to, how).map_err(cap_error)
    };
    let all = |kind: ObjectKind| kind.rights();
    derive(kept(0), mine(startup::SLOT_SPACE), all(ObjectKind::AddressSpace), 0)
        .map_err(|err| failed(name, "giving it its address space", err))?;
    derive(at, mine(startup::SLOT_PROCESS), Rights::READ.union(Rights::CONTROL), 0)
        .map_err(|err| failed(name, "giving it its process", err))?;
    caps::create_memory(mine(startup::SLOT_MEMORY), program.memory_order)
        .map_err(|err| failed(name, "making its memory", cap_error(err)))?;
    process::install(mine(startup::SLOT_RANDOM), ObjectKind::Random, Object::Random)?;
    process::install(mine(startup::SLOT_LOG), ObjectKind::KernelLog, Object::KernelLog)?;
    if program.root {
        let root = crate::fs::vfs().ok_or(error(Kernel::Gone))?.root();
        let root = crate::fs::FileObject::new(root, super::names::Identity::ROOT)
            .ok_or(error(Kernel::NoMemory))?;
        process::install(mine(startup::SLOT_ROOT), ObjectKind::Directory, Object::Directory(root))
            .map_err(|err| failed(name, "giving it the root directory", err))?;
    }
    let mut kinds = [0u8; EXTRA_MAX];
    for (i, &(from, rights, badge)) in program.extra.iter().enumerate() {
        derive(from, mine(startup::SLOT_FIRST_EXTRA + i as u32), rights, badge)
            .map_err(|err| failed(name, "passing it capabilities", err))?;
        let cap = caps::get(from).map_err(cap_error)?;
        kinds[i] = cap.kind() as u8;
    }

    // Frames from its memory, kept hidden: the stack, the image and the
    // start-up page, largest first so that none wastes memory on
    // alignment.
    let stack_pages = startup::STACK_SIZE / PAGE_SIZE;
    let image_order = image.pages().next_power_of_two().trailing_zeros();
    let mut frames =
        [(stack_pages.trailing_zeros(), KEPT_STACK), (image_order, KEPT_IMAGE), (0, KEPT_PAGE)];
    frames.sort_unstable_by_key(|frame| core::cmp::Reverse(frame.0));
    for (order, slot) in frames {
        retype(mine(startup::SLOT_MEMORY), ObjectKind::Frame, order, kept(slot), 1)
            .map_err(|err| failed(name, "making its frames", err))?;
    }
    let frame = |slot| objects::frame(keep, slot, Rights::NONE).map(|(_, frame)| frame);
    let (stack, code, page) = (frame(KEPT_STACK)?, frame(KEPT_IMAGE)?, frame(KEPT_PAGE)?);

    load(&placed, program.image, &code, &space)
        .map_err(|err| failed(name, "loading its image", err))?;
    let stack_bottom = stack_top - startup::STACK_SIZE;
    FrameObject::map_into(&stack, &space, stack_bottom, 0, stack_pages, UserAccess::ReadWrite)
        .map_err(|err| failed(name, "mapping its stack", err))?;
    FrameObject::map_into(&page, &space, startup::STARTUP_ADDRESS, 0, 1, UserAccess::Read)
        .map_err(|err| failed(name, "mapping its start-up page", err))?;

    let used = caps::store()
        .ok()
        .and_then(|store| {
            let cap = store.get(mine(startup::SLOT_MEMORY)).ok()?;
            Some(store.object(cap.id()?)?.watermark)
        })
        .unwrap_or(0);
    let mut bytes = [0u8; PAGE_SIZE as usize];
    let mut put = |offset: usize, value: &[u8]| {
        bytes[offset..offset + value.len()].copy_from_slice(value);
    };
    put(Startup::OFFSET_VERSION, &startup::STARTUP_VERSION.to_le_bytes());
    let cpu = program.cpu.map_or(startup::ANY_CPU, |cpu| cpu as u32);
    put(Startup::OFFSET_CPU, &cpu.to_le_bytes());
    put(Startup::OFFSET_ROLE, &program.role.to_le_bytes());
    put(Startup::OFFSET_ARGUMENT, &program.argument.to_le_bytes());
    put(Startup::OFFSET_TSC_HZ, &Arch::cycle_counter_hz().to_le_bytes());
    put(Startup::OFFSET_EXTRA, &(program.extra.len() as u32).to_le_bytes());
    put(Startup::OFFSET_SLOTS, &kinds);
    let free = ((1u64 << program.memory_order) - used) * PAGE_SIZE;
    put(Startup::OFFSET_MEMORY_BYTES, &free.to_le_bytes());
    put(Startup::OFFSET_IMAGE_BIAS, &placed.bias().to_le_bytes());
    // The boot archive's address and size stay 0: no program gets it
    // mapped (ADR 0392).
    // The arguments, each ending with a NUL, after the fields.
    let mut at_byte = Startup::SIZE;
    for arg in program.args {
        let end = at_byte + arg.len() + 1;
        if arg.contains('\0') || end > PAGE_SIZE as usize {
            return Err(failed(name, "passing it its arguments", error(Kernel::InvalidArgument)));
        }
        put(at_byte, arg.as_bytes());
        at_byte = end;
    }
    put(Startup::OFFSET_ARGS_OFFSET, &(Startup::SIZE as u32).to_le_bytes());
    put(Startup::OFFSET_ARGS_SIZE, &((at_byte - Startup::SIZE) as u32).to_le_bytes());

    let start = [placed.entry(), stack_top, startup::STARTUP_ADDRESS];
    ProcessObject::start(
        process,
        Start::Fresh(start),
        startup::SLOT_THREAD,
        program.cpu,
        None,
        0,
        &mut |thread| {
            bytes[Startup::OFFSET_THREAD_ID..][..4]
                .copy_from_slice(&(thread.id() as u32).to_le_bytes());
            page.write(0, &bytes)
        },
    )
    .map_err(|err| failed(name, "starting its thread", err))?;
    Ok(())
}

/// Copies the image into `code`, one segment after another, relocated, and
/// maps it into `space`.
fn load(
    placed: &radix_elf::Placed<'_>,
    bytes: &[u8],
    code: &FrameRef,
    space: &SpaceRef,
) -> Result<()> {
    let mut written = Ok(());
    placed
        .load(bytes, &mut |offset, data| {
            if written.is_ok() {
                written = code.write(offset, data);
            }
        })
        .map_err(|err| {
            warn!("User: an image cannot be loaded: {err}");
            error(Kernel::InvalidArgument)
        })?;
    written?;
    for mapping in placed.mappings().into_iter().flatten() {
        let access = match mapping.access {
            Access::Read => UserAccess::Read,
            Access::ReadWrite => UserAccess::ReadWrite,
            Access::ReadExecute => UserAccess::ReadExecute,
        };
        FrameObject::map_into(code, space, mapping.address, mapping.page, mapping.pages, access)?;
    }
    Ok(())
}
