// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the MCP server answers to a client's bytes, whichever way they
//! came in (ADR 0692).
//!
//! `/mcp` and the socket both give [`handle`] the bytes of one message
//! or one batch, with the client authd named for the credential that
//! came with them. It reads them ([`rpc`]), asks the protocol what each
//! means ([`protocol`]), runs the tool a call names ([`tools`]), and
//! gives the answer with the HTTP status the protocol fixes for it. A
//! batch's messages run one after another, in their order.

use std::rc::Rc;

use serde_json::Value;

use super::super::shared::Shared;
use super::client::Client;
use super::protocol::{self, Headers, Step};
use super::rpc::{self, Incoming, Message, Refused};
use super::tools;

/// What is answered to a client's bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Answered {
    /// The HTTP status, which `/mcp` answers with: 202 when there is no
    /// body.
    pub status: u16,
    /// The answer, or nothing, for notifications.
    pub body: Option<Value>,
}

impl Answered {
    /// The answer that refuses `refused`.
    fn refusal(refused: Refused) -> Answered {
        let Refused { id, error } = refused;
        Answered { status: error.status, body: Some(rpc::answer(id.as_ref(), Err(error))) }
    }

    /// No answer: what was sent was taken.
    fn nothing() -> Answered {
        Answered { status: 202, body: None }
    }
}

/// Answers `bytes` from `client`, which came with `headers` when they
/// came by HTTPS.
pub async fn handle(
    shared: &Rc<Shared>,
    client: &Client,
    bytes: &[u8],
    headers: Option<&Headers<'_>>,
) -> Answered {
    let messages = match rpc::parse(bytes) {
        Ok(Incoming::One(message)) => {
            return match one(shared, client, message, headers).await {
                Some((status, body)) => Answered { status, body: Some(body) },
                None => Answered::nothing(),
            };
        }
        Ok(Incoming::Batch(messages)) => messages,
        Err(refused) => return Answered::refusal(refused),
    };
    if let Err(error) = protocol::batch(&messages, headers) {
        return Answered::refusal(Refused { id: None, error });
    }
    let mut answers = Vec::new();
    for message in messages {
        match message {
            Ok(message) => {
                if let Some((_, body)) = one(shared, client, message, headers).await {
                    answers.push(body);
                }
            }
            Err(Refused { id, error }) => answers.push(rpc::answer(id.as_ref(), Err(error))),
        }
    }
    if answers.is_empty() {
        Answered::nothing()
    } else {
        Answered { status: 200, body: Some(Value::Array(answers)) }
    }
}

/// Answers one message: the status and the answer, or nothing for a
/// notification. A notification that is out of order is answered only
/// by HTTPS, whose answer needs a status; the socket answers none.
async fn one(
    shared: &Rc<Shared>,
    client: &Client,
    message: Message,
    headers: Option<&Headers<'_>>,
) -> Option<(u16, Value)> {
    match message {
        Message::Notification { method, params } => {
            match protocol::notification(&method, &params, headers) {
                Err(error) if headers.is_some() => {
                    Some((error.status, rpc::answer(None, Err(error))))
                }
                Ok(()) | Err(_) => None,
            }
        }
        Message::Request { id, method, params } => {
            let outcome = match protocol::request(&method, &params, headers, &tools::TOOLS) {
                Ok(Step::Answer(result)) => Ok(result),
                Ok(Step::Call { era, name, arguments }) => {
                    Ok(match tools::call(shared, client, &name, arguments).await {
                        Ok(answer) => protocol::called(era, Ok(answer)),
                        Err(failure) => protocol::called(era, Err(&failure)),
                    })
                }
                Err(error) => Err(error),
            };
            let status = outcome.as_ref().err().map_or(200, |error| error.status);
            Some((status, rpc::answer(Some(&id), outcome)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use accounts::console::Way;
    use serde_json::json;

    use super::*;
    use crate::admind::actions::tests::LAB;
    use crate::admind::fixture::{Console as World, run};
    use crate::admind::mcp::client::tests::client;

    /// A console with the test actions, whose authd is asked nothing.
    fn world(what: &str) -> World {
        let world = World::new(what, Arc::new(|_, _| vec!["no nothing is asked here".into()]));
        world.shared.actions.set(LAB);
        world
    }

    /// What the server answers `sent` when it came by the socket, from
    /// chase's client that reaches everything.
    async fn answered(world: &World, sent: &Value) -> Answered {
        let helper = client("chase", "helper", Way::Ssh, "*");
        handle(&world.shared, &helper, sent.to_string().as_bytes(), None).await
    }

    fn asks(id: u64, method: &str, params: &Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn tells(method: &str, params: &Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": method, "params": params })
    }

    #[test]
    fn a_request_is_answered_with_its_id_and_a_notification_with_nothing() {
        run(async {
            let world = world("server-one");
            let ping = json!({ "jsonrpc": "2.0", "id": "a-1", "method": "ping" });
            let pong = answered(&world, &ping).await;
            let body = json!({ "jsonrpc": "2.0", "id": "a-1", "result": {} });
            assert_eq!(pong, Answered { status: 200, body: Some(body) });
            let ready = tells("notifications/initialized", &json!({}));
            assert_eq!(answered(&world, &ready).await, Answered::nothing());
            // The socket answers no notification, one of a revision that
            // is not spoken neither: there is no request to answer.
            let envelope =
                json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" } });
            let odd = tells("notifications/cancelled", &envelope);
            assert_eq!(answered(&world, &odd).await, Answered::nothing());
            // The same as a request is refused, with its ID.
            let refused = answered(&world, &asks(2, "tools/list", &envelope)).await;
            let body = refused.body.expect("an answer");
            assert_eq!(
                (&body["id"], &body["error"]["code"]),
                (&json!(2), &json!(rpc::UNSUPPORTED_VERSION))
            );
        });
    }

    #[test]
    fn a_batch_is_answered_in_its_order_with_what_each_request_came_to() {
        run(async {
            let world = world("server-batch");
            let batch = json!([
                asks(1, "ping", &json!({})),
                tells("notifications/initialized", &json!({})),
                asks(2, "resources/list", &json!({})),
                { "jsonrpc": "2.0", "id": 3, "method": "ping", "params": [] },
                asks(4, "tools/call", &json!({ "name": "areas", "arguments": {} })),
            ]);
            let all = answered(&world, &batch).await;
            assert_eq!(all.status, 200);
            let answers = all.body.expect("the answers");
            let answers = answers.as_array().expect("a list");
            let ids: Vec<&Value> = answers.iter().map(|answer| &answer["id"]).collect();
            assert_eq!(ids, [&json!(1), &json!(2), &json!(3), &json!(4)]);
            assert_eq!(answers[0]["result"], json!({}));
            assert_eq!(answers[1]["error"]["code"], rpc::NO_METHOD);
            assert_eq!(answers[2]["error"]["code"], rpc::PARAMS);
            assert_eq!(answers[3]["result"]["isError"], false);
            // Notifications alone have no answer.
            let told = json!([tells("notifications/initialized", &json!({}))]);
            assert_eq!(answered(&world, &told).await, Answered::nothing());
        });
    }

    #[test]
    fn a_batch_that_holds_what_is_no_message_or_too_many_is_refused_whole() {
        run(async {
            let world = world("server-batch-refused");
            let ping = asks(1, "ping", &json!({}));
            let many: Vec<Value> = (0..=rpc::BATCH_MAX).map(|_| ping.clone()).collect();
            for wrong in [json!([]), json!([ping, 7]), json!([ping, { "id": 2 }]), json!(many)] {
                let refused = answered(&world, &wrong).await;
                assert_eq!(refused.status, 400, "{wrong}");
                let body = refused.body.expect("an answer");
                assert_eq!(
                    (&body["id"], &body["error"]["code"]),
                    (&Value::Null, &json!(rpc::INVALID)),
                    "{wrong}"
                );
            }
            // The revision that names itself in each message has none.
            let envelope =
                json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } });
            let named = json!([asks(1, "tools/list", &envelope)]);
            let refused = answered(&world, &named).await.body.expect("an answer");
            assert_eq!(refused["error"]["code"], rpc::INVALID);
        });
    }

    #[test]
    fn a_tool_s_failure_is_a_result_and_a_tool_there_is_not_is_the_protocol_s_error() {
        run(async {
            let world = world("server-tools");
            let call = |tool: &str, arguments: Value| {
                asks(5, "tools/call", &json!({ "name": tool, "arguments": arguments }))
            };
            let failed = answered(&world, &call("actions", json!({ "area": "x" }))).await;
            assert_eq!(failed.status, 200);
            let result = &failed.body.expect("an answer")["result"];
            assert_eq!(result["isError"], true, "{result}");
            assert_eq!(result["structuredContent"]["error"]["id"], "ADM-125");
            let none = answered(&world, &call("shell", json!({}))).await.body.expect("an answer");
            assert_eq!(none["error"]["code"], rpc::PARAMS, "{none}");
            assert!(none.get("result").is_none(), "{none}");
        });
    }
}
