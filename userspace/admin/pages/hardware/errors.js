// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Hardware errors card's body: one calm line when the kernel has heard of
// no hardware error since this start, else a line for the worst of them, the
// newest ten in a table under the group they belong to (the firmware's record
// from before this start, and the rest), and the memory devices that have
// errors. Two buttons sit under the heading: Show all, which lists every error
// the record keeps in place and then reads Show the last ten, and Save as a
// file. It is read when the page opens and again when the page reads the rest,
// and this file with it, which a machine with no error never needs more of.

import { fill, h } from '../../dom.js';
import { saveText } from '../../lib/save-file.js';
import { button } from './cards.js';
import {
  CAPTION, deviceRows, errorGroups, hiddenCount, isAnswer, NOT_READ, summaryOf,
} from './errors-model.js';

const COLUMNS = ['When', 'Part', 'What happened', 'Severity', 'Found by'];
export const FILE = 'hardware-errors.json';

/** The table of `groups`; a group's header spans the table, and the rows turn into cards on a phone. */
const tableOf = (caption, groups) => h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
  h('caption', {}, caption),
  h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
  groups.map((group) => h('tbody', { 'data-group': group.key },
    h('tr', { class: 'hw-grp' }, h('th', { scope: 'colgroup', colspan: String(COLUMNS.length) }, group.title)),
    group.rows.map((row) => h('tr', {},
      h('td', { 'data-label': COLUMNS[0] }, row.when),
      h('td', { 'data-label': COLUMNS[1] }, row.part),
      h('td', { 'data-label': COLUMNS[2] }, row.what, row.count > 1 && h('small', {}, `${row.count} times`)),
      h('td', { 'data-label': COLUMNS[3] }, h('span', { class: `st ${row.severity.tone}` }, row.severity.word)),
      h('td', { 'data-label': COLUMNS[4] }, row.found)))))));

/** The memory devices that have errors, as a list. */
const devicesOf = (rows) => h('dl', { class: 'rows' }, rows.flatMap((row) => [
  h('dt', {}, row.name, row.size && h('small', {}, row.size)),
  h('dd', {}, row.words),
]));

/** The buttons under the heading, made once for `card` (cards.js, with a `.btns` row) and kept in `card.errorsView`. */
function viewOf(card, redraw) {
  const view = { all: false, answer: null };
  view.toggle = button('Show all', () => {
    view.all = !view.all;
    redraw();
  });
  view.save = button('Save as a file', () => saveText(FILE, `${JSON.stringify(view.answer, null, 2)}\n`));
  // The place for them is the card's, which page.js made with the card, between the place for a refusal and the body.
  view.row = card.element.querySelector('.btns');
  fill(view.row, view.toggle, view.save);
  return view;
}

/** Draws `answer` (`GET /api/1/hardware/errors`, undefined when it could not be read) into `card`, made by cards.js. */
export function drawErrors(ctx, card, answer) {
  const { data, view } = ctx;
  card.errorsView ??= viewOf(card, () => drawErrors(ctx, card, card.errorsView.answer));
  const mine = card.errorsView;
  if (!isAnswer(answer)) {
    mine.answer = null;
    mine.row.hidden = true;
    card.head();
    fill(card.body, h('p', {}, NOT_READ));
    return;
  }
  mine.answer = answer;
  const kept = answer.errors.length;
  const more = hiddenCount(answer) > 0;
  if (!more) mine.all = false;
  mine.row.hidden = false;
  mine.toggle.hidden = !more;
  mine.toggle.textContent = mine.all ? 'Show the last ten' : `Show all ${kept}`;
  const summary = summaryOf(answer);
  card.head(summary.pill);
  const groups = errorGroups(answer, { up: data.about?.up, now: Math.floor(Date.now() / 1000), time: view.time }, mine.all);
  const devices = deviceRows(answer);
  fill(card.body,
    h('p', {}, summary.lead),
    groups.length > 0 && tableOf(mine.all ? `All ${kept} hardware errors the record keeps` : CAPTION, groups),
    devices.length > 0 && h('p', {}, h('b', {}, 'Memory devices with errors')),
    devices.length > 0 && devicesOf(devices));
}
