// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The palette parts of the services and certificates folders (design 3.5):
// what they make of the answers of `GET /api/1/services` and
// `GET /api/1/certificates`, with a fake `cx`, and the rows the palette
// makes of them with its index.

import assert from 'node:assert/strict';
import test from 'node:test';

import * as certificates from '../pages/certificates/palette.js';
import * as services from '../pages/services/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import { listedActions } from './registry.mjs';

const SERVICES = {
  show_version: 1,
  services: [
    { name: 'nginx', description: 'Web server', state: 'running', enabled: true },
    { name: 'backup-sync', description: 'Copies to the NAS', state: 'failed', enabled: true },
    { name: 'diskhealth', description: 'Watches the disks', state: 'stopped', enabled: false },
    { name: 'admind', description: 'The console', state: 'running', enabled: true },
    { name: '', state: 'running' },
    { state: 'running' },
    null,
  ],
};

const CERTIFICATES = [
  { name: 'this-machine', days_left: 59, used_by: ['admind'], trusted: true },
  { name: 'blog.example.org', days_left: 9, used_by: ['nginx', 'blog'], renews: '2026-11-20T00:00:00Z' },
  { name: 'old.example.org', days_left: -3, used_by: [] },
  { name: 'gone.example.org', days_left: 200, revoked: true },
  { name: 'today.example.org', days_left: 0 },
  { name: 'one.example.org', days_left: 1, used_by: ['x'] },
  { name: 'no-days' },
];

/** A `cx` that answers `GET` of each path in `answers`, and counts the calls. */
function fakeCx(answers) {
  const calls = [];
  const kept = new Map();
  return {
    calls,
    api: { get: async (path) => { calls.push(path); return answers[path]; } },
    cache: (key, ms, make) => {
      if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
      return kept.get(key);
    },
  };
}

test('services are rows with their state words, the failed first and then by name', async () => {
  const cx = fakeCx({ '/api/1/services': SERVICES });
  const [provider] = services.providers;
  assert.deepEqual([provider.group, provider.kind], ['Services', 'service']);
  assert.deepEqual(provider.filters.state.slice(0, 3), ['failed', 'running', 'stopped']);
  const items = await provider.items(cx);
  assert.deepEqual(items.map((item) => item.key), ['backup-sync', 'admind', 'diskhealth', 'nginx']);
  assert.deepEqual(items[0], {
    key: 'backup-sync', title: 'backup-sync', detail: 'Copies to the NAS', state: 'failed', href: '/services/backup-sync',
    action: 'service.explain', args: { name: 'backup-sync' },
  });
  assert.deepEqual(cx.calls, ['/api/1/services']);
});

test('the services answer may be Spark\'s document or the list alone, and anything else is none', () => {
  assert.equal(services.itemsOf(SERVICES.services).length, 4);
  assert.deepEqual(services.itemsOf(null), []);
  assert.deepEqual(services.itemsOf({ services: 'x' }), []);
  assert.deepEqual(services.itemsOf({}), []);
  const odd = services.itemsOf([{ name: 'a b/c', state: 5, description: 7 }]);
  assert.deepEqual([odd[0].state, odd[0].detail, odd[0].href], ['', '', '/services/a%20b%2Fc']);
});

test('the phrases are for the five changes of a service, with the name in each', () => {
  assert.deepEqual(Object.keys(services.phrases), ['service.start', 'service.stop', 'service.restart', 'service.enable', 'service.disable']);
  for (const phrase of Object.values(services.phrases)) assert.match(phrase, /\{name\}/);
});

test('a certificate has a state from its days and whether it was revoked, and a line of words', () => {
  const state = (certificate) => certificates.stateOf(certificate);
  assert.equal(state({ days_left: 59 }), 'valid');
  assert.equal(state({ days_left: 15 }), 'valid');
  assert.equal(state({ days_left: certificates.EXPIRING_DAYS }), 'expiring');
  assert.equal(state({ days_left: 0 }), 'expiring');
  assert.equal(state({ days_left: -1 }), 'expired');
  assert.equal(state({ days_left: 3, revoked: true }), 'revoked');
  assert.equal(state({}), 'valid');
  const detail = (certificate) => certificates.detailOf(certificate);
  assert.equal(detail({ days_left: 59, used_by: ['admind'] }), 'Expires in 59 days · used by admind');
  assert.equal(detail({ days_left: 1 }), 'Expires in 1 day');
  assert.equal(detail({ days_left: 0 }), 'Expires today');
  assert.equal(detail({ days_left: -3, used_by: [] }), 'Expired 3 days ago');
  assert.equal(detail({ days_left: -1 }), 'Expired 1 day ago');
  assert.equal(detail({ used_by: ['a', 'b'] }), 'used by a, b');
  assert.equal(detail({}), '');
});

test('certificates are rows, from the list or from an object that holds it', async () => {
  const cx = fakeCx({ '/api/1/certificates': CERTIFICATES });
  const [provider] = certificates.providers;
  assert.deepEqual([provider.group, provider.kind], ['Certificates', 'certificate']);
  const items = await provider.items(cx);
  assert.equal(items.length, 7);
  assert.deepEqual(items[1], {
    key: 'blog.example.org', title: 'blog.example.org', detail: 'Expires in 9 days · used by nginx, blog', state: 'expiring',
    href: '/certificates/blog.example.org', action: 'certificate.show', args: { name: 'blog.example.org' },
  });
  assert.deepEqual(items.map((item) => item.state), ['valid', 'expiring', 'expired', 'revoked', 'expiring', 'expiring', 'valid']);
  assert.equal(certificates.itemsOf({ certificates: CERTIFICATES }).length, 7);
  assert.deepEqual(certificates.itemsOf(undefined), []);
  assert.deepEqual(certificates.itemsOf({ certificates: 3 }), []);
  assert.deepEqual(certificates.phrases, { 'certificate.renew': 'Renew {name}' });
});

test('the palette finds a failed service and its restart, and a certificate and its renewal, from the parts', async () => {
  const cx = fakeCx({ '/api/1/services': SERVICES, '/api/1/certificates': CERTIFICATES });
  const collect = async (entry, module) => ({
    entry,
    value: {
      phrases: module.phrases,
      groups: await Promise.all(module.providers.map(async (provider) => ({ ...provider, items: await provider.items(cx) }))),
    },
  });
  const collected = [
    await collect(PAGES.find((one) => one.id === 'services'), services),
    await collect(PAGES.find((one) => one.id === 'certificates'), certificates),
  ];
  const made = buildIndex({ pages: PAGES, actions: listedActions(), collected, session: { admin: true } });
  assert.deepEqual(made.filterKeys, ['kind', 'state']);
  assert.deepEqual(made.hints, ['kind:service', 'state:failed']);
  const titles = (query) => find(made, query).map((one) => one.row.title);
  assert.deepEqual(titles('state:failed'), ['backup-sync']);
  assert.equal(titles('rest back')[0], 'Restart backup-sync');
  assert.equal(titles('rest disk')[0], 'Restart diskhealth');
  assert.equal(titles('renew blog')[0], 'Renew blog.example.org');
  assert.deepEqual(titles('state:expiring').sort(), ['blog.example.org', 'one.example.org', 'today.example.org']);
  assert.equal(titles('restart admind').includes('Restart admind'), false, 'the console\'s own service is not restarted from here');
  assert.ok(titles('stop').includes('Stop nginx'));
  assert.ok(titles('start admind').includes('Start admind'));
  // Both parts read the services and the certificates once each, through the cache.
  assert.deepEqual(cx.calls.sort(), ['/api/1/certificates', '/api/1/services']);
});
