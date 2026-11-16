// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Last check card: every check of the last result, a failure first, with
// the ID of a warning or a failure linked to its page and what to do, and the
// checks before it. What the check is doing while it runs is on the page's
// head (page.js); a check that did not finish says so here.

import { fill, h, icon } from '../../dom.js';
import { makeCard } from './cards.js';
import {
  checkRows, earlierRows, hasResult, NOT_CHECKED, READER_CHECK, stateOf,
} from './model.js';

/** The card, drawn from `ctx` (see page.js). Gives `{element, draw}`. */
export function makeCheck(ctx) {
  const card = makeCard('check', 'Last check');

  /** A check as a list item that starts with its result in words, and whose ID leads to its page. */
  const itemOf = (row) => h('li', { class: 'hw-check' },
    h('p', {}, h('span', { class: `st ${row.tone}` }, row.word), ' ', h('b', {}, row.name), h('span', { class: 'muted' }, ` · ${row.area}`)),
    row.detail && h('p', { class: row.result === 'skipped' ? 'muted small' : 'small' }, row.detail),
    row.id && h('p', { class: 'small' }, h('a', { class: 'mono', href: `/help/errors/${encodeURIComponent(row.id)}` }, row.id), row.advice && ` ${row.advice}`));

  function draw() {
    const { data, admin, view } = ctx;
    const { last } = data;
    const ended = data.ended && h('div', { class: 'banner bad', role: 'status' }, icon('warn'),
      h('div', {}, h('p', {}, h('b', {}, 'The check did not finish. '), data.ended)));
    if (last === undefined) {
      card.head(null);
      fill(card.body, ended, h('p', {}, 'The last check could not be read.'));
      return;
    }
    if (!hasResult(last)) {
      card.head(null);
      fill(card.body, ended, h('p', {}, NOT_CHECKED), !admin && h('p', { class: 'muted' }, READER_CHECK));
      return;
    }
    const earlier = earlierRows(data.history, last, view.time);
    card.head(stateOf(last));
    fill(card.body,
      ended,
      h('ul', { class: 'hw-checks' }, checkRows(last).map(itemOf)),
      earlier.length > 0 && h('details', { class: 'hw-earlier' },
        h('summary', {}, 'Earlier checks'),
        h('ul', {}, earlier.map((one) => h('li', {}, one.words)))));
  }

  return { element: card.element, draw };
}
