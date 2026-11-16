// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The three answers written again in place of the text, with Done and Cancel.
// Read when Edit your answers is pressed.

import {
  clearError, errorSlot, h, showError, uniqueId,
} from '../../dom.js';

const QUESTIONS = [
  ['happened', 'What happened?'],
  ['expected', 'What did you expect?'],
  ['again', 'How can it be made to happen again?'],
];

/** The form: `onDone(answers)` gives true when the text was made, which ends the edit; `onCancel()` ends it. */
export function makeAnswers({ onDone, onCancel }) {
  const id = uniqueId('report-answers');
  const fields = QUESTIONS.map(([name, label]) => {
    const input = h('textarea', {
      id: `${id}-${name}`, class: 'input', rows: '3', maxlength: '2000', name,
    });
    const slot = errorSlot(`${id}-${name}-error`);
    input.addEventListener('input', () => clearError(slot, input));
    return { name, input, slot, node: h('div', { class: 'field' }, h('label', { for: input.id }, label), input, slot) };
  });
  const done = h('button', { class: 'btn default', type: 'submit' }, 'Done');
  const cancel = h('button', { class: 'btn', type: 'button', onclick: onCancel }, 'Cancel');
  const element = h('form', { class: 'rp-editor', hidden: true, novalidate: true }, ...fields.map((field) => field.node), h('div', { class: 'btns' }, done, cancel));
  let needsFirst = false;

  element.addEventListener('submit', async (event) => {
    event.preventDefault();
    const answers = Object.fromEntries(fields.map((field) => [field.name, field.input.value.trim()]));
    if (needsFirst && answers.happened === '') {
      showError(fields[0].slot, { title: 'Say what happened', detail: 'The report needs the first answer.' }, fields[0].input);
      return;
    }
    done.disabled = true;
    try {
      await onDone(answers);
    } finally {
      done.disabled = false;
    }
  });

  return {
    element,

    /** Shows the form with `values`, the three answers as they are now; `first` says that the first is needed. */
    open(values, first) {
      needsFirst = first;
      for (const field of fields) {
        field.input.value = values[field.name];
        clearError(field.slot, field.input);
      }
      element.hidden = false;
      fields[0].input.focus();
    },
  };
}
