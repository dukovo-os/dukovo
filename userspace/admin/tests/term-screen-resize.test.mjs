// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { at, cell, CSI, ESC, feed, lineText, make, rows, scrollback } from './term-screen-helpers.mjs';

// Every line of the text, scrollback first, as the text of the lines that
// run into each other joined: what a program wrote, whatever the width.
function logical(screen) {
  const lines = [];
  let open = false;
  const total = screen.ringCount + screen.rows;
  for (let n = screen.scrolled - screen.ringCount; n < screen.scrolled - screen.ringCount + total; n++) {
    const line = screen.line(n);
    // A row that runs into the next is whole, blanks and all.
    let text = '';
    for (let k = 0; k < line.length; k++) text += screen.textOf(line.cells[line.offset + 3 * k]);
    if (!line.wrapped) text = text.trimEnd();
    if (open) lines[lines.length - 1] += text;
    else lines.push(text);
    open = line.wrapped;
  }
  while (lines.length && lines.at(-1) === '') lines.pop();
  return lines;
}

test('the same size changes nothing', () => {
  const screen = make(10, 4);
  feed(screen, 'abc');
  screen.clearDirty();
  const epoch = screen.epoch;
  screen.resize(10, 4);
  assert.equal(screen.epoch, epoch);
  assert.deepEqual([...screen.dirty], [0, 0, 0, 0]);
});

test('the size is kept inside the limits', () => {
  const screen = make(10, 4);
  screen.resize(1, 0);
  assert.deepEqual([screen.cols, screen.rows], [2, 1]);
  screen.resize(9999, 9999);
  assert.deepEqual([screen.cols, screen.rows], [500, 300]);
  assert.equal(screen.dirty.length, 300);
  assert.equal(screen.bottom, 299);
});

test('a resize marks every row changed and renumbers the lines', () => {
  const screen = make(10, 4);
  feed(screen, 'abc');
  screen.clearDirty();
  const epoch = screen.epoch;
  screen.resize(12, 5);
  assert.deepEqual([...screen.dirty], [1, 1, 1, 1, 1]);
  assert.equal(screen.epoch, epoch + 1);
});

test('widening joins a line that was wrapped', () => {
  const screen = make(10, 4);
  feed(screen, 'alpha beta gamma\r\n$ ');
  assert.deepEqual(rows(screen), ['alpha beta', ' gamma', '$', '']);
  screen.resize(20, 4);
  assert.deepEqual(rows(screen), ['alpha beta gamma', '$', '', '']);
  assert.deepEqual(at(screen), [2, 1]);
  assert.equal(screen.line(screen.scrolled).wrapped, false);
});

test('narrowing wraps a long line and the cursor stays on its character', () => {
  const screen = make(20, 4);
  feed(screen, 'alpha beta gamma\r\n$ ');
  screen.resize(10, 4);
  assert.deepEqual(rows(screen), ['alpha beta', ' gamma', '$', '']);
  assert.deepEqual(at(screen), [2, 2]);
  assert.equal(screen.line(screen.scrolled).wrapped, true);
  assert.equal(screen.line(screen.scrolled + 1).wrapped, false);
});

test('narrowing then widening gives the text back', () => {
  const screen = make(80, 6);
  feed(screen, 'one two three four five six seven eight nine ten eleven twelve thirteen fourteen\r\nshort\r\n$ ');
  const before = rows(screen);
  screen.resize(23, 6);
  screen.resize(7, 6);
  screen.resize(80, 6);
  assert.deepEqual(rows(screen), before);
  assert.deepEqual(at(screen), [2, 2]);
});

test('lines that no longer fit go to the scrollback and come back', () => {
  const screen = make(10, 4);
  feed(screen, 'alpha beta gamma delta\r\nsecond\r\n$ ');
  assert.deepEqual(scrollback(screen), ['alpha beta']);
  screen.resize(6, 3);
  assert.deepEqual(scrollback(screen), ['alpha', 'beta g', 'amma d']);
  assert.deepEqual(rows(screen), ['elta', 'second', '$']);
  assert.deepEqual(at(screen), [2, 2]);
  screen.resize(10, 6);
  assert.deepEqual(scrollback(screen), []);
  assert.deepEqual(rows(screen), ['alpha beta', ' gamma del', 'ta', 'second', '$', '']);
  assert.deepEqual(at(screen), [2, 4]);
});

test('a screen that grows taller pulls lines back from the scrollback', () => {
  const screen = make(10, 3);
  feed(screen, '1\r\n2\r\n3\r\n4\r\n5\r\n6');
  assert.deepEqual(scrollback(screen), ['1', '2', '3']);
  screen.resize(10, 5);
  assert.deepEqual(scrollback(screen), ['1']);
  assert.deepEqual(rows(screen), ['2', '3', '4', '5', '6']);
  assert.deepEqual(at(screen), [1, 4]);
});

test('a screen that shrinks keeps the cursor in view', () => {
  const screen = make(10, 5);
  feed(screen, '1\r\n2\r\n3\r\n4\r\n5');
  screen.resize(10, 2);
  assert.deepEqual(scrollback(screen), ['1', '2', '3']);
  assert.deepEqual(rows(screen), ['4', '5']);
  assert.deepEqual(at(screen), [1, 1]);
});

test('a cursor above the last text stays in place when the screen shrinks', () => {
  const screen = make(10, 5);
  feed(screen, '1\r\n2\r\n3\r\n4\r\n5');
  feed(screen, `${CSI}1;1H`);
  screen.resize(10, 3);
  assert.deepEqual(rows(screen), ['1', '2', '3']);
  assert.deepEqual(at(screen), [0, 0]);
  assert.equal(screen.ringCount, 0);
});

test('a cursor below the text keeps its row', () => {
  const screen = make(10, 6);
  feed(screen, `ab${CSI}5;3H`);
  screen.resize(7, 6);
  assert.deepEqual(rows(screen), ['ab', '', '', '', '', '']);
  assert.deepEqual(at(screen), [2, 4]);
  screen.resize(7, 4);
  assert.deepEqual(at(screen), [2, 3]);
  assert.equal(screen.ringCount, 1);
  assert.deepEqual(scrollback(screen), ['ab']);
});

test('a pending wrap is kept when the cursor is still on the last column', () => {
  const screen = make(10, 4);
  feed(screen, 'abcdefghij');
  assert.equal(screen.pendingWrap, true);
  screen.resize(10, 6);
  assert.deepEqual(at(screen), [9, 0]);
  assert.equal(screen.pendingWrap, true);
  screen.resize(12, 6);
  assert.deepEqual(at(screen), [10, 0]);
  assert.equal(screen.pendingWrap, false);
  feed(screen, 'k');
  assert.deepEqual(rows(screen)[0], 'abcdefghijk');
});

test('a pending wrap becomes a plain position when the screen gets wider, and the other way', () => {
  const screen = make(10, 4);
  feed(screen, 'abcdefghij');
  screen.resize(5, 4);
  assert.deepEqual(rows(screen), ['abcde', 'fghij', '', '']);
  assert.deepEqual(at(screen), [4, 1]);
  assert.equal(screen.pendingWrap, true);
  feed(screen, 'k');
  assert.deepEqual(rows(screen), ['abcde', 'fghij', 'k', '']);
});

test('a cursor in the middle of a line stays on its character', () => {
  const screen = make(20, 3);
  feed(screen, `0123456789abcdefghij${CSI}1;13H`);
  screen.resize(5, 6);
  assert.deepEqual(rows(screen).slice(0, 4), ['01234', '56789', 'abcde', 'fghij']);
  assert.deepEqual(at(screen), [2, 2]);
  assert.equal(screen.pendingWrap, false);
  screen.resize(20, 3);
  assert.deepEqual(at(screen), [12, 0]);
});

test('a wide character is not cut by the new width', () => {
  const screen = make(10, 4);
  feed(screen, 'abcd中文xy');
  screen.resize(5, 4);
  assert.deepEqual(rows(screen), ['abcd', '中文x', 'y', '']);
  assert.equal(screen.line(screen.scrolled).wrapped, true);
  assert.deepEqual([cell(screen, 0, 1).width, cell(screen, 1, 1).width], [2, 0]);
  screen.resize(10, 4);
  assert.deepEqual(rows(screen), ['abcd中文xy', '', '', '']);
});

test('a width of two keeps wide characters whole', () => {
  const screen = make(6, 3);
  feed(screen, '中文字');
  screen.resize(2, 6);
  assert.deepEqual(rows(screen).slice(0, 3), ['中', '文', '字']);
});

test('combining marks and colours go with their cells', () => {
  const screen = make(8, 3);
  feed(screen, `${CSI}1;31mab${CSI}0mécdefgh`);
  screen.resize(4, 4);
  assert.deepEqual(rows(screen).slice(0, 3), ['abe\u0301c', 'defg', 'h']);
  assert.deepEqual(cell(screen, 0, 0).fg, [1, 1]);
  assert.equal(cell(screen, 0, 0).flags & 1, 1);
  assert.deepEqual(cell(screen, 2, 0).fg, [0, 0]);
});

test('colours of blanks survive a resize', () => {
  const screen = make(8, 3);
  feed(screen, `${CSI}44m  ${CSI}0mx`);
  screen.resize(5, 3);
  assert.deepEqual(cell(screen, 0, 0).bg, [1, 4]);
  assert.deepEqual(cell(screen, 1, 0).bg, [1, 4]);
  assert.deepEqual(cell(screen, 2, 0).bg, [0, 0]);
});

test('the text does not change however the screen is resized', () => {
  const screen = make(30, 5);
  const text = ['the quick brown fox jumps over the lazy dog again and again', 'short', '', 'x'.repeat(65), '$ '];
  feed(screen, text.join('\r\n'));
  const before = logical(screen);
  for (const [cols, rows_] of [[7, 3], [40, 9], [2, 5], [13, 2], [90, 4], [30, 5]]) {
    screen.resize(cols, rows_);
    assert.deepEqual(logical(screen), before, `${cols}x${rows_}`);
    assert.ok(screen.x < cols && screen.y < rows_);
  }
});

test('the scrollback keeps no more lines than its limit', () => {
  const screen = make(10, 3, { scrollback: 3 });
  feed(screen, 'aaaaabbbbbcccccdddddeeeee\r\nf\r\ng');
  screen.resize(5, 3);
  assert.equal(screen.ringCount, 3);
  assert.deepEqual(rows(screen), ['eeeee', 'f', 'g']);
  assert.deepEqual(scrollback(screen), ['bbbbb', 'ccccc', 'ddddd']);
  assert.equal(screen.scrolled, screen.ringCount);
});

test('a screen with no scrollback loses what does not fit', () => {
  const screen = make(10, 3, { scrollback: 0 });
  feed(screen, '1\r\n2\r\n3');
  screen.resize(10, 2);
  assert.deepEqual(rows(screen), ['2', '3']);
  assert.equal(screen.ringCount, 0);
});

test('the scroll region and wrap are reset, the cursor stays inside', () => {
  const screen = make(10, 6);
  feed(screen, `${CSI}2;4r${CSI}4;1Hx`);
  screen.resize(8, 5);
  assert.deepEqual([screen.top, screen.bottom], [0, 4]);
  assert.ok(screen.y < 5);
});

test('tab stops stay where they were set and the new columns get the usual ones', () => {
  const screen = make(10, 3);
  feed(screen, `${CSI}3g${CSI}1;5H${ESC}H`);
  screen.resize(20, 3);
  feed(screen, `${CSI}1;1H\t`);
  assert.equal(screen.x, 4);
  feed(screen, '\t');
  assert.equal(screen.x, 16);
  screen.resize(6, 3);
  feed(screen, `${CSI}1;1H\t`);
  assert.equal(screen.x, 4);
  feed(screen, '\t');
  assert.equal(screen.x, 5);
});

test('the alternate screen is cut and padded, and the main one is reflowed behind it', () => {
  const screen = make(10, 4);
  feed(screen, `main text here\r\n$ ${CSI}?1049h${CSI}2J${CSI}1;1Habcdefghij${CSI}4;1Hlast`);
  assert.equal(screen.alternate, true);
  screen.resize(6, 3);
  assert.deepEqual(rows(screen), ['abcdef', '', '']);
  assert.deepEqual(at(screen), [4, 2]);
  assert.equal(screen.pendingWrap, false);
  screen.resize(12, 5);
  assert.deepEqual(rows(screen), ['abcdef', '', '', '', '']);
  feed(screen, `${CSI}?1049l`);
  assert.equal(screen.alternate, false);
  assert.deepEqual(rows(screen), ['main text he', 're', '$', '', '']);
  assert.deepEqual(at(screen), [2, 2]);
});

test('a wide character cut in two by the alternate screen leaves a blank', () => {
  const screen = make(6, 3);
  feed(screen, `${CSI}?1049h${CSI}1;1Hab中文`);
  screen.resize(3, 3);
  assert.deepEqual(rows(screen)[0], 'ab');
  assert.equal(cell(screen, 2, 0).width, 1);
  assert.equal(cell(screen, 2, 0).text, ' ');
});

test('the main cursor saved by the alternate screen follows the text', () => {
  const screen = make(10, 4);
  feed(screen, `0123456789ab${CSI}?1049h`);
  screen.resize(5, 4);
  feed(screen, `${CSI}?1049l`);
  assert.deepEqual(rows(screen), ['01234', '56789', 'ab', '']);
  assert.deepEqual(at(screen), [2, 2]);
});

test('with nothing saved, a resize on the alternate screen keeps the main text', () => {
  const screen = make(10, 3);
  feed(screen, `0123456789ab\r\ncd${ESC}[?47h`);
  assert.equal(screen.alternate, true);
  screen.resize(5, 3);
  feed(screen, `${CSI}?47l`);
  assert.deepEqual(scrollback(screen), ['01234']);
  assert.deepEqual(rows(screen), ['56789', 'ab', 'cd']);
});

test('saved cursors fit the new size', () => {
  const screen = make(10, 6);
  feed(screen, `${CSI}6;9H${ESC}7${CSI}1;1H`);
  screen.resize(4, 3);
  feed(screen, ESC + '8');
  assert.deepEqual(at(screen), [3, 2]);
  assert.equal(screen.pendingWrap, false);
});

test('a saved cursor of the screen not shown is kept too', () => {
  const screen = make(10, 6);
  feed(screen, `${CSI}?1049h${CSI}6;9H${ESC}7${CSI}?1049l`);
  screen.resize(4, 3);
  feed(screen, `${CSI}?1049h`);
  feed(screen, ESC + '8');
  assert.deepEqual(at(screen), [3, 2]);
});

test('typing after a resize goes where the cursor is', () => {
  const screen = make(10, 3);
  feed(screen, 'hello world');
  screen.resize(6, 3);
  feed(screen, '!');
  assert.deepEqual(rows(screen), ['hello', 'world!', '']);
  screen.resize(20, 3);
  assert.deepEqual(rows(screen), ['hello world!', '', '']);
});

test('an empty screen resizes with the cursor at home', () => {
  const screen = make(10, 4);
  screen.resize(3, 2);
  assert.deepEqual(at(screen), [0, 0]);
  assert.deepEqual(rows(screen), ['', '']);
  screen.resize(30, 20);
  assert.deepEqual(at(screen), [0, 0]);
});

test('the blank a wide character leaves at the end of a row is not text', () => {
  const screen = make(5, 3);
  feed(screen, 'abcd中');
  assert.deepEqual(rows(screen), ['abcd', '中', '']);
  const first = screen.line(screen.scrolled);
  assert.deepEqual([first.length, first.wrapped], [4, true]);
  screen.resize(10, 3);
  assert.deepEqual(rows(screen), ['abcd中', '', '']);
  screen.resize(5, 3);
  assert.deepEqual(rows(screen), ['abcd', '中', '']);
  assert.deepEqual([screen.line(screen.scrolled).length, screen.line(screen.scrolled).wrapped], [4, true]);
  screen.resize(5, 1);
  assert.deepEqual(scrollback(screen), ['abcd']);
  assert.equal(screen.line(screen.scrolled - 1).length, 4);
  assert.equal(screen.line(screen.scrolled - 1).wrapped, true);
});
