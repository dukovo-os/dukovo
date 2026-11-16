// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog that every question of the Security page is asked in (design 12):
// a modal named by its heading, with what the change does said in its
// description, the fields it needs, and the buttons that go on, last and never
// focused first. Read when the first dialog is opened.

import {
  fill, h, uniqueId,
} from '../../dom.js';
import { head, openModal } from '../../shell/dialogs.js';

/** How long the command under the fields waits after the last key. */
const TYPING_MS = 400;

/**
 * Opens a dialog. `title` and `icon` head it; `lead` is what the change does,
 * paragraphs of text or nodes, held by the dialog's description; `fields` are
 * nodes under them; `command` is `{action, args}` or a function that gives it
 * from what the fields hold now (null while they hold too little), for the
 * Show command panel that comes only while pages show commands. `buttons` are
 * `{label, kind, submit(button, slot)}` that go on, in order, after Cancel
 * (`cancel` is its words); `kind` is `default` or `danger` or nothing, and
 * `submit` gives true when the change was made, which closes the dialog, and
 * false when it was not, which leaves it open for the person to put right.
 *
 * Gives `{closed, buttons, errors, lead}`: `closed` is a promise of whether a
 * change was made, `buttons` the elements of `buttons`, `errors` the place for
 * a problem, and `lead` the element that holds the description, for a page
 * that changes it as the fields do.
 */
export function openForm(view, {
  title, icon = 'info', lead = [], fields = [], command = null, buttons, cancel = 'Cancel',
}) {
  const id = uniqueId('sec-dialog');
  const close = h('button', { class: 'btn', type: 'button', autofocus: fields.length === 0 }, cancel);
  const errors = h('div', { class: 'sec-errors' });
  const goes = buttons.map((one) => h('button', { class: `btn ${one.kind ?? ''}`.trim(), type: 'button' }, one.label));
  const leadBox = h('div', { id: `${id}-lead`, class: 'sec-lead' }, ...[].concat(lead).map((words) => (typeof words === 'string' ? h('p', {}, words) : words)));
  const shown = command !== null && view.prefs.commands === 'shown' ? h('div', { class: 'sec-command' }) : null;
  const drawCommand = () => {
    const call = typeof command === 'function' ? command() : command;
    fill(shown, call && [h('h3', {}, 'This runs'), view.api.panel(call.action, call.args)]);
  };

  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': `${id}-h`, 'aria-describedby': leadBox.id },
    h('form', { method: 'dialog', novalidate: true },
      head(`${id}-h`, icon, title),
      h('div', { class: 'dialog-body' }, leadBox, ...fields, errors, shown),
      h('div', { class: 'dialog-foot' }, close, ...goes)));

  let working = false;
  let waiting = null;
  if (shown) {
    drawCommand();
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
  goes.forEach((button, at) => button.addEventListener('click', async () => {
    fill(errors);
    working = true;
    let done = false;
    try {
      done = await buttons[at].submit(button, errors);
    } finally {
      working = false;
    }
    if (done && !view.signal.aborted) dialog.close('done');
  }));
  view.signal.addEventListener('abort', () => dialog.close('close'), { once: true });
  return {
    closed: openModal(dialog).then((value) => value === 'done'), buttons: goes, errors, lead: leadBox,
  };
}
