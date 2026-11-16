// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Report a problem page: the reads,
// the drafts a fake reportd keeps and cuts, what Send and the phone link
// answer, and a record of what was asked, so that a test can press a button
// and see which call it made. Not a test itself, and never installed.

import { ApiError } from '../api.js';
import { makeTime } from '../lib/time.js';
import { Element, usePage } from './fake-dom.mjs';
import {
  actions, crashes, failed, list, makeDesk, NOW, phone, sendAnswer, services, status,
} from './report-fixtures.mjs';

/** Lets what is waiting on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** What the reads answer when a test does not say otherwise. */
const reads = () => ({
  '/api/1/reports/crashes': crashes(),
  '/api/1/updates/failed': failed(),
  '/api/1/services': services(),
  '/api/1/actions': { actions: actions() },
  '/api/1/reports': list(),
  '/api/1/reports/status': status(),
});

/** The text of each element of `nodes`. */
export const words = (nodes) => nodes.map((node) => node.textContent);

/**
 * Opens the Report a problem page on a fresh document and gives what a test
 * needs to look at it and press it. `query` is the address's query; `session`
 * who looks; `world` replaces what a read answers (`reads`: a value, a function
 * of the path, or an error to raise) and what an action answers (`answers`);
 * `desk` is the fake reportd.
 */
export async function openReport(context, {
  query = '', session = {}, world = {}, prefs = {}, desk = makeDesk(),
} = {}) {
  const page = usePage(context);
  const documentListeners = {};
  page.document.addEventListener = (type, listener) => { (documentListeners[type] ??= []).push(listener); };
  // The fake elements have no style; the button that follows a selection sets one.
  Object.defineProperty(Element.prototype, 'style', { get() { return (this.styles ??= {}); }, configurable: true });
  context.after(() => { delete Element.prototype.style; });
  context.mock.method(Date, 'now', () => NOW);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const answers = { ...reads(), ...world.reads };
  const calls = [];
  const ran = [];
  const posted = [];
  const problems = [];
  const inlines = [];
  const notices = [];
  const answer = (path) => {
    calls.push(path);
    const value = answers[path];
    if (value === undefined) throw refusal('ADM-123', 'The command failed', 422, { command: { status: 4 } });
    if (value instanceof Error) throw value;
    return typeof value === 'function' ? value(path) : value;
  };
  const post = async (path, body) => {
    posted.push([path, body]);
    const phoneAt = /^\/api\/1\/reports\/drafts\/([^/]+)\/phone$/.exec(path);
    const redactAt = /^\/api\/1\/reports\/drafts\/([^/]+)\/redact$/.exec(path);
    const given = world.posts?.[path] ?? (phoneAt && world.posts?.phone);
    if (given instanceof Error) throw given;
    if (typeof given === 'function') return given(body);
    if (given != null) return given;
    try {
      if (path === '/api/1/reports/drafts') return desk.draft(body);
      if (redactAt) return desk.redact(redactAt[1], body);
      if (phoneAt) return phone();
    } catch (error) {
      throw error.id ? refusal(error.id, error.message) : error;
    }
    throw refusal('ADM-123', 'The command failed', 422);
  };
  const view = {
    main: page.document.createElement('div'),
    signal: controller.signal,
    session: { name: 'chase', admin: true, terminal: true, ...session },
    time: makeTime('UTC', () => NOW),
    query: new URLSearchParams(query),
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'hidden', ...prefs,
    },
    head: (options) => { view.headed = options; },
    onPrefs: (listener) => { view.prefsListener = listener; },
    problem: (error, where) => problems.push([error, where]),
    notice: (text, options) => notices.push([text, options]),
    api: {
      get: async (path) => answer(path),
      post,
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = world.answers?.[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        if (typeof value === 'function') return value(args);
        return value ?? { ok: true, result: action === 'reports.send' ? sendAnswer() : { done: true, changed: true, output: [] } };
      },
      inline: (action, args) => {
        inlines.push([action, args]);
        return page.document.createElement('code');
      },
    },
  };
  page.document.body.append(view.main);
  const { show } = await import('../pages/report/page.js');
  await show(view);
  await settle();
  const main = view.main;
  return {
    ...page,
    view,
    desk,
    calls,
    posted,
    problems,
    inlines,
    notices,
    main,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    drafts: () => posted.filter(([path]) => path === '/api/1/reports/drafts').map(([, body]) => body),
    redacts: () => posted.filter(([path]) => path.endsWith('/redact')).map(([, body]) => JSON.parse(body.edits)),
    fireDocument: (type) => (documentListeners[type] ?? []).forEach((listener) => listener({ type })),
    /** Sets what a read answers from now on. */
    answers: (more) => Object.assign(answers, more),
    dialog: () => page.document.body.querySelector('dialog'),
    /** Presses `node` and lets what it began end. */
    press: async (node) => {
      node.click();
      await settle();
    },
    /** Chooses the radio card whose value is `key`. */
    choose: async (key) => {
      const radio = main.querySelectorAll('input').find((one) => one.getAttribute('type') === 'radio' && one.getAttribute('value') === key);
      radio.checked = true;
      radio.fire('change');
      await settle();
    },
    /** Types `text` in the field `name` and leaves it, as a person does. */
    write: async (name, text, { leave = true } = {}) => {
      const input = main.querySelectorAll('textarea').find((one) => one.id.endsWith(`-${name}`));
      input.value = text;
      input.fire('input');
      if (leave) {
        input.fire('change');
        await settle();
      }
    },
    /** The button of `main` named `label`. */
    button: (label, root = main) => root.querySelectorAll('button').find((one) => one.textContent === label) ?? null,
    /** The rows of the text: `{n, text, node}`. */
    rows: () => main.querySelectorAll('.rl').map((node) => ({ n: Number(node.getAttribute('data-n')), text: node.querySelector('.rl-text').textContent, node })),
    /** The text as the page shows it, joined by lines. */
    shown: () => main.querySelectorAll('.rl').map((node) => node.querySelector('.rl-text').textContent).join('\n'),
    leave: () => controller.abort(),
  };
}
