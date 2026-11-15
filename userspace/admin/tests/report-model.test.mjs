// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Firewall page says of what the machine reports (design 10.3): the
// head's pill, the rules and the lines that are none, what was dropped and
// what listens, and the answer of a test, from the shapes firewall's
// `--json` writes (systools/src/firewall/report.rs).

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  allowTextOf, blankTest, checkTest, dropRows, headPill, listenerRows, problemWords, reachWords, ruleRows, testWords, trialLeft,
  whyWords,
} from '../pages/firewall/report-model.js';
import { parseRule } from '../pages/firewall/rules-model.js';

const NOW = Date.UTC(2026, 10, 14, 14, 10);
const time = makeTime('UTC', () => NOW);

test('the pill is On while the rules are loaded, says so when they did not load, and is Off when the service is stopped', () => {
  const show = (state, extra = {}) => ({ services: [{ name: 'nginx', state: 'running' }, { name: 'firewall', state, ...extra }] });
  assert.deepEqual(headPill(show('done')), { words: 'On', kind: 'ok', explain: null });
  assert.equal(headPill(show('stopped')).words, 'Off');
  assert.equal(headPill(show('disabled')).words, 'Off');
  assert.deepEqual(headPill(show('failed', { why: 'firewall load: line 3 is not a rule' })), {
    words: 'The rules did not load', kind: 'bad', explain: 'firewall load: line 3 is not a rule',
  });
  assert.equal(headPill(show('failed', { last_end: 'exited with code 1' })).explain, 'exited with code 1');
  assert.equal(headPill(show('failed')).explain, null);
  assert.equal(headPill({ services: [] }), null);
  assert.equal(headPill(null), null);
});

const LISTED = {
  rules: [
    { number: 1, text: 'allow tcp 443 from 192.168.1.0/24', action: 'allow', protocol: 'tcp', ports: [443, 443], from: '192.168.1.0/24', on: null, program: null },
    { number: 2, text: 'deny program /users/alice/bin/x', action: 'deny', protocol: 'any', ports: null, from: null, on: null, program: '/users/alice/bin/x' },
  ],
  problems: [{ line: 7, text: 'allow tcp 99999', why: '`99999` is not a port, 1 to 65535, or a range of them' }],
  trial: { left_ms: 41_000 },
};

test('the rules are rows with their words, and the lines that are no rule say which line and why', () => {
  assert.deepEqual(ruleRows(LISTED), [
    { number: 1, words: 'Allow TCP port 443 from 192.168.1.0/24', text: 'allow tcp 443 from 192.168.1.0/24', program: null },
    { number: 2, words: 'Deny everything for the program /users/alice/bin/x', text: 'deny program /users/alice/bin/x', program: '/users/alice/bin/x' },
  ]);
  assert.deepEqual(problemWords(LISTED), [{
    line: 7, text: 'allow tcp 99999', words: 'Line 7 is not a rule: `99999` is not a port, 1 to 65535, or a range of them',
  }]);
  assert.deepEqual(ruleRows(null), []);
  assert.deepEqual(problemWords({}), []);
  assert.equal(trialLeft(LISTED), 41_000);
  assert.equal(trialLeft({ trial: null }), null);
});

const group = (over = {}) => ({
  from: '203.0.113.9', protocol: 'tcp', port: 22, card: 'net0', why: null, count: 14, first_ms: NOW - 3_600_000, last_ms: NOW - 8 * 60_000, ...over,
});

test('what was dropped is said by address, port and card, as at least, and the rule that would let it in is ready', () => {
  const rows = dropRows({ since_ms: 0, groups: [group(), group({ protocol: 'udp', port: 5353, count: 1, from: '10.0.0.1', card: 'br0' }), group({ protocol: 'ping', port: null, from: 'fe80::1', count: 3 })] }, time);
  assert.deepEqual(rows.blocked, []);
  assert.equal(rows.groups[0].words, '203.0.113.9 tried port 22 on net0 at least 14 times, last 8 minutes ago');
  assert.equal(rows.groups[0].allow, 'allow tcp 22 from 203.0.113.9');
  assert.equal(rows.groups[0].why, 'No rule lets it in, and no service listens there with the right to be reached.');
  assert.equal(rows.groups[0].full, 'Nov 14, 2026, 2:02:00 PM');
  assert.equal(rows.groups[1].words, '10.0.0.1 tried UDP port 5353 on br0 at least 1 time, last 8 minutes ago');
  assert.equal(rows.groups[1].allow, 'allow udp 5353 from 10.0.0.1');
  assert.equal(rows.groups[2].words, 'fe80::1 pinged net0 at least 3 times, last 8 minutes ago');
  assert.equal(rows.groups[2].allow, 'allow ping from fe80::1');
  assert.deepEqual(dropRows(null, time), { groups: [], blocked: [] });
});

test('the rule each group offers is one the rules model reads', () => {
  for (const one of [group(), group({ protocol: 'udp', port: 53, from: '2001:db8::9' }), group({ protocol: 'ping', port: null, from: '10.1.2.3' })]) {
    const found = parseRule(allowTextOf(one).split(' '));
    assert.ok(found.rule, `${allowTextOf(one)}: ${found.why}`);
  }
});

test('a block for guessing passwords is apart, and offers no rule', () => {
  const rows = dropRows({ groups: [group({ why: 'the address is blocked', count: 5 }), group({ from: '198.51.100.2' })] }, time);
  assert.equal(rows.blocked.length, 1);
  assert.equal(rows.blocked[0].allow, null);
  assert.match(rows.blocked[0].words, /^203\.0\.113\.9 tried port 22 on net0 at least 5 times/);
  assert.equal(rows.groups.length, 1);
});

test('what the kernel says after the card is told in words', () => {
  assert.equal(whyWords('process 312 listens for this machine alone'), 'A program (process 312) listens there for this machine alone, and no rule lets others reach it.');
  assert.equal(whyWords('something else'), 'Something else.');
  assert.equal(whyWords('stopped.'), 'Stopped.');
  assert.equal(whyWords(null), whyWords(''));
});

test('what listens says who reaches it, and by which rule', () => {
  const status = {
    listening: [
      { port: 9090, tcp: true, program: '/system/commands/admind', process: 312, reach: 'service', rule: null },
      { port: 22, tcp: true, program: '/system/commands/sshd', process: 40, reach: 'some', rule: 3 },
      { port: 5353, tcp: false, program: null, process: 41, reach: 'local', rule: null },
      { port: 8080, tcp: true, program: '/users/alice/bin/web', process: 77, reach: 'denied', rule: 2 },
      { port: 80, tcp: true, program: '/system/commands/nginx', process: 90, reach: 'everyone', rule: 1 },
    ],
  };
  const rows = listenerRows(status);
  assert.deepEqual(rows.map((row) => row.port), ['9090/TCP', '22/TCP', '5353/UDP', '8080/TCP', '80/TCP']);
  assert.equal(rows[0].program, '/system/commands/admind');
  assert.equal(rows[0].words, 'Its service has the right to be reached, so every machine does.');
  assert.equal(rows[1].words, 'Some machines or cards reach it by rule 3.');
  assert.equal(rows[2].program, 'a program that is not found');
  assert.equal(rows[2].words, 'This machine only.');
  assert.equal(rows[3].words, 'No other machine reaches it by rule 2.');
  assert.equal(rows[4].words, 'Every machine reaches it by rule 1.');
  assert.equal(new Set(rows.map((row) => row.key)).size, 5);
  assert.equal(reachWords('everyone', null), 'Every machine reaches it.');
  assert.deepEqual(listenerRows(null), []);
});

test('a test is checked before it is asked, and the first wrong value says which', () => {
  const form = (over = {}) => ({ ...blankTest(), port: '22', from: '192.168.1.20', ...over });
  assert.deepEqual(checkTest(form()), { args: { protocol: 'tcp', port: 22, from: '192.168.1.20' } });
  assert.deepEqual(checkTest(form({ protocol: 'udp', on: ' net1 ' })).args, { protocol: 'udp', port: 22, from: '192.168.1.20', on: 'net1' });
  assert.deepEqual(checkTest(form({ port: '' })), { field: 'port', why: 'Write a port, 1 to 65535.' });
  assert.equal(checkTest(form({ port: '0' })).why, '`0` is not a port, 1 to 65535.');
  assert.equal(checkTest(form({ port: '70000' })).field, 'port');
  assert.equal(checkTest(form({ port: '22-80' })).field, 'port');
  assert.equal(checkTest(form({ from: '' })).field, 'from');
  assert.equal(checkTest(form({ from: '192.168.1.0/24' })).why, '`192.168.1.0/24` is not an address');
  assert.equal(checkTest(form({ on: 'net 0' })).field, 'on');
});

test('the answer of a test says which rule decides, or that none does, and the rules passed over', () => {
  const rules = ['allow tcp 22 program /x', 'allow tcp 22 from 192.168.1.0/24'].map((text) => parseRule(text.split(' ')).rule);
  assert.deepEqual(testWords({ rule: 2, text: 'allow tcp 22 from 192.168.1.0/24', allowed: true, passed: [1] }, 22, rules), [
    'Rule 2 decides: allowed. allow tcp 22 from 192.168.1.0/24',
    'Passed over, since they name what a test does not know: rule 1 (allow tcp 22 program /x).',
  ]);
  assert.deepEqual(testWords({ rule: 4, text: 'deny tcp 22', allowed: false, passed: [] }, 22, []), ['Rule 4 decides: denied. deny tcp 22']);
  assert.deepEqual(testWords({ rule: null, text: null, allowed: null, passed: [] }, 8080, []), [
    'No rule decides: it reaches port 8080 only while a service listens there with the right to be reached.',
  ]);
});
