// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add an app: /apps/add is the search, the ready-made setups and the apps the
// catalogs list, and /apps/add/ID is the sheet of one of them, beside the list
// on a wide screen and alone on a phone. What is typed is searched 300 ms
// after the typing stops, and is kept in the address, so that the sheet that
// a row opens, and the way back from it, keep the search. The apps installed
// are read when the page opens and when a change says that something moved.
// The sheet is read when it is asked for and the install is a task: both are
// in modules that are loaded only for a sheet.

import { h } from '../../dom.js';
import {
  addHref, emptyOf, rowsOf, searchPath, setupsOf, sheetHref, unreadOf, wordsOf,
} from '../../lib/catalog-model.js';
import { PAGES } from '../../pages.js';
import { idOf } from '../apps/apps-model.js';
import { captionOf, makeList } from './list.js';

/** How long the typing has to stop before the search is sent. */
const TYPING_MS = 300;

/** What the table says before the catalogs have answered, and when they could not be asked. */
const LOADING = Object.freeze({ kind: 'loading', words: 'Reading the catalogs\u2026' });
const FAILED = Object.freeze({ kind: 'failed', words: 'The catalogs could not be listed.' });

/** Whether the console has a page at `path`. */
const hasPath = (path) => PAGES.some((entry) => entry.path === path && entry.load !== null);

export async function show(view) {
  const wanted = idOf(view.rest);
  const admin = view.session.admin;
  const updates = hasPath('/updates');
  let typed = view.query?.get('q') ?? '';
  let found = wordsOf(typed);
  let answer = null;
  let apps = null;
  let searching = false;
  let latest = 0;
  let timer = null;
  let failed = false;
  let headed = false;

  /** Draws the head: the page's name under its crumb, and the two views of the apps with how many are installed. */
  function drawHead() {
    const count = Array.isArray(apps?.bundles) ? ` (${apps.bundles.length})` : '';
    headed = true;
    view.head({
      title: 'Add an app',
      crumbs: [{ title: 'Apps', path: '/apps' }],
      commands: true,
      buttons: [h('span', { class: 'seg', role: 'group', 'aria-label': 'View' },
        h('a', { href: '/apps' }, `Installed${count}`),
        h('a', { class: 'on', href: addHref(), 'aria-current': 'page' }, 'Add an app'))],
    });
  }

  /** The address of this page with what is typed in it, kept without a step of the history for each key. */
  const keepTyped = () => {
    try {
      history.replaceState(history.state, '', wanted ? sheetHref(wanted, typed) : addHref(typed));
    } catch {
      // Without the address, a reload starts the search again from nothing.
    }
  };

  const list = makeList({
    type: onType,
    clear() {
      list.input.value = '';
      onType('');
      list.input.focus();
    },
  });
  list.input.value = typed;
  list.problem(found.problem);

  // The sheet is read only for one that is asked for; an address that cannot be an app's ID has none.
  const panel = wanted ? (await import('./sheet.js')).makePanel(view, {
    id: wanted, typed, admin, apps: () => apps, installed: () => { read(); search(); },
  }) : null;
  const missing = wanted === null
    ? h('section', { class: 'card add-panel', role: 'status' },
      h('p', {}, `No app can have the ID ${view.rest}.`), h('a', { class: 'btn', href: addHref(typed) }, 'Back to the catalogs'))
    : null;
  view.main.append(h('div', { class: `add${wanted === '' ? '' : ' has-sheet'}` }, list.element, panel?.element, missing));

  // ---- reading ----

  /** Draws the setups and the table from what was read. */
  function draw() {
    const ids = Array.isArray(apps?.bundles) ? apps.bundles.map((one) => one?.id) : null;
    list.setups(setupsOf(typed, ids));
    const rows = answer === null ? [] : rowsOf(answer, apps?.bundles ?? [], typed);
    list.catalogs({
      rows,
      unread: unreadOf(answer),
      empty: answer === null ? (failed ? FAILED : LOADING) : emptyOf(answer, found.words),
      selected: wanted || null,
      busy: searching,
      updates,
    });
  }

  /** Reads the installed apps; the rows say which of them are installed, and the head how many. */
  async function read() {
    try {
      apps = await view.api.get('/api/1/apps');
    } catch (error) {
      if (error?.name === 'AbortError') return;
      // What is installed is not needed to look at the catalogs: the page says nothing of it.
    }
    drawHead();
    draw();
  }

  const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
  again.addEventListener('click', () => {
    view.slot.replaceChildren();
    search();
  });

  /** Asks the catalogs for the apps that match the words typed. Only the last answer counts. */
  async function search() {
    latest += 1;
    const mine = latest;
    searching = true;
    draw();
    try {
      const result = await view.api.get(searchPath(found.words));
      if (mine !== latest) return;
      answer = result;
      failed = false;
      view.slot.replaceChildren();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      failed = true;
      view.problem(error, { buttons: [again] });
    }
    searching = false;
    draw();
    const empty = answer === null ? FAILED : emptyOf(answer, found.words);
    list.say(empty ? empty.words : captionOf(rowsOf(answer, apps?.bundles ?? []).length));
  }

  /** Called as the person types: the setups follow at once, and the search is sent when the typing stops. */
  function onType(text) {
    typed = text;
    found = wordsOf(text);
    list.problem(found.problem);
    keepTyped();
    clearTimeout(timer);
    if (found.problem === null) timer = setTimeout(search, TYPING_MS);
    draw();
  }

  view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
  view.on('change', read);
  drawHead();
  panel?.load();
  await Promise.all([read(), search()]);
  if (!headed) drawHead();
}
