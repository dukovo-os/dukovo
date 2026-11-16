// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The table of installed apps: each app's initial in a square, its name and
// ID, its version, and what is to be done about it (an update, a hold, the
// flaws of the version installed). A row is a link to the app's page. The
// apps a person installed for themselves are apart, "Yours", since no
// button here runs them: each has the commands that do. What a running
// task says is put in the rows in place, so that a link in hand stays in
// hand.

import { fill, h } from '../../dom.js';
import { copyBox } from '../snapshots/copy-box.js';
import { captionWords, ownCommands, updatable } from './list-model.js';
import { square } from './mark.js';
import { jobWords } from './run-model.js';

/** How an update's state is drawn in a row: the pill's class, or none for the plain words. */
const PILLS = Object.freeze({ update: 'pill info', waiting: 'pill', held: 'pill warn' });

/** Calls `run(button)` when `button` is pressed, unless a task that runs has put it off. */
export function onPress(button, run) {
  button.addEventListener('click', () => {
    if (button.getAttribute('aria-disabled') !== 'true') run(button);
  });
}

/** One cell of the Update column: the words of a task that runs, and the words of the update, one shown at a time. */
function updateCell(row) {
  const pill = PILLS[row.update.state];
  return h('td', { 'data-label': 'Update', 'data-app': row.id, 'data-updates': String(row.update.state === 'update') },
    h('span', { class: 'pill info apps-now', hidden: true }),
    h('span', { class: 'apps-next' },
      pill ? h('span', { class: pill }, row.update.words) : h('span', { class: 'small muted' }, row.update.words),
      row.flaws && h('small', { class: row.flaws.urgent ? 'warn' : null }, row.flaws.words)));
}

/**
 * The list. `updateAll(button)` is called when Update all is pressed.
 * Gives `{element, set, paint}`: `set({rows, selected, line,
 * admin})` draws the rows `{everyone, yours}` of rowsOf, with the row of the
 * app `selected` marked, and `line` (when the apps were last checked) over
 * the table; `paint(jobs)` puts what the tasks of `jobs` (model's `makeJobs`)
 * say in the rows and keeps the buttons off while one runs.
 */
export function makeList(view, { updateAll }) {
  const element = h('section', { class: 'apps-list', 'aria-label': 'Installed apps' });
  let drawn = null;
  let jobs = null;

  /** One row of the apps installed for everyone. */
  const rowOf = (row, selected) => h('tr', { class: row.id === selected ? 'sel' : null },
    h('td', { 'data-label': 'App' },
      h('a', { class: 'apps-who', href: row.href, 'aria-current': row.id === selected ? 'page' : null },
        square(row), h('span', {}, h('b', {}, row.name), h('small', { class: 'mono' }, row.id)))),
    h('td', { 'data-label': 'Version' }, row.version),
    updateCell(row));

  /** The table of the apps a person installed for themselves, each with the commands that update or remove it. */
  const yoursOf = (rows) => h('section', { class: 'apps-yours', 'aria-labelledby': 'apps-yours-h' },
    h('h2', { id: 'apps-yours-h' }, 'Yours'),
    h('p', { class: 'small muted' }, 'Apps you installed for yourself. They are yours at a terminal: no button here runs them.'),
    h('div', { class: 'tbl-box' },
      h('table', { class: 'tbl stack' },
        h('caption', { class: 'sr-only' }, captionWords(rows.length, 'for you')),
        h('thead', {}, h('tr', {}, ['App', 'Version', 'Commands'].map((name) => h('th', { scope: 'col' }, name)))),
        h('tbody', {}, rows.map((row) => h('tr', {},
          h('td', { 'data-label': 'App' }, h('span', { class: 'apps-who' }, square(row), h('span', {}, h('b', {}, row.name), h('small', { class: 'mono' }, row.id)))),
          h('td', { 'data-label': 'Version' }, row.version),
          h('td', { 'data-label': 'Commands' },
            copyBox(ownCommands(row.id).update, `Command to update ${row.name}`),
            copyBox(ownCommands(row.id).remove, `Command to remove ${row.name}`))))))));

  /** Puts the words of the tasks in the rows and the guard on the buttons. */
  function paint(next = jobs) {
    jobs = next;
    for (const cell of element.querySelectorAll('td[data-app]')) {
      const job = jobs?.of(cell.getAttribute('data-app'), cell.getAttribute('data-updates') === 'true') ?? null;
      const now = cell.querySelector('.apps-now');
      const words = job ? jobWords(job) : '';
      if (now.textContent !== words) now.textContent = words;
      now.hidden = job === null;
      cell.querySelector('.apps-next').hidden = job !== null;
    }
    for (const button of element.querySelectorAll('[data-guard]')) {
      if (jobs?.any()) button.setAttribute('aria-disabled', 'true');
      else button.removeAttribute('aria-disabled');
    }
  }

  /** Draws the list, unless it says what is drawn. */
  function set({
    rows, selected, line, admin,
  }) {
    const key = JSON.stringify([rows, selected, line, admin]);
    if (key === drawn) return;
    drawn = key;
    const waiting = updatable(rows.everyone).length;
    const button = admin && waiting > 1
      ? h('button', { class: 'btn', type: 'button', 'data-action': 'update-all', 'data-guard': '' }, `Update all (${waiting})`)
      : null;
    if (button) onPress(button, updateAll);
    fill(element,
      h('div', { class: 'apps-tools' },
        h('p', { class: 'small muted' }, line),
        button && h('div', { class: 'apps-action' }, button, view.api.inline('apps.update-all', {}))),
      rows.everyone.length === 0
        ? h('p', { class: 'apps-empty' }, h('b', {}, 'No apps yet.'))
        : h('div', { class: 'tbl-box' },
          h('table', { class: 'tbl stack' },
            h('caption', { class: 'sr-only' }, captionWords(rows.everyone.length, 'for everyone')),
            h('thead', {}, h('tr', {}, ['App', 'Version', 'Update'].map((name) => h('th', { scope: 'col' }, name)))),
            h('tbody', {}, rows.everyone.map((row) => rowOf(row, selected))))),
      rows.yours.length > 0 && yoursOf(rows.yours));
    paint();
  }

  return { element, set, paint };
}
