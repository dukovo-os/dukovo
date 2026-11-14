// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// help/model.js: the error pages' list, filtered and grouped by area.

import assert from 'node:assert/strict';
import test from 'node:test';

import { areaOf, byArea, countWords, isErrorId, matching } from '../help/model.js';

const index = [
  { id: 'UPD-103', title: 'The update did not fit on the disk', area: 'Updates of the system' },
  { id: 'ADM-123', title: 'The command did not work', area: 'The web console, Dukovo Admin' },
  { id: 'ADM-106', title: 'The console is locked', area: 'The web console, Dukovo Admin' },
  { id: 'FW-101', title: 'That port cannot work' },
];

test('a filter keeps the pages with every word in their ID, title or area, in the order of their IDs', () => {
  assert.deepEqual(matching(index, '').map((page) => page.id), ['ADM-106', 'ADM-123', 'FW-101', 'UPD-103']);
  assert.deepEqual(matching(index, 'web console').map((page) => page.id), ['ADM-106', 'ADM-123']);
  assert.deepEqual(matching(index, 'did not').map((page) => page.id), ['ADM-123', 'UPD-103']);
  assert.deepEqual(matching(index, 'adm 123').map((page) => page.id), ['ADM-123']);
  assert.deepEqual(matching(index, 'zzz'), []);
});

test('pages are grouped by the area text, the groups in the order of their first page, and a page with no area goes under its ID\'s letters', () => {
  assert.equal(areaOf(index[3]), 'FW');
  assert.deepEqual(byArea(matching(index, '')).map((group) => [group.area, group.pages.length]), [
    ['The web console, Dukovo Admin', 2], ['FW', 1], ['Updates of the system', 1],
  ]);
});

test('an error ID is letters, a hyphen and three digits', () => {
  assert.equal(isErrorId('ADM-123'), true);
  assert.equal(isErrorId('BOOT-101'), true);
  for (const bad of ['adm-123', 'ADM-12', 'ADM123', '../ADM-123', 'ADM-123.md', '']) assert.equal(isErrorId(bad), false, bad);
});

test('the count is in words', () => {
  assert.equal(countWords(0), 'No page matches.');
  assert.equal(countWords(1), '1 page');
  assert.equal(countWords(72), '72 pages');
});
