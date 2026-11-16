// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The text of a report as rows that know their part and number, the list of
// cuts reportd takes (against the lines as first drafted), and the way a place
// in the shown text goes back to the drafted line and characters. No DOM.

/** The parts the system gathered: they can be cut and never typed into. */
export const CUTTABLE = ['crash', 'failed-start', 'log', 'start-log'];

/** What a cut run of words reads as in the text. */
export const CUT = '[cut]';

const CUT_LENGTH = [...CUT].length;
const MARKS = /\[(?:user|host|IPv4|IPv6|MAC|email|Wi-Fi|serial|machine|cut)\]/g;

/** The code points before the UTF-16 offset `at`: reportd counts characters so. */
const pointsBefore = (text, at) => [...text.slice(0, at)].length;

/** A line as pieces `[{text, mark}]`: `mark` is `taken` for what redaction took out, `cut` for words the person cut, null for the rest. */
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

/** The rows of an answer's text: `{n, text, kind, section, at}`; `kind` is `head`, `heading`, `typed`, `gathered` or `plain`, `at` the line's number in its part now. */
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

/** The lines the system gathered, as the first draft has them: a Map of the part's ID to its lines. */
export function gatheredOf(first) {
  const lines = new Map();
  for (const row of rowsOf(first)) {
    if (row.kind === 'gathered') lines.set(row.section, [...(lines.get(row.section) ?? []), row.text]);
  }
  return lines;
}

// ---- the list of cuts ------------------------------------------------------

/** The cuts in order and merged: whole lines as runs, words that touch as one range, none inside a line cut whole. */
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

/** The cuts with the characters `from` to `to` (not included) of a line cut. */
export const withWords = (cuts, section, line, from, to) => normalize([...cuts, { section, line, from, to }]);

/** The edit list reportd takes: the cuts, and the answers when the person wrote them again. */
export const editsOf = (cuts, answers = null) => (answers ? { cuts, answers } : { cuts });

/** The ranges of words cut of the line `line` of `section`. */
export const rangesOf = (cuts, section, line) => cuts.filter((cut) => cut.section === section && cut.line === line && cut.from !== undefined).map((cut) => [cut.from, cut.to]);

/** The number in the first draft of the `at`th line left of `section`; one past the last when `at` is past the lines left. */
export function drafted(cuts, section, at) {
  const gone = new Set();
  for (const cut of cuts) {
    if (cut.section === section && cut.from === undefined) for (let line = cut.line; line <= (cut.through ?? cut.line); line += 1) gone.add(line);
  }
  for (let line = 0, seen = -1; ; line += 1) {
    if (!gone.has(line)) seen += 1;
    if (seen === at) return line;
  }
}

/** `line` as it reads with the characters of `ranges` cut. */
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

/** A place `shown` in a line that has `[cut]` in it, as a place in the line as drafted; inside a `[cut]` it is the start of the words when it begins a selection, their end when it ends one. */
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
 * What the words `from` to `to` (UTF-16 offsets) selected in `row` cut: `{section,
 * line, from, to}` of the drafted line, or null when the system did not gather
 * the row, its text is not what the cuts make of the drafted line (redaction
 * changed it), or nothing is selected. `original` is `gatheredOf` the first draft.
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

/** What `rows` selected cut, as a run of whole lines `{section, line, through}`, or null when one is not gathered or they are of two parts. */
export function linesPick({ cuts, original }, rows) {
  const [first] = rows;
  if (!first || rows.some((row) => row.kind !== 'gathered' || row.section !== first.section)) return null;
  const line = drafted(cuts, first.section, Math.min(...rows.map((row) => row.at)));
  const through = drafted(cuts, first.section, Math.max(...rows.map((row) => row.at)));
  return through < (original.get(first.section)?.length ?? 0) ? { section: first.section, line, through } : null;
}
