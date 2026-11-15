// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The full journal (design 7.3) at /reliability/changes: every change the
// journal gives, newest first, 50 at a time, filtered by who made it, how
// and on which day; each opens the dialog of its diffs, and the changes that
// were made through the console and worked can be chosen and copied as a
// script that makes the same changes on another machine. The filters are
// in the address, so that a link keeps them.

import { fill, h } from '../../dom.js';
import { commandBox } from '../overview/card.js';
import { makeChangeActions } from '../overview/change-actions.js';
import { changeRow } from '../overview/change-model.js';
import {
  addressOf, dayChoices, filterRows, HOWS, PAGE, queryOf, READ_PATH, scriptable, scriptOf, whoChoices,
} from './journal-model.js';
import { CHANGES_READ, pageOf } from './reads-model.js';

/** What a screen reader says of a dot, which the colour only repeats. */
const SAID = Object.freeze({
  warn: 'Needs a look', bad: 'Did not work', ok: 'Kept', info: '',
});

/** A labelled choice of `options`, `{value, label}`, with `first` standing for no filter; `current` is kept in the list when the address named something the rows do not have. */
function choice(id, label, first, options, current, pick) {
  const list = current !== '' && !options.some((one) => one.value === current) ? [...options, { value: current, label: current }] : options;
  const select = h('select', { id, class: 'input' },
    h('option', { value: '' }, first),
    ...list.map((one) => h('option', { value: one.value, selected: one.value === current }, one.label)));
  select.addEventListener('change', () => pick(select.value));
  return h('div', { class: 'field' }, h('label', { for: id }, label), select);
}

export async function show(view) {
  view.head({ title: 'Journal', crumbs: [{ title: 'Reliability', path: '/reliability' }] });

  let query = queryOf(view.query);
  let changes = [];
  // Who runs each action, to tell which changes can be a line of a script; null when the list could not be read.
  let runs = null;
  let shown = PAGE;
  let latest = 0;
  const picked = new Set();

  const actions = makeChangeActions(view, { prefix: 'rel-change', lazy: true, done: () => read() });
  const filters = h('div', { class: 'rel-filters' });
  const all = h('button', { class: 'btn small', type: 'button' }, 'Choose all shown');
  const copy = h('button', { class: 'btn small default', type: 'button' }, 'Copy as a script');
  const count = h('span', { class: 'small muted', role: 'status' });
  const notOffered = h('p', { class: 'small muted' }, 'Copy as a script is not offered: the console could not read which commands run as an administrator.');
  const toolbar = h('div', { class: 'rel-toolbar' }, h('div', { class: 'btns' }, all, copy, count), notOffered);
  const scriptSlot = h('div', { class: 'rel-script' });
  const table = h('div', { class: 'rel-journal' });
  const status = h('p', { class: 'small muted', role: 'status' });
  const content = h('section', { class: 'card', 'aria-labelledby': 'rel-journal-h' },
    h('h2', { id: 'rel-journal-h' }, 'What changed'), filters, toolbar, scriptSlot, table, status);
  view.main.append(h('div', { class: 'rel' }, content));

  // ---- the script ----

  /** The rows that pass the filters and are shown now. */
  const shownRows = () => pageOf(filterRows(changes, query, view.time.zone), shown);

  /** The buttons over the table: what can be chosen among the rows shown, and how many are. */
  function refreshToolbar() {
    toolbar.querySelector('.btns').hidden = runs === null;
    notOffered.hidden = runs !== null;
    if (runs === null) return;
    all.hidden = !shownRows().shown.some((change) => scriptable(change, runs));
    count.textContent = `${picked.size} chosen`;
  }

  all.addEventListener('click', () => {
    for (const change of shownRows().shown) if (scriptable(change, runs)) picked.add(change.id);
    for (const tick of table.querySelectorAll('input[type="checkbox"]')) tick.checked = true;
    refreshToolbar();
  });

  copy.addEventListener('click', () => {
    const text = scriptOf(changes.filter((change) => picked.has(change.id)), runs);
    if (text === '') {
      scriptSlot.replaceChildren(h('p', { class: 'muted', role: 'status' }, 'Choose the changes to copy first.'));
      return;
    }
    const box = commandBox(text, { title: 'Script of the chosen changes' });
    scriptSlot.replaceChildren(box);
    // The box's own button copies, and says whether it could.
    box.querySelector('.btn').click();
  });

  // ---- drawing ----

  function rowOf(change) {
    const row = changeRow(change, view.time);
    const tick = runs !== null && scriptable(change, runs)
      ? h('input', { type: 'checkbox', id: `rel-pick-${change.id}`, 'aria-label': `Choose: ${row.title}`, checked: picked.has(change.id) })
      : null;
    tick?.addEventListener('change', () => {
      if (tick.checked) picked.add(change.id);
      else picked.delete(change.id);
      refreshToolbar();
    });
    return h('tr', { 'data-id': change.id },
      h('td', { class: 'rel-pick' }, tick && h('label', { class: 'rel-pick-box' }, tick)),
      h('th', { scope: 'row' }, h('time', { datetime: new Date(change.at * 1000).toISOString(), title: view.time.full(change.at) }, row.time)),
      h('td', { 'data-label': 'What changed' },
        h('span', { class: `ev ${row.dot === 'ok' ? 'info' : row.dot}`, 'aria-hidden': 'true' }), ' ',
        h('b', {}, h('span', { class: 'sr-only' }, SAID[row.dot]), row.title),
        h('span', { class: 'muted small rel-sub' }, row.sub)),
      h('td', { class: 'end' }, h('span', { class: 'btns' }, ...actions.buttons(change, row, null))));
  }

  /** The filters, drawn when the journal is read: the choices are the rows' own. */
  function drawFilters() {
    const pick = (name) => (value) => {
      query = { ...query, [name]: value === '' && name === 'how' ? 'any' : value };
      shown = PAGE;
      history.replaceState(null, '', addressOf(query));
      draw();
    };
    fill(filters,
      choice('rel-who', 'Who', 'Anyone', whoChoices(changes), query.who, pick('who')),
      choice('rel-how', 'How', 'Any way', HOWS.filter((one) => one.id !== 'any').map((one) => ({ value: one.id, label: one.label })), query.how === 'any' ? '' : query.how, pick('how')),
      choice('rel-day', 'Day', 'Any day', dayChoices(changes, view.time.zone), query.day, pick('day')));
  }

  /** The table of the rows that pass the filters, a page of them; `focusNew` puts focus on the first row that "Show older" added. */
  function draw(focusNew = false) {
    const filtered = filterRows(changes, query, view.time.zone);
    const rows = pageOf(filtered, shown);
    const older = h('button', { class: 'btn small', type: 'button' }, `Show older (${rows.left} more)`);
    older.addEventListener('click', () => {
      shown += PAGE;
      draw(true);
    });
    fill(table,
      rows.shown.length === 0
        ? h('p', { class: 'muted' }, changes.length === 0 ? 'The journal has no change yet.' : 'No change passes these filters.')
        : h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack rel-tbl' },
          h('caption', { class: 'sr-only' }, `Changes, ${rows.shown.length} of ${filtered.length} shown, newest first`),
          h('thead', {}, h('tr', {},
            h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Choose')), h('th', { scope: 'col' }, 'When'), h('th', { scope: 'col' }, 'What changed'), h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'What you can do')))),
          h('tbody', {}, ...rows.shown.map(rowOf)))),
      rows.left > 0 && h('div', { class: 'btns' }, older));
    status.textContent = changes.length >= CHANGES_READ ? `The journal lists the newest ${CHANGES_READ} changes; the console cannot ask for older ones.` : '';
    refreshToolbar();
    if (focusNew) table.querySelectorAll('tbody tr')[shown - PAGE]?.querySelector('button, input')?.focus();
  }

  // ---- reading ----

  /** Reads the journal and which commands run as an administrator, and draws. Only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    try {
      const [answer, list] = await Promise.all([view.api.get(READ_PATH), view.api.get('/api/1/actions').catch(() => null)]);
      if (view.signal.aborted || mine !== latest) return;
      changes = Array.isArray(answer?.changes) ? answer.changes : [];
      runs = Array.isArray(list) ? new Map(list.map((one) => [one.id, one.runs])) : null;
      // A change that is gone from the journal is no longer chosen.
      for (const id of picked) if (!changes.some((change) => change.id === id)) picked.delete(id);
      drawFilters();
      draw();
    } catch (error) {
      if (view.signal.aborted || mine !== latest || error?.name === 'AbortError') return;
      content.hidden = true;
      view.problem(error, {
        buttons: [h('button', {
          class: 'btn small',
          type: 'button',
          onclick() {
            view.slot.replaceChildren();
            content.hidden = false;
            read();
          },
        }, 'Try again')],
      });
    }
  }

  await read();
}
