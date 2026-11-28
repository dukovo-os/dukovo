// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The thunks the kernel's indirect branches and returns go through, and
//! the rewrite of every place that uses one at boot (ADR 0793).
//!
//! The kernel is compiled with every indirect call and jump sent to
//! `__x86_indirect_thunk_r11` and every return sent to
//! `__x86_return_thunk` (`-Zretpoline-external-thunk`,
//! `-Zfunction-return=thunk-extern` and `-Zindirect-branch-cs-prefix`; the
//! uACPI C the same way), and the assembly's returns are written as the
//! same jump (`radix_ret`, [`asm_begin!`]). `ob build` lists every such place in `.kthunks`
//! (`radix_thunks` has the format). The boot CPU rewrites each one for
//! what the processor needs ([`patch`]), before any other CPU starts: as
//! a plain branch or return, as one placed out of Indirect Target
//! Selection's reach, or as a branch to one of the thunks here. Until
//! then the defaults hold everywhere: the retpoline, and a plain return.
//!
//! The thunks:
//!
//! - the retpoline: a call that lands past a speculation trap, which
//!   replaces its own return address with the target and returns to it,
//!   so that the indirect branch predictor is never asked; its return is
//!   in the upper half of its cache line, out of ITS's reach;
//! - an aligned `jmp r11` and the default return, a `ret`, both in the
//!   upper half of their lines, for ITS;
//! - for Retbleed on AMD's Zen 1 and 2, one return that every return goes
//!   through, whose prediction each entry from a program untrains: a
//!   `test` whose ModRM byte is that return's `ret` decodes over it, so
//!   the predictor forgets what it had there, and the `ret` then runs
//!   for real and teaches it a return;
//! - for SRSO on Zen 1 and 2, a safe return: each return calls a `lea`
//!   that drops the call's own return address and returns, so that the
//!   return stack predicts the trap after the call and the return goes
//!   where the stack says; its untraining is a `movabs` whose immediate is
//!   those eight bytes;
//! - for SRSO on Zen 3 and 4, the same pair at two addresses whose bits
//!   2, 8, 14 and 20 differ, which those processors' predictor takes for
//!   one place: the untraining's no-op there evicts what the predictor
//!   holds for the return. The two pages that hold them are mapped a
//!   second time at such a pair of addresses ([`alias`]), and only that
//!   second mapping runs.
//!
//! The indirect calls into the firmware's runtime services and Hyper-V's
//! hypercall page go through the retpoline too; the firmware's own
//! branches are left to IBRS or IBPB around the call (`speculation.rs`).

use radix_cpuflaws::{Controls, IndirectBranches, ReturnPath};
use radix_thunks::{Indirect, Return, SITE, SiteError, Table, Thunks};

use crate::{memory, paging};

/// The start of a `global_asm!` block that returns or calls through R11:
/// defines `radix_ret`, a return written as the jump to the return thunk
/// that the compiler makes of one, and `radix_call_r11`, an indirect call
/// through R11 as the compiler makes one, each with its CS prefix, so that
/// `ob build` lists them with the compiler's and the boot CPU rewrites
/// them. The bytes are spelled out so that the assembler cannot shorten
/// the jump to one without a relocation, which `ob build` would not see.
/// [`asm_end!`] ends the block.
macro_rules! asm_begin {
    () => {
        concat!(
            ".macro radix_ret\n",
            ".byte 0x2e, 0xe9\n",
            ".long __x86_return_thunk - . - 4\n",
            ".endm\n",
            ".macro radix_call_r11\n",
            ".byte 0x2e, 0xe8\n",
            ".long __x86_indirect_thunk_r11 - . - 4\n",
            ".endm",
        )
    };
}
pub(crate) use asm_begin;

/// The end of a block that [`asm_begin!`] started, which leaves the names
/// free for the next block.
macro_rules! asm_end {
    () => {
        ".purgem radix_ret\n.purgem radix_call_r11"
    };
}
pub(crate) use asm_end;

/// Where the untraining's no-op sits in its page, and where the safe
/// return sits in its own: the two differ in bits 2 and 8.
const ALIAS_UNTRAIN: u64 = 0x40;
const ALIAS_SAFE_RET: u64 = 0x144;
/// Where the return thunk that calls the safe return sits in its page.
const ALIAS_RETURN: u64 = 0x14c;
/// How far apart the two pages are mapped: bits 14 and 20.
const ALIAS_PAGES_APART: u64 = 0x10_4000;
/// The bits in which the untraining's address and the safe return's
/// differ, as Zen 3 and 4 need.
const ALIAS_BITS: u64 = 1 << 2 | 1 << 8 | 1 << 14 | 1 << 20;
const _: () = assert!(ALIAS_UNTRAIN ^ (ALIAS_PAGES_APART + ALIAS_SAFE_RET) == ALIAS_BITS);

// Each thunk's place in its cache line is part of what it does: see the
// module's documentation. Everything here is in a section of its own, so
// that a jump to a thunk from other code is always a relocation that
// `ob build` sees, and the `ret`s here are the real ones.
core::arch::global_asm!(
    r#"
    .pushsection .text.radix_thunks, "ax", @progbits

    /* The retpoline. */
    .balign 64, 0xcc
    .global __x86_indirect_thunk_r11
__x86_indirect_thunk_r11:
    call 2f
1:
    pause
    lfence
    jmp 1b
    .balign 32, 0xcc
2:
    mov qword ptr [rsp], r11
    ret
    int3

    /* `jmp r11`, then the default return, in the upper half of a line. */
    .balign 64, 0xcc
    .skip 32, 0xcc
    .global radix_aligned_indirect_r11
radix_aligned_indirect_r11:
    jmp r11
    int3
    .balign 16, 0xcc
    .global __x86_return_thunk
__x86_return_thunk:
    ret
    int3

    /* Retbleed's untrained return: the return at the start of a line, its
       untraining the byte before. From the untraining, the bytes read as
       `test bl, 0xcc`, `lfence` and a jump back to the return. */
    .balign 64, 0xcc
    .skip 63, 0xcc
    .global radix_untrain_ret
radix_untrain_ret:
    .byte 0xf6
    .global radix_untrained_return
radix_untrained_return:
    ret
    int3
    lfence
    jmp radix_untrained_return
    int3

    /* SRSO's safe return on Zen 1 and 2: the eight bytes of the safe
       return at the start of a line, its untraining the `movabs rax`
       whose immediate they are, in the two bytes before. */
    .balign 64, 0xcc
    .skip 62, 0xcc
    .global radix_safe_ret_untrain
radix_safe_ret_untrain:
    .byte 0x48, 0xb8
radix_safe_ret:
    /* lea rsp, [rsp + 8]; ret; int3; int3 */
    .byte 0x48, 0x8d, 0x64, 0x24, 0x08, 0xc3, 0xcc, 0xcc
    lfence
    call radix_safe_ret
    ud2
    .global radix_safe_return
radix_safe_return:
    call radix_safe_ret
    ud2

    /* SRSO's safe return on Zen 3 and 4, in two pages that run only where
       `alias` maps them, bits 14 and 20 apart. */
    .balign 4096, 0xcc
    .global radix_alias_untrain_page
radix_alias_untrain_page:
    .skip {untrain}, 0xcc
    /* xchg ax, ax; lfence; jmp to the return thunk in the other page. */
    .byte 0x66, 0x90
    lfence
    .byte 0xe9
    .long {apart} + {return_thunk} - ({untrain} + 10)
    int3
    .balign 4096, 0xcc
    .global radix_alias_ret_page
radix_alias_ret_page:
    .skip {safe_ret}, 0xcc
    /* lea rsp, [rsp + 8]; ret; int3; int3 */
    .byte 0x48, 0x8d, 0x64, 0x24, 0x08, 0xc3, 0xcc, 0xcc
    /* call the safe return; ud2 */
    .byte 0xe8
    .long {safe_ret} - ({return_thunk} + 5)
    ud2
    .balign 4096, 0xcc

    /* What each entry from a program calls when it untrains its return:
       a jump that `patch` points at the untraining the processor needs. */
    .balign 64, 0xcc
    .global radix_return_untrain
radix_return_untrain:
    .byte 0x2e, 0xe9
    .long 0
    ret
    int3

    .popsection
"#,
    untrain = const ALIAS_UNTRAIN,
    safe_ret = const ALIAS_SAFE_RET,
    return_thunk = const ALIAS_RETURN,
    apart = const ALIAS_PAGES_APART,
);
const _: () = assert!(ALIAS_RETURN == ALIAS_SAFE_RET + 8);

unsafe extern "C" {
    static __kernel_text_start: u8;
    static __kernel_data_end: u8;
    static __kthunks_start: u8;
    static __kthunks_end: u8;
    static __x86_indirect_thunk_r11: u8;
    static __x86_return_thunk: u8;
    static radix_aligned_indirect_r11: u8;
    static radix_untrain_ret: u8;
    static radix_untrained_return: u8;
    static radix_safe_ret_untrain: u8;
    static radix_safe_return: u8;
    static radix_alias_untrain_page: u8;
    static radix_alias_ret_page: u8;
    static radix_return_untrain: u8;
}

/// Address of a linker symbol, where it runs.
macro_rules! symbol {
    ($name:ident) => {
        &raw const $name as u64
    };
}

/// The thunks' addresses.
fn thunks() -> Thunks {
    Thunks {
        retpoline: symbol!(__x86_indirect_thunk_r11),
        ret: symbol!(__x86_return_thunk),
        aligned_indirect: symbol!(radix_aligned_indirect_r11),
    }
}

/// The table `ob build` wrote.
fn table() -> Table<'static> {
    let start = &raw const __kthunks_start;
    let len = &raw const __kthunks_end as usize - start as usize;
    // SAFETY: the linker script puts the table's room between the two
    // symbols, in read-only data that lives as long as the kernel.
    let bytes = unsafe { core::slice::from_raw_parts(start, len) };
    Table::parse(bytes).expect("thunks: the kernel has no table of its thunk sites")
}

/// Maps the two pages of SRSO's aliased safe return a second time, apart
/// by bits 14 and 20, above the kernel image; returns where the
/// untraining page is. Its untraining's address and its safe return's
/// then differ in exactly [`ALIAS_BITS`], wherever KASLR put the image,
/// as the pages are 2 MiB-aligned.
fn alias() -> u64 {
    // A free 2 MiB page above the image, then one more for a gap.
    let end = symbol!(__kernel_data_end);
    let untrain = end.next_multiple_of(memory::LARGE_PAGE) + memory::LARGE_PAGE;
    let ret = untrain + ALIAS_PAGES_APART;
    for (virt, page) in
        [(untrain, symbol!(radix_alias_untrain_page)), (ret, symbol!(radix_alias_ret_page))]
    {
        paging::map_code_alias(virt, memory::image_phys(page))
            .unwrap_or_else(|err| panic!("thunks: mapping SRSO's safe return: {err:?}"));
    }
    untrain
}

/// How many sites [`patch`] found, and of what.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub indirect: usize,
    pub returns: usize,
}

/// Rewrites every thunk site, and the untraining's jump, for `controls`.
/// Called by the boot CPU from `speculation::patch_sites`, with interrupts
/// masked and CR0.WP clear, before any other CPU starts.
pub fn patch(controls: &Controls) -> Counts {
    let thunks = thunks();
    let indirect = match controls.indirect_branches {
        IndirectBranches::Plain => Indirect::Plain,
        IndirectBranches::Retpoline => Indirect::Retpoline,
        IndirectBranches::Aligned => Indirect::Aligned,
    };
    let (returns, untrain) = match controls.returns {
        ReturnPath::Plain => (Return::Plain, None),
        ReturnPath::Aligned => (Return::Aligned, None),
        ReturnPath::Untrained => {
            (Return::Thunk(symbol!(radix_untrained_return)), Some(symbol!(radix_untrain_ret)))
        }
        ReturnPath::SafeRet => {
            (Return::Thunk(symbol!(radix_safe_return)), Some(symbol!(radix_safe_ret_untrain)))
        }
        ReturnPath::SafeRetAliased => {
            let page = alias();
            (Return::Thunk(page + ALIAS_PAGES_APART + ALIAS_RETURN), Some(page + ALIAS_UNTRAIN))
        }
    };
    let text = symbol!(__kernel_text_start);
    let mut counts = Counts::default();
    for offset in table().offsets() {
        let at = text + u64::from(offset);
        // SAFETY: `ob build` listed `at` as a site in the kernel's text,
        // and checked that the eight bytes from it are text too.
        let bytes: [u8; SITE] = unsafe { core::ptr::read_unaligned(at as *const [u8; SITE]) };
        let site = radix_thunks::decode(at, bytes, &thunks).unwrap_or_else(|err| fail(at, err));
        match site.kind {
            radix_thunks::Kind::Indirect => counts.indirect += 1,
            radix_thunks::Kind::Return => counts.returns += 1,
        }
        let new = radix_thunks::rewrite(at, bytes, &thunks, indirect, returns)
            .unwrap_or_else(|err| fail(at, err));
        if new != bytes {
            write(at, new);
        }
    }
    if let Some(to) = untrain {
        let at = symbol!(radix_return_untrain);
        let distance = to.wrapping_sub(at + SITE as u64) as i64;
        let displacement = i32::try_from(distance).expect("thunks: the untraining is out of reach");
        let mut new = [0x2e, 0xe9, 0, 0, 0, 0];
        new[2..].copy_from_slice(&displacement.to_le_bytes());
        write(at, new);
    }
    counts
}

fn fail(at: u64, err: SiteError) -> ! {
    panic!("thunks: the site at {at:#x} is wrong: {err}")
}

/// Writes `new` over the six bytes at `at`, with the two after them as
/// they were, in one store: an NMI on this CPU sees the site whole.
fn write(at: u64, new: [u8; SITE]) {
    // SAFETY: `at` is a site in the kernel's text, which stays mapped; the
    // eight bytes from it are text (`ob build` checks).
    let old = unsafe { core::ptr::read_unaligned(at as *const u64) };
    let mut bytes = old.to_le_bytes();
    bytes[..SITE].copy_from_slice(&new);
    let value = u64::from_le_bytes(bytes);
    // SAFETY: as for the read; the caller cleared CR0.WP, and no other
    // CPU runs.
    unsafe {
        core::arch::asm!(
            "mov qword ptr [{at}], {value}",
            at = in(reg) at,
            value = in(reg) value,
            options(nostack, preserves_flags),
        );
    }
}
