// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The split of the style sheets (design 23.1): console.css holds what the
// shell draws, pages.css what only some pages draw, and each of those pages
// names pages.css in its entry; dialogs.css holds what a dialog draws and
// sign-in.css what the sign-in page draws, each linked by the modules that
// draw them. The checks read the sources, so a page that
// draws a class of pages.css without naming it, or a class that moves back
// into the shell's files, fails here and not as an unstyled page. Every icon
// is a file of icons/ with one line of console.css, so that none is a
// drawing in the shell's start (C5 design 3.1); the checks hold the two
// together.

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import test from 'node:test';

import { PAGES } from '../pages.js';
import { importsOf, ROOT } from './graph.mjs';

const read = (file) => readFileSync(file, 'utf8');
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, '');
const consoleCss = stripComments(read(`${ROOT}console.css`));
const pagesCss = stripComments(read(`${ROOT}pages.css`));
// The other sheets the shell links: each has rules that console.css does not.
const OTHER_SHEETS = new Map(['pages.css', 'dialogs.css', 'sign-in.css'].map((name) => [name, stripComments(read(`${ROOT}${name}`))]));

const classesOf = (css) => new Set([...css.matchAll(/\.([A-Za-z][\w-]*)/g)].map((found) => found[1]));
const inShell = classesOf(consoleCss);
// A class both sheets have a rule for (`.on`, which the range control shares with others) is the shell's.
const heldByPages = new Set([...classesOf(pagesCss)].filter((name) => !inShell.has(name)));

// An icon's rule is one line, `.ic-NAME { --m: url("/icons/NAME.svg"); }`. The
// address is absolute: a relative url() in a custom property resolves against
// the sheet that uses it in some engines and against the one that sets it in
// others.
const ICON_RULE = /^\.ic-([a-z][a-z-]*) \{ --m: url\("([^"]*)"\); \}$/;
const iconLines = consoleCss.split('\n').filter((line) => line.startsWith('.ic-'));
const iconNamesWithRules = iconLines.map((line) => ICON_RULE.exec(line)?.[1]).filter(Boolean);

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

test('every icon rule is one line that names the icon\'s own file', () => {
  assert.ok(iconLines.length > 0, 'console.css has icon rules');
  for (const line of iconLines) {
    const found = ICON_RULE.exec(line);
    assert.ok(found, `${line.slice(0, 70)} is not \`.ic-NAME { --m: url("/icons/NAME.svg"); }\``);
    assert.equal(found[2], `/icons/${found[1]}.svg`, `.ic-${found[1]} names ${found[2]}`);
  }
});

test('every icon rule has its file in icons/ and every file has its rule', () => {
  const files = readdirSync(`${ROOT}icons`).sort();
  assert.deepEqual(files, iconNamesWithRules.map((name) => `${name}.svg`).sort());
  assert.equal(new Set(iconNamesWithRules).size, iconNamesWithRules.length, 'a rule is made twice');
});

test('every icon file is a 16 by 16 SVG with the licence comment, no script and no reference to anything else', () => {
  for (const name of iconNamesWithRules) {
    const text = read(`${ROOT}icons/${name}.svg`);
    assert.match(text, /^<!--\nSPDX-FileCopyrightText: .+\nSPDX-License-Identifier: .+\n-->\n<svg /, `${name}.svg starts with the licence comment and the svg element`);
    assert.match(text, /<svg xmlns=(["'])http:\/\/www\.w3\.org\/2000\/svg\1 /, `${name}.svg has the SVG namespace`);
    assert.match(text, /\bviewBox=(["'])0 0 16 16\1/, `${name}.svg has the viewBox 0 0 16 16`);
    const drawing = text.replace(/<!--[\s\S]*?-->/g, '').replace(/\bxmlns=(["'])http:\/\/www\.w3\.org\/2000\/svg\1/, '');
    assert.doesNotMatch(drawing, /<(?:script|style|image|use|foreignObject)\b|\bhref\s*=|xlink|\bon[a-z]+\s*=|url\(|https?:|data:|@import/i, `${name}.svg draws with paths only`);
  }
});

test('every page\'s icon has a rule in console.css and a file, since the navigation and the palette draw it before any page\'s sheet is linked', () => {
  for (const entry of PAGES) {
    assert.ok(iconNamesWithRules.includes(entry.icon), `${entry.id}: console.css has no .ic-${entry.icon}`);
    assert.ok(exists(`${ROOT}icons/${entry.icon}.svg`), `${entry.id}: icons/${entry.icon}.svg is not a file`);
  }
});

test('the icons of the power, devices and phone notices are there, as a file and as a rule', () => {
  for (const name of ['power', 'chip', 'push']) {
    assert.ok(iconNamesWithRules.includes(name), `console.css has no .ic-${name}`);
    assert.ok(exists(`${ROOT}icons/${name}.svg`), `icons/${name}.svg is not a file`);
  }
});

test('no sheet but console.css has an icon rule', () => {
  for (const [name, css] of OTHER_SHEETS) assert.doesNotMatch(css, /\.ic-/, name);
});

test('no rule is in two sheets', () => {
  const shellRules = new Set(consoleCss.split('\n').map((line) => line.trim()).filter(Boolean));
  for (const [name, css] of OTHER_SHEETS) {
    for (const line of css.split('\n').map((one) => one.trim()).filter((one) => one && one !== '}' && !one.startsWith('@'))) {
      assert.equal(shellRules.has(line), false, `${line.slice(0, 60)} is in console.css and in ${name}`);
    }
  }
});

test('no file of the shell draws a class that only pages.css has', () => {
  for (const file of shellFiles) {
    const drawn = classWords(read(file));
    for (const name of heldByPages) assert.equal(drawn.has(name), false, `${relative(ROOT, file)} draws .${name}, which only pages.css has`);
  }
});

test('a page whose files draw a class of pages.css names pages.css, before its own sheets', () => {
  for (const entry of PAGES) {
    const files = [...filesOfPage(entry)].filter((file) => !shellFiles.has(file));
    const draws = [];
    for (const file of files) {
      const classes = classWords(read(file));
      for (const name of heldByPages) if (classes.has(name)) draws.push(`.${name} in ${relative(ROOT, file)}`);
    }
    if (draws.length === 0) continue;
    assert.ok(entry.css.includes('pages.css'), `${entry.id} draws ${draws.join(', ')}, and does not name pages.css`);
    const own = entry.css.filter((name) => !['pages.css', 'boards.css', 'dialogs.css'].includes(name));
    assert.ok(entry.css.indexOf('pages.css') < Math.min(...own.map((name) => entry.css.indexOf(name)), Infinity), `${entry.id} lists pages.css after a sheet of its own`);
  }
});
