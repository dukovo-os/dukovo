// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! S3-compatible storage as a destination's store (ADR 0465): Amazon
//! S3, Backblaze B2, Cloudflare R2, MinIO, Garage and the rest of the
//! stores that speak S3's interface. Each file is an object under the
//! machine's folder, as `[PREFIX/]dukovo-backup/MACHINE/PATH` in the
//! bucket, reached by path (`/BUCKET/KEY` on the store's host), and each
//! request is signed with AWS Signature Version 4.
//!
//! An object is written whole by one PUT once its file is finished, so
//! no other request ever sees half of one. A rename is a copy on the
//! store and a removal.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::Path;

use ring::{digest, hmac};
use tuf::Url;
use tuf::http::{self, Answer};

use super::store::{FOLDER, Sink, Store};
use crate::util::{self, Fail};

/// The region signed for when the destination names none: the one
/// MinIO, Garage and most stores other than Amazon's take.
pub const REGION: &str = "us-east-1";

/// The most keys one list request returns, S3's own limit.
const PAGE: usize = 1000;

/// The SHA-256 of nothing, a request's payload hash when it sends none.
const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// A bucket on an S3-compatible store: `s3://HOST[:PORT]/BUCKET[/PREFIX]`
/// over HTTPS, or `s3+http://...` over plain HTTP, for a store on the
/// machine's own network that has no certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// Whether requests go over TLS.
    pub https: bool,
    /// The host, and its port if the address names one.
    pub host: String,
    pub port: Option<u16>,
    pub bucket: String,
    /// The folder of keys under the bucket, without slashes at its ends;
    /// empty for the bucket's top.
    pub prefix: String,
}

impl Address {
    /// The address `text` names, if it is an S3 address at all.
    pub fn parse(text: &str) -> Result<Option<Address>, String> {
        let (https, rest) = if let Some(rest) = text.strip_prefix("s3://") {
            (true, rest)
        } else if let Some(rest) = text.strip_prefix("s3+http://") {
            (false, rest)
        } else {
            return Ok(None);
        };
        let bad = || format!("'{text}' is not s3://HOST[:PORT]/BUCKET[/PREFIX]");
        let (authority, path) = rest.split_once('/').ok_or_else(bad)?;
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => {
                let port: u16 = port.parse().map_err(|_| bad())?;
                (host, Some(port))
            }
            _ => (authority, None),
        };
        let fine_host = |host: &str| {
            !host.is_empty()
                && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        };
        let (bucket, prefix) = path.split_once('/').unwrap_or((path, ""));
        let prefix = prefix.trim_matches('/');
        // S3's bucket names: 3 to 63 lower-case letters, digits, dots and
        // hyphens.
        let fine_bucket = (3..=63).contains(&bucket.len())
            && bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.".contains(&b));
        let fine_prefix = prefix
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." || prefix.is_empty())
            && !prefix.bytes().any(|b| b.is_ascii_control());
        if !fine_host(host) || !fine_bucket || !fine_prefix {
            return Err(bad());
        }
        Ok(Some(Address {
            https,
            host: host.to_ascii_lowercase(),
            port,
            bucket: bucket.into(),
            prefix: prefix.into(),
        }))
    }

    /// The address as written.
    pub fn text(&self) -> String {
        let scheme = if self.https { "s3" } else { "s3+http" };
        let port = self.port.map_or_else(String::new, |p| format!(":{p}"));
        let prefix =
            if self.prefix.is_empty() { String::new() } else { format!("/{}", self.prefix) };
        format!("{scheme}://{}{port}/{}{prefix}", self.host, self.bucket)
    }
}

/// The keys requests are signed with: an access key's ID and its secret,
/// and the region the bucket is in.
#[derive(Clone)]
pub struct Credentials {
    pub id: String,
    pub secret: String,
    pub region: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("id", &self.id)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// Reads the file `path`: lines of `id = ...`, `secret = ...` and,
    /// optionally, `region = ...`, which `backup add` writes.
    pub fn read(path: &Path) -> Result<Credentials, Fail> {
        let text = fs::read_to_string(path).map_err(|e| util::failed(&path.display(), e))?;
        Credentials::parse(&text).map_err(|why| util::fail(format!("{}: {why}", path.display())))
    }

    /// Credentials from the text of their file.
    pub fn parse(text: &str) -> Result<Credentials, String> {
        let (mut id, mut secret, mut region) = (None, None, None);
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (key, value) =
                line.split_once('=').ok_or_else(|| format!("'{line}' is not KEY = VALUE"))?;
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "id" => id = Some(value),
                "secret" => secret = Some(value),
                "region" => region = Some(value),
                other => return Err(format!("{other} is not id, secret or region")),
            }
        }
        match (id, secret) {
            (Some(id), Some(secret)) => Credentials::new(id, secret, region),
            _ => Err("it needs an id and a secret".into()),
        }
    }

    /// The credentials of the access key `id` with the secret `secret`,
    /// for the region `region`, or [`REGION`] without one.
    pub fn new(id: String, secret: String, region: Option<String>) -> Result<Credentials, String> {
        let fine = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic());
        let region = region.unwrap_or_else(|| REGION.into());
        if !fine(&id) || !fine(&secret) {
            return Err("an access key's id and secret are letters, digits and signs".into());
        }
        if !fine(&region) {
            return Err(format!("'{region}' is not a region"));
        }
        Ok(Credentials { id, secret, region })
    }

    /// The text of their file.
    pub fn text(&self) -> String {
        format!("id = {}\nsecret = {}\nregion = {}\n", self.id, self.secret, self.region)
    }
}

/// `text` with every byte but S3's unreserved ones percent-encoded, and
/// `/` kept when `keep_slash`, as Signature Version 4 encodes paths and
/// queries.
fn encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) || (keep_slash && b == b'/') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The hex SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, data).as_ref())
}

/// A request to sign: its method, its path already encoded, its query's
/// pairs, its headers besides the signature, by lower-case name, and its
/// payload's hash.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a [(&'a str, &'a str)],
    pub headers: &'a [(&'a str, &'a str)],
    pub payload: &'a str,
}

impl Request<'_> {
    /// The encoded query, its pairs sorted, as both the request and its
    /// signature have it.
    pub fn query_text(&self) -> String {
        let mut pairs: Vec<(String, String)> =
            self.query.iter().map(|(k, v)| (encode(k, false), encode(v, false))).collect();
        pairs.sort();
        let pairs: Vec<String> = pairs.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.join("&")
    }

    /// The Authorization header of the request, signed with `keys` at
    /// `date`, `YYYYMMDDTHHMMSSZ` in UTC, which its x-amz-date header
    /// must give too.
    pub fn authorization(&self, keys: &Credentials, date: &str) -> String {
        let mut headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        headers.sort();
        let mut canonical_headers = String::new();
        for (name, value) in &headers {
            canonical_headers.push_str(name);
            canonical_headers.push(':');
            canonical_headers.push_str(value);
            canonical_headers.push('\n');
        }
        let signed: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        let signed = signed.join(";");
        let canonical = format!(
            "{}\n{}\n{}\n{canonical_headers}\n{signed}\n{}",
            self.method,
            self.path,
            self.query_text(),
            self.payload
        );
        let day = date.get(..8).unwrap_or(date);
        let scope = format!("{day}/{}/s3/aws4_request", keys.region);
        let to_sign =
            format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", sha256(canonical.as_bytes()));
        let mac = |key: &[u8], data: &str| {
            hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes()).as_ref().to_vec()
        };
        let key = mac(format!("AWS4{}", keys.secret).as_bytes(), day);
        let key = mac(&key, &keys.region);
        let key = mac(&key, "s3");
        let key = mac(&key, "aws4_request");
        let signature = hex(&mac(&key, &to_sign));
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            keys.id
        )
    }
}

/// What the store said, for a message: its error's code and message, or
/// the status alone.
fn said(status: u16, body: &str) -> String {
    match (element(body, "Code"), element(body, "Message")) {
        (Some(code), Some(message)) => format!("HTTP {status}, {code}: {message}"),
        (Some(code), None) => format!("HTTP {status}, {code}"),
        _ => format!("HTTP {status}"),
    }
}

/// The text of the first element `name` in `xml`, unescaped.
fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(unescape(&xml[start..end]))
}

/// Each `<Contents>` element of a list's answer.
fn contents(xml: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Contents>") {
        let after = &rest[start + "<Contents>".len()..];
        let Some(end) = after.find("</Contents>") else { break };
        found.push(&after[..end]);
        rest = &after[end..];
    }
    found
}

/// XML's escapes undone: the five named ones and numbered ones.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A bucket's folder of keys as a store: the machine's folder in it, or
/// the folder the address names.
#[derive(Clone)]
pub struct S3 {
    address: Address,
    keys: Credentials,
    /// The machine's folder's keys start with this, ending in `/`.
    root: String,
}

impl S3 {
    /// The store of `address`, signed with `keys`, for machine
    /// `machine`'s folder in it, or with no machine, the folder the
    /// address names.
    pub fn new(address: Address, keys: Credentials, machine: Option<&str>) -> S3 {
        let mut root = address.prefix.clone();
        if let Some(machine) = machine {
            if !root.is_empty() {
                root.push('/');
            }
            root.push_str(&format!("{FOLDER}/{machine}"));
        }
        if !root.is_empty() {
            root.push('/');
        }
        S3 { address, keys, root }
    }

    /// The URL of the bucket's path `path` (encoded), with the query
    /// `query` (encoded).
    fn url(&self, path: &str, query: &str) -> Result<Url, Fail> {
        let scheme = if self.address.https { "https" } else { "http" };
        let port = self.address.port.map_or_else(String::new, |p| format!(":{p}"));
        let query = if query.is_empty() { String::new() } else { format!("?{query}") };
        let text = format!("{scheme}://{}{port}{path}{query}", self.address.host);
        Url::parse(&text).map_err(|e| util::fail(e.to_string()))
    }

    /// The encoded path of the object `key`.
    fn object(&self, key: &str) -> String {
        format!("/{}/{}", self.address.bucket, encode(key, true))
    }

    /// Sends a signed request; the store's answer.
    fn send(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        extra: &[(&str, &str)],
        body: &[u8],
    ) -> Result<Answer, Fail> {
        let date =
            util::time_text(util::now_ns().cast_signed() / 1_000_000_000, "%Y%m%dT%H%M%SZ", true);
        let payload = if body.is_empty() { EMPTY.to_string() } else { sha256(body) };
        let mut url = self.url(path, "")?;
        let host = http::host_header(&url);
        let mut headers: Vec<(&str, &str)> =
            vec![("host", &host), ("x-amz-content-sha256", &payload), ("x-amz-date", &date)];
        headers.extend_from_slice(extra);
        let request = Request { method, path, query, headers: &headers, payload: &payload };
        let query_text = request.query_text();
        if !query_text.is_empty() {
            url = self.url(path, &query_text)?;
        }
        let authorization = request.authorization(&self.keys, &date);
        let mut sent: Vec<(&str, &str)> =
            headers.iter().filter(|(name, _)| *name != "host").copied().collect();
        sent.push(("Authorization", &authorization));
        http::exchange(method, &url, &sent, body)
            .map_err(|e| util::fail(format!("{}: {e}", self.place())))
    }

    /// Fails with what the store said, unless `answer` has the status
    /// `good`; its body otherwise, read whole.
    fn expect(&self, mut answer: Answer, good: &[u16], what: &str) -> Result<Vec<u8>, Fail> {
        let mut body = Vec::new();
        answer.body.read_to_end(&mut body).map_err(|e| util::failed(&self.place(), e))?;
        if good.contains(&answer.status) {
            return Ok(body);
        }
        let text = String::from_utf8_lossy(&body);
        Err(util::fail(format!("{}: {what}: {}", self.place(), said(answer.status, &text))))
    }

    /// Every key under `prefix` and its size, from the bucket's top.
    fn keys_under(&self, prefix: &str) -> Result<Vec<(String, u64)>, Fail> {
        let mut found = Vec::new();
        let mut token: Option<String> = None;
        let path = format!("/{}", self.address.bucket);
        let max = PAGE.to_string();
        loop {
            let mut query =
                vec![("list-type", "2"), ("prefix", prefix), ("max-keys", max.as_str())];
            if let Some(token) = &token {
                query.push(("continuation-token", token));
            }
            let answer = self.send("GET", &path, &query, &[], &[])?;
            let body = self.expect(answer, &[200], "listing the folder")?;
            let xml = String::from_utf8_lossy(&body);
            for item in contents(&xml) {
                let (Some(key), Some(size)) = (element(item, "Key"), element(item, "Size")) else {
                    continue;
                };
                found.push((key, size.parse().unwrap_or(0)));
            }
            match (element(&xml, "IsTruncated").as_deref(), element(&xml, "NextContinuationToken"))
            {
                (Some("true"), Some(next)) => token = Some(next),
                _ => break,
            }
        }
        Ok(found)
    }
}

/// A file being written, sent by one PUT once whole.
struct S3Sink {
    store: S3,
    key: String,
    data: Vec<u8>,
}

impl Write for S3Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.data.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Sink for S3Sink {
    fn finish(self: Box<Self>) -> Result<(), Fail> {
        self.store.put(&self.key, &self.data)
    }
}

impl S3 {
    fn put(&self, key: &str, data: &[u8]) -> Result<(), Fail> {
        let answer = self.send("PUT", &self.object(key), &[], &[], data)?;
        self.expect(answer, &[200], &format!("writing {key}")).map(drop)
    }

    fn delete(&self, key: &str) -> Result<(), Fail> {
        let answer = self.send("DELETE", &self.object(key), &[], &[], &[])?;
        self.expect(answer, &[200, 204, 404], &format!("removing {key}")).map(drop)
    }

    fn get(&self, key: &str, range: Option<(u64, u64)>) -> Result<Vec<u8>, Fail> {
        let range_text =
            range.map(|(start, length)| format!("bytes={start}-{}", start + length - 1));
        let extra: Vec<(&str, &str)> = range_text.iter().map(|r| ("range", r.as_str())).collect();
        let answer = self.send("GET", &self.object(key), &[], &extra, &[])?;
        let good: &[u16] = if range.is_some() { &[200, 206] } else { &[200] };
        let status = answer.status;
        let body = self.expect(answer, good, &format!("reading {key}"))?;
        // A store that ignores Range sends the whole object.
        match range {
            Some((start, length)) if status == 200 => {
                let start = usize::try_from(start).unwrap_or(usize::MAX).min(body.len());
                let end = start
                    .saturating_add(usize::try_from(length).unwrap_or(usize::MAX))
                    .min(body.len());
                Ok(body[start..end].to_vec())
            }
            _ => Ok(body),
        }
    }
}

impl Store for S3 {
    fn place(&self) -> String {
        let mut text = self.address.text();
        let folder = self.root.trim_end_matches('/');
        let shown =
            folder.strip_prefix(&self.address.prefix).unwrap_or(folder).trim_start_matches('/');
        if !shown.is_empty() {
            text.push('/');
            text.push_str(shown);
        }
        text
    }

    fn list(&mut self) -> Result<Vec<String>, Fail> {
        Ok(self.list_sizes("")?.into_iter().map(|(path, _)| path).collect())
    }

    fn list_sizes(&mut self, folder: &str) -> Result<Vec<(String, u64)>, Fail> {
        let under =
            if folder.is_empty() { self.root.clone() } else { format!("{}{folder}/", self.root) };
        let mut found: Vec<(String, u64)> = self
            .keys_under(&under)?
            .into_iter()
            .filter_map(|(key, size)| Some((key.strip_prefix(&self.root)?.to_string(), size)))
            .filter(|(path, _)| !path.is_empty() && !path.ends_with('/'))
            .collect();
        found.sort();
        Ok(found)
    }

    fn size(&mut self, path: &str) -> Result<Option<u64>, Fail> {
        let key = format!("{}{path}", self.root);
        Ok(self
            .keys_under(&key)?
            .into_iter()
            .find(|(found, _)| *found == key)
            .map(|(_, size)| size))
    }

    fn make_folders(&mut self, _folders: &[String]) -> Result<(), Fail> {
        // A bucket has no folders, only keys with slashes in them.
        Ok(())
    }

    fn read(&mut self, path: &str) -> Result<Box<dyn Read>, Fail> {
        let data = self.get(&format!("{}{path}", self.root), None)?;
        Ok(Box::new(Cursor::new(data)))
    }

    fn read_range(&mut self, path: &str, start: u64, length: u64) -> Result<Vec<u8>, Fail> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let data = self.get(&format!("{}{path}", self.root), Some((start, length)))?;
        if (data.len() as u64) < length {
            return Err(util::fail(format!("{}: {path}: shorter than it should be", self.place())));
        }
        Ok(data)
    }

    fn create(&mut self, path: &str) -> Result<Box<dyn Sink>, Fail> {
        let key = format!("{}{path}", self.root);
        Ok(Box::new(S3Sink { store: self.clone(), key, data: Vec::new() }))
    }

    fn put_whole(&mut self, path: &str, data: &[u8]) -> Result<(), Fail> {
        self.put(&format!("{}{path}", self.root), data)
    }

    fn rename_all(&mut self, pairs: &[(String, String)]) -> Result<(), Fail> {
        for (from, to) in pairs {
            let from = format!("{}{from}", self.root);
            let to = format!("{}{to}", self.root);
            let source = format!("/{}/{}", self.address.bucket, encode(&from, true));
            let answer =
                self.send("PUT", &self.object(&to), &[], &[("x-amz-copy-source", &source)], &[])?;
            let body = self.expect(answer, &[200], &format!("copying {from}"))?;
            // A copy can fail after its 200, with an error in the body.
            let text = String::from_utf8_lossy(&body);
            if text.contains("<Error>") {
                return Err(util::fail(format!(
                    "{}: copying {from}: {}",
                    self.place(),
                    said(200, &text)
                )));
            }
            self.delete(&from)?;
        }
        Ok(())
    }

    fn remove(&mut self, paths: &[String]) -> Result<(), Fail> {
        for path in paths {
            let key = format!("{}{path}", self.root);
            for (inner, _) in self.keys_under(&format!("{key}/"))? {
                self.delete(&inner)?;
            }
            self.delete(&key)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS's own example keys and requests, from the S3 documentation of
    /// Signature Version 4 ("Examples: Signature Calculations in AWS
    /// Signature Version 4").
    fn aws() -> Credentials {
        Credentials {
            id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
        }
    }

    #[test]
    fn requests_are_signed_as_aws_documents() {
        let date = "20130524T000000Z";
        let get = Request {
            method: "GET",
            path: "/test.txt",
            query: &[],
            headers: &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", EMPTY),
                ("x-amz-date", date),
            ],
            payload: EMPTY,
        };
        assert!(get.authorization(&aws(), date).ends_with(
            "SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        ));
        let list = Request {
            method: "GET",
            path: "/",
            query: &[("prefix", "J"), ("max-keys", "2")],
            headers: &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", EMPTY),
                ("x-amz-date", date),
            ],
            payload: EMPTY,
        };
        assert_eq!(list.query_text(), "max-keys=2&prefix=J");
        assert_eq!(
            list.authorization(&aws(), date),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, \
             Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
        let body = b"Welcome to Amazon S3.";
        let payload = sha256(body);
        assert_eq!(payload, "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072");
        let put = Request {
            method: "PUT",
            path: &encode("/test$file.text", true),
            query: &[],
            headers: &[
                ("date", "Fri, 24 May 2013 00:00:00 GMT"),
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", &payload),
                ("x-amz-date", date),
                ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ],
            payload: &payload,
        };
        assert!(put.authorization(&aws(), date).ends_with(
            "Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        ));
    }

    #[test]
    fn addresses_are_read_strictly() {
        let address = Address::parse("s3://s3.eu-central-1.amazonaws.com/my-backups/laptops/")
            .unwrap()
            .unwrap();
        assert!(address.https);
        assert_eq!(address.host, "s3.eu-central-1.amazonaws.com");
        assert_eq!(
            (address.bucket.as_str(), address.prefix.as_str(), address.port),
            ("my-backups", "laptops", None)
        );
        assert_eq!(address.text(), "s3://s3.eu-central-1.amazonaws.com/my-backups/laptops");
        let local = Address::parse("s3+http://10.0.2.2:9000/backups").unwrap().unwrap();
        assert!(!local.https);
        assert_eq!((local.port, local.prefix.as_str()), (Some(9000), ""));
        assert_eq!(Address::parse("/volumes/STICK").unwrap(), None);
        for bad in [
            "s3://host",
            "s3://host/B",
            "s3://host/ab",
            "s3://host:port/bucket",
            "s3://ho st/bucket",
            "s3://host/bucket/../x",
            "s3:///bucket",
        ] {
            assert!(Address::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn credentials_go_through_their_file() {
        let keys =
            Credentials::parse("# made by backup add\nid = AKID\nsecret = s3cr/t+\n").unwrap();
        assert_eq!(
            (keys.id.as_str(), keys.secret.as_str(), keys.region.as_str()),
            ("AKID", "s3cr/t+", REGION)
        );
        assert_eq!(Credentials::parse(&keys.text()).unwrap().secret, "s3cr/t+");
        assert!(Credentials::parse("id = AKID\n").is_err());
        assert!(Credentials::parse("id = AKID\nsecret = x\ntoken = y\n").is_err());
        assert!(!format!("{keys:?}").contains("s3cr"));
    }

    #[test]
    fn lists_and_errors_are_read() {
        let xml = "<?xml version=\"1.0\"?><ListBucketResult><IsTruncated>true</IsTruncated>\
                   <Contents><Key>a/b&amp;c</Key><Size>12</Size></Contents>\
                   <Contents><Key>a/d</Key><Size>0</Size></Contents>\
                   <NextContinuationToken>t&#x2F;1</NextContinuationToken></ListBucketResult>";
        let items = contents(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(element(items[0], "Key").as_deref(), Some("a/b&c"));
        assert_eq!(element(items[0], "Size").as_deref(), Some("12"));
        assert_eq!(element(xml, "NextContinuationToken").as_deref(), Some("t/1"));
        let error = "<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>";
        assert_eq!(said(403, error), "HTTP 403, AccessDenied: Access Denied");
        assert_eq!(said(500, ""), "HTTP 500");
    }

    #[test]
    fn keys_go_under_the_machines_folder() {
        let address = Address::parse("s3://host/bucket/team").unwrap().unwrap();
        let store = S3::new(address.clone(), aws(), Some("atlas"));
        assert_eq!(store.root, "team/dukovo-backup/atlas/");
        assert_eq!(store.place(), "s3://host/bucket/team/dukovo-backup/atlas");
        assert_eq!(
            store.object("team/dukovo-backup/atlas/data/ab cd"),
            "/bucket/team/dukovo-backup/atlas/data/ab%20cd"
        );
        let top = S3::new(Address::parse("s3://host/bucket").unwrap().unwrap(), aws(), None);
        assert_eq!((top.root.as_str(), top.place()), ("", "s3://host/bucket".to_string()));
    }
}
