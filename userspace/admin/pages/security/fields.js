// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fields of the Security page's dialogs: a text field with its label,
// hint and place for a fault, a switch, a choice of one and a choice of
// names. Read when a dialog that has one is opened.

import {
  clearError, errorSlot, h, showError, uniqueId,
} from '../../dom.js';

/**
 * A text field named `label`; `hint` is the line under it and `mono` draws
 * it in the code face. Gives `{element, input, fault(words), clear()}`:
 * `fault` puts `words` under the field and moves the focus to it.
 */
export function textField({
  label, hint = null, value = '', mono = false, type = 'text',
}) {
  const id = uniqueId('sec-field');
  const slot = errorSlot(`${id}-error`);
  const input = h('input', {
    id, class: mono ? 'input mono' : 'input', type, value, autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': hint ? `${id}-hint` : null,
  });
  input.addEventListener('input', () => clearError(slot, input));
  return {
    input,
    element: h('div', { class: 'field' }, h('label', { for: id }, label), input, hint && h('span', { id: `${id}-hint`, class: 'hint' }, hint), slot),
    fault: (words) => showError(slot, { title: words, detail: '' }, input),
    clear: () => clearError(slot, input),
  };
}

/** A switch in a form: a checkbox that a screen reader says is a switch, with `label` and the `hint` under it. Gives `{element, input}`. */
export function switchField({
  label, hint = null, on = false,
}) {
  const id = uniqueId('sec-switch');
  const input = h('input', { id, type: 'checkbox', role: 'switch', checked: on, 'aria-describedby': hint ? `${id}-hint` : null });
  return {
    input,
    element: h('div', { class: 'field' }, h('label', { class: 'check', for: id }, input, h('b', {}, label)), hint && h('span', { id: `${id}-hint`, class: 'hint' }, hint)),
  };
}

/**
 * A choice of one among `choices` `[{value, label, hint}]`, as radio cards in a group named `label`. `onChange(value)` is called when the choice changes. Gives
 * `{element, value(), set(value)}`.
 */
export function choiceField({
  label, choices, value, onChange = () => {},
}) {
  const name = uniqueId('sec-choice');
  const cards = choices.map((choice) => {
    const input = h('input', { type: 'radio', name, value: choice.value, checked: choice.value === value });
    const card = h('label', { class: `radio-card${choice.value === value ? ' on' : ''}` }, input, h('span', { class: 'sec-choice-words' }, h('b', {}, choice.label), choice.hint && h('small', { class: 'muted' }, choice.hint)));
    return { input, card, value: choice.value };
  });
  const group = h('fieldset', { class: 'sec-choice' }, h('legend', {}, label), ...cards.map((one) => one.card));
  const mark = () => cards.forEach((one) => one.card.classList.toggle('on', one.input.checked));
  group.addEventListener('change', () => {
    mark();
    onChange(cards.find((one) => one.input.checked)?.value);
  });
  return {
    element: group,
    value: () => cards.find((one) => one.input.checked)?.value ?? value,
    set(next) {
      for (const one of cards) one.input.checked = one.value === next;
      mark();
    },
  };
}

/**
 * A choice of names among `names`, each a checkbox drawn as a chip, in a group named `label`. `chosen` are those that start chosen;
 * `onChange(list)` hears each change. Gives `{element, chosen(), set(list)}`.
 */
export function namesField({
  label, names, chosen = [], onChange = () => {},
}) {
  const boxes = names.map((name) => {
    const input = h('input', { type: 'checkbox', value: name, checked: chosen.includes(name) });
    return { input, name, chip: h('label', { class: 'sec-chip' }, input, name) };
  });
  const group = h('fieldset', { class: 'sec-names' }, h('legend', {}, label), h('div', { class: 'sec-chips' }, ...boxes.map((one) => one.chip)));
  const list = () => boxes.filter((one) => one.input.checked).map((one) => one.name);
  group.addEventListener('change', () => onChange(list()));
  return {
    element: group,
    chosen: list,
    set(next) {
      for (const one of boxes) one.input.checked = next.includes(one.name);
    },
  };
}
