// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One account (design 7.3, 7.5, 7.6 and 7.8) and Your account, drawn on a page
// of the test's own: what each kind of person who looks is shown (a reader, a
// locked administrator, an unlocked one, the person at their own account), the
// rows and what each control asks, the guards, and the states. The actions
// are answered by the view as the registry of design 13.2 would; which
// answers are real waits on the registry's accounts actions.

import assert from 'node:assert/strict';
import test from 'node:test';

import { PAGES } from '../pages.js';
import {
  fingerprint, key, keyLine, shown,
} from './accounts-fixtures.mjs';
import {
  buttonOf, control, holdings, openAccounts, refusal, settle,
} from './accounts-view.mjs';

/** The words of the row `label` of the account's list. */
function rowOf(app, label) {
  const list = app.view.main.querySelector('dl');
  const at = list.children.findIndex((one) => one.tag === 'dt' && one.textContent === label);
  return at < 0 ? null : list.children[at + 1];
}

/** Types `text` in `input`. */
function type(input, text) {
  input.value = text;
  input.fire('input');
}

// ---- who sees what ------------------------------------------------------------------

test('a reader opening another account sees its row and that administrators see the rest, and reads nothing more', async (context) => {
  const app = await openAccounts(context, { rest: 'alice', session: { name: 'sam', admin: false } });
  assert.deepEqual(app.calls, ['/api/1/accounts']);
  const text = app.view.main.textContent;
  assert.ok(text.includes('alice'));
  assert.ok(text.includes('Administrator · Alice Liddell'));
  assert.ok(text.includes('Administrators see the rest.'));
  assert.equal(app.view.main.querySelectorAll('dl').length, 0);
  assert.equal(app.view.main.querySelectorAll('button').length, 0);
});

test('a locked administrator sees the account\'s row and is asked to unlock to see the rest', async (context) => {
  const app = await openAccounts(context, { rest: 'sam' });
  assert.deepEqual(app.calls, ['/api/1/accounts']);
  assert.ok(app.view.main.textContent.includes('Unlock to see sam\'s keys, second steps, tokens and sessions.'));
  assert.ok(buttonOf(app.view.main, 'Unlock'));
  assert.equal(app.view.main.querySelectorAll('dl').length, 0);
});

test('an unlocked administrator sees another account whole, under the Accounts crumb', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  assert.deepEqual(app.view.headed, { title: 'sam', crumbs: [{ title: 'Accounts', path: '/accounts' }], commands: true });
  assert.ok(app.calls.includes('/api/1/accounts/sam'));
  assert.ok(app.calls.includes('/api/1/accounts/sam/sessions'));
  assert.ok(!app.calls.includes('/api/1/account/show'));
  assert.ok(app.view.main.textContent.includes('Account 1002 · since Oct 19'));
  assert.deepEqual(app.view.main.querySelectorAll('dt').map((one) => one.textContent), [
    'Full name', 'Groups', 'Administrator', 'Console', 'Password', 'Second step', 'Shell',
  ]);
  for (const id of ['keys', 'tokens', 'signed-in', 'danger']) assert.ok(app.view.main.querySelector(`#${id}`), id);
});

test('the person at their own account sees it whole whatever their standing, from the person\'s own read', async (context) => {
  for (const [session, unlocked] of [[{ name: 'chase', admin: true }, false], [{ name: 'chase', admin: true }, true], [{ name: 'sam', admin: false }, false]]) {
    const app = await openAccounts(context, { route: 'account', session, unlocked });
    assert.ok(app.calls.includes('/api/1/account/show'), `${session.name} unlocked=${unlocked}`);
    assert.ok(!app.calls.includes('/api/1/sign-ins/last'));
    assert.deepEqual(app.view.headed, { title: 'Your account', crumbs: [], commands: true });
    assert.ok(app.view.main.querySelector('dl'));
    for (const id of ['display', 'zone', 'terminal-settings', 'second-step', 'tokens']) assert.ok(app.view.main.querySelector(`#${id}`), `#${id}`);
  }
});

test('/accounts and the person\'s own name show the same page as /account', async (context) => {
  const app = await openAccounts(context, { rest: 'chase' });
  assert.equal(app.view.headed.title, 'Your account');
  assert.ok(app.calls.includes('/api/1/account/show'));
  assert.ok(app.view.main.querySelector('#display'));
});

test('Your account draws the Accounts rows, so its entry names the Accounts sheet', () => {
  const sheets = (id) => PAGES.find((entry) => entry.id === id).css;
  assert.ok(sheets('account').includes('pages/accounts/acc.css'));
  assert.ok(sheets('accounts').includes('pages/accounts/acc.css'));
});

test('an account that is not there says so with the way back, whether admind says so or the list does not have it', async (context) => {
  const gone = refusal('ACC-101', 'There is no account called ghost', 404);
  const app = await openAccounts(context, { rest: 'ghost', unlocked: true, world: { reads: { '/api/1/accounts/ghost': gone } } });
  assert.ok(app.view.main.textContent.includes('There is no account called ghost.'));
  assert.equal(app.view.main.querySelector('a').getAttribute('href'), '/accounts');
  assert.equal(app.problems.length, 0, 'an unknown account is a state, not an error');
  const locked = await openAccounts(context, { rest: 'ghost' });
  assert.ok(locked.view.main.textContent.includes('There is no account called ghost.'));
});

test('another error is the problem view with Try again, and the page is read again when it is pressed', async (context) => {
  let fail = true;
  const app = await openAccounts(context, {
    rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': () => { if (fail) throw refusal('ADM-500', 'Could not read', 500); return shown('sam'); } } },
  });
  assert.equal(app.problems.length, 1);
  fail = false;
  app.problems[0][1].buttons[0].click();
  await settle();
  assert.ok(app.view.main.querySelector('dl'));
});

// ---- the rows ---------------------------------------------------------------------

test('the rows say the full name, the groups, the password, the second step and the shell as words', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  assert.ok(rowOf(app, 'Full name').textContent.startsWith('Sam'));
  assert.deepEqual(rowOf(app, 'Groups').querySelectorAll('.acc-chip').map((one) => one.textContent), ['sam', 'console', 'photos']);
  assert.ok(rowOf(app, 'Password').textContent.startsWith('Set, changed Oct 19'));
  assert.ok(rowOf(app, 'Password').textContent.includes('Never expires. Dukovo never forces a change.'));
  const steps = rowOf(app, 'Second step').textContent;
  assert.ok(steps.startsWith('Authenticator app and 1 passkey'));
  assert.ok(steps.includes('8 of 10 recovery codes left · 2 trusted browsers'));
  assert.equal(rowOf(app, 'Shell').textContent, '/system/commands/nukesh');
});

test('an account with no second step says None and offers nothing to remove', async (context) => {
  const none = shown('sam', { steps: { app: false, passkeys: [], recovery_left: 0, trusted_browsers: 0 } });
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': none } } });
  assert.equal(rowOf(app, 'Second step').textContent, 'None');
  assert.equal(control(app.view.main, 'steps-reset'), null);
});

test('a disabled account says so and offers Enable, and Enable asks nothing but the action', async (context) => {
  const app = await openAccounts(context, { rest: 'old', unlocked: true });
  assert.ok(app.view.main.textContent.includes('old is disabled: nothing signs in to the account.'));
  assert.equal(control(app.view.main, 'disable'), null);
  control(app.view.main, 'enable').click();
  await settle();
  assert.deepEqual(app.ran('accounts.enable'), [{ name: 'old' }]);
  assert.equal(app.dialog(), null);
});

test('the account\'s own page is read again after a change, and the sessions every ten seconds', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  assert.deepEqual(app.timers.map((one) => one[0]), [10_000]);
  const before = app.calls.filter((path) => path === '/api/1/accounts/sam').length;
  app.emit('change');
  await settle();
  assert.equal(app.calls.filter((path) => path === '/api/1/accounts/sam').length, before + 1);
});

test('a read after a change keeps what an action showed in the page\'s slot, and takes away only a failed read\'s words', async (context) => {
  let fail = false;
  const app = await openAccounts(context, {
    rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': () => { if (fail) throw refusal('ADM-500', 'Could not read', 500); return shown('sam'); } } },
  });
  // Reads after a change come at most every two seconds.
  context.mock.timers.enable({ apis: ['setTimeout', 'Date'] });
  const again = async () => {
    app.emit('change');
    context.mock.timers.tick(2000);
    await settle();
  };
  const words = app.document.createElement('p');
  words.textContent = 'Too many tries.';
  app.view.slot.append(words);
  await again();
  assert.equal(app.view.slot.textContent, 'Too many tries.');
  fail = true;
  await again();
  assert.equal(app.problems.length, 1);
  fail = false;
  await again();
  assert.equal(app.view.slot.children.length, 0);
});

// ---- the switches --------------------------------------------------------------------

test('the console switch of an account that is not an administrator changes it, and an administrator\'s is on and cannot', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const toggle = control(app.view.main, 'console');
  assert.equal(toggle.getAttribute('role'), 'switch');
  assert.equal(toggle.getAttribute('aria-checked'), 'true');
  toggle.click();
  await settle();
  assert.deepEqual(app.ran('accounts.set'), [{ name: 'sam', console: false }]);

  const alice = await openAccounts(context, { rest: 'alice', unlocked: true });
  const fixed = control(alice.view.main, 'console');
  assert.equal(fixed.getAttribute('aria-checked'), 'true');
  assert.equal(fixed.getAttribute('aria-disabled'), 'true');
  assert.ok(alice.view.main.textContent.includes('An administrator can always sign in to Dukovo Admin.'));
  assert.ok(fixed.getAttribute('aria-describedby'), 'the switch is described by the sentence');
  fixed.click();
  await settle();
  assert.equal(alice.ran('accounts.set').length, 0);
});

test('making an administrator asks first and says what it gives, and runs accounts.set when it is said yes', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const toggle = control(app.view.main, 'admin');
  assert.equal(toggle.getAttribute('aria-checked'), 'false');
  toggle.click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Make sam an administrator?');
  assert.equal(app.ran('accounts.set').length, 0, 'nothing runs before it is said yes');
  await app.submit();
  assert.deepEqual(app.ran('accounts.set'), [{ name: 'sam', admin: true }]);
  assert.equal(app.dialog(), null);
});

test('taking the right away from another administrator asks first, and the last administrator\'s refusal is shown in the dialog', async (context) => {
  const last = refusal('ACC-102', 'That would leave no administrator', 409);
  const app = await openAccounts(context, { rest: 'alice', unlocked: true, world: { answers: { 'accounts.set': last } } });
  control(app.view.main, 'admin').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Take away alice\'s administrator right?');
  await app.submit();
  assert.deepEqual(app.ran('accounts.set'), [{ name: 'alice', admin: false }]);
  assert.ok(app.dialog(), 'the dialog stays with the refusal');
  assert.equal(app.problems[0][0], last);
  assert.ok(app.problems[0][1].slot, 'authd\'s words go into the dialog');
});

test('on the account in use there is no switch: the console says where to do it instead', async (context) => {
  const app = await openAccounts(context, { route: 'account', unlocked: true });
  assert.equal(control(app.view.main, 'admin'), null);
  assert.equal(control(app.view.main, 'console'), null);
  assert.equal(control(app.view.main, 'disable'), null);
  assert.equal(control(app.view.main, 'remove'), null);
  assert.equal(rowOf(app, 'Console').textContent, 'Can sign in to Dukovo Admin');
  const text = app.view.main.textContent;
  assert.ok(text.includes('Do this from another administrator\'s account, or at a terminal with elevate user …'));
  assert.ok(text.includes('The console does not disable or remove the account you are signed in with.'));
  assert.ok(app.view.main.querySelector('#danger').querySelector('code'));
});

test('a person who is not an administrator is told nothing of administrators\' rights, and has no section for them', async (context) => {
  const app = await openAccounts(context, { route: 'account', session: { name: 'sam', admin: false } });
  assert.equal(app.view.main.querySelector('#danger'), null);
  assert.equal(rowOf(app, 'Administrator').textContent, 'No');
  assert.equal(app.view.main.querySelectorAll('dt').filter((one) => one.textContent === 'Unlock asks').length, 0);
});

// ---- the full name ---------------------------------------------------------------------

test('the full name is changed from a dialog that starts with what it is, by the person\'s own action for their own account', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(app.view.main, 'full-name').click();
  await settle();
  const input = app.dialog().querySelector('input');
  assert.equal(input.getAttribute('value'), 'Sam');
  type(input, 'Sam Two');
  await app.submit();
  assert.deepEqual(app.ran('accounts.set'), [{ name: 'sam', full_name: 'Sam Two' }]);

  const own = await openAccounts(context, { route: 'account', unlocked: true });
  control(own.view.main, 'full-name').click();
  await settle();
  type(own.dialog().querySelector('input'), 'Chase C');
  await own.submit();
  assert.deepEqual(own.ran('account.name'), [{ full_name: 'Chase C' }]);
});

test('a full name with a colon is refused in the dialog, and the command is not asked', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(app.view.main, 'full-name').click();
  await settle();
  type(app.dialog().querySelector('input'), 'Sam: Two');
  await app.submit();
  assert.ok(app.dialog().textContent.includes('A full name cannot hold a colon or a line break.'));
  assert.equal(app.ran('accounts.set').length, 0);
});

// ---- the password -----------------------------------------------------------------------

const SECRET = 'old-pass-SECRET-1';
const NEXT = 'new-pass-SECRET-2';

test('another account\'s password is a new one typed twice, and they are not told it', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, prefs: { commands: 'shown' } });
  control(app.view.main, 'password').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Change the password of sam');
  assert.ok(app.dialog().textContent.includes('sam is not told the new password: give it to them yourself.'));
  const inputs = app.dialog().querySelectorAll('input');
  assert.equal(inputs.length, 2, 'the new password and again');
  assert.deepEqual(app.panels.at(-1), ['accounts.password', { name: 'sam' }], 'the command shown carries no password');
  type(inputs[0], NEXT);
  type(inputs[1], NEXT);
  await app.submit();
  assert.deepEqual(app.ran('accounts.password'), [{ name: 'sam', password: NEXT }]);
  assert.equal(holdings(app.document.body).filter((one) => one.includes(NEXT)).length, 0);
  assert.equal(JSON.stringify([app.panels, app.calls, app.posts, app.notices, app.went]).includes(NEXT), false);
});

test('the person\'s own password asks for the one they have, and sends both in the one call', async (context) => {
  const app = await openAccounts(context, { route: 'account', session: { name: 'sam', admin: false }, prefs: { commands: 'shown' } });
  control(app.view.main, 'password').click();
  await settle();
  const inputs = app.dialog().querySelectorAll('input');
  assert.equal(inputs.length, 3, 'the current password, the new one and again');
  assert.equal(inputs[0].getAttribute('autocomplete'), 'current-password');
  assert.deepEqual(app.panels.at(-1), ['account.password', {}], 'both are typed when asked');
  type(inputs[0], SECRET);
  type(inputs[1], NEXT);
  type(inputs[2], 'not the same');
  await app.submit();
  assert.equal(app.ran('account.password').length, 0, 'two that differ ask nothing');
  assert.equal(inputs[0].value, SECRET, 'and the one they have stays typed');
  type(inputs[2], NEXT);
  await app.submit();
  assert.deepEqual(app.ran('account.password'), [{ current: SECRET, new: NEXT }], 'the one they have is sent as typed, with the new one');
  assert.equal(holdings(app.document.body).filter((one) => one.includes(NEXT)).length, 0);
});

test('a wrong current password is authd\'s refusal, shown in the dialog, which stays', async (context) => {
  const wrong = refusal('ADM-123', 'The password is not right', 403);
  const app = await openAccounts(context, { route: 'account', world: { answers: { 'account.password': wrong } } });
  control(app.view.main, 'password').click();
  await settle();
  const inputs = app.dialog().querySelectorAll('input');
  type(inputs[0], 'wrong');
  type(inputs[1], NEXT);
  type(inputs[2], NEXT);
  await app.submit();
  assert.ok(app.dialog());
  assert.equal(app.problems[0][0], wrong);
  assert.equal(inputs[1].value, '', 'what was typed is gone, and is typed again');
});

// ---- the second step -----------------------------------------------------------------------

test('Remove second steps… of another account says what goes and that they are told, and runs accounts.steps-reset', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(app.view.main, 'steps-reset').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Remove sam\'s second steps?');
  const words = app.dialog().textContent;
  assert.ok(words.includes('Use this when sam has lost the phone or key.'));
  assert.ok(words.includes('Their app, passkeys, recovery codes and trusted browsers are removed.'));
  assert.ok(words.includes('they add a new one before anything else'));
  assert.equal(app.ran('accounts.steps-reset').length, 0);
  await app.submit();
  assert.deepEqual(app.ran('accounts.steps-reset'), [{ name: 'sam' }]);
});

test('the person\'s own second step is managed on C2a\'s pages, and an administrator is told what Unlock asks', async (context) => {
  const app = await openAccounts(context, { route: 'account', unlocked: true });
  const row = rowOf(app, 'Second step');
  assert.equal(row.querySelector('a').getAttribute('href'), '/second-step');
  assert.equal(control(app.view.main, 'steps-reset'), null);
  assert.ok(rowOf(app, 'Unlock asks').textContent.startsWith('for a tap on a passkey, or the code from the app'));
});

// ---- SSH keys ---------------------------------------------------------------------------------

test('each key is its name, its type and its fingerprint, short until it has the focus', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const rows = app.view.main.querySelector('#keys').querySelectorAll('tbody tr');
  assert.deepEqual(rows.map((one) => one.querySelector('b').textContent), ['laptop', 'phone']);
  const code = rows[0].querySelector('.acc-fp');
  assert.equal(code.textContent, 'SHA256:pR8x…xxx');
  assert.equal(rows[0].querySelector('small').textContent.startsWith('ED25519 · '), true);
  code.fire('focus');
  assert.equal(code.textContent, fingerprint('pR8x'));
  assert.equal(code.getAttribute('aria-label'), `Fingerprint ${fingerprint('pR8x')}`);
  code.fire('blur');
  assert.equal(code.textContent, 'SHA256:pR8x…xxx');
});

test('a key that is not the only way in is removed at once, and the account is read again', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const before = app.calls.filter((path) => path === '/api/1/accounts/sam').length;
  buttonOf(app.view.main.querySelector('#keys'), 'Remove').click();
  await settle();
  assert.equal(app.dialog(), null, 'no question for a key that leaves another');
  assert.deepEqual(app.ran('accounts.key-remove'), [{ name: 'sam', fingerprint: fingerprint('pR8x') }]);
  assert.equal(app.calls.filter((path) => path === '/api/1/accounts/sam').length, before + 1);
});

test('the only way an account signs in is removed only when its name is typed, and says so', async (context) => {
  const lone = shown('backup', { keys: [key('nas', 'Zq1w')], sessions: [] });
  const app = await openAccounts(context, { rest: 'backup', unlocked: true, world: { reads: { '/api/1/accounts/backup': lone } } });
  buttonOf(app.view.main.querySelector('#keys'), 'Remove').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Remove the key nas?');
  assert.ok(app.dialog().textContent.includes('This is the only way backup signs in: after this it cannot sign in at all.'));
  const typed = app.dialog().querySelector('input');
  await app.submit();
  assert.equal(app.ran('accounts.key-remove').length, 0, 'nothing is typed');
  assert.ok(app.dialog().textContent.includes('Type backup exactly as it is written'));
  type(typed, 'nas');
  await app.submit();
  assert.equal(app.ran('accounts.key-remove').length, 0, 'the key\'s name is not the account\'s');
  type(typed, 'backup');
  await app.submit();
  assert.deepEqual(app.ran('accounts.key-remove'), [{ name: 'backup', fingerprint: fingerprint('Zq1w') }]);
  assert.equal(app.dialog(), null);
});

test('a key that SSH passwords make no way in is removed at once, and one of an account whose passwords are off is asked about', async (context) => {
  const only = shown('sam', { keys: [key('nas', 'Zq1w')] });
  const open = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': only, '/api/1/ssh': { passwords: true } } } });
  buttonOf(open.view.main.querySelector('#keys'), 'Remove').click();
  await settle();
  assert.equal(open.dialog(), null);
  const off = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': { ...only, account: { ...only.account, console: false } }, '/api/1/ssh': { passwords: false } } } });
  buttonOf(off.view.main.querySelector('#keys'), 'Remove').click();
  await settle();
  assert.ok(off.dialog().textContent.includes('This is the only way sam signs in'));
});

test('on the person\'s own account the keys are the person\'s own actions and the words say you', async (context) => {
  const lone = shown('chase', { keys: [key('nas', 'Zq1w')] });
  const app = await openAccounts(context, { route: 'account', session: { name: 'sam', admin: false }, world: { reads: { '/api/1/account/show': { ...lone, account: { ...lone.account, name: 'sam', admin: false, console: false, password: 'none' } } } } });
  buttonOf(app.view.main.querySelector('#keys'), 'Remove').click();
  await settle();
  assert.ok(app.dialog().textContent.includes('This is the only way you sign in: after this you cannot sign in at all.'));
  type(app.dialog().querySelector('input'), 'sam');
  await app.submit();
  assert.deepEqual(app.ran('account.key-remove'), [{ fingerprint: fingerprint('Zq1w') }]);
});

test('a line with options is told as one the machine does not use', async (context) => {
  const optioned = shown('sam', { keys: [key('old', 'Op1', { options: true })] });
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, world: { reads: { '/api/1/accounts/sam': optioned } } });
  assert.ok(app.view.main.querySelector('#keys').textContent.includes('This line has options, so the machine does not use it.'));
});

test('Add a key… checks the line as authd reads it, and adds it for the account or the person', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true, prefs: { commands: 'shown' } });
  control(app.view.main, 'key-add').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Add an SSH key');
  const line = app.dialog().querySelector('input');
  type(line, '');
  await app.submit();
  assert.ok(app.dialog().textContent.includes('Paste the key'));
  type(line, `command="ls" ${keyLine()}`);
  await app.submit();
  assert.ok(app.dialog().textContent.includes('options before the key'));
  assert.equal(app.ran('accounts.key-add').length, 0);
  const good = keyLine('ecdsa-sha2-nistp256', 'phone');
  type(line, good);
  await app.submit();
  assert.deepEqual(app.ran('accounts.key-add'), [{ name: 'sam', line: good }]);
  assert.equal(app.dialog(), null);

  const own = await openAccounts(context, { route: 'account', unlocked: true });
  control(own.view.main, 'key-add').click();
  await settle();
  type(own.dialog().querySelector('input'), good);
  await own.submit();
  assert.deepEqual(own.ran('account.key-add'), [{ line: good }]);
});

// ---- tokens ------------------------------------------------------------------------------------

test('each token says what it may do and when it runs out, and an administrator does not make tokens for another account', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  const section = app.view.main.querySelector('#tokens');
  const rows = section.querySelectorAll('tbody tr').map((one) => one.textContent);
  assert.deepEqual(rows, ['ansibleEverything sam can do · expires Dec 13Revoke', 'nas-snapshotsSnapshots: read · expires Feb 1, 2027Revoke']);
  assert.ok(section.textContent.includes('A token can never do more than its owner, and every token runs out.'));
  assert.ok(!section.textContent.includes('admin token add'), 'no command to make one for another account');
  assert.equal(buttonOf(section, 'New token…'), undefined);
});

test('Revoke asks first, says that whatever uses the token stops, and then runs accounts.token-revoke', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  buttonOf(app.view.main.querySelector('#tokens'), 'Revoke').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Revoke the token ansible?');
  assert.ok(app.dialog().textContent.includes('Whatever uses it stops working.'));
  assert.equal(app.ran('accounts.token-revoke').length, 0);
  await app.submit();
  assert.deepEqual(app.ran('accounts.token-revoke'), [{ name: 'sam', token: 'ansible' }]);
});

test('the person\'s own token is revoked by C2a\'s route, and the command that makes one is shown to copy', async (context) => {
  const app = await openAccounts(context, { route: 'account', unlocked: true });
  const section = app.view.main.querySelector('#tokens');
  assert.ok(section.textContent.includes('admin token add NAME --scope services:read --days 30'));
  buttonOf(section, 'Revoke').click();
  await settle();
  await app.submit();
  assert.deepEqual(app.posts, [['/api/1/account/tokens/remove', { name: 'ansible' }]]);
  assert.equal(app.ran('accounts.token-revoke').length, 0);
});

test('a token that cannot be revoked says why in the dialog, which stays', async (context) => {
  const refused = refusal('ADM-106', 'Unlock first', 403);
  const app = await openAccounts(context, { route: 'account', world: { posts: { '/api/1/account/tokens/remove': refused } } });
  buttonOf(app.view.main.querySelector('#tokens'), 'Revoke').click();
  await settle();
  await app.submit();
  assert.ok(app.dialog());
  assert.equal(app.problems[0][0], refused);
});

// ---- disabling and removing ------------------------------------------------------------------------

test('Disable says what ends and that nothing signs in until it is enabled again, and runs accounts.disable', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(app.view.main, 'disable').click();
  await settle();
  assert.equal(app.dialog().querySelector('h2').textContent, 'Disable sam?');
  assert.ok(app.dialog().textContent.includes('Everything sam runs ends now: their sessions, terminals, SSH connections and processes.'));
  assert.ok(app.dialog().textContent.includes('Nothing signs in to the account until it is enabled again, with the same password.'));
  await app.submit();
  assert.deepEqual(app.ran('accounts.disable'), [{ name: 'sam' }]);
});

test('the destructive button is the last of the dialog and is never the one that has the focus first', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(app.view.main, 'remove').click();
  await settle();
  const foot = app.dialog().querySelector('.dialog-foot').children.map((one) => one.textContent);
  assert.deepEqual(foot, ['Cancel', 'Remove the account']);
  assert.ok(app.dialog().querySelector('.dialog-foot').children[1].classes.has('danger'));
  assert.equal(app.dialog().querySelector('.dialog-foot').children[1].hasAttribute('autofocus'), false);
});

test('Remove offers what becomes of the home folder with Keep it first, asks for the name typed, and goes back to the list', async (context) => {
  const app = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(app.view.main, 'remove').click();
  await settle();
  const cards = app.dialog().querySelectorAll('.radio-card');
  assert.deepEqual(cards.map((one) => one.querySelector('b').textContent), ['Keep it in /users/sam', 'Archive it', 'Delete it']);
  assert.deepEqual(cards.map((one) => one.classes.has('on')), [true, false, false]);
  const radios = app.dialog().querySelectorAll('input').filter((one) => one.getAttribute('type') === 'radio');
  radios[1].fire('change');
  assert.deepEqual(cards.map((one) => one.classes.has('on')), [false, true, false]);
  await app.submit();
  assert.equal(app.ran('accounts.remove').length, 0, 'the name is not typed');
  type(app.dialog().querySelectorAll('input').find((one) => one.getAttribute('type') === 'text'), 'sam');
  await app.submit();
  assert.deepEqual(app.ran('accounts.remove'), [{ name: 'sam', home: 'archive' }]);
  assert.deepEqual(app.went, ['/accounts']);
});

test('a removal that authd refuses leaves the dialog and the page where they are', async (context) => {
  const last = refusal('ACC-102', 'That would leave no administrator', 409);
  const app = await openAccounts(context, { rest: 'alice', unlocked: true, world: { answers: { 'accounts.remove': last } } });
  control(app.view.main, 'remove').click();
  await settle();
  type(app.dialog().querySelectorAll('input').find((one) => one.getAttribute('type') === 'text'), 'alice');
  await app.submit();
  assert.ok(app.dialog());
  assert.deepEqual(app.went, []);
  assert.equal(app.problems[0][0], last);
});

test('the Show command panel of a dialog comes only while commands are shown, and Remove\'s follows the home folder chosen', async (context) => {
  const hidden = await openAccounts(context, { rest: 'sam', unlocked: true });
  control(hidden.view.main, 'remove').click();
  await settle();
  assert.equal(hidden.dialog().querySelectorAll('.acc-command').length, 0);
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const shownCommands = await openAccounts(context, { rest: 'sam', unlocked: true, prefs: { commands: 'shown' } });
  control(shownCommands.view.main, 'remove').click();
  await settle();
  assert.deepEqual(shownCommands.panels.at(-1), ['accounts.remove', { name: 'sam', home: 'keep' }]);
});
