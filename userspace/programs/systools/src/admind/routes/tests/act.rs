// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The actions' routes, with the actions of `actions::tests`: a read that
//! runs in the person's worker, and a change that runs as an
//! administrator, each asked for by a session and by an API token.

use super::*;
use crate::admind::actions::tests::LAB;

/// A world whose console offers the test actions.
fn lab_world(what: &str) -> World {
    let world = world(what);
    world.console.shared.actions.set(LAB);
    world
}

/// A request as a script makes it with `token`: no cookie, no origin.
fn with_token(req: Req, token: &str) -> Req {
    req.with("origin", None).with("authorization", Some(&format!("Bearer {token}")))
}

/// What authd was asked to run as an administrator, as its lines.
fn elevations(world: &World) -> Vec<String> {
    let seen = world.console.authd.seen.lock().unwrap();
    seen.iter().filter(|line| line.starts_with("elevate ")).cloned().collect()
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

#[test]
fn the_list_of_actions_is_for_a_session_or_a_token_and_no_one_else() {
    run(async {
        let world = lab_world("actions-list");
        let session = sign_in(&world, "dave").await;
        let listed = send(&world, get("/api/1/actions").by(&session)).await;
        assert_eq!(listed.status, 200, "{}", listed.body);
        let ids: Vec<Value> =
            listed.json().as_array().unwrap().iter().map(|a| a["id"].clone()).collect();
        assert_eq!(ids, [json!("lab.restart"), json!("lab.list")]);
        let by_token = send(&world, with_token(get("/api/1/actions"), TOKEN_READ)).await;
        assert_eq!(by_token.json(), listed.json());
        let nobody = send(&world, get("/api/1/actions")).await;
        assert_eq!((nobody.status, nobody.error()), (401, Kind::SessionEnded.id().to_string()));
        // A session limited to adding a second step has no actions yet.
        let eve = sign_in(&world, "eve").await;
        assert_eq!(send(&world, get("/api/1/actions").by(&eve)).await.error(), "ADM-118");
    });
}

#[test]
fn a_read_runs_in_the_person_s_worker_and_answers_the_json_its_command_printed() {
    run(async {
        let world = lab_world("actions-read");
        let session = sign_in(&world, "dave").await;
        let read = send(&world, get("/api/1/lab").by(&session)).await;
        assert_eq!(read.status, 200, "{}", read.body);
        assert_eq!(read.json(), json!([{ "name": "nginx" }]));
        let argv = json!({ "argv": ["service", "list", "--json"] });
        assert!(called(&world).contains(&("run".into(), argv)), "{:?}", called(&world));
        assert!(elevations(&world).is_empty());
        // A read is a GET and nothing else: not a POST, and not a HEAD,
        // which would run it for nothing.
        let posted = send(&world, post("/api/1/lab", &json!({})).by(&session)).await;
        assert_eq!((posted.status, posted.error().as_str()), (405, "ADM-117"));
        let mut head = get("/api/1/lab").by(&session);
        head.method = "HEAD";
        assert_eq!(send(&world, head).await.status, 405);
    });
}

#[test]
fn an_administrator_s_change_needs_an_administrator_whose_session_is_unlocked() {
    run(async {
        let world = lab_world("actions-change");
        let restart = |session: &Session| post("/api/1/lab/nginx/restart", &json!({})).by(session);
        let dave = sign_in(&world, "dave").await;
        let not_admin = send(&world, restart(&dave)).await;
        assert_eq!((not_admin.status, not_admin.error().as_str()), (403, "ADM-122"));
        let chase = sign_in(&world, "chase").await;
        let locked = send(&world, restart(&chase)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        // Neither was put to authd.
        assert!(elevations(&world).is_empty());
        unlock(&world, &chase).await;
        let done = send(&world, restart(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(done.json(), json!({ "done": true, "output": ["nginx restart"] }));
        let ran = elevations(&world);
        assert_eq!(ran.len(), 1);
        assert!(ran[0].starts_with("elevate session "), "{}", ran[0]);
        assert!(ran[0].ends_with(" -- service nginx restart"), "{}", ran[0]);
        assert_eq!(world.console.shared.elevating.get(), 0);
        // A change is a POST, which carries the session's token as every
        // change does.
        let forged = post("/api/1/lab/nginx/restart", &json!({}))
            .by(&chase)
            .with("x-dukovo-csrf", Some("forged"));
        assert_eq!(send(&world, forged).await.status, 403);
        let got = send(&world, get("/api/1/lab/nginx/restart").by(&chase)).await;
        assert_eq!(got.error(), "ADM-117");
    });
}

#[test]
fn a_command_that_fails_answers_with_its_own_error_id_and_words() {
    run(async {
        let world = lab_world("actions-failed");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let failed = send(&world, post("/api/1/lab/broken/restart", &json!({})).by(&chase)).await;
        assert_eq!(failed.status, 422, "{}", failed.body);
        let error = &failed.json()["error"];
        assert_eq!(error["id"], "SVC-104");
        assert_eq!(error["detail"], "broken.conf did not load (SVC-104)");
        assert_eq!(
            error["command"],
            json!({
                "argv": ["service", "broken", "restart"],
                "status": 1,
                "errors": "starting broken\nbroken.conf did not load (SVC-104)\n",
            })
        );
    });
}

#[test]
fn arguments_are_checked_before_anyone_is_asked_and_a_bad_one_is_named() {
    run(async {
        let world = lab_world("actions-arguments");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let ask = |path: &str, body: Value| post(path, &body).by(&chase);
        let bad = send(&world, ask("/api/1/lab/-x/restart", json!({}))).await;
        assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-125"));
        assert_eq!(bad.json()["error"]["field"], "name");
        let wait = send(&world, ask("/api/1/lab/nginx/restart", json!({ "wait": 61 }))).await;
        assert_eq!(wait.json()["error"]["field"], "wait");
        for body in [json!({ "name": "sshd" }), json!({ "force": true }), json!({ "wait": "soon" })]
        {
            let refused = send(&world, ask("/api/1/lab/nginx/restart", body.clone())).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-108"), "{body}");
        }
        // An escaped slash is no part of a name.
        let slash = send(&world, ask("/api/1/lab/a%2Fb/restart", json!({}))).await;
        assert_eq!(slash.status, 404);
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_token_does_what_its_scopes_name_and_authd_checks_its_area_again() {
    run(async {
        let world = lab_world("actions-token");
        let restart = |token: &str| with_token(post("/api/1/lab/nginx/restart", &json!({})), token);
        let done = send(&world, restart(TOKEN_ALL)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(done.json()["output"], json!(["nginx restart"]));
        let ran = elevations(&world);
        let hex = accounts::request::hex(TOKEN_ALL.as_bytes());
        assert_eq!(ran, [format!("elevate token {hex} services -- service nginx restart")]);
        // A token that may only read services may not restart one, and
        // authd is not asked.
        let read_only = send(&world, restart(TOKEN_READ)).await;
        assert_eq!((read_only.status, read_only.error().as_str()), (403, "ADM-124"));
        assert_eq!(elevations(&world).len(), 1);
        // What authd refuses is told as it is.
        let guarded = with_token(post("/api/1/lab/guarded/restart", &json!({})), TOKEN_ALL);
        let refused = send(&world, guarded).await;
        assert_eq!((refused.status, refused.error()), (403, Kind::Refused.id().to_string()));
        // A read runs in a worker of the token's owner, started for its
        // first read and kept for the next; a change through authd needs
        // none.
        for _ in 0..2 {
            let read = send(&world, with_token(get("/api/1/lab"), TOKEN_READ)).await;
            assert_eq!(read.json(), json!([{ "name": "nginx" }]));
        }
        let verbs = world.console.authd.verbs();
        assert_eq!(verbs.iter().filter(|verb| *verb == "token-start").count(), 1, "{verbs:?}");
        // authd is asked about the token on every request.
        assert_eq!(verbs.iter().filter(|verb| *verb == "token").count(), 5, "{verbs:?}");
    });
}

#[test]
fn a_wrong_token_is_refused_and_ten_in_a_minute_stop_the_address_before_authd_hears() {
    run(async {
        let world = lab_world("actions-wrong-token");
        let wrong = "dkv_chase_0000000000000000000000000000000000000000000000000000000000000000";
        for _ in 0..10 {
            let refused = send(&world, with_token(get("/api/1/lab"), wrong)).await;
            assert_eq!((refused.status, refused.error().as_str()), (401, "ADM-115"));
        }
        let stopped = send(&world, with_token(get("/api/1/lab"), wrong)).await;
        assert_eq!((stopped.status, stopped.error().as_str()), (429, "ADM-105"));
        assert!(stopped.headers.contains_key("retry-after"));
        // The right token from the address waits too: the address is
        // stopped, not the token.
        let right = send(&world, with_token(get("/api/1/lab"), TOKEN_READ)).await;
        assert_eq!(right.status, 429);
        let verbs = world.console.authd.verbs();
        assert_eq!(verbs.iter().filter(|verb| *verb == "token").count(), 10);
        // Another address is not stopped.
        let elsewhere =
            send_from(&world, "192.0.2.99", with_token(get("/api/1/lab"), TOKEN_READ)).await;
        assert_eq!(elsewhere.status, 200, "{}", elsewhere.body);
    });
}
