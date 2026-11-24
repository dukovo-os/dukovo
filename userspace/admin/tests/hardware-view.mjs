// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Hardware page: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import { usePage } from './fake-dom.mjs';
import {
  about, devices, diskHealth, highest, history, inventory, last, memtest, NOW, screens, sensors, show, storage, time, updates,
} from './hardware-fixtures.mjs';

/** Lets what waits on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** The button of `root` whose words are `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** Puts a `localStorage` in place that holds `entries` (the name the console keeps it under, then what it keeps), gives its map, and takes it away when the test ends. */
export function useStore(context, entries = {}) {
  const store = new Map(Object.entries(entries).map(([name, value]) => [`dukovo-admin.${name}`, JSON.stringify(value)]));
  Object.defineProperty(globalThis, 'localStorage', {
    value: { getItem: (name) => store.get(name) ?? null, setItem: (name, value) => store.set(name, value), removeItem: (name) => store.delete(name) },
    configurable: true,
  });
  context.after(() => { delete globalThis.localStorage; });
  return store;
}

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409) => new ApiError({
  id, title, detail: '', fix: '', status,
});

/** What the reads answer when a test does not say otherwise: a machine that was checked on Nov 9, nothing running, nothing set. */
export const reads = () => ({
  '/api/1/hardware': show(),
  '/api/1/hardware/inventory': inventory(),
  '/api/1/hardware/check': last(),
  '/api/1/hardware/history': history(),
  '/api/1/console/about': about(),
  '/api/1/storage': storage(),
  '/api/1/hardware/sensors': sensors(),
  '/api/1/hardware/highest': highest(),
  '/api/1/hardware/devices': devices(),
  '/api/1/hardware/screens': screens(),
  '/api/1/hardware/memtest': memtest(),
  '/api/1/updates/history': updates(),
  '/api/1/storage/disks/sda/health': diskHealth('sda'),
  '/api/1/storage/disks/sdb/health': diskHealth('sdb'),
  '/api/1/tasks': { tasks: [] },
  '/api/1/power': { shutdown: null },
  '/api/1/accounts': [{ name: 'chase', uid: 1000, console: true }],
});

/**
 * Opens the Hardware page on a fresh document. `world.reads` replaces what a
 * read answers (a value, a function or an error to raise); `world.answers`
 * what an action answers, by action. `session` is who looks; `unlocked`
 * whether the console is unlocked.
 */
export async function openHardware(context, {
  session = {}, unlocked = true, world = {}, hash = '',
} = {}) {
  const page = usePage(context);
  context.mock.method(Date, 'now', () => NOW);
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => {
    controller.abort();
    setUnlockSeconds(null);
    delete globalThis.location;
  });
  if (hash) globalThis.location = { hash };
  const who = {
    name: 'chase', admin: true, terminal: true, kinds: [], host: 'duke-nas', ...session,
  };
  const answers = { ...reads(), ...world.reads };
  const calls = [];
  const ran = [];
  const inlines = [];
  const problems = [];
  const notes = [];
  const expected = [];
  const listeners = {};
  const timers = [];
  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    host: 'duke-nas',
    time,
    path: '/hardware',
    rest: '',
    query: new URLSearchParams(),
    prefs: { theme: 'dark', zone: 'UTC', commands: 'hidden' },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => timers.push([ms, fn]),
    go: () => {},
    notice: (words, options) => notes.push([words, options]),
    problem: (error, where) => problems.push([error, where]),
    expectRestart: (words, options) => expected.push([words, options]),
    api: {
      get: async (path) => {
        calls.push(path);
        const value = answers[path];
        if (value === undefined) throw refusal('ADM-404', `no answer for ${path}`, 404);
        if (value instanceof Error) throw value;
        return typeof value === 'function' ? value(path) : value;
      },
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return typeof value === 'function' ? value(args) : (value ?? { ok: true, result: { done: true, changed: true } });
      },
      panel: (action, args) => {
        const panel = page.document.createElement('div');
        panel.setAttribute('data-panel', `${action} ${JSON.stringify(args)}`);
        return panel;
      },
      inline: (action, args) => {
        inlines.push([action, args]);
        const element = page.document.createElement('div');
        element.setAttribute('data-inline', action);
        return element;
      },
    },
  };
  page.document.body.append(view.main, view.slot);
  const { show: showPage } = await import('../pages/hardware/page.js');
  await showPage(view);
  await settle();
  return {
    ...page,
    view,
    calls,
    problems,
    notes,
    expected,
    listeners,
    inlines,
    answers,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    /** Lets the minute pass `ms` long, as the shell does for a page that is seen. */
    tick: async (ms) => {
      for (const [every, fn] of timers) if (every === ms) await fn();
      await settle();
    },
    emit: async (event, data) => {
      for (const listener of listeners[event] ?? []) await listener(data);
      await settle();
    },
    card: (id) => view.main.querySelector(`#${id}`),
    dialog: () => page.document.body.querySelector('dialog'),
    submit: async () => {
      page.document.body.querySelector('dialog').querySelector('form').fire('submit');
      await settle();
    },
    press: async (root, words) => {
      const one = buttonOf(root, words);
      if (!one) throw new Error(`no button "${words}" in ${root.textContent.slice(0, 200)}`);
      one.click();
      await settle();
    },
  };
}
