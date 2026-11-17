// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How a hold and an update put off say when and by whom, with no DOM: a time
// in the person's own clock, the accounts' names, and what a hold says of
// itself.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  atWords, byWords, holdStory, namesOf, untilWords,
} from '../lib/hold-words.js';
import { time } from './updates-fixtures.mjs';

const names = new Map([[1000, 'admin']]);
const AT = Date.UTC(2026, 10, 2, 3, 6);

test('a time is said as the person reads it: today, yesterday, or the day', () => {
  assert.equal(atWords(Date.UTC(2026, 10, 14, 12, 5), time), 'today at 12:05 PM');
  assert.equal(atWords(Date.UTC(2026, 10, 13, 22, 5), time), 'yesterday at 10:05 PM');
  assert.equal(atWords(AT, time), 'on Nov 2 at 3:06 AM');
  assert.equal(untilWords(Date.UTC(2026, 10, 23, 15, 12), time), 'Nov 23 at 3:12 PM');
  assert.equal(untilWords(Date.UTC(2026, 10, 14, 18, 0), time), 'today at 6:00 PM');
});

test('the accounts are named by their user ID, from a list or the object that holds it', () => {
  const accounts = [{ name: 'admin', uid: 1000 }, { name: 'reader', uid: 1001 }, { name: '', uid: 1002 }, { name: 'x' }, null];
  assert.deepEqual([...namesOf(accounts)], [[1000, 'admin'], [1001, 'reader']]);
  assert.deepEqual([...namesOf({ accounts })], [[1000, 'admin'], [1001, 'reader']]);
  assert.equal(namesOf(null).size, 0);
  assert.equal(namesOf('no').size, 0);
  assert.equal(byWords(1000, names), ' by admin');
  assert.equal(byWords(1001, names), ' by account 1001', 'a name that is not known is said by its number');
  assert.equal(byWords(1001, null), ' by account 1001');
  assert.equal(byWords(0, names), ' by root', "root's name is known though the accounts do not list it");
  assert.equal(byWords(null, names), '');
  assert.equal(byWords(undefined, names), '');
});

test('a hold says when, by whom and why, each of them that is known', () => {
  assert.equal(holdStory({ at: AT, by: 1000, why: 'Waiting for the nginx fix in alpha.3' }, time, names), 'held on Nov 2 at 3:06 AM by admin: "Waiting for the nginx fix in alpha.3"');
  assert.equal(holdStory({ at: AT, by: 1000, why: null }, time, names), 'held on Nov 2 at 3:06 AM by admin', 'a hold with no reason says when and who only');
  assert.equal(holdStory({ at: AT, by: 1000, why: '  ' }, time, names), 'held on Nov 2 at 3:06 AM by admin');
  assert.equal(holdStory({ by: 1000 }, time, names), 'held by admin');
  assert.equal(holdStory({ why: 'asked' }, time, names), 'held: "asked"');
  assert.equal(holdStory({}, time, names), null);
  assert.equal(holdStory({ at: null, by: null, why: null }, time, names), null);
  assert.equal(holdStory({ why: '<b>bold</b>' }, time, names), 'held: "<b>bold</b>"', 'a reason is text, kept as it was typed');
});
