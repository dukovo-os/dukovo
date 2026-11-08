// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firmware's runtime services under UEFI (Phase 14, ADR 0313): its
//! variables, where its boot manager keeps the entries it starts systems
//! from, as `efiboot(8)` writes them.
//!
//! GRUB ends the firmware's boot services before it starts the kernel and
//! passes the UEFI memory map. The runtime services stay callable after
//! that in the firmware's own physical addressing, since nothing calls
//! `SetVirtualAddressMap`, as long as the memory the map marks
//! `EFI_MEMORY_RUNTIME` is mapped where it is. The lower half of every
//! address space is its program's, so the kernel keeps a page table for
//! the firmware alone: the kernel's half, as every address space has it,
//! and the runtime regions mapped where they are
//! ([`paging::firmware_table`]).
//!
//! A call ([`call`]) holds a lock, so that one CPU at a time is in the
//! firmware, which also holds off preemption. It saves the running
//! thread's FPU registers and loads their initial state, which UEFI
//! requires (x87 control word 0x37f, MXCSR 0x1f80), switches CR3 to the
//! firmware's table (PCID 0, flushed), and calls through a trampoline
//! that moves to a stack of the firmware's own, 64 KiB with a guard page
//! below, and makes the Microsoft x64 call that UEFI uses. Interrupts stay
//! on, as Linux keeps them, since writing flash may take a while: their
//! handlers run on any stack in the kernel's half. Coming back it loads
//! the CR3 it had without keeping that PCID's TLB entries, since a
//! shootdown that came meanwhile flushed the firmware's PCID instead, and
//! gives the thread its FPU registers back.
//!
//! Firmware has bugs. A fault in its code (not in the kernel's, which an
//! interrupt handler may be running) resumes at the trampoline's fault
//! label ([`fault`]), which returns as the call would, and the kernel
//! calls the firmware no more until the machine restarts, as Linux does:
//! the firmware may have stopped holding a lock of its own.

use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use radix_acpi::Memory;
use radix_bootinfo::{EfiMemoryDescriptor, EfiMemoryMap, efi_memory_type};
use radix_hal::{MemoryKind, VariableError, VariableSpace};
use radix_sync::{Once, SpinLock};

use crate::acpi::PhysicalMemory;
use crate::interrupts::TrapFrame;
use crate::paging::{self, Cache};
use crate::{cpu, fpu, memory, percpu};

/// The most runtime regions the kernel maps: firmware has a few dozen.
const MAX_REGIONS: usize = 128;

/// The most memory the runtime regions may take, all told: firmware's
/// take a few MiB, and its flash's registers 16 MiB more.
const MAX_MAPPED: u64 = 1 << 30;

/// Pages of the firmware's stack: 64 KiB. Linux calls it on a 16 KiB
/// kernel stack; this one starts empty and is the firmware's alone.
const STACK_PAGES: u64 = 16;

/// The UEFI system table's signature, `IBI SYST`.
const SYSTEM_TABLE: u64 = 0x5453_5953_2049_4249;

/// The runtime services table's signature, `RUNTSERV`.
const RUNTIME_SERVICES: u64 = 0x5652_4553_544e_5552;

/// Bytes of the system table the kernel reads: up to and with its
/// pointer to the runtime services.
const SYSTEM_TABLE_SIZE: usize = 96;

/// Bytes of a UEFI 2 runtime services table, `QueryVariableInfo` the last
/// service the kernel uses.
const RUNTIME_SERVICES_SIZE: usize = 136;

/// Where the services the kernel calls are in the runtime services table
/// (UEFI 2.10, section 4.5).
mod offset {
    pub const GET_VARIABLE: usize = 72;
    pub const GET_NEXT_VARIABLE_NAME: usize = 80;
    pub const SET_VARIABLE: usize = 88;
    pub const QUERY_VARIABLE_INFO: usize = 128;
}

/// The bit of a UEFI status that makes it an error rather than a warning.
const ERROR: u64 = 1 << 63;

/// The UEFI statuses the kernel tells apart, without [`ERROR`].
mod status {
    pub const INVALID_PARAMETER: u64 = 2;
    pub const UNSUPPORTED: u64 = 3;
    pub const BUFFER_TOO_SMALL: u64 = 5;
    pub const DEVICE_ERROR: u64 = 7;
    pub const WRITE_PROTECTED: u64 = 8;
    pub const OUT_OF_RESOURCES: u64 = 9;
    pub const NOT_FOUND: u64 = 14;
    pub const SECURITY_VIOLATION: u64 = 26;
}

/// What the kernel found of the runtime services, and what it made to
/// call them.
struct Runtime {
    /// The firmware's page table.
    pml4: u64,
    /// The top of the firmware's stack.
    stack_top: u64,
    /// `GetVariable`.
    get_variable: u64,
    /// `GetNextVariableName`.
    get_next_variable_name: u64,
    /// `SetVariable`.
    set_variable: u64,
    /// `QueryVariableInfo`, which UEFI 1 firmware lacks.
    query_variable_info: Option<u64>,
}

static RUNTIME: Once<Runtime> = Once::new();

/// Where the running thread's FPU registers wait while the firmware runs:
/// a save area, as every thread has one.
#[repr(C, align(64))]
struct SaveArea([u8; fpu::AREA_MAX]);

/// The lock a call holds: one CPU at a time is in the firmware.
static CALL: SpinLock<SaveArea> = SpinLock::new(SaveArea([0; fpu::AREA_MAX]));

/// The CPU in the firmware, numbered from 1, or 0 when none is.
static IN_CALL: AtomicUsize = AtomicUsize::new(0);

/// Whether the firmware faulted in a call: then the kernel calls it no
/// more.
static FAULTED: AtomicBool = AtomicBool::new(false);

/// The fault's vector and the address of the instruction that took it,
/// for the log.
static FAULT: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// The stack pointer the trampoline goes back to, with the caller's
/// registers above it.
static SAVED_RSP: AtomicU64 = AtomicU64::new(0);

// The trampoline: `radix_efi_call(function, args, stack_top)` calls
// `function` with the five arguments at `args` the Microsoft x64 way:
// the first four in RCX, RDX, R8 and R9, the fifth on the stack above 32
// bytes of home space, the stack 16-byte aligned at the call. It saves
// the registers the System V ABI keeps across calls, and RFLAGS, on the
// caller's stack, and gets back to them through SAVED_RSP, whether the
// firmware returns or faults (`radix_efi_call_fault`, where `fault`
// sends it, returns u64::MAX). Microsoft's convention keeps RSI, RDI and
// XMM6-15 too, so nothing the caller needs is lost.
core::arch::global_asm!(
    r#"
    .section .text
    .balign 64
    .global radix_efi_call
radix_efi_call:
    push rbp
    mov rbp, rsp
    push rbx
    push r12
    push r13
    push r14
    push r15
    pushfq
    mov [rip + {saved}], rsp
    mov rax, rdi
    mov r10, rsi
    mov rsp, rdx
    sub rsp, 48
    mov rcx, [r10]
    mov rdx, [r10 + 8]
    mov r8, [r10 + 16]
    mov r9, [r10 + 24]
    mov r11, [r10 + 32]
    mov [rsp + 32], r11
    call rax
radix_efi_call_resume:
    mov rsp, [rip + {saved}]
    popfq
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
    .global radix_efi_call_fault
radix_efi_call_fault:
    mov rax, -1
    jmp radix_efi_call_resume
"#,
    saved = sym SAVED_RSP,
);

unsafe extern "sysv64" {
    /// Calls the runtime service at `function` with `args` on the stack
    /// whose top is `stack_top`, and returns its status.
    fn radix_efi_call(function: u64, args: *const [u64; 5], stack_top: u64) -> u64;
    static radix_efi_call_fault: u8;
}

/// A GUID where the firmware may read it as the structure it is, whose
/// first field is 32 bits wide.
#[repr(C, align(8))]
struct Guid([u8; 16]);

/// Finds the runtime services: the UEFI system table at `system_table`
/// names them, and `map`, the UEFI memory map, says what they need mapped.
/// Logs what it found, or why the kernel cannot call them. Called once,
/// after the frame allocator starts.
pub fn init(system_table: Option<u64>, map: Option<EfiMemoryMap<'static>>) {
    let Some(address) = system_table else { return };
    let Some(map) = map else {
        radix_klog::info!(
            "EFI: the boot loader passed no UEFI memory map, so the firmware's variables are \
             out of reach"
        );
        return;
    };
    match find(address, map) {
        Ok((runtime, revision, regions, bytes)) => {
            radix_klog::info!(
                "EFI: runtime services of UEFI {}.{}{}, {regions} runtime regions, {} KiB mapped",
                revision >> 16,
                (revision & 0xffff) / 10,
                Minor((revision & 0xffff) % 10),
                bytes >> 10
            );
            let _ = RUNTIME.set(runtime);
        }
        Err(why) => {
            radix_klog::warn!("EFI: {why}, so the firmware's variables are out of reach");
        }
    }
}

/// The third digit of a UEFI revision, as in 2.3.1, shown when not 0.
struct Minor(u32);

impl core::fmt::Display for Minor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == 0 { Ok(()) } else { write!(f, ".{}", self.0) }
    }
}

/// The runtime services the system table at `address` names, with the
/// UEFI revision they claim, how many runtime regions there are and how
/// many bytes they take; or why the kernel cannot call them.
fn find(
    address: u64,
    map: EfiMemoryMap<'static>,
) -> Result<(Runtime, u32, usize, u64), &'static str> {
    let mut regions: [(Range<u64>, Cache); MAX_REGIONS] =
        core::array::from_fn(|_| (0..0, Cache::WriteBack));
    let mut count = 0;
    let mut bytes = 0u64;
    for descriptor in map.descriptors().filter(EfiMemoryDescriptor::is_runtime) {
        let range = descriptor.range().ok_or("a runtime region runs past the end of memory")?;
        if range.is_empty() {
            continue;
        }
        if count == MAX_REGIONS {
            return Err("the firmware has more than 128 runtime regions");
        }
        if in_free_memory(range.clone()) {
            return Err("a runtime region is in memory the boot loader called free");
        }
        bytes = bytes.saturating_add(range.end - range.start);
        regions[count] = (range, cache_of(&descriptor));
        count += 1;
    }
    if count == 0 {
        return Err("the UEFI memory map has no runtime regions");
    }
    if bytes > MAX_MAPPED {
        return Err("the runtime regions take more than 1 GiB");
    }
    let regions = &mut regions[..count];
    regions.sort_unstable_by_key(|(range, _)| range.start);
    if regions.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
        return Err("runtime regions overlap");
    }
    let within = |at: u64, len: usize| {
        let end = at.saturating_add(len as u64);
        regions.iter().any(|(range, _)| range.start <= at && end <= range.end)
    };
    let word = |table: &[u8], at: usize| {
        u64::from_le_bytes(table[at..at + 8].try_into().unwrap_or_default())
    };

    if !within(address, SYSTEM_TABLE_SIZE) {
        return Err("the UEFI system table is not in runtime memory");
    }
    let system = PhysicalMemory.bytes(address, SYSTEM_TABLE_SIZE).ok_or("no system table")?;
    if word(system, 0) != SYSTEM_TABLE {
        return Err("no UEFI system table where the boot loader said");
    }
    let services_at = word(system, 88);
    if !within(services_at, RUNTIME_SERVICES_SIZE) {
        return Err("the runtime services table is not in runtime memory");
    }
    let services =
        PhysicalMemory.bytes(services_at, RUNTIME_SERVICES_SIZE).ok_or("no runtime services")?;
    if word(services, 0) != RUNTIME_SERVICES {
        return Err("the runtime services table has the wrong signature");
    }
    let revision = u32::from_le_bytes(services[8..12].try_into().unwrap_or_default());
    let header_size = u32::from_le_bytes(services[12..16].try_into().unwrap_or_default());
    // Every service must be code the firmware's table maps; one outside
    // would fault at the call.
    let service = |at: usize| Some(word(services, at)).filter(|&code| within(code, 1));
    let (Some(get_variable), Some(get_next_variable_name), Some(set_variable)) = (
        service(offset::GET_VARIABLE),
        service(offset::GET_NEXT_VARIABLE_NAME),
        service(offset::SET_VARIABLE),
    ) else {
        return Err("a variable service is not in runtime memory");
    };
    let query_variable_info =
        if revision >> 16 >= 2 && header_size as usize >= RUNTIME_SERVICES_SIZE {
            service(offset::QUERY_VARIABLE_INFO)
        } else {
            None
        };

    let pml4 =
        paging::firmware_table(regions).map_err(|_| "no memory for the firmware's page table")?;
    let (_guard, stack) = paging::allocate_stack(STACK_PAGES);
    // SAFETY: the save area is 64-byte aligned and AREA_MAX bytes long,
    // and nothing calls the firmware yet, so nothing else uses it.
    unsafe { fpu::init_area(CALL.lock().0.as_mut_ptr() as usize) };
    let runtime = Runtime {
        pml4,
        stack_top: stack.end as u64,
        get_variable,
        get_next_variable_name,
        set_variable,
        query_variable_info,
    };
    Ok((runtime, revision, count, bytes))
}

/// How the firmware's page table caches a runtime region: write-back
/// where the firmware allows it, uncached for its device registers.
fn cache_of(descriptor: &EfiMemoryDescriptor) -> Cache {
    let device = matches!(
        descriptor.kind,
        efi_memory_type::MEMORY_MAPPED_IO | efi_memory_type::MEMORY_MAPPED_IO_PORT_SPACE
    );
    if !device && descriptor.attributes & EfiMemoryDescriptor::WRITE_BACK != 0 {
        Cache::WriteBack
    } else {
        Cache::Uncached
    }
}

/// Whether any of `range` is memory the boot loader's map calls usable,
/// which the kernel's allocator hands out: the firmware must not write
/// there.
fn in_free_memory(range: Range<u64>) -> bool {
    memory::memory_map().iter().any(|region| {
        region.kind == MemoryKind::Usable && region.start < range.end && range.start < region.end
    })
}

/// Where a fault resumes if the firmware's code took it during a call:
/// the trampoline's fault label, which returns from the call. `None` for
/// any other fault, which stays the kernel's to report. A fault in the
/// kernel's own code is the kernel's even during a call: an interrupt
/// handler may be running.
pub fn fault(vector: u8, frame: &TrapFrame) -> Option<u64> {
    let me = percpu::current_index() + 1;
    if IN_CALL.load(Ordering::Relaxed) != me || paging::is_kernel_text(frame.rip as usize) {
        return None;
    }
    FAULTED.store(true, Ordering::Relaxed);
    FAULT[0].store(u64::from(vector), Ordering::Relaxed);
    FAULT[1].store(frame.rip, Ordering::Relaxed);
    Some(&raw const radix_efi_call_fault as u64)
}

/// Calls the runtime service at `function` with `args` (unused ones 0)
/// and returns its UEFI status.
fn call(function: u64, args: [u64; 5]) -> Result<u64, VariableError> {
    let runtime = RUNTIME.get().ok_or(VariableError::NotSupported)?;
    let mut save = CALL.lock();
    if FAULTED.load(Ordering::Relaxed) {
        return Err(VariableError::Faulted);
    }
    let area = save.0.as_mut_ptr() as usize;
    // SAFETY: the area is 64-byte aligned and AREA_MAX bytes long, and
    // only the lock's holder uses it. The lock holds off preemption, so
    // the registers saved are this thread's until they are restored.
    unsafe { fpu::save(area) };
    fpu::load_initial();
    let cr3 = cpu::cr3();
    IN_CALL.store(percpu::current_index() + 1, Ordering::Relaxed);
    // SAFETY: the firmware's table maps the kernel's half as every
    // address space does, so this code, its stack and every static stay
    // where they are, and the runtime regions where the firmware expects
    // them. `function` is a runtime service the system table named, in
    // memory that table maps; `args` point into the kernel's half. The
    // trampoline comes back here even if the firmware faults. CR3 as it
    // reads has bit 63 clear, so loading it again flushes its PCID.
    let status = unsafe {
        cpu::write_cr3(runtime.pml4);
        let status = radix_efi_call(function, &raw const args, runtime.stack_top);
        cpu::write_cr3(cr3);
        status
    };
    IN_CALL.store(0, Ordering::Relaxed);
    // SAFETY: as for the save above; the area holds what it saved.
    unsafe { fpu::restore(area) };
    drop(save);
    if FAULTED.load(Ordering::Relaxed) {
        radix_klog::error!(
            "EFI: the firmware faulted (vector {}) at {:#x} in a runtime service; the kernel \
             calls it no more until the machine restarts",
            FAULT[0].load(Ordering::Relaxed),
            FAULT[1].load(Ordering::Relaxed)
        );
        return Err(VariableError::Faulted);
    }
    Ok(status)
}

/// The error a UEFI status stands for, or `Ok` for success and for the
/// warnings, which have [`ERROR`] clear.
fn check(value: u64) -> Result<(), VariableError> {
    if value & ERROR == 0 {
        return Ok(());
    }
    Err(match value & !ERROR {
        status::INVALID_PARAMETER => VariableError::InvalidParameter,
        status::UNSUPPORTED => VariableError::NotSupported,
        status::BUFFER_TOO_SMALL => VariableError::TooSmall(0),
        status::DEVICE_ERROR => VariableError::Device,
        status::WRITE_PROTECTED => VariableError::WriteProtected,
        status::OUT_OF_RESOURCES => VariableError::NoSpace,
        status::NOT_FOUND => VariableError::NotFound,
        status::SECURITY_VIOLATION => VariableError::SecurityViolation,
        _ => VariableError::Other(value),
    })
}

/// The address the firmware gets for `bytes`: none for an empty buffer,
/// as UEFI asks when its size is 0.
fn pointer(bytes: &[u8]) -> u64 {
    if bytes.is_empty() { 0 } else { bytes.as_ptr() as u64 }
}

/// A name the firmware can read: UCS-2 that ends in its only NUL.
fn check_name(name: &[u16]) -> Result<(), VariableError> {
    match name.iter().position(|&unit| unit == 0) {
        Some(end) if end + 1 == name.len() => Ok(()),
        _ => Err(VariableError::InvalidParameter),
    }
}

/// `GetVariable`: the variable `name` of `vendor`, read into `data`; see
/// [`radix_hal::Platform::firmware_variable`].
pub fn variable(
    vendor: &[u8; 16],
    name: &[u16],
    data: &mut [u8],
) -> Result<(u32, usize), VariableError> {
    let runtime = RUNTIME.get().ok_or(VariableError::NotSupported)?;
    check_name(name)?;
    let vendor = Guid(*vendor);
    let mut attributes = 0u32;
    let mut size = data.len() as u64;
    let args = [
        name.as_ptr() as u64,
        &raw const vendor as u64,
        &raw mut attributes as u64,
        &raw mut size as u64,
        if data.is_empty() { 0 } else { data.as_mut_ptr() as u64 },
    ];
    match check(call(runtime.get_variable, args)?) {
        Ok(()) => Ok((attributes, size as usize)),
        Err(VariableError::TooSmall(_)) => Err(VariableError::TooSmall(size as usize)),
        Err(err) => Err(err),
    }
}

/// `SetVariable`: writes, or with no `data` deletes, the variable `name`
/// of `vendor`.
pub fn set_variable(
    vendor: &[u8; 16],
    name: &[u16],
    attributes: u32,
    data: &[u8],
) -> Result<(), VariableError> {
    let runtime = RUNTIME.get().ok_or(VariableError::NotSupported)?;
    check_name(name)?;
    let vendor = Guid(*vendor);
    let args = [
        name.as_ptr() as u64,
        &raw const vendor as u64,
        u64::from(attributes),
        data.len() as u64,
        pointer(data),
    ];
    check(call(runtime.set_variable, args)?)
}

/// `GetNextVariableName`: replaces `vendor` and `name` with the next
/// variable's and gives its name's length in UCS-2 characters, NUL and
/// all.
pub fn next_variable(vendor: &mut [u8; 16], name: &mut [u16]) -> Result<usize, VariableError> {
    let runtime = RUNTIME.get().ok_or(VariableError::NotSupported)?;
    // The previous name, which ends in a NUL within the buffer.
    if !name.contains(&0) {
        return Err(VariableError::InvalidParameter);
    }
    let mut guid = Guid(*vendor);
    let mut size = (name.len() * 2) as u64;
    let args = [&raw mut size as u64, name.as_mut_ptr() as u64, &raw mut guid as u64, 0, 0];
    let result = check(call(runtime.get_next_variable_name, args)?);
    let units = (size / 2) as usize;
    match result {
        Ok(()) if units == 0 || units > name.len() || name[units - 1] != 0 => {
            // A name that does not end where the firmware says it does:
            // what it keeps is not as it should be.
            Err(VariableError::Device)
        }
        Ok(()) => {
            *vendor = guid.0;
            Ok(units)
        }
        Err(VariableError::TooSmall(_)) => Err(VariableError::TooSmall(units)),
        Err(err) => Err(err),
    }
}

/// `QueryVariableInfo`: the room for variables with `attributes`.
pub fn variable_space(attributes: u32) -> Result<VariableSpace, VariableError> {
    let runtime = RUNTIME.get().ok_or(VariableError::NotSupported)?;
    let function = runtime.query_variable_info.ok_or(VariableError::NotSupported)?;
    let (mut total, mut remaining, mut largest) = (0u64, 0u64, 0u64);
    let args = [
        u64::from(attributes),
        &raw mut total as u64,
        &raw mut remaining as u64,
        &raw mut largest as u64,
        0,
    ];
    check(call(function, args)?)?;
    Ok(VariableSpace { total, remaining, largest })
}
