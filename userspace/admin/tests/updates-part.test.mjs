// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Updates page gives the shell: the Overview's things to look at, its
// row and the buttons of the failed start's notice, the navigation's count,
// and the palette's rows. Each is read with a fake `cx` that answers the
// reads and counts them.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach } from 'node:test';

import { count } from '../pages/updates/badge.js';
import {
  needs, needsOf, noticeButtons, rows,
} from '../pages/updates/overview.js';
import * as palette from '../pages/updates/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import {
  check, checkWith, failed, last, NOW, time,
} from './updates-fixtures.mjs';

/** A `cx` that answers `GET` of each path in `answers`, and counts the calls. */
function fakeCx(answers) {
  const calls = [];
  const kept = new Map();
  return {
    calls,
    time,
    api: { get: async (path) => { calls.push(path); if (answers[path] instanceof Error) throw answers[path]; return answers[path]; } },
    cache: (key, ms, make) => {
      if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
      return kept.get(key);
    },
  };
}

const storage = new Map();
const realStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
beforeEach(() => {
  storage.clear();
  Object.defineProperty(globalThis, 'localStorage', {
    value: { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value), removeItem: (key) => storage.delete(key) },
    configurable: true,
  });
});
afterEach(() => {
  if (realStorage) Object.defineProperty(globalThis, 'localStorage', realStorage);
  else delete globalThis.localStorage;
});

const SEEN = 'dukovo-admin.updates.happened.seen';

test('an update that waits is a thing to look at, worse when it fixes a flaw', () => {
  const found = needsOf(last(), NOW);
  assert.deepEqual(found.map((one) => [one.id, one.severity, one.title]), [
    ['update-system', 'warn', '0.16.0 is ready'],
    ['update-apps', 'info', '1 app update is ready'],
    ['update-unchecked', 'warn', 'Updates could not be checked'],
  ]);
  assert.equal(found[0].detail, 'It fixes DKSA-2026-0001, high.');
  assert.equal(found[0].href, '/updates#system');
  assert.equal(found[0].at, Math.floor((NOW - 12 * 60_000) / 1000));
  assert.equal(found[2].detail, 'apps: bundled does not answer');
  const quiet = needsOf(last({ ...checkWith({ system: 'update' }), security: { system: [], app_flaws: 0 } }), NOW);
  assert.deepEqual(quiet.map((one) => [one.severity, one.detail]), [['info', 'Stage it, then restart to finish.']]);
});

test('a release that fell back and is held is a thing to look at, which opens What happened', () => {
  const doc = checkWith({ system: 'held' }, { system: { held_because: 'fell-back', held_at: '2026-11-14T12:12:00Z' } });
  const [one] = needsOf(last(doc), NOW);
  assert.deepEqual([one.id, one.severity, one.title, one.href], ['update-held', 'warn', '0.16.0 was taken back and is held', '/updates#happened']);
  assert.equal(one.at, Math.floor(Date.parse('2026-11-14T12:12:00Z') / 1000));
  assert.deepEqual(needsOf(last(checkWith({ system: 'held' }, { system: { held_because: 'asked' } })), NOW), [], 'a hold that was asked for is not a thing to look at');
});

test('a check that is a week old or more is a thing to look at, and nothing kept is not', () => {
  const old = needsOf(last(checkWith({ system: 'current', firmware: 'current' }), { checked_ms: NOW - 9 * 86_400_000 }), NOW);
  assert.deepEqual(old.map((one) => [one.id, one.title]), [['update-stale', 'Updates were last checked 9 days ago']]);
  assert.deepEqual(needsOf(last(checkWith({ system: 'current', firmware: 'current' }), { checked_ms: NOW - 6 * 86_400_000 }), NOW), []);
  assert.deepEqual(needsOf(null, NOW), []);
  assert.deepEqual(needsOf({}, NOW), []);
});

test('the Overview part reads the last check once and says nothing when it cannot', async () => {
  const cx = fakeCx({ '/api/1/updates': last(), '/api/1/updates/failed': failed() });
  assert.equal((await needs(cx)).length, 3);
  assert.deepEqual(cx.calls, ['/api/1/updates']);
  assert.deepEqual(await needs(fakeCx({ '/api/1/updates': new Error('no') })), []);
});

test('a new failed start lets go of the one that was seen', async () => {
  storage.set(SEEN, '41');
  await needs(fakeCx({ '/api/1/updates': last(), '/api/1/updates/failed': failed() }));
  assert.equal(storage.has(SEEN), false);
  storage.set(SEEN, '42');
  await needs(fakeCx({ '/api/1/updates': last(), '/api/1/updates/failed': failed() }));
  assert.equal(storage.get(SEEN), '42', 'the same start stays seen');
});

test('the row says when the check was and what is ready', async () => {
  assert.deepEqual(await rows(fakeCx({ '/api/1/updates': last() })), [{ label: 'Updates', value: 'Checked 12 minutes ago, 3 ready', order: 25 }]);
  assert.equal((await rows(fakeCx({ '/api/1/updates': last(checkWith({ system: 'current', firmware: 'current' })) })))[0].value, 'Checked 12 minutes ago');
  assert.deepEqual(await rows(fakeCx({ '/api/1/updates': null })), []);
  assert.deepEqual(await rows(fakeCx({ '/api/1/updates': new Error('no') })), []);
});

test('the failed start\'s notice gets What happened and Report it, and says "See it again" once it was opened', () => {
  assert.deepEqual(noticeButtons('failed-start'), [
    { label: 'What happened', href: '/updates#happened' },
    { label: 'Report it', href: '/report?about=failure' },
  ]);
  storage.set(SEEN, '42');
  assert.equal(noticeButtons('failed-start')[0].label, 'See it again');
  assert.deepEqual(noticeButtons('another'), []);
});

test('the navigation counts the parts with an update ready, and nothing when none is', async () => {
  assert.deepEqual(await count(fakeCx({ '/api/1/updates': last() })), { count: 3, label: 'ready', kind: 'info' });
  assert.equal(await count(fakeCx({ '/api/1/updates': last(checkWith({ system: 'current', firmware: 'held' })) })), null);
  assert.equal(await count(fakeCx({ '/api/1/updates': null })), null);
  assert.equal(await count(fakeCx({ '/api/1/updates': new Error('no') })), null);
});

test('the palette finds the system and the firmware set, and to stage, hold or release them', async () => {
  const cx = fakeCx({ '/api/1/updates': last(check()) });
  const [provider] = palette.providers;
  assert.deepEqual([provider.group, provider.kind, provider.filters.state[0]], ['Updates', 'update', 'update']);
  const items = await provider.items(cx);
  assert.deepEqual(items, [
    {
      key: 'system', title: 'System 0.14.0', detail: '0.16.0 is ready', state: 'update', href: '/updates#system', action: 'updates.apply', args: { item: 'system' },
    },
    {
      key: 'firmware', title: 'Firmware set 2026.2', detail: '2026.3 is ready', state: 'update', href: '/updates#firmware', action: 'updates.apply', args: { item: 'firmware' },
    },
  ]);
  assert.deepEqual(palette.itemsOf(null), []);
  assert.equal(palette.itemsOf(last(checkWith({ system: 'current' }))).at(0).detail, 'Up to date');
  const actions = ['updates.apply', 'updates.hold', 'updates.unhold'].map((id) => ({
    id, area: 'updates', title: id, runs: 'as-administrator', changes: true,
  }));
  const made = buildIndex({
    pages: PAGES,
    actions,
    collected: [{ entry: PAGES.find((one) => one.id === 'updates'), value: { phrases: palette.phrases, groups: [{ ...provider, items }] } }],
    session: { admin: true },
  });
  const titles = (query) => find(made, query).map((one) => one.row.title);
  assert.equal(titles('stage system')[0], 'Stage the system update');
  assert.ok(titles('hold firmware').includes('Hold the firmware update'));
  assert.ok(titles('release system').includes('Release the hold on the system update'));
  assert.deepEqual(titles('state:update').sort(), ['Firmware set 2026.2', 'System 0.14.0']);
});
