// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test virtio`: a virtio graphics card of four screens (ADR
//! 0704), on a machine with no other card. The first screen has a
//! monitor of 1024 by 768 from the start; the host plugs in, resizes and
//! pulls out the others' while the test runs, at the steps named here.
//!
//! | Step | What the screens show at it | What the host then does |
//! |---|---|---|
//! | `kept` | the first: a picture the card keeps, sent with PUT | |
//! | `put` | a part of it sent again | plugs in monitors of 800x600 and 640x480 |
//! | `three` | all three: pictures in the program's memory, shown by one change | |
//! | `again` | all three: new pictures, by one change that sets no mode | |
//! | `patch` | the second: only the rectangle the change named | |
//! | `mode` | the second: another of its modes | resizes the third to 1024x600 |
//! | `resized` | the third in its new size | pulls the second's monitor out |
//! | `unplugged` | the second: nothing | |
//! | `pointer` | what they did: the host draws the pointer over it | |

use display::Unmade;
use display::abi::{
    CARD_HOT_PLUG, CARD_SETS_MODES, CARD_VIRTIO, CHECK_ONLY, KIND_PIPE, MAY_BLANK, MODE_PREFERRED,
    PIPE_IS_ACTIVE, REASON_UNPLUGGED, REASON_WOULD_BLANK,
};
use display::wire;
use display_pattern::said::Rect;

use crate::rig::{Found, Keys, Outcome, Seat, State, ensure, say};
use crate::stage::{Stage, unmade};

/// The screens, by name.
const FIRST: &str = "virtio0/0";
const SECOND: &str = "virtio0/1";
const THIRD: &str = "virtio0/2";

/// Runs the test.
pub fn run() -> Outcome {
    let mut keys = Keys::open()?;
    let mut stage = Stage::of(Seat::open()?, 0)?;
    the_card_is_as_the_host_made_it(&stage)?;
    a_kept_picture(&mut stage, &mut keys)?;
    three_screens(&mut stage, &mut keys)?;
    one_change_for_all(&mut stage, &mut keys)?;
    only_what_changed(&mut stage, &mut keys)?;
    another_mode(&mut stage, &mut keys)?;
    a_window_resized(&mut stage, &mut keys)?;
    a_monitor_pulled_out(&mut stage, &mut keys)?;
    a_pointer(&mut stage, &mut keys)?;
    let front = State::read()?.front().to_string();
    let wanted = format!("front process {}", std::process::id());
    ensure(front == wanted, || format!("the kernel's account says `{front}`, not `{wanted}`"))
}

/// The sizes of the modes the screens with a monitor prefer, in order.
fn preferred(found: &Found) -> Vec<(String, u32, u32)> {
    let plugged = found.screens.iter().filter(|screen| screen.plugged());
    let size = |screen: &crate::rig::Screen| {
        let mode = screen.modes.first()?;
        Some((screen.name.clone(), mode.width, mode.height))
    };
    plugged.filter_map(size).collect()
}

/// How many times each pipe's card was asked for a mode, in the pipes'
/// order.
fn mode_sets(stage: &Stage) -> Outcome<Vec<u64>> {
    let state = State::read()?;
    let pipes = stage.found.of_kind(KIND_PIPE);
    pipes.map(|pipe| state.number("pipe", pipe.id, "mode_sets")).collect()
}

/// Before anything is shown: the log has the screens, and the layer has
/// one card, with four screens of which the first has a monitor that
/// says what it is.
fn the_card_is_as_the_host_made_it(stage: &Stage) -> Outcome {
    let front = State::read()?.front().to_string();
    ensure(front == "front log", || format!("`{front}` before anything was shown"))?;
    let found = &stage.found;
    ensure(found.cards.len() == 1, || format!("{} cards, not one", found.cards.len()))?;
    let card = found.card_named("virtio0")?;
    let flags = CARD_HOT_PLUG | CARD_SETS_MODES;
    ensure(card.kind == CARD_VIRTIO && card.flags & flags == flags, || {
        format!("the card is of kind {} with flags {:#x}", card.kind, card.flags)
    })?;
    let counts = (card.screens, card.pipes, card.planes);
    ensure(counts == (4, 4, 8), || format!("screens, pipes and planes: {counts:?}"))?;
    let sizes = preferred(found);
    ensure(sizes == [(FIRST.to_string(), 1024, 768)], || format!("monitors at first: {sizes:?}"))?;
    let first = found.screen(FIRST)?;
    ensure(first.modes[0].flags & MODE_PREFERRED != 0, || "the first mode is not marked".into())?;
    let data = stage.seat.holder.monitor_data(&first.info);
    let data = data.map_err(|err| format!("the monitor's data: {err}"))?;
    let whole = data.len() >= 128 && data.len().is_multiple_of(128);
    ensure(whole && data.starts_with(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]), || {
        format!("the monitor's data is {} bytes that start {:02x?}", data.len(), data.get(..8))
    })?;
    let (maker, name) = (wire::text(&first.info.maker), wire::text(&first.info.name));
    ensure(!maker.is_empty() && !name.is_empty(), || {
        format!("the monitor's maker is `{maker}` and its name `{name}`")
    })
}

/// The first change takes the screens from the log. The picture is one
/// the card keeps, as a console's is: PUT sends all of it, and then a
/// part of it again.
fn a_kept_picture(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let mut plans = stage.plans();
    plans[0].kept = true;
    stage.arrange(&plans, MAY_BLANK).map_err(|err| unmade("the first change", &err))?;
    let pipe = stage.seat.holder.pipe(stage.shown[0].pipe);
    let pipe = pipe.map_err(|err| format!("the first pipe: {err}"))?;
    let running =
        pipe.flags & PIPE_IS_ACTIVE != 0 && (pipe.mode.width, pipe.mode.height) == (1024, 768);
    ensure(running, || format!("the first pipe after the change: {pipe:?}"))?;
    keys.step("kept")?;
    stage.put_part(0, Rect { x: 300, y: 200, width: 200, height: 100 })?;
    keys.step("put")
}

/// The host has plugged two more monitors in. One change shows a
/// picture on each of the three, every picture the pages of a memory
/// file the program draws in.
fn three_screens(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let wanted = [
        (FIRST.to_string(), 1024, 768),
        (SECOND.to_string(), 800, 600),
        (THIRD.to_string(), 640, 480),
    ];
    stage.until_screens("two more monitors", |found| preferred(found) == wanted)?;
    stage.show_all(MAY_BLANK)?;
    // PUT is for pictures a card keeps.
    let mut put = wire::PutBytes::new();
    put.fill(0, 0, 8, 8, [0; 4]);
    let refused = stage.seat.holder.put(stage.shown[0].canvas.picture, &mut put);
    ensure(refused.is_err(), || "PUT wrote into a picture the program maps".into())?;
    for one in &stage.shown {
        let fed =
            stage.seat.holder.screen(one.screen).map_err(|err| format!("{}: {err}", one.name))?;
        ensure(fed.pipe == one.pipe, || format!("{} is fed by pipe {}", one.name, fed.pipe))?;
    }
    keys.step("three")
}

/// New pictures on all three by one change that may blank nothing, and
/// sets no mode.
fn one_change_for_all(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let before = mode_sets(stage)?;
    stage.repaint(0).map_err(|err| unmade("new pictures on three screens", &err))?;
    let after = mode_sets(stage)?;
    ensure(after == before, || format!("new pictures set modes: {before:?} then {after:?}"))?;
    keys.step("again")
}

/// All of the second screen's picture is drawn anew, and the change
/// names one rectangle of it: the screen shows that rectangle alone,
/// unless the host shows the program's own pages.
fn only_what_changed(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let rect = Rect { x: 100, y: 50, width: 200, height: 150 };
    let said = stage.patch(1, rect, 0, 0).map_err(|err| unmade("a change of a rectangle", &err))?;
    say(&said);
    keys.step("patch")
}

/// The second screen in another of its modes: refused unless the change
/// may blank the screen, and then one mode set, of its pipe alone.
fn another_mode(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let modes = &stage.found.screen(SECOND)?.modes;
    let other = modes.iter().filter(|mode| (mode.width, mode.height) != (800, 600));
    let other = other.min_by_key(|mode| mode.width * mode.height).copied();
    let other = other.ok_or_else(|| format!("{SECOND} has no second mode: {modes:?}"))?;
    let mut plans = stage.as_shown();
    plans[1].mode = other;
    match stage.arrange(&plans, CHECK_ONLY) {
        Err(Unmade::Refused { reason: REASON_WOULD_BLANK, .. }) => {}
        other => return Err(format!("a new mode that may not blank: {other:?}")),
    }
    let before = mode_sets(stage)?;
    stage.arrange(&plans, MAY_BLANK).map_err(|err| unmade("a new mode", &err))?;
    let after = mode_sets(stage)?;
    let wanted = [before[0], before[1] + 1, before[2], before[3]];
    ensure(after == wanted, || format!("mode sets were {before:?} and are {after:?}"))?;
    keys.step("mode")
}

/// The host has resized the third screen's window: the screen prefers
/// the new size, and is shown in it.
fn a_window_resized(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let resized = |found: &Found| preferred(found).contains(&(THIRD.to_string(), 1024, 600));
    stage.until_screens("the third screen's new size", resized)?;
    stage.show_all(MAY_BLANK)?;
    keys.step("resized")
}

/// The host has pulled the second monitor out. The state still feeds
/// its screen, and asking for that state again is refused; the new
/// state shows the two that are left.
fn a_monitor_pulled_out(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let with_it = stage.as_shown();
    let gone = |found: &Found| found.screen(SECOND).is_ok_and(|screen| !screen.plugged());
    stage.until_screens("the second monitor to go", gone)?;
    match stage.arrange(&with_it, CHECK_ONLY | MAY_BLANK) {
        Err(Unmade::Refused { reason: REASON_UNPLUGGED, .. }) => {}
        other => return Err(format!("a screen with no monitor was asked for: {other:?}")),
    }
    stage.show_all(MAY_BLANK)?;
    ensure(stage.shown.len() == 2, || format!("{} screens are left", stage.shown.len()))?;
    keys.step("unplugged")
}

/// A pointer on the first screen, moved twice. The host draws it, so
/// the screens' pictures are as they were; only a pointer's plane is
/// moved so.
fn a_pointer(stage: &mut Stage, keys: &mut Keys) -> Outcome {
    let mut pointer = stage.point(0, 32, (4, 6), (200, 150))?;
    stage.move_pointer(&mut pointer, (900, 700))?;
    let main = stage.shown[0].plane;
    let refused = stage.seat.holder.move_pointer(main, 1, 1);
    ensure(refused.is_err(), || "MOVE_POINTER moved a main plane".into())?;
    keys.step("pointer")
}
