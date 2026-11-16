// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/nav-model.js: what the navigation lists for a session, the badge
// words, the phone's tabs and when badges are read.

import assert from 'node:assert/strict';
import test from 'node:test';

import { PAGES } from '../pages.js';
import { createNav } from '../shell/nav.js';
import {
  badgeName, badgesDue, currentEntry, navEntries, phoneTabs,
} from '../shell/nav-model.js';
import { usePage } from './fake-dom.mjs';

const entry = (id, nav, more = {}) => ({ id, title: id, path: `/${id}`, nav, needs: 'person', phone: null, ...more });
const pages = [
  entry('terminal', { order: 150, flag: 'terminal' }),
  entry('overview', { order: 10 }, { path: '/', phone: 1 }),
  entry('report', { order: 1000, bottom: true }),
  entry('setup', { order: 20 }, { needs: 'admin' }),
  entry('welcome', null),
  entry('services', { order: 90 }, { phone: 2 }),
  entry('logs', { order: 30 }, { phone: 2 }),
];

test('the navigation lists entries by order, and the bottom ones apart', () => {
  const { main, bottom } = navEntries(pages, { admin: true, terminal: true });
  assert.deepEqual(main.map((one) => one.id), ['overview', 'setup', 'logs', 'services', 'terminal']);
  assert.deepEqual(bottom.map((one) => one.id), ['report']);
});

test('an entry for administrators is left out for a reader, and one with a flag needs the flag', () => {
  const { main } = navEntries(pages, { admin: false, terminal: false });
  assert.deepEqual(main.map((one) => one.id), ['overview', 'logs', 'services']);
  assert.equal(navEntries(pages, { admin: false, terminal: true }).main.at(-1).id, 'terminal');
});

test('the entry with no nav is never listed', () => {
  const ids = Object.values(navEntries(pages, { admin: true, terminal: true })).flat().map((one) => one.id);
  assert.equal(ids.includes('welcome'), false);
});

test('the current entry is the page that owns the path', () => {
  assert.equal(currentEntry(pages, '/services/nginx').id, 'services');
  assert.equal(currentEntry(pages, '/').id, 'overview');
  assert.equal(currentEntry(pages, '/nothing'), null);
});

const ADMIN = { admin: true, terminal: true };

/** The navigation of the console's pages for an administrator, drawn on `path`, on a page of the test's own. */
function navAt(context, path) {
  usePage(context);
  const nav = createNav({ pages: PAGES, session: () => ADMIN, host: 'duke-nas', parts: async () => [], openMore: () => {} });
  nav.update(path);
  return nav;
}

/** The links of the side navigation that are marked as the current ones, each as `[title, href, aria-current]`. */
const marked = (nav) => nav.side.querySelectorAll('a')
  .filter((link) => link.classList.contains('cur'))
  .map((link) => [link.textContent, link.getAttribute('href'), link.getAttribute('aria-current')]);

test('the current entry of a page that names an entry in under says so, and only the pages of adding an app, websites and databases do', () => {
  assert.equal(currentEntry(PAGES, '/websites/shop.example.org').under, 'apps');
  assert.equal(currentEntry(PAGES, '/apps').under, null);
  assert.deepEqual(PAGES.filter((one) => one.under).map((one) => [one.id, one.under]), [
    ['add-app', 'apps'], ['websites', 'apps'], ['databases', 'apps'],
  ]);
});

test('Apps is the current entry on the pages of adding an app, the websites and the databases, with aria-current, and nothing else is', (context) => {
  for (const path of ['/apps/add', '/apps/add/org.example.notes', '/websites', '/websites/add', '/websites/shop.example.org', '/databases', '/databases/add']) {
    assert.deepEqual(marked(navAt(context, path)), [['Apps', '/apps', 'page']], path);
  }
});

test('Apps is current on the list and on an app\'s page, which is no page of adding an app', (context) => {
  for (const path of ['/apps', '/apps/org.example.notes']) {
    assert.deepEqual(marked(navAt(context, path)), [['Apps', '/apps', 'page']], path);
  }
  assert.equal(currentEntry(PAGES, '/apps/org.example.notes').id, 'apps');
});

test('a page that names no entry in under marks its own entry, and a child\'s page has the aria-current mark and its entry has none', (context) => {
  assert.deepEqual(marked(navAt(context, '/services/nginx')), [['Services', '/services', 'page']]);
  assert.deepEqual(marked(navAt(context, '/certificates')), [['Certificates', '/certificates', 'page']]);
  assert.deepEqual(marked(navAt(context, '/services/scheduled')), [['Services', '/services', null], ['Scheduled tasks', '/services/scheduled', 'page']]);
});

test('the phone\'s bar marks no tab on the pages that Apps holds, since Apps is not a tab', (context) => {
  for (const path of ['/apps/add', '/websites/x', '/databases/add']) {
    const nav = navAt(context, path);
    assert.deepEqual(nav.tabbar.querySelectorAll('a').filter((link) => link.classList.contains('cur')), [], path);
    assert.deepEqual(nav.tabbar.querySelectorAll('a').map((link) => link.getAttribute('href')), ['/', '/services', '/logs', '/alerts'], path);
  }
});

test('a badge says its words for a screen reader, and a zero or no badge says nothing more', () => {
  assert.equal(badgeName('Services', { count: 1, label: 'failed' }), 'Services, 1 failed');
  assert.equal(badgeName('Updates', { count: 3 }), 'Updates, 3');
  assert.equal(badgeName('Services', { count: 0, label: 'failed' }), 'Services');
  assert.equal(badgeName('Services', null), 'Services');
});

test('the phone has the entries with a place in order, and More holds the rest', () => {
  const { tabs, more } = phoneTabs(pages, { admin: false, terminal: false });
  assert.deepEqual(tabs.map((one) => one.id), ['overview', 'logs']);
  assert.deepEqual(more.map((one) => one.id), ['services', 'report']);
});

test('an entry that has a place and no navigation holds it, and is left out for a session that may not see it', () => {
  const alerts = entry('alerts', null, { needs: 'admin', phone: 4 });
  const listed = [...pages, alerts];
  const admin = phoneTabs(listed, { admin: true, terminal: false });
  assert.deepEqual(admin.tabs.map((one) => one.id), ['overview', 'logs', 'alerts']);
  assert.equal(admin.more.includes(alerts), false, 'More lists the navigation, which does not have it');
  const reader = phoneTabs(listed, { admin: false, terminal: false });
  assert.deepEqual(reader.tabs.map((one) => one.id), ['overview', 'logs']);
});

test('a place that an entry of the navigation claims is not taken by one that is in none', () => {
  const apart = entry('apart', null, { phone: 1 });
  assert.deepEqual(phoneTabs([apart, ...pages], { admin: true, terminal: false }).tabs.map((one) => one.id), ['overview', 'logs']);
});

test('the bar has four places at most, and More after them', () => {
  const many = Array.from({ length: 6 }, (_, number) => entry(`p${number}`, { order: number + 1 }, { phone: number + 1 }));
  assert.deepEqual(phoneTabs(many, { admin: true, terminal: false }).tabs.map((one) => one.id), ['p0', 'p1', 'p2', 'p3']);
});

test('the phone\'s bar of the console is Overview, Services, Logs, Alerts for an administrator and the same without Alerts for a reader', () => {
  const titles = (session) => phoneTabs(PAGES, session).tabs.map((one) => one.title);
  assert.deepEqual(titles({ admin: true, terminal: true }), ['Overview', 'Services', 'Logs', 'Alerts']);
  assert.deepEqual(titles({ admin: false, terminal: false }), ['Overview', 'Services', 'Logs']);
});

test('More holds the pages of the navigation that the bar does not, and Reliability is one of them', () => {
  const more = (session) => phoneTabs(PAGES, session).more.map((one) => one.id);
  assert.ok(more({ admin: true, terminal: true }).includes('reliability'));
  assert.equal(more({ admin: true, terminal: true }).at(-1), 'report');
  assert.equal(more({ admin: false, terminal: false }).includes('alerts'), false);
  for (const id of ['overview', 'services', 'logs']) assert.equal(more({ admin: true, terminal: true }).includes(id), false, id);
});

test('badges are read at the start, then at most every 30 s on events and every 5 min while seen', () => {
  assert.equal(badgesDue({ why: 'start', last: 1000, now: 1001, visible: true }), true);
  assert.equal(badgesDue({ why: 'event', last: null, now: 0, visible: true }), true);
  assert.equal(badgesDue({ why: 'event', last: 0, now: 29_999, visible: true }), false);
  assert.equal(badgesDue({ why: 'event', last: 0, now: 30_000, visible: true }), true);
  assert.equal(badgesDue({ why: 'poll', last: 0, now: 299_999, visible: true }), false);
  assert.equal(badgesDue({ why: 'poll', last: 0, now: 300_000, visible: true }), true);
  assert.equal(badgesDue({ why: 'poll', last: 0, now: 300_000, visible: false }), false);
});
