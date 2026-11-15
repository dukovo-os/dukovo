// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The panes' layouts as plain values: where each pane goes, the splits and
// their sizes, and the moves the keys make from pane to pane.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  LAYOUTS, layoutForSplit, movedSizes, neighbour, placeOf, sizeAt, sizesFor, splitsOf,
} from '../pages/terminal/layout-model.js';
import { flush, setLayout } from '../pages/terminal/splits.js';

const near = (actual, expected) => assert.ok(Math.abs(actual - expected) < 1e-9, `${actual} is not ${expected}`);
const box = (place) => [place.x, place.y, place.w, place.h].map((n) => Math.round(n * 1000) / 1000);

test('a layout has as many splits as it needs for its panes', () => {
  assert.deepEqual(['one', 'columns', 'rows', 'main-left', 'grid'], LAYOUTS);
  assert.deepEqual([1, 2, 3, 4].map((n) => splitsOf('columns', n)), [0, 1, 2, 3]);
  assert.deepEqual([1, 2, 3, 4].map((n) => splitsOf('rows', n)), [0, 1, 2, 3]);
  assert.deepEqual([1, 2, 3, 4].map((n) => splitsOf('main-left', n)), [0, 1, 2, 3]);
  assert.deepEqual([1, 2, 3, 4].map((n) => splitsOf('grid', n)), [0, 1, 2, 2]);
  assert.deepEqual([1, 2, 4].map((n) => splitsOf('one', n)), [0, 0, 0]);
});

test('missing sizes are equal shares, and every size is kept within 0.2 to 0.8', () => {
  near(sizesFor('columns', 2)[0], 0.5);
  assert.deepEqual(sizesFor('columns', 4).map((n) => Math.round(n * 1000) / 1000), [0.25, 0.333, 0.5]);
  assert.deepEqual(sizesFor('columns', 3, [0.1, 0.95]), [0.2, 0.8]);
  assert.deepEqual(sizesFor('grid', 4), [0.5, 0.5]);
  assert.deepEqual(sizesFor('columns', 3, [0.3, 'x']), [0.3, 0.5]);
  assert.deepEqual(sizesFor('columns', 3, [0.3, NaN]), [0.3, 0.5]);
  assert.deepEqual(sizesFor('columns', 3, 'bad'), [1 / 3, 0.5]);
  assert.deepEqual(sizesFor('columns', 3, [0.3, 0.4, 0.5, 0.6]), [0.3, 0.4], 'sizes past the splits go');
  assert.deepEqual(sizesFor('columns', 1, [0.3]), []);
});

test('one pane fills the box, with no separator', () => {
  for (const layout of LAYOUTS) {
    const { panes, separators } = placeOf(layout, 1);
    assert.deepEqual(panes.map(box), [[0, 0, 1, 1]], layout);
    assert.deepEqual(separators, [], layout);
  }
});

test('layout one shows the focused pane alone', () => {
  const { panes, separators } = placeOf('one', 3, [], 1);
  assert.deepEqual(panes.map((place) => place && box(place)), [null, [0, 0, 1, 1], null]);
  assert.deepEqual(separators, []);
});

test('columns put the panes side by side, each taking its share of what is left', () => {
  const { panes, separators } = placeOf('columns', 3, [0.5, 0.5]);
  assert.deepEqual(panes.map(box), [[0, 0, 0.5, 1], [0.5, 0, 0.25, 1], [0.75, 0, 0.25, 1]]);
  assert.deepEqual(separators.map((one) => [one.index, one.axis, one.at, one.from, one.to, one.across]), [
    [0, 'x', 0.5, 0, 1, [0, 1]],
    [1, 'x', 0.75, 0.5, 1, [0, 1]],
  ]);
});

test('rows put them one above the other', () => {
  const { panes, separators } = placeOf('rows', 2, [0.3]);
  assert.deepEqual(panes.map(box), [[0, 0, 1, 0.3], [0, 0.3, 1, 0.7]]);
  assert.deepEqual(separators.map((one) => [one.axis, one.at, one.across]), [['y', 0.3, [0, 1]]]);
});

test('main-left gives pane 0 the left and stacks the rest at its right', () => {
  const { panes, separators } = placeOf('main-left', 3, [0.6, 0.5]);
  assert.deepEqual(panes.map(box), [[0, 0, 0.6, 1], [0.6, 0, 0.4, 0.5], [0.6, 0.5, 0.4, 0.5]]);
  assert.deepEqual(separators.map((one) => [one.index, one.axis, one.at, one.from, one.to]), [
    [0, 'x', 0.6, 0, 1],
    [1, 'y', 0.5, 0, 1],
  ]);
  assert.deepEqual(placeOf('main-left', 2, [0.4]).panes.map(box), [[0, 0, 0.4, 1], [0.4, 0, 0.6, 1]]);
});

test('a grid of two is two columns, of three has the third across the bottom, of four is four', () => {
  assert.deepEqual(placeOf('grid', 2, [0.4]).panes.map(box), [[0, 0, 0.4, 1], [0.4, 0, 0.6, 1]]);
  const three = placeOf('grid', 3, [0.5, 0.4]);
  assert.deepEqual(three.panes.map(box), [[0, 0, 0.5, 0.4], [0.5, 0, 0.5, 0.4], [0, 0.4, 1, 0.6]]);
  assert.deepEqual(three.separators.map((one) => [one.index, one.axis, one.at, one.across]), [
    [1, 'y', 0.4, [0, 1]],
    [0, 'x', 0.5, [0, 0.4]],
  ]);
  const four = placeOf('grid', 4, [0.5, 0.5]);
  assert.deepEqual(four.panes.map(box), [[0, 0, 0.5, 0.5], [0.5, 0, 0.5, 0.5], [0, 0.5, 0.5, 0.5], [0.5, 0.5, 0.5, 0.5]]);
  assert.equal(four.separators.length, 2, 'one line each way');
  assert.deepEqual(four.separators.map((one) => one.index), [1, 0]);
});

test('sizes move by a step or are set, and stay within 0.2 to 0.8', () => {
  assert.deepEqual(movedSizes('columns', 2, [0.5], 0, 0.05, true), [0.55]);
  assert.deepEqual(movedSizes('columns', 2, [0.5], 0, -0.05, true), [0.45]);
  assert.deepEqual(movedSizes('columns', 2, [0.78], 0, 0.05, true), [0.8]);
  assert.deepEqual(movedSizes('columns', 2, [0.22], 0, -0.05, true), [0.2]);
  assert.deepEqual(movedSizes('columns', 3, [0.5, 0.5], 1, 0.7), [0.5, 0.7]);
  assert.deepEqual(movedSizes('columns', 3, [0.5, 0.5], 1, 0.99), [0.5, 0.8]);
  assert.deepEqual(movedSizes('columns', 2, [0.5], 5, 0.7), [0.5], 'a split that is not there changes nothing');
  assert.deepEqual(movedSizes('columns', 2, [0.5], -1, 0.7), [0.5]);
  assert.deepEqual(movedSizes('columns', 2, [], 0, 0.3), [0.3], 'sizes admind has not got yet are made');
});

test('a pointer at a position asks for the share of the separator\'s range', () => {
  const { separators } = placeOf('columns', 3, [0.5, 0.5]);
  near(sizeAt(separators[0], 0.25), 0.25);
  near(sizeAt(separators[1], 0.75), 0.5);
  near(sizeAt(separators[1], 0.9), 0.8);
});

test('the pane next to another is the one that touches its edge and shares most of it', () => {
  const columns = placeOf('columns', 3, [0.5, 0.5]).panes;
  assert.deepEqual(['left', 'right'].map((to) => neighbour(columns, 1, to)), [0, 2]);
  assert.equal(neighbour(columns, 0, 'left'), null);
  assert.equal(neighbour(columns, 2, 'right'), null);
  assert.equal(neighbour(columns, 1, 'up'), null, 'columns have nothing above');
  const rows = placeOf('rows', 3).panes;
  assert.deepEqual(['up', 'down'].map((to) => neighbour(rows, 1, to)), [0, 2]);
  assert.equal(neighbour(rows, 0, 'up'), null);
  const main = placeOf('main-left', 3, [0.5, 0.2]).panes;
  assert.equal(neighbour(main, 0, 'right'), 2, 'the right pane that shares more of the left one\'s edge');
  assert.equal(neighbour(main, 1, 'down'), 2);
  assert.equal(neighbour(main, 2, 'left'), 0);
  assert.equal(neighbour(main, 1, 'up'), null);
  const grid = placeOf('grid', 4).panes;
  assert.deepEqual(['left', 'right', 'up', 'down'].map((to) => neighbour(grid, 0, to)), [null, 1, null, 2]);
  assert.deepEqual(['left', 'right', 'up', 'down'].map((to) => neighbour(grid, 3, to)), [2, null, 1, null]);
  const three = placeOf('grid', 3).panes;
  assert.equal(neighbour(three, 2, 'up'), 0, 'the one across the bottom goes up to the left one, which shares as much as the right');
  assert.equal(neighbour(three, 1, 'down'), 2);
});

test('a pane that is not shown has no neighbours, and is not one', () => {
  const alone = placeOf('one', 3, [], 1).panes;
  assert.equal(neighbour(alone, 1, 'left'), null);
  assert.equal(neighbour(alone, 0, 'left'), null);
  assert.equal(neighbour(alone, 7, 'left'), null);
  assert.equal(neighbour(placeOf('columns', 2).panes, 0, 'sideways'), null);
});

test('splitting keeps the line the panes lie in, and a split across makes a grid', () => {
  assert.equal(layoutForSplit('one', 1, 'right'), 'columns');
  assert.equal(layoutForSplit('one', 1, 'down'), 'rows');
  assert.equal(layoutForSplit('columns', 1, 'down'), 'rows');
  assert.equal(layoutForSplit('columns', 2, 'right'), 'columns');
  assert.equal(layoutForSplit('columns', 2, 'down'), 'grid');
  assert.equal(layoutForSplit('rows', 2, 'down'), 'rows');
  assert.equal(layoutForSplit('rows', 3, 'right'), 'grid');
  assert.equal(layoutForSplit('main-left', 2, 'right'), 'main-left');
  assert.equal(layoutForSplit('grid', 3, 'down'), 'grid');
  assert.equal(layoutForSplit('one', 3, 'down'), 'rows', 'a layout of one pane at a time becomes one with all shown');
});

/** Has `fetch` answer each call from `answers` in turn, noting each call in the list it gives. */
function answering(t, ...answers) {
  const calls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (path, options) => {
    calls.push([path, JSON.parse(options.body)]);
    return Promise.resolve(answers.shift()());
  };
  t.after(() => {
    globalThis.fetch = realFetch;
  });
  return calls;
}

const JSON_TYPE = { 'content-type': 'application/json' };
const done = () => new Response('{}', { status: 200, headers: JSON_TYPE });

/** A workspace with what setLayout uses: it notes each drawing, each read of the list and each problem. */
function workspace(saving = null) {
  const ws = {
    saving, edits: 0, changing: 0, drawn: 0, reads: 0, problems: [],
    terminal: { id: 3, layout: 'columns', sizes: [0.55] },
    draw() { ws.drawn += 1; },
    later() { ws.reads += 1; },
  };
  ws.view = { problem: (error) => ws.problems.push(error) };
  return ws;
}

test('a split first saves what a separator moved by the keys waits to save, and nothing when nothing waits', async (t) => {
  const calls = answering(t, done);
  const ws = workspace(7);
  await flush(ws);
  assert.deepEqual(calls, [['/api/1/terminals/3/layout', { layout: 'columns', sizes: [0.55] }]]);
  assert.equal(ws.saving, null);
  await flush(ws);
  assert.equal(calls.length, 1);
});

test('a layout is shown at once, and the list is read again only once admind has answered', async (t) => {
  let answer;
  const refused = () => new Response(JSON.stringify({ error: { id: 'ADM-131', title: 'No such terminal', detail: 'x' } }), { status: 404, headers: JSON_TYPE });
  const calls = answering(t, () => new Promise((resolve) => { answer = resolve; }), refused);
  const ws = workspace();
  const saving = setLayout(ws, 'rows');
  // Drawn before admind answers, and marked for a read of the list begun before.
  assert.deepEqual([ws.terminal.layout, ws.terminal.sizes, ws.drawn, ws.edits, ws.changing, ws.reads], ['rows', [], 1, 1, 1, 0]);
  while (answer === undefined) await new Promise(setImmediate);
  answer(done());
  await saving;
  assert.deepEqual([ws.changing, ws.reads, ws.problems.length], [0, 1, 0]);
  // A refusal is said, and the list is read again for what admind keeps.
  await setLayout(ws, 'grid', [0.5, 0.5]);
  assert.deepEqual([ws.edits, ws.changing, ws.reads, ws.problems.length], [2, 0, 2, 1]);
  assert.deepEqual(calls.map(([path]) => path), ['/api/1/terminals/3/layout', '/api/1/terminals/3/layout']);
  // With no terminal shown there is nothing to change.
  ws.terminal = null;
  await setLayout(ws, 'rows');
  assert.equal(calls.length, 2);
});
