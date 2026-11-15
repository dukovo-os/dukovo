// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the cards of the page of one service are made of: a card that is
// drawn again only when what it says changes, the list of labelled rows that
// most of them hold, and a command in a box with a button that copies it.

import { fill, h, icon, uniqueId } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { serviceHref } from './needs.js';

/**
 * A card with the heading `title`, whose body `draw(model)` makes. `set(model)`
 * draws it again only when `model` is not what it last drew, so that a card
 * that is read every few seconds does not lose a selection or a place; a
 * `model` of null hides the card. `id` is the card's address on the page.
 */
export function makeCard(id, title, draw) {
  const body = h('div', { class: 'svc-body' });
  const element = h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` }, h('h2', { id: `${id}-h` }, title), body);
  let last;
  return {
    element,
    body,
    set(model) {
      element.hidden = model === null;
      const key = JSON.stringify(model);
      if (key === last) return;
      last = key;
      if (model !== null) fill(body, draw(model));
    },
  };
}

/**
 * A place that holds what `make(model)` makes, drawn again only when `model`
 * is not what it last drew; a `model` of null leaves it empty. Gives
 * `set(model, make)`.
 */
export function stable(box) {
  let last;
  return (model, make) => {
    const key = JSON.stringify(model);
    if (key === last) return;
    last = key;
    box.replaceChildren(...(model === null ? [] : [make(model)]));
  };
}

/** A card with grey bars where its words will be, while the service is read. */
export function frameOf(id, title) {
  const body = h('div', { class: 'svc-body', 'aria-hidden': 'true' }, ...[0, 1, 2].map(() => h('span', { class: 'svc-bar wide' })));
  return h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` }, h('h2', { id: `${id}-h` }, title), body);
}

/** The services of `names` as links to their pages. */
const links = (names) => h('ul', { class: 'svc-items inline' }, names.map((name) => h('li', {}, h('a', { href: serviceHref(name) }, name))));

/**
 * The labelled rows of a card, from `{label, value}`, `{label, items}` or
 * `{label, names}`: text, a list of lines, or a list of services to link to.
 */
export function rowsList(rows) {
  return h('dl', { class: 'rows svc-rows' }, rows.map((row) => [
    h('dt', {}, row.label),
    h('dd', {},
      row.names ? links(row.names)
        : row.items ? h('ul', { class: 'svc-items' }, row.items.map((item) => h('li', {}, item)))
          : row.value),
  ]));
}

/** The muted line that stands where a card has nothing to say. */
export const quiet = (words) => h('p', { class: 'muted' }, words);

/** A box with `command` in it, under `title`, and a button that copies it. */
export function copyBox(command, title = 'At a terminal') {
  const id = uniqueId('svc-cmd');
  const pre = h('pre', { id, class: 'cmdtext', tabindex: '0' }, command);
  const status = h('span', { class: 'small', role: 'status' });
  const copy = h('button', { class: 'btn small', type: 'button', 'aria-describedby': id }, icon('copy'), 'Copy');
  copy.addEventListener('click', async () => {
    status.textContent = (await copyText(command, pre)) ? COPIED : PRESS_CTRL_C;
  });
  return h('div', { class: 'cmd' }, h('div', { class: 'cmd-head' }, icon('code'), title, h('div', { class: 'btns' }, copy)), pre, status);
}
