// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the AI clients page: what each
// read gives, what each post and action answers, and a record of what was
// asked, so that a test can press a button and see which call it made. Not
// a test itself, and never installed.

import { setUnlockSeconds } from '../unlock.js';
import {
  about, areas, every, history, machine, NOW, own, requests, time,
} from './clients-fixtures.mjs';
import {
  buttonOf, refusal, settle, usePageWithProperties,
} from './security-view.mjs';

export { buttonOf, refusal, settle };

/** What the reads answer when a test does not say otherwise: one request that waits, three clients, a history of every kind. */
export const reads = () => ({
  '/api/1/clients/areas': areas(),
  '/api/1/clients/requests': requests(),
  '/api/1/clients/history': history(),
  '/api/1/account/clients': own(),
  '/api/1/console/clients': every(),
  '/api/1/console/about': about(),
  '/api/1/certificates/this-machine': machine(),
});

/** What the posts answer when a test does not say otherwise. */
const posts = () => ({
  '/api/1/check': { changed: true, now: null },
  '/api/1/account/clients/add': { token: null },
  '/api/1/account/clients/remove': { done: true },
  '/api/1/clients/requests/12/approve': { done: true, changed: true, output: [] },
  '/api/1/clients/requests/12/refuse': { done: true },
});

/** Every attribute and text under `node`, for what a secret must not be left in. */
export const holdings = (node) => [node, ...node.descendants].flatMap((one) => [...one.attributes.values(), one.textContent]);

/** The text of `root`, with its white space as one space. */
export const text = (root) => root.textContent.replace(/\s+/g, ' ').trim();

/**
 * Opens the AI clients page on a fresh document. `world.reads` replaces
 * what a read answers and `world.posts` what a post does (a value, a
 * function of the path or of the body, or an error to raise);
 * `world.answers` what an action answers. `session` is who looks,
 * `unlocked` whether the console is unlocked, `host` the name the browser
 * reaches the console by, and `entry` the function of the page to call.
 */
export async function openClients(context, {
  session = {}, unlocked = true, world = {}, host = 'duke-nas.local', prefs = {}, entry = 'show',
} = {}) {
  const page = usePageWithProperties(context);
  context.mock.method(Date, 'now', () => NOW);
  globalThis.matchMedia = () => ({ matches: false });
  globalThis.location = {
    hostname: host, port: '9090', origin: `https://${host}:9090`, href: `https://${host}:9090/clients`,
  };
  // The form dialog hides its errors while they are empty; nothing here looks at that.
  const realObserver = globalThis.MutationObserver;
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
  context.after(() => {
    delete globalThis.matchMedia;
    delete globalThis.location;
    if (realObserver === undefined) delete globalThis.MutationObserver;
    else globalThis.MutationObserver = realObserver;
    setUnlockSeconds(null);
  });
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const who = {
    name: 'chase', admin: true, kinds: ['password'], terminal: true, from: '192.168.1.25', ...session,
  };
  const found = { ...reads(), ...world.reads };
  const answered = { ...posts(), ...world.posts };
  const calls = [];
  const posted = [];
  const problems = [];
  const notices = [];
  const listeners = {};
  const timers = [];
  const ran = [];
  const tasks = [];
  const followers = [];
  const view = {
    main: page.document.createElement('main'),
    signal: controller.signal,
    session: who,
    time,
    host: 'duke-nas',
    path: '/clients',
    rest: '',
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'shown', ...prefs,
    },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    onPrefs: (listener) => followers.push(listener),
    every: (ms, fn) => timers.push([ms, fn]),
    go: () => {},
    openTask: (id) => tasks.push(id),
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where) => {
      problems.push([error, where]);
      where?.slot?.replaceChildren(`${error.title ?? ''} ${error.detail ?? ''} ${error.id ?? ''}`);
    },
    api: {
      get: async (path) => {
        calls.push(path);
        const value = found[path];
        if (value === undefined) throw refusal('ADM-135', `no answer for ${path}`, 404);
        if (value instanceof Error) throw value;
        return typeof value === 'function' ? value(path) : value;
      },
      post: async (path, body) => {
        // What was sent, as it was when the call was made.
        posted.push([path, JSON.parse(JSON.stringify(body))]);
        const value = answered[path];
        if (value === undefined) throw refusal('ADM-135', `no answer for ${path}`, 404);
        if (value instanceof Error) throw value;
        return typeof value === 'function' ? value(body) : value;
      },
      run: async (action, args, options = {}) => {
        ran.push([action, JSON.parse(JSON.stringify(args)), options]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return value ?? { ok: true, result: { done: true, changed: true, output: [] } };
      },
      panel: (action) => {
        const element = page.document.createElement('div');
        element.setAttribute('data-panel', action);
        return element;
      },
      inline: (action) => {
        const element = page.document.createElement('span');
        element.setAttribute('data-inline', action);
        return element;
      },
    },
  };
  page.document.body.append(view.main);
  const module = await import('../pages/clients/page.js');
  await module[entry](view);
  await settle();
  const dialog = () => page.document.body.querySelectorAll('dialog').filter((one) => one.open).at(-1) ?? null;
  return {
    ...page,
    view,
    calls,
    problems,
    notices,
    tasks,
    timers,
    reads: found,
    answered,
    /** The bodies posted to `path`. */
    posted: (path) => posted.filter((call) => call[0] === path).map((call) => call[1]),
    /** Every path posted to, in order. */
    allPosted: () => posted.map((call) => call[0]),
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    emit: async (event, data) => {
      for (const listener of listeners[event] ?? []) await listener(data);
      await settle();
    },
    tick: async (ms) => {
      for (const [each, fn] of timers) if (each === ms) await fn();
      await settle();
    },
    /** The person turns Show commands on or off. */
    setCommands: (commands) => {
      view.prefs = { ...view.prefs, commands };
      for (const follower of followers) follower(view.prefs);
    },
    /** Locks or unlocks the console, as the stream's `unlock` event does. */
    setUnlocked: (on) => setUnlockSeconds(on ? 900 : null),
    leave: () => controller.abort(),
    card: (id) => view.main.querySelector(`#${id}`),
    dialog,
    press: async (root, words) => {
      const one = buttonOf(root, words);
      if (!one) throw new Error(`no button "${words}" in ${text(root).slice(0, 300)}`);
      one.click();
      await settle();
    },
    /** Types `words` in `input`, as a person does: the form hears it. */
    type: async (input, words) => {
      input.value = words;
      input.closest('form').fire('input');
      await settle();
    },
    /** Chooses the radio of `root` whose value is `value`. */
    choose: async (root, value) => {
      const radios = root.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
      for (const one of radios) one.checked = one.getAttribute('value') === value;
      root.querySelector('form').fire('change');
      await settle();
    },
    /** Chooses the option of `select` whose value is `value`. */
    pick: async (select, value) => {
      select.value = value;
      select.closest('form').fire('change');
      await settle();
    },
  };
}
