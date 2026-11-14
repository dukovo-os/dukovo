// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Terminal settings (design 13.10): text size, lines kept, copy on select,
// keep for N days, read new output aloud and the shortcut keys, and the table
// of the keys. They are the person's preferences (section 5), so they follow
// them to any browser; each change is saved as it is made.

import { clearError, errorSlot, h, showError, uniqueId } from '../../dom.js';
import {
  checkNumber, NUMBERS, SWITCHES, TERMINAL_KEYS,
} from './settings-model.js';

export async function show(view) {
  view.head({ title: 'Terminal settings', crumbs: [{ title: 'Terminal', path: '/terminal' }] });
  const saved = h('span', { class: 'small', role: 'status' });
  const save = async (key, value, input, slot) => {
    saved.textContent = '';
    try {
      await view.setPrefs({ terminal: { ...view.prefs.terminal, [key]: value } });
      saved.textContent = 'Saved.';
    } catch (error) {
      // What the server did not keep goes back in the field.
      const now = view.prefs.terminal[key];
      if (input.type === 'checkbox') input.checked = now === true;
      else input.value = String(now);
      showError(slot, error, input);
    }
  };

  const numbers = NUMBERS.map((spec) => {
    const id = uniqueId(`term-${spec.key}`);
    const slot = errorSlot(`${id}-error`);
    const input = h('input', {
      id, class: 'input', type: 'number', inputmode: 'numeric', min: String(spec.min), max: String(spec.max), step: String(spec.step), value: String(view.prefs.terminal[spec.key]), 'aria-describedby': `${id}-hint`,
    });
    input.addEventListener('change', () => {
      clearError(slot, input);
      const checked = checkNumber(spec.key, input.value);
      if (!checked.ok) showError(slot, { title: checked.words }, input);
      else save(spec.key, checked.value, input, slot);
    });
    return h('div', { class: 'field' }, h('label', { for: id }, `${spec.label} (${spec.unit})`), input, h('span', { id: `${id}-hint`, class: 'hint' }, spec.hint), slot);
  });

  const switches = SWITCHES.map((spec) => {
    const id = uniqueId(`term-${spec.key}`);
    const slot = errorSlot(`${id}-error`);
    const input = h('input', { id, type: 'checkbox', checked: view.prefs.terminal[spec.key] === true, 'aria-describedby': `${id}-hint` });
    input.addEventListener('change', () => {
      clearError(slot, input);
      save(spec.key, input.checked, input, slot);
    });
    return h('div', { class: 'field' }, h('div', { class: 'check' }, input, h('div', {}, h('label', { for: id }, spec.label), h('div', { id: `${id}-hint`, class: 'hint' }, spec.hint))), slot);
  });

  view.main.append(
    h('section', { class: 'card', 'aria-labelledby': 'term-h' },
      h('h2', { id: 'term-h' }, 'Terminal'),
      h('p', { class: 'muted' }, 'These follow you to every browser you sign in on. Each choice is saved as you make it.'),
      ...numbers, ...switches, saved),
    h('section', { class: 'card', id: 'keys', 'aria-labelledby': 'keys-h' },
      h('h2', { id: 'keys-h' }, 'Terminal keys'),
      h('p', { class: 'muted' }, 'Every other key goes to the program. The browser keeps Ctrl+W, Ctrl+T, Ctrl+N and Ctrl+Tab for itself, so a page never sees them: on the key bar, press Ctrl, then W, to send Ctrl+W to the program.'),
      h('div', { class: 'tbl-box' }, h('table', { class: 'tbl' },
        h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, 'Key'), h('th', { scope: 'col' }, 'What it does'))),
        h('tbody', {}, ...TERMINAL_KEYS.map(([keys, words]) => h('tr', {}, h('td', {}, h('span', { class: 'kbd' }, keys)), h('td', {}, words))))))));
}
