// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The tools by themselves, with the actions of `actions::tests` and an
//! authd that is asked nothing: what they are told of, their limits, the
//! arguments they take, and a request from its asking to where it ends.
//! `routes::tests::mcp` has them over HTTPS, with the reads and the
//! checks that need a worker.

use std::sync::Arc;

use accounts::console::Way;

use super::*;
use crate::admind::actions::tests::LAB;
use crate::admind::fixture::{Console as World, run};
use crate::admind::mcp::client::tests::client;
use crate::admind::mcp::client::{CALLS_AT_ONCE, begin};
use crate::admind::mcp::requests::{Outcome, State};

/// A console with the test actions, whose `ask` waits only a moment.
fn world(what: &str) -> World {
    let world = World::new(what, Arc::new(|_, _| vec!["no nothing is asked of authd here".into()]));
    world.shared.actions.set(LAB);
    world.shared.ask_wait.set(Duration::from_millis(20));
    world
}

/// chase's client that reaches everything, and his one that reads what
/// his account reads before it unlocks.
fn helper() -> Client {
    client("chase", "helper", Way::Https, "*")
}

fn reader() -> Client {
    client("chase", "reader", Way::Https, "*:read")
}

fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().expect("an object")
}

/// What `tool` answers `client` for `arguments`.
async fn called(world: &World, client: &Client, tool: &str, arguments: &Value) -> Value {
    call(&world.shared, client, tool, object(arguments)).await.expect("an answer")
}

/// The kind of the failure `tool` answers `client` with, and its words.
async fn refused(world: &World, client: &Client, tool: &str, arguments: &Value) -> (Kind, String) {
    let failure =
        call(&world.shared, client, tool, object(arguments)).await.expect_err("a failure");
    (failure.kind, failure.detail)
}

/// What asking to restart `name` gives `client`.
async fn ask_to_restart(world: &World, client: &Client, name: &str) -> Value {
    let arguments = json!({
        "action": "lab.restart",
        "arguments": { "name": name },
        "reason": "nginx stopped answering after its settings changed",
    });
    called(world, client, "ask", &arguments).await
}

#[test]
fn a_client_is_told_of_six_tools_whose_schemas_take_nothing_unnamed() {
    let names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
    assert_eq!(names, ["areas", "actions", "read", "check", "ask", "request"]);
    for tool in &TOOLS {
        assert!(!tool.title.is_empty() && !tool.description.is_empty(), "{}", tool.name);
        let schema = (tool.input)();
        assert_eq!(schema["type"], "object", "{}", tool.name);
        assert_eq!(schema["additionalProperties"], false, "{}", tool.name);
        let properties = schema["properties"].as_object().expect("properties");
        for required in schema["required"].as_array().into_iter().flatten() {
            let name = required.as_str().expect("a name");
            assert!(properties.contains_key(name), "{}: {name}", tool.name);
        }
        for (name, property) in properties {
            let said = property["description"].as_str().is_some_and(|text| !text.is_empty());
            assert!(said, "{}: {name} says nothing of itself", tool.name);
        }
        // Only `ask` is not a read, and it changes nothing either.
        assert_eq!(tool.reads, tool.name != "ask", "{}", tool.name);
    }
    // What `ask` says of its wait and of a reason is what the server does.
    let ask = TOOLS.iter().find(|tool| tool.name == "ask").expect("ask");
    assert!(ask.description.contains(&format!("{} seconds", ASK_WAIT.as_secs())));
    assert_eq!((ask.input)()["properties"]["reason"]["maxLength"], REASON_MAX);
}

#[test]
fn every_tool_a_client_is_told_of_is_one_the_server_has_and_no_other_is() {
    run(async {
        let world = world("tools-names");
        for tool in &TOOLS {
            // Whatever it answers to no arguments, it is the tool's own.
            let outcome = call(&world.shared, &helper(), tool.name, Map::new()).await;
            let kind = outcome.err().map(|failure| failure.kind);
            assert_ne!(kind, Some(Kind::Internal), "{}", tool.name);
        }
        let (kind, why) = refused(&world, &helper(), "shell", &json!({})).await;
        assert_eq!((kind, why.as_str()), (Kind::Internal, "the server has no such tool"));
    });
}

#[test]
fn arguments_and_answers_have_their_limits() {
    // `{"pad":"…"}` is ten bytes longer than its padding.
    let padded = |bytes: usize| json!({ "pad": "x".repeat(bytes - 10) });
    assert!(within(&object(&padded(ARGUMENTS_MAX))).is_ok());
    let long = within(&object(&padded(ARGUMENTS_MAX + 1))).expect_err("too long");
    assert_eq!(long.kind, Kind::TooBig);
    assert!(long.detail.contains("16384 bytes"), "{}", long.detail);
    assert_eq!(fitting(padded(ANSWER_MAX)).expect("it fits"), padded(ANSWER_MAX));
    let long = fitting(padded(ANSWER_MAX + 2048)).expect_err("too long");
    assert_eq!(long.kind, Kind::TooBig);
    assert!(
        long.detail.contains("258 KiB") && long.detail.contains("at most 256"),
        "{}",
        long.detail
    );
    assert!(long.detail.contains("narrow the read"), "{}", long.detail);
}

#[test]
fn arguments_that_are_too_long_are_refused_before_the_tool_reads_them() {
    run(async {
        let world = world("tools-long");
        let long = json!({ "area": "x".repeat(ARGUMENTS_MAX) });
        assert_eq!(refused(&world, &helper(), "actions", &long).await.0, Kind::TooBig);
        // A tool there is not is not looked for either.
        assert_eq!(refused(&world, &helper(), "shell", &long).await.0, Kind::TooBig);
    });
}

#[test]
fn a_tool_takes_the_arguments_it_names_and_nothing_else() {
    assert_eq!(list(&[]), "no arguments");
    assert_eq!(list(&["area"]), "area");
    assert_eq!(list(&["action", "arguments"]), "action and arguments");
    assert_eq!(list(&["action", "arguments", "reason"]), "action, arguments and reason");
    let given = object(&json!({ "action": "lab.list", "all": true }));
    assert!(only(&given, &["action", "all"]).is_ok());
    let more = only(&given, &["action", "arguments"]).expect_err("one too many");
    assert_eq!(more.kind, Kind::BadRequest);
    assert_eq!(more.detail, "the tool takes action and arguments and nothing else");
    // What it says does not repeat a name the client made up.
    assert!(!more.detail.contains("all\""), "{}", more.detail);

    let texts = object(&json!({ "area": "services", "empty": "", "number": 7 }));
    assert_eq!(text(&texts, "area").expect("text"), "services");
    for (name, why) in
        [("empty", "it is empty"), ("number", "it is not text"), ("gone", "it is not given")]
    {
        let bad = text(&texts, name).expect_err("no text");
        assert_eq!(bad.kind, Kind::BadValue, "{name}");
        assert!(bad.detail.contains(why), "{name}: {}", bad.detail);
    }
}

#[test]
fn an_action_is_named_by_its_id_with_arguments_that_are_an_object_or_left_out() {
    run(async {
        let world = world("tools-named");
        let shared = &world.shared;
        let of = |value: Value| named(shared, &object(&value));
        let (action, given) = of(json!({ "action": "lab.list" })).expect("an action");
        assert_eq!((action.id(), given), ("lab.list", json!({})));
        let (_, given) = of(json!({ "action": "lab.list", "arguments": null })).expect("an action");
        assert_eq!(given, json!({}));
        let (action, given) =
            of(json!({ "action": "lab.restart", "arguments": { "name": "nginx" } })).expect("one");
        assert_eq!((action.id(), given), ("lab.restart", json!({ "name": "nginx" })));
        for wrong in [
            json!({}),
            json!({ "action": 7 }),
            json!({ "action": "" }),
            json!({ "action": "lab.nothing" }),
            json!({ "action": "lab.list", "arguments": [] }),
            json!({ "action": "lab.list", "arguments": "name=nginx" }),
        ] {
            let bad = of(wrong.clone()).expect_err("not an action");
            assert_eq!(bad.kind, Kind::BadValue, "{wrong}");
        }
    });
}

#[test]
fn a_reason_is_one_line_of_at_most_two_hundred_characters() {
    let of = |text: &str| reason(&object(&json!({ "reason": text }))).map(str::to_string);
    assert_eq!(of("  nginx stopped answering \n").expect("a reason"), "nginx stopped answering");
    // Characters are counted, not bytes.
    let longest = "ä".repeat(REASON_MAX);
    assert_eq!(of(&longest).expect("a reason"), longest);
    for (wrong, why) in [
        (String::new(), "it is empty"),
        ("   ".into(), "it is empty"),
        ("ä".repeat(REASON_MAX + 1), "longer than 200 characters"),
        ("two\nlines".into(), "one line"),
        ("a\ttab".into(), "one line"),
        ("an escape \u{1b}[2J".into(), "one line"),
    ] {
        let bad = of(&wrong).expect_err("no reason");
        assert_eq!(bad.kind, Kind::BadValue, "{wrong:?}");
        assert!(bad.detail.contains(why), "{wrong:?}: {}", bad.detail);
    }
    let none = reason(&object(&json!({}))).expect_err("no reason");
    assert!(none.detail.contains("it is not given"), "{}", none.detail);
}

#[test]
fn a_call_is_counted_while_it_runs_and_no_longer() {
    run(async {
        let world = world("tools-counted");
        let now = Instant::now();
        let mut running: Vec<_> = (0..CALLS_AT_ONCE)
            .map(|_| begin(&world.shared, &helper(), now).expect("a place"))
            .collect();
        let (kind, why) = refused(&world, &helper(), "areas", &json!({})).await;
        assert_eq!(kind, Kind::Wait, "{why}");
        // Another client's calls are its own.
        called(&world, &reader(), "areas", &json!({})).await;
        running.pop();
        called(&world, &helper(), "areas", &json!({})).await;
        // The call that ended left its place: the next finds it again.
        called(&world, &helper(), "areas", &json!({})).await;
    });
}

#[test]
fn asking_files_one_request_for_one_change_and_tells_where_it_stands() {
    run(async {
        let world = world("tools-ask");
        let filed = ask_to_restart(&world, &helper(), "nginx").await;
        assert_eq!((&filed["state"], &filed["action"]), (&json!("waiting"), &json!("lab.restart")));
        let id = filed["request"].as_u64().expect("a number");
        assert!(filed["note"].as_str().unwrap().contains(&format!("request {id}")));
        assert!(filed["lapses"].as_i64().unwrap() > filed["asked"].as_i64().unwrap());
        // The same change again is the same request; another is another.
        assert_eq!(ask_to_restart(&world, &helper(), "nginx").await["request"], id);
        let other = ask_to_restart(&world, &helper(), "sshd").await;
        assert_ne!(other["request"], id);
        assert_eq!(world.shared.client_requests.borrow().waiting().count(), 2);
        // The history has each once, as waiting.
        let history = world.shared.client_history.borrow();
        let asked: Vec<&What> = history.newest(10).into_iter().map(|entry| &entry.what).collect();
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(
            asked.iter().all(|what| matches!(what, What::Ask { state, .. } if state == "waiting"))
        );
    });
}

#[test]
fn what_may_not_be_asked_for_files_nothing() {
    run(async {
        let world = world("tools-ask-refused");
        let restart = |more: Value| {
            let mut arguments =
                json!({ "action": "lab.restart", "arguments": { "name": "nginx" } });
            arguments.as_object_mut().unwrap().extend(object(&more));
            arguments
        };
        let reason = json!({ "reason": "it stopped answering" });
        // No reason, or one that is not a line of text.
        assert_eq!(refused(&world, &helper(), "ask", &restart(json!({}))).await.0, Kind::BadValue);
        let lines = json!({ "reason": "first\nsecond" });
        assert_eq!(refused(&world, &helper(), "ask", &restart(lines)).await.0, Kind::BadValue);
        // A read, which the tool read runs.
        let read = json!({ "action": "lab.list", "reason": "to see them" });
        let (kind, why) = refused(&world, &helper(), "ask", &read).await;
        assert_eq!(kind, Kind::ClientMayNot);
        assert!(why.contains("is a read"), "{why}");
        // A client that only reads.
        let (kind, _) = refused(&world, &reader(), "ask", &restart(reason.clone())).await;
        assert_eq!(kind, Kind::ClientMayNot);
        // Arguments the action does not take, refused as the API refuses
        // them: what waits must be able to run.
        let unnamed = json!({ "action": "lab.restart", "reason": "it stopped answering" });
        assert_eq!(refused(&world, &helper(), "ask", &unnamed).await.0, Kind::BadRequest);
        let dashed = json!({
            "action": "lab.restart", "arguments": { "name": "-rf" }, "reason": "it stopped",
        });
        assert_eq!(refused(&world, &helper(), "ask", &dashed).await.0, Kind::BadValue);
        // An argument of the tool's that there is not.
        let more = restart(json!({ "reason": "it stopped answering", "force": true }));
        assert_eq!(refused(&world, &helper(), "ask", &more).await.0, Kind::BadRequest);
        assert_eq!(world.shared.client_requests.borrow().waiting().count(), 0);
        assert!(world.shared.client_history.borrow().newest(10).is_empty());
    });
}

#[test]
fn an_answer_that_comes_while_a_client_waits_is_what_ask_tells() {
    run(async {
        let world = world("tools-ask-answered");
        // Longer than the test may take: only an answer ends the wait.
        world.shared.ask_wait.set(Duration::from_secs(30));
        let shared = Rc::clone(&world.shared);
        let refuser = tokio::task::spawn_local(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
                let waits = shared.client_requests.borrow().waiting().next().map(|r| r.id);
                if let Some(id) = waits {
                    let refused =
                        shared.client_requests.borrow_mut().refuse(id, "alice", Instant::now());
                    refused.expect("it waits");
                    crate::admind::mcp::moved(&shared, id);
                    return id;
                }
            }
        });
        let began = Instant::now();
        let answered = ask_to_restart(&world, &helper(), "nginx").await;
        assert!(began.elapsed() < Duration::from_secs(10));
        let id = refuser.await.expect("the task");
        assert_eq!(answered["request"], id);
        assert_eq!((&answered["state"], &answered["by"]), (&json!("refused"), &json!("alice")));
        assert!(answered["note"].as_str().unwrap().contains("Nothing was changed"));
        // A request that was answered is not the one a new asking joins.
        world.shared.ask_wait.set(Duration::from_millis(20));
        let again = ask_to_restart(&world, &helper(), "nginx").await;
        assert_eq!(again["state"], "waiting");
        assert_ne!(again["request"], id);
    });
}

#[test]
fn a_request_is_its_own_client_s_to_look_at() {
    run(async {
        let world = world("tools-request");
        let id = ask_to_restart(&world, &helper(), "nginx").await["request"].clone();
        let stands = called(&world, &helper(), "request", &json!({ "id": id })).await;
        assert_eq!((&stands["request"], &stands["state"]), (&id, &json!("waiting")));
        // Another client of the same person, and another person's client
        // of the same name, are told there is none.
        let namesake = client("alice", "helper", Way::Https, "*");
        for other in [reader(), namesake] {
            let (kind, why) = refused(&world, &other, "request", &json!({ "id": id })).await;
            assert_eq!(kind, Kind::NoRequest);
            assert_eq!(why, format!("this AI client has no request {id}"));
        }
        let (kind, _) = refused(&world, &helper(), "request", &json!({ "id": 999 })).await;
        assert_eq!(kind, Kind::NoRequest);
        for wrong in [json!({}), json!({ "id": 0 }), json!({ "id": "1" }), json!({ "id": -1 })] {
            let (kind, _) = refused(&world, &helper(), "request", &wrong).await;
            assert_eq!(kind, Kind::BadValue, "{wrong}");
        }
        let more = json!({ "id": id, "wait": true });
        assert_eq!(refused(&world, &helper(), "request", &more).await.0, Kind::BadRequest);
    });
}

#[test]
fn what_came_of_a_change_is_told_while_the_request_is_held_and_where_it_ended_after() {
    run(async {
        let world = world("tools-request-old");
        let shared = &world.shared;
        let id = ask_to_restart(&world, &helper(), "nginx").await["request"].as_u64().unwrap();
        let now = Instant::now();
        shared.client_requests.borrow_mut().approve(id, "alice", now).expect("it waits");
        crate::admind::mcp::moved(shared, id);
        let runs = called(&world, &helper(), "request", &json!({ "id": id })).await;
        assert_eq!((&runs["state"], &runs["by"]), (&json!("approved"), &json!("alice")));
        let answer = json!({ "restarted": "nginx" });
        shared.client_requests.borrow_mut().finish(id, Outcome::Done(answer.clone()), now);
        crate::admind::mcp::moved(shared, id);
        let done = called(&world, &helper(), "request", &json!({ "id": id })).await;
        assert_eq!((&done["state"], &done["answer"]), (&json!("done"), &answer));

        // An hour and more on, admind holds it no longer: the history
        // says where it ended, and not what the change answered.
        let later = now + Duration::from_hours(2);
        shared.client_requests.borrow_mut().lapse(later);
        assert!(shared.client_requests.borrow().get(id).is_none());
        let old = told(shared, &helper(), id).expect("the history has it");
        assert_eq!((&old["request"], &old["action"]), (&json!(id), &json!("lab.restart")));
        assert_eq!((&old["state"], &old["by"]), (&json!("done"), &json!("alice")));
        assert!(old.get("answer").is_none(), "{old}");
        assert!(old["note"].as_str().unwrap().contains("is not kept"), "{old}");
        assert!(old["ended"].as_i64().unwrap() >= old["asked"].as_i64().unwrap(), "{old}");
        // It is still its own client's alone.
        let theirs = told(shared, &reader(), id).expect_err("not the reader's");
        assert_eq!(theirs.kind, Kind::NoRequest);
    });
}

#[test]
fn a_run_of_reads_in_the_history_is_no_request() {
    let read = Entry {
        at: 1_800_000_000,
        owner: "chase".into(),
        client: "helper".into(),
        what: What::Read { since: 1_800_000_000, count: 3, areas: vec!["logs".into()] },
    };
    assert_eq!(told_of_old(&read), None);
    let asked = Entry {
        what: What::Ask {
            request: 4,
            action: "lab.restart".into(),
            title: "Restart a lab service".into(),
            state: State::Lapsed { why: "" }.word().into(),
            by: String::new(),
            task: None,
            ended: Some(1_800_003_600),
        },
        ..read
    };
    let told = told_of_old(&asked).expect("a request");
    assert_eq!(told["state"], "lapsed");
    assert_eq!(told["ended"], 1_800_003_600_i64);
    // Nobody answered it, and it ran as no task.
    assert!(told.get("by").is_none() && told.get("task").is_none(), "{told}");
}
