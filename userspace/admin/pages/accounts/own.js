// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What only the person's own account has (design 7.3 and 12.3): how the console
// looks to them, the time zone it shows times in, and the terminal's settings.
// The display and the time zone are saved to the account, so that they follow
// the person to every browser; each choice applies at once.

import { busy, h } from '../../dom.js';
import { displayOf } from '../../shell/prefs-model.js';
import { displayChoices, zoneField } from '../account/controls.js';

/** A card with a heading, named by it. */
const card = (id, title, ...children) => h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` }, h('h2', { id: `${id}-h` }, title), ...children);

/** The Display card. */
function displayCard(view) {
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
  return card('display', 'Display',
    h('p', { class: 'muted' }, 'Each choice applies at once, here and on every browser you sign in on.'),
    choices, saved);
}

/** The Time zone card. */
function zoneCard(view) {
  const zone = zoneField({ value: view.prefs.zone });
  const status = h('span', { class: 'small', role: 'status' });
  const save = h('button', { class: 'btn', type: 'button' }, 'Save the time zone');
  save.addEventListener('click', async () => {
    status.textContent = '';
    let failed = null;
    await busy([save], async () => {
      try {
        await view.setPrefs({ zone: zone.value() });
        status.textContent = 'Saved.';
      } catch (error) {
        failed = error;
      }
    });
    // After the button is free again, so that a wait can hold it.
    if (failed) view.problem(failed, { input: zone.input, button: save });
  });
  return card('zone', 'Time zone',
    h('p', { class: 'muted' }, `Times in the console are shown in ${view.prefs.zone || view.time.zone}.`),
    zone.field, h('div', { class: 'btns' }, save, status));
}

/** The Terminal settings card, which says so when the account has no terminal. */
const terminalCard = (view) => card('terminal-settings', 'Terminal settings',
  view.session.terminal
    ? [h('p', { class: 'muted' }, 'The size of the text, the lines kept and the keys of the terminal.'),
      h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/terminal/settings' }, 'Terminal settings'))]
    : h('p', { class: 'muted' }, 'This account has no terminal in the console.'));

/** The cards of the person's own account that follow its rows: Display, Time zone, Terminal settings. */
export const ownCards = (view) => [displayCard(view), zoneCard(view), terminalCard(view)];
