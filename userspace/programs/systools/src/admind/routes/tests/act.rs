// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The actions' routes, with the actions of `actions::tests`: a read that
//! runs in the person's worker, and a change that runs as an
//! administrator, each asked for by a session and by an API token; the
//! showing of an action, which runs neither; and the checking of one,
//! which runs only its probe (ADR 0545).

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
        assert_eq!(
            ids,
            [
                json!("lab.restart"),
                json!("lab.list"),
                json!("lab.long"),
                json!("lab.set"),
                json!("lab.missing")
            ]
        );
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
fn a_read_whose_program_is_not_there_is_adm_129_naming_it() {
    run(async {
        let world = lab_world("actions-missing");
        let session = sign_in(&world, "dave").await;
        let read = send(&world, get("/api/1/lab/missing").by(&session)).await;
        assert_eq!(read.status, 501, "{}", read.body);
        let error = &read.json()["error"];
        assert_eq!(error["id"], "ADM-129");
        assert_eq!(error["detail"], "lab-missing: command not found");
        assert_eq!(error["fix"], "It comes with a later update of Dukovo.");
        // Nothing ran, so there is no command's output to show.
        assert!(error.get("command").is_none(), "{error}");
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn the_last_checks_are_adm_129_until_their_command_is_in_this_tree() {
    run(async {
        // The console's own list, not the tests'.
        let world = world("actions-checks");
        let dave = sign_in(&world, "dave").await;
        let read = send(&world, get("/api/1/checks/last").by(&dave)).await;
        assert_eq!(read.status, 501, "{}", read.body);
        let error = &read.json()["error"];
        assert_eq!(error["id"], "ADM-129");
        assert_eq!(error["detail"], "dukovo-check: command not found");
        assert_eq!(error["fix"], "It comes with a later update of Dukovo.");
        let argv = json!({ "argv": ["dukovo-check", "--last", "--json"] });
        assert!(called(&world).contains(&("run".into(), argv)), "{:?}", called(&world));
        // A token reads it with the security scope alone, and no other.
        let refused = send(&world, with_token(get("/api/1/checks/last"), TOKEN_READ)).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-124"));
        assert!(refused.json()["error"]["detail"].as_str().unwrap().contains("security:read"));
        let all = send(&world, with_token(get("/api/1/checks/last"), TOKEN_ALL)).await;
        assert_eq!((all.status, all.error().as_str()), (501, "ADM-129"));
    });
}

#[test]
fn the_new_reads_run_their_commands_in_the_person_s_worker_and_never_as_an_administrator() {
    run(async {
        let world = world("actions-reads");
        let dave = sign_in(&world, "dave").await;
        let usage = "/api/1/metrics/usage/7d";
        let asked: [(&str, &str); 4] = [
            ("/api/1/console/about", "admin about --json"),
            ("/api/1/console/notices", "admin notices --json"),
            ("/api/1/metrics/now", "metrics now --json"),
            (
                usage,
                "metrics show cpu memory.used memory.available disk.read disk.write net.rx \
                 net.tx --since 7d --every 2h --json",
            ),
        ];
        for (path, command) in asked {
            // The test worker has no such programs: what is asked of it is
            // what the route made of the address.
            let read = send(&world, get(path).by(&dave)).await;
            assert_eq!(read.status, 501, "{path}: {}", read.body);
            let words: Vec<&str> = command.split(' ').collect();
            let argv = json!({ "argv": words });
            assert!(called(&world).contains(&("run".into(), argv)), "{path} {:?}", called(&world));
        }
        assert!(elevations(&world).is_empty());
        // A word that is no range is refused before the worker hears of it,
        // with its field.
        let before = called(&world).len();
        let bad = send(&world, get("/api/1/metrics/usage/2h").by(&dave)).await;
        assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-125"));
        assert_eq!(bad.json()["error"]["field"], "range");
        assert_eq!(called(&world).len(), before);
        // The address takes the word whole.
        for path in
            ["/api/1/metrics/usage", "/api/1/metrics/usage/7d/more", "/api/1/metrics/usage/"]
        {
            let missing = send(&world, get(path).by(&dave)).await;
            assert_eq!(missing.status, 404, "{path}");
        }
        let posted = send(&world, post(usage, &json!({})).by(&dave)).await;
        assert_eq!((posted.status, posted.error().as_str()), (405, "ADM-117"));
        // A token for metrics alone reads the usage and is refused the rest.
        let metrics = send(&world, with_token(get("/api/1/metrics/now"), TOKEN_ALL)).await;
        assert_eq!(metrics.status, 501);
        let other = send(&world, with_token(get("/api/1/metrics/now"), TOKEN_OTHER)).await;
        assert_eq!((other.status, other.error().as_str()), (403, "ADM-124"));
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
        assert_eq!(
            done.json(),
            json!({ "done": true, "changed": true, "output": ["nginx restart"] })
        );
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
                "stderr": "starting broken\nbroken.conf did not load (SVC-104)\n",
            })
        );
    });
}

/// The commands run so far, in order: `person: ...` for each the worker
/// ran as the person, `administrator: ...` for each authd ran.
fn ran(world: &World) -> Vec<String> {
    let words = |list: &Value| {
        let words: Vec<&str> = list.as_array().unwrap().iter().filter_map(Value::as_str).collect();
        words.join(" ")
    };
    called(world)
        .iter()
        .filter_map(|(method, params)| match method.as_str() {
            "run" => Some(format!("person: {}", words(&params["argv"]))),
            "elevate" => Some(format!("administrator: {}", words(params))),
            _ => None,
        })
        .collect()
}

#[test]
fn a_change_whose_probe_reads_the_same_twice_answers_changed_false() {
    run(async {
        let world = lab_world("changed-same");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let done = send(&world, post("/api/1/lab/steady/set", &json!({})).by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        // What the probe prints after its semicolon moved, 1 and then 2,
        // and the action does not compare it.
        assert_eq!(
            done.json(),
            json!({ "done": true, "changed": false, "output": ["steady set"] })
        );
        // The probe ran in the person's worker, the command through authd,
        // and the probe again: the change itself ran once.
        assert_eq!(
            ran(&world),
            ["person: probe steady", "administrator: service steady set", "person: probe steady"]
        );
        assert_eq!(elevations(&world).len(), 1);
    });
}

#[test]
fn a_change_whose_probe_reads_differently_answers_changed_true() {
    run(async {
        let world = lab_world("changed-moved");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let done = send(&world, post("/api/1/lab/moving/set", &json!({})).by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(done.json(), json!({ "done": true, "changed": true, "output": ["moving set"] }));
        assert_eq!(
            ran(&world),
            ["person: probe moving", "administrator: service moving set", "person: probe moving"]
        );
        // Asked again, it reads the same as it did the second time.
        let again = send(&world, post("/api/1/lab/moving/set", &json!({})).by(&chase)).await;
        assert_eq!(again.json()["changed"], false);
    });
}

#[test]
fn a_probe_that_fails_or_finds_nothing_says_changed_and_the_change_still_succeeds() {
    run(async {
        let world = lab_world("changed-cannot-tell");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        // Failing before and after, failing after only, and an output the
        // action finds what it moves nowhere in.
        for name in ["fails", "fails-after", "unreadable"] {
            let done =
                send(&world, post(&format!("/api/1/lab/{name}/set"), &json!({})).by(&chase)).await;
            assert_eq!(done.status, 200, "{name}: {}", done.body);
            assert_eq!(
                done.json(),
                json!({ "done": true, "changed": true, "output": [format!("{name} set")] }),
                "{name}"
            );
        }
        // Each ran its probe twice, and the command between them.
        let probe = |name: &str| format!("person: probe {name}");
        let command = |name: &str| format!("administrator: service {name} set");
        let expected: Vec<String> = ["fails", "fails-after", "unreadable"]
            .iter()
            .flat_map(|name| [probe(name), command(name), probe(name)])
            .collect();
        assert_eq!(ran(&world), expected);
    });
}

#[test]
fn a_change_without_a_probe_always_says_changed_and_a_read_says_nothing_of_it() {
    run(async {
        let world = lab_world("changed-always");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let restart = || post("/api/1/lab/nginx/restart", &json!({})).by(&chase);
        for _ in 0..2 {
            let done = send(&world, restart()).await;
            assert_eq!(
                done.json(),
                json!({ "done": true, "changed": true, "output": ["nginx restart"] })
            );
        }
        let read = send(&world, get("/api/1/lab").by(&chase)).await;
        assert_eq!(read.json(), json!([{ "name": "nginx" }]));
        assert_eq!(
            ran(&world),
            [
                "administrator: service nginx restart",
                "administrator: service nginx restart",
                "person: service list --json",
            ]
        );
    });
}

#[test]
fn a_change_that_fails_runs_no_second_probe_and_answers_its_failure_as_before() {
    run(async {
        let world = lab_world("changed-failed");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let failed = send(&world, post("/api/1/lab/broken/set", &json!({})).by(&chase)).await;
        assert_eq!(failed.status, 422, "{}", failed.body);
        let error = &failed.json()["error"];
        assert_eq!(error["id"], "SVC-104");
        assert_eq!(error["detail"], "broken.conf did not load (SVC-104)");
        assert_eq!(error["command"]["argv"], json!(["service", "broken", "set"]));
        assert!(failed.json().get("changed").is_none());
        assert_eq!(ran(&world), ["person: probe broken", "administrator: service broken set"]);
    });
}

#[test]
fn a_change_that_is_refused_runs_no_probe() {
    run(async {
        let world = lab_world("changed-refused");
        let set = |token: Option<&str>, session: Option<&Session>| {
            let req = post("/api/1/lab/steady/set", &json!({}));
            match (token, session) {
                (Some(token), _) => with_token(req, token),
                (None, Some(session)) => req.by(session),
                (None, None) => req,
            }
        };
        let dave = sign_in(&world, "dave").await;
        let not_admin = send(&world, set(None, Some(&dave))).await;
        assert_eq!(not_admin.error(), "ADM-122");
        let chase = sign_in(&world, "chase").await;
        let locked = send(&world, set(None, Some(&chase))).await;
        assert_eq!(locked.error(), "ADM-106");
        let read_only = send(&world, set(Some(TOKEN_READ), None)).await;
        assert_eq!(read_only.error(), "ADM-124");
        let other_area = send(&world, set(Some(TOKEN_OTHER), None)).await;
        assert_eq!(other_area.error(), "ADM-124");
        let bad = send(&world, post("/api/1/lab/-x/set", &json!({})).by(&chase)).await;
        assert_eq!(bad.error(), "ADM-125");
        assert!(ran(&world).is_empty(), "{:?}", ran(&world));
        assert_eq!(world.console.authd.verbs().iter().filter(|v| *v == "token-start").count(), 0);
    });
}

#[test]
fn a_token_s_change_reads_its_probe_in_the_owner_s_worker_started_once() {
    run(async {
        let world = lab_world("changed-token");
        let set =
            |name: &str| with_token(post(&format!("/api/1/lab/{name}/set"), &json!({})), TOKEN_ALL);
        let same = send(&world, set("steady")).await;
        assert_eq!(same.status, 200, "{}", same.body);
        assert_eq!(
            same.json(),
            json!({ "done": true, "changed": false, "output": ["steady set"] })
        );
        let moved = send(&world, set("moving")).await;
        assert_eq!(moved.json()["changed"], true);
        assert_eq!(
            ran(&world),
            [
                "person: probe steady",
                "administrator: service steady set",
                "person: probe steady",
                "person: probe moving",
                "administrator: service moving set",
                "person: probe moving",
            ]
        );
        let verbs = world.console.authd.verbs();
        assert_eq!(verbs.iter().filter(|verb| *verb == "token-start").count(), 1, "{verbs:?}");
        assert_eq!(elevations(&world).len(), 2);
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

/// The answer a restart of nginx is shown as, at the console's address.
fn shown_restart() -> Value {
    json!({
        "command": "elevate service nginx restart",
        "api": "curl -sS -X POST -H \"Authorization: Bearer $DUKOVO_TOKEN\" \
                -H \"Content-Type: application/json\" -d '{}' \
                https://nas.example:9090/api/1/lab/nginx/restart",
        "ansible": "- name: \"Restart a lab service: nginx\"\n  ansible.builtin.command:\n    \
                    argv:\n      - \"service\"\n      - \"nginx\"\n      - \"restart\"\n  \
                    become: true\n",
        "files": [],
    })
}

/// Asks to see a restart of nginx.
fn show_restart() -> Req {
    post("/api/1/show", &json!({ "action": "lab.restart", "args": { "name": "nginx" } }))
}

#[test]
fn any_signed_in_session_is_shown_an_action_locked_or_not_and_nothing_is_run() {
    run(async {
        let world = lab_world("show-session");
        // An administrator whose session is locked, and a person who is no
        // administrator at all: neither could run the change.
        for name in ["chase", "dave"] {
            let session = sign_in(&world, name).await;
            let shown = send(&world, show_restart().by(&session)).await;
            assert_eq!(shown.status, 200, "{name}: {}", shown.body);
            assert_eq!(shown.json(), shown_restart(), "{name}");
        }
        let session = sign_in(&world, "dave").await;
        let read = post("/api/1/show", &json!({ "action": "lab.list" })).by(&session);
        let shown = send(&world, read).await;
        assert_eq!(shown.status, 200, "{}", shown.body);
        assert_eq!(shown.json()["command"], "service list --json");
        assert_eq!(
            shown.json()["api"],
            "curl -sS -H \"Authorization: Bearer $DUKOVO_TOKEN\" https://nas.example:9090/api/1/lab"
        );
        assert!(called(&world).iter().all(|(method, _)| method != "run"), "{:?}", called(&world));
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_token_is_shown_an_action_of_an_area_it_may_read_even_if_it_may_not_change_it() {
    run(async {
        let world = lab_world("show-token");
        for token in [TOKEN_ALL, TOKEN_READ] {
            let shown = send(&world, with_token(show_restart(), token)).await;
            assert_eq!(shown.status, 200, "{}", shown.body);
            assert_eq!(shown.json(), shown_restart());
        }
        // The token's own request needs no cookie and no CSRF token, and
        // nothing was asked of a worker or of authd to run.
        assert!(called(&world).iter().all(|(method, _)| method != "run"));
        assert!(elevations(&world).is_empty());
        assert_eq!(
            world.console.authd.verbs().iter().filter(|verb| *verb == "token-start").count(),
            0
        );
    });
}

#[test]
fn a_token_of_another_area_is_refused_with_the_scope_it_needs() {
    run(async {
        let world = lab_world("show-other-area");
        let refused = send(&world, with_token(show_restart(), TOKEN_OTHER)).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-124"));
        assert_eq!(
            refused.json()["error"]["detail"],
            "the API token \"other\" may not do this: it needs the scope services:read, services or *"
        );
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn an_action_that_is_not_in_the_list_is_adm_125_for_the_field_action() {
    run(async {
        let world = lab_world("show-unknown");
        let session = sign_in(&world, "dave").await;
        let body = json!({ "action": "lab.explode", "args": {} });
        let refused = send(&world, post("/api/1/show", &body).by(&session)).await;
        assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-125"));
        let error = &refused.json()["error"];
        assert_eq!(error["field"], "action");
        assert_eq!(
            error["detail"],
            "There is no action lab.explode: admin actions lists them. Nothing was done."
        );
        let by_token = send(&world, with_token(post("/api/1/show", &body), TOKEN_READ)).await;
        assert_eq!(by_token.error(), "ADM-125");
    });
}

#[test]
fn arguments_are_checked_as_the_action_checks_them() {
    run(async {
        let world = lab_world("show-arguments");
        let session = sign_in(&world, "dave").await;
        let ask = |body: Value| post("/api/1/show", &body).by(&session);
        let bad =
            send(&world, ask(json!({ "action": "lab.restart", "args": { "name": "-x" } }))).await;
        assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-125"));
        assert_eq!(bad.json()["error"]["field"], "name");
        assert_eq!(bad.json()["error"]["detail"], "A name does not start with -.");
        let wait = json!({ "action": "lab.restart", "args": { "name": "nginx", "wait": 61 } });
        assert_eq!(send(&world, ask(wait)).await.json()["error"]["field"], "wait");
        for body in [
            json!({ "action": "lab.restart", "args": { "name": "nginx", "force": true } }),
            json!({ "action": "lab.restart", "args": { "name": 7 } }),
            json!({ "action": "lab.restart", "args": {} }),
            json!({ "action": "lab.restart", "args": [] }),
            json!({ "action": "lab.restart" }),
            json!({ "action": "lab.list", "args": { "name": "nginx" } }),
            json!({ "args": {} }),
            json!({ "action": 7 }),
            json!({ "action": "lab.list", "more": true }),
            json!([]),
        ] {
            let refused = send(&world, ask(body.clone())).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-108"), "{body}");
        }
        let empty = Req { body: Vec::new(), ..ask(json!({})) };
        assert_eq!(send(&world, empty).await.error(), "ADM-108");
        // An action that takes nothing takes its arguments left out.
        let list = send(&world, ask(json!({ "action": "lab.list" }))).await;
        assert_eq!(list.status, 200, "{}", list.body);
        assert!(called(&world).iter().all(|(method, _)| method != "run"));
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_showing_is_a_post_that_a_session_makes_as_every_change_is_made() {
    run(async {
        let world = lab_world("show-guards");
        let session = sign_in(&world, "dave").await;
        let forged = show_restart().by(&session).with("x-dukovo-csrf", Some("forged"));
        let refused = send(&world, forged).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-109"));
        let foreign = show_restart().by(&session).with("origin", Some("https://evil.example"));
        assert_eq!(send(&world, foreign).await.error(), "ADM-109");
        let nobody = send(&world, show_restart()).await;
        assert_eq!((nobody.status, nobody.error()), (401, Kind::SessionEnded.id().to_string()));
        let mut got = show_restart().by(&session);
        got.method = "GET";
        got.body = Vec::new();
        assert_eq!(send(&world, got).await.error(), "ADM-117");
        // A session that may only add a second step may not.
        let eve = sign_in(&world, "eve").await;
        assert_eq!(send(&world, show_restart().by(&eve)).await.error(), "ADM-118");
        // A token and a cookie together are refused.
        let both = with_token(show_restart(), TOKEN_ALL)
            .with("cookie", Some(&format!("{SESSION_COOKIE}={}", session.cookie)));
        assert_eq!(send(&world, both).await.error(), "ADM-108");
        assert!(called(&world).iter().all(|(method, _)| method != "run"));
        assert!(elevations(&world).is_empty());
    });
}

/// A request to check `id` of the lab's actions with `args`.
fn check(id: &str, args: &Value) -> Req {
    post("/api/1/check", &json!({ "action": id, "args": args }))
}

#[test]
fn a_check_reads_the_probe_as_the_person_and_runs_no_change() {
    run(async {
        let world = lab_world("check-session");
        // Locked, and no administrator at all: neither could run the
        // change, and both may ask whether it would change anything.
        for name in ["chase", "dave"] {
            let session = sign_in(&world, name).await;
            let steady = send(&world, check("lab.set", &json!({ "name": "steady" })).by(&session));
            let steady = steady.await;
            assert_eq!(steady.status, 200, "{name}: {}", steady.body);
            assert_eq!(steady.json(), json!({ "changed": false, "now": "on" }), "{name}");
        }
        // The probe of `moving` reads `off` the first time, which the
        // change would move.
        let chase = sign_in(&world, "chase").await;
        let moving = send(&world, check("lab.set", &json!({ "name": "moving" })).by(&chase)).await;
        assert_eq!(moving.json(), json!({ "changed": true, "now": "off" }));
        assert_eq!(
            ran(&world),
            ["person: probe steady", "person: probe steady", "person: probe moving"]
        );
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_check_that_cannot_tell_says_null_and_one_with_nothing_to_compare_says_true() {
    run(async {
        let world = lab_world("check-cannot-tell");
        let chase = sign_in(&world, "chase").await;
        // A probe that fails, and one whose output has nothing the action
        // picks.
        for name in ["fails", "unreadable"] {
            let done = send(&world, check("lab.set", &json!({ "name": name })).by(&chase)).await;
            assert_eq!(done.status, 200, "{name}: {}", done.body);
            assert_eq!(done.json(), json!({ "changed": null, "now": null }), "{name}");
        }
        // A restart compares nothing: it always does something. A read
        // changes nothing. Neither runs a probe.
        let restart = send(&world, check("lab.restart", &json!({ "name": "nginx" })).by(&chase));
        assert_eq!(restart.await.json(), json!({ "changed": true, "now": null }));
        let read = send(&world, check("lab.list", &json!({})).by(&chase)).await;
        assert_eq!(read.json(), json!({ "changed": false, "now": null }));
        assert_eq!(ran(&world), ["person: probe fails", "person: probe unreadable"]);
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_token_checks_an_action_of_an_area_it_reads_with_its_owner_s_worker() {
    run(async {
        let world = lab_world("check-token");
        let steady = || check("lab.set", &json!({ "name": "steady" }));
        for token in [TOKEN_ALL, TOKEN_READ] {
            let done = send(&world, with_token(steady(), token)).await;
            assert_eq!(done.status, 200, "{}", done.body);
            assert_eq!(done.json(), json!({ "changed": false, "now": "on" }));
        }
        let refused = send(&world, with_token(steady(), TOKEN_OTHER)).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-124"));
        let wrong = "dkv_chase_0000000000000000000000000000000000000000000000000000000000000000";
        let unknown = send(&world, with_token(steady(), wrong)).await;
        assert_eq!((unknown.status, unknown.error().as_str()), (401, "ADM-115"));
        // The refused token ran no probe; the others ran one each.
        assert_eq!(ran(&world), ["person: probe steady", "person: probe steady"]);
        assert!(elevations(&world).is_empty());
    });
}

#[test]
fn a_check_takes_what_a_showing_takes_with_its_guards() {
    run(async {
        let world = lab_world("check-guards");
        let dave = sign_in(&world, "dave").await;
        let unknown = check("lab.explode", &json!({})).by(&dave);
        let refused = send(&world, unknown).await;
        assert_eq!((refused.status, refused.error().as_str()), (400, "ADM-125"));
        assert_eq!(refused.json()["error"]["field"], "action");
        let bad = send(&world, check("lab.set", &json!({ "name": "-x" })).by(&dave)).await;
        assert_eq!(
            (bad.error().as_str(), &bad.json()["error"]["field"]),
            ("ADM-125", &json!("name"))
        );
        let extra = check("lab.set", &json!({ "name": "steady", "force": true })).by(&dave);
        assert_eq!(send(&world, extra).await.error(), "ADM-108");
        let forged = check("lab.set", &json!({ "name": "steady" }))
            .by(&dave)
            .with("x-dukovo-csrf", Some("x"));
        assert_eq!(send(&world, forged).await.error(), "ADM-109");
        let nobody = send(&world, check("lab.set", &json!({ "name": "steady" }))).await;
        assert_eq!((nobody.status, nobody.error()), (401, Kind::SessionEnded.id().to_string()));
        let eve = sign_in(&world, "eve").await;
        let limited = check("lab.set", &json!({ "name": "steady" })).by(&eve);
        assert_eq!(send(&world, limited).await.error(), "ADM-118");
        assert!(called(&world).iter().all(|(method, _)| method != "run"));
        assert!(elevations(&world).is_empty());
    });
}
