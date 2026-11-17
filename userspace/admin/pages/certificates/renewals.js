// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Renewals card: what certd and `cert` did to the certificates, newest
// first, for the last 90 days or all of it, with a failure and the success
// that followed it as one line.

import { fill, h } from '../../dom.js';
import { button, makeCard } from './cards.js';
import { linesOf, shownLines } from './history-model.js';

/** The card. `ctx` is the page's (see page.js). Gives `{element, errors, draw}`. */
export function makeRenewals(ctx) {
  const card = makeCard('renewals', 'Renewals');
  let all = false;

  const lineOf = (line) => h('li', { class: 'certs-line' },
    h('span', { class: 'certs-when' }, ctx.view.time.when(line.at)),
    ' · ',
    h('b', {}, line.name), line.tone === 'bad' ? ': ' : ' ', line.words,
    (line.detail || line.id) && [' · ', line.detail, line.id && h('span', { class: 'mono muted' }, ` (${line.id})`)],
    line.then && h('span', { class: 'small muted certs-then' }, ` ${line.then}`));

  function draw() {
    const { data } = ctx;
    if (data.history === undefined) {
      fill(card.body, h('p', { class: 'muted' }, 'What happened to the certificates could not be read.'));
      return;
    }
    if (data.history === null) {
      fill(card.body, h('p', { class: 'muted' }, 'Reading what happened…'));
      return;
    }
    const exists = new Set((data.list ?? []).map((one) => one.name));
    const lines = linesOf(data.history, { exists, zone: ctx.view.time.zone });
    const shown = shownLines(lines, all, Date.now());
    fill(card.body,
      shown.length > 0 ? h('ul', { class: 'certs-lines' }, shown.map(lineOf)) : h('p', { class: 'muted' }, lines.length > 0 ? 'Nothing in the last 90 days.' : 'Nothing has happened to a certificate yet.'),
      !all && lines.length > shown.length && button('Show all', () => {
        all = true;
        draw();
      }, 'small'));
  }

  return { element: card.element, errors: card.errors, draw };
}
