// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What /setup shows and says (design 12.2): the step shown, the step after
// it, what a step does to the score and the words around the wizard.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  chosenStep, introWords, moveOnWords, previewOf, runWords, startedWords, stepAfter,
} from '../pages/setup/model.js';

const step = (id, done = false, points = 0) => ({ id, title: `Step ${id}`, done, points });
const six = [step('a', true), step('b'), step('c', true), step('d'), step('e'), step('f')];

test('the step shown is the one the address names, else the first that is not done, else the first', () => {
  assert.equal(chosenStep(six, 'e').id, 'e');
  assert.equal(chosenStep(six, 'c').id, 'c', 'a step that is done can be looked at');
  assert.equal(chosenStep(six, 'nope').id, 'b');
  assert.equal(chosenStep(six, null).id, 'b');
  assert.equal(chosenStep([step('a', true), step('b', true)], null).id, 'a');
  assert.equal(chosenStep([], 'a'), null);
});

test('the step after one is the next that is not done, counting on and then round from the start', () => {
  assert.equal(stepAfter(six, 'b').id, 'd');
  assert.equal(stepAfter(six, 'd').id, 'e');
  assert.equal(stepAfter(six, 'f').id, 'b', 'from the end it goes round');
  assert.equal(stepAfter(six, 'a').id, 'b');
});

test('no step is after a step when every other one is done', () => {
  assert.equal(stepAfter([step('a'), step('b', true)], 'a'), null);
  assert.equal(stepAfter([step('a')], 'a'), null);
  assert.equal(stepAfter([], 'a'), null);
  assert.equal(stepAfter([step('a', true), step('b')], 'zzz').id, 'b', 'an id that is not there counts from the start');
});

test('a step with points shows what it does to the score, capped at 100', () => {
  assert.deepEqual(previewOf(step('x', false, 6), 86), {
    from: 86, to: 92, words: 'This step takes the score from 86 to 92.', label: 'Health score 86, 92 after this step',
  });
  assert.equal(previewOf(step('x', false, 6), 97).to, 100);
});

test('a step that does nothing to the score, or a score that is not known, shows no preview', () => {
  assert.equal(previewOf(step('x', true, 6), 86), null, 'done');
  assert.equal(previewOf(step('x', false, 0), 86), null, 'no points');
  assert.equal(previewOf(step('x', false, 6), null), null, 'no checks yet');
  assert.equal(previewOf(step('x', false, 6), 100), null, 'a full score');
  assert.equal(previewOf({ id: 'x', done: false }, 80), null, 'points left out');
});

test('the introduction counts the steps and says the score only when a step raises it', () => {
  assert.equal(introWords(six), 'Six steps. Each can be skipped and finished later from the Overview.');
  assert.equal(introWords([step('a', false, 4), step('b')]), 'Two steps. Each can be skipped and finished later from the Overview. Each one raises the health score.');
  assert.equal(introWords([step('a')]), 'One step. Each can be skipped and finished later from the Overview.');
});

test('the buttons say what they do for a step that is done', () => {
  assert.equal(runWords(step('a')), 'Set up');
  assert.equal(runWords(step('a', true)), 'Set up again');
  assert.equal(moveOnWords(step('a')), 'Skip for now');
  assert.equal(moveOnWords(step('a', true)), 'Next step');
  assert.equal(startedWords({ task: 42 }), 'Started. Its progress is in Tasks.');
  assert.equal(startedWords({ done: true }), 'Done.');
  assert.equal(startedWords(undefined), 'Done.');
});
