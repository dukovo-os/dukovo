// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Security page says and asks (design 8): the score's words, the SSH
// row and form, the sign-in rules and their warnings, the blocking rule and
// the never-block list, and private access's devices and set-up.

import assert from 'node:assert/strict';
import test from 'node:test';

import { assess } from '../lib/health.js';
import {
  checkedWords, nothingRan, passedButton, passedRows, plusWords, pointsWords, ringWords, scoreWords,
} from '../pages/security/check-model.js';
import {
  beganAt, checkWords, connectCommand, hasMistakes, keepNote, listWords, machineOf, mistakeWords, rowWords, signInsSince, signInWords,
} from '../pages/security/ssh-model.js';
import {
  callsOf, effectOf, formOf, problemOf, setArgs, settingsOf, settingsOfShow, SSH_WINDOWS,
} from '../pages/security/ssh-form-model.js';
import { blockingWords, isPublic, neverWords, reachRow, rulesOf, rulesWords } from '../pages/security/rules-model.js';
import { cutsWords, neverCheck, ruleChange, rulesProblems } from '../pages/security/rules-form-model.js';
import { deviceRows, handshakeWords, interfaceOf, interfaceWords, transferWords } from '../pages/security/private-model.js';
import {
  blankSetUp, deviceFile, fileName, nameProblem, openPort, serveArgs, setUpProblems,
} from '../pages/security/private-form-model.js';
import { at, NOW, time } from './snapshots-fixtures.mjs';

// ---- the score ------------------------------------------------------------------------------

const result = (...outcomes) => ({
  checked: '2026-11-14T05:17:03Z',
  results: outcomes.map((outcome, number) => ({
    area: 'ssh', check: `check ${number}`, result: outcome, detail: `detail ${number}`, id: `SEC-1${number}0`, fix: 'Fix it.',
  })),
});

test('the score says how many checks pass, from the counts when the result has them', () => {
  assert.equal(scoreWords({ counts: { passed: 26, warning: 3, failed: 0, skipped: 0 } }), '26 of 29 checks pass');
  assert.equal(scoreWords({ counts: { passed: 27, warning: 1, failed: 1, skipped: 2 } }), '27 of 29 checks pass, 2 skipped');
  assert.equal(scoreWords({ counts: { passed: 12, warning: 0, failed: 0, skipped: 0 } }), 'All 12 checks pass');
  assert.equal(scoreWords({ counts: { passed: 0, warning: 0, failed: 0, skipped: 3 } }), 'No check ran, 3 skipped');
});

test('a result with no counts, or counts that are not numbers, is counted from its results', () => {
  assert.equal(scoreWords(result('passed', 'passed', 'warning', 'FAILED', 'skipped')), '2 of 4 checks pass, 1 skipped');
  assert.equal(scoreWords({ ...result('passed', 'warning'), counts: { passed: 'many' } }), '1 of 2 checks pass');
  assert.equal(scoreWords(null), 'No check ran');
});

test('the ring, the points and the time have their words', () => {
  assert.equal(ringWords(86, { words: 'Good' }), 'Health score 86 of 100: Good');
  assert.equal(pointsWords(6), 'takes 6 points');
  assert.equal(pointsWords(1), 'takes 1 point');
  assert.equal(plusWords(6), '+6');
  assert.equal(checkedWords(Math.floor(at(2026, 11, 14, 5, 17) / 1000), time), 'today at 5:17 AM');
  assert.equal(checkedWords(Math.floor(at(2026, 11, 13, 19, 40) / 1000), time), 'yesterday at 7:40 PM');
  assert.equal(checkedWords(Math.floor(at(2026, 11, 3, 2, 4) / 1000), time), 'on Nov 3 at 2:04 AM');
  assert.equal(checkedWords(Math.floor(at(2025, 11, 3, 2, 4) / 1000), time), 'on Nov 3, 2025');
  assert.ok(NOW > 0);
});

test('the checks that passed or were skipped are listed in order, with the words of the button that opens them', () => {
  const rows = passedRows(result('passed', 'warning', 'skipped', 'passed'));
  assert.deepEqual(rows.map((row) => [row.check, row.skipped]), [['check 0', false], ['check 2', true], ['check 3', false]]);
  assert.equal(passedButton(rows), 'Show the 2 that pass and 1 skipped');
  assert.equal(passedButton(passedRows(result('passed', 'passed'))), 'Show the 2 that pass');
  assert.deepEqual(passedRows(null), []);
  assert.equal(nothingRan({ results: [] }), true);
  assert.equal(nothingRan(null), true);
  assert.equal(nothingRan(result('passed')), false);
  assert.equal(assess(result('warning')).findings.length, 1, 'the model reads what health.js reads');
});

// ---- SSH: the row -----------------------------------------------------------------------------

const show = (more = {}) => ({
  on: true, running: true, port: 22, passwords: false, keys: true, allow_users: [], allow_groups: [], deny_users: [], deny_groups: [], forwarding: 'yes', gateway_ports: 'no', mistakes: [], host_key: 'SHA256:x', trial: null, ...more,
});

test('the row is the board\'s line, and Off when SSH is off', () => {
  assert.equal(rowWords(show({ passwords: true })), 'Port 22 · keys and passwords · root cannot sign in');
  assert.equal(rowWords(show()), 'Port 22 · keys only · root cannot sign in');
  assert.equal(rowWords(show({ keys: false, passwords: true })), 'Port 22 · passwords only · root cannot sign in');
  assert.equal(rowWords(show({ on: false })), 'Off');
});

test('the row says who may sign in, and what is not known or not running', () => {
  assert.equal(rowWords(show({ allow_users: ['alice', 'bob'] })), 'Port 22 · keys only · only alice and bob · root cannot sign in');
  assert.equal(rowWords(show({ allow_groups: ['admins'], allow_users: ['alice'] })), 'Port 22 · keys only · only alice, and only the group admins · root cannot sign in');
  assert.equal(rowWords(show({ running: false })), 'Port 22 · keys only · root cannot sign in · not running now');
  assert.equal(rowWords(show({ port: null, passwords: null, keys: null })), 'root cannot sign in');
  assert.equal(rowWords(null), 'root cannot sign in');
});

test('a file with a mistake is said with its line, and the row does not guess its settings', () => {
  const wrong = show({
    passwords: null, keys: null, mistakes: [{ line: 7, text: '  PasswordAuthentication maybe ', why: 'maybe is not yes or no' }],
  });
  assert.equal(hasMistakes(wrong), true);
  assert.equal(mistakeWords(wrong), 'sshd_config has a mistake on line 7: maybe is not yes or no (PasswordAuthentication maybe).');
  assert.equal(rowWords(wrong), 'On · port 22 · the settings file has a mistake');
  assert.equal(mistakeWords(show()), '');
  assert.equal(hasMistakes(show()), false);
});

test('names make a list in a sentence', () => {
  assert.equal(listWords([]), '');
  assert.equal(listWords(['alice']), 'alice');
  assert.equal(listWords(['alice', 'bob']), 'alice and bob');
  assert.equal(listWords(['alice', 'bob', 'carol']), 'alice, bob and carol');
});

// ---- SSH: the form ------------------------------------------------------------------------------

test('the form starts from the settings, and the presets are Keys only and the choice of accounts', () => {
  assert.deepEqual(formOf(show({ passwords: true })), {
    on: true, passwords: true, keys: true, users: [], groups: [], who: 'everyone',
  });
  assert.deepEqual(formOf(show({ passwords: true }), 'keys-only'), {
    on: true, passwords: false, keys: true, users: [], groups: [], who: 'everyone',
  });
  assert.equal(formOf(show(), 'who').who, 'users');
  assert.equal(formOf(show({ allow_groups: ['admins'] }), 'who').who, 'groups', 'a choice that was made is kept');
  assert.equal(formOf(show({ allow_users: ['a'], allow_groups: ['g'] })).who, 'both');
  assert.equal(formOf(show({ on: false })).on, false);
  assert.equal(formOf(show({ passwords: null, keys: null })).passwords, false, 'a setting that is not known is the default');
  assert.equal(formOf(show({ passwords: null, keys: null })).keys, true);
});

test('the settings of the form leave out the list that its choice does not use', () => {
  const form = { on: true, passwords: false, keys: true, who: 'users', users: ['alice'], groups: ['admins'] };
  assert.deepEqual(settingsOf(form), { on: true, passwords: false, keys: true, allow_users: ['alice'], allow_groups: [] });
  assert.deepEqual(settingsOf({ ...form, who: 'groups' }).allow_groups, ['admins']);
  assert.deepEqual(settingsOf({ ...form, who: 'both' }).allow_users, ['alice']);
  assert.deepEqual(settingsOf({ ...form, who: 'everyone' }).allow_groups, []);
  assert.deepEqual(settingsOfShow(show({ allow_users: ['alice'] })).allow_users, ['alice']);
});

test('a form with no way in, or a list with nobody in it, is refused with the words of the problem', () => {
  const base = { on: true, passwords: true, keys: true, who: 'everyone', users: [], groups: [] };
  assert.equal(problemOf(base), '');
  assert.equal(problemOf({ ...base, passwords: false, keys: false }), 'Nobody could sign in over SSH. Turn on passwords, keys or both.');
  assert.equal(problemOf({ ...base, on: false, passwords: false, keys: false }), '', 'SSH off needs no way in');
  assert.match(problemOf({ ...base, who: 'users' }), /at least one account/);
  assert.match(problemOf({ ...base, who: 'groups' }), /at least one group/);
  assert.match(problemOf({ ...base, who: 'users', users: Array.from({ length: 33 }, (_, at2) => `u${at2}`) }), /at most 32/);
});

test('the arguments of ssh.set are what changed, in the words the command takes', () => {
  const before = settingsOfShow(show({ passwords: true, allow_users: ['alice', 'bob'] }));
  assert.deepEqual(setArgs(before, { ...before, passwords: false }), { passwords: 'no' });
  assert.deepEqual(setArgs(before, { ...before, keys: false }), { keys: 'no' });
  assert.deepEqual(setArgs(before, { ...before, allow_users: ['bob', 'alice'] }), {}, 'the same names in another order change nothing');
  assert.deepEqual(setArgs(before, { ...before, allow_users: ['alice'] }), { allow_users: 'alice' });
  assert.deepEqual(setArgs(before, { ...before, allow_users: [], allow_groups: ['admins', 'staff'] }), { allow_users: 'everyone', allow_groups: 'admins staff' });
  assert.deepEqual(setArgs(before, before), {});
});

test('the calls: off is ssh.off, on comes first and is never tried, and a try goes with the call that takes a way in', () => {
  const on = settingsOfShow(show({ passwords: true }));
  const off = { ...on, on: false };
  assert.deepEqual(callsOf(on, off), [{ action: 'ssh.off', args: {} }]);
  assert.deepEqual(callsOf(on, off, 300), [{ action: 'ssh.off', args: { try: 300 } }]);
  assert.deepEqual(callsOf(off, off), []);
  assert.deepEqual(callsOf(off, { ...off, on: true }), [{ action: 'ssh.on', args: {} }]);
  assert.deepEqual(callsOf(on, { ...on, passwords: false }, 120), [{ action: 'ssh.set', args: { passwords: 'no', try: 120 } }]);
  assert.deepEqual(callsOf(on, { ...on, passwords: false }), [{ action: 'ssh.set', args: { passwords: 'no' } }]);
  assert.deepEqual(callsOf(on, on, 300), []);
});

test('turning SSH on with settings that differ from the file is ssh.on and then ssh.set, and only the set is tried', () => {
  const off = { ...settingsOfShow(show({ passwords: true })), on: false };
  const wanted = { ...off, on: true, passwords: false };
  assert.deepEqual(callsOf(off, wanted, 300), [
    { action: 'ssh.on', args: {} },
    { action: 'ssh.set', args: { passwords: 'no', try: 300 } },
  ]);
});

test('the form says what a change does and whether it takes a way in', () => {
  const before = settingsOfShow(show({ passwords: true }));
  const keysOnly = effectOf(before, { ...before, passwords: false });
  assert.deepEqual(keysOnly, { lines: ['Passwords stop signing in over SSH.'], narrows: true, changes: true });
  const widen = effectOf(settingsOfShow(show()), { ...settingsOfShow(show()), passwords: true });
  assert.deepEqual(widen, { lines: ['Passwords sign in over SSH.'], narrows: false, changes: true });
  assert.deepEqual(effectOf(before, { ...before, on: false }).lines, ['SSH stops. Nobody can sign in over SSH.']);
  const some = effectOf(before, { ...before, allow_users: ['alice', 'bob'] });
  assert.deepEqual(some.lines, ['Over SSH, only alice and bob may sign in.']);
  assert.deepEqual(effectOf(settingsOfShow(show({ allow_users: ['alice'], passwords: true })), before).lines, ['Every account may sign in over SSH.']);
  assert.deepEqual(effectOf(before, before), { lines: [], narrows: false, changes: false });
});

test('the windows of an SSH try are two, five and ten minutes', () => {
  assert.deepEqual(SSH_WINDOWS.map((one) => one.seconds), [120, 300, 600]);
});

// ---- SSH: while a try runs -----------------------------------------------------------------------------

test('the command to type names the account and the machine, with .local for a name', () => {
  assert.equal(connectCommand('owner', 'duke-nas'), 'ssh owner@duke-nas.local');
  assert.equal(connectCommand('owner', 'duke-nas.example.org'), 'ssh owner@duke-nas.example.org');
  assert.equal(machineOf('192.168.1.5'), '192.168.1.5');
  assert.equal(machineOf('fd00::1'), 'fd00::1');
  assert.match(checkWords, /Connections that were open stay open, so only a new one tells\./);
});

test('the try began at the time of the newest journal row of an ssh change that was tried', () => {
  const row = (action, over = {}) => ({ action, at: 1_763_100_000, status: 0, argv: ['ssh-server', 'set', 'passwords', 'no', '--try', '300'], ...over });
  assert.equal(beganAt([row('ssh.set')]), 1_763_100_000_000);
  assert.equal(beganAt([row('ssh.set', { status: 1 }), row('ssh.off', { at: 1_763_000_000 })]), 1_763_000_000_000, 'a refused change changed nothing');
  assert.equal(beganAt([row('ssh.set', { argv: ['ssh-server', 'set', 'passwords', 'no'] })]), null, 'a change that was not tried');
  assert.equal(beganAt([row('network.set')]), null);
  assert.equal(beganAt(null), null);
});

/** An entry of `lastlog --json`: over SSH, at a console terminal, or in the web console. */
const PLACES = Object.freeze({ ssh: { terminal: 'pts/1', via: 'ssh' }, terminal: { terminal: 'pts/2', via: 'console' }, web: { terminal: 'web', via: 'console' } });
const last = (name, place, atMs, from = '192.168.1.25') => ({ name, sign_in: { at_ms: atMs, ...PLACES[place], from }, failed: 0, last_failed: null });

test('the SSH sign-ins since a change are those of the accounts whose latest sign-in is over SSH since then, oldest first', () => {
  const rows = signInsSince([
    last('alice', 'ssh', at(2026, 11, 14, 10, 5)),
    last('bob', 'web', at(2026, 11, 14, 10, 6)),
    last('erin', 'terminal', at(2026, 11, 14, 10, 7)),
    last('carol', 'ssh', at(2026, 11, 14, 9, 0)),
    last('owner', 'ssh', at(2026, 11, 14, 10, 1), null),
    { name: 'dave', sign_in: null },
  ], at(2026, 11, 14, 10, 0));
  assert.deepEqual(rows.map((one) => one.name), ['owner', 'alice']);
  assert.equal(signInWords(rows[1], time), 'alice signed in over SSH at 10:05 AM from 192.168.1.25');
  assert.equal(signInWords(rows[0], time), 'owner signed in over SSH at 10:01 AM');
  assert.deepEqual(signInsSince(null, 0), []);
});

test('Keep says that nobody has signed in until someone has', () => {
  assert.equal(keepNote([]), 'Nobody has signed in over SSH since the change.');
  assert.equal(keepNote([{}]), '1 account signed in over SSH since the change.');
  assert.equal(keepNote([{}, {}]), '2 accounts signed in over SSH since the change.');
});

// ---- the sign-in rules ----------------------------------------------------------------------------------------

const rules = (more = {}) => rulesOf({ sign_in: { second_step: 'if-set', session_hours: 12, idle_minutes: 30, ...more } });

test('the rules are the two lines of the design, whatever the second step is', () => {
  assert.deepEqual(rulesWords(rules({ second_step: 'administrators' })), [
    'Who gives a second step: administrators, and anyone who has added one.',
    'A session lasts 12 hours after its sign-in, and ends after 30 minutes with no console page open.',
  ]);
  assert.equal(rulesWords(rules())[0], 'Who gives a second step: anyone who has added one.');
  assert.equal(rulesWords(rules({ second_step: 'everyone', session_hours: 1, idle_minutes: 5 }))[0], 'Who gives a second step: everyone.');
  assert.equal(rulesWords(rules({ session_hours: 1 }))[1], 'A session lasts 1 hour after its sign-in, and ends after 30 minutes with no console page open.');
});

test('rules the server does not say are the defaults, and an unknown second step is the default one', () => {
  assert.deepEqual(rulesOf(null), { second_step: 'if-set', session_hours: 12, idle_minutes: 30 });
  assert.equal(rulesOf({ sign_in: { second_step: 'some' } }).second_step, 'if-set');
  assert.equal(rulesOf({ sign_in: { session_hours: '8' } }).session_hours, 12);
});

test('the numbers are held to the bounds of admin.toml(5), field by field', () => {
  const form = { second_step: 'if-set', hours: '12', minutes: '30' };
  assert.deepEqual(rulesProblems(form), {});
  assert.deepEqual(Object.keys(rulesProblems({ ...form, hours: '0' })), ['hours']);
  assert.deepEqual(Object.keys(rulesProblems({ ...form, hours: '169', minutes: '4' })), ['hours', 'minutes']);
  assert.deepEqual(rulesProblems({ ...form, hours: '168', minutes: '1440' }), {});
  assert.deepEqual(Object.keys(rulesProblems({ ...form, hours: '' })), ['hours']);
  assert.deepEqual(Object.keys(rulesProblems({ ...form, minutes: '2.5' })), ['minutes']);
  assert.match(rulesProblems({ ...form, hours: 'x' }).hours, /from 1 to 168/);
});

test('a second step for more people says that nobody is locked out, and fewer hours says what ends', () => {
  const now = rules();
  const more = ruleChange(now, { second_step: 'administrators', hours: '12', minutes: '30' });
  assert.deepEqual(more.args, { second_step: 'administrators' });
  assert.deepEqual(more.lines, ['Accounts that have none add one at their next sign-in, before anything else. Nobody is locked out.']);
  const less = ruleChange(now, { second_step: 'if-set', hours: '2', minutes: '30' });
  assert.deepEqual(less.args, { session_hours: 2 });
  assert.deepEqual(less.lines, ['Sessions older than 2 hours end within 30 seconds, this one too if it is: you sign in again.']);
  const idle = ruleChange(now, { second_step: 'if-set', hours: '12', minutes: '10' });
  assert.deepEqual(idle.args, { idle_minutes: 10 });
  assert.match(idle.lines[0], /no console page open for more than 10 minutes/);
});

test('a second step for fewer people, or more hours, changes the rule and says nothing of what follows', () => {
  const now = rules({ second_step: 'everyone', session_hours: 8 });
  const change = ruleChange(now, { second_step: 'if-set', hours: '24', minutes: '45' });
  assert.deepEqual(change.args, { second_step: 'if-set', session_hours: 24, idle_minutes: 45 });
  assert.deepEqual(change.lines, []);
  assert.deepEqual(ruleChange(now, { second_step: 'everyone', hours: '8', minutes: '30' }), { args: {}, lines: [] });
});

// ---- who may reach the console -------------------------------------------------------------------------------------

test('the console\'s row says its port and whom it answers', () => {
  assert.equal(reachRow({ listen: 9090, reach: 'private' }), 'Port 9090 · this network and WireGuard only');
  assert.equal(reachRow({ listen: 443, reach: 'any' }), 'Port 443 · anyone who can reach this server');
  assert.equal(reachRow(null), 'this network and WireGuard only');
  assert.equal(isPublic({ reach: 'any' }), true);
  assert.equal(isPublic({ reach: 'private' }), false);
  assert.equal(isPublic(undefined), false);
});

test('a browser outside the network is told that it will be cut off, and when the change is undone', () => {
  assert.equal(
    cutsWords('203.0.113.9', '2:32 PM'),
    'This browser comes from 203.0.113.9, which is outside. After this change it cannot reach the console: the change is undone at 2:32 PM unless it is kept from a browser on this network.',
  );
});

// ---- the blocked addresses ----------------------------------------------------------------------------------------------

test('the blocking rule is said from the settings, and not at all when the command does not give them', () => {
  assert.equal(
    blockingWords({ tries: 5, within_s: 600, first_s: 600, longest_s: 86_400 }),
    '5 wrong tries in 10 minutes block an address for 10 minutes; each new block of it doubles, up to a day. An administrator\'s own address is never blocked while they are signed in.',
  );
  assert.match(blockingWords({ tries: 1, within_s: 30, first_s: 3600, longest_s: 172_800 }), /^1 wrong try in 30 seconds block an address for an hour; .* up to 2 days\./);
  assert.equal(blockingWords({ on: true }), '');
  assert.equal(blockingWords(null), '');
});

test('the never-block list is a line of networks', () => {
  assert.equal(neverWords(['192.168.1.0/24', '10.8.0.0/24']), 'Never blocked: 192.168.1.0/24 and 10.8.0.0/24');
  assert.equal(neverWords(['10.0.0.0/8']), 'Never blocked: 10.0.0.0/8');
  assert.equal(neverWords(['a', 'b', 'c']), 'Never blocked: a, b and c');
  assert.equal(neverWords([]), 'Never blocked: no network');
  assert.equal(neverWords(undefined), 'Never blocked: no network');
});

test('a private network of a /16 or narrower is added without a question', () => {
  assert.deepEqual(neverCheck('192.168.1.0/24'), { network: '192.168.1.0/24', problem: '', ask: null });
  assert.equal(neverCheck(' 10.8.0.5 ').ask, null);
  assert.equal(neverCheck('192.168.0.0/16').ask, null);
  assert.equal(neverCheck('fd00:1234::/48').ask, null);
});

test('a network that is not private, or wider than a /16 or a /48, asks first, in the words of the review', () => {
  assert.equal(neverCheck('203.0.113.0/24').ask, 'Addresses in 203.0.113.0/24 are never blocked, however often they get a password wrong.');
  assert.equal(neverCheck('10.0.0.0/8').ask, 'Addresses in 10.0.0.0/8 are never blocked, however often they get a password wrong.');
  assert.equal(neverCheck('192.168.0.0/15').ask === null, false);
  assert.equal(neverCheck('fd00::/32').ask === null, false);
  assert.equal(neverCheck('2001:db8::/32').ask === null, false);
  assert.equal(neverCheck('203.0.113.9').ask === null, false);
});

test('text that is not an address or a network is a problem, with the way to write one', () => {
  for (const text of ['', 'my laptop', '300.1.1.1', '10.0.0.0/33', '10.0.0.0/x']) {
    const found = neverCheck(text);
    assert.match(found.problem, /such as 192\.168\.1\.0\/24/, text);
    assert.equal(found.ask, null);
  }
});

// ---- private access --------------------------------------------------------------------------------------------------------

const wireguard = {
  interfaces: [{
    name: 'wg0',
    address: '10.8.0.1/24',
    port: 51_820,
    endpoint: 'home.example.org',
    public_key: 'KEY=',
    up: true,
    devices: [
      { name: 'phone', public_key: 'P=', addresses: ['10.8.0.2/32'], endpoint: '198.51.100.7:40122', latest_handshake_ms: at(2026, 11, 14, 10, 10), received: 1234, sent: 5678 },
      { name: 'laptop', public_key: 'L=', addresses: ['10.8.0.3/32'], endpoint: null, latest_handshake_ms: 0, received: 0, sent: 0 },
    ],
  }],
};

test('private access is set up when wg0 is there', () => {
  assert.equal(interfaceOf(wireguard).address, '10.8.0.1/24');
  assert.equal(interfaceOf({ interfaces: [] }), null);
  assert.equal(interfaceOf(null), null);
  assert.equal(interfaceOf({ interfaces: [{ name: 'wg1' }] }), null);
  assert.equal(interfaceWords(interfaceOf(wireguard)), 'wg0 · 10.8.0.1/24 · UDP port 51820 · devices reach it at home.example.org');
  assert.match(interfaceWords({ name: 'wg0', up: false }), /down$/);
});

test('each device has its address, when it last connected and what it sent and received', () => {
  const rows = deviceRows(interfaceOf(wireguard), time);
  assert.deepEqual(rows.map((row) => row.name), ['phone', 'laptop']);
  assert.equal(rows[0].addresses, '10.8.0.2/32');
  assert.equal(rows[0].connected, 'Connected 2 minutes ago');
  assert.equal(rows[1].connected, 'Never connected');
  assert.match(rows[0].transfer, /^received 1\.\d+ KB, sent 5\.\d+ KB$/);
  assert.deepEqual(deviceRows(null, time), []);
  assert.equal(transferWords({}), '');
  assert.equal(handshakeWords(null, time), 'Never connected');
  assert.equal(handshakeWords(NOW - 30_000, time), 'Connected just now');
});

test('a device\'s name is 1 to 32 letters, digits, - and _, and not one that is taken', () => {
  assert.equal(nameProblem('lab-phone_2'), '');
  assert.equal(nameProblem('a'.repeat(32)), '');
  assert.match(nameProblem(''), /Write a name/);
  assert.match(nameProblem('a b'), /1 to 32 letters/);
  assert.match(nameProblem('a'.repeat(33)), /1 to 32 letters/);
  assert.match(nameProblem('phone', ['phone']), /already has that name/);
  assert.equal(fileName('phone'), 'phone.conf');
});

test('the device\'s file is the config of the line that wireguard add --json prints in a change\'s output', () => {
  const file = '[Interface]\nPrivateKey = k\nAddress = 10.8.0.2/32\n';
  const line = JSON.stringify({ name: 'phone', address: '10.8.0.2/32', config: file, live: true });
  assert.equal(deviceFile({ done: true, changed: true, output: [line] }), file);
  assert.equal(deviceFile({ done: true, changed: true, output: ['wireguard: wgd reads the file when it starts', line] }), file);
  assert.equal(deviceFile({ done: true, changed: true, output: ['phone is added.'] }), null);
  assert.equal(deviceFile({ done: true, changed: true, output: [JSON.stringify({ config: 'no file' })] }), null);
  assert.equal(deviceFile({ config: file }), null);
  assert.equal(deviceFile(null), null);
});

test('the set-up form starts from the console\'s host and the defaults, and refuses what the action refuses', () => {
  const form = blankSetUp('home.example.org');
  assert.deepEqual(form, { endpoint: 'home.example.org', port: '51820', address: '10.8.0.1/24' });
  assert.deepEqual(setUpProblems(form), {});
  assert.deepEqual(Object.keys(setUpProblems({ ...form, endpoint: '' })), ['endpoint']);
  assert.deepEqual(Object.keys(setUpProblems({ ...form, endpoint: 'a b' })), ['endpoint']);
  assert.deepEqual(Object.keys(setUpProblems({ ...form, port: '80' })), ['port']);
  assert.deepEqual(Object.keys(setUpProblems({ ...form, port: '65536' })), ['port']);
  assert.deepEqual(Object.keys(setUpProblems({ ...form, address: '10.8.0.1' })), ['address']);
  assert.deepEqual(Object.keys(setUpProblems({ ...form, address: '10.8.0.1/31' })), ['address']);
  assert.deepEqual(Object.keys(setUpProblems({ ...form, address: '10.8.0.1/15' })), ['address']);
  assert.deepEqual(setUpProblems({ ...form, address: 'fd00::1/64' }), {});
  assert.deepEqual(Object.keys(setUpProblems({ ...form, address: 'fd00::1/32' })), ['address']);
  assert.deepEqual(Object.keys(setUpProblems({ endpoint: '', port: 'x', address: 'no' })), ['endpoint', 'port', 'address']);
});

test('what private.serve is sent is the endpoint and what is not the default', () => {
  assert.deepEqual(serveArgs(blankSetUp(' home.example.org ')), { endpoint: 'home.example.org' });
  assert.deepEqual(serveArgs({ endpoint: 'h', port: '40000', address: '10.9.0.1/24' }), { endpoint: 'h', port: 40_000, address: '10.9.0.1/24' });
  assert.deepEqual(openPort(40_000), { action: 'allow', protocol: 'udp', ports: '40000' });
});
