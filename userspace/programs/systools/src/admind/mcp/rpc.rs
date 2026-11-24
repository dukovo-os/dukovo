// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! JSON-RPC 2.0 as MCP speaks it (ADR 0692): what a client's bytes are
//! read as, and how an answer is written.
//!
//! Nothing here knows a method or a way in. [`parse`] reads one message
//! or one batch; [`answer`] writes the answer to a request. What a
//! method means is [`protocol`](super::protocol)'s, and the bytes come
//! and go by `/mcp` or by the socket.
//!
//! MCP is narrower than JSON-RPC in three ways, and so is this: an ID is
//! a string or a whole number and never null, parameters are an object
//! and never a list, and this server asks its clients nothing, so it
//! takes no answers.

use serde_json::{Map, Number, Value, json};

/// The longest message taken, in bytes, by either way in.
pub const MESSAGE_MAX: usize = 64 * 1024;
/// The most messages one batch may hold.
pub const BATCH_MAX: usize = 16;

/// The bytes are not JSON.
pub const PARSE: i64 = -32700;
/// The JSON is not a request or a notification.
pub const INVALID: i64 = -32600;
/// There is no such method.
pub const NO_METHOD: i64 = -32601;
/// The method does not take those parameters.
pub const PARAMS: i64 = -32602;
/// The server could not do what it is for.
pub const INTERNAL: i64 = -32603;
/// An HTTPS request's headers and its body disagree.
pub const HEADER_MISMATCH: i64 = -32020;
/// The revision of the protocol that was asked for is not served.
pub const UNSUPPORTED_VERSION: i64 = -32022;

/// The ID a client gave a request, which its answer carries back.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    /// A whole number.
    Number(Number),
    /// A string.
    Text(String),
}

impl Id {
    /// The ID that `value` is, if it is one: a string, or a number with
    /// no fraction.
    fn of(value: &Value) -> Option<Id> {
        match value {
            Value::String(text) => Some(Id::Text(text.clone())),
            Value::Number(number) if number.is_i64() || number.is_u64() => {
                Some(Id::Number(number.clone()))
            }
            _ => None,
        }
    }

    /// The ID as JSON.
    pub fn json(&self) -> Value {
        match self {
            Id::Number(number) => Value::Number(number.clone()),
            Id::Text(text) => Value::String(text.clone()),
        }
    }
}

/// One message of a client's.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// A request, which is answered under its ID.
    Request {
        /// The ID to answer under.
        id: Id,
        /// The method asked for.
        method: String,
        /// Its parameters: none is the same as an empty object.
        params: Map<String, Value>,
    },
    /// A notification, which is never answered.
    Notification {
        /// What is notified.
        method: String,
        /// Its parameters. Ones that are not an object are read as none:
        /// a notification cannot be refused, and none that is taken here
        /// reads its parameters.
        params: Map<String, Value>,
    },
}

/// What one read of a client's bytes holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// One message.
    One(Message),
    /// A batch: its messages in order, each one read or refused by
    /// itself.
    Batch(Vec<Result<Message, Refused>>),
}

/// A message that cannot be taken, and the ID to say so under.
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    /// The request's ID, where one could be read.
    pub id: Option<Id>,
    /// Why not.
    pub error: Error,
}

/// An error, as JSON-RPC answers one.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    /// Which error.
    pub code: i64,
    /// What happened, in a sentence.
    pub message: String,
    /// More, for a client to act on.
    pub data: Option<Value>,
    /// The HTTP status `/mcp` answers it with. The protocol fixes one
    /// for each error, and for some by where it was found, so whoever
    /// finds the error says it. The socket has no use for it.
    pub status: u16,
}

impl Error {
    /// An error with `code` and `message`, answered with `status`.
    fn new(code: i64, message: impl Into<String>, status: u16) -> Error {
        Error { code, message: message.into(), data: None, status }
    }

    /// The bytes are not JSON.
    pub fn parse() -> Error {
        Error::new(PARSE, "the message is not JSON", 400)
    }

    /// The JSON is not a message this server takes, for `why`.
    pub fn invalid(why: impl Into<String>) -> Error {
        Error::new(INVALID, why, 400)
    }

    /// There is no method `method`. The answer's status is 200, as the
    /// older revisions have it; the newest answers 404
    /// ([`Error::with_status`]).
    pub fn no_method(method: &str) -> Error {
        Error::new(NO_METHOD, format!("there is no method {}", shown(method)), 200)
    }

    /// A method does not take its parameters, for `why`.
    pub fn params(why: impl Into<String>) -> Error {
        Error::new(PARAMS, why, 200)
    }

    /// The server could not do what it is for.
    pub fn internal(why: impl Into<String>) -> Error {
        Error::new(INTERNAL, why, 200)
    }

    /// An HTTPS request's headers and its body disagree, as `why` says.
    pub fn header_mismatch(why: impl Into<String>) -> Error {
        Error::new(HEADER_MISMATCH, why, 400)
    }

    /// The revision `requested` is not served; `supported` are.
    pub fn unsupported_version(requested: &str, supported: &[&str]) -> Error {
        Error {
            data: Some(json!({ "supported": supported, "requested": requested })),
            ..Error::new(
                UNSUPPORTED_VERSION,
                format!("this server does not speak revision {} of MCP", shown(requested)),
                400,
            )
        }
    }

    /// The same error, answered with `status`.
    #[must_use]
    pub fn with_status(self, status: u16) -> Error {
        Error { status, ..self }
    }

    /// The error as JSON.
    fn json(&self) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        error
    }
}

/// A client's word as an error's sentence shows it: its first 64
/// characters, with none that could break a line or a terminal.
pub(super) fn shown(word: &str) -> String {
    let safe: String =
        word.chars().take(64).map(|c| if c.is_control() { '?' } else { c }).collect();
    format!("\"{safe}\"")
}

/// What `bytes` hold: a message or a batch.
///
/// # Errors
///
/// Bytes that are too long, are not JSON, or are JSON that is neither a
/// message nor a list of them. A batch's messages are refused each by
/// itself, inside [`Incoming::Batch`].
pub fn parse(bytes: &[u8]) -> Result<Incoming, Refused> {
    let whole = |error| Refused { id: None, error };
    if bytes.len() > MESSAGE_MAX {
        let words = format!("the message is longer than {MESSAGE_MAX} bytes");
        return Err(whole(Error::invalid(words)));
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| whole(Error::parse()))?;
    match value {
        Value::Array(messages) if messages.is_empty() => {
            Err(whole(Error::invalid("the batch holds no message")))
        }
        Value::Array(messages) if messages.len() > BATCH_MAX => {
            let words = format!("the batch holds more than {BATCH_MAX} messages");
            Err(whole(Error::invalid(words)))
        }
        Value::Array(messages) => Ok(Incoming::Batch(messages.into_iter().map(message).collect())),
        one => message(one).map(Incoming::One),
    }
}

/// The message that `value` is.
fn message(value: Value) -> Result<Message, Refused> {
    let whole = |why: &str| Refused { id: None, error: Error::invalid(why) };
    let Value::Object(mut fields) = value else {
        return Err(whole("the message is not an object"));
    };
    if fields.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(whole("the message does not say jsonrpc 2.0"));
    }
    let id = match fields.get("id") {
        None => None,
        Some(given) => match Id::of(given) {
            Some(id) => Some(id),
            None => return Err(whole("the ID is neither a string nor a whole number")),
        },
    };
    let Some(Value::String(method)) = fields.remove("method") else {
        let why = if fields.contains_key("result") || fields.contains_key("error") {
            "this server asks its clients nothing, so it takes no answers"
        } else {
            "the message names no method"
        };
        return Err(Refused { id: None, error: Error::invalid(why) });
    };
    let params = match fields.remove("params") {
        None => Some(Map::new()),
        Some(Value::Object(params)) => Some(params),
        Some(_) => None,
    };
    match (id, params) {
        (Some(id), Some(params)) => Ok(Message::Request { id, method, params }),
        (Some(id), None) => {
            Err(Refused { id: Some(id), error: Error::params("the parameters are not an object") })
        }
        (None, params) => Ok(Message::Notification { method, params: params.unwrap_or_default() }),
    }
}

/// The answer to the request `id`: its result, or why there is none.
/// With no ID, the answer to bytes no request could be read from.
pub fn answer(id: Option<&Id>, outcome: Result<Value, Error>) -> Value {
    let id = id.map_or(Value::Null, Id::json);
    match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error.json() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Result<Message, Refused> {
        match parse(text.as_bytes()) {
            Ok(Incoming::One(message)) => Ok(message),
            Ok(Incoming::Batch(_)) => panic!("{text} was read as a batch"),
            Err(refused) => Err(refused),
        }
    }

    fn refused(text: &str) -> Refused {
        one(text).expect_err(text)
    }

    #[test]
    fn a_request_is_read_with_its_id_method_and_parameters() {
        let read =
            one(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"areas"}}"#);
        let Ok(Message::Request { id, method, params }) = read else {
            panic!("not a request: {read:?}");
        };
        assert_eq!(id.json(), json!(7));
        assert_eq!(method, "tools/call");
        assert_eq!(Value::Object(params), json!({ "name": "areas" }));
    }

    #[test]
    fn an_id_is_a_string_or_a_whole_number() {
        for (given, taken) in [
            (r#""a-1""#, json!("a-1")),
            ("0", json!(0)),
            ("-3", json!(-3)),
            ("18446744073709551615", json!(18_446_744_073_709_551_615_u64)),
            (r#""""#, json!("")),
        ] {
            let text = format!(r#"{{"jsonrpc":"2.0","id":{given},"method":"ping"}}"#);
            let Ok(Message::Request { id, .. }) = one(&text) else {
                panic!("{given} was not taken as an ID");
            };
            assert_eq!(id.json(), taken);
        }
        for given in ["null", "1.5", "true", "[1]", r#"{"a":1}"#] {
            let text = format!(r#"{{"jsonrpc":"2.0","id":{given},"method":"ping"}}"#);
            let refused = refused(&text);
            assert_eq!((refused.id, refused.error.code), (None, INVALID), "{given}");
        }
    }

    #[test]
    fn no_parameters_are_an_empty_object() {
        let read = one(r#"{"jsonrpc":"2.0","id":"x","method":"ping"}"#);
        assert_eq!(
            read,
            Ok(Message::Request {
                id: Id::Text("x".into()),
                method: "ping".into(),
                params: Map::new()
            })
        );
    }

    #[test]
    fn a_request_whose_parameters_are_a_list_is_refused_under_its_id() {
        let refused =
            refused(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":["areas"]}"#);
        assert_eq!(refused.id.map(|id| id.json()), Some(json!(4)));
        assert_eq!((refused.error.code, refused.error.status), (PARAMS, 200));
    }

    #[test]
    fn a_message_without_an_id_is_a_notification() {
        let read = one(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert_eq!(
            read,
            Ok(Message::Notification {
                method: "notifications/initialized".into(),
                params: Map::new()
            })
        );
        // One cannot be refused, so parameters that are no object are none.
        let read = one(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":[1]}"#);
        assert_eq!(
            read,
            Ok(Message::Notification {
                method: "notifications/cancelled".into(),
                params: Map::new()
            })
        );
    }

    #[test]
    fn what_is_not_a_message_is_refused_with_no_id() {
        for (text, code) in [
            ("{", PARSE),
            ("", PARSE),
            ("\u{feff}{}", PARSE),
            ("7", INVALID),
            (r#""tools/list""#, INVALID),
            ("null", INVALID),
            ("{}", INVALID),
            (r#"{"id":1,"method":"ping"}"#, INVALID),
            (r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#, INVALID),
            (r#"{"jsonrpc":2.0,"id":1,"method":"ping"}"#, INVALID),
            (r#"{"jsonrpc":"2.0","id":1}"#, INVALID),
            (r#"{"jsonrpc":"2.0","id":1,"method":7}"#, INVALID),
            ("[]", INVALID),
        ] {
            let refused = match parse(text.as_bytes()) {
                Err(refused) => refused,
                Ok(read) => panic!("{text:?} was read as {read:?}"),
            };
            assert_eq!((refused.id, refused.error.code), (None, code), "{text:?}");
            assert_eq!(refused.error.status, 400, "{text:?}");
        }
    }

    #[test]
    fn an_answer_is_not_taken() {
        for text in [
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"no"}}"#,
        ] {
            let refused = refused(text);
            assert_eq!((refused.id, refused.error.code), (None, INVALID));
            assert!(
                refused.error.message.contains("takes no answers"),
                "{}",
                refused.error.message
            );
        }
    }

    #[test]
    fn a_message_has_a_longest_length() {
        let method = "m".repeat(MESSAGE_MAX);
        let text = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#);
        let refused = refused(&text);
        assert_eq!((refused.id, refused.error.code), (None, INVALID));
        assert!(refused.error.message.contains("65536 bytes"), "{}", refused.error.message);

        let most = MESSAGE_MAX - r#"{"jsonrpc":"2.0","id":1,"method":""}"#.len();
        let text = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{}"}}"#, "m".repeat(most));
        assert_eq!(text.len(), MESSAGE_MAX);
        assert!(one(&text).is_ok());
    }

    #[test]
    fn json_nested_too_deep_is_not_json() {
        let text = format!("{}{}", "[".repeat(4096), "]".repeat(4096));
        let refused = match parse(text.as_bytes()) {
            Err(refused) => refused,
            Ok(read) => panic!("read as {read:?}"),
        };
        assert_eq!(refused.error.code, PARSE);
    }

    #[test]
    fn a_batch_s_messages_are_read_each_by_itself() {
        let text = r#"[
            {"jsonrpc":"2.0","id":1,"method":"ping"},
            {"jsonrpc":"2.0","method":"notifications/initialized"},
            {"jsonrpc":"2.0","id":2,"method":"tools/list","params":[]},
            7
        ]"#;
        let Ok(Incoming::Batch(messages)) = parse(text.as_bytes()) else {
            panic!("not a batch");
        };
        assert_eq!(messages.len(), 4);
        assert!(matches!(&messages[0], Ok(Message::Request { method, .. }) if method == "ping"));
        assert!(matches!(&messages[1], Ok(Message::Notification { .. })));
        assert!(
            matches!(&messages[2], Err(Refused { id: Some(Id::Number(_)), error }) if error.code == PARAMS)
        );
        assert!(matches!(&messages[3], Err(Refused { id: None, error }) if error.code == INVALID));
    }

    #[test]
    fn a_batch_has_a_most() {
        let one = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let batch = |count: usize| format!("[{}]", vec![one; count].join(","));
        assert!(
            matches!(parse(batch(BATCH_MAX).as_bytes()), Ok(Incoming::Batch(all)) if all.len() == BATCH_MAX)
        );
        let refused = match parse(batch(BATCH_MAX + 1).as_bytes()) {
            Err(refused) => refused,
            Ok(read) => panic!("read as {read:?}"),
        };
        assert_eq!((refused.id, refused.error.code), (None, INVALID));
    }

    #[test]
    fn an_answer_carries_the_id_back() {
        assert_eq!(
            answer(Some(&Id::Text("a".into())), Ok(json!({ "tools": [] }))),
            json!({ "jsonrpc": "2.0", "id": "a", "result": { "tools": [] } })
        );
        assert_eq!(
            answer(None, Err(Error::parse())),
            json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": "the message is not JSON" },
            })
        );
    }

    #[test]
    fn an_error_shows_a_client_s_word_safely() {
        let error = Error::no_method("tools/\u{1b}[31mcall\n");
        assert_eq!(error.message, "there is no method \"tools/?[31mcall?\"");
        let long = Error::no_method(&"x".repeat(1000));
        assert_eq!(long.message.len(), "there is no method \"\"".len() + 64);

        let error = Error::unsupported_version("2027-01-01", &["2026-07-28", "2025-11-25"]);
        assert_eq!((error.code, error.status), (UNSUPPORTED_VERSION, 400));
        assert_eq!(
            error.data,
            Some(json!({ "supported": ["2026-07-28", "2025-11-25"], "requested": "2027-01-01" }))
        );
        assert_eq!(Error::no_method("x").with_status(404).status, 404);
    }
}
