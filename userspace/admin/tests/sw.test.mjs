// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The service worker, run on fakes of what a worker is given: the notice it
// shows for a push, the page each alert opens, what a press on the notice
// does, and that it has nothing else (no fetch, no cache).

import assert from 'node:assert/strict';
import { readFileSync, statSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

import { pageFor } from '../pages/alerts/model.js';

const FILE = new URL('../sw.js', import.meta.url);
const source = readFileSync(FILE, 'utf8');

/** `value` made of this realm's objects, which the worker's are not. */
const plain = (value) => JSON.parse(JSON.stringify(value));

/** Runs the worker's file with a `self` that records what it is asked. */
function startWorker({ windows = [] } = {}) {
  const listeners = {};
  const shown = [];
  const opened = [];
  const focused = [];
  const self = {
    addEventListener: (type, listener) => { (listeners[type] ??= []).push(listener); },
    registration: { showNotification: async (title, options) => { shown.push([title, options]); } },
    clients: {
      matchAll: async (options) => { self.asked = options; return windows.map((url) => ({ url, focus: async () => { focused.push(url); } })); },
      openWindow: async (path) => { opened.push(path); },
    },
    location: { origin: 'https://duke-nas.example.org' },
  };
  vm.runInNewContext(source, { self, URL });
  return {
    self, listeners, shown, opened, focused,
    async push(message) {
      const waits = [];
      const data = message === undefined ? null : { json: () => (typeof message === 'function' ? message() : message) };
      for (const listener of listeners.push) listener({ data, waitUntil: (promise) => waits.push(promise) });
      await Promise.all(waits);
    },
    async click(path, more = {}) {
      const waits = [];
      const closed = [];
      for (const listener of listeners.notificationclick) {
        listener({ notification: { close: () => closed.push(true), data: path === undefined ? undefined : { path }, ...more }, waitUntil: (promise) => waits.push(promise) });
      }
      await Promise.all(waits);
      return closed.length;
    },
  };
}

const alert = (more = {}, count = 1) => ({
  v: 1,
  host: 'duke-nas',
  count,
  first: {
    state: 'raised', severity: 'critical', title: 'A disk is failing', detail: 'Copy its data off.', source: 'diskhealth', key: 'smart:wd-1', at_ms: 1_763_000_000_000, ...more,
  },
});

test('the worker has a push handler and a click handler, and nothing else', () => {
  const worker = startWorker();
  assert.deepEqual(Object.keys(worker.listeners).sort(), ['notificationclick', 'push']);
  const code = source.replace(/\/\/.*$/gm, '');
  assert.doesNotMatch(code, /\bfetch\b|\bcaches\b|\bCache\b|skipWaiting|importScripts|\bimport\b|\bexport\b/);
});

test('the worker is a small file on its own', () => {
  assert.ok(statSync(FILE).size <= 6000, `${statSync(FILE).size} bytes`);
});

test('a push shows the title with the server\'s name, the first line of the detail and the page it opens', async () => {
  const worker = startWorker();
  await worker.push(alert({ detail: 'Copy its data off.\nThe second line is for the console.' }));
  assert.equal(worker.shown.length, 1);
  const [title, options] = worker.shown[0];
  assert.equal(title, 'duke-nas · A disk is failing');
  assert.deepEqual(plain(options), {
    body: 'Copy its data off.', tag: 'diskhealthsmart:wd-1', renotify: true, data: { path: '/storage' },
  });
});

test('a push of several changes says how many more there are', async () => {
  const worker = startWorker();
  await worker.push(alert({}, 4));
  assert.equal(worker.shown[0][1].body, 'Copy its data off.\nand 3 more');
  await worker.push(alert({ detail: '' }, 2));
  assert.equal(worker.shown[1][1].body, 'and 1 more');
});

test('only a critical alert renotifies, and a cleared one says it cleared', async () => {
  const worker = startWorker();
  await worker.push(alert({ severity: 'warning' }));
  assert.equal(worker.shown[0][1].renotify, false);
  await worker.push(alert({ state: 'cleared', severity: 'warning' }));
  assert.equal(worker.shown[1][0], 'duke-nas · Cleared: A disk is failing');
  await worker.push(alert({ source: undefined, key: undefined }));
  assert.equal(worker.shown[2][1].tag, '');
  assert.equal(worker.shown[2][1].renotify, false, 'renotify needs a tag');
});

test('a push with no readable words still shows a notice that opens Alerts', async () => {
  const worker = startWorker();
  await worker.push(undefined);
  await worker.push(() => { throw new SyntaxError('not JSON'); });
  await worker.push({});
  assert.equal(worker.shown.length, 3);
  for (const [title, options] of worker.shown) {
    assert.equal(title, 'Dukovo Admin · An alert was told');
    assert.equal(options.data.path, '/alerts');
  }
});

test('words that are long or odd are only text', async () => {
  const worker = startWorker();
  await worker.push(alert({ title: '<b>x</b>', detail: 'y'.repeat(900) }));
  assert.equal(worker.shown[0][0], 'duke-nas · <b>x</b>');
  assert.equal(worker.shown[0][1].body.length, 300);
});

const KEYS = [
  ['smart:wd-1', '/storage'], ['smart-rising:wd-1', '/storage'], ['fs-full:/users', '/storage'], ['fs-filling:/shared', '/storage'],
  ['btrfs-missing:data', '/storage'], ['btrfs-behind:data', '/storage'],
  ['backup-failed:off_site', '/snapshots/backups'], ['backup-overdue:off_site', '/snapshots/backups'],
  ['service:web', '/services/web'], ['service:odd name/..', '/services/odd%20name%2F..'], ['service:a\\b', '/services/a%5Cb'],
  ['certificate:duke-nas', '/certificates'], ['temp:coretemp-isa-0000/Core 0', '/hardware'], ['memory-test', '/hardware'],
  ['update-failed', '/updates#happened'], ['update-fell-back', '/updates#happened'], ['update-check-wanted', '/updates#happened'], ['start-failed', '/updates#happened'],
  ['memory:low', '/alerts'], ['kernel-panic', '/alerts'], ['test', '/alerts'], ['', '/alerts'], ['smart', '/alerts'], ['memory-test-x', '/alerts'],
];

test('each alert opens the page it is about', async () => {
  const worker = startWorker();
  for (const [key] of KEYS) await worker.push(alert({ key }));
  assert.deepEqual(worker.shown.map((one) => one[1].data.path), KEYS.map((one) => one[1]));
});

test('the worker and the page agree on the page of each alert', () => {
  for (const [key, path] of KEYS) assert.equal(pageFor(key)?.path ?? '/alerts', path, key);
});

test('a press closes the notice and opens a window on its page', async () => {
  const worker = startWorker();
  assert.equal(await worker.click('/storage'), 1);
  assert.deepEqual(worker.opened, ['/storage']);
  assert.deepEqual(plain(worker.self.asked), { type: 'window', includeUncontrolled: true });
});

test('a press focuses a window that is open on that page, and opens none', async () => {
  const worker = startWorker({ windows: ['https://duke-nas.example.org/logs', 'https://duke-nas.example.org/storage?x=1'] });
  await worker.click('/storage');
  assert.deepEqual(worker.focused, ['https://duke-nas.example.org/storage?x=1']);
  assert.deepEqual(worker.opened, []);
  await worker.click('/updates#happened');
  assert.deepEqual(worker.opened, ['/updates#happened'], 'no window is open on /updates');
});

test('a path is used only when it stays on this site', async () => {
  const worker = startWorker();
  for (const path of ['//evil.example/x', '/\\evil.example', 'https://evil.example/', 'javascript:alert(1)', 'storage', '', undefined, 42, null, {}]) {
    await worker.click(path);
  }
  assert.deepEqual(worker.opened, Array(10).fill('/alerts'));
  await worker.click('/hardware');
  assert.equal(worker.opened.at(-1), '/hardware');
});

test('a press on a notice with no data opens Alerts', async () => {
  const worker = startWorker();
  await worker.click(undefined, { data: undefined });
  assert.deepEqual(worker.opened, ['/alerts']);
});
