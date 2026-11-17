// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashSet;

use super::*;
use crate::admind::actions::tests::{LAB, validate};
use crate::admind::actions::{REGISTRY, offered};
use crate::admind::errors::Failure;

/// The action sets the document is made for: what the console offers, and
/// the tests' own, which have a read, a change with a probe and a long one.
fn sets() -> [&'static [&'static dyn Registered]; 2] {
    [offered(), LAB]
}

/// The operation of `method` and `path` in `document`.
fn operation<'a>(document: &'a Value, method: &str, path: &str) -> &'a Value {
    let operation = &document["paths"][path][method];
    assert!(operation.is_object(), "no {method} {path}");
    operation
}

/// The names of `keys` of `value`, which is an object.
fn names(value: &Value) -> HashSet<&str> {
    value.as_object().unwrap().keys().map(String::as_str).collect()
}

/// Whether `status` is an error's: 4xx or 5xx.
fn is_error(status: &str) -> bool {
    status.starts_with('4') || status.starts_with('5')
}

/// Whether `status` is an error that admind answers with an ID: any but
/// 431, which the HTTP server gives with no body.
fn has_ids(status: &str) -> bool {
    is_error(status) && status != "431"
}

/// The statuses `operation` answers with.
fn statuses(operation: &Value) -> HashSet<&str> {
    names(&operation["responses"])
}

/// The IDs `operation` says it answers `status` with.
fn ids<'a>(operation: &'a Value, status: &str) -> Vec<&'a str> {
    let schema = &operation["responses"][status]["content"]["application/json"]["schema"];
    let listed = &schema["allOf"][1]["properties"]["error"]["properties"]["id"]["enum"];
    listed.as_array().unwrap().iter().map(|id| id.as_str().unwrap()).collect()
}

/// The schema of all the arguments of `operation`: its parameters, of
/// its address and its query, and its body's properties, put together as
/// `actions::schema` made them.
fn arguments_of(operation: &Value) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for parameter in operation["parameters"].as_array().into_iter().flatten() {
        let name = parameter["name"].as_str().unwrap();
        properties.insert(name.into(), parameter["schema"].clone());
        if parameter["required"] == true {
            required.push(json!(name));
        }
    }
    let body = &operation["requestBody"]["content"]["application/json"]["schema"];
    if body.is_object() {
        properties.extend(body["properties"].as_object().unwrap().clone());
        required.extend(body["required"].as_array().unwrap().clone());
    }
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// The example `operation` gives: its parameters' and its body's.
fn example_of(operation: &Value) -> Value {
    let mut example = Map::new();
    for parameter in operation["parameters"].as_array().into_iter().flatten() {
        if let Some(value) = parameter.get("example") {
            example.insert(parameter["name"].as_str().unwrap().into(), value.clone());
        }
    }
    let body = &operation["requestBody"]["content"]["application/json"];
    if let Some(Value::Object(given)) = body.get("example") {
        example.extend(given.clone());
    }
    Value::Object(example)
}

#[test]
fn the_document_is_json_of_openapi_3_1_with_the_two_ways_to_ask() {
    let text = document(REGISTRY).to_string();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["openapi"], "3.1.0");
    assert!(parsed["info"]["title"].as_str().is_some_and(|title| !title.is_empty()));
    let schemes = &parsed["components"]["securitySchemes"];
    assert_eq!(names(schemes), HashSet::from(["session", "csrf", "token"]));
    assert_eq!(schemes["session"]["in"], "cookie");
    assert_eq!(schemes["session"]["name"], "__Host-dukovo");
    assert_eq!(schemes["csrf"]["in"], "header");
    assert_eq!(schemes["csrf"]["name"], "x-dukovo-csrf");
    assert_eq!(schemes["token"]["type"], "http");
    assert_eq!(schemes["token"]["scheme"], "bearer");
}

#[test]
fn every_route_of_the_table_is_an_operation_named_for_it_and_there_are_no_others() {
    for actions in sets() {
        let document = document(actions);
        for entry in TABLE {
            let operation = operation(&document, method(entry.verb), entry.path);
            assert_eq!(operation["operationId"], format!("{:?}", entry.route), "{}", entry.path);
            assert!(operation["summary"].as_str().is_some_and(|text| !text.is_empty()));
            assert_eq!(operation["tags"], json!([tag(entry.access)]), "{}", entry.path);
            // The address's own parts are its path's parameters: a
            // number, or a trial's, an account's or a session's name. Only the follow of the log takes
            // a query, its filters.
            let parameters = operation["parameters"].as_array().cloned().unwrap_or_default();
            let place = |place: &str| -> Vec<&Value> {
                parameters.iter().filter(|parameter| parameter["in"] == place).collect()
            };
            let path: Vec<&str> =
                place("path").iter().map(|parameter| parameter["name"].as_str().unwrap()).collect();
            assert_eq!(path, parameters_of(entry.path).collect::<Vec<_>>(), "{}", entry.path);
            for parameter in place("path") {
                let text = matches!(parameter["name"].as_str(), Some("trial" | "name"))
                    || entry.path.contains("/sessions/");
                let kind = if text { "string" } else { "integer" };
                assert_eq!(parameter["schema"]["type"], kind, "{}", entry.path);
            }
            assert_eq!(place("query").is_empty(), entry.route != Route::Follow, "{}", entry.path);
            assert_eq!(parameters.len(), place("path").len() + place("query").len());
        }
        let all: usize = document["paths"]
            .as_object()
            .unwrap()
            .values()
            .map(|item| item.as_object().unwrap().len())
            .sum();
        assert_eq!(all, TABLE.len() + actions.len());
    }
}

#[test]
fn every_action_is_an_operation_named_by_its_id_with_its_method_and_address() {
    for actions in sets() {
        let document = document(actions);
        for action in actions {
            let method = if action.posts() { "post" } else { "get" };
            let operation = operation(&document, method, action.path());
            assert_eq!(operation["operationId"], action.id());
            assert_eq!(operation["summary"], action.title());
            assert_eq!(operation["tags"], json!(["api"]));
        }
    }
}

#[test]
fn an_action_s_arguments_are_in_its_address_its_query_or_its_body_and_a_get_has_no_body() {
    for actions in sets() {
        let document = document(actions);
        for action in actions {
            let id = action.id();
            let method = if action.posts() { "post" } else { "get" };
            let operation = operation(&document, method, action.path());
            let (mut in_address, mut in_query) = (HashSet::new(), HashSet::new());
            for parameter in operation["parameters"].as_array().into_iter().flatten() {
                let name = parameter["name"].as_str().unwrap();
                let field = action.fields().iter().find(|field| field.name == name).unwrap();
                assert!(parameter["schema"].is_object(), "{id}");
                match parameter["in"].as_str() {
                    Some("path") => {
                        assert_eq!(parameter["required"], true, "{id}");
                        in_address.insert(name);
                    }
                    // A read's other arguments are its query's, there
                    // when the action needs them; a read that is a POST,
                    // for a secret (ADR 0561), has them in its body.
                    Some("query") if !action.posts() => {
                        assert_eq!(parameter["required"], field.required, "{id}: {name}");
                        in_query.insert(name);
                    }
                    other => panic!("{id}: {name} is in {other:?}"),
                }
            }
            assert_eq!(in_address, parameters_of(action.path()).collect::<HashSet<_>>(), "{id}");
            let body = &operation["requestBody"]["content"]["application/json"]["schema"];
            let in_body =
                if body.is_object() { names(&body["properties"]) } else { HashSet::new() };
            assert!(in_address.is_disjoint(&in_body), "{id}");
            let fields: HashSet<&str> = action.fields().iter().map(|field| field.name).collect();
            assert_eq!(&(&in_address | &in_query) | &in_body, fields, "{id}");
            if !action.posts() {
                assert!(operation.get("requestBody").is_none(), "{id}: a read is a GET");
            }
        }
    }
}

#[test]
fn the_log_s_read_takes_its_filters_in_its_query() {
    let document = document(REGISTRY);
    let read = operation(&document, "get", "/api/1/logs");
    let query: Vec<(&str, &Value)> = read["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|parameter| {
            assert_eq!(parameter["in"], "query");
            (parameter["name"].as_str().unwrap(), &parameter["required"])
        })
        .collect();
    let names: HashSet<&str> = query.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        HashSet::from(["lines", "service", "kernel", "priority", "since", "until", "grep"])
    );
    assert!(query.iter().all(|(_, required)| **required == json!(false)));
}

#[test]
fn every_action_s_good_examples_fit_what_the_document_says_and_its_bad_ones_do_not() {
    for actions in sets() {
        let document = document(actions);
        for action in actions {
            let id = action.id();
            let method = if action.posts() { "post" } else { "get" };
            let operation = operation(&document, method, action.path());
            let schema = arguments_of(operation);
            let (good, bad, _) = action.example_values();
            for args in &good {
                validate::validate(&schema, args).unwrap_or_else(|why| panic!("{id}: {why}"));
            }
            // A bad example a schema cannot refuse, as two times in the
            // wrong order, has the types the action takes and values its
            // own check refuses.
            for args in &bad {
                if validate::validate(&schema, args).is_ok() {
                    let refused = action.prepare(args.clone()).err().map(|failure| failure.kind);
                    assert_eq!(refused, Some(Kind::BadValue), "{id}: accepts {args}");
                }
            }
            // What a tester sends as it is is the first good example.
            assert_eq!(&example_of(operation), &good[0], "{id}");
        }
    }
}

#[test]
fn a_body_that_has_nothing_required_is_optional_and_one_that_has_is_not() {
    let document = document(REGISTRY);
    let body = |id: &str| {
        let action = REGISTRY.iter().find(|action| action.id() == id).unwrap();
        operation(&document, "post", action.path())["requestBody"].clone()
    };
    // The service's name is in the address, so there is nothing for a body.
    let start = body("service.start");
    assert_eq!(start["required"], false);
    assert_eq!(start["content"]["application/json"]["example"], json!({}));
    let reach = body("console.reach");
    assert_eq!(reach["required"], true);
    assert_eq!(reach["content"]["application/json"]["example"], json!({ "reach": "private" }));
}

#[test]
fn what_a_session_or_a_token_must_send_is_what_the_table_says_of_the_route() {
    let document = document(REGISTRY);
    let security =
        |method: &str, path: &str| operation(&document, method, path)["security"].clone();
    // Nothing is needed for what is open, or to sign in.
    assert_eq!(security("get", "/api/1/health"), json!([]));
    assert_eq!(security("get", "/api/1/openapi.json"), json!([]));
    assert_eq!(security("post", "/api/1/sign-in"), json!([]));
    // A session may only read with its cookie, and changes with the header.
    assert_eq!(security("get", "/api/1/session"), json!([{ "session": [] }]));
    assert_eq!(security("post", "/api/1/sign-out"), json!([{ "session": [], "csrf": [] }]));
    assert_eq!(security("post", "/api/1/lock"), json!([{ "session": [], "csrf": [] }]));
    // The API is for a token as well, which needs no header.
    assert_eq!(security("get", "/api/1/actions"), json!([{ "token": [] }, { "session": [] }]));
    assert_eq!(
        security("post", "/api/1/show"),
        json!([{ "token": [] }, { "session": [], "csrf": [] }])
    );
    assert_eq!(
        security("post", "/api/1/services/{name}/start"),
        json!([{ "token": [] }, { "session": [], "csrf": [] }])
    );
    assert_eq!(
        security("get", "/api/1/services/{name}/explain"),
        json!([{ "token": [] }, { "session": [] }])
    );
}

#[test]
fn every_error_status_lists_ids_that_have_that_status() {
    for actions in sets() {
        let document = document(actions);
        for (path, item) in document["paths"].as_object().unwrap() {
            for (method, operation) in item.as_object().unwrap() {
                let action = actions
                    .iter()
                    .find(|action| action.path() == path && action.posts() == (method == "post"));
                let own: Vec<&str> = action.map_or_default(|action| action.errors().to_vec());
                let others: Vec<&str> = action
                    .map_or_default(|action| refusals(*action).iter().map(|(id, _)| *id).collect());
                let responses = operation["responses"].as_object().unwrap();
                // The Terminal's socket works when the connection is handed over.
                assert!(
                    responses.keys().any(|status| status.starts_with('2') || status == "101"),
                    "{method} {path} has no success"
                );
                assert!(responses.keys().all(|status| status.len() == 3), "{method} {path}");
                // A head over the limits is refused before any route is
                // found, by the HTTP server, with no body.
                let refused = &responses["431"];
                assert!(
                    refused["description"].as_str().unwrap().contains("16 KiB"),
                    "{method} {path}"
                );
                assert!(refused.get("content").is_none(), "{method} {path}");
                for (status, response) in responses.iter().filter(|(s, _)| has_ids(s)) {
                    let schema = &response["content"]["application/json"]["schema"];
                    assert_eq!(schema["allOf"][0]["$ref"], "#/components/schemas/Error");
                    for id in ids(operation, status) {
                        let wanted = Kind::of(id).map_or_else(
                            || {
                                if others.contains(&id) {
                                    return Kind::Cannot.status();
                                }
                                assert!(own.contains(&id), "{method} {path}: {id} is nobody's");
                                Kind::CommandFailed.status()
                            },
                            Kind::status,
                        );
                        assert_eq!(wanted.to_string(), *status, "{method} {path} {id}");
                    }
                    // Only the answer that says to wait carries when.
                    assert_eq!(
                        response.get("headers").is_some(),
                        *status == "429",
                        "{method} {path} {status}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_errors_of_an_operation_follow_from_its_access_its_handler_and_its_command() {
    let document = document(offered());
    let get = |path: &str| operation(&document, "get", path);
    let post = |path: &str| operation(&document, "post", path);
    // Open to anyone: nothing but a request that was not right, and a
    // token, which no route that is not the API's takes.
    assert_eq!(statuses(get("/api/1/health")), HashSet::from(["200", "400", "403", "421", "431"]));
    assert_eq!(ids(get("/api/1/health"), "403"), ["ADM-124"]);
    // The sign-in is counted, which says when to try again.
    assert!(ids(post("/api/1/sign-in"), "429").contains(&"ADM-105"));
    assert!(ids(post("/api/1/sign-in"), "401").contains(&"ADM-101"));
    assert!(ids(post("/api/1/sign-in"), "403").contains(&"ADM-109"));
    // A session that has ended, and one that may only add a second step.
    assert!(ids(get("/api/1/session"), "401").contains(&"ADM-107"));
    assert!(!ids(get("/api/1/session"), "403").contains(&"ADM-118"));
    assert!(ids(get("/api/1/account/preferences"), "403").contains(&"ADM-118"));
    // A GET has no body to be too big or not JSON, and no origin to check.
    assert!(!statuses(get("/api/1/session")).contains("413"));
    assert!(!ids(get("/api/1/session"), "403").contains(&"ADM-109"));
    assert!(statuses(post("/api/1/lock")).contains("413"));
    // A number in the address that cannot be one is no route, and a number
    // there is none of is not kept.
    assert_eq!(ids(get("/api/1/changes/{id}"), "404"), ["ADM-116", "ADM-127"]);
    // The API's own are a token that is not one, and one that may not.
    assert!(ids(get("/api/1/actions"), "401").contains(&"ADM-115"));
    assert!(ids(post("/api/1/show"), "403").contains(&"ADM-124"));
}

#[test]
fn an_action_answers_with_its_command_s_own_ids_and_a_change_with_what_only_a_change_meets() {
    let document = document(LAB);
    let restart = operation(&document, "post", "/api/1/lab/{name}/restart");
    assert_eq!(ids(restart, "422"), ["ADM-123", "SVC-104"]);
    assert!(ids(restart, "403").contains(&"ADM-122"));
    assert!(ids(restart, "403").contains(&"ADM-106"));
    let list = operation(&document, "get", "/api/1/lab");
    assert_eq!(ids(list, "422"), ["ADM-123"]);
    assert!(!ids(list, "403").contains(&"ADM-106"));
    // A read's address has no part that is an argument here, and a change's has.
    assert!(!statuses(list).contains("404"));
    assert!(statuses(restart).contains("404"));
    // What works: a long action begins a task.
    let long = operation(&document, "post", "/api/1/lab/long");
    assert_eq!(statuses(long).iter().filter(|status| status.starts_with('2')).count(), 1);
    assert_eq!(
        long["responses"]["202"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Started"
    );
    assert!(ids(long, "503").contains(&"ADM-126"));
    let done = &restart["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(done["$ref"], "#/components/schemas/Done");
}

#[test]
fn every_error_the_console_has_is_the_answer_of_some_operation_but_the_wrong_method() {
    let document = document(REGISTRY);
    let mut listed = HashSet::new();
    for item in document["paths"].as_object().unwrap().values() {
        for operation in item.as_object().unwrap().values() {
            for status in statuses(operation).iter().filter(|status| has_ids(status)) {
                listed.extend(ids(operation, status));
            }
        }
    }
    for kind in Kind::ALL {
        // An operation is asked with its own method, which no answer of
        // the table's says is not used.
        let said = listed.contains(kind.id());
        assert_eq!(said, kind != Kind::NoMethod, "{}", kind.id());
    }
}

#[test]
fn an_error_answer_has_the_members_the_schema_names_and_an_id_that_matches() {
    let schemas = schemas();
    let error = &schemas["Error"]["properties"]["error"];
    let known = names(&error["properties"]);
    let pattern = error["properties"]["id"]["pattern"].as_str().unwrap();
    let mut failures: Vec<Failure> =
        Kind::ALL.iter().map(|kind| Failure::new(*kind, "x")).collect();
    failures.extend([
        Failure::bad_value("name", "x"),
        Failure::wait("x", 5),
        Failure::command(Some("SVC-104"), "x".into(), &["service".into()], 1, "x (SVC-104)"),
    ]);
    for failure in failures {
        let body = failure.body();
        let given = names(&body["error"]);
        assert!(given.is_subset(&known), "{}: {given:?}", failure.kind.id());
        for required in error["required"].as_array().unwrap() {
            assert!(given.contains(required.as_str().unwrap()), "{}", failure.kind.id());
        }
        let id = body["error"]["id"].as_str().unwrap();
        assert_eq!(validate::matches(pattern, id), Ok(true), "{id}");
    }
    // The members of a command that did not work.
    let command = Failure::command(None, "x".into(), &["a".into()], 1, "").body();
    assert_eq!(names(&command["error"]["command"]), HashSet::from(["argv", "status", "stderr"]));
}

#[test]
fn what_a_route_answers_is_described_where_the_table_knows_it() {
    let document = document(REGISTRY);
    let check = &operation(&document, "post", "/api/1/check")["responses"]["200"];
    let answer = &check["content"]["application/json"]["schema"];
    assert_eq!(answer["required"], json!(["changed", "now"]));
    assert_eq!(answer["properties"]["changed"]["type"], json!(["boolean", "null"]));
    let socket = &operation(&document, "get", "/api/1/terminal")["responses"];
    assert_eq!(names(socket).iter().filter(|status| !is_error(status)).count(), 1);
    assert!(socket["101"]["description"].as_str().is_some_and(|text| text.contains("csrf")));
    assert!(ids(operation(&document, "get", "/api/1/terminal"), "403").contains(&"ADM-109"));
    let pane = operation(&document, "post", "/api/1/terminals/panes/{pane}/end");
    assert!(pane["parameters"][0]["description"].as_str().unwrap().contains("pane"));
    assert!(ids(pane, "404").contains(&"ADM-131"));
    let health = &operation(&document, "get", "/api/1/health")["responses"]["200"];
    assert_eq!(health["content"]["application/json"]["schema"]["properties"]["ok"]["const"], true);
    let events = &operation(&document, "get", "/api/1/events")["responses"]["200"];
    assert!(events["content"]["text/event-stream"].is_object());
    assert!(events["content"].get("application/json").is_none());
    // A read answers what its command printed: no shape is claimed.
    let read = &operation(&document, "get", "/api/1/services")["responses"]["200"];
    assert!(read["content"]["application/json"].get("schema").is_none());
}

#[test]
fn show_and_check_take_any_action_and_its_arguments_and_no_other_route_of_the_table_has_a_body() {
    for actions in sets() {
        let document = document(actions);
        let show = operation(&document, "post", "/api/1/show");
        let check = operation(&document, "post", "/api/1/check");
        assert_eq!(check["requestBody"], show["requestBody"]);
        let media = &show["requestBody"]["content"]["application/json"];
        assert_eq!(show["requestBody"]["required"], true);
        let listed: Vec<&str> = media["schema"]["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect();
        let offered: Vec<&str> = actions.iter().map(|action| action.id()).collect();
        assert_eq!(listed, offered);
        // What a tester sends as it is names the first action, with its first good example.
        let (good, _, _) = actions[0].example_values();
        assert_eq!(media["example"], json!({ "action": actions[0].id(), "args": good[0] }));
        // The validator reads no object that says nothing of its members, so the action alone.
        let named = &media["schema"]["properties"]["action"];
        validate::validate(named, &media["example"]["action"]).unwrap();
        assert!(validate::validate(named, &json!("no.such")).is_err());
        assert_eq!(media["schema"]["required"], json!(["action"]));
        for entry in TABLE.iter().filter(|entry| !matches!(entry.route, Route::Show | Route::Check))
        {
            let operation = operation(&document, method(entry.verb), entry.path);
            assert!(operation.get("requestBody").is_none(), "{}", entry.path);
        }
    }
}
