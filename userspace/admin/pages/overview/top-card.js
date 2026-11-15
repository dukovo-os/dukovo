// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Using the most memory (design 10): the five services that hold the most
// memory, with their CPU and what they are, from the latest values of
// `metrics.now` and the list of services.

import { h } from '../../dom.js';
import { makeCard, reader } from './card.js';
import { noNumbersWords, topServices } from './model.js';

/** The card. */
export function makeTop(view) {
  const card = makeCard('top', 'Using the most memory');
  const load = reader(view, card.body, {
    async read() {
      const [now, show] = await Promise.all([
        view.api.get('/api/1/metrics/now').catch((error) => {
          // No latest values yet is the machine's, not a failure.
          if (error?.id === 'ADM-123') return null;
          throw error;
        }),
        view.api.get('/api/1/services').catch(() => null),
      ]);
      return { found: topServices(now, show), show };
    },
    draw({ found, show }) {
      if (found.length === 0) {
        card.body.replaceChildren(h('p', { class: 'muted' }, noNumbersWords(show)));
        return;
      }
      card.body.replaceChildren(h('div', { class: 'tbl-box' }, h('table', { class: 'tbl' },
        h('caption', { class: 'sr-only' }, 'The services that use the most memory'),
        h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, 'Service'), h('th', { scope: 'col', class: 'num' }, 'Memory'), h('th', { scope: 'col', class: 'num' }, 'CPU'))),
        h('tbody', {}, ...found.map((one) => h('tr', {},
          h('td', {}, h('b', {}, one.name), one.description && h('small', {}, one.description)),
          h('td', { class: 'num' }, one.memory),
          h('td', { class: 'num' }, one.cpu)))))));
    },
  });
  return { element: card.element, load };
}
