// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The modules of the Reliability pages that draw do nothing when they are
// read (design 3.1): Node, which has no document, can read each of them, and
// each gives the functions the page and its neighbours call.

import assert from 'node:assert/strict';
import test from 'node:test';

const MODULES = {
  'days.js': ['show'],
  'export.js': ['makeExport'],
  'journal.js': ['show'],
  'page.js': ['show'],
  'start.js': ['show'],
  'starts.js': ['makeFailed', 'makeStarts'],
};

test('each module that draws can be read where there is no page, and gives its functions', async () => {
  assert.equal(typeof globalThis.document, 'undefined');
  for (const [file, names] of Object.entries(MODULES)) {
    const module = await import(`../pages/reliability/${file}`);
    for (const name of names) assert.equal(typeof module[name], 'function', `${file} ${name}`);
  }
});
