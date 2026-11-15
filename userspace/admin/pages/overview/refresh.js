// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How often a card reads again (design 10, the table's last column). An
// event such as `change` or `task` may come many times in a second while one
// thing happens, and a card that read at each would ask the server as often.

/**
 * `run` as a function that may be called at will but runs at most once every
 * `gap` milliseconds: a call after a quiet time runs at once, and calls
 * while one is waiting are one call, run when the gap has passed. `stop()`
 * cancels a call that is waiting. `now()` and `wait(ms, fn)` (which gives the
 * function that cancels it) are the clock and the timer; tests replace them.
 */
export function atMostEvery(run, gap, {
  now = () => Date.now(),
  wait = (ms, fn) => {
    const timer = setTimeout(fn, ms);
    return () => clearTimeout(timer);
  },
} = {}) {
  let last = -Infinity;
  let cancel = null;
  const fire = () => {
    cancel = null;
    last = now();
    run();
  };
  const call = () => {
    if (cancel) return;
    const left = last + gap - now();
    if (left <= 0) fire();
    else cancel = wait(left, fire);
  };
  call.stop = () => {
    cancel?.();
    cancel = null;
  };
  return call;
}

/** How often the Usage card reads, by range, in milliseconds: a minute for the hour, which has a point each minute, and five for the rest. */
export const READ_EVERY = Object.freeze({ '1h': 60_000, '24h': 300_000, '7d': 300_000, '1y': 300_000 });

/** How much earlier than its time a read may come, so that a timer that ticks each minute is not a minute late. */
const SLACK_MS = 1000;

/** Whether the Usage card, last read at `last` (milliseconds), is to be read again at `now` for `range`. */
export const usageDue = (last, now, range) => now - last >= (READ_EVERY[range] ?? READ_EVERY['24h']) - SLACK_MS;
