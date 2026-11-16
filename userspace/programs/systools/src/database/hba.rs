// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The block of `pg_hba.conf` that `database` keeps: who may connect from
//! other machines.
//!
//! The file is the administrator's. `database` writes only between two
//! comment lines,
//!
//! ```text
//! # database(8): who connects from other machines (ADR 0569)
//! hostssl "shop" "shop" 192.168.1.0/24 scram-sha-256
//! # database(8): end
//! ```
//!
//! one line for each network a database is opened to, at the end of the
//! file, after the lines of the bundle and after a blank one. Every other
//! line stays as it was written, byte for byte, and is the
//! administrator's: `database list` shows it as written by hand. A line
//! of the block that is none of `database`'s own form is kept too, and
//! the block stays while one is in it.

use super::names::Network;

/// The comment that begins the block.
pub const BEGIN: &str = "# database(8): who connects from other machines (ADR 0569)";

/// The comment that ends the block.
pub const END: &str = "# database(8): end";

/// The line a network of a database is opened by: over TLS, with the
/// role's password, as SCRAM-SHA-256 keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The database the line is for.
    pub database: String,
    /// The role that signs in to it.
    pub role: String,
    /// The addresses that may.
    pub network: Network,
}

impl Rule {
    /// The line, without its newline.
    #[must_use]
    pub fn line(&self) -> String {
        format!("hostssl \"{}\" \"{}\" {} scram-sha-256", self.database, self.role, self.network)
    }

    /// The rule a line of the block writes, when it is in the form
    /// [`Rule::line`] gives, with the names quoted or not.
    fn parse(line: &str) -> Option<Rule> {
        let words = words(line)?;
        let [kind, database, role, address, method] = words.as_slice() else { return None };
        if kind != "hostssl" || method != "scram-sha-256" {
            return None;
        }
        Some(Rule {
            database: database.clone(),
            role: role.clone(),
            network: Network::parse(address).ok()?,
        })
    }
}

/// The words of a line as `pg_hba.conf` reads them: separated by blanks,
/// a double quote holding a word together, and a `#` outside quotes
/// beginning a comment. None for a comment or a blank line, and for a
/// quote that is not closed.
fn words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut in_word, mut quoted) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '#' if !quoted => break,
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if in_word {
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

/// What a line of the file is, without its newline.
fn text_of(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// The place of the block in a file's lines.
struct Place {
    /// The index of the line that begins the block.
    begin: usize,
    /// The index of the line that ends it.
    end: usize,
}

/// The lines of `text`, each with its newline.
fn lines_of(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Where the block is in `lines`.
///
/// # Errors
///
/// When the file has the block's first line twice, or has it and not its
/// last.
fn find(lines: &[&str]) -> Result<Option<Place>, String> {
    let mut begins = lines.iter().enumerate().filter(|(_, line)| text_of(line).trim_end() == BEGIN);
    let Some((begin, _)) = begins.next() else { return Ok(None) };
    if let Some((again, _)) = begins.next() {
        return Err(format!(
            "pg_hba.conf has database's first line twice, at lines {} and {}",
            begin + 1,
            again + 1
        ));
    }
    let end = lines[begin + 1..]
        .iter()
        .position(|line| text_of(line).trim_end() == END)
        .map(|at| begin + 1 + at)
        .ok_or_else(|| {
            format!(
                "pg_hba.conf has database's first line, at line {}, and not its last, `{END}`",
                begin + 1
            )
        })?;
    Ok(Some(Place { begin, end }))
}

/// The lines `database` wrote in `text`, in the order they are in.
///
/// # Errors
///
/// When the block is not whole, as [`find`] says.
pub fn rules(text: &str) -> Result<Vec<Rule>, String> {
    let lines = lines_of(text);
    let Some(place) = find(&lines)? else { return Ok(Vec::new()) };
    Ok(lines[place.begin + 1..place.end].iter().filter_map(|line| Rule::parse(line)).collect())
}

/// The line numbers, counted from 1, of the block's first and last
/// lines: the lines between them, and those two, are not written by hand.
/// None when there is no block, or it is not whole.
#[must_use]
pub fn block_lines(text: &str) -> Option<(usize, usize)> {
    let lines = lines_of(text);
    let place = find(&lines).ok()??;
    Some((place.begin + 1, place.end + 1))
}

/// `text` with the lines `add` in the block, which is made at the end of
/// the file when there is none. A line the block has already is not
/// written again.
///
/// # Errors
///
/// When the block is not whole.
pub fn with_rules(text: &str, add: &[Rule]) -> Result<String, String> {
    let lines = lines_of(text);
    let place = find(&lines)?;
    let have: Vec<Rule> = match &place {
        Some(place) => {
            lines[place.begin + 1..place.end].iter().filter_map(|line| Rule::parse(line)).collect()
        }
        None => Vec::new(),
    };
    let mut new_lines: Vec<String> = Vec::new();
    for rule in add {
        let line = rule.line();
        if !have.contains(rule) && !new_lines.contains(&line) {
            new_lines.push(line);
        }
    }
    let mut written = String::new();
    for line in &new_lines {
        written.push_str(line);
        written.push('\n');
    }
    match place {
        Some(place) => {
            let mut out = String::with_capacity(text.len() + written.len());
            for (at, line) in lines.iter().enumerate() {
                if at == place.end {
                    out.push_str(&written);
                }
                out.push_str(line);
            }
            Ok(out)
        }
        None if new_lines.is_empty() => Ok(text.to_string()),
        None => {
            let mut out = text.to_string();
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(&format!("{BEGIN}\n{written}{END}\n"));
            Ok(out)
        }
    }
}

/// `text` without the lines of the block that `gone` says yes to. A block
/// with no line of its own left, and none written by hand in it, goes
/// with the blank line that came before it.
///
/// # Errors
///
/// When the block is not whole.
pub fn without_rules(text: &str, gone: impl Fn(&Rule) -> bool) -> Result<String, String> {
    let lines = lines_of(text);
    let Some(place) = find(&lines)? else { return Ok(text.to_string()) };
    let inside = &lines[place.begin + 1..place.end];
    let keep: Vec<&str> = inside
        .iter()
        .copied()
        .filter(|line| Rule::parse(line).is_none_or(|rule| !gone(&rule)))
        .collect();
    let empty = keep.iter().all(|line| text_of(line).trim().is_empty());
    let mut out = String::with_capacity(text.len());
    if empty {
        let before = &lines[..place.begin];
        let blank = before.last().is_some_and(|line| text_of(line).trim().is_empty());
        let kept = if blank { before.len() - 1 } else { before.len() };
        before[..kept].iter().for_each(|line| out.push_str(line));
        lines[place.end + 1..].iter().for_each(|line| out.push_str(line));
        return Ok(out);
    }
    lines[..=place.begin].iter().for_each(|line| out.push_str(line));
    keep.iter().for_each(|line| out.push_str(line));
    lines[place.end..].iter().for_each(|line| out.push_str(line));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle's `pg_hba.conf`, as far as the block goes.
    const BUNDLE: &str = "\
# TYPE  DATABASE     USER      ADDRESS         METHOD

local   all          postgres                  peer map=administrators
local   all          all                       peer
host    all          all       127.0.0.1/32    scram-sha-256
host    all          all       ::1/128         scram-sha-256
";

    fn rule(database: &str, role: &str, network: &str) -> Rule {
        Rule {
            database: database.into(),
            role: role.into(),
            network: Network::parse(network).unwrap(),
        }
    }

    fn lab() -> Rule {
        rule("shop", "shop", "192.168.1.0/24")
    }

    #[test]
    fn a_rule_is_a_hostssl_line_with_scram() {
        assert_eq!(lab().line(), "hostssl \"shop\" \"shop\" 192.168.1.0/24 scram-sha-256");
        assert_eq!(
            rule("blog", "writer", "2001:db8::/64").line(),
            "hostssl \"blog\" \"writer\" 2001:db8::/64 scram-sha-256"
        );
    }

    #[test]
    fn the_block_is_made_at_the_end_after_a_blank_line() {
        let text = with_rules(BUNDLE, &[lab()]).unwrap();
        assert_eq!(
            text,
            format!(
                "{BUNDLE}\n# database(8): who connects from other machines (ADR 0569)\n\
                 hostssl \"shop\" \"shop\" 192.168.1.0/24 scram-sha-256\n# database(8): end\n"
            )
        );
        assert_eq!(rules(&text).unwrap(), vec![lab()]);
    }

    #[test]
    fn a_file_with_no_newline_at_its_end_gets_one_before_the_block() {
        let text = with_rules("local all all peer", &[lab()]).unwrap();
        assert!(text.starts_with("local all all peer\n\n# database(8): who"), "{text}");
        let empty = with_rules("", &[lab()]).unwrap();
        assert!(empty.starts_with(BEGIN), "{empty}");
    }

    #[test]
    fn a_line_joins_the_block_before_its_last_line_and_is_never_written_twice() {
        let one = with_rules(BUNDLE, &[lab()]).unwrap();
        let two = with_rules(&one, &[lab(), rule("blog", "blog", "10.0.0.0/8")]).unwrap();
        assert_eq!(rules(&two).unwrap(), vec![lab(), rule("blog", "blog", "10.0.0.0/8")], "{two}");
        assert!(two.ends_with("scram-sha-256\n# database(8): end\n"));
        assert_eq!(two.matches(BEGIN).count(), 1);
        // The same again changes nothing.
        assert_eq!(with_rules(&two, &[lab()]).unwrap(), two);
        // Twice in one call is once.
        let once = with_rules(BUNDLE, &[lab(), lab()]).unwrap();
        assert_eq!(rules(&once).unwrap(), vec![lab()]);
        // Nothing to add makes no block.
        assert_eq!(with_rules(BUNDLE, &[]).unwrap(), BUNDLE);
    }

    #[test]
    fn every_line_outside_the_block_is_kept_as_written() {
        let hand = "# my own lines\r\nhost  all\talll  10.9.0.0/16\tmd5   # old\n\n\n   host replication all 127.0.0.1/32 trust\n";
        let with = with_rules(hand, &[lab()]).unwrap();
        assert!(with.starts_with(hand), "{with}");
        let after = format!("{with}# a line after the block\nlocal all all trust\n");
        let two = with_rules(&after, &[rule("blog", "blog", "10.0.0.0/8")]).unwrap();
        assert!(two.starts_with(hand));
        assert!(two.ends_with("# a line after the block\nlocal all all trust\n"));
        let gone = without_rules(&two, |_| true).unwrap();
        assert_eq!(gone, format!("{hand}# a line after the block\nlocal all all trust\n"));
    }

    #[test]
    fn taking_the_lines_of_one_database_out_keeps_the_others_and_the_block() {
        let base = with_rules(
            BUNDLE,
            &[lab(), rule("shop", "shop", "10.0.0.0/8"), rule("blog", "blog", "10.0.0.0/8")],
        )
        .unwrap();
        let some = without_rules(&base, |r| r.database == "shop").unwrap();
        assert_eq!(rules(&some).unwrap(), vec![rule("blog", "blog", "10.0.0.0/8")]);
        assert!(some.contains(BEGIN) && some.contains(END));
        let one =
            without_rules(&base, |r| r.database == "shop" && r.network.to_string() == "10.0.0.0/8")
                .unwrap();
        assert_eq!(rules(&one).unwrap(), vec![lab(), rule("blog", "blog", "10.0.0.0/8")]);
        // None that matches: the text is the same.
        assert_eq!(without_rules(&base, |_| false).unwrap(), base);
    }

    #[test]
    fn the_last_line_takes_the_block_and_its_blank_line_with_it() {
        let with = with_rules(BUNDLE, &[lab()]).unwrap();
        assert_eq!(without_rules(&with, |_| true).unwrap(), BUNDLE);
        // The blank lines the file had at its end are its own.
        let trailing = format!("{BUNDLE}\n\n");
        let with = with_rules(&trailing, &[lab()]).unwrap();
        assert_eq!(without_rules(&with, |_| true).unwrap(), trailing);
        // A file that was only the block.
        let only = with_rules("", &[lab()]).unwrap();
        assert_eq!(without_rules(&only, |_| true).unwrap(), "");
        // No block, nothing to take out.
        assert_eq!(without_rules(BUNDLE, |_| true).unwrap(), BUNDLE);
    }

    #[test]
    fn a_line_in_the_block_that_is_not_database_s_stays_and_keeps_the_block() {
        let text = format!(
            "{BUNDLE}\n{BEGIN}\n# a note of mine\n{}\nhostssl all all 10.9.0.0/16 cert\n{END}\n",
            lab().line()
        );
        assert_eq!(rules(&text).unwrap(), vec![lab()]);
        let gone = without_rules(&text, |_| true).unwrap();
        assert_eq!(
            gone,
            format!(
                "{BUNDLE}\n{BEGIN}\n# a note of mine\nhostssl all all 10.9.0.0/16 cert\n{END}\n"
            )
        );
        assert_eq!(rules(&gone).unwrap(), vec![]);
    }

    #[test]
    fn the_lines_written_with_other_blanks_or_without_quotes_are_read() {
        let text = format!(
            "{BEGIN}\nhostssl\tshop   shop 192.168.1.77/24   scram-sha-256   # why\n{END}\n"
        );
        assert_eq!(rules(&text).unwrap(), vec![lab()]);
        let wrong = format!(
            "{BEGIN}\nhostssl \"shop\" \"shop\" 192.168.1.0/24 md5\n\
             host \"shop\" \"shop\" 192.168.1.0/24 scram-sha-256\n\
             hostssl \"shop\" \"shop\" 192.168.1.0 scram-sha-256\n\
             hostssl \"shop\" \"sh scram-sha-256\n{END}\n"
        );
        assert_eq!(rules(&wrong).unwrap(), vec![]);
    }

    #[test]
    fn by_hand_is_every_line_outside_the_block() {
        assert_eq!(block_lines(BUNDLE), None);
        let text = with_rules(BUNDLE, &[lab(), rule("blog", "blog", "10.0.0.0/8")]).unwrap();
        // The bundle's six lines, a blank one, the block's four.
        assert_eq!(block_lines(&text), Some((8, 11)));
        let after = format!("{text}host all all 10.1.0.0/16 md5\n");
        assert_eq!(block_lines(&after), Some((8, 11)));
    }

    #[test]
    fn a_block_that_is_not_whole_is_refused_and_nothing_is_changed() {
        let no_end = format!("{BUNDLE}{BEGIN}\n{}\n", lab().line());
        let twice = format!("{BUNDLE}{BEGIN}\n{END}\n{BEGIN}\n{END}\n");
        for text in [&no_end, &twice] {
            assert!(rules(text).is_err(), "{text}");
            assert!(with_rules(text, &[lab()]).is_err(), "{text}");
            assert!(without_rules(text, |_| true).is_err(), "{text}");
            assert_eq!(block_lines(text), None);
        }
        assert!(rules(&no_end).unwrap_err().contains("not its last"));
        assert!(rules(&twice).unwrap_err().contains("twice"));
        // A last line alone is a comment.
        assert_eq!(rules(&format!("{END}\n")).unwrap(), vec![]);
    }

    #[test]
    fn the_words_of_a_line_are_read_as_postgresql_reads_them() {
        assert_eq!(
            words("host \"my db\" all 10.0.0.0/8 scram-sha-256 # why"),
            Some(
                ["host", "my db", "all", "10.0.0.0/8", "scram-sha-256"].map(String::from).to_vec()
            )
        );
        assert_eq!(words("   # a comment"), None);
        assert_eq!(words(""), None);
        assert_eq!(words("host \"all"), None);
        assert_eq!(words("a \"#\" b"), Some(["a", "#", "b"].map(String::from).to_vec()));
    }
}
