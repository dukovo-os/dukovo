// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A crash record's stack trace (ADR 0531): its return addresses, found
//! from the program counter, stack pointer and frame pointer the record
//! holds and the copy of the stack it holds, with each program's call
//! frame information (`.eh_frame`) and, where an address has none, the
//! chain of frame pointers; then each address named from the file's
//! symbol table (`.symtab`, else `.dynsym`), as far as it has one.
//!
//! Only addresses and names come out: the stack's bytes are read here and
//! nowhere else, and go with the record. The files are read from the
//! disk by the paths the kernel gave, and each is checked to be an ELF
//! file whose segments hold the mapped offsets; a file that changed since
//! the crash gives wrong names at worst, never a wrong read.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use gimli::{
    BaseAddresses, CfaRule, EhFrame, EhFrameHdr, EndianSlice, LittleEndian, ParsedEhFrameHdr,
    Register, RegisterRule, UnwindContext, UnwindSection,
};
use object::read::ReadCache;
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol, SymbolKind};

use super::record::Record;

/// The most frames a trace has.
pub const FRAMES_MAX: usize = 64;
/// The largest file read for its unwinding and names: larger files are
/// named by offset only.
const FILE_MAX: u64 = 1024 * 1024 * 1024;
/// How many files are kept read between crashes: each holds its call
/// frame information and its functions' names, megabytes for a large
/// program.
const FILES_KEPT: usize = 4;

/// x86-64's DWARF register numbers.
const RBP: Register = Register(6);
const RSP: Register = Register(7);
const RA: Register = Register(16);

/// A frame of a trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Where it is: the program counter for the first, the return
    /// address for each after.
    pub address: u64,
    /// The file it is in, and where in the file.
    pub file: Option<Place>,
    /// The function it is in, readable, and how far into it.
    pub function: Option<(String, u64)>,
}

/// Where an address is in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The file's path.
    pub path: String,
    /// The address's offset in the file.
    pub offset: u64,
    /// The file's build ID, in hex.
    pub build_id: Option<String>,
}

impl Frame {
    /// The frame as a line: `name+0x4c (path +0x5a2c1c, build 7be9…)`.
    pub fn line(&self) -> String {
        let function = match &self.function {
            Some((name, offset)) => format!("{name}+{offset:#x}"),
            None => format!("{:#x}", self.address),
        };
        match &self.file {
            Some(Place { path, offset, build_id: Some(id) }) => {
                format!("{function} ({path} +{offset:#x}, build {id})")
            }
            Some(Place { path, offset, build_id: None }) => {
                format!("{function} ({path} +{offset:#x})")
            }
            None => function,
        }
    }
}

/// An ELF file, read for unwinding and naming.
struct Elf {
    /// Its loadable segments: virtual address, file offset and size in
    /// the file.
    segments: Vec<(u64, u64, u64)>,
    /// `.eh_frame`'s bytes and address, and `.eh_frame_hdr`'s.
    eh_frame: Option<(Vec<u8>, u64)>,
    eh_frame_hdr: Option<(Vec<u8>, u64)>,
    /// `.text`'s address, which call frame information may be relative to.
    text: u64,
    /// Its functions, by address: start, size, name.
    functions: Vec<(u64, u64, String)>,
    build_id: Option<String>,
}

impl Elf {
    fn read(path: &Path) -> Result<Elf, String> {
        let file = File::open(path).map_err(|err| crate::util::strip_os_error(&err))?;
        let len = file.metadata().map_err(|err| crate::util::strip_os_error(&err))?.len();
        if len > FILE_MAX {
            return Err(format!("larger than {} MiB", FILE_MAX >> 20));
        }
        // Only the parts asked for are read: the headers, two sections and
        // the symbol tables, not the code.
        let data = ReadCache::new(file);
        let elf = object::File::parse(&data).map_err(|err| format!("not an ELF file: {err}"))?;
        if elf.architecture() != object::Architecture::X86_64 {
            return Err("not a program for this machine".into());
        }
        let segments = elf
            .segments()
            .map(|s| {
                let (offset, size) = s.file_range();
                (s.address(), offset, size)
            })
            .collect();
        let section = |name: &str| {
            let section = elf.section_by_name(name)?;
            Some((section.data().ok()?.to_vec(), section.address()))
        };
        let text = elf.section_by_name(".text").map_or(0, |s| s.address());
        let mut functions: Vec<(u64, u64, String)> = elf
            .symbols()
            .chain(elf.dynamic_symbols())
            .filter(|s| s.kind() == SymbolKind::Text && s.address() != 0)
            .filter_map(|s| Some((s.address(), s.size(), readable(s.name().ok()?))))
            .collect();
        functions.sort_by_key(|f| (f.0, std::cmp::Reverse(f.1)));
        functions.dedup_by_key(|f| f.0);
        let build_id = elf.build_id().ok().flatten().map(super::server::hex);
        Ok(Elf {
            segments,
            eh_frame: section(".eh_frame"),
            eh_frame_hdr: section(".eh_frame_hdr"),
            text,
            functions,
            build_id,
        })
    }

    /// The virtual address the file's `offset` is loaded at.
    fn address_of(&self, offset: u64) -> Option<u64> {
        self.segments
            .iter()
            .find(|&&(_, at, size)| at <= offset && offset < at + size)
            .map(|&(address, at, _)| address + (offset - at))
    }

    /// The function `address` is in, and how far into it.
    fn function(&self, address: u64) -> Option<(String, u64)> {
        let at = self.functions.partition_point(|f| f.0 <= address).checked_sub(1)?;
        let (start, size, name) = &self.functions[at];
        // A symbol of no size covers up to the next one.
        let end = if *size == 0 {
            self.functions.get(at + 1).map_or(u64::MAX, |f| f.0)
        } else {
            start + size
        };
        (address < end).then(|| (name.clone(), address - start))
    }

    /// The stack pointer and frame pointer of the caller, and the return
    /// address, for a frame at `address` (in the file's addresses) whose
    /// registers are `sp` and `fp`; none where the file has no rule for
    /// it, or its rule is one this unwinder does not follow.
    fn step(&self, address: u64, sp: u64, fp: u64, record: &Record) -> Option<Step> {
        let (eh_frame, eh_address) = self.eh_frame.as_ref()?;
        let section = EhFrame::new(eh_frame, LittleEndian);
        let mut bases = BaseAddresses::default().set_eh_frame(*eh_address).set_text(self.text);
        let mut context = Box::new(UnwindContext::new());
        let row = match &self.eh_frame_hdr {
            Some((hdr, hdr_address)) => {
                bases = bases.set_eh_frame_hdr(*hdr_address);
                let parsed: ParsedEhFrameHdr<EndianSlice<'_, LittleEndian>> =
                    EhFrameHdr::new(hdr, LittleEndian).parse(&bases, 8).ok()?;
                let table = parsed.table()?;
                table
                    .unwind_info_for_address(
                        &section,
                        &bases,
                        &mut context,
                        address,
                        EhFrame::cie_from_offset,
                    )
                    .ok()?
                    .clone()
            }
            None => section
                .unwind_info_for_address(&bases, &mut context, address, EhFrame::cie_from_offset)
                .ok()?
                .clone(),
        };
        let cfa = match row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                let base = match *register {
                    RSP => sp,
                    RBP => fp,
                    _ => return None,
                };
                base.checked_add_signed(*offset)?
            }
            CfaRule::Expression(_) => return None,
        };
        let saved = |register: Register, now: u64| match row.register(register) {
            RegisterRule::Undefined | RegisterRule::SameValue => Some(now),
            RegisterRule::Offset(offset) => record.stack_word(cfa.checked_add_signed(offset)?),
            RegisterRule::ValOffset(offset) => cfa.checked_add_signed(offset),
            _ => None,
        };
        let ra = match row.register(RA) {
            RegisterRule::Offset(offset) => record.stack_word(cfa.checked_add_signed(offset)?)?,
            _ => return None,
        };
        Some(Step { sp: cfa, fp: saved(RBP, fp)?, ra })
    }
}

/// What one step of the unwinding found.
struct Step {
    sp: u64,
    fp: u64,
    ra: u64,
}

/// The files read for earlier crashes, kept by path, size and time of
/// change, so that a program that crashes again is not read again, and
/// why the files that could not be read were not, each said once.
#[derive(Default)]
pub struct Files {
    kept: HashMap<String, (u64, i64, Option<Arc<Elf>>)>,
    problems: Vec<String>,
    told: std::collections::HashSet<String>,
}

impl Files {
    /// Why files were not read, since the last time this was asked: each
    /// path once while reportd runs.
    pub fn problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    fn problem(&mut self, path: &str, why: &str) {
        if self.told.len() < 1024 && self.told.insert(path.to_string()) {
            self.problems.push(format!("{path}: {why}"));
        }
    }

    /// The file at `path`, read, if it is an ELF file for this machine.
    fn get(&mut self, path: &str) -> Option<Arc<Elf>> {
        use std::os::unix::fs::MetadataExt;
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) => {
                self.problem(path, &crate::util::strip_os_error(&err));
                return None;
            }
        };
        // A device or a pipe is never read: only a program's own file.
        if !meta.is_file() {
            self.problem(path, "not a regular file");
            return None;
        }
        let key = (meta.len(), meta.mtime());
        if let Some((len, mtime, elf)) = self.kept.get(path)
            && (*len, *mtime) == key
        {
            return elf.clone();
        }
        if self.kept.len() >= FILES_KEPT {
            self.kept.clear();
        }
        let elf = match Elf::read(Path::new(path)) {
            Ok(elf) => Some(Arc::new(elf)),
            Err(why) => {
                self.problem(path, &why);
                None
            }
        };
        self.kept.insert(path.to_string(), (key.0, key.1, elf.clone()));
        elf
    }
}

/// The trace of `record`'s thread, at most [`FRAMES_MAX`] frames.
pub fn trace(record: &Record, files: &mut Files) -> Vec<Frame> {
    let mut frames = Vec::new();
    let (mut pc, mut sp, mut fp) = (record.pc, record.sp, record.fp);
    // A jump or call to an address with nothing there: the caller's
    // return address is the word the call pushed.
    if record.mapping_of(pc).is_none()
        && let Some(ra) = record.stack_word(sp)
        && record.mapping_of(ra).is_some()
    {
        frames.push(name(record, files, pc, false));
        pc = ra;
        sp += 8;
    }
    while frames.len() < FRAMES_MAX {
        let caller = !frames.is_empty();
        frames.push(name(record, files, pc, caller));
        // A return address is past its call: the call is the byte before.
        let lookup = if caller { pc.saturating_sub(1) } else { pc };
        let step = located(record, files, lookup)
            .and_then(|(elf, address)| elf.step(address, sp, fp, record))
            .or_else(|| by_frame_pointer(record, sp, fp));
        let Some(step) = step else { break };
        if step.ra == 0 || step.sp <= sp || record.mapping_of(step.ra).is_none() {
            break;
        }
        (pc, sp, fp) = (step.ra, step.sp, step.fp);
    }
    frames
}

/// The step the frame pointer chain gives: the caller's frame pointer
/// at `fp`, the return address above it.
fn by_frame_pointer(record: &Record, sp: u64, fp: u64) -> Option<Step> {
    if fp < sp {
        return None;
    }
    let caller_fp = record.stack_word(fp)?;
    let ra = record.stack_word(fp.checked_add(8)?)?;
    Some(Step { sp: fp + 16, fp: caller_fp, ra })
}

/// The file `address` is in, read, and the address in the file's own
/// addresses.
fn located(record: &Record, files: &mut Files, address: u64) -> Option<(Arc<Elf>, u64)> {
    let mapping = record.mapping_of(address)?;
    let elf = files.get(mapping.path.as_deref()?)?;
    let at = elf.address_of(mapping.offset + (address - mapping.start))?;
    Some((elf, at))
}

/// The frame at `address`: its file, offset and function, as far as they
/// are known. A caller's frame is named by the byte before its return
/// address, which is the call.
fn name(record: &Record, files: &mut Files, address: u64, caller: bool) -> Frame {
    let lookup = if caller { address.saturating_sub(1) } else { address };
    let Some(mapping) = record.mapping_of(lookup) else {
        return Frame { address, file: None, function: None };
    };
    let offset = mapping.offset + (address - mapping.start);
    let elf = mapping.path.as_deref().and_then(|path| files.get(path));
    let function = elf.as_ref().and_then(|elf| {
        let at = elf.address_of(mapping.offset + (lookup - mapping.start))?;
        let (name, into) = elf.function(at)?;
        Some((name, into + u64::from(caller)))
    });
    let file = mapping.path.clone().map(|path| Place {
        path,
        offset,
        build_id: elf.as_ref().and_then(|elf| elf.build_id.clone()),
    });
    Frame { address, file, function }
}

/// A symbol's name made readable: Rust's demangled without its hash, the
/// rest as they are.
fn readable(name: &str) -> String {
    format!("{:#}", rustc_demangle::demangle(name))
}

#[cfg(test)]
mod tests {
    use super::super::record::{Mapping, Record};
    use super::*;

    /// A record of a thread of a program at `path` mapped at 0x10_0000
    /// from file offset 0, whose stack holds `words` from 0x7000_0000.
    fn record(path: &str, pc: u64, fp: u64, words: &[u64]) -> Record {
        Record {
            dropped: 0,
            signal: 11,
            code: 1,
            address: 0,
            process: 9,
            thread: 9,
            user: 0,
            group: 0,
            ancestors: vec![],
            wall_ns: None,
            thread_name: "t".into(),
            pc,
            sp: 0x7000_0000,
            fp,
            path: Some(path.into()),
            domain: String::new(),
            mappings: vec![Mapping {
                start: 0x10_0000,
                end: 0x20_0000,
                offset: 0,
                path: Some(path.into()),
            }],
            stack: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }

    #[test]
    fn frame_pointers_give_the_callers_where_there_is_no_file() {
        // fp at 0x7000_0010: [saved fp 0x7000_0030][ra 0x10_2000]; at
        // 0x7000_0030: [saved fp 0][ra 0x10_3000].
        let words = [0, 0, 0x7000_0030, 0x10_2000, 0, 0, 0, 0x10_3000];
        let record = record("/nowhere/program", 0x10_1000, 0x7000_0010, &words);
        let frames = trace(&record, &mut Files::default());
        let addresses: Vec<u64> = frames.iter().map(|f| f.address).collect();
        assert_eq!(addresses, [0x10_1000, 0x10_2000, 0x10_3000]);
        assert_eq!(frames[1].file.as_ref().map(|p| p.offset), Some(0x2000));
        assert_eq!(frames[1].line(), "0x102000 (/nowhere/program +0x2000)");
    }

    #[test]
    fn a_call_to_nothing_starts_from_the_word_it_pushed() {
        let words = [0x10_4000, 0];
        let record = record("/nowhere/program", 0, 0, &words);
        let frames = trace(&record, &mut Files::default());
        assert_eq!(frames[0].address, 0);
        assert_eq!(frames[0].file, None);
        assert_eq!(frames[1].address, 0x10_4000);
    }

    #[test]
    fn a_frame_pointer_that_goes_down_ends_the_trace() {
        let words = [0, 0, 0x7000_0000, 0x10_2000];
        let record = record("/nowhere/program", 0x10_1000, 0x7000_0010, &words);
        let frames = trace(&record, &mut Files::default());
        // The second step would not move up the stack.
        assert_eq!(frames.len(), 2);
    }

    /// This test's own program, which the host built with call frame
    /// information and its symbols: a trace from a real stack of it.
    #[inline(never)]
    fn innermost(out: &mut Option<(u64, u64, u64, Vec<u8>)>) {
        let (pc, sp, fp): (u64, u64, u64);
        // SAFETY: reads three registers and writes nothing.
        unsafe {
            std::arch::asm!(
                "lea {pc}, [rip]",
                "mov {sp}, rsp",
                "mov {fp}, rbp",
                pc = out(reg) pc,
                sp = out(reg) sp,
                fp = out(reg) fp,
            );
        }
        // To 16 KiB up, or the end of the stack's mapping, as Linux has it:
        // a test's thread may have less above it.
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let end = maps
            .lines()
            .filter_map(|line| {
                let (start, end) = line.split_whitespace().next()?.split_once('-')?;
                Some((u64::from_str_radix(start, 16).ok()?, u64::from_str_radix(end, 16).ok()?))
            })
            .find(|&(start, end)| start <= sp && sp < end)
            .map(|(_, end)| end)
            .unwrap();
        let len = (end - sp).min(16 * 1024) as usize;
        // SAFETY: the bytes from this frame's stack pointer to `len` are
        // this thread's stack, mapped, and its callers' frames, which
        // outlive the copy.
        let stack = unsafe { std::slice::from_raw_parts(sp as *const u8, len) }.to_vec();
        *out = Some((pc, sp, fp, stack));
    }

    #[inline(never)]
    fn middle(out: &mut Option<(u64, u64, u64, Vec<u8>)>) {
        innermost(out);
        std::hint::black_box(());
    }

    #[test]
    fn a_trace_of_this_program_names_its_functions() {
        let mut taken = None;
        middle(&mut taken);
        let (pc, sp, fp, stack) = taken.unwrap();
        let exe = std::fs::read_link("/proc/self/exe").unwrap();
        let exe = exe.to_str().unwrap().to_string();
        // Where the program is mapped, from Linux's own record of it.
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let mappings = maps
            .lines()
            .filter(|l| {
                l.ends_with(&exe) && l.split_whitespace().nth(1).is_some_and(|p| p.contains('x'))
            })
            .map(|l| {
                let mut fields = l.split_whitespace();
                let (start, end) = fields.next().unwrap().split_once('-').unwrap();
                fields.next();
                let offset = u64::from_str_radix(fields.next().unwrap(), 16).unwrap();
                Mapping {
                    start: u64::from_str_radix(start, 16).unwrap(),
                    end: u64::from_str_radix(end, 16).unwrap(),
                    offset,
                    path: Some(exe.clone()),
                }
            })
            .collect();
        let record = Record { pc, sp, fp, mappings, stack, ..record(&exe, 0, 0, &[]) };
        let frames = trace(&record, &mut Files::default());
        let names: Vec<String> =
            frames.iter().filter_map(|f| f.function.as_ref().map(|n| n.0.clone())).collect();
        let at = |part: &str| names.iter().position(|n| n.ends_with(part));
        let (inner, middle) = (at("unwind::tests::innermost"), at("unwind::tests::middle"));
        assert!(inner.is_some() && middle.is_some(), "{names:#?}");
        assert!(inner < middle, "{names:#?}");
        assert!(at("a_trace_of_this_program_names_its_functions").is_some(), "{names:#?}");
        assert!(frames.iter().all(|f| f.file.as_ref().is_none_or(|p| p.path == exe)));
    }
}
