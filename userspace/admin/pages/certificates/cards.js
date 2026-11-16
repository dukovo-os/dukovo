// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The frame every card of the Certificates page has, and the buttons they
// draw: a heading that names the card, a place for a refusal, and the body.

import { fill, h } from '../../dom.js';

/**
 * A card: `{element, heading, errors, body}`. `before` are nodes that come
 * between the place for a refusal and the body.
 */
export function makeCard(id, name, ...before) {
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1' }, name);
  const errors = h('div', { class: 'certs-errors' });
  const body = h('div', { class: 'certs-body' });
  const element = h('section', { class: 'card', id, 'aria-labelledby': heading.id }, heading, errors, ...before, body);
  return {
    element, heading, errors, body,
  };
}

/** A button: `onclick` runs with it when it is pressed; `kind` is `default` for the one a card is for, `small` for one in a row. */
export function button(label, onclick, kind = '') {
  const one = h('button', { class: `btn${kind ? ` ${kind}` : ''}`, type: 'button' }, label);
  one.addEventListener('click', () => {
    if (one.getAttribute('aria-disabled') !== 'true') onclick(one);
  });
  return one;
}

/** Whether a button is off for now: it stays in the order of keys, and its reason is read with it. */
export function setOff(one, off) {
  if (off) one.setAttribute('aria-disabled', 'true');
  else one.removeAttribute('aria-disabled');
}

/** A line of text that says what happened, which a screen reader reads when it changes. */
export function statusLine() {
  const line = h('p', { class: 'small muted certs-status', role: 'status' });
  return {
    element: line,
    say: (words) => fill(line, words ?? ''),
  };
}
