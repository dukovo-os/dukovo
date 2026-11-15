// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview's cards share: the frame of a card, what it shows while
// it reads and when a read fails, whether a link goes anywhere, and a box
// with a command to copy.

import { asSentence, h, icon, uniqueId } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { inPlaceWords, PLACE, placeOf } from '../../lib/problem-model.js';
import { PAGES } from '../../pages.js';
import { matchPage, SECOND_STEP_PATH, WELCOME_PATH } from '../../shell/route-model.js';

/**
 * Whether a link to `href` goes to a page that is shown: one a folder gives
 * with a `page.js`, or one of the shell's own. A page that a later release
 * adds is not linked yet (design 3.5), and the item shows without a link.
 */
export function linkable(href) {
  if (typeof href !== 'string' || !href.startsWith('/')) return false;
  const path = href.split(/[?#]/)[0];
  if (path === SECOND_STEP_PATH || path === WELCOME_PATH) return true;
  const found = matchPage(PAGES, path);
  return found !== null && found.entry.load !== null;
}

/**
 * A card: `{element, heading, more, body}`. `title` is its h2, `more` the
 * quiet words or link at the right of it; the card's contents go in `body`,
 * which is replaced as the card reads again.
 */
export function makeCard(name, title, { className = '' } = {}) {
  const id = `ov-${name}`;
  const more = h('span', { class: 'more' });
  const heading = h('h2', { id: `${id}-h` }, title, more);
  const body = h('div', { class: 'card-body' }, h('p', { class: 'muted' }, 'Loading…'));
  const element = h('section', { id, class: `card ov-${name} ${className}`.trim(), 'aria-labelledby': `${id}-h` }, heading, body);
  return { element, heading, more, body };
}

/** Whether `error` is the browser failing to reach the server, which the shell says in its own banner. */
const unreachable = (error) => error?.status === 0 && error?.id === null;

/** The words a card shows in place of what it could not read. */
export function failureWords(error) {
  if (placeOf(error) === PLACE.inPlace) return inPlaceWords(error);
  if (unreachable(error)) return 'The server is not answering.';
  const words = error?.title ? [asSentence(error.title), error.fix].filter(Boolean).join(' ') : null;
  return words ?? `This could not be shown: ${error?.message ?? 'something went wrong'}.`;
}

/**
 * What a card does to read: `read()` gives what it needs, `draw(data)` puts
 * it in the card, and a read that fails says why in the card, with a button
 * to try again, unless the card has numbers already and the server is only
 * not answering: they stay, and the shell says so. Gives the function that
 * reads; it never throws.
 */
export function reader(view, body, { read, draw }) {
  let drawn = false;
  async function load() {
    try {
      const data = await read();
      if (view.signal.aborted) return;
      await draw(data);
      drawn = true;
    } catch (error) {
      if (view.signal.aborted || error?.name === 'AbortError') return;
      if (drawn && unreachable(error)) return;
      const again = h('button', { class: 'btn small', type: 'button', onclick: () => { body.replaceChildren(h('p', { class: 'muted' }, 'Loading…')); load(); } }, 'Try again');
      body.replaceChildren(h('p', { class: 'muted', role: 'status' }, failureWords(error)), h('div', { class: 'btns' }, again));
      drawn = false;
    }
  }
  return load;
}

/** A box with `command` in it and a button that copies it, as the Show command panel has. */
export function commandBox(command, { title = 'At a terminal' } = {}) {
  const id = uniqueId('cmd');
  const pre = h('pre', { id, class: 'cmdtext', tabindex: '0' }, command);
  const status = h('span', { class: 'small', role: 'status' });
  const copy = h('button', { class: 'btn small', type: 'button' }, icon('copy'), 'Copy');
  copy.addEventListener('click', async () => {
    status.textContent = (await copyText(command, pre)) ? COPIED : PRESS_CTRL_C;
  });
  return h('div', { class: 'cmd' }, h('div', { class: 'cmd-head' }, icon('code'), title, h('div', { class: 'btns' }, copy)), pre, status);
}
