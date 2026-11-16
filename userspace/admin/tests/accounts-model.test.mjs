// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// pages/accounts/model.js and typed-model.js (design 7): which view an address and a caller give,
// the group chips and the order of the list, the sentence about the sign-in
// rules, who may end what, the guard of a key that is an account's only way in,
// the checks of what is typed, and the calls that the pages make, over fixtures
// of the JSON of design 11.7 and 13.3.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  accountHref, disableCall, endCall, enableCall, groupChips, keyAddCall, keyCount, keyRemoveCall, listRows, mayEnd, nameCall,
  nameOf, onlyWayIn, ordered, passwordCall, removeCall, rightsCall, ruleWords, SELF, showPasswordArgs, standingOf,
  stepsResetCall, tokenRevokeCall, unlockAsks, viewOf,
} from '../pages/accounts/model.js';
import {
  addArgs, fullNameProblem, HOME_CHOICES, keyLineProblem, nameProblem, showAddArgs,
} from '../pages/accounts/typed-model.js';
import {
  account, keyLine, key, lastlog, NOW, people, rules, shown,
} from './accounts-fixtures.mjs';

// ---- who looks, and at what -------------------------------------------------

test('an address names the account it opens, and the list has none', () => {
  assert.equal(nameOf(''), null);
  assert.equal(nameOf('chase'), 'chase');
  assert.equal(nameOf('chase/'), 'chase');
  assert.equal(nameOf('a%2Eb'), 'a.b');
  assert.equal(nameOf('%E0%A4'), '%E0%A4', 'an address that is not text is left as it is');
  assert.equal(accountHref('chase'), '/accounts/chase');
  assert.equal(accountHref('a b'), '/accounts/a%20b');
});

test('the standing is the person\'s, and the view is the account\'s for the one who looks', () => {
  assert.equal(standingOf({ admin: false }, true), 'reader');
  assert.equal(standingOf({ admin: true }, false), 'locked');
  assert.equal(standingOf({ admin: true }, true), 'open');
  for (const standing of ['reader', 'locked', 'open']) assert.equal(viewOf({ own: true, standing }), 'own', `own account, ${standing}`);
  assert.equal(viewOf({ own: false, standing: 'open' }), 'full');
  assert.equal(viewOf({ own: false, standing: 'locked' }), 'locked');
  assert.equal(viewOf({ own: false, standing: 'reader' }), 'reader');
});

// ---- the list --------------------------------------------------------------------

test('the groups are chips with their counts, the ones that say what an account can do first, and an account\'s own group left out', () => {
  assert.deepEqual(groupChips(people()), [
    { name: 'admins', count: 2 },
    { name: 'console', count: 3 },
    { name: 'photos', count: 1 },
  ]);
  assert.deepEqual(groupChips([]), []);
  const shared = [account('a', { groups: ['a', 'team'] }), account('b', { groups: ['b', 'team', 'a'] })];
  assert.deepEqual(groupChips(shared), [{ name: 'a', count: 2 }, { name: 'team', count: 2 }], 'a group named as an account but with more in it is a group');
});

test('the disabled accounts come last, and nothing else moves', () => {
  assert.deepEqual(ordered([account('x', { disabled: true }), account('b'), account('a', { disabled: { at_ms: 1 } }), account('c')]).map((one) => one.name), ['b', 'c', 'x', 'a']);
});

test('a row has what the account can do, and when it was last active only where the person may know', () => {
  const last = new Map(lastlog().map((one) => [one.name, one]));
  const zone = 'UTC';
  const rows = listRows({
    accounts: people(), last, keys: new Map([['backup', 1]]), self: 'chase', zone, now: NOW,
  });
  assert.deepEqual(rows.map((row) => [row.account.name, row.can, row.last, row.own, row.disabled]), [
    ['chase', 'Administrator', 'Now, this browser', true, false],
    ['alice', 'Administrator', 'Yesterday, SSH', false, false],
    ['sam', 'Console and shell', 'Nov 8, file sharing', false, false],
    ['backup', 'SSH key only', 'Never', false, false],
    ['old', 'Disabled', 'Never', false, true],
  ]);
  const without = listRows({
    accounts: people(), self: 'chase', zone, now: NOW,
  });
  assert.ok(without.every((row) => row.last === null), 'no Last active without the reads of an administrator');
  assert.equal(without.find((row) => row.account.name === 'backup').can, 'No password', 'keys not read: not claimed');
});

test('the sign-in rules are one sentence from admin settings, and nothing when the answer does not say', () => {
  assert.equal(ruleWords(rules()), 'Administrators give a second step. A session lasts 12 hours.');
  assert.equal(ruleWords({ sign_in: { second_step: 'everyone', session_hours: 1 } }), 'Everyone gives a second step. A session lasts 1 hour.');
  assert.equal(ruleWords({ sign_in: { second_step: 'if-set', session_hours: 8 } }), 'Anyone who has a second step gives it. A session lasts 8 hours.');
  assert.equal(ruleWords({ sign_in: { second_step: 'never', session_hours: 8 } }), 'A session lasts 8 hours.');
  assert.equal(ruleWords(null), null);
  assert.equal(ruleWords({ sign_in: {} }), null);
});

test('the keys that the machine can use are the ones without options, and none is counted when nothing was read', () => {
  assert.equal(keyCount(shown('backup')), 2);
  assert.equal(keyCount({ keys: [key('a', 'aa'), key('b', 'bb', { options: true })] }), 1);
  assert.equal(keyCount({ keys: [] }), 0);
  assert.equal(keyCount(null), null);
});

// ---- sessions --------------------------------------------------------------------

test('End is offered by who the person is: their own other sessions and terminals, and anything of another account to an administrator', () => {
  const console_ = { scope: 'console', id: 's-2', this: false };
  const mine = { scope: 'console', id: 's-1', this: true };
  const terminal = { scope: 'terminal', id: 3, this: false };
  const machine = { scope: 'machine', id: '4123-1763110000', this: false };
  assert.deepEqual([console_, terminal, machine, mine].map((row) => mayEnd(row, 'own')), [true, true, false, false], 'a session at the machine is an administrator\'s to end, never of the account in use');
  assert.deepEqual([console_, terminal, machine, mine].map((row) => mayEnd(row, 'full')), [true, true, true, false], 'and the session in use is Sign out\'s');
  for (const view of ['locked', 'reader']) assert.deepEqual([console_, terminal, machine].map((row) => mayEnd(row, view)), [false, false, false]);
});

test('End asks the route of what it ends: admind\'s for a console session and a terminal, the action for a session at the machine', () => {
  assert.deepEqual(endCall('sam', { scope: 'console', id: 's-2' }), { path: '/api/1/accounts/sam/sessions/s-2/end' });
  assert.deepEqual(endCall('sam', { scope: 'terminal', id: 3 }), { path: '/api/1/terminals/3/end' });
  assert.deepEqual(endCall('sam', { scope: 'machine', id: '4123-1763110000' }), { action: 'accounts.end', args: { name: 'sam', session: '4123-1763110000' } });
  assert.deepEqual(endCall('a b', { scope: 'console', id: 'x/y' }), { path: '/api/1/accounts/a%20b/sessions/x%2Fy/end' });
});

// ---- keys ------------------------------------------------------------------------

test('a key line that authd reads is one line of Ed25519 or ECDSA with its blob whole', () => {
  for (const type of ['ssh-ed25519', 'ecdsa-sha2-nistp256', 'ecdsa-sha2-nistp384']) assert.equal(keyLineProblem(keyLine(type)), '', type);
  assert.equal(keyLineProblem(`  ${keyLine()}\n`), '', 'the spaces around it are not part of it');
  assert.equal(keyLineProblem(keyLine('ssh-ed25519', '')), '', 'a key may have no comment');
});

test('a key line that authd would pass over or refuse is told why, before the action is asked', () => {
  assert.match(keyLineProblem(''), /Paste the key/);
  assert.match(keyLineProblem(`${keyLine()}\n${keyLine()}`), /one key, on one line/);
  assert.match(keyLineProblem(keyLine('ssh-rsa')), /not ssh-rsa/);
  assert.match(keyLineProblem(keyLine('ssh-dss')), /Ed25519 and ECDSA keys/);
  assert.match(keyLineProblem(`command="x" ${keyLine()}`), /options before the key/);
  assert.match(keyLineProblem('ssh-ed25519'), /not whole/);
  assert.match(keyLineProblem('ssh-ed25519 AAAA'), /not whole/);
  assert.match(keyLineProblem('ssh-ed25519 not-base64!! laptop'), /not whole/);
  assert.match(keyLineProblem(keyLine('ecdsa-sha2-nistp256').replace('ecdsa-sha2-nistp256', 'ssh-ed25519')), /not whole/, 'a blob of another type than the line says');
  assert.match(keyLineProblem(`ssh-ed25519 ${'A'.repeat(20_000)}`), /longer than a key/);
});

test('a key is the only way in when it is the last that can be used and nothing else lets the account sign in', () => {
  const lone = [key('laptop', 'aa')];
  const sam = account('sam', { password: 'set' });
  const backup = account('backup', { password: 'none' });
  assert.equal(onlyWayIn({ account: backup, keys: lone, key: lone[0] }), true, 'no password');
  assert.equal(onlyWayIn({ account: sam, keys: lone, key: lone[0], sshPasswords: false }), true, 'SSH takes no passwords');
  assert.equal(onlyWayIn({ account: sam, keys: lone, key: lone[0], sshPasswords: true }), false);
  assert.equal(onlyWayIn({ account: sam, keys: lone, key: lone[0] }), false, 'not known is not claimed');
  assert.equal(onlyWayIn({ account: { ...backup, console: true }, keys: lone, key: lone[0] }), false, 'it has the console');
  assert.equal(onlyWayIn({ account: { ...backup, admin: true }, keys: lone, key: lone[0] }), false, 'an administrator has the console');
  const two = [key('laptop', 'aa'), key('phone', 'bb')];
  assert.equal(onlyWayIn({ account: backup, keys: two, key: two[0] }), false, 'another key is left');
  const optioned = [key('laptop', 'aa'), key('old', 'bb', { options: true })];
  assert.equal(onlyWayIn({ account: backup, keys: optioned, key: optioned[0] }), true, 'a line with options is no way in');
  assert.equal(onlyWayIn({ account: backup, keys: optioned, key: optioned[1] }), false, 'and removing it takes nothing away');
});

// ---- what is typed -----------------------------------------------------------------

test('a name is checked as the command checks it, and as taken', () => {
  assert.equal(nameProblem('sam', ['chase']), '');
  assert.equal(nameProblem('a.b-c_9'), '');
  assert.equal(nameProblem(''), 'A name needs a letter at least.');
  assert.match(nameProblem('Sam'), /lowercase letters/);
  assert.match(nameProblem('9lives'), /starting with a letter/);
  assert.match(nameProblem('a b'), /lowercase letters/);
  assert.match(nameProblem('x'.repeat(33)), /at most 32/);
  assert.equal(nameProblem('x'.repeat(32)), '');
  assert.match(nameProblem('help'), /kept for the helpers/);
  assert.equal(nameProblem('chase', ['chase']), 'An account is already called chase.');
});

test('a full name holds no colon or line break and at most 256 bytes', () => {
  assert.equal(fullNameProblem(''), '');
  assert.equal(fullNameProblem('Alice Liddell'), '');
  assert.match(fullNameProblem('a:b'), /colon/);
  assert.match(fullNameProblem('a\nb'), /line break/);
  assert.equal(fullNameProblem('é'.repeat(128)), '');
  assert.match(fullNameProblem('é'.repeat(129)), /256 bytes/);
});

// ---- the calls ------------------------------------------------------------------------

test('Add an account asks for what was typed and nothing that was not, with the password in the arguments here and nowhere else', () => {
  assert.deepEqual(addArgs({ name: 'sam' }), { name: 'sam' });
  assert.deepEqual(addArgs({
    name: 'sam', fullName: 'Sam', admin: true, password: 'p1',
  }), {
    name: 'sam', full_name: 'Sam', admin: true, password: 'p1',
  });
  assert.deepEqual(addArgs({ name: 'sam', admin: false, password: '' }), { name: 'sam' }, 'empty means none yet, and is not sent');
});

test('the command shown for Add an account says that a password is typed, and carries none', () => {
  assert.deepEqual(showAddArgs({ name: 'sam', hasPassword: true }), { name: 'sam', password: '' });
  assert.deepEqual(showAddArgs({ name: 'sam', hasPassword: false, admin: true }), { name: 'sam', admin: true });
});

test('a change is the person\'s own action for the person\'s own account and an administrator\'s for another\'s', () => {
  assert.deepEqual(nameCall({ own: true, name: 'chase', fullName: 'C' }), { action: 'account.name', args: { full_name: 'C' } });
  assert.deepEqual(nameCall({ own: false, name: 'sam', fullName: 'S' }), { action: 'accounts.set', args: { name: 'sam', full_name: 'S' } });
  assert.deepEqual(passwordCall({ own: true, name: 'chase', current: 'old', password: 'new' }), { action: 'account.password', args: { current: 'old', new: 'new' } });
  assert.deepEqual(passwordCall({ own: false, name: 'sam', password: 'new' }), { action: 'accounts.password', args: { name: 'sam', password: 'new' } });
  assert.deepEqual(keyAddCall({ own: true, name: 'chase', line: 'L' }), { action: 'account.key-add', args: { line: 'L' } });
  assert.deepEqual(keyAddCall({ own: false, name: 'sam', line: 'L' }), { action: 'accounts.key-add', args: { name: 'sam', line: 'L' } });
  assert.deepEqual(keyRemoveCall({ own: true, name: 'chase', fingerprint: 'F' }), { action: 'account.key-remove', args: { fingerprint: 'F' } });
  assert.deepEqual(keyRemoveCall({ own: false, name: 'sam', fingerprint: 'F' }), { action: 'accounts.key-remove', args: { name: 'sam', fingerprint: 'F' } });
});

test('the other calls name the account, and only the right to administer is asked of an administrator', () => {
  assert.deepEqual(rightsCall({ name: 'sam', admin: true }), { action: 'accounts.set', args: { name: 'sam', admin: true } });
  assert.deepEqual(rightsCall({ name: 'sam', console: false }), { action: 'accounts.set', args: { name: 'sam', console: false } });
  assert.deepEqual(tokenRevokeCall({ name: 'sam', token: 'ansible' }), { action: 'accounts.token-revoke', args: { name: 'sam', token: 'ansible' } });
  assert.deepEqual(stepsResetCall({ name: 'sam' }), { action: 'accounts.steps-reset', args: { name: 'sam' } });
  assert.deepEqual(disableCall({ name: 'sam' }), { action: 'accounts.disable', args: { name: 'sam' } });
  assert.deepEqual(enableCall({ name: 'sam' }), { action: 'accounts.enable', args: { name: 'sam' } });
  assert.deepEqual(removeCall({ name: 'sam', home: 'archive' }), { action: 'accounts.remove', args: { name: 'sam', home: 'archive' } });
});

test('the command shown for a password change is sent no password: a required secret is shown as typed when asked', () => {
  assert.deepEqual(showPasswordArgs({ own: true, name: 'chase' }), { action: 'account.password', args: {} });
  assert.deepEqual(showPasswordArgs({ own: false, name: 'sam' }), { action: 'accounts.password', args: { name: 'sam' } });
});

test('removing an account offers the three things that can become of the home folder, the first as the default', () => {
  assert.deepEqual(HOME_CHOICES.map((choice) => choice.value), ['keep', 'archive', 'delete']);
  assert.equal(HOME_CHOICES[0].label('sam'), 'Keep it in /users/sam');
  assert.ok(HOME_CHOICES.every((choice) => choice.words.length > 20));
});

test('what the account in use may not do is said with where it is done instead', () => {
  assert.match(SELF.words, /another administrator's account/);
  assert.equal(SELF.command, 'elevate user …');
});

test('"Unlock asks" says what the unlock dialog asks for, from the second steps', () => {
  assert.equal(unlockAsks({ app: true, passkeys: [{ id: 'a' }] }), 'for a tap on a passkey, or the code from the app');
  assert.equal(unlockAsks({ app: true, passkeys: [] }), 'for the code from the app');
  assert.equal(unlockAsks({ app: false, passkeys: [{ id: 'a' }] }), 'for a tap on a passkey');
  assert.equal(unlockAsks({ app: false, passkeys: [] }), 'for your password');
  assert.equal(unlockAsks(null), 'for your password');
});
