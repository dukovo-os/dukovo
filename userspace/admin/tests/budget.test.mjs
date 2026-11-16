// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The size budget of design 3.6, by walking the import graph: the shell's
// start (index.html, theme.js, the files app.js imports at load, and the
// style sheets and scripts index.html names) is at most 150 KB as served,
// and each graph that is started later, a page's or a dialog's, is at most
// 150 KB beyond what the shell already holds. Files are served as they are
// on disk, so their sizes are the bytes.

import assert from 'node:assert/strict';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { test } from 'node:test';
import { PAGES } from '../pages.js';
import { graphOf, ROOT, shown, sizeOf } from './graph.mjs';

// 150 KB is 150,000 bytes here, the smaller of the two readings.
const LIMIT = 150_000;

// The start is held under the limit with room for what the console takes in
// next: a change that adds to it raises this number and says what grew.
const START_PLAN = 149_600;

/** The files `index.html` names: its scripts and style sheets, as paths on disk. */
function pageFiles() {
  const html = readFileSync(`${ROOT}index.html`, 'utf8');
  const names = [...html.matchAll(/<(?:script|link)\b[^>]*?\b(?:src|href)="\/([^"]+)"/g)].map((found) => found[1]);
  return [`${ROOT}index.html`, ...names.filter((name) => /\.(?:js|css)$/.test(name)).map((name) => `${ROOT}${name}`)];
}

/** The shell's start: the page, its scripts and sheets, and what app.js imports at load. */
function shellStart() {
  const graph = graphOf(`${ROOT}app.js`);
  const files = new Set([...pageFiles(), ...graph.files]);
  return { files, later: graph.later };
}

test('the shell starts within 150 KB', () => {
  const { files } = shellStart();
  const size = sizeOf(files);
  const list = [...files].map((file) => `${statSync(file).size} ${shown(file)}`).sort((a, b) => parseInt(b, 10) - parseInt(a, 10));
  assert.ok(size <= LIMIT, `the shell starts with ${size} bytes, over ${LIMIT}:\n${list.join('\n')}`);
});

test('the shell starts within the number its next work has left room for', () => {
  const size = sizeOf(shellStart().files);
  assert.ok(size <= START_PLAN, `the shell starts with ${size} bytes, over the ${START_PLAN} that its next work has left room for`);
});

test('the shell starts with the page, its script and its sheet', () => {
  const names = pageFiles().map(shown);
  assert.ok(names.includes('theme.js') && names.includes('console.css'), names.join(', '));
});

/** Every graph started with `import()` from the shell, a page's own entry or a graph under it, with its size beyond the shell. */
function laterGraphs() {
  const shell = shellStart();
  const seen = new Map();
  const queue = [...shell.later];
  while (queue.length > 0) {
    const root = queue.shift();
    if (seen.has(root) || shell.files.has(root)) continue;
    const graph = graphOf(root, shell.files);
    seen.set(root, graph);
    queue.push(...graph.later);
  }
  return { shell, seen };
}

test('each graph the shell starts later is within 150 KB beyond the shell', () => {
  const { shell, seen } = laterGraphs();
  for (const [root, graph] of seen) {
    const size = sizeOf(graph.files);
    assert.ok(size <= LIMIT, `${shown(root)} brings ${size} bytes beyond the shell's ${sizeOf(shell.files)}, over ${LIMIT}`);
  }
});

test('each page with its style sheets is within 150 KB beyond the shell', () => {
  const { shell } = laterGraphs();
  for (const page of PAGES) {
    const entry = `${ROOT}pages/${page.id}/page.js`;
    let files = new Set();
    try {
      statSync(entry);
      files = graphOf(entry, shell.files).files;
    } catch {
      // A page with no `page.js` of its own: parts only.
    }
    // A sheet that is not there yet is left out, as a part is.
    const sheets = (page.css ?? []).map((name) => `${ROOT}${name}`).filter((file) => existsSync(file));
    const size = sizeOf(files) + sizeOf(sheets);
    assert.ok(size <= LIMIT, `page ${page.id} brings ${size} bytes beyond the shell, over ${LIMIT}`);
  }
});
