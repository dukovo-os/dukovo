// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The WebSocket's handshake and frames: RFC 6455's own examples, each
//! rule a browser's frame must keep, and frames that come a byte at a
//! time.

use http::HeaderValue;

use super::*;

/// The mask of the RFC's examples.
const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

/// A browser's frame: `first` is its first byte, the body is masked with
/// [`MASK`], and the length takes the fewest bytes.
fn masked(first: u8, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![first];
    match body.len() {
        length @ 0..=125 => frame.push(MASKED | u8::try_from(length).unwrap()),
        length @ 126..=0xFFFF => {
            frame.push(MASKED | 0x7e);
            frame.extend_from_slice(&u16::try_from(length).unwrap().to_be_bytes());
        }
        length => {
            frame.push(MASKED | 0x7f);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&MASK);
    frame.extend(body.iter().zip(MASK.iter().cycle()).map(|(byte, key)| byte ^ key));
    frame
}

/// The messages `bytes` make, pushed in pieces of `piece` bytes, and the
/// code they end with, if a rule was broken.
fn read(bytes: &[u8], piece: usize) -> (Vec<Message>, Option<Code>) {
    let mut frames = Frames::new();
    let mut messages = Vec::new();
    for chunk in bytes.chunks(piece.max(1)) {
        frames.push(chunk);
        loop {
            match frames.next() {
                Ok(Some(message)) => messages.push(message),
                Ok(None) => break,
                Err(code) => return (messages, Some(code)),
            }
        }
    }
    (messages, None)
}

/// The code `bytes` are refused with, whole or a byte at a time.
fn refused(bytes: &[u8]) -> Option<Code> {
    let whole = read(bytes, bytes.len()).1;
    assert_eq!(whole, read(bytes, 1).1, "the same, a byte at a time");
    whole
}

#[test]
fn the_rfcs_masked_hello_reads_whole_or_a_byte_at_a_time() {
    let hello = [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
    for piece in [1, 2, 5, 11] {
        assert_eq!(read(&hello, piece), (vec![Message::Text("Hello".into())], None));
    }
    assert_eq!(masked(FIN | TEXT, b"Hello"), hello);
}

#[test]
fn fragments_are_joined_and_a_control_frame_may_come_between_them() {
    let mut bytes = masked(TEXT, b"Hel");
    bytes.extend(masked(FIN | PING, b"are you there"));
    bytes.extend(masked(CONTINUATION, b"l"));
    bytes.extend(masked(FIN | CONTINUATION, b"o"));
    bytes.extend(masked(FIN | BINARY, &[0, 0, 0, 12, b'l', b's', b'\r']));
    let wanted = vec![
        Message::Ping(b"are you there".to_vec()),
        Message::Text("Hello".into()),
        Message::Binary(vec![0, 0, 0, 12, b'l', b's', b'\r']),
    ];
    for piece in [1, 3, 7, bytes.len()] {
        assert_eq!(read(&bytes, piece), (wanted.clone(), None), "pieces of {piece}");
    }
}

#[test]
fn a_character_split_between_fragments_is_text() {
    let word = "élan".as_bytes();
    let mut bytes = masked(TEXT, &word[..1]);
    bytes.extend(masked(FIN | CONTINUATION, &word[1..]));
    assert_eq!(read(&bytes, 1), (vec![Message::Text("élan".into())], None));
}

#[test]
fn lengths_of_two_and_eight_bytes_read_and_a_longer_form_than_needed_is_taken() {
    let body = vec![7; 300];
    assert_eq!(read(&masked(FIN | BINARY, &body), 64), (vec![Message::Binary(body)], None));
    let mut long = vec![FIN | BINARY, MASKED | 127];
    long.extend_from_slice(&5u64.to_be_bytes());
    long.extend_from_slice(&[0; 4]);
    long.extend_from_slice(b"abcde");
    assert_eq!(read(&long, 1), (vec![Message::Binary(b"abcde".to_vec())], None));
}

#[test]
fn a_frame_that_is_not_masked_breaks_the_protocol() {
    assert_eq!(refused(&[FIN | TEXT, 2, b'h', b'i']), Some(Code::Protocol));
}

#[test]
fn reserved_bits_and_unknown_opcodes_break_the_protocol() {
    for bit in [0x40, 0x20, 0x10] {
        assert_eq!(refused(&masked(FIN | bit | TEXT, b"x")), Some(Code::Protocol), "{bit:#x}");
    }
    for opcode in (3..=7).chain(11..=15) {
        assert_eq!(refused(&masked(FIN | opcode, b"")), Some(Code::Protocol), "{opcode}");
    }
}

#[test]
fn a_control_frame_is_short_and_never_fragmented() {
    assert_eq!(refused(&masked(PING, b"")), Some(Code::Protocol));
    assert_eq!(refused(&masked(FIN | PONG, &[0; 126])), Some(Code::Protocol));
    assert_eq!(read(&masked(FIN | PING, &[1; 125]), 9).0, vec![Message::Ping(vec![1; 125])]);
}

#[test]
fn fragments_come_in_order() {
    assert_eq!(refused(&masked(FIN | CONTINUATION, b"x")), Some(Code::Protocol));
    let mut bytes = masked(TEXT, b"a");
    bytes.extend(masked(FIN | BINARY, b"b"));
    assert_eq!(refused(&bytes), Some(Code::Protocol));
}

#[test]
fn a_length_with_its_top_bit_set_breaks_the_protocol() {
    let mut bytes = vec![FIN | BINARY, MASKED | 127];
    bytes.extend_from_slice(&(1u64 << 63).to_be_bytes());
    assert_eq!(refused(&bytes), Some(Code::Protocol));
}

#[test]
fn a_message_is_at_most_64_kib_and_a_longer_one_is_refused_before_its_bytes() {
    let most = vec![1; MESSAGE_MAX];
    assert_eq!(read(&masked(FIN | BINARY, &most), 4096).0, vec![Message::Binary(most)]);
    // Only the head of a frame one byte too long: refused with no body.
    let head = &masked(FIN | BINARY, &vec![0; MESSAGE_MAX + 1])[..14];
    assert_eq!(refused(head), Some(Code::TooBig));
    let mut huge = vec![FIN | BINARY, MASKED | 127];
    huge.extend_from_slice(&(1u64 << 40).to_be_bytes());
    assert_eq!(refused(&huge), Some(Code::TooBig));
    // Fragments that pass the limit together.
    let mut bytes = masked(BINARY, &vec![0; 40_000]);
    bytes.extend(masked(FIN | CONTINUATION, &vec![0; 30_000]));
    assert_eq!(read(&bytes, 1000).1, Some(Code::TooBig));
}

#[test]
fn text_must_be_utf8() {
    assert_eq!(refused(&masked(FIN | TEXT, &[0xff, 0xfe])), Some(Code::NotText));
    let mut bytes = masked(TEXT, "é".as_bytes());
    bytes.extend(masked(FIN | CONTINUATION, &[0xc3]));
    assert_eq!(refused(&bytes), Some(Code::NotText));
    // Bytes that are not UTF-8 are a binary message's to carry.
    assert_eq!(read(&masked(FIN | BINARY, &[0xff]), 1).0, vec![Message::Binary(vec![0xff])]);
}

#[test]
fn a_close_has_no_body_or_a_code_a_browser_may_send_and_text() {
    let close = |body: &[u8]| read(&masked(FIN | CLOSE, body), 1);
    assert_eq!(close(b""), (vec![Message::Close(None)], None));
    let mut done = 1000u16.to_be_bytes().to_vec();
    done.extend_from_slice("bye é".as_bytes());
    assert_eq!(close(&done), (vec![Message::Close(Some(1000))], None));
    for code in [1000, 1001, 1003, 1007, 1011, 1014, 3000, 4001, 4999] {
        let body = u16::to_be_bytes(code);
        assert_eq!(close(&body).0, vec![Message::Close(Some(code))], "{code}");
    }
    for code in [0, 999, 1004, 1005, 1006, 1015, 1016, 2999, 5000, 65535] {
        let body = u16::to_be_bytes(code);
        assert_eq!(close(&body).1, Some(Code::Protocol), "{code}");
    }
    assert_eq!(close(&[3]).1, Some(Code::Protocol));
    let mut bad = 1000u16.to_be_bytes().to_vec();
    bad.push(0xff);
    assert_eq!(close(&bad).1, Some(Code::NotText));
}

#[test]
fn once_a_rule_is_broken_nothing_more_is_read() {
    let mut frames = Frames::new();
    frames.push(&[FIN | TEXT, 1, b'x']);
    assert_eq!(frames.next(), Err(Code::Protocol));
    assert_eq!(frames.held(), 0);
    frames.push(&masked(FIN | TEXT, b"fine"));
    assert_eq!(frames.held(), 0);
    assert_eq!(frames.next(), Err(Code::Protocol));
}

#[test]
fn what_is_held_is_counted_until_its_message_is_given() {
    let mut frames = Frames::new();
    let bytes = masked(TEXT, b"abc");
    frames.push(&bytes);
    assert_eq!(frames.next(), Ok(None));
    assert_eq!(frames.held(), 3, "the fragment's body, unmasked");
    frames.push(&masked(FIN | CONTINUATION, b"d")[..3]);
    assert_eq!(frames.next(), Ok(None));
    assert_eq!(frames.held(), 6);
    frames.push(&masked(FIN | CONTINUATION, b"d")[3..]);
    assert_eq!(frames.next(), Ok(Some(Message::Text("abcd".into()))));
    assert_eq!(frames.held(), 0);
}

#[test]
fn admind_frames_are_final_unmasked_and_take_the_fewest_length_bytes() {
    assert_eq!(frame(Kind::Text, b"Hello"), [0x81, 0x05, b'H', b'e', b'l', b'l', b'o']);
    for (length, head) in [
        (0, vec![0x82, 0]),
        (125, vec![0x82, 125]),
        (126, vec![0x82, 126, 0, 126]),
        (65_535, vec![0x82, 126, 0xff, 0xff]),
        (65_536, vec![0x82, 127, 0, 0, 0, 0, 0, 1, 0, 0]),
    ] {
        let made = frame(Kind::Binary, &vec![9; length]);
        assert_eq!(made[..head.len()], head[..], "{length}");
        assert_eq!(made.len(), head.len() + length);
    }
    assert_eq!(frame(Kind::Ping, &[1; 200]).len(), 2 + 125);
    assert_eq!(frame(Kind::Pong, b"p"), [0x8A, 1, b'p']);
}

#[test]
fn a_close_frame_carries_its_code_and_a_reason_that_fits() {
    let made = close(Code::SessionEnded);
    assert_eq!(made[0], FIN | CLOSE);
    assert_eq!(made[2..4], 4001u16.to_be_bytes());
    assert_eq!(&made[4..], b"the session ended");
    for code in [
        Code::Done,
        Code::Stopping,
        Code::Protocol,
        Code::NotText,
        Code::Policy,
        Code::TooBig,
        Code::Failed,
        Code::SessionEnded,
    ] {
        assert!(code.reason().len() <= CONTROL_MAX - 2, "{code:?}");
        let length = usize::from(close(code)[1]);
        assert_eq!(length, 2 + code.reason().len());
    }
}

#[test]
fn what_admind_sends_reads_back_once_masked() {
    for (kind, body) in
        [(Kind::Text, &b"caf\xc3\xa9"[..]), (Kind::Binary, &vec![0; 70_000][..65_536])]
    {
        let made = frame(kind, body);
        let opcode = made[0];
        let start = made.len() - body.len();
        let again = masked(opcode, &made[start..]);
        let (messages, broken) = read(&again, 1500);
        assert_eq!(broken, None);
        let wanted = match kind {
            Kind::Text => Message::Text(String::from_utf8(body.to_vec()).unwrap()),
            _ => Message::Binary(body.to_vec()),
        };
        assert_eq!(messages, vec![wanted]);
    }
}

/// The headers of a browser's request to open the socket.
fn asked(extra: &[(&'static str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("upgrade", "websocket"),
        ("connection", "Upgrade"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ("sec-websocket-protocol", "dukovo-terminal-1, csrf.AbC-_1"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    for (name, value) in extra {
        if value.is_empty() {
            headers.remove(*name);
        } else {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
    }
    headers
}

#[test]
fn the_accept_key_is_the_rfcs_example() {
    assert_eq!(accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

#[test]
fn a_browsers_request_opens_the_socket_and_gives_the_token() {
    let wanted = Opening { accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".into(), csrf: "AbC-_1".into() };
    assert_eq!(opening(&asked(&[])), Ok(wanted));
    // Other browsers' ways: a list in `Connection`, words in any case, and
    // the protocols on two lines.
    let mut headers = asked(&[("connection", "keep-alive, Upgrade"), ("upgrade", "WebSocket")]);
    headers.insert("sec-websocket-protocol", HeaderValue::from_static("csrf.xyz"));
    headers.append("sec-websocket-protocol", HeaderValue::from_static("dukovo-terminal-1"));
    assert_eq!(opening(&headers).map(|opened| opened.csrf), Ok("xyz".into()));
}

#[test]
fn a_request_that_is_not_a_websocket_of_version_13_is_refused() {
    for extra in [
        [("upgrade", "")],
        [("upgrade", "h2c")],
        [("connection", "")],
        [("connection", "keep-alive")],
        [("sec-websocket-version", "")],
        [("sec-websocket-version", "8")],
    ] {
        assert!(opening(&asked(&extra)).is_err(), "{extra:?}");
    }
    let mut twice = asked(&[]);
    twice.append("sec-websocket-version", HeaderValue::from_static("13"));
    assert!(opening(&twice).is_err());
}

#[test]
fn a_key_is_16_bytes_in_base64() {
    for key in ["", "not base64!", "AAAAAAAAAAAAAAAAAAAA", "dGhlIHNhbXBsZSBub25jZQ"] {
        assert!(opening(&asked(&[("sec-websocket-key", key)])).is_err(), "{key}");
    }
}

#[test]
fn the_protocol_and_one_token_must_be_named() {
    for protocols in [
        "",
        "csrf.abc",
        "dukovo-terminal-2, csrf.abc",
        "dukovo-terminal-1",
        "dukovo-terminal-1, csrf.a, csrf.b",
    ] {
        let headers = asked(&[("sec-websocket-protocol", protocols)]);
        assert!(opening(&headers).is_err(), "{protocols}");
    }
}
