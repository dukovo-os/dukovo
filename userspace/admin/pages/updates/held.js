// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Held card: the versions that are not taken until released, each with
// Release, and a way to hold one more: the system's offered release, the
// firmware set, or an app at the version it has.

import { fill, h, uniqueId } from '../../dom.js';
import { button, makeCard } from './cards.js';
import { heldRows, holdChoices } from './model.js';

/** The Held card. Gives `{element, draw}`. */
export function makeHeld(ctx) {
  const card = makeCard('held', 'Held');

  async function release(row, one) {
    const [action, args] = row.kind === 'app' ? ['apps.unhold', { id: row.item }] : ['updates.unhold', { item: row.item }];
    const result = await ctx.run(card, action, args, one, { unlock: 'release the hold', done: 'The hold is released.' });
    if (result.ok) await ctx.reload(card.heading);
  }

  async function hold(choice, one) {
    const [action, args] = choice.kind === 'app' ? ['apps.hold', { id: choice.item }] : ['updates.hold', { item: choice.item }];
    const result = await ctx.run(card, action, args, one, { unlock: 'hold a version', done: 'The version is held.' });
    if (result.ok) await ctx.reload(card.heading);
  }

  function draw() {
    const { data, admin } = ctx;
    const rows = heldRows(data.settings, data.apps);
    const choices = holdChoices(data.check, data.settings, data.apps);
    const select = h('select', { id: uniqueId('hold'), class: 'select' }, ...choices.map((choice, at) => h('option', { value: String(at) }, choice.label)));
    card.head(rows.length > 0 ? `${rows.length} held` : null);
    fill(card.body,
      rows.length === 0 ? h('p', {}, 'Nothing is held.') : h('ul', { class: 'upd-list' }, ...rows.map((row) => h('li', { class: 'upd-row' },
        h('div', {}, h('b', {}, row.title), h('div', { class: 'muted' }, row.line)),
        admin && button('Release', (one) => release(row, one))))),
      h('p', { class: 'hint' }, 'A held version gets no security fixes; hold for a while, not for good.'),
      admin && choices.length > 0 && h('div', { class: 'field' },
        h('label', { for: select.id }, 'Hold a version'),
        h('div', { class: 'btns' }, select, button('Hold it', (one) => hold(choices[Number(select.value)], one)))));
  }
  return { element: card.element, draw };
}
