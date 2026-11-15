// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The routes of actions that take or answer a secret (ADR 0561), with the
//! actions of `actions::tests::secrets`: the secret reaches its command's
//! input, an administrator's through authd and a person's through their
//! worker, a task's as a short command's does, and nothing else holds it:
//! not the answer, authd's request, the journal, the task or the Show
//! command.

use super::*;
use crate::admind::actions::tests::secrets::SECRETS;

/// A world whose console offers the actions that take secrets.
fn secrets_world(what: &str) -> World {
    let world = world(what);
    world.console.shared.actions.set(SECRETS);
    world
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// What the test authd and worker were asked with `name`, in order.
fn asked(world: &World, name: &str) -> Vec<Value> {
    let calls = world.calls.lock().unwrap();
    calls.iter().filter(|(called, _)| called == name).map(|(_, value)| value.clone()).collect()
}

/// Everything authd was asked, every call the worker had but its input,
/// the journal's rows and the tasks, as one text to look for a secret in.
async fn everything_kept(world: &World, session: &Session) -> String {
    let seen = world.console.authd.seen.lock().unwrap().join("\n");
    let calls: Vec<String> = world
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name != "elevate-input")
        .map(|(name, value)| {
            let mut value = value.clone();
            if let Some(params) = value.as_object_mut() {
                params.remove("input");
            }
            format!("{name} {value}")
        })
        .collect();
    let changes = send(world, get("/api/1/changes").by(session)).await.body;
    let tasks = send(world, get("/api/1/tasks").by(session)).await.body;
    format!("{seen}\n{}\n{changes}\n{tasks}", calls.join("\n"))
}

const MARKER: &str = "mk-route-5f1e a secret";

#[test]
fn an_administrator_s_secret_is_the_elevated_command_s_input_and_kept_nowhere() {
    run(async {
        let world = secrets_world("secret-elevated");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let body = json!({ "password": MARKER });
        let done = send(&world, post("/api/1/lab/bob/password", &body).by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(done.json()["output"], json!(["bob"]));
        assert_eq!(asked(&world, "elevate"), [json!(["passwd", "bob"])]);
        assert_eq!(asked(&world, "elevate-input"), [json!(format!("{MARKER}\n"))]);
        let kept = everything_kept(&world, &chase).await;
        assert!(kept.contains("passwd") && !kept.contains("mk-route"), "{kept}");
        // A secret that is not one line is refused before anything runs, and
        // its refusal does not quote it.
        let bad = json!({ "password": "mk-route\nsecond" });
        let refused = send(&world, post("/api/1/lab/bob/password", &bad).by(&chase)).await;
        assert_eq!(refused.status, 400, "{}", refused.body);
        assert!(!refused.body.contains("mk-route"), "{}", refused.body);
        assert_eq!(asked(&world, "elevate").len(), 1);
    });
}

#[test]
fn a_person_s_secrets_are_their_command_s_input_as_lines_in_the_worker() {
    run(async {
        let world = secrets_world("secret-worker");
        let dave = sign_in(&world, "dave").await;
        let body = json!({ "password": "mk-route-now", "new_password": "mk-route-new" });
        let done = send(&world, post("/api/1/lab/password", &body).by(&dave)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        let runs = asked(&world, "run");
        let [run] = runs.as_slice() else { panic!("{runs:?}") };
        assert_eq!(run["argv"], json!(["passwd"]));
        assert_eq!(run["input"], "mk-route-now\nmk-route-new\n");
        let kept = everything_kept(&world, &dave).await;
        assert!(!kept.contains("mk-route"), "{kept}");
    });
}

#[test]
fn a_read_that_takes_a_secret_is_a_post_and_its_address_with_get_is_refused() {
    run(async {
        let world = secrets_world("secret-read");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let read =
            send(&world, post("/api/1/lab/unlock", &json!({ "key": MARKER })).by(&chase)).await;
        assert_eq!(read.status, 200, "{}", read.body);
        assert_eq!(read.json(), json!({ "read": true }));
        assert_eq!(asked(&world, "elevate"), [json!(["lab-unlock", "--json", "--key", "-"])]);
        assert_eq!(asked(&world, "elevate-input"), [json!(format!("{MARKER}\n"))]);
        // The file instead of the key: nothing on the input.
        let file = json!({ "file": "/users/chase/key.txt" });
        let read = send(&world, post("/api/1/lab/unlock", &file).by(&chase)).await;
        assert_eq!(read.status, 200, "{}", read.body);
        assert_eq!(asked(&world, "elevate-input").len(), 1);
        // With GET, the address is the other method's, and nothing runs.
        for path in ["/api/1/lab/unlock", "/api/1/lab/unlock?key=mk-route", "/api/1/lab/web/key"] {
            let refused = send(&world, get(path).by(&chase)).await;
            assert_eq!(refused.status, 405, "{path}: {}", refused.body);
            assert!(!refused.body.contains("mk-route"), "{}", refused.body);
        }
        assert_eq!(asked(&world, "elevate").len(), 2);
        // A secret answer is a POST's, and is not kept: no-store, as all of
        // the API's answers are.
        let key = send(&world, post("/api/1/lab/web/key", &json!({})).by(&chase)).await;
        assert_eq!(key.status, 200, "{}", key.body);
        let store = key.headers.get("cache-control").and_then(|value| value.to_str().ok());
        assert_eq!(store, Some("no-store"));
    });
}

#[test]
fn a_task_s_command_reads_its_secret_and_the_task_keeps_it_as_secret() {
    run(async {
        let world = secrets_world("secret-task");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let asked_for = post("/api/1/lab/restore", &json!({ "key": MARKER })).by(&chase);
        let started = send(&world, asked_for).await;
        assert_eq!(started.status, 202, "{}", started.body);
        let id = started.json()["task"].as_i64().unwrap();
        let mut task = Value::Null;
        for _ in 0..500 {
            task = send(&world, get(&format!("/api/1/tasks/{id}")).by(&chase)).await.json();
            if task["state"] == "done" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(task["state"], "done", "{task}");
        assert_eq!(task["args"], json!({ "key": "(secret)" }));
        assert_eq!(task["argv"], json!(["admin", "lab-steps", "1", "--key", "-"]));
        assert_eq!(asked(&world, "elevate-input"), [json!(format!("{MARKER}\n"))]);
        let kept = everything_kept(&world, &chase).await;
        assert!(!kept.contains("mk-route"), "{kept}");
    });
}

#[test]
fn the_show_command_needs_no_secret_and_keeps_none_it_is_sent() {
    run(async {
        let world = secrets_world("secret-show");
        let dave = sign_in(&world, "dave").await;
        for args in [json!({ "name": "bob" }), json!({ "name": "bob", "password": MARKER })] {
            let body = json!({ "action": "lab.password", "args": args });
            let shown = send(&world, post("/api/1/show", &body).by(&dave)).await;
            assert_eq!(shown.status, 200, "{}", shown.body);
            let shown = shown.json();
            assert_eq!(shown["command"], "elevate passwd bob");
            assert_eq!(shown["typed"], "then the password, typed when asked");
            assert!(shown["api"].as_str().unwrap().contains("$PASSWORD"));
            assert!(!shown.to_string().contains("mk-route"), "{shown}");
        }
        assert!(asked(&world, "elevate").is_empty(), "showing runs nothing");
    });
}
