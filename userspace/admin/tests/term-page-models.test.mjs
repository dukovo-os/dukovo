// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Terminal page's small models: when the socket is tried again, what
// pasting does, the key bar's sticky keys, and what a screen reader is told.

import assert from 'node:assert/strict';
import test from 'node:test';

import { Announcer, EVERY_MS, MOST_CHARS } from '../pages/terminal/announce-model.js';
import { KEYS, KeyBar } from '../pages/terminal/keybar-model.js';
import {
  BRACKET_END, BRACKET_START, MOST_BYTES, linesOf, pasteOf,
} from '../pages/terminal/paste-model.js';
import { SESSION_ENDED, delayFor, retries, statusWords } from '../pages/terminal/reconnect-model.js';

const text = (bytes) => new TextDecoder().decode(bytes);

// The reconnect schedule.

test('the socket is tried again after 1, 2, 5 and 10 seconds, then every 30', () => {
  assert.deepEqual([1, 2, 3, 4, 5, 6, 40].map(delayFor), [1000, 2000, 5000, 10000, 30000, 30000, 30000]);
});

test('only an ended session stops the tries', () => {
  assert.equal(retries(SESSION_ENDED), false);
  for (const code of [1000, 1001, 1006, 1008, 1011, undefined]) assert.equal(retries(code), true, String(code));
});

test('the footer says what is so', () => {
  assert.match(statusWords({ state: 'open', attempt: 0 }), /^Connected \u00b7 terminals keep running when you close this page, until the console restarts$/);
  assert.equal(statusWords({ state: 'reconnecting', attempt: 3 }), 'Reconnecting\u2026 try 3');
  assert.equal(statusWords({ state: 'connecting', attempt: 0 }), 'Connecting\u2026');
  assert.match(statusWords({ state: 'ended', attempt: 0 }), /session ended/);
});

// Paste.

test('lines are counted as a person counts them', () => {
  assert.deepEqual(['', 'a', 'a\n', 'a\nb', 'a\r\nb\r\n', 'a\rb\rc', '\n', 'a\n\nb'].map(linesOf), [0, 1, 1, 2, 2, 3, 1, 3]);
});

test('one line goes as it is, with no question', () => {
  const paste = pasteOf('ls -l', false);
  assert.equal(text(paste.bytes), 'ls -l');
  assert.deepEqual([paste.lines, paste.ask], [1, null]);
});

test('lines end in CR, whatever ended them', () => {
  assert.equal(text(pasteOf('a\r\nb\nc', true).bytes), `${BRACKET_START}a\rb\rc${BRACKET_END}`);
  assert.equal(text(pasteOf('a\nb', false).bytes), 'a\rb');
});

test('several lines for a program that did not ask for brackets are asked about first', () => {
  const paste = pasteOf('one\ntwo\nthree\n', false);
  assert.equal(paste.ask, 'Paste 3 lines? Each line may run as a command.');
  assert.equal(pasteOf('a\nb', false).ask, 'Paste 2 lines? Each line may run as a command.');
});

test('a program that asked for brackets gets the lines bracketed, with no question', () => {
  const paste = pasteOf('one\ntwo', true);
  assert.equal(paste.ask, null);
  assert.equal(text(paste.bytes), `${BRACKET_START}one\rtwo${BRACKET_END}`);
});

test('the text cannot end the bracket or start a sequence', () => {
  const evil = `x${BRACKET_END}rm -rf ~\u001b[31m\u0007\u0000\u007f\tend`;
  const clean = text(pasteOf(evil, true).bytes);
  assert.equal(clean, `${BRACKET_START}x[201~rm -rf ~[31m\tend${BRACKET_END}`);
  assert.equal(clean.slice(BRACKET_START.length, -BRACKET_END.length).includes('\u001b'), false);
});

test('nothing, not text, and too much make no paste', () => {
  assert.equal(pasteOf('', true), null);
  assert.equal(pasteOf(undefined, true), null);
  assert.equal(pasteOf(7, false), null);
  assert.deepEqual(pasteOf('x'.repeat(MOST_BYTES + 1), false), { tooBig: true });
  // The bound is of bytes: an é is two of them.
  assert.deepEqual(pasteOf('é'.repeat(MOST_BYTES / 2 + 1), false), { tooBig: true });
  assert.equal(pasteOf('é'.repeat(MOST_BYTES / 2), false).bytes.length, MOST_BYTES);
  assert.equal(pasteOf('x'.repeat(MOST_BYTES), true).bytes.length, MOST_BYTES + 12);
});

test('characters past ASCII are sent as UTF-8', () => {
  assert.deepEqual([...pasteOf('\u00e9\u4e2d', false).bytes], [0xc3, 0xa9, 0xe4, 0xb8, 0xad]);
});

// The key bar.

test('the bar has the keys the design lists, and Ctrl and Alt are the sticky ones', () => {
  assert.deepEqual(KEYS.map((key) => key.label), ['Esc', 'Tab', 'Ctrl', 'Alt', '\u2190', '\u2191', '\u2193', '\u2192', '|', '/', '~', '-']);
  assert.deepEqual(KEYS.filter((key) => key.sticky).map((key) => key.id), ['ctrl', 'alt']);
  assert.ok(KEYS.every((key) => key.name.length > 0));
});

test('plain keys give their bytes', () => {
  const bar = new KeyBar();
  assert.deepEqual([...bar.press('Escape')], [0x1b]);
  assert.deepEqual([...bar.press('Tab')], [9]);
  assert.equal(text(bar.press('|')), '|');
  assert.equal(text(bar.press('ArrowUp')), '\u001b[A');
  assert.equal(text(bar.press('ArrowUp', { applicationCursor: true })), '\u001bOA');
});

test('Ctrl waits for the next key and is spent by it', () => {
  const bar = new KeyBar();
  assert.equal(bar.press('ctrl'), null);
  assert.deepEqual([bar.armed, bar.ctrl, bar.alt], [true, true, false]);
  assert.deepEqual([...bar.keyed({ key: 'w', code: 'KeyW' })], [0x17]);
  assert.equal(bar.armed, false);
  assert.deepEqual([...bar.keyed({ key: 'w', code: 'KeyW' })], [0x77]);
});

test('Alt puts an ESC in front, and both can wait together', () => {
  const bar = new KeyBar();
  bar.press('alt');
  assert.equal(text(bar.press('/')), '\u001b/');
  bar.press('ctrl');
  bar.press('alt');
  assert.deepEqual([...bar.keyed({ key: 'c', code: 'KeyC' })], [0x1b, 0x03]);
  assert.equal(bar.armed, false);
});

test('a sticky key spent by a key of the keyboard, or let go, tells the bar once', () => {
  const bar = new KeyBar();
  let told = 0;
  bar.changed = () => { told += 1; };
  bar.press('ctrl');
  bar.keyed({ key: 'c', code: 'KeyC' });
  assert.equal(told, 1);
  bar.keyed({ key: 'c', code: 'KeyC' });
  bar.clear();
  assert.equal(told, 1);
  bar.press('alt');
  assert.deepEqual([...bar.typed('x')], [0x1b, 0x78]);
  assert.equal(told, 2);
  bar.press('ctrl');
  bar.clear();
  assert.equal(told, 3);
});

test('pressing a sticky key again lets it go', () => {
  const bar = new KeyBar();
  bar.press('ctrl');
  bar.press('ctrl');
  assert.equal(bar.armed, false);
  bar.press('alt');
  bar.clear();
  assert.equal(bar.armed, false);
});

test('a modifier key on its own, or a key that sends nothing, leaves the sticky keys waiting', () => {
  const bar = new KeyBar();
  bar.press('ctrl');
  assert.equal(bar.keyed({ key: 'Shift' }), null);
  assert.equal(bar.keyed({ key: 'Unidentified' }), null);
  assert.equal(bar.armed, true);
});

test('the keyboard\'s own modifiers add to the sticky ones, and the text of a soft keyboard is turned too', () => {
  const bar = new KeyBar();
  bar.press('ctrl');
  assert.equal(bar.typed('', {}), null);
  assert.deepEqual([...bar.typed('d')], [0x04]);
  assert.equal(bar.typed('d'), null, 'nothing waits now');
  bar.press('alt');
  assert.equal(text(bar.keyed({ key: 'x', code: 'KeyX', altKey: false, shiftKey: true })), '\u001bx');
  bar.press('alt');
  assert.equal(text(bar.typed('\u00e9z')), '\u001b\u00e9');
  assert.equal(text(new KeyBar().keyed({ key: 'x', ctrlKey: true })), '\u0018');
});

// What a screen reader is told.

test('escape sequences, controls and carriage returns are not read', () => {
  const said = new Announcer();
  said.push('\u001b[1;31merror\u001b[0m: \u001b]0;title\u0007no such file\u001b(B\r\n\u0007done\t!\r\n');
  assert.equal(said.take(0), 'error: no such file\ndone !');
});

test('a sequence cut in two pieces is not read in either', () => {
  const said = new Announcer();
  said.push('a\u001b[3');
  said.push('1mb\u001b]0;ti');
  said.push('tle\u0007c\u001b');
  said.push('7d');
  assert.equal(said.take(0), 'abcd');
});

test('a sequence that never ends is let go when it is long', () => {
  const said = new Announcer();
  said.push(`x\u001b]${'a'.repeat(5000)}`);
  said.push('y\n');
  assert.equal(said.take(0), 'xy');
});

test('nothing is said of output that is only blanks, and the reader is told of text once', () => {
  const said = new Announcer();
  said.push('  \r\n \t ');
  assert.equal(said.waiting, false);
  assert.equal(said.take(0), null);
  said.push('hi\n');
  assert.equal(said.waiting, true);
  assert.equal(said.take(0), 'hi');
  assert.equal(said.waiting, false);
  assert.equal(said.take(1), null);
});

test('words are given at most every 500 ms', () => {
  const said = new Announcer();
  said.push('one\n');
  assert.equal(said.wait(1000), 0);
  assert.equal(said.take(1000), 'one');
  said.push('two\n');
  assert.equal(said.wait(1100), EVERY_MS - 100);
  assert.equal(said.take(1100), null);
  assert.equal(said.take(1000 + EVERY_MS - 1), null);
  assert.equal(said.take(1000 + EVERY_MS), 'two');
});

test('pieces that come between are put together', () => {
  const said = new Announcer();
  said.push('ls');
  said.push(' -l\nfile');
  said.push('s\n');
  assert.equal(said.take(0), 'ls -l\nfiles');
});

test('at most 500 characters are said, and the rest is counted in lines', () => {
  const said = new Announcer();
  const lines = Array.from({ length: 60 }, (_, i) => `line number ${i + 1}`);
  said.push(`${lines.join('\n')}\n`);
  const words = said.take(0);
  const [kept, more] = words.split('\n\u2026');
  assert.ok(kept.length <= MOST_CHARS, String(kept.length));
  assert.match(more, /^and \d+ more lines$/);
  const shown = kept.split('\n').length;
  assert.equal(shown + Number(more.match(/\d+/)[0]), 60);
  assert.deepEqual(kept.split('\n'), lines.slice(0, shown));
});

test('one more line is said as one more line, and a single long line is cut', () => {
  const said = new Announcer();
  said.push(`${'a'.repeat(300)}\n${'b'.repeat(300)}\n`);
  assert.equal(said.take(0), `${'a'.repeat(300)}\n\u2026and 1 more line`);
  const long = new Announcer();
  long.push(`${'c'.repeat(900)}\nnext\n`);
  assert.equal(long.take(0), `${'c'.repeat(MOST_CHARS)}\n\u2026and 1 more line`);
  const alone = new Announcer();
  alone.push('d'.repeat(900));
  assert.equal(alone.take(0), 'd'.repeat(MOST_CHARS));
});
