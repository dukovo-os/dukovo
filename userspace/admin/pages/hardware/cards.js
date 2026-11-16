// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The frame every card of the Hardware page has, and the button they draw:
// a heading that names the card and wears the state in a pill that a screen
// reader skips (the words say it), a place for a refusal, and the body.

import { fill, h } from '../../dom.js';

/**
 * A card: `{element, heading, errors, body, head(pill)}`. `head` draws the
 * heading again with the pill `{word, tone}`, or none; `before` are nodes
 * that come between the place for a refusal and the body.
 */
export function makeCard(id, name, ...before) {
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1' }, name);
  const errors = h('div', { class: 'hw-errors' });
  const body = h('div', { class: 'hw-body' });
  const element = h('section', { class: 'card', id, 'aria-labelledby': heading.id }, heading, errors, ...before, body);
  return {
    element,
    heading,
    errors,
    body,
    head(pill = null) {
      const tone = pill?.tone === 'off' ? '' : pill?.tone;
      fill(heading, name, pill && h('span', { class: `pill ${tone}`.trim(), 'aria-hidden': 'true' }, pill.word));
    },
  };
}

/** A button: `onclick` runs with it when it is pressed; `kind` is `default` for the one a card is for, `danger` for what cannot be undone from afar. */
export function button(label, onclick, kind = '') {
  const one = h('button', { class: `btn${kind ? ` ${kind}` : ''}`, type: 'button' }, label);
  one.addEventListener('click', () => onclick(one));
  return one;
}

/** Whether a button is off for now: it stays in the tab order, and says why where it is read. */
export function setOff(one, off) {
  if (off) one.setAttribute('aria-disabled', 'true');
  else one.removeAttribute('aria-disabled');
}
