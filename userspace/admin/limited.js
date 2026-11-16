// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What app.js shows when it is not the shell: the page for a start that
// failed, and the frame of a session that the sign-in rules limit to adding
// a second step. app.js reads this file when one of them is called for.

import { errorSlot, h, marks, sheet, showError } from './dom.js';
import { followEvents } from './shell/events.js';
import { setUnlockSeconds } from './unlock.js';

/** How long to wait before asking again when the event stream is closed for good. */
const STREAM_RETRY_MS = 5000;

/** The page for a start that failed: what admind said, and a button that starts again with `retry`. */
export async function showFailure({ app, error, retry }) {
  await Promise.all([sheet('/sign-in.css'), sheet('/dialogs.css')]);
  const problem = errorSlot('start-error');
  document.title = 'Dukovo Admin';
  app.replaceChildren(
    h('main', { class: 'sign-in' },
      h('div', { class: 'sign-in-dialogs' },
        h('div', { class: 'dialog narrow' },
          h('div', { class: 'dialog-body roomy' },
            h('h1', { class: 'dialog-title' }, 'The console could not start'),
            problem,
            h('button', { class: 'btn default block', type: 'button', onclick: retry }, 'Try again'))))));
  showError(problem, error);
}

/**
 * The frame of a session limited to adding a second step: the brand, the
 * name and Sign out, and that page alone. `still()` says whether the
 * session is still there once the page's code is read. Gives the event
 * stream it follows, or null when the session had gone.
 */
export async function showLimited({ app, session, still, signOut, signInAgain, onEnded, onClosed }) {
  document.title = `Second step · ${session.host} · Dukovo Admin`;
  const { secondStepView } = await import('./account-steps.js');
  if (!still()) return null;
  const main = h('main', { class: 'main' }, ...secondStepView({ session, signInAgain }));
  app.replaceChildren(
    h('header', { class: 'top' },
      h('a', { class: 'brand', href: '/' }, ...marks(17, 22), 'Dukovo Admin'),
      h('span', { class: 'spacer' }),
      h('span', {}, session.name),
      h('button', { class: 'topbtn', type: 'button', onclick: signOut }, 'Sign out')),
    h('div', { class: 'shell' }, main));
  main.querySelector('h1').focus();
  return followEvents({
    onEvent(name, data) {
      if (name === 'unlock') setUnlockSeconds(data.unlock_seconds);
      if (name === 'session') onEnded(data.ended);
    },
    onFailure(closed) {
      if (closed) setTimeout(onClosed, STREAM_RETRY_MS);
    },
  });
}
