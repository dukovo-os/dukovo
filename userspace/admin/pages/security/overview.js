// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Security page gives the Overview (design 8.11): two steps of Finish
// setting up and /setup, and no things to look at, since the Health card
// already shows the checks. The first is a key for the person's own account
// while SSH is on; the second is private access, whose set-up fields are those
// of the Security page. Every read is the person's own; one that an unlocked
// console may answer and a locked one may not leaves the step not done.

import { h } from '../../dom.js';
import { keyLineProblem } from '../accounts/typed-model.js';
import { DEFAULTS, openPort } from './private-form-model.js';
import { interfaceOf } from './private-model.js';

const readSsh = (cx) => cx.cache('/api/1/ssh', 5000, () => cx.api.get('/api/1/ssh'));
const readAccount = (cx) => cx.cache('/api/1/account/show', 5000, () => cx.api.get('/api/1/account/show'));
const readPrivate = (cx) => cx.cache('/api/1/private', 5000, () => cx.api.get('/api/1/private'));

/** Whether the person's own account has a key, from `user show --json`. */
const hasKey = (shown) => Array.isArray(shown?.keys) && shown.keys.length > 0;

/** Security has nothing to look at of its own: the Health card shows the checks. */
export const needs = async () => [];

export const steps = [{
  id: 'ssh-keys',
  title: 'Sign in over SSH with a key',
  order: 20,
  points: 0,
  action: 'account.key-add',
  // Only worth showing where SSH is on; a read that fails offers nothing.
  async available(cx) {
    try {
      return (await readSsh(cx)).on !== false;
    } catch {
      return false;
    }
  },
  async done(cx) {
    try {
      return hasKey(await readAccount(cx));
    } catch {
      return false;
    }
  },
  async summary(cx) {
    try {
      return hasKey(await readAccount(cx)) ? 'Your key is in' : 'No key yet';
    } catch {
      return 'No key yet';
    }
  },
  /** The key line, which `account.key-add` takes; after it is in, what to do next. */
  form() {
    const input = h('textarea', { id: 'ssh-key-line', class: 'input mono', rows: '3', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': 'ssh-key-hint' });
    const next = h('p', { class: 'small', role: 'status', hidden: true }, 'Next: ', h('a', { href: '/security#ssh' }, 'Keys only…'), ' on Security turns passwords off.');
    const element = h('section', { class: 'card', 'aria-labelledby': 'ssh-key-h' },
      h('h2', { id: 'ssh-key-h' }, 'Sign in over SSH with a key'),
      h('p', {}, 'A key signs you in over SSH without a password that can be guessed. Paste the public key: the line that ssh-keygen wrote in the .pub file.'),
      h('div', { class: 'field' }, h('label', { for: input.id }, 'Public key'), input, h('span', { id: 'ssh-key-hint', class: 'hint' }, 'Ed25519, or ECDSA on P-256 or P-384.')),
      next);
    return {
      element,
      args() {
        const problem = keyLineProblem(input.value);
        if (problem) throw Object.assign(new Error(problem), { words: problem });
        return { line: input.value.trim() };
      },
      async after() {
        input.value = '';
        next.hidden = false;
      },
    };
  },
}, {
  id: 'private',
  title: 'Reach this console from anywhere, privately',
  order: 40,
  points: 0,
  action: 'private.serve',
  // Every machine can have it, so there is nothing to ask first.
  available: async () => true,
  async done(cx) {
    try {
      return (interfaceOf(await readPrivate(cx))?.devices ?? []).length > 0;
    } catch {
      return false;
    }
  },
  async summary(cx) {
    try {
      const iface = interfaceOf(await readPrivate(cx));
      if (iface === null) return 'Not set up';
      return iface.devices?.length > 0 ? 'A device is connected' : 'Set up, with no device yet';
    } catch {
      return 'Not set up';
    }
  },
  /** The fields of the page's Set up, read when the step is first shown; after it runs, the page where a device is added. */
  form(view) {
    const element = h('section', { class: 'card', 'aria-labelledby': 'private-step-h', 'aria-busy': 'true' },
      h('h2', { id: 'private-step-h' }, 'Reach this console from anywhere, privately'),
      h('p', { class: 'muted', role: 'status' }, 'Reading the form…'));
    let fields = null;
    import('./private-form.js').then((loaded) => {
      fields = loaded.setUpFields(view.host ?? location.hostname);
      element.replaceChildren(
        h('h2', { id: 'private-step-h' }, 'Reach this console from anywhere, privately'),
        h('p', {}, 'A phone or laptop with the WireGuard app joins this server\'s own small network, wg0, from anywhere. The console stays off the internet and answers only on this network and through WireGuard. Setting it up opens WireGuard\'s UDP port in the firewall.'),
        ...fields.elements);
      element.removeAttribute('aria-busy');
    }, () => {
      element.replaceChildren(h('h2', {}, 'Reach this console from anywhere, privately'), h('p', { class: 'muted', role: 'status' }, 'This step\'s form could not be shown. Reload the page.'));
      element.removeAttribute('aria-busy');
    });
    return {
      element,
      args() {
        if (fields === null) throw Object.assign(new Error('the form is not drawn yet'), { words: '' });
        const { args, problems } = fields.check();
        if (args === null) throw Object.assign(new Error('the form has a mistake'), { words: Object.values(problems)[0] });
        return args;
      },
      /** WireGuard's port is opened, and then the person adds a device, on the page that shows the file once. */
      async after() {
        const { args } = fields.check();
        if (args !== null) {
          await view.api.run('firewall.add', openPort(args.port ?? DEFAULTS.port), { words: { unlock: 'open WireGuard\'s port', running: 'Opening WireGuard\'s port', done: 'WireGuard\'s port is open.' } });
        }
        view.go('/security#private');
      },
    };
  },
}];
