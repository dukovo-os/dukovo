// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Your account" (design 12.3): the display, the time zone, the second
// step, API tokens and the terminal's settings, until Accounts has them.

import { busy, h, icon } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { displayOf } from '../../shell/prefs-model.js';
import { displayChoices, zoneField } from './controls.js';

const TOKEN_COMMAND = 'admin token add NAME --scope services:read --days 30';

function card(id, title, ...children) {
  return h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` }, h('h2', { id: `${id}-h` }, title), ...children);
}

export async function show(view) {
  const { session } = view;
  view.head({ title: 'Your account' });

  const saved = h('span', { class: 'small', role: 'status' });
  const choices = displayChoices({
    current: displayOf(view.prefs),
    onChange: (change) => {
      saved.textContent = '';
      view.setPrefs(change).then(() => { saved.textContent = 'Saved.'; }, (error) => {
        // A choice the server did not keep is undone, and so is its card.
        const now = displayOf(view.prefs);
        for (const input of choices.querySelectorAll('input[type="radio"]')) {
          input.checked = String(now[input.name]) === input.value;
          input.closest('label').classList.toggle('on', input.checked);
        }
        view.problem(error, {});
      });
    },
  });

  const zone = zoneField({ value: view.prefs.zone });
  const zoneStatus = h('span', { class: 'small', role: 'status' });
  const save = h('button', { class: 'btn', type: 'button' }, 'Save the time zone');
  save.addEventListener('click', async () => {
    zoneStatus.textContent = '';
    let failed = null;
    await busy([save], async () => {
      try {
        await view.setPrefs({ zone: zone.value() });
        zoneStatus.textContent = 'Saved.';
      } catch (error) {
        failed = error;
      }
    });
    // After the button is free again, so that a wait can hold it.
    if (failed) view.problem(failed, { input: zone.input, button: save });
  });

  const copyStatus = h('span', { class: 'small', role: 'status' });
  const command = h('pre', { class: 'cmdtext', tabindex: '0' }, TOKEN_COMMAND);
  const copy = h('button', { class: 'btn small', type: 'button' }, icon('copy'), 'Copy');
  copy.addEventListener('click', async () => {
    copyStatus.textContent = (await copyText(TOKEN_COMMAND, command)) ? COPIED : PRESS_CTRL_C;
  });

  view.main.append(
    h('p', {},
      'Signed in as ', h('b', {}, session.name), session.full_name && ` (${session.full_name})`, '. ',
      session.admin
        ? 'This account is an administrator: unlock the console from the header to make changes.'
        : 'This account may look and change its own things, not the server\'s: it is not an administrator.'),
    card('display', 'Display',
      h('p', { class: 'muted' }, 'Each choice applies at once, here and on every browser you sign in on.'),
      choices, saved),
    card('zone', 'Time zone',
      h('p', { class: 'muted' }, `Times in the console are shown in ${view.prefs.zone || view.time.zone}.`),
      zone.field, h('div', { class: 'btns' }, save, zoneStatus)),
    card('second-step', 'Second step',
      h('p', { class: 'muted' }, 'An authenticator app, passkeys and recovery codes for signing in.'),
      h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/second-step' }, 'Manage the second step'))),
    card('tokens', 'API tokens',
      h('p', { class: 'muted' },
        'A script comes in with an API token, made at a terminal as you. The token is shown once; the scopes say what it may do.'),
      h('div', { class: 'cmd' },
        h('div', { class: 'cmd-head' }, icon('code'), 'At a terminal', h('div', { class: 'btns' }, copy)),
        command, copyStatus)),
    card('terminal-settings', 'Terminal settings',
      session.terminal
        ? [h('p', { class: 'muted' }, 'The size of the text, the lines kept and the keys of the terminal.'),
          h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/terminal/settings' }, 'Terminal settings'))]
        : h('p', { class: 'muted' }, 'This account has no terminal in the console.')));
}
