// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Devices card's list: the devices of the machine in a table for each
// group, what each is to the system, its driver, its state and where it is,
// with the ones that work and need nothing folded away. Read when the card's
// button is pressed, and this file with it.

import { fill, h } from '../../dom.js';
import { setOff } from './cards.js';
import { devicesOf, hiddenWords, NO_DEVICES } from './devices-model.js';

const COLUMNS = ['Device', 'Driver', 'State', 'Where'];

/** A table of `rows` whose caption is `title`; the rows turn into cards on a phone. */
const tableOf = (title, rows) => h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
  h('caption', {}, title),
  h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
  h('tbody', {}, rows.map((row) => h('tr', {},
    h('td', { 'data-label': COLUMNS[0] }, h('b', {}, row.name), row.small && h('small', {}, row.small)),
    h('td', { 'data-label': COLUMNS[1], class: row.driver.muted ? 'muted' : null }, row.driver.text),
    h('td', { 'data-label': COLUMNS[2] }, row.state ? h('span', { class: `st ${row.state.tone}` }, row.state.word) : h('span', { class: 'muted' }, '—')),
    h('td', { 'data-label': COLUMNS[3] }, row.where))))));

/** Reads the devices and draws them into `card` (made by makeCard); `one` is the button that was pressed. */
export async function drawDevices(ctx, card, one) {
  const { view } = ctx;
  setOff(one, true);
  try {
    const { groups, hidden } = devicesOf(await view.api.get('/api/1/hardware/devices'));
    fill(card.errors);
    fill(card.body,
      groups.length === 0 && hidden.length === 0 && h('p', {}, NO_DEVICES),
      groups.map((group) => tableOf(`${group.title} · ${group.rows.length}`, group.rows)),
      hidden.length > 0 && h('details', {}, h('summary', {}, hiddenWords(hidden.length)), tableOf('Hidden devices', hidden)),
      h('p', { class: 'small muted' }, 'A device with no driver that you need? ', h('a', { href: '/report?about=hardware' }, 'Report it'), '; the report carries its IDs (vendor and device numbers, never a serial).'));
    card.heading.focus();
  } catch (error) {
    setOff(one, false);
    view.problem(error, { slot: card.errors });
  }
}
