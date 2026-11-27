// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens: the shown console drawn on every screen a monitor is
//! plugged into, through the display layer (radix-display(2)).
//!
//! consoled is one of the layer's holders, and the one that has
//! _/devices/display/attention_. Its first change takes the
//! front from the kernel's log and puts a picture on each screen's main
//! plane, the size of the screen's mode, which the card keeps: consoled
//! maps nothing and holds no copy of a screen. A draw sends the cells
//! that changed since the last one and no others, from the first changed
//! cell of each row to the last, as rectangles of one PUT; a draw of more
//! than [`PUT_BYTES`] of rows, as when a whole screen scrolls, goes in as
//! many. The layer tells consoled when it is no longer in front, and it
//! draws nothing until it is told it is again, when it draws everything:
//! a kept picture's pixels are promised only while its holder is in
//! front. When a screen comes, goes or changes its size, the consoles
//! are arranged anew and take the size of the new grid.
//!
//! Only the holder in front can pass the screens on, so a program that
//! draws them itself, a desktop, has them from consoled ([`Screen::lend`],
//! `crate::lend`). It gives them back, and the layer makes the consoles'
//! state again. Or it ends, as one that crashes or is started again
//! does: the layer then leaves its last picture on the screens, and
//! consoled draws nothing for [`BORROWER_WAIT`], so that the next
//! borrower takes the screens from that picture, with no console between
//! the two. Only when none has come does consoled show the consoles
//! again.
//!
//! Ctrl+Alt+Delete is the secure key (ADR 0708). The kernel sees it, and
//! raises the holder of the attention file over whoever has the
//! screens: consoled is told ([`Screen::take_key`]), is in front, and
//! alone reads the keys, until it says it is done ([`Screen::lower`]).
//! The borrower it was raised over has the screens back then.
//!
//! Every screen shows the same grid, the largest that fits them all,
//! centred, in the Spleen font the kernel's log used, chosen the same way
//! (radix-fbcon), so a screen looks the same when consoled takes it
//! over; the installer's screens ask for a larger font where it fits
//! them. A screen with room for the grid in a bigger font draws it in
//! that. The colours are those of the shown console's palette, and the
//! margins round the grid are its colour 0, the background, so that a
//! program that changes the palette colours the screen edge to edge.
//!
//! The text can be made one or two steps bigger (accessibility.toml(5),
//! ADR 0325): the steps are the built-in fonts and then the two larger ones
//! drawn at twice their size, each pixel two by two, so that a screen too
//! big for the largest font still has bigger text to give.

mod paint;

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use display::abi::{
    DisplayEvent, EVENT_ATTENTION, EVENT_BACK, EVENT_BORROWER_ENDED, EVENT_FRONT, EVENT_OVERFLOW,
    EVENT_SCREEN, MAY_BLANK,
};
use display::wire::{self, ChangeBytes, PutBytes};
use display::{Control, Holder, Unmade};
use radix_abi::input::INPUT_TIE;
use radix_fbcon::{Font, builtin_font, choose_font_for};
use sysapi::kernel;

use crate::access::Size;
use crate::term::{Cell, Palette, Terminal, default_palette};
use display::outputs::{self, Found, Output};
use paint::{Drawn, Look, Span};

/// The text sizes, smallest first: a built-in font, and how many times
/// its size it is drawn.
const LADDER: [(&str, usize); 5] =
    [("8x16", 1), ("12x24", 1), ("16x32", 1), ("12x24", 2), ("16x32", 2)];

/// The fewest columns and rows a bigger text size leaves: past them, it
/// is a step smaller.
const SMALLEST_GRID: (usize, usize) = (40, 12);

/// The grid of the consoles while no screen shows them.
const UNSEEN_GRID: (usize, usize) = (80, 25);

/// The most bytes of rows one PUT carries, unless a single row of cells
/// is more.
const PUT_BYTES: usize = 1024 * 1024;

/// How many times consoled asks about the screens as it starts, if they
/// change while it asks, as when a card's monitor is found just then.
const START_TRIES: usize = 4;

/// How long consoled leaves the last picture of a borrower that ended
/// on the screens, for another to take them from.
pub const BORROWER_WAIT: Duration = Duration::from_secs(5);

/// The layer's file, as the screens use it: its requests, and the events
/// to read from it.
pub trait Link: Control + Sized {
    /// Reads the events that wait into `into`, whole ones, without
    /// waiting for one. Returns the bytes read.
    ///
    /// # Errors
    ///
    /// `WouldBlock` when none waits.
    fn events(&self, into: &mut [u8]) -> io::Result<usize>;

    /// Opens the layer's file once more: another holder, with nothing of
    /// this one's, to lend the front to.
    ///
    /// # Errors
    ///
    /// If the file cannot be opened.
    fn another(&self) -> io::Result<Self>;

    /// The slot the description is in, which LEND names a borrower by.
    fn slot(&self) -> u32;
}

/// A description of one of the layer's files: consoled's own, of
/// _/devices/display/attention_, or one of _/devices/display/control_
/// that it opened to lend the front to.
#[derive(Debug)]
pub struct Device {
    file: File,
    /// The layer's file as a borrower has it, to open it once more.
    lends: PathBuf,
}

impl Device {
    /// Opens the layer's file at `path` with the open flags `flags`, and
    /// never for a program consoled starts. A borrower's is `lends`.
    fn open(path: &Path, flags: i32, lends: &Path) -> io::Result<Device> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | flags)
            .open(path)?;
        Ok(Device { file, lends: lends.to_path_buf() })
    }
}

impl AsFd for Device {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl Control for Device {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        kernel::control(&self.file, request, argument, buffer).map_err(kernel::io_error)
    }
}

impl Link for Device {
    fn events(&self, into: &mut [u8]) -> io::Result<usize> {
        (&self.file).read(into)
    }

    // The borrower chooses for itself whether its reads wait.
    fn another(&self) -> io::Result<Device> {
        Device::open(&self.lends, 0, &self.lends)
    }

    fn slot(&self) -> u32 {
        kernel::slot_of(&self.file)
    }
}

/// The pixels of a cell at step `step` of [`LADDER`].
fn cell_size(step: usize) -> (usize, usize) {
    let (name, scale) = LADDER[step.min(LADDER.len() - 1)];
    builtin_font(name).map_or((8 * scale, 16 * scale), |f| (f.width * scale, f.height * scale))
}

/// The columns and rows a screen of `screen` pixels has at step `step`.
fn cells(screen: (usize, usize), step: usize) -> (usize, usize) {
    let (width, height) = cell_size(step);
    (screen.0 / width, screen.1 / height)
}

/// The step of [`LADDER`] that text `size` is on a screen of `screen`
/// pixels whose normal size is step `normal`: `size`'s steps above it,
/// or fewer if that leaves less than [`SMALLEST_GRID`].
fn size_step(screen: (usize, usize), normal: usize, size: Size) -> usize {
    (normal..=(normal + size.steps()).min(LADDER.len() - 1))
        .rev()
        .find(|&step| {
            let (cols, rows) = cells(screen, step);
            cols >= SMALLEST_GRID.0 && rows >= SMALLEST_GRID.1
        })
        .unwrap_or(normal)
}

/// How one screen shows the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fit {
    /// The step of [`LADDER`] it is drawn at.
    step: usize,
    /// The pixel of its top-left cell.
    origin: (usize, usize),
}

/// The grid that screens of `screens` pixels all show at text `size`,
/// and how each shows it. `want` is the columns and rows the normal font
/// should leave at least, where a font does.
///
/// Each screen by itself would show the grid of the largest built-in
/// font that leaves `want`, as radix-fbcon chooses it, `size`'s steps
/// bigger. The grid is the smallest of those, each way. A screen then
/// draws it centred, in the biggest text that still shows all of it,
/// which for the screen that set the grid is its own.
fn arrange(
    screens: &[(usize, usize)],
    want: (usize, usize),
    size: Size,
) -> ((usize, usize), Vec<Fit>) {
    let own = |screen: (usize, usize)| {
        let font = choose_font_for(screen.0, screen.1, want);
        let normal = LADDER.iter().position(|&(name, scale)| name == font && scale == 1);
        size_step(screen, normal.unwrap_or(0), size)
    };
    let steps: Vec<usize> = screens.iter().map(|&screen| own(screen)).collect();
    let grids = screens.iter().zip(&steps).map(|(&screen, &step)| cells(screen, step));
    let Some(grid) = grids.reduce(|a, b| (a.0.min(b.0), a.1.min(b.1))) else {
        return (UNSEEN_GRID, Vec::new());
    };
    let fit = |(&screen, &own): (&(usize, usize), &usize)| {
        let shows_all = |&step: &usize| {
            let (cols, rows) = cells(screen, step);
            cols >= grid.0 && rows >= grid.1
        };
        let step = (own..LADDER.len()).rev().find(shows_all).unwrap_or(own);
        let (width, height) = cell_size(step);
        let origin = (
            screen.0.saturating_sub(grid.0 * width) / 2,
            screen.1.saturating_sub(grid.1 * height) / 2,
        );
        Fit { step, origin }
    };
    (grid, screens.iter().zip(&steps).map(fit).collect())
}

/// One screen the consoles show on.
struct Surface {
    output: Output,
    /// The picture its card keeps for it, on its main plane.
    picture: u32,
    /// The value that holds its mode.
    mode: u32,
    fit: Fit,
}

impl Surface {
    fn size(&self) -> (usize, usize) {
        (self.output.mode.width as usize, self.output.mode.height as usize)
    }

    /// The font it is drawn in, and how many times its size.
    fn font(&self) -> Option<(Font<'static>, usize)> {
        let (name, scale) = LADDER[self.fit.step];
        Some((builtin_font(name)?, scale))
    }
}

/// The shown console on the screens.
pub struct Screen<L: Link = Device> {
    holder: Holder<L>,
    /// The columns and rows the normal font should leave at least.
    want: (usize, usize),
    /// The text size asked for.
    size: Size,
    surfaces: Vec<Surface>,
    cols: usize,
    rows: usize,
    /// Whether consoled is in front, and draws.
    front: bool,
    /// Whether the layer shows the consoles' state, as far as consoled
    /// knows. It does not when the front has come back, until consoled
    /// has asked for its state again: a borrower that ended left its
    /// own picture on the screens.
    shows: bool,
    /// Whether a borrower has the screens.
    lent: bool,
    /// Whether the secure key raised consoled, which has not said it is
    /// done.
    raised: bool,
    /// Whether it was raised over a borrower, which has the screens
    /// back when consoled is done.
    over: bool,
    /// Set when the secure key is read of, until it is acted on
    /// ([`Screen::take_key`]).
    key: bool,
    /// Set when a loan's description never reached its borrower: the
    /// borrower's end, when the layer tells of it, leaves no picture to
    /// wait behind.
    unsent: bool,
    /// Set when a borrower's end is read of, until the wait for another
    /// is timed ([`Screen::tick`]).
    ended: bool,
    /// When consoled draws again, while it leaves the screens to the
    /// last picture of a borrower that ended.
    waits_until: Option<Instant>,
    /// Whether the screens changed and are still to be arranged anew.
    stale: bool,
    /// The palette the screens are drawn in: the shown console's.
    palette: Palette,
    /// What each cell shows now, to draw only what changed.
    drawn: Drawn,
    drawn_reverse: bool,
    /// Whether the margins round the grid are painted in the palette's
    /// background.
    margins: bool,
    /// Whether the last draw failed, which is logged once.
    failing: bool,
    /// The PUT being filled, kept for its memory.
    put: PutBytes,
}

impl Screen<Device> {
    /// Opens the display layer's file at `path` and takes the screens
    /// from the kernel's log, with the largest font that gives a grid of
    /// `want`'s columns and rows at least, or the smallest. A program
    /// the screens are lent to is given the layer's file at `lends`.
    ///
    /// If a program has the screens, lent by a consoled that has ended,
    /// the consoles stand under it (ADR 0716) and show when it gives the
    /// screens back or ends.
    ///
    /// # Errors
    ///
    /// If the file cannot be opened, there is no graphics card, or the
    /// layer does not let the consoles show or stand under the program
    /// that is in front.
    pub fn open(path: &str, lends: &str, want: (usize, usize)) -> io::Result<Screen> {
        let device = Device::open(Path::new(path), libc::O_NONBLOCK, Path::new(lends))?;
        Screen::on(device, want)
    }

    /// The file to wait on for the layer's events.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.holder.file().as_fd()
    }

    /// Ties `input`, a description of an input file, to the consoles'
    /// holder (radix-input(2)): it reads keys only while the consoles
    /// are in front, and alone while the secure key raised them.
    ///
    /// # Errors
    ///
    /// Why the kernel did not tie it.
    pub fn tie(&self, input: &File) -> io::Result<()> {
        let slot = u64::from(self.holder.file().slot());
        kernel::control(input, INPUT_TIE, slot, &mut []).map(|_| ()).map_err(kernel::io_error)
    }
}

impl<L: Link> Screen<L> {
    /// The consoles on the screens of the layer `link` reaches.
    fn on(link: L, want: (usize, usize)) -> io::Result<Screen<L>> {
        let mut screen = Screen {
            holder: Holder::new(link),
            want,
            size: Size::Normal,
            surfaces: Vec::new(),
            cols: UNSEEN_GRID.0,
            rows: UNSEEN_GRID.1,
            front: false,
            shows: false,
            lent: false,
            raised: false,
            over: false,
            key: false,
            unsent: false,
            ended: false,
            waits_until: None,
            stale: false,
            palette: default_palette(),
            drawn: Drawn::new(UNSEEN_GRID.0, UNSEEN_GRID.1),
            drawn_reverse: false,
            margins: false,
            failing: false,
            put: PutBytes::new(),
        };
        let mut last = io::Error::other("no graphics card");
        for _ in 0..START_TRIES {
            match screen.start() {
                Ok(true) => return Ok(screen),
                Ok(false) => break,
                // Another holder is in front: a program the consoled
                // before this one lent the screens to. This one stands
                // under it, and has the screens when it is done.
                Err(err) if err.raw_os_error() == Some(libc::EBUSY) => {
                    match screen.holder.stand_under() {
                        Ok(()) => {
                            (screen.lent, screen.stale) = (true, true);
                            return Ok(screen);
                        }
                        // It ended meanwhile: the screens are there to
                        // take.
                        Err(under) if under.raw_os_error() == Some(libc::EBUSY) => last = err,
                        Err(under) => return Err(under),
                    }
                }
                Err(err) => last = err,
            }
        }
        Err(last)
    }

    /// Asks the layer what it has and shows the consoles on its screens.
    /// Returns whether it has a graphics card at all.
    fn start(&mut self) -> io::Result<bool> {
        let found = Found::read(&self.holder)?;
        if !found.has_card() {
            return Ok(false);
        }
        self.show_on(&found).map(|()| true)
    }

    /// Columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// What the consoles' geometry log lines say, one for each screen.
    /// The first screen's is in the words of the kernel's own, which the
    /// boot tests read the screen by: the grid, the font, `doubled` if
    /// it is drawn at twice its size, and the grid's corner.
    pub fn describe(&self) -> Vec<String> {
        if self.surfaces.is_empty() {
            return vec!["no screen is plugged in: the consoles show when one is".into()];
        }
        let line = |(at, surface): (usize, &Surface)| {
            let (font, scale) = LADDER[surface.fit.step];
            let doubled = if scale > 1 { " doubled" } else { "" };
            let line = format!(
                "console of {}x{} cells with font {font}{doubled} at ({}, {})",
                self.cols, self.rows, surface.fit.origin.0, surface.fit.origin.1
            );
            if at == 0 { line } else { format!("screen {}: {line}", at + 1) }
        };
        self.surfaces.iter().enumerate().map(line).collect()
    }

    /// Lets go of the pictures and mode values in `made`.
    fn let_go(&self, made: &[(u32, u32)]) {
        for &(picture, mode) in made {
            // One that is not there, as after a failure half way, is
            // nothing to let go of.
            let _ = self.holder.drop_object(picture);
            let _ = self.holder.drop_object(mode);
        }
    }

    /// Makes a picture the size of each output's mode, and a value of
    /// the mode, as (picture, mode value).
    fn make(&self, outputs: &[Output]) -> io::Result<Vec<(u32, u32)>> {
        let mut made = Vec::with_capacity(outputs.len());
        for output in outputs {
            let both = self
                .holder
                .keep_picture(output.card, output.mode.width, output.mode.height)
                .and_then(|picture| Ok((picture, self.holder.make_mode(&output.mode)?)));
            match both {
                Ok(pair) => made.push(pair),
                Err(err) => {
                    self.let_go(&made);
                    return Err(err);
                }
            }
        }
        Ok(made)
    }

    /// Shows the consoles on the screens `found` has monitors on, and on
    /// no other. With none, and nothing shown before, nothing is asked
    /// of the layer: the kernel's log keeps the screens.
    fn show_on(&mut self, found: &Found) -> io::Result<()> {
        let outputs = outputs::outputs(found);
        let same = outputs.iter().eq(self.surfaces.iter().map(|surface| &surface.output));
        if same && ((self.front && self.shows) || outputs.is_empty()) {
            return Ok(());
        }
        let made = self.make(&outputs)?;
        let mut change = outputs::change(found, &outputs, &made);
        if let Err(unmade) = self.holder.change(&mut change) {
            self.let_go(&made);
            return Err(match unmade {
                Unmade::Failed(err) => err,
                refused @ Unmade::Refused { .. } => {
                    io::Error::other(format!("the display layer's check: {refused}"))
                }
            });
        }
        // What showed before shows no more.
        let before: Vec<_> = self.surfaces.drain(..).map(|s| (s.picture, s.mode)).collect();
        self.let_go(&before);
        let fit = Fit { step: 0, origin: (0, 0) };
        let surface = |(&output, &(picture, mode))| Surface { output, picture, mode, fit };
        self.surfaces = outputs.iter().zip(&made).map(surface).collect();
        (self.front, self.shows) = (true, true);
        self.arrange();
        Ok(())
    }

    /// Fits the grid to the screens there are and the text size asked
    /// for; everything is then drawn again. Returns whether the grid, or
    /// how a screen shows it, changed.
    fn arrange(&mut self) -> bool {
        let sizes: Vec<_> = self.surfaces.iter().map(Surface::size).collect();
        let (grid, fits) = arrange(&sizes, self.want, self.size);
        let same = grid == (self.cols, self.rows)
            && fits.iter().eq(self.surfaces.iter().map(|surface| &surface.fit));
        for (surface, fit) in self.surfaces.iter_mut().zip(fits) {
            surface.fit = fit;
        }
        (self.cols, self.rows) = grid;
        self.drawn = Drawn::new(grid.0, grid.1);
        self.margins = false;
        !same
    }

    /// Makes the text `size`, or the biggest below it that still leaves
    /// [`SMALLEST_GRID`]; everything is drawn again. Returns whether the
    /// grid, or how a screen shows it, changed.
    pub fn set_size(&mut self, size: Size) -> bool {
        self.size = size;
        self.arrange()
    }

    /// Forgets what is on the screens, so the next draw draws every
    /// cell: for another console.
    pub fn invalidate(&mut self) {
        self.drawn.forget();
    }

    /// Forgets the cells and the margins: for another palette, or after
    /// another holder drew on the screens.
    fn forget(&mut self) {
        self.drawn.forget();
        self.margins = false;
    }

    /// Takes in one of the layer's events.
    fn event(&mut self, event: &DisplayEvent) {
        match event.kind {
            EVENT_FRONT => {
                self.lent = false;
                if !self.front {
                    // Whoever had the front drew on the screens, and may
                    // have left its own picture on them.
                    (self.front, self.shows) = (true, false);
                    self.forget();
                }
            }
            // Only a borrower is ever in front of consoled: as when it
            // is done with the secure key over one.
            EVENT_BACK => (self.front, self.lent) = (false, true),
            // Its last picture stays for the next borrower, unless it
            // never had the screens to draw one, or the raised consoles
            // are what shows.
            EVENT_BORROWER_ENDED => {
                let drew = !core::mem::take(&mut self.unsent);
                self.ended = drew && !self.raised;
                (self.lent, self.over) = (false, false);
            }
            // The count says whether the front passed to consoled for
            // the key, and so passes back. A borrower's last picture is
            // no longer waited behind: the person asked for consoled.
            EVENT_ATTENTION => {
                (self.raised, self.key, self.over) = (true, true, event.count != 0);
                (self.ended, self.waits_until) = (false, None);
            }
            EVENT_SCREEN => self.stale = true,
            // Some were lost, and one may have been any of the above:
            // the screens are asked about again, and the next draw
            // finds whether consoled is in front. If the secure key was
            // among them, nothing shows that it was: consoled says it
            // is done, so that its hold does not last.
            EVENT_OVERFLOW => {
                self.stale = true;
                (self.front, self.shows, self.lent) = (true, false, false);
                (self.raised, self.over) = (true, false);
                self.forget();
            }
            _ => {}
        }
    }

    /// Whether the secure key was pressed since this was last asked.
    pub fn take_key(&mut self) -> bool {
        core::mem::take(&mut self.key)
    }

    /// Whether the secure key raised consoled, which has not said it is
    /// done ([`Screen::lower`]): it alone reads the keys.
    pub fn raised(&self) -> bool {
        self.raised
    }

    /// Whether consoled is raised over a borrower, which has the
    /// screens back when consoled is done.
    pub fn over(&self) -> bool {
        self.raised && self.over
    }

    /// consoled is done with the secure key: every reader reads the keys
    /// again, and the borrower it was raised over has the screens back.
    pub fn lower(&mut self) {
        if !core::mem::take(&mut self.raised) {
            return;
        }
        self.over = false;
        match self.holder.attention_end() {
            // Not raised, as the layer has it: after events were lost.
            Err(err) if err.raw_os_error() == Some(libc::EBUSY) => {}
            Err(err) => eprintln!("consoled: cannot end the secure key's hold: {err}"),
            // What the layer said of the front is taken in at once: a
            // key typed from here is the borrower's.
            Ok(()) => self.read(),
        }
    }

    /// Lends the front to a new holder of the layer's file, and returns
    /// its description for the borrower to draw the screens with. From
    /// here consoled draws nothing until the layer says it is in front
    /// again. The description is to reach the borrower and be closed
    /// here, so that the layer knows when the borrower ends.
    ///
    /// # Errors
    ///
    /// EBUSY while consoled is not in front, as while another borrower
    /// has the screens or no screen shows the consoles, and while the
    /// secure key has it raised; and why the layer's file could not be
    /// opened or the front lent.
    pub fn lend(&mut self) -> io::Result<L> {
        // Asked before a description is opened for a loan that cannot
        // be made.
        self.read();
        if !self.front || self.raised {
            return Err(io::Error::from_raw_os_error(libc::EBUSY));
        }
        let borrower = self.holder.file().another()?;
        self.lend_to(&borrower)?;
        Ok(borrower)
    }

    /// Lends the front to `borrower`, a holder consoled lent to before
    /// and kept a description of: what it last showed shows again at
    /// once.
    ///
    /// # Errors
    ///
    /// As [`Screen::lend`].
    pub fn lend_to(&mut self, borrower: &L) -> io::Result<()> {
        // What the layer said before the loan is taken in first: an
        // event read after it is the loan's.
        self.read();
        // Nobody passes the front while the secure key has it raised.
        if !self.front || self.raised {
            return Err(io::Error::from_raw_os_error(libc::EBUSY));
        }
        self.holder.lend(borrower.slot())?;
        (self.front, self.lent) = (false, true);
        (self.ended, self.waits_until) = (false, None);
        Ok(())
    }

    /// The description of the last loan never reached its borrower and
    /// is closed: there will be no picture of the borrower's to leave on
    /// the screens.
    pub fn unlent(&mut self) {
        self.unsent = true;
    }

    /// Whether the screens are away: a borrower has them, or its last
    /// picture is still on them. A key typed then is not for a console
    /// that nobody sees.
    pub fn away(&self) -> bool {
        self.lent || self.waits()
    }

    /// Whether consoled leaves the screens to the last picture of a
    /// borrower that ended.
    fn waits(&self) -> bool {
        self.ended || self.waits_until.is_some()
    }

    /// When [`Screen::tick`] is next needed.
    pub fn deadline(&self) -> Option<Instant> {
        self.waits_until
    }

    /// Keeps the time of the wait for another borrower: `now` starts it
    /// if a borrower's end was read of, and ends it once
    /// [`BORROWER_WAIT`] has passed with no other. consoled then draws
    /// again, on screens arranged anew if they changed in the wait.
    /// Returns whether the grid, or how a screen shows it, changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if core::mem::take(&mut self.ended) {
            self.waits_until = Some(now + BORROWER_WAIT);
        }
        if self.waits_until.is_none_or(|until| now < until) {
            return false;
        }
        self.waits_until = None;
        self.settle()
    }

    /// Has the layer make the consoles' state again, where what shows
    /// differs from it: the pictures, and the modes if a borrower set
    /// others. Returns whether the screens show the consoles' pictures
    /// now.
    fn show_again(&mut self) -> bool {
        let mut nothing = ChangeBytes::new(MAY_BLANK, 0);
        match self.holder.change(&mut nothing) {
            Ok(()) => self.shows = true,
            Err(Unmade::Failed(err)) => self.not_sent(&err),
            // As when a screen went meanwhile: its event arranges the
            // screens anew, with a state that passes.
            Err(Unmade::Refused { .. }) => self.stale = true,
        }
        self.shows
    }

    /// Reads the layer's events and acts on them. Returns whether the
    /// grid, or how a screen shows it, changed: the consoles then take
    /// the new size.
    pub fn events(&mut self) -> bool {
        self.read();
        self.settle()
    }

    /// Reads the layer's events and takes each in.
    fn read(&mut self) {
        let mut bytes = [0u8; 16 * DisplayEvent::SIZE];
        loop {
            match self.holder.file().events(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    for event in wire::events(&bytes[..n]) {
                        self.event(&event);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    eprintln!("consoled: cannot read the display layer's events: {err}");
                    break;
                }
            }
        }
    }

    /// Arranges the screens anew if they changed. Returns whether the
    /// grid, or how a screen shows it, changed.
    fn settle(&mut self) -> bool {
        // Behind another holder, a change would be refused, and one made
        // while a borrower's last picture is left on the screens would
        // take it off them: the screens are arranged when consoled
        // draws again.
        if !self.stale || self.waits() || self.lent || !(self.front || self.surfaces.is_empty()) {
            return false;
        }
        let before = (self.cols, self.rows, self.fits());
        match Found::read(&self.holder).and_then(|found| self.show_on(&found)) {
            Ok(()) => self.stale = false,
            // As when a screen went between the listing and the change:
            // its event is still to come, and arranges them.
            Err(err) => eprintln!("consoled: the screens are not arranged anew yet: {err}"),
        }
        before != (self.cols, self.rows, self.fits())
    }

    fn fits(&self) -> Vec<Fit> {
        self.surfaces.iter().map(|surface| surface.fit).collect()
    }

    /// Draws what changed of `term` since the last draw, with `bottom`'s
    /// cells over its last row if there are any: a notification
    /// (`notices`). Nothing is drawn while consoled is not in front, or
    /// while it leaves the screens to the last picture of a borrower
    /// that ended. A draw the layer does not take is logged and drawn
    /// again, all of it, at the next: the consoles go on without the
    /// screens, as their readers of speech and braille do.
    pub fn draw(&mut self, term: &Terminal, bottom: Option<&[Cell]>) {
        if !self.front || self.waits() || self.surfaces.is_empty() {
            return;
        }
        // A picture that does not show takes no rows.
        if !self.shows && !self.show_again() {
            return;
        }
        // Another console's colours, or the program changed them: rare,
        // so everything is painted again.
        if term.palette() != &self.palette {
            self.palette = *term.palette();
            self.forget();
        }
        let reverse = term.modes().reverse_screen;
        if reverse != self.drawn_reverse {
            self.drawn.forget();
            self.drawn_reverse = reverse;
        }
        let last = self.rows.saturating_sub(1);
        let shown = |x: usize, y: usize| match bottom {
            Some(cells) if y == last => cells.get(x).copied().unwrap_or(Cell::BLANK),
            _ if x < term.cols() && y < term.rows() => term.shown(x, y),
            _ => Cell::BLANK,
        };
        // No cursor under a notification.
        let cursor = term.cursor().filter(|&(_, cy)| bottom.is_none() || cy != last);
        let spans = self.drawn.changed(shown, cursor);
        if spans.is_empty() && self.margins {
            return;
        }
        let look = |x: usize, y: usize| {
            Look::of(shown(x, y), &self.palette, reverse, cursor == Some((x, y)))
        };
        let mut put = core::mem::take(&mut self.put);
        let sent = self.surfaces.iter().try_for_each(|surface| {
            put.clear();
            if !self.margins {
                self.send_margins(surface, &mut put)?;
            }
            self.send_cells(surface, &spans, &look, &mut put)
        });
        put.clear();
        self.put = put;
        match sent {
            Ok(()) => self.margins = true,
            Err(err) => self.not_sent(&err),
        }
        if self.failing && self.margins {
            self.failing = false;
            eprintln!("consoled: the screens are drawn on again");
        }
    }

    /// Takes in that the layer did not take a draw, because of `err`.
    fn not_sent(&mut self, err: &io::Error) {
        // Part of it may show: all of it is drawn at the next draw.
        self.forget();
        if err.raw_os_error() == Some(libc::EBUSY) {
            // Not in front after all, with the event still to read:
            // the next draw is when consoled is. Only a borrower is
            // ever in front of it.
            (self.front, self.lent) = (false, true);
        } else if !self.failing {
            self.failing = true;
            eprintln!("consoled: cannot draw on the screens: {err}");
        }
    }

    /// Adds the margins round the grid on `surface` to `put`, in the
    /// palette's background, and sends it whenever its table is full.
    fn send_margins(&self, surface: &Surface, put: &mut PutBytes) -> io::Result<()> {
        let Some((font, scale)) = surface.font() else { return Ok(()) };
        let paper = paint::pixel(self.palette[0]);
        let grid = (self.cols * font.width * scale, self.rows * font.height * scale);
        for (x, y, wide, tall) in paint::margins(surface.size(), surface.fit.origin, grid) {
            let (mut top, mut left) = (y, tall);
            while left > 0 {
                let rest = put.fill(x as u16, top as u16, wide as u16, left as u16, paper);
                let rest = usize::from(rest);
                if rest == left && put.is_empty() {
                    // More than a PUT can say, which no screen is.
                    break;
                }
                (top, left) = (top + left - rest, rest);
                if rest > 0 {
                    self.holder.put(surface.picture, put)?;
                    put.clear();
                }
            }
        }
        Ok(())
    }

    /// Adds the cells of `spans` on `surface` to `put`, each cell as
    /// `look` says, and sends it: before a row that would make it more
    /// than [`PUT_BYTES`], and when all are in.
    fn send_cells(
        &self,
        surface: &Surface,
        spans: &[Span],
        look: &impl Fn(usize, usize) -> Look,
        put: &mut PutBytes,
    ) -> io::Result<()> {
        let Some((font, scale)) = surface.font() else { return Ok(()) };
        let (width, height) = (font.width * scale, font.height * scale);
        let origin = surface.fit.origin;
        let mut looks = Vec::new();
        for span in spans {
            looks.clear();
            looks.extend((span.from..=span.to).map(|x| look(x, span.y)));
            let wide = looks.len() * width;
            let over = put.row_bytes() + wide * height * 4 > PUT_BYTES;
            if put.is_full() || (!put.is_empty() && over) {
                self.holder.put(surface.picture, put)?;
                put.clear();
            }
            let (x, y) = (origin.0 + span.from * width, origin.1 + span.y * height);
            if let Some(rows) = put.rect(x as u16, y as u16, wide as u16, height as u16) {
                paint::paint(&font, scale, &looks, rows);
            }
        }
        if !put.is_empty() {
            self.holder.put(surface.picture, put)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
