// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A reader of the part of YAML 1.2 that cloud-init's files are written in
//! (ADR 0328): `#cloud-config` user data, NoCloud's `meta-data` and
//! `network-config`.
//!
//! It reads block mappings and lists by their indentation, a list's
//! entries at the indentation of their key or deeper, plain, single- and
//! double-quoted scalars, literal (`|`) and folded (`>`) block scalars
//! with their chomping, flow lists and mappings (`[a, b]`, `{k: v}`) over
//! one line or several, comments, and the `---` that starts a document.
//! A scalar is kept as its text: what a key means says whether `yes` is
//! a truth or a word ([`Value::truth`]). Anchors, aliases, complex keys
//! (`? `) and a stream of several documents are not read, and say so,
//! rather than being read wrongly: cloud-init's files do not use them.
//!
//! If a file is read wrongly, start with [`Reader::block`], which picks
//! what a line starts, and [`Reader::value_after`], which reads what
//! follows a key.

use std::fmt;

/// A value of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `~`, `null`, or nothing after a key.
    Null,
    /// A scalar, plain, quoted or a block's, as its text.
    Text(String),
    /// A list, in order.
    List(Vec<Value>),
    /// A mapping, its keys in the file's order.
    Map(Vec<(String, Value)>),
}

impl Value {
    /// The value of `key`, if this is a mapping that has it and its value
    /// is not null.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
                .filter(|value| **value != Value::Null),
            _ => None,
        }
    }

    /// The text of a scalar.
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// A scalar read as a truth, as cloud-init reads one: `true`, `yes`
    /// and `on` are true, `false`, `no` and `off` false, in any case.
    pub fn truth(&self) -> Option<bool> {
        let text = self.text()?.to_ascii_lowercase();
        match text.as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    /// The entries of a list; a scalar is a list of itself, as cloud-init
    /// takes a single key where it takes a list; nothing for the rest.
    pub fn items(&self) -> Vec<&Value> {
        match self {
            Value::List(items) => items.iter().collect(),
            Value::Text(_) => vec![self],
            _ => Vec::new(),
        }
    }

    /// The texts of [`Value::items`] that are scalars.
    pub fn texts(&self) -> Vec<&str> {
        self.items().into_iter().filter_map(Value::text).collect()
    }
}

/// What is wrong with a file, and on which line, counted from 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The line.
    pub line: usize,
    /// What is wrong there.
    pub what: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.what)
    }
}

/// Reads `text`, a YAML document; null if it holds nothing.
pub fn parse(text: &str) -> Result<Value, Error> {
    let mut reader = Reader { lines: text.lines().collect(), at: 0, inline: None, started: false };
    let value = match reader.peek()? {
        Some(line) => reader.block(line, None)?,
        None => Value::Null,
    };
    match reader.peek()? {
        Some(line) => Err(line.error("this line does not fit the indentation of those before it")),
        None => Ok(value),
    }
}

/// A line that holds something: its index in the file, the number of
/// spaces before it, and the rest, without spaces at its end.
#[derive(Clone, Copy, Debug)]
struct Line<'a> {
    index: usize,
    indent: usize,
    text: &'a str,
}

impl Line<'_> {
    /// An error on this line.
    fn error(&self, what: &str) -> Error {
        Error { line: self.index + 1, what: what.into() }
    }
}

/// The lines of a file, read one block at a time.
struct Reader<'a> {
    lines: Vec<&'a str>,
    /// The index of the next line to read.
    at: usize,
    /// What follows a list entry's `- ` on its line, read as a line of its
    /// own at that column: `- name: x` starts a mapping there.
    inline: Option<Line<'a>>,
    /// Whether a line that holds something was read: a `---` after one
    /// starts a second document.
    started: bool,
}

impl<'a> Reader<'a> {
    /// The next line that holds something, without taking it: blank
    /// lines, comments and a document's start are passed over.
    fn peek(&mut self) -> Result<Option<Line<'a>>, Error> {
        if let Some(line) = self.inline {
            return Ok(Some(line));
        }
        while let Some(&raw) = self.lines.get(self.at) {
            let trimmed = raw.trim_end();
            let text = trimmed.trim_start_matches(' ');
            let indent = trimmed.len() - text.len();
            let line = Line { index: self.at, indent, text };
            if text.starts_with('\t') {
                return Err(line.error("a tab indents this line; YAML indents with spaces"));
            }
            let start = indent == 0 && (text == "---" || text.starts_with("--- #"));
            if text.is_empty() || text.starts_with('#') || (start && !self.started) {
                self.at += 1;
                continue;
            }
            if indent == 0 && (text.starts_with("---") || text == "...") {
                return Err(line.error("only one document is read"));
            }
            self.started = true;
            return Ok(Some(line));
        }
        Ok(None)
    }

    /// Takes `line`: the next line to read is the one after it.
    fn take(&mut self, line: Line<'_>) {
        self.inline = None;
        self.at = line.index + 1;
    }

    /// Reads the block that starts at `line`: a list, a mapping or a
    /// scalar, by what the line starts with. A scalar goes on over the
    /// lines after it that are more indented than `parent`, the column of
    /// what holds it: any lines, if nothing does.
    fn block(&mut self, line: Line<'a>, parent: Option<usize>) -> Result<Value, Error> {
        if entry(line.text).is_some() {
            return self.list(line.indent);
        }
        if key(line.text).map_err(|what| line.error(&what))?.is_some() {
            return self.mapping(line.indent);
        }
        self.take(line);
        self.scalar(line.text, line, parent)
    }

    /// Reads a list whose `- ` are at column `indent`.
    fn list(&mut self, indent: usize) -> Result<Value, Error> {
        let mut items = Vec::new();
        while let Some(line) = self.peek()? {
            let Some(rest) = entry(line.text).filter(|_| line.indent == indent) else { break };
            self.take(line);
            if rest.is_empty() || rest.starts_with('#') {
                // The entry is on the lines below, more indented.
                items.push(match self.peek()? {
                    Some(next) if next.indent > indent => self.block(next, Some(indent))?,
                    _ => Value::Null,
                });
            } else if rest.starts_with(['|', '>']) {
                items.push(self.block_scalar(rest, line, indent)?);
            } else if entry(rest).is_some()
                || key(rest).map_err(|what| line.error(&what))?.is_some()
            {
                // `- - a` or `- name: x`: a list or a mapping that starts
                // after the `- `, at its column.
                let column = line.indent + (line.text.len() - rest.len());
                let inner = Line { index: line.index, indent: column, text: rest };
                self.inline = Some(inner);
                items.push(self.block(inner, Some(indent))?);
            } else {
                items.push(self.scalar(rest, line, Some(indent))?);
            }
        }
        Ok(Value::List(items))
    }

    /// Reads a mapping whose keys are at column `indent`.
    fn mapping(&mut self, indent: usize) -> Result<Value, Error> {
        let mut entries: Vec<(String, Value)> = Vec::new();
        while let Some(line) = self.peek()? {
            if line.indent != indent {
                break;
            }
            let Some((name, rest)) = key(line.text).map_err(|what| line.error(&what))? else {
                if entry(line.text).is_some() {
                    return Err(line.error("a list entry where the mapping's next key was due"));
                }
                return Err(line.error("a line with no key where the mapping's next key was due"));
            };
            if entries.iter().any(|(seen, _)| *seen == name) {
                return Err(line.error(&format!("the key {name} is in this mapping twice")));
            }
            self.take(line);
            let value = self.value_after(rest, line, indent)?;
            entries.push((name, value));
        }
        Ok(Value::Map(entries))
    }

    /// Reads the value of a key at column `indent`, of which `rest` is what
    /// follows its colon on `line`.
    fn value_after(
        &mut self,
        rest: &'a str,
        line: Line<'a>,
        indent: usize,
    ) -> Result<Value, Error> {
        let rest = strip_tag(rest);
        if rest.is_empty() || rest.starts_with('#') {
            return match self.peek()? {
                Some(next) if next.indent > indent => self.block(next, Some(indent)),
                // A list may be as indented as its key.
                Some(next) if next.indent == indent && entry(next.text).is_some() => {
                    self.list(indent)
                }
                _ => Ok(Value::Null),
            };
        }
        if rest.starts_with(['|', '>']) {
            return self.block_scalar(rest, line, indent);
        }
        self.scalar(rest, line, Some(indent))
    }

    /// Reads a scalar or a flow collection that starts with `text`, on
    /// `line`, already taken: it may go on over the lines after it that
    /// are more indented than `parent`, any lines if `None`.
    fn scalar(
        &mut self,
        text: &'a str,
        line: Line<'a>,
        parent: Option<usize>,
    ) -> Result<Value, Error> {
        let text = strip_tag(text);
        if text.starts_with(['&', '*']) {
            return Err(line.error("anchors and aliases are not read"));
        }
        if text.starts_with(['[', '{']) {
            let mut whole = strip_comment(text).to_string();
            while !balanced(&whole) {
                let Some(next) = self.next_raw() else {
                    return Err(line.error("a [ or { with no ] or } to close it"));
                };
                whole.push('\n');
                whole.push_str(strip_comment(next.trim()));
            }
            return flow(&whole).map_err(|what| line.error(&what));
        }
        let mut whole = text.to_string();
        let quoted = text.starts_with(['"', '\'']);
        if quoted {
            // A quoted scalar goes on until its quote closes.
            while !quote_closed(&whole) {
                let Some(next) = self.next_raw() else {
                    return Err(line.error("a quote with no quote to close it"));
                };
                whole.push(' ');
                whole.push_str(next.trim());
            }
            let (value, after) = quoted_text(&whole).map_err(|what| line.error(&what))?;
            if !strip_comment(after).trim().is_empty() {
                return Err(line.error("more after the closing quote"));
            }
            return Ok(Value::Text(value));
        }
        let mut whole = strip_comment(&whole).trim().to_string();
        // A plain scalar's later lines, more indented, join it with a
        // space between.
        while let Some(next) = self.peek()?
            && parent.is_none_or(|parent| next.indent > parent)
            && self.inline.is_none()
        {
            self.take(next);
            whole.push(' ');
            whole.push_str(strip_comment(next.text).trim());
        }
        Ok(plain(whole))
    }

    /// Takes the next line as it is, blank or not, for a scalar that goes
    /// on over it.
    fn next_raw(&mut self) -> Option<&'a str> {
        self.inline = None;
        let line = self.lines.get(self.at).copied()?;
        self.at += 1;
        Some(line)
    }

    /// Reads a block scalar whose header, `|` or `>` with its chomping and
    /// indentation indicators, is `header`, on `line`; its lines are those
    /// after, more indented than `parent`.
    fn block_scalar(
        &mut self,
        header: &str,
        line: Line<'_>,
        parent: usize,
    ) -> Result<Value, Error> {
        let header = strip_comment(header).trim();
        let folded = header.starts_with('>');
        let (mut chomp, mut explicit) = ('c', None);
        for c in header.chars().skip(1) {
            match c {
                '-' | '+' => chomp = c,
                '1'..='9' => explicit = c.to_digit(10).map(|n| n as usize),
                _ => return Err(line.error(&format!("{header} is not a block scalar's header"))),
            }
        }
        self.take(line);
        // The block's own indentation: as its header says, or as its
        // first line that holds something is indented.
        let first = self.lines[self.at..].iter().find(|l| !l.trim().is_empty());
        let indent = match (explicit, first) {
            (Some(n), _) => parent + n,
            (None, Some(first)) => first.len() - first.trim_start_matches(' ').len(),
            (None, None) => parent + 1,
        };
        let mut kept: Vec<&str> = Vec::new();
        if indent > parent {
            while let Some(&raw) = self.lines.get(self.at) {
                let raw = raw.trim_end_matches('\r');
                if raw.trim().is_empty() {
                    kept.push("");
                } else if raw.len() - raw.trim_start_matches(' ').len() >= indent {
                    kept.push(&raw[indent..]);
                } else {
                    break;
                }
                self.at += 1;
            }
        }
        // Blank lines at the end are the chomping's, not the text's.
        let trailing = kept.iter().rev().take_while(|l| l.is_empty()).count();
        let body = &kept[..kept.len() - trailing];
        let mut text = if folded { fold(body) } else { body.join("\n") };
        if !body.is_empty() {
            match chomp {
                '-' => {}
                '+' => text.push_str(&"\n".repeat(trailing + 1)),
                _ => text.push('\n'),
            }
        }
        Ok(Value::Text(text))
    }
}

/// What follows a list entry's `-` in `text`, if it starts one.
fn entry(text: &str) -> Option<&str> {
    match text.strip_prefix('-')? {
        "" => Some(""),
        rest if rest.starts_with(' ') => Some(rest.trim_start_matches(' ')),
        _ => None,
    }
}

/// The key `text` starts with, and what follows its colon, if it starts
/// with one: a plain or quoted key, then a colon at the end or before a
/// space.
fn key(text: &str) -> Result<Option<(String, &str)>, String> {
    if text.starts_with("? ") || text == "?" {
        return Err("complex keys (? ) are not read".into());
    }
    if text.starts_with(['[', '{', '|', '>', '#', '&', '*', '!']) || entry(text).is_some() {
        return Ok(None);
    }
    if text.starts_with(['"', '\'']) {
        let Ok((name, after)) = quoted_text(text) else { return Ok(None) };
        return Ok(colon(after).map(|rest| (name, rest)));
    }
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let next = chars.peek().map(|&(_, c)| c);
        if c == '#' && at > 0 && text[..at].ends_with(' ') {
            return Ok(None);
        }
        if c == ':' && next.is_none_or(|c| c == ' ') {
            let name = text[..at].trim_end();
            return Ok((!name.is_empty()).then(|| (name.to_string(), text[at + 1..].trim_start())));
        }
    }
    Ok(None)
}

/// What follows a colon at the start of `text`, if it is a key's.
fn colon(text: &str) -> Option<&str> {
    let rest = text.trim_start_matches(' ').strip_prefix(':')?;
    (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim_start())
}

/// `text` without a tag such as `!!str ` in front of it.
fn strip_tag(text: &str) -> &str {
    match text.strip_prefix('!') {
        Some(_) => text.split_once(' ').map_or("", |(_, rest)| rest.trim_start()),
        None => text,
    }
}

/// `text` without a comment at its end: from a `#` after a space, or at
/// its start, outside quotes.
fn strip_comment(text: &str) -> &str {
    let mut quote = None;
    let mut previous = ' ';
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match (quote, c) {
            (None, '#') if previous == ' ' => return text[..at].trim_end(),
            (None, '"' | '\'') if matches!(previous, ' ' | '[' | '{' | ',' | ':') || at == 0 => {
                quote = Some(c);
            }
            (Some('"'), '\\') => {
                chars.next();
            }
            (Some('\''), '\'') if chars.peek().is_some_and(|&(_, c)| c == '\'') => {
                chars.next();
            }
            (Some(q), c) if q == c => quote = None,
            _ => {}
        }
        previous = c;
    }
    text
}

/// Whether the quoted scalar `text` starts with has closed.
fn quote_closed(text: &str) -> bool {
    quoted_text(text).is_ok()
}

/// Whether every `[` and `{` of `text`, outside quotes, is closed.
fn balanced(text: &str) -> bool {
    let mut depth = 0i32;
    let mut quote = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            (Some('"'), '\\') => {
                chars.next();
            }
            (Some('\''), '\'') if chars.peek() == Some(&'\'') => {
                chars.next();
            }
            (Some(q), c) if q == c => quote = None,
            _ => {}
        }
    }
    depth <= 0 && quote.is_none()
}

/// The scalar a quoted `text` starts with, its escapes read, and what
/// follows its closing quote; or why it cannot be read.
fn quoted_text(text: &str) -> Result<(String, &str), String> {
    let mut chars = text.char_indices();
    let Some((_, quote)) = chars.next() else { return Err("nothing to read".into()) };
    let mut value = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            c if c == quote => {
                if quote == '\'' && text[at + 1..].starts_with('\'') {
                    // '' in single quotes is one '.
                    chars.next();
                    value.push('\'');
                    continue;
                }
                return Ok((value, &text[at + 1..]));
            }
            '\\' if quote == '"' => {
                let Some((_, escaped)) = chars.next() else { break };
                match escaped {
                    'n' => value.push('\n'),
                    't' | '\t' => value.push('\t'),
                    'r' => value.push('\r'),
                    '0' => value.push('\0'),
                    'a' => value.push('\x07'),
                    'b' => value.push('\x08'),
                    'e' => value.push('\x1b'),
                    'f' => value.push('\x0c'),
                    'v' => value.push('\x0b'),
                    ' ' | '"' | '/' | '\\' => value.push(escaped),
                    'N' => value.push('\u{85}'),
                    '_' => value.push('\u{a0}'),
                    'L' => value.push('\u{2028}'),
                    'P' => value.push('\u{2029}'),
                    'x' | 'u' | 'U' => {
                        let digits = match escaped {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let hex: String = chars.by_ref().take(digits).map(|(_, c)| c).collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .ok()
                            .filter(|_| hex.len() == digits)
                            .and_then(char::from_u32)
                            .ok_or_else(|| format!("\\{escaped}{hex} is not a character"))?;
                        value.push(code);
                    }
                    other => return Err(format!("\\{other} is not an escape YAML knows")),
                }
            }
            c => value.push(c),
        }
    }
    Err("a quote with no quote to close it".into())
}

/// A plain scalar's value: null for nothing, `~` and `null`.
fn plain(text: String) -> Value {
    match text.as_str() {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        _ => Value::Text(text),
    }
}

/// The lines of a folded block scalar, joined: two lines of text with a
/// space between, blank lines between them as as many line breaks, and
/// lines more indented than the block kept on lines of their own.
fn fold(lines: &[&str]) -> String {
    let mut text = String::new();
    let (mut blanks, mut started, mut indented) = (0, false, false);
    for &line in lines {
        if line.is_empty() {
            blanks += 1;
            continue;
        }
        let more = line.starts_with(' ');
        text.push_str(&"\n".repeat(blanks));
        if started && (more || indented) {
            text.push('\n');
        } else if started && blanks == 0 {
            text.push(' ');
        }
        text.push_str(line);
        (blanks, started, indented) = (0, true, more);
    }
    text
}

/// Reads a flow list or mapping, `text` whole.
fn flow(text: &str) -> Result<Value, String> {
    let mut flow = Flow { chars: text.chars().collect(), at: 0 };
    let value = flow.value(false)?;
    flow.space();
    if flow.at < flow.chars.len() {
        return Err(format!("more after the flow collection: {}", flow.rest()));
    }
    Ok(value)
}

/// A flow collection, read a character at a time.
struct Flow {
    chars: Vec<char>,
    at: usize,
}

impl Flow {
    /// Passes over spaces and line breaks.
    fn space(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }

    /// The rest of the text, for an error.
    fn rest(&self) -> String {
        self.chars[self.at..].iter().collect()
    }

    /// Reads a value: a list, a mapping, a quoted or a plain scalar; a
    /// plain one in a mapping's key (`key`) ends before a colon too.
    fn value(&mut self, key: bool) -> Result<Value, String> {
        self.space();
        match self.chars.get(self.at) {
            Some('[') => self.list(),
            Some('{') => self.mapping(),
            Some('"' | '\'') => {
                let rest = self.rest();
                let (value, after) = quoted_text(&rest)?;
                self.at += rest.chars().count() - after.chars().count();
                Ok(Value::Text(value))
            }
            Some(_) => {
                let start = self.at;
                while let Some(&c) = self.chars.get(self.at) {
                    let colon = c == ':'
                        && self
                            .chars
                            .get(self.at + 1)
                            .is_none_or(|c| c.is_whitespace() || ",]}".contains(*c));
                    if ",]}".contains(c) || (key && colon) {
                        break;
                    }
                    self.at += 1;
                }
                let text: String = self.chars[start..self.at].iter().collect();
                Ok(plain(text.trim().to_string()))
            }
            None => Err("a flow collection ends early".into()),
        }
    }

    /// Reads a list, from its `[`.
    fn list(&mut self) -> Result<Value, String> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.space();
            match self.chars.get(self.at) {
                Some(']') => {
                    self.at += 1;
                    return Ok(Value::List(items));
                }
                None => return Err("a [ with no ] to close it".into()),
                _ => {}
            }
            items.push(self.value(false)?);
            self.space();
            match self.chars.get(self.at) {
                Some(',') => self.at += 1,
                Some(']') => {}
                _ => return Err(format!("a , or ] was due before {}", self.rest())),
            }
        }
    }

    /// Reads a mapping, from its `{`.
    fn mapping(&mut self) -> Result<Value, String> {
        self.at += 1;
        let mut entries: Vec<(String, Value)> = Vec::new();
        loop {
            self.space();
            match self.chars.get(self.at) {
                Some('}') => {
                    self.at += 1;
                    return Ok(Value::Map(entries));
                }
                None => return Err("a { with no } to close it".into()),
                _ => {}
            }
            let name = match self.value(true)? {
                Value::Text(name) => name,
                Value::Null => String::new(),
                _ => return Err("a key that is not a scalar".into()),
            };
            self.space();
            let value = if self.chars.get(self.at) == Some(&':') {
                self.at += 1;
                let next = self.chars[self.at..].iter().find(|c| !c.is_whitespace()).copied();
                match next {
                    Some(',' | '}') => {
                        self.space();
                        Value::Null
                    }
                    _ => self.value(false)?,
                }
            } else {
                Value::Null
            };
            if entries.iter().any(|(seen, _)| *seen == name) {
                return Err(format!("the key {name} is in this mapping twice"));
            }
            entries.push((name, value));
            self.space();
            match self.chars.get(self.at) {
                Some(',') => self.at += 1,
                Some('}') => {}
                _ => return Err(format!("a , or }} was due before {}", self.rest())),
            }
        }
    }
}
