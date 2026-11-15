// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What is protected (design 5.4): a table of each place the machine keeps
// data in, with the snapshots on this server, the copies off it and the last
// check by a restore, so that a gap is seen in one look. On a phone each
// row is a card. It says nothing of a place's size, which needs Btrfs
// quotas, and it never says "protected" of what has only a snapshot.

import { fill, h } from '../../dom.js';
import { bars, failed } from './body.js';
import { copyBox } from './copy-box.js';
import { placeHref, protectedRows, skippedPlaces } from './model.js';

/** A state's words, with its colour repeated in a dot: a neutral one has a hollow dot. */
const state = (cell) => h('span', { class: `st ${cell.tone || 'off'}` }, cell.words);

/** The columns, by the words a stacked row labels each cell with. */
const COLUMNS = Object.freeze(['Data', 'Snapshots on this server', 'Copy off this server', 'Last restore check']);

/**
 * Makes the card. `jobs` is jobs.js's, for Check now; `journal` says
 * whether the page of the journal is in this console. Gives `{element,
 * set}`; `set(list, backups, {time, now})` draws the rows and leaves the
 * card as it is when what they say has not changed.
 */
export function makeProtected(view, { jobs, journal }) {
  const element = h('section', { class: 'card snap-prot', id: 'protected', 'aria-labelledby': 'snap-prot-h', 'aria-busy': 'true' },
    h('h2', { id: 'snap-prot-h' }, 'What is protected'),
    h('div', { class: 'snap-body' }, bars()));
  let last = null;

  /** One row of the table. */
  function row(one) {
    const copies = one.copies.length === 0
      ? [state({ words: 'Not copied off this server', tone: 'warn' })]
      : one.copies.map((copy) => h('span', { class: `st ${copy.tone || 'off'}` }, h('a', { href: placeHref(copy.name) }, copy.name), ` · ${copy.rest}`));
    const check = [state(one.check)];
    if (view.session.admin && one.check.name && one.check.words === 'Never') check.push(jobs.button('verify', one.check.name, 'Check now', { size: 'small' }));
    return h('tr', {},
      h('td', { 'data-label': COLUMNS[0] }, h('b', {}, one.name), h('small', {}, one.path)),
      h('td', { 'data-label': COLUMNS[1] }, state(one.snapshots)),
      h('td', { 'data-label': COLUMNS[2] }, h('div', { class: 'snap-stack' }, copies)),
      h('td', { 'data-label': COLUMNS[3] }, h('div', { class: 'snap-stack' }, check)));
  }

  return {
    element,
    /** Says that the rows could not be worked out, `words`, with a button that tries again. */
    fail(words, retry) {
      last = null;
      element.removeAttribute('aria-busy');
      fill(element.querySelector('.snap-body'), failed(words, retry));
    },
    set(list, backups, context) {
      const rows = protectedRows(list, backups, context);
      const skipped = skippedPlaces(list);
      const key = JSON.stringify([rows, skipped, view.session.admin]);
      if (key === last) return;
      last = key;
      element.removeAttribute('aria-busy');
      fill(element.querySelector('.snap-body'),
        h('div', { class: 'tbl-box' },
          h('table', { class: 'tbl stack' },
            h('caption', { class: 'sr-only' }, 'What is protected: each place, its snapshots, its copies off this server and its last restore check'),
            h('thead', {}, h('tr', {}, COLUMNS.map((words) => h('th', { scope: 'col' }, words)))),
            h('tbody', {}, rows.map(row)))),
        skipped.length > 0 && h('div', { class: 'snap-note' },
          ...skipped.map((path) => h('p', {}, h('b', {}, `snap skips ${path}`), ': it is not in the snap service\'s view. Add it with this command.')),
          copyBox('elevate service edit snap', 'The command that adds a place to the snap service')),
        h('p', { class: 'small muted' }, 'Older copies of a file are in its snapshots: Browse below. ',
          journal && ['Each change to the settings is also kept one by one in ', h('a', { href: '/reliability' }, 'the journal'), '.']));
    },
  };
}
