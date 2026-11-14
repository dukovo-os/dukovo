// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The words dom.js makes from what admind says. The functions that build
// elements are checked in a browser, by the lab's web-console tests.

import assert from 'node:assert/strict';
import test from 'node:test';

import { asSentence, waitInWords } from '../dom.js';

test('a title that names what happened gets a full stop, and a sentence keeps its own mark', () => {
  assert.equal(asSentence('That code is not right'), 'That code is not right.');
  assert.equal(asSentence('Too many tries: wait a moment'), 'Too many tries: wait a moment.');
  assert.equal(asSentence('Is that the right name?'), 'Is that the right name?');
  assert.equal(asSentence('Sign in again:'), 'Sign in again:');
});

test('a wait of under a minute is in seconds, and a longer one in whole minutes, rounded up', () => {
  assert.equal(waitInWords(1), '1 second');
  assert.equal(waitInWords(45), '45 seconds');
  assert.equal(waitInWords(60), '1 minute');
  assert.equal(waitInWords(61), '2 minutes');
  assert.equal(waitInWords(90), '2 minutes');
  assert.equal(waitInWords(900), '15 minutes');
});
