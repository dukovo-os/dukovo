// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! URLs of repositories, as far as fetching from them takes: a scheme,
//! a host and port, and a path with its query. Names are joined to a
//! base by putting them after it, as python-tuf does; a redirect's
//! Location is resolved against the URL it answered.

use std::fmt;
use std::path::PathBuf;

/// A URL: `scheme://host[:port]/path[?query]`, without its fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    scheme: String,
    /// As written, an IPv6 address in its brackets; empty for a file URL
    /// on this computer.
    host: String,
    port: Option<u16>,
    /// From its first `/`, with the query.
    path: String,
}

/// Why text is not a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub String);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not a URL this takes", self.0)
    }
}

impl std::error::Error for Malformed {}

impl Url {
    /// Parses `text`, an absolute URL.
    ///
    /// # Errors
    ///
    /// [`Malformed`] if it has no scheme or host (but for a file URL), a
    /// user name, a bad port, or spaces or control characters.
    pub fn parse(text: &str) -> Result<Url, Malformed> {
        let bad = || Malformed(text.to_string());
        if text.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(bad());
        }
        let text_no_fragment = text.split('#').next().unwrap_or_default();
        let (scheme, rest) = text_no_fragment.split_once("://").ok_or_else(bad)?;
        if scheme.is_empty()
            || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        {
            return Err(bad());
        }
        let scheme = scheme.to_ascii_lowercase();
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(end);
        if authority.contains('@') {
            return Err(bad());
        }
        let (host, port) = if authority.starts_with('[') {
            let close = authority.find(']').ok_or_else(bad)?;
            let (host, after) = authority.split_at(close + 1);
            match after.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None if after.is_empty() => (host, None),
                None => return Err(bad()),
            }
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        let port = match port {
            Some(port) => Some(port.parse::<u16>().map_err(|_| bad())?),
            None => None,
        };
        if host.is_empty() && scheme != "file" {
            return Err(bad());
        }
        let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
        Ok(Url { scheme, host: host.to_ascii_lowercase(), port, path })
    }

    /// Its scheme, in lower case.
    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Its host as written, an IPv6 address in brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Its port, if it names one.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Its path, from its first `/`, with its query.
    #[must_use]
    pub fn path_and_query(&self) -> &str {
        &self.path
    }

    /// Its path alone.
    #[must_use]
    pub fn path(&self) -> &str {
        self.path.split('?').next().unwrap_or_default()
    }

    /// This URL with a closing `/` on its path, so that names put after
    /// it are under it.
    #[must_use]
    pub fn base(&self) -> Url {
        let mut url = self.clone();
        let (path, query) = match self.path.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (self.path.as_str(), None),
        };
        if !path.ends_with('/') {
            url.path = match query {
                Some(query) => format!("{path}/?{query}"),
                None => format!("{path}/"),
            };
        }
        url
    }

    /// `reference`, as a redirect's Location gives it, resolved against
    /// this URL: absolute, from this scheme (`//`), from this host
    /// (`/`), or beside this path.
    ///
    /// # Errors
    ///
    /// [`Malformed`] if the result is not a URL.
    pub fn resolve(&self, reference: &str) -> Result<Url, Malformed> {
        let has_scheme = reference
            .find(':')
            .is_some_and(|colon| reference[..colon].bytes().all(|b| b.is_ascii_alphanumeric()));
        if has_scheme && reference.contains("://") {
            return Url::parse(reference);
        }
        if let Some(rest) = reference.strip_prefix("//") {
            return Url::parse(&format!("{}://{rest}", self.scheme));
        }
        let authority = self.authority();
        if reference.starts_with('/') {
            return Url::parse(&format!("{}://{authority}{reference}", self.scheme));
        }
        let dir = &self.path()[..=self.path().rfind('/').unwrap_or(0)];
        Url::parse(&format!("{}://{authority}{dir}{reference}", self.scheme))
    }

    fn authority(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }

    /// The path of a file URL on this computer.
    #[must_use]
    pub fn to_file_path(&self) -> Option<PathBuf> {
        if self.scheme != "file" || !(self.host.is_empty() || self.host == "localhost") {
            return None;
        }
        let bytes = decode(self.path())?;
        Some(PathBuf::from(std::ffi::OsString::from(String::from_utf8(bytes).ok()?)))
    }
}

/// `text` with its `%XX` escapes decoded; `None` for a malformed one.
fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}{}", self.scheme, self.authority(), self.path)
    }
}

impl std::str::FromStr for Url {
    type Err = Malformed;

    fn from_str(text: &str) -> Result<Url, Malformed> {
        Url::parse(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_parse_into_their_parts() {
        let url = Url::parse("HTTPS://Updates.Dukovo.org:8443/catalog/metadata?x=1#top").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host(), "updates.dukovo.org");
        assert_eq!(url.port(), Some(8443));
        assert_eq!(url.path(), "/catalog/metadata");
        assert_eq!(url.path_and_query(), "/catalog/metadata?x=1");
        assert_eq!(url.to_string(), "https://updates.dukovo.org:8443/catalog/metadata?x=1");
        let six = Url::parse("http://[::1]:8080").unwrap();
        assert_eq!((six.host(), six.port(), six.path()), ("[::1]", Some(8080), "/"));
        let file = Url::parse("file:///var/cache/a%20b.json").unwrap();
        assert_eq!(file.to_file_path(), Some(PathBuf::from("/var/cache/a b.json")));
        assert_eq!(Url::parse("http://h/x").unwrap().to_file_path(), None);
        for bad in ["nothing", "http://", "http://u@h/", "http://h:99999/", "http://h/a b", "://h/"]
        {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bases_end_in_a_slash_and_references_resolve() {
        let url = Url::parse("http://h:1/repo/metadata").unwrap();
        assert_eq!(url.base().to_string(), "http://h:1/repo/metadata/");
        assert_eq!(url.base().base(), url.base());
        let file = Url::parse("http://h:1/repo/a/1.root.json").unwrap();
        assert_eq!(file.resolve("/b/c.json").unwrap().to_string(), "http://h:1/b/c.json");
        assert_eq!(file.resolve("c.json?v=2").unwrap().to_string(), "http://h:1/repo/a/c.json?v=2");
        assert_eq!(file.resolve("//mirror/x").unwrap().to_string(), "http://mirror/x");
        assert_eq!(file.resolve("https://m/x").unwrap().to_string(), "https://m/x");
    }
}
