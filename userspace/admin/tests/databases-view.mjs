// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Databases pages: what each
// read gives, what each action answers, and a record of what was asked, so
// that a test can press a button and see which call it made. Not a test
// itself, and never installed.

import { ApiError } from '../api.js';
import { errorView } from '../lib/problem.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  accounts, backups, certificates, list, machine,
} from './databases-fixtures.mjs';
import { Element, usePage } from './fake-dom.mjs';

// A checkbox and a radio button are checked when they have the attribute, as the browser's property says.
Object.defineProperty(Element.prototype, 'checked', {
  configurable: true,
  get() {
    return this.hasAttribute('checked');
  },
  set(on) {
    if (on) this.setAttribute('checked', '');
    else this.removeAttribute('checked');
  },
});

/** Lets what is waiting on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 30; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** The button of `root` that says `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** Every attribute and every text under `node`, for what a secret must not be left in. */
export const holdings = (node) => [node, ...node.descendants].flatMap((one) => [...one.attributes.values(), one.textContent]);

/** The field of `root` whose label says `words`. */
export const fieldOf = (root, words) => {
  const label = root.querySelectorAll('label').find((one) => one.textContent.trim().startsWith(words));
  const id = label?.getAttribute('for');
  return ['input', 'select', 'textarea'].flatMap((tag) => root.querySelectorAll(tag)).find((one) => one.getAttribute('id') === id) ?? null;
};

/** Types `text` into `input`, as a person does. */
export const type = (input, text) => {
  input.value = text;
  input.fire('input');
};

/** Chooses the radio card of `root` whose title says `words`. */
export const choose = (root, words) => {
  const card = root.querySelectorAll('label.radio-card').find((one) => one.textContent.includes(words));
  const input = card.querySelector('input');
  for (const other of root.querySelectorAll(`input[name="${input.getAttribute('name')}"]`)) other.checked = false;
  input.checked = true;
  input.closest('fieldset').fire('change');
  return input;
};

/** The dialogs that are open on `document`. */
export const dialogsOf = (document) => document.body.querySelectorAll('dialog').filter((dialog) => dialog.open);

/**
 * Opens the Databases page for `rest` and `query` on a fresh document and
 * gives what the test needs to look at it and press it. `session` is who
 * looks; `unlocked` whether the console is unlocked; `world` replaces what
 * `GET` answers (`reads`: a value, a function of the path, or an error to
 * raise), what an action answers (`runs`: `{ok, result, task}`, a function
 * of `(args, options)`, or an error) and the list `databases.list` gives
 * (`list`, which a test may change between reads).
 */
export async function openDatabases(context, {
  rest = '', query = '', session = {}, unlocked = true, world = {}, prefs = {}, load = true,
} = {}) {
  const page = usePage(context);
  // The dialogs watch where their problems go for a change; a test has none to watch.
  const observer = globalThis.MutationObserver;
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
  context.after(() => {
    if (observer === undefined) delete globalThis.MutationObserver;
    else globalThis.MutationObserver = observer;
  });
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => {
    controller.abort();
    setUnlockSeconds(null);
  });
  const who = {
    name: 'chase', admin: true, terminal: true, ...session,
  };
  const state = { list: world.list ?? list() };
  const reads = {
    '/api/1/backups': backups(),
    '/api/1/certificates': certificates(),
    '/api/1/accounts': accounts(),
    '/api/1/machine/name': machine(),
    '/api/1/certificates/this-machine': { name: 'this-machine', chain_pem: '-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n' },
    ...world.reads,
  };
  const calls = [];
  const ran = [];
  const problems = [];
  const notices = [];
  const went = [];
  const opened = [];
  const panels = [];
  const inlines = [];
  const listeners = [];
  const timers = [];
  const changed = [];
  const cx = { path: `/databases/${rest}`, time: { full: () => 'now' }, hasPage: () => false };

  const view = {
    main: page.document.createElement('div'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    rest,
    path: `/databases/${rest}`,
    query: new URLSearchParams(query),
    host: 'nas.example.org',
    leaving: null,
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'hidden', ...prefs,
    },
    head: (options) => { view.headed = options; },
    onPrefs: (listener) => changed.push(listener),
    on: (event, listener) => {
      const entry = [event, listener];
      listeners.push(entry);
      return () => listeners.splice(listeners.indexOf(entry), 1);
    },
    every: (ms, fn) => {
      const entry = [ms, fn];
      timers.push(entry);
      return () => timers.splice(timers.indexOf(entry), 1);
    },
    go: (path) => went.push(path),
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where = {}) => {
      problems.push([error, where]);
      if (where.slot) errorView(error, where, cx, where.slot);
    },
    openTask: (id) => opened.push(id),
    api: {
      get: async (path) => {
        calls.push(path);
        const value = reads[path];
        if (value === undefined) throw refusal('ADM-123', 'The command failed', 422, { command: { status: 4 } });
        if (value instanceof Error) throw value;
        return typeof value === 'function' ? value(path) : value;
      },
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const way = world.runs?.[action] ?? (action === 'databases.list' ? () => ({ ok: true, result: state.list }) : undefined);
        const value = typeof way === 'function' ? await way(args, options) : way;
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return value ?? { ok: true, result: { done: true, changed: true, output: [] } };
      },
      panel: (action, args) => {
        panels.push([action, args]);
        const element = page.document.createElement('div');
        element.setAttribute('data-panel', action);
        return element;
      },
      inline: (action, args) => {
        inlines.push([action, args]);
        const element = page.document.createElement('button');
        element.setAttribute('class', 'inline-cmd');
        element.setAttribute('data-inline', action);
        return element;
      },
    },
  };
  page.document.body.append(view.main);
  const world2 = {
    ...page, view, state, calls, ran, problems, notices, went, opened, panels, inlines, listeners, timers, changed,
    emit: async (event, data) => {
      listeners.filter(([name]) => name === event).forEach(([, listener]) => listener(data));
      await settle();
    },
    /** What was run of `action`, as `[args, options]`. */
    ranOf: (action) => ran.filter((call) => call[0] === action).map(([, args, options]) => [args, options]),
    dialog: () => dialogsOf(page.document)[0],
    press: async (node) => {
      node.click();
      await settle();
    },
    submit: async (dialog = dialogsOf(page.document)[0]) => {
      dialog.querySelector('form').fire('submit');
      await settle();
    },
    leave: () => controller.abort(),
    opening: null,
  };
  if (load) {
    const { show } = await import('../pages/databases/page.js');
    world2.opening = show(view);
    await settle();
  }
  return world2;
}
