// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's thunk sites, and how the boot CPU rewrites each one for
//! the processor it runs on (ADR 0793).
//!
//! The kernel is compiled so that no indirect branch and no return is left
//! to the processor's predictors as written. Every indirect call or jump
//! goes to `__x86_indirect_thunk_r11` with its target in R11, and every
//! return is a jump to `__x86_return_thunk`. Each such instruction is a
//! *site* of six bytes: a CS segment prefix and a `call rel32` or `jmp
//! rel32`, or a conditional `jcc rel32`. After linking, `ob build` finds
//! the sites from the linker's relocation records and lists them in the
//! image's `.kthunks` section ([`encode`]); at boot the kernel reads the
//! list ([`Table`]) and rewrites each site with [`rewrite`], before any
//! other CPU starts:
//!
//! - an indirect branch becomes `call r11` or `jmp r11` where the
//!   processor's own protection of indirect branches is enough, stays a
//!   branch to the retpoline where it is not, and on a processor with
//!   Indirect Target Selection (ITS) is placed so that it ends in the upper
//!   half of its 64-byte cache line, or else goes to a thunk whose branch
//!   does;
//! - a return becomes `ret` where returns need nothing, is placed in the
//!   upper half of its line the same way for ITS (the default return thunk
//!   is such a `ret`), or goes to the return thunk the processor needs.
//!
//! The format of the table, all integers little-endian:
//!
//! ```text
//! 0   8   magic "RADIXTHK"
//! 8   4   format version (1)
//! 12  4   number of sites, n
//! 16  4   total length of the table in bytes
//! 20  12  reserved, zero
//! 32  4n  each site's offset from the start of the kernel's text, ascending
//! ```
//!
//! Offsets from the text, not addresses, so that the table holds wherever
//! KASLR moves the kernel. Which thunk a site leads to says what it is, so
//! the table needs no kind: [`decode`] reads it from the site's bytes, and
//! fails for bytes that are no site, so that a wrong table stops the boot
//! instead of changing code it does not describe.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

use core::fmt;

/// First eight bytes of a table.
pub const MAGIC: [u8; 8] = *b"RADIXTHK";
/// Format version.
pub const VERSION: u32 = 1;
/// Bytes before the first site.
pub const HEADER: usize = 32;
/// Bytes of a site.
pub const SITE: usize = 6;

/// The CS segment prefix the compiler puts in front of a site's `call` or
/// `jmp`, which makes it as long as a `jcc rel32` and leaves room for any
/// of the rewrites.
const CS: u8 = 0x2e;
/// `call rel32`.
const CALL: u8 = 0xe8;
/// `jmp rel32`.
const JMP: u8 = 0xe9;
/// `int3`, which fills what follows a jump or return that is never run.
const INT3: u8 = 0xcc;
/// `ret`.
const RET: u8 = 0xc3;
/// `call r11`.
const CALL_R11: [u8; 3] = [0x41, 0xff, 0xd3];
/// `jmp r11`.
const JMP_R11: [u8; 3] = [0x41, 0xff, 0xe3];
/// In a 64-byte cache line, the bit of an address that is set in its upper
/// half.
const UPPER_HALF: u64 = 0x20;

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

/// Why a table could not be encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The output buffer is too small.
    TooSmall {
        /// Bytes the table needs.
        needed: usize,
    },
    /// The offsets are not in ascending order, or two sites overlap.
    Unsorted,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::TooSmall { needed } => write!(f, "the table needs {needed} bytes"),
            EncodeError::Unsorted => f.write_str("the sites are not sorted, or two overlap"),
        }
    }
}

/// Bytes a table of `sites` sites takes.
pub fn encoded_len(sites: usize) -> usize {
    HEADER + 4 * sites
}

/// Writes a table of the sites at `offsets` into `out` and returns its
/// length. The rest of `out` is left as it is.
pub fn encode(offsets: &[u32], out: &mut [u8]) -> Result<usize, EncodeError> {
    let needed = encoded_len(offsets.len());
    if out.len() < needed {
        return Err(EncodeError::TooSmall { needed });
    }
    if offsets.windows(2).any(|pair| pair[1] < pair[0] + SITE as u32) {
        return Err(EncodeError::Unsorted);
    }
    out[..8].copy_from_slice(&MAGIC);
    out[8..12].copy_from_slice(&VERSION.to_le_bytes());
    out[12..16].copy_from_slice(&(offsets.len() as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(needed as u32).to_le_bytes());
    out[20..HEADER].fill(0);
    let slots = out[HEADER..needed].as_chunks_mut::<4>().0;
    for (slot, offset) in slots.iter_mut().zip(offsets) {
        slot.copy_from_slice(&offset.to_le_bytes());
    }
    Ok(needed)
}

/// A parsed table.
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    offsets: &'a [u8],
}

impl<'a> Table<'a> {
    /// Checks the header and finds the list. `None` if `bytes` holds no
    /// valid table (for example, the zeros of a kernel that `ob` did not
    /// post-process).
    pub fn parse(bytes: &'a [u8]) -> Option<Table<'a>> {
        if bytes.get(..8)? != MAGIC || u32_at(bytes, 8)? != VERSION {
            return None;
        }
        let sites = u32_at(bytes, 12)? as usize;
        let len = u32_at(bytes, 16)? as usize;
        if len != encoded_len(sites) || len > bytes.len() {
            return None;
        }
        Some(Table { offsets: &bytes[HEADER..len] })
    }

    /// Number of sites.
    pub fn len(&self) -> usize {
        self.offsets.len() / 4
    }

    /// Whether the table lists no sites.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Each site's offset from the start of the kernel's text.
    pub fn offsets(&self) -> impl Iterator<Item = u32> + 'a {
        self.offsets.as_chunks::<4>().0.iter().map(|&word| u32::from_le_bytes(word))
    }
}

/// Where the thunks are, at the addresses the kernel runs at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thunks {
    /// `__x86_indirect_thunk_r11`, the retpoline: where the compiler sends
    /// every indirect branch.
    pub retpoline: u64,
    /// `__x86_return_thunk`, a `ret` in the upper half of its cache line:
    /// where the compiler sends every return.
    pub ret: u64,
    /// A `jmp r11` in the upper half of its cache line, for the indirect
    /// branches that cannot be placed there themselves (ITS).
    pub aligned_indirect: u64,
}

/// How the kernel's indirect branches run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Indirect {
    /// As plain indirect branches.
    #[default]
    Plain,
    /// Through the retpoline.
    Retpoline,
    /// As plain indirect branches, each ending in the upper half of its
    /// cache line, or through a thunk whose branch does (ITS).
    Aligned,
}

/// How the kernel's returns run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Return {
    /// As plain returns.
    #[default]
    Plain,
    /// As plain returns in the upper half of their cache lines, or through
    /// the default return thunk, which is one (ITS).
    Aligned,
    /// Through the return thunk at this address.
    Thunk(u64),
}

/// What a site's instruction is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `cs call rel32`.
    Call,
    /// `cs jmp rel32`.
    Jump,
    /// `jcc rel32`, with its condition (0 to 15).
    Branch(u8),
}

/// What a site leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An indirect branch, through the retpoline.
    Indirect,
    /// A return, through the return thunk.
    Return,
}

/// A decoded site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    /// Its instruction.
    pub form: Form,
    /// What it leads to.
    pub kind: Kind,
}

/// Why a site could not be read or rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteError {
    /// Its bytes are no `cs call`, `cs jmp` or `jcc` with a 32-bit
    /// displacement.
    NotABranch([u8; SITE]),
    /// It branches to neither thunk, but to this address.
    Elsewhere(u64),
    /// A call to the return thunk, which no compiler makes.
    CallToReturn,
    /// The thunk it would go to is out of reach of a 32-bit displacement.
    OutOfReach(u64),
}

impl fmt::Display for SiteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SiteError::NotABranch(bytes) => write!(f, "{bytes:02x?} is no branch to a thunk"),
            SiteError::Elsewhere(target) => write!(f, "it branches to {target:#x}, no thunk"),
            SiteError::CallToReturn => f.write_str("it calls the return thunk"),
            SiteError::OutOfReach(target) => write!(f, "{target:#x} is out of its reach"),
        }
    }
}

/// The address a site's displacement leads to.
fn target(at: u64, bytes: [u8; SITE]) -> u64 {
    let displacement = i32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    (at + SITE as u64).wrapping_add_signed(i64::from(displacement))
}

/// Reads the site at address `at`, whose bytes are `bytes`.
pub fn decode(at: u64, bytes: [u8; SITE], thunks: &Thunks) -> Result<Site, SiteError> {
    let form = match bytes[..2] {
        [CS, CALL] => Form::Call,
        [CS, JMP] => Form::Jump,
        [0x0f, opcode @ 0x80..=0x8f] => Form::Branch(opcode & 0xf),
        _ => return Err(SiteError::NotABranch(bytes)),
    };
    let kind = match target(at, bytes) {
        to if to == thunks.retpoline => Kind::Indirect,
        to if to == thunks.ret => Kind::Return,
        to => return Err(SiteError::Elsewhere(to)),
    };
    if form == Form::Call && kind == Kind::Return {
        return Err(SiteError::CallToReturn);
    }
    Ok(Site { form, kind })
}

/// The shortest no-op of each length up to five bytes.
const NOPS: [&[u8]; 6] = [
    &[],
    &[0x90],
    &[0x66, 0x90],
    &[0x0f, 0x1f, 0x00],
    &[0x0f, 0x1f, 0x40, 0x00],
    &[0x0f, 0x1f, 0x44, 0x00, 0x00],
];

/// Six bytes from `parts`, then `fill`.
fn bytes_of(parts: &[&[u8]], fill: u8) -> [u8; SITE] {
    let mut out = [fill; SITE];
    let mut at = 0;
    for part in parts {
        out[at..at + part.len()].copy_from_slice(part);
        at += part.len();
    }
    out
}

/// The short `jcc rel8` that jumps when `condition` does not hold.
fn short_unless(condition: u8, over: u8) -> [u8; 2] {
    [0x70 | (condition ^ 1), over]
}

/// The site's instruction as it is, sent to `to` instead.
fn retarget(at: u64, bytes: [u8; SITE], to: u64) -> Result<[u8; SITE], SiteError> {
    let distance = to.wrapping_sub(at + SITE as u64) as i64;
    let displacement = i32::try_from(distance).map_err(|_| SiteError::OutOfReach(to))?;
    let mut out = bytes;
    out[2..].copy_from_slice(&displacement.to_le_bytes());
    Ok(out)
}

/// Whether a branch whose last byte is at `last` is out of ITS's reach.
fn upper_half(last: u64) -> bool {
    last & UPPER_HALF != 0
}

/// The plain indirect branch through R11 that the site at `at` becomes,
/// placed `before` bytes in, with no-ops before it and after it (a call)
/// or `int3` after it (a jump, after which nothing runs).
fn indirect_at(form: Form, before: usize) -> [u8; SITE] {
    match form {
        Form::Call => bytes_of(&[NOPS[before], &CALL_R11, NOPS[3 - before]], INT3),
        Form::Jump => bytes_of(&[NOPS[before], &JMP_R11], INT3),
        // Over the no-ops and the jump when the condition does not hold.
        Form::Branch(condition) => {
            bytes_of(&[&short_unless(condition, 3 + before as u8), NOPS[before], &JMP_R11], INT3)
        }
    }
}

/// The plain return that the site at `at` becomes, placed `before` bytes
/// in, after no-ops.
fn return_at(form: Form, before: usize) -> [u8; SITE] {
    match form {
        Form::Branch(condition) => {
            bytes_of(&[&short_unless(condition, 1 + before as u8), NOPS[before], &[RET]], INT3)
        }
        _ => bytes_of(&[NOPS[before], &[RET]], INT3),
    }
}

/// The new bytes of the site at address `at`, whose bytes are now `bytes`,
/// for a processor whose indirect branches run as `indirect` asks and
/// whose returns as `returns` does.
pub fn rewrite(
    at: u64,
    bytes: [u8; SITE],
    thunks: &Thunks,
    indirect: Indirect,
    returns: Return,
) -> Result<[u8; SITE], SiteError> {
    let site = decode(at, bytes, thunks)?;
    // Where the branch may start, after the short jump of a `jcc`.
    let first = if matches!(site.form, Form::Branch(_)) { 2 } else { 0 };
    match site.kind {
        Kind::Indirect => match indirect {
            Indirect::Retpoline => Ok(bytes),
            Indirect::Plain => Ok(indirect_at(site.form, 0)),
            Indirect::Aligned => {
                // `jmp r11` and `call r11` are three bytes; their last byte
                // is two past their first.
                let room = SITE - first - CALL_R11.len();
                match (0..=room).find(|&before| upper_half(at + (first + before + 2) as u64)) {
                    Some(before) => Ok(indirect_at(site.form, before)),
                    None => retarget(at, bytes, thunks.aligned_indirect),
                }
            }
        },
        Kind::Return => match returns {
            Return::Plain => Ok(return_at(site.form, 0)),
            Return::Aligned => {
                let room = SITE - first - 1;
                match (0..=room).find(|&before| upper_half(at + (first + before) as u64)) {
                    Some(before) => Ok(return_at(site.form, before)),
                    // The default return thunk is a `ret` in the upper half.
                    None => Ok(bytes),
                }
            }
            Return::Thunk(to) => retarget(at, bytes, to),
        },
    }
}

#[cfg(test)]
mod tests;
