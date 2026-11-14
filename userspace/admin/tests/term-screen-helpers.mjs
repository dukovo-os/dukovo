// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the terminal's tests share: making a screen, feeding it text with
// escape sequences written out, and reading it back as text and as cells.

import { Screen, cellFlags, cellWidth, colourKind, colourValue } from '../pages/terminal/term/screen.js';

const encoder = new TextEncoder();

export const ESC = '\x1b';
export const CSI = '\x1b[';

/** A screen of `cols` by `rows` with `options` (scrollback, onBell, onTitle). */
export function make(cols = 10, rows = 4, options = {}) {
  return new Screen({ cols, rows, ...options });
}

/** Writes strings (as UTF-8) and byte arrays to `screen`, and returns what it answers. */
export function feed(screen, ...chunks) {
  const answers = [];
  for (const chunk of chunks) {
    const bytes = typeof chunk === 'string' ? encoder.encode(chunk) : chunk;
    answers.push(...(screen.write(bytes) ?? []));
  }
  return Uint8Array.from(answers);
}

/** The text of line `number` of `screen` (see `Screen.line`), without trailing blanks, or null. */
export function lineText(screen, number) {
  const line = screen.line(number);
  if (!line) return null;
  let text = '';
  for (let k = 0; k < screen.cols; k++) {
    text += k < line.length ? screen.textOf(line.cells[line.offset + 3 * k]) : ' ';
  }
  return text.trimEnd();
}

/** The rows of the screen as text, without trailing blanks. */
export function rows(screen) {
  return Array.from({ length: screen.rows }, (_, y) => lineText(screen, screen.scrolled + y));
}

/** The lines of the scrollback as text, oldest first. */
export function scrollback(screen) {
  return Array.from({ length: screen.ringCount }, (_, i) => lineText(screen, screen.scrolled - screen.ringCount + i));
}

/** The cell at column `x` of row `y`: its text, width, flags and colours. */
export function cell(screen, x, y) {
  const { cells, offset } = screen.line(screen.scrolled + y);
  const at = offset + 3 * x;
  return {
    text: screen.textOf(cells[at]),
    width: cellWidth(cells[at]),
    flags: cellFlags(cells[at]),
    fg: [colourKind(cells[at + 1]), colourValue(cells[at + 1])],
    bg: [colourKind(cells[at + 2]), colourValue(cells[at + 2])],
  };
}

/** The bytes of `reply` as the text they spell. */
export const spelled = (reply) => String.fromCharCode(...reply);

/** The cursor as `[x, y]`. */
export const at = (screen) => [screen.x, screen.y];
