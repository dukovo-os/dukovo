// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// This server (design 10.4): what the machine says of itself and the person's
// last sign-ins, with the rows that other pages add through their `rows`
// part. model.js says which rows and in what order.

import { h } from '../../dom.js';
import { isZone, makeTime } from '../../lib/time.js';
import { makeCard, reader } from './card.js';
import { serverRows } from './model.js';

/** The card. `about()` gives `console.about`'s answer, which the page reads once for every card that needs it. */
export function makeServer(view, { about }) {
  const card = makeCard('server', 'This server');
  const load = reader(view, card.body, {
    async read() {
      const [facts, found] = await Promise.all([about(), view.parts('overview', (part, cx) => part.rows?.(cx) ?? [])]);
      return { facts, extra: found.flatMap((one) => (Array.isArray(one.value) ? one.value : [])) };
    },
    draw({ facts, extra }) {
      if (facts === null || typeof facts !== 'object') {
        card.body.replaceChildren(h('p', { class: 'muted' }, 'The machine did not say anything of itself.'));
        return;
      }
      const serverTime = isZone(facts.zone) ? makeTime(facts.zone) : null;
      const rows = serverRows(facts, { time: view.time, serverTime, now: Math.floor(Date.now() / 1000) }, extra);
      card.body.replaceChildren(h('dl', { class: 'rows' }, ...rows.flatMap((row) => [
        h('dt', {}, row.label),
        h('dd', { class: row.mono ? 'mono wrap' : '' }, row.value, row.more && [h('br'), h('span', { class: 'muted' }, row.more)]),
      ])));
    },
  });
  return { element: card.element, load };
}
