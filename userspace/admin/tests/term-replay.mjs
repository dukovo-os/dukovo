// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Replays what a program sent to a terminal through the emulator's screen and
// reads back what is drawn, so a recording is a test: the bytes, and the
// screen text expected after them. The tests in term-replay.test.mjs run each
// fixture; `node term-replay.mjs NAME` prints one, which is how to see what
// a new recording draws and write its expected screen.
//
// A fixture NAME is files in term-fixtures/:
//
//   NAME.json        what it is and what is expected besides the rows:
//                    { "source": "hand-built, ..." or "recorded on ...",
//                      "program": "less 590", "cols": 80, "rows": 24,
//                      "scrollback": 100,           lines kept (default 100)
//                      "cursor": [x, y],            where the cursor is at the end
//                      "cursorVisible": true,       (default true)
//                      "alternate": false,          on the alternate screen at the end
//                      "title": "",                 the title at the end (default "")
//                      "replies": "" }              what the screen answers, as text
//   NAME.vt          the bytes, as text: escapes written out (see `decodeVt`)
//   NAME.raw         or the bytes themselves, as `script` or a pty records them
//   NAME.screen      the rows of the screen at the end, one per line, without
//                    the blanks at the end of each, as many lines as `rows`
//   NAME.scrollback  optionally, the lines in the scrollback, oldest first
//
// A fixture built by hand says so in `source`, and its name starts with
// `hand-`; one recorded on a program starts with `host-` or `dukovo-`
// for where it was recorded (the lead adds recordings from Dukovo itself).

import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { Screen } from '../pages/terminal/term/screen.js';

export const FIXTURES = fileURLToPath(new URL('./term-fixtures/', import.meta.url));

const ESCAPES = { e: 0x1b, a: 7, b: 8, t: 9, n: 10, v: 11, f: 12, r: 13, '\\': 0x5c };

/**
 * The bytes a `.vt` file stands for. It is text in lines, each line
 * read for escapes and joined to the next with nothing between (a line
 * break is `\r\n`, written out): `\e` escape, `\a` bell, `\b`, `\t`, `\n`,
 * `\v`, `\f`, `\r`, `\\`, `\xHH` a byte, `\uHHHH` and `\u{H...}` a
 * character, which goes in as UTF-8. Lines that start with `#`, and empty
 * ones, are comments. Anything else after a backslash is a mistake.
 */
export function decodeVt(source) {
  const bytes = [];
  const encoder = new TextEncoder();
  for (const line of source.split('\n')) {
    if (line === '' || line.startsWith('#')) continue;
    for (let at = 0; at < line.length;) {
      const character = String.fromCodePoint(line.codePointAt(at));
      if (character !== '\\') {
        bytes.push(...encoder.encode(character));
        at += character.length;
        continue;
      }
      const rest = line.slice(at + 1);
      const hex = /^x([0-9A-Fa-f]{2})/.exec(rest);
      const unicode = /^u([0-9A-Fa-f]{4})|^u\{([0-9A-Fa-f]{1,6})\}/.exec(rest);
      if (hex) {
        bytes.push(parseInt(hex[1], 16));
        at += 1 + hex[0].length;
      } else if (unicode) {
        bytes.push(...encoder.encode(String.fromCodePoint(parseInt(unicode[1] ?? unicode[2], 16))));
        at += 1 + unicode[0].length;
      } else if (Object.hasOwn(ESCAPES, rest[0])) {
        bytes.push(ESCAPES[rest[0]]);
        at += 2;
      } else {
        throw new Error(`unknown escape \\${rest[0] ?? ''} in "${line}"`);
      }
    }
  }
  return Uint8Array.from(bytes);
}

/** The names of the fixtures in `dir`, in order. */
export function fixtureNames(dir = FIXTURES) {
  return readdirSync(dir)
    .filter((file) => file.endsWith('.json'))
    .map((file) => file.slice(0, -'.json'.length))
    .sort();
}

const lines = (text) => (text.endsWith('\n') ? text.slice(0, -1) : text).split('\n');

/** The fixture `name` of `dir`: `{ name, meta, bytes, screen, scrollback }`. */
export function loadFixture(name, dir = FIXTURES) {
  const meta = JSON.parse(readFileSync(`${dir}${name}.json`, 'utf8'));
  const bytes = existsSync(`${dir}${name}.vt`)
    ? decodeVt(readFileSync(`${dir}${name}.vt`, 'utf8'))
    : new Uint8Array(readFileSync(`${dir}${name}.raw`));
  const screen = existsSync(`${dir}${name}.screen`) ? lines(readFileSync(`${dir}${name}.screen`, 'utf8')) : null;
  const scrollback = existsSync(`${dir}${name}.scrollback`) ? lines(readFileSync(`${dir}${name}.scrollback`, 'utf8')) : null;
  return { name, meta, bytes, screen, scrollback };
}

/** The text of line `number` of `screen`, without the blanks at its end. */
export function lineText(screen, number) {
  const line = screen.line(number);
  let text = '';
  for (let col = 0; col < line.length; col++) text += screen.textOf(line.cells[line.offset + 3 * col]);
  return text.trimEnd();
}

/** The rows of the screen as text. */
export const screenRows = (screen) => Array.from({ length: screen.rows }, (_, y) => lineText(screen, screen.scrolled + y));

/** The lines of the scrollback as text, oldest first. */
export const scrollbackLines = (screen) =>
  Array.from({ length: screen.ringCount }, (_, i) => lineText(screen, screen.scrolled - screen.ringCount + i));

/**
 * How `bytes` are handed to the screen: `'whole'` in one write, `'bytes'`
 * one byte at a time, or a number, a seed for chunks of one to thirteen
 * bytes, so that sequences and characters are cut in odd places.
 */
export function chunks(bytes, how) {
  if (how === 'whole') return [bytes];
  const out = [];
  let seed = typeof how === 'number' ? how : 1;
  for (let at = 0; at < bytes.length;) {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    const size = how === 'bytes' ? 1 : 1 + ((seed >>> 16) % 13);
    out.push(bytes.subarray(at, at + size));
    at += size;
  }
  return out;
}

/** Replays `fixture` into a new screen, cut as `how` says (see `chunks`): `{ screen, replies }`. */
export function replay(fixture, how = 'whole') {
  const { cols = 80, rows = 24, scrollback = 100 } = fixture.meta;
  let title = '';
  const screen = new Screen({ cols, rows, scrollback, onTitle: (text) => (title = text) });
  const replies = [];
  for (const part of chunks(fixture.bytes, how)) replies.push(...screen.write(part));
  return { screen, replies: String.fromCharCode(...replies), title };
}

/** What a replay of `fixture` shows now, as the object the tests compare to `expected(fixture)`. */
export function observed({ screen, replies, title }) {
  return {
    rows: screenRows(screen),
    scrollback: scrollbackLines(screen),
    cursor: [screen.x, screen.y],
    cursorVisible: screen.cursorVisible,
    alternate: screen.alternate,
    title,
    replies,
  };
}

/** What `fixture` says a replay shows, from its files. */
export function expected(fixture) {
  const { meta } = fixture;
  return {
    rows: fixture.screen,
    scrollback: fixture.scrollback ?? [],
    cursor: meta.cursor,
    cursorVisible: meta.cursorVisible ?? true,
    alternate: meta.alternate ?? false,
    title: meta.title ?? '',
    replies: meta.replies ?? '',
  };
}

/**
 * The command line, `node term-replay.mjs NAME [--write]`: prints a fixture's
 * screen and the rest of what it shows, or the names of the fixtures with no
 * NAME. With --write it puts the screen and the scrollback in the fixture's
 * files, which the person then reads against the real program before
 * committing. `log` prints a line.
 */
export function main(args, dir = FIXTURES, log = console.log) {
  const [name, flag] = args;
  if (!name) {
    log(`fixtures: ${fixtureNames(dir).join(' ')}`);
    return;
  }
  const shown = observed(replay(loadFixture(name, dir)));
  log(shown.rows.map((row) => `|${row}`).join('\n'));
  log(`cursor ${shown.cursor} visible ${shown.cursorVisible} alternate ${shown.alternate} title ${JSON.stringify(shown.title)}`);
  log(`scrollback ${shown.scrollback.length} lines, replies ${JSON.stringify(shown.replies)}`);
  if (flag === '--write') {
    writeFileSync(`${dir}${name}.screen`, `${shown.rows.join('\n')}\n`);
    if (shown.scrollback.length) writeFileSync(`${dir}${name}.scrollback`, `${shown.scrollback.join('\n')}\n`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2));
