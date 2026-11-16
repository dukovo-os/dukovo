// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The rows of one account that are changed in place (design 7.5 and 7.6): the
// full name, the groups, the right to administer and to sign in to the
// console, the password and the shell. On the person's own account the
// administrator right and the console are told and not switched: the
// console never takes away what the person is using (ADM-136), and says where
// it is done instead.

import { h } from '../../dom.js';
import { passwordWords } from '../../lib/account-model.js';
import {
  nameCall, passwordCall, rightsCall, SELF, showPasswordArgs,
} from './model.js';
import { openForm } from './open.js';
import { rowSwitch } from './switch.js';

/** One row of the list: `label` and what it says. */
const row = (label, ...content) => [h('dt', {}, label), h('dd', {}, ...content)];

/** A button with `words` that calls `ask` when pressed; `label` is its name when the words alone do not say what it changes. */
function button(words, ask, label, action) {
  const one = h('button', {
    class: 'btn small', type: 'button', 'aria-label': label, 'data-action': action,
  }, words);
  one.addEventListener('click', ask);
  return one;
}

/** The full name's dialog. */
function changeName({ view, own, name, account, reload }) {
  return async () => {
    const [{ textField }, { fullNameProblem }] = await Promise.all([import('./fields.js'), import('./typed-model.js')]);
    const field = textField({ label: 'Full name', value: account.full_name ?? '', hint: 'As the person wants it written. It can be empty.' });
    const done = await openForm(view, {
      title: own ? 'Change your full name' : `Change the full name of ${name}`,
      icon: 'accounts',
      lead: [own ? 'Your full name is how others see you next to your account name.' : `${name}'s full name is shown next to the account name.`],
      fields: [field.element],
      label: 'Save',
      command: () => nameCall({ own, name, fullName: field.input.value.trim() }),
      async submit(confirm, errors) {
        const typed = field.input.value.trim();
        const problem = fullNameProblem(typed);
        if (problem !== '') {
          field.fault(problem);
          return false;
        }
        const call = nameCall({ own, name, fullName: typed });
        return (await view.api.run(call.action, call.args, {
          button: confirm, words: { unlock: `change the full name of ${name}`, done: 'The full name is saved.' }, where: { slot: errors, fields: { full_name: field.input } },
        })).ok;
      },
    });
    if (done) reload();
  };
}

/** The password's dialog: the person's own asks for the one they have, another's does not. */
function changePassword({ view, own, name, reload }) {
  return async () => {
    const { makeSecretField } = await import('../../lib/secret-field.js');
    const current = own ? makeSecretField({ label: 'Current password', kind: 'current', signal: view.signal }) : null;
    const next = makeSecretField({ label: 'New password', kind: 'password', confirm: true, signal: view.signal });
    const done = await openForm(view, {
      title: own ? 'Change your password' : `Change the password of ${name}`,
      icon: 'key',
      lead: own
        ? ['The new password is used from the next sign-in. Your other sessions stay signed in.']
        : [`${name} is not told the new password: give it to them yourself.`],
      fields: [current?.element, next.element].filter(Boolean),
      label: 'Change the password',
      command: showPasswordArgs({ own, name }),
      async submit(confirm, errors) {
        // The new one is taken first: two that differ leave both typed, for the person to put right.
        const password = next.take();
        if (password === null) return false;
        const had = current ? current.take() : '';
        if (had === null) return false;
        const call = passwordCall({
          own, name, current: had, password,
        });
        return (await view.api.run(call.action, call.args, {
          button: confirm, words: { unlock: `change the password of ${own ? 'your account' : name}`, done: 'The password is changed.' }, where: { slot: errors },
        })).ok;
      },
    });
    if (done) reload();
  };
}

/** The question before the administrator right is given or taken away. */
function askAdmin({ view, name, reload }, on) {
  return async (switchButton) => {
    const call = rightsCall({ name, admin: on });
    const done = await openForm(view, {
      title: on ? `Make ${name} an administrator?` : `Take away ${name}'s administrator right?`,
      icon: on ? 'info' : 'warn',
      lead: on
        ? [`${name} can change everything on this server once they unlock the console, and run commands as root with elevate. Give this to a person you trust with the whole machine.`]
        : [`${name} keeps the account and its files, and can no longer change the server's settings. A server always keeps one administrator: if this is the last, it is refused.`],
      label: on ? 'Make administrator' : 'Take it away',
      danger: !on,
      command: call,
      submit: async (confirm, errors) => (await view.api.run(call.action, call.args, {
        button: confirm, words: { unlock: `${on ? 'give' : 'take away'} ${name}'s administrator right`, done: 'Saved.' }, where: { slot: errors },
      })).ok,
    });
    if (done) reload();
    else switchButton.focus();
  };
}

/** The switch for the console, for an account that is not an administrator's. */
function consoleSwitch({ view, name, account, reload }) {
  const fixed = account.admin === true;
  return rowSwitch({
    label: 'Can sign in to Dukovo Admin',
    on: fixed || account.console === true,
    fixed,
    why: fixed ? 'An administrator can always sign in to Dukovo Admin.' : null,
    action: 'console',
    async toggle(switchButton) {
      const call = rightsCall({ name, console: account.console !== true });
      const done = await view.api.run(call.action, call.args, {
        button: switchButton, words: { unlock: `change whether ${name} can sign in to Dukovo Admin`, busy: 'Saving…', done: 'Saved.' },
      });
      if (done.ok) reload();
    },
  });
}

/** What the row of the administrator right says on the account in use, which the console does not change. */
const selfNote = () => h('div', { class: 'small muted' }, `The console does not take this away from the account you are signed in with. ${SELF.words} `, h('code', {}, SELF.command), '.');

/**
 * The rows of the account, each as `[dt, dd]` for a `dl`: `{name, groups,
 * admin, console, password, shell}`. `ctx` is `{view, kind, own, name,
 * account, reload}` with `kind` `own` or `full`.
 */
export function accountRows(ctx) {
  const {
    view, kind, own, name, account,
  } = ctx;
  const full = kind === 'full';
  const password = passwordWords(account, view.time.zone);
  const groups = account.groups ?? [];
  return {
    name: row('Full name', account.full_name ? account.full_name : h('span', { class: 'muted' }, 'None'), ' ',
      button('Change', changeName(ctx), own ? 'Change your full name' : `Change the full name of ${name}`, 'full-name')),
    groups: row('Groups', groups.map((group) => h('span', { class: 'acc-chip' }, group)), groups.length === 0 && h('span', { class: 'muted' }, 'None')),
    admin: row('Administrator', full
      ? rowSwitch({
        label: 'Administrator', on: account.admin === true, action: 'admin', toggle: askAdmin(ctx, account.admin !== true),
      })
      : [account.admin ? 'Yes' : 'No', account.admin && selfNote()]),
    console: row('Console', full
      ? consoleSwitch(ctx)
      : (account.admin || account.console ? 'Can sign in to Dukovo Admin' : 'Cannot sign in to Dukovo Admin')),
    password: row('Password',
      password && [password, ' '],
      button('Change…', changePassword(ctx), own ? 'Change your password' : `Change the password of ${name}`, 'password'),
      h('div', { class: 'small muted' }, 'Never expires. Dukovo never forces a change.')),
    shell: row('Shell', h('span', { class: 'mono' }, account.shell || 'None')),
  };
}
