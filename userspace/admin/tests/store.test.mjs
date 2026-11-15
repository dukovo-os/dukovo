// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/store.js: what a browser keeps, and that a browser that refuses to
// keep anything changes nothing for the page.

import assert from 'node:assert/strict';
import test, { afterEach } from 'node:test';

import { fingerprint, forget, keep, recall } from '../lib/store.js';

const real = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');

/** Puts a `localStorage` in place that holds `entries` and throws from each call when `blocked`. */
function browserWith(entries = {}, blocked = false) {
  const held = new Map(Object.entries(entries));
  const guard = () => {
    if (blocked) throw new DOMException('blocked', 'SecurityError');
  };
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (name) => (guard(), held.get(name) ?? null),
      setItem: (name, value) => (guard(), held.set(name, value)),
      removeItem: (name) => (guard(), held.delete(name)),
    },
  });
  return held;
}

afterEach(() => {
  if (real) Object.defineProperty(globalThis, 'localStorage', real);
  else delete globalThis.localStorage;
});

test('a value kept comes back the same, under the console\'s own prefix', () => {
  const held = browserWith();
  assert.equal(keep('tasks-seen.v1', [3, 4]), true);
  assert.deepEqual(recall('tasks-seen.v1'), [3, 4]);
  assert.deepEqual([...held.keys()], ['dukovo-admin.tasks-seen.v1']);
});

test('nothing kept is the fallback, and so is a copy that is damaged', () => {
  browserWith({ 'dukovo-admin.broken': '{not json' });
  assert.equal(recall('missing'), null);
  assert.deepEqual(recall('missing', []), []);
  assert.deepEqual(recall('broken', { a: 1 }), { a: 1 });
});

test('forgetting removes a value, and forgetting what is not there does nothing', () => {
  browserWith();
  keep('a', 1);
  forget('a');
  assert.equal(recall('a'), null);
  assert.doesNotThrow(() => forget('never kept'));
});

test('a browser that refuses storage gives the fallback, keeps nothing and throws nothing', () => {
  browserWith({}, true);
  assert.equal(keep('a', 1), false);
  assert.equal(recall('a', 'fallback'), 'fallback');
  assert.doesNotThrow(() => forget('a'));
});

test('a browser with no storage at all is the same', () => {
  delete globalThis.localStorage;
  assert.equal(keep('a', 1), false);
  assert.equal(recall('a', 7), 7);
  assert.doesNotThrow(() => forget('a'));
});

test('a text has a fingerprint that is the same each time and differs for another text', () => {
  assert.equal(fingerprint('Backup failed'), fingerprint('Backup failed'));
  assert.notEqual(fingerprint('Backup failed'), fingerprint('Backup failed.'));
  assert.match(fingerprint(''), /^[0-9a-f]{8}$/);
  assert.match(fingerprint('é, 日本語, 🙂'), /^[0-9a-f]{8}$/);
  assert.equal(fingerprint('a'), 'e40c292c');
});
