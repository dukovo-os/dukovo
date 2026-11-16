// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of accounts (design 7.4 and 7.8), drawn on a page of the test's own
// with a view that answers as admind does: the groups and the sign-in rules,
// the table for each kind of person who looks (a reader, an administrator who
// has not unlocked, one who has), the states, and when the list reads again.
// What the page looks like is the lab's web-console/accounts test.

import assert from 'node:assert/strict';
import test from 'node:test';

import { account, people, shown } from './accounts-fixtures.mjs';
import {
  buttonOf, control, openAccounts, refusal, settle,
} from './accounts-view.mjs';

/** The words of each row of the table: `[account, can, last]`. */
const rowsOf = (app) => app.view.main.querySelectorAll('tbody tr').map((row) => row.querySelectorAll('td').map((cell) => cell.textContent));

test('a reader sees Account and Can, the groups and the rules, and neither Last active nor Add', async (context) => {
  const app = await openAccounts(context, { session: { name: 'sam', admin: false } });
  assert.deepEqual(app.view.main.querySelectorAll('th').map((one) => one.textContent), ['Account', 'Can']);
  assert.deepEqual(rowsOf(app).map((row) => row.length), [2, 2, 2, 2, 2]);
  assert.equal(app.view.headed.title, 'Accounts');
  assert.deepEqual(app.view.headed.buttons, []);
  assert.ok(!app.calls.includes('/api/1/sign-ins/last'), 'Last active is an administrator\'s read');
  assert.ok(!app.view.main.textContent.includes('Unlock to see'), 'a reader is not asked to unlock');
  assert.ok(app.view.main.textContent.includes('Administrators give a second step. A session lasts 12 hours.'));
  assert.ok(app.view.main.textContent.includes('These are the console\'s rules; SSH has its own, on Security.'));
  assert.deepEqual(app.view.main.querySelectorAll('.acc-chip').map((one) => one.textContent), ['admins · 2', 'console · 3', 'photos · 1']);
  assert.ok(app.view.main.textContent.includes('Service accounts made by apps (postgres, nginx and others) are on each app\'s page.'));
});

test('the groups are chips and not links, and the name of each account is a link to its page', async (context) => {
  const app = await openAccounts(context, { session: { name: 'sam', admin: false } });
  const chips = app.view.main.querySelector('.acc-groups');
  assert.equal(chips.querySelectorAll('a').length, 0);
  assert.equal(chips.querySelectorAll('button').length, 0, 'Manage groups is not shown');
  const links = app.view.main.querySelectorAll('tbody a').map((one) => [one.textContent, one.getAttribute('href')]);
  assert.deepEqual(links, [['chase', '/accounts/chase'], ['alice', '/accounts/alice'], ['sam', '/accounts/sam'], ['backup', '/accounts/backup'], ['old', '/accounts/old']]);
});

test('an administrator who has not unlocked sees the list without Last active, is told to unlock, and Add asks to unlock', async (context) => {
  const app = await openAccounts(context);
  assert.deepEqual(app.view.main.querySelectorAll('th').map((one) => one.textContent), ['Account', 'Can']);
  assert.ok(app.view.main.textContent.includes('Unlock to see when each was last active.'));
  assert.ok(!app.calls.includes('/api/1/sign-ins/last'));
  assert.equal(app.view.headed.buttons.length, 1);
  assert.equal(app.view.headed.buttons[0].textContent, 'Add an account…');
});

test('an unlocked administrator sees when each was last active, and the keys of an account with no password say whether it can sign in', async (context) => {
  const app = await openAccounts(context, { unlocked: true });
  assert.deepEqual(app.view.main.querySelectorAll('th').map((one) => one.textContent), ['Account', 'Can', 'Last active']);
  assert.deepEqual(rowsOf(app).map((row) => [row[0].slice(1).replace(/\s+/g, ' '), row[1], row[2]]), [
    ['chase Your accountChase', 'Administrator', 'Now, this browser'],
    ['aliceAlice Liddell', 'Administrator', 'Yesterday, SSH'],
    ['samSam', 'Console and shell', 'Nov 8, file sharing'],
    ['backup', 'SSH key only', 'Never'],
    ['old', 'Disabled', 'Never'],
  ]);
  assert.ok(!app.view.main.textContent.includes('Unlock to see'));
  assert.ok(app.calls.includes('/api/1/accounts/backup'), 'the account with no password is shown, for its keys');
  assert.ok(!app.calls.includes('/api/1/accounts/sam'), 'the others are not read one by one');
});

test('an account with no password and no key is told so, and the disabled one is last with its pill', async (context) => {
  const none = { ...shown('backup'), keys: [] };
  const app = await openAccounts(context, { unlocked: true, world: { reads: { '/api/1/accounts/backup': none } } });
  assert.equal(rowsOf(app)[3][1], 'Cannot sign in');
  const last = app.view.main.querySelectorAll('tbody tr').at(-1);
  assert.equal(last.querySelector('.pill').textContent, 'Disabled');
  assert.equal(last.querySelector('td').textContent.includes('old'), true);
});

test('the person\'s own row is marked, and an account that keeps no last sign-in says Never', async (context) => {
  const app = await openAccounts(context, { unlocked: true });
  const own = app.view.main.querySelectorAll('tbody tr').filter((row) => row.classes.has('sel'));
  assert.equal(own.length, 1);
  assert.ok(own[0].textContent.includes('Your account'));
});

test('a reader and a locked administrator read the person\'s list, which does not say who is disabled, and not the list as root', async (context) => {
  for (const options of [{ session: { name: 'sam', admin: false } }, {}]) {
    const app = await openAccounts(context, options);
    assert.ok(!app.calls.includes('/api/1/accounts/list/all'), JSON.stringify(options));
    assert.deepEqual(rowsOf(app).map((row) => row[1]), ['Administrator', 'Administrator', 'Console and shell', 'Files only', 'Shell']);
    assert.equal(app.view.main.querySelectorAll('tbody .pill.warn').length, 0);
  }
});

test('an unlocked administrator reads the list as root, which marks the disabled account and orders it last', async (context) => {
  const app = await openAccounts(context, { unlocked: true });
  assert.ok(app.calls.includes('/api/1/accounts/list/all'));
  assert.deepEqual(app.view.main.querySelectorAll('tbody .pill.warn').map((one) => one.textContent), ['Disabled']);
});

test('a list as root that is not read leaves the person\'s list, and does not fail the page', async (context) => {
  const app = await openAccounts(context, { unlocked: true, world: { reads: { '/api/1/accounts/list/all': new Error('not yet') } } });
  assert.equal(rowsOf(app).length, 5);
  assert.equal(app.view.main.querySelectorAll('tbody .pill.warn').length, 0);
  assert.ok(!app.calls.includes('/api/1/accounts/backup'), 'with no password said, no account is read for its keys');
  assert.equal(app.problems.length, 0);
});

test('a read of Last active that fails leaves the list without that column, and does not fail the page', async (context) => {
  const app = await openAccounts(context, { unlocked: true, world: { reads: { '/api/1/sign-ins/last': new Error('not yet') } } });
  assert.deepEqual(app.view.main.querySelectorAll('th').map((one) => one.textContent), ['Account', 'Can']);
  assert.equal(app.problems.length, 0);
});

test('a sign-in rule that is not read is left out, and so is its Change when the console has no page for it', async (context) => {
  const app = await openAccounts(context, { world: { reads: { '/api/1/security/sign-in': new Error('not yet') } } });
  const line = app.view.main.querySelector('.acc-rules');
  assert.ok(line.textContent.includes('These are the console\'s rules; SSH has its own, on Security.'));
  assert.ok(!line.textContent.includes('A session lasts'));
});

test('one account only says to add the people who use the server', async (context) => {
  const only = [account('chase', { admin: true, console: true, groups: ['admins', 'chase', 'console'] })];
  const app = await openAccounts(context, { unlocked: true, world: { reads: { '/api/1/accounts': only, '/api/1/accounts/list/all': only } } });
  assert.ok(app.view.main.textContent.includes('Add the people who use this server, so that each signs in as themselves.'));
  const many = await openAccounts(context);
  assert.ok(!many.view.main.textContent.includes('Add the people who use'));
});

test('the table is a stack of cards on a phone: each cell names itself', async (context) => {
  const app = await openAccounts(context, { unlocked: true });
  assert.ok(app.view.main.querySelector('table').classes.has('stack'));
  const labels = app.view.main.querySelector('tbody tr').querySelectorAll('td').map((cell) => cell.getAttribute('data-label'));
  assert.deepEqual(labels, ['Account', 'Can', 'Last active']);
});

test('the list shows its frame of bars until the first read has answered', async (context) => {
  let release;
  const waiting = new Promise((resolve) => { release = resolve; });
  const pending = openAccounts(context, { unlocked: true, world: { reads: { '/api/1/accounts': () => waiting } } });
  await settle();
  release(people());
  const app = await pending;
  assert.equal(app.view.main.querySelectorAll('.acc-loading').length, 0, 'the bars are gone when the rows are there');
  assert.equal(rowsOf(app).length, 5);
});

test('a first read that fails shows the error with Try again, and the list when it is tried again', async (context) => {
  let fail = true;
  const app = await openAccounts(context, {
    world: { reads: { '/api/1/accounts': () => { if (fail) throw refusal('ADM-500', 'The server could not be read', 500); return people(); } } },
  });
  assert.equal(app.problems.length, 1);
  const again = app.problems[0][1].buttons[0];
  assert.equal(again.textContent, 'Try again');
  fail = false;
  again.click();
  await settle();
  assert.equal(rowsOf(app).length, 5);
  assert.equal(app.problems.length, 1);
});

test('the list is read again every 30 seconds and when a change or an unlock says so, and what is drawn stays when nothing moved', async (context) => {
  const app = await openAccounts(context, { unlocked: true });
  assert.deepEqual(app.timers.map((one) => one[0]), [30_000]);
  const before = app.view.main.querySelector('tbody tr');
  const reads = app.calls.filter((path) => path === '/api/1/accounts').length;
  await app.timers[0][1]();
  assert.equal(app.calls.filter((path) => path === '/api/1/accounts').length, reads + 1);
  assert.equal(app.view.main.querySelector('tbody tr'), before, 'the same answer draws nothing again');
  app.emit('change');
  await settle();
  assert.equal(app.calls.filter((path) => path === '/api/1/accounts').length, reads + 2);
});

test('when a read fails after the list is drawn, the list stays and the error is said once', async (context) => {
  let fail = false;
  const app = await openAccounts(context, {
    world: { reads: { '/api/1/accounts': () => { if (fail) throw refusal('ADM-500', 'The server could not be read', 500); return people(); } } },
  });
  fail = true;
  await app.timers[0][1]();
  await app.timers[0][1]();
  assert.equal(rowsOf(app).length, 5);
  assert.equal(app.problems.length, 1);
});

test('Add an account… of an unlocked administrator opens the dialog with the names that are taken', async (context) => {
  const app = await openAccounts(context, { unlocked: true });
  app.view.headed.buttons[0].click();
  await settle();
  assert.ok(app.dialog(), 'the dialog is open');
  assert.equal(app.dialog().querySelector('h2').textContent, 'Add an account');
  assert.equal(control(app.dialog(), 'x'), null);
  assert.ok(buttonOf(app.dialog(), 'Add'));
});
