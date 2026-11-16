// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Signed in now" and End (design 7.7), over a fake of the sessions route and
// of the routes that end a console session and a terminal, in the shapes of
// design 13.3. The route is the lead's and comes after this page: these tests
// hold what the page asks of it and what it does with the answers, and are
// the ones that change if the route's shapes do.

import assert from 'node:assert/strict';
import test from 'node:test';

import { sessionsAnswer, shown } from './accounts-fixtures.mjs';
import {
  buttonOf, openAccounts, refusal, settle,
} from './accounts-view.mjs';

/** The rows of Signed in now: `[title and detail, since, last cell]`. */
const rowsOf = (app) => app.view.main.querySelector('#signed-in').querySelectorAll('tbody tr')
  .map((row) => row.querySelectorAll('td').map((cell) => cell.textContent));

test('Signed in now lists this browser first, then the other browsers, the terminals and the sessions at the machine', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  assert.deepEqual(rowsOf(app), [
    ['This browserFirefox on Linux · 192.168.1.25', 'since 7:12 AM', 'this one'],
    ['Firefox on Android192.168.1.40 · last used 9:50 AM', 'since 8:30 AM', 'End'],
    ['Terminal build2 panes', 'since 9:00 AM', 'End'],
    ['SSH10.8.0.2 · pts/3 · 5 processes', 'since yesterday 9:40 PM', 'End'],
  ]);
  assert.deepEqual(app.calls.filter((path) => path.endsWith('/sessions')), ['/api/1/accounts/sam/sessions']);
});

test('the person at their own account may end their other browsers and terminals, and not a session at the machine or the one in use', async (context) => {
  const app = await openAccounts(context, { route: 'account', unlocked: true });
  assert.deepEqual(rowsOf(app).map((row) => row[2]), ['this one', 'End', 'End', ''], 'a session at the machine is an administrator\'s to end');
  assert.deepEqual(app.calls.filter((path) => path.endsWith('/sessions')), ['/api/1/accounts/chase/sessions']);
});

test('End on a console session asks first, then asks admind\'s route for that session, and reads the sessions again', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const before = app.calls.filter((path) => path.endsWith('/sessions')).length;
  buttonOf(app.view.main.querySelector('#signed-in'), 'End').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'End the session in Firefox on Android from 192.168.1.40?');
  assert.ok(app.dialog().textContent.includes('ends now. Whoever was signed in there has to sign in again.'));
  assert.deepEqual(app.posts, [], 'nothing is asked before it is said yes');
  await app.submit();
  assert.deepEqual(app.posts, [['/api/1/accounts/sam/sessions/s-2/end', {}]]);
  assert.equal(app.dialog(), null);
  assert.equal(app.calls.filter((path) => path.endsWith('/sessions')).length, before + 1);
});

test('End on a terminal asks for the terminal\'s own route and says that its shells are hung up', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  app.view.main.querySelector('#signed-in').querySelectorAll('button').find((one) => one.getAttribute('aria-label') === 'End the terminal build').click();
  await settle();
  assert.ok(app.dialog().textContent.includes('its shells get a hangup, so whatever runs in them stops.'));
  await app.submit();
  assert.deepEqual(app.posts, [['/api/1/terminals/3/end', {}]]);
});

test('End on a session at the machine runs accounts.end with the session as user show gives it, and shows the command', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, prefs: { commands: 'shown' } });
  app.view.main.querySelector('#signed-in').querySelectorAll('button').find((one) => one.getAttribute('aria-label') === 'End the SSH session from 10.8.0.2').click();
  await settle();
  assert.ok(app.dialog().textContent.includes('what does not stop in 5 seconds is killed.'));
  assert.deepEqual(app.panels.at(-1), ['accounts.end', { name: 'sam', session: '4123-1763110000' }]);
  await app.submit();
  assert.deepEqual(app.ran('accounts.end'), [{ name: 'sam', session: '4123-1763110000' }]);
  assert.deepEqual(app.posts, []);
});

test('a name for the button says what it ends, so that a list of them is not a list of End', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const names = app.view.main.querySelector('#signed-in').querySelectorAll('button').map((one) => one.getAttribute('aria-label'));
  assert.deepEqual(names, [
    'End the session in Firefox on Android from 192.168.1.40',
    'End the terminal build',
    'End the SSH session from 10.8.0.2',
  ]);
});

test('Cancel ends nothing and a refusal of the route is shown in the dialog, which stays', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const end = buttonOf(app.view.main.querySelector('#signed-in'), 'End');
  end.click();
  await settle();
  buttonOf(app.dialog(), 'Cancel').click();
  await settle();
  assert.deepEqual(app.posts, []);
  assert.equal(app.dialog(), null);

  const refused = refusal('ADM-136', 'The console does not end the session in use', 409);
  const again = await openAccounts(context, { rest: 'sam', unlocked: true, world: { posts: { '/api/1/accounts/sam/sessions/s-2/end': refused } } });
  buttonOf(again.view.main.querySelector('#signed-in'), 'End').click();
  await settle();
  await again.submit();
  assert.ok(again.dialog());
  assert.equal(again.problems[0][0], refused);
});

test('a sessions route that does not answer leaves the sessions at the machine and says so, and the page does not fail', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam/sessions': new Error('not yet') } } });
  assert.deepEqual(rowsOf(app).map((row) => row[0]), ['SSH10.8.0.2 · pts/3 · 5 processes']);
  assert.ok(app.view.main.querySelector('#signed-in').textContent.includes('could not be read just now'));
  assert.equal(app.problems.length, 0);
});

test('an account with nothing signed in says so, in the words of whose it is', async (context) => {
  const none = shown('sam', { sessions: [] });
  const empty = { sessions: [], terminals: [] };
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': none, '/api/1/accounts/sam/sessions': empty } } });
  assert.ok(app.view.main.querySelector('#signed-in').textContent.includes('sam is not signed in.'));
  const own = await openAccounts(context, { route: 'account', world: { reads: { '/api/1/account/show': { ...shown('chase'), sessions: [] }, '/api/1/accounts/chase/sessions': { sessions: [{ ...sessionsAnswer().sessions[1] }], terminals: [] } } } });
  assert.deepEqual(rowsOf(own).map((row) => row[2]), ['this one']);
  const alone = await openAccounts(context, { route: 'account', world: { reads: { '/api/1/account/show': { ...shown('chase'), sessions: [] }, '/api/1/accounts/chase/sessions': empty } } });
  assert.ok(alone.view.main.querySelector('#signed-in').textContent.includes('Nothing else is signed in as you.'));
});

test('the sessions are read again every ten seconds and only that section is drawn again, so that a dialog opened from the rest is not pulled away', async (context) => {
  let answer = sessionsAnswer();
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam/sessions': () => answer } } });
  const list = app.view.main.querySelector('dl');
  const section = app.view.main.querySelector('#signed-in');
  await app.timers[0][1]();
  assert.equal(app.view.main.querySelector('#signed-in'), section, 'the same sessions draw nothing again');
  answer = { sessions: [sessionsAnswer().sessions[1]], terminals: [] };
  await app.timers[0][1]();
  assert.deepEqual(rowsOf(app).map((row) => row[2]), ['this one', 'End'], 'only the session at the machine is left beside this browser');
  assert.equal(app.view.main.querySelector('dl'), list, 'the rows of the account were not drawn again');
});

test('a locked administrator and a reader are not given sessions, and the sessions route is not asked', async (context) => {
  const locked = await openAccounts(context, { rest: 'sam' });
  assert.equal(locked.view.main.querySelector('#signed-in'), null);
  assert.equal(locked.calls.filter((path) => path.endsWith('/sessions')).length, 0);
  await locked.timers[0][1]();
  assert.equal(locked.calls.filter((path) => path.endsWith('/sessions')).length, 0);
});
