// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/online-model.js and the stream watch of shell/events.js: the waits,
// the words, the lengths of a restart's quiet time and the failures that
// count; and the call a page makes to ask for a restart.

import assert from 'node:assert/strict';
import test from 'node:test';

import { readFileSync } from 'node:fs';

import { makeTime } from '../lib/time.js';
import { makeStreamWatch } from '../shell/events.js';
import {
  asOfWords, BACK_MS, backWords, MACHINE_QUIET_MS, makeConnection, offlineWords, RESTART_QUIET_MS, restartingWords, retryDelay,
  runningVersion, tryingWords, waitingWords,
} from '../shell/online-model.js';
import { makeView } from '../shell/view.js';
import { ROOT } from './graph.mjs';

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

test('a restart without words or a length is the console\'s own: its words and 120 seconds', () => {
  const link = makeConnection();
  link.expectRestart(0);
  assert.equal(link.words(), restartingWords);
  assert.equal(link.words(), 'The console is restarting.');
  assert.equal(link.state(RESTART_QUIET_MS - 1), 'restarting');
  assert.equal(link.state(RESTART_QUIET_MS), 'offline');
  const empty = makeConnection();
  empty.expectRestart(0, {});
  assert.deepEqual([empty.words(), empty.state(RESTART_QUIET_MS)], [restartingWords, 'offline']);
});

test('a restart with words says them, and one with a length is quiet that long', () => {
  const link = makeConnection();
  link.expectRestart(5000, { words: 'duke-nas is restarting.', quietMs: MACHINE_QUIET_MS });
  assert.equal(MACHINE_QUIET_MS, 600_000);
  assert.equal(link.words(), 'duke-nas is restarting.');
  assert.equal(link.state(5000 + RESTART_QUIET_MS), 'restarting', 'past the console\'s 120 s the machine is still quiet');
  assert.equal(link.state(5000 + MACHINE_QUIET_MS - 1), 'restarting');
  assert.equal(link.state(5000 + MACHINE_QUIET_MS), 'offline');
});

test('a restart starts the looks again, an answer ends it, and the next restart says its own words and length', () => {
  const link = makeConnection();
  link.nextRetry();
  link.nextRetry();
  link.expectRestart(0, { words: 'first' });
  assert.equal(link.nextRetry(), 5, 'the looks start from 5 s');
  link.answered();
  assert.equal(link.state(1), 'online');
  link.expectRestart(10, { words: 'second', quietMs: 1000 });
  assert.deepEqual([link.words(), link.state(1009), link.state(1010)], ['second', 'restarting', 'offline']);
});

test('under the restart words the banner says since when the server has not answered', () => {
  const time = makeTime('UTC', () => Date.UTC(2026, 10, 14, 2, 10));
  assert.equal(waitingWords(time, Date.UTC(2026, 10, 14, 2, 4) / 1000), 'Waiting since 2:04 AM');
});

test('the running release is the last word of what console.about says, and none when it says nothing', () => {
  assert.equal(runningVersion({ release: 'Dukovo Server 1.0.0-alpha.2' }), '1.0.0-alpha.2');
  assert.equal(runningVersion({ release: ' 1.0.0 ' }), '1.0.0');
  for (const about of [null, undefined, {}, { release: null }, { release: '' }, { release: 7 }]) assert.equal(runningVersion(about), null, JSON.stringify(about));
});

test('after a restart the banner says Back, or that the update was taken back with a link to what happened', () => {
  const now = { release: 'Dukovo Server 1.0.0-alpha.2' };
  assert.deepEqual(backWords(now, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: false });
  assert.deepEqual(backWords(now, null), { words: 'Back', link: null, trying: false }, 'a restart that was no update has no version to compare');
  assert.deepEqual(backWords({ release: 'Dukovo Server 1.0.0-alpha.1', fallback: true }, '1.0.0-alpha.2'), {
    words: 'Back, on 1.0.0-alpha.1: the update was taken back.',
    link: { label: 'See what happened', href: '/updates#happened' },
    trying: false,
  });
  assert.deepEqual(backWords(null, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: false }, 'an answer that could not be read says no more than Back');
  assert.deepEqual(backWords({}, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: false });
  assert.equal(BACK_MS, 5000);
});

test('another release that is no fallback is the update on its try: Back, and not taken back yet', () => {
  const tried = { release: 'Dukovo Server 1.0.0-alpha.1' };
  assert.deepEqual(backWords(tried, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: true });
  assert.deepEqual(backWords({ ...tried, fallback: false }, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: true });
  assert.deepEqual(backWords({ ...tried, fallback: 'yes' }, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: true }, 'only true says a fallback');
  assert.deepEqual(backWords({ ...tried, fallback: true }, null), { words: 'Back', link: null, trying: false }, 'a fallback after a restart that was no update is not the update taken back');
  assert.deepEqual(backWords({ release: 'Dukovo Server 1.0.0-alpha.2', fallback: true }, '1.0.0-alpha.2'), { words: 'Back', link: null, trying: false });
});

test('the event stream names the power event', () => {
  assert.match(readFileSync(`${ROOT}shell/events.js`, 'utf8'), /const NAMES = \[[^\]]*'power'[^\]]*\];/);
});

test('a page asks for a restart through its view, and the shell\'s banner gets what it says', () => {
  const asked = [];
  const view = makeView({ online: { expectRestart: (...words) => asked.push(words) } }, { signal: new AbortController().signal });
  view.expectRestart('duke-nas is restarting.', { quietMs: MACHINE_QUIET_MS, version: '1.0.0-alpha.2' });
  view.expectRestart('The console is restarting.');
  assert.deepEqual(asked, [['duke-nas is restarting.', { quietMs: 600_000, version: '1.0.0-alpha.2' }], ['The console is restarting.', undefined]]);
});

test('the event stream counts as not answering when it fails twice within 10 seconds', () => {
  const watch = makeStreamWatch();
  assert.equal(watch.failed(0), false);
  assert.equal(watch.failed(9000), true);
  assert.equal(watch.failed(30_000), false, 'too long after the last');
  watch.opened();
  assert.equal(watch.failed(31_000), false, 'an open stream starts the count again');
});
