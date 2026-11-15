// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A line of text in a box with a button that copies it: the command to type
// at a terminal, and this machine's SSH key to give a server. The text is
// public, so it stays in the page; a secret goes to shown-once.js instead.

import { h, icon, uniqueId } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';

/** `text` in a box named by `label`, with Copy, and a polite line that says whether the copy went through. */
export function copyBox(text, label) {
  const id = uniqueId('snap-copy');
  const pre = h('pre', { id, class: 'cmdtext', tabindex: '0' }, text);
  const status = h('span', { class: 'small', role: 'status' });
  const copy = h('button', { class: 'btn small', type: 'button', 'aria-describedby': id }, icon('copy'), 'Copy');
  copy.addEventListener('click', async () => {
    status.textContent = (await copyText(text, pre)) ? COPIED : PRESS_CTRL_C;
  });
  return h('div', { class: 'cmd', role: 'group', 'aria-label': label }, pre, h('div', { class: 'btns' }, copy, status));
}
