// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/account-model.js (design 3.5, 7.4, 7.5 and 7.7): what an account can do
// in the words of the list, its Last active for each kind of `terminal` that
// `lastlog` gives, the second step, keys and tokens in words, and what it is
// signed in with, over fixtures of the JSON in design 11.7 and 13.3.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  accountLine, canWords, dayLabel, initialOf, isDisabled, keyWords, lastWords, passwordWords, RECOVERY_CODES,
  scopesWords, SHADES, sessionWords, shadeOf, shortFingerprint, signedIn, signInWords, sinceWords, stepsWords, tokenWords,
} from '../lib/account-model.js';

/** Unix milliseconds of a UTC time. */
const at = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute);

// Saturday Nov 14 2026, 10:12 UTC: 11:12 in Prague, 03:12 in Denver.
const NOW = at(2026, 11, 14, 10, 12);
const PRAGUE = 'Europe/Prague';
const DENVER = 'America/Denver';

/** An account as `user list --json` gives it. */
const account = (name, more = {}) => ({
  name, uid: 1000, full_name: '', home: `/users/${name}`, shell: '/system/commands/nukesh', admin: false, console: false,
  groups: [name], disabled: null, password: null, ...more,
});

// ---- what an account can do ---------------------------------------------------

test('an administrator is told by one word, since an administrator can do all there is', () => {
  assert.equal(canWords(account('owner', { admin: true, console: true, groups: ['admins', 'owner'] })), 'Administrator');
});

test('an account is told by what it may sign in to', () => {
  assert.equal(canWords(account('sam', { console: true })), 'Console and shell');
  assert.equal(canWords(account('sam')), 'Shell');
  assert.equal(canWords(account('sam', { console: true, shell: '' })), 'Console');
  assert.equal(canWords(account('sam', { shell: '' })), 'Files only');
});

test('an account that is disabled says so before anything else', () => {
  assert.equal(canWords(account('old', { admin: true, disabled: true })), 'Disabled');
  assert.equal(canWords(account('old', { password: 'none', disabled: { at_ms: NOW } }), 2), 'Disabled');
});

test('an account with no password is told by its keys, or by that they are not known', () => {
  const backup = account('backup', { password: 'none' });
  assert.equal(canWords(backup, 1), 'SSH key only');
  assert.equal(canWords(backup, 3), 'SSH key only');
  assert.equal(canWords(backup, 0), 'Cannot sign in');
  assert.equal(canWords(backup), 'No password');
  assert.equal(canWords(account('sam', { password: 'set', console: true }), 4), 'Console and shell', 'keys change nothing for an account with a password');
});

test('disabled is root\'s to read: null and false are accounts that work, and any other value is one that does not', () => {
  assert.equal(isDisabled(account('a')), false);
  assert.equal(isDisabled(account('a', { disabled: false })), false);
  assert.equal(isDisabled(account('a', { disabled: true })), true);
  assert.equal(isDisabled(account('a', { disabled: { at_ms: NOW } })), true);
});

test('a circle has the first letter of the name in capitals, and a colour that is the same each time', () => {
  assert.equal(initialOf(account('alice')), 'A');
  assert.equal(initialOf(account('_build')), '_');
  for (const name of ['alice', 'sam', 'backup', 'owner', 'x']) {
    assert.equal(shadeOf(account(name)), shadeOf(account(name)));
    assert.ok(shadeOf(account(name)) >= 0 && shadeOf(account(name)) < SHADES);
  }
  assert.equal(new Set(['a', 'b', 'c', 'd'].map((name) => shadeOf(account(name)))).size, SHADES);
});

test('the headline says the right to administer, the number and since when', () => {
  const owner = account('owner', { uid: 1000, admin: true, since_ms: at(2026, 10, 19, 12) });
  assert.equal(accountLine(owner, PRAGUE, NOW), 'Administrator · account 1000 · since Oct 19');
  assert.equal(accountLine(account('sam', { uid: 1001 }), PRAGUE, NOW), 'Account 1001');
  assert.equal(accountLine(account('sam', { uid: 1001, since_ms: at(2025, 3, 2, 12) }), PRAGUE, NOW), 'Account 1001 · since Mar 2, 2025');
  assert.equal(accountLine(owner, PRAGUE, NOW).includes('null'), false);
});

test('the password row says set or none with the day it changed, and nothing for a person who is not told', () => {
  assert.equal(passwordWords(account('a', { password: 'set', password_changed_ms: at(2026, 10, 19, 12) }), DENVER, NOW), 'Set, changed Oct 19');
  assert.equal(passwordWords(account('a', { password: 'set' }), DENVER, NOW), 'Set');
  assert.equal(passwordWords(account('a', { password: 'none' }), DENVER, NOW), 'None');
  assert.equal(passwordWords(account('a'), DENVER, NOW), null);
});

// ---- the second step, keys and tokens --------------------------------------------

test('the second step is the app and the passkeys it has, with the codes and browsers beside it', () => {
  const steps = { app: true, passkeys: [{ name: 'Security key', id: 'a1' }], recovery_left: 8, trusted_browsers: 2 };
  assert.deepEqual(stepsWords(steps), {
    summary: 'Authenticator app and 1 passkey', extras: [`8 of ${RECOVERY_CODES} recovery codes left`, '2 trusted browsers'],
  });
  assert.deepEqual(stepsWords({ app: false, passkeys: [{}, {}], recovery_left: 10, trusted_browsers: 1 }), {
    summary: '2 passkeys', extras: ['10 of 10 recovery codes left', '1 trusted browser'],
  });
  assert.deepEqual(stepsWords({ app: true, passkeys: [], recovery_left: 0, trusted_browsers: 0 }), { summary: 'Authenticator app', extras: [] });
});

test('no app and no passkey is no second step, whatever codes and browsers are left', () => {
  assert.deepEqual(stepsWords({ app: false, passkeys: [], recovery_left: 3, trusted_browsers: 2 }), { summary: 'None', extras: [] });
  assert.deepEqual(stepsWords(undefined), { summary: 'None', extras: [] });
});

test('a key is its comment, its type and a fingerprint short enough to read and whole on focus', () => {
  const key = {
    type: 'ssh-ed25519', comment: 'laptop', fingerprint: 'SHA256:pR8xAbCdEfGhIjKlMnOpQrStUvWxYz0123456789abc1kQ', line: 3, options: false,
  };
  assert.deepEqual(keyWords(key), {
    name: 'laptop', type: 'ED25519', short: 'SHA256:pR8x…1kQ', full: key.fingerprint, options: false,
  });
  assert.equal(keyWords({ ...key, type: 'ecdsa-sha2-nistp256' }).type, 'ECDSA P-256');
  assert.equal(keyWords({ ...key, type: 'ecdsa-sha2-nistp384' }).type, 'ECDSA P-384');
  assert.equal(keyWords({ ...key, type: 'ssh-rsa' }).type, 'ssh-rsa', 'a type that is not told as it is');
  assert.equal(keyWords({ ...key, comment: '' }).name, 'A key with no name');
  assert.equal(keyWords({ ...key, options: true }).options, true);
  assert.equal(shortFingerprint('SHA256:abc'), 'SHA256:abc');
});

test('a token\'s scopes are told as what it may do: everything its owner can, or an area read or read and changed', () => {
  assert.equal(scopesWords(['*'], 'chase'), 'Everything chase can do');
  assert.equal(scopesWords(['snapshots:read'], 'chase'), 'Snapshots: read');
  assert.equal(scopesWords(['snapshots'], 'chase'), 'Snapshots: read and change');
  assert.equal(scopesWords(['services:read', 'ssh-keys'], 'chase'), 'Services: read, Ssh keys: read and change');
  assert.equal(scopesWords(['snapshots:read', '*'], 'chase'), 'Everything chase can do', 'everything covers the rest');
});

test('a token says what it may do and when it runs out, in the zone shown', () => {
  const token = { name: 'ansible', scopes: ['*'], until_ms: at(2026, 12, 13, 12) };
  assert.deepEqual(tokenWords(token, 'chase', DENVER, NOW), { name: 'ansible', detail: 'Everything chase can do · expires Dec 13' });
  assert.equal(tokenWords({ ...token, until_ms: at(2026, 11, 1, 12) }, 'chase', DENVER, NOW).detail, 'Everything chase can do · expired Nov 1');
  assert.equal(tokenWords({ ...token, until_ms: at(2027, 2, 1, 12) }, 'chase', DENVER, NOW).detail, 'Everything chase can do · expires Feb 1, 2027');
  assert.equal(tokenWords({ name: 'x', scopes: ['snapshots:read'] }, 'chase', DENVER, NOW).detail, 'Snapshots: read');
});

// ---- when -------------------------------------------------------------------------

test('a day is Today, Yesterday or the date, on the clocks of the zone shown', () => {
  assert.equal(dayLabel(at(2026, 11, 14, 8), PRAGUE, NOW), 'Today');
  assert.equal(dayLabel(at(2026, 11, 13, 20), PRAGUE, NOW), 'Yesterday');
  assert.equal(dayLabel(at(2026, 11, 8, 20), PRAGUE, NOW), 'Nov 8');
  assert.equal(dayLabel(at(2025, 11, 8, 20), PRAGUE, NOW), 'Nov 8, 2025');
  // 23:30 UTC on the 13th is already the 14th in Prague and still the 13th in Denver.
  assert.equal(dayLabel(at(2026, 11, 13, 23, 30), PRAGUE, NOW), 'Today');
  assert.equal(dayLabel(at(2026, 11, 13, 23, 30), DENVER, NOW), 'Yesterday');
});

test('since says the clock today, and the day with it after', () => {
  assert.equal(sinceWords(at(2026, 11, 14, 6, 12), PRAGUE, NOW), 'since 7:12 AM');
  assert.equal(sinceWords(at(2026, 11, 14, 6, 12), DENVER, NOW), 'since yesterday 11:12 PM', 'the same time is the evening before in Denver');
  assert.equal(sinceWords(at(2026, 11, 14, 8, 5), PRAGUE, NOW), 'since 9:05 AM');
  assert.equal(sinceWords(at(2026, 11, 13, 20, 40), PRAGUE, NOW), 'since yesterday 9:40 PM');
  assert.equal(sinceWords(at(2026, 11, 9, 20, 40), PRAGUE, NOW), 'since Nov 9 9:40 PM');
});

// ---- last active --------------------------------------------------------------------

/** An entry of `lastlog --json`; `via` is how the person came, as authd records it. */
const last = (terminal, atMs, from = '192.168.1.25', via = null) => ({
  name: 'x', sign_in: terminal === null ? null : { at_ms: atMs, terminal, from, via }, failed: 0, last_failed: null,
});

test('Last active names the day and the place of the last sign-in, for each kind of terminal', () => {
  const yesterday = at(2026, 11, 13, 14);
  const nov8 = at(2026, 11, 8, 14);
  assert.equal(lastWords(last('pts/2', yesterday, '192.168.1.25', 'ssh'), PRAGUE, { now: NOW }), 'Yesterday, SSH');
  assert.equal(lastWords(last('smb', nov8), PRAGUE, { now: NOW }), 'Nov 8, file sharing');
  assert.equal(lastWords(last('nfs', nov8), PRAGUE, { now: NOW }), 'Nov 8, file sharing');
  assert.equal(lastWords(last('web', yesterday, '192.168.1.25', 'console'), PRAGUE, { now: NOW }), 'Yesterday, web console');
  assert.equal(lastWords(last('pts/3', yesterday, '192.168.1.25', 'console'), PRAGUE, { now: NOW }), 'Yesterday, console terminal');
  assert.equal(lastWords(last('tty1', at(2026, 11, 14, 8), null), PRAGUE, { now: NOW }), 'Today, at the machine');
  assert.equal(lastWords(last('ttyS0', nov8, null), PRAGUE, { now: NOW }), 'Nov 8, ttyS0', 'a terminal\'s own name is said as it is');
  assert.equal(lastWords(last('pts/4', nov8), PRAGUE, { now: NOW }), 'Nov 8, pts/4', 'an older record does not say SSH from the console');
});

test('Last active is Never for an account that has not signed in, and Now for the person\'s own', () => {
  assert.equal(lastWords(last(null), PRAGUE, { now: NOW }), 'Never');
  assert.equal(lastWords(undefined, PRAGUE, { now: NOW }), 'Never');
  assert.equal(lastWords({ sign_in: { at_ms: 'soon', terminal: 'pts/2', via: 'ssh' } }, PRAGUE, { now: NOW }), 'Never');
  assert.equal(lastWords(last('pts/2', at(2026, 11, 13, 14), '192.168.1.25', 'ssh'), PRAGUE, { now: NOW, own: true }), 'Now, this browser');
  assert.equal(lastWords(last(null), PRAGUE, { now: NOW, own: true }), 'Now, this browser');
});

test('where a sign-in came from has words for the places lastlog gives', () => {
  const places = [
    { terminal: 'pts/0', via: 'ssh' }, { terminal: 'web', via: 'console' }, { terminal: 'pts/1', via: 'console' },
    { terminal: 'web' }, { terminal: 'tty1' }, { terminal: 'smb' }, { terminal: 'nfs' }, { terminal: 'pts/4' }, {},
  ];
  assert.deepEqual(places.map(signInWords), [
    'SSH', 'web console', 'console terminal', 'web console', 'at the machine', 'file sharing', 'file sharing', 'pts/4', '',
  ]);
});

// ---- what an account is signed in with ------------------------------------------------

/** The answer of the sessions route (design 13.3) and of `user show`'s `sessions`. */
const browserSession = (id, more = {}) => ({
  id, browser: 'Firefox on Linux', from: '192.168.1.25', since_ms: at(2026, 11, 14, 6, 12), last_ms: at(2026, 11, 14, 10, 10), this: false, ...more,
});
const terminal = (id, more = {}) => ({
  id, title: 'build', panes: 2, admin: false, since_ms: at(2026, 11, 13, 20, 40), attached: true, ...more,
});
const shell = (id, more = {}) => ({
  id, kind: 'ssh', terminal: 'pts/3', from: '10.8.0.2', since_ms: at(2026, 11, 13, 20, 40), processes: 5, ...more,
});

test('this browser is told with its browser and address, since when', () => {
  assert.deepEqual(sessionWords({ ...browserSession('a'), kind: 'console', this: true }, PRAGUE, NOW), {
    title: 'This browser', detail: 'Firefox on Linux · 192.168.1.25', since: 'since 7:12 AM', what: 'the session in Firefox on Linux from 192.168.1.25',
  });
});

test('another browser of the account is told by its browser, with its address and when it was last used', () => {
  assert.deepEqual(sessionWords({ ...browserSession('b', { from: '192.168.1.30' }), kind: 'console' }, PRAGUE, NOW), {
    title: 'Firefox on Linux', detail: '192.168.1.30 · last used 11:10 AM', since: 'since 7:12 AM', what: 'the session in Firefox on Linux from 192.168.1.30',
  });
  const unknown = sessionWords({ id: 'c', from: '192.168.1.31', kind: 'console' }, PRAGUE, NOW);
  assert.equal(unknown.title, 'A browser');
  assert.equal(unknown.detail, '192.168.1.31');
  assert.equal(unknown.since, '');
  assert.equal(unknown.what, 'the session in a browser from 192.168.1.31');
});

test('a terminal is told by its name, its panes, an administrator\'s shell and whether a browser has it open', () => {
  assert.deepEqual(sessionWords({ ...terminal(3), kind: 'terminal' }, PRAGUE, NOW), {
    title: 'Terminal build', detail: '2 panes', since: 'since yesterday 9:40 PM', what: 'the terminal build',
  });
  assert.equal(sessionWords({ ...terminal(4, { panes: 1, admin: true, attached: false }), kind: 'terminal' }, PRAGUE, NOW).detail,
    '1 pane · with an administrator shell · not open in a browser');
  const unnamed = sessionWords({ ...terminal(7, { title: '' }), kind: 'terminal' }, PRAGUE, NOW);
  assert.equal(unnamed.title, 'Terminal 7');
  assert.equal(unnamed.what, 'the terminal 7');
});

test('a session at the machine is told by its kind, its address, its terminal and its processes', () => {
  assert.deepEqual(sessionWords(shell('4123-1'), PRAGUE, NOW), {
    title: 'SSH', detail: '10.8.0.2 · pts/3 · 5 processes', since: 'since yesterday 9:40 PM', what: 'the SSH session from 10.8.0.2',
  });
  assert.deepEqual(sessionWords(shell('9-1', { kind: 'screen', from: null, terminal: 'tty1', processes: 1 }), PRAGUE, NOW), {
    title: 'The machine\'s screen', detail: 'tty1 · 1 process', since: 'since yesterday 9:40 PM', what: 'the session on the machine\'s screen',
  });
  assert.equal(sessionWords(shell('9-2', { kind: 'serial', from: null, terminal: 'ttyS0' }), PRAGUE, NOW).title, 'A serial line');
  assert.equal(sessionWords(shell('9-3', { kind: 'other', from: null }), PRAGUE, NOW).title, 'Another session');
  assert.equal(sessionWords(shell('9-4', { kind: 'something new', from: null }), PRAGUE, NOW).title, 'Another session', 'a kind the console does not know is another session');
});

test('everything signed in is listed browsers first, this one at the top, then terminals, then the machine, oldest first', () => {
  const rows = signedIn({
    sessions: [browserSession('b', { since_ms: at(2026, 11, 14, 7) }), browserSession('a', { this: true, since_ms: at(2026, 11, 14, 9) }), browserSession('c', { since_ms: at(2026, 11, 14, 5) })],
    terminals: [terminal(9, { since_ms: at(2026, 11, 14, 1) }), terminal(2, { since_ms: at(2026, 11, 13, 1) })],
    machine: [shell('s2', { since_ms: at(2026, 11, 14, 2) }), shell('s1', { since_ms: at(2026, 11, 13, 2) })],
  }, PRAGUE, NOW);
  assert.deepEqual(rows.map((row) => `${row.scope}:${row.id}`), ['console:a', 'console:c', 'console:b', 'terminal:2', 'terminal:9', 'machine:s1', 'machine:s2']);
  assert.deepEqual(rows.map((row) => row.this), [true, false, false, false, false, false, false]);
  assert.equal(rows[0].title, 'This browser');
  assert.equal(rows[3].title, 'Terminal build');
  assert.equal(rows[5].title, 'SSH');
});

test('a console terminal\'s shells are its row, and not a session at the machine as well', () => {
  const rows = signedIn({
    terminals: [terminal(4, { ttys: ['pts/1', 'pts/2'] })],
    machine: [shell('s1', { terminal: 'pts/1' }), shell('s2', { terminal: 'pts/2' }), shell('s3', { terminal: 'pts/3' })],
  }, PRAGUE, NOW);
  assert.deepEqual(rows.map((row) => `${row.scope}:${row.id}`), ['terminal:4', 'machine:s3']);
  // Once the terminal has ended, a shell that is still there is listed.
  const after = signedIn({ terminals: [], machine: [shell('s1', { terminal: 'pts/1' })] }, PRAGUE, NOW);
  assert.deepEqual(after.map((row) => `${row.scope}:${row.id}`), ['machine:s1']);
});

test('an account with nothing signed in has no rows, and lists that are missing are none', () => {
  assert.deepEqual(signedIn({ sessions: [], terminals: [], machine: [] }, PRAGUE, NOW), []);
  assert.deepEqual(signedIn({}, PRAGUE, NOW), []);
});

test('the lists are not changed by sorting them', () => {
  const sessions = [browserSession('b', { since_ms: 2 }), browserSession('a', { since_ms: 1 })];
  signedIn({ sessions }, PRAGUE, NOW);
  assert.deepEqual(sessions.map((one) => one.id), ['b', 'a']);
});
