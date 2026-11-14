// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Sessions (design 13.10): the person's terminals as a table, each with
// Open, Rename and End. Ending asks while a program runs in it.

import { h } from '../../dom.js';
import { kept } from './kept.js';
import { rowsOf } from './sessions-model.js';

const API = '/api/1/terminals';

export async function show(view) {
  view.head({
    title: 'Sessions',
    crumbs: [{ title: 'Terminal', path: '/terminal' }],
    buttons: [h('a', { class: 'btn', href: '/terminal?new' }, 'New terminal')],
    lead: 'Terminals keep running when you close this page, until the console restarts. Open one here to see what it showed.',
  });
  const body = h('tbody');
  const box = h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
    h('thead', {}, h('tr', {}, ...['Name', 'Panes', 'Made', 'Last used', 'From', 'Open', 'Running a program'].map((title) => h('th', { scope: 'col' }, title)), h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Actions')))),
    body));
  const empty = h('p', { class: 'muted', hidden: true }, 'No terminal is open. ', h('a', { href: '/terminal?new' }, 'Open one'), '.');
  view.main.append(box, empty);

  let terminals = [];
  let list = null;

  const draw = () => {
    const rows = rowsOf(terminals, { time: view.time, mine: kept.panes });
    box.hidden = rows.length === 0;
    empty.hidden = rows.length > 0;
    body.replaceChildren(...rows.map((row) => h('tr', {},
      h('td', { 'data-label': 'Name' }, row.admin && h('span', { class: 'pill warn' }, 'administrator'), row.admin && ' ', h('b', {}, row.name)),
      h('td', { 'data-label': 'Panes', class: 'num' }, String(row.panes)),
      h('td', { 'data-label': 'Made', title: row.madeFull }, row.made),
      h('td', { 'data-label': 'Last used', title: row.usedFull }, row.used),
      h('td', { 'data-label': 'From', class: 'mono' }, row.from),
      h('td', { 'data-label': 'Open' }, row.open),
      h('td', { 'data-label': 'Running a program' }, row.running),
      h('td', { class: 'end' }, h('div', { class: 'btns' },
        h('a', { class: 'btn small', href: `/terminal?open=${row.id}`, 'aria-label': `Open ${row.name}` }, 'Open'),
        h('button', { class: 'btn small', type: 'button', 'aria-label': `Rename ${row.name}`, onclick: () => rename(row) }, 'Rename'),
        h('button', { class: 'btn small danger', type: 'button', 'aria-label': `End ${row.name}`, onclick: () => end(row) }, 'End'))))));
  };

  const refresh = async () => {
    try {
      list = await view.api.get(API);
      terminals = list.terminals;
      draw();
    } catch (error) {
      view.problem(error);
    }
  };

  async function rename(row) {
    const { renameDialog } = await import('./rename.js');
    if (await renameDialog(row.name, (name) => view.api.post(`${API}/${row.id}/rename`, { name })) !== null) refresh();
  }

  async function end(row) {
    if (row.asks) {
      const { ask } = await import('../../shell/dialogs.js');
      const choice = await ask({
        title: 'End this terminal?',
        words: `A program is running in ${row.name}, and it ends too.`,
        iconName: 'warn',
        choices: [{ label: 'End the terminal', value: 'end', kind: 'danger' }, { label: 'Cancel', value: 'cancel', kind: 'default', safe: true }],
      });
      if (choice !== 'end') return;
    }
    try {
      await view.api.post(`${API}/${row.id}/end`, {});
      view.notice(`Ended ${row.name}.`, { kind: 'ok' });
      refresh();
    } catch (error) {
      view.problem(error);
    }
  }

  await refresh();
  view.every(10_000, refresh);
}
