// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use super::changes::{FileChange, Marking, NewChange, ROWS_MAX, Seen};
use super::tasks::{LOG_MAX, State};
use super::versions::{Limits, sha256};
use super::*;
use crate::cert::fixtures::Scratch;

/// A text, as a file's side of a change.
fn text(bytes: &[u8]) -> Seen {
    Seen::Text { sha256: sha256(bytes), size: bytes.len() as u64 }
}

/// A row that says `who` changed `path` by the console's action.
fn console_change<'a>(who: &'a str, files: &'a [FileChange]) -> NewChange<'a> {
    NewChange {
        at: 1_000,
        who,
        how: "console",
        action: Some("service.enable"),
        argv: None,
        status: Some(0),
        task: None,
        note: None,
        files,
    }
}

#[test]
fn a_new_store_is_at_the_last_schema_version_with_its_tables() {
    let store = Store::in_memory();
    assert_eq!(store.schema_version().unwrap(), SCHEMA.len());
    for table in ["changes", "blobs", "versions", "watched", "tasks"] {
        let found: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = ?1", [table], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(found, 1, "{table}");
    }
}

#[test]
fn a_file_is_made_private_and_uses_the_rollback_journal() {
    let folder = Scratch::new("store-file");
    let path = folder.0.join("store.db");
    let store = Store::open(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let journal: String =
        store.conn.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
    assert_eq!(journal, "delete");
    let timeout: i64 =
        store.conn.pragma_query_value(None, "busy_timeout", |row| row.get(0)).unwrap();
    assert_eq!(timeout, 5000);
}

#[test]
fn what_is_kept_is_there_when_the_file_is_opened_again() {
    let folder = Scratch::new("store-again");
    let path = folder.0.join("store.db");
    let id = {
        let store = Store::open(&path).unwrap();
        store.add_change(&console_change("alice", &[])).unwrap()
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA.len());
    assert_eq!(store.change(id).unwrap().unwrap().who, "alice");
}

#[test]
fn a_file_of_a_newer_schema_is_refused_and_left_as_it_is() {
    let folder = Scratch::new("store-newer");
    let path = folder.0.join("store.db");
    drop(Store::open(&path).unwrap());
    let plain = Connection::open(&path).unwrap();
    plain.pragma_update(None, "user_version", i64::try_from(SCHEMA.len() + 1).unwrap()).unwrap();
    drop(plain);
    let refused = Store::open(&path).err().unwrap();
    assert!(refused.to_string().contains("a newer admind made"), "{refused}");
    let plain = Connection::open(&path).unwrap();
    assert_eq!(user_version(&plain).unwrap(), SCHEMA.len() + 1);
}

#[test]
fn each_schema_version_is_applied_whole_or_not_at_all() {
    let mut conn = Connection::open_in_memory().unwrap();
    let schema: &[&[&str]] = &[
        &["CREATE TABLE a (x INTEGER)", "INSERT INTO a VALUES (1)"],
        &["CREATE TABLE b (y INTEGER)", "this is not SQL"],
    ];
    assert!(migrate(&mut conn, schema).is_err());
    // The first version stands; the second left nothing, not even its
    // first statement.
    assert_eq!(user_version(&conn).unwrap(), 1);
    let tables: i64 = conn
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('a', 'b')", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 1);
    // A schema of three versions brings the file from where it was.
    let longer: &[&[&str]] = &[schema[0], &["CREATE TABLE b (y INTEGER)"], &["DROP TABLE a"]];
    migrate(&mut conn, longer).unwrap();
    assert_eq!(user_version(&conn).unwrap(), 3);
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(tables, ["b"]);
}

#[test]
fn a_reader_opens_the_file_while_admind_has_it_and_cannot_write() {
    let folder = Scratch::new("store-reader");
    let path = folder.0.join("store.db");
    let admind = Store::open(&path).unwrap();
    let id = admind.add_change(&console_change("alice", &[])).unwrap();
    let reader = Store::open_read_only(&path).unwrap();
    assert_eq!(reader.change(id).unwrap().unwrap().who, "alice");
    let wrote = reader.add_change(&console_change("eve", &[]));
    assert!(wrote.is_err());
    // What admind writes after is read by the reader's next call.
    let next = admind.add_change(&console_change("bob", &[])).unwrap();
    assert_eq!(reader.change(next).unwrap().unwrap().who, "bob");
}

#[test]
fn a_reader_refuses_a_file_that_is_not_at_its_schema_or_not_there() {
    let folder = Scratch::new("store-reader-old");
    let path = folder.0.join("store.db");
    assert!(Store::open_read_only(&path).is_err());
    // A file no admind has opened has no schema.
    drop(Connection::open(&path).unwrap());
    let refused = Store::open_read_only(&path).err().unwrap();
    assert!(refused.to_string().contains("schema version 0"), "{refused}");
}

#[test]
fn a_transaction_keeps_all_its_calls_or_none() {
    let store = Store::in_memory();
    let failed: Result<()> = store.atomically(|| {
        store.add_change(&console_change("alice", &[]))?;
        Err(Error::new("no"))
    });
    assert!(failed.is_err());
    assert!(store.changes(0, 10).unwrap().is_empty());
    store
        .atomically(|| {
            store.add_change(&console_change("alice", &[]))?;
            // Inside another, it is part of it.
            store.atomically(|| store.add_change(&console_change("bob", &[])))
        })
        .unwrap();
    assert_eq!(store.changes(0, 10).unwrap().len(), 2);
}

#[test]
fn a_version_is_kept_by_its_hash_once_however_many_files_have_it() {
    let store = Store::in_memory();
    let hash = store.keep_version("/system/settings/a.toml", b"one\n", 10).unwrap();
    assert_eq!(hash, sha256(b"one\n"));
    assert_eq!(hash.len(), 64);
    let same = store.keep_version("/system/settings/b.toml", b"one\n", 11).unwrap();
    assert_eq!(same, hash);
    assert_eq!(store.version_text(&hash).unwrap().as_deref(), Some(&b"one\n"[..]));
    assert_eq!(store.text_size().unwrap(), 4);
    assert_eq!(store.version_text(&sha256(b"other")).unwrap(), None);
}

#[test]
fn the_same_text_again_adds_no_version_and_a_text_that_comes_back_does() {
    let store = Store::in_memory();
    let path = "/system/settings/a.toml";
    store.keep_version(path, b"a", 1).unwrap();
    store.keep_version(path, b"a", 2).unwrap();
    store.keep_version(path, b"b", 3).unwrap();
    store.keep_version(path, b"a", 4).unwrap();
    assert_eq!(store.versions_of(path).unwrap(), [sha256(b"a"), sha256(b"b"), sha256(b"a")]);
}

#[test]
fn a_file_keeps_its_last_20_versions_and_a_text_no_version_has_goes() {
    let store = Store::in_memory();
    let path = "/system/settings/a.toml";
    for number in 0..25 {
        store.keep_version(path, format!("version {number}").as_bytes(), number).unwrap();
    }
    let kept = store.versions_of(path).unwrap();
    assert_eq!(kept.len(), 20);
    assert_eq!(kept[0], sha256(b"version 24"));
    assert_eq!(kept[19], sha256(b"version 5"));
    assert_eq!(store.version_text(&sha256(b"version 4")).unwrap(), None);
    assert!(store.version_text(&sha256(b"version 5")).unwrap().is_some());
    // Another file has its own 20.
    store.keep_version("/system/settings/b.toml", b"version 4", 30).unwrap();
    assert_eq!(store.versions_of(path).unwrap().len(), 20);
}

#[test]
fn a_text_two_files_have_stays_until_both_have_let_it_go() {
    let mut store = Store::in_memory();
    store.limits.per_file = 1;
    store.keep_version("/a", b"shared", 1).unwrap();
    store.keep_version("/b", b"shared", 2).unwrap();
    store.keep_version("/a", b"new", 3).unwrap();
    assert!(store.version_text(&sha256(b"shared")).unwrap().is_some());
    store.keep_version("/b", b"newer", 4).unwrap();
    assert_eq!(store.version_text(&sha256(b"shared")).unwrap(), None);
}

#[test]
fn a_text_longer_than_a_file_may_be_is_refused() {
    let store = Store::in_memory();
    let longest = vec![b'x'; Limits::DEFAULT.file as usize];
    assert!(store.keep_version("/a", &longest, 1).is_ok());
    let too_long = vec![b'x'; Limits::DEFAULT.file as usize + 1];
    let refused = store.keep_version("/b", &too_long, 1).unwrap_err();
    assert!(refused.to_string().contains("at most 1048576 bytes"), "{refused}");
    assert_eq!(store.versions_of("/b").unwrap().len(), 0);
}

#[test]
fn the_oldest_versions_go_first_when_the_texts_are_more_than_the_store_keeps() {
    let mut store = Store::in_memory();
    store.limits.total = 10;
    store.keep_version("/a", b"aaaa", 1).unwrap();
    store.keep_version("/b", b"bbbb", 2).unwrap();
    store.keep_version("/a", b"AAAA", 3).unwrap();
    // 12 bytes: the oldest version that is not a file's newest, /a's
    // first, goes.
    assert_eq!(store.text_size().unwrap(), 8);
    assert_eq!(store.version_text(&sha256(b"aaaa")).unwrap(), None);
    assert!(store.version_text(&sha256(b"bbbb")).unwrap().is_some());
    store.keep_version("/b", b"BBBB", 4).unwrap();
    assert_eq!(store.text_size().unwrap(), 8);
    assert_eq!(store.versions_of("/b").unwrap(), [sha256(b"BBBB")]);
}

#[test]
fn a_files_newest_version_goes_only_when_nothing_older_is_left_to_go() {
    let mut store = Store::in_memory();
    store.limits.total = 6;
    store.keep_version("/a", b"aaaa", 1).unwrap();
    store.keep_version("/b", b"bbbb", 2).unwrap();
    // Two files' newest versions are more than the limit: the older goes.
    assert_eq!(store.text_size().unwrap(), 4);
    assert_eq!(store.versions_of("/a").unwrap().len(), 0);
    assert_eq!(store.versions_of("/b").unwrap(), [sha256(b"bbbb")]);
}

#[test]
fn a_row_of_the_journal_reads_back_as_it_was_written() {
    let store = Store::in_memory();
    let files = [
        FileChange {
            path: "/system/settings/spark/services/web.toml".into(),
            before: Seen::Absent,
            after: text(b"enabled = true\n"),
        },
        FileChange {
            path: "/system/state/spark/secrets/web/key".into(),
            before: Seen::Secret,
            after: Seen::Secret,
        },
        FileChange {
            path: "/system/settings/big".into(),
            before: Seen::Large { sha256: sha256(b"big"), size: 2 << 20 },
            after: Seen::Unreadable,
        },
    ];
    let argv = ["service".to_string(), "web".into(), "enable".into()];
    let id = store
        .add_change(&NewChange {
            at: 1_793_000_000,
            who: "alice",
            how: "token ci",
            action: Some("service.enable"),
            argv: Some(&argv),
            status: Some(1),
            task: Some(7),
            note: Some("it did not end"),
            files: &files,
        })
        .unwrap();
    let row = store.change(id).unwrap().unwrap();
    assert_eq!(row.id, id);
    assert_eq!((row.at, row.who.as_str(), row.how.as_str()), (1_793_000_000, "alice", "token ci"));
    assert_eq!(row.action.as_deref(), Some("service.enable"));
    assert_eq!(row.argv.as_deref(), Some(&argv[..]));
    assert_eq!(
        (row.status, row.task, row.note.as_deref()),
        (Some(1), Some(7), Some("it did not end"))
    );
    assert_eq!(row.files, files);
    assert_eq!(row.seen, None);
    assert!(!row.by_hand());
    assert_eq!(store.change(id + 1).unwrap(), None);
}

#[test]
fn the_journal_says_a_file_s_side_in_words_a_page_can_read() {
    let side = serde_json::to_value(text(b"x")).unwrap();
    assert_eq!(side, json!({ "state": "text", "sha256": sha256(b"x"), "size": 1 }));
    assert_eq!(serde_json::to_value(Seen::Absent).unwrap(), json!({ "state": "absent" }));
    assert_eq!(serde_json::to_value(Seen::Secret).unwrap(), json!({ "state": "secret" }));
    assert_eq!(serde_json::to_value(Seen::Unreadable).unwrap(), json!({ "state": "unreadable" }));
}

#[test]
fn two_sides_are_the_same_when_their_hashes_are_or_both_are_absent() {
    assert!(text(b"a").same_as(&text(b"a")));
    assert!(!text(b"a").same_as(&text(b"b")));
    assert!(Seen::Absent.same_as(&Seen::Absent));
    assert!(!Seen::Absent.same_as(&text(b"a")));
    // Of a secret nothing is known, so it is never the same.
    assert!(!Seen::Secret.same_as(&Seen::Secret));
    assert!(!Seen::Unreadable.same_as(&Seen::Unreadable));
}

#[test]
fn the_journal_lists_the_newest_first_from_a_time_and_up_to_a_limit() {
    let store = Store::in_memory();
    for at in [100, 200, 300, 400] {
        let mut row = console_change("alice", &[]);
        row.at = at;
        store.add_change(&row).unwrap();
    }
    let all = store.changes(0, 10).unwrap();
    assert_eq!(all.iter().map(|row| row.at).collect::<Vec<_>>(), [400, 300, 200, 100]);
    let recent = store.changes(250, 10).unwrap();
    assert_eq!(recent.iter().map(|row| row.at).collect::<Vec<_>>(), [400, 300]);
    let limited = store.changes(0, 3).unwrap();
    assert_eq!(limited.iter().map(|row| row.at).collect::<Vec<_>>(), [400, 300, 200]);
}

#[test]
fn the_journal_keeps_the_newest_rows_only() {
    let store = Store::in_memory();
    store
        .atomically(|| {
            for _ in 0..ROWS_MAX + 5 {
                store.add_change(&console_change("alice", &[]))?;
            }
            Ok(())
        })
        .unwrap();
    let all = store.changes(0, 20_000).unwrap();
    assert_eq!(all.len() as i64, ROWS_MAX);
    assert_eq!(all[0].id, ROWS_MAX + 5);
    assert_eq!(all.last().unwrap().id, 6);
}

#[test]
fn a_hand_edit_is_marked_seen_once_and_nothing_else_is() {
    let store = Store::in_memory();
    let by_hand = NewChange {
        how: "by hand",
        action: None,
        status: None,
        ..console_change("outside the console", &[])
    };
    let hand = store.add_change(&by_hand).unwrap();
    let console = store.add_change(&console_change("alice", &[])).unwrap();
    assert_eq!(store.mark_seen(console, "bob", 5).unwrap(), Marking::NotByHand);
    assert_eq!(store.mark_seen(hand + 100, "bob", 5).unwrap(), Marking::Missing);
    assert_eq!(store.mark_seen(hand, "bob", 5).unwrap(), Marking::Done);
    assert_eq!(store.mark_seen(hand, "carol", 9).unwrap(), Marking::Already);
    let seen = store.change(hand).unwrap().unwrap().seen.unwrap();
    assert_eq!((seen.by.as_str(), seen.at), ("bob", 5));
    assert!(store.change(hand).unwrap().unwrap().by_hand());
}

/// The arguments of a task the tests make.
fn new_task(store: &Store, started: i64) -> i64 {
    let argv = ["admin".to_string(), "lab".into()];
    store
        .add_task("lab.long", &json!({ "seconds": 20 }), &argv, "alice", "console", started)
        .unwrap()
}

#[test]
fn a_task_is_made_waiting_and_saved_as_it_goes() {
    let store = Store::in_memory();
    let id = new_task(&store, 100);
    let mut task = store.task(id).unwrap().unwrap();
    assert_eq!((task.state, task.step, task.steps, task.percent), (State::Waiting, 0, 0, None));
    assert_eq!((task.who.as_str(), task.how.as_str(), task.started), ("alice", "console", 100));
    assert_eq!(task.args, json!({ "seconds": 20 }));
    assert_eq!(task.argv, ["admin", "lab"]);
    task.state = State::Running;
    task.step = 2;
    task.steps = 5;
    task.words = "Copying".into();
    task.percent = Some(40);
    task.log = vec!["one".into(), "two".into()];
    store.save_task(&task).unwrap();
    assert_eq!(store.task(id).unwrap().unwrap(), task);
    assert_eq!(task.changed, None);
    task.state = State::Done;
    task.ended = Some(130);
    task.status = Some(0);
    task.changed = Some(false);
    store.save_task(&task).unwrap();
    assert_eq!(store.task(id).unwrap().unwrap(), task);
    assert_eq!(task.summary()["changed"], json!(false));
    assert_eq!(store.task(id + 1).unwrap(), None);
}

#[test]
fn a_task_keeps_the_last_2000_lines_of_its_log() {
    let store = Store::in_memory();
    let id = new_task(&store, 1);
    let mut task = store.task(id).unwrap().unwrap();
    task.log = (0..LOG_MAX + 50).map(|number| format!("line {number}")).collect();
    store.save_task(&task).unwrap();
    let kept = store.task(id).unwrap().unwrap().log;
    assert_eq!(kept.len(), LOG_MAX);
    assert_eq!(kept[0], "line 50");
    assert_eq!(kept[LOG_MAX - 1], format!("line {}", LOG_MAX + 49));
}

#[test]
fn the_list_has_what_waits_or_runs_and_what_ended_lately() {
    let store = Store::in_memory();
    let (old, recent, running, waiting) =
        (new_task(&store, 1), new_task(&store, 2), new_task(&store, 3), new_task(&store, 4));
    for (id, state, ended) in [(old, State::Done, 100), (recent, State::Failed, 900)] {
        let mut task = store.task(id).unwrap().unwrap();
        (task.state, task.ended) = (state, Some(ended));
        store.save_task(&task).unwrap();
    }
    let mut task = store.task(running).unwrap().unwrap();
    task.state = State::Running;
    store.save_task(&task).unwrap();
    let ids =
        |since| store.tasks(since, 10).unwrap().iter().map(|task| task.id).collect::<Vec<_>>();
    assert_eq!(ids(500), [waiting, running, recent]);
    assert_eq!(ids(0), [waiting, running, recent, old]);
    assert_eq!(store.tasks(0, 2).unwrap().len(), 2);
}

#[test]
fn tasks_left_over_end_as_stopped_with_the_reason() {
    let store = Store::in_memory();
    let (waiting, running, done) = (new_task(&store, 1), new_task(&store, 2), new_task(&store, 3));
    let mut task = store.task(running).unwrap().unwrap();
    task.state = State::Running;
    store.save_task(&task).unwrap();
    let mut task = store.task(done).unwrap().unwrap();
    (task.state, task.ended, task.status) = (State::Done, Some(50), Some(0));
    store.save_task(&task).unwrap();
    assert_eq!(store.stop_unfinished(777, "the console restarted").unwrap(), 2);
    for id in [waiting, running] {
        let task = store.task(id).unwrap().unwrap();
        assert_eq!(task.state, State::Stopped);
        assert_eq!(task.ended, Some(777));
        assert_eq!(task.note.as_deref(), Some("the console restarted"));
    }
    let task = store.task(done).unwrap().unwrap();
    assert_eq!((task.state, task.ended, task.note), (State::Done, Some(50), None));
    assert_eq!(store.stop_unfinished(800, "again").unwrap(), 0);
}

#[test]
fn a_task_is_forgotten_30_days_after_it_ended() {
    let store = Store::in_memory();
    let day = 86_400;
    let now = 100 * day;
    let (old, young, running) = (new_task(&store, 1), new_task(&store, 2), new_task(&store, 3));
    for (id, ended) in [(old, now - 31 * day), (young, now - 29 * day)] {
        let mut task = store.task(id).unwrap().unwrap();
        (task.state, task.ended) = (State::Done, Some(ended));
        store.save_task(&task).unwrap();
    }
    assert_eq!(store.forget_old_tasks(now).unwrap(), 1);
    assert_eq!(store.task(old).unwrap(), None);
    assert!(store.task(young).unwrap().is_some());
    assert!(store.task(running).unwrap().is_some());
}
