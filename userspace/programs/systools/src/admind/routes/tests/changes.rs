// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The journal's routes: the list, one change with its diff, Keep it and
//! Put back, and the rows that the actions' routes make.

use super::*;
use crate::admind::actions::tests::LAB;
use crate::admind::store::changes::{FileChange, NewChange, Seen};
use crate::admind::store::versions::sha256;

/// A world whose console offers the test actions.
fn lab_world(what: &str) -> World {
    let world = world(what);
    world.console.shared.actions.set(LAB);
    world
}

fn store(world: &World) -> &crate::admind::store::Store {
    world.console.shared.store.as_ref().unwrap()
}

/// What a file's side is for `text`, whose version the store keeps under
/// `path`.
fn kept(world: &World, path: &str, text: &str) -> Seen {
    store(world).keep_version(path, text.as_bytes(), 1).unwrap();
    Seen::Text { sha256: sha256(text.as_bytes()), size: text.len() as u64 }
}

/// A row for `how` at Unix time `at`, naming `files`.
fn add(world: &World, how: &str, at: i64, files: &[FileChange]) -> i64 {
    store(world)
        .add_change(&NewChange {
            at,
            who: "alice",
            how,
            action: (how != "by hand").then_some("lab.restart"),
            argv: None,
            status: (how != "by hand").then_some(0),
            task: None,
            note: None,
            files,
        })
        .unwrap()
}

/// A change of `path` from `before` to `after` by hand, which the store
/// holds the versions of.
fn edit(world: &World, path: &str, before: &str, after: &str) -> i64 {
    let files = [FileChange {
        path: path.into(),
        before: kept(world, path, before),
        after: kept(world, path, after),
    }];
    add(world, "by hand", 1_000, &files)
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// What authd was asked to run as an administrator, as its lines.
fn elevations(world: &World) -> Vec<String> {
    let seen = world.console.authd.seen.lock().unwrap();
    seen.iter().filter(|line| line.starts_with("elevate ")).cloned().collect()
}

#[test]
fn every_signed_in_session_reads_the_list_newest_first_and_a_token_and_no_one_do_not() {
    run(async {
        let world = world("changes-list");
        for at in [100, 200, 300] {
            add(&world, "console", at, &[]);
        }
        // A locked session of a person who is no administrator reads.
        let dave = sign_in(&world, "dave").await;
        let listed = send(&world, get("/api/1/changes").by(&dave)).await;
        assert_eq!(listed.status, 200, "{}", listed.body);
        let rows = listed.json()["changes"].as_array().unwrap().clone();
        let ats: Vec<i64> = rows.iter().map(|row| row["at"].as_i64().unwrap()).collect();
        assert_eq!(ats, [300, 200, 100]);
        assert_eq!(rows[0]["who"], "alice");
        assert_eq!(rows[0]["how"], "console");
        assert_eq!(rows[0]["action"], "lab.restart");
        assert_eq!(rows[0]["title"], Value::Null);
        assert_eq!(rows[0]["files"], json!([]));
        assert_eq!(rows[0]["seen"], Value::Null);
        // Only a token does not.
        let by_token = send(&world, with_token(get("/api/1/changes"), TOKEN_ALL)).await;
        assert_eq!((by_token.status, by_token.error().as_str()), (403, "ADM-124"));
        let nobody = send(&world, get("/api/1/changes")).await;
        assert_eq!((nobody.status, nobody.error().as_str()), (401, "ADM-107"));
        let eve = sign_in(&world, "eve").await;
        assert_eq!(send(&world, get("/api/1/changes").by(&eve)).await.error(), "ADM-118");
    });
}

/// A request as a script makes it with `token`: no cookie, no origin.
fn with_token(req: Req, token: &str) -> Req {
    req.with("origin", None).with("authorization", Some(&format!("Bearer {token}")))
}

#[test]
fn the_list_takes_a_time_and_a_limit_and_names_a_bad_one() {
    run(async {
        let world = world("changes-query");
        for at in 1..=250 {
            add(&world, "console", at, &[]);
        }
        let dave = sign_in(&world, "dave").await;
        let count = |answered: &Answered| answered.json()["changes"].as_array().unwrap().len();
        // 50 when it is not said, and what is said up to 200.
        assert_eq!(count(&send(&world, get("/api/1/changes").by(&dave)).await), 50);
        assert_eq!(count(&send(&world, get("/api/1/changes?limit=").by(&dave)).await), 50);
        assert_eq!(count(&send(&world, get("/api/1/changes?limit=7").by(&dave)).await), 7);
        assert_eq!(count(&send(&world, get("/api/1/changes?limit=200").by(&dave)).await), 200);
        assert_eq!(
            count(&send(&world, get("/api/1/changes?since=248&limit=10").by(&dave)).await),
            3
        );
        assert_eq!(count(&send(&world, get("/api/1/changes?since=&limit=2").by(&dave)).await), 2);
        for (query, field) in [
            ("limit=201", "limit"),
            ("limit=0", "limit"),
            ("limit=-1", "limit"),
            ("limit=many", "limit"),
            ("since=-5", "since"),
            ("since=yesterday", "since"),
        ] {
            let bad = send(&world, get(&format!("/api/1/changes?{query}")).by(&dave)).await;
            assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-125"), "{query}");
            assert_eq!(bad.json()["error"]["field"], field, "{query}");
        }
    });
}

#[test]
fn one_change_has_each_file_with_its_diff_and_says_how_to_put_it_back() {
    run(async {
        let world = world("changes-one");
        let hand = edit(
            &world,
            "/system/settings/admin.toml",
            "reach = \"private\"\n",
            "reach = \"any\"\n",
        );
        let dave = sign_in(&world, "dave").await;
        let shown = send(&world, get(&format!("/api/1/changes/{hand}")).by(&dave)).await;
        assert_eq!(shown.status, 200, "{}", shown.body);
        let row = shown.json();
        assert_eq!((row["id"].as_i64(), row["how"].as_str()), (Some(hand), Some("by hand")));
        let file = &row["files"][0];
        assert_eq!(file["path"], "/system/settings/admin.toml");
        assert_eq!(file["before"]["state"], "text");
        assert_eq!(file["diff_truncated"], false);
        assert_eq!(
            file["diff"],
            "--- /system/settings/admin.toml (before)\n+++ /system/settings/admin.toml (after)\n\
             @@ -1,1 +1,1 @@\n-reach = \"private\"\n+reach = \"any\"\n"
        );
        assert_eq!(
            row["put_back"],
            json!({ "possible": true, "command": format!("elevate admin put-back {hand}") })
        );
    });
}

#[test]
fn a_secret_an_unread_file_and_a_text_no_longer_kept_have_no_diff_and_say_why() {
    run(async {
        let world = world("changes-no-diff");
        let gone = Seen::Text { sha256: sha256(b"gone"), size: 4 };
        let files = [
            FileChange { path: "/s".into(), before: Seen::Secret, after: Seen::Secret },
            FileChange { path: "/u".into(), before: Seen::Unreadable, after: Seen::Absent },
            FileChange { path: "/g".into(), before: gone, after: kept(&world, "/g", "new\n") },
            FileChange {
                path: "/l".into(),
                before: Seen::Large { sha256: sha256(b"l"), size: 2 << 20 },
                after: Seen::Absent,
            },
            FileChange {
                path: "/same".into(),
                before: kept(&world, "/same", "x\n"),
                after: kept(&world, "/same", "x\n"),
            },
            FileChange {
                path: "/bin".into(),
                before: kept(&world, "/bin", "a\0b"),
                after: kept(&world, "/bin", "c"),
            },
        ];
        let id = add(&world, "console", 5, &files);
        let dave = sign_in(&world, "dave").await;
        let row = send(&world, get(&format!("/api/1/changes/{id}")).by(&dave)).await.json();
        let why = |at: usize| row["files"][at]["no_diff"].as_str().unwrap().to_string();
        assert!(why(0).contains("only its name is kept"), "{}", why(0));
        assert!(why(1).contains("could not read"), "{}", why(1));
        assert!(why(2).contains("not kept any more"), "{}", why(2));
        assert!(why(3).contains("longer than 1 MiB"), "{}", why(3));
        assert_eq!(why(4), "the file is as it was");
        assert_eq!(why(5), "it is not text");
        for at in 0..6 {
            assert!(row["files"][at].get("diff").is_none(), "{at}");
        }
        // Not everything can be put back, and it says which is why.
        assert_eq!(row["put_back"]["possible"], false);
        assert!(row["put_back"]["why"].as_str().unwrap().contains("/s"), "{}", row["put_back"]);
    });
}

#[test]
fn the_diffs_of_a_change_are_at_most_2000_lines_in_all() {
    run(async {
        let world = world("changes-diff-limit");
        let lines = |tag: &str| -> String {
            use std::fmt::Write as _;
            let mut text = String::new();
            for n in 0..1500 {
                let _ = writeln!(text, "{tag} {n}");
            }
            text
        };
        let files: Vec<FileChange> = ["/a", "/b"]
            .iter()
            .map(|path| FileChange {
                path: (*path).into(),
                before: kept(&world, path, &lines("old")),
                after: kept(&world, path, &lines("new")),
            })
            .collect();
        let id = add(&world, "console", 5, &files);
        let dave = sign_in(&world, "dave").await;
        let row = send(&world, get(&format!("/api/1/changes/{id}")).by(&dave)).await.json();
        let first = row["files"][0]["diff"].as_str().unwrap();
        // 1,500 deleted, 1,500 inserted, a hunk header and two file names:
        // more than the whole answer may hold.
        assert_eq!(first.lines().count(), 2000);
        assert_eq!(row["files"][0]["diff_truncated"], true);
        assert!(row["files"][1].get("diff").is_none());
        assert!(row["files"][1]["no_diff"].as_str().unwrap().contains("2000 lines"));
    });
}

#[test]
fn a_change_that_is_not_there_or_not_a_number_is_adm_127() {
    run(async {
        let world = world("changes-missing");
        let dave = sign_in(&world, "dave").await;
        for path in
            ["/api/1/changes/99", "/api/1/changes/0", "/api/1/changes/abc", "/api/1/changes/-3"]
        {
            let missing = send(&world, get(path).by(&dave)).await;
            assert_eq!((missing.status, missing.error().as_str()), (404, "ADM-127"), "{path}");
        }
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        for path in ["/api/1/changes/99/keep", "/api/1/changes/99/put-back"] {
            let missing = send(&world, post(path, &json!({})).by(&chase)).await;
            assert_eq!((missing.status, missing.error().as_str()), (404, "ADM-127"), "{path}");
        }
    });
}

#[test]
fn keep_it_marks_a_hand_edit_seen_by_a_session_that_is_unlocked() {
    run(async {
        let world = world("changes-keep");
        let hand = add(&world, "by hand", 100, &[]);
        let console = add(&world, "console", 200, &[]);
        // Any person's, not only an administrator's: dave is not one.
        let dave = sign_in(&world, "dave").await;
        let keep = |id: i64| post(&format!("/api/1/changes/{id}/keep"), &json!({}));
        let locked = send(&world, keep(hand).by(&dave)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        assert_eq!(store(&world).change(hand).unwrap().unwrap().seen, None);
        unlock(&world, &dave).await;
        let kept = send(&world, keep(hand).by(&dave)).await;
        assert_eq!(kept.status, 200, "{}", kept.body);
        assert_eq!(kept.json()["seen"]["by"], "dave");
        assert!(kept.json()["seen"]["at"].as_i64().unwrap() > 0);
        // Again, by another person: it stays as it was.
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let again = send(&world, keep(hand).by(&chase)).await;
        assert_eq!(again.status, 200);
        assert_eq!(again.json()["seen"]["by"], "dave");
        // A change made through the console is kept already.
        let refused = send(&world, keep(console).by(&dave)).await;
        assert_eq!((refused.status, refused.error().as_str()), (409, "ADM-128"));
        // A token does not, and a plain GET is the wrong method.
        let token = send(&world, with_token(keep(hand), TOKEN_ALL)).await;
        assert_eq!(token.error(), "ADM-124");
        let got = send(&world, get(&format!("/api/1/changes/{hand}/keep")).by(&dave)).await;
        assert_eq!(got.error(), "ADM-117");
    });
}

#[test]
fn put_back_runs_admin_put_back_as_an_administrator_and_has_a_row_of_its_own() {
    run(async {
        let world = world("changes-put-back");
        let path = "/system/settings/admin.toml";
        let hand = edit(&world, path, "reach = \"private\"\n", "reach = \"any\"\n");
        let put = |id: i64| post(&format!("/api/1/changes/{id}/put-back"), &json!({}));
        let dave = sign_in(&world, "dave").await;
        let not_admin = send(&world, put(hand).by(&dave)).await;
        assert_eq!((not_admin.status, not_admin.error().as_str()), (403, "ADM-122"));
        let chase = sign_in(&world, "chase").await;
        let locked = send(&world, put(hand).by(&chase)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        // A token does not, whatever its scopes.
        let by_token = send(&world, with_token(put(hand), TOKEN_ALL)).await;
        assert_eq!(by_token.error(), "ADM-124");
        assert!(elevations(&world).is_empty());
        unlock(&world, &chase).await;
        let done = send(&world, put(hand).by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        let row = done.json()["change"].as_i64().unwrap();
        assert_eq!(
            done.json(),
            json!({
                "done": true,
                "output": [format!("put-back {hand}")],
                "command": format!("elevate admin put-back {hand}"),
                "change": row,
            })
        );
        let ran = elevations(&world);
        assert_eq!(ran.len(), 1);
        assert!(ran[0].starts_with("elevate session "), "{}", ran[0]);
        assert!(ran[0].ends_with(&format!(" -- admin put-back {hand}")), "{}", ran[0]);
        // The put back is a change of the console's, which names the file
        // and the change it puts back.
        let made = store(&world).change(row).unwrap().unwrap();
        assert_eq!((made.who.as_str(), made.how.as_str()), ("chase", "console"));
        assert_eq!(made.action.as_deref(), Some("changes.put-back"));
        assert_eq!(made.argv.unwrap(), ["admin", "put-back", &hand.to_string()]);
        assert_eq!((made.status, made.note), (Some(0), Some(format!("puts back change {hand}"))));
        assert_eq!(made.files.len(), 1);
        assert_eq!(made.files[0].path, path);
        // It is on the list with its title.
        let listed = send(&world, get("/api/1/changes?limit=1").by(&chase)).await.json();
        assert_eq!(listed["changes"][0]["title"], "Put a change back");
        assert_eq!(world.console.shared.elevating.get(), 0);
    });
}

#[test]
fn a_change_that_cannot_be_put_back_is_refused_before_anyone_is_asked() {
    run(async {
        let world = world("changes-cannot");
        let made = [FileChange {
            path: "/system/settings/new.toml".into(),
            before: Seen::Absent,
            after: kept(&world, "/system/settings/new.toml", "a\n"),
        }];
        let id = add(&world, "console", 5, &made);
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let refused =
            send(&world, post(&format!("/api/1/changes/{id}/put-back"), &json!({})).by(&chase))
                .await;
        assert_eq!((refused.status, refused.error().as_str()), (409, "ADM-128"));
        assert!(refused.json()["error"]["detail"].as_str().unwrap().contains("no earlier text"));
        let nothing = add(&world, "console", 6, &[]);
        let none = send(
            &world,
            post(&format!("/api/1/changes/{nothing}/put-back"), &json!({})).by(&chase),
        )
        .await;
        assert_eq!(none.error(), "ADM-128");
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_change_that_runs_has_a_row_with_who_how_and_the_command_and_one_not_run_has_none() {
    run(async {
        let world = lab_world("changes-rows");
        let restart = |name: &str| post(&format!("/api/1/lab/{name}/restart"), &json!({}));
        let dave = sign_in(&world, "dave").await;
        let chase = sign_in(&world, "chase").await;
        // Refused, locked: nothing ran, and nothing is kept.
        send(&world, restart("nginx").by(&dave)).await;
        send(&world, restart("nginx").by(&chase)).await;
        assert!(store(&world).changes(0, 10).unwrap().is_empty());
        unlock(&world, &chase).await;
        assert_eq!(send(&world, restart("nginx").by(&chase)).await.status, 200);
        assert_eq!(send(&world, restart("broken").by(&chase)).await.status, 422);
        let by_token = send(&world, with_token(restart("nginx"), TOKEN_ALL)).await;
        assert_eq!(by_token.status, 200, "{}", by_token.body);
        // A read is not a change.
        send(&world, get("/api/1/lab").by(&dave)).await;
        let rows = store(&world).changes(0, 10).unwrap();
        assert_eq!(rows.len(), 3);
        let (token, broken, nginx) = (&rows[0], &rows[1], &rows[2]);
        assert_eq!((nginx.who.as_str(), nginx.how.as_str()), ("chase", "console"));
        assert_eq!(nginx.action.as_deref(), Some("lab.restart"));
        assert_eq!(nginx.argv.as_deref().unwrap(), ["service", "nginx", "restart"]);
        assert_eq!((nginx.status, nginx.task), (Some(0), None));
        assert!(nginx.files.is_empty());
        assert!(nginx.at > 1_790_000_000);
        // A command that did not work is in the journal as well.
        assert_eq!(broken.status, Some(1));
        assert_eq!(broken.argv.as_deref().unwrap(), ["service", "broken", "restart"]);
        assert_eq!((token.who.as_str(), token.how.as_str()), ("chase", "token ci"));
        assert_eq!(world.console.shared.journal.pending_files(), 0);
    });
}

#[test]
fn a_console_without_a_store_still_changes_things_and_answers_adm_126_for_the_journal() {
    run(async {
        let mut world = lab_world("changes-no-store");
        std::rc::Rc::get_mut(&mut world.console.shared).unwrap().store = None;
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let done = send(&world, post("/api/1/lab/nginx/restart", &json!({})).by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        for (path, post_it) in [
            ("/api/1/changes", false),
            ("/api/1/changes/1", false),
            ("/api/1/changes/1/keep", true),
            ("/api/1/changes/1/put-back", true),
        ] {
            let req = if post_it { post(path, &json!({})) } else { get(path) };
            let answered = send(&world, req.by(&chase)).await;
            assert_eq!((answered.status, answered.error().as_str()), (503, "ADM-126"), "{path}");
        }
    });
}
