// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the page that adds an app: what each
// read gives, what each action answers, the address the page keeps, and a
// record of what was asked, so that a test can type in the search, press a
// button and see which call it made. The install runs through a function that
// stands for lib/steps.js's `runSteps`, which gives the list and a promise of
// how the steps ended, and which a test can make fail. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { runner } from '../pages/add-app/install.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  NOW, plain, services, time,
} from './apps-fixtures.mjs';
import {
  backups, installedList, nginx, OTHER_SHA, php, search,
} from './catalog-fixtures.mjs';
import { usePage } from './fake-dom.mjs';

import { settle as wait } from './apps-view.mjs';

export { wait as settle };

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** The element of `root` that carries `data-action="id"`. */
export const control = (root, id) => root.querySelector(`[data-action="${id}"]`);

/** The words of the cells of each row of `root`'s body, which may be a table. */
export const cellsOf = (root) => root.querySelectorAll('tbody tr').map((row) => row.querySelectorAll('td').map((cell) => cell.textContent));

/** How steps end when all went well: what `runSteps`' `finished` gives. */
const ENDED = Object.freeze({
  done: true, failed: null, answers: [], left: false,
});

/** How the steps end when the step failed: `task` is the task as it ended, or nothing when no task was made. */
export const failedWith = (task) => ({
  done: false, failed: 0, answers: [task], left: false,
});

/** The tests whose globals are put away already. */
const patched = new WeakSet();

/** The address of each read of the page's own: the apps, the search, the sheet, the backups and the services. */
export const PATHS = Object.freeze({
  apps: '/api/1/apps', search: '/api/1/apps/catalog/search', services: '/api/1/services', backups: '/api/1/backups',
});

/** What the reads answer when a test does not say otherwise: nothing but PostgreSQL is installed. */
const reads = () => ({
  [PATHS.apps]: installedList(['org.postgresql.postgresql', 'PostgreSQL', '17.2']),
  [PATHS.search]: search(),
  '/api/1/apps/catalog/org.nginx.nginx/sheet': nginx(),
  '/api/1/apps/catalog/net.php.php/sheet': php(),
  [PATHS.services]: services(),
  [PATHS.backups]: backups('/users', '/shared'),
});

/**
 * Opens the page for `rest` and `query` on a fresh document and gives what the
 * test needs to look at it and press it. `session` is who looks; `world.reads`
 * replaces what a read answers (a value, a function of the path, or an error to
 * raise) and `world.answers` what an action answers. `world.install` is what
 * stands for the install's step: given the step's call and the page's answers,
 * it changes them as the machine would; it defaults to a successful install.
 */
export async function openAdd(context, {
  rest = '', query = '', session = {}, unlocked = true, world = {},
} = {}) {
  const page = usePage(context);
  context.mock.method(Date, 'now', () => NOW);
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  const kept = [];
  const steps = [];
  // A test may open the page more than once: what is put back is what was there before the first.
  if (!patched.has(context)) {
    patched.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
    const had = Object.getOwnPropertyDescriptor(globalThis, 'history');
    const was = runner.steps;
    context.after(() => {
      setUnlockSeconds(null);
      runner.steps = was;
      if (had) Object.defineProperty(globalThis, 'history', had);
      else delete globalThis.history;
    });
  }
  Object.defineProperty(globalThis, 'history', { value: { state: null, replaceState: (...call) => kept.push(call[2]) }, configurable: true });
  context.after(() => controller.abort());
  const who = {
    name: 'chase', admin: true, terminal: true, ...session,
  };
  const answers = { ...reads(), ...world.reads };
  const calls = [];
  const ran = [];
  const problems = [];
  const inlines = [];
  const went = [];
  const listeners = {};
  const answer = (path) => {
    calls.push(path);
    const value = answers[path] ?? answers[path.split('?')[0]];
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
    path: `/apps/add/${rest}`,
    query: new URLSearchParams(query),
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'hidden',
    },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: () => {},
    go: (path) => went.push(path),
    notice: () => {},
    problem: (error, where) => problems.push([error, where]),
    api: {
      get: async (path) => answer(path),
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return typeof value === 'function' ? value(args) : (value ?? { ok: true, result: { done: true, changed: true } });
      },
      panel: () => page.document.createElement('div'),
      inline: (action, args) => {
        inlines.push([action, args]);
        return page.document.createElement('button');
      },
    },
  };
  // The install's one step: by default the machine has the app afterwards, as it would after a task that is done.
  runner.steps = (stepView, list) => {
    steps.push({ view: stepView, steps: list });
    const [step] = list;
    const finished = (async () => {
      if (world.install) return (await world.install(step, answers, stepView)) ?? ENDED;
      const sheet = answers[`/api/1/apps/catalog/${step.args.id}/sheet`];
      answers[`/api/1/apps/${step.args.id}`] = plain(step.args.id, sheet.name, sheet.version, { service_list: sheet.service_list });
      return ENDED;
    })();
    return { element: page.document.createElement('ol'), finished };
  };
  page.document.body.append(view.main);
  const { show } = await import('../pages/add-app/page.js');
  await show(view);
  await wait();
  return {
    ...page,
    view,
    calls,
    problems,
    inlines,
    went,
    steps,
    kept,
    answers,
    listeners,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    emit: async (event, data) => {
      (listeners[event] ?? []).forEach((listener) => listener(data));
      await wait();
    },
    dialog: () => page.document.body.querySelector('dialog'),
    /** Types `text` in the search field as the browser would. */
    type: async (text) => {
      const input = view.main.querySelector('#add-q');
      input.value = text;
      input.fire('input');
      await wait();
    },
    /** Lets `ms` pass, which sends a search that waited for the typing to stop. */
    tick: async (ms) => {
      context.mock.timers.tick(ms);
      await wait();
    },
    /** Presses `node` and lets what it began end. */
    press: async (node) => {
      node.click();
      await wait();
    },
    leave: () => controller.abort(),
  };
}

export { NOW, OTHER_SHA };
