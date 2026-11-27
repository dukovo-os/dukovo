<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0793. The kernel's indirect branches and returns go through thunks that the boot CPU rewrites for the processor

- Status: Accepted
- Date: 2026-11-27

## Context

ADR 0790's plan left four flaws open on some processors because the
kernel had no way to change how its indirect branches and returns run:

- Spectre v2 on AMD's Zen 1 to 3, which have no automatic IBRS: a program
  can train the indirect branch predictor to send a kernel branch to a
  gadget. IBRS closes it, at a cost on every entry and exit.
- Retbleed on Zen 1 and 2: a return whose return stack prediction is
  missing falls back to the branch target buffer, which a program trains.
- SRSO (speculative return stack overflow) on Zen 1 to 4: a program
  trains a prediction that makes the processor push its own return
  address, so that a kernel return predicts into a gadget.
- ITS (indirect target selection) on Intel's processors from Kaby Lake
  and Cascade Lake to Tiger Lake and Rocket Lake: an indirect
  branch or return that ends in the lower half of a 64-byte cache line
  can be predicted from a target trained elsewhere.

Each is closed by sending the branch or return through a thunk: a
retpoline for indirect branches, a return thunk for returns. Each costs
on processors that do not need it, and which thunk a processor needs is
known only at boot. ADR 0791 patches the entry and exit paths at boot for
the same reason; the branches and returns are everywhere else, so they
need the same treatment at every one of them.

## Decision

**The kernel is built with every indirect branch and return going
through a thunk.** `ob build` compiles the kernel and the libraries it
rebuilds (`core`, `alloc`, `compiler_builtins`) with
`-Zretpoline-external-thunk`, `-Zfunction-return=thunk-extern` and
`-Zindirect-branch-cs-prefix`, and uACPI's C with Clang's options of the
same meaning. Every indirect call or jump becomes a call or jump to
`__x86_indirect_thunk_r11`, and every return a jump to
`__x86_return_thunk`, each six bytes long: `2e e8 rel32`, `2e e9 rel32`,
or a conditional jump `0f 8x rel32`. The assembly writes its returns and
its calls through a register the same way (`radix_ret`,
`radix_call_r11`). The firmware's runtime services and Hyper-V's
hypercall page are called through the retpoline too.

**`ob build` lists every site.** The kernel is linked with
`--emit-relocs`, as for KASLR. Each PC-relative relocation in executable
code that leads to one of the two thunks marks a site; `ob build` checks
that each decodes as one of the three forms, lies in the text with the
two bytes after it, and writes their offsets from the start of the text
into the section `.kthunks` (`radix_thunks` holds the format). A compiler
that made another form fails the build with the site's address, not the
boot. The release kernel has about 21,000 sites, 82 KiB of table.

**The boot CPU rewrites each site.** From the plan (`radix_cpuflaws`),
`thunks::patch` runs inside the window of ADR 0791, on the boot CPU with
interrupts masked and CR0.WP clear, before any other CPU starts, and
writes each changed site with one 8-byte store. Every site is decoded
first; one that is not what the table says stops the boot with its
address. Indirect branches become one of:

- plain: `call r11` or `jmp r11` in the site's six bytes, with no-ops or
  traps after it, where the processor's own protection serves (enhanced
  or automatic IBRS, or IBRS while in the kernel) or there is none to
  give;
- the retpoline, left as it is (`spectre_v2=retpoline`, and the default
  on Zen 1 to 3);
- aligned, for ITS: the branch placed within its site so that it ends in
  the upper half of its line, with no-ops before it, or a jump to a
  `jmp r11` that does where no place in the site's six bytes would.

Returns become one of:

- plain: `ret`, with traps after it;
- aligned, for ITS: the `ret` placed in the upper half as above, or left
  as the jump to `__x86_return_thunk`, which is itself a `ret` in the
  upper half of its line;
- a jump to a return thunk.

**The return thunks.**

| Processor | Thunk | What it does |
|---|---|---|
| Zen 1 and 2, Retbleed | untrained return | every return is one `ret` at the start of a line; on each entry from a program the kernel runs a `test` whose ModRM byte is that `ret`, which the processor decodes over it, so what the predictor held for the `ret` is gone, and the `ret` then runs and trains it to return |
| Zen 1 and 2, SRSO | safe return | every return calls a `lea rsp, [rsp + 8]; ret`, which drops the call's own return address and returns to the real one, so that the return stack's prediction is the trap after the call; on each entry the kernel runs a `movabs` whose immediate is those eight bytes; covers Retbleed too |
| Zen 3 and 4, SRSO | aliased safe return | the same pair, run at two addresses that differ in bits 2, 8, 14 and 20, which these processors' predictor takes for the same place: the untraining's no-op there evicts what the predictor held for the safe return |

The aliased pair needs addresses the linker cannot give without padding
the image by a megabyte, so its two pages are mapped a second time at
boot, 2 MiB-aligned above the image (wherever KASLR put it) and
`0x104000` apart, and only that second mapping runs. The untraining runs
on every entry from a program as a new site of ADR 0791,
`UntrainEntry`, which calls a jump that `patch` points at the untraining
the processor needs. It uses RAX, ECX and EDX, which the entry paths have
saved.

**The flags.** `spectre_v2=retpoline` (and `retpoline,generic`) asks for
the retpoline, `retbleed=unret` and `spec_rstack_overflow=safe-ret` for
their return thunks even with `mitigations=off`, and
`indirect_target_selection=on|off` turns ITS's alignment on or off, as
Linux's flags of those names do. The boot log says how many indirect
branches and returns were found and how each runs.

## Consequences

- AMD's Zen 1 to 4 and the Intel parts with ITS report Spectre v2,
  Retbleed, SRSO and ITS closed by default, as on Linux.
- A processor that needs none of it runs every indirect branch and
  return plain: the six bytes of a site hold the plain instruction and
  no-ops or traps, at the cost of a slightly larger image.
- The running text differs from the image at every site the processor
  needs changed.
- QEMU's emulation without KVM, on which the instruction budgets are
  counted, names its processor AMD's with a family before Zen, so its
  indirect branches go through the retpoline: a system call's entry and
  exit take 192 instructions where they took 182, and the budgets of the
  operations that grew near theirs are set again from the new counts.
- Hand-written assembly that returns, or branches through a register,
  writes `radix_ret` or `radix_call_r11` inside the blocks that
  `asm_begin!` starts; the only plain returns left in the image are those
  of the thunks themselves and the branch history clearing sequence, whose
  calls and returns are the sequence.
- A per-process speculation control (`prctl(PR_SET_SPECULATION_CTRL)`) and
  KPTI remain in docs/deferred.md.
