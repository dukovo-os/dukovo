// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Firewall page: what each read
// gives, and an address and a history that move as a browser's do when the
// page changes its place. Not a test itself, and never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import { usePage } from './fake-dom.mjs';

/** Lets what waits on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** The rules of a machine with one rule, as `firewall list --json` says them. */
export const listed = () => ({
  rules: [{
    number: 1, text: 'allow tcp 443', action: 'allow', protocol: 'tcp', ports: [443, 443], from: null, on: null, program: null,
  }],
  problems: [],
  trial: null,
});

/** What the reads answer when a test does not say otherwise: one rule, a firewall that runs, no card. */
export const reads = () => ({
  '/api/1/firewall': listed(),
  '/api/1/services': { show_version: 1, now_ms: 1_000_000, services: [{ name: 'firewall', state: 'running' }], trials: [], undone: [] },
  '/api/1/network': { cards: [] },
  '/api/1/console/fresh/firewall': { fresh: true },
  '/api/1/changes?limit=100': { changes: [] },
});

/**
 * Opens the Firewall page on a fresh document, with `hash` as the address
 * says it, and gives what the test needs to look at it. `reads` replaces what
 * a read answers (a value, a function of the path, or a promise, which makes
 * the read wait for it). The page is opened without waiting for its first
 * read: `ready` is what the test awaits when it wants the page drawn.
 */
export async function openFirewall(context, { hash = '', reads: replaced = {}, session = {} } = {}) {
  const page = usePage(context);
  // The fake elements keep no `dataset`, which the form and the table write to.
  const create = page.document.createElement;
  page.document.createElement = (tag) => Object.assign(create(tag), { dataset: {} });
  setUnlockSeconds(null);
  globalThis.matchMedia = () => ({ matches: false });
  const location = {
    pathname: '/firewall', search: '', hash,
  };
  globalThis.location = location;
  // As in a browser, a new address that has a fragment is the new hash.
  globalThis.history = {
    state: null,
    replaceState: (state, _, address) => {
      globalThis.history.state = state;
      location.hash = address.includes('#') ? address.slice(address.indexOf('#')) : '';
    },
  };
  const controller = new AbortController();
  context.after(() => {
    controller.abort();
    setUnlockSeconds(null);
    delete globalThis.matchMedia;
    delete globalThis.location;
    delete globalThis.history;
  });
  const answers = { ...reads(), ...replaced };
  const handlers = {};
  const everyStep = [];
  const view = {
    main: page.document.createElement('main'),
    slot: page.document.createElement('div'),
    signal: controller.signal,
    session: {
      name: 'chase', admin: true, terminal: true, kinds: [], host: 'duke-nas', ...session,
    },
    host: 'duke-nas',
    time: { clock: () => '' },
    path: '/firewall',
    rest: '',
    query: new URLSearchParams(),
    prefs: { theme: 'dark', zone: 'UTC', commands: 'hidden' },
    head: () => {
      const title = page.document.createElement('h1');
      page.document.body.append(title);
      return title;
    },
    on: (kind, handler) => {
      (handlers[kind] ??= []).push(handler);
    },
    onPrefs: () => {},
    every: (ms, step) => {
      everyStep.push(step);
    },
    notice: () => {},
    problem: () => {},
    api: {
      get: async (path) => {
        const value = answers[path];
        if (value === undefined) throw new ApiError({
          id: 'ADM-404', title: `no answer for ${path}`, detail: '', fix: '', status: 404,
        });
        return typeof value === 'function' ? value(path) : value;
      },
      run: async () => ({ ok: true, result: {} }),
      panel: () => page.document.createElement('div'),
      inline: () => page.document.createElement('div'),
    },
  };
  page.document.body.append(view.main, view.slot);
  const { show } = await import('../pages/firewall/page.js');
  const ready = show(view).then(settle);
  await settle();
  return {
    ...page,
    view,
    location,
    ready,
    form: () => view.main.querySelector('#add'),
    field: (id) => view.main.querySelector(`#${id}`),
    /** Runs what the page does every so often, as the time for it comes. */
    every: async () => {
      for (const step of everyStep) step();
      await settle();
    },
    /** Says `kind` to the page, as the console's events do: `change` for a new row of the journal. */
    emit: async (kind) => {
      for (const handler of handlers[kind] ?? []) handler();
      await settle();
    },
    /** Presses the button `id`. */
    press: async (id) => {
      view.main.querySelector(`#${id}`).click();
      await settle();
    },
    /** Types `words` into the field `id` as a person does: the value changes and the field says so. */
    type: async (id, words) => {
      const field = view.main.querySelector(`#${id}`);
      field.value = words;
      field.fire('input');
      await settle();
    },
  };
}
