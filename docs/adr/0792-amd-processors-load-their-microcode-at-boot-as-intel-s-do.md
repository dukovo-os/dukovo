<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0792. AMD processors load their microcode at boot as Intel's do

- Status: Accepted
- Date: 2026-11-27

## Context

The kernel loads CPU microcode early from a boot module (ADR 0014), which
the firmware sets of the slots carry (ADR 0468). Only Intel's format was
read: on an AMD processor the kernel logged "only Intel processors are
supported; module ignored" and `/kernel/firmware/microcode` said
`unsupported`. AMD's fixes for speculation flaws (the IBPB extension
of SRSO, Zenbleed, the VERW fix of TSA) and the signature check of
EntrySign all arrive as microcode patches, so an AMD machine that does not
load them runs with the flaws its firmware has not fixed.

AMD ships patches in containers, as linux-firmware's `amd-ucode`
directory has them: one file for families 10h to 14h and one for each of
families 15h, 16h, 17h, 19h and 1Ah. A container is a magic word, an
equivalence table that maps the CPUID signatures it serves to equivalence
IDs, and patch sections. A patch is for a processor whose signature the
table maps to the patch's equivalence ID. The files linux-firmware
ships have patches of at most 1,568 bytes (families 10h to 14h), 2,592
(15h), 3,458 (16h), 3,200 (17h), 5,568 (19h) and 14,368 (1Ah).

## Decision

**One module serves every machine.** The microcode module is any number of
Intel updates and AMD containers back to back. Each CPU reads the pieces
for its own vendor and skips the others, so a firmware set holds one file
for all the machines that set is staged on. `radix-microcode` gains
`amd`, which parses and checks a container, and `pieces`, which splits a
file into its Intel updates and AMD containers; `updates` and `find` for
Intel skip containers instead of calling them damaged.

**A container has no length.** It runs to the next container, to the end of
the file, or to an Intel update that passes its checksums: an Intel header
starts with version 1, which is also the type number of an AMD patch
section, so a whole update that checks out is the only thing taken for the
end. `ob image --microcode` puts the Intel updates first and the
containers after them, since each Intel update is a multiple of 1 KiB long,
which keeps the data of the next 16-byte aligned, and a container is not.

**What is refused.** A module with any piece that fails its checks is not
used at all, as before. A container fails when its equivalence table has
size 0 or a size that is not a multiple of 16, a section runs past the end
of the file, a section's type is neither the table nor a patch, a patch
is smaller than its 64-byte header or larger than 15,872 bytes, or the
word where the next container should start is not the magic. A patch is
also left out for a processor whose family takes smaller patches than it
(3,200 bytes up to family 14h, 4,096 on 15h, 3,458 on 16h, 3,200 on 17h and
18h, 5,568 on 19h), so a patch of the wrong family is never written to the
processor.

**Loading.** On every CPU, the boot CPU before the page tables are built and
the others as they start, the kernel takes the patch with the highest
revision whose equivalence ID is the one the table gives its signature, and
loads it if its revision is above the running one (MSR 0x8B,
`MSR_AMD64_PATCH_LEVEL`, read as it is, with none of Intel's CPUID
steps). The patch is copied to an aligned static buffer of the largest
patch's size, which a spin lock guards, and the address of its header is
written to `MSR_AMD64_PATCH_LOADER` (0xC001_0020). On family 17h and later
each page of the patch is then invalidated, as Zen cores keep the
translation of the buffer they last used, and the revision is read back:
the patch's own is applied, any other refused. Under a hypervisor, which
keeps the processor's microcode, a patch that did not take effect is
reported as `hypervisor` with the running revision, as it is on Intel.
Hygon processors, though AMD's design, are not loaded: no container
holds a patch for their signatures.

**The report.** `/kernel/firmware/microcode` gains a line `vendor`: `intel`,
`amd`, or `other` for a processor of neither. `updates` counts the Intel
updates or the AMD patches, by the vendor of the CPU that reads it. The
log line of the boot CPU gives the count of each vendor's pieces: "module
of N bytes with I Intel updates and A AMD patches". The other fields are
as before, so the health check of ADR 0468 reads an AMD machine's report
with the code it has.

**Not decided here.** Nothing is loaded after boot, for either vendor. The
kernel does not check AMD patches against a list of SHA-256 sums, as
Linux's checks do for EntrySign: a firmware set is signed (ADR 0468), and
a module from `ob image --microcode` is the image builder's own choice.

## Consequences

- AMD machines get the microcode that fixes their speculation flaws from
  the same firmware sets, with the same try, fallback and health check as
  Intel's. The row of `docs/deferred.md` for AMD microcode is gone.
- QEMU does not apply patches, so the boot test `microcode` checks the
  matching and the checks on an emulated Intel part only, and the loader
  itself is confirmed on AMD hardware: the Intel path has the same limit
  (ADR 0014).
- A module built from files other than `ob image --microcode`'s, with an
  Intel update after a container, may place that update's data off the
  16-byte alignment Intel's loader needs, and the update is then refused
  with `refused`: a builder of such a file puts the Intel updates first, as
  `ob image --microcode` does.
- A patch for a family not listed is limited by the largest size, 15,872
  bytes; the limits for the listed families are revised when
  linux-firmware's files outgrow them.
- The kernel holds a 15,872-byte buffer in its data for the load.
