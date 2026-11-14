// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { clipSelection, lineRange, selectionSpan, selectionText, wordRange } from '../pages/terminal/term/select.js';
import { CSI, feed, make } from './term-screen-helpers.mjs';

// The position of column `col` of row `y` of the screen.
const pos = (screen, y, col) => ({ line: screen.scrolled + y, col });
// A selection from one position to another, by rows of the screen.
const sel = (screen, [y0, c0], [y1, c1]) => ({ start: pos(screen, y0, c0), end: pos(screen, y1, c1) });
// The text a double click at `y`, `col` selects.
function word(screen, y, col) {
  const range = wordRange(screen, screen.scrolled + y, col);
  return range && selectionText(screen, range);
}

test('a double click selects the word under it', () => {
  const screen = make(20, 3);
  feed(screen, 'hello world  again');
  assert.deepEqual(wordRange(screen, screen.scrolled, 7), { start: pos(screen, 0, 6), end: pos(screen, 0, 10) });
  assert.equal(word(screen, 0, 0), 'hello');
  assert.equal(word(screen, 0, 4), 'hello');
  assert.equal(word(screen, 0, 6), 'world');
  assert.equal(word(screen, 0, 14), 'again');
  assert.equal(word(screen, 0, 12), '');
});

test('a double click on blanks selects the blanks', () => {
  const screen = make(20, 3);
  feed(screen, 'hello   world');
  assert.deepEqual(wordRange(screen, screen.scrolled, 6), { start: pos(screen, 0, 5), end: pos(screen, 0, 7) });
  assert.deepEqual(wordRange(screen, screen.scrolled, 15), { start: pos(screen, 0, 13), end: pos(screen, 0, 19) });
  assert.deepEqual(wordRange(screen, screen.scrolled + 1, 3), { start: pos(screen, 1, 0), end: pos(screen, 1, 19) });
});

test('a path, a host name and a URL are one word', () => {
  const screen = make(60, 2);
  feed(screen, 'cd /usr/local/bin; ssh a-b.example.org https://x.y/z?q=1&r=2#top');
  assert.equal(word(screen, 0, 5), '/usr/local/bin');
  assert.equal(word(screen, 0, 20), 'ssh');
  assert.equal(word(screen, 0, 30), 'a-b.example.org');
  assert.equal(word(screen, 0, 50), 'https://x.y/z?q=1&r=2#top');
  assert.equal(word(screen, 0, 17), ';');
});

test('other punctuation is a run of the same character', () => {
  const screen = make(20, 2);
  feed(screen, '((a)) [b] a,b "x"');
  assert.equal(word(screen, 0, 0), '((');
  assert.equal(word(screen, 0, 1), '((');
  assert.equal(word(screen, 0, 2), 'a');
  assert.equal(word(screen, 0, 3), '))');
  assert.equal(word(screen, 0, 7), 'b');
  assert.equal(word(screen, 0, 11), ',');
  assert.equal(word(screen, 0, 12), 'b');
});

test('letters past ASCII, wide characters and marks are words', () => {
  const screen = make(30, 2);
  feed(screen, 'na\u00efve e\u0301t\u00e9 \u4e2d\u6587\u5b57 x');
  assert.equal(word(screen, 0, 2), 'na\u00efve');
  assert.equal(word(screen, 0, 7), 'e\u0301t\u00e9');
  assert.equal(word(screen, 0, 10), '\u4e2d\u6587\u5b57');
  assert.equal(word(screen, 0, 12), '\u4e2d\u6587\u5b57');
  assert.equal(word(screen, 0, 15), '\u4e2d\u6587\u5b57');
  assert.deepEqual(wordRange(screen, screen.scrolled, 13), { start: pos(screen, 0, 10), end: pos(screen, 0, 15) });
});

test('a word goes on over rows that run into each other', () => {
  const screen = make(10, 4);
  feed(screen, 'xx aaaaaaaaaabbbb cc');
  assert.deepEqual(wordRange(screen, screen.scrolled + 1, 2), { start: pos(screen, 0, 3), end: pos(screen, 1, 6) });
  assert.deepEqual(wordRange(screen, screen.scrolled, 5), { start: pos(screen, 0, 3), end: pos(screen, 1, 6) });
  assert.equal(word(screen, 1, 3), 'aaaaaaaaaabbbb');
  assert.equal(word(screen, 1, 8), 'cc');
});

test('a word does not go on over a row that ends its line', () => {
  const screen = make(10, 3);
  feed(screen, 'abcdefghij\r\nklm');
  assert.deepEqual(wordRange(screen, screen.scrolled, 9), { start: pos(screen, 0, 0), end: pos(screen, 0, 9) });
  assert.deepEqual(wordRange(screen, screen.scrolled + 1, 0), { start: pos(screen, 1, 0), end: pos(screen, 1, 2) });
});

test('the blank a wide character leaves at the end of a row is not a break', () => {
  const screen = make(5, 3);
  feed(screen, 'abcd\u4e2d\u6587 z');
  assert.deepEqual(wordRange(screen, screen.scrolled, 1), { start: pos(screen, 0, 0), end: pos(screen, 1, 3) });
  assert.equal(word(screen, 1, 1), 'abcd\u4e2d\u6587');
});

test('a double click in the scrollback', () => {
  const screen = make(10, 2);
  feed(screen, 'one two\r\nthree\r\nfour five\r\nsix');
  assert.equal(screen.ringCount, 2);
  assert.equal(selectionText(screen, wordRange(screen, screen.scrolled - 2, 5)), 'two');
  assert.equal(selectionText(screen, wordRange(screen, screen.scrolled - 1, 2)), 'three');
});

test('a double click outside the lines or the columns selects nothing', () => {
  const screen = make(10, 2);
  feed(screen, 'abc');
  assert.equal(wordRange(screen, screen.scrolled + 2, 0), null);
  assert.equal(wordRange(screen, screen.scrolled - 1, 0), null);
  assert.equal(wordRange(screen, screen.scrolled, -1), null);
  assert.equal(wordRange(screen, screen.scrolled, 10), null);
  assert.equal(wordRange(screen, screen.scrolled, 1.5), null);
});

test('a triple click selects the whole line the program wrote', () => {
  const screen = make(10, 5);
  feed(screen, 'first\r\naaaaaaaaaabbbbbbbbbbcc\r\nlast');
  const range = { start: pos(screen, 1, 0), end: pos(screen, 3, 9) };
  assert.deepEqual(lineRange(screen, screen.scrolled + 1), range);
  assert.deepEqual(lineRange(screen, screen.scrolled + 2), range);
  assert.deepEqual(lineRange(screen, screen.scrolled + 3), range);
  assert.deepEqual(lineRange(screen, screen.scrolled), { start: pos(screen, 0, 0), end: pos(screen, 0, 9) });
  assert.equal(selectionText(screen, lineRange(screen, screen.scrolled + 2)), 'aaaaaaaaaabbbbbbbbbbcc');
  assert.equal(lineRange(screen, screen.scrolled + 5), null);
  assert.equal(lineRange(screen, screen.scrolled - 1), null);
});

test('a triple click on the last row of the screen stops there', () => {
  const screen = make(5, 2);
  feed(screen, 'abcdefgh');
  assert.deepEqual(lineRange(screen, screen.scrolled), { start: pos(screen, 0, 0), end: pos(screen, 1, 4) });
  const full = make(5, 1);
  feed(full, 'abcdefgh');
  assert.equal(selectionText(full, lineRange(full, full.scrolled)), 'abcdefgh');
  assert.equal(selectionText(full, lineRange(full, full.scrolled - 1)), 'abcdefgh');
});

test('the text of a selection inside a row', () => {
  const screen = make(20, 3);
  feed(screen, 'hello world');
  assert.equal(selectionText(screen, sel(screen, [0, 6], [0, 10])), 'world');
  assert.equal(selectionText(screen, sel(screen, [0, 10], [0, 6])), 'world');
  assert.equal(selectionText(screen, sel(screen, [0, 4], [0, 6])), 'o w');
  assert.equal(selectionText(screen, sel(screen, [0, 4], [0, 4])), 'o');
  assert.equal(selectionText(screen, sel(screen, [0, 6], [0, 19])), 'world');
  assert.equal(selectionText(screen, sel(screen, [0, 12], [0, 15])), '');
});

test('rows that end a line are joined with a line break and lose their trailing blanks', () => {
  const screen = make(10, 4);
  feed(screen, 'one  \r\n\r\ntwo\r\nthree');
  assert.equal(selectionText(screen, sel(screen, [0, 0], [3, 9])), 'one\n\ntwo\nthree');
  assert.equal(selectionText(screen, sel(screen, [0, 2], [2, 1])), 'e\n\ntw');
  assert.equal(selectionText(screen, sel(screen, [3, 9], [0, 2])), 'e\n\ntwo\nthree');
});

test('rows that run into each other are joined without a break', () => {
  const screen = make(10, 4);
  feed(screen, 'aaaaaaaaaabbbbbbbbbbcc\r\nx');
  assert.equal(selectionText(screen, sel(screen, [0, 0], [3, 0])), 'aaaaaaaaaabbbbbbbbbbcc\nx');
  assert.equal(selectionText(screen, sel(screen, [0, 8], [1, 1])), 'aabb');
  assert.equal(selectionText(screen, sel(screen, [0, 8], [0, 9])), 'aa');
});

test('blanks inside a wrapped line are kept and the end of the line is not', () => {
  const screen = make(5, 3);
  feed(screen, 'ab   cde  ');
  assert.equal(selectionText(screen, sel(screen, [0, 0], [1, 4])), 'ab   cde');
  assert.equal(selectionText(screen, sel(screen, [0, 0], [0, 4])), 'ab   ');
});

test('the text of a selection over the scrollback and the screen', () => {
  const screen = make(10, 2);
  feed(screen, 'one\r\ntwo\r\nthree\r\nfour');
  assert.equal(screen.ringCount, 2);
  const all = { start: { line: screen.firstLine, col: 0 }, end: { line: screen.scrolled + 1, col: 9 } };
  assert.equal(selectionText(screen, all), 'one\ntwo\nthree\nfour');
  const part = { start: { line: screen.scrolled - 1, col: 1 }, end: { line: screen.scrolled, col: 1 } };
  assert.equal(selectionText(screen, part), 'wo\nth');
});

test('a wide character is copied whole whichever of its cells is the edge', () => {
  const screen = make(10, 2);
  feed(screen, 'a\u4e2d\u6587b');
  assert.equal(selectionText(screen, sel(screen, [0, 2], [0, 3])), '\u4e2d\u6587');
  assert.equal(selectionText(screen, sel(screen, [0, 2], [0, 2])), '\u4e2d');
  assert.equal(selectionText(screen, sel(screen, [0, 1], [0, 1])), '\u4e2d');
  assert.equal(selectionText(screen, sel(screen, [0, 0], [0, 1])), 'a\u4e2d');
  assert.equal(selectionText(screen, sel(screen, [0, 4], [0, 5])), '\u6587b');
  assert.equal(selectionText(screen, sel(screen, [0, 3], [0, 4])), '\u6587');
});

test('combining marks are copied with their character', () => {
  const screen = make(10, 2);
  feed(screen, 'e\u0301x');
  assert.equal(selectionText(screen, sel(screen, [0, 0], [0, 1])), 'e\u0301x');
});

test('coloured blanks at the end of a row are not text', () => {
  const screen = make(10, 2);
  feed(screen, `ab${CSI}44m      ${CSI}0m`);
  assert.equal(selectionText(screen, sel(screen, [0, 0], [0, 9])), 'ab');
});

test('what is no longer there is left out', () => {
  const screen = make(10, 2, { scrollback: 1 });
  feed(screen, 'a\r\nb\r\nc\r\nd');
  assert.equal(screen.ringCount, 1);
  const wide = { start: { line: screen.scrolled - 5, col: 0 }, end: { line: screen.scrolled + 9, col: 9 } };
  assert.equal(selectionText(screen, wide), 'b\nc\nd');
  assert.equal(selectionText(screen, { start: { line: -9, col: 0 }, end: { line: -8, col: 3 } }), '');
  assert.equal(selectionText(screen, { start: { line: 99, col: 0 }, end: { line: 99, col: 3 } }), '');
});

test('the span of a selection in each row, for drawing it', () => {
  const screen = make(10, 4);
  feed(screen, 'abcdefghij');
  const selection = sel(screen, [0, 3], [2, 5]);
  assert.deepEqual(selectionSpan(screen, selection, screen.scrolled), { from: 3, to: 10 });
  assert.deepEqual(selectionSpan(screen, selection, screen.scrolled + 1), { from: 0, to: 10 });
  assert.deepEqual(selectionSpan(screen, selection, screen.scrolled + 2), { from: 0, to: 6 });
  assert.equal(selectionSpan(screen, selection, screen.scrolled + 3), null);
  assert.equal(selectionSpan(screen, selection, screen.scrolled - 1), null);
  assert.equal(selectionSpan(screen, selection, screen.scrolled + 9), null);
  // A line the selection reaches over but that is not there (it has scrolled away).
  const beyond = { start: pos(screen, 1, 0), end: pos(screen, 9, 0) };
  assert.equal(selectionSpan(screen, beyond, screen.scrolled + 5), null);
  assert.deepEqual(selectionSpan(screen, beyond, screen.scrolled + 1), { from: 0, to: 10 });
  assert.deepEqual(selectionSpan(screen, sel(screen, [2, 5], [0, 3]), screen.scrolled + 2), { from: 0, to: 6 });
  assert.deepEqual(selectionSpan(screen, sel(screen, [1, 4], [1, 4]), screen.scrolled + 1), { from: 4, to: 5 });
});

test('the span takes a wide character in whole, and stays inside the row', () => {
  const screen = make(10, 2);
  feed(screen, 'a\u4e2d\u6587b');
  assert.deepEqual(selectionSpan(screen, sel(screen, [0, 2], [0, 4]), screen.scrolled), { from: 1, to: 5 });
  assert.deepEqual(selectionSpan(screen, sel(screen, [0, 1], [0, 1]), screen.scrolled), { from: 1, to: 3 });
  assert.deepEqual(selectionSpan(screen, sel(screen, [0, -4], [0, 30]), screen.scrolled), { from: 0, to: 10 });
  assert.deepEqual(selectionSpan(screen, sel(screen, [0, 3], [0, 1]), screen.scrolled), { from: 1, to: 5 });
});

test('a selection is cut to the lines that are left as output scrolls', () => {
  const screen = make(10, 2, { scrollback: 2 });
  feed(screen, 'a\r\nb\r\nc\r\nd');
  const selection = { start: { line: screen.firstLine, col: 3 }, end: { line: screen.scrolled + 1, col: 4 } };
  assert.deepEqual(clipSelection(screen, selection), selection);
  feed(screen, '\r\ne');
  assert.deepEqual(clipSelection(screen, selection), {
    start: { line: screen.firstLine, col: 0 },
    end: selection.end,
  });
  feed(screen, '\r\nf\r\ng\r\nh');
  assert.equal(clipSelection(screen, selection), null);
  assert.deepEqual(clipSelection(screen, { start: { line: screen.scrolled, col: 2 }, end: { line: screen.scrolled + 50, col: 2 } }), {
    start: { line: screen.scrolled, col: 2 },
    end: { line: screen.scrolled + 1, col: 9 },
  });
  assert.equal(clipSelection(screen, { start: { line: screen.scrolled + 5, col: 0 }, end: { line: screen.scrolled + 6, col: 0 } }), null);
  assert.deepEqual(clipSelection(screen, { start: { line: screen.scrolled, col: 5 }, end: { line: screen.scrolled, col: 1 } }), {
    start: { line: screen.scrolled, col: 1 },
    end: { line: screen.scrolled, col: 5 },
  });
});

test('on the alternate screen there is no scrollback to select', () => {
  const screen = make(10, 2);
  feed(screen, 'old\r\nlines\r\nhere');
  feed(screen, `${CSI}?1049h${CSI}Halt`);
  assert.equal(screen.firstLine, screen.scrolled);
  assert.equal(selectionText(screen, { start: { line: screen.scrolled - 2, col: 0 }, end: { line: screen.scrolled + 1, col: 9 } }), 'alt\n');
  assert.equal(wordRange(screen, screen.scrolled - 1, 0), null);
  assert.equal(clipSelection(screen, { start: { line: screen.scrolled - 2, col: 0 }, end: { line: screen.scrolled - 1, col: 0 } }), null);
});
