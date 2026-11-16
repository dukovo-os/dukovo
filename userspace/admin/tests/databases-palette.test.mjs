// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette knows of databases (pages/databases/palette.js, seen.js):
// the rows of what the page last read, for an administrator only, and the
// address each opens; nothing is read to make them.

import assert from 'node:assert/strict';
import test from 'node:test';

import { itemsOf, providers } from '../pages/databases/palette.js';
import { readOf } from '../pages/databases/model.js';
import { remember, remembered } from '../pages/databases/seen.js';
import { buildIndex, find } from '../palette/index-model.js';
import { list } from './databases-fixtures.mjs';

const ENTRY = {
  id: 'databases', title: 'Databases', path: '/databases', icon: 'db', needs: 'admin', load: () => null,
};
const PAGES = [ENTRY];
const ADMIN = { name: 'chase', admin: true };

/** The rows the palette makes of what `read` holds, found by `query`. */
function found(read, query, session = ADMIN) {
  remember(session.name, read);
  const [provider] = providers;
  return provider.items({ session }).then((items) => {
    const collected = [{ entry: ENTRY, value: { phrases: {}, groups: [{ ...provider, items }] } }];
    return find(buildIndex({ pages: PAGES, actions: [], collected, session }), query).map((one) => one.row);
  });
}

test('an administrator has Add a database, and each database with a way to make a new password', () => {
  const rows = itemsOf(readOf(list()), ADMIN);
  assert.deepEqual(rows.map((row) => row.title), [
    'Add a database', 'blog', 'Make a new password for blog', 'notes', 'Make a new password for notes', 'shop', 'Make a new password for shop',
  ]);
  assert.deepEqual(rows.map((row) => row.href), [
    '/databases/add', '/databases/blog', '/databases/blog?password', '/databases/notes', '/databases/notes?password', '/databases/shop', '/databases/shop?password',
  ]);
  assert.equal(rows.find((row) => row.title === 'shop').detail, 'Owner shop · 38 MB · for shop.example.org');
  assert.equal(rows.find((row) => row.title === 'notes').detail, 'Owner webdev · 8 KB');
  assert.equal(new Set(rows.map((row) => row.key)).size, rows.length, 'no two rows share a key');
});

test('no row runs an action: a new password is asked for on the page, where it is shown once', () => {
  for (const row of itemsOf(readOf(list()), ADMIN)) {
    assert.equal(row.action, undefined, row.title);
    assert.equal(row.args, undefined, row.title);
  }
});

test('a reader has no row, and nothing read has only Add a database', () => {
  assert.deepEqual(itemsOf(readOf(list()), { name: 'riley', admin: false }), []);
  assert.deepEqual(itemsOf(null, ADMIN).map((row) => row.title), ['Add a database']);
  assert.deepEqual(itemsOf(null, null), []);
});

test('a database called add keeps its own page apart from Add a database', () => {
  const rows = itemsOf({ databases: [{ name: 'add', owner: 'add', bytes: null, site: null }] }, ADMIN);
  assert.equal(rows[0].href, '/databases/add');
  assert.equal(rows[1].href, '/databases/%61dd');
  assert.equal(rows[1].detail, 'Owner add');
});

test('what a person last read is theirs: another person has none, and a later read replaces it', () => {
  remember('chase', readOf(list()));
  assert.equal(remembered('chase').databases.length, 3);
  assert.equal(remembered('riley'), null);
  remember('chase', readOf(list({ databases: [] })));
  assert.equal(remembered('chase').databases.length, 0);
  remember('riley', null);
  assert.equal(remembered('chase'), null, 'one record is kept, for the last who read');
});

test('the provider reads nothing: it gives the rows of what was remembered, for the person who asks', async () => {
  remember('chase', readOf(list()));
  const [provider] = providers;
  assert.equal(provider.group, 'Databases');
  assert.equal(provider.kind, 'database');
  assert.deepEqual((await provider.items({ session: ADMIN })).map((row) => row.key).slice(0, 2), ['add', 'blog']);
  assert.deepEqual(await provider.items({ session: { name: 'riley', admin: false } }), []);
  assert.deepEqual((await provider.items({ session: { name: 'stranger', admin: true } })).map((row) => row.key), ['add']);
  assert.deepEqual((await provider.items({})).map((row) => row.key), []);
});

test('the palette finds a database by its name and opens its page, and a new password opens the page that asks', async () => {
  const rows = await found(readOf(list()), 'shop');
  const titles = rows.map((row) => row.title);
  assert.ok(titles.includes('shop'));
  assert.ok(titles.includes('Make a new password for shop'));
  assert.deepEqual(rows.find((row) => row.title === 'shop').target, { path: '/databases/shop' });
  assert.deepEqual(rows.find((row) => row.title === 'Make a new password for shop').target, { path: '/databases/shop?password' });
  assert.deepEqual((await found(readOf(list()), 'add a database'))[0].target, { path: '/databases/add' });
});

test('a reader\'s palette has none of it', async () => {
  const rows = await found(readOf(list()), 'shop', { name: 'riley', admin: false });
  assert.equal(rows.filter((row) => row.kind === 'thing').length, 0);
});
