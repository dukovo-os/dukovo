// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog that every question of the Accounts pages is asked in (design 7.5,
// 7.6 and 12): a modal named by its heading, with what the change does said in
// its description, the fields it needs, and the button that goes on last and
// never focused first. A change that ends someone's work or removes
// something is a question that may ask for the name typed. The pages read
// this file with the dialog they open.

import {
  clearError, errorSlot, fill, h, showError, uniqueId,
} from '../../dom.js';
import { head, openModal } from '../../shell/dialogs.js';

/** How long the command under the fields waits after the last key. */
const TYPING_MS = 400;

/**
 * Opens a dialog and resolves, when it closes, with whether a change was
 * made. `title` and `icon` head it; `lead` is what the change does, in
 * paragraphs of text or nodes, held by the dialog's description; `fields`
 * are nodes under them; `label` is the words of the button that goes on
 * (`danger` draws it as one) and `cancel` the other's; `typed` is a name the
 * person has to type first, or null; `command` is `{action, args}`, or a
 * function that gives it from what the fields hold now (null while they do
 * not hold enough), for the Show command panel, which comes only while pages
 * show commands and follows the typing; `wide` gives a dialog of the full
 * width. `submit(button, slot)` makes the change:
 * `button` is the one that goes busy, `slot` is where its problems go, and
 * it gives true when the change was made, which closes the dialog, and false
 * when it was not, which leaves the dialog open for the person to put right.
 */
export function openForm(view, {
  title, icon = 'info', lead = [], fields = [], label, danger = false, cancel = 'Cancel', typed = null, command = null, wide = false, submit,
}) {
  const id = uniqueId('acc-dialog');
  const confirm = h('button', { class: danger ? 'btn danger' : 'btn default', type: 'submit' }, label);
  const close = h('button', { class: 'btn', type: 'button', autofocus: fields.length === 0 && typed === null }, cancel);
  const errors = h('div', { class: 'acc-errors' });

  const answer = typed === null ? null : h('input', {
    id: `${id}-typed`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off',
  });
  const answerSlot = errorSlot(`${id}-typed-error`);
  answer?.addEventListener('input', () => clearError(answerSlot, answer));
  const typedField = answer && h('div', { class: 'field' }, h('label', { for: answer.id }, `Type ${typed} to go on`), answer, answerSlot);

  const shown = command !== null && view.prefs.commands === 'shown' ? h('div', { class: 'acc-command' }) : null;
  /** Draws the command that the fields hold now, or nothing while they hold too little. */
  const drawCommand = () => {
    const call = typeof command === 'function' ? command() : command;
    fill(shown, call && [h('h3', {}, 'This runs'), view.api.panel(call.action, call.args)]);
  };

  const dialog = h('dialog', {
    class: `dialog${wide ? '' : ' medium'}`, 'aria-labelledby': `${id}-h`, 'aria-describedby': `${id}-lead`,
  },
  h('form', { method: 'dialog', novalidate: true },
    head(`${id}-h`, icon, title),
    h('div', { class: 'dialog-body' },
      h('div', { id: `${id}-lead`, class: 'acc-lead' }, ...[].concat(lead).map((words) => (typeof words === 'string' ? h('p', {}, words) : words))),
      ...fields,
      typedField,
      errors,
      shown),
    h('div', { class: 'dialog-foot' }, close, confirm)));

  let working = false;
  let waiting = null;
  if (shown) {
    drawCommand();
    // One call for a burst of typing, so that the server is asked once the person pauses.
    dialog.querySelector('form').addEventListener('input', () => {
      clearTimeout(waiting);
      waiting = setTimeout(drawCommand, TYPING_MS);
    });
    view.signal.addEventListener('abort', () => clearTimeout(waiting), { once: true });
  }
  close.addEventListener('click', () => dialog.close('close'));
  dialog.addEventListener('cancel', (event) => {
    if (working) event.preventDefault();
  });
  dialog.querySelector('form').addEventListener('submit', async (event) => {
    event.preventDefault();
    fill(errors);
    if (answer && answer.value.trim() !== typed) {
      showError(answerSlot, { title: `Type ${typed} exactly as it is written`, detail: 'The name is typed so that this is not done by a slip.' }, answer);
      return;
    }
    working = true;
    let done = false;
    try {
      done = await submit(confirm, errors);
    } finally {
      working = false;
    }
    if (done && !view.signal.aborted) dialog.close('done');
  });
  view.signal.addEventListener('abort', () => dialog.close('close'), { once: true });
  return openModal(dialog).then((value) => value === 'done');
}
