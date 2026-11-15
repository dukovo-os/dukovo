// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The cards of use (design 4.3): how full each volume is, whether it is a
// mirror and in what state, when it will be full at its rate, and the card of
// the reads and writes of the last hour with the chart the Overview draws.
// A card is drawn again only when what it says changes.

import { fill, h } from '../../dom.js';
import { makeChart } from '../../lib/chart.js';
import { CHARTS, modelOf, rangeOf } from '../../lib/chart-model.js';

/** The chart of the disks' reads and writes. */
const DISK_CHART = CHARTS.find((chart) => chart.id === 'disk');

/** The range of the chart: the last hour. */
const HOUR = rangeOf('1h');

/** The grey bars that stand for a card while the disks are read. */
export const bar = () => h('span', { class: 'sto-bar', 'aria-hidden': 'true' });

/** One card of use, from `cardsOf`'s `card`. */
function cardOf(card) {
  const meter = h('i');
  meter.style.setProperty('--v', String(Math.min(100, card.percent)));
  return h('section', { class: 'card sto-card', 'aria-labelledby': `${card.id}-h`, 'data-card': card.id },
    h('h2', { id: `${card.id}-h` }, card.title, card.pill && h('span', { class: `pill ${card.pill.tone}` }, card.pill.word)),
    h('div', { class: 'stat' }, h('b', {}, card.used), h('span', {}, card.of)),
    h('div', { class: `meter sto-meter ${card.tone}`.trim(), role: 'img', 'aria-label': `${card.percent} percent used` }, meter),
    card.note && h('p', { class: 'small muted' }, card.note),
    card.projection && h('span', { class: 'small muted' }, `${card.projection}.`));
}

/**
 * The cards. `set(cards)` draws the cards of `cardsOf`; `setUsage(answer)`
 * draws the chart of the last hour, `answer` being `GET /api/1/storage/usage/1h`
 * (an empty list when the machine has kept no history); `frame()` draws the grey
 * bars; `fail(error)` puts a problem in the chart's card. Gives `{element,
 * set, setUsage, frame, fail}`.
 */
export function makeUse(view) {
  const chart = makeChart('disk');
  const errors = h('div', { class: 'sto-errors' });
  const io = h('section', { class: 'card sto-io', 'aria-labelledby': 'sto-io-h' }, h('h2', { id: 'sto-io-h' }, 'Reads and writes, last hour'), errors, chart.element);
  const cards = h('div', { class: 'sto-cards-list' });
  const element = h('div', { class: 'sto-cards', 'aria-busy': 'true' }, cards, io);
  let last = null;

  return {
    element,
    frame() {
      fill(cards, h('section', { class: 'card sto-card', 'aria-hidden': 'true' }, h('h2', {}, 'Storage'), bar(), bar(), bar()));
    },
    set(list) {
      element.removeAttribute('aria-busy');
      const key = JSON.stringify(list);
      if (key === last) return;
      last = key;
      fill(cards, list.map(cardOf));
    },
    setUsage(answer) {
      errors.replaceChildren();
      chart.update(modelOf(DISK_CHART, answer, { range: HOUR, now: Math.floor(Date.now() / 1000) }), view.time);
    },
    fail(error) {
      view.problem(error, { slot: errors });
    },
  };
}
