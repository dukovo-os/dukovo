// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The storage's part of the Overview (design 4.6), from fixtures of `GET
// /api/1/storage` and the 30 days of `fs.used`: what it reads, what it gives
// when the history is missing, and that a server that does not answer makes
// the part fail so that the shell leaves it out.

import assert from 'node:assert/strict';
import test from 'node:test';

import { needs } from '../pages/storage/overview.js';
import { fakeCx } from './overview-fixtures.mjs';
import { show, showMissing, showWearing, usage30d } from './storage-fixtures.mjs';

test('the part reads the disks and 30 days of use, once each', async () => {
  const cx = fakeCx({ '/api/1/storage': showWearing(), '/api/1/storage/usage/30d': usage30d({ perDay: 35e9 }) });
  const found = await needs(cx);
  assert.deepEqual(found.map((one) => one.id), ['storage-rising-SDB-SERIAL', `storage-full-${show().volumes[1].uuid}`]);
  assert.deepEqual(cx.calls.sort(), ['/api/1/storage', '/api/1/storage/usage/30d']);
});

test('a machine that has kept no history still gives what the disks say', async () => {
  const cx = fakeCx({ '/api/1/storage': showMissing() });
  const found = await needs(cx);
  assert.deepEqual(found.map((one) => one.severity), ['bad']);
  assert.equal(found[0].title, 'The data mirror has lost a disk');
});

test('every need has what the Overview needs of one', async () => {
  const cx = fakeCx({ '/api/1/storage': showWearing(), '/api/1/storage/usage/30d': usage30d({ perDay: 35e9 }) });
  for (const need of await needs(cx)) {
    assert.equal(typeof need.id, 'string');
    assert.ok(['bad', 'warn', 'info'].includes(need.severity));
    assert.ok(need.title.length > 0 && need.detail.length > 0);
    assert.ok(need.href.startsWith('/storage'));
    assert.equal(need.icon, 'storage');
    assert.ok(Array.isArray(need.covers) && need.covers.length > 0);
  }
});

test('a server that does not answer makes the part fail, so that the shell leaves it out', async () => {
  await assert.rejects(needs(fakeCx({})), /no answer/);
});

test('a machine in good order gives nothing', async () => {
  assert.deepEqual(await needs(fakeCx({ '/api/1/storage': show(), '/api/1/storage/usage/30d': usage30d() })), []);
});
