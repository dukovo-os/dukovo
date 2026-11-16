// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The square that stands for an app, which the Apps pages and the page that
// adds an app draw: its letter, its colour, the class that each page draws it
// with, and that the two pages' sheets give the square the same shape and the
// same six colours, since each page carries the rules of its own class and no
// page carries the other's.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { initialOf, square, tintOf } from '../pages/apps/mark.js';
import { usePage } from './fake-dom.mjs';
import { ROOT } from './graph.mjs';

const read = (name) => readFileSync(`${ROOT}${name}`, 'utf8');

/** The rules of `css` that name `kind`, with the class written as `.mark`, as `[selector, declarations]`. */
function rulesOf(css, kind) {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  return [...text.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .map((found) => [found[1].trim().replaceAll(kind, 'mark'), found[2].trim()])
    .filter(([selector]) => selector.includes('mark') && !selector.includes('.add-') && !selector.includes('.apps-'));
}

test('the initial is the first letter of the name in capitals, a question mark when there is none, and a whole character', () => {
  assert.equal(initialOf('nginx'), 'N');
  assert.equal(initialOf('  python'), 'P');
  assert.equal(initialOf(''), '?');
  assert.equal(initialOf('\u{1F600} smile'), '\u{1F600}');
});

test('the colour is one of six that the ID picks, the same every time', () => {
  for (const id of ['org.nginx.nginx', 'net.php.php', 'org.postgresql.postgresql', 'org.example.notes', 'x']) {
    assert.equal(tintOf(id), tintOf(id));
    assert.ok(Number.isInteger(tintOf(id)) && tintOf(id) >= 0 && tintOf(id) <= 5, id);
  }
  assert.ok(new Set(Array.from({ length: 60 }, (_, at) => tintOf(`org.example.app${at}`))).size >= 4, 'the IDs are spread over the colours');
});

test('the square carries the class its page names, its colour and its size, and is not read', (context) => {
  usePage(context);
  const small = square({ initial: 'N', tint: 3 }, 'add-ic');
  assert.equal(small.getAttribute('class'), 'add-ic tint-3');
  assert.equal(small.getAttribute('aria-hidden'), 'true');
  assert.equal(small.textContent, 'N');
  assert.equal(square({ initial: 'P', tint: 0 }, 'apps-ic', true).getAttribute('class'), 'apps-ic tint-0 big');
});

test('each page draws the square with the class its own sheet has, and names no other', () => {
  const draws = (names) => names.map((name) => [name, [...read(name).matchAll(/square\([^)]*'([\w-]+)'/g)].map((found) => found[1])]);
  assert.deepEqual(draws(['pages/apps/list.js', 'pages/apps/app.js']).map(([, kinds]) => [...new Set(kinds)]), [['apps-ic'], ['apps-ic']]);
  assert.deepEqual(draws(['pages/add-app/list.js', 'pages/add-app/sheet.js']).map(([, kinds]) => [...new Set(kinds)]), [['add-ic'], ['add-ic']]);
  assert.ok(!read('pages/apps/mark.js').includes('apps-ic') && !read('pages/apps/mark.js').includes('add-ic'), 'the module names the class of no page');
});

test('the sheets of the two pages give the square the same shape, the same six colours and the same forced colours', () => {
  const apps = rulesOf(read('pages/apps/apps.css'), 'apps-ic');
  const add = rulesOf(read('pages/add-app/add.css'), 'add-ic');
  assert.ok(apps.length >= 8, `the Apps sheet has ${apps.length} rules of the square`);
  assert.deepEqual(add, apps);
  assert.equal(new Set(apps.map(([selector]) => selector)).size, apps.length, 'each rule is once');
  assert.ok(apps.some(([selector]) => selector === '.dk .mark'), 'forced colours have a rule');
});
