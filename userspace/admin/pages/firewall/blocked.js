// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What was blocked in the last 24 hours and what listens (design 10.3), the two
// cards an administrator alone reads. Each blocked group says who tried what
// and why it was dropped, with Allow this, which fills the add form with the
// rule that would let it in; the blocks for guessing passwords are apart, with
// a link to Security when the console has that page.

import { h } from '../../dom.js';
import { pageById } from '../../pages.js';
import { makeAdminCard } from './admin-card.js';
import { dropRows, listenerRows } from './report-model.js';

/** The words of a card of this page that is read by an administrator alone: what the unlock says, and the lines for a reader and a locked administrator. */
const wordsFor = (what, unlock) => ({
  unlock,
  reader: `Administrators see ${what}.`,
  locked: `Unlock to see ${what}.`,
  failed: `${what.charAt(0).toUpperCase()}${what.slice(1)} could not be read.`,
});

/**
 * The Blocked card. `onAllow(words)` is called with the rule that Allow this
 * stands for, as the add form takes it. Gives `{element, load, set({trial})}`.
 */
export function makeBlocked(view, { onAllow }) {
  const card = makeAdminCard(view, {
    id: 'fw-blocked',
    title: 'Blocked in the last 24 hours',
    action: 'firewall.dropped',
    args: { since: '24h' },
    words: wordsFor('what the firewall blocked', 'see what the firewall blocked'),
    draw(answer, { trial = false } = {}) {
      const { groups, blocked } = dropRows(answer, view.time);
      const items = groups.map((group) => {
        const allow = h('button', {
          class: 'btn small', type: 'button', 'aria-disabled': trial ? 'true' : null,
          'aria-label': `Allow this: ${group.allow}`,
        }, 'Allow this');
        allow.addEventListener('click', () => {
          if (allow.getAttribute('aria-disabled') !== 'true') onAllow(group.allow, allow);
        });
        return h('li', { class: 'fw-drop' },
          h('div', { class: 'fw-drop-words' }, h('time', { title: group.full }, group.words), h('small', { class: 'muted' }, group.why)),
          allow);
      });
      const security = pageById('security');
      return [
        items.length === 0 && blocked.length === 0 ? h('p', { class: 'muted' }, 'The firewall blocked nothing it logged in the last 24 hours.') : null,
        items.length > 0 && [
          h('ul', { class: 'fw-drops' }, items),
          h('p', { class: 'small muted' }, 'A count is of the lines the kernel logged, at most five a second, so more were blocked.'),
        ],
        blocked.length > 0 && h('div', { class: 'fw-guess' },
          h('h3', {}, 'Blocked for guessing passwords'),
          h('ul', { class: 'fw-drops' }, blocked.map((group) => h('li', { class: 'fw-drop' }, h('div', { class: 'fw-drop-words' }, h('time', { title: group.full }, group.words))))),
          security && h('p', {}, h('a', { href: '/security' }, 'Manage these blocks on Security'))),
      ];
    },
  });
  return card;
}

/** The What listens card: the sockets the machine listens on, and who reaches each. Gives `{element, load}`. */
export function makeListening(view) {
  return makeAdminCard(view, {
    id: 'fw-listening',
    title: 'What listens',
    action: 'firewall.status',
    args: {},
    words: wordsFor('what listens on this machine', 'see what listens on this machine'),
    draw(answer) {
      const rows = listenerRows(answer);
      if (rows.length === 0) return h('p', { class: 'muted' }, 'Nothing listens on this machine.');
      return h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack fw-tbl' },
        h('caption', { class: 'sr-only' }, 'What listens, and who reaches it'),
        h('thead', {}, h('tr', {}, ['Port', 'Program', 'Who reaches it'].map((name) => h('th', { scope: 'col' }, name)))),
        h('tbody', {}, rows.map((row) => h('tr', {},
          h('th', { scope: 'row', class: 'mono', 'data-label': 'Port' }, row.port),
          h('td', { class: 'mono', 'data-label': 'Program' }, row.program),
          h('td', { 'data-label': 'Who reaches it' }, row.words))))));
    },
  });
}
