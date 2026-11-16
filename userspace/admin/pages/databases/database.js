// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A database's page: its facts, how programs connect to it, who may and the
// changes that follow: let other machines connect, stop them, make a new
// password, remove it. The read is an administrator's and is made when the page
// opens, after a change and when the console is unlocked, never on a timer.
// Each change that PostgreSQL may restart for is a task, shown as the steps of
// lib/steps.js; the questions are read when a button is pressed.

import { fill, h } from '../../dom.js';
import { allowArgs, denyArgs, machineOf } from '../../lib/database-model.js';
import { runSteps } from '../../lib/steps.js';
import { PAGES } from '../../pages.js';
import { matchPage } from '../../shell/route-model.js';
import { problemsBanner, reindexNotice, serverCard } from './cards.js';
import {
  accessOf, denyRestarts, factsOf, guessMachine, handWords, removeOf, serverState, sessionsWords, THIS_MACHINE, waysOf,
} from './model.js';
import { makeRead } from './read.js';
import { connectionCard } from './settings.js';

/** Whether the console has a page for `path`. */
const reaches = (path) => matchPage(PAGES, path)?.entry.load != null;

/** The buttons of the cards have a command under them while commands are shown. */
const withCommand = (view, button, action, args) => h('div', { class: 'db-action' }, button, view.api.inline(action, args));

export async function showDatabase(view, name) {
  view.head({ title: name, crumbs: [{ title: 'Apps', path: '/apps' }, { title: 'Databases', path: '/databases' }], commands: true });
  const errors = h('div', { class: 'db-errors' });
  const read = makeRead(view, { errors });
  const body = h('div', { class: 'db-database' });
  const flowSlot = h('div', { class: 'db-flow' });
  view.main.append(h('div', { class: 'db-page' }, errors, read.element, flowSlot, body));
  let machine = null;
  let running = false;
  let asked = false;

  /** A button of a card, which is off while a change runs. */
  const button = (words, { kind = '', action, onPress }) => {
    const one = h('button', {
      class: `btn${kind ? ` ${kind}` : ''}`, type: 'button', disabled: running, 'data-action': action, 'data-guard': '',
    }, words);
    one.addEventListener('click', onPress);
    return one;
  };

  // ---- the changes ----

  /** Runs `steps` under `title`, then reads again. Gives how they ended. */
  async function flow(title, steps) {
    running = true;
    draw();
    const run = runSteps(view, steps);
    fill(flowSlot, h('section', { class: 'card', 'aria-labelledby': 'db-flow-h' }, h('h2', { id: 'db-flow-h' }, title), run.element));
    const outcome = await run.finished;
    running = false;
    if (!outcome.left) await read.read();
    draw();
    return outcome;
  }

  async function allow() {
    const { askAllow } = await import('./dialogs.js');
    const certificates = await view.api.get('/api/1/certificates').catch(() => null);
    const picked = await askAllow(view, {
      name, server: read.data.server, machine: machine ?? guessMachine(view.host ?? ''), certificates,
    });
    if (picked === null || view.signal.aborted) return;
    await flow(`Letting other machines connect to ${name}`, [{
      title: 'Let other machines connect',
      action: 'databases.allow',
      args: allowArgs(name, picked.networks, picked.certificate),
      wait: 'task',
      words: {
        unlock: `let other machines connect to ${name}`,
        running: 'Letting other machines connect',
        done: 'Other machines can connect.',
        failed: 'Other machines cannot connect yet',
        stands: 'PostgreSQL\'s settings are as they were.',
      },
    }]);
  }

  async function stop(from) {
    const { askStop } = await import('./dialogs.js');
    if (!(await askStop(view, { name, from, restarts: denyRestarts(read.data, name, from) })) || view.signal.aborted) return;
    await flow(from === null ? `Stopping other machines connecting to ${name}` : `Stopping ${from} connecting to ${name}`, [{
      title: from === null ? 'Stop other machines connecting' : `Stop ${from} connecting`,
      action: 'databases.deny',
      args: denyArgs(name, from),
      wait: 'task',
      words: {
        unlock: `stop other machines connecting to ${name}`,
        running: 'Stopping them',
        done: 'They can no longer connect.',
        failed: 'They can still connect',
        stands: 'PostgreSQL\'s settings are as they were.',
      },
    }]);
  }

  async function newPassword() {
    const { askNewPassword } = await import('./dialogs.js');
    let secret = await askNewPassword(view, { name });
    if (secret === null) return;
    // The old password no longer works: the new one is the only copy there is until it is shown.
    view.leaving = () => `The new password of ${name} is shown only once. If you leave now it is lost, and you would have to make another.`;
    const { showPassword } = await import('./once.js');
    const shown = waysOf(accessOf(read.data, read.data.databases.find((one) => one.name === name))).includes('network') ? 'network' : 'local';
    const kept = await showPassword(view, {
      name, secret, way: shown, machine: machine ?? guessMachine(view.host ?? ''),
    });
    secret = null;
    view.leaving = null;
    if (!kept && !view.signal.aborted) view.notice(`The new password of ${name} was not kept. Make another when you are ready.`, { kind: 'warn' });
  }

  async function remove(db) {
    const { askRemove } = await import('./dialogs.js');
    if (await askRemove(view, { db, removal: removeOf(read.data, db) }) && !view.signal.aborted) view.go('/databases');
  }

  // ---- the cards ----

  const factsCard = (db) => h('section', { class: 'card', 'aria-labelledby': 'db-facts-h' },
    h('h2', { id: 'db-facts-h' }, 'About this database'),
    h('dl', { class: 'rows' }, [...factsOf(db).flatMap((row) => [h('dt', {}, row.label), h('dd', {}, row.value)]),
      db.site && [h('dt', {}, 'For'), h('dd', {}, reaches(`/websites/${db.site}`) ? h('a', { href: `/websites/${encodeURIComponent(db.site)}` }, db.site) : db.site)]]),
    h('p', { class: 'small muted' }, sessionsWords(db.sessions)));

  const whoCard = (access) => {
    const lines = [
      access.password && h('li', {}, 'Programs on this server, with the password'),
      access.peer !== null && h('li', {}, `${access.peer}, by its account`),
      ...access.networks.map((one) => h('li', { class: 'db-line' },
        h('span', {}, `${one.address}, over TLS with the password`),
        h('button', {
          class: 'btn small', type: 'button', disabled: running, 'data-guard': '', 'aria-label': `Remove ${one.address}`, onclick: () => stop(one.address),
        }, 'Remove'))),
      ...access.hand.map((one) => h('li', {}, h('span', { class: 'mono' }, handWords(one)), h('small', { class: 'muted' }, 'Written by hand in pg_hba.conf: kept as it is.'))),
    ].filter(Boolean);
    return h('section', { class: 'card', 'aria-labelledby': 'db-who-h' },
      h('h2', { id: 'db-who-h' }, 'Who connects'),
      lines.length === 0 ? h('p', { class: 'muted' }, 'No one is known to connect.') : h('ul', { class: 'db-who' }, lines),
      h('div', { class: 'btns' },
        access.password && button('Let other machines connect', { action: 'allow', onPress: allow }),
        access.networks.length > 0 && withCommand(view, button('Stop them', { action: 'stop', onPress: () => stop(null) }), 'databases.deny', { name })));
  };

  const manageCard = (db, access) => h('section', { class: 'card', 'aria-labelledby': 'db-manage-h' },
    h('h2', { id: 'db-manage-h' }, 'Password and removal'),
    access.password
      ? h('div', { class: 'db-manage' },
        withCommand(view, button('Make a new password', { action: 'password', onPress: newPassword }), 'databases.password', { name }),
        h('p', { class: 'small muted' }, 'Programs that use the old one stop signing in until they have the new one.'))
      : access.peer !== null && h('p', { class: 'muted' }, `Its role signs in as the account ${access.peer}, so it has no password.`),
    h('div', { class: 'db-manage' },
      withCommand(view, button('Remove…', { kind: 'danger', action: 'remove', onPress: () => remove(db) }), 'databases.remove', { name }),
      h('p', { class: 'small muted' }, 'Removes the database and everything in it. Its data stays in the snapshots of /shared until they are pruned.')));

  // ---- drawing ----

  function draw() {
    const data = read.data;
    if (read.mode !== 'open' || data === null) {
      fill(body);
      return;
    }
    const db = data.databases.find((one) => one.name === name);
    const state = serverState(data.server);
    if (!db) {
      fill(body, state === 'on'
        ? h('section', { class: 'card', role: 'status' }, h('p', {}, `There is no database named ${name}.`), h('p', {}, h('a', { href: '/databases' }, 'All the databases')))
        : serverCard(view, data, () => read.read()));
      return;
    }
    const access = accessOf(data, db);
    fill(body,
      state !== 'on' && serverCard(view, data, () => read.read()),
      problemsBanner(data),
      db.mismatch && reindexNotice(name),
      factsCard(db),
      connectionCard(view, {
        database: name, role: db.owner, ways: waysOf(access), machine: machine ?? guessMachine(view.host ?? ''), certificate: data.server.certificate ?? THIS_MACHINE,
      }),
      whoCard(access),
      manageCard(db, access));
    // Make a new password, from the palette: the question is asked, and nothing is done until it is answered.
    if (view.query.has('password') && !asked && access.password) {
      asked = true;
      newPassword();
    }
  }

  read.on(draw);
  view.api.get('/api/1/machine/name').then((answer) => {
    machine = machineOf(answer);
    draw();
  }, () => {});
  await read.load();
  if (read.mode === 'open' && read.data === null) {
    const again = h('button', { class: 'btn', type: 'button' }, 'Try again');
    again.addEventListener('click', () => read.read());
    fill(body, h('p', { class: 'muted' }, 'The databases could not be read.'), h('div', { class: 'btns' }, again));
  }
}
