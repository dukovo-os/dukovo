// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The cards of what is active now and of what happened lately.

import { fill, h } from '../../dom.js';
import { card, nothing } from './cards.js';
import { listOf } from './channels-model.js';
import { activeRows, recentRows } from './model.js';

/** The Active card: critical alerts first, then the oldest, each with the page it is about. `ctx` is the page's: `{view, reads, load}`. */
export function makeActive(ctx) {
  const active = card('active', 'Active');
  const refresh = h('button', { class: 'btn small', type: 'button' }, 'Refresh');
  refresh.addEventListener('click', () => ctx.load());
  active.tools.append(refresh);
  return {
    element: active.element,
    set(result) {
      if (result.state !== 'ok') {
        fill(active.body, ctx.reads.note(result, 'the alerts', ctx.load));
        return;
      }
      const rows = activeRows(listOf(result.value), ctx.view.time);
      fill(active.body, rows.length === 0 ? nothing('No alerts are active.') : h('ul', { class: 'al-list' }, ...rows.map((row) => h('li', { class: 'al-item' },
        h('span', { class: `pill ${row.tone}` }, row.severity),
        h('div', {},
          h('b', {}, row.title),
          h('span', { class: 'muted small' }, ` ${[row.since, row.source].filter(Boolean).join(' · ')}`),
          row.detail && h('p', { class: 'al-detail' }, row.detail),
          row.page && h('a', { href: row.page.path }, `Open ${row.page.label}`))))));
    },
  };
}

/** The Recent card: the last changes, with a way to the rest. */
export function makeRecent(ctx) {
  const recent = card('recent', 'Recent');
  return {
    element: recent.element,
    set(result) {
      if (result.state !== 'ok') {
        fill(recent.body, ctx.reads.note(result, 'the alerts', ctx.load));
        return;
      }
      const rows = recentRows(listOf(result.value), ctx.view.time);
      fill(recent.body,
        rows.length === 0 ? nothing('No alert has been raised or cleared.') : h('ul', { class: 'al-list' }, ...rows.map((row) => h('li', { class: 'al-item' },
          h('span', { class: `al-dot ${row.tone}`, 'aria-hidden': 'true' }),
          h('div', {}, h('b', {}, row.title), h('span', { class: 'muted small' }, ` ${row.severity} · ${row.state} · ${row.when} · ${row.source}`))))),
        h('p', {}, h('a', { href: '/reliability#alerts' }, 'Show all on Reliability')));
    },
  };
}
