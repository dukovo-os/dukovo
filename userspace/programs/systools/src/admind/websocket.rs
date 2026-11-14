// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The WebSocket of the Terminal page (RFC 6455, ADR 0517): the checks of
//! the request that opens one, the answer's key, and the frames both ways.
//!
//! Nothing here reads or writes a connection. [`Frames`] takes the bytes a
//! browser sends, in pieces of any size, and gives whole messages, or the
//! close code of the rule a frame broke, after which it reads nothing more;
//! [`frame`] and [`close`] make what admind sends. The terminal's socket
//! does the reading and the writing, the pings and the limits on what
//! waits.
//!
//! The rules a browser's frames must keep:
//!
//! - every frame is masked, its three reserved bits are 0, and its opcode
//!   is one of the six: else close 1002;
//! - a message is at most [`MESSAGE_MAX`] bytes, its fragments joined, and
//!   one that would pass that is refused as soon as a frame's length says
//!   so, before its bytes come: 1009;
//! - a control frame (close, ping, pong) is at most 125 bytes and never
//!   fragmented, a continuation follows a fragment, and a new message does
//!   not start inside another: 1002;
//! - a text message, and a close's reason, are UTF-8: 1007;
//! - a close has no body, or a code the protocol lets a browser send,
//!   followed by its reason: 1002.
//!
//! admind's frames are never masked or fragmented, and no extension is
//! ever agreed, so compression never is.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use http::header::{self, HeaderMap, HeaderName};

/// The most bytes a message may have, its fragments joined.
pub const MESSAGE_MAX: usize = 64 << 10;

/// The most bytes a control frame's body may have.
const CONTROL_MAX: usize = 125;

/// The one protocol the socket speaks, which the request names.
pub const PROTOCOL: &str = "dukovo-terminal-1";

/// What the request's list of protocols puts before the session's token.
const CSRF_PREFIX: &str = "csrf.";

/// What RFC 6455 adds to the key to make the answer's.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The opcodes.
const CONTINUATION: u8 = 0x0;
const TEXT: u8 = 0x1;
const BINARY: u8 = 0x2;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xA;

/// The bits of a frame's first two bytes.
const FIN: u8 = 0x80;
const RESERVED: u8 = 0x70;
const OPCODE: u8 = 0x0F;
const MASKED: u8 = 0x80;
const LENGTH: u8 = 0x7F;

/// Why a socket closes, as its close frame says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    /// 1000: it is done, as either side asked.
    Done,
    /// 1001: admind is stopping.
    Stopping,
    /// 1002: the browser broke the protocol.
    Protocol,
    /// 1007: a text message, or a close's reason, is not UTF-8.
    NotText,
    /// 1008: a message the console does not take, or no answer to its
    /// pings.
    Policy,
    /// 1009: a message over [`MESSAGE_MAX`].
    TooBig,
    /// 1011: admind failed.
    Failed,
    /// 4001: the console session ended: sign-out, its hours, or idle.
    SessionEnded,
}

impl Code {
    /// The code's number.
    pub fn number(self) -> u16 {
        match self {
            Code::Done => 1000,
            Code::Stopping => 1001,
            Code::Protocol => 1002,
            Code::NotText => 1007,
            Code::Policy => 1008,
            Code::TooBig => 1009,
            Code::Failed => 1011,
            Code::SessionEnded => 4001,
        }
    }

    /// The reason the close frame gives, in a few words.
    pub fn reason(self) -> &'static str {
        match self {
            Code::Done => "done",
            Code::Stopping => "the console is stopping",
            Code::Protocol => "a frame broke the protocol",
            Code::NotText => "a text message was not UTF-8",
            Code::Policy => "a message the console does not take",
            Code::TooBig => "a message was over 64 KiB",
            Code::Failed => "the console failed",
            Code::SessionEnded => "the session ended",
        }
    }
}

/// A whole message from the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    /// A ping, whose body the pong gives back.
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// A close, with the browser's code if it gave one.
    Close(Option<u16>),
}

/// The head of a frame: what its first 2 to 14 bytes say.
#[derive(Debug)]
struct Head {
    fin: bool,
    opcode: u8,
    mask: [u8; 4],
    /// The bytes of the body.
    length: usize,
    /// The bytes of the head.
    size: usize,
}

/// The frames a browser sends, made into messages as their bytes come.
#[derive(Debug, Default)]
pub struct Frames {
    /// What came and is not yet part of a message given.
    held: Vec<u8>,
    /// The message whose fragments are being joined: whether it is text,
    /// and its bytes so far.
    joining: Option<(bool, Vec<u8>)>,
    /// The rule a frame broke: after it, nothing more is read.
    broken: Option<Code>,
}

impl Frames {
    pub fn new() -> Frames {
        Frames::default()
    }

    /// Takes `bytes` the browser sent. Nothing is read once a rule was
    /// broken.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.broken.is_none() {
            self.held.extend_from_slice(bytes);
        }
    }

    /// How many bytes came and are not yet given as a message. A socket
    /// that reads only when [`Frames::next`] wants more holds at most one
    /// frame's worth and one read's.
    #[cfg(any(test, feature = "fuzz"))]
    pub fn held(&self) -> usize {
        self.held.len() + self.joining.as_ref().map_or(0, |(_, bytes)| bytes.len())
    }

    /// The next whole message, `None` while its bytes have not all come.
    ///
    /// # Errors
    ///
    /// The close code of the rule a frame broke, then and on every call
    /// after it.
    pub fn next(&mut self) -> Result<Option<Message>, Code> {
        if let Some(code) = self.broken {
            return Err(code);
        }
        let next = self.read();
        if let Err(code) = next {
            self.broken = Some(code);
            self.held = Vec::new();
            self.joining = None;
        }
        next
    }

    /// [`Frames::next`], without remembering a broken rule.
    fn read(&mut self) -> Result<Option<Message>, Code> {
        loop {
            let Some(head) = self.head()? else { return Ok(None) };
            let end = head.size + head.length;
            if self.held.len() < end {
                return Ok(None);
            }
            let mut body: Vec<u8> = self.held.drain(..end).skip(head.size).collect();
            unmask(&mut body, head.mask);
            if let Some(message) = self.take(&head, body)? {
                return Ok(Some(message));
            }
            // A fragment that is not the last: read on.
        }
    }

    /// The head of the frame the held bytes start with, `None` while it
    /// has not all come; a head that breaks a rule is refused as soon as
    /// enough of it has come to say so.
    fn head(&self) -> Result<Option<Head>, Code> {
        let held = &self.held;
        let (Some(&first), Some(&second)) = (held.first(), held.get(1)) else { return Ok(None) };
        let fin = first & FIN != 0;
        let opcode = first & OPCODE;
        if first & RESERVED != 0 || !matches!(opcode, CONTINUATION | TEXT | BINARY | CLOSE..=PONG) {
            return Err(Code::Protocol);
        }
        if second & MASKED == 0 {
            return Err(Code::Protocol);
        }
        let control = opcode & 0x8 != 0;
        let short = second & LENGTH;
        if control && (!fin || usize::from(short) > CONTROL_MAX) {
            return Err(Code::Protocol);
        }
        match (opcode, &self.joining) {
            (CONTINUATION, None) | (TEXT | BINARY, Some(_)) => return Err(Code::Protocol),
            _ => {}
        }
        let (length, size) = match short {
            126 => {
                let Some(bytes) = held.get(2..4) else { return Ok(None) };
                (u64::from(u16::from_be_bytes([bytes[0], bytes[1]])), 4)
            }
            127 => {
                let Some(bytes) = held.get(2..10) else { return Ok(None) };
                let mut word = [0; 8];
                word.copy_from_slice(bytes);
                (u64::from_be_bytes(word), 10)
            }
            short => (u64::from(short), 2),
        };
        // The length's top bit is 0 (RFC 6455, 5.2); a longer frame than a
        // message may be is refused before its bytes come.
        if length >> 63 != 0 {
            return Err(Code::Protocol);
        }
        let joined = self.joining.as_ref().map_or(0, |(_, bytes)| bytes.len());
        let length = usize::try_from(length).map_err(|_| Code::TooBig)?;
        if !control && length > MESSAGE_MAX - joined {
            return Err(Code::TooBig);
        }
        let Some(key) = held.get(size..size + 4) else { return Ok(None) };
        let mask = [key[0], key[1], key[2], key[3]];
        Ok(Some(Head { fin, opcode, mask, length, size: size + 4 }))
    }

    /// The message a frame with `head` and the unmasked `body` ends, if it
    /// ends one.
    fn take(&mut self, head: &Head, body: Vec<u8>) -> Result<Option<Message>, Code> {
        match head.opcode {
            CLOSE => close_of(&body).map(Some),
            PING => Ok(Some(Message::Ping(body))),
            PONG => Ok(Some(Message::Pong(body))),
            CONTINUATION => {
                let (text, mut bytes) = self.joining.take().ok_or(Code::Protocol)?;
                bytes.extend_from_slice(&body);
                if head.fin {
                    message(text, bytes).map(Some)
                } else {
                    self.joining = Some((text, bytes));
                    Ok(None)
                }
            }
            opcode => {
                let text = opcode == TEXT;
                if head.fin {
                    message(text, body).map(Some)
                } else {
                    self.joining = Some((text, body));
                    Ok(None)
                }
            }
        }
    }
}

/// Undoes the mask `mask` of a browser's frame on `body`.
fn unmask(body: &mut [u8], mask: [u8; 4]) {
    for (byte, key) in body.iter_mut().zip(mask.iter().cycle()) {
        *byte ^= key;
    }
}

/// A whole data message of `bytes`, text if `text`.
fn message(text: bool, bytes: Vec<u8>) -> Result<Message, Code> {
    if text {
        String::from_utf8(bytes).map(Message::Text).map_err(|_| Code::NotText)
    } else {
        Ok(Message::Binary(bytes))
    }
}

/// The close a close frame's `body` says: none, or a code a browser may
/// send and a UTF-8 reason.
fn close_of(body: &[u8]) -> Result<Message, Code> {
    match body {
        [] => Ok(Message::Close(None)),
        [_] => Err(Code::Protocol),
        [high, low, reason @ ..] => {
            let code = u16::from_be_bytes([*high, *low]);
            // 1004 to 1006 and 1015 are never sent: they stand for no code,
            // or for what the connection did. 1016 to 2999 are the
            // protocol's own, not yet given; 3000 to 4999 are for the
            // applications.
            if !matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999) {
                return Err(Code::Protocol);
            }
            if std::str::from_utf8(reason).is_err() {
                return Err(Code::NotText);
            }
            Ok(Message::Close(Some(code)))
        }
    }
}

/// What admind sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Binary,
    Ping,
    Pong,
}

/// The frame admind sends to give `body` as one message of `kind`. A
/// ping's or pong's body is cut to the 125 bytes a control frame may have.
pub fn frame(kind: Kind, body: &[u8]) -> Vec<u8> {
    let (opcode, body) = match kind {
        Kind::Text => (TEXT, body),
        Kind::Binary => (BINARY, body),
        Kind::Ping => (PING, &body[..body.len().min(CONTROL_MAX)]),
        Kind::Pong => (PONG, &body[..body.len().min(CONTROL_MAX)]),
    };
    raw(opcode, body)
}

/// The close frame for `code`, with its reason.
pub fn close(code: Code) -> Vec<u8> {
    let mut body = code.number().to_be_bytes().to_vec();
    body.extend_from_slice(code.reason().as_bytes());
    raw(CLOSE, &body)
}

/// One unmasked, final frame with `opcode` and `body`.
fn raw(opcode: u8, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(body.len() + 10);
    frame.push(FIN | opcode);
    let length = body.len();
    match (u8::try_from(length), u16::try_from(length)) {
        (Ok(short), _) if usize::from(short) <= CONTROL_MAX => frame.push(short),
        (_, Ok(medium)) => {
            frame.push(126);
            frame.extend_from_slice(&medium.to_be_bytes());
        }
        _ => {
            frame.push(127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(body);
    frame
}

/// What a request to open a socket brings, once it passes [`opening`].
#[derive(Debug, PartialEq, Eq)]
pub struct Opening {
    /// The answer's `Sec-WebSocket-Accept`.
    pub accept: String,
    /// The session's token, as the page put it in the list of protocols:
    /// the one header a browser lets a page set on a socket's request.
    pub csrf: String,
}

/// Whether `headers` ask to open a WebSocket the console speaks: an
/// upgrade to `websocket`, version 13, a key of 16 bytes, and
/// [`PROTOCOL`] and the session's token among the protocols. The refusal
/// says what is missing. The `Origin`, the session and its token are the
/// route's to check.
///
/// # Errors
///
/// What the request lacks, in words for the log and the answer.
pub fn opening(headers: &HeaderMap) -> Result<Opening, &'static str> {
    if !has_token(headers, &header::UPGRADE, "websocket")
        || !has_token(headers, &header::CONNECTION, "upgrade")
    {
        return Err("the terminal's address opens a WebSocket, and nothing else");
    }
    let version = single(headers, &header::SEC_WEBSOCKET_VERSION);
    if version.map(str::trim) != Some("13") {
        return Err("the terminal's socket speaks WebSocket version 13");
    }
    let key = single(headers, &header::SEC_WEBSOCKET_KEY).map(str::trim);
    let key = key.filter(|key| STANDARD.decode(key).is_ok_and(|bytes| bytes.len() == 16));
    let Some(key) = key else {
        return Err("a WebSocket's key is 16 bytes in base64");
    };
    let protocols: Vec<&str> = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    if !protocols.contains(&PROTOCOL) {
        return Err("the terminal's socket speaks dukovo-terminal-1 alone");
    }
    let mut tokens = protocols.iter().filter_map(|protocol| protocol.strip_prefix(CSRF_PREFIX));
    let (Some(csrf), None) = (tokens.next(), tokens.next()) else {
        return Err("the socket's request did not carry the session's token: reload the page");
    };
    Ok(Opening { accept: accept(key), csrf: csrf.to_string() })
}

/// The `Sec-WebSocket-Accept` for the request's `key`: the base64 of the
/// SHA-1 of the key and RFC 6455's GUID. SHA-1 is what the protocol
/// fixes, as a check that the server read the request, not as a secret.
pub fn accept(key: &str) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    context.update(key.as_bytes());
    context.update(GUID.as_bytes());
    STANDARD.encode(context.finish())
}

/// Whether one of the comma-separated values of the headers `name` is
/// `token`, in any case.
fn has_token(headers: &HeaderMap, name: &HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case(token))
}

/// The value of the only header `name`, if there is exactly one and it
/// reads as text.
fn single<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

#[cfg(test)]
mod tests;
