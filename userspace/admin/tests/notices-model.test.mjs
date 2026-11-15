// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/notices-model.js: three at most, 8 seconds, and a button keeps one.

import assert from 'node:assert/strict';
import test from 'node:test';

import { GONE_MS, lifetime, makeNotices } from '../shell/notices-model.js';

test('a notice has words and a kind, and a plain one goes after 8 seconds', () => {
  const notices = makeNotices();
  const { notice, fresh } = notices.add('Saved.', { kind: 'ok' });
  assert.deepEqual([notice.words, notice.kind, fresh], ['Saved.', 'ok', true]);
  assert.equal(lifetime(notice), GONE_MS);
  assert.equal(GONE_MS, 8000);
});

test('a notice with a button, or one that is kept, stays until it is closed', () => {
  const notices = makeNotices();
  const withButton = notices.add('Restarting nginx: running as task 42', { action: { label: 'Open Tasks', run() {} } }).notice;
  const kept = notices.add('Kept.', { keep: true }).notice;
  assert.equal(lifetime(withButton), null);
  assert.equal(lifetime(kept), null);
});

test('an unknown kind is information', () => {
  assert.equal(makeNotices().add('x', { kind: 'loud' }).notice.kind, 'info');
});

test('the newest is last, and at most three are shown: the oldest plain one goes first', () => {
  const notices = makeNotices();
  notices.add('one', { keep: true });
  notices.add('two');
  notices.add('three');
  const { dropped } = notices.add('four');
  assert.deepEqual(dropped.map((one) => one.words), ['two']);
  assert.deepEqual(notices.list().map((one) => one.words), ['one', 'three', 'four']);
});

test('when all three are kept, the oldest goes', () => {
  const notices = makeNotices();
  for (const words of ['a', 'b', 'c']) notices.add(words, { keep: true });
  const { dropped } = notices.add('d', { keep: true });
  assert.deepEqual(dropped.map((one) => one.words), ['a']);
});

test('the same words again are the notice already shown', () => {
  const notices = makeNotices();
  const first = notices.add('Copied.').notice;
  const again = notices.add('Copied.');
  assert.equal(again.fresh, false);
  assert.equal(again.notice, first);
  assert.equal(notices.list().length, 1);
  assert.equal(notices.add('Copied.', { kind: 'ok' }).fresh, true, 'another kind is another notice');
});

test('closing takes one away and gives it back, or null', () => {
  const notices = makeNotices();
  const { notice } = notices.add('x');
  assert.equal(notices.close(notice.id), notice);
  assert.equal(notices.close(notice.id), null);
  assert.deepEqual(notices.list(), []);
});
