// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Whether a browser reaches the console over a change being tried (ADR
//! 0518): `GET /api/1/console/fresh/{trial}`, with a Spark of the test's
//! own that lists the trials.

use std::net::SocketAddr;

use spark_api::show::{self, Show};

use super::*;
use crate::admind::trials::fake_spark;

/// Asks `req` by `connection`.
async fn send_on(world: &World, req: Req, connection: &Rc<Connection>) -> Answered {
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

/// Spark's document while the network's settings are tried.
fn trying_network() -> String {
    let document = Show {
        now_ms: 100_000,
        trials: vec![show::Trial {
            id: "network".into(),
            files: vec!["/system/settings/network".into()],
            services: vec!["netd".into()],
            user: 0,
            left_ms: 60_000,
        }],
        ..Show::default()
    };
    show::to_json(&document).unwrap()
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
        let path = std::env::temp_dir().join(format!("admind-fresh-{}", std::process::id()));
        let path: &'static str = path.to_string_lossy().into_owned().leak();
        world.console.shared.spark.set(path);
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
