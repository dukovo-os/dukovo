// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/prefs.js: read after sign-in, a change applied at once and kept,
// and put back when the server refuses it.

import assert from 'node:assert/strict';
import test from 'node:test';

import { createPrefs } from '../shell/prefs.js';

const stored = { theme: 'dark', text: 100, motion: 'full', zone: 'Europe/Prague' };

function setup({ answers = {}, fail = null } = {}) {
  const shown = [];
  const kept = [];
  const posts = [];
  const prefs = createPrefs({
    get: async () => structuredClone(stored),
    post: async (path, body) => {
      posts.push([path, body]);
      if (fail) throw fail;
      return { ...stored, ...body, ...answers };
    },
    display: { apply: (display) => shown.push(display), keep: (display) => kept.push(display) },
  });
  return { prefs, shown, kept, posts };
}

test('after sign-in the server\'s preferences are applied, kept and given the terminal\'s defaults', async () => {
  const { prefs, shown, kept } = setup();
  const value = await prefs.load();
  assert.equal(value.terminal.size, 14);
  assert.equal(value.commands, 'hidden');
  assert.deepEqual(shown.at(-1), { theme: 'dark', text: 100, motion: 'full' });
  assert.deepEqual(kept.at(-1), { theme: 'dark', text: 100, motion: 'full' });
});

test('a change posts only the keys that differ, applies at once and keeps what the server answers', async () => {
  const { prefs, shown, kept, posts } = setup();
  await prefs.load();
  const heard = [];
  prefs.onChange((value) => heard.push(value.theme));
  const value = await prefs.set({ theme: 'light', text: 100 });
  assert.deepEqual(posts, [['/api/1/account/preferences', { theme: 'light' }]]);
  assert.equal(shown.at(-1).theme, 'light');
  assert.equal(kept.at(-1).theme, 'light');
  assert.equal(value.theme, 'light');
  assert.deepEqual(heard, ['light', 'light']);
});

test('a change to what is already there posts nothing', async () => {
  const { prefs, posts } = setup();
  await prefs.load();
  await prefs.set({ theme: 'dark' });
  assert.deepEqual(posts, []);
});

test('a change the server refuses is put back and its error is thrown', async () => {
  const refusal = Object.assign(new Error('no'), { id: 'ADM-125', field: 'zone' });
  const { prefs, shown } = setup({ fail: refusal });
  await prefs.load();
  await assert.rejects(prefs.set({ zone: 'Mars/Base', theme: 'light' }), refusal);
  assert.equal(prefs.value.zone, 'Europe/Prague');
  assert.equal(prefs.value.theme, 'dark');
  assert.equal(shown.at(-1).theme, 'dark');
});

test('a preview shows other colours without saving them, and null shows the saved ones again', async () => {
  const { prefs, shown, kept, posts } = setup();
  await prefs.load();
  const keptBefore = kept.length;
  prefs.preview({ theme: 'high-contrast', text: 150 });
  assert.deepEqual(shown.at(-1), { theme: 'high-contrast', text: 150, motion: 'full' });
  prefs.preview(null);
  assert.deepEqual(shown.at(-1), { theme: 'dark', text: 100, motion: 'full' });
  assert.equal(kept.length, keptBefore);
  assert.deepEqual(posts, []);
});

test('without a display, preferences still load and change', async () => {
  const prefs = createPrefs({ get: async () => structuredClone(stored), post: async (_, body) => ({ ...stored, ...body }), display: null });
  await prefs.load();
  assert.equal((await prefs.set({ motion: 'less' })).motion, 'less');
});
