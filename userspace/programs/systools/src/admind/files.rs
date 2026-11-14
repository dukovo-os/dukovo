// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's own files (ADR 0514), which the system installs in
//! `/system/resources/admin/` with a `manifest.txt` of the SHA-256 of each:
//! `HASH  path` on a line, as `sha256sum` writes it. admind reads the
//! manifest when it starts, reads each file it lists, and keeps the files
//! whose hash is the one listed, in memory. A file that is not listed, or
//! does not match its line, is never served, so a changed file on the disk
//! cannot become a page of the console. The pages are single-page: a path
//! that is no file and has no extension is the page router's, and is
//! answered with `index.html`.
//!
//! When the console shows nothing, look in the log for the lines that
//! begin `admind: the console's files`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use accounts::request;
use bytes::Bytes;
use ring::digest;

use crate::util;

/// Where the system installs the console's files.
pub const FOLDER: &str = "/system/resources/admin";

/// The file the page router's paths are answered with.
const SHELL: &str = "index.html";

/// A file of the console's, whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// What it holds.
    pub bytes: Bytes,
    /// Its `Content-Type`.
    pub content_type: &'static str,
}

/// The files that match their manifest.
#[derive(Debug, Default)]
pub struct Files {
    served: HashMap<String, File>,
}

/// The hash and path of each line of a manifest.
///
/// # Errors
///
/// With the line that is not `HASH  path`: a hash of 64 hexadecimal
/// digits, then the path of a file inside the folder.
fn parse(manifest: &str) -> Result<Vec<(String, String)>, String> {
    let mut lines = Vec::new();
    for (number, line) in manifest.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let wrong = || format!("line {} is not `HASH  path`", number + 1);
        let (hash, path) = line.split_once(char::is_whitespace).ok_or_else(wrong)?;
        let path = path.trim_start_matches([' ', '*']);
        let plain = path.split('/').all(|part| !matches!(part, "" | "." | ".."));
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || !plain {
            return Err(wrong());
        }
        lines.push((hash.to_ascii_lowercase(), path.to_string()));
    }
    Ok(lines)
}

/// The `Content-Type` of a file called `path`, by its extension.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/vnd.microsoft.icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl Files {
    /// The files of `folder` that its `manifest.txt` lists and that match.
    /// What does not is said in `said`, a line each.
    ///
    /// # Errors
    ///
    /// If the manifest cannot be read or has a line that is not right.
    pub fn read(folder: &Path, said: &mut Vec<String>) -> Result<Files, String> {
        let manifest = folder.join("manifest.txt");
        let text = fs::read_to_string(&manifest)
            .map_err(|err| format!("cannot read {}: {}", manifest.display(), util::strip_os_error(&err)))?;
        let mut served = HashMap::new();
        for (hash, path) in parse(&text).map_err(|why| format!("{}: {why}", manifest.display()))? {
            let bytes = match fs::read(folder.join(&path)) {
                Ok(bytes) => bytes,
                Err(err) => {
                    said.push(format!("{path} is listed but cannot be read: {}", util::strip_os_error(&err)));
                    continue;
                }
            };
            if request::hex(digest::digest(&digest::SHA256, &bytes).as_ref()) != hash {
                said.push(format!("{path} is not the file the manifest lists, and is not served"));
                continue;
            }
            let file = File { bytes: Bytes::from(bytes), content_type: content_type(&path) };
            served.insert(path, file);
        }
        Ok(Files { served })
    }

    /// What `GET path` is answered with: the file of that name, or the
    /// page router's `index.html` for a path with no extension. `None`
    /// is a 404.
    pub fn get(&self, path: &str) -> Option<&File> {
        let name = path.strip_prefix('/').unwrap_or(path);
        let name = if name.is_empty() { SHELL } else { name };
        if let Some(file) = self.served.get(name) {
            return Some(file);
        }
        let last = name.rsplit('/').next().unwrap_or(name);
        if last.contains('.') { None } else { self.served.get(SHELL) }
    }

    /// How many files are served.
    pub fn len(&self) -> usize {
        self.served.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::fixtures::Scratch;

    fn sum(bytes: &[u8]) -> String {
        request::hex(digest::digest(&digest::SHA256, bytes).as_ref())
    }

    /// A folder with the files `(path, text)` and a manifest that lists
    /// `listed`, which are written as they are.
    fn folder(what: &str, files: &[(&str, &str)], listed: &[(&str, &str)]) -> Scratch {
        let scratch = Scratch::new(what);
        for (path, text) in files {
            let at = scratch.0.join(path);
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(at, text).unwrap();
        }
        let manifest = listed.iter().fold(String::new(), |text, (hash, path)| {
            text + &format!("{hash}  {path}\n")
        });
        fs::write(scratch.0.join("manifest.txt"), manifest).unwrap();
        scratch
    }

    #[test]
    fn only_the_files_that_match_the_manifest_are_served() {
        let right = sum(b"console.log(1)");
        let scratch = folder(
            "match",
            &[
                ("index.html", "<html>"),
                ("app.js", "console.log(1)"),
                ("sign-in.js", "changed on the disk"),
                ("extra.js", "never listed"),
                ("css/console.css", "body{}"),
            ],
            &[
                (&sum(b"<html>"), "index.html"),
                (&right, "app.js"),
                (&sum(b"console.log(2)"), "sign-in.js"),
                (&sum(b"body{}"), "css/console.css"),
                (&sum(b"absent"), "gone.js"),
            ],
        );
        let mut said = Vec::new();
        let files = Files::read(&scratch.0, &mut said).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files.get("/app.js").unwrap().bytes, "console.log(1)");
        assert_eq!(files.get("/app.js").unwrap().content_type, "text/javascript; charset=utf-8");
        assert_eq!(files.get("/css/console.css").unwrap().content_type, "text/css; charset=utf-8");
        assert!(files.get("/sign-in.js").is_none());
        assert!(files.get("/extra.js").is_none());
        assert!(files.get("/manifest.txt").is_none());
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].starts_with("sign-in.js is not the file the manifest lists"), "{said:?}");
        assert!(said[1].starts_with("gone.js is listed but cannot be read"), "{said:?}");
    }

    #[test]
    fn a_path_that_is_no_file_is_the_page_routers_unless_it_names_one() {
        let scratch =
            folder("router", &[("index.html", "<html>")], &[(&sum(b"<html>"), "index.html")]);
        let files = Files::read(&scratch.0, &mut Vec::new()).unwrap();
        for router in ["/", "/services", "/services/nginx/log", "/accounts/second-step"] {
            assert_eq!(files.get(router).unwrap().bytes, "<html>", "{router}");
        }
        assert_eq!(files.get("/index.html").unwrap().content_type, "text/html; charset=utf-8");
        for no_file in ["/missing.js", "/css/none.css", "/favicon.ico", "/a/b.c"] {
            assert!(files.get(no_file).is_none(), "{no_file}");
        }
        // Nothing climbs out of the folder: only a listed name is a file.
        assert!(files.get("/../index.html").is_none());
        assert!(files.get("/%2e%2e/index.html").is_none());
    }

    #[test]
    fn a_manifest_that_is_missing_or_wrong_serves_nothing() {
        let scratch = Scratch::new("manifest");
        let error = Files::read(&scratch.0, &mut Vec::new()).unwrap_err();
        assert!(error.starts_with("cannot read "), "{error}");
        for text in [
            "nothing",
            "abc  index.html",
            &format!("{}  ../etc/passwd", "a".repeat(64)),
            &format!("{}  /index.html", "a".repeat(64)),
            &format!("{}  a//b", "a".repeat(64)),
            &"a".repeat(64),
            &format!("{}  ", "z".repeat(64)),
        ] {
            fs::write(scratch.0.join("manifest.txt"), text).unwrap();
            let error = Files::read(&scratch.0, &mut Vec::new()).unwrap_err();
            assert!(error.contains("line 1 is not `HASH  path`"), "{text}: {error}");
        }
        // Blank lines are nothing, and a binary-mode star is sha256sum's.
        let text = format!("\n{}  *index.html\n\n", sum(b"x"));
        assert_eq!(parse(&text).unwrap(), [(sum(b"x"), "index.html".to_string())]);
    }

    #[test]
    fn each_kind_of_file_has_its_type() {
        for (path, type_) in [
            ("a.html", "text/html; charset=utf-8"),
            ("a.mjs", "text/javascript; charset=utf-8"),
            ("a.json", "application/json"),
            ("a.svg", "image/svg+xml"),
            ("a.woff2", "font/woff2"),
            ("a.png", "image/png"),
            ("a.bin", "application/octet-stream"),
            ("noextension", "application/octet-stream"),
        ] {
            assert_eq!(content_type(path), type_, "{path}");
        }
    }
}
