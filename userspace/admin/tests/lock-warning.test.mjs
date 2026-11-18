// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/lock-warning.js: the notice that the unlock is about to end, with the
// button that asks the server for 15 minutes more.

import assert from 'node:assert/strict';
import test from 'node:test';

import { setUnlockSeconds, unlockMinutes } from '../unlock.js';
import { warnOfLock } from '../shell/lock-warning.js';

/** Makes the warning for `minutes` with a `post` of the test's; gives the notices that are made, as `[words, options]`. */
function warn(context, minutes, post) {
  context.mock.timers.enable({ apis: ['setInterval'] });
  context.after(() => setUnlockSeconds(null));
  const notices = [];
  warnOfLock({ minutes, post, notice: (words, options) => notices.push([words, options]) });
  return notices;
}

test('the warning says how many minutes are left', (context) => {
  assert.equal(warn(context, 2, async () => ({}))[0][0], 'The console locks in 2 minutes');
});

test('the warning says a minute in the singular', (context) => {
  assert.equal(warn(context, 1, async () => ({}))[0][0], 'The console locks in 1 minute');
});

test('Reset to 15 min asks the server for more and counts what it says', async (context) => {
  const posted = [];
  const [[, options]] = warn(context, 2, async (path) => {
    posted.push(path);
    return { unlock_seconds: 900 };
  });
  assert.equal(options.kind, 'warn');
  assert.equal(options.action.label, 'Reset to 15 min');
  await options.action.run();
  assert.deepEqual(posted, ['/api/1/unlock/extend']);
  assert.equal(unlockMinutes(), 15);
});

test('Reset to 15 min says why when the server refuses', async (context) => {
  const refused = Object.assign(new Error('refused'), { title: 'The console is locked' });
  const notices = warn(context, 2, async () => { throw refused; });
  await notices[0][1].action.run();
  assert.deepEqual(notices.slice(1).map(([words, options]) => [words, options.kind]), [['The console is locked', 'warn']]);
  assert.equal(unlockMinutes(), null);
});
