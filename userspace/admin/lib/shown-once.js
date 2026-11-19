// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog for a secret that an action answered once (design 3.5 and 3.6,
// ADR 0561): a recovery key, a device's settings. The text is in a `pre`,
// with Copy, Save as file and, when asked, a QR code; it is not read from
// anywhere after the call that gave it, and the dialog drops it when it
// closes: the text, the picture and the variable that held it. The QR code is
// read only when it is asked for, so that the page that shows a key carries
// none of it. Nothing runs at import.

import { h, icon, uniqueId } from '../dom.js';
import { ask, openModal } from '../shell/dialogs.js';
import { COPIED, copyText, PRESS_CTRL_C } from './copy.js';
import { lowerFirst } from './action-model.js';
import { saveText } from './save-file.js';

/** The words of the question that Escape asks, when `loss` says nothing else. */
const LOSS = 'It is shown only now.';

/** The QR code of `text` for `label`, or null when `text` is longer than a code holds. */
async function pictureOf(text, label) {
  const { qrSvg } = await import('../qr.js');
  try {
    return qrSvg(text, label);
  } catch (error) {
    if (error instanceof RangeError) return null;
    throw error;
  }
}

/**
 * Shows `text`, a secret that was answered, in a modal dialog named `title`,
 * and gives a promise that is true when the person said they kept it ("I
 * have kept it") and false when the dialog was closed in another way, or
 * when the page was left. `view` is the page's, whose `signal` closes it.
 *
 * `label` names what the text is ("Recovery key", "Settings"); `file` is
 * the name that Save as file gives, or null for no such button; `qr` adds
 * a QR code, with the text behind "Show the … as text"; `warn` is what to
 * keep it for, in a warning over the text; `loss` is what is lost when the
 * dialog is closed without keeping it, said by the question that Escape
 * asks.
 */
export async function showOnce(view, {
  title, label = 'The text', text, file = null, qr = false, warn = null, loss = LOSS,
}) {
  if (view.signal.aborted) return false;
  let secret = text;
  const id = uniqueId('once');
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1', autofocus: true }, title);
  const labelId = `${id}-label`;
  const pre = h('pre', { class: 'cmdtext', tabindex: '0' }, secret);
  const status = h('p', { class: 'small', role: 'status' });
  const picture = qr ? await pictureOf(secret, `QR code of the ${lowerFirst(label)}. The same text is under Show the ${lowerFirst(label)} as text.`) : null;
  if (view.signal.aborted) return false;

  const buttons = [
    h('button', { class: 'btn', type: 'button', onclick: async () => {
      status.textContent = (await copyText(secret ?? '', pre)) ? COPIED : PRESS_CTRL_C;
    } }, icon('copy'), 'Copy'),
    file && h('button', { class: 'btn', type: 'button', onclick: () => {
      saveText(file, secret ?? '');
      status.textContent = `Saved as ${file}.`;
    } }, 'Save as file'),
  ];
  if (picture) {
    pre.hidden = true;
    const words = `Show the ${lowerFirst(label)} as text`;
    const reveal = h('button', { class: 'btn', type: 'button', 'aria-expanded': 'false', onclick: () => {
      pre.hidden = !pre.hidden;
      reveal.setAttribute('aria-expanded', String(!pre.hidden));
    } }, words);
    buttons.push(reveal);
  }

  // The text leaves the page in the step that closes the dialog, not when
  // `close` arrives: the browser delivers that in a later task, and until
  // then the hidden dialog is still in the document with the text in its
  // `pre`. A form with method=dialog closes it during its submit.
  function drop() {
    secret = null;
    pre.replaceChildren();
    picture?.remove();
  }

  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': heading.id },
    h('form', { method: 'dialog', onsubmit: drop },
      h('div', { class: 'dialog-head' }, icon('key'), heading),
      h('div', { class: 'dialog-body' },
        warn && h('div', { class: 'banner warn' }, icon('warn'), h('div', {}, h('p', {}, warn))),
        h('div', { role: 'group', 'aria-labelledby': labelId },
          h('b', { id: labelId }, label),
          picture && h('div', { class: 'qr-row' }, picture),
          pre),
        qr && !picture && h('p', { class: 'small muted' }, 'It is too long for a QR code. Copy it or save it as a file.'),
        h('div', { class: 'btns' }, ...buttons),
        status),
      h('div', { class: 'dialog-foot' }, h('button', { class: 'btn default', type: 'submit', value: 'kept' }, 'I have kept it'))));

  // Escape asks first: a secret closed unkept may be gone for good.
  let asking = false;
  dialog.addEventListener('cancel', async (event) => {
    event.preventDefault();
    if (asking) return;
    asking = true;
    const choice = await ask({
      title: 'Close without keeping it?',
      words: loss,
      iconName: 'warn',
      choices: [{ label: 'Go back', value: 'back', kind: 'default', safe: true }, { label: 'Close without keeping it', value: 'close' }],
    });
    asking = false;
    if (choice === 'close') {
      drop();
      dialog.close('');
    }
  });
  view.signal.addEventListener('abort', () => {
    drop();
    dialog.close('');
  }, { once: true });

  return (await openModal(dialog)) === 'kept';
}
