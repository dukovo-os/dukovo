// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The frames sshd(8) and its forwarder pass (ADR 0349).
//!
//! sshd holds no network beyond its port: a connection's forwarding is
//! done by `sshd --forward`, which authd(8) starts as the person who
//! signed in, with the network their session has (ADR 0257). sshd writes
//! frames to its standard input and reads them from its standard output.
//! Each is a kind, a byte; a stream's or listen's number, four; the
//! length of what follows, four, both big-endian; then that many bytes,
//! at most [`PAYLOAD_MAX`].
//!
//! sshd numbers the streams and listens it asks for with even numbers,
//! the forwarder the connections it takes with odd ones, so neither
//! takes one the other did. Bytes pass on a stream only as the side that
//! receives them gives room, with [`Frame::Credit`]: the forwarder sends
//! sshd no more than sshd let it, which starts at nothing, and sshd lets
//! a client send it no more than the forwarder wrote, as RFC 4254's
//! window keeps it.
//!
//! It is here, with what authd is asked, so that its unit tests run on
//! the host.

/// A frame's kind, number and length.
pub const HEADER: usize = 9;
/// The most bytes a frame carries after its header.
pub const PAYLOAD_MAX: usize = 32 * 1024;
/// The room sshd gives the forwarder on a stream as it opens: what may
/// come before sshd has passed any of it on.
pub const CREDIT: u32 = 256 * 1024;
/// The most bytes the forwarder keeps for a stream that it has not
/// written yet: twice the window sshd gives clients, as sshd passes on
/// no more than the window lets a client send.
pub const QUEUE_MAX: usize = 4 * 1024 * 1024;
/// The longest host name or address taken.
pub const HOST_MAX: usize = 255;
/// The longest reason a failure gives.
pub const WHY_MAX: usize = 512;

/// A frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// sshd: connect to `host` at `port`, for a client's `direct-tcpip`
    /// channel.
    Connect {
        /// The stream it will be.
        stream: u32,
        /// A name or an address.
        host: String,
        /// The port.
        port: u16,
    },
    /// sshd: listen at `address` and `port`, for a client's
    /// `tcpip-forward`: `localhost` is the loopback addresses, and an
    /// empty one every address; port 0 asks for any that is free.
    Listen {
        /// The listen's number.
        listen: u32,
        /// Where: `localhost`, empty, or an address.
        address: String,
        /// The port.
        port: u16,
    },
    /// sshd: stop listening; the connections taken go on.
    Unlisten {
        /// The listen's number.
        listen: u32,
    },
    /// The forwarder: stream `stream` connected.
    Connected {
        /// The stream.
        stream: u32,
    },
    /// The forwarder: the stream or listen `id` could not be made.
    Failed {
        /// Its number.
        id: u32,
        /// Why, in words.
        why: String,
    },
    /// The forwarder: listen `listen` listens, at `port`.
    Listening {
        /// The listen.
        listen: u32,
        /// Its port, which port 0 asked to be chosen.
        port: u16,
    },
    /// The forwarder: listen `listen` took a connection from `address`
    /// and `port`, which is stream `stream`.
    Accepted {
        /// The listen.
        listen: u32,
        /// The new stream: an odd number.
        stream: u32,
        /// The address that connected.
        address: String,
        /// Its port.
        port: u16,
    },
    /// Bytes on a stream.
    Data {
        /// The stream.
        stream: u32,
        /// The bytes: one to [`PAYLOAD_MAX`] of them.
        bytes: Vec<u8>,
    },
    /// Nothing more comes on a stream from the side that sends it; the
    /// other way stays open.
    Eof {
        /// The stream.
        stream: u32,
    },
    /// A stream ends, both ways.
    Close {
        /// The stream.
        stream: u32,
    },
    /// The other side may send `bytes` more on a stream.
    Credit {
        /// The stream.
        stream: u32,
        /// How many.
        bytes: u32,
    },
}

/// A frame no forwarder or sshd sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

const CONNECT: u8 = 1;
const LISTEN: u8 = 2;
const UNLISTEN: u8 = 3;
const CONNECTED: u8 = 4;
const FAILED: u8 = 5;
const LISTENING: u8 = 6;
const ACCEPTED: u8 = 7;
const DATA: u8 = 8;
const EOF: u8 = 9;
const CLOSE: u8 = 10;
const CREDIT_KIND: u8 = 11;

impl Frame {
    /// Whether sshd sends frames of this kind; the forwarder sends the
    /// others, and both send bytes, their end, and room.
    pub fn from_sshd(&self) -> bool {
        !matches!(
            self,
            Frame::Connected { .. }
                | Frame::Failed { .. }
                | Frame::Listening { .. }
                | Frame::Accepted { .. }
        )
    }

    /// Whether the forwarder sends frames of this kind.
    pub fn from_forwarder(&self) -> bool {
        !matches!(self, Frame::Connect { .. } | Frame::Listen { .. } | Frame::Unlisten { .. })
    }

    /// Appends the frame to `out`. A host, address or reason longer
    /// than this takes is cut, and bytes past [`PAYLOAD_MAX`] are the
    /// caller's to split.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut payload = Vec::new();
        let (kind, number) = match self {
            Frame::Connect { stream, host, port } => {
                payload.extend_from_slice(&port.to_be_bytes());
                payload.extend_from_slice(cut(host.as_bytes(), HOST_MAX));
                (CONNECT, *stream)
            }
            Frame::Listen { listen, address, port } => {
                payload.extend_from_slice(&port.to_be_bytes());
                payload.extend_from_slice(cut(address.as_bytes(), HOST_MAX));
                (LISTEN, *listen)
            }
            Frame::Unlisten { listen } => (UNLISTEN, *listen),
            Frame::Connected { stream } => (CONNECTED, *stream),
            Frame::Failed { id, why } => {
                payload.extend_from_slice(cut(why.as_bytes(), WHY_MAX));
                (FAILED, *id)
            }
            Frame::Listening { listen, port } => {
                payload.extend_from_slice(&port.to_be_bytes());
                (LISTENING, *listen)
            }
            Frame::Accepted { listen, stream, address, port } => {
                payload.extend_from_slice(&stream.to_be_bytes());
                payload.extend_from_slice(&port.to_be_bytes());
                payload.extend_from_slice(cut(address.as_bytes(), HOST_MAX));
                (ACCEPTED, *listen)
            }
            Frame::Data { stream, bytes } => {
                payload.extend_from_slice(cut(bytes, PAYLOAD_MAX));
                (DATA, *stream)
            }
            Frame::Eof { stream } => (EOF, *stream),
            Frame::Close { stream } => (CLOSE, *stream),
            Frame::Credit { stream, bytes } => {
                payload.extend_from_slice(&bytes.to_be_bytes());
                (CREDIT_KIND, *stream)
            }
        };
        out.push(kind);
        out.extend_from_slice(&number.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
    }

    /// The first frame of `buffer`, and how many bytes it took; `None`
    /// while it is not all there.
    ///
    /// # Errors
    ///
    /// When the bytes are not a frame.
    pub fn decode(buffer: &[u8]) -> Result<Option<(Frame, usize)>, Malformed> {
        let Some(header) = buffer.get(..HEADER) else {
            return Ok(None);
        };
        let kind = header[0];
        let number = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        let length = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if length > PAYLOAD_MAX {
            return Err(Malformed("a frame longer than a frame may be"));
        }
        let Some(payload) = buffer.get(HEADER..HEADER + length) else {
            return Ok(None);
        };
        let frame = match kind {
            CONNECT => {
                let (port, host) = port_and_text(payload, HOST_MAX)?;
                if host.is_empty() {
                    return Err(Malformed("a connection to no host"));
                }
                Frame::Connect { stream: number, host, port }
            }
            LISTEN => {
                let (port, address) = port_and_text(payload, HOST_MAX)?;
                Frame::Listen { listen: number, address, port }
            }
            UNLISTEN => Frame::Unlisten { listen: empty(payload, number)? },
            CONNECTED => Frame::Connected { stream: empty(payload, number)? },
            FAILED => Frame::Failed { id: number, why: text(payload, WHY_MAX)? },
            LISTENING => {
                let (port, rest) = port_and_text(payload, 0)?;
                if !rest.is_empty() {
                    return Err(Malformed("a listen's port with more after it"));
                }
                Frame::Listening { listen: number, port }
            }
            ACCEPTED => {
                let Some((stream, rest)) = payload.split_first_chunk::<4>() else {
                    return Err(Malformed("an accepted connection without its stream"));
                };
                let (port, address) = port_and_text(rest, HOST_MAX)?;
                Frame::Accepted {
                    listen: number,
                    stream: u32::from_be_bytes(*stream),
                    address,
                    port,
                }
            }
            DATA => {
                if payload.is_empty() {
                    return Err(Malformed("data of no bytes"));
                }
                Frame::Data { stream: number, bytes: payload.to_vec() }
            }
            EOF => Frame::Eof { stream: empty(payload, number)? },
            CLOSE => Frame::Close { stream: empty(payload, number)? },
            CREDIT_KIND => {
                let bytes = <[u8; 4]>::try_from(payload)
                    .map_err(|_| Malformed("room that is not four bytes"))?;
                let bytes = u32::from_be_bytes(bytes);
                if bytes == 0 {
                    return Err(Malformed("room for no bytes"));
                }
                Frame::Credit { stream: number, bytes }
            }
            _ => return Err(Malformed("a frame of a kind there is not")),
        };
        Ok(Some((frame, HEADER + length)))
    }
}

/// The first `max` bytes of `bytes`.
fn cut(bytes: &[u8], max: usize) -> &[u8] {
    &bytes[..bytes.len().min(max)]
}

/// `number`, for a frame that carries nothing else.
fn empty(payload: &[u8], number: u32) -> Result<u32, Malformed> {
    if payload.is_empty() {
        Ok(number)
    } else {
        Err(Malformed("a frame with bytes it has none for"))
    }
}

/// UTF-8 text with no control characters, at most `max` bytes.
fn text(bytes: &[u8], max: usize) -> Result<String, Malformed> {
    let text = std::str::from_utf8(bytes).map_err(|_| Malformed("text that is not UTF-8"))?;
    if text.len() > max || text.chars().any(char::is_control) {
        return Err(Malformed("text too long, or with a control character"));
    }
    Ok(text.to_string())
}

/// A port, then text.
fn port_and_text(payload: &[u8], max: usize) -> Result<(u16, String), Malformed> {
    let Some((port, rest)) = payload.split_first_chunk::<2>() else {
        return Err(Malformed("a frame without its port"));
    };
    Ok((u16::from_be_bytes(*port), text(rest, max)?))
}

/// The frames that come in a stream of bytes, as they come.
#[derive(Default)]
pub struct Frames {
    buffer: Vec<u8>,
    at: usize,
}

impl Frames {
    /// Takes the `bytes` that came.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.at > 0 && self.at == self.buffer.len() {
            self.buffer.clear();
            self.at = 0;
        } else if self.at > PAYLOAD_MAX {
            self.buffer.drain(..self.at);
            self.at = 0;
        }
        self.buffer.extend_from_slice(bytes);
    }

    /// The next frame, once all of it came.
    ///
    /// # Errors
    ///
    /// When the bytes are not a frame: nothing after them can be read.
    pub fn pop(&mut self) -> Result<Option<Frame>, Malformed> {
        match Frame::decode(&self.buffer[self.at..])? {
            Some((frame, used)) => {
                self.at += used;
                Ok(Some(frame))
            }
            None => Ok(None),
        }
    }

    /// Whether part of a frame came and waits for the rest.
    pub fn partial(&self) -> bool {
        self.at < self.buffer.len()
    }
}

/// `bytes` as [`Frame::Data`] frames of [`PAYLOAD_MAX`] bytes at most,
/// appended to `out`.
pub fn data(stream: u32, bytes: &[u8], out: &mut Vec<u8>) {
    for chunk in bytes.chunks(PAYLOAD_MAX) {
        Frame::Data { stream, bytes: chunk.to_vec() }.encode(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> Vec<Frame> {
        vec![
            Frame::Connect { stream: 2, host: "example.com".into(), port: 80 },
            Frame::Listen { listen: 4, address: "localhost".into(), port: 8080 },
            Frame::Listen { listen: 6, address: String::new(), port: 0 },
            Frame::Unlisten { listen: 4 },
            Frame::Connected { stream: 2 },
            Frame::Failed { id: 2, why: "Connection refused (os error 111)".into() },
            Frame::Listening { listen: 6, port: 40_000 },
            Frame::Accepted { listen: 4, stream: 1, address: "127.0.0.1".into(), port: 51_234 },
            Frame::Data { stream: 1, bytes: b"GET / HTTP/1.0\r\n\r\n".to_vec() },
            Frame::Data { stream: 3, bytes: vec![0xa5; PAYLOAD_MAX] },
            Frame::Eof { stream: 1 },
            Frame::Close { stream: 2 },
            Frame::Credit { stream: 1, bytes: CREDIT },
        ]
    }

    #[test]
    fn every_kind_reads_back_the_same() {
        for frame in every_kind() {
            let mut bytes = Vec::new();
            frame.encode(&mut bytes);
            assert_eq!(Frame::decode(&bytes), Ok(Some((frame.clone(), bytes.len()))), "{frame:?}");
            // Nothing is read until all of it came.
            for end in 0..bytes.len() {
                assert_eq!(Frame::decode(&bytes[..end]), Ok(None), "{frame:?} {end}");
            }
        }
    }

    #[test]
    fn the_header_is_kind_number_and_length() {
        let mut bytes = Vec::new();
        Frame::Credit { stream: 0x0102_0304, bytes: 5 }.encode(&mut bytes);
        assert_eq!(bytes, [11, 1, 2, 3, 4, 0, 0, 0, 4, 0, 0, 0, 5]);
        bytes.clear();
        Frame::Connect { stream: 2, host: "a".into(), port: 22 }.encode(&mut bytes);
        assert_eq!(bytes, [1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 22, b'a']);
    }

    #[test]
    fn each_side_sends_its_own_kinds() {
        let frames = every_kind();
        let sshd: Vec<bool> = frames.iter().map(Frame::from_sshd).collect();
        let forwarder: Vec<bool> = frames.iter().map(Frame::from_forwarder).collect();
        assert_eq!(
            sshd,
            [true, true, true, true, false, false, false, false, true, true, true, true, true]
        );
        assert_eq!(
            forwarder,
            [false, false, false, false, true, true, true, true, true, true, true, true, true]
        );
    }

    #[test]
    fn what_no_side_sends_is_malformed() {
        let header = |kind: u8, length: u32| {
            let mut bytes = vec![kind, 0, 0, 0, 2];
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes
        };
        let with = |kind: u8, payload: &[u8]| {
            let mut bytes = header(kind, payload.len() as u32);
            bytes.extend_from_slice(payload);
            bytes
        };
        // Too long, before the rest comes.
        assert!(Frame::decode(&header(DATA, PAYLOAD_MAX as u32 + 1)).is_err());
        for bytes in [
            with(0, b""),
            with(12, b""),
            with(DATA, b""),
            with(CONNECT, &[0, 80]),
            with(CONNECT, &[0]),
            with(CONNECT, b"\0\x50a\nb"),
            with(CONNECT, &[0, 80, 0xff]),
            with(EOF, b"x"),
            with(CLOSE, b"x"),
            with(CONNECTED, b"x"),
            with(UNLISTEN, b"x"),
            with(CREDIT_KIND, &[0, 0, 0, 0]),
            with(CREDIT_KIND, &[0, 0, 1]),
            with(LISTENING, &[0, 80, b'x']),
            with(ACCEPTED, &[0, 0, 0]),
            with(FAILED, &[b'x'; WHY_MAX + 1]),
        ] {
            assert!(Frame::decode(&bytes).is_err(), "{bytes:?}");
        }
        let long = Frame::Connect { stream: 2, host: "h".repeat(HOST_MAX + 9), port: 1 };
        let mut bytes = Vec::new();
        long.encode(&mut bytes);
        // A host too long is cut as it is sent.
        let Ok(Some((Frame::Connect { host, .. }, _))) = Frame::decode(&bytes) else {
            panic!("{bytes:?}");
        };
        assert_eq!(host.len(), HOST_MAX);
    }

    #[test]
    fn frames_come_out_as_their_bytes_come_in() {
        let mut bytes = Vec::new();
        for frame in every_kind() {
            frame.encode(&mut bytes);
        }
        let mut frames = Frames::default();
        let mut read = Vec::new();
        for chunk in bytes.chunks(7) {
            frames.push(chunk);
            while let Some(frame) = frames.pop().unwrap() {
                read.push(frame);
            }
        }
        assert_eq!(read, every_kind());
        assert!(!frames.partial());
        frames.push(&[DATA, 0]);
        assert_eq!(frames.pop(), Ok(None));
        assert!(frames.partial());
    }

    #[test]
    fn data_is_split_into_frames() {
        let mut out = Vec::new();
        data(5, &vec![7; PAYLOAD_MAX * 2 + 3], &mut out);
        let mut frames = Frames::default();
        frames.push(&out);
        let mut lengths = Vec::new();
        while let Some(Frame::Data { stream: 5, bytes }) = frames.pop().unwrap() {
            lengths.push(bytes.len());
        }
        assert_eq!(lengths, [PAYLOAD_MAX, PAYLOAD_MAX, 3]);
    }
}
