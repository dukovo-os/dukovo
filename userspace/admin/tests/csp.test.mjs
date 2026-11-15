// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the content security policy forbids (design 3.6), found in every file
// the console serves: markup built from strings, code built from strings,
// inline style and script, `javascript:` addresses and imports that are not
// relative. Setting `element.style.x` is allowed: the CSSOM is not inline
// style to the policy. The files of `tests/` are not served, and are not read.

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import test from 'node:test';

const ROOT = new URL('../', import.meta.url).pathname;

/** The paths of the files under `dir` that the console serves, which is all but `tests/`. */
function served(dir = ROOT) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === 'tests' && dir === ROOT ? [] : served(path);
    return [path];
  });
}

/** `source` without comments, which may name what they warn against. */
const withoutComments = (source) => source
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/(^|\s)\/\/.*$/gm, '$1');

// What a script must not hold, each with the words that say why.
const SCRIPT_PATTERNS = [
  [/\binnerHTML\b/, 'innerHTML makes markup from a string'],
  [/\bouterHTML\b/, 'outerHTML makes markup from a string'],
  [/\binsertAdjacentHTML\b/, 'insertAdjacentHTML makes markup from a string'],
  [/\bdocument\.write(?:ln)?\b/, 'document.write makes markup from a string'],
  [/\beval\s*\(/, 'eval runs a string as code'],
  [/\bnew\s+Function\b/, 'new Function runs a string as code'],
  [/setAttribute\(\s*['"`]style['"`]/, 'setAttribute("style") is inline style'],
  [/javascript:/i, 'a javascript: address is a script'],
];

/** The things `source` imports that are not relative: static, `export from` and `import()`. */
export function nonRelativeImports(source) {
  const text = withoutComments(source);
  const specifiers = [
    ...text.matchAll(/^\s*(?:import|export)\s[^'"`;]*?from\s*['"]([^'"]+)['"]/gm),
    ...text.matchAll(/^\s*import\s*['"]([^'"]+)['"]/gm),
    ...text.matchAll(/\bimport\(\s*['"`]([^'"`]+)['"`]\s*\)/g),
  ].map((found) => found[1]);
  return specifiers.filter((specifier) => !/^\.\.?\//.test(specifier));
}

/** What in the HTML of `source` the policy forbids. */
export function htmlProblems(source) {
  const text = source.replace(/<!--[\s\S]*?-->/g, '');
  const found = [];
  for (const [tag] of text.matchAll(/<[a-zA-Z][^>]*>/g)) {
    if (/\sstyle\s*=/i.test(tag)) found.push('a style attribute');
    if (/\son[a-z]+\s*=/i.test(tag)) found.push('an event attribute');
    if (/<script\b/i.test(tag) && !/\ssrc\s*=/i.test(tag)) found.push('a script with no src');
    if (/\s(?:href|src|action)\s*=\s*["']?\s*javascript:/i.test(tag)) found.push('a javascript: address');
  }
  return found;
}

test('no script holds markup or code made from a string, inline style or a javascript: address', () => {
  for (const file of served().filter((path) => path.endsWith('.js'))) {
    const text = withoutComments(readFileSync(file, 'utf8'));
    for (const [pattern, why] of SCRIPT_PATTERNS) {
      assert.doesNotMatch(text, pattern, `${relative(ROOT, file)}: ${why}`);
    }
  }
});

test('every import is relative', () => {
  for (const file of served().filter((path) => path.endsWith('.js'))) {
    assert.deepEqual(nonRelativeImports(readFileSync(file, 'utf8')), [], `${relative(ROOT, file)} imports from outside the console`);
  }
});

test('no page holds inline style or script', () => {
  for (const file of served().filter((path) => path.endsWith('.html'))) {
    assert.deepEqual(htmlProblems(readFileSync(file, 'utf8')), [], relative(ROOT, file));
  }
});

// A sheet that imports another loads it only once it has loaded itself, and
// the accessibility check of the lab tests reads an imported sheet by putting
// it in a style element, which the policy stops: a page's sheets are listed
// in pages.js instead, and load together.
test('no style sheet imports another or holds a javascript: address', () => {
  for (const file of served().filter((path) => path.endsWith('.css'))) {
    const text = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    assert.doesNotMatch(text, /@import\b/i, `${relative(ROOT, file)}: an import, where pages.js should list the sheet`);
    assert.doesNotMatch(text, /javascript:/i, `${relative(ROOT, file)}: a javascript: address`);
  }
});

test('the detectors find what they are for', () => {
  assert.deepEqual(nonRelativeImports("import a from 'lit';\nimport './x.js';\nexport * from '../y.js';\nconst z = await import('https://x.test/z.js');"), ['lit', 'https://x.test/z.js']);
  assert.deepEqual(nonRelativeImports("// import a from 'lit';\nimport b from './b.js';"), []);
  assert.deepEqual(htmlProblems('<div style="x"></div><p onclick="f()"></p><script>1</script><script src="/a.js"></script>'), [
    'a style attribute', 'an event attribute', 'a script with no src',
  ]);
  assert.deepEqual(htmlProblems('<!-- <p style="x"> --><a href="/a">a</a><script type="module" src="/app.js"></script>'), []);
  for (const [pattern] of SCRIPT_PATTERNS) assert.ok(pattern instanceof RegExp);
  assert.match('el.innerHTML = x', SCRIPT_PATTERNS[0][0]);
  assert.match("el.setAttribute('style', 'x')", SCRIPT_PATTERNS[6][0]);
  assert.doesNotMatch("el.style.setProperty('--v', '1')", SCRIPT_PATTERNS[6][0]);
});

test('the files read are the ones the console serves', () => {
  const names = served().map((path) => relative(ROOT, path));
  assert.ok(names.includes('app.js') && names.includes('index.html') && names.includes('console.css'));
  assert.ok(!names.some((name) => name.startsWith('tests/')));
});
