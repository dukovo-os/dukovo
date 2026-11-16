// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Whether a browser reaches the console over a change being tried (ADR
//! 0518): `GET /api/1/console/fresh/{trial}`, with a Spark of the test's
//! own that lists the trials; and the `trial` event that tells every tab
//! which trials run.

use std::net::SocketAddr;

use spark_api::show::{self, Show};

use super::*;
use crate::admind::actions::tests::TRYING;
use crate::admind::serve::look_at_trials;
use crate::admind::trials::fake_spark;

/// Asks `req` by `connection`.
pub(super) async fn send_on(world: &World, req: Req, connection: &Rc<Connection>) -> Answered {
    let shared = Rc::clone(&world.console.shared);
    let Ok(response) =
        handle_on(shared, PEER.parse().unwrap(), Rc::clone(connection), build(req)).await;
    let (head, body) = response.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    Answered {
        status: head.status.as_u16(),
        headers: head.headers,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

/// Spark's document while the trial `id` has `left_ms` to run, which is
/// none of the others.
pub(super) fn trying(id: &str, left_ms: u64) -> String {
    let document = Show {
        now_ms: 100_000,
        trials: vec![show::Trial {
            id: id.into(),
            files: vec![format!("/system/settings/{id}")],
            services: vec!["netd".into()],
            user: 0,
            left_ms,
        }],
        ..Show::default()
    };
    show::to_json(&document).unwrap()
}

/// Spark's document while the network's settings are tried.
pub(super) fn trying_network() -> String {
    trying("network", 60_000)
}

/// Spark's document while nothing is tried.
pub(super) fn trying_nothing() -> String {
    show::to_json(&Show { now_ms: 100_000, ..Show::default() }).unwrap()
}

/// The path of the socket of the Spark that the console of `world` reads,
/// named for the test.
pub(super) fn spark_path(world: &World, what: &str) -> &'static str {
    let path = std::env::temp_dir().join(format!("admind-{what}-{}", std::process::id()));
    let path: &'static str = path.to_string_lossy().into_owned().leak();
    world.console.shared.spark.set(path);
    path
}

/// Whether `connection` was asked to end.
async fn asked_to_end(connection: &Connection) -> bool {
    tokio::time::timeout(Duration::from_millis(20), connection.ending.notified()).await.is_ok()
}

#[test]
fn a_browser_reaches_the_console_over_a_change_only_by_a_connection_opened_after_it() {
    run(async {
        let world = world("fresh");
        let chase = sign_in(&world, "chase").await;
        let path = spark_path(&world, "fresh");
        let to: SocketAddr = "192.0.2.10:9090".parse().unwrap();
        let old = Rc::new(Connection::new(Some(to), None));
        tokio::time::sleep(Duration::from_millis(5)).await;
        world.console.shared.trials.borrow_mut().begun("network", Instant::now());
        tokio::time::sleep(Duration::from_millis(5)).await;
        let new = Rc::new(Connection::new(Some(to), None));
        let spark = fake_spark(path, vec![trying_network(); 4]);
        let fresh = || get("/api/1/console/fresh/network").by(&chase);

        // Over the connection opened before, the answer proves nothing,
        // and the connection is asked to end so that the next is new.
        let before = send_on(&world, fresh(), &old).await;
        assert_eq!((before.status, before.error().as_str()), (409, "ADM-133"), "{}", before.body);
        assert!(asked_to_end(&old).await);
        assert_eq!(before.headers["connection"], "close", "HTTP/1.1 says the connection ends");

        let after = send_on(&world, fresh(), &new).await;
        assert_eq!(after.status, 200, "{}", after.body);
        assert_eq!(
            after.json(),
            json!({
                "fresh": true, "from": PEER,
                "via": { "address": "192.0.2.10", "port": 9090, "card": null },
            })
        );
        assert!(!asked_to_end(&new).await);
        assert!(!after.headers.contains_key("connection"));

        // Something Spark is not trying is no trial, whatever the
        // connection.
        let firewall = send_on(&world, get("/api/1/console/fresh/firewall").by(&chase), &new).await;
        assert_eq!((firewall.status, firewall.error().as_str()), (404, "ADM-135"));

        // A trial begun at a terminal began, for the console, when it first
        // saw Spark list it.
        world.console.shared.trials.borrow_mut().ended("network");
        let unseen = send_on(&world, fresh(), &new).await;
        assert_eq!((unseen.status, unseen.error().as_str()), (409, "ADM-133"));
        assert_eq!(spark.join().unwrap(), vec!["show\n"; 4]);

        // A name that is no trial's is not asked of Spark, and a Spark
        // that does not answer is the console's failure.
        let bad = send_on(&world, get("/api/1/console/fresh/-x").by(&chase), &new).await;
        assert_eq!((bad.status, bad.error().as_str()), (400, "ADM-108"));
        std::fs::remove_file(path).unwrap();
        let silent = send_on(&world, fresh(), &new).await;
        assert_eq!((silent.status, silent.error().as_str()), (500, "ADM-121"), "{}", silent.body);
        assert!(silent.body.contains("cannot ask Spark"), "{}", silent.body);
    });
}

#[test]
fn only_a_person_who_is_signed_in_asks_whether_their_browser_reaches_the_console() {
    run(async {
        let world = world("fresh-anonymous");
        let asked = send(&world, get("/api/1/console/fresh/network")).await;
        assert_eq!((asked.status, asked.error().as_str()), (401, "ADM-107"));
    });
}

#[test]
fn the_session_says_where_the_browser_came_from_and_the_address_it_came_to() {
    run(async {
        let world = world("session-via");
        let chase = sign_in(&world, "chase").await;
        let to: SocketAddr = "[::ffff:192.0.2.10]:9090".parse().unwrap();
        let connection = Rc::new(Connection::new(Some(to), None));
        let shown = send_on(&world, get("/api/1/session").by(&chase), &connection).await;
        assert_eq!(shown.json()["from"], PEER);
        assert_eq!(
            shown.json()["via"],
            json!({ "address": "192.0.2.10", "port": 9090, "card": null }),
            "an IPv4 address carried in IPv6 is written as itself"
        );
    });
}

pub(super) async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// The next event of a tab that is not a `change`, which each change that
/// the tests make has as well.
pub(super) async fn next_trial(next: &mut impl AsyncFnMut() -> String) -> String {
    loop {
        let told = next().await;
        if !told.starts_with("event: change") {
            return told;
        }
    }
}

/// Whether a tab is told nothing but rows of the journal for a moment.
pub(super) async fn no_trial(next: &mut impl AsyncFnMut() -> String) -> bool {
    tokio::time::timeout(Duration::from_millis(100), next_trial(next)).await.is_err()
}

/// The trials of the `trial` event `told`, in the order they are sent.
fn trial_list(told: &str) -> Vec<Value> {
    let data = told
        .strip_prefix("event: trial\ndata: ")
        .and_then(|rest| rest.strip_suffix("\n\n"))
        .unwrap_or_else(|| panic!("not a trial event: {told:?}"));
    let event: Value = serde_json::from_str(data).unwrap();
    assert_eq!(event.as_object().unwrap().len(), 1, "{told}");
    event["trials"].as_array().unwrap().clone()
}

/// What a trial of `list` says of itself but the time left, which is
/// as of when it was told and is given apart.
fn trial_words(trial: &Value) -> (&str, &str, &str, u64) {
    let text = |field: &str| trial[field].as_str().unwrap();
    assert_eq!(trial.as_object().unwrap().len(), 4, "{trial}");
    (text("id"), text("page"), text("what"), trial["left_ms"].as_u64().unwrap())
}

#[test]
fn a_change_that_begins_a_trial_tells_every_tab_once_spark_has_said_how_long_it_has() {
    run(async {
        let world = world("trial-told");
        world.console.shared.actions.set(TRYING);
        // Spark is asked once, by the change that began the trial.
        let spark = fake_spark(spark_path(&world, "trial-told"), vec![trying("firewall", 58_000)]);
        let dave = sign_in(&world, "dave").await;
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        // Dave is not an administrator and has not unlocked; both are told.
        let mut dave_tab = tab(&world, &dave).await;
        let mut chase_tab = tab(&world, &chase).await;
        let mut other_tab = tab(&world, &chase).await;
        assert!(no_trial(&mut dave_tab).await, "no trial runs");

        // A change that did not work begins nothing and asks nothing.
        let broken = send(&world, post("/api/1/lab/broken/try", &json!({})).by(&chase)).await;
        assert_eq!(broken.status, 422, "{}", broken.body);
        assert!(no_trial(&mut dave_tab).await);

        let begun = send(&world, post("/api/1/lab/web/try", &json!({})).by(&chase)).await;
        assert_eq!(begun.status, 200, "{}", begun.body);
        for next in [&mut dave_tab, &mut chase_tab, &mut other_tab] {
            let told = next_trial(next).await;
            let list = trial_list(&told);
            let [firewall] = list.as_slice() else { panic!("{told}") };
            let (id, page, what, left) = trial_words(firewall);
            assert_eq!((id, page, what), ("firewall", "/firewall", "The firewall's rules"));
            assert!((57_000..=58_000).contains(&left), "as of the moment it was told: {left}");
            assert!(no_trial(next).await, "told once");
        }

        // A tab that opens while the trial runs is told on opening, and once.
        let mut late = tab(&world, &dave).await;
        let told = next_trial(&mut late).await;
        assert_eq!(trial_list(&told).len(), 1, "{told}");
        assert!(no_trial(&mut late).await);

        // The keep ends it, and each tab is told so at once, with no list.
        let kept = send(&world, post("/api/1/lab/web/keep", &json!({})).by(&chase)).await;
        assert_eq!(kept.status, 200, "{}", kept.body);
        for next in [&mut dave_tab, &mut chase_tab, &mut other_tab, &mut late] {
            assert_eq!(next_trial(next).await, "event: trial\ndata: {\"trials\":[]}\n\n");
            assert!(no_trial(next).await);
        }
        // A tab that opens now has nothing to be told.
        let mut after = tab(&world, &chase).await;
        assert!(no_trial(&mut after).await);
        assert_eq!(spark.join().unwrap(), vec!["show\n"]);
    });
}

#[test]
fn the_round_tells_a_trial_begun_elsewhere_each_time_while_it_runs_and_none_once_when_it_ends() {
    run(async {
        let world = world("trial-round");
        let answers = [
            trying("network", 50_000),
            trying("network", 20_000),
            trying_nothing(),
            trying_nothing(),
        ];
        let spark = fake_spark(spark_path(&world, "trial-round"), answers.to_vec());
        let chase = sign_in(&world, "chase").await;
        let mut first = tab(&world, &chase).await;
        let shared = &world.console.shared;

        // Begun at a terminal, the trial is told when the round sees it.
        look_at_trials(shared).await;
        let told = first().await;
        let list = trial_list(&told);
        let (id, page, what, left) = trial_words(&list[0]);
        assert_eq!((id, page, what), ("network", "/networking", "The network settings"));
        assert!((49_000..=50_000).contains(&left), "{left}");

        // A tab that opens then is told on opening; the next round tells
        // both, with the time that is left.
        let mut second = tab(&world, &chase).await;
        assert_eq!(trial_list(&second().await).len(), 1);
        look_at_trials(shared).await;
        for next in [&mut first, &mut second] {
            let list = trial_list(&next().await);
            let (_, _, _, left) = trial_words(&list[0]);
            assert!((19_000..=20_000).contains(&left), "{left}");
        }

        // Spark undid it, or it was kept at a terminal: none, once.
        look_at_trials(shared).await;
        for next in [&mut first, &mut second] {
            assert_eq!(next().await, "event: trial\ndata: {\"trials\":[]}\n\n");
        }
        look_at_trials(shared).await;
        for next in [&mut first, &mut second] {
            assert!(no_trial(next).await, "nothing runs and nothing is left to tell");
        }
        assert_eq!(spark.join().unwrap(), vec!["show\n"; 4]);
    });
}
