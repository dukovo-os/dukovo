// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Hardware page gives the shell: the Overview's things to look at
// and its row, and the palette's rows. Each is read with a fake `cx` that
// answers the reads and counts them.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  needs, needsOf, rows,
} from '../pages/hardware/overview.js';
import * as palette from '../pages/hardware/palette.js';
import { PAGES } from '../pages.js';
import { buildIndex, find } from '../palette/index-model.js';
import {
  broken, clean, last, NOW, show, skipped, time,
} from './hardware-fixtures.mjs';
import { listedActions } from './registry.mjs';

/** A `cx` that answers `GET` of each path in `answers`, and counts the calls. */
function fakeCx(answers, session = { admin: true }) {
  const calls = [];
  const kept = new Map();
  return {
    calls,
    time,
    session,
    api: { get: async (path) => { calls.push(path); if (answers[path] instanceof Error) throw answers[path]; return answers[path]; } },
    cache: (key, ms, make) => {
      if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
      return kept.get(key);
    },
  };
}

const SOON = { end: 'reboot', in_seconds: 5400, at_ms: NOW + 5_400_000 };

test('a failure is a thing that needs you and a warning one worth a look, each with its IDs and what to do', () => {
  const found = needsOf(broken(), null, time);
  assert.deepEqual(found.map((one) => [one.id, one.severity, one.title]), [
    ['hardware-failed', 'bad', 'Hardware check found HW-113, HW-103'],
    ['hardware-warning', 'warn', 'Hardware check found HW-114'],
  ]);
  assert.deepEqual(found[0], {
    id: 'hardware-failed', severity: 'bad', title: 'Hardware check found HW-113, HW-103', detail: 'Replace the disk.', href: '/hardware#check', icon: 'hardware', at: broken().time,
  });
  assert.equal(needsOf(last(), null, time)[0].title, 'Hardware check found HW-115');
});

test('a result with no failure or warning has nothing to look at, and no result has none either', () => {
  assert.deepEqual(needsOf(clean(), null, time), []);
  assert.deepEqual(needsOf(skipped(), null, time), []);
  assert.deepEqual(needsOf(null, null, time), []);
  assert.deepEqual(needsOf(undefined, undefined, time), []);
});

test('a restart or shut down that waits is a thing to look at, in the words of the page\'s own line', () => {
  assert.deepEqual(needsOf(null, SOON, time), [{
    id: 'hardware-power', severity: 'info', title: 'Restarts at 4:42 PM', detail: 'Cancel it from Hardware.', href: '/hardware#power', icon: 'power', at: null,
  }]);
  assert.equal(needsOf(null, { end: 'reboot', in_seconds: 42, at_ms: NOW + 42_000 }, time)[0].title, 'Restarts in 42 seconds');
  assert.equal(needsOf(null, { end: 'poweroff', in_seconds: 1, at_ms: NOW + 1000 }, time)[0].title, 'Shuts down in 1 second');
  assert.equal(needsOf(null, { end: 'poweroff', in_seconds: 0, at_ms: NOW }, time)[0].title, 'Shuts down now');
  assert.equal(needsOf(null, { end: 'reboot' }, time)[0].title, 'Restarts');
});

test('the Overview reads the last check and the restart once each, and a read that fails gives nothing', async () => {
  const cx = fakeCx({ '/api/1/hardware/check': last(), '/api/1/power': { shutdown: SOON } });
  assert.deepEqual((await needs(cx)).map((one) => one.id), ['hardware-warning', 'hardware-power']);
  assert.deepEqual(cx.calls.sort(), ['/api/1/hardware/check', '/api/1/power']);
  const down = fakeCx({ '/api/1/hardware/check': new Error('down'), '/api/1/power': new Error('down') });
  assert.deepEqual(await needs(down), []);
});

test('the row says what the machine is and when it was checked', async () => {
  const cx = fakeCx({ '/api/1/hardware': show(), '/api/1/hardware/check': last() });
  assert.deepEqual(await rows(cx), [{ label: 'Machine', value: 'Dell Inc. OptiPlex 7040 · checked 5 days ago', order: 35 }]);
  const never = fakeCx({ '/api/1/hardware': show(), '/api/1/hardware/check': null });
  assert.equal((await rows(never))[0].value, 'Dell Inc. OptiPlex 7040 · not checked yet');
  const nameless = fakeCx({ '/api/1/hardware': show({ maker: null, model: ' ' }), '/api/1/hardware/check': last() });
  assert.deepEqual(await rows(nameless), []);
  assert.deepEqual(await rows(fakeCx({ '/api/1/hardware': new Error('down'), '/api/1/hardware/check': null })), []);
});

test('the palette has the check, with the state of its last result, the temperatures, and for an administrator the restart and the shut down', () => {
  const items = (answer, admin) => palette.itemsOf(answer, admin);
  assert.deepEqual(items(last(), true).map((one) => [one.key, one.title, one.state, one.href]), [
    ['check', 'Check the hardware', 'warning', '/hardware#check'],
    ['temps', 'Temperatures', undefined, '/hardware#temps'],
    ['restart', 'Restart this machine…', undefined, '/hardware#power'],
    ['shutdown', 'Shut down this machine…', undefined, '/hardware#power'],
  ]);
  assert.deepEqual(items(last(), false).map((one) => one.key), ['check', 'temps']);
  assert.equal(items(broken(), false)[0].state, 'failed');
  assert.equal(items(broken(), false)[0].detail, 'The last check has a failure');
  assert.equal(items(clean(), false)[0].state, 'passed');
  assert.equal(items(skipped(), false)[0].state, null);
  assert.deepEqual([items(null, false)[0].state, items(null, false)[0].detail], [null, 'Not checked yet']);
});

test('the provider reads the last check once and knows who may restart', async () => {
  const [provider] = palette.providers;
  assert.deepEqual([provider.group, provider.kind], ['Hardware', 'hardware']);
  assert.deepEqual(provider.filters.state, ['failed', 'warning', 'passed']);
  const cx = fakeCx({ '/api/1/hardware/check': last() });
  assert.equal((await provider.items(cx)).length, 4);
  assert.equal((await provider.items(fakeCx({ '/api/1/hardware/check': last() }, { admin: false }))).length, 2);
  assert.deepEqual(cx.calls, ['/api/1/hardware/check']);
  assert.equal((await provider.items(fakeCx({ '/api/1/hardware/check': new Error('down') }))).at(0).detail, 'Not checked yet');
});

test('the palette finds the check, the temperatures, the restart and the shut down, and filters by the state of the last check', async () => {
  const [provider] = palette.providers;
  const cx = fakeCx({ '/api/1/hardware/check': broken() });
  const collected = [{
    entry: PAGES.find((one) => one.id === 'hardware'),
    value: { phrases: palette.phrases ?? {}, groups: [{ ...provider, items: await provider.items(cx) }] },
  }];
  const made = buildIndex({ pages: PAGES, actions: listedActions(), collected, session: { admin: true } });
  const titles = (query) => find(made, query).map((one) => one.row.title);
  assert.deepEqual(titles('state:failed'), ['Check the hardware']);
  assert.ok(titles('temperatures').includes('Temperatures'));
  assert.ok(titles('restart this').includes('Restart this machine…'));
  assert.ok(titles('shut down').includes('Shut down this machine…'));
  const row = find(made, 'restart this').find((one) => one.row.title === 'Restart this machine…').row;
  assert.deepEqual(row.target, { path: '/hardware#power' });
});
