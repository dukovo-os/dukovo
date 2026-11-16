// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fields of the Accounts pages' dialogs: a text field with its label, hint
// and place for a fault, and a switch in a form. Read when a dialog that has
// one is opened.

import {
  clearError, errorSlot, h, showError, uniqueId,
} from '../../dom.js';

/**
 * A text field named `label`. `mono` draws it in the code face, `hint` is
 * the line under it. Gives `{element, input, fault(words), clear()}`:
 * `fault` puts `words` under the field and moves the focus to it.
 */
export function textField({
  label, hint = null, value = '', mono = false, placeholder = null,
}) {
  const id = uniqueId('acc-field');
  const slot = errorSlot(`${id}-error`);
  const input = h('input', {
    id,
    class: mono ? 'input mono' : 'input',
    type: 'text',
    value,
    placeholder,
    autocomplete: 'off',
    spellcheck: 'false',
    autocapitalize: 'off',
    'aria-describedby': hint ? `${id}-hint` : null,
  });
  input.addEventListener('input', () => clearError(slot, input));
  // The hint goes on describing the field when a fault takes the place of it.
  const element = h('div', { class: 'field' },
    h('label', { for: id }, label), input,
    hint && h('span', { id: `${id}-hint`, class: 'hint' }, hint),
    slot);
  return {
    element,
    input,
    fault: (words) => showError(slot, { title: words, detail: '' }, input),
    clear: () => clearError(slot, input),
  };
}

/**
 * A switch in a form: a checkbox that a screen reader says is a switch,
 * with `label` and the `hint` that says what it does, which also says why a
 * switch that is `fixed` cannot be changed. Gives `{element, input}`.
 */
export function formSwitch({
  label, hint, on = false, fixed = false,
}) {
  const id = uniqueId('acc-switch');
  const input = h('input', {
    id, type: 'checkbox', role: 'switch', checked: on, disabled: fixed, 'aria-describedby': `${id}-hint`,
  });
  return {
    input,
    element: h('div', { class: 'field acc-switch-field' },
      h('label', { class: 'acc-switch-label', for: id }, input, h('b', {}, label)),
      h('span', { id: `${id}-hint`, class: 'hint' }, hint)),
  };
}
