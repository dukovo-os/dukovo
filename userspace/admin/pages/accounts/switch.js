// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A switch that is a row's own control (design 12): a button with
// `role="switch"`, named by what it turns on, which says why it cannot be
// changed when it cannot.

import { h, uniqueId } from '../../dom.js';

/**
 * A switch that is a row's control: a button named `label`, `on` or not,
 * which says why it cannot be changed (`why`, text) when it is `fixed`.
 * `toggle(button)` is called when it is pressed, unless it is `fixed`; the
 * page draws it again when the answer to what it did comes.
 */
export function rowSwitch({
  label, on, fixed = false, why = null, action, toggle = null,
}) {
  const id = uniqueId('acc-row-switch');
  const button = h('button', {
    class: 'acc-switch',
    type: 'button',
    role: 'switch',
    'aria-checked': String(on),
    'aria-disabled': fixed ? 'true' : null,
    'aria-describedby': why ? id : null,
    'data-action': action,
  }, h('span', { class: `sw${on ? ' on' : ''}`, 'aria-hidden': 'true' }), label);
  if (!fixed && toggle) button.addEventListener('click', () => toggle(button));
  return h('div', { class: 'acc-switch-row' }, button, why && h('span', { id, class: 'hint' }, why));
}
