// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The backup places (design 5.7): a table of where backups go and how each is
// doing, and the place that the address opens, which detail.js draws. That
// file is read when a place is first opened, since most visits never open
// one and the dialogs it needs are not small.

import { fill, h } from '../../dom.js';
import { bars, failed } from './body.js';
import { findPlace, placeRows } from './model.js';

/**
 * Makes the card. `jobs` is jobs.js's; `links` says which pages are in this
 * console, `{move}`; `add()` opens Add a backup place, `restore(place,
 * show, preset)` opens Restore files…, and `changed()` reads the page again.
 * Gives `{element, set, ended, unlocked}`: `set({backups, time, now, open})`
 * draws what `backup list` says; `ended(name)` reads again what a place
 * holds after a task of it; `unlocked()` after the console was unlocked or locked.
 */
export function makePlaces(view, { jobs, links, add, restore, changed }) {
  const element = h('section', { class: 'card snap-places', id: 'backups', 'aria-labelledby': 'snap-places-h', 'aria-busy': 'true' },
    h('h2', { id: 'snap-places-h', tabindex: '-1' }, 'Backup places'),
    h('div', { class: 'snap-body' }, bars()));
  const table = h('div', { class: 'snap-table' });
  const detail = h('div', { class: 'snap-detail' });
  fill(element.querySelector('.snap-body'), table, detail);

  let model = null;
  let lastTable = null;
  // The name of the place that the box says is not there, so that the words are not drawn again.
  let lastGone = null;
  let ui = null;
  let loading = null;

  /** Draws the opened place, reading detail.js first when it is the first. */
  function showDetail() {
    if (ui !== null) {
      const place = model.open === null ? null : findPlace(model.backups, model.open);
      if (place !== null) ui.set(model, place);
      return;
    }
    if (loading !== null) return;
    fill(detail, bars());
    loading = import('./detail.js').then(({ makeDetail }) => {
      ui = makeDetail(view, { container: detail, jobs, links, restore, changed });
      if (!view.signal.aborted && model?.open) showDetail();
    }, (error) => {
      loading = null;
      view.problem(error);
    });
  }

  return {
    element,
    /** Says that the backup places could not be read, `words`, with a button that tries again. */
    fail(words, retry) {
      lastTable = null;
      lastGone = null;
      ui?.clear();
      element.removeAttribute('aria-busy');
      fill(table, failed(words, retry));
      detail.replaceChildren();
    },
    set(next) {
      model = next;
      element.removeAttribute('aria-busy');
      const rows = placeRows(next.backups, next);
      const admin = view.session.admin;
      const tableKey = JSON.stringify([rows, next.open, admin]);
      if (tableKey !== lastTable) {
        lastTable = tableKey;
        if (rows.length === 0) {
          const addButton = admin && h('button', { class: 'btn default', type: 'button' }, 'Add a backup place…');
          addButton?.addEventListener('click', () => add());
          fill(table, h('p', { class: 'muted' }, 'Nothing is copied off this server yet. A copy elsewhere is what brings files back after a lost disk, a fire or a mistake that reached the snapshots.'),
            addButton && h('div', { class: 'btns' }, addButton));
        } else {
          fill(table, h('div', { class: 'tbl-box' },
            h('table', { class: 'tbl stack' },
              h('caption', { class: 'sr-only' }, 'Backup places: where each goes, how often, the last good backup and how it is now'),
              h('thead', {}, h('tr', {}, ['Place', 'Every', 'Last good', 'Now'].map((words) => h('th', { scope: 'col' }, words)))),
              h('tbody', {}, rows.map((row) => h('tr', { class: row.name === next.open ? 'sel' : null },
                h('td', { 'data-label': 'Place' }, h('a', { href: row.href, 'aria-current': row.name === next.open ? 'true' : null }, h('b', {}, row.name)), h('small', {}, row.where)),
                h('td', { 'data-label': 'Every' }, row.every),
                h('td', { 'data-label': 'Last good' }, row.last),
                h('td', { 'data-label': 'Now' }, h('span', { class: `st ${row.now.tone || 'off'}`, 'data-now': row.name, 'data-words': row.now.words, 'data-tone': row.now.tone || 'off' }, row.now.words))))))));
        }
      }
      const place = next.open === null ? null : findPlace(next.backups, next.open);
      if (place === null) {
        ui?.clear();
        const gone = next.open;
        if (gone === null) {
          lastGone = null;
          detail.replaceChildren();
        } else if (gone !== lastGone) {
          lastGone = gone;
          fill(detail, h('p', { class: 'muted', role: 'status' }, `There is no backup place called ${gone}. `, h('a', { href: '/snapshots/backups' }, 'Back to the backup places')));
        }
        return;
      }
      lastGone = null;
      showDetail();
    },
    ended(name) {
      ui?.ended(name);
    },
    unlocked() {
      ui?.unlocked();
      if (model) this.set(model);
    },
  };
}
