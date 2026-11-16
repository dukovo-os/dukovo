// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The reader of nginx's configuration syntax (ADR 0569): a file's words,
//! its semicolons, blocks, quotes and comments, and the facts about a
//! website that a site's file states.
//!
//! The syntax is nginx's own, as `ngx_conf_read_token` reads it: a
//! directive is words ended by `;`, or by a `{` that opens its block; a
//! `#` starts a comment only where a word could start; a word in quotes
//! may hold spaces, `;`, braces and `#`, and a backslash there says
//! `\"`, `\'`, `\\`, `\t`, `\r` or `\n`; and the `{` right after a `$`
//! belongs to the variable (`${name}`). Nothing else is read: what a
//! directive means is for [`facts`], and for nginx.
//!
//! The reader is for files a person may have edited, so it keeps each
//! word's place in the text (the check of a new set of sites rewrites
//! one word of `nginx.conf`) and gives up on a file over [`FILE_MAX`]
//! bytes, nested deeper than [`DEPTH_MAX`] blocks, or one it cannot
//! read, with the line and what is wrong.

use std::ops::Range;

use manifest::service::check_certificate;

/// The largest file read: a website's file is a few hundred bytes, and
/// even a hand-written `nginx.conf` is well within it.
pub const FILE_MAX: usize = 64 * 1024;

/// The most blocks inside one another.
pub const DEPTH_MAX: usize = 32;

/// Where certd's copies of a service's certificates are in nginx's view,
/// which a site's `ssl_certificate` names.
pub const CERTIFICATES: &str = "/system/live/certificates/nginx";

/// A word of a directive, as nginx reads it: the quotes and escapes taken
/// away, and where it was in the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    /// The word.
    pub text: String,
    /// The bytes of the text it was read from, quotes included.
    pub span: Range<usize>,
}

/// A directive: its name, its arguments and, for one that opens a block,
/// what is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    /// Its first word.
    pub name: Word,
    /// The words after it.
    pub args: Vec<Word>,
    /// What is in its block; none for a directive that ends with `;`.
    pub block: Option<Vec<Directive>>,
    /// The line it starts on, from 1.
    pub line: usize,
}

impl Directive {
    /// Whether its name is `name`.
    pub fn is(&self, name: &str) -> bool {
        self.name.text == name
    }

    /// Its argument number `at`, from 0.
    pub fn arg(&self, at: usize) -> Option<&str> {
        self.args.get(at).map(|word| word.text.as_str())
    }

    /// What is in its block: nothing for a directive without one.
    pub fn children(&self) -> &[Directive] {
        self.block.as_deref().unwrap_or_default()
    }
}

/// What the lexer found.
enum Token {
    Word(Word),
    End,
    Open,
    Close,
}

/// Reads the word that starts at byte `start` of `text`, which is a quote
/// or any byte that is no space, `;`, `{`, `}` or `#`. Gives it, the byte
/// after it and the lines it spans.
fn read_word(text: &str, start: usize, line: usize) -> Result<(Word, usize, usize), String> {
    let bytes = text.as_bytes();
    let quote = matches!(bytes[start], b'"' | b'\'').then_some(bytes[start]);
    let mut at = start + usize::from(quote.is_some());
    let mut lines = 0;
    let mut kept = Vec::new();
    let mut variable = false;
    loop {
        let Some(&byte) = bytes.get(at) else {
            if quote.is_some() {
                return Err(format!("line {line}: a quote is not closed"));
            }
            break;
        };
        match byte {
            b'\\' if at + 1 < bytes.len() => {
                match bytes[at + 1] {
                    next @ (b'"' | b'\'' | b'\\') => kept.push(next),
                    b't' => kept.push(b'\t'),
                    b'r' => kept.push(b'\r'),
                    b'n' => kept.push(b'\n'),
                    next => kept.extend([b'\\', next]),
                }
                if bytes[at + 1] == b'\n' {
                    lines += 1;
                }
                at += 2;
                variable = false;
            }
            _ if Some(byte) == quote => {
                at += 1;
                return match bytes.get(at) {
                    None | Some(b' ' | b'\t' | b'\r' | b'\n' | b';' | b'{') => {
                        Ok((word_of(&kept, start..at), at, lines))
                    }
                    Some(&other) => Err(format!(
                        "line {}: {:?} follows a quoted word, which a space or ; must end",
                        line + lines,
                        char::from(other)
                    )),
                };
            }
            b'\n' if quote.is_some() => {
                lines += 1;
                kept.push(byte);
                at += 1;
            }
            _ if quote.is_some() => {
                kept.push(byte);
                at += 1;
            }
            b' ' | b'\t' | b'\r' | b'\n' | b';' => break,
            b'{' if variable => {
                kept.push(byte);
                at += 1;
            }
            b'{' => break,
            b'$' => {
                variable = true;
                kept.push(byte);
                at += 1;
            }
            _ => {
                variable = false;
                kept.push(byte);
                at += 1;
            }
        }
    }
    Ok((word_of(&kept, start..at), at, lines))
}

/// The word made of `bytes`: the text had them between ASCII bytes alone,
/// so they are UTF-8 still.
fn word_of(bytes: &[u8], span: Range<usize>) -> Word {
    Word { text: String::from_utf8_lossy(bytes).into_owned(), span }
}

/// The tokens of `text`, each with its line.
fn tokens(text: &str) -> Result<Vec<(Token, usize)>, String> {
    let bytes = text.as_bytes();
    let (mut found, mut at, mut line) = (Vec::new(), 0, 1);
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                line += 1;
                at += 1;
            }
            b' ' | b'\t' | b'\r' => at += 1,
            b'#' => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            b';' => {
                found.push((Token::End, line));
                at += 1;
            }
            b'{' => {
                found.push((Token::Open, line));
                at += 1;
            }
            b'}' => {
                found.push((Token::Close, line));
                at += 1;
            }
            _ => {
                let (word, next, lines) = read_word(text, at, line)?;
                found.push((Token::Word(word), line));
                line += lines;
                at = next;
            }
        }
    }
    Ok(found)
}

/// Reads `text` as nginx reads a configuration file.
///
/// # Errors
///
/// A file over [`FILE_MAX`] bytes, a quote that is not closed, a `;` or a
/// `{` with no directive before it, a `}` with no block to close or after
/// words no `;` ends, blocks deeper than [`DEPTH_MAX`], and a block or a
/// directive the file ends in the middle of, each with its line.
pub fn parse(text: &str) -> Result<Vec<Directive>, String> {
    if text.len() > FILE_MAX {
        return Err(format!("it is over {} KiB, more than is read", FILE_MAX / 1024));
    }
    let mut open: Vec<(Directive, Vec<Directive>)> = Vec::new();
    let mut level: Vec<Directive> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut first_line = 0;
    for (token, line) in tokens(text)? {
        match token {
            Token::Word(word) => {
                if words.is_empty() {
                    first_line = line;
                }
                words.push(word);
            }
            Token::End | Token::Open => {
                if words.is_empty() {
                    return Err(format!("line {line}: a ; or {{ with no directive before it"));
                }
                let name = words.remove(0);
                let head = Directive {
                    name,
                    args: std::mem::take(&mut words),
                    block: None,
                    line: first_line,
                };
                if matches!(token, Token::End) {
                    level.push(head);
                } else {
                    if open.len() >= DEPTH_MAX {
                        return Err(format!(
                            "line {line}: blocks are nested over {DEPTH_MAX} deep"
                        ));
                    }
                    open.push((head, std::mem::take(&mut level)));
                }
            }
            Token::Close => {
                if !words.is_empty() {
                    return Err(format!("line {line}: a }} follows words that no ; ends"));
                }
                let Some((mut head, outer)) = open.pop() else {
                    return Err(format!("line {line}: a }} closes no block"));
                };
                head.block = Some(std::mem::replace(&mut level, outer));
                level.push(head);
            }
        }
    }
    if !words.is_empty() {
        return Err(format!("line {first_line}: the file ends before a ; ends the directive"));
    }
    if let Some((head, _)) = open.last() {
        return Err(format!("line {}: the block of {} is not closed", head.line, head.name.text));
    }
    Ok(level)
}

/// Calls `each` for every directive of `directives` and of the blocks in
/// them, an outer one before the blocks it holds, with the names of the
/// blocks it is in, the outermost first.
pub fn visit<'a>(directives: &'a [Directive], each: &mut impl FnMut(&'a Directive, &[&'a str])) {
    fn go<'a>(
        directives: &'a [Directive],
        parents: &mut Vec<&'a str>,
        each: &mut impl FnMut(&'a Directive, &[&'a str]),
    ) {
        for directive in directives {
            each(directive, parents);
            if directive.block.is_some() {
                parents.push(directive.name.text.as_str());
                go(directive.children(), parents, each);
                parents.pop();
            }
        }
    }
    go(directives, &mut Vec::new(), each);
}

/// What a site's file says it serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Files from a folder, and no PHP.
    Static,
    /// Files from a folder, and PHP through PHP-FPM.
    Php,
    /// Anything else: a server that passes requests on, or one the reader
    /// does not know.
    Other,
}

impl Kind {
    /// The word `website list --json` says.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Static => "static",
            Kind::Php => "php",
            Kind::Other => "other",
        }
    }
}

/// What a site's file states, as far as the reader knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// The names its servers answer to, each once, in order.
    pub names: Vec<String>,
    /// The folder it serves, from its `root`.
    pub folder: Option<String>,
    /// Whether a server of it listens with `ssl`.
    pub https: bool,
    /// The ports its servers listen on, each once, in order.
    pub ports: Vec<u16>,
    /// The certificate of certd's that its `ssl_certificate` names.
    pub certificate: Option<String>,
    /// The `ssl_certificate` that is not one of certd's.
    pub certificate_path: Option<String>,
    /// What it serves.
    pub kind: Kind,
    /// Whether a server on port 80 sends visitors on to `https://`.
    pub redirects_http: bool,
}

/// The directives that pass a request on to another program or server.
const PASSES_ON: [&str; 4] = ["proxy_pass", "uwsgi_pass", "scgi_pass", "grpc_pass"];

/// The port a `listen` directive's first word names: `80`, `[::]:443`,
/// `127.0.0.1:8080`, `*:80`; none for a socket of a path (`unix:`).
pub fn listen_port(address: &str) -> Option<u16> {
    if address.starts_with("unix:") {
        return None;
    }
    let port = match address.rsplit_once(':') {
        Some((_, port)) if !address.ends_with(']') => port,
        _ => address,
    };
    match port.parse() {
        Ok(port) => Some(port),
        // An address with no port is on port 80.
        Err(_) if address.starts_with('[') || address.parse::<std::net::IpAddr>().is_ok() => {
            Some(80)
        }
        Err(_) => None,
    }
}

/// The certificate of certd's that the path `path` of a `ssl_certificate`
/// names: `/system/live/certificates/nginx/NAME/FILE`, with a name `cert`
/// takes.
pub fn certificate_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(CERTIFICATES)?.strip_prefix('/')?;
    let (name, file) = rest.split_once('/')?;
    (!file.is_empty() && !file.contains('/') && check_certificate(name).is_ok()).then_some(name)
}

/// Whether `directive`, a `return`, sends visitors on to an `https://`
/// address with a redirect.
fn redirects_to_https(directive: &Directive) -> bool {
    directive.is("return")
        && matches!(directive.arg(0), Some("301" | "302" | "303" | "307" | "308"))
        && directive.arg(1).is_some_and(|to| to.starts_with("https://"))
}

/// What the servers of a site's file `directives` state.
pub fn facts(directives: &[Directive]) -> Facts {
    let mut facts = Facts {
        names: Vec::new(),
        folder: None,
        https: false,
        ports: Vec::new(),
        certificate: None,
        certificate_path: None,
        kind: Kind::Static,
        redirects_http: false,
    };
    let mut known = directives.iter().all(|directive| directive.is("server"));
    let mut php = false;
    let mut root_of_https = None;
    let mut root_of_any = None;
    for server in directives.iter().filter(|directive| directive.is("server")) {
        let mut ssl = false;
        let mut port_80 = false;
        let mut redirects = false;
        let mut root = None;
        for directive in server.children() {
            match directive.name.text.as_str() {
                "server_name" => {
                    for word in &directive.args {
                        if !facts.names.contains(&word.text) {
                            facts.names.push(word.text.clone());
                        }
                    }
                }
                "listen" => {
                    let flags = directive.args.iter().skip(1);
                    let with_ssl = flags.clone().any(|flag| flag.text == "ssl");
                    let port = directive.arg(0).and_then(listen_port);
                    ssl |= with_ssl;
                    port_80 |= !with_ssl && port == Some(80);
                    if let Some(port) = port
                        && !facts.ports.contains(&port)
                    {
                        facts.ports.push(port);
                    }
                }
                "root" if root.is_none() => root = directive.arg(0).map(str::to_string),
                "ssl_certificate" if ssl && facts.certificate.is_none() => {
                    if let Some(path) = directive.arg(0) {
                        match certificate_of(path) {
                            Some(name) => facts.certificate = Some(name.to_string()),
                            None => facts.certificate_path = Some(path.to_string()),
                        }
                    }
                }
                _ => {}
            }
        }
        visit(server.children(), &mut |directive, _| {
            php |= directive.is("fastcgi_pass");
            known &= !PASSES_ON.iter().any(|name| directive.is(name));
            redirects |= redirects_to_https(directive);
        });
        facts.https |= ssl;
        facts.redirects_http |= port_80 && redirects;
        root_of_any = root_of_any.or_else(|| root.clone());
        if ssl {
            root_of_https = root_of_https.or(root);
        }
    }
    facts.folder = root_of_https.or(root_of_any);
    // A server with no folder to serve, as a site of files and PHP has,
    // is one the reader does not know.
    known &= facts.folder.is_some() && !facts.names.is_empty();
    facts.kind = match (known, php) {
        (false, _) => Kind::Other,
        (true, true) => Kind::Php,
        (true, false) => Kind::Static,
    };
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directive<'a>(directives: &'a [Directive], name: &str) -> &'a Directive {
        directives.iter().find(|directive| directive.is(name)).unwrap()
    }

    #[test]
    fn directives_are_words_ended_by_a_semicolon_or_a_block() {
        let read =
            parse("worker_processes auto;\nhttp {\n  server {\n    listen 80;\n  }\n}\n").unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].arg(0), Some("auto"));
        let http = directive(&read, "http");
        let server = directive(http.children(), "server");
        assert_eq!(directive(server.children(), "listen").arg(0), Some("80"));
        assert_eq!((read[0].line, http.line, server.line), (1, 2, 3));
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn a_hash_starts_a_comment_only_where_a_word_could_start() {
        let read = parse(
            "# a comment; with {braces}\nroot /a#b;  # not part of the word\n\
             return 200 \"a # b; c { d }\";\nx '#';\nlocation ~ ^/#x { }\n",
        )
        .unwrap();
        assert_eq!(directive(&read, "root").arg(0), Some("/a#b"));
        assert_eq!(directive(&read, "return").arg(1), Some("a # b; c { d }"));
        assert_eq!(directive(&read, "x").arg(0), Some("#"));
        // The `#` of the location's regular expression is mid-word only
        // when something precedes it: here `^/#x` is one word.
        assert_eq!(directive(&read, "location").arg(1), Some("^/#x"));
    }

    #[test]
    fn quotes_hold_spaces_and_escapes_as_nginx_reads_them() {
        let read = parse(
            r#"a "two words" 'single \'quoted\'' "tab\there" \"bare\" "back\\slash" "keep\.dot";"#,
        )
        .unwrap();
        let args: Vec<&str> = read[0].args.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(
            args,
            ["two words", "single 'quoted'", "tab\there", "\"bare\"", "back\\slash", "keep\\.dot"]
        );
        // A word keeps the place it was read from, quotes and all.
        let text = "include \"/a b/*.conf\";";
        let read = parse(text).unwrap();
        assert_eq!(&text[read[0].args[0].span.clone()], "\"/a b/*.conf\"");
    }

    #[test]
    fn a_dollar_and_a_brace_after_it_are_one_variable() {
        let read =
            parse("set $x ${name}_suffix;\nreturn 301 https://$host$request_uri;\n").unwrap();
        assert_eq!(read[0].arg(1), Some("${name}_suffix"));
        assert_eq!(read[1].arg(1), Some("https://$host$request_uri"));
    }

    #[test]
    fn a_file_that_does_not_read_is_refused_with_its_line() {
        for (text, line, why) in [
            ("a;\nb \"never closed;\n", 2, "a quote is not closed"),
            ("a;\n}\n", 2, "closes no block"),
            ("a {\nb;\n", 1, "not closed"),
            ("a b\nc", 1, "ends before"),
            ("a b }\n", 1, "no ; ends"),
            (";\n", 1, "no directive"),
            ("{ a; }\n", 1, "no directive"),
            ("a \"x\"y;\n", 1, "follows a quoted word"),
        ] {
            let why_not = parse(text).unwrap_err();
            assert!(why_not.starts_with(&format!("line {line}: ")), "{text:?}: {why_not}");
            assert!(why_not.contains(why), "{text:?}: {why_not}");
        }
        let deep = format!("{}{}", "a {".repeat(DEPTH_MAX + 1), "}".repeat(DEPTH_MAX + 1));
        assert!(parse(&deep).unwrap_err().contains("nested"));
        let just = format!("{}{}", "a {".repeat(DEPTH_MAX), "}".repeat(DEPTH_MAX));
        assert!(parse(&just).is_ok());
    }

    #[test]
    fn a_file_over_64_kib_is_refused_before_it_is_read() {
        let big = "# ".to_string() + &"x".repeat(FILE_MAX);
        assert!(parse(&big).unwrap_err().contains("64 KiB"));
        let fits = "#".repeat(FILE_MAX);
        assert!(parse(&fits).unwrap().is_empty());
    }

    #[test]
    fn a_port_is_read_from_a_listen_as_nginx_reads_it() {
        for (address, port) in [
            ("80", Some(80)),
            ("443", Some(443)),
            ("[::]:80", Some(80)),
            ("[::]:443", Some(443)),
            ("0.0.0.0:8080", Some(8080)),
            ("*:80", Some(80)),
            ("127.0.0.1", Some(80)),
            ("[::1]", Some(80)),
            ("unix:/run/x.sock", None),
            ("localhost", None),
        ] {
            assert_eq!(listen_port(address), port, "{address}");
        }
    }

    #[test]
    fn a_certificate_is_certd_s_when_its_path_is_in_nginx_s_copies() {
        let base = "/system/live/certificates/nginx";
        assert_eq!(
            certificate_of(&format!("{base}/shop.example.org/fullchain.pem")),
            Some("shop.example.org")
        );
        for path in [
            "/etc/ssl/x.pem",
            &format!("{base}/fullchain.pem"),
            &format!("{base}/Shop/fullchain.pem"),
            &format!("{base}/../x/fullchain.pem"),
            &format!("{base}/a/b/fullchain.pem"),
            &format!("{base}/a/"),
            &format!("{base}x/a/b.pem"),
        ] {
            assert_eq!(certificate_of(path), None, "{path}");
        }
    }

    const HTTP_ONLY: &str = "server {\n    listen 80;\n    listen [::]:80;\n    \
        server_name blog.example.org;\n    root /shared/www/sites/blog.example.org;\n    \
        location ~ \\.php$ {\n        return 404;\n    }\n}\n";

    #[test]
    fn a_site_of_files_over_http_states_its_names_and_folder() {
        let facts = facts(&parse(HTTP_ONLY).unwrap());
        assert_eq!(facts.names, ["blog.example.org"]);
        assert_eq!(facts.folder.as_deref(), Some("/shared/www/sites/blog.example.org"));
        assert_eq!((facts.https, facts.redirects_http, facts.kind), (false, false, Kind::Static));
        assert_eq!((facts.certificate, facts.certificate_path), (None, None));
    }

    #[test]
    fn a_site_over_https_with_php_states_its_certificate_and_its_redirect() {
        let text = "server {\n    listen 80;\n    server_name shop.example.org www.shop.example.org;\n    \
            location / {\n        return 301 https://$host$request_uri;\n    }\n}\n\
            server {\n    listen 443 ssl;\n    listen [::]:443 ssl;\n    http2 on;\n    \
            server_name shop.example.org www.shop.example.org;\n    \
            ssl_certificate /system/live/certificates/nginx/shop.example.org/fullchain.pem;\n    \
            root /shared/www/sites/shop.example.org;\n    \
            location ~ \\.php$ {\n        fastcgi_pass 127.0.0.1:9000;\n    }\n}\n";
        let facts = facts(&parse(text).unwrap());
        assert_eq!(facts.names, ["shop.example.org", "www.shop.example.org"]);
        assert_eq!(facts.folder.as_deref(), Some("/shared/www/sites/shop.example.org"));
        assert_eq!((facts.https, facts.redirects_http, facts.kind), (true, true, Kind::Php));
        assert_eq!(facts.certificate.as_deref(), Some("shop.example.org"));
        assert_eq!(facts.certificate_path, None);
    }

    #[test]
    fn a_certificate_nginx_gets_elsewhere_is_a_path_and_no_certificate() {
        let text = "server { listen 443 ssl; server_name a.example.org; root /r; \
            ssl_certificate /etc/mine/chain.pem; }";
        let facts = facts(&parse(text).unwrap());
        assert!(facts.https);
        assert_eq!(facts.certificate, None);
        assert_eq!(facts.certificate_path.as_deref(), Some("/etc/mine/chain.pem"));
    }

    #[test]
    fn a_server_that_passes_requests_on_or_that_has_no_folder_is_of_another_kind() {
        for text in [
            "server { listen 80; server_name legacy.example.org; proxy_pass http://127.0.0.1:8080; }",
            "server { listen 80; server_name a.example.org; root /r; location / { proxy_pass http://x; } }",
            "server { listen 80; server_name a.example.org; return 301 https://b.example.org; }",
            "upstream x { server 127.0.0.1:1; }\nserver { listen 80; server_name a.example.org; root /r; }",
            "map $a $b { default 1; }",
            "",
        ] {
            let facts = facts(&parse(text).unwrap());
            assert_eq!(facts.kind, Kind::Other, "{text}");
        }
    }

    #[test]
    fn a_hand_edit_that_adds_a_location_a_comment_or_quotes_reads_the_same() {
        let edited = "# my own note\nserver {\n    listen 80;\n    listen [::]:80;\n    \
            server_name \"blog.example.org\";  # the blog\n    \
            root '/shared/www/sites/blog.example.org';\n    \
            location /api/ { return 200 \"a # b\"; }\n    \
            include /etc/extra/*.conf;\n}\n";
        let facts = facts(&parse(edited).unwrap());
        assert_eq!(facts.names, ["blog.example.org"]);
        assert_eq!(facts.folder.as_deref(), Some("/shared/www/sites/blog.example.org"));
        assert_eq!(facts.kind, Kind::Static);
    }

    #[test]
    fn the_visit_goes_outer_first_with_the_blocks_each_is_in() {
        let read = parse("http { server { listen 80; location / { root /r; } } }").unwrap();
        let mut seen = Vec::new();
        visit(&read, &mut |directive, parents| {
            seen.push(format!("{}<{}", directive.name.text, parents.join("/")));
        });
        assert_eq!(
            seen,
            [
                "http<",
                "server<http",
                "listen<http/server",
                "location<http/server",
                "root<http/server/location"
            ]
        );
    }
}
