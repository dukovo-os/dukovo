// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The terminals' registry, on pseudo-terminals of the host with no shell
//! on them: who sees which pane, what a socket is sent and when, how a
//! flood is held back, typing, and how panes and terminals end.

use super::*;

const ALICE: &str = "alice";
const BOB: &str = "bob";
/// Alice's two console sessions, as authd names them.
const HERS: &str = "s-alice-1";
const HER_OTHER: &str = "s-alice-2";

/// A pane of terminal `terminal` on a new pseudo-terminal.
fn pane(id: u32, terminal: u32, admin: Option<&str>) -> Pane {
    let (master, _terminal) = pty::open(80, 24).unwrap();
    let shell = Shell { master, process: 4242, tty: "pts/9".into() };
    Pane::new(id, terminal, admin.map(str::to_string), shell, 80, 24, 1000)
}

/// A terminal of `account`'s, with no panes yet.
fn terminal(id: u32, account: &str) -> Terminal {
    Terminal {
        id,
        account: account.into(),
        name: format!("{account}@nas"),
        made: 1000,
        used: 1000,
        from: "192.0.2.7".parse().unwrap(),
        keep_days: KEEP_DAYS,
        layout: Layout::One,
        sizes: Vec::new(),
        panes: Vec::new(),
    }
}

/// Terminals with Alice's terminal 1, whose pane 1 is hers, and Bob's
/// terminal 2, pane 2.
fn world() -> Terminals {
    let mut terminals = Terminals::default();
    terminals.add_terminal(terminal(1, ALICE), pane(1, 1, None));
    terminals.add_terminal(terminal(2, BOB), pane(2, 2, None));
    terminals
}

/// What socket `id` would write now: each frame's opcode and body.
fn sent(terminals: &mut Terminals, id: u64) -> Vec<(u8, Vec<u8>)> {
    let mut frames = Vec::new();
    while let Next::Frame(frame) = terminals.next_frame(id) {
        let opcode = frame[0] & 0x0f;
        let (length, start) = match frame[1] {
            126 => (usize::from(u16::from_be_bytes([frame[2], frame[3]])), 4),
            127 => {
                (usize::try_from(u64::from_be_bytes(frame[2..10].try_into().unwrap())).unwrap(), 10)
            }
            short => (usize::from(short), 2),
        };
        assert_eq!(frame.len(), start + length);
        frames.push((opcode, frame[start..].to_vec()));
    }
    frames
}

/// The text messages among `frames`, as JSON.
fn texts(frames: &[(u8, Vec<u8>)]) -> Vec<Value> {
    frames
        .iter()
        .filter(|(opcode, _)| *opcode == 1)
        .map(|(_, body)| serde_json::from_slice(body).unwrap())
        .collect()
}

/// The output among `frames`: each one's pane, offset and bytes.
fn outputs(frames: &[(u8, Vec<u8>)]) -> Vec<(u32, u64, Vec<u8>)> {
    frames
        .iter()
        .filter(|(opcode, _)| *opcode == 2)
        .map(|(_, body)| {
            let pane = u32::from_be_bytes(body[..4].try_into().unwrap());
            let offset = u64::from_be_bytes(body[4..12].try_into().unwrap());
            (pane, offset, body[12..].to_vec())
        })
        .collect()
}

fn now() -> (Instant, i64) {
    (Instant::now(), 2000)
}

#[test]
fn a_pane_is_shown_to_its_account_alone_and_an_administrators_to_its_session() {
    let mut terminals = world();
    terminals.add_pane(pane(3, 1, Some(HERS)));
    let hers = terminals.open_socket(ALICE, HERS).unwrap();
    let her_other = terminals.open_socket(ALICE, HER_OTHER).unwrap();
    let his = terminals.open_socket(BOB, "s-bob").unwrap();
    assert!(terminals.attach(hers, 1, (80, 24), None, now()).is_ok());
    assert!(terminals.attach(hers, 3, (80, 24), None, now()).is_ok());
    assert!(terminals.attach(her_other, 1, (80, 24), None, now()).is_ok());
    // Another session of hers may not show her administrator's pane, nor
    // Bob her panes, nor anyone a pane that is not there: the same error.
    for (socket, pane) in [(her_other, 3), (his, 1), (his, 3), (hers, 2), (hers, 99)] {
        let refused = terminals.attach(socket, pane, (80, 24), None, now()).unwrap_err();
        assert_eq!(refused.kind, Kind::NoTerminal, "socket {socket} pane {pane}");
    }
    // The lists say the same.
    let ids = |list: &Value| -> Vec<Vec<u64>> {
        list["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                t["panes"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect()
            })
            .collect()
    };
    assert_eq!(ids(&terminals.list(ALICE, HERS, 32)), [vec![1, 3]]);
    assert_eq!(ids(&terminals.list(ALICE, HER_OTHER, 32)), [vec![1]]);
    assert_eq!(ids(&terminals.list(BOB, "s-bob", 32)), [vec![2]]);
    let list = terminals.list(ALICE, HERS, 32);
    assert_eq!(list["most"], 32);
    assert_eq!(list["sockets"], json!({"open": 1, "most": 4}));
    let one = &list["terminals"][0];
    assert_eq!(one["name"], "alice@nas");
    assert_eq!(one["from"], "192.0.2.7");
    assert_eq!(one["layout"], "one");
    assert_eq!(one["panes"][1]["admin"], true);
    assert_eq!(one["panes"][0]["attached"], 2);
    assert_eq!(one["panes"][0]["busy"], Value::Null, "no session on a test's terminal");
}

#[test]
fn an_attach_replays_the_ring_from_its_first_whole_line_with_the_modes_first() {
    let mut terminals = world();
    terminals.output(1, b"\x1b[?1049h\x1b[?25lwelcome\n$ ", Instant::now());
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    assert_eq!(terminals.attach(socket, 1, (80, 24), None, now()), Ok(true));
    let frames = sent(&mut terminals, socket);
    let attached = &texts(&frames)[0];
    assert_eq!(attached["attached"], 1);
    assert_eq!(attached["terminal"], 1);
    assert_eq!(attached["name"], "alice@nas");
    assert_eq!(attached["offset"], 0);
    assert_eq!(attached["live"], 24, "what the ring held was written before the attach");
    assert_eq!(attached["replay"], true);
    assert_eq!(attached["modes"], "\x1b[?1049h\x1b[?25l");
    let output = outputs(&frames);
    assert_eq!(output, [(1, 0, b"\x1b[?1049h\x1b[?25lwelcome\n$ ".to_vec())]);
    // Output that comes is sent with its offset.
    terminals.output(1, b"ls\r\n", Instant::now());
    let end = 24;
    assert_eq!(outputs(&sent(&mut terminals, socket)), [(1, end, b"ls\r\n".to_vec())]);
    assert_eq!(terminals.terminals[&1].used, 2000, "opened now");
}

#[test]
fn a_page_that_comes_back_gets_what_follows_what_it_saw() {
    let mut terminals = world();
    terminals.output(1, b"one\ntwo\n", Instant::now());
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    assert_eq!(terminals.attach(socket, 1, (80, 24), Some(4), now()), Ok(false));
    let frames = sent(&mut terminals, socket);
    assert_eq!(texts(&frames)[0]["replay"], false);
    assert_eq!(texts(&frames)[0]["live"], 8, "what it missed was written before the attach");
    assert_eq!(texts(&frames)[0].get("modes"), None);
    assert_eq!(outputs(&frames), [(1, 4, b"two\n".to_vec())]);
    // An offset the ring no longer holds, or never gave, is a replay.
    for since in [9, u64::MAX] {
        assert_eq!(terminals.attach(socket, 1, (80, 24), Some(since), now()), Ok(true));
    }
    terminals.output(1, &vec![b'x'; 70_000], Instant::now());
    assert_eq!(terminals.attach(socket, 1, (80, 24), Some(0), now()), Ok(true));
}

#[test]
fn the_first_socket_to_show_a_pane_gives_it_its_size_and_a_resize_from_any_wins() {
    let mut terminals = world();
    let first = terminals.open_socket(ALICE, HERS).unwrap();
    let second = terminals.open_socket(ALICE, HER_OTHER).unwrap();
    terminals.attach(first, 1, (120, 40), None, now()).unwrap();
    assert_eq!((terminals.panes[&1].cols, terminals.panes[&1].rows), (120, 40));
    terminals.attach(second, 1, (100, 30), None, now()).unwrap();
    let attached = texts(&sent(&mut terminals, second))[0].clone();
    assert_eq!((attached["cols"].clone(), attached["rows"].clone()), (json!(120), json!(40)));
    terminals.resize(second, 1, 100, 30).unwrap();
    assert_eq!((terminals.panes[&1].cols, terminals.panes[&1].rows), (100, 30));
    // A pane the socket does not show is not resized by it.
    let refused = terminals.resize(second, 2, 80, 24).unwrap_err();
    assert_eq!(refused.kind, Kind::NoTerminal);
}

#[test]
fn a_flood_pauses_reading_until_the_page_catches_up_and_a_slow_socket_is_let_go() {
    let mut terminals = world();
    let quick = terminals.open_socket(ALICE, HERS).unwrap();
    let slow = terminals.open_socket(ALICE, HER_OTHER).unwrap();
    let start = Instant::now();
    terminals.attach(quick, 1, (80, 24), None, (start, 2000)).unwrap();
    terminals.attach(slow, 1, (80, 24), None, (start, 2000)).unwrap();
    let chunk = vec![b'y'; CHUNK_MAX];
    for _ in 0..16 {
        terminals.output(1, &chunk, start);
    }
    assert_eq!(terminals.paused(1), Some(false), "256 KiB is not past the limit");
    terminals.output(1, b"z", start);
    assert_eq!(terminals.paused(1), Some(true));
    // One socket catching up is not enough; both under 64 KiB is.
    let end = 16 * CHUNK_MAX as u64 + 1;
    terminals.ack(quick, 1, end, start);
    assert_eq!(terminals.paused(1), Some(true));
    terminals.ack(slow, 1, end - RESUME_AT - 1, start);
    assert_eq!(terminals.paused(1), Some(true));
    terminals.ack(slow, 1, end - RESUME_AT, start);
    assert_eq!(terminals.paused(1), Some(false));
    // An acknowledgement past what was sent counts as all of it, and one
    // before what was counted is nothing.
    terminals.ack(quick, 1, u64::MAX, start);
    terminals.ack(quick, 1, 0, start);
    let flow = &terminals.sockets[&quick].flows[&1];
    assert_eq!((flow.sent, flow.acked), (end, end));
    // Over again, and for 30 seconds: the slow socket no longer shows the
    // pane, and the pane is read again for the quick one.
    for _ in 0..17 {
        terminals.output(1, &chunk, start);
    }
    terminals.ack(quick, 1, u64::MAX, start);
    assert_eq!(terminals.paused(1), Some(true));
    let _ = sent(&mut terminals, slow);
    terminals.tick((start + SLOW_FOR - Duration::from_secs(1), 2000), |_| true);
    assert!(terminals.sockets[&slow].flows.contains_key(&1));
    terminals.tick((start + SLOW_FOR, 2000), |_| true);
    assert!(!terminals.sockets[&slow].flows.contains_key(&1));
    assert_eq!(texts(&sent(&mut terminals, slow)), [json!({"detached": 1, "why": "slow"})]);
    assert_eq!(terminals.paused(1), Some(false));
}

#[test]
fn a_pane_shown_on_two_pages_takes_the_answers_of_the_first_opened() {
    let mut terminals = world();
    let first = terminals.open_socket(ALICE, HERS).unwrap();
    let second = terminals.open_socket(ALICE, HER_OTHER).unwrap();
    let answer = b"\x1b[3;5R";
    assert!(matches!(terminals.answer(first, 1, answer), Typed::Dropped), "not shown yet");
    // The second page attaches first: until the first shows the pane too,
    // it is the one that answers.
    terminals.attach(second, 1, (80, 24), None, now()).unwrap();
    assert!(matches!(terminals.answer(second, 1, answer), Typed::Queued));
    terminals.attach(first, 1, (80, 24), None, now()).unwrap();
    assert!(
        matches!(terminals.answer(second, 1, answer), Typed::Dropped),
        "one question, one answer"
    );
    assert!(matches!(terminals.answer(first, 1, answer), Typed::Queued));
    // Both pages type, and both are heard.
    assert!(matches!(terminals.type_in(second, 1, b"ls\r"), Typed::Queued));
    // With the first page gone, the second answers again.
    terminals.detach(first, 1, Instant::now());
    assert!(matches!(terminals.answer(second, 1, answer), Typed::Queued));
    let typing: Vec<_> = std::iter::from_fn(|| terminals.next_typing(1).unwrap()).collect();
    assert_eq!(typing, [&answer[..], answer, b"ls\r", answer]);
    // Bob's page shows none of Alice's panes, and answers for none.
    let bobs = terminals.open_socket(BOB, "s-bob-1").unwrap();
    assert!(matches!(terminals.answer(bobs, 1, answer), Typed::Dropped));
}

#[test]
fn typing_waits_for_the_master_up_to_64_kib_and_only_from_a_socket_that_shows_the_pane() {
    let mut terminals = world();
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    assert!(matches!(terminals.type_in(socket, 1, b"ls\r"), Typed::Dropped), "not shown yet");
    terminals.attach(socket, 1, (80, 24), None, now()).unwrap();
    assert!(matches!(terminals.type_in(socket, 1, b"ls\r"), Typed::Queued));
    let most = vec![b'a'; TYPING_MAX - 3];
    assert!(matches!(terminals.type_in(socket, 1, &most), Typed::Queued));
    assert!(matches!(terminals.type_in(socket, 1, b"!"), Typed::Full(_)));
    assert_eq!(terminals.next_typing(1), Ok(Some(b"ls\r".to_vec())));
    terminals.typing_taken(1, 3);
    assert!(matches!(terminals.type_in(socket, 1, b"!"), Typed::Queued));
    assert_eq!(terminals.next_typing(1).unwrap().unwrap().len(), most.len());
    assert_eq!(terminals.next_typing(1), Ok(Some(b"!".to_vec())));
    assert_eq!(terminals.next_typing(1), Ok(None));
    assert_eq!(terminals.next_typing(99), Err(()));
}

#[test]
fn a_socket_shows_16_panes_at_most_and_a_session_opens_4_sockets() {
    let mut terminals = Terminals::default();
    for id in 1..=17 {
        terminals.add_terminal(terminal(id, ALICE), pane(id, id, None));
    }
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    for id in 1..=16 {
        terminals.attach(socket, id, (80, 24), None, now()).unwrap();
    }
    let refused = terminals.attach(socket, 17, (80, 24), None, now()).unwrap_err();
    assert_eq!(refused.kind, Kind::TooManyTerminals);
    // Attaching again to one it shows is no more.
    assert!(terminals.attach(socket, 16, (80, 24), None, now()).is_ok());
    for _ in 0..3 {
        terminals.open_socket(ALICE, HERS).unwrap();
    }
    assert_eq!(terminals.open_socket(ALICE, HERS).unwrap_err().kind, Kind::TooManyTerminals);
    assert!(terminals.open_socket(ALICE, HER_OTHER).is_ok(), "another session has its own");
}

#[test]
fn an_account_keeps_its_terminals_up_to_its_limit_and_admind_256_panes() {
    let mut terminals = world();
    assert!(terminals.room_for_terminal(ALICE, 2).is_ok());
    terminals.add_terminal(terminal(3, ALICE), pane(3, 3, None));
    let refused = terminals.room_for_terminal(ALICE, 2).unwrap_err();
    assert_eq!(refused.kind, Kind::TooManyTerminals);
    assert!(refused.detail.contains("you have 2 terminals"));
    assert!(terminals.room_for_terminal(BOB, 2).is_ok());
    for id in 4..=u32::try_from(PANES_MAX).unwrap() {
        terminals.panes.insert(id, pane(id, 3, None));
    }
    assert_eq!(terminals.room_for_pane().unwrap_err().kind, Kind::TooManyTerminals);
}

#[test]
fn a_pane_that_ends_is_said_to_its_sockets_and_its_terminal_ends_with_its_last() {
    let mut terminals = world();
    terminals.add_pane(pane(3, 1, None));
    let shows = terminals.open_socket(ALICE, HERS).unwrap();
    let other = terminals.open_socket(ALICE, HER_OTHER).unwrap();
    terminals.attach(shows, 1, (80, 24), None, now()).unwrap();
    let _ = sent(&mut terminals, shows);
    terminals.end_pane(1, Why::Exited, Some(0));
    assert_eq!(
        texts(&sent(&mut terminals, shows)),
        [json!({"ended": 1, "why": "exited", "status": 0}), json!({"changed": 1})]
    );
    assert_eq!(texts(&sent(&mut terminals, other)), [json!({"changed": 1})]);
    assert_eq!(terminals.terminals[&1].panes, [3]);
    assert!(terminals.sockets[&shows].flows.is_empty());
    terminals.end_terminal(1);
    assert!(!terminals.terminals.contains_key(&1));
    assert!(terminals.terminal_of(ALICE, 1).is_err());
    assert_eq!(terminals.terminal_of(BOB, 2).map(|t| t.id), Ok(2));
    assert_eq!(terminals.terminal_of(ALICE, 2).unwrap_err().kind, Kind::NoTerminal);
}

#[test]
fn administrators_panes_end_with_the_unlock_and_terminals_left_unopened_with_their_days() {
    let mut terminals = world();
    terminals.add_pane(pane(3, 1, Some(HERS)));
    terminals.add_terminal(terminal(4, ALICE), pane(4, 4, Some(HER_OTHER)));
    let running = Rooted { account: ALICE.into(), started: 1000, ended: None };
    assert_eq!(terminals.rooted(unix_now()), [running.clone(), running.clone()]);
    terminals.tick(now(), |session| session == HER_OTHER);
    assert!(!terminals.panes.contains_key(&3), "her first session is locked");
    assert!(terminals.panes.contains_key(&4));
    terminals.end_admin_panes(HER_OTHER, Why::SignedOut);
    assert!(!terminals.terminals.contains_key(&4));
    // The journal still knows they ran, for as long as it looks back.
    let rooted = terminals.rooted(unix_now());
    assert_eq!(rooted.len(), 2);
    assert!(rooted.iter().all(|r| r.account == ALICE && r.ended.is_some()), "{rooted:?}");
    assert!(terminals.rooted(unix_now() + ROOTED_KEPT + 1).is_empty());
    // Terminal 2 has been unopened for a week, terminal 1 a moment less;
    // a page that shows a pane keeps its terminal.
    let week = i64::from(KEEP_DAYS) * 86_400;
    terminals.terminals.get_mut(&1).unwrap().used = 1001;
    let socket = terminals.open_socket(BOB, "s-bob").unwrap();
    terminals.attach(socket, 2, (80, 24), None, (Instant::now(), 1000)).unwrap();
    terminals.tick((Instant::now(), 1000 + week), |_| true);
    assert!(terminals.terminals.contains_key(&1));
    assert!(terminals.terminals.contains_key(&2));
    terminals.detach(socket, 2, Instant::now());
    terminals.tick((Instant::now(), 1000 + week), |_| true);
    assert!(terminals.terminals.contains_key(&1));
    assert!(!terminals.terminals.contains_key(&2));
}

#[test]
fn a_closed_socket_sends_its_close_last_and_lets_its_panes_go() {
    let mut terminals = world();
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    terminals.attach(socket, 1, (80, 24), None, now()).unwrap();
    terminals.close_session(HERS, Code::SessionEnded);
    assert!(terminals.closing(socket));
    assert_eq!(terminals.next_frame(socket), Next::Frame(websocket::close(Code::SessionEnded)));
    assert_eq!(terminals.next_frame(socket), Next::Stop);
    // Nothing more is queued, and the pane is shown on it no more.
    terminals.send_text(socket, "{}");
    assert_eq!(terminals.next_frame(socket), Next::Stop);
    assert!(terminals.panes[&1].attached.is_empty());
    terminals.close_socket(socket, Instant::now());
    assert_eq!(terminals.next_frame(socket), Next::Stop);
    assert!(terminals.closing(socket), "gone");
}

#[test]
fn a_socket_with_more_waiting_than_it_could_need_is_closed_as_a_fault() {
    let mut terminals = world();
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    let big = "x".repeat(1 << 20);
    for _ in 0..8 {
        terminals.send_text(socket, &big);
    }
    assert!(terminals.closing(socket));
    assert_eq!(terminals.next_frame(socket), Next::Frame(websocket::close(Code::Failed)));
}

#[test]
fn everything_ends_as_admind_stops() {
    let mut terminals = world();
    let socket = terminals.open_socket(ALICE, HERS).unwrap();
    terminals.attach(socket, 1, (80, 24), None, now()).unwrap();
    let _ = sent(&mut terminals, socket);
    // Output that has not gone yet is let go; the page is told why the
    // pane ended before the socket closes.
    terminals.output(1, b"not yet sent", Instant::now());
    terminals.end_all();
    assert!(terminals.terminals.is_empty() && terminals.panes.is_empty());
    let frames = sent(&mut terminals, socket);
    assert_eq!(frames.last().map(|frame| frame.0), Some(0x8), "the close comes last");
    assert!(outputs(&frames).is_empty(), "the output that waited is let go");
    let messages = texts(&frames);
    assert!(messages.contains(&json!({"ended": 1, "why": "restart"})), "{messages:?}");
}

#[test]
fn a_layout_is_one_of_five_words() {
    for word in ["one", "columns", "rows", "main-left", "grid"] {
        assert_eq!(Layout::of(word).map(Layout::word), Some(word));
    }
    assert_eq!(Layout::of("tabs"), None);
}
