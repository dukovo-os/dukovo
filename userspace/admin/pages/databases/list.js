// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of databases: a line about the server, and a table of each database
// with its owner, who connects, its size and its sessions, which becomes a card
// for each on a phone. Listing signs in to PostgreSQL as its superuser, so a
// reader is told who sees it, and an administrator whose console is locked is
// asked to unlock; see read.js.

import { fill, h, icon } from '../../dom.js';
import { problemsBanner, serverCard } from './cards.js';
import { rowsOf, serverState } from './model.js';
import { makeRead } from './read.js';

/** The line under the table. */
const OWN_WORDS = 'PostgreSQL\'s own databases are not listed.';

/** The table of `rows`. */
function tableOf(rows) {
  return h('div', { class: 'tbl-box' },
    h('table', { class: 'tbl stack' },
      h('caption', { class: 'sr-only' }, 'Databases: each with its owner, who connects, its size and its sessions'),
      h('thead', {}, h('tr', {}, ['Database', 'Owner', 'Who connects', 'Size', 'Sessions'].map((title) => h('th', { scope: 'col' }, title)))),
      h('tbody', {}, rows.map((row) => h('tr', {},
        h('td', { 'data-label': 'Database' },
          h('a', { href: row.href }, h('b', {}, row.name)),
          row.site && h('small', {}, `for ${row.site}`),
          row.mismatch && h('small', { class: 'warn' }, 'Its indexes need to be rebuilt')),
        h('td', { 'data-label': 'Owner', class: 'mono' }, row.owner),
        h('td', { 'data-label': 'Who connects' }, row.who.map((line) => h('div', {}, line))),
        h('td', { 'data-label': 'Size' }, row.size),
        h('td', { 'data-label': 'Sessions', class: 'num' }, String(row.sessions)))))));
}

export async function showList(view) {
  view.head({
    title: 'Databases',
    crumbs: [{ title: 'Apps', path: '/apps' }],
    commands: true,
    buttons: [h('a', { class: 'btn default', href: '/databases/add' }, icon('plus'), 'Add a database')],
  });
  const errors = h('div', { class: 'db-errors' });
  const read = makeRead(view, { errors });
  const body = h('div', { class: 'db-list' });
  view.main.append(h('div', { class: 'db-page' }, errors, read.element, body));

  function draw() {
    const data = read.data;
    if (read.mode !== 'open') {
      fill(body);
      return;
    }
    if (data === null) {
      const again = h('button', { class: 'btn', type: 'button' }, 'Try again');
      again.addEventListener('click', () => read.read());
      fill(body, h('p', { class: 'muted' }, 'The databases could not be read.'), h('div', { class: 'btns' }, again));
      return;
    }
    const on = serverState(data.server) === 'on';
    const again = h('button', { class: 'btn small', type: 'button', 'data-action': 'read-again' }, 'Read again');
    again.addEventListener('click', () => read.read());
    fill(body,
      serverCard(view, data, () => read.read()),
      on && problemsBanner(data),
      on && (data.databases.length === 0
        ? h('div', { class: 'db-empty' }, h('p', {}, h('b', {}, 'No databases yet.')), h('a', { class: 'btn default', href: '/databases/add' }, icon('plus'), 'Add a database'))
        : tableOf(rowsOf(data))),
      on && h('div', { class: 'db-foot' }, h('p', { class: 'small muted' }, OWN_WORDS), again, view.api.inline('databases.list', {})));
  }

  read.on(draw);
  await read.load();
}
