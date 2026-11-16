// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of certificates: each one's first name, who made it, when it ends
// and whether it renews by itself, which services use it and what can be done
// with it, and under a row whose last renewal failed, why and the two ways to
// go on. What a button does is the page's (`ctx.act`).

import { fill, h } from '../../dom.js';
import { button, makeCard, statusLine } from './cards.js';
import { failureWords } from './history-model.js';
import {
  accountWords, ACTION_WORDS, actionsOf, serviceHref, useOffer, WARNING_WORDS,
} from './model.js';

const COLUMNS = ['Certificate', 'From', 'Good until', 'Used by', 'Actions'];

/** The address the browser reaches the console by. */
const hostOf = () => globalThis.location?.hostname ?? '';

/** The card. `ctx` is the page's (see page.js). Gives `{element, errors, say, draw}`. */
export function makeList(ctx) {
  const card = makeCard('certificates', 'Certificates on this server');
  const status = statusLine();
  const { view } = ctx;

  /** What "Use for this console" is on `row`: a button, the words that say what to do first, or nothing. */
  function useItem(row, more) {
    const { data } = ctx;
    const offer = useOffer(row.certificate, {
      host: hostOf(), full: data.machine?.name, port: data.about?.port ?? globalThis.location?.port, serving: data.serving,
    });
    if (offer.offer === 'use') return button(offer.words, (one) => { more.open = false; ctx.act('use', row, one); }, 'small');
    if (offer.offer === 'open') return h('p', { class: 'small' }, 'Open the console at ', h('a', { href: offer.url }, offer.url), ' and choose it there');
    return offer.offer === 'none' ? h('p', { class: 'small muted' }, offer.words) : null;
  }

  /** The cell of the row's buttons, and under More those that are asked less often. */
  function actionsCell(row) {
    const can = actionsOf(row.certificate, { admin: ctx.admin, serving: ctx.data.serving, byDns: ctx.byDns });
    const one = (id) => button(ACTION_WORDS[id], (pressed) => ctx.act(id, row, pressed), id === 'renew' || id === 'replace' ? 'small default' : 'small');
    const more = can.more.length > 0 && h('details', { class: 'certs-more' });
    if (more) {
      const inside = h('div', { class: 'certs-menu' }, can.more.map((id) => {
        if (id === 'use') return useItem(row, more);
        const pressed = button(ACTION_WORDS[id], (target) => { more.open = false; ctx.act(id, row, target); }, `small${id === 'remove' || id === 'revoke' ? ' danger' : ''}`);
        return pressed;
      }), can.removeNote && h('p', { class: 'small muted' }, can.removeNote));
      fill(more, h('summary', { class: 'btn small', 'aria-label': `More for ${row.name}` }, 'More'), inside);
    }
    return h('td', { 'data-label': 'Actions', class: 'certs-actions' }, h('div', { class: 'btns' }, can.buttons.map(one), more));
  }

  /** The cell of the names: the first, and how many more, with the label of the one the console serves. */
  function nameCell(row) {
    const { machine } = ctx.data;
    const sub = row.kind === 'machine' ? 'made by this server' : (row.more > 0 ? `and ${row.more} more` : '');
    return h('td', { 'data-label': COLUMNS[0] },
      h('b', {}, row.kind === 'machine' ? (machine?.first ?? row.first) : row.first),
      row.console && [' ', h('span', { class: 'pill info' }, 'this console')],
      sub && h('small', {}, sub));
  }

  function usedCell(row) {
    return h('td', { 'data-label': COLUMNS[3] }, row.usedBy.length > 0
      ? row.usedBy.flatMap((name, at) => [at > 0 && ', ', h('a', { href: serviceHref(name) }, name)])
      : h('span', { class: 'small muted' }, row.unused));
  }

  /** The line under a row whose renewal failed: what happened, and the two ways to go on. */
  function failureRow(row) {
    const { failure } = row;
    const buttons = ctx.admin ? [
      button('Check again', (one) => ctx.act('check-again', row, one), 'small'),
      button('Retry', (one) => ctx.act('renew', row, one), 'small'),
    ] : [];
    return h('tr', { class: 'certs-failure' }, h('td', { colspan: String(COLUMNS.length) },
      h('p', { class: 'msg certs-tone-bad' }, h('span', {}, failureWords(failure, view.time.when))),
      h('p', { class: 'small' }, failure.words, failure.id && h('span', { class: 'mono muted' }, ` ${failure.id}`)),
      buttons.length > 0 && h('div', { class: 'btns' }, buttons)));
  }

  const rowOf = (row) => [
    h('tr', { 'data-name': row.name },
      nameCell(row),
      h('td', { 'data-label': COLUMNS[1] }, row.from.words, row.from.note && h('small', {}, row.from.note)),
      h('td', { 'data-label': COLUMNS[2] }, h('b', {}, row.end.date), row.end.note && h('small', { class: row.end.tone ? `certs-tone-${row.end.tone}` : null }, row.end.note)),
      usedCell(row),
      actionsCell(row)),
    row.failure && failureRow(row),
  ];

  function draw() {
    const { data } = ctx;
    if (data.list === undefined) {
      fill(card.body, h('p', { class: 'muted' }, 'The certificates could not be read.'));
      return;
    }
    if (data.list === null) {
      fill(card.body, h('p', { class: 'muted' }, 'Reading the certificates…'));
      return;
    }
    const rows = ctx.rows();
    const account = accountWords(data.list, view.time);
    fill(card.body,
      status.element,
      h('div', { class: 'tbl-box' },
        h('table', { class: 'tbl stack' },
          h('caption', { class: 'sr-only' }, `${rows.length} ${rows.length === 1 ? 'certificate' : 'certificates'}`),
          h('thead', {}, h('tr', {}, COLUMNS.map((name) => h('th', { scope: 'col' }, name)))),
          h('tbody', {}, rows.map(rowOf)))),
      account && h('p', { class: 'small muted' }, account),
      h('p', { class: 'small muted' }, WARNING_WORDS, ' ', h('a', { href: '/alerts' }, 'Open the alerts')));
  }

  return {
    element: card.element, errors: card.errors, say: status.say, draw,
  };
}
