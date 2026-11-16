// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An account's console sessions and terminals, as the Accounts page's
//! Signed in now lists and ends them (ADR 0562): chase, an administrator,
//! and dave, who is not one, with the test authd.

use super::*;

/// The address of `name`'s sessions, and of ending session `id`.
fn sessions_of(name: &str) -> String {
    format!("/api/1/accounts/{name}/sessions")
}

fn end_of(name: &str, id: &str) -> String {
    format!("/api/1/accounts/{name}/sessions/{id}/end")
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// Whether the test authd is told a line that `wanted` holds for within a
/// second: admind tells it after the answer.
async fn told(world: &World, wanted: impl Fn(&str) -> bool) -> bool {
    for _ in 0..100 {
        if world.console.authd.lines().iter().any(|line| wanted(line)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// `name`'s sessions and terminals as `asker` sees them.
async fn listed(world: &World, asker: &Session, name: &str) -> Value {
    let answered = send(world, get(&sessions_of(name)).by(asker)).await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    answered.json()
}

#[test]
fn an_account_sees_its_own_sessions_and_ends_another_of_them_but_never_the_one_in_use() {
    run(async {
        let world = world("signed-in-own");
        let here = sign_in(&world, "chase").await;
        let there = sign_in_with(&world, "chase", "elsewhere").await;
        let seen = listed(&world, &here, "chase").await;
        let sessions = seen["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2, "{seen}");
        assert_eq!(seen["terminals"], json!([]));
        // The oldest first, the one in use marked, and the browser as the
        // sign-in named it: the test's requests send no User-Agent.
        assert_eq!(
            sessions.iter().map(|s| s["this"].clone()).collect::<Vec<_>>(),
            [json!(true), json!(false)]
        );
        for session in sessions {
            assert_eq!(
                (&session["browser"], &session["from"]),
                (&json!("a browser"), &json!(PEER))
            );
            assert!(session["since_ms"].as_i64().unwrap() <= session["last_ms"].as_i64().unwrap());
            assert_eq!(session["id"].as_str().unwrap().len(), 16);
            // Nothing that opens it.
            assert!(!session.to_string().contains(&here.cookie));
            assert!(!session.to_string().contains(&there.cookie));
        }
        let (mine, other) =
            (sessions[0]["id"].as_str().unwrap(), sessions[1]["id"].as_str().unwrap());

        // The session in use is Sign out's.
        let refused = send(&world, post(&end_of("chase", mine), &json!({})).by(&here)).await;
        assert_eq!((refused.status, refused.error().as_str()), (409, "ADM-136"));
        // The other ends as a sign-out does, and its tab is told why.
        let mut tab = tab(&world, &there).await;
        let ended = send(&world, post(&end_of("chase", other), &json!({})).by(&here)).await;
        assert_eq!(ended.json(), json!({ "done": true }), "{}", ended.body);
        assert_eq!(tab().await, "event: session\ndata: {\"ended\":\"by-you\"}\n\n");
        let gone = send(&world, get("/api/1/session").by(&there)).await;
        assert_eq!(gone.status, 401);
        assert!(gone.body.contains("you ended this session from another browser"), "{}", gone.body);
        // authd is told who, and the same id again has ended already.
        let line = format!("end 0a0b11{} by chase", "0".repeat(26));
        assert!(told(&world, |told| told == line).await, "{:?}", world.console.authd.lines());
        let again = send(&world, post(&end_of("chase", other), &json!({})).by(&here)).await;
        assert_eq!((again.status, again.error().as_str()), (404, "ADM-139"));
        assert_eq!(listed(&world, &here, "chase").await["sessions"].as_array().unwrap().len(), 1);
    });
}

#[test]
fn another_accounts_sessions_are_an_unlocked_administrators_alone() {
    run(async {
        let world = world("signed-in-other");
        let chase = sign_in(&world, "chase").await;
        let dave = sign_in(&world, "dave").await;
        // dave sees his own, and none of chase's.
        assert_eq!(listed(&world, &dave, "dave").await["sessions"][0]["this"], json!(true));
        let refused = send(&world, get(&sessions_of("chase")).by(&dave)).await;
        assert_eq!((refused.status, refused.error().as_str()), (403, "ADM-122"));
        // chase, locked, is asked to unlock first, for the list and an end.
        let locked = send(&world, get(&sessions_of("dave")).by(&chase)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        let locked =
            send(&world, post(&end_of("dave", "0011223344556677"), &json!({})).by(&chase)).await;
        assert_eq!(locked.error(), "ADM-106");

        unlock(&world, &chase).await;
        let seen = listed(&world, &chase, "dave").await;
        let sessions = seen["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "{seen}");
        assert_eq!(sessions[0]["this"], json!(false));
        // An account with none, or that is not there, has empty lists.
        assert_eq!(
            listed(&world, &chase, "nobody").await,
            json!({ "sessions": [], "terminals": [] })
        );

        let mut tab = tab(&world, &dave).await;
        let id = sessions[0]["id"].as_str().unwrap();
        // The id is dave's: under another account's name it is not there.
        let wrong = send(&world, post(&end_of("chase", id), &json!({})).by(&chase)).await;
        assert_eq!(wrong.error(), "ADM-139");
        let ended = send(&world, post(&end_of("dave", id), &json!({})).by(&chase)).await;
        assert_eq!(ended.json(), json!({ "done": true }), "{}", ended.body);
        assert_eq!(tab().await, "event: session\ndata: {\"ended\":\"by-administrator\"}\n\n");
        let gone = send(&world, get("/api/1/session").by(&dave)).await;
        assert!(gone.body.contains("an administrator ended this session"), "{}", gone.body);
        let by_chase = |line: &str| line.starts_with("end 0a0b04") && line.ends_with(" by chase");
        assert!(told(&world, by_chase).await, "{:?}", world.console.authd.lines());
    });
}

#[test]
fn an_unlocked_administrator_sees_and_ends_another_accounts_terminal_and_authd_is_told() {
    run(async {
        let world = world("signed-in-terminals");
        let alice = super::tasks::sign_in_alice(&world).await;
        let body = json!({ "cols": 80, "rows": 24, "name": "build" });
        let opened = send(&world, post("/api/1/terminals", &body).by(&alice)).await;
        assert_eq!(opened.status, 200, "{}", opened.body);
        let terminal = opened.json()["terminal"]["id"].as_u64().unwrap();
        let path = format!("/api/1/terminals/{terminal}/end");

        // dave, no administrator, finds no terminal of alice's to end.
        let dave = sign_in(&world, "dave").await;
        let refused = send(&world, post(&path, &json!({})).by(&dave)).await;
        assert_eq!((refused.status, refused.error().as_str()), (404, "ADM-131"));
        // chase, an administrator, is asked to unlock first.
        let chase = sign_in(&world, "chase").await;
        let locked = send(&world, post(&path, &json!({})).by(&chase)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        assert!(!world.console.authd.verbs().contains(&"terminal-ended".to_string()));

        unlock(&world, &chase).await;
        // The list has the terminal, with nothing of what it shows.
        let seen = listed(&world, &chase, "alice").await;
        let shown = &seen["terminals"][0];
        assert_eq!(
            (&shown["id"], &shown["title"], &shown["panes"], &shown["admin"], &shown["attached"]),
            (&json!(terminal), &json!("build"), &json!(1), &json!(false), &json!(false))
        );
        assert!(shown["since_ms"].as_i64().unwrap() > 1_700_000_000_000);
        // Its pane's pseudo-terminal, which the page leaves out of the
        // sessions at the machine that `user show` lists.
        let ttys = shown["ttys"].as_array().unwrap();
        assert!(ttys.len() == 1 && ttys[0].as_str().unwrap().starts_with("pts/"), "{shown}");
        let ended = send(&world, post(&path, &json!({})).by(&chase)).await;
        assert_eq!(ended.json(), json!({ "done": true }), "{}", ended.body);
        assert_eq!(listed(&world, &alice, "alice").await["terminals"], json!([]));
        // authd hears whose, by whom and its name, for its audit line and
        // alice's next sign-in; it is told after the answer.
        let line = |told: &str| told == "terminal-ended alice chase build";
        assert!(told(&world, line).await, "{:?}", world.console.authd.lines());
    });
}

#[test]
fn the_console_does_not_disable_or_demote_the_callers_own_account_by_session_or_token() {
    run(async {
        let world = world("not-self");
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let elevated = || {
            let calls = world.calls.lock().unwrap();
            calls.iter().filter(|(name, _)| name == "elevate").count()
        };
        for (path, body) in [
            ("/api/1/accounts/chase/disable", json!({})),
            ("/api/1/accounts/chase/set", json!({ "admin": false })),
            ("/api/1/accounts/chase/remove", json!({ "home": "keep" })),
            ("/api/1/accounts/chase/end", json!({ "session": "41-2100" })),
        ] {
            let refused = send(&world, post(path, &body).by(&chase)).await;
            assert_eq!((refused.status, refused.error().as_str()), (409, "ADM-136"), "{path}");
            assert!(refused.body.contains("elevate user"), "{}", refused.body);
        }
        // With chase's own token, as with a session of his.
        let bearer = format!("Bearer {TOKEN_ALL}");
        let by_token = post("/api/1/accounts/chase/disable", &json!({}))
            .with("origin", None)
            .with("authorization", Some(&bearer));
        assert_eq!(send(&world, by_token).await.error(), "ADM-136");
        assert_eq!(elevated(), 0, "nothing ran");

        // His full name, and another account, are his to change.
        let named = json!({ "full_name": "Chase" });
        let done = send(&world, post("/api/1/accounts/chase/set", &named).by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        let other = send(&world, post("/api/1/accounts/dave/disable", &json!({})).by(&chase)).await;
        assert_eq!(other.status, 200, "{}", other.body);
        assert_eq!(elevated(), 2);
    });
}
