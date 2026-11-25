// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test beat`: the beat of each card's pipe (ADR 0703), on a
//! machine with the firmware's card and a virtio one. Neither has a
//! beat of its own, so the kernel's clock gives each its mode's rate.
//!
//! For each pipe in turn:
//!
//! - beats asked for one after another come at moments a whole number
//!   of beats apart, to the nanosecond, each with its number, and mostly
//!   none is missed; a beat's deadline is within the beat after it;
//! - a change that does not wait shows at a later beat than the one it
//!   was asked after, and its event carries that beat;
//! - a change that waits comes back at the next beat, and one that asks
//!   to show at once comes back at once;
//! - while a change waits, another of the same pipe is refused as busy,
//!   unless it says the newest wins: then the one that waited ends as
//!   replaced, and only the newest shows.
//!
//! Then, with nothing to show and nobody asking, neither card's thread
//! wakes for two seconds. Last, a pointer on each screen: the firmware's
//! card draws it into its frame, and puts back what was under it when
//! it moves.
//!
//! | Step | What the screens show at it |
//! |---|---|
//! | `shown` | a picture on each |
//! | `beats` | a new picture on each, after the beats |
//! | `pointer` | a pointer on each (in the firmware's frame; the host's own over the virtio screen) |
//! | `moved` | the pointers elsewhere, and nothing where they were |

use std::time::{Duration, Instant};

use display::Unmade;
use display::abi::{
    DisplayEvent, EVENT_BEAT, EVENT_REPLACED, EVENT_SHOWN, MAY_BLANK, NEWEST_WINS, NO_WAIT, NOW,
};

use crate::rig::{Keys, Outcome, Seat, State, ensure};
use crate::stage::{Stage, unmade};
use crate::sys;

/// How many beats in a row are asked for.
const BEATS: u64 = 40;

/// How long an event may take to come.
const PATIENCE: Duration = Duration::from_secs(5);

/// Runs the test.
pub fn run() -> Outcome {
    let mut keys = Keys::open()?;
    let mut stage = Stage::of(Seat::open()?, 0)?;
    stage.show_all(MAY_BLANK)?;
    let cards: Vec<u32> = stage.shown.iter().map(|one| one.card).collect();
    ensure(cards.len() == 2 && cards[0] != cards[1], || {
        format!("the screens shown are of cards {cards:?}, not one of each of two")
    })?;
    keys.step("shown")?;
    for at in 0..stage.shown.len() {
        let woke = wakes(&stage)?[at];
        steady_beats(&mut stage, at)?;
        let woken = wakes(&stage)?[at] - woke;
        ensure(woken >= BEATS / 2, || {
            format!("{BEATS} beats woke the card's thread {woken} times")
        })?;
        a_change_shows_at_a_later_beat(&mut stage, at)?;
        waits_for_the_beat_unless_now(&stage, at)?;
        the_newest_wins(&mut stage, at)?;
        println!("display-test: the beat of {} is steady", stage.shown[at].name);
    }
    a_still_screen_wakes_nobody(&stage)?;
    stage.repaint(0).map_err(|err| unmade("new pictures after the beats", &err))?;
    keys.step("beats")?;
    let mut pointers = Vec::new();
    for at in 0..stage.shown.len() {
        pointers.push(stage.point(at, 32, (4, 6), (200, 150))?);
    }
    keys.step("pointer")?;
    for pointer in &mut pointers {
        stage.move_pointer(pointer, (640, 400))?;
    }
    keys.step("moved")
}

/// How many times the thread of each shown screen's card has woken.
fn wakes(stage: &Stage) -> Outcome<Vec<u64>> {
    let state = State::read()?;
    stage.shown.iter().map(|one| state.number("card", one.card, "wakes")).collect()
}

/// The nanoseconds of `beats` beats at `rate` frames in 1000 seconds,
/// rounded down.
fn span(beats: u64, rate: u64) -> u64 {
    (u128::from(beats) * 1_000_000_000_000 / u128::from(rate.max(1))) as u64
}

/// Fails unless `later` is a whole number of beats after `earlier`, to
/// the nanosecond, with a number that much higher.
fn on_the_grid(earlier: &DisplayEvent, later: &DisplayEvent, rate: u64) -> Outcome {
    ensure(later.count > earlier.count, || {
        format!("beat {} came after beat {}", later.count, earlier.count)
    })?;
    let (apart, wanted) =
        (later.time_ns - earlier.time_ns, span(later.count - earlier.count, rate));
    ensure(apart.abs_diff(wanted) <= 1, || {
        format!("beats {} and {} are {apart} ns apart, not {wanted}", earlier.count, later.count)
    })
}

/// The next beat of `pipe`, asked for now.
fn next_beat(seat: &mut Seat, pipe: u32) -> Outcome<DisplayEvent> {
    seat.holder.ask_beat(pipe).map_err(|err| format!("ASK_BEAT: {err}"))?;
    seat.event("EVENT_BEAT", PATIENCE, |event| event.kind == EVENT_BEAT && event.thing == pipe)
}

/// Beats asked for one after another.
fn steady_beats(stage: &mut Stage, at: usize) -> Outcome {
    let (pipe, rate) = (stage.shown[at].pipe, u64::from(stage.shown[at].mode.refresh_mhz));
    let info = stage.seat.holder.pipe(pipe).map_err(|err| format!("pipe {pipe}: {err}"))?;
    let period = span(1, rate);
    ensure(info.beat_ns.abs_diff(period) <= 1, || {
        format!("a pipe at {rate} mHz says its beat is {} ns", info.beat_ns)
    })?;
    let mut last = next_beat(&mut stage.seat, pipe)?;
    let mut in_step = 0;
    for _ in 0..BEATS {
        let beat = next_beat(&mut stage.seat, pipe)?;
        on_the_grid(&last, &beat, rate)?;
        in_step += u64::from(beat.count == last.count + 1);
        // It was told after it came, and not long after.
        let late = sys::now_ns().saturating_sub(beat.time_ns);
        ensure(sys::now_ns() >= beat.time_ns && late < 2_000_000_000, || {
            format!("a beat of {} ns was told at {} ns", beat.time_ns, sys::now_ns())
        })?;
        let room = beat.deadline_ns.saturating_sub(beat.time_ns);
        ensure(room > 0 && room <= period + 1, || {
            format!("a beat's deadline is {room} ns after it, of a beat of {period} ns")
        })?;
        last = beat;
    }
    ensure(in_step >= BEATS / 2, || {
        format!("of {BEATS} beats asked for in a row, {in_step} were")
    })?;
    let info = stage.seat.holder.pipe(pipe).map_err(|err| format!("pipe {pipe}: {err}"))?;
    ensure(info.beat_count >= last.count, || {
        format!("the pipe is at beat {} after beat {}", info.beat_count, last.count)
    })
}

/// A change that does not wait, asked for right after a beat: its
/// EVENT_SHOWN is of a later beat, on the same grid.
fn a_change_shows_at_a_later_beat(stage: &mut Stage, at: usize) -> Outcome {
    const COOKIE: u64 = 77;
    let (pipe, rate) = (stage.shown[at].pipe, u64::from(stage.shown[at].mode.refresh_mhz));
    let beat = next_beat(&mut stage.seat, pipe)?;
    stage.touch(at, NO_WAIT, COOKIE).map_err(|err| unmade("a change that does not wait", &err))?;
    let mine = |event: &DisplayEvent| event.kind == EVENT_SHOWN && event.cookie == COOKIE;
    let shown = stage.seat.event("EVENT_SHOWN", PATIENCE, mine)?;
    ensure(shown.thing == pipe, || format!("pipe {pipe}'s change showed on {}", shown.thing))?;
    on_the_grid(&beat, &shown, rate)
}

/// The middle one of nine times a change of `flags` takes, each asked
/// for as soon as the one before came back.
fn middle_time(stage: &Stage, at: usize, flags: u32) -> Outcome<Duration> {
    // The first comes back at a beat: the rest start from there.
    stage.touch(at, 0, 0).map_err(|err| unmade("a change that waits", &err))?;
    let mut times = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        stage.touch(at, flags, 0).map_err(|err| unmade("a change", &err))?;
        times.push(start.elapsed());
    }
    times.sort();
    Ok(times[times.len() / 2])
}

/// A change waits for its pipe's next beat, unless it asks to show at
/// once.
fn waits_for_the_beat_unless_now(stage: &Stage, at: usize) -> Outcome {
    let period = Duration::from_nanos(span(1, u64::from(stage.shown[at].mode.refresh_mhz)));
    let waited = middle_time(stage, at, 0)?;
    ensure(waited >= period / 2, || {
        format!("a change that waits for a beat of {period:?} came back after {waited:?}")
    })?;
    let at_once = middle_time(stage, at, NOW)?;
    ensure(at_once <= waited / 2, || {
        format!("a change to show at once took {at_once:?}; one that waits takes {waited:?}")
    })
}

/// How one try of [`the_newest_wins`] went.
enum Try {
    /// As it should.
    Passed,
    /// A beat came between the changes, so that nothing waited when the
    /// next was asked for: the try says nothing.
    Raced,
}

/// Three changes of one pipe within one beat: the first waits, the
/// second is refused as busy, and the third takes the first's place.
fn one_try(stage: &mut Stage, at: usize) -> Outcome<Try> {
    // From right after a beat, so that a whole beat is ahead.
    stage.touch(at, 0, 0).map_err(|err| unmade("a change that waits", &err))?;
    stage.seat.forget_events(Duration::ZERO)?;
    stage.touch(at, NO_WAIT, 1).map_err(|err| unmade("the first change", &err))?;
    let second = stage.touch(at, NO_WAIT, 2);
    let third = stage.touch(at, NO_WAIT | NEWEST_WINS, 3);
    third.map_err(|err| unmade("a change that takes the place of one that waits", &err))?;
    let mut ends = Vec::new();
    while ends.last() != Some(&(EVENT_SHOWN, 3)) {
        let end = |event: &DisplayEvent| matches!(event.kind, EVENT_SHOWN | EVENT_REPLACED);
        let event = stage.seat.event("end of the three changes", PATIENCE, end)?;
        ends.push((event.kind, event.cookie));
    }
    match second {
        Err(Unmade::Failed(err)) if err.raw_os_error() == Some(libc::EBUSY) => {}
        Ok(()) => return Ok(Try::Raced),
        Err(other) => return Err(unmade("a change while another waits", &other)),
    }
    if ends == [(EVENT_REPLACED, 1), (EVENT_SHOWN, 3)] {
        Ok(Try::Passed)
    } else if ends == [(EVENT_SHOWN, 1), (EVENT_SHOWN, 3)] {
        Ok(Try::Raced)
    } else {
        Err(format!("the changes ended as {ends:?} (1 is shown, 2 replaced; then the cookie)"))
    }
}

/// While a change waits, another is busy, and one that says the newest
/// wins replaces it.
fn the_newest_wins(stage: &mut Stage, at: usize) -> Outcome {
    for _ in 0..8 {
        if let Try::Passed = one_try(stage, at)? {
            return Ok(());
        }
    }
    Err("eight times in a row, a beat came between changes made one right after another".into())
}

/// With nothing to show and nobody asking for a beat, no card's thread
/// wakes.
fn a_still_screen_wakes_nobody(stage: &Stage) -> Outcome {
    // What the checks before left has shown by then.
    std::thread::sleep(Duration::from_millis(500));
    let before = wakes(stage)?;
    std::thread::sleep(Duration::from_secs(2));
    let after = wakes(stage)?;
    ensure(before == after, || {
        format!("with nothing to show, the cards' threads woke: {before:?}, then {after:?}")
    })
}
