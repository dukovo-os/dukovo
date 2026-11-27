// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::alloc::Global;
use alloc::rc::Rc;
use core::cell::Cell;

use radix_abi::input::{
    ABS_X, ABS_Y, BTN_LEFT, BUS_I8042, BUS_USB, EV_KEY, EV_REL, EV_SYN, KEY_A, KEY_B, REL_X, REL_Y,
};

use super::*;
use crate::device::{Axis, Few};

/// Counts how often its reader was woken.
#[derive(Clone, Debug, Default)]
struct Woken(Rc<Cell<u32>>);

impl Wake for Woken {
    fn wake(&self) {
        self.0.set(self.0.get() + 1);
    }
}

type TestHub = Hub<Woken, Global>;

/// A keyboard that sends a held key again itself, as a PS/2 one does.
fn keyboard(name: &str) -> About {
    let mut about = About::new(BUS_I8042, name, "ps2");
    about.repeats = true;
    about.keys =
        Keys::of(&[KEY_A, KEY_B, KEY_LEFTCTRL, KEY_RIGHTCTRL, KEY_LEFTALT, KEY_DELETE, KEY_KPDOT]);
    about
}

fn mouse() -> About {
    let mut about = About::new(BUS_USB, "A mouse", "usb 1-1");
    about.keys = Keys::of(&[BTN_LEFT]);
    about.moves = Few::of(&[REL_X, REL_Y]);
    about
}

/// A hub with a keyboard (device 0) and a mouse (device 1).
fn hub() -> TestHub {
    let mut hub = Hub::new_in(Global);
    assert_eq!(hub.add(&keyboard("A keyboard"), 0), Ok(0));
    assert_eq!(hub.add(&mouse(), 0), Ok(1));
    hub
}

/// Opens the reader `key` of `source` and returns its count of wake-ups.
fn open(hub: &mut TestHub, key: usize, source: Source) -> Woken {
    let woken = Woken::default();
    hub.open(key, source, woken.clone()).unwrap();
    woken
}

/// Everything the reader `key` of a device's file has to read, as kind,
/// code and value; a device that has gone has nothing more.
fn events(hub: &mut TestHub, key: usize) -> Vec<(u16, u16, i32)> {
    let mut all = Vec::new();
    let mut out = [0u8; 64];
    loop {
        let done = match hub.read(key, &mut out) {
            Ok(0) | Err(Refused::NoDevice) => return all,
            Ok(done) => done,
            Err(refused) => panic!("the read was refused: {refused:?}"),
        };
        for event in out[..done].chunks(16) {
            let kind = u16::from_le_bytes([event[8], event[9]]);
            let code = u16::from_le_bytes([event[10], event[11]]);
            all.push((kind, code, i32::from_le_bytes(event[12..].try_into().unwrap())));
        }
    }
}

/// Everything the reader `key` of _/devices/kbd_ has to read, as code
/// and value.
fn keys(hub: &mut TestHub, key: usize) -> Vec<(u16, u16)> {
    let mut all = Vec::new();
    let mut out = [0u8; 64];
    loop {
        let done = hub.read(key, &mut out).unwrap();
        if done == 0 {
            return all;
        }
        for event in out[..done].chunks(16) {
            let code = u16::from_le_bytes([event[8], event[9]]);
            all.push((code, u16::from_le_bytes([event[10], event[11]])));
        }
    }
}

const REPORT: (u16, u16, i32) = (EV_SYN, SYN_REPORT, 0);
const PAUSED: (u16, u16, i32) = (EV_SYN, SYN_PAUSED, 0);
const RESUMED: (u16, u16, i32) = (EV_SYN, SYN_RESUMED, 0);

fn down(code: u16) -> (u16, u16, i32) {
    (EV_KEY, code, 1)
}

fn up(code: u16) -> (u16, u16, i32) {
    (EV_KEY, code, 0)
}

#[test]
fn a_device_s_reader_reads_a_report_whole_with_its_time_and_is_woken_once() {
    let mut hub = hub();
    let woken = open(&mut hub, 1, Source::Device(1));
    let report = [Said::moved(REL_X, 3), Said::moved(REL_Y, -1), Said::key(BTN_LEFT, true)];
    assert!(!hub.frame(1, 777, &report));
    assert_eq!(woken.0.get(), 1);
    assert_eq!(hub.ready(1), Ready::Readable);
    let mut out = [0u8; 128];
    assert_eq!(hub.read(1, &mut out), Ok(64));
    for event in out[..64].chunks(16) {
        assert_eq!(u64::from_le_bytes(event[..8].try_into().unwrap()), 777);
    }
    hub.frame(1, 778, &report[..2]);
    assert_eq!(events(&mut hub, 1), [(EV_REL, REL_X, 3), (EV_REL, REL_Y, -1), REPORT]);
    assert_eq!(hub.ready(1), Ready::Nothing);
    // Another device's report is not its to read.
    hub.frame(0, 779, &[Said::key(KEY_A, true)]);
    assert_eq!(hub.ready(1), Ready::Nothing);
    assert_eq!(woken.0.get(), 2);
}

#[test]
fn what_changes_nothing_reaches_nobody_and_wakes_nobody() {
    let mut hub = hub();
    let device = open(&mut hub, 1, Source::Device(0));
    let merged = open(&mut hub, 2, Source::Keyboard);
    hub.frame(0, 1, &[Said::key(KEY_A, false), Said::moved(REL_X, 0)]);
    hub.frame(0, 2, &[]);
    hub.frame(9, 3, &[Said::key(KEY_A, true)]);
    assert_eq!((device.0.get(), merged.0.get()), (0, 0));
    assert_eq!(hub.read(1, &mut [0; 16]), Ok(0));
}

#[test]
fn the_merged_keyboard_reads_keys_of_every_keyboard_with_their_repeats() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Keyboard);
    open(&mut hub, 2, Source::Device(0));
    hub.frame(0, 1, &[Said::key(KEY_A, true)]);
    // The built-in keyboard sends a held key again; a button is no key.
    hub.frame(0, 3, &[Said::key(KEY_A, true)]);
    hub.frame(0, 4, &[Said { kind: EV_KEY, code: KEY_A, value: 2 }]);
    hub.frame(1, 5, &[Said::key(BTN_LEFT, true)]);
    assert_eq!(keys(&mut hub, 1), [(KEY_A, PRESSED), (KEY_A, REPEATED), (KEY_A, REPEATED)]);
    // The device's own file has no repeats, and nothing is repeated for
    // a keyboard that repeats its keys itself.
    assert_eq!(events(&mut hub, 2), [down(KEY_A), REPORT]);
    assert_eq!(hub.repeat_at(), None);
}

/// A hub with a keyboard that does not repeat its keys itself (device 0),
/// as a USB one, and a reader of _/devices/kbd_ with the key 1.
fn plain() -> TestHub {
    let mut hub = Hub::new_in(Global);
    let mut about = keyboard("A USB keyboard");
    about.repeats = false;
    assert_eq!(hub.add(&about, 0), Ok(0));
    open(&mut hub, 1, Source::Keyboard);
    hub
}

#[test]
fn the_hub_repeats_a_held_key_of_a_keyboard_that_does_not() {
    let mut hub = plain();
    let woken = open(&mut hub, 2, Source::Device(0));
    hub.frame(0, 1_000, &[Said::key(KEY_A, true)]);
    assert_eq!(hub.repeat_at(), Some(1_000 + REPEAT_DELAY_NS));
    // Not yet; and what the host of a virtual machine sends again for a
    // held key is not a repeat.
    assert_eq!(hub.tick(2_000), Some(1_000 + REPEAT_DELAY_NS));
    hub.frame(0, 3_000, &[Said::key(KEY_A, true)]);
    hub.frame(0, 4_000, &[Said { kind: EV_KEY, code: KEY_A, value: 2 }]);
    assert_eq!(keys(&mut hub, 1), [(KEY_A, PRESSED)]);
    // After the delay, and then every period.
    let first = 1_000 + REPEAT_DELAY_NS;
    assert_eq!(hub.tick(first), Some(first + REPEAT_PERIOD_NS));
    assert_eq!(hub.tick(first + REPEAT_PERIOD_NS), Some(first + 2 * REPEAT_PERIOD_NS));
    // Late, it goes on from now: no burst to catch up.
    let late = first + 10 * REPEAT_PERIOD_NS;
    assert_eq!(hub.tick(late), Some(late + 1));
    assert_eq!(keys(&mut hub, 1), [(KEY_A, REPEATED); 3]);
    // A second key takes over, and any key going up stops it.
    hub.frame(0, late + 5, &[Said::key(KEY_B, true)]);
    assert_eq!(hub.repeat_at(), Some(late + 5 + REPEAT_DELAY_NS));
    hub.frame(0, late + 6, &[Said::key(KEY_A, false)]);
    assert_eq!((hub.repeat_at(), hub.tick(u64::MAX)), (None, None));
    // The device's own file read no repeat, and was woken for none.
    assert_eq!(events(&mut hub, 2).len(), 6);
    assert_eq!(woken.0.get(), 3);
}

#[test]
fn a_key_stops_repeating_when_its_keyboard_goes_or_a_hold_begins() {
    let mut hub = plain();
    hub.frame(0, 1, &[Said::key(KEY_A, true)]);
    hub.remove(0, 2);
    assert_eq!(hub.repeat_at(), None);
    assert_eq!(keys(&mut hub, 1), [(KEY_A, PRESSED), (KEY_A, RELEASED)]);

    // Ctrl is held and repeats; the secure key ends that.
    let mut hub = plain();
    hub.attention_is(7, 0);
    let chord = [Said::key(KEY_LEFTCTRL, true), Said::key(KEY_LEFTALT, true)];
    hub.frame(0, 1, &chord);
    assert!(hub.repeat_at().is_some());
    assert!(hub.frame(0, 2, &[Said::key(KEY_DELETE, true)]));
    assert_eq!(hub.repeat_at(), None);
}

#[test]
fn the_merged_mouse_reads_one_motion_for_a_report() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Mouse);
    hub.frame(1, 5, &[Said::moved(REL_X, 4), Said::moved(REL_Y, 2), Said::key(BTN_LEFT, true)]);
    hub.frame(0, 6, &[Said::key(KEY_A, true)]);
    let mut out = [0u8; 80];
    assert_eq!(hub.read(1, &mut out), Ok(40));
    assert_eq!(out[8..16], [4, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(out[32], 1);
}

#[test]
fn a_tablet_s_position_reaches_both_files() {
    let mut hub = hub();
    let mut about = About::new(BUS_USB, "A tablet", "usb 1-3");
    about.add_axis(Axis { code: ABS_X, max: 32767, ..Axis::default() });
    about.add_axis(Axis { code: ABS_Y, max: 32767, ..Axis::default() });
    let tablet = hub.add(&about, 0).unwrap();
    open(&mut hub, 1, Source::Device(tablet));
    open(&mut hub, 2, Source::Mouse);
    hub.frame(tablet, 1, &[Said::at(ABS_X, 32767), Said::at(ABS_Y, 0)]);
    assert_eq!(events(&mut hub, 1), [(3, ABS_X, 32767), REPORT]);
    let mut out = [0u8; 40];
    assert_eq!(hub.read(2, &mut out), Ok(40));
    assert_eq!(out[24..28], 65535u32.to_le_bytes());
    let (device, heard) = hub.asked(1).unwrap();
    assert_eq!(device.axis(ABS_X, heard).map(|axis| axis.value), Some(32767));
    assert_eq!(hub.asked(2).unwrap_err(), Refused::NotThisFile);
}

#[test]
fn a_reader_tied_to_a_holder_hears_only_while_that_holder_is_in_front() {
    let mut hub = hub();
    hub.front_is(5, 0);
    open(&mut hub, 1, Source::Device(0));
    open(&mut hub, 2, Source::Device(0));
    assert_eq!(hub.tie(1, 5, 1), Ok(()));
    assert_eq!(hub.tie(2, 6, 1), Ok(()));
    // The holder in the back is told at once that it is not heard.
    assert_eq!(events(&mut hub, 2), [PAUSED]);
    hub.frame(0, 2, &[Said::key(KEY_A, true)]);
    assert_eq!(events(&mut hub, 1), [down(KEY_A), REPORT]);
    assert_eq!(events(&mut hub, 2), []);
    // It is told nothing is down, though A is.
    let mut state = [0u8; 96];
    let (device, heard) = hub.asked(2).unwrap();
    device.state(EV_KEY, heard, &mut state);
    assert_eq!(state, [0; 96]);
    // The front passes: one reader is paused and the other resumes, and
    // asks what is down.
    hub.front_is(6, 3);
    hub.frame(0, 4, &[Said::key(KEY_B, true)]);
    assert_eq!(events(&mut hub, 1), [PAUSED]);
    assert_eq!(events(&mut hub, 2), [RESUMED, down(KEY_B), REPORT]);
    let (device, heard) = hub.asked(2).unwrap();
    device.state(EV_KEY, heard, &mut state);
    assert_eq!(state[usize::from(KEY_A) / 8], 1 << (KEY_A % 8));
    assert_eq!(state[usize::from(KEY_B) / 8], 1 << (KEY_B % 8));
    // The same front told again changes nothing.
    hub.front_is(6, 5);
    assert_eq!(events(&mut hub, 2), []);
}

#[test]
fn while_nobody_is_in_front_tied_readers_hear_nothing_and_nothing_is_kept() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Device(0));
    open(&mut hub, 2, Source::Device(0));
    hub.tie(1, 5, 0).unwrap();
    hub.frame(0, 1, &[Said::key(KEY_A, true)]);
    hub.frame(0, 2, &[Said::key(KEY_A, false)]);
    hub.front_is(5, 3);
    // Nothing of what was typed meanwhile; a reader tied to no holder
    // read it all.
    assert_eq!(events(&mut hub, 1), [PAUSED, RESUMED]);
    assert_eq!(events(&mut hub, 2), [down(KEY_A), REPORT, up(KEY_A), REPORT]);
}

#[test]
fn a_reader_tied_to_a_holder_that_ended_hears_nothing_more() {
    let mut hub = hub();
    hub.front_is(5, 0);
    open(&mut hub, 1, Source::Device(0));
    hub.tie(1, 5, 0).unwrap();
    // The holder ends: nobody is in front, and its number is free.
    hub.front_is(0, 1);
    hub.forget(5);
    // A new holder is given the old number and comes to the front.
    hub.front_is(5, 2);
    hub.frame(0, 3, &[Said::key(KEY_A, true)]);
    assert_eq!(events(&mut hub, 1), [PAUSED]);
    assert_eq!(hub.tie(1, 5, 4), Err(Refused::Tied));
}

#[test]
fn tying_forgets_what_came_before_and_is_done_once() {
    let mut hub = hub();
    hub.front_is(5, 0);
    open(&mut hub, 1, Source::Device(0));
    open(&mut hub, 2, Source::Keyboard);
    open(&mut hub, 3, Source::Control);
    hub.frame(0, 1, &[Said::key(KEY_A, true)]);
    assert_eq!(hub.tie(1, 5, 2), Ok(()));
    assert_eq!(hub.tie(2, 5, 2), Ok(()));
    assert_eq!(events(&mut hub, 1), []);
    assert_eq!(keys(&mut hub, 2), []);
    assert_eq!(hub.tie(1, 6, 3), Err(Refused::Tied));
    assert_eq!(hub.tie(3, 5, 3), Err(Refused::NotThisFile));
    assert_eq!(hub.tie(9, 5, 3), Err(Refused::NoReader));
    // Still tied to the first holder.
    hub.frame(0, 4, &[Said::key(KEY_A, false)]);
    assert_eq!(events(&mut hub, 1), [up(KEY_A), REPORT]);
}

#[test]
fn a_paused_reader_of_the_merged_files_reads_everything_let_go() {
    let mut hub = hub();
    hub.front_is(5, 0);
    open(&mut hub, 1, Source::Keyboard);
    open(&mut hub, 2, Source::Mouse);
    hub.tie(1, 5, 0).unwrap();
    hub.tie(2, 5, 0).unwrap();
    hub.frame(0, 1, &[Said::key(KEY_A, true), Said::key(KEY_LEFTCTRL, true)]);
    hub.frame(1, 2, &[Said::key(BTN_LEFT, true)]);
    assert_eq!(keys(&mut hub, 1), [(KEY_A, PRESSED), (KEY_LEFTCTRL, PRESSED)]);
    let mut out = [0u8; 80];
    assert_eq!(hub.read(2, &mut out), Ok(40));
    hub.front_is(6, 3);
    // The keys in the order of their codes; the button is no key.
    assert_eq!(keys(&mut hub, 1), [(KEY_LEFTCTRL, RELEASED), (KEY_A, RELEASED)]);
    assert_eq!(hub.read(2, &mut out), Ok(40));
    assert_eq!(out[32..36], [0; 4]);
    // What goes up while it is not heard, it does not read.
    hub.frame(0, 4, &[Said::key(KEY_A, false)]);
    hub.front_is(5, 5);
    assert_eq!(keys(&mut hub, 1), []);
}

#[test]
fn a_key_down_when_the_front_passes_types_nothing_into_the_next_reader() {
    let mut hub = plain();
    hub.front_is(5, 0);
    open(&mut hub, 2, Source::Keyboard);
    open(&mut hub, 3, Source::Keyboard);
    hub.tie(2, 5, 0).unwrap();
    hub.tie(3, 6, 0).unwrap();
    // A goes down for the holder in front, and the front passes with it
    // still down: the one that read it go down reads it let go.
    hub.frame(0, 1_000, &[Said::key(KEY_A, true)]);
    hub.front_is(6, 2_000);
    assert_eq!(keys(&mut hub, 2), [(KEY_A, PRESSED), (KEY_A, RELEASED)]);
    // The next reads neither its repeats nor its release.
    let woken = hub.readers[2].waker.clone();
    let repeats = 1_000 + REPEAT_DELAY_NS;
    assert_eq!(hub.tick(repeats), Some(repeats + REPEAT_PERIOD_NS));
    hub.frame(0, repeats + 1, &[Said::key(KEY_A, false)]);
    assert_eq!(keys(&mut hub, 3), []);
    assert_eq!(woken.0.get(), 0, "and is woken for none of it");
    // A key that goes down from here is its own, and so is A again.
    hub.frame(0, repeats + 2, &[Said::key(KEY_B, true), Said::key(KEY_A, true)]);
    hub.frame(0, repeats + 3, &[Said::key(KEY_A, false)]);
    assert_eq!(keys(&mut hub, 3), [(KEY_B, PRESSED), (KEY_A, PRESSED), (KEY_A, RELEASED)]);
    // The front comes back with B down: the first reader, which read
    // nothing of B, reads nothing of it now; the other reads it let go.
    hub.front_is(5, repeats + 4);
    hub.frame(0, repeats + 5, &[Said::key(KEY_B, false)]);
    assert_eq!(keys(&mut hub, 2), []);
    assert_eq!(keys(&mut hub, 3), [(KEY_B, RELEASED)]);
    // The reader tied to none read every key as it was.
    assert_eq!(keys(&mut hub, 1).len(), 7);
}

#[test]
fn a_key_down_when_a_file_is_opened_or_tied_is_not_that_reader_s() {
    let mut hub = plain();
    hub.front_is(5, 0);
    hub.frame(0, 1, &[Said::key(KEY_A, true), Said::key(KEY_B, true)]);
    // Opened with A and B down, as by the program that Enter started.
    open(&mut hub, 2, Source::Keyboard);
    hub.frame(0, 2, &[Said::key(KEY_A, false)]);
    assert_eq!(keys(&mut hub, 2), []);
    // Tied to the holder in front, with B still down.
    hub.tie(2, 5, 3).unwrap();
    hub.frame(0, 4, &[Said::key(KEY_B, false), Said::key(KEY_A, true)]);
    assert_eq!(keys(&mut hub, 2), [(KEY_A, PRESSED)]);
    // A reader that stops being heard is let go of what it read go down,
    // in the order of the codes, and of nothing else.
    hub.frame(0, 5, &[Said::key(KEY_B, true)]);
    open(&mut hub, 3, Source::Keyboard);
    hub.tie(3, 5, 6).unwrap();
    hub.front_is(6, 7);
    assert_eq!(keys(&mut hub, 2), [(KEY_B, PRESSED), (KEY_A, RELEASED), (KEY_B, RELEASED)]);
    assert_eq!(keys(&mut hub, 3), []);
}

#[test]
fn the_attention_holder_reads_nothing_of_the_keys_that_began_its_hold() {
    let mut hub = plain();
    hub.attention_is(1, 0);
    hub.front_is(2, 0);
    open(&mut hub, 2, Source::Keyboard);
    hub.tie(2, 1, 0).unwrap();
    let chord = [Said::key(KEY_LEFTCTRL, true), Said::key(KEY_LEFTALT, true)];
    hub.frame(0, 1, &chord);
    assert!(hub.frame(0, 2, &[Said::key(KEY_DELETE, true)]));
    // The holder of the attention file is heard from here, with Ctrl and
    // Alt down that it never read go down.
    let all_up = [KEY_DELETE, KEY_LEFTALT, KEY_LEFTCTRL].map(|code| Said::key(code, false));
    hub.frame(0, 3, &all_up);
    hub.frame(0, 4, &[Said::key(KEY_A, true)]);
    assert_eq!(keys(&mut hub, 2), [(KEY_A, PRESSED)]);
}

/// A hub where holder 1 has the attention file and holder 2 is in front,
/// with a second keyboard (device 2), and four readers of the first
/// keyboard: 1 tied to the attention holder, 2 tied to the front, 3 tied
/// to none, and 4 the merged file, tied to none.
fn guarded() -> TestHub {
    let mut hub = hub();
    assert_eq!(hub.add(&keyboard("Another"), 0), Ok(2));
    hub.attention_is(1, 0);
    hub.front_is(2, 0);
    open(&mut hub, 1, Source::Device(0));
    open(&mut hub, 2, Source::Device(0));
    open(&mut hub, 3, Source::Device(0));
    open(&mut hub, 4, Source::Keyboard);
    hub.tie(1, 1, 0).unwrap();
    hub.tie(2, 2, 0).unwrap();
    assert_eq!(events(&mut hub, 1), [PAUSED]);
    hub
}

#[test]
fn the_secure_key_pauses_everyone_but_the_attention_holder_until_released() {
    let mut hub = guarded();
    // Ctrl on one keyboard, Alt on another, Delete on the first.
    assert!(!hub.frame(0, 1, &[Said::key(KEY_LEFTCTRL, true)]));
    assert!(!hub.frame(2, 2, &[Said::key(KEY_LEFTALT, true)]));
    assert!(!hub.is_held());
    assert!(hub.frame(0, 3, &[Said::key(KEY_DELETE, true)]));
    assert!(hub.is_held());
    // Typed during the hold, with the front still the other holder's.
    hub.frame(0, 4, &[Said::key(KEY_A, true)]);
    hub.frame(0, 5, &[Said::key(KEY_DELETE, false), Said::key(KEY_A, false)]);
    // Nobody read the Delete go down or up. The front's reader and the
    // one tied to none were paused at the key.
    assert_eq!(events(&mut hub, 2), [down(KEY_LEFTCTRL), REPORT, PAUSED]);
    assert_eq!(events(&mut hub, 3), [down(KEY_LEFTCTRL), REPORT, PAUSED]);
    assert_eq!(
        keys(&mut hub, 4),
        [
            (KEY_LEFTCTRL, PRESSED),
            (KEY_LEFTALT, PRESSED),
            (KEY_LEFTCTRL, RELEASED),
            (KEY_LEFTALT, RELEASED)
        ]
    );
    assert_eq!(events(&mut hub, 1), [RESUMED, down(KEY_A), REPORT, up(KEY_A), REPORT]);
    // The display layer says the attention holder is in front: nothing
    // changes for anyone.
    hub.front_is(1, 6);
    assert_eq!(events(&mut hub, 2), []);
    // The hold ends and the front is given back.
    hub.front_is(2, 7);
    hub.release(7);
    assert!(!hub.is_held());
    hub.frame(0, 8, &[Said::key(KEY_B, true)]);
    assert_eq!(events(&mut hub, 1), [PAUSED]);
    assert_eq!(events(&mut hub, 2), [RESUMED, down(KEY_B), REPORT]);
    assert_eq!(events(&mut hub, 3), [RESUMED, down(KEY_B), REPORT]);
    assert_eq!(keys(&mut hub, 4), [(KEY_B, PRESSED)]);
}

#[test]
fn what_a_report_said_before_the_secure_key_is_for_those_heard_until_then() {
    let mut hub = guarded();
    hub.frame(0, 1, &[Said::key(KEY_RIGHTCTRL, true), Said::key(KEY_LEFTALT, true)]);
    events(&mut hub, 2);
    let report = [Said::key(KEY_A, true), Said::key(KEY_KPDOT, true), Said::key(KEY_B, true)];
    assert!(hub.frame(0, 2, &report));
    assert_eq!(events(&mut hub, 2), [down(KEY_A), REPORT, PAUSED]);
    assert_eq!(events(&mut hub, 1), [RESUMED, down(KEY_B), REPORT]);
    // A held Delete that the keyboard sends again is not read either.
    assert!(!hub.frame(0, 3, &[Said::key(KEY_KPDOT, true)]));
    assert_eq!(events(&mut hub, 1), []);
    // The key again during the hold is three plain keys to the holder.
    hub.frame(0, 4, &[Said::key(KEY_KPDOT, false)]);
    assert!(!hub.frame(0, 5, &[Said::key(KEY_KPDOT, true)]));
    assert_eq!(events(&mut hub, 1), [down(KEY_KPDOT), REPORT]);
}

#[test]
fn without_an_attention_holder_or_a_chord_delete_is_a_plain_key() {
    let mut hub = guarded();
    // No Ctrl and Alt: a plain Delete.
    assert!(!hub.frame(0, 1, &[Said::key(KEY_DELETE, true), Said::key(KEY_DELETE, false)]));
    assert_eq!(events(&mut hub, 3), [down(KEY_DELETE), up(KEY_DELETE), REPORT]);
    // The chord, with nobody holding the attention file.
    hub.attention_is(0, 2);
    hub.frame(0, 3, &[Said::key(KEY_LEFTCTRL, true), Said::key(KEY_LEFTALT, true)]);
    assert!(!hub.frame(0, 4, &[Said::key(KEY_DELETE, true)]));
    assert!(!hub.is_held());
    assert_eq!(events(&mut hub, 3).last(), Some(&REPORT));
}

#[test]
fn a_hold_ends_when_its_holder_gives_the_attention_file_up() {
    let mut hub = guarded();
    hub.frame(0, 1, &[Said::key(KEY_LEFTCTRL, true), Said::key(KEY_LEFTALT, true)]);
    assert!(hub.frame(0, 2, &[Said::key(KEY_DELETE, true)]));
    events(&mut hub, 2);
    hub.attention_is(0, 3);
    assert!(!hub.is_held());
    assert_eq!(events(&mut hub, 2), [RESUMED]);
    // A reader opened during a hold is not heard until it ends.
    hub.attention_is(1, 4);
    hub.frame(0, 5, &[Said::key(KEY_DELETE, false)]);
    assert!(hub.frame(0, 6, &[Said::key(KEY_DELETE, true)]));
    open(&mut hub, 7, Source::Device(0));
    hub.frame(0, 7, &[Said::key(KEY_A, true)]);
    assert_eq!(events(&mut hub, 7), []);
    hub.release(8);
    assert_eq!(events(&mut hub, 7), [RESUMED]);
}

#[test]
fn devices_come_and_go_with_numbers_never_used_twice() {
    let mut hub = hub();
    let control = open(&mut hub, 1, Source::Control);
    open(&mut hub, 2, Source::Device(1));
    open(&mut hub, 3, Source::Mouse);
    assert_eq!(hub.open(4, Source::Device(7), Woken::default()), Err(Refused::NoDevice));
    hub.frame(1, 1, &[Said::key(BTN_LEFT, true)]);
    hub.remove(1, 2);
    assert_eq!(hub.add(&mouse(), 3), Ok(2));
    // Whole numbers only: the last three bytes of room stay as they were.
    let mut numbers = [9u8; 15];
    assert_eq!(hub.list(&mut numbers), 2);
    assert_eq!(numbers, [0, 0, 0, 0, 2, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9]);
    assert_eq!(hub.list(&mut numbers[..7]), 2);
    assert_eq!(control.0.get(), 2);
    let mut out = [0u8; 80];
    assert_eq!(hub.read(1, &mut out), Ok(32));
    assert_eq!(out[8..16], [1, 0, 0, 0, DEVICE_REMOVED as u8, 0, 0, 0]);
    assert_eq!(out[24..32], [2, 0, 0, 0, DEVICE_ADDED as u8, 0, 0, 0]);
    // What it held down went up before it went, for every reader.
    assert_eq!(events(&mut hub, 2)[2..], [up(BTN_LEFT), REPORT]);
    assert_eq!(hub.read(3, &mut out), Ok(80));
    assert_eq!(out[40 + 32], 0);
    // Its reader has read everything and finds it gone.
    assert_eq!(hub.ready(2), Ready::Gone);
    assert_eq!(hub.read(2, &mut out), Err(Refused::NoDevice));
    assert_eq!(hub.asked(2).unwrap_err(), Refused::NoDevice);
    assert_eq!(hub.open(4, Source::Device(1), Woken::default()), Err(Refused::NoDevice));
    hub.remove(1, 4);
    hub.close(2);
    hub.close(2);
    assert_eq!(hub.readers(), 2);
    assert_eq!(hub.ready(2), Ready::Nothing);
}

#[test]
fn a_mouse_at_8000_reports_a_second_loses_none_while_its_reader_stalls_150_ms() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Device(1));
    assert_eq!(hub.kept(1), 0);
    hub.frame(1, 0, &[Said::moved(REL_X, 1), Said::moved(REL_Y, 1)]);
    // An idle reader's ring is 4 KiB.
    assert_eq!(hub.kept(1), 4096);
    for report in 1..1200 {
        hub.frame(1, report * 125_000, &[Said::moved(REL_X, 1), Said::moved(REL_Y, 1)]);
    }
    assert_eq!(hub.kept(1), 65536);
    let read = events(&mut hub, 1);
    assert_eq!(read.len(), 3600);
    assert!(
        read.chunks(3).all(|report| report == [(EV_REL, REL_X, 1), (EV_REL, REL_Y, 1), REPORT])
    );
    // Read empty, the ring gives its memory back.
    assert_eq!(hub.kept(1), 0);
}

#[test]
fn a_reader_that_falls_too_far_behind_is_told_and_no_other_reader_loses() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Device(1));
    open(&mut hub, 2, Source::Device(1));
    let mut read = 0;
    for report in 0..2000 {
        hub.frame(1, report, &[Said::moved(REL_X, 1), Said::moved(REL_Y, 1)]);
        read += events(&mut hub, 2).len();
    }
    assert_eq!(read, 6000);
    // The slow one reads the mark, then whole reports from the one that
    // did not fit on.
    let slow = events(&mut hub, 1);
    assert_eq!(slow[0], (EV_SYN, SYN_DROPPED, 0));
    assert_eq!(slow[1], (EV_REL, REL_X, 1));
    assert_eq!((slow.len() - 1) % 3, 0);
    assert!(slow.len() < 4097);
}

#[test]
fn the_merged_files_and_the_control_file_say_once_that_they_lost_records() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Keyboard);
    open(&mut hub, 2, Source::Control);
    for press in 0..300 {
        hub.frame(0, press, &[Said::key(KEY_A, true)]);
        hub.frame(0, press, &[Said::key(KEY_A, false)]);
    }
    let mut out = [0u8; 16];
    assert_eq!(hub.read(1, &mut out), Err(Refused::Lost));
    assert_eq!(hub.read(1, &mut out), Ok(16));
    assert_eq!(keys(&mut hub, 1).len(), 511);
    for _ in 0..70 {
        let number = hub.add(&mouse(), 9).unwrap();
        hub.remove(number, 9);
    }
    assert_eq!(hub.read(2, &mut out), Ok(16));
    assert_eq!(out[8..], [0, 0, 0, 0, DEVICES_LOST as u8, 0, 0, 0]);
    assert_eq!(hub.read(2, &mut out), Ok(16));
    assert_eq!(out[12], DEVICE_ADDED as u8);
}

#[test]
fn a_read_that_has_records_already_leaves_a_loss_for_the_next_read() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Keyboard);
    hub.frame(0, 1, &[Said::key(KEY_A, true)]);
    let mut out = [0u8; 16];
    // The reader has taken a record of this read when its ring fills.
    assert_eq!(hub.read(1, &mut out), Ok(16));
    for press in 0..300 {
        hub.frame(0, press, &[Said::key(KEY_A, false)]);
        hub.frame(0, press, &[Said::key(KEY_A, true)]);
    }
    // The merged file reads on to nothing, and its next read says so.
    assert_eq!(hub.read_on(1, &mut out), Ok(0));
    assert_eq!(hub.read(1, &mut out), Err(Refused::Lost));
    assert_eq!(hub.read_on(1, &mut out), Ok(16));
}

#[test]
fn a_buffer_too_small_for_one_record_is_refused_and_loses_nothing() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Device(0));
    open(&mut hub, 2, Source::Mouse);
    hub.frame(0, 1, &[Said::key(KEY_A, true)]);
    assert_eq!(hub.read(1, &mut [0; 15]), Err(Refused::TooSmall));
    assert_eq!(hub.read(2, &mut [0; 39]), Err(Refused::TooSmall));
    assert_eq!(hub.read(9, &mut [0; 64]), Err(Refused::NoReader));
    assert_eq!(events(&mut hub, 1), [down(KEY_A), REPORT]);
}

#[test]
fn a_report_that_says_more_than_a_frame_holds_is_read_as_several() {
    let mut hub = hub();
    open(&mut hub, 1, Source::Device(1));
    let mut report = [Said::moved(REL_X, 1); 2 * FRAME_MAX + 1];
    report[2 * FRAME_MAX] = Said::key(BTN_LEFT, true);
    hub.frame(1, 1, &report);
    let read = events(&mut hub, 1);
    assert_eq!(read.len(), 2 * FRAME_MAX + 1 + 3);
    assert_eq!(read[FRAME_MAX], REPORT);
    assert_eq!(read[read.len() - 2..], [down(BTN_LEFT), REPORT]);
}

#[test]
fn there_is_a_most_of_devices_and_of_readers() {
    let mut hub: TestHub = Hub::new_in(Global);
    for _ in 0..DEVICES_MAX {
        hub.add(&mouse(), 0).unwrap();
    }
    assert_eq!(hub.add(&mouse(), 0), Err(Refused::NoRoom));
    for key in 0..READERS_MAX {
        hub.open(key, Source::Keyboard, Woken::default()).unwrap();
    }
    assert_eq!(hub.open(READERS_MAX, Source::Keyboard, Woken::default()), Err(Refused::NoRoom));
    assert_eq!(hub.devices().len(), DEVICES_MAX);
}
