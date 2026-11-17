// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Held card: the versions that are not taken until released, each with
// Release, and a way to hold one more: the system's offered release, the
// firmware set, or an app at the version it has.

import { fill, h, uniqueId } from '../../dom.js';
import { button, makeCard } from './cards.js';
import { heldRows, holdCall, holdChoices } from './model.js';

/** What holding does, said in the question that asks why. */
const LEAD = 'A held version is not taken until you release it, and gets no security fixes; hold for a while, not for good.';

/** The Held card. Gives `{element, draw}`. */
export function makeHeld(ctx) {
  const card = makeCard('held', 'Held');

  async function release(row, one) {
    const [action, args] = row.kind === 'app' ? ['apps.unhold', { id: row.item }] : ['updates.unhold', { item: row.item }];
    const result = await ctx.run(card, action, args, one, { unlock: 'release the hold', done: 'The hold is released.' });
    if (result.ok) await ctx.reload(card.heading);
  }

  /** Asks why, then holds. */
  async function hold(choice) {
    const { openHold } = await import('../../lib/hold-dialog.js');
    const made = await openHold(ctx.view, {
      title: `Hold ${choice.what}`,
      lead: LEAD,
      place: choice.kind === 'app' ? 'bundle list' : 'update settings',
      call: (why) => holdCall(choice, why),
      words: { unlock: 'hold a version', done: 'The version is held.' },
    });
    if (made) await ctx.reload(card.heading);
  }

  function draw() {
    const { data, admin } = ctx;
    const rows = heldRows(data.settings, data.apps, { time: ctx.view.time, names: data.names });
    const choices = holdChoices(data.check, data.settings, data.apps);
    const select = h('select', { id: uniqueId('hold'), class: 'select' }, ...choices.map((choice, at) => h('option', { value: String(at) }, choice.label)));
    card.head(rows.length > 0 ? `${rows.length} held` : null);
    fill(card.body,
      rows.length === 0 ? h('p', {}, 'Nothing is held.') : h('ul', { class: 'upd-list' }, ...rows.map((row) => h('li', { class: 'upd-row' },
        h('div', {}, h('b', {}, row.title), row.held && [' · ', row.held], h('div', { class: 'muted' }, row.line)),
        admin && button('Release', (one) => release(row, one))))),
      h('p', { class: 'hint' }, 'A held version gets no security fixes; hold for a while, not for good.'),
      admin && choices.length > 0 && h('div', { class: 'field' },
        h('label', { for: select.id }, 'Hold a version'),
        h('div', { class: 'btns' }, select, button('Hold it…', () => hold(choices[Number(select.value)])))));
  }
  return { element: card.element, draw };
}
