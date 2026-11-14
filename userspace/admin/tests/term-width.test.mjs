// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's character widths in term/width.js: wide characters take 2
// cells, marks that join the cell before take 0, and the edges of the
// tables sit where the Unicode data puts them.

import assert from 'node:assert/strict';
import test from 'node:test';

import { charWidth } from '../pages/terminal/term/width.js';

test('ASCII and Latin letters take one cell', () => {
  assert.equal(charWidth(0x20), 1);
  assert.equal(charWidth(0x41), 1);
  assert.equal(charWidth(0x7e), 1);
  assert.equal(charWidth(0xe9), 1);
  assert.equal(charWidth(0x2ff), 1);
});

test('Chinese, Japanese and Korean characters take two cells', () => {
  assert.equal(charWidth(0x4e2d), 2);
  assert.equal(charWidth(0x3042), 2);
  assert.equal(charWidth(0x30ab), 2);
  assert.equal(charWidth(0xac00), 2);
  assert.equal(charWidth(0x20000), 2);
  assert.equal(charWidth(0x30000), 2);
});

test('fullwidth forms take two cells and halfwidth forms one', () => {
  assert.equal(charWidth(0x3000), 2);
  assert.equal(charWidth(0xff21), 2);
  assert.equal(charWidth(0xff60), 2);
  assert.equal(charWidth(0xff61), 1);
  assert.equal(charWidth(0xff71), 1);
});

test('emoji that are shown as pictures take two cells and text symbols one', () => {
  assert.equal(charWidth(0x1f600), 2);
  assert.equal(charWidth(0x1f680), 2);
  assert.equal(charWidth(0x231a), 2);
  assert.equal(charWidth(0x2764), 1);
  assert.equal(charWidth(0x00a9), 1);
});

test('the edges of a wide range are exact', () => {
  assert.equal(charWidth(0x10ff), 1);
  assert.equal(charWidth(0x1100), 2);
  assert.equal(charWidth(0x115f), 2);
  assert.equal(charWidth(0x303e), 2);
  assert.equal(charWidth(0x303f), 1);
  assert.equal(charWidth(0x4dbf), 2);
  assert.equal(charWidth(0x4dc0), 1);
  assert.equal(charWidth(0x4e00), 2);
});

test('combining marks, joiners and variation selectors take no cell', () => {
  assert.equal(charWidth(0x300), 0);
  assert.equal(charWidth(0x36f), 0);
  assert.equal(charWidth(0x5b0), 0);
  assert.equal(charWidth(0x64b), 0);
  assert.equal(charWidth(0x200b), 0);
  assert.equal(charWidth(0x200d), 0);
  assert.equal(charWidth(0xfe0f), 0);
  assert.equal(charWidth(0x20e3), 0);
  assert.equal(charWidth(0xe0100), 0);
  assert.equal(charWidth(0x3099), 0);
});

test('Hangul vowels and final consonants join the syllable before them', () => {
  assert.equal(charWidth(0x115f), 2);
  assert.equal(charWidth(0x1160), 0);
  assert.equal(charWidth(0x11ff), 0);
  assert.equal(charWidth(0x1200), 1);
});

test('the soft hyphen and spacing marks take one cell', () => {
  assert.equal(charWidth(0xad), 1);
  assert.equal(charWidth(0x903), 1);
  assert.equal(charWidth(0x600), 1);
});

test('private use, unassigned and the last code points take one cell', () => {
  assert.equal(charWidth(0xe000), 1);
  assert.equal(charWidth(0xf8ff), 1);
  assert.equal(charWidth(0x10ffff), 1);
});

test('every code point has a width of 0, 1 or 2', () => {
  const seen = new Set();
  for (let cp = 0; cp <= 0x10ffff; cp++) seen.add(charWidth(cp));
  assert.deepEqual([...seen].sort(), [0, 1, 2]);
});
