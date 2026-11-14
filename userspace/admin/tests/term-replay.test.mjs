// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { FIXTURES, chunks, decodeVt, expected, fixtureNames, loadFixture, main, observed, replay } from './term-replay.mjs';

// How each fixture is handed to the screen: whole, a byte at a time, and
// in pieces cut in odd places by seeds.
const WAYS = ['whole', 'bytes', 1, 2, 3, 4, 5, 6, 7];

test('there are fixtures, and each says what it is', () => {
  const names = fixtureNames();
  assert.ok(names.length >= 8, names.join(' '));
  for (const name of names) {
    const { meta } = loadFixture(name);
    assert.equal(typeof meta.source, 'string', name);
    assert.equal(typeof meta.program, 'string', name);
    assert.ok(Array.isArray(meta.cursor) && meta.cursor.length === 2, name);
    if (name.startsWith('hand-')) assert.match(meta.source, /^hand-built/, name);
    else assert.match(meta.source, /^recorded on/, name);
    assert.ok(/^(hand|host|dukovo)-/.test(name), `${name} starts with where it came from`);
  }
});

test('every file in the fixtures folder belongs to a fixture', () => {
  const names = new Set(fixtureNames());
  for (const file of readdirSync(FIXTURES)) {
    const name = file.replace(/\.(json|vt|raw|screen|scrollback)$/, '');
    assert.ok(names.has(name), `${file} has no ${name}.json`);
  }
});

for (const name of fixtureNames()) {
  test(`fixture ${name} draws what is expected, however it is cut`, () => {
    const fixture = loadFixture(name);
    assert.equal(fixture.screen.length, fixture.meta.rows, 'the screen file has one line per row');
    for (const how of WAYS) {
      assert.deepEqual(observed(replay(fixture, how)), expected(fixture), `${name} cut as ${how}`);
    }
  });
}

test('every fixture leaves the cursor on the screen and keeps its memory bounded', () => {
  for (const name of fixtureNames()) {
    const fixture = loadFixture(name);
    const { screen } = replay(fixture, 'whole');
    assert.ok(screen.x >= 0 && screen.x < screen.cols && screen.y >= 0 && screen.y < screen.rows, name);
    assert.ok(screen.ringCount <= (fixture.meta.scrollback ?? 100), name);
  }
});

test('decodeVt reads the escapes', () => {
  const text = (source) => [...decodeVt(source)];
  assert.deepEqual(text('abc'), [97, 98, 99]);
  assert.deepEqual(text('\\e[1m\\a\\b\\t\\n\\v\\f\\r\\\\'), [0x1b, 0x5b, 0x31, 0x6d, 7, 8, 9, 10, 11, 12, 13, 0x5c]);
  assert.deepEqual(text('\\x00\\xFf\\x7f'), [0, 255, 127]);
  assert.deepEqual(text('\\u00e9\\u4e2d\\u{1F600}'), [0xc3, 0xa9, 0xe4, 0xb8, 0xad, 0xf0, 0x9f, 0x98, 0x80]);
  assert.deepEqual(text('\u00e9\u4e2d'), [0xc3, 0xa9, 0xe4, 0xb8, 0xad]);
  assert.deepEqual(text('a\\\\b'), [97, 0x5c, 98]);
});

test('decodeVt joins lines with nothing and skips comments and empty lines', () => {
  assert.deepEqual([...decodeVt('# a comment\nab\n\n  \ncd\n#\\e\n')], [97, 98, 32, 32, 99, 100]);
  assert.deepEqual([...decodeVt('')], []);
  assert.deepEqual([...decodeVt('a\\r\\nb')], [97, 13, 10, 98]);
});

test('decodeVt refuses an escape it does not know', () => {
  for (const bad of ['\\q', '\\x1', '\\xzz', '\\u12', '\\u{}', '\\u{1234567}', 'a\\']) {
    assert.throws(() => decodeVt(bad), /unknown escape/, bad);
  }
  assert.throws(() => decodeVt('\\toString'.replace('t', '_')), /unknown escape/);
  assert.throws(() => decodeVt('\\constructor'), /unknown escape/);
});

test('chunks cuts the bytes as asked and loses none', () => {
  const bytes = Uint8Array.from({ length: 1000 }, (_, i) => i % 256);
  assert.equal(chunks(bytes, 'whole').length, 1);
  assert.equal(chunks(bytes, 'bytes').length, 1000);
  for (const seed of [1, 2, 3, 99]) {
    const parts = chunks(bytes, seed);
    assert.ok(parts.length > 80 && parts.length < 1000);
    assert.ok(parts.every((part) => part.length >= 1 && part.length <= 13));
    assert.deepEqual(parts.flatMap((part) => [...part]), [...bytes]);
    assert.deepEqual(chunks(bytes, seed).map((p) => p.length), parts.map((p) => p.length));
  }
  assert.notDeepEqual(chunks(bytes, 1).map((p) => p.length), chunks(bytes, 2).map((p) => p.length));
  assert.deepEqual(chunks(new Uint8Array(0), 'bytes'), []);
});

// A fixture folder of its own, to test what reads and writes one.
function scratch() {
  const dir = `${mkdtempSync(join(tmpdir(), 'term-fixtures-'))}/`;
  writeFileSync(`${dir}demo.vt`, '# a demo\nhi \\e[1mthere\\e[0m\\r\\nsecond\\e]0;the title\\a\\e[6n\n');
  writeFileSync(
    `${dir}demo.json`,
    JSON.stringify({ source: 'hand-built', program: 'demo', cols: 10, rows: 3, scrollback: 5, cursor: [0, 0], title: 'the title', replies: '\x1b[2;7R' }),
  );
  return dir;
}

test('replay reads the size, the title and the answers from a fixture', () => {
  const dir = scratch();
  try {
    const fixture = loadFixture('demo', dir);
    const result = replay(fixture);
    assert.deepEqual([result.screen.cols, result.screen.rows], [10, 3]);
    assert.equal(result.title, 'the title');
    assert.equal(result.replies, '\x1b[2;7R');
    const shown = observed(result);
    assert.deepEqual(shown.rows, ['hi there', 'second', '']);
    assert.deepEqual(shown.cursor, [6, 1]);
    assert.deepEqual(shown.scrollback, []);
    assert.equal(shown.cursorVisible, true);
    assert.equal(shown.alternate, false);
    assert.equal(fixture.screen, null);
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test('a fixture with no size is 80 by 24', () => {
  const dir = scratch();
  try {
    writeFileSync(`${dir}plain.json`, JSON.stringify({ source: 'hand-built', program: 'x', cursor: [0, 0] }));
    writeFileSync(`${dir}plain.raw`, Buffer.from('plain'));
    const result = replay(loadFixture('plain', dir));
    assert.deepEqual([result.screen.cols, result.screen.rows], [80, 24]);
    assert.deepEqual(fixtureNames(dir), ['demo', 'plain']);
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test('the command line lists the fixtures and prints one', () => {
  const dir = scratch();
  try {
    const lines = [];
    main([], dir, (line) => lines.push(line));
    assert.deepEqual(lines, ['fixtures: demo']);
    lines.length = 0;
    main(['demo'], dir, (line) => lines.push(line));
    assert.match(lines[0], /^\|hi there\n\|second\n\|$/);
    assert.match(lines[1], /^cursor 6,1 visible true alternate false title "the title"$/);
    assert.match(lines[2], /^scrollback 0 lines, replies /);
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test('--write puts the screen and the scrollback in the fixture\'s files', () => {
  const dir = scratch();
  try {
    writeFileSync(`${dir}long.json`, JSON.stringify({ source: 'hand-built', program: 'x', cols: 10, rows: 2, scrollback: 5, cursor: [0, 1] }));
    writeFileSync(`${dir}long.vt`, 'one\\r\\ntwo\\r\\nthree\\r\\nfour');
    main(['long', '--write'], dir, () => {});
    assert.equal(readFileSync(`${dir}long.screen`, 'utf8'), 'three\nfour\n');
    assert.equal(readFileSync(`${dir}long.scrollback`, 'utf8'), 'one\ntwo\n');
    const fixture = loadFixture('long', dir);
    assert.deepEqual(fixture.screen, ['three', 'four']);
    assert.deepEqual(fixture.scrollback, ['one', 'two']);
    main(['demo', '--write'], dir, () => {});
    assert.equal(readFileSync(`${dir}demo.screen`, 'utf8'), 'hi there\nsecond\n\n');
    assert.deepEqual(readdirSync(dir).filter((file) => file.startsWith('demo.scroll')), []);
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test('an expected screen may lack its last line break', () => {
  const dir = scratch();
  try {
    writeFileSync(`${dir}demo.screen`, 'hi there\nsecond');
    writeFileSync(`${dir}demo.scrollback`, 'only');
    const fixture = loadFixture('demo', dir);
    assert.deepEqual(fixture.screen, ['hi there', 'second']);
    assert.deepEqual(fixture.scrollback, ['only']);
  } finally {
    rmSync(dir, { recursive: true });
  }
});

test('run as a program it lists the fixtures and prints one', () => {
  const script = fileURLToPath(new URL('./term-replay.mjs', import.meta.url));
  const listed = execFileSync(process.execPath, [script], { encoding: 'utf8' });
  assert.match(listed, /^fixtures: .*hand-nukesh-line-edit.*host-top\n$/);
  const shown = execFileSync(process.execPath, [script, 'hand-nukesh-line-edit'], { encoding: 'utf8' });
  assert.match(shown, /^\|chase@duke-nas:~\$ echo hello\n\|hello\n/);
  assert.match(shown, /cursor 20,2 visible true alternate false title "chase@duke-nas: ~"/);
  assert.ok(FIXTURES.endsWith('/term-fixtures/'));
});
