// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Reports from this server": the reports that wait and those sent, where
// each stands when the person asks, Retry now for the ones that wait, and
// Delete for one sent.

import {
  busy, fill, h, uniqueId,
} from '../../dom.js';
import { ask } from '../../shell/dialogs.js';
import {
  DELETE_WORDS, statusMap, tableRows, whyWords,
} from './sent-model.js';

const LIST = '/api/1/reports';

/** What each button says while it works, and when it is done, for the notice. */
const WORDS = {
  retry: {
    unlock: 'try the reports that wait again', running: 'Trying the reports that wait', done: 'The reports that wait were tried again.', unchanged: 'The reports that wait were tried again.', busy: 'Trying…',
  },
  delete: {
    unlock: 'delete a report', running: 'Deleting the report', done: 'The report was deleted.', unchanged: 'The report was deleted.', busy: 'Deleting…',
  },
};

/** Makes the card. `listed` says whether this server's admind offers the reports' list. */
export function makeSent(view, { listed = true } = {}) {
  const id = uniqueId('report-sent');
  let list = null;
  let status = null;
  let asked = null;
  const body = h('div', { class: 'rp-table' });
  const check = h('button', { class: 'btn small', type: 'button', hidden: true }, 'Check the status');
  const retry = h('button', { class: 'btn small', type: 'button', hidden: true }, 'Retry now');
  const element = h('section', { class: 'card', 'aria-labelledby': `${id}-h` },
    h('div', { class: 'rp-card-head' }, h('h2', { id: `${id}-h` }, 'Reports from this server'), h('div', { class: 'btns' }, retry, check)),
    body);

  /** Draws the table, or says that there is none. */
  function paint() {
    const rows = tableRows(list, { status, asked, time: view.time });
    check.hidden = !rows.some((row) => row.canDelete);
    retry.hidden = !rows.some((row) => row.queued);
    if (rows.length === 0) {
      fill(body, h('p', { class: 'note' }, 'No report has been sent from this server.'));
      return;
    }
    fill(body, h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
      h('thead', {}, h('tr', {}, ...['Sent', 'Problem', 'Status'].map((title) => h('th', { scope: 'col' }, title)), h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Actions')))),
      h('tbody', {}, rows.map((row) => h('tr', {},
        h('td', { 'data-label': 'Sent' }, row.sent),
        h('td', { 'data-label': 'Problem' }, row.title),
        h('td', { 'data-label': 'Status' }, h('span', { class: `st ${row.kind}` }, row.status), row.note && h('small', {}, row.note)),
        h('td', { class: 'end' }, row.canDelete && h('button', {
          class: 'btn small', type: 'button', 'aria-label': `Delete report ${row.number}`, onclick: (event) => remove(row, event.currentTarget ?? event.target),
        }, 'Delete'))))))));
  }

  /** Reads the reports again. */
  async function reload() {
    if (!listed) {
      fill(body, h('p', { class: 'note' }, 'This server\'s admind does not offer this yet; run ', h('code', {}, 'report list'), '.'));
      return;
    }
    try {
      list = await view.api.get(LIST);
    } catch (error) {
      if (error?.name === 'AbortError') return;
      fill(body);
      view.problem(error, { slot: body });
      return;
    }
    paint();
  }

  /** Asks the reports server where each report sent stands. */
  check.addEventListener('click', () => busy([check], async () => {
    try {
      status = statusMap(await view.api.get(`${LIST}/status`));
      asked = null;
    } catch (error) {
      if (error?.name === 'AbortError') return;
      asked = { why: whyWords(error) };
    }
    paint();
  }));

  /** Tries the reports that wait now; `button` is the one pressed. */
  async function tryAgain(button) {
    const result = await view.api.run('reports.retry', {}, { button, words: WORDS.retry, where: { slot: body } });
    await reload();
    return result;
  }
  retry.addEventListener('click', () => tryAgain(retry));

  async function remove(row, button) {
    const choice = await ask({
      title: `Delete report ${row.number}?`,
      words: DELETE_WORDS,
      iconName: 'warn',
      choices: [{ label: 'Keep it', value: 'keep', kind: 'default', safe: true }, { label: 'Delete', value: 'delete', kind: 'danger' }],
    });
    if (choice !== 'delete') return;
    const result = await view.api.run('reports.delete', { number: row.number }, { button, words: WORDS.delete, where: { slot: body } });
    if (result.ok) await reload();
  }

  return { element, reload, retry: tryAgain };
}
