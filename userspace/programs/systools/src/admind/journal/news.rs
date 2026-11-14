// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the open tabs are told of the journal: a row that has ended. A
//! console change ends with its row ([`Begun::end`]) and the round of hand
//! edits ends with one for each person it names ([`Journal::record`]); each
//! announces its row here once the store has kept it, and each open
//! stream of events (`routes::events`) tells its tab of the rows that
//! ended since it last asked, as `event: change` with the row's number,
//! how it was made and when. Nothing else of the row is in it: not who,
//! not the action, not a file's name.
//!
//! A tab that opens is not told of the rows from before: the page reads
//! them from the journal. The last [`KEEP`] rows are held in memory for the
//! streams that have not asked yet, and a stream that falls further
//! behind misses the oldest.
//!
//! [`Begun::end`]: super::Begun::end
//! [`Journal::record`]: super::Journal::record

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use tokio::sync::watch;

/// How many rows are held for the streams that have not asked.
pub const KEEP: usize = 256;

/// A row that ended: what a tab is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The row's number in the journal.
    pub id: i64,
    /// How it was made: `console`, `token NAME` or `by hand`.
    pub how: String,
    /// When it ended, in seconds since 1970.
    pub at: i64,
}

/// The rows that ended lately, and who is waiting for the next.
pub struct News {
    /// The rows held, each with its position, oldest first.
    rows: RefCell<VecDeque<(u64, Row)>>,
    /// The position of the newest row, 0 before any.
    last: Cell<u64>,
    /// Sent to at each row, which wakes the streams.
    wake: watch::Sender<()>,
}

impl News {
    /// News with no row.
    pub fn new() -> News {
        News { rows: RefCell::default(), last: Cell::new(0), wake: watch::channel(()).0 }
    }

    /// Announces the row `id`, made `how`, which ended at Unix time `at`.
    pub fn announce(&self, id: i64, how: &str, at: i64) {
        let position = self.last.get() + 1;
        self.last.set(position);
        let mut rows = self.rows.borrow_mut();
        rows.push_back((position, Row { id, how: how.to_string(), at }));
        while rows.len() > KEEP {
            rows.pop_front();
        }
        drop(rows);
        self.wake.send_replace(());
    }

    /// A receiver that is woken at each row from now on.
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.wake.subscribe()
    }

    /// Where a stream that opens now starts: the rows after it are the
    /// stream's to tell.
    pub fn position(&self) -> u64 {
        self.last.get()
    }

    /// The rows after `position`, oldest first, and the position of the
    /// last of them (`position` when there are none).
    pub fn since(&self, position: u64) -> (u64, Vec<Row>) {
        let rows = self.rows.borrow();
        let told: Vec<Row> =
            rows.iter().filter(|(at, _)| *at > position).map(|(_, row)| row.clone()).collect();
        (position.max(self.last.get()), told)
    }
}

impl Default for News {
    fn default() -> News {
        News::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_told_of_the_rows_after_where_it_started_and_each_once() {
        let news = News::new();
        news.announce(1, "console", 100);
        let start = news.position();
        assert_eq!(news.since(start), (start, Vec::new()));
        news.announce(2, "by hand", 110);
        news.announce(3, "token ci", 120);
        let (position, rows) = news.since(start);
        assert_eq!(
            rows,
            [
                Row { id: 2, how: "by hand".into(), at: 110 },
                Row { id: 3, how: "token ci".into(), at: 120 }
            ]
        );
        assert_eq!(news.since(position), (position, Vec::new()));
        // Another stream that started earlier is told of the rows it has not
        // been.
        assert_eq!(news.since(0).1.len(), 3);
    }

    #[test]
    fn only_the_last_rows_are_held_for_a_stream_that_has_not_asked() {
        let news = News::new();
        for id in 0..KEEP as i64 + 10 {
            news.announce(id, "console", id);
        }
        let (position, rows) = news.since(0);
        assert_eq!(rows.len(), KEEP);
        assert_eq!(rows[0].id, 10);
        assert_eq!(rows[KEEP - 1].id, KEEP as i64 + 9);
        assert_eq!(position, KEEP as u64 + 10);
    }

    #[test]
    fn a_row_wakes_the_streams() {
        let news = News::new();
        let mut woken = news.subscribe();
        assert!(!woken.has_changed().unwrap());
        news.announce(1, "console", 5);
        assert!(woken.has_changed().unwrap());
        woken.mark_unchanged();
        news.announce(2, "console", 6);
        news.announce(3, "console", 7);
        // Two rows are one waking: the stream takes both when it asks.
        assert!(woken.has_changed().unwrap());
        assert_eq!(news.since(0).1.len(), 3);
    }
}
