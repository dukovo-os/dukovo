// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Terminal page works out from the terminals admind lists: which
// panes the socket shows, how tabs and administrator panes are named, which
// tab is next, and why a pane ended in words.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  adminWords, ATTACH_MOST, attachPlan, closesAtOnce, endedWords, fitOf, LAYOUT_WORDS, LAYOUTS, nearsEnd, sizeStep, tabAfter, tabName, withKept,
} from '../pages/terminal/workspace-model.js';

const terminal = (id, panes, used = 0, extra = {}) => ({
  id, name: `t${id}`, used, panes: panes.map((pane) => ({ id: pane })), ...extra,
});

test('the socket shows the panes of the tab in view first, then the others\' by use, up to 16', () => {
  const terminals = [terminal(1, [10, 11], 100), terminal(2, [20], 300), terminal(3, [30, 31], 200)];
  assert.deepEqual(attachPlan(terminals, 3), [30, 31, 20, 10, 11]);
  assert.deepEqual(attachPlan(terminals, null), [20, 30, 31, 10, 11]);
  assert.deepEqual(attachPlan([], 1), []);
  const many = Array.from({ length: 8 }, (_, i) => terminal(i + 1, [i * 10 + 1, i * 10 + 2, i * 10 + 3], i));
  const plan = attachPlan(many, 1);
  assert.equal(plan.length, ATTACH_MOST);
  assert.deepEqual(plan.slice(0, 3), [1, 2, 3], 'the tab in view always has its panes');
  assert.deepEqual(attachPlan(many, 1, 4).length, 4);
});

test('a tab\'s name says its state', () => {
  assert.equal(tabName({ name: 'alice@duke-nas' }), 'alice@duke-nas');
  assert.equal(tabName({ name: 'alice@duke-nas', activity: true }), 'alice@duke-nas, new output');
  assert.equal(tabName({ name: 'root@duke-nas', admin: true, activity: true }), 'root@duke-nas, administrator, new output');
});

test('an administrator pane says when it ends, if the minutes are known', () => {
  assert.equal(adminWords(12), 'Administrator · ends when the console locks, in 12 min');
  assert.equal(adminWords(1), 'Administrator · ends when the console locks, in 1 min');
  assert.equal(adminWords(null), 'Administrator · ends when the console locks');
  assert.equal(adminWords(undefined), 'Administrator · ends when the console locks');
});

test('an administrator pane offers Reset to 15 min at 2 minutes left or fewer', () => {
  assert.deepEqual([null, undefined, 15, 3, 2, 1, 0].map(nearsEnd), [false, false, false, false, true, true, true]);
});

test('the next tab goes round the end', () => {
  const list = [{ id: 4 }, { id: 7 }, { id: 9 }];
  assert.equal(tabAfter(list, 4, 1), 7);
  assert.equal(tabAfter(list, 9, 1), 4);
  assert.equal(tabAfter(list, 4, -1), 9);
  assert.equal(tabAfter(list, 99, 1), 4, 'a tab that is not there starts from the first');
  assert.equal(tabAfter(list, 99, -1), 4);
  assert.equal(tabAfter([], 1, 1), null);
});

test('a pane whose program ended or that was ended goes at once; the others stay with their reason', () => {
  assert.deepEqual(['exited', 'ended', 'locked', 'signed-out', 'idle', 'restart'].map(closesAtOnce), [true, true, false, false, false, false]);
  assert.match(endedWords('exited'), /program in this pane ended\.$/);
  assert.match(endedWords('exited', 2), /ended with status 2\.$/);
  assert.match(endedWords('locked'), /locked, which ends administrator panes/);
  assert.match(endedWords('signed-out'), /signed out/);
  assert.match(endedWords('idle'), /kept/);
  assert.match(endedWords('restart'), /restarted/);
  assert.match(endedWords('ended'), /was ended/);
});

test('a terminal with a pane that stayed is shown with it, and one that admind forgot is shown for it', () => {
  const listed = [terminal(1, [10]), terminal(2, [20])];
  const kept = new Map([[1, terminal(1, [11])], [3, terminal(3, [30])]]);
  const shown = withKept(listed, kept);
  assert.deepEqual(shown.map((one) => [one.id, one.panes.map((pane) => pane.id)]), [[1, [10, 11]], [2, [20]], [3, [30]]]);
  assert.deepEqual(withKept(listed, new Map()).map((one) => one.id), [1, 2]);
  const again = new Map([[1, terminal(1, [10, 11])]]);
  assert.deepEqual(withKept(listed, again)[0].panes.map((pane) => pane.id), [10, 11], 'a pane admind lists is not shown twice');
});

test('a room gives a size within what a screen may be', () => {
  assert.deepEqual(fitOf(800, 480, { w: 8, h: 16 }), { cols: 100, rows: 30 });
  assert.deepEqual(fitOf(5, 5, { w: 8, h: 16 }), { cols: 2, rows: 1 });
  assert.deepEqual(fitOf(99999, 99999, { w: 8, h: 16 }), { cols: 500, rows: 300 });
});

test('the text size moves by 2 pixels within 12 to 20', () => {
  assert.equal(sizeStep(14, 1), 16);
  assert.equal(sizeStep(14, -1), 12);
  assert.equal(sizeStep(12, -1), null);
  assert.equal(sizeStep(20, 1), null);
  assert.equal(sizeStep(19, 1), 20);
  assert.equal(sizeStep(13, -1), 12);
});

test('the layouts are the five admind has, each in words', () => {
  assert.deepEqual(LAYOUTS, ['one', 'columns', 'rows', 'main-left', 'grid']);
  for (const layout of LAYOUTS) assert.ok(LAYOUT_WORDS[layout], layout);
});
