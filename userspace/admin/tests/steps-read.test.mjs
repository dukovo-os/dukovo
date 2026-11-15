// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps that /setup and Finish setting up read from the pages' parts
// (design 3.5, 12.2): which are on this machine, in what order, and how far
// along the person is.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  nextStep, progressOf, readSteps, stepsWords,
} from '../pages/overview/steps-read.js';
import { fakeCx } from './overview-fixtures.mjs';

/** A `view` whose parts are `modules`, each called with a cx of its own. */
function fakeView(modules) {
  return { parts: async (name, call) => Promise.all(modules.map(async (part) => ({ value: await call(part, fakeCx({})) }))) };
}

const step = (id, more = {}) => ({ id, title: `Step ${id}`, done: () => false, ...more });

test('the steps of every part come in the order they say, and by ID when they say the same', async () => {
  const view = fakeView([
    { steps: [step('c', { order: 30 }), step('b', { order: 10 })] },
    { steps: [step('a', { order: 10 }), step('z')] },
    { needs: () => [] },
  ]);
  const read = await readSteps(view);
  assert.deepEqual(read.map((one) => one.id), ['a', 'b', 'c', 'z']);
  assert.equal(read.at(-1).order, 1000, 'a step with no order goes last');
});

test('a step that says it is not on this machine is left out, and one that says nothing is on it', async () => {
  const view = fakeView([{ steps: [
    step('off', { available: async () => false }),
    step('on', { available: () => true }),
    step('plain'),
  ] }]);
  assert.deepEqual((await readSteps(view)).map((one) => one.id), ['on', 'plain']);
});

test('a step is read as plain values: done, its summary, its points and its action', async () => {
  const form = () => ({});
  const view = fakeView([{ steps: [step('s', { done: async () => 1, summary: async () => 'It is set', points: 4, action: 'a.b', form })] }]);
  const [read] = await readSteps(view);
  assert.deepEqual(read, { id: 's', title: 'Step s', order: 1000, points: 4, action: 'a.b', done: true, summary: 'It is set', form });
});

test('a step with no summary, points, action or form reads as empty ones', async () => {
  const [read] = await readSteps(fakeView([{ steps: [step('s')] }]));
  assert.deepEqual([read.summary, read.points, read.action, read.form], ['', 0, null, null]);
});

test('no part with steps gives no steps', async () => {
  assert.deepEqual(await readSteps(fakeView([{ needs: () => [] }, {}])), []);
  assert.deepEqual(await readSteps(fakeView([])), []);
});

test('the progress counts the steps done and the next step is the first that is not', () => {
  const steps = [{ id: 'a', done: true }, { id: 'b', done: false }, { id: 'c', done: true }, { id: 'd', done: false }];
  assert.deepEqual(progressOf(steps), { done: 2, total: 4, words: '2 of 4 done', percent: 50 });
  assert.equal(nextStep(steps).id, 'b');
  assert.deepEqual(progressOf([]), { done: 0, total: 0, words: '0 of 0 done', percent: 0 });
  assert.equal(nextStep([{ id: 'a', done: true }]), null);
  assert.equal(progressOf([{ done: true }, { done: false }, { done: false }]).percent, 33);
});

test('steps are counted in words up to ten, and in digits after', () => {
  assert.equal(stepsWords(1), 'One step');
  assert.equal(stepsWords(5), 'Five steps');
  assert.equal(stepsWords(12), '12 steps');
});
