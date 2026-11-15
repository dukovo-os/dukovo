// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Selecting text on the screen and in the scrollback, and the text a
// selection copies.
//
// A position is `{ line, col }`: `line` is a line number as `Screen.line`
// has it (a row of the screen is `screen.scrolled + y`, and the scrollback
// is below that), `col` a column from 0. So a selection keeps its place
// while output scrolls, until its text leaves the scrollback (see
// `clipSelection`). A selection is `{ start, end }` with both ends in it,
// in either order. Line numbers mean nothing after `screen.epoch` changes
// (a resize, the alternate screen, a reset), and the page clears its
// selection then.
//
// What is copied is the text, not the cells: the blanks at the end of a
// row are dropped, rows that ran into each other (the program's line was
// longer than the screen) are joined without a line break, and a wide
// character is copied whole whichever of its two cells was the edge.

import { cellWidth } from './screen.js';

// The characters besides letters and digits that a double click takes into
// a word, so that a path, a host name or a URL is selected whole.
const WORD_CHARACTERS = '-./?%&#:_=+@~';

const WIDE = 2;
const TAIL = 0;

// Reads lines of `screen` once each: a line of the scrollback is built
// when asked for, so walking over it cell by cell must not ask again.
class Lines {
  #cache = new Map();

  constructor(screen) {
    this.screen = screen;
  }

  get(number) {
    if (!this.#cache.has(number)) this.#cache.set(number, this.screen.line(number));
    return this.#cache.get(number);
  }

  // Word 0 of the cell, or 0 (a blank) after the last the line holds.
  word(line, col) {
    return col < line.length ? line.cells[line.offset + 3 * col] : 0;
  }

  // Where text ends in the line: all of a line that runs on, else its width.
  end(line) {
    return line.wrapped ? line.length : this.screen.cols;
  }
}

// What kind of character a cell holds, for a double click: letters, digits
// and WORD_CHARACTERS and anything past ASCII make 'word', a blank 'blank',
// and each other character is a kind of its own, so `((` is one selection
// and `(a` is two.
function kindOf(screen, word) {
  const text = screen.textOf(word);
  if (text === ' ') return 'blank';
  const first = text.codePointAt(0);
  if (first > 0x7f || /[A-Za-z0-9]/.test(text[0]) || WORD_CHARACTERS.includes(text[0])) return 'word';
  return text[0];
}

// The cell before `pos`, or null at the start of the text: the row's last
// cell is reached from the start of the next row when the row runs on.
function before(lines, pos) {
  let { line, col } = pos;
  if (col > 0) {
    col -= 1;
  } else {
    const previous = lines.get(line - 1);
    if (!previous?.wrapped) return null;
    line -= 1;
    col = previous.length - 1;
  }
  return { line, col: lead(lines, line, col) };
}

// The cell after `pos`, or null at the end of the text.
function after(lines, pos) {
  const here = lines.get(pos.line);
  const col = pos.col + Math.max(cellWidth(lines.word(here, pos.col)), 1);
  if (col < lines.end(here)) return { line: pos.line, col };
  if (!here.wrapped || !lines.get(pos.line + 1)) return null;
  return { line: pos.line + 1, col: 0 };
}

// The column of the cell that holds the character at `col`: the left cell
// of a wide character, whichever of the two was asked for.
function lead(lines, line, col) {
  const row = lines.get(line);
  return col > 0 && col < row.length && cellWidth(lines.word(row, col)) === TAIL ? col - 1 : col;
}

/**
 * The selection a double click at `line`, `col` makes: the run of the same
 * kind of character (see `kindOf`), across rows that run into each other. It
 * is null where there is no such line.
 */
export function wordRange(screen, line, col) {
  const lines = new Lines(screen);
  if (!lines.get(line) || !Number.isInteger(col) || col < 0 || col >= screen.cols) return null;
  const start0 = { line, col: lead(lines, line, col) };
  const kind = kindOf(screen, lines.word(lines.get(line), start0.col));
  const same = (pos) => pos && kindOf(screen, lines.word(lines.get(pos.line), pos.col)) === kind;
  let start = start0;
  for (let pos = before(lines, start); same(pos); pos = before(lines, start)) start = pos;
  let end = start0;
  for (let pos = after(lines, end); same(pos); pos = after(lines, end)) end = pos;
  // The end is the last cell of a wide character, not its first.
  const last = lines.word(lines.get(end.line), end.col);
  if (cellWidth(last) === WIDE) end = { line: end.line, col: end.col + 1 };
  return { start, end };
}

/**
 * The selection a triple click on `line` makes: the whole line the program
 * wrote, from the first row that starts it to the last row that ends it.
 * It is null where there is no such line.
 */
export function lineRange(screen, line) {
  const lines = new Lines(screen);
  if (!lines.get(line)) return null;
  let first = line;
  while (lines.get(first - 1)?.wrapped) first -= 1;
  let last = line;
  while (lines.get(last).wrapped && lines.get(last + 1)) last += 1;
  return { start: { line: first, col: 0 }, end: { line: last, col: screen.cols - 1 } };
}

// The selection with its start before its end.
function ordered(selection) {
  const { start, end } = selection;
  const swap = start.line > end.line || (start.line === end.line && start.col > end.col);
  return swap ? { start: end, end: start } : { start, end };
}

/**
 * The selection cut to the lines that still exist, as it changes while
 * output scrolls the oldest ones away: null when none of it is left.
 */
export function clipSelection(screen, selection) {
  const { start, end } = ordered(selection);
  const first = screen.firstLine;
  const last = screen.scrolled + screen.rows - 1;
  if (end.line < first || start.line > last) return null;
  return {
    start: start.line < first ? { line: first, col: 0 } : start,
    end: end.line > last ? { line: last, col: screen.cols - 1 } : end,
  };
}

// The columns of `row` that the selection holds, from the first up to but
// not including the last, with a wide character in whole.
function columns(lines, selection, number) {
  const { start, end } = ordered(selection);
  if (number < start.line || number > end.line) return null;
  const row = lines.get(number);
  if (!row) return null;
  const cols = lines.screen.cols;
  let from = number === start.line ? start.col : 0;
  let to = (number === end.line ? end.col : cols - 1) + 1;
  from = Math.min(Math.max(from, 0), cols - 1);
  to = Math.min(Math.max(to, from + 1), cols);
  from = lead(lines, number, from);
  if (to < cols && cellWidth(lines.word(row, to)) === TAIL) to += 1;
  return { from, to };
}

/**
 * The columns of line `line` that `selection` covers, as `{ from, to }`
 * (the first, and the one after the last), or null when the line is not in
 * it. The renderer draws the selection's look over them. A wide character
 * is covered whole.
 */
export function selectionSpan(screen, selection, line) {
  return columns(new Lines(screen), selection, line);
}

/**
 * The text of `selection`: the cells' characters, a line break between
 * rows that end a line, none between rows that run into each other, and no
 * blanks at the end of a row that ends a line. A part of the selection
 * that is not on the screen or in the scrollback any more is left out.
 */
export function selectionText(screen, selection) {
  const lines = new Lines(screen);
  const { start, end } = ordered(selection);
  const first = Math.max(start.line, screen.firstLine);
  const last = Math.min(end.line, screen.scrolled + screen.rows - 1);
  let text = '';
  for (let number = first; number <= last; number++) {
    const row = lines.get(number);
    const { from, to } = columns(lines, selection, number);
    let part = '';
    for (let col = from; col < Math.min(to, row.length); col++) part += screen.textOf(lines.word(row, col));
    if (row.wrapped) {
      text += part;
    } else {
      text += part.trimEnd();
      if (number < last) text += '\n';
    }
  }
  return text;
}
