// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the AI clients page gives the shell: the entry of the page itself,
// and the navigation's count of what waits for a yes.

import assert from 'node:assert/strict';
import test from 'node:test';

import { count } from '../pages/clients/badge.js';
import { PAGES } from '../pages.js';
import { navEntries } from '../shell/nav-model.js';
import { request, requests } from './clients-fixtures.mjs';

/** A `cx` for `session` that answers the requests with `answer`, and counts the calls. */
function fakeCx(session, answer) {
  const calls = [];
  return {
    calls,
    session,
    api: { get: async (path) => { calls.push(path); if (answer instanceof Error) throw answer; return answer; } },
  };
}

test('the page is listed for administrators only, after Security, with its own icon and badge', () => {
  const entry = PAGES.find((one) => one.id === 'clients');
  assert.deepEqual([entry.title, entry.path, entry.icon, entry.needs], ['AI clients', '/clients', 'ai', 'admin']);
  const ids = (session) => navEntries(PAGES, session).main.map((one) => one.id);
  const admin = ids({ admin: true, terminal: true });
  assert.equal(admin[admin.indexOf('security') + 1], 'clients');
  assert.equal(ids({ admin: false, terminal: false }).includes('clients'), false);
  assert.equal(typeof entry.parts.badge, 'function');
});

test('the badge counts the requests that wait, for an administrator', async () => {
  const cx = fakeCx({ name: 'chase', admin: true }, requests(request(), request({ request: 13 })));
  assert.deepEqual(await count(cx), { count: 2, label: 'waiting for a yes', kind: 'info' });
  assert.deepEqual(cx.calls, ['/api/1/clients/requests']);
  assert.equal(await count(fakeCx({ name: 'chase', admin: true }, { requests: [] })), null);
  assert.equal(await count(fakeCx({ name: 'chase', admin: true }, {})), null);
});

test('a person who is no administrator has no badge, and nothing is read for one', async () => {
  const cx = fakeCx({ name: 'bob', admin: false }, requests());
  assert.equal(await count(cx), null);
  assert.deepEqual(cx.calls, []);
});
