// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of websites (pages/websites/palette.js): each site
// with what it serves and how, the states to filter by, Add a website and a
// way to get a certificate for an administrator, and the phrase that removes a
// site. The actions of websites are written here as the registry will list them.

import assert from 'node:assert/strict';
import test from 'node:test';

import * as websites from '../pages/websites/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import { list, site } from './websites-fixtures.mjs';

const REMOVE = {
  id: 'websites.remove',
  title: 'Remove a website',
  area: 'websites',
  changes: true,
  method: 'POST',
  path: '/api/1/websites/{name}/remove',
  runs: 'as-administrator',
  long: true,
  input: {
    type: 'object',
    properties: {
      name: { type: 'string', minLength: 1, maxLength: 253, pattern: '^[a-z0-9][a-z0-9.-]*$' },
      files: { type: 'boolean' },
    },
    required: ['name'],
    additionalProperties: false,
  },
};

/** A `cx` that answers the read of the sites with `answer`, and counts the reads. */
function fakeCx(answer, admin = true) {
  const calls = [];
  const kept = new Map();
  return {
    calls,
    session: { admin },
    api: { get: async (path) => { calls.push(path); return answer; } },
    cache: (key, ms, make) => {
      if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
      return kept.get(key);
    },
  };
}

const indexFor = async (answer, admin = true) => {
  const cx = fakeCx(answer, admin);
  const groups = [];
  for (const provider of websites.providers) groups.push({ ...provider, items: await provider.items(cx) });
  const collected = [{ entry: PAGES.find((one) => one.id === 'websites'), value: { phrases: websites.phrases, groups } }];
  return { made: buildIndex({ pages: PAGES, actions: [REMOVE], collected, session: { admin } }), cx };
};

const titles = (made, query) => find(made, query).map((one) => one.row.title);

test('the providers and the phrase: the sites and the page\'s own things, under Websites, and Remove {name}', () => {
  const [sites, own] = websites.providers;
  assert.deepEqual([sites.group, sites.kind, Object.keys(sites.filters), sites.filters.state], ['Websites', 'website', ['state'], ['http-only', 'by-hand']]);
  assert.deepEqual([own.group, own.kind, own.filters], ['Websites', 'website', undefined]);
  assert.deepEqual(websites.phrases, { 'websites.remove': 'Remove {name}' });
});

test('each site is a row with what it serves and its HTTPS, its first name, and the address of its page', () => {
  const rows = websites.itemsOf(list());
  assert.deepEqual(rows.map((row) => row.title), ['shop.example.org', 'blog.example.org', 'duke-nas.local', 'legacy.example.org']);
  assert.deepEqual(rows.map((row) => row.detail), ['PHP · HTTPS', 'Files · HTTP only', 'Files · HTTPS', 'By hand · HTTP only']);
  assert.deepEqual(rows.map((row) => row.href), ['/websites/shop.example.org', '/websites/blog.example.org', '/websites/duke-nas.local', '/websites/legacy']);
  assert.deepEqual(rows.map((row) => row.state), ['', 'http-only', '', 'by-hand']);
  assert.deepEqual(rows.map((row) => row.args), rows.map((row) => ({ name: row.key })));
  assert.equal(rows.some((row) => 'action' in row), false, 'a row opens the page and runs nothing');
});

test('a site called add keeps its own page apart from Add a website', () => {
  const [row] = websites.itemsOf({ websites: [site('add')] });
  assert.equal(row.href, '/websites/%61dd');
  assert.equal(websites.pageItems({ websites: [] }, true)[0].href, '/websites/add');
});

test('an administrator has Add a website and a certificate to get for each site on HTTP that website wrote, and a reader has neither', () => {
  const rows = websites.pageItems(list(), true);
  assert.deepEqual(rows.map((row) => [row.title, row.href]), [
    ['Add a website', '/websites/add'],
    ['Get a certificate for blog.example.org', '/websites/blog.example.org'],
  ]);
  assert.deepEqual(websites.pageItems(list(), false), []);
  assert.deepEqual(websites.pageItems(null, true).map((row) => row.title), ['Add a website']);
  assert.equal(new Set(rows.map((row) => row.key)).size, rows.length, 'no two rows share a key');
});

test('the sites are found by what a person types, and state:http-only and state:by-hand keep those', async () => {
  const { made } = await indexFor(list());
  assert.ok(titles(made, 'shop').includes('shop.example.org'));
  assert.deepEqual(titles(made, 'state:http-only').filter((title) => title.includes('example.org')), ['blog.example.org']);
  assert.deepEqual(titles(made, 'state:by-hand'), ['legacy.example.org']);
  const row = find(made, 'blog.example.org').find((one) => one.row.title === 'blog.example.org').row;
  assert.deepEqual(row.target, { path: '/websites/blog.example.org' });
  assert.equal(row.group, 'Websites');
});

test('each site has the phrase Remove {name}, which asks the administrator before it runs', async () => {
  const { made } = await indexFor(list());
  const row = find(made, 'remove blog').find((one) => one.row.title === 'Remove blog.example.org')?.row;
  assert.ok(row, titles(made, 'remove blog').join(' | '));
  assert.deepEqual(row.target, { action: 'websites.remove', args: { name: 'blog.example.org' } });
  assert.equal(row.change, true);
});

test('a reader finds the sites, and none of the page\'s own rows', async () => {
  const { made } = await indexFor(list(), false);
  assert.ok(titles(made, 'shop').includes('shop.example.org'));
  assert.equal(titles(made, 'add a website').includes('Add a website'), false);
  assert.equal(titles(made, 'get a certificate for').some((title) => title.startsWith('Get a certificate for')), false);
});

test('the rows read the sites once, through the cache, for both providers', async () => {
  const { cx } = await indexFor(list());
  assert.deepEqual(cx.calls, ['/api/1/websites']);
});
