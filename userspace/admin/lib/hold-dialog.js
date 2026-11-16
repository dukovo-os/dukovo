// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The small question a hold asks: why, which is kept with the hold and read by
// everyone who can sign in. Holding the system, the firmware set and an app
// ask it in this dialog. Read by the page that holds, when a hold is asked for.

import {
  clearError, errorSlot, h, showError, uniqueId,
} from '../dom.js';
import { openFormDialog } from './form-dialog.js';
import { WHY_MOST, whyNote, whyOf } from './hold-model.js';

/**
 * Opens the question and resolves, when it closes, with whether the hold was
 * made. `title` heads it and `lead` says what holding does; `place` is the
 * command that shows the reason beside the hold. `call(why)` gives the
 * `{action, args}` that holds, with the reason or null for none, and `words`
 * are what the action says on the way.
 */
export function openHold(view, {
  title, lead, place, call, words,
}) {
  const id = uniqueId('hold-why');
  const input = h('input', {
    id, class: 'input', type: 'text', maxlength: String(WHY_MOST), autocomplete: 'off', 'aria-describedby': `${id}-note`,
  });
  const problem = errorSlot(`${id}-error`);
  input.addEventListener('input', () => {
    clearError(problem, input);
    input.setAttribute('aria-describedby', `${id}-note`);
  });
  const field = h('div', { class: 'field' },
    h('label', { for: id }, 'Why (optional)'), input, h('span', { id: `${id}-note`, class: 'hint' }, whyNote(place)), problem);
  return openFormDialog(view, {
    title,
    lead,
    fields: [field],
    label: 'Hold it',
    command: () => (whyOf(input.value).problem ? null : call(whyOf(input.value).why)),
    async submit(button, slot) {
      const { why, problem: wrong } = whyOf(input.value);
      if (wrong) {
        showError(problem, { title: wrong }, input);
        return false;
      }
      const made = call(why);
      return (await view.api.run(made.action, made.args, { button, where: { slot }, words })).ok;
    },
  });
}
