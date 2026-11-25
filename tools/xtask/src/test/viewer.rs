// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A remote viewer of one screen of QEMU's virtio graphics card, as far
//! as a boot test needs one: it asks for the screen to be a size.
//!
//! QEMU shows each screen of the card to a viewer that connects to a
//! socket of that screen's, and speaks the Remote Framebuffer protocol
//! with it (RFC 6143). A viewer may ask for a desktop of another size,
//! with the protocol's extension for desktops that change size, as a
//! person does who resizes the window of a virtual machine. The card
//! then tells the guest that the monitor of that screen wants the size,
//! and that one is plugged in if none was. A size of nothing by nothing
//! is a monitor pulled out.
//!
//! One request is one short connection: the greeting, no password, the
//! size, and the server's word that it took the request. The viewer asks
//! for no picture, so none is sent; a test reads a screen's picture with
//! the machine protocol's `screendump`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// The version of the protocol the viewer speaks, 3.8.
const VERSION: &[u8; 12] = b"RFB 003.008\n";
/// The security type of a server that asks for no password.
const NO_PASSWORD: u8 = 1;
/// The viewer's message that lists the encodings it takes.
const SET_ENCODINGS: u8 = 2;
/// The viewer's message that asks for a desktop of another size.
const SET_DESKTOP_SIZE: u8 = 251;
/// The server's message that carries rectangles.
const UPDATE: u8 = 0;
/// The server's message that rings the bell.
const BELL: u8 = 2;
/// The server's message that carries cut text.
const CUT_TEXT: u8 = 3;
/// The encoding of a rectangle that carries no pixels and says the
/// desktop's size and screens, which a viewer that lists it may ask to
/// change.
const DESKTOP_SIZE: i32 = -308;
/// The column of such a rectangle that answers this viewer's own
/// request.
const ASKED_HERE: u16 = 1;
/// The row of such a rectangle, which is how the request went: done, or
/// passed on to the guest, which changes size when it will.
const DONE: u16 = 0;
const PASSED_ON: u16 = 4;
/// How long the server has to say each thing.
const PATIENCE: Duration = Duration::from_secs(10);

/// Asks the screen whose viewers connect to `socket` to be `width` by
/// `height` pixels, which plugs a monitor of that size into it; 0 by 0
/// pulls the monitor out.
pub fn ask_size(socket: &Path, width: u16, height: u16) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    let stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => return Err(format!("connecting to {}: {err}", socket.display())),
        }
    };
    ask_size_on(stream, width, height).map_err(|why| format!("{}: {why}", socket.display()))
}

/// As [`ask_size`], of the server at the other end of `stream`.
fn ask_size_on(stream: UnixStream, width: u16, height: u16) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let mut viewer = Viewer { stream };
    viewer.greet()?;
    viewer.send(&size_request(width, height))?;
    viewer.answer(width, height)
}

/// The message that asks for a desktop of `width` by `height`, of one
/// screen that fills it.
fn size_request(width: u16, height: u16) -> Vec<u8> {
    let mut message = vec![SET_DESKTOP_SIZE, 0];
    message.extend_from_slice(&width.to_be_bytes());
    message.extend_from_slice(&height.to_be_bytes());
    // One screen, a byte of padding, then the screen: its number, where
    // it is, its size and its flags.
    message.extend_from_slice(&[1, 0]);
    message.extend_from_slice(&0u32.to_be_bytes());
    message.extend_from_slice(&[0; 4]);
    message.extend_from_slice(&width.to_be_bytes());
    message.extend_from_slice(&height.to_be_bytes());
    message.extend_from_slice(&0u32.to_be_bytes());
    message
}

/// One connection to a screen's server.
struct Viewer {
    stream: UnixStream,
}

impl Viewer {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stream.write_all(bytes).map_err(|err| format!("writing to the server: {err}"))
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut bytes = [0; N];
        self.stream
            .read_exact(&mut bytes)
            .map_err(|err| format!("reading from the server: {err}"))?;
        Ok(bytes)
    }

    fn take_u16(&mut self) -> Result<u16, String> {
        self.take().map(u16::from_be_bytes)
    }

    fn take_u32(&mut self) -> Result<u32, String> {
        self.take().map(u32::from_be_bytes)
    }

    /// Reads and drops `count` bytes.
    fn skip(&mut self, count: u64) -> Result<(), String> {
        let mut rest = (&self.stream).take(count);
        match std::io::copy(&mut rest, &mut std::io::sink()) {
            Ok(read) if read == count => Ok(()),
            Ok(_) => Err("the server closed the connection".into()),
            Err(err) => Err(format!("reading from the server: {err}")),
        }
    }

    /// Text the server sends with its length before it.
    fn take_text(&mut self) -> Result<String, String> {
        let length = self.take_u32()?.min(4096);
        let mut text = vec![0; length as usize];
        self.stream
            .read_exact(&mut text)
            .map_err(|err| format!("reading from the server: {err}"))?;
        Ok(String::from_utf8_lossy(&text).into_owned())
    }

    /// The start of a connection: the versions, no password, the
    /// desktop as the server has it, and the one encoding the viewer
    /// takes beside raw pixels.
    fn greet(&mut self) -> Result<(), String> {
        let version: [u8; 12] = self.take()?;
        if !version.starts_with(b"RFB 003.") {
            return Err(format!("the server greets with {:?}", String::from_utf8_lossy(&version)));
        }
        self.send(VERSION)?;
        let [count] = self.take()?;
        if count == 0 {
            return Err(format!("the server refuses viewers: {}", self.take_text()?));
        }
        let mut kinds = vec![0; usize::from(count)];
        self.stream.read_exact(&mut kinds).map_err(|err| format!("reading the server: {err}"))?;
        if !kinds.contains(&NO_PASSWORD) {
            return Err(format!("the server wants a password (security types {kinds:?})"));
        }
        self.send(&[NO_PASSWORD])?;
        if self.take_u32()? != 0 {
            return Err(format!("the server refused the viewer: {}", self.take_text()?));
        }
        // Other viewers of the screen stay.
        self.send(&[1])?;
        // The desktop's size and its pixels' layout, then its name.
        let _: [u8; 20] = self.take()?;
        self.take_text()?;
        let mut encodings = vec![SET_ENCODINGS, 0, 0, 1];
        encodings.extend_from_slice(&DESKTOP_SIZE.to_be_bytes());
        self.send(&encodings)
    }

    /// Reads what the server says until it answers the request for
    /// `width` by `height`.
    fn answer(&mut self, width: u16, height: u16) -> Result<(), String> {
        loop {
            let [kind] = self.take()?;
            match kind {
                UPDATE => {
                    let [_] = self.take()?;
                    for _ in 0..self.take_u16()? {
                        if let Some(how) = self.rectangle()? {
                            return match how {
                                DONE | PASSED_ON => Ok(()),
                                other => Err(format!(
                                    "the server refused a screen of {width}x{height}: status \
                                     {other}"
                                )),
                            };
                        }
                    }
                }
                BELL => {}
                CUT_TEXT => {
                    let _: [u8; 3] = self.take()?;
                    let length = self.take_u32()?;
                    self.skip(u64::from(length))?;
                }
                other => return Err(format!("the server sent a message of kind {other}")),
            }
        }
    }

    /// Reads one rectangle of an update. Returns how this viewer's
    /// request went if the rectangle answers it.
    fn rectangle(&mut self) -> Result<Option<u16>, String> {
        let (why, how) = (self.take_u16()?, self.take_u16()?);
        let _: [u8; 4] = self.take()?;
        let encoding = i32::from_be_bytes(self.take()?);
        if encoding != DESKTOP_SIZE {
            return Err(format!("the server sent pixels nobody asked for (encoding {encoding})"));
        }
        // How many screens, padding, and 16 bytes for each.
        let [screens, ..]: [u8; 4] = self.take()?;
        self.skip(u64::from(screens) * 16)?;
        Ok((why == ASKED_HERE).then_some(how))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that greets as QEMU's does, says the desktop's size once
    /// the viewer lists the encoding, and answers a request with
    /// `status`. Returns the request it read.
    fn serve(mut stream: UnixStream, status: u16) -> Vec<u8> {
        let size = |why: u16, how: u16| {
            let mut update = vec![UPDATE, 0, 0, 1];
            for half in [why, how, 800, 600] {
                update.extend_from_slice(&half.to_be_bytes());
            }
            update.extend_from_slice(&DESKTOP_SIZE.to_be_bytes());
            update.extend_from_slice(&[1, 0, 0, 0]);
            update.extend_from_slice(&[0; 16]);
            update
        };
        stream.write_all(VERSION).unwrap();
        let mut version = [0; 12];
        stream.read_exact(&mut version).unwrap();
        assert_eq!(&version, VERSION);
        stream.write_all(&[1, NO_PASSWORD]).unwrap();
        let mut one = [0; 1];
        stream.read_exact(&mut one).unwrap();
        assert_eq!(one, [NO_PASSWORD]);
        stream.write_all(&0u32.to_be_bytes()).unwrap();
        stream.read_exact(&mut one).unwrap();
        assert_eq!(one, [1], "the viewer shares the screen");
        stream.write_all(&[0; 20]).unwrap();
        stream.write_all(&4u32.to_be_bytes()).unwrap();
        stream.write_all(b"QEMU").unwrap();
        let mut encodings = [0; 8];
        stream.read_exact(&mut encodings).unwrap();
        assert_eq!(encodings, [SET_ENCODINGS, 0, 0, 1, 0xff, 0xff, 0xfe, 0xcc]);
        // The desktop as it is, which no viewer asked for, and a bell.
        stream.write_all(&size(0, 0)).unwrap();
        stream.write_all(&[BELL]).unwrap();
        let mut request = vec![0; 24];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(&size(ASKED_HERE, status)).unwrap();
        request
    }

    fn asked(width: u16, height: u16, status: u16) -> (Result<(), String>, Vec<u8>) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || serve(theirs, status));
        let result = ask_size_on(ours, width, height);
        (result, server.join().unwrap())
    }

    #[test]
    fn a_size_is_asked_for_as_one_screen_that_fills_the_desktop() {
        let (result, request) = asked(1024, 768, PASSED_ON);
        assert_eq!(result, Ok(()));
        let expected = [
            [SET_DESKTOP_SIZE, 0, 4, 0, 3, 0, 1, 0],
            [0, 0, 0, 0, 0, 0, 0, 0],
            [4, 0, 3, 0, 0, 0, 0, 0],
        ];
        assert_eq!(request, expected.concat());
    }

    #[test]
    fn nothing_by_nothing_pulls_the_monitor_out() {
        let (result, request) = asked(0, 0, DONE);
        assert_eq!(result, Ok(()));
        assert_eq!(request[2..6], [0; 4]);
        assert_eq!(request[16..20], [0; 4]);
    }

    #[test]
    fn a_server_that_refuses_the_size_fails_the_request() {
        let (result, _) = asked(640, 480, 3);
        assert_eq!(result, Err("the server refused a screen of 640x480: status 3".to_string()));
    }
}
