// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The bridge between socket pairs: the test is the client at one end
//! and the console at the other, and the bridge runs in a thread of its
//! own as it runs in `admin mcp`.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;

/// How long a test waits for a line before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

fn whole(text: &str) -> Line {
    Line::Whole(text.as_bytes().to_vec())
}

fn number(id: u64) -> Id {
    Id::of(&json!(id)).expect("an ID")
}

fn ping(id: u64) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }).to_string()
}

fn pong(id: u64) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string()
}

#[test]
fn bytes_that_come_in_pieces_are_given_out_a_line_at_a_time() {
    let mut lines = Lines::new(8);
    assert_eq!(lines.take(b"one\ntwo\nthr"), [whole("one"), whole("two")]);
    assert_eq!(lines.take(b""), []);
    assert_eq!(lines.take(b"ee\n\nfour"), [whole("three"), whole("")]);
    // As long as a line may be, and one byte longer.
    assert_eq!(
        lines.take(b"four\n12345678\n123456789\nok\n"),
        [whole("fourfour"), whole("12345678"), Line::TooLong, whole("ok")]
    );
    // Too long over several pieces: said once, at its end, and the line
    // after it is whole.
    assert_eq!(lines.take(b"12345"), []);
    assert_eq!(lines.take(b"6789"), []);
    assert_eq!(lines.take(b"0123456789"), []);
    assert_eq!(lines.take(b"x\nnext\n"), [Line::TooLong, whole("next")]);
}

#[test]
fn the_requests_on_their_way_are_those_the_console_answers_by_their_ids() {
    let ids = |line: &str| asked(line.as_bytes());
    assert_eq!(ids(&ping(7)), [number(7)]);
    assert_eq!(
        ids(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#),
        [Id::of(&json!("a")).unwrap()]
    );
    // A notification has no answer.
    assert_eq!(ids(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), []);
    // A request whose parameters are no object is refused under its ID.
    assert_eq!(ids(r#"{"jsonrpc":"2.0","id":3,"method":"ping","params":[]}"#), [number(3)]);
    // What is no message is answered with no ID.
    for unread in ["", "{", "7", r#"{"id":4,"method":"ping"}"#, r#"{"jsonrpc":"2.0","id":4}"#] {
        assert_eq!(ids(unread), [], "{unread}");
    }
    // A batch's requests, but not when the batch is refused whole.
    let batch = format!(
        r#"[{},{{"jsonrpc":"2.0","method":"x"}},{},{{"jsonrpc":"2.0","id":9,"method":"x","params":1}}]"#,
        ping(1),
        ping(2)
    );
    assert_eq!(ids(&batch), [number(1), number(2), number(9)]);
    assert_eq!(ids(&format!("[{},7]", ping(1))), []);
    assert_eq!(ids("[]"), []);
    let named = r#"[{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}]"#;
    assert_eq!(ids(named), []);
}

#[test]
fn an_answer_s_ids_are_its_own_or_its_batch_s() {
    let ids = |line: &str| answered(line.as_bytes());
    assert_eq!(ids(&pong(7)), [number(7)]);
    assert_eq!(ids(&format!("[{},{}]", pong(1), pong(2))), [number(1), number(2)]);
    for none in [r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"x"}}"#, "{", ""] {
        assert_eq!(ids(none), [], "{none}");
    }
}

#[test]
fn a_request_is_on_its_way_until_it_is_answered_and_the_oldest_are_forgotten() {
    let mut flight = Flight::default();
    flight.sent(vec![number(1), number(2), number(1)]);
    flight.answered(pong(1).as_bytes());
    // An ID a client used twice is on its way once more.
    assert_eq!(flight.0, [number(2), number(1)]);
    // An answer to what was never asked changes nothing.
    flight.answered(pong(9).as_bytes());
    assert_eq!(flight.0.len(), 2);
    let failed = flight.failed("the console closed the connection");
    assert!(flight.0.is_empty());
    let failed: Vec<Value> =
        failed.iter().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(failed.len(), 2);
    assert_eq!((&failed[0]["id"], &failed[1]["id"]), (&json!(2), &json!(1)));
    assert_eq!(failed[0]["error"]["code"], rpc::INTERNAL);
    assert_eq!(failed[0]["error"]["message"], "the console closed the connection");
    assert!(failed[0].get("result").is_none());

    let many = u64::try_from(FLIGHT_MAX).unwrap() + 3;
    flight.sent((0..many).map(number).collect());
    assert_eq!(flight.0.len(), FLIGHT_MAX);
    assert_eq!(flight.0.front(), Some(&number(3)));
}

/// The far end of a socket pair, read a line at a time.
struct End {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl End {
    fn of(stream: UnixStream) -> End {
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        End { reader: BufReader::new(stream.try_clone().unwrap()), writer: stream }
    }

    fn say(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
    }

    /// The next line; `None` when the other end closed.
    fn hear(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("a line or the end in time");
        (read > 0).then(|| line.trim_end_matches('\n').to_string())
    }

    fn answer(&mut self) -> Value {
        let line = self.hear().expect("an answer");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line}"))
    }
}

/// A bridge that runs, with the test at both of its ends.
struct Running {
    /// The MCP client's end.
    client: End,
    /// The console's end of each connection the bridge makes, as it
    /// makes it.
    consoles: Receiver<End>,
    /// What the next connection is refused with, when it is to be.
    refusal: Arc<Mutex<Option<String>>>,
    /// The Unix time the bridge is told.
    now: Arc<AtomicI64>,
    bridge: JoinHandle<io::Result<()>>,
}

/// The Unix time each pass of the test ends.
const UNTIL: i64 = 1_800_000_000;

/// Starts a bridge that has no connection yet.
fn running() -> Running {
    let (input, client_writes) = UnixStream::pair().unwrap();
    let (out, client_reads) = UnixStream::pair().unwrap();
    client_reads.set_read_timeout(Some(PATIENCE)).unwrap();
    let client = End { reader: BufReader::new(client_reads), writer: client_writes };
    let (made, consoles) = mpsc::channel();
    let refusal = Arc::new(Mutex::new(None::<String>));
    let now = Arc::new(AtomicI64::new(UNTIL - 3600));
    let (refused, clock) = (Arc::clone(&refusal), Arc::clone(&now));
    let bridge = thread::spawn(move || {
        let mut connect = || {
            if let Some(why) = refused.lock().unwrap().take() {
                return Err(why);
            }
            let (ours, theirs) = UnixStream::pair().unwrap();
            ours.set_nonblocking(true).unwrap();
            made.send(End::of(theirs)).unwrap();
            Ok(Link { stream: ours, until: UNTIL })
        };
        let clock = || clock.load(Ordering::SeqCst);
        carry(input.as_raw_fd(), out, None, &mut connect, &clock)
    });
    Running { client, consoles, refusal, now, bridge }
}

impl Running {
    /// The console's end of the connection the bridge made next.
    fn console(&self) -> End {
        self.consoles.recv_timeout(PATIENCE).expect("a connection")
    }

    /// Ends the client, and waits for the bridge to end with it.
    fn end(self) {
        drop(self.client);
        self.bridge.join().expect("the bridge").expect("ended in order");
    }
}

#[test]
fn messages_go_to_the_console_and_answers_come_back_a_line_each() {
    let mut running = running();
    // Nothing is connected until there is something to carry.
    assert!(running.consoles.try_recv().is_err());
    running.client.say(&ping(1));
    let mut console = running.console();
    assert_eq!(console.hear().as_deref(), Some(ping(1).as_str()));
    console.say(&pong(1));
    assert_eq!(running.client.answer(), json!({ "jsonrpc": "2.0", "id": 1, "result": {} }));
    // Empty lines are not messages; a notification is carried, and what
    // the console says of its own accord too. The connection is kept.
    running.client.say("");
    running.client.say("  ");
    let ready = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    running.client.say(ready);
    assert_eq!(console.hear().as_deref(), Some(ready));
    console.say(&pong(5));
    console.say(&pong(4));
    assert_eq!(running.client.answer()["id"], 5);
    assert_eq!(running.client.answer()["id"], 4);
    assert!(running.consoles.try_recv().is_err());
    // The client ends: so does the bridge, and its connection.
    running.end();
    assert_eq!(console.hear(), None);
}

#[test]
fn a_line_that_is_too_long_is_answered_here_and_the_console_is_not_told() {
    let mut running = running();
    running.client.say(&"x".repeat(MESSAGE_MAX + 1));
    let refused = running.client.answer();
    assert_eq!((&refused["id"], &refused["error"]["code"]), (&Value::Null, &json!(rpc::INVALID)));
    assert!(refused["error"]["message"].as_str().unwrap().contains("65536 bytes"), "{refused}");
    assert!(running.consoles.try_recv().is_err());
    // The line after it is a message as any other.
    running.client.say(&ping(2));
    let mut console = running.console();
    assert_eq!(console.hear().as_deref(), Some(ping(2).as_str()));
    running.end();
}

#[test]
fn requests_on_their_way_when_the_console_goes_are_told_and_the_next_connects_again() {
    let mut running = running();
    running.client.say(&ping(1));
    running.client.say(&ping(2));
    running.client.say(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    running.client.say(&ping(3));
    let mut console = running.console();
    for id in 1..=2 {
        assert_eq!(console.hear().as_deref(), Some(ping(id).as_str()));
    }
    // The second is answered; the console then goes, as when admind
    // stops or the pass is taken no longer.
    console.say(&pong(2));
    assert_eq!(running.client.answer()["id"], 2);
    drop(console);
    for id in [1, 3] {
        let failed = running.client.answer();
        assert_eq!((&failed["id"], &failed["error"]["code"]), (&json!(id), &json!(rpc::INTERNAL)));
        let words = failed["error"]["message"].as_str().unwrap();
        assert!(words.contains("the console") && words.ends_with("so ask again"), "{words}");
    }
    // Nothing is sent again: the new connection hears only what is new.
    running.client.say(&ping(4));
    let mut console = running.console();
    assert_eq!(console.hear().as_deref(), Some(ping(4).as_str()));
    console.say(&pong(4));
    assert_eq!(running.client.answer()["id"], 4);
    running.end();
}

#[test]
fn a_request_that_cannot_connect_is_answered_why_and_the_next_tries_again() {
    let mut running = running();
    let why = "chase has no AI client named \"desk\" that connects over SSH and has not ended";
    *running.refusal.lock().unwrap() = Some(why.into());
    running.client.say(&ping(1));
    let failed = running.client.answer();
    assert_eq!((&failed["id"], &failed["error"]["code"]), (&json!(1), &json!(rpc::INTERNAL)));
    assert_eq!(failed["error"]["message"], format!("cannot connect to the console: {why}"));
    // A notification that cannot be carried has no answer to say so in.
    *running.refusal.lock().unwrap() = Some(why.into());
    running.client.say(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    running.client.say(&ping(2));
    let mut console = running.console();
    assert_eq!(console.hear().as_deref(), Some(ping(2).as_str()));
    console.say(&pong(2));
    assert_eq!(running.client.answer()["id"], 2);
    running.end();
}

#[test]
fn a_pass_that_ends_within_the_minute_is_replaced_while_nothing_is_on_its_way() {
    let mut running = running();
    running.client.say(&ping(1));
    let mut first = running.console();
    assert_eq!(first.hear().as_deref(), Some(ping(1).as_str()));
    // Nearly over, with a request on its way: the connection is kept.
    running.now.store(UNTIL - RENEW_BEFORE + 1, Ordering::SeqCst);
    running.client.say(&ping(2));
    assert_eq!(first.hear().as_deref(), Some(ping(2).as_str()));
    first.say(&pong(1));
    first.say(&pong(2));
    assert_eq!(running.client.answer()["id"], 1);
    assert_eq!(running.client.answer()["id"], 2);
    // Nothing on its way: the next message goes by a new connection,
    // and the old one is closed with nothing lost.
    running.client.say(&ping(3));
    let mut second = running.console();
    assert_eq!(second.hear().as_deref(), Some(ping(3).as_str()));
    assert_eq!(first.hear(), None);
    second.say(&pong(3));
    assert_eq!(running.client.answer()["id"], 3);
    // A pass with an hour left is kept.
    running.now.store(UNTIL - 3600, Ordering::SeqCst);
    running.client.say(&ping(4));
    assert_eq!(second.hear().as_deref(), Some(ping(4).as_str()));
    assert!(running.consoles.try_recv().is_err());
    running.end();
}

#[test]
fn the_console_s_answers_are_carried_while_it_does_not_read_what_is_sent_to_it() {
    let mut running = running();
    running.client.say(&ping(0));
    let mut console = running.console();
    assert_eq!(console.hear().as_deref(), Some(ping(0).as_str()));
    // More than a socket holds, which the console does not read yet.
    let pad = "x".repeat(60_000);
    let big = |id: u64| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "ping", "params": { "pad": pad } })
            .to_string()
    };
    let sent: Vec<String> = (1..=8).map(big).collect();
    let mut writer = running.client.writer.try_clone().unwrap();
    let lines = sent.clone();
    let writing = thread::spawn(move || {
        for line in lines {
            writer.write_all(line.as_bytes()).unwrap();
            writer.write_all(b"\n").unwrap();
        }
    });
    // An answer comes back all the same.
    console.say(&pong(0));
    assert_eq!(running.client.answer()["id"], 0);
    // And once the console reads, every message arrives whole, in order.
    for line in &sent {
        assert_eq!(console.hear().as_deref(), Some(line.as_str()));
    }
    writing.join().unwrap();
    running.end();
}

#[test]
fn an_answer_longer_than_any_the_console_gives_ends_the_connection() {
    let mut running = running();
    running.client.say(&ping(1));
    let mut console = running.console();
    assert_eq!(console.hear().as_deref(), Some(ping(1).as_str()));
    let chunk = vec![b'x'; 1024 * 1024];
    let mut writer = console.writer.try_clone().unwrap();
    let flooding = thread::spawn(move || {
        // The bridge lets go part of the way through: the rest fails.
        for _ in 0..=ANSWER_LINE_MAX / chunk.len() {
            if writer.write_all(&chunk).is_err() {
                return;
            }
        }
        let _ = writer.write_all(b"\n");
    });
    let failed = running.client.answer();
    assert_eq!((&failed["id"], &failed["error"]["code"]), (&json!(1), &json!(rpc::INTERNAL)));
    assert!(failed["error"]["message"].as_str().unwrap().contains("too long"), "{failed}");
    flooding.join().unwrap();
    running.end();
}
