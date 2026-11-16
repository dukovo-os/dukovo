// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Disable, enable and remove an account (design 7.5 and 7.6). Disabling ends
// everything the account runs; removing asks what becomes of its home
// folder and for the name typed. Neither is offered for the account in use:
// the console refuses it (ADM-136), and the page says where it is done
// instead. authd refuses either when it would leave no administrator
// (ACC-102), and its words are shown as they come.

import { h, uniqueId } from '../../dom.js';
import { isDisabled } from '../../lib/account-model.js';
import {
  disableCall, enableCall, removeCall, SELF,
} from './model.js';
import { openForm } from './open.js';

/** What disabling does, for the account `name`. */
const disableWords = (name) => [
  `Everything ${name} runs ends now: their sessions, terminals, SSH connections and processes. Nothing signs in to the account until it is enabled again, with the same password.`,
];

/** The radio cards that ask what becomes of the home folder. Gives `{element, value}`. */
function homeChoice(name, HOME_CHOICES) {
  const group = uniqueId('acc-home');
  let chosen = HOME_CHOICES[0].value;
  const cards = HOME_CHOICES.map((choice) => {
    const input = h('input', { type: 'radio', name: group, value: choice.value, checked: choice.value === chosen });
    const card = h('label', { class: `radio-card${choice.value === chosen ? ' on' : ''}` }, input,
      h('span', {}, h('b', {}, choice.label(name)), h('span', { class: 'small muted' }, choice.words)));
    input.addEventListener('change', () => {
      chosen = choice.value;
      HOME_CHOICES.forEach((other, at) => cards[at].classList.toggle('on', other.value === chosen));
    });
    return card;
  });
  return {
    element: h('fieldset', { class: 'choices' }, h('legend', {}, 'What becomes of the home folder'), cards),
    value: () => chosen,
  };
}

/**
 * The section that disables and removes. `ctx` is `{view, own, name,
 * account, reload}`; for the person's own account it says where this is done.
 */
export function dangerSection(ctx) {
  const {
    view, own, name, account, reload,
  } = ctx;
  const head = h('h3', { id: 'acc-danger-h' }, isDisabled(account) ? 'Enable or remove' : 'Disable or remove');
  if (own) {
    return h('section', { class: 'acc-section', id: 'danger', 'aria-labelledby': 'acc-danger-h' }, head,
      h('p', { class: 'muted' }, `The console does not disable or remove the account you are signed in with. ${SELF.words} `, h('code', {}, SELF.command), '.'));
  }

  const disabled = isDisabled(account);
  const toggle = h('button', { class: 'btn', type: 'button', 'data-action': disabled ? 'enable' : 'disable' }, disabled ? 'Enable' : 'Disable the account…');
  toggle.addEventListener('click', async () => {
    if (disabled) {
      const call = enableCall({ name });
      const done = await view.api.run(call.action, call.args, { button: toggle, words: { unlock: `enable ${name}`, done: `${name} can sign in again.` } });
      if (done.ok) reload();
      return;
    }
    const call = disableCall({ name });
    const done = await openForm(view, {
      title: `Disable ${name}?`,
      icon: 'warn',
      lead: disableWords(name),
      label: 'Disable the account',
      danger: true,
      command: call,
      submit: async (confirm, errors) => (await view.api.run(call.action, call.args, {
        button: confirm, words: { unlock: `disable ${name}`, done: `${name} is disabled.` }, where: { slot: errors },
      })).ok,
    });
    if (done) reload();
  });

  const remove = h('button', { class: 'btn danger', type: 'button', 'data-action': 'remove' }, 'Remove the account…');
  remove.addEventListener('click', async () => {
    const { HOME_CHOICES } = await import('./typed-model.js');
    const home = homeChoice(name, HOME_CHOICES);
    const done = await openForm(view, {
      title: `Remove ${name}?`,
      icon: 'warn',
      lead: [`${name} cannot sign in any more, and everything they run ends now. Choose what becomes of their files.`],
      fields: [home.element],
      label: 'Remove the account',
      danger: true,
      typed: name,
      command: () => removeCall({ name, home: home.value() }),
      async submit(confirm, errors) {
        const call = removeCall({ name, home: home.value() });
        return (await view.api.run(call.action, call.args, {
          button: confirm, words: { unlock: `remove ${name}`, done: `${name} is removed.` }, where: { slot: errors },
        })).ok;
      },
    });
    if (done) view.go('/accounts');
  });

  return h('section', { class: 'acc-section', id: 'danger', 'aria-labelledby': 'acc-danger-h' }, head,
    disabled && h('p', { class: 'muted' }, `${name} is disabled: nothing signs in to the account until it is enabled again.`),
    h('div', { class: 'btns' }, toggle, remove));
}
