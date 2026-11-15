// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The words dom.js makes from what admind says, and how work that moves
// the page waits for a press to end. The functions that build elements are
// checked in a browser, by the lab's web-console tests.

import assert from 'node:assert/strict';
import test from 'node:test';

import { asSentence, CLICK_WAIT_MS, pressWatch, waitInWords } from '../dom.js';

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

/** A document's presses, with a timer the test runs by hand. */
function watched() {
  const target = new EventTarget();
  const timers = new Map();
  let next = 0;
  const later = (run, ms) => {
    next += 1;
    timers.set(next, { run, ms });
    return next;
  };
  const cancel = (id) => timers.delete(id);
  const { afterPress } = pressWatch(target, { later, cancel });
  const send = (type) => target.dispatchEvent(new Event(type));
  const runTimers = () => {
    for (const [id, timer] of [...timers]) {
      timers.delete(id);
      timer.run();
    }
  };
  return { afterPress, send, timers, runTimers };
}

test('work with no press under way runs at once', () => {
  const { afterPress } = watched();
  const done = [];
  afterPress(() => done.push('checked'));
  assert.deepEqual(done, ['checked']);
});

test('work asked for during a press runs after its click, and once', () => {
  const { afterPress, send, timers, runTimers } = watched();
  const done = [];
  send('pointerdown');
  afterPress(() => done.push('checked'));
  send('pointerup');
  assert.deepEqual(done, [], 'a press that is let go may still be clicked');
  send('click');
  assert.deepEqual([...timers.values()].map((timer) => timer.ms), [0], 'the click leaves one timer, for after its handlers');
  runTimers();
  assert.deepEqual(done, ['checked']);
  runTimers();
  assert.deepEqual(done, ['checked']);
});

test('a press that ends with no click lets the work run a moment later', () => {
  const { afterPress, send, timers, runTimers } = watched();
  const done = [];
  send('pointerdown');
  afterPress(() => done.push('checked'));
  send('pointerup');
  assert.deepEqual([...timers.values()].map((timer) => timer.ms), [CLICK_WAIT_MS]);
  runTimers();
  assert.deepEqual(done, ['checked']);
});

test('a cancelled press, as a finger that scrolls, lets the work run at once', () => {
  const { afterPress, send } = watched();
  const done = [];
  send('pointerdown');
  afterPress(() => done.push('checked'));
  send('pointercancel');
  assert.deepEqual(done, ['checked']);
});

test('a click from the keyboard, with no press, starts no wait', () => {
  const { afterPress, send, timers } = watched();
  send('click');
  assert.equal(timers.size, 0);
  const done = [];
  afterPress(() => done.push('checked'));
  assert.deepEqual(done, ['checked']);
});

test('a new press before the wait ends holds the work to the new press', () => {
  const { afterPress, send, timers, runTimers } = watched();
  const done = [];
  send('pointerdown');
  afterPress(() => done.push('checked'));
  send('pointerup');
  send('pointerdown');
  assert.equal(timers.size, 0, 'the new press takes the old wait away');
  assert.deepEqual(done, []);
  send('pointerup');
  send('click');
  runTimers();
  assert.deepEqual(done, ['checked']);
});
