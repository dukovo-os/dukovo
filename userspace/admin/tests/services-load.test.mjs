// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The modules of the Services pages that draw do nothing when they are read
// (design 3.1): Node, which has no document, can read each of them, and each
// gives the functions the page and its neighbours call.

import assert from 'node:assert/strict';
import test from 'node:test';

const MODULES = {
  'cards.js': ['copyBox', 'frameOf', 'makeCard', 'quiet', 'rowsList', 'stable'],
  'controls.js': ['makeControls'],
  'detail.js': ['show'],
  'list.js': ['show'],
  'page.js': ['show'],
  'recent-log.js': ['makeRecentLog'],
  'scheduled.js': ['show'],
  'timer-card.js': ['makeTimer'],
  'use.js': ['makeUse'],
};

test('each module that draws can be read where there is no page, and gives its functions', async () => {
  assert.equal(typeof globalThis.document, 'undefined');
  for (const [file, names] of Object.entries(MODULES)) {
    const module = await import(`../pages/services/${file}`);
    for (const name of names) assert.equal(typeof module[name], 'function', `${file} ${name}`);
  }
});
