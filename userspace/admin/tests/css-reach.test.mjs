// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The reach of the style sheets: every class a module names must be defined
// in a sheet that its graph has. The shell starts with console.css alone; a
// page's entry lists its own sheets; a dialog, the sign-in page and the
// other graphs the shell starts later are linked by `sheet()`, which also
// gives every graph that opens over a page dialogs.css, since frame.js links
// it once the first page has drawn. A graph that opens before the first page
// draws has to link dialogs.css itself. A class that a sheet of the console
// defines and no sheet of the graph does fails with the graph, the module,
// the class and the sheet that has it, so that a rule that moved out of
// console.css cannot leave a module drawing it bare. A page that draws what
// dialogs.css holds in its own content lists it, so that it has it at its
// first paint; and a drawer or panel that opens over any page waits for it.

import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import test from 'node:test';

import { PAGES } from '../pages.js';
import {
  classesOf, classNamesOf, filesUnder, sheetLinksOf, startFiles,
} from './css-reach.mjs';
import { graphOf, ROOT } from './graph.mjs';

const read = (file) => readFileSync(file, 'utf8');
const shown = (file) => relative(ROOT, file);

// What a graph that opens before the first page draws has to link itself.
const EARLY = ['sign-in.js', 'limited.js', 'unlock-dialog.js', 'account-steps.js'];

// Sheets that the router gives a route which is no entry of pages.js.
const ROUTE_SHEETS = { 'help/help.js': ['help/help.css'] };

// What a module names that its graph has no sheet for, and that is older than
// this test: the picker of lib/chart.js is drawn only when a page passes
// `pick`, which only Logs does, but the module names its classes whatever
// page has it; the card body and the list of changed files are drawn on pages
// whose sheets do not hold their rules. Each is `module|class`. The test fails
// for one that is not here, and for one here that no graph names any more, so
// that the list only shrinks.
const KNOWN = [
  'lib/chart.js|chart-frame', 'lib/chart.js|cursor', 'lib/chart.js|mark', 'lib/chart.js|marks', 'lib/chart.js|marks-box',
  'lib/chart.js|pick', 'lib/chart.js|pick-label', 'lib/chart.js|pick-layer',
  'pages/overview/card.js|card-body', 'pages/overview/change-dialogs.js|all-list',
];

// What dialogs.css holds that a page draws in its own content and not in a
// dialog: the meters, and the head of a command card. A page whose modules
// name one lists dialogs.css before its other sheets, so that the rules are
// there at its first paint and come before the ones its own sheet gives them.
const IN_PAGES = ['cmd-head', 'meter'];

// The modules that draw one of them over any page, in a drawer or a panel,
// and so wait for dialogs.css themselves.
const OVER_PAGES = ['tasks/cards.js', 'lib/command-panel.js'];

const start = startFiles();

// Each class a sheet of the console defines, with the sheets that define it.
const definedIn = new Map();
for (const sheet of filesUnder(ROOT, '.css')) {
  for (const name of classesOf(read(`${ROOT}${sheet}`))) definedIn.set(name, [...(definedIn.get(name) ?? []), sheet]);
}

/** The files `roots` and what they import, at load or later, beyond the start. */
function filesOf(roots) {
  const files = new Set();
  const queue = [...roots];
  while (queue.length > 0) {
    const root = queue.pop();
    if (files.has(root) || start.has(root) || !existsSync(root)) continue;
    const graph = graphOf(root, start);
    for (const file of graph.files) files.add(file);
    queue.push(...graph.later);
  }
  return files;
}

/** The module an `import('...')` in the source of `load` names, from the console's folder. */
const moduleOf = (load) => resolve(ROOT, /import\('([^']+)'\)/.exec(String(load))[1]);

/** The graph of the entry `entry`: its page and its parts, with its own sheets. */
function entryGraph(entry) {
  const roots = [entry.load, ...Object.values(entry.parts)].filter(Boolean).map(moduleOf);
  return { name: `the page ${entry.id}`, files: filesOf(roots), sheets: ['console.css', 'dialogs.css', ...entry.css] };
}

/** The graphs the shell starts later that are no page: each file it imports at load with the sheets that open over any page. */
function laterGraphs() {
  const graphs = [];
  const seen = new Set();
  const queue = [...graphOf(`${ROOT}app.js`).later].filter((root) => !shown(root).startsWith('pages/'));
  while (queue.length > 0) {
    const root = queue.shift();
    if (seen.has(root) || start.has(root)) continue;
    seen.add(root);
    const graph = graphOf(root, start);
    const name = shown(root);
    graphs.push({
      name: `the graph of ${name}`,
      files: graph.files,
      sheets: ['console.css', ...(EARLY.includes(name) ? [] : ['dialogs.css']), ...(ROUTE_SHEETS[name] ?? [])],
    });
    queue.push(...[...graph.later].filter((later) => !shown(later).startsWith('pages/')));
  }
  return graphs;
}

/** What is wrong in `graph`: a class a module names that only a sheet the graph has not got defines, as `{known, words}`. */
function problemsOf(graph) {
  const linked = [...graph.files].flatMap((file) => sheetLinksOf(read(file)));
  const allowed = new Set([...graph.sheets, ...linked]);
  const problems = [];
  for (const file of graph.files) {
    for (const name of classNamesOf(read(file))) {
      const owners = definedIn.get(name);
      if (owners && !owners.some((sheet) => allowed.has(sheet))) {
        problems.push({ known: `${shown(file)}|${name}`, words: `${graph.name}: ${shown(file)} names .${name}, which only ${owners.join(' and ')} has` });
      }
    }
  }
  return problems;
}

/** The shell's own start, which draws before any sheet but console.css is linked. */
const startGraph = { name: 'the shell\'s start', files: new Set([...start].filter((file) => file.endsWith('.js'))), sheets: ['console.css'] };

const graphs = [startGraph, ...PAGES.map(entryGraph), ...laterGraphs()];
const problems = graphs.flatMap(problemsOf);

test('every graph has the sheets of the classes its modules name', () => {
  assert.deepEqual(problems.filter((one) => !KNOWN.includes(one.known)).map((one) => one.words), []);
});

test('every class that a module names without a sheet for it, and that is known, is still named', () => {
  assert.deepEqual(KNOWN.filter((known) => !problems.some((one) => one.known === known)), []);
});

test('the graphs that open before the first page draws link dialogs.css and sign-in.css themselves', () => {
  const found = new Map(graphs.map((graph) => [graph.name, graph]));
  const linksOf = (name) => [...found.get(`the graph of ${name}`).files].flatMap((file) => sheetLinksOf(read(file)));
  for (const name of ['sign-in.js', 'limited.js']) assert.deepEqual(linksOf(name).sort(), ['dialogs.css', 'sign-in.css'], name);
  assert.deepEqual(linksOf('unlock-dialog.js'), ['dialogs.css']);
  assert.deepEqual(linksOf('account-steps.js'), ['sign-in.css']);
});

test('a page that draws a meter or a command head lists dialogs.css first', () => {
  const drawn = (entry) => [...entryGraph(entry).files].flatMap((file) => [...classNamesOf(read(file))].filter((name) => IN_PAGES.includes(name)).map((name) => `.${name} in ${shown(file)}`));
  const late = PAGES.filter((entry) => drawn(entry).length > 0 && entry.css[0] !== 'dialogs.css');
  assert.deepEqual(late.map((entry) => `${entry.id} draws ${drawn(entry)[0]} and does not list dialogs.css first`), []);
  assert.ok(PAGES.filter((entry) => drawn(entry).length > 0).length >= 9, 'the pages that draw them are found');
});

test('the drawer and the panel that draw a meter or a command head over any page wait for dialogs.css', () => {
  for (const name of OVER_PAGES) {
    assert.ok(sheetLinksOf(read(`${ROOT}${name}`)).includes('dialogs.css'), `${name} links dialogs.css`);
    assert.ok(classNamesOf(read(`${ROOT}${name}`)).size > 0, `${name} names classes`);
  }
});

test('the graphs the shell starts later are the ones the console has', () => {
  const names = graphs.map((graph) => graph.name.replace('the graph of ', ''));
  for (const name of ['palette/palette.js', 'shell/dialogs.js', 'shell/more.js', 'tasks/cards.js', 'lib/command-work.js', 'lib/command-panel.js', ...EARLY]) {
    assert.ok(names.includes(name), `${name} is a graph the shell starts later`);
  }
});

test('the classes a module names are found where a class goes', () => {
  const names = (source) => [...classNamesOf(source)].sort();
  assert.deepEqual(names("h('div', { class: 'card muted', title: 'not a class' })"), ['card', 'muted']);
  assert.deepEqual(names("h('i', { class: kind === 'bad' ? 'meter bad' : `meter ${state} wait`, role: 'x' })"), ['bad', 'meter', 'wait']);
  assert.deepEqual(names("box.className = 'tbl-box'; other('not here');"), ['tbl-box']);
  assert.deepEqual(names("el.classList.add('on', 'sel'); el.classList.toggle('open', true)"), ['on', 'open', 'sel']);
  assert.deepEqual(names("el.closest('.dialog-head .btn'); el.querySelector('[data-x] .num')"), ['btn', 'data-x', 'dialog-head', 'num']);
  assert.deepEqual(names("// class: 'commented'\nh('p', { class: pill('a, b') })"), ['a', 'b']);
});

test('the classes a sheet defines are the ones its selectors name', () => {
  const css = '/* .gone { } */ .a, .b .c { x: 1; }\n@media (max-width: 1px) { .d:hover { y: .5rem; } }\n@keyframes k { from { m: 0; } }\n.ic-x { --m: url("/icons/x.svg"); }';
  assert.deepEqual([...classesOf(css)].sort(), ['a', 'b', 'c', 'd', 'ic-x']);
});

test('a sheet is linked by a call with its path, with or without the slash', () => {
  assert.deepEqual(sheetLinksOf("await sheet('/dialogs.css'); await sheet(\"sign-in.css\");\n// sheet('/no.css')"), ['dialogs.css', 'sign-in.css']);
});
