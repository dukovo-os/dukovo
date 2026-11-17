// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/route-model.js: which page owns an address, and which view a
// session gets, with the sign-in, second-step and first-sign-in choices.

import assert from 'node:assert/strict';
import test from 'node:test';

import { chooseView, followsInPage, matchPage, normal, titleFor } from '../shell/route-model.js';

const pages = [
  { id: 'overview', path: '/', needs: 'person' },
  { id: 'terminal', path: '/terminal', needs: 'person' },
  { id: 'terminal-more', path: '/terminal/sessions/archive', needs: 'person' },
  { id: 'setup', path: '/setup', needs: 'admin' },
  { id: 'welcome', path: '/welcome', needs: 'person' },
  { id: 'account', path: '/account', needs: 'person' },
];

const person = { name: 'alice', admin: false, needs_step: false };
const admin = { name: 'chase', admin: true, needs_step: false };
const zoned = { zone: 'Europe/Prague' };
const unzoned = { zone: '' };

test('the page that owns an address is the one with the longest path that is a prefix at a slash', () => {
  assert.equal(matchPage(pages, '/terminal').entry.id, 'terminal');
  assert.deepEqual([matchPage(pages, '/terminal/sessions').entry.id, matchPage(pages, '/terminal/sessions').rest], ['terminal', 'sessions']);
  assert.equal(matchPage(pages, '/terminal/sessions/archive').entry.id, 'terminal-more');
  assert.equal(matchPage(pages, '/terminal/sessions/archive/2024').rest, '2024');
  assert.equal(matchPage(pages, '/terminal/sessions/archived').entry.id, 'terminal', 'not at a slash');
  assert.equal(matchPage(pages, '/terminal').rest, '');
});

test('the overview owns the root and nothing under it', () => {
  assert.deepEqual(matchPage(pages, '/'), { entry: pages[0], rest: '' });
  assert.equal(matchPage(pages, '/nothing'), null);
  assert.equal(matchPage(pages, '/terminals'), null, 'a path that only starts with the same letters');
  assert.equal(matchPage(pages, '/setup/x/y').rest, 'x/y');
});

test('no session is the sign-in pages, whatever is asked', () => {
  assert.deepEqual(chooseView({ pages, session: null, prefs: null, path: '/terminal' }), { kind: 'sign-in' });
});

test('a session limited to a second step gets that page alone, whatever it asks for', () => {
  const limited = { ...person, needs_step: true };
  for (const path of ['/', '/terminal', '/welcome', '/nothing']) {
    assert.deepEqual(chooseView({ pages, session: limited, prefs: null, path }), { kind: 'second-step' });
  }
});

test('a person with no time zone is taken to welcome first, and then to what they asked for', () => {
  assert.deepEqual(chooseView({ pages, session: person, prefs: unzoned, path: '/terminal/sessions' }), { kind: 'welcome', then: '/terminal/sessions' });
  assert.deepEqual(chooseView({ pages, session: person, prefs: unzoned, path: '/' }), { kind: 'welcome', then: '/' });
  const there = chooseView({ pages, session: person, prefs: unzoned, path: '/welcome' });
  assert.equal(there.kind, 'page');
  assert.equal(there.entry.id, 'welcome');
});

test('the help pages and the second step are open to a person who has not been welcomed yet', () => {
  assert.deepEqual(chooseView({ pages, session: person, prefs: unzoned, path: '/help/errors/ADM-123' }), { kind: 'help', rest: 'errors/ADM-123' });
  assert.deepEqual(chooseView({ pages, session: person, prefs: unzoned, path: '/second-step' }), { kind: 'second-step-page' });
});

test('welcome is for the first sign-in alone: once the zone is set it goes to the overview', () => {
  assert.deepEqual(chooseView({ pages, session: person, prefs: zoned, path: '/welcome' }), { kind: 'redirect', to: '/' });
});

test('until the preferences are read, nothing is taken for a first sign-in', () => {
  assert.equal(chooseView({ pages, session: person, prefs: null, path: '/account' }).kind, 'page');
  assert.equal(chooseView({ pages, session: person, prefs: null, path: '/welcome' }).kind, 'page');
});

test('a person with a zone gets the page asked for, with what follows its path', () => {
  const view = chooseView({ pages, session: person, prefs: zoned, path: '/terminal/settings' });
  assert.deepEqual([view.kind, view.entry.id, view.rest], ['page', 'terminal', 'settings']);
  assert.equal(chooseView({ pages, session: person, prefs: zoned, path: '/' }).entry.id, 'overview');
});

test('a reader who asks for an administrators\' page gets its reader view, and an administrator the page', () => {
  const reader = chooseView({ pages, session: person, prefs: zoned, path: '/setup' });
  assert.deepEqual([reader.kind, reader.entry.id], ['admins-only', 'setup']);
  const owner = chooseView({ pages, session: admin, prefs: zoned, path: '/setup' });
  assert.deepEqual([owner.kind, owner.entry.id], ['page', 'setup']);
});

test('an address with no page is "No page here", and the shell\'s own pages are its own', () => {
  assert.deepEqual(chooseView({ pages, session: person, prefs: zoned, path: '/nothing/here' }), { kind: 'none' });
  assert.deepEqual(chooseView({ pages, session: person, prefs: zoned, path: '/second-step' }), { kind: 'second-step-page' });
  assert.deepEqual(chooseView({ pages, session: person, prefs: zoned, path: '/help' }), { kind: 'help', rest: '' });
  assert.deepEqual(chooseView({ pages, session: person, prefs: zoned, path: '/help/errors' }), { kind: 'help', rest: 'errors' });
});

test('an address with a slash at its end is asked for again without it', () => {
  assert.deepEqual(chooseView({ pages, session: person, prefs: zoned, path: '/terminal/' }), { kind: 'redirect', to: '/terminal' });
  assert.equal(normal('/'), '/');
  assert.equal(normal('/a//'), '/a');
  assert.equal(normal('/a/b'), '/a/b');
});

test('the title of a tab is the page, the machine and the console', () => {
  assert.equal(titleFor('Terminal', 'duke-nas'), 'Terminal · duke-nas · Dukovo Admin');
  assert.equal(titleFor('', 'duke-nas'), 'duke-nas · Dukovo Admin');
  assert.equal(titleFor('Overview', ''), 'Overview · Dukovo Admin');
});

test('a click on a link of the console is followed in the page, and the browser keeps the rest', () => {
  const origin = 'https://duke-nas:9090';
  const plain = { button: 0, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false };
  assert.equal(followsInPage({ href: '/terminal' }, plain, origin), true);
  assert.equal(followsInPage({ href: 'https://duke-nas:9090/help/errors/ADM-123' }, plain, origin), true);
  assert.equal(followsInPage({ href: '/terminal#keys' }, plain, origin), true);
  assert.equal(followsInPage({ href: 'https://example.org/' }, plain, origin), false);
  assert.equal(followsInPage({ href: '/help/errors/index.json' }, plain, origin), false, 'a file');
  assert.equal(followsInPage({ href: '/mark-dark.svg' }, plain, origin), false);
  assert.equal(followsInPage({ href: '/help/certificates.md' }, plain, origin), false, "help's file");
  for (const page of ['/websites/dukovo.local', '/websites/shop.example.md', '/certificates/site.example.org', '/apps/org.postgresql.postgresql']) {
    assert.equal(followsInPage({ href: page }, plain, origin), true, `${page}: a page whose path ends in a dotted name`);
  }
  assert.equal(followsInPage({ href: '/terminal', target: '_blank' }, plain, origin), false);
  assert.equal(followsInPage({ href: '/terminal', download: true }, plain, origin), false);
  assert.equal(followsInPage({ href: '/terminal' }, { ...plain, ctrlKey: true }, origin), false);
  assert.equal(followsInPage({ href: '/terminal' }, { ...plain, metaKey: true }, origin), false);
  assert.equal(followsInPage({ href: '/terminal' }, { ...plain, shiftKey: true }, origin), false);
  assert.equal(followsInPage({ href: '/terminal' }, { ...plain, altKey: true }, origin), false);
  assert.equal(followsInPage({ href: '/terminal' }, { ...plain, button: 1 }, origin), false);
  assert.equal(followsInPage({ href: 'http://[bad' }, plain, origin), false);
  // A link whose own handler took the click, as Change… while locked.
  assert.equal(followsInPage({ href: '/networking/net0' }, { ...plain, defaultPrevented: true }, origin), false);
});
