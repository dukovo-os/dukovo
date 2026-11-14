// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::admind::store::changes::{FileChange, NewChange};
use crate::cert::fixtures::Scratch;

/// A machine's files in a folder of the test's own, a store, and what
/// Spark was asked.
struct Machine {
    folder: Scratch,
    store: Store,
    asked: RefCell<Vec<String>>,
}

impl Machine {
    fn new(what: &str) -> Machine {
        let folder = Scratch::new(what);
        fs::create_dir_all(folder.0.join("services/settings.d")).unwrap();
        fs::create_dir_all(folder.0.join("records")).unwrap();
        Machine { folder, store: Store::in_memory(), asked: RefCell::new(Vec::new()) }
    }

    fn places(&self) -> Places {
        Places {
            service_files: self.folder.0.join("services"),
            records: self.folder.0.join("records"),
        }
    }

    /// The path of `name` in the folder.
    fn path(&self, name: &str) -> String {
        self.folder.0.join(name).to_string_lossy().into_owned()
    }

    /// A file whose text was `before` and is `after` now, changed by hand,
    /// as a row of the journal, which keeps `before`.
    fn changed(&self, path: &str, before: &str, after: &str) -> i64 {
        fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
        fs::write(path, after).unwrap();
        self.row(path, Seen::Text { sha256: sha256(before.as_bytes()), size: 0 }, after)
            .inspect(|_| {
                self.store.keep_version(path, before.as_bytes(), 1).unwrap();
            })
            .unwrap()
    }

    /// A row that names `path` from `before` to the text `after`.
    fn row(&self, path: &str, before: Seen, after: &str) -> Option<i64> {
        let files = [FileChange {
            path: path.into(),
            before,
            after: Seen::Text { sha256: sha256(after.as_bytes()), size: after.len() as u64 },
        }];
        self.store
            .add_change(&NewChange {
                at: 10,
                who: "alice",
                how: "by hand",
                action: None,
                argv: None,
                status: None,
                task: None,
                note: None,
                files: &files,
            })
            .ok()
    }

    /// Puts change `id` back, with Spark answering `answer` to each
    /// request, and gives the exit status or the failure, and what it said.
    fn put_back(&self, id: i64, answer: &str) -> (Result<i32, Fail>, String) {
        let ask = |request: &str| {
            self.asked.borrow_mut().push(request.to_string());
            if answer == "unreachable" {
                Err(fail("/system/live/spark/control: Connection refused".into()))
            } else {
                Ok(answer.to_string())
            }
        };
        let mut said = Vec::new();
        let done = put_back(&self.store, id, &self.places(), &ask, &mut said);
        (done, String::from_utf8(said).unwrap())
    }
}

/// A change of a file kept as `before` before it, which the store holds
/// the text of, to the text `after`.
fn text(bytes: &str) -> Seen {
    Seen::Text { sha256: sha256(bytes.as_bytes()), size: bytes.len() as u64 }
}

#[test]
fn the_earlier_text_is_written_back_and_what_it_did_is_said() {
    let machine = Machine::new("put-back");
    let path = machine.path("services/web.toml");
    let id = machine.changed(&path, "enabled = false\n", "enabled = true\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let (done, said) = machine.put_back(id, "ok: asked web to reload\n");
    assert_eq!(done.unwrap(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), "enabled = false\n");
    // The file keeps its mode, as settings_edit::write keeps it.
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
    assert_eq!(
        said,
        format!("put back {path}\nasked web to reload\nchange {id} is put back: 1 file\n")
    );
    assert_eq!(*machine.asked.borrow(), ["reload web"]);
}

#[test]
fn a_second_put_back_writes_nothing_and_says_so() {
    let machine = Machine::new("put-back-twice");
    let path = machine.path("services/web.toml");
    let id = machine.changed(&path, "a\n", "b\n");
    assert_eq!(machine.put_back(id, "ok: x\n").0.unwrap(), 0);
    let (done, said) = machine.put_back(id, "ok: x\n");
    assert_eq!(done.unwrap(), 0);
    assert!(said.starts_with(&format!("{path} holds the earlier text already\n")), "{said}");
}

#[test]
fn a_file_changed_again_since_is_written_over_and_it_says_so() {
    let machine = Machine::new("put-back-again");
    let path = machine.path("other.toml");
    let id = machine.changed(&path, "a\n", "b\n");
    fs::write(&path, "c\n").unwrap();
    let (done, said) = machine.put_back(id, "ok: x\n");
    assert_eq!(done.unwrap(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), "a\n");
    assert!(
        said.contains(&format!("put back {path}; it was changed again after change {id}")),
        "{said}"
    );
    // It belongs to no service, so Spark is not asked.
    assert!(machine.asked.borrow().is_empty());
}

#[test]
fn a_file_removed_since_is_made_again_with_a_settings_file_s_mode() {
    let machine = Machine::new("put-back-removed");
    let path = machine.path("other.toml");
    let id = machine.changed(&path, "a\n", "b\n");
    fs::remove_file(&path).unwrap();
    let (done, said) = machine.put_back(id, "");
    assert_eq!(done.unwrap(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), "a\n");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
    assert!(said.contains("changed again after change"), "{said}");
}

#[test]
fn each_service_of_the_files_is_asked_once_and_its_answer_is_said() {
    let machine = Machine::new("put-back-services");
    let web = machine.path("services/web.toml");
    let over = machine.path("services/settings.d/web.toml");
    let db = machine.path("services/db.toml");
    let files: Vec<FileChange> = [&web, &over, &db]
        .iter()
        .map(|path| {
            fs::write(path, "new\n").unwrap();
            machine.store.keep_version(path, b"old\n", 1).unwrap();
            FileChange { path: (*path).clone(), before: text("old\n"), after: text("new\n") }
        })
        .collect();
    let id = machine
        .store
        .add_change(&NewChange {
            at: 1,
            who: "alice",
            how: "console",
            action: Some("x.y"),
            argv: None,
            status: Some(0),
            task: None,
            note: None,
            files: &files,
        })
        .unwrap();
    let (done, said) = machine.put_back(id, "ok: asked to reload\n");
    assert_eq!(done.unwrap(), 0);
    assert_eq!(*machine.asked.borrow(), ["reload web", "reload db"]);
    assert!(
        said.ends_with(&format!(
            "asked web to reload\nasked db to reload\nchange {id} is put back: 3 files\n"
        )),
        "{said}"
    );
}

#[test]
fn a_service_that_does_not_run_or_that_spark_does_not_know_is_no_failure() {
    let machine = Machine::new("put-back-not-running");
    let path = machine.path("services/web.toml");
    let id = machine.changed(&path, "a\n", "b\n");
    let (done, said) = machine.put_back(id, "error: web is not running\n");
    assert_eq!(done.unwrap(), 0);
    assert!(said.contains("web is not running, so it reads its file when it starts\n"), "{said}");
    let id = machine.changed(&path, "c\n", "d\n");
    let (done, said) = machine.put_back(id, "error: no service is called web\n");
    assert_eq!(done.unwrap(), 0);
    assert!(
        said.contains("Spark has no service called web, so nothing was asked to reload\n"),
        "{said}"
    );
}

#[test]
fn a_spark_that_cannot_be_asked_leaves_the_files_written_and_fails_in_words() {
    let machine = Machine::new("put-back-spark");
    let path = machine.path("services/web.toml");
    let id = machine.changed(&path, "a\n", "b\n");
    let (done, said) = machine.put_back(id, "unreachable");
    let failed = done.unwrap_err();
    assert_eq!(failed.code, 1);
    assert!(
        failed
            .message
            .starts_with(&format!("change {id} is put back in its files, but web could not")),
        "{}",
        failed.message
    );
    assert!(failed.message.contains("Connection refused"), "{}", failed.message);
    assert_eq!(fs::read_to_string(&path).unwrap(), "a\n");
    assert!(said.contains(&format!("put back {path}\n")), "{said}");
    let id = machine.changed(&path, "c\n", "d\n");
    let (done, _) = machine.put_back(id, "error: the supervisor is busy\n");
    assert!(done.unwrap_err().message.contains("the supervisor is busy"));
}

#[test]
fn a_change_that_is_not_there_or_cannot_be_put_back_writes_nothing() {
    let machine = Machine::new("put-back-refused");
    let gone = machine.put_back(77, "").0.unwrap_err();
    assert_eq!((gone.code, gone.message.as_str()), (1, "change 77 is not in the journal"));
    // A file the change made has no earlier text, and the one before it
    // in the same change is not written either.
    let path = machine.path("services/web.toml");
    let first = machine.path("first.toml");
    fs::write(&first, "new\n").unwrap();
    machine.store.keep_version(&first, b"old\n", 1).unwrap();
    let files = [
        FileChange { path: first.clone(), before: text("old\n"), after: text("new\n") },
        FileChange { path: path.clone(), before: Seen::Absent, after: text("x\n") },
    ];
    let id = machine
        .store
        .add_change(&NewChange {
            at: 1,
            who: "alice",
            how: "console",
            action: None,
            argv: None,
            status: Some(0),
            task: None,
            note: None,
            files: &files,
        })
        .unwrap();
    let (done, said) = machine.put_back(id, "");
    let failed = done.unwrap_err();
    assert!(failed.message.starts_with(&format!("change {id}: ")), "{}", failed.message);
    assert!(failed.message.contains("was not there before"), "{}", failed.message);
    assert_eq!(said, "");
    assert_eq!(fs::read_to_string(&first).unwrap(), "new\n");
}

#[test]
fn a_path_that_is_not_absolute_or_climbs_out_is_refused_before_anything_is_written() {
    let machine = Machine::new("put-back-path");
    let first = machine.path("first.toml");
    fs::write(&first, "new\n").unwrap();
    machine.store.keep_version(&first, b"old\n", 1).unwrap();
    for bad in ["relative.toml", "/system/../etc/passwd"] {
        machine.store.keep_version(bad, b"old\n", 1).unwrap();
        let files = [
            FileChange { path: first.clone(), before: text("old\n"), after: text("new\n") },
            FileChange { path: bad.into(), before: text("old\n"), after: text("new\n") },
        ];
        let id = machine
            .store
            .add_change(&NewChange {
                at: 1,
                who: "alice",
                how: "by hand",
                action: None,
                argv: None,
                status: None,
                task: None,
                note: None,
                files: &files,
            })
            .unwrap();
        let failed = machine.put_back(id, "").0.unwrap_err();
        assert!(failed.message.contains("not an absolute one"), "{}", failed.message);
        assert_eq!(fs::read_to_string(&first).unwrap(), "new\n", "{bad}");
    }
}

#[test]
fn the_services_of_a_file_are_found_by_its_place_and_by_bundled_s_records() {
    let machine = Machine::new("services-of");
    let places = machine.places();
    let services = |path: &str| services_of(path, &places);
    assert_eq!(services(&machine.path("services/web.toml")), ["web"]);
    assert_eq!(services(&machine.path("services/settings.d/web.toml")), ["web"]);
    for none in [
        machine.path("services/web.txt"),
        machine.path("services/.web.toml"),
        machine.path("services/settings.d/more/web.toml"),
        machine.path("elsewhere/web.toml"),
        "/".into(),
    ] {
        assert!(services(&none).is_empty(), "{none}");
    }
    // A bundle service's data folder is the service whose record names the
    // bundle and the account, and two services of one account are both.
    for (name, id, account) in [
        ("shop", "org.example.shop", "shop"),
        ("shop-jobs", "org.example.shop", "shop"),
        ("web", "org.example.web", "web"),
    ] {
        let record =
            services::Record { id: id.into(), account: account.into(), written: "sum".into() };
        fs::write(places.records.join(name), record.text()).unwrap();
    }
    fs::write(places.records.join("broken"), "not a record").unwrap();
    assert_eq!(
        services("/shared/services/org.example.shop/shop/settings/a.conf"),
        ["shop", "shop-jobs"]
    );
    assert_eq!(services("/shared/services/org.example.web/web/a.conf"), ["web"]);
    assert!(services("/shared/services/org.example.web/other/a.conf").is_empty());
    assert!(services("/shared/services/org.example.web/web").is_empty());
    assert!(services("/shared/services/org.example.shop").is_empty());
}

#[test]
fn the_command_line_takes_one_change_number() {
    let run = |items: &[&str]| {
        let mut args = Args::new("admin", items.iter().map(Into::into).collect());
        main(&mut args).unwrap_err()
    };
    for bad in [&[][..], &["0"], &["-1"], &["x"], &["1", "2"], &["--all"]] {
        let failed = run(bad);
        assert!(failed.usage || failed.message.contains("usage"), "{bad:?}: {}", failed.message);
    }
}
