// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview page (design 10) with the shell's own parts under it: what
// the What changed card and the Health list say of one hand edit, and that
// Keep it takes the edit out of Health at once. Keeping an edit sends no
// `change` event, so nothing but the page makes the parts forget the answer
// they kept for the list.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';
import { makeTime } from '../lib/time.js';
import { show } from '../pages/overview/page.js';
import { makeParts } from '../shell/parts.js';
import { setUnlockSeconds } from '../unlock.js';

const tick = () => new Promise((resolve) => setImmediate(resolve));

/** Waits, a turn at a time, until `done()` is true. */
async function until(done) {
  for (let waited = 0; !done() && waited < 50; waited += 1) await tick();
}

const failure = (id) => Object.assign(new Error(id), { id, status: 404 });

/**
 * The Overview on a page, with a journal that has one unseen hand edit, 12.
 * Gives the page's `main`, the journal and the paths that were asked.
 */
async function overview(context) {
  const page = usePage(context);
  // The ring and the meters take a custom property.
  const make = page.document.createElement;
  page.document.createElement = (tag) => {
    const element = make(tag);
    element.style = { setProperty() {} };
    return element;
  };
  context.after(() => setUnlockSeconds(null));
  setUnlockSeconds(900);
  const now = Math.floor(Date.now() / 1000);
  const edit = {
    id: 12, at: now - 60, who: 'alice', how: 'by hand', action: null, title: null, note: null, seen: null,
    files: [{ path: '/system/settings/spark/services/nginx.toml' }],
  };
  const journal = [edit];
  const asked = [];
  const get = async (path) => {
    asked.push(path);
    if (path === '/api/1/tasks') return { tasks: [] };
    if (path.startsWith('/api/1/changes?')) return { changes: structuredClone(journal) };
    if (path === '/api/1/changes/12') return { ...structuredClone(edit), put_back: { possible: false } };
    if (path === '/api/1/console/about') return { last_good_start: null, up: 100, zone: 'UTC', clock: { synchronized: true } };
    if (path === '/api/1/checks/last') throw failure('ADM-123');
    throw failure('ADM-000');
  };
  const session = { name: 'alice', admin: true };
  const time = makeTime('UTC');
  const parts = makeParts({
    pages: [{ id: 'overview', parts: { overview: () => import('../pages/overview/overview.js') } }],
    get,
    session: () => session,
    time: () => time,
    warn() {},
    note() {},
  });
  const main = page.document.createElement('div');
  const view = {
    main,
    query: new URLSearchParams(),
    session,
    time,
    signal: new AbortController().signal,
    head() {},
    on() {},
    every() {},
    notice() {},
    problem(error) { throw error; },
    api: {
      get,
      post: async (path) => {
        asked.push(`POST ${path}`);
        edit.seen = { by: 'alice', at: now };
        return { done: true };
      },
    },
    parts: parts.collect,
    forgetParts: parts.forget,
  };
  await show(view);
  const health = () => main.querySelector('#ov-health');
  await until(() => health().querySelector('ul.need') !== null);
  return { main, health, asked };
}

test('a hand edit is a row of Health and an item of What changed, each named by the change it is', async (context) => {
  const { main, health } = await overview(context);
  assert.deepEqual(health().querySelectorAll('ul.need li').map((row) => row.getAttribute('data-need')), ['change-12']);
  assert.equal(main.querySelectorAll('#ov-changed .tl-item').map((item) => item.getAttribute('data-change')).join(), '12');
});

test('Keep it takes the edit out of Health at once, though the parts kept the list for five seconds', async (context) => {
  const { main, health, asked } = await overview(context);
  main.querySelector('#ov-change-12-keep-it').click();
  await until(() => health().querySelector('ul.need') === null);
  assert.equal(health().querySelector('ul.need') === null, true, 'the row is still in Health');
  assert.equal(asked.includes('POST /api/1/changes/12/keep'), true);
  assert.match(health().querySelector('.look').textContent, /^Nothing to look at$/);
});
