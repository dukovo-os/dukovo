// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens of a boot test's machine, each on its own: for the tests
//! of the display layer (ADR 0700), whose machines have a virtio
//! graphics card of several screens beside, or in place of, the screen
//! the other tests look at.
//!
//! A case gives the card the id `gpu0`, and for each screen whose
//! monitor it plugs in, resizes or pulls out, a remote viewer's socket:
//! `-vnc` with [`SCREEN_VIEW`]'s word for the screen. The script asks a
//! size of that socket as a viewer does ([`viewer`]), which is how a
//! monitor comes to a screen of the card, changes size and goes
//! ([`Vm::plug_monitor`]). A picture of one screen is the machine
//! protocol's `screendump` of that head of the card
//! ([`Vm::screenshot_of`]).
//!
//! The guest's side is `display-test` (userspace/programs/display-test),
//! which draws test patterns and says in the log what each screen
//! shows. A [`Watch`] reads those lines up to each step of the program
//! and compares a picture of every screen with what they say, pixel
//! for pixel. The tests of the desktop watch the same way
//! ([`Watch::of_desktop`]): their program is `window-test`
//! (userspace/programs/window-test), a window on the desktop, which says
//! what the screens show around it and in it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use display_pattern::pattern;
use display_pattern::said::{self, Rect, Said};

use super::{Case, QEMU_EXIT_RESET, Qmp, Vm, boot_report, screen_shows, viewer};
use crate::qemu;
use crate::screen::{self, Image};

/// A viewer of screen 0, 1, 2 or 3 of the virtio graphics card `gpu0`,
/// as `-vnc`'s argument: a Unix socket `<case>.screen<n>` in the test's
/// directory that QEMU listens on ([`view_socket`]).
pub(super) const SCREEN_VIEW: [&str; 4] =
    ["<screen view 0>", "<screen view 1>", "<screen view 2>", "<screen view 3>"];

/// The id the cases give their virtio graphics card.
const CARD: &str = "gpu0";

/// How long a screen has to show what the log says it does: the log
/// comes by the serial port, ahead of the frame.
const PATIENCE: Duration = Duration::from_secs(20);

/// The socket of case `name`'s viewer of screen `screen` in the test's
/// directory `dir`.
fn view_socket(dir: &Path, name: &str, screen: usize) -> PathBuf {
    dir.join(format!("{name}.screen{screen}"))
}

/// What `arg` stands for if it is one of [`SCREEN_VIEW`], for case
/// `name` in the test's directory `dir`.
pub(super) fn view_argument(arg: &str, dir: &Path, name: &str) -> Option<String> {
    let screen = SCREEN_VIEW.iter().position(|view| *view == arg)?;
    let socket = qemu::option_value(&view_socket(dir, name, screen));
    Some(format!("unix:{socket},display={CARD},head={screen},id=view{screen}"))
}

/// What follows each of `starts` on its line of the log so far, which
/// must have them in that order. The cards are found in the PCI scan,
/// which the boot report has read past by the time a case looks.
pub(super) fn found(vm: &Vm, starts: &[&str]) -> Result<Vec<String>, String> {
    let log = vm.log();
    let mut from = 0;
    let mut rests = Vec::new();
    for start in starts {
        let Some(at) = log[from..].find(start) else {
            return Err(format!("the kernel did not log \"{start}\" where it was looked for"));
        };
        from += at + start.len();
        rests.push(log[from..].lines().next().unwrap_or_default().trim().to_string());
    }
    Ok(rests)
}

/// Which screen of the machine the program's `name` is: `None` for the
/// first screen, the firmware's, and the head of the virtio card for
/// one of its.
fn head_of(name: &str) -> Result<Option<usize>, String> {
    let unknown = || format!("the test knows no screen named {name}");
    match name.split_once('/').ok_or_else(unknown)? {
        ("firmware0", "0") => Ok(None),
        ("virtio0", head) => head.parse().map(Some).map_err(|_| unknown()),
        _ => Err(unknown()),
    }
}

impl Vm {
    /// Plugs a monitor of `size` into screen `screen` of the virtio
    /// card, or gives the monitor that is there that size, as resizing
    /// the machine's window does.
    pub fn plug_monitor(
        &mut self,
        case: &Case,
        screen: usize,
        size: (u16, u16),
    ) -> Result<(), String> {
        viewer::ask_size(&view_socket(self.dir(), case.name, screen), size.0, size.1)
    }

    /// Pulls the monitor out of screen `screen` of the virtio card.
    pub fn unplug_monitor(&mut self, case: &Case, screen: usize) -> Result<(), String> {
        self.plug_monitor(case, screen, (0, 0))
    }

    /// Takes a picture of one screen: the machine's first for `None`,
    /// as [`Vm::screenshot`], and head `head` of the virtio card
    /// otherwise.
    pub fn screenshot_of(&mut self, head: Option<usize>) -> Result<Image, String> {
        let Some(head) = head else { return self.screenshot() };
        let socket = self.qmp.clone().ok_or("no QMP socket")?;
        let _ = std::fs::remove_file(&self.screenshot);
        Qmp::connect(&socket)?.execute(&format!(
            r#"{{"execute":"screendump","arguments":{{"filename":{},"device":"{CARD}","head":{head}}}}}"#,
            crate::bench::json_string(&self.screenshot.to_string_lossy())
        ))?;
        let bytes = std::fs::read(&self.screenshot)
            .map_err(|err| format!("reading {}: {err}", self.screenshot.display()))?;
        Image::parse_ppm(&bytes)
    }

    /// Waits until the consoles show each of `needles` on the screen
    /// consoled counts as `number`, from 1, which is the machine's
    /// screen `head` ([`Vm::screenshot_of`]): for up to 15 seconds, as
    /// [`Vm::expect_screen`] waits for the first screen.
    pub fn expect_console_on(
        &mut self,
        number: usize,
        head: Option<usize>,
        needles: &[&str],
    ) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            let geometry = screen::Geometry::of_screen(&self.log(), number)
                .ok_or_else(|| format!("consoled did not say where screen {number}'s console is"));
            let shows = geometry.and_then(|geometry| {
                let text = screen::read_text(&self.screenshot_of(head)?, geometry)?;
                screen_shows(&text, needles)
            });
            match shows {
                Ok(()) => return Ok(()),
                Err(err) if Instant::now() >= until => {
                    return Err(format!("screen {number}: {err}"));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }
}

/// What one screen should show.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// Nothing.
    Dark,
    /// A picture: its width and its pixels, row after row.
    Picture(usize, Vec<[u8; 3]>),
}

/// Why `image` is not what is `expected`, or `None` if it is.
fn differs(expected: &Expected, image: &Image) -> Option<String> {
    let pixels = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y)));
    let Expected::Picture(width, wanted) = expected else {
        // A screen that shows nothing is black, but for the few words
        // the host may write on it to say so.
        let lit = pixels.filter(|&(x, y)| image.pixel(x, y) != [0; 3]).count();
        let dark = lit * 10 < image.width * image.height;
        return (!dark).then(|| format!("{lit} pixels are lit of a screen that shows nothing"));
    };
    let height = wanted.len() / width.max(&1);
    if (image.width, image.height) != (*width, height) {
        return Some(format!("it is {}x{}, not {width}x{height}", image.width, image.height));
    }
    let mut wrong = pixels.filter(|&(x, y)| image.pixel(x, y) != wanted[y * width + x]);
    let (x, y) = wrong.next()?;
    Some(format!(
        "{} of its {} pixels are not as drawn, the first at ({x}, {y}): {:?}, not {:?}",
        wrong.count() + 1,
        wanted.len(),
        image.pixel(x, y),
        wanted[y * width + x]
    ))
}

/// What every screen should show, by what `display-test` has said, or
/// `window-test`.
#[derive(Debug)]
pub(super) struct Watch {
    /// How each line of the program starts.
    start: &'static str,
    /// How far the log has been read.
    read: usize,
    /// Each screen the program has named.
    screens: BTreeMap<String, Expected>,
}

impl Default for Watch {
    /// A watch of what `display-test` says.
    fn default() -> Watch {
        Watch { start: said::START, read: 0, screens: BTreeMap::new() }
    }
}

impl Watch {
    /// A watch of the desktop's screens, by what `window-test` says.
    pub(super) fn of_desktop() -> Watch {
        Watch { start: said::WINDOW_START, ..Watch::default() }
    }

    /// The program's name, as its lines have it.
    fn program(&self) -> &'static str {
        self.start.trim_end_matches(": ")
    }

    /// Colours `rect` of `screen`'s picture, each pixel as `colour`
    /// gives it for its column and row in the rectangle.
    fn paint(
        &mut self,
        screen: &str,
        rect: &Rect,
        colour: impl Fn(u32, u32) -> [u8; 3],
    ) -> Result<(), String> {
        let Some(Expected::Picture(width, pixels)) = self.screens.get_mut(screen) else {
            return Err(format!("{screen} was drawn on before it showed a picture"));
        };
        let (right, bottom) = ((rect.x + rect.width) as usize, (rect.y + rect.height) as usize);
        if right > *width || bottom * *width > pixels.len() {
            return Err(format!("{screen} was drawn on outside its picture: {rect:?}"));
        }
        for y in 0..rect.height {
            for x in 0..rect.width {
                let at = (rect.y + y) as usize * *width + (rect.x + x) as usize;
                pixels[at] = colour(x, y);
            }
        }
        Ok(())
    }

    /// Takes in one line of the program's.
    fn take(&mut self, line: &Said) -> Result<(), String> {
        match line {
            Said::Shows { screen, width, height, seed } => {
                let all = (0..*height).flat_map(|y| (0..*width).map(move |x| (x, y)));
                let pixels = all.map(|(x, y)| pattern::colour(*seed, x, y)).collect();
                self.screens.insert(screen.clone(), Expected::Picture(*width as usize, pixels));
            }
            Said::Fill { screen, width, height, colour } => {
                let pixels = vec![*colour; *width as usize * *height as usize];
                self.screens.insert(screen.clone(), Expected::Picture(*width as usize, pixels));
            }
            Said::Patch { screen, rect, seed } => {
                self.paint(screen, rect, |x, y| pattern::colour(*seed, rect.x + x, rect.y + y))?;
            }
            Said::Pointer { screen, rect, seed } => {
                self.paint(screen, rect, |x, y| pattern::colour(*seed, x, y))?;
            }
            Said::Dark { screen } => {
                self.screens.insert(screen.clone(), Expected::Dark);
            }
            Said::Step(_) | Said::Passed | Said::Failed(_) => {}
        }
        Ok(())
    }

    /// Takes in the program's lines of `log` from where the last call
    /// stopped up to `end`.
    fn read_to(&mut self, log: &str, end: usize) -> Result<(), String> {
        let new = log.get(self.read..end).unwrap_or_default();
        for line in new.lines().filter_map(Said::read) {
            self.take(&line)?;
        }
        self.read = end;
        Ok(())
    }

    /// Compares a picture of every screen the program has named with
    /// what it said of it, for as long as a frame may take to follow
    /// the log.
    pub(super) fn look(&self, vm: &mut Vm) -> Result<(), String> {
        for (name, expected) in &self.screens {
            let head = head_of(name)?;
            let until = Instant::now() + PATIENCE;
            while let Some(why) = differs(expected, &vm.screenshot_of(head)?) {
                if Instant::now() >= until {
                    let program = self.program();
                    return Err(format!("{name} does not show what {program} said: {why}"));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
        Ok(())
    }

    /// Waits until the program is at its step `name`, where it waits for
    /// a key, and takes in what it has said the screens show. The
    /// caller then looks at the screens, does what the step is for and
    /// presses a key ([`Watch::go_on`]).
    pub(super) fn arrive(&mut self, vm: &mut Vm, name: &str) -> Result<(), String> {
        let step = Said::Step(name.to_string()).said_by(self.start);
        if vm.expect_first(&[&step, &said::failure(self.start)])? != 0 {
            let from = vm.cursor;
            vm.expect("\n")?;
            let why = vm.log().get(from..vm.cursor).unwrap_or_default().trim_end().to_string();
            return Err(format!("{} failed before its step {name}{why}", self.program()));
        }
        self.read_to(&vm.log(), vm.cursor)
    }

    /// As [`Watch::arrive`], and compares the screens with what the
    /// program said of them ([`Watch::look`]).
    pub(super) fn step(&mut self, vm: &mut Vm, name: &str) -> Result<(), String> {
        self.arrive(vm, name)?;
        self.look(vm).map_err(|why| format!("at step {name}: {why}"))
    }

    /// Presses the key the program waits for at a step.
    pub(super) fn go_on(vm: &mut Vm) -> Result<(), String> {
        vm.press(&["spc"])
    }

    /// Goes through the steps `names`, one after another, with nothing
    /// for the host to do at them but look.
    pub(super) fn steps(&mut self, vm: &mut Vm, names: &[&str]) -> Result<(), String> {
        for name in names {
            self.step(vm, name)?;
            Watch::go_on(vm)?;
        }
        Ok(())
    }
}

/// Where Spark says how `display-test` ended.
const ENDED: &str = "(/system/tests/posix/display-test) exited with code ";

/// Where Spark says how the `window-test` that ran the desktop ended:
/// as the desktop did, which ends as the command it ran (desktop(1)),
/// the `window-test` in it.
const DESKTOP_ENDED: &str = "(/system/tests/posix/window-test) exited with code ";

/// Waits for `display-test` to end, and fails unless it passed.
pub(super) fn passed(vm: &mut Vm) -> Result<(), String> {
    ended_well(vm, ENDED, said::START)
}

/// Waits for the desktop that ran `window-test` to end, and fails unless
/// `window-test` passed.
pub(super) fn desktop_passed(vm: &mut Vm) -> Result<(), String> {
    ended_well(vm, DESKTOP_ENDED, said::WINDOW_START)
}

/// Waits for Spark's line `ended`, and fails unless the code on it is 0
/// and the program whose lines start with `start` said it passed, and
/// of no failure.
fn ended_well(vm: &mut Vm, ended: &str, start: &'static str) -> Result<(), String> {
    let code = super::program_line(vm, ended)?;
    let log = vm.log();
    let failure = said::failure(start);
    let failed: Vec<&str> =
        log.lines().filter(|line| line.contains(&failure)).map(str::trim).collect();
    let passed = Said::Passed.said_by(start);
    let passed = log.lines().any(|line| line.trim_end().ends_with(&passed));
    if !failed.is_empty() || !passed || code != "0x0" {
        let program = start.trim_end_matches(": ");
        return Err(format!("{program} did not pass ({code}): {}", failed.join("; ")));
    }
    Ok(())
}

/// The script of the two tests of a panic while a program has the
/// screens (`display-panic`, `display-panic-vga`): `display-test panic`
/// shows its pictures and asks for a panic, and the report is read from
/// the screen of the card the kernel's log is on, which is the
/// machine's first.
pub(super) fn panic_behind_a_program(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let mut watch = Watch::default();
    watch.step(vm, "shown")?;
    Watch::go_on(vm)?;
    vm.expect("Crash dump: a test panic was asked for by process ")?;
    vm.expect("Radix panic on CPU ")?;
    vm.expect("dumps test: a panic asked for on purpose")?;
    vm.expect("Press a key to restart")?;
    vm.expect_screen(&[
        "RADIX KERNEL PANIC",
        "dumps test: a panic asked for on purpose",
        "Recent kernel log:",
        "Press a key to restart",
    ])?;
    // A key on the serial port restarts the machine, which ends QEMU.
    vm.send("\r")?;
    vm.expect_exit(QEMU_EXIT_RESET)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screenshot of `width` by `height` whose pixels `colour` gives.
    fn image(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&colour(x, y));
            }
        }
        Image::parse_ppm(&bytes).unwrap()
    }

    /// A watch that has read `lines`.
    fn watched(lines: &[&str]) -> Result<Watch, String> {
        let said = |log: String, line: &&str| log + "[  1.0 cpu0] display-test: " + line + "\n";
        let log = lines.iter().fold(String::new(), said);
        let mut watch = Watch::default();
        watch.read_to(&log, log.len())?;
        Ok(watch)
    }

    #[test]
    fn a_screen_shows_what_the_lines_built_up() {
        let watch = watched(&[
            "shows virtio0/1 40x30 seed 3",
            "patch virtio0/1 10x5 at 20,10 seed 4",
            "pointer virtio0/1 4x4 at 1,2 seed 5",
            "step shown.",
        ])
        .unwrap();
        let drawn = |x: u32, y: u32| {
            if (1..5).contains(&x) && (2..6).contains(&y) {
                pattern::colour(5, x - 1, y - 2)
            } else if (20..30).contains(&x) && (10..15).contains(&y) {
                pattern::colour(4, x, y)
            } else {
                pattern::colour(3, x, y)
            }
        };
        let expected = &watch.screens["virtio0/1"];
        assert_eq!(differs(expected, &image(40, 30, drawn)), None);
        // One pixel off, a picture shown a pixel to the side, another
        // size: each is told.
        let off = |x, y| if (x, y) == (7, 9) { [1, 2, 3] } else { drawn(x, y) };
        let why = differs(expected, &image(40, 30, off)).unwrap();
        assert!(
            why.starts_with(
                "1 of its 1200 pixels are not as drawn, the first at (7, 9): [1, 2, 3]"
            ),
            "{why}"
        );
        let why = differs(expected, &image(40, 30, |x, y| drawn(x + 1, y))).unwrap();
        assert!(why.starts_with("1200 of its 1200 pixels"), "{why}");
        let why = differs(expected, &image(30, 40, drawn)).unwrap();
        assert_eq!(why, "it is 30x40, not 40x30");
    }

    #[test]
    fn a_dark_screen_may_have_a_few_words_on_it() {
        let watch = watched(&["shows firmware0/0 40x30 seed 3", "dark firmware0/0"]).unwrap();
        let expected = &watch.screens["firmware0/0"];
        assert_eq!(*expected, Expected::Dark);
        assert_eq!(
            differs(expected, &image(64, 48, |x, y| [u8::from(y == 3 && x < 30) * 255; 3])),
            None
        );
        let why = differs(expected, &image(40, 30, |x, y| pattern::colour(3, x, y))).unwrap();
        assert!(why.ends_with("pixels are lit of a screen that shows nothing"), "{why}");
    }

    #[test]
    fn lines_that_cannot_be_are_refused_and_the_log_is_read_once() {
        let outside =
            watched(&["shows virtio0/0 40x30 seed 3", "patch virtio0/0 10x5 at 35,10 seed 4"]);
        assert!(outside.unwrap_err().contains("outside its picture"));
        let early = watched(&["patch virtio0/0 10x5 at 0,0 seed 4"]);
        assert!(early.unwrap_err().contains("before it showed a picture"));
        // A line read once is not read again: the second read starts
        // after it, where a line says the screen is dark.
        let log = "display-test: shows virtio0/0 4x4 seed 1\ndisplay-test: dark virtio0/0\n";
        let mut watch = Watch::default();
        let first = log.find("display-test: dark").unwrap();
        watch.read_to(log, first).unwrap();
        assert!(matches!(watch.screens["virtio0/0"], Expected::Picture(4, _)));
        watch.read_to(log, log.len()).unwrap();
        assert_eq!(watch.screens["virtio0/0"], Expected::Dark);
    }

    #[test]
    fn a_screens_name_says_which_head_it_is() {
        assert_eq!(head_of("firmware0/0"), Ok(None));
        assert_eq!(head_of("virtio0/2"), Ok(Some(2)));
        assert!(head_of("virtio1/0").is_err());
        assert!(head_of("firmware0").is_err());
        let view = view_argument(SCREEN_VIEW[2], Path::new("/tmp/t"), "display-virtio");
        assert_eq!(
            view.as_deref(),
            Some("unix:/tmp/t/display-virtio.screen2,display=gpu0,head=2,id=view2")
        );
        assert_eq!(view_argument("-vnc", Path::new("/tmp/t"), "display-virtio"), None);
    }
}
