// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of the Certificates page besides the certificates:
// the three things an administrator does there, each a row that goes to the
// card that does it, with no read to make and no state to filter by.

import assert from 'node:assert/strict';
import test from 'node:test';

import * as certificates from '../pages/certificates/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import { listedActions } from './registry.mjs';

/** A `cx` that makes no read: the page's own rows need none. */
const cx = (admin) => ({
  session: { admin },
  api: { get: async () => assert.fail('a row of the page asks for nothing') },
  cache: async () => assert.fail('a row of the page asks for nothing'),
});

const indexFor = async (admin) => {
  const [, own] = certificates.providers;
  const collected = [{
    entry: PAGES.find((one) => one.id === 'certificates'),
    value: { phrases: certificates.phrases, groups: [{ ...own, items: await own.items(cx(admin)) }] },
  }];
  return buildIndex({ pages: PAGES, actions: listedActions(), collected, session: { admin } });
};

const titles = (made, query) => find(made, query).map((one) => one.row.title);

test('the first provider is the certificates, and the second is the page\'s own things in the same group', () => {
  const [list, own] = certificates.providers;
  assert.deepEqual([list.group, list.kind, Object.keys(list.filters)], ['Certificates', 'certificate', ['state']]);
  assert.deepEqual([own.group, own.kind, own.filters], ['Certificates', 'certificate', undefined], 'no state to filter by');
  assert.deepEqual(certificates.phrases, { 'certificate.renew': 'Renew {name}' });
});

test('an administrator finds the three things, each going to its card, and a reader finds none', async () => {
  const [, own] = certificates.providers;
  assert.deepEqual((await own.items(cx(true))).map((item) => [item.title, item.href]), [
    ['Get a certificate', '/certificates#get'],
    ['Upload a certificate', '/certificates#upload'],
    ['Change this machine\'s name', '/certificates#name'],
  ]);
  assert.deepEqual(await own.items(cx(false)), []);
  for (const item of await own.items(cx(true))) {
    assert.equal('state' in item, false, item.title);
    assert.equal('args' in item, false, 'no phrase is made of a row that names no certificate');
  }
});

test('the rows are found by what a person types, under Certificates', async () => {
  const made = await indexFor(true);
  assert.ok(titles(made, 'get a cert').includes('Get a certificate'));
  assert.ok(titles(made, 'upload cert').includes('Upload a certificate'));
  assert.ok(titles(made, 'machine name').includes('Change this machine\'s name'));
  const row = find(made, 'upload a cert').find((one) => one.row.title === 'Upload a certificate').row;
  assert.equal(row.group, 'Certificates');
  assert.deepEqual(row.target, { path: '/certificates#upload' });
  assert.equal(row.state, null);
  assert.equal(titles(await indexFor(false), 'upload a cert').includes('Upload a certificate'), false);
});
