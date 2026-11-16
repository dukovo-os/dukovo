// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// An account's SSH keys (design 7.5 and 7.6): each with its name, type and
// fingerprint (whole on focus), Remove, and Add a key…. The key's line is
// checked as authd reads it (Ed25519 and ECDSA, no options) before the
// action is asked. A key that is the only way an account signs in is
// removed only when the account's name is typed.

import { h, icon } from '../../dom.js';
import { keyWords } from '../../lib/account-model.js';
import { keyAddCall, keyRemoveCall, onlyWayIn } from './model.js';
import { openForm } from './open.js';

/** The fingerprint short, and whole while it has the focus, so that two keys that look alike can be told apart. */
function fingerprint(words) {
  const code = h('code', { class: 'acc-fp', tabindex: '0', 'aria-label': `Fingerprint ${words.full}` }, words.short);
  code.addEventListener('focus', () => code.replaceChildren(words.full));
  code.addEventListener('blur', () => code.replaceChildren(words.short));
  return code;
}

/**
 * Opens the dialog that adds a key to the account `name` (the person's own
 * when `own`). `title`, `lead`, `label` and `cancel` word it, so that Add an
 * account can offer it after the account is made. Resolves with whether a
 * key was added.
 */
export async function openAddKey(view, {
  name, own, title = 'Add an SSH key', lead = null, label = 'Add the key', cancel = 'Cancel',
}) {
  const [{ textField }, { keyLineProblem }] = await Promise.all([import('./fields.js'), import('./typed-model.js')]);
  const field = textField({
    label: 'Public key',
    mono: true,
    placeholder: 'ssh-ed25519 AAAA… laptop',
    hint: 'The line that ssh-keygen wrote in the .pub file. Ed25519 and ECDSA keys are taken.',
  });
  const callOf = () => {
    const typed = field.input.value.trim();
    return typed !== '' && keyLineProblem(typed) === '' ? keyAddCall({ own, name, line: typed }) : null;
  };
  return openForm(view, {
    title,
    icon: 'key',
    lead: lead ?? [own
      ? 'The key goes in your own ~/.ssh/authorized_keys.'
      : `The key goes in ${name}'s ~/.ssh/authorized_keys. ${name} signs in over SSH with it, from the machine that holds the matching private key.`],
    fields: [field.element],
    label,
    cancel,
    command: callOf,
    async submit(button, errors) {
      const problem = keyLineProblem(field.input.value);
      if (problem !== '') {
        field.fault(problem);
        return false;
      }
      const call = callOf();
      const done = await view.api.run(call.action, call.args, {
        button,
        words: { unlock: `add a key to ${own ? 'your account' : name}`, done: 'The key is added.', unchanged: 'The key was there already.' },
        where: { slot: errors, fields: { line: field.input } },
      });
      return done.ok;
    },
  });
}

/** What the dialog of a key that is a way in says, for the account `name`. */
const onlyWayWords = (name, own) => (own
  ? 'This is the only way you sign in: after this you cannot sign in at all.'
  : `This is the only way ${name} signs in: after this it cannot sign in at all.`);

/**
 * The section of keys. `ctx` is `{view, own, name, account, keys,
 * sshPasswords, reload}`: `keys` are `user show`'s, and `sshPasswords` is
 * whether SSH takes passwords (null when it is not known).
 */
export function keysSection(ctx) {
  const {
    view, own, name, account, keys, sshPasswords, reload,
  } = ctx;
  const list = (Array.isArray(keys) ? keys : []).map((key) => ({ key, words: keyWords(key) }));

  async function remove(key, words, button) {
    const call = keyRemoveCall({ own, name, fingerprint: key.fingerprint });
    const lone = onlyWayIn({
      account, keys: list.map((one) => one.key), key, sshPasswords,
    });
    const stay = { button, words: { unlock: `remove the key ${words.name}`, done: 'The key is removed.' } };
    if (!lone) {
      if ((await view.api.run(call.action, call.args, stay)).ok) reload();
      return;
    }
    const removed = await openForm(view, {
      title: `Remove the key ${words.name}?`,
      icon: 'warn',
      lead: [onlyWayWords(name, own), 'The key stays on the machine it is kept on; this only takes it off the account.'],
      label: 'Remove the key',
      danger: true,
      typed: name,
      command: call,
      submit: async (confirm, errors) => (await view.api.run(call.action, call.args, { ...stay, button: confirm, where: { slot: errors } })).ok,
    });
    if (removed) reload();
  }

  const rows = list.map(({ key, words }) => {
    const button = h('button', { class: 'btn small', type: 'button', 'aria-label': `Remove the key ${words.name}` }, 'Remove');
    button.addEventListener('click', () => remove(key, words, button));
    return h('tr', {},
      h('td', {}, h('b', {}, words.name),
        h('small', { class: 'mono' }, `${words.type} · `, fingerprint(words)),
        words.options && h('small', {}, 'This line has options, so the machine does not use it.')),
      h('td', { class: 'end' }, button));
  });

  const add = h('button', { class: 'btn', type: 'button', 'data-action': 'key-add' }, icon('plus'), 'Add a key…');
  add.addEventListener('click', async () => {
    if (await openAddKey(view, { name, own })) reload();
  });

  return h('section', { class: 'acc-section', id: 'keys', 'aria-labelledby': 'acc-keys-h' },
    h('h3', { id: 'acc-keys-h' }, 'SSH keys'),
    rows.length > 0
      ? h('div', { class: 'tbl-box' }, h('table', { class: 'tbl acc-mini' }, h('caption', { class: 'sr-only' }, 'SSH keys'), h('tbody', {}, rows)))
      : h('p', { class: 'muted' }, own ? 'You have no SSH key.' : `${name} has no SSH key.`),
    add);
}
