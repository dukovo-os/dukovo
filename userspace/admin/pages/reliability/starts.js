// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Starts card and the card of a failed start (design 7.3): the starts the
// machine kept, newest first, each a link to its page, and what failed the
// newest one that failed, the changes since the last good start and the way
// to find which of them broke it. Both are made with `draw(result)`, which
// the page calls with what `reads.read` gave.

import { fill, h, icon } from '../../dom.js';
import { commandBox, makeCard } from '../overview/card.js';
import { pageOf } from './reads-model.js';
import { FAILED_COMMANDS, failedOf, startRows } from './starts-model.js';

/** The starts shown at first, and each time "Show older" is pressed. */
const SHOWN = 10;

/** A row of the table: the start's time is the link, since a row's link is on its name. */
function rowOf(row) {
  return h('tr', { 'data-start': row.number },
    h('th', { scope: 'row' }, h('a', { href: row.href }, row.started)),
    h('td', { 'data-label': 'Version' }, row.version),
    h('td', { class: 'num', 'data-label': 'Ready in' }, row.ready),
    h('td', { 'data-label': 'Result' }, row.result.words && h('span', { class: `st ${row.result.dot === 'bad' ? 'bad' : 'ok'}` }, row.result.words)));
}

/**
 * The Starts card: `{element, draw}`. `view` is the page's, `reads` its
 * `makeReads`, and `retry()` reads again.
 */
export function makeStarts(view, reads, retry) {
  const card = makeCard('starts', 'Starts');
  let shown = SHOWN;
  let last = null;

  function draw(result = last) {
    last = result;
    if (result.state !== 'ok') {
      fill(card.body, reads.note(result, 'the starts', retry));
      return;
    }
    const starts = Array.isArray(result.value) ? result.value : [];
    if (starts.length === 0) {
      fill(card.body, h('p', { class: 'muted' }, 'No start is kept yet.'));
      return;
    }
    const rows = pageOf(startRows(starts, view.time), shown);
    const older = h('button', { class: 'btn small', type: 'button' }, `Show older (${rows.left} more)`);
    older.addEventListener('click', () => {
      shown += SHOWN;
      draw();
    });
    fill(card.body,
      h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack rel-tbl' },
        h('caption', { class: 'sr-only' }, `Starts, ${rows.shown.length} of ${starts.length} shown, newest first`),
        h('thead', {}, h('tr', {},
          h('th', { scope: 'col' }, 'Started'), h('th', { scope: 'col' }, 'Version'), h('th', { scope: 'col', class: 'num' }, 'Ready in'), h('th', { scope: 'col' }, 'Result'))),
        h('tbody', {}, ...rows.shown.map(rowOf)))),
      rows.left > 0 && h('div', { class: 'btns' }, older));
  }
  return { element: card.element, draw };
}

/**
 * The card of a failed start: `{element, draw}`. It is left out while no
 * start kept has failed, and says why when the read did not work.
 */
export function makeFailed(view, reads, retry) {
  const card = makeCard('failed', 'A failed start');
  card.element.hidden = true;

  function draw(result) {
    if (result.state !== 'ok') {
      card.element.hidden = false;
      fill(card.body, reads.note(result, 'the failed start', retry));
      return;
    }
    const failed = failedOf(result.value, view.time);
    card.element.hidden = failed === null;
    if (failed === null) return;
    fill(card.body,
      h('p', {}, failed.sentence, ' ', h('a', { href: failed.href }, 'Open the start')),
      h('p', {}, failed.lead),
      failed.changes.length > 0 && h('ul', { class: 'rel-changes' }, ...failed.changes.map((words) => h('li', {}, words))),
      failed.running && h('div', { class: 'banner info' },
        icon('info'),
        h('div', {}, h('p', {}, failed.running.words), commandBox(failed.running.stop, { title: 'To stop the search' }))),
      failed.offer && h('div', { class: 'rel-offer' },
        h('p', {}, failed.offer.words),
        commandBox(failed.offer.command, { title: 'To find the change' }),
        h('p', { class: 'small muted' }, view.session.admin === true ? h('a', { href: failed.offer.run.href }, failed.offer.run.words) : failed.offer.later)),
      ...FAILED_COMMANDS.map((one) => commandBox(one.command, { title: one.title })));
  }
  return { element: card.element, draw };
}
