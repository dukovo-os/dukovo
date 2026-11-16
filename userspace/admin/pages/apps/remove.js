// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Remove… on an app's page: what the app is removed for, what uses it, and
// the one question that matters, what becomes of its data. Keeping it is the
// answer that is chosen; removing it with the app asks for the app's name to
// be typed first. Read when Remove… is pressed.

import {
  clearError, errorSlot, h, icon, showError, uniqueId,
} from '../../dom.js';
import {
  REMOVE_CHOICES, removeLead, usedByOf, usersWords,
} from './app-model.js';
import { openQuestion } from './dialog.js';
import { removeCall, runWords } from './run-model.js';

/**
 * Opens the dialog for the app of `info` (`bundle info --json`).
 * `started(id, action, args)` is told of the task that removing begins.
 * Resolves with whether the removal was begun.
 */
export function openRemove(view, { info, started }) {
  const usedBy = usedByOf(info);
  const group = uniqueId('apps-remove');
  const name = info.name;
  let data = 'keep';

  const cards = REMOVE_CHOICES.map((choice) => {
    const input = h('input', { type: 'radio', name: group, value: choice.value, checked: choice.value === data });
    const label = h('label', { class: `radio-card${choice.value === data ? ' on' : ''}` }, input,
      h('span', {}, h('b', {}, choice.label), h('small', { class: 'hint' }, choice.words)));
    return { input, label };
  });

  const typed = h('input', {
    id: `${group}-typed`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off',
  });
  const typedError = errorSlot(`${group}-typed-error`);
  typed.addEventListener('input', () => clearError(typedError, typed));
  const typedField = h('div', { class: 'field', hidden: true },
    h('label', { for: typed.id }, `Type ${name} to go on`), typed, typedError);

  for (const card of cards) {
    card.input.addEventListener('change', () => {
      data = card.input.getAttribute('value');
      for (const other of cards) other.label.classList.toggle('on', other === card);
      typedField.hidden = data !== 'purge';
      clearError(typedError, typed);
      if (data === 'purge') typed.focus();
    });
  }
  const choices = h('fieldset', { class: 'choices' }, h('legend', {}, 'What becomes of its data'), ...cards.map((card) => card.label), typedField);

  const users = usedBy !== null && usedBy.length > 0
    ? h('div', { class: 'banner warn', role: 'note' }, icon('warn'), h('div', {}, h('ul', { class: 'apps-users' }, usersWords(usedBy).map((words) => h('li', {}, words)))))
    : null;

  return openQuestion(view, {
    title: `Remove ${name}?`,
    icon: 'warn',
    lead: [removeLead(info), users].filter(Boolean),
    fields: [choices],
    label: 'Remove the app',
    danger: true,
    command: () => removeCall(info.id, data),
    async submit(confirm, errors) {
      if (data === 'purge' && typed.value.trim() !== name) {
        showError(typedError, { title: `Type ${name} exactly as it is written`, detail: 'The name is typed so that its data is not removed by a slip.' }, typed);
        return false;
      }
      const call = removeCall(info.id, data);
      const result = await view.api.run(call.action, call.args, {
        button: confirm, words: runWords('remove', name), where: { slot: errors },
      });
      if (typeof result.task === 'number') started(result.task, call.action, call.args);
      return result.ok;
    },
  });
}
