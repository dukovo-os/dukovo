// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's parser in term/parser.js: each state of the machine and
// the ways out of it, the UTF-8 decoder against the standard one, and the
// limits that keep a hostile stream from growing anything.

import assert from 'node:assert/strict';
import test from 'node:test';

import { MAX_OSC_LENGTH, MAX_PARAMS, MAX_PARAM_VALUE, Parser, State } from '../pages/terminal/term/parser.js';

// A handler that writes down what it is told, in a form a test can compare.
function recorder() {
  const calls = [];
  return {
    calls,
    print: (cp) => calls.push(['print', String.fromCodePoint(cp)]),
    control: (code) => calls.push(['control', code]),
    escape: (intermediates, final) => calls.push(['escape', intermediates, final]),
    csi: (prefix, params, intermediates, final) =>
      calls.push(['csi', prefix, [...params], intermediates, final]),
    osc: (text) => calls.push(['osc', text]),
  };
}

const bytesOf = (text) => Uint8Array.from(text, (character) => character.charCodeAt(0));

// Runs `chunks` (strings of bytes, one char each, or Uint8Arrays) through a fresh parser.
function run(...chunks) {
  const handler = recorder();
  const parser = new Parser(handler);
  for (const chunk of chunks) parser.write(typeof chunk === 'string' ? bytesOf(chunk) : chunk);
  return { calls: handler.calls, parser };
}

const ESC = '\x1b';

test('a new parser is in the ground state', () => {
  assert.equal(run().parser.state, State.GROUND);
});

test('ground: printable ASCII is printed and C0 controls are carried out', () => {
  const { calls } = run('ab\x07\x08\t\n\x0b\x0c\r\x0e\x0f c');
  assert.deepEqual(calls, [
    ['print', 'a'], ['print', 'b'],
    ['control', 7], ['control', 8], ['control', 9], ['control', 10], ['control', 11],
    ['control', 12], ['control', 13], ['control', 14], ['control', 15],
    ['print', ' '], ['print', 'c'],
  ]);
});

test('ground: DEL is dropped, and so are the C1 controls, however they arrive', () => {
  const { calls } = run('a\x7fb', Uint8Array.from([0xc2, 0x9b, 0xc2, 0x9d, 0xc2, 0x90, 0xc2, 0x80]), 'c');
  assert.deepEqual(calls, [['print', 'a'], ['print', 'b'], ['print', 'c']]);
});

test('ground: a C1 CSI in UTF-8 does not start a sequence', () => {
  const { calls, parser } = run(Uint8Array.from([0xc2, 0x9b]), '31mX');
  assert.deepEqual(calls.map((call) => call[1]).join(''), '31mX');
  assert.equal(parser.state, State.GROUND);
});

test('UTF-8: characters of every length are decoded', () => {
  const { calls } = run(new TextEncoder().encode('é€😀中'));
  assert.deepEqual(calls, [['print', 'é'], ['print', '€'], ['print', '😀'], ['print', '中']]);
});

test('UTF-8: a character may be split between writes at any byte', () => {
  const whole = new TextEncoder().encode('a€😀z');
  for (let cut = 1; cut < whole.length; cut++) {
    const { calls } = run(whole.slice(0, cut), whole.slice(cut));
    assert.deepEqual(calls.map((call) => call[1]).join(''), 'a€😀z', `cut at ${cut}`);
  }
});

test('UTF-8: a byte at a time gives the same characters', () => {
  const whole = new TextEncoder().encode('é€😀中z');
  const { calls } = run(...Array.from(whole, (byte) => Uint8Array.of(byte)));
  assert.deepEqual(calls.map((call) => call[1]).join(''), 'é€😀中z');
});

test('UTF-8: a stray continuation byte, and a byte that never starts a character, give U+FFFD', () => {
  assert.deepEqual(run(Uint8Array.of(0x80)).calls, [['print', '�']]);
  assert.deepEqual(run(Uint8Array.of(0xc0, 0xc1)).calls, [['print', '�'], ['print', '�']]);
  assert.deepEqual(run(Uint8Array.of(0xf5, 0xff)).calls, [['print', '�'], ['print', '�']]);
});

test('UTF-8: a cut-short character gives one U+FFFD and the byte after it is read afresh', () => {
  assert.deepEqual(run(Uint8Array.of(0xe2, 0x82), 'a').calls, [['print', '�'], ['print', 'a']]);
  assert.deepEqual(run(Uint8Array.of(0xf0, 0x9f, 0x98, 0xc3, 0xa9)).calls, [['print', '�'], ['print', 'é']]);
  assert.deepEqual(run(Uint8Array.of(0xe2), Uint8Array.of(0x82, 0x1b), '[m').calls[0], ['print', '�']);
});

test('UTF-8: a control in the middle of a character is still carried out', () => {
  const { calls } = run(Uint8Array.of(0xe2, 0x0d, 0x0a));
  assert.deepEqual(calls, [['print', '�'], ['control', 13], ['control', 10]]);
});

test('UTF-8: overlong forms, surrogates and values past U+10FFFF are refused', () => {
  const refused = (...bytes) => run(Uint8Array.from(bytes)).calls.map((call) => call[1]);
  assert.deepEqual(refused(0xe0, 0x80, 0x80), ['�', '�', '�']);
  assert.deepEqual(refused(0xed, 0xa0, 0x80), ['�', '�', '�']);
  assert.deepEqual(refused(0xf0, 0x80, 0x80, 0x80), ['�', '�', '�', '�']);
  assert.deepEqual(refused(0xf4, 0x90, 0x80, 0x80), ['�', '�', '�', '�']);
  assert.deepEqual(refused(0xed, 0x9f, 0xbf), ['퟿']);
  assert.deepEqual(refused(0xf4, 0x8f, 0xbf, 0xbf), ['\u{10ffff}']);
});

test('UTF-8: any bytes decode as the standard decoder does, in any chunks', () => {
  // The standard decoder is the reference: the same characters, bar those the
  // parser treats as controls (C0, DEL and C1), which both sides drop here.
  let seed = 12345;
  const next = () => (seed = (Math.imul(seed, 1103515245) + 12345) >>> 0) / 4294967296;
  const standard = new TextDecoder('utf-8', { ignoreBOM: true });
  const pool = [0x61, 0x80, 0x9f, 0xa0, 0xbf, 0xc2, 0xc3, 0xdf, 0xe0, 0xe1, 0xed, 0xef, 0xf0, 0xf4, 0xf5, 0xff, 0x90, 0x8f, 0x41];
  for (let run_ = 0; run_ < 300; run_++) {
    // A final ASCII byte ends any character left open, as the end of input does for the standard decoder.
    const length = 2 + Math.floor(next() * 40);
    const bytes = Uint8Array.from({ length }, () => pool[Math.floor(next() * pool.length)]);
    bytes[length - 1] = 0x41;
    const expected = [...standard.decode(bytes)]
      .filter((c) => c.codePointAt(0) >= 0x20 && !(c.codePointAt(0) >= 0x7f && c.codePointAt(0) <= 0x9f))
      .join('');
    const handler = recorder();
    const parser = new Parser(handler);
    for (let at = 0; at < length;) {
      const size = 1 + Math.floor(next() * 5);
      parser.write(bytes.slice(at, at + size));
      at += size;
    }
    const got = handler.calls.filter((call) => call[0] === 'print').map((call) => call[1]).join('');
    assert.equal(got, expected, `bytes ${[...bytes].map((b) => b.toString(16))}`);
  }
});

test('escape: ESC then a final character is one escape', () => {
  const { calls, parser } = run(`${ESC}7${ESC}8${ESC}D${ESC}c${ESC}=${ESC}>`);
  assert.deepEqual(calls, [
    ['escape', '', '7'], ['escape', '', '8'], ['escape', '', 'D'],
    ['escape', '', 'c'], ['escape', '', '='], ['escape', '', '>'],
  ]);
  assert.equal(parser.state, State.GROUND);
});

test('escape: the state after ESC, and after an intermediate', () => {
  assert.equal(run(ESC).parser.state, State.ESCAPE);
  assert.equal(run(`${ESC}(`).parser.state, State.ESCAPE_INTERMEDIATE);
});

test('escape: intermediates come before the final', () => {
  assert.deepEqual(run(`${ESC}(0${ESC}(B${ESC}#8`).calls, [
    ['escape', '(', '0'], ['escape', '(', 'B'], ['escape', '#', '8'],
  ]);
});

test('escape: more intermediates than the limit drop the escape', () => {
  assert.deepEqual(run(`${ESC} !"0x`).calls, [['print', 'x']]);
});

test('escape: a control inside is carried out and the escape goes on', () => {
  const { calls, parser } = run(`${ESC}\n`, '7', `${ESC}(\r0`);
  assert.deepEqual(calls, [['control', 10], ['escape', '', '7'], ['control', 13], ['escape', '(', '0']]);
  assert.equal(parser.state, State.GROUND);
});

test('escape: ESC again starts over, and CAN and SUB cancel', () => {
  assert.deepEqual(run(`${ESC}${ESC}7`).calls, [['escape', '', '7']]);
  assert.deepEqual(run(`${ESC}(${ESC}7`).calls, [['escape', '', '7']]);
  assert.deepEqual(run(`${ESC}\x18a`).calls, [['print', 'a']]);
  assert.deepEqual(run(`${ESC}(\x1ab`).calls, [['print', 'b']]);
});

test('escape: a character past ASCII drops the escape and is text', () => {
  assert.deepEqual(run(ESC, new TextEncoder().encode('é')).calls, [['print', 'é']]);
  assert.deepEqual(run(`${ESC}(`, new TextEncoder().encode('é')).calls, [['print', 'é']]);
});

test('escape: DEL inside is dropped', () => {
  assert.deepEqual(run(`${ESC}\x7f7`).calls, [['escape', '', '7']]);
});

test('CSI: no parameters, one, and several', () => {
  assert.deepEqual(run(`${ESC}[m${ESC}[5A${ESC}[1;2;3H`).calls, [
    ['csi', '', [], '', 'm'], ['csi', '', [5], '', 'A'], ['csi', '', [1, 2, 3], '', 'H'],
  ]);
});

test('CSI: states while it is read', () => {
  assert.equal(run(`${ESC}[`).parser.state, State.CSI_ENTRY);
  assert.equal(run(`${ESC}[1`).parser.state, State.CSI_PARAM);
  assert.equal(run(`${ESC}[;`).parser.state, State.CSI_PARAM);
  assert.equal(run(`${ESC}[?`).parser.state, State.CSI_PARAM);
  assert.equal(run(`${ESC}[1 `).parser.state, State.CSI_INTERMEDIATE);
  assert.equal(run(`${ESC}[ `).parser.state, State.CSI_INTERMEDIATE);
  assert.equal(run(`${ESC}[1:`).parser.state, State.CSI_IGNORE);
  assert.equal(run(`${ESC}[1m`).parser.state, State.GROUND);
});

test('CSI: a missing parameter is 0', () => {
  assert.deepEqual(run(`${ESC}[;m${ESC}[1;m${ESC}[;2m${ESC}[1;;3m`).calls, [
    ['csi', '', [0, 0], '', 'm'], ['csi', '', [1, 0], '', 'm'],
    ['csi', '', [0, 2], '', 'm'], ['csi', '', [1, 0, 3], '', 'm'],
  ]);
});

test('CSI: a marker before the parameters is the prefix', () => {
  assert.deepEqual(run(`${ESC}[?25h${ESC}[>c${ESC}[<5M${ESC}[=7u${ESC}[?h`).calls, [
    ['csi', '?', [25], '', 'h'], ['csi', '>', [], '', 'c'], ['csi', '<', [5], '', 'M'],
    ['csi', '=', [7], '', 'u'], ['csi', '?', [], '', 'h'],
  ]);
});

test('CSI: intermediates come between the parameters and the final', () => {
  assert.deepEqual(run(`${ESC}[2 q${ESC}[!p${ESC}[?1$p`).calls, [
    ['csi', '', [2], ' ', 'q'], ['csi', '', [], '!', 'p'], ['csi', '?', [1], '$', 'p'],
  ]);
});

test('CSI: a marker after a parameter ignores the sequence', () => {
  const { calls, parser } = run(`${ESC}[1?h`);
  assert.deepEqual(calls, []);
  assert.equal(parser.state, State.GROUND);
  assert.deepEqual(run(`${ESC}[1;?hx`).calls, [['print', 'x']]);
  assert.deepEqual(run(`${ESC}[??hx`).calls, [['print', 'x']]);
});

test('CSI: a parameter or a marker after an intermediate ignores the sequence', () => {
  assert.deepEqual(run(`${ESC}[ 1qx`).calls, [['print', 'x']]);
  assert.deepEqual(run(`${ESC}[ ?qx`).calls, [['print', 'x']]);
});

test('CSI: a colon ignores the sequence, up to its final character', () => {
  assert.deepEqual(run(`${ESC}[38:2::1:2:3mx`).calls, [['print', 'x']]);
  assert.deepEqual(run(`${ESC}[:mx`).calls, [['print', 'x']]);
});

test('CSI: more than two intermediates drop the sequence', () => {
  assert.deepEqual(run(`${ESC}[ !"qx`).calls, [['print', 'x']]);
});

test('CSI: a value over the limit stops at it', () => {
  assert.deepEqual(run(`${ESC}[99999999H`).calls, [['csi', '', [MAX_PARAM_VALUE], '', 'H']]);
  assert.deepEqual(run(`${ESC}[65535;65536H`).calls, [['csi', '', [65535, 65535], '', 'H']]);
});

test('CSI: the limit on the number of parameters', () => {
  const many = (count) => Array.from({ length: count }, (_, i) => i + 1).join(';');
  assert.deepEqual(run(`${ESC}[${many(MAX_PARAMS)}m`).calls[0][2].length, MAX_PARAMS);
  assert.deepEqual(run(`${ESC}[${many(MAX_PARAMS + 1)}mx`).calls, [['print', 'x']]);
  assert.deepEqual(run(`${ESC}[${many(MAX_PARAMS)};mx`).calls, [['print', 'x']]);
  assert.deepEqual(run(`${ESC}[${many(5000)}mx`).calls, [['print', 'x']]);
});

test('CSI: controls inside are carried out and the sequence goes on', () => {
  assert.deepEqual(run(`${ESC}[1\n;2\x08H`).calls, [
    ['control', 10], ['control', 8], ['csi', '', [1, 2], '', 'H'],
  ]);
  assert.deepEqual(run(`${ESC}[\r1 \n!q`).calls.filter((call) => call[0] === 'control'), [['control', 13], ['control', 10]]);
  assert.deepEqual(run(`${ESC}[1:\nmx`).calls, [['control', 10], ['print', 'x']]);
});

test('CSI: ESC starts over, CAN and SUB cancel', () => {
  assert.deepEqual(run(`${ESC}[1${ESC}[2m`).calls, [['csi', '', [2], '', 'm']]);
  assert.deepEqual(run(`${ESC}[1\x18m`).calls, [['print', 'm']]);
  assert.deepEqual(run(`${ESC}[1;\x1am`).calls, [['print', 'm']]);
  assert.deepEqual(run(`${ESC}[1 \x18m`).calls, [['print', 'm']]);
  assert.deepEqual(run(`${ESC}[1:\x18m`).calls, [['print', 'm']]);
});

test('CSI: a character past ASCII drops the sequence and is text', () => {
  const bytes = new TextEncoder().encode('é');
  for (const lead of ['[', '[1', '[1 ', '[1:']) {
    const { calls, parser } = run(ESC + lead, bytes, 'm');
    assert.deepEqual(calls, [['print', 'é'], ['print', 'm']], lead);
    assert.equal(parser.state, State.GROUND);
  }
});

test('CSI: DEL inside is dropped', () => {
  assert.deepEqual(run(`${ESC}[1\x7f2H`).calls, [['csi', '', [12], '', 'H']]);
});

test('CSI: one sequence split between writes at any byte', () => {
  const text = `${ESC}[?1049h${ESC}[1;38;5;196mx`;
  for (let cut = 1; cut < text.length; cut++) {
    const { calls } = run(text.slice(0, cut), text.slice(cut));
    assert.deepEqual(calls, [
      ['csi', '?', [1049], '', 'h'], ['csi', '', [1, 38, 5, 196], '', 'm'], ['print', 'x'],
    ], `cut at ${cut}`);
  }
});

test('the parameters of one sequence do not leak into the next', () => {
  assert.deepEqual(run(`${ESC}[1;2;3m${ESC}[m${ESC}[?7h`).calls, [
    ['csi', '', [1, 2, 3], '', 'm'], ['csi', '', [], '', 'm'], ['csi', '?', [7], '', 'h'],
  ]);
});

test('OSC: it ends with BEL or with ESC \\, and the state in between is OSC', () => {
  assert.equal(run(`${ESC}]`).parser.state, State.OSC);
  assert.equal(run(`${ESC}]0;ti`).parser.state, State.OSC);
  const { calls, parser } = run(`${ESC}]0;one\x07${ESC}]2;two${ESC}\\x`);
  assert.deepEqual(calls, [['osc', '0;one'], ['osc', '2;two'], ['escape', '', '\\'], ['print', 'x']]);
  assert.equal(parser.state, State.GROUND);
});

test('OSC: the text is UTF-8 and may be split between writes', () => {
  const bytes = new TextEncoder().encode(`${ESC}]0;ключ 😀\x07`);
  for (let cut = 1; cut < bytes.length; cut++) {
    assert.deepEqual(run(bytes.slice(0, cut), bytes.slice(cut)).calls, [['osc', '0;ключ 😀']], `cut at ${cut}`);
  }
});

test('OSC: controls inside are left out, and DEL and C1 characters too', () => {
  const { calls } = run(`${ESC}]0;a\x01b\x7fc`, Uint8Array.of(0xc2, 0x9b), 'd\x07');
  assert.deepEqual(calls, [['osc', '0;abcd']]);
});

test('OSC: CAN and SUB cancel it without a call', () => {
  assert.deepEqual(run(`${ESC}]0;title\x18x`).calls, [['print', 'x']]);
  assert.deepEqual(run(`${ESC}]0;title\x1ax`).calls, [['print', 'x']]);
});

test('OSC: ESC followed by something else ends it and starts that', () => {
  assert.deepEqual(run(`${ESC}]0;a${ESC}[1m`).calls, [['osc', '0;a'], ['csi', '', [1], '', 'm']]);
});

test('OSC: a string over the limit is dropped whole', () => {
  assert.deepEqual(run(`${ESC}]0;${'a'.repeat(MAX_OSC_LENGTH)}\x07`).calls, []);
  assert.equal(run(`${ESC}]${'a'.repeat(MAX_OSC_LENGTH)}\x07`).calls.length, 1);
  assert.deepEqual(run(`${ESC}]0;${'a'.repeat(MAX_OSC_LENGTH * 5)}\x07x`).calls, [['print', 'x']]);
});

test('OSC: an empty string is still a string', () => {
  assert.deepEqual(run(`${ESC}]\x07`).calls, [['osc', '']]);
});

test('DCS, SOS, PM and APC strings are dropped up to ST', () => {
  for (const opener of ['P', 'X', '^', '_']) {
    const { calls, parser } = run(`a${ESC}${opener}anything 1;2m \x07 ... `);
    assert.equal(parser.state, State.STRING);
    assert.deepEqual(calls, [['print', 'a']]);
    parser.write(bytesOf(`${ESC}\\b`));
    assert.deepEqual(calls, [['print', 'a'], ['escape', '', '\\'], ['print', 'b']]);
  }
});

test('a string state holds text, BEL and controls, until ESC, CAN or SUB', () => {
  const states = (text) => run(text).parser.state;
  assert.equal(states(`${ESC}Pabc\x07\x01\n`), State.STRING);
  assert.equal(states(`${ESC}_abc\x18`), State.GROUND);
  assert.equal(states(`${ESC}^abc\x1a`), State.GROUND);
  assert.equal(states(`${ESC}Xabc${ESC}`), State.ESCAPE);
  assert.deepEqual(run(`${ESC}Pabc\x18ok`).calls, [['print', 'o'], ['print', 'k']]);
});

test('reset drops a half-read sequence and a half-read character', () => {
  const handler = recorder();
  const parser = new Parser(handler);
  parser.write(bytesOf(`${ESC}[1;`));
  parser.reset();
  assert.equal(parser.state, State.GROUND);
  parser.write(bytesOf('m'));
  parser.write(Uint8Array.of(0xe2, 0x82));
  parser.reset();
  parser.write(Uint8Array.of(0xac));
  assert.deepEqual(handler.calls, [['print', 'm'], ['print', '�']]);
});

test('text written after any sequence prints as it did before', () => {
  assert.deepEqual(run(`x${ESC}[1m${ESC}]0;t\x07${ESC}(0y`).calls.filter((call) => call[0] === 'print'), [
    ['print', 'x'], ['print', 'y'],
  ]);
});
