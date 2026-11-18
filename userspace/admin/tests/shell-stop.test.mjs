// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the shell registers outside its own elements ends with its `signal`:
// a person who signs out and in again gets a new shell in the same page, and
// the old one's header and trial chip must not go on following the unlock and
// the clock.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';
import { setUnlockSeconds } from '../unlock.js';
import { createHeader } from '../shell/header.js';
import { trialChip } from '../shell/trial-chip.js';

/** The services the header and the chip read, with a `signal` that the test ends. */
function shellOf(ending) {
  const handlers = new Map();
  return {
    host: 'nas',
    signal: ending.signal,
    session: () => ({ name: 'alice', full_name: 'Alice', admin: true }),
    prefs: { value: {} },
    tasks: { pill: () => ({ count: 0, warn: false, label: 'Tasks' }), onPill() {}, open() {} },
    bus: { on: (name, listener) => handlers.set(name, listener) },
    time: () => ({ clock: () => '2:32 PM' }),
    notice() {},
    get: async () => ({}),
    pageHelp: () => null,
  };
}

test('the header follows the unlock until the shell stops, and then no more', async (context) => {
  usePage(context);
  context.after(() => setUnlockSeconds(null));
  const ending = new AbortController();
  const header = createHeader(shellOf(ending));
  const access = header.element.querySelector('.access-icon');
  assert.equal(access.getAttribute('aria-label'), 'Limited access, unlock');
  setUnlockSeconds(900);
  assert.equal(access.getAttribute('aria-label'), 'Unlocked, 15 minutes left');
  ending.abort();
  setUnlockSeconds(null);
  assert.equal(access.getAttribute('aria-label'), 'Unlocked, 15 minutes left', 'a stopped shell draws nothing');
});

test('the trial chip counts down until the shell stops, and then no more', async (context) => {
  usePage(context);
  context.mock.timers.enable({ apis: ['setInterval', 'Date'], now: 1_000_000 });
  Object.defineProperty(globalThis, 'location', { value: { pathname: '/' }, configurable: true });
  context.after(() => delete globalThis.location);
  const ending = new AbortController();
  const slot = globalThis.document.createElement('span');
  const chip = trialChip(slot, shellOf(ending));
  chip.show([{ id: 1, left_ms: 120_000, page: '/security', what: 'The SSH settings' }]);
  const clock = () => slot.querySelector('.clock').textContent;
  assert.equal(clock(), '2:00');
  context.mock.timers.tick(1000);
  assert.equal(clock(), '1:59');
  ending.abort();
  context.mock.timers.tick(5000);
  assert.equal(clock(), '1:59', 'a stopped shell counts nothing');
});
