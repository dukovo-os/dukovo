// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test cost`: what the layer's short paths cost, for `ob bench
//! --instructions`, on a machine with the firmware's card and a virtio
//! one. Each row is one `BENCH` line: the nanoseconds one operation
//! took, which are the instructions it took where the machine counts
//! them.
//!
//! | Row | One operation |
//! |---|---|
//! | `display_check` | CHECK_ONLY of a change that puts another picture on a main plane |
//! | `display_change_queued` | that change with NO_WAIT and NEWEST_WINS: checked, and left to wait for the beat in the place of the one before |
//! | `display_change_<card>` | that change made at once (NOW): the whole picture is new to the card |
//! | `display_small_<card>` | a change made at once that names 64 by 64 pixels of the picture as new, clear of the pointer |
//! | `display_pointer_<card>` | MOVE_POINTER of a pointer of 32 by 32 pixels, every one of them solid, to a place clear of the last |
//!
//! `<card>` is `firmware` or `virtio`. An operation is timed by itself,
//! and what reading the clock twice costs is taken off. A card is given
//! a moment after each operation that sends it something, outside the
//! time: a virtio card's answers are then there to be taken by the next
//! operation, as many each time, and none has to be waited for. The
//! events of the changes that wait are read outside the time too, as
//! a program that makes such changes reads them.

use std::time::Duration;

use display::abi::{
    CARD_FIRMWARE, CARD_VIRTIO, CHECK_ONLY, MAY_BLANK, NEWEST_WINS, NO_WAIT, NOW, PLANE_CHANGED,
    PLANE_PICTURE,
};
use display::wire::{self, ChangeBytes};
use sysapi::time;

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
        one_card(&mut stage, at, card, clock)?;
    }
    Ok(())
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
