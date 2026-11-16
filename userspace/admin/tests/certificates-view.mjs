// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Certificates page: what each
// read gives, what each action answers, and a record of what was asked, so
// that a test can press a button and see which call it made. Not a test
// itself, and never installed.

import { setUnlockSeconds } from '../unlock.js';
import {
  about, history, list, machine, NOW, report, show, signIn, time,
} from './certificates-fixtures.mjs';
import { sparkIdle } from './security-fixtures.mjs';
import { list as websites } from './websites-fixtures.mjs';
import {
  buttonOf, refusal, settle, usePageWithProperties,
} from './security-view.mjs';

export { buttonOf, refusal, settle };

/** What the reads answer when a test does not say otherwise: a machine with each kind of certificate, nothing running. */
export const reads = () => ({
  '/api/1/certificates': list(),
  '/api/1/certificates/history/all': history(),
  '/api/1/security/sign-in': signIn(),
  '/api/1/websites': websites(),
  '/api/1/console/about': about(),
  '/api/1/machine/name': machine(),
  '/api/1/tasks': { tasks: [] },
  '/api/1/services': sparkIdle(),
  '/api/1/changes?limit=100': { changes: [] },
  '/api/1/console/fresh/console': { fresh: false },
  '/api/1/console/fresh/firewall': { fresh: true },
  '/api/1/certificates/check/*': () => report(),
  '/api/1/certificates/*': (path) => show({ name: decodeURIComponent(path.split('/').at(-1)) }),
});

/** The tests whose timers are already made by hand, which a second page of the same test must not do again. */
const timed = new WeakSet();

/** Every attribute and text under `node`, for what a secret must not be left in. */
export const holdings = (node) => [node, ...node.descendants].flatMap((one) => [...one.attributes.values(), one.textContent]);

/** The text of `root`, with its white space as one space. */
export const text = (root) => root.textContent.replace(/\s+/g, ' ');

/**
 * Opens the Certificates page on a fresh document. `world.reads` replaces
 * what a read answers (a value, a function of the path, or an error to
 * raise; a key that ends in `*` answers every path that starts so);
 * `world.answers` what an action answers (a result, an error, or a function
 * of its arguments). `session` is who looks, `unlocked` whether the console
 * is unlocked, `host` the address the browser reaches the console by.
 */
export async function openCertificates(context, {
  session = {}, unlocked = true, world = {}, host = '192.168.1.10', hash = '', rest = '', prefs = {},
} = {}) {
  const page = usePageWithProperties(context);
  context.mock.method(Date, 'now', () => NOW);
  globalThis.matchMedia = () => ({ matches: false });
  globalThis.location = {
    hostname: host, port: '9090', hash, href: `https://${host}:9090/certificates`,
  };
  URL.createObjectURL = () => 'blob:saved';
  URL.revokeObjectURL = () => {};
  context.after(() => {
    delete globalThis.matchMedia;
    delete globalThis.location;
    setUnlockSeconds(null);
  });
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => controller.abort());
  // The command under a dialog's fields waits for a pause in the typing; the test makes the pause.
  if (!timed.has(context)) {
    timed.add(context);
    context.mock.timers.enable({ apis: ['setTimeout'] });
  }
  const who = {
    name: 'chase', admin: true, kinds: ['password'], terminal: true, from: '192.168.1.25', via: { address: '192.168.1.10', port: 9090, card: 'net0' }, ...session,
  };
  const found = { ...reads(), ...world.reads };
  const calls = [];
  const panels = [];
  const problems = [];
  const notices = [];
  const listeners = {};
  const timers = [];
  const ran = [];
  const saved = [];
  const answer = (path) => {
    calls.push(path);
    let value = found[path];
    if (value === undefined) {
      const key = Object.keys(found).filter((one) => one.endsWith('*') && path.startsWith(one.slice(0, -1))).sort((one, other) => other.length - one.length)[0];
      value = key === undefined ? undefined : found[key];
    }
    if (value === undefined) throw refusal('ADM-135', `no answer for ${path}`, 404);
    if (value instanceof Error) throw value;
    return typeof value === 'function' ? value(path) : value;
  };
  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    time,
    host: 'duke-nas',
    path: '/certificates',
    rest,
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'shown', ...prefs,
    },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => timers.push([ms, fn]),
    go: () => {},
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where) => {
      problems.push([error, where]);
      where?.slot?.replaceChildren(`${error.title ?? ''} ${error.detail ?? ''} ${error.id ?? ''}`);
    },
    api: {
      get: async (path) => answer(path),
      post: async () => ({}),
      run: async (action, args, options = {}) => {
        // What was sent, as it was when the call was made: a secret in it is let go of after.
        ran.push([action, JSON.parse(JSON.stringify(args)), options, args]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        if (typeof value === 'function') return value(args);
        return value ?? { ok: true, result: { done: true, changed: true, output: [] } };
      },
      panel: (action, args) => {
        panels.push([action, JSON.parse(JSON.stringify(args))]);
        const element = page.document.createElement('div');
        element.setAttribute('data-panel', action);
        return element;
      },
      inline: () => null,
    },
  };
  page.document.body.append(view.main);
  const append = page.document.body.append.bind(page.document.body);
  page.document.body.append = (...nodes) => {
    for (const node of nodes) if (node.tag === 'a' && node.hasAttribute('download')) saved.push(node.getAttribute('download'));
    append(...nodes);
  };
  const { show: showPage } = await import('../pages/certificates/page.js');
  await showPage(view);
  await settle();
  return {
    ...page,
    context,
    view,
    calls,
    panels,
    problems,
    notices,
    listeners,
    timers,
    saved,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    live: (action) => ran.filter((call) => call[0] === action).map((call) => call[3]),
    allRan: () => ran.map((call) => call[0]),
    emit: async (event, data) => {
      for (const listener of listeners[event] ?? []) await listener(data);
      await settle();
    },
    tick: async (ms) => {
      for (const [every, fn] of timers) if (every === ms) await fn();
      await settle();
    },
    reads: found,
    /** Leaves the page, as a click on another one does. */
    leave: () => controller.abort(),
    /** The call the last dialog shows under its fields once the person pauses. */
    shown: () => {
      page.document.body.querySelectorAll('dialog').at(-1).querySelector('form').fire('input');
      context.mock.timers.tick(400);
      return panels.at(-1);
    },
    card: (id) => view.main.querySelector(`#${id}`),
    row: (name) => view.main.querySelector(`tr[data-name="${name}"]`),
    dialog: () => page.document.body.querySelectorAll('dialog').at(-1) ?? null,
    press: async (root, words) => {
      const one = buttonOf(root, words);
      if (!one) throw new Error(`no button "${words}" in ${text(root).slice(0, 300)}`);
      one.click();
      await settle();
    },
    submit: async () => {
      page.document.body.querySelectorAll('dialog').at(-1).querySelector('form').fire('submit');
      await settle();
    },
    /** Chooses the radio of `root` whose value is `value`. */
    choose: (root, value) => {
      const radios = root.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
      for (const one of radios) one.checked = one.getAttribute('value') === value;
      radios.find((one) => one.getAttribute('value') === value).fire('change');
    },
    /** Types `words` in `input` and leaves it, as a person does. */
    type: async (input, words) => {
      input.value = words;
      input.fire('input');
      input.fire('change');
      await settle();
    },
  };
}
