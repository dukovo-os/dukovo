// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How far the pointer moves for how far a mouse did (ADR 0715).
//!
//! A hand that moves slowly moves the pointer a pixel for each count of
//! the mouse, so that a pixel can be pointed at. A hand that moves fast
//! moves it up to [`MOST`] times as far, so that a wide screen is
//! crossed without lifting the mouse. Between the two the gain rises
//! smoothly, with no step a hand could feel.
//!
//! The speed is what one report of the mouse says over the time since
//! the one before: counts a millisecond.

/// The speed, in counts a millisecond, up to which a count is a pixel.
const SLOW: f64 = 0.5;

/// The speed from which a count is [`MOST`] pixels.
const FAST: f64 = 3.0;

/// The most pixels a count moves the pointer.
const MOST: f64 = 2.5;

/// The shortest time between two reports that a speed is reckoned
/// with: reports that a busy machine read together are not a faster
/// hand.
const SHORTEST_NS: u64 = 1_000_000;

/// The longest: a mouse that rested starts slowly.
const LONGEST_NS: u64 = 16_000_000;

/// The gain at `speed`.
fn gain(speed: f64) -> f64 {
    let along = ((speed - SLOW) / (FAST - SLOW)).clamp(0.0, 1.0);
    // Smooth at both ends: 3t² - 2t³.
    1.0 + (MOST - 1.0) * along * along * (3.0 - 2.0 * along)
}

/// The curve, and when the mouse last moved.
#[derive(Clone, Copy, Debug, Default)]
pub struct Curve {
    last_ns: Option<u64>,
}

impl Curve {
    /// How far the pointer moves, in pixels right and down, for a
    /// report of (`dx`, `dy`) counts at `time_ns`.
    pub fn moved(&mut self, time_ns: u64, dx: i32, dy: i32) -> (f64, f64) {
        let since = self.last_ns.map_or(LONGEST_NS, |last| time_ns.saturating_sub(last));
        self.last_ns = Some(time_ns);
        let ms = since.clamp(SHORTEST_NS, LONGEST_NS) as f64 / 1e6;
        let (dx, dy) = (f64::from(dx), f64::from(dy));
        let gain = gain(dx.hypot(dy) / ms);
        (dx * gain, dy * gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn a_slow_hand_moves_the_pointer_a_pixel_a_count() {
        let mut curve = Curve::default();
        // The first report, after a rest, and one count every 8 ms.
        assert_eq!(curve.moved(0, 1, 0), (1.0, 0.0));
        assert_eq!(curve.moved(8 * MS, 0, -1), (0.0, -1.0));
        assert_eq!(curve.moved(16 * MS, 3, 2), (3.0, 2.0));
    }

    #[test]
    fn a_fast_hand_moves_it_further_and_never_more_than_the_most() {
        let mut curve = Curve::default();
        curve.moved(0, 1, 0);
        let (dx, dy) = curve.moved(MS, 30, -40);
        assert_eq!((dx, dy), (30.0 * MOST, -40.0 * MOST));
        // Reports read together are reckoned a millisecond apart.
        let (dx, _) = curve.moved(MS, 1, 0);
        assert!(dx > 1.0 && dx < MOST, "{dx}");
    }

    #[test]
    fn the_gain_rises_without_a_step() {
        let mut last = gain(0.0);
        assert_eq!(last, 1.0);
        for step in 1..=400 {
            let now = gain(f64::from(step) * 0.01);
            assert!(now >= last && now - last < 0.02, "a step at {step}: {last} to {now}");
            last = now;
        }
        assert_eq!(last, MOST);
    }
}
