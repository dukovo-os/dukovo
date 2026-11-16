// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Accounts pages: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  account, lastlog, NOW, people, peopleAsAPerson, rules, sessionsAnswer, shown, time,
} from './accounts-fixtures.mjs';
import { Element, usePage } from './fake-dom.mjs';

/** Lets what is waiting on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 14; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** The button of `root` whose words are `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** The element of `root` that carries `data-action="id"`. */
export const control = (root, id) => root.querySelector(`[data-action="${id}"]`);

/** Every attribute and text under `node`, for what a secret must not be left in. */
export const holdings = (node) => [node, ...node.descendants].flatMap((one) => [...one.attributes.values(), one.textContent]);

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** What the reads of the accounts answer when a test does not say otherwise. */
const reads = (self) => ({
  '/api/1/accounts': peopleAsAPerson(),
  '/api/1/accounts/list/all': people(),
  '/api/1/sign-ins/last': lastlog(),
  '/api/1/security/sign-in': rules(),
  '/api/1/account/show': shown(self),
  '/api/1/ssh': { passwords: true },
  ...Object.fromEntries(people().map((one) => [`/api/1/accounts/${one.name}`, shown(one.name)])),
  ...Object.fromEntries(people().map((one) => [`/api/1/accounts/${one.name}/sessions`, sessionsAnswer()])),
});

/**
 * Opens a page of Accounts on a fresh document and gives what the test needs
 * to look at it and press it. `route` is `accounts` (the page for `rest`) or
 * `account` (Your account); `session` is who looks; `unlocked` whether the
 * console is unlocked; `world` replaces what a read answers (a value, a
 * function of the path, or an error to raise) and what an action answers.
 */
export async function openAccounts(context, {
  route = 'accounts', rest = '', session = {}, unlocked = false, world = {}, prefs = {},
} = {}) {
  const page = usePage(context);
  // The words of "Today" and "Yesterday" are of the day the fixtures are made on.
  context.mock.method(Date, 'now', () => NOW);
  // A sheet that is linked loads at once, as the console's own do.
  const head = new Element('head', page.document);
  const append = head.append.bind(head);
  head.append = (...nodes) => {
    append(...nodes);
    for (const node of nodes) if (node.tag === 'link') setImmediate(() => node.fire('load'));
  };
  page.document.head = head;
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => {
    controller.abort();
    setUnlockSeconds(null);
  });
  const who = {
    name: 'chase', admin: true, terminal: true, ...session,
  };
  const answers = { ...reads(who.name), ...world.reads };
  const calls = [];
  const posts = [];
  const panels = [];
  const problems = [];
  const notices = [];
  const went = [];
  const listeners = {};
  const timers = [];
  const answer = (path) => {
    calls.push(path);
    const value = answers[path];
    if (value === undefined) throw new Error(`no answer for ${path}`);
    if (value instanceof Error) throw value;
    return typeof value === 'function' ? value(path) : value;
  };
  const ran = [];
  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    time,
    rest,
    path: route === 'account' ? '/account' : `/accounts/${rest}`,
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'hidden', ...prefs,
    },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => timers.push([ms, fn]),
    go: (path) => went.push(path),
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where) => problems.push([error, where]),
    setPrefs: async () => {},
    api: {
      get: async (path) => answer(path),
      post: async (path, body) => {
        posts.push([path, body]);
        const value = world.posts?.[path];
        if (value instanceof Error) throw value;
        return typeof value === 'function' ? value(body) : (value ?? {});
      },
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return typeof value === 'function' ? value(args) : (value ?? { ok: true, result: {} });
      },
      panel: (action, args) => {
        panels.push([action, args]);
        return page.document.createElement('div');
      },
    },
  };
  page.document.body.append(view.main);
  const module = route === 'account' ? '../pages/account/page.js' : '../pages/accounts/page.js';
  const { show } = await import(module);
  await show(view);
  return {
    ...page,
    view,
    calls,
    posts,
    panels,
    problems,
    notices,
    went,
    listeners,
    timers,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    emit: (event, data) => (listeners[event] ?? []).forEach((listener) => listener(data)),
    dialog: () => page.document.body.querySelector('dialog'),
    submit: async () => {
      page.document.body.querySelector('dialog').querySelector('form').fire('submit');
      await settle();
    },
    leave: () => controller.abort(),
    account,
  };
}
