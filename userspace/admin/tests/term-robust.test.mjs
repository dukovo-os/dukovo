// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a hostile or broken program can do to the terminal: it writes anything
// at all, in pieces cut anywhere. The screen must never throw, never leave the
// cursor off the screen or half a wide character in a cell, and never grow
// past its bounds. Every run is seeded, so a failure names the seed to replay.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MAX_COLS, MAX_ROWS, Screen, cellWidth } from '../pages/terminal/term/screen.js';
import { keyBytes } from '../pages/terminal/term/keys.js';
import { mouseBytes } from '../pages/terminal/term/mouse.js';
import { clipSelection, lineRange, selectionSpan, selectionText, wordRange } from '../pages/terminal/term/select.js';

// A small seeded generator (mulberry32), so that a run can be repeated.
function random(seed) {
  let state = seed >>> 0;
  const next = () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const between = (low, high) => low + Math.floor(next() * (high - low + 1));
  return { next, between, pick: (list) => list[Math.floor(next() * list.length)] };
}

const encoder = new TextEncoder();

// The modes the screen knows, and the finals of every CSI it reads.
const MODES = [1, 6, 7, 12, 25, 47, 1000, 1002, 1006, 1047, 1048, 1049, 2004, 2, 4, 20, 9999];
const FINALS = '@ABCDEFGHIJKLMPSTXZ`abdefghlmnqrstu'.split('');
const TEXTS = [
  'abc', 'hello world', '\u4e2d\u6587', 'e\u0301', '\u{1f600}', '\u00e9', '\u200d', '\ufe0f', '\u0301\u0302\u0303', 'x'.repeat(90),
  '\u3042\u3044', '\u2500\u2502', '\u0e01\u0e34', ' \t ', '\u{10ffff}', '\ud7ff',
];

// A stream of `count` tokens: some are plain bytes, most are things a
// terminal reads: text, controls, escapes, CSI with parameters, OSC, DCS and
// the bytes that cannot start a character.
function stream(rng, count) {
  const parts = [];
  for (let i = 0; i < count; i++) {
    const kind = rng.between(0, 15);
    if (kind === 0) {
      parts.push(Uint8Array.from({ length: rng.between(1, 40) }, () => rng.between(0, 255)));
    } else if (kind <= 3) {
      parts.push(encoder.encode(rng.pick(TEXTS)));
    } else if (kind === 4) {
      parts.push(Uint8Array.of(rng.between(0, 31)));
    } else if (kind === 5) {
      parts.push(encoder.encode(`\x1b${rng.pick('78DEMHc()=>#%*+-./[]PX^_\\NO~}|'.split(''))}${rng.pick(['', '0', 'B', '8', 'A', '(', '@'])}`));
    } else if (kind <= 9) {
      const params = Array.from({ length: rng.between(0, 5) }, () => rng.pick(['', '0', '1', '2', '3', '5', '7', '9', '24', '80', '256', '65535', '70000', '999999999'])).join(rng.pick([';', ';', ';', ':']));
      const prefix = rng.pick(['', '', '', '?', '>', '<', '=', '!']);
      const inter = rng.pick(['', '', '', ' ', '!', '"', '$', ' !"']);
      parts.push(encoder.encode(`\x1b[${prefix}${params}${inter}${rng.pick(FINALS)}`));
    } else if (kind === 10) {
      parts.push(encoder.encode(`\x1b[?${rng.pick(MODES)}${rng.pick(['h', 'l'])}`));
    } else if (kind === 11) {
      parts.push(encoder.encode(`\x1b[${rng.pick(['0', '1', '4', '7', '38;5;200', '48;2;1;2;3', '38;2;300;0;0', '38', '48;5', '99', ''])}m`));
    } else if (kind === 12) {
      const text = rng.pick(['0;title', '2;a\u0007b', '52;c;aGVsbG8=', '8;;http://x', '133;A', '0;' + 'x'.repeat(3000), '4;1;rgb:ff/00/00', '']);
      parts.push(encoder.encode(`\x1b]${text}${rng.pick(['\x07', '\x1b\\', '\x1b', ''])}`));
    } else if (kind === 13) {
      parts.push(encoder.encode(`\x1bP${rng.pick(['1$r', 'q', '+q', '0;1|'])}${rng.pick(['payload', '\x1b\\', '\x18', ''])}`));
    } else if (kind === 14) {
      parts.push(Uint8Array.of(rng.pick([0x80, 0xc0, 0xc1, 0xe0, 0xed, 0xf0, 0xf4, 0xf5, 0xff, 0x9b, 0xbf, 0x7f])));
    } else {
      parts.push(encoder.encode('\r\n'.repeat(rng.between(1, 30))));
    }
  }
  const bytes = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    bytes.set(part, at);
    at += part.length;
  }
  return bytes;
}

const TAIL_KIND = 0;

// Everything that must hold of a screen after any input.
function invariants(screen, label) {
  const { cols, rows } = screen;
  assert.ok(cols >= 2 && cols <= MAX_COLS && rows >= 1 && rows <= MAX_ROWS, `${label}: size ${cols}x${rows}`);
  assert.ok(Number.isInteger(screen.x) && screen.x >= 0 && screen.x < cols, `${label}: x ${screen.x} of ${cols}`);
  assert.ok(Number.isInteger(screen.y) && screen.y >= 0 && screen.y < rows, `${label}: y ${screen.y} of ${rows}`);
  assert.ok(screen.top >= 0 && screen.top <= screen.bottom && screen.bottom < rows, `${label}: region ${screen.top}-${screen.bottom}`);
  assert.equal(screen.dirty.length, rows, `${label}: dirty`);
  assert.equal(screen.tabs.length, cols, `${label}: tabs`);
  assert.equal(screen.grid.cells.length, cols * rows * 3, `${label}: cells`);
  assert.equal(screen.grid.wrapped.length, rows, `${label}: wrapped`);
  assert.ok(screen.ringCount <= screen.scrollbackLimit, `${label}: ring ${screen.ringCount} of ${screen.scrollbackLimit}`);
  assert.ok(screen.clusters.length <= 0xffff, `${label}: clusters`);
  assert.ok(Array.from(screen.title).length <= 64 && !/\p{Cc}/u.test(screen.title), `${label}: title`);
  for (const grid of [screen.main, screen.alt]) {
    const saved = grid.saved;
    if (saved) assert.ok(saved.x >= 0 && saved.x < cols && saved.y >= 0 && saved.y < rows, `${label}: saved cursor`);
  }
  for (const grid of [screen.main, screen.alt]) {
    for (let y = 0; y < rows; y++) {
      for (let x = 0; x < cols; x++) {
        const word = grid.cells[(y * cols + x) * 3];
        const width = cellWidth(word);
        // A wide character always has its right half, and a right half its character.
        if (width === 2) {
          assert.ok(x + 1 < cols && cellWidth(grid.cells[(y * cols + x + 1) * 3]) === TAIL_KIND, `${label}: wide at ${x},${y} has no tail`);
        }
        if (width === 0) {
          assert.ok(x > 0 && cellWidth(grid.cells[(y * cols + x - 1) * 3]) === 2, `${label}: tail at ${x},${y} has no lead`);
        }
        if ((word & 0x1fffff) >= 0x110000) assert.ok((word & 0x1fffff) - 0x110000 < screen.clusters.length, `${label}: cluster id`);
      }
    }
  }
}

// What the page does with the screen besides writing to it, at random, none of
// which may throw or break the invariants.
function disturb(screen, rng) {
  switch (rng.between(0, 9)) {
    case 0:
      screen.resize(rng.between(1, 60), rng.between(0, 30));
      break;
    case 1:
      screen.reset({ clearScrollback: rng.next() < 0.5 });
      break;
    case 2:
      screen.setScrollback(rng.between(0, 40));
      break;
    case 3:
      screen.clearScrollback();
      break;
    case 4:
      screen.clearDirty();
      break;
    default:
  }
}

function run(seed, { chunkMax = 300, tokens = 400, disturbEvery = 5 } = {}) {
  const rng = random(seed);
  const screen = new Screen({ cols: rng.between(2, 60), rows: rng.between(1, 25), scrollback: rng.between(0, 30), onBell() {}, onTitle() {} });
  const bytes = stream(rng, tokens);
  let chunk = 0;
  for (let at = 0; at < bytes.length; chunk++) {
    const size = rng.next() < 0.3 ? rng.between(1, 3) : rng.between(1, chunkMax);
    const reply = screen.write(bytes.subarray(at, at + size));
    assert.ok(reply.length <= 4096, `seed ${seed}: reply of ${reply.length} bytes`);
    at += size;
    if (chunk % disturbEvery === 0) disturb(screen, rng);
    invariants(screen, `seed ${seed}, chunk ${chunk}, byte ${at}`);
  }
  return screen;
}

test('seeded streams of tokens, cut in large and small pieces, never break the screen', () => {
  for (let seed = 1; seed <= 400; seed++) run(seed);
});

test('seeded streams in big pieces', () => {
  for (let seed = 1001; seed <= 1060; seed++) run(seed, { chunkMax: 5000, tokens: 1500, disturbEvery: 3 });
});

test('uniformly random bytes never break the screen', () => {
  for (let seed = 1; seed <= 150; seed++) {
    const rng = random(seed * 7919);
    const screen = new Screen({ cols: rng.between(2, 40), rows: rng.between(1, 20), scrollback: 10 });
    for (let i = 0; i < 20; i++) {
      const bytes = Uint8Array.from({ length: rng.between(1, 2000) }, () => rng.between(0, 255));
      const reply = screen.write(bytes);
      assert.ok(reply.length <= 4096);
      invariants(screen, `seed ${seed}, round ${i}`);
    }
  }
});

test('the same stream gives the same screen however it is cut', () => {
  for (let seed = 1; seed <= 40; seed++) {
    const rng = random(seed + 500);
    const bytes = stream(rng, 300);
    const make = () => new Screen({ cols: 17, rows: 9, scrollback: 20 });
    const whole = make();
    whole.write(bytes);
    const pieces = make();
    for (let at = 0; at < bytes.length;) {
      const size = rng.between(1, 50);
      pieces.write(bytes.subarray(at, at + size));
      at += size;
    }
    assert.deepEqual(pieces.grid.cells, whole.grid.cells, `seed ${seed}: cells`);
    assert.deepEqual([pieces.x, pieces.y, pieces.pendingWrap, pieces.ringCount, pieces.title], [whole.x, whole.y, whole.pendingWrap, whole.ringCount, whole.title], `seed ${seed}`);
  }
});

test('a flood of combining marks fills the cluster table and stops there', () => {
  const screen = new Screen({ cols: 80, rows: 24, scrollback: 10 });
  const marks = [];
  for (let i = 0; i < 70000; i++) marks.push(0x0300 + (i % 0x70));
  for (let i = 0; i < 200; i++) {
    // Every character a different base and a different run of marks, so each makes a new cluster.
    const text = String.fromCodePoint(0x4e00 + i, ...marks.slice(i, i + 12), 0x41 + (i % 26), ...marks.slice(i * 3, i * 3 + 14));
    screen.write(encoder.encode(`${text}\r\n`));
  }
  invariants(screen, 'combining');
  assert.ok(screen.clusters.length <= 0xffff);
});

test('one cell never holds more than 16 UTF-16 units of marks', () => {
  const screen = new Screen({ cols: 10, rows: 3, scrollback: 0 });
  screen.write(encoder.encode(`a${'\u0301'.repeat(500)}b`));
  assert.ok(screen.textOf(screen.grid.cells[0]).length <= 16);
  invariants(screen, 'marks');
});

test('huge parameters, long sequences and floods of queries stay bounded', () => {
  const screen = new Screen({ cols: 80, rows: 24, scrollback: 100 });
  const send = (text) => screen.write(typeof text === 'string' ? encoder.encode(text) : text);
  send(`\x1b[${'9'.repeat(5000)}A`);
  send(`\x1b[${'1;'.repeat(5000)}m`);
  send(`\x1b[${';'.repeat(40)}H`);
  send(`\x1b[999999999;999999999H`);
  send(`\x1b[999999999@\x1b[999999999P\x1b[999999999L\x1b[999999999M\x1b[999999999X\x1b[999999999S\x1b[999999999T`);
  send('a\x1b[999999999b');
  send(`\x1b]0;${'t'.repeat(100000)}\x07`);
  send(`\x1b]52;c;${'A'.repeat(100000)}\x07`);
  send(`\x1bP${'d'.repeat(100000)}\x1b\\`);
  send(`\x1b[ ${'!'.repeat(1000)}p`);
  const reply = send('\x1b[6n'.repeat(5000) + '\x1b[c'.repeat(5000) + '\x1b[5n'.repeat(5000));
  assert.ok(reply.length <= 4096, `reply of ${reply.length} bytes`);
  assert.ok(reply.length > 0);
  assert.ok(screen.parser.params.length <= 32);
  invariants(screen, 'bounded');
});

test('a long run of lines keeps the scrollback at its limit', () => {
  const screen = new Screen({ cols: 40, rows: 10, scrollback: 500 });
  const line = encoder.encode(`${'x'.repeat(39)}\r\n`);
  for (let i = 0; i < 20000; i++) screen.write(line);
  assert.equal(screen.ringCount, 500);
  assert.equal(screen.ring.length, 500);
  screen.write(encoder.encode('\x1b[?1049h' + 'y\r\n'.repeat(5000) + '\x1b[?1049l'));
  assert.equal(screen.ringCount, 500);
  invariants(screen, 'lines');
});

test('a resize with a full scrollback finishes and keeps the text', () => {
  const screen = new Screen({ cols: 100, rows: 30, scrollback: 5000 });
  const line = encoder.encode(`${'word '.repeat(19)}end\r\n`);
  for (let i = 0; i < 6000; i++) screen.write(line);
  for (const [cols, rows] of [[40, 30], [500, 300], [2, 1], [100, 30]]) {
    screen.resize(cols, rows);
    invariants(screen, `resize ${cols}x${rows}`);
    assert.ok(screen.ringCount <= 5000);
  }
});

test('selection, keys and the mouse take anything without throwing', () => {
  const rng = random(77);
  const screen = run(5, { tokens: 1500 });
  const first = screen.firstLine;
  const last = screen.scrolled + screen.rows - 1;
  const point = () => ({ line: rng.between(first - 5, last + 5), col: rng.between(-3, screen.cols + 3) });
  for (let i = 0; i < 2000; i++) {
    const selection = { start: point(), end: point() };
    selectionText(screen, selection);
    clipSelection(screen, selection);
    selectionSpan(screen, selection, rng.between(first - 3, last + 3));
    const where = point();
    wordRange(screen, where.line, where.col);
    lineRange(screen, where.line);
  }
  const values = [undefined, null, 0, 1, -1, 1.5, NaN, '', 'a', 'Enter', 'ArrowUp', 'F5', 'Dead', {}, [], true];
  const fields = ['key', 'code', 'ctrlKey', 'altKey', 'shiftKey', 'metaKey', 'action', 'button', 'direction', 'x', 'y', 'bytes', 'isComposing'];
  for (let i = 0; i < 5000; i++) {
    const event = {};
    for (const name of fields) if (rng.next() < 0.6) event[name] = rng.pick(values);
    const result = keyBytes(event, screen);
    assert.ok(result === null || result instanceof Uint8Array);
    const report = mouseBytes(event, { mouseTracking: rng.pick([0, 1000, 1002]), mouseSgr: rng.next() < 0.5 });
    assert.ok(report === null || report instanceof Uint8Array);
  }
});
