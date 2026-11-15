// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The modules of the Logs page that draw (design 23.1: DOM modules do
// nothing at import) load where there is no document, so a page that is not
// opened costs nothing and a test can read their exports.

import assert from 'node:assert/strict';
import test from 'node:test';

const FILES = [
  '../pages/logs/page.js',
  '../pages/logs/filters.js',
  '../pages/logs/follow.js',
  '../pages/logs/chart-view.js',
  '../lib/log-list.js',
  '../lib/chart.js',
  '../lib/save-file.js',
];

test('no document is needed to load the modules that draw', async () => {
  assert.equal(typeof document, 'undefined');
  for (const file of FILES) {
    const loaded = await import(file);
    assert.ok(Object.keys(loaded).length > 0, file);
  }
});
