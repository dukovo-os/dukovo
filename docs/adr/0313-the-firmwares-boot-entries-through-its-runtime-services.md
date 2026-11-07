<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0313. The firmware's boot entries, through its runtime services

- Status: Accepted
- Date: 2026-11-07

## Context

Installing Dukovo next to Windows (ADR 0312's second step) puts GRUB on
an EFI system partition of Dukovo's own. Windows Boot Manager stays first
in the firmware's boot order, and firmware starts the fallback path,
`\EFI\BOOT\BOOTX64.EFI`, only when no entry works, so the machine would
keep starting Windows. Linux's installers add a boot entry with
`efibootmgr`, which writes UEFI variables through the kernel. Radix had
no way to reach them: GRUB ends the firmware's boot services before the
kernel starts, and the kernel read only the system table's configuration
tables (ADR 0231), never calling the firmware.

UEFI keeps its runtime services callable after that, in the firmware's
own physical addressing until something calls `SetVirtualAddressMap`,
provided the memory the UEFI memory map marks `EFI_MEMORY_RUNTIME` is
mapped where it is. GRUB passes that map (Multiboot2 tag 17) when it ends
the boot services itself, which it does for Radix.

## Decision

- **The kernel calls the firmware's variable services.** The x86-64
  layer reads the UEFI memory map from the boot information, and the
  runtime services table from the system table, and offers
  `GetVariable`, `GetNextVariableName`, `SetVariable` and
  `QueryVariableInfo` to the rest of the kernel (`radix_hal::Platform`).
  It never calls `SetVirtualAddressMap`, so the firmware runs as it did
  before the kernel started, and a kexec or a crash kernel would find it
  so too.
- **A page table of the firmware's own.** The lower half of every
  address space is its program's, so the runtime regions are mapped
  where they are in a table used only during a call: the kernel's half,
  as every address space has it, and each region writable and
  executable (firmware code and data share pages without the firmware's
  attributes table, as Linux finds), write-back where the firmware
  allows it, its device registers uncached, in 2 MiB pages where they
  fit. Regions that overlap, lie in memory the boot loader called free,
  or take over 1 GiB turn the services off, and the log says why.
- **A call** holds a lock, so one CPU at a time is in the firmware,
  which holds off preemption; saves the running thread's FPU registers
  and loads their initial state, as UEFI requires; switches CR3 to the
  firmware's table on PCID 0, flushed; and calls through a trampoline
  that moves to a 64 KiB stack of the firmware's own, with a guard page,
  and makes the Microsoft x64 call. Interrupts stay on, as Linux keeps
  them, since writing flash can take a while; their handlers run on any
  stack in the kernel's half. Coming back it reloads the CR3 it had
  without keeping that PCID's entries, since a shootdown in the meantime
  flushed the firmware's PCID instead. The firmware gets copies in the
  kernel's memory, never a program's pages, which its table does not map.
- **A fault in the firmware ends the call, not the kernel.** An exception
  in the firmware's code during a call resumes at the trampoline's fault
  label, which returns as the call would; the kernel then calls the
  firmware no more until the machine restarts, as Linux does, since the
  firmware may have stopped holding a lock of its own. A fault in the
  kernel's code during a call, from an interrupt handler, stays fatal.
- **The platform capability carries them** (interface version 3):
  `efi_variable`, `set_efi_variable`, `next_efi_variable` and
  `efi_variable_space`. Names pass as UCS-2 bytes, exactly as the
  firmware keeps them, and vendors as their GUIDs' two halves. Reading
  needs the read right, writing the control right; Spark gives both only
  to services that ask for admin.hardware, and `sudo` passes them on.
- **The kernel writes only the boot manager's variables** of the global
  vendor: `Boot####`, `BootOrder`, `BootNext`, `Timeout`, `Driver####`,
  `DriverOrder`, `SysPrep####`, `SysPrepOrder` and `OsIndications`.
  Firmware makers keep settings in variables of their own, and deleting
  some has left machines unable to start (Linux made its variables
  immutable by default in 2016 after that); Secure Boot's keys need
  signed writes of their own, for Phase 1.2.0. Each write is logged with
  the program that made it.
- **Room is kept.** A non-volatile write that would leave the firmware
  under 5 KiB of room for variables fails with no_space, as Linux's
  `EFI_MIN_RESERVE` refuses it: Samsung's laptops failed to start with
  their store full (2013). Deleting is always allowed.
- **`efiboot(8)`** lists and changes the entries with the options of
  Linux's `efibootmgr`, by whose name it runs too: it makes an entry as
  `HD(part,GPT,guid,start,size)/File(loader)`, the short form firmware
  expands by finding the partition wherever the disk is.

## Consequences

- The installer can add a *Dukovo* entry first in the order when it
  installs next to Windows, and leave Windows Boot Manager as it is.
- Firmware that misbehaves when called in physical mode after its boot
  services end is found by the first call, which then fails with io and
  logs the fault, instead of stopping the kernel; such firmware has been
  rare since UEFI 2.3, and Linux's mixed and old maps met it.
- Boot services memory is not kept for the firmware, as Linux keeps it
  until `SetVirtualAddressMap`: firmware that still reads it after its
  boot services end faults, and its services go off.
- Under BIOS, and on aarch64 and RISC-V for now, the methods fail with
  not_supported.
- The `efi-boot-entries` boot test makes an entry under OVMF, has the
  firmware start it through `BootNext` after a restart, and finds the
  firmware started it and forgot `BootNext`; efiboot's unit tests read
  and write load options and device paths, and list as efibootmgr does.

## Alternatives considered

- **Calling `SetVirtualAddressMap`**, as Linux does, to map the firmware
  at addresses of the kernel's choosing. It can be called once per boot,
  breaks firmware that keeps physical pointers, and gains nothing when
  the firmware's table is used only during calls.
- **An `efivarfs` of files.** Linux's interface; a filesystem whose
  writes must be whole, with an attribute word before each value and
  immutable files to unlock first, where three methods say the same.
  A compatible `/sys/firmware/efi/efivars` can be built on them later
  if ports need it.
- **Writing every variable** as Linux lets root, once unlocked. Nothing
  Dukovo does needs more than the boot manager's, and the firmware's
  own setup screens change the rest.
