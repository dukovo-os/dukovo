// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the cards and dialogs of the AI clients page share: a card's frame,
// a box of text to copy under a line that says what it is, a command on a
// line of its own, a read that tells a locked console and a failure apart,
// and what a dialog says of a choice it cannot take.

import { fill, h, icon, uniqueId } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';

/** The card `id`, headed by `heading`: `{element, errors, body}`. */
export function section(id, ...heading) {
  const errors = h('div', {});
  const body = h('div', { class: 'cl-body' });
  const element = h('section', { class: 'card', id, 'aria-labelledby': `${id}-h` },
    h('h2', { id: `${id}-h`, tabindex: '-1' }, ...heading), errors, body);
  return { element, errors, body };
}

/**
 * `text` in a box named `label`, under a line of `words` with Copy. The
 * text is public: a token goes into one only in the dialog that shows it
 * once. Gives `{element, set(text)}`.
 */
export function commandBox(words, text, label) {
  const id = uniqueId('cl-cmd');
  const pre = h('pre', { id, tabindex: '0' }, text);
  const status = h('span', { class: 'small', role: 'status' });
  const copy = h('button', { class: 'btn small', type: 'button', 'aria-describedby': id }, icon('copy'), 'Copy');
  copy.addEventListener('click', async () => {
    status.textContent = (await copyText(pre.textContent, pre)) ? COPIED : PRESS_CTRL_C;
  });
  return {
    element: h('div', { class: 'cmd', role: 'group', 'aria-label': label },
      h('div', { class: 'cmd-head' }, icon('code'), words, h('div', { class: 'btns' }, status, copy)), pre),
    set(next) {
      pre.textContent = next;
    },
  };
}

/**
 * Reads `path` for `view`: `{state: 'ok', value}`, `{state: 'locked'}` for
 * a read that needs the console unlocked, or `{state: 'error', error}`.
 */
export async function read(view, path) {
  try {
    return { state: 'ok', value: await view.api.get(path) };
  } catch (error) {
    if (error?.name === 'AbortError') throw error;
    return error?.id === 'ADM-106' ? { state: 'locked' } : { state: 'error', error };
  }
}

/** A command that is no action's, on a line that shows while Show commands is on. Made once for a card, since it follows the switch. */
export function plainCommand(view, text) {
  const line = h('span', { class: 'inline-cmd', hidden: view.prefs.commands !== 'shown' }, text);
  view.onPrefs((prefs) => {
    line.hidden = prefs.commands !== 'shown';
  });
  return line;
}

/** What a card says in place of `what` when its read failed, with a way to read again. */
export function unread(result, what, retry) {
  return h('div', { class: 'cl-body' },
    h('p', { class: 'muted', role: 'status' }, `${what} could not be read: ${result.error?.title ?? 'no answer'}.`),
    h('div', { class: 'btns' }, h('button', { class: 'btn small', type: 'button', onclick: retry }, 'Try again')));
}

/** Says in `slot` what is wrong with what the person chose; `input`, when given, is the field it is about, which takes the focus. */
export function refuse(slot, words, input = null) {
  fill(slot, h('p', { class: 'msg', role: 'alert' }, icon('warn'), h('span', {}, words)));
  if (input) {
    input.setAttribute('aria-invalid', 'true');
    input.addEventListener('input', () => input.removeAttribute('aria-invalid'), { once: true });
    input.focus();
  }
}
