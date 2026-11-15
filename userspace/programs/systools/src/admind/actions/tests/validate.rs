// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A validator for the part of JSON Schema that [`schema`] writes (ADR
//! 0515), and the regular expressions its patterns are, for the tests that
//! keep each action's fields true to its types.
//!
//! It is the oracle of those tests, so it is plain and small, and refuses
//! a keyword it does not know: a schema that gains one is not quietly
//! accepted. A pattern is of ECMAScript's kind as far as the actions use
//! it: characters, `.`, classes with ranges, groups, alternation, `*`, `+`
//! and `?`, `^` and `$`, and lookahead, `(?=` and `(?!`.
//!
//! [`schema`]: super::super::schema

use serde_json::Value;

/// A regular expression, parsed.
#[derive(Debug)]
enum Node {
    Char(char),
    /// Any character but a line end.
    Any,
    Class {
        ranges: Vec<(char, char)>,
        negated: bool,
    },
    /// The start of the text.
    Start,
    /// The end of the text.
    End,
    /// Alternatives, each a sequence.
    Group(Vec<Vec<Node>>),
    /// A lookahead: what follows matches an alternative, or, when
    /// `negative`, none.
    Look {
        alternatives: Vec<Vec<Node>>,
        negative: bool,
    },
    Repeat {
        node: Box<Node>,
        min: usize,
        max: Option<usize>,
    },
}

/// A pattern being read.
struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.at += 1;
        Some(next)
    }

    /// Takes `text` if the pattern goes on with it.
    fn eat(&mut self, text: &str) -> bool {
        let wanted: Vec<char> = text.chars().collect();
        if self.chars[self.at..].starts_with(&wanted) {
            self.at += wanted.len();
            return true;
        }
        false
    }

    fn alternatives(&mut self) -> Result<Vec<Vec<Node>>, String> {
        let mut all = vec![self.sequence()?];
        while self.eat("|") {
            all.push(self.sequence()?);
        }
        Ok(all)
    }

    fn sequence(&mut self) -> Result<Vec<Node>, String> {
        let mut nodes = Vec::new();
        while let Some(next) = self.peek() {
            if next == '|' || next == ')' {
                break;
            }
            let atom = self.atom()?;
            nodes.push(self.repeated(atom));
        }
        Ok(nodes)
    }

    fn atom(&mut self) -> Result<Node, String> {
        let next = self.next().ok_or("the pattern ends early")?;
        Ok(match next {
            '^' => Node::Start,
            '$' => Node::End,
            '.' => Node::Any,
            '(' => {
                let look = if self.eat("?!") {
                    Some(true)
                } else if self.eat("?=") {
                    Some(false)
                } else {
                    None
                };
                let alternatives = self.alternatives()?;
                if !self.eat(")") {
                    return Err("a group is not closed".into());
                }
                match look {
                    Some(negative) => Node::Look { alternatives, negative },
                    None => Node::Group(alternatives),
                }
            }
            '[' => self.class()?,
            '\\' => Node::Char(self.next().ok_or("the pattern ends after a backslash")?),
            '*' | '+' | '?' => return Err(format!("{next} has nothing to repeat")),
            other => Node::Char(other),
        })
    }

    fn repeated(&mut self, node: Node) -> Node {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            _ => return node,
        };
        self.at += 1;
        Node::Repeat { node: Box::new(node), min, max }
    }

    /// A class, after its `[`. A `-` that ends it is itself.
    fn class(&mut self) -> Result<Node, String> {
        let negated = self.eat("^");
        let mut ranges = Vec::new();
        loop {
            let mut first = self.next().ok_or("a class is not closed")?;
            if first == ']' {
                break;
            }
            if first == '\\' {
                first = self.next().ok_or("a class ends after a backslash")?;
            }
            let range_follows =
                self.peek() == Some('-') && self.chars.get(self.at + 1) != Some(&']');
            if range_follows {
                self.at += 1;
                let mut last = self.next().ok_or("a range has no end")?;
                if last == '\\' {
                    last = self.next().ok_or("a class ends after a backslash")?;
                }
                ranges.push((first, last));
            } else {
                ranges.push((first, first));
            }
        }
        Ok(Node::Class { ranges, negated })
    }
}

/// Whether `nodes` match `text` from `at`, and `then` takes where they end.
fn matches_at(
    nodes: &[Node],
    text: &[char],
    at: usize,
    then: &mut dyn FnMut(usize) -> bool,
) -> bool {
    let Some((first, rest)) = nodes.split_first() else { return then(at) };
    let here = text.get(at).copied();
    match first {
        Node::Char(wanted) => here == Some(*wanted) && matches_at(rest, text, at + 1, then),
        Node::Any => {
            here.is_some_and(|c| c != '\n' && c != '\r') && matches_at(rest, text, at + 1, then)
        }
        Node::Class { ranges, negated } => {
            let inside = |c: char| ranges.iter().any(|&(low, high)| (low..=high).contains(&c));
            here.is_some_and(|c| inside(c) != *negated) && matches_at(rest, text, at + 1, then)
        }
        Node::Start => at == 0 && matches_at(rest, text, at, then),
        Node::End => at == text.len() && matches_at(rest, text, at, then),
        Node::Group(alternatives) => alternatives.iter().any(|alternative| {
            matches_at(alternative, text, at, &mut |end| matches_at(rest, text, end, then))
        }),
        Node::Look { alternatives, negative } => {
            let found = alternatives
                .iter()
                .any(|alternative| matches_at(alternative, text, at, &mut |_| true));
            found != *negative && matches_at(rest, text, at, then)
        }
        Node::Repeat { node, min, max } => repeated(node, (*min, *max), 0, rest, text, at, then),
    }
}

/// Matches `node` as many times as it can, from `count` of them, up to
/// `limits`, and then `rest`, going back a time at a time.
fn repeated(
    node: &Node,
    limits: (usize, Option<usize>),
    count: usize,
    rest: &[Node],
    text: &[char],
    at: usize,
    then: &mut dyn FnMut(usize) -> bool,
) -> bool {
    let (min, max) = limits;
    if max.is_none_or(|max| count < max) {
        let more = matches_at(std::slice::from_ref(node), text, at, &mut |end| {
            // A match that is empty would go on for ever.
            end != at && repeated(node, limits, count + 1, rest, text, end, then)
        });
        if more {
            return true;
        }
    }
    count >= min && matches_at(rest, text, at, then)
}

/// Whether `pattern` matches anywhere in `text`, as a JSON Schema pattern
/// does.
pub fn matches(pattern: &str, text: &str) -> Result<bool, String> {
    let mut parser = Parser { chars: pattern.chars().collect(), at: 0 };
    let alternatives = parser.alternatives()?;
    if parser.at != parser.chars.len() {
        return Err(format!("{pattern}: a ) with no ( before it"));
    }
    let whole = [Node::Group(alternatives)];
    let text: Vec<char> = text.chars().collect();
    Ok((0..=text.len()).any(|start| matches_at(&whole, &text, start, &mut |_| true)))
}

/// What `schema` may say of a value of `kind`.
fn keywords(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "object" => &["type", "properties", "required", "additionalProperties"],
        "string" => &["type", "description", "minLength", "maxLength", "pattern", "enum"],
        "integer" => &["type", "description", "minimum", "maximum"],
        "boolean" => &["type", "description"],
        _ => return None,
    })
}

/// Whether `instance` is what `schema` describes.
///
/// # Errors
///
/// What is wrong, naming where: `/name` for the property `name`.
pub fn validate(schema: &Value, instance: &Value) -> Result<(), String> {
    check(schema, instance, "")
}

fn check(schema: &Value, instance: &Value, at: &str) -> Result<(), String> {
    let at_name = if at.is_empty() { "the arguments" } else { at };
    let schema = schema.as_object().ok_or_else(|| format!("{at_name}: a schema is an object"))?;
    let kind = schema.get("type").and_then(Value::as_str).unwrap_or_default();
    let allowed = keywords(kind).ok_or_else(|| format!("{at_name}: no type {kind:?} here"))?;
    if let Some(unknown) = schema.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{at_name}: the validator does not know {unknown}"));
    }
    let wrong = |what: &str| Err(format!("{at_name}: {what}, in {instance}"));
    match kind {
        "object" => {
            let Some(given) = instance.as_object() else { return wrong("not an object") };
            let properties = schema["properties"].as_object().ok_or("properties is an object")?;
            for name in schema["required"].as_array().ok_or("required is a list")? {
                let name = name.as_str().ok_or("required names are text")?;
                if !given.contains_key(name) {
                    return Err(format!("{at_name}: {name} is required"));
                }
            }
            for (name, value) in given {
                match properties.get(name) {
                    Some(property) => check(property, value, &format!("{at}/{name}"))?,
                    None if schema["additionalProperties"] == false => {
                        return Err(format!("{at_name}: {name} is not one of the arguments"));
                    }
                    None => {}
                }
            }
            Ok(())
        }
        "string" => {
            let Some(text) = instance.as_str() else { return wrong("not text") };
            let length = text.chars().count() as u64;
            if schema.get("minLength").and_then(Value::as_u64).is_some_and(|min| length < min) {
                return wrong("shorter than minLength");
            }
            if schema.get("maxLength").and_then(Value::as_u64).is_some_and(|max| length > max) {
                return wrong("longer than maxLength");
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
                && !matches(pattern, text)?
            {
                return wrong(&format!("does not match {pattern}"));
            }
            match schema.get("enum").and_then(Value::as_array) {
                Some(words) if !words.contains(instance) => wrong("not one of the words"),
                _ => Ok(()),
            }
        }
        "integer" => {
            let Some(number) = instance.as_i64() else { return wrong("not a whole number") };
            if schema.get("minimum").and_then(Value::as_i64).is_some_and(|min| number < min) {
                return wrong("below minimum");
            }
            if schema.get("maximum").and_then(Value::as_i64).is_some_and(|max| number > max) {
                return wrong("above maximum");
            }
            Ok(())
        }
        _ => {
            if instance.is_boolean() {
                Ok(())
            } else {
                wrong("not true or false")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn is(pattern: &str, text: &str) -> bool {
        matches(pattern, text).unwrap()
    }

    #[test]
    fn a_pattern_matches_anywhere_unless_it_is_anchored() {
        assert!(is("ab", "xabx"));
        assert!(!is("^ab", "xab"));
        assert!(!is("ab$", "abx"));
        assert!(is("^ab$", "ab"));
        assert!(!is("^ab$", "ab\n"));
        assert!(is("", ""));
        assert!(is("^$", ""));
        assert!(!is("^$", "a"));
    }

    #[test]
    fn classes_have_ranges_negation_escapes_and_a_hyphen_that_ends_them() {
        assert!(is("^[a-c]+$", "abcabc"));
        assert!(!is("^[a-c]+$", "abd"));
        assert!(is("^[^-]", "x"));
        assert!(!is("^[^-]", "-x"));
        assert!(is("^[A-Za-z0-9_.-]*$", "a-b.c_9-"));
        assert!(!is("^[A-Za-z0-9_.-]*$", "a b"));
        assert!(is(r"^[\]a]+$", "]a"));
        assert!(is(r"^a\.b$", "a.b"));
        assert!(!is(r"^a\.b$", "axb"));
        assert!(is("^a.c$", "abc"));
        assert!(!is("^a.c$", "a\nc"));
    }

    #[test]
    fn groups_alternatives_and_repeats_go_back_to_find_a_match() {
        assert!(is("^(ab|a)b$", "ab"));
        assert!(is("^(ab|a)b$", "abb"));
        assert!(is("^a*ab$", "aaab"));
        assert!(is("^(a|b)+c?$", "abbac"));
        assert!(!is("^(a|b)+c?$", "abbacc"));
        assert!(is("^a?a?a$", "aa"));
        assert!(is("^(a*)*b$", "aab"));
        assert!(!is("^(a*)*b$", "aac"));
    }

    #[test]
    fn a_lookahead_looks_without_taking() {
        let name = "^(?!(check|new)$)[a-z]+$";
        assert!(is(name, "nginx"));
        assert!(is(name, "checks"));
        assert!(!is(name, "check"));
        assert!(!is(name, "new"));
        assert!(is("^(?=ab)a", "ab"));
        assert!(!is("^(?=ab)a", "ac"));
        assert!(is("^(?!.*\\.\\.)[a-z.]+$", "a.b"));
        assert!(!is("^(?!.*\\.\\.)[a-z.]+$", "a..b"));
    }

    #[test]
    fn a_pattern_that_does_not_read_is_an_error() {
        for pattern in ["(a", "a)", "[a", "*a", "a\\", "(?!a"] {
            assert!(matches(pattern, "a").is_err(), "{pattern}");
        }
    }

    #[test]
    fn a_string_is_held_to_its_length_in_characters_pattern_and_words() {
        let text =
            json!({ "type": "string", "minLength": 1, "maxLength": 3, "pattern": "^[a-z]*$" });
        assert!(validate(&text, &json!("abc")).is_ok());
        assert!(validate(&text, &json!("")).unwrap_err().contains("minLength"));
        assert!(validate(&text, &json!("abcd")).unwrap_err().contains("maxLength"));
        assert!(validate(&text, &json!("AB")).unwrap_err().contains("does not match"));
        // Three characters of two bytes each.
        let wide = json!({ "type": "string", "maxLength": 3 });
        assert!(validate(&wide, &json!("\u{e9}\u{e9}\u{e9}")).is_ok());
        assert!(validate(&text, &json!(7)).unwrap_err().contains("not text"));
        let words = json!({ "type": "string", "enum": ["a", "b"] });
        assert!(validate(&words, &json!("b")).is_ok());
        assert!(validate(&words, &json!("c")).is_err());
        assert!(validate(&words, &json!(null)).is_err());
    }

    #[test]
    fn a_number_is_a_whole_number_within_its_limits_and_a_flag_is_true_or_false() {
        let number = json!({ "type": "integer", "minimum": 1, "maximum": 5 });
        for good in [1, 3, 5] {
            assert!(validate(&number, &json!(good)).is_ok());
        }
        assert!(validate(&number, &json!(0)).unwrap_err().contains("minimum"));
        assert!(validate(&number, &json!(6)).unwrap_err().contains("maximum"));
        assert!(validate(&number, &json!(1.5)).is_err());
        assert!(validate(&number, &json!("3")).is_err());
        let flag = json!({ "type": "boolean" });
        assert!(validate(&flag, &json!(false)).is_ok());
        assert!(validate(&flag, &json!(0)).is_err());
        assert!(validate(&flag, &json!(null)).is_err());
    }

    #[test]
    fn an_object_has_what_is_required_and_nothing_that_is_not_a_property() {
        let schema = json!({
            "type": "object",
            "properties": { "a": { "type": "boolean" }, "b": { "type": "boolean" } },
            "required": ["a"],
            "additionalProperties": false,
        });
        assert!(validate(&schema, &json!({ "a": true })).is_ok());
        assert!(validate(&schema, &json!({ "a": true, "b": false })).is_ok());
        assert!(validate(&schema, &json!({ "b": true })).unwrap_err().contains("a is required"));
        assert!(validate(&schema, &json!({ "a": true, "c": 1 })).unwrap_err().contains("c is not"));
        assert!(validate(&schema, &json!({ "a": 1 })).unwrap_err().starts_with("/a:"));
        assert!(validate(&schema, &json!([])).is_err());
    }

    #[test]
    fn a_keyword_or_type_the_validator_does_not_know_is_refused() {
        let unknown = json!({ "type": "string", "format": "uri" });
        assert!(validate(&unknown, &json!("x")).unwrap_err().contains("does not know format"));
        assert!(validate(&json!({ "type": "number" }), &json!(1)).is_err());
        assert!(validate(&json!({}), &json!(1)).is_err());
        assert!(validate(&json!(true), &json!(1)).is_err());
    }
}
