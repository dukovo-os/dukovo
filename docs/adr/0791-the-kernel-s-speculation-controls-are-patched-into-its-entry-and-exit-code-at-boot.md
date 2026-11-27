<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0791. The kernel's speculation controls are patched into its entry and exit code at boot

- Status: Accepted; retpolines and return thunks since [0793](0793-the-kernel-s-indirect-branches-and-returns-go-through-thunks-that-the-boot-cpu-rewrites-for-the-processor.md), page-table isolation since [0794](0794-on-processors-with-meltdown-programs-run-on-page-tables-without-the-kernel.md)
- Date: 2026-11-27

## Context

The plan of ADR 0790 says, per processor, what has to happen at each
crossing between a program and the kernel: write IA32_SPEC_CTRL on entry
and on exit, issue IBPB on entry, clear the branch history on a system
call, run VERW before the return to a program, clear the divider, fence
after SWAPGS, fill the return stack on a context switch. Most machines
need only some of them, and every one costs cycles on the paths that run
most often.

Testing a flag at each crossing would put a conditional branch in front of
each protection, and a conditional branch is what Spectre v1 trains: a
program that teaches the processor to skip the branch skips the
protection while the processor runs ahead. Linux patches its code at boot
instead (`ALTERNATIVE`), so that each crossing runs straight-line code
with exactly the protections the machine needs.

## Decision

**Each protection is a site.** In the entry and exit code (`user.rs`,
`interrupts.rs`, `thread.rs` of `kernel/arch/x86_64`), a protection sits
behind a short jump over it:

```text
660: jmp 662f     ; 2 or 5 bytes, over the protection
661: <the protection's instructions>
662:
```

and a record in the section `.spec_sites`: the jump's address relative to
the record, the jump's length, and which site it is. The jump is built
in, so a kernel that has not yet patched itself, or a processor that needs
none of the protections, skips each at the cost of one taken jump.

**Patching at boot.** `speculation::init`, on the boot CPU before any
other CPU starts and with interrupts masked, decides the plan and, for
every record whose site the plan turns on, replaces the jump with a no-op
of the same length (`66 90` or `0f 1f 44 00 00`), in one 8-byte store,
with CR0.WP clear for the duration. Writing CR0 back serializes the boot
CPU, and the others start later, so none can see a half-written jump.
Every record is checked at every boot, whether its site is on or not: one
whose jump is not where it says, or of a length not 2 or 5, stops the boot
with a panic that names its address. The records are PC-relative, so KASLR
(which relocates absolute addresses) moves nothing in them.

**The sites.**

| Site | Where | What it does |
|---|---|---|
| `SpecCtrlEntry` | system call and interrupt entry from a program | IA32_SPEC_CTRL to the kernel's value (IBRS where the plan uses it) |
| `IbpbEntry` | the same | IBPB, then the return stack filled (`retbleed=ibpb`, `spec_rstack_overflow=ibpb`) |
| `BhbClear` | system call entry | Intel's branch history clearing loop, where BHI_DIS_S is missing |
| `SwapgsFence` | interrupt entry | LFENCE after the SWAPGS decision |
| `SpecCtrlParanoid` | NMI and machine-check entry from the kernel | saves IA32_SPEC_CTRL, sets the kernel's, restores it on return |
| `SpecCtrlExit` | every return to a program | IA32_SPEC_CTRL to the program's value |
| `Div0Exit` | the same | a division of 0 by 1, which leaves no quotient for Zen 1 to leak |
| `VerwExit` | immediately before SYSRET or IRET to a program | VERW, which clears the processor's buffers |
| `RsbFillSwitch` | context switch | 32 calls whose returns are never taken, so no return predicts into the old thread |
| `UntrainEntry` | system call and interrupt entry from a program | the return thunk's untraining (ADR 0793) |
| `Pti` | every entry from a program and exit to one; NMI, machine-check and exception entry in kernel mode, and their return | CR3 to the kernel's page tables, and back to the program's (ADR 0794) |

An NMI or machine check can arrive between a site of the exit path and
the return, with the program's value in IA32_SPEC_CTRL; the paranoid site
saves what it finds there in a register the handler keeps and restores it,
so the kernel never runs that handler with the program's value and the
program never returns with the kernel's.

**Controls that are not on a crossing** are set in Rust, once:
IA32_SPEC_CTRL's kernel value, automatic IBRS (EFER bit 21), TSX_CTRL, the
Zenbleed chicken bit and IA32_MCU_OPT_CTRL on every CPU as it starts
(`init_cpu`); IBRS or IBPB around each call into UEFI runtime services,
whose code was not built with the kernel's protections; VERW before a CPU
idles where the other hardware thread could read what the kernel left;
IBPB when a CPU switches to another program's address space, where
`spectre_v2_user=on` asks for it; and, with `nosmt`, the second hardware
threads left unstarted.

**What arrives later.** Retpolines and return thunks, patched the same way
at each indirect branch and return, together with a per-process strict
mode (`prctl(PR_SET_SPECULATION_CTRL)`); and KPTI for Meltdown. Both are in
docs/deferred.md.

## Consequences

- A machine that needs no protection runs each crossing with a few taken
  jumps; one that needs them runs them without a branch that could skip
  them.
- `.spec_sites` is a list the kernel reads once, at boot; its 33 records
  are 264 bytes.
- A site is added with the `site!` macro in the assembly and a `Site`
  variant; the boot check catches a record that does not point at its
  jump.
- Code patched at boot differs from the image: at each site that is on,
  the running kernel has a no-op where the image has a jump.
