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
// next: a change that adds to it raises this number and says what grew. The
// manifest's link in index.html is 51 bytes of it, and the two more sheets
// the entry of Websites names (dialogs.css and the tried change's banner) 38
// bytes in pages.js. The entry of AI clients, its icon and the read of its
// count when a request moves are 633 bytes, less than the 734 of the eight
// rules of console.css that a later rule of the same selector repeated in
// full, which left with them.
const START_PLAN = 149_837;

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

/** The bytes `page` brings beyond the shell: its page and what it imports, with the style sheets its entry lists. */
function pageBytes(page, shell) {
  const entry = `${ROOT}pages/${page.id}/page.js`;
  let files = new Set();
  try {
    statSync(entry);
    files = graphOf(entry, shell.files).files;
  } catch {
    // A page with no `page.js` of its own: parts only.
  }
  // A sheet that is not there yet is left out, as a part is. dialogs.css is
  // the shell's: frame.js links it for every page, so one that lists it only
  // has it sooner and does not bring it.
  const sheets = (page.css ?? []).filter((name) => name !== 'dialogs.css').map((name) => `${ROOT}${name}`).filter((file) => existsSync(file));
  return sizeOf(files) + sizeOf(sheets);
}

test('each page with its style sheets is within 150 KB beyond the shell', () => {
  const { shell } = laterGraphs();
  for (const page of PAGES) {
    const size = pageBytes(page, shell);
    assert.ok(size <= LIMIT, `page ${page.id} brings ${size} bytes beyond the shell, over ${LIMIT}`);
  }
});

// A page's own row, tighter than the 150 KB: the bytes it brings beyond the
// shell, with its sheets. A page that grows past its row fails with its name;
// one that needs more says so, with what grew, and raises the row. Security,
// Move and Reliability keep a little room for what they gain; the Terminal's
// graph is as it was.
// Report a problem's row is 7,000 more than it was, for the cuts the page makes.
// Apps' row is 1,100 more than it was, for the mark that the page that adds an
// app draws too, which has a module of its own, and for the count of what
// nginx and PostgreSQL manage.
// Updates' row is 11,700 more than it was, for what the page gained: the
// words of a hold's reason, who and when (held rows, the cards' lines, the
// names of accounts), Later with its menu and the muted lines that say why it
// is not offered (put-off.js, the cards, the Apps rows), Install tonight with
// the window's words and the sheet of an update that asks for more, and the
// rows of the Apps card that carry them. The question that asks why, and the
// sheet's reading, are loaded when they are wanted and are not in the row.
// Updates' row is a further 1,000 more, for the kind a rebuild is and the build
// number the version words take off before they compare. Apps' row is 1,200
// more for the same, and for the reason a rebuild's banner gives; Add an
// app's is 500 more.
// Apps' row is 5,400 more than it was, for the Size row's every state, the
// hold row's words, and Tonight, Not tonight and the window's words.
// Hardware's row is 6,000 more than the plan's: the disks' rows and the memory test at the next restart,
// and 1,000 more for lib/how.js, which the task words it reads take in to name the client that asked for a task,
// and 500 more for the Screens card; its list and its model are read when its button is pressed and are not in the row.
// It is 2,500 more again for the Battery and UPS card, its model and its bar (ADR 0741), which a machine
// with neither never reads.
// Move's is 500 more than 107,000 for lib/how.js too.
// Alerts' is 5,000 more than 60,000: 4,000 for the kinds of alert each channel is told of and 1,000 for the card of Set up a phone.
// Security's is 2,500 more than 147,000: 1,500 for the rows of its Alerts card, one for each kind of alert, and 1,000 for the steps' rules in pages.css.
// It is 300 more again for the trial banner's count of its journal reads, so that a late answer does not hide who began a trial.
// It is 100 more again for the weight of the check that the processor has a flaw nothing closes, SEC-180.
// Add an app, Websites and Databases each keep about 1,000 of room over what
// they bring; Websites' list reads the whole model of a site, its words and checks.
// Databases' row is 4,200 more than it was, for the firewall's row and its rule that
// comes first, the copy card and the Restore… button; the dialog, its model and the
// steps of the restore are read when Restore… is pressed and are not in the row.
// Websites' row is 3,300 more than it was, for the Size and Visits yesterday columns, the
// card of a site's visits with its words, the HTTPS row's words while a certificate is
// being got, and the row for settings with no log format; the chart and its model are read
// when a site that has been counted is shown and are not in the row.
const PAGE_LIMITS = {
  updates: 82_700,
  apps: 62_700,
  hardware: 65_000,
  certificates: 60_000,
  report: 67_000,
  alerts: 65_000,
  security: 149_900,
  move: 107_500,
  reliability: 22_000,
  terminal: 149_908,
  'add-app': 56_000,
  websites: 84_600,
  databases: 61_000,
};

// The parts the Overview reads from a page, each a graph the shell starts
// later, and the service worker, which no graph holds: the browser fetches it.
const PART_LIMITS = {
  'pages/updates/overview.js': 8_000,
  'pages/hardware/overview.js': 8_000,
  'pages/alerts/overview.js': 8_000,
  'pages/alerts/phone-setup.js': 8_000,
  'sw.js': 6_000,
};

test('each page is within its own row', () => {
  const { shell } = laterGraphs();
  for (const [id, limit] of Object.entries(PAGE_LIMITS)) {
    const page = PAGES.find((one) => one.id === id);
    assert.ok(page, `${id} is a page of the console`);
    const size = pageBytes(page, shell);
    assert.ok(size <= limit, `page ${id} brings ${size} bytes beyond the shell, over its row of ${limit}`);
  }
});

test('each part the Overview reads, and the service worker, is within its own row', () => {
  const { shell } = laterGraphs();
  for (const [name, limit] of Object.entries(PART_LIMITS)) {
    const file = `${ROOT}${name}`;
    // A part that is not written yet is left out, as a page's missing sheet is.
    if (!existsSync(file)) continue;
    const size = name.endsWith('sw.js') ? statSync(file).size : sizeOf(graphOf(file, shell.files).files);
    assert.ok(size <= limit, `${name} brings ${size} bytes, over its row of ${limit}`);
  }
});
