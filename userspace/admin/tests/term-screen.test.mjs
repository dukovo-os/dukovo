// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's screen in term/screen.js: drawing text, wrapping, wide and
// joined characters, the cursor, tab stops, erasing, inserting and deleting,
// scroll regions and the scrollback.

import assert from 'node:assert/strict';
import test from 'node:test';

import { MAX_SCROLLBACK } from '../pages/terminal/term/screen.js';
import { CSI, ESC, at, cell, feed, lineText, make, rows, scrollback } from './term-screen-helpers.mjs';

// Rows 'aaaaa', 'bbbbb', 'ccccc', 'ddddd' on a screen of 5 by 4, the cursor at the origin.
function filled(options) {
  const screen = make(5, 4, options);
  feed(screen, `${CSI}H`, 'aaaaa\r\nbbbbb\r\nccccc\r\nddddd', `${CSI}H`);
  return screen;
}

test('text is drawn at the cursor, which moves along', () => {
  const screen = make(10, 3);
  feed(screen, 'abc');
  assert.deepEqual(rows(screen), ['abc', '', '']);
  assert.deepEqual(at(screen), [3, 0]);
});

test('a new screen is clear, with the cursor at the origin', () => {
  const screen = make(7, 3);
  assert.deepEqual(rows(screen), ['', '', '']);
  assert.deepEqual(at(screen), [0, 0]);
  assert.equal(cell(screen, 0, 0).width, 1);
});

test('the size is kept inside what the page may ask for', () => {
  const screen = make(9999, 9999);
  assert.deepEqual([screen.cols, screen.rows], [500, 300]);
  const small = make(0, 0);
  assert.deepEqual([small.cols, small.rows], [2, 1]);
  const odd = make(Number.NaN, 'x');
  assert.deepEqual([odd.cols, odd.rows], [2, 1]);
});

test('CR goes to the first column, and LF down a row without it', () => {
  const screen = make(10, 3);
  feed(screen, 'ab\r\ncd\nef');
  assert.deepEqual(rows(screen), ['ab', 'cd', '  ef']);
});

test('BS moves left and stops at the first column', () => {
  const screen = make(10, 3);
  feed(screen, 'abc\b\bX');
  assert.deepEqual(rows(screen)[0], 'aXc');
  feed(screen, '\r\b\b\bY');
  assert.deepEqual(rows(screen)[0], 'YXc');
});

test('VT and FF are line feeds', () => {
  const screen = make(10, 3);
  feed(screen, 'a\x0bb\x0cc');
  assert.deepEqual(rows(screen), ['a', ' b', '  c']);
});

test('BEL calls onBell, and controls with no meaning here do nothing', () => {
  let bells = 0;
  const screen = make(10, 2, { onBell: () => { bells += 1; } });
  feed(screen, 'a\x07\x07b\x00\x05\x1c\x7f');
  assert.equal(bells, 2);
  assert.deepEqual(rows(screen), ['ab', '']);
  assert.doesNotThrow(() => feed(make(10, 2), '\x07'));
});

test('text longer than a row wraps to the next', () => {
  const screen = make(5, 3);
  feed(screen, 'abcdefgh');
  assert.deepEqual(rows(screen), ['abcde', 'fgh', '']);
  assert.deepEqual(at(screen), [3, 1]);
  assert.equal(screen.line(0).wrapped, true);
  assert.equal(screen.line(1).wrapped, false);
});

test('a row filled to its end waits to wrap: the cursor stays on the last cell', () => {
  const screen = make(5, 3);
  feed(screen, 'abcde');
  assert.deepEqual(at(screen), [4, 0]);
  assert.equal(screen.pendingWrap, true);
  feed(screen, 'f');
  assert.deepEqual(at(screen), [1, 1]);
  assert.equal(screen.pendingWrap, false);
  assert.deepEqual(rows(screen), ['abcde', 'f', '']);
});

test('CR, LF, BS and cursor moves cancel a wait to wrap', () => {
  const screen = make(5, 3);
  feed(screen, 'abcde\rX');
  assert.deepEqual(rows(screen)[0], 'Xbcde');
  feed(screen, `${CSI}H`, 'abcde\nf');
  assert.deepEqual(rows(screen).slice(0, 2), ['abcde', '    f']);
  feed(screen, `${CSI}H`, 'abcde\bX');
  assert.deepEqual(rows(screen)[0], 'abcXe');
  feed(screen, `${CSI}H`, 'abcde', `${CSI}2DY`);
  assert.deepEqual(rows(screen)[0], 'abYde');
});

test('a row filled on the last line scrolls only when another character comes', () => {
  const screen = make(5, 2);
  feed(screen, 'abcdefghij');
  assert.deepEqual(rows(screen), ['abcde', 'fghij']);
  assert.equal(screen.scrolled, 0);
  feed(screen, 'k');
  assert.deepEqual(rows(screen), ['fghij', 'k']);
  assert.equal(screen.scrolled, 1);
  assert.deepEqual(scrollback(screen), ['abcde']);
  assert.equal(screen.line(0).wrapped, true);
});

test('LF on the last row scrolls, and a full row followed by CR LF makes no blank row', () => {
  const screen = make(5, 2);
  feed(screen, 'abcde\r\nfghij\r\nk');
  assert.deepEqual(rows(screen), ['fghij', 'k']);
  assert.deepEqual(scrollback(screen), ['abcde']);
  assert.equal(screen.line(0).wrapped, false);
});

test('a character two cells wide takes two cells, the second with no text', () => {
  const screen = make(6, 2);
  feed(screen, '中a');
  assert.deepEqual([cell(screen, 0, 0).text, cell(screen, 0, 0).width], ['中', 2]);
  assert.deepEqual([cell(screen, 1, 0).text, cell(screen, 1, 0).width], ['', 0]);
  assert.deepEqual([cell(screen, 2, 0).text, cell(screen, 2, 0).width], ['a', 1]);
  assert.deepEqual(at(screen), [3, 0]);
  assert.deepEqual(rows(screen)[0], '中a');
});

test('a wide character that does not fit in the last cell goes to the next row', () => {
  const screen = make(5, 2);
  feed(screen, 'abcd中');
  assert.deepEqual(rows(screen), ['abcd', '中']);
  assert.equal(screen.line(0).wrapped, true);
  assert.deepEqual(at(screen), [2, 1]);
  const waiting = make(5, 2);
  feed(waiting, 'abcde中');
  assert.deepEqual(rows(waiting), ['abcde', '中']);
});

test('a wide character fills the last two cells when it fits there', () => {
  const screen = make(6, 2);
  feed(screen, 'abcd中');
  assert.deepEqual(rows(screen), ['abcd中', '']);
  assert.deepEqual(at(screen), [5, 0]);
  assert.equal(screen.pendingWrap, true);
});

test('writing over the left half of a wide character blanks the right half', () => {
  const screen = make(6, 2);
  feed(screen, '中\rx');
  assert.deepEqual([cell(screen, 0, 0).text, cell(screen, 1, 0).text, cell(screen, 1, 0).width], ['x', ' ', 1]);
});

test('writing over the right half of a wide character blanks the left half', () => {
  const screen = make(6, 2);
  feed(screen, '中', `${CSI}2Gx`);
  assert.deepEqual([cell(screen, 0, 0).text, cell(screen, 0, 0).width, cell(screen, 1, 0).text], [' ', 1, 'x']);
});

test('a wide character written across two others leaves no half behind', () => {
  const screen = make(8, 2);
  feed(screen, '中中', `${CSI}2G中`);
  const widths = Array.from({ length: 4 }, (_, x) => cell(screen, x, 0).width);
  assert.deepEqual(widths, [1, 2, 0, 1]);
  assert.deepEqual(rows(screen)[0], ' 中');
});

test('a mark that takes no cell joins the character before', () => {
  const screen = make(6, 2);
  feed(screen, 'éx');
  assert.equal(cell(screen, 0, 0).text, 'é');
  assert.equal(cell(screen, 1, 0).text, 'x');
  assert.deepEqual(at(screen), [2, 0]);
});

test('a mark joins a wide character, from either of its cells', () => {
  const screen = make(6, 2);
  feed(screen, '中́');
  assert.equal(cell(screen, 0, 0).text, '中́');
  assert.deepEqual(at(screen), [2, 0]);
  assert.equal(cell(screen, 1, 0).text, '');
});

test('a mark after the last cell of a row joins it, before the wrap', () => {
  const screen = make(3, 2);
  feed(screen, 'abćd');
  assert.equal(cell(screen, 2, 0).text, 'ć');
  assert.deepEqual(rows(screen)[1], 'd');
});

test('a mark joins the last cell of a row that ends in a wide character', () => {
  const screen = make(4, 2);
  feed(screen, 'ab中́');
  assert.equal(cell(screen, 2, 0).text, '中́');
});

test('a mark at the start of a row has nothing to join and is dropped', () => {
  const screen = make(6, 2);
  feed(screen, '́a');
  assert.equal(cell(screen, 0, 0).text, 'a');
  assert.deepEqual(at(screen), [1, 0]);
});

test('a mark after a cursor move joins the blank cell before it', () => {
  const screen = make(6, 2);
  feed(screen, `${CSI}3Ć`);
  assert.equal(cell(screen, 2, 0).text, ' ́');
});

test('a joiner and the emoji after it each keep their own cell', () => {
  const screen = make(8, 2);
  feed(screen, '👩‍👩');
  assert.equal(cell(screen, 0, 0).text, '👩‍');
  assert.equal(cell(screen, 2, 0).text, '👩');
  assert.deepEqual(at(screen), [4, 0]);
});

test('the same characters and marks are kept once', () => {
  const screen = make(6, 2);
  feed(screen, 'ééá');
  assert.equal(screen.clusters.length, 2);
});

test('the marks on one cell are limited', () => {
  const screen = make(6, 2);
  feed(screen, 'a', '́̂̃̄̅̆̇̈̉̊̋̌'.repeat(3));
  assert.ok(cell(screen, 0, 0).text.length <= 16);
});

test('the characters with marks that are kept are limited, and the screen still works', () => {
  const screen = make(80, 2);
  for (let i = 0; i < 70000; i++) {
    feed(screen, `${String.fromCodePoint(0x4e00 + (i >> 8))}${String.fromCodePoint(0x300 + (i & 0x3f))}${String.fromCodePoint(0x1ab0 + (i >> 6) % 16)}\r`);
  }
  assert.ok(screen.clusters.length <= 0xffff);
  feed(screen, 'ok');
  assert.equal(cell(screen, 0, 0).text, 'o');
});

test('CUP, HVP, CUF, CUB, CUU, CUD put the cursor where they say, inside the screen', () => {
  const screen = make(10, 5);
  feed(screen, `${CSI}3;4H`);
  assert.deepEqual(at(screen), [3, 2]);
  feed(screen, `${CSI}H`);
  assert.deepEqual(at(screen), [0, 0]);
  feed(screen, `${CSI};5H`);
  assert.deepEqual(at(screen), [4, 0]);
  feed(screen, `${CSI}0;0H`);
  assert.deepEqual(at(screen), [0, 0]);
  feed(screen, `${CSI}99;99H`);
  assert.deepEqual(at(screen), [9, 4]);
  feed(screen, `${CSI}2;3f`);
  assert.deepEqual(at(screen), [2, 1]);
  feed(screen, `${CSI}3C${CSI}B`);
  assert.deepEqual(at(screen), [5, 2]);
  feed(screen, `${CSI}2D${CSI}0A`);
  assert.deepEqual(at(screen), [3, 1]);
  feed(screen, `${CSI}99A${CSI}99D`);
  assert.deepEqual(at(screen), [0, 0]);
  feed(screen, `${CSI}99B${CSI}99C`);
  assert.deepEqual(at(screen), [9, 4]);
});

test('CNL and CPL move whole rows and go to the first column', () => {
  const screen = make(10, 5);
  feed(screen, `${CSI}2;5H${CSI}2E`);
  assert.deepEqual(at(screen), [0, 3]);
  feed(screen, `${CSI}3;5H${CSI}F`);
  assert.deepEqual(at(screen), [0, 1]);
});

test('CHA, HPA, VPA, HPR and VPR set or move one coordinate', () => {
  const screen = make(10, 5);
  feed(screen, `${CSI}2;2H${CSI}6G`);
  assert.deepEqual(at(screen), [5, 1]);
  feed(screen, `${CSI}9\``);
  assert.deepEqual(at(screen), [8, 1]);
  feed(screen, `${CSI}4d`);
  assert.deepEqual(at(screen), [8, 3]);
  feed(screen, `${CSI}2a${CSI}e`);
  assert.deepEqual(at(screen), [9, 4]);
  feed(screen, `${CSI}0d${CSI}99G`);
  assert.deepEqual(at(screen), [9, 0]);
});

test('VPR is not stopped by the scroll region as CUD is, only by the screen', () => {
  const screen = make(5, 8);
  feed(screen, `${CSI}3;6r${CSI}1;1H${CSI}9B`);
  assert.deepEqual(at(screen), [0, 5]);
  feed(screen, `${CSI}1;1H${CSI}9e`);
  assert.deepEqual(at(screen), [0, 7]);
  feed(screen, `${CSI}4;2H${CSI}2e`);
  assert.deepEqual(at(screen), [1, 5]);
  feed(screen, `${CSI}?6h${CSI}1;1H${CSI}2e`);
  assert.deepEqual(at(screen), [0, 4]);
  feed(screen, `${CSI}9e`);
  assert.deepEqual(at(screen), [0, 5]);
});

test('tab stops are every 8 columns, and the last column stops a tab', () => {
  const screen = make(20, 2);
  feed(screen, '\t');
  assert.equal(screen.x, 8);
  feed(screen, '\t');
  assert.equal(screen.x, 16);
  feed(screen, '\t');
  assert.equal(screen.x, 19);
  feed(screen, '\t');
  assert.equal(screen.x, 19);
});

test('CHT moves forward by tab stops and CBT back', () => {
  const screen = make(20, 2);
  feed(screen, `${CSI}2I`);
  assert.equal(screen.x, 16);
  feed(screen, `${CSI}Z`);
  assert.equal(screen.x, 8);
  feed(screen, `${CSI}9Z`);
  assert.equal(screen.x, 0);
  feed(screen, `${CSI}Z`);
  assert.equal(screen.x, 0);
});

test('HTS sets a stop at the cursor, and TBC clears one or all', () => {
  const screen = make(20, 2);
  feed(screen, `${CSI}5G${ESC}H\r\t`);
  assert.equal(screen.x, 4);
  feed(screen, `${CSI}g\r\t`);
  assert.equal(screen.x, 8);
  feed(screen, `${CSI}9G${CSI}g\r\t`);
  assert.equal(screen.x, 16);
  feed(screen, `${CSI}3g\r\t`);
  assert.equal(screen.x, 19);
  feed(screen, `${CSI}7g\r\t`);
  assert.equal(screen.x, 19);
});

test('a tab does not end a wait to wrap: the cursor is on the last column already', () => {
  const screen = make(10, 2);
  feed(screen, 'abcdefghij\tk');
  assert.deepEqual(rows(screen), ['abcdefghij', 'k']);
  const forward = make(10, 2);
  feed(forward, `abcdefghij${CSI}Ik`);
  assert.deepEqual(rows(forward), ['abcdefghij', 'k']);
});

test('ED erases to the end, to the start, or the whole screen, in the cursor\'s cell too', () => {
  const below = filled();
  feed(below, `${CSI}2;3H${CSI}J`);
  assert.deepEqual(rows(below), ['aaaaa', 'bb', '', '']);
  const above = filled();
  feed(above, `${CSI}2;3H${CSI}1J`);
  assert.deepEqual(rows(above), ['', '   bb', 'ccccc', 'ddddd']);
  const all = filled();
  feed(all, `${CSI}2;3H${CSI}2J`);
  assert.deepEqual(rows(all), ['', '', '', '']);
  assert.deepEqual(at(all), [2, 1]);
});

test('ED 3 forgets the scrollback and leaves the screen', () => {
  const screen = make(5, 2);
  feed(screen, 'a\r\nb\r\nc');
  assert.deepEqual(scrollback(screen), ['a']);
  feed(screen, `${CSI}3J`);
  assert.deepEqual(scrollback(screen), []);
  assert.deepEqual(rows(screen), ['b', 'c']);
  assert.equal(screen.firstLine, screen.scrolled);
});

test('an ED with a parameter this terminal does not know does nothing', () => {
  const screen = filled();
  feed(screen, `${CSI}9J`);
  assert.deepEqual(rows(screen), ['aaaaa', 'bbbbb', 'ccccc', 'ddddd']);
});

test('EL erases to the end, to the start, or the whole row', () => {
  const end = filled();
  feed(end, `${CSI}2;3H${CSI}K`);
  assert.deepEqual(rows(end)[1], 'bb');
  const start = filled();
  feed(start, `${CSI}2;3H${CSI}1K`);
  assert.deepEqual(rows(start)[1], '   bb');
  const whole = filled();
  feed(whole, `${CSI}2;3H${CSI}2K`);
  assert.deepEqual(rows(whole)[1], '');
  feed(whole, `${CSI}9K`);
  assert.deepEqual(rows(whole), ['aaaaa', '', 'ccccc', 'ddddd']);
});

test('ECH erases cells without moving the rest, and stops at the end of the row', () => {
  const screen = filled();
  feed(screen, `${CSI}1;2H${CSI}2X`);
  assert.deepEqual(rows(screen)[0], 'a  aa');
  feed(screen, `${CSI}1;4H${CSI}99X`);
  assert.deepEqual(rows(screen)[0], 'a');
  assert.deepEqual(at(screen), [3, 0]);
});

test('ICH makes room at the cursor and loses what is pushed off', () => {
  const screen = make(5, 2);
  feed(screen, 'abcde', `${CSI}2G${CSI}2@`);
  assert.deepEqual(rows(screen)[0], 'a  bc');
  feed(screen, `${CSI}3G${CSI}99@`);
  assert.deepEqual(rows(screen)[0], 'a');
});

test('DCH removes cells at the cursor and pulls the rest left', () => {
  const screen = make(5, 2);
  feed(screen, 'abcde', `${CSI}2G${CSI}2P`);
  assert.deepEqual(rows(screen)[0], 'ade');
  feed(screen, `${CSI}3G${CSI}99P`);
  assert.deepEqual(rows(screen)[0], 'ad');
});

test('ICH, DCH, ECH, EL and ED end a wait to wrap, as xterm does', () => {
  const screen = make(5, 3);
  feed(screen, 'abcde');
  assert.equal(screen.pendingWrap, true);
  feed(screen, `${CSI}K`);
  assert.equal(screen.pendingWrap, false);
  assert.deepEqual(rows(screen)[0], 'abcd');
  for (const edit of ['@', 'P', 'X', 'K', '1K', '2K', 'J', '1J', '2J']) {
    feed(screen, `${CSI}H`, 'abcde');
    assert.equal(screen.pendingWrap, true, edit);
    feed(screen, `${CSI}${edit}`);
    assert.equal(screen.pendingWrap, false, edit);
  }
  // Written after the erase, a character goes in the last cell: it does not wrap.
  feed(screen, `${CSI}H${CSI}2J`, 'abcde', `${CSI}2Kz`);
  assert.deepEqual(rows(screen), ['    z', '', '']);
  // A mode that is not an erase does not.
  feed(screen, `${CSI}H`, 'abcde', `${CSI}4K${CSI}3J`);
  assert.equal(screen.pendingWrap, true);
});

test('the alternate screen cleared by mode 1049 ends a wait to wrap, 47 and 1047 do not', () => {
  for (const [mode, ends] of [['47', false], ['1047', false], ['1049', true]]) {
    const screen = make(5, 3);
    feed(screen, 'abcde', `${CSI}?${mode}h`);
    assert.equal(screen.pendingWrap, !ends, mode);
  }
  const screen = make(5, 3);
  feed(screen, 'abcde', `${CSI}?1049h${CSI}?1049lz`);
  assert.deepEqual(rows(screen), ['abcde', 'z', '']);
});

test('a row a wide character left short is whole again once its last cell has text', () => {
  const screen = make(5, 3);
  feed(screen, 'abcd\u4e2d');
  const short = () => screen.line(screen.scrolled).length;
  assert.equal(short(), 4);
  feed(screen, `${CSI}1;2H${CSI}@`, 'x');
  assert.equal(short(), 5);
  assert.deepEqual(rows(screen)[0], 'axbcd');
  assert.equal(screen.line(screen.scrolled).wrapped, true);
});

test('erasing from the middle of a wide character blanks the left half too', () => {
  const screen = make(6, 2);
  feed(screen, '中a', `${CSI}2G${CSI}K`);
  assert.deepEqual([cell(screen, 0, 0).text, cell(screen, 0, 0).width, cell(screen, 1, 0).width], [' ', 1, 1]);
});

test('erasing up to the left half of a wide character blanks the right half too', () => {
  const screen = make(6, 2);
  feed(screen, '中a', `${CSI}1G${CSI}1K`);
  assert.deepEqual([cell(screen, 1, 0).width, cell(screen, 2, 0).text], [1, 'a']);
});

test('inserting in the middle of a wide character leaves no half of it', () => {
  const screen = make(6, 2);
  feed(screen, '中a', `${CSI}2G${CSI}@`);
  assert.deepEqual(Array.from({ length: 4 }, (_, x) => cell(screen, x, 0).width), [1, 1, 1, 1]);
  assert.equal(cell(screen, 3, 0).text, 'a');
});

test('inserting that pushes half of a wide character off the end blanks the other half', () => {
  const screen = make(4, 2);
  feed(screen, 'ab中', `${CSI}1G${CSI}@`);
  assert.deepEqual(Array.from({ length: 4 }, (_, x) => cell(screen, x, 0).width), [1, 1, 1, 1]);
  assert.deepEqual([cell(screen, 1, 0).text, cell(screen, 2, 0).text, cell(screen, 3, 0).text], ['a', 'b', ' ']);
});

test('deleting up to half of a wide character blanks the other half', () => {
  const screen = make(6, 2);
  feed(screen, 'a中b', `${CSI}1G${CSI}2P`);
  assert.deepEqual([cell(screen, 0, 0).width, cell(screen, 0, 0).text, cell(screen, 1, 0).text], [1, ' ', 'b']);
  const middle = make(6, 2);
  feed(middle, 'a中b', `${CSI}3G${CSI}P`);
  assert.deepEqual([cell(middle, 1, 0).width, cell(middle, 2, 0).width], [1, 1]);
});

test('a wide character cut by a deletion from the middle leaves a blank, not half', () => {
  const screen = make(6, 2);
  feed(screen, '中中', `${CSI}2G${CSI}P`);
  const widths = Array.from({ length: 4 }, (_, x) => cell(screen, x, 0).width);
  assert.deepEqual(widths, [1, 2, 0, 1]);
});

test('IL pushes rows down from the cursor\'s and DL pulls them up, both to the first column', () => {
  const insert = filled();
  feed(insert, `${CSI}2;3H${CSI}L`);
  assert.deepEqual(rows(insert), ['aaaaa', '', 'bbbbb', 'ccccc']);
  assert.deepEqual(at(insert), [0, 1]);
  const remove = filled();
  feed(remove, `${CSI}2;3H${CSI}M`);
  assert.deepEqual(rows(remove), ['aaaaa', 'ccccc', 'ddddd', '']);
  assert.deepEqual(at(remove), [0, 1]);
});

test('IL and DL are bounded by the screen, and drop nothing into the scrollback', () => {
  const insert = filled();
  feed(insert, `${CSI}2;1H${CSI}99L`);
  assert.deepEqual(rows(insert), ['aaaaa', '', '', '']);
  const remove = filled();
  feed(remove, `${CSI}2;1H${CSI}99M`);
  assert.deepEqual(rows(remove), ['aaaaa', '', '', '']);
  assert.deepEqual([scrollback(remove).length, remove.scrolled], [0, 0]);
});

test('IL and DL keep inside the scroll region, and do nothing outside it', () => {
  const region = filled();
  feed(region, `${CSI}2;3r${CSI}2;1H${CSI}L`);
  assert.deepEqual(rows(region), ['aaaaa', '', 'bbbbb', 'ddddd']);
  feed(region, `${CSI}2;1H${CSI}M`);
  assert.deepEqual(rows(region), ['aaaaa', 'bbbbb', '', 'ddddd']);
  feed(region, `${CSI}1;1H${CSI}L${CSI}4;1H${CSI}M`);
  assert.deepEqual(rows(region), ['aaaaa', 'bbbbb', '', 'ddddd']);
  assert.deepEqual(at(region), [0, 3]);
});

test('SU scrolls the region up, keeping the lines that leave the top of the screen', () => {
  const screen = filled();
  feed(screen, `${CSI}2S`);
  assert.deepEqual(rows(screen), ['ccccc', 'ddddd', '', '']);
  assert.deepEqual(scrollback(screen), ['aaaaa', 'bbbbb']);
  feed(screen, `${CSI}99S`);
  assert.deepEqual(rows(screen), ['', '', '', '']);
});

test('SD scrolls the region down', () => {
  const screen = filled();
  feed(screen, `${CSI}T`);
  assert.deepEqual(rows(screen), ['', 'aaaaa', 'bbbbb', 'ccccc']);
  feed(screen, `${CSI}99T`);
  assert.deepEqual(rows(screen), ['', '', '', '']);
  assert.equal(screen.ringCount, 0);
});

test('a T with more than one parameter is not a scroll', () => {
  const screen = filled();
  feed(screen, `${CSI}1;2;3;4;5T`);
  assert.deepEqual(rows(screen), ['aaaaa', 'bbbbb', 'ccccc', 'ddddd']);
});

test('DECSTBM sets the scroll region and homes the cursor, and a bad one is ignored', () => {
  const screen = filled();
  feed(screen, `${CSI}3;3H${CSI}2;3r`);
  assert.deepEqual([screen.top, screen.bottom], [1, 2]);
  assert.deepEqual(at(screen), [0, 0]);
  feed(screen, `${CSI}3;3H${CSI}3;2r`);
  assert.deepEqual([screen.top, screen.bottom, ...at(screen)], [1, 2, 2, 2]);
  feed(screen, `${CSI}2;2r`);
  assert.deepEqual([screen.top, screen.bottom], [1, 2]);
  feed(screen, `${CSI}4;99r`);
  assert.deepEqual([screen.top, screen.bottom], [1, 2]);
  feed(screen, `${CSI}2;99r`);
  assert.deepEqual([screen.top, screen.bottom], [1, 3]);
  feed(screen, `${CSI}r`);
  assert.deepEqual([screen.top, screen.bottom], [0, 3]);
});

test('LF on the last row of the region scrolls the region only', () => {
  const screen = filled();
  feed(screen, `${CSI}2;3r${CSI}3;1H\n`);
  assert.deepEqual(rows(screen), ['aaaaa', 'ccccc', '', 'ddddd']);
  assert.deepEqual([screen.scrolled, screen.ringCount], [0, 0]);
  assert.deepEqual(at(screen), [0, 2]);
});

test('LF below the region moves down, and stops on the last row without scrolling', () => {
  const screen = filled();
  feed(screen, `${CSI}1;2r${CSI}3;1H\n`);
  assert.deepEqual(at(screen), [0, 3]);
  feed(screen, '\n');
  assert.deepEqual(at(screen), [0, 3]);
  assert.deepEqual(rows(screen), ['aaaaa', 'bbbbb', 'ccccc', 'ddddd']);
});

test('a region that starts at the top keeps the lines that leave it', () => {
  const screen = filled();
  feed(screen, `${CSI}1;3r${CSI}3;1H\n`);
  assert.deepEqual(rows(screen), ['bbbbb', 'ccccc', '', 'ddddd']);
  assert.deepEqual(scrollback(screen), ['aaaaa']);
});

test('IND, NEL and RI move the cursor and scroll at the edges of the region', () => {
  const screen = filled();
  feed(screen, `${CSI}2;3r${CSI}3;3H${ESC}D`);
  assert.deepEqual(rows(screen), ['aaaaa', 'ccccc', '', 'ddddd']);
  assert.deepEqual(at(screen), [2, 2]);
  feed(screen, `${ESC}E`);
  assert.deepEqual(rows(screen), ['aaaaa', '', '', 'ddddd']);
  assert.deepEqual(at(screen), [0, 2]);
  feed(screen, `${CSI}2;1H${ESC}M`);
  assert.deepEqual(rows(screen), ['aaaaa', '', '', 'ddddd']);
  feed(screen, `${CSI}2;1HX${CSI}2;1H${ESC}M`);
  assert.deepEqual(rows(screen), ['aaaaa', '', 'X', 'ddddd']);
  feed(screen, `${CSI}4;1H${ESC}M`);
  assert.deepEqual(at(screen), [0, 2]);
});

test('RI on the first row of the screen does not move the cursor above it', () => {
  const screen = filled();
  feed(screen, `${ESC}M`);
  assert.deepEqual(at(screen), [0, 0]);
  assert.deepEqual(rows(screen), ['', 'aaaaa', 'bbbbb', 'ccccc']);
});

test('up and down stop at the edges of the region when the cursor is inside it', () => {
  const screen = make(10, 6);
  feed(screen, `${CSI}2;4r${CSI}3;1H${CSI}9A`);
  assert.equal(screen.y, 1);
  feed(screen, `${CSI}9B`);
  assert.equal(screen.y, 3);
  feed(screen, `${CSI}1;1H${CSI}9B`);
  assert.equal(screen.y, 3);
  feed(screen, `${CSI}6;1H${CSI}9A`);
  assert.equal(screen.y, 1);
  feed(screen, `${CSI}6;1H${CSI}9B`);
  assert.equal(screen.y, 5);
  feed(screen, `${CSI}3;5r${CSI}2;1H${CSI}9A`);
  assert.equal(screen.y, 0);
});

test('lines that leave the top of the screen are kept, newest last, up to the limit', () => {
  const screen = make(5, 2, { scrollback: 3 });
  feed(screen, 'a\r\nb\r\nc\r\nd\r\ne\r\nf');
  assert.deepEqual(rows(screen), ['e', 'f']);
  assert.deepEqual(scrollback(screen), ['b', 'c', 'd']);
  assert.equal(screen.scrolled, 4);
  assert.equal(screen.firstLine, 1);
  assert.equal(lineText(screen, 0), null);
  assert.equal(lineText(screen, 1), 'b');
  assert.equal(lineText(screen, 3), 'd');
  assert.equal(lineText(screen, 4), 'e');
  assert.equal(lineText(screen, 6), null);
});

test('a screen that keeps no scrollback still numbers its lines', () => {
  const screen = make(5, 2, { scrollback: 0 });
  feed(screen, 'a\r\nb\r\nc');
  assert.deepEqual(rows(screen), ['b', 'c']);
  assert.equal(screen.scrolled, 1);
  assert.equal(screen.ringCount, 0);
  assert.equal(screen.line(0), null);
});

test('a line of the scrollback says whether it went on in the next, and how wide it is', () => {
  const screen = make(5, 2, { scrollback: 5 });
  feed(screen, 'abcdefg\r\nh\r\ni');
  const first = screen.line(0);
  assert.deepEqual([first.wrapped, first.length], [true, 5]);
  const second = screen.line(1);
  assert.deepEqual([second.wrapped, second.length], [false, 2]);
});

test('the scrollback may be made shorter or longer, and the newest lines stay', () => {
  const screen = make(5, 2, { scrollback: 10 });
  feed(screen, '1\r\n2\r\n3\r\n4\r\n5\r\n6');
  assert.deepEqual(scrollback(screen), ['1', '2', '3', '4']);
  screen.setScrollback(2);
  assert.deepEqual(scrollback(screen), ['3', '4']);
  feed(screen, '\r\n7\r\n8');
  assert.deepEqual(scrollback(screen), ['5', '6']);
  screen.setScrollback(5);
  feed(screen, '\r\n9');
  assert.deepEqual(scrollback(screen), ['5', '6', '7']);
  screen.setScrollback(MAX_SCROLLBACK * 2);
  assert.equal(screen.scrollbackLimit, MAX_SCROLLBACK);
  screen.setScrollback(0);
  assert.deepEqual(scrollback(screen), []);
  feed(screen, '\r\nz');
  assert.deepEqual(scrollback(screen), []);
});

test('REP draws the last character again, bounded', () => {
  const screen = make(10, 4);
  feed(screen, `${CSI}3b`);
  assert.deepEqual(rows(screen)[0], '');
  feed(screen, `a${CSI}3b`);
  assert.deepEqual(rows(screen)[0], 'aaaa');
  const flood = make(10, 4);
  feed(flood, `a${CSI}65535b`);
  assert.equal(flood.scrolled, 1);
});

test('REP repeats only what was printed right before it, as xterm does', () => {
  // After anything that is not a character, and after a REP, there is nothing to repeat.
  for (const between of ['\r', '\n', '\b', '\t', '\x07', `${CSI}m`, `${CSI}1m`, `${ESC}7`, `${CSI}?25l`, `${ESC}]0;t\x07`, `${CSI}0K`, `${CSI}1C`, `${CSI}1b`]) {
    const screen = make(20, 4);
    feed(screen, `ab${between}${CSI}3b`);
    assert.ok(!rows(screen).join('').includes('bbb'), JSON.stringify(between));
  }
  const screen = make(20, 4);
  feed(screen, `x${CSI}1b${CSI}2b`);
  assert.deepEqual(rows(screen)[0], 'xx');
  feed(screen, `\r\ny${CSI}1bz${CSI}2b`);
  assert.deepEqual(rows(screen)[1], 'yyzzz');
});

test('the title is set by OSC 0 and OSC 2, and the program is told', () => {
  const heard = [];
  const screen = make(10, 2, { onTitle: (title) => heard.push(title) });
  feed(screen, `${ESC}]0;first\x07`);
  assert.equal(screen.title, 'first');
  feed(screen, `${ESC}]2;second${ESC}\\`);
  assert.equal(screen.title, 'second');
  feed(screen, `${ESC}]0;a;b;c\x07`);
  assert.equal(screen.title, 'a;b;c');
  assert.deepEqual(heard, ['first', 'second', 'a;b;c']);
});

test('a title has no control characters and at most 64 characters', () => {
  const screen = make(10, 2);
  screen.osc('0;ab\x01c\x1b\x7fd\u0085e');
  assert.equal(screen.title, 'abcde');
  feed(screen, `${ESC}]0;${'é'.repeat(100)}\x07`);
  assert.equal(Array.from(screen.title).length, 64);
  feed(screen, `${ESC}]2;${'😀'.repeat(70)}\x07`);
  assert.equal(Array.from(screen.title).length, 64);
  assert.ok(!screen.title.includes('�'));
});

test('an OSC 0 with no text sets an empty title', () => {
  const screen = make(10, 2);
  feed(screen, `${ESC}]0;x\x07${ESC}]0\x07`);
  assert.equal(screen.title, '');
});

test('the clipboard, hyperlinks, marks and other strings do nothing', () => {
  const heard = [];
  const screen = make(10, 2, { onTitle: (title) => heard.push(title) });
  feed(screen, `${ESC}]0;kept\x07`);
  const replies = feed(screen,
    `${ESC}]52;c;aGVsbG8=\x07${ESC}]52;c;?\x07`,
    `${ESC}]8;;https://example.com\x07link${ESC}]8;;\x07`,
    `${ESC}]133;A\x07${ESC}]133;D;0\x07`,
    `${ESC}]4;1;rgb:ff/00/00\x07${ESC}]104\x07${ESC}]999;x\x07`,
    `${ESC}Pq#0${ESC}\\${ESC}_apc${ESC}\\`);
  assert.equal(replies.length, 0);
  assert.equal(screen.title, 'kept');
  assert.deepEqual(heard, ['kept']);
  assert.deepEqual(rows(screen)[0], 'link');
});

test('escapes and sequences this terminal does not know do nothing', () => {
  const screen = filled();
  feed(screen, `${ESC}#8${ESC}%G${ESC}N${CSI}?1$p${CSI}>1u${CSI}<s${CSI}=c${CSI}5;5;5v`);
  assert.deepEqual(rows(screen), ['aaaaa', 'bbbbb', 'ccccc', 'ddddd']);
  assert.deepEqual(at(screen), [0, 0]);
});

test('the rows that change are marked, until the renderer clears the marks', () => {
  const screen = make(5, 4);
  assert.deepEqual([...screen.dirty], [1, 1, 1, 1]);
  const after = (...chunks) => {
    screen.clearDirty();
    feed(screen, ...chunks);
    return [...screen.dirty];
  };
  assert.deepEqual(after('ab'), [1, 0, 0, 0]);
  assert.deepEqual(after(`${CSI}3;1Hx`), [0, 0, 1, 0]);
  assert.deepEqual(after(`${CSI}2;2H${CSI}A${CSI}3C`), [0, 0, 0, 0]);
  assert.deepEqual(after(`${CSI}2;1H${CSI}K`), [0, 1, 0, 0]);
  assert.deepEqual(after(`${CSI}2;1H${CSI}@`), [0, 1, 0, 0]);
  assert.deepEqual(after(`${CSI}2;1H${CSI}L`), [0, 1, 1, 1]);
  assert.deepEqual(after(`${CSI}4;1H\n`), [1, 1, 1, 1]);
  assert.deepEqual(after(`${CSI}2;3r${CSI}3;1H\n`), [0, 1, 1, 0]);
  assert.deepEqual(after(`${CSI}2;1H${ESC}M`), [0, 1, 1, 0]);
  assert.deepEqual(after(`${CSI}2J`), [1, 1, 1, 1]);
  assert.deepEqual(after('e\u0301'), [0, 1, 0, 0]);
});
