// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What an account is signed in with, and End (design 7.7): its console
// sessions, its console terminals and its sessions at the machine. The person
// ends their own other sessions and terminals; an unlocked administrator ends
// any of another account's. The session in use is Sign out's, and says "this
// one". Console sessions and terminals end by admind's own routes (13.3), and
// a session at the machine by `accounts.end`.

import { h } from '../../dom.js';
import { endCall, mayEnd } from './model.js';
import { openForm } from './open.js';

/** What happens to the person or the work when each kind of row is ended. */
const ENDS = Object.freeze({
  console: (what) => `${what} ends now. Whoever was signed in there has to sign in again.`,
  terminal: (what) => `${what} ends now: its shells get a hangup, so whatever runs in them stops.`,
  machine: (what) => `${what} is asked to stop, and what does not stop in 5 seconds is killed.`,
});

/** The first letter of `words` in capitals, for a sentence that begins with a name. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/**
 * The section "Signed in now". `ctx` is `{view, kind, name, rows, unread,
 * reload}`: `rows` are `signedIn`'s, `unread` is whether the sessions
 * route did not answer (the rows are then the machine's only).
 */
export function sessionsSection(ctx) {
  const {
    view, kind, name, rows, unread, reload,
  } = ctx;

  /** Asks first, then ends `row`. */
  async function end(row, opener) {
    const call = endCall(name, row);
    const done = await openForm(view, {
      title: `End ${row.what}?`,
      icon: 'warn',
      lead: [ENDS[row.scope](capital(row.what))],
      label: 'End it',
      danger: true,
      command: call.action ? call : null,
      async submit(confirm, errors) {
        if (call.action) {
          return (await view.api.run(call.action, call.args, {
            button: confirm, words: { unlock: `end ${row.what}`, done: 'It is ended.' }, where: { slot: errors },
          })).ok;
        }
        try {
          await view.api.post(call.path, {});
          view.notice('It is ended.', { kind: 'ok' });
          return true;
        } catch (error) {
          view.problem(error, { slot: errors });
          return false;
        }
      },
    });
    if (done) reload();
    else opener.focus();
  }

  const lines = rows.map((row) => {
    let last = h('span', { class: 'small muted' }, row.this ? 'this one' : '');
    if (mayEnd(row, kind)) {
      last = h('button', { class: 'btn small', type: 'button', 'aria-label': `End ${row.what}`, 'data-action': 'end' }, 'End');
      last.addEventListener('click', () => end(row, last));
    }
    return h('tr', {},
      h('td', { 'data-label': 'Signed in with' }, h('b', {}, row.title), row.detail && h('small', {}, row.detail)),
      h('td', { 'data-label': 'Since' }, row.since),
      h('td', { class: 'end' }, last));
  });

  return h('section', { class: 'acc-section', id: 'signed-in', 'aria-labelledby': 'acc-signed-h' },
    h('h3', { id: 'acc-signed-h' }, 'Signed in now'),
    lines.length > 0
      ? h('div', { class: 'tbl-box' }, h('table', { class: 'tbl acc-mini stack' }, h('caption', { class: 'sr-only' }, 'Signed in now'), h('tbody', {}, lines)))
      : h('p', { class: 'muted' }, kind === 'own' ? 'Nothing else is signed in as you.' : `${name} is not signed in.`),
    unread && h('p', { class: 'small muted' }, 'The console sessions and terminals could not be read just now. They are tried again in a few seconds.'));
}
