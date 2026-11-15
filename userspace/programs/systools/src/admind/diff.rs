// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A unified diff of two texts, for the journal's "What changed" (ADR
//! 0516). The tree had none, and a crate for it would be one more to keep
//! for a few dozen lines.
//!
//! The lines are compared with Myers's algorithm, after the lines the two
//! texts start and end with in common are set aside, which is nearly all
//! of a settings file. It looks for the shortest list of lines to delete
//! and insert, but gives up when that is more than [`EDITS_MAX`] lines:
//! the diff then deletes the middle of the old text and inserts the
//! middle of the new, which is a correct diff that shows more than it
//! must, and keeps the memory of the search small. A diff has at most as
//! many lines as the caller asks for, the rest being left out and said so.

/// The most lines to delete and insert that are searched for the shortest
/// list; past it the middle of the texts is deleted and inserted whole.
const EDITS_MAX: usize = 500;

/// The lines of context around each change.
const CONTEXT: usize = 3;

/// What was made of two texts.
#[derive(Debug, PartialEq, Eq)]
pub struct Diff {
    /// The unified diff, with its two header lines; empty for two texts
    /// that are the same.
    pub text: String,
    /// Whether it stops before the end, for the line limit.
    pub truncated: bool,
}

/// One line of the lists, in an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    /// A line both texts have.
    Keep,
    /// A line only the old text has.
    Delete,
    /// A line only the new text has.
    Insert,
}

/// The lines of `text`, each with its newline if it has one.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The shortest list of [`Edit`]s that makes `old` into `new`, or a list
/// that deletes all of `old` and inserts all of `new` when the shortest
/// has more than [`EDITS_MAX`] lines to delete and insert.
fn edits<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Edit> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
    let suffix =
        old_rest.iter().rev().zip(new_rest.iter().rev()).take_while(|(a, b)| a == b).count();
    let (old_mid, new_mid) =
        (&old_rest[..old_rest.len() - suffix], &new_rest[..new_rest.len() - suffix]);
    let mut all = vec![Edit::Keep; prefix];
    match myers(old_mid, new_mid) {
        Some(middle) => all.extend(middle),
        None => {
            all.extend(std::iter::repeat_n(Edit::Delete, old_mid.len()));
            all.extend(std::iter::repeat_n(Edit::Insert, new_mid.len()));
        }
    }
    all.extend(std::iter::repeat_n(Edit::Keep, suffix));
    all
}

/// Myers's shortest edit script from `old` to `new`, which have no line
/// at their start or end in common; `None` when it takes more than
/// [`EDITS_MAX`] lines to delete and insert.
fn myers(old: &[&str], new: &[&str]) -> Option<Vec<Edit>> {
    let (n, m) = (old.len() as isize, new.len() as isize);
    if n == 0 || m == 0 {
        let mut all = vec![Edit::Delete; old.len()];
        all.extend(std::iter::repeat_n(Edit::Insert, new.len()));
        return Some(all);
    }
    let limit = (n + m).min(EDITS_MAX as isize);
    // `trace[d]` is the furthest x on each diagonal k = -d..=d after d
    // edits, as index k + d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut furthest = vec![0isize; 2 * limit as usize + 2];
    let offset = limit + 1;
    let mut found = None;
    'search: for d in 0..=limit {
        for k in (-d..=d).step_by(2) {
            let at = (k + offset) as usize;
            let mut x = if k == -d || (k != d && furthest[at - 1] < furthest[at + 1]) {
                furthest[at + 1]
            } else {
                furthest[at - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            furthest[at] = x;
            if x >= n && y >= m {
                found = Some(d);
                trace.push(((-d)..=d).map(|k| furthest[(k + offset) as usize]).collect());
                break 'search;
            }
        }
        trace.push(((-d)..=d).map(|k| furthest[(k + offset) as usize]).collect());
    }
    let last = found?;
    // Walk back from the end to the start, one edit for each step.
    let (mut x, mut y) = (n, m);
    let mut script = Vec::new();
    for d in (1..=last).rev() {
        let k = x - y;
        let previous = &trace[(d - 1) as usize];
        let at = |k: isize| previous[(k + d - 1) as usize];
        let came_from = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let (prev_x, prev_y) = (at(came_from), at(came_from) - came_from);
        while x > prev_x && y > prev_y {
            script.push(Edit::Keep);
            x -= 1;
            y -= 1;
        }
        script.push(if x == prev_x { Edit::Insert } else { Edit::Delete });
        (x, y) = (prev_x, prev_y);
    }
    script.extend(std::iter::repeat_n(Edit::Keep, x as usize));
    script.reverse();
    Some(script)
}

/// A line as a diff shows it: the sign, the line, and a note when it has
/// no newline.
fn shown(sign: char, line: &str, out: &mut Vec<String>) {
    match line.strip_suffix('\n') {
        Some(line) => out.push(format!("{sign}{line}")),
        None => {
            out.push(format!("{sign}{line}"));
            out.push("\\ No newline at end of file".to_string());
        }
    }
}

/// The runs of lines in `script` that are not kept, as the indexes where
/// each starts and ends, joined into hunks: two runs with no more than two
/// contexts of kept lines between them are one hunk.
fn hunks(script: &[Edit]) -> Vec<(usize, usize)> {
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < script.len() {
        if script[at] == Edit::Keep {
            at += 1;
            continue;
        }
        let start = at;
        while at < script.len() && script[at] != Edit::Keep {
            at += 1;
        }
        match hunks.last_mut() {
            Some(last) if start - last.1 <= 2 * CONTEXT => last.1 = at,
            _ => hunks.push((start, at)),
        }
    }
    hunks
}

/// The unified diff from `old`, named `old_name`, to `new`, named
/// `new_name`, of at most `max` lines.
pub fn unified(old: &str, new: &str, old_name: &str, new_name: &str, max: usize) -> Diff {
    let (old_lines, new_lines) = (lines(old), lines(new));
    let script = edits(&old_lines, &new_lines);
    let hunks = hunks(&script);
    if hunks.is_empty() {
        return Diff { text: String::new(), truncated: false };
    }
    // The line of each text that each edit comes at.
    let (mut x, mut y) = (0, 0);
    let mut places = Vec::with_capacity(script.len());
    for edit in &script {
        places.push((x, y));
        match edit {
            Edit::Keep => (x, y) = (x + 1, y + 1),
            Edit::Delete => x += 1,
            Edit::Insert => y += 1,
        }
    }
    let mut out = vec![format!("--- {old_name}"), format!("+++ {new_name}")];
    for (changed_from, changed_to) in hunks {
        let start = changed_from.saturating_sub(CONTEXT);
        let end = (changed_to + CONTEXT).min(script.len());
        let (old_start, new_start) = places[start];
        let old_count = script[start..end].iter().filter(|edit| **edit != Edit::Insert).count();
        let new_count = script[start..end].iter().filter(|edit| **edit != Edit::Delete).count();
        // A side with no lines starts at the line before, as diff writes it.
        let number = |start: usize, count: usize| if count == 0 { start } else { start + 1 };
        out.push(format!(
            "@@ -{},{old_count} +{},{new_count} @@",
            number(old_start, old_count),
            number(new_start, new_count)
        ));
        for (edit, (x, y)) in script[start..end].iter().zip(&places[start..end]) {
            match edit {
                Edit::Keep => shown(' ', old_lines[*x], &mut out),
                Edit::Delete => shown('-', old_lines[*x], &mut out),
                Edit::Insert => shown('+', new_lines[*y], &mut out),
            }
        }
    }
    let truncated = out.len() > max;
    out.truncate(max);
    let mut text = out.join("\n");
    text.push('\n');
    Diff { text, truncated }
}

#[cfg(test)]
mod tests;
