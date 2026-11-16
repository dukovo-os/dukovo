// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The reason a hold keeps, with no DOM: what a person may give and what is
// wrong with one that does not fit, and the note that says who reads it.

import assert from 'node:assert/strict';
import test from 'node:test';

import { WHY_MOST, whyNote, whyOf } from '../lib/hold-model.js';

test('a reason is the text without the spaces around it, or none, or what is wrong with it', () => {
  assert.deepEqual(whyOf('  Waiting for the nginx fix in alpha.3 '), { why: 'Waiting for the nginx fix in alpha.3', problem: null });
  assert.deepEqual(whyOf(''), { why: null, problem: null });
  assert.deepEqual(whyOf('   '), { why: null, problem: null });
  assert.deepEqual(whyOf(null), { why: null, problem: null });
  assert.equal(whyOf('two\nlines').why, null);
  assert.match(whyOf('two\nlines').problem, /one line/);
  assert.match(whyOf('a\tb').problem, /one line/);
  assert.match(whyOf('a\u007fb').problem, /one line/);
});

test('a reason has at most 200 characters, counted as people count them', () => {
  assert.equal(WHY_MOST, 200);
  assert.equal(whyOf('x'.repeat(200)).why.length, 200);
  assert.match(whyOf('x'.repeat(201)).problem, /at most 200 characters/);
  assert.equal(whyOf('é'.repeat(200)).problem, null);
  assert.equal(whyOf('\u{1F600}'.repeat(200)).problem, null, 'an emoji is a character, not two');
  assert.match(whyOf('\u{1F600}'.repeat(201)).problem, /at most 200/);
});

test('the note says who reads the reason, and where', () => {
  assert.equal(whyNote('update settings'), 'Everyone who can sign in sees the reason, here and with `update settings`.');
});
