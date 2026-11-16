// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A change that could cut off the browser that asks for it (ADR 0562,
//! ADM-137), with the test action `lab.cut`, which can cut off what comes
//! by `lo` as `private.remove` can what comes by its WireGuard interface:
//! admind reads what the change would take away, as the caller's
//! administrator, before it runs anything, and refuses when that holds the
//! browser's address or cannot be read.

use std::net::SocketAddr;

use super::trials::send_on;
use super::*;
use crate::admind::actions::tests::CUTTING;

/// A world whose console offers `lab.cut`.
fn cutting_world(what: &str) -> World {
    let world = world(what);
    world.console.shared.actions.set(CUTTING);
    world
}

/// A connection that came to the console's port on `to`: `lo`'s address,
/// or one that is no card's.
fn came_to(to: &str) -> Rc<Connection> {
    let to: SocketAddr = to.parse().unwrap();
    Rc::new(Connection::new(Some(to), None))
}

/// The commands authd ran as an administrator, in order.
fn elevated_commands(world: &World) -> Vec<Value> {
    let calls = world.calls.lock().unwrap();
    calls.iter().filter(|(name, _)| name == "elevate").map(|(_, command)| command.clone()).collect()
}

fn cut(name: &str) -> Req {
    post(&format!("/api/1/lab/{name}/cut"), &json!({}))
}

#[test]
fn a_change_that_would_cut_off_its_browser_is_refused_before_it_runs() {
    run(async {
        let world = cutting_world("cuts-off");
        let chase = sign_in(&world, "chase").await;
        let body = json!({ "kind": "password", "password": "right" });
        assert_eq!(send(&world, post("/api/1/unlock", &body).by(&chase)).await.status, 200);
        let lo = came_to("127.0.0.1:9090");

        // The browser's own device: read, then refused, and its command
        // never runs, nor has the journal a row of it.
        let refused = send_on(&world, cut("phone").by(&chase), &lo).await;
        assert_eq!((refused.status, refused.error().as_str()), (409, "ADM-137"));
        assert!(refused.body.contains("through phone"), "{}", refused.body);
        assert_eq!(elevated_commands(&world), [json!(["lab-peers", "phone"])]);
        let changes = send(&world, get("/api/1/changes").by(&chase)).await.json();
        assert_eq!(changes["changes"], json!([]), "{changes}");

        // Another device is read and then removed.
        let done = send_on(&world, cut("laptop").by(&chase), &lo).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(
            elevated_commands(&world)[1..],
            [json!(["lab-peers", "laptop"]), json!(["lab-cut", "laptop"])]
        );

        // A read that fails refuses the change: the guard fails closed.
        let broken = send_on(&world, cut("broken").by(&chase), &lo).await;
        assert_eq!(broken.error(), "ADM-137");
        assert!(broken.body.contains("could not be read"), "{}", broken.body);
        assert_eq!(elevated_commands(&world).len(), 4);
    });
}

#[test]
fn a_request_by_another_card_is_never_cut_off_and_nothing_is_read_for_it() {
    run(async {
        let world = cutting_world("cuts-off-elsewhere");
        let chase = sign_in(&world, "chase").await;
        let body = json!({ "kind": "password", "password": "right" });
        assert_eq!(send(&world, post("/api/1/unlock", &body).by(&chase)).await.status, 200);
        // An address that is no card's, and a connection whose address
        // admind does not know.
        let elsewhere = send_on(&world, cut("phone").by(&chase), &came_to("192.0.2.10:9090")).await;
        assert_eq!(elsewhere.status, 200, "{}", elsewhere.body);
        let unknown = send(&world, cut("phone").by(&chase)).await;
        assert_eq!(unknown.status, 200, "{}", unknown.body);
        assert_eq!(elevated_commands(&world), vec![json!(["lab-cut", "phone"]); 2]);
    });
}

#[test]
fn the_read_is_the_callers_and_runs_only_for_one_who_may_make_the_change() {
    run(async {
        let world = cutting_world("cuts-off-who");
        let lo = came_to("127.0.0.1:9090");
        // Not an administrator, or one who has not unlocked: refused as
        // the change would be, with nothing read.
        let dave = sign_in(&world, "dave").await;
        let refused = send_on(&world, cut("phone").by(&dave), &lo).await;
        assert_eq!(refused.error(), "ADM-122");
        let chase = sign_in(&world, "chase").await;
        let locked = send_on(&world, cut("phone").by(&chase), &lo).await;
        assert_eq!(locked.error(), "ADM-106");
        assert_eq!(elevated_commands(&world), Vec::<Value>::new());

        // A token's read runs by the token, which authd checks.
        let bearer = format!("Bearer {TOKEN_ALL}");
        let by_token = cut("phone").with("origin", None).with("authorization", Some(&bearer));
        let refused = send_on(&world, by_token, &lo).await;
        assert_eq!(refused.error(), "ADM-137", "{}", refused.body);
        let seen = world.console.authd.seen.lock().unwrap().clone();
        let read = seen.iter().find(|line| line.contains("lab-peers")).unwrap();
        assert!(read.starts_with("elevate token "), "{read}");
    });
}
