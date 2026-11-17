// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// This browser: alerts as notices on this device. The card says whether this
// browser gets them (when the machine lists the subscription whose ID this
// browser kept), offers to turn them on with the steps of push-flow.js, and
// turns them off: the machine forgets the subscription, then the browser.

import { fill, h } from '../../dom.js';
import { forget, keep, recall } from '../../lib/store.js';
import { card, refuse } from './cards.js';
import { listOf } from './channels-model.js';
import {
  canPush, hereState, PRIVACY_WORDS, PUSH_ID_KEY, UNSUPPORTED_WORDS,
} from './push-model.js';

const ID = /^[0-9a-f]{16}$/;

/**
 * The This browser card. `ctx` is the page's: `{view, load, test, env}`;
 * `env` is where the browser's own objects are (`globalThis`, which a test
 * replaces). Gives `{element, set, enable, turnOff, removed, banner, isOn, id,
 * onChange}`.
 */
export function makePush(ctx) {
  const { view } = ctx;
  const env = () => ctx.env ?? globalThis;
  const here = card('browser', 'This browser');
  const slot = here.errors;
  const stored = recall(PUSH_ID_KEY, null);
  let id = typeof stored === 'string' && ID.test(stored) ? stored : null;
  let state = 'off';
  let flow = null;
  let form = null;
  let drawn = 0;
  const watchers = [];
  const read = () => {
    flow ??= import('./push-flow.js');
    return flow;
  };

  const get = h('button', { class: 'btn default', type: 'button' }, 'Get alerts in this browser');
  get.addEventListener('click', () => enable(get, slot));
  const send = h('button', { class: 'btn', type: 'button' }, 'Send a test');
  send.addEventListener('click', () => ctx.test(send));
  const off = h('button', { class: 'btn', type: 'button' }, 'Turn off in this browser');
  off.addEventListener('click', () => turnOff(off, slot));
  const privacy = h('p', { class: 'hint' }, PRIVACY_WORDS);

  /** Ends this browser's subscription, if it has one; it may not, and nothing is lost when it cannot. */
  async function unsubscribe() {
    try {
      const registration = await env().navigator.serviceWorker.getRegistration('/');
      await (await registration?.pushManager.getSubscription())?.unsubscribe();
    } catch {
      // The browser keeps what it could not end; it sends nothing without the machine's key.
    }
  }

  /** Forgets this browser's ID and says so to what watches. */
  function letGo() {
    forget(PUSH_ID_KEY);
    id = null;
    state = 'off';
  }

  async function draw() {
    drawn += 1;
    const mine = drawn;
    if (!canPush(env())) {
      fill(here.body, h('p', { class: 'muted' }, `${UNSUPPORTED_WORDS} Email and webhooks such as ntfy reach your phone.`));
      return;
    }
    if (state === 'on') {
      fill(here.body,
        h('p', {}, h('b', {}, 'This browser gets alerts.'), ' A notice shows on this device when an alert is raised, even with the console closed.'),
        h('div', { class: 'btns' }, send, off),
        privacy);
      return;
    }
    const steps = await read();
    if (mine !== drawn) return;
    form ??= steps.makeForm(env());
    fill(here.body,
      h('p', { class: 'muted' }, 'Get a notice on this device when an alert is raised, even with the console closed.'),
      form.element,
      h('div', { class: 'btns' }, get),
      privacy);
  }

  /** Steps one to five, pressed with `button`; `where` is where a refusal goes. Gives whether this browser gets alerts now. */
  async function enable(button, where) {
    const steps = await read();
    form ??= steps.makeForm(env());
    const got = await steps.enable({ view, env: env() }, form, button, where);
    if (got === null) return false;
    id = got.id;
    if (id === null) refuse(where, 'The server added this browser but did not say its ID. Look for it in Where alerts go.');
    else if (!keep(PUSH_ID_KEY, id)) refuse(where, 'This browser gets alerts, but it would not keep a note of it, so this page cannot tell which row of Where alerts go is this browser. Remove it from there to stop.');
    state = id === null ? 'off' : 'on';
    ctx.load();
    watchers.forEach((fn) => fn());
    return true;
  }

  /** The second button: the machine forgets this browser, and then the browser its subscription. */
  async function turnOff(button, where) {
    fill(where);
    if (id !== null) {
      const removed = await view.api.run('alerts.push-remove', { id }, {
        button, where: { slot: where }, words: { unlock: 'turn off alerts in this browser', done: 'This browser gets no more alerts.' },
      });
      if (!removed.ok) return false;
    }
    await unsubscribe();
    letGo();
    ctx.load();
    watchers.forEach((fn) => fn());
    return true;
  }

  draw();
  return {
    element: here.element,
    enable,
    turnOff,
    banner: async () => (await read()).certificateBanner(),
    id: () => id,
    isOn: () => state === 'on',
    onChange: (fn) => watchers.push(fn),
    /** Takes what the machine lists: this browser gets alerts when it lists the subscription this browser kept. */
    set(channels) {
      state = hereState(listOf(channels), id);
      if (id !== null && state === 'off') letGo();
      draw();
      watchers.forEach((fn) => fn());
    },
    /** The channel of a row that was removed from Where alerts go: when it is this browser's, the browser lets go of it too. */
    removed(channel) {
      if (channel.kind !== 'push' || channel.id !== id) return;
      letGo();
      unsubscribe();
      draw();
    },
  };
}
