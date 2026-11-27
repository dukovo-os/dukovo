// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `display-test handover`: the screens passed from program to program
//! (ADR 0702), on a machine with the firmware's card and a virtio one,
//! where consoled has them from the kernel's log.
//!
//! The test borrows the screens from consoled as a desktop does, lends
//! them on to a second holder of its own and has them given back and
//! left behind; no hand-over sets a mode, as every picture is in the
//! mode that shows. Then a program that has borrowed the screens is
//! killed, twice: what it showed stays until the next borrower's first
//! frame, or for five seconds, after which the consoles show again.
//!
//! | Step | What the screens show at it | What the host then does |
//! |---|---|---|
//! | `borrowed` | the test's pictures, over the consoles | |
//! | `lent` | the second holder's | |
//! | `returned` | the test's again, put back by the layer | |
//! | `ended` | the test's, after the second holder ended with the screens | |
//! | `consoles` | the consoles | |
//! | `child` | a child's pictures | |
//! | `again` | the test's, borrowed as soon as the child was killed | waits past five seconds, and looks again |
//! | `consoles-again` | the consoles | |
//! | `child-again` | another child's pictures | |
//! | `killed` | the same: the child is dead | looks again, and waits for the consoles |

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use display::Unmade;
use display::loan::Answer;
use display::abi::{DisplayEvent, EVENT_BACK, EVENT_BORROWER_ENDED, EVENT_FRONT, KIND_PIPE};
use display::wire::ChangeBytes;
use display_pattern::said::Said;
use sysapi::net;

use crate::rig::{Keys, Outcome, Seat, State, ensure};
use crate::stage::{Stage, unmade};

/// Where consoled lends the screens (consoled(8)).
const LENDER: &str = display::loan::SOCKET;

/// How long consoled has to start and to have the screens to lend.
const LENDER_PATIENCE: Duration = Duration::from_secs(120);

/// How long an event may take to come.
const PATIENCE: Duration = Duration::from_secs(5);

/// What the child says once its pictures show.
const READY: &str = "display-test: the child shows";

/// Reads consoled's answer on `stream`, and the descriptor that came
/// with it.
fn answer(stream: &UnixStream) -> Outcome<(String, Option<OwnedFd>)> {
    let mut bytes = [0u8; 32];
    let mut passed = Vec::new();
    let read = net::receive_with_fds(stream, &mut bytes, &mut passed, 1)
        .map_err(|err| format!("{LENDER}: {err}"))?;
    Ok((String::from_utf8_lossy(&bytes[..read]).into_owned(), passed.pop()))
}

/// Borrows the screens from consoled: a holder that is in front. It
/// asks again while consoled is not there yet or has no screens to
/// lend.
pub fn borrow_seat() -> Outcome<Seat> {
    let until = Instant::now() + LENDER_PATIENCE;
    loop {
        let said = match UnixStream::connect(LENDER) {
            Ok(stream) => match answer(&stream)? {
                (said, Some(holder)) if matches!(Answer::parse(&said), Some(Answer::Lent(_))) => {
                    return Ok(Seat::of(File::from(holder)));
                }
                (said, _) => format!("consoled answered `{}`", said.trim_end()),
            },
            Err(err) => format!("{LENDER}: {err}"),
        };
        if Instant::now() >= until {
            return Err(format!("the screens could not be borrowed: {said}"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// How many times each pipe's card was asked for a mode.
fn mode_sets(stage: &Stage) -> Outcome<Vec<u64>> {
    let state = State::read()?;
    let pipes = stage.found.of_kind(KIND_PIPE);
    pipes.map(|pipe| state.number("pipe", pipe.id, "mode_sets")).collect()
}

/// Waits for `seat`'s event of `kind`, named `what`.
fn told(seat: &mut Seat, kind: u32, what: &str) -> Outcome<DisplayEvent> {
    seat.event(what, PATIENCE, |event| event.kind == kind)
}

/// Fails unless `made` failed as busy.
fn busy(made: Result<(), Unmade>, what: &str) -> Outcome {
    match made {
        Err(Unmade::Failed(err)) if err.raw_os_error() == Some(libc::EBUSY) => Ok(()),
        other => Err(format!("{what}: {other:?}, not busy")),
    }
}

/// Runs the test.
pub fn run() -> Outcome {
    let mut keys = Keys::open()?;
    let mut ours = Stage::of(borrow_seat()?, 100)?;
    let sets = mode_sets(&ours)?;
    // No change here may blank a screen: each picture is in the mode
    // its screen shows.
    ours.show_all(0)?;
    ensure(ours.shown.len() == 2, || format!("{} screens show, not two", ours.shown.len()))?;
    keys.step("borrowed")?;
    lent_and_given_back(&mut ours, &mut keys)?;
    lent_and_left_behind(&mut ours, &mut keys)?;
    let after = mode_sets(&ours)?;
    ensure(after == sets, || format!("the hand-overs set modes: {sets:?}, then {after:?}"))?;
    ours.seat.holder.give_back().map_err(|err| format!("RETURN to consoled: {err}"))?;
    drop(ours);
    keys.step("consoles")?;
    a_borrower_that_starts_again(&mut keys)?;
    a_borrower_that_is_gone(&mut keys)
}

/// The test lends the screens to a second holder, which shows its own
/// pictures and gives the screens back: the layer puts the test's
/// pictures back by itself.
fn lent_and_given_back(ours: &mut Stage, keys: &mut Keys) -> Outcome {
    let mut theirs = Stage::of(Seat::open()?, 200)?;
    ours.seat.holder.lend(theirs.seat.slot()).map_err(|err| format!("LEND: {err}"))?;
    told(&mut ours.seat, EVENT_BACK, "EVENT_BACK for the lender")?;
    told(&mut theirs.seat, EVENT_FRONT, "EVENT_FRONT for the borrower")?;
    busy(ours.touch(0, 0, 0), "a change of a holder that lent the screens")?;
    theirs.show_all(0)?;
    keys.step("lent")?;
    theirs.seat.holder.give_back().map_err(|err| format!("RETURN: {err}"))?;
    told(&mut theirs.seat, EVENT_BACK, "EVENT_BACK for the borrower")?;
    told(&mut ours.seat, EVENT_FRONT, "EVENT_FRONT for the lender")?;
    busy(theirs.touch(0, 0, 0), "a change of a holder that gave the screens back")?;
    // The test has made no change since it lent the screens: what shows
    // is what the layer put back.
    ours.say_shown();
    keys.step("returned")
}

/// The second holder ends while it has the screens: its pictures stay
/// until the test, which is told, makes its next change.
fn lent_and_left_behind(ours: &mut Stage, keys: &mut Keys) -> Outcome {
    let mut theirs = Stage::of(Seat::open()?, 300)?;
    ours.seat.holder.lend(theirs.seat.slot()).map_err(|err| format!("LEND: {err}"))?;
    theirs.show_all(0)?;
    drop(theirs);
    told(&mut ours.seat, EVENT_BORROWER_ENDED, "EVENT_BORROWER_ENDED")?;
    told(&mut ours.seat, EVENT_FRONT, "EVENT_FRONT after the borrower ended")?;
    // What the borrower left shows until here. A change that asks for
    // nothing new makes the test's state again.
    let made = ours.seat.holder.change(&mut ChangeBytes::new(0, 0));
    made.map_err(|err| unmade("a change of nothing, after the borrower ended", &err))?;
    ours.say_shown();
    keys.step("ended")
}

/// The child's part: borrows the screens, shows pictures from `seed`
/// on, says so and waits to be killed.
pub fn borrow(seed: &str) -> Outcome {
    let seed = seed.parse().map_err(|_| format!("`{seed}` is no seed"))?;
    let mut stage = Stage::of(borrow_seat()?, seed)?;
    stage.show_all(0)?;
    println!("{READY}");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Starts a child that borrows the screens and shows pictures from
/// `seed` on. Returns once it shows them; its lines are in the log.
fn child(seed: u32) -> Outcome<Child> {
    let program =
        std::env::current_exe().map_err(|err| format!("the program's own path: {err}"))?;
    let mut command = Command::new(program);
    command.arg("borrow").arg(seed.to_string()).stdout(Stdio::piped());
    let mut child = command.spawn().map_err(|err| format!("starting a child: {err}"))?;
    let lines = BufReader::new(child.stdout.take().ok_or("the child has no output")?);
    for line in lines.lines() {
        let line = line.map_err(|err| format!("the child's output: {err}"))?;
        println!("{line}");
        if line == READY {
            return Ok(child);
        }
        if Said::read(&line).is_some_and(|said| matches!(said, Said::Failed(_))) {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("the child ended before it showed its pictures".into())
}

/// Kills `child` and waits until it is gone.
fn kill(mut child: Child) -> Outcome {
    child.kill().map_err(|err| format!("killing the child: {err}"))?;
    child.wait().map(|_| ()).map_err(|err| format!("waiting for the child: {err}"))
}

/// A borrower is killed and another borrows at once, as a desktop that
/// is started again does: its first frame is drawn over what the dead
/// one left, and consoled does not take the screens back meanwhile, nor
/// when its five seconds for the dead one are over.
fn a_borrower_that_starts_again(keys: &mut Keys) -> Outcome {
    let first = child(400)?;
    keys.step("child")?;
    kill(first)?;
    let mut ours = Stage::of(borrow_seat()?, 500)?;
    ours.show_all(0)?;
    keys.step("again")?;
    ours.seat.holder.give_back().map_err(|err| format!("RETURN to consoled: {err}"))?;
    drop(ours);
    keys.step("consoles-again")
}

/// A borrower is killed and nobody borrows: what it showed stays for
/// consoled's five seconds, and then the consoles show.
fn a_borrower_that_is_gone(keys: &mut Keys) -> Outcome {
    let last = child(600)?;
    keys.step("child-again")?;
    kill(last)?;
    // What the dead child said its screens show still holds: the host
    // looks, and then waits for the consoles.
    keys.step("killed")
}
