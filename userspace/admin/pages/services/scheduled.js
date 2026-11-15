// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Scheduled tasks page (design 4.5): each service that has a timer, when
// it runs, when it runs next and last, and how the last run ended. Only an
// unlocked administrator reads the timers (timerd answers root alone, design
// 3.4), so the page reads them when it opens, when the person asks again and
// when the console is unlocked or locked, never on a timer.

import { fill, h } from '../../dom.js';
import { makeReads } from '../overview/admin-reads.js';
import { timerHeaders, timerRows, TIMER_NOTES } from './timer-model.js';

/** The words of the columns that a phone's card names each value by. */
const LABELS = ['Task', 'When', 'Next', 'Last', 'Result'];

/** A time as the person's page writes it: its words, with the whole time under the pointer. */
const stampNode = (stamp) => h('time', { datetime: stamp.iso, title: stamp.full }, stamp.words);

/** A row of the table, from `timerRows`. */
function rowOf(row) {
  return h('tr', { 'data-name': row.service },
    h('th', { scope: 'row', class: 'svc-name' }, h('a', { href: row.href }, row.service)),
    h('td', { 'data-label': LABELS[1] }, row.when, row.written && h('span', { class: 'muted small svc-written' }, row.written)),
    h('td', { 'data-label': LABELS[2] }, row.next ? stampNode(row.next) : '–'),
    h('td', { 'data-label': LABELS[3] }, row.last ? stampNode(row.last) : 'Never'),
    h('td', { 'data-label': LABELS[4] }, h('span', { class: `st ${row.result.kind}` }, row.result.words)));
}

export async function show(view) {
  view.head({ title: 'Scheduled tasks', crumbs: [{ title: 'Services', path: '/services' }], commands: true });
  const reads = makeReads(view);
  const body = h('div', { class: 'svc-body', 'aria-busy': 'true' }, ...[0, 1, 2].map(() => h('span', { class: 'svc-bar wide', 'aria-hidden': 'true' })));
  const card = h('section', { class: 'card' }, body);
  view.main.append(h('div', { class: 'svc-list' }, card));
  let latest = 0;

  /** The table of `timers`, or the words that say there are none. */
  function draw(timers) {
    const rows = timerRows(timers, { time: view.time, nowMs: Date.now() });
    if (rows.length === 0) {
      fill(body, h('p', { class: 'muted' }, 'No service has a timer.'));
      return;
    }
    const again = h('button', { class: 'btn small', type: 'button', onclick: load }, 'Read again');
    fill(body,
      h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack svc-tbl' },
        h('caption', { class: 'sr-only' }, `Scheduled tasks, ${rows.length}, soonest first`),
        h('thead', {}, h('tr', {}, ...timerHeaders(rows).map((words) => h('th', { scope: 'col' }, words)))),
        h('tbody', {}, ...rows.map(rowOf)))),
      h('p', { class: 'muted small' }, TIMER_NOTES),
      h('div', { class: 'btns' }, again),
      view.api.inline('service.timers', {}));
  }

  /** Reads the timers and draws them. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    const result = await reads.read('/api/1/timers', { admin: true });
    if (view.signal.aborted || mine !== latest) return;
    body.removeAttribute('aria-busy');
    if (result.state === 'ok') draw(result.value);
    else fill(body, reads.note(result, 'the scheduled tasks', load));
  }

  view.on('unlock', () => {
    reads.forget();
    load();
  });
  await load();
}
