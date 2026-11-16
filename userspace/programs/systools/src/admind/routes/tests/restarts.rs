// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A change that restarts the machine (ADR 0565), with a Spark of the
//! test's own: refused while a trial runs (ADM-142) and while the search
//! for a bad change runs (UPD-106), and when either cannot be told; and
//! `event: power`, which tells every tab what is set to end the machine.

use spark_api::show::{self, Show};

use super::trials::{next_trial, no_trial, spark_path, trying_network, trying_nothing, unlock};
use super::*;
use crate::admind::errors::Kind;
use crate::admind::trials::fake_spark;

/// Spark's document while a restart is set for `in_seconds` from now.
fn restart_set(in_seconds: u64) -> String {
    let shutdown = show::Shutdown { end: "restart".into(), in_seconds };
    show::to_json(&Show { now_ms: 100_000, shutdown: Some(shutdown), ..Show::default() }).unwrap()
}

/// The commands authd ran as an administrator, in order.
fn elevated_commands(world: &World) -> Vec<Value> {
    let calls = world.calls.lock().unwrap();
    calls.iter().filter(|(name, _)| name == "elevate").map(|(_, command)| command.clone()).collect()
}

fn restart() -> Req {
    post("/api/1/power/restart", &json!({}))
}

/// The `power` event `told`'s data.
fn power(told: &str) -> Value {
    let data = told
        .strip_prefix("event: power\ndata: ")
        .and_then(|rest| rest.strip_suffix("\n\n"))
        .unwrap_or_else(|| panic!("not a power event: {told:?}"));
    serde_json::from_str(data).unwrap()
}

#[test]
fn a_restart_waits_for_a_trial_to_be_kept_or_undone_and_every_tab_is_told_what_is_set() {
    run(async {
        let world = world("restart-trial");
        let path = spark_path(&world, "restart-trial");
        let answers = [trying_network(), trying_nothing(), restart_set(60), trying_nothing()];
        let spark = fake_spark(path, answers.to_vec());
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let mut first = tab(&world, &chase).await;

        // While the network's settings are tried, nothing is run, not even
        // the read of the search.
        let refused = send(&world, restart().by(&chase)).await;
        assert_eq!(
            (refused.status, refused.error().as_str()),
            (409, "ADM-142"),
            "{}",
            refused.body
        );
        assert!(refused.body.contains("`network`"), "{}", refused.body);
        assert!(refused.body.contains("/networking"), "{}", refused.body);
        assert_eq!(elevated_commands(&world), Vec::<Value>::new());
        assert!(no_trial(&mut first).await, "nothing is set, so nothing is told");

        // With none, the search is read, the restart is set, and Spark is
        // read again at once, which every tab is told.
        let done = send(&world, restart().by(&chase)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(
            elevated_commands(&world),
            [
                json!(["changes", "bisect", "--status", "--json"]),
                json!(["shutdown", "-r", "+1", "--no-wall"]),
            ]
        );
        let told = power(&next_trial(&mut first).await);
        assert_eq!(told["shutdown"]["end"], "reboot", "{told}");
        let left = told["shutdown"]["in_seconds"].as_u64().unwrap();
        assert!((58..=60).contains(&left), "{told}");
        assert!(no_trial(&mut first).await, "told once");

        // A tab that opens while it is set is told on opening.
        let mut late = tab(&world, &chase).await;
        assert_eq!(power(&next_trial(&mut late).await)["shutdown"]["end"], "reboot");

        // A cancel is no restart, and is told too.
        let cancelled = send(&world, post("/api/1/power/cancel", &json!({})).by(&chase)).await;
        assert_eq!(cancelled.status, 200, "{}", cancelled.body);
        for next in [&mut first, &mut late] {
            assert_eq!(power(&next_trial(next).await), json!({ "shutdown": null }));
        }
        assert_eq!(spark.join().unwrap(), vec!["show\n"; 4]);

        // A Spark that does not answer refuses the restart: the guard fails
        // closed.
        std::fs::remove_file(path).unwrap();
        let silent = send(&world, restart().by(&chase)).await;
        assert_eq!((silent.status, silent.error().as_str()), (409, "ADM-142"), "{}", silent.body);
        assert!(silent.body.contains("cannot tell whether a trial runs"), "{}", silent.body);
        assert_eq!(elevated_commands(&world).len(), 3, "the cancel's, and nothing since");
    });
}

#[test]
fn a_restart_or_a_shut_down_waits_for_the_search_and_for_a_search_that_cannot_be_read() {
    run(async {
        let world = world("restart-search");
        let spark = fake_spark(spark_path(&world, "restart-search"), vec![trying_nothing(); 2]);
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;

        world.console.shared.search.set(&["lab-search"]);
        let shut_down = post("/api/1/power/shutdown", &json!({}));
        let refused = send(&world, shut_down.by(&chase)).await;
        assert_eq!(
            (refused.status, refused.error().as_str()),
            (409, "UPD-106"),
            "{}",
            refused.body
        );
        assert!(refused.body.contains("search for a bad change runs"), "{}", refused.body);

        // A read that fails refuses as well.
        world.console.shared.search.set(&["lab-search", "broken"]);
        let unread = send(&world, restart().by(&chase)).await;
        assert_eq!(unread.error(), "UPD-106", "{}", unread.body);
        assert!(unread.body.contains("cannot tell"), "{}", unread.body);
        assert_eq!(
            elevated_commands(&world),
            [json!(["lab-search"]), json!(["lab-search", "broken"])],
            "nothing was set"
        );
        assert_eq!(spark.join().unwrap(), vec!["show\n"; 2]);
    });
}

#[test]
fn the_search_waits_for_a_restart_that_is_set_and_for_a_trial() {
    run(async {
        let world = world("search-restart");
        let answers = [restart_set(42), trying_network(), trying_nothing()];
        let spark = fake_spark(spark_path(&world, "search-restart"), answers.to_vec());
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let search = || post("/api/1/updates/bisect", &json!({}));

        let refused = send(&world, search().by(&chase)).await;
        assert_eq!(
            (refused.status, refused.error().as_str()),
            (409, "UPD-106"),
            "{}",
            refused.body
        );
        assert!(refused.body.contains("in 42 seconds"), "{}", refused.body);
        let tried = send(&world, search().by(&chase)).await;
        assert_eq!(tried.error(), "ADM-142", "{}", tried.body);
        assert_eq!(elevated_commands(&world), Vec::<Value>::new(), "nothing ran");

        // With neither, the search is not read by admind: the command
        // refuses one that runs itself.
        let begun = send(&world, search().by(&chase)).await;
        assert_eq!(begun.status, 200, "{}", begun.body);
        assert_eq!(elevated_commands(&world), [json!(["changes", "bisect", "--yes", "--later"])]);
        assert_eq!(spark.join().unwrap(), vec!["show\n"; 3]);
    });
}

#[test]
fn only_one_who_may_restart_learns_what_runs() {
    run(async {
        // Spark answers no one here: a guard that asked it would wait for
        // it and say so.
        let world = world("restart-who");
        spark_path(&world, "restart-who");
        let dave = sign_in(&world, "dave").await;
        let refused = send(&world, restart().by(&dave)).await;
        assert_eq!(refused.error(), Kind::NotAdmin.id(), "{}", refused.body);
        let chase = sign_in(&world, "chase").await;
        let locked = send(&world, restart().by(&chase)).await;
        assert_eq!(locked.error(), Kind::Locked.id(), "{}", locked.body);
        assert_eq!(elevated_commands(&world), Vec::<Value>::new());
    });
}
