// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a device said, from the events of its file (radix-input(2)).
//!
//! A device's file gives what one report of the device said as events,
//! and then an EV_SYN event with SYN_REPORT. [`Report`] takes the events
//! one by one and, at the end of each report, says what the report came
//! to: each key and button in the order they came, then how far the
//! device moved, where it points and how far its wheels turned, each
//! once, however many events said so.
//!
//! Three marks are not of a report. SYN_PAUSED: the desktop is no longer
//! in front, and every key counts as let go. SYN_RESUMED: it is in
//! front again. SYN_DROPPED: the reader fell behind and events were
//! lost, the rest of the report it was in among them; what is held is
//! let go, as nothing says which keys went up meanwhile.

use input::abi::{
    ABS_X, ABS_Y, BTN_LEFT, EV_ABS, EV_KEY, EV_REL, EV_SYN, InputEvent, REL_HWHEEL_HI_RES,
    REL_WHEEL_HI_RES, REL_X, REL_Y, REPEATED, SYN_DROPPED, SYN_PAUSED, SYN_REPORT, SYN_RESUMED,
};

/// The codes of a pointer's buttons: BTN_LEFT and the fifteen after it,
/// as Linux numbers them. The keys of keyboards are below and above.
const BUTTONS: core::ops::Range<u16> = BTN_LEFT..BTN_LEFT + 16;

/// The first code that is no key of a keyboard but a button of some
/// device (BTN_MISC), and the first after the buttons (KEY_OK).
const NOT_KEYS: core::ops::Range<u16> = 0x100..0x160;

/// One thing a report came to, or a mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A key of a keyboard went down or up.
    Key {
        /// Its Linux code.
        code: u16,
        /// Whether it went down.
        pressed: bool,
    },
    /// A pointer's button went down or up.
    Button {
        /// Its Linux code.
        code: u16,
        /// Whether it went down.
        pressed: bool,
    },
    /// The device moved, in its own counts: right and down are more.
    Moved {
        /// How far to the right.
        dx: i32,
        /// How far down.
        dy: i32,
    },
    /// Where the device points, within its axes' ranges; an axis the
    /// report did not name is where it was.
    Points {
        /// From the left edge.
        x: Option<i32>,
        /// From the top edge.
        y: Option<i32>,
    },
    /// The wheels turned, in 120ths of a notch: away from the person
    /// and to the right are more.
    Wheel {
        /// The wheel.
        vertical: i32,
        /// The horizontal wheel.
        horizontal: i32,
    },
    /// Everything held counts as let go: the desktop went to the back,
    /// or events were lost.
    LetGo,
}

/// The report a device's file is in the middle of.
#[derive(Clone, Debug, Default)]
pub struct Report {
    keys: Vec<Heard>,
    moved: (i32, i32),
    points: (Option<i32>, Option<i32>),
    wheel: (i32, i32),
}

impl Report {
    /// Takes `event`, the next of the device's file. At the end of a
    /// report, and for a mark, what was heard is added to `heard`.
    pub fn take(&mut self, event: &InputEvent, heard: &mut Vec<Heard>) {
        match (event.kind, event.code) {
            (EV_SYN, SYN_REPORT) => self.end(heard),
            (EV_SYN, SYN_PAUSED | SYN_DROPPED) => {
                *self = Report::default();
                heard.push(Heard::LetGo);
            }
            // What was held when the desktop went to the back was let
            // go then, and a key held since reaches no program now.
            (EV_SYN, SYN_RESUMED) => *self = Report::default(),
            // A key that is held repeats in the program that has it,
            // at the rate windowd tells every program.
            (EV_KEY, _) if event.value == i32::from(REPEATED) => {}
            (EV_KEY, code) if BUTTONS.contains(&code) => {
                self.keys.push(Heard::Button { code, pressed: event.value != 0 });
            }
            (EV_KEY, code) if !NOT_KEYS.contains(&code) => {
                self.keys.push(Heard::Key { code, pressed: event.value != 0 });
            }
            (EV_REL, REL_X) => self.moved.0 = self.moved.0.saturating_add(event.value),
            (EV_REL, REL_Y) => self.moved.1 = self.moved.1.saturating_add(event.value),
            (EV_REL, REL_WHEEL_HI_RES) => self.wheel.0 = self.wheel.0.saturating_add(event.value),
            (EV_REL, REL_HWHEEL_HI_RES) => {
                self.wheel.1 = self.wheel.1.saturating_add(event.value);
            }
            (EV_ABS, ABS_X) => self.points.0 = Some(event.value),
            (EV_ABS, ABS_Y) => self.points.1 = Some(event.value),
            // A tool of a tablet, a switch, and whatever a later kernel
            // adds: nothing a window is told in release 1.1.
            _ => {}
        }
    }

    /// The report is over: what it came to is added to `heard`.
    fn end(&mut self, heard: &mut Vec<Heard>) {
        let report = core::mem::take(self);
        // Where the pointer is comes before the buttons, so that a
        // press lands where the same report moved the pointer to.
        if report.moved != (0, 0) {
            heard.push(Heard::Moved { dx: report.moved.0, dy: report.moved.1 });
        }
        if report.points != (None, None) {
            heard.push(Heard::Points { x: report.points.0, y: report.points.1 });
        }
        heard.extend(report.keys);
        if report.wheel != (0, 0) {
            heard.push(Heard::Wheel { vertical: report.wheel.0, horizontal: report.wheel.1 });
        }
    }
}

#[cfg(test)]
mod tests {
    use input::abi::{BTN_RIGHT, KEY_A};

    use super::*;

    fn event(kind: u16, code: u16, value: i32) -> InputEvent {
        InputEvent { time_ns: 7, kind, code, value }
    }

    fn heard(events: &[InputEvent]) -> Vec<Heard> {
        let (mut report, mut heard) = (Report::default(), Vec::new());
        for event in events {
            report.take(event, &mut heard);
        }
        heard
    }

    const END: InputEvent = InputEvent { time_ns: 7, kind: EV_SYN, code: SYN_REPORT, value: 0 };

    #[test]
    fn a_report_is_heard_at_its_end_the_pointer_s_place_before_its_buttons() {
        let press = event(EV_KEY, BTN_LEFT, 1);
        let moves = [event(EV_REL, REL_X, 3), event(EV_REL, REL_Y, -2), event(EV_REL, REL_X, 4)];
        assert_eq!(heard(&[press, moves[0], moves[1], moves[2]]), []);
        assert_eq!(
            heard(&[press, moves[0], moves[1], moves[2], END]),
            [Heard::Moved { dx: 7, dy: -2 }, Heard::Button { code: BTN_LEFT, pressed: true }]
        );
        // A report of nothing says nothing.
        assert_eq!(heard(&[END, END]), []);
    }

    #[test]
    fn keys_buttons_wheels_and_positions_are_told_apart() {
        let said = heard(&[
            event(EV_KEY, KEY_A, 1),
            event(EV_KEY, BTN_RIGHT, 0),
            event(EV_KEY, 0x14a, 1),
            event(EV_ABS, ABS_Y, 900),
            event(EV_REL, REL_WHEEL_HI_RES, -120),
            event(EV_REL, REL_HWHEEL_HI_RES, 30),
            END,
        ]);
        assert_eq!(
            said,
            [
                Heard::Points { x: None, y: Some(900) },
                Heard::Key { code: KEY_A, pressed: true },
                Heard::Button { code: BTN_RIGHT, pressed: false },
                Heard::Wheel { vertical: -120, horizontal: 30 },
            ]
        );
        // A key above the buttons, as a remote control's, is a key,
        // and a key that repeats is heard once.
        let ok = heard(&[event(EV_KEY, 0x160, 1), END, event(EV_KEY, 0x160, 2), END]);
        assert_eq!(ok, [Heard::Key { code: 0x160, pressed: true }]);
    }

    #[test]
    fn a_pause_and_lost_events_let_go_and_drop_the_report_they_cut() {
        for mark in [SYN_PAUSED, SYN_DROPPED] {
            let said = heard(&[
                event(EV_KEY, KEY_A, 1),
                event(EV_REL, REL_X, 5),
                event(EV_SYN, mark, 0),
                event(EV_REL, REL_Y, 1),
                END,
            ]);
            assert_eq!(said, [Heard::LetGo, Heard::Moved { dx: 0, dy: 1 }]);
        }
        let resumed = heard(&[event(EV_REL, REL_X, 5), event(EV_SYN, SYN_RESUMED, 0), END]);
        assert_eq!(resumed, []);
    }
}
