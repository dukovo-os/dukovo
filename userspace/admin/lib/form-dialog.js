// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A modal question with fields: what the change does in its description, the
// fields it needs, and the button that goes on, last and never focused first.
// A change that ends work may ask for a name typed before its button works.
// Read by the page that opens one, when it first needs one.

import { fill, h, uniqueId } from '../dom.js';
import { head, openModal } from '../shell/dialogs.js';

/** How long the command under the fields waits after the last key. */
const TYPING_MS = 400;

/**
 * Opens a dialog and resolves, when it closes, with whether a change was
 * made. `title` and `icon` head it; `lead` is what the change does, in
 * paragraphs of text or nodes, held by the dialog's description; `fields`
 * are nodes under them; `label` is the words of the button that goes on
 * (`danger` draws it as one) and `cancel` the other's. `typed` is `{name,
 * label, why}` when the person has to type `name` first: the button stays
 * off until they have, and `why` says so to a screen reader. `command` is
 * `{action, args}`, or a function that gives it from what the fields hold
 * now (null while they hold too little), for the Show command panel, which
 * comes only while pages show commands and follows the typing.
 * `submit(button, slot)` makes the change: `button` is the one that goes
 * busy, `slot` is where its problems go, and it gives true when the change
 * was made, which closes the dialog, and false when it was not, which
 * leaves it open for the person to put right.
 */
export function openFormDialog(view, {
  title, icon = 'info', lead = [], fields = [], label, danger = false, cancel = 'Cancel', typed = null, command = null, submit,
}) {
  const id = uniqueId('form-dialog');
  const confirm = h('button', { class: danger ? 'btn danger' : 'btn default', type: 'submit' }, label);
  confirm.disabled = typed !== null;
  const close = h('button', { class: 'btn', type: 'button', autofocus: fields.length === 0 && typed === null }, cancel);

  // A problem fills this and leaves it, so it takes no room while it is empty.
  const errors = h('div', { hidden: true });
  new MutationObserver(() => {
    errors.hidden = errors.childElementCount === 0;
  }).observe(errors, { childList: true });

  const answer = typed && h('input', {
    id: `${id}-typed`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': `${id}-why`,
  });
  answer?.addEventListener('input', () => {
    confirm.disabled = answer.value.trim() !== typed.name;
  });
  const typedField = typed && h('div', { class: 'field' },
    h('label', { for: answer.id }, typed.label), answer, h('span', { id: `${id}-why`, class: 'hint' }, typed.why));

  const shown = command !== null && view.prefs.commands === 'shown' ? h('div', {}) : null;
  const drawCommand = () => {
    const call = typeof command === 'function' ? command() : command;
    fill(shown, call && [h('h3', {}, 'This runs'), view.api.panel(call.action, call.args)]);
  };

  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': `${id}-h`, 'aria-describedby': `${id}-lead` },
    h('form', { method: 'dialog', novalidate: true },
      head(`${id}-h`, icon, title),
      h('div', { class: 'dialog-body' },
        h('div', { id: `${id}-lead` }, ...[].concat(lead).map((words) => (typeof words === 'string' ? h('p', {}, words) : words))),
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
