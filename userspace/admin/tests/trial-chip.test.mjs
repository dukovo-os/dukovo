// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The header's chip of a trial that runs (C5 design 3.4) as far as a script
// test sees it: the stream names the `trial` event and the header gives the
// chip a place, and the chip's rules hold nothing that moves, so that less
// motion leaves nothing to take away but the digits, which lib/trial-model.js's
// tests hold to be the only thing that changes with time. What the chip looks
// like is the lab's `shell` test.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { ROOT } from './graph.mjs';

const read = (name) => readFileSync(`${ROOT}${name}`, 'utf8');

test('the event stream names the trial event, and the frame gives it to the header', () => {
  assert.match(read('shell/events.js'), /const NAMES = \[[^\]]*'trial'[^\]]*\];/);
  assert.match(read('shell/frame.js'), /if \(name === 'trial'\) header\.trials\(data\.trials\);/);
});

test('the header has an empty hidden slot for the chip before Tasks, and reads the chip only when a trial first runs', () => {
  const header = read('shell/header.js');
  assert.match(header, /h\('span', \{ class: 'trial-slot', hidden: true \}\)/);
  assert.ok(header.indexOf('trialSlot,\n    tasksButton') > 0, 'the slot is before the Tasks button');
  assert.match(header, /if \(!chip && list\.length === 0\) return;/);
  assert.match(header, /chip \?\?= import\('\.\/trial-chip\.js'\)/);
  assert.doesNotMatch(header, /^import .*trial-chip/m, 'the chip is not in the start');
});

test('the chip\'s rules in console.css have no animation, transition or transform', () => {
  const css = read('console.css').replace(/\/\*[\s\S]*?\*\//g, '');
  const rules = css.split('\n').filter((line) => line.includes('trial-chip'));
  assert.ok(rules.length >= 3, 'the chip has its colours, its phone width words and its size');
  for (const rule of rules) assert.doesNotMatch(rule, /animation|transition|transform/, rule.trim());
});

test('on a phone the chip keeps its icon and its minutes and drops its words', () => {
  const css = read('console.css');
  assert.match(css, /\.trial-chip \.words \{ display: none; \}/);
  assert.doesNotMatch(css, /\.trial-chip \.clock/);
});
