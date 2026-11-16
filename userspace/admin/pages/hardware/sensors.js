// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Temperatures and fans card's body: a row for each temperature the
// machine reads, each disk and each thermal zone, with what it is now, the
// highest of the last 30 days and its limit, then a line for each fan. A
// temperature near its limit says so in words. The choice of degrees Celsius
// or Fahrenheit is kept in this browser. The page reads again every minute;
// the rows are drawn again from what it read, so that a change of unit is at
// once. It is read with the first answer, which the card waits for anyway.

import { fill, h } from '../../dom.js';
import { keep, recall } from '../../lib/store.js';
import {
  ALERT_WITH_DISKS, ALERT_WORDS, fanRows, NO_FAN, NO_TEMPERATURE, noRecordWords, startTimes, tempRows, UNIT_KEY, UNITS, unitOf,
} from './sensors-model.js';

const COLUMNS = ['Sensor', 'Now', 'Highest, 30 days', 'Limit'];

/** A cell: its words, muted or with a state word after them, and a note under. */
const cellOf = (cell) => [
  cell.text && (cell.muted ? h('span', { class: 'muted' }, cell.text) : cell.text),
  cell.words && [cell.text && ' ', h('span', { class: `st ${cell.tone}` }, cell.words)],
  cell.note && h('small', {}, cell.note),
];

/** The table of `rows`, which turn into cards on a phone. */
const tableOf = (rows) => h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
  h('caption', { class: 'sr-only' }, 'Temperatures now, and the highest of the last 30 days'),
  h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
  h('tbody', {}, rows.map((row) => h('tr', {},
    h('td', { 'data-label': COLUMNS[0] }, h('b', {}, row.name), row.small && h('small', {}, row.small)),
    [row.now, row.highest, row.limit].map((cell, at) => h('td', { 'data-label': COLUMNS[at + 1] }, cellOf(cell))))))));

/** The body of `card`, made by cards.js, drawn from `ctx` (see page.js). Gives `{draw}`. */
export function makeTemps(ctx, card) {
  let unit = unitOf(recall(UNIT_KEY));
  const buttons = UNITS.map((id) => h('button', { type: 'button', 'data-unit': id }, `°${id}`));
  for (const one of buttons) {
    one.addEventListener('click', () => {
      unit = one.getAttribute('data-unit');
      keep(UNIT_KEY, unit);
      draw();
    });
  }
  const rows = h('div', { class: 'hw-body' });
  fill(card.body, h('div', { class: 'hw-unit' },
    h('span', { id: 'temps-unit', class: 'muted' }, 'Show in'),
    h('span', { class: 'seg', role: 'group', 'aria-labelledby': 'temps-unit' }, buttons)), rows);

  function draw() {
    const { data, view } = ctx;
    for (const one of buttons) {
      const on = one.getAttribute('data-unit') === unit;
      one.setAttribute('aria-pressed', String(on));
      one.classList.toggle('on', on);
    }
    if (data.sensors === undefined) {
      fill(rows, h('p', {}, 'The temperatures and fans could not be read.'));
      return;
    }
    const { rows: found, none } = tempRows(data.sensors, data.highest, {
      unit, time: view.time, now: Math.floor(Date.now() / 1000), starts: startTimes(data.updates),
    });
    const fans = fanRows(data.sensors);
    const noRecord = noRecordWords(data.storage);
    fill(rows,
      none && h('p', {}, NO_TEMPERATURE),
      found.length > 0 && tableOf(found),
      found.length > 0 && h('p', { class: 'small muted' }, found.some((row) => row.name.startsWith('Disk ')) ? ALERT_WITH_DISKS : ALERT_WORDS),
      noRecord && h('p', { class: 'small muted' }, noRecord),
      fans.length > 0
        ? h('dl', { class: 'rows' }, fans.flatMap((fan) => [h('dt', {}, fan.name), h('dd', {}, fan.tone === 'ok' ? fan.words : h('span', { class: `st ${fan.tone}` }, fan.words))]))
        : h('p', {}, NO_FAN));
  }

  return { draw };
}
