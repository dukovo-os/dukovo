// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A view that answers as admind does for the Security page: what each read
// gives, what each action answers, and a record of what was asked, so that a
// test can press a button and see which call it made. Not a test itself, and
// never installed.

import { ApiError } from '../api.js';
import { setUnlockSeconds } from '../unlock.js';
import {
  accounts, blocked, kept, lastlog, network, NOW, settings, sparkIdle, ssh, time, wireguard,
} from './security-fixtures.mjs';
import { Element, usePage } from './fake-dom.mjs';

/** Lets what is waiting on a promise run, as a click does. */
export const settle = async () => {
  for (let at = 0; at < 20; at += 1) await new Promise((resolve) => setImmediate(resolve));
};

/** The button of `root` whose words are `words`. */
export const buttonOf = (root, words) => root.querySelectorAll('button').find((one) => one.textContent.trim() === words);

/** The element of `root` that carries `data-action="id"`. */
export const control = (root, id) => root.querySelector(`[data-action="${id}"]`);

/** The input of `root` that the label named `words` is for, or that the label holds. */
export const fieldOf = (root, words) => {
  const label = root.querySelectorAll('label').find((one) => one.textContent.trim() === words);
  if (!label) return null;
  return label.hasAttribute('for') ? root.querySelector(`#${label.getAttribute('for')}`) : label.querySelector('input');
};

/** Types `text` into `input`, as a person does. */
export function type(input, text) {
  input.value = text;
  input.fire('input');
}

/** Chooses the radio card of `root` whose value is `value`. */
export function choose(root, value) {
  const radios = root.querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
  const target = radios.find((one) => one.getAttribute('value') === value);
  for (const one of radios) if (one.getAttribute('name') === target.getAttribute('name')) one.checked = one === target;
  target.closest('fieldset').fire('change');
}

/** Sets the checkbox `box` on or off, as a press does. */
export function tick(box, on) {
  box.checked = on;
  (box.closest('fieldset') ?? box).fire('change');
}

/** An admind refusal, as `api.js` raises it. */
export const refusal = (id, title, status = 409, more = {}) => new ApiError({
  id, title, detail: '', fix: '', status, ...more,
});

/** Makes `element` keep, as a browser does, what its `value`, `checked`, `disabled` and `selected` attributes start from, gives it a `style` that keeps what it is set, and names itself as the event's `currentTarget`. */
function reflect(element) {
  const set = element.setAttribute.bind(element);
  element.setAttribute = (name, value) => {
    set(name, value);
    if (name === 'value') element.value = String(value);
  };
  const fire = element.fire.bind(element);
  element.fire = (type, more = {}) => fire(type, { currentTarget: element, ...more });
  if (element.tag === 'select') {
    // A select holds the value of the option that is selected, else of the first.
    const options = () => element.querySelectorAll('option');
    Object.defineProperty(element, 'value', {
      get: () => (options().find((one) => one.hasAttribute('selected')) ?? options()[0])?.getAttribute('value') ?? '',
      set: (value) => options().forEach((one) => (one.getAttribute('value') === value ? one.setAttribute('selected', '') : one.removeAttribute('selected'))),
      configurable: true,
    });
  }
  const properties = new Map();
  element.style = { setProperty: (name, value) => properties.set(name, value), getPropertyValue: (name) => properties.get(name) ?? '' };
  for (const name of ['checked', 'disabled', 'selected']) {
    Object.defineProperty(element, name, {
      get: () => element.hasAttribute(name),
      set: (on) => (on ? element.setAttribute(name, '') : element.removeAttribute(name)),
      configurable: true,
    });
  }
  return element;
}

/** What the reads of the page answer when a test does not say otherwise. */
const reads = () => ({
  '/api/1/checks/last': kept(),
  '/api/1/security/sign-in': settings(),
  '/api/1/ssh': ssh(),
  '/api/1/services': sparkIdle(),
  '/api/1/changes?limit=100': { changes: [] },
  '/api/1/accounts': accounts(),
  '/api/1/sign-ins/last': lastlog(),
  '/api/1/network': network(),
});

/** What the actions answer when a test does not say otherwise. */
const answers = () => ({
  'security.blocked': { ok: true, result: blocked() },
  'private.show': { ok: true, result: { interfaces: [] } },
});

/** A fresh page, as usePage gives it, whose elements keep their properties as `reflect` says. */
export function usePageWithProperties(context) {
  const page = usePage(context);
  const create = page.document.createElement;
  page.document.createElement = (tag) => reflect(create(tag));
  return page;
}

/**
 * Opens the Security page on a fresh document and gives what the test needs to
 * look at it and press it. `session` is who looks; `unlocked` whether the
 * console is unlocked; `world.reads` replaces what a read answers (a value, a
 * function of the path, or an error to raise) and `world.answers` what an
 * action answers (a result, an error, or a function of its arguments).
 */
export async function openSecurity(context, {
  session = {}, unlocked = false, world = {}, prefs = {}, module = '../pages/security/page.js', hash = '',
} = {}) {
  const page = usePageWithProperties(context);
  context.mock.method(Date, 'now', () => NOW);
  globalThis.matchMedia = () => ({ matches: false });
  globalThis.location = { hostname: 'duke-lab.local', hash };
  context.after(() => {
    delete globalThis.matchMedia;
    delete globalThis.location;
    setUnlockSeconds(null);
  });
  const head = new Element('head', page.document);
  page.document.head = head;
  setUnlockSeconds(unlocked ? 900 : null);
  const controller = new AbortController();
  context.after(() => controller.abort());
  const who = {
    name: 'chase', admin: true, kinds: ['password'], terminal: true, from: '192.168.1.25', via: { address: '192.168.1.10', port: 9090, card: 'net0' }, ...session,
  };
  const found = { ...reads(), ...world.reads };
  const calls = [];
  const panels = [];
  const problems = [];
  const notices = [];
  const went = [];
  const listeners = {};
  const timers = [];
  const ran = [];
  const answer = (path) => {
    calls.push(path);
    const value = found[path];
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
    host: 'duke-lab',
    path: '/security',
    rest: '',
    prefs: {
      theme: 'dark', text: 100, motion: 'normal', zone: 'UTC', commands: 'hidden', ...prefs,
    },
    head: (options) => { view.headed = options; },
    on: (event, listener) => { (listeners[event] ??= []).push(listener); },
    every: (ms, fn) => timers.push([ms, fn]),
    go: (path) => went.push(path),
    notice: (words, options) => notices.push([words, options]),
    problem: (error, where) => {
      problems.push([error, where]);
      // The shell draws the error in the slot it is given; a test reads its words there.
      where?.slot?.replaceChildren(`${error.title ?? ''} ${error.id ?? ''}`);
    },
    api: {
      get: async (path) => answer(path),
      post: async () => ({}),
      run: async (action, args, options = {}) => {
        ran.push([action, args, options]);
        const value = { ...answers(), ...world.answers }[action];
        if (value instanceof Error) {
          view.problem(value, options.where ?? {});
          return { ok: false, error: value };
        }
        if (typeof value === 'function') return value(args);
        return value ?? { ok: true, result: {} };
      },
      panel: (action, args) => {
        panels.push([action, args]);
        return page.document.createElement('div');
      },
      inline: () => null,
    },
  };
  page.document.body.append(view.main);
  const { show } = await import(module);
  await show(view);
  await settle();
  return {
    ...page,
    view,
    calls,
    panels,
    problems,
    notices,
    went,
    listeners,
    timers,
    ran: (action) => ran.filter((call) => call[0] === action).map((call) => call[1]),
    ranWith: (action) => ran.filter((call) => call[0] === action),
    allRan: () => ran.map((call) => call[0]),
    emit: (event, data) => (listeners[event] ?? []).forEach((listener) => listener(data)),
    tick: async (ms) => {
      for (const [every, fn] of timers) if (every === ms) await fn();
      await settle();
    },
    dialog: () => page.document.body.querySelectorAll('dialog').at(-1) ?? null,
    card: (id) => view.main.querySelector(`#${id}`),
    answers: world.answers,
    reads: found,
    wireguard,
  };
}
