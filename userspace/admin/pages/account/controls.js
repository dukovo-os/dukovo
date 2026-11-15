// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The display and time-zone controls that /account and /welcome share
// (design 12.1 and 12.3): the same words, the same radio groups, the same
// zone field, so that a person learns them once.

import { h, uniqueId } from '../../dom.js';
import { isZone, zones } from '../../lib/time.js';
import { MOTION_CHOICES, TEXT_CHOICES, THEME_CHOICES } from '../../shell/prefs-model.js';

/** One group of radio cards: a fieldset named `legend` with a card for each choice. */
function group(name, legend, choices, current, onPick, hint) {
  const cards = choices.map((choice) => {
    const input = h('input', { type: 'radio', name, value: String(choice.value), checked: String(choice.value) === String(current) });
    const card = h('label', { class: 'radio-card' }, input, h('span', {}, choice.label));
    card.classList.toggle('on', input.checked);
    input.addEventListener('change', () => {
      for (const other of cards) other.classList.toggle('on', other.querySelector('input').checked);
      onPick(choice.value);
    });
    return card;
  });
  return h('fieldset', { class: 'choices' }, h('legend', {}, legend), hint && h('p', { class: 'hint' }, hint), h('div', { class: 'cards-row' }, ...cards));
}

/**
 * The three groups of the display: colours, text size and motion. `current`
 * is `{theme, text, motion}`; `onChange` is called with the key that
 * changed, `{theme: 'light'}`, as each choice is made.
 */
export function displayChoices({ current, onChange }) {
  return h('div', { class: 'choice-groups' },
    group('theme', 'Colours', THEME_CHOICES, current.theme, (theme) => onChange({ theme })),
    group('text', 'Text size', TEXT_CHOICES, current.text, (text) => onChange({ text: Number(text) }),
      'The size of the text on every page. Your browser\'s own text size works on top of this.'),
    group('motion', 'Motion', MOTION_CHOICES, current.motion, (motion) => onChange({ motion }),
      'Less takes away fades, stripes on progress bars and smooth scrolling.'));
}

/**
 * The time-zone field: a text field with the browser's zones to pick from.
 * Gives `{field, input, value, valid}`.
 */
export function zoneField({ value }) {
  const id = uniqueId('zone');
  const list = h('datalist', { id: `${id}-list` }, ...zones().map((zone) => h('option', { value: zone })));
  const input = h('input', {
    id, class: 'input', type: 'text', value, list: `${id}-list`, autocomplete: 'off', spellcheck: 'false', 'aria-describedby': `${id}-hint`,
  });
  const field = h('div', { class: 'field' },
    h('label', { for: id }, 'Time zone'),
    input,
    list,
    h('span', { id: `${id}-hint`, class: 'hint' }, 'Type a place, such as Europe/Prague or America/Denver, and pick from the list.'));
  return { field, input, value: () => input.value.trim(), valid: () => isZone(input.value.trim()) };
}
