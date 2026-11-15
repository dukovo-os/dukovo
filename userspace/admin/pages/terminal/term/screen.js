// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's screen: a grid of character cells and what the bytes a
// program writes do to it, as an xterm does and as the machine's terminfo
// entry `xterm-256color` says programs may count on. It has no DOM: the
// renderer reads the grid, the keys and the mouse read the modes, and the
// tests read both.
//
// The grid. Each cell is three 32-bit words, in a typed array for each
// screen (the main one and the alternate one):
//
//   word 0   bits 0 to 20   the code point, or CLUSTER_BASE plus the number of
//                           a cluster: a character with the marks that join
//                           it, kept once in `clusters` (0 is a blank)
//            bits 21 to 22  NARROW (one cell), WIDE (the left cell of a
//                           character two cells wide) or TAIL (the right one)
//            bits 23 to 31  the FLAGS, one bit each
//   word 1   the foreground colour: its kind in bits 24 and 25 (DEFAULT,
//            PALETTE, RGB) and its value in the 24 bits below
//   word 2   the background colour, the same
//
// A zeroed cell is a blank of the default colours, so a new array is a clear
// screen. `cellWidth`, `cellFlags`, `colourKind` and `colourValue` read a
// word; `Screen.textOf` gives a cell's text.
//
// Lines. Lines that scroll off the top of the main screen go to the
// scrollback, a ring of `scrollbackLimit` lines kept compactly (trailing
// blanks cut, no colour words for a line of the default colours). A line has
// a number that stays with its text as the screen scrolls: `scrolled + y` for
// the screen's row y, and below `scrolled` the scrollback, newest first. A
// selection can keep a line number, and `epoch` changes when a resize, a
// reset or a change of screen makes the numbers mean something else.
//
// What the renderer reads after `write`: `dirty` (1 for each row whose cells
// changed, until `clearDirty`), the cursor (`x`, `y`, `cursorVisible`,
// `cursorShape`, `cursorBlink`), `title`, and `line(n)` for a row's cells.
// What the keys and the mouse read: `applicationCursor`, `applicationKeypad`,
// `mouseTracking` (0, 1000 or 1002), `mouseSgr` and `bracketedPaste`.
//
// If the screen misbehaves, replay the bytes the program wrote into
// `new Screen(...).write(...)` in a Node test: no other state matters. The
// fixtures in tests/term-fixtures/ do that for real programs.

import { Parser } from './parser.js';
import { charWidth } from './width.js';

export const MAX_COLS = 500;
export const MAX_ROWS = 300;
export const MAX_SCROLLBACK = 50000;

// The words in a cell, and the layout of the first.
const CELL = 3;
const CODE_POINT = 0x1fffff;
const KIND_SHIFT = 21;
const FLAGS_SHIFT = 23;
const NARROW = 0;
const WIDE = 1;
const TAIL = 2;
const CLUSTER_BASE = 0x110000;
const MAX_CLUSTERS = 0xffff;
const MAX_CLUSTER_UNITS = 16;
const MAX_TITLE = 64;
const MAX_REPLY = 4096;
const NO_REPLY = new Uint8Array(0);

// The answers to the questions a program asks the terminal: what xterm
// says when it is set to be a VT100 with video attributes (DA1), and as
// its patch 277, the first with the mouse encoding of mode 1006 (DA2).
const DEVICE_ATTRIBUTES = `\x1b[?1;2c`;
const SECONDARY_ATTRIBUTES = `\x1b[>0;277;0c`;

/** The attributes SGR sets, each a bit of a cell's flags. */
export const FLAGS = Object.freeze({
  BOLD: 1,
  DIM: 2,
  ITALIC: 4,
  UNDERLINE: 8,
  BLINK: 16,
  INVERSE: 32,
  INVISIBLE: 64,
  STRIKE: 128,
  DOUBLE_UNDERLINE: 256,
});

/** The kinds of colour: the theme's own, one of the 256 of the palette, or red, green and blue. */
export const COLOUR = Object.freeze({ DEFAULT: 0, PALETTE: 1, RGB: 2 });

/** The width in cells of the character whose cell has the word 0 `word`: 1, 2 or 0 for the right half. */
export function cellWidth(word) {
  return [1, 2, 0][(word >>> KIND_SHIFT) & 3];
}

/** The FLAGS of a cell, from its word 0. */
export function cellFlags(word) {
  return word >>> FLAGS_SHIFT;
}

/** The kind of a colour word: COLOUR.DEFAULT, PALETTE or RGB. */
export function colourKind(word) {
  return word >>> 24;
}

/** The palette number, or 0xRRGGBB, of a colour word. */
export function colourValue(word) {
  return word & 0xffffff;
}

// What a row's mark in `Grid.wrapped` says: 0 the row ends a line, WRAPPED
// it runs into the next, WRAPPED_PADDED it does too, but its last cell is
// a blank left because a wide character did not fit there.
const WRAPPED = 1;
const WRAPPED_PADDED = 2;

// A screen's cells, which rows continue on the next (`wrapped`), and the
// cursor DECSC saved there.
class Grid {
  constructor(cols, rows) {
    this.cells = new Uint32Array(cols * rows * CELL);
    this.wrapped = new Uint8Array(rows);
    this.saved = null;
  }
}

const clamp = (value, low, high) => (Number.isFinite(value) ? Math.min(Math.max(Math.trunc(value), low), high) : low);

export class Screen {
  /**
   * Makes a screen of `cols` by `rows` (at most MAX_COLS and MAX_ROWS) that
   * keeps `scrollback` lines. `onBell()` is called for each BEL, and
   * `onTitle(title)` when the program changes the title.
   */
  constructor({ cols = 80, rows = 24, scrollback = 10000, onBell = null, onTitle = null } = {}) {
    this.cols = clamp(cols, 2, MAX_COLS);
    this.rows = clamp(rows, 1, MAX_ROWS);
    this.onBell = onBell;
    this.onTitle = onTitle;
    this.main = new Grid(this.cols, this.rows);
    this.alt = new Grid(this.cols, this.rows);
    this.grid = this.main;
    this.alternate = false;
    this.dirty = new Uint8Array(this.rows).fill(1);
    this.ring = [];
    this.ringStart = 0;
    this.ringCount = 0;
    this.scrollbackLimit = clamp(scrollback, 0, MAX_SCROLLBACK);
    this.scrolled = 0;
    this.epoch = 0;
    this.clusters = [];
    this.clusterIds = new Map();
    this.title = '';
    this.parser = new Parser(this);
    this.#resetState();
  }

  // What a reset of the terminal (RIS) puts back, apart from the cells.
  #resetState() {
    this.x = 0;
    this.y = 0;
    this.pendingWrap = false;
    this.top = 0;
    this.bottom = this.rows - 1;
    this.flags = 0;
    this.fg = 0;
    this.bg = 0;
    this.autoWrap = true;
    this.originMode = false;
    this.insertMode = false;
    this.applicationCursor = false;
    this.applicationKeypad = false;
    this.bracketedPaste = false;
    this.mouseTracking = 0;
    this.mouseSgr = false;
    this.cursorVisible = true;
    this.cursorShape = 'block';
    this.cursorBlink = true;
    this.charsets = ['B', 'B'];
    this.shifted = 0;
    this.lastPrinted = 0;
    this.replyText = '';
    this.#defaultTabs(0);
  }

  // Tab stops every 8 columns, from column `from` on.
  #defaultTabs(from) {
    if (from === 0) this.tabs = new Uint8Array(this.cols);
    for (let column = Math.max(from, 1); column < this.cols; column++) this.tabs[column] = column % 8 === 0 ? 1 : 0;
  }

  /** Marks every row as unchanged: the renderer calls it after it has drawn. */
  clearDirty() {
    this.dirty.fill(0);
  }

  /** The number of the oldest line there is: the first of the scrollback, or of the screen on the alternate one. */
  get firstLine() {
    return this.alternate ? this.scrolled : this.scrolled - this.ringCount;
  }

  /**
   * The cells of line `number` (see the module comment): `{ cells, offset,
   * length, wrapped }` with the cell k at `cells[offset + 3 * k]` for k below
   * `length`, and every cell after it a blank; or null where there is no such
   * line. `wrapped` says the line goes on in the next. The arrays are the
   * screen's own for a row of the screen and must not be written.
   */
  line(number) {
    const row = number - this.scrolled;
    if (row >= 0) {
      if (row >= this.rows) return null;
      const { cells, wrapped } = this.grid;
      return { cells, offset: row * this.cols * CELL, length: this.#rowLength(this.grid, row), wrapped: wrapped[row] !== 0 };
    }
    const back = -row;
    if (this.alternate || back > this.ringCount) return null;
    const entry = this.ring[(this.ringStart + this.ringCount - back) % this.scrollbackLimit];
    return { cells: this.#expand(entry), offset: 0, length: entry.length, wrapped: entry.wrapped };
  }

  /** The text of the cell whose word 0 is `word`: '' for a right half, a space for a blank. */
  textOf(word) {
    if (((word >>> KIND_SHIFT) & 3) === TAIL) return '';
    const codePoint = word & CODE_POINT;
    if (codePoint === 0) return ' ';
    return codePoint >= CLUSTER_BASE ? this.clusters[codePoint - CLUSTER_BASE] : String.fromCodePoint(codePoint);
  }

  /** Changes the number of lines kept, dropping the oldest ones that no longer fit. */
  setScrollback(lines) {
    const limit = clamp(lines, 0, MAX_SCROLLBACK);
    const kept = Math.min(this.ringCount, limit);
    const ring = [];
    for (let i = this.ringCount - kept; i < this.ringCount; i++) {
      ring.push(this.ring[(this.ringStart + i) % this.scrollbackLimit]);
    }
    this.ring = ring;
    this.ringStart = 0;
    this.ringCount = kept;
    this.scrollbackLimit = limit;
  }

  /** Forgets the lines of the scrollback. */
  clearScrollback() {
    this.ring = [];
    this.ringStart = 0;
    this.ringCount = 0;
  }

  /**
   * Reads the bytes (a Uint8Array) a program wrote. Returns the bytes to send
   * back to it as its input, which are the answers to its questions (an empty
   * array when it asked none).
   */
  write(bytes) {
    this.parser.write(bytes);
    return this.#takeReply();
  }

  /**
   * Puts the terminal back as it is when new, as RIS does, and drops what
   * the parser had half read. The scrollback is kept unless `clearScrollback`.
   */
  reset({ clearScrollback = false } = {}) {
    this.parser.reset();
    this.#fullReset();
    if (clearScrollback) this.clearScrollback();
  }

  #fullReset() {
    for (const grid of [this.main, this.alt]) {
      this.#clear(grid);
      grid.saved = null;
    }
    this.grid = this.main;
    this.alternate = false;
    this.#resetState();
    this.dirty.fill(1);
    this.epoch += 1;
  }

  #reply(text) {
    if (this.replyText.length + text.length <= MAX_REPLY) this.replyText += text;
  }

  #takeReply() {
    if (this.replyText === '') return NO_REPLY;
    const reply = Uint8Array.from(this.replyText, (character) => character.charCodeAt(0));
    this.replyText = '';
    return reply;
  }

  // The parser's handler: print, control, escape, csi and osc.

  print(codePoint) {
    let cp = codePoint;
    if (cp >= 0x5f && cp <= 0x7e && this.charsets[this.shifted] === '0') cp = LINE_DRAWING[cp - 0x5f];
    const width = charWidth(cp);
    if (width === 0) {
      this.#join(cp);
      return;
    }
    if (this.pendingWrap && this.autoWrap) {
      this.#wrap();
    } else if (width === 2 && this.x === this.cols - 1) {
      // A wide character does not fit in the last cell, and there is no cell to cut it in.
      if (!this.autoWrap) return;
      this.#erase(this.y, this.x, this.cols);
      this.#wrap(true);
    }
    this.lastPrinted = cp;
    this.#put(cp, width);
  }

  control(code) {
    this.lastPrinted = 0;
    switch (code) {
      case 0x07:
        this.onBell?.();
        break;
      case 0x08:
        this.x = Math.max(this.x - 1, 0);
        this.pendingWrap = false;
        break;
      case 0x09:
        this.#tabForward(1);
        break;
      case 0x0a:
      case 0x0b:
      case 0x0c:
        this.#index();
        break;
      case 0x0d:
        this.x = 0;
        this.pendingWrap = false;
        break;
      case 0x0e:
        this.shifted = 1;
        break;
      case 0x0f:
        this.shifted = 0;
        break;
      default:
    }
  }

  escape(intermediates, final) {
    this.lastPrinted = 0;
    if (intermediates === '(' || intermediates === ')') {
      // Only ASCII and DEC's line drawing set are here; another set leaves the slot as it was.
      if (final === '0' || final === 'B') this.charsets[intermediates === '(' ? 0 : 1] = final;
      return;
    }
    if (intermediates !== '') return;
    switch (final) {
      case '7':
        this.#saveCursor();
        break;
      case '8':
        this.#restoreCursor();
        break;
      case 'c':
        this.#fullReset();
        break;
      case '=':
        this.applicationKeypad = true;
        break;
      case '>':
        this.applicationKeypad = false;
        break;
      case 'D':
        this.#index();
        break;
      case 'E':
        this.#index();
        this.x = 0;
        break;
      case 'M':
        this.#reverseIndex();
        break;
      case 'H':
        this.tabs[this.x] = 1;
        break;
      default:
    }
  }

  csi(prefix, params, intermediates, final) {
    const last = this.lastPrinted;
    this.lastPrinted = 0;
    if (intermediates !== '') {
      this.#csiWithIntermediates(prefix, params, intermediates, final);
      return;
    }
    if (prefix === '?') {
      if (final === 'h' || final === 'l') this.#setModes(params, final === 'h', true);
      return;
    }
    if (prefix === '>') {
      if (final === 'c' && !params[0]) this.#reply(SECONDARY_ATTRIBUTES);
      return;
    }
    if (prefix !== '') return;
    const n = params[0] || 1;
    switch (final) {
      case '@':
        this.#editChars(n, true);
        break;
      case 'A':
        this.#up(n);
        break;
      case 'B':
        this.#down(n);
        break;
      case 'e':
        // VPR counts from where the cursor is, but is not stopped by the
        // region's edge as CUD is: only by the screen's (the region's, with
        // origin mode on).
        this.#goTo(this.y - (this.originMode ? this.top : 0) + n, this.x);
        break;
      case 'C':
      case 'a':
        this.#moveTo(this.y, this.x + n);
        break;
      case 'D':
        this.#moveTo(this.y, this.x - n);
        break;
      case 'E':
        this.#down(n);
        this.x = 0;
        break;
      case 'F':
        this.#up(n);
        this.x = 0;
        break;
      case 'G':
      case '`':
        this.#moveTo(this.y, n - 1);
        break;
      case 'H':
      case 'f':
        this.#goTo(n - 1, (params[1] || 1) - 1);
        break;
      case 'I':
        this.#tabForward(n);
        break;
      case 'J':
        this.#eraseDisplay(params[0]);
        break;
      case 'K':
        this.#eraseLine(params[0]);
        break;
      case 'L':
        this.#editLines(n, true);
        break;
      case 'M':
        this.#editLines(n, false);
        break;
      case 'P':
        this.#editChars(n, false);
        break;
      case 'S':
        this.#scrollUp(n);
        break;
      case 'T':
        // With more than one parameter this is xterm's mouse tracking start, which is not here.
        if (params.length <= 1) this.#scrollDown(n);
        break;
      case 'X':
        this.#erase(this.y, this.x, Math.min(this.x + n, this.cols));
        this.pendingWrap = false;
        break;
      case 'Z':
        this.#tabBack(n);
        break;
      case 'b':
        this.#repeat(n, last);
        break;
      case 'c':
        if (!params[0]) this.#reply(DEVICE_ATTRIBUTES);
        break;
      case 'd':
        this.#goTo(n - 1, this.x);
        break;
      case 'g':
        this.#clearTabs(params[0] ?? 0);
        break;
      case 'h':
      case 'l':
        this.#setModes(params, final === 'h', false);
        break;
      case 'm':
        this.#setAttributes(params);
        break;
      case 'n':
        this.#report(params[0]);
        break;
      case 'r':
        this.#setRegion(n - 1, (params[1] || this.rows) - 1);
        break;
      case 's':
        // With parameters this is DECSLRM, the left and right margins, which are not here.
        if (params.length === 0) this.#saveCursor();
        break;
      case 'u':
        if (params.length === 0) this.#restoreCursor();
        break;
      default:
    }
  }

  // DECSCUSR (the cursor's shape) and DECSTR (a soft reset), the sequences
  // that have an intermediate.
  #csiWithIntermediates(prefix, params, intermediates, final) {
    if (prefix !== '') return;
    if (intermediates === ' ' && final === 'q') this.#setCursorStyle(params[0] ?? 0);
    else if (intermediates === '!' && final === 'p') this.#softReset();
  }

  osc(text) {
    this.lastPrinted = 0;
    const separator = text.indexOf(';');
    const command = separator < 0 ? text : text.slice(0, separator);
    // The clipboard (52) is refused, and hyperlinks (8) and the shell's marks
    // (133) are read and dropped, so only the title is acted on.
    if (command !== '0' && command !== '2') return;
    const title = separator < 0 ? '' : text.slice(separator + 1).replace(/\p{Cc}/gu, '');
    this.title = Array.from(title).slice(0, MAX_TITLE).join('');
    this.onTitle?.(this.title);
  }

  // Writes one character at the cursor and moves it on, to the last column
  // and a wait to wrap if the character ended the row.
  #put(cp, width) {
    const { cols, grid } = this;
    if (this.insertMode) this.#insertCells(this.x, width);
    this.#clearHalves(this.y, this.x, width);
    const at = (this.y * cols + this.x) * CELL;
    const flags = this.flags << FLAGS_SHIFT;
    grid.cells[at] = (width === 2 ? cp | (WIDE << KIND_SHIFT) : cp) | flags;
    grid.cells[at + 1] = this.fg;
    grid.cells[at + 2] = this.bg;
    if (width === 2) {
      grid.cells[at + 3] = (TAIL << KIND_SHIFT) | flags;
      grid.cells[at + 4] = this.fg;
      grid.cells[at + 5] = this.bg;
    }
    this.dirty[this.y] = 1;
    this.x += width;
    if (this.x >= cols) {
      this.x = cols - 1;
      this.pendingWrap = true;
    }
  }

  // Joins a mark that takes no cell to the character before the cursor. At
  // the start of a row there is none, and the mark is dropped.
  #join(cp) {
    const x = this.pendingWrap ? this.x : this.x - 1;
    if (x < 0) return;
    const { cells } = this.grid;
    let at = (this.y * this.cols + x) * CELL;
    if (((cells[at] >>> KIND_SHIFT) & 3) === TAIL) at -= CELL;
    const word = cells[at];
    const base = this.textOf(word);
    if (base.length >= MAX_CLUSTER_UNITS) return;
    const text = base + String.fromCodePoint(cp);
    let id = this.clusterIds.get(text);
    if (id === undefined) {
      if (this.clusters.length >= MAX_CLUSTERS) return;
      id = this.clusters.length;
      this.clusters.push(text);
      this.clusterIds.set(text, id);
    }
    cells[at] = (word & ~CODE_POINT) | (CLUSTER_BASE + id);
    this.dirty[this.y] = 1;
  }

  // Puts the cursor at the start of the next row, which the current one
  // runs into, scrolling at the bottom of the scroll region.
  #wrap(padded = false) {
    this.grid.wrapped[this.y] = padded ? WRAPPED_PADDED : WRAPPED;
    this.#index();
    this.x = 0;
    this.pendingWrap = false;
  }

  // Moves the cursor down a row, scrolling the region when it is on its last row.
  #index() {
    if (this.y === this.bottom) this.#scrollUp(1);
    else if (this.y < this.rows - 1) this.y += 1;
    this.pendingWrap = false;
  }

  // Writing over half of a wide character would leave the other half alone
  // in its cell, which nothing could draw: that half is blanked.
  #clearHalves(y, x, count) {
    const { cells } = this.grid;
    const row = y * this.cols * CELL;
    if (x > 0 && ((cells[row + x * CELL] >>> KIND_SHIFT) & 3) === TAIL) this.#blank(row + (x - 1) * CELL);
    const last = x + count - 1;
    if (last + 1 < this.cols && ((cells[row + last * CELL] >>> KIND_SHIFT) & 3) === WIDE) {
      this.#blank(row + (last + 1) * CELL);
    }
  }

  // A blank in the colours erasing leaves: the background the program has set.
  #blank(at) {
    const { cells } = this.grid;
    cells[at] = 0;
    cells[at + 1] = 0;
    cells[at + 2] = this.bg;
  }

  // Blanks cells `from` up to but not including `to` of row `y`, whatever
  // they were.
  #blankRange(y, from, to) {
    const row = y * this.cols * CELL;
    for (let at = row + from * CELL; at < row + to * CELL; at += CELL) this.#blank(at);
    this.dirty[y] = 1;
  }

  // Erases cells `from` up to but not including `to` of row `y`. A row
  // erased to its end no longer runs into the next.
  #erase(y, from, to) {
    this.#clearHalves(y, from, to - from);
    this.#blankRange(y, from, to);
    if (to === this.cols) this.grid.wrapped[y] = 0;
  }

  // Erases rows `from` up to but not including `to`.
  #eraseRows(from, to) {
    for (let y = from; y < to; y++) this.#erase(y, 0, this.cols);
  }

  // Makes room for `count` cells at column `x` of the cursor's row: what
  // was there moves right, and what is pushed off the end is lost.
  #insertCells(x, count) {
    const { cells } = this.grid;
    const row = this.y * this.cols * CELL;
    const moved = Math.min(count, this.cols - x);
    // The cell at `x` moves away from the character it belonged to, and so
    // does the one at the end that is pushed off.
    if (x > 0 && ((cells[row + x * CELL] >>> KIND_SHIFT) & 3) === TAIL) {
      this.#blank(row + (x - 1) * CELL);
      this.#blank(row + x * CELL);
    }
    this.#clearHalves(this.y, this.cols - moved, moved);
    cells.copyWithin(row + (x + moved) * CELL, row + x * CELL, row + (this.cols - moved) * CELL);
    this.#blankRange(this.y, x, x + moved);
  }

  // Removes `count` cells at column `x` of the cursor's row: what was after
  // them moves left, and blanks fill the end.
  #deleteCells(x, count) {
    const { cells } = this.grid;
    const row = this.y * this.cols * CELL;
    const removed = Math.min(count, this.cols - x);
    this.#clearHalves(this.y, x, removed);
    cells.copyWithin(row + x * CELL, row + (x + removed) * CELL, row + this.cols * CELL);
    this.#blankRange(this.y, this.cols - removed, this.cols);
  }

  // Scrolls the region up by `count` rows, as a line feed on its last row
  // does. Rows that leave the top of the whole main screen are kept.
  #scrollUp(count) {
    const { top, bottom } = this;
    const n = Math.min(count, bottom - top + 1);
    const keep = top === 0 && !this.alternate;
    if (keep) {
      for (let y = 0; y < n; y++) this.#keep(y);
      this.scrolled += n;
    }
    this.#shiftRows(top, bottom, n, -1);
    this.dirty.fill(1, top, bottom + 1);
  }

  // Moves rows `from` to `to` (inclusive) up (`direction` -1) or down (1) by
  // `n`, and blanks the `n` rows left behind.
  #shiftRows(from, to, n, direction) {
    const { cells, wrapped } = this.grid;
    const stride = this.cols * CELL;
    if (direction < 0) {
      cells.copyWithin(from * stride, (from + n) * stride, (to + 1) * stride);
      wrapped.copyWithin(from, from + n, to + 1);
      this.#eraseRows(to - n + 1, to + 1);
    } else {
      cells.copyWithin((from + n) * stride, from * stride, (to + 1 - n) * stride);
      wrapped.copyWithin(from + n, from, to + 1 - n);
      this.#eraseRows(from, from + n);
    }
  }

  /**
   * Changes the size, as the page does when the pane's box changes. The main
   * screen and its scrollback are laid out again for the new width, so no
   * text is cut: a line that was wrapped is joined and wrapped anew, and the
   * cursor stays on the same character. The alternate screen is cut or padded
   * at the bottom and right, as its program draws it again at once. The
   * scroll region is reset, and the line numbers change (`epoch`). The time it
   * takes grows with the scrollback, so the page should not ask for each size
   * a drag passes through.
   */
  resize(cols, rows) {
    const newCols = clamp(cols, 2, MAX_COLS);
    const newRows = clamp(rows, 1, MAX_ROWS);
    if (newCols === this.cols && newRows === this.rows) return;
    // While the alternate screen shows, the main screen's cursor is the one
    // saved for the way back, if any.
    const mainCursor = this.alternate ? this.main.saved : this;
    const placed = this.#reflow(newCols, newRows, mainCursor);
    this.alt = this.#cut(this.alt, newCols, newRows);
    const oldCols = this.cols;
    this.cols = newCols;
    this.rows = newRows;
    this.grid = this.alternate ? this.alt : this.main;
    this.dirty = new Uint8Array(newRows).fill(1);
    this.top = 0;
    this.bottom = newRows - 1;
    this.epoch += 1;
    this.tabs = Uint8Array.from({ length: newCols }, (_, x) => this.tabs[x] ?? 0);
    if (newCols > oldCols) this.#defaultTabs(oldCols);
    this.main.saved = this.#kept(this.main.saved);
    this.alt.saved = this.#kept(this.alt.saved);
    if (this.alternate) {
      this.x = Math.min(this.x, newCols - 1);
      this.y = Math.min(this.y, newRows - 1);
      this.pendingWrap = false;
      if (placed) Object.assign(this.main.saved, placed);
    } else {
      Object.assign(this, placed);
    }
  }

  // A saved cursor that fits the new size.
  #kept(saved) {
    if (!saved) return null;
    return { ...saved, x: Math.min(saved.x, this.cols - 1), y: Math.min(saved.y, this.rows - 1), pendingWrap: false };
  }

  // The alternate screen's cells that fit in `cols` by `rows`, in a new grid.
  #cut(grid, cols, rows) {
    const fresh = new Grid(cols, rows);
    const width = Math.min(cols, this.cols);
    for (let y = 0; y < Math.min(rows, this.rows); y++) {
      const from = y * this.cols * CELL;
      fresh.cells.set(grid.cells.subarray(from, from + width * CELL), y * cols * CELL);
      fresh.wrapped[y] = grid.wrapped[y];
      // A wide character cut in two at the new edge is a lead with no tail.
      const last = (y * cols + width - 1) * CELL;
      if (((fresh.cells[last] >>> KIND_SHIFT) & 3) === WIDE) fresh.cells.fill(0, last, last + CELL);
    }
    fresh.saved = grid.saved;
    return fresh;
  }

  // Lays out the main screen's lines for `cols` by `rows` and keeps the
  // ones that no longer fit on the screen in the scrollback. `cursor` has
  // `x`, `y` and `pendingWrap` of the cursor on the main screen, or is null
  // when nothing says where it is. Returns where it is now, or null.
  #reflow(cols, rows, cursor) {
    const old = this.main;
    const stride = this.cols * CELL;
    // Every line of the text in order: the scrollback, then the screen down
    // to the cursor's row or the last row that has anything on it.
    const lines = [];
    for (let i = 0; i < this.ringCount; i++) {
      const entry = this.ring[(this.ringStart + i) % this.scrollbackLimit];
      lines.push({ cells: this.#expand(entry), length: entry.length, wrapped: entry.wrapped });
    }
    const first = lines.length;
    let last = this.rows - 1;
    while (last > (cursor ? cursor.y : -1) && this.#usedLength(old.cells, last * stride, this.cols) === 0) last -= 1;
    for (let y = 0; y <= last; y++) {
      lines.push({
        cells: old.cells.subarray(y * stride, (y + 1) * stride),
        length: this.#rowLength(old, y),
        wrapped: old.wrapped[y] !== 0,
      });
    }
    const placed = this.#layOut(lines, cols, cursor ? first + cursor.y : -1, cursor);
    const fresh = new Grid(cols, rows);
    fresh.saved = old.saved;
    // The last `rows` rows are the screen, or fewer when the cursor is higher.
    const { out } = placed;
    const top = Math.max(0, Math.min(out.length - rows, placed.row < 0 ? out.length : placed.row));
    this.clearScrollback();
    for (let i = Math.max(0, top - this.scrollbackLimit); i < top; i++) {
      this.#keepCells(out[i].cells, 0, out[i].length, out[i].wrapped);
    }
    this.scrolled = this.ringCount;
    for (let y = 0; y < rows && top + y < out.length; y++) {
      const row = out[top + y];
      fresh.cells.set(row.cells.subarray(0, row.length * CELL), y * cols * CELL);
      fresh.wrapped[y] = row.wrapped ? (row.length < cols ? WRAPPED_PADDED : WRAPPED) : 0;
    }
    this.main = fresh;
    return placed.row < 0 ? null : { x: placed.col, y: placed.row - top, pendingWrap: placed.pending };
  }

  // Joins the rows of `lines` that run into each other and cuts each text
  // again for `cols` columns, without cutting a wide character. Returns the
  // new rows as `out`, and where the cursor (on line `cursorLine`) is in
  // them: `row`, `col` and `pending`, with `row` -1 when there is none.
  #layOut(lines, cols, cursorLine, cursor) {
    const out = [];
    const placed = { out, row: -1, col: 0, pending: false };
    for (let from = 0; from < lines.length;) {
      let to = from;
      while (to < lines.length - 1 && lines[to].wrapped) to += 1;
      let total = 0;
      let cursorAt = -1;
      for (let i = from; i <= to; i++) {
        if (i === cursorLine) cursorAt = total + cursor.x + (cursor.pendingWrap ? 1 : 0);
        total += lines[i].length;
      }
      const length = Math.max(total, cursorAt);
      const joined = new Uint32Array(length * CELL);
      let at = 0;
      for (let i = from; i <= to; i++) {
        joined.set(lines[i].cells.subarray(0, lines[i].length * CELL), at * CELL);
        at += lines[i].length;
      }
      this.#cutLine(joined, length, cols, cursorAt, out, placed);
      from = to + 1;
    }
    return placed;
  }

  // Cuts one text of `length` cells into rows of at most `cols`.
  #cutLine(joined, length, cols, cursorAt, out, placed) {
    let start = 0;
    do {
      let end = Math.min(start + cols, length);
      // A wide character stays whole, in the next row if it does not fit here.
      if (end < length && end > start + 1 && ((joined[(end - 1) * CELL] >>> KIND_SHIFT) & 3) === WIDE) end -= 1;
      const inside = cursorAt >= start && (cursorAt < end || (end === length && cursorAt === length));
      if (inside) {
        const col = cursorAt - start;
        Object.assign(placed, { row: out.length, col: Math.min(col, cols - 1), pending: col === cols });
      }
      out.push({ cells: joined.subarray(start * CELL, end * CELL), length: end - start, wrapped: end < length });
      start = end;
    } while (start < length);
  }

  // The number of cells of row `y` of `grid` that hold text: all of a row
  // that runs into the next (but the last, where a wide character that did
  // not fit left a blank), else up to the last that is not a default blank.
  #rowLength(grid, y) {
    const mark = grid.wrapped[y];
    if (mark === WRAPPED) return this.cols;
    // The last cell is the blank a wide character left only while it is blank.
    if (mark === WRAPPED_PADDED) return grid.cells[(y * this.cols + this.cols - 1) * CELL] === 0 ? this.cols - 1 : this.cols;
    return this.#usedLength(grid.cells, y * this.cols * CELL, this.cols);
  }

  // The number of cells of the row at `row` (a word index into `cells`) up
  // to its last that is not a blank of the default colours.
  #usedLength(cells, row, cols) {
    let length = cols;
    while (length > 0) {
      const at = row + (length - 1) * CELL;
      if ((cells[at] | cells[at + 1] | cells[at + 2]) !== 0) break;
      length -= 1;
    }
    return length;
  }

  // Moves row `y` of the main screen into the scrollback.
  #keep(y) {
    const { cells } = this.grid;
    const row = y * this.cols * CELL;
    this.#keepCells(cells, row, this.#rowLength(this.grid, y), this.grid.wrapped[y] !== 0);
  }

  // Puts the `length` cells that start at word `row` of `cells` into the
  // scrollback as its newest line, and drops the oldest if it is full.
  #keepCells(cells, row, length, wrapped) {
    if (this.scrollbackLimit === 0) return;
    const chars = new Uint32Array(length);
    let colours = null;
    for (let k = 0; k < length; k++) {
      const at = row + k * CELL;
      chars[k] = cells[at];
      if ((cells[at + 1] | cells[at + 2]) !== 0) {
        colours ??= new Uint32Array(length * 2);
        colours[2 * k] = cells[at + 1];
        colours[2 * k + 1] = cells[at + 2];
      }
    }
    const entry = { chars, colours, length, wrapped };
    if (this.ringCount < this.scrollbackLimit) {
      this.ring[(this.ringStart + this.ringCount) % this.scrollbackLimit] = entry;
      this.ringCount += 1;
    } else {
      this.ring[this.ringStart] = entry;
      this.ringStart = (this.ringStart + 1) % this.scrollbackLimit;
    }
  }

  // A scrollback entry as cells of three words, like the screen's.
  #expand(entry) {
    const cells = new Uint32Array(entry.length * CELL);
    for (let k = 0; k < entry.length; k++) {
      cells[k * CELL] = entry.chars[k];
      if (entry.colours) {
        cells[k * CELL + 1] = entry.colours[2 * k];
        cells[k * CELL + 2] = entry.colours[2 * k + 1];
      }
    }
    return cells;
  }

  // Moves the cursor to row `row` and column `column`, inside the screen.
  #moveTo(row, column) {
    this.y = clamp(row, 0, this.rows - 1);
    this.x = clamp(column, 0, this.cols - 1);
    this.pendingWrap = false;
  }

  // CUP, VPA and what puts the cursor at a place the program counts from the
  // origin: with origin mode on, that is the top of the scroll region, and the
  // cursor may not leave the region.
  #goTo(row, column) {
    const first = this.originMode ? this.top : 0;
    const last = this.originMode ? this.bottom : this.rows - 1;
    this.#moveTo(clamp(first + row, first, last), column);
  }

  // Up and down stop at the scroll region's edge when the cursor is inside
  // it, and at the screen's when it is not.
  #up(count) {
    this.y = Math.max(this.y - count, this.y >= this.top ? this.top : 0);
    this.pendingWrap = false;
  }

  #down(count) {
    this.y = Math.min(this.y + count, this.y <= this.bottom ? this.bottom : this.rows - 1);
    this.pendingWrap = false;
  }

  // Moves the cursor up a row, scrolling the region down when it is on its first row.
  #reverseIndex() {
    if (this.y === this.top) this.#scrollDown(1);
    else if (this.y > 0) this.y -= 1;
    this.pendingWrap = false;
  }

  #scrollDown(count) {
    const n = Math.min(count, this.bottom - this.top + 1);
    this.#shiftRows(this.top, this.bottom, n, 1);
    this.dirty.fill(1, this.top, this.bottom + 1);
  }

  // ICH and DCH: make room for `count` cells at the cursor, or remove them.
  #editChars(count, insert) {
    if (insert) this.#insertCells(this.x, count);
    else this.#deleteCells(this.x, count);
    this.pendingWrap = false;
  }

  // IL and DL: insert `count` rows at the cursor's, or remove them, inside
  // the scroll region. The cursor goes to the first column, and nothing
  // happens when it is outside the region.
  #editLines(count, insert) {
    if (this.y < this.top || this.y > this.bottom) return;
    const n = Math.min(count, this.bottom - this.y + 1);
    this.#shiftRows(this.y, this.bottom, n, insert ? 1 : -1);
    this.dirty.fill(1, this.y, this.bottom + 1);
    this.x = 0;
    this.pendingWrap = false;
  }

  // ED. A cursor that waits to wrap is on its last cell, which these erase,
  // and the wait is over, as in xterm.
  #eraseDisplay(mode = 0) {
    if (mode <= 2) this.pendingWrap = false;
    if (mode === 0) {
      this.#erase(this.y, this.x, this.cols);
      this.#eraseRows(this.y + 1, this.rows);
    } else if (mode === 1) {
      this.#eraseRows(0, this.y);
      this.#erase(this.y, 0, this.x + 1);
    } else if (mode === 2) {
      this.#eraseRows(0, this.rows);
    } else if (mode === 3) {
      this.clearScrollback();
    }
  }

  #eraseLine(mode = 0) {
    if (mode <= 2) this.pendingWrap = false;
    if (mode === 0) this.#erase(this.y, this.x, this.cols);
    else if (mode === 1) this.#erase(this.y, 0, this.x + 1);
    else if (mode === 2) this.#erase(this.y, 0, this.cols);
  }

  // DECSTBM: rows `top` to `bottom` become the scroll region, and the cursor
  // goes home. A region of fewer than two rows is refused.
  #setRegion(top, bottom) {
    const last = Math.min(bottom, this.rows - 1);
    if (top >= last) return;
    this.top = top;
    this.bottom = last;
    this.#goTo(0, 0);
  }

  // REP: the character that was just printed, again `count` times, which is
  // bounded so that a few bytes cannot ask for a million cell writes. xterm
  // repeats only what came right before: after any control, escape or
  // sequence, and after a REP, there is nothing to repeat.
  #repeat(count, last) {
    if (last === 0) return;
    for (let i = Math.min(count, this.cols * 4); i > 0; i--) this.print(last);
    this.lastPrinted = 0;
  }

  #tabBack(count) {
    let x = this.x;
    for (let i = 0; i < count; i++) {
      do x -= 1;
      while (x > 0 && !this.tabs[x]);
    }
    this.x = Math.max(x, 0);
    this.pendingWrap = false;
  }

  // TBC: 0 clears the stop at the cursor, 3 clears every stop.
  #clearTabs(mode) {
    if (mode === 0) this.tabs[this.x] = 0;
    else if (mode === 3) this.tabs.fill(0);
  }

  // SGR: the attributes and colours that characters drawn from now on take.
  #setAttributes(params) {
    if (params.length === 0) {
      this.#resetAttributes();
      return;
    }
    for (let i = 0; i < params.length; i++) {
      const code = params[i];
      if (code === 38 || code === 48) i = this.#setExtendedColour(params, i);
      else this.#setAttribute(code);
    }
  }

  #resetAttributes() {
    this.flags = 0;
    this.fg = 0;
    this.bg = 0;
  }

  #setAttribute(code) {
    if (code === 0) {
      this.#resetAttributes();
    } else if (code >= 1 && code <= 9) {
      this.flags |= SET_FLAGS[code - 1];
      if (code === 4) this.flags &= ~FLAGS.DOUBLE_UNDERLINE;
    } else if (code === 21) {
      this.flags = (this.flags | FLAGS.DOUBLE_UNDERLINE) & ~FLAGS.UNDERLINE;
    } else if (code >= 22 && code <= 29) {
      this.flags &= ~CLEAR_FLAGS[code - 22];
    } else if (code >= 30 && code <= 37) {
      this.fg = (COLOUR.PALETTE << 24) | (code - 30);
    } else if (code === 39) {
      this.fg = 0;
    } else if (code >= 40 && code <= 47) {
      this.bg = (COLOUR.PALETTE << 24) | (code - 40);
    } else if (code === 49) {
      this.bg = 0;
    } else if (code >= 90 && code <= 97) {
      this.fg = (COLOUR.PALETTE << 24) | (code - 90 + 8);
    } else if (code >= 100 && code <= 107) {
      this.bg = (COLOUR.PALETTE << 24) | (code - 100 + 8);
    }
  }

  // 38 or 48 at `params[i]`, then 5 and a palette number, or 2 and red,
  // green and blue. Returns the index of the last parameter it used. A
  // number over 255 changes no colour but is used up, and so is the whole
  // form. Another kind of colour, or too few parameters, ends the sequence:
  // what follows could only be read as attributes it was never meant to be.
  #setExtendedColour(params, i) {
    const mode = params[i + 1];
    let colour = null;
    let last = params.length - 1;
    if (mode === 5 && i + 2 < params.length) {
      last = i + 2;
      if (params[last] <= 255) colour = (COLOUR.PALETTE << 24) | params[last];
    } else if (mode === 2 && i + 4 < params.length) {
      last = i + 4;
      const [red, green, blue] = params.slice(i + 2, i + 5);
      if (red <= 255 && green <= 255 && blue <= 255) colour = (COLOUR.RGB << 24) | (red << 16) | (green << 8) | blue;
    }
    if (colour !== null) {
      if (params[i] === 38) this.fg = colour;
      else this.bg = colour;
    }
    return last;
  }

  // SM and RM: the modes of ANSI (only insert) or, with `isPrivate`, of DEC.
  #setModes(params, on, isPrivate) {
    for (const mode of params) {
      if (isPrivate) this.#setPrivateMode(mode, on);
      else if (mode === 4) this.insertMode = on;
    }
  }

  #setPrivateMode(mode, on) {
    switch (mode) {
      case 1:
        this.applicationCursor = on;
        break;
      case 6:
        this.originMode = on;
        this.#goTo(0, 0);
        break;
      case 7:
        this.autoWrap = on;
        break;
      case 25:
        this.cursorVisible = on;
        break;
      case 47:
        this.#useAlternate(on);
        break;
      case 1047:
        if (!on && this.alternate) this.#clear(this.alt);
        this.#useAlternate(on);
        break;
      case 1048:
        if (on) this.#saveCursor();
        else this.#restoreCursor();
        break;
      case 1049:
        this.#useAlternateWithCursor(on);
        break;
      case 1000:
      case 1002:
        this.mouseTracking = on ? mode : 0;
        break;
      case 1006:
        this.mouseSgr = on;
        break;
      case 2004:
        this.bracketedPaste = on;
        break;
      default:
    }
  }

  // Shows the alternate screen, which a full-screen program draws on so
  // that the main one, and its scrollback, are there when it ends. The
  // cursor stays where it is, and so does the scroll region: it belongs to
  // the terminal, not to a screen, as in xterm.
  #useAlternate(on) {
    if (on === this.alternate) return;
    this.alternate = on;
    this.grid = on ? this.alt : this.main;
    this.dirty.fill(1);
    this.epoch += 1;
  }

  // Mode 1049: the cursor is kept for the way back, and the alternate
  // screen starts clear. As in xterm, this is done each time, also when the
  // alternate screen is shown already (it is cleared again), and turning it
  // off puts the cursor back also when the main screen is shown (to where
  // the last save put it, or home).
  #useAlternateWithCursor(on) {
    if (on) {
      this.#saveCursor();
      this.#useAlternate(true);
      this.#clear(this.alt);
      this.pendingWrap = false;
      this.dirty.fill(1);
    } else {
      this.#useAlternate(false);
      this.#restoreCursor();
    }
  }

  #clear(grid) {
    grid.cells.fill(0);
    grid.wrapped.fill(0);
  }

  // DECSC: the cursor, the attributes and the character sets, kept for each
  // screen apart.
  #saveCursor() {
    this.grid.saved = {
      x: this.x,
      y: this.y,
      pendingWrap: this.pendingWrap,
      flags: this.flags,
      fg: this.fg,
      bg: this.bg,
      originMode: this.originMode,
      charsets: [...this.charsets],
      shifted: this.shifted,
    };
  }

  // DECRC. With nothing saved the cursor goes home, and the attributes and
  // character sets go back to what a new terminal has.
  #restoreCursor() {
    const saved = this.grid.saved;
    if (!saved) {
      this.#resetAttributes();
      this.originMode = false;
      this.charsets = ['B', 'B'];
      this.shifted = 0;
      this.#moveTo(0, 0);
      return;
    }
    this.flags = saved.flags;
    this.fg = saved.fg;
    this.bg = saved.bg;
    this.originMode = saved.originMode;
    this.charsets = [...saved.charsets];
    this.shifted = saved.shifted;
    this.#moveTo(saved.originMode ? clamp(saved.y, this.top, this.bottom) : saved.y, saved.x);
    this.pendingWrap = saved.pendingWrap;
  }

  // DECSTR: what a program that has left the terminal in a strange state
  // needs undone, short of clearing the screen or moving the cursor.
  #softReset() {
    this.cursorVisible = true;
    this.insertMode = false;
    this.originMode = false;
    this.autoWrap = true;
    this.applicationCursor = false;
    this.applicationKeypad = false;
    this.top = 0;
    this.bottom = this.rows - 1;
    this.charsets = ['B', 'B'];
    this.shifted = 0;
    this.#resetAttributes();
    this.#setCursorStyle(0);
    this.grid.saved = null;
  }

  // DECSCUSR: 0 and 1 a blinking block, 2 a steady one, then underline
  // (3, 4) and bar (5, 6) the same way.
  #setCursorStyle(style) {
    if (style > 6) return;
    this.cursorShape = ['block', 'block', 'block', 'underline', 'underline', 'bar', 'bar'][style];
    this.cursorBlink = style % 2 === 1 || style === 0;
  }

  // DSR: 5 asks whether the terminal is working, and 6 where the cursor is
  // (counted from the region's top with origin mode on).
  #report(question) {
    if (question === 5) this.#reply('\x1b[0n');
    else if (question === 6) this.#reply(`\x1b[${this.y - (this.originMode ? this.top : 0) + 1};${this.x + 1}R`);
  }

  #tabForward(count) {
    let x = this.x;
    for (let i = 0; i < count; i++) {
      do x += 1;
      while (x < this.cols - 1 && !this.tabs[x]);
    }
    this.x = Math.min(x, this.cols - 1);
  }
}

// The flag each of the SGR codes 1 to 9 sets (6, rapid blink, is blink), and
// the flags each of 22 to 29 clears.
const SET_FLAGS = [
  FLAGS.BOLD, FLAGS.DIM, FLAGS.ITALIC, FLAGS.UNDERLINE, FLAGS.BLINK, FLAGS.BLINK, FLAGS.INVERSE,
  FLAGS.INVISIBLE, FLAGS.STRIKE,
];
const CLEAR_FLAGS = [
  FLAGS.BOLD | FLAGS.DIM, FLAGS.ITALIC, FLAGS.UNDERLINE | FLAGS.DOUBLE_UNDERLINE, FLAGS.BLINK, 0,
  FLAGS.INVERSE, FLAGS.INVISIBLE, FLAGS.STRIKE,
];

// The characters that DEC's special graphics set puts in place of the ASCII
// from 0x5F to 0x7E: the boxes and lines that ncurses draws with `ESC ( 0`.
const LINE_DRAWING = [
  0xa0, 0x25c6, 0x2592, 0x2409, 0x240c, 0x240d, 0x240a, 0xb0, 0xb1, 0x2424, 0x240b, 0x2518, 0x2510,
  0x250c, 0x2514, 0x253c, 0x23ba, 0x23bb, 0x2500, 0x23bc, 0x23bd, 0x251c, 0x2524, 0x2534, 0x252c,
  0x2502, 0x2264, 0x2265, 0x3c0, 0x2260, 0xa3, 0xb7,
];
