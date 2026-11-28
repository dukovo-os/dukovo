// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! When a screen's next frame is drawn (ADR 0714).
//!
//! A change shows at its pipe's next beat, and each beat's event says
//! the last moment a change can be asked for and still show at the beat
//! after it (radix-display(2)'s `deadline_ns`). windowd draws as late as
//! that allows: at the deadline less what its last frames took to draw,
//! so that a program has nearly the whole period to draw its own frame
//! and still be in windowd's.
//!
//! A screen is in one of three states. It is still: nothing changed, no
//! beat is asked for, and windowd is not woken for it. It is due: what
//! it shows changed, and it is drawn at [`Pace::due`]. Or it waits: a
//! frame was asked for, and nothing more is drawn until the beat's event
//! says it shows, since the card may still read the picture before
//! that.

/// How many of the last frames' drawing times the next is planned from.
const REMEMBERED: usize = 4;

/// Added to the longest of them: the way from the timer to the first
/// pixel and from the last to the layer.
const SLACK_NS: u64 = 500_000;

/// What a screen's frame waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Waits {
    /// Nothing: no frame is on its way.
    #[default]
    Nothing,
    /// The beat a change shows at, or the beat that was asked for.
    Beat,
}

/// The pacing of one screen's frames. Times are nanoseconds on the
/// monotonic clock, which stamps the layer's events.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pace {
    /// What the screen shows changed since its last frame was drawn.
    stale: bool,
    waits: Waits,
    /// The last moment a change can be asked for and show at the next
    /// beat, as the last beat's event gave it.
    deadline_ns: Option<u64>,
    /// How long each of the last frames took to draw.
    took_ns: [u64; REMEMBERED],
}

impl Pace {
    /// What the screen shows changed: a frame is due.
    pub fn stale(&mut self) {
        self.stale = true;
    }

    /// When the next frame is to be drawn, if one is due and none is on
    /// its way: at the deadline less the drawing time, or at `now_ns` if
    /// that has passed or no beat has said a deadline, as for a screen
    /// that was still.
    pub fn due(&self, now_ns: u64) -> Option<u64> {
        if !self.stale || self.waits == Waits::Beat {
            return None;
        }
        let late = self.deadline_ns.map_or(0, |deadline| deadline.saturating_sub(self.budget()));
        Some(late.max(now_ns))
    }

    /// What drawing a frame is planned to take: the longest of the last
    /// few, and [`SLACK_NS`].
    fn budget(&self) -> u64 {
        self.took_ns.iter().copied().max().unwrap_or(0).saturating_add(SLACK_NS)
    }

    /// A frame was drawn in `took_ns` and a change or a beat asked for:
    /// the screen waits for the beat's event.
    pub fn drawn(&mut self, took_ns: u64) {
        self.took_ns.rotate_right(1);
        self.took_ns[0] = took_ns;
        (self.stale, self.waits) = (false, Waits::Beat);
        // The deadline was this beat's. The next one comes with the
        // beat's event.
        self.deadline_ns = None;
    }

    /// A frame was drawn and nothing had changed: the screen is still.
    pub fn unchanged(&mut self) {
        self.stale = false;
    }

    /// The beat's event came: the frame shows, and the next change is
    /// due by `deadline_ns`.
    pub fn shown(&mut self, deadline_ns: u64) {
        (self.waits, self.deadline_ns) = (Waits::Nothing, Some(deadline_ns));
    }

    /// The screens went to another program or came back, or the layer's
    /// events were lost: no frame is on its way, and no deadline holds.
    pub fn reset(&mut self) {
        (self.waits, self.deadline_ns) = (Waits::Nothing, None);
    }

    /// Whether a frame is on its way to the screen.
    #[cfg(test)]
    pub fn waits(&self) -> bool {
        self.waits == Waits::Beat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn a_still_screen_is_never_due() {
        let pace = Pace::default();
        assert_eq!(pace.due(5 * MS), None);
        assert!(!pace.waits());
    }

    #[test]
    fn the_first_frame_of_a_screen_that_was_still_is_drawn_at_once() {
        let mut pace = Pace::default();
        pace.stale();
        assert_eq!(pace.due(5 * MS), Some(5 * MS));
    }

    #[test]
    fn a_frame_is_drawn_as_late_as_the_deadline_and_the_last_frames_allow() {
        let mut pace = Pace::default();
        pace.stale();
        pace.drawn(2 * MS);
        // Nothing more is drawn until the frame shows, whatever changes.
        pace.stale();
        assert!(pace.waits());
        assert_eq!(pace.due(6 * MS), None);
        // It shows at 16 ms, and the next change is due by 32 ms.
        pace.shown(32 * MS);
        assert_eq!(pace.due(17 * MS), Some(32 * MS - 2 * MS - SLACK_NS));
        // The longest of the last frames is planned for, and one that
        // has left the memory is not.
        pace.drawn(MS);
        pace.shown(48 * MS);
        pace.stale();
        assert_eq!(pace.due(33 * MS), Some(48 * MS - 2 * MS - SLACK_NS));
        for _ in 0..REMEMBERED {
            pace.drawn(MS);
            pace.shown(64 * MS);
        }
        pace.stale();
        assert_eq!(pace.due(49 * MS), Some(64 * MS - MS - SLACK_NS));
    }

    #[test]
    fn a_frame_whose_deadline_has_passed_is_drawn_at_once() {
        let mut pace = Pace::default();
        pace.stale();
        pace.drawn(MS);
        pace.shown(32 * MS);
        // The screen was still for a second.
        pace.stale();
        assert_eq!(pace.due(1000 * MS), Some(1000 * MS));
    }

    #[test]
    fn a_screen_that_went_and_came_back_waits_for_no_beat() {
        let mut pace = Pace::default();
        pace.stale();
        pace.drawn(MS);
        pace.reset();
        pace.stale();
        assert_eq!(pace.due(7 * MS), Some(7 * MS));
    }
}
