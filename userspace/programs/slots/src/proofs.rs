// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The properties the slot state's functions promise (ADR 0452), checked
//! two ways.
//!
//! Each property is a plain function over a state. A Kani harness
//! (`ob test kani`) runs it over a symbolic state whose fields hold every
//! value a copy can hold, and a unit test runs it over each of the 648
//! states of [`every_state`], so that `cargo test` checks the same logic
//! on the host. No property formats a string: `decode(encode(state))` is
//! checked by the unit test of every state in [`crate::state`] and by the
//! `slot-state` fuzz target, since the cost of Kani's proof grows with
//! the formatting.

#[cfg(test)]
use alloc::vec::Vec;

use crate::state::{Checked, Copy, Refused, Slot, State, TRIES, newer, newest};

/// Whether `written` has the generation that follows `state`'s, counting
/// 0, 1, 2 and round, which is how every write of the state differs from
/// its predecessor in the generation.
fn follows(state: State, written: State) -> bool {
    written.generation == (state.generation + 1) % 3 && newer(written.generation, state.generation)
}

/// Every state a copy can hold.
#[cfg(test)]
pub(crate) fn every_state() -> Vec<State> {
    let maybe = [None, Some(Slot::A), Some(Slot::B)];
    let mut all = Vec::new();
    for generation in 0..3 {
        for default in [Slot::A, Slot::B] {
            for previous in maybe {
                for trying in maybe {
                    for tries in 0..=TRIES {
                        for boot in maybe {
                            all.push(State { generation, default, previous, trying, tries, boot });
                        }
                    }
                }
            }
        }
    }
    all
}

/// A slot being tried starts only while it has tries left; otherwise the
/// default starts, told of the fallback when a try ran out.
pub(crate) fn starts_the_slot_being_tried_only_with_tries_left(state: State) {
    let (slot, fallback) = state.starts();
    match state.trying {
        Some(trying) if state.tries > 0 => assert_eq!((slot, fallback), (trying, None)),
        Some(trying) => assert_eq!((slot, fallback), (state.default, Some(trying))),
        None => assert_eq!((slot, fallback), (state.default, None)),
    }
}

/// A start takes exactly one try from the slot being tried and never adds
/// one, marks the start as not seen through, and writes the next
/// generation; nothing else changes.
pub(crate) fn a_start_takes_one_try(state: State) {
    let (slot, _) = state.starts();
    let started = state.started();
    let tries_left = state.trying.is_some() && state.tries > 0;
    if tries_left {
        assert_eq!(started.tries, state.tries - 1);
        assert_eq!(Some(slot), state.trying);
    } else {
        assert_eq!(started.tries, state.tries);
    }
    assert!(started.tries <= state.tries);
    assert_eq!(started.boot, Some(slot));
    assert!(follows(state, started));
    assert_eq!(
        State { generation: started.generation, tries: state.tries, boot: state.boot, ..started },
        State { generation: started.generation, ..state }
    );
}

/// Choosing a slot by hand takes no try, marks the start as not seen
/// through, and writes the next generation; nothing else changes.
pub(crate) fn a_chosen_start_takes_no_try(state: State, slot: Slot) {
    let chose = state.chose(slot);
    assert_eq!(chose.tries, state.tries);
    assert_eq!(chose.boot, Some(slot));
    assert!(follows(state, chose));
    assert_eq!(State { generation: state.generation, boot: state.boot, ..chose }, state);
}

/// Of two whole copies one generation apart the newer is taken, whichever
/// file it is in; two of one generation give the first; one whole copy
/// gives that one; none give nothing.
pub(crate) fn the_newer_copy_is_taken(first: Option<State>, second: Option<State>) {
    match (first, second) {
        (Some(a), Some(b)) if newer(a.generation, b.generation) => {
            assert_eq!(newest(first, second), Some((a, Copy::First)));
        }
        (Some(a), Some(b)) if newer(b.generation, a.generation) => {
            assert_eq!(newest(first, second), Some((b, Copy::Second)));
        }
        (Some(a), Some(_)) => assert_eq!(newest(first, second), Some((a, Copy::First))),
        (Some(a), None) => assert_eq!(newest(first, second), Some((a, Copy::First))),
        (None, Some(b)) => assert_eq!(newest(first, second), Some((b, Copy::Second))),
        (None, None) => assert_eq!(newest(first, second), None),
    }
}

/// An update is staged in the spare slot of the default, which runs, with
/// all its tries, and the previous version is forgotten; any other slot
/// running refuses the stage. The running slot is never the one staged.
pub(crate) fn a_stage_goes_in_the_spare_slot(state: State, running: Slot) {
    match state.stage(running) {
        Ok(staged) => {
            assert_eq!(running, state.default);
            assert_eq!(staged.trying, Some(running.other()));
            assert_ne!(staged.trying, Some(running));
            assert_eq!((staged.tries, staged.previous), (TRIES, None));
            assert_eq!((staged.default, staged.boot), (state.default, state.boot));
            assert!(follows(state, staged));
        }
        Err(refused) => {
            assert_ne!(running, state.default);
            assert_eq!(refused, Refused::NotDefault(running));
        }
    }
}

/// Unstaging takes the update back and leaves the rest as it is, unless
/// nothing is staged or the update is the system running, which it never
/// unstages.
pub(crate) fn an_unstage_never_takes_the_running_slot(state: State, running: Slot) {
    match (state.trying, state.unstage(running)) {
        (None, result) => assert_eq!(result, Err(Refused::NothingStaged)),
        (Some(slot), result) if slot == running => assert_eq!(result, Err(Refused::Running(slot))),
        (Some(_), Ok(unstaged)) => {
            assert_eq!((unstaged.trying, unstaged.tries), (None, 0));
            assert_eq!(
                State { generation: state.generation, ..unstaged },
                State { trying: None, tries: 0, ..state }
            );
            assert!(follows(state, unstaged));
        }
        (Some(_), Err(_)) => panic!("an update staged in the other slot is unstaged"),
    }
}

/// A rollback is refused while an update is staged or when there is no
/// previous version other than the default; otherwise it swaps the default
/// and the previous version and keeps all else, and a second rollback
/// undoes it.
pub(crate) fn a_rollback_swaps_the_default_and_the_previous(state: State) {
    let result = state.rollback();
    if let Some(slot) = state.trying {
        assert_eq!(result, Err(Refused::Trying(slot)));
        return;
    }
    let Some(previous) = state.previous.filter(|&slot| slot != state.default) else {
        assert_eq!(result, Err(Refused::NoPrevious));
        return;
    };
    let rolled = result.unwrap();
    assert_eq!((rolled.default, rolled.previous), (previous, Some(state.default)));
    assert_ne!(Some(rolled.default), rolled.previous);
    assert_eq!((rolled.trying, rolled.tries, rolled.boot), (None, state.tries, state.boot));
    assert!(follows(state, rolled));
    let back = rolled.rollback().unwrap();
    assert_eq!(
        State { generation: state.generation, ..back },
        State { generation: state.generation, ..state }
    );
}

/// What a start's health check decides, and the state it writes, if the
/// state changes: an update that passes becomes the default, one that
/// fails gets no more tries, a fallback is acknowledged, and any other
/// start is seen through. No check adds a try, and a write is the next
/// generation of a state that differs.
pub(crate) fn a_check_decides_what_the_start_was(state: State, running: Slot, passed: bool) {
    let (written, checked) = state.checked(running, passed);
    let after = written.unwrap_or(state);
    if let Some(written) = written {
        assert!(follows(state, written));
        assert_ne!(State { generation: state.generation, ..written }, state);
    }
    assert!(after.tries <= state.tries);
    match (state.trying, checked) {
        (Some(slot), Checked::Proved(proved)) => {
            assert!(slot == running && passed && proved == slot);
            assert_eq!(
                (after.default, after.trying, after.tries, after.boot),
                (running, None, 0, None)
            );
            let before = (state.default != running).then_some(state.default);
            assert_eq!(after.previous, before);
        }
        (Some(slot), Checked::Failed(failed)) => {
            assert!(slot == running && !passed && failed == slot);
            assert_eq!(after.tries, 0);
            assert_eq!(
                (after.default, after.previous, after.trying),
                (state.default, state.previous, state.trying)
            );
            assert_eq!(after.boot, state.boot);
        }
        (Some(slot), Checked::FellBack(fell)) => {
            assert!(slot != running && state.tries == 0 && running == state.default);
            assert_eq!(fell, slot);
            assert_eq!((after.trying, after.tries, after.boot), (None, 0, None));
            assert_eq!((after.default, after.previous), (state.default, state.previous));
        }
        (_, Checked::Start) => {
            assert_eq!(after.boot, None);
            assert_eq!(
                (after.default, after.previous, after.trying, after.tries),
                (state.default, state.previous, state.trying, state.tries)
            );
        }
        (None, _) => panic!("a start with no update tried decided something else"),
    }
    // A start of the slot being tried never decides it was an ordinary
    // start, and the other slot's start never proves or fails the update.
    if let Some(slot) = state.trying {
        assert_eq!(matches!(checked, Checked::Proved(_) | Checked::Failed(_)), slot == running);
    } else {
        assert_eq!(checked, Checked::Start);
    }
}

/// A passing check of the update being tried makes the tried slot the
/// default.
pub(crate) fn a_passing_check_makes_the_tried_slot_the_default(state: State) {
    let Some(tried) = state.trying else { return };
    let (written, checked) = state.checked(tried, true);
    let after = written.unwrap_or(state);
    assert_eq!(checked, Checked::Proved(tried));
    assert_eq!(after.default, tried);
    assert_eq!(after.trying, None);
}

/// A slot forgotten is neither the previous version nor the update being
/// tried, loses the tries it had, and nothing else changes.
pub(crate) fn a_forgotten_slot_is_named_nowhere(state: State, slot: Slot) {
    let forgot = state.forget(slot);
    assert!(!forgot.names(slot));
    assert!(follows(state, forgot));
    assert_eq!((forgot.default, forgot.boot), (state.default, state.boot));
    if state.trying == Some(slot) {
        assert_eq!((forgot.trying, forgot.tries), (None, 0));
    } else {
        assert_eq!((forgot.trying, forgot.tries), (state.trying, state.tries));
    }
    if state.previous != Some(slot) {
        assert_eq!(forgot.previous, state.previous);
    }
    if !state.names(slot) {
        assert_eq!(State { generation: state.generation, ..forgot }, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_property_holds_for_every_state() {
        let all = every_state();
        assert_eq!(all.len(), 3 * 2 * 3 * 3 * 4 * 3);
        for state in all {
            starts_the_slot_being_tried_only_with_tries_left(state);
            a_start_takes_one_try(state);
            a_rollback_swaps_the_default_and_the_previous(state);
            a_passing_check_makes_the_tried_slot_the_default(state);
            for slot in [Slot::A, Slot::B] {
                a_chosen_start_takes_no_try(state, slot);
                a_stage_goes_in_the_spare_slot(state, slot);
                an_unstage_never_takes_the_running_slot(state, slot);
                a_forgotten_slot_is_named_nowhere(state, slot);
                for passed in [true, false] {
                    a_check_decides_what_the_start_was(state, slot, passed);
                }
            }
        }
    }

    #[test]
    fn the_newer_of_any_two_copies_is_taken() {
        let copies: Vec<Option<State>> =
            core::iter::once(None).chain(every_state().into_iter().map(Some)).collect();
        for &first in &copies {
            for &second in &copies {
                the_newer_copy_is_taken(first, second);
            }
        }
    }
}

#[cfg(kani)]
mod harnesses {
    use super::*;

    /// A slot, either.
    fn any_slot() -> Slot {
        if kani::any() { Slot::A } else { Slot::B }
    }

    /// A slot or none.
    fn any_maybe() -> Option<Slot> {
        if kani::any() { Some(any_slot()) } else { None }
    }

    /// A state with every value a copy can hold: a generation of 0, 1 or
    /// 2, and no more tries than [`TRIES`].
    fn any_state() -> State {
        State {
            generation: kani::any_where(|&generation: &u8| generation <= 2),
            default: any_slot(),
            previous: any_maybe(),
            trying: any_maybe(),
            tries: kani::any_where(|&tries: &u8| tries <= TRIES),
            boot: any_maybe(),
        }
    }

    /// A copy that is whole, or not there.
    fn any_copy() -> Option<State> {
        if kani::any() { Some(any_state()) } else { None }
    }

    #[kani::proof]
    fn the_slot_being_tried_starts_only_with_tries_left() {
        starts_the_slot_being_tried_only_with_tries_left(any_state());
    }

    #[kani::proof]
    fn a_start_takes_exactly_one_try() {
        a_start_takes_one_try(any_state());
    }

    #[kani::proof]
    fn a_chosen_start_takes_no_try_from_any_state() {
        a_chosen_start_takes_no_try(any_state(), any_slot());
    }

    #[kani::proof]
    fn the_newest_copy_is_the_one_a_write_followed() {
        the_newer_copy_is_taken(any_copy(), any_copy());
    }

    #[kani::proof]
    fn a_stage_never_goes_in_the_running_slot() {
        a_stage_goes_in_the_spare_slot(any_state(), any_slot());
    }

    #[kani::proof]
    fn an_unstage_never_takes_the_running_slot_back() {
        an_unstage_never_takes_the_running_slot(any_state(), any_slot());
    }

    #[kani::proof]
    fn a_rollback_swaps_and_undoes_itself() {
        a_rollback_swaps_the_default_and_the_previous(any_state());
    }

    #[kani::proof]
    fn a_check_decides_by_the_slot_that_ran() {
        a_check_decides_what_the_start_was(any_state(), any_slot(), kani::any());
    }

    #[kani::proof]
    fn a_passing_check_proves_the_tried_slot() {
        a_passing_check_makes_the_tried_slot_the_default(any_state());
    }

    #[kani::proof]
    fn a_forgotten_slot_is_gone_from_the_state() {
        a_forgotten_slot_is_named_nowhere(any_state(), any_slot());
    }
}
