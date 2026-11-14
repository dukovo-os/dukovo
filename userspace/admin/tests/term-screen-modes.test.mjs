// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's modes, the alternate screen, the saved cursor, the
// character sets, the resets, and the answers it gives to a program's
// questions.

import assert from 'node:assert/strict';
import test from 'node:test';

import { COLOUR, FLAGS } from '../pages/terminal/term/screen.js';
import { CSI, ESC, at, cell, feed, make, rows, scrollback, spelled } from './term-screen-helpers.mjs';

// A screen of 5 by 4 with 'aaaaa' to 'ddddd' on its rows, the cursor at the origin.
function filled() {
  const screen = make(5, 4);
  feed(screen, `${CSI}H`, 'aaaaa\r\nbbbbb\r\nccccc\r\nddddd', `${CSI}H`);
  return screen;
}

test('mode 1 is the cursor keys, and ESC = and ESC > are the keypad', () => {
  const screen = make();
  assert.deepEqual([screen.applicationCursor, screen.applicationKeypad], [false, false]);
  feed(screen, `${CSI}?1h${ESC}=`);
  assert.deepEqual([screen.applicationCursor, screen.applicationKeypad], [true, true]);
  feed(screen, `${CSI}?1l${ESC}>`);
  assert.deepEqual([screen.applicationCursor, screen.applicationKeypad], [false, false]);
});

test('mode 25 shows and hides the cursor', () => {
  const screen = make();
  assert.equal(screen.cursorVisible, true);
  feed(screen, `${CSI}?25l`);
  assert.equal(screen.cursorVisible, false);
  feed(screen, `${CSI}?25h`);
  assert.equal(screen.cursorVisible, true);
});

test('modes 1000, 1002 and 1006 are the mouse, and 2004 the paste', () => {
  const screen = make();
  assert.deepEqual([screen.mouseTracking, screen.mouseSgr, screen.bracketedPaste], [0, false, false]);
  feed(screen, `${CSI}?1000h`);
  assert.equal(screen.mouseTracking, 1000);
  feed(screen, `${CSI}?1002h`);
  assert.equal(screen.mouseTracking, 1002);
  feed(screen, `${CSI}?1006h${CSI}?2004h`);
  assert.deepEqual([screen.mouseSgr, screen.bracketedPaste], [true, true]);
  feed(screen, `${CSI}?1002l`);
  assert.equal(screen.mouseTracking, 0);
  feed(screen, `${CSI}?1000h${CSI}?1000l${CSI}?1006l${CSI}?2004l`);
  assert.deepEqual([screen.mouseTracking, screen.mouseSgr, screen.bracketedPaste], [0, false, false]);
});

test('several modes in one sequence are each set, and one this terminal does not know is skipped', () => {
  const screen = make();
  feed(screen, `${CSI}?1;12;25;2004h`);
  assert.deepEqual([screen.applicationCursor, screen.bracketedPaste], [true, true]);
  feed(screen, `${CSI}?12l${CSI}?25l`);
  assert.equal(screen.cursorVisible, false);
  feed(screen, `${CSI}?3;4l${CSI}?1034h${CSI}?69h`);
  assert.equal(screen.autoWrap, true);
});

test('ANSI modes other than insert are ignored', () => {
  const screen = make(5, 2);
  feed(screen, `${CSI}20h${CSI}12l`, 'ab\nc');
  assert.deepEqual(rows(screen), ['ab', '  c']);
});

test('mode 7 off makes the last column take every character that comes', () => {
  const screen = make(5, 2);
  feed(screen, `${CSI}?7l`, 'abcdefg');
  assert.deepEqual(rows(screen), ['abcdg', '']);
  assert.deepEqual(at(screen), [4, 0]);
  // The wait to wrap is still there, as xterm has it, and ends when wrapping is on again.
  assert.equal(screen.pendingWrap, true);
  feed(screen, `${CSI}?7h`, 'h');
  assert.deepEqual(rows(screen), ['abcdg', 'h']);
});

test('turning mode 7 off or on does not end a wait to wrap', () => {
  const off = make(5, 2);
  feed(off, 'abcde', `${CSI}?7lf`);
  assert.deepEqual(rows(off), ['abcdf', '']);
  const on = make(5, 2);
  feed(on, 'abcde', `${CSI}?7l${CSI}?7hf`);
  assert.deepEqual(rows(on), ['abcde', 'f']);
});

test('a wide character that does not fit is dropped when wrapping is off', () => {
  const screen = make(5, 2);
  feed(screen, `${CSI}?7l`, 'abcd中e');
  assert.deepEqual(rows(screen), ['abcde', '']);
});

test('with wrapping off a wide character in the last two cells is overwritten by the next', () => {
  const screen = make(5, 2);
  feed(screen, `${CSI}?7l`, 'abc中d');
  assert.deepEqual(rows(screen), ['abc d', '']);
  assert.deepEqual(Array.from({ length: 5 }, (_, x) => cell(screen, x, 0).width), [1, 1, 1, 1, 1]);
});

test('insert mode pushes the rest of the row right as characters are drawn', () => {
  const screen = make(6, 2);
  feed(screen, 'abcdef\r', `${CSI}4hXY`);
  assert.deepEqual(rows(screen)[0], 'XYabcd');
  feed(screen, `${CSI}4l\rZ`);
  assert.deepEqual(rows(screen)[0], 'ZYabcd');
});

test('insert mode with a wide character makes two cells of room, and cuts nothing in half', () => {
  const screen = make(6, 2);
  feed(screen, 'ab中c\r', `${CSI}4h中`);
  assert.deepEqual(Array.from({ length: 6 }, (_, x) => cell(screen, x, 0).width), [2, 0, 1, 1, 2, 0]);
  const cut = make(5, 2);
  feed(cut, 'abc中\r', `${CSI}4hx`);
  assert.deepEqual(Array.from({ length: 5 }, (_, x) => cell(cut, x, 0).width), [1, 1, 1, 1, 1]);
  assert.deepEqual(rows(cut)[0], 'xabc');
});

test('origin mode counts rows from the top of the region and keeps the cursor in it', () => {
  const screen = filled();
  feed(screen, `${CSI}2;3r`);
  feed(screen, `${CSI}?6h`);
  assert.deepEqual(at(screen), [0, 1]);
  feed(screen, `${CSI}1;1H`);
  assert.deepEqual(at(screen), [0, 1]);
  feed(screen, `${CSI}9;3H`);
  assert.deepEqual(at(screen), [2, 2]);
  feed(screen, `${CSI}2dx`);
  assert.deepEqual(at(screen), [3, 2]);
  assert.deepEqual(rows(screen)[2], 'ccxcc');
  feed(screen, `${CSI}9A`);
  assert.equal(screen.y, 1);
  feed(screen, `${CSI}3;4r`);
  assert.deepEqual(at(screen), [0, 2]);
});

test('turning origin mode off sends the cursor to the corner of the screen', () => {
  const screen = filled();
  feed(screen, `${CSI}2;3r${CSI}?6h${CSI}?6l`);
  assert.deepEqual(at(screen), [0, 0]);
  feed(screen, `${CSI}3;1H`);
  assert.deepEqual(at(screen), [0, 2]);
});

test('the alternate screen starts clear, keeps the cursor, and gives the main screen back', () => {
  const screen = make(8, 3);
  feed(screen, 'main text', `${CSI}2;3H`);
  feed(screen, `${CSI}?1049h`);
  assert.equal(screen.alternate, true);
  assert.deepEqual(rows(screen), ['', '', '']);
  assert.deepEqual(at(screen), [2, 1]);
  feed(screen, `${CSI}Halt${CSI}?1049l`);
  assert.equal(screen.alternate, false);
  assert.deepEqual(rows(screen), ['main tex', 't', '']);
  assert.deepEqual(at(screen), [2, 1]);
});

test('mode 1049 gives the cursor and the attributes of the main screen back', () => {
  const screen = make(8, 3);
  feed(screen, `${CSI}2;3H${CSI}1;31m`, `${CSI}?1049h`, `${CSI}3;6H${CSI}0m`, `${CSI}?1049l`);
  assert.deepEqual(at(screen), [2, 1]);
  feed(screen, 'x');
  assert.deepEqual([cell(screen, 2, 1).flags, cell(screen, 2, 1).fg], [FLAGS.BOLD, [COLOUR.PALETTE, 1]]);
});

test('mode 1049 clears the alternate screen each time it is entered', () => {
  const screen = make(8, 3);
  feed(screen, `${CSI}?1049hfirst${CSI}?1049l${CSI}?1049h`);
  assert.deepEqual(rows(screen), ['', '', '']);
});

test('the alternate screen has no scrollback, and the main screen\'s is out of sight', () => {
  const screen = make(5, 2, { scrollback: 10 });
  feed(screen, 'a\r\nb\r\nc');
  assert.deepEqual(scrollback(screen), ['a']);
  const first = screen.firstLine;
  feed(screen, `${CSI}?1049h`, '1\r\n2\r\n3\r\n4\r\n5');
  assert.equal(screen.ringCount, 1);
  assert.equal(screen.line(screen.scrolled - 1), null);
  assert.equal(screen.firstLine, screen.scrolled);
  assert.deepEqual(rows(screen), ['4', '5']);
  feed(screen, `${CSI}?1049l`);
  assert.equal(screen.firstLine, first);
  assert.deepEqual(scrollback(screen), ['a']);
  assert.deepEqual(rows(screen), ['b', 'c']);
});

test('lines scrolled off the alternate screen are lost', () => {
  const screen = make(5, 2, { scrollback: 10 });
  feed(screen, `${CSI}?1049h`, '1\r\n2\r\n3');
  assert.equal(screen.scrolled, 0);
});

test('mode 47 switches without clearing or saving anything', () => {
  const screen = make(8, 3);
  feed(screen, 'main', `${CSI}?47h`);
  assert.deepEqual(rows(screen), ['', '', '']);
  assert.deepEqual(at(screen), [4, 0]);
  feed(screen, `${CSI}Hin alt${CSI}?47l`);
  assert.deepEqual(rows(screen), ['main', '', '']);
  assert.deepEqual(at(screen), [6, 0]);
  feed(screen, `${CSI}?47h`);
  assert.deepEqual(rows(screen), ['in alt', '', '']);
});

test('mode 1047 clears the alternate screen when it is left, not when it is entered', () => {
  const screen = make(8, 3);
  feed(screen, `${CSI}?1047hwork${CSI}?1047l`);
  assert.deepEqual(rows(screen), ['', '', '']);
  feed(screen, `${CSI}H`, 'main', `${CSI}?1047h`);
  assert.deepEqual(rows(screen), ['', '', '']);
  feed(screen, `${CSI}?1047l`);
  assert.deepEqual(rows(screen), ['main', '', '']);
  feed(screen, `${CSI}?1047l`);
  assert.deepEqual(rows(screen), ['main', '', '']);
});

test('mode 1049 acts each time, as in xterm: off from the main screen restores the cursor, on twice clears again', () => {
  const screen = make(8, 3);
  feed(screen, `${CSI}2;3Hab${CSI}?1049l`);
  assert.deepEqual(at(screen), [0, 0]);
  assert.deepEqual(rows(screen), ['', '  ab', '']);
  feed(screen, `${CSI}2;2H${ESC}7${CSI}3;3H${CSI}?1049l`);
  assert.deepEqual(at(screen), [1, 1]);
  // the second 1049h saves on the alternate screen, so the way back is the first
  feed(screen, `${CSI}?1049h${CSI}1;1Hnew`);
  screen.clearDirty();
  feed(screen, `${CSI}?1049h`);
  assert.deepEqual([screen.alternate, rows(screen), at(screen)], [true, ['', '', ''], [3, 0]]);
  assert.deepEqual([...screen.dirty], [1, 1, 1]);
  feed(screen, `${CSI}3;3H${CSI}?1049l`);
  assert.deepEqual([screen.alternate, at(screen)], [false, [1, 1]]);
});

test('the scroll region stays when the screen changes, as in xterm', () => {
  for (const mode of [47, 1047, 1049]) {
    const screen = make(8, 4);
    feed(screen, `${CSI}2;3r${CSI}?${mode}h`);
    assert.deepEqual([screen.top, screen.bottom], [1, 2], `${mode} on`);
    feed(screen, `${CSI}?${mode}l`);
    assert.deepEqual([screen.top, screen.bottom], [1, 2], `${mode} off`);
  }
  // so a move down from above the region stops at its bottom on the other screen
  const screen = make(8, 8);
  feed(screen, `${CSI}2;6r${CSI}?47h${CSI}1;1H${CSI}9B`);
  assert.deepEqual(at(screen), [0, 5]);
});

test('ESC 7 and ESC 8 keep and bring back the cursor, the attributes and the character sets', () => {
  const screen = make(10, 3);
  feed(screen, `${CSI}1;32;45m${CSI}2;4H${ESC}(0${ESC}7`);
  feed(screen, `${CSI}0m${CSI}1;1H${ESC}(B${ESC}8x`);
  const kept = cell(screen, 3, 1);
  assert.deepEqual([kept.text, kept.flags, kept.fg, kept.bg], ['│', FLAGS.BOLD, [COLOUR.PALETTE, 2], [COLOUR.PALETTE, 5]]);
});

test('ESC 8 brings back a wait to wrap, and origin mode', () => {
  const screen = make(5, 4);
  feed(screen, 'abcde', `${ESC}7`, `${CSI}H`, `${ESC}8`);
  assert.equal(screen.pendingWrap, true);
  feed(screen, `${CSI}2;3r${CSI}?6h${CSI}2;2H${ESC}7${CSI}?6l${ESC}8`);
  assert.equal(screen.originMode, true);
  assert.deepEqual(at(screen), [1, 2]);
});

test('CSI s and CSI u do the same, and with parameters or a marker they do nothing', () => {
  const screen = make(10, 3);
  feed(screen, `${CSI}2;4H${CSI}s${CSI}H${CSI}u`);
  assert.deepEqual(at(screen), [3, 1]);
  feed(screen, `${CSI}3;3H${CSI}1;5s${CSI}H${CSI}u`);
  assert.deepEqual(at(screen), [3, 1]);
  feed(screen, `${CSI}H${CSI}?u${CSI}>1u${CSI}1u`);
  assert.deepEqual(at(screen), [0, 0]);
});

test('with no saved cursor, ESC 8 goes home with the attributes of a new terminal', () => {
  const screen = make(10, 3);
  feed(screen, `${CSI}1;31m${CSI}2;4H${ESC}(0${ESC}8`);
  assert.deepEqual(at(screen), [0, 0]);
  feed(screen, 'q');
  const x = cell(screen, 0, 0);
  assert.deepEqual([x.text, x.flags, x.fg[0]], ['q', 0, COLOUR.DEFAULT]);
});

test('each screen keeps its own saved cursor', () => {
  const screen = make(10, 3);
  feed(screen, `${CSI}1;4H${ESC}7`, `${CSI}?47h`, `${CSI}3;9H${ESC}7`, `${CSI}1;1H${ESC}8`);
  assert.deepEqual(at(screen), [8, 2]);
  feed(screen, `${CSI}?47l${CSI}2;2H${ESC}8`);
  assert.deepEqual(at(screen), [3, 0]);
});

test('a saved cursor that is below a smaller region is brought into it with origin mode on', () => {
  const screen = make(10, 5);
  feed(screen, `${CSI}2;4r${CSI}?6h${CSI}3;1H${ESC}7`);
  assert.equal(screen.y, 3);
  feed(screen, `${CSI}2;3r${ESC}8`);
  assert.deepEqual(at(screen), [0, 2]);
  feed(screen, `${CSI}?6l${CSI}5;1H${ESC}7${CSI}1;2r${ESC}8`);
  assert.equal(screen.y, 4);
});

test('mode 1048 saves and restores the cursor alone', () => {
  const screen = make(10, 3);
  feed(screen, `${CSI}2;5H${CSI}?1048h${CSI}H${CSI}?1048l`);
  assert.deepEqual(at(screen), [4, 1]);
});

test('ESC ( 0 draws with the line characters, and ESC ( B with ASCII again', () => {
  const screen = make(10, 2);
  feed(screen, `${ESC}(0lqqk${ESC}(Bx`);
  assert.deepEqual(rows(screen)[0], '┌──┐x');
});

test('every character of the line drawing set maps, and what is outside its range does not', () => {
  const screen = make(40, 2);
  feed(screen, `${ESC}(0`, '_`abcdefghijklmnopqrstuvwxyz{|}~', '0A[é');
  assert.equal(rows(screen)[0], ' ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·0A[é');
});

test('SO and SI choose between the two sets, and ESC ) 0 sets the second', () => {
  const screen = make(10, 2);
  feed(screen, `${ESC})0q\x0eq\x0fq`);
  assert.deepEqual(rows(screen)[0], 'q─q');
  feed(screen, `\r${ESC}(0${ESC})Bq\x0eq`);
  assert.deepEqual(rows(screen)[0], '─qq');
});

test('a character set that is not here leaves the slot as it was', () => {
  const screen = make(10, 2);
  feed(screen, `${ESC}(0${ESC}(Aq${ESC})3q${ESC}(%5q`);
  assert.deepEqual(rows(screen)[0], '───');
});

test('DECSCUSR sets the cursor\'s shape and whether it blinks', () => {
  const screen = make();
  const style = (n) => {
    feed(screen, `${CSI}${n} q`);
    return [screen.cursorShape, screen.cursorBlink];
  };
  assert.deepEqual(style(1), ['block', true]);
  assert.deepEqual(style(2), ['block', false]);
  assert.deepEqual(style(3), ['underline', true]);
  assert.deepEqual(style(4), ['underline', false]);
  assert.deepEqual(style(5), ['bar', true]);
  assert.deepEqual(style(6), ['bar', false]);
  assert.deepEqual(style(0), ['block', true]);
  feed(screen, `${CSI}4 q${CSI} q`);
  assert.deepEqual([screen.cursorShape, screen.cursorBlink], ['block', true]);
  feed(screen, `${CSI}4 q${CSI}7 q`);
  assert.deepEqual([screen.cursorShape, screen.cursorBlink], ['underline', false]);
  feed(screen, `${CSI}?2 q${CSI}2 !q`);
  assert.equal(screen.cursorShape, 'underline');
});

test('DECSTR undoes the modes and attributes and keeps the screen and the cursor', () => {
  const screen = filled();
  feed(screen, `${CSI}2;3r${CSI}?6h${CSI}4h${CSI}?7l${CSI}?25l${CSI}?1h${ESC}=${CSI}1;31m${ESC}(0${ESC}7${CSI}4 q`);
  feed(screen, `${CSI}?2004h${CSI}?1006h${CSI}?1000h`);
  feed(screen, `${CSI}2;2H${CSI}!p`);
  assert.deepEqual([screen.originMode, screen.insertMode, screen.autoWrap, screen.cursorVisible], [false, false, true, true]);
  assert.deepEqual([screen.applicationCursor, screen.applicationKeypad], [false, false]);
  assert.deepEqual([screen.top, screen.bottom, screen.flags, screen.fg, screen.charsets[0]], [0, 3, 0, 0, 'B']);
  assert.deepEqual([screen.cursorShape, screen.cursorBlink], ['block', true]);
  assert.deepEqual([screen.bracketedPaste, screen.mouseSgr, screen.mouseTracking], [true, true, 1000]);
  assert.deepEqual(rows(screen), ['aaaaa', 'bbbbb', 'ccccc', 'ddddd']);
  assert.deepEqual(at(screen), [1, 2]);
  feed(screen, `${ESC}8`);
  assert.deepEqual(at(screen), [0, 0]);
});

test('DECSTR with a marker or another intermediate does nothing', () => {
  const screen = make();
  feed(screen, `${CSI}?25l${CSI}?!p${CSI}$p`);
  assert.equal(screen.cursorVisible, false);
});

test('RIS clears everything, leaves the alternate screen and keeps the scrollback', () => {
  const screen = make(8, 3, { scrollback: 10 });
  feed(screen, 'a\r\nb\r\nc\r\nd', `${CSI}1;31m${CSI}2;3r${CSI}?6h${CSI}?1000h${CSI}?2004h${CSI}?25l${ESC}=${ESC}(0`);
  feed(screen, `${CSI}?1049hxyz${ESC}]0;title\x07`);
  feed(screen, `${ESC}c`);
  assert.equal(screen.alternate, false);
  assert.deepEqual(rows(screen), ['', '', '']);
  assert.deepEqual(scrollback(screen), ['a']);
  assert.deepEqual(at(screen), [0, 0]);
  assert.deepEqual([screen.top, screen.bottom, screen.flags, screen.fg], [0, 2, 0, 0]);
  assert.deepEqual([screen.originMode, screen.mouseTracking, screen.bracketedPaste, screen.cursorVisible, screen.applicationKeypad], [false, 0, false, true, false]);
  assert.deepEqual(screen.charsets, ['B', 'B']);
  assert.equal(screen.title, 'title');
  feed(screen, `${CSI}?1049h`);
  assert.deepEqual(rows(screen), ['', '', '']);
});

test('RIS puts the tab stops back, and forgets the saved cursors', () => {
  const screen = make(20, 3);
  feed(screen, `${CSI}3g${CSI}6G${ESC}H${ESC}7${ESC}c\t`);
  assert.equal(screen.x, 8);
  feed(screen, `${CSI}3;3H${ESC}8`);
  assert.deepEqual(at(screen), [0, 0]);
});

test('reset in code does what RIS does, drops a half-read sequence, and may drop the scrollback', () => {
  const screen = make(8, 3, { scrollback: 10 });
  feed(screen, 'a\r\nb\r\nc\r\nd', `${ESC}[3`);
  const epoch = screen.epoch;
  screen.reset();
  feed(screen, 'x');
  assert.deepEqual(rows(screen), ['x', '', '']);
  assert.deepEqual(scrollback(screen), ['a']);
  assert.ok(screen.epoch > epoch);
  screen.reset({ clearScrollback: true });
  assert.deepEqual(scrollback(screen), []);
});

test('the epoch changes when line numbers would mean something else', () => {
  const screen = make(8, 3);
  const epoch = screen.epoch;
  feed(screen, 'abc\r\n\r\n\r\ndef');
  assert.equal(screen.epoch, epoch);
  feed(screen, `${CSI}?1049h`);
  assert.equal(screen.epoch, epoch + 1);
  feed(screen, `${CSI}?1049l`);
  assert.equal(screen.epoch, epoch + 2);
});

test('DSR 5 says the terminal is working', () => {
  const screen = make();
  assert.equal(spelled(feed(screen, `${CSI}5n`)), `${ESC}[0n`);
});

test('DSR 6 says where the cursor is, counting from 1', () => {
  const screen = make(10, 5);
  assert.equal(spelled(feed(screen, `${CSI}6n`)), `${ESC}[1;1R`);
  assert.equal(spelled(feed(screen, `${CSI}3;7H${CSI}6n`)), `${ESC}[3;7R`);
  feed(screen, `${CSI}1;10H`);
  assert.equal(spelled(feed(screen, 'x', `${CSI}6n`)), `${ESC}[1;10R`);
});

test('DSR 6 counts from the top of the region with origin mode on', () => {
  const screen = make(10, 5);
  feed(screen, `${CSI}2;4r${CSI}?6h${CSI}2;3H`);
  assert.equal(spelled(feed(screen, `${CSI}6n`)), `${ESC}[2;3R`);
});

test('other DSR questions get no answer', () => {
  const screen = make();
  assert.equal(feed(screen, `${CSI}n${CSI}7n${CSI}?6n${CSI}?15n`).length, 0);
});

test('DA1 is answered as a VT100 with video attributes, as xterm does', () => {
  const screen = make();
  assert.equal(spelled(feed(screen, `${CSI}c`)), `${ESC}[?1;2c`);
  assert.equal(spelled(feed(screen, `${CSI}0c`)), `${ESC}[?1;2c`);
  assert.equal(feed(screen, `${CSI}1c${CSI}=c`).length, 0);
});

test('DA2 is answered with the terminal\'s patch level', () => {
  const screen = make();
  assert.equal(spelled(feed(screen, `${CSI}>c`)), `${ESC}[>0;277;0c`);
  assert.equal(spelled(feed(screen, `${CSI}>0c`)), `${ESC}[>0;277;0c`);
  assert.equal(feed(screen, `${CSI}>1c${CSI}>5;5c`).length, 0);
});

test('the answers of one write come back together and in order, and only once', () => {
  const screen = make();
  const answer = feed(screen, `${CSI}5n${CSI}c${CSI}6n`);
  assert.equal(spelled(answer), `${ESC}[0n${ESC}[?1;2c${ESC}[1;1R`);
  assert.equal(screen.write(new Uint8Array(0)).length, 0);
  assert.ok(screen.write(new Uint8Array(0)) instanceof Uint8Array);
});

test('a flood of questions gets a bounded answer', () => {
  const screen = make();
  const answer = feed(screen, `${CSI}c`.repeat(10000));
  assert.ok(answer.length <= 4096);
  assert.ok(answer.length > 0);
});

test('the answers are for a program, so they are not drawn', () => {
  const screen = make();
  feed(screen, `${CSI}6n${CSI}c`);
  assert.deepEqual(rows(screen), ['', '', '', '']);
});
