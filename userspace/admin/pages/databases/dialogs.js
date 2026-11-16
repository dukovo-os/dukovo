// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The questions a database's page asks: which networks may connect, whether a
// new password is wanted, whether to stop a network and what to remove. Each
// says what the change does before it is made and shows the command while
// commands are shown. Read when the first is opened.

import { h } from '../../dom.js';
import {
  allowArgs, removeArgs, restarts, secretOf,
} from '../../lib/database-model.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { ask } from '../../shell/dialogs.js';
import { accessFields } from './fields.js';
import { endsSessions, removeLead } from './model.js';

/**
 * Asks which networks may connect to `name`, and with which certificate.
 * `server` is the last read's, which says whether PostgreSQL restarts.
 * Gives `{networks, certificate}`, or null when the person cancelled; the
 * call is made by the page, which shows its steps.
 */
export async function askAllow(view, {
  name, server, machine, certificates,
}) {
  const access = accessFields({ certificates, machine });
  const restart = h('p', {});
  const words = () => {
    restart.textContent = restarts(server, access.certificate.input.value)
      ? 'PostgreSQL restarts once to listen on the network: its open sessions end.'
      : 'PostgreSQL is listening on the network already: it reads its settings again, and no session ends.';
  };
  words();
  access.certificate.input.addEventListener('change', words);
  let chosen = null;
  const done = await openFormDialog(view, {
    title: `Let other machines connect to ${name}`,
    icon: 'lock',
    lead: [
      `Lets the addresses you name sign in to ${name} with its password, over TLS with this server's certificate. Programs on this server connect as before.`,
      restart,
    ],
    fields: access.elements,
    label: 'Let them connect',
    command: () => {
      const asked = access.check();
      return Object.keys(asked.problems).length === 0 ? { action: 'databases.allow', args: allowArgs(name, asked.found.networks, asked.certificate) } : null;
    },
    async submit() {
      const asked = access.check();
      if (asked.problems.networks) access.networks.fault(asked.problems.networks);
      else if (asked.problems.anywhere) access.anywhere.fault(asked.problems.anywhere);
      else chosen = { networks: asked.found.networks, certificate: asked.certificate };
      return chosen !== null;
    },
  });
  return done ? chosen : null;
}

/**
 * Asks whether to stop `from` (one network, or every one when null) from
 * connecting to `name`. `restarts` says whether PostgreSQL goes back to
 * listening on this machine only. Gives whether to go on.
 */
export async function askStop(view, { name, from, restarts: restart }) {
  const who = from === null ? 'every network' : from;
  const choice = await ask({
    title: from === null ? `Stop other machines connecting to ${name}?` : `Stop ${from} connecting to ${name}?`,
    words: [
      `Programs on ${who} can no longer sign in to ${name}. Programs on this server connect as before.`,
      restart ? 'No other database lets other machines in, so PostgreSQL goes back to listening on this machine only, and restarts once: its open sessions end.' : 'PostgreSQL reads its settings again; no session ends.',
    ],
    iconName: 'warn',
    choices: [
      { label: 'Keep it', value: 'keep', kind: 'default', safe: true },
      { label: from === null ? 'Stop them' : `Stop ${from}`, value: 'stop', kind: 'danger' },
    ],
  });
  return choice === 'stop';
}

/**
 * Asks whether to make a new password for `name`, and makes it: the old one
 * stops working at once. Gives what the answer held (`secretOf`'s), which
 * the caller shows once and lets go of, or null.
 */
export async function askNewPassword(view, { name }) {
  let secret = null;
  const done = await openFormDialog(view, {
    title: `Make a new password for ${name}?`,
    icon: 'key',
    lead: ['Programs that use the old one stop signing in until they have the new one.', 'The new password is shown once, as the first was.'],
    label: 'Make a new password',
    command: { action: 'databases.password', args: { name } },
    async submit(button, slot) {
      // The answer is a secret: it is asked once, and kept nowhere but in `secret`, until it is shown.
      const result = await view.api.run('databases.password', { name }, {
        button,
        where: { slot },
        words: { unlock: `make a new password for ${name}`, running: 'Making a new password', done: `A new password is made for ${name}.` },
      });
      if (!result.ok) return false;
      secret = secretOf(result.result);
      return true;
    },
  });
  if (done && secret === null) view.notice(`The password of ${name} was changed, but the answer held no password. Make another.`, { kind: 'warn' });
  return secret;
}

/**
 * Asks, and removes the database `db` (a row of the read) after the name is
 * typed. `removal` is `removeOf`'s. Gives whether it was removed.
 */
export async function askRemove(view, { db, removal }) {
  const name = db.name;
  const tick = h('input', { id: 'db-remove-role', type: 'checkbox', checked: removal.ticked });
  const role = removal.offered && h('div', { class: 'field' },
    h('label', { class: 'check', for: tick.id }, tick, h('b', {}, `Also remove the role ${db.owner}`)),
    h('span', { class: 'hint' }, 'It was made with the database, and it owns nothing else that this page knows of.'));
  return openFormDialog(view, {
    title: `Remove ${name}?`,
    icon: 'warn',
    lead: [removeLead(name), db.sessions > 0 && endsSessions(db.sessions), removal.shared, removal.stays].filter(Boolean),
    fields: [role].filter(Boolean),
    label: 'Remove the database',
    danger: true,
    typed: { name, label: `Type ${name} to go on`, why: 'The name is typed so that the database is not removed by a slip.' },
    command: () => ({ action: 'databases.remove', args: removeArgs(name, { role: removal.offered && tick.checked, force: db.sessions > 0 }) }),
    async submit(button, slot) {
      const args = removeArgs(name, { role: removal.offered && tick.checked, force: db.sessions > 0 });
      const result = await view.api.run('databases.remove', args, {
        button, where: { slot }, words: { unlock: `remove ${name}`, running: `Removing ${name}`, done: `${name} is removed.` },
      });
      return result.ok;
    },
  });
}
