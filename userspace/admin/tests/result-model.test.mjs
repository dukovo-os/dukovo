// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// palette/result-model.js (design 11.4): what the result view shows of a read
// action's answer.

import assert from 'node:assert/strict';
import test from 'node:test';

import { cellOf, columnsOf, describeAnswer, listIn, MAX_COLUMNS, RESULT_ROWS, tableWords } from '../palette/result-model.js';

const SERVICES = {
  show_version: 1,
  now_ms: 5,
  services: [
    { name: 'nginx', description: 'Web server', enabled: true, order: 40, restart: 'always', state: 'running', pid: 301, requires: [], after: ['net'], holds: { names: true } },
    { name: 'backup-sync', description: 'Copies to the NAS', enabled: false, order: 90, restart: 'on-failure', state: 'failed', pid: null, requires: ['net', 'disks'] },
  ],
  trials: [],
  undone: [{ id: 'u1' }],
};

test('a cell shows a text, a number, a flag or a list of them, and nothing else', () => {
  assert.equal(cellOf('x'), 'x');
  assert.equal(cellOf(7), '7');
  assert.equal(cellOf(false), 'false');
  assert.equal(cellOf(['a', 'b']), 'a, b');
  assert.equal(cellOf([]), null);
  assert.equal(cellOf([{ a: 1 }]), null);
  assert.equal(cellOf({ a: 1 }), null);
  assert.equal(cellOf(null), null);
  assert.equal(cellOf(undefined), null);
});

test('the list in an answer is the answer, or its longest list of objects', () => {
  assert.deepEqual(listIn([{ a: 1 }]), { name: null, items: [{ a: 1 }] });
  assert.equal(listIn(SERVICES).name, 'services');
  assert.deepEqual(listIn({ a: [{ x: 1 }], b: [{ x: 1 }, { x: 2 }] }).name, 'b');
  assert.deepEqual(listIn({ a: [{ x: 1 }], b: [{ x: 2 }] }).name, 'a', 'the first of two as long');
  assert.equal(listIn([]), null);
  assert.equal(listIn([1, 2]), null);
  assert.equal(listIn([{ a: 1 }, 2]), null);
  assert.equal(listIn({ a: [1, 2], b: 'x' }), null);
  assert.equal(listIn('text'), null);
  assert.equal(listIn(null), null);
});

test('the columns hold what a cell can show, the well-known ones first, and at most six', () => {
  const columns = columnsOf(SERVICES.services).map((column) => column.key);
  assert.deepEqual(columns, ['name', 'state', 'description', 'enabled', 'order', 'restart']);
  assert.equal(columns.length, MAX_COLUMNS);
  assert.deepEqual(columnsOf(SERVICES.services).map((column) => column.label).slice(0, 2), ['Name', 'State']);
  assert.deepEqual(columnsOf([{ a: 1, b: { c: 1 } }, { d: 'x' }]).map((column) => column.key), ['a', 'd'], 'a column is each key that some row holds in a cell');
  assert.deepEqual(columnsOf([]), []);
});

test('a list is a table of its rows, in cells of words', () => {
  const table = describeAnswer(SERVICES);
  assert.equal(table.kind, 'table');
  assert.equal(table.name, 'services');
  assert.deepEqual([table.total, table.shown], [2, 2]);
  assert.deepEqual(table.columns.map((column) => column.key), ['name', 'state', 'description', 'enabled', 'order', 'restart']);
  assert.deepEqual(table.rows, [
    ['nginx', 'running', 'Web server', 'true', '40', 'always'],
    ['backup-sync', 'failed', 'Copies to the NAS', 'false', '90', 'on-failure'],
  ]);
  assert.equal(table.json, JSON.stringify(SERVICES, null, 2));
});

test('a table has the first fifty rows and says how many there are', () => {
  const many = Array.from({ length: 120 }, (_, n) => ({ name: `s${n}` }));
  const table = describeAnswer({ services: many });
  assert.equal(RESULT_ROWS, 50);
  assert.deepEqual([table.total, table.shown, table.rows.length], [120, 50, 50]);
  assert.equal(table.rows[49][0], 's49');
  assert.equal(tableWords(table), 'Services: the first 50 of 120');
  assert.equal(tableWords({ name: 'services', total: 3, shown: 3 }), 'Services: 3');
  assert.equal(tableWords({ name: null, total: 1, shown: 1 }), 'Rows: 1');
  assert.equal(describeAnswer(many).name, null);
});

test('an answer that is one thing is its facts, and what is not a cell is left to the JSON', () => {
  const answer = { release: '2026.11', reach: 'private', limits: { a: 1 }, tags: ['x', 'y'], gone: null };
  const facts = describeAnswer(answer);
  assert.equal(facts.kind, 'facts');
  assert.deepEqual(facts.facts, [['Release', '2026.11'], ['Reach', 'private'], ['Tags', 'x, y']]);
  assert.match(facts.json, /"limits": \{\n    "a": 1\n  \}/);
  assert.deepEqual(describeAnswer([]).facts, []);
  assert.deepEqual(describeAnswer(null).facts, []);
  assert.deepEqual(describeAnswer(7).facts, []);
});

test('an answer that is text is text, and an answer of nothing has empty JSON', () => {
  assert.deepEqual(describeAnswer('plain words'), { kind: 'text', text: 'plain words', json: 'plain words' });
  assert.equal(describeAnswer(undefined).json, '');
});
