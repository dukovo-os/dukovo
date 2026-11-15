// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How a page reads what only an administrator may (design 3.4): the reads
// that need an unlocked administrator are not made for anyone else, and what a
// card says when it has not read is the same on each page: a line to unlock
// with a button, the line a reader gets, or why the read failed with a way to
// try it again. A read as an administrator is made when a page opens, when
// the person asks again and when the console is unlocked or locked, never on
// a timer. Reliability and Services use it.

import { h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { failureWords } from './card.js';

/** What a card says in place of what it did not read: `what` is what an administrator sees, for a locked administrator, who may unlock, and for a reader. */
export const onlyWords = (what, admin) => (admin ? `Unlock to see ${what}.` : `Administrators see ${what}.`);

/** The reads of one page of `view`: `{mayReadAll, forget, read, note}`. */
export function makeReads(view) {
  // Set when the server says the console is locked, though this page thought it was not: the reads wait for the unlock to change.
  let lockedAnyway = false;

  const mayReadAll = () => Boolean(view.session.admin) && isUnlocked() && !lockedAnyway;

  return {
    mayReadAll,

    /** Forgets what the server said of the lock, when the unlock changes. */
    forget() {
      lockedAnyway = false;
    },

    /**
     * Reads `path`, as an administrator's read (`admin`) only when this
     * session may. Gives `{state: 'ok', value}`, or `{state}` of `locked`
     * (an administrator who may unlock), `reader`, or `error` with the
     * `error`. A read the server refuses for the lock or the account is a
     * state and not an error, since the card says what to do.
     */
    async read(path, { admin = false } = {}) {
      if (admin && !mayReadAll()) return { state: view.session.admin ? 'locked' : 'reader' };
      try {
        return { state: 'ok', value: await view.api.get(path) };
      } catch (error) {
        if (error?.id === 'ADM-122') return { state: 'reader' };
        if (error?.id === 'ADM-106') {
          lockedAnyway = true;
          return { state: 'locked' };
        }
        return { state: 'error', error };
      }
    },

    /**
     * What a card shows in place of what `result` did not read: `what` is
     * what an administrator sees ("the starts"). `retry()` reads again, after
     * the unlock or when the person asks.
     */
    note(result, what, retry) {
      if (result.state === 'error') {
        return h('div', { class: 'read-note' },
          h('p', { class: 'muted', role: 'status' }, failureWords(result.error)),
          h('div', { class: 'btns' }, h('button', { class: 'btn small', type: 'button', onclick: retry }, 'Try again')));
      }
      const locked = result.state === 'locked';
      const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
      unlock.addEventListener('click', async () => {
        if (await askToUnlock(view.session, `see ${what}`)) retry();
      });
      return h('div', { class: 'read-note' },
        h('p', { class: 'muted' }, onlyWords(what, locked)),
        locked && h('div', { class: 'btns' }, unlock));
    },
  };
}
