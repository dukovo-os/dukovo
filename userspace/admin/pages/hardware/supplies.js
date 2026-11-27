// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Battery and UPS card: each UPS and battery of `hardware power` with its
// state in a pill, a bar of its charge, how long the charge lasts and, for a
// UPS, its load and when this machine shuts down cleanly (ADR 0741). The page
// reads it with the rest and again every minute, with the temperatures, and
// reads this file only on a machine with a UPS or a battery, which alone
// shows the card.

import { fill, h } from '../../dom.js';
import { cardPill, suppliesOf } from './supplies-model.js';

/** The body of `card`, made by cards.js, drawn from `ctx` (see page.js). Gives `{draw}`. */
export function makeSupplies(ctx, card) {

  /** The bar of a supply's charge, in its pill's tone where that is a warning. */
  const meterOf = (one) => {
    const tone = one.pill?.tone === 'bad' || one.pill?.tone === 'warn' ? ` ${one.pill.tone}` : '';
    const meter = h('div', { class: `hw-charge${tone}`, role: 'img', 'aria-label': `Charged ${one.percent}%` }, h('i'));
    meter.style.setProperty('--v', String(Math.max(0, Math.min(100, one.percent))));
    return meter;
  };

  /** A supply: its name and pill, its charge as a bar, its rows and its notes. */
  const supplyOf = (one) => h('div', { class: 'hw-supply' },
    h('p', {}, h('b', {}, one.title), h('small', { class: 'muted' }, ` ${one.name}`),
      one.pill && [' ', h('span', { class: `st ${one.pill.tone}` }, one.pill.word)]),
    one.percent !== null && meterOf(one),
    one.rows.length > 0 && h('dl', { class: 'rows' }, one.rows.flatMap(([label, words]) => [h('dt', {}, label), h('dd', {}, words)])),
    one.replace && h('p', { class: 'hw-start bad' }, one.replace),
    one.note && h('p', { class: 'small muted' }, one.note));

  function draw() {
    const supplies = suppliesOf(ctx.data.power);
    card.element.hidden = supplies.length === 0;
    if (supplies.length === 0) return;
    card.head(cardPill(supplies));
    fill(card.body, supplies.map(supplyOf));
  }

  return { draw };
}
