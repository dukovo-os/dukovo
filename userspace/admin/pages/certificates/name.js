// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Name and domain card: what the machine is called, its domain, the name
// it answers to on the local network, and the button that changes them. Once
// the name is changed the page asks for nothing more: the card says what the
// new fingerprint is and offers the link to the new address.

import { fill, h } from '../../dom.js';
import { button, makeCard } from './cards.js';
import { machineWords, renamedWords, RULES_WORDS } from './name-model.js';

/** The card. `ctx` is the page's (see page.js). Gives `{element, errors, draw, renamed}`. */
export function makeName(ctx) {
  const card = makeCard('name', 'Name and domain');
  let done = null;

  /** Shows that the machine is renamed: the new fingerprint, and the link that follows it. */
  function renamed(info) {
    done = info;
    draw();
  }

  function draw() {
    const { machine } = ctx.data;
    if (done !== null) {
      const link = done.follow
        ? h('a', { class: 'btn default', href: done.follow.url }, `Open the console at ${done.follow.host}`)
        : h('a', { class: 'btn default', href: globalThis.location?.href ?? '/' }, 'Reload this page');
      fill(card.body,
        h('p', { class: 'msg certs-tone-ok', role: 'status' }, renamedWords(done.name, done.fingerprint)),
        done.follow ? h('p', { class: 'small muted' }, 'This page asks for nothing more: the old name no longer answers.') : h('p', { class: 'small muted' }, `This page keeps working at ${done.host}, and asks for nothing more.`),
        link);
      return;
    }
    if (machine === undefined) {
      fill(card.body, h('p', { class: 'muted' }, 'The machine\'s name could not be read.'));
      return;
    }
    if (machine === null) {
      fill(card.body, h('p', { class: 'muted' }, 'Reading the name…'));
      return;
    }
    fill(card.body,
      h('dl', { class: 'rows' },
        h('dt', {}, 'Name'), h('dd', {}, machine.first),
        h('dt', {}, 'Domain'), h('dd', {}, machine.domain ?? h('span', { class: 'muted' }, 'none'))),
      h('p', { class: 'small muted' }, machineWords(machine)),
      h('p', { class: 'hint' }, RULES_WORDS),
      ctx.admin && h('div', { class: 'btns' }, button('Change…', (one) => ctx.rename(one))));
  }

  return {
    element: card.element, errors: card.errors, draw, renamed,
  };
}
