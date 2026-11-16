// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What stands beside the sheet of an app: the search, the ready-made setups
// and the table of the apps the catalogs list. A row is a link to the app's
// sheet. Nothing here is drawn again while its words are the same, so that
// the field a person types in keeps its place, and a link in hand stays in
// hand.

import { fill, h, icon } from '../../dom.js';
import { counted } from '../../lib/size-words.js';
import { square } from '../apps/mark.js';
import { copyBox } from '../snapshots/copy-box.js';

/** The label of the search field, which a screen reader hears and nothing shows. */
const LABEL = 'Search apps and setups';

/** What the table says of the apps it lists, for a screen reader. */
export const captionOf = (count) => `${counted(count, 'app')} in the catalogs`;

/**
 * The list. `type(text)` is called as the person types, and `clear()` when
 * the search is cleared from the table. Gives `{element, input, setups,
 * catalogs, problem, say}`: `setups(list)` draws the cards of catalog-model's
 * `setupsOf`; `catalogs({rows, unread, empty, selected, busy, updates})`
 * draws the table of `rowsOf`, the banners of `unreadOf` (`updates` says
 * whether the Updates page is there to link to) and what `emptyOf` says when
 * there is no row; `problem(words)` says why the search was not sent; `say(words)`
 * is the line a screen reader hears when the table changed.
 */
export function makeList({ type, clear }) {
  const input = h('input', {
    id: 'add-q', class: 'input', type: 'search', placeholder: 'A name, or what you want to do, such as a website', autocomplete: 'off', 'aria-describedby': 'add-q-note',
  });
  input.addEventListener('input', () => type(input.value));
  const note = h('div', { id: 'add-q-note', class: 'hint', role: 'status' });
  const spoken = h('p', { class: 'sr-only', role: 'status' });
  const setupsBox = h('div', {});
  const catalogsBox = h('div', {});
  const element = h('div', { class: 'add-list' },
    h('div', { class: 'field' }, h('label', { for: 'add-q', class: 'sr-only' }, LABEL), input, note),
    spoken, setupsBox, catalogsBox);
  let drawnSetups = null;
  let drawnCatalogs = null;

  /** One card: a link to the page that does the setup, with what it uses and what it installs first. */
  const card = (setup) => h('li', {},
    h('a', { class: 'radio-card', href: setup.href },
      h('span', {},
        h('b', {}, setup.title),
        h('span', { class: 'muted' }, setup.words),
        h('span', { class: 'uses' }, setup.uses.map((name) => h('span', { class: 'chip' }, name))),
        setup.says && h('span', { class: 'small warn' }, setup.says))));

  /** Draws the cards of the setups, or nothing when none matches. */
  function setups(list) {
    const key = JSON.stringify(list);
    if (key === drawnSetups) return;
    drawnSetups = key;
    fill(setupsBox, list.length > 0 && h('section', { class: 'card', 'aria-labelledby': 'add-setups-h' },
      h('h2', { id: 'add-setups-h' }, 'Ready-made setups'),
      h('p', { class: 'small muted' }, 'Each setup is a page of this console; it shows every command it runs before it runs it.'),
      h('ul', { class: 'setups' }, list.map(card))));
  }

  /** The banner of a catalog that could not be read, with the way to the Updates page when the network is why. */
  const bannerOf = (one, updates) => h('div', { class: 'banner warn', role: 'note' }, icon('warn'),
    h('div', {}, h('p', {}, one.words), one.network && updates && h('p', { class: 'small' }, h('a', { href: '/updates' }, 'Look at the connection on the Updates page'))));

  /** One row of the table. */
  const rowOf = (row, selected) => h('tr', { class: row.id === selected ? 'sel' : null },
    h('td', { 'data-label': 'App' },
      h('a', { class: 'add-who', href: row.href, 'aria-current': row.id === selected ? 'page' : null },
        square(row), h('span', {}, h('b', {}, row.name), h('small', { class: 'mono' }, row.id)))),
    h('td', { 'data-label': 'Version' }, row.version),
    h('td', { 'data-label': 'Catalog' }, row.catalog),
    h('td', { 'data-label': 'Flaws' }, h('span', { class: row.flaws.urgent ? 'small warn' : 'small' }, row.flaws.words)),
    h('td', { 'data-label': 'Installed' }, row.installed
      ? [h('span', { class: 'pill ok' }, 'Installed'), h('small', {}, `Version ${row.installed}`)]
      : h('span', { class: 'small muted' }, 'Not installed')));

  /** What stands in place of the table when it has no row. */
  function emptyNode(empty) {
    const clearButton = empty.kind === 'nomatch' ? h('button', { class: 'btn', type: 'button' }, 'Clear the search') : null;
    clearButton?.addEventListener('click', clear);
    return h('div', { class: 'add-empty', role: 'status' },
      h('p', {}, h('b', {}, empty.words)),
      empty.command && copyBox(empty.command, 'Command that lists the catalogs'),
      clearButton);
  }

  /** Draws the table, the banners and the empty states, unless they say what is drawn. */
  function catalogs(data) {
    const key = JSON.stringify(data);
    if (key === drawnCatalogs) return;
    drawnCatalogs = key;
    const {
      rows, unread, empty, selected, busy, updates,
    } = data;
    fill(catalogsBox, h('section', { class: 'card', 'aria-labelledby': 'add-cat-h', 'aria-busy': busy ? 'true' : null },
      h('h2', { id: 'add-cat-h' }, 'Apps in the catalogs'),
      unread.map((one) => bannerOf(one, updates)),
      empty
        ? emptyNode(empty)
        : h('div', { class: 'tbl-box' },
          h('table', { class: 'tbl stack' },
            h('caption', { class: 'sr-only' }, captionOf(rows.length)),
            h('thead', {}, h('tr', {}, ['App', 'Version', 'Catalog', 'Flaws', 'Installed'].map((name) => h('th', { scope: 'col' }, name)))),
            h('tbody', {}, rows.map((row) => rowOf(row, selected)))))));
  }

  /** Says why the search was not sent, under the field; empty words clear it. */
  function problem(words) {
    fill(note, words);
    input.classList.toggle('bad', Boolean(words));
    if (words) input.setAttribute('aria-invalid', 'true');
    else input.removeAttribute('aria-invalid');
  }

  /** The line a screen reader hears when the table changed. */
  const say = (words) => { spoken.textContent = words; };

  return {
    element, input, setups, catalogs, problem, say,
  };
}
