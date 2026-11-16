// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Apps pages: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  list, last, NOW, plain, services, settings, time, valkey,
} from './apps-fixtures.mjs';
import { usePage } from './fake-dom.mjs';

/** Lets what is waiting on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** The element of `root` that carries `data-action="id"`. */
export const control = (root, id) => root.querySelector(`[data-action="${id}"]`);

/** The words of the cells of each row of `root`'s body, which may be a table. */
export const rowsOf = (root) => root.querySelectorAll('tbody tr').map((row) => row.querySelectorAll('td').map((cell) => cell.textContent));

/** What the reads answer when a test does not say otherwise. */
const reads = () => ({
  '/api/1/apps': list(),
  '/api/1/updates': last(),
  '/api/1/tasks': { tasks: [] },
  '/api/1/services': services(),
  '/api/1/updates/settings': settings(),
  '/api/1/apps/io.valkey.valkey': valkey(),
  '/api/1/apps/io.nginx.nginx': plain('io.nginx.nginx', 'nginx', '1.27.3'),
});

/**
 * Opens the page of Apps for `rest` on a fresh document and gives what the
 * test needs to look at it and press it. `session` is who looks; `unlocked`
 * whether the console is unlocked; `world` replaces what a read answers
 * (`reads`: a value, a function of the path, or an error to raise) and what
 * an action answers (`answers`).
 */
export async function openApps(context, {
  rest = '', session = {}, unlocked = true, world = {}, prefs = {},
} = {}) {
  const page = usePage(context);
  context.mock.method(Date, 'now', () => NOW);
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => {
    controller.abort();
    setUnlockSeconds(null);
  });
  const who = {
    name: 'chase', admin: true, terminal: true, ...session,
  };
  const answers = { ...reads(), ...world.reads };
  const calls = [];
  const ran = [];
  const panels = [];
  const inlines = [];
  const problems = [];
  const notices = [];
  const went = [];
  const opened = [];
  const listeners = {};
  const timers = [];
  const answer = (path) => {
    calls.push(path);
    const value = answers[path];
    if (value === undefined) throw refusal('ADM-123', 'The command failed', 422, { command: { status: 4 } });
    if (value instanceof Error) throw value;
    return typeof value === 'function' ? value(path) : value;
  };
  const view = {
    main: page.document.createElement('div'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    time,
    rest,
    path: `/apps/${rest}`,
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'hidden', ...prefs,
    },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => timers.push([ms, fn]),
    go: (path) => went.push(path),
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where) => problems.push([error, where]),
    openTask: (id) => opened.push(id),
    api: {
      get: async (path) => answer(path),
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return typeof value === 'function' ? value(args) : (value ?? { ok: true, result: { done: true } });
      },
      panel: (action, args) => {
        panels.push([action, args]);
        return page.document.createElement('div');
      },
      inline: (action, args) => {
        inlines.push([action, args]);
        return page.document.createElement('button');
      },
    },
  };
  page.document.body.append(view.main);
  const { show } = await import('../pages/apps/page.js');
  await show(view);
  await settle();
  return {
    ...page,
    view,
    calls,
    panels,
    inlines,
    problems,
    notices,
    went,
    opened,
    listeners,
    timers,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    emit: async (event, data) => {
      (listeners[event] ?? []).forEach((listener) => listener(data));
      await settle();
    },
    /** Sets what a read answers from now on. */
    answers: (more) => Object.assign(answers, more),
    dialog: () => page.document.body.querySelector('dialog'),
    submit: async () => {
      page.document.body.querySelector('dialog').querySelector('form').fire('submit');
      await settle();
    },
    /** Presses `node` and lets what it began end. */
    press: async (node) => {
      node.click();
      await settle();
    },
    leave: () => controller.abort(),
  };
}
