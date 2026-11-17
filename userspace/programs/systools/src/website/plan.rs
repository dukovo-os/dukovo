// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `website` writes and what it reads of the settings that are there
//! (ADR 0569): a site's file and its starter page from the facts of a
//! site, nginx's own `nginx.conf` as far as the websites need it, and the
//! keys of nginx's service file that the sites need.
//!
//! Nothing here touches the machine. A site's file is the text
//! [`site_text`] makes from a [`Spec`], and a file that reads back as a
//! spec and renders to its own text is one `website` wrote and nobody
//! changed ([`by_hand`]). The service file's override is changed as text
//! ([`override_text`]), through the same editor the web console uses, so
//! that what a person wrote in it stays as it was.

use std::ops::Range;

use settings_edit::Unedited;
use toml_edit::{DocumentMut, Item, Table, Value};

use super::conf::{self, Directive, Facts, Kind, certificate_of, listen_port};
use crate::settings_edit;

/// Where a site's folder is, unless the person names another under it.
pub const SITES: &str = "/shared/www/sites";

/// Where the site files are, which `nginx.conf` includes.
pub const SETTINGS: &str = "/system/settings/websites";

/// What `nginx.conf` includes to serve the sites.
pub const INCLUDE: &str = "/system/settings/websites/*.conf";

/// The line that includes them, as the bundle's `nginx.conf` has it.
pub const INCLUDE_LINE: &str = "include /system/settings/websites/*.conf;";

/// The lines that keep the folders of the sites from being served by the
/// default server.
pub const HIDE_LINES: [&str; 3] = ["location ^~ /sites/ {", "    return 404;", "}"];

/// The file that answers an HTTPS request for a name no site has.
pub const DEFAULT_FILE: &str = "_default.conf";

/// What certd's copies are called in nginx's view.
pub const CERTD: &str = "/system/live/certd";

/// The comment that heads an override `website` makes.
pub const HEADER: &str = "# Keys website(8) keeps for nginx's websites (ADR 0569). Your own keys\n\
                          # here are kept; a list website changes keeps every item you added.\n";

/// What a site's file says, as far as `website` writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The site's name: its file's, without `.conf`.
    pub name: String,
    /// The names it answers to, the first being its name.
    pub names: Vec<String>,
    /// What it serves: files, or files and PHP.
    pub kind: Kind,
    /// The folder it serves.
    pub folder: String,
    /// The certificate of certd's it serves HTTPS with.
    pub certificate: Option<String>,
}

/// The first line of a site's file that `website` wrote.
pub fn first_line(name: &str) -> String {
    format!("# {name}, a website added by website(8) (ADR 0569).")
}

/// The text of the file of site `spec`.
pub fn site_text(spec: &Spec) -> String {
    let names = spec.names.join(" ");
    let mut text = format!(
        "{}\n# This file is the site: a change made here is kept and served, and\n\
         # the web console shows the site as changed by hand.\n",
        first_line(&spec.name)
    );
    let body = server_body(spec);
    match &spec.certificate {
        Some(certificate) => {
            let base = format!("/system/live/certificates/nginx/{certificate}");
            text.push_str(&format!(
                "server {{\n    listen 80;\n    listen [::]:80;\n    server_name {names};\n    \
                 include acme-challenge.conf;\n    location / {{\n        \
                 return 301 https://$host$request_uri;\n    }}\n}}\n\n\
                 server {{\n    listen 443 ssl;\n    listen [::]:443 ssl;\n    http2 on;\n    \
                 server_name {names};\n    ssl_certificate {base}/fullchain.pem;\n    \
                 ssl_certificate_key {base}/key.pem;\n{body}}}\n"
            ));
        }
        None => text.push_str(&format!(
            "server {{\n    listen 80;\n    listen [::]:80;\n    server_name {names};\n{body}}}\n"
        )),
    }
    text
}

/// What the server that serves the files has after its names: the
/// folder, the index files and what each kind of request gets.
fn server_body(spec: &Spec) -> String {
    let index = match spec.kind {
        Kind::Php => "index.php index.html",
        _ => "index.html index.htm",
    };
    let mut body = format!(
        "    root {};\n    index {index};\n    include acme-challenge.conf;\n\n    \
         # Files whose names start with a dot (.git, .env) are never served.\n    \
         location ~ /\\.(?!well-known/) {{\n        return 404;\n    }}\n\n",
        spec.folder
    );
    if spec.kind == Kind::Php {
        body.push_str(
            "    # What visitors upload is served as files; PHP there never runs.\n    \
             location ^~ /uploads/ {\n        location ~ \\.php$ {\n            \
             return 404;\n        }\n    }\n\n    \
             location ~ \\.php$ {\n        try_files $uri =404;\n        \
             fastcgi_pass 127.0.0.1:9000;\n        include fastcgi.conf;\n    }\n\n",
        );
    } else {
        body.push_str("    location ~ \\.php$ {\n        return 404;\n    }\n\n");
    }
    body.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
    body
}

/// The text of `_default.conf`.
pub fn default_text() -> &'static str {
    "# The answer to an HTTPS request for a name no website has: the\n\
     # handshake is refused (website(8), ADR 0569).\n\
     server {\n    listen 443 ssl default_server;\n    listen [::]:443 ssl default_server;\n    \
     ssl_reject_handshake on;\n}\n"
}

/// The starter page of a site `name` of kind `kind` in `folder`: the
/// file's name and its text.
pub fn starter(kind: Kind, name: &str, folder: &str) -> (&'static str, String) {
    let words = format!("{name} works. Put your site's files in {folder}.");
    match kind {
        Kind::Php => ("index.php", format!("<?php\necho \"{words}\";\n")),
        _ => (
            "index.html",
            format!(
                "<!doctype html>\n<meta charset=\"utf-8\">\n<title>{name}</title>\n<p>{words}</p>\n"
            ),
        ),
    }
}

/// The spec a file with `facts` would be the file of, when `website`
/// could write one like it.
pub fn spec_of(name: &str, facts: &Facts) -> Option<Spec> {
    if facts.kind == Kind::Other || facts.https && facts.certificate.is_none() {
        return None;
    }
    Some(Spec {
        name: name.to_string(),
        names: facts.names.clone(),
        kind: facts.kind,
        folder: facts.folder.clone()?,
        certificate: facts.certificate.clone().filter(|_| facts.https),
    })
}

/// Whether the file `text` of site `name`, which has `facts`, differs from
/// what `website` writes for those facts.
pub fn by_hand(name: &str, text: &str, facts: &Facts) -> bool {
    spec_of(name, facts).is_none_or(|spec| site_text(&spec) != text)
}

/// Whether the file `text` of site `name` does not start as `website`
/// starts the file of a site.
pub fn added_by_hand(name: &str, text: &str) -> bool {
    text.lines().next() != Some(first_line(name).as_str())
}

/// What the websites need to know of `nginx.conf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Main {
    /// Whether the `http` block includes the sites' files.
    pub includes: bool,
    /// Whether a server of it answers 404 under `/sites/`.
    pub hides_sites: bool,
    /// The names its servers answer to.
    pub names: Vec<String>,
    /// The certificates of certd's its `ssl_certificate` lines name.
    pub certificates: Vec<String>,
    /// Whether one of its servers listens on port 443.
    pub listens_443: bool,
    /// The place of the include's argument in the text.
    include_at: Option<Range<usize>>,
}

/// Reads the text of `nginx.conf` as far as the websites need it.
///
/// # Errors
///
/// When it does not read as nginx's syntax, with the line.
pub fn read_main(text: &str) -> Result<Main, String> {
    let directives = conf::parse(text)?;
    let mut main = Main {
        includes: false,
        hides_sites: false,
        names: Vec::new(),
        certificates: Vec::new(),
        listens_443: false,
        include_at: None,
    };
    conf::visit(&directives, &mut |directive: &Directive, parents: &[&str]| {
        let in_server = parents.contains(&"server");
        match directive.name.text.as_str() {
            "include" if parents.contains(&"http") && directive.arg(0) == Some(INCLUDE) => {
                main.includes = true;
                main.include_at =
                    main.include_at.take().or(directive.args.first().map(|a| a.span.clone()));
            }
            "location" if in_server && directive.args.len() == 2 => {
                let hides = directive.arg(0) == Some("^~") && directive.arg(1) == Some("/sites/");
                let answers = directive
                    .children()
                    .iter()
                    .any(|inner| inner.is("return") && inner.arg(0) == Some("404"));
                main.hides_sites |= hides && answers;
            }
            "server_name" if in_server => {
                for word in &directive.args {
                    if word.text != "_" && !main.names.contains(&word.text) {
                        main.names.push(word.text.clone());
                    }
                }
            }
            "listen" if in_server => {
                main.listens_443 |= directive.arg(0).and_then(listen_port) == Some(443);
            }
            "ssl_certificate" if in_server => {
                if let Some(name) = directive.arg(0).and_then(certificate_of)
                    && !main.certificates.iter().any(|known| known == name)
                {
                    main.certificates.push(name.to_string());
                }
            }
            _ => {}
        }
    });
    Ok(main)
}

impl Main {
    /// `text`, the file this was read from, with the argument of its
    /// include of the sites' files set to `pattern`: the copy nginx
    /// checks while the new sites are not live.
    ///
    /// # Errors
    ///
    /// When there is no such include.
    pub fn with_include(&self, text: &str, pattern: &str) -> Result<String, String> {
        let span = self.include_at.clone().ok_or("it does not include the sites' files")?;
        Ok(format!("{}{pattern}{}", &text[..span.start], &text[span.end..]))
    }
}

/// The lines to add to `nginx.conf` for the checks of `main` that fail,
/// as words that end in what to do.
pub fn conf_words(path: &str, main: &Main) -> Option<String> {
    let mut words = Vec::new();
    if !main.includes {
        words.push(format!(
            "it does not include the websites' folder: add this line at the end of its http \
             block, after the servers:\n    {INCLUDE_LINE}"
        ));
    }
    if !main.hides_sites {
        let lines = HIDE_LINES.map(|line| format!("    {line}")).join("\n");
        words.push(format!(
            "its default server does not answer 404 under /sites/: add this inside it, before \
             location / {{:\n{lines}"
        ));
    }
    if words.is_empty() {
        return None;
    }
    Some(format!("nginx's settings, {path}, {}", words.join("\nand ")))
}

/// What nginx's service file holds of what the sites need, with the
/// override over it.
struct Effective {
    /// `certificates`.
    certificates: Vec<String>,
    /// `permissions.files.read`.
    read: Vec<String>,
    /// `[[sockets]]`.
    sockets: Vec<toml::Table>,
}

/// The tables of `file` with the keys of `over` set over them, as Spark
/// reads an override: a table key by key, anything else in place.
fn merged(file: &toml::Table, over: &toml::Table) -> toml::Table {
    let mut out = file.clone();
    for (key, value) in over {
        if let (Some(toml::Value::Table(inner)), toml::Value::Table(more)) = (out.get(key), value) {
            let both = merged(inner, more);
            out.insert(key.clone(), toml::Value::Table(both));
        } else {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

/// The list of words `value` is, or none when there is no such key.
fn words(value: Option<&toml::Value>, key: &str) -> Result<Vec<String>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{key} holds a value that is not text"))
            })
            .collect(),
        Some(_) => Err(format!("{key} is not a list")),
    }
}

/// The `[[sockets]]` tables `value` holds.
fn socket_tables(value: Option<&toml::Value>) -> Result<Vec<toml::Table>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|item| item.as_table().cloned().ok_or_else(|| "a socket is not a table".into()))
            .collect(),
        Some(_) => Err("sockets is not a list of tables".into()),
    }
}

/// What the service file `file` holds with `over` set over it.
fn effective(file: &toml::Table, over: &toml::Table) -> Result<Effective, String> {
    let all = merged(file, over);
    let read = all.get("permissions").and_then(|p| p.get("files")).and_then(|f| f.get("read"));
    Ok(Effective {
        certificates: words(all.get("certificates"), "certificates")?,
        read: words(read, "permissions.files.read")?,
        sockets: socket_tables(all.get("sockets"))?,
    })
}

/// Reads `text`, a settings file of TOML, as a table.
fn table_of(text: &str, what: &str) -> Result<toml::Table, String> {
    toml::from_str(text).map_err(|error| format!("{what} does not read: {}", error.message()))
}

/// The certificates nginx's service file, `file`, names with `over` set
/// over it.
///
/// # Errors
///
/// When either does not read.
pub fn named_certificates(file: &str, over: Option<&str>) -> Result<Vec<String>, String> {
    let over = table_of(over.unwrap_or_default(), "nginx's override")?;
    Ok(effective(&table_of(file, "nginx's service file")?, &over)?.certificates)
}

/// What the override of nginx's service file has to hold for the sites.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    /// The certificates that must be named.
    pub add: Vec<String>,
    /// The certificates that no site and no line of `nginx.conf` names
    /// any more.
    pub remove: Vec<String>,
    /// Whether a socket for port 443 is needed.
    pub https: bool,
}

/// The keys of a socket in the order they are written, before any other.
const SOCKET_KEYS: [&str; 4] = ["port", "address", "name", "non-blocking"];

/// The names of the sockets for 443 that `website` writes.
const SOCKETS_443: [(&str, &str); 2] = [("https", "0.0.0.0"), ("https6", "::")];

/// The text the override of nginx's service file `file` should have, with
/// `over` the text it has now, once it holds `wanted`; none when it
/// should stay as it is. A new override starts with [`HEADER`].
///
/// # Errors
///
/// When the file or the override does not read, or holds what `website`
/// cannot change in place.
pub fn override_text(
    file: &str,
    over: Option<&str>,
    wanted: &Wanted,
) -> Result<Option<String>, String> {
    let file = table_of(file, "nginx's service file")?;
    let current = table_of(over.unwrap_or_default(), "nginx's override")?;
    let now = effective(&file, &current)?;
    let bundled = effective(&file, &toml::Table::new())?;
    let edit = |document: &mut DocumentMut| -> Result<(), String> {
        edit_certificates(document, &now, &bundled, wanted)?;
        edit_read(document, &now)?;
        edit_sockets(document, &now, &bundled, wanted)
    };
    let made = match over {
        Some(text) => settings_edit::edited(text, edit),
        None => {
            settings_edit::edited("", edit).map(|made| made.map(|body| format!("{HEADER}\n{body}")))
        }
    };
    made.map_err(|why| match why {
        Unedited::Parse { message, .. } => format!("nginx's override does not read: {message}"),
        Unedited::Edit(why) => why,
    })
}

/// Makes `certificates` hold what the sites name and no more of what
/// they left.
fn edit_certificates(
    document: &mut DocumentMut,
    now: &Effective,
    bundled: &Effective,
    wanted: &Wanted,
) -> Result<(), String> {
    let mut want = now.certificates.clone();
    for name in &wanted.add {
        if !want.contains(name) {
            want.push(name.clone());
        }
    }
    want.retain(|name| !wanted.remove.contains(name));
    if want == now.certificates {
        return Ok(());
    }
    let top = document.as_table_mut();
    if top.contains_key("certificates") {
        for name in &wanted.add {
            if !settings_edit::push_string(top, "certificates", name)
                && !now.certificates.contains(name)
            {
                return Err("its certificates is not a list of names".into());
            }
        }
        for name in &wanted.remove {
            settings_edit::remove_string(top, "certificates", name);
        }
        if want == bundled.certificates {
            settings_edit::remove_key(top, "certificates");
        }
    } else {
        settings_edit::set_strings(top, "certificates", &want);
    }
    Ok(())
}

/// Adds certd's folder to the files the service reads, which its
/// override replaces whole.
fn edit_read(document: &mut DocumentMut, now: &Effective) -> Result<(), String> {
    if now.read.iter().any(|path| path == CERTD) {
        return Ok(());
    }
    let permissions =
        settings_edit::table(document, "permissions").ok_or("its permissions is not a table")?;
    if permissions.get("files").is_none() {
        let mut files = Table::new();
        files.set_dotted(true);
        permissions.insert("files", Item::Table(files));
    }
    let files = permissions
        .get_mut("files")
        .and_then(Item::as_table_like_mut)
        .ok_or("its permissions.files is not a table")?;
    if files.contains_key("read") {
        if !settings_edit::push_string(files, "read", CERTD) {
            return Err("its permissions.files.read is not a list of paths".into());
        }
    } else {
        let mut all = now.read.clone();
        all.push(CERTD.to_string());
        settings_edit::set_strings(files, "read", &all);
    }
    Ok(())
}

/// Whether `socket` is a TCP socket for port 443.
fn is_443(socket: &toml::Table) -> bool {
    socket.get("port").and_then(toml::Value::as_integer) == Some(443)
        && socket.get("udp").and_then(toml::Value::as_bool) != Some(true)
}

/// Whether `socket` is one `website` wrote for 443.
fn is_ours(socket: &toml::Table) -> bool {
    is_443(socket)
        && SOCKETS_443
            .iter()
            .any(|(name, _)| socket.get("name").and_then(toml::Value::as_str) == Some(*name))
}

/// The value `value` as TOML the editor writes.
fn edit_value(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::from(text.as_str()),
        toml::Value::Integer(number) => Value::from(*number),
        toml::Value::Float(number) => Value::from(*number),
        toml::Value::Boolean(switch) => Value::from(*switch),
        toml::Value::Datetime(time) => time
            .to_string()
            .parse::<toml_edit::Datetime>()
            .map_or_else(|_| Value::from(time.to_string()), Value::from),
        toml::Value::Array(items) => Value::Array(items.iter().map(edit_value).collect()),
        toml::Value::Table(inner) => {
            let mut inline = toml_edit::InlineTable::new();
            for (key, item) in inner {
                inline.insert(key, edit_value(item));
            }
            Value::InlineTable(inline)
        }
    }
}

/// Adds the `[[sockets]]` table `socket` to `document`.
fn push_socket(document: &mut DocumentMut, socket: &toml::Table) -> Result<(), String> {
    let table = settings_edit::push_table(document, "sockets")
        .ok_or("its sockets are written as a list, which website does not change: add the tables for port 443 yourself")?;
    // The keys in the order the bundle's own file has them, which
    // nginx's start-up script reads by line.
    let first = SOCKET_KEYS.iter().filter_map(|key| socket.get_key_value(*key));
    let rest = socket.iter().filter(|(key, _)| !SOCKET_KEYS.contains(&key.as_str()));
    for (key, value) in first.chain(rest) {
        table.insert(key, Item::Value(edit_value(value)));
    }
    Ok(())
}

/// Makes the sockets hold the two for 443 when the sites need that port,
/// and not when nothing listens there.
fn edit_sockets(
    document: &mut DocumentMut,
    now: &Effective,
    bundled: &Effective,
    wanted: &Wanted,
) -> Result<(), String> {
    let has = now.sockets.iter().any(is_443);
    if wanted.https && !has {
        if !document.contains_key("sockets") {
            for socket in &now.sockets {
                push_socket(document, socket)?;
            }
        }
        for (name, address) in SOCKETS_443 {
            let mut socket = toml::Table::new();
            socket.insert("port".into(), toml::Value::Integer(443));
            socket.insert("address".into(), toml::Value::String(address.into()));
            socket.insert("name".into(), toml::Value::String(name.into()));
            socket.insert("non-blocking".into(), toml::Value::Boolean(true));
            push_socket(document, &socket)?;
        }
    } else if !wanted.https && now.sockets.iter().any(is_ours) && document.contains_key("sockets") {
        while settings_edit::remove_table(document, "sockets", |table| {
            table.get("port").and_then(Item::as_integer) == Some(443)
                && SOCKETS_443
                    .iter()
                    .any(|(name, _)| table.get("name").and_then(Item::as_str) == Some(*name))
        }) {}
        // What is left is the bundle's own: the key goes, so that the
        // bundle's next version of its sockets reaches the service.
        let left: Vec<toml::Table> =
            now.sockets.iter().filter(|socket| !is_ours(socket)).cloned().collect();
        if left == bundled.sockets {
            document.remove("sockets");
        }
    }
    Ok(())
}

/// The first name of `names` that a site file or `nginx.conf` already
/// answers to, with the file it is in.
pub fn taken<'a>(
    names: &'a [String],
    sites: &[(String, Vec<String>)],
    main: Option<(&str, &Main)>,
) -> Option<(&'a str, String)> {
    for name in names {
        if let Some((file, _)) = sites.iter().find(|(_, known)| known.contains(name)) {
            return Some((name, file.clone()));
        }
        if let Some((file, main)) = main
            && main.names.contains(name)
        {
            return Some((name, file.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: Kind, certificate: Option<&str>) -> Spec {
        Spec {
            name: "shop.example.org".into(),
            names: vec!["shop.example.org".into(), "www.shop.example.org".into()],
            kind,
            folder: "/shared/www/sites/shop.example.org".into(),
            certificate: certificate.map(Into::into),
        }
    }

    /// The text of nginx's service file as bundled writes it.
    const FILE: &str = crate::website::fake::NGINX_SERVICE;

    fn wanted(add: &[&str], remove: &[&str], https: bool) -> Wanted {
        Wanted {
            add: add.iter().map(ToString::to_string).collect(),
            remove: remove.iter().map(ToString::to_string).collect(),
            https,
        }
    }

    #[test]
    fn a_site_over_https_with_php_is_the_file_the_design_shows() {
        let text = site_text(&spec(Kind::Php, Some("shop.example.org")));
        let shown = "# shop.example.org, a website added by website(8) (ADR 0569).\n\
# This file is the site: a change made here is kept and served, and\n\
# the web console shows the site as changed by hand.\n\
server {\n    listen 80;\n    listen [::]:80;\n    server_name shop.example.org www.shop.example.org;\n    \
include acme-challenge.conf;\n    location / {\n        return 301 https://$host$request_uri;\n    }\n}\n\n\
server {\n    listen 443 ssl;\n    listen [::]:443 ssl;\n    http2 on;\n    \
server_name shop.example.org www.shop.example.org;\n    \
ssl_certificate /system/live/certificates/nginx/shop.example.org/fullchain.pem;\n    \
ssl_certificate_key /system/live/certificates/nginx/shop.example.org/key.pem;\n    \
root /shared/www/sites/shop.example.org;\n    index index.php index.html;\n    \
include acme-challenge.conf;\n\n    \
# Files whose names start with a dot (.git, .env) are never served.\n    \
location ~ /\\.(?!well-known/) {\n        return 404;\n    }\n\n    \
# What visitors upload is served as files; PHP there never runs.\n    \
location ^~ /uploads/ {\n        location ~ \\.php$ {\n            return 404;\n        }\n    }\n\n    \
location ~ \\.php$ {\n        try_files $uri =404;\n        fastcgi_pass 127.0.0.1:9000;\n        \
include fastcgi.conf;\n    }\n\n    location / {\n        try_files $uri $uri/ =404;\n    }\n}\n";
        assert_eq!(text, shown);
    }

    #[test]
    fn a_static_site_has_no_uploads_location_and_no_php_and_http_is_one_server() {
        let text = site_text(&spec(Kind::Static, None));
        assert!(!text.contains("uploads") && !text.contains("fastcgi"), "{text}");
        assert!(text.contains("location ~ \\.php$ {\n        return 404;"), "{text}");
        assert!(text.contains("index index.html index.htm;"), "{text}");
        assert_eq!(text.matches("server {").count(), 1, "{text}");
        assert!(text.contains("include acme-challenge.conf;") && !text.contains("ssl"), "{text}");
    }

    #[test]
    fn what_website_writes_reads_back_as_its_own_and_as_the_spec_it_came_from() {
        for (kind, certificate) in [
            (Kind::Static, None),
            (Kind::Php, None),
            (Kind::Static, Some("shop.example.org")),
            (Kind::Php, Some("this-machine")),
        ] {
            let made = spec(kind, certificate);
            let text = site_text(&made);
            let facts = conf::facts(&conf::parse(&text).unwrap());
            assert_eq!(facts.kind, kind);
            assert_eq!(facts.https, certificate.is_some());
            assert_eq!(facts.redirects_http, certificate.is_some());
            assert_eq!(spec_of("shop.example.org", &facts), Some(made));
            assert!(!by_hand("shop.example.org", &text, &facts), "{text}");
            assert!(!added_by_hand("shop.example.org", &text));
        }
    }

    #[test]
    fn a_change_a_comment_or_another_server_makes_a_site_by_hand() {
        let text = site_text(&spec(Kind::Static, None));
        let appended = format!("{text}# lab: kept by hand\n");
        let facts = conf::facts(&conf::parse(&appended).unwrap());
        assert!(by_hand("shop.example.org", &appended, &facts));
        assert!(!added_by_hand("shop.example.org", &appended));
        let proxy = "server { listen 80; server_name legacy.example.org; proxy_pass http://127.0.0.1:8080; }\n";
        let facts = conf::facts(&conf::parse(proxy).unwrap());
        assert!(by_hand("legacy.example.org", proxy, &facts));
        assert!(added_by_hand("legacy.example.org", proxy));
        assert!(added_by_hand("other.example.org", &text));
    }

    #[test]
    fn the_starter_page_says_what_works_in_html_or_php() {
        let (file, text) =
            starter(Kind::Static, "shop.example.org", "/shared/www/sites/shop.example.org");
        assert_eq!(file, "index.html");
        assert!(text.contains(
            "<p>shop.example.org works. Put your site's files in /shared/www/sites/shop.example.org.</p>"
        ));
        let (file, text) =
            starter(Kind::Php, "shop.example.org", "/shared/www/sites/shop.example.org");
        assert_eq!(file, "index.php");
        assert_eq!(
            text,
            "<?php\necho \"shop.example.org works. Put your site's files in /shared/www/sites/shop.example.org.\";\n"
        );
        assert!(!text.contains("phpinfo"));
    }

    #[test]
    fn nginx_conf_is_read_for_its_include_its_hidden_sites_and_what_it_serves() {
        let text = include_str!("../../../../../ports/nginx/nginx.conf");
        let main = read_main(text).unwrap();
        assert!(main.includes && main.hides_sites, "the bundle's own file has both lines");
        assert!(main.names.is_empty(), "its server's name is _: {:?}", main.names);
        assert!(main.certificates.is_empty() && !main.listens_443);
        // The sample HTTPS server is a comment until a person turns it on.
        let start = text.find("    #server {").unwrap();
        let end = start + text[start..].find("\n    #}").unwrap() + "\n    #}".len();
        let uncommented =
            text[start..end].replace("\n    #", "\n    ").replacen("    #", "    ", 1);
        let on = format!("{}{uncommented}{}", &text[..start], &text[end..]);
        let on = read_main(&on).unwrap();
        assert!(on.listens_443);
        assert_eq!(on.certificates, ["this-machine"]);
        let old = text.replace(INCLUDE_LINE, "").replace("location ^~ /sites/", "location /x/");
        let old = read_main(&old).unwrap();
        assert!(!old.includes && !old.hides_sites);
    }

    #[test]
    fn the_include_counts_only_in_the_http_block_and_the_hide_only_with_a_404() {
        let outside = "include /system/settings/websites/*.conf;\nhttp { server { location ^~ /sites/ { return 200; } } }";
        let main = read_main(outside).unwrap();
        assert!(!main.includes && !main.hides_sites);
        let named = "http { include \"/system/settings/websites/*.conf\"; server { server_name _ a.example.org; \
                     listen [::]:443 ssl; ssl_certificate /system/live/certificates/nginx/a.example.org/fullchain.pem; } }";
        let main = read_main(named).unwrap();
        assert!(main.includes && main.listens_443);
        assert_eq!(main.names, ["a.example.org"]);
        assert_eq!(main.certificates, ["a.example.org"]);
        assert!(read_main("http { server { ").unwrap_err().contains("not closed"));
    }

    #[test]
    fn the_copy_nginx_checks_includes_the_staged_folder_and_nothing_else_changes() {
        let text = "http {\n    include mime.types;\n    # a comment\n    include   '/system/settings/websites/*.conf'  ;\n}\n";
        let main = read_main(text).unwrap();
        let copy = main.with_include(text, "/conf/.website-next/*.conf").unwrap();
        assert_eq!(
            copy,
            "http {\n    include mime.types;\n    # a comment\n    include   /conf/.website-next/*.conf  ;\n}\n"
        );
        let none = read_main("http { }").unwrap();
        assert!(none.with_include("http { }", "x").is_err());
    }

    #[test]
    fn the_words_for_a_conf_that_lacks_a_line_say_which_and_where_it_goes() {
        let mut main = read_main("http { server { listen 80; } }").unwrap();
        let words = conf_words("/n/conf/nginx.conf", &main).unwrap();
        assert!(
            words.starts_with("nginx's settings, /n/conf/nginx.conf, it does not include"),
            "{words}"
        );
        assert!(words.contains("\n    include /system/settings/websites/*.conf;"), "{words}");
        assert!(words.contains("location ^~ /sites/ {\n        return 404;\n    }"), "{words}");
        main.includes = true;
        assert!(!conf_words("x", &main).unwrap().contains("include /system"));
        main.hides_sites = true;
        assert_eq!(conf_words("x", &main), None);
    }

    #[test]
    fn the_first_site_adds_its_certificate_the_file_to_read_and_the_sockets_for_443() {
        let made =
            override_text(FILE, None, &wanted(&["shop.example.org"], &[], true)).unwrap().unwrap();
        assert!(made.starts_with(HEADER), "{made}");
        assert!(made.contains("certificates = [\"shop.example.org\"]"), "{made}");
        assert!(
            made.contains(
                "[permissions]\nfiles.read = [\"/applications/org.nginx.nginx\", \"/shared/www\", \"/system/live/certd\"]"
            ),
            "{made}"
        );
        // Both of the bundle's sockets, then the two for 443, whole.
        assert_eq!(made.matches("[[sockets]]").count(), 4, "{made}");
        for line in [
            "port = 80\naddress = \"0.0.0.0\"\nname = \"http\"\nnon-blocking = true",
            "port = 443\naddress = \"0.0.0.0\"\nname = \"https\"\nnon-blocking = true",
            "port = 443\naddress = \"::\"\nname = \"https6\"\nnon-blocking = true",
        ] {
            assert!(made.contains(line), "{line}\n{made}");
        }
        // It reads as Spark reads an override, and says what was meant.
        let over = manifest::service::Override::from_toml(&made).unwrap();
        let read = manifest::service::from_toml_over(FILE, Some(&over)).unwrap();
        assert_eq!(read.certificates, ["shop.example.org"]);
        assert_eq!(read.sockets.len(), 4);
        assert!(read.permissions.files.read.iter().any(|path| path == CERTD));
        assert!(read.permissions.files.read.iter().any(|path| path == "/shared/www"));
        // The bundle's own keys that the override did not name stay.
        assert_eq!(read.permissions.files.read_write, ["/shared/services/org.nginx.nginx"]);
    }

    #[test]
    fn a_second_site_that_needs_nothing_new_writes_nothing() {
        let made =
            override_text(FILE, None, &wanted(&["a.example.org"], &[], true)).unwrap().unwrap();
        assert_eq!(
            override_text(FILE, Some(&made), &wanted(&["a.example.org"], &[], true)).unwrap(),
            None
        );
        let next = override_text(FILE, Some(&made), &wanted(&["b.example.org"], &[], true))
            .unwrap()
            .unwrap();
        assert!(next.contains("certificates = [\"a.example.org\", \"b.example.org\"]"), "{next}");
        assert_eq!(next.matches("[[sockets]]").count(), 4);
        // A site over HTTP needs the folder certd answers from, and no more.
        let plain = override_text(FILE, None, &wanted(&[], &[], false)).unwrap().unwrap();
        assert!(plain.contains("/system/live/certd"), "{plain}");
        assert!(!plain.contains("certificates") && !plain.contains("[[sockets]]"), "{plain}");
        assert_eq!(override_text(FILE, Some(&plain), &wanted(&[], &[], false)).unwrap(), None);
    }

    #[test]
    fn what_a_person_wrote_in_the_override_is_kept_and_their_lists_are_added_to() {
        let over = "# mine\nlimits.memory = \"1G\"\ncertificates = [\"mine.example.org\"]\n\n\
                    [permissions]\nfiles.read = [\"/shared/www\", \"/mine\"]  # my folders\n";
        let made = override_text(FILE, Some(over), &wanted(&["shop.example.org"], &[], false))
            .unwrap()
            .unwrap();
        assert!(made.starts_with("# mine\nlimits.memory = \"1G\"\n"), "{made}");
        assert!(
            made.contains("certificates = [\"mine.example.org\", \"shop.example.org\"]"),
            "{made}"
        );
        assert!(
            made.contains(
                "files.read = [\"/shared/www\", \"/mine\", \"/system/live/certd\"]  # my folders"
            ),
            "{made}"
        );
        assert!(!made.contains("[[sockets]]"), "no 443 asked: {made}");
        let reading = manifest::service::Override::from_toml(&made).unwrap();
        assert!(manifest::service::from_toml_over(FILE, Some(&reading)).is_ok());
    }

    #[test]
    fn the_last_site_takes_its_certificate_and_the_sockets_away_and_nothing_else() {
        let made =
            override_text(FILE, None, &wanted(&["a.example.org"], &[], true)).unwrap().unwrap();
        let back = override_text(FILE, Some(&made), &wanted(&[], &["a.example.org"], false))
            .unwrap()
            .unwrap();
        assert!(!back.contains("a.example.org") && !back.contains("[[sockets]]"), "{back}");
        assert!(!back.contains("certificates"), "the bundle's own list is none: {back}");
        // The file to read stays: it is never taken away.
        assert!(back.contains("/system/live/certd"), "{back}");
        let reading = manifest::service::Override::from_toml(&back).unwrap();
        let read = manifest::service::from_toml_over(FILE, Some(&reading)).unwrap();
        assert!(read.certificates.is_empty());
        assert_eq!(read.sockets.len(), 2);
        // Another site's certificate stays, and the sockets while 443 is wanted.
        let two = override_text(FILE, Some(&made), &wanted(&["b.example.org"], &[], true))
            .unwrap()
            .unwrap();
        let one = override_text(FILE, Some(&two), &wanted(&[], &["a.example.org"], true))
            .unwrap()
            .unwrap();
        assert!(one.contains("certificates = [\"b.example.org\"]"), "{one}");
        assert_eq!(one.matches("[[sockets]]").count(), 4, "{one}");
    }

    #[test]
    fn a_certificate_the_service_file_names_leaves_through_the_override() {
        // The key comes before the tables, as a file keeps its top-level keys.
        let file = format!("certificates = [\"this-machine\"]\n{FILE}");
        let made =
            override_text(&file, None, &wanted(&["a.example.org"], &[], false)).unwrap().unwrap();
        assert!(made.contains("certificates = [\"this-machine\", \"a.example.org\"]"), "{made}");
        let back = override_text(&file, Some(&made), &wanted(&[], &["a.example.org"], false))
            .unwrap()
            .unwrap();
        assert!(!back.contains("certificates"), "it is the file's again: {back}");
        // The file's own list is not written over for a name that is not
        // in it.
        let kept =
            override_text(&file, None, &wanted(&[], &["a.example.org"], false)).unwrap().unwrap();
        assert!(!kept.contains("certificates"), "{kept}");
        // A name only the file has leaves through an override that says so.
        let gone =
            override_text(&file, None, &wanted(&[], &["this-machine"], false)).unwrap().unwrap();
        assert!(gone.contains("certificates = []"), "{gone}");
    }

    #[test]
    fn a_socket_the_person_made_for_443_is_theirs_and_stays() {
        let file = format!(
            "{FILE}\n[[sockets]]\nport = 443\naddress = \"::\"\nname = \"mine\"\nnon-blocking = true\n"
        );
        let made = override_text(&file, None, &wanted(&[], &[], true)).unwrap().unwrap();
        assert!(!made.contains("[[sockets]]"), "443 is held already: {made}");
        let back = override_text(&file, Some(&made), &wanted(&[], &[], false)).unwrap();
        assert_eq!(back, None, "none of the sockets is website's");
    }

    #[test]
    fn what_the_override_holds_wrongly_is_refused_and_nothing_is_written() {
        for over in [
            "certificates = \"x\"\n",
            "certificates = [1]\n",
            "permissions = 3\n",
            "[permissions]\nfiles.read = \"x\"\n",
            "sockets = [{ port = 80 }]\n",
            "this is not toml",
        ] {
            let why = override_text(FILE, Some(over), &wanted(&["a.example.org"], &[], true));
            assert!(why.is_err(), "{over}: {why:?}");
        }
        assert!(override_text("not = [toml", None, &wanted(&[], &[], false)).is_err());
    }

    #[test]
    fn the_certificates_a_service_file_names_are_the_override_s_when_it_has_a_list() {
        assert_eq!(named_certificates(FILE, None).unwrap(), Vec::<String>::new());
        assert_eq!(named_certificates(FILE, Some("certificates = [\"a\"]")).unwrap(), ["a"]);
        let file = format!("certificates = [\"b\"]\n{FILE}");
        assert_eq!(named_certificates(&file, None).unwrap(), ["b"]);
        assert_eq!(
            named_certificates(&file, Some("certificates = []")).unwrap(),
            Vec::<String>::new()
        );
        assert!(named_certificates("x = [", None).is_err());
    }

    #[test]
    fn a_name_that_a_site_file_or_nginx_conf_answers_to_is_taken() {
        let sites = vec![("a.example.org.conf".to_string(), vec!["a.example.org".to_string()])];
        let main = read_main("http { server { server_name b.example.org; } }").unwrap();
        let names = vec!["c.example.org".to_string(), "b.example.org".to_string()];
        assert_eq!(
            taken(&names, &sites, Some(("nginx.conf", &main))),
            Some(("b.example.org", "nginx.conf".into()))
        );
        assert_eq!(taken(&names, &sites, None), None);
        let names = vec!["a.example.org".to_string()];
        assert_eq!(
            taken(&names, &sites, None),
            Some(("a.example.org", "a.example.org.conf".into()))
        );
    }

    #[test]
    fn the_default_file_refuses_a_handshake_for_a_name_no_site_has() {
        let facts = conf::facts(&conf::parse(default_text()).unwrap());
        assert!(facts.https && facts.names.is_empty());
        assert!(default_text().contains("ssl_reject_handshake on;"));
        assert!(default_text().contains("default_server"));
    }
}
