// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/overview/refresh.js: a card that an event asks to read again reads at
// most once in the gap, and the last ask is not lost.

import assert from 'node:assert/strict';
import test from 'node:test';

import { atMostEvery } from '../pages/overview/refresh.js';

/** A clock and a timer that the test moves by hand. */
function fake() {
  const state = { at: 1000, timers: [], runs: [] };
  return {
    state,
    options: {
      now: () => state.at,
      wait: (ms, fn) => {
        const timer = { due: state.at + ms, fn, live: true };
        state.timers.push(timer);
        return () => { timer.live = false; };
      },
    },
    advance(ms) {
      state.at += ms;
      for (const timer of state.timers.filter((one) => one.live && one.due <= state.at)) {
        timer.live = false;
        timer.fn();
      }
    },
  };
}

test('the first call runs at once, and one within the gap waits for the end of it', () => {
  const clock = fake();
  const runs = [];
  const call = atMostEvery(() => runs.push(clock.state.at), 5000, clock.options);
  call();
  assert.deepEqual(runs, [1000]);
  clock.advance(1000);
  call();
  assert.deepEqual(runs, [1000], 'too soon');
  clock.advance(3999);
  assert.deepEqual(runs, [1000]);
  clock.advance(1);
  assert.deepEqual(runs, [1000, 6000], 'at the end of the gap');
});

test('many calls while one waits are one run', () => {
  const clock = fake();
  let runs = 0;
  const call = atMostEvery(() => { runs += 1; }, 5000, clock.options);
  call();
  for (let count = 0; count < 20; count += 1) {
    clock.advance(100);
    call();
  }
  assert.equal(runs, 1);
  clock.advance(5000);
  assert.equal(runs, 2);
});

test('a call after a quiet time runs at once again', () => {
  const clock = fake();
  let runs = 0;
  const call = atMostEvery(() => { runs += 1; }, 5000, clock.options);
  call();
  clock.advance(5000);
  call();
  assert.equal(runs, 2);
  clock.advance(60_000);
  call();
  assert.equal(runs, 3);
});

test('stopping cancels the call that is waiting', () => {
  const clock = fake();
  let runs = 0;
  const call = atMostEvery(() => { runs += 1; }, 5000, clock.options);
  call();
  call();
  call.stop();
  clock.advance(10_000);
  assert.equal(runs, 1);
  call();
  assert.equal(runs, 2, 'it can be used again after it is stopped');
  call.stop();
  call.stop();
});

test('with the real clock and timer, a second call waits', async () => {
  let runs = 0;
  const call = atMostEvery(() => { runs += 1; }, 30);
  call();
  call();
  assert.equal(runs, 1);
  await new Promise((resolve) => { setTimeout(resolve, 80); });
  assert.equal(runs, 2);
});

test('the Usage card reads each minute for the hour and each five for the rest, a second early at most', async () => {
  const { READ_EVERY, usageDue } = await import('../pages/overview/refresh.js');
  assert.deepEqual(READ_EVERY, { '1h': 60_000, '24h': 300_000, '7d': 300_000, '1y': 300_000 });
  assert.equal(usageDue(0, 59_000, '1h'), true);
  assert.equal(usageDue(0, 58_999, '1h'), false);
  assert.equal(usageDue(0, 299_000, '24h'), true);
  assert.equal(usageDue(0, 120_000, '24h'), false);
  assert.equal(usageDue(0, 299_000, '1y'), true);
  assert.equal(usageDue(0, 299_000, 'other'), true, 'an unknown range is read as the day is');
  assert.equal(usageDue(0, 100, '7d'), false);
});
