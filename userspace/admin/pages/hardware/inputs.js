// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Input devices card's list: a table of every keyboard, mouse, tablet,
// touch device and switch the kernel reads, with what each is, how it is
// connected, how often it is read and what it has. Read when the card's
// button is pressed, and this file with it. It changes nothing, and reads
// no key.

import { fill, h } from '../../dom.js';
import { setOff } from './cards.js';
import { HELD, inputsOf, NO_INPUTS } from './inputs-model.js';

const COLUMNS = ['Device', 'Is', 'Connected', 'Read', 'Has'];

/** A table of `rows` whose caption is `title`; the rows turn into cards on a phone. */
const tableOf = (title, rows) => h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
  h('caption', {}, title),
  h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
  h('tbody', {}, rows.map((row) => h('tr', {},
    h('td', { 'data-label': COLUMNS[0], class: row.muted ? 'muted' : null }, h('b', {}, row.name), h('small', {}, row.small)),
    h('td', { 'data-label': COLUMNS[1] }, row.is),
    h('td', { 'data-label': COLUMNS[2] }, row.connected, row.where && h('small', {}, row.where)),
    h('td', { 'data-label': COLUMNS[3] }, row.read),
    h('td', { 'data-label': COLUMNS[4] }, row.has))))));

/** Reads the input devices and draws them into `card` (made by makeCard); `one` is the button that was pressed. */
export async function drawInputs(ctx, card, one) {
  const { view } = ctx;
  setOff(one, true);
  try {
    const { held, rows } = inputsOf(await view.api.get('/api/1/hardware/inputs'));
    fill(card.errors);
    fill(card.body,
      rows.length === 0 && h('p', {}, NO_INPUTS),
      held && h('p', {}, HELD),
      rows.length > 0 && tableOf(`Input devices · ${rows.length}`, rows));
    card.heading.focus();
  } catch (error) {
    setOff(one, false);
    view.problem(error, { slot: card.errors });
  }
}
