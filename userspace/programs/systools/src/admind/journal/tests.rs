// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::cert::fixtures::Scratch;

/// A machine's settings in a folder of the test's own, a journal that
/// watches them, and a store.
struct Machine {
    folder: Scratch,
    journal: Journal,
    store: Store,
}

impl Machine {
    fn new(what: &str) -> Machine {
        let folder = Scratch::new(what);
        let settings = folder.0.join("settings");
        fs::create_dir_all(&settings).unwrap();
        let journal = Journal::new(&settings);
        Machine { folder, journal, store: Store::in_memory() }
    }

    /// The path `name` has under the settings.
    fn path(&self, name: &str) -> String {
        self.folder.0.join("settings").join(name).to_string_lossy().into_owned()
    }

    /// One round at the Unix time `now`, whose people are `who`, and the
    /// rows it made.
    fn round(&self, now: i64, who: &[(&str, &str)]) -> Vec<Change> {
        let scan = self.journal.scan(&self.store, now).unwrap();
        let who = who
            .iter()
            .map(|(path, who)| {
                (path.to_string(), Editor { name: who.to_string(), in_terminal: false })
            })
            .collect();
        let before = self.store.changes(0, 1000).unwrap().len();
        self.journal.record(&self.store, &scan, &who, now).unwrap();
        let rows = self.store.changes(0, 1000).unwrap();
        rows[..rows.len() - before].to_vec()
    }

    /// The rows that recording `scan`, by no one, at the Unix time `now`
    /// made.
    fn round_of(&self, scan: &Scan, now: i64) -> Vec<Change> {
        let before = self.store.changes(0, 1000).unwrap().len();
        self.journal.record(&self.store, scan, &BTreeMap::new(), now).unwrap();
        let rows = self.store.changes(0, 1000).unwrap();
        rows[..rows.len() - before].to_vec()
    }
}

use crate::admind::store::changes::Change;

/// Writes `text` to `path`, whose folders are made, and gives it the
/// modification time `seconds`.
fn put(path: &str, text: &str, seconds: u64) {
    fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds)).unwrap();
}

fn text(bytes: &str) -> Seen {
    Seen::Text { sha256: sha256(bytes.as_bytes()), size: bytes.len() as u64 }
}

#[test]
fn a_file_is_looked_at_and_its_text_kept_as_a_version() {
    let machine = Machine::new("look-text");
    let path = machine.path("a.toml");
    put(&path, "one = 1\n", 1_000);
    let (seen, stamp) = look(&machine.store, &path, 50).unwrap();
    assert_eq!(seen, text("one = 1\n"));
    assert_eq!(stamp, Some(Stamp { size: 8, mtime: 1_000_000_000_000 }));
    assert_eq!(stamp.unwrap().seconds(), 1_000);
    let kept = machine.store.version_text(seen.hash().unwrap()).unwrap();
    assert_eq!(kept.as_deref(), Some(&b"one = 1\n"[..]));
}

#[test]
fn a_missing_file_is_absent_and_a_folder_is_not_a_file() {
    let machine = Machine::new("look-absent");
    let (seen, stamp) = look(&machine.store, &machine.path("none"), 1).unwrap();
    assert_eq!((seen, stamp), (Seen::Absent, None));
    let folder = machine.path("folder");
    fs::create_dir_all(&folder).unwrap();
    assert_eq!(look(&machine.store, &folder, 1).unwrap().0, Seen::Unreadable);
}

#[test]
fn a_secret_and_a_file_not_everyone_reads_are_never_read() {
    let machine = Machine::new("look-secret");
    let private = machine.path("private.toml");
    put(&private, "password = \"x\"\n", 1);
    fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
    let (seen, stamp) = look(&machine.store, &private, 1).unwrap();
    assert_eq!(seen, Seen::Secret);
    assert!(stamp.is_some());
    assert!(machine.store.versions_of(&private).unwrap().is_empty());
    assert_eq!(machine.store.text_size().unwrap(), 0);
    // One of a service's secrets is a secret by its place, and not read
    // even where it could be.
    let secret = format!("{SECRETS}/web/api-key");
    assert!(is_secret(&secret));
    assert!(!is_secret(&format!("{SECRETS}-not/web")));
    assert_eq!(look(&machine.store, &secret, 1).unwrap(), (Seen::Absent, None));
}

#[test]
fn a_file_longer_than_the_store_keeps_has_its_hash_and_size_and_no_text() {
    let mut machine = Machine::new("look-large");
    machine.store.limits.file = 16;
    let (big, exact) = (machine.path("big"), machine.path("exact"));
    put(&big, &"x".repeat(100), 1);
    put(&exact, &"y".repeat(16), 1);
    let (seen, _) = look(&machine.store, &big, 1).unwrap();
    assert_eq!(seen, Seen::Large { sha256: sha256("x".repeat(100).as_bytes()), size: 100 });
    assert_eq!(machine.store.text_size().unwrap(), 0);
    assert_eq!(look(&machine.store, &exact, 1).unwrap().0, text(&"y".repeat(16)));
}

#[test]
fn the_first_round_notes_what_is_there_and_makes_no_row() {
    let machine = Machine::new("first");
    put(&machine.path("a.toml"), "a\n", 100);
    put(&machine.path("spark/services/b.toml"), "b\n", 100);
    assert!(machine.round(1_000, &[]).is_empty());
    assert_eq!(machine.store.watched().unwrap().len(), 2);
    // Nothing changed since: no row, and nothing hashed again.
    assert!(machine.round(1_030, &[]).is_empty());
}

#[test]
fn a_file_changed_by_hand_has_a_row_with_its_two_sides_and_the_person() {
    let machine = Machine::new("by-hand");
    let path = machine.path("admin.toml");
    put(&path, "reach = \"private\"\n", 100);
    machine.round(1_000, &[]);
    put(&path, "reach = \"any\"\n", 1_020);
    let rows = machine.round(1_030, &[(&path, "chase")]);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!((row.who.as_str(), row.how.as_str(), row.at), ("chase", "by hand", 1_030));
    assert_eq!((row.action.as_deref(), row.status, row.task), (None, None, None));
    assert!(row.by_hand());
    assert_eq!(
        row.files,
        [FileChange {
            path: path.clone(),
            before: text("reach = \"private\"\n"),
            after: text("reach = \"any\"\n"),
        }]
    );
    // Both texts are kept, for the diff and for put-back.
    let hash = |seen: &Seen| machine.store.version_text(seen.hash().unwrap()).unwrap();
    assert_eq!(hash(&row.files[0].before).as_deref(), Some(&b"reach = \"private\"\n"[..]));
    assert_eq!(hash(&row.files[0].after).as_deref(), Some(&b"reach = \"any\"\n"[..]));
    // And it is noted: the next round has nothing.
    assert!(machine.round(1_060, &[]).is_empty());
}

#[test]
fn a_change_no_one_is_found_for_is_outside_the_console() {
    let machine = Machine::new("nobody");
    let path = machine.path("a.toml");
    put(&path, "a\n", 100);
    machine.round(1_000, &[]);
    put(&path, "b\n", 1_010);
    let rows = machine.round(1_030, &[]);
    assert_eq!(rows[0].who, "outside the console");
}

#[test]
fn a_change_in_an_administrators_terminal_has_a_row_of_its_own_that_says_so() {
    let machine = Machine::new("terminal");
    let (a, b) = (machine.path("a"), machine.path("b"));
    for path in [&a, &b] {
        put(path, "old\n", 100);
    }
    machine.round(1_000, &[]);
    for path in [&a, &b] {
        put(path, "new\n", 1_010);
    }
    let scan = machine.journal.scan(&machine.store, 1_030).unwrap();
    let elevated = Editor { name: "alice".into(), in_terminal: false };
    let mut who = BTreeMap::from([(a.clone(), elevated)]);
    let rooted = [Rooted { account: "alice".into(), started: 1_005, ended: None }];
    audit::add_terminal_editors(&mut who, &rooted, &scan.times(1_030));
    machine.journal.record(&machine.store, &scan, &who, 1_030).unwrap();
    let rows = machine.store.changes(0, 10).unwrap();
    let mut found: Vec<(&str, Option<&str>, &str)> = rows[..2]
        .iter()
        .map(|row| (row.who.as_str(), row.note.as_deref(), row.files[0].path.as_str()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [("alice", None, a.as_str()), ("alice", Some(audit::IN_TERMINAL), b.as_str())]
    );
}

#[test]
fn files_changed_in_one_round_have_a_row_for_each_person() {
    let machine = Machine::new("people");
    let (a, b, c, d) = (machine.path("a"), machine.path("b"), machine.path("c"), machine.path("d"));
    for path in [&a, &b, &c, &d] {
        put(path, "old\n", 100);
    }
    machine.round(1_000, &[]);
    for path in [&a, &b, &c, &d] {
        put(path, "new\n", 1_010);
    }
    let rows = machine.round(1_030, &[(&a, "alice"), (&b, "bob"), (&c, "alice")]);
    let mut found: Vec<(String, usize)> =
        rows.iter().map(|row| (row.who.clone(), row.files.len())).collect();
    found.sort();
    assert_eq!(
        found,
        [("alice".to_string(), 2), ("bob".to_string(), 1), ("outside the console".to_string(), 1)]
    );
}

#[test]
fn a_file_touched_and_not_changed_has_no_row() {
    let machine = Machine::new("touched");
    let path = machine.path("a.toml");
    put(&path, "same\n", 100);
    machine.round(1_000, &[]);
    put(&path, "same\n", 1_010);
    assert!(machine.round(1_030, &[]).is_empty());
    assert_eq!(machine.store.watched().unwrap()[&path].mtime, 1_010_000_000_000);
}

#[test]
fn a_file_made_by_hand_and_one_removed_have_rows_too() {
    let machine = Machine::new("made-removed");
    let old = machine.path("old.toml");
    put(&old, "old\n", 100);
    machine.round(1_000, &[]);
    let new = machine.path("sub/new.toml");
    put(&new, "new\n", 1_010);
    fs::remove_file(&old).unwrap();
    let rows = machine.round(1_030, &[]);
    let files: Vec<&FileChange> = rows.iter().flat_map(|row| &row.files).collect();
    assert_eq!(files.len(), 2);
    let made = files.iter().find(|file| file.path == new).unwrap();
    assert_eq!((&made.before, &made.after), (&Seen::Absent, &text("new\n")));
    let removed = files.iter().find(|file| file.path == old).unwrap();
    assert_eq!((&removed.before, &removed.after), (&text("old\n"), &Seen::Absent));
    // The removed file is no longer watched, and the made one is.
    let watched = machine.store.watched().unwrap();
    assert!(watched.contains_key(&new) && !watched.contains_key(&old));
    assert!(machine.round(1_060, &[]).is_empty());
}

#[test]
fn a_secret_is_listed_by_name_when_it_changes_and_a_file_that_a_dot_starts_is_not_looked_at() {
    let machine = Machine::new("secret-dot");
    let secret = machine.path("secret.toml");
    put(&secret, "one\n", 100);
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    machine.round(1_000, &[]);
    put(&secret, "two\n", 1_010);
    put(&machine.path(".editor.swp"), "x\n", 1_010);
    let rows = machine.round(1_030, &[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].files.len(), 1);
    let file = &rows[0].files[0];
    assert_eq!((&file.before, &file.after), (&Seen::Secret, &Seen::Secret));
    assert_eq!(machine.store.text_size().unwrap(), 0);
}

#[test]
fn a_console_change_has_a_row_with_the_files_before_and_after_and_no_hand_edit() {
    let machine = Machine::new("console");
    let path = machine.path("spark/services/web.toml");
    put(&path, "enabled = false\n", 100);
    machine.round(1_000, &[]);
    let argv = ["service".to_string(), "web".into(), "enable".into()];
    let begun = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1_010);
    // The command writes the file, and a round comes while it runs.
    put(&path, "enabled = true\n", 1_012);
    assert!(machine.round(1_015, &[]).is_empty());
    let ended = Ended {
        who: "alice",
        how: "console",
        action: "service.enable",
        argv: &argv,
        status: Some(0),
        task: None,
        note: None,
    };
    let ending = begun.end(Some(&machine.store), &ended, 1_020);
    // The file moved, so the change changed something.
    assert!(!ending.same);
    let id = ending.row.unwrap();
    let row = machine.store.change(id).unwrap().unwrap();
    assert_eq!((row.who.as_str(), row.how.as_str(), row.at), ("alice", "console", 1_020));
    assert_eq!((row.action.as_deref(), row.status), (Some("service.enable"), Some(0)));
    assert_eq!(row.argv.as_deref(), Some(&argv[..]));
    assert_eq!(row.files[0].before, text("enabled = false\n"));
    assert_eq!(row.files[0].after, text("enabled = true\n"));
    // The round that comes after finds what the console wrote noted.
    assert!(machine.round(1_045, &[]).is_empty());
    assert!(machine.journal.pending.borrow().is_empty());
}

#[test]
fn a_command_that_changed_nothing_still_names_its_files_and_one_not_run_has_no_row() {
    let machine = Machine::new("console-same");
    let path = machine.path("a.toml");
    put(&path, "a\n", 100);
    let ended = |note| Ended {
        who: "alice",
        how: "token ci",
        action: "x.y",
        argv: &[],
        status: Some(1),
        task: Some(4),
        note,
    };
    let begun = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1);
    let ending = begun.end(Some(&machine.store), &ended(Some("it failed")), 2);
    assert!(ending.same);
    let id = ending.row.unwrap();
    let row = machine.store.change(id).unwrap().unwrap();
    assert_eq!((row.status, row.task, row.note.as_deref()), (Some(1), Some(4), Some("it failed")));
    assert_eq!(row.files[0].before, row.files[0].after);
    let begun = machine.journal.begin(Some(&machine.store), &[path], 3);
    assert_eq!(machine.journal.pending.borrow().len(), 1);
    begun.cancel();
    assert!(machine.journal.pending.borrow().is_empty());
    assert_eq!(machine.store.changes(0, 10).unwrap().len(), 1);
}

#[test]
fn a_file_a_console_change_takes_while_the_person_is_looked_for_is_left_to_it() {
    let machine = Machine::new("race");
    let path = machine.path("a.toml");
    put(&path, "a\n", 100);
    machine.round(1_000, &[]);
    put(&path, "b\n", 1_010);
    let scan = machine.journal.scan(&machine.store, 1_030).unwrap();
    assert_eq!(scan.edits.len(), 1);
    // While logd is asked, a console change of the file runs and ends.
    let begun = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1_031);
    put(&path, "c\n", 1_032);
    let ended = Ended {
        who: "alice",
        how: "console",
        action: "x.y",
        argv: &[],
        status: Some(0),
        task: None,
        note: None,
    };
    begun.end(Some(&machine.store), &ended, 1_033).row.unwrap();
    let made = machine.journal.record(&machine.store, &scan, &BTreeMap::new(), 1_034).unwrap();
    assert_eq!(made, 0);
    // The console's change is the only row, and its state is the one noted.
    assert_eq!(machine.store.changes(0, 10).unwrap().len(), 1);
    assert!(machine.round(1_060, &[]).is_empty());
}

#[test]
fn a_file_two_changes_are_under_way_on_is_left_alone_until_both_have_ended() {
    let machine = Machine::new("two-pending");
    let path = machine.path("a.toml");
    put(&path, "a\n", 100);
    machine.round(1_000, &[]);
    let one = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1_001);
    let two = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1_002);
    put(&path, "b\n", 1_003);
    drop(one);
    assert!(machine.round(1_030, &[]).is_empty());
    drop(two);
    assert_eq!(machine.round(1_060, &[]).len(), 1);
}

#[test]
fn a_row_that_ends_is_announced_once_with_its_number_how_and_time() {
    let machine = Machine::new("news");
    let path = machine.path("a.toml");
    put(&path, "a\n", 100);
    machine.round(1_000, &[]);
    assert_eq!(machine.journal.news.since(0).1, []);
    // A console change.
    let begun = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1_010);
    put(&path, "b\n", 1_012);
    let ended = Ended {
        who: "alice",
        how: "console",
        action: "x.y",
        argv: &[],
        status: Some(0),
        task: None,
        note: None,
    };
    let id = begun.end(Some(&machine.store), &ended, 1_020).row.unwrap();
    // A hand edit, in the next round.
    put(&path, "c\n", 1_030);
    let hand = machine.round(1_040, &[(&path, "chase")]);
    assert_eq!(hand.len(), 1);
    let (_, told) = machine.journal.news.since(0);
    assert_eq!(
        told,
        [
            Row { id, how: "console".into(), at: 1_020 },
            Row { id: hand[0].id, how: "by hand".into(), at: 1_040 },
        ]
    );
    // A round with nothing to record announces nothing.
    machine.round(1_070, &[]);
    assert_eq!(machine.journal.news.since(0).1.len(), 2);
}

#[test]
fn a_row_by_token_is_announced_as_the_token_made_it_and_a_row_not_kept_is_not() {
    let machine = Machine::new("news-token");
    let path = machine.path("a.toml");
    put(&path, "a\n", 100);
    let ended = |how| Ended {
        who: "alice",
        how,
        action: "x.y",
        argv: &[],
        status: Some(1),
        task: Some(3),
        note: None,
    };
    let begun = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 1);
    let id = begun.end(Some(&machine.store), &ended("token ci"), 2).row.unwrap();
    assert_eq!(machine.journal.news.since(0).1, [Row { id, how: "token ci".into(), at: 2 }]);
    // A change cancelled has no row, and none with no store.
    machine.journal.begin(Some(&machine.store), std::slice::from_ref(&path), 3).cancel();
    machine.journal.begin(None, std::slice::from_ref(&path), 4).end(None, &ended("console"), 5);
    assert_eq!(machine.journal.news.since(0).1.len(), 1);
}

#[test]
fn a_change_with_no_store_is_not_kept_and_the_files_are_still_let_go() {
    let journal = Journal::new(Path::new("/nonexistent"));
    let begun = journal.begin(None, &["/a".to_string()], 1);
    assert_eq!(journal.pending.borrow().len(), 1);
    let ended = Ended {
        who: "alice",
        how: "console",
        action: "x.y",
        argv: &[],
        status: Some(0),
        task: None,
        note: None,
    };
    assert_eq!(begun.end(None, &ended, 2), Ending { row: None, same: false });
    assert!(journal.pending.borrow().is_empty());
}

#[test]
fn a_folder_an_action_named_a_file_in_is_watched_too() {
    let machine = Machine::new("named");
    let outside = machine.folder.0.join("bundle/web/settings");
    let named = outside.join("web.toml").to_string_lossy().into_owned();
    let other = outside.join("other.toml").to_string_lossy().into_owned();
    put(&named, "a\n", 100);
    put(&other, "o\n", 100);
    machine.round(1_000, &[]);
    // Nothing outside the settings is watched until an action names a file
    // there.
    put(&other, "o2\n", 1_010);
    assert!(machine.round(1_030, &[]).is_empty());
    let argv: Vec<String> = Vec::new();
    let ended = Ended {
        who: "alice",
        how: "console",
        action: "bundle.set",
        argv: &argv,
        status: Some(0),
        task: None,
        note: None,
    };
    let begun = machine.journal.begin(Some(&machine.store), std::slice::from_ref(&named), 1_040);
    begun.end(Some(&machine.store), &ended, 1_041).row.unwrap();
    // Now its folder is watched: the sibling that was there is not a
    // change, one made after is, and so is one changed after.
    assert!(machine.round(1_060, &[]).is_empty());
    put(&other, "o3\n", 1_070);
    let rows = machine.round(1_090, &[]);
    assert_eq!((rows.len(), rows[0].files[0].path.as_str()), (1, other.as_str()));
    let made = outside.join("made.toml").to_string_lossy().into_owned();
    put(&made, "m\n", 1_100);
    let rows = machine.round(1_120, &[]);
    assert_eq!((rows.len(), rows[0].files[0].before.clone()), (1, Seen::Absent));
}

#[test]
fn the_round_names_each_change_s_path_and_time_for_the_audit_log() {
    let machine = Machine::new("times");
    let (a, b) = (machine.path("a"), machine.path("b"));
    put(&a, "1\n", 100);
    put(&b, "1\n", 100);
    machine.round(1_000, &[]);
    put(&a, "2\n", 1_010);
    // A file that is changed, and made in the future of the clock.
    put(&b, "2\n", 9_000);
    fs::remove_file(machine.path("none")).ok();
    let scan = machine.journal.scan(&machine.store, 1_030).unwrap();
    assert_eq!(scan.times(1_030), [(a, 1_010), (b, 1_030)]);
}

/// A change of `path` from `before` to `after`, as the store would hold
/// it.
fn change_of(files: Vec<FileChange>) -> Change {
    Change {
        id: 1,
        at: 5,
        who: "alice".into(),
        how: "console".into(),
        action: None,
        argv: None,
        status: Some(0),
        task: None,
        note: None,
        files,
        seen: None,
    }
}

/// A file whose earlier text the store keeps.
fn kept(machine: &Machine, path: &str, before: &str, after: &str) -> FileChange {
    machine.store.keep_version(path, before.as_bytes(), 1).unwrap();
    FileChange { path: path.into(), before: text(before), after: text(after) }
}

#[test]
fn a_change_is_put_back_by_the_earlier_text_of_each_file_it_changed() {
    let machine = Machine::new("restore");
    let files = vec![
        kept(&machine, "/a", "one\n", "two\n"),
        // The same on both sides: nothing to write.
        kept(&machine, "/same", "x\n", "x\n"),
        kept(&machine, "/b", "three\n", "four\n"),
    ];
    let plan = restore_plan(&machine.store, &change_of(files)).unwrap();
    assert_eq!(
        plan,
        [
            Restore { path: "/a".into(), text: b"one\n".to_vec() },
            Restore { path: "/b".into(), text: b"three\n".to_vec() },
        ]
    );
}

#[test]
fn a_change_with_no_earlier_text_to_write_says_why_and_writes_nothing() {
    let machine = Machine::new("restore-refused");
    let ok = kept(&machine, "/ok", "a\n", "b\n");
    let refused = |before: Seen, want: &str| {
        let file = FileChange { path: "/f".into(), before, after: text("new\n") };
        let why = restore_plan(&machine.store, &change_of(vec![ok.clone(), file])).unwrap_err();
        assert!(why.contains("/f"), "{why}");
        assert!(why.contains(want), "{why}");
    };
    refused(Seen::Absent, "was not there before");
    refused(Seen::Large { sha256: sha256(b"l"), size: 2 << 20 }, "longer than 1 MiB");
    refused(Seen::Secret, "secret");
    refused(Seen::Unreadable, "could not read");
    // A text that is a version no more: the store keeps the last 20.
    refused(text("forgotten\n"), "not kept any more");
    let none = restore_plan(&machine.store, &change_of(vec![])).unwrap_err();
    assert!(none.contains("nothing to put back"), "{none}");
    let same = FileChange { path: "/s".into(), before: text("x\n"), after: text("x\n") };
    assert!(restore_plan(&machine.store, &change_of(vec![same])).is_err());
}

#[test]
fn the_account_files_are_a_row_of_their_own_which_is_never_put_back() {
    // An account made at the first sign-in, and a service file changed by
    // hand, in the same round and by no one the audit names: put back, one
    // row would take the account away with the service's setting.
    let machine = Machine::new("accounts");
    let service = machine.path("spark/services/diskhealth.toml");
    let made = |path: &str, body: &str| {
        machine.store.keep_version(path, body.as_bytes(), 1).unwrap();
        Edited { path: path.into(), before: Seen::Absent, after: text(body), stamp: None }
    };
    let scan = Scan {
        edits: vec![
            made(accounts::GROUP, "admins:x:10:alice\n"),
            made(accounts::PASSWD, "alice:x:1000:1000::/users/alice:/x\n"),
            made(&service, "enabled = false\n"),
        ],
    };
    let rows = machine.round_of(&scan, 1_030);
    let found: Vec<Vec<&str>> =
        rows.iter().map(|row| row.files.iter().map(|f| f.path.as_str()).collect()).collect();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.contains(&vec![accounts::GROUP, accounts::PASSWD]), "{found:?}");
    assert!(found.contains(&vec![service.as_str()]), "{found:?}");
    assert!(rows.iter().all(|row| row.who == "outside the console"));
    // Neither file is ever written back, alone or with others.
    let ok = kept(&machine, "/ok", "a\n", "b\n");
    for path in [accounts::PASSWD, accounts::GROUP] {
        let file = kept(&machine, path, "before\n", "after\n");
        let why = restore_plan(&machine.store, &change_of(vec![ok.clone(), file])).unwrap_err();
        assert!(why.starts_with(&format!("{path} is one of the account files")), "{why}");
        assert!(why.contains("elevate user"), "{why}");
    }
}

#[test]
fn a_kept_text_that_no_longer_matches_its_hash_is_not_written() {
    let machine = Machine::new("restore-damaged");
    let file = kept(&machine, "/a", "one\n", "two\n");
    let Seen::Text { sha256: hash, .. } = &file.before else { panic!("text") };
    machine.store.damage_blob_for_test(hash, b"other\n");
    let why = restore_plan(&machine.store, &change_of(vec![file])).unwrap_err();
    assert!(why.contains("not kept any more"), "{why}");
}

#[test]
fn the_settings_files_are_the_regular_files_without_a_dot_and_the_folders_not_opened_are_told() {
    let machine = Machine::new("settings-files");
    let settings = machine.folder.0.join("settings");
    put(&machine.path("admin.toml"), "a = 1\n", 1);
    put(&machine.path("spark/services/web.toml"), "b = 2\n", 1);
    put(&machine.path(".editing.toml"), "half", 1);
    let found = settings_files(&settings);
    let names: Vec<&str> = found
        .files
        .iter()
        .map(|path| path.strip_prefix(settings.to_str().unwrap()).unwrap())
        .collect();
    assert_eq!(names, ["/admin.toml", "/spark/services/web.toml"]);
    assert!(found.closed.is_empty());

    // A folder that is not there is the one that was not opened.
    let gone = settings_files(&machine.folder.0.join("none"));
    assert!(gone.files.is_empty());
    assert_eq!(gone.closed, [machine.folder.0.join("none").to_string_lossy().into_owned()]);
}

#[test]
fn a_secret_and_a_file_not_everyone_may_read_are_hidden() {
    let machine = Machine::new("is-hidden");
    let open = machine.path("open.toml");
    let closed = machine.path("closed.toml");
    put(&open, "x", 1);
    put(&closed, "y", 1);
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(!is_hidden(&open, &fs::metadata(&open).unwrap()));
    assert!(is_hidden(&closed, &fs::metadata(&closed).unwrap()));
    assert!(is_hidden(&format!("{SECRETS}/web/key"), &fs::metadata(&open).unwrap()));
}

#[test]
fn a_link_is_not_followed_nor_watched() {
    let machine = Machine::new("look-link");
    let outside = machine.folder.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let target = outside.join("key.toml");
    put(target.to_str().unwrap(), "key = \"theirs\"\n", 1);
    let link = machine.path("linked.toml");
    fs::create_dir_all(Path::new(&link).parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    // The link itself is not read, and its target's text is not kept.
    let (seen, _) = look(&machine.store, &link, 1).unwrap();
    assert_eq!(seen, Seen::Unreadable);
    assert_eq!(machine.store.text_size().unwrap(), 0);
    // A link where a folder of the path is leads nowhere either.
    let folder_link = machine.path("through");
    std::os::unix::fs::symlink(&outside, &folder_link).unwrap();
    let through = format!("{folder_link}/key.toml");
    assert_eq!(look(&machine.store, &through, 1).unwrap(), (Seen::Unreadable, None));
    // The round does not list either link.
    let listed = settings_files(Path::new(&machine.path("")));
    assert!(listed.files.iter().all(|path| !path.contains("linked") && !path.contains("through")));
}

#[test]
fn a_file_in_a_folder_others_may_not_pass_is_hidden() {
    let machine = Machine::new("look-closed-folder");
    let inside = machine.path("closed/inside.toml");
    put(&inside, "token = \"t\"\n", 1);
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o644)).unwrap();
    let closed = Path::new(&inside).parent().unwrap();
    fs::set_permissions(closed, fs::Permissions::from_mode(0o750)).unwrap();
    let (seen, stamp) = look(&machine.store, &inside, 1).unwrap();
    assert_eq!(seen, Seen::Secret);
    assert!(stamp.is_some());
    assert_eq!(machine.store.text_size().unwrap(), 0);
    fs::set_permissions(closed, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(look(&machine.store, &inside, 1).unwrap().0, text("token = \"t\"\n"));
}

#[test]
fn a_pipe_is_not_opened() {
    let machine = Machine::new("look-pipe");
    let pipe = machine.path("fifo");
    fs::create_dir_all(Path::new(&pipe).parent().unwrap()).unwrap();
    let name = std::ffi::CString::new(pipe.clone()).unwrap();
    // SAFETY: the name is a C string.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
    // Opening it to read would wait for a writer: it is not opened.
    let (seen, stamp) = look(&machine.store, &pipe, 1).unwrap();
    assert_eq!(seen, Seen::Unreadable);
    assert!(stamp.is_some());
}

#[test]
fn an_undo_spark_made_by_itself_has_a_row_of_its_own_once_and_is_no_hand_edit() {
    use crate::admind::trials::Trials;
    use spark_api::show::{Show, Undone};
    use spark_api::trial::Why;

    let machine = Machine::new("undone");
    let path = machine.path("network");
    let (dhcp, fixed) = ("[net0]\nmode = \"dhcp\"\n", "[net0]\nmode = \"fixed\"\n");
    put(&path, dhcp, 100);
    machine.round(1_000, &[]);
    // Tried at a terminal: the round's hand edit.
    put(&path, fixed, 1_010);
    assert_eq!(machine.round(1_030, &[(&path, "chase")]).len(), 1);
    // Not kept: Spark puts the copy back, 10 seconds before admind looks.
    put(&path, dhcp, 1_070);
    let document = Show {
        now_ms: 80_000,
        undone: vec![Undone {
            id: "network".into(),
            why: Why::TimeUp.words(),
            at_ms: 70_000,
            files: vec![path.clone()],
        }],
        ..Show::default()
    };
    let undos = Trials::default().undos(&document, 1_080);
    let row = machine.journal.undone(&machine.store, &undos[0], 1_080).unwrap().unwrap();
    let change = machine.store.change(row).unwrap().unwrap();
    assert_eq!(
        (change.who.as_str(), change.how.as_str(), change.at, change.note.as_deref()),
        ("Spark", "undone", 1_070, Some("it was not kept in time"))
    );
    assert_eq!((change.action, change.argv, change.status), (None, None, None));
    assert_eq!(change.files, [FileChange { path, before: text(fixed), after: text(dhcp) }]);
    // The file's change back is the undo's, and no hand edit.
    assert!(machine.round(1_090, &[]).is_empty());
    // admind started again, and Spark lists the undo still: the journal
    // has its row, whose time it reads a second apart from Spark's clock.
    let later = Show { now_ms: 102_500, ..document };
    let again = Trials::default().undos(&later, 1_101);
    assert_eq!(again[0].at, 1_069);
    assert_eq!(machine.journal.undone(&machine.store, &again[0], 1_101).unwrap(), None);
    assert_eq!(machine.store.changes(0, 100).unwrap().len(), 2);
}

#[test]
fn check_mode_reads_a_file_everyone_reads_and_tells_nothing_of_the_rest() {
    let machine = Machine::new("check-read");
    let open = machine.path("open.toml");
    put(&open, "enabled = true\n", 1);
    assert_eq!(current_text(&open), Some(Some("enabled = true\n".to_string())));
    assert_eq!(current_text(&machine.path("absent.toml")), Some(None));
    // One that not everyone reads, one that is not text, one larger than
    // a check reads and a link: nothing to tell, and nothing kept.
    let closed = machine.path("closed.toml");
    put(&closed, "secret = 1\n", 1);
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(current_text(&closed), None);
    let binary = machine.path("binary.toml");
    fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(current_text(&binary), None);
    let large = machine.path("large.toml");
    fs::write(&large, vec![b'#'; usize::try_from(CHECKED_MAX).unwrap() + 1]).unwrap();
    assert_eq!(current_text(&large), None);
    let link = machine.path("linked.toml");
    std::os::unix::fs::symlink(&open, &link).unwrap();
    assert_eq!(current_text(&link), None);
    assert_eq!(current_text("relative.toml"), None);
    assert_eq!(machine.store.text_size().unwrap(), 0);
}
