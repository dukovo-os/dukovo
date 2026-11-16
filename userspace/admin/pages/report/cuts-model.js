// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The text of a report and what a person cuts of it, as plain values: the
// text as rows that know their part and their number in it, the list of cuts
// that reportd takes (against the lines as it first drafted them, never
// against the text as it stands), and the way a place in the shown text goes
// back to the line and the characters that were drafted. No DOM.

/** The parts of a report that the system gathered, which can be cut and never typed into. */
export const CUTTABLE = ['crash', 'failed-start', 'log', 'start-log'];

/** What a cut run of words reads as in the text. */
export const CUT = '[cut]';

const CUT_LENGTH = [...CUT].length;
const MARKS = /\[(?:user|host|IPv4|IPv6|MAC|email|Wi-Fi|serial|machine|cut)\]/g;

/** The characters of `text` before the UTF-16 offset `at`, counted as the code points that reportd counts. */
const pointsBefore = (text, at) => [...text.slice(0, at)].length;

/**
 * A line as pieces: `[{text, mark}]`, `mark` being `'taken'` for what
 * redaction took out (`[user]`), `'cut'` for words the person cut, and null
 * for the rest.
 */
export function piecesOf(line) {
  const pieces = [];
  let at = 0;
  for (const found of line.matchAll(MARKS)) {
    if (found.index > at) pieces.push({ text: line.slice(at, found.index), mark: null });
    pieces.push({ text: found[0], mark: found[0] === CUT ? 'cut' : 'taken' });
    at = found.index + found[0].length;
  }
  if (at < line.length) pieces.push({ text: line.slice(at), mark: null });
  return pieces;
}

/**
 * The rows of an answer's text, one for each line: `{n, text, kind, section,
 * at}`. `kind` is `head` (and its blank line, which is `plain`), `heading`,
 * `typed` for what the person wrote, `gathered` for what the system did, or
 * `plain`; `section` is the part's ID and `at` the line's number in it,
 * from 0, which is its place in the lines of the draft as the answer says
 * the part is now. A heading of the answers carries `typed: true`.
 */
export function rowsOf(answer) {
  const lines = String(answer?.text ?? '').split('\n');
  if (lines.at(-1) === '') lines.pop();
  const rows = lines.map((text, n) => ({ n, text, kind: 'plain', section: null, at: null }));
  const typed = new Set((answer?.typed ?? []).map((place) => place.id));
  for (const place of answer?.sections ?? []) {
    const kind = place.id === 'head' ? 'head' : typed.has(place.id) ? 'typed' : CUTTABLE.includes(place.id) ? 'gathered' : 'plain';
    const heading = rows[place.first_line - 1];
    if (place.id !== 'head' && heading) Object.assign(heading, { kind: 'heading', section: place.id, typed: kind === 'typed' });
    for (let at = 0; at < place.lines && rows[place.first_line + at]; at += 1) Object.assign(rows[place.first_line + at], { kind, section: place.id, at });
  }
  return rows;
}

/** The lines of each part that the system gathered, as the first draft has them: a Map of the part's ID to its lines. */
export function gatheredOf(first) {
  const lines = new Map();
  for (const row of rowsOf(first)) {
    if (row.kind === 'gathered') lines.set(row.section, [...(lines.get(row.section) ?? []), row.text]);
  }
  return lines;
}

// ---- the list of cuts ------------------------------------------------------

/** The cuts, put in order and merged: whole lines as runs, and words as ranges that touch merged, none inside a line cut whole. */
export function normalize(cuts) {
  const gone = new Map();
  const words = new Map();
  for (const cut of cuts) {
    if (cut.from === undefined) {
      const set = gone.get(cut.section) ?? new Set();
      for (let line = cut.line; line <= (cut.through ?? cut.line); line += 1) set.add(line);
      gone.set(cut.section, set);
    } else {
      const key = `${cut.section}\n${cut.line}`;
      words.set(key, [...(words.get(key) ?? []), [cut.from, cut.to]]);
    }
  }
  const out = [];
  for (const [section, set] of gone) {
    const lines = [...set].sort((a, b) => a - b);
    for (let at = 0; at < lines.length;) {
      let end = at;
      while (lines[end + 1] === lines[end] + 1) end += 1;
      out.push(lines[at] === lines[end] ? { section, line: lines[at] } : { section, line: lines[at], through: lines[end] });
      at = end + 1;
    }
  }
  for (const [key, ranges] of words) {
    const [section, line] = key.split('\n');
    if (gone.get(section)?.has(Number(line))) continue;
    const merged = [];
    for (const [from, to] of ranges.sort((a, b) => a[0] - b[0] || a[1] - b[1])) {
      const last = merged.at(-1);
      if (last && from <= last[1]) last[1] = Math.max(last[1], to);
      else merged.push([from, to]);
    }
    for (const [from, to] of merged) out.push({ section, line: Number(line), from, to });
  }
  const order = (cut) => CUTTABLE.indexOf(cut.section);
  return out.sort((a, b) => order(a) - order(b) || a.line - b.line || (a.from ?? -1) - (b.from ?? -1));
}

/** The cuts with the lines `line` to `through` of `section` cut whole. */
export const withLines = (cuts, section, line, through = line) => normalize([...cuts, { section, line, through }]);

/** The cuts with the characters `from` to `to` (not including it) of a line cut. */
export const withWords = (cuts, section, line, from, to) => normalize([...cuts, { section, line, from, to }]);

/** The edit list reportd takes: the cuts, and the answers when the person wrote them anew. */
export const editsOf = (cuts, answers = null) => (answers ? { cuts, answers } : { cuts });

/** The lines of `section` that are cut whole, by their number in the draft. */
function goneOf(cuts, section) {
  const gone = new Set();
  for (const cut of cuts) {
    if (cut.section === section && cut.from === undefined) for (let at = cut.line; at <= (cut.through ?? cut.line); at += 1) gone.add(at);
  }
  return gone;
}

/** The ranges of words cut of the line `line` of `section`: `[[from, to]]`. */
export const rangesOf = (cuts, section, line) => cuts.filter((cut) => cut.section === section && cut.line === line && cut.from !== undefined).map((cut) => [cut.from, cut.to]);

/** The number in the draft of the `at`th line that is left of `section`, which is the line as it was first drafted; one past the last line when `at` is past the lines that are left. */
export function drafted(cuts, section, at) {
  const gone = goneOf(cuts, section);
  let seen = -1;
  for (let line = 0; ; line += 1) {
    if (!gone.has(line)) seen += 1;
    if (seen === at) return line;
  }
}

/** `line` as it reads when the characters of `ranges` are cut. */
export function shownLine(line, ranges) {
  const chars = [...line];
  let at = 0;
  let out = '';
  for (const [from, to] of ranges) {
    out += chars.slice(at, from).join('') + CUT;
    at = to;
  }
  return out + chars.slice(at).join('');
}

/**
 * The place `shown` of a line that has `[cut]` in it, in code points, as a
 * place in the line as drafted. A place inside a `[cut]` is the start of
 * the words it stands for when it begins a selection, and their end when it
 * ends one.
 */
function backTo(ranges, shown, ends) {
  let shift = 0;
  for (const [from, to] of ranges) {
    const start = from - shift;
    if (shown <= start) break;
    if (shown < start + CUT_LENGTH) return ends ? to : from;
    shift += to - from - CUT_LENGTH;
  }
  return shown + shift;
}

// ---- what was selected ------------------------------------------------------

/**
 * What a selection of words inside one row cuts, as a cut `{section, line,
 * from, to}` of the drafted line, or null: the row is not one the system
 * gathered, its text is not what the cuts so far make of the drafted line
 * (redaction changed it), or nothing is left selected. `from` and `to` are
 * the UTF-16 offsets in the row's text. `original` is `gatheredOf` the first
 * draft.
 */
export function wordsPick({ cuts, original }, row, from, to) {
  if (row.kind !== 'gathered') return null;
  const line = drafted(cuts, row.section, row.at);
  const source = original.get(row.section)?.[line];
  if (source === undefined) return null;
  const ranges = rangesOf(cuts, row.section, line);
  if (shownLine(source, ranges) !== row.text) return null;
  const start = backTo(ranges, pointsBefore(row.text, from), false);
  const end = backTo(ranges, pointsBefore(row.text, to), true);
  return start < end ? { section: row.section, line, from: start, to: end } : null;
}

/**
 * What a selection of several rows cuts, as a run of whole lines `{section,
 * line, through}`, or null when a row is not one the system gathered or the
 * rows are of two parts.
 */
export function linesPick({ cuts, original }, rows) {
  const [first] = rows;
  if (!first || rows.some((row) => row.kind !== 'gathered' || row.section !== first.section)) return null;
  const line = drafted(cuts, first.section, Math.min(...rows.map((row) => row.at)));
  const through = drafted(cuts, first.section, Math.max(...rows.map((row) => row.at)));
  return through < (original.get(first.section)?.length ?? 0) ? { section: first.section, line, through } : null;
}
