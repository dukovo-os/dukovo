// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Fetching a file of at most so many bytes, from an http, https or file
//! URL: a longer one is cut off and refused, so that a repository cannot
//! fill the disk or memory (TUF's endless data attack).

use std::fmt;
use std::io::{self, Read};

use crate::url::Url;

use crate::http;

/// Why a fetch failed.
#[derive(Debug)]
pub enum Error {
    /// The file is not there.
    NotFound,
    /// It is longer than the most it may be.
    TooLong(u64),
    /// Anything else: the URL, the connection, the server.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => write!(f, "not there"),
            Error::TooLong(most) => write!(f, "longer than the {most} bytes it may be"),
            Error::Failed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Reads all of `reader`, refusing more than `most` bytes.
fn read_at_most(reader: impl Read, most: u64) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    reader
        .take(most.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|err| Error::Failed(err.to_string()))?;
    if data.len() as u64 > most {
        return Err(Error::TooLong(most));
    }
    Ok(data)
}

/// Fetches `url`, of at most `most` bytes.
///
/// # Errors
///
/// [`Error::NotFound`] if it is not there, [`Error::TooLong`] if it is
/// longer, and any other failure.
pub fn fetch(url: &Url, most: u64) -> Result<Vec<u8>, Error> {
    match url.scheme() {
        "file" => {
            let path = url
                .to_file_path()
                .ok_or_else(|| Error::Failed(format!("{url} is not a path here")))?;
            match std::fs::File::open(&path) {
                Ok(file) => read_at_most(file, most),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Err(Error::NotFound),
                Err(err) => Err(Error::Failed(format!("{}: {err}", path.display()))),
            }
        }
        "http" | "https" => match http::get(url) {
            Ok(body) => read_at_most(body, most),
            Err(http::Error::NotFound(_)) => Err(Error::NotFound),
            Err(err) => Err(Error::Failed(err.to_string())),
        },
        other => Err(Error::Failed(format!("{other}: URLs are not fetched"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_fetched_up_to_their_limit() {
        let dir = std::env::temp_dir().join(format!("tuf-fetch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), b"{\"a\":1}").unwrap();
        let url = Url::parse(&format!("file://{}", dir.join("a.json").display())).unwrap();
        assert_eq!(fetch(&url, 7).unwrap(), b"{\"a\":1}");
        assert!(matches!(fetch(&url, 6), Err(Error::TooLong(6))));
        let missing = Url::parse(&format!("file://{}", dir.join("b.json").display())).unwrap();
        assert!(matches!(fetch(&missing, 7), Err(Error::NotFound)));
        assert!(matches!(fetch(&Url::parse("ftp://h/x").unwrap(), 7), Err(Error::Failed(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
