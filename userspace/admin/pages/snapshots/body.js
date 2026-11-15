// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the cards of the page put in their body before they have anything to
// say: grey bars while it is read, and the words and a way to try again when
// it could not be.

import { h } from '../../dom.js';

/** The grey bars that stand where the rows are read. They are still, and nothing reads them. */
export const bars = () => [0, 1, 2].map(() => h('span', { class: 'snap-bar' }));

/** The words of a read that failed, with a button that reads again. */
export function failed(words, retry) {
  const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
  again.addEventListener('click', retry);
  return [h('p', { class: 'muted', role: 'status' }, words), h('div', { class: 'btns' }, again)];
}
