// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The HTTP client instant-acme sends its requests through (ADR 0513):
//! tuf's, which speaks HTTP/1.1 over rustls with the system's roots.
//!
//! A request is run inside the future the library gets back. The runtime
//! is a current-thread one that does one thing at a time, so a request
//! that blocks holds nothing else up. The answer is read whole, up to
//! [`ANSWER_MAX`], with every header it came with: ACME's nonce, location,
//! links and retry time are all headers.
//!
//! When an order fails for a reason the log does not explain, a request to
//! the authority that failed names its host in [`NotReached`]'s words.

use std::fmt;
use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue, USER_AGENT};
use http::{Response, StatusCode};
use http_body_util::BodyExt;
use instant_acme::{BodyWrapper, BytesResponse, Error, HttpClient};
use tuf::http::{self as tuf_http};
use tuf::url::Url;

/// The most bytes of an answer that are read: no answer of an authority
/// comes near it, and a server that sends more is not one.
pub const ANSWER_MAX: u64 = 1024 * 1024;

/// The request failed before the authority answered.
#[derive(Debug)]
pub struct NotReached {
    host: String,
    why: String,
}

impl fmt::Display for NotReached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot reach the authority at {}: {}", self.host, self.why)
    }
}

impl std::error::Error for NotReached {}

/// An answer that is larger than [`ANSWER_MAX`].
#[derive(Debug)]
struct TooLarge;

impl fmt::Display for TooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the authority's answer is larger than {} MiB", ANSWER_MAX / (1024 * 1024))
    }
}

impl std::error::Error for TooLarge {}

/// instant-acme's HTTP client over tuf's.
#[derive(Clone, Default)]
pub struct TufClient {
    /// The Retry-After of the last answer that had one, as text: the
    /// authority says when a rate limit ends there, and the library's
    /// problem has no room for it.
    retry_after: Arc<Mutex<Option<String>>>,
}

impl TufClient {
    /// The Retry-After header of the last answer that had one.
    pub fn retry_after(&self) -> Option<String> {
        self.retry_after.lock().ok().and_then(|held| held.clone())
    }
}

impl HttpClient for TufClient {
    fn request(
        &self,
        request: http::Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, Error>> + Send>> {
        let retry_after = Arc::clone(&self.retry_after);
        Box::pin(async move { send(request, &retry_after).await })
    }
}

/// Sends `request` and builds what the library reads of the answer.
async fn send(
    request: http::Request<BodyWrapper<Bytes>>,
    retry_after: &Mutex<Option<String>>,
) -> Result<BytesResponse, Error> {
    let (parts, body) = request.into_parts();
    // Its error cannot happen: the body is bytes already.
    let Ok(collected) = body.collect().await;
    let body = collected.to_bytes();
    let url = Url::parse(&parts.uri.to_string()).map_err(|why| Error::Other(why.into()))?;
    // The library names itself as the client, and tuf names Dukovo: one
    // User-Agent is sent, tuf's.
    let mut headers = Vec::new();
    for (name, value) in &parts.headers {
        if name == USER_AGENT {
            continue;
        }
        let value = value.to_str().map_err(|_| Error::Str("a header that is not text"))?;
        headers.push((name.as_str(), value));
    }
    let host = tuf_http::host_header(&url);
    let not_reached = |why: &dyn fmt::Display| {
        Error::Other(Box::new(NotReached { host: host.clone(), why: why.to_string() }))
    };
    let mut answer = tuf_http::exchange(parts.method.as_str(), &url, &headers, &body)
        .map_err(|why| not_reached(&why))?;
    let mut bytes = Vec::new();
    answer
        .body
        .by_ref()
        .take(ANSWER_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|why| not_reached(&why))?;
    if bytes.len() as u64 > ANSWER_MAX {
        return Err(Error::Other(Box::new(TooLarge)));
    }
    let mut response = Response::builder()
        .status(StatusCode::from_u16(answer.status).map_err(|_| Error::Str("a status"))?);
    for (name, value) in &answer.headers {
        let name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| Error::Str("a header name"))?;
        let value =
            HeaderValue::from_str(value).map_err(|_| Error::Str("a header that is not text"))?;
        if name == http::header::RETRY_AFTER
            && let Ok(mut held) = retry_after.lock()
        {
            *held = Some(value.to_str().unwrap_or_default().to_string());
        }
        response = response.header(name, value);
    }
    let (parts, ()) = response.body(()).map_err(Error::Http)?.into_parts();
    Ok(BytesResponse { parts, body: Box::new(Bytes::from(bytes)) })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    use instant_acme::Error as AcmeError;

    use super::*;

    /// Serves `answer` to one connection, and gives back the request:
    /// its head and its body.
    fn serve(answer: Vec<u8>) -> (String, thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase().strip_prefix("content-length: ").map(String::from)
                })
                .map_or(0, |length| length.parse().unwrap());
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            stream.write_all(&answer).unwrap();
            (head, body)
        });
        (format!("http://{address}/acme/new-order"), server)
    }

    fn run(
        client: &TufClient,
        method: &str,
        url: &str,
        body: &str,
    ) -> Result<BytesResponse, AcmeError> {
        let request = http::Request::builder()
            .method(method)
            .uri(url)
            .header(USER_AGENT, "instant-acme/0.8.5")
            .header("content-type", "application/jose+json")
            .body(BodyWrapper::from(body.as_bytes().to_vec()))
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(client.request(request))
    }

    fn read_body(response: BytesResponse) -> Bytes {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let mut body = response.body;
        runtime.block_on(body.into_bytes()).unwrap()
    }

    #[test]
    fn a_request_goes_out_whole_and_the_answer_comes_back_with_every_header() {
        let (url, server) = serve(
            b"HTTP/1.1 201 Created\r\nReplay-Nonce: n-1\r\nLocation: https://a.example/o/1\r\n\
              Link: <a>;rel=\"up\"\r\nLink: <b>;rel=\"index\"\r\nRetry-After: 30\r\n\
              Content-Length: 11\r\n\r\n{\"ok\":true}\n"
                .to_vec(),
        );
        let client = TufClient::default();
        let response = run(&client, "POST", &url, "{\"payload\":1}").unwrap();
        assert_eq!(response.parts.status, StatusCode::CREATED);
        let header = |name: &str| -> Vec<&str> {
            response.parts.headers.get_all(name).iter().map(|v| v.to_str().unwrap()).collect()
        };
        assert_eq!(header("replay-nonce"), ["n-1"]);
        assert_eq!(header("location"), ["https://a.example/o/1"]);
        assert_eq!(header("link"), ["<a>;rel=\"up\"", "<b>;rel=\"index\""]);
        assert_eq!(client.retry_after().as_deref(), Some("30"));
        assert_eq!(read_body(response), Bytes::from_static(b"{\"ok\":true}"));
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /acme/new-order HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("content-type: application/jose+json\r\n"), "{head}");
        assert_eq!(body, b"{\"payload\":1}");
        // One User-Agent: tuf's.
        assert_eq!(head.to_ascii_lowercase().matches("user-agent:").count(), 1, "{head}");
        assert!(head.contains("User-Agent: Dukovo/1"), "{head}");
    }

    #[test]
    fn a_head_request_has_no_body_and_a_status_other_than_200_is_an_answer() {
        let (url, server) = serve(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3600\r\nContent-Length: 40\r\n\r\n"
                .to_vec(),
        );
        let client = TufClient::default();
        let response = run(&client, "HEAD", &url, "").unwrap();
        assert_eq!(response.parts.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(read_body(response).is_empty());
        assert_eq!(client.retry_after().as_deref(), Some("3600"));
        assert!(server.join().unwrap().0.starts_with("HEAD "));
    }

    #[test]
    fn an_answer_larger_than_the_cap_is_refused_in_plain_words() {
        let size = usize::try_from(ANSWER_MAX).unwrap() + 1;
        let mut answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\n\r\n").into_bytes();
        answer.extend(std::iter::repeat_n(b'x', size));
        let (url, server) = serve(answer);
        let refused = run(&TufClient::default(), "GET", &url, "").map(|_| ()).unwrap_err();
        assert_eq!(refused.to_string(), "the authority's answer is larger than 1 MiB");
        // The server may stop writing once the client has given up.
        let _ = server.join();
    }

    #[test]
    fn a_server_that_is_not_there_is_named_in_the_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{address}/dir");
        let refused = run(&TufClient::default(), "GET", &url, "").map(|_| ()).unwrap_err();
        let text = refused.to_string();
        assert!(text.starts_with(&format!("cannot reach the authority at {address}: ")), "{text}");
    }
}
