// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages.js: every folder of pages/ is listed and every entry is well formed
// (design 3.2 and 17): the paths have no dot, the navigation orders are
// unique, and what an entry loads is there.

import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import test from 'node:test';

import { PAGES } from '../pages.js';
import { matchPage } from '../shell/route-model.js';

const ROOT = new URL('../', import.meta.url);
const PART_NAMES = ['overview', 'palette', 'badge'];

const folders = readdirSync(new URL('pages/', ROOT)).filter((name) => statSync(new URL(`pages/${name}`, ROOT)).isDirectory());

test('every folder of pages/ has a line in pages.js, and every line has a folder', () => {
  assert.deepEqual(PAGES.map((entry) => entry.id).sort(), folders.sort());
});

test('every entry is well formed', () => {
  for (const entry of PAGES) {
    const where = `entry ${entry.id}`;
    assert.match(entry.id, /^[a-z][a-z0-9-]*$/, where);
    assert.ok(entry.title && typeof entry.title === 'string', `${where}: a title`);
    assert.match(entry.path, /^\/[^.#?]*$/, `${where}: a path with no dot`);
    assert.ok(entry.nav === null || Number.isInteger(entry.nav.order), `${where}: nav is null or has an order`);
    assert.ok(entry.phone === null || [1, 2, 3, 4].includes(entry.phone), `${where}: phone is null or 1 to 4`);
    assert.ok(['person', 'admin'].includes(entry.needs), `${where}: needs`);
    assert.ok(typeof entry.icon === 'string' && entry.icon !== '', `${where}: an icon`);
    assert.ok(Array.isArray(entry.keywords) && entry.keywords.every((word) => typeof word === 'string'), `${where}: keywords`);
    assert.ok(entry.load === null || typeof entry.load === 'function', `${where}: load`);
    assert.ok(entry.under === null || PAGES.some((one) => one.id === entry.under && one.nav), `${where}: under names an entry of the navigation`);
    for (const child of [...entry.children, ...entry.sections]) {
      assert.ok(child.title && child.path.startsWith(entry.path), `${where}: ${child.path} is under the entry`);
      assert.doesNotMatch(child.path.split('#')[0], /\./, `${where}: ${child.path} has no dot`);
    }
    for (const [name, load] of Object.entries(entry.parts)) {
      assert.ok(PART_NAMES.includes(name), `${where}: ${name} is a part the shell calls`);
      assert.equal(typeof load, 'function', `${where}: part ${name}`);
    }
  }
});

test('the paths are one to a page, the root is the Overview\'s and the navigation orders are unique', () => {
  const paths = PAGES.map((entry) => entry.path);
  assert.equal(new Set(paths).size, paths.length);
  assert.deepEqual(PAGES.filter((entry) => entry.path === '/').map((entry) => entry.id), ['overview']);
  const orders = PAGES.filter((entry) => entry.nav).map((entry) => entry.nav.order);
  assert.equal(new Set(orders).size, orders.length);
  assert.deepEqual(
    Object.fromEntries(PAGES.filter((entry) => entry.nav).map((entry) => [entry.id, entry.nav.order])),
    {
      overview: 10, logs: 20, reliability: 30, storage: 40, snapshots: 50, networking: 60, firewall: 70, accounts: 80,
      services: 90, apps: 100, certificates: 110, security: 120, hardware: 130, updates: 140, terminal: 150, report: 1000,
    },
    'the orders of the table at the top of pages.js',
  );
});

test('the phone has Overview, Services, Logs and Alerts in its four places, and Alerts is for administrators and has no row of the navigation', () => {
  assert.deepEqual(
    Object.fromEntries(PAGES.filter((entry) => entry.phone).map((entry) => [entry.phone, entry.id])),
    { 1: 'overview', 2: 'services', 3: 'logs', 4: 'alerts' },
  );
  const alerts = PAGES.find((entry) => entry.id === 'alerts');
  assert.equal(alerts.nav, null);
  assert.equal(alerts.needs, 'admin');
  assert.equal(PAGES.find((entry) => entry.id === 'reliability').phone, null, 'Reliability is in More');
});

test('the pages that were added each list pages.css and a sheet of their own, load their page, and have both files', () => {
  const added = ['apps', 'hardware', 'updates', 'report', 'alerts'];
  for (const id of added) {
    const entry = PAGES.find((one) => one.id === id);
    assert.equal(entry.css.length, 2, `${id}: two sheets`);
    assert.equal(entry.css[0], 'pages.css', `${id}: pages.css first`);
    assert.match(entry.css[1], new RegExp(`^pages/${id}/[a-z-]+\\.css$`), `${id}: a sheet of its folder`);
    assert.equal(typeof entry.load, 'function', `${id}: load`);
    assert.ok(existsSync(new URL(`pages/${id}/page.js`, ROOT)), `${id}: page.js`);
    assert.match(readFileSync(new URL(entry.css[1], ROOT), 'utf8'), /^\/\*\n \* SPDX-FileCopyrightText: .+\n \* SPDX-License-Identifier: .+\n \*\//, `${id}: its sheet has the licence header`);
  }
  assert.deepEqual(Object.keys(PAGES.find((entry) => entry.id === 'certificates').parts), ['overview', 'palette'], 'Certificates keeps the parts it had');
});

test('Add an app, Websites and Databases are pages of their own that the navigation shows as Apps, with the sheets and parts they list', () => {
  const entries = ['add-app', 'websites', 'databases'].map((id) => PAGES.find((one) => one.id === id));
  assert.deepEqual(entries.map((entry) => [entry.title, entry.path, entry.icon]), [
    ['Add an app', '/apps/add', 'apps'], ['Websites', '/websites', 'globe'], ['Databases', '/databases', 'db'],
  ]);
  for (const entry of entries) {
    assert.equal(entry.under, 'apps', `${entry.id}: under`);
    assert.equal(entry.nav, null, `${entry.id}: no place of its own in the navigation`);
    assert.equal(entry.phone, null, `${entry.id}: no tab`);
    assert.equal(entry.needs, 'person', `${entry.id}: a reader may open it`);
    assert.equal(typeof entry.load, 'function', `${entry.id}: load`);
    assert.deepEqual(entry.css.slice(0, 2), ['boards.css', 'pages.css'], `${entry.id}: the sheets of the boards and the pages`);
    assert.equal(entry.css.length, 3, `${entry.id}: and one of its own`);
    assert.ok(existsSync(new URL(entry.css[2], ROOT)), `${entry.id}: its own sheet is a file`);
    assert.ok(existsSync(new URL(`pages/${entry.id}/page.js`, ROOT)), `${entry.id}: page.js`);
  }
  assert.deepEqual(entries.map((entry) => entry.css[2]), ['pages/add-app/add.css', 'pages/websites/web.css', 'pages/databases/db.css']);
  assert.deepEqual(entries.map((entry) => Object.keys(entry.parts)), [[], ['palette'], ['palette']]);
  assert.equal(PAGES.find((entry) => entry.id === 'apps').under, null, 'Apps stands for itself');
});

test('/apps/add and what is under it reach Add an app, and an app\'s page reaches Apps, since a bundle\'s ID has a dot and add has none', () => {
  const reach = (path) => matchPage(PAGES, path);
  assert.deepEqual([reach('/apps/add').entry.id, reach('/apps/add').rest], ['add-app', '']);
  assert.deepEqual([reach('/apps/add/org.example.notes').entry.id, reach('/apps/add/org.example.notes').rest], ['add-app', 'org.example.notes']);
  assert.deepEqual([reach('/apps/org.example.notes').entry.id, reach('/apps/org.example.notes').rest], ['apps', 'org.example.notes']);
  assert.equal(reach('/apps').entry.id, 'apps');
  assert.equal(reach('/websites/add').entry.id, 'websites');
  assert.equal(reach('/websites/shop.example.org').entry.id, 'websites');
  assert.equal(reach('/databases/add').entry.id, 'databases');
});

test('Certificates tries changes, so it lists dialogs.css first and the banner\'s sheet before its own', () => {
  const entry = PAGES.find((one) => one.id === 'certificates');
  assert.deepEqual(entry.css, ['dialogs.css', 'pages.css', 'lib/trial-banner.css', 'pages/certificates/certs.css']);
  assert.equal(typeof entry.load, 'function');
  for (const sheet of entry.css) assert.ok(existsSync(new URL(sheet, ROOT)), sheet);
  assert.match(readFileSync(new URL(entry.css.at(-1), ROOT), 'utf8'), /^\/\*\n \* SPDX-FileCopyrightText: .+\n \* SPDX-License-Identifier: .+\n \*\//);
});

test('the pages that are shown have a page with a show function, and a folder of parts only has none', async () => {
  for (const entry of PAGES) {
    if (entry.load === null) {
      assert.equal(existsSync(new URL(`pages/${entry.id}/page.js`, ROOT)), false, `${entry.id} has parts only`);
    } else {
      assert.equal(typeof (await entry.load()).show, 'function', `${entry.id}: show`);
    }
  }
});

test('the files an entry links exist, unless its page is still a stand-in', () => {
  for (const entry of PAGES) {
    const page = new URL(`pages/${entry.id}/page.js`, ROOT);
    const standIn = existsSync(page) && readFileSync(page, 'utf8').includes('This page is being built.');
    for (const css of entry.css) {
      assert.match(css, /^(?:(?:pages\/[a-z-]+|lib)\/)?[a-z-]+\.css$/);
      assert.ok(standIn || existsSync(new URL(css, ROOT)), `${entry.id}: ${css} is a file`);
    }
  }
});

test('a part that is there gives what the shell calls, and one that is not there is allowed (3.5)', async () => {
  const gives = { overview: ['needs'], palette: ['providers'], badge: ['count'] };
  for (const entry of PAGES) {
    for (const [name, load] of Object.entries(entry.parts)) {
      let part;
      try {
        part = await load();
      } catch (error) {
        if (error?.code === 'ERR_MODULE_NOT_FOUND' || /Cannot find module/.test(String(error?.message))) continue;
        throw error;
      }
      for (const key of gives[name]) {
        assert.ok(key in part, `${entry.id}'s ${name} part gives ${key}`);
      }
    }
  }
});
