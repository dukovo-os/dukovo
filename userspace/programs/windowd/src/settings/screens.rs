// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the person has set of the screens (windowd.json(5), ADR 0725).
//!
//! `screens` has an entry for a screen by its name, as programs are
//! told it (`virtio0/0`), or by its monitor's maker and model (`DEL
//! U2720Q`). A screen that has both has the entry by its name laid
//! over the one by its monitor, key by key: the plug is the more
//! particular of the two. `layouts` has a place for each screen of a
//! set of screens, and holds for that set alone.

use std::collections::BTreeMap;

use serde_json::Value;

use super::Reading;

/// How far a screen's picture is turned, clockwise, for a monitor that
/// stands turned as far the other way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Turn {
    /// Not at all.
    #[default]
    None,
    /// By 90 degrees.
    Quarter,
    /// By 180 degrees.
    Half,
    /// By 270 degrees.
    ThreeQuarters,
}

impl Turn {
    /// The turn by `degrees`, if a screen can be turned so.
    pub fn of_degrees(degrees: u64) -> Option<Turn> {
        match degrees {
            0 => Some(Turn::None),
            90 => Some(Turn::Quarter),
            180 => Some(Turn::Half),
            270 => Some(Turn::ThreeQuarters),
            _ => None,
        }
    }

    /// Whether the picture's width is the screen's height.
    pub fn sideways(self) -> bool {
        matches!(self, Turn::Quarter | Turn::ThreeQuarters)
    }
}

/// How many of a screen's pixels a point of the desktop is across:
/// from 1 to 4, in quarters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Zoom(u32);

impl Default for Zoom {
    fn default() -> Zoom {
        Zoom::ONE
    }
}

impl Zoom {
    /// A point is a pixel.
    pub const ONE: Zoom = Zoom(4);

    /// The zoom `number`, if a screen can be set to it.
    pub fn of(number: f64) -> Option<Zoom> {
        let quarters = number * 4.0;
        let fits = (4.0..=16.0).contains(&quarters) && quarters.fract() == 0.0;
        fits.then_some(Zoom(quarters as u32))
    }

    /// The zoom, as a number.
    pub fn number(self) -> f64 {
        f64::from(self.0) / 4.0
    }

    /// The whole number at or above it, which a program that draws at
    /// whole zooms alone draws at.
    pub fn whole(self) -> i32 {
        self.0.div_ceil(4) as i32
    }

    /// Whether it is a whole number.
    pub fn is_whole(self) -> bool {
        self.0.is_multiple_of(4)
    }
}

/// The mode a screen is set to: its size in pixels and, if the person
/// names one, its frames a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSet {
    /// The width and the height.
    pub size: (u32, u32),
    /// The frames a second, to the nearest whole number.
    pub hz: Option<u32>,
}

impl ModeSet {
    /// The mode `text` names: `1920x1080` or `1920x1080@60`.
    pub fn of(text: &str) -> Option<ModeSet> {
        let (size, hz) = match text.split_once('@') {
            Some((size, hz)) => (size, Some(hz)),
            None => (text, None),
        };
        let (width, height) = size.split_once('x')?;
        let number = |digits: &str| {
            let plain = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
            digits.parse::<u32>().ok().filter(|number| plain && *number > 0)
        };
        let hz = match hz {
            Some(hz) => Some(number(hz)?),
            None => None,
        };
        Some(ModeSet { size: (number(width)?, number(height)?), hz })
    }
}

impl core::fmt::Display for ModeSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}x{}", self.size.0, self.size.1)?;
        self.hz.map_or(Ok(()), |hz| write!(f, "@{hz}"))
    }
}

/// What is set of one screen. What is not set is as the screen and
/// windowd have it: the mode the monitor prefers, no zoom, no turn,
/// in use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenSet {
    /// Its mode.
    pub mode: Option<ModeSet>,
    /// Its zoom.
    pub zoom: Option<Zoom>,
    /// Its turn.
    pub turn: Option<Turn>,
    /// Whether the desktop shows on it.
    pub used: Option<bool>,
}

impl ScreenSet {
    /// This with `over` laid over it, key by key.
    pub fn under(self, over: ScreenSet) -> ScreenSet {
        ScreenSet {
            mode: over.mode.or(self.mode),
            zoom: over.zoom.or(self.zoom),
            turn: over.turn.or(self.turn),
            used: over.used.or(self.used),
        }
    }

    /// Lays the entry `value` of the key `key` over this.
    fn lay(&mut self, reading: &mut Reading<'_>, key: &str, value: &Value) {
        let Some(entry) = reading.object(key, value) else { return };
        for (within, value) in entry {
            let key = format!("{key}.{within}");
            match within.as_str() {
                "mode" => match value.as_str().and_then(ModeSet::of) {
                    Some(mode) => self.mode = Some(mode),
                    None => reading.complain(
                        &key,
                        format_args!("is {value}, not a mode such as \"1920x1080@60\""),
                    ),
                },
                "scale" => match value.as_f64().and_then(Zoom::of) {
                    Some(zoom) => self.zoom = Some(zoom),
                    None => reading.complain(
                        &key,
                        format_args!("is {value}, not a number from 1 to 4 in quarters"),
                    ),
                },
                "turn" => match value.as_u64().and_then(Turn::of_degrees) {
                    Some(turn) => self.turn = Some(turn),
                    None => {
                        reading.complain(&key, format_args!("is {value}, not 0, 90, 180 or 270"));
                    }
                },
                "use" => match value.as_bool() {
                    Some(used) => self.used = Some(used),
                    None => reading.complain(&key, format_args!("is {value}, not true or false")),
                },
                _ => reading.complain(&key, "is not a setting windowd knows"),
            }
        }
    }
}

/// The places of a set of screens: for each, by its name or its
/// monitor, where its corner is on the desktop.
pub type Places = BTreeMap<String, (i32, i32)>;

/// What is set of the screens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreensSet {
    /// The entries of `screens`, by their keys.
    pub screens: BTreeMap<String, ScreenSet>,
    /// The entries of `layouts`.
    pub layouts: Vec<Places>,
}

impl ScreensSet {
    /// What is set of the screen programs are told is `name`, whose
    /// monitor's maker and model are `monitor`.
    pub fn of(&self, name: &str, monitor: &str) -> ScreenSet {
        let entry = |key: &str| self.screens.get(key).copied().unwrap_or_default();
        entry(monitor).under(entry(name))
    }

    /// Lays `value`, a file's `screens`, over these.
    pub(super) fn lay_screens(&mut self, reading: &mut Reading<'_>, value: &Value) {
        let Some(entries) = reading.object("screens", value) else { return };
        for (screen, value) in entries {
            if screen.is_empty() {
                reading.complain("screens", "has an entry for a screen of no name");
                continue;
            }
            let key = format!("screens.{screen}");
            self.screens.entry(screen.clone()).or_default().lay(reading, &key, value);
        }
    }

    /// Lays `value`, a file's `layouts`, over these: a layout takes
    /// the place of the one for the same screens, and is one more
    /// otherwise.
    pub(super) fn lay_layouts(&mut self, reading: &mut Reading<'_>, value: &Value) {
        let Some(list) = value.as_array() else {
            reading.complain("layouts", "is not a list of layouts");
            return;
        };
        for (at, value) in list.iter().enumerate() {
            let key = format!("layouts[{at}]");
            let Some(places) = places(reading, &key, value) else { continue };
            let same = |layout: &&mut Places| layout.keys().eq(places.keys());
            match self.layouts.iter_mut().find(same) {
                Some(layout) => *layout = places,
                None => self.layouts.push(places),
            }
        }
    }
}

/// The places the layout `value` has, if it is one: an object whose
/// `screens` has a column and a row for each of one screen or more.
fn places(reading: &mut Reading<'_>, key: &str, value: &Value) -> Option<Places> {
    let layout = reading.object(key, value)?;
    for within in layout.keys().filter(|within| *within != "screens") {
        reading.complain(&format!("{key}.{within}"), "is not a setting windowd knows");
    }
    let key = format!("{key}.screens");
    let Some(screens) = layout.get("screens") else {
        reading.complain(&key, "is not there");
        return None;
    };
    let screens = reading.object(&key, screens)?;
    let mut places = Places::new();
    for (screen, value) in screens {
        let place = value.as_array().and_then(|pair| match pair.as_slice() {
            [x, y] => {
                let whole = |value: &Value| value.as_i64().and_then(|at| i32::try_from(at).ok());
                Some((whole(x)?, whole(y)?))
            }
            _ => None,
        });
        let Some(place) = place.filter(|_| !screen.is_empty()) else {
            let key = format!("{key}.{screen}");
            reading.complain(&key, format_args!("is {value}, not a column and a row"));
            return None;
        };
        places.insert(screen.clone(), place);
    }
    if places.is_empty() {
        reading.complain(&key, "names no screen");
        return None;
    }
    Some(places)
}

#[cfg(test)]
mod tests {
    use super::super::Settings;
    use super::*;

    fn laid(texts: &[&str]) -> (ScreensSet, Vec<String>) {
        let mut settings = Settings::default();
        let mut wrong = Vec::new();
        for (at, text) in texts.iter().enumerate() {
            wrong.extend(settings.lay(&format!("file{at}"), text));
        }
        (settings.screens, wrong)
    }

    #[test]
    fn a_mode_is_a_size_and_perhaps_a_rate() {
        assert_eq!(ModeSet::of("1920x1080"), Some(ModeSet { size: (1920, 1080), hz: None }));
        assert_eq!(ModeSet::of("1280x800@75"), Some(ModeSet { size: (1280, 800), hz: Some(75) }));
        for wrong in ["", "1920", "1920x", "x1080", "0x600", "800x600@", "800x600@0", "+8x6"] {
            assert_eq!(ModeSet::of(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_zoom_is_from_one_to_four_in_quarters() {
        assert_eq!(Zoom::of(1.0), Some(Zoom::ONE));
        let zoom = Zoom::of(1.25).expect("a quarter over one is a zoom");
        assert_eq!((zoom.number(), zoom.whole()), (1.25, 2));
        assert!(!zoom.is_whole());
        let zoom = Zoom::of(4.0).expect("four is a zoom");
        assert_eq!((zoom.whole(), zoom.is_whole()), (4, true));
        for wrong in [0.0, 0.75, 1.1, 4.25, -2.0, f64::NAN] {
            assert_eq!(Zoom::of(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_screen_s_entry_is_read_key_by_key() {
        let (set, wrong) = laid(&[r#"{"screens": {"virtio0/0":
            {"mode": "1920x1080@60", "scale": 1.5, "turn": 90, "use": false}}}"#]);
        assert_eq!(wrong, Vec::<String>::new());
        let screen = set.of("virtio0/0", "");
        assert_eq!(screen.mode, Some(ModeSet { size: (1920, 1080), hz: Some(60) }));
        assert_eq!(screen.zoom, Zoom::of(1.5));
        assert_eq!(screen.turn, Some(Turn::Quarter));
        assert_eq!(screen.used, Some(false));
        assert_eq!(set.of("virtio0/1", "DEL U2720Q"), ScreenSet::default());
    }

    #[test]
    fn the_entry_by_the_name_is_laid_over_the_one_by_the_monitor() {
        let (set, wrong) = laid(&[
            r#"{"screens": {"DEL U2720Q": {"scale": 2, "turn": 180}}}"#,
            r#"{"screens": {"virtio0/1": {"turn": 0}, "DEL U2720Q": {"mode": "800x600"}}}"#,
        ]);
        assert_eq!(wrong, Vec::<String>::new());
        let screen = set.of("virtio0/1", "DEL U2720Q");
        assert_eq!(screen.zoom, Zoom::of(2.0));
        assert_eq!(screen.turn, Some(Turn::None));
        assert_eq!(screen.mode, Some(ModeSet { size: (800, 600), hz: None }));
        assert_eq!(set.of("virtio0/0", "DEL U2720Q").turn, Some(Turn::Half));
    }

    #[test]
    fn what_is_wrong_of_a_screen_costs_that_key_alone() {
        let (set, wrong) = laid(&[r#"{"screens": {"a": {"mode": "big", "scale": 1.1,
            "turn": 45, "use": 1, "colour": "red", "zoom": 2}, "b": 3, "c": {"scale": 2}}}"#]);
        assert_eq!(
            wrong,
            [
                "file0: `screens.a.colour` is not a setting windowd knows",
                "file0: `screens.a.mode` is \"big\", not a mode such as \"1920x1080@60\"",
                "file0: `screens.a.scale` is 1.1, not a number from 1 to 4 in quarters",
                "file0: `screens.a.turn` is 45, not 0, 90, 180 or 270",
                "file0: `screens.a.use` is 1, not true or false",
                "file0: `screens.a.zoom` is not a setting windowd knows",
                "file0: `screens.b` is not an object",
            ]
        );
        assert_eq!(set.of("a", ""), ScreenSet::default());
        assert_eq!(set.of("c", "").zoom, Zoom::of(2.0));
    }

    #[test]
    fn a_layout_takes_the_place_of_the_one_for_the_same_screens() {
        let (set, wrong) = laid(&[
            r#"{"layouts": [{"screens": {"a": [0, 0], "b": [800, 0]}},
                {"screens": {"a": [0, 0]}}]}"#,
            r#"{"layouts": [{"screens": {"b": [0, 0], "a": [0, 600]}},
                {"screens": {"a": [0, 0], "c": [-800, 0]}}]}"#,
        ]);
        assert_eq!(wrong, Vec::<String>::new());
        let places = |pairs: &[(&str, (i32, i32))]| -> Places {
            pairs.iter().map(|(name, at)| ((*name).to_owned(), *at)).collect()
        };
        assert_eq!(
            set.layouts,
            [
                places(&[("a", (0, 600)), ("b", (0, 0))]),
                places(&[("a", (0, 0))]),
                places(&[("a", (0, 0)), ("c", (-800, 0))]),
            ]
        );
    }

    #[test]
    fn a_layout_that_is_wrong_is_left_out_whole() {
        let (set, wrong) = laid(&[r#"{"layouts": [{"screens": {"a": [0, 0], "b": [1]}},
            {"screens": {}}, {"name": "desk"}, 7, {"screens": {"a": [0.5, 0]}}]}"#]);
        assert_eq!(
            wrong,
            [
                "file0: `layouts[0].screens.b` is [1], not a column and a row",
                "file0: `layouts[1].screens` names no screen",
                "file0: `layouts[2].name` is not a setting windowd knows",
                "file0: `layouts[2].screens` is not there",
                "file0: `layouts[3]` is not an object",
                "file0: `layouts[4].screens.a` is [0.5,0], not a column and a row",
            ]
        );
        assert!(set.layouts.is_empty());
        let (_, wrong) = laid(&[r#"{"layouts": {}}"#]);
        assert_eq!(wrong, ["file0: `layouts` is not a list of layouts"]);
    }
}
