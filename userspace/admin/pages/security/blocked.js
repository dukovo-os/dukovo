// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Blocked after failed sign-ins (design 8.5 and 22.7): the addresses that
// wrong passwords have blocked, with Unblock; the networks that are never
// blocked, with the dialog that lists them; and the rule in words. What only
// an administrator may read is shown when the console is unlocked, and read
// again every 30 seconds while the card is shown and after each change.

import { h, icon } from '../../dom.js';
import { adminReader, makeCard, unlocked } from './card.js';
import { blockedRows } from './model.js';
import { BLOCKING_OFF, blockingWords, neverWords } from './rules-model.js';

/** How often the blocks are read again while the card is shown. */
const READ_MS = 30_000;

/**
 * The card. Gives `{element, load}`; `load()` reads the blocks when the
 * console is unlocked.
 */
export function makeBlocked(view) {
  const card = makeCard('blocked', 'Blocked after failed sign-ins');
  let reader = null;

  /** The table, or the line that says that nothing is blocked. */
  function tableOf(answer) {
    const rows = blockedRows(answer, view.time);
    if (rows.length === 0) return h('p', { class: 'muted' }, 'No address is blocked now.');
    return h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack' },
      h('caption', { class: 'sr-only' }, 'Addresses blocked after wrong sign-ins'),
      h('thead', {}, h('tr', {}, ...['Address', 'Tried', 'Until'].map((name) => h('th', { scope: 'col' }, name)), h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Action')))),
      h('tbody', {}, ...rows.map((row) => h('tr', {},
        h('td', { 'data-label': 'Address' }, h('span', { class: 'mono' }, row.address), row.outside && h('small', {}, 'outside')),
        h('td', { 'data-label': 'Tried' }, row.why),
        h('td', { 'data-label': 'Until', title: row.full }, row.until),
        h('td', { class: 'end' }, h('button', { class: 'btn small', type: 'button', onclick: async (event) => {
          const result = await view.api.run('security.unblock', { address: row.key }, {
            button: event.currentTarget, where: { slot: card.errors }, words: { unlock: `unblock ${row.address}`, running: 'Unblocking', done: `${row.address} is unblocked.` },
          });
          if (result.ok) reader.read();
        } }, 'Unblock'), view.api.inline('security.unblock', { address: row.key })))))));
  }

  /** The dialog that lists the networks that are never blocked, with Remove for each and a field to add one. */
  async function openNever() {
    if (!(await unlocked(view, 'change the never-block list'))) return;
    const { openNeverForm } = await import('./never-form.js');
    await openNeverForm(view, reader);
  }

  reader = adminReader(view, card, {
    action: 'security.blocked',
    every: READ_MS,
    words: {
      unlock: 'see the blocked addresses',
      reader: 'Administrators see blocked addresses.',
      locked: 'Unlock to see blocked addresses.',
      failed: 'The blocked addresses could not be read.',
      missing: 'This server shows blocked addresses with a later update of Dukovo.',
    },
    draw: (answer) => [
      answer?.on === false && h('div', { class: 'banner warn', role: 'status' }, icon('warn'), h('div', {}, h('p', {}, h('b', {}, 'Blocking is off. '), BLOCKING_OFF))),
      tableOf(answer),
      h('p', { class: 'small muted' }, `${neverWords(answer?.never)} · `, h('button', { class: 'btn link small', type: 'button', onclick: openNever }, 'change')),
      blockingWords(answer) && h('p', { class: 'small muted' }, blockingWords(answer)),
    ],
  });
  return { element: card.element, load: reader.load };
}
