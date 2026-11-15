// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The question that renames a terminal: one field, and admind's own words
// under it when the name is refused. The Terminal page and the Sessions page
// both ask it.

import {
  busy, clearError, errorSlot, focusBack, h, showError, uniqueId,
} from '../../dom.js';

/**
 * Asks for a new name for a terminal called `current`. `save(name)` makes
 * the change and throws what admind answers. Resolves with the name that was
 * saved, or null when the person cancels.
 */
export function renameDialog(current, save) {
  return new Promise((resolve) => {
    const id = uniqueId('rename');
    const input = h('input', {
      class: 'input', id: `${id}-name`, value: current, maxlength: '40', required: true, autocomplete: 'off',
    });
    const slot = errorSlot(`${id}-error`);
    const ok = h('button', { class: 'btn default', type: 'submit' }, 'Rename');
    const form = h('form', {
      async onsubmit(event) {
        event.preventDefault();
        clearError(slot, input);
        try {
          await busy([ok], () => save(input.value.trim()));
          dialog.close('ok');
        } catch (error) {
          showError(slot, error, input);
        }
      },
    },
    h('div', { class: 'dialog-head' }, h('h2', { id }, 'Rename the terminal')),
    h('div', { class: 'dialog-body' },
      h('div', { class: 'field' }, h('label', { for: input.id }, 'Name'), input,
        h('span', { class: 'hint' }, 'Up to 40 characters.')), slot),
    h('div', { class: 'dialog-foot' }, ok, h('button', { class: 'btn', type: 'button', onclick: () => dialog.close() }, 'Cancel')));
    const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': id }, form);
    const opener = document.activeElement;
    dialog.addEventListener('close', () => {
      dialog.remove();
      focusBack(opener);
      resolve(dialog.returnValue === 'ok' ? input.value.trim() : null);
    });
    document.body.append(dialog);
    dialog.showModal();
    input.select();
  });
}
