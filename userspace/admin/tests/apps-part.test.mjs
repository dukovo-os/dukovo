// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The palette part of the Apps folder: the rows it makes of `GET /api/1/apps`
// and the last check, the states they are found by, and the changes that name
// an app, with a fake `cx` and the rows the palette makes of them with its
// index.

import assert from 'node:assert/strict';
import test from 'node:test';

import { phrases, providers, STATES } from '../pages/apps/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import { last, list } from './apps-fixtures.mjs';

/** An action of the registry as `GET /api/1/actions` lists it, taking the app's ID. */
const action = (id, title, more = {}) => ({
  id,
  title,
  area: 'apps',
  changes: true,
  runs: 'as-administrator',
  input: {
    type: 'object',
    properties: { id: { type: 'string', minLength: 1, maxLength: 128, pattern: '^[A-Za-z0-9][A-Za-z0-9._@-]*$' }, ...more },
    required: ['id'],
    additionalProperties: false,
  },
});

const ACTIONS = [
  action('apps.show', 'Show an app', {}),
  action('apps.update', 'Update an app', { yes: { type: 'boolean' } }),
  action('apps.hold', 'Hold an app'),
  action('apps.rollback', 'Go back to the version an app kept'),
];

/** A `cx` that answers `GET` of each path in `answers`, and counts the calls. */
function fakeCx(answers) {
  const calls = [];
  const kept = new Map();
  return {
    calls,
    api: {
      get: async (path) => {
        calls.push(path);
        if (answers[path] instanceof Error) throw answers[path];
        return answers[path];
      },
    },
    cache: (key, ms, make) => {
      if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
      return kept.get(key);
    },
  };
}

test('the apps are rows with their version and what waits, the apps of a person left out, and the flawed first', async () => {
  const cx = fakeCx({ '/api/1/apps': list(), '/api/1/updates': last() });
  const [provider] = providers;
  assert.deepEqual([provider.group, provider.kind, provider.filters], ['Apps', 'app', { state: STATES }]);
  const items = await provider.items(cx);
  assert.deepEqual(items.map((item) => item.key), [
    'io.valkey.valkey', 'net.php.php-fpm', 'org.postgresql.postgresql', 'io.nginx.nginx', 'org.python.python', 'org.sqlite.sqlite',
  ]);
  assert.deepEqual(items[0], {
    key: 'io.valkey.valkey',
    title: 'Valkey',
    detail: '8.0.3 · update waits · 2 known flaws, worst high',
    state: 'flawed',
    href: '/apps/io%2Evalkey%2Evalkey',
    action: 'apps.show',
    args: { id: 'io.valkey.valkey' },
  });
  assert.deepEqual(items.map((item) => item.state), ['flawed', 'update', 'update', '', '', 'held']);
  assert.deepEqual(cx.calls.sort(), ['/api/1/apps', '/api/1/updates']);
});

test('an app is a row when the last check cannot be read, and none when the apps cannot', async () => {
  const [provider] = providers;
  const without = await provider.items(fakeCx({ '/api/1/apps': list(), '/api/1/updates': new Error('no') }));
  assert.equal(without.length, 6);
  assert.equal(without.find((item) => item.key === 'org.postgresql.postgresql').detail, '17.2 · update waits');
  await assert.rejects(provider.items(fakeCx({ '/api/1/apps': new Error('no'), '/api/1/updates': last() })));
});

test('the phrases are for update, hold and going back, each naming the app by the ID the action takes', () => {
  assert.deepEqual(phrases, { 'apps.update': 'Update {id}', 'apps.hold': 'Hold {id}', 'apps.rollback': 'Go back on {id}' });
});

test('the palette finds an app by its state, and the changes that name it', async () => {
  const cx = fakeCx({ '/api/1/apps': list(), '/api/1/updates': last() });
  const [provider] = providers;
  const collected = [{ entry: PAGES.find((one) => one.id === 'apps'), value: { phrases, groups: [{ ...provider, items: await provider.items(cx) }] } }];
  const made = buildIndex({ pages: PAGES, actions: ACTIONS, collected, session: { admin: true } });
  const titles = (query) => find(made, query).map((one) => one.row.title);
  assert.deepEqual(titles('state:flawed'), ['Valkey']);
  assert.deepEqual(titles('state:held'), ['SQLite']);
  assert.deepEqual(titles('state:update').sort(), ['PHP-FPM', 'PostgreSQL']);
  assert.equal(titles('hold valkey')[0], 'Hold io.valkey.valkey');
  assert.equal(titles('go back valkey')[0], 'Go back on io.valkey.valkey');
  assert.equal(titles('update postgres')[0], 'Update org.postgresql.postgresql');
  assert.ok(titles('valkey').includes('Valkey'));
});
