// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `linux-bench --only display`: what Linux's own interface to a card's
//! screens (DRM) and its event devices cost, beside the rows that
//! `display-test cost` measures on Radix (ADRs 0700 and 0706). The
//! machine has the firmware's framebuffer, which Linux drives with
//! `simpledrm`, a virtio graphics card and a virtio mouse.
//!
//! | Row | One operation |
//! |---|---|
//! | `drm_check` | an atomic change that puts another framebuffer on a main plane, with TEST_ONLY |
//! | `drm_flip_<card>` | that change made: the whole picture is new to the card |
//! | `drm_dirty_<card>` | DIRTYFB of 64 by 64 pixels of the framebuffer that is shown |
//! | `drm_cursor_virtio` | the pointer moved (MODE_CURSOR) |
//! | `<row>_passed` | the DRM row `<row>` again, by the time that passed |
//! | `evdev_to_reader` | from the time stamp of a mouse's motion to its reader having read it: the middle one of the motions |
//! | `evdev_to_reader_p99` | the same, for the motion that 99 in 100 were faster than |
//!
//! `<card>` is `firmware` or `virtio`. Each operation is timed as
//! `display-test cost` times its own: by itself, what reading the clock
//! twice costs taken off, and the card given the same moment after it,
//! outside the time. Each DRM row is timed twice, by two clocks. The
//! row of its name is the calling thread's processor time. Then
//! `<row>_passed` is the time that passed, as Radix's rows are, over as
//! many operations again. `ob bench` sets the time that passed beside
//! Radix's row, but for `drm_flip_virtio` and `drm_dirty_virtio`:
//! `virtio_gpu` holds those two calls until the next of the 60 frames a
//! second it counts, and that wait is no cost of the change, so theirs
//! is the processor time. `simpledrm` has no pointer plane: a Linux
//! desktop draws the pointer into its picture there, so the row has no
//! like.
//!
//! The motions come from outside: the program prints
//! `linux-bench: input: ready for <count> motions`, and `ob bench` sends
//! that many through QEMU, one at a time. Before each card's rows it
//! prints `linux-bench: display: <card> shows <width> by <height>`, which
//! `ob bench` holds against the sizes Radix's rows are measured at.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sysapi::linux::drm::{Card, PLANE_PRIMARY, TEST_ONLY};
use sysapi::linux::evdev::{self, EV_SYN, EVENT_LEN, Event, SYN_REPORT};
use sysapi::linux::thread_cpu_ns;
use sysapi::time;

/// How many times each operation is done, the moment a card is given
/// after one that sent it something, and the side of the rectangle a
/// small change names: `display-test cost`'s own.
const ROUNDS: u32 = 400;
const MOMENT: Duration = Duration::from_millis(2);
const SMALL: u16 = 64;

/// How many more times each operation is done for the time that passed.
const PASSED: u32 = ROUNDS;

/// The side of the pointer's picture, which is the one size a virtio
/// card takes, and the two places it moves between.
const POINTER: u32 = 64;
const POINTER_AT: [(i32, i32); 2] = [(400, 300), (440, 300)];

/// Where the cards' and the event devices' files are.
const CARDS: &str = "/dev/dri";
const EVENT_DEVICES: &str = "/dev/input";

/// The name the virtio mouse gives itself.
const MOUSE: &str = "QEMU Virtio Mouse";

/// How many motions are timed.
const MOTIONS: usize = 400;

/// A row's outcome: an error says what could not be done.
type Outcome<T = ()> = Result<T, String>;

/// Runs the rows; what cannot be measured is said and left out.
pub fn run() {
    let clock = clock_cost();
    let mut checked = false;
    for path in files_of(CARDS, "card") {
        if let Err(why) = one_card(&path, clock, &mut checked) {
            eprintln!("linux-bench: display: {}: {why}", path.display());
        }
    }
    if let Err(why) = input_rows() {
        eprintln!("linux-bench: input: {why}");
    }
}

/// The files of `folder` whose names start with `prefix` and go on with
/// a number, in the order of their numbers.
fn files_of(folder: &str, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut found: Vec<(u32, PathBuf)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let number = path.file_name()?.to_str()?.strip_prefix(prefix)?.parse().ok()?;
            Some((number, path))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

/// What reading the thread's clock twice in a row costs: the least of
/// many tries.
fn clock_cost() -> u64 {
    let pair = || {
        let before = thread_cpu_ns();
        thread_cpu_ns().saturating_sub(before)
    };
    (0..1000).map(|_| pair()).min().unwrap_or(0)
}

/// Prints the row `name`: the processor time `one` took, on average
/// over [`ROUNDS`] operations, each timed by itself and followed by a
/// rest of `rest` outside its time. The first operation is not timed:
/// it sets up what the others find.
fn row(
    name: &str,
    clock: u64,
    rest: Duration,
    mut one: impl FnMut(u32) -> std::io::Result<()>,
) -> Outcome {
    let mut timed = |round: u32| -> Outcome<u64> {
        let before = thread_cpu_ns();
        one(round).map_err(|err| format!("{name}: {err}"))?;
        let took = thread_cpu_ns().saturating_sub(before).saturating_sub(clock);
        std::thread::sleep(rest);
        Ok(took)
    };
    timed(0)?;
    let mut total = 0;
    for round in 1..=ROUNDS {
        total += timed(round)?;
    }
    let each = total as f64 / f64::from(ROUNDS);
    println!("BENCH name={name} ns={each:.2} iterations={ROUNDS}");
    passed_row(name, rest, one)
}

/// Prints the row `name`_passed: the time that passed while `one` was
/// made, on average over [`PASSED`] more operations, each followed by a
/// rest of `rest`. It is the processor time and whatever the call waited
/// for, which Radix's rows have in them too: where Linux holds a call
/// for nothing, it is the row to set beside Radix's.
fn passed_row(
    name: &str,
    rest: Duration,
    mut one: impl FnMut(u32) -> std::io::Result<()>,
) -> Outcome {
    let pair = || {
        let before = time::monotonic_ns();
        time::monotonic_ns().saturating_sub(before)
    };
    let clock = (0..1000).map(|_| pair()).min().unwrap_or(0);
    let mut total = 0;
    for round in ROUNDS + 1..=ROUNDS + PASSED {
        let before = time::monotonic_ns();
        one(round).map_err(|err| format!("{name}: {err}"))?;
        total += time::monotonic_ns().saturating_sub(before).saturating_sub(clock);
        std::thread::sleep(rest);
    }
    let each = total as f64 / f64::from(PASSED);
    println!("BENCH name={name}_passed ns={each:.2} iterations={PASSED}");
    Ok(())
}

/// What Radix's rows call the card that Linux's driver `driver` drives.
fn card_name(driver: &str) -> Option<&'static str> {
    match driver {
        "simpledrm" => Some("firmware"),
        "virtio_gpu" => Some("virtio"),
        _ => None,
    }
}

/// The main plane that `crtc` shows, and its property that names the
/// framebuffer on it.
fn main_plane(card: &Card, crtc: u32) -> Outcome<(u32, u32)> {
    for plane in card.planes().map_err(|err| format!("its planes: {err}"))? {
        if card.plane_crtc(plane).ok() != Some(crtc) {
            continue;
        }
        let properties =
            card.plane_properties(plane).map_err(|err| format!("a plane's properties: {err}"))?;
        let of = |name: &str| properties.iter().find(|(_, is, _)| is == name);
        if let (Some((_, _, PLANE_PRIMARY)), Some(&(framebuffer, _, _))) = (of("type"), of("FB_ID"))
        {
            return Ok((plane, framebuffer));
        }
    }
    Err("no main plane shows on its screen".into())
}

/// The rows of the card at `path`. The check's row is the first card's,
/// as on Radix: `checked` says whether it has been printed.
fn one_card(path: &Path, clock: u64, checked: &mut bool) -> Outcome {
    let card = Card::open(path).map_err(|err| format!("opening it: {err}"))?;
    let driver = card.driver().map_err(|err| format!("its driver's name: {err}"))?;
    let Some(name) = card_name(&driver) else {
        eprintln!("linux-bench: display: {} is {driver}'s, which has no rows", path.display());
        return Ok(());
    };
    let resources = card.resources().map_err(|err| format!("its screens: {err}"))?;
    let screen = resources
        .connectors
        .iter()
        .find_map(|&connector| Some((connector, *card.modes(connector).ok()?.first()?)));
    let (Some((connector, mode)), Some(&crtc)) = (screen, resources.crtcs.first()) else {
        return Err(format!("{driver} has no screen plugged in"));
    };
    let (width, height) = mode.size();
    println!("linux-bench: display: {name} shows {width} by {height}");
    let mut framebuffers = [0; 2];
    for framebuffer in &mut framebuffers {
        let picture = card.picture(width, height).map_err(|err| format!("a picture: {err}"))?;
        *framebuffer = card.framebuffer(&picture).map_err(|err| format!("a framebuffer: {err}"))?;
    }
    card.set_mode(crtc, connector, &mode, framebuffers[0])
        .map_err(|err| format!("setting its mode: {err}"))?;
    let (plane, shows) = main_plane(&card, crtc)?;
    let next = |round: u32| u64::from(framebuffers[round as usize % 2]);
    if !std::mem::replace(checked, true) {
        // The framebuffer the check names is never the one shown: the
        // flips below start from the first.
        let check = |_| card.commit(TEST_ONLY, plane, shows, u64::from(framebuffers[1]));
        row("drm_check", clock, Duration::ZERO, check)?;
    }
    row(&format!("drm_flip_{name}"), clock, MOMENT, |round| {
        card.commit(0, plane, shows, next(round + 1))
    })?;
    // The flips ended on the first framebuffer or the second, as ROUNDS
    // is even or odd.
    let shown = framebuffers[(ROUNDS as usize + 1) % 2];
    row(&format!("drm_dirty_{name}"), clock, MOMENT, |_| card.changed(shown, 64, 64, SMALL))?;
    pointer_row(&card, crtc, name, clock)
}

/// The pointer's row, on a card that has a pointer plane.
fn pointer_row(card: &Card, crtc: u32, name: &str, clock: u64) -> Outcome {
    let picture = card.picture(POINTER, POINTER).map_err(|err| format!("a pointer: {err}"))?;
    if let Err(err) = card.set_pointer(crtc, &picture) {
        eprintln!("linux-bench: display: {name} has no pointer plane ({err}), so no row for it");
        return Ok(());
    }
    row(&format!("drm_cursor_{name}"), clock, MOMENT, |round| {
        let (x, y) = POINTER_AT[round as usize % 2];
        card.move_pointer(crtc, x, y)
    })
}

/// The virtio mouse's event device, stamping with the monotonic clock.
fn mouse() -> Outcome<File> {
    for path in files_of(EVENT_DEVICES, "event") {
        let Ok(device) = File::open(&path) else { continue };
        if evdev::name(&device).is_ok_and(|name| name == MOUSE) {
            evdev::stamp_with_monotonic(&device)
                .map_err(|err| format!("{}: its clock: {err}", path.display()))?;
            return Ok(device);
        }
    }
    Err(format!("no event device is the {MOUSE}"))
}

/// The value of `sorted` that `part` in 100 of them are at or below.
fn at_part(sorted: &[u64], part: usize) -> u64 {
    sorted[(sorted.len() - 1) * part / 100]
}

/// The input rows: each of [`MOTIONS`] motions of the mouse, from its
/// time stamp to this reader having read it. The reader waits in
/// read(2) for each.
fn input_rows() -> Outcome {
    let mut device = mouse()?;
    println!("linux-bench: input: ready for {MOTIONS} motions");
    let mut took = Vec::with_capacity(MOTIONS);
    let mut bytes = [0u8; EVENT_LEN * 16];
    while took.len() < MOTIONS {
        let read = device.read(&mut bytes).map_err(|err| format!("reading the mouse: {err}"))?;
        let now = time::monotonic_ns();
        let before = took.len();
        took.extend(reports(&bytes[..read]).map(|stamp| now.saturating_sub(stamp)));
        if took.len() == before {
            return Err(format!("the mouse gave {read} bytes, which end no report"));
        }
    }
    took.sort_unstable();
    let motions = took.len();
    println!("BENCH name=evdev_to_reader ns={} iterations={motions}", at_part(&took, 50));
    println!("BENCH name=evdev_to_reader_p99 ns={} iterations={motions}", at_part(&took, 99));
    Ok(())
}

/// The time stamps of the reports that end among the events `read`
/// holds. A motion is one report, which one read takes; a reader kept
/// from the processor for longer than the motions are apart reads two
/// at once, and each counts with the time it waited.
fn reports(read: &[u8]) -> impl Iterator<Item = u64> + '_ {
    read.as_chunks::<EVENT_LEN>()
        .0
        .iter()
        .map(Event::of)
        .filter(|event| (event.kind, event.code) == (EV_SYN, SYN_REPORT))
        .map(|event| event.time_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_and_the_99th_of_100() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(at_part(&sorted, 50), 50);
        assert_eq!(at_part(&sorted, 99), 99);
        assert_eq!(at_part(&[7], 99), 7);
    }

    #[test]
    fn each_report_that_ends_in_a_read_is_one_motion() {
        let event = |time_us: u64, kind: u16, code: u16| {
            let mut bytes = [0u8; EVENT_LEN];
            bytes[8..16].copy_from_slice(&time_us.to_ne_bytes());
            bytes[16..18].copy_from_slice(&kind.to_ne_bytes());
            bytes[18..20].copy_from_slice(&code.to_ne_bytes());
            bytes
        };
        // Two motions (EV_REL 2, REL_X 0) read at once, and half a third.
        let read =
            [event(5, 2, 0), event(5, 0, 0), event(9, 2, 0), event(9, 0, 0), event(12, 2, 0)];
        assert_eq!(reports(read.as_flattened()).collect::<Vec<_>>(), [5000, 9000]);
        assert_eq!(reports(&[]).count(), 0);
    }

    #[test]
    fn cards_are_named_as_radix_names_them() {
        assert_eq!(card_name("simpledrm"), Some("firmware"));
        assert_eq!(card_name("virtio_gpu"), Some("virtio"));
        assert_eq!(card_name("bochs-drm"), None);
    }
}
