// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The unlock's countdown in unlock.js: what the server last said is left,
// counted down on the browser's own clock, and who is told when it changes.
// The clock and the timer that checks it are replaced, so that a test
// moves through 15 minutes at once.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach, mock } from 'node:test';

import { isUnlocked, onUnlockChange, setUnlockSeconds, unlockMinutes } from '../unlock.js';

const realPerformance = Object.getOwnPropertyDescriptor(globalThis, 'performance');
let now;
let heard;
let forget;

// Moves the clock by `seconds`, one second at a time, as the timer would see it.
function pass(seconds) {
  for (let i = 0; i < seconds; i++) {
    now += 1000;
    mock.timers.tick(1000);
  }
}

beforeEach(() => {
  now = 5000;
  heard = 0;
  Object.defineProperty(globalThis, 'performance', { value: { now: () => now }, configurable: true });
  mock.timers.enable({ apis: ['setInterval'] });
  forget = onUnlockChange(() => { heard += 1; });
});

afterEach(() => {
  setUnlockSeconds(null);
  forget();
  mock.timers.reset();
  Object.defineProperty(globalThis, 'performance', realPerformance);
});

test('the console is locked until the server says how long it is unlocked', () => {
  assert.equal(isUnlocked(), false);
  setUnlockSeconds(900);
  assert.equal(isUnlocked(), true);
});

test('seconds that are null or zero lock it', () => {
  setUnlockSeconds(900);
  setUnlockSeconds(null);
  assert.equal(isUnlocked(), false);
  setUnlockSeconds(900);
  setUnlockSeconds(0);
  assert.equal(isUnlocked(), false);
});

test('whoever follows it is told when the server says and once for each minute that passes', () => {
  setUnlockSeconds(900);
  assert.equal(heard, 1);
  pass(59);
  assert.equal(heard, 1, 'still 15 minutes: a started minute counts');
  pass(1);
  assert.equal(heard, 2, 'at 840 seconds left it is 14');
  pass(60 * 13);
  assert.equal(heard, 15);
  assert.equal(isUnlocked(), true);
});

test('it locks itself when the time is up, and tells whoever follows it', () => {
  setUnlockSeconds(120);
  pass(119);
  assert.equal(isUnlocked(), true);
  const before = heard;
  pass(1);
  assert.equal(isUnlocked(), false);
  assert.equal(heard, before + 1, 'told once that it is locked');
  pass(30);
  assert.equal(heard, before + 1, 'the timer has stopped');
});

test('a longer unlock from the server replaces the time left', () => {
  setUnlockSeconds(60);
  pass(30);
  setUnlockSeconds(900);
  pass(60);
  assert.equal(isUnlocked(), true);
});

test('one that stopped following is not told', () => {
  forget();
  setUnlockSeconds(900);
  assert.equal(heard, 0);
});

test('the minutes left count a started minute, and are null while it is locked', () => {
  assert.equal(unlockMinutes(), null);
  setUnlockSeconds(900);
  assert.equal(unlockMinutes(), 15);
  pass(1);
  assert.equal(unlockMinutes(), 15);
  pass(59);
  assert.equal(unlockMinutes(), 14);
  pass(60 * 12 + 1);
  assert.equal(unlockMinutes(), 2);
  pass(60);
  assert.equal(unlockMinutes(), 1);
  pass(58);
  assert.equal(unlockMinutes(), 1);
  pass(1);
  assert.equal(unlockMinutes(), null);
});
