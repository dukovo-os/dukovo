// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The split of the style sheets (design 23.1): console.css holds what the
// shell draws, pages.css what only some pages draw, and each of those pages
// names pages.css in its entry. The checks read the sources, so a page that
// draws a class of pages.css without naming it, or a class that moves back
// into the shell's files, fails here and not as an unstyled page.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import test from 'node:test';

import { PAGES } from '../pages.js';
import { importsOf, ROOT } from './graph.mjs';

const read = (file) => readFileSync(file, 'utf8');
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, '');
const consoleCss = stripComments(read(`${ROOT}console.css`));
const pagesCss = stripComments(read(`${ROOT}pages.css`));

const classesOf = (css) => new Set([...css.matchAll(/\.([A-Za-z][\w-]*)/g)].map((found) => found[1]));
const iconsIn = (css) => new Set([...classesOf(css)].filter((name) => name.startsWith('ic-')).map((name) => name.slice(3)));
const inShell = classesOf(consoleCss);
// A class both sheets have a rule for (`.on`, which the range control shares with others) is the shell's.
const heldByPages = new Set([...classesOf(pagesCss)].filter((name) => !name.startsWith('ic-') && !inShell.has(name)));
const iconsInPages = iconsIn(pagesCss);

/** The words a source gives a class attribute or `classList`: the quoted pieces after `class:`, `className` and `classList.x(`. */
function classWords(source) {
  const text = source.replace(/\/\*[\s\S]*?\*\//g, '').split('\n').filter((line) => !line.trim().startsWith('//')).join('\n');
  const bodies = [
    ...[...text.matchAll(/\bclass\s*:\s*([^}]*)/g)].map((found) => found[1]),
    ...[...text.matchAll(/\bclassName\s*=\s*([^;\n]*)/g)].map((found) => found[1]),
    ...[...text.matchAll(/\bclassList\.\w+\(([^)]*)\)/g)].map((found) => found[1]),
  ];
  const words = new Set();
  for (const body of bodies) {
    for (const quoted of body.matchAll(/'([^']*)'|"([^"]*)"|`([^`]*)`/g)) {
      for (const word of (quoted[1] ?? quoted[2] ?? quoted[3]).replace(/\$\{[^}]*\}/g, ' ').split(/\s+/)) if (word) words.add(word);
    }
  }
  return words;
}

/** The icon names a source draws with `icon('name')`. */
const iconNames = (source) => new Set([...source.matchAll(/\bicon\(\s*'([a-z-]+)'\s*\)/g)].map((found) => found[1]));

/** Every file reached from `roots` by imports, `follow` deciding which are entered. */
function reach(roots, follow) {
  const seen = new Set();
  const queue = [...roots];
  while (queue.length > 0) {
    const file = queue.pop();
    if (seen.has(file) || !follow(file)) continue;
    seen.add(file);
    const { fixed, later } = importsOf(read(file));
    for (const specifier of [...fixed, ...later]) queue.push(resolve(dirname(file), specifier));
  }
  return seen;
}

const inPages = (file) => relative(ROOT, file).startsWith('pages/');
const exists = (file) => {
  try {
    readFileSync(file);
    return true;
  } catch {
    return false;
  }
};

// What the shell loads and draws outside any page: app.js and what it imports, never entering a page.
const shellFiles = reach([`${ROOT}app.js`, `${ROOT}theme.js`], (file) => !inPages(file) && exists(file));

/** The files a page can draw with: its page and parts, and what they import. */
function filesOfPage(entry) {
  const paths = [entry.load, ...Object.values(entry.parts)].filter(Boolean).map((load) => /import\('([^']+)'\)/.exec(String(load))[1]);
  return reach(paths.map((path) => resolve(ROOT, path)), exists);
}

test('every page\'s icon is drawn by console.css, since the navigation and the palette draw it before any page\'s sheet is linked', () => {
  const drawn = iconsIn(consoleCss);
  for (const entry of PAGES) assert.ok(drawn.has(entry.icon), `${entry.id}: console.css has no .ic-${entry.icon}`);
});

test('no rule is in both sheets', () => {
  const shellRules = new Set(consoleCss.split('\n').map((line) => line.trim()).filter(Boolean));
  for (const line of pagesCss.split('\n').map((one) => one.trim()).filter((one) => one && one !== '}' && !one.startsWith('@'))) {
    assert.equal(shellRules.has(line), false, `${line.slice(0, 60)} is in console.css too`);
  }
});

test('no file of the shell draws a class or an icon that only pages.css has', () => {
  for (const file of shellFiles) {
    const source = read(file);
    const drawn = classWords(source);
    for (const name of heldByPages) assert.equal(drawn.has(name), false, `${relative(ROOT, file)} draws .${name}, which only pages.css has`);
    for (const name of iconNames(source)) assert.equal(iconsInPages.has(name), false, `${relative(ROOT, file)} draws the icon ${name}, which only pages.css has`);
  }
});

test('a page whose files draw a class or an icon of pages.css names pages.css, before its own sheets', () => {
  for (const entry of PAGES) {
    const files = [...filesOfPage(entry)].filter((file) => !shellFiles.has(file));
    const draws = [];
    for (const file of files) {
      const source = read(file);
      const classes = classWords(source);
      for (const name of heldByPages) if (classes.has(name)) draws.push(`.${name} in ${relative(ROOT, file)}`);
      for (const name of iconNames(source)) if (iconsInPages.has(name)) draws.push(`the icon ${name} in ${relative(ROOT, file)}`);
    }
    if (draws.length === 0) continue;
    assert.ok(entry.css.includes('pages.css'), `${entry.id} draws ${draws.join(', ')}, and does not name pages.css`);
    const own = entry.css.filter((name) => name !== 'pages.css' && name !== 'boards.css');
    assert.ok(entry.css.indexOf('pages.css') < Math.min(...own.map((name) => entry.css.indexOf(name)), Infinity), `${entry.id} lists pages.css after a sheet of its own`);
  }
});
