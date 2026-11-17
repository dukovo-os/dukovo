// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Where showProblem draws: a fault of the page's own is shown in the page,
// and only a request that got no answer is left to the offline banner.

import assert from 'node:assert/strict';
import test from 'node:test';

import { ApiError } from '../api.js';
import { showProblem } from '../lib/problem.js';
import { usePage } from './fake-dom.mjs';

/** The fields of the shell's `cx` that the error view reads. */
const cx = { path: '/report', time: { full: () => 'now' }, hasPage: () => false };

test('a fault of the page\'s own is shown in its slot with its words', (context) => {
  const { document } = usePage(context);
  const slot = document.createElement('div');
  showProblem(new RangeError('Invalid time value'), { slot, first: true }, cx);
  assert.match(slot.textContent, /This page could not be shown/);
  assert.match(slot.textContent, /Invalid time value/);
});

test('a request that got no answer is left to the offline banner', (context) => {
  const { document } = usePage(context);
  const slot = document.createElement('div');
  showProblem(new ApiError({ title: 'The console cannot reach the server' }), { slot, first: true }, cx);
  assert.equal(slot.textContent, '');
});
