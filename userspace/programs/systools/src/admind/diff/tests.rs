// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::*;

/// The diff of `old` to `new` with no limit, by the names `a` and `b`.
fn diff(old: &str, new: &str) -> String {
    unified(old, new, "a", "b", usize::MAX).text
}

/// Applies a unified diff `text` to `old`, as patch would, so that a test
/// can check that a diff says what it must: that it turns the old text
/// into the new.
fn apply(old: &str, text: &str) -> String {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut at = 0;
    let mut lines = text.lines().skip(2).peekable();
    while let Some(header) = lines.next() {
        let header = header.strip_prefix("@@ -").unwrap();
        let (old_part, _) = header.split_once(' ').unwrap();
        let (start, count) = old_part.split_once(',').unwrap();
        let (start, count): (usize, usize) = (start.parse().unwrap(), count.parse().unwrap());
        let first = if count == 0 { start } else { start - 1 };
        while at < first {
            out.push_str(old_lines[at]);
            at += 1;
        }
        let mut last = " ";
        while let Some(line) = lines.next_if(|line| !line.starts_with("@@")) {
            if line.starts_with('\\') {
                // "No newline at end of file" says the line before has none:
                // a line of the old text has none already, and one that was
                // deleted is not in the new.
                if last == "+" {
                    assert!(out.ends_with('\n'));
                    out.pop();
                }
                continue;
            }
            let (sign, rest) = line.split_at(1);
            last = sign;
            match sign {
                " " => {
                    assert_eq!(old_lines[at].trim_end_matches('\n'), rest);
                    out.push_str(old_lines[at]);
                    at += 1;
                }
                "-" => {
                    assert_eq!(old_lines[at].trim_end_matches('\n'), rest);
                    at += 1;
                }
                "+" => {
                    out.push_str(rest);
                    out.push('\n');
                }
                _ => panic!("{line:?}"),
            }
        }
    }
    while at < old_lines.len() {
        out.push_str(old_lines[at]);
        at += 1;
    }
    out
}

#[test]
fn two_texts_that_are_the_same_have_no_diff() {
    assert_eq!(diff("", ""), "");
    assert_eq!(diff("a\nb\n", "a\nb\n"), "");
    assert_eq!(diff("no newline", "no newline"), "");
}

#[test]
fn a_changed_line_is_deleted_and_inserted_with_its_context() {
    let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
    let new = "one\ntwo\nthree\nfour\n5\nsix\nseven\neight\nnine\n";
    assert_eq!(
        diff(old, new),
        "--- a\n+++ b\n@@ -2,7 +2,7 @@\n two\n three\n four\n-five\n+5\n six\n seven\n eight\n"
    );
}

#[test]
fn a_change_at_the_start_and_one_at_the_end_have_as_much_context_as_there_is() {
    assert_eq!(diff("a\nb\nc\n", "x\nb\nc\n"), "--- a\n+++ b\n@@ -1,3 +1,3 @@\n-a\n+x\n b\n c\n");
    assert_eq!(diff("a\nb\nc\n", "a\nb\nx\n"), "--- a\n+++ b\n@@ -1,3 +1,3 @@\n a\n b\n-c\n+x\n");
}

#[test]
fn lines_added_to_an_empty_text_and_all_deleted_have_a_side_with_no_lines() {
    assert_eq!(diff("", "a\nb\n"), "--- a\n+++ b\n@@ -0,0 +1,2 @@\n+a\n+b\n");
    assert_eq!(diff("a\nb\n", ""), "--- a\n+++ b\n@@ -1,2 +0,0 @@\n-a\n-b\n");
}

#[test]
fn changes_far_apart_are_two_hunks_and_near_ones_are_one() {
    let numbers = |changed: &[usize]| -> String {
        (1..=30)
            .map(|n| if changed.contains(&n) { format!("X{n}\n") } else { format!("{n}\n") })
            .collect()
    };
    let old = numbers(&[]);
    let far = diff(&old, &numbers(&[3, 20]));
    assert_eq!(far.matches("@@ -").count(), 2, "{far}");
    // Seven lines between the changes: the contexts touch.
    let near = diff(&old, &numbers(&[10, 17]));
    assert_eq!(near.matches("@@ -").count(), 1, "{near}");
    // Eight lines between: they do not.
    let apart = diff(&old, &numbers(&[10, 18]));
    assert_eq!(apart.matches("@@ -").count(), 2, "{apart}");
    for changed in [&[3usize, 20][..], &[10, 17], &[10, 18]] {
        let new = numbers(changed);
        assert_eq!(apply(&old, &diff(&old, &new)), new);
    }
}

#[test]
fn a_last_line_with_no_newline_is_a_line_of_its_own_and_is_said() {
    let text = diff("a\nb", "a\nb\n");
    assert_eq!(text, "--- a\n+++ b\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n");
    assert_eq!(apply("a\nb", &text), "a\nb\n");
    let text = diff("a\nb\n", "a\nb");
    assert_eq!(apply("a\nb\n", &text), "a\nb");
}

#[test]
fn a_diff_turns_the_old_text_into_the_new() {
    let cases = [
        ("a\nb\nc\nd\n", "b\nc\nd\ne\n"),
        ("a\nb\nc\nd\n", "d\nc\nb\na\n"),
        ("x\nx\nx\nx\n", "x\nx\n"),
        ("x\nx\n", "x\nx\nx\nx\n"),
        ("1\n2\n3\n4\n5\n6\n7\n8\n9\n", "1\n2\n3\n9\n5\n6\n7\n8\n4\n"),
        ("a\n\n\nb\n", "a\nb\n\n\n"),
        ("", "only\n"),
        ("only", ""),
    ];
    for (old, new) in cases {
        let text = diff(old, new);
        assert_eq!(apply(old, &text), new, "{old:?} to {new:?}\n{text}");
    }
}

#[test]
fn texts_that_differ_in_many_lines_are_still_a_correct_diff() {
    // More changes than the search follows: the middle is deleted and
    // inserted whole, which is more than the shortest diff and as true.
    let (old, new) = (numbered("old ", 0, 800), numbered("new ", 0, 800));
    let text = diff(&old, &new);
    assert_eq!(apply(&old, &text), new);
    assert!(text.contains("@@ -1,800 +1,800 @@"));
}

#[test]
fn a_pseudo_random_edit_of_a_long_text_is_found_whole() {
    let mut seed = 12345u64;
    let mut next = move |bound: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % bound
    };
    for _ in 0..50 {
        let old: Vec<String> = (0..120).map(|n| format!("line {n}")).collect();
        let mut new = old.clone();
        for _ in 0..next(12) {
            let at = next(new.len() as u64) as usize;
            match next(3) {
                0 => {
                    new.remove(at);
                }
                1 => new.insert(at, format!("added {}", next(1000))),
                _ => new[at] = format!("changed {}", next(1000)),
            }
        }
        let (old, new) = (old.join("\n") + "\n", new.join("\n") + "\n");
        let text = diff(&old, &new);
        assert_eq!(apply(&old, &text), new, "{text}");
    }
}

#[test]
fn a_diff_has_at_most_the_lines_asked_for_and_says_when_it_stops() {
    let (old, new) = (numbered("", 0, 100), numbered("", 1000, 1100));
    let all = unified(&old, &new, "a", "b", usize::MAX);
    assert!(!all.truncated);
    let some = unified(&old, &new, "a", "b", 10);
    assert!(some.truncated);
    assert_eq!(some.text.lines().count(), 10);
    assert!(all.text.starts_with(&some.text));
    // A diff of exactly the limit is whole.
    let count = all.text.lines().count();
    assert!(!unified(&old, &new, "a", "b", count).truncated);
    assert!(unified(&old, &new, "a", "b", count - 1).truncated);
}

/// The lines `PREFIX` and a number each, for the numbers from `from` up to
/// `to`.
fn numbered(prefix: &str, from: u32, to: u32) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for n in from..to {
        let _ = writeln!(text, "{prefix}{n}");
    }
    text
}
