// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The console's router. It asks admind who is signed in and draws the view
// that answer calls for: the sign-in page when nobody is, the page that adds
// a second step when the sign-in rules limit the session to that, else the
// signed-in frame with its page. It follows the session's event stream for
// as long as the session lasts, which also tells admind that a tab is open.

import { secondStepView } from './account-steps.js';
import { get, post, setCsrf, whenSessionEnds } from './api.js';
import { errorSlot, h, marks, showError } from './dom.js';
import { showSignIn } from './sign-in.js';
import { accessControl, setUnlockSeconds } from './unlock.js';

const app = document.getElementById('app');

// Why admind says a session ended, in the words that follow "Sign in again:".
const ENDED = {
  'signed-out': 'you signed out.',
  expired: 'the session reached its time limit.',
  idle: 'no console tab was open for a while.',
  restarted: 'the console restarted.',
  worker: 'the process that works for your session stopped.',
};

// How long to wait before asking again when the event stream is closed
// for good, so that a server that keeps refusing it is not asked in a loop.
const STREAM_RETRY_MS = 5000;

let session = null;
let events = null;
let main = null;
// Whether this page has had a session. A 401 on the first request is a
// person who has not signed in yet, and needs no notice; a 401 after one
// is a session that ended where the page could not see it.
let hadSession = false;

/** Ends what belongs to the signed-in session: its stream, unlock state and open dialogs. */
function forgetSession() {
  events?.close();
  events = null;
  session = null;
  setUnlockSeconds(null);
  for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close();
}

function showSignInPage(notice) {
  forgetSession();
  showSignIn(app, { notice, onSignedIn: start });
}

function showFailure(error) {
  const problem = errorSlot('start-error');
  document.title = 'Dukovo Admin';
  app.replaceChildren(
    h('main', { class: 'sign-in' },
      h('div', { class: 'sign-in-dialogs' },
        h('div', { class: 'dialog narrow' },
          h('div', { class: 'dialog-body roomy' },
            h('h1', { class: 'dialog-title' }, 'The console could not start'),
            problem,
            h('button', { class: 'btn default block', type: 'button', onclick: start }, 'Try again'))))));
  showError(problem, error);
}

function followEvents() {
  const source = new EventSource('/api/1/events');
  source.addEventListener('unlock', (event) => setUnlockSeconds(JSON.parse(event.data).unlock_seconds));
  source.addEventListener('session', (event) => {
    const { ended } = JSON.parse(event.data);
    showSignInPage({ title: 'Sign in again:', detail: ENDED[ended] ?? 'the session ended.' });
  });
  source.addEventListener('error', () => {
    // The browser reconnects a lost stream by itself, and gives up only on
    // an answer that is not a stream, such as a session that is gone.
    if (source.readyState === EventSource.CLOSED) setTimeout(start, STREAM_RETRY_MS);
  });
  return source;
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
  showSignInPage(notice);
}

function homeView() {
  return [
    h('div', { class: 'ph' }, h('div', { class: 'title' }, h('h1', { tabindex: '-1' }, session.host))),
    h('p', {},
      'Signed in as ', h('b', {}, session.name), session.full_name && ` (${session.full_name})`, '. ',
      session.admin
        ? 'This account is an administrator: unlock the console from the header to make changes.'
        : 'This account may look but not change anything: it is not an administrator.'),
    h('section', { class: 'card' },
      h('h2', {}, 'Second step'),
      h('p', { class: 'muted' }, 'An authenticator app, passkeys and recovery codes for signing in.'),
      h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/second-step' }, 'Manage the second step'))),
  ];
}

function topBar() {
  return h('header', { class: 'top' },
    h('a', { class: 'brand', href: '/' }, ...marks(17, 22), 'Dukovo Admin'),
    h('span', { class: 'spacer' }),
    !session.needs_step && accessControl(session),
    h('span', {}, session.name),
    h('button', { class: 'topbtn', type: 'button', onclick: () => signOut() }, 'Sign out'));
}

/** Draws the page that the address and the session call for, inside the signed-in frame. */
function route() {
  if (!main?.isConnected) {
    main = h('main', { class: 'main' });
    app.replaceChildren(topBar(), h('div', { class: 'shell' }, main));
  }
  const secondStep = session.needs_step || location.pathname === '/second-step';
  document.title = `${secondStep ? 'Second step' : session.host} · Dukovo Admin`;
  main.replaceChildren(...(secondStep
    ? secondStepView({
      session,
      signInAgain: () => signOut({ title: 'The second step is added.', detail: 'Sign in again to use it.' }),
    })
    : homeView()));
  main.querySelector('h1').focus();
}

async function start() {
  events?.close();
  try {
    session = await get('/api/1/session');
  } catch (error) {
    if (error.status === 401) {
      showSignInPage(hadSession ? { title: error.title, detail: error.detail } : undefined);
    } else {
      showFailure(error);
    }
    return;
  }
  hadSession = true;
  setCsrf(session.csrf);
  setUnlockSeconds(session.unlock_seconds);
  events = followEvents();
  route();
}

// A link to another view of the console is opened here, without a page load.
document.addEventListener('click', (event) => {
  const link = event.target.closest?.('a[href^="/"]');
  if (!session || !link || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey) return;
  event.preventDefault();
  history.pushState(null, '', link.getAttribute('href'));
  route();
});
window.addEventListener('popstate', () => session && route());
whenSessionEnds((error) => session && showSignInPage({ title: error.title, detail: error.detail }));

start();
