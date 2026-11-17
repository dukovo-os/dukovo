// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How the Alerts page reads what only an administrator may: not at all for a
// reader or a locked console, which get a line that says so, and with a way
// to read again for a read that failed.

import { h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';

/** The reads of `view`: `{read(path), note(result, what, retry)}`. */
export function makeReads(view) {
  return {
    /** Gives `{state: 'ok', value}`, or `{state}` of `locked`, `reader` or `error` with the `error`. */
    async read(path) {
      if (view.session.admin !== true) return { state: 'reader' };
      if (!isUnlocked()) return { state: 'locked' };
      try {
        return { state: 'ok', value: await view.api.get(path) };
      } catch (error) {
        if (error?.id === 'ADM-106') return { state: 'locked' };
        if (error?.id === 'ADM-122') return { state: 'reader' };
        return { state: 'error', error };
      }
    },

    /** What a card says in place of what `result` did not read: `what` is what an administrator sees ("the alerts"). */
    note(result, what, retry) {
      if (result.state === 'error') {
        return h('div', { class: 'al-body' },
          h('p', { class: 'muted', role: 'status' }, `${what.charAt(0).toUpperCase()}${what.slice(1)} could not be read: ${result.error?.title ?? 'no answer'}.`),
          h('div', { class: 'btns' }, h('button', { class: 'btn small', type: 'button', onclick: retry }, 'Try again')));
      }
      const locked = result.state === 'locked';
      const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
      unlock.addEventListener('click', async () => {
        if (await askToUnlock(view.session, `see ${what}`)) retry();
      });
      return h('div', { class: 'al-body' }, h('p', { class: 'muted' }, locked ? `Unlock to see ${what}.` : `Administrators see ${what}.`), locked && h('div', { class: 'btns' }, unlock));
    },
  };
}
