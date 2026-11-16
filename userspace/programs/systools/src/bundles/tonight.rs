// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An app's update, installed tonight (ADR 0565): `bundle update --tonight`.
//!
//! An administrator asks for the update of an app everyone has to be
//! installed in tonight's window and not now, as an app's service
//! restarting in the middle of the day is what they put off. The window
//! is the hour that ends at the restart time of updates.toml(5), so that
//! the apps are done before a staged system update restarts the machine;
//! with no restart time it is 2:00 to 3:00 (local time).
//!
//! bundled keeps the requests in `tonight.json`, beside its registry and
//! not one of its files. What a wake does with them is a plain function
//! of the requests, the window and the time ([`plan`]): a request is
//! installed when a wake falls inside the window with more than five
//! minutes left, and one whose window passed with the machine off waits
//! for the next, never to run in the day. bundled reads the clock and the
//! settings and does the installs.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::json;
use crate::sysupdate::online::window_of;

/// The file, in `/system/state/bundled`.
pub const FILE: &str = "tonight.json";
/// Its mode: bundled's alone.
pub const MODE: u32 = 0o600;
/// What an install made only to see what an update asks for says when it
/// asks for nothing more: it unpacked and checked the update, and kept
/// none of it.
pub const CHECKED: &str = "checked, not installed";
/// How many seconds a day takes.
const DAY: u64 = 86_400;
/// How long before the window ends that no install starts, in seconds.
pub const LAST: u64 = 5 * 60;
/// The most requests kept: one for each app.
const MOST: usize = 512;
/// The longest bundled sleeps while a request waits, so that a changed
/// restart time is seen at the next wake.
pub const NAP_MOST: Duration = Duration::from_hours(1);

/// An update asked for tonight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Want {
    /// The app's ID.
    pub id: String,
    /// The newest version offered when it was asked.
    pub to: String,
    /// Whether the person accepted what that version asks for.
    pub yes: bool,
    /// The user ID of who asked; for an administrator who used `elevate`,
    /// their own and not root's.
    pub by: u32,
    /// When it was asked, in milliseconds since 1970.
    pub asked_ms: u64,
    /// When the first window passed without it, if one did.
    pub missed_ms: Option<u64>,
}

/// What the file holds: one request for each app.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wanted {
    pub wanted: Vec<Want>,
}

impl Wanted {
    /// The requests the file text `text` holds: none for a text that is
    /// not the record, or is of another format.
    #[must_use]
    pub fn read(text: &str) -> Wanted {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return Wanted::default() };
        if value.get("format").and_then(Value::as_u64) != Some(u64::from(json::FORMAT)) {
            return Wanted::default();
        }
        serde_json::from_value(value).unwrap_or_default()
    }

    /// The file's text: one line, with its newline.
    ///
    /// # Panics
    ///
    /// Never: strings and numbers always serialize.
    #[must_use]
    pub fn text(&self) -> String {
        let mut line = json::line(self).expect("requests are strings and numbers");
        line.push('\n');
        line
    }

    /// The request for `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Want> {
        self.wanted.iter().find(|want| want.id == id)
    }

    /// Keeps `want` for its app, in place of the one there was. Asked
    /// again, an app keeps the time it was first asked and the window it
    /// missed.
    pub fn ask(&mut self, mut want: Want) {
        if let Some(before) = self.get(&want.id) {
            want.asked_ms = before.asked_ms;
            want.missed_ms = before.missed_ms;
        }
        self.wanted.retain(|one| one.id != want.id);
        self.wanted.push(want);
        if self.wanted.len() > MOST {
            self.wanted.sort_by_key(|want| want.asked_ms);
            let over = self.wanted.len() - MOST;
            self.wanted.drain(..over);
        }
    }

    /// Takes away the request for `id`; whether there was one.
    pub fn cancel(&mut self, id: &str) -> bool {
        let before = self.wanted.len();
        self.wanted.retain(|want| want.id != id);
        self.wanted.len() != before
    }

    /// Notes that the window of `id`'s request passed without it, at
    /// `ms`, unless that is noted already.
    pub fn missed(&mut self, id: &str, ms: u64) {
        if let Some(want) = self.wanted.iter_mut().find(|want| want.id == id)
            && want.missed_ms.is_none()
        {
            want.missed_ms = Some(ms);
        }
    }
}

/// The window, in minutes after local midnight: the hour that ends at
/// `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    start: u32,
    end: u32,
}

impl Window {
    /// The hour that ends at the restart time `restart`, `HH:MM`, of
    /// updates.toml(5); 2:00 to 3:00 where there is none or it does not
    /// read.
    #[must_use]
    pub fn of(restart: Option<&str>) -> Window {
        let (start, end) = window_of(restart);
        Window { start, end }
    }

    /// `HH:MM` of `minutes` after midnight.
    fn clock(minutes: u32) -> String {
        format!("{:02}:{:02}", minutes / 60, minutes % 60)
    }

    /// "between 02:00 and 03:00".
    #[must_use]
    pub fn words(self) -> String {
        format!("between {} and {}", Window::clock(self.start), Window::clock(self.end))
    }

    /// Whether `sod`, seconds after local midnight, is in the window,
    /// which ends the moment its end comes. Across midnight, a restart
    /// at 00:30 makes 23:30 to 00:30.
    #[must_use]
    pub fn contains(self, sod: u32) -> bool {
        let (start, end) = (u64::from(self.start) * 60, u64::from(self.end) * 60);
        let sod = u64::from(sod);
        if start < end { (start..end).contains(&sod) } else { sod >= start || sod < end }
    }

    /// How many seconds from `sod` until the next end of the window: one
    /// to a whole day, since the moment of the end is in the window no
    /// more.
    #[must_use]
    pub fn ends_in(self, sod: u32) -> u64 {
        (u64::from(self.end) * 60 + DAY - u64::from(sod) - 1) % DAY + 1
    }

    /// How many seconds from `sod` until the next start of the window; none
    /// at the start itself.
    #[must_use]
    pub fn starts_in(self, sod: u32) -> u64 {
        (u64::from(self.start) * 60 + DAY - u64::from(sod)) % DAY
    }

    /// Whether an install may start at `sod`: in the window, with more
    /// than [`LAST`] seconds of it left.
    #[must_use]
    pub fn may_start(self, sod: u32) -> bool {
        self.contains(sod) && self.ends_in(sod) > LAST
    }
}

/// What a wake does with the requests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The apps to install now, in the order they were asked.
    pub install: Vec<String>,
    /// The apps whose first window passed without them, and when it
    /// ended, in milliseconds since 1970.
    pub missed: Vec<(String, u64)>,
}

/// What a wake at `now`, in seconds since 1970, does with `wanted` for
/// `window`; `sod` says the seconds after local midnight of a time.
///
/// In the window with time to spare, every request is installed, one
/// asked then at once. Outside it nothing is: an app's service restarting
/// in the day is what the person put off. A request whose first window
/// ended without it, because the machine was off or bundled not running,
/// is noted so, and goes on to the next window.
#[must_use]
pub fn plan(wanted: &Wanted, window: Window, now: u64, sod: &dyn Fn(u64) -> u32) -> Plan {
    if window.may_start(sod(now)) {
        return Plan {
            install: wanted.wanted.iter().map(|want| want.id.clone()).collect(),
            missed: Vec::new(),
        };
    }
    let missed = wanted
        .wanted
        .iter()
        .filter(|want| want.missed_ms.is_none())
        .filter_map(|want| {
            // The first end of the window after it was asked.
            let asked = want.asked_ms / 1000;
            let ended = asked + window.ends_in(sod(asked));
            (ended <= now).then(|| (want.id.clone(), ended * 1000))
        })
        .collect();
    Plan { install: Vec::new(), missed }
}

/// How long bundled sleeps while a request waits, at `now`: until the
/// window starts, or ends, and an hour at most, so that a changed
/// restart time is seen.
#[must_use]
pub fn nap(window: Window, now: u64, sod: &dyn Fn(u64) -> u32) -> Duration {
    let sod = sod(now);
    let secs = if window.contains(sod) { window.ends_in(sod) + 1 } else { window.starts_in(sod) };
    Duration::from_secs(secs.max(1)).min(NAP_MOST)
}

/// When a request waiting at `now` is installed: now, in the window with
/// time to spare, else at the window's next start. In seconds since 1970.
#[must_use]
pub fn starts(window: Window, now: u64, sod: &dyn Fn(u64) -> u32) -> u64 {
    let sod = sod(now);
    if window.may_start(sod) { now } else { now + window.starts_in(sod) }
}

/// The seconds after local midnight of `seconds` since 1970.
#[must_use]
pub fn local_second_of_day(seconds: u64) -> u32 {
    let text = crate::util::local_time(i64::try_from(seconds).unwrap_or(0), "%H:%M:%S");
    let mut parts = text.split(':').map(|part| part.parse::<u32>().unwrap_or(0));
    let (hours, minutes, secs) =
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    hours * 3600 + minutes * 60 + secs.min(59)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seconds of a day of time `seconds` when the zone is UTC.
    fn utc(seconds: u64) -> u32 {
        u32::try_from(seconds % DAY).unwrap()
    }

    /// A time `hours`:`minutes`:`seconds` on the day `days` after 1970.
    fn at(days: u64, hours: u64, minutes: u64, seconds: u64) -> u64 {
        days * DAY + hours * 3600 + minutes * 60 + seconds
    }

    fn want(id: &str, asked: u64) -> Want {
        Want {
            id: id.into(),
            to: "8.0.4".into(),
            yes: false,
            by: 1000,
            asked_ms: asked * 1000,
            missed_ms: None,
        }
    }

    fn asked(wants: &[Want]) -> Wanted {
        Wanted { wanted: wants.to_vec() }
    }

    #[test]
    fn the_window_is_the_hour_before_the_restart_time_and_2_to_3_without_one() {
        assert_eq!(Window::of(Some("03:00")).words(), "between 02:00 and 03:00");
        assert_eq!(Window::of(Some("04:30")).words(), "between 03:30 and 04:30");
        // Across midnight.
        assert_eq!(Window::of(Some("00:30")).words(), "between 23:30 and 00:30");
        assert_eq!(Window::of(Some("00:00")).words(), "between 23:00 and 00:00");
        assert_eq!(Window::of(None).words(), "between 02:00 and 03:00");
        // A time that does not read is no restart time.
        for odd in ["", "3:00", "25:00", "03:60", "soon"] {
            assert_eq!(Window::of(Some(odd)), Window::of(None), "{odd:?}");
        }
    }

    #[test]
    fn a_time_is_in_the_window_from_its_start_to_its_end_and_not_at_it() {
        let three = Window::of(Some("03:00"));
        assert!(!three.contains(at(0, 1, 59, 59) as u32));
        assert!(three.contains(at(0, 2, 0, 0) as u32));
        assert!(three.contains(at(0, 2, 59, 59) as u32));
        assert!(!three.contains(at(0, 3, 0, 0) as u32));
        let night = Window::of(Some("00:30"));
        assert!(night.contains(at(0, 23, 30, 0) as u32));
        assert!(night.contains(at(0, 23, 59, 59) as u32));
        assert!(night.contains(0));
        assert!(night.contains(at(0, 0, 29, 59) as u32));
        assert!(!night.contains(at(0, 0, 30, 0) as u32));
        assert!(!night.contains(at(0, 12, 0, 0) as u32));
    }

    #[test]
    fn the_end_and_the_start_are_counted_from_a_time_to_the_next_of_each() {
        let three = Window::of(Some("03:00"));
        assert_eq!(three.ends_in(at(0, 2, 30, 0) as u32), 1800);
        assert_eq!(three.ends_in(at(0, 2, 59, 59) as u32), 1);
        // At the end, the next end is a day away.
        assert_eq!(three.ends_in(at(0, 3, 0, 0) as u32), DAY);
        assert_eq!(three.ends_in(at(0, 12, 0, 0) as u32), 15 * 3600);
        assert_eq!(three.starts_in(at(0, 12, 0, 0) as u32), 14 * 3600);
        assert_eq!(three.starts_in(at(0, 2, 0, 0) as u32), 0);
        assert_eq!(three.starts_in(at(0, 2, 30, 0) as u32), DAY - 1800);
        let night = Window::of(Some("00:30"));
        assert_eq!(night.ends_in(at(0, 23, 45, 0) as u32), 45 * 60);
        assert_eq!(night.starts_in(at(0, 23, 0, 0) as u32), 30 * 60);
    }

    #[test]
    fn no_install_starts_in_the_last_five_minutes() {
        let three = Window::of(Some("03:00"));
        assert!(three.may_start(at(0, 2, 0, 0) as u32));
        assert!(three.may_start(at(0, 2, 54, 59) as u32));
        // Five minutes left, to the second, are too few; more than five are enough.
        assert!(!three.may_start(at(0, 2, 55, 0) as u32));
        assert!(!three.may_start(at(0, 2, 59, 59) as u32));
        assert!(!three.may_start(at(0, 3, 0, 0) as u32));
        assert!(!three.may_start(at(0, 1, 0, 0) as u32));
    }

    #[test]
    fn a_request_asked_at_noon_runs_at_the_window_s_start_and_one_asked_in_it_runs_at_once() {
        let three = Window::of(Some("03:00"));
        let noon = at(10, 12, 0, 0);
        let wanted = asked(&[want("org.example.notes", noon)]);
        // Nothing before the window, and nothing is missed.
        for now in [noon, at(10, 23, 0, 0), at(11, 1, 59, 59)] {
            let plan = plan(&wanted, three, now, &utc);
            assert_eq!(plan, Plan::default(), "{now}");
        }
        assert_eq!(
            plan(&wanted, three, at(11, 2, 0, 0), &utc).install,
            ["org.example.notes".to_string()]
        );
        // Asked inside the window: the wake the request makes runs it.
        let inside = at(10, 2, 20, 0);
        let wanted = asked(&[want("org.example.tea", inside)]);
        assert_eq!(plan(&wanted, three, inside, &utc).install, ["org.example.tea".to_string()]);
    }

    #[test]
    fn a_wake_after_the_window_installs_nothing_and_notes_that_it_passed() {
        let three = Window::of(Some("03:00"));
        let noon = at(10, 12, 0, 0);
        let wanted = asked(&[want("org.example.notes", noon)]);
        // The machine started at four in the morning.
        let after = plan(&wanted, three, at(11, 4, 0, 0), &utc);
        assert!(after.install.is_empty());
        assert_eq!(after.missed, [("org.example.notes".to_string(), at(11, 3, 0, 0) * 1000)]);
        // A day later, still not seen: the time noted is the first window's.
        let mut kept = wanted.clone();
        kept.missed("org.example.notes", at(11, 3, 0, 0) * 1000);
        kept.missed("org.example.notes", at(12, 3, 0, 0) * 1000);
        assert_eq!(kept.wanted[0].missed_ms, Some(at(11, 3, 0, 0) * 1000));
        assert!(plan(&kept, three, at(12, 12, 0, 0), &utc).missed.is_empty());
        // And the next window runs it.
        assert_eq!(
            plan(&kept, three, at(12, 2, 10, 0), &utc).install,
            ["org.example.notes".to_string()]
        );
        // Asked after the window of the same night: that window is not a miss.
        let late = asked(&[want("org.example.tea", at(11, 3, 30, 0))]);
        assert_eq!(plan(&late, three, at(11, 9, 0, 0), &utc), Plan::default());
        assert_eq!(plan(&late, three, at(12, 4, 0, 0), &utc).missed.len(), 1);
    }

    #[test]
    fn the_last_five_minutes_start_nothing_and_the_window_that_ends_so_is_noted_after() {
        let three = Window::of(Some("03:00"));
        let early = at(10, 23, 0, 0);
        let wanted = asked(&[want("org.example.notes", early)]);
        let late = plan(&wanted, three, at(11, 2, 56, 0), &utc);
        assert_eq!(late, Plan::default());
        let after = plan(&wanted, three, at(11, 3, 0, 5), &utc);
        assert_eq!(after.missed.len(), 1);
    }

    #[test]
    fn a_restart_at_half_past_midnight_makes_the_window_the_half_hour_before_midnight_too() {
        let night = Window::of(Some("00:30"));
        let wanted = asked(&[want("org.example.notes", at(10, 18, 0, 0))]);
        assert!(plan(&wanted, night, at(10, 23, 29, 59), &utc).install.is_empty());
        assert_eq!(plan(&wanted, night, at(10, 23, 30, 0), &utc).install.len(), 1);
        assert_eq!(plan(&wanted, night, at(11, 0, 10, 0), &utc).install.len(), 1);
        assert!(plan(&wanted, night, at(11, 0, 25, 0), &utc).install.is_empty());
    }

    #[test]
    fn bundled_sleeps_until_the_window_and_an_hour_at_most() {
        let three = Window::of(Some("03:00"));
        let nap = |now: u64| nap(three, now, &utc);
        assert_eq!(nap(at(0, 1, 30, 0)), Duration::from_mins(30));
        assert_eq!(nap(at(0, 12, 0, 0)), NAP_MOST);
        // In the window, until just after its end.
        assert_eq!(nap(at(0, 2, 59, 0)), Duration::from_secs(61));
        assert_eq!(nap(at(0, 2, 0, 0)), NAP_MOST);
        // Never none, at the start.
        assert!(nap(at(0, 1, 59, 59)) >= Duration::from_secs(1));
    }

    #[test]
    fn a_request_says_when_it_installs() {
        let three = Window::of(Some("03:00"));
        let starts = |now: u64| starts(three, now, &utc);
        assert_eq!(starts(at(5, 12, 0, 0)), at(6, 2, 0, 0));
        assert_eq!(starts(at(5, 2, 10, 0)), at(5, 2, 10, 0));
        // In the last minutes, the window of the next night.
        assert_eq!(starts(at(5, 2, 58, 0)), at(6, 2, 0, 0));
    }

    #[test]
    fn the_requests_read_back_and_a_text_that_is_not_one_holds_none() {
        let mut wanted = Wanted::default();
        wanted.ask(want("org.example.notes", 1_794_000_000));
        wanted.ask(Want { yes: true, ..want("org.example.tea", 1_794_000_100) });
        let text = wanted.text();
        assert!(text.starts_with("{\"format\":1,\"wanted\":[{\"id\":\"org.example.notes\""));
        assert!(text.ends_with("\n"));
        assert_eq!(text.matches('\n').count(), 1);
        assert_eq!(Wanted::read(&text), wanted);
        for odd in ["", "not json", "{}", "{\"format\":2,\"wanted\":[]}", "[]"] {
            assert_eq!(Wanted::read(odd), Wanted::default(), "{odd:?}");
        }
    }

    #[test]
    fn asking_again_keeps_the_first_time_and_the_window_missed_and_cancelling_takes_it_away() {
        let mut wanted = asked(&[Want {
            missed_ms: Some(1_794_100_000_000),
            ..want("org.example.notes", 1_794_000_000)
        }]);
        wanted.ask(Want {
            to: "8.0.5".into(),
            yes: true,
            ..want("org.example.notes", 1_794_200_000)
        });
        assert_eq!(wanted.wanted.len(), 1);
        let kept = wanted.get("org.example.notes").unwrap();
        assert_eq!((kept.to.as_str(), kept.yes), ("8.0.5", true));
        assert_eq!((kept.asked_ms, kept.missed_ms), (1_794_000_000_000, Some(1_794_100_000_000)));
        assert!(wanted.cancel("org.example.notes"));
        assert!(!wanted.cancel("org.example.notes"));
        assert_eq!(wanted, Wanted::default());
    }

    #[test]
    fn the_most_requests_kept_are_the_newest() {
        let mut wanted = Wanted::default();
        for n in 0..(MOST as u64 + 3) {
            wanted.ask(want(&format!("org.example.app{n}"), 1_794_000_000 + n));
        }
        assert_eq!(wanted.wanted.len(), MOST);
        assert!(wanted.get("org.example.app0").is_none());
        assert!(wanted.get(&format!("org.example.app{}", MOST + 2)).is_some());
    }

    #[test]
    fn the_local_second_of_the_day_reads_the_clock_text() {
        let seconds = local_second_of_day(1_794_000_000);
        assert!(seconds < 86_400);
        let text = crate::util::local_time(1_794_000_000, "%H:%M:%S");
        let parts: Vec<u32> = text.split(':').map(|part| part.parse().unwrap()).collect();
        assert_eq!(seconds, parts[0] * 3600 + parts[1] * 60 + parts[2]);
    }
}
