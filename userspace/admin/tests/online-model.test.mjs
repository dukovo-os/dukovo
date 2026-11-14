// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/online-model.js and the stream watch of shell/events.js: the waits,
// the words and the failures that count.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeStreamWatch } from '../shell/events.js';
import {
  asOfWords, makeConnection, offlineWords, RESTART_QUIET_MS, retryDelay, tryingWords,
} from '../shell/online-model.js';
import { makeTime } from '../lib/time.js';

test('health is asked again after 5, 10, 30 and 60 seconds, and every 60 from then on', () => {
  assert.deepEqual([0, 1, 2, 3, 4, 5, 20].map(retryDelay), [5, 10, 30, 60, 60, 60, 60]);
});

test('the banner says who is not answering and when it looks again', () => {
  assert.equal(offlineWords('duke-nas', 30), 'duke-nas is not answering. Trying again in 30 s.');
  assert.equal(offlineWords('duke-nas', 60), 'duke-nas is not answering. Trying again in 1 min.');
  assert.equal(tryingWords('duke-nas'), 'duke-nas is not answering. Trying now.');
});

test('stale pages are marked with the time of what they show', () => {
  const time = makeTime('UTC', () => Date.UTC(2026, 10, 14, 14, 5));
  assert.equal(asOfWords(time, Date.UTC(2026, 10, 14, 14, 2) / 1000), 'As of 2:02 PM');
});

test('the connection goes offline once, looks again on the schedule and comes back at the first answer', () => {
  const link = makeConnection();
  assert.equal(link.state(0), 'online');
  assert.equal(link.lost(), true);
  assert.equal(link.lost(), false, 'a second failure is no news');
  assert.equal(link.state(0), 'offline');
  assert.deepEqual([link.nextRetry(), link.nextRetry(), link.nextRetry(), link.nextRetry(), link.nextRetry()], [5, 10, 30, 60, 60]);
  link.answered();
  assert.equal(link.state(0), 'online');
  assert.equal(link.lost(), true);
  assert.equal(link.nextRetry(), 5, 'the schedule starts again');
});

test('an expected restart is quiet for 120 seconds and then offline', () => {
  const link = makeConnection();
  link.expectRestart(1000);
  assert.equal(link.state(1000), 'restarting');
  assert.equal(link.state(1000 + RESTART_QUIET_MS - 1), 'restarting');
  assert.equal(link.state(1000 + RESTART_QUIET_MS), 'offline');
  link.answered();
  assert.equal(link.state(5000), 'online');
});

test('the event stream counts as not answering when it fails twice within 10 seconds', () => {
  const watch = makeStreamWatch();
  assert.equal(watch.failed(0), false);
  assert.equal(watch.failed(9000), true);
  assert.equal(watch.failed(30_000), false, 'too long after the last');
  watch.opened();
  assert.equal(watch.failed(31_000), false, 'an open stream starts the count again');
});
