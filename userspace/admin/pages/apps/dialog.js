// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog that the questions of the Apps pages are asked in (going back,
// removing, updating without asking): a modal named by its heading, with what
// the change does said in its description, the fields it needs, the command it
// runs while pages show commands, and a button that goes on, last and never
// focused first. Read when the first dialog is opened.

import { fill, h, uniqueId } from '../../dom.js';
import { head, openModal } from '../../shell/dialogs.js';

/** How long the command under the fields waits after the last key. */
const TYPING_MS = 400;

/**
 * Opens a dialog and resolves, when it closes, with whether the change was
 * made. `title` and `icon` head it; `lead` is what the change does, in
 * paragraphs of text or nodes, held by the dialog's description; `fields` are
 * nodes under it; `label` is the words of the button that goes on (`danger`
 * draws it as one); `command` is `{action, args}`, or a function that gives it
 * from what the fields hold now, for the Show command panel that comes only
 * while pages show commands. `submit(button, slot)` makes the change: `button`
 * is the one that goes busy and `slot` is where its problems go; it gives true
 * when the change was made, which closes the dialog, and false when it was
 * not, which leaves the dialog open for the person to put right.
 */
export function openQuestion(view, {
  title, icon = 'info', lead, fields = [], label, danger = false, command = null, submit,
}) {
  const id = uniqueId('apps-dialog');
  const confirm = h('button', { class: danger ? 'btn danger' : 'btn default', type: 'submit' }, label);
  const close = h('button', { class: 'btn', type: 'button', autofocus: fields.length === 0 }, 'Cancel');
  const errors = h('div', { class: 'apps-errors' });
  const shown = command !== null && view.prefs.commands === 'shown' ? h('div', { class: 'apps-command' }) : null;
  /** Draws the command that the fields hold now, or nothing while they hold too little. */
  const drawCommand = () => {
    const call = typeof command === 'function' ? command() : command;
    fill(shown, call && [h('h3', {}, 'This runs'), view.api.panel(call.action, call.args)]);
  };

  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': `${id}-h`, 'aria-describedby': `${id}-lead` },
    h('form', { method: 'dialog', novalidate: true },
      head(`${id}-h`, icon, title),
      h('div', { class: 'dialog-body' },
        h('div', { id: `${id}-lead`, class: 'apps-lead' }, [].concat(lead).map((words) => (typeof words === 'string' ? h('p', {}, words) : words))),
        ...fields,
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
