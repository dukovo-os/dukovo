// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The console's start. It asks admind who is signed in and shows what that
// calls for: the sign-in page when nobody is, the second step's page alone
// when the sign-in rules limit the session to that, else the shell
// (shell/frame.js). It follows the session's event stream while the session
// lasts, which also tells admind that a tab is open.

import { get, post, setCsrf, whenSessionEnds } from './api.js';
import { enterShell } from './shell/frame.js';
import { setUnlockSeconds } from './unlock.js';

const app = document.getElementById('app');

// Why admind says a session ended, in the words that follow "Sign in again:".
const ENDED = {
  'signed-out': 'you signed out.',
  expired: 'the session reached its time limit.',
  idle: 'no console tab was open for a while.',
  restarted: 'the console restarted.',
  worker: 'the process that works for your session stopped.',
};

let session = null;
let events = null;
let shell = null;
// Counts the starts, so that one overtaken by a later one does not draw over it.
let started = 0;
// Whether this page has had a session: a 401 on the first request is a person
// who has not signed in and needs no notice; a later one is a session that
// ended where the page could not see it.
let hadSession = false;

// The tabs of this browser share its session's cookie, which a sign-out
// clears. A tab whose request then finds no session cannot learn why from
// admind, and its stream may not have said so first: the tab that signs out
// tells the others, which say "you signed out" in place of not knowing.
const otherTabs = typeof BroadcastChannel === 'function' ? new BroadcastChannel('dukovo-admin-session') : null;
// Whether the sign-in page shows an end whose reason admind did not give.
let endedUnknown = false;

/** Ends what belongs to the signed-in session: its stream, shell, unlock state and open dialogs. */
function forgetSession() {
  events?.close();
  events = null;
  shell?.stop();
  shell = null;
  session = null;
  setUnlockSeconds(null);
  for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close();
}

// The sign-in page, the second step's frame and the failure page are other
// modules, read when they are needed: a signed-in console does not carry them.
async function showSignInPage(notice, { unknown = false } = {}) {
  endedUnknown = unknown;
  forgetSession();
  const { showSignIn } = await import('./sign-in.js');
  showSignIn(app, { notice, onSignedIn: start });
}

/** The sign-in page for a session that ended, with the reason (a key of ENDED) in words. */
function sessionEnded(why, { error } = {}) {
  const reason = ENDED[why] ?? 'the session ended.';
  // The terminals are the account's and go on after its session, except
  // when the console restarted, which ends them.
  const terminals = why !== 'restarted' && shell?.terminals() > 0 ? ' Your terminals are still running.' : '';
  if (error && !shell?.wasOffline()) {
    showSignInPage({ title: error.title, detail: error.detail }, { unknown: true });
    return;
  }
  showSignInPage({ title: 'Sign in again:', detail: `${reason}${terminals}` });
}

/** The page for a start that failed, which another module draws. */
async function showFailure(error) {
  const { showFailure: draw } = await import('./limited.js');
  draw({ app, error, retry: start });
}

/** Signs the session out and shows the sign-in page, with `notice` if given. */
async function signOut(notice) {
  try {
    await post('/api/1/sign-out');
  } catch (error) {
    // A session that has ended already is as good as signed out.
    if (error.status !== 401) {
      showFailure(error);
      return;
    }
  }
  otherTabs?.postMessage('signed-out');
  showSignInPage(notice);
}

otherTabs?.addEventListener('message', (event) => {
  if (event.data === 'signed-out' && (session !== null || endedUnknown)) sessionEnded('signed-out');
});

const signInAgain = () => signOut({ title: 'The second step is added.', detail: 'Sign in again to use it.' });

/** The frame of a session that the sign-in rules limit to adding a second step, which another module draws. */
async function limitedFrame() {
  const { showLimited } = await import('./limited.js');
  events = await showLimited({
    app,
    session,
    still: () => session !== null,
    signOut: () => signOut(),
    signInAgain,
    onEnded: (reason) => sessionEnded(reason),
    onClosed: start,
  });
}

async function start() {
  started += 1;
  const mine = started;
  forgetSessionKeepingPage();
  try {
    session = await get('/api/1/session');
  } catch (error) {
    if (mine !== started) return;
    if (error.status === 401) {
      showSignInPage(hadSession ? { title: error.title, detail: error.detail } : undefined, { unknown: hadSession });
    } else {
      showFailure(error);
    }
    return;
  }
  if (mine !== started) return;
  hadSession = true;
  setCsrf(session.csrf);
  setUnlockSeconds(session.unlock_seconds);
  if (session.needs_step) {
    await limitedFrame();
    return;
  }
  const entered = await enterShell({
    app,
    session,
    signOut,
    signInAgain,
    onEnded: (ended) => sessionEnded(ended),
    onClosed: start,
    onFailure: showFailure,
  });
  if (mine !== started) {
    entered?.stop();
    return;
  }
  shell = entered;
}

/** Stops the stream and the shell of an earlier start without clearing the page, which the next one draws over. */
function forgetSessionKeepingPage() {
  events?.close();
  events = null;
  shell?.stop();
  shell = null;
}

whenSessionEnds((error) => session && sessionEnded('restarted', { error }));

start();
