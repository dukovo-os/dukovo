// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Just enough HTTP/1.1 to talk to the lab's net-boot server: a GET or a
//! POST to a path under the boot's base URL, one connection each, with
//! the answer's body sized by its Content-Length, as the lab's server
//! always sends it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

/// How long the lab PC may take to answer, or to send the next part of
/// the data.
const READ_TIME: Duration = Duration::from_secs(60);

/// How long the writer keeps trying to reach the lab PC: the switch port
/// and the lab PC's ARP cache can take some seconds after the link comes
/// up.
const CONNECT_TIME: Duration = Duration::from_secs(60);

/// The longest answer head read.
const HEAD_LIMIT: usize = 16 * 1024;

/// The longest answer body held in memory (the plan, a word): the data
/// itself is streamed with [`Client::open`].
const BODY_LIMIT: u64 = 16 << 20;

/// The base URL of one boot, `http://<address>:<port>/<token>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub server: SocketAddrV4,
    /// The path the URL names, starting and ending with `/`.
    pub path: String,
}

impl Base {
    /// Reads a base URL, which must be plain HTTP to an IPv4 address: the
    /// lab network has no names.
    pub fn parse(url: &str) -> Result<Base, String> {
        let rest = url.strip_prefix("http://").ok_or_else(|| format!("{url} is not http://"))?;
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let server = if authority.contains(':') {
            authority.parse::<SocketAddrV4>().ok()
        } else {
            authority.parse().ok().map(|address| SocketAddrV4::new(address, 80))
        };
        let server = server.ok_or_else(|| format!("{url} does not name an IPv4 address"))?;
        let mut path = format!("/{path}");
        if !path.ends_with('/') {
            path.push('/');
        }
        if path.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control()) {
            return Err(format!("{url} holds spaces"));
        }
        Ok(Base { server, path })
    }

    /// The URL, for messages.
    pub fn url(&self) -> String {
        format!("http://{}{}", self.server, self.path)
    }
}

/// An answer held whole.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Answer {
    /// The body as text, trimmed, for messages.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }
}

/// The lab's net-boot server for one boot.
pub struct Client {
    base: Base,
}

impl Client {
    pub fn new(base: Base) -> Client {
        Client { base }
    }

    /// GETs `what` and holds the answer.
    pub fn get(&self, what: &str) -> Result<Answer, String> {
        self.whole("GET", what, None)
    }

    /// POSTs `body` (of `content_type`) to `what` and holds the answer.
    pub fn post(&self, what: &str, content_type: &str, body: &[u8]) -> Result<Answer, String> {
        self.whole("POST", what, Some((content_type, body)))
    }

    /// GETs `what`, and gives its status, its body's length and the body
    /// to read, for an answer too big to hold.
    pub fn open(&self, what: &str) -> Result<(u16, u64, impl Read), String> {
        let mut stream = self.connect()?;
        send(&mut stream, &self.request("GET", what, None))?;
        let mut reader = BufReader::new(stream);
        let (status, length) = read_head(&mut reader, what)?;
        Ok((status, length, reader.take(length)))
    }

    fn whole(
        &self,
        method: &str,
        what: &str,
        body: Option<(&str, &[u8])>,
    ) -> Result<Answer, String> {
        let mut stream = self.connect()?;
        let mut request = self.request(method, what, body.map(|(kind, body)| (kind, body.len())));
        if let Some((_, body)) = body {
            request.extend_from_slice(body);
        }
        send(&mut stream, &request)?;
        let mut reader = BufReader::new(stream);
        let (status, length) = read_head(&mut reader, what)?;
        if length > BODY_LIMIT {
            return Err(format!("the answer to {what} is {length} bytes, too long to hold"));
        }
        let mut body = Vec::new();
        reader
            .take(length)
            .read_to_end(&mut body)
            .map_err(|err| format!("reading the answer to {what}: {err}"))?;
        if (body.len() as u64) < length {
            return Err(format!("the answer to {what} broke off"));
        }
        Ok(Answer { status, body })
    }

    /// The request's head, for a body of the given type and length.
    fn request(&self, method: &str, what: &str, body: Option<(&str, usize)>) -> Vec<u8> {
        let mut head = format!(
            "{method} {}{what} HTTP/1.1\r\nHost: {}\r\nUser-Agent: dukovo-lab-writer\r\n\
             Connection: close\r\n",
            self.base.path, self.base.server
        );
        if let Some((kind, length)) = body {
            head.push_str(&format!("Content-Type: {kind}\r\nContent-Length: {length}\r\n"));
        }
        head.push_str("\r\n");
        head.into_bytes()
    }

    /// Connects to the lab PC, trying again for [`CONNECT_TIME`].
    fn connect(&self) -> Result<TcpStream, String> {
        let deadline = Instant::now() + CONNECT_TIME;
        loop {
            match TcpStream::connect_timeout(&self.base.server.into(), Duration::from_secs(5)) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(READ_TIME)).map_err(|err| err.to_string())?;
                    stream.set_write_timeout(Some(READ_TIME)).map_err(|err| err.to_string())?;
                    return Ok(stream);
                }
                Err(err) if Instant::now() >= deadline => {
                    return Err(format!("reaching the lab PC at {}: {err}", self.base.server));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }
}

fn send(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), String> {
    stream.write_all(bytes).and_then(|()| stream.flush()).map_err(|err| format!("sending: {err}"))
}

/// Reads an answer's head: its status and Content-Length.
fn read_head(reader: &mut impl BufRead, what: &str) -> Result<(u16, u64), String> {
    let mut read = 0;
    let mut line = String::new();
    let mut next = |line: &mut String| -> Result<(), String> {
        line.clear();
        let got = reader
            .take((HEAD_LIMIT - read) as u64)
            .read_line(line)
            .map_err(|err| format!("reading the answer to {what}: {err}"))?;
        if got == 0 || !line.ends_with('\n') {
            return Err(format!("the answer to {what} broke off in its head"));
        }
        read += got;
        Ok(())
    };
    next(&mut line)?;
    let status = line
        .strip_prefix("HTTP/1.")
        .and_then(|rest| rest.get(2..5))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("the answer to {what} is not HTTP: {}", line.trim()))?;
    let mut length = None;
    loop {
        next(&mut line)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<u64>().ok();
        }
    }
    let length = length.ok_or_else(|| format!("the answer to {what} has no Content-Length"))?;
    Ok((status, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn reads_a_base_url() {
        let base = Base::parse("http://10.77.0.1:7431/0123abcd/").unwrap();
        assert_eq!(base.server, SocketAddrV4::new(Ipv4Addr::new(10, 77, 0, 1), 7431));
        assert_eq!(base.path, "/0123abcd/");
        assert_eq!(base.url(), "http://10.77.0.1:7431/0123abcd/");
        assert_eq!(Base::parse("http://10.77.0.1/t").unwrap().path, "/t/");
        assert_eq!(Base::parse("http://10.77.0.1").unwrap().server.port(), 80);
        assert!(Base::parse("https://10.77.0.1/").unwrap_err().contains("not http://"));
        assert!(Base::parse("http://lab.example/").unwrap_err().contains("IPv4"));
    }

    /// A server that answers each of `answers` in turn, and gives back
    /// the requests it read.
    fn server(answers: Vec<&'static str>) -> (Base, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            unreachable!()
        };
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0u8; 4096];
                let read = stream.read(&mut request).unwrap();
                requests.push(String::from_utf8_lossy(&request[..read]).into_owned());
                stream.write_all(answer.as_bytes()).unwrap();
            }
            requests
        });
        (Base { server: address, path: "/tok/".into() }, thread)
    }

    #[test]
    fn gets_posts_and_streams() {
        let (base, server) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 409 Conflict\r\ncontent-length: 11\r\nConnection: close\r\n\r\nwrong hash\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabcdeEXTRA",
        ]);
        let client = Client::new(base);
        let plan = client.get("plan").unwrap();
        assert_eq!((plan.status, plan.body.as_slice()), (200, b"{}".as_slice()));
        let done = client.post("done", "application/json", br#"{"sha256":"00"}"#).unwrap();
        assert_eq!((done.status, done.text().as_str()), (409, "wrong hash"));
        let (status, length, mut data) = client.open("data").unwrap();
        let mut bytes = Vec::new();
        data.read_to_end(&mut bytes).unwrap();
        assert_eq!((status, length, bytes.as_slice()), (200, 5, b"abcde".as_slice()));
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET /tok/plan HTTP/1.1\r\n"), "{}", requests[0]);
        assert!(requests[1].starts_with("POST /tok/done HTTP/1.1\r\n"), "{}", requests[1]);
        assert!(requests[1].contains("Content-Length: 15\r\n"), "{}", requests[1]);
        assert!(requests[1].ends_with("\r\n\r\n{\"sha256\":\"00\"}"), "{}", requests[1]);
    }

    #[test]
    fn refuses_an_answer_without_its_length() {
        let (base, server) = server(vec![
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
            "SSH-2.0-OpenSSH\r\n",
        ]);
        let client = Client::new(base);
        assert!(client.get("plan").unwrap_err().contains("no Content-Length"));
        assert!(client.get("plan").unwrap_err().contains("broke off"));
        assert!(client.get("plan").unwrap_err().contains("not HTTP"));
        server.join().unwrap();
    }
}
