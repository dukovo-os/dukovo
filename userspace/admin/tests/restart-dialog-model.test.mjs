// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/restart-dialog-model.js: what the restart and shut down dialogs say,
// who they count as signed in, and what the countdown says.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  isClockTime, minuteWords, nowWords, PEOPLE_MAX, restartLead, restartTitle, secondsLeft, shutDownNotice, shutdownLead, shutdownTitle,
  signedInWords, taskWords, typedLabel, typedWhy,
} from '../lib/restart-dialog-model.js';

test('a restart is set for a time of day written HH:MM', () => {
  for (const time of ['00:00', '03:00', '09:05', '12:59', '23:59']) assert.equal(isClockTime(time), true, time);
  for (const time of ['24:00', '3:00', '03:60', '0300', '03:0', '', 'noon', '03:00:00', ' 03:00', null, undefined]) assert.equal(isClockTime(time), false, String(time));
});

test('the question names the machine and the lead says what a restart ends', () => {
  assert.equal(restartTitle('duke-nas'), 'Restart duke-nas?');
  const lead = restartLead('duke-nas');
  assert.match(lead, /^duke-nas restarts in one minute, and you can cancel until then\./);
  assert.match(lead, /open terminals end\.$/);
  assert.equal(restartLead('duke-nas', 'To finish the update to 1.0.0-alpha.2.'), `To finish the update to 1.0.0-alpha.2. ${lead}`);
});

test('a shut down says nothing brings the machine back but its power button', () => {
  assert.equal(shutdownTitle('duke-nas'), 'Shut down duke-nas?');
  assert.match(shutdownLead('duke-nas'), /does not come back until someone presses its power button/);
  assert.match(shutdownLead('duke-nas'), /use Restart instead/);
  assert.equal(typedLabel('duke-nas'), 'Type duke-nas to shut it down');
  assert.match(typedWhy('duke-nas'), /stays off until duke-nas is typed exactly/);
  assert.match(shutDownNotice('duke-nas'), /^duke-nas is shutting down\. It will not answer again until someone turns it on\.$/);
});

test('who is signed in is told per person, the asker as "you", with counts of what ends', () => {
  const people = [
    { name: 'sam', sessions: 2, terminals: 1 },
    { name: 'ada', sessions: 1, terminals: 0 },
    { name: 'kim', sessions: 0, terminals: 0 },
    { name: 'lee', sessions: 0, terminals: 3 },
  ];
  assert.deepEqual(signedInWords(people, 'sam'), [
    'You are signed in with 2 other browsers and 1 terminal.',
    'ada is signed in with 1 browser.',
    'lee is signed in with 3 terminals.',
  ]);
  assert.equal(signedInWords([{ name: 'kim', sessions: 0, terminals: 0 }], 'sam'), null);
  assert.equal(signedInWords([], 'sam'), null);
  assert.ok(PEOPLE_MAX >= 1);
});

test('a task that a restart ends is named with what it works on', () => {
  assert.equal(taskWords({ title: 'Back up to a place', args: { place: 'nas', retries: 3, skip: '', flag: true } }), 'Back up to a place for nas, 3 is running and stops.');
  assert.equal(taskWords({ action: 'snapshots.take', args: {} }), 'snapshots.take is running and stops.');
  assert.equal(taskWords({ title: 'Update the system' }), 'Update the system is running and stops.');
});

test('the seconds left count down from the moment they were read and stop at zero', () => {
  const shutdown = { end: 'reboot', in_seconds: 60, at_ms: 1_700_000_000_000 };
  assert.equal(secondsLeft(shutdown, 1000, 1000), 60);
  assert.equal(secondsLeft(shutdown, 1000, 31_000), 30);
  assert.equal(secondsLeft(shutdown, 1000, 90_000), 0);
  assert.equal(secondsLeft(null, 0, 0), null);
  assert.equal(secondsLeft({ end: 'reboot', at_ms: 5 }, 0, 10), null);
});

test('the last minute and its end are said once, in the words of what is set', () => {
  assert.equal(minuteWords({ end: 'reboot' }, 'duke-nas'), 'duke-nas restarts in 1 minute.');
  assert.equal(minuteWords({ end: 'poweroff' }, 'duke-nas'), 'duke-nas shuts down in 1 minute.');
  assert.equal(nowWords({ end: 'reboot' }), 'Restarting now.');
  assert.equal(nowWords({ end: 'poweroff' }), 'Shutting down now.');
});
