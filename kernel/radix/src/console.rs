// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The framebuffer console: the kernel log on the screen, and the screen
//! the panic report takes over. The screen is also the display layer's
//! first card (`crate::display`, ADR 0705): while one of the layer's
//! holders is in front the log stays off it, and is drawn again when the
//! front is the log's.
//!
//! With the `quiet` flag, as an installed system starts (ADR 0426), the
//! screen shows the startup screen instead: Dukovo's name and a bar that
//! fills as the kernel and then Spark's services start ([`progress`],
//! [`started_process`]). The log is kept, unseen, and shows again
//! ([`reveal`]) when Esc is pressed, when an error is logged, or when no
//! program has taken the screen a minute after the boot began; the serial
//! port and dmesg(1) get it all either way.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use radix_fbcon::Console;
use radix_hal::{Framebuffer, PixelFormat};
use radix_klog::{Sink, Style, info, warn};
use radix_sync::{SpinLock, TakeOnce};

/// Cells of grid storage: enough for 1920x1080 with the smallest font
/// (240 x 67, twice), or 4K with the font picked for it.
const STORAGE_CELLS: usize = 96 * 1024;

static STORAGE: TakeOnce<[u32; STORAGE_CELLS]> = TakeOnce::new([0; STORAGE_CELLS]);
/// The console. The log's sink takes it from interrupts too (the log masks
/// them around its sinks), so everything else takes it with them masked.
static SCREEN: SpinLock<Option<Console>> = SpinLock::new(None);
/// Set by the panic path, which draws its report itself.
static TAKEN_OVER: AtomicBool = AtomicBool::new(false);
/// Set while a holder of the display layer is in front, and draws on the
/// screen.
static HANDED_OVER: AtomicBool = AtomicBool::new(false);
/// Set while the startup screen shows in place of the log.
static QUIET: AtomicBool = AtomicBool::new(false);
/// How far the startup screen's bar is, of 1000.
static PROGRESS: AtomicU32 = AtomicU32::new(0);
/// When the startup screen gives way to the log if no program has taken
/// the screen, in `sched::now_ns`; 0 while it does not show.
static QUIET_UNTIL: AtomicU64 = AtomicU64::new(0);

/// The startup screen's name and hint.
const TITLE: &str = "Dukovo";
const HINT: &str = "Esc shows the messages";
/// How long after the boot began the startup screen gives way to the log,
/// if no program has taken the screen: a boot that stops shows why.
const QUIET_NS: u64 = 60_000_000_000;
/// Where the bar is when Spark starts, of 1000; the services' processes
/// fill it from there towards [`STARTED_MOST`].
pub const SPARK_STARTS: u32 = 500;
const STARTED_MOST: u32 = 950;

/// The kernel log's framebuffer console.
struct ScreenSink;

impl Sink for ScreenSink {
    fn write(&self, text: &str, style: Style) {
        if TAKEN_OVER.load(Ordering::Relaxed) {
            return;
        }
        if let Some(console) = SCREEN.lock().as_mut() {
            console.write(text, style);
            // An error shows the log behind the startup screen.
            if style == Style::Error && QUIET.swap(false, Ordering::Relaxed) {
                QUIET_UNTIL.store(0, Ordering::Relaxed);
                if !HANDED_OVER.load(Ordering::Relaxed) {
                    console.repaint();
                }
            }
            // Kept in the grid while a program holds the screen, or the
            // startup screen shows, for when it is shown.
            if !HANDED_OVER.load(Ordering::Relaxed) && !QUIET.load(Ordering::Relaxed) {
                console.flush();
            }
        }
    }
}

static SCREEN_SINK: ScreenSink = ScreenSink;

/// Starts the framebuffer console with the font named by the `font` flag,
/// or one chosen for the screen size.
pub fn init_framebuffer(framebuffer: Framebuffer, font: Option<&str>) {
    let auto = radix_fbcon::choose_font(framebuffer.width, framebuffer.height);
    let font_name = match font {
        Some(name) if radix_fbcon::builtin_font(name).is_some() => name,
        Some(name) => {
            warn!("Framebuffer: no built-in font `{name}`; using {auto}");
            auto
        }
        None => auto,
    };
    let Some(font) = radix_fbcon::builtin_font(font_name) else { return };
    let Some(storage) = STORAGE.take() else { return };
    let (width, height) = (framebuffer.width, framebuffer.height);
    match Console::new(framebuffer, font, storage) {
        Some(console) => {
            *SCREEN.lock_irq() = Some(console);
            radix_klog::add_sink(&SCREEN_SINK, true);
        }
        None => warn!("Framebuffer: cannot draw text on a {width}x{height} framebuffer"),
    }
}

/// Logs what the screen console looks like.
pub fn report() {
    let screen = SCREEN.lock_irq();
    let Some(console) = screen.as_ref() else {
        return;
    };
    let fb = console.framebuffer();
    let (font_width, font_height) = console.font_size();
    let (x, y) = console.origin();
    let (cols, rows) = (console.cols(), console.rows());
    let (address, width, height, pitch, bpp) =
        (fb.phys_addr, fb.width, fb.height, fb.pitch, fb.format.bytes_per_pixel);
    drop(screen);
    info!(
        "Framebuffer: {width}x{height}, {} bits per pixel, pitch {pitch}, at {address:#x}",
        u32::from(bpp) * 8
    );
    info!(
        "Framebuffer: console of {cols}x{rows} cells with font {font_width}x{font_height} at ({x}, {y})"
    );
}

/// Shows the startup screen in place of the log (`quiet`, ADR 0426), if
/// there is a framebuffer console, until a minute after the boot began.
pub fn quiet() {
    let mut screen = SCREEN.lock_irq();
    let Some(console) = screen.as_mut() else { return };
    QUIET.store(true, Ordering::Relaxed);
    QUIET_UNTIL.store(QUIET_NS, Ordering::Relaxed);
    console.draw_splash(TITLE, HINT, PROGRESS.load(Ordering::Relaxed));
}

/// Moves the startup screen's bar to `per_mille` of 1000, if it is
/// further than the bar is.
pub fn progress(per_mille: u32) {
    let before = PROGRESS.fetch_max(per_mille, Ordering::Relaxed);
    if before >= per_mille || !QUIET.load(Ordering::Relaxed) {
        return;
    }
    let mut screen = SCREEN.lock_irq();
    if let Some(console) = screen.as_mut()
        && QUIET.load(Ordering::Relaxed)
        && !HANDED_OVER.load(Ordering::Relaxed)
        && !TAKEN_OVER.load(Ordering::Relaxed)
    {
        console.draw_progress(PROGRESS.load(Ordering::Relaxed));
    }
}

/// A process was made: once Spark runs, each moves the startup screen's
/// bar an eighth of the way that is left.
pub fn started_process() {
    if !QUIET.load(Ordering::Relaxed) {
        return;
    }
    let now = PROGRESS.load(Ordering::Relaxed);
    if now >= SPARK_STARTS {
        progress(now + (STARTED_MOST.saturating_sub(now)).div_ceil(8));
    }
}

/// When the startup screen gives way to the log, if it shows.
pub fn quiet_until() -> Option<u64> {
    match QUIET_UNTIL.load(Ordering::Relaxed) {
        0 => None,
        at => Some(at),
    }
}

/// Shows the log in place of the startup screen, if it shows: for Esc, or
/// once [`quiet_until`] has passed.
pub fn reveal() {
    let mut screen = SCREEN.lock_irq();
    QUIET_UNTIL.store(0, Ordering::Relaxed);
    if QUIET.swap(false, Ordering::Relaxed)
        && let Some(console) = screen.as_mut()
        && !HANDED_OVER.load(Ordering::Relaxed)
        && !TAKEN_OVER.load(Ordering::Relaxed)
    {
        console.repaint();
    }
}

/// Whether there is a framebuffer console.
pub fn present() -> bool {
    SCREEN.lock_irq().is_some()
}

/// Stops log output to the screen and hands it to `draw`, for the panic
/// report, over whatever a program drew there.
pub fn take_over(draw: impl FnOnce(&mut Console)) {
    TAKEN_OVER.store(true, Ordering::Relaxed);
    if let Some(console) = SCREEN.lock_irq().as_mut() {
        // Over a program's drawing or the startup screen: the log first.
        let quiet = QUIET.swap(false, Ordering::Relaxed);
        if HANDED_OVER.load(Ordering::Relaxed) || quiet {
            console.repaint();
        }
        draw(console);
        console.flush();
    }
}

/// The screen's size and layout, as (width, height, pitch, format), if
/// there is a framebuffer console.
pub fn screen() -> Option<(usize, usize, usize, PixelFormat)> {
    let screen = SCREEN.lock_irq();
    let fb = screen.as_ref()?.framebuffer();
    Some((fb.width, fb.height, fb.pitch, fb.format))
}

/// Where the framebuffer starts in physical memory, if there is a
/// framebuffer console: addresses there are not given to devices.
pub fn framebuffer_start() -> Option<u64> {
    let screen = SCREEN.lock_irq();
    Some(screen.as_ref()?.framebuffer().phys_addr)
}

/// Gives the screen to the display layer's holder in front: the log stops
/// drawing on it, and the startup screen is done with, so that the log
/// shows when the front is the log's again.
pub fn hand_over() {
    HANDED_OVER.store(true, Ordering::Relaxed);
    QUIET.store(false, Ordering::Relaxed);
    QUIET_UNTIL.store(0, Ordering::Relaxed);
}

/// Takes the screen back from the display layer's holders, and draws the
/// log on it again.
pub fn give_back() {
    // With interrupts masked, as the log writes the screen from them.
    let mut screen = SCREEN.lock_irq();
    HANDED_OVER.store(false, Ordering::Relaxed);
    if let Some(console) = screen.as_mut()
        && !TAKEN_OVER.load(Ordering::Relaxed)
    {
        console.repaint();
    }
}

/// Hands the framebuffer's pixels to `draw`, for the display layer's
/// holder in front (`crate::display`), and returns what it returns.
/// `None`, with `draw` not run, once the panic report owns the screen,
/// while the log has it, and without a framebuffer console. `draw` runs
/// with the console's lock held and interrupts masked: it copies a row
/// and logs nothing.
pub fn with_pixels<R>(draw: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
    let mut screen = SCREEN.lock_irq();
    if TAKEN_OVER.load(Ordering::Relaxed) || !HANDED_OVER.load(Ordering::Relaxed) {
        return None;
    }
    Some(draw(screen.as_mut()?.pixels_mut()))
}

/// Changes one line of the screen in place, such as a restart countdown.
pub fn update_line(row_from_bottom: usize, text: &str, style: Style) {
    if HANDED_OVER.load(Ordering::Relaxed) && !TAKEN_OVER.load(Ordering::Relaxed) {
        return;
    }
    if let Some(console) = SCREEN.lock_irq().as_mut() {
        if QUIET.swap(false, Ordering::Relaxed) {
            console.repaint();
        }
        let row = console.rows().saturating_sub(1 + row_from_bottom);
        console.fill_row(row, Style::Plain);
        console.set_position(0, row);
        console.write(text, style);
        console.flush();
    }
}
