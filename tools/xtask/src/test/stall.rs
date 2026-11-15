// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where each CPU was when a boot test failed with the machine still
//! running. A hang in which the kernel prints nothing more, such as two
//! CPUs that each wait for a lock the other holds, or a CPU that waits
//! with every thread asleep, leaves the serial log silent; so the harness
//! stops the machine through QEMU's monitor and writes, beside the serial
//! log, each CPU's registers and, for a CPU in the kernel, the kernel's
//! functions down its stack. The kernel keeps frame pointers, so each
//! frame holds the one before it and the address it returns to.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::elf::Elf;
use crate::ksyms;

use super::Vm;

/// The frames followed down a kernel stack, at most.
const FRAMES: usize = 48;

/// The report's path beside the serial log `log`: `boot-NAME-cpus.log`
/// for `boot-NAME.log`.
pub fn report_path(log: &Path) -> PathBuf {
    let stem = log.file_stem().unwrap_or_default().to_string_lossy();
    log.with_file_name(format!("{stem}-cpus.log"))
}

/// The kernel's functions, link addresses first, and how far KASLR moved
/// the kernel, which the serial log says.
struct Symbols {
    functions: Vec<(u64, u64, String)>,
    moved: u64,
}

impl Symbols {
    fn read(kernel: &Path, log: &str) -> Option<Symbols> {
        let bytes = std::fs::read(kernel).ok()?;
        let elf = Elf::parse(&bytes)?;
        let near = elf.section(".ksyms")?.addr;
        let functions = ksyms::functions(&elf, near).ok()?;
        Some(Symbols { functions, moved: moved_by(log).unwrap_or(0) })
    }

    /// `address`, at which the kernel runs, as a function and an offset
    /// into it.
    fn name(&self, address: u64) -> Option<String> {
        let linked = address.wrapping_sub(self.moved);
        let at = self.functions.partition_point(|&(start, _, _)| start <= linked);
        let (start, size, name) = self.functions.get(at.checked_sub(1)?)?;
        (linked < start + size).then(|| format!("{name}+{:#x}", linked - start))
    }
}

/// How far the kernel was moved, from its line `KASLR: kernel moved by
/// 0x2ba00000 (...)`.
fn moved_by(log: &str) -> Option<u64> {
    let (_, rest) = log.split_once("KASLR: kernel moved by 0x")?;
    let digits: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    u64::from_str_radix(&digits, 16).ok()
}

/// The value of register `name` in QEMU's `info registers`, as in
/// `RIP=ffffffff8000abcd`.
fn register(registers: &str, name: &str) -> Option<u64> {
    let (_, rest) = registers.split_once(&format!("{name}="))?;
    let digits: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    u64::from_str_radix(&digits, 16).ok()
}

/// The words QEMU's monitor printed for `x /Ngx ADDRESS`: lines of an
/// address, a colon and the words.
fn words(dump: &str) -> Vec<u64> {
    dump.lines()
        .filter_map(|line| line.split_once(':'))
        .flat_map(|(_, words)| words.split_whitespace())
        .filter_map(|word| u64::from_str_radix(word.strip_prefix("0x")?, 16).ok())
        .collect()
}

/// Stops the machine and writes the report of its `cpus` CPUs, the kernel
/// `kernel` running them, to `path`. Answers the report without the
/// registers, for the test's failure to say: where each CPU is and the
/// functions down its stack, which CI's log then holds too.
pub fn write(vm: &mut Vm, cpus: u32, kernel: &Path, path: &Path) -> Result<String, String> {
    vm.monitor("stop")?;
    let symbols = Symbols::read(kernel, &vm.log());
    let name =
        |address: u64| symbols.as_ref().and_then(|s| s.name(address)).unwrap_or_else(|| "?".into());
    // The report, and what of it the failure says: all but the registers.
    let (mut out, mut summary) = (String::new(), String::new());
    if symbols.is_none() {
        let _ = writeln!(summary, "(no symbols: {} could not be read)", kernel.display());
        out.push_str(&summary);
    }
    for cpu in 0..cpus {
        let registers = vm.monitor_output("info registers", cpu)?;
        let mut said = String::new();
        let _ = writeln!(said, "CPU {cpu}");
        let rip = register(&registers, "RIP").unwrap_or(0);
        let kernel_mode = register(&registers, "CPL") == Some(0);
        let halted = register(&registers, "HLT") == Some(1);
        let _ = writeln!(
            said,
            "  {} at {rip:#018x} {}{}",
            if kernel_mode { "kernel" } else { "user" },
            if kernel_mode { name(rip) } else { String::new() },
            if halted { ", halted" } else { "" },
        );
        if kernel_mode {
            let mut frame = register(&registers, "RBP").unwrap_or(0);
            for _ in 0..FRAMES {
                if frame == 0 || !frame.is_multiple_of(8) {
                    break;
                }
                let pair = words(&vm.monitor_output(&format!("x /2gx {frame:#x}"), cpu)?);
                let [next, back] = pair[..] else { break };
                if back == 0 {
                    break;
                }
                let _ = writeln!(said, "    {back:#018x} {}", name(back));
                // A frame further down the stack is at a higher address.
                if next <= frame {
                    break;
                }
                frame = next;
            }
        }
        out.push_str(&said);
        summary.push_str(&said);
        for line in registers.lines() {
            let _ = writeln!(out, "  | {line}");
        }
    }
    std::fs::write(path, out).map_err(|err| format!("writing {}: {err}", path.display()))?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_registers_words_and_the_move() {
        let registers = "RAX=0000000000000001 RBX=0000000000000002\n\
                         RSI=0 RDI=0 RBP=ffff800000123f80 RSP=ffff800000123f00\n\
                         RIP=ffffffff8012abcd RFL=00000246 [---Z-P-] CPL=0 II=0 A20=1 SMM=0 HLT=1\n";
        assert_eq!(register(registers, "RIP"), Some(0xffff_ffff_8012_abcd));
        assert_eq!(register(registers, "RBP"), Some(0xffff_8000_0012_3f80));
        assert_eq!(register(registers, "CPL"), Some(0));
        assert_eq!(register(registers, "HLT"), Some(1));
        assert_eq!(register(registers, "R15"), None);
        let dump = "ffff800000123f80: 0xffff800000123fc0 0xffffffff80101234\r\n";
        assert_eq!(words(dump), [0xffff_8000_0012_3fc0, 0xffff_ffff_8010_1234]);
        assert!(words("ffff800000000000: Cannot access memory\r\n").is_empty());
        let log = "[    0.000000] KASLR: kernel moved by 0x2ba00000 (one of 499 places)\n";
        assert_eq!(moved_by(log), Some(0x2ba0_0000));
        assert_eq!(moved_by("no such line"), None);
        assert_eq!(
            report_path(Path::new("out/test/boot-admin-actions.log")),
            Path::new("out/test/boot-admin-actions-cpus.log")
        );
    }

    #[test]
    fn names_an_address_where_the_kernel_was_moved() {
        let symbols = Symbols {
            functions: vec![(0x1000, 0x100, "a".into()), (0x1100, 0x40, "b".into())],
            moved: 0x2000,
        };
        assert_eq!(symbols.name(0x3010).as_deref(), Some("a+0x10"));
        assert_eq!(symbols.name(0x3100).as_deref(), Some("b+0x0"));
        assert_eq!(symbols.name(0x3140), None);
        assert_eq!(symbols.name(0x2fff), None);
    }
}
