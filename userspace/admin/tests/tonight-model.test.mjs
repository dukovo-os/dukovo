// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The words of tonight's window, with no DOM: the hour that ends at the
// restart time, 2:00 to 3:00 without one, across midnight, and what a request
// that waits for it says.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  betweenWords, tonightOf, windowOf, windowWords,
} from '../lib/tonight-model.js';

test('the window is the hour before the restart time, and 2:00 to 3:00 without one', () => {
  assert.deepEqual(windowOf('03:00'), [120, 180]);
  assert.deepEqual(windowOf(null), [120, 180]);
  assert.deepEqual(windowOf(undefined), [120, 180]);
  assert.deepEqual(windowOf('04:30'), [210, 270]);
  assert.deepEqual(windowOf('00:30'), [1410, 30], 'it crosses midnight');
  assert.deepEqual(windowOf('00:00'), [1380, 0]);
  assert.deepEqual(windowOf('24:00'), [120, 180], 'a time that is not a time of day is none');
  assert.deepEqual(windowOf('3:00'), [120, 180]);
  assert.deepEqual(windowOf('soon'), [120, 180]);
});

test('the window is said as the person reads a clock', () => {
  assert.equal(betweenWords(null), '2:00 and 3:00 AM');
  assert.equal(betweenWords('04:30'), '3:30 and 4:30 AM');
  assert.equal(betweenWords('00:30'), '11:30 PM and 12:30 AM');
  assert.equal(betweenWords('13:00'), '12:00 and 1:00 PM');
  assert.equal(betweenWords('12:00'), '11:00 AM and 12:00 PM');
});

test('the words of the window say why that hour when the server has a restart time', () => {
  assert.equal(windowWords('03:00'), 'Installs tonight, between 2:00 and 3:00 AM: the hour before this server\'s restart time.');
  assert.equal(windowWords(null), 'Installs tonight, between 2:00 and 3:00 AM.');
  assert.equal(windowWords('00:30'), 'Installs tonight, between 11:30 PM and 12:30 AM: the hour before this server\'s restart time.');
  assert.equal(windowWords('later'), 'Installs tonight, between 2:00 and 3:00 AM.');
});

test('a request that waits says it installs tonight, or that the window passed while the machine was off', () => {
  const asked = { to: '8.0.4', asked_ms: 1, by: 1000, missed_ms: null, starts_ms: 2 };
  assert.deepEqual(tonightOf(asked), { words: 'Installs tonight', missed: false });
  assert.deepEqual(tonightOf({ ...asked, missed_ms: 5 }), { words: 'The window passed while the machine was off: it installs tonight.', missed: true });
  assert.equal(tonightOf(null), null);
  assert.equal(tonightOf(undefined), null);
  assert.equal(tonightOf('no'), null);
});
