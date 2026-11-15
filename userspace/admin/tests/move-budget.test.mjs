// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The size of the Move to new hardware page's graph (design 3.8), measured
// as budget.test.mjs measures a page: its files beyond what the shell
// starts with, and its style sheets. The design expects 45 KB and caps it at
// 150 KB; the cap is held, and what is measured is reported against the
// expectation, so that the report of the work has the number.

import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

import { PAGES } from '../pages.js';
import { graphOf, ROOT, shown, sizeOf } from './graph.mjs';

const EXPECTED = 45_000;
const LIMIT = 150_000;

/** The files the shell starts with, as budget.test.mjs counts them. */
function shellFiles() {
  const html = readFileSync(`${ROOT}index.html`, 'utf8');
  const names = [...html.matchAll(/<(?:script|link)\b[^>]*?\b(?:src|href)="\/([^"]+)"/g)].map((found) => found[1]);
  const named = names.filter((name) => /\.(?:js|css)$/.test(name)).map((name) => `${ROOT}${name}`);
  return new Set([`${ROOT}index.html`, ...named, ...graphOf(`${ROOT}app.js`).files]);
}

test('the Move graph is within 150 KB beyond the shell, and its size is reported against the 45 KB expected', (context) => {
  const entry = PAGES.find((page) => page.id === 'move');
  assert.equal(entry.path, '/move');
  const files = graphOf(`${ROOT}pages/move/page.js`, shellFiles()).files;
  const sheets = (entry.css ?? []).map((name) => `${ROOT}${name}`).filter((file) => existsSync(file));
  const size = sizeOf(files) + sizeOf(sheets);
  context.diagnostic(`Move brings ${size} bytes beyond the shell (expected ${EXPECTED}, capped at ${LIMIT}): ${[...files, ...sheets].map(shown).join(', ')}`);
  assert.ok(size <= LIMIT, `Move brings ${size} bytes beyond the shell, over ${LIMIT}`);
  assert.ok(files.has(`${ROOT}pages/move/model.js`) && sheets.length > 0);
});
