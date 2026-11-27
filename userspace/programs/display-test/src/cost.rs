// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test cost`: what the layer's short paths cost, and an input
//! event's way to its reader, for `ob bench`, on a machine with the
//! firmware's card, a virtio one and a virtio mouse. Each row is one
//! `BENCH` line: the nanoseconds one operation took, which are the
//! instructions it took where the machine counts them (`ob bench
//! --instructions`). `linux-bench --only display` measures Linux's like
//! of each row in the same way.
//!
//! | Row | One operation |
//! |---|---|
//! | `display_check` | CHECK_ONLY of a change that puts another picture on a main plane |
//! | `display_change_queued` | that change with NO_WAIT and NEWEST_WINS: checked, and left to wait for the beat in the place of the one before |
//! | `display_change_<card>` | that change made at once (NOW): the whole picture is new to the card |
//! | `display_small_<card>` | a change made at once that names 64 by 64 pixels of the picture as new, clear of the pointer |
//! | `display_pointer_<card>` | MOVE_POINTER of a pointer of 32 by 32 pixels, every one of them solid, to a place clear of the last |
//! | `input_to_reader` | from the time a motion of the mouse was stamped with at its interrupt to a reader, which waited in read(2), having read its report: the middle one of the motions |
//! | `input_to_reader_p99` | the same, for the motion that 99 in 100 were faster than |
//!
//! `<card>` is `firmware` or `virtio`. An operation is timed by itself,
//! and what reading the clock twice costs is taken off. A card is given
//! a moment after each operation that sends it something, outside the
//! time: a virtio card's answers are then there to be taken by the next
//! operation, as many each time, and none has to be waited for. The
//! events of the changes that wait are read outside the time too, as
//! a program that makes such changes reads them.
//!
//! Before each card's rows the program says `display-test: <card> shows
//! <width> by <height>`, which `ob bench` holds against the size Linux's
//! rows are measured at. The motions come from outside: the program
//! says `display-test: input: ready for <count> motions`, and `ob bench`
//! sends that many through QEMU, one at a time.

use std::io::Read;
use std::time::Duration;

use display::abi::{
    CARD_FIRMWARE, CARD_VIRTIO, CHECK_ONLY, MAY_BLANK, NEWEST_WINS, NO_WAIT, NOW, PLANE_CHANGED,
    PLANE_PICTURE,
};
use display::wire::{self, ChangeBytes};
use input::abi::{EV_SYN, SYN_REPORT};
use input::wire::events;
use sysapi::time;

use crate::input::open_named;
use crate::rig::{Outcome, Seat};
use crate::stage::{Stage, unmade};

/// How many times each operation is done.
const ROUNDS: u32 = 400;

/// The moment a card is given after an operation that sent it something.
const MOMENT: Duration = Duration::from_millis(2);

/// The side of the rectangle a small change names.
const SMALL: u16 = 64;

/// The side of the pointer's picture.
const POINTER: u32 = 32;

/// Where the pointer is, and the column it moves to and back from: clear
/// of the small change's rectangle and of each other.
const POINTER_AT: (i16, i16) = (400, 300);
const POINTER_THERE: i16 = 440;

/// The name the virtio mouse gives itself, and how many of its motions
/// are timed.
const MOUSE: &str = "QEMU Virtio Mouse";
const MOTIONS: usize = 400;

/// Runs the rows.
pub fn run() -> Outcome {
    let mut stage = Stage::of(Seat::open()?, 0)?;
    stage.show_all(MAY_BLANK)?;
    let clock = clock_cost();
    whole_layer(&mut stage, clock)?;
    for at in 0..stage.shown.len() {
        let card = stage.found.card(stage.shown[at].card)?.kind;
        let card = match card {
            CARD_FIRMWARE => "firmware",
            CARD_VIRTIO => "virtio",
            other => return Err(format!("a card of kind {other}")),
        };
        let canvas = &stage.shown[at].canvas;
        println!("display-test: {card} shows {} by {}", canvas.width, canvas.height);
        one_card(&mut stage, at, card, clock)?;
    }
    input_rows()
}

/// What reading the clock twice in a row costs: the least of many
/// tries.
fn clock_cost() -> u64 {
    let pair = || {
        let before = time::monotonic_ns();
        time::monotonic_ns().saturating_sub(before)
    };
    (0..1000).map(|_| pair()).min().unwrap_or(0)
}

/// Prints the row `name`: what `one` took, on average over [`ROUNDS`]
/// operations, each timed by itself and followed by `rest` outside its
/// time. The first operation is not timed: it sets up what the others
/// find.
fn row(
    name: &str,
    clock: u64,
    mut one: impl FnMut(u32) -> Outcome,
    mut rest: impl FnMut(u32) -> Outcome,
) -> Outcome {
    // What the row before printed has reached the log by then.
    std::thread::sleep(Duration::from_millis(100));
    one(0)?;
    rest(0)?;
    let mut total = 0;
    for round in 1..=ROUNDS {
        let before = time::monotonic_ns();
        one(round)?;
        total += time::monotonic_ns().saturating_sub(before).saturating_sub(clock);
        rest(round)?;
    }
    let each = total as f64 / f64::from(ROUNDS);
    println!("BENCH name={name} ns={each:.2} iterations={ROUNDS}");
    Ok(())
}

/// The value of `sorted` that `part` in 100 of them are at or below.
fn at_part(sorted: &[u64], part: usize) -> u64 {
    sorted[(sorted.len() - 1) * part / 100]
}

/// The input rows: each of [`MOTIONS`] motions of the mouse, from the
/// time its interrupt stamped it with to this reader having read it.
/// The reader is tied to nobody, and waits in read(2) for each.
fn input_rows() -> Outcome {
    let mut mouse = open_named(MOUSE)?;
    println!("display-test: input: ready for {MOTIONS} motions");
    let mut took = Vec::with_capacity(MOTIONS);
    let mut bytes = [0u8; 256];
    while took.len() < MOTIONS {
        let read = mouse.read(&mut bytes).map_err(|err| format!("reading the mouse: {err}"))?;
        let now = time::monotonic_ns();
        let before = took.len();
        took.extend(reports(&bytes[..read]).map(|stamp| now.saturating_sub(stamp)));
        if took.len() == before {
            return Err(format!("the mouse gave {read} bytes, which end no report"));
        }
    }
    took.sort_unstable();
    let motions = took.len();
    println!("BENCH name=input_to_reader ns={} iterations={motions}", at_part(&took, 50));
    println!("BENCH name=input_to_reader_p99 ns={} iterations={motions}", at_part(&took, 99));
    Ok(())
}

/// The time stamps of the reports that end among the events `read`
/// holds. A motion is one report, which one read takes; a reader kept
/// from the processor for longer than the motions are apart reads two
/// at once, and each counts with the time it waited.
fn reports(read: &[u8]) -> impl Iterator<Item = u64> + '_ {
    events(read)
        .filter(|event| (event.kind, event.code) == (EV_SYN, SYN_REPORT))
        .map(|event| event.time_ns)
}

/// The rest a card is given after an operation that sent it something.
fn moment(_round: u32) -> Outcome {
    std::thread::sleep(MOMENT);
    Ok(())
}

/// The rows that reach no card: the check alone, and a change that is
/// checked and left to wait.
fn whole_layer(stage: &mut Stage, clock: u64) -> Outcome {
    let plane = stage.shown[0].plane;
    let pictures = [stage.spare(0)?, stage.shown[0].canvas.picture];
    let seat = &stage.seat;
    let flip = |flags: u32, round: u32| {
        let mut change = ChangeBytes::new(flags, u64::from(round));
        change.set(plane, PLANE_PICTURE, u64::from(pictures[round as usize % 2]));
        seat.holder.change(&mut change)
    };
    let check = |round| flip(CHECK_ONLY, round).map_err(|err| unmade("a check", &err));
    row("display_check", clock, check, |_| Ok(()))?;
    let queue = |round| {
        flip(NO_WAIT | NEWEST_WINS, round).map_err(|err| unmade("a change that waits", &err))
    };
    row("display_change_queued", clock, queue, |_| seat.drop_unread())?;
    // The last of them shows at the next beat.
    std::thread::sleep(Duration::from_millis(100));
    seat.drop_unread()
}

/// The rows of the card that the screen at place `at` is of.
fn one_card(stage: &mut Stage, at: usize, card: &str, clock: u64) -> Outcome {
    let plane = stage.shown[at].plane;
    let pictures = [stage.spare(at)?, stage.shown[at].canvas.picture];
    let pointer = stage.point(at, POINTER, (0, 0), POINTER_AT)?.plane;
    let holder = &stage.seat.holder;
    let whole = |round: u32| {
        let mut change = ChangeBytes::new(NOW, 0);
        change.set(plane, PLANE_PICTURE, u64::from(pictures[round as usize % 2]));
        holder.change(&mut change).map_err(|err| unmade("a new picture at once", &err))
    };
    row(&format!("display_change_{card}"), clock, whole, moment)?;
    let small = |_| {
        let mut change = ChangeBytes::new(NOW, 0);
        change.set(plane, PLANE_CHANGED, wire::rect(64, 64, SMALL, SMALL));
        holder.change(&mut change).map_err(|err| unmade("a small change at once", &err))
    };
    row(&format!("display_small_{card}"), clock, small, moment)?;
    let point = |round: u32| {
        let x = if round.is_multiple_of(2) { POINTER_AT.0 } else { POINTER_THERE };
        holder.move_pointer(pointer, x, POINTER_AT.1).map_err(|err| format!("MOVE_POINTER: {err}"))
    };
    row(&format!("display_pointer_{card}"), clock, point, moment)
}

#[cfg(test)]
mod tests {
    use input::abi::{EV_REL, InputEvent, REL_X};

    use super::*;

    /// An event of `kind` and `code` stamped `time_ns`, as a read gives it.
    fn event(time_ns: u64, kind: u16, code: u16) -> [u8; InputEvent::SIZE] {
        let mut bytes = [0u8; InputEvent::SIZE];
        bytes[InputEvent::OFFSET_TIME_NS..][..8].copy_from_slice(&time_ns.to_le_bytes());
        bytes[InputEvent::OFFSET_KIND..][..2].copy_from_slice(&kind.to_le_bytes());
        bytes[InputEvent::OFFSET_CODE..][..2].copy_from_slice(&code.to_le_bytes());
        bytes
    }

    #[test]
    fn each_report_that_ends_in_a_read_is_one_motion() {
        // Two motions read at once, and half a third.
        let read = [
            event(5, EV_REL, REL_X),
            event(5, EV_SYN, SYN_REPORT),
            event(9, EV_REL, REL_X),
            event(9, EV_SYN, SYN_REPORT),
            event(12, EV_REL, REL_X),
        ];
        assert_eq!(reports(read.as_flattened()).collect::<Vec<_>>(), [5, 9]);
        assert_eq!(reports(&[]).count(), 0);
    }

    #[test]
    fn the_middle_and_the_99th_of_100() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(at_part(&sorted, 50), 50);
        assert_eq!(at_part(&sorted, 99), 99);
        assert_eq!(at_part(&[7], 99), 7);
    }
}
