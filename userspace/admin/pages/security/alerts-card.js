// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Alerts card of Security: where each kind of alert goes, a test alert and
// the way to the Alerts page. Only an administrator may read where alerts go,
// so the card reads it when the console is unlocked, and says who sees it
// otherwise.

import { h, icon } from '../../dom.js';
import { kindRows, listOf } from '../alerts/channels-model.js';
import { adminReader, makeCard } from './card.js';

/** The card. Gives `{element, load}`; `load()` reads the channels when the console is unlocked. */
export function makeAlerts(view) {
  const card = makeCard('alerts', 'Alerts');
  const said = h('p', { class: 'small', role: 'status' });

  const reader = adminReader(view, card, {
    action: 'alerts.channels',
    words: {
      unlock: 'see where alerts go',
      reader: 'Alerts are for administrators.',
      locked: 'Unlock to see where alerts go.',
      failed: 'Where alerts go could not be read.',
      missing: 'This server shows where alerts go with a later update of Dukovo.',
    },
    draw(answer) {
      const channels = listOf(answer);
      said.textContent = '';
      if (channels.length === 0) {
        return h('div', { class: 'banner warn', role: 'status' }, icon('warn'), h('div', {},
          h('p', {}, h('b', {}, 'Alerts reach this server\'s screens only. '), 'No email, webhook or browser is set up to take them.'),
          h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/alerts' }, 'Set up alerts'))));
      }
      return [
        h('dl', { class: 'rows' }, kindRows(channels).flatMap((row) => [
          h('dt', {}, row.label),
          h('dd', { class: row.tone === 'warn' ? 'warn' : null }, row.tone === 'warn' && icon('warn'), row.words),
        ])),
        h('div', { class: 'btns' },
          h('button', { class: 'btn small', type: 'button', onclick: (event) => import('./alerts-test.js').then(({ sendTest }) => sendTest(view, card, said, event.currentTarget)) }, 'Send a test alert'),
          h('a', { class: 'btn small', href: '/alerts' }, 'Alert settings')),
        said,
      ];
    },
  });
  return { element: card.element, load: reader.load };
}
