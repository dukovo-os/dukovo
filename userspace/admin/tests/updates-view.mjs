// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Updates page: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import { usePage } from './fake-dom.mjs';
import {
  advisories, bundles, checks, failed, history, idle, last, notices, NOW, search, settings, time,
} from './updates-fixtures.mjs';

/** Lets what waits on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** The button of `root` whose words are `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409) => new ApiError({
  id, title, detail: '', fix: '', status,
});

/** What the reads answer when a test does not say otherwise: a check with an update ready, nothing staged. */
export const reads = () => ({
  '/api/1/updates': last(),
  '/api/1/updates/settings': settings(),
  '/api/1/updates/checks': checks(),
  '/api/1/updates/failed': { failed: null, since_good: [], bisect: { running: false, try: null, of: null } },
  '/api/1/apps': bundles(),
  '/api/1/console/about': { release: 'Dukovo Server 0.14.0' },
  '/api/1/tasks': { tasks: [] },
  '/api/1/updates/slots': idle(),
  '/api/1/updates/advisories': advisories(),
  '/api/1/updates/history': history(),
  '/api/1/updates/bisect': search(),
  '/api/1/console/notices': notices(),
  '/api/1/power': { shutdown: null },
  '/api/1/accounts': [{ name: 'chase', uid: 1000, console: true }],
  '/api/1/services': { services: [{ name: 'nginx' }, { name: 'logd' }, { name: 'postgres' }] },
});

/**
 * Opens the Updates page on a fresh document. `world.reads` replaces what a
 * read answers (a value, a function or an error to raise); `world.answers`
 * what an action answers, by action. `session` is who looks; `unlocked`
 * whether the console is unlocked.
 */
export async function openUpdates(context, {
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
    path: '/updates',
    rest: '',
    query: new URLSearchParams(),
    prefs: { theme: 'dark', zone: 'UTC', commands: 'hidden' },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => { timers.push([ms, fn]); return () => {}; },
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
    },
  };
  page.document.body.append(view.main, view.slot);
  const { show } = await import('../pages/updates/page.js');
  await show(view);
  await settle();
  return {
    ...page,
    view,
    calls,
    problems,
    notes,
    expected,
    listeners,
    timers,
    answers,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
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
    failed,
  };
}
