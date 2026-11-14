// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's colours and attributes (SGR): every code it reads, what
// erasing leaves behind, and that the scrollback keeps the colours of a line.

import assert from 'node:assert/strict';
import test from 'node:test';

import { COLOUR, FLAGS } from '../pages/terminal/term/screen.js';
import { CSI, cell, feed, make, scrollback } from './term-screen-helpers.mjs';

const PALETTE = COLOUR.PALETTE;
const RGB = COLOUR.RGB;

// Draws one character after `sequence` and returns its cell.
function drawn(sequence, screen = make(10, 3)) {
  feed(screen, `${CSI}0m${CSI}H${sequence}x`);
  return cell(screen, 0, 0);
}

test('a character drawn with no attributes has the default colours', () => {
  const x = drawn('');
  assert.deepEqual([x.flags, x.fg, x.bg], [0, [COLOUR.DEFAULT, 0], [COLOUR.DEFAULT, 0]]);
});

test('each of SGR 1 to 9 sets its attribute', () => {
  const expected = {
    1: FLAGS.BOLD, 2: FLAGS.DIM, 3: FLAGS.ITALIC, 4: FLAGS.UNDERLINE, 5: FLAGS.BLINK, 6: FLAGS.BLINK,
    7: FLAGS.INVERSE, 8: FLAGS.INVISIBLE, 9: FLAGS.STRIKE,
  };
  for (const [code, flag] of Object.entries(expected)) {
    assert.equal(drawn(`${CSI}${code}m`).flags, flag, `SGR ${code}`);
  }
});

test('SGR 21 is a double underline, and 4 and 21 replace each other', () => {
  assert.equal(drawn(`${CSI}21m`).flags, FLAGS.DOUBLE_UNDERLINE);
  assert.equal(drawn(`${CSI}21;4m`).flags, FLAGS.UNDERLINE);
  assert.equal(drawn(`${CSI}4;21m`).flags, FLAGS.DOUBLE_UNDERLINE);
});

test('attributes add up, and each of SGR 22 to 29 takes its own away', () => {
  const all = [1, 2, 3, 4, 5, 7, 8, 9].map(String).join(';');
  const everything = FLAGS.BOLD | FLAGS.DIM | FLAGS.ITALIC | FLAGS.UNDERLINE | FLAGS.BLINK | FLAGS.INVERSE
    | FLAGS.INVISIBLE | FLAGS.STRIKE;
  assert.equal(drawn(`${CSI}${all}m`).flags, everything);
  const off = { 22: FLAGS.BOLD | FLAGS.DIM, 23: FLAGS.ITALIC, 24: FLAGS.UNDERLINE, 25: FLAGS.BLINK, 27: FLAGS.INVERSE, 28: FLAGS.INVISIBLE, 29: FLAGS.STRIKE };
  for (const [code, flags] of Object.entries(off)) {
    assert.equal(drawn(`${CSI}${all};${code}m`).flags, everything & ~flags, `SGR ${code}`);
  }
  assert.equal(drawn(`${CSI}${all};26m`).flags, everything);
  assert.equal(drawn(`${CSI}21;24m`).flags, 0);
});

test('SGR 0, and an empty SGR, put every attribute and colour back', () => {
  assert.equal(drawn(`${CSI}1;4;31;42m${CSI}0m`).flags, 0);
  const x = drawn(`${CSI}1;4;31;42m${CSI}m`);
  assert.deepEqual([x.flags, x.fg[0], x.bg[0]], [0, COLOUR.DEFAULT, COLOUR.DEFAULT]);
  const middle = drawn(`${CSI}31;0;1m`);
  assert.deepEqual([middle.flags, middle.fg[0]], [FLAGS.BOLD, COLOUR.DEFAULT]);
});

test('SGR 30 to 37 and 90 to 97 set the foreground from the first 16 colours', () => {
  for (let i = 0; i < 8; i++) {
    assert.deepEqual(drawn(`${CSI}${30 + i}m`).fg, [PALETTE, i]);
    assert.deepEqual(drawn(`${CSI}${90 + i}m`).fg, [PALETTE, 8 + i]);
  }
});

test('SGR 40 to 47 and 100 to 107 set the background from the first 16 colours', () => {
  for (let i = 0; i < 8; i++) {
    assert.deepEqual(drawn(`${CSI}${40 + i}m`).bg, [PALETTE, i]);
    assert.deepEqual(drawn(`${CSI}${100 + i}m`).bg, [PALETTE, 8 + i]);
  }
});

test('SGR 39 and 49 put back the default foreground and background alone', () => {
  const x = drawn(`${CSI}31;42;1m${CSI}39m`);
  assert.deepEqual([x.fg[0], x.bg, x.flags], [COLOUR.DEFAULT, [PALETTE, 2], FLAGS.BOLD]);
  const y = drawn(`${CSI}31;42m${CSI}49m`);
  assert.deepEqual([y.fg, y.bg[0]], [[PALETTE, 1], COLOUR.DEFAULT]);
});

test('SGR 38;5 and 48;5 pick one of the 256 colours', () => {
  assert.deepEqual(drawn(`${CSI}38;5;196m`).fg, [PALETTE, 196]);
  assert.deepEqual(drawn(`${CSI}48;5;0m`).bg, [PALETTE, 0]);
  assert.deepEqual(drawn(`${CSI}38;5;255m`).fg, [PALETTE, 255]);
});

test('SGR 38;2 and 48;2 pick a colour by its red, green and blue', () => {
  assert.deepEqual(drawn(`${CSI}38;2;18;52;86m`).fg, [RGB, 0x123456]);
  assert.deepEqual(drawn(`${CSI}48;2;255;255;255m`).bg, [RGB, 0xffffff]);
  assert.deepEqual(drawn(`${CSI}38;2;0;0;0m`).fg, [RGB, 0]);
});

test('the parameters after a colour are read as their own', () => {
  const x = drawn(`${CSI}38;5;1;1;4m`);
  assert.deepEqual([x.fg, x.flags], [[PALETTE, 1], FLAGS.BOLD | FLAGS.UNDERLINE]);
  const y = drawn(`${CSI}1;48;2;1;2;3;3m`);
  assert.deepEqual([y.bg, y.flags], [[RGB, 0x010203], FLAGS.BOLD | FLAGS.ITALIC]);
  const z = drawn(`${CSI}31;38;5;2;48;5;3m`);
  assert.deepEqual([z.fg, z.bg], [[PALETTE, 2], [PALETTE, 3]]);
});

test('a number out of range changes no colour, and the parameters after it are read as their own', () => {
  const palette = drawn(`${CSI}38;5;256;1m`);
  assert.deepEqual([palette.fg[0], palette.flags], [COLOUR.DEFAULT, FLAGS.BOLD]);
  const red = drawn(`${CSI}38;2;300;0;0;1m`);
  assert.deepEqual([red.fg[0], red.flags], [COLOUR.DEFAULT, FLAGS.BOLD]);
});

test('a colour with too few parameters, or of a kind this terminal does not know, drops the rest', () => {
  assert.deepEqual(drawn(`${CSI}38;2;1;2m`).fg[0], COLOUR.DEFAULT);
  assert.deepEqual(drawn(`${CSI}38;5m`).fg[0], COLOUR.DEFAULT);
  assert.deepEqual(drawn(`${CSI}38m`).fg[0], COLOUR.DEFAULT);
  const unknown = drawn(`${CSI}38;9;1;4m`);
  assert.deepEqual([unknown.fg[0], unknown.flags], [COLOUR.DEFAULT, 0]);
  const kept = drawn(`${CSI}31;38;5;999m`);
  assert.deepEqual(kept.fg, [PALETTE, 1]);
  assert.deepEqual(drawn(`${CSI}1;38;2;1;2m`).flags, FLAGS.BOLD);
});

test('codes this terminal does not know are skipped', () => {
  assert.deepEqual(drawn(`${CSI}10;53;58;99;1m`).flags, FLAGS.BOLD);
  assert.deepEqual(drawn(`${CSI}1;;3m`).flags, FLAGS.ITALIC);
});

test('a sequence with a colon in it is ignored whole', () => {
  const x = drawn(`${CSI}1m${CSI}38:2::1:2:3m`);
  assert.deepEqual([x.fg[0], x.flags], [COLOUR.DEFAULT, FLAGS.BOLD]);
});

test('both cells of a wide character carry the attributes', () => {
  const screen = make(6, 2);
  feed(screen, `${CSI}1;31;44m中`);
  for (const x of [0, 1]) {
    const part = cell(screen, x, 0);
    assert.deepEqual([part.flags, part.fg, part.bg], [FLAGS.BOLD, [PALETTE, 1], [PALETTE, 4]], `cell ${x}`);
  }
});

test('erasing leaves the background colour and nothing else', () => {
  const screen = make(6, 3);
  feed(screen, `${CSI}1;4;31;44mabcdef${CSI}H${CSI}K`);
  const blank = cell(screen, 3, 0);
  assert.deepEqual([blank.text, blank.flags, blank.fg, blank.bg], [' ', 0, [COLOUR.DEFAULT, 0], [PALETTE, 4]]);
  feed(screen, `${CSI}0m${CSI}2J`);
  assert.deepEqual(cell(screen, 3, 0).bg, [COLOUR.DEFAULT, 0]);
});

test('ED, EL, ECH, ICH, DCH, IL, DL and scrolling fill with the background colour', () => {
  const background = [PALETTE, 2];
  const check = (sequence, x, y, label) => {
    const screen = make(6, 3);
    feed(screen, 'abcdef\r\nghijkl\r\nmnopqr', `${CSI}42m${CSI}2;3H${sequence}`);
    assert.deepEqual(cell(screen, x, y).bg, background, label);
  };
  check(`${CSI}J`, 4, 1, 'ED 0');
  check(`${CSI}1J`, 0, 0, 'ED 1');
  check(`${CSI}2J`, 5, 2, 'ED 2');
  check(`${CSI}K`, 5, 1, 'EL');
  check(`${CSI}2X`, 3, 1, 'ECH');
  check(`${CSI}2@`, 3, 1, 'ICH');
  check(`${CSI}2P`, 5, 1, 'DCH');
  check(`${CSI}L`, 0, 1, 'IL');
  check(`${CSI}M`, 5, 2, 'DL');
  check(`${CSI}S`, 0, 2, 'SU');
  check(`${CSI}T`, 0, 0, 'SD');
  check('\n\n', 0, 2, 'LF');
});

test('what scrolls into the scrollback keeps its colours, and a plain line has none to keep', () => {
  const screen = make(6, 2, { scrollback: 5 });
  feed(screen, `${CSI}1;31;44mred${CSI}0m\r\nplain\r\nlast\r\nx`);
  const coloured = screen.line(0);
  assert.equal(coloured.length, 3);
  const second = coloured.cells.slice(coloured.offset + 3, coloured.offset + 6);
  assert.deepEqual([...second].slice(1), [(PALETTE << 24) | 1, (PALETTE << 24) | 4]);
  assert.equal(second[0] >>> 23, FLAGS.BOLD);
  assert.deepEqual(scrollback(screen), ['red', 'plain']);
  const plain = screen.line(1);
  assert.deepEqual([...plain.cells.slice(0, 3)], [0x70, 0, 0]);
  assert.equal(screen.ring[(screen.ringStart + 1) % screen.scrollbackLimit].colours, null);
});

test('the attributes stay while the cursor moves and the screen scrolls', () => {
  const screen = make(6, 2);
  feed(screen, `${CSI}32m${CSI}2;3H\n\nx`);
  assert.deepEqual(cell(screen, 2, 1).fg, [PALETTE, 2]);
});
