// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of services (design 4.3): one table, the failed services first,
// and a filter whose words and state are in the address. It is read again
// every 10 seconds while the tab is shown, and when a task or a change says
// that something moved; what is drawn changes only when what it says does,
// so that a link in hand stays in hand.

import { fill, h } from '../../dom.js';
import { atMostEvery } from '../overview/refresh.js';
import {
  addressOf, captionOf, filterRows, queryOf, rowsOf, STATE_CHOICES,
} from './model.js';

/** How often the list is read again while the tab is shown. */
const READ_MS = 10_000;

/** The least time between two reads that a task or a change asks for. */
const EVENT_GAP_MS = 5000;

/** How long after the last key the filter's result is said aloud. */
const SAY_MS = 600;

const COLUMNS = ['Service', 'Now', 'At boot', 'Since', 'What it does'];

/** The rows of grey bars that stand for the table while it is read. */
const SKELETON_ROWS = 5;

/** A grey bar that stands for a word not read yet. */
const bar = () => h('span', { class: 'svc-bar', 'aria-hidden': 'true' });

/** A row of the table, from `{row, since, full}`: the model's row and the time words of its state. */
function rowOf({ row, since, full }) {
  return h('tr', { 'data-name': row.name },
    h('th', { scope: 'row', class: 'svc-name' }, h('a', { href: row.href }, row.name)),
    h('td', { 'data-label': 'Now' },
      h('span', { class: `st ${row.state.kind}` }, row.state.word),
      row.trying && [' ', h('span', { class: 'pill warn' }, 'Being tried')]),
    h('td', { 'data-label': 'At boot' }, h('span', { class: `pill ${row.atBoot.kind}`.trim() }, row.atBoot.word)),
    h('td', { 'data-label': 'Since' }, row.since === null ? null : h('time', { datetime: new Date(row.since * 1000).toISOString(), title: full }, since)),
    h('td', { 'data-label': 'What it does' }, row.description));
}

export async function show(view) {
  view.head({ title: 'Services' });

  let query = queryOf(view.query);
  let rows = null;
  let drawn = null;
  let failed = false;
  let latest = 0;

  const input = h('input', { id: 'svc-q', class: 'input', type: 'search', value: query.q, autocomplete: 'off', spellcheck: 'false' });
  const select = h('select', { id: 'svc-state', class: 'input' },
    ...STATE_CHOICES.map((choice) => h('option', { value: choice.id, selected: choice.id === query.state }, choice.words)));
  const caption = h('caption', { class: 'sr-only' });
  const body = h('tbody');
  const table = h('table', { class: 'tbl stack svc-tbl', 'aria-busy': 'true' },
    caption,
    h('thead', {}, h('tr', {}, ...COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
    body);
  const box = h('div', { class: 'tbl-box' }, table);
  const words = h('p', { class: 'muted svc-none', hidden: true });
  const said = h('p', { class: 'sr-only', role: 'status' });
  const content = h('div', { class: 'svc-list' },
    h('form', { class: 'svc-filter', role: 'search', onsubmit: (event) => event.preventDefault() },
      h('div', { class: 'field' }, h('label', { for: 'svc-q' }, 'Filter'), input),
      h('div', { class: 'field' }, h('label', { for: 'svc-state' }, 'State'), select)),
    box, words, said);
  view.main.append(content);

  const skeleton = () => fill(body, Array.from({ length: SKELETON_ROWS }, () => h('tr', { 'aria-hidden': 'true' },
    h('th', { scope: 'row' }, bar()), ...COLUMNS.slice(1).map((name) => h('td', { 'data-label': name }, bar())))));
  skeleton();

  /** Says how many rows the filter leaves, a moment after the last key. */
  let sayTimer = null;
  const say = () => {
    clearTimeout(sayTimer);
    sayTimer = setTimeout(() => { said.textContent = caption.textContent; }, SAY_MS);
  };
  view.signal.addEventListener('abort', () => clearTimeout(sayTimer), { once: true });

  /** Draws the rows that pass the filter, if what they say is not what is drawn. */
  function draw() {
    const shown = filterRows(rows, query);
    const lines = shown.map((row) => ({ row, since: row.since === null ? '' : view.time.when(row.since), full: row.since === null ? '' : view.time.full(row.since) }));
    const key = JSON.stringify(lines);
    caption.textContent = captionOf(shown.length, rows.length);
    table.removeAttribute('aria-busy');
    box.hidden = shown.length === 0;
    words.hidden = shown.length > 0;
    if (shown.length === 0) {
      words.replaceChildren(...(rows.length === 0
        ? ['Spark lists no services.']
        : ['No service matches the filter. ', h('button', { class: 'btn link', type: 'button', onclick: clear }, 'Show every service')]));
    }
    if (key === drawn) return;
    drawn = key;
    const held = body.contains(document.activeElement) ? document.activeElement.closest('tr')?.dataset.name : null;
    fill(body, lines.map(rowOf));
    if (held) body.querySelector(`tr[data-name="${CSS.escape(held)}"] a`)?.focus();
  }

  /** Puts the filter in the address, without a step in the history for each key. */
  function filtered() {
    query = { q: input.value, state: select.value };
    history.replaceState(history.state, '', addressOf(query));
    if (rows !== null) {
      draw();
      say();
    }
  }

  function clear() {
    input.value = '';
    select.value = 'any';
    filtered();
    input.focus();
  }

  input.addEventListener('input', filtered);
  select.addEventListener('change', filtered);

  /** Reads the services. Only the last read counts, whatever order they answer in. */
  async function read() {
    latest += 1;
    const mine = latest;
    try {
      const answer = await view.api.get('/api/1/services');
      if (mine !== latest) return;
      rows = rowsOf(answer, Math.floor(Date.now() / 1000));
      content.hidden = false;
      if (failed) {
        failed = false;
        view.slot.replaceChildren();
      }
      draw();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      // What is drawn stays while the server does not answer; the first read that fails leaves nothing to show.
      if (rows === null) content.hidden = true;
      if (!failed) {
        failed = true;
        view.problem(error, { buttons: [h('button', { class: 'btn small', type: 'button', onclick: again }, 'Try again')] });
      }
    }
  }

  function again() {
    view.slot.replaceChildren();
    failed = false;
    content.hidden = false;
    if (rows === null) skeleton();
    read();
  }

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.on('task', soon);
  view.every(READ_MS, read);
  await read();
}
