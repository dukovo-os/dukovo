// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The certificates' part of the Overview (design 3.5), from a fixture of
// `GET /api/1/certificates`: a certificate with 14 days or fewer left, one
// that has ended and one that was revoked are needs, most urgent first.

import assert from 'node:assert/strict';
import test from 'node:test';

import { listOf, needsOf, URGENT_DAYS, WARN_DAYS } from '../pages/certificates/needs.js';
import { needs } from '../pages/certificates/overview.js';
import { certificates, fakeCx } from './overview-fixtures.mjs';

test('the limits are 14 days to look and 3 to hurry', () => {
  assert.equal(WARN_DAYS, 14);
  assert.equal(URGENT_DAYS, 3);
});

test('a certificate with 14 days or fewer left, or ended, or revoked is a need, and one with more is not', () => {
  const found = needsOf(certificates);
  assert.deepEqual(found.map((one) => [one.id, one.severity, one.title]), [
    ['certificate-revoked-stolen', 'bad', 'stolen was revoked'],
    ['certificate-ends-old', 'bad', 'old has ended'],
    ['certificate-ends-home', 'bad', 'home ends tomorrow'],
    ['certificate-ends-blog', 'warn', 'blog ends in 9 days'],
  ]);
});

test('the words say what uses it and what to do, by the kind of certificate', () => {
  const byId = Object.fromEntries(needsOf(certificates).map((one) => [one.id, one]));
  assert.equal(byId['certificate-ends-blog'].detail, 'nginx and valkey use it. Renew it.');
  assert.equal(byId['certificate-ends-old'].detail, 'It ended 3 days ago. mail uses it. It was added by hand: add a new one.');
  assert.equal(byId['certificate-ends-home'].detail, 'Renew it.');
  assert.equal(byId['certificate-revoked-stolen'].detail, 'The authority that made it has revoked it, so browsers do not trust it. Renew it to get a new one.');
  assert.equal(byId['certificate-ends-blog'].href, '/certificates/blog');
  assert.equal(byId['certificate-ends-blog'].icon, 'certs');
  assert.equal(byId['certificate-ends-blog'].at, null);
});

test('the check that warns of a certificate that ends soon is covered, and a revoked one covers none', () => {
  const found = needsOf(certificates);
  assert.deepEqual(found.find((one) => one.id === 'certificate-ends-blog').covers, ['MET-104']);
  assert.equal(found.find((one) => one.id === 'certificate-revoked-stolen').covers, undefined);
});

test('the days left give the title its words: today, tomorrow, a number, and an ended one the other way', () => {
  const title = (days) => needsOf([{ name: 'x', days_left: days }])[0].title;
  assert.equal(title(0), 'x ends today');
  assert.equal(title(1), 'x ends tomorrow');
  assert.equal(title(14), 'x ends in 14 days');
  assert.equal(title(-1), 'x has ended');
  assert.deepEqual(needsOf([{ name: 'x', days_left: 15 }]), []);
  assert.equal(needsOf([{ name: 'x', days_left: -1 }])[0].detail, 'It ended 1 day ago. Renew it.');
});

test('the most urgent come first: by how bad, then by how few days are left', () => {
  const list = [{ name: 'c', days_left: 10 }, { name: 'a', days_left: 12 }, { name: 'b', days_left: 2 }, { name: 'd', days_left: 0 }];
  assert.deepEqual(needsOf(list).map((one) => one.id.replace('certificate-ends-', '')), ['d', 'b', 'c', 'a']);
});

test('a certificate with no days left to count is not a need, and the list may be in an object', () => {
  assert.deepEqual(needsOf([{ name: 'x' }, { name: 'y', days_left: null }, { name: 'z', days_left: 'soon' }]), []);
  assert.equal(listOf({ certificates: [1] }).length, 1);
  for (const answer of [null, undefined, {}, 'x', { certificates: 3 }]) assert.deepEqual(needsOf(answer), []);
});

test('the part reads the list of certificates', async () => {
  const cx = fakeCx({ '/api/1/certificates': certificates });
  assert.equal((await needs(cx)).length, 4);
  assert.deepEqual(cx.calls, ['/api/1/certificates']);
  await assert.rejects(needs(fakeCx({})), /no answer/);
});
