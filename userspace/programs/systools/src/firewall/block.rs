// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A block of rules that a command keeps in the firewall's settings
//! (ADR 0570): the lines between a start line and an end line that the
//! command names, which it writes whole each time and takes away when it
//! needs none. database(8) keeps PostgreSQL's port so, open to the networks
//! a database is opened to and shut to the rest.
//!
//! Every line outside the block is kept byte for byte, comments and rules
//! written by hand among them: the rest of the file is the administrator's.
//! A block is found by its two lines alone, each matched whole once its
//! spaces at either end are taken off. A file with a start and no end, an
//! end and no start, an end before its start, or two starts, is not
//! guessed at: it is refused, and an administrator mends it by hand.
//!
//! The block goes at the end of the file when it is new, after the
//! administrator's rules, which so decide first; where it is when it is
//! there, so that an administrator may move it. [`decides_first`] says
//! which rule before the block decides for the block's port before the
//! block does.

use super::{Action, Protocol, RULES_MAX, Rule, parse_settings, rules, words_of};

/// The two lines that hold a command's block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Marks<'a> {
    /// The first line of the block, a comment.
    pub start: &'a str,
    /// The last line of the block, a comment.
    pub end: &'a str,
}

/// Where a block is in a file's lines: the indexes of its start and end
/// lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    start: usize,
    end: usize,
}

/// The file's lines, each without its line end, and whether the file
/// ended with one.
fn lines_of(text: &str) -> (Vec<&str>, bool) {
    if text.is_empty() {
        return (Vec::new(), true);
    }
    let ended = text.ends_with('\n');
    let body = if ended { &text[..text.len() - 1] } else { text };
    (body.split('\n').collect(), ended)
}

/// Whether `line` is the mark `mark`: the same once the spaces and tabs at
/// either end, and a carriage return, are taken off.
fn is_mark(line: &str, mark: &str) -> bool {
    line.trim_matches([' ', '\t', '\r']) == mark
}

/// Where the block of `marks` is among `lines`; none when the file has
/// none.
///
/// # Errors
///
/// When the marks do not make one block: two starts or two ends, a start
/// with no end, an end with no start, or an end before the start.
fn find(lines: &[&str], marks: Marks<'_>) -> Result<Option<Place>, String> {
    let starts: Vec<usize> =
        (0..lines.len()).filter(|&at| is_mark(lines[at], marks.start)).collect();
    let ends: Vec<usize> = (0..lines.len()).filter(|&at| is_mark(lines[at], marks.end)).collect();
    let mend = "; mend the file by hand";
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([start], [end]) if start < end => Ok(Some(Place { start: *start, end: *end })),
        ([start], [end]) => Err(format!(
            "line {} ends a block that line {} starts after it{mend}",
            end + 1,
            start + 1
        )),
        ([start], []) => Err(format!("line {} starts a block that no line ends{mend}", start + 1)),
        ([], [end]) => Err(format!("line {} ends a block that no line starts{mend}", end + 1)),
        _ => Err(format!(
            "the file has {} lines that start the block and {} that end it, where one of each is \
             wanted{mend}",
            starts.len(),
            ends.len()
        )),
    }
}

/// The text of `lines`, ended as `ended` says, each line with a line end
/// after it but the last of a file that had none.
fn text_of(lines: &[String], ended: bool) -> String {
    let mut text = lines.join("\n");
    if ended && !lines.is_empty() {
        text.push('\n');
    }
    text
}

/// The rules the block of `marks` holds in `text`, in order; none when the
/// file has no block. A line in it that is not a rule or a comment is left
/// out, as `firewall load` leaves it out.
///
/// # Errors
///
/// As [`find`].
pub(crate) fn rules_of(text: &str, marks: Marks<'_>) -> Result<Option<Vec<Rule>>, String> {
    let (lines, _) = lines_of(text);
    let Some(place) = find(&lines, marks)? else { return Ok(None) };
    let inside = lines[place.start + 1..place.end].join("\n");
    Ok(Some(rules(&parse_settings(&inside).0).cloned().collect()))
}

/// `text` with the block of `marks` holding `wanted`, in that order, and
/// nothing else: written where the block is, or at the end of the file when
/// it has none. With no rule wanted, the block is taken away, as
/// [`without`] does. Every other line is kept as it is.
///
/// # Errors
///
/// As [`find`]; and when the file would hold more rules than the firewall
/// holds.
pub(crate) fn with(text: &str, marks: Marks<'_>, wanted: &[Rule]) -> Result<String, String> {
    if wanted.is_empty() {
        return without(text, marks);
    }
    let (lines, ended) = lines_of(text);
    let place = find(&lines, marks)?;
    let block = std::iter::once(marks.start.to_string())
        .chain(wanted.iter().map(ToString::to_string))
        .chain(std::iter::once(marks.end.to_string()));
    let kept = |range: std::ops::Range<usize>| lines[range].iter().map(|line| (*line).to_string());
    let made: Vec<String> = match place {
        Some(place) => {
            kept(0..place.start).chain(block).chain(kept(place.end + 1..lines.len())).collect()
        }
        None => {
            // A file whose last line had no line end gets one, and a blank
            // line goes before a block that follows other lines.
            let blank = lines.last().is_some_and(|last| !last.trim().is_empty());
            kept(0..lines.len()).chain(blank.then(String::new)).chain(block).collect()
        }
    };
    let ended = ended || place.is_none();
    let made = text_of(&made, ended);
    let count = rules(&parse_settings(&made).0).count();
    if count > RULES_MAX {
        return Err(format!(
            "the file would hold {count} rules, and the firewall holds {RULES_MAX} at most"
        ));
    }
    Ok(made)
}

/// `text` without the block of `marks`, its two lines and what is between
/// them; the same text when it has none.
///
/// # Errors
///
/// As [`find`].
pub(crate) fn without(text: &str, marks: Marks<'_>) -> Result<String, String> {
    let (lines, ended) = lines_of(text);
    let Some(place) = find(&lines, marks)? else { return Ok(text.to_string()) };
    let mut made: Vec<String> =
        lines[..place.start].iter().map(|line| (*line).to_string()).collect();
    let after = &lines[place.end + 1..];
    // The blank line that [`with`] put before a block at the end goes with it.
    if after.is_empty() && made.last().is_some_and(|last| last.trim().is_empty()) {
        made.pop();
    }
    made.extend(after.iter().map(|line| (*line).to_string()));
    Ok(text_of(&made, ended))
}

/// Whether `rule` may decide for a TCP connection to `port` that some
/// other machine starts: it is for TCP or for any protocol, and holds the
/// port or names none. A rule that also names a network, an interface or
/// a program decides for some of them.
fn may_decide(rule: &Rule, port: u16) -> bool {
    matches!(rule.protocol, Protocol::Tcp | Protocol::Any)
        && rule.ports.is_none_or(|(first, last)| (first..=last).contains(&port))
}

/// The first rule before the block of `marks` in `text` that decides for
/// some TCP connections to `port` before the block's own rules do, as the
/// file writes it: an administrator's own rule, which the block cannot
/// overrule. None when no rule comes first, or the file has no block.
///
/// # Errors
///
/// As [`find`].
pub(crate) fn decides_first(
    text: &str,
    marks: Marks<'_>,
    port: u16,
) -> Result<Option<String>, String> {
    let (lines, _) = lines_of(text);
    let Some(place) = find(&lines, marks)? else { return Ok(None) };
    for line in &lines[..place.start] {
        let words = words_of(line);
        if words.is_empty() {
            continue;
        }
        if let Ok(rule) = Rule::parse(&words)
            && may_decide(&rule, port)
        {
            return Ok(Some(rule.to_string()));
        }
    }
    Ok(None)
}

/// The rules that open TCP `port` to each of `networks`, in order, and
/// shut it to every other source after them: what a block holds for one
/// port. None for no network: a port shut to all needs no block, since a
/// socket the port is not opened to has no inbound right without one.
pub(crate) fn port_for(port: u16, networks: &[(std::net::IpAddr, u8)]) -> Vec<Rule> {
    if networks.is_empty() {
        return Vec::new();
    }
    let rule = |action, from| Rule {
        action,
        protocol: Protocol::Tcp,
        ports: Some((port, port)),
        from,
        on: None,
        program: None,
    };
    let mut made: Vec<Rule> = Vec::new();
    for network in networks {
        let allow = rule(Action::Allow, Some(*network));
        if !made.contains(&allow) {
            made.push(allow);
        }
    }
    made.push(rule(Action::Deny, None));
    made
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    const MARKS: Marks<'static> = Marks {
        start: "# database(8): who reaches PostgreSQL from other machines (ADR 0570)",
        end: "# database(8): end",
    };

    fn net(text: &str, prefix: u8) -> (IpAddr, u8) {
        (text.parse().unwrap(), prefix)
    }

    /// A file of `count` rules, each opening a port of its own.
    fn ports_open(count: usize) -> String {
        let mut text = String::new();
        for at in 0..count {
            text.push_str(&format!("allow tcp {}\n", 1000 + at));
        }
        text
    }

    fn two() -> Vec<Rule> {
        port_for(5432, &[net("192.168.1.0", 24), net("10.0.0.0", 8)])
    }

    const BLOCK: &str = "# database(8): who reaches PostgreSQL from other machines (ADR 0570)
allow tcp 5432 from 192.168.1.0/24
allow tcp 5432 from 10.0.0.0/8
deny tcp 5432
# database(8): end
";

    #[test]
    fn a_port_is_opened_to_each_network_once_then_shut_to_the_rest() {
        let rules: Vec<String> = two().iter().map(ToString::to_string).collect();
        assert_eq!(
            rules,
            [
                "allow tcp 5432 from 192.168.1.0/24",
                "allow tcp 5432 from 10.0.0.0/8",
                "deny tcp 5432"
            ]
        );
        let again = port_for(5432, &[net("10.0.0.0", 8), net("10.0.0.0", 8)]);
        assert_eq!(again.len(), 2, "a network named twice is one rule");
        assert!(port_for(5432, &[]).is_empty(), "no network wants no block");
        let six = port_for(5432, &[net("fd00::", 8)]);
        assert_eq!(six[0].to_string(), "allow tcp 5432 from fd00::/8");
    }

    #[test]
    fn a_new_block_goes_at_the_end_after_a_blank_line_and_the_rest_is_kept_byte_for_byte() {
        let text = "# The administrator's rules.\nallow tcp 22   # ssh, by hand\nallow tcp 8080";
        let made = with(text, MARKS, &two()).unwrap();
        assert_eq!(
            made,
            format!(
                "# The administrator's rules.\nallow tcp 22   # ssh, by hand\nallow tcp 8080\n\n{BLOCK}"
            )
        );
        assert_eq!(with("", MARKS, &two()).unwrap(), BLOCK, "an empty file is the block alone");
        assert_eq!(
            with("allow tcp 22\n\n", MARKS, &two()).unwrap(),
            format!("allow tcp 22\n\n{BLOCK}")
        );
    }

    #[test]
    fn a_block_that_is_there_is_written_where_it_is_and_nothing_else_moves() {
        let text = format!("allow tcp 22\n{BLOCK}deny udp 5353 on net1\n");
        let one = port_for(5432, &[net("10.0.0.0", 8)]);
        let made = with(&text, MARKS, &one).unwrap();
        assert_eq!(
            made,
            "allow tcp 22\n# database(8): who reaches PostgreSQL from other machines (ADR 0570)\n\
             allow tcp 5432 from 10.0.0.0/8\ndeny tcp 5432\n# database(8): end\ndeny udp 5353 on net1\n"
        );
        assert_eq!(with(&made, MARKS, &one).unwrap(), made, "the same rules write the same file");
        assert_eq!(rules_of(&made, MARKS).unwrap(), Some(one));
        assert_eq!(rules_of("allow tcp 22\n", MARKS).unwrap(), None);
    }

    #[test]
    fn marks_are_matched_whole_with_their_spaces_and_a_carriage_return_taken_off() {
        let text = "  # database(8): who reaches PostgreSQL from other machines (ADR 0570)\t\r\nallow tcp 5432 from 10.0.0.0/8\r\n# database(8): end  \n";
        assert_eq!(rules_of(text, MARKS).unwrap().map(|rules| rules.len()), Some(1));
        let like = "# database(8): end of what?\n# database(8): who reaches PostgreSQL from other machines\n";
        assert_eq!(
            rules_of(like, MARKS).unwrap(),
            None,
            "a line that only begins as a mark is none"
        );
    }

    #[test]
    fn a_block_with_no_rule_wanted_is_taken_away_with_the_blank_line_before_it() {
        let text = format!("allow tcp 22\n\n{BLOCK}");
        assert_eq!(with(&text, MARKS, &[]).unwrap(), "allow tcp 22\n");
        assert_eq!(without(&text, MARKS).unwrap(), "allow tcp 22\n");
        let middle = format!("allow tcp 22\n\n{BLOCK}\ndeny udp 53\n");
        assert_eq!(
            without(&middle, MARKS).unwrap(),
            "allow tcp 22\n\n\ndeny udp 53\n",
            "a block in the middle leaves the lines around it"
        );
        assert_eq!(
            without("allow tcp 22", MARKS).unwrap(),
            "allow tcp 22",
            "a file with no block is the same"
        );
        assert_eq!(without(BLOCK, MARKS).unwrap(), "");
        let back = with(&without(&text, MARKS).unwrap(), MARKS, &two()).unwrap();
        assert_eq!(back, text, "taken away and written again is the file it was");
    }

    #[test]
    fn marks_that_make_no_one_block_are_refused_and_nothing_is_guessed() {
        let end_first = format!("# database(8): end\n{}\n", MARKS.start);
        let start_only = format!("{}\nallow tcp 5432\n", MARKS.start);
        let end_only = "allow tcp 22\n# database(8): end\n".to_string();
        let twice = format!("{BLOCK}{BLOCK}");
        for (text, words) in [
            (&end_first, "line 1 ends a block that line 2 starts after it"),
            (&start_only, "line 1 starts a block that no line ends"),
            (&end_only, "line 2 ends a block that no line starts"),
            (&twice, "2 lines that start the block and 2 that end it"),
        ] {
            for result in [with(text, MARKS, &two()), without(text, MARKS)] {
                let why = result.unwrap_err();
                assert!(why.contains(words) && why.ends_with("mend the file by hand"), "{why}");
            }
            assert!(rules_of(text, MARKS).is_err());
            assert!(decides_first(text, MARKS, 5432).is_err());
        }
    }

    #[test]
    fn a_file_that_would_hold_more_rules_than_the_firewall_is_refused() {
        let full: String = ports_open(RULES_MAX - 2);
        let why = with(&full, MARKS, &two()).unwrap_err();
        assert!(why.contains(&format!("{} rules", RULES_MAX + 1)), "{why}");
        let room: String = ports_open(RULES_MAX - 3);
        assert!(with(&room, MARKS, &two()).is_ok(), "the most the firewall holds is taken");
    }

    #[test]
    fn a_rule_before_the_block_that_holds_the_port_decides_first_and_is_named() {
        let block = |before: &str| format!("{before}\n{BLOCK}");
        for (before, first) in [
            ("allow tcp", Some("allow tcp")),
            ("allow tcp 5000-6000", Some("allow tcp 5000-6000")),
            ("deny tcp 5432 from 203.0.113.0/24", Some("deny tcp 5432 from 203.0.113.0/24")),
            ("allow tcp 5432 on net1", Some("allow tcp 5432 on net1")),
            ("allow udp 5432", None),
            ("allow tcp 22\nallow ping", None),
            ("# allow tcp 5432", None),
            ("", None),
        ] {
            assert_eq!(
                decides_first(&block(before), MARKS, 5432).unwrap().as_deref(),
                first,
                "{before}"
            );
        }
        let after = format!("{BLOCK}allow tcp\n");
        assert_eq!(
            decides_first(&after, MARKS, 5432).unwrap(),
            None,
            "a rule after the block decides after it"
        );
        assert_eq!(
            decides_first("allow tcp\n", MARKS, 5432).unwrap(),
            None,
            "no block, nothing to come first of"
        );
    }

    #[test]
    fn the_block_s_rules_are_read_as_the_firewall_reads_the_whole_file() {
        let made = with("allow tcp 22\n", MARKS, &two()).unwrap();
        let all = super::super::rules_in(&made);
        assert_eq!(all.len(), 4);
        assert_eq!(all[3].to_string(), "deny tcp 5432");
        let opening = super::super::lets_tcp_in(&made, 5432, true);
        assert!(!opening.open, "for every source but the networks named, the port is shut");
        assert_eq!(opening.rule, Some(4));
    }
}
