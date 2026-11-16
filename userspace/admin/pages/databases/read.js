// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The read of the databases, which only an administrator may make: it signs in
// to PostgreSQL as its superuser. An administrator whose console is unlocked
// reads it when the page opens, when they ask again, after a change and when
// the console is unlocked; never on a timer. One whose console is locked is
// asked to unlock, and a reader is told who sees it, without a request.

import { fill, h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { LOCKED_WORDS, READER_WORDS, readOf } from './model.js';
import { remember } from './seen.js';

/** What the unlock dialog says the console is unlocked to do. */
export const UNLOCK_WORDS = 'list the databases';

/**
 * The read for `view`. `errors` is where a problem of the read goes; `place`
 * is false for a page that says in its own words what a reader and a locked
 * console are told. Gives
 * `{element, load, read, on, mode, data}`: `element` holds the words that
 * stand where the list would be, for a reader and a locked console; `load()`
 * reads when the console is unlocked; `read()` reads again and gives what
 * it read (null when it could not); `on(listener)` is called with `{mode,
 * read}` whenever either changes, `mode` being `reader`, `locked` or `open`.
 */
export function makeRead(view, { errors, place = true }) {
  const element = h('div', { class: 'db-place' });
  const listeners = [];
  let mode = null;
  let data = null;
  let latest = 0;

  const modeOf = () => (view.session.admin !== true ? 'reader' : isUnlocked() ? 'open' : 'locked');
  const tell = () => listeners.forEach((listener) => listener({ mode, read: data }));

  /** The words and the button that stand where the list would be. */
  function drawPlace() {
    if (mode === 'open' || !place) {
      fill(element);
      return;
    }
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', async () => {
      if (await askToUnlock(view.session, UNLOCK_WORDS)) recheck();
    });
    fill(element,
      h('p', { class: 'muted' }, mode === 'reader' ? READER_WORDS : LOCKED_WORDS),
      h('div', { class: 'btns' }, mode === 'locked' && unlock, view.api.inline('databases.list', {})));
  }

  /** Reads as an administrator. Only the last read counts. */
  async function read() {
    if (mode !== 'open') return null;
    latest += 1;
    const mine = latest;
    fill(errors);
    const result = await view.api.run('databases.list', {}, { words: { unlock: UNLOCK_WORDS }, where: { slot: errors } });
    if (mine !== latest || view.signal.aborted) return null;
    const found = result.ok ? readOf(result.result) : null;
    if (found === null) return null;
    data = found;
    remember(view.session.name, found);
    tell();
    return found;
  }

  /** Reads again when the console was unlocked or locked since the last time. */
  function recheck() {
    const next = modeOf();
    if (next === mode) return;
    mode = next;
    if (mode !== 'open') data = null;
    drawPlace();
    tell();
    if (mode === 'open') read();
  }

  view.on('unlock', recheck);
  return {
    element,
    async load() {
      mode = modeOf();
      drawPlace();
      tell();
      await read();
    },
    read,
    on: (listener) => listeners.push(listener),
    get mode() {
      return mode;
    },
    get data() {
      return data;
    },
  };
}
