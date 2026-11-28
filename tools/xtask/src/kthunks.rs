// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Writing the table of thunk sites into the linked kernel (ADR 0793).
//!
//! The kernel is compiled so that every indirect branch is a call or jump
//! to `__x86_indirect_thunk_r11` and every return a jump to
//! `__x86_return_thunk`, and the boot CPU rewrites each of those six-byte
//! sites for the processor it runs on. It finds them in the
//! `radix-thunks` table this module writes over the zeros of the image's
//! `.kthunks` section, as `ksyms` fills `.ksyms`. The sites come from the
//! linker's relocation records (the kernel is linked with
//! `--emit-relocs`): each PC-relative one that leads to a thunk is a site,
//! which must be one that `radix_thunks::decode` reads, so that a
//! compiler that made some other instruction fails the build here, not
//! the boot.

use std::collections::BTreeSet;

use radix_thunks::Thunks;

use crate::elf::{
    Elf, SHF_ALLOC, SHF_EXECINSTR, SHT_RELA, SHT_SYMTAB, c_string, read_u32, read_u64,
};
use crate::{Result, bail};

const R_X86_64_PC32: u32 = 2;
const R_X86_64_PLT32: u32 = 4;

/// The values of the symbols of `elf` named in `names`, in that order.
fn symbol_values(elf: &Elf<'_>, names: &[&str]) -> Result<Vec<u64>> {
    let Some(symtab) = elf.sections.iter().find(|s| s.kind == SHT_SYMTAB) else {
        bail!("the kernel has no ELF symbol table");
    };
    let strtab = usize::try_from(symtab.link).ok().and_then(|link| elf.sections.get(link));
    let (Some(symbols), Some(strings)) = (elf.data(symtab), strtab.and_then(|s| elf.data(s)))
    else {
        bail!("the kernel's ELF symbol table cannot be read");
    };
    let mut found = vec![None; names.len()];
    for entry in symbols.as_chunks::<24>().0 {
        let (Some(name), Some(value)) = (read_u32(entry, 0), read_u64(entry, 8)) else { continue };
        let name = usize::try_from(name).ok().and_then(|at| c_string(strings, at));
        if let Some(index) = names.iter().position(|&wanted| Some(wanted) == name) {
            found[index] = Some(value);
        }
    }
    names
        .iter()
        .zip(found)
        .map(|(name, value)| match value {
            Some(value) => Ok(value),
            None => bail!("the kernel has no symbol {name}"),
        })
        .collect()
}

/// The value of every symbol of `elf`, by index, for the relocations.
fn symbols_by_index(elf: &Elf<'_>) -> Vec<u64> {
    let symtab = elf.sections.iter().find(|s| s.kind == SHT_SYMTAB);
    let data = symtab.and_then(|s| elf.data(s)).unwrap_or_default();
    data.as_chunks::<24>().0.iter().map(|entry| read_u64(entry, 8).unwrap_or(0)).collect()
}

/// The sites of `elf`, as offsets from the start of its text, sorted.
pub fn collect(elf: &Elf<'_>) -> Result<Vec<u32>> {
    let names = [
        "__kernel_text_start",
        "__kernel_text_end",
        "__x86_indirect_thunk_r11",
        "__x86_return_thunk",
        "radix_aligned_indirect_r11",
    ];
    let [text_start, text_end, retpoline, ret, aligned_indirect] = symbol_values(elf, &names)?[..]
    else {
        unreachable!("one value for each name")
    };
    let thunks = Thunks { retpoline, ret, aligned_indirect };
    let symbols = symbols_by_index(elf);
    let mut sites = BTreeSet::new();
    for rela in elf.sections.iter().filter(|s| s.kind == SHT_RELA) {
        let Some(target) = elf.sections.get(rela.info as usize) else { continue };
        if target.flags & (SHF_ALLOC | SHF_EXECINSTR) != SHF_ALLOC | SHF_EXECINSTR {
            continue;
        }
        let Some(entries) = elf.data(rela) else {
            bail!("cannot read relocation section {}", rela.name);
        };
        for entry in entries.as_chunks::<24>().0 {
            let (Some(location), Some(info), Some(addend)) =
                (read_u64(entry, 0), read_u64(entry, 8), read_u64(entry, 16))
            else {
                continue;
            };
            if !matches!(info as u32, R_X86_64_PC32 | R_X86_64_PLT32) {
                continue;
            }
            let Some(&value) = symbols.get((info >> 32) as usize) else { continue };
            // The displacement is the last four bytes of its instruction,
            // so the branch leads to the symbol plus the addend, plus 4.
            let to = value.wrapping_add(addend).wrapping_add(4);
            if to != thunks.retpoline && to != thunks.ret {
                continue;
            }
            let at = location - 2;
            let Some(offset) = elf.file_offset(at) else {
                bail!("the thunk site at {at:#x} is outside the kernel file");
            };
            let Some(bytes) = elf.bytes.get(offset..offset + radix_thunks::SITE) else {
                bail!("the thunk site at {at:#x} is outside the kernel file");
            };
            let bytes: [u8; radix_thunks::SITE] = bytes.try_into().expect("six bytes");
            if let Err(err) = radix_thunks::decode(at, bytes, &thunks) {
                bail!(
                    "the branch to a thunk at {at:#x} is not one the kernel can rewrite: {err}; \
                     the kernel must be compiled with -Zindirect-branch-cs-prefix"
                );
            }
            // The boot CPU writes a site with an eight-byte store.
            if !(text_start..text_end.saturating_sub(8)).contains(&at) {
                bail!("the thunk site at {at:#x} is not well inside the kernel's text");
            }
            let Ok(offset) = u32::try_from(at - text_start) else {
                bail!("the thunk site at {at:#x} is too far into the text");
            };
            sites.insert(offset);
        }
    }
    Ok(sites.into_iter().collect())
}

/// Writes the table of thunk sites into the `.kthunks` section of the
/// kernel image `bytes`. Returns how many sites it lists.
pub fn embed(bytes: &mut [u8]) -> Result<usize> {
    let (count, offset, encoded) = {
        let Some(elf) = Elf::parse(bytes) else {
            bail!("the kernel is not a 64-bit little-endian ELF file");
        };
        let Some(section) = elf.section(".kthunks").cloned() else {
            bail!("the kernel has no .kthunks section to hold its table of thunk sites");
        };
        let sites = collect(&elf)?;
        let room = usize::try_from(section.size).unwrap_or(0);
        let mut encoded = vec![0u8; room];
        if let Err(err) = radix_thunks::encode(&sites, &mut encoded) {
            bail!(
                "writing the table of thunk sites into .kthunks ({room} bytes): {err}; enlarge it \
                 in kernel/arch/x86_64/src/boot.rs"
            );
        }
        (sites.len(), usize::try_from(section.offset).unwrap_or(usize::MAX), encoded)
    };
    let Some(target) = bytes.get_mut(offset..offset + encoded.len()) else {
        bail!("the .kthunks section lies outside the kernel file");
    };
    target.copy_from_slice(&encoded);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0xffff_ffff_8010_0000;
    const RETPOLINE: u64 = BASE + 0x80;
    const RETURN: u64 = BASE + 0xc0;
    const FUNCTION: u64 = BASE + 0xe0;

    /// `head` and the displacement from `at` to `to`.
    fn branch(text: &mut [u8], at: usize, head: &[u8], to: u64) {
        let end = BASE + (at + head.len() + 4) as u64;
        text[at..at + head.len()].copy_from_slice(head);
        let displacement = to.wrapping_sub(end) as i64 as i32;
        text[at + head.len()..at + head.len() + 4].copy_from_slice(&displacement.to_le_bytes());
    }

    /// An ELF file with 256 bytes of text at `BASE`, the symbols the
    /// table needs, a `.kthunks` section and the text's relocations, each
    /// (offset of its displacement in the text, symbol index).
    fn tiny_elf(text: &[u8], relocations: &[(u64, u32)]) -> Vec<u8> {
        let mut rela = Vec::new();
        for &(at, symbol) in relocations {
            rela.extend_from_slice(&(BASE + at).to_le_bytes());
            let info = u64::from(symbol) << 32 | u64::from(R_X86_64_PLT32);
            rela.extend_from_slice(&info.to_le_bytes());
            rela.extend_from_slice(&(-4i64).to_le_bytes());
        }
        let names = [
            "__kernel_text_start",
            "__kernel_text_end",
            "__x86_indirect_thunk_r11",
            "__x86_return_thunk",
            "radix_aligned_indirect_r11",
            "a_function",
        ];
        let values = [BASE, BASE + 0x100, RETPOLINE, RETURN, BASE + 0xa0, FUNCTION];
        let mut strtab = vec![0u8];
        let mut symtab = vec![0u8; 24];
        for (name, value) in names.iter().zip(values) {
            let mut entry = [0u8; 24];
            entry[0..4].copy_from_slice(&(strtab.len() as u32).to_le_bytes());
            entry[6..8].copy_from_slice(&1u16.to_le_bytes());
            entry[8..16].copy_from_slice(&value.to_le_bytes());
            symtab.extend_from_slice(&entry);
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
        }
        let shstrtab = b"\0.text\0.kthunks\0.rela.text\0.symtab\0.strtab\0.shstrtab\0".to_vec();
        let mut file = vec![0u8; 64];
        let mut place = |bytes: &[u8]| {
            let at = file.len() as u64;
            file.extend_from_slice(bytes);
            (at, bytes.len() as u64)
        };
        let t = place(text);
        let k = place(&[0u8; 64]);
        let r = place(&rela);
        let y = place(&symtab);
        let s = place(&strtab);
        let h = place(&shstrtab);
        let shoff = file.len() as u64;
        // (name, type, flags, addr, (offset, size), link, info)
        type Header = (u32, u32, u64, u64, (u64, u64), u32, u32);
        let headers: [Header; 7] = [
            (0, 0, 0, 0, (0, 0), 0, 0),
            (1, 1, SHF_ALLOC | SHF_EXECINSTR, BASE, t, 0, 0),
            (7, 1, SHF_ALLOC, BASE + 0x1000, k, 0, 0),
            (16, SHT_RELA, 0, 0, r, 4, 1),
            (27, SHT_SYMTAB, 0, 0, y, 5, 0),
            (35, 3, 0, 0, s, 0, 0),
            (43, 3, 0, 0, h, 0, 0),
        ];
        for (name, kind, flags, addr, (offset, size), link, info) in headers {
            let mut h = [0u8; 64];
            h[0..4].copy_from_slice(&name.to_le_bytes());
            h[4..8].copy_from_slice(&kind.to_le_bytes());
            h[8..16].copy_from_slice(&flags.to_le_bytes());
            h[16..24].copy_from_slice(&addr.to_le_bytes());
            h[24..32].copy_from_slice(&offset.to_le_bytes());
            h[32..40].copy_from_slice(&size.to_le_bytes());
            h[40..44].copy_from_slice(&link.to_le_bytes());
            h[44..48].copy_from_slice(&info.to_le_bytes());
            file.extend_from_slice(&h);
        }
        file[0..4].copy_from_slice(b"\x7fELF");
        file[4] = 2;
        file[5] = 1;
        file[0x28..0x30].copy_from_slice(&shoff.to_le_bytes());
        file[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        file[0x3c..0x3e].copy_from_slice(&7u16.to_le_bytes());
        file[0x3e..0x40].copy_from_slice(&6u16.to_le_bytes());
        file
    }

    /// Symbol indices, counting the null symbol.
    const SYM_RETPOLINE: u32 = 3;
    const SYM_RETURN: u32 = 4;
    const SYM_FUNCTION: u32 = 6;

    #[test]
    fn lists_the_branches_to_the_thunks_and_nothing_else() {
        let mut text = vec![0xccu8; 0x100];
        branch(&mut text, 0x10, &[0x2e, 0xe8], RETPOLINE);
        branch(&mut text, 0x20, &[0x2e, 0xe9], RETURN);
        branch(&mut text, 0x30, &[0x0f, 0x85], RETURN);
        branch(&mut text, 0x40, &[0xe8], FUNCTION);
        let relocations =
            [(0x32, SYM_RETURN), (0x12, SYM_RETPOLINE), (0x22, SYM_RETURN), (0x41, SYM_FUNCTION)];
        let mut elf = tiny_elf(&text, &relocations);
        assert_eq!(embed(&mut elf).unwrap(), 3);
        let parsed = Elf::parse(&elf).unwrap();
        let room = parsed.data(parsed.section(".kthunks").unwrap()).unwrap();
        let table = radix_thunks::Table::parse(room).unwrap();
        assert_eq!(table.offsets().collect::<Vec<_>>(), [0x10, 0x20, 0x30]);
    }

    #[test]
    fn refuses_a_branch_to_a_thunk_it_cannot_rewrite() {
        // A call without its CS prefix is five bytes, not six.
        let mut text = vec![0xccu8; 0x100];
        branch(&mut text, 0x11, &[0xe8], RETPOLINE);
        let mut elf = tiny_elf(&text, &[(0x12, SYM_RETPOLINE)]);
        let err = embed(&mut elf).unwrap_err().to_string();
        assert!(err.contains("-Zindirect-branch-cs-prefix"), "{err}");
        // A site whose eight bytes run past the text.
        let mut text = vec![0xccu8; 0x100];
        branch(&mut text, 0xfa, &[0x2e, 0xe9], RETURN);
        let mut elf = tiny_elf(&text, &[(0xfc, SYM_RETURN)]);
        assert!(embed(&mut elf).is_err());
        // More sites than the room holds.
        let mut text = vec![0xccu8; 0x100];
        let mut relocations = Vec::new();
        for site in 0..9 {
            branch(&mut text, site * 6, &[0x2e, 0xe9], RETURN);
            relocations.push((site as u64 * 6 + 2, SYM_RETURN));
        }
        let mut elf = tiny_elf(&text, &relocations);
        let err = embed(&mut elf).unwrap_err().to_string();
        assert!(err.contains("boot.rs"), "{err}");
    }
}
