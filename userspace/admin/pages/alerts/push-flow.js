// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of getting alerts in this browser: check that it can, start the
// worker (the browser refuses it when it does not trust the page's
// certificate), ask leave to show notices once the person pressed the button,
// read the machine's key, subscribe, and hand the subscription to the
// machine. The subscription is a secret: it goes only in the body of that one
// action, and the ID that comes back is all this browser keeps. Read when the
// card is drawn for a browser that does not get alerts yet.

import { fill, h, icon } from '../../dom.js';
import { refuse } from './cards.js';
import { canPush, UNSUPPORTED_WORDS } from './push-model.js';
import {
  addedId, CERTIFICATE_WORDS, certificateRefusal, DENIED_WORDS, keyBytes, nameProblem, sameKey, subscriptionLine, suggestedName,
} from './push-flow-model.js';
import { kindsFields, severityFields } from './told-fields.js';
import { aboutArg } from './told-model.js';

/** The words of a browser that does not trust the certificate (the two ways), as a banner. */
export function certificateBanner() {
  const {
    lead, letsEncrypt, own, meanwhile,
  } = CERTIFICATE_WORDS;
  return h('div', { class: 'banner warn' }, icon('warn'), h('div', {},
    h('p', {}, lead),
    h('ul', {},
      h('li', {}, h('a', { href: '/certificates' }, h('b', {}, letsEncrypt[0])), letsEncrypt[1]),
      h('li', {}, h('b', {}, own[0]), own[1])),
    h('p', {}, meanwhile)));
}

/** The fields of a browser that does not get alerts yet: its name, and what it is told. Gives `{element, options, problem}`. */
export function makeForm(env) {
  const name = h('input', {
    id: 'browser-name', class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', maxlength: '64', value: suggestedName(env.navigator?.userAgentData),
  });
  const severity = severityFields();
  const kinds = kindsFields();
  return {
    element: [
      h('div', { class: 'field' }, h('label', { for: name.id }, 'Name'), name, h('span', { class: 'hint' }, 'How this browser is listed in Where alerts go.')),
      h('details', {}, h('summary', {}, 'What it is told'), severity.element, kinds.element),
    ],
    options: () => ({
      name: name.value.trim(), ...severity.values(), about: kinds.ids(),
    }),
    /** What is wrong with the fields, `{words, input}`, or null. */
    problem() {
      const words = nameProblem(name.value);
      if (words) return { words, input: name };
      return kinds.problem() ? { words: kinds.problem(), input: null } : null;
    },
  };
}

/** The subscription of this browser to the machine's key: the one it has when it is for that key, else a new one. */
async function subscribe(registration, key) {
  const current = await registration.pushManager.getSubscription();
  if (current && sameKey(current.options?.applicationServerKey, key)) return current;
  await current?.unsubscribe();
  return registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
}

/**
 * Steps one to five, for the fields `form`, pressed with `button`; `slot` is
 * where a refusal goes. Gives `{id}` (null when the machine did not say it)
 * when this browser gets alerts now, and null when it does not.
 */
export async function enable({ view, env }, form, button, slot) {
  fill(slot);
  if (!canPush(env)) {
    refuse(slot, UNSUPPORTED_WORDS);
    return null;
  }
  const wrong = form.problem();
  if (wrong) {
    refuse(slot, wrong.words, wrong.input);
    return null;
  }
  const wanted = form.options();
  let registration;
  try {
    await env.navigator.serviceWorker.register('/sw.js', { scope: '/' });
    registration = await env.navigator.serviceWorker.ready;
  } catch (error) {
    if (certificateRefusal(error)) fill(slot, certificateBanner());
    else refuse(slot, `This browser could not start its notice service: ${error?.message ?? 'no reason given'}`);
    return null;
  }
  if (await env.Notification.requestPermission() !== 'granted') {
    refuse(slot, DENIED_WORDS);
    return null;
  }
  const asked = await view.api.run('alerts.push-key', {}, { button, where: { slot }, words: { unlock: 'get alerts in this browser', busy: 'Getting alerts…' } });
  if (!asked.ok) return null;
  const key = keyBytes(asked.result?.public_key);
  if (key === null) {
    refuse(slot, 'The server gave a key that is not a push key. Update it, and try again.');
    return null;
  }
  let line;
  try {
    line = subscriptionLine((await subscribe(registration, key)).toJSON());
  } catch (error) {
    refuse(slot, `This browser could not subscribe: ${error?.message ?? 'no reason given'}`);
    return null;
  }
  if (line === null) {
    refuse(slot, 'This browser gave a subscription that the server cannot take.');
    return null;
  }
  const about = aboutArg(wanted.about);
  const added = await view.api.run('alerts.push-add', {
    subscription: line, name: wanted.name, severity: wanted.severity, cleared: wanted.cleared, ...(about === 'all' ? {} : { about }),
  }, {
    button, where: { slot }, words: { unlock: 'get alerts in this browser', running: 'Getting alerts in this browser', done: 'This browser gets alerts.', busy: 'Getting alerts…' },
  });
  return added.ok ? { id: addedId(added.result) } : null;
}
