// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// An account's second step (design 7.5): what it has, as words, and for
// another account Remove second steps…, which an administrator uses when the
// person has lost the phone or key. The person's own are managed on C2a's
// pages, which this row links to.

import { h } from '../../dom.js';
import { stepsWords } from '../../lib/account-model.js';
import { stepsResetCall, unlockAsks } from './model.js';
import { openForm } from './open.js';

/** What Remove second steps… says it does, for the account `name`. */
const resetWords = (name) => [
  `Use this when ${name} has lost the phone or key. Their app, passkeys, recovery codes and trusted browsers are removed. They sign in with their password alone; if the sign-in rules ask them for a second step, they add a new one before anything else.`,
  `${name} is told at the next sign-in.`,
];

/**
 * The rows "Second step", and for an administrator's own account "Unlock
 * asks", as `[dt, dd, …]` for the account's list. `ctx` is `{view, own, name,
 * admin, steps, reload}`; `admin` is whether the account is an administrator's.
 */
export function stepsRows(ctx) {
  const {
    view, own, name, admin, steps, reload,
  } = ctx;
  const words = stepsWords(steps);
  const has = words.summary !== 'None';

  const action = (() => {
    if (own) return h('a', { class: 'btn small', href: '/second-step' }, 'Manage the second step');
    if (!has) return null;
    const button = h('button', { class: 'btn small', type: 'button', 'data-action': 'steps-reset' }, 'Remove second steps…');
    button.addEventListener('click', async () => {
      const call = stepsResetCall({ name });
      const done = await openForm(view, {
        title: `Remove ${name}'s second steps?`,
        icon: 'warn',
        lead: resetWords(name),
        label: 'Remove second steps',
        danger: true,
        command: call,
        submit: async (confirm, errors) => (await view.api.run(call.action, call.args, {
          button: confirm, words: { unlock: `remove ${name}'s second steps`, done: 'The second steps are removed.' }, where: { slot: errors },
        })).ok,
      });
      if (done) reload();
    });
    return button;
  })();

  return [
    h('dt', { id: 'second-step' }, 'Second step'),
    h('dd', {},
      words.summary,
      words.extras.length > 0 && h('div', { class: 'small muted' }, words.extras.join(' · ')),
      action && h('div', { class: 'btns' }, action)),
    ...(own && admin
      ? [h('dt', {}, 'Unlock asks'), h('dd', {}, unlockAsks(steps), h('div', { class: 'small muted' }, 'What the console asks for when you unlock it to make changes.'))]
      : []),
  ];
}
