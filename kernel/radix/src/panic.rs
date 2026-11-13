// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The panic handler and the panic report.
//!
//! A panic stops the other CPUs, then writes a report to every console: the
//! message, where it happened, the registers when an exception caused it,
//! a symbolized backtrace, and where each other CPU was when it stopped.
//! On the framebuffer the report fills the screen, with the most recent
//! log messages below it, because on a laptop without a serial port the
//! screen is all there is (or a Debug Capability console, which is given
//! up to 2 s to send the report). The crash log keeps the report for the
//! next boot, and a crash dump beside it holds the kernel's state: its
//! CPUs, threads, run queues, locks, memory and stacks (ADR 0483). Then the
//! kernel debugger runs if it was asked for, and finally the machine
//! restarts after a key press, or after the delay given with
//! `panic=seconds`.

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicUsize, Ordering};

use radix_hal::clock::civil_from_days;
use radix_hal::{Cpu, CpuLocal, Fault, Platform, Smp, StackWord, Timer};
use radix_klog::crashlog::Ending;
use radix_klog::{Style, TextBuf};

use crate::symbols::{self, Backtrace};
use crate::{Arch, console, crashlog, debug_console, kdb, report, sched, settings, usb};

/// The CPU that is panicking, plus one; 0 while none is.
static PANICKING: AtomicUsize = AtomicUsize::new(0);

/// The marker the self-test of the crash log looks for.
pub const SELFTEST_MARKER: &str = "selftest crash-reboot: deliberate panic";

/// Writes one line of the report to the serial port, the text screen and
/// the crash log (not the framebuffer, which `draw_screen` fills).
fn out(style: Style, args: fmt::Arguments<'_>) {
    let mut line = TextBuf::<512>::new();
    let _ = line.write_fmt(args);
    line.push_str("\n");
    radix_klog::write_raw(line.as_str(), style);
}

/// Everything the report shows.
struct Report<'a> {
    info: &'a PanicInfo<'a>,
    cpu: usize,
    time_ns: u64,
    stopped: usize,
    fault: Option<Fault>,
    backtrace: Backtrace,
    boot: u32,
}

impl Report<'_> {
    /// Calls `line` with each line of the report and its style.
    fn lines(&self, line: &mut dyn FnMut(Style, fmt::Arguments<'_>)) {
        let secs = self.time_ns / 1_000_000_000;
        let micros = self.time_ns % 1_000_000_000 / 1_000;
        line(
            Style::Error,
            format_args!(
                "Radix panic on CPU {} at {secs}.{micros:06} s: {}",
                self.cpu,
                self.info.message()
            ),
        );
        // After an exception the location is the exception handler, which
        // says nothing; the faulting address is in the message instead.
        if let Some(location) = self.info.location().filter(|_| self.fault.is_none()) {
            line(
                Style::Plain,
                format_args!("  at {}:{}:{}", location.file(), location.line(), location.column()),
            );
        }
        let mut cpus = TextBuf::<48>::new();
        match Arch::cpu_count().saturating_sub(1) {
            // A single CPU, or a panic before the others were found.
            0 => {
                let _ = write!(cpus, "no other CPUs");
            }
            others => {
                let _ = write!(cpus, "{} of {others} other CPUs stopped", self.stopped);
            }
        }
        line(
            Style::Plain,
            format_args!(
                "  Radix {} on {}, boot {}; {}",
                env!("CARGO_PKG_VERSION"),
                <Arch as Cpu>::NAME,
                self.boot,
                cpus.as_str()
            ),
        );
        if let Some(fault) = &self.fault {
            line(Style::Emphasis, format_args!("Registers:"));
            for row in fault.registers().chunks(4) {
                let mut text = TextBuf::<160>::new();
                for (name, value) in row {
                    let _ = write!(text, "  {name:>6} {value:016x}");
                }
                line(Style::Plain, format_args!("{text}"));
            }
        }
        line(Style::Emphasis, format_args!("Backtrace:"));
        self.backtrace.write(&mut |args| line(Style::Plain, args));
        // What the other CPUs were doing: one that did not answer a
        // request (a TLB shootdown, say) shows where it was stuck.
        let others = (0..Arch::cpu_count()).filter(|&index| index != self.cpu);
        let traces = others.filter_map(|index| Some((index, Backtrace::of_stopped_cpu(index)?)));
        for (index, trace) in traces.take(OTHER_TRACES) {
            line(Style::Emphasis, format_args!("CPU {index} when it stopped:"));
            trace.write(&mut |args| line(Style::Plain, args));
        }
    }
}

/// Other CPUs whose backtraces the report shows, at most.
const OTHER_TRACES: usize = 8;

/// The last line of the report: what happens next.
fn next_step(text: &mut TextBuf<160>) {
    let settings = settings();
    let _ = if settings.kdb { write!(text, "kdb is waiting on the serial port. ") } else { Ok(()) };
    let _ = match settings.panic_restart {
        Some(0) => write!(text, "Restarting now; the log is kept in memory for the next boot."),
        Some(secs) => {
            write!(text, "Restarting in {secs} s; the log is kept in memory for the next boot.")
        }
        None => {
            write!(text, "Press a key to restart; the log is kept in memory for the next boot.")
        }
    };
}

/// Draws the full-screen report on the framebuffer.
fn draw_screen(report: &Report<'_>) {
    console::take_over(|screen| {
        screen.clear();
        screen.fill_row(0, Style::Banner);
        screen.set_position(1, 0);
        screen.write("RADIX KERNEL PANIC", Style::Banner);
        screen.set_position(0, 2);
        report.lines(&mut |style, args| {
            let mut text = TextBuf::<512>::new();
            let _ = text.write_fmt(args);
            text.push_str("\n");
            screen.write(text.as_str(), style);
        });

        // Fill what is left, but for the footer, with recent log lines.
        let (_, row) = screen.position();
        let footer_rows = 2;
        let room = screen.rows().saturating_sub(row + footer_rows + 2);
        if room > 0 {
            screen.write("\nRecent kernel log:\n", Style::Emphasis);
            let range = radix_klog::record_range();
            let first = range.end.saturating_sub(room as u64).max(range.start);
            for seq in first..range.end {
                if let Some(record) = radix_klog::record(seq) {
                    let mut text = TextBuf::<300>::new();
                    let _ = record.format(&mut text);
                    let width = screen.cols().saturating_sub(1);
                    let shown = text
                        .as_str()
                        .char_indices()
                        .nth(width)
                        .map_or(text.as_str(), |(i, _)| &text.as_str()[..i]);
                    screen.write(shown, record.level.into());
                    screen.write("\n", Style::Plain);
                }
            }
        }
        let mut footer = TextBuf::<160>::new();
        next_step(&mut footer);
        let bottom = screen.rows() - 1;
        screen.fill_row(bottom, Style::Banner);
        screen.set_position(1, bottom);
        screen.write(footer.as_str(), Style::Banner);
    });
}

/// Bytes of the panicking CPU's stack the dump holds, and of each other
/// CPU's; and how many other CPUs' stacks it holds.
const OWN_STACK: usize = 8 * 1024;
const OTHER_STACK: usize = 2 * 1024;
const OTHER_STACKS: usize = 8;

/// Writes the crash dump's sections (ADR 0483): the report, then the
/// state kdb shows, then the stacks. `stack_pointer` is where the
/// panicking CPU's stack is copied from.
fn write_dump(
    dump: &mut crashlog::DumpWriter<'_>,
    report: &Report<'_>,
    wall_ns: Option<u64>,
    stack_pointer: usize,
) {
    dump.line(format_args!("Radix crash dump"));
    dump.line(format_args!(
        "Radix {} on {}, boot {}",
        env!("CARGO_PKG_VERSION"),
        <Arch as Cpu>::NAME,
        report.boot
    ));
    let (secs, micros) = (report.time_ns / 1_000_000_000, report.time_ns % 1_000_000_000 / 1_000);
    match wall_ns {
        Some(ns) => {
            dump.line(format_args!("Panic at {}, {secs}.{micros:06} s after boot", Rfc3339(ns)));
        }
        None => {
            dump.line(format_args!(
                "Panic at an unknown time of day, {secs}.{micros:06} s after boot"
            ));
        }
    }
    dump.section(format_args!("Panic"), &mut |line| report.lines(&mut |_, args| line(args)));
    dump.section(format_args!("CPUs"), &mut |line| report::write_cpu_activity(line));
    dump.section(format_args!("Threads"), &mut |line| sched::write_threads(line));
    dump.section(format_args!("Run queues"), &mut |line| sched::write_queues(line));
    dump.section(format_args!("Locks"), &mut |line| sched::write_lockdep(line));
    dump.section(format_args!("Memory"), &mut |line| {
        report::write_memory(line);
        line(format_args!("Kernel heap:"));
        report::write_heap(line);
    });
    let others = (0..Arch::cpu_count()).filter(|&index| index != report.cpu);
    let stopped = others.filter_map(|index| Some((index, Arch::stopped_stack_pointer(index)?)));
    let stacks = [(report.cpu, stack_pointer, OWN_STACK)]
        .into_iter()
        .chain(stopped.take(OTHER_STACKS).map(|(index, sp)| (index, sp, OTHER_STACK)));
    for (index, sp, max) in stacks {
        dump.section(format_args!("Stack of CPU {index}"), &mut |line| {
            if !Arch::read_stack(index, sp, max, &mut |word| stack_line(word, line)) {
                line(format_args!("{sp:016x} is on no kernel stack this CPU has"));
            }
        });
    }
}

/// A line of a stack in the crash dump: the word's address, its value,
/// and the function it is in when it is an address in the kernel's code,
/// as a return address is.
fn stack_line(word: StackWord, line: &mut dyn FnMut(fmt::Arguments<'_>)) {
    let StackWord { address, value, code } = word;
    match code.then(|| symbols::lookup_return(value)).flatten() {
        Some(location) => line(format_args!("{address:016x}  {value:016x}  {location}")),
        None => line(format_args!("{address:016x}  {value:016x}")),
    }
}

/// Nanoseconds since 1970 as RFC 3339 writes a time in UTC, to the second.
struct Rfc3339(u64);

impl fmt::Display for Rfc3339 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0 / 1_000_000_000;
        let (days, time) = (seconds / 86_400, seconds % 86_400);
        let (year, month, day) = civil_from_days(days as i64);
        let (hour, minute, second) = (time / 3600, time / 60 % 60, time % 60);
        write!(f, "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }
}

/// Waits for a key on the keyboard or the debug console, or for the restart
/// delay, counting down on the screen.
fn wait_then_restart() -> ! {
    match settings().panic_restart {
        Some(secs) => {
            for left in (1..=secs).rev() {
                let mut text = TextBuf::<160>::new();
                let _ = write!(
                    text,
                    " Restarting in {left} s; the log is kept in memory for the next boot."
                );
                console::update_line(0, text.as_str(), Style::Banner);
                for _ in 0..100 {
                    Arch::delay_us(10_000);
                }
            }
        }
        None => {
            // Drop keys typed before the panic.
            let _ = Arch::key_pressed();
            while debug_console::read().is_some() {}
            while !Arch::key_pressed() && debug_console::read().is_none() {
                Arch::delay_us(10_000);
            }
        }
    }
    Arch::reset()
}

/// Lets the panic path take locks that the stopped CPUs may still hold.
/// Call only after asking the other CPUs to stop.
#[allow(unsafe_code)]
fn take_locks_by_force() {
    // SAFETY: the other CPUs were just asked to stop (and those that did
    // not answer in time cannot be helped), so taking their locks by force
    // is the only way left to get the report out. The consoles tolerate
    // half-finished state.
    unsafe { radix_sync::enter_panic_mode() };
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    Arch::disable_interrupts();
    let cpu = Arch::current_cpu();
    if let Err(owner) = PANICKING.compare_exchange(0, cpu + 1, Ordering::AcqRel, Ordering::Acquire)
    {
        if owner == cpu + 1 {
            // A panic while reporting a panic: say so as simply as possible.
            let mut text = TextBuf::<512>::new();
            let _ =
                write!(text, "\nRadix panic while panicking on CPU {cpu}: {}\n", info.message());
            Arch::console_write(text.as_str());
            if settings().qemu_exit {
                Arch::emulator_exit(false);
            }
            // The report is out and the crash log ended; the dump ends after
            // its last whole section. A server restarts rather than stopping.
            if crashlog::writing_dump() {
                wait_then_restart();
            }
        }
        // Another CPU is reporting; it stops this one.
        Arch::halt();
    }

    let stopped = Arch::stop_other_cpus();
    take_locks_by_force();

    let fault = Arch::current_fault();
    let backtrace = match &fault {
        Some(fault) => Backtrace::from_fault(fault.pc, fault.frame_pointer),
        None => Backtrace::here(),
    };
    let wall_ns = Arch::wall_clock_ns_without_waiting();
    let report = Report {
        info,
        cpu,
        time_ns: Arch::now_ns(),
        stopped,
        fault,
        backtrace,
        boot: crashlog::boot_number(),
    };

    out(Style::Plain, format_args!(""));
    out(
        Style::Banner,
        format_args!("==================== RADIX KERNEL PANIC ===================="),
    );
    report.lines(&mut |style, args| out(style, args));
    let mut footer = TextBuf::<160>::new();
    next_step(&mut footer);
    out(Style::Emphasis, format_args!("{footer}"));
    if console::present() {
        draw_screen(&report);
    }
    crashlog::set_ending(Ending::Panicked);
    usb::dbc::flush();
    crate::serial::flush();

    let settings = settings();
    let planned_crash = settings.selftest == Some("crash-reboot");
    if settings.qemu_exit && !planned_crash {
        Arch::emulator_exit(false);
    }
    if settings.crash_dump {
        let stack_pointer = fault.as_ref().map_or_else(Arch::frame_pointer, |f| f.stack_pointer);
        crashlog::write_dump(wall_ns, &mut |dump| {
            write_dump(dump, &report, wall_ns, stack_pointer);
        });
    }
    if settings.kdb {
        kdb::run_after_panic(fault.as_ref(), &backtrace);
    }
    wait_then_restart()
}
