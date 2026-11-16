// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/restart-model.js: the words the banner holds for each kind of
// restart, how long each stays quiet, and the words of a restart or shut
// down that waits for its time.

import assert from 'node:assert/strict';
import test from 'node:test';

import { pendingWords, quietMsOf, restartWords } from '../lib/restart-model.js';
import { makeTime } from '../lib/time.js';
import { MACHINE_QUIET_MS, RESTART_QUIET_MS } from '../shell/online-model.js';

test('the words of each kind of restart', () => {
  assert.equal(restartWords({ kind: 'console' }), 'The console is restarting.');
  assert.equal(restartWords({ kind: 'machine', host: 'duke-nas' }), 'duke-nas is restarting. This page comes back by itself when it answers.');
  assert.equal(
    restartWords({ kind: 'update', host: 'duke-nas', version: '1.0.0-alpha.2' }),
    'duke-nas is restarting into 1.0.0-alpha.2. It has three tries to prove itself; the console answers again in a few minutes.',
  );
  assert.equal(
    restartWords({ kind: 'bisect', host: 'duke-nas', try: 2, of: 3 }),
    'duke-nas is restarting for try 2 of 3 of changes bisect. It restarts again by itself after the try.',
  );
});

test('going back to the version before is an update with the old version in its words', () => {
  assert.match(restartWords({ kind: 'update', host: 'duke-nas', version: '1.0.0-alpha.1' }), /^duke-nas is restarting into 1\.0\.0-alpha\.1\./);
});

test('what a page leaves out is not said as "undefined" or "null"', () => {
  assert.equal(restartWords({ kind: 'machine' }), 'The machine is restarting. This page comes back by itself when it answers.');
  assert.equal(restartWords({ kind: 'machine', host: null }), 'The machine is restarting. This page comes back by itself when it answers.');
  assert.match(restartWords({ kind: 'update', host: 'duke-nas' }), /^duke-nas is restarting into the new version\./);
  assert.match(restartWords({ kind: 'bisect', host: 'duke-nas' }), /^duke-nas is restarting for a try of changes bisect\./);
  assert.match(restartWords({ kind: 'bisect', host: 'duke-nas', try: 1 }), /for a try of changes bisect/);
  assert.equal(restartWords({ kind: 'sideways', host: 'duke-nas' }), restartWords({ kind: 'machine', host: 'duke-nas' }), 'a kind that is not known says what a machine\'s restart does');
});

test('the console is quiet for two minutes after its own restart, and the machine for ten after any other', () => {
  assert.equal(quietMsOf('console'), RESTART_QUIET_MS);
  assert.equal(quietMsOf('console'), 120_000);
  for (const kind of ['machine', 'update', 'bisect']) assert.equal(quietMsOf(kind), MACHINE_QUIET_MS, kind);
  assert.equal(MACHINE_QUIET_MS, 600_000);
});

test('a restart or shut down that waits says how many seconds are left when it is a minute or less, and its time when it is more', () => {
  const time = makeTime('UTC', () => Date.UTC(2026, 10, 16, 2, 0));
  const at = Date.UTC(2026, 10, 16, 3, 0);
  assert.equal(pendingWords({ end: 'reboot', in_seconds: 52, at_ms: at }, time), 'Restarts in 52 seconds');
  assert.equal(pendingWords({ end: 'reboot', in_seconds: 60, at_ms: at }, time), 'Restarts in 60 seconds');
  assert.equal(pendingWords({ end: 'poweroff', in_seconds: 1, at_ms: at }, time), 'Shuts down in 1 second');
  assert.equal(pendingWords({ end: 'reboot', in_seconds: 61, at_ms: at }, time), 'Restarts at 3:00 AM');
  assert.equal(pendingWords({ end: 'poweroff', in_seconds: 3600, at_ms: at }, time), 'Shuts down at 3:00 AM');
  assert.equal(pendingWords({ end: 'halt', in_seconds: 3600, at_ms: at }, time), 'Shuts down at 3:00 AM', 'a halt is a shut down');
});

test('a restart whose time has come says so, and a count that is not whole is rounded up', () => {
  const time = makeTime('UTC');
  assert.equal(pendingWords({ end: 'reboot', in_seconds: 0, at_ms: 0 }, time), 'Restarts now');
  assert.equal(pendingWords({ end: 'poweroff', in_seconds: -3, at_ms: 0 }, time), 'Shuts down now');
  assert.equal(pendingWords({ end: 'reboot', in_seconds: 51.2, at_ms: 0 }, time), 'Restarts in 52 seconds');
});

test('nothing waiting says nothing, and an answer with no times still says what is to happen', () => {
  const time = makeTime('UTC');
  assert.equal(pendingWords(null, time), null);
  assert.equal(pendingWords(undefined, time), null);
  assert.equal(pendingWords({ end: 'reboot' }, time), 'Restarts');
  assert.equal(pendingWords({ end: 'poweroff', in_seconds: null, at_ms: null }, time), 'Shuts down');
});
