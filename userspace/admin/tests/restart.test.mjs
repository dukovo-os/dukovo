// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/restart.js and lib/pending-restart.js: the questions before a restart
// or a shut down, the actions they run, the line that counts the minute and
// what it tells the shell when the minute ends.

import assert from 'node:assert/strict';
import test, { afterEach, beforeEach, mock } from 'node:test';

import { ApiError } from '../api.js';
import { cancelRestart, restartAt, restartNow, shutDown } from '../lib/restart.js';
import { pendingRestart } from '../lib/pending-restart.js';
import { makeTime } from '../lib/time.js';
import { MACHINE_QUIET_MS } from '../shell/online-model.js';
import { setUnlockSeconds } from '../unlock.js';
import { usePage } from './fake-dom.mjs';

const realPerformance = Object.getOwnPropertyDescriptor(globalThis, 'performance');
const realObserver = globalThis.MutationObserver;
let now;

const settle = async () => {
  for (let at = 0; at < 14; at += 1) await new Promise((resolve) => setImmediate(resolve));
};
// Moves the clock by `seconds`, a second at a time, as the line's timer sees it.
const pass = (seconds) => {
  for (let i = 0; i < seconds; i += 1) {
    now += 1000;
    mock.timers.tick(1000);
  }
};

beforeEach(() => {
  now = 5000;
  Object.defineProperty(globalThis, 'performance', { value: { now: () => now }, configurable: true });
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
  mock.timers.enable({ apis: ['setInterval'] });
});

afterEach(() => {
  setUnlockSeconds(null);
  mock.timers.reset();
  Object.defineProperty(globalThis, 'performance', realPerformance);
  if (realObserver === undefined) delete globalThis.MutationObserver;
  else globalThis.MutationObserver = realObserver;
});

const AT = Date.UTC(2026, 10, 16, 3, 0);
const ACCOUNTS = [{ name: 'chase', console: true }, { name: 'sam', console: true }, { name: 'kid', console: false }];
const SESSIONS = {
  chase: { sessions: [{ this: true }, { this: false }], terminals: [{}] },
  sam: { sessions: [{ this: false }], terminals: [] },
};

/** A view of the shell's, with the reads a restart's questions make and a record of what was run. */
function open(context, { reads = {}, runs = {}, unlocked = true, commands = 'hidden' } = {}) {
  const page = usePage(context);
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const answers = {
    '/api/1/tasks': { tasks: [{ state: 'running', title: 'Back up to a place', args: { place: 'nas' } }, { state: 'done', title: 'Old' }] },
    '/api/1/accounts': ACCOUNTS,
    '/api/1/accounts/chase/sessions': SESSIONS.chase,
    '/api/1/accounts/sam/sessions': SESSIONS.sam,
    '/api/1/power': { shutdown: null },
    ...reads,
  };
  const ran = [];
  const expected = [];
  const notices = [];
  const problems = [];
  const view = {
    host: 'duke-nas',
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: { name: 'chase', admin: true, kinds: [], host: 'duke-nas' },
    time: makeTime('UTC', () => AT - 3_600_000),
    prefs: { commands },
    listeners: {},
    on(event, listener) { (this.listeners[event] ??= []).push(listener); },
    notice: (words, options) => notices.push([words, options]),
    expectRestart: (words, options) => expected.push([words, options]),
    api: {
      get: async (path) => {
        const value = answers[path];
        if (value === undefined) throw new Error(`no answer for ${path}`);
        if (value instanceof Error) throw value;
        return value;
      },
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = runs[action];
        if (value instanceof Error) {
          problems.push([value, options.where]);
          return { ok: false, error: value };
        }
        return { ok: true, result: value ?? {} };
      },
      panel: (action, args) => {
        const panel = page.document.createElement('div');
        panel.setAttribute('data-panel', `${action} ${JSON.stringify(args)}`);
        return panel;
      },
    },
  };
  page.document.body.append(view.main, view.slot);
  return {
    ...page, view, answers, ran, expected, notices, problems,
    dialog: () => page.document.body.querySelector('dialog'),
    submit: async () => {
      page.document.body.querySelector('dialog').querySelector('form').fire('submit');
      await settle();
    },
  };
}

const refusal = (id, title) => new ApiError({ id, title, detail: '', fix: '', status: 409 });

test('Restart now asks first what ends: who is signed in, and what is running', async (context) => {
  const world = open(context);
  const done = restartNow(world.view, { why: 'To finish the update to 1.0.0-alpha.2.', kind: 'update', version: '1.0.0-alpha.2' });
  await settle();
  const dialog = world.dialog();
  assert.ok(dialog, 'the question is open');
  assert.equal(dialog.querySelector('h2').textContent, 'Restart duke-nas?');
  const lead = dialog.textContent;
  assert.match(lead, /^.*To finish the update to 1\.0\.0-alpha\.2\. duke-nas restarts in one minute/s);
  assert.match(lead, /You are signed in with 1 other browser and 1 terminal\./);
  assert.match(lead, /sam is signed in with 1 browser\./);
  assert.doesNotMatch(lead, /kid/, 'an account that cannot use the console is not asked after');
  assert.match(lead, /Back up to a place for nas is running and stops\./);
  assert.doesNotMatch(lead, /Old/, 'a finished task does not end');
  assert.equal(world.ran.length, 0, 'nothing runs before the answer');
  dialog.querySelector('button').click();
  assert.equal(await done, false, 'Cancel sets nothing');
  assert.equal(world.ran.length, 0);
});

test('Restart now runs the restart, then counts the minute and tells the shell when it ends', async (context) => {
  const world = open(context);
  const done = restartNow(world.view, { why: 'To finish the update to 1.0.0-alpha.2.', kind: 'update', version: '1.0.0-alpha.2' });
  await settle();
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: AT } };
  await world.submit();
  assert.equal(await done, true);
  assert.deepEqual(world.ran.map(([action, args]) => [action, args]), [['power.restart', { at: null }]]);
  assert.equal(world.dialog(), null, 'the question closes');
  const line = world.view.slot.querySelector('.banner.info');
  assert.ok(line && !line.hidden, 'the line is shown at the top of the page');
  assert.match(line.textContent, /^duke-nas restarts in 60 seconds/);
  pass(30);
  assert.match(line.textContent, /^duke-nas restarts in 30 seconds/);
  assert.equal(world.expected.length, 0);
  pass(30);
  assert.equal(world.expected.length, 1, 'the shell is told once the minute ends');
  assert.match(world.expected[0][0], /^duke-nas is restarting into 1\.0\.0-alpha\.2\./);
  assert.deepEqual(world.expected[0][1], { quietMs: MACHINE_QUIET_MS, version: '1.0.0-alpha.2' });
  pass(5);
  assert.equal(world.expected.length, 1, 'and not again');
});

test('a restart that is not for an update tells the shell no version', async (context) => {
  const world = open(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: AT } };
  const done = restartNow(world.view, { kind: 'machine', version: '9.9.9' });
  await settle();
  await world.submit();
  await done;
  pass(60);
  assert.match(world.expected[0][0], /^duke-nas is restarting\. This page comes back by itself/);
  assert.equal(world.expected[0][1].version, null);
});

test('a restart that tests the memory first waits as long as the test may take, and says the test can take an hour', async (context) => {
  const world = open(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: AT } };
  const done = restartNow(world.view, { why: 'To test the memory first.', kind: 'memtest', quiet_minutes: 14 });
  await settle();
  assert.match(world.dialog().textContent, /^.*To test the memory first\. duke-nas restarts in one minute/s);
  await world.submit();
  assert.equal(await done, true);
  pass(60);
  assert.equal(world.expected.length, 1);
  assert.match(world.expected[0][0], /^duke-nas is restarting and tests its memory first, which can take up to an hour\./);
  assert.deepEqual(world.expected[0][1], { quietMs: 14 * 60_000, version: null });
});

test('a restart that gives no minutes keeps the usual ten', async (context) => {
  const world = open(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 60, at_ms: AT } };
  const done = restartNow(world.view, { kind: 'memtest' });
  await settle();
  await world.submit();
  await done;
  pass(60);
  assert.equal(world.expected[0][1].quietMs, MACHINE_QUIET_MS);
});

test('a restart that admind refuses is said in the dialog, which stays open', async (context) => {
  const world = open(context, { runs: { 'power.restart': refusal('UPD-106', 'A restart is already set') } });
  const done = restartNow(world.view);
  await settle();
  await world.submit();
  assert.ok(world.dialog(), 'the question stays for the person to read it');
  assert.equal(world.problems.length, 1);
  assert.equal(world.problems[0][0].id, 'UPD-106');
  assert.ok(world.problems[0][1].slot, 'the refusal goes under the fields');
  world.dialog().querySelector('button').click();
  assert.equal(await done, false);
  assert.equal(world.view.slot.querySelector('.banner.info'), null, 'no line for what was not set');
});

test('a locked console asks to unlock first, and a person who does not has nothing run', async (context) => {
  const world = open(context, { unlocked: false });
  const done = restartNow(world.view);
  await settle();
  assert.equal(world.dialog().querySelector('h2').textContent, 'Unlock to make changes');
  assert.match(world.dialog().textContent, /You are about to restart this machine\./);
  world.dialog().querySelectorAll('button').find((one) => one.textContent === 'Cancel').click();
  assert.equal(await done, false);
  assert.equal(world.ran.length, 0);
  assert.equal(world.dialog(), null, 'the restart question is never asked');
});

test('Show command draws the call under the fields only while pages show commands', async (context) => {
  const shown = open(context, { commands: 'shown' });
  restartNow(shown.view);
  await settle();
  assert.equal(shown.dialog().querySelector('[data-panel]').getAttribute('data-panel'), 'power.restart {"at":null}');
  const hidden = open(context);
  restartNow(hidden.view);
  await settle();
  assert.equal(hidden.dialog().querySelector('[data-panel]'), null);
});

test('a restart at a time takes HH:MM and says so when it is not one', async (context) => {
  const world = open(context);
  assert.equal(await restartAt(world.view, '3am'), false);
  assert.equal(world.ran.length, 0);
  assert.match(world.notices[0][0], /HH:MM/);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 3600, at_ms: AT } };
  assert.equal(await restartAt(world.view, '03:00'), true);
  assert.deepEqual(world.ran[0].slice(0, 2), ['power.restart', { at: '03:00' }]);
  const line = world.view.slot.querySelector('.banner.info');
  assert.match(line.textContent, /^duke-nas restarts at 3:00 AM/);
  pass(2);
  assert.equal(world.expected.length, 0, 'an hour away is not counted in seconds');
});

test('a restart is cancelled from its line, which then reads again and goes away', async (context) => {
  const world = open(context);
  world.answers['/api/1/power'] = { shutdown: { end: 'reboot', in_seconds: 3600, at_ms: AT } };
  await restartAt(world.view, '03:00');
  const line = world.view.slot.querySelector('.banner.info');
  world.answers['/api/1/power'] = { shutdown: null };
  line.querySelector('button').click();
  await settle();
  assert.deepEqual(world.ran.at(-1).slice(0, 2), ['power.cancel', {}]);
  assert.equal(line.hidden, true);
  assert.equal(await cancelRestart(world.view), true);
});

test('only an administrator is offered Cancel on the line', async (context) => {
  const world = open(context);
  const own = pendingRestart(world.view);
  assert.equal(own.element.querySelector('.btns').hidden, false);
  world.view.session = { name: 'sam', admin: false, kinds: [], host: 'duke-nas' };
  const reader = pendingRestart(world.view);
  reader.set({ end: 'reboot', in_seconds: 60, at_ms: AT });
  assert.equal(reader.element.hidden, false, 'a reader sees the line');
  assert.equal(reader.element.querySelector('.btns').hidden, true, 'and no button that would be refused');
});

test('the event of a restart set, changed or cancelled keeps the line current', async (context) => {
  const world = open(context);
  const line = pendingRestart(world.view);
  world.view.slot.append(line.element);
  assert.equal(line.element.hidden, true);
  world.view.listeners.power[0]({ shutdown: { end: 'poweroff', in_seconds: 50, at_ms: AT } });
  assert.equal(line.element.hidden, false);
  assert.match(line.element.textContent, /^duke-nas shuts down in 50 seconds/);
  world.view.listeners.power[0]({ shutdown: null });
  assert.equal(line.element.hidden, true);
});

test('the last minute is said once to a screen reader, and the end of it once', async (context) => {
  const world = open(context);
  const line = pendingRestart(world.view);
  world.view.slot.append(line.element);
  line.set({ end: 'reboot', in_seconds: 3, at_ms: AT });
  const status = () => line.element.querySelector('[role="status"]').textContent;
  assert.equal(status(), 'duke-nas restarts in 1 minute.');
  pass(3);
  assert.equal(status(), 'Restarting now.');
});

test('a shut down needs the machine\'s name typed, and then says the machine will not answer', async (context) => {
  const world = open(context);
  const done = shutDown(world.view);
  await settle();
  const dialog = world.dialog();
  assert.equal(dialog.querySelector('h2').textContent, 'Shut down duke-nas?');
  const button = dialog.querySelector('button[type="submit"]');
  const input = dialog.querySelector('input');
  assert.equal(button.disabled, true, 'the button is off until the name is typed');
  input.value = 'duke-na';
  input.fire('input');
  assert.equal(button.disabled, true);
  input.value = ' duke-nas ';
  input.fire('input');
  assert.equal(button.disabled, false);
  world.answers['/api/1/power'] = { shutdown: { end: 'poweroff', in_seconds: 60, at_ms: AT } };
  await world.submit();
  assert.equal(await done, true);
  assert.deepEqual(world.ran[0].slice(0, 2), ['power.shutdown', {}]);
  assert.match(world.view.slot.querySelector('.banner.info').textContent, /^duke-nas shuts down in 60 seconds/);
  pass(60);
  assert.equal(world.expected.length, 0, 'a shut down is not a restart the shell waits out');
  assert.match(world.view.main.textContent, /duke-nas is shutting down\. It will not answer again until someone turns it on\./);
});

test('leaving the page stops the line\'s timer and the open question', async (context) => {
  const world = open(context);
  const done = restartNow(world.view);
  await settle();
  assert.ok(world.dialog());
  world.view.signal.dispatchEvent(new Event('abort'));
  assert.equal(await done, false);
  assert.equal(world.dialog(), null);
});
