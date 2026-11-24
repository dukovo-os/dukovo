// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Clients: a row for each client with how it connects, what it reads, what
// it may ask to change, when it was last used and when it ends, and Remove,
// which asks first. A locked console lists the person's own clients; an
// unlocked one lists every administrator's, which is `console.clients`. A
// person's own client is removed by their own route, and another's by
// `console.client-remove`.

import { fill, h } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { askToUnlock } from '../../unlock.js';
import { clientName, clientRows } from './model.js';
import { plainCommand, section, unread } from './parts.js';

const NONE = 'No client is let in. Add one, and an AI assistant of your choice can look after this server.';
const BELONGS = 'A client belongs to the administrator who added it. It never does more than that account may, and it stops when the account does.';
const STOPS = 'It stops at once: its token or its SSH command lets nothing in from its next message on, and what it asked for and nobody answered lapses.';

/** The route that removes one of the person's own clients, which is not an action: authd keeps them. */
const OWN_REMOVE = '/api/1/account/clients/remove';

/** A cell of areas: all of them in words, none in words, or a chip each. */
function reach({ every, titles }, all) {
  if (every) return all;
  if (titles.length === 0) return h('span', { class: 'muted' }, 'Nothing');
  return h('span', { class: 'chips' }, titles.map((title) => h('span', { class: 'chip' }, title)));
}

/** The Clients card. `ctx` is the page's: `{view, load}`. Gives `{element, set(found)}`. */
export function makeList(ctx) {
  const { view } = ctx;
  const card = section('cl-clients', 'Clients');
  const ownCommand = plainCommand(view, 'admin mcp list');

  /** Removes the client of `row`; gives whether it was removed. */
  async function remove(row, own, confirm, errors) {
    if (!own) {
      return (await view.api.run('console.client-remove', { owner: row.owner, name: row.name }, {
        button: confirm, where: { slot: errors }, words: { unlock: `remove ${row.owner}'s client ${row.name}`, done: `${row.name} is removed.` },
      })).ok === true;
    }
    try {
      await view.api.post(OWN_REMOVE, { name: row.name });
      view.notice(`${row.name} is removed.`, { kind: 'ok' });
      return true;
    } catch (error) {
      view.problem(error, { slot: errors });
      return false;
    }
  }

  function rowOf(row) {
    const own = row.owner === view.session.name;
    const named = clientName(row.owner, row.name, view.session.name);
    const button = h('button', { class: 'btn small', type: 'button', 'aria-label': `Remove the client ${named}` }, 'Remove');
    button.addEventListener('click', async () => {
      const done = await openFormDialog(view, {
        title: `Remove ${named}?`,
        icon: 'warn',
        lead: [STOPS],
        label: 'Remove',
        danger: true,
        command: own ? null : { action: 'console.client-remove', args: { owner: row.owner, name: row.name } },
        submit: (confirm, errors) => remove(row, own, confirm, errors),
      });
      if (done) ctx.load();
    });
    return h('tr', {},
      h('td', {}, h('b', {}, row.name), h('small', {}, row.owner)),
      h('td', {}, row.way),
      h('td', {}, reach(row.reads, 'Every area')),
      h('td', {}, reach(row.asks, 'Every area a client may')),
      h('td', {}, row.used, row.from && h('small', {}, row.from)),
      h('td', {}, row.ended ? h('span', { class: 'pill' }, row.ends) : row.ends),
      h('td', { class: 'end' }, button));
  }

  /** The line for a locked console, which lists the person's own clients alone. */
  function ownOnly() {
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', async () => {
      if (await askToUnlock(view.session, 'see every administrator\'s clients')) ctx.load();
    });
    return h('p', { class: 'small muted cl-own' }, 'These are your own clients. Unlock to see every administrator\'s. ', unlock);
  }

  return {
    element: card.element,
    /** The clients: `found` is `{result, all}`, a read of the page's and whether it is every administrator's; `areas` the areas. */
    set(found, areas) {
      if (found.result.state !== 'ok') {
        fill(card.body, unread(found.result, 'The clients', ctx.load));
        return;
      }
      const clients = Array.isArray(found.result.value?.clients) ? found.result.value.clients : [];
      const rows = clientRows(clients, areas, view.time);
      const heads = ['Client', 'Connects over', 'Reads', 'May ask to change', 'Last used', 'Ends'];
      fill(card.body,
        rows.length === 0
          ? h('p', { class: 'muted' }, NONE)
          : h('div', { class: 'tbl-box' }, h('table', { class: 'tbl' },
            h('caption', { class: 'sr-only' }, found.all ? 'Every administrator\'s AI clients' : 'Your AI clients'),
            h('thead', {}, h('tr', {}, heads.map((head) => h('th', { scope: 'col' }, head)), h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Action')))),
            h('tbody', {}, rows.map(rowOf)))),
        !found.all && ownOnly(),
        h('p', { class: 'small muted' }, BELONGS),
        found.all
          ? view.api.inline('console.clients', {})
          : ownCommand);
    },
  };
}
