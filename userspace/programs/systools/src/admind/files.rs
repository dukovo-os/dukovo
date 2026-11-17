// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's own files (ADR 0514), which the system installs in
//! `/system/resources/admin/` with a `manifest.txt` of the SHA-256 of each:
//! `HASH  path` on a line, as `sha256sum` writes it. admind reads the
//! manifest when it starts and checks each file it lists; a file that is
//! not listed, or does not match its line, is never served, so a changed
//! file on the disk cannot become a page of the console. A file's bytes
//! are read from the disk each time they are sent, and checked again
//! against the manifest: admind holds none of them, so the console's files
//! cost an idle machine no memory, and a file that changes after admind
//! started is not served either. The pages are single-page: a path that is
//! no file and does not end in the extension of a kind of file the console
//! has is the page router's, and is answered with `index.html`. A page's
//! path can end in a name with dots, a website's or a certificate's
//! (`/websites/example.org`), and the shell's links follow the same rule
//! (`route-model.js`).
//!
//! The manifest's hash of a file is its `ETag`, so that a browser that
//! has the file asks "has it changed?" and is told 304, with no body and
//! nothing read from the disk, when it has not ([`Listed::matches`]).
//!
//! When the console shows nothing, look in the log for the lines that
//! begin `admind: the console's files`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use accounts::request;
use bytes::Bytes;
use ring::digest;

use crate::util;

/// Where the system installs the console's files.
pub const FOLDER: &str = "/system/resources/admin";

/// The file the page router's paths are answered with.
const SHELL: &str = "index.html";

/// The extensions of the kinds of file the console has, but `md`, which is
/// help's alone: a path whose last part ends in one names a file. Keep in
/// step with the pattern of `followsInPage` in `userspace/admin/shell/route-model.js`.
const FILE_EXTENSIONS: &[&str] =
    &["html", "js", "mjs", "css", "json", "webmanifest", "svg", "png", "ico", "woff2", "txt"];

/// Whether `name`, a path with no leading `/`, names one of the console's
/// files rather than a page: its last part ends in the extension of a kind
/// of file the console has, or in `.md` inside help. A name with other dots,
/// as `websites/shop.example.md` or `certificates/example.org`, is a page's.
fn names_a_file(name: &str) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);
    match last.rsplit_once('.') {
        Some((_, "md")) => name.starts_with("help/"),
        Some((_, extension)) => FILE_EXTENSIONS.contains(&extension),
        None => false,
    }
}

/// A file of the console's as a request is answered with it: its bytes,
/// none for a 304, and its headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// What it holds.
    pub bytes: Bytes,
    /// Its `Content-Type`.
    pub content_type: &'static str,
    /// Its `ETag`: the manifest's hash of it, in quotes.
    pub etag: String,
}

/// A file the manifest lists, and that matched its line when admind
/// started: where it is and its headers, without its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Its path in the folder, as the manifest names it.
    path: String,
    /// The manifest's hash of it, in lowercase hexadecimal.
    hash: String,
    /// Its `Content-Type`.
    pub content_type: &'static str,
    /// Its `ETag`: the hash, in quotes.
    pub etag: String,
}

impl Listed {
    /// Whether `if_none_match`, the value of a request's `If-None-Match`,
    /// names this file as it is: `*`, or a list of entity tags one of which
    /// is its own, weak or strong, as a `GET` compares them.
    pub fn matches(&self, if_none_match: &str) -> bool {
        if_none_match
            .split(',')
            .map(str::trim)
            .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == self.etag)
    }

    /// The file without its bytes: what a 304 is answered with.
    pub fn held(&self) -> File {
        File { bytes: Bytes::new(), content_type: self.content_type, etag: self.etag.clone() }
    }
}

/// The files that matched their manifest when admind started.
#[derive(Debug, Default)]
pub struct Files {
    /// The folder they are in.
    folder: PathBuf,
    /// Each by its path in the folder.
    listed: HashMap<String, Listed>,
}

/// The bytes of the file `path` of `folder`, when their SHA-256 is `hash`.
///
/// # Errors
///
/// With a line for the log: the file cannot be read, or is not the one
/// the manifest lists.
fn read_matching(folder: &Path, path: &str, hash: &str) -> Result<Vec<u8>, String> {
    let bytes = fs::read(folder.join(path)).map_err(|err| {
        format!("{path} is listed but cannot be read: {}", util::strip_os_error(&err))
    })?;
    if request::hex(digest::digest(&digest::SHA256, &bytes).as_ref()) != hash {
        return Err(format!("{path} is not the file the manifest lists, and is not served"));
    }
    Ok(bytes)
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
        Some("md") => "text/markdown; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl Files {
    /// The files of `folder` that its `manifest.txt` lists and that match.
    /// What does not is said in `said`, a line each. Each file is read
    /// once to check it, and none is kept.
    ///
    /// # Errors
    ///
    /// If the manifest cannot be read or has a line that is not right.
    pub fn read(folder: &Path, said: &mut Vec<String>) -> Result<Files, String> {
        let manifest = folder.join("manifest.txt");
        let text = fs::read_to_string(&manifest).map_err(|err| {
            format!("cannot read {}: {}", manifest.display(), util::strip_os_error(&err))
        })?;
        let mut listed = HashMap::new();
        for (hash, path) in parse(&text).map_err(|why| format!("{}: {why}", manifest.display()))? {
            if let Err(why) = read_matching(folder, &path, &hash) {
                said.push(why);
                continue;
            }
            let etag = format!("\"{hash}\"");
            let file = Listed { path: path.clone(), hash, content_type: content_type(&path), etag };
            listed.insert(path, file);
        }
        Ok(Files { folder: folder.to_path_buf(), listed })
    }

    /// The file `GET path` is answered with: the file of that name, or
    /// the page router's `index.html` for a path that names no file
    /// ([`names_a_file`]). `None` is a 404.
    pub fn get(&self, path: &str) -> Option<&Listed> {
        let name = path.strip_prefix('/').unwrap_or(path);
        let name = if name.is_empty() { SHELL } else { name };
        if let Some(file) = self.listed.get(name) {
            return Some(file);
        }
        if names_a_file(name) { None } else { self.listed.get(SHELL) }
    }

    /// `listed` with its bytes, read from the disk now.
    ///
    /// # Errors
    ///
    /// With a line for the log, when the file cannot be read or is no
    /// longer the one the manifest lists: it is then not served.
    pub fn load(&self, listed: &Listed) -> Result<File, String> {
        let bytes = read_matching(&self.folder, &listed.path, &listed.hash)?;
        Ok(File { bytes: Bytes::from(bytes), ..listed.held() })
    }

    /// How many files are served.
    pub fn len(&self) -> usize {
        self.listed.len()
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
        let manifest = listed
            .iter()
            .fold(String::new(), |text, (hash, path)| text + &format!("{hash}  {path}\n"));
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
        let app = files.load(files.get("/app.js").unwrap()).unwrap();
        assert_eq!(app.bytes, "console.log(1)");
        assert_eq!(app.content_type, "text/javascript; charset=utf-8");
        assert_eq!(files.get("/css/console.css").unwrap().content_type, "text/css; charset=utf-8");
        assert!(files.get("/sign-in.js").is_none());
        assert!(files.get("/extra.js").is_none());
        assert!(files.get("/manifest.txt").is_none());
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].starts_with("sign-in.js is not the file the manifest lists"), "{said:?}");
        assert!(said[1].starts_with("gone.js is listed but cannot be read"), "{said:?}");
    }

    #[test]
    fn a_file_is_read_when_it_is_sent_and_only_while_it_still_matches() {
        let scratch = folder(
            "later",
            &[("index.html", "<html>"), ("app.js", "console.log(1)")],
            &[(&sum(b"<html>"), "index.html"), (&sum(b"console.log(1)"), "app.js")],
        );
        let files = Files::read(&scratch.0, &mut Vec::new()).unwrap();
        let app = files.get("/app.js").unwrap();
        // What a 304 is answered with has the headers and no bytes.
        let held = app.held();
        assert!(held.bytes.is_empty());
        assert_eq!((held.content_type, held.etag.as_str()), (app.content_type, app.etag.as_str()));
        // A file changed on the disk after admind started is not served,
        // and the line for the log names it.
        fs::write(scratch.0.join("app.js"), "console.log(2)").unwrap();
        let why = files.load(app).unwrap_err();
        assert_eq!(why, "app.js is not the file the manifest lists, and is not served");
        fs::remove_file(scratch.0.join("app.js")).unwrap();
        let why = files.load(app).unwrap_err();
        assert!(why.starts_with("app.js is listed but cannot be read"), "{why}");
        // Put back as it was, it is served again; the others never stopped.
        fs::write(scratch.0.join("app.js"), "console.log(1)").unwrap();
        assert_eq!(files.load(app).unwrap().bytes, "console.log(1)");
        assert_eq!(files.load(files.get("/").unwrap()).unwrap().bytes, "<html>");
    }

    #[test]
    fn a_files_etag_is_the_manifests_hash_in_quotes_and_a_matching_tag_is_told() {
        let scratch = folder(
            "etag",
            &[("index.html", "<html>"), ("app.js", "console.log(1)")],
            &[(&sum(b"<html>"), "index.html"), (&sum(b"console.log(1)"), "app.js")],
        );
        let files = Files::read(&scratch.0, &mut Vec::new()).unwrap();
        let app = files.get("/app.js").unwrap();
        assert_eq!(app.etag, format!("\"{}\"", sum(b"console.log(1)")));
        assert_ne!(app.etag, files.get("/index.html").unwrap().etag);
        // The page router's paths have the shell's tag.
        assert_eq!(files.get("/services").unwrap().etag, files.get("/").unwrap().etag);
        let tag = app.etag.clone();
        let other = format!("\"{}\"", sum(b"console.log(2)"));
        for matching in [
            tag.clone(),
            format!("W/{tag}"),
            format!("{other}, {tag}"),
            format!("{other},{tag}"),
            format!("  {tag}  "),
            format!("{other}, W/{tag} , \"x\""),
            "*".to_string(),
            "\"x\", *".to_string(),
        ] {
            assert!(app.matches(&matching), "{matching}");
        }
        for different in [
            String::new(),
            other.clone(),
            sum(b"console.log(1)"),
            tag.trim_matches('"').to_string(),
            format!("{tag}x"),
            format!("\"x{}", tag.trim_start_matches('"')),
            format!("w/{tag}"),
            format!("{other}, \"y\""),
            "**".to_string(),
        ] {
            assert!(!app.matches(&different), "{different}");
        }
    }

    #[test]
    fn a_path_that_is_no_file_is_the_page_routers_unless_it_names_one() {
        let scratch =
            folder("router", &[("index.html", "<html>")], &[(&sum(b"<html>"), "index.html")]);
        let files = Files::read(&scratch.0, &mut Vec::new()).unwrap();
        // A website's or a certificate's name has dots, and its page is the
        // router's all the same, even with a top-level domain that is the
        // extension of help's files.
        for router in [
            "/",
            "/services",
            "/services/nginx/log",
            "/accounts/second-step",
            "/websites/dukovo.local",
            "/certificates/site.example.org",
            "/websites/shop.example.md",
            "/apps/org.postgresql.postgresql",
        ] {
            assert_eq!(files.load(files.get(router).unwrap()).unwrap().bytes, "<html>", "{router}");
        }
        assert_eq!(files.get("/index.html").unwrap().content_type, "text/html; charset=utf-8");
        for no_file in [
            "/missing.js",
            "/css/none.css",
            "/favicon.ico",
            "/help/errors/index.json",
            "/help/none.md",
            "/pages/websites/none.mjs",
        ] {
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
            ("manifest.webmanifest", "application/manifest+json"),
            ("sw.js", "text/javascript; charset=utf-8"),
            ("icons/app-192.png", "image/png"),
            ("a.svg", "image/svg+xml"),
            ("a.woff2", "font/woff2"),
            ("help/errors/ADM-101.md", "text/markdown; charset=utf-8"),
            ("a.png", "image/png"),
            ("a.bin", "application/octet-stream"),
            ("noextension", "application/octet-stream"),
        ] {
            assert_eq!(content_type(path), type_, "{path}");
        }
    }
}
