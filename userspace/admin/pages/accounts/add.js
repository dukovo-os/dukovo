// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add an account… (design 7.4): a name, a full name, whether the person
// administers, whether they can sign in to the console, and a password typed
// twice that may be left for later. The name and the full name are checked as
// the command checks them before it is asked. The password goes in the one
// call that makes the account and nowhere else, and the Show command panel is
// given none. After it the dialog offers an SSH key for the new account.

import { makeSecretField } from '../../lib/secret-field.js';
import { openForm } from './dialog.js';
import { formSwitch, textField } from './fields.js';
import { openAddKey } from './keys.js';
import { rightsCall } from './model.js';
import {
  addArgs, fullNameProblem, nameProblem, showAddArgs,
} from './typed-model.js';

/**
 * Opens Add an account…. `taken` are the names of the accounts that there
 * are, so that a name that is one is refused before the command is. Resolves
 * with the new account's name, or null when none was made.
 */
export async function openAdd(view, { taken }) {
  const name = textField({
    label: 'Name',
    mono: true,
    hint: 'Lowercase letters, digits, - _ and ., starting with a letter. It cannot be changed later.',
  });
  const full = textField({ label: 'Full name', hint: 'Optional.' });
  const admin = formSwitch({
    label: 'Administrator',
    hint: 'Can change everything on this server once they unlock the console. Give this only to a person you trust with the whole machine.',
  });
  const signIn = formSwitch({
    label: 'Can sign in to this console',
    hint: 'Dukovo Admin, with the things their account may do.',
    on: true,
  });
  const password = makeSecretField({
    label: 'Password',
    kind: 'password',
    confirm: true,
    optional: true,
    hint: 'Leave it empty to give one later; the account cannot sign in until it has one or an SSH key.',
    signal: view.signal,
  });
  const choseConsole = { on: true };
  signIn.input.addEventListener('change', () => { choseConsole.on = signIn.input.checked; });
  admin.input.addEventListener('change', () => {
    // An administrator can always sign in to the console: the switch says so and stays on.
    signIn.input.checked = admin.input.checked ? true : choseConsole.on;
    signIn.input.disabled = admin.input.checked;
  });

  /** What the fields say now, for the command shown under them: the password is not in it. */
  const shownCall = () => {
    const typed = name.input.value.trim();
    if (nameProblem(typed, taken) !== '') return null;
    const hasPassword = password.element.querySelector('input').value !== '';
    return {
      action: 'accounts.add',
      args: showAddArgs({
        name: typed, fullName: full.input.value.trim(), admin: admin.input.checked, hasPassword,
      }),
    };
  };

  let made = null;
  await openForm(view, {
    title: 'Add an account',
    icon: 'accounts',
    lead: ['Each person who uses this server signs in as themselves, so that what they do is theirs.'],
    fields: [name.element, full.element, admin.element, signIn.element, password.element],
    label: 'Add',
    command: shownCall,
    async submit(confirm, errors) {
      const typed = name.input.value.trim();
      const fault = nameProblem(typed, taken);
      if (fault !== '') {
        name.fault(fault);
        return false;
      }
      const fullFault = fullNameProblem(full.input.value.trim());
      if (fullFault !== '') {
        full.fault(fullFault);
        return false;
      }
      const secret = password.take();
      if (secret === null) return false;
      const wanted = {
        name: typed, fullName: full.input.value.trim(), admin: admin.input.checked, password: secret,
      };
      const done = await view.api.run('accounts.add', addArgs(wanted), {
        button: confirm,
        words: { unlock: 'add an account', running: 'Adding the account', done: `${typed} is added.` },
        where: { slot: errors, fields: { name: name.input, full_name: full.input } },
      });
      if (!done.ok) return false;
      made = typed;
      if (!wanted.admin) {
        const call = rightsCall({ name: typed, console: signIn.input.checked });
        const set = await view.api.run(call.action, call.args, { words: { unlock: `change whether ${typed} can sign in to Dukovo Admin`, done: 'Saved.' } });
        if (!set.ok) view.notice(`${typed} was added, but not given the console right asked for. Change it on the account's page.`, { kind: 'warn' });
      }
      return true;
    },
  });
  if (made === null) return null;

  await openAddKey(view, {
    name: made,
    own: false,
    title: `Add an SSH key for ${made}`,
    lead: [`${made} is added. If they sign in over SSH, add the key that goes with theirs now; it can be done later on their page.`],
    cancel: 'Not now',
  });
  return made;
}
