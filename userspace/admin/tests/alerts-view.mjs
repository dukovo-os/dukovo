// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Alerts page: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  active, emailChannel, history, NOW, ntfyChannel, time,
} from './alerts-fixtures.mjs';
import { usePage } from './fake-dom.mjs';

/** Lets what waits on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 30; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** The button of `root` whose words are `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** What the reads answer when a test does not say otherwise. */
export const reads = () => ({
  '/api/1/alerts': active(),
  '/api/1/alerts/history': history(),
  '/api/1/alerts/channels': [emailChannel(), ntfyChannel()],
  '/api/1/console/about': {
    host: 'duke-nas', addresses: ['10.0.0.5', 'fd00::5'], port: 8443, fingerprint: 'AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99',
  },
  '/api/1/certificates/this-machine': { name: 'this-machine', fingerprint_sha256: 'AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99' },
});

/** A `localStorage` that keeps what it is given in a map. */
export function useStorage(context, kept = {}) {
  const map = new Map(Object.entries(kept));
  const was = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key) => (map.has(key) ? map.get(key) : null), setItem: (key, value) => map.set(key, String(value)), removeItem: (key) => map.delete(key) },
  });
  context.after(() => {
    if (was) Object.defineProperty(globalThis, 'localStorage', was);
    else delete globalThis.localStorage;
  });
  return map;
}

/**
 * Opens the Alerts page on a fresh document. `world.reads` replaces what a
 * read answers (a value, a function or an error to raise); `world.answers`
 * what an action answers, by action; `world.env` is the browser's own
 * objects for push. `session` is who looks; `unlocked` whether the console is
 * unlocked; `query` the address's query.
 */
export async function openAlerts(context, {
  session = {}, unlocked = true, world = {}, query = '', hostname = 'duke-nas.local', browser = null, width = 1280,
} = {}) {
  const page = usePage(context);
  context.mock.method(Date, 'now', () => NOW);
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  const realObserver = globalThis.MutationObserver;
  const realLocation = Object.getOwnPropertyDescriptor(globalThis, 'location');
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
  globalThis.location = { hostname, hash: '' };
  const was = ['innerWidth', 'PushManager', 'Notification', 'addEventListener'].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  const navigatorWas = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  globalThis.innerWidth = width;
  if (browser) {
    Object.defineProperty(globalThis, 'navigator', { value: { ...globalThis.navigator, ...browser.navigator }, configurable: true });
    globalThis.PushManager = browser.PushManager ?? class {};
    globalThis.Notification = browser.Notification;
  }
  const events = [];
  globalThis.addEventListener = (type, listener) => events.push([type, listener]);
  context.after(() => {
    for (const [name, descriptor] of was) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
    if (navigatorWas) Object.defineProperty(globalThis, 'navigator', navigatorWas);
    controller.abort();
    setUnlockSeconds(null);
    if (realObserver === undefined) delete globalThis.MutationObserver;
    else globalThis.MutationObserver = realObserver;
    if (realLocation) Object.defineProperty(globalThis, 'location', realLocation);
    else delete globalThis.location;
  });
  const who = {
    name: 'chase', admin: true, terminal: true, kinds: [], host: 'duke-nas', ...session,
  };
  const answers = { ...reads(), ...world.reads };
  const calls = [];
  const ran = [];
  const problems = [];
  const notes = [];
  const listeners = {};
  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    host: 'duke-nas',
    time,
    path: '/alerts',
    rest: '',
    query: new URLSearchParams(query),
    prefs: { theme: 'dark', zone: 'UTC', commands: 'hidden' },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    go: () => {},
    notice: (words, options) => notes.push([words, options]),
    problem: (error, where) => problems.push([error, where]),
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
        return typeof value === 'function' ? value(args) : (value ?? { ok: true, result: { done: true, changed: true, output: [] } });
      },
      panel: (action, args) => {
        const panel = page.document.createElement('div');
        panel.setAttribute('data-panel', `${action} ${JSON.stringify(args)}`);
        return panel;
      },
      inline: () => page.document.createElement('div'),
    },
  };
  page.document.body.append(view.main, view.slot);
  const { show: showPage } = await import('../pages/alerts/page.js');
  await showPage(view);
  await settle();
  return {
    ...page,
    view,
    calls,
    problems,
    notes,
    listeners,
    events,
    answers,
    leave: () => controller.abort(),
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    emit: async (event, data) => {
      for (const listener of listeners[event] ?? []) await listener(data);
      await settle();
    },
    card: (id) => view.main.querySelector(`#${id}`),
    dialog: () => page.document.body.querySelector('dialog'),
    press: async (root, words) => {
      const one = buttonOf(root, words);
      if (!one) throw new Error(`no button "${words}" in ${root.textContent.slice(0, 200)}`);
      one.click();
      await settle();
    },
  };
}
