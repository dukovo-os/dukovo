// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Connections card of the Networking page (design 9.3): the TCP
// connections the machine holds now, from `ss -t`, which everyone may read.
// The page reads them with its cards, so the rows are drawn again only when
// what they say changes.

import { fill, h } from '../../dom.js';
import { connectionRows } from './network-model.js';

const COLUMNS = ['This machine', 'The other end', 'State'];

/** The card. `set(answer)` draws the rows of `ss -t --json`, `fail(error)` says they could not be read. */
export function makeConnections(view) {
  const caption = h('caption', { class: 'sr-only' }, 'TCP connections now');
  const body = h('tbody');
  const table = h('table', { class: 'tbl stack net-conn' }, caption,
    h('thead', {}, h('tr', {}, ...COLUMNS.map((name) => h('th', { scope: 'col' }, name)))), body);
  const box = h('div', { class: 'tbl-box' }, table);
  const errors = h('div', { class: 'net-errors' });
  const empty = h('p', { class: 'muted', hidden: true }, 'No TCP connections are open now.');
  const element = h('section', { class: 'card net-connections', 'aria-labelledby': 'net-conn-h', hidden: true },
    h('h2', { id: 'net-conn-h' }, 'Connections'), errors, box, empty);
  let last = null;
  return {
    element,
    set(answer) {
      element.hidden = false;
      errors.replaceChildren();
      const rows = connectionRows(answer, view.session);
      const key = JSON.stringify(rows);
      box.hidden = rows.length === 0;
      empty.hidden = rows.length > 0;
      caption.textContent = `TCP connections now: ${rows.length}`;
      if (key === last) return;
      last = key;
      fill(body, rows.map((row) => h('tr', {},
        h('th', { scope: 'row', class: 'mono', 'data-label': COLUMNS[0] }, row.local),
        h('td', { class: 'mono', 'data-label': COLUMNS[1] }, row.remote, row.thisBrowser && [' ', h('span', { class: 'pill info' }, 'This browser')]),
        h('td', { 'data-label': COLUMNS[2] }, row.state))));
    },
    fail(error) {
      element.hidden = false;
      view.problem(error, { slot: errors });
    },
  };
}
