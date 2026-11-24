// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The AI clients page (ADR 0690): one route, /clients, for administrators. What
// waits for a yes, the clients, what they did and what no client can do,
// with Add a client… at the top. A locked console lists the person's own
// clients and an unlocked one every administrator's. The page reads when
// it opens, when the stream says that a request moved (`clients`), when
// the console is locked or unlocked and after a change of its own, never
// on a timer.

import { h, icon } from '../../dom.js';
import { isUnlocked } from '../../unlock.js';
import { makeDid, neverCard } from './did.js';
import { makeList } from './list.js';
import { read } from './parts.js';
import { makeWaiting } from './waiting.js';

const LEAD = 'An AI assistant of your choice can look after this server through the Model Context Protocol (MCP). A client reads only the areas you choose. It changes nothing by itself: each change it asks for waits here for an administrator\'s yes.';
const SENT = ' Add a client only for a service you trust with this server\'s logs and settings.';

const PATHS = {
  areas: '/api/1/clients/areas',
  requests: '/api/1/clients/requests',
  history: '/api/1/clients/history',
  own: '/api/1/account/clients',
  all: '/api/1/console/clients',
  about: '/api/1/console/about',
  machine: '/api/1/certificates/this-machine',
};

/** What a reader sees: the page is for administrators, since a client is let in by one and acts for one. */
export async function reader(view) {
  view.head({ title: 'AI clients' });
  view.main.append(h('div', { class: 'banner warn' }, icon('lock'), h('div', {},
    h('p', {}, h('b', {}, 'AI clients are for administrators.'), ` ${view.session.name}'s account can look but not change, and a client never does more than the account that added it. Ask an administrator, or ask to be made one.`),
    h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/accounts' }, 'Who are the administrators?')))));
}

export async function show(view) {
  const add = h('button', { class: 'btn default', type: 'button', disabled: true }, icon('plus'), 'Add a client…');
  view.head({
    title: 'AI clients', commands: true, buttons: [add], lead: LEAD,
  });
  const ctx = { view, load: () => load() };
  const waiting = makeWaiting(ctx);
  const list = makeList(ctx);
  const did = makeDid(ctx);
  view.main.append(h('div', { class: 'cl' },
    h('div', { class: 'banner info' }, icon('info'), h('div', {},
      h('p', {}, h('b', {}, 'What a client reads is sent to the AI service behind it.'), SENT))),
    waiting.element,
    list.element,
    h('div', { class: 'grid two' }, did.element, neverCard())));

  // What the Add dialog needs of the last read: the areas, and the names the person's own clients have.
  let areas = [];
  let taken = [];
  // What the dialog that connects a client says of this server, read once: neither changes while the page is open.
  const server = { about: null, machine: null };
  for (const part of ['about', 'machine']) {
    view.api.get(PATHS[part]).then((answer) => {
      server[part] = answer;
    }, () => {});
  }

  /** The clients: every administrator's while the console is unlocked, else the person's own. */
  async function clients() {
    if (isUnlocked()) {
      const result = await read(view, PATHS.all);
      if (result.state !== 'locked') return { result, all: true };
    }
    return { result: await read(view, PATHS.own), all: false };
  }

  let latest = 0;
  /** Reads the page again. Only the last read is drawn. */
  async function load() {
    latest += 1;
    const mine = latest;
    const [found, requests, history, listed] = await Promise.all([read(view, PATHS.areas), read(view, PATHS.requests), read(view, PATHS.history), clients()]);
    if (mine !== latest || view.signal.aborted) return;
    areas = found.state === 'ok' && Array.isArray(found.value?.areas) ? found.value.areas : [];
    const shown = listed.result.state === 'ok' && Array.isArray(listed.result.value?.clients) ? listed.result.value.clients : [];
    taken = shown.filter((client) => client.owner === view.session.name).map((client) => client.name);
    add.disabled = areas.length === 0;
    waiting.set(requests);
    // A row says what a client reaches by the areas' names: without the areas there is nothing true to say.
    list.set(found.state === 'ok' ? listed : { result: found, all: listed.all }, areas);
    did.set(found.state === 'ok' ? history : found, areas);
  }

  add.addEventListener('click', async () => {
    const { openAdd } = await import('./add.js');
    const added = await openAdd(view, { areas, taken });
    if (added === null) return;
    load();
    const { openConnect } = await import('./connect.js');
    await openConnect(view, added, server);
  });

  view.on('clients', load);
  view.on('unlock', load);
  await load();
}
