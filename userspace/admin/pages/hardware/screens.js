// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Screens card's list: which program has the screens, and a table for
// each graphics card with its screens, the monitor plugged into each, the
// mode it shows, the modes it offers, and what its colours can do. Read
// when the card's button is pressed, and this file with it. It changes
// nothing.

import { fill, h } from '../../dom.js';
import { setOff } from './cards.js';
import { NO_SCREENS, screensOf } from './screens-model.js';

const COLUMNS = ['Screen', 'Monitor', 'Shows', 'Modes', 'Colour depth', 'HDR', 'Variable refresh'];

/** A table of `rows` whose caption is `title`; the rows turn into cards on a phone. */
const tableOf = (title, rows) => h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
  h('caption', {}, title),
  h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
  h('tbody', {}, rows.map((row) => h('tr', {},
    h('td', { 'data-label': COLUMNS[0] }, h('b', {}, row.name)),
    h('td', { 'data-label': COLUMNS[1], class: row.muted ? 'muted' : null }, row.monitor, row.small && h('small', {}, row.small)),
    h('td', { 'data-label': COLUMNS[2] }, row.tone ? h('span', { class: `st ${row.tone}` }, row.shows) : row.shows),
    h('td', { 'data-label': COLUMNS[3] }, row.modes),
    h('td', { 'data-label': COLUMNS[4] }, row.depth),
    h('td', { 'data-label': COLUMNS[5] }, row.hdr),
    h('td', { 'data-label': COLUMNS[6] }, row.variable))))));

/** Reads the screens and draws them into `card` (made by makeCard); `one` is the button that was pressed. */
export async function drawScreens(ctx, card, one) {
  const { view } = ctx;
  setOff(one, true);
  try {
    const { front, cards } = screensOf(await view.api.get('/api/1/hardware/screens'));
    fill(card.errors);
    fill(card.body,
      cards.length === 0 && h('p', {}, NO_SCREENS),
      cards.length > 0 && h('p', {}, h('b', {}, 'In front: '), front),
      cards.map((one) => tableOf(one.title, one.rows)));
    card.heading.focus();
  } catch (error) {
    setOff(one, false);
    view.problem(error, { slot: card.errors });
  }
}
