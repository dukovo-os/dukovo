// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The frame each card of the Alerts page has: a heading with its buttons at
// the right, a place for a problem and a body that the card fills.

import { fill, h, icon } from '../../dom.js';

/** The card `id` named `title`: `{element, tools, errors, body}`; `tools` is where its buttons go beside the heading. */
export function card(id, title) {
  const tools = h('span', { class: 'more' });
  const errors = h('div', { class: 'al-errors' });
  const body = h('div', { class: 'al-body' });
  const element = h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` }, h('h2', { id: `${id}-h`, tabindex: '-1' }, title, tools), errors, body);
  return {
    element, tools, errors, body,
  };
}

/** A line of a card that says nothing is there: "No alerts are active.". */
export const nothing = (words) => h('p', { class: 'muted' }, words);

/** Says in `slot`, a place for a problem, what is wrong with what the person typed; `input` is the field it is about, which takes the focus. */
export function refuse(slot, words, input = null) {
  fill(slot, h('p', { class: 'msg', role: 'alert' }, icon('warn'), h('span', {}, words)));
  if (input) {
    input.setAttribute('aria-invalid', 'true');
    input.addEventListener('input', () => input.removeAttribute('aria-invalid'), { once: true });
    input.focus();
  }
}

/** What is typed in the secret field `secret`, to be checked and not kept: it stays in the field until `take()` hands it over. */
export const peek = (secret) => secret.element.querySelector('input').value;
