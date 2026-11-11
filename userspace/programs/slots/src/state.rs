// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The slot state (ADR 0452): which slot starts, which one an update is
//! trying and with how many tries left, and whether the last start was
//! seen through. GRUB reads and writes it before each start, and the
//! system writes it when an update is staged, proves itself or fails, so
//! it lives where GRUB can write: in two files of each EFI system
//! partition, `boot/grub/state1` and `boot/grub/state2`, each a GRUB
//! environment block ([`crate::envblock`]) holding the whole state.
//!
//! Every writer, GRUB too, reads both copies, takes the newer whole one,
//! and writes the state that follows from it over the other, with the
//! next generation. A write that a power cut leaves half done spoils only
//! the copy it was writing: its check no longer matches its fields, and
//! the copy before it is still whole, so the state is the one from before
//! that write, never a mix. The generations count 0, 1, 2 and round
//! again, which tells which of two copies is newer without arithmetic,
//! which GRUB's scripts lack ([`newer`]). When GRUB cannot write, it
//! starts the default and leaves the try for a later start, so a try is
//! never started without being counted.
//!
//! GRUB's side is `image/grub/installed.cfg.in`, the menu on the EFI
//! system partition, and the slot's own `startup/menu.cfg`
//! (`image/grub/menu.cfg`), which must read and write exactly this
//! format; `tla/SlotState.tla` in this crate models both writers and the
//! power cuts between them, and `ob test tla` checks it.

use alloc::format;
use alloc::string::String;

use crate::envblock::{self, BLOCK};

/// The names of the copies' files beside GRUB's menu on each EFI system
/// partition.
pub const COPIES: [&str; 2] = ["boot/grub/state1", "boot/grub/state2"];

/// The most tries a new slot gets: GRUB takes one away before each start
/// of it, so that a hang, a panic and a power cut each cost one.
pub const TRIES: u8 = 3;

/// One of the two system slots, the subvolumes `@system.a` and
/// `@system.b` of the Dukovo root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    /// `@system.a`, the one an install makes.
    A,
    /// `@system.b`.
    B,
}

impl Slot {
    /// The slot's letter, as the state, the menu and the kernel's command
    /// line (`slot=a`) name it.
    #[must_use]
    pub const fn letter(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }

    /// The slot named by `letter`.
    #[must_use]
    pub fn named(letter: &str) -> Option<Slot> {
        match letter {
            "a" => Some(Slot::A),
            "b" => Some(Slot::B),
            _ => None,
        }
    }

    /// The other slot: the spare, when this one runs.
    #[must_use]
    pub const fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }

    /// The slot's subvolume in the root's top level.
    #[must_use]
    pub const fn subvolume(self) -> &'static str {
        match self {
            Slot::A => "@system.a",
            Slot::B => "@system.b",
        }
    }
}

/// The letter of `slot`, or nothing.
fn letter(slot: Option<Slot>) -> &'static str {
    slot.map_or("", Slot::letter)
}

/// The slot `text` names, which may be none; `Err` if it names neither.
fn maybe(text: &str) -> Result<Option<Slot>, ()> {
    if text.is_empty() { Ok(None) } else { Slot::named(text).map(Some).ok_or(()) }
}

/// The slot state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct State {
    /// Which write this is, 0, 1 or 2, counting round.
    pub generation: u8,
    /// The slot that starts unless an update is being tried: the version
    /// that last proved itself.
    pub default: Slot,
    /// The other slot, when it holds the version before the default,
    /// which the menu offers and a rollback goes back to.
    pub previous: Option<Slot>,
    /// The slot an update is being tried in, which starts while it has
    /// tries left.
    pub trying: Option<Slot>,
    /// The tries it has left. With none, the default starts, and it is
    /// told that the update fell back.
    pub tries: u8,
    /// The slot GRUB last started whose start was not yet seen through:
    /// the menu shows at the next start while it is set, so that a
    /// machine that stopped on its way up can be started otherwise.
    pub boot: Option<Slot>,
}

/// The variables of a copy, in the order a copy holds them: GRUB loads
/// and saves these by name.
pub const VARIABLES: [&str; 7] = [
    "slot_gen",
    "slot_default",
    "slot_previous",
    "slot_try",
    "slot_tries",
    "slot_boot",
    "slot_check",
];

impl State {
    /// The state an install leaves: slot A, the only one, started.
    #[must_use]
    pub const fn installed() -> State {
        State {
            generation: 0,
            default: Slot::A,
            previous: None,
            trying: None,
            tries: 0,
            boot: None,
        }
    }

    /// The copy's check: its fields again, in one value, which a copy
    /// that a write left half done does not match.
    #[must_use]
    pub fn check(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.generation,
            self.default.letter(),
            letter(self.previous),
            letter(self.trying),
            self.tries,
            letter(self.boot)
        )
    }

    /// The copy of this state, as a block for its file.
    ///
    /// # Panics
    ///
    /// Never: the state's values are a few short words.
    #[must_use]
    pub fn encode(&self) -> [u8; BLOCK] {
        let generation = format!("{}", self.generation);
        let tries = format!("{}", self.tries);
        let check = self.check();
        envblock::write(
            &["# Dukovo's slot state (ADR 0452): written by GRUB and sysupdate(8), not by hand."],
            &[
                (VARIABLES[0], &generation),
                (VARIABLES[1], self.default.letter()),
                (VARIABLES[2], letter(self.previous)),
                (VARIABLES[3], letter(self.trying)),
                (VARIABLES[4], &tries),
                (VARIABLES[5], letter(self.boot)),
                (VARIABLES[6], &check),
            ],
        )
        .expect("the slot state fits a block")
    }

    /// The state a copy's block holds, if the copy is whole: GRUB's
    /// signature, every field one it can hold, and the check matching
    /// them, as the menu's `slot_read` decides too.
    #[must_use]
    pub fn decode(block: &[u8]) -> Option<State> {
        let variables = envblock::parse(block)?;
        let field = |name| envblock::get(&variables, name);
        let generation = match field(VARIABLES[0])? {
            "0" => 0,
            "1" => 1,
            "2" => 2,
            _ => return None,
        };
        let default = Slot::named(field(VARIABLES[1])?)?;
        let previous = maybe(field(VARIABLES[2])?).ok()?;
        let trying = maybe(field(VARIABLES[3])?).ok()?;
        let tries = match field(VARIABLES[4])? {
            "0" => 0,
            "1" => 1,
            "2" => 2,
            "3" => 3,
            _ => return None,
        };
        let boot = maybe(field(VARIABLES[5])?).ok()?;
        let state = State { generation, default, previous, trying, tries, boot };
        (field(VARIABLES[6])? == state.check()).then_some(state)
    }

    /// The same state as the write after this one: the next generation.
    #[must_use]
    pub const fn next(self) -> State {
        State { generation: (self.generation + 1) % 3, ..self }
    }

    /// The slot GRUB starts, as the menu's Dukovo entry decides: the one
    /// being tried while it has tries left, else the default. With a
    /// second slot, the default is started as the fallback of a try that
    /// ran out, which the kernel's command line says (`fallback=`).
    #[must_use]
    pub fn starts(&self) -> (Slot, Option<Slot>) {
        match self.trying {
            Some(slot) if self.tries > 0 => (slot, None),
            Some(slot) => (self.default, Some(slot)),
            None => (self.default, None),
        }
    }

    /// What GRUB's Dukovo entry writes before it starts a slot: a try
    /// taken from the one being tried, and the start marked as not yet
    /// seen through. The model and the tests check the menu against it.
    #[must_use]
    pub fn started(self) -> State {
        let (slot, _) = self.starts();
        // The slot being tried starts exactly when it has tries left.
        let tries =
            if self.trying.is_some() && self.tries > 0 { self.tries - 1 } else { self.tries };
        State { tries, boot: Some(slot), ..self }.next()
    }

    /// What GRUB writes before it starts `slot` from an entry chosen by
    /// hand, the previous version or the default without the update being
    /// tried: the start marked as not yet seen through, and no try taken.
    #[must_use]
    pub fn chose(self, slot: Slot) -> State {
        State { boot: Some(slot), ..self }.next()
    }
}

/// Whether a copy of generation `a` was written after one of generation
/// `b`: the one after it, counting 0, 1, 2 and round. Two copies are only
/// ever one generation apart, or the same when an install wrote both.
#[must_use]
pub const fn newer(a: u8, b: u8) -> bool {
    matches!((a, b), (1, 0) | (2, 1) | (0, 2))
}

/// Which copy a state was read from, or is to be written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copy {
    /// `state1`.
    First,
    /// `state2`.
    Second,
}

impl Copy {
    /// The other copy.
    #[must_use]
    pub const fn other(self) -> Copy {
        match self {
            Copy::First => Copy::Second,
            Copy::Second => Copy::First,
        }
    }

    /// Its file's path on the EFI system partition.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Copy::First => COPIES[0],
            Copy::Second => COPIES[1],
        }
    }
}

/// The state two copies hold, as every writer reads it: the newer of the
/// two whole ones, or the whole one, and the copy it came from; the next
/// write goes over the other. The second is taken only when it is newer,
/// as the menu takes it. `None` if neither is whole.
#[must_use]
pub fn newest(first: Option<State>, second: Option<State>) -> Option<(State, Copy)> {
    match (first, second) {
        (Some(a), Some(b)) if newer(b.generation, a.generation) => Some((b, Copy::Second)),
        (Some(a), _) => Some((a, Copy::First)),
        (None, Some(b)) => Some((b, Copy::Second)),
        (None, None) => None,
    }
}

/// Why the system will not change the state as asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// An update is staged or being tried, so a rollback waits until it
    /// has proved itself, fallen back or been unstaged.
    Trying(Slot),
    /// The slot running is not the default, so the spare slot would be
    /// the default's: start the default first.
    NotDefault(Slot),
    /// There is no update staged to take back.
    NothingStaged,
    /// The update staged is the system running, so it can only prove
    /// itself or fall back.
    Running(Slot),
    /// There is no previous version to go back to.
    NoPrevious,
}

impl core::fmt::Display for Refused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refused::Trying(slot) => write!(
                f,
                "an update is staged in slot {}; unstage it, or let it prove itself or fall back, first",
                slot.letter()
            ),
            Refused::NotDefault(slot) => write!(
                f,
                "this system runs slot {}, which is not the default; start the default first",
                slot.letter()
            ),
            Refused::NothingStaged => f.write_str("no update is staged"),
            Refused::Running(slot) => write!(
                f,
                "the update in slot {} is the system running; it proves itself or falls back",
                slot.letter()
            ),
            Refused::NoPrevious => f.write_str("there is no previous version to go back to"),
        }
    }
}

/// What a start's health check decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checked {
    /// The update being tried in this slot passed, and is now the
    /// default; the slot before it is the previous version.
    Proved(Slot),
    /// The update being tried in this slot failed, and gets no more
    /// tries: the restart that follows starts the default.
    Failed(Slot),
    /// The update tried in the other slot ran out of tries or failed its
    /// check, and the default, which runs, is kept.
    FellBack(Slot),
    /// An ordinary start of the default or the previous version, seen
    /// through, whether its check passed or not.
    Start,
}

impl State {
    /// Stages the update just built in the spare slot, as `sysupdate
    /// stage` does, while `running`, the default, runs: it gets
    /// [`TRIES`] tries, and the version that was in the spare slot is no
    /// longer the previous one. An update staged before is replaced.
    ///
    /// # Errors
    ///
    /// If `running` is not the default, whose spare slot is the one an
    /// update goes in.
    pub fn stage(self, running: Slot) -> Result<State, Refused> {
        if running != self.default {
            return Err(Refused::NotDefault(running));
        }
        let spare = running.other();
        Ok(State { previous: None, trying: Some(spare), tries: TRIES, ..self }.next())
    }

    /// The state with `slot` neither the previous version nor the update
    /// being tried, as a stage writes over both copies before it removes
    /// what the spare slot held, so that no start goes to the slot while
    /// it is rebuilt, whatever a power cut leaves.
    #[must_use]
    pub fn forget(self, slot: Slot) -> State {
        let previous = self.previous.filter(|&previous| previous != slot);
        let (trying, tries) =
            if self.trying == Some(slot) { (None, 0) } else { (self.trying, self.tries) };
        State { previous, trying, tries, ..self }.next()
    }

    /// Whether the state names `slot` as the previous version or the
    /// update being tried, which [`State::forget`] undoes.
    #[must_use]
    pub fn names(&self, slot: Slot) -> bool {
        self.previous == Some(slot) || self.trying == Some(slot)
    }

    /// Takes a staged update back, as `sysupdate unstage` does: the
    /// default starts again, and keeps its settings.
    ///
    /// # Errors
    ///
    /// If nothing is staged, or the update is the system `running`.
    pub fn unstage(self, running: Slot) -> Result<State, Refused> {
        let Some(slot) = self.trying else { return Err(Refused::NothingStaged) };
        if slot == running {
            return Err(Refused::Running(slot));
        }
        Ok(State { trying: None, tries: 0, ..self }.next())
    }

    /// Makes the previous version the default from the next start, as
    /// `sysupdate rollback` does; the default becomes the previous one.
    ///
    /// # Errors
    ///
    /// If an update is staged or being tried, or there is no previous
    /// version.
    pub fn rollback(self) -> Result<State, Refused> {
        if let Some(slot) = self.trying {
            return Err(Refused::Trying(slot));
        }
        let Some(previous) = self.previous.filter(|&slot| slot != self.default) else {
            return Err(Refused::NoPrevious);
        };
        Ok(State { default: previous, previous: Some(self.default), ..self }.next())
    }

    /// The state once the health check of the start of `running` has
    /// run, and whether it `passed`: the state to write, if it changed,
    /// and what the check decided. An update tried there becomes the
    /// default if it passed, and gets no more tries if not; a fallback
    /// from one is acknowledged; any other start is seen through, so that
    /// the menu does not show at the next.
    #[must_use]
    pub fn checked(self, running: Slot, passed: bool) -> (Option<State>, Checked) {
        let (state, checked) = match self.trying {
            Some(slot) if slot == running && passed => {
                let previous = (self.default != running).then_some(self.default);
                let state = State {
                    default: running,
                    previous,
                    trying: None,
                    tries: 0,
                    boot: None,
                    ..self
                };
                (state, Checked::Proved(slot))
            }
            // The start stays marked as not seen through, so the menu
            // shows as the default starts.
            Some(slot) if slot == running => (State { tries: 0, ..self }, Checked::Failed(slot)),
            Some(slot) if self.tries == 0 && running == self.default => {
                (State { trying: None, boot: None, ..self }, Checked::FellBack(slot))
            }
            _ => (State { boot: None, ..self }, Checked::Start),
        };
        ((state != self).then(|| state.next()), checked)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    /// Every state a copy can hold.
    fn every_state() -> Vec<State> {
        let maybe = [None, Some(Slot::A), Some(Slot::B)];
        let mut all = Vec::new();
        for generation in 0..3 {
            for default in [Slot::A, Slot::B] {
                for previous in maybe {
                    for trying in maybe {
                        for tries in 0..=TRIES {
                            for boot in maybe {
                                all.push(State {
                                    generation,
                                    default,
                                    previous,
                                    trying,
                                    tries,
                                    boot,
                                });
                            }
                        }
                    }
                }
            }
        }
        all
    }

    #[test]
    fn every_state_reads_back_as_written() {
        let all = every_state();
        assert_eq!(all.len(), 3 * 2 * 3 * 3 * 4 * 3);
        for state in all {
            assert_eq!(State::decode(&state.encode()), Some(state), "{state:?}");
        }
    }

    #[test]
    fn a_copy_cut_short_or_changed_is_not_whole() {
        let state =
            State { trying: Some(Slot::B), tries: 2, boot: Some(Slot::B), ..State::installed() };
        let block = state.encode();
        // Cut at every byte: the start new, the rest as zeros, as a
        // write that stopped there and a disk that lost the rest.
        for cut in 0..block.len() {
            let mut torn = [0u8; BLOCK];
            torn[..cut].copy_from_slice(&block[..cut]);
            if let Some(read) = State::decode(&torn) {
                assert_eq!(read, state, "cut at {cut}");
            }
        }
        // The new copy's start over the old copy's end, at every byte:
        // either whole copy, never a mix of the two.
        let old = State::installed().next().encode();
        for cut in 0..block.len() {
            let mut torn = old;
            torn[..cut].copy_from_slice(&block[..cut]);
            if let Some(read) = State::decode(&torn) {
                assert!(read == state || read == State::installed().next(), "cut at {cut}");
            }
        }
        // A field changed, so that its check does not match.
        let text = String::from_utf8(block.to_vec()).unwrap();
        let changed = text.replacen("slot_tries=2", "slot_tries=3", 1);
        assert!(State::decode(changed.as_bytes()).is_none());
        let bad_value = text.replacen("slot_default=a", "slot_default=c", 1);
        assert!(State::decode(bad_value.as_bytes()).is_none());
        assert!(State::decode(&[b'#'; BLOCK]).is_none());
    }

    #[test]
    fn the_newer_generation_wins_and_the_other_copy_is_written_next() {
        for a in 0..3u8 {
            for b in 0..3u8 {
                let newer_count = u8::from(newer(a, b)) + u8::from(newer(b, a));
                assert!(newer_count <= 1, "{a} {b}");
                assert!(newer((a + 1) % 3, a));
                assert!(!newer(a, a));
            }
        }
        let first = State::installed();
        let second = first.next();
        assert_eq!(newest(Some(first), Some(second)), Some((second, Copy::Second)));
        assert_eq!(newest(Some(second.next()), Some(second)), Some((second.next(), Copy::First)));
        assert_eq!(newest(Some(first), Some(first)), Some((first, Copy::First)));
        assert_eq!(newest(None, Some(first)), Some((first, Copy::Second)));
        assert_eq!(newest(Some(first), None), Some((first, Copy::First)));
        assert_eq!(newest(None, None), None);
    }

    /// The state written once `state`'s start of `running` was checked.
    fn after(state: State, running: Slot, passed: bool) -> (State, Checked) {
        let (written, checked) = state.checked(running, passed);
        (written.unwrap_or(state), checked)
    }

    #[test]
    fn an_update_proves_itself_in_its_tries() {
        let installed = State::installed();
        let staged = installed.stage(Slot::A).unwrap();
        assert_eq!(staged.trying, Some(Slot::B));
        assert_eq!(staged.tries, TRIES);
        assert_eq!(staged.starts(), (Slot::B, None));
        // GRUB takes a try and starts it; it hangs, twice.
        let first = staged.started();
        assert_eq!((first.tries, first.boot), (2, Some(Slot::B)));
        let third = first.started().started();
        assert_eq!((third.tries, third.boot), (0, Some(Slot::B)));
        // The third start passes its health check.
        let (proved, how) = after(third, Slot::B, true);
        assert_eq!(how, Checked::Proved(Slot::B));
        assert_eq!(proved.default, Slot::B);
        assert_eq!(proved.previous, Some(Slot::A));
        assert_eq!((proved.trying, proved.tries, proved.boot), (None, 0, None));
        assert_eq!(proved.starts(), (Slot::B, None));
    }

    #[test]
    fn an_update_that_never_proves_itself_falls_back() {
        let staged = State::installed().stage(Slot::A).unwrap();
        let spent = staged.started().started().started();
        assert_eq!(spent.tries, 0);
        // The default starts, told of the fallback.
        assert_eq!(spent.starts(), (Slot::A, Some(Slot::B)));
        let started = spent.started();
        assert_eq!((started.tries, started.boot), (0, Some(Slot::A)));
        for passed in [true, false] {
            let (kept, how) = after(started, Slot::A, passed);
            assert_eq!(how, Checked::FellBack(Slot::B));
            assert_eq!((kept.default, kept.previous, kept.trying), (Slot::A, None, None));
            assert_eq!((kept.starts(), kept.boot), ((Slot::A, None), None));
        }
    }

    #[test]
    fn a_failed_health_check_spends_the_other_tries() {
        let started = State::installed().stage(Slot::A).unwrap().started();
        let (failed, how) = after(started, Slot::B, false);
        assert_eq!(how, Checked::Failed(Slot::B));
        assert_eq!((failed.tries, failed.boot), (0, Some(Slot::B)));
        assert_eq!(failed.starts(), (Slot::A, Some(Slot::B)));
        // Failing again, on the last try, writes nothing more.
        assert_eq!(failed.checked(Slot::B, false), (None, Checked::Failed(Slot::B)));
        // An ordinary start that fails is seen through all the same.
        let (seen, how) = after(State::installed().started(), Slot::A, false);
        assert_eq!((seen.boot, how), (None, Checked::Start));
    }

    #[test]
    fn staging_unstaging_and_rollback_are_refused_when_they_would_lose_a_start() {
        let installed = State::installed();
        assert_eq!(installed.unstage(Slot::A), Err(Refused::NothingStaged));
        assert_eq!(installed.rollback(), Err(Refused::NoPrevious));
        assert_eq!(installed.stage(Slot::B), Err(Refused::NotDefault(Slot::B)));
        let staged = installed.stage(Slot::A).unwrap();
        // Staged again before it starts: the newer build takes its place.
        assert!(staged.stage(Slot::A).is_ok());
        assert_eq!(staged.rollback(), Err(Refused::Trying(Slot::B)));
        let unstaged = staged.unstage(Slot::A).unwrap();
        assert_eq!((unstaged.trying, unstaged.tries), (None, 0));
        // Started: the update cannot unstage or stage over itself.
        let started = staged.started();
        assert_eq!(started.unstage(Slot::B), Err(Refused::Running(Slot::B)));
        assert_eq!(started.stage(Slot::B), Err(Refused::NotDefault(Slot::B)));
        // The default, started by hand meanwhile, can.
        assert!(started.unstage(Slot::A).is_ok());
        // Once it proved itself: rollback, which swaps the two.
        let (proved, _) = after(started, Slot::B, true);
        let back = proved.rollback().unwrap();
        assert_eq!((back.default, back.previous), (Slot::A, Some(Slot::B)));
        assert_eq!(back.rollback().unwrap().default, Slot::B);
        // After a fallback, another update can be staged, acknowledged or
        // not.
        let (fell, _) = after(staged.started(), Slot::B, false);
        let fell = fell.started();
        assert!(fell.stage(Slot::A).is_ok());
        let (kept, _) = after(fell, Slot::A, true);
        assert!(kept.stage(Slot::A).is_ok());
    }

    #[test]
    fn a_slot_forgotten_is_named_nowhere_and_the_rest_is_kept() {
        for state in every_state() {
            for slot in [Slot::A, Slot::B] {
                let forgot = state.forget(slot);
                assert!(!forgot.names(slot), "{state:?}");
                assert!(newer(forgot.generation, state.generation));
                assert_eq!((forgot.default, forgot.boot), (state.default, state.boot));
                if !state.names(slot) {
                    assert_eq!(State { generation: state.generation, ..forgot }, state);
                }
            }
        }
        // The spare holds the previous version until a stage forgets it.
        let (proved, _) =
            after(State::installed().stage(Slot::A).unwrap().started(), Slot::B, true);
        assert!(proved.names(Slot::A));
        assert_eq!(proved.forget(Slot::A).previous, None);
    }

    #[test]
    fn an_ordinary_start_is_seen_through_and_nothing_else_changes() {
        let started = State::installed().started();
        assert_eq!(started.boot, Some(Slot::A));
        let (seen, how) = after(started, Slot::A, true);
        assert_eq!(how, Checked::Start);
        assert_eq!(State { generation: 0, ..seen }, State::installed());
        assert_eq!(seen.checked(Slot::A, true), (None, Checked::Start));
        // The previous version started from the menu stays the previous.
        let (proved, _) = after(State::installed().stage(Slot::A).unwrap(), Slot::B, true);
        let menu = State { boot: Some(Slot::A), ..proved };
        let (seen, how) = after(menu, Slot::A, true);
        assert_eq!(how, Checked::Start);
        assert_eq!((seen.default, seen.previous), (Slot::B, Some(Slot::A)));
    }

    #[test]
    fn every_state_starts_a_slot_and_every_write_is_the_next_generation() {
        for state in every_state() {
            let (slot, fallback) = state.starts();
            assert!(fallback.is_none() || slot == state.default, "{state:?}");
            let started = state.started();
            assert!(newer(started.generation, state.generation));
            assert!(started.tries <= state.tries);
            for slot in [Slot::A, Slot::B] {
                let chose = state.chose(slot);
                assert!(newer(chose.generation, state.generation));
                assert_eq!(
                    State { generation: state.generation, boot: state.boot, ..chose },
                    state
                );
            }
            for running in [Slot::A, Slot::B] {
                for passed in [true, false] {
                    if let (Some(next), _) = state.checked(running, passed) {
                        assert!(newer(next.generation, state.generation));
                    }
                }
                for next in [state.stage(running), state.unstage(running), state.rollback()]
                    .into_iter()
                    .flatten()
                {
                    assert!(newer(next.generation, state.generation));
                }
            }
        }
    }
}
