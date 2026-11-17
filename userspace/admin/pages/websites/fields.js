// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fields of Add a website and of the dialogs of a site's page: a line of
// text, lines of text, a choice from a list and a box that is ticked, each
// with its label, its hint and the place under it for a fault; and the
// radio cards of a choice with words under each.

import {
  clearError, errorSlot, h, showError, uniqueId,
} from '../../dom.js';

/** The pieces every field has: its label and hint, and what `fault` and `clear` do to its input. */
function fieldOf(id, { label, hint }, input) {
  const slot = errorSlot(`${id}-error`);
  const note = h('span', { id: `${id}-hint`, class: 'hint' }, hint ?? '');
  if (hint) input.setAttribute('aria-describedby', note.id);
  input.addEventListener('input', () => clearError(slot, input));
  input.addEventListener('change', () => clearError(slot, input));
  return {
    input,
    note,
    element: h('div', { class: 'field' }, h('label', { for: id }, label), input, hint && note, slot),
    fault: (words) => showError(slot, { title: words, detail: '' }, input),
    clear: () => clearError(slot, input),
  };
}

/** A line of text in the code face; `value` starts it. */
export function textField({
  label, hint = null, value = '', placeholder = null,
}) {
  const id = uniqueId('web-field');
  const input = h('input', {
    id, class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', placeholder,
  });
  input.value = value;
  return fieldOf(id, { label, hint }, input);
}

/** Lines of text, one name a line. */
export function areaField({ label, hint = null, rows = 3 }) {
  const id = uniqueId('web-field');
  const input = h('textarea', {
    id, class: 'input mono web-area', rows: String(rows), autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off',
  });
  return fieldOf(id, { label, hint }, input);
}

/** A choice of one of `options` `{value, label}`; `value` is chosen first. */
export function selectField({
  label, hint = null, options, value = null,
}) {
  const id = uniqueId('web-field');
  const input = h('select', { id, class: 'input' }, options.map((option) => h('option', { value: option.value }, option.label)));
  input.value = value ?? options[0]?.value ?? '';
  return fieldOf(id, { label, hint }, input);
}

/** A box that is ticked or not, with `label` beside it and `hint` under. */
export function checkField({ label, hint = null, checked = false }) {
  const id = uniqueId('web-field');
  const input = h('input', { id, type: 'checkbox', checked });
  const slot = errorSlot(`${id}-error`);
  const note = h('span', { id: `${id}-hint`, class: 'hint' }, hint ?? '');
  if (hint) input.setAttribute('aria-describedby', note.id);
  input.addEventListener('change', () => clearError(slot, input));
  return {
    input,
    note,
    element: h('div', { class: 'field' }, h('label', { class: 'check', for: id }, input, h('span', {}, label)), hint && note, slot),
    fault: (words) => showError(slot, { title: words, detail: '' }, input),
    clear: () => clearError(slot, input),
  };
}

/**
 * The radio cards of one choice, `choices` `[{value, title, words, sub, off,
 * why, warn}]` (`sub` is a node drawn under the card while it is chosen, `off`
 * a card that cannot be chosen, `why` what says so, `warn` a card in the
 * warning colour), in a group named `legend`. `onChange(value)` is called
 * when the choice changes. Gives `{element, value(), set(value)}`.
 */
export function cardsField({
  legend, choices, value, onChange = () => {},
}) {
  const name = uniqueId('web-cards');
  const cards = choices.map((choice) => {
    const input = h('input', { type: 'radio', name, value: choice.value, checked: choice.value === value, disabled: choice.off === true });
    const label = h('label', { class: `radio-card${choice.warn ? ' web-warn' : ''}` }, input,
      h('span', {}, h('b', {}, choice.title), h('small', { class: 'muted web-words' }, choice.words), choice.off && choice.why && h('small', { class: 'web-why' }, choice.why)));
    return {
      input, label, sub: choice.sub ?? null, element: choice.sub ? h('div', { class: 'web-card' }, label, choice.sub) : label,
    };
  });
  const mark = () => cards.forEach((one) => {
    one.label.classList.toggle('on', one.input.checked);
    if (one.sub) one.sub.hidden = !one.input.checked;
  });
  mark();
  const element = h('fieldset', { class: 'choices web-cards' }, h('legend', {}, legend), ...cards.map((one) => one.element));
  element.addEventListener('change', () => {
    mark();
    onChange(cards.find((one) => one.input.checked)?.input.getAttribute('value'));
  });
  const current = () => cards.find((one) => one.input.checked)?.input.getAttribute('value') ?? value;
  return {
    element,
    value: current,
    set(next) {
      for (const one of cards) one.input.checked = one.input.getAttribute('value') === next;
      mark();
    },
  };
}
