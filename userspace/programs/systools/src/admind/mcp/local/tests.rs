// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The socket with a test authd, and the test's own process as the
//! account that connects: the fixture's `chase` is given the number the
//! tests run as, so a pass of his is his own and one of alice's is not.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::*;
use crate::admind::actions::tests::LAB;
use crate::admind::fixture::{Asked, Console as World, Script, run};

/// chase's pass, alice's, one authd takes twice and then no more, and
/// a token of chase's client that connects over HTTPS.
const PASS: &str = "dkp_chase_good";
const PASS_OF_ALICE: &str = "dkp_alice_good";
const PASS_THAT_ENDS: &str = "dkp_chase_ends";
const TOKEN: &str = "dkc_chase_web";

/// An authd that knows those.
fn script() -> Arc<Script> {
    let ending = AtomicUsize::new(0);
    Arc::new(move |asked: &Asked, _| {
        let hex = |text: &str| accounts::request::hex(text.as_bytes());
        let shown = asked.text(0);
        let answer = match asked.verb.as_str() {
            "client" if shown == hex(PASS) => "ok chase shell ssh * 1900000000",
            "client" if shown == hex(PASS_OF_ALICE) => "ok alice shell ssh * 1900000000",
            "client" if shown == hex(TOKEN) => "ok chase web https * 1900000000",
            "client" if shown == hex(PASS_THAT_ENDS) => {
                if ending.fetch_add(1, Ordering::SeqCst) < 2 {
                    "ok chase brief ssh * 1900000000"
                } else {
                    "no that AI client's pass has ended"
                }
            }
            _ => "no that AI client's pass is not known",
        };
        vec![answer.into()]
    })
}

/// The scratch folder of `world`, where its accounts' file is.
fn folder(world: &World) -> PathBuf {
    Path::new(&world.shared.accounts.passwd).parent().expect("a folder").to_path_buf()
}

/// A console that serves the socket in its scratch folder.
struct Served {
    world: World,
    path: PathBuf,
}

/// Starts a console named for `what` whose `chase` is the account the
/// tests run as, and has it accept at a socket of its own.
fn served(what: &str) -> Served {
    use std::os::unix::fs::MetadataExt;
    let world = World::new(what, script());
    world.shared.actions.set(LAB);
    let path = folder(&world).join("mcp").join("socket");
    let socket = listen(&path).expect("a socket");
    // Who the tests run as owns what they make.
    let user = std::fs::metadata(&path).expect("the socket").uid();
    let accounts = format!(
        "chase:x:{user}:{user}:Chase Y,,:/users/chase:/system/commands/nukesh\n\
         alice:x:{other}:{other}:Alice,,:/users/alice:/system/commands/nukesh\n",
        other = user.wrapping_add(1),
    );
    std::fs::write(&world.shared.accounts.passwd, accounts).expect("the accounts");
    tokio::task::spawn_local(accepting(Rc::clone(&world.shared), socket));
    Served { world, path }
}

impl Served {
    /// What authd was asked about, as how many credentials.
    fn asked(&self) -> usize {
        self.world.authd.lines().len()
    }
}

/// A program that connected.
struct Peer {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Peer {
    async fn connect(served: &Served) -> Peer {
        let stream = UnixStream::connect(&served.path).await.expect("a connection");
        let (reading, writer) = stream.into_split();
        Peer { reader: BufReader::new(reading), writer }
    }

    /// Connects and gives `pass`: the one line it is answered.
    async fn greeted(served: &Served, pass: &str) -> (Peer, String) {
        let mut peer = Peer::connect(served).await;
        peer.say(pass).await;
        let answer = peer.hear().await.expect("one line");
        (peer, answer)
    }

    /// Connects with chase's own pass, which is taken.
    async fn of_chase(served: &Served) -> Peer {
        let (peer, answer) = Peer::greeted(served, PASS).await;
        assert_eq!(answer, "ok chase shell");
        peer
    }

    async fn say(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).await.expect("written");
        self.writer.write_all(b"\n").await.expect("written");
    }

    /// The next line, without its end; `None` when the server closed.
    async fn hear(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(10), self.reader.read_line(&mut line))
            .await
            .expect("a line or the end within 10 seconds")
            .expect("read");
        (read > 0).then(|| line.trim_end_matches('\n').to_string())
    }

    /// The next line, as JSON.
    async fn answer(&mut self) -> Value {
        let line = self.hear().await.expect("an answer");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line}"))
    }
}

fn ping(id: u64) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }).to_string()
}

#[test]
fn the_socket_is_for_any_account_to_connect_to_and_replaces_one_left_behind() {
    use std::os::unix::fs::PermissionsExt;
    run(async {
        let world = World::new("mcp-listen", script());
        let path = folder(&world).join("live").join("mcp");
        let mode = |path: &Path| std::fs::metadata(path).expect("it is there").permissions().mode();
        let first = listen(&path).expect("a socket");
        assert_eq!(mode(&path) & 0o777, 0o666);
        assert_eq!(mode(path.parent().unwrap()) & 0o777, 0o755);
        // An admind that did not stop in order left its socket.
        drop(first);
        assert!(path.exists());
        let second = listen(&path).expect("a socket again");
        assert_eq!(mode(&path) & 0o777, 0o666);
        drop(second);
        // Where no folder can be made, it says so.
        let file = folder(&world).join("file");
        std::fs::write(&file, "").unwrap();
        assert!(listen(&file.join("mcp")).is_err());
    });
}

#[test]
fn a_pass_from_its_owner_s_account_is_greeted_and_its_messages_answered_a_line_each() {
    run(async {
        let served = served("mcp-socket");
        let mut peer = Peer::of_chase(&served).await;
        peer.say(&ping(1)).await;
        assert_eq!(peer.answer().await, json!({ "jsonrpc": "2.0", "id": 1, "result": {} }));
        // A notification has no answer, and an empty line is no message:
        // the next line heard is the next request's.
        peer.say(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string())
            .await;
        peer.say("").await;
        peer.say("   ").await;
        peer.say(&ping(2)).await;
        assert_eq!(peer.answer().await["id"], 2);
        // authd was asked about the pass as it came and with each message,
        // and never told an address.
        let lines = served.world.authd.lines();
        let line = format!("client {}", accounts::request::hex(PASS.as_bytes()));
        assert_eq!(lines, vec![line; 4]);

        // A tool answers as the client that the pass names.
        let call = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "areas", "arguments": {} },
        });
        peer.say(&call.to_string()).await;
        let areas = peer.answer().await;
        assert_eq!(areas["result"]["isError"], false, "{areas}");
        assert_eq!(areas["result"]["structuredContent"]["areas"][0]["area"], "services");
        // What is no JSON is answered so, and the connection stays.
        peer.say("{\"jsonrpc\":").await;
        let broken = peer.answer().await;
        assert_eq!((&broken["error"]["code"], &broken["id"]), (&json!(-32700), &Value::Null));
        peer.say(&ping(4)).await;
        assert_eq!(peer.answer().await["id"], 4);
    });
}

#[test]
fn what_is_no_pass_of_the_account_that_connected_is_refused_in_one_line() {
    run(async {
        let served = served("mcp-socket-refused");
        let not_a_pass = "no ADM-143: the first line is the AI client's pass, dkp_…";
        // A token, a pass with a space in it, one that is far too long:
        // authd is not asked.
        let long = format!("dkp_chase_{}", "a".repeat(TOKEN_MAX));
        for wrong in [TOKEN, "dkp_chase good", "dkp_chase_\u{7}", long.as_str(), ""] {
            let (mut peer, answer) = Peer::greeted(&served, wrong).await;
            assert_eq!(answer, not_a_pass, "{wrong:?}");
            assert_eq!(peer.hear().await, None, "{wrong:?}");
        }
        assert_eq!(served.asked(), 0);
        // One that authd does not know.
        let (mut peer, answer) = Peer::greeted(&served, "dkp_chase_none").await;
        assert_eq!(answer, "no ADM-143: that AI client's pass is not known");
        assert_eq!(peer.hear().await, None);
        // Another account's, which authd would take from her.
        let (mut peer, answer) = Peer::greeted(&served, PASS_OF_ALICE).await;
        assert_eq!(
            answer,
            "no ADM-143: this pass is alice's, and is taken from that account alone"
        );
        assert_eq!(peer.hear().await, None);
        // A connection that goes without a line is told nothing it reads.
        drop(Peer::connect(&served).await);
        // A message before the pass is no pass.
        let (_, answer) = Peer::greeted(&served, &ping(1)).await;
        assert_eq!(answer, not_a_pass);
    });
}

#[test]
fn ten_passes_that_are_not_taken_make_the_account_wait_out_the_minute() {
    let open = Open::default();
    let start = Instant::now();
    let (chase, alice) = (1000, 1002);
    for _ in 0..TRIES_MAX {
        assert_eq!(open.waits(chase, start), None);
        open.wrong(chase, start);
    }
    assert_eq!(open.waits(chase, start), Some(61));
    assert_eq!(open.waits(chase, start + Duration::from_secs(45)), Some(16));
    // Another account brought none.
    assert_eq!(open.waits(alice, start), None);
    // The minute over, the account is forgotten and counts from one.
    assert_eq!(open.waits(chase, start + TRIES_WINDOW), None);
    assert!(open.wrong.borrow().is_empty());
    open.wrong(chase, start + TRIES_WINDOW);
    assert_eq!(open.waits(chase, start + TRIES_WINDOW), None);
    // One that came after its minute begins the next.
    open.wrong(alice, start);
    open.wrong(alice, start + TRIES_WINDOW * 2);
    assert_eq!(open.wrong.borrow()[&alice].1, 1);
}

#[test]
fn an_account_that_waits_is_refused_before_authd_is_asked_about_a_good_pass() {
    run(async {
        let served = served("mcp-socket-wrong");
        for _ in 0..TRIES_MAX {
            let (_, answer) = Peer::greeted(&served, "dkp_chase_none").await;
            assert!(answer.starts_with("no ADM-143: "), "{answer}");
        }
        let asked = served.asked();
        // Told as it connects, before it gives a pass.
        let mut peer = Peer::connect(&served).await;
        let answer = peer.hear().await.expect("one line");
        assert!(answer.starts_with("no ADM-105: this account brought too many"), "{answer}");
        assert_eq!(peer.hear().await, None);
        assert_eq!(served.asked(), asked);
    });
}

#[test]
fn a_line_that_is_too_long_is_answered_and_ends_the_connection() {
    run(async {
        let served = served("mcp-socket-long");
        let mut peer = Peer::of_chase(&served).await;
        // The longest line there is, to the byte, is a message.
        let room = MESSAGE_MAX - ping(1).len() - ",\"pad\":\"\"".len();
        let padded = ping(1).replacen('}', &format!(",\"pad\":\"{}\"}}", "x".repeat(room)), 1);
        assert_eq!(padded.len(), MESSAGE_MAX);
        peer.say(&padded).await;
        assert_eq!(peer.answer().await["id"], 1);
        // One byte more is not, and what follows it is not read.
        peer.say(&format!("{padded} ")).await;
        let refused = peer.answer().await;
        assert_eq!((&refused["error"]["code"], &refused["id"]), (&json!(-32600), &Value::Null));
        assert!(refused["error"]["message"].as_str().unwrap().contains("65536 bytes"), "{refused}");
        assert_eq!(peer.hear().await, None);
    });
}

#[test]
fn a_pass_that_authd_takes_no_longer_ends_the_connection_at_the_next_message() {
    run(async {
        let served = served("mcp-socket-ends");
        let (mut peer, answer) = Peer::greeted(&served, PASS_THAT_ENDS).await;
        assert_eq!(answer, "ok chase brief");
        peer.say(&ping(1)).await;
        assert_eq!(peer.answer().await["id"], 1);
        // The third time authd is asked, the pass has ended: the message
        // is not answered, and the program is to bring a new pass.
        peer.say(&ping(2)).await;
        assert_eq!(peer.hear().await, None);
        assert_eq!(served.asked(), 3);
    });
}

#[test]
fn connections_have_places_by_account_and_in_all() {
    let open = Rc::new(Open::default());
    let mut chase: Vec<Place> =
        (0..CONNECTIONS_EACH).map(|_| Place::take(&open, 1000).expect("a place")).collect();
    assert!(Place::take(&open, 1000).is_none());
    // Another account has its own, until all are taken.
    let mut others: Vec<Place> = Vec::new();
    for user in 2000.. {
        match Place::take(&open, user) {
            Some(place) => others.push(place),
            None => break,
        }
    }
    assert_eq!(chase.len() + others.len(), CONNECTIONS_MAX);
    // A connection that goes gives its place back: to its own account
    // while the others are full, and to anyone.
    chase.pop();
    assert!(Place::take(&open, 3000).is_some());
    others.clear();
    chase.clear();
    assert!(open.connections.borrow().is_empty());
}

#[test]
fn an_account_with_as_many_connections_as_it_may_have_is_told_to_wait() {
    run(async {
        let served = served("mcp-socket-full");
        let mut peers = Vec::new();
        for _ in 0..CONNECTIONS_EACH {
            peers.push(Peer::of_chase(&served).await);
        }
        let mut one_more = Peer::connect(&served).await;
        let answer = one_more.hear().await.expect("one line");
        assert_eq!(answer, "no ADM-105: as many AI clients are connected as may be");
        assert_eq!(one_more.hear().await, None);
        // One that goes makes room, once the server has seen it go.
        peers.pop();
        for _ in 0..500 {
            let mut peer = Peer::connect(&served).await;
            peer.say(PASS).await;
            if peer.hear().await.as_deref() == Some("ok chase shell") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no place came free");
    });
}
