// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a row of the screen looks like: the runs of cells that share a look,
// their classes and colours, wide characters, the selection and the cursor.

import assert from 'node:assert/strict';
import test from 'node:test';

import { MINIMUM, contrast, readableRuns } from '../pages/terminal/contrast-model.js';
import { paletteColour, rowRuns } from '../pages/terminal/render-model.js';
import { Screen } from '../pages/terminal/term/screen.js';
import { selectionSpan } from '../pages/terminal/term/select.js';

const ESC = '\u001b';
const encoder = new TextEncoder();

function screenWith(text, cols = 12, rows = 3) {
  const screen = new Screen({ cols, rows, scrollback: 50 });
  screen.write(encoder.encode(text));
  return screen;
}
const selected = (screen, selection, row = 0) => ({ span: selectionSpan(screen, selection, screen.scrolled + row) });
const runsOf = (screen, row = 0, options) => rowRuns(screen, screen.scrolled + row, options).map((run) => [run.text, run.cls, run.color, run.background]);

test('the 256 colours: the cube, then the greys', () => {
  assert.equal(paletteColour(16), '#000000');
  assert.equal(paletteColour(21), '#0000ff');
  assert.equal(paletteColour(196), '#ff0000');
  assert.equal(paletteColour(200), '#ff00d7');
  assert.equal(paletteColour(231), '#ffffff');
  assert.equal(paletteColour(232), '#080808');
  assert.equal(paletteColour(244), '#808080');
  assert.equal(paletteColour(255), '#eeeeee');
});

test('plain text is one run, and the blanks at the end are left out', () => {
  assert.deepEqual(runsOf(screenWith('hello  world   ')), [['hello  world', '', '', '']]);
  assert.deepEqual(runsOf(screenWith('')), []);
  assert.deepEqual(runsOf(screenWith('hi'), 2), []);
});

test('a line that is not there has no runs', () => {
  const screen = screenWith('x');
  assert.deepEqual(rowRuns(screen, 99), []);
  assert.deepEqual(rowRuns(screen, -4), []);
});

test('the first 16 colours are classes, as foreground and as background', () => {
  const runs = runsOf(screenWith(`a${ESC}[31mb${ESC}[44mc${ESC}[91;102md${ESC}[0me`));
  assert.deepEqual(runs, [
    ['a', '', '', ''], ['b', 'f1', '', ''], ['c', 'f1 g4', '', ''], ['d', 'f9 g10', '', ''], ['e', '', '', ''],
  ]);
});

test('the 256 colours and true colour are colours for the style', () => {
  const runs = runsOf(screenWith(`${ESC}[38;5;200mx${ESC}[48;5;21my${ESC}[38;2;1;2;3;48;2;250;251;252mz${ESC}[38;5;7mw`));
  assert.deepEqual(runs, [
    ['x', '', '#ff00d7', ''], ['y', '', '#ff00d7', '#0000ff'], ['z', '', '#010203', '#fafbfc'], ['w', 'f7', '', '#fafbfc'],
  ]);
});

test('each attribute is a class', () => {
  const sgr = { 1: 'b', 2: 'm', 3: 'i', 4: 'u', 9: 's', 8: 'v', 21: 'u' };
  for (const [code, cls] of Object.entries(sgr)) assert.deepEqual(runsOf(screenWith(`${ESC}[${code}mx`)), [['x', cls, '', '']], code);
  assert.deepEqual(runsOf(screenWith(`${ESC}[1;3;4mx`)), [['x', 'b i u', '', '']]);
  assert.deepEqual(runsOf(screenWith(`${ESC}[5mx`)), [['x', '', '', '']], 'blinking text does not blink');
});

test('inverse swaps the colours, and a default stands for the theme\'s own', () => {
  assert.deepEqual(runsOf(screenWith(`${ESC}[7mx`)), [['x', 'fb bf', '', '']]);
  assert.deepEqual(runsOf(screenWith(`${ESC}[31;7mx`)), [['x', 'fb g1', '', '']]);
  assert.deepEqual(runsOf(screenWith(`${ESC}[31;42;7mx`)), [['x', 'f2 g1', '', '']]);
  assert.deepEqual(runsOf(screenWith(`${ESC}[7;38;2;9;9;9mx`)), [['x', 'fb', '', '#090909']]);
});

test('runs of the same look are one, and another look starts another', () => {
  const runs = runsOf(screenWith(`ab${ESC}[1mcd${ESC}[22mef`));
  assert.deepEqual(runs, [['ab', '', '', ''], ['cd', 'b', '', ''], ['ef', '', '', '']]);
});

test('blanks of another look stay, to the end of the line', () => {
  const runs = runsOf(screenWith(`hi${ESC}[44m   ${ESC}[0m   `));
  assert.deepEqual(runs, [['hi', '', '', ''], ['   ', 'g4', '', '']]);
  assert.deepEqual(runsOf(screenWith(`${ESC}[4m  `)), [['  ', 'u', '', '']], 'an underlined blank shows');
});

test('a wide character stands alone and takes two cells, and its right half is not a run', () => {
  const runs = runsOf(screenWith('a\u4e2d\u6587b'));
  assert.deepEqual(runs, [['a', '', '', ''], ['\u4e2d', 'w', '', ''], ['\u6587', 'w', '', ''], ['b', '', '', '']]);
});

test('a character with its marks is one run of text', () => {
  assert.deepEqual(runsOf(screenWith('e\u0301x')), [['e\u0301x', '', '', '']]);
});

test('the selection marks the cells it covers, by whole characters', () => {
  const screen = screenWith('abcdef');
  const selection = { start: { line: screen.scrolled, col: 1 }, end: { line: screen.scrolled, col: 3 } };
  assert.deepEqual(runsOf(screen, 0, selected(screen, selection)), [['a', '', '', ''], ['bcd', 'sel', '', ''], ['ef', '', '', '']]);
  const wide = screenWith('a\u4e2db');
  const edge = { start: { line: wide.scrolled, col: 2 }, end: { line: wide.scrolled, col: 2 } };
  assert.deepEqual(runsOf(wide, 0, selected(wide, edge)), [['a', '', '', ''], ['\u4e2d', 'w sel', '', ''], ['b', '', '', '']]);
  assert.deepEqual(runsOf(screen, 1, selected(screen, selection, 1)), [], 'another line has none');
  const across = { start: { line: screen.scrolled, col: 4 }, end: { line: screen.scrolled + 1, col: 2 } };
  // up to the end of the line it runs past, blanks included
  assert.deepEqual(runsOf(screen, 0, selected(screen, across)), [['abcd', '', '', ''], ['ef      ', 'sel', '', '']]);
});

test('the cursor is a run of its own, with the class the page gives it', () => {
  const screen = screenWith('abc');
  assert.deepEqual(runsOf(screen, 0, { cursor: 1 }), [['a', '', '', ''], ['b', 'cur', '', ''], ['c', '', '', '']]);
  assert.deepEqual(runsOf(screen, 0, { cursor: 3, cursorClass: 'cur hollow' }), [['abc', '', '', ''], [' ', 'cur hollow', '', '']]);
  assert.deepEqual(runsOf(screen, 0, { cursor: 0 })[0], ['a', 'cur', '', '']);
  const wide = screenWith('\u4e2dx');
  assert.deepEqual(runsOf(wide, 0, { cursor: 0 }), [['\u4e2d', 'w cur', '', ''], ['x', '', '', '']]);
});

test('lines in the scrollback keep their looks', () => {
  const screen = screenWith(`${ESC}[32mgreen${ESC}[0m\r\nsecond\r\nthird\r\nfourth`, 12, 2);
  assert.equal(screen.ringCount, 2);
  assert.deepEqual(rowRuns(screen, screen.scrolled - 2).map((run) => [run.text, run.cls]), [['green', 'f2']]);
  assert.deepEqual(rowRuns(screen, screen.scrolled - 1).map((run) => [run.text, run.cls]), [['second', '']]);
});

// The dark theme's colours, as the page reads them from term.css.
const DARK = {
  fg: '#d7e3f4',
  bg: '#0a1220',
  colours: ['#3a465a', '#ff7b7b', '#5fd38a', '#e8c15a', '#6ca8ff', '#d68bef', '#4fd1d9', '#c9d4e3',
    '#7c8aa0', '#ffa3a3', '#86e8a8', '#f5d98a', '#9cc4ff', '#e7b0f7', '#8de6ec', '#f5f9ff'],
};
const readableOf = (screen, row = 0, options = {}) => readableRuns(rowRuns(screen, screen.scrolled + row, options), DARK)
  .map((run) => [run.text, run.cls, run.color, run.background]);

test('hard-to-read text gets a readable colour, in place of its class, and keeps its background', () => {
  // A root prompt's badge: bold bright white on yellow.
  const runs = readableOf(screenWith(`${ESC}[1;97;43m admin ${ESC}[0m #`));
  assert.equal(runs.length, 2);
  const [text, cls, color, background] = runs[0];
  assert.equal(text, ' admin ');
  assert.equal(cls, 'b g3');
  assert.ok(contrast(color, DARK.colours[3]) >= MINIMUM, color);
  assert.equal(background, '');
  assert.deepEqual(runs[1], [' #', '', '', '']);
});

test('readable text, the default look and inverse defaults are as rowRuns gave them', () => {
  const text = `a${ESC}[31mb${ESC}[44mc${ESC}[91;102md${ESC}[0me${ESC}[7mf`;
  const plain = runsOf(screenWith(text));
  const themed = readableOf(screenWith(text));
  // Red on blue and bright red on bright green are hard to read; the rest is as it was.
  assert.deepEqual(themed.map((run) => run[0]), plain.map((run) => run[0]));
  for (const [i, run] of themed.entries()) {
    if (run[0] === 'c' || run[0] === 'd') {
      assert.equal(run[1], plain[i][1].replace(/f\d+ ?/, ''), run[0]);
      assert.match(run[2], /^#[0-9a-f]{6}$/, run[0]);
    } else {
      assert.deepEqual(run, plain[i]);
    }
  }
});

test('black on the default background is lifted, and true colour on true colour too', () => {
  const [black] = readableOf(screenWith(`${ESC}[30mdark`));
  assert.equal(black[1], '');
  assert.ok(contrast(black[2], DARK.bg) >= MINIMUM, black[2]);
  const [grey] = readableOf(screenWith(`${ESC}[38;2;100;100;100;48;2;110;110;110mgrey`));
  assert.ok(contrast(grey[2], '#6e6e6e') >= MINIMUM, grey[2]);
  assert.equal(grey[3], '#6e6e6e');
});

test('dim, invisible, selected and cursor text, and blanks, keep their colours', () => {
  assert.deepEqual(readableOf(screenWith(`${ESC}[2;30mdim`)), [['dim', 'm f0', '', '']]);
  assert.deepEqual(readableOf(screenWith(`${ESC}[8;30mhid`)), [['hid', 'v f0', '', '']]);
  const screen = screenWith(`${ESC}[30mab`);
  assert.deepEqual(readableOf(screen, 0, { cursor: 0 })[0], ['a', 'f0 cur', '', '']);
  assert.deepEqual(readableOf(screen, 0, { span: { from: 0, to: 2 } }), [['ab', 'f0 sel', '', '']]);
  assert.deepEqual(readableOf(screenWith(`${ESC}[30;43m  ${ESC}[0mx`)), [['  ', 'f0 g3', '', ''], ['x', '', '', '']]);
});
