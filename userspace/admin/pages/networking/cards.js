// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The cards of the Networking page (design 9.3): one section for each network
// card the machine has, with its link, addresses, router, name servers and
// lease, and the way into its settings. A card is drawn again only when what
// it says changes, so that a link in hand stays in hand.

import { fill, h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { cardRow } from './network-model.js';

/** A list of lines, one to a line. */
const lines = (items) => h('ul', { class: 'net-items' }, items.map((item) => h('li', {}, item)));

/**
 * The facts of the card `row` (see network-model.js's `cardRow`) as labelled
 * rows: what the card is, has and gets. A fact the card has none of is left
 * out.
 */
export function factsOf(row) {
  const rows = [
    { label: 'Hardware address', value: h('span', { class: 'mono' }, row.mac) },
    { label: 'Set up', value: row.serves ? `${row.mode}. It serves DHCP to other machines.` : row.mode },
    {
      label: 'Addresses',
      value: row.addresses.length === 0
        ? h('span', { class: 'muted' }, 'none')
        : lines(row.addresses.map((held) => [h('span', { class: 'mono' }, held.text), ' ', h('span', { class: 'pill' }, held.from)])),
    },
    { label: 'Router', value: row.routers.length === 0 ? null : lines(row.routers.map((router) => h('span', { class: 'mono' }, router))) },
    { label: 'Name servers', value: row.dns.length === 0 ? null : lines(row.dns.map((server) => h('span', { class: 'mono' }, server))) },
    { label: 'Metered', value: row.metered },
    { label: 'DHCP lease', value: row.lease },
  ];
  return h('dl', { class: 'rows net-rows' }, rows.filter((one) => one.value != null).map((one) => [
    h('dt', {}, one.label),
    h('dd', {}, one.value),
  ]));
}

/** The card `row` (see network-model.js's `cardRow`) as a section. */
function cardOf(row, { admin, view }) {
  const id = `net-${row.name}`;
  const change = h('a', { class: 'btn', href: row.href }, 'Change…');
  if (admin) {
    // A locked administrator is asked to unlock before the editor opens.
    change.addEventListener('click', async (event) => {
      if (isUnlocked()) return;
      event.preventDefault();
      if (await askToUnlock(view.session, 'change the network settings')) view.go(row.href);
    });
  }
  return h('section', { class: 'card net-card', 'aria-labelledby': `${id}-h`, 'data-card': row.name },
    h('h2', { id: `${id}-h` },
      h('a', { href: row.href }, row.name),
      h('span', { class: `st ${row.state.kind}` }, row.state.word)),
    row.through && h('p', { class: 'note net-through' }, 'You are connected through this card.'),
    factsOf(row),
    admin && h('div', { class: 'btns' }, change));
}

/**
 * The list of cards. `set(cards, nowMs)` draws the cards of `network show`;
 * the page's `view` gives the session and the time zone.
 */
export function makeCards(view) {
  const element = h('div', { class: 'net-cards' });
  let last = null;
  return {
    element,
    set(cards, nowMs) {
      const admin = view.session.admin === true;
      const rows = cards.map((card) => cardRow(card, view.session, nowMs, view.time));
      const key = JSON.stringify([admin, rows]);
      if (key === last) return;
      last = key;
      const held = element.contains(document.activeElement) ? document.activeElement.closest('[data-card]')?.dataset.card : null;
      fill(element, rows.map((row) => cardOf(row, { admin, view })));
      if (held) element.querySelector(`[data-card="${CSS.escape(held)}"] a`)?.focus();
    },
  };
}
