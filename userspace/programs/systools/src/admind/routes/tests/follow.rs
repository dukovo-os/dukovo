// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Logs page's follow of the log (ADR 0518), with the test authd's
//! follows: what a follow is told, who may follow the whole log, the
//! checks of its query, its limits, and how each end is told and whether
//! admind asks authd to end log.

use http_body_util::combinators::UnsyncBoxBody;

use super::*;
use crate::admind::follow::FOLLOWS_MAX;

/// A follow's stream.
type Stream = UnsyncBoxBody<Bytes, Infallible>;

/// The stream `req` opens, which must be one.
async fn opened(world: &World, req: Req) -> Stream {
    let shared = Rc::clone(&world.console.shared);
    let Ok(response) = handle(shared, PEER.parse().unwrap(), build(req)).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    response.into_body()
}

/// The stream's next event, or None at its end.
async fn next(stream: &mut Stream) -> Option<String> {
    let frame = tokio::time::timeout(Duration::from_secs(5), stream.frame()).await.unwrap();
    frame.map(|frame| String::from_utf8(frame.unwrap().into_data().unwrap().to_vec()).unwrap())
}

/// The lines authd was sent that begin with `verb` and a space.
fn asked(world: &World, verb: &str) -> Vec<String> {
    let seen = world.console.authd.seen.lock().unwrap();
    seen.iter().filter(|line| line.starts_with(&format!("{verb} "))).cloned().collect()
}

/// Waits until `done`, for at most 5 seconds.
async fn eventually(done: impl Fn() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("it did not happen in 5 seconds");
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

#[test]
fn a_follow_tells_its_command_and_each_record_and_ends_when_the_session_begins_another() {
    run(async {
        let world = world("follow");
        let chase = sign_in(&world, "chase").await;
        let query = "/api/1/logs/follow?service=nginx&priority=warning&grep=timed+out";
        let mut first = opened(&world, get(query).by(&chase)).await;
        assert_eq!(
            next(&mut first).await.unwrap(),
            "event: command\ndata: {\"shown\":\"log --output json --follow --lines 0 --quiet \
             --service nginx --priority warning --grep 'timed out'\"}\n\n"
        );
        assert_eq!(next(&mut first).await.unwrap(), "event: record\ndata: {\"n\":1}\n\n");
        // A line that is not JSON is not a record; one with spaces is
        // written again on one line.
        assert_eq!(next(&mut first).await.unwrap(), "event: record\ndata: {\"n\":2}\n\n");
        let follows = asked(&world, "follow");
        assert_eq!(follows.len(), 1);
        assert!(follows[0].ends_with(" own nginx  warning timed out"), "{}", follows[0]);
        let mut second = opened(&world, get("/api/1/logs/follow").by(&chase)).await;
        assert_eq!(
            next(&mut first).await.unwrap(),
            "event: ended\ndata: {\"why\":\"replaced\"}\n\n"
        );
        assert_eq!(next(&mut first).await, None);
        assert!(next(&mut second).await.unwrap().starts_with("event: command\n"));
        // admind asks authd to end the first's log, and only it.
        eventually(|| asked(&world, "follow-end").len() == 1).await;
        // The second goes with its tab, and its log is ended too.
        drop(second);
        eventually(|| asked(&world, "follow-end").len() == 2).await;
    });
}

#[test]
fn the_whole_log_is_an_unlocked_administrator_s_from_the_console_s_own_page() {
    run(async {
        let world = world("follow-all");
        let all = "/api/1/logs/follow?all=true&kernel=true";
        let dave = sign_in(&world, "dave").await;
        let reader = send(&world, get(all).by(&dave)).await;
        assert_eq!((reader.status, reader.error().as_str()), (403, "ADM-122"));
        let chase = sign_in(&world, "chase").await;
        let locked = send(&world, get(all).by(&chase)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        unlock(&world, &chase).await;
        for site in ["cross-site", "same-site", "none"] {
            let foreign =
                send(&world, get(all).by(&chase).with("sec-fetch-site", Some(site))).await;
            assert_eq!((foreign.status, foreign.error().as_str()), (403, "ADM-109"), "{site}");
        }
        assert!(asked(&world, "follow").is_empty());
        let own_page = get(all).by(&chase).with("sec-fetch-site", Some("same-origin"));
        let mut stream = opened(&world, own_page).await;
        assert_eq!(
            next(&mut stream).await.unwrap(),
            "event: command\ndata: {\"shown\":\"elevate log --output json --follow --lines 0 \
             --quiet --kernel\"}\n\n"
        );
        let follows = asked(&world, "follow");
        assert!(follows[0].ends_with(" all  kernel"), "{}", follows[0]);
        assert!(next(&mut stream).await.unwrap().starts_with("event: record\n"));
        assert!(next(&mut stream).await.unwrap().starts_with("event: record\n"));
        // Locking ends it, and authd ends log on its own.
        send(&world, post("/api/1/lock", &json!({})).by(&chase)).await;
        assert_eq!(
            next(&mut stream).await.unwrap(),
            "event: ended\ndata: {\"why\":\"locked\"}\n\n"
        );
        assert_eq!(next(&mut stream).await, None);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(asked(&world, "follow-end").is_empty());
        // A reader follows the records of their own programs.
        let mut own = opened(&world, get("/api/1/logs/follow").by(&dave)).await;
        assert!(
            next(&mut own).await.unwrap().starts_with("event: command\ndata: {\"shown\":\"log ")
        );
        send(&world, post("/api/1/sign-out", &json!({})).by(&dave)).await;
        let ended = loop {
            let event = next(&mut own).await.unwrap();
            if event.starts_with("event: ended") {
                break event;
            }
        };
        assert_eq!(ended, "event: ended\ndata: {\"why\":\"session-ended\"}\n\n");
    });
}

#[test]
fn a_follow_s_query_is_checked_as_a_read_s_before_authd_is_asked() {
    run(async {
        let world = world("follow-query");
        let chase = sign_in(&world, "chase").await;
        for (query, id) in [
            ("since=1", "ADM-108"),
            ("lines=5", "ADM-108"),
            ("kernel=yes", "ADM-108"),
            ("service=nginx&service=sshd", "ADM-108"),
            ("service=nginx&kernel=true", "ADM-125"),
            ("service=..%2Fx", "ADM-125"),
            ("grep=%28", "ADM-125"),
            ("grep=", "ADM-125"),
            ("priority=loud", "ADM-125"),
        ] {
            let refused =
                send(&world, get(&format!("/api/1/logs/follow?{query}")).by(&chase)).await;
            assert_eq!((refused.status, refused.error().as_str()), (400, id), "{query}");
        }
        assert!(asked(&world, "follow").is_empty());
    });
}

#[test]
fn a_ninth_follow_is_told_to_wait_thirty_seconds() {
    run(async {
        let world = world("follow-many");
        for n in 0..FOLLOWS_MAX {
            world
                .console
                .shared
                .follows
                .borrow_mut()
                .begin(&format!("elsewhere-{n}"), false)
                .unwrap();
        }
        let chase = sign_in(&world, "chase").await;
        let refused = send(&world, get("/api/1/logs/follow").by(&chase)).await;
        assert_eq!(
            (refused.status, refused.error().as_str(), refused.header("retry-after")),
            (429, "ADM-134", Some("30"))
        );
        assert!(asked(&world, "follow").is_empty());
    });
}

#[test]
fn a_follow_whose_log_ends_by_itself_says_so_with_its_last_error() {
    run(async {
        let world = world("follow-failing");
        let chase = sign_in(&world, "chase").await;
        let mut stream = opened(&world, get("/api/1/logs/follow?service=failing").by(&chase)).await;
        assert!(next(&mut stream).await.unwrap().starts_with("event: command\n"));
        assert_eq!(
            next(&mut stream).await.unwrap(),
            "event: ended\ndata: {\"error\":\"log: logd is not running\",\"why\":\"failed\"}\n\n"
        );
        assert_eq!(next(&mut stream).await, None);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(asked(&world, "follow-end").is_empty());
    });
}

#[test]
fn a_hundred_records_go_in_a_second_and_the_rest_are_counted() {
    run(async {
        let world = world("follow-flood");
        let chase = sign_in(&world, "chase").await;
        let mut stream = opened(&world, get("/api/1/logs/follow?service=many").by(&chase)).await;
        assert!(next(&mut stream).await.unwrap().starts_with("event: command\n"));
        for n in 0..100 {
            let record = next(&mut stream).await.unwrap();
            assert_eq!(record, format!("event: record\ndata: {{\"n\":{n}}}\n\n"));
        }
        assert_eq!(next(&mut stream).await.unwrap(), "event: skipped\ndata: {\"count\":50}\n\n");
    });
}
