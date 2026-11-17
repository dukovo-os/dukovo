// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fields of Add a database and of its dialogs: a text field, a text area
// and a choice from a list, each with its label, its hint and the place under
// it for a fault; and the radio cards of Who connects.

import {
  clearError, errorSlot, h, icon, showError, uniqueId,
} from '../../dom.js';
import {
  ANYWHERE, needsAnywhere, networksProblem, NETWORKS_MAX, parseNetworks,
} from '../../lib/database-model.js';
import { certificateChoices, covers, THIS_MACHINE } from './model.js';

/** Names a field's pieces: its label and hint, and what `fault` and `clear` do to its input. */
function fieldOf(id, { label, hint }, input) {
  const slot = errorSlot(`${id}-error`);
  const described = [hint && `${id}-hint`].filter(Boolean).join(' ');
  if (described) input.setAttribute('aria-describedby', described);
  input.addEventListener('input', () => clearError(slot, input));
  input.addEventListener('change', () => clearError(slot, input));
  const note = h('span', { id: `${id}-hint`, class: 'hint' }, hint ?? '');
  return {
    input,
    note,
    element: h('div', { class: 'field' }, h('label', { for: id }, label), input, hint && note, slot),
    fault: (words) => showError(slot, { title: words, detail: '' }, input),
    clear: () => clearError(slot, input),
  };
}

/** A line of text, in the code face. `value` starts it. */
export function textField({ label, hint = null, value = '' }) {
  const id = uniqueId('db-field');
  const input = h('input', {
    id, class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off',
  });
  input.value = value;
  return fieldOf(id, { label, hint }, input);
}

/** Lines of text, one network a line. */
export function areaField({ label, hint = null, rows = 3 }) {
  const id = uniqueId('db-field');
  const input = h('textarea', {
    id, class: 'input mono db-area', rows: String(rows), autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off',
  });
  return fieldOf(id, { label, hint }, input);
}

/** A choice of one of `options` `{value, label}`; `value` is chosen first. */
export function selectField({
  label, hint = null, options, value = null,
}) {
  const id = uniqueId('db-field');
  const input = h('select', { id, class: 'input' }, options.map((option) => h('option', { value: option.value }, option.label)));
  input.value = value ?? options[0]?.value ?? '';
  return fieldOf(id, { label, hint }, input);
}

/**
 * The radio cards of one choice, `choices` `[{value, title, words, sub}]`
 * (`sub` is a node drawn under the words), in a group named `legend`.
 * `onChange(value)` is called when the choice changes. Gives `{element,
 * value(), set(value)}`.
 */
export function cardsField({
  legend, choices, value, onChange = () => {},
}) {
  const name = uniqueId('db-cards');
  const cards = choices.map((choice) => {
    const input = h('input', { type: 'radio', name, value: choice.value, checked: choice.value === value });
    const words = h('small', { class: 'muted db-words' }, choice.words);
    const label = h('label', { class: 'radio-card' }, input, h('span', {}, h('b', {}, choice.title), words));
    return {
      input, label, words, value: choice.value, sub: choice.sub ?? null, element: choice.sub ? h('div', { class: 'db-card' }, label, choice.sub) : label,
    };
  });
  const mark = () => cards.forEach((one) => {
    one.label.classList.toggle('on', one.input.checked);
    if (one.sub) one.sub.hidden = !one.input.checked;
  });
  mark();
  const element = h('fieldset', { class: 'choices db-cards' }, h('legend', {}, legend), ...cards.map((one) => one.element));
  element.addEventListener('change', () => {
    mark();
    onChange(cards.find((one) => one.input.checked)?.value);
  });
  return {
    element,
    value: () => cards.find((one) => one.input.checked)?.value ?? value,
    set(next) {
      for (const one of cards) one.input.checked = one.value === next;
      mark();
    },
    /** Writes the words of each card again: `words(value)` gives them. */
    describe(words) {
      for (const one of cards) one.words.textContent = words(one.value);
    },
  };
}

/**
 * The fields of the way that lets other machines connect: the networks, the
 * certificate and the word that lets every address in. `certificates` is the
 * answer of `cert list`, `machine` is `machineOf`'s. Gives `{elements,
 * networks, certificate, anywhere, check, update}`: `check()` gives
 * `{found, problems, certificate}`; `update()` draws what depends on what is
 * typed, and is called on every change.
 */
export function accessFields({ certificates, machine }) {
  const choices = certificateChoices(certificates);
  const networks = areaField({ label: 'Addresses', hint: `One network a line, such as 192.168.1.0/24 or 2001:db8::/64: at most ${NETWORKS_MAX}.` });
  const certificate = selectField({
    label: 'Certificate', options: choices.map((one) => ({ value: one.value, label: one.label })), value: THIS_MACHINE,
  });
  const note = h('p', { class: 'small', hidden: true });
  const anywhere = textField({ label: `Type ${ANYWHERE} to let every address in`, hint: 'Every address on the network may then try to sign in. A password and TLS still stand in the way.' });
  const box = h('div', { class: 'banner warn', hidden: true }, icon('warn'), anywhere.element);

  const check = () => {
    const found = parseNetworks(networks.input.value);
    const problems = {};
    const why = networksProblem(found);
    if (why) problems.networks = why;
    else if (needsAnywhere(found, anywhere.input.value)) problems.anywhere = `Type ${ANYWHERE} to let every address in.`;
    return { found, problems, certificate: certificate.input.value };
  };
  const update = () => {
    box.hidden = !parseNetworks(networks.input.value).anywhere;
    const chosen = choices.find((one) => one.value === certificate.input.value);
    const wrong = chosen?.names != null && !covers(chosen.names, machine.host);
    note.hidden = !wrong;
    note.textContent = wrong ? `This certificate does not name ${machine.host}, so programs that check the name will refuse it.` : '';
  };
  for (const field of [networks, certificate, anywhere]) {
    field.input.addEventListener('input', update);
    field.input.addEventListener('change', update);
  }
  return {
    elements: [networks.element, certificate.element, note, box], networks, certificate, anywhere, check, update,
  };
}
