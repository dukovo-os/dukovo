// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A measurement, never merged: how long each way of copying a picture
//! of the screen's size from memory to the framebuffer takes at boot.

use core::arch::asm;

use crate::paging::{self, Access, Cache};
use crate::{cpu, memory};

/// Where the pictures are copied from: RAM that nothing writes at boot.
const SOURCE: u64 = 64 << 20;

/// How many times each way is timed; the least and the middle are logged.
const TIMES: usize = 15;

type Copy = unsafe fn(*mut u8, *const u8, usize);

unsafe fn kernel_memcpy(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, core::hint::black_box(len)) };
}

unsafe fn movsb(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's.
    unsafe {
        asm!("rep movsb", inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") len => _,
            options(nostack, preserves_flags));
    }
}

unsafe fn movsl(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's; every length here is a multiple of 4.
    unsafe {
        asm!("rep movsd", inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") len / 4 => _,
            options(nostack, preserves_flags));
    }
}

unsafe fn movsq(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's; every length here is a multiple of 8.
    unsafe {
        asm!("rep movsq", inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") len / 8 => _,
            options(nostack, preserves_flags));
    }
}

unsafe fn registers(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's; every length here is a multiple of 32.
    unsafe {
        asm!(
            "2:",
            "mov {a}, [rsi]",
            "mov {b}, [rsi + 8]",
            "mov {c}, [rsi + 16]",
            "mov {d}, [rsi + 24]",
            "mov [rdi], {a}",
            "mov [rdi + 8], {b}",
            "mov [rdi + 16], {c}",
            "mov [rdi + 24], {d}",
            "add rsi, 32",
            "add rdi, 32",
            "sub rcx, 32",
            "ja 2b",
            a = out(reg) _, b = out(reg) _, c = out(reg) _, d = out(reg) _,
            inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") len => _,
            options(nostack),
        );
    }
}

unsafe fn in_order(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's.
    unsafe { crate::copy::in_order(dst, src, len) };
}

/// What a locked instruction is done on, as taking a lock does one.
static LOCKED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

unsafe fn movnti(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: the caller's; every length here is a multiple of 32.
    unsafe {
        asm!(
            "2:",
            "mov {a}, [rsi]",
            "mov {b}, [rsi + 8]",
            "mov {c}, [rsi + 16]",
            "mov {d}, [rsi + 24]",
            "movnti [rdi], {a}",
            "movnti [rdi + 8], {b}",
            "movnti [rdi + 16], {c}",
            "movnti [rdi + 24], {d}",
            "add rsi, 32",
            "add rdi, 32",
            "sub rcx, 32",
            "ja 2b",
            "sfence",
            a = out(reg) _, b = out(reg) _, c = out(reg) _, d = out(reg) _,
            inout("rdi") dst => _, inout("rsi") src => _, inout("rcx") len => _,
            options(nostack),
        );
    }
}

/// How a picture's rows are handed to the copy.
#[derive(Clone, Copy)]
enum Rows {
    /// A row at a time.
    Whole,
    /// Each row cut where the source crosses a page, as the display
    /// layer's pictures are.
    Cut,
    /// Every row in one copy.
    Frame,
    /// As `Cut`, with a locked instruction after every so many rows.
    Locked(usize),
}

#[derive(Clone, Copy)]
struct Screen {
    pitch: usize,
    row: usize,
    height: usize,
}

/// Copies one picture to `screen` from `source` and returns the ticks.
fn once(screen: *mut u8, shape: &Screen, source: usize, copy: Copy, rows: Rows) -> u64 {
    let started = cpu::rdtsc();
    for y in 0..shape.height {
        let (mut from, mut left) = (source + y * shape.row, shape.row);
        // SAFETY: the row is within the framebuffer's mapping.
        let mut to = unsafe { screen.add(y * shape.pitch) };
        if matches!(rows, Rows::Frame) {
            // SAFETY: the screen's rows follow one another without a gap
            // here (the caller checked), and the source is RAM.
            unsafe { copy(to, from as *const u8, shape.row * shape.height) };
            break;
        }
        while left > 0 {
            let step = match rows {
                Rows::Cut | Rows::Locked(_) => left.min(4096 - from % 4096),
                Rows::Whole | Rows::Frame => left,
            };
            // SAFETY: within the row and within the RAM the caller checked.
            unsafe { copy(to, from as *const u8, step) };
            // SAFETY: still within the row, or at its end.
            to = unsafe { to.add(step) };
            from += step;
            left -= step;
        }
        if let Rows::Locked(every) = rows
            && y % every == every - 1
        {
            LOCKED.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        }
    }
    cpu::rdtsc() - started
}

/// Times one way and logs its least and middle ticks.
fn time(what: &str, screen: *mut u8, shape: &Screen, source: usize, copy: Copy, rows: Rows) {
    let mut ticks = [0u64; TIMES];
    for slot in &mut ticks {
        *slot = once(screen, shape, source, copy, rows);
    }
    ticks.sort_unstable();
    radix_klog::info!("FBDIAG {what}: least {} middle {} ticks", ticks[0], ticks[TIMES / 2]);
}

/// Runs the measurement on the framebuffer at `phys`, mapped
/// write-combining at `screen`.
pub fn run(phys: u64, screen: &mut [u8], pitch: usize, width: usize, height: usize) {
    let shape = Screen { pitch, row: width * 4, height };
    let bytes = (shape.row * height) as u64;
    if !memory::is_ram(SOURCE..SOURCE + bytes + (2 << 20)) || screen.len() < pitch * height {
        radix_klog::info!("FBDIAG: not run: no RAM at {SOURCE:#x} or a short screen");
        return;
    }
    // SAFETY: MSR 0x277 is the page attribute table, which every CPU
    // this kernel runs on has.
    let pat = unsafe { cpu::rdmsr(cpu::msr::PAT) };
    radix_klog::info!(
        "FBDIAG screen {width}x{height} pitch {pitch} at {phys:#x}, mapped at {:#x}; PAT {pat:#x}; \
         source at {SOURCE:#x}",
        screen.as_ptr() as usize
    );
    let wc = screen.as_mut_ptr();
    let cached =
        paging::map_physical(phys, (pitch * height) as u64, Access::ReadWrite, Cache::WriteBack)
            as *mut u8;
    // A page into the source, so that one before it is RAM too.
    let source = memory::phys_to_virt(SOURCE) + 4096;
    let frame = if pitch == shape.row { Rows::Frame } else { Rows::Whole };
    // The first pass brings the source into the cache.
    time("warm-up (WC, rep movsb, rows)", wc, &shape, source, movsb, Rows::Whole);
    for (name, to) in [("WC", wc), ("WB", cached)] {
        radix_klog::info!("FBDIAG to the {name} mapping at {:#x}", to as usize);
        time("kernel memcpy, rows cut at pages", to, &shape, source, kernel_memcpy, Rows::Cut);
        time("kernel memcpy, rows cut, source +16", to, &shape, source + 16, kernel_memcpy, Rows::Cut);
        time("kernel memcpy, rows cut, source +2048", to, &shape, source + 2048, kernel_memcpy, Rows::Cut);
        time("kernel memcpy, whole rows", to, &shape, source, kernel_memcpy, Rows::Whole);
        time("rep movsb, whole rows", to, &shape, source, movsb, Rows::Whole);
        time("rep movsb, rows cut at pages", to, &shape, source, movsb, Rows::Cut);
        time("rep movsb, rows cut, source +16", to, &shape, source + 16, movsb, Rows::Cut);
        time("rep movsb, whole rows, source -16", to, &shape, source - 16, movsb, Rows::Whole);
        time("rep movsb, whole rows, source +16", to, &shape, source + 16, movsb, Rows::Whole);
        time("rep movsb, one copy", to, &shape, source, movsb, frame);
        time("rep movsl, whole rows", to, &shape, source, movsl, Rows::Whole);
        time("rep movsq, whole rows", to, &shape, source, movsq, Rows::Whole);
        time("registers, whole rows", to, &shape, source, registers, Rows::Whole);
        time("movnti, whole rows", to, &shape, source, movnti, Rows::Whole);
        time("in order, whole rows", to, &shape, source, in_order, Rows::Whole);
        time("in order, rows cut at pages", to, &shape, source, in_order, Rows::Cut);
        time("in order, rows cut, source +16", to, &shape, source + 16, in_order, Rows::Cut);
        time("in order, rows cut, source +2048", to, &shape, source + 2048, in_order, Rows::Cut);
        time("in order, rows cut, source +2", to, &shape, source + 2, in_order, Rows::Cut);
        let shorter = Screen { height: height - 1, ..shape };
        // SAFETY: four bytes on, with a row fewer, is within the mapping.
        let on = unsafe { to.add(4) };
        time("in order, rows cut, screen +4", on, &shorter, source, in_order, Rows::Cut);
        time("in order, cut, locked every row", to, &shape, source, in_order, Rows::Locked(1));
        time("in order, cut, locked every 3 rows", to, &shape, source, in_order, Rows::Locked(3));
        time("in order, cut, locked every 12 rows", to, &shape, source, in_order, Rows::Locked(12));
        time("in order, cut, locked every 48 rows", to, &shape, source, in_order, Rows::Locked(48));
        time("in order, one copy", to, &shape, source, in_order, frame);
    }
    radix_klog::info!("FBDIAG done");
}
