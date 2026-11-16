// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Websites pages: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { errorView } from '../lib/problem.js';
import { setUnlockSeconds } from '../unlock.js';
import { report } from './certificates-fixtures.mjs';
import { sparkIdle } from './security-fixtures.mjs';
import { usePageWithProperties } from './security-view.mjs';
import {
  accounts, backups, certificates, finished, list, machine, open, plan, planPath, sheet, snapshots, time,
} from './websites-fixtures.mjs';

/** Lets what is waiting on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 40; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** The button of `root` that says `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** The link of `root` that says `words`. */
export const linkOf = (root, words) => root.querySelectorAll('a').find((one) => one.textContent.trim() === words);

/** The text of `root`, with its white space as one space. */
export const text = (root) => root.textContent.replace(/\s+/g, ' ');

/** The field of `root` whose label says `words`. */
export const fieldOf = (root, words) => {
  const label = root.querySelectorAll('label').find((one) => one.textContent.trim().startsWith(words));
  const id = label?.getAttribute('for');
  return ['input', 'select', 'textarea'].flatMap((tag) => root.querySelectorAll(tag)).find((one) => one.getAttribute('id') === id) ?? null;
};

/** Types `words` into `input`, as a person does. */
export const type = (input, words) => {
  input.value = words;
  input.fire('input');
};

/** Sets the box `box` on or off, as a press does. */
export const tick = (box, on) => {
  box.checked = on;
  box.fire('change');
};

/** Chooses the radio card of `root` whose title says `words`. */
export const choose = (root, words) => {
  const card = root.querySelectorAll('label.radio-card').find((one) => one.textContent.includes(words));
  if (!card) throw new Error(`no card "${words}"`);
  const input = card.querySelector('input');
  for (const other of root.querySelectorAll(`input[name="${input.getAttribute('name')}"]`)) other.checked = false;
  input.checked = true;
  input.closest('fieldset').fire('change');
  return input;
};

/** The dialogs that are open on `document`. */
export const dialogsOf = (document) => document.body.querySelectorAll('dialog').filter((dialog) => dialog.open);

/**
 * Opens the Websites page for `rest` and `query` on a fresh document and
 * gives what the test needs to look at it and press it. `session` is who
 * looks; `unlocked` whether the console is unlocked; `world` replaces what
 * `GET` answers (`reads`: a value, a function of the path, or an error to
 * raise; a key that ends in `*` answers every path that starts so), what an
 * action answers (`runs`: `{ok, result, task}`, a function of `(args,
 * options)`, or an error) and the list `websites.list` gives (`list`, which
 * a test may change between reads). `steps` replaces what the page reads
 * of the steps' runner.
 */
export async function openWebsites(context, {
  rest = '', query = '', session = {}, unlocked = true, world = {}, prefs = {}, load = true,
} = {}) {
  const page = usePageWithProperties(context);
  // The dialogs watch where their problems go for a change; a test has none to watch.
  const observer = globalThis.MutationObserver;
  globalThis.MutationObserver = class { observe() {} disconnect() {} };
  globalThis.matchMedia = () => ({ matches: false });
  globalThis.location = { hostname: 'duke-nas', port: '9090', hash: '' };
  context.after(() => {
    if (observer === undefined) delete globalThis.MutationObserver;
    else globalThis.MutationObserver = observer;
    delete globalThis.matchMedia;
    delete globalThis.location;
    setUnlockSeconds(null);
  });
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const who = {
    name: 'chase', admin: true, terminal: true, ...session,
  };
  const state = { list: world.list ?? list() };
  const reads = {
    '/api/1/websites': () => state.list,
    '/api/1/certificates': certificates(),
    '/api/1/accounts': accounts(),
    '/api/1/backups': backups(),
    '/api/1/snapshots': snapshots(),
    '/api/1/machine/name': machine(),
    '/api/1/services': sparkIdle(),
    '/api/1/changes?limit=100': { changes: [] },
    '/api/1/console/fresh/firewall': { fresh: true },
    '/api/1/apps/catalog/*': (path) => sheet(decodeURIComponent(path.split('/')[5])),
    '/api/1/certificates/check/*': () => report(),
    '/api/1/tasks/*': (path) => finished(Number(path.split('/').at(-1))),
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
  const path = `/websites${rest === '' ? '' : `/${rest}`}`;
  const cx = { path, time: { full: () => 'now' }, hasPage: () => false };
  const answer = (at) => {
    calls.push(at);
    let value = reads[at];
    if (value === undefined) {
      const key = Object.keys(reads).filter((one) => one.endsWith('*') && at.startsWith(one.slice(0, -1))).sort((a, b) => b.length - a.length)[0];
      value = key === undefined ? undefined : reads[key];
    }
    if (value === undefined) throw refusal('ADM-123', 'The command failed', 422, { command: { status: 4 } });
    if (value instanceof Error) throw value;
    return typeof value === 'function' ? value(at) : value;
  };
  const defaults = {
    'websites.plan': (args) => ({ ok: true, result: plan({ folder: { path: args.folder ?? `/shared/www/sites/${args.name}`, exists: false, entries: 0, owner: null } }) }),
    'websites.list': () => ({ ok: true, result: state.list }),
    'firewall.test': () => ({ ok: true, result: open() }),
  };

  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: who,
    time,
    rest,
    path,
    query: new URLSearchParams(query),
    host: 'duke-nas.example.org',
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
    go: (to) => went.push(to),
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where = {}) => {
      problems.push([error, where]);
      if (where.slot) errorView(error, where, cx, where.slot);
    },
    openTask: (id) => opened.push(id),
    api: {
      get: async (at) => answer(at),
      run: async (action, args, options = {}) => {
        ran.push([action, JSON.parse(JSON.stringify(args ?? {})), options]);
        const way = world.runs?.[action] ?? defaults[action];
        const value = typeof way === 'function' ? await way(args, options) : way;
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        return value ?? { ok: true, result: { done: true, changed: true, output: [] } };
      },
      panel: (action, args) => {
        panels.push([action, JSON.parse(JSON.stringify(args))]);
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
  const opening = {
    ...page,
    view,
    state,
    calls,
    ran,
    problems,
    notices,
    went,
    opened,
    panels,
    inlines,
    listeners,
    timers,
    changed,
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
    /** Opens the page again on the same document, as a new visit does. */
    show: async () => {
      const { show } = await import('../pages/websites/page.js');
      opening.opening = show(view);
      await settle();
      return opening.opening;
    },
  };
  if (load) await opening.show();
  return opening;
}

export { finished, planPath };
