// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { keyBytes, pageAction } from '../pages/terminal/term/keys.js';
import { make } from './term-screen-helpers.mjs';

// An event with only the fields that are set; the rest are off.
function ev(key, flags = '', code = '') {
  return {
    key,
    code,
    ctrlKey: flags.includes('c'),
    altKey: flags.includes('a'),
    shiftKey: flags.includes('s'),
    metaKey: flags.includes('m'),
  };
}

// The bytes as the text they spell, or null.
function send(key, flags = '', code = '', modes = {}) {
  const bytes = keyBytes(ev(key, flags, code), modes);
  return bytes === null ? null : String.fromCharCode(...bytes);
}

// The bytes `text` is in UTF-8, spelled as `send` spells them.
const utf8 = (text) => String.fromCharCode(...new TextEncoder().encode(text));

const APP = { applicationCursor: true };
const KEYPAD = { applicationKeypad: true };

test('the cursor keys in the normal and the application mode', () => {
  const keys = { ArrowUp: 'A', ArrowDown: 'B', ArrowRight: 'C', ArrowLeft: 'D', Home: 'H', End: 'F' };
  for (const [key, final] of Object.entries(keys)) {
    assert.equal(send(key), `\x1b[${final}`, key);
    assert.equal(send(key, '', '', APP), `\x1bO${final}`, `${key} in application mode`);
  }
});

test('the screen can be passed as the modes', () => {
  const screen = make(10, 4);
  assert.equal(send('ArrowUp', '', '', screen), '\x1b[A');
  screen.write(Uint8Array.from([0x1b, 0x5b, 0x3f, 0x31, 0x68, 0x1b, 0x3d]));
  assert.equal(send('ArrowUp', '', '', screen), '\x1bOA');
  assert.equal(send('5', '', 'Numpad5', screen), '\x1bOu');
});

test('a modifier makes the CSI form, in both modes', () => {
  assert.equal(send('ArrowRight', 's'), '\x1b[1;2C');
  assert.equal(send('ArrowRight', 'a'), '\x1b[1;3C');
  assert.equal(send('ArrowRight', 'as'), '\x1b[1;4C');
  assert.equal(send('ArrowRight', 'c'), '\x1b[1;5C');
  assert.equal(send('ArrowRight', 'cs'), '\x1b[1;6C');
  assert.equal(send('ArrowRight', 'ca'), '\x1b[1;7C');
  assert.equal(send('ArrowRight', 'cas'), '\x1b[1;8C');
  assert.equal(send('ArrowLeft', 'c', '', APP), '\x1b[1;5D');
  assert.equal(send('Home', 'a', '', APP), '\x1b[1;3H');
  assert.equal(send('End', 'c'), '\x1b[1;5F');
});

test('Insert, Delete, PageUp and PageDown', () => {
  assert.equal(send('Insert'), '\x1b[2~');
  assert.equal(send('Delete'), '\x1b[3~');
  assert.equal(send('PageUp'), '\x1b[5~');
  assert.equal(send('PageDown'), '\x1b[6~');
  assert.equal(send('Delete', 'c'), '\x1b[3;5~');
  assert.equal(send('PageUp', 'c'), '\x1b[5;5~');
  assert.equal(send('PageDown', 'a'), '\x1b[6;3~');
  assert.equal(send('Insert', 'a'), '\x1b[2;3~');
  assert.equal(send('Delete', '', '', APP), '\x1b[3~');
});

test('F1 to F12', () => {
  const plain = ['\x1bOP', '\x1bOQ', '\x1bOR', '\x1bOS', '\x1b[15~', '\x1b[17~', '\x1b[18~', '\x1b[19~', '\x1b[20~', '\x1b[21~', '\x1b[23~', '\x1b[24~'];
  plain.forEach((bytes, i) => assert.equal(send(`F${i + 1}`), bytes, `F${i + 1}`));
  assert.equal(send('F1', 'c'), '\x1b[1;5P');
  assert.equal(send('F4', 's'), '\x1b[1;2S');
  assert.equal(send('F5', 's'), '\x1b[15;2~');
  assert.equal(send('F12', 'ca'), '\x1b[24;7~');
  assert.equal(send('F1', '', '', APP), '\x1bOP');
});

test('keys past F12 and media keys send nothing', () => {
  for (const key of ['F13', 'F24', 'MediaPlayPause', 'AudioVolumeUp', 'PrintScreen', 'ContextMenu', 'Clear', 'Pause']) {
    assert.equal(send(key), null, key);
  }
});

test('Enter is CR, Backspace is DEL, Tab and Escape', () => {
  assert.equal(send('Enter'), '\r');
  assert.equal(send('Enter', 's'), '\r');
  assert.equal(send('Enter', 'c'), '\r');
  assert.equal(send('Enter', 'a'), '\x1b\r');
  assert.equal(send('Backspace'), '\x7f');
  assert.equal(send('Backspace', 's'), '\x7f');
  assert.equal(send('Backspace', 'c'), '\b');
  assert.equal(send('Backspace', 'a'), '\x1b\x7f');
  assert.equal(send('Backspace', 'ca'), '\x1b\b');
  assert.equal(send('Tab'), '\t');
  assert.equal(send('Tab', 's'), '\x1b[Z');
  assert.equal(send('Tab', 'a'), '\x1b\t');
  assert.equal(send('Escape'), '\x1b');
  assert.equal(send('Escape', 'a'), '\x1b\x1b');
});

test('printable characters, as UTF-8', () => {
  assert.equal(send('a'), 'a');
  assert.equal(send('A', 's'), 'A');
  assert.equal(send(' '), ' ');
  assert.equal(send('~'), '~');
  assert.deepEqual([...keyBytes(ev('\u00e9'))], [0xc3, 0xa9]);
  assert.deepEqual([...keyBytes(ev('\u4e2d'))], [0xe4, 0xb8, 0xad]);
  assert.deepEqual([...keyBytes(ev('\u{1f600}'))], [0xf0, 0x9f, 0x98, 0x80]);
});

test('Alt puts an ESC first', () => {
  assert.equal(send('b', 'a'), '\x1bb');
  assert.equal(send('B', 'as'), '\x1bB');
  assert.equal(send('.', 'a'), '\x1b.');
  assert.equal(send(' ', 'a'), '\x1b ');
  assert.deepEqual([...keyBytes(ev('\u00e9', 'a'))], [0x1b, 0xc3, 0xa9]);
});

test('Option on a Mac makes another character, and the key is what counts', () => {
  assert.equal(send('\u222b', 'a', 'KeyB'), '\x1bb');
  assert.equal(send('\u0131', 'as', 'KeyB'), '\x1bB');
  assert.equal(send('\u2022', 'a', 'Digit8'), utf8('\x1b\u2022'));
  assert.equal(send('\u222b', '', 'KeyB'), utf8('\u222b'));
});

test('Ctrl with the letters', () => {
  for (let i = 0; i < 26; i++) {
    const lower = String.fromCharCode(97 + i);
    assert.equal(send(lower, 'c', `Key${lower.toUpperCase()}`), String.fromCharCode(i + 1), `Ctrl+${lower}`);
    assert.equal(send(lower.toUpperCase(), 'cs'), String.fromCharCode(i + 1), `Ctrl+Shift+${lower}`);
  }
  assert.equal(send('c', 'ca'), '\x1b\x03');
});

test('Ctrl with a letter on a layout that is not Latin goes by the key', () => {
  assert.equal(send('\u0441', 'c', 'KeyC'), '\x03');
  assert.equal(send('\u0434', 'c', 'KeyL'), '\x0c');
  assert.equal(send('\u0434', 'c', ''), utf8('\u0434'));
  assert.equal(send('\u0434', 'c', 'Digit1'), utf8('\u0434'));
});

test('Ctrl with the usual punctuation and digits', () => {
  const table = {
    '@': 0, ' ': 0, 2: 0, '[': 27, 3: 27, '\\': 28, 4: 28, ']': 29, 5: 29,
    '^': 30, 6: 30, _: 31, '/': 31, '-': 31, 7: 31, '?': 127, 8: 127,
  };
  for (const [key, code] of Object.entries(table)) {
    assert.equal(send(key, 'c'), String.fromCharCode(code), `Ctrl+${key}`);
  }
  assert.equal(send('@', 'cs'), '\0');
});

test('Ctrl with a character that has no control code sends the character', () => {
  assert.equal(send('1', 'c'), '1');
  assert.equal(send('.', 'c'), '.');
  assert.equal(send(';', 'c'), ';');
});

test('Ctrl and Alt with a character that is no letter is AltGr', () => {
  assert.equal(send('@', 'ca'), '@');
  assert.equal(send('\\', 'ca'), '\\');
  assert.equal(send('\u20ac', 'ca'), utf8('\u20ac'));
  assert.equal(send('[', 'ca'), '[');
  assert.equal(send(' ', 'ca'), ' ');
});

test('Meta, modifiers alone, dead keys and composition send nothing', () => {
  assert.equal(send('c', 'm'), null);
  assert.equal(send('ArrowUp', 'm'), null);
  for (const key of ['Shift', 'Control', 'Alt', 'Meta', 'AltGraph', 'CapsLock', 'NumLock', 'Dead', 'Process', 'Compose', 'Unidentified']) {
    assert.equal(send(key), null, key);
  }
  assert.equal(keyBytes({ ...ev('a'), isComposing: true }), null);
});

test('events that are not keys send nothing and do not throw', () => {
  assert.equal(keyBytes({}), null);
  assert.equal(keyBytes({ key: 5 }), null);
  assert.equal(keyBytes({ key: '' }), null);
  assert.equal(keyBytes({ key: 'ab' }), null);
  assert.equal(keyBytes({ key: '\ud800' }), null);
  assert.equal(keyBytes({ key: 'constructor' }), null);
  assert.equal(keyBytes({ key: '__proto__' }), null);
  assert.equal(keyBytes({ key: 'a' }).length, 1);
});

test('the keypad in application mode', () => {
  for (let digit = 0; digit <= 9; digit++) {
    assert.equal(send(String(digit), '', `Numpad${digit}`, KEYPAD), `\x1bO${'pqrstuvwxy'[digit]}`);
  }
  const operators = { '*': 'j', '+': 'k', '-': 'm', '.': 'n', '/': 'o' };
  for (const [key, final] of Object.entries(operators)) {
    assert.equal(send(key, '', 'NumpadX', KEYPAD), `\x1bO${final}`, key);
  }
  assert.equal(send('Enter', '', 'NumpadEnter', KEYPAD), '\x1bOM');
  assert.equal(send('Enter', '', 'Enter', KEYPAD), '\r');
  assert.equal(send('5', 'a', 'Numpad5', KEYPAD), '\x1b\x1bOu');
});

test('the keypad in the normal mode, and keys of the main block, send characters', () => {
  assert.equal(send('5', '', 'Numpad5'), '5');
  assert.equal(send('+', '', 'NumpadAdd'), '+');
  assert.equal(send('Enter', '', 'NumpadEnter'), '\r');
  assert.equal(send('5', '', 'Digit5', KEYPAD), '5');
  assert.equal(send('-', '', 'Minus', KEYPAD), '-');
  assert.equal(send('ArrowLeft', '', 'Numpad4', KEYPAD), '\x1b[D');
});

test('the keys the page keeps, with the shortcuts on', () => {
  const keys = {
    KeyT: 'new-terminal', KeyW: 'end-pane', PageUp: 'previous-tab', PageDown: 'next-tab',
    ArrowLeft: 'pane-left', ArrowRight: 'pane-right', ArrowUp: 'pane-up', ArrowDown: 'pane-down',
    Backslash: 'split-right', Minus: 'split-down', KeyK: 'palette', KeyR: 'read-screen',
  };
  for (const [code, action] of Object.entries(keys)) {
    assert.equal(pageAction(ev('x', 'as', code)), action, code);
    assert.equal(pageAction(ev('x', 'as', code), true), action, code);
    assert.equal(keyBytes(ev('x', 'as', code)), null, `${code} sends nothing`);
  }
});

test('with the shortcuts off, only the way out and the palette stay', () => {
  for (const [code, action] of [['ArrowUp', 'pane-up'], ['KeyK', 'palette']]) {
    assert.equal(pageAction(ev('x', 'as', code), false), action);
    assert.equal(keyBytes(ev('x', 'as', code), { shortcuts: false }), null);
  }
  for (const code of ['KeyT', 'KeyW', 'PageUp', 'PageDown', 'ArrowLeft', 'ArrowRight', 'ArrowDown', 'Backslash', 'Minus', 'KeyR']) {
    assert.equal(pageAction(ev('x', 'as', code), false), null, code);
  }
  assert.equal(send('t', 'as', 'KeyT', { shortcuts: false }), '\x1bt');
  assert.equal(send('ArrowRight', 'as', 'ArrowRight', { shortcuts: false }), '\x1b[1;4C');
});

test('the page keeps a key by its code, whatever it makes', () => {
  assert.equal(pageAction(ev('\u02c7', 'as', 'KeyK')), 'palette');
  assert.equal(pageAction(ev('\u0442', 'as', 'KeyT')), 'new-terminal');
});

test('only Alt and Shift together are the page\'s', () => {
  assert.equal(pageAction(ev('t', 'a', 'KeyT')), null);
  assert.equal(pageAction(ev('T', 's', 'KeyT')), null);
  assert.equal(pageAction(ev('T', 'cas', 'KeyT')), null);
  assert.equal(pageAction(ev('T', 'mas', 'KeyT')), null);
  assert.equal(pageAction(ev('t', '', 'KeyT')), null);
  assert.equal(pageAction(ev('z', 'as', 'KeyZ')), null);
  assert.equal(send('t', 'a', 'KeyT'), '\x1bt');
  assert.equal(send('ArrowUp', 'a', 'ArrowUp'), '\x1b[1;3A');
});

test('pasting is the page\'s, with the shortcuts on or off', () => {
  for (const shortcuts of [true, false]) {
    assert.equal(pageAction(ev('V', 'cs', 'KeyV'), shortcuts), 'paste');
    assert.equal(pageAction(ev('Insert', 's', 'Insert'), shortcuts), 'paste');
    assert.equal(keyBytes(ev('V', 'cs', 'KeyV'), { shortcuts }), null);
    assert.equal(keyBytes(ev('Insert', 's', 'Insert'), { shortcuts }), null);
  }
  assert.equal(pageAction(ev('v', 'c', 'KeyV')), null);
  assert.equal(pageAction(ev('Insert', 'cs', 'Insert')), null);
  assert.equal(pageAction(ev('V', 'cas', 'KeyV')), null);
  assert.equal(send('Insert', 'c', 'Insert'), '\x1b[2;5~');
  assert.equal(send('v', 'c', 'KeyV'), '\x16');
});

test('Ctrl+C always sends ETX: the page decides to copy first', () => {
  assert.equal(send('c', 'c', 'KeyC'), '\x03');
  assert.equal(pageAction(ev('c', 'c', 'KeyC')), null);
});

test('an event with no code still sends its character', () => {
  const spell = (event) => String.fromCharCode(...keyBytes(event));
  assert.equal(spell({ key: '\u222b', altKey: true }), utf8('\x1b\u222b'));
  assert.equal(spell({ key: '\u0434', ctrlKey: true }), utf8('\u0434'));
  assert.equal(spell({ key: 'Enter' }), '\r');
});

test('a code that is not a key name is not a key', () => {
  for (const code of [5, {}, [], null, undefined, true, '__proto__', 'constructor', 'toString']) {
    const event = { key: 'a', code, altKey: true, shiftKey: true };
    assert.equal(pageAction(event), null, String(code));
    assert.equal(pageAction(event, false), null, String(code));
    assert.deepEqual([...keyBytes(event)], [0x1b, 0x61], String(code));
    assert.deepEqual([...keyBytes({ key: '\u222b', code, altKey: true })], [0x1b, 0xe2, 0x88, 0xab], String(code));
    assert.deepEqual([...keyBytes({ key: '\u0434', code, ctrlKey: true })], [0xd0, 0xb4], String(code));
    assert.deepEqual([...keyBytes({ key: '5', code }, { applicationKeypad: true })], [0x35], String(code));
    assert.deepEqual([...keyBytes({ key: 'Enter', code }, { applicationKeypad: true })], [0x0d], String(code));
  }
});
