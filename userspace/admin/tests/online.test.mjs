// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/online.js as far as a script test sees it: the banner while the
// server is away and while it restarts, with the words a page gave it and
// "Waiting since" under them, and what it says when the server answers
// again after a restart: "Back" for five seconds, or the words of an update
// that was taken back, with the link that goes to what happened. Time is the
// test's: the banner counts it with timers that the test moves.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { h } from '../dom.js';
import { makeTime } from '../lib/time.js';
import { createOnline } from '../shell/online.js';
import { BACK_KEPT_MS, LOOK_MS, MACHINE_QUIET_MS, RESTART_QUIET_MS } from '../shell/online-model.js';
import { usePage } from './fake-dom.mjs';
import { ROOT } from './graph.mjs';

const START = Date.UTC(2026, 10, 14, 2, 4);
const KEPT = 'dukovo-admin.updates.back';
const ABOUT = { release: 'Dukovo Server 1.0.0-alpha.1' };

/** Puts a `localStorage` in place that holds `entries`, for this test only; gives its map. */
function useStorage(context, entries = {}) {
  const storage = new Map(Object.entries(entries));
  const real = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    value: { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value), removeItem: (key) => storage.delete(key) },
    configurable: true,
  });
  context.after(() => {
    if (real) Object.defineProperty(globalThis, 'localStorage', real);
    else delete globalThis.localStorage;
  });
  return storage;
}

/** Lets the calls that wait on each other finish. */
const settle = async () => {
  for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
};

/**
 * The banner on a page with a clock the test moves. `world.up` says whether
 * the server answers, and `world.silent` that a call gets no answer at all;
 * `about` is what `console.about` gives, or a function that throws. Gives
 * the banner's pieces, what `onBack` heard and the paths asked. `again`
 * makes another banner on the same page and clock, as the next session does.
 */
function openOnline(context, { about = () => Promise.resolve(ABOUT), again = false } = {}) {
  const { document } = again ? globalThis : usePage(context);
  if (!again) {
    context.mock.timers.enable({ apis: ['setInterval', 'setTimeout', 'Date'], now: START });
    const was = globalThis.MutationObserver;
    globalThis.MutationObserver = class { observe() {} disconnect() {} };
    context.after(() => {
      if (was === undefined) delete globalThis.MutationObserver;
      else globalThis.MutationObserver = was;
    });
  }
  const region = h('div', {});
  const stale = h('div', {});
  const main = h('main', {});
  document.body.append(region, stale, main);
  const world = { up: false, ended: false, silent: false, backs: [], asked: 0, calls: [] };
  const online = createOnline({
    host: 'duke-nas',
    region,
    stale,
    main,
    get: async (path, { abandon }) => {
      world.calls.push(path);
      if (path === '/api/1/console/about') {
        world.asked += 1;
        return about();
      }
      if (world.silent) {
        // The connection the server had: no answer comes, until the call is given up.
        return new Promise((resolve, reject) => abandon.addEventListener('abort', () => reject(new Error('given up'))));
      }
      if (path === '/api/1/health' && !world.up) throw new Error('no answer');
      if (path === '/api/1/session' && world.ended) throw Object.assign(new Error('ended'), { status: 401 });
      return {};
    },
    time: () => makeTime('UTC'),
    onBack: (restarted) => world.backs.push(restarted),
    onChange: () => {},
  });
  context.after(() => online.stop());
  const banners = () => region.querySelectorAll('.banner');
  return {
    online, region, world, banners, stale,
    words: () => banners().map((one) => one.textContent),
    wait: async (ms) => {
      context.mock.timers.tick(ms);
      await settle();
    },
  };
}

test('a restart says the words it was given, with since when under them, and no Try now', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('duke-nas is restarting. This page comes back by itself when it answers.', { quietMs: MACHINE_QUIET_MS });
  const [banner] = page.banners();
  assert.equal(banner.querySelector('span').textContent, 'duke-nas is restarting. This page comes back by itself when it answers.');
  const since = banner.querySelector('p.muted');
  assert.equal(since.textContent, 'Waiting since 2:04 AM');
  assert.equal(since.hidden, false);
  assert.equal(banner.querySelector('button').hidden, true, 'a server that restarts is not tried at once');
});

test('a restart with no words says the console\'s own, and is quiet for 120 seconds before it says offline', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart();
  assert.match(page.words()[0], /^The console is restarting\./);
  await page.wait(RESTART_QUIET_MS - 1000);
  assert.match(page.words()[0], /^The console is restarting\./);
  await page.wait(2000);
  const [banner] = page.banners();
  assert.match(banner.querySelector('span').textContent, /^duke-nas is not answering\. Trying (again in|now)/);
  assert.equal(banner.querySelector('p.muted').hidden, true, 'nothing is waited for once the quiet time is over');
  assert.equal(banner.querySelector('button').hidden, false);
});

test('a machine\'s restart stays quiet for ten minutes, and its looks do not say that they are trying', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('duke-nas is restarting.', { quietMs: MACHINE_QUIET_MS });
  await page.wait(RESTART_QUIET_MS + 60_000);
  assert.equal(page.banners()[0].querySelector('span').textContent, 'duke-nas is restarting.');
  await page.wait(MACHINE_QUIET_MS - RESTART_QUIET_MS - 60_000 + 1000);
  assert.match(page.banners()[0].querySelector('span').textContent, /^duke-nas is not answering\./);
});

test('when the server answers after a restart with no version, the banner says Back for five seconds and the page is told to load again', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('The console is restarting.');
  page.world.up = true;
  await page.wait(5000);
  assert.deepEqual(page.world.backs, [true]);
  assert.deepEqual(page.words(), ['Back']);
  assert.equal(page.banners()[0].classes.has('ok'), true);
  assert.equal(page.world.asked, 0, 'a restart that is no update reads nothing more');
  await page.wait(4999);
  assert.deepEqual(page.words(), ['Back']);
  await page.wait(1);
  assert.deepEqual(page.words(), []);
});

test('when an update was to bring a release and the server runs it, the banner says Back, and reads console.about once', async (context) => {
  const page = openOnline(context, { about: () => Promise.resolve({ release: 'Dukovo Server 1.0.0-alpha.2' }) });
  page.online.expectRestart('duke-nas is restarting into 1.0.0-alpha.2.', { quietMs: MACHINE_QUIET_MS, version: '1.0.0-alpha.2' });
  page.world.up = true;
  await page.wait(5000);
  assert.deepEqual(page.words(), ['Back']);
  assert.equal(page.world.asked, 1);
});

test('when the server runs another release than the update was to bring, the banner says it was taken back, with a link to what happened, until it is closed', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('duke-nas is restarting into 1.0.0-alpha.2.', { version: '1.0.0-alpha.2' });
  page.world.up = true;
  await page.wait(5000);
  const [banner] = page.banners();
  assert.equal(banner.classes.has('warn'), true);
  assert.match(banner.textContent, /^Back, on 1\.0\.0-alpha\.1: the update was taken back\. See what happened/);
  assert.equal(banner.querySelector('a').getAttribute('href'), '/updates#happened');
  await page.wait(60_000);
  assert.equal(page.banners().length, 1, 'it stays');
  banner.querySelectorAll('button').find((one) => one.textContent === 'Close').click();
  assert.equal(page.banners().length, 0);
});

test('following the link to what happened closes the banner', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('duke-nas is restarting into 1.0.0-alpha.2.', { version: '1.0.0-alpha.2' });
  page.world.up = true;
  await page.wait(5000);
  page.banners()[0].querySelector('a').click();
  assert.equal(page.banners().length, 0);
});

test('when console.about cannot be read, the banner says Back', async (context) => {
  const page = openOnline(context, { about: () => Promise.reject(new Error('no answer')) });
  page.online.expectRestart('duke-nas is restarting into 1.0.0-alpha.2.', { version: '1.0.0-alpha.2' });
  page.world.up = true;
  await page.wait(5000);
  assert.deepEqual(page.words(), ['Back']);
  assert.deepEqual(page.world.backs, [true]);
});

test('a server that was lost without a restart that a page asked for says nothing when it is back, and the page is not told it restarted', async (context) => {
  const page = openOnline(context);
  page.online.lost();
  assert.match(page.words()[0], /^duke-nas is not answering\./);
  page.world.up = true;
  await page.wait(5000);
  assert.deepEqual(page.world.backs, [false]);
  assert.deepEqual(page.words(), []);
  assert.equal(page.world.asked, 0);
});

test('a restart that is asked for again drops the words of the one before', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('first', { version: '1.0.0-alpha.2' });
  page.world.up = true;
  await page.wait(5000);
  assert.equal(page.banners().length, 1);
  page.world.up = false;
  page.online.expectRestart('second');
  assert.deepEqual(page.banners().map((one) => one.querySelector('span')?.textContent), ['second']);
});

test('the frame hands the words and the options of a restart to the banner, gives it api.js\'s get, and shows the page again when the server is back', () => {
  const frame = readFileSync(`${ROOT}shell/frame.js`, 'utf8');
  assert.match(frame, /async expectRestart\(words, options\) \{\n\s+offline = true;\n\s+\(await begin\(\)\)\.expectRestart\(words, options\);/);
  assert.match(frame, /createOnline\(\{[^}]*\n\s+get,\n/);
  assert.match(frame, /onBack\(restarted\) \{[^}]*if \(restarted\) router\.refresh\(\);\n\s+else bus\.emit\('online'\);/);
});

test('a look that gets no answer is given up after ten seconds, and a later one sees the server back', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('duke-nas is restarting.', { quietMs: MACHINE_QUIET_MS, version: '1.0.0-alpha.2' });
  page.world.silent = true;
  await page.wait(5000);
  assert.deepEqual(page.world.calls, ['/api/1/health']);
  await page.wait(LOOK_MS - 1000);
  assert.deepEqual(page.world.calls, ['/api/1/health'], 'the look still waits');
  await page.wait(1000);
  page.world.silent = false;
  page.world.up = true;
  await page.wait(10_000);
  assert.deepEqual(page.world.calls, ['/api/1/health', '/api/1/health', '/api/1/session', '/api/1/console/about']);
  assert.deepEqual(page.world.backs, [true]);
  assert.match(page.words()[0], /^Back, on 1\.0\.0-alpha\.1: the update was taken back\./);
});

test('a restart that ends the session is over, and the next time the server is lost and back is no restart', async (context) => {
  const page = openOnline(context);
  page.online.expectRestart('The console is restarting.', { version: '1.0.0-alpha.2' });
  page.world.up = true;
  page.world.ended = true;
  await page.wait(5000);
  assert.deepEqual(page.world.backs, [], 'the sign-in page is for api.js\'s handler');
  assert.deepEqual(page.words(), []);
  page.world.ended = false;
  page.online.lost();
  await page.wait(5000);
  assert.deepEqual(page.world.backs, [false]);
  assert.equal(page.world.asked, 0);
});

test('an update\'s restart that ends the session keeps the release it was to bring, and the banner of the next session says what came', async (context) => {
  const storage = useStorage(context);
  const page = openOnline(context);
  page.online.expectRestart('duke-nas is restarting.', { quietMs: MACHINE_QUIET_MS, version: '1.0.0-alpha.2' });
  page.world.up = true;
  page.world.ended = true;
  await page.wait(MACHINE_QUIET_MS + 5000);
  const kept = JSON.parse(storage.get(KEPT));
  assert.equal(kept.version, '1.0.0-alpha.2');
  assert.ok(kept.at > START + MACHINE_QUIET_MS && kept.at <= Date.now(), `kept at ${kept.at}`);
  page.online.stop();

  const next = openOnline(context, { again: true });
  await next.wait(0);
  assert.equal(storage.has(KEPT), false, 'it is said once');
  assert.match(next.words()[0], /^Back, on 1\.0\.0-alpha\.1: the update was taken back\. See what happened/);
});

test('a restart with no release to bring keeps nothing, and a kept release over an hour old is forgotten unsaid', async (context) => {
  const storage = useStorage(context);
  const page = openOnline(context);
  page.online.expectRestart('The console is restarting.');
  page.world.up = true;
  page.world.ended = true;
  await page.wait(RESTART_QUIET_MS + 5000);
  assert.equal(storage.has(KEPT), false);
  page.online.stop();

  storage.set(KEPT, JSON.stringify({ version: '1.0.0-alpha.2', at: START - BACK_KEPT_MS }));
  const later = openOnline(context, { again: true });
  await later.wait(0);
  assert.equal(storage.has(KEPT), false);
  assert.deepEqual(later.words(), []);
  assert.equal(later.world.asked, 0);
});

test('the frame reads the banner once the first page is shown, and a session that a restart ended is the server going away', () => {
  const frame = readFileSync(`${ROOT}shell/frame.js`, 'utf8');
  assert.match(frame, /await router\.start\(\);\n\s+begin\(\);\n/);
  assert.match(frame, /if \(name === 'session' && data\.ended === 'restarted'\) online\.lost\(\);\n\s+else if \(name === 'session'\) onEnded\(data\.ended\);/);
});
