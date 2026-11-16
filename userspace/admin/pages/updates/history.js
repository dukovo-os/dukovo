// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The History card: the changes of the system and the firmware set by start,
// then what bundled did to the apps, newest first, 20 rows of each until the
// person asks for all. Read when the card is opened.

import { fill, h } from '../../dom.js';
import { appRows, OUTCOME, SHOWN, startRows } from './history-model.js';

const table = (head, rows) => h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
  h('thead', {}, h('tr', {}, ...head.map((name) => h('th', { scope: 'col' }, name)))),
  h('tbody', {}, ...rows.map((cells) => h('tr', {}, ...cells.map((cell, at) => h('td', { 'data-label': head[at] }, cell)))))));

/** Reads the history and draws it into `card` (made by makeCard); `one` is the button that was pressed. */
export async function drawHistory(ctx, card, one) {
  const { view } = ctx;
  one.disabled = true;
  try {
    const [doc, accounts] = await Promise.all([view.api.get('/api/1/updates/history'), view.api.get('/api/1/accounts').catch(() => [])]);
    const starts = startRows(doc);
    const apps = appRows(doc, Array.isArray(accounts) ? accounts : []);
    let all = false;
    const draw = () => {
      const cut = (rows) => (all ? rows : rows.slice(0, SHOWN));
      fill(card.body,
        h('h3', {}, 'The system and the firmware set'),
        starts.length === 0 ? h('p', {}, 'No change is kept.') : table(['When', 'What', 'How it went'], cut(starts).map((row) => [
          view.time.when(row.ms / 1000), row.words,
          [h('span', { class: `pill ${OUTCOME[row.outcome]?.[0] ?? ''}` }, OUTCOME[row.outcome]?.[1] ?? row.outcome), row.error && [' ', h('a', { href: `/help/errors/${row.error}` }, row.error)]],
        ])),
        h('h3', {}, 'Apps'),
        apps.length === 0 ? h('p', {}, 'Nothing was done to an app.') : table(['When', 'App', 'What', 'Version', 'By'], cut(apps).map((row) => [
          view.time.when(row.ms / 1000), row.app, row.what, row.pair ?? '', row.who,
        ])),
        !all && (starts.length > SHOWN || apps.length > SHOWN) && h('div', { class: 'btns' }, h('button', {
          class: 'btn', type: 'button', onclick: () => { all = true; draw(); },
        }, 'Show all')));
    };
    draw();
  } catch (error) {
    view.problem(error, { slot: card.errors });
    one.disabled = false;
  }
}
