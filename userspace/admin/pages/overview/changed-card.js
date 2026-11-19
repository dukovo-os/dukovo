// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What changed (design 10.2): the last five changes, as the board's timeline,
// each with the buttons that fit it (change-actions.js), and "See all", which
// opens the whole journal on Reliability.

import { fill, h } from '../../dom.js';
import { linkable, makeCard, reader } from './card.js';
import { makeChangeActions } from './change-actions.js';
import { changedHeading, changeRow } from './change-model.js';

/** How many changes the card lists. */
const SHOWN = 5;

/** The card. `about()` gives `console.about`'s answer or null, and `reload()` reads the cards again that a change in the journal alters. */
export function makeChanged(view, { about, reload }) {
  const card = makeCard('changed', 'What changed');
  card.heading.tabIndex = -1;
  card.more.classList.add('muted');

  const actions = makeChangeActions(view, {
    prefix: 'ov-change',
    async done() {
      await reload();
      card.heading.focus();
    },
  });

  function itemOf(change, detail) {
    const row = changeRow(change, view.time);
    return h('div', { class: 'tl-item', 'data-change': change.id },
      h('time', { datetime: new Date(change.at * 1000).toISOString(), title: view.time.full(change.at) }, row.time),
      h('span', { class: 'tl-rail' }, h('i', { class: row.dot }, h('span', { class: 'sr-only' }, { warn: 'Needs a look', bad: 'Did not work', ok: 'Kept', info: '' }[row.dot]))),
      h('div', { class: 'tl-body' },
        h('b', {}, row.title),
        h('span', { class: 'muted small' }, row.sub),
        h('span', { class: 'btns' }, ...actions.buttons(change, row, detail))));
  }

  const load = reader(view, card.body, {
    async read() {
      const now = Math.floor(Date.now() / 1000);
      const heading = changedHeading(await about().catch(() => null), view.time, now);
      const { changes = [] } = await view.api.get(`/api/1/changes?since=${heading.since}&limit=${SHOWN}`);
      // Whether a change can be put back is in the change itself, not in the list.
      const details = view.session.admin
        ? await Promise.all(changes.map((change) => view.api.get(`/api/1/changes/${change.id}`).catch(() => null)))
        : changes.map(() => null);
      return { heading, changes, details };
    },
    draw({ heading, changes, details }) {
      card.more.textContent = heading.more;
      fill(card.body,
        changes.length === 0
          ? h('p', { class: 'muted' }, `Nothing has changed ${heading.more}.`)
          : h('div', { class: 'tl' }, ...changes.map((change, at) => itemOf(change, details[at]))),
        linkable('/reliability/changes') && h('a', { href: '/reliability/changes', 'aria-label': 'See all changes' }, 'See all'));
    },
  });
  return { element: card.element, load };
}
