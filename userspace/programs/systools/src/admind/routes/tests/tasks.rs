// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The tasks' routes and the tasks themselves, through the console with
//! the test authd, whose `admin lab-steps N` is a command of its own kind
//! for each N (see `lab_steps` in the parent): 1 runs well and ends, 2
//! fails, 3 runs until it is sent SIGTERM, 4 ignores SIGTERM until it is
//! hung up, and 5 writes 2,500 lines.

use super::*;
use crate::admind::actions::tests::LAB;
use crate::admind::run::Place;
use crate::admind::store::tasks::State;
use crate::admind::tasks::{Limits, STOP_GRACE};

fn lab_world(what: &str) -> World {
    let world = world(what);
    world.console.shared.actions.set(LAB);
    world
}

fn store(world: &World) -> &crate::admind::store::Store {
    world.console.shared.store.as_ref().unwrap()
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// Signs alice in, who has a second step, with the code the test authd
/// takes.
async fn sign_in_alice(world: &World) -> Session {
    let body = json!({ "name": "alice", "password": "right" });
    let answered = send(world, post("/api/1/sign-in", &body)).await;
    let ticket = answered.json()["step"]["ticket"].as_str().unwrap().to_string();
    let code = json!({ "ticket": ticket, "code": "123456" });
    let right = send(world, post("/api/1/sign-in/code", &code)).await;
    let cookie = right.cookie(SESSION_COOKIE).expect("a session cookie");
    let header = format!("{SESSION_COOKIE}={cookie}");
    let shown = send(world, get("/api/1/session").with("cookie", Some(&header))).await;
    Session { csrf: shown.json()["csrf"].as_str().unwrap().into(), cookie }
}

/// A request as a script makes it with `token`: no cookie, no origin.
fn with_token(req: Req, token: &str) -> Req {
    req.with("origin", None).with("authorization", Some(&format!("Bearer {token}")))
}

/// Asks for the long action in mode `mode` as `session`, and gives the
/// task's number.
async fn start(world: &World, session: &Session, mode: u32) -> i64 {
    let asked = post("/api/1/lab/long", &json!({ "seconds": mode })).by(session);
    let answered = send(world, asked).await;
    assert_eq!(answered.status, 202, "{}", answered.body);
    assert_eq!(answered.json().as_object().unwrap().len(), 1);
    answered.json()["task"].as_i64().unwrap()
}

/// The task `id`, as the API gives it to `session`.
async fn task(world: &World, session: &Session, id: i64) -> Value {
    let answered = send(world, get(&format!("/api/1/tasks/{id}")).by(session)).await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    answered.json()
}

/// Waits until the task `id` is in `state`, and gives it.
async fn until(world: &World, session: &Session, id: i64, state: &str) -> Value {
    for _ in 0..500 {
        let now = task(world, session, id).await;
        if now["state"] == state {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task {id} did not get to {state}: {}", task(world, session, id).await);
}

/// Waits until the task `id` has said its first step, and gives it.
async fn until_stepping(world: &World, session: &Session, id: i64) -> Value {
    for _ in 0..500 {
        let now = task(world, session, id).await;
        if now["state"] == "running"
            && now["step"] == 1
            && !now["log"].as_array().unwrap().is_empty()
        {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task {id} did not run: {}", task(world, session, id).await);
}

#[test]
fn a_long_action_answers_202_and_its_task_runs_with_steps_progress_and_a_log_to_its_end() {
    run(async {
        let world = lab_world("tasks-run");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 1).await;
        let done = until(&world, &chase, id, "done").await;
        assert_eq!(done["id"], id);
        assert_eq!(done["action"], "lab.long");
        assert_eq!(done["title"], "Run the lab's long test");
        assert_eq!((done["who"].as_str(), done["how"].as_str()), (Some("chase"), Some("console")));
        assert_eq!(done["argv"], json!(["admin", "lab-steps", "1"]));
        assert_eq!(done["args"], json!({ "seconds": 1 }));
        assert_eq!((done["step"].as_i64(), done["steps"].as_i64()), (Some(2), Some(2)));
        assert_eq!((done["words"].as_str(), done["percent"].as_i64()), (Some("Two"), Some(100)));
        assert_eq!(done["status"], 0);
        assert_eq!(done["note"], Value::Null);
        assert_eq!(done["log"], json!(["a log line"]));
        assert!(done["started"].as_i64().unwrap() > 1_700_000_000);
        assert!(done["ended"].as_i64().unwrap() >= done["started"].as_i64().unwrap());
        // It is in the store, whole.
        let kept = store(&world).task(id).unwrap().unwrap();
        assert_eq!((kept.state, kept.status, kept.log.len()), (State::Done, Some(0), 1));
        // It is on the list, without its log.
        let listed = send(&world, get("/api/1/tasks").by(&chase)).await.json();
        assert_eq!(listed["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(listed["tasks"][0]["id"], id);
        assert!(listed["tasks"][0].get("log").is_none());
        // The change is in the journal, with its task, and the place is back.
        let rows = store(&world).changes(0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].action.as_deref(), rows[0].task, rows[0].status),
            (Some("lab.long"), Some(id), Some(0))
        );
        assert_eq!(rows[0].argv.as_deref().unwrap(), ["admin", "lab-steps", "1"]);
        assert_eq!(world.console.shared.elevating.get(), 0);
        assert_eq!(world.console.shared.tasks.waiting(), 0);
    });
}

#[test]
fn a_task_whose_command_fails_is_failed_with_its_last_error_line_and_a_row_of_its_own() {
    run(async {
        let world = lab_world("tasks-fail");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 2).await;
        let failed = until(&world, &chase, id, "failed").await;
        assert_eq!(failed["status"], 1);
        assert_eq!(failed["note"], "lab-steps: it broke");
        assert_eq!(failed["log"], json!(["lab-steps: it broke"]));
        assert_eq!((failed["step"].as_i64(), failed["steps"].as_i64()), (Some(1), Some(3)));
        let rows = store(&world).changes(0, 10).unwrap();
        assert_eq!((rows[0].task, rows[0].status), (Some(id), Some(1)));
        assert_eq!(rows[0].note.as_deref(), Some("lab-steps: it broke"));
    });
}

#[test]
fn the_log_of_a_task_that_writes_a_lot_is_its_last_2000_lines() {
    run(async {
        let world = lab_world("tasks-log");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 5).await;
        let done = until(&world, &chase, id, "done").await;
        let log = done["log"].as_array().unwrap();
        assert_eq!(log.len(), 2000);
        assert_eq!((log[0].as_str(), log[1999].as_str()), (Some("line 500"), Some("line 2499")));
    });
}

#[test]
fn a_task_is_stopped_by_sigterm_through_the_relay_by_its_person_or_an_administrator() {
    run(async {
        let world = lab_world("tasks-stop");
        let chase = sign_in(&world, "chase").await;
        let alice = sign_in_alice(&world).await;
        let dave = sign_in(&world, "dave").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 3).await;
        let running = until_stepping(&world, &chase, id).await;
        assert_eq!(running["percent"], 10);
        let stop = |id: i64| post(&format!("/api/1/tasks/{id}/stop"), &json!({}));
        // A person who is neither its own nor an administrator is refused.
        let refused = send(&world, stop(id).by(&dave)).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-122"));
        assert_eq!(task(&world, &chase, id).await["state"], "running");
        // Another administrator stops it, with a locked session: stopping only has the
        // command put back what it did.
        let stopping = send(&world, stop(id).by(&alice)).await;
        assert_eq!(stopping.status, 200, "{}", stopping.body);
        assert_eq!(stopping.json()["stopping"], true);
        assert_eq!(stopping.json()["task"]["id"], id);
        let stopped = until(&world, &chase, id, "stopped").await;
        assert_eq!(stopped["note"], "stopped by alice");
        assert_eq!(stopped["status"], 0);
        assert!(stopped["log"].as_array().unwrap().iter().any(|line| line == "stopping"));
        let rows = store(&world).changes(0, 10).unwrap();
        assert_eq!((rows[0].task, rows[0].note.as_deref()), (Some(id), Some("stopped by alice")));
        // An ended task cannot be stopped, and one there is not is not there.
        let again = send(&world, stop(id).by(&chase)).await;
        assert_eq!((again.status, again.error().as_str()), (409, "ADM-128"));
        for path in ["/api/1/tasks/99/stop", "/api/1/tasks/0/stop", "/api/1/tasks/x/stop"] {
            let missing = send(&world, post(path, &json!({})).by(&chase)).await;
            assert_eq!((missing.status, missing.error().as_str()), (404, "ADM-127"), "{path}");
        }
        assert_eq!(world.console.shared.elevating.get(), 0);
    });
}

#[test]
fn a_command_that_ignores_sigterm_is_hung_up_after_the_grace_and_the_task_is_stopped() {
    run(async {
        let world = lab_world("tasks-hang-up");
        world.console.shared.tasks.set_limits(Limits {
            time: Duration::from_secs(60),
            grace: Duration::from_millis(200),
        });
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 4).await;
        until_stepping(&world, &chase, id).await;
        let before = std::time::Instant::now();
        send(&world, post(&format!("/api/1/tasks/{id}/stop"), &json!({})).by(&chase)).await;
        let stopped = until(&world, &chase, id, "stopped").await;
        assert!(before.elapsed() >= Duration::from_millis(200));
        // Hung up, it ended as a signal ended it: 128 and SIGHUP.
        assert_eq!(
            (stopped["status"].as_i64(), stopped["note"].as_str()),
            (Some(129), Some("stopped by chase"))
        );
    });
}

#[test]
fn a_task_that_runs_out_of_time_is_sent_sigterm_and_has_failed() {
    run(async {
        let world = lab_world("tasks-time");
        world
            .console
            .shared
            .tasks
            .set_limits(Limits { time: Duration::from_secs(1), grace: STOP_GRACE });
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 3).await;
        let failed = until(&world, &chase, id, "failed").await;
        assert_eq!(failed["note"], "it did not end in 1 second, and was stopped");
        // The command put back what it did and ended well, which is not done.
        assert_eq!(failed["status"], 0);
        assert!(failed["log"].as_array().unwrap().iter().any(|line| line == "stopping"));
    });
}

#[test]
fn tasks_wait_for_a_place_in_the_order_they_were_asked_and_a_waiting_one_may_be_stopped() {
    run(async {
        let world = lab_world("tasks-queue");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let shared = &*world.console.shared;
        // Every place is taken by commands that are not tasks.
        let mut held: Vec<Place<'_>> = (0..32).map(|_| Place::take(shared).unwrap()).collect();
        let first = start(&world, &chase, 1).await;
        let second = start(&world, &chase, 1).await;
        let third = start(&world, &chase, 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        for id in [first, second, third] {
            let waiting = task(&world, &chase, id).await;
            assert_eq!(waiting["state"], "waiting", "{waiting}");
        }
        assert_eq!(shared.tasks.waiting(), 3);
        // A short elevated command finds no place.
        let short = send(&world, post("/api/1/lab/nginx/restart", &json!({})).by(&chase)).await;
        assert_eq!((short.status, short.error().as_str()), (429, "ADM-105"));
        // The second leaves the queue, by a stop.
        let stopped =
            send(&world, post(&format!("/api/1/tasks/{second}/stop"), &json!({})).by(&chase)).await;
        assert_eq!(stopped.status, 200);
        let left = until(&world, &chase, second, "stopped").await;
        assert_eq!(left["note"], "stopped by chase before it began");
        assert_eq!((left["status"].clone(), left["step"].as_i64()), (Value::Null, Some(0)));
        assert_eq!(shared.tasks.waiting(), 2);
        // A place is given back: the first runs, the third waits its turn.
        drop(held.pop());
        until(&world, &chase, first, "done").await;
        until(&world, &chase, third, "done").await;
        // Only the two that ran made rows.
        let rows = store(&world).changes(0, 10).unwrap();
        let tasks: Vec<Option<i64>> = rows.iter().map(|row| row.task).collect();
        assert_eq!(tasks, [Some(third), Some(first)]);
        drop(held);
        assert_eq!(shared.elevating.get(), 0);
    });
}

#[test]
fn at_most_64_tasks_wait() {
    run(async {
        let world = lab_world("tasks-waiting-max");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let shared = &*world.console.shared;
        let held: Vec<Place<'_>> = (0..32).map(|_| Place::take(shared).unwrap()).collect();
        for _ in 0..64 {
            start(&world, &chase, 1).await;
        }
        let asked = post("/api/1/lab/long", &json!({ "seconds": 1 })).by(&chase);
        let refused = send(&world, asked).await;
        assert_eq!((refused.status, refused.error().as_str()), (429, "ADM-105"));
        assert!(refused.headers.contains_key("retry-after"));
        drop(held);
    });
}

#[test]
fn a_long_action_is_asked_for_as_an_action_is_and_a_token_asks_for_it_too() {
    run(async {
        let world = lab_world("tasks-who");
        let chase = sign_in(&world, "chase").await;
        let dave = sign_in(&world, "dave").await;
        let long = || post("/api/1/lab/long", &json!({ "seconds": 1 }));
        let not_admin = send(&world, long().by(&dave)).await;
        assert_eq!(not_admin.error(), "ADM-122");
        let locked = send(&world, long().by(&chase)).await;
        assert_eq!(locked.error(), "ADM-106");
        let bad = send(&world, post("/api/1/lab/long", &json!({ "seconds": 0 })).by(&chase)).await;
        assert_eq!(bad.error(), "ADM-125");
        let read_only = send(&world, with_token(long(), TOKEN_READ)).await;
        assert_eq!(read_only.error(), "ADM-124");
        // None of them made a task.
        assert!(store(&world).tasks(0, 10).unwrap().is_empty());
        let asked = send(&world, with_token(long(), TOKEN_ALL)).await;
        assert_eq!(asked.status, 202, "{}", asked.body);
        let id = asked.json()["task"].as_i64().unwrap();
        // The token reads its task, and a token of another area does not know it.
        let read = |token: &str| with_token(get(&format!("/api/1/tasks/{id}")), token);
        let mut done = Value::Null;
        for _ in 0..500 {
            done = send(&world, read(TOKEN_ALL)).await.json();
            if done["state"] == "done" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            (done["state"].as_str(), done["how"].as_str(), done["who"].as_str()),
            (Some("done"), Some("token ci"), Some("chase"))
        );
        assert_eq!(send(&world, read(TOKEN_READ)).await.status, 200);
        let other = send(&world, read(TOKEN_OTHER)).await;
        assert_eq!((other.status, other.error().as_str()), (404, "ADM-127"));
        let list = |token: &str| with_token(get("/api/1/tasks"), token);
        assert_eq!(
            send(&world, list(TOKEN_READ)).await.json()["tasks"].as_array().unwrap().len(),
            1
        );
        assert!(
            send(&world, list(TOKEN_OTHER)).await.json()["tasks"].as_array().unwrap().is_empty()
        );
        // The journal names the token.
        let rows = store(&world).changes(0, 10).unwrap();
        assert_eq!(
            (rows[0].how.as_str(), rows[0].who.as_str(), rows[0].task),
            ("token ci", "chase", Some(id))
        );
    });
}

#[test]
fn a_token_stops_a_task_if_its_scopes_change_the_area_and_its_owner_is_the_person_or_an_administrator()
 {
    run(async {
        let world = lab_world("tasks-token-stop");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 3).await;
        until_stepping(&world, &chase, id).await;
        let stop =
            |token: &str| with_token(post(&format!("/api/1/tasks/{id}/stop"), &json!({})), token);
        let read_only = send(&world, stop(TOKEN_READ)).await;
        assert_eq!((read_only.status, read_only.error().as_str()), (403, "ADM-124"));
        let stopped = send(&world, stop(TOKEN_ALL)).await;
        assert_eq!(stopped.status, 200, "{}", stopped.body);
        let ended = until(&world, &chase, id, "stopped").await;
        assert_eq!(ended["note"], "stopped by chase");
    });
}

#[test]
fn every_signed_in_session_reads_the_tasks_and_no_one_who_is_not_does() {
    run(async {
        let world = lab_world("tasks-read");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let id = start(&world, &chase, 1).await;
        let dave = sign_in(&world, "dave").await;
        until(&world, &dave, id, "done").await;
        let listed = send(&world, get("/api/1/tasks").by(&dave)).await;
        assert_eq!(listed.json()["tasks"].as_array().unwrap().len(), 1);
        let nobody = send(&world, get("/api/1/tasks")).await;
        assert_eq!((nobody.status, nobody.error().as_str()), (401, "ADM-107"));
        // The list takes a limit and a time as the journal's does.
        let none =
            send(&world, get(&format!("/api/1/tasks?since={}", unix_now() + 100)).by(&dave)).await;
        assert!(none.json()["tasks"].as_array().unwrap().is_empty());
        let one = send(&world, get("/api/1/tasks?limit=1&since=0").by(&dave)).await;
        assert_eq!(one.json()["tasks"].as_array().unwrap().len(), 1);
        for query in ["limit=0", "limit=201", "limit=x", "since=-1", "since=x"] {
            let bad = send(&world, get(&format!("/api/1/tasks?{query}")).by(&dave)).await;
            assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-125"), "{query}");
        }
        for path in ["/api/1/tasks/99", "/api/1/tasks/0", "/api/1/tasks/x"] {
            let missing = send(&world, get(path).by(&dave)).await;
            assert_eq!((missing.status, missing.error().as_str()), (404, "ADM-127"), "{path}");
        }
        // A task that ended a day ago is on the list only when it is asked for.
        let mut old = store(&world).task(id).unwrap().unwrap();
        old.ended = Some(unix_now() - 2 * 86_400);
        store(&world).save_task(&old).unwrap();
        world.console.shared.tasks.prune(tokio::time::Instant::now() + Duration::from_secs(61));
        let recent = send(&world, get("/api/1/tasks").by(&dave)).await;
        assert!(recent.json()["tasks"].as_array().unwrap().is_empty());
        let all = send(&world, get("/api/1/tasks?since=0").by(&dave)).await;
        assert_eq!(all.json()["tasks"].as_array().unwrap().len(), 1);
    });
}

#[test]
fn without_a_store_a_long_action_is_adm_126_and_runs_nothing() {
    run(async {
        let mut world = lab_world("tasks-no-store");
        std::rc::Rc::get_mut(&mut world.console.shared).unwrap().store = None;
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let asked = post("/api/1/lab/long", &json!({ "seconds": 1 })).by(&chase);
        let refused = send(&world, asked).await;
        assert_eq!((refused.status, refused.error().as_str()), (503, "ADM-126"));
        for path in ["/api/1/tasks", "/api/1/tasks/1"] {
            assert_eq!(send(&world, get(path).by(&chase)).await.error(), "ADM-126", "{path}");
        }
        let stop = send(&world, post("/api/1/tasks/1/stop", &json!({})).by(&chase)).await;
        assert_eq!(stop.error(), "ADM-126");
        assert!(elevations(&world).is_empty());
    });
}

/// What authd was asked to run as an administrator.
fn elevations(world: &World) -> Vec<String> {
    let seen = world.console.authd.seen.lock().unwrap();
    seen.iter().filter(|line| line.starts_with("elevate ")).cloned().collect()
}

#[test]
fn a_tab_is_told_of_a_task_as_it_changes() {
    run(async {
        let world = lab_world("tasks-events");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let request = build(get("/api/1/events").by(&chase));
        let Ok(response) =
            handle(Rc::clone(&world.console.shared), PEER.parse().unwrap(), request).await;
        let mut body = response.into_body();
        let mut next = async || {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame()).await.unwrap();
            String::from_utf8(frame.unwrap().unwrap().into_data().unwrap().to_vec()).unwrap()
        };
        assert!(next().await.starts_with("event: unlock"));
        let id = start(&world, &chase, 3).await;
        // Its events say where it is, until it runs and has a step.
        let mut states = Vec::new();
        loop {
            let told = next().await;
            let data = told
                .strip_prefix("event: task\ndata: ")
                .and_then(|rest| rest.strip_suffix("\n\n"))
                .expect(&told);
            let event: Value = serde_json::from_str(data).unwrap();
            assert_eq!(event["id"], id);
            assert!(event.get("log").is_none() && event.get("argv").is_none());
            states.push(event["state"].as_str().unwrap().to_string());
            if event["state"] == "running" && event["step"] == 1 {
                assert_eq!(
                    (event["steps"].as_i64(), event["words"].as_str()),
                    (Some(4), Some("Getting ready"))
                );
                break;
            }
        }
        assert!(["waiting", "running"].contains(&states[0].as_str()), "{states:?}");
        send(&world, post(&format!("/api/1/tasks/{id}/stop"), &json!({})).by(&chase)).await;
        loop {
            let told = next().await;
            if told.contains("\"state\":\"stopped\"") {
                assert!(told.contains("\"note\":\"stopped by chase\""), "{told}");
                break;
            }
        }
    });
}
