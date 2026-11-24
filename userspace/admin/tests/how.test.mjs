// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/how.js: the words for who asked for a change that an administrator
// approved, from the `how` of a row of the journal or of a task.

import assert from 'node:assert/strict';
import test from 'node:test';

import { askedBy } from '../lib/how.js';

test('who asked follows the name of the person who approved, with whose client it is when it is another\'s', () => {
  assert.equal(askedBy('client desk of chase', 'chase'), ', asked for by the AI client desk');
  assert.equal(askedBy('client night-watch-2 of alice', 'chase'), ', asked for by alice\'s AI client night-watch-2');
});

test('any other way a change was made names no client', () => {
  for (const how of ['console', 'token ci', 'token client desk of chase', 'by hand', 'undone', 'client Desk of chase', 'client desk of', 'client desk of a b', '', null, undefined]) {
    assert.equal(askedBy(how, 'chase'), '', String(how));
  }
});
