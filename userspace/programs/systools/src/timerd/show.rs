// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What service(8) and systemctl(1) show of the timers (ADR 0478): the
//! last and next run of each service timer, which timerd(8) keeps.

use super::protocol::{self, Reply, Request, TimerRow};

/// The timers of the service files, by name, as timerd keeps them.
///
/// # Errors
///
/// When timerd cannot be asked, or refuses (only root asks), in words.
pub fn timers() -> Result<Vec<TimerRow>, String> {
    match protocol::answer(&Request::Timers)? {
        Reply::Timers { mut timers } => {
            timers.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(timers)
        }
        _ => Err("timerd's answer was not a list of timers".into()),
    }
}

/// A time as `service` shows it, by `clock`; `never` or `none` for a
/// time that is not there.
fn when(seconds: Option<i64>, clock: &dyn Fn(i64) -> String, missing: &str) -> String {
    seconds.map_or_else(|| missing.to_string(), clock)
}

/// When the timer starts the service next, as `service` shows it: it is
/// running now, at a time, or none.
fn next_words(row: &TimerRow, clock: &dyn Fn(i64) -> String, missing: &str) -> String {
    if row.running { "running now".into() } else { when(row.next, clock, missing) }
}

/// The words of `service NAME status`'s `Timer:` line.
#[must_use]
pub fn status_line(row: &TimerRow, clock: &dyn Fn(i64) -> String) -> String {
    format!(
        "last start {}, next {}",
        when(row.last, clock, "never"),
        next_words(row, clock, "none within five years")
    )
}

/// The table `service list --timers` prints: a header when `header`, then
/// a line a timer.
#[must_use]
pub fn table(rows: &[TimerRow], clock: &dyn Fn(i64) -> String, header: bool) -> String {
    let titles = ["SERVICE", "NEXT RUN", "LAST START"].map(String::from);
    let cells: Vec<[String; 3]> = rows
        .iter()
        .map(|row| {
            [row.name.clone(), next_words(row, clock, "none"), when(row.last, clock, "never")]
        })
        .collect();
    let mut widths = [0; 2];
    for row in cells.iter().chain(header.then_some(&titles)) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut text = String::new();
    for row in header.then_some(&titles).into_iter().chain(&cells) {
        for (cell, width) in row.iter().zip(widths) {
            text.push_str(&format!("{cell:<width$}  "));
        }
        text.push_str(&row[2]);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(seconds: i64) -> String {
        format!("t{seconds}")
    }

    fn rows() -> Vec<TimerRow> {
        vec![
            TimerRow { name: "backup".into(), last: Some(5), next: Some(9), running: false },
            TimerRow { name: "logturn".into(), last: None, next: None, running: false },
            TimerRow { name: "scrub".into(), last: Some(7), next: None, running: true },
        ]
    }

    #[test]
    fn a_timer_line_says_its_last_and_next_start() {
        assert_eq!(status_line(&rows()[0], &clock), "last start t5, next t9");
        assert_eq!(
            status_line(&rows()[1], &clock),
            "last start never, next none within five years"
        );
        assert_eq!(status_line(&rows()[2], &clock), "last start t7, next running now");
    }

    #[test]
    fn the_table_has_a_line_a_timer() {
        assert_eq!(
            table(&rows(), &clock, true),
            "SERVICE  NEXT RUN     LAST START\nbackup   t9           t5\nlogturn  none         \
             never\nscrub    running now  t7\n"
        );
        assert!(!table(&rows(), &clock, false).contains("SERVICE"));
    }
}
