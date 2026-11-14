// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The services' parts of the Overview (design 3.5), from a fixture of `GET
// /api/1/services`: each failed service is a need, with its words from
// Spark's state, and the badge counts them.

import assert from 'node:assert/strict';
import test from 'node:test';

import { count } from '../pages/services/badge.js';
import { badgeOf, detailOf, failedOf, needsOf } from '../pages/services/needs.js';
import { needs } from '../pages/services/overview.js';
import { fakeCx, NOW, services } from './overview-fixtures.mjs';

test('a failed service is a need, with the words Spark has of it', () => {
  const found = needsOf(services, NOW);
  assert.equal(found.length, 1);
  assert.deepEqual(found[0], {
    id: 'service-failed-backup-sync',
    severity: 'bad',
    title: 'backup-sync failed',
    detail: 'It gave up after 3 starts that failed in a row. Its last run exited with code 1. It last said: the NAS at 192.168.1.20 did not answer.',
    href: '/services/backup-sync',
    icon: 'services',
    at: NOW - 3 * 3600,
    covers: ['SEC-150'],
  });
});

test('a service that is stopped, disabled or off as meant is not a need', () => {
  assert.deepEqual(failedOf(services).map((one) => one.name), ['backup-sync']);
  assert.deepEqual(needsOf({ now_ms: 1, services: [{ name: 'a', state: 'stopped', why: 'x' }, { name: 'b', state: 'waiting' }] }, NOW), []);
});

test('what Spark says of a service is told in the order it matters, and what it does not say is left out', () => {
  assert.equal(detailOf({ failures: 1 }), 'It gave up after 1 start that failed in a row.');
  assert.equal(detailOf({ last_end: 'was killed by signal 9.' }), 'Its last run was killed by signal 9.');
  assert.equal(detailOf({ why: 'it requires db, which is stopped' }), 'It requires db, which is stopped.');
  assert.equal(detailOf({ last_line: 'cannot bind to port 80!' }), 'It last said: cannot bind to port 80!');
  assert.equal(detailOf({}), 'It is not running.');
  assert.equal(detailOf({ failures: 2, last_end: 'exited with code 3', why: 'it requires db' }),
    'It gave up after 2 starts that failed in a row. Its last run exited with code 3. It requires db.');
});

test('a time that cannot be worked out is none, and a name is made safe in the address', () => {
  const [odd] = needsOf({ services: [{ name: 'a b/c', state: 'failed' }] }, NOW);
  assert.equal(odd.at, null);
  assert.equal(odd.href, '/services/a%20b%2Fc');
  const [fresh] = needsOf({ now_ms: 1000, services: [{ name: 'x', state: 'failed', since_ms: 5000 }] }, NOW);
  assert.equal(fresh.at, NOW, 'a time ahead of the document is now');
});

test('an answer that is not the document is no services', () => {
  for (const answer of [null, undefined, {}, { services: null }, []]) {
    assert.deepEqual(needsOf(answer, NOW), []);
    assert.equal(badgeOf(answer), null);
  }
});

test('the badge counts the failed services and is gone when none failed', () => {
  assert.deepEqual(badgeOf(services), { count: 1, label: 'failed', kind: 'bad' });
  const two = { services: [{ name: 'a', state: 'failed' }, { name: 'b', state: 'failed' }, { name: 'c', state: 'running' }] };
  assert.deepEqual(badgeOf(two), { count: 2, label: 'failed', kind: 'bad' });
  assert.equal(badgeOf({ services: [{ name: 'a', state: 'running' }] }), null);
});

test('the parts read the services once between them and give the same findings', async () => {
  const cx = fakeCx({ '/api/1/services': services });
  const [found, badge] = await Promise.all([needs(cx), count(cx)]);
  assert.equal(found[0].id, 'service-failed-backup-sync');
  assert.deepEqual(badge, { count: 1, label: 'failed', kind: 'bad' });
  assert.deepEqual(cx.calls, ['/api/1/services']);
});

test('a server that does not answer makes the part fail, so that the shell leaves it out', async () => {
  await assert.rejects(needs(fakeCx({})), /no answer/);
  await assert.rejects(count(fakeCx({})), /no answer/);
});
