// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The framebuffer console: the kernel log on the screen, and the screen
//! the panic report takes over. The screen is also the display layer's
//! first card (`crate::display`, ADR 0705): while one of the layer's
//! holders is in front the log stays off it, and is drawn again when the
//! front is the log's.
//!
//! A virtual machine's graphics card has no framebuffer: it is sent its
//! pixels (ADR 0704). When such a card shows the log, the console is
//! made again for its screen ([`lay`]) with no framebuffer, and from
//! then on draws nothing itself: the card's thread asks what changed
//! ([`next_draw`]) and sends that. All else here is the same for both.
//!
//! With the `quiet` flag, as an installed system starts (ADR 0426), the
//! screen shows the startup screen instead: Dukovo's name and a bar that
//! fills as the kernel and then Spark's services start ([`progress`],
//! [`started_process`]). The log is kept, unseen, and shows again
//! ([`reveal`]) when Esc is pressed, when an error is logged, or when no
//! program has taken the screen a minute after the boot began; the serial
//! port and dmesg(1) get it all either way.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use radix_fbcon::{Console, Draw, FONTS, Font, Painter};
use radix_hal::{Framebuffer, PixelFormat};
use radix_klog::{Sink, Style, info, warn};
use radix_sync::{Once, SpinLock, TakeOnce};

/// Cells of grid storage: enough for 1920x1080 with the smallest font
/// (240 x 67, twice), or 4K with the font picked for it.
const STORAGE_CELLS: usize = 96 * 1024;

static STORAGE: TakeOnce<[u32; STORAGE_CELLS]> = TakeOnce::new([0; STORAGE_CELLS]);
/// The grid storage while no console has it: one could not be made for
/// the screen it was last asked for.
static SPARE: SpinLock<Option<&'static mut [u32]>> = SpinLock::new(None);
/// The built-in font the `font` flag names.
static FONT: Once<&'static str> = Once::new();
/// The built-in fonts as read, each from when a console first has it.
/// A font is large: it is read into its place here, and lent from it.
static FONTS_READ: [Once<Font<'static>>; FONTS.len()] = [const { Once::new() }; FONTS.len()];
/// Whether the log writes to the console: from the first one made.
static LOGGED_TO: AtomicBool = AtomicBool::new(false);
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

/// Starts the console on the firmware's framebuffer, if the boot loader
/// left one, with the font the `font` flag names or one chosen for the
/// screen's size. The flag is kept for a console made later ([`lay`]).
pub fn init(framebuffer: Option<Framebuffer>, font: Option<&str>) {
    if let Some(name) = font {
        match FONTS.iter().find(|(known, _)| *known == name) {
            Some(&(known, _)) => {
                let _ = FONT.set(known);
            }
            None => warn!("Console: no built-in font `{name}`; one is chosen for the screen"),
        }
    }
    let Some(framebuffer) = framebuffer else { return };
    let (width, height) = (framebuffer.width, framebuffer.height);
    let (Some(font), Some(storage)) = (font_for((width, height)), STORAGE.take()) else { return };
    match Console::new(framebuffer, *font, storage) {
        Some(console) => {
            *SCREEN.lock_irq() = Some(console);
            LOGGED_TO.store(true, Ordering::Release);
            radix_klog::add_sink(&SCREEN_SINK, true);
        }
        None => warn!("Framebuffer: cannot draw text on a {width}x{height} framebuffer"),
    }
}

/// The font for a screen of `size` pixels, wide and high: the `font`
/// flag's, or the largest that leaves the grid the console should have.
fn font_for((width, height): (usize, usize)) -> Option<&'static Font<'static>> {
    let chosen = radix_fbcon::choose_font(width, height);
    let name = FONT.get().copied().unwrap_or(chosen);
    let at = FONTS.iter().position(|(known, _)| *known == name)?;
    if FONTS_READ[at].get().is_none() {
        read_font(at);
    }
    FONTS_READ[at].get()
}

/// Reads built-in font `at` into its place. In a frame of its own, as
/// the font passes through it.
#[inline(never)]
fn read_font(at: usize) {
    if let Some(font) = radix_fbcon::builtin_font(FONTS[at].0) {
        // Another CPU that read it first read the same.
        let _ = FONTS_READ[at].set(font);
    }
}

/// Logs what the screen console looks like.
pub fn report() {
    let screen = SCREEN.lock_irq();
    let Some(console) = screen.as_ref() else {
        return;
    };
    let (font_width, font_height) = console.font_size();
    let (x, y) = console.origin();
    let (cols, rows) = (console.cols(), console.rows());
    let framebuffer = console.framebuffer().map(|fb| {
        (fb.phys_addr, fb.width, fb.height, fb.pitch, u32::from(fb.format.bytes_per_pixel) * 8)
    });
    drop(screen);
    // The boot tests find the grid on the screen by the second line.
    let of = match framebuffer {
        Some((address, width, height, pitch, bits)) => {
            info!(
                "Framebuffer: {width}x{height}, {bits} bits per pixel, pitch {pitch}, at \
                 {address:#x}"
            );
            "Framebuffer"
        }
        None => "Display",
    };
    info!(
        "{of}: console of {cols}x{rows} cells with font {font_width}x{font_height} at ({x}, {y})"
    );
}

/// Makes the console one for a screen of `width` by `height` pixels that
/// has no framebuffer, for the card that shows the log on such a screen
/// and is sent its pixels. A console that is one for that size already
/// stays as it is. Any other gives way to a new one, whose text starts
/// again from the log's records, and on which the startup screen shows
/// if it showed. The card draws what [`next_draw`] gives it with the
/// console's [`painter`].
///
/// Returns whether there is such a console now: no text fits a screen
/// too small, which is left with no console.
pub fn lay(width: usize, height: usize) -> bool {
    let size = (width, height);
    if sent_size() == Some(size) {
        return true;
    }
    let mut made = false;
    if LOGGED_TO.swap(true, Ordering::AcqRel) {
        // With the log locked, so that no line is written between the
        // new console and the records it starts from.
        radix_klog::replay(&SCREEN_SINK, || made = make_sent(size));
    } else {
        made = make_sent(size);
        radix_klog::add_sink(&SCREEN_SINK, true);
    }
    made
}

/// The size of the screen the console is for, if it is one with no
/// framebuffer.
fn sent_size() -> Option<(usize, usize)> {
    let screen = SCREEN.lock_irq();
    let console = screen.as_ref().filter(|console| console.framebuffer().is_none())?;
    Some(console.painter().screen())
}

/// Replaces the console with one of no framebuffer for a screen of
/// `size`, on the storage of the one there was, and returns whether it
/// could. It logs nothing: [`lay`] calls it with the log locked.
fn make_sent(size: (usize, usize)) -> bool {
    let mut screen = SCREEN.lock_irq();
    let Some(storage) = take_storage(&mut screen) else { return false };
    match put_sent(&mut screen, size, storage) {
        Ok(()) => true,
        Err(storage) => {
            *SPARE.lock_irq() = Some(storage);
            false
        }
    }
}

/// The grid storage: that of the console in `screen`, which ends, or
/// from where it waits while there is no console. A console is large,
/// so each step of [`make_sent`] has a frame of its own.
#[inline(never)]
fn take_storage(screen: &mut Option<Console>) -> Option<&'static mut [u32]> {
    if let Some(console) = screen.take() {
        return Some(console.into_storage());
    }
    if let Some(spare) = SPARE.lock_irq().take() {
        return Some(spare);
    }
    let never_used: &'static mut [u32] = STORAGE.take()?;
    Some(never_used)
}

/// Puts in `screen` a console of no framebuffer for a screen of `size`
/// pixels, on `storage`, with the startup screen on it if that shows.
///
/// # Errors
///
/// `storage` again, if no text fits the screen.
#[inline(never)]
fn put_sent(
    screen: &mut Option<Console>,
    size: (usize, usize),
    storage: &'static mut [u32],
) -> Result<(), &'static mut [u32]> {
    let Some(font) = font_for(size) else { return Err(storage) };
    let console = screen.insert(Console::without_framebuffer(size, font, storage)?);
    if QUIET.load(Ordering::Relaxed) {
        console.draw_splash(TITLE, HINT, PROGRESS.load(Ordering::Relaxed));
    }
    Ok(())
}

/// Puts what draws the console's cells as rows of pixels in `into`, or
/// `None` if there is no console.
pub fn painter(into: &mut Option<Painter>) {
    *into = SCREEN.lock_irq().as_ref().map(Console::painter);
}

/// What the card that is sent the console's pixels draws next, which
/// counts as drawn from then on: a part of the screen, and in `cells`
/// the cells of one that is text. `None` when nothing waits, while a
/// holder of the display layer has the screens, and for a console on a
/// framebuffer, which draws itself.
pub fn next_draw(cells: &mut [u32]) -> Option<Draw> {
    let mut screen = SCREEN.lock_irq();
    let taken = TAKEN_OVER.load(Ordering::Relaxed);
    if HANDED_OVER.load(Ordering::Relaxed) && !taken {
        return None;
    }
    // The startup screen hides the text, and the panic report is text.
    let text = taken || !QUIET.load(Ordering::Relaxed);
    screen.as_mut()?.next_draw(text, cells)
}

/// All of the console's screen is drawn again, as the log or the
/// startup screen: for the card that is sent its pixels, when it has a
/// new picture to draw them in.
pub fn redraw() {
    if let Some(console) = SCREEN.lock_irq().as_mut() {
        if QUIET.load(Ordering::Relaxed) && !TAKEN_OVER.load(Ordering::Relaxed) {
            console.draw_splash(TITLE, HINT, PROGRESS.load(Ordering::Relaxed));
        } else {
            console.repaint();
        }
    }
}

/// Shows the startup screen in place of the log (`quiet`, ADR 0426)
/// until a minute after the boot began: on the console there is, and on
/// one made while it shows.
pub fn quiet() {
    let mut screen = SCREEN.lock_irq();
    QUIET.store(true, Ordering::Relaxed);
    QUIET_UNTIL.store(QUIET_NS, Ordering::Relaxed);
    if let Some(console) = screen.as_mut() {
        console.draw_splash(TITLE, HINT, PROGRESS.load(Ordering::Relaxed));
    }
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

/// Whether there is a console on a screen.
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
    let fb = screen.as_ref()?.framebuffer()?;
    Some((fb.width, fb.height, fb.pitch, fb.format))
}

/// Where the framebuffer starts in physical memory, if there is a
/// framebuffer console: addresses there are not given to devices.
pub fn framebuffer_start() -> Option<u64> {
    let screen = SCREEN.lock_irq();
    Some(screen.as_ref()?.framebuffer()?.phys_addr)
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
/// while the log has it, and without a framebuffer. `draw` runs
/// with the console's lock held and interrupts masked: it copies a row
/// and logs nothing.
pub fn with_pixels<R>(draw: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
    let mut screen = SCREEN.lock_irq();
    if TAKEN_OVER.load(Ordering::Relaxed) || !HANDED_OVER.load(Ordering::Relaxed) {
        return None;
    }
    Some(draw(screen.as_mut()?.pixels_mut()?))
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
