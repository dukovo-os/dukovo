// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Storage page, with the RAID part: what each
// read gives, a record of what was asked and of the problems the page raised, so that a test
// can open the page and read its cards. Not a test itself, and never installed.

import { ApiError } from '../api.js';
import { makeTime } from '../lib/time.js';
import { Element, usePage } from './fake-dom.mjs';
import { NOW, show } from './storage-fixtures.mjs';
import { healthy } from './storage-raid-fixtures.mjs';

/** Lets what waits on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 404) => new ApiError({
  id, title, detail: '', fix: '', status,
});

/** The button of `root` whose words are `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** The words of `root` as they are read: each element's words apart from the next's, and no space before a stop or a comma. */
export const text = (root) => {
  const walk = (node) => (node.tag === undefined ? String(node.text ?? '') : node.children.map(walk).join(' '));
  return walk(root).replace(/\s+/g, ' ').replace(/ ([.,:;%])/g, '$1').trim();
};

/** What the reads answer when a test does not say otherwise: the machine of the fixtures and a healthy controller. */
export const reads = () => ({
  '/api/1/storage': show(),
  '/api/1/storage/raid': healthy(),
  '/api/1/storage/usage/1h': [],
  '/api/1/storage/usage/30d': [],
  '/api/1/storage/log': refusal('ADM-404', 'the log is not read here'),
  '/api/1/tasks': { tasks: [] },
});

/**
 * Opens the Storage page on a fresh document. `world.reads` replaces what a read answers (a
 * value, a function or an error to raise); `rest` is the disk's name after /storage/.
 */
export async function openStorage(context, { session = {}, world = {}, rest = '' } = {}) {
  const page = usePage(context);
  // The fake elements have no style; the views set a property on it (the meter's width, the bays' number), which is kept in `values`.
  Object.defineProperty(Element.prototype, 'style', {
    get() { return (this.styleOf ??= { values: new Map(), setProperty(name, value) { this.values.set(name, value); } }); },
    configurable: true,
  });
  context.after(() => { delete Element.prototype.style; });
  context.mock.method(Date, 'now', () => NOW * 1000);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const answers = { ...reads(), ...world.reads };
  const calls = [];
  const problems = [];
  const heads = [];
  const listeners = {};
  const timers = [];
  const dialogs = [];
  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: { name: 'chase', admin: true, terminal: true, kinds: [], host: 'duke-nas', ...session },
    host: 'duke-nas',
    time: makeTime('UTC', () => NOW * 1000),
    path: '/storage',
    rest,
    query: new URLSearchParams(),
    prefs: { theme: 'dark', zone: 'UTC', commands: 'hidden' },
    head: (options) => { heads.push(options); },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => timers.push([ms, fn]),
    go: () => {},
    notice: () => {},
    problem: (error, where) => problems.push([error, where]),
    api: {
      get: async (path) => {
        calls.push(path);
        const value = answers[path];
        if (value === undefined) throw refusal('ADM-404', `no answer for ${path}`);
        if (value instanceof Error) throw value;
        return typeof value === 'function' ? value(path) : value;
      },
      run: async () => ({ ok: true, result: {} }),
      inline: () => page.document.createElement('div'),
    },
  };
  page.document.body.append(view.main, view.slot);
  const { show: showPage } = await import('../pages/storage/page.js');
  await showPage(view);
  await settle();
  return {
    ...page, view, calls, problems, heads, listeners, dialogs, answers,
    head: () => heads[heads.length - 1],
    tick: async (ms) => {
      for (const [every, fn] of timers) if (every === ms) await fn();
      await settle();
    },
    card: (id) => view.main.querySelector(`#${id}`),
    dialog: () => page.document.body.querySelector('dialog'),
  };
}
