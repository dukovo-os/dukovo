// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a row of the screen looks like, as plain values: the runs of cells
// that share a look, each with its text, the classes term.css gives it and
// the colours the 256-colour palette and true colour need, which a class
// cannot name (they go on the span's style, through the CSSOM). render.js
// makes spans of them.
//
// Classes: f0 to f15 and g0 to g15 the palette's first 16 as foreground and
// background; b bold, m dim, i italic, u underline, s strike, v invisible;
// fb and bg the theme's own colours when inverse swaps defaults; w a wide
// character, which takes two cells whatever the font's own width; sel a
// selected cell; cur the cursor.

import {
  COLOUR, FLAGS, cellFlags, cellWidth, colourKind, colourValue,
} from './term/screen.js';

const LEVELS = [0, 95, 135, 175, 215, 255];
const hex = (n) => `#${n.toString(16).padStart(6, '0')}`;

/** The colour of palette number `n` from 16 to 255, as '#rrggbb': the 6x6x6 cube, then 24 greys. */
export function paletteColour(n) {
  if (n >= 232) {
    const level = 8 + 10 * (n - 232);
    return hex((level << 16) | (level << 8) | level);
  }
  const k = n - 16;
  return hex((LEVELS[Math.floor(k / 36) % 6] << 16) | (LEVELS[Math.floor(k / 6) % 6] << 8) | LEVELS[k % 6]);
}

// The colour word as `{ cls, color }`: a class for the 16 the theme owns, a colour for the rest, nothing for the default.
function paint(word, prefix) {
  const kind = colourKind(word);
  const value = colourValue(word);
  if (kind === COLOUR.PALETTE) return value < 16 ? { cls: `${prefix}${value}`, color: '' } : { cls: '', color: paletteColour(value) };
  if (kind === COLOUR.RGB) return { cls: '', color: hex(value) };
  return { cls: '', color: '' };
}

const FLAG_CLASSES = [[FLAGS.BOLD, 'b'], [FLAGS.DIM, 'm'], [FLAGS.ITALIC, 'i'], [FLAGS.UNDERLINE, 'u'], [FLAGS.DOUBLE_UNDERLINE, 'u'], [FLAGS.STRIKE, 's'], [FLAGS.INVISIBLE, 'v']];

// The look of one cell: classes, foreground and background colours.
function lookOf(flags, fgWord, bgWord) {
  const classes = FLAG_CLASSES.filter(([flag]) => flags & flag).map(([, name]) => name);
  let fg = paint(fgWord, 'f');
  let bg = paint(bgWord, 'g');
  if (flags & FLAGS.INVERSE) {
    // Swapped, a default stands for the theme's other colour.
    const swapFg = colourKind(bgWord) === COLOUR.DEFAULT ? { cls: 'fb', color: '' } : paint(bgWord, 'f');
    const swapBg = colourKind(fgWord) === COLOUR.DEFAULT ? { cls: 'bf', color: '' } : paint(fgWord, 'g');
    fg = swapFg;
    bg = swapBg;
  }
  classes.push(fg.cls, bg.cls);
  return { cls: classes.filter(Boolean).join(' '), color: fg.color, background: bg.color };
}

/**
 * The runs of line `number` of `screen` (see Screen.line), left to right:
 * `{ text, cls, color, background }`. `span` is the columns of this line the
 * selection covers (`{from, to}`, which select.js's `selectionSpan` gives) or
 * null; `cursor` is the column of the cursor in this line or -1, and
 * `cursorClass` the classes it gets. Blank cells of the default look at the
 * end of the line are left out.
 */
export function rowRuns(screen, number, { span = null, cursor = -1, cursorClass = 'cur' } = {}) {
  const line = screen.line(number);
  if (!line) return [];
  const runs = [];
  let last = null;
  for (let col = 0; col < screen.cols; col++) {
    const at = line.offset + 3 * col;
    const inside = col < line.length;
    const word = inside ? line.cells[at] : 0;
    if (cellWidth(word) === 0) continue;
    const wide = cellWidth(word) === 2;
    const look = lookOf(cellFlags(word), inside ? line.cells[at + 1] : 0, inside ? line.cells[at + 2] : 0);
    const marks = [look.cls, wide ? 'w' : '', span && col >= span.from && col < span.to ? 'sel' : '', col === cursor ? cursorClass : ''];
    const cls = marks.filter(Boolean).join(' ');
    const text = screen.textOf(word);
    // Wide characters and the cursor stand alone; the rest join the run before when their look is the same.
    if (last && !wide && col !== cursor && last.cls === cls && last.color === look.color && last.background === look.background && !last.alone) {
      last.text += text;
    } else {
      last = { text, cls, color: look.color, background: look.background, alone: wide || col === cursor };
      runs.push(last);
    }
  }
  // The blanks that end the line, when they have no look of their own, are not drawn.
  while (runs.length > 0) {
    const end = runs.at(-1);
    if (end.cls !== '' || end.color !== '' || end.background !== '') break;
    end.text = end.text.trimEnd();
    if (end.text !== '') break;
    runs.pop();
  }
  return runs.map(({ text, cls, color, background }) => ({ text, cls, color, background }));
}
