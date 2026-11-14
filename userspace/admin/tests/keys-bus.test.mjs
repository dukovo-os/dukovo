// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/keys.js and shell/bus.js: which keys the shell takes, and the bus
// that tells pages of events.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeBus } from '../shell/bus.js';
import { followKeys, keyAction } from '../shell/keys.js';

const press = (more) => ({ key: '', code: '', ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...more });

test('Ctrl K and Meta K open the palette, and Ctrl / shows the keys', () => {
  assert.equal(keyAction(press({ key: 'k', ctrlKey: true }), false), 'palette');
  assert.equal(keyAction(press({ key: 'K', metaKey: true }), false), 'palette');
  assert.equal(keyAction(press({ key: '/', ctrlKey: true }), false), 'shortcuts');
});

test('a key of one character alone is nobody\'s shortcut, and other chords are left to the browser', () => {
  for (const key of ['k', '/', '?', 'g', 'Escape']) assert.equal(keyAction(press({ key }), false), null, key);
  assert.equal(keyAction(press({ key: 'k', ctrlKey: true, shiftKey: true }), false), null);
  assert.equal(keyAction(press({ key: 'k', ctrlKey: true, altKey: true }), false), null);
});

test('in a terminal pane Ctrl K belongs to the program, and Alt Shift K opens the palette', () => {
  assert.equal(keyAction(press({ key: 'k', ctrlKey: true }), true), null);
  assert.equal(keyAction(press({ key: '/', ctrlKey: true }), true), null);
  assert.equal(keyAction(press({ key: 'K', code: 'KeyK', altKey: true, shiftKey: true }), true), 'palette');
  assert.equal(keyAction(press({ key: 'K', code: 'KeyK', altKey: true, shiftKey: true }), false), null, 'only from a pane');
});

test('the keys are followed until the shell stops, so a new shell in the same page is the only one that answers', () => {
  const listeners = new Set();
  const before = globalThis.document;
  globalThis.document = {
    addEventListener: (name, listener) => name === 'keydown' && listeners.add(listener),
    removeEventListener: (name, listener) => name === 'keydown' && listeners.delete(listener),
  };
  try {
    const asked = [];
    const first = followKeys({ openPalette: () => asked.push('first'), openShortcuts: () => asked.push('shortcuts') });
    const second = followKeys({ openPalette: () => asked.push('second'), openShortcuts: () => asked.push('shortcuts') });
    first();
    const event = { ...press({ key: 'k', ctrlKey: true }), target: { closest: () => null }, preventDefault: () => asked.push('prevented') };
    for (const listener of listeners) listener(event);
    assert.deepEqual(asked, ['prevented', 'second']);
    second();
    assert.equal(listeners.size, 0);
  } finally {
    globalThis.document = before;
  }
});

test('the bus tells each listener, stops one that is off and survives one that throws', () => {
  const bus = makeBus();
  const seen = [];
  const off = bus.on('task', (data) => seen.push(['a', data]));
  const noisy = console.error;
  console.error = () => {};
  bus.on('task', () => { throw new Error('bad listener'); });
  bus.on('task', (data) => seen.push(['b', data]));
  bus.emit('task', 1);
  off();
  bus.emit('task', 2);
  bus.emit('nobody', 3);
  console.error = noisy;
  assert.deepEqual(seen, [['a', 1], ['b', 1], ['b', 2]]);
});
