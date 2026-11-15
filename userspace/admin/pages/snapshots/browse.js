// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Browsing a snapshot and restoring files from it (design 5.6): a folder of a
// snapshot as it was, each entry with how it compares with the same path now,
// and Restore N chosen, which puts a copy of each chosen path beside the
// current one with the snapshot's time in its name, so that nothing is
// overwritten. The browse runs as the person, who sees in a snapshot what
// they could see then; an administrator whose console is unlocked sees every
// file of a folder outside their home, and restores as the administrator.
// This is its own graph: the page reads it when an address under
// `/snapshots/browse` is opened.

import {
  errorSlot, fill, h, uniqueId,
} from '../../dom.js';
import { PAGES } from '../../pages.js';
import { besideName, RESTORE_MOST, rowsOf, trail, within } from '../../lib/snapshot-folder.js';
import { beside, browseHref, whenWords } from '../../lib/snapshot-words.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { bars } from './body.js';

/** Whether the page with the id `id` is in this console. */
const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);

/** The words of a count of entries: "2,000". */
const number = (count) => count.toLocaleString('en-US');

/** What the person is told when the address names no snapshot. */
function missing(view, words) {
  view.head({ title: 'Browse a snapshot', crumbs: [{ title: 'Snapshots and backups', path: '/snapshots' }] });
  view.main.append(h('section', { class: 'card' }, h('h2', {}, words),
    h('p', {}, 'Open a snapshot from the list of snapshots, with Browse.'),
    h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/snapshots' }, 'Back to the snapshots'))));
}

/** Shows the folder that the address names. */
export async function show(view) {
  const place = view.query.get('place') ?? '';
  const snapshot = view.query.get('snapshot') ?? '';
  const folder = view.query.get('folder') || place;
  if (!place.startsWith('/') || snapshot === '' || !within(place, folder)) {
    missing(view, 'This address does not name a snapshot');
    return;
  }

  // The snapshot's time is the label of restored files, and says which snapshot this is.
  let list;
  try {
    list = await view.api.get(`/api/1/snapshots?${new URLSearchParams({ place })}`);
  } catch (error) {
    if (error?.name !== 'AbortError') view.problem(error);
    return;
  }
  if (view.signal.aborted) return;
  const found = (Array.isArray(list?.snapshots) ? list.snapshots : []).find((one) => one.name === snapshot);
  const made = Date.parse(found?.made);
  if (!Number.isFinite(made)) {
    missing(view, `There is no snapshot called ${snapshot} of ${place}`);
    return;
  }
  const { time } = view;
  const now = Date.now();
  const when = whenWords(made, time);
  const label = beside(made, time.zone, now);
  view.head({
    title: folder,
    crumbs: [{ title: 'Snapshots and backups', path: '/snapshots' }, { title: when, path: browseHref(place, snapshot, place) }],
    commands: true,
  });

  // ---- the card ----
  const id = uniqueId('snap-browse');
  const errors = errorSlot(`${id}-errors`);
  const note = h('div', { class: 'snap-note' });
  const table = h('div', { class: 'snap-table', 'aria-busy': 'true' }, bars());
  const result = h('div', { class: 'snap-result', role: 'status', tabindex: '-1' });
  const count = h('span', { class: 'small', role: 'status' });
  const restore = h('button', { class: 'btn default', type: 'button', disabled: true }, 'Restore 0 chosen');
  const crumbs = trail(place, folder).map((one, at, all) => (at === all.length - 1
    ? h('span', { 'aria-current': 'location' }, one.name)
    : [h('a', { href: browseHref(place, snapshot, one.folder) }, one.name), ' › ']));
  view.main.append(h('section', { class: 'card snap-browse', 'aria-labelledby': `${id}-h` },
    h('h2', { id: `${id}-h` }, `Browsing: ${when}`),
    h('nav', { class: 'snap-trail', 'aria-label': 'Folders of the snapshot' }, crumbs),
    errors, note, table,
    h('p', { class: 'small muted' }, `Restored files come back beside the current ones with "(${label})" in the name, so nothing is overwritten.`),
    result,
    h('div', { class: 'snap-actions' }, restore, count)));

  // ---- reading the folder ----
  const picked = new Set();
  let rows = [];
  let latest = 0;
  const own = () => `/users/${view.session.name}`;
  const unlocked = () => view.session.admin && isUnlocked();
  /** Whether an administrator reads and restores as the administrator: the console is unlocked and the folder is not in their home. */
  const asAdmin = () => unlocked() && !within(own(), folder);

  function drawCount() {
    const total = picked.size;
    restore.textContent = `Restore ${total} chosen`;
    restore.disabled = total === 0 || total > RESTORE_MOST;
    count.textContent = total > RESTORE_MOST ? `Choose at most ${RESTORE_MOST} at a time.` : '';
  }

  function draw(browse) {
    rows = rowsOf(browse);
    for (const path of [...picked]) if (!rows.some((row) => row.path === path)) picked.delete(path);
    table.removeAttribute('aria-busy');
    if (rows.length === 0) {
      fill(table, h('p', { class: 'muted' }, 'This folder is empty in the snapshot.'));
    } else {
      const boxes = new Map();
      fill(table, h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
        h('caption', { class: 'sr-only' }, `The entries of ${folder} in the snapshot of ${when}`),
        h('thead', {}, h('tr', {},
          h('th', { scope: 'col' }, h('span', { class: 'muted' }, 'Pick')), h('th', { scope: 'col' }, 'Name'),
          h('th', { scope: 'col', class: 'num' }, 'Size'), h('th', { scope: 'col' }, 'Then'))),
        h('tbody', {}, rows.map((row) => {
          const box = h('input', { type: 'checkbox', 'aria-label': `Pick ${row.name}`, checked: picked.has(row.path), disabled: !row.pickable });
          // The label around the box is the larger target to press.
          box.addEventListener('change', () => {
            if (box.checked) picked.add(row.path);
            else picked.delete(row.path);
            drawCount();
          });
          boxes.set(row.path, box);
          return h('tr', {},
            h('td', { 'data-label': 'Pick' }, h('label', { class: 'snap-pick' }, box)),
            h('td', { 'data-label': 'Name' }, row.folder ? h('a', { class: 'snap-folder', href: browseHref(place, snapshot, row.path) }, h('b', {}, row.name), h('span', { class: 'sr-only' }, ', folder')) : row.name, row.kind === 'link' && h('small', {}, 'A link')),
            h('td', { 'data-label': 'Size', class: 'num' }, row.size),
            h('td', { 'data-label': 'Then' }, row.then && h('span', { class: `pill ${row.then.tone}`.trim() }, row.then.word)));
        })))));
    }
    drawCount();
    fill(note,
      browse?.more === true && h('p', { class: 'small muted', role: 'status' }, `Showing the first ${number(rows.length)} entries. Open a folder below to see fewer.`),
      view.session.admin && !unlocked() && !within(own(), folder) && h('p', { class: 'small muted' }, 'Some files here may be hidden while the console is locked. ', (() => {
        const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock to see every file');
        unlock.addEventListener('click', async () => {
          if (await askToUnlock(view.session, 'see every file in a snapshot')) read();
        });
        return unlock;
      })()));
  }

  /** Reads the folder. Only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    errors.replaceChildren();
    const query = { place, snapshot, folder };
    let browse;
    try {
      if (asAdmin()) {
        const done = await view.api.run('snapshots.browse-all', query, { words: { unlock: 'see every file in a snapshot' }, where: { slot: errors } });
        browse = done.ok ? done.result : null;
      } else {
        browse = await view.api.get(`/api/1/snapshots/browse?${new URLSearchParams(query)}`);
      }
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      view.problem(error, { slot: errors });
      browse = null;
    }
    if (mine !== latest || view.signal.aborted) return;
    if (browse === null) {
      table.removeAttribute('aria-busy');
      if (rows.length === 0) fill(table, h('p', { class: 'muted' }, 'This folder could not be read.'));
      return;
    }
    draw(browse);
  }

  restore.addEventListener('click', async () => {
    const paths = rows.filter((row) => picked.has(row.path));
    errors.replaceChildren();
    result.replaceChildren();
    const done = await view.api.run(asAdmin() ? 'snapshots.restore-all' : 'snapshots.restore', { snapshot, label, paths: paths.map((row) => row.path).join('\n') }, {
      words: { unlock: 'restore files from a snapshot', done: 'The files were restored beside the current ones.', busy: 'Restoring…' }, button: restore, where: { slot: errors },
    }).catch(() => ({ ok: false }));
    if (done.ok) {
      picked.clear();
      fill(result,
        h('p', {}, h('b', {}, paths.length === 1 ? 'Restored 1 path beside the current one:' : `Restored ${paths.length} paths beside the current ones:`)),
        h('ul', { class: 'snap-restored' }, paths.map((row) => h('li', { class: 'mono' }, besideName(row.path, label, { folder: row.folder })))),
        hasPage('terminal') && h('p', {}, h('a', { href: '/terminal' }, 'Open the Terminal'), ' to look at them.'));
      result.focus();
    }
    // A restore that stopped part of the way did the paths before the one that failed; what it did not change is read again.
    read();
    drawCount();
  });

  view.on('unlock', read);
  await read();
}
