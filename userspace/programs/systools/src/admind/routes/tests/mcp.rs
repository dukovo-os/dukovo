// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MCP server at `/mcp` and the routes of the page for AI clients
//! (ADR 0690), with the actions of `actions::tests`: who is let in and
//! where, what the protocol answers over HTTPS, a read as the client's
//! owner and one as an administrator, and a change from the asking to
//! the yes or the no.

use std::time::Duration;

use super::*;
use crate::admind::actions::tests::LAB;

/// A world whose console offers the test actions, and whose `ask` waits
/// only a moment for an administrator.
fn lab_world(what: &str) -> World {
    let world = world(what);
    world.console.shared.actions.set(LAB);
    world.console.shared.ask_wait.set(Duration::from_millis(40));
    world
}

/// A message to `/mcp` as the client with `token` sends it: no cookie,
/// no origin.
fn mcp(token: &str, message: &Value) -> Req {
    post("/mcp", message)
        .with("origin", None)
        .with("authorization", Some(&format!("Bearer {token}")))
}

/// A request of the revisions that begin with `initialize`.
fn asks(id: u64, method: &str, params: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A call of `tool` with `arguments`, of those revisions.
fn calls(tool: &str, arguments: &Value) -> Value {
    asks(7, "tools/call", &json!({ "name": tool, "arguments": arguments }))
}

/// What the tool answered: the call's result, which must not be an error
/// of the protocol's.
async fn called_tool(world: &World, token: &str, tool: &str, arguments: &Value) -> Value {
    let answered = send(world, mcp(token, &calls(tool, arguments))).await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    let answer = answered.json();
    assert_eq!(answer["id"], 7, "{answer}");
    assert!(answer["result"].is_object(), "{answer}");
    answer["result"].clone()
}

/// What the tool answered as data, which must not be a failure.
async fn told(world: &World, token: &str, tool: &str, arguments: &Value) -> Value {
    let result = called_tool(world, token, tool, arguments).await;
    assert_eq!(result["isError"], false, "{result}");
    result["structuredContent"].clone()
}

/// The ID of the failure the tool answered with.
async fn refused(world: &World, token: &str, tool: &str, arguments: &Value) -> String {
    let result = called_tool(world, token, tool, arguments).await;
    assert_eq!(result["isError"], true, "{result}");
    result["structuredContent"]["error"]["id"].as_str().expect("an ID").to_string()
}

/// What authd was asked with `verb`, as its lines.
fn asked_of_authd(world: &World, verb: &str) -> Vec<String> {
    let start = format!("{verb} ");
    let seen = world.console.authd.seen.lock().unwrap();
    seen.iter().filter(|line| line.starts_with(&start) || **line == verb).cloned().collect()
}

/// Signs alice in, another administrator, from the browser she trusts,
/// which needs no second step.
async fn sign_in_alice(world: &World) -> Session {
    let body = json!({ "name": "alice", "password": "right" });
    let trusted =
        post("/api/1/sign-in", &body).with("cookie", Some("__Host-dukovo-trust=aa55bb66"));
    let answered = send(world, trusted).await;
    assert_eq!(answered.json(), json!({ "done": true }));
    let cookie = answered.cookie(SESSION_COOKIE).expect("a session cookie");
    let header = format!("{SESSION_COOKIE}={cookie}");
    let shown = send(world, get("/api/1/session").with("cookie", Some(&header))).await;
    Session { csrf: shown.json()["csrf"].as_str().unwrap().into(), cookie }
}

async fn unlock(world: &World, session: &Session) {
    let body = json!({ "kind": "password", "password": "right" });
    let done = send(world, post("/api/1/unlock", &body).by(session)).await;
    assert_eq!(done.status, 200, "{}", done.body);
}

/// Files a request for `lab.restart` of `name` as the client with
/// `token`, and gives its number.
async fn ask_to_restart(world: &World, token: &str, name: &str) -> u64 {
    let arguments = json!({
        "action": "lab.restart",
        "arguments": { "name": name },
        "reason": "nginx stopped answering after its settings changed",
    });
    let filed = told(world, token, "ask", &arguments).await;
    assert_eq!(filed["state"], "waiting", "{filed}");
    filed["request"].as_u64().expect("a number")
}

/// The requests that wait, as an administrator's page lists them.
async fn waiting(world: &World, session: &Session) -> Vec<Value> {
    let listed = send(world, get("/api/1/clients/requests").by(session)).await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    listed.json()["requests"].as_array().expect("a list").clone()
}

#[test]
fn the_mcp_server_takes_a_client_s_token_and_nothing_else_and_a_client_s_token_goes_nowhere_else() {
    run(async {
        let world = lab_world("mcp-doors");
        let ping = asks(1, "ping", &json!({}));
        let pong = send(&world, mcp(CLIENT_ALL, &ping)).await;
        assert_eq!(pong.status, 200, "{}", pong.body);
        assert_eq!(pong.json(), json!({ "jsonrpc": "2.0", "id": 1, "result": {} }));
        assert_eq!(pong.header("content-type"), Some("application/json"));
        assert_eq!(pong.header("cache-control"), Some("no-store"));
        // authd is asked about the token, with where it came from, each time.
        send(&world, mcp(CLIENT_ALL, &ping)).await;
        let hex = accounts::request::hex(CLIENT_ALL.as_bytes());
        let line = format!("client {hex} {PEER}");
        assert_eq!(asked_of_authd(&world, "client"), [line.clone(), line]);

        // No token, a session, an API token: each is told where it belongs.
        let nobody = send(&world, post("/mcp", &ping).with("origin", None)).await;
        assert_eq!((nobody.status, nobody.error().as_str()), (401, "ADM-143"));
        assert!(nobody.json()["error"]["detail"].as_str().unwrap().contains("dkc_"));
        let session = sign_in(&world, "chase").await;
        let by_session = send(&world, post("/mcp", &ping).by(&session)).await;
        assert_eq!((by_session.status, by_session.error().as_str()), (401, "ADM-143"));
        let by_api = send(&world, mcp(TOKEN_ALL, &ping)).await;
        assert_eq!((by_api.status, by_api.error().as_str()), (401, "ADM-143"));
        assert!(by_api.json()["error"]["detail"].as_str().unwrap().contains("for /mcp"));
        // The API token was not shown to authd as a client's.
        assert_eq!(asked_of_authd(&world, "client").len(), 2);
        assert!(asked_of_authd(&world, "token").is_empty());

        // A client's token is no API token: not for an action, the list
        // of them, the description that anyone may read, or a page.
        for path in ["/api/1/lab", "/api/1/actions", "/api/1/openapi.json", "/"] {
            let req = get(path).with("authorization", Some(&format!("Bearer {CLIENT_ALL}")));
            let elsewhere = send(&world, req).await;
            assert_eq!((elsewhere.status, elsewhere.error().as_str()), (403, "ADM-144"), "{path}");
        }
        assert_eq!(asked_of_authd(&world, "client").len(), 2);

        // A token authd does not know, and one of a client that connects
        // over SSH, whose pass is not a bearer at all.
        let unknown = "dkc_chase_0000000000000000000000000000000000000000000000000000000000000";
        let wrong = send(&world, mcp(unknown, &ping)).await;
        assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-143"));
        let pass = send(&world, mcp(CLIENT_PASS, &ping)).await;
        assert_eq!((pass.status, pass.error().as_str()), (400, "ADM-108"));
        // A token and a session together are neither.
        let both = send(&world, mcp(CLIENT_ALL, &ping).by(&session)).await;
        assert_eq!((both.status, both.error().as_str()), (400, "ADM-108"));
    });
}

#[test]
fn the_mcp_server_answers_a_post_of_json_from_no_other_site() {
    run(async {
        let world = lab_world("mcp-http");
        let ping = asks(1, "ping", &json!({}));
        // GET would open a stream, which this server has none of.
        let got =
            send(&world, get("/mcp").with("authorization", Some(&format!("Bearer {CLIENT_ALL}"))))
                .await;
        assert_eq!((got.status, got.error().as_str()), (405, "ADM-117"));
        // A page of another site that holds a token is not let in; the
        // console's own origin, and a client that names none, are.
        let foreign =
            send(&world, mcp(CLIENT_ALL, &ping).with("origin", Some("https://evil.example"))).await;
        assert_eq!((foreign.status, foreign.error().as_str()), (403, "ADM-109"));
        let own = send(&world, mcp(CLIENT_ALL, &ping).with("origin", Some(ORIGIN))).await;
        assert_eq!(own.status, 200, "{}", own.body);
        // Another machine's name is not this one's.
        let other = send(&world, mcp(CLIENT_ALL, &ping).with("host", Some("evil.example"))).await;
        assert_eq!((other.status, other.error().as_str()), (421, "ADM-114"));
        // The body is JSON and at most 64 KiB, refused before it is read.
        let plain =
            send(&world, mcp(CLIENT_ALL, &ping).with("content-type", Some("text/plain"))).await;
        assert_eq!((plain.status, plain.error().as_str()), (415, "ADM-119"));
        let long = asks(1, "ping", &json!({ "pad": "x".repeat(70 * 1024) }));
        let big = send(&world, mcp(CLIENT_ALL, &long)).await;
        assert_eq!((big.status, big.error().as_str()), (413, "ADM-113"));
        // A header that comes twice is not one to choose from.
        let mut twice = mcp(CLIENT_ALL, &ping);
        twice.headers.push(("mcp-protocol-version".into(), "2025-06-18".into()));
        twice.headers.push(("mcp-protocol-version".into(), "2025-11-25".into()));
        let twice = send(&world, twice).await;
        assert_eq!((twice.status, twice.error().as_str()), (400, "ADM-108"));
    });
}

#[test]
fn ten_wrong_tokens_from_an_address_make_it_wait_and_a_right_one_from_another_does_not() {
    run(async {
        let world = lab_world("mcp-wrong");
        let ping = asks(1, "ping", &json!({}));
        let unknown = "dkc_chase_0000000000000000000000000000000000000000000000000000000000000";
        for _ in 0..10 {
            let wrong = send(&world, mcp(unknown, &ping)).await;
            assert_eq!((wrong.status, wrong.error().as_str()), (401, "ADM-143"));
        }
        // The right token waits too: the address is what is held back.
        let held = send(&world, mcp(CLIENT_ALL, &ping)).await;
        assert_eq!((held.status, held.error().as_str()), (429, "ADM-105"));
        assert!(held.header("retry-after").is_some());
        assert_eq!(asked_of_authd(&world, "client").len(), 10);
        let elsewhere = send_from(&world, "192.0.2.99", mcp(CLIENT_ALL, &ping)).await;
        assert_eq!(elsewhere.status, 200, "{}", elsewhere.body);
    });
}

#[test]
fn the_protocol_s_answers_come_with_their_statuses_in_both_eras() {
    run(async {
        let world = lab_world("mcp-protocol");
        // The revisions that begin with initialize.
        let hello = asks(1, "initialize", &json!({ "protocolVersion": "2025-06-18" }));
        let greeted = send(&world, mcp(CLIENT_ALL, &hello)).await.json();
        assert_eq!(greeted["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(greeted["result"]["serverInfo"]["name"], "dukovo-admind");
        let ready = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let taken = send(&world, mcp(CLIENT_ALL, &ready)).await;
        assert_eq!((taken.status, taken.body.as_str()), (202, ""));
        assert_eq!(taken.header("content-type"), None);
        let list = send(&world, mcp(CLIENT_ALL, &asks(2, "tools/list", &json!({})))).await.json();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["areas", "actions", "read", "check", "ask", "request"]);
        // Their errors are answers, 200, as their clients expect.
        let none = send(&world, mcp(CLIENT_ALL, &asks(3, "resources/list", &json!({})))).await;
        assert_eq!(none.status, 200);
        assert_eq!(none.json()["error"]["code"], -32601);
        // A batch, which the two oldest have: answers for its requests.
        let batch = json!([asks(1, "ping", &json!({})), ready, asks(2, "nothing", &json!({}))]);
        let both = send(&world, mcp(CLIENT_ALL, &batch)).await;
        assert_eq!(both.status, 200);
        let answers = both.json();
        assert_eq!(answers.as_array().unwrap().len(), 2, "{answers}");
        assert_eq!((&answers[0]["id"], &answers[1]["error"]["code"]), (&json!(1), &json!(-32601)));
        let newer =
            send(&world, mcp(CLIENT_ALL, &batch).with("mcp-protocol-version", Some("2025-06-18")))
                .await;
        assert_eq!((newer.status, &newer.json()["error"]["code"]), (400, &json!(-32600)));

        // The revision without a handshake: the envelope and the headers.
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
        });
        let stateless = |method: &str, params: Value| {
            let mut params = params;
            params["_meta"] = meta.clone();
            mcp(CLIENT_ALL, &asks(4, method, &params))
                .with("mcp-protocol-version", Some("2026-07-28"))
                .with("mcp-method", Some(method))
        };
        let found = send(&world, stateless("server/discover", json!({}))).await;
        assert_eq!(found.status, 200, "{}", found.body);
        let found = found.json();
        assert_eq!(found["result"]["resultType"], "complete");
        assert_eq!(found["result"]["supportedVersions"][0], "2026-07-28");
        let areas = stateless("tools/call", json!({ "name": "areas", "arguments": {} }))
            .with("mcp-name", Some("areas"));
        let areas = send(&world, areas).await;
        assert_eq!(areas.status, 200, "{}", areas.body);
        assert_eq!(areas.json()["result"]["resultType"], "complete");
        assert_eq!(areas.json()["result"]["isError"], false);
        // No such method is 404 there, and headers that disagree 400.
        let none = send(&world, stateless("resources/list", json!({}))).await;
        assert_eq!((none.status, &none.json()["error"]["code"]), (404, &json!(-32601)));
        let lied =
            stateless("tools/call", json!({ "name": "areas" })).with("mcp-name", Some("ask"));
        let lied = send(&world, lied).await;
        assert_eq!((lied.status, &lied.json()["error"]["code"]), (400, &json!(-32020)));
        let old =
            stateless("tools/list", json!({})).with("mcp-protocol-version", Some("2025-06-18"));
        let old = send(&world, old).await;
        assert_eq!((old.status, &old.json()["error"]["code"]), (400, &json!(-32020)));

        // What is not JSON-RPC at all.
        let mut broken = mcp(CLIENT_ALL, &json!({}));
        broken.body = b"{\"jsonrpc\":".to_vec();
        let broken = send(&world, broken).await;
        assert_eq!((broken.status, &broken.json()["error"]["code"]), (400, &json!(-32700)));
        assert_eq!(broken.json()["id"], Value::Null);
    });
}

#[test]
fn a_client_finds_what_it_reaches_and_no_more() {
    run(async {
        let world = lab_world("mcp-reach");
        // All of it: both areas of the test actions, with their changes.
        let all = told(&world, CLIENT_ALL, "areas", &json!({})).await;
        assert_eq!(
            all["areas"],
            json!([
                { "area": "services", "title": "Services", "reach": "read all and ask",
                  "reads": 2, "changes": 3 },
                { "area": "logs", "title": "Logs", "reach": "read all", "reads": 1, "changes": 0 },
            ])
        );
        // What its owner's account reads before it unlocks: no log, and
        // no change.
        let some = told(&world, CLIENT_READ, "areas", &json!({})).await;
        assert_eq!(
            some["areas"],
            json!([{ "area": "services", "title": "Services", "reach": "read", "reads": 2,
                     "changes": 0 }])
        );
        let listed = told(&world, CLIENT_ALL, "actions", &json!({ "area": "services" })).await;
        let ids: Vec<(&str, &str, &str)> = listed["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|action| {
                let text = |key: &str| action[key].as_str().unwrap();
                (text("id"), text("kind"), text("as"))
            })
            .collect();
        assert_eq!(
            ids,
            [
                ("lab.restart", "change", "administrator"),
                ("lab.list", "read", "owner"),
                ("lab.long", "change", "administrator"),
                ("lab.set", "change", "administrator"),
                ("lab.missing", "read", "owner"),
            ]
        );
        assert_eq!(listed["actions"][0]["arguments"]["required"], json!(["name"]));
        let fewer = told(&world, CLIENT_READ, "actions", &json!({ "area": "services" })).await;
        let ids: Vec<&str> = fewer["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["lab.list", "lab.missing"]);
        // An area it reaches and has nothing to do in, one it does not
        // reach, one there is not, and an argument too many.
        let logs = json!({ "area": "logs" });
        let nothing = told(&world, CLIENT_READ, "actions", &logs).await;
        assert_eq!(nothing["actions"], json!([]), "{nothing}");
        assert_eq!(refused(&world, CLIENT_NARROWED, "actions", &logs).await, "ADM-144");
        assert_eq!(
            refused(&world, CLIENT_ALL, "actions", &json!({ "area": "x" })).await,
            "ADM-125"
        );
        assert_eq!(refused(&world, CLIENT_ALL, "areas", &json!({ "all": true })).await, "ADM-108");
        // A tool there is not is the protocol's to refuse, and runs nothing.
        let none = send(&world, mcp(CLIENT_ALL, &calls("shell", &json!({})))).await.json();
        assert_eq!(none["error"]["code"], -32602, "{none}");
    });
}

#[test]
fn a_read_runs_as_the_client_s_owner_or_through_authd_as_an_administrator() {
    run(async {
        let world = lab_world("mcp-read");
        // As the owner, in a worker authd starts for the token.
        let read = told(&world, CLIENT_READ, "read", &json!({ "action": "lab.list" })).await;
        assert_eq!(read, json!({ "action": "lab.list", "answer": [{ "name": "nginx" }] }));
        let argv = json!({ "argv": ["service", "list", "--json"] });
        assert!(called(&world).contains(&("run".into(), argv)), "{:?}", called(&world));
        let hex = accounts::request::hex(CLIENT_READ.as_bytes());
        assert_eq!(asked_of_authd(&world, "client-start"), [format!("client-start {hex}")]);
        // The worker is kept for the next read.
        told(&world, CLIENT_READ, "read", &json!({ "action": "lab.list", "arguments": {} })).await;
        assert_eq!(asked_of_authd(&world, "client-start").len(), 1);
        assert!(asked_of_authd(&world, "elevate").is_empty());

        // As an administrator: admind names the action, not a command, and
        // authd's fixed program decides.
        let lines = json!({ "action": "lab.log", "arguments": { "lines": 5 } });
        let log = told(&world, CLIENT_ALL, "read", &lines).await;
        assert_eq!(log, json!({ "action": "lab.log", "answer": [{ "n": 1 }, { "n": 2 }] }));
        let noted = ("client-read".to_string(), json!(["lab.log", { "lines": 5 }]));
        assert!(called(&world).contains(&noted), "{:?}", called(&world));
        assert!(asked_of_authd(&world, "elevate").is_empty());
        // A client that reads only as its owner is refused before authd
        // is asked to run anything.
        assert_eq!(refused(&world, CLIENT_READ, "read", &lines).await, "ADM-144");
        assert_eq!(asked_of_authd(&world, "client-read").len(), 1);

        // A change is not read, an action there is not is no action, and
        // arguments the action does not take are refused as the API's are.
        let change = json!({ "action": "lab.restart", "arguments": { "name": "nginx" } });
        assert_eq!(refused(&world, CLIENT_ALL, "read", &change).await, "ADM-144");
        let none = json!({ "action": "lab.nothing" });
        assert_eq!(refused(&world, CLIENT_ALL, "read", &none).await, "ADM-125");
        let bad = json!({ "action": "lab.log", "arguments": { "lines": 0 } });
        assert_eq!(refused(&world, CLIENT_ALL, "read", &bad).await, "ADM-125");
        assert_eq!(asked_of_authd(&world, "client-read").len(), 1);
        // A command that fails is the tool's failure, with what it said.
        let missing =
            called_tool(&world, CLIENT_ALL, "read", &json!({ "action": "lab.missing" })).await;
        assert_eq!(missing["isError"], true);
        assert_eq!(missing["structuredContent"]["error"]["id"], "ADM-129");

        // What they read is in the history, joined into one run each.
        let session = sign_in(&world, "chase").await;
        let history = send(&world, get("/api/1/clients/history").by(&session)).await.json();
        let runs: Vec<(&str, u64, &Value)> = history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["client"].as_str().unwrap(),
                    entry["count"].as_u64().unwrap(),
                    &entry["areas"],
                )
            })
            .collect();
        assert!(runs.contains(&("reader", 2, &json!(["services"]))), "{history}");
        assert!(runs.contains(&("helper", 1, &json!(["logs"]))), "{history}");
    });
}

#[test]
fn a_check_says_whether_a_change_would_change_anything_and_changes_nothing() {
    run(async {
        let world = lab_world("mcp-check");
        let set = json!({ "action": "lab.set", "arguments": { "name": "moving" } });
        let checked = told(&world, CLIENT_ALL, "check", &set).await;
        assert_eq!(checked, json!({ "action": "lab.set", "changes": true, "now": "off" }));
        // Only the probe ran, as the owner.
        let ran: Vec<Value> = called(&world)
            .into_iter()
            .filter(|(method, _)| method == "run")
            .map(|(_, params)| params["argv"].clone())
            .collect();
        assert_eq!(ran, [json!(["probe", "moving"])]);
        assert!(asked_of_authd(&world, "elevate").is_empty());
        // A change that compares nothing always does something; a read
        // changes nothing.
        let restart = json!({ "action": "lab.restart", "arguments": { "name": "nginx" } });
        assert_eq!(told(&world, CLIENT_ALL, "check", &restart).await["changes"], true);
        let list = json!({ "action": "lab.list" });
        assert_eq!(told(&world, CLIENT_ALL, "check", &list).await["changes"], false);
        // What it may not ask for it may not check.
        assert_eq!(refused(&world, CLIENT_READ, "check", &set).await, "ADM-144");
    });
}

#[test]
fn a_change_waits_for_a_yes_and_runs_as_the_administrator_who_gave_it() {
    run(async {
        let world = lab_world("mcp-approve");
        let id = ask_to_restart(&world, CLIENT_ALL, "nginx").await;
        // Nothing ran, and asking again is the same request.
        assert!(asked_of_authd(&world, "elevate").is_empty());
        assert_eq!(ask_to_restart(&world, CLIENT_ALL, "nginx").await, id);
        let stands = told(&world, CLIENT_ALL, "request", &json!({ "id": id })).await;
        assert_eq!(
            (&stands["state"], &stands["action"]),
            (&json!("waiting"), &json!("lab.restart"))
        );

        // An administrator sees it, locked or not, with the command a yes runs.
        let chase = sign_in(&world, "chase").await;
        let cards = waiting(&world, &chase).await;
        assert_eq!(cards.len(), 1, "{cards:?}");
        let card = &cards[0];
        assert_eq!(card["request"], id);
        assert_eq!((&card["owner"], &card["client"]), (&json!("chase"), &json!("helper")));
        assert_eq!(
            (&card["title"], &card["area"]),
            (&json!("Restart a lab service: nginx"), &json!("services"))
        );
        assert_eq!(card["args"], json!({ "name": "nginx" }));
        assert_eq!(card["reason"], "nginx stopped answering after its settings changed");
        assert_eq!(card["command"], "elevate service nginx restart");
        assert!(card["lapses"].as_i64().unwrap() > card["asked"].as_i64().unwrap());

        // A yes needs an unlocked session of an administrator.
        let approve = format!("/api/1/clients/requests/{id}/approve");
        let locked = send(&world, post(&approve, &json!({})).by(&chase)).await;
        assert_eq!((locked.status, locked.error().as_str()), (403, "ADM-106"));
        let dave = sign_in(&world, "dave").await;
        let not_admin = send(&world, post(&approve, &json!({})).by(&dave)).await;
        assert_eq!((not_admin.status, not_admin.error().as_str()), (403, "ADM-122"));
        let peeked = send(&world, get("/api/1/clients/requests").by(&dave)).await;
        assert_eq!(peeked.error(), "ADM-122");
        // No token says yes: not an API token's, and not the client's own.
        let by_api = post(&approve, &json!({}))
            .with("origin", None)
            .with("authorization", Some(&format!("Bearer {TOKEN_ALL}")));
        assert_eq!(send(&world, by_api).await.error(), "ADM-124");
        let by_itself = post(&approve, &json!({}))
            .with("origin", None)
            .with("authorization", Some(&format!("Bearer {CLIENT_ALL}")));
        assert_eq!(send(&world, by_itself).await.error(), "ADM-144");
        assert!(asked_of_authd(&world, "elevate").is_empty());
        assert_eq!(waiting(&world, &chase).await.len(), 1);

        // Alice, another administrator, unlocks and says yes.
        let alice = sign_in_alice(&world).await;
        unlock(&world, &alice).await;
        let done = send(&world, post(&approve, &json!({})).by(&alice)).await;
        assert_eq!(done.status, 200, "{}", done.body);
        assert_eq!(done.json()["done"], true);
        // It ran as alice's session, and authd was asked whether the
        // client was still there first: by each of the two lists above,
        // and by the yes.
        let elevated = asked_of_authd(&world, "elevate");
        assert_eq!(elevated.len(), 1, "{elevated:?}");
        assert!(elevated[0].ends_with("-- service nginx restart"), "{elevated:?}");
        assert_eq!(asked_of_authd(&world, "client-of"), ["client-of chase helper"; 3]);
        // The client hears what came of it, and the page no longer shows it.
        let stands = told(&world, CLIENT_ALL, "request", &json!({ "id": id })).await;
        assert_eq!((&stands["state"], &stands["by"]), (&json!("done"), &json!("alice")));
        assert_eq!(stands["answer"]["done"], true);
        assert!(waiting(&world, &chase).await.is_empty());
        // A second yes has nothing to answer.
        let again = send(&world, post(&approve, &json!({})).by(&alice)).await;
        assert_eq!((again.status, again.error().as_str()), (404, "ADM-145"));
        assert!(again.json()["error"]["detail"].as_str().unwrap().contains("by alice"));
        assert_eq!(asked_of_authd(&world, "elevate").len(), 1);

        // The journal says who made the change and whose client asked.
        let changes = send(&world, get("/api/1/changes").by(&alice)).await.json();
        let row = &changes["changes"][0];
        assert_eq!((&row["who"], &row["how"]), (&json!("alice"), &json!("client helper of chase")));
        // And the history says the request and its end.
        let history = send(&world, get("/api/1/clients/history").by(&chase)).await.json();
        let entry = &history["entries"][0];
        assert_eq!((&entry["kind"], &entry["request"]), (&json!("ask"), &json!(id)));
        assert_eq!((&entry["state"], &entry["by"]), (&json!("done"), &json!("alice")));
        assert_eq!(entry["title"], "Restart a lab service: nginx");
    });
}

#[test]
fn a_no_runs_nothing_and_the_client_is_told_who_refused() {
    run(async {
        let world = lab_world("mcp-refuse");
        let id = ask_to_restart(&world, CLIENT_ALL, "nginx").await;
        // A locked session of an administrator is enough to say no.
        let chase = sign_in(&world, "chase").await;
        let refuse = format!("/api/1/clients/requests/{id}/refuse");
        let dave = sign_in(&world, "dave").await;
        assert_eq!(send(&world, post(&refuse, &json!({})).by(&dave)).await.error(), "ADM-122");
        let done = send(&world, post(&refuse, &json!({})).by(&chase)).await;
        assert_eq!((done.status, done.json()), (200, json!({ "done": true })));
        assert!(asked_of_authd(&world, "elevate").is_empty());
        let stands = told(&world, CLIENT_ALL, "request", &json!({ "id": id })).await;
        assert_eq!((&stands["state"], &stands["by"]), (&json!("refused"), &json!("chase")));
        // It cannot be approved after that, and another client's request
        // is none of this one's to look at.
        unlock(&world, &chase).await;
        let approve = format!("/api/1/clients/requests/{id}/approve");
        assert_eq!(send(&world, post(&approve, &json!({})).by(&chase)).await.error(), "ADM-145");
        assert_eq!(refused(&world, CLIENT_READ, "request", &json!({ "id": id })).await, "ADM-145");
        // A number that is none.
        for none in ["0", "x", "99"] {
            let path = format!("/api/1/clients/requests/{none}/refuse");
            let answered = send(&world, post(&path, &json!({})).by(&chase)).await;
            assert_eq!((answered.status, answered.error().as_str()), (404, "ADM-145"), "{none}");
        }
    });
}

#[test]
fn a_yes_for_a_client_that_is_gone_or_reaches_less_runs_nothing_and_its_requests_lapse() {
    run(async {
        let world = lab_world("mcp-gone");
        let gone = ask_to_restart(&world, CLIENT_GONE, "nginx").await;
        let also_gone = ask_to_restart(&world, CLIENT_GONE, "postfix").await;
        let narrowed = ask_to_restart(&world, CLIENT_NARROWED, "nginx").await;
        let kept = ask_to_restart(&world, CLIENT_ALL, "nginx").await;
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        for id in [gone, narrowed] {
            let approve = format!("/api/1/clients/requests/{id}/approve");
            let answered = send(&world, post(&approve, &json!({})).by(&chase)).await;
            assert_eq!((answered.status, answered.error().as_str()), (404, "ADM-145"), "{id}");
            let why = answered.json()["error"]["detail"].as_str().unwrap().to_string();
            assert!(why.contains("nothing was changed"), "{why}");
        }
        assert!(asked_of_authd(&world, "elevate").is_empty());
        // Every request of the client that is gone has lapsed, and the
        // other client's still waits.
        let left: Vec<u64> = waiting(&world, &chase)
            .await
            .iter()
            .map(|card| card["request"].as_u64().unwrap())
            .collect();
        assert_eq!(left, [kept]);
        let history = send(&world, get("/api/1/clients/history").by(&chase)).await.json();
        let states: Vec<(u64, &str)> = history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry["request"].as_u64().unwrap(), entry["state"].as_str().unwrap()))
            .collect();
        for id in [gone, also_gone, narrowed] {
            assert!(states.contains(&(id, "lapsed")), "{id}: {states:?}");
        }
        assert!(states.contains(&(kept, "waiting")), "{states:?}");
    });
}

#[test]
fn the_list_of_what_waits_drops_the_requests_of_a_client_that_is_gone_or_reaches_less() {
    run(async {
        let world = lab_world("mcp-gone-listed");
        let gone = ask_to_restart(&world, CLIENT_GONE, "nginx").await;
        let also_gone = ask_to_restart(&world, CLIENT_GONE, "postfix").await;
        let narrowed = ask_to_restart(&world, CLIENT_NARROWED, "nginx").await;
        let kept = ask_to_restart(&world, CLIENT_ALL, "nginx").await;
        // A locked administrator opens the page: nobody said yes or no,
        // and the cards are of the client that still stands alone.
        let chase = sign_in(&world, "chase").await;
        let left: Vec<u64> = waiting(&world, &chase)
            .await
            .iter()
            .map(|card| card["request"].as_u64().unwrap())
            .collect();
        assert_eq!(left, [kept]);
        // authd was asked once for each client, not for each request.
        assert_eq!(asked_of_authd(&world, "client-of").len(), 3);
        assert!(asked_of_authd(&world, "elevate").is_empty());
        let history = send(&world, get("/api/1/clients/history").by(&chase)).await.json();
        let states: Vec<(u64, &str)> = history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry["request"].as_u64().unwrap(), entry["state"].as_str().unwrap()))
            .collect();
        for id in [gone, also_gone, narrowed] {
            assert!(states.contains(&(id, "lapsed")), "{id}: {states:?}");
        }
        assert!(states.contains(&(kept, "waiting")), "{states:?}");
        // A yes for one of them finds a request that no longer waits.
        unlock(&world, &chase).await;
        let approve = format!("/api/1/clients/requests/{gone}/approve");
        let answered = send(&world, post(&approve, &json!({})).by(&chase)).await;
        assert_eq!((answered.status, answered.error().as_str()), (404, "ADM-145"));
    });
}

#[test]
fn a_change_that_is_refused_or_fails_after_the_yes_is_told_to_both() {
    run(async {
        let world = lab_world("mcp-fails");
        let id = ask_to_restart(&world, CLIENT_ALL, "broken").await;
        let chase = sign_in(&world, "chase").await;
        unlock(&world, &chase).await;
        let approve = format!("/api/1/clients/requests/{id}/approve");
        let failed = send(&world, post(&approve, &json!({})).by(&chase)).await;
        assert_eq!((failed.status, failed.error().as_str()), (422, "SVC-104"));
        let stands = told(&world, CLIENT_ALL, "request", &json!({ "id": id })).await;
        assert_eq!(
            (&stands["state"], &stands["error"]["id"]),
            (&json!("failed"), &json!("SVC-104"))
        );
        // A long change is a task, which the client is told of.
        let long =
            json!({ "action": "lab.long", "arguments": { "seconds": 1 }, "reason": "a test" });
        let filed = told(&world, CLIENT_ALL, "ask", &long).await;
        let id = filed["request"].as_u64().unwrap();
        let approve = format!("/api/1/clients/requests/{id}/approve");
        let begun = send(&world, post(&approve, &json!({})).by(&chase)).await;
        assert_eq!(begun.status, 202, "{}", begun.body);
        let task = begun.json()["task"].as_i64().expect("a task");
        let stands = told(&world, CLIENT_ALL, "request", &json!({ "id": id })).await;
        assert_eq!(stands["state"], "task", "{stands}");
        assert_eq!(stands["task"]["id"], task, "{stands}");
    });
}

#[test]
fn what_no_client_may_ask_for_is_refused_and_its_requests_are_counted() {
    run(async {
        let world = lab_world("mcp-limits");
        let ask = |action: &str, arguments: Value, reason: &str| json!({ "action": action, "arguments": arguments, "reason": reason });
        // A read is not asked for, a client that only reads asks for
        // nothing, and a reason is one line that says something.
        let list = ask("lab.list", json!({}), "to see");
        assert_eq!(refused(&world, CLIENT_ALL, "ask", &list).await, "ADM-144");
        let restart = ask("lab.restart", json!({ "name": "nginx" }), "to see");
        assert_eq!(refused(&world, CLIENT_READ, "ask", &restart).await, "ADM-144");
        for reason in ["", "  ", "two\nlines", &"x".repeat(201)] {
            let bad = ask("lab.restart", json!({ "name": "nginx" }), reason);
            assert_eq!(refused(&world, CLIENT_ALL, "ask", &bad).await, "ADM-125", "{reason:?}");
        }
        // Arguments are checked when it is filed, as the API checks them.
        let bad = ask("lab.restart", json!({ "name": "-rf" }), "to see");
        assert_eq!(refused(&world, CLIENT_ALL, "ask", &bad).await, "ADM-125");
        let chase = sign_in(&world, "chase").await;
        assert!(waiting(&world, &chase).await.is_empty());
        // Eight of one client's wait, and the ninth is refused.
        for n in 0..8 {
            ask_to_restart(&world, CLIENT_ALL, &format!("service-{n}")).await;
        }
        let ninth = ask("lab.restart", json!({ "name": "service-8" }), "to see");
        assert_eq!(refused(&world, CLIENT_ALL, "ask", &ninth).await, "ADM-146");
        assert_eq!(waiting(&world, &chase).await.len(), 8);
    });
}

#[test]
fn an_administrator_s_tabs_are_told_how_many_requests_wait() {
    run(async {
        let world = lab_world("mcp-events");
        let chase = sign_in(&world, "chase").await;
        let dave = sign_in(&world, "dave").await;
        let mut chase_tab = tab(&world, &chase).await;
        let mut dave_tab = tab(&world, &dave).await;
        let id = ask_to_restart(&world, CLIENT_ALL, "nginx").await;
        assert_eq!(chase_tab().await, "event: clients\ndata: {\"waiting\":1}\n\n");
        let refuse = format!("/api/1/clients/requests/{id}/refuse");
        send(&world, post(&refuse, &json!({})).by(&chase)).await;
        assert_eq!(chase_tab().await, "event: clients\ndata: {\"waiting\":0}\n\n");
        // One who is no administrator is told nothing of them: the next
        // thing dave's tab hears is his own lock.
        send(&world, post("/api/1/lock", &json!({})).by(&dave)).await;
        let next = tokio::time::timeout(Duration::from_millis(300), dave_tab()).await;
        assert!(next.is_err() || !next.unwrap().contains("clients"));
    });
}

#[test]
fn a_person_s_own_clients_are_listed_added_and_removed_through_their_worker() {
    run(async {
        let world = lab_world("mcp-own");
        let chase = sign_in(&world, "chase").await;
        let listed = send(&world, get("/api/1/account/clients").by(&chase)).await;
        assert_eq!(listed.json(), json!({ "clients": [{ "owner": "chase", "name": "helper" }] }));
        let new = json!({ "name": "desk", "way": "https", "scopes": ["*:read"], "days": 90 });
        let added = send(&world, post("/api/1/account/clients/add", &new).by(&chase)).await;
        assert_eq!(added.json(), json!({ "token": "dkc_chase_aa" }));
        let asked = json!({
            "proof": "", "name": "desk", "way": "https", "scopes": ["*:read"], "days": 90,
        });
        assert!(called(&world).contains(&("client-add".into(), asked)), "{:?}", called(&world));
        let over_ssh = json!({ "name": "shell", "way": "ssh", "scopes": ["services"], "days": 30 });
        let added = send(&world, post("/api/1/account/clients/add", &over_ssh).by(&chase)).await;
        assert_eq!(added.json(), json!({ "token": null }));
        // Removing one lapses what it had asked for.
        let id = ask_to_restart(&world, CLIENT_ALL, "nginx").await;
        let gone = json!({ "name": "helper" });
        let removed = send(&world, post("/api/1/account/clients/remove", &gone).by(&chase)).await;
        assert_eq!(removed.json(), json!({ "done": true }));
        assert!(waiting(&world, &chase).await.is_empty());
        let stands = told(&world, CLIENT_ALL, "request", &json!({ "id": id })).await;
        assert_eq!(stands["state"], "lapsed", "{stands}");
        // The areas the dialog offers, for administrators.
        let areas = send(&world, get("/api/1/clients/areas").by(&chase)).await.json();
        let services = areas["areas"]
            .as_array()
            .unwrap()
            .iter()
            .find(|area| area["area"] == "services")
            .expect("services");
        assert_eq!((&services["reads"], &services["asks"]), (&json!(2), &json!(3)));
        let dave = sign_in(&world, "dave").await;
        assert_eq!(send(&world, get("/api/1/clients/areas").by(&dave)).await.error(), "ADM-122");
    });
}
